use std::{env, fs, path::Path, process::Command};

use tempfile::{Builder, TempDir};
use twigsmith::git;

/// Re-execute only this test in a clean child environment. Never mutate the
/// test runner's environment: tests may run concurrently.
pub fn isolated(name: &str, git_available: bool, test: impl FnOnce(&Path)) {
    if env::var("TWIGSMITH_ISOLATED_TEST").as_deref() == Ok(name) {
        let root = env::var_os("TWIGSMITH_TEST_ROOT").expect("isolated test root");
        test(Path::new(&root));
        return;
    }

    let root = Builder::new()
        .prefix("oneflow tests 'spaces' ")
        .tempdir()
        .expect("temporary test root");
    let home = root.path().join("home");
    let config = root.path().join("config");
    let empty_config = root.path().join("empty.gitconfig");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&config).unwrap();
    fs::write(&empty_config, "").unwrap();

    let mut child = Command::new(env::current_exe().unwrap());
    child.env_clear();
    for name in [
        "TWIGSMITH_TEST_BASH",
        "TWIGSMITH_TEST_ZSH",
        "TWIGSMITH_TEST_FISH",
        "TWIGSMITH_TEST_PWSH",
        "TWIGSMITH_TEST_ZSH_MODULE_PATH",
        "TWIGSMITH_TEST_ZSH_FUNCTION_PATH",
    ] {
        if let Some(value) = env::var_os(name) {
            child.env(name, value);
        }
    }
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATHEXT"] {
        if let Some(value) = env::var_os(name) {
            child.env(name, value);
        }
    }
    let output = child
        .args(["--exact", name, "--include-ignored", "--nocapture"])
        .current_dir(root.path())
        .env("TWIGSMITH_ISOLATED_TEST", name)
        .env("TWIGSMITH_TEST_ROOT", root.path())
        .env(
            "PATH",
            if git_available {
                env::var_os("PATH").unwrap_or_default()
            } else {
                // An existing empty directory gives deterministic lookup failure.
                home.as_os_str().to_owned()
            },
        )
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", &empty_config)
        .env("GIT_CONFIG_GLOBAL", &empty_config)
        .env("GIT_ATTR_NOSYSTEM", "1")
        // ./dev puts temporary directories inside the project checkout. Stop
        // discovery at the fixture parent instead of inspecting that checkout.
        .env("GIT_CEILING_DIRECTORIES", root.path().parent().unwrap())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .output()
        .expect("start isolated test process");
    assert!(
        output.status.success(),
        "isolated test {name}: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub struct Repository {
    directory: TempDir,
}

impl Repository {
    pub fn new(root: &Path) -> Self {
        let directory = Builder::new()
            .prefix("repository 'quotes' ;$ ")
            .tempdir_in(root)
            .unwrap();
        let repository = Self { directory };
        let template = root.join("empty-template");
        let hooks = root.join("empty-hooks");
        fs::create_dir_all(&template).unwrap();
        fs::create_dir_all(&hooks).unwrap();
        repository.success(&[
            "init".as_ref(),
            "--initial-branch=main".as_ref(),
            "--template".as_ref(),
            template.as_os_str(),
            ".".as_ref(),
        ]);
        repository.success(&["config", "user.name", "Fixture User"]);
        repository.success(&["config", "user.email", "fixture@example.invalid"]);
        repository.success(&[
            "config".as_ref(),
            "core.hooksPath".as_ref(),
            hooks.as_os_str(),
        ]);
        repository
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn success<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> std::process::Output {
        let output = git::run(self.path(), args).expect("start fixture Git");
        assert!(
            output.status.success(),
            "fixture Git: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}
