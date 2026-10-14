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
fn settings(repo: &Repository, action: &str) {
    fs::write(
        repo.path().join(".git/git-oneflow.config.toml"),
        format!("save-action = '{action}'\n"),
    )
    .unwrap();
}

#[test]
fn save_help_and_syntax_work_without_git_or_repository() {
    cases(
        "save_help_and_syntax_work_without_git_or_repository",
        false,
        |root, cli| {
            for arg in ["--help", "-h"] {
                check(
                    cli.run(root, &[arg]),
                    0,
                    &[
                        "Usage: git-oneflow save",
                        "Alias: v",
                        "Options:",
                        "Examples:",
                        "Settings:",
                        "Exit status:",
                        "save-action",
                        "cancel",
                    ],
                );
            }
            for args in [
                vec!["branch"],
                vec!["--unknown"],
                vec!["--message"],
                vec!["--wip", "--wip"],
                vec!["--staged", "--stash"],
                vec!["--message", "a", "--message", "b"],
                vec!["--message=a"],
                vec!["--"],
                vec!["--help", "--wip"],
                vec!["--wip", "operand"],
            ] {
                check(cli.run(root, &args), 2, &["use save --help"]);
            }
        },
    );
}
#[test]
fn save_clean_actions_publish_without_messages_or_questions() {
    cases(
        "save_clean_actions_publish_without_messages_or_questions",
        true,
        |root, cli| {
            for args in [
                vec![],
                vec!["--publish-only"],
                vec!["--staged"],
                vec!["--wip"],
                vec!["--stash"],
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, false, false);
                let head = oid(repo.path(), "HEAD");
                check(
                    cli.run(repo.path(), &args),
                    0,
                    &[
                        "Save complete",
                        "confirmed at every destination",
                        "created",
                        "Work still local: 0 staged, 0 unstaged tracked, 0 untracked",
                    ],
                );
                published(&repo, &remote);
                assert_eq!(oid(repo.path(), "HEAD"), head);
                assert!(oid(repo.path(), "refs/stash").is_none());
                check(
                    cli.run(repo.path(), &args),
                    0,
                    &["unchanged (already published)"],
                );
            }
        },
    );
}
#[test]
fn save_all_dirty_actions_preserve_and_report_their_selected_scope() {
    cases(
        "save_all_dirty_actions_preserve_and_report_their_selected_scope",
        true,
        |root, cli| {
            for action in ["--publish-only", "--staged", "--wip", "--stash"] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let head = oid(repo.path(), "HEAD");
                let index = fs::read(repo.path().join(".git/index")).unwrap();
                let message = "--literal ;$ `quoted`\n\nDetail\t\u{1b}text";
                let mut args = vec![action];
                if action == "--staged" {
                    args.extend(["--message", message]);
                }
                let output = check(
                    cli.run(repo.path(), &args),
                    0,
                    &["Save complete", "confirmed at every destination"],
                );
                published(&repo, &remote);
                assert!(!output.contains("Observed HEAD"), "{output}");
                assert!(!output.contains("Observed local stash"), "{output}");
                match action {
                    "--publish-only" => {
                        assert_eq!(oid(repo.path(), "HEAD"), head);
                        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
                        contains(
                            &output,
                            &["Work still local: 1 staged, 1 unstaged tracked, 1 untracked"],
                        );
                    }
                    "--staged" => {
                        assert_eq!(
                            repo.success(&["show", "HEAD:tracked"]).stdout,
                            b"selected\n"
                        );
                        assert_eq!(
                            repo.success(&["log", "-1", "--format=%B"]).stdout,
                            format!("{message}\n\n").as_bytes()
                        );
                        contains(
                            &output,
                            &[
                                "Local checkpoint retained",
                                "Work still local: 0 staged, 1 unstaged tracked, 1 untracked",
                                "\\n\\nDetail\\t\\u{1b}text",
                            ],
                        );
                        assert!(!output.contains('\u{1b}'));
                    }
                    "--wip" => {
                        contains(
                            &output,
                            &[
                                "[WIP - Staged]",
                                "[WIP - Unstaged]",
                                "Work still local: 0 staged, 0 unstaged tracked, 0 untracked",
                            ],
                        );
                        assert_eq!(
                            repo.success(&["show", "HEAD~1:tracked"]).stdout,
                            b"selected\n"
                        );
                        assert_eq!(
                            repo.success(&["show", "HEAD:tracked"]).stdout,
                            b"remaining\n"
                        );
                        assert_eq!(repo.success(&["show", "HEAD:untracked"]).stdout, b"new\n");
                    }
                    "--stash" => {
                        assert_eq!(oid(repo.path(), "HEAD"), head);
                        let stash = oid(repo.path(), "refs/stash").unwrap();
                        contains(
                            &output,
                            &[
                                "Retained local stash",
                                &stash,
                                "contents not remotely published",
                                "git stash apply --index",
                                "Work still local: 0 staged, 0 unstaged tracked, 0 untracked",
                            ],
                        );
                        assert_eq!(
                            repo.success(&["show", "stash@{0}:tracked"]).stdout,
                            b"remaining\n"
                        );
                        assert_eq!(
                            repo.success(&["show", "stash@{0}^2:tracked"]).stdout,
                            b"selected\n"
                        );
                        assert!(oid(&remote, "refs/stash").is_none());
                    }
                    _ => unreachable!(),
                }
                assert_eq!(
                    fs::read(repo.path().join("ignored")).unwrap(),
                    b"ignored local\n"
                );
            }
        },
    );
}
#[test]
fn save_preferences_explicit_selection_and_message_validation() {
    cases(
        "save_preferences_explicit_selection_and_message_validation",
        true,
        |root, cli| {
            for action in ["publish-only", "staged", "wip", "stash"] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                settings(&repo, action);
                let args = if action == "staged" {
                    vec!["--message", "configured selection"]
                } else {
                    vec![]
                };
                check(cli.run(repo.path(), &args), 0, &["Save complete"]);
                published(&repo, &remote);
                if action == "stash" {
                    assert!(oid(repo.path(), "refs/stash").is_some());
                }
            }
            for preference in ["ask", "publish-only", "staged", "wip", "stash"] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                settings(&repo, preference);
                let head = oid(repo.path(), "HEAD");
                check(
                    cli.run(repo.path(), &["--publish-only"]),
                    0,
                    &["1 staged, 1 unstaged tracked, 1 untracked"],
                );
                assert_eq!(oid(repo.path(), "HEAD"), head);
                published(&repo, &remote);
                check(
                    cli.run(
                        repo.path(),
                        &["--staged", "--message", "explicit selection"],
                    ),
                    0,
                    &["explicit selection"],
                );
            }
            for preference in ["ask", "publish-only", "wip", "stash"] {
                let (repo, remote) = fixture(root);
                settings(&repo, preference);
                dirty(&repo, true, true);
                let before = snapshot(repo.path());
                check(
                    cli.run(repo.path(), &["--message", "incompatible"]),
                    2,
                    &[
                        "--message requires the staged action",
                        "gof save --staged --message",
                    ],
                );
                assert_eq!(snapshot(repo.path()), before);
                assert!(oid(&remote, "refs/heads/work").is_none());
            }
        },
    );
}
#[test]
fn save_unresolved_and_blank_messages_fail_without_effects() {
    cases(
        "save_unresolved_and_blank_messages_fail_without_effects",
        true,
        |root, cli| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            for (preference, args, expected) in [
                ("ask", vec![], "requires an action"),
                ("staged", vec![], "requires a staged commit message"),
                ("ask", vec!["--staged"], "requires a staged commit message"),
                ("staged", vec!["--message", ""], "nonblank"),
                ("ask", vec!["--staged", "--message", " \n\t"], "nonblank"),
            ] {
                settings(&repo, preference);
                let before = snapshot(repo.path());
                let remote_before = snapshot(&remote);
                check(cli.run(repo.path(), &args), 1, &[expected]);
                assert_eq!(snapshot(repo.path()), before);
                assert_eq!(snapshot(&remote), remote_before);
            }
            repo.success(&["reset", "--quiet"]);
            let before = snapshot(repo.path());
            check(
                cli.run(repo.path(), &["--staged", "--message", "message"]),
                1,
                &["requires staged changes"],
            );
            assert_eq!(snapshot(repo.path()), before);
        },
    );
}
#[cfg(target_os = "linux")]
#[test]
fn save_piped_input_never_substitutes_for_a_terminal_choice() {
    cases(
        "save_piped_input_never_substitutes_for_a_terminal_choice",
        true,
        |root, cli| {
            use std::io::Write;
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            let before = snapshot(repo.path());
            let mut child = cli
                .process(repo.path(), &[])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(b"wip\n").unwrap();
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
fn save_terminal_choices_and_message_entry_use_the_same_backend() {
    cases(
        "save_terminal_choices_and_message_entry_use_the_same_backend",
        true,
        |root, cli| {
            for (action, answer) in [
                ("publish-only", "publish-only"),
                ("staged", "2"),
                ("wip", "3"),
                ("stash", "stash"),
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let mut terminal = cli.terminal(repo.path(), &[]);
                terminal.until("How would you like to save this work?");
                contains(
                    &terminal.output,
                    &[
                        "1 staged, 1 unstaged tracked, 1 untracked",
                        "Commit staged work and publish; keep other work local",
                        "Publish existing commits; keep uncommitted work local",
                        "Stash tracked/untracked work locally; publish existing commits",
                        "cancel",
                    ],
                );
                terminal.send(format!("{answer}\n").as_bytes());
                if action == "staged" {
                    terminal.until("Staged commit message");
                    terminal.send(b"literal --wip ;$ message\n");
                }
                assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                assert_eq!(
                    terminal.output.matches("Save work").count(),
                    1,
                    "{}",
                    terminal.output
                );
                assert_eq!(
                    terminal.output.matches("Branch: work").count(),
                    1,
                    "{}",
                    terminal.output
                );
                contains(
                    &terminal.output,
                    &["Save complete", "confirmed at every destination"],
                );
                published(&repo, &remote);
            }
            for args in [vec!["--staged"], vec![]] {
                let (repo, remote) = fixture(root);
                settings(&repo, "staged");
                dirty(&repo, true, false);
                let mut terminal = cli.terminal(repo.path(), &args);
                terminal.until("Staged commit message");
                assert!(
                    !terminal
                        .output
                        .contains("How would you like to save this work?")
                );
                let message = if args.is_empty() {
                    "configured or explicit message"
                } else {
                    "cancel"
                };
                terminal.send(format!("{message}\n").as_bytes());
                assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                published(&repo, &remote);
                assert_eq!(
                    repo.success(&["log", "-1", "--format=%s"]).stdout,
                    format!("{message}\n").as_bytes()
                );
            }
        },
    );
}
#[cfg(target_os = "linux")]
#[test]
fn save_terminal_omits_inapplicable_actions_and_does_not_prompt_resolved_actions() {
    cases(
        "save_terminal_omits_inapplicable_actions_and_does_not_prompt_resolved_actions",
        true,
        |root, cli| {
            let (repo, _) = fixture(root);
            dirty(&repo, false, true);
            let mut terminal = cli.terminal(repo.path(), &[]);
            terminal.until("How would you like to save this work?");
            assert!(!terminal.output.contains("Commit staged work and publish"));
            terminal.send(b"staged\n");
            terminal.until("Choose a listed name or number");
            terminal.send(b"cancel\n");
            assert_eq!(terminal.finish().code(), Some(1));
            for preference in ["ask", "wip", "stash", "staged"] {
                let (repo, remote) = fixture(root);
                settings(&repo, preference);
                let mut terminal = cli.terminal(repo.path(), &[]);
                assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                assert!(
                    !terminal
                        .output
                        .contains("How would you like to save this work?")
                );
                assert!(!terminal.output.contains("Staged commit message"));
                published(&repo, &remote);
            }
            for (preference, args) in [
                ("ask", vec!["--staged", "--message", "literal"]),
                ("wip", vec![]),
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                settings(&repo, preference);
                let mut terminal = cli.terminal(repo.path(), &args);
                assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                assert!(
                    !terminal
                        .output
                        .contains("How would you like to save this work?")
                );
                assert!(!terminal.output.contains("Staged commit message"));
                published(&repo, &remote);
            }
        },
    );
}
#[cfg(target_os = "linux")]
#[test]
fn save_terminal_cancel_eof_blank_invalid_and_interrupt_leave_no_effects() {
    cases(
        "save_terminal_cancel_eof_blank_invalid_and_interrupt_leave_no_effects",
        true,
        |root, cli| {
            for input in [
                b"cancel\n".as_slice(),
                b"5\n",
                b"0\n",
                b"99\n",
                b"\x04",
                b"wip\x04\x04",
                b"invalid\n",
                b"\n",
                b"interrupt",
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let before = snapshot(repo.path());
                let remote_before = snapshot(&remote);
                let mut terminal = cli.terminal(repo.path(), &[]);
                terminal.until("How would you like to save this work?");
                if input == b"interrupt" {
                    terminal.interrupt();
                } else {
                    terminal.send(input);
                    if [b"0\n".as_slice(), b"99\n", b"invalid\n", b"\n"].contains(&input) {
                        terminal.until("Choose a listed name or number");
                        terminal.send(b"cancel\n");
                    }
                }
                assert!(!terminal.finish().success(), "{}", terminal.output);
                assert!(!terminal.output.contains("Save complete"));
                assert_eq!(snapshot(repo.path()), before);
                assert_eq!(snapshot(&remote), remote_before);
            }
            for input in [b"\x04".as_slice(), b"message\x04\x04", b" \t\n"] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, false);
                let before = snapshot(repo.path());
                let mut terminal = cli.terminal(repo.path(), &[]);
                terminal.until("How would you like to save this work?");
                terminal.send(b"staged\n");
                terminal.until("Staged commit message");
                terminal.send(input);
                if input == b" \t\n" {
                    terminal.until("Enter a nonblank staged commit message");
                    terminal.send(b"\x04");
                }
                assert_eq!(terminal.finish().code(), Some(1), "{}", terminal.output);
                assert!(!terminal.output.contains("Save complete"));
                assert_eq!(snapshot(repo.path()), before);
                assert!(oid(&remote, "refs/heads/work").is_none());
            }
        },
    );
}
#[cfg(target_os = "linux")]
#[test]
fn save_terminal_revalidates_context_after_both_input_waits() {
    cases(
        "save_terminal_revalidates_context_after_both_input_waits",
        true,
        |root, cli| {
            for mutation in [
                "branch",
                "settings",
                "upstream",
                "index",
                "contents",
                "new-file",
                "message-wait",
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let mut terminal = cli.terminal(repo.path(), &[]);
                terminal.until("How would you like to save this work?");
                if mutation == "message-wait" {
                    terminal.send(b"staged\n");
                    terminal.until("Staged commit message");
                }
                match mutation {
                    "branch" => {
                        repo.success(&["switch", "-c", "other"]);
                    }
                    "settings" => settings(&repo, "wip"),
                    "upstream" => {
                        repo.success(&["config", "branch.work.remote", "wrong"]);
                    }
                    "index" => {
                        repo.success(&["add", "tracked"]);
                    }
                    "contents" | "message-wait" => {
                        fs::write(repo.path().join("tracked"), "changed during input\n").unwrap()
                    }
                    "new-file" => {
                        fs::write(repo.path().join("new-file"), "changed scope\n").unwrap()
                    }
                    _ => unreachable!(),
                }
                let before = snapshot(repo.path());
                let remote_before = snapshot(&remote);
                terminal.send(if mutation == "message-wait" {
                    b"message\n"
                } else {
                    b"wip\n"
                });
                assert_eq!(terminal.finish().code(), Some(1), "{}", terminal.output);
                assert!(!terminal.output.contains("Save complete"));
                assert_eq!(snapshot(repo.path()), before);
                assert_eq!(snapshot(&remote), remote_before);
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
            for action in ["--wip", "--stash"] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                local_hook(
                    &repo,
                    "pre-push",
                    "printf 'blocked\\033[31m\\n' >&2\nexit 1",
                );
                let output = check(
                    cli.run(repo.path(), &[action]),
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
                    &[if action == "--wip" {
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
                cli.run(repo.path(), &["--wip"]),
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
                cli.run(repo.path(), &["--wip"]),
                1,
                &[
                    "Save incomplete",
                    "publication not attempted",
                    "Local commit process",
                    "Work still local: 2 staged",
                    "Failed/interrupted Git steps can leave additional effects",
                ],
            );
            assert_eq!(repo.success(&["show", ":hook-file"]).stdout, b"hook work\n");
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

#[test]
fn save_invalid_grammar_preserves_repository_and_remote() {
    cases(
        "save_invalid_grammar_preserves_repository_and_remote",
        true,
        |root, cli| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            let before = snapshot(repo.path());
            let remote_before = snapshot(&remote);
            for args in [
                vec!["--publish-only", "--message", "incompatible"],
                vec!["--wip", "--message", "incompatible"],
                vec!["--stash", "--message", "incompatible"],
                vec!["--staged", "--message"],
                vec!["--staged", "--message", "message", "extra"],
                vec!["--message", "first", "--message", "second", "--staged"],
            ] {
                check(cli.run(repo.path(), &args), 2, &["use save --help"]);
                assert_eq!(snapshot(repo.path()), before);
                assert_eq!(snapshot(&remote), remote_before);
            }
            for first in ["--publish-only", "--staged", "--wip", "--stash"] {
                for second in ["--publish-only", "--staged", "--wip", "--stash"] {
                    check(
                        cli.run(repo.path(), &[first, second]),
                        2,
                        &["mutually exclusive"],
                    );
                    assert_eq!(snapshot(repo.path()), before);
                    assert_eq!(snapshot(&remote), remote_before);
                }
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
                dirty(&repo, true, false);
                check(
                    cli.run(repo.path(), &["--message", message, "--staged"]),
                    0,
                    &["Save complete", message.trim()],
                );
                published(&repo, &remote);
                // Git's ordinary cleanup/hooks remain enabled, but no CLI interpretation occurs.
                assert_eq!(
                    repo.success(&["log", "-1", "--format=%s"]).stdout,
                    format!("{}\n", message.trim_end()).as_bytes()
                );
            }
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
                cli.run(repo.path(), &["--wip"]),
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
                cli.run(repo.path(), &["--staged", "--message", "interrupted"]),
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
                cli.run(repo.path(), &["--wip"]),
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

#[cfg(target_os = "linux")]
#[test]
fn save_capable_navigation_corrects_message_and_reports_delivery_failure_and_cancel() {
    isolated(
        "save_capable_navigation_corrects_message_and_reports_delivery_failure_and_cancel",
        true,
        |root| {
            let exe = env!("CARGO_BIN_EXE_git-oneflow");
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            let mut terminal = terminal::Terminal::start_capable(exe, repo.path(), &["save"]);
            terminal.until("How would you like to save this work?");
            // The initial cursor is cancel. Move to staged using real navigation.
            terminal.send(b"\x1b[A\x1b[A\x1b[A\r");
            terminal.until("Staged commit message");
            terminal.send(b"\r");
            terminal.until("Enter a nonblank staged commit message");
            terminal.send("修正 café λ\r".as_bytes());
            assert!(terminal.finish().success(), "{}", terminal.output);
            terminal.assert_restored();
            published(&repo, &remote);
            assert_eq!(
                repo.success(&["log", "-1", "--format=%s"]).stdout,
                "修正 café λ\n".as_bytes()
            );
            assert_eq!(
                fs::read(repo.path().join("tracked")).unwrap(),
                b"remaining\n"
            );
            assert!(repo.path().join("untracked").exists());

            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            let before = snapshot(repo.path());
            let remote_before = snapshot(&remote);
            let mut terminal = terminal::Terminal::start_capable(exe, repo.path(), &["save"]);
            terminal.until("How would you like to save this work?");
            terminal.send(b"\x1b[A\x1b[A\x1b[A\r");
            terminal.until("Staged commit message");
            terminal.send(b"partial message\x04");
            assert_eq!(terminal.finish().code(), Some(1), "{}", terminal.output);
            terminal.assert_restored();
            assert_eq!(snapshot(repo.path()), before);
            assert_eq!(snapshot(&remote), remote_before);

            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            let old_head = oid(repo.path(), "HEAD");
            hook(
                &remote.join("hooks"),
                "pre-receive",
                "echo capable-push-rejected >&2\nexit 1",
            );
            let mut terminal = terminal::Terminal::start_capable(exe, repo.path(), &["save"]);
            terminal.until("How would you like to save this work?");
            terminal.send(b"\x1b[A\x1b[A\r");
            assert_eq!(terminal.finish().code(), Some(1), "{}", terminal.output);
            terminal.assert_restored();
            contains(
                &terminal.output,
                &["capable-push-rejected", "Save incomplete"],
            );
            assert_ne!(oid(repo.path(), "HEAD"), old_head);
            assert!(oid(&remote, "refs/heads/work").is_none());
            assert_eq!(
                fs::read(repo.path().join("tracked")).unwrap(),
                b"remaining\n"
            );
            assert_eq!(fs::read(repo.path().join("untracked")).unwrap(), b"new\n");
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn confirmed_save_uses_success_and_warning_colours_without_turning_cache_caution_into_failure() {
    isolated(
        "confirmed_save_uses_success_and_warning_colours_without_turning_cache_caution_into_failure",
        true,
        |root| {
            let executable = env!("CARGO_BIN_EXE_git-oneflow");
            for preference in [
                None,
                Some(("NO_COLOR", "1")),
                Some(("CLICOLOR", "0")),
                Some(("TERM", "dumb")),
            ] {
                let (repo, remote) = fixture(root);
                let base = oid(repo.path(), "main").unwrap();
                local_hook(
                    &repo,
                    "pre-push",
                    "git update-ref refs/remotes/origin/work main",
                );
                let mut command = Command::new(executable);
                command
                    .current_dir(repo.path())
                    .args(["save", "--publish-only"])
                    .env("TERM", "xterm-256color");
                if let Some((name, value)) = preference {
                    command.env(name, value);
                }
                let mut tty = terminal::Terminal::start_command(command);
                assert_eq!(tty.finish().code(), Some(0), "{}", tty.output);
                tty.assert_restored();
                let warning = "Local tracking cache needs attention";
                if preference.is_none() {
                    assert!(
                        tty.output.contains("\x1b[32mSave complete"),
                        "success is not green: {}",
                        tty.output
                    );
                    assert!(
                        tty.output.contains(&format!("\x1b[33m{warning}")),
                        "warning is not yellow: {}",
                        tty.output
                    );
                } else {
                    assert!(!tty.output.contains('\x1b'), "{}", tty.output);
                }
                contains(
                    &tty.output,
                    &[
                        "Save complete",
                        warning,
                        "cache changed concurrently",
                        "Confirmed remote effects are retained",
                    ],
                );
                published(&repo, &remote);
                assert_eq!(oid(repo.path(), "refs/remotes/origin/work").unwrap(), base);
            }
            // The same nonfatal caution retains its meaning and stdout stream in logs.
            let (repo, remote) = fixture(root);
            local_hook(
                &repo,
                "pre-push",
                "git update-ref refs/remotes/origin/work main",
            );
            let output = Command::new(executable)
                .current_dir(repo.path())
                .args(["save", "--publish-only"])
                .env("TERM", "xterm-256color")
                .env("CLICOLOR_FORCE", "1")
                .output()
                .unwrap();
            let text = check(
                output,
                0,
                &[
                    "Save complete",
                    "Local tracking cache needs attention",
                    "cache changed concurrently",
                ],
            );
            assert!(!text.contains('\x1b'));
            published(&repo, &remote);
        },
    );
}
