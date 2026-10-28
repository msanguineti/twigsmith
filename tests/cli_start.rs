#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use preservation::snapshot;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use support::{Repository, isolated};

#[derive(Clone, Copy)]
struct Invocation {
    executable: &'static str,
    command: &'static str,
}

impl Invocation {
    fn run(self, directory: &Path, args: &[&str]) -> Output {
        let mut args = args.to_vec();
        if args.first() == Some(&"start") {
            args[0] = self.command;
        }
        Command::new(self.executable)
            .current_dir(directory)
            .args(args)
            .output()
            .unwrap()
    }

    fn start(self, repo: &Repository, input: &str) -> Output {
        self.run(repo.path(), &["start", input])
    }
}

// Detailed behavior shares the dispatcher; spelling coverage stays in the
// grammar/help cases and the public lifecycle entrypoint matrix.
fn cli_cases(name: &str, git_available: bool, test: impl Fn(&Path, Invocation)) {
    isolated(name, git_available, |root| {
        test(
            root,
            Invocation {
                executable: env!("CARGO_BIN_EXE_twigsmith"),
                command: "start",
            },
        );
    });
}

fn cli_spellings(name: &str, git_available: bool, test: impl Fn(&Path, Invocation)) {
    isolated(name, git_available, |root| {
        for executable in [env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")] {
            for command in ["start", "s"] {
                let fixture = tempfile::tempdir_in(root).unwrap();
                test(
                    fixture.path(),
                    Invocation {
                        executable,
                        command,
                    },
                );
            }
        }
    });
}
fn check(output: Output, code: i32, expected: &[&str]) -> String {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    let text = String::from_utf8(if code == 0 {
        assert!(output.stderr.is_empty());
        output.stdout
    } else {
        assert!(output.stdout.is_empty());
        output.stderr
    })
    .unwrap();
    for expected in expected {
        assert!(text.contains(expected), "missing {expected:?}: {text}");
    }
    text
}
fn commit(repo: &Repository, content: &str) -> String {
    fs::write(repo.path().join("tracked"), content).unwrap();
    repo.success(&["add", "tracked"]);
    repo.success(&["commit", "-m", "fixture"]);
    oid(repo, "HEAD")
}
fn oid(repo: &Repository, reference: &str) -> String {
    String::from_utf8(repo.success(&["rev-parse", reference]).stdout)
        .unwrap()
        .trim()
        .into()
}
fn pair(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base\n");
    let local = Repository::new(root);
    local.success(&[
        "remote".as_ref(),
        "add".as_ref(),
        "origin".as_ref(),
        remote.path().as_os_str(),
    ]);
    local.success(&["fetch", "origin"]);
    local.success(&["reset", "--hard", "origin/main"]);
    (remote, local)
}
#[test]
fn cli_start_literal_names_use_fresh_base() {
    cli_cases(
        "cli_start_literal_names_use_fresh_base",
        true,
        |root, cli| {
            let (remote, local) = pair(root);
            let old = oid(&local, "main");
            let fresh = commit(&remote, "fresh\n");
            local.success(&["config", "branch.autoSetupMerge", "always"]);
            let nested = local.path().join("nested");
            fs::create_dir(&nested).unwrap();
            check(
                cli.run(&nested, &["start", "Add-CAFÉ-avatars"]),
                0,
                &["Created branch Add-CAFÉ-avatars"],
            );
            assert_eq!(oid(&local, "HEAD"), fresh);
            local.success(&["switch", "main"]);
            check(
                cli.start(&local, "Nasty-Deployment-Bug"),
                0,
                &["Created branch Nasty-Deployment-Bug"],
            );
            for name in [
                "feature",
                "Fix/DeploymentBug",
                "fix/parser_v2",
                "fix/Empty-Response",
                "docs/E\u{301}cole",
                "release/V1",
                "hotfix/Urgent",
                "notes",
                "docs/École",
                "custom/$;literal",
                "中文/工作",
            ] {
                local.success(&["switch", "main"]);
                check(
                    cli.start(&local, name),
                    0,
                    &[&format!("Created branch {name}")],
                );
                assert_eq!(oid(&local, "HEAD"), fresh);
                assert_eq!(
                    local.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                    format!("{name}\n").as_bytes()
                );
                let upstream = twigsmith::git::run(
                    local.path(),
                    ["config", "--get", &format!("branch.{name}.remote")],
                )
                .unwrap();
                assert_eq!(upstream.status.code(), Some(1));
            }
            assert_eq!(oid(&local, "main"), old);
        },
    );
}

#[test]
fn cli_start_local_resume_and_dirty_current_noop_do_not_contact_remote() {
    cli_cases(
        "cli_start_local_resume_and_dirty_current_noop_do_not_contact_remote",
        true,
        |root, cli| {
            let repo = Repository::new(root);
            commit(&repo, "base\n");
            repo.success(&["switch", "-c", "release/Local"]);
            let work = commit(&repo, "work\n");
            repo.success(&["switch", "main"]);
            repo.success(&["remote", "add", "origin", "/unreachable-oneflow-cli"]);
            let config = fs::read(repo.path().join(".git/config")).unwrap();
            check(
                cli.start(&repo, "release/Local"),
                0,
                &["Resumed local branch release/Local"],
            );
            assert_eq!(oid(&repo, "HEAD"), work);
            assert_eq!(fs::read(repo.path().join(".git/config")).unwrap(), config);
            fs::write(repo.path().join("tracked"), "dirty\n").unwrap();
            let before = snapshot(root);
            check(
                cli.start(&repo, "release/Local"),
                0,
                &["Already current: branch release/Local"],
            );
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn cli_start_resumes_exact_unicode_names() {
    cli_cases(
        "cli_start_resumes_exact_unicode_names",
        true,
        |root, cli| {
            let repo = Repository::new(root);
            let base = commit(&repo, "base\n");
            for name in [
                "Fix/DeploymentBug",
                "fix/parser_v2",
                "docs/École",
                "docs/E\u{301}cole",
                "feature",
                "中文/工作",
            ] {
                repo.success(&["switch", "-c", name, "main"]);
                let work = commit(&repo, name);
                repo.success(&["switch", "main"]);
                check(
                    cli.start(&repo, name),
                    0,
                    &[&format!("Resumed local branch {name}")],
                );
                assert_eq!(oid(&repo, "HEAD"), work);
                assert_eq!(
                    repo.success(&["symbolic-ref", "HEAD"]).stdout,
                    format!("refs/heads/{name}\n").as_bytes()
                );
                repo.success(&["switch", "main"]);
            }
            assert_eq!(oid(&repo, "main"), base);
            repo.success(&["branch", "café-work"]);
            for input in ["CAFÉ work", "CAFE\u{301} work", "中文 工作"] {
                let before = snapshot(root);
                check(
                    cli.start(&repo, input),
                    1,
                    &["invalid target branch name", "without spaces"],
                );
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn cli_start_remote_resume_tracks_exact_counterpart_for_every_prefix() {
    cli_cases(
        "cli_start_remote_resume_tracks_exact_counterpart_for_every_prefix",
        true,
        |root, cli| {
            let (remote, local) = pair(root);
            for name in [
                "feature/remote",
                "fix/Remote",
                "release/Remote",
                "hotfix/Remote",
                "plain",
            ] {
                remote.success(&["switch", "-c", name, "main"]);
                let work = commit(&remote, name);
                remote.success(&["switch", "main"]);
                check(
                    cli.start(&local, name),
                    0,
                    &[&format!("Resumed remote branch {name}")],
                );
                assert_eq!(oid(&local, "HEAD"), work);
                assert_eq!(
                    local
                        .success(&["config", "--get", &format!("branch.{name}.remote")])
                        .stdout,
                    b"origin\n"
                );
                assert_eq!(
                    local
                        .success(&["config", "--get", &format!("branch.{name}.merge")])
                        .stdout,
                    format!("refs/heads/{name}\n").as_bytes()
                );
                local.success(&["switch", "main"]);
            }
        },
    );
}

#[test]
fn cli_start_malformed_arguments_are_preserved_syntax_failures() {
    cli_spellings(
        "cli_start_malformed_arguments_are_preserved_syntax_failures",
        true,
        |root, cli| {
            let (_remote, repo) = pair(root);
            for args in [
                vec!["start"],
                vec!["start", "feature", "description"],
                vec!["start", "work", "extra"],
                vec!["start", "work", "--name", "target"],
                vec!["start", "feature", "work", "--name", "target"],
                vec!["start", "--name"],
                vec!["start", "--name=target"],
                vec!["start", "--dry-run"],
                vec!["start", "--", "work"],
                vec!["start", "work", "--help"],
                vec!["start", "feature", "--help"],
                vec!["st", "work"],
                vec!["sta", "work"],
                vec!["S", "work"],
            ] {
                let before = snapshot(root);
                check(cli.run(repo.path(), &args), 2, &["use --help"]);
                assert_eq!(snapshot(root), before, "{args:?}");
            }
            #[cfg(unix)]
            {
                use std::{ffi::OsString, os::unix::ffi::OsStringExt};
                let before = snapshot(root);
                let output = Command::new(cli.executable)
                    .current_dir(repo.path())
                    .args([OsString::from(cli.command), OsString::from_vec(vec![0xff])])
                    .output()
                    .unwrap();
                check(output, 2, &["expected start <branch-name>"]);
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn cli_start_eligibility_settings_and_remote_errors() {
    cli_cases(
        "cli_start_eligibility_settings_and_remote_errors",
        true,
        |root, cli| {
            check(
                cli.run(root, &["start", "work"]),
                1,
                &["Git repository", "enter a Git working tree"],
            );
            let repo = Repository::new(root);
            check(
                cli.start(&repo, "work"),
                1,
                &["HEAD has no commit", "initial branch commit"],
            );
            commit(&repo, "base\n");
            for (setup, expected) in [
                ("dirty", "contain changes"),
                ("detached", "HEAD is detached"),
                ("merge", "Git operation is active"),
            ] {
                match setup {
                    "dirty" => fs::write(repo.path().join("tracked"), "dirty").unwrap(),
                    "detached" => {
                        repo.success(&["switch", "--detach"]);
                    }
                    _ => {
                        fs::write(repo.path().join(".git/MERGE_HEAD"), oid(&repo, "HEAD")).unwrap()
                    }
                }
                let before = snapshot(root);
                check(
                    cli.start(&repo, "work"),
                    1,
                    &[expected, "Completed steps: 0"],
                );
                assert_eq!(snapshot(root), before);
                match setup {
                    "dirty" => {
                        repo.success(&["reset", "--hard"]);
                    }
                    "detached" => {
                        repo.success(&["switch", "main"]);
                    }
                    _ => fs::remove_file(repo.path().join(".git/MERGE_HEAD")).unwrap(),
                }
            }
            for (input, expected) in [
                ("main", "configured base branch"),
                ("bad\x1bname", "invalid target branch"),
                ("", "invalid target branch"),
                ("@{-1}", "invalid target branch"),
                ("main~1", "invalid target branch"),
                ("a..b", "invalid target branch"),
                (" !!! ", "invalid target branch name"),
            ] {
                let before = snapshot(root);
                check(cli.start(&repo, input), 1, &[expected]);
                assert_eq!(snapshot(root), before, "{input:?}");
            }
            fs::write(
                repo.path().join(".git/twigsmith.config.toml"),
                "base = 'main'\nbase = 'trunk'\n",
            )
            .unwrap();
            let before = snapshot(root);
            check(
                cli.start(&repo, "work"),
                1,
                &["invalid settings:", "duplicate key", "tgi config show"],
            );
            assert_eq!(snapshot(root), before);
            fs::remove_file(repo.path().join(".git/twigsmith.config.toml")).unwrap();
            check(
                cli.start(&repo, "work"),
                1,
                &[
                    "Git refused",
                    "origin",
                    "configured publication remote and project base",
                ],
            );
            repo.success(&["remote", "add", "origin", "/oneflow-unreachable-cli"]);
            check(
                cli.start(&repo, "work"),
                1,
                &["Git refused", "ls-remote", "Completed steps: 0"],
            );
            let (remote, local) = pair(root);
            fs::write(
                local.path().join(".git/twigsmith.config.toml"),
                "base = 'absent'\n",
            )
            .unwrap();
            check(
                cli.start(&local, "work"),
                1,
                &["no exact base branch", "absent"],
            );
            remote.success(&["branch", "trunk"]);
            local.success(&["remote", "rename", "origin", "upstream"]);
            fs::write(
                local.path().join(".git/twigsmith.config.toml"),
                "publish-remote = 'upstream'\nbase = 'trunk'\n",
            )
            .unwrap();
            check(
                cli.start(&local, "custom-base"),
                0,
                &["Created branch custom-base"],
            );
            assert_eq!(oid(&local, "HEAD"), oid(&remote, "trunk"));
        },
    );
}

#[test]
fn cli_start_help_is_independent_of_git_repository_and_settings() {
    cli_spellings(
        "cli_start_help_is_independent_of_git_repository_and_settings",
        true,
        |root, cli| {
            let repo = Repository::new(root);
            fs::write(
                repo.path().join(".git/twigsmith.config.toml"),
                "invalid settings",
            )
            .unwrap();
            for directory in [root, repo.path()] {
                for args in [
                    vec!["--help"],
                    vec!["--version"],
                    vec!["start", "--help"],
                    vec!["start", "-h"],
                    vec!["status", "--help"],
                    vec!["status", "-h"],
                ] {
                    let before = snapshot(root);
                    let output = Command::new(cli.executable)
                        .current_dir(directory)
                        .args(
                            args.iter()
                                .map(|arg| if *arg == "start" { cli.command } else { arg }),
                        )
                        .env("PATH", "")
                        .output()
                        .unwrap();
                    let help = check(output, 0, &[]);
                    if args[0] == "start" {
                        for content in [
                            "Usage: twigsmith start <branch-name>",
                            "Alias: s",
                            "Arguments:",
                            "Options:",
                            "Examples:",
                            "tgi s fix/parser_v2",
                            "Settings:",
                            "Exit status:",
                        ] {
                            assert!(help.contains(content));
                        }
                        assert!(!help.contains("--name"));
                        assert!(!help.contains("start feature"));
                    }
                    assert_eq!(snapshot(root), before);
                }
            }
            let output = Command::new(cli.executable)
                .current_dir(root)
                .args([cli.command, "work"])
                .env("PATH", "")
                .output()
                .unwrap();
            check(output, 1, &["Git is installed on PATH"]);
        },
    );
}

#[cfg(unix)]
#[test]
fn cli_start_partial_failure_reports_completed_effects_and_actual_head_safely() {
    cli_cases(
        "cli_start_partial_failure_reports_completed_effects_and_actual_head_safely",
        true,
        |root, cli| {
            use std::os::unix::fs::PermissionsExt;
            let (_remote, repo) = pair(root);
            let hooks = repo.path().join(".git/hooks\n\x1bpath");
            fs::create_dir(&hooks).unwrap();
            let hook = hooks.join("post-checkout");
            fs::write(
                &hook,
                "#!/bin/sh\nprintf 'hook refused\\033[31m\\n' >&2\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            repo.success(&[
                "config".as_ref(),
                "core.hooksPath".as_ref(),
                hooks.as_os_str(),
            ]);
            let output = check(
                cli.start(&repo, "partial"),
                1,
                &[
                    "Git refused",
                    "hook refused",
                    "Completed steps: 2",
                    "Fetched",
                    "Created branch partial",
                    "Current branch: partial",
                    "Target branch partial:",
                    "No rollback",
                ],
            );
            assert!(!output.contains('\x1b'));
            assert_eq!(
                repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                b"partial\n"
            );
            let before = snapshot(root);
            check(cli.start(&repo, "partial"), 0, &["Already current"]);
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn cli_start_occupied_worktree_paths_and_settings_paths_escape_controls() {
    cli_cases(
        "cli_start_occupied_worktree_paths_and_settings_paths_escape_controls",
        true,
        |root, cli| {
            let repo = Repository::new(root);
            commit(&repo, "base\n");
            let linked = root.join("linked\n\x1bpath");
            repo.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "-b".as_ref(),
                "occupied".as_ref(),
                linked.as_os_str(),
            ]);
            let before = snapshot(root);
            let output = check(
                cli.start(&repo, "occupied"),
                1,
                &[
                    "checked out at",
                    "linked\\n\\u{1b}path",
                    "use that worktree",
                ],
            );
            assert!(!output.contains('\x1b'));
            assert_eq!(snapshot(root), before);
            let settings = repo.path().join(".git/twigsmith.config.toml");
            fs::create_dir(&settings).unwrap();
            check(
                cli.start(&repo, "work"),
                1,
                &["cannot read settings", "twigsmith.config.toml"],
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn cli_start_fetch_refusal_and_config_lock_report_remaining_state() {
    cli_cases(
        "cli_start_fetch_refusal_and_config_lock_report_remaining_state",
        true,
        |root, cli| {
            use std::os::unix::fs::PermissionsExt;
            let (remote, local) = pair(root);
            remote.success(&["branch", "remote-work"]);
            let script = local.path().join(".git/transport-probe");
            let counter = local.path().join(".git/transport-count");
            let quote =
                |path: &Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
            fs::write(&script, format!("#!/bin/sh\nif test -f {counter}; then printf 'fetch transport refused\\n' >&2; exit 1; fi\ntouch {counter}\nexec git-upload-pack \"$@\"\n", counter = quote(&counter))).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            local.success(&["config", "remote.origin.uploadpack", &quote(&script)]);
            let initial = oid(&local, "HEAD");
            check(
                cli.start(&local, "remote-work"),
                1,
                &[
                    "fetch transport refused",
                    "Git refused",
                    "fetch",
                    "Completed steps: 0",
                    "Target branch remote-work: absent",
                    "Fetched reference refs/remotes/origin/remote-work",
                ],
            );
            assert_eq!(oid(&local, "HEAD"), initial);
            local.success(&["config", "--unset", "remote.origin.uploadpack"]);
            fs::write(local.path().join(".git/config.lock"), "preserve").unwrap();
            check(
                cli.start(&local, "remote-work"),
                1,
                &[
                    "Git refused",
                    "could not lock config file",
                    "Completed steps: 2",
                    "Created branch remote-work",
                    "Current branch: main",
                    "Target upstream: remote not configured",
                ],
            );
            assert_eq!(oid(&local, "HEAD"), initial);
            assert_eq!(oid(&local, "remote-work"), oid(&remote, "remote-work"));
            assert_eq!(
                fs::read(local.path().join(".git/config.lock")).unwrap(),
                b"preserve"
            );
        },
    );
}

#[test]
fn cli_start_bare_repository_is_a_preserved_failure() {
    cli_cases(
        "cli_start_bare_repository_is_a_preserved_failure",
        true,
        |root, cli| {
            let repo = Repository::new(root);
            commit(&repo, "base\n");
            let bare = root.join("bare.git");
            repo.success(&[
                "clone".as_ref(),
                "--bare".as_ref(),
                repo.path().as_os_str(),
                bare.as_os_str(),
            ]);
            let before = snapshot(root);
            check(
                cli.run(&bare, &["start", "work"]),
                1,
                &[
                    "use a checkout instead of this bare repository",
                    "Completed steps: 0",
                ],
            );
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn explicit_hotfix_policy_resumes_published_history_without_creation_tag() {
    use twigsmith::settings::{self, WorkstreamKind};
    cli_cases(
        "explicit_hotfix_policy_resumes_published_history_without_creation_tag",
        true,
        |root, invocation| {
            let (remote, local) = pair(root);
            let base = oid(&local, "main");
            local.success(&["tag", "Train-base", &base]);
            remote.success(&["switch", "-c", "hotfix/train"]);
            let published = commit(&remote, "published fix\n");
            let mut proposed = settings::Settings::default();
            settings::oneflow_bundle(&mut proposed, None);
            // A configured creation default neither resets remote work nor turns
            // omission of CLI creation input into an incompatible resume request.
            proposed
                .workstreams
                .get_mut(&WorkstreamKind::Hotfix)
                .unwrap()
                .creation_source = Some("refs/tags/Train-base".into());
            let path = local.path().join(".git/twigsmith.config.toml");
            fs::write(
                &path,
                twigsmith::config::render(twigsmith::config::Format::Toml, &proposed),
            )
            .unwrap();
            check(
                invocation.run(
                    local.path(),
                    &["start", "hotfix/train", "--policy", "hotfix"],
                ),
                0,
                &["Resumed remote"],
            );
            assert_eq!(oid(&local, "HEAD"), published);
            let before = snapshot(root);
            check(
                invocation.run(
                    local.path(),
                    &[
                        "start",
                        "hotfix/train",
                        "--policy",
                        "hotfix",
                        "--from-tag",
                        "Train-base",
                    ],
                ),
                1,
                &["explicit creation source differs"],
            );
            assert_eq!(snapshot(root), before);
            local.success(&["switch", "main"]);
            proposed
                .workstreams
                .get_mut(&WorkstreamKind::Hotfix)
                .unwrap()
                .creation_source = None;
            fs::write(
                path,
                twigsmith::config::render(twigsmith::config::Format::Toml, &proposed),
            )
            .unwrap();
            let before = snapshot(root);
            check(
                invocation.run(local.path(), &["start", "hotfix/new", "--policy", "hotfix"]),
                1,
                &["--from-tag"],
            );
            assert_eq!(snapshot(root), before);
        },
    );
}
