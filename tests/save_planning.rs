#[path = "support/snapshot.rs"]
mod preservation;
mod support;
use preservation::snapshot;
use std::{fs, path::Path};
use support::{Repository, isolated};
use twigsmith::{
    git,
    save::{self, LocalStep, Plan, Request, RequiredInput, SaveError, TreeState},
    settings::{SaveKind, SaveOperation, SavePlan, SaveScope},
};

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
#[derive(Clone, Copy)]
enum TestPlan {
    Ask,
    PublishOnly,
    Split,
    Stash,
}
fn request(action: TestPlan) -> Request {
    let plan = match action {
        TestPlan::Ask => SavePlan::Ask,
        TestPlan::PublishOnly => SavePlan::PublishOnly,
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
    };
    Request {
        plan: Some(plan),
        ..Request::default()
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
fn groups_skip_empty_selection_and_labels_are_resolved_without_message_input() {
    isolated(
        "groups_skip_empty_selection_and_labels_are_resolved_without_message_input",
        true,
        |root| {
            for (stage, remain) in [(false, false), (true, false), (false, true), (true, true)] {
                let repo = fixture(root);
                if stage {
                    staged(&repo);
                }
                if remain {
                    fs::write(repo.path().join("new"), "new").unwrap();
                }
                let plan = assess(root, repo.path(), &request(TestPlan::Split)).unwrap();
                assert_eq!(
                    plan.local_steps.len(),
                    usize::from(stage) + usize::from(remain)
                );
                assert_eq!(plan.required_input, RequiredInput::None);
                assert!(plan.publication.is_some());
                for step in &plan.local_steps {
                    assert_eq!(
                        step.message,
                        format!("Save work [{}]", step.operation.scope.as_str())
                    );
                }
                let default = assess(root, repo.path(), &Request::default()).unwrap();
                assert_eq!(
                    default.required_input,
                    if stage || remain {
                        RequiredInput::Action
                    } else {
                        RequiredInput::None
                    }
                );
            }
        },
    );
}
#[test]
fn invalid_selection_and_labels_are_rejected_even_when_clean() {
    isolated(
        "invalid_selection_and_labels_are_rejected_even_when_clean",
        true,
        |root| {
            let repo = fixture(root);
            for scopes in [
                vec![SaveScope::All, SaveScope::Staged],
                vec![SaveScope::Staged, SaveScope::Staged],
            ] {
                let req = Request {
                    plan: Some(SavePlan::Operations(
                        scopes
                            .into_iter()
                            .map(|scope| SaveOperation {
                                action: SaveKind::Commit,
                                scope,
                            })
                            .collect(),
                    )),
                    ..Request::default()
                };
                assert!(matches!(
                    assess(root, repo.path(), &req),
                    Err(SaveError::InvalidPlan(_))
                ));
            }
            for req in [
                Request {
                    plan: Some(SavePlan::PublishOnly),
                    message: Some("message".into()),
                    ..Request::default()
                },
                Request {
                    message: Some(" ".into()),
                    ..Request::default()
                },
            ] {
                assert!(matches!(
                    assess(root, repo.path(), &req),
                    Err(SaveError::InvalidPlan(_))
                ));
            }
        },
    );
}
#[test]
fn original_scope_includes_partial_files_deletions_and_nonignored_additions() {
    isolated(
        "original_scope_includes_partial_files_deletions_and_nonignored_additions",
        true,
        |root| {
            let repo = fixture(root);
            staged(&repo);
            remaining(&repo);
            fs::write(repo.path().join("tracked"), "later").unwrap();
            fs::write(repo.path().join("new"), "new").unwrap();
            fs::write(repo.path().join("ignored"), "ignored").unwrap();
            let plan = assess(root, repo.path(), &request(TestPlan::Split)).unwrap();
            assert_eq!(plan.scope.staged, [b"tracked".to_vec()]);
            assert_eq!(plan.scope.remaining_tracked, [b"tracked".to_vec()]);
            assert_eq!(plan.scope.untracked, [b"new".to_vec()]);
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
                repo.path().join(".git/twigsmith.config.toml"),
                "publish-remote='team'\nbase='trunk'\nsave-plan=['commit:staged','commit:unstaged']",
            )
            .unwrap();
            fs::write(linked.join("untracked"), "work").unwrap();
            let local = assess(root, repo.path(), &Request::default()).unwrap();
            let other = assess(root, &linked, &Request::default()).unwrap();
            assert_eq!(local.settings, other.settings);
            assert_eq!(local.scope.tree_state(), TreeState::Clean);
            assert_eq!(other.scope.tree_state(), TreeState::UnstagedOnly);
            assert_eq!(
                other.local_steps,
                [LocalStep {
                    operation: SaveOperation {
                        action: SaveKind::Commit,
                        scope: SaveScope::Unstaged
                    },
                    message: "Save work [unstaged]".into()
                }]
            );
            assert_eq!(
                other.publication.as_ref().unwrap().branch_ref,
                b"refs/heads/release/Exact"
            );
            assert_eq!(
                twigsmith::start::prepare(&linked, "another")
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
                let plan = assess(root, repo.path(), &request(TestPlan::Split)).unwrap();
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
                    "settings" => fs::write(
                        repo.path().join(".git/twigsmith.config.toml"),
                        "base='trunk'",
                    )
                    .unwrap(),
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
                    assess(root, parent.path(), &request(TestPlan::Split)),
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
            let plan = assess(root, parent.path(), &request(TestPlan::Split)).unwrap();
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
                TestPlan::Ask,
                TestPlan::PublishOnly,
                TestPlan::Split,
                TestPlan::Stash,
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
            let plan = assess(root, repo.path(), &request(TestPlan::Stash)).unwrap();
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
            let plan = assess(root, &nested, &request(TestPlan::Split)).unwrap();
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
                repo.path().join(".git/twigsmith.config.toml"),
                "base='Feature/Exact_Name'",
            )
            .unwrap();
            assert!(matches!(
                assess(root, &nested, &Request::default()),
                Err(SaveError::ProtectedBase)
            ));
        },
    );
}
