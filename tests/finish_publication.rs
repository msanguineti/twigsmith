#![cfg(unix)]
mod support;
use git_oneflow::{
    finish::{
        self, Plan, Recovery, Request, preparation,
        publication::{
            self, Action, DestinationState, PublicationError, PublicationState, TrackingState,
            TrackingWrite,
        },
    },
    git,
    settings::{FinishRewrite, FinishUpdate},
};
use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
use support::{Repository, isolated};

const BRANCH: &str = "feature/exact";
const REF: &str = "refs/heads/feature/exact";
fn oid_at(directory: &Path, reference: &str) -> Option<Vec<u8>> {
    let output = git::run(directory, ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| output.stdout.strip_suffix(b"\n").unwrap().to_vec())
}
fn oid(repo: &Repository, reference: &str) -> Vec<u8> {
    oid_at(repo.path(), reference).unwrap()
}
fn commit(repo: &Repository, file: &str) -> Vec<u8> {
    fs::write(repo.path().join(file), file).unwrap();
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", file]);
    oid(repo, "HEAD")
}
fn fixture(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base");
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin"]);
    local.success(&["switch", "--no-track", "-c", BRANCH, "origin/main"]);
    commit(&local, "work");
    (local, remote)
}
fn seed(local: &Repository, remote: &Repository, reference: &str) {
    remote.success(&[
        "fetch",
        "--no-tags",
        local.path().to_str().unwrap(),
        &format!("{reference}:{REF}"),
    ]);
}
fn destinations(local: &Repository, remotes: &[&Repository]) {
    for remote in remotes {
        local.success(&[
            "config",
            "--add",
            "remote.origin.pushurl",
            remote.path().to_str().unwrap(),
        ]);
    }
}
fn req(update: FinishUpdate, rewrite: FinishRewrite) -> Request {
    Request {
        update: Some(update),
        rewrite: Some(rewrite),
        ..Default::default()
    }
}
fn fresh_at(directory: &Path, request: &Request) -> Plan {
    let initial = finish::assess(directory, request, None).unwrap();
    let report = preparation::refresh(directory, &initial);
    assert!(report.error.is_none(), "{report:?}");
    report.plan.unwrap()
}
fn prepared(local: &Repository, update: FinishUpdate) -> Plan {
    let p = fresh_at(local.path(), &req(update, FinishRewrite::Allow));
    let r = preparation::prepare(local.path(), &p);
    assert!(r.is_prepared(), "{r:?}");
    r.plan.unwrap()
}
fn rewrite(local: &Repository, remote: &Repository) -> Plan {
    seed(local, remote, "HEAD");
    commit(remote, "upstream");
    let p = prepared(local, FinishUpdate::Rebase);
    assert!(matches!(p.recovery, Recovery::Verified(_)));
    p
}
fn publish(local: &Repository) -> publication::Report {
    publication::publish(local.path(), &prepared(local, FinishUpdate::None))
}
fn hook(directory: &Path, name: &str, body: &str) {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join(name), format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(directory.join(name), fs::Permissions::from_mode(0o755)).unwrap();
}
fn local_hooks(local: &Repository) -> PathBuf {
    let hooks = local.path().join(".git/publication-hooks");
    fs::create_dir_all(&hooks).unwrap();
    local.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    hooks
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn complete(report: &publication::Report, states: &[DestinationState]) {
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(report.state, PublicationState::Complete);
    assert_eq!(
        report
            .destinations
            .iter()
            .map(|d| d.state)
            .collect::<Vec<_>>(),
        states
    );
}

#[test]
fn creates_advances_and_confirms_unchanged_without_touching_local_history() {
    isolated(
        "creates_advances_and_confirms_unchanged_without_touching_local_history",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let before = oid(&local, "HEAD");
            let index = fs::read(local.path().join(".git/index")).unwrap();
            let report = publish(&local);
            complete(&report, &[DestinationState::Created]);
            assert_eq!(
                report.tracking.writes,
                [TrackingWrite::Remote, TrackingWrite::MergeRef]
            );
            assert_eq!(oid(&remote, REF), before);
            assert_eq!(oid(&local, "HEAD"), before);
            assert_eq!(fs::read(local.path().join(".git/index")).unwrap(), index);
            let config = fs::read(local.path().join(".git/config")).unwrap();
            let report = publish(&local);
            complete(&report, &[DestinationState::Unchanged]);
            assert!(
                report.destinations[0]
                    .commands
                    .iter()
                    .all(|c| !c.arguments.contains(&"push".into()))
            );
            assert_eq!(fs::read(local.path().join(".git/config")).unwrap(), config);
            let next = commit(&local, "next");
            let report = publish(&local);
            complete(&report, &[DestinationState::Advanced]);
            assert_eq!(report.destinations[0].action, Some(Action::FastForward));
            assert_eq!(oid(&remote, REF), next);
        },
    );
}

#[test]
fn authorized_rebase_uses_exact_original_lease_and_retains_recovery() {
    isolated(
        "authorized_rebase_uses_exact_original_lease_and_retains_recovery",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let original = oid(&local, "HEAD");
            let plan = rewrite(&local, &remote);
            let record = local
                .path()
                .join(".git/oneflow-finish/branches/feature/exact/rewrite.json");
            let bytes = fs::read(&record).unwrap();
            let report = publication::publish(local.path(), &plan);
            complete(&report, &[DestinationState::Rewritten]);
            assert_eq!(oid(&remote, REF), plan.commit);
            let args = &report.destinations[0].commands.last().unwrap().arguments;
            assert!(
                args.contains(
                    &format!(
                        "--force-with-lease={REF}:{}",
                        String::from_utf8(original).unwrap()
                    )
                    .into()
                )
            );
            assert!(!args.contains(&"--force".into()));
            assert_eq!(fs::read(record).unwrap(), bytes);
            complete(&publish(&local), &[DestinationState::Unchanged]);
        },
    );
}

#[test]
fn each_push_destination_has_its_own_action_and_fetch_url_is_untouched() {
    isolated(
        "each_push_destination_has_its_own_action_and_fetch_url_is_untouched",
        true,
        |root| {
            let (local, fetch) = fixture(root);
            let absent = Repository::new(root);
            let behind = Repository::new(root);
            let rewritten = Repository::new(root);
            let equal = Repository::new(root);
            seed(&local, &behind, "origin/main");
            seed(&local, &rewritten, "HEAD");
            seed(&local, &equal, "HEAD");
            destinations(&local, &[&absent, &behind, &rewritten, &equal]);
            commit(&fetch, "upstream");
            let plan = prepared(&local, FinishUpdate::Rebase);
            // Simulate an earlier acknowledged or unacknowledged completed destination.
            equal.success(&[
                "fetch",
                local.path().to_str().unwrap(),
                &format!("+HEAD:{REF}"),
            ]);
            let report = publication::publish(local.path(), &plan);
            complete(
                &report,
                &[
                    DestinationState::Created,
                    DestinationState::Advanced,
                    DestinationState::Rewritten,
                    DestinationState::Unchanged,
                ],
            );
            assert!(oid_at(fetch.path(), REF).is_none());
            for remote in [&absent, &behind, &rewritten, &equal] {
                assert_eq!(oid(remote, REF), plan.commit);
            }
            assert_eq!(report.destinations[1].action, Some(Action::FastForward));
            assert_eq!(report.destinations[2].action, Some(Action::Rewrite));
        },
    );
}

#[test]
fn repeated_destinations_use_fresh_equality_after_first_update() {
    isolated(
        "repeated_destinations_use_fresh_equality_after_first_update",
        true,
        |root| {
            let (local, remote) = fixture(root);
            destinations(&local, &[&remote, &remote]);
            complete(
                &publish(&local),
                &[DestinationState::Created, DestinationState::Unchanged],
            );
        },
    );
}

#[test]
fn unresolved_denied_and_unprepared_plans_never_push() {
    isolated(
        "unresolved_denied_and_unprepared_plans_never_push",
        true,
        |root| {
            let (local, remote) = fixture(root);
            seed(&local, &remote, "HEAD");
            commit(&remote, "upstream");
            let p = fresh_at(
                local.path(),
                &req(FinishUpdate::Rebase, FinishRewrite::Allow),
            );
            let report = publication::publish(local.path(), &p);
            assert_eq!(report.state, PublicationState::NotAttempted);
            assert!(matches!(
                report.error,
                Some(PublicationError::UnpreparedPlan)
            ));
            let p = prepared(&local, FinishUpdate::Rebase);
            for rewrite in [FinishRewrite::Ask, FinishRewrite::Deny] {
                let p = finish::assess(
                    local.path(),
                    &req(FinishUpdate::None, rewrite),
                    p.evidence.as_ref(),
                )
                .unwrap();
                let report = publication::publish(local.path(), &p);
                assert_eq!(report.state, PublicationState::NotAttempted);
                assert!(report.destinations.iter().all(|d| d.commands.is_empty()));
            }
            assert_ne!(oid(&remote, REF), p.commit);
        },
    );
}

#[test]
fn changed_local_context_stops_before_publication() {
    isolated(
        "changed_local_context_stops_before_publication",
        true,
        |root| {
            for change in [
                "branch",
                "dirty",
                "settings",
                "upstream",
                "destinations",
                "config",
                "recovery",
            ] {
                let (local, remote) = fixture(root);
                let p = rewrite(&local, &remote);
                match change {
                    "branch" => {
                        local.success(&["switch", "-c", "other"]);
                    }
                    "dirty" => {
                        fs::write(local.path().join("untracked"), "dirty").unwrap();
                    }
                    "settings" => {
                        fs::write(
                            local.path().join(".git/git-oneflow.config.toml"),
                            "finish-rewrite = 'deny'\n",
                        )
                        .unwrap();
                    }
                    "upstream" => {
                        local.success(&["config", &format!("branch.{BRANCH}.remote"), "other"]);
                    }
                    "destinations" => {
                        destinations(&local, &[&Repository::new(root)]);
                    }
                    "config" => {
                        local.success(&["config", "push.followTags", "true"]);
                    }
                    "recovery" => {
                        fs::write(
                            local
                                .path()
                                .join(".git/oneflow-finish/branches/feature/exact/rewrite.json"),
                            "{}",
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                let r = publication::publish(local.path(), &p);
                assert_eq!(r.state, PublicationState::NotAttempted, "{change}: {r:?}");
                assert!(r.error.is_some(), "{change}");
                assert_ne!(oid(&remote, REF), p.commit);
            }
        },
    );
}

#[test]
fn expansive_defaults_cannot_publish_other_branches_tags_or_delete_refs() {
    isolated(
        "expansive_defaults_cannot_publish_other_branches_tags_or_delete_refs",
        true,
        |root| {
            let (local, remote) = fixture(root);
            local.success(&["branch", "unrelated"]);
            local.success(&["tag", "-a", "local-tag", "-m", "tag"]);
            remote.success(&["branch", "remote-only"]);
            let remote_only = oid(&remote, "remote-only");
            for (key, value) in [
                ("remote.origin.mirror", "true"),
                ("remote.origin.push", "+refs/*:refs/*"),
                ("push.default", "matching"),
                ("push.followTags", "true"),
                ("push.autoSetupRemote", "true"),
                ("push.recurseSubmodules", "on-demand"),
            ] {
                local.success(&["config", key, value]);
            }
            complete(&publish(&local), &[DestinationState::Created]);
            assert!(oid_at(remote.path(), "refs/heads/unrelated").is_none());
            assert!(oid_at(remote.path(), "refs/tags/local-tag").is_none());
            assert_eq!(oid(&remote, "remote-only"), remote_only);
            assert_eq!(oid(&local, "HEAD"), oid(&remote, REF));
        },
    );
}

#[test]
fn partial_rejection_retains_rewrite_proof_and_retry_skips_completed_destination() {
    isolated(
        "partial_rejection_retains_rewrite_proof_and_retry_skips_completed_destination",
        true,
        |root| {
            for fails_first in [false, true] {
                let (local, remote) = fixture(root);
                let second = Repository::new(root);
                seed(&local, &remote, "HEAD");
                seed(&local, &second, "HEAD");
                destinations(&local, &[&remote, &second]);
                commit(&remote, "upstream");
                let failed = if fails_first { &remote } else { &second };
                hook(&failed.path().join(".git/hooks"), "pre-receive", "exit 1");
                failed.success(&[
                    "config",
                    "core.hooksPath",
                    failed.path().join(".git/hooks").to_str().unwrap(),
                ]);
                let p = prepared(&local, FinishUpdate::Rebase);
                let r = publication::publish(local.path(), &p);
                assert_eq!(r.state, PublicationState::Partial, "{r:?}");
                assert_eq!(r.tracking.state, TrackingState::Absent);
                assert!(r.tracking.writes.is_empty());
                assert!(matches!(r.recovery, Recovery::Verified(_)));
                fs::remove_file(failed.path().join(".git/hooks/pre-receive")).unwrap();
                let r = if fails_first {
                    publish(&local)
                } else {
                    publication::publish(local.path(), &p)
                };
                complete(
                    &r,
                    if fails_first {
                        &[DestinationState::Rewritten, DestinationState::Unchanged]
                    } else {
                        &[DestinationState::Unchanged, DestinationState::Rewritten]
                    },
                );
            }
        },
    );
}

#[test]
fn remote_movement_before_publication_stops_only_that_destination() {
    isolated(
        "remote_movement_before_publication_stops_only_that_destination",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let second = Repository::new(root);
            seed(&local, &remote, "HEAD");
            seed(&local, &second, "HEAD");
            destinations(&local, &[&remote, &second]);
            commit(&remote, "upstream");
            let p = prepared(&local, FinishUpdate::Rebase);
            remote.success(&["update-ref", REF, "HEAD"]);
            let changed = oid(&remote, REF);
            let r = publication::publish(local.path(), &p);
            assert_eq!(r.state, PublicationState::Partial, "{r:?}");
            assert_eq!(r.destinations[0].state, DestinationState::Rejected);
            assert!(matches!(
                r.destinations[0].error,
                Some(PublicationError::RemoteChanged)
            ));
            assert!(
                r.destinations[0]
                    .commands
                    .iter()
                    .all(|c| !c.arguments.contains(&"push".into()))
            );
            assert_eq!(r.destinations[1].state, DestinationState::Rewritten);
            assert_eq!(oid(&remote, REF), changed);
        },
    );
}

#[test]
fn tracking_failure_is_separate_and_retry_only_establishes_tracking() {
    isolated(
        "tracking_failure_is_separate_and_retry_only_establishes_tracking",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let hooks = local_hooks(&local);
            hook(
                &hooks,
                "pre-push",
                "printf 'existing lock' > .git/config.lock",
            );
            let r = publish(&local);
            assert_eq!(r.state, PublicationState::Complete, "{r:?}");
            assert_eq!(r.tracking.state, TrackingState::Absent);
            assert!(matches!(r.error, Some(PublicationError::Tracking(_))));
            assert!(!r.is_complete());
            assert_eq!(oid(&remote, REF), oid(&local, "HEAD"));
            fs::remove_file(local.path().join(".git/config.lock")).unwrap();
            fs::remove_file(hooks.join("pre-push")).unwrap();
            complete(&publish(&local), &[DestinationState::Unchanged]);
        },
    );
}

#[test]
fn pinned_commit_and_local_hook_effects_are_reported_without_continuing() {
    isolated(
        "pinned_commit_and_local_hook_effects_are_reported_without_continuing",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let second = Repository::new(root);
            destinations(&local, &[&remote, &second]);
            let hooks = local_hooks(&local);
            hook(
                &hooks,
                "pre-push",
                "git commit --allow-empty -m hook-change",
            );
            let p = prepared(&local, FinishUpdate::None);
            let r = publication::publish(local.path(), &p);
            assert_eq!(r.state, PublicationState::Partial, "{r:?}");
            assert_eq!(r.destinations[0].state, DestinationState::Created);
            assert_eq!(r.destinations[1].state, DestinationState::NotAttempted);
            assert!(r.error.is_some());
            assert_eq!(oid(&remote, REF), p.commit);
            assert_ne!(oid(&local, "HEAD"), p.commit);
            assert!(oid_at(second.path(), REF).is_none());
            assert!(r.tracking.writes.is_empty());
        },
    );
}

// Faults wrap real Git in a child process: test runner environment is untouched.
fn publisher_child(local: &Repository, body: &str) -> std::process::Output {
    let wrapper = tempfile::tempdir().unwrap();
    hook(
        wrapper.path(),
        "git",
        &format!(
            "for arg do\n if [ \"$arg\" = push ]; then\n{body}\n fi\ndone\nexec \"$ONEFLOW_REAL_GIT\" \"$@\""
        ),
    );
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|p| p.join("git"))
        .find(|p| p.is_file())
        .unwrap();
    let paths = std::env::join_paths(
        std::iter::once(wrapper.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "publication_child", "--nocapture"])
        .env("ONEFLOW_PUBLICATION_DIR", local.path())
        .env("ONEFLOW_REAL_GIT", real)
        .env("PATH", paths)
        .output()
        .unwrap()
}
fn child_result(local: &Repository, body: &str) -> serde_json::Value {
    let output = publisher_child(local, body);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&fs::read(local.path().join(".git/finish-result.json")).unwrap())
        .unwrap()
}
#[test]
fn publication_child() {
    let Some(directory) = std::env::var_os("ONEFLOW_PUBLICATION_DIR") else {
        return;
    };
    let directory = Path::new(&directory);
    let p = fresh_at(directory, &req(FinishUpdate::None, FinishRewrite::Allow));
    assert!(p.is_ready(), "{p:?}");
    let r = publication::publish(directory, &p);
    let data = serde_json::json!({
        "state": format!("{:?}", r.state), "tracking": format!("{:?}", r.tracking.state),
        "complete": r.is_complete(), "error": r.error.map(|e| e.to_string()),
        "destinations": r.destinations.iter().map(|d| serde_json::json!({
            "state": format!("{:?}", d.state), "detail": String::from_utf8_lossy(&d.detail),
            "pushes": d.commands.iter().filter(|c| c.arguments.contains(&OsString::from("push"))).count(),
            "arguments": d.commands.last().map(|c| c.arguments.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>()),
            "error": d.error.as_ref().map(|e| e.to_string()),
        })).collect::<Vec<_>>(),
    });
    fs::write(
        directory.join(".git/finish-result.json"),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
}

#[test]
fn exact_leases_reject_racing_rewrites_fast_forwards_and_competing_creation_even_after_fetch() {
    isolated(
        "exact_leases_reject_racing_rewrites_fast_forwards_and_competing_creation_even_after_fetch",
        true,
        |root| {
            for action in ["create", "advance", "rewrite"] {
                let (local, remote) = fixture(root);
                let expected = match action {
                    "create" => String::new(),
                    "advance" => {
                        seed(&local, &remote, "origin/main");
                        String::from_utf8(oid(&remote, REF)).unwrap()
                    }
                    "rewrite" => {
                        rewrite(&local, &remote);
                        String::from_utf8(oid(&remote, REF)).unwrap()
                    }
                    _ => unreachable!(),
                };
                // Competing creation is even an ancestor of our prepared tip: the
                // absence lease must refuse overwriting it despite a possible FF.
                let moved = if action == "create" {
                    oid(&remote, "main")
                } else {
                    remote.success(&["switch", "-c", "concurrent", REF]);
                    let moved = commit(&remote, "concurrent");
                    remote.success(&["switch", "main"]);
                    moved
                };
                let moved = String::from_utf8(moved).unwrap();
                let body = format!(
                    "\"$ONEFLOW_REAL_GIT\" -C {} update-ref {} {} || exit 91\n\"$ONEFLOW_REAL_GIT\" fetch origin >&2 || exit 92\nexec \"$ONEFLOW_REAL_GIT\" \"$@\"",
                    quote(remote.path().to_str().unwrap()),
                    REF,
                    moved
                );
                let result = child_result(&local, &body);
                assert_eq!(result["state"], "Rejected", "{action}: {result}");
                assert_eq!(result["tracking"], "Absent");
                assert_eq!(result["destinations"][0]["pushes"], 1);
                assert!(
                    result["destinations"][0]["detail"]
                        .as_str()
                        .unwrap()
                        .contains("stale info"),
                    "{result}"
                );
                assert!(
                    result["destinations"][0]["arguments"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|a| a == &format!("--force-with-lease={REF}:{expected}"))
                );
                assert_eq!(String::from_utf8(oid(&remote, REF)).unwrap(), moved);
                assert_eq!(
                    String::from_utf8(oid(&local, "refs/remotes/origin/feature/exact")).unwrap(),
                    moved
                );
            }
        },
    );
}

#[test]
fn server_race_after_advertisement_cannot_replace_competing_creation() {
    isolated(
        "server_race_after_advertisement_cannot_replace_competing_creation",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let hooks = local_hooks(&local);
            hook(
                &hooks,
                "pre-push",
                &format!(
                    "git -C {} update-ref {} HEAD",
                    quote(remote.path().to_str().unwrap()),
                    REF
                ),
            );
            let result = publish(&local);
            assert_eq!(result.state, PublicationState::Rejected, "{result:?}");
            assert_eq!(oid(&remote, REF), oid(&remote, "main"));
            assert_ne!(oid(&remote, REF), oid(&local, "HEAD"));
        },
    );
}

#[test]
fn lost_acknowledgement_and_interrupted_transport_are_uncertain_and_retry_observes_remote() {
    isolated(
        "lost_acknowledgement_and_interrupted_transport_are_uncertain_and_retry_observes_remote",
        true,
        |root| {
            for body in [
                "\"$ONEFLOW_REAL_GIT\" \"$@\" > /dev/null\nexit 0",
                "\"$ONEFLOW_REAL_GIT\" \"$@\" > /dev/null\nexit 1",
                "\"$ONEFLOW_REAL_GIT\" \"$@\" > /dev/null\nkill -TERM \"$$\"",
                "exit 1",
            ] {
                let (local, remote) = fixture(root);
                let p = rewrite(&local, &remote);
                let result = child_result(&local, body);
                assert_eq!(result["state"], "Unconfirmed", "{result}");
                assert_eq!(result["tracking"], "Absent");
                let updated = body != "exit 1";
                assert_eq!(oid(&remote, REF) == p.commit, updated);
                let retry = publish(&local);
                complete(
                    &retry,
                    &[if updated {
                        DestinationState::Unchanged
                    } else {
                        DestinationState::Rewritten
                    }],
                );
            }
        },
    );
}

#[test]
fn killed_executor_after_first_destination_recovers_in_another_process() {
    isolated(
        "killed_executor_after_first_destination_recovers_in_another_process",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let second = Repository::new(root);
            seed(&local, &remote, "HEAD");
            seed(&local, &second, "HEAD");
            destinations(&local, &[&remote, &second]);
            commit(&remote, "upstream");
            let p = prepared(&local, FinishUpdate::Rebase);
            let out = publisher_child(
                &local,
                "\"$ONEFLOW_REAL_GIT\" \"$@\" > /dev/null\nkill -KILL \"$PPID\"\nexit 0",
            );
            assert!(!out.status.success());
            assert_eq!(oid(&remote, REF), p.commit);
            assert_ne!(oid(&second, REF), p.commit);
            assert!(
                !fs::read_to_string(local.path().join(".git/config"))
                    .unwrap()
                    .contains("[branch")
            );
            let retry = child_result(&local, "exec \"$ONEFLOW_REAL_GIT\" \"$@\"");
            assert_eq!(retry["state"], "Complete", "{retry}");
            assert_eq!(retry["destinations"][0]["state"], "Unchanged");
            assert_eq!(retry["destinations"][0]["pushes"], 0);
            assert_eq!(retry["destinations"][1]["state"], "Rewritten");
            assert_eq!(retry["complete"], true);
        },
    );
}

#[test]
fn partial_lost_acknowledgement_and_failed_process_with_acknowledgement_retain_effects() {
    isolated(
        "partial_lost_acknowledgement_and_failed_process_with_acknowledgement_retain_effects",
        true,
        |root| {
            for lose_ack in [true, false] {
                let (local, remote) = fixture(root);
                let second = Repository::new(root);
                destinations(&local, &[&remote, &second]);
                let body = format!(
                    "if [ ! -f .git/handled ]; then\n touch .git/handled\n \"$ONEFLOW_REAL_GIT\" \"$@\" {}\n exit 1\nfi\nexec \"$ONEFLOW_REAL_GIT\" \"$@\"",
                    if lose_ack { "> /dev/null" } else { "" }
                );
                let result = child_result(&local, &body);
                assert_eq!(
                    result["state"],
                    if lose_ack { "Partial" } else { "Complete" },
                    "{result}"
                );
                assert_eq!(
                    result["tracking"],
                    if lose_ack { "Absent" } else { "Matching" }
                );
                assert_eq!(result["complete"], false);
                assert_eq!(oid(&remote, REF), oid(&local, "HEAD"));
                assert_eq!(oid(&second, REF), oid(&local, "HEAD"));
                complete(
                    &publish(&local),
                    &[DestinationState::Unchanged, DestinationState::Unchanged],
                );
            }
        },
    );
}

#[test]
fn explicit_push_urls_do_not_get_rewritten_again_and_transport_options_are_preserved() {
    isolated(
        "explicit_push_urls_do_not_get_rewritten_again_and_transport_options_are_preserved",
        true,
        |root| {
            let (local, fetch) = fixture(root);
            let target = Repository::new(root);
            let wrong = Repository::new(root);
            destinations(&local, &[&target]);
            local.success(&[
                "config",
                &format!("url.{}.pushInsteadOf", wrong.path().display()),
                target.path().to_str().unwrap(),
            ]);
            let receive = local.path().join(".git/receive-helper");
            hook(
                &local.path().join(".git"),
                "receive-helper",
                &format!(
                    "touch {}\nexec git-receive-pack \"$@\"",
                    quote(local.path().join(".git/received").to_str().unwrap())
                ),
            );
            local.success(&[
                "config",
                "remote.origin.receivepack",
                &quote(receive.to_str().unwrap()),
            ]);
            // A preexisting remote with the internal preferred name is preserved.
            local.success(&[
                "remote",
                "add",
                "oneflow-finish-publication",
                wrong.path().to_str().unwrap(),
            ]);
            complete(&publish(&local), &[DestinationState::Created]);
            assert!(local.path().join(".git/received").exists());
            assert_eq!(oid(&target, REF), oid(&local, "HEAD"));
            assert!(oid_at(fetch.path(), REF).is_none());
            assert!(oid_at(wrong.path(), REF).is_none());
        },
    );
}

#[test]
fn unreachable_destination_is_not_absence_or_success_even_with_a_matching_cache() {
    isolated(
        "unreachable_destination_is_not_absence_or_success_even_with_a_matching_cache",
        true,
        |root| {
            for partial in [false, true] {
                let (local, remote) = fixture(root);
                let missing = Repository::new(root);
                if partial {
                    destinations(&local, &[&remote, &missing]);
                } else {
                    destinations(&local, &[&missing]);
                }
                let p = prepared(&local, FinishUpdate::None);
                // Tracking refs are only hints, including a misleading equality.
                local.success(&["update-ref", "refs/remotes/origin/feature/exact", "HEAD"]);
                fs::remove_dir_all(missing.path()).unwrap();
                let r = publication::publish(local.path(), &p);
                assert_eq!(
                    r.state,
                    if partial {
                        PublicationState::Partial
                    } else {
                        PublicationState::Unconfirmed
                    },
                    "{r:?}"
                );
                let failed = r.destinations.last().unwrap();
                assert_eq!(failed.state, DestinationState::Unconfirmed);
                assert!(failed.observed.is_none());
                assert!(
                    failed
                        .commands
                        .iter()
                        .all(|c| !c.arguments.contains(&"push".into()))
                );
                assert!(r.tracking.writes.is_empty());
                assert_eq!(r.tracking.state, TrackingState::Absent);
            }
        },
    );
}

#[test]
fn multiple_remote_urls_and_push_instead_of_select_actual_destinations() {
    isolated(
        "multiple_remote_urls_and_push_instead_of_select_actual_destinations",
        true,
        |root| {
            for rewritten in [false, true] {
                let (local, fetch) = fixture(root);
                let second = Repository::new(root);
                if rewritten {
                    local.success(&[
                        "config",
                        &format!("url.{}.pushInsteadOf", second.path().display()),
                        fetch.path().to_str().unwrap(),
                    ]);
                } else {
                    local.success(&[
                        "config",
                        "--add",
                        "remote.origin.url",
                        second.path().to_str().unwrap(),
                    ]);
                }
                let r = publish(&local);
                complete(
                    &r,
                    if rewritten {
                        &[DestinationState::Created]
                    } else {
                        &[DestinationState::Created, DestinationState::Created]
                    },
                );
                assert_eq!(oid_at(fetch.path(), REF).is_some(), !rewritten);
                assert_eq!(oid(&second, REF), oid(&local, "HEAD"));
            }
        },
    );
}

#[test]
fn unexpected_porcelain_is_unconfirmed_despite_a_successful_process() {
    isolated(
        "unexpected_porcelain_is_unconfirmed_despite_a_successful_process",
        true,
        |root| {
            for response in ["wrong-ref", "duplicate", "forced", "deleted"] {
                let (local, remote) = fixture(root);
                let flag = match response {
                    "forced" => "+",
                    "deleted" => "-",
                    _ => "*",
                };
                let reference = if response == "wrong-ref" {
                    "refs/heads/other"
                } else {
                    REF
                };
                let line = format!(
                    "{flag}\t{}:{reference}\t[status]\n",
                    String::from_utf8(oid(&local, "HEAD")).unwrap()
                );
                let output = format!(
                    "To fixture\n{}{}Done\n",
                    line,
                    if response == "duplicate" { &line } else { "" }
                );
                let body = format!(
                    "\"$ONEFLOW_REAL_GIT\" \"$@\" > /dev/null || exit 1\nprintf '%s' {}\nexit 0",
                    quote(&output)
                );
                let r = child_result(&local, &body);
                assert_eq!(r["state"], "Unconfirmed", "{response}: {r}");
                assert_eq!(r["tracking"], "Absent");
                assert_eq!(oid(&remote, REF), oid(&local, "HEAD"));
                complete(&publish(&local), &[DestinationState::Unchanged]);
            }
        },
    );
}

#[test]
fn newly_observed_remote_work_cannot_borrow_old_rewrite_proof_on_retry() {
    isolated(
        "newly_observed_remote_work_cannot_borrow_old_rewrite_proof_on_retry",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let p = rewrite(&local, &remote);
            remote.success(&["switch", "-c", "new-work", REF]);
            let moved = commit(&remote, "unincorporated");
            remote.success(&["update-ref", REF, "HEAD"]);
            remote.success(&["switch", "main"]);
            let r = publication::publish(local.path(), &p);
            assert_eq!(r.state, PublicationState::Rejected, "{r:?}");
            let retry = fresh_at(local.path(), &req(FinishUpdate::None, FinishRewrite::Allow));
            assert!(matches!(retry.recovery, Recovery::Verified(_)));
            assert!(!retry.is_ready());
            let r = publication::publish(local.path(), &retry);
            assert_eq!(r.state, PublicationState::NotAttempted);
            assert_eq!(oid(&remote, REF), moved);
        },
    );
}
