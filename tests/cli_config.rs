#[path = "support/snapshot.rs"]
mod preservation;
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
mod terminal;

use git_oneflow::{
    config::{self, ConfigError, Format},
    git, repository,
    settings::{self, CONFIG_FILES, SaveAction, SettingSource, Settings},
};
use preservation::snapshot;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};

fn cases(name: &str, git_available: bool, test: impl Fn(&Path, &str)) {
    isolated(name, git_available, |root| {
        for executable in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
            let fixture = tempfile::tempdir_in(root).unwrap();
            test(fixture.path(), executable);
        }
    });
}
fn run(executable: &str, directory: &Path, args: &[&str]) -> Output {
    Command::new(executable)
        .current_dir(directory)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn check(output: Output, code: i32, expected: &[&str]) -> String {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    let text = String::from_utf8(if code == 0 {
        assert!(output.stderr.is_empty(), "{output:?}");
        output.stdout
    } else {
        assert!(output.stdout.is_empty(), "{output:?}");
        output.stderr
    })
    .unwrap();
    for expected in expected {
        assert!(text.contains(expected), "missing {expected:?}: {text}");
    }
    text
}
fn load(directory: &Path) -> Settings {
    settings::load(
        directory,
        &repository::discover(directory).unwrap().unwrap(),
    )
    .unwrap()
}
fn commit(repo: &Repository) {
    fs::write(repo.path().join("tracked"), "initial\n").unwrap();
    repo.success(&["add", "tracked"]);
    repo.success(&["commit", "-m", "fixture"]);
}
fn assert_defaults(directory: &Path, filename: &str) {
    let loaded = load(directory);
    let defaults = Settings::default();
    assert_eq!(loaded.remote, defaults.remote);
    assert_eq!(loaded.base, defaults.base);
    assert_eq!(loaded.save_action, defaults.save_action);
    assert_eq!(loaded.finish_update, defaults.finish_update);
    assert_eq!(loaded.finish_rewrite, defaults.finish_rewrite);
    let source = SettingSource::Shared(directory.join(filename));
    assert_eq!(loaded.sources.remote, source);
    assert_eq!(loaded.sources.base, source);
    assert_eq!(loaded.sources.save_action, source);
    assert_eq!(loaded.sources.finish_update, source);
    assert_eq!(loaded.sources.finish_rewrite, source);
}

#[test]
fn config_help_terminal_requirement_and_syntax_need_no_git() {
    cases(
        "config_help_terminal_requirement_and_syntax_need_no_git",
        false,
        |root, exe| {
            let before = snapshot(root);
            for flag in ["--help", "-h"] {
                check(
                    run(exe, root, &["config", flag]),
                    0,
                    &[
                        "Usage:",
                        "Arguments:",
                        "Options:",
                        "Examples:",
                        "--yes",
                        "--format",
                        "show",
                        "Exit status:",
                    ],
                );
            }
            check(
                run(exe, root, &["config"]),
                1,
                &["terminal", "config --yes"],
            );
            for args in [
                vec!["config", "--yes", "--format"],
                vec!["config", "--yes", "--format", "yml"],
                vec!["config", "--yes", "--format", "TOML"],
                vec!["config", "--yes", "-y"],
                vec!["config", "--yes", "--format", "json", "--format", "toml"],
                vec!["config", "show", "--yes"],
                vec!["config", "show", "--format", "json"],
                vec!["config", "--help", "--yes"],
                vec!["config", "--format=json", "--yes"],
                vec!["config", "--remote", "origin"],
                vec!["config", "unknown"],
            ] {
                check(run(exe, root, &args), 2, &["use config --help"]);
            }
            check(run(exe, root, &["config", "--yes"]), 1, &["Git", "PATH"]);
            check(run(exe, root, &["config", "show"]), 1, &["Git", "PATH"]);
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn unattended_formats_generate_defaults_from_nested_unborn_worktrees() {
    cases(
        "unattended_formats_generate_defaults_from_nested_unborn_worktrees",
        true,
        |root, exe| {
            for (args, filename) in [
                (vec!["config", "--yes"], CONFIG_FILES[0]),
                (vec!["config", "-y"], CONFIG_FILES[0]),
                (vec!["config", "--yes", "--format", "toml"], CONFIG_FILES[0]),
                (vec!["config", "--yes", "--format", "yaml"], CONFIG_FILES[1]),
                (vec!["config", "--format", "json", "-y"], CONFIG_FILES[3]),
            ] {
                let repo = Repository::new(root);
                let nested = repo.path().join("nested/deep");
                fs::create_dir_all(&nested).unwrap();
                // Git settings deliberately differ; there is no remote or base ref.
                repo.success(&["config", "init.defaultBranch", "trunk"]);
                repo.success(&["config", "oneflow.remote", "private"]);
                let before_git = snapshot(&repo.path().join(".git"));
                check(
                    run(exe, &nested, &args),
                    0,
                    &[
                        "Created shared configuration",
                        &repo.path().join(filename).display().to_string(),
                    ],
                );
                assert_defaults(repo.path(), filename);
                assert_eq!(snapshot(&repo.path().join(".git")), before_git);
                assert!(fs::read_dir(&nested).unwrap().next().is_none());
                assert_eq!(fs::read_dir(repo.path()).unwrap().count(), 3);
                check(
                    run(exe, &nested, &["config", "show"]),
                    0,
                    &[
                        "remote: origin",
                        "base: main",
                        "save-action: ask",
                        "shared:",
                        filename,
                    ],
                );
            }
        },
    );
}

#[test]
fn inspection_reports_sources_and_locations_read_only() {
    cases(
        "inspection_reports_sources_and_locations_read_only",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            let nested = repo.path().join("nested");
            fs::create_dir(&nested).unwrap();
            let before = snapshot(repo.path());
            check(
                run(exe, &nested, &["config", "show"]),
                0,
                &[
                    "remote: origin (built-in default)",
                    "base: main (built-in default)",
                    "save-action: ask (built-in default)",
                    "finish-update: ask (built-in default)",
                    "finish-rewrite: ask (built-in default)",
                    "Shared file: none",
                    "Local file: none",
                    &repo.path().display().to_string(),
                    &repo.path().join(".git").display().to_string(),
                    "Precedence per key",
                ],
            );
            assert_eq!(snapshot(repo.path()), before);
            fs::write(
                repo.path().join(CONFIG_FILES[2]),
                "base: trunk\nremote: upstream\n",
            )
            .unwrap();
            let local = repo.path().join(".git/oneflow.conf");
            fs::write(&local, "remote = private\n").unwrap();
            let before = snapshot(repo.path());
            check(
                run(exe, &nested, &["config", "show"]),
                0,
                &[
                    "remote: private (local:",
                    "base: trunk (shared:",
                    "save-action: ask (built-in default)",
                    "finish-update: ask (built-in default)",
                    "finish-rewrite: ask (built-in default)",
                    &local.display().to_string(),
                    CONFIG_FILES[2],
                ],
            );
            check(run(exe, &nested, &["config"]), 1, &["config --yes"]);
            assert_eq!(snapshot(repo.path()), before);
            fs::write(&local, "save-action = invalid\n").unwrap();
            check(
                run(exe, &nested, &["config", "show"]),
                1,
                &["invalid settings", "oneflow.conf", "save-action"],
            );
            fs::remove_file(&local).unwrap();
            fs::write(repo.path().join(CONFIG_FILES[0]), "").unwrap();
            check(
                run(exe, &nested, &["config", "show"]),
                1,
                &[
                    "multiple configuration files",
                    CONFIG_FILES[0],
                    CONFIG_FILES[2],
                ],
            );
        },
    );
}

#[test]
fn generation_preserves_dirty_work_index_refs_and_git_configuration() {
    cases(
        "generation_preserves_dirty_work_index_refs_and_git_configuration",
        true,
        |root, exe| {
            for detached in [false, true] {
                let repo = Repository::new(root);
                commit(&repo);
                if detached {
                    repo.success(&["switch", "--detach"]);
                }
                fs::write(repo.path().join("tracked"), "staged\n").unwrap();
                repo.success(&["add", "tracked"]);
                fs::write(repo.path().join("tracked"), "unstaged\n").unwrap();
                fs::write(repo.path().join("untracked"), "keep\n").unwrap();
                fs::write(repo.path().join(".gitignore"), "ignored\n").unwrap();
                fs::write(repo.path().join("ignored"), "keep ignored\n").unwrap();
                let before = snapshot(repo.path());
                let refs = repo.success(&["show-ref"]).stdout;
                let index = repo.success(&["ls-files", "--stage", "-z"]).stdout;
                check(run(exe, repo.path(), &["config", "--yes"]), 0, &["Created"]);
                assert_defaults(repo.path(), CONFIG_FILES[0]);
                let mut after = snapshot(repo.path());
                let mut expected = before;
                // Only the root directory metadata and the newly created file change.
                after.remove(Path::new(CONFIG_FILES[0]));
                after.remove(Path::new(""));
                expected.remove(Path::new(""));
                assert_eq!(after, expected);
                assert_eq!(repo.success(&["show-ref"]).stdout, refs);
                assert_eq!(repo.success(&["ls-files", "--stage", "-z"]).stdout, index);
            }
        },
    );
}

#[test]
fn every_shared_extension_prevents_generation_without_modification() {
    cases(
        "every_shared_extension_prevents_generation_without_modification",
        true,
        |root, exe| {
            for filename in CONFIG_FILES {
                let repo = Repository::new(root);
                let path = repo.path().join(filename);
                // Invalid content and even empty files still constitute an existing config.
                fs::write(&path, "existing invalid configuration").unwrap();
                let before = snapshot(repo.path());
                for args in [
                    vec!["config", "--yes"],
                    vec!["config", "-y", "--format", "json"],
                ] {
                    check(
                        run(exe, repo.path(), &args),
                        1,
                        &["shared configuration already exists", filename],
                    );
                    assert_eq!(snapshot(repo.path()), before);
                }
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
                let before = snapshot(repo.path());
                check(
                    run(exe, repo.path(), &["config", "--yes"]),
                    1,
                    &["already exists", filename],
                );
                assert_eq!(snapshot(repo.path()), before);
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn shared_symlinks_including_dangling_links_are_preserved() {
    cases(
        "shared_symlinks_including_dangling_links_are_preserved",
        true,
        |root, exe| {
            for filename in CONFIG_FILES {
                for dangling in [false, true] {
                    let repo = Repository::new(root);
                    let target = repo.path().join("target");
                    if !dangling {
                        fs::write(&target, "keep").unwrap();
                    }
                    std::os::unix::fs::symlink(&target, repo.path().join(filename)).unwrap();
                    let before = snapshot(repo.path());
                    check(
                        run(exe, repo.path(), &["config", "--yes"]),
                        1,
                        &["already exists", filename],
                    );
                    assert_eq!(snapshot(repo.path()), before);
                    if dangling {
                        check(
                            run(exe, repo.path(), &["config", "show"]),
                            1,
                            &["cannot read settings", filename],
                        );
                        assert_eq!(snapshot(repo.path()), before);
                    }
                }
            }
        },
    );
}

#[test]
fn private_overrides_allow_generation_without_copying_values() {
    cases(
        "private_overrides_allow_generation_without_copying_values",
        true,
        |root, exe| {
            for (filename, content) in [
                (CONFIG_FILES[0], "remote='private'\nsave-action='stash'\n"),
                ("oneflow.conf", "remote = private\nsave-action = stash\n"),
                (CONFIG_FILES[3], "invalid JSON"),
            ] {
                let repo = Repository::new(root);
                let local = repo.path().join(".git").join(filename);
                fs::write(&local, content).unwrap();
                let before_git = snapshot(&repo.path().join(".git"));
                check(
                    run(exe, repo.path(), &["config", "--yes"]),
                    0,
                    &[
                        "Local overrides",
                        "take precedence",
                        &local.display().to_string(),
                    ],
                );
                assert_eq!(snapshot(&repo.path().join(".git")), before_git);
                assert_eq!(
                    fs::read_to_string(repo.path().join(CONFIG_FILES[0])).unwrap(),
                    config::render_defaults(Format::Toml)
                );
                if filename != CONFIG_FILES[3] {
                    assert_eq!(load(repo.path()).remote, "private");
                    assert_eq!(load(repo.path()).save_action, SaveAction::Stash);
                    assert_eq!(load(repo.path()).base, "main");
                } else {
                    check(
                        run(exe, repo.path(), &["config", "show"]),
                        1,
                        &["invalid settings"],
                    );
                }
            }
        },
    );
}

#[test]
fn linked_worktrees_generate_at_their_root_and_inspect_common_overrides() {
    cases(
        "linked_worktrees_generate_at_their_root_and_inspect_common_overrides",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            commit(&repo);
            fs::write(repo.path().join(CONFIG_FILES[0]), "base='trunk'\n").unwrap();
            fs::write(
                repo.path().join(".git").join(CONFIG_FILES[1]),
                "remote: private\n",
            )
            .unwrap();
            let linked = root.join("linked");
            repo.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "--detach".as_ref(),
                linked.as_os_str(),
            ]);
            let nested = linked.join("nested");
            fs::create_dir(&nested).unwrap();
            let before = snapshot(repo.path());
            check(
                run(exe, &nested, &["config", "--yes", "--format", "yaml"]),
                0,
                &[
                    &linked.join(CONFIG_FILES[1]).display().to_string(),
                    &repo
                        .path()
                        .join(".git")
                        .join(CONFIG_FILES[1])
                        .display()
                        .to_string(),
                    "take precedence",
                ],
            );
            assert_eq!(snapshot(repo.path()), before);
            assert_eq!(load(&nested).remote, "private");
            assert_eq!(load(&nested).base, "main");
            check(
                run(exe, &nested, &["config", "show"]),
                0,
                &[
                    "remote: private (local:",
                    "base: main (shared:",
                    &linked.join(CONFIG_FILES[1]).display().to_string(),
                ],
            );
        },
    );
}

#[test]
fn outside_and_bare_generation_fail_but_bare_inspection_works() {
    cases(
        "outside_and_bare_generation_fail_but_bare_inspection_works",
        true,
        |root, exe| {
            let before = snapshot(root);
            for command in ["--yes", "show"] {
                check(
                    run(exe, root, &["config", command]),
                    1,
                    &["outside a Git repository"],
                );
            }
            assert_eq!(snapshot(root), before);
            let bare = root.join("bare.git");
            assert!(
                git::run(root, ["init".as_ref(), "--bare".as_ref(), bare.as_os_str()])
                    .unwrap()
                    .status
                    .success()
            );
            fs::write(bare.join(CONFIG_FILES[3]), "{\"base\":\"trunk\"}").unwrap();
            let before = snapshot(&bare);
            check(
                run(exe, &bare, &["config", "--yes"]),
                1,
                &["requires a Git working tree", "bare"],
            );
            check(
                run(exe, &bare, &["config", "show"]),
                0,
                &[
                    "base: trunk (local:",
                    "Shared file: unavailable",
                    "remote: origin (built-in default)",
                ],
            );
            assert_eq!(snapshot(&bare), before);
        },
    );
}

#[test]
fn stale_generation_plans_recheck_context_and_all_shared_extensions() {
    isolated(
        "stale_generation_plans_recheck_context_and_all_shared_extensions",
        true,
        |root| {
            let first = Repository::new(root);
            let second = Repository::new(root);
            let plan = config::prepare(first.path(), Format::Toml).unwrap();
            let before = snapshot(root);
            assert!(matches!(
                config::generate(second.path(), &plan),
                Err(ConfigError::ContextChanged)
            ));
            assert_eq!(snapshot(root), before);
            fs::write(first.path().join(CONFIG_FILES[2]), "concurrently created").unwrap();
            let before = snapshot(root);
            assert!(matches!(
                config::generate(first.path(), &plan),
                Err(ConfigError::ExistingShared(_))
            ));
            assert_eq!(snapshot(root), before);
        },
    );
}

#[cfg(unix)]
fn git_wrapper(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::{env, os::unix::fs::PermissionsExt};
    let real_git = env::split_paths(&env::var_os("PATH").unwrap())
        .map(|p| p.join("git"))
        .find(|p| p.is_file())
        .unwrap()
        .canonicalize()
        .unwrap();
    let wrapper = root.join("git-wrapper");
    fs::create_dir(&wrapper).unwrap();
    let script = wrapper.join("git");
    fs::write(&script, r#"#!/bin/sh
printf '%s\n' "$*" >> "$ONEFLOW_TEST_LOG"
for staged in "$ONEFLOW_TEST_REPO"/.git-oneflow-config-*.tmp; do
    if [ -f "$staged" ]; then
        case "$ONEFLOW_TEST_MODE" in
            collision) printf '%s\n' 'concurrent shared file' > "$ONEFLOW_TEST_REPO/git-oneflow.config.yml" ;;
            failure) echo 'injected discovery failure after staging' >&2; exit 99 ;;
            context) exec "$ONEFLOW_REAL_GIT" -C "$ONEFLOW_OTHER_REPO" "$@" ;;
            interrupt) printf '%s\n' "$$" > "$ONEFLOW_TEST_PID"; exec sleep 30 ;;
        esac
        break
    fi
done
exec "$ONEFLOW_REAL_GIT" "$@"
"#).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    (wrapper, real_git)
}
#[cfg(unix)]
fn wrapped_command(
    exe: &str,
    repo: &Path,
    root: &Path,
    wrapper: &Path,
    real_git: &Path,
    mode: &str,
) -> Command {
    let mut command = Command::new(exe);
    let mut paths = vec![wrapper.to_owned()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    command
        .current_dir(repo)
        .args(["config", "--yes"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("ONEFLOW_REAL_GIT", real_git)
        .env("ONEFLOW_TEST_REPO", repo)
        .env("ONEFLOW_TEST_MODE", mode)
        .env("ONEFLOW_TEST_LOG", root.join("git-commands"))
        .env("ONEFLOW_TEST_PID", root.join("blocked-git.pid"));
    command
}

#[cfg(unix)]
#[test]
fn generation_rechecks_after_staging_and_cleans_up_on_failure() {
    cases(
        "generation_rechecks_after_staging_and_cleans_up_on_failure",
        true,
        |root, exe| {
            let (wrapper, real_git) = git_wrapper(root);
            for mode in ["collision", "failure", "context"] {
                let repo = Repository::new(root);
                let other = Repository::new(root);
                let before_git = snapshot(&repo.path().join(".git"));
                let mut command =
                    wrapped_command(exe, repo.path(), root, &wrapper, &real_git, mode);
                command.env("ONEFLOW_OTHER_REPO", other.path());
                let expected = match mode {
                    "collision" => "shared configuration already exists",
                    "failure" => "injected discovery failure after staging",
                    "context" => "repository context changed",
                    _ => unreachable!(),
                };
                check(command.output().unwrap(), 1, &[expected]);
                assert!(!repo.path().join(CONFIG_FILES[0]).exists());
                assert_eq!(snapshot(&repo.path().join(".git")), before_git);
                let remaining: Vec<_> = fs::read_dir(repo.path())
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                assert!(
                    remaining
                        .iter()
                        .all(|n| n == ".git" || (mode == "collision" && n == CONFIG_FILES[2]))
                );
                if mode == "collision" {
                    assert_eq!(
                        fs::read_to_string(repo.path().join(CONFIG_FILES[2])).unwrap(),
                        "concurrent shared file\n"
                    );
                }
            }
            // Generation's Git boundary performs discovery only, without a fetch,
            // config write, status/index refresh, or any remote command.
            for call in fs::read_to_string(root.join("git-commands"))
                .unwrap()
                .lines()
            {
                assert!(
                    call.contains(" rev-parse ") || call.contains(" check-ref-format "),
                    "unexpected Git call: {call}"
                );
                assert!(
                    call.starts_with(
                        "--no-optional-locks --no-lazy-fetch -c core.fsmonitor=false "
                    )
                );
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn interruption_before_publication_exposes_no_recognized_configuration() {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    cases(
        "interruption_before_publication_exposes_no_recognized_configuration",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            let before_git = snapshot(&repo.path().join(".git"));
            let (wrapper, real_git) = git_wrapper(root);
            let mut child =
                wrapped_command(exe, repo.path(), root, &wrapper, &real_git, "interrupt")
                    .spawn()
                    .unwrap();
            let marker = root.join("blocked-git.pid");
            let deadline = Instant::now() + Duration::from_secs(5);
            let pid = loop {
                if let Ok(pid) = fs::read_to_string(&marker)
                    && !pid.trim().is_empty()
                {
                    break pid;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    panic!("generation did not reach post-staging discovery");
                }
                thread::sleep(Duration::from_millis(10));
            };
            child.kill().unwrap();
            assert!(
                Command::new("kill")
                    .args(["-KILL", pid.trim()])
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(!child.wait_with_output().unwrap().status.success());
            for filename in CONFIG_FILES {
                assert!(fs::symlink_metadata(repo.path().join(filename)).is_err());
            }
            assert_eq!(snapshot(&repo.path().join(".git")), before_git);
            // Signal termination may retain a hidden temporary file, never a partial
            // recognized configuration. Its complete contents are safe to discard.
            for entry in fs::read_dir(repo.path()).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name() != ".git" {
                    assert!(
                        entry
                            .file_name()
                            .to_str()
                            .unwrap()
                            .starts_with(".git-oneflow-config-")
                    );
                    assert_eq!(
                        fs::read_to_string(entry.path()).unwrap(),
                        config::render_defaults(Format::Toml)
                    );
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn unwritable_root_reports_write_failure_without_changing_existing_files() {
    use std::os::unix::fs::PermissionsExt;
    cases(
        "unwritable_root_reports_write_failure_without_changing_existing_files",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            fs::write(repo.path().join("work"), "keep").unwrap();
            let original = fs::metadata(repo.path()).unwrap().permissions();
            fs::set_permissions(repo.path(), fs::Permissions::from_mode(0o555)).unwrap();
            let probe = repo.path().join("permission-probe");
            // Privileged test runners may bypass directory permissions.
            if fs::File::create(&probe).is_ok() {
                fs::set_permissions(repo.path(), original).unwrap();
                fs::remove_file(probe).unwrap();
                return;
            }
            let before = snapshot(repo.path());
            let output = run(exe, repo.path(), &["config", "--yes"]);
            let after = snapshot(repo.path());
            fs::set_permissions(repo.path(), original).unwrap();
            check(
                output,
                1,
                &[
                    "cannot create configuration",
                    CONFIG_FILES[0],
                    "Permission denied",
                ],
            );
            assert_eq!(after, before);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_formats_defaults_custom_values_and_private_precedence() {
    cases(
        "guided_formats_defaults_custom_values_and_private_precedence",
        true,
        |root, exe| {
            for (format, filename) in [
                ("toml", CONFIG_FILES[0]),
                ("yaml", CONFIG_FILES[1]),
                ("json", CONFIG_FILES[3]),
            ] {
                for custom in [false, true] {
                    let repo = Repository::new(root);
                    repo.success(&["remote", "add", "hint-only", "/unreachable/offline"]);
                    let private = repo.path().join(".git/oneflow.conf");
                    fs::write(&private, "remote = private\nsave-action = stash\n").unwrap();
                    let before_git = snapshot(&repo.path().join(".git"));
                    let mut terminal = terminal::Terminal::start(
                        exe,
                        repo.path(),
                        &["config", "--format", format],
                    );
                    terminal.until("Remote used");
                    assert!(terminal.output.contains("hint-only"));
                    terminal.send(if custom {
                        b"future-remote\nfuture/base\nwip\n"
                    } else {
                        b"\n\n\n"
                    });
                    terminal.until("Create this file?");
                    assert!(!repo.path().join(filename).exists());
                    assert!(terminal.output.contains("Effective remote: private"));
                    assert!(terminal.output.contains("Effective save-action: stash"));
                    let proposed = Settings {
                        remote: if custom { "future-remote" } else { "origin" }.into(),
                        base: if custom { "future/base" } else { "main" }.into(),
                        save_action: if custom {
                            SaveAction::Wip
                        } else {
                            SaveAction::Ask
                        },
                        ..Settings::default()
                    };
                    let contents = config::render(
                        match format {
                            "toml" => Format::Toml,
                            "yaml" => Format::Yaml,
                            _ => Format::Json,
                        },
                        &proposed,
                    );
                    assert!(terminal.output.replace('\r', "").contains(&contents));
                    terminal.send(b"yes\n");
                    assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                    assert_eq!(
                        fs::read_to_string(repo.path().join(filename)).unwrap(),
                        contents
                    );
                    assert_eq!(snapshot(&repo.path().join(".git")), before_git);
                    let effective = load(repo.path());
                    assert_eq!(effective.remote, "private");
                    assert_eq!(effective.base, proposed.base);
                    assert_eq!(effective.save_action, SaveAction::Stash);
                    assert_eq!(
                        git_oneflow::start::prepare(repo.path(), "work")
                            .unwrap()
                            .settings,
                        effective
                    );
                    commit(&repo);
                    repo.success(&["switch", "-c", "work"]);
                    repo.success(&["remote", "add", "private", "/offline"]);
                    let save = git_oneflow::save::assess(repo.path(), &Default::default()).unwrap();
                    assert_eq!(save.settings, effective);
                    assert_eq!(save.action, SaveAction::Stash);
                    fs::remove_file(private).unwrap();
                    let effective = load(repo.path());
                    assert_eq!(effective.remote, proposed.remote);
                    assert_eq!(effective.save_action, proposed.save_action);
                    repo.success(&["remote", "add", &proposed.remote, "/offline"]);
                    let save = git_oneflow::save::assess(repo.path(), &Default::default()).unwrap();
                    assert_eq!(save.settings, effective);
                    assert_eq!(save.action, proposed.save_action);
                    assert!(matches!(
                        git_oneflow::start::prepare(repo.path(), &proposed.base),
                        Err(git_oneflow::start::ResolveError::ProtectedBase(_))
                    ));
                }
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_invalid_answers_retry_and_confirmation_is_explicit() {
    cases(
        "guided_invalid_answers_retry_and_confirmation_is_explicit",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
            terminal.until("Remote used");
            terminal.send(b"bad remote\n");
            terminal.until("Use a valid named Git remote");
            terminal.send(b"upstream\nHEAD\n");
            terminal.until("Use a valid literal Git branch");
            terminal.send(b"trunk\nunknown\n");
            terminal.until("save-action must be");
            terminal.send(b"staged\n");
            terminal.until("Create this file?");
            terminal.send(b"y\n");
            terminal.until("Enter yes to create");
            assert!(!repo.path().join(CONFIG_FILES[0]).exists());
            terminal.send(b"yes\n");
            assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
            let settings = load(repo.path());
            assert_eq!(settings.remote, "upstream");
            assert_eq!(settings.base, "trunk");
            assert_eq!(settings.save_action, SaveAction::Staged);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_cancel_eof_and_declined_confirmation_preserve_repository() {
    cases(
        "guided_cancel_eof_and_declined_confirmation_preserve_repository",
        true,
        |root, exe| {
            for (answers, prompt) in [
                ("", "Remote used"),
                ("\n", "Base branch"),
                ("\n\n", "Default save action"),
                ("\n\n\n", "Create this file?"),
            ] {
                for cancel in [b"cancel\n".as_slice(), b"\x04", b"partial\x04\x04"] {
                    let repo = Repository::new(root);
                    let before = snapshot(repo.path());
                    let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
                    terminal.until("Remote used");
                    terminal.send(answers.as_bytes());
                    terminal.until(prompt);
                    terminal.send(cancel);
                    assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                    assert!(terminal.output.contains("Setup cancelled"));
                    assert_eq!(snapshot(repo.path()), before);
                }
            }
            let repo = Repository::new(root);
            let before = snapshot(repo.path());
            let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
            terminal.until("Remote used");
            terminal.interrupt();
            assert!(!terminal.finish().success());
            assert_eq!(snapshot(repo.path()), before);
            for answer in [b"no\n".as_slice(), b"\n"] {
                let repo = Repository::new(root);
                let before = snapshot(repo.path());
                let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
                terminal.until("Remote used");
                terminal.send(b"\n\n\n");
                terminal.until("Create this file?");
                terminal.send(answer);
                assert_eq!(terminal.finish().code(), Some(0));
                assert_eq!(snapshot(repo.path()), before);
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_rechecks_changes_during_answers_and_confirmation() {
    cases(
        "guided_rechecks_changes_during_answers_and_confirmation",
        true,
        |root, exe| {
            for confirmation in [false, true] {
                for mutation in [
                    "shared",
                    "private",
                    "private-change",
                    "private-invalid",
                    "private-remove",
                    "remote",
                    "branch",
                    "context",
                ] {
                    let repo = Repository::new(root);
                    commit(&repo);
                    if mutation.starts_with("private-") {
                        fs::write(repo.path().join(".git/oneflow.conf"), "base = main\n").unwrap();
                    }
                    let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
                    terminal.until("Remote used");
                    if confirmation {
                        terminal.send(b"\n\n\n");
                        terminal.until("Create this file?");
                    }
                    match mutation {
                        "shared" => {
                            fs::write(repo.path().join(CONFIG_FILES[2]), "base: trunk\n").unwrap()
                        }
                        "private" => {
                            fs::write(repo.path().join(".git/oneflow.conf"), "base = trunk\n")
                                .unwrap()
                        }
                        "private-change" => {
                            fs::write(repo.path().join(".git/oneflow.conf"), "base = trunk\n")
                                .unwrap();
                        }
                        "private-invalid" => {
                            fs::write(repo.path().join(".git/oneflow.conf"), "unknown = value\n")
                                .unwrap();
                        }
                        "private-remove" => {
                            fs::remove_file(repo.path().join(".git/oneflow.conf")).unwrap();
                        }
                        "remote" => {
                            repo.success(&["remote", "add", "new", "/offline"]);
                        }
                        "branch" => {
                            repo.success(&["branch", "new"]);
                        }
                        "context" => {
                            let other = root.join("other-worktree");
                            fs::create_dir_all(&other).unwrap();
                            repo.success(&["config", "core.worktree", other.to_str().unwrap()]);
                        }
                        _ => unreachable!(),
                    }
                    let before = snapshot(repo.path());
                    terminal.send(if confirmation { b"yes\n" } else { b"\n\n\n" });
                    assert_eq!(terminal.finish().code(), Some(1), "{}", terminal.output);
                    assert_eq!(snapshot(repo.path()), before);
                    assert!(!repo.path().join(CONFIG_FILES[0]).exists());
                }
            }
        },
    );
}

#[test]
fn piped_answers_cannot_approve_guided_setup() {
    use std::io::Write;
    cases(
        "piped_answers_cannot_approve_guided_setup",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            let before = snapshot(repo.path());
            for args in [vec!["config"], vec!["config", "--format", "yaml"]] {
                let mut child = Command::new(exe)
                    .current_dir(repo.path())
                    .args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                let _ = child.stdin.take().unwrap().write_all(b"\n\n\nyes\n");
                check(child.wait_with_output().unwrap(), 1, &["terminal", "--yes"]);
                assert_eq!(snapshot(repo.path()), before);
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_existing_files_fail_before_prompt_and_yes_never_prompts() {
    cases(
        "guided_existing_files_fail_before_prompt_and_yes_never_prompts",
        true,
        |root, exe| {
            for filename in CONFIG_FILES {
                let repo = Repository::new(root);
                fs::write(repo.path().join(filename), "existing").unwrap();
                let before = snapshot(repo.path());
                let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
                assert_eq!(terminal.finish().code(), Some(1));
                assert!(terminal.output.contains("config show"));
                assert!(!terminal.output.contains("Remote used"));
                assert_eq!(snapshot(repo.path()), before);
            }
            let repo = Repository::new(root);
            let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config", "--yes"]);
            assert_eq!(terminal.finish().code(), Some(0));
            assert!(!terminal.output.contains("Remote used"));
            assert!(!terminal.output.contains("Create this file?"));
            assert_defaults(repo.path(), CONFIG_FILES[0]);
        },
    );
}

#[test]
fn config_show_reports_effective_finish_values_and_sources() {
    cases(
        "config_show_reports_effective_finish_values_and_sources",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            fs::write(
                repo.path().join(CONFIG_FILES[0]),
                "finish-update='merge'\nfinish-rewrite='deny'\n",
            )
            .unwrap();
            for filename in [CONFIG_FILES[3], "oneflow.conf"] {
                let local = repo.path().join(".git").join(filename);
                let contents = if filename == "oneflow.conf" {
                    "finish-rewrite=allow\n"
                } else {
                    "{\"finish-rewrite\":\"allow\"}"
                };
                fs::write(&local, contents).unwrap();
                let before = snapshot(root);
                let text = check(
                    run(exe, repo.path(), &["config", "show"]),
                    0,
                    &[
                        "finish-update: merge (shared:",
                        "finish-rewrite: allow (local:",
                        filename,
                    ],
                );
                assert!(text.contains(&repo.path().join(CONFIG_FILES[0]).display().to_string()));
                assert!(text.contains(&local.display().to_string()));
                assert_eq!(snapshot(root), before);
                fs::remove_file(local).unwrap();
            }
        },
    );
}
