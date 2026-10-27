#[path = "support/snapshot.rs"]
mod preservation;
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
mod terminal;

use git_oneflow::git;
use preservation::snapshot;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use support::{Repository, isolated};

#[derive(Clone, Copy)]
struct Invocation {
    executable: &'static str,
    command: &'static str,
}
impl Invocation {
    fn process(self, repo: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(self.executable);
        command.current_dir(repo).arg(self.command).args(args);
        command
    }
    fn run(self, repo: &Path, args: &[&str]) -> Output {
        // Null input and a deadline prove unresolved unattended use cannot hang.
        let mut child = self
            .process(repo, args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("save did not finish promptly: {output:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    #[cfg(target_os = "linux")]
    fn terminal(self, repo: &Path, args: &[&str]) -> terminal::Terminal {
        let mut full_args = vec![self.command];
        full_args.extend_from_slice(args);
        terminal::Terminal::start(self.executable, repo, &full_args)
    }
}
fn cases(name: &str, git_available: bool, test: impl Fn(&Path, Invocation)) {
    isolated(name, git_available, |root| {
        for executable in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
            for command in ["save", "v"] {
                let dir = tempfile::tempdir_in(root).unwrap();
                test(
                    dir.path(),
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
    let bytes = if code == 0 {
        assert!(output.stderr.is_empty(), "{output:?}");
        output.stdout
    } else {
        assert!(output.stdout.is_empty(), "{output:?}");
        output.stderr
    };
    let output = String::from_utf8(bytes).unwrap();
    contains(&output, expected);
    output
}
fn contains(output: &str, expected: &[&str]) {
    for expected in expected {
        assert!(output.contains(expected), "missing {expected:?}: {output}");
    }
}
fn fixture(root: &Path) -> (Repository, PathBuf) {
    let repo = Repository::new(root);
    fs::write(repo.path().join("tracked"), "base\n").unwrap();
    fs::write(repo.path().join(".gitignore"), "ignored\n").unwrap();
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", "base"]);
    repo.success(&["switch", "-c", "work"]);
    let remote = root.join(format!(
        "{}.git",
        repo.path().file_name().unwrap().to_string_lossy()
    ));
    assert!(
        git::run(
            root,
            ["init".as_ref(), "--bare".as_ref(), remote.as_os_str()]
        )
        .unwrap()
        .status
        .success()
    );
    repo.success(&[
        "remote".as_ref(),
        "add".as_ref(),
        "origin".as_ref(),
        remote.as_os_str(),
    ]);
    (repo, remote)
}
fn oid(directory: &Path, reference: &str) -> Option<String> {
    let output = git::run(directory, ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().into())
}
fn published(repo: &Repository, remote: &Path) {
    assert_eq!(oid(remote, "refs/heads/work"), oid(repo.path(), "HEAD"));
    assert_eq!(
        repo.success(&["config", "branch.work.remote"]).stdout,
        b"origin\n"
    );
    assert_eq!(
        repo.success(&["config", "branch.work.merge"]).stdout,
        b"refs/heads/work\n"
    );
}
fn dirty(repo: &Repository, staged: bool, remaining: bool) {
    if staged {
        fs::write(repo.path().join("tracked"), "selected\n").unwrap();
        repo.success(&["add", "tracked"]);
    }
    if remaining {
        fs::write(repo.path().join("tracked"), "remaining\n").unwrap();
        fs::write(repo.path().join("untracked"), "new\n").unwrap();
    }
    fs::write(repo.path().join("ignored"), "ignored local\n").unwrap();
}
fn settings(repo: &Repository, plan: &str) {
    fs::write(
        repo.path().join(".git/git-oneflow.config.toml"),
        format!("save-plan = [{plan}]\n"),
    )
    .unwrap();
}

#[test]
fn save_help_and_invalid_syntax_work_without_git_or_repository() {
    cases(
        "save_help_and_invalid_syntax_work_without_git_or_repository",
        false,
        |root, cli| {
            for arg in ["--help", "-h"] {
                check(
                    cli.run(root, &[arg]),
                    0,
                    &[
                        "Usage: git-oneflow save",
                        "Alias: v",
                        "save-plan",
                        "--commit",
                        "--stash-message",
                        "cancel",
                    ],
                );
            }
            for args in [
                vec!["branch"],
                vec!["--unknown"],
                vec!["--commit"],
                vec!["--stash"],
                vec!["--commit", "unknown"],
                vec!["--staged"],
                vec!["--wip"],
                vec!["--commit", "staged", "--commit", "staged"],
                vec!["--commit", "all", "--stash", "unstaged"],
                vec!["--publish-only", "--commit", "staged"],
                vec!["--publish-only", "--publish-only"],
                vec!["--message", "a", "--message", "b"],
                vec!["--message"],
                vec!["--stash-message", ""],
                vec!["--message=a"],
                vec!["--"],
                vec!["--stash", "all", "--message", "unused"],
                vec!["--commit", "all", "--stash-message", "unused"],
            ] {
                check(cli.run(root, &args), 2, &["use save --help"]);
            }
        },
    );
}

#[test]
fn save_clean_empty_selected_groups_publish_without_questions() {
    cases(
        "save_clean_empty_selected_groups_publish_without_questions",
        true,
        |root, cli| {
            for args in [
                vec![],
                vec!["--publish-only"],
                vec!["--commit", "staged"],
                vec!["--stash", "unstaged"],
                vec!["--commit", "staged", "--stash", "unstaged"],
            ] {
                let (repo, remote) = fixture(root);
                let head = oid(repo.path(), "HEAD");
                check(
                    cli.run(repo.path(), &args),
                    0,
                    &["Save complete", "confirmed at every destination"],
                );
                published(&repo, &remote);
                assert_eq!(oid(repo.path(), "HEAD"), head);
                assert!(oid(repo.path(), "refs/stash").is_none());
            }
        },
    );
}

#[test]
fn save_configured_plan_labels_and_cli_replacement_are_observable() {
    cases(
        "save_configured_plan_labels_and_cli_replacement_are_observable",
        true,
        |root, cli| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            settings(&repo, "'commit:staged', 'stash:unstaged'");
            check(
                cli.run(
                    repo.path(),
                    &[
                        "--message",
                        "Routine checkpoint",
                        "--stash-message",
                        "Local remainder",
                    ],
                ),
                0,
                &[
                    "Save complete",
                    "Routine checkpoint",
                    "Retained local stash",
                ],
            );
            published(&repo, &remote);
            assert_eq!(
                repo.success(&["show", "HEAD:tracked"]).stdout,
                b"selected\n"
            );
            assert!(repo.success(&["status", "--porcelain"]).stdout.is_empty());
            let stash = oid(repo.path(), "refs/stash").unwrap();
            repo.success(&["stash", "apply", "--index", &stash]);
            assert_eq!(
                fs::read(repo.path().join("tracked")).unwrap(),
                b"remaining\n"
            );
            assert_eq!(fs::read(repo.path().join("untracked")).unwrap(), b"new\n");
            let old_head = oid(repo.path(), "HEAD");
            check(
                cli.run(repo.path(), &["--publish-only"]),
                0,
                &["Save complete"],
            );
            assert_eq!(oid(repo.path(), "HEAD"), old_head);
            check(
                cli.run(repo.path(), &["--commit", "all"]),
                0,
                &["Save complete"],
            );
            assert_ne!(oid(repo.path(), "HEAD"), old_head);
            assert_eq!(
                fs::read(repo.path().join("ignored")).unwrap(),
                b"ignored local\n"
            );
        },
    );
}

#[test]
fn save_effective_label_errors_and_invalid_selection_leave_everything_unchanged() {
    cases(
        "save_effective_label_errors_and_invalid_selection_leave_everything_unchanged",
        true,
        |root, cli| {
            let (repo, remote) = fixture(root);
            settings(&repo, "'stash:all'");
            for dirty_tree in [false, true] {
                if dirty_tree {
                    dirty(&repo, true, true);
                }
                let before = snapshot(repo.path());
                let remote_before = snapshot(&remote);
                for args in [
                    vec!["--commit", "all", "--commit", "staged"],
                    vec!["--stash", "staged", "--commit", "staged"],
                    vec!["--publish-only", "--stash", "unstaged"],
                ] {
                    check(cli.run(repo.path(), &args), 2, &["use save --help"]);
                    assert_eq!(snapshot(repo.path()), before);
                    assert_eq!(snapshot(&remote), remote_before);
                }
                check(
                    cli.run(repo.path(), &["--message", "unused"]),
                    1,
                    &["error:"],
                );
                assert_eq!(snapshot(repo.path()), before);
                assert_eq!(snapshot(&remote), remote_before);
            }
        },
    );
}

#[test]
fn save_message_values_are_literal_and_options_may_be_reordered() {
    cases(
        "save_message_values_are_literal_and_options_may_be_reordered",
        true,
        |root, cli| {
            for message in ["--wip", "-h", "cancel", "  literal ;$ value  "] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                check(
                    cli.run(repo.path(), &["--message", message, "--commit", "all"]),
                    0,
                    &["Save complete", message.trim()],
                );
                published(&repo, &remote);
                assert_eq!(
                    repo.success(&["log", "-1", "--format=%s"]).stdout,
                    format!("{}\n", message.trim_end()).as_bytes()
                );
            }
        },
    );
}

#[test]
fn save_unresolved_dirty_work_and_piped_input_have_no_effects() {
    cases(
        "save_unresolved_dirty_work_and_piped_input_have_no_effects",
        true,
        |root, cli| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            let before = snapshot(repo.path());
            check(
                cli.run(repo.path(), &[]),
                1,
                &["requires an action", "no interactive terminal"],
            );
            let mut child = cli
                .process(repo.path(), &[])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"commit-all\n")
                .unwrap();
            check(
                child.wait_with_output().unwrap(),
                1,
                &["no interactive terminal"],
            );
            assert_eq!(snapshot(repo.path()), before);
            assert!(oid(&remote, "refs/heads/work").is_none());
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn save_terminal_selects_combined_and_ordered_group_plans_without_message_prompts() {
    cases(
        "save_terminal_selects_combined_and_ordered_group_plans_without_message_prompts",
        true,
        |root, cli| {
            for answers in [
                vec!["commit-all"],
                vec!["stash-all"],
                vec!["publish-only"],
                vec!["staged-first", "commit", "stash"],
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let mut terminal = cli.terminal(repo.path(), &[]);
                terminal.until("How would you like to save this work?");
                for (index, answer) in answers.iter().enumerate() {
                    if index == 1 {
                        terminal.until("How should staged changes be preserved?");
                    }
                    if index == 2 {
                        terminal.until("How should unstaged changes be preserved?");
                    }
                    terminal.send(format!("{answer}\n").as_bytes());
                }
                assert!(terminal.finish().success(), "{}", terminal.output);
                assert!(!terminal.output.contains("commit message"));
                published(&repo, &remote);
            }
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            settings(&repo, "'commit:all'");
            let mut terminal = cli.terminal(repo.path(), &[]);
            assert!(terminal.finish().success(), "{}", terminal.output);
            assert!(!terminal.output.contains("How would you like"));
            published(&repo, &remote);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn save_terminal_correction_cancellation_and_context_revalidation_preserve_work() {
    cases(
        "save_terminal_correction_cancellation_and_context_revalidation_preserve_work",
        true,
        |root, cli| {
            for answer in [
                "cancel",
                "invalid",
                "eof",
                "interrupt",
                "split-cancel",
                "changed",
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let mut terminal = cli.terminal(repo.path(), &[]);
                terminal.until("How would you like to save this work?");
                if answer == "changed" {
                    fs::write(repo.path().join("tracked"), "changed during prompt\n").unwrap();
                }
                let before = snapshot(repo.path());
                let remote_before = snapshot(&remote);
                match answer {
                    "invalid" => {
                        terminal.send(b"unknown\n");
                        terminal.until("Choose a listed name or number");
                        terminal.send(b"cancel\n");
                    }
                    "eof" => terminal.send(b"\x04"),
                    "interrupt" => terminal.interrupt(),
                    "split-cancel" => {
                        terminal.send(b"staged-first\n");
                        terminal.until("How should staged changes be preserved?");
                        terminal.send(b"commit\n");
                        terminal.until("How should unstaged changes be preserved?");
                        terminal.send(b"cancel\n");
                    }
                    "changed" => terminal.send(b"commit-all\n"),
                    _ => terminal.send(b"cancel\n"),
                }
                assert!(!terminal.finish().success(), "{}", terminal.output);
                assert_eq!(snapshot(repo.path()), before);
                assert_eq!(snapshot(&remote), remote_before);
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn save_capable_keyboard_choice_and_cancel_restore_terminal() {
    isolated(
        "save_capable_keyboard_choice_and_cancel_restore_terminal",
        true,
        |root| {
            for accept in [true, false] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let before = snapshot(repo.path());
                let mut terminal = terminal::Terminal::start_capable(
                    env!("CARGO_BIN_EXE_gof"),
                    repo.path(),
                    &["save"],
                );
                terminal.until("How would you like to save this work?");
                if accept {
                    terminal.send(b"\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A\r");
                } else {
                    terminal.send(b"\x04");
                }
                assert_eq!(terminal.finish().success(), accept, "{}", terminal.output);
                terminal.assert_restored();
                if accept {
                    published(&repo, &remote);
                } else {
                    assert_eq!(snapshot(repo.path()), before);
                    assert!(oid(&remote, "refs/heads/work").is_none());
                }
            }
        },
    );
}

#[cfg(unix)]
fn hook(directory: &Path, name: &str, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(directory).unwrap();
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\n{contents}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(unix)]
fn local_hook(repo: &Repository, name: &str, contents: &str) {
    let directory = repo.path().join(".git/hooks");
    hook(&directory, name, contents);
    repo.success(&[
        "config".as_ref(),
        "core.hooksPath".as_ref(),
        directory.as_os_str(),
    ]);
}
#[cfg(unix)]
#[test]
fn save_failures_report_retained_checkpoints_stashes_and_remote_effects() {
    cases(
        "save_failures_report_retained_checkpoints_stashes_and_remote_effects",
        true,
        |root, cli| {
            for action in ["commit", "stash"] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                local_hook(
                    &repo,
                    "pre-push",
                    "printf 'blocked\\033[31m\\n' >&2\nexit 1",
                );
                let output = check(
                    cli.run(
                        repo.path(),
                        &[
                            if action == "commit" {
                                "--commit"
                            } else {
                                "--stash"
                            },
                            "all",
                        ],
                    ),
                    1,
                    &[
                        "Save incomplete",
                        "unconfirmed; remote effects may exist",
                        "Push process",
                        "blocked\\u{1b}[31m",
                        "No rollback",
                    ],
                );
                assert!(!output.contains('\u{1b}'));
                assert!(oid(&remote, "refs/heads/work").is_none());
                contains(
                    &output,
                    &[if action == "commit" {
                        "Local checkpoint retained"
                    } else {
                        "Retained local stash"
                    }],
                );
                fs::remove_file(repo.path().join(".git/hooks/pre-push")).unwrap();
                let head = oid(repo.path(), "HEAD");
                check(cli.run(repo.path(), &[]), 0, &["Save complete"]);
                published(&repo, &remote);
                assert_eq!(oid(repo.path(), "HEAD"), head);
            }
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            let rejected = root.join("rejected.git");
            assert!(
                git::run(
                    root,
                    ["init".as_ref(), "--bare".as_ref(), rejected.as_os_str()]
                )
                .unwrap()
                .status
                .success()
            );
            hook(&rejected.join("hooks"), "pre-receive", "exit 1");
            for destination in [&remote, &rejected] {
                repo.success(&[
                    "config".as_ref(),
                    "--add".as_ref(),
                    "remote.origin.pushurl".as_ref(),
                    destination.as_os_str(),
                ]);
            }
            check(
                cli.run(repo.path(), &["--commit", "all"]),
                1,
                &[
                    "Local checkpoint retained",
                    "partly confirmed",
                    "created",
                    "rejected",
                    "Observed upstream tracking: absent",
                    "No rollback",
                ],
            );
            assert_eq!(oid(&remote, "refs/heads/work"), oid(repo.path(), "HEAD"));
            assert!(oid(&rejected, "refs/heads/work").is_none());
        },
    );
}
#[cfg(unix)]
#[test]
fn save_reports_local_failed_step_effects_and_confirmed_publication_with_unfinished_tracking() {
    cases(
        "save_reports_local_failed_step_effects_and_confirmed_publication_with_unfinished_tracking",
        true,
        |root, cli| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            local_hook(
                &repo,
                "pre-commit",
                "printf 'hook work\\n' > hook-file\ngit add -- hook-file\nexit 1",
            );
            check(
                cli.run(repo.path(), &["--commit", "all"]),
                1,
                &[
                    "Save incomplete",
                    "publication not attempted",
                    "Local commit process",
                    "Work still local: 3 staged",
                    "Retained save recovery",
                    "Failed/interrupted Git steps can leave additional effects",
                ],
            );
            assert_eq!(repo.success(&["show", ":hook-file"]).stdout, b"hook work\n");
            let recovery = String::from_utf8(
                repo.success(&[
                    "for-each-ref",
                    "--format=%(objectname)",
                    "refs/git-oneflow/save-recovery/",
                ])
                .stdout,
            )
            .unwrap();
            let recovery = recovery.trim();
            assert!(!recovery.is_empty());
            assert_eq!(
                repo.success(&["show", &format!("{recovery}^2:tracked")])
                    .stdout,
                b"selected\n"
            );
            assert_eq!(
                repo.success(&["show", &format!("{recovery}:tracked")])
                    .stdout,
                b"remaining\n"
            );
            assert_eq!(
                repo.success(&["show", &format!("{recovery}^3:untracked")])
                    .stdout,
                b"new\n"
            );

            assert!(oid(&remote, "refs/heads/work").is_none());
            let (repo, remote) = fixture(root);
            local_hook(&repo, "pre-push", "printf 'lock\\n' > .git/config.lock");
            check(
                cli.run(repo.path(), &[]),
                1,
                &[
                    "Save incomplete",
                    "confirmed at every destination",
                    "branch published, but tracking setup failed",
                    "Observed upstream tracking: absent; setup unfinished",
                ],
            );
            assert_eq!(oid(&remote, "refs/heads/work"), oid(repo.path(), "HEAD"));
        },
    );
}
#[cfg(unix)]
#[test]
fn save_lost_acknowledgement_and_interrupted_git_steps_never_report_success() {
    cases(
        "save_lost_acknowledgement_and_interrupted_git_steps_never_report_success",
        true,
        |root, cli| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            hook(
                &remote.join("hooks"),
                "reference-transaction",
                "if test \"$1\" = committed; then kill -KILL \"$PPID\"; fi",
            );
            let output = check(
                cli.run(repo.path(), &["--commit", "all"]),
                1,
                &[
                    "Save incomplete",
                    "Local checkpoint retained",
                    "unconfirmed; remote effects may exist",
                ],
            );
            assert!(!output.contains("Save complete"));
            // Independent fixture observation proves remote effects despite lost acknowledgement.
            assert_eq!(oid(&remote, "refs/heads/work"), oid(repo.path(), "HEAD"));
            let (repo, remote) = fixture(root);
            dirty(&repo, true, false);
            local_hook(&repo, "pre-commit", "kill -INT \"$PPID\"");
            let output = check(
                cli.run(
                    repo.path(),
                    &["--commit", "staged", "--message", "interrupted"],
                ),
                1,
                &[
                    "Save incomplete",
                    "Local commit process",
                    "publication not attempted",
                ],
            );
            assert!(!output.contains("Save complete"));
            assert!(oid(&remote, "refs/heads/work").is_none());
            let (repo, remote) = fixture(root);
            dirty(&repo, true, false);
            local_hook(&repo, "post-commit", "printf 'hook work\\n' > hook-file");
            let head = oid(repo.path(), "HEAD");
            let output = check(
                cli.run(repo.path(), &["--commit", "all"]),
                1,
                &[
                    "Save incomplete",
                    "Observed HEAD",
                    "1 untracked",
                    "publication not attempted",
                ],
            );
            assert_ne!(oid(repo.path(), "HEAD"), head);
            contains(&output, &[&oid(repo.path(), "HEAD").unwrap()]);
            assert!(oid(&remote, "refs/heads/work").is_none());
        },
    );
}
