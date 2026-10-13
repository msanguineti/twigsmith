#![cfg(unix)]
mod support;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};

const WORK: &str = "work/history";
fn commit(repo: &Repository, name: &str) {
    fs::write(repo.path().join(name), format!("{name}\n")).unwrap();
    repo.success(&["add", name]);
    repo.success(&["commit", "-m", name]);
}
fn oid(repo: &Repository, reference: &str) -> String {
    String::from_utf8(repo.success(&["rev-parse", reference]).stdout)
        .unwrap()
        .trim()
        .into()
}
fn fixture(root: &Path, published: bool) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base");
    remote.success(&["config", "receive.denyCurrentBranch", "ignore"]);
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin"]);
    local.success(&["switch", "--no-track", "-c", WORK, "origin/main"]);
    for name in ["first", "second", "third"] {
        commit(&local, name);
    }
    if published {
        local.success(&["push", "origin", "HEAD:refs/heads/work/history"]);
    }
    fs::write(
        local.path().join(".git/git-oneflow.config.toml"),
        "finish-message='auto'\nfinish-cleanup='keep'\n",
    )
    .unwrap();
    (local, remote)
}
// Editors receive Git's native todo/message file as the final argument. Keep
// shell quoting correct for fixture paths containing spaces and apostrophes.
fn editor(root: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}
fn finish(repo: &Repository, mode: &str, sequence: &str, message: &str, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
        .current_dir(repo.path())
        .args([
            "finish",
            "--mode",
            mode,
            "--update",
            "rebase",
            "--history-edit",
            "interactive",
        ])
        .args(extra)
        .env("GIT_SEQUENCE_EDITOR", sequence)
        .env("GIT_EDITOR", message)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn success(output: Output) {
    assert!(output.status.success(), "{output:?}");
}
fn subjects(repo: &Repository, base: &str, tip: &str) -> Vec<String> {
    String::from_utf8(
        repo.success(&["log", "--reverse", "--format=%s", &format!("{base}..{tip}")])
            .stdout,
    )
    .unwrap()
    .lines()
    .map(str::to_owned)
    .collect()
}
#[test]
fn review_native_reorder_combine_reword_and_drop_publish_the_actual_result() {
    isolated(
        "review_native_reorder_combine_reword_and_drop_publish_the_actual_result",
        true,
        |root| {
            for (name, body, expected) in [
                ("unchanged", ":", vec!["first", "second", "third"]),
                (
                    "reorder",
                    "awk '/^pick / { a[++n]=$0; next } { print } END { for(i=n;i>0;i--)print a[i] }' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
                    vec!["third", "second", "first"],
                ),
                (
                    "combine",
                    "sed '0,/^pick /!s/^pick /fixup /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
                    vec!["first"],
                ),
                (
                    "reword",
                    "sed 's/^pick /reword /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
                    vec!["Edited subject", "Edited subject", "Edited subject"],
                ),
                (
                    "drop",
                    "sed 's/^pick /drop /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
                    vec![],
                ),
            ] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let (local, remote) = fixture(dir.path(), true);
                let base = oid(&remote, "main");
                let sequence = editor(dir.path(), name, body);
                let message = editor(dir.path(), "message", "printf 'Edited subject\\n' > \"$1\"");
                success(finish(
                    &local,
                    "review",
                    &sequence,
                    &message,
                    &["--allow-rewrite"],
                ));
                assert_eq!(subjects(&local, &base, "HEAD"), expected, "{name}");
                assert_eq!(oid(&remote, "refs/heads/work/history"), oid(&local, "HEAD"));
                assert_eq!(oid(&remote, "main"), base);
                if name == "drop" {
                    assert!(!local.path().join("first").exists());
                }
            }
        },
    );
}
#[test]
fn direct_editing_preserves_ff_only_and_no_ff_integration_choices() {
    isolated(
        "direct_editing_preserves_ff_only_and_no_ff_integration_choices",
        true,
        |root| {
            for method in ["ff-only", "merge"] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let (local, remote) = fixture(dir.path(), true);
                let published = oid(&remote, "refs/heads/work/history");
                commit(&remote, "incoming");
                let base = oid(&remote, "main");
                let sequence = editor(
                    dir.path(),
                    "combine",
                    "sed '0,/^pick /!s/^pick /fixup /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
                );
                success(finish(
                    &local,
                    "integrate",
                    &sequence,
                    ":",
                    &["--integration", method],
                ));
                let edited = oid(&local, WORK);
                assert_eq!(subjects(&local, &base, WORK), ["first"]);
                assert_eq!(oid(&remote, "main"), oid(&local, "HEAD"));
                assert_eq!(oid(&remote, "refs/heads/work/history"), published);
                if method == "ff-only" {
                    assert_eq!(oid(&local, "HEAD"), edited);
                } else {
                    assert_eq!(oid(&local, "HEAD^1"), base);
                    assert_eq!(oid(&local, "HEAD^2"), edited);
                }
            }
        },
    );
}
#[test]
fn direct_drop_all_needs_no_integration_message_or_new_commit() {
    isolated(
        "direct_drop_all_needs_no_integration_message_or_new_commit",
        true,
        |root| {
            let (local, remote) = fixture(root, false);
            let base = oid(&remote, "main");
            let sequence = editor(
                root,
                "drop",
                "sed 's/^pick /drop /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
            );
            fs::write(
                local.path().join(".git/git-oneflow.config.toml"),
                "finish-cleanup='keep'\n",
            )
            .unwrap();
            success(finish(
                &local,
                "integrate",
                &sequence,
                "false",
                &["--integration", "merge"],
            ));
            assert_eq!(oid(&local, "HEAD"), base);
            assert_eq!(oid(&remote, "main"), base);
            assert_eq!(oid(&local, WORK), base);
        },
    );
}
#[test]
fn native_edit_stop_continue_and_retry_do_not_reopen_the_editor() {
    isolated(
        "native_edit_stop_continue_and_retry_do_not_reopen_the_editor",
        true,
        |root| {
            for mode in ["review", "integrate"] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let (local, remote) = fixture(dir.path(), true);
                let base = oid(&remote, "main");
                let sequence = editor(
                    dir.path(),
                    "stop",
                    "sed '0,/^pick /s//edit /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
                );
                let extra: &[&str] = if mode == "review" {
                    &["--allow-rewrite"]
                } else {
                    &["--integration", "ff-only"]
                };
                let output = finish(&local, mode, &sequence, ":", extra);
                assert_eq!(output.status.code(), Some(1), "{output:?}");
                assert!(
                    local.path().join(".git/rebase-merge").is_dir(),
                    "{output:?}"
                );
                local.success(&["commit", "--amend", "-m", "Edited first"]);
                local.success(&["-c", "core.editor=true", "rebase", "--continue"]);
                success(finish(&local, mode, "false", "false", extra));
                assert_eq!(
                    subjects(&local, &base, WORK),
                    ["Edited first", "second", "third"]
                );
                assert_eq!(
                    oid(
                        &remote,
                        if mode == "review" {
                            "refs/heads/work/history"
                        } else {
                            "main"
                        }
                    ),
                    oid(&local, WORK)
                );
            }
        },
    );
}

#[test]
fn check_failure_retains_edited_result_and_retry_checks_that_result() {
    isolated(
        "check_failure_retains_edited_result_and_retry_checks_that_result",
        true,
        |root| {
            let (local, remote) = fixture(root, false);
            let check_path = local.path().join(".git/check");
            fs::write(&check_path, "#!/bin/sh\nexit 1\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&check_path, fs::Permissions::from_mode(0o755)).unwrap();
            fs::write(local.path().join(".git/git-oneflow.config.toml"),
            "finish-message='auto'\nfinish-cleanup='keep'\n[[finish-checks]]\nname='Edited tree'\ncommand=['sh','.git/check']\n").unwrap();
            let sequence = editor(
                root,
                "combine",
                "sed '0,/^pick /!s/^pick /fixup /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
            );
            let output = finish(&local, "review", &sequence, ":", &["--no-rewrite"]);
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            let edited = oid(&local, "HEAD");
            fs::write(
                &check_path,
                "#!/bin/sh\ntest \"$(git log -1 --format=%s)\" = first\n",
            )
            .unwrap();
            success(finish(
                &local,
                "review",
                "false",
                "false",
                &["--no-rewrite"],
            ));
            assert_eq!(oid(&local, "HEAD"), edited);
            assert_eq!(oid(&remote, "refs/heads/work/history"), edited);
        },
    );
}
#[test]
fn unavailable_editors_and_invalid_todo_cannot_publish() {
    isolated(
        "unavailable_editors_and_invalid_todo_cannot_publish",
        true,
        |root| {
            let (local, remote) = fixture(root, false);
            let source = oid(&local, "HEAD");
            let missing = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
                .current_dir(local.path())
                .args([
                    "finish",
                    "--mode",
                    "review",
                    "--update",
                    "rebase",
                    "--history-edit",
                    "interactive",
                    "--no-rewrite",
                ])
                .env_remove("GIT_EDITOR")
                .env_remove("GIT_SEQUENCE_EDITOR")
                .env_remove("VISUAL")
                .env_remove("EDITOR")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert_eq!(missing.status.code(), Some(1), "{missing:?}");
            assert_eq!(oid(&local, "HEAD"), source);
            let bad = editor(
                root,
                "invalid",
                "printf 'invalid todo instruction\\n' > \"$1\"",
            );
            assert_eq!(
                finish(&local, "review", &bad, ":", &["--no-rewrite"])
                    .status
                    .code(),
                Some(1)
            );
            assert!(
                !remote
                    .success(&["for-each-ref", "--format=%(refname)", "refs/heads/work/"])
                    .stdout
                    .contains(&b'w')
            );
        },
    );
}

#[test]
fn unchanged_direct_edit_is_completed_once_and_preserves_existing_commits() {
    isolated(
        "unchanged_direct_edit_is_completed_once_and_preserves_existing_commits",
        true,
        |root| {
            let (local, remote) = fixture(root, false);
            let source = oid(&local, "HEAD");
            success(finish(
                &local,
                "integrate",
                ":",
                ":",
                &["--integration", "ff-only"],
            ));
            assert_eq!(oid(&local, WORK), source);
            assert_eq!(oid(&remote, "main"), source);
            success(finish(
                &local,
                "integrate",
                "false",
                "false",
                &["--integration", "ff-only"],
            ));
            assert_eq!(oid(&remote, "main"), source);
        },
    );
}

#[test]
fn initial_no_work_still_honors_editor_failure_without_cleanup() {
    isolated(
        "initial_no_work_still_honors_editor_failure_without_cleanup",
        true,
        |root| {
            let (local, remote) = fixture(root, false);
            local.success(&["reset", "--hard", "origin/main"]);
            let base = oid(&remote, "main");
            let failed = finish(
                &local,
                "integrate",
                "false",
                ":",
                &["--integration", "ff-only", "--cleanup", "local"],
            );
            assert_eq!(failed.status.code(), Some(1), "{failed:?}");
            assert_eq!(oid(&local, WORK), base);
            assert_eq!(oid(&remote, "main"), base);
        },
    );
}

#[test]
fn editor_cannot_adopt_concurrent_local_or_remote_shared_history_changes() {
    isolated(
        "editor_cannot_adopt_concurrent_local_or_remote_shared_history_changes",
        true,
        |root| {
            for scope in ["local", "remote"] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let (local, remote) = fixture(dir.path(), false);
                let base = oid(&remote, "main");
                let body = if scope == "local" {
                    "git update-ref refs/heads/main HEAD".to_owned()
                } else {
                    let remote_path = format!(
                        "'{}'",
                        remote.path().to_str().unwrap().replace('\'', "'\\''")
                    );
                    format!("git -C {remote_path} commit --allow-empty -m concurrent")
                };
                let sequence = editor(dir.path(), "concurrent", &body);
                let failed = finish(&local, "review", &sequence, ":", &["--no-rewrite"]);
                assert_eq!(failed.status.code(), Some(1), "scope={scope}: {failed:?}");
                assert!(
                    remote
                        .success(&["for-each-ref", "--format=%(refname)", "refs/heads/work/"])
                        .stdout
                        .is_empty(),
                    "scope={scope}"
                );
                if scope == "local" {
                    assert_eq!(oid(&remote, "main"), base);
                } else {
                    assert_ne!(oid(&remote, "main"), base);
                }
            }
        },
    );
}

#[test]
fn distinct_preparation_and_integration_routes_preserve_both_shared_lines() {
    isolated(
        "distinct_preparation_and_integration_routes_preserve_both_shared_lines",
        true,
        |root| {
            for mode in ["review", "integrate"] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let remote = Repository::new(dir.path());
                commit(&remote, "base");
                remote.success(&["config", "receive.denyCurrentBranch", "ignore"]);
                remote.success(&["switch", "-c", "preparation"]);
                commit(&remote, "preparation-content");
                let preparation = oid(&remote, "HEAD");
                remote.success(&["switch", "main"]);
                commit(&remote, "target-content");
                let target = oid(&remote, "HEAD");
                let local = Repository::new(dir.path());
                local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
                local.success(&["fetch", "origin"]);
                local.success(&["reset", "--hard", "origin/main"]);
                fs::write(local.path().join(".git/git-oneflow.config.toml"),
                "finish-message='auto'\nfinish-cleanup='keep'\n[workstreams.ordinary]\npreparation-source='preparation'\nintegration-target='main'\n").unwrap();
                let start = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
                    .current_dir(local.path())
                    .args([
                        "start",
                        WORK,
                        "--policy",
                        "ordinary",
                        "--from",
                        "refs/remotes/origin/preparation",
                    ])
                    .stdin(Stdio::null())
                    .output()
                    .unwrap();
                success(start);
                commit(&local, "feature");
                let sequence = editor(
                    dir.path(),
                    "reword",
                    "sed 's/^pick /reword /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
                );
                let message = editor(dir.path(), "message", "printf 'Edited feature\\n' > \"$1\"");
                success(finish(
                    &local,
                    mode,
                    &sequence,
                    &message,
                    if mode == "review" {
                        &["--no-rewrite"]
                    } else {
                        &["--integration", "merge"]
                    },
                ));
                assert_eq!(oid(&remote, "preparation"), preparation);
                assert_eq!(oid(&local, &format!("{WORK}^")), preparation);
                assert_eq!(subjects(&local, &preparation, WORK), ["Edited feature"]);
                if mode == "review" {
                    assert_eq!(oid(&remote, "main"), target);
                    assert_eq!(oid(&remote, "refs/heads/work/history"), oid(&local, WORK));
                } else {
                    assert_eq!(oid(&remote, "main^1"), target);
                    assert_eq!(oid(&remote, "main^2"), oid(&local, WORK));
                }
            }
        },
    );
}

#[test]
fn linked_worktree_native_edit_completion_keeps_recovery_interval_offsets() {
    isolated(
        "linked_worktree_native_edit_completion_keeps_recovery_interval_offsets",
        true,
        |root| {
            let (local, remote) = fixture(root, false);
            local.success(&["switch", "--no-track", "-c", "parking", "origin/main"]);
            let linked = root.join("linked-work");
            local.success(&["worktree", "add", linked.to_str().unwrap(), WORK]);
            let sequence = editor(
                root,
                "reword",
                "sed 's/^pick /reword /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
            );
            let message = editor(root, "message", "printf 'Linked edit\\n' > \"$1\"");
            let output = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
                .current_dir(&linked)
                .args([
                    "finish",
                    "--mode",
                    "integrate",
                    "--update",
                    "rebase",
                    "--history-edit",
                    "interactive",
                    "--integration",
                    "ff-only",
                ])
                .env("GIT_SEQUENCE_EDITOR", sequence)
                .env("GIT_EDITOR", message)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            success(output);
            assert_eq!(oid(&remote, "main"), oid(&local, WORK));
            assert_eq!(
                subjects(&local, "origin/main", WORK),
                ["Linked edit", "Linked edit", "Linked edit"]
            );
            assert_eq!(oid(&local, "HEAD"), oid(&local, "parking"));
        },
    );
}

#[test]
fn changed_preparation_context_cannot_repeat_a_completed_edit() {
    isolated(
        "changed_preparation_context_cannot_repeat_a_completed_edit",
        true,
        |root| {
            let (local, remote) = fixture(root, false);
            success(finish(&local, "review", ":", ":", &["--no-rewrite"]));
            let source = oid(&local, "HEAD");
            commit(&remote, "later-base");
            let sequence = editor(
                root,
                "must-not-reopen",
                "printf opened > .git/repeated-editor; exit 1",
            );
            let stopped = finish(&local, "review", &sequence, ":", &["--allow-rewrite"]);
            assert_eq!(stopped.status.code(), Some(1), "{stopped:?}");
            assert!(
                !local.path().join(".git/repeated-editor").exists(),
                "{stopped:?}"
            );
            assert_eq!(oid(&local, "HEAD"), source);
            assert_eq!(oid(&remote, "refs/heads/work/history"), source);
        },
    );
}
