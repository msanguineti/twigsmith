#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use git_oneflow::{
    git,
    repository::{Head, Observation},
    save::{
        self, Plan, Request, SaveError, TreeState, WIP_REMAINING_MESSAGE, WIP_STAGED_MESSAGE,
        execution::{self, Effect, ExecutionError, LocalObservation, Operation, Report},
        publication::{DestinationState, PublicationState},
    },
    settings::SaveAction,
};
use preservation::snapshot;
use std::{
    fs,
    path::{Path, PathBuf},
};
use support::{Repository, isolated};

const BRANCH: &str = "work";
const BRANCH_REF: &str = "refs/heads/work";
fn output(repo: &Repository, args: &[&str]) -> Vec<u8> {
    repo.success(args).stdout
}
fn oid(repo: &Repository, reference: &str) -> Vec<u8> {
    output(repo, &["rev-parse", "--verify", reference])
        .strip_suffix(b"\n")
        .unwrap()
        .to_vec()
}
fn fixture(root: &Path) -> (Repository, PathBuf) {
    let repo = Repository::new(root);
    fs::write(repo.path().join("tracked"), "base\n").unwrap();
    fs::write(repo.path().join("delete"), "delete me\n").unwrap();
    fs::write(repo.path().join(".gitignore"), "ignored\n").unwrap();
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", "base"]);
    repo.success(&["switch", "-c", BRANCH]);
    let remote = root.join(format!(
        "{}.git",
        repo.path().file_name().unwrap().to_string_lossy()
    ));
    let init = git::run(
        root,
        ["init".as_ref(), "--bare".as_ref(), remote.as_os_str()],
    )
    .unwrap();
    assert!(init.status.success());
    repo.success(&[
        "remote".as_ref(),
        "add".as_ref(),
        "origin".as_ref(),
        remote.as_os_str(),
    ]);
    (repo, remote)
}
fn plan(repo: &Repository, action: SaveAction) -> Plan {
    save::assess(
        repo.path(),
        &Request {
            action: Some(action),
            message: (action == SaveAction::Staged)
                .then(|| "Developer selected work\n\nDetails".into()),
        },
    )
    .unwrap()
}
fn execute(repo: &Repository, action: SaveAction) -> Report {
    execution::execute(repo.path(), &plan(repo, action))
}
fn dirty(repo: &Repository, stage: bool, remaining: bool) {
    if stage {
        fs::write(repo.path().join("tracked"), "selected\n").unwrap();
        fs::write(repo.path().join("added"), "selected addition\n").unwrap();
        repo.success(&["add", "tracked", "added"]);
    }
    if remaining {
        fs::write(repo.path().join("tracked"), "remaining\n").unwrap();
        fs::remove_file(repo.path().join("delete")).unwrap();
        fs::write(repo.path().join("untracked"), "new\n").unwrap();
    }
    fs::write(repo.path().join("ignored"), "ignored local\n").unwrap();
}
fn remote_tip(remote: &Path) -> Option<Vec<u8>> {
    let output = git::run(remote, ["rev-parse", "--verify", BRANCH_REF]).unwrap();
    output
        .status
        .success()
        .then(|| output.stdout.strip_suffix(b"\n").unwrap().to_vec())
}
fn complete(repo: &Repository, remote: &Path, report: &Report) {
    assert!(report.is_complete(), "{report:#?}");
    assert_eq!(
        report.publication.as_ref().unwrap().state,
        PublicationState::Complete
    );
    assert_eq!(remote_tip(remote), Some(oid(repo, "HEAD")));
    assert_eq!(
        fs::read(repo.path().join("ignored")).unwrap(),
        b"ignored local\n"
    );
}
fn tree(repo: &Repository, revision: &str, path: &str) -> Option<Vec<u8>> {
    let out = git::run(repo.path(), ["show", &format!("{revision}:{path}")]).unwrap();
    out.status.success().then_some(out.stdout)
}
fn observed_head(observation: &LocalObservation) -> Vec<u8> {
    let Observation::Repository(state) = observation.repository.as_ref().unwrap() else {
        panic!()
    };
    let Head::Attached { commit, .. } = &state.head else {
        panic!()
    };
    commit.clone()
}
fn remaining(observation: &LocalObservation) -> bool {
    let Observation::Repository(state) = observation.repository.as_ref().unwrap() else {
        panic!()
    };
    state.working_tree.as_ref().unwrap().is_dirty()
}

#[test]
fn publish_only_preserves_all_four_tree_states() {
    isolated(
        "publish_only_preserves_all_four_tree_states",
        true,
        |root| {
            for (stage, remain) in [(false, false), (true, false), (false, true), (true, true)] {
                let (repo, remote) = fixture(root);
                dirty(&repo, stage, remain);
                let head = oid(&repo, "HEAD");
                let index = fs::read(repo.path().join(".git/index")).unwrap();
                let before = snapshot(repo.path());
                let report = execute(&repo, SaveAction::PublishOnly);
                complete(&repo, &remote, &report);
                assert!(report.commands.is_empty());
                assert_eq!(oid(&repo, "HEAD"), head);
                assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
                let after = snapshot(repo.path());
                for (path, value) in before
                    .iter()
                    .filter(|(path, _)| !path.starts_with(".git") && !path.as_os_str().is_empty())
                {
                    assert_eq!(after.get(path), Some(value));
                }
                assert_eq!(remaining(&report.after), stage || remain);
            }
        },
    );
}
#[test]
fn clean_actions_create_no_checkpoint_or_stash() {
    isolated(
        "clean_actions_create_no_checkpoint_or_stash",
        true,
        |root| {
            for action in [
                SaveAction::Ask,
                SaveAction::PublishOnly,
                SaveAction::Staged,
                SaveAction::Wip,
                SaveAction::Stash,
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, false, false);
                let report = execute(&repo, action);
                complete(&repo, &remote, &report);
                assert!(report.commands.is_empty());
                assert_eq!(observed_head(&report.before), observed_head(&report.after));
                assert_eq!(report.after.stash.as_ref().unwrap(), &None);
            }
        },
    );
}
#[test]
fn staged_preserves_partial_selection_and_remaining_work() {
    isolated(
        "staged_preserves_partial_selection_and_remaining_work",
        true,
        |root| {
            for remain in [false, true] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, remain);
                let report = execute(&repo, SaveAction::Staged);
                complete(&repo, &remote, &report);
                assert_eq!(report.commands.len(), 1);
                assert_eq!(
                    output(&repo, &["log", "-1", "--format=%B"]),
                    b"Developer selected work\n\nDetails\n\n"
                );
                assert_eq!(tree(&repo, "HEAD", "tracked"), Some(b"selected\n".to_vec()));
                assert_eq!(
                    tree(&repo, "HEAD", "added"),
                    Some(b"selected addition\n".to_vec())
                );
                assert_eq!(tree(&repo, "HEAD", "delete"), Some(b"delete me\n".to_vec()));
                assert_eq!(tree(&repo, "HEAD", "untracked"), None);
                assert!(output(&repo, &["diff", "--cached"]).is_empty());
                assert_eq!(
                    fs::read(repo.path().join("tracked")).unwrap(),
                    if remain {
                        b"remaining\n".as_slice()
                    } else {
                        b"selected\n".as_slice()
                    }
                );
                assert_eq!(remaining(&report.after), remain);
            }
        },
    );
}
#[test]
fn unresolved_or_invalid_inputs_precede_all_effects() {
    isolated(
        "unresolved_or_invalid_inputs_precede_all_effects",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            dirty(&repo, false, true);
            assert!(matches!(
                save::assess(
                    repo.path(),
                    &Request {
                        action: Some(SaveAction::Staged),
                        message: Some("message".into())
                    }
                ),
                Err(SaveError::NoStagedChanges)
            ));
            for action in [SaveAction::Ask, SaveAction::Staged] {
                if action == SaveAction::Staged {
                    repo.success(&["add", "tracked"]);
                }
                let approved = save::assess(
                    repo.path(),
                    &Request {
                        action: Some(action),
                        message: None,
                    },
                )
                .unwrap();
                let before = snapshot(repo.path());
                let report = execution::execute(repo.path(), &approved);
                assert!(matches!(
                    report.error,
                    Some(ExecutionError::RequiredInput(_))
                ));
                assert!(report.commands.is_empty());
                assert!(report.publication.is_none());
                assert_eq!(snapshot(repo.path()), before);
            }
            assert!(matches!(
                save::assess(
                    repo.path(),
                    &Request {
                        action: Some(SaveAction::Staged),
                        message: Some(" \n\t".into())
                    }
                ),
                Err(SaveError::BlankMessage)
            ));
            assert!(remote_tip(&remote).is_none());
        },
    );
}
#[test]
fn wip_commits_content_in_order_omitting_empty_groups() {
    isolated(
        "wip_commits_content_in_order_omitting_empty_groups",
        true,
        |root| {
            for (stage, remain) in [(false, false), (true, false), (false, true), (true, true)] {
                let (repo, remote) = fixture(root);
                dirty(&repo, stage, remain);
                let report = execute(&repo, SaveAction::Wip);
                complete(&repo, &remote, &report);
                let log = output(&repo, &["log", "--reverse", "--format=%s", "main..HEAD"]);
                let expected = [(stage, WIP_STAGED_MESSAGE), (remain, WIP_REMAINING_MESSAGE)]
                    .into_iter()
                    .filter(|(include, _)| *include)
                    .map(|(_, message)| format!("{message}\n"))
                    .collect::<String>();
                assert_eq!(log, expected.as_bytes());
                if stage {
                    let revision = if remain { "HEAD^" } else { "HEAD" };
                    assert_eq!(
                        tree(&repo, revision, "tracked"),
                        Some(b"selected\n".to_vec())
                    );
                    assert_eq!(
                        tree(&repo, revision, "added"),
                        Some(b"selected addition\n".to_vec())
                    );
                    assert_eq!(
                        tree(&repo, revision, "delete"),
                        Some(b"delete me\n".to_vec())
                    );
                    assert_eq!(tree(&repo, revision, "untracked"), None);
                }
                if remain {
                    assert_eq!(
                        tree(&repo, "HEAD", "tracked"),
                        Some(b"remaining\n".to_vec())
                    );
                    assert_eq!(tree(&repo, "HEAD", "delete"), None);
                    assert_eq!(tree(&repo, "HEAD", "untracked"), Some(b"new\n".to_vec()));
                }
                assert_eq!(tree(&repo, "HEAD", "ignored"), None);
                assert!(!remaining(&report.after));
                let retried = execute(&repo, SaveAction::Wip);
                assert!(retried.is_complete(), "{retried:?}");
                assert!(retried.commands.is_empty());
                assert_eq!(
                    retried.publication.unwrap().destinations[0].state,
                    DestinationState::Unchanged
                );
            }
        },
    );
}
#[test]
fn stash_recovers_index_tree_and_untracked_without_touching_older_entries() {
    isolated(
        "stash_recovers_index_tree_and_untracked_without_touching_older_entries",
        true,
        |root| {
            for (stage, remain) in [(false, false), (true, false), (false, true), (true, true)] {
                let (repo, remote) = fixture(root);
                fs::write(repo.path().join("tracked"), "older stash\n").unwrap();
                repo.success(&["stash", "push", "-m", "older"]);
                let older = oid(&repo, "refs/stash");
                dirty(&repo, stage, remain);
                let before_tree = snapshot(repo.path());
                let before_index = output(&repo, &["ls-files", "--stage", "-z"]);
                let before_status = output(
                    &repo,
                    &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
                );
                let report = execute(&repo, SaveAction::Stash);
                complete(&repo, &remote, &report);
                assert!(!remaining(&report.after));
                assert_eq!(observed_head(&report.before), observed_head(&report.after));
                if stage || remain {
                    assert_eq!(report.commands.len(), 1);
                    assert_ne!(report.after.stash.as_ref().unwrap(), &Some(older.clone()));
                    assert!(
                        matches!(&report.effects[..], [Effect::LocalStash { commit }] if Some(commit) == report.after.stash.as_ref().unwrap().as_ref())
                    );
                    let local_stash = report.after.stash.as_ref().unwrap().as_ref().unwrap();
                    assert!(
                        !git::run(
                            &remote,
                            ["cat-file", "-e", std::str::from_utf8(local_stash).unwrap()]
                        )
                        .unwrap()
                        .status
                        .success(),
                        "stash was transferred remotely"
                    );
                    assert_eq!(oid(&repo, "stash@{1}"), older);
                    assert!(
                        String::from_utf8(output(&repo, &["stash", "list"]))
                            .unwrap()
                            .contains(execution::STASH_MESSAGE)
                    );
                    assert_eq!(fs::read(repo.path().join("tracked")).unwrap(), b"base\n");
                    assert!(!repo.path().join("untracked").exists());
                    // Ordinary Git recovery is a test control, not an executor action.
                    repo.success(&["stash", "apply", "--index", "stash@{0}"]);
                    assert_eq!(output(&repo, &["ls-files", "--stage", "-z"]), before_index);
                    assert_eq!(
                        output(
                            &repo,
                            &["status", "--porcelain=v2", "-z", "--untracked-files=all"]
                        ),
                        before_status
                    );
                    let recovered = snapshot(repo.path());
                    for (path, value) in before_tree.iter().filter(|(path, _)| {
                        !path.starts_with(".git") && !path.as_os_str().is_empty()
                    }) {
                        // Git restoration legitimately changes timestamps; compare contents below.
                        assert!(recovered.contains_key(path), "{path:?}: {value:?}");
                    }
                    assert_eq!(
                        fs::read(repo.path().join("tracked")).unwrap(),
                        if remain {
                            b"remaining\n".as_slice()
                        } else {
                            b"selected\n".as_slice()
                        }
                    );
                    if stage {
                        assert_eq!(
                            fs::read(repo.path().join("added")).unwrap(),
                            b"selected addition\n"
                        );
                    }
                    if remain {
                        assert!(!repo.path().join("delete").exists());
                        assert_eq!(fs::read(repo.path().join("untracked")).unwrap(), b"new\n");
                    }
                    assert_eq!(oid(&repo, "stash@{1}"), older);
                } else {
                    assert!(report.commands.is_empty());
                    assert_eq!(report.after.stash.as_ref().unwrap(), &Some(older));
                }
            }
        },
    );
}

#[test]
fn staged_deletions_and_clean_save_with_index_lock() {
    isolated(
        "staged_deletions_and_clean_save_with_index_lock",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            dirty(&repo, false, false);
            repo.success(&["rm", "delete"]);
            fs::write(repo.path().join("untracked"), "keep local\n").unwrap();
            let report = execute(&repo, SaveAction::Staged);
            complete(&repo, &remote, &report);
            assert_eq!(tree(&repo, "HEAD", "delete"), None);
            assert_eq!(
                fs::read(repo.path().join("untracked")).unwrap(),
                b"keep local\n"
            );
            fs::remove_file(repo.path().join("untracked")).unwrap();
            fs::write(repo.path().join(".git/index.lock"), "existing lock").unwrap();
            for action in [SaveAction::Staged, SaveAction::Wip, SaveAction::Stash] {
                let report = execute(&repo, action);
                complete(&repo, &remote, &report);
                assert!(report.commands.is_empty());
                assert_eq!(
                    fs::read(repo.path().join(".git/index.lock")).unwrap(),
                    b"existing lock"
                );
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn failed_local_commands_with_completed_effects_are_reported_and_retried() {
    use std::{env, os::unix::fs::PermissionsExt, process::Command};
    const TEST: &str = "failed_local_commands_with_completed_effects_are_reported_and_retried";
    isolated(TEST, true, |root| {
        if env::var_os("ONEFLOW_EXECUTION_WRAPPER").is_none() {
            let wrapper = root.join("git-wrapper");
            fs::create_dir(&wrapper).unwrap();
            let script = wrapper.join("git");
            fs::write(&script, "#!/bin/sh\ncase \"$*\" in\n  *' add --all -- '*|*' commit --message '*|*' stash push --include-untracked --message '*)\n    if test -f .git/fail-command; then\n      \"$ONEFLOW_REAL_GIT\" \"$@\" || exit $?\n      printf 'fixture failure after actual effect\\n' >&2\n      exit 1\n    fi;;\nesac\nexec \"$ONEFLOW_REAL_GIT\" \"$@\"\n").unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            let real = Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap();
            assert!(real.status.success());
            let real = String::from_utf8(real.stdout).unwrap();
            let out = Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(
                    "PATH",
                    format!("{}:{}", wrapper.display(), env::var("PATH").unwrap()),
                )
                .env("ONEFLOW_REAL_GIT", real.trim())
                .env("ONEFLOW_EXECUTION_WRAPPER", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        for action in [SaveAction::Wip, SaveAction::Staged, SaveAction::Stash] {
            let (repo, remote) = fixture(root);
            dirty(&repo, action == SaveAction::Staged, true);
            fs::write(repo.path().join(".git/fail-command"), "fail once").unwrap();
            let report = execute(&repo, action);
            assert!(matches!(
                report.error,
                Some(ExecutionError::LocalCommandFailed)
            ));
            assert_eq!(report.commands.len(), 1);
            assert!(report.effects.is_empty());
            assert!(report.publication.is_none());
            assert!(remote_tip(&remote).is_none());
            match action {
                SaveAction::Wip => {
                    assert!(matches!(
                        report.commands[0].operation,
                        Operation::StageRemaining { .. }
                    ));
                    assert_ne!(
                        report.before.index.as_ref().unwrap(),
                        report.after.index.as_ref().unwrap()
                    );
                    assert_eq!(observed_head(&report.before), observed_head(&report.after));
                    assert_eq!(
                        plan(&repo, action).scope.tree_state(),
                        TreeState::StagedOnly
                    );
                }
                SaveAction::Staged => {
                    assert_ne!(observed_head(&report.before), observed_head(&report.after));
                    assert_eq!(tree(&repo, "HEAD", "tracked"), Some(b"selected\n".to_vec()));
                    assert!(remaining(&report.after));
                }
                SaveAction::Stash => {
                    assert!(report.before.stash.as_ref().unwrap().is_none());
                    assert!(report.after.stash.as_ref().unwrap().is_some());
                    assert!(!remaining(&report.after));
                }
                _ => unreachable!(),
            }
            fs::remove_file(repo.path().join(".git/fail-command")).unwrap();
            let retry = execute(
                &repo,
                if action == SaveAction::Staged {
                    SaveAction::PublishOnly
                } else {
                    action
                },
            );
            complete(&repo, &remote, &retry);
            if action != SaveAction::Wip {
                assert!(retry.commands.is_empty());
            } else {
                assert_eq!(retry.commands.len(), 1);
            }
            assert_eq!(
                retry.after.stash.as_ref().unwrap(),
                report.after.stash.as_ref().unwrap()
            );
        }
    });
}

#[cfg(unix)]
#[test]
fn stash_cleanup_failure_keeps_recoverable_stash_and_remaining_work() {
    isolated(
        "stash_cleanup_failure_keeps_recoverable_stash_and_remaining_work",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            hook(
                &repo,
                "reference-transaction",
                "if test \"$1\" = committed; then\n  while read old new reference; do\n    if test \"$reference\" = refs/stash; then printf 'hook lock\\n' > .git/index.lock; fi\n  done\nfi",
            );
            let report = execute(&repo, SaveAction::Stash);
            assert!(
                matches!(report.error, Some(ExecutionError::LocalCommandFailed)),
                "{report:?}"
            );
            assert_eq!(report.commands.len(), 1);
            assert!(report.publication.is_none());
            let saved = report.after.stash.as_ref().unwrap().as_ref().unwrap();
            assert_eq!(
                tree(&repo, std::str::from_utf8(saved).unwrap(), "tracked"),
                Some(b"remaining\n".to_vec())
            );
            assert_eq!(
                tree(&repo, "refs/stash^2", "tracked"),
                Some(b"selected\n".to_vec())
            );
            assert!(remaining(&report.after));
            assert_eq!(observed_head(&report.before), observed_head(&report.after));
            assert!(repo.path().join(".git/index.lock").exists());
            assert!(remote_tip(&remote).is_none());
            fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
            fs::remove_file(repo.path().join(".git/hooks/reference-transaction")).unwrap();
            // Recover independently in a fresh tree, leaving the failed tree intact.
            let recovery = root.join("stash-recovery");
            repo.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "--detach".as_ref(),
                recovery.as_os_str(),
                "main".as_ref(),
            ]);
            let recovered = git::run(
                &recovery,
                [
                    "stash",
                    "apply",
                    "--index",
                    std::str::from_utf8(saved).unwrap(),
                ],
            )
            .unwrap();
            assert!(
                recovered.status.success(),
                "{}",
                String::from_utf8_lossy(&recovered.stderr)
            );
            assert_eq!(fs::read(recovery.join("tracked")).unwrap(), b"remaining\n");
            assert_eq!(
                git::run(&recovery, ["show", ":tracked"]).unwrap().stdout,
                b"selected\n"
            );
            assert_eq!(fs::read(recovery.join("untracked")).unwrap(), b"new\n");
            assert_eq!(oid(&repo, "refs/stash"), *saved);
        },
    );
}

#[cfg(unix)]
#[test]
fn signing_failure_is_respected_after_staging_without_publication() {
    isolated(
        "signing_failure_is_respected_after_staging_without_publication",
        true,
        |root| {
            use std::os::unix::fs::PermissionsExt;
            let (repo, remote) = fixture(root);
            dirty(&repo, false, true);
            let signer = repo.path().join(".git/signer");
            fs::write(
                &signer,
                "#!/bin/sh\nprintf 'signing attempted\\n' > .git/signing-attempt\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(&signer, fs::Permissions::from_mode(0o755)).unwrap();
            repo.success(&[
                "config".as_ref(),
                "gpg.program".as_ref(),
                signer.as_os_str(),
            ]);
            repo.success(&["config", "commit.gpgsign", "true"]);
            let report = execute(&repo, SaveAction::Wip);
            assert!(matches!(
                report.error,
                Some(ExecutionError::LocalCommandFailed)
            ));
            assert_eq!(report.commands.len(), 2);
            assert!(matches!(
                &report.effects[..],
                [Effect::StagedRemaining { .. }]
            ));
            assert!(repo.path().join(".git/signing-attempt").exists());
            assert_eq!(observed_head(&report.before), observed_head(&report.after));
            assert_ne!(
                report.before.index.as_ref().unwrap(),
                report.after.index.as_ref().unwrap()
            );
            assert!(report.publication.is_none());
            assert!(remote_tip(&remote).is_none());
        },
    );
}

#[test]
fn linked_worktree_uses_its_own_index_and_publishes_its_branch() {
    isolated(
        "linked_worktree_uses_its_own_index_and_publishes_its_branch",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            let linked = root.join("linked-worktree");
            repo.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "-b".as_ref(),
                "linked-work".as_ref(),
                linked.as_os_str(),
                "main".as_ref(),
            ]);
            fs::write(linked.join("tracked"), "selected\n").unwrap();
            assert!(
                git::run(&linked, ["add", "tracked"])
                    .unwrap()
                    .status
                    .success()
            );
            fs::write(linked.join("tracked"), "remaining\n").unwrap();
            let original_index = fs::read(repo.path().join(".git/index")).unwrap();
            let approved = save::assess(
                &linked,
                &Request {
                    action: Some(SaveAction::Wip),
                    message: None,
                },
            )
            .unwrap();
            let report = execution::execute(&linked, &approved);
            assert!(report.is_complete(), "{report:?}");
            assert_eq!(
                git::run(&linked, ["show", "HEAD^:tracked"]).unwrap().stdout,
                b"selected\n"
            );
            assert_eq!(
                git::run(&linked, ["show", "HEAD:tracked"]).unwrap().stdout,
                b"remaining\n"
            );
            assert_eq!(
                fs::read(repo.path().join(".git/index")).unwrap(),
                original_index
            );
            assert_eq!(
                git::run(&remote, ["rev-parse", "refs/heads/linked-work"])
                    .unwrap()
                    .stdout,
                git::run(&linked, ["rev-parse", "HEAD"]).unwrap().stdout
            );
            assert!(remote_tip(&remote).is_none());
        },
    );
}

#[test]
fn selected_index_path_is_used_for_lock_guards_and_observations() {
    use std::{env, process::Command};
    const TEST: &str = "selected_index_path_is_used_for_lock_guards_and_observations";
    isolated(TEST, true, |root| {
        if env::var_os("ONEFLOW_CUSTOM_INDEX").is_none() {
            let out = Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env("GIT_INDEX_FILE", root.join("selected.index"))
                .env("ONEFLOW_CUSTOM_INDEX", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let (repo, remote) = fixture(root);
        dirty(&repo, true, true);
        let index_path = PathBuf::from(env::var_os("GIT_INDEX_FILE").unwrap());
        let lock_path = root.join("selected.index.lock");
        fs::write(&lock_path, "custom lock").unwrap();
        let report = execute(&repo, SaveAction::Wip);
        assert!(matches!(report.error, Some(ExecutionError::IndexLocked)));
        assert!(report.commands.is_empty());
        assert_eq!(
            report.before.index.as_ref().unwrap().as_ref().unwrap(),
            &fs::read(&index_path).unwrap()
        );
        assert!(!repo.path().join(".git/index").exists());
        fs::remove_file(lock_path).unwrap();
        let report = execute(&repo, SaveAction::Wip);
        complete(&repo, &remote, &report);
        assert_eq!(
            report.after.index.as_ref().unwrap().as_ref().unwrap(),
            &fs::read(index_path).unwrap()
        );
        assert!(!repo.path().join(".git/index").exists());
    });
}

#[test]
fn clean_submodule_revisions_are_separate_wip_checkpoints() {
    isolated(
        "clean_submodule_revisions_are_separate_wip_checkpoints",
        true,
        |root| {
            let child = Repository::new(root);
            fs::write(child.path().join("content"), "base\n").unwrap();
            child.success(&["add", "."]);
            child.success(&["commit", "-m", "base"]);
            let (repo, remote) = fixture(root);
            repo.success(&[
                "-c".as_ref(),
                "protocol.file.allow=always".as_ref(),
                "submodule".as_ref(),
                "add".as_ref(),
                child.path().as_os_str(),
                "child".as_ref(),
            ]);
            repo.success(&["commit", "-m", "add child"]);
            let child_path = repo.path().join("child");
            for args in [
                ["config", "user.name", "Fixture User"],
                ["config", "user.email", "fixture@example.invalid"],
            ] {
                assert!(git::run(&child_path, args).unwrap().status.success());
            }
            let mut revisions = vec![];
            for (contents, stage) in [("selected child\n", true), ("remaining child\n", false)] {
                fs::write(child_path.join("content"), contents).unwrap();
                assert!(
                    git::run(&child_path, ["add", "."])
                        .unwrap()
                        .status
                        .success()
                );
                assert!(
                    git::run(&child_path, ["commit", "-m", contents])
                        .unwrap()
                        .status
                        .success()
                );
                revisions.push(git::run(&child_path, ["rev-parse", "HEAD"]).unwrap().stdout);
                if stage {
                    repo.success(&["add", "child"]);
                }
            }
            fs::write(repo.path().join("ignored"), "ignored local\n").unwrap();
            let report = execute(&repo, SaveAction::Wip);
            complete(&repo, &remote, &report);
            assert_eq!(output(&repo, &["rev-parse", "HEAD^:child"]), revisions[0]);
            assert_eq!(output(&repo, &["rev-parse", "HEAD:child"]), revisions[1]);
            assert!(!remaining(&report.after));
        },
    );
}

#[test]
fn identical_stash_oid_remains_recoverable_without_disturbing_entries() {
    use std::{env, process::Command};
    const TEST: &str = "identical_stash_oid_remains_recoverable_without_disturbing_entries";
    isolated(TEST, true, |root| {
        if env::var_os("ONEFLOW_FIXED_DATE").is_none() {
            let out = Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2001-01-01T00:00:00Z")
                .env("ONEFLOW_FIXED_DATE", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let (repo, remote) = fixture(root);
        dirty(&repo, true, true);
        repo.success(&[
            "stash",
            "push",
            "--include-untracked",
            "--message",
            execution::STASH_MESSAGE,
        ]);
        let older = oid(&repo, "refs/stash");
        dirty(&repo, true, true);
        let report = execute(&repo, SaveAction::Stash);
        complete(&repo, &remote, &report);
        assert_eq!(report.before.stash.as_ref().unwrap(), &Some(older.clone()));
        assert_eq!(report.after.stash.as_ref().unwrap(), &Some(older.clone()));
        assert_eq!(
            output(&repo, &["stash", "list"])
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            1
        );
        assert!(matches!(&report.effects[..], [Effect::LocalStash { .. }]));
        repo.success(&["stash", "apply", "--index"]);
        assert_eq!(
            fs::read(repo.path().join("tracked")).unwrap(),
            b"remaining\n"
        );
        assert_eq!(output(&repo, &["show", ":tracked"]), b"selected\n");
    });
}
#[test]
fn stale_approval_refuses_changed_context_and_content_before_writes() {
    isolated(
        "stale_approval_refuses_changed_context_and_content_before_writes",
        true,
        |root| {
            for change in [
                "branch",
                "commit",
                "upstream",
                "settings",
                "url",
                "index",
                "same-path",
                "new-path",
                "ignored-scope",
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let approved = plan(&repo, SaveAction::Wip);
                match change {
                    "branch" => {
                        repo.success(&["switch", "-c", "another"]);
                    }
                    "commit" => {
                        repo.success(&["commit", "-m", "other writer"]);
                    }
                    "upstream" => {
                        repo.success(&["config", "branch.work.remote", "another"]);
                    }
                    "settings" => fs::write(
                        repo.path().join(".git/oneflow.conf"),
                        "save-action = stash\n",
                    )
                    .unwrap(),
                    "url" => {
                        repo.success(&["remote", "set-url", "origin", "/missing/other"]);
                    }
                    "index" => {
                        repo.success(&["add", "tracked"]);
                    }
                    "same-path" => {
                        fs::write(repo.path().join("tracked"), "changed approval\n").unwrap()
                    }
                    "new-path" => {
                        fs::write(repo.path().join("new-path"), "changed scope\n").unwrap()
                    }
                    "ignored-scope" => {
                        fs::write(repo.path().join(".gitignore"), "ignored\nuntracked\n").unwrap()
                    }
                    _ => unreachable!(),
                }
                let before = snapshot(repo.path());
                let report = execution::execute(repo.path(), &approved);
                assert!(report.error.is_some(), "{change}: {report:#?}");
                assert!(report.commands.is_empty());
                assert!(report.publication.is_none());
                assert_eq!(snapshot(repo.path()), before);
                assert!(remote_tip(&remote).is_none());
            }
        },
    );
}
#[test]
fn known_publication_blockers_and_index_lock_precede_local_effects() {
    isolated(
        "known_publication_blockers_and_index_lock_precede_local_effects",
        true,
        |root| {
            for action in [SaveAction::Staged, SaveAction::Wip, SaveAction::Stash] {
                for blocker in ["mapping", "symbolic", "lock", "identity"] {
                    let (repo, remote) = fixture(root);
                    dirty(&repo, true, true);
                    match blocker {
                        "mapping" => {
                            repo.success(&[
                                "config",
                                "remote.origin.fetch",
                                "+refs/heads/*:refs/heads/*",
                            ]);
                        }
                        "symbolic" => {
                            repo.success(&["symbolic-ref", "refs/remotes/origin/work", BRANCH_REF]);
                        }
                        "lock" => fs::write(repo.path().join(".git/index.lock"), "lock").unwrap(),
                        "identity" => {
                            repo.success(&["config", "user.name", ""]);
                        }
                        _ => unreachable!(),
                    }
                    let approved = plan(&repo, action);
                    let before = snapshot(repo.path());
                    let report = execution::execute(repo.path(), &approved);
                    assert!(report.error.is_some(), "{action:?} {blocker}: {report:#?}");
                    assert!(report.commands.is_empty());
                    assert!(report.publication.is_none());
                    assert_eq!(snapshot(repo.path()), before);
                    assert!(remote_tip(&remote).is_none());
                }
            }
        },
    );
}

#[cfg(unix)]
fn hook(repo: &Repository, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let hooks = repo.path().join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    repo.success(&[
        "config".as_ref(),
        "core.hooksPath".as_ref(),
        hooks.as_os_str(),
    ]);
    let path = hooks.join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(unix)]
#[test]
fn first_commit_failure_retains_hook_staging_and_does_not_publish() {
    isolated(
        "first_commit_failure_retains_hook_staging_and_does_not_publish",
        true,
        |root| {
            for action in [SaveAction::Staged, SaveAction::Wip] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                hook(
                    &repo,
                    "pre-commit",
                    "printf 'hook work\\n' > hook-file\ngit add -- hook-file\nexit 1",
                );
                let report = execute(&repo, action);
                assert!(matches!(
                    report.error,
                    Some(ExecutionError::LocalCommandFailed)
                ));
                assert_eq!(report.commands.len(), 1);
                assert!(report.publication.is_none());
                assert_eq!(observed_head(&report.before), observed_head(&report.after));
                assert_ne!(
                    report.before.index.as_ref().unwrap(),
                    report.after.index.as_ref().unwrap()
                );
                assert_eq!(output(&repo, &["show", ":hook-file"]), b"hook work\n");
                assert!(remaining(&report.commands[0].after));
                assert!(remote_tip(&remote).is_none());
            }
        },
    );
}
#[cfg(unix)]
#[test]
fn second_commit_failure_retains_first_checkpoint_and_retry_uses_actual_index() {
    isolated(
        "second_commit_failure_retains_first_checkpoint_and_retry_uses_actual_index",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            hook(
                &repo,
                "pre-commit",
                "if test -f .git/first-done; then exit 1; fi\ntouch .git/first-done",
            );
            let report = execute(&repo, SaveAction::Wip);
            assert!(matches!(
                report.error,
                Some(ExecutionError::LocalCommandFailed)
            ));
            assert_eq!(report.commands.len(), 3);
            assert!(report.publication.is_none());
            assert_eq!(tree(&repo, "HEAD", "tracked"), Some(b"selected\n".to_vec()));
            assert_eq!(output(&repo, &["show", ":tracked"]), b"remaining\n");
            assert_eq!(
                output(&repo, &["log", "--format=%s", "main..HEAD"]),
                format!("{WIP_STAGED_MESSAGE}\n").as_bytes()
            );
            assert!(remote_tip(&remote).is_none());
            fs::remove_file(repo.path().join(".git/hooks/pre-commit")).unwrap();
            let approved = plan(&repo, SaveAction::Wip);
            assert_eq!(approved.scope.tree_state(), TreeState::StagedOnly);
            let retried = execution::execute(repo.path(), &approved);
            complete(&repo, &remote, &retried);
            assert_eq!(retried.commands.len(), 1);
            // The retry records the now-selected index, not an imaginary rollback.
            assert_eq!(
                output(&repo, &["log", "--format=%s", "main..HEAD"]),
                format!("{WIP_STAGED_MESSAGE}\n{WIP_STAGED_MESSAGE}\n").as_bytes()
            );
            assert_eq!(
                tree(&repo, "HEAD", "tracked"),
                Some(b"remaining\n".to_vec())
            );
        },
    );
}
#[cfg(unix)]
#[test]
fn changed_scope_or_context_from_commit_hooks_stops_before_next_mutation() {
    isolated(
        "changed_scope_or_context_from_commit_hooks_stops_before_next_mutation",
        true,
        |root| {
            for script in [
                "printf 'unapproved\\n' > tracked",
                "printf 'new scope\\n' > new-file",
                "git config branch.work.remote other",
                "printf 'save-action = stash\\n' > .git/oneflow.conf",
                "printf '{\"save-action\":\"stash\"}' > .git/git-oneflow.config.json",
                "printf 'save-action = \"stash\"\\n' > git-oneflow.config.toml",
                "git switch -c other",
                "git config remote.origin.fetch '+refs/heads/*:refs/heads/*'",
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                hook(&repo, "post-commit", script);
                let report = execute(&repo, SaveAction::Wip);
                assert!(report.error.is_some(), "{script}: {report:#?}");
                assert_eq!(report.commands.len(), 1);
                assert!(report.commands[0].result.as_ref().unwrap().status.success());
                assert!(report.publication.is_none());
                assert!(remote_tip(&remote).is_none());
                assert_ne!(observed_head(&report.before), observed_head(&report.after));
            }
        },
    );
}
#[cfg(unix)]
#[test]
fn hook_altered_commit_selection_is_retained_but_not_published() {
    isolated(
        "hook_altered_commit_selection_is_retained_but_not_published",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            dirty(&repo, true, true);
            hook(&repo, "pre-commit", "git add -- tracked");
            let report = execute(&repo, SaveAction::Wip);
            assert!(matches!(
                report.error,
                Some(ExecutionError::Save(SaveError::ContextChanged))
            ));
            assert_eq!(report.commands.len(), 1);
            assert!(report.publication.is_none());
            assert_eq!(
                tree(&repo, "HEAD", "tracked"),
                Some(b"remaining\n".to_vec())
            );
            assert!(remote_tip(&remote).is_none());
        },
    );
}
#[cfg(unix)]
#[test]
fn rejected_publication_keeps_checkpoints_or_stash_and_clean_retry_is_empty() {
    isolated(
        "rejected_publication_keeps_checkpoints_or_stash_and_clean_retry_is_empty",
        true,
        |root| {
            for action in [SaveAction::Staged, SaveAction::Wip, SaveAction::Stash] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                hook(&repo, "pre-push", "exit 1");
                let report = execute(&repo, action);
                assert!(!report.is_complete());
                assert!(report.error.is_none());
                assert!(report.publication.as_ref().unwrap().error.is_some());
                assert!(remote_tip(&remote).is_none());
                let preserved_head = oid(&repo, "HEAD");
                let preserved_stash = report.after.stash.as_ref().unwrap().clone();
                if action == SaveAction::Stash {
                    assert!(preserved_stash.is_some());
                } else {
                    assert_ne!(observed_head(&report.before), preserved_head);
                }
                fs::remove_file(repo.path().join(".git/hooks/pre-push")).unwrap();
                // Staged leaves remaining work; publishing the actual checkpoint is explicit.
                let retried = execute(
                    &repo,
                    if action == SaveAction::Staged {
                        SaveAction::PublishOnly
                    } else {
                        action
                    },
                );
                complete(&repo, &remote, &retried);
                assert!(retried.commands.is_empty());
                assert_eq!(oid(&repo, "HEAD"), preserved_head);
                assert_eq!(retried.after.stash.as_ref().unwrap(), &preserved_stash);
            }
        },
    );
}
#[test]
fn nested_invocation_and_staged_delete_with_recreated_file_preserve_groups() {
    isolated(
        "nested_invocation_and_staged_delete_with_recreated_file_preserve_groups",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            dirty(&repo, false, false);
            repo.success(&["rm", "tracked"]);
            fs::write(repo.path().join("tracked"), "recreated\n").unwrap();
            fs::create_dir(repo.path().join("nested")).unwrap();
            let approved = save::assess(
                repo.path().join("nested"),
                &Request {
                    action: Some(SaveAction::Wip),
                    message: None,
                },
            )
            .unwrap();
            let report = execution::execute(repo.path().join("nested"), &approved);
            complete(&repo, &remote, &report);
            assert_eq!(tree(&repo, "HEAD^", "tracked"), None);
            assert_eq!(
                tree(&repo, "HEAD", "tracked"),
                Some(b"recreated\n".to_vec())
            );
        },
    );
}
#[cfg(unix)]
#[test]
fn literal_paths_symlinks_permissions_and_non_utf8_are_preserved() {
    isolated(
        "literal_paths_symlinks_permissions_and_non_utf8_are_preserved",
        true,
        |root| {
            use std::{
                ffi::OsString,
                os::unix::{
                    ffi::OsStringExt,
                    fs::{PermissionsExt, symlink},
                },
            };
            for action in [SaveAction::Wip, SaveAction::Stash] {
                let (repo, remote) = fixture(root);
                dirty(&repo, false, false);
                let names = vec![
                    OsString::from("-option"),
                    OsString::from(":(glob)*"),
                    OsString::from("line\nfile"),
                    OsString::from("quote '$`;.txt"),
                    OsString::from_vec(b"bad-\xff".to_vec()),
                ];
                for name in &names {
                    fs::write(repo.path().join(name), b"literal\0content\n").unwrap();
                }
                fs::set_permissions(
                    repo.path().join(&names[0]),
                    fs::Permissions::from_mode(0o755),
                )
                .unwrap();
                symlink("-option", repo.path().join("link")).unwrap();
                let report = execute(&repo, action);
                complete(&repo, &remote, &report);
                if action == SaveAction::Stash {
                    repo.success(&["stash", "apply", "--index"]);
                }
                for name in &names {
                    assert_eq!(
                        fs::read(repo.path().join(name)).unwrap(),
                        b"literal\0content\n"
                    );
                }
                assert_eq!(
                    fs::read_link(repo.path().join("link")).unwrap(),
                    Path::new("-option")
                );
                assert_ne!(
                    fs::metadata(repo.path().join(&names[0]))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o111,
                    0
                );
                assert_eq!(tree(&repo, "HEAD", "ignored"), None);
            }
        },
    );
}
