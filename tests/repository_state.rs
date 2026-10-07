#[path = "support/snapshot.rs"]
mod preservation;
mod support;
use preservation::snapshot;

use std::{fs, path::Path, time::SystemTime};

use git_oneflow::{
    git,
    repository::{
        self, Change, ChangeKind, Divergence, Head, InspectionError, Observation, Operations,
        RepositoryKind, RepositoryState, SubmoduleState, Upstream, UpstreamReference,
    },
};
use support::{Repository, isolated};

fn state(directory: &Path) -> RepositoryState {
    match repository::inspect(directory).unwrap() {
        Observation::Repository(state) => *state,
        Observation::OutsideRepository => panic!("expected repository"),
    }
}

fn commit(repo: &Repository, file: &str, content: &str, message: &str) -> String {
    fs::write(repo.path().join(file), content).unwrap();
    repo.success(&["add", "--", file]);
    repo.success(&["commit", "-m", message]);
    String::from_utf8(repo.success(&["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}

fn divergence(state: &RepositoryState) -> Divergence {
    match &state.upstream {
        Upstream::Configured {
            reference:
                UpstreamReference::Available {
                    divergence: Some(counts),
                    ..
                },
            ..
        } => *counts,
        other => panic!("expected available divergence: {other:?}"),
    }
}

fn preserved(root: &Path, directory: &Path) -> RepositoryState {
    let before = snapshot(root);
    let observation = state(directory);
    assert_eq!(
        snapshot(root),
        before,
        "inspection changed files, metadata, or their set"
    );
    observation
}

#[test]
fn discovery_and_head_states() {
    isolated("discovery_and_head_states", true, |root| {
        assert_eq!(
            repository::inspect(root).unwrap(),
            Observation::OutsideRepository
        );
        let repo = Repository::new(root);
        let initial = preserved(root, repo.path());
        assert_eq!(initial.kind, RepositoryKind::Working);
        assert_eq!(
            initial.head,
            Head::Unborn {
                branch: b"main".to_vec()
            }
        );
        assert_eq!(initial.upstream, Upstream::Absent);
        assert!(!initial.working_tree.as_ref().unwrap().is_dirty());
        assert_eq!(initial.operations, Operations::default());
        let oid = commit(&repo, "tracked", "base\n", "base");
        let nested = repo.path().join("nested directory\nwith newline");
        fs::create_dir(&nested).unwrap();
        let ordinary = preserved(root, repo.path());
        assert_eq!(ordinary, preserved(root, &nested));
        assert_eq!(
            ordinary.head,
            Head::Attached {
                branch: b"main".to_vec(),
                commit: oid.as_bytes().to_vec()
            }
        );
        assert_eq!(
            ordinary.git_dir,
            fs::canonicalize(repo.path().join(".git")).unwrap()
        );
        assert_eq!(ordinary.git_dir, ordinary.common_dir);
        assert_eq!(
            ordinary.working_tree.unwrap().root,
            fs::canonicalize(repo.path()).unwrap()
        );
        repo.success(&["checkout", "--detach", &oid]);
        let detached = preserved(root, &nested);
        assert_eq!(
            detached.head,
            Head::Detached {
                commit: oid.as_bytes().to_vec()
            }
        );
        assert_eq!(detached.upstream, Upstream::NotApplicable);
    });
}

#[test]
fn linked_worktree_uses_its_own_head_index_and_operations() {
    isolated(
        "linked_worktree_uses_its_own_head_index_and_operations",
        true,
        |root| {
            let repo = Repository::new(root);
            commit(&repo, "tracked", "base\n", "base");
            let linked = root.join("linked 'worktree'\n");
            repo.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "-b".as_ref(),
                "linked-branch".as_ref(),
                linked.as_os_str(),
            ]);
            fs::write(linked.join("tracked"), "linked\n").unwrap();
            let observed = preserved(root, &linked);
            assert_eq!(observed.kind, RepositoryKind::LinkedWorktree);
            assert_ne!(observed.git_dir, observed.common_dir);
            assert_eq!(observed.common_dir, state(repo.path()).git_dir);
            assert!(
                matches!(observed.head, Head::Attached { branch, .. } if branch == b"linked-branch")
            );
            assert!(observed.working_tree.unwrap().is_dirty());
            assert!(!state(repo.path()).working_tree.unwrap().is_dirty());

            // A real conflict in this worktree must use its private MERGE_HEAD.
            repo.success(&["checkout", "-b", "other"]);
            commit(&repo, "tracked", "other\n", "other");
            let output = git::run(&linked, ["add", "tracked"]).unwrap();
            assert!(output.status.success());
            assert!(
                git::run(&linked, ["commit", "-m", "linked"])
                    .unwrap()
                    .status
                    .success()
            );
            assert!(
                !git::run(&linked, ["merge", "other"])
                    .unwrap()
                    .status
                    .success()
            );
            let conflict = preserved(root, &linked);
            assert!(conflict.operations.merge);
            assert!(
                conflict
                    .working_tree
                    .unwrap()
                    .changes
                    .iter()
                    .any(|c| matches!(c.kind, ChangeKind::Conflicted { .. }))
            );
            assert_eq!(state(repo.path()).operations, Operations::default());
        },
    );
}

#[test]
fn bare_repository_has_no_working_tree() {
    isolated("bare_repository_has_no_working_tree", true, |root| {
        let repo = Repository::new(root);
        let oid = commit(&repo, "tracked", "base\n", "base");
        let bare = root.join("bare repo\n.git");
        repo.success(&[
            "clone".as_ref(),
            "--bare".as_ref(),
            repo.path().as_os_str(),
            bare.as_os_str(),
        ]);
        let observed = preserved(root, &bare);
        assert_eq!(observed.kind, RepositoryKind::Bare);
        assert_eq!(observed.working_tree, None);
        assert_eq!(
            observed.head,
            Head::Attached {
                branch: b"main".to_vec(),
                commit: oid.into_bytes()
            }
        );
        let empty = root.join("empty.git");
        repo.success(&[
            "init".as_ref(),
            "--bare".as_ref(),
            "--initial-branch=empty".as_ref(),
            "--template=".as_ref(),
            empty.as_os_str(),
        ]);
        assert_eq!(
            preserved(root, &empty).head,
            Head::Unborn {
                branch: b"empty".to_vec()
            }
        );
    });
}

#[test]
fn change_categories_and_filename_bytes() {
    isolated("change_categories_and_filename_bytes", true, |root| {
        let repo = Repository::new(root);
        let odd = "spaces 'quotes' \"double\" \\ tabs\t newline\n Unicode λ -> name";
        for name in [
            "both",
            "staged",
            "unstaged",
            "deleted",
            "renamed",
            odd,
            ".gitignore",
        ] {
            fs::write(
                repo.path().join(name),
                if name == ".gitignore" {
                    "ignored\n"
                } else {
                    "base\n"
                },
            )
            .unwrap();
        }
        repo.success(&["add", "."]);
        repo.success(&["commit", "-m", "base"]);
        for name in ["both", "staged", "unstaged", odd] {
            fs::write(repo.path().join(name), "changed\n").unwrap();
        }
        repo.success(&["add", "both", "staged"]);
        fs::write(repo.path().join("both"), "changed again\n").unwrap();
        fs::remove_file(repo.path().join("deleted")).unwrap();
        repo.success(&["mv", "renamed", "new name\n"]);
        fs::write(repo.path().join("added"), "new\n").unwrap();
        repo.success(&["add", "added"]);
        fs::write(repo.path().join("ignored"), "ignored\n").unwrap();
        let untracked = "? untracked\n# branch.oid fake\n";
        fs::write(repo.path().join(untracked), "untracked\n").unwrap();
        fs::create_dir(repo.path().join("untracked dir")).unwrap();
        fs::write(repo.path().join("untracked dir/file"), "untracked\n").unwrap();
        let observed = preserved(root, repo.path());
        let changes = &observed.working_tree.as_ref().unwrap().changes;
        let tracked = |path: &[u8], staged, unstaged| {
            assert!(changes.iter().any(|c| c.path == path && matches!(&c.kind, ChangeKind::Tracked { staged: s, unstaged: u, .. } if *s == staged && *u == unstaged)), "missing {path:?}: {changes:?}");
        };
        tracked(b"both", Change::Modified, Change::Modified);
        tracked(b"staged", Change::Modified, Change::Unchanged);
        tracked(b"unstaged", Change::Unchanged, Change::Modified);
        tracked(odd.as_bytes(), Change::Unchanged, Change::Modified);
        tracked(b"deleted", Change::Unchanged, Change::Deleted);
        tracked(b"added", Change::Added, Change::Unchanged);
        // Deliberately disable rename heuristics: the two paths are observable
        // additions/deletions regardless of the user's rename display settings.
        tracked(b"renamed", Change::Deleted, Change::Unchanged);
        tracked(b"new name\n", Change::Added, Change::Unchanged);
        assert!(!changes.iter().any(|c| c.path == b"ignored"));
        for path in [untracked.as_bytes(), b"untracked dir/file"] {
            assert!(
                changes
                    .iter()
                    .any(|c| c.path == path && c.kind == ChangeKind::Untracked)
            );
        }
        for (key, value) in [
            ("color.ui", "always"),
            ("color.status", "always"),
            ("core.quotePath", "false"),
            ("status.relativePaths", "true"),
            ("status.showUntrackedFiles", "no"),
            ("status.renames", "copies"),
            ("diff.renames", "true"),
            ("column.status", "always"),
            ("status.short", "true"),
            ("status.branch", "true"),
        ] {
            repo.success(&["config", key, value]);
        }
        let nested = repo.path().join("nested");
        fs::create_dir(&nested).unwrap();
        assert_eq!(preserved(root, &nested), observed);
    });
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_and_type_changes() {
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::symlink},
    };
    isolated("non_utf8_paths_and_type_changes", true, |root| {
        let repo = Repository::new(root);
        commit(&repo, "type", "base\n", "base");
        fs::remove_file(repo.path().join("type")).unwrap();
        symlink("missing target", repo.path().join("type")).unwrap();
        let name = b"non-UTF8 \xff\n";
        let filename = OsString::from_vec(name.to_vec());
        fs::write(repo.path().join(&filename), "new\n").unwrap();
        let observed = preserved(root, repo.path());
        let changes = observed.working_tree.unwrap().changes;
        assert!(
            changes
                .iter()
                .any(|c| c.path == name && c.kind == ChangeKind::Untracked)
        );
        assert!(changes.iter().any(|c| c.path == b"type"
            && matches!(
                c.kind,
                ChangeKind::Tracked {
                    unstaged: Change::TypeChanged,
                    ..
                }
            )));
        repo.success(&["add".as_ref(), "--".as_ref(), filename.as_os_str()]);
        assert!(
            state(repo.path())
                .working_tree
                .unwrap()
                .changes
                .iter()
                .any(|c| c.path == name
                    && matches!(
                        c.kind,
                        ChangeKind::Tracked {
                            staged: Change::Added,
                            ..
                        }
                    ))
        );
    });
}

#[test]
fn ignored_files_are_clean_and_status_does_not_refresh_index() {
    isolated(
        "ignored_files_are_clean_and_status_does_not_refresh_index",
        true,
        |root| {
            let repo = Repository::new(root);
            commit(&repo, ".gitignore", "ignored\n", "ignore");
            commit(&repo, "tracked", "base\n", "base");
            fs::write(repo.path().join("ignored"), "ignore me\n").unwrap();
            // Change stat information without changing content: ordinary status
            // would write an index refresh, even though the working tree is clean.
            let file = fs::File::options()
                .write(true)
                .open(repo.path().join("tracked"))
                .unwrap();
            file.set_times(fs::FileTimes::new().set_modified(
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000),
            ))
            .unwrap();
            let observed = preserved(root, repo.path());
            assert!(!observed.working_tree.unwrap().is_dirty());
            // Confirm the fixture actually exposes the write prevented above.
            let before = fs::read(repo.path().join(".git/index")).unwrap();
            repo.success(&["status", "--porcelain=v2"]);
            assert_ne!(fs::read(repo.path().join(".git/index")).unwrap(), before);
            fs::write(repo.path().join(".git/index.lock"), "existing lock\n").unwrap();
            assert!(
                !preserved(root, repo.path())
                    .working_tree
                    .unwrap()
                    .is_dirty()
            );
        },
    );
}

#[test]
fn upstream_absent_gone_and_local_divergence() {
    isolated("upstream_absent_gone_and_local_divergence", true, |root| {
        let repo = Repository::new(root);
        let base = commit(&repo, "base", "base\n", "base");
        assert_eq!(state(repo.path()).upstream, Upstream::Absent);
        repo.success(&["branch", "upstream"]);
        repo.success(&["branch", "--set-upstream-to=upstream"]);
        assert_eq!(
            divergence(&preserved(root, repo.path())),
            Divergence {
                ahead: 0,
                behind: 0
            }
        );
        repo.success(&["config", "status.aheadBehind", "false"]);
        repo.success(&["config", "core.abbrev", "4"]);
        commit(&repo, "local", "local\n", "local");
        assert_eq!(
            divergence(&preserved(root, repo.path())),
            Divergence {
                ahead: 1,
                behind: 0
            }
        );
        repo.success(&["checkout", "upstream"]);
        commit(&repo, "remote", "remote\n", "upstream");
        repo.success(&["checkout", "main"]);
        assert_eq!(
            divergence(&preserved(root, repo.path())),
            Divergence {
                ahead: 1,
                behind: 1
            }
        );
        repo.success(&["reset", "--hard", &base]);
        assert_eq!(
            divergence(&preserved(root, repo.path())),
            Divergence {
                ahead: 0,
                behind: 1
            }
        );
        repo.success(&["branch", "-D", "upstream"]);
        assert!(
            matches!(preserved(root, repo.path()).upstream, Upstream::Configured { remote: Some(remote), merge_refs, local_reference: Some(reference), reference: UpstreamReference::Unavailable } if remote == b"." && merge_refs == [b"refs/heads/upstream".to_vec()] && reference == b"refs/heads/upstream")
        );
    });
}

#[test]
fn remote_tracking_upstream_uses_only_local_refs() {
    isolated(
        "remote_tracking_upstream_uses_only_local_refs",
        true,
        |root| {
            let repo = Repository::new(root);
            let oid = commit(&repo, "base", "base\n", "base");
            // Deliberately inaccessible remote; no command may try to contact it.
            repo.success(&[
                "remote",
                "add",
                "origin",
                "https://oneflow.invalid/never-contact.git",
            ]);
            repo.success(&["config", "branch.main.remote", "origin"]);
            repo.success(&["config", "branch.main.merge", "refs/heads/main"]);
            assert!(matches!(
                preserved(root, repo.path()).upstream,
                Upstream::Configured {
                    reference: UpstreamReference::Unavailable,
                    ..
                }
            ));
            repo.success(&["update-ref", "refs/remotes/origin/main", &oid]);
            assert_eq!(
                divergence(&preserved(root, repo.path())),
                Divergence {
                    ahead: 0,
                    behind: 0
                }
            );
            repo.success(&["checkout", "--orphan", "unborn"]);
            repo.success(&["config", "branch.unborn.remote", "origin"]);
            repo.success(&["config", "branch.unborn.merge", "refs/heads/main"]);
            assert!(matches!(
                preserved(root, repo.path()).upstream,
                Upstream::Configured {
                    reference: UpstreamReference::Available {
                        divergence: None,
                        ..
                    },
                    ..
                }
            ));
            repo.success(&["update-ref", "-d", "refs/remotes/origin/main"]);
            assert!(matches!(
                preserved(root, repo.path()).upstream,
                Upstream::Configured {
                    reference: UpstreamReference::Unavailable,
                    ..
                }
            ));
        },
    );
}

#[test]
fn unborn_and_incomplete_upstream_configuration_stays_explicit() {
    isolated(
        "unborn_and_incomplete_upstream_configuration_stays_explicit",
        true,
        |root| {
            let repo = Repository::new(root);
            repo.success(&["config", "branch.main.remote", "."]);
            assert!(
                matches!(preserved(root, repo.path()).upstream, Upstream::Configured { remote: Some(_), merge_refs, reference: UpstreamReference::Unavailable, .. } if merge_refs.is_empty())
            );
            repo.success(&["config", "branch.main.merge", "refs/heads/gone"]);
            assert!(matches!(
                preserved(root, repo.path()).upstream,
                Upstream::Configured {
                    reference: UpstreamReference::Unavailable,
                    ..
                }
            ));
            repo.success(&["config", "--unset", "branch.main.remote"]);
            assert!(matches!(
                preserved(root, repo.path()).upstream,
                Upstream::Configured {
                    remote: None,
                    reference: UpstreamReference::Unavailable,
                    ..
                }
            ));
        },
    );
}

fn conflicting_branches(root: &Path) -> (Repository, String, String) {
    let repo = Repository::new(root);
    let base = commit(&repo, "conflict\nname", "base\n", "base");
    repo.success(&["checkout", "-b", "other"]);
    let other = commit(&repo, "conflict\nname", "other\n", "other");
    repo.success(&["checkout", "main"]);
    commit(&repo, "conflict\nname", "main\n", "main");
    (repo, base, other)
}

#[test]
fn merge_rebase_cherry_pick_and_revert_conflicts_are_preserved() {
    isolated(
        "merge_rebase_cherry_pick_and_revert_conflicts_are_preserved",
        true,
        |root| {
            for operation in ["merge", "rebase", "rebase-apply", "cherry-pick", "revert"] {
                let (repo, base, other) = conflicting_branches(root);
                let args = match operation {
                    "merge" => vec!["merge", "other"],
                    "rebase" => vec!["rebase", "other"],
                    "rebase-apply" => vec!["rebase", "--apply", "other"],
                    "cherry-pick" => vec!["cherry-pick", &other],
                    // Reverting base would delete the modified file, causing a
                    // modify/delete conflict with a different XY conflict category.
                    _ => vec!["revert", "--no-edit", &base],
                };
                assert!(
                    !git::run(repo.path(), &args).unwrap().status.success(),
                    "expected {operation} conflict"
                );
                let observed = preserved(root, repo.path());
                let ops = observed.operations;
                match operation {
                    "merge" => assert!(ops.merge && !ops.rebase && !ops.cherry_pick && !ops.revert),
                    "rebase" | "rebase-apply" => assert!(ops.rebase && !ops.merge && !ops.revert),
                    "cherry-pick" => {
                        assert!(ops.cherry_pick && !ops.rebase && !ops.merge && !ops.revert)
                    }
                    _ => assert!(ops.revert && !ops.rebase && !ops.merge && !ops.cherry_pick),
                }
                let tree = observed.working_tree.unwrap();
                assert!(tree.is_dirty());
                assert!(
                    tree.changes.iter().any(|c| c.path == b"conflict\nname"
                        && matches!(c.kind, ChangeKind::Conflicted { .. })),
                    "{operation}: {:?}",
                    tree.changes
                );
            }
        },
    );
}

#[test]
fn sequencer_survives_manual_conflict_commit() {
    isolated("sequencer_survives_manual_conflict_commit", true, |root| {
        let (repo, _, other) = conflicting_branches(root);
        repo.success(&["checkout", "other"]);
        let second = commit(&repo, "second", "second\n", "second");
        repo.success(&["checkout", "main"]);
        assert!(
            !git::run(repo.path(), ["cherry-pick", &other, &second])
                .unwrap()
                .status
                .success()
        );
        fs::write(repo.path().join("conflict\nname"), "resolved\n").unwrap();
        repo.success(&["add", "--", "conflict\nname"]);
        repo.success(&["commit", "-m", "resolved manually"]);
        assert!(!repo.path().join(".git/CHERRY_PICK_HEAD").exists());
        assert!(preserved(root, repo.path()).operations.cherry_pick);
    });
}

#[test]
fn dirty_submodules_override_ignore_settings_and_remain_read_only() {
    isolated(
        "dirty_submodules_override_ignore_settings_and_remain_read_only",
        true,
        |root| {
            let child = Repository::new(root);
            commit(&child, "tracked", "base\n", "base");
            commit(&child, ".gitignore", "ignored\n", "ignore");
            let parent = Repository::new(root);
            parent.success(&[
                "-c".as_ref(),
                "protocol.file.allow=always".as_ref(),
                "submodule".as_ref(),
                "add".as_ref(),
                child.path().as_os_str(),
                "module name".as_ref(),
            ]);
            parent.success(&["commit", "-m", "submodule"]);
            let module = parent.path().join("module name");
            fs::write(module.join("ignored"), "ignored\n").unwrap();
            assert!(
                !preserved(root, parent.path())
                    .working_tree
                    .unwrap()
                    .is_dirty()
            );
            parent.success(&["config", "submodule.module name.ignore", "all"]);
            parent.success(&["config", "diff.ignoreSubmodules", "all"]);
            for (tracked, untracked) in [(false, true), (true, false), (true, true)] {
                fs::write(
                    module.join("tracked"),
                    if tracked { "changed\n" } else { "base\n" },
                )
                .unwrap();
                if untracked {
                    fs::write(module.join("untracked"), "new\n").unwrap();
                } else {
                    fs::remove_file(module.join("untracked")).unwrap();
                }
                let observed = preserved(root, parent.path());
                assert!(observed.working_tree.as_ref().unwrap().is_dirty());
                assert!(observed.working_tree.unwrap().changes.iter().any(|c| c.path == b"module name" && matches!(c.kind, ChangeKind::Tracked { submodule: Some(s), .. } if s == SubmoduleState { commit_changed: false, tracked_changes: tracked, untracked_changes: untracked })));
            }
            assert!(
                git::run(
                    &module,
                    [
                        "-c",
                        "user.name=Fixture",
                        "-c",
                        "user.email=fixture@example.invalid",
                        "commit",
                        "-am",
                        "child changed"
                    ]
                )
                .unwrap()
                .status
                .success()
            );
            assert!(preserved(root, parent.path()).working_tree.unwrap().changes.iter().any(|c| matches!(c.kind, ChangeKind::Tracked { submodule: Some(s), .. } if s.commit_changed)));
        },
    );
}

#[test]
fn inspection_errors_are_not_valid_observations() {
    isolated(
        "inspection_errors_are_not_valid_observations",
        true,
        |root| {
            assert!(matches!(
                repository::inspect(root.join("missing")),
                Err(InspectionError::Io(_))
            ));
            let broken = root.join("broken");
            fs::create_dir(&broken).unwrap();
            fs::write(
                broken.join(".git"),
                "gitdir: /oneflow-nonexistent-git-dir\n",
            )
            .unwrap();
            assert!(matches!(
                repository::inspect(&broken),
                Err(InspectionError::Git { .. })
            ));
            let repo = Repository::new(root);
            commit(&repo, "tracked", "base\n", "base");
            fs::write(repo.path().join(".git/index"), "invalid index\n").unwrap();
            let before = snapshot(root);
            assert!(matches!(
                repository::inspect(repo.path()),
                Err(InspectionError::Git { .. })
            ));
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn missing_git_is_an_inspection_error() {
    isolated("missing_git_is_an_inspection_error", false, |root| {
        assert!(matches!(
            repository::inspect(root),
            Err(InspectionError::Io(_))
        ));
    });
}

#[cfg(unix)]
#[test]
fn fsmonitor_hooks_are_disabled_during_inspection() {
    use std::os::unix::fs::PermissionsExt;
    isolated(
        "fsmonitor_hooks_are_disabled_during_inspection",
        true,
        |root| {
            let repo = Repository::new(root);
            commit(&repo, "tracked", "base\n", "base");
            let hook = root.join("fsmonitor-hook");
            fs::write(
                &hook,
                "#!/bin/sh\nprintf called > fsmonitor-was-called\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            // Git treats this setting as a shell command; fixture paths contain
            // spaces and quotes, so quote the executable for Git's shell.
            let command = format!("'{}'", hook.to_str().unwrap().replace('\'', "'\\''"));
            repo.success(&["config", "core.fsmonitor", &command]);
            fs::write(repo.path().join("tracked"), "changed\n").unwrap();
            assert!(
                preserved(root, repo.path())
                    .working_tree
                    .unwrap()
                    .is_dirty()
            );
            assert!(!repo.path().join("fsmonitor-was-called").exists());
            // Prove ordinary status would execute this configured hook.
            repo.success(&["status", "--porcelain=v2"]);
            assert!(repo.path().join("fsmonitor-was-called").exists());
        },
    );
}

#[cfg(unix)]
#[test]
fn missing_promisor_objects_do_not_trigger_a_fetch() {
    isolated(
        "missing_promisor_objects_do_not_trigger_a_fetch",
        true,
        |root| {
            let source = Repository::new(root);
            let oid = commit(&source, "tracked", "base\n", "base");
            let repo = Repository::new(root);
            // Copy the commit/tree/blob without using a network transport.
            repo.success(&["fetch".as_ref(), source.path().as_os_str(), "main".as_ref()]);
            repo.success(&["reset", "--hard", &oid]);
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                source.path().as_os_str(),
            ]);
            repo.success(&["config", "remote.origin.promisor", "true"]);
            repo.success(&["config", "remote.origin.partialclonefilter", "blob:none"]);
            // A local upload-pack marker detects even an attempted lazy fetch.
            repo.success(&[
                "config",
                "remote.origin.uploadpack",
                "touch fetch-was-called; git-upload-pack",
            ]);
            let object = repo
                .path()
                .join(".git/objects")
                .join(&oid[..2])
                .join(&oid[2..]);
            fs::remove_file(object).unwrap();
            let before = snapshot(root);
            assert!(matches!(
                repository::inspect(repo.path()),
                Err(InspectionError::Git { .. })
            ));
            assert_eq!(snapshot(root), before);
            assert!(!repo.path().join("fetch-was-called").exists());
            // Demonstrate the same object lookup normally attempts a lazy fetch.
            let _ = git::run(repo.path(), ["cat-file", "-e", &oid]).unwrap();
            assert!(repo.path().join("fetch-was-called").exists());
        },
    );
}
