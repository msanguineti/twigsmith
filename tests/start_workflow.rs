#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use git_oneflow::{
    git,
    repository::{Head, Observation, RepositoryKind, Upstream},
    start::{self, Action, Effect, Failure, Outcome, StartError},
};
use preservation::snapshot;
use std::{fs, path::Path};
use support::{Repository, isolated};

fn commit(repo: &Repository, file: &str, contents: &str) -> String {
    fs::write(repo.path().join(file), contents).unwrap();
    repo.success(&["add", "--", file]);
    repo.success(&["commit", "-m", contents]);
    oid(repo, "HEAD")
}
fn oid(repo: &Repository, reference: &str) -> String {
    String::from_utf8(repo.success(&["rev-parse", "--verify", reference]).stdout)
        .unwrap()
        .trim()
        .to_owned()
}
fn pair(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "tracked", "base\n");
    let local = Repository::new(root);
    local.success(&[
        "remote".as_ref(),
        "add".as_ref(),
        "origin".as_ref(),
        remote.path().as_os_str(),
    ]);
    local.success(&["fetch", "origin"]);
    local.success(&["reset", "--hard", "refs/remotes/origin/main"]);
    (remote, local)
}
fn start(directory: &Path, name: &str) -> Result<Outcome, Box<Failure>> {
    let request = start::prepare(directory, name).unwrap();
    start::execute(directory, &request)
}
fn branch(state: &git_oneflow::repository::RepositoryState, name: &str, commit: &str) {
    assert_eq!(
        state.head,
        Head::Attached {
            branch: name.as_bytes().to_vec(),
            commit: commit.as_bytes().to_vec()
        }
    );
}
fn preserved_failure(root: &Path, directory: &Path, name: &str) -> Box<Failure> {
    let before = snapshot(root);
    let failure = start(directory, name).unwrap_err();
    assert_eq!(snapshot(root), before, "guard changed repository state");
    assert!(failure.effects.is_empty());
    failure
}
fn remaining(f: &Failure) -> &git_oneflow::repository::RepositoryState {
    match f.remaining.repository.as_ref().unwrap() {
        Observation::Repository(s) => s,
        _ => panic!("not repository"),
    }
}

#[test]
fn current_dirty_branch_is_a_preserved_noop() {
    isolated("current_dirty_branch_is_a_preserved_noop", true, |root| {
        let repo = Repository::new(root);
        let base = commit(&repo, "tracked", "base\n");
        repo.success(&["switch", "-c", "hotfix/Urgent"]);
        fs::write(repo.path().join("tracked"), "work\n").unwrap();
        fs::write(repo.path().join("untracked"), "work\n").unwrap();
        fs::write(repo.path().join(".git/index.lock"), "keep lock").unwrap();
        let before = snapshot(root);
        let outcome = start(repo.path(), "hotfix/Urgent").unwrap();
        assert_eq!(snapshot(root), before);
        assert_eq!(outcome.action, Action::AlreadyCurrent);
        assert!(outcome.effects.is_empty());
        branch(&outcome.state, "hotfix/Urgent", &base);
        assert!(outcome.state.working_tree.unwrap().is_dirty());
    });
}

#[test]
fn local_resume_preserves_history_upstream_and_never_contacts_remote() {
    isolated(
        "local_resume_preserves_history_upstream_and_never_contacts_remote",
        true,
        |root| {
            let repo = Repository::new(root);
            let base = commit(&repo, "tracked", "base\n");
            repo.success(&["switch", "-c", "release/V1"]);
            let target = commit(&repo, "tracked", "local work\n");
            repo.success(&["config", "branch.release/V1.remote", "origin"]);
            repo.success(&["config", "branch.release/V1.merge", "refs/heads/main"]);
            repo.success(&["config", "branch.release/V1.rebase", "true"]);
            repo.success(&["remote", "add", "origin", "/oneflow-unreachable-fixture"]);
            fs::create_dir(repo.path().join(".git/info")).unwrap();
            fs::write(
                repo.path().join(".git/info/exclude"),
                "git-oneflow.config.toml\n",
            )
            .unwrap();
            fs::write(
                repo.path().join("git-oneflow.config.toml"),
                "base=\"main\"\n",
            )
            .unwrap();
            repo.success(&["switch", "main"]);
            let config = fs::read(repo.path().join(".git/config")).unwrap();
            let refs = repo.success(&["show-ref"]).stdout;
            let outcome = start(repo.path(), "release/V1").unwrap();
            assert_eq!(outcome.action, Action::ResumeLocal);
            assert_eq!(
                outcome.effects,
                vec![Effect::Switched {
                    target: "release/V1".into()
                }]
            );
            branch(&outcome.state, "release/V1", &target);
            assert_eq!(fs::read(repo.path().join(".git/config")).unwrap(), config);
            assert_eq!(
                fs::read_to_string(repo.path().join("git-oneflow.config.toml")).unwrap(),
                "base=\"main\"\n"
            );
            assert_eq!(repo.success(&["show-ref"]).stdout, refs);
            assert_eq!(oid(&repo, "refs/heads/main"), base);
            assert!(
                matches!(outcome.state.upstream, Upstream::Configured { remote: Some(ref r), ref merge_refs, .. } if r == b"origin" && merge_refs == &[b"refs/heads/main".to_vec()])
            );
        },
    );
}

#[test]
fn local_resume_restores_and_removes_tracked_shared_configuration() {
    isolated(
        "local_resume_restores_and_removes_tracked_shared_configuration",
        true,
        |root| {
            for crlf in [false, true] {
                let repo = Repository::new(root);
                if crlf {
                    repo.success(&["config", "core.autocrlf", "true"]);
                }
                commit(&repo, "tracked", "base\n");
                repo.success(&["branch", "without-config"]);
                repo.success(&["switch", "-c", "work"]);
                let contents = git_oneflow::config::render(
                    git_oneflow::config::Format::Toml,
                    &git_oneflow::settings::Settings::default(),
                );
                let work = commit(&repo, "git-oneflow.config.toml", &contents);
                repo.success(&["switch", "main"]);
                let config = fs::read(repo.path().join(".git/config")).unwrap();
                let outcome = start(repo.path(), "work").unwrap();
                assert_eq!(outcome.action, Action::ResumeLocal);
                branch(&outcome.state, "work", &work);
                assert_eq!(
                    fs::read_to_string(repo.path().join("git-oneflow.config.toml")).unwrap(),
                    if crlf {
                        contents.replace('\n', "\r\n")
                    } else {
                        contents
                    }
                );
                assert_eq!(fs::read(repo.path().join(".git/config")).unwrap(), config);
                let outcome = start(repo.path(), "without-config").unwrap();
                assert_eq!(outcome.action, Action::ResumeLocal);
                assert!(!repo.path().join("git-oneflow.config.toml").exists());
                assert_eq!(fs::read(repo.path().join(".git/config")).unwrap(), config);
            }
        },
    );
}

#[test]
fn restored_settings_do_not_change_start_authority() {
    isolated(
        "restored_settings_do_not_change_start_authority",
        true,
        |root| {
            let repo = Repository::new(root);
            commit(&repo, "tracked", "base\n");
            repo.success(&["switch", "-c", "work"]);
            let work = commit(
                &repo,
                "git-oneflow.config.toml",
                "base=\"work\"\npublish-remote=\"elsewhere\"\n",
            );
            repo.success(&["switch", "main"]);
            let config = fs::read(repo.path().join(".git/config")).unwrap();
            let refs = repo.success(&["show-ref"]).stdout;
            let outcome = start(repo.path(), "work").unwrap();
            branch(&outcome.state, "work", &work);
            assert_eq!(
                outcome.effects,
                vec![Effect::Switched {
                    target: "work".into()
                }]
            );
            assert_eq!(fs::read(repo.path().join(".git/config")).unwrap(), config);
            assert_eq!(repo.success(&["show-ref"]).stdout, refs);
        },
    );
}

#[cfg(unix)]
#[test]
fn checkout_configuration_transition_rejects_hook_changes() {
    isolated(
        "checkout_configuration_transition_rejects_hook_changes",
        true,
        |root| {
            for scenario in [
                "shared",
                "shared-addition",
                "private",
                "routing",
                "association",
            ] {
                let repo = Repository::new(root);
                commit(&repo, "tracked", "base\n");
                repo.success(&["switch", "-c", "work"]);
                commit(&repo, "git-oneflow.config.toml", "base=\"main\"\n");
                repo.success(&["switch", "main"]);
                let hooks = repo.path().join(".git/start-hooks");
                fs::create_dir(&hooks).unwrap();
                let mutation = match scenario {
                    "shared" => "printf '# hook change\\n' >> git-oneflow.config.toml",
                    "shared-addition" => "printf '{}\\n' > git-oneflow.config.json",
                    "private" => "printf '{}\\n' > .git/git-oneflow.config.json",
                    "routing" => "git config branch.work.pushRemote elsewhere",
                    _ => "git config branch.work.oneflow-policy invalid",
                };
                executable(
                    &hooks.join("post-checkout"),
                    &format!("#!/bin/sh\n{mutation}\n"),
                );
                repo.success(&[
                    "config".as_ref(),
                    "core.hooksPath".as_ref(),
                    hooks.as_os_str(),
                ]);
                let failure = start(repo.path(), "work").unwrap_err();
                assert!(
                    matches!(failure.error, StartError::Changed),
                    "{scenario}: {failure:?}"
                );
                assert_eq!(
                    failure.effects,
                    vec![Effect::Switched {
                        target: "work".into()
                    }]
                );
                assert_eq!(
                    repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                    b"work\n"
                );
                match scenario {
                    "shared" => assert!(
                        fs::read_to_string(repo.path().join("git-oneflow.config.toml"))
                            .unwrap()
                            .contains("# hook change")
                    ),
                    "shared-addition" => {
                        assert!(repo.path().join("git-oneflow.config.json").exists())
                    }
                    "private" => assert!(repo.path().join(".git/git-oneflow.config.json").exists()),
                    "routing" => assert_eq!(
                        repo.success(&["config", "--get", "branch.work.pushRemote"])
                            .stdout,
                        b"elsewhere\n"
                    ),
                    _ => assert_eq!(
                        repo.success(&["config", "--get", "branch.work.oneflow-policy"])
                            .stdout,
                        b"invalid\n"
                    ),
                }
            }
        },
    );
}

#[test]
fn remote_resume_and_new_work_defeat_automatic_tracking() {
    isolated(
        "remote_resume_and_new_work_defeat_automatic_tracking",
        true,
        |root| {
            for automatic in ["always", "inherit", "true"] {
                let (remote, local) = pair(root);
                remote.success(&["switch", "-c", "feature/remote"]);
                let counterpart = commit(&remote, "remote-file", "remote work\n");
                local.success(&["config", "branch.autoSetupMerge", automatic]);
                local.success(&["config", "branch.autoSetupRebase", "always"]);
                let outcome = start(local.path(), "feature/remote").unwrap();
                assert_eq!(outcome.action, Action::ResumeRemote);
                branch(&outcome.state, "feature/remote", &counterpart);
                assert!(
                    matches!(outcome.state.upstream, Upstream::Configured { remote: Some(ref r), ref merge_refs, .. } if r == b"origin" && merge_refs == &[b"refs/heads/feature/remote".to_vec()])
                );
                local.success(&["switch", "main"]);
                let base = oid(&remote, "refs/heads/main");
                let outcome = start(local.path(), "docs/new").unwrap();
                branch(&outcome.state, "docs/new", &base);
                assert_eq!(outcome.action, Action::Create);
                assert_eq!(outcome.state.upstream, Upstream::Absent);
            }
        },
    );
}

#[test]
fn fresh_custom_base_leaves_local_base_and_unrelated_refs_untouched() {
    isolated(
        "fresh_custom_base_leaves_local_base_and_unrelated_refs_untouched",
        true,
        |root| {
            let (remote, local) = pair(root);
            remote.success(&["branch", "trunk"]);
            remote.success(&["switch", "trunk"]);
            local.success(&["remote", "rename", "origin", "team/upstream"]);
            local.success(&["branch", "trunk"]);
            let local_base = commit(&local, "local-only", "local history\n");
            local.success(&["branch", "-f", "trunk", &local_base]);
            let fresh = commit(&remote, "tracked", "fresh remote base\n");
            remote.success(&["tag", "feature/new"]); // A tag is never a branch counterpart.
            remote.success(&["branch", "unrelated"]);
            fs::write(
                local.path().join(".git/git-oneflow.config.toml"),
                "publish-remote='team/upstream'\nbase='trunk'\n",
            )
            .unwrap();
            let outcome = start(local.path(), "feature/new").unwrap();
            branch(&outcome.state, "feature/new", &fresh);
            assert_eq!(outcome.state.upstream, Upstream::Absent);
            assert_eq!(oid(&local, "refs/heads/trunk"), local_base);
            assert_eq!(oid(&local, "refs/heads/main"), local_base);
            assert!(
                !git::run(
                    local.path(),
                    [
                        "show-ref",
                        "--verify",
                        "--quiet",
                        "refs/remotes/team/upstream/unrelated"
                    ]
                )
                .unwrap()
                .status
                .success()
            );
            assert!(
                !git::run(
                    local.path(),
                    ["show-ref", "--verify", "--quiet", "refs/tags/feature/new"]
                )
                .unwrap()
                .status
                .success()
            );
        },
    );
}

#[test]
fn dirty_index_worktree_and_conflicts_are_preserved() {
    isolated(
        "dirty_index_worktree_and_conflicts_are_preserved",
        true,
        |root| {
            for kind in ["unstaged", "staged", "untracked", "conflict"] {
                let repo = Repository::new(root);
                let base = commit(&repo, "tracked", "base\n");
                repo.success(&["branch", "target"]);
                match kind {
                    "unstaged" => fs::write(repo.path().join("tracked"), "dirty\n").unwrap(),
                    "staged" => {
                        fs::write(repo.path().join("tracked"), "dirty\n").unwrap();
                        repo.success(&["add", "tracked"]);
                    }
                    "untracked" => fs::write(repo.path().join("new"), "dirty\n").unwrap(),
                    _ => {
                        repo.success(&["switch", "target"]);
                        commit(&repo, "tracked", "other\n");
                        repo.success(&["switch", "main"]);
                        commit(&repo, "tracked", "ours\n");
                        assert!(
                            !git::run(repo.path(), ["merge", "target"])
                                .unwrap()
                                .status
                                .success()
                        );
                        // Retain an unresolved index without an operation marker.
                        fs::remove_file(repo.path().join(".git/MERGE_HEAD")).unwrap();
                    }
                }
                let failure = preserved_failure(root, repo.path(), "target");
                assert!(matches!(failure.error, StartError::Dirty), "{failure:?}");
                assert!(
                    remaining(&failure)
                        .working_tree
                        .as_ref()
                        .unwrap()
                        .is_dirty()
                );
                if kind != "conflict" {
                    branch(remaining(&failure), "main", &base);
                }
            }
        },
    );
}

#[test]
fn dirty_submodules_block_switching_even_when_ignored() {
    isolated(
        "dirty_submodules_block_switching_even_when_ignored",
        true,
        |root| {
            let child = Repository::new(root);
            commit(&child, "tracked", "base\n");
            let parent = Repository::new(root);
            parent.success(&[
                "-c".as_ref(),
                "protocol.file.allow=always".as_ref(),
                "submodule".as_ref(),
                "add".as_ref(),
                child.path().as_os_str(),
                "module".as_ref(),
            ]);
            parent.success(&["commit", "-m", "module"]);
            parent.success(&["branch", "target"]);
            parent.success(&["config", "submodule.module.ignore", "all"]);
            parent.success(&["config", "diff.ignoreSubmodules", "all"]);
            for kind in ["untracked", "tracked", "commit"] {
                let module = parent.path().join("module");
                if kind == "untracked" {
                    fs::write(module.join("new"), "work\n").unwrap();
                } else {
                    fs::write(module.join("tracked"), "work\n").unwrap();
                }
                if kind == "commit" {
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
                                "work"
                            ]
                        )
                        .unwrap()
                        .status
                        .success()
                    );
                }
                assert!(matches!(
                    preserved_failure(root, parent.path(), "target").error,
                    StartError::Dirty
                ));
                if kind == "untracked" {
                    fs::remove_file(module.join("new")).unwrap();
                }
            }
        },
    );
}

#[test]
fn contexts_and_protected_targets_are_refused_without_writes() {
    isolated(
        "contexts_and_protected_targets_are_refused_without_writes",
        true,
        |root| {
            let repo = Repository::new(root);
            assert!(matches!(
                preserved_failure(root, repo.path(), "target").error,
                StartError::Unborn
            ));
            commit(&repo, "tracked", "base\n");
            let request = start::prepare(repo.path(), "target").unwrap();
            let before = snapshot(root);
            assert!(matches!(
                start::execute(root, &request).unwrap_err().error,
                StartError::OutsideRepository
            ));
            assert!(matches!(
                start::prepare(repo.path(), "main"),
                Err(start::ResolveError::ProtectedBase(_))
            ));
            let mut protected = request.clone();
            protected.target = "main".into();
            assert!(matches!(
                start::execute(repo.path(), &protected).unwrap_err().error,
                StartError::Resolve(start::ResolveError::ProtectedBase(_))
            ));
            assert_eq!(snapshot(root), before);
            repo.success(&["switch", "--detach"]);
            assert!(matches!(
                preserved_failure(root, repo.path(), "target").error,
                StartError::Detached
            ));
            let bare = root.join("bare.git");
            repo.success(&[
                "clone".as_ref(),
                "--bare".as_ref(),
                repo.path().as_os_str(),
                bare.as_os_str(),
            ]);
            assert!(matches!(
                preserved_failure(root, &bare, "target").error,
                StartError::Bare
            ));
        },
    );
}

#[test]
fn linked_worktrees_support_start_and_report_other_occupation() {
    isolated(
        "linked_worktrees_support_start_and_report_other_occupation",
        true,
        |root| {
            let (remote, local) = pair(root);
            let linked = root.join("linked\nworktree");
            local.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "-b".as_ref(),
                "linked".as_ref(),
                linked.as_os_str(),
            ]);
            let failure = preserved_failure(root, local.path(), "linked");
            assert!(
                matches!(failure.error, StartError::Occupied { ref location, .. } if location == &linked)
            );
            fs::create_dir(linked.join("nested")).unwrap();
            let outcome = start(&linked.join("nested"), "feature/new").unwrap();
            assert_eq!(outcome.state.kind, RepositoryKind::LinkedWorktree);
            branch(
                &outcome.state,
                "feature/new",
                &oid(&remote, "refs/heads/main"),
            );
            let failure = preserved_failure(root, local.path(), "feature/new");
            assert!(
                matches!(failure.error, StartError::Occupied { ref location, .. } if location == &linked)
            );
        },
    );
}

fn conflicting(root: &Path) -> (Repository, String, String) {
    let repo = Repository::new(root);
    let base = commit(&repo, "conflict", "base\n");
    repo.success(&["switch", "-c", "other"]);
    let other = commit(&repo, "conflict", "other\n");
    repo.success(&["switch", "main"]);
    commit(&repo, "conflict", "ours\n");
    (repo, base, other)
}

#[test]
fn real_operations_block_start_and_preserve_all_state() {
    isolated(
        "real_operations_block_start_and_preserve_all_state",
        true,
        |root| {
            for operation in [
                "merge",
                "rebase",
                "rebase-apply",
                "cherry-pick",
                "revert",
                "am",
                "bisect",
            ] {
                let (repo, base, other) = conflicting(root);
                let args = match operation {
                    "merge" => vec!["merge", "other"],
                    "rebase" => vec!["rebase", "other"],
                    "rebase-apply" => vec!["rebase", "--apply", "other"],
                    "cherry-pick" => vec!["cherry-pick", &other],
                    "revert" => vec!["revert", &other],
                    "am" => {
                        let patch = repo
                            .success(&["format-patch", "--stdout", "-1", &other])
                            .stdout;
                        let path = repo.path().join(".git/mail.patch");
                        fs::write(&path, patch).unwrap();
                        assert!(
                            !git::run(repo.path(), ["am".as_ref(), path.as_os_str()])
                                .unwrap()
                                .status
                                .success()
                        );
                        Vec::new()
                    }
                    _ => {
                        repo.success(&["bisect", "start", "HEAD", &base]);
                        Vec::new()
                    }
                };
                if !args.is_empty() {
                    assert!(!git::run(repo.path(), args).unwrap().status.success());
                }
                if matches!(operation, "am" | "bisect") {
                    let before = snapshot(root);
                    let output = std::process::Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
                        .current_dir(repo.path())
                        .arg("status")
                        .output()
                        .unwrap();
                    assert!(output.status.success());
                    assert!(
                        String::from_utf8(output.stdout)
                            .unwrap()
                            .contains(&format!("Operations: {operation}"))
                    );
                    assert_eq!(snapshot(root), before, "status changed operation state");
                }
                for target in ["main", "new"] {
                    // Choose main as target by changing the protected base setting.
                    fs::write(
                        repo.path().join(".git/git-oneflow.config.toml"),
                        "base='trunk'\n",
                    )
                    .unwrap();
                    let failure = preserved_failure(root, repo.path(), target);
                    let StartError::Operation(ops) = failure.error else {
                        panic!("{operation}: {failure:?}")
                    };
                    match operation {
                        "merge" => assert!(ops.merge),
                        "rebase" | "rebase-apply" => assert!(ops.rebase),
                        "cherry-pick" => assert!(ops.cherry_pick),
                        "revert" => assert!(ops.revert),
                        "am" => assert!(ops.am && !ops.rebase),
                        _ => assert!(ops.bisect),
                    }
                }
            }
        },
    );
}

#[test]
fn stale_deleted_remote_branches_and_restrictive_fetch_mappings() {
    isolated(
        "stale_deleted_remote_branches_and_restrictive_fetch_mappings",
        true,
        |root| {
            for mapping in ["restricted", "negative"] {
                let (remote, local) = pair(root);
                remote.success(&["branch", "deleted"]);
                local.success(&["fetch", "origin"]);
                assert!(
                    git::run(
                        local.path(),
                        [
                            "show-ref",
                            "--verify",
                            "--quiet",
                            "refs/remotes/origin/deleted"
                        ]
                    )
                    .unwrap()
                    .status
                    .success()
                );
                remote.success(&["branch", "-D", "deleted"]);
                let fresh = commit(&remote, "tracked", "fresh\n");
                remote.success(&["branch", "remote-only"]);
                if mapping == "restricted" {
                    local.success(&[
                        "config",
                        "remote.origin.fetch",
                        "+refs/heads/main:refs/remotes/origin/main",
                    ]);
                } else {
                    local.success(&[
                        "config",
                        "--add",
                        "remote.origin.fetch",
                        "^refs/heads/remote-only",
                    ]);
                }
                let outcome = start(local.path(), "remote-only").unwrap();
                branch(&outcome.state, "remote-only", &fresh);
                assert!(matches!(outcome.action, Action::ResumeRemote));
                assert!(
                    matches!(outcome.state.upstream, Upstream::Configured { ref merge_refs, .. } if merge_refs == &[b"refs/heads/remote-only".to_vec()])
                );
                local.success(&["switch", "main"]);
                let outcome = start(local.path(), "deleted").unwrap();
                branch(&outcome.state, "deleted", &fresh);
                assert!(matches!(outcome.action, Action::Create));
                assert_eq!(outcome.state.upstream, Upstream::Absent);
            }
        },
    );
}

#[test]
fn missing_remote_and_base_fail_without_cached_fallback() {
    isolated(
        "missing_remote_and_base_fail_without_cached_fallback",
        true,
        |root| {
            let (remote, local) = pair(root);
            local.success(&[
                "remote",
                "set-url",
                "origin",
                "/oneflow-unreachable-fixture",
            ]);
            let failure = preserved_failure(root, local.path(), "new");
            assert!(matches!(failure.error, StartError::Git { .. }));
            assert!(failure.to_string().contains("resolve the refusal"));
            local.success(&[
                "remote".as_ref(),
                "set-url".as_ref(),
                "origin".as_ref(),
                remote.path().as_os_str(),
            ]);
            remote.success(&["switch", "-c", "other"]);
            remote.success(&["branch", "-D", "main"]);
            // A tag with the configured base name must never be used instead.
            remote.success(&["tag", "main"]);
            assert!(matches!(
                preserved_failure(root, local.path(), "new").error,
                StartError::BaseUnavailable { .. }
            ));
            remote.success(&["branch", "remote-only"]);
            assert!(matches!(
                start(local.path(), "remote-only").unwrap().action,
                Action::ResumeRemote
            ));
            local.success(&["switch", "main"]);
            local.success(&["remote", "remove", "origin"]);
            // Even a local directory named origin is not a configured remote.
            fs::create_dir(local.path().join("origin")).unwrap();
            fs::create_dir_all(local.path().join(".git/info")).unwrap();
            fs::write(local.path().join(".git/info/exclude"), "origin/\n").unwrap();
            assert!(matches!(
                preserved_failure(root, local.path(), "new").error,
                StartError::Git { .. }
            ));
        },
    );
}

#[test]
fn overwrite_collisions_and_partial_switch_failures_keep_work() {
    isolated(
        "overwrite_collisions_and_partial_switch_failures_keep_work",
        true,
        |root| {
            let (remote, local) = pair(root);
            commit(&local, ".gitignore", "collision\n");
            remote.success(&["switch", "-c", "remote-only"]);
            let target = commit(&remote, "collision", "remote contents\n");
            fs::write(local.path().join("collision"), "precious ignored file\n").unwrap();
            let initial = oid(&local, "HEAD");
            let failure = start(local.path(), "remote-only").unwrap_err();
            assert!(matches!(failure.error, StartError::Git { .. }));
            assert_eq!(
                fs::read_to_string(local.path().join("collision")).unwrap(),
                "precious ignored file\n"
            );
            branch(remaining(&failure), "main", &initial);
            assert_eq!(
                failure.remaining.target_commit.as_ref().unwrap().as_deref(),
                Some(target.as_bytes())
            );
            assert_eq!(failure.effects.len(), 4); // fetched, created, two tracking keys
            assert_eq!(
                failure.remaining.target_upstream.as_ref().unwrap(),
                &start::TargetUpstream {
                    remotes: vec![b"origin".to_vec()],
                    merge_refs: vec![b"refs/heads/remote-only".to_vec()],
                }
            );
            // No rollback removes the prepared branch; after preserving the file a
            // retry is a local resume and does not require the remote.
            fs::rename(local.path().join("collision"), root.join("saved-work")).unwrap();
            local.success(&[
                "remote",
                "set-url",
                "origin",
                "/oneflow-unreachable-fixture",
            ]);
            assert_eq!(
                start(local.path(), "remote-only").unwrap().action,
                Action::ResumeLocal
            );
        },
    );
}

#[test]
fn index_and_config_locks_report_partial_effects_without_removing_locks() {
    isolated(
        "index_and_config_locks_report_partial_effects_without_removing_locks",
        true,
        |root| {
            for lock in ["index.lock", "config.lock"] {
                let (remote, local) = pair(root);
                remote.success(&["branch", "target"]);
                let initial = oid(&local, "HEAD");
                let path = local.path().join(".git").join(lock);
                fs::write(&path, "existing lock\n").unwrap();
                let failure = start(local.path(), "target").unwrap_err();
                assert!(matches!(failure.error, StartError::Git { .. }));
                assert_eq!(fs::read_to_string(&path).unwrap(), "existing lock\n");
                branch(remaining(&failure), "main", &initial);
                assert!(failure.remaining.target_commit.as_ref().unwrap().is_some());
                assert_eq!(
                    failure.effects.len(),
                    if lock == "index.lock" { 4 } else { 2 }
                );
                assert!(
                    !failure
                        .effects
                        .iter()
                        .any(|e| matches!(e, Effect::Switched { .. }))
                );
            }
        },
    );
}

#[test]
fn exact_literal_branches_win_over_tags_and_revision_shorthands() {
    isolated(
        "exact_literal_branches_win_over_tags_and_revision_shorthands",
        true,
        |root| {
            for name in ["refs/heads/odd", "a;echo-pwned", "feature/École"] {
                let repo = Repository::new(root);
                let tag = commit(&repo, "tracked", "tag\n");
                repo.success(&["tag", name]);
                let target = commit(&repo, "tracked", "branch history\n");
                repo.success(&["branch", name]);
                let initial = commit(&repo, "tracked", "current history\n");
                let outcome = start(repo.path(), name).unwrap();
                branch(&outcome.state, name, &target);
                assert_eq!(oid(&repo, "refs/heads/main"), initial);
                assert_eq!(oid(&repo, &format!("refs/tags/{name}")), tag);
                assert_eq!(outcome.action, Action::ResumeLocal);
            }
        },
    );
}

#[test]
fn old_or_forged_requests_are_revalidated() {
    isolated("old_or_forged_requests_are_revalidated", true, |root| {
        let repo = Repository::new(root);
        commit(&repo, "tracked", "base\n");
        let request = start::prepare(repo.path(), "target").unwrap();
        fs::write(
            repo.path().join(".git/git-oneflow.config.toml"),
            "base='trunk'\n",
        )
        .unwrap();
        let before = snapshot(root);
        assert!(matches!(
            start::execute(repo.path(), &request).unwrap_err().error,
            StartError::Changed
        ));
        assert_eq!(snapshot(root), before);
        let mut invalid = request;
        invalid.target = "main~1".into();
        assert!(matches!(
            start::execute(repo.path(), &invalid).unwrap_err().error,
            StartError::Resolve(start::ResolveError::InvalidTarget(_))
        ));
        assert_eq!(snapshot(root), before);
    });
}

#[cfg(unix)]
fn shell_path(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[cfg(unix)]
fn executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// The local transport mutates fixture state at the second connection (fetch),
/// after discovery completed. No sleeps, network servers or global config.
#[cfg(unix)]
fn transport_probe(local: &Repository, on_fetch: &str) {
    let script = local.path().join(".git/upload pack probe");
    let counter = local.path().join(".git/transport-count");
    executable(
        &script,
        &format!(
            "#!/bin/sh\ncount=0\nif test -f {counter}; then count=$(cat {counter}); fi\ncount=$((count + 1))\nprintf '%s' \"$count\" > {counter}\nif test \"$count\" = 2; then\n{on_fetch}\nfi\nexec git-upload-pack \"$@\"\n",
            counter = shell_path(&counter)
        ),
    );
    local.success(&["config", "remote.origin.uploadpack", &shell_path(&script)]);
}

#[cfg(unix)]
#[test]
fn remote_fetch_failures_do_not_fall_back_to_cached_refs() {
    isolated(
        "remote_fetch_failures_do_not_fall_back_to_cached_refs",
        true,
        |root| {
            for scenario in ["transport", "deleted-target", "deleted-base"] {
                let (remote, local) = pair(root);
                let target = if scenario == "deleted-target" {
                    remote.success(&["branch", "target"]);
                    local.success(&["fetch", "origin"]);
                    "target"
                } else {
                    "new"
                };
                let on_fetch = match scenario {
                    "transport" => "printf 'fixture fetch failure\\n' >&2\nexit 1".into(),
                    "deleted-target" => format!(
                        "git -C {} update-ref -d refs/heads/target",
                        shell_path(remote.path())
                    ),
                    _ => format!(
                        "git -C {} update-ref -d refs/heads/main",
                        shell_path(remote.path())
                    ),
                };
                transport_probe(&local, &on_fetch);
                let initial = oid(&local, "HEAD");
                let failure = start(local.path(), target).unwrap_err();
                assert!(
                    matches!(failure.error, StartError::Git { .. }),
                    "{failure:?}"
                );
                assert!(failure.effects.is_empty());
                assert_eq!(failure.remaining.target_commit.as_ref().unwrap(), &None);
                assert!(
                    failure
                        .remaining
                        .fetched_reference
                        .as_ref()
                        .unwrap()
                        .commit
                        .as_ref()
                        .unwrap()
                        .is_some()
                        || scenario != "deleted-base"
                );
                branch(remaining(&failure), "main", &initial);
                assert_eq!(
                    fs::read_to_string(local.path().join(".git/transport-count")).unwrap(),
                    "2"
                );
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn execution_rechecks_changes_after_remote_inspection_and_fetch() {
    isolated(
        "execution_rechecks_changes_after_remote_inspection_and_fetch",
        true,
        |root| {
            for scenario in [
                "dirty",
                "operation",
                "head",
                "target",
                "worktree",
                "settings",
            ] {
                let (remote, local) = pair(root);
                let initial = oid(&local, "HEAD");
                let git = format!("git -C {}", shell_path(local.path()));
                let on_fetch = match scenario {
                    "dirty" => format!(
                        "printf 'new work\\n' > {}",
                        shell_path(&local.path().join("tracked"))
                    ),
                    "operation" => format!("{git} bisect start >&2"),
                    "head" => format!("{git} switch -c changed"),
                    "target" => format!("{git} branch new"),
                    "worktree" => format!(
                        "{git} worktree add -b new {} >&2",
                        shell_path(&root.join("occupied tree"))
                    ),
                    _ => format!(
                        "printf 'base=\"trunk\"\\n' > {}",
                        shell_path(&local.path().join(".git/git-oneflow.config.toml"))
                    ),
                };
                transport_probe(&local, &on_fetch);
                let failure = start(local.path(), "new").unwrap_err();
                assert_eq!(failure.effects.len(), 1, "{scenario}: {failure:?}");
                assert!(matches!(failure.effects[0], Effect::Fetched { .. }));
                match scenario {
                    "dirty" => assert!(matches!(failure.error, StartError::Dirty)),
                    "operation" => assert!(matches!(failure.error, StartError::Operation(_))),
                    "worktree" => assert!(matches!(failure.error, StartError::Occupied { .. })),
                    _ => assert!(matches!(failure.error, StartError::Changed)),
                }
                assert_eq!(oid(&local, "refs/heads/main"), initial);
                assert_eq!(oid(&remote, "refs/heads/main"), initial);
                if scenario == "dirty" {
                    assert_eq!(
                        fs::read_to_string(local.path().join("tracked")).unwrap(),
                        "new work\n"
                    );
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn base_updated_between_advertisement_and_fetch_is_used() {
    isolated(
        "base_updated_between_advertisement_and_fetch_is_used",
        true,
        |root| {
            let (remote, local) = pair(root);
            remote.success(&["switch", "-c", "update"]);
            let new_base = commit(&remote, "tracked", "new base\n");
            transport_probe(
                &local,
                &format!(
                    "git -C {} update-ref refs/heads/main {new_base}",
                    shell_path(remote.path())
                ),
            );
            let outcome = start(local.path(), "new").unwrap();
            branch(&outcome.state, "new", &new_base);
            assert_eq!(outcome.state.upstream, Upstream::Absent);
        },
    );
}

#[cfg(unix)]
#[test]
fn failing_checkout_hook_reports_the_actual_switched_state() {
    isolated(
        "failing_checkout_hook_reports_the_actual_switched_state",
        true,
        |root| {
            let repo = Repository::new(root);
            let base = commit(&repo, "tracked", "base\n");
            repo.success(&["branch", "target"]);
            let hooks = repo.path().join(".git/hooks-for-test");
            fs::create_dir(&hooks).unwrap();
            executable(
                &hooks.join("post-checkout"),
                "#!/bin/sh\nprintf 'hook refused\\n' >&2\nexit 1\n",
            );
            repo.success(&[
                "config".as_ref(),
                "core.hooksPath".as_ref(),
                hooks.as_os_str(),
            ]);
            let failure = start(repo.path(), "target").unwrap_err();
            assert!(matches!(failure.error, StartError::Git { .. }));
            // Git returned nonzero even though it already switched. The failed step
            // is not declared completed, but the observed state is honest.
            assert!(failure.effects.is_empty());
            branch(remaining(&failure), "target", &base);
            let before = snapshot(root);
            assert_eq!(
                start(repo.path(), "target").unwrap().action,
                Action::AlreadyCurrent
            );
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn orphan_upstream_configuration_is_not_silently_reused_or_overwritten() {
    isolated(
        "orphan_upstream_configuration_is_not_silently_reused_or_overwritten",
        true,
        |root| {
            let (_, local) = pair(root);
            local.success(&["config", "branch.new.remote", "origin"]);
            local.success(&["config", "branch.new.merge", "refs/heads/main"]);
            assert!(matches!(
                preserved_failure(root, local.path(), "new").error,
                StartError::ExistingUpstreamConfiguration
            ));
        },
    );
}

#[test]
fn unusual_git_switch_refusals_preserve_the_exact_local_branch() {
    isolated(
        "unusual_git_switch_refusals_preserve_the_exact_local_branch",
        true,
        |root| {
            let repo = Repository::new(root);
            commit(&repo, "tracked", "base\n");
            repo.success(&["branch", "@"]);
            let failure = preserved_failure(root, repo.path(), "@");
            assert!(matches!(failure.error, StartError::Git { .. }));
            assert_eq!(failure.action, Some(Action::ResumeLocal));
        },
    );
}

#[test]
fn nested_directory_removed_by_switch_still_reports_the_remaining_state() {
    isolated(
        "nested_directory_removed_by_switch_still_reports_the_remaining_state",
        true,
        |root| {
            let repo = Repository::new(root);
            let base = commit(&repo, "tracked", "base\n");
            repo.success(&["branch", "target"]);
            fs::create_dir(repo.path().join("nested")).unwrap();
            commit(&repo, "nested/file", "nested file\n");
            let outcome = start(&repo.path().join("nested"), "target").unwrap();
            assert!(!repo.path().join("nested").exists());
            branch(&outcome.state, "target", &base);
            assert_eq!(outcome.action, Action::ResumeLocal);
        },
    );
}

#[test]
fn established_absent_workstream_policy_stops_start_before_recreation() {
    use git_oneflow::settings::{self, WorkstreamKind};
    isolated(
        "established_absent_workstream_policy_stops_start_before_recreation",
        true,
        |root| {
            let (_remote, local) = pair(root);
            let mut settings = settings::Settings::default();
            settings::oneflow_bundle(&mut settings, None);
            fs::write(
                local.path().join(".git/git-oneflow.config.toml"),
                git_oneflow::config::render(git_oneflow::config::Format::Toml, &settings),
            )
            .unwrap();
            let options = start::Options {
                policy: Some(WorkstreamKind::Release),
                from: Some(oid(&local, "main")),
                ..Default::default()
            };
            let request =
                start::prepare_with_options(local.path(), "release/train", &options).unwrap();
            start::execute(local.path(), &request).unwrap();
            local.success(&["switch", "main"]);
            local.success(&["update-ref", "-d", "refs/heads/release/train"]);
            let before = snapshot(root);
            for options in [&options, &start::Options::default()] {
                let error = start::prepare_with_options(local.path(), "release/train", options)
                    .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("established work branch is absent")
                );
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn configured_hotfix_exact_version_tag_is_explicit_creation_input() {
    use git_oneflow::settings::{self, WorkstreamKind};
    isolated(
        "configured_hotfix_exact_version_tag_is_explicit_creation_input",
        true,
        |root| {
            let (_remote, local) = pair(root);
            let version = oid(&local, "main");
            for annotated in [false, true] {
                let tag = if annotated {
                    "Train-annotated"
                } else {
                    "Train-lightweight"
                };
                if annotated {
                    local.success(&["tag", "-a", "-m", "version", tag, &version]);
                } else {
                    local.success(&["tag", tag, &version]);
                }
                let mut settings = settings::Settings::default();
                settings::oneflow_bundle(&mut settings, None);
                settings
                    .workstreams
                    .get_mut(&WorkstreamKind::Hotfix)
                    .unwrap()
                    .creation_source = Some(format!("refs/tags/{tag}"));
                fs::write(
                    local.path().join(".git/git-oneflow.config.toml"),
                    git_oneflow::config::render(git_oneflow::config::Format::Toml, &settings),
                )
                .unwrap();
                let branch = format!("hotfix/{tag}");
                let options = start::Options {
                    policy: Some(WorkstreamKind::Hotfix),
                    ..Default::default()
                };
                let request = start::prepare_with_options(local.path(), &branch, &options).unwrap();
                assert_eq!(request.source_commit.as_deref(), Some(version.as_bytes()));
                start::execute(local.path(), &request).unwrap();
                assert_eq!(oid(&local, "HEAD"), version);
                local.success(&["switch", "main"]);
            }
        },
    );
}

#[test]
fn established_hotfix_route_change_and_raw_save_association_change_are_guarded() {
    use git_oneflow::{
        save,
        settings::{self, WorkstreamKind},
    };
    isolated(
        "established_hotfix_route_change_and_raw_save_association_change_are_guarded",
        true,
        |root| {
            let (_remote, local) = pair(root);
            let mut proposed = settings::Settings::default();
            settings::oneflow_bundle(&mut proposed, None);
            local.success(&["tag", "Train-version"]);
            let path = local.path().join(".git/git-oneflow.config.toml");
            let original =
                git_oneflow::config::render(git_oneflow::config::Format::Toml, &proposed);
            fs::write(&path, &original).unwrap();
            let options = start::Options {
                policy: Some(WorkstreamKind::Hotfix),
                from_tag: Some("Train-version".into()),
                ..Default::default()
            };
            let request =
                start::prepare_with_options(local.path(), "hotfix/train", &options).unwrap();
            start::execute(local.path(), &request).unwrap();
            proposed
                .workstreams
                .get_mut(&WorkstreamKind::Hotfix)
                .unwrap()
                .active_release = Some("release/train".into());
            fs::write(
                &path,
                git_oneflow::config::render(git_oneflow::config::Format::Toml, &proposed),
            )
            .unwrap();
            let before = snapshot(root);
            assert!(
                start::prepare(local.path(), "hotfix/train")
                    .unwrap_err()
                    .to_string()
                    .contains("established workstream routes changed")
            );
            assert_eq!(snapshot(root), before);
            fs::write(path, original).unwrap();
            let plan = save::assess(local.path(), &save::Request::default()).unwrap();
            let key = "branch.hotfix/train.oneflow-policy";
            let raw = local.success(&["config", "--get", key]).stdout;
            let mut record: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            record["creation_input"] =
                serde_json::Value::String("refs/tags/another-same-commit-name".into());
            local.success(&[
                "config",
                "--local",
                "--replace-all",
                key,
                &serde_json::to_string(&record).unwrap(),
            ]);
            let before = snapshot(root);
            assert!(
                save::execution::execute(local.path(), &plan)
                    .error
                    .unwrap()
                    .to_string()
                    .contains("changed")
            );
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn advanced_published_hotfix_resume_selects_policy_without_new_creation_input() {
    use git_oneflow::settings::{self, WorkstreamKind};
    isolated(
        "advanced_published_hotfix_resume_selects_policy_without_new_creation_input",
        true,
        |root| {
            for configured in [
                None,
                Some("refs/tags/Train-base"),
                Some("refs/tags/locally-missing-version"),
            ] {
                let (remote, local) = pair(root);
                let base = oid(&local, "main");
                local.success(&["tag", "Train-base", &base]);
                remote.success(&["switch", "-c", "hotfix/train"]);
                let published = commit(&remote, "hotfix", "published advanced fix\n");
                let mut proposed = settings::Settings::default();
                settings::oneflow_bundle(&mut proposed, None);
                proposed
                    .workstreams
                    .get_mut(&WorkstreamKind::Hotfix)
                    .unwrap()
                    .creation_source = configured.map(str::to_owned);
                fs::write(
                    local.path().join(".git/git-oneflow.config.toml"),
                    git_oneflow::config::render(git_oneflow::config::Format::Toml, &proposed),
                )
                .unwrap();
                let options = start::Options {
                    policy: Some(WorkstreamKind::Hotfix),
                    ..Default::default()
                };
                let request =
                    start::prepare_with_options(local.path(), "hotfix/train", &options).unwrap();
                let result = start::execute(local.path(), &request).unwrap();
                assert_eq!(result.action, Action::ResumeRemote);
                assert_eq!(oid(&local, "HEAD"), published);
                assert_eq!(oid(&local, "main"), base);
                let raw = local
                    .success(&["config", "--get", "branch.hotfix/train.oneflow-policy"])
                    .stdout;
                let record: serde_json::Value = serde_json::from_slice(&raw).unwrap();
                assert_eq!(record["policy"]["kind"], "hotfix");
                assert_eq!(record["creation_commit"], published);
                assert_eq!(record["creation_input"], serde_json::Value::Null);
                let before = snapshot(root);
                let resumed =
                    start::prepare_with_options(local.path(), "hotfix/train", &options).unwrap();
                assert_eq!(
                    start::execute(local.path(), &resumed).unwrap().action,
                    Action::AlreadyCurrent
                );
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn fresh_hotfix_without_explicit_version_stops_before_fetch_or_branch_creation() {
    use git_oneflow::settings::{self, WorkstreamKind};
    isolated(
        "fresh_hotfix_without_explicit_version_stops_before_fetch_or_branch_creation",
        true,
        |root| {
            let (_remote, local) = pair(root);
            let mut proposed = settings::Settings::default();
            settings::oneflow_bundle(&mut proposed, None);
            fs::write(
                local.path().join(".git/git-oneflow.config.toml"),
                git_oneflow::config::render(git_oneflow::config::Format::Toml, &proposed),
            )
            .unwrap();
            let options = start::Options {
                policy: Some(WorkstreamKind::Hotfix),
                ..Default::default()
            };
            let before = snapshot(root);
            let request =
                start::prepare_with_options(local.path(), "hotfix/new", &options).unwrap();
            assert_eq!(snapshot(root), before);
            let failure = start::execute(local.path(), &request).unwrap_err();
            assert!(failure.error.to_string().contains("--from-tag"));
            assert!(failure.effects.is_empty());
            assert_eq!(snapshot(root), before);
        },
    );
}
