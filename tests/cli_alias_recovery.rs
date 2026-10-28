//! Public lifecycle and interrupted direct delivery across executable names.
mod support;

use std::{
    fs,
    process::{Command, Stdio},
};
use support::{Repository, isolated};

fn run(repo: &Repository, executable: &str, args: &[&str], success: bool) -> String {
    let output = Command::new(executable)
        .current_dir(repo.path())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.success(), success, "{args:?}: {output:?}");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn git_value(repo: &Repository, args: &[&str]) -> String {
    String::from_utf8(repo.success(args).stdout)
        .unwrap()
        .trim()
        .to_owned()
}

#[cfg(unix)]
#[test]
fn aliases_share_workstream_configuration_and_interrupted_delivery() {
    isolated(
        "aliases_share_workstream_configuration_and_interrupted_delivery",
        true,
        |root| {
            let remote = Repository::new(root);
            fs::write(remote.path().join("base"), "base\n").unwrap();
            remote.success(&["add", "base"]);
            remote.success(&["commit", "-m", "base"]);
            remote.success(&["config", "receive.denyCurrentBranch", "ignore"]);
            let local = Repository::new(root);
            local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
            local.success(&["fetch", "origin", "main"]);
            local.success(&["reset", "--hard", "FETCH_HEAD"]);
            fs::write(local.path().join(".git/twigsmith.config.toml"), "[[finish-checks]]\nname='Alias gate'\ncommand=['sh','-c','echo check >> .git/alias-check-count; test -f .git/alias-check-pass']\n").unwrap();

            let long = env!("CARGO_BIN_EXE_twigsmith");
            let short = env!("CARGO_BIN_EXE_tgi");
            run(&local, long, &["start", "work/alias"], true);
            fs::write(local.path().join("feature"), "saved through alias\n").unwrap();
            run(
                &local,
                short,
                &["save", "--commit", "unstaged", "--message", "Alias work"],
                true,
            );
            let work = git_value(&local, &["rev-parse", "HEAD"]);
            assert_eq!(
                git_value(&remote, &["rev-parse", "refs/heads/work/alias"]),
                work
            );
            let base = git_value(&remote, &["rev-parse", "main"]);
            let first = run(
                &local,
                long,
                &[
                    "finish",
                    "--mode",
                    "integrate",
                    "--update",
                    "none",
                    "--integration",
                    "ff-only",
                    "--cleanup",
                    "both",
                ],
                false,
            );
            assert!(first.contains("Alias gate"), "{first}");
            assert_eq!(git_value(&remote, &["rev-parse", "main"]), base);
            assert!(local.path().join(".git/twigsmith-integration").exists());
            fs::write(local.path().join(".git/alias-check-pass"), "").unwrap();
            run(&local, short, &["finish", "--mode", "integrate"], true);
            assert_eq!(git_value(&remote, &["rev-parse", "main"]), work);
            assert_eq!(git_value(&local, &["branch", "--show-current"]), "main");
            assert_eq!(
                fs::read_to_string(local.path().join(".git/alias-check-count")).unwrap(),
                "check\ncheck\n"
            );
            assert_eq!(
                local.success(&["show", "main:feature"]).stdout,
                b"saved through alias\n"
            );
            assert!(
                !twigsmith::git::run(
                    remote.path(),
                    ["rev-parse", "--verify", "refs/heads/work/alias"]
                )
                .unwrap()
                .status
                .success()
            );
        },
    );
}
