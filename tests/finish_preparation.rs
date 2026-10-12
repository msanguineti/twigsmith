#![cfg(unix)]
mod support;
use git_oneflow::{
    finish::{
        self, Constraint, PublicationMode, Recovery, RefObservation, Request, RequiredInput,
        preparation::{self, Effect, PreparationError},
    },
    git,
    repository::{Head, Observation},
    settings::{FinishRewrite, FinishUpdate},
};
use std::{fs, path::Path, process::Command};
use support::{Repository, isolated};

fn oid(repo: &Repository, reference: &str) -> Vec<u8> {
    repo.success(&["rev-parse", reference])
        .stdout
        .strip_suffix(b"\n")
        .unwrap()
        .to_vec()
}
fn commit(repo: &Repository, file: &str, contents: &str) -> Vec<u8> {
    fs::write(repo.path().join(file), contents).unwrap();
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", contents]);
    oid(repo, "HEAD")
}
fn request(update: FinishUpdate, rewrite: FinishRewrite) -> Request {
    Request {
        update: Some(update),
        rewrite: Some(rewrite),
        history_edit: None,
    }
}
fn fixture(
    root: &Path,
    published: bool,
    conflict: bool,
) -> (Repository, Repository, Vec<u8>, Vec<u8>) {
    let remote = Repository::new(root);
    let base = commit(&remote, "tracked", "base\n");
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin"]);
    local.success(&["switch", "-c", "work", "--no-track", "origin/main"]);
    local.success(&["branch", "main", std::str::from_utf8(&base).unwrap()]);
    let work = commit(&local, if conflict { "tracked" } else { "work" }, "work\n");
    if published {
        publish_fixture(&local, &remote);
    }
    let new_base = commit(
        &remote,
        if conflict { "tracked" } else { "upstream" },
        "upstream\n",
    );
    (local, remote, work, new_base)
}
fn publish_fixture(local: &Repository, remote: &Repository) {
    remote.success(&[
        "fetch",
        "--no-tags",
        local.path().to_str().unwrap(),
        "+refs/heads/work:refs/heads/work",
    ]);
}
fn fresh(local: &Repository, req: &Request) -> finish::Plan {
    let initial = finish::assess(local.path(), req, None).unwrap();
    let report = preparation::refresh(local.path(), &initial);
    assert!(report.error.is_none(), "{report:#?}");
    report.plan.unwrap()
}
fn prepare(local: &Repository, req: &Request) -> preparation::Report {
    preparation::prepare(local.path(), &fresh(local, req))
}
fn ancestor(local: &Repository, older: &[u8], newer: &[u8]) -> bool {
    git::run(
        local.path(),
        [
            "merge-base",
            "--is-ancestor",
            std::str::from_utf8(older).unwrap(),
            std::str::from_utf8(newer).unwrap(),
        ],
    )
    .unwrap()
    .status
    .success()
}
fn record_path(local: &Repository) -> std::path::PathBuf {
    local
        .path()
        .join(".git/oneflow-finish/branches/work/rewrite.json")
}
fn hooks(local: &Repository, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = local.path().join(".git/finish-hooks");
    fs::create_dir_all(&path).unwrap();
    local.success(&["config", "core.hooksPath", path.to_str().unwrap()]);
    let file = path.join(name);
    fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn update_choices_use_remote_base_and_preserve_other_refs() {
    isolated(
        "update_choices_use_remote_base_and_preserve_other_refs",
        true,
        |root| {
            for update in [
                FinishUpdate::Rebase,
                FinishUpdate::Merge,
                FinishUpdate::None,
            ] {
                for published in [false, true] {
                    let (local, remote, work, base) = fixture(root, published, false);
                    local.success(&["switch", "main"]);
                    let local_main = commit(&local, "local-only", "divergent local main");
                    local.success(&["switch", "work"]);
                    local.success(&["branch", "unrelated"]);
                    local.success(&["tag", "local-tag"]);
                    local.success(&["config", "rebase.updateRefs", "true"]);
                    local.success(&["config", "rebase.autoSquash", "true"]);
                    local.success(&["config", "remote.origin.tagOpt", "--tags"]);
                    remote.success(&["tag", "remote-tag"]);
                    let report = prepare(&local, &request(update, FinishRewrite::Allow));
                    assert!(report.is_prepared(), "{report:#?}");
                    let after = oid(&local, "HEAD");
                    assert_eq!(oid(&local, "main"), local_main);
                    assert_eq!(oid(&local, "unrelated"), work);
                    assert_eq!(oid(&local, "local-tag"), work);
                    assert!(
                        !git::run(
                            local.path(),
                            ["show-ref", "--verify", "refs/tags/remote-tag"]
                        )
                        .unwrap()
                        .status
                        .success()
                    );
                    assert_eq!(
                        report.evidence.as_ref().unwrap().base,
                        RefObservation::Commit(base.clone())
                    );
                    assert_eq!(oid(&remote, "main"), base);
                    if published {
                        assert_eq!(oid(&remote, "work"), work);
                    }
                    if update == FinishUpdate::None {
                        assert_eq!(after, work);
                    } else {
                        assert!(ancestor(&local, &base, &after));
                        assert!(!ancestor(&local, &local_main, &after));
                    }
                    if update == FinishUpdate::Merge {
                        assert!(ancestor(&local, &work, &after));
                    }
                    if update == FinishUpdate::Rebase && published {
                        assert_eq!(
                            report.plan.as_ref().unwrap().destinations[0].mode,
                            PublicationMode::VerifiedRewrite
                        );
                    }
                }
            }
        },
    );
}

#[test]
fn already_updated_branch_and_fast_forward_rebase() {
    isolated(
        "already_updated_branch_and_fast_forward_rebase",
        true,
        |root| {
            let (local, remote, _, base) = fixture(root, true, false);
            let report = prepare(&local, &request(FinishUpdate::Merge, FinishRewrite::Deny));
            assert!(report.is_prepared(), "{report:#?}");
            let head = oid(&local, "HEAD");
            let report = prepare(&local, &Request::default());
            assert!(report.is_prepared(), "{report:#?}");
            assert_eq!(head, oid(&local, "HEAD"));
            assert!(
                !report
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::Updated { .. } | Effect::RecoveryRecorded(_)))
            );
            local.success(&["reset", "--hard", "main"]);
            remote.success(&["branch", "-D", "work"]);
            let report = prepare(&local, &request(FinishUpdate::Rebase, FinishRewrite::Deny));
            assert!(report.is_prepared(), "{report:#?}");
            assert_eq!(oid(&local, "HEAD"), base);
        },
    );
}

#[test]
fn rebase_keeps_merge_structure_and_other_worktree_branches() {
    isolated(
        "rebase_keeps_merge_structure_and_other_worktree_branches",
        true,
        |root| {
            let (local, _remote, _, base) = fixture(root, false, false);
            local.success(&["switch", "-c", "side", "main"]);
            let side = commit(&local, "side", "side");
            local.success(&["switch", "work"]);
            local.success(&["merge", "--no-ff", "side", "-m", "merge side"]);
            local.success(&["config", "rebase.updateRefs", "true"]);
            let linked = root.join("linked");
            local.success(&["worktree", "add", linked.to_str().unwrap(), "side"]);
            let report = prepare(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow));
            assert!(report.is_prepared(), "{report:#?}");
            assert_eq!(oid(&local, "side"), side);
            assert!(ancestor(&local, &base, &oid(&local, "HEAD")));
            assert_eq!(
                local
                    .success(&["rev-list", "--count", "--merges", "main..work"])
                    .stdout,
                b"1\n"
            );
        },
    );
}

#[test]
fn authorization_and_known_remote_work_are_independent() {
    isolated(
        "authorization_and_known_remote_work_are_independent",
        true,
        |root| {
            let (local, remote, work, _) = fixture(root, true, false);
            for rewrite in [FinishRewrite::Ask, FinishRewrite::Deny] {
                let plan = fresh(&local, &request(FinishUpdate::Rebase, rewrite));
                assert!(!plan.is_ready());
                if rewrite == FinishRewrite::Ask {
                    assert_eq!(plan.required_input, [RequiredInput::Rewrite]);
                }
                let report = preparation::prepare(local.path(), &plan);
                assert!(matches!(
                    report.error,
                    Some(PreparationError::UnresolvedPlan)
                ));
                assert!(report.commands.is_empty());
                assert!(!record_path(&local).exists());
                assert_eq!(oid(&local, "HEAD"), work);
            }
            remote.success(&["switch", "work"]);
            commit(&remote, "someone-else", "remote work");
            let plan = fresh(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow));
            assert!(
                plan.constraints
                    .iter()
                    .any(|c| matches!(c, Constraint::RemoteWorkNotIncorporated(_)))
            );
            assert!(!preparation::prepare(local.path(), &plan).is_prepared());
            assert_eq!(oid(&local, "HEAD"), work);
        },
    );
}

#[test]
fn refresh_observes_push_destinations_and_missing_history() {
    isolated(
        "refresh_observes_push_destinations_and_missing_history",
        true,
        |root| {
            let (local, fetch, work, _) = fixture(root, true, false);
            let push = Repository::new(root);
            local.success(&[
                "config",
                "remote.origin.pushurl",
                push.path().to_str().unwrap(),
            ]);
            let plan = fresh(&local, &request(FinishUpdate::Rebase, FinishRewrite::Deny));
            assert!(plan.is_ready());
            assert_eq!(plan.destinations[0].observed, RefObservation::Absent);
            publish_fixture(&local, &push);
            let plan = fresh(&local, &request(FinishUpdate::Rebase, FinishRewrite::Ask));
            assert_eq!(plan.destinations[0].observed, RefObservation::Commit(work));
            assert_eq!(plan.required_input, [RequiredInput::Rewrite]);
            local.success(&[
                "config",
                "--add",
                "remote.origin.pushurl",
                fetch.path().to_str().unwrap(),
            ]);
            let plan = fresh(&local, &request(FinishUpdate::None, FinishRewrite::Allow));
            assert_eq!(plan.destinations.len(), 2);
            fetch.success(&["switch", "work"]);
            fetch.success(&["branch", "-D", "main"]);
            let plan = fresh(&local, &request(FinishUpdate::None, FinishRewrite::Allow));
            assert_eq!(plan.evidence.as_ref().unwrap().base, RefObservation::Absent);
            assert!(plan.constraints.contains(&Constraint::BaseUnavailable));
            local.success(&[
                "config",
                "--replace-all",
                "remote.origin.pushurl",
                "/nonexistent/finish-destination",
            ]);
            let initial = finish::assess(local.path(), &Request::default(), None).unwrap();
            assert!(preparation::refresh(local.path(), &initial).error.is_some());
        },
    );
}

#[test]
fn guards_and_revalidation_stop_before_updates() {
    isolated(
        "guards_and_revalidation_stop_before_updates",
        true,
        |root| {
            for change in [
                "dirty",
                "branch",
                "upstream",
                "destination",
                "settings",
                "remote",
                "mapping",
            ] {
                let (local, remote, work, _) = fixture(root, false, false);
                let plan = fresh(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow));
                match change {
                    "dirty" => fs::write(local.path().join("dirty"), "dirty").unwrap(),
                    "branch" => {
                        local.success(&["switch", "-c", "other"]);
                    }
                    "upstream" => {
                        local.success(&["config", "branch.work.remote", "elsewhere"]);
                    }
                    "destination" => {
                        local.success(&["config", "remote.origin.pushurl", "/changed"]);
                    }
                    "settings" => fs::write(
                        local.path().join(".git/git-oneflow.config.toml"),
                        "finish-update = 'merge'\n",
                    )
                    .unwrap(),
                    "remote" => {
                        remote.success(&["fetch", local.path().to_str().unwrap(), "work:work"]);
                    }
                    _ => {
                        local.success(&[
                            "config",
                            "remote.origin.fetch",
                            "+refs/heads/*:refs/heads/*",
                        ]);
                    }
                }
                let report = preparation::prepare(local.path(), &plan);
                assert!(!report.is_prepared(), "{change}: {report:#?}");
                assert_eq!(oid(&local, "HEAD"), work);
                assert!(!record_path(&local).exists());
            }
        },
    );
}

#[test]
fn changed_configuration_during_update_requires_fresh_assessment() {
    isolated(
        "changed_configuration_during_update_requires_fresh_assessment",
        true,
        |root| {
            for update in [FinishUpdate::Rebase, FinishUpdate::Merge] {
                let (local, remote, _, _) = fixture(root, false, false);
                commit(
                    &remote,
                    "git-oneflow.config.toml",
                    "finish-update = 'none'\n",
                );
                let report = prepare(&local, &request(update, FinishRewrite::Allow));
                assert!(
                    matches!(
                        report.error,
                        Some(PreparationError::Finish(
                            finish::FinishError::ContextChanged
                        ))
                    ),
                    "{report:#?}"
                );
                assert!(
                    report
                        .effects
                        .iter()
                        .any(|e| matches!(e, Effect::Updated { .. }))
                );
                assert!(prepare(&local, &request(update, FinishRewrite::Allow)).is_prepared());
            }
            let (local, _remote, _, _) = fixture(root, true, false);
            hooks(&local, "post-rewrite", "git config finish.example changed");
            let report = prepare(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow));
            assert!(
                matches!(
                    report.error,
                    Some(PreparationError::Finish(
                        finish::FinishError::ContextChanged
                    ))
                ),
                "{report:#?}"
            );
            assert!(matches!(
                report.plan.unwrap().recovery,
                Recovery::Verified(_)
            ));
        },
    );
}

#[test]
fn rebase_conflict_continuation_and_abortion_recover_safely() {
    isolated(
        "rebase_conflict_continuation_and_abortion_recover_safely",
        true,
        |root| {
            for abort in [false, true] {
                let (local, _remote, work, _) = fixture(root, true, true);
                local.success(&["config", "core.editor", "true"]);
                let report = prepare(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow));
                assert!(
                    matches!(report.error, Some(PreparationError::CommandFailed)),
                    "{report:#?}"
                );
                assert!(report.guidance().contains("git rebase --continue"));
                assert!(record_path(&local).exists());
                let initial = finish::assess(local.path(), &Request::default(), None);
                assert!(initial.is_err());
                if abort {
                    local.success(&["rebase", "--abort"]);
                    assert_eq!(oid(&local, "HEAD"), work);
                    let plan = fresh(&local, &request(FinishUpdate::None, FinishRewrite::Deny));
                    assert!(plan.is_ready());
                    assert_eq!(plan.recovery, Recovery::Unchanged);
                } else {
                    fs::write(local.path().join("tracked"), "resolved both\n").unwrap();
                    local.success(&["add", "tracked"]);
                    local.success(&["rebase", "--continue"]);
                    child(&local, "retry");
                }
            }
        },
    );
}

#[test]
fn merge_conflict_retains_operation_and_supports_git_recovery() {
    isolated(
        "merge_conflict_retains_operation_and_supports_git_recovery",
        true,
        |root| {
            for abort in [true, false] {
                let (local, _remote, work, _) = fixture(root, true, true);
                local.success(&["config", "core.editor", "true"]);
                let report = prepare(&local, &request(FinishUpdate::Merge, FinishRewrite::Deny));
                assert!(!report.is_prepared());
                assert!(report.guidance().contains("git merge --continue"));
                assert!(!record_path(&local).exists());
                if abort {
                    local.success(&["merge", "--abort"]);
                    assert_eq!(oid(&local, "HEAD"), work);
                } else {
                    fs::write(local.path().join("tracked"), "resolved both").unwrap();
                    local.success(&["add", "tracked"]);
                    local.success(&["merge", "--continue"]);
                }
                let plan = fresh(&local, &request(FinishUpdate::None, FinishRewrite::Deny));
                assert!(plan.is_ready());
            }
        },
    );
}

fn child(local: &Repository, mode: &str) {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "preparation_child", "--nocapture"])
        .env("ONEFLOW_PREP_DIRECTORY", local.path())
        .env("ONEFLOW_PREP_MODE", mode)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
#[test]
fn preparation_child() {
    let Some(path) = std::env::var_os("ONEFLOW_PREP_DIRECTORY") else {
        return;
    };
    let mode = std::env::var("ONEFLOW_PREP_MODE").unwrap();
    let path = Path::new(&path);
    let req = request(FinishUpdate::Rebase, FinishRewrite::Allow);
    let initial = finish::assess(path, &req, None).unwrap();
    let fresh = preparation::refresh(path, &initial);
    assert!(fresh.error.is_none(), "{fresh:#?}");
    let plan = fresh.plan.unwrap();
    if mode == "retry" {
        assert!(matches!(plan.recovery, Recovery::Verified(_)), "{plan:#?}");
        assert_eq!(plan.destinations[0].mode, PublicationMode::VerifiedRewrite);
    }
    let report = preparation::prepare(path, &plan);
    assert!(report.is_prepared(), "{report:#?}");
}

#[test]
fn completed_rewrite_survives_new_process_and_publication_failure() {
    isolated(
        "completed_rewrite_survives_new_process_and_publication_failure",
        true,
        |root| {
            let (local, remote, work, _) = fixture(root, true, false);
            child(&local, "prepare");
            // An ordinary non-forced publication fails after the completed rewrite.
            let result = git::run(local.path(), ["push", "origin", "work:work"]).unwrap();
            assert!(!result.status.success());
            assert_eq!(oid(&remote, "work"), work);
            child(&local, "retry");
            let denied = fresh(&local, &request(FinishUpdate::None, FinishRewrite::Deny));
            assert!(!denied.is_ready());
            assert!(
                denied
                    .constraints
                    .iter()
                    .any(|c| matches!(c, Constraint::RewriteDenied(_)))
            );
        },
    );
}

#[test]
fn interrupted_parent_after_rewrite_can_retry_in_new_process() {
    isolated(
        "interrupted_parent_after_rewrite_can_retry_in_new_process",
        true,
        |root| {
            let (local, _remote, _, _) = fixture(root, true, false);
            // Kill the executor after real Git exits successfully, before its
            // result is collected. This makes the critical restart window
            // deterministic without racing Git's own final operation cleanup.
            use std::os::unix::fs::PermissionsExt;
            let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|p| p.join("git"))
                .find(|p| p.is_file())
                .unwrap();
            let wrapper = root.join("git-wrapper");
            fs::create_dir(&wrapper).unwrap();
            let quoted = format!("'{}'", real_git.to_str().unwrap().replace('\'', "'\\''"));
            fs::write(wrapper.join("git"), format!(
                "#!/bin/sh\nfor arg do\n if [ \"$arg\" = rebase ]; then\n  {quoted} \"$@\"\n  code=$?\n  kill -KILL \"$PPID\"\n  exit \"$code\"\n fi\ndone\nexec {quoted} \"$@\"\n"
            )).unwrap();
            fs::set_permissions(wrapper.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
            let paths = std::env::join_paths(
                std::iter::once(wrapper)
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap();
            let out = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "preparation_child", "--nocapture"])
                .env("ONEFLOW_PREP_DIRECTORY", local.path())
                .env("ONEFLOW_PREP_MODE", "killed")
                .env("PATH", paths)
                .output()
                .unwrap();
            assert!(!out.status.success());
            child(&local, "retry");
        },
    );
}

#[test]
fn stale_or_invalid_recovery_never_authorizes_remote_replacement() {
    isolated(
        "stale_or_invalid_recovery_never_authorizes_remote_replacement",
        true,
        |root| {
            for change in [
                "corrupt",
                "marker",
                "original",
                "base",
                "permission",
                "reflog",
                "reset",
                "remote",
            ] {
                let (local, remote, _, _) = fixture(root, true, false);
                let req = request(FinishUpdate::Rebase, FinishRewrite::Allow);
                assert!(prepare(&local, &req).is_prepared());
                match change {
                    "corrupt" => fs::write(record_path(&local), "{").unwrap(),
                    "marker" => fs::write(record_path(&local), r#"{"rewrite":true}"#).unwrap(),
                    "original" | "base" => {
                        let mut value: serde_json::Value =
                            serde_json::from_slice(&fs::read(record_path(&local)).unwrap())
                                .unwrap();
                        let oid = serde_json::to_value(oid(&local, "HEAD")).unwrap();
                        if change == "original" {
                            value["original"] = oid;
                        } else {
                            value["attempts"][0]["base"] = oid;
                        }
                        fs::write(record_path(&local), serde_json::to_vec(&value).unwrap())
                            .unwrap();
                    }
                    "permission" => {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(record_path(&local), fs::Permissions::from_mode(0o644))
                            .unwrap();
                    }
                    "reflog" => {
                        local.success(&["reflog", "expire", "--expire=now", "--all"]);
                    }
                    "reset" => {
                        local.success(&["reset", "--hard", "main"]);
                        commit(&local, "later", "unrelated later work");
                    }
                    _ => {
                        remote.success(&["switch", "work"]);
                        commit(&remote, "remote-work", "new remote work");
                    }
                }
                let plan = fresh(&local, &req);
                assert!(!plan.is_ready(), "{change}: {plan:#?}");
                assert!(
                    plan.constraints
                        .iter()
                        .any(|c| matches!(c, Constraint::RemoteWorkNotIncorporated(_))),
                    "{change}: {plan:#?}"
                );
                assert!(!preparation::prepare(local.path(), &plan).is_prepared());
            }
        },
    );
}

#[test]
fn active_git_interruption_preserves_rebase_state() {
    isolated(
        "active_git_interruption_preserves_rebase_state",
        true,
        |root| {
            let (local, _remote, _, _) = fixture(root, true, false);
            hooks(&local, "post-checkout", "kill -TERM \"$PPID\"");
            let report = prepare(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow));
            assert!(
                matches!(report.error, Some(PreparationError::CommandFailed)),
                "{report:#?}"
            );
            let Ok(Observation::Repository(state)) = report.remaining else {
                panic!("missing state")
            };
            assert!(state.operations.rebase);
            assert!(matches!(state.head, Head::Detached { .. }));
            fs::remove_file(local.path().join(".git/finish-hooks/post-checkout")).unwrap();
            local.success(&["rebase", "--continue"]);
            child(&local, "retry");
        },
    );
}

#[test]
fn repeated_rewrites_retain_proof_after_base_advances_and_partial_publication() {
    isolated(
        "repeated_rewrites_retain_proof_after_base_advances_and_partial_publication",
        true,
        |root| {
            let (local, remote, _, _) = fixture(root, true, false);
            let second = Repository::new(root);
            publish_fixture(&local, &second);
            local.success(&[
                "config",
                "--add",
                "remote.origin.pushurl",
                remote.path().to_str().unwrap(),
            ]);
            local.success(&[
                "config",
                "--add",
                "remote.origin.pushurl",
                second.path().to_str().unwrap(),
            ]);
            let req = request(FinishUpdate::Rebase, FinishRewrite::Allow);
            assert!(prepare(&local, &req).is_prepared());
            publish_fixture(&local, &second);
            commit(&remote, "base-again", "another base change");
            let plan = fresh(&local, &req);
            assert!(plan.is_ready(), "{plan:#?}");
            let result = preparation::prepare(local.path(), &plan);
            assert!(result.is_prepared(), "{result:#?}");
            assert!(
                fresh(&local, &req)
                    .destinations
                    .iter()
                    .all(|d| d.mode == PublicationMode::VerifiedRewrite)
            );
        },
    );
}

#[test]
fn aborted_second_rebase_preserves_first_rewrite_proof() {
    isolated(
        "aborted_second_rebase_preserves_first_rewrite_proof",
        true,
        |root| {
            let (local, remote, _, _) = fixture(root, true, false);
            let req = request(FinishUpdate::Rebase, FinishRewrite::Allow);
            assert!(prepare(&local, &req).is_prepared());
            let first = oid(&local, "HEAD");
            commit(&remote, "work", "conflicting remote addition");
            assert!(!prepare(&local, &req).is_prepared());
            local.success(&["rebase", "--abort"]);
            assert_eq!(oid(&local, "HEAD"), first);
            let plan = fresh(&local, &req);
            assert!(plan.is_ready(), "{plan:#?}");
            let report = preparation::prepare(local.path(), &plan);
            assert!(
                matches!(report.error, Some(PreparationError::CommandFailed)),
                "{report:#?}"
            );
            fs::write(local.path().join("work"), "resolved").unwrap();
            local.success(&["add", "work"]);
            local.success(&["-c", "core.editor=true", "rebase", "--continue"]);
            child(&local, "retry");
        },
    );
}

#[test]
fn refresh_stops_after_transport_changes_local_context() {
    isolated(
        "refresh_stops_after_transport_changes_local_context",
        true,
        |root| {
            let (local, _remote, work, _) = fixture(root, false, false);
            use std::os::unix::fs::PermissionsExt;
            let helper = root.join("upload-pack");
            let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
            fs::write(&helper, format!("#!/bin/sh\ngit --git-dir={} config branch.work.remote changed\nexec git-upload-pack \"$@\"\n", quote(local.path().join(".git").to_str().unwrap()))).unwrap();
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
            local.success(&[
                "config",
                "remote.origin.uploadpack",
                &quote(helper.to_str().unwrap()),
            ]);
            let plan = finish::assess(
                local.path(),
                &request(FinishUpdate::Rebase, FinishRewrite::Allow),
                None,
            )
            .unwrap();
            let result = preparation::refresh(local.path(), &plan);
            assert!(result.error.is_some());
            assert_eq!(result.commands.len(), 1);
            assert_eq!(oid(&local, "HEAD"), work);
            assert!(!record_path(&local).exists());
        },
    );
}

#[test]
fn linked_worktree_preparation_uses_private_worktree_evidence() {
    isolated(
        "linked_worktree_preparation_uses_private_worktree_evidence",
        true,
        |root| {
            let (local, _remote, _, _) = fixture(root, true, false);
            local.success(&["switch", "main"]);
            let linked = root.join("linked");
            local.success(&["worktree", "add", linked.to_str().unwrap(), "work"]);
            let req = request(FinishUpdate::Rebase, FinishRewrite::Allow);
            let initial = finish::assess(&linked, &req, None).unwrap();
            let report = preparation::refresh(&linked, &initial);
            let report = preparation::prepare(&linked, &report.plan.unwrap());
            assert!(report.is_prepared(), "{report:#?}");
            assert!(!record_path(&local).exists());
            let Effect::RecoveryRecorded(path) = report
                .effects
                .iter()
                .find(|e| matches!(e, Effect::RecoveryRecorded(_)))
                .unwrap()
            else {
                unreachable!()
            };
            assert!(path.starts_with(local.path().join(".git/worktrees")));
            assert_eq!(oid(&local, "HEAD"), oid(&local, "main"));
            let retry =
                preparation::refresh(&linked, &finish::assess(&linked, &req, None).unwrap());
            assert!(retry.plan.unwrap().is_ready());
        },
    );
}

#[test]
fn absent_reflogs_stop_before_rewriting() {
    isolated("absent_reflogs_stop_before_rewriting", true, |root| {
        let (local, _remote, work, _) = fixture(root, true, false);
        local.success(&["reflog", "expire", "--expire=now", "--all"]);
        let report = prepare(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow));
        assert!(!report.is_prepared());
        assert_eq!(oid(&local, "HEAD"), work);
        assert!(!record_path(&local).exists());
    });
}

#[test]
fn refresh_fetches_missing_push_history_without_trusting_fetch_branch() {
    isolated(
        "refresh_fetches_missing_push_history_without_trusting_fetch_branch",
        true,
        |root| {
            let (local, _fetch, work, _) = fixture(root, true, false);
            let push = Repository::new(root);
            publish_fixture(&local, &push);
            push.success(&["switch", "work"]);
            let remote_work = commit(&push, "remote-only", "remote-only");
            assert!(
                !git::run(
                    local.path(),
                    ["cat-file", "-e", std::str::from_utf8(&remote_work).unwrap()]
                )
                .unwrap()
                .status
                .success()
            );
            local.success(&[
                "config",
                "remote.origin.pushurl",
                push.path().to_str().unwrap(),
            ]);
            let plan = fresh(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow));
            assert_eq!(
                plan.destinations[0].observed,
                RefObservation::Commit(remote_work.clone())
            );
            assert!(
                plan.constraints
                    .iter()
                    .any(|c| matches!(c, Constraint::RemoteWorkNotIncorporated(_)))
            );
            assert!(
                git::run(
                    local.path(),
                    ["cat-file", "-e", std::str::from_utf8(&remote_work).unwrap()]
                )
                .unwrap()
                .status
                .success()
            );
            assert_eq!(oid(&local, "HEAD"), work);
        },
    );
}

#[test]
fn committed_configuration_changes_are_checked_from_nested_invocations() {
    isolated(
        "committed_configuration_changes_are_checked_from_nested_invocations",
        true,
        |root| {
            let remote = Repository::new(root);
            commit(&remote, "git-oneflow.config.toml", "# original\n");
            let local = Repository::new(root);
            local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
            local.success(&["fetch", "origin"]);
            local.success(&["switch", "-c", "work", "--no-track", "origin/main"]);
            fs::create_dir(local.path().join("nested")).unwrap();
            commit(&local, "nested/work", "local work");
            commit(
                &remote,
                "git-oneflow.config.toml",
                "# changed formatting, same settings\n",
            );
            let req = request(FinishUpdate::Rebase, FinishRewrite::Allow);
            let nested = local.path().join("nested");
            let initial = finish::assess(&nested, &req, None).unwrap();
            finish::revalidate(local.path(), &initial, None).unwrap();
            let report = preparation::refresh(&nested, &initial);
            assert!(report.error.is_none(), "{report:#?}");
            let report = preparation::prepare(&nested, &report.plan.unwrap());
            assert!(
                matches!(
                    report.error,
                    Some(PreparationError::Finish(
                        finish::FinishError::ContextChanged
                    ))
                ),
                "{report:#?}"
            );
            assert!(
                report
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::Updated { .. }))
            );
        },
    );
}

#[test]
fn aborted_finish_does_not_authorize_a_later_manual_rebase() {
    isolated(
        "aborted_finish_does_not_authorize_a_later_manual_rebase",
        true,
        |root| {
            let (local, _remote, _, base) = fixture(root, true, true);
            let req = request(FinishUpdate::Rebase, FinishRewrite::Allow);
            assert!(!prepare(&local, &req).is_prepared());
            local.success(&["rebase", "--abort"]);
            assert!(
                !git::run(
                    local.path(),
                    [
                        "rebase",
                        "--rebase-merges",
                        std::str::from_utf8(&base).unwrap()
                    ]
                )
                .unwrap()
                .status
                .success()
            );
            fs::write(local.path().join("tracked"), "resolved both").unwrap();
            local.success(&["add", "tracked"]);
            local.success(&["-c", "core.editor=true", "rebase", "--continue"]);
            let plan = fresh(&local, &req);
            assert_eq!(plan.recovery, Recovery::Invalid);
            assert!(!plan.is_ready());
            assert!(
                plan.constraints
                    .iter()
                    .any(|c| matches!(c, Constraint::RemoteWorkNotIncorporated(_)))
            );
        },
    );
}

#[test]
fn verified_rewrite_survives_preserving_commits_and_merges() {
    isolated(
        "verified_rewrite_survives_preserving_commits_and_merges",
        true,
        |root| {
            let (local, remote, _, _) = fixture(root, true, false);
            assert!(
                prepare(&local, &request(FinishUpdate::Rebase, FinishRewrite::Allow)).is_prepared()
            );
            commit(&local, "more-work", "more work");
            child(&local, "retry");
            commit(&remote, "new-base", "new base");
            let req = request(FinishUpdate::Merge, FinishRewrite::Allow);
            let result = prepare(&local, &req);
            assert!(result.is_prepared(), "{result:#?}");
            child(&local, "retry");
        },
    );
}

fn interactive_request() -> Request {
    Request {
        update: Some(FinishUpdate::Rebase),
        rewrite: Some(FinishRewrite::Allow),
        history_edit: Some(git_oneflow::settings::HistoryEdit::Interactive),
    }
}
fn interactive_editors(local: &Repository) {
    fs::write(
        local.path().join(".git/history-sequence"),
        "sed -i 's/^pick /reword /' \"$1\"\n",
    )
    .unwrap();
    fs::write(
        local.path().join(".git/history-message"),
        "printf 'Edited work\\n' > \"$1\"\n",
    )
    .unwrap();
    local.success(&["config", "sequence.editor", "sh .git/history-sequence"]);
    local.success(&["config", "core.editor", "sh .git/history-message"]);
}
#[test]
fn interactive_recovery_rejects_reset_changed_context_and_missing_retention() {
    isolated(
        "interactive_recovery_rejects_reset_changed_context_and_missing_retention",
        true,
        |root| {
            for mutation in ["reset", "context", "retention", "raw-anchor", "record"] {
                let (local, remote, original, _) = fixture(root, true, false);
                interactive_editors(&local);
                let req = interactive_request();
                let report = prepare(&local, &req);
                assert!(report.is_prepared(), "{mutation}: {report:#?}");
                let edited = oid(&local, "HEAD");
                assert_ne!(edited, original);
                match mutation {
                    "reset" => {
                        local.success(&["reset", "--hard", "HEAD"]);
                    }
                    "context" => {
                        local.success(&["config", "oneflow.changed", "true"]);
                    }
                    "retention" => {
                        let refs = local.success(&[
                            "for-each-ref",
                            "--format=%(refname)",
                            "refs/oneflow/finish/",
                        ]);
                        let reference = std::str::from_utf8(&refs.stdout).unwrap().trim();
                        local.success(&["update-ref", "-d", reference]);
                    }
                    "raw-anchor" => {
                        let log = local.path().join(".git/logs/refs/heads/work");
                        let bytes = fs::read(&log).unwrap();
                        // Preserve OIDs/actions/counts but alter the raw author anchor.
                        let text =
                            String::from_utf8(bytes)
                                .unwrap()
                                .replacen(" <", " changed <", 1);
                        fs::write(log, text).unwrap();
                    }
                    "record" => {
                        fs::remove_file(record_path(&local)).unwrap();
                    }
                    _ => unreachable!(),
                }
                let plan = fresh(&local, &req);
                assert!(
                    !matches!(plan.recovery, Recovery::Verified(_)),
                    "{mutation}: {:?}",
                    plan.recovery
                );
                assert!(!plan.is_ready(), "{mutation}: {:?}", plan.constraints);
                assert_eq!(oid(&remote, "work"), original);
            }
        },
    );
}
#[test]
fn interactive_edit_preserves_unincorporated_and_newly_published_work() {
    isolated(
        "interactive_edit_preserves_unincorporated_and_newly_published_work",
        true,
        |root| {
            let (local, remote, _, _) = fixture(root, true, false);
            interactive_editors(&local);
            let req = interactive_request();
            let report = prepare(&local, &req);
            assert!(report.is_prepared(), "{report:#?}");
            remote.success(&["switch", "work"]);
            let changed = commit(&remote, "concurrent", "concurrent remote work\n");
            let plan = fresh(&local, &req);
            assert!(!plan.is_ready(), "{plan:#?}");
            assert!(
                plan.constraints
                    .iter()
                    .any(|c| matches!(c, Constraint::RemoteWorkNotIncorporated(_)))
            );
            assert_eq!(oid(&remote, "work"), changed);
        },
    );
}
#[test]
fn interactive_edit_protects_ignored_paths_in_intermediate_commits() {
    isolated(
        "interactive_edit_protects_ignored_paths_in_intermediate_commits",
        true,
        |root| {
            let (local, remote, _, _) = fixture(root, false, false);
            interactive_editors(&local);
            commit(&local, "private-output", "tracked temporarily\n");
            local.success(&["rm", "private-output"]);
            local.success(&["commit", "-m", "remove temporary path"]);
            fs::create_dir_all(local.path().join(".git/info")).unwrap();
            fs::write(local.path().join(".git/info/exclude"), "private-output\n").unwrap();
            fs::write(
                local.path().join("private-output"),
                "precious ignored content\n",
            )
            .unwrap();
            let original = oid(&local, "HEAD");
            let report = prepare(&local, &interactive_request());
            assert!(!report.is_prepared(), "{report:#?}");
            assert!(
                report
                    .error
                    .unwrap()
                    .to_string()
                    .contains("ignored local path")
            );
            assert_eq!(oid(&local, "HEAD"), original);
            assert_eq!(
                fs::read_to_string(local.path().join("private-output")).unwrap(),
                "precious ignored content\n"
            );
            assert!(
                !git::run(remote.path(), ["show-ref", "--verify", "refs/heads/work"])
                    .unwrap()
                    .status
                    .success()
            );
        },
    );
}
