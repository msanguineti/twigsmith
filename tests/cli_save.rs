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
                    &["effective save action is staged"],
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
            for action in ["publish-only", "staged", "wip", "stash"] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let mut terminal = cli.terminal(repo.path(), &[]);
                terminal.until("Action (");
                contains(
                    &terminal.output,
                    &[
                        "1 staged, 1 unstaged tracked, 1 untracked",
                        "staged       Commit/publish exactly the index",
                        "publish-only Publish existing commits",
                        "stash        Retain a local stash",
                        "cancel",
                    ],
                );
                terminal.send(format!("{action}\n").as_bytes());
                if action == "staged" {
                    terminal.until("Staged commit message");
                    terminal.send(b"literal --wip ;$ message\n");
                }
                assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
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
                assert!(!terminal.output.contains("Action ("));
                terminal.send(b"configured or explicit message\n");
                assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                published(&repo, &remote);
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
            terminal.until("Action (");
            assert!(!terminal.output.contains("staged       Commit/publish"));
            terminal.send(b"staged\n");
            assert_eq!(terminal.finish().code(), Some(1));
            contains(&terminal.output, &["invalid save choice"]);
            for preference in ["ask", "wip", "stash", "staged"] {
                let (repo, remote) = fixture(root);
                settings(&repo, preference);
                let mut terminal = cli.terminal(repo.path(), &[]);
                assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
                assert!(!terminal.output.contains("Action ("));
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
                assert!(!terminal.output.contains("Action ("));
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
                terminal.until("Action (");
                if input == b"interrupt" {
                    terminal.interrupt();
                } else {
                    terminal.send(input);
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
                terminal.until("Action (");
                terminal.send(b"staged\n");
                terminal.until("Staged commit message");
                terminal.send(input);
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
                terminal.until("Action (");
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
