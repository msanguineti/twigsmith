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
fn cases(name: &str, available: bool, test: impl Fn(&Path, &str)) {
    isolated(name, available, |root| {
        for exe in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
            let dir = tempfile::tempdir_in(root).unwrap();
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
        repo.path().join(".git/oneflow.conf"),
        format!("finish-update={update}\nfinish-rewrite={rewrite}\n"),
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
                local.path().join(".git/oneflow.conf"),
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
    cases("finish_grammar_and_help_without_git", false, |root, exe| {
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
                        "--allow-rewrite",
                        "--no-rewrite",
                        "Exit status:",
                        "checks",
                        "deferred",
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
                vec!["--update", "merge", "--allow-rewrite"],
                vec!["--no-rewrite", "--update", "none"],
            ] {
                check(run_command(exe, root, command, &args), 1, &["Git"]);
            }
            for args in [
                vec!["--update"],
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
                            "Next: submit fix/deployment-bug for review into origin/main.",
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
                    let mut term = terminal::Terminal::start(exe, local.path(), &[command]);
                    term.until("Update (rebase/merge/none/cancel)");
                    contains(
                        &term.output,
                        &["Replay branch", "Incorporate", "without updating"],
                    );
                    term.send(format!("{update}\n").as_bytes());
                    if update == "rebase" {
                        term.until("Replace published history (allow/deny/cancel)");
                        unchanged_work(&local, &before);
                        assert_eq!(oid(&remote, REF), oid(&local, "HEAD"));
                        term.send(b"allow\n");
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
            let mut term =
                terminal::Terminal::start(exe, local.path(), &["finish", "--update", "rebase"]);
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
            let mut term = terminal::Terminal::start(exe, local.path(), &["finish"]);
            term.until("Update (");
            term.interrupt();
            assert!(!term.finish().success());
            unchanged_work(&local, &before);
            for rewrite in [false, true] {
                for answer in [
                    b"cancel\n".as_slice(),
                    b"\x04",
                    b"allow\x04\x04",
                    b"invalid\n",
                ] {
                    let (local, remote) = fixture(root, true, true);
                    let before = work_snapshot(&local);
                    let args = if rewrite {
                        vec!["finish", "--update", "rebase"]
                    } else {
                        vec!["finish"]
                    };
                    let mut term = terminal::Terminal::start(exe, local.path(), &args);
                    term.until(if rewrite {
                        "Replace published history ("
                    } else {
                        "Update ("
                    });
                    term.send(answer);
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
                        vec!["finish", "--update", "rebase"]
                    } else {
                        vec!["finish"]
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
