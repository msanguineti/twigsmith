#[path = "support/snapshot.rs"]
mod preservation;
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
mod terminal;

use git_oneflow::{
    config::{self, ConfigError, Format},
    git, repository,
    settings::{
        self, CONFIG_FILES, FinishRewrite, FinishUpdate, SaveAction, SettingSource, Settings,
    },
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
    assert_eq!(loaded.publish_remote, defaults.publish_remote);
    assert_eq!(loaded.base_remote, defaults.publish_remote);
    assert_eq!(loaded.sources.base_remote, None);
    let contents = fs::read_to_string(directory.join(filename)).unwrap();
    assert!(!contents.contains("base-remote"));
    assert_eq!(loaded.base, defaults.base);
    assert_eq!(loaded.save_action, defaults.save_action);
    assert_eq!(loaded.finish_update, defaults.finish_update);
    assert_eq!(loaded.history_edit, defaults.history_edit);
    assert_eq!(loaded.finish_rewrite, defaults.finish_rewrite);
    assert!(loaded.finish_checks.is_empty());
    for (value, expected, origin) in [
        (
            loaded.finish_mode.as_str(),
            "ask",
            &loaded.sources.finish_mode,
        ),
        (
            loaded.finish_integration.as_str(),
            "ask",
            &loaded.sources.finish_integration,
        ),
        (
            loaded.finish_message.as_str(),
            "ask",
            &loaded.sources.finish_message,
        ),
        (
            loaded.finish_checkout.as_str(),
            "target",
            &loaded.sources.finish_checkout,
        ),
        (
            loaded.finish_cleanup.as_str(),
            "ask",
            &loaded.sources.finish_cleanup,
        ),
    ] {
        assert_eq!(value, expected);
        assert_eq!(*origin, SettingSource::Shared(directory.join(filename)));
    }
    let source = SettingSource::Shared(directory.join(filename));
    assert_eq!(loaded.sources.publish_remote, source);
    assert_eq!(loaded.sources.base, source);
    assert_eq!(loaded.sources.save_action, source);
    assert_eq!(loaded.sources.finish_update, source);
    assert_eq!(loaded.sources.history_edit, source);
    assert_eq!(loaded.sources.finish_rewrite, source);
    assert_eq!(loaded.sources.finish_checks, source);
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
                        "publish-remote: origin",
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
                    "publish-remote: origin (built-in default)",
                    "base: main (built-in default)",
                    "save-action: ask (built-in default)",
                    "finish-update: ask (built-in default)",
                    "finish-history-edit: none (built-in default)",
                    "finish-rewrite: ask (built-in default)",
                    "Shared file: none",
                    "Local file: none",
                    &repo.path().display().to_string(),
                    &repo.path().join(".git").display().to_string(),
                    "Precedence:",
                ],
            );
            assert_eq!(snapshot(repo.path()), before);
            fs::write(
                repo.path().join(CONFIG_FILES[2]),
                "base: trunk\npublish-remote: upstream\n",
            )
            .unwrap();
            let local = repo.path().join(".git/git-oneflow.config.toml");
            fs::write(&local, "publish-remote = 'private'\n").unwrap();
            let before = snapshot(repo.path());
            check(
                run(exe, &nested, &["config", "show"]),
                0,
                &[
                    "publish-remote: private (local:",
                    "base: trunk (shared:",
                    "save-action: ask (built-in default)",
                    "finish-update: ask (built-in default)",
                    "finish-history-edit: none (built-in default)",
                    "finish-rewrite: ask (built-in default)",
                    &local.display().to_string(),
                    CONFIG_FILES[2],
                ],
            );
            check(run(exe, &nested, &["config"]), 1, &["config --yes"]);
            assert_eq!(snapshot(repo.path()), before);
            fs::write(&local, "save-action = 'invalid'\n").unwrap();
            check(
                run(exe, &nested, &["config", "show"]),
                1,
                &["invalid settings", "git-oneflow.config.toml", "save-action"],
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
fn unsupported_settings_stop_workflows_before_fallback_publication() {
    cases(
        "unsupported_settings_stop_workflows_before_fallback_publication",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            commit(&repo);
            repo.success(&["switch", "-c", "work"]);
            repo.success(&["remote", "add", "origin", "ext::touch remote-was-contacted"]);
            fs::write(
                repo.path().join(".git").join(CONFIG_FILES[0]),
                "remote = 'private'\npublish-remote = 'origin'\n",
            )
            .unwrap();
            let before = snapshot(root);
            for args in [
                vec!["config", "show"],
                vec!["start", "new-work"],
                vec!["save", "--publish-only"],
                vec![
                    "finish",
                    "--mode",
                    "review",
                    "--update",
                    "none",
                    "--allow-rewrite",
                ],
            ] {
                check(
                    run(exe, repo.path(), &args),
                    1,
                    &["unknown key \"remote\"", CONFIG_FILES[0]],
                );
                assert_eq!(snapshot(root), before, "{args:?}");
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
                (
                    CONFIG_FILES[0],
                    "publish-remote='private'\nsave-action='stash'\n[[finish-checks]]\nname='Private tests'\ncommand=['private-check']\n",
                ),
                (
                    CONFIG_FILES[1],
                    "publish-remote: private\nsave-action: stash\nfinish-update: none\nfinish-rewrite: deny\n",
                ),
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
                    assert_eq!(load(repo.path()).publish_remote, "private");
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
                "publish-remote: private\n",
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
            assert_eq!(load(&nested).publish_remote, "private");
            assert_eq!(load(&nested).base, "main");
            check(
                run(exe, &nested, &["config", "show"]),
                0,
                &[
                    "publish-remote: private (local:",
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
                    "publish-remote: origin (built-in default)",
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
                    let private = repo.path().join(".git/git-oneflow.config.toml");
                    fs::write(&private, "publish-remote = 'private'\nsave-action = 'stash'\nfinish-update = 'none'\nfinish-rewrite = 'deny'\n[[finish-checks]]\nname='Private tests'\ncommand=['private-check','']\n").unwrap();
                    let before_git = snapshot(&repo.path().join(".git"));
                    let mut terminal = terminal::Terminal::start(
                        exe,
                        repo.path(),
                        &["config", "--format", format],
                    );
                    terminal.until("Publication remote");
                    assert!(terminal.output.contains("hint-only"));
                    terminal.send(if custom && format == "toml" {
                        b"future-remote\n\nfuture/base\n4\n3\n2\n1\n1\n1\n1\n1\n\n"
                    } else if custom {
                        b"future-remote\n\nfuture/base\nwip\nmerge\nallow\n\n\n\n\n\n\n"
                    } else {
                        b"\n\n\n\n\n\n\n\n\n\n\n\n"
                    });
                    terminal.until("Create this file?");
                    assert!(!repo.path().join(filename).exists());
                    assert!(
                        terminal
                            .output
                            .contains("Effective publish-remote: private")
                    );
                    assert!(terminal.output.contains("Effective save-action: stash"));
                    assert!(terminal.output.contains("Effective finish-update: none"));
                    assert!(terminal.output.contains("Effective finish-rewrite: deny"));
                    assert!(terminal.output.contains(
                        r#"Effective finish-checks: [{"name":"Private tests","command":["private-check",""]}] (local:"#
                    ));
                    let proposed = Settings {
                        publish_remote: if custom { "future-remote" } else { "origin" }.into(),
                        base_remote: if custom { "future-remote" } else { "origin" }.into(),
                        base: if custom { "future/base" } else { "main" }.into(),
                        save_action: if custom {
                            SaveAction::Wip
                        } else {
                            SaveAction::Ask
                        },
                        finish_update: if custom {
                            FinishUpdate::Merge
                        } else {
                            FinishUpdate::Ask
                        },
                        finish_rewrite: if custom {
                            FinishRewrite::Allow
                        } else {
                            FinishRewrite::Ask
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
                    assert!(!contents.contains("redact-credentials"));
                    assert!(!contents.contains("private-check"));
                    assert!(terminal.output.replace('\r', "").contains(&contents));
                    terminal.send(if format == "toml" { b"1\n" } else { b"yes\n" });
                    assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                    assert_eq!(
                        fs::read_to_string(repo.path().join(filename)).unwrap(),
                        contents
                    );
                    assert_eq!(snapshot(&repo.path().join(".git")), before_git);
                    let effective = load(repo.path());
                    assert_eq!(effective.publish_remote, "private");
                    assert_eq!(effective.base, proposed.base);
                    assert_eq!(effective.finish_update, FinishUpdate::None);
                    assert_eq!(effective.finish_rewrite, FinishRewrite::Deny);
                    assert!(terminal.output.contains("Standing authorization"));
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
                    assert_eq!(effective.publish_remote, proposed.publish_remote);
                    assert_eq!(effective.save_action, proposed.save_action);
                    assert_eq!(effective.finish_update, proposed.finish_update);
                    assert_eq!(effective.finish_rewrite, proposed.finish_rewrite);
                    repo.success(&["remote", "add", &proposed.publish_remote, "/offline"]);
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
            terminal.until("Publication remote");
            terminal.send(b"bad remote\n");
            terminal.until("Use a valid named Git remote");
            terminal.send(b"upstream\nbad remote\n");
            terminal.until("Use a valid named Git remote (no URL");
            terminal.send(b"\nHEAD\n");
            terminal.until("Use a valid literal Git branch");
            terminal.send(b"trunk\nunknown\n");
            terminal.until("save-action must be");
            terminal.send(b"staged\ninvalid\n");
            terminal.until("finish-update must be");
            terminal.send(b"none\ninvalid\n");
            terminal.until("finish-rewrite must be");
            terminal.send(b"deny\n\n\n\n\n\n\n");
            terminal.until("Create this file?");
            terminal.send(b"y\n");
            terminal.until("Enter yes to create");
            assert!(!repo.path().join(CONFIG_FILES[0]).exists());
            terminal.send(b"yes\n");
            assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
            let settings = load(repo.path());
            assert_eq!(settings.publish_remote, "upstream");
            assert_eq!(settings.base, "trunk");
            assert_eq!(settings.save_action, SaveAction::Staged);
            assert_eq!(settings.finish_update, FinishUpdate::None);
            assert_eq!(settings.finish_rewrite, FinishRewrite::Deny);
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
                ("", "Publication remote"),
                ("\n", "Base source remote"),
                ("\n\n", "Base branch"),
                ("\n\n\n", "Default save action"),
                ("\n\n\n\n", "Default finish update"),
                ("\n\n\n\n\n", "Default finish rewrite"),
                ("\n\n\n\n\n\n", "Default finish mode"),
                ("\n\n\n\n\n\n\n", "Default finish integration"),
                ("\n\n\n\n\n\n\n\n", "Default finish message"),
                ("\n\n\n\n\n\n\n\n\n", "Default finish checkout"),
                ("\n\n\n\n\n\n\n\n\n\n", "Default finish cleanup"),
                ("\n\n\n\n\n\n\n\n\n\n\n", "Finish check name"),
                ("\n\n\n\n\n\n\n\n\n\n\nCheck\n", "Command as JSON"),
                (
                    "\n\n\n\n\n\n\n\n\n\n\nCheck\n[\"missing\"]\n",
                    "Finish check name [Enter to finish]:",
                ),
                ("\n\n\n\n\n\n\n\n\n\n\n\n", "Create this file?"),
            ] {
                for cancel in [b"cancel\n".as_slice(), b"\x04", b"partial\x04\x04"] {
                    let repo = Repository::new(root);
                    let before = snapshot(repo.path());
                    let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
                    terminal.until("Publication remote");
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
            terminal.until("Publication remote");
            terminal.interrupt();
            assert!(!terminal.finish().success());
            assert_eq!(snapshot(repo.path()), before);
            for answer in [b"no\n".as_slice(), b"2\n", b"3\n", b"\n"] {
                let repo = Repository::new(root);
                let before = snapshot(repo.path());
                let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
                terminal.until("Publication remote");
                terminal.send(b"\n\n\n\n\n\n\n\n\n\n\n\n");
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
                        fs::write(
                            repo.path().join(".git/git-oneflow.config.toml"),
                            "base = 'main'\n",
                        )
                        .unwrap();
                    }
                    let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config"]);
                    terminal.until("Publication remote");
                    if confirmation {
                        terminal.send(b"\n\n\n\n\n\n\n\n\n\n\n\n");
                        terminal.until("Create this file?");
                    }
                    match mutation {
                        "shared" => {
                            fs::write(repo.path().join(CONFIG_FILES[2]), "base: trunk\n").unwrap()
                        }
                        "private" => fs::write(
                            repo.path().join(".git/git-oneflow.config.toml"),
                            "base = 'trunk'\n",
                        )
                        .unwrap(),
                        "private-change" => {
                            fs::write(
                                repo.path().join(".git/git-oneflow.config.toml"),
                                "base = 'trunk'\n",
                            )
                            .unwrap();
                        }
                        "private-invalid" => {
                            fs::write(
                                repo.path().join(".git/git-oneflow.config.toml"),
                                "unknown = value\n",
                            )
                            .unwrap();
                        }
                        "private-remove" => {
                            fs::remove_file(repo.path().join(".git/git-oneflow.config.toml"))
                                .unwrap();
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
                    terminal.send(if confirmation {
                        b"yes\n"
                    } else {
                        b"\n\n\n\n\n\n\n\n\n\n\n\n"
                    });
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
                assert!(!terminal.output.contains("Publication remote"));
                assert_eq!(snapshot(repo.path()), before);
            }
            let repo = Repository::new(root);
            let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config", "--yes"]);
            assert_eq!(terminal.finish().code(), Some(0));
            assert!(!terminal.output.contains("Publication remote"));
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
                "finish-update='merge'\nfinish-history-edit='interactive'\nfinish-rewrite='deny'\n",
            )
            .unwrap();
            for filename in [CONFIG_FILES[3], "git-oneflow.config.toml"] {
                let local = repo.path().join(".git").join(filename);
                let contents = if filename == "git-oneflow.config.toml" {
                    "finish-rewrite='allow'\n"
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
                        "finish-history-edit: interactive (shared:",
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

#[test]
fn config_show_identifies_remote_role_values_and_derived_sources() {
    cases(
        "config_show_identifies_remote_role_values_and_derived_sources",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            let shared = repo.path().join(CONFIG_FILES[0]);
            let local = repo.path().join(".git/git-oneflow.config.toml");
            check(
                run(exe, repo.path(), &["config", "show"]),
                0,
                &[
                    "publish-remote: origin (built-in default)",
                    "base-remote: origin (follows publish-remote; built-in default)",
                ],
            );
            fs::write(
                &shared,
                "publish-remote='personal'\nbase-remote='project'\n",
            )
            .unwrap();
            fs::write(&local, "publish-remote='local'\n").unwrap();
            let before = snapshot(root);
            check(
                run(exe, repo.path(), &["config", "show"]),
                0,
                &[
                    &format!("publish-remote: local (local: {})", local.display()),
                    &format!("base-remote: project (shared: {})", shared.display()),
                ],
            );
            assert_eq!(snapshot(root), before);
            fs::write(&shared, "publish-remote='personal'\n").unwrap();
            check(
                run(exe, repo.path(), &["config", "show"]),
                0,
                &[&format!(
                    "base-remote: local (follows publish-remote; local: {})",
                    local.display()
                )],
            );
            fs::write(
                &local,
                "publish-remote='selected'\nbase-remote='selected'\n",
            )
            .unwrap();
            check(
                run(exe, repo.path(), &["config", "show"]),
                0,
                &[
                    &format!("publish-remote: selected (local: {})", local.display()),
                    &format!("base-remote: selected (local: {})", local.display()),
                ],
            );
        },
    );
}

#[test]
fn differing_remote_roles_preserve_finish_readiness_guards() {
    cases(
        "differing_remote_roles_preserve_finish_readiness_guards",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            commit(&repo);
            repo.success(&["switch", "-c", "work"]);
            fs::write(repo.path().join("tracked"), "dirty\n").unwrap();
            fs::write(
                repo.path().join(".git/git-oneflow.config.toml"),
                "publish-remote='personal'\nbase-remote='project'\n",
            )
            .unwrap();
            repo.success(&["remote", "add", "personal", "/missing/personal"]);
            repo.success(&["remote", "add", "project", "/missing/project"]);
            let before = snapshot(root);
            for args in [
                vec![
                    "finish",
                    "--mode",
                    "review",
                    "--update",
                    "none",
                    "--allow-rewrite",
                ],
                vec!["f", "--mode", "review"],
            ] {
                check(
                    run(exe, repo.path(), &args),
                    1,
                    &["finish requires a clean index, working tree, and submodules"],
                );
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn generated_remote_roles_round_trip_in_each_format() {
    isolated(
        "generated_remote_roles_round_trip_in_each_format",
        true,
        |root| {
            let repo = Repository::new(root);
            for format in [Format::Toml, Format::Yaml, Format::Json] {
                for explicit_base in [None, Some("project"), Some("personal")] {
                    let mut proposed = Settings {
                        publish_remote: "personal".into(),
                        base_remote: explicit_base.unwrap_or("personal").into(),
                        ..Settings::default()
                    };
                    if explicit_base.is_some() {
                        proposed.sources.base_remote = Some(SettingSource::BuiltIn);
                    }
                    let rendered = config::render(format, &proposed);
                    fs::write(repo.path().join(format.filename()), rendered).unwrap();
                    let resolved = load(repo.path());
                    assert_eq!(resolved.publish_remote, "personal");
                    assert_eq!(resolved.base_remote, explicit_base.unwrap_or("personal"));
                    assert_eq!(
                        resolved.sources.base_remote.is_some(),
                        explicit_base.is_some()
                    );
                    fs::remove_file(repo.path().join(format.filename())).unwrap();
                }
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_remote_roles_review_selected_intent_with_private_precedence() {
    cases(
        "guided_remote_roles_review_selected_intent_with_private_precedence",
        true,
        |root, exe| {
            for (format, filename) in [
                ("toml", CONFIG_FILES[0]),
                ("yaml", CONFIG_FILES[1]),
                ("json", CONFIG_FILES[3]),
            ] {
                for base_answer in ["", "personal", "project"] {
                    for private_case in ["none", "publication", "roles", "base"] {
                        let repo = Repository::new(root);
                        // Names are hints, not assigned roles.
                        for name in ["origin", "upstream", "fork"] {
                            repo.success(&["remote", "add", name, "/offline"]);
                        }
                        let (private_name, private_text, publication, private_base) =
                            match private_case {
                                "publication" => (
                                    CONFIG_FILES[0],
                                    "publish-remote='private'\n",
                                    "private",
                                    None,
                                ),
                                "roles" => (
                                    CONFIG_FILES[0],
                                    "publish-remote='selected'\nbase-remote='local-base'\n",
                                    "selected",
                                    Some("local-base"),
                                ),
                                "base" => (
                                    CONFIG_FILES[0],
                                    "base-remote='local-base'\nredact-credentials=false\n",
                                    "personal",
                                    Some("local-base"),
                                ),
                                _ => ("", "", "personal", None),
                            };
                        if !private_name.is_empty() {
                            fs::write(repo.path().join(".git").join(private_name), private_text)
                                .unwrap();
                        }
                        let mut term = terminal::Terminal::start(
                            exe,
                            repo.path(),
                            &["config", "--format", format],
                        );
                        term.until("Publication remote");
                        term.send(
                            format!("personal\n{base_answer}\n{}", "\n".repeat(10)).as_bytes(),
                        );
                        term.until("Create this file?");
                        let split = base_answer == "project";
                        let base =
                            private_base.unwrap_or(if split { "project" } else { publication });
                        assert!(
                            term.output
                                .contains(&format!("Effective publish-remote: {publication}")),
                            "{}",
                            term.output
                        );
                        assert!(
                            term.output
                                .contains(&format!("Effective base-remote: {base}")),
                            "{}",
                            term.output
                        );
                        let follows = !split && private_base.is_none();
                        assert_eq!(
                            term.output.contains("follows publish-remote"),
                            follows,
                            "{}",
                            term.output
                        );
                        term.send(b"yes\n");
                        assert!(term.finish().success(), "{}", term.output);
                        let contents = fs::read_to_string(repo.path().join(filename)).unwrap();
                        assert_eq!(contents.contains("base-remote"), split);
                        assert!(!contents.contains("redact-credentials"));
                        assert!(!contents.contains("private-check"));
                        assert!(!contents.contains("private"));
                        assert!(!contents.contains("local-base"));
                        let effective = load(repo.path());
                        assert_eq!(effective.publish_remote, publication);
                        assert_eq!(effective.base_remote, base);
                        assert_eq!(effective.sources.base_remote.is_none(), follows);
                        let shown = check(
                            run(exe, repo.path(), &["config", "show"]),
                            0,
                            &[
                                &format!("publish-remote: {publication}"),
                                &format!("base-remote: {base}"),
                            ],
                        );
                        assert_eq!(shown.contains("(follows publish-remote;"), follows);
                    }
                }
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_checks_preserve_order_arguments_and_private_list_replacement() {
    cases(
        "guided_checks_preserve_order_arguments_and_private_list_replacement",
        true,
        |root, exe| {
            let checks = vec![
                settings::FinishCheck {
                    name: "Literal arguments ✓".into(),
                    command: vec![
                        "unavailable executable".into(),
                        "".into(),
                        " two words ".into(),
                        "\"quotes\" 'single' \\path".into(),
                        "$HOME; $(touch must-not-exist) && *".into(),
                        "line\nbreak\t\u{1b}".into(),
                    ],
                },
                settings::FinishCheck {
                    name: "Second".into(),
                    command: vec!["another-unavailable-check".into(), "雪".into()],
                },
            ];
            for (format, filename) in [
                ("toml", CONFIG_FILES[0]),
                ("yaml", CONFIG_FILES[1]),
                ("json", CONFIG_FILES[3]),
            ] {
                for private_case in ["inherit", "replace", "disable"] {
                    let repo = Repository::new(root);
                    let replacement = vec![settings::FinishCheck {
                        name: "Private only".into(),
                        command: vec!["private-executable".into(), "".into()],
                    }];
                    let private = match private_case {
                        "replace" => serde_json::json!({"finish-checks": replacement}),
                        "disable" => serde_json::json!({"finish-checks": []}),
                        _ => serde_json::json!({"save-action": "stash"}),
                    };
                    fs::write(
                        repo.path().join(".git").join(CONFIG_FILES[3]),
                        private.to_string(),
                    )
                    .unwrap();
                    let before = snapshot(repo.path());
                    let mut term = terminal::Terminal::start(
                        exe,
                        repo.path(),
                        &["config", "--format", format],
                    );
                    term.until("Publication remote");
                    let mut answers = "\n\n\n\n\n\n\n\n\n\n\n".to_string();
                    for check in &checks {
                        answers.push_str(&check.name);
                        answers.push('\n');
                        answers.push_str(&serde_json::to_string(&check.command).unwrap());
                        answers.push('\n');
                    }
                    answers.push('\n');
                    term.send(answers.as_bytes());
                    term.until("Create this file?");
                    assert_eq!(
                        snapshot(repo.path()),
                        before,
                        "setup must only read before approval"
                    );
                    for (index, check) in checks.iter().enumerate() {
                        assert!(
                            term.output.contains(&format!(
                                "{}. {}: {}",
                                index + 1,
                                serde_json::to_string(&check.name).unwrap(),
                                serde_json::to_string(&check.command).unwrap()
                            )),
                            "{}",
                            term.output
                        );
                    }
                    if private_case != "inherit" {
                        assert!(
                            term.output.contains(
                                "Effective private finish checks (replace the shared list)"
                            )
                        );
                    }
                    if private_case == "disable" {
                        assert!(term.output.contains(
                            "Effective private finish checks (replace the shared list): none"
                        ));
                    }
                    term.send(b"yes\n");
                    assert!(term.finish().success(), "{}", term.output);
                    let effective = load(repo.path());
                    let expected = match private_case {
                        "replace" => replacement,
                        "disable" => vec![],
                        _ => checks.clone(),
                    };
                    assert_eq!(effective.finish_checks, expected);
                    let contents = fs::read_to_string(repo.path().join(filename)).unwrap();
                    assert!(!contents.contains("private-executable"));
                    fs::remove_file(repo.path().join(".git").join(CONFIG_FILES[3])).unwrap();
                    assert_eq!(load(repo.path()).finish_checks, checks);
                }
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_invalid_check_entries_can_be_corrected() {
    cases(
        "guided_invalid_check_entries_can_be_corrected",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
            term.until("Publication remote");
            term.send(b"\n\n\n\n\n\n\n\n\n\n\n   \n");
            term.until("Use a nonblank display name");
            term.send(b"Tests\ncargo test\n[1]\nnull\n{\"command\": []}\n");
            term.until("Use a JSON array of strings");
            term.send(b"[]\n[\"\"]\n");
            term.until("command must contain a nonempty executable");
            term.send(b"[\"cargo\", \"\\u0000\"]\n");
            term.until("command strings must not contain NUL");
            term.send(b"[\"cargo\", \"test\", \"\"]\n\nyes\n");
            assert!(term.finish().success(), "{}", term.output);
            assert_eq!(
                load(repo.path()).finish_checks,
                vec![settings::FinishCheck {
                    name: "Tests".into(),
                    command: vec!["cargo".into(), "test".into(), "".into()],
                }]
            );
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn guided_completion_questions_generation_and_effective_private_review() {
    cases(
        "guided_completion_questions_generation_and_effective_private_review",
        true,
        |root, exe| {
            for (format, filename) in [
                ("toml", CONFIG_FILES[0]),
                ("yaml", CONFIG_FILES[1]),
                ("json", CONFIG_FILES[3]),
            ] {
                for (mode, method, message) in [
                    ("review", "ask", "ask"),
                    ("integrate", "ff-only", "ask"),
                    ("integrate", "merge", "auto"),
                    ("ask", "squash", "ask"),
                ] {
                    let repo = Repository::new(root);
                    fs::write(repo.path().join(".git/git-oneflow.config.toml"), "finish-mode='integrate'\nfinish-integration='squash'\nfinish-message='auto'\nfinish-checkout='work'\nfinish-cleanup='both'\n").unwrap();
                    let before = snapshot(&repo.path().join(".git"));
                    let mut term = terminal::Terminal::start(
                        exe,
                        repo.path(),
                        &["config", "--format", format],
                    );
                    term.until("Publication remote");
                    let mut answers = format!("{}{}\n", "\n".repeat(6), mode);
                    if mode != "review" {
                        answers.push_str(&format!("{method}\n"));
                        if method != "ff-only" {
                            answers.push_str(&format!("{message}\n"));
                        }
                        answers.push_str("target\nlocal\n");
                    }
                    answers.push('\n'); // no checks
                    term.send(answers.as_bytes());
                    term.until("Create this file?");
                    assert_eq!(
                        term.output.contains("Default finish integration"),
                        mode != "review",
                        "{}",
                        term.output
                    );
                    assert_eq!(
                        term.output.contains("Default finish message"),
                        mode != "review" && method != "ff-only",
                        "{}",
                        term.output
                    );
                    assert_eq!(
                        term.output.contains("Default finish checkout"),
                        mode != "review",
                        "{}",
                        term.output
                    );
                    assert_eq!(
                        term.output.contains("Default finish cleanup"),
                        mode != "review",
                        "{}",
                        term.output
                    );
                    for (key, value) in [
                        ("mode", "integrate"),
                        ("integration", "squash"),
                        ("message", "auto"),
                        ("checkout", "work"),
                        ("cleanup", "both"),
                    ] {
                        assert!(
                            term.output
                                .contains(&format!("Effective finish-{key}: {value} (local:")),
                            "{}",
                            term.output
                        );
                    }
                    term.send(b"yes\n");
                    assert!(term.finish().success(), "{}", term.output);
                    assert_eq!(snapshot(&repo.path().join(".git")), before);
                    let contents = fs::read_to_string(repo.path().join(filename)).unwrap();
                    for key in ["finish-integration", "finish-checkout", "finish-cleanup"] {
                        assert_eq!(contents.contains(key), mode != "review", "{contents}");
                    }
                    assert_eq!(
                        contents.contains("finish-message"),
                        mode != "review" && method != "ff-only"
                    );
                    fs::remove_file(repo.path().join(".git/git-oneflow.config.toml")).unwrap();
                    let shared = load(repo.path());
                    assert_eq!(shared.finish_mode.as_str(), mode);
                    assert_eq!(shared.finish_integration.as_str(), method);
                    assert_eq!(shared.finish_message.as_str(), message);
                    assert_eq!(shared.finish_checkout.as_str(), "target");
                    assert_eq!(
                        shared.finish_cleanup.as_str(),
                        if mode == "review" { "ask" } else { "local" }
                    );
                }
            }
            // Review with no private override accurately shows omitted policies as built-ins.
            let repo = Repository::new(root);
            let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
            term.until("Publication remote");
            term.send(b"\n\n\n\n\n\nreview\n\n");
            term.until("Create this file?");
            assert!(
                term.output
                    .contains("Effective finish-mode: review (shared:")
            );
            assert!(
                term.output
                    .contains("Effective finish-cleanup: ask (built-in default)")
            );
            assert!(term.output.contains("inactive for review delivery"));
            term.send(b"no\n");
            assert!(term.finish().success());
        },
    );
}

#[test]
fn unattended_oneflow_bundle_generation_round_trips_every_format() {
    use settings::{FinishMode, WorkstreamKind};
    cases(
        "unattended_oneflow_bundle_generation_round_trips_every_format",
        true,
        |root, exe| {
            for (format, filename) in [
                ("toml", CONFIG_FILES[0]),
                ("yaml", CONFIG_FILES[1]),
                ("json", CONFIG_FILES[3]),
            ] {
                let repo = Repository::new(root);
                let before_git = snapshot(&repo.path().join(".git"));
                check(
                    run(
                        exe,
                        repo.path(),
                        &[
                            "config",
                            "--yes",
                            "--oneflow",
                            "--marker",
                            "released",
                            "--format",
                            format,
                        ],
                    ),
                    0,
                    &["Created shared configuration"],
                );
                let configured = load(repo.path());
                assert_eq!(configured.workstreams.len(), 3);
                for kind in [
                    WorkstreamKind::Ordinary,
                    WorkstreamKind::Release,
                    WorkstreamKind::Hotfix,
                ] {
                    let mut effective = configured.clone();
                    settings::resolve_workstream(&mut effective, kind, None).unwrap();
                    assert_eq!(effective.finish_mode, FinishMode::Integrate);
                    let policy = effective.workstream.unwrap();
                    assert_eq!(policy.version_tagging, kind != WorkstreamKind::Ordinary);
                    assert_eq!(
                        policy.release_marker.as_deref(),
                        (kind != WorkstreamKind::Ordinary).then_some("released")
                    );
                }
                assert!(repo.path().join(filename).exists());
                assert_eq!(snapshot(&repo.path().join(".git")), before_git);
                check(
                    run(exe, repo.path(), &["config", "show"]),
                    0,
                    &[
                        "workstreams.release.version-tagging: required",
                        "workstreams.hotfix.active-release: none",
                        "workstreams.ordinary.finish-mode: integrate",
                    ],
                );
            }
        },
    );
}

#[test]
fn inspection_reports_ordered_tag_identity_and_declared_active_release_read_only() {
    use settings::WorkstreamKind;
    isolated(
        "inspection_reports_ordered_tag_identity_and_declared_active_release_read_only",
        true,
        |root| {
            let remote = Repository::new(root);
            commit(&remote);
            let repo = Repository::new(root);
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                remote.path().as_os_str(),
            ]);
            repo.success(&["fetch", "origin"]);
            repo.success(&["reset", "--hard", "refs/remotes/origin/main"]);
            let mut proposed = Settings::default();
            settings::oneflow_bundle(&mut proposed, None);
            let release = proposed
                .workstreams
                .get_mut(&WorkstreamKind::Release)
                .unwrap();
            release.integration_target = None;
            release.integration_targets = Some(vec!["main".into(), "stable".into()]);
            release.version_tag_identity = Some("integrated-result".into());
            let hotfix = proposed
                .workstreams
                .get_mut(&WorkstreamKind::Hotfix)
                .unwrap();
            hotfix.active_release = Some("release/train".into());
            hotfix.integration_target = None;
            hotfix.integration_targets = Some(vec!["release/train".into(), "main".into()]);
            fs::write(
                repo.path().join(".git/git-oneflow.config.toml"),
                config::render(Format::Toml, &proposed),
            )
            .unwrap();
            let exe = env!("CARGO_BIN_EXE_git-oneflow");
            let before = snapshot(root);
            check(
                run(exe, repo.path(), &["config", "show"]),
                0,
                &[
                    "Effective release version-tag-identity: integrated-result; integration-targets (in order): main -> stable.",
                    "Effective hotfix version-tag-identity: work-source; integration-targets (in order): release/train -> main.",
                    "workstreams.hotfix.active-release: release/train",
                    "workstreams.release.integration-targets: [\"main\",\"stable\"]",
                ],
            );
            assert_eq!(snapshot(root), before);
            check(
                run(
                    exe,
                    repo.path(),
                    &["start", "release/inspection", "--policy", "release"],
                ),
                0,
                &["Created"],
            );
            let before = snapshot(root);
            check(
                run(exe, repo.path(), &["config", "show"]),
                0,
                &[
                    "workstream-policy: release",
                    "integration-targets (in order): main -> stable",
                    "version-tag-identity: integrated-result",
                    "preparation-source: main",
                ],
            );
            assert_eq!(snapshot(root), before);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_oneflow_templates_are_editable_and_cancel_preserves_repository() {
    use settings::WorkstreamKind;
    cases(
        "guided_oneflow_templates_are_editable_and_cancel_preserves_repository",
        true,
        |root, exe| {
            for (format, filename) in [
                ("toml", CONFIG_FILES[0]),
                ("yaml", CONFIG_FILES[1]),
                ("json", CONFIG_FILES[3]),
            ] {
                for cancel in [false, true] {
                    let repo = Repository::new(root);
                    let before_git = snapshot(&repo.path().join(".git"));
                    let mut term = terminal::Terminal::start(
                        exe,
                        repo.path(),
                        &["config", "--oneflow", "--format", format],
                    );
                    term.until("Publication remote");
                    term.send(b"\n\n\n\n\n\n\n\n\n\n\n\n");
                    term.until("ordinary policy [");
                    assert!(!repo.path().join(filename).exists());
                    if cancel {
                        term.send(b"cancel\n");
                        assert_eq!(term.finish().code(), Some(0), "{}", term.output);
                        assert!(term.output.contains("Setup cancelled"));
                        assert!(!repo.path().join(filename).exists());
                    } else {
                        let mut proposed = Settings::default();
                        settings::oneflow_bundle(&mut proposed, None);
                        let ordinary = proposed
                            .workstreams
                            .get_mut(&WorkstreamKind::Ordinary)
                            .unwrap();
                        ordinary.preparation_source = Some("release/preview".into());
                        let json = serde_json::to_string(ordinary).unwrap();
                        term.send(format!("{json}\n\n\n").as_bytes());
                        term.until("Create this file?");
                        assert!(term.output.contains("release/preview"));
                        assert!(!repo.path().join(filename).exists());
                        term.send(b"yes\n");
                        assert_eq!(term.finish().code(), Some(0), "{}", term.output);
                        assert_eq!(
                            load(repo.path()).workstreams[&WorkstreamKind::Ordinary]
                                .preparation_source
                                .as_deref(),
                            Some("release/preview")
                        );
                    }
                    assert_eq!(snapshot(&repo.path().join(".git")), before_git);
                }
            }
        },
    );
}

#[test]
fn inspection_reports_established_explicit_creation_commit_separately_from_default_route() {
    cases(
        "inspection_reports_established_explicit_creation_commit_separately_from_default_route",
        true,
        |root, exe| {
            let remote = Repository::new(root);
            commit(&remote);
            let repo = Repository::new(root);
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                remote.path().as_os_str(),
            ]);
            repo.success(&["fetch", "origin"]);
            repo.success(&["reset", "--hard", "refs/remotes/origin/main"]);
            let earlier = String::from_utf8(repo.success(&["rev-parse", "HEAD"]).stdout)
                .unwrap()
                .trim()
                .to_owned();
            fs::write(repo.path().join("newer"), "development\n").unwrap();
            repo.success(&["add", "newer"]);
            repo.success(&["commit", "-m", "newer development"]);
            let mut proposed = Settings::default();
            settings::oneflow_bundle(&mut proposed, None);
            fs::write(
                repo.path().join(".git/git-oneflow.config.toml"),
                config::render(Format::Toml, &proposed),
            )
            .unwrap();
            check(
                run(
                    exe,
                    repo.path(),
                    &[
                        "start",
                        "release/train",
                        "--policy",
                        "release",
                        "--from",
                        &earlier,
                    ],
                ),
                0,
                &["Created"],
            );
            let before = snapshot(root);
            check(
                run(exe, repo.path(), &["config", "show"]),
                0,
                &[
                    "workstream-policy: release",
                    "creation-source: main",
                    &format!("creation-commit: {earlier}"),
                    &format!("creation-input: {earlier}"),
                ],
            );
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn unattended_hosted_setup_is_editable_and_reports_effective_routes_in_every_format() {
    use settings::{FinishMode, WorkstreamKind};
    cases(
        "unattended_hosted_setup_is_editable_and_reports_effective_routes_in_every_format",
        true,
        |root, exe| {
            for (format, filename) in [
                ("toml", CONFIG_FILES[0]),
                ("yaml", CONFIG_FILES[1]),
                ("json", CONFIG_FILES[3]),
            ] {
                let repo = Repository::new(root);
                let before = snapshot(&repo.path().join(".git"));
                check(
                    run(
                        exe,
                        repo.path(),
                        &[
                            "config",
                            "--oneflow-hosted",
                            "--yes",
                            "--marker",
                            "stable",
                            "--format",
                            format,
                        ],
                    ),
                    0,
                    &["Created shared configuration"],
                );
                assert_eq!(snapshot(&repo.path().join(".git")), before);
                let mut effective = load(repo.path());
                settings::resolve_workstream(&mut effective, WorkstreamKind::Release, None)
                    .unwrap();
                assert_eq!(effective.finish_mode, FinishMode::Review);
                assert_eq!(effective.workstream.unwrap().review_remote, "origin");
                let private = repo.path().join(".git/git-oneflow.config.json");
                fs::write(&private, r#"{"workstreams":{"release":{"review-remote":"fork","merge-back-branch":"review/version"}}}"#).unwrap();
                check(
                    run(exe, repo.path(), &["config", "show"]),
                    0,
                    &[
                        "Effective release routes: work origin/original work branch; review source review/version on fork (local:",
                        "target origin/main",
                        "workstreams.release.review-remote: fork (local:",
                        "workstreams.release.merge-back-branch: review/version (local:",
                        "workstreams.release.finish-checkout: work",
                        "workstreams.release.finish-cleanup: keep",
                    ],
                );
                assert!(repo.path().join(filename).exists());
                let generated = fs::read_to_string(repo.path().join(filename)).unwrap();
                assert!(!generated.contains("review/version"));
            }
            let repo = Repository::new(root);
            for args in [
                vec!["config", "--yes", "--oneflow", "--oneflow-hosted"],
                vec!["config", "--yes", "--oneflow-hosted", "--oneflow-hosted"],
                vec!["config", "--yes", "--marker", "stable"],
            ] {
                check(run(exe, repo.path(), &args), 2, &["expected config"]);
                assert!(!repo.path().join(CONFIG_FILES[0]).exists());
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_hosted_setup_edits_routes_and_cancellation_preserves_work() {
    use settings::WorkstreamKind;
    cases(
        "guided_hosted_setup_edits_routes_and_cancellation_preserves_work",
        true,
        |root, exe| {
            for cancel in [false, true] {
                let repo = Repository::new(root);
                let before = snapshot(&repo.path().join(".git"));
                let mut term =
                    terminal::Terminal::start(exe, repo.path(), &["config", "--oneflow-hosted"]);
                term.until("Publication remote");
                term.send(b"\n\n\n\n\n\n\n\n\n\n\n\n");
                term.until("ordinary policy [");
                assert!(
                    term.output
                        .contains("Hosted tagged finish publishes the version")
                );
                if cancel {
                    term.send(b"cancel\n");
                    assert!(term.finish().success(), "{}", term.output);
                    assert!(!repo.path().join(CONFIG_FILES[0]).exists());
                } else {
                    term.send(b"\n");
                    term.until("release policy [");
                    let mut proposed = Settings::default();
                    settings::oneflow_hosted_bundle(&mut proposed, None);
                    let release = proposed
                        .workstreams
                        .get_mut(&WorkstreamKind::Release)
                        .unwrap();
                    release.review_remote = Some("fork".into());
                    release.merge_back_branch = Some("review/version".into());
                    term.send(
                        format!("{}\n\n", serde_json::to_string(release).unwrap()).as_bytes(),
                    );
                    term.until("Create this file?");
                    assert!(
                        term.output.contains("review source review/version on fork"),
                        "{}",
                        term.output
                    );
                    term.send(b"yes\n");
                    assert!(term.finish().success(), "{}", term.output);
                    assert_eq!(
                        load(repo.path()).workstreams[&WorkstreamKind::Release]
                            .review_remote
                            .as_deref(),
                        Some("fork")
                    );
                }
                assert_eq!(snapshot(&repo.path().join(".git")), before);
            }
        },
    );
}

#[test]
fn hosted_inspection_keeps_template_routes_independent_of_selected_branch_policy() {
    cases(
        "hosted_inspection_keeps_template_routes_independent_of_selected_branch_policy",
        true,
        |root, exe| {
            let remote = Repository::new(root);
            commit(&remote);
            let repo = Repository::new(root);
            for name in ["project", "fork"] {
                repo.success(&[
                    "remote".as_ref(),
                    "add".as_ref(),
                    name.as_ref(),
                    remote.path().as_os_str(),
                ]);
            }
            repo.success(&["fetch", "project"]);
            repo.success(&["reset", "--hard", "refs/remotes/project/main"]);
            let mut proposed = Settings {
                publish_remote: "fork".into(),
                base_remote: "project".into(),
                ..Settings::default()
            };
            settings::oneflow_hosted_bundle(&mut proposed, None);
            let private = repo.path().join(".git/git-oneflow.config.toml");
            fs::write(&private, config::render(Format::Toml, &proposed)).unwrap();
            check(
                run(
                    exe,
                    repo.path(),
                    &["start", "release/train", "--policy", "release"],
                ),
                0,
                &["Created"],
            );
            check(
                run(exe, repo.path(), &["config", "show"]),
                0,
                &[
                    "workstream-policy: release",
                    "publish-remote: project (local:",
                    "review-remote: fork",
                    "Effective ordinary routes: work fork/work branch; review source work branch on fork",
                    "Effective release routes: work project/original work branch; review source explicit --merge-back branch on fork (local:",
                ],
            );
            // A route change cannot silently authorize publication under a branch's older association.
            proposed
                .workstreams
                .get_mut(&settings::WorkstreamKind::Release)
                .unwrap()
                .review_remote = Some("project".into());
            fs::write(&private, config::render(Format::Toml, &proposed)).unwrap();
            check(
                run(exe, repo.path(), &["config", "show"]),
                1,
                &["established workstream routes changed"],
            );
        },
    );
}
