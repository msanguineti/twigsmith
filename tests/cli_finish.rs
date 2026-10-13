#[path = "support/snapshot.rs"]
mod preservation;
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
mod terminal;
use git_oneflow::git;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};

const BRANCH: &str = "fix/deployment-bug";
const REF: &str = "refs/heads/fix/deployment-bug";
// Workflow scenarios exercise the common dispatcher once. Grammar/help retains
// explicit coverage of both executable entry points and the finish alias.
fn cases(name: &str, available: bool, test: impl Fn(&Path, &str)) {
    isolated(name, available, |root| {
        let dir = tempfile::tempdir_in(root).unwrap();
        test(dir.path(), env!("CARGO_BIN_EXE_git-oneflow"));
    });
}
fn entrypoint_cases(name: &str, available: bool, test: impl Fn(&Path, &str)) {
    isolated(name, available, |root| {
        for exe in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
            let dir = tempfile::tempdir_in(root).unwrap();
            println!("finish grammar: executable={exe}");
            test(dir.path(), exe);
        }
    });
}
fn run(exe: &str, directory: &Path, args: &[&str]) -> Output {
    run_command(exe, directory, "finish", args)
}
fn run_command(exe: &str, directory: &Path, command: &str, args: &[&str]) -> Output {
    Command::new(exe)
        .current_dir(directory)
        .arg(command)
        .args(args)
        .args(if matches!(args, ["--help" | "-h"]) {
            vec![]
        } else {
            vec!["--mode", "review"]
        })
        .stdin(Stdio::null())
        .output()
        .unwrap()
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
    let result = String::from_utf8(bytes).unwrap();
    contains(&result, expected);
    result
}
fn contains(text: &str, expected: &[&str]) {
    for expected in expected {
        assert!(text.contains(expected), "missing {expected:?}: {text}");
    }
}
fn oid(repo: &Repository, reference: &str) -> Option<String> {
    let output = git::run(repo.path(), ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().into())
}
fn commit(repo: &Repository, path: &str, content: &str) {
    fs::write(repo.path().join(path), content).unwrap();
    repo.success(&["add", path]);
    repo.success(&["commit", "-m", path]);
}
fn fixture(root: &Path, published: bool, advanced: bool) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base", "initial\n");
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin"]);
    local.success(&["switch", "--no-track", "-c", BRANCH, "origin/main"]);
    commit(&local, "work", "feature\n");
    if published {
        seed(&local, &remote);
    }
    if advanced {
        commit(&remote, "upstream", "new base\n");
    }
    (local, remote)
}
fn seed(local: &Repository, remote: &Repository) {
    remote.success(&[
        "fetch",
        "--no-tags",
        local.path().to_str().unwrap(),
        &format!("HEAD:{REF}"),
    ]);
}
fn settings(repo: &Repository, update: &str, rewrite: &str) {
    fs::write(
        repo.path().join(".git/git-oneflow.config.toml"),
        format!("finish-update='{update}'\nfinish-rewrite='{rewrite}'\n"),
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn finish_credential_redaction_defaults_local_override_and_flag_precedence() {
    use std::os::unix::fs::PermissionsExt;
    cases(
        "finish_credential_redaction_defaults_local_override_and_flag_precedence",
        true,
        |root, exe| {
            let (local, remote) = fixture(root, false, false);
            let hooks = remote.path().join(".git/hooks");
            fs::create_dir_all(&hooks).unwrap();
            remote.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
            let hook = hooks.join("pre-receive");
            fs::write(
                &hook,
                "#!/bin/sh\necho 'fatal: rejected https://synthetic-user:synthetic-secret@example.invalid/repository' >&2\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

            let default = run(exe, local.path(), &[]);
            assert_eq!(default.status.code(), Some(1), "{default:?}");
            assert!(!String::from_utf8_lossy(&default.stdout).contains("synthetic-secret"));
            assert!(!String::from_utf8_lossy(&default.stderr).contains("synthetic-secret"));
            assert!(
                String::from_utf8_lossy(&default.stderr)
                    .contains("<redacted>@example.invalid/repository")
            );

            fs::write(
                local.path().join(".git/git-oneflow.config.toml"),
                "redact-credentials=false\n",
            )
            .unwrap();
            let local_override = run(exe, local.path(), &[]);
            assert_eq!(local_override.status.code(), Some(1), "{local_override:?}");
            assert!(
                String::from_utf8_lossy(&local_override.stderr).contains("synthetic-secret"),
                "{}",
                String::from_utf8_lossy(&local_override.stderr)
            );

            let flag = run(exe, local.path(), &["--show-credentials"]);
            assert_eq!(flag.status.code(), Some(1), "{flag:?}");
            assert!(String::from_utf8_lossy(&flag.stderr).contains("synthetic-secret"));
        },
    );
}
fn published(local: &Repository, remote: &Repository) {
    assert_eq!(oid(local, "HEAD"), oid(remote, REF));
    assert_eq!(
        local
            .success(&["config", &format!("branch.{BRANCH}.remote")])
            .stdout,
        b"origin\n"
    );
    assert_eq!(
        local
            .success(&["config", &format!("branch.{BRANCH}.merge")])
            .stdout,
        format!("{REF}\n").as_bytes()
    );
}
fn unchanged_work(
    repo: &Repository,
    before: &std::collections::BTreeMap<std::path::PathBuf, preservation::FileState>,
) {
    assert_eq!(&work_snapshot(repo), before);
}
fn work_snapshot(
    repo: &Repository,
) -> std::collections::BTreeMap<std::path::PathBuf, preservation::FileState> {
    preservation::snapshot(repo.path())
        .into_iter()
        .filter(|(path, _)| {
            !path.starts_with(".git")
                || path == Path::new(".git/index")
                || path == Path::new(".git/HEAD")
                || path.starts_with(".git/refs/heads")
        })
        .collect()
}

#[test]
fn finish_grammar_and_help_without_git() {
    entrypoint_cases("finish_grammar_and_help_without_git", false, |root, exe| {
        for command in ["finish", "f"] {
            for help in ["--help", "-h"] {
                let output = check(
                    run_command(exe, root, command, &[help]),
                    0,
                    &[
                        "Usage:",
                        "Options:",
                        "Examples:",
                        "--update",
                        "--history-edit",
                        "--allow-rewrite",
                        "--no-rewrite",
                        "Exit status:",
                        "manual",
                        "project-base tracking",
                        "Ordinary Git commands",
                    ],
                );
                assert!(output.contains("Alias: f."));
                assert_eq!(output, check(run(exe, root, &[help]), 0, &[]));
            }
            for args in [
                vec![],
                vec!["--allow-rewrite"],
                vec!["--no-rewrite"],
                vec!["--update", "rebase"],
                vec!["--history-edit", "interactive"],
                vec!["--history-edit", "none"],
                vec!["--update", "merge", "--allow-rewrite"],
                vec!["--no-rewrite", "--update", "none"],
            ] {
                check(run_command(exe, root, command, &args), 1, &["Git"]);
            }
            for args in [
                vec!["--update"],
                vec!["--history-edit"],
                vec!["--history-edit", "ask"],
                vec!["--history-edit", "INTERACTIVE"],
                vec!["--history-edit", "none", "--history-edit", "interactive"],
                vec!["--update", "ask"],
                vec!["--update", "REBASE"],
                vec!["--update", "squash"],
                vec!["--update=merge"],
                vec!["--update", "merge", "--update", "none"],
                vec!["--allow-rewrite", "--no-rewrite"],
                vec!["--no-rewrite", "--allow-rewrite"],
                vec!["--allow-rewrite", "--allow-rewrite"],
                vec!["--no-rewrite", "--no-rewrite"],
                vec!["branch"],
                vec!["--yes"],
                vec!["--dry-run"],
                vec!["--help", "--update", "none"],
                vec!["--"],
            ] {
                check(
                    run_command(exe, root, command, &args),
                    2,
                    &["finish --help"],
                );
            }
        }
    });
}

#[test]
fn finish_unattended_publication_updates_and_precedence() {
    cases(
        "finish_unattended_publication_updates_and_precedence",
        true,
        |root, exe| {
            for update in ["rebase", "merge", "none"] {
                for flags in [false, true] {
                    let (local, remote) = fixture(root, false, true);
                    let before = oid(&local, "HEAD").unwrap();
                    let base = oid(&remote, "main").unwrap();
                    // Opposite configuration demonstrates explicit flag precedence.
                    settings(&local, if flags { "none" } else { update }, "ask");
                    let args = if flags {
                        vec!["--update", update, "--no-rewrite"]
                    } else {
                        vec![]
                    };
                    let output = check(
                        run(exe, local.path(), &args),
                        0,
                        &[
                            "Published fix/deployment-bug to origin at",
                            "created",
                            "Next: submit origin/fix/deployment-bug for review into origin/main.",
                        ],
                    );
                    contains(
                        &output,
                        &[
                            &oid(&local, "HEAD").unwrap(),
                            &remote.path().display().to_string(),
                        ],
                    );
                    assert!(!output.contains("checks passed"));
                    published(&local, &remote);
                    assert_eq!(oid(&remote, "main"), Some(base.clone()));
                    let includes =
                        git::run(local.path(), ["merge-base", "--is-ancestor", &base, "HEAD"])
                            .unwrap()
                            .status
                            .success();
                    assert_eq!(includes, update != "none");
                    if update == "none" {
                        assert_eq!(oid(&local, "HEAD"), Some(before));
                    }
                    if update == "merge" {
                        assert_eq!(
                            local
                                .success(&["rev-list", "--parents", "-1", "HEAD"])
                                .stdout
                                .split(|b| *b == b' ')
                                .count(),
                            3
                        );
                    }
                }
            }
            // All ask defaults are irrelevant when history already includes the base.
            let (local, remote) = fixture(root, false, false);
            check(run(exe, local.path(), &[]), 0, &["created"]);
            published(&local, &remote);
            check(
                run(exe, local.path(), &[]),
                0,
                &["unchanged (already published)"],
            );
        },
    );
}

#[test]
fn finish_unattended_unresolved_choices_and_rewrite_precedence() {
    cases(
        "finish_unattended_unresolved_choices_and_rewrite_precedence",
        true,
        |root, exe| {
            let (local, remote) = fixture(root, false, true);
            let before = work_snapshot(&local);
            check(
                run(exe, local.path(), &[]),
                1,
                &[
                    "--update <rebase|merge|none>",
                    "finish-update",
                    "Fetched objects",
                    "Branch publication not attempted",
                ],
            );
            unchanged_work(&local, &before);
            assert_eq!(oid(&remote, REF), None);
            // Rebase choice is independent of replacement authorization.
            let (local, remote) = fixture(root, true, true);
            let before = work_snapshot(&local);
            check(
                run(exe, local.path(), &["--update", "rebase"]),
                1,
                &[
                    "--allow-rewrite",
                    "--no-rewrite",
                    "finish-rewrite",
                    "unresolved rewrite",
                ],
            );
            unchanged_work(&local, &before);
            settings(&local, "rebase", "allow");
            check(
                run(exe, local.path(), &["--no-rewrite"]),
                1,
                &["replacement denied"],
            );
            unchanged_work(&local, &before);
            settings(&local, "none", "deny");
            check(
                run(
                    exe,
                    local.path(),
                    &["--update", "rebase", "--allow-rewrite"],
                ),
                0,
                &["replaced"],
            );
            published(&local, &remote);
            let (local, remote) = fixture(root, true, true);
            settings(&local, "rebase", "allow");
            check(run(exe, local.path(), &[]), 0, &["replaced"]);
            published(&local, &remote);
            // Deny and ask are irrelevant for a history-preserving merge.
            let (local, remote) = fixture(root, true, true);
            settings(&local, "rebase", "ask");
            check(
                run(exe, local.path(), &["--update", "merge", "--no-rewrite"]),
                0,
                &["advanced"],
            );
            published(&local, &remote);
        },
    );
}

#[test]
fn finish_readiness_conflicts_and_retry_after_failed_push() {
    cases(
        "finish_readiness_conflicts_and_retry_after_failed_push",
        true,
        |root, exe| {
            let (local, remote) = fixture(root, false, true);
            fs::write(local.path().join("work"), "dirty\n").unwrap();
            let before = preservation::snapshot(root);
            check(
                run(exe, local.path(), &["--update", "none"]),
                1,
                &["clean index", "contains changes"],
            );
            assert_eq!(preservation::snapshot(root), before);
            assert_eq!(oid(&remote, REF), None);
            for update in ["rebase", "merge"] {
                let (local, remote) = fixture(root, true, false);
                commit(&local, "base", "local change\n");
                commit(&remote, "base", "remote change\n");
                let published_before = oid(&remote, REF);
                check(
                    run(exe, local.path(), &["--update", update, "--allow-rewrite"]),
                    1,
                    &[
                        "Finish incomplete",
                        "Git process",
                        "Git operations: active",
                        &format!("git {update} --continue"),
                        &format!("git {update} --abort"),
                        "rerun finish",
                        "Branch publication not attempted",
                    ],
                );
                assert_eq!(oid(&remote, REF), published_before);
                if update == "rebase" {
                    // Continue through Git; the next CLI run verifies durable recovery.
                    fs::write(local.path().join("base"), "resolved\n").unwrap();
                    local.success(&["add", "base"]);
                    let result = Command::new("git")
                        .current_dir(local.path())
                        .args(["rebase", "--continue"])
                        .env("GIT_EDITOR", "true")
                        .output()
                        .unwrap();
                    assert!(result.status.success(), "{result:?}");
                    check(
                        run(exe, local.path(), &["--allow-rewrite"]),
                        0,
                        &["replaced"],
                    );
                    published(&local, &remote);
                }
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let (local, remote) = fixture(root, true, true);
                let hooks = remote.path().join(".git/reject-hooks");
                fs::create_dir_all(&hooks).unwrap();
                let hook = hooks.join("pre-receive");
                fs::write(
                    &hook,
                    "#!/bin/sh\necho 'fixture rejects publication' >&2\nexit 1\n",
                )
                .unwrap();
                fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
                remote.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
                let original = oid(&remote, REF);
                settings(&local, "rebase", "allow");
                check(
                    run(exe, local.path(), &[]),
                    1,
                    &[
                        "Prepared with rebase",
                        "rejected",
                        "fixture rejects publication",
                        "Remaining state",
                        "Completed effects are retained",
                    ],
                );
                assert_eq!(oid(&remote, REF), original);
                let prepared = oid(&local, "HEAD");
                assert_ne!(prepared, original);
                fs::remove_file(&hook).unwrap();
                check(run(exe, local.path(), &[]), 0, &["replaced"]);
                assert_eq!(oid(&local, "HEAD"), prepared);
                published(&local, &remote);
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn finish_terminal_choices_and_separate_authorization() {
    cases(
        "finish_terminal_choices_and_separate_authorization",
        true,
        |root, exe| {
            for command in ["finish", "f"] {
                for update in ["rebase", "merge", "none"] {
                    let (local, remote) = fixture(root, true, true);
                    let before = work_snapshot(&local);
                    let mut term = terminal::Terminal::start(
                        exe,
                        local.path(),
                        &[command, "--mode", "review"],
                    );
                    term.until("Update (rebase/merge/none/cancel)");
                    contains(
                        &term.output,
                        &["replays branch", "incorporates", "without updating"],
                    );
                    let answer = if command == "f" {
                        match update {
                            "rebase" => "1",
                            "merge" => "2",
                            "none" => "3",
                            _ => unreachable!(),
                        }
                    } else {
                        update
                    };
                    term.send(format!("{answer}\n").as_bytes());
                    if update == "rebase" {
                        term.until("Replace published history (allow/deny/cancel)");
                        unchanged_work(&local, &before);
                        assert_eq!(oid(&remote, REF), oid(&local, "HEAD"));
                        term.send(if command == "f" { b"1\n" } else { b"allow\n" });
                    }
                    assert_eq!(term.finish().code(), Some(0), "{}", term.output);
                    published(&local, &remote);
                    if update != "rebase" {
                        assert!(!term.output.contains("Replace published history ("));
                    }
                    contains(&term.output, &["Published", "Next:"]);
                }
            }
            let (local, _remote) = fixture(root, true, true);
            let before = work_snapshot(&local);
            let mut term = terminal::Terminal::start(
                exe,
                local.path(),
                &["finish", "--mode", "review", "--update", "rebase"],
            );
            term.until("Replace published history (");
            term.send(b"deny\n");
            assert_eq!(term.finish().code(), Some(1));
            contains(&term.output, &["replacement denied"]);
            unchanged_work(&local, &before);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn finish_terminal_cancel_eof_and_invalid_input_preserve_branch_work() {
    cases(
        "finish_terminal_cancel_eof_and_invalid_input_preserve_branch_work",
        true,
        |root, exe| {
            let (local, _remote) = fixture(root, true, true);
            let before = work_snapshot(&local);
            let mut term =
                terminal::Terminal::start(exe, local.path(), &["finish", "--mode", "review"]);
            term.until("Update (");
            term.interrupt();
            assert!(!term.finish().success());
            unchanged_work(&local, &before);
            for rewrite in [false, true] {
                for answer in [
                    b"cancel\n".as_slice(),
                    b"numeric-cancel",
                    b"0\n",
                    b"99\n",
                    b"\x04",
                    b"allow\x04\x04",
                    b"invalid\n",
                ] {
                    let (local, remote) = fixture(root, true, true);
                    let before = work_snapshot(&local);
                    let args = if rewrite {
                        vec!["finish", "--mode", "review", "--update", "rebase"]
                    } else {
                        vec!["finish", "--mode", "review"]
                    };
                    let mut term = terminal::Terminal::start(exe, local.path(), &args);
                    term.until(if rewrite {
                        "Replace published history ("
                    } else {
                        "Update ("
                    });
                    term.send(if answer == b"numeric-cancel" {
                        if rewrite { b"3\n" } else { b"4\n" }
                    } else {
                        answer
                    });
                    if [b"0\n".as_slice(), b"99\n", b"invalid\n"].contains(&answer) {
                        term.until("Choose a listed name or number");
                        term.send(b"cancel\n");
                    }
                    assert_eq!(term.finish().code(), Some(1), "{}", term.output);
                    contains(
                        &term.output,
                        &[
                            "Finish incomplete",
                            "Fetched objects",
                            "Branch publication not attempted",
                        ],
                    );
                    unchanged_work(&local, &before);
                    assert_eq!(oid(&remote, REF), oid(&local, "HEAD"));
                }
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn finish_terminal_revalidates_local_and_remote_changes_during_input() {
    cases(
        "finish_terminal_revalidates_local_and_remote_changes_during_input",
        true,
        |root, exe| {
            for rewrite in [false, true] {
                for mutation in [
                    "head",
                    "settings",
                    "git-config",
                    "index",
                    "tree",
                    "remote",
                    "base",
                ] {
                    let (local, remote) = fixture(root, true, true);
                    let args = if rewrite {
                        vec!["finish", "--mode", "review", "--update", "rebase"]
                    } else {
                        vec!["finish", "--mode", "review"]
                    };
                    let mut term = terminal::Terminal::start(exe, local.path(), &args);
                    term.until(if rewrite {
                        "Replace published history ("
                    } else {
                        "Update ("
                    });
                    match mutation {
                        "head" => {
                            commit(&local, "another", "external commit\n");
                        }
                        "settings" => settings(&local, "merge", "allow"),
                        "git-config" => {
                            local.success(&["config", "arbitrary.value", "changed"]);
                        }
                        "index" => {
                            fs::write(local.path().join("work"), "changed\n").unwrap();
                            local.success(&["add", "work"]);
                        }
                        "tree" => {
                            fs::write(local.path().join("work"), "changed\n").unwrap();
                        }
                        "remote" => {
                            remote.success(&["update-ref", REF, "main"]);
                        }
                        "base" => {
                            commit(&remote, "more-base", "advanced again\n");
                        }
                        _ => unreachable!(),
                    }
                    let before = work_snapshot(&local);
                    let remote_head = oid(&remote, REF);
                    term.send(if rewrite { b"allow\n" } else { b"rebase\n" });
                    // Remote-only changes are detected by preparation's repeated refresh,
                    // after both choices have been resolved.
                    if !rewrite && ["remote", "base"].contains(&mutation) {
                        term.until("Replace published history (");
                        term.send(b"allow\n");
                    }
                    assert_eq!(term.finish().code(), Some(1), "{}", term.output);
                    unchanged_work(&local, &before);
                    assert_eq!(oid(&remote, REF), remote_head);
                    contains(
                        &term.output,
                        &["Branch publication not attempted", "Remaining state"],
                    );
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn finish_reports_partial_uncertain_and_tracking_failure_then_retries() {
    use std::os::unix::fs::PermissionsExt;
    fn hook(repo: &Repository, name: &str, body: &str) -> std::path::PathBuf {
        let hooks = repo.path().join(".git/cli-hooks");
        fs::create_dir_all(&hooks).unwrap();
        repo.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
        let path = hooks.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
    cases(
        "finish_reports_partial_uncertain_and_tracking_failure_then_retries",
        true,
        |root, exe| {
            let (local, first) = fixture(root, false, false);
            let second = Repository::new(root);
            for destination in [&first, &second] {
                local.success(&[
                    "config",
                    "--add",
                    "remote.origin.pushurl",
                    destination.path().to_str().unwrap(),
                ]);
            }
            let reject = hook(
                &second,
                "pre-receive",
                "echo 'second destination rejected' >&2\nexit 1",
            );
            let output = check(
                run(exe, local.path(), &[]),
                1,
                &[
                    "created",
                    "rejected",
                    "second destination rejected",
                    "Finish incomplete",
                    "Observed upstream tracking: absent",
                ],
            );
            contains(
                &output,
                &[
                    &first.path().display().to_string(),
                    &second.path().display().to_string(),
                ],
            );
            assert!(!output.contains("Next:"));
            assert_eq!(oid(&first, REF), oid(&local, "HEAD"));
            assert_eq!(oid(&second, REF), None);
            fs::remove_file(reject).unwrap();
            check(
                run(exe, local.path(), &[]),
                0,
                &["unchanged (already published)", "created"],
            );
            published(&local, &first);
            published(&local, &second);

            let (local, remote) = fixture(root, false, false);
            let fail = hook(
                &local,
                "pre-push",
                "echo 'local push hook failed' >&2\nexit 1",
            );
            check(
                run(exe, local.path(), &[]),
                1,
                &[
                    "unconfirmed; remote effects may exist",
                    "local push hook failed",
                    "Remaining state",
                ],
            );
            assert_eq!(oid(&remote, REF), None);
            fs::remove_file(fail).unwrap();
            check(run(exe, local.path(), &[]), 0, &["created"]);
            published(&local, &remote);

            let (local, remote) = fixture(root, false, false);
            let lock = hook(
                &local,
                "pre-push",
                "oneflow_lock_path=$(git rev-parse --git-path config.lock)\n: > \"$oneflow_lock_path\"",
            );
            check(
                run(exe, local.path(), &[]),
                1,
                &[
                    "publication confirmed, but tracking setup failed",
                    "created",
                    "Observed upstream tracking: absent",
                ],
            );
            assert_eq!(oid(&remote, REF), oid(&local, "HEAD"));
            fs::remove_file(local.path().join(".git/config.lock")).unwrap();
            fs::remove_file(lock).unwrap();
            check(
                run(exe, local.path(), &[]),
                0,
                &["unchanged (already published)"],
            );
            published(&local, &remote);
        },
    );
}

#[test]
fn finish_shorthand_publishes_with_identical_options() {
    cases(
        "finish_shorthand_publishes_with_identical_options",
        true,
        |root, exe| {
            for update in ["rebase", "merge", "none"] {
                let (local, remote) = fixture(root, true, true);
                let before = oid(&local, "HEAD");
                check(
                    run_command(
                        exe,
                        local.path(),
                        "f",
                        &["--update", update, "--allow-rewrite"],
                    ),
                    0,
                    &["Published", "Next:"],
                );
                published(&local, &remote);
                if update == "none" {
                    assert_eq!(oid(&local, "HEAD"), before);
                }
                check(
                    run_command(
                        exe,
                        local.path(),
                        "f",
                        &["--update", "none", "--no-rewrite"],
                    ),
                    0,
                    &["unchanged (already published)"],
                );
            }
        },
    );
}

// Completion tests invoke the public grammar without the review helper's override.
fn completion_run(exe: &str, directory: &Path, command: &str, args: &[&str]) -> Output {
    Command::new(exe)
        .current_dir(directory)
        .arg(command)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn completion_missing_mode_and_incompatible_choices_preserve_work() {
    cases(
        "completion_missing_mode_and_incompatible_choices_preserve_work",
        true,
        |root, exe| {
            for command in ["finish", "f"] {
                let (local, remote) = fixture(root, false, true);
                let before = preservation::snapshot(root);
                check(
                    completion_run(exe, local.path(), command, &[]),
                    1,
                    &["unresolved completion mode"],
                );
                assert_eq!(preservation::snapshot(root), before);
                let result = completion_run(
                    exe,
                    local.path(),
                    command,
                    &[
                        "--mode",
                        "integrate",
                        "--integration",
                        "merge",
                        "-m",
                        "Final integration",
                        "--checkout",
                        "work",
                        "--cleanup",
                        "both",
                    ],
                );
                assert_eq!(result.status.code(), Some(2), "{result:?}");
                assert_eq!(preservation::snapshot(root), before);
                assert_eq!(oid(&remote, REF), None);
            }
        },
    );
}

#[test]
fn completion_mode_layers_flag_precedence_review_conflicts_and_inactive_validation() {
    cases(
        "completion_mode_layers_flag_precedence_review_conflicts_and_inactive_validation",
        true,
        |root, exe| {
            for command in ["finish", "f"] {
                let (local, remote) = fixture(root, false, false);
                fs::write(
                    local.path().join("git-oneflow.config.toml"),
                    "finish-mode='integrate'\n",
                )
                .unwrap();
                local.success(&["add", "git-oneflow.config.toml"]);
                local.success(&["commit", "-m", "shared completion"]);
                let private = local.path().join(".git/git-oneflow.config.json");
                fs::write(&private, r#"{"finish-mode":"review","finish-integration":"squash","finish-message":"auto","finish-checkout":"work","finish-cleanup":"both"}"#).unwrap();
                let before = preservation::snapshot(root);
                check(
                    completion_run(exe, local.path(), command, &["--integration", "merge"]),
                    2,
                    &["require integrate mode"],
                );
                assert_eq!(preservation::snapshot(root), before);
                check(
                    completion_run(exe, local.path(), command, &[]),
                    0,
                    &["Published", "for review"],
                );
                published(&local, &remote);
                fs::write(&private, r#"{"finish-mode":"integrate"}"#).unwrap();
                check(
                    completion_run(exe, local.path(), command, &["--mode", "review"]),
                    0,
                    &["already published"],
                );
                fs::write(
                    &private,
                    r#"{"finish-mode":"review","finish-cleanup":"invalid"}"#,
                )
                .unwrap();
                let before = preservation::snapshot(root);
                check(
                    completion_run(exe, local.path(), command, &["--mode", "review"]),
                    1,
                    &["invalid settings", "finish-cleanup"],
                );
                assert_eq!(preservation::snapshot(root), before);
            }
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn completion_terminal_mode_choices_cancellation_and_settings_revalidation() {
    cases(
        "completion_terminal_mode_choices_cancellation_and_settings_revalidation",
        true,
        |root, exe| {
            for command in ["finish", "f"] {
                for answer in [
                    b"cancel\n".as_slice(),
                    b"3\n",
                    b"0\n",
                    b"99\n",
                    b"\x04",
                    b"review\x04\x04",
                    b"invalid\n",
                ] {
                    let (local, _remote) = fixture(root, false, true);
                    let before = preservation::snapshot(root);
                    let mut term = terminal::Terminal::start(exe, local.path(), &[command]);
                    term.until("Completion mode (");
                    term.send(answer);
                    if [b"0\n".as_slice(), b"99\n", b"invalid\n"].contains(&answer) {
                        term.until("Choose a listed name or number");
                        term.send(b"cancel\n");
                    }
                    assert_eq!(term.finish().code(), Some(1), "{}", term.output);
                    assert!(!term.output.contains("Default finish message"));
                    assert_eq!(preservation::snapshot(root), before);
                }
                let (local, remote) = fixture(root, false, false);
                let mut term = terminal::Terminal::start(exe, local.path(), &[command]);
                term.until("Completion mode (");
                term.send(if command == "f" { b"1\n" } else { b"review\n" });
                assert!(term.finish().success(), "{}", term.output);
                assert!(!term.output.contains("Integration commit message"));
                published(&local, &remote);
                let (local, _remote) = fixture(root, false, true);
                let mut term = terminal::Terminal::start(exe, local.path(), &[command]);
                term.until("Completion mode (");
                fs::write(
                    local.path().join(".git/git-oneflow.config.toml"),
                    "finish-mode='integrate'\n",
                )
                .unwrap();
                let before = preservation::snapshot(root);
                term.send(b"review\n");
                assert_eq!(term.finish().code(), Some(1), "{}", term.output);
                assert!(
                    term.output.contains("context") && term.output.contains("changed"),
                    "{}",
                    term.output
                );
                assert_eq!(preservation::snapshot(root), before);
            }
        },
    );
}

struct IntegrationFixture {
    local: Repository,
    project: tempfile::TempDir,
    fork: tempfile::TempDir,
}
fn integration_fixture(root: &Path, advanced: bool) -> IntegrationFixture {
    let local = Repository::new(root);
    commit(&local, "base", "initial\n");
    let project = tempfile::tempdir_in(root).unwrap();
    let fork = tempfile::tempdir_in(root).unwrap();
    for destination in [&project, &fork] {
        local.success(&[
            "clone".as_ref(),
            "--bare".as_ref(),
            local.path().as_os_str(),
            destination.path().as_os_str(),
        ]);
    }
    local.success(&["remote", "add", "project", project.path().to_str().unwrap()]);
    local.success(&["remote", "add", "personal", fork.path().to_str().unwrap()]);
    local.success(&["switch", "-c", BRANCH]);
    commit(&local, "work", "feature\n");
    local.success(&["push", "personal", &format!("HEAD:{REF}")]);
    if advanced {
        let upstream = Repository::new(root);
        upstream.success(&["fetch", project.path().to_str().unwrap(), "main"]);
        upstream.success(&["reset", "--hard", "FETCH_HEAD"]);
        commit(&upstream, "upstream", "new base\n");
        upstream.success(&[
            "push",
            project.path().to_str().unwrap(),
            "HEAD:refs/heads/main",
        ]);
    }
    fs::write(local.path().join(".git/git-oneflow.config.toml"),
        "finish-mode='integrate'\nfinish-message='auto'\nbase-remote='project'\npublish-remote='personal'\n").unwrap();
    IntegrationFixture {
        local,
        project,
        fork,
    }
}
fn remote_oid(path: &Path, reference: &str) -> Option<String> {
    let result = git::run(path, ["rev-parse", "--verify", reference]).unwrap();
    result
        .status
        .success()
        .then(|| String::from_utf8(result.stdout).unwrap().trim().into())
}
fn integration_run(exe: &str, f: &IntegrationFixture, args: &[&str]) -> Output {
    completion_run(exe, f.local.path(), "finish", args)
}
fn integration_status(output: Output, code: i32) -> String {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    format!(
        "{}{}",
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap()
    )
}
#[cfg(unix)]
fn integration_hook(repo: &Repository, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let hooks = repo.path().join(".git/integration-cli-hooks");
    fs::create_dir_all(&hooks).unwrap();
    repo.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let path = hooks.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}
#[cfg(unix)]
fn bare_hook(path: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let hooks = path.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join(name);
    fs::write(&hook, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    hook
}

#[test]
fn integration_methods_updates_completion_and_split_roles() {
    cases(
        "integration_methods_updates_completion_and_split_roles",
        true,
        |root, exe| {
            for (command, method, checkout, cleanup) in [
                ("finish", "ff-only", "target", "keep"),
                ("f", "ff-only", "target", "local"),
                ("finish", "ff-only", "target", "both"),
                ("f", "ff-only", "work", "keep"),
                ("finish", "merge", "target", "keep"),
                ("f", "squash", "target", "keep"),
            ] {
                let f = integration_fixture(root, false);
                let original = oid(&f.local, "HEAD").unwrap();
                let base = remote_oid(f.project.path(), "refs/heads/main").unwrap();
                let output = integration_status(
                    completion_run(
                        exe,
                        f.local.path(),
                        command,
                        &[
                            "--integration",
                            method,
                            "--checkout",
                            checkout,
                            "--cleanup",
                            cleanup,
                        ],
                    ),
                    0,
                );
                contains(&output, &[BRANCH, "project", "main"]);
                let result = remote_oid(f.project.path(), "refs/heads/main").unwrap();
                let parents = f.local.success(&["rev-list", "--parents", "-1", &result]);
                let parents = String::from_utf8(parents.stdout).unwrap();
                let parents: Vec<_> = parents.split_whitespace().collect();
                match method {
                    "ff-only" => assert_eq!(result, original),
                    "merge" => assert_eq!(&parents[1..], &[base.as_str(), original.as_str()]),
                    "squash" => assert_eq!(&parents[1..], &[base.as_str()]),
                    _ => unreachable!(),
                }
                assert_eq!(oid(&f.local, REF).is_some(), cleanup == "keep");
                assert_eq!(
                    remote_oid(f.fork.path(), REF),
                    (cleanup != "both").then_some(original)
                );
                assert_eq!(remote_oid(f.project.path(), REF), None);
                assert_eq!(
                    f.local.success(&["branch", "--show-current"]).stdout,
                    format!("{}\n", if checkout == "target" { "main" } else { BRANCH }).as_bytes()
                );
                assert!(f.local.success(&["tag", "--list"]).stdout.is_empty());
                assert!(
                    !git::run(
                        f.local.path(),
                        ["config", "--get", &format!("branch.{BRANCH}.remote")]
                    )
                    .unwrap()
                    .status
                    .success()
                );
                let before = oid(&f.local, "HEAD");
                integration_status(integration_run(exe, &f, &[]), 0);
                assert_eq!(oid(&f.local, "HEAD"), before);
            }
            for update in ["rebase", "merge", "none"] {
                let f = integration_fixture(root, true);
                let published = remote_oid(f.fork.path(), REF);
                let base = remote_oid(f.project.path(), "main").unwrap();
                integration_status(
                    integration_run(
                        exe,
                        &f,
                        &[
                            "--update",
                            update,
                            "--allow-rewrite",
                            "--integration",
                            "merge",
                            "--cleanup",
                            "keep",
                        ],
                    ),
                    0,
                );
                f.local
                    .success(&["merge-base", "--is-ancestor", &base, "HEAD"]);
                assert_eq!(
                    remote_oid(f.fork.path(), REF),
                    published,
                    "integration must not publish prepared work"
                );
            }
            let f = integration_fixture(root, true);
            let before = oid(&f.local, "HEAD");
            integration_status(
                integration_run(
                    exe,
                    &f,
                    &[
                        "--update",
                        "none",
                        "--integration",
                        "ff-only",
                        "--cleanup",
                        "keep",
                    ],
                ),
                1,
            );
            assert_eq!(oid(&f.local, "HEAD"), before);
        },
    );
}

#[test]
fn integration_literal_multiline_and_already_incorporated_work() {
    cases(
        "integration_literal_multiline_and_already_incorporated_work",
        true,
        |root, exe| {
            for method in ["merge", "squash"] {
                let f = integration_fixture(root, false);
                let message =
                    "Integration subject\n\nLiteral `touch injected` and $(touch injected)\n";
                integration_status(
                    integration_run(
                        exe,
                        &f,
                        &["--integration", method, "--cleanup", "keep", "-m", message],
                    ),
                    0,
                );
                assert_eq!(
                    f.local.success(&["log", "-1", "--format=%B"]).stdout,
                    format!("{message}\n").as_bytes()
                );
                assert!(!f.local.path().join("injected").exists());
            }
            let f = integration_fixture(root, false);
            f.local
                .success(&["push", "project", "HEAD:refs/heads/main"]);
            // Method, update and message defaults are ask; incorporated work needs none.
            fs::write(
                f.local.path().join(".git/git-oneflow.config.toml"),
                "finish-mode='integrate'\nbase-remote='project'\npublish-remote='personal'\n",
            )
            .unwrap();
            integration_status(integration_run(exe, &f, &["--cleanup", "keep"]), 0);
            assert_eq!(oid(&f.local, "HEAD"), remote_oid(f.fork.path(), REF));
        },
    );
}

#[test]
#[cfg(unix)]
fn integration_check_failure_reuses_result_and_delivered_retry_skips_effects() {
    cases(
        "integration_check_failure_reuses_result_and_delivered_retry_skips_effects",
        true,
        |root, exe| {
            for skip in [false, true] {
                let f = integration_fixture(root, false);
                let base = remote_oid(f.project.path(), "main");
                let private = f.local.path().join(".git/git-oneflow.config.toml");
                let mut config = fs::read_to_string(&private).unwrap();
                config.push_str("[[finish-checks]]\nname='Candidate gate'\ncommand=['sh','-c','echo check >> .git/check-count; echo streamed-candidate-output; test -f .git/check-pass']\n");
                fs::write(private, config).unwrap();
                let args = if skip {
                    vec![
                        "--integration",
                        "merge",
                        "--cleanup",
                        "local",
                        "--skip-checks",
                    ]
                } else {
                    vec!["--integration", "merge", "--cleanup", "local"]
                };
                let output =
                    integration_status(integration_run(exe, &f, &args), if skip { 0 } else { 1 });
                if skip {
                    assert!(output.to_lowercase().contains("skip"), "{output}");
                    assert!(!f.local.path().join(".git/check-count").exists());
                } else {
                    contains(&output, &["streamed-candidate-output", "Candidate gate"]);
                    assert_eq!(remote_oid(f.project.path(), "main"), base);
                    let candidate = oid(&f.local, "HEAD");
                    assert_eq!(
                        fs::read_to_string(f.local.path().join(".git/check-count")).unwrap(),
                        "check\n"
                    );
                    fs::write(f.local.path().join(".git/check-pass"), "").unwrap();
                    // Recovery restores the retained candidate when the developer returned to work.
                    f.local.success(&["switch", BRANCH]);
                    integration_status(integration_run(exe, &f, &[]), 0);
                    assert_eq!(oid(&f.local, "HEAD"), candidate);
                    assert_eq!(
                        fs::read_to_string(f.local.path().join(".git/check-count")).unwrap(),
                        "check\ncheck\n"
                    );
                }
                assert_eq!(oid(&f.local, REF), None);
                // A fresh process on the target after deletion uses retained choices, with no check or push.
                let count = fs::read(f.local.path().join(".git/check-count")).ok();
                bare_hook(
                    f.project.path(),
                    "pre-receive",
                    "echo duplicate-publication >&2; exit 1",
                );
                integration_status(integration_run(exe, &f, &[]), 0);
                assert_eq!(
                    fs::read(f.local.path().join(".git/check-count")).ok(),
                    count
                );
            }
        },
    );
}

#[test]
#[cfg(unix)]
fn integration_partial_target_delivery_and_remote_cleanup_resume() {
    cases(
        "integration_partial_target_delivery_and_remote_cleanup_resume",
        true,
        |root, exe| {
            for cleanup_failure in [false, true] {
                let f = integration_fixture(root, false);
                let second = tempfile::tempdir_in(root).unwrap();
                let source = if cleanup_failure {
                    f.fork.path()
                } else {
                    f.project.path()
                };
                f.local.success(&[
                    "clone".as_ref(),
                    "--bare".as_ref(),
                    source.as_os_str(),
                    second.path().as_os_str(),
                ]);
                let remote = if cleanup_failure {
                    "personal"
                } else {
                    "project"
                };
                for destination in [source, second.path()] {
                    f.local.success(&[
                        "config",
                        "--add",
                        &format!("remote.{remote}.pushurl"),
                        destination.to_str().unwrap(),
                    ]);
                }
                let reject = bare_hook(
                    second.path(),
                    "pre-receive",
                    "echo second-destination-rejected >&2; exit 1",
                );
                let original = oid(&f.local, "HEAD");
                let first = integration_status(
                    integration_run(
                        exe,
                        &f,
                        &[
                            "--integration",
                            "squash",
                            "-m",
                            "Keep accepted message",
                            "--cleanup",
                            "both",
                        ],
                    ),
                    1,
                );
                contains(
                    &first,
                    &[
                        "second-destination-rejected",
                        &second.path().display().to_string(),
                    ],
                );
                let candidate = remote_oid(f.project.path(), "main").unwrap();
                assert_eq!(oid(&f.local, "HEAD"), Some(candidate.clone()));
                if cleanup_failure {
                    assert_eq!(oid(&f.local, REF), None);
                    assert_eq!(remote_oid(f.fork.path(), REF), None);
                    assert_eq!(remote_oid(second.path(), REF), original);
                } else {
                    assert_eq!(oid(&f.local, REF), original);
                    assert_ne!(remote_oid(second.path(), "main"), Some(candidate.clone()));
                }
                fs::remove_file(reject).unwrap();
                integration_status(integration_run(exe, &f, &[]), 0);
                assert_eq!(oid(&f.local, "HEAD"), Some(candidate.clone()));
                assert_eq!(oid(&f.local, REF), None);
                assert_eq!(remote_oid(f.fork.path(), REF), None);
                if cleanup_failure {
                    assert_eq!(remote_oid(second.path(), REF), None);
                } else {
                    assert_eq!(remote_oid(second.path(), "main"), Some(candidate));
                }
                assert_eq!(
                    f.local.success(&["log", "-1", "--format=%s"]).stdout,
                    b"Keep accepted message\n"
                );
                integration_status(integration_run(exe, &f, &[]), 0);
            }
        },
    );
}

#[test]
#[cfg(unix)]
fn integration_checkout_failure_reports_delivery_and_retries_without_publication() {
    cases(
        "integration_checkout_failure_reports_delivery_and_retries_without_publication",
        true,
        |root, exe| {
            let f = integration_fixture(root, false);
            // Target candidate checkout is the first hook; permit that and fail final work checkout.
            integration_hook(
                &f.local,
                "post-checkout",
                "test \"$(git branch --show-current)\" != 'fix/deployment-bug' || test -f .git/checkout-pass || { echo checkout-refused >&2; exit 1; }",
            );
            let output = integration_status(
                integration_run(
                    exe,
                    &f,
                    &[
                        "--integration",
                        "merge",
                        "--checkout",
                        "work",
                        "--cleanup",
                        "keep",
                    ],
                ),
                1,
            );
            contains(
                &output,
                &[
                    "checkout-refused",
                    "Target delivery: Complete",
                    &f.project.path().display().to_string(),
                ],
            );
            let delivered = remote_oid(f.project.path(), "main");
            assert_ne!(delivered, remote_oid(f.fork.path(), REF));
            fs::write(f.local.path().join(".git/checkout-pass"), "").unwrap();
            integration_status(integration_run(exe, &f, &[]), 0);
            assert_eq!(remote_oid(f.project.path(), "main"), delivered);
            assert_eq!(
                f.local.success(&["branch", "--show-current"]).stdout,
                format!("{BRANCH}\n").as_bytes()
            );
            assert_eq!(oid(&f.local, REF), remote_oid(f.fork.path(), REF));
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn integration_terminal_choices_are_relevant_and_context_bound() {
    cases(
        "integration_terminal_choices_are_relevant_and_context_bound",
        true,
        |root, exe| {
            for choice in ["mode", "update", "method", "cleanup"] {
                for mutation in ["cancel", "config", "head", "endpoint", "remote"] {
                    if choice == "mode" && mutation == "remote" {
                        continue;
                    }
                    let f = integration_fixture(root, choice == "update");
                    let args = match choice {
                        "mode" => vec!["finish", "--integration", "ff-only", "--cleanup", "keep"],
                        "update" => vec!["finish", "--integration", "merge", "--cleanup", "keep"],
                        "method" => vec!["finish", "--cleanup", "keep"],
                        "cleanup" => vec!["finish", "--integration", "ff-only"],
                        _ => unreachable!(),
                    };
                    if choice == "mode" {
                        let private = f.local.path().join(".git/git-oneflow.config.toml");
                        let settings = fs::read_to_string(&private)
                            .unwrap()
                            .replace("finish-mode='integrate'", "finish-mode='ask'");
                        fs::write(private, settings).unwrap();
                    }
                    let prompt = match choice {
                        "mode" => "Completion mode (",
                        "update" => "Update (",
                        "method" => "Integration (",
                        "cleanup" => "Cleanup (",
                        _ => unreachable!(),
                    };
                    let mut term = terminal::Terminal::start(exe, f.local.path(), &args);
                    term.until(prompt);
                    let work = oid(&f.local, REF);
                    match mutation {
                        "config" => {
                            f.local
                                .success(&["config", "fixture.concurrent-change", "true"]);
                        }
                        "head" => commit(&f.local, "concurrent", "concurrent commit\n"),
                        "endpoint" => {
                            f.local.success(&[
                                "config",
                                "remote.project.pushurl",
                                f.fork.path().to_str().unwrap(),
                            ]);
                        }
                        "remote" => {
                            let upstream = Repository::new(root);
                            upstream.success(&[
                                "fetch",
                                f.project.path().to_str().unwrap(),
                                "main",
                            ]);
                            upstream.success(&["reset", "--hard", "FETCH_HEAD"]);
                            commit(&upstream, "concurrent", "concurrent remote commit\n");
                            upstream.success(&[
                                "push",
                                f.project.path().to_str().unwrap(),
                                "HEAD:refs/heads/main",
                            ]);
                        }
                        "cancel" => (),
                        _ => unreachable!(),
                    }
                    let expected_work = oid(&f.local, REF);
                    let expected_base = remote_oid(f.project.path(), "main");
                    let before = work_snapshot(&f.local);
                    term.send(if mutation != "cancel" {
                        match choice {
                            "mode" => b"integrate\n",
                            "update" => b"merge\n",
                            "method" => b"ff-only\n",
                            "cleanup" => b"keep\n",
                            _ => unreachable!(),
                        }
                    } else {
                        b"cancel\n"
                    });
                    assert_eq!(term.finish().code(), Some(1), "{}", term.output);
                    unchanged_work(&f.local, &before);
                    assert_eq!(oid(&f.local, REF), expected_work);
                    assert_eq!(remote_oid(f.project.path(), "main"), expected_base);
                    assert_eq!(remote_oid(f.fork.path(), REF), work);
                }
            }
            for answer in [b"ff-only\n".as_slice(), b"2\n", b"3\n"] {
                let f = integration_fixture(root, false);
                let mut term = terminal::Terminal::start(
                    exe,
                    f.local.path(),
                    &["finish", "--cleanup", "keep"],
                );
                term.until("Integration (");
                term.send(answer);
                assert!(term.finish().success(), "{}", term.output);
                assert!(!term.output.contains("Update ("), "{}", term.output);
                assert!(!term.output.contains("Cleanup ("), "{}", term.output);
                assert!(
                    !term.output.contains("Replace published history ("),
                    "{}",
                    term.output
                );
            }
            for answer in [b"bogus\n".as_slice(), b"\x04", b"ff-only\x04\x04"] {
                let f = integration_fixture(root, false);
                let before = work_snapshot(&f.local);
                let mut term = terminal::Terminal::start(
                    exe,
                    f.local.path(),
                    &["finish", "--cleanup", "keep"],
                );
                term.until("Integration (");
                term.send(answer);
                if answer == b"bogus\n" {
                    term.until("Choose a listed name or number");
                    term.send(b"cancel\n");
                }
                assert_eq!(term.finish().code(), Some(1), "{}", term.output);
                unchanged_work(&f.local, &before);
            }
        },
    );
}

#[cfg(unix)]
fn editor_script(root: &Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("message-editor");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}
#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[test]
#[cfg(target_os = "linux")]
fn integration_git_editor_suggestion_multiline_failure_and_context_guard() {
    cases(
        "integration_git_editor_suggestion_multiline_failure_and_context_guard",
        true,
        |root, exe| {
            for scenario in ["accepted", "failed", "blank", "nul", "non-utf8", "changed"] {
                let f = integration_fixture(root, false);
                let private = f.local.path().join(".git/git-oneflow.config.toml");
                fs::write(
                    &private,
                    fs::read_to_string(&private)
                        .unwrap()
                        .replace("finish-message='auto'", "finish-message='ask'"),
                )
                .unwrap();
                let editor = editor_script(
                    root,
                    match scenario {
                        "accepted" => {
                            "cp \"$1\" .git/editor-suggestion; printf 'Edited subject\\n\\nLiteral $(touch injected)\\n# comment\\n' > \"$1\""
                        }
                        "failed" => "exit 7",
                        "blank" => "printf '\\n# comment only\\n' > \"$1\"",
                        "nul" => "printf 'invalid\\000message\\n' > \"$1\"",
                        "non-utf8" => "printf '\\377\\n' > \"$1\"",
                        "changed" => {
                            "git config fixture.editor-change true; printf 'Edited subject\\n' > \"$1\""
                        }
                        _ => unreachable!(),
                    },
                );
                f.local.success(&[
                    "config",
                    "core.editor",
                    &shell_quote(editor.to_str().unwrap()),
                ]);
                let before = oid(&f.local, "HEAD");
                let base = remote_oid(f.project.path(), "main");
                let mut term = terminal::Terminal::start(
                    exe,
                    f.local.path(),
                    &["finish", "--integration", "merge", "--cleanup", "keep"],
                );
                assert_eq!(
                    term.finish().code(),
                    Some(if scenario == "accepted" { 0 } else { 1 }),
                    "{}",
                    term.output
                );
                if scenario == "accepted" {
                    let suggestion =
                        fs::read_to_string(f.local.path().join(".git/editor-suggestion")).unwrap();
                    contains(&suggestion, &[BRANCH, "main"]);
                    assert_eq!(
                        f.local.success(&["log", "-1", "--format=%B"]).stdout,
                        b"Edited subject\n\nLiteral $(touch injected)\n\n"
                    );
                    assert!(!f.local.path().join("injected").exists());
                } else {
                    assert_eq!(oid(&f.local, "HEAD"), before);
                    assert_eq!(remote_oid(f.project.path(), "main"), base);
                    assert_eq!(remote_oid(f.fork.path(), REF), before);
                }
            }
            // GIT_EDITOR overrides core.editor; no separate oneflow editor preference is needed.
            let f = integration_fixture(root, false);
            let private = f.local.path().join(".git/git-oneflow.config.toml");
            fs::write(
                &private,
                fs::read_to_string(&private)
                    .unwrap()
                    .replace("finish-message='auto'", "finish-message='ask'"),
            )
            .unwrap();
            f.local.success(&["config", "core.editor", "false"]);
            let editor = editor_script(root, "printf 'Git editor override\\n' > \"$1\"");
            let wrapper = root.join("editor-environment-cli");
            fs::write(
                &wrapper,
                format!(
                    "#!/bin/sh\nexport GIT_EDITOR={}\nexec {} \"$@\"\n",
                    shell_quote(&shell_quote(editor.to_str().unwrap())),
                    shell_quote(exe)
                ),
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
            let mut term = terminal::Terminal::start(
                wrapper.to_str().unwrap(),
                f.local.path(),
                &["finish", "--integration", "squash", "--cleanup", "keep"],
            );
            assert!(term.finish().success(), "{}", term.output);
            assert_eq!(
                f.local.success(&["log", "-1", "--format=%s"]).stdout,
                b"Git editor override\n"
            );
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn integration_cancelled_editor_after_preparation_retains_work_and_retries() {
    cases(
        "integration_cancelled_editor_after_preparation_retains_work_and_retries",
        true,
        |root, exe| {
            let f = integration_fixture(root, true);
            let private = f.local.path().join(".git/git-oneflow.config.toml");
            fs::write(
                &private,
                fs::read_to_string(&private)
                    .unwrap()
                    .replace("finish-message='auto'", "finish-message='ask'"),
            )
            .unwrap();
            let editor = editor_script(root, "exit 1");
            f.local.success(&[
                "config",
                "core.editor",
                &shell_quote(editor.to_str().unwrap()),
            ]);
            let original = oid(&f.local, REF);
            let target = remote_oid(f.project.path(), "main");
            let mut term = terminal::Terminal::start(
                exe,
                f.local.path(),
                &["finish", "--update", "merge", "--cleanup", "keep"],
            );
            term.until("Integration (");
            term.send(b"merge\n");
            assert_eq!(term.finish().code(), Some(1), "{}", term.output);
            let prepared = oid(&f.local, REF);
            assert_ne!(prepared, original);
            assert_eq!(remote_oid(f.project.path(), "main"), target);
            assert_eq!(remote_oid(f.fork.path(), REF), original);
            integration_status(
                integration_run(exe, &f, &["-m", "Accepted after preparation"]),
                0,
            );
            assert_eq!(oid(&f.local, REF), prepared);
        },
    );
}

#[test]
fn integration_streamed_credentials_follow_default_private_and_flag_preferences() {
    cases(
        "integration_streamed_credentials_follow_default_private_and_flag_preferences",
        true,
        |root, exe| {
            for preference in ["default", "private", "flag"] {
                let f = integration_fixture(root, false);
                let private = f.local.path().join(".git/git-oneflow.config.toml");
                let mut config = fs::read_to_string(&private).unwrap();
                if preference == "private" {
                    config.push_str("redact-credentials=false\n");
                }
                config.push_str("[[finish-checks]]\nname='Credential diagnostic'\ncommand=['sh','-c','echo https://synthetic-user:synthetic-secret@example.invalid/repository; exit 1']\n");
                fs::write(private, config).unwrap();
                let args = if preference == "flag" {
                    vec![
                        "--integration",
                        "ff-only",
                        "--cleanup",
                        "keep",
                        "--show-credentials",
                    ]
                } else {
                    vec!["--integration", "ff-only", "--cleanup", "keep"]
                };
                let output = integration_status(integration_run(exe, &f, &args), 1);
                if preference == "default" {
                    assert!(!output.contains("synthetic-secret"), "{output}");
                    assert!(!output.contains("synthetic-user"), "{output}");
                    contains(&output, &["<redacted>@example.invalid/repository"]);
                } else {
                    contains(
                        &output,
                        &["synthetic-user:synthetic-secret@example.invalid/repository"],
                    );
                }
                assert_ne!(oid(&f.local, "HEAD"), remote_oid(f.project.path(), "main"));
            }
        },
    );
}

#[test]
fn integration_completion_survives_removed_nested_caller_directory() {
    cases(
        "integration_completion_survives_removed_nested_caller_directory",
        true,
        |root, exe| {
            let f = integration_fixture(root, false);
            fs::create_dir(f.local.path().join("nested")).unwrap();
            commit(&f.local, "nested/work", "source-only directory\n");
            f.local
                .success(&["push", "personal", &format!("HEAD:{REF}")]);
            let upstream = Repository::new(root);
            upstream.success(&["fetch", f.fork.path().to_str().unwrap(), REF]);
            upstream.success(&["reset", "--hard", "FETCH_HEAD"]);
            upstream.success(&["rm", "nested/work"]);
            upstream.success(&["commit", "-m", "remove incorporated directory"]);
            upstream.success(&[
                "push",
                f.project.path().to_str().unwrap(),
                "HEAD:refs/heads/main",
            ]);
            let output = integration_status(
                completion_run(
                    exe,
                    &f.local.path().join("nested"),
                    "f",
                    &["--cleanup", "local"],
                ),
                0,
            );
            contains(
                &output,
                &["Integrated", "Final checkout:", "main", "local deleted"],
            );
            assert!(!f.local.path().join("nested").exists());
            assert_eq!(oid(&f.local, REF), None);
            assert_eq!(oid(&f.local, "HEAD"), remote_oid(f.project.path(), "main"));
            integration_status(integration_run(exe, &f, &[]), 0);
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn integration_editor_respects_git_message_cleanup_and_comment_preferences() {
    cases(
        "integration_editor_respects_git_message_cleanup_and_comment_preferences",
        true,
        |root, exe| {
            for policy in ["whitespace", "verbatim", "scissors", "comment-aliases"] {
                let f = integration_fixture(root, false);
                let private = f.local.path().join(".git/git-oneflow.config.toml");
                fs::write(
                    &private,
                    fs::read_to_string(&private)
                        .unwrap()
                        .replace("finish-message='auto'", "finish-message='ask'"),
                )
                .unwrap();
                let (body, expected) = match policy {
                    "whitespace" => (
                        "printf 'Subject  \\n\\n# retained comment\\n' > \"$1\"",
                        "Subject\n\n# retained comment\n\n",
                    ),
                    "verbatim" => (
                        "printf 'Subject  \\n\\n# retained comment\\n\\n' > \"$1\"",
                        "Subject  \n\n# retained comment\n\n\n",
                    ),
                    "scissors" => (
                        "printf 'Subject\\n\\n; ------------------------ >8 ------------------------\\nDiscarded tail\\n' > \"$1\"",
                        "Subject\n\n",
                    ),
                    "comment-aliases" => (
                        "printf 'Subject\\n; removed comment\\n// retained alias comment\\n' > \"$1\"",
                        "Subject\n// retained alias comment\n\n",
                    ),
                    _ => unreachable!(),
                };
                if policy == "comment-aliases" {
                    f.local.success(&["config", "core.commentString", "//"]);
                    f.local.success(&["config", "core.commentChar", ";"]);
                } else {
                    f.local.success(&["config", "commit.cleanup", policy]);
                    if policy == "scissors" {
                        f.local.success(&["config", "core.commentChar", ";"]);
                    }
                }
                let editor = editor_script(root, body);
                f.local.success(&[
                    "config",
                    "core.editor",
                    &shell_quote(editor.to_str().unwrap()),
                ]);
                let mut term = terminal::Terminal::start(
                    exe,
                    f.local.path(),
                    &["finish", "--integration", "squash", "--cleanup", "keep"],
                );
                assert!(term.finish().success(), "{}", term.output);
                assert_eq!(
                    f.local.success(&["log", "-1", "--format=%B"]).stdout,
                    expected.as_bytes(),
                    "policy={policy}"
                );
            }
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn integration_interrupted_before_result_retention_recovers_without_reintegration() {
    cases(
        "integration_interrupted_before_result_retention_recovers_without_reintegration",
        true,
        |root, exe| {
            for externally_delivered in [false, true] {
                let f = integration_fixture(root, false);
                let private = f.local.path().join(".git/git-oneflow.config.toml");
                let mut config = fs::read_to_string(&private).unwrap();
                config.push_str("[[finish-checks]]\nname='Recovered candidate'\ncommand=['sh','-c','echo checked >> .git/recovered-checks']\n");
                fs::write(private, config).unwrap();
                integration_hook(
                    &f.local,
                    "post-merge",
                    "oneflow_cli_pid=$(ps -o ppid= -p \"$PPID\")\nkill -KILL \"$oneflow_cli_pid\"",
                );
                let base = remote_oid(f.project.path(), "main");
                let interrupted = integration_run(
                    exe,
                    &f,
                    &[
                        "--integration",
                        "merge",
                        "--cleanup",
                        "local",
                        "-m",
                        "Retain interrupted message",
                    ],
                );
                assert_eq!(interrupted.status.code(), None, "{interrupted:?}");
                let candidate = oid(&f.local, "main");
                assert_ne!(candidate, base);
                assert_eq!(remote_oid(f.project.path(), "main"), base);
                assert!(
                    !String::from_utf8(
                        f.local
                            .success(&[
                                "for-each-ref",
                                "--format=%(refname)",
                                "refs/oneflow/integration/"
                            ])
                            .stdout
                    )
                    .unwrap()
                    .contains("/result")
                );
                f.local.success(&["merge", "--quit"]);
                if externally_delivered {
                    f.local
                        .success(&["push", "project", "main:refs/heads/main"]);
                    bare_hook(
                        f.project.path(),
                        "pre-receive",
                        "echo duplicate-delivery >&2; exit 1",
                    );
                }
                // The hook remains installed: a duplicate merge would kill this invocation too.
                integration_status(integration_run(exe, &f, &[]), 0);
                assert_eq!(oid(&f.local, "HEAD"), candidate);
                assert_eq!(remote_oid(f.project.path(), "main"), candidate);
                assert_eq!(oid(&f.local, REF), None);
                assert_eq!(
                    f.local.path().join(".git/recovered-checks").exists(),
                    !externally_delivered
                );
                assert_eq!(
                    f.local.success(&["log", "-1", "--format=%s"]).stdout,
                    b"Retain interrupted message\n"
                );
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn finish_capable_navigation_review_and_integration_delivery_cancel_and_failure() {
    cases(
        "finish_capable_navigation_review_and_integration_delivery_cancel_and_failure",
        true,
        |root, exe| {
            let (local, remote) = fixture(root, false, true);
            let mut terminal = terminal::Terminal::start_capable(exe, local.path(), &["finish"]);
            terminal.until("Completion mode (");
            terminal.send(b"\x1b[A\x1b[A\r");
            terminal.until("Update (");
            terminal.send(b"\x1b[A\r");
            assert!(terminal.finish().success(), "{}", terminal.output);
            terminal.assert_restored();
            published(&local, &remote);

            let f = integration_fixture(root, false);
            let original = oid(&f.local, REF);
            let mut terminal = terminal::Terminal::start_capable(exe, f.local.path(), &["finish"]);
            terminal.until("Integration (");
            terminal.send(b"\x1b[A\x1b[A\x1b[A\r");
            terminal.until("Cleanup (");
            terminal.send(b"\x1b[A\x1b[A\x1b[A\r");
            assert!(terminal.finish().success(), "{}", terminal.output);
            terminal.assert_restored();
            assert_eq!(remote_oid(f.project.path(), "main"), original);
            assert_eq!(oid(&f.local, REF), original);
            assert_eq!(remote_oid(f.fork.path(), REF), original);

            let f = integration_fixture(root, false);
            let before = work_snapshot(&f.local);
            let target = remote_oid(f.project.path(), "main");
            let mut terminal = terminal::Terminal::start_capable(exe, f.local.path(), &["finish"]);
            terminal.until("Integration (");
            terminal.send(b"\x1b");
            assert_eq!(terminal.finish().code(), Some(1), "{}", terminal.output);
            terminal.assert_restored();
            unchanged_work(&f.local, &before);
            assert_eq!(remote_oid(f.project.path(), "main"), target);

            let f = integration_fixture(root, false);
            let work = oid(&f.local, REF);
            let target = remote_oid(f.project.path(), "main");
            bare_hook(
                f.project.path(),
                "pre-receive",
                "echo capable-target-rejected >&2; exit 1",
            );
            let mut terminal = terminal::Terminal::start_capable(
                exe,
                f.local.path(),
                &["finish", "--cleanup", "keep"],
            );
            terminal.until("Integration (");
            terminal.send(b"\x1b[A\x1b[A\x1b[A\r");
            assert_eq!(terminal.finish().code(), Some(1), "{}", terminal.output);
            terminal.assert_restored();
            contains(
                &terminal.output,
                &["capable-target-rejected", "Finish integration incomplete"],
            );
            assert_eq!(oid(&f.local, REF), work);
            assert_eq!(remote_oid(f.fork.path(), REF), work);
            assert_eq!(fs::read(f.local.path().join("work")).unwrap(), b"feature\n");
            assert_eq!(remote_oid(f.project.path(), "main"), target);
        },
    );
}
