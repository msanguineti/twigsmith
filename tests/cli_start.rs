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

// Every scenario runs through all four spellings in a fresh fixture.
fn cli_cases(name: &str, git_available: bool, test: impl Fn(&Path, Invocation)) {
    isolated(name, git_available, |root| {
        for executable in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
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
fn cli_start_phrase_and_literal_names_use_fresh_base() {
    cli_cases(
        "cli_start_phrase_and_literal_names_use_fresh_base",
        true,
        |root, cli| {
            let (remote, local) = pair(root);
            let old = oid(&local, "main");
            let fresh = commit(&remote, "fresh\n");
            local.success(&["config", "branch.autoSetupMerge", "always"]);
            let nested = local.path().join("nested");
            fs::create_dir(&nested).unwrap();
            check(
                cli.run(&nested, &["start", "Add CAFÉ avatars"]),
                0,
                &["Created branch add-café-avatars"],
            );
            assert_eq!(oid(&local, "HEAD"), fresh);
            local.success(&["switch", "main"]);
            check(
                cli.start(&local, "--- nasty deployment bug !!!"),
                0,
                &["Created branch nasty-deployment-bug"],
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
                let upstream = git_oneflow::git::run(
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
fn cli_start_resumes_exact_unicode_names_and_canonical_phrases() {
    cli_cases(
        "cli_start_resumes_exact_unicode_names_and_canonical_phrases",
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
            for input in ["CAFÉ work", "CAFE\u{301} work"] {
                check(
                    cli.start(&repo, input),
                    0,
                    &["Resumed local branch café-work"],
                );
                repo.success(&["switch", "main"]);
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
    cli_cases(
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
                check(output, 2, &["expected start <branch-name-or-phrase>"]);
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
                (" !!! ", "no usable letters or numbers"),
            ] {
                let before = snapshot(root);
                check(cli.start(&repo, input), 1, &[expected]);
                assert_eq!(snapshot(root), before, "{input:?}");
            }
            fs::write(
                repo.path().join(".git/oneflow.conf"),
                "base = main\nbase = trunk\n",
            )
            .unwrap();
            let before = snapshot(root);
            check(
                cli.start(&repo, "work"),
                1,
                &[
                    "invalid settings:",
                    "duplicate key",
                    "check git-oneflow.config",
                ],
            );
            assert_eq!(snapshot(root), before);
            fs::remove_file(repo.path().join(".git/oneflow.conf")).unwrap();
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
            fs::write(local.path().join(".git/oneflow.conf"), "base = absent\n").unwrap();
            check(
                cli.start(&local, "work"),
                1,
                &["no exact base branch", "absent"],
            );
            remote.success(&["branch", "trunk"]);
            local.success(&["remote", "rename", "origin", "upstream"]);
            fs::write(
                local.path().join(".git/oneflow.conf"),
                "remote = upstream\nbase = trunk\n",
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
    cli_cases(
        "cli_start_help_is_independent_of_git_repository_and_settings",
        true,
        |root, cli| {
            let repo = Repository::new(root);
            fs::write(repo.path().join(".git/oneflow.conf"), "invalid settings").unwrap();
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
                            "Usage: git-oneflow start <branch-name-or-phrase>",
                            "Alias: s",
                            "Arguments:",
                            "Options:",
                            "Examples:",
                            "gof s fix/parser_v2",
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
            let settings = repo.path().join(".git/oneflow.conf");
            fs::create_dir(&settings).unwrap();
            check(
                cli.start(&repo, "work"),
                1,
                &["cannot read settings", "oneflow.conf"],
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
