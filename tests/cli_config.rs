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
        self, CONFIG_FILES, FinishRewrite, FinishUpdate, SavePlan, SettingSource, Settings,
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
    assert_eq!(loaded.save_plan, defaults.save_plan);
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
    assert_eq!(loaded.sources.save_plan, source);
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
                        "save-plan: ask",
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
                    "save-plan: ask (built-in default)",
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
                    "save-plan: ask (built-in default)",
                    "finish-update: ask (built-in default)",
                    "finish-history-edit: none (built-in default)",
                    "finish-rewrite: ask (built-in default)",
                    &local.display().to_string(),
                    CONFIG_FILES[2],
                ],
            );
            check(run(exe, &nested, &["config"]), 1, &["config --yes"]);
            assert_eq!(snapshot(repo.path()), before);
            fs::write(&local, "save-plan = 'invalid'\n").unwrap();
            check(
                run(exe, &nested, &["config", "show"]),
                1,
                &["invalid settings", "git-oneflow.config.toml", "save-plan"],
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
                    "publish-remote='private'\nsave-plan=['stash:all']\n[[finish-checks]]\nname='Private tests'\ncommand=['private-check']\n",
                ),
                (
                    CONFIG_FILES[1],
                    "publish-remote: private\nsave-plan: [stash:all]\nfinish-update: none\nfinish-rewrite: deny\n",
                ),
                (CONFIG_FILES[3], "invalid JSON"),
            ] {
                let repo = Repository::new(root);
                let local = repo.path().join(".git").join(filename);
                fs::write(&local, content).unwrap();
                let before_git = snapshot(&repo.path().join(".git"));
                if filename == CONFIG_FILES[3] {
                    check(
                        run(exe, repo.path(), &["config", "--yes"]),
                        1,
                        &["invalid settings"],
                    );
                    assert!(!repo.path().join(CONFIG_FILES[0]).exists());
                    assert_eq!(snapshot(&repo.path().join(".git")), before_git);
                    continue;
                }
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
                    assert_eq!(
                        load(repo.path()).save_plan,
                        SavePlan::parse(&["stash:all".into()]).unwrap()
                    );
                    assert_eq!(load(repo.path()).base, "main");
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
                    terminal.until("Remote receiving work");
                    if confirmation {
                        terminal.send(b"\n\n\n\n\n\n");
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
                        b"\n\n\n\n\n\n"
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
                assert!(!terminal.output.contains("Remote receiving work"));
                assert_eq!(snapshot(repo.path()), before);
            }
            let repo = Repository::new(root);
            let mut terminal = terminal::Terminal::start(exe, repo.path(), &["config", "--yes"]);
            assert_eq!(terminal.finish().code(), Some(0));
            assert!(!terminal.output.contains("Remote receiving work"));
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

#[test]
fn unattended_generation_audits_combined_private_policy_without_writing_invalid_settings() {
    cases(
        "unattended_generation_audits_combined_private_policy_without_writing_invalid_settings",
        true,
        |root, exe| {
            let repo = Repository::new(root);
            // A merge update is valid for the private layer's untagged defaults, but
            // cannot combine with the generated tagged direct-integration policy.
            fs::write(
                repo.path().join(".git/git-oneflow.config.toml"),
                "[workstreams.release]\nfinish-update = 'merge'\n",
            )
            .unwrap();
            let before = snapshot(repo.path());
            check(
                run(exe, repo.path(), &["config", "--oneflow", "--yes"]),
                1,
                &["tagged integration requires finish-update = none"],
            );
            assert_eq!(snapshot(repo.path()), before);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn guided_keyboard_cancellation_creates_no_configuration() {
    cases(
        "guided_keyboard_cancellation_creates_no_configuration",
        true,
        |root, exe| {
            for cancel in [b"\x1b".as_slice(), b"\x04", b"\x03"] {
                let repo = Repository::new(root);
                let before = snapshot(repo.path());
                let mut term = terminal::Terminal::start_capable(exe, repo.path(), &["config"]);
                term.until("Remote receiving work");
                term.send(cancel);
                assert!(term.finish().success(), "{}", term.output);
                assert!(term.output.contains("Setup cancelled"), "{}", term.output);
                term.assert_restored();
                assert_eq!(snapshot(repo.path()), before);
            }
        },
    );
}

#[cfg(target_os = "linux")]
fn default_journey(term: &mut terminal::Terminal) {
    term.until("Remote receiving work");
    term.send(b"\n\n\n\n\n\n");
    term.until("Create this file?");
}

#[cfg(target_os = "linux")]
#[test]
fn guided_group_defaults_formats_exact_contents_and_private_isolation() {
    cases("guided_group_defaults_formats", true, |root, exe| {
        for (format, filename) in [
            ("toml", CONFIG_FILES[0]),
            ("yaml", CONFIG_FILES[1]),
            ("json", CONFIG_FILES[3]),
        ] {
            let repo = Repository::new(root);
            let private = repo.path().join(".git/git-oneflow.config.toml");
            fs::write(&private, "publish-remote='private'\nsave-plan=['stash:all']\nfinish-update='none'\nfinish-rewrite='deny'\n[[finish-checks]]\nname='Private tests'\ncommand=['private-check','']\n").unwrap();
            let before = snapshot(&repo.path().join(".git"));
            let mut term =
                terminal::Terminal::start(exe, repo.path(), &["config", "--format", format]);
            default_journey(&mut term);
            assert!(!repo.path().join(filename).exists());
            assert!(
                term.output
                    .contains("Work is published to private. New work starts from private/main.")
            );
            assert!(term.output.contains("Private overrides in"));
            assert!(
                term.output
                    .contains("finish checks replace the shared list")
            );
            assert!(!term.output.contains("Exact shared contents"));
            assert!(!term.output.contains("private-check"));
            term.send(b"contents\n");
            term.until("Exact shared contents");
            term.until("finish-checks");
            assert!(!repo.path().join(filename).exists());
            term.send(b"yes\n");
            assert!(term.finish().success(), "{}", term.output);
            let contents = fs::read_to_string(repo.path().join(filename)).unwrap();
            assert!(!contents.contains("private"));
            assert!(!contents.contains("redact-credentials"));
            assert!(term.output.replace('\r', "").contains(&contents));
            assert_eq!(term.output.matches("Private overrides in").count(), 1);
            assert_eq!(snapshot(&repo.path().join(".git")), before);
            let effective = load(repo.path());
            assert_eq!(effective.publish_remote, "private");
            assert_eq!(
                effective.save_plan,
                SavePlan::parse(&["stash:all".into()]).unwrap()
            );
            assert_eq!(effective.finish_update, FinishUpdate::None);
            assert_eq!(effective.finish_rewrite, FinishRewrite::Deny);
            fs::remove_file(private).unwrap();
            assert_defaults(repo.path(), filename);
        }
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_custom_direct_preferences_and_earlier_correction() {
    cases("guided_custom_direct_preferences", true, |root, exe| {
        let repo = Repository::new(root);
        let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
        term.until("Remote receiving work");
        term.send("future-remote\nupstream\nfeature/β\nedit\ncommit:staged,commit:unstaged\n\n\nedit\nintegrate\nrebase\nallow\nsquash\nauto\nwork\nboth\nedit\ninteractive\nkeep\n".as_bytes());
        term.until("Create this file?");
        term.send(b"repository\n\nfuture-project\nfuture/base\nyes\n");
        assert!(term.finish().success(), "{}", term.output);
        let value = load(repo.path());
        assert_eq!(value.publish_remote, "future-remote");
        assert_eq!(value.base_remote, "future-project");
        assert_eq!(value.base, "future/base");
        assert_eq!(
            value.save_plan,
            SavePlan::parse(&["commit:staged".into(), "commit:unstaged".into()]).unwrap()
        );
        assert_eq!(value.finish_mode, settings::FinishMode::Integrate);
        assert_eq!(value.finish_update, FinishUpdate::Rebase);
        assert_eq!(value.finish_rewrite, FinishRewrite::Allow);
        assert_eq!(value.history_edit, settings::HistoryEdit::Interactive);
        assert_eq!(
            value.finish_integration,
            settings::FinishIntegration::Squash
        );
        assert_eq!(value.finish_message, settings::FinishMessage::Auto);
        assert_eq!(value.finish_checkout, settings::FinishCheckout::Work);
        assert_eq!(value.finish_cleanup, settings::FinishCleanup::Both);
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_review_skips_inactive_delivery_choices_and_ff_skips_message() {
    cases("guided_review_skips_inactive", true, |root, exe| {
        for review in [true, false] {
            let repo = Repository::new(root);
            let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
            term.until("Remote receiving work");
            term.send(if review {
                b"\n\n\nkeep\nedit\nreview\nmerge\ndeny\nkeep\nkeep\nyes\n"
            } else {
                b"\n\n\nkeep\nedit\nintegrate\nnone\ndeny\nff-only\ntarget\nkeep\nkeep\nkeep\nyes\n"
            });
            assert!(term.finish().success(), "{}", term.output);
            assert_eq!(
                term.output.contains("How should work enter the target?"),
                !review
            );
            assert!(!term.output.contains("Integration commit message:"));
            assert_eq!(
                term.output.contains("Work branches after direct delivery:"),
                !review
            );
            let contents = fs::read_to_string(repo.path().join(CONFIG_FILES[0])).unwrap();
            assert!(!contents.contains("finish-message"));
            if review {
                assert!(!contents.contains("finish-integration"));
            }
        }
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_invalid_input_correctable_and_creation_explicit() {
    cases("guided_invalid_input_correctable", true, |root, exe| {
        let repo = Repository::new(root);
        let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
        term.until("Remote receiving work");
        term.send(b"bad remote\n");
        term.until("Use a named Git remote");
        term.send(b"follow\nbad remote\n\nHEAD\ntrunk\ninvalid\nkeep\nkeep\nkeep\ny\n");
        term.until("Create this file?");
        term.until("Enter a listed number");
        assert!(!repo.path().join(CONFIG_FILES[0]).exists());
        term.send(b"yes\n");
        assert!(term.finish().success(), "{}", term.output);
        assert_eq!(load(repo.path()).publish_remote, "follow");
        assert_eq!(load(repo.path()).base_remote, "follow");
        assert_eq!(load(repo.path()).base, "trunk");
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_cancellation_at_group_and_literal_entry_preserves_repository() {
    cases("guided_cancellation_at_group", true, |root, exe| {
        for (answers, prompt) in [
            ("", "Remote receiving work"),
            ("\n", "Project source remote"),
            ("\n\n", "Project base branch"),
            ("\n\n\n", "Saving preferences:"),
            ("\n\n\nedit\n", "When saving uncommitted work:"),
            ("\n\n\nkeep\n", "Finishing preferences:"),
            (
                "\n\n\nkeep\nedit\n",
                "How should finished work be delivered?",
            ),
            ("\n\n\nkeep\nkeep\n", "Finish checks:"),
            ("\n\n\nkeep\nkeep\nadd\n", "Check display name:"),
            ("\n\n\nkeep\nkeep\nadd\nTests\n", "Executable name or path:"),
            (
                "\n\n\nkeep\nkeep\nadd\nTests\ncargo\nadd\n",
                "Literal argument",
            ),
            ("\n\n\nkeep\nkeep\nkeep\n", "Create this file?"),
        ] {
            let repo = Repository::new(root);
            let before = snapshot(repo.path());
            let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
            term.until("Remote receiving work");
            term.send(answers.as_bytes());
            term.until(prompt);
            term.send(b"\x04");
            assert!(term.finish().success(), "{}", term.output);
            assert!(term.output.contains("Setup cancelled"));
            assert_eq!(snapshot(repo.path()), before);
        }
        let repo = Repository::new(root);
        let before = snapshot(repo.path());
        let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
        term.until("Remote receiving work");
        term.interrupt();
        assert!(!term.finish().success());
        assert_eq!(snapshot(repo.path()), before);
        for answer in [b"\n".as_slice(), b"no\n", b"cancel\n"] {
            let repo = Repository::new(root);
            let before = snapshot(repo.path());
            let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
            default_journey(&mut term);
            term.send(answer);
            assert!(term.finish().success(), "{}", term.output);
            assert_eq!(snapshot(repo.path()), before);
        }
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_literal_checks_order_empty_arguments_and_private_replacement() {
    cases("guided_literal_checks_order", true, |root, exe| {
        for replacement in [false, true] {
            let repo = Repository::new(root);
            if replacement {
                fs::write(
                    repo.path().join(".git/git-oneflow.config.toml"),
                    "finish-checks=[]\n",
                )
                .unwrap();
            }
            let before = snapshot(&repo.path().join(".git"));
            let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
            term.until("Remote receiving work");
            term.send(b"\n\n\nkeep\nkeep\nadd\n   \nTests\n\n/offline/executable\nadd\n\nadd\ncancel\nadd\n$(touch NEVER)\ndone\nadd\nSecond\ncancel\nadd\nspace argument\ndone\nkeep\nchecks\nedit\n1\nexecutable\n/offline/edited\ndone\nkeep\nyes\n");
            assert!(term.finish().success(), "{}", term.output);
            assert!(term.output.contains("Use a nonblank display name"));
            assert!(term.output.contains("Use a nonempty executable"));
            assert_eq!(snapshot(&repo.path().join(".git")), before);
            let expected = vec![
                settings::FinishCheck {
                    name: "Tests".into(),
                    command: vec![
                        "/offline/edited".into(),
                        "".into(),
                        "cancel".into(),
                        "$(touch NEVER)".into(),
                    ],
                },
                settings::FinishCheck {
                    name: "Second".into(),
                    command: vec!["cancel".into(), "space argument".into()],
                },
            ];
            let effective = load(repo.path());
            if replacement {
                assert!(effective.finish_checks.is_empty());
                assert!(
                    term.output
                        .contains("finish checks replace the shared list")
                );
                fs::remove_file(repo.path().join(".git/git-oneflow.config.toml")).unwrap();
            }
            assert_eq!(load(repo.path()).finish_checks, expected);
            assert!(!repo.path().join("NEVER").exists());
        }
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_bundle_structured_edit_and_relationship_correction() {
    cases("guided_bundle_structured_edit", true, |root, exe| {
        for hosted in [false, true] {
            let repo = Repository::new(root);
            let mut term = terminal::Terminal::start(
                exe,
                repo.path(),
                &[
                    "config",
                    if hosted {
                        "--oneflow-hosted"
                    } else {
                        "--oneflow"
                    },
                ],
            );
            term.until("Remote receiving work");
            term.send(b"\n\n\nkeep\nkeep\nkeep\nedit\ncreation-source\nset\nfuture/base\ndone\nedit\nrelease-marker\nset\nmain\ndone\n");
            term.until("preferences need correction:");
            assert!(!repo.path().join(CONFIG_FILES[0]).exists());
            term.send(if hosted { b"release-marker\nset\nstable\nreview-remote\nset\nfuture-fork\nmerge-back-branch\nset\nreview/version\ndone\nkeep\nyes\n" } else { b"release-marker\nset\nstable\nintegration-targets\nset\nadd\nmain\nadd\nstable/base\ndone\ndone\nkeep\nyes\n" });
            assert!(term.finish().success(), "{}", term.output);
            let value = load(repo.path());
            assert_eq!(
                value.workstreams[&settings::WorkstreamKind::Ordinary]
                    .creation_source
                    .as_deref(),
                Some("future/base")
            );
            let release = &value.workstreams[&settings::WorkstreamKind::Release];
            assert_eq!(release.release_marker.as_deref(), Some("stable"));
            if hosted {
                assert_eq!(release.review_remote.as_deref(), Some("future-fork"));
                assert_eq!(release.merge_back_branch.as_deref(), Some("review/version"));
            } else {
                assert!(release.integration_target.is_none());
                assert_eq!(
                    release.integration_targets,
                    Some(vec!["main".into(), "stable/base".into()])
                );
            }
        }
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_private_only_policy_audit_and_changed_override_review() {
    cases("guided_private_only_policy_audit", true, |root, exe| {
        let repo = Repository::new(root);
        fs::write(repo.path().join(".git/git-oneflow.config.toml"), "save-plan=['stash:all']\n[workstreams.release]\nversion-tagging='required'\nfinish-mode='integrate'\nfinish-update='none'\nrelease-marker='stable'\n").unwrap();
        let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
        term.until("Remote receiving work");
        term.send(b"\n\nstable\nkeep\nkeep\nkeep\n");
        term.until("preferences need correction:");
        term.send(b"integration-target\nset\nfuture/base\ndone\n");
        term.until("Create this file?");
        assert!(!repo.path().join(CONFIG_FILES[0]).exists());
        term.send(b"save\nedit\nstash\nsave\nedit\nwip\nyes\n");
        assert!(term.finish().success(), "{}", term.output);
        assert_eq!(term.output.matches("saving: Keep a local stash").count(), 2);
        let generated = fs::read_to_string(repo.path().join(CONFIG_FILES[0])).unwrap();
        assert!(generated.contains("future/base"));
        assert!(!generated.contains("version-tagging"));
        assert_eq!(
            load(repo.path()).save_plan,
            SavePlan::parse(&["stash:all".into()]).unwrap()
        );
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_keyboard_suggestions_and_unicode_literal_values() {
    cases("guided_keyboard_suggestions", true, |root, exe| {
        let repo = Repository::new(root);
        commit(&repo);
        repo.success(&["remote", "add", "réseau", "/offline"]);
        repo.success(&["branch", "feature/β;$"]);
        let before = snapshot(&repo.path().join(".git"));
        let mut term = terminal::Terminal::start_capable_narrow(exe, repo.path(), &["config"]);
        term.until("Remote receiving work");
        term.send("ré\t\r".as_bytes());
        term.until("Project source remote");
        term.send(b"\r");
        term.until("Project base branch");
        term.send("feature/\t\r".as_bytes());
        term.until("Saving preferences:");
        term.send(b"\r");
        term.until("Finishing preferences:");
        term.send(b"\r");
        term.until("Finish checks:");
        term.send(b"\r");
        term.until("Create this file?");
        term.send(b"\x1b[A\r");
        assert!(term.finish().success(), "{}", term.output);
        term.assert_restored();
        assert_eq!(load(repo.path()).publish_remote, "réseau");
        assert_eq!(load(repo.path()).base, "feature/β;$");
        assert_eq!(snapshot(&repo.path().join(".git")), before);
        assert!(!term.output.contains("Local branch references"));
    });
}

#[test]
fn local_branch_hints_invert_fetch_mappings_and_omit_ambiguity() {
    isolated("local_branch_hints", true, |root| {
        let repo = Repository::new(root);
        commit(&repo);
        repo.success(&["branch", "local/β;$"]);
        repo.success(&["remote", "add", "project", "/offline"]);
        repo.success(&["config", "--unset-all", "remote.project.fetch"]);
        for mapping in [
            "+refs/heads/topic/*:refs/cache/project/*",
            "refs/heads/fixed:refs/cache/exact",
            "^refs/heads/topic/hidden",
            "refs/heads/first/*:refs/cache/ambiguous/*",
            "refs/heads/second/*:refs/cache/ambiguous/*",
        ] {
            repo.success(&["config", "--add", "remote.project.fetch", mapping]);
        }
        for reference in [
            "refs/cache/project/β;$",
            "refs/cache/project/hidden",
            "refs/cache/exact",
            "refs/cache/ambiguous/value",
            "refs/remotes/other/nope",
        ] {
            repo.success(&["update-ref", reference, "HEAD"]);
        }
        repo.success(&[
            "symbolic-ref",
            "refs/cache/project/HEAD",
            "refs/cache/project/β;$",
        ]);
        let before = snapshot(repo.path());
        let plan = config::prepare_guided(repo.path(), Format::Toml).unwrap();
        assert_eq!(plan.review().remote_suggestions(), ["project"]);
        assert_eq!(
            plan.review().branch_suggestions("project"),
            ["fixed", "local/β;$", "main", "topic/β;$"]
        );
        assert_eq!(
            plan.review().branch_suggestions("future"),
            ["local/β;$", "main"]
        );
        assert_eq!(snapshot(repo.path()), before);
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_remote_roles_keep_literal_names_and_sparse_follow_relationship() {
    cases("guided_remote_roles_sparse_follow", true, |root, exe| {
        for (project, expected, follows) in [
            ("work", "private", true),
            ("follow", "follow", false),
            ("cancel", "cancel", false),
        ] {
            let repo = Repository::new(root);
            fs::write(
                repo.path().join(".git/git-oneflow.config.toml"),
                "publish-remote='private'\n",
            )
            .unwrap();
            let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
            term.until("Remote receiving work");
            term.send(format!("work\n{project}\ncancel\nkeep\nkeep\nkeep\nyes\n").as_bytes());
            assert!(term.finish().success(), "{}", term.output);
            assert_eq!(load(repo.path()).base_remote, expected);
            assert_eq!(load(repo.path()).base, "cancel");
            let contents = fs::read_to_string(repo.path().join(CONFIG_FILES[0])).unwrap();
            assert_eq!(contents.contains("base-remote"), !follows);
            fs::remove_file(repo.path().join(".git/git-oneflow.config.toml")).unwrap();
            assert_eq!(
                load(repo.path()).base_remote,
                if follows { "work" } else { project }
            );
        }
    });
}

#[test]
fn generation_audits_composable_plans_and_labels_in_all_formats() {
    isolated("generation_audits_composable_plans", true, |root| {
        let repo = Repository::new(root);
        for format in [Format::Toml, Format::Yaml, Format::Json] {
            let mut plan = config::prepare(repo.path(), format).unwrap();
            plan.proposed.save_plan =
                SavePlan::parse(&["stash:staged".into(), "commit:unstaged".into()]).unwrap();
            plan.proposed.save_message = "Literal $message `text`".into();
            plan.proposed.save_stash_message = "Local work".into();
            config::generate(repo.path(), &plan).unwrap();
            let loaded = load(repo.path());
            assert_eq!(loaded.save_plan, plan.proposed.save_plan);
            assert_eq!(loaded.save_message, plan.proposed.save_message);
            assert_eq!(loaded.save_stash_message, plan.proposed.save_stash_message);
            fs::remove_file(repo.path().join(format.filename())).unwrap();
            let step = settings::SaveOperation {
                action: settings::SaveKind::Commit,
                scope: settings::SaveScope::Staged,
            };
            plan.proposed.save_plan = SavePlan::Operations(vec![step, step]);
            assert!(config::generate(repo.path(), &plan).is_err());
            assert!(!plan.destination().exists());
            plan.proposed.save_plan = SavePlan::Ask;
            plan.proposed.save_message = " \t".into();
            assert!(config::generate(repo.path(), &plan).is_err());
            assert!(!plan.destination().exists());
        }
    });
}

#[cfg(target_os = "linux")]
#[test]
fn guided_save_plan_and_literal_labels_remain_correctable() {
    cases("guided_save_plan_and_literal_labels", true, |root, exe| {
        let repo = Repository::new(root);
        let mut term = terminal::Terminal::start(exe, repo.path(), &["config"]);
        term.until("Remote receiving work");
        term.send(b"\n\n\nedit\ncustom\ncommit:all,stash:staged\ncustom\nstash:staged,commit:unstaged\n \nLiteral $message `text`\nLocal $label\nkeep\nkeep\n");
        term.until("Create this file?");
        assert!(
            term.output.contains("repeats or overlaps"),
            "{}",
            term.output
        );
        assert!(term.output.contains("must be nonblank"), "{}", term.output);
        assert!(!repo.path().join(CONFIG_FILES[0]).exists());
        term.send(b"yes\n");
        assert!(term.finish().success(), "{}", term.output);
        let loaded = load(repo.path());
        assert_eq!(
            loaded.save_plan,
            SavePlan::parse(&["stash:staged".into(), "commit:unstaged".into()]).unwrap()
        );
        assert_eq!(loaded.save_message, "Literal $message `text`");
        assert_eq!(loaded.save_stash_message, "Local $label");
    });
}
