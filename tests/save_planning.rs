#[path = "support/snapshot.rs"]
mod preservation;
mod support;
use git_oneflow::{
    git,
    save::{self, LocalStep, Plan, Request, RequiredInput, SaveError, TreeState},
    settings::SaveAction,
};
use preservation::snapshot;
use std::{fs, path::Path};
use support::{Repository, isolated};

fn fixture(root: &Path) -> Repository {
    let repo = Repository::new(root);
    fs::write(repo.path().join("tracked"), "base\n").unwrap();
    fs::write(repo.path().join(".gitignore"), "ignored\n").unwrap();
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", "base"]);
    repo.success(&["switch", "-c", "Feature/Exact_Name"]);
    // Intentionally inaccessible: assessment must need no available server/base.
    repo.success(&["remote", "add", "origin", "/missing/save-remote"]);
    repo
}
fn request(action: SaveAction) -> Request {
    Request {
        action: Some(action),
        message: None,
    }
}
fn assess(root: &Path, path: &Path, request: &Request) -> Result<Plan, SaveError> {
    let before = snapshot(root);
    let result = save::assess(path, request);
    let after = snapshot(root);
    assert!(
        after == before,
        "assessment changed paths: {:?}",
        after
            .keys()
            .chain(before.keys())
            .filter(|p| after.get(*p) != before.get(*p))
            .collect::<Vec<_>>()
    );
    result
}
fn revalidate(root: &Path, path: &Path, plan: &Plan) -> Result<(), SaveError> {
    let before = snapshot(root);
    let result = save::revalidate(path, plan);
    assert_eq!(
        snapshot(root),
        before,
        "revalidation changed repository state"
    );
    result
}
fn remaining(repo: &Repository) {
    fs::write(repo.path().join("tracked"), "remaining\n").unwrap();
}
fn staged(repo: &Repository) {
    remaining(repo);
    repo.success(&["add", "tracked"]);
}

#[test]
fn four_tree_states_and_checkpoint_groups() {
    isolated("four_tree_states_and_checkpoint_groups", true, |root| {
        for (stage, remain, expected) in [
            (false, false, TreeState::Clean),
            (true, false, TreeState::StagedOnly),
            (false, true, TreeState::UnstagedOnly),
            (true, true, TreeState::Mixed),
        ] {
            let repo = fixture(root);
            if stage {
                staged(&repo);
            }
            if remain {
                fs::write(repo.path().join("tracked"), "remaining again\n").unwrap();
            }
            let plan = assess(root, repo.path(), &request(SaveAction::Wip)).unwrap();
            assert_eq!(plan.scope.tree_state(), expected);
            assert_eq!(plan.scope.staged.len(), usize::from(stage));
            assert_eq!(plan.scope.remaining_tracked.len(), usize::from(remain));
            let mut steps = vec![];
            if stage {
                steps.push(LocalStep::CommitStagedWip);
            }
            if remain {
                steps.push(LocalStep::StageAndCommitRemainingWip);
            }
            assert_eq!(plan.local_steps, steps);
            assert_eq!(plan.required_input, RequiredInput::None);
            assert_eq!(
                plan.publication.as_ref().unwrap().branch_ref,
                b"refs/heads/Feature/Exact_Name"
            );
            assert_eq!(plan.publication.as_ref().unwrap().remote, "origin");
            assert!(plan.publication.as_ref().unwrap().establish_upstream);
            revalidate(root, repo.path(), &plan).unwrap();
            if stage && remain {
                assert_eq!(plan.scope.staged, plan.scope.remaining_tracked);
            }
        }
    });
}

#[test]
fn clean_always_publishes_without_checkpoint_or_message() {
    isolated(
        "clean_always_publishes_without_checkpoint_or_message",
        true,
        |root| {
            let repo = fixture(root);
            fs::write(repo.path().join("ignored"), "local ignored work").unwrap();
            for action in [
                SaveAction::Ask,
                SaveAction::PublishOnly,
                SaveAction::Staged,
                SaveAction::Wip,
                SaveAction::Stash,
            ] {
                fs::write(
                    repo.path().join(".git/oneflow.conf"),
                    format!(
                        "save-action = {}",
                        match action {
                            SaveAction::Ask => "ask",
                            SaveAction::PublishOnly => "publish-only",
                            SaveAction::Staged => "staged",
                            SaveAction::Wip => "wip",
                            SaveAction::Stash => "stash",
                        }
                    ),
                )
                .unwrap();
                for req in [Request::default(), request(action)] {
                    let plan = assess(root, repo.path(), &req).unwrap();
                    assert_eq!(plan.action, SaveAction::PublishOnly);
                    assert_eq!(plan.required_input, RequiredInput::None);
                    assert!(plan.local_steps.is_empty());
                    assert_eq!(plan.applicable_actions, [SaveAction::PublishOnly]);
                    assert!(plan.publication.is_some());
                    assert_eq!(plan.scope.tree_state(), TreeState::Clean);
                }
            }
            // Clean staged selection does not need a message, even a supplied blank.
            let req = Request {
                action: Some(SaveAction::Staged),
                message: Some(" ".into()),
            };
            assert!(
                assess(root, repo.path(), &req)
                    .unwrap()
                    .local_steps
                    .is_empty()
            );
        },
    );
}

#[test]
fn input_preferences_and_effects() {
    isolated("input_preferences_and_effects", true, |root| {
        let repo = fixture(root);
        remaining(&repo);
        let ask = assess(root, repo.path(), &Request::default()).unwrap();
        assert_eq!(ask.required_input, RequiredInput::Action);
        assert!(ask.publication.is_none());
        assert!(ask.local_steps.is_empty());
        assert_eq!(
            ask.applicable_actions,
            [SaveAction::PublishOnly, SaveAction::Wip, SaveAction::Stash]
        );
        assert!(matches!(
            assess(root, repo.path(), &request(SaveAction::Staged)),
            Err(SaveError::NoStagedChanges)
        ));
        fs::write(repo.path().join(".git/oneflow.conf"), "save-action=stash").unwrap();
        let stash = assess(root, repo.path(), &Request::default()).unwrap();
        assert_eq!(stash.local_steps, [LocalStep::StashIncludingUntracked]);
        assert!(stash.publication.is_some()); // Stash and branch publication are separate.
        let publish = assess(root, repo.path(), &request(SaveAction::PublishOnly)).unwrap();
        assert!(publish.local_steps.is_empty());
        assert_eq!(publish.scope.remaining_tracked, [b"tracked".to_vec()]);
        staged(&repo);
        let missing = assess(root, repo.path(), &request(SaveAction::Staged)).unwrap();
        assert_eq!(missing.required_input, RequiredInput::Message);
        assert!(missing.local_steps.is_empty());
        assert!(missing.publication.is_none());
        for message in ["", " \n\t", "\u{2003}"] {
            assert!(matches!(
                assess(
                    root,
                    repo.path(),
                    &Request {
                        action: Some(SaveAction::Staged),
                        message: Some(message.into())
                    }
                ),
                Err(SaveError::BlankMessage)
            ));
        }
        let message = "Developer's exact message\n\nBody";
        let req = Request {
            action: Some(SaveAction::Staged),
            message: Some(message.into()),
        };
        assert_eq!(
            assess(root, repo.path(), &req).unwrap().local_steps,
            [LocalStep::CommitIndex {
                message: message.into()
            }]
        );
        for action in [
            SaveAction::Ask,
            SaveAction::PublishOnly,
            SaveAction::Wip,
            SaveAction::Stash,
        ] {
            assert!(matches!(
                assess(
                    root,
                    repo.path(),
                    &Request {
                        action: Some(action),
                        message: Some("message".into())
                    }
                ),
                Err(SaveError::MessageNotApplicable)
            ));
        }
        fs::write(repo.path().join(".git/oneflow.conf"), "save-action=nope").unwrap();
        let err = assess(root, repo.path(), &request(SaveAction::PublishOnly)).unwrap_err();
        assert!(err.to_string().contains("save-action must be"));
    });
}

#[test]
fn untracked_ignored_partial_staging_and_deletions() {
    isolated(
        "untracked_ignored_partial_staging_and_deletions",
        true,
        |root| {
            let repo = fixture(root);
            staged(&repo);
            remaining(&repo);
            fs::write(repo.path().join("tracked"), "partial\n").unwrap();
            fs::write(repo.path().join("new\nname"), "untracked\n").unwrap();
            fs::write(repo.path().join("ignored"), "ignored\n").unwrap();
            fs::create_dir(repo.path().join("dir")).unwrap();
            fs::write(repo.path().join("dir/file"), "new").unwrap();
            let plan = assess(root, repo.path(), &request(SaveAction::Wip)).unwrap();
            assert_eq!(plan.scope.staged, [b"tracked".to_vec()]);
            assert_eq!(plan.scope.remaining_tracked, [b"tracked".to_vec()]);
            assert_eq!(
                plan.scope.untracked,
                [b"dir/file".to_vec(), b"new\nname".to_vec()]
            );
            assert_eq!(
                plan.local_steps,
                [
                    LocalStep::CommitStagedWip,
                    LocalStep::StageAndCommitRemainingWip
                ]
            );
            fs::remove_file(repo.path().join("tracked")).unwrap();
            let deleted = assess(root, repo.path(), &request(SaveAction::Wip)).unwrap();
            assert_eq!(deleted.scope.tree_state(), TreeState::Mixed);
            assert!(matches!(
                revalidate(root, repo.path(), &plan),
                Err(SaveError::ContextChanged)
            ));
            repo.success(&["add", "-u"]);
            assert_eq!(
                assess(root, repo.path(), &request(SaveAction::Staged))
                    .unwrap()
                    .required_input,
                RequiredInput::Message
            );
        },
    );
}

#[test]
fn upstream_absent_exact_incomplete_conflicting_and_ambiguous() {
    isolated(
        "upstream_absent_exact_incomplete_conflicting_and_ambiguous",
        true,
        |root| {
            let branch = "Feature/Exact_Name";
            for (remotes, merges, valid) in [
                (vec![], vec![], true),
                (vec!["origin"], vec!["refs/heads/Feature/Exact_Name"], true),
                (vec!["origin"], vec![], false),
                (vec![], vec!["refs/heads/Feature/Exact_Name"], false),
                (vec!["other"], vec!["refs/heads/Feature/Exact_Name"], false),
                (vec!["."], vec!["refs/heads/Feature/Exact_Name"], false),
                (vec!["origin"], vec!["refs/heads/other"], false),
                (
                    vec!["origin", "origin"],
                    vec!["refs/heads/Feature/Exact_Name"],
                    false,
                ),
                (
                    vec!["other", "origin"],
                    vec!["refs/heads/Feature/Exact_Name"],
                    false,
                ),
                (
                    vec!["origin"],
                    vec![
                        "refs/heads/Feature/Exact_Name",
                        "refs/heads/Feature/Exact_Name",
                    ],
                    false,
                ),
                (
                    vec!["origin"],
                    vec!["refs/heads/Feature/Exact_Name", "refs/heads/other"],
                    false,
                ),
                (vec![""], vec!["refs/heads/Feature/Exact_Name"], false),
                (vec!["origin"], vec![""], false),
            ] {
                let repo = fixture(root);
                for remote in &remotes {
                    repo.success(&[
                        "config",
                        "--add",
                        &format!("branch.{branch}.remote"),
                        remote,
                    ]);
                }
                for merge in &merges {
                    repo.success(&["config", "--add", &format!("branch.{branch}.merge"), merge]);
                }
                // Tracking ref need not exist, nor need fetch refmaps cover the branch.
                repo.success(&[
                    "config",
                    "--replace-all",
                    "remote.origin.fetch",
                    "+refs/heads/trunk:refs/remotes/origin/trunk",
                ]);
                let result = assess(root, repo.path(), &Request::default());
                if valid {
                    assert_eq!(
                        result.unwrap().publication.unwrap().establish_upstream,
                        remotes.is_empty()
                    );
                } else {
                    assert!(
                        matches!(result, Err(SaveError::InvalidUpstream)),
                        "{remotes:?} {merges:?}: {result:?}"
                    );
                }
            }
            let repo = fixture(root);
            repo.success(&["remote", "remove", "origin"]);
            assert!(matches!(
                assess(root, repo.path(), &Request::default()),
                Err(SaveError::InvalidRemote { .. })
            ));
        },
    );
}

#[test]
fn branch_repository_and_operation_guards() {
    isolated("branch_repository_and_operation_guards", true, |root| {
        assert!(matches!(
            assess(root, root, &Request::default()),
            Err(SaveError::OutsideRepository)
        ));
        let unborn = Repository::new(root);
        assert!(matches!(
            assess(root, unborn.path(), &Request::default()),
            Err(SaveError::UnbornBranch)
        ));
        let repo = fixture(root);
        repo.success(&["switch", "main"]);
        assert!(matches!(
            assess(root, repo.path(), &Request::default()),
            Err(SaveError::ProtectedBase)
        ));
        repo.success(&["switch", "--detach"]);
        assert!(matches!(
            assess(root, repo.path(), &Request::default()),
            Err(SaveError::DetachedHead)
        ));
        let bare = root.join("bare.git");
        repo.success(&["init".as_ref(), "--bare".as_ref(), bare.as_os_str()]);
        assert!(matches!(
            assess(root, &bare, &Request::default()),
            Err(SaveError::BareRepository)
        ));
        for marker in [
            "MERGE_HEAD",
            "rebase-merge",
            "rebase-apply/rebasing",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-apply/applying",
            "BISECT_START",
            "sequencer/todo",
        ] {
            let repo = fixture(root);
            let path = repo.path().join(".git").join(marker);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if marker == "rebase-merge" {
                fs::create_dir(&path).unwrap();
            } else {
                fs::write(
                    &path,
                    if marker == "sequencer/todo" {
                        "revert abc message\n"
                    } else {
                        "operation\n"
                    },
                )
                .unwrap();
            }
            assert!(
                matches!(
                    assess(root, repo.path(), &Request::default()),
                    Err(SaveError::ActiveOperation)
                ),
                "{marker}"
            );
        }
        // Real unmerged index independent of a remaining operation marker.
        let repo = fixture(root);
        repo.success(&["switch", "-c", "other", "main"]);
        fs::write(repo.path().join("tracked"), "other\n").unwrap();
        repo.success(&["commit", "-am", "other"]);
        repo.success(&["switch", "Feature/Exact_Name"]);
        fs::write(repo.path().join("tracked"), "ours\n").unwrap();
        repo.success(&["commit", "-am", "ours"]);
        assert!(
            !git::run(repo.path(), ["merge", "other"])
                .unwrap()
                .status
                .success()
        );
        fs::remove_file(repo.path().join(".git/MERGE_HEAD")).unwrap();
        assert!(matches!(
            assess(root, repo.path(), &Request::default()),
            Err(SaveError::Conflicts)
        ));
    });
}

#[test]
fn linked_worktrees_share_settings_but_keep_local_scope() {
    isolated(
        "linked_worktrees_share_settings_but_keep_local_scope",
        true,
        |root| {
            let repo = fixture(root);
            let linked = root.join("linked");
            repo.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "-b".as_ref(),
                "release/Exact".as_ref(),
                linked.as_os_str(),
            ]);
            repo.success(&["remote", "add", "team", "/missing/team"]);
            fs::write(
                repo.path().join(".git/oneflow.conf"),
                "remote=team\nbase=trunk\nsave-action=wip",
            )
            .unwrap();
            fs::write(linked.join("untracked"), "work").unwrap();
            let local = assess(root, repo.path(), &Request::default()).unwrap();
            let other = assess(root, &linked, &Request::default()).unwrap();
            assert_eq!(local.settings, other.settings);
            assert_eq!(local.scope.tree_state(), TreeState::Clean);
            assert_eq!(other.scope.tree_state(), TreeState::UnstagedOnly);
            assert_eq!(other.local_steps, [LocalStep::StageAndCommitRemainingWip]);
            assert_eq!(
                other.publication.as_ref().unwrap().branch_ref,
                b"refs/heads/release/Exact"
            );
            assert_eq!(
                git_oneflow::start::prepare(&linked, "another")
                    .unwrap()
                    .settings,
                other.settings
            );
            revalidate(root, &linked, &other).unwrap();
        },
    );
}

#[test]
fn revalidation_detects_content_index_head_settings_and_destination_changes() {
    isolated(
        "revalidation_detects_content_index_head_settings_and_destination_changes",
        true,
        |root| {
            for change in [
                "content",
                "index",
                "untracked",
                "new-path",
                "settings",
                "head",
                "branch",
                "upstream",
                "push-url",
            ] {
                let repo = fixture(root);
                staged(&repo);
                fs::write(repo.path().join("tracked"), "remaining content\n").unwrap();
                fs::write(repo.path().join("untracked"), "before\n").unwrap();
                let plan = assess(root, repo.path(), &request(SaveAction::Wip)).unwrap();
                match change {
                    "content" => {
                        fs::write(repo.path().join("tracked"), "different content\n").unwrap()
                    }
                    "index" => {
                        fs::write(repo.path().join("tracked"), "different staged content\n")
                            .unwrap();
                        repo.success(&["add", "tracked"]);
                        fs::write(repo.path().join("tracked"), "remaining content\n").unwrap();
                    }
                    "untracked" => fs::write(repo.path().join("untracked"), "after\n").unwrap(),
                    "new-path" => fs::write(repo.path().join("another"), "new\n").unwrap(),
                    "settings" => {
                        fs::write(repo.path().join(".git/oneflow.conf"), "base=trunk").unwrap()
                    }
                    "head" => {
                        repo.success(&["commit", "-m", "checkpoint"]);
                    }
                    "branch" => {
                        repo.success(&["switch", "-c", "different"]);
                    }
                    "upstream" => {
                        repo.success(&["config", "branch.Feature/Exact_Name.remote", "origin"]);
                        repo.success(&[
                            "config",
                            "branch.Feature/Exact_Name.merge",
                            "refs/heads/Feature/Exact_Name",
                        ]);
                    }
                    "push-url" => {
                        repo.success(&["config", "remote.origin.pushurl", "/different/remote"]);
                    }
                    _ => unreachable!(),
                }
                assert!(
                    matches!(
                        revalidate(root, repo.path(), &plan),
                        Err(SaveError::ContextChanged)
                    ),
                    "{change}"
                );
            }
        },
    );
}

#[test]
fn dirty_submodules_rejected_and_gitlink_changes_revalidated() {
    isolated(
        "dirty_submodules_rejected_and_gitlink_changes_revalidated",
        true,
        |root| {
            let child = fixture(root);
            let parent = fixture(root);
            parent.success(&[
                "-c".as_ref(),
                "protocol.file.allow=always".as_ref(),
                "submodule".as_ref(),
                "add".as_ref(),
                child.path().as_os_str(),
                "module".as_ref(),
            ]);
            parent.success(&["commit", "-am", "submodule"]);
            parent.success(&["config", "submodule.module.ignore", "all"]);
            parent.success(&["config", "diff.ignoreSubmodules", "all"]);
            let module = parent.path().join("module");
            fs::write(module.join("ignored"), "ignored").unwrap();
            assert_eq!(
                assess(root, parent.path(), &Request::default())
                    .unwrap()
                    .scope
                    .tree_state(),
                TreeState::Clean
            );
            for (tracked, untracked) in [(false, true), (true, false), (true, true)] {
                fs::write(
                    module.join("tracked"),
                    if tracked { "changed\n" } else { "base\n" },
                )
                .unwrap();
                if untracked {
                    fs::write(module.join("untracked"), "new").unwrap();
                } else {
                    fs::remove_file(module.join("untracked")).unwrap();
                }
                assert!(matches!(
                    assess(root, parent.path(), &request(SaveAction::Wip)),
                    Err(SaveError::DirtySubmodules)
                ));
            }
            fs::remove_file(module.join("untracked")).unwrap();
            let commit = |message: &str| {
                let output = git::run(
                    &module,
                    [
                        "-c",
                        "user.name=Fixture",
                        "-c",
                        "user.email=fixture@example.invalid",
                        "commit",
                        "-am",
                        message,
                    ],
                )
                .unwrap();
                assert!(output.status.success());
            };
            commit("child change");
            let plan = assess(root, parent.path(), &request(SaveAction::Wip)).unwrap();
            assert_eq!(plan.scope.remaining_tracked, [b"module".to_vec()]);
            revalidate(root, parent.path(), &plan).unwrap();
            fs::write(module.join("tracked"), "second changed commit\n").unwrap();
            commit("second child change");
            assert!(matches!(
                revalidate(root, parent.path(), &plan),
                Err(SaveError::ContextChanged)
            ));
        },
    );
}

#[cfg(unix)]
#[test]
fn no_transport_fsmonitor_diff_or_textconv_execution_or_index_refresh() {
    use std::time::{Duration, SystemTime};
    isolated(
        "no_transport_fsmonitor_diff_or_textconv_execution_or_index_refresh",
        true,
        |root| {
            let repo = fixture(root);
            fs::write(repo.path().join(".gitattributes"), "tracked diff=trap\n").unwrap();
            repo.success(&["add", ".gitattributes"]);
            repo.success(&["commit", "-m", "attributes"]);
            for (key, value) in [
                ("core.fsmonitor", "touch fsmonitor-ran"),
                ("diff.external", "touch diff-ran"),
                ("diff.trap.command", "touch diff-driver-ran"),
                ("diff.trap.textconv", "touch textconv-ran"),
                ("protocol.ext.allow", "always"),
                ("remote.origin.url", "ext::touch remote-ran"),
            ] {
                repo.success(&["config", key, value]);
            }
            // Clean-content stale stat information would normally refresh the index.
            fs::File::options()
                .write(true)
                .open(repo.path().join("tracked"))
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)),
                )
                .unwrap();
            assess(root, repo.path(), &Request::default()).unwrap();
            fs::write(repo.path().join(".git/index.lock"), "existing lock").unwrap();
            staged_without_monitor(&repo);
            for action in [
                SaveAction::Ask,
                SaveAction::PublishOnly,
                SaveAction::Wip,
                SaveAction::Stash,
            ] {
                let plan = assess(root, repo.path(), &request(action)).unwrap();
                revalidate(root, repo.path(), &plan).unwrap();
            }
            for name in [
                "fsmonitor-ran",
                "diff-ran",
                "diff-driver-ran",
                "textconv-ran",
                "remote-ran",
            ] {
                assert!(!repo.path().join(name).exists());
            }
            // Positive controls prove that the trap configuration is operative.
            let _ = git::run(repo.path(), ["ls-remote", "origin"]).unwrap();
            assert!(repo.path().join("remote-ran").exists());
            let _ = git::run(repo.path(), ["diff", "HEAD"]).unwrap();
            assert!(
                repo.path().join("diff-ran").exists()
                    || repo.path().join("diff-driver-ran").exists()
            );
            let _ = git::run(repo.path(), ["status", "--porcelain=v2"]).unwrap();
            assert!(repo.path().join("fsmonitor-ran").exists());
        },
    );
    fn staged_without_monitor(repo: &Repository) {
        // Dirty work with a lock already held is still observable read-only.
        fs::write(repo.path().join("tracked"), "changed\n").unwrap();
    }
}

#[cfg(unix)]
#[test]
fn byte_paths_symlinks_and_scope_content_changes() {
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::symlink},
    };
    isolated(
        "byte_paths_symlinks_and_scope_content_changes",
        true,
        |root| {
            let repo = fixture(root);
            let path = OsString::from_vec(b"bytes\xff".to_vec());
            fs::write(repo.path().join(&path), b"\0binary\xff").unwrap();
            symlink("missing-target", repo.path().join("link")).unwrap();
            let plan = assess(root, repo.path(), &request(SaveAction::Stash)).unwrap();
            assert_eq!(
                plan.scope.untracked,
                [b"bytes\xff".to_vec(), b"link".to_vec()]
            );
            revalidate(root, repo.path(), &plan).unwrap();
            fs::remove_file(repo.path().join("link")).unwrap();
            symlink("different-target", repo.path().join("link")).unwrap();
            assert!(matches!(
                revalidate(root, repo.path(), &plan),
                Err(SaveError::ContextChanged)
            ));
        },
    );
}

#[test]
fn nested_scope_and_multiple_push_urls_revalidate() {
    isolated(
        "nested_scope_and_multiple_push_urls_revalidate",
        true,
        |root| {
            let repo = fixture(root);
            remaining(&repo);
            let nested = repo.path().join("nested");
            fs::create_dir(&nested).unwrap();
            repo.success(&["config", "diff.relative", "true"]);
            repo.success(&["config", "remote.origin.pushurl", "/first/push"]);
            repo.success(&["config", "--add", "remote.origin.pushurl", "/second/push"]);
            let plan = assess(root, &nested, &request(SaveAction::Wip)).unwrap();
            assert_eq!(plan.scope.remaining_tracked, [b"tracked".to_vec()]);
            revalidate(root, repo.path(), &plan).unwrap();
            repo.success(&[
                "config",
                "--replace-all",
                "remote.origin.pushurl",
                "/third/push",
                "^/second/push$",
            ]);
            assert!(matches!(
                revalidate(root, repo.path(), &plan),
                Err(SaveError::ContextChanged)
            ));
            fs::write(
                repo.path().join(".git/oneflow.conf"),
                "base=Feature/Exact_Name",
            )
            .unwrap();
            assert!(matches!(
                assess(root, &nested, &Request::default()),
                Err(SaveError::ProtectedBase)
            ));
        },
    );
}
