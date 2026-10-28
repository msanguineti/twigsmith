#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use preservation::snapshot;
use std::{
    fs,
    path::{Path, PathBuf},
};
use support::{Repository, isolated};
use twigsmith::{
    git,
    repository::{Head, Observation},
    save::{
        self, Plan, Request, SaveError, TreeState,
        execution::{self, Effect, ExecutionError, LocalObservation, Report},
        publication::{DestinationState, PublicationState},
    },
    settings::{SaveKind, SaveOperation, SavePlan, SaveScope},
};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestPlan {
    Ask,
    PublishOnly,
    Staged,
    Split,
    Stash,
}
const STAGED_MESSAGE: &str = "Save work [staged]";
const UNSTAGED_MESSAGE: &str = "Save work [unstaged]";
fn selection(action: TestPlan) -> SavePlan {
    match action {
        TestPlan::Ask => SavePlan::Ask,
        TestPlan::PublishOnly => SavePlan::PublishOnly,
        TestPlan::Staged => SavePlan::Operations(vec![SaveOperation {
            action: SaveKind::Commit,
            scope: SaveScope::Staged,
        }]),
        TestPlan::Split => SavePlan::Operations(vec![
            SaveOperation {
                action: SaveKind::Commit,
                scope: SaveScope::Staged,
            },
            SaveOperation {
                action: SaveKind::Commit,
                scope: SaveScope::Unstaged,
            },
        ]),
        TestPlan::Stash => SavePlan::Operations(vec![SaveOperation {
            action: SaveKind::Stash,
            scope: SaveScope::All,
        }]),
    }
}
fn plan(repo: &Repository, action: TestPlan) -> Plan {
    save::assess(
        repo.path(),
        &Request {
            plan: Some(selection(action)),
            message: (action == TestPlan::Staged)
                .then(|| "Developer selected work\n\nDetails".into()),
            ..Request::default()
        },
    )
    .unwrap()
}
fn execute(repo: &Repository, action: TestPlan) -> Report {
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
                let report = execute(&repo, TestPlan::PublishOnly);
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
                TestPlan::Ask,
                TestPlan::PublishOnly,
                TestPlan::Staged,
                TestPlan::Split,
                TestPlan::Stash,
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
                let report = execute(&repo, TestPlan::Staged);
                complete(&repo, &remote, &report);

                assert_eq!(
                    output(&repo, &["log", "-1", "--format=%B"]),
                    b"Developer selected work\n\nDetails [staged]\n\n"
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
            dirty(&repo, true, true);
            let approved = save::assess(repo.path(), &Request::default()).unwrap();
            let before = snapshot(repo.path());
            let report = execution::execute(repo.path(), &approved);
            assert!(matches!(
                report.error,
                Some(ExecutionError::RequiredInput(_))
            ));
            assert!(report.commands.is_empty());
            assert!(report.publication.is_none());
            assert_eq!(snapshot(repo.path()), before);
            assert!(
                save::assess(
                    repo.path(),
                    &Request {
                        plan: Some(selection(TestPlan::Split)),
                        message: Some(" ".into()),
                        ..Request::default()
                    }
                )
                .is_err()
            );
            assert!(remote_tip(&remote).is_none());
        },
    );
}
#[test]
fn split_commits_content_in_order_omitting_empty_groups() {
    isolated(
        "split_commits_content_in_order_omitting_empty_groups",
        true,
        |root| {
            for (stage, remain) in [(false, false), (true, false), (false, true), (true, true)] {
                let (repo, remote) = fixture(root);
                dirty(&repo, stage, remain);
                let report = execute(&repo, TestPlan::Split);
                complete(&repo, &remote, &report);
                let log = output(&repo, &["log", "--reverse", "--format=%s", "main..HEAD"]);
                let expected = [(stage, STAGED_MESSAGE), (remain, UNSTAGED_MESSAGE)]
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
                let retried = execute(&repo, TestPlan::Split);
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
                let report = execute(&repo, TestPlan::Stash);
                complete(&repo, &remote, &report);
                assert!(!remaining(&report.after));
                assert_eq!(observed_head(&report.before), observed_head(&report.after));
                if stage || remain {
                    assert_ne!(report.after.stash.as_ref().unwrap(), &Some(older.clone()));
                    assert!(
                        matches!(&report.effects[..], [Effect::LocalStash { commit, .. }] if Some(commit) == report.after.stash.as_ref().unwrap().as_ref())
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
                            .contains("Saved work")
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
            let report = execute(&repo, TestPlan::Staged);
            complete(&repo, &remote, &report);
            assert_eq!(tree(&repo, "HEAD", "delete"), None);
            assert_eq!(
                fs::read(repo.path().join("untracked")).unwrap(),
                b"keep local\n"
            );
            fs::remove_file(repo.path().join("untracked")).unwrap();
            fs::write(repo.path().join(".git/index.lock"), "existing lock").unwrap();
            for action in [TestPlan::Staged, TestPlan::Split, TestPlan::Stash] {
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
fn reconciliation_failure_retains_native_recovery_of_unselected_work() {
    use std::{env, os::unix::fs::PermissionsExt, process::Command};
    const TEST: &str = "reconciliation_failure_retains_native_recovery_of_unselected_work";
    isolated(TEST, true, |root| {
        if env::var_os("TWIGSMITH_EXECUTION_WRAPPER").is_none() {
            let wrapper = root.join("git-wrapper");
            fs::create_dir(&wrapper).unwrap();
            let script = wrapper.join("git");
            fs::write(&script, "#!/bin/sh\ncase \"$*\" in *' read-tree --reset -u '*) if test -f .git/fail-command; then \"$TWIGSMITH_REAL_GIT\" \"$@\" || exit $?; exit 1; fi;; esac\nexec \"$TWIGSMITH_REAL_GIT\" \"$@\"\n").unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            let real = Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap();
            let out = Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(
                    "PATH",
                    format!("{}:{}", wrapper.display(), env::var("PATH").unwrap()),
                )
                .env(
                    "TWIGSMITH_REAL_GIT",
                    String::from_utf8(real.stdout).unwrap().trim(),
                )
                .env("TWIGSMITH_EXECUTION_WRAPPER", "1")
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
        independent(&repo);
        let original_head = oid(&repo, "HEAD");
        fs::write(
            repo.path().join(".git/fail-command"),
            "fail after reconcile",
        )
        .unwrap();
        let report = run_operations(&repo, &[(SaveKind::Stash, SaveScope::Staged)]);
        assert!(
            matches!(report.error, Some(ExecutionError::LocalCommandFailed)),
            "{report:#?}"
        );
        assert!(report.publication.is_none());
        assert!(remote_tip(&remote).is_none());
        let recovery = report.recovery.as_ref().unwrap();
        assert!(recovery.retained);
        assert_eq!(oid(&repo, &recovery.reference), recovery.commit);
        let recovered = root.join("recovered");
        repo.success(&[
            "worktree".as_ref(),
            "add".as_ref(),
            "--detach".as_ref(),
            recovered.as_os_str(),
            std::str::from_utf8(&original_head).unwrap().as_ref(),
        ]);
        let restore = git::run(
            &recovered,
            [
                "stash",
                "apply",
                "--index",
                std::str::from_utf8(&recovery.commit).unwrap(),
            ],
        )
        .unwrap();
        assert!(restore.status.success(), "{restore:?}");
        assert_eq!(
            fs::read(recovered.join("tracked")).unwrap(),
            independent_text(true, true).as_bytes()
        );
        assert_eq!(
            git::run(&recovered, ["show", ":tracked"]).unwrap().stdout,
            independent_text(true, false).as_bytes()
        );
    });
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
            let report = execute(&repo, TestPlan::Split);
            assert!(matches!(
                report.error,
                Some(ExecutionError::LocalCommandFailed)
            ));

            assert!(report.effects.is_empty());
            assert!(report.recovery.as_ref().unwrap().retained);
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
                    plan: Some(selection(TestPlan::Split)),
                    message: None,
                    ..Request::default()
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
        if env::var_os("TWIGSMITH_CUSTOM_INDEX").is_none() {
            let out = Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env("GIT_INDEX_FILE", root.join("selected.index"))
                .env("TWIGSMITH_CUSTOM_INDEX", "1")
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
        let report = execute(&repo, TestPlan::Split);
        assert!(matches!(report.error, Some(ExecutionError::IndexLocked)));
        assert!(report.commands.is_empty());
        assert_eq!(
            report.before.index.as_ref().unwrap().as_ref().unwrap(),
            &fs::read(&index_path).unwrap()
        );
        assert!(!repo.path().join(".git/index").exists());
        fs::remove_file(lock_path).unwrap();
        let report = execute(&repo, TestPlan::Split);
        complete(&repo, &remote, &report);
        assert_eq!(
            report.after.index.as_ref().unwrap().as_ref().unwrap(),
            &fs::read(index_path).unwrap()
        );
        assert!(!repo.path().join(".git/index").exists());
    });
}

#[test]
fn clean_submodule_revisions_are_separate_checkpoints() {
    isolated(
        "clean_submodule_revisions_are_separate_checkpoints",
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
            let before = snapshot(repo.path());
            let refused = run_operations(&repo, &[(SaveKind::Stash, SaveScope::All)]);
            assert!(matches!(
                refused.error,
                Some(ExecutionError::Save(SaveError::InvalidPlan(_)))
            ));
            assert!(refused.commands.is_empty());
            assert!(refused.recovery.is_none());
            assert!(refused.publication.is_none());
            assert_eq!(snapshot(repo.path()), before);
            let report = execute(&repo, TestPlan::Split);
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
        if env::var_os("TWIGSMITH_FIXED_DATE").is_none() {
            let out = Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2001-01-01T00:00:00Z")
                .env("TWIGSMITH_FIXED_DATE", "1")
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
        let first = execute(&repo, TestPlan::Stash);
        complete(&repo, &remote, &first);
        let older = oid(&repo, "refs/stash");
        dirty(&repo, true, true);
        let report = execute(&repo, TestPlan::Stash);
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
                let approved = plan(&repo, TestPlan::Split);
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
                        repo.path().join(".git/twigsmith.config.toml"),
                        "save-plan = 'stash'\n",
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
            for action in [TestPlan::Staged, TestPlan::Split, TestPlan::Stash] {
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
            for action in [TestPlan::Staged, TestPlan::Split] {
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
            let report = execute(&repo, TestPlan::Split);
            assert!(matches!(
                report.error,
                Some(ExecutionError::LocalCommandFailed)
            ));

            assert!(report.publication.is_none());
            assert_eq!(tree(&repo, "HEAD", "tracked"), Some(b"selected\n".to_vec()));
            assert_eq!(output(&repo, &["show", ":tracked"]), b"remaining\n");
            assert_eq!(
                output(&repo, &["log", "--format=%s", "main..HEAD"]),
                format!("{STAGED_MESSAGE}\n").as_bytes()
            );
            assert!(remote_tip(&remote).is_none());
            fs::remove_file(repo.path().join(".git/hooks/pre-commit")).unwrap();
            let approved = plan(&repo, TestPlan::Split);
            assert_eq!(approved.scope.tree_state(), TreeState::StagedOnly);
            let retried = execution::execute(repo.path(), &approved);
            complete(&repo, &remote, &retried);

            // The retry records the now-selected index, not an imaginary rollback.
            assert_eq!(
                output(&repo, &["log", "--format=%s", "main..HEAD"]),
                format!("{STAGED_MESSAGE}\n{STAGED_MESSAGE}\n").as_bytes()
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
                "printf 'save-action = \"stash\"\\n' > .git/twigsmith.config.toml",
                "printf '{\"save-plan\":\"stash\"}' > .git/twigsmith.config.json",
                "printf 'save-action = \"stash\"\\n' > twigsmith.config.toml",
                "git switch -c other",
                "git config remote.origin.fetch '+refs/heads/*:refs/heads/*'",
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                hook(&repo, "post-commit", script);
                let report = execute(&repo, TestPlan::Split);
                assert!(report.error.is_some(), "{script}: {report:#?}");

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
            let report = execute(&repo, TestPlan::Split);
            assert!(matches!(
                report.error,
                Some(ExecutionError::Save(SaveError::ContextChanged))
            ));

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
            for action in [TestPlan::Staged, TestPlan::Split, TestPlan::Stash] {
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
                if action == TestPlan::Stash {
                    assert!(preserved_stash.is_some());
                } else {
                    assert_ne!(observed_head(&report.before), preserved_head);
                }
                fs::remove_file(repo.path().join(".git/hooks/pre-push")).unwrap();
                // Staged leaves remaining work; publishing the actual checkpoint is explicit.
                let retried = execute(
                    &repo,
                    if action == TestPlan::Staged {
                        TestPlan::PublishOnly
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
                    plan: Some(selection(TestPlan::Split)),
                    message: None,
                    ..Request::default()
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
            for action in [TestPlan::Split, TestPlan::Stash] {
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
                if action == TestPlan::Stash {
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

fn independent_text(staged: bool, unstaged: bool) -> String {
    let mut lines = (0..20).map(|n| format!("line {n}\n")).collect::<Vec<_>>();
    if staged {
        lines[2] = "selected staged change\n".into();
    }
    if unstaged {
        lines[17] = "selected unstaged change\n".into();
    }
    lines.concat()
}
fn independent(repo: &Repository) {
    fs::write(repo.path().join("tracked"), independent_text(false, false)).unwrap();
    repo.success(&["commit", "-am", "independent base"]);
    fs::write(repo.path().join("tracked"), independent_text(true, false)).unwrap();
    repo.success(&["add", "tracked"]);
    fs::write(repo.path().join("tracked"), independent_text(true, true)).unwrap();
    fs::write(repo.path().join("ignored"), "ignored local\n").unwrap();
}
fn run_operations(repo: &Repository, operations: &[(SaveKind, SaveScope)]) -> Report {
    let requested = Request {
        plan: Some(SavePlan::Operations(
            operations
                .iter()
                .map(|(action, scope)| SaveOperation {
                    action: *action,
                    scope: *scope,
                })
                .collect(),
        )),
        ..Request::default()
    };
    let approved = save::assess(repo.path(), &requested).unwrap();
    execution::execute(repo.path(), &approved)
}
#[test]
fn independent_partial_file_obeys_every_composable_assignment_and_order() {
    use SaveKind::{Commit as C, Stash as S};
    use SaveScope::{All as A, Staged as I, Unstaged as U};
    isolated(
        "independent_partial_file_obeys_every_composable_assignment_and_order",
        true,
        |root| {
            let plans = [
                vec![(C, A)],
                vec![(C, I), (C, U)],
                vec![(C, U), (C, I)],
                vec![(C, I), (S, U)],
                vec![(S, U), (C, I)],
                vec![(S, I), (C, U)],
                vec![(C, U), (S, I)],
                vec![(S, A)],
                vec![(S, I), (S, U)],
                vec![(S, U), (S, I)],
                vec![(C, I)],
                vec![(C, U)],
                vec![(S, I)],
                vec![(S, U)],
            ];
            for operations in plans {
                let (repo, remote) = fixture(root);
                independent(&repo);
                let report = run_operations(&repo, &operations);
                complete(&repo, &remote, &report);
                let commits_staged = operations
                    .iter()
                    .any(|(kind, scope)| *kind == C && *scope != U);
                let commits_unstaged = operations
                    .iter()
                    .any(|(kind, scope)| *kind == C && *scope != I);
                let leaves_staged = !operations.iter().any(|(_, scope)| *scope != U);
                let leaves_unstaged = !operations.iter().any(|(_, scope)| *scope != I);
                assert_eq!(
                    tree(&repo, "HEAD", "tracked").unwrap(),
                    independent_text(commits_staged, commits_unstaged).as_bytes(),
                    "{operations:?}"
                );
                assert_eq!(
                    output(&repo, &["show", ":tracked"]),
                    independent_text(commits_staged || leaves_staged, commits_unstaged).as_bytes(),
                    "{operations:?}"
                );
                assert_eq!(
                    fs::read(repo.path().join("tracked")).unwrap(),
                    independent_text(
                        commits_staged || leaves_staged,
                        commits_unstaged || leaves_unstaged
                    )
                    .as_bytes(),
                    "{operations:?}"
                );
                let requested_labels = operations
                    .iter()
                    .map(|(kind, scope)| match scope {
                        A => {
                            if *kind == C {
                                "Save work".into()
                            } else {
                                "Saved work".into()
                            }
                        }
                        scope => format!(
                            "{} [{}]",
                            if *kind == C {
                                "Save work"
                            } else {
                                "Saved work"
                            },
                            scope.as_str()
                        ),
                    })
                    .collect::<Vec<String>>();
                let actual_labels = report
                    .effects
                    .iter()
                    .map(|effect| match effect {
                        Effect::Checkpoint { message, .. } | Effect::LocalStash { message, .. } => {
                            message.clone()
                        }
                    })
                    .collect::<Vec<_>>();
                assert_eq!(actual_labels, requested_labels);
                assert!(!report.recovery.as_ref().unwrap().retained);
                let recoveries = output(&repo, &["for-each-ref", "refs/twigsmith/save-recovery/"]);
                assert!(recoveries.is_empty());
            }
        },
    );
}
#[test]
fn split_stashes_contain_only_requested_delta_and_native_recovery_preserves_membership() {
    isolated(
        "split_stashes_contain_only_requested_delta_and_native_recovery_preserves_membership",
        true,
        |root| {
            for order in [
                [SaveScope::Staged, SaveScope::Unstaged],
                [SaveScope::Unstaged, SaveScope::Staged],
            ] {
                let (repo, remote) = fixture(root);
                independent(&repo);
                fs::write(repo.path().join("new"), "new addition\n").unwrap();
                let report = run_operations(&repo, &order.map(|scope| (SaveKind::Stash, scope)));
                complete(&repo, &remote, &report);
                let mut staged = None;
                let mut unstaged = None;
                for effect in &report.effects {
                    let Effect::LocalStash { commit, message } = effect else {
                        continue;
                    };
                    let revision = std::str::from_utf8(commit).unwrap();
                    if message.ends_with("[staged]") {
                        staged = Some(revision.to_owned());
                        assert_eq!(
                            tree(&repo, &format!("{revision}^1"), "tracked").unwrap(),
                            independent_text(false, false).as_bytes()
                        );
                        assert_eq!(
                            tree(&repo, revision, "tracked").unwrap(),
                            independent_text(true, false).as_bytes()
                        );
                        assert!(tree(&repo, revision, "new").is_none());
                    } else {
                        unstaged = Some(revision.to_owned());
                        assert_eq!(
                            tree(&repo, &format!("{revision}^1"), "tracked").unwrap(),
                            independent_text(true, false).as_bytes()
                        );
                        assert_eq!(
                            tree(&repo, &format!("{revision}^2"), "tracked").unwrap(),
                            independent_text(true, false).as_bytes()
                        );
                        assert_eq!(
                            tree(&repo, revision, "tracked").unwrap(),
                            independent_text(true, true).as_bytes()
                        );
                        assert_eq!(
                            tree(&repo, &format!("{revision}^3"), "new").unwrap(),
                            b"new addition\n"
                        );
                    }
                }
                repo.success(&["stash", "apply", "--index", &staged.unwrap()]);
                repo.success(&["stash", "apply", "--index", &unstaged.unwrap()]);
                assert_eq!(
                    output(&repo, &["show", ":tracked"]),
                    independent_text(true, false).as_bytes()
                );
                assert_eq!(
                    fs::read(repo.path().join("tracked")).unwrap(),
                    independent_text(true, true).as_bytes()
                );
                assert_eq!(
                    output(&repo, &["ls-files", "--others", "--exclude-standard"]),
                    b"new\n"
                );
            }
        },
    );
}
#[test]
fn dependent_groups_refuse_before_effects_only_when_independent_tree_is_needed() {
    isolated(
        "dependent_groups_refuse_before_effects_only_when_independent_tree_is_needed",
        true,
        |root| {
            for operations in [
                vec![(SaveKind::Commit, SaveScope::Unstaged)],
                vec![(SaveKind::Stash, SaveScope::Staged)],
                vec![
                    (SaveKind::Stash, SaveScope::Staged),
                    (SaveKind::Commit, SaveScope::Unstaged),
                ],
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let head = oid(&repo, "HEAD");
                let index = fs::read(repo.path().join(".git/index")).unwrap();
                let work = fs::read(repo.path().join("tracked")).unwrap();
                let report = run_operations(&repo, &operations);
                assert!(
                    matches!(
                        report.error,
                        Some(ExecutionError::Save(SaveError::Inseparable))
                    ),
                    "{report:#?}"
                );
                assert!(report.commands.is_empty());
                assert!(report.recovery.is_none());
                assert!(report.publication.is_none());
                assert_eq!(oid(&repo, "HEAD"), head);
                assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
                assert_eq!(fs::read(repo.path().join("tracked")).unwrap(), work);
                assert!(remote_tip(&remote).is_none());
            }
            for operations in [
                vec![
                    (SaveKind::Commit, SaveScope::Staged),
                    (SaveKind::Commit, SaveScope::Unstaged),
                ],
                vec![
                    (SaveKind::Stash, SaveScope::Staged),
                    (SaveKind::Stash, SaveScope::Unstaged),
                ],
            ] {
                let (repo, remote) = fixture(root);
                dirty(&repo, true, true);
                let report = run_operations(&repo, &operations);
                complete(&repo, &remote, &report);
            }
        },
    );
}

#[test]
fn alternate_index_does_not_hide_same_size_same_timestamp_work() {
    use std::time::{Duration, SystemTime};
    isolated(
        "alternate_index_does_not_hide_same_size_same_timestamp_work",
        true,
        |root| {
            for kind in [SaveKind::Commit, SaveKind::Stash] {
                let (repo, remote) = fixture(root);
                fs::write(repo.path().join("tracked"), "next\n").unwrap();
                fs::write(repo.path().join("ignored"), "ignored local\n").unwrap();
                let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
                for path in [repo.path().join("tracked"), repo.path().join(".git/index")] {
                    fs::File::options()
                        .write(true)
                        .open(path)
                        .unwrap()
                        .set_times(fs::FileTimes::new().set_modified(timestamp))
                        .unwrap();
                }
                let report = run_operations(&repo, &[(kind, SaveScope::Unstaged)]);
                complete(&repo, &remote, &report);
                match kind {
                    SaveKind::Commit => {
                        assert_eq!(tree(&repo, "HEAD", "tracked").unwrap(), b"next\n")
                    }
                    SaveKind::Stash => {
                        assert_eq!(tree(&repo, "refs/stash", "tracked").unwrap(), b"next\n");
                        repo.success(&["stash", "apply", "--index"]);
                        assert_eq!(fs::read(repo.path().join("tracked")).unwrap(), b"next\n");
                    }
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn group_derivation_ignores_custom_merge_drivers_and_refuses_dependent_content() {
    isolated(
        "group_derivation_ignores_custom_merge_drivers_and_refuses_dependent_content",
        true,
        |root| {
            for dependent in [false, true] {
                let (repo, remote) = fixture(root);
                if dependent {
                    dirty(&repo, true, true);
                } else {
                    independent(&repo);
                }
                fs::create_dir_all(repo.path().join(".git/info")).unwrap();
                fs::write(
                    repo.path().join(".git/info/attributes"),
                    "tracked merge=trap\n",
                )
                .unwrap();
                repo.success(&[
                    "config",
                    "merge.trap.driver",
                    "touch .git/driver-ran; cp %B %A",
                ]);
                repo.success(&["config", "merge.default", "trap"]);
                let report = run_operations(&repo, &[(SaveKind::Commit, SaveScope::Unstaged)]);
                assert!(!repo.path().join(".git/driver-ran").exists());
                if dependent {
                    assert!(
                        matches!(
                            report.error,
                            Some(ExecutionError::Save(SaveError::Inseparable))
                        ),
                        "{report:#?}"
                    );
                    assert!(report.commands.is_empty());
                    assert!(report.recovery.is_none());
                    assert!(report.publication.is_none());
                } else {
                    complete(&repo, &remote, &report);
                    assert_eq!(
                        tree(&repo, "HEAD", "tracked").unwrap(),
                        independent_text(false, true).as_bytes()
                    );
                }
            }
        },
    );
}

#[test]
fn group_derivation_uses_selected_sha256_object_store() {
    use std::{env, process::Command};
    const TEST: &str = "group_derivation_uses_selected_sha256_object_store";
    isolated(TEST, true, |root| {
        if env::var_os("TWIGSMITH_SHA256_TEST").is_none() {
            let output = Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env("GIT_DEFAULT_HASH", "sha256")
                .env("TWIGSMITH_SHA256_TEST", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let (repo, remote) = fixture(root);
        independent(&repo);
        assert_eq!(oid(&repo, "HEAD").len(), 64);
        let report = run_operations(
            &repo,
            &[
                (SaveKind::Stash, SaveScope::Staged),
                (SaveKind::Commit, SaveScope::Unstaged),
            ],
        );
        complete(&repo, &remote, &report);
        assert_eq!(
            tree(&repo, "HEAD", "tracked").unwrap(),
            independent_text(false, true).as_bytes()
        );
    });
}

#[test]
fn ignored_obstructions_refuse_before_preservation_without_losing_literal_nested_paths() {
    isolated(
        "ignored_obstructions_refuse_before_preservation_without_losing_literal_nested_paths",
        true,
        |root| {
            for shape in ["directory", "same-path", "ancestor"] {
                let scopes = if shape == "directory" {
                    [SaveScope::All, SaveScope::Unstaged]
                } else {
                    [SaveScope::All, SaveScope::Staged]
                };
                for scope in scopes {
                    let (repo, remote) = fixture(root);
                    let path = if shape == "ancestor" {
                        "nested literal/parent-obstruction/secret :(glob)*\nname"
                    } else {
                        "nested literal/secret :(glob)*\nname"
                    };
                    let full = repo.path().join(path);
                    fs::create_dir_all(full.parent().unwrap()).unwrap();
                    fs::write(&full, "tracked original\n").unwrap();
                    repo.success(&["--literal-pathspecs", "add", "--", path]);
                    repo.success(&["commit", "-m", "tracked literal path"]);
                    fs::create_dir_all(repo.path().join(".git/info")).unwrap();
                    fs::write(
                        repo.path().join(".git/info/exclude"),
                        "*.ignored\nsecret*\nparent-obstruction\n",
                    )
                    .unwrap();
                    let ignored = match shape {
                        "directory" => {
                            fs::remove_file(&full).unwrap();
                            fs::create_dir_all(full.join("deep ignored directory")).unwrap();
                            let ignored = full.join("deep ignored directory/private.ignored");
                            fs::write(&ignored, "only local ignored contents\n").unwrap();
                            ignored
                        }
                        "same-path" => {
                            repo.success(&["--literal-pathspecs", "rm", "--", path]);
                            fs::create_dir_all(full.parent().unwrap()).unwrap();
                            fs::write(&full, "only local ignored contents\n").unwrap();
                            full.clone()
                        }
                        "ancestor" => {
                            repo.success(&["--literal-pathspecs", "rm", "--", path]);
                            let ignored = full.parent().unwrap().to_path_buf();
                            if ignored.exists() {
                                fs::remove_dir(&ignored).unwrap();
                            }
                            fs::create_dir_all(ignored.parent().unwrap()).unwrap();
                            fs::write(&ignored, "only local ignored contents\n").unwrap();
                            ignored
                        }
                        _ => unreachable!(),
                    };
                    let head = oid(&repo, "HEAD");
                    let index = fs::read(repo.path().join(".git/index")).unwrap();
                    let status = output(&repo, &["status", "--porcelain=v2", "-z", "--ignored"]);
                    let report = run_operations(&repo, &[(SaveKind::Stash, scope)]);
                    assert!(
                        matches!(&report.error, Some(ExecutionError::Save(SaveError::InvalidPlan(message))) if message.contains("ignored untracked work")),
                        "{shape} {scope:?}: {report:#?}"
                    );
                    assert!(report.commands.is_empty());
                    assert!(report.effects.is_empty());
                    assert!(report.recovery.is_none());
                    assert!(report.publication.is_none());
                    assert_eq!(oid(&repo, "HEAD"), head);
                    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
                    assert_eq!(
                        output(&repo, &["status", "--porcelain=v2", "-z", "--ignored"]),
                        status
                    );
                    assert_eq!(
                        fs::read(&ignored).unwrap(),
                        b"only local ignored contents\n"
                    );
                    assert!(report.after.stash.as_ref().unwrap().is_none());
                    assert!(remote_tip(&remote).is_none());
                }
            }
        },
    );
}

#[test]
fn tracked_paths_matching_ignores_and_ignored_siblings_remain_usable() {
    isolated(
        "tracked_paths_matching_ignores_and_ignored_siblings_remain_usable",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            fs::create_dir(repo.path().join("nested")).unwrap();
            fs::write(repo.path().join("nested/tracked.ignored"), "tracked base\n").unwrap();
            repo.success(&["add", "nested/tracked.ignored"]);
            repo.success(&["commit", "-m", "tracked ignored-name base"]);
            fs::create_dir_all(repo.path().join(".git/info")).unwrap();
            fs::write(repo.path().join(".git/info/exclude"), "*.ignored\n").unwrap();
            fs::write(
                repo.path().join("nested/tracked.ignored"),
                "selected tracked work\n",
            )
            .unwrap();
            fs::write(
                repo.path().join("nested/sibling.ignored"),
                "unselected ignored work\n",
            )
            .unwrap();
            fs::write(repo.path().join("ignored"), "ignored local\n").unwrap();
            let report = run_operations(&repo, &[(SaveKind::Stash, SaveScope::Unstaged)]);
            complete(&repo, &remote, &report);
            assert_eq!(
                fs::read(repo.path().join("nested/tracked.ignored")).unwrap(),
                b"tracked base\n"
            );
            assert_eq!(
                fs::read(repo.path().join("nested/sibling.ignored")).unwrap(),
                b"unselected ignored work\n"
            );
            repo.success(&["stash", "apply", "--index"]);
            assert_eq!(
                fs::read(repo.path().join("nested/tracked.ignored")).unwrap(),
                b"selected tracked work\n"
            );
            assert_eq!(
                fs::read(repo.path().join("nested/sibling.ignored")).unwrap(),
                b"unselected ignored work\n"
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn ignored_obstructions_created_by_index_hook_stop_before_destructive_reconciliation() {
    isolated(
        "ignored_obstructions_created_by_index_hook_stop_before_destructive_reconciliation",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            fs::remove_file(repo.path().join("tracked")).unwrap();
            fs::create_dir(repo.path().join("tracked")).unwrap();
            fs::create_dir_all(repo.path().join(".git/info")).unwrap();
            fs::write(repo.path().join(".git/info/exclude"), "*.ignored\n").unwrap();
            hook(
                &repo,
                "post-index-change",
                "if ! git ls-files --error-unmatch -- tracked >/dev/null 2>&1; then mkdir -p tracked; printf 'hook ignored contents\\n' > tracked/hook.ignored; fi",
            );
            let report = run_operations(&repo, &[(SaveKind::Stash, SaveScope::Unstaged)]);
            assert!(
                matches!(&report.error, Some(ExecutionError::Save(SaveError::InvalidPlan(message))) if message.contains("ignored untracked work")),
                "{report:#?}"
            );
            assert!(report.recovery.as_ref().unwrap().retained);
            assert!(matches!(&report.effects[..], [Effect::LocalStash { .. }]));
            assert!(!report.commands.iter().any(|command| matches!(
                command.operation,
                execution::Operation::Reconcile { .. }
            )));
            assert!(report.publication.is_none());
            assert!(remote_tip(&remote).is_none());
            assert_eq!(
                fs::read(repo.path().join("tracked/hook.ignored")).unwrap(),
                b"hook ignored contents\n"
            );
        },
    );
}
