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
    save::{
        self, Plan, Request, SaveError,
        publication::{
            self, DestinationState, PublicationState, PublishError, Report, TrackingState,
            TrackingWrite,
        },
    },
    settings::{SaveKind, SaveOperation, SavePlan, SaveScope},
};

const BRANCH: &str = "Feature/Exact_Name";
const REF: &str = "refs/heads/Feature/Exact_Name";

fn commit(repo: &Repository, contents: &str) -> String {
    fs::write(repo.path().join("tracked"), contents).unwrap();
    repo.success(&["add", "tracked"]);
    repo.success(&["commit", "-m", contents]);
    oid(repo.path(), "HEAD").unwrap()
}
fn oid(directory: &Path, reference: &str) -> Option<String> {
    let output = git::run(directory, ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}
fn bare(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    let output = git::run(
        root,
        [
            "init".as_ref(),
            "--bare".as_ref(),
            "--initial-branch=main".as_ref(),
            path.as_os_str(),
        ],
    )
    .unwrap();
    assert!(output.status.success());
    path
}
fn fixture(root: &Path) -> (Repository, PathBuf) {
    let local = Repository::new(root);
    commit(&local, "base\n");
    local.success(&["switch", "-c", BRANCH]);
    let remote = bare(
        root,
        &format!(
            "remote-{}.git",
            local.path().file_name().unwrap().to_string_lossy()
        ),
    );
    local.success(&[
        "remote".as_ref(),
        "add".as_ref(),
        "origin".as_ref(),
        remote.as_os_str(),
    ]);
    (local, remote)
}
fn plan(repo: &Repository) -> Plan {
    save::assess(
        repo.path(),
        &Request {
            plan: Some(SavePlan::PublishOnly),
            ..Request::default()
        },
    )
    .unwrap()
}
fn publish(repo: &Repository) -> Report {
    publication::publish(repo.path(), &plan(repo))
}
fn matching(repo: &Repository) {
    repo.success(&["config", &format!("branch.{BRANCH}.remote"), "origin"]);
    repo.success(&["config", &format!("branch.{BRANCH}.merge"), REF]);
}
fn has_config(repo: &Repository, suffix: &str) -> Option<Vec<u8>> {
    let output = git::run(
        repo.path(),
        ["config", "--get", &format!("branch.{BRANCH}.{suffix}")],
    )
    .unwrap();
    output.status.success().then_some(output.stdout)
}
fn assert_success(report: &Report, state: DestinationState) {
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(report.state, PublicationState::Complete);
    assert_eq!(report.destinations.len(), 1);
    assert_eq!(report.destinations[0].state, state);
    assert!(
        report
            .push
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .status
            .success()
    );
}
// Capture the worktree plus all local history, reflogs and raw index. Normal
// push tracking caches/config are allowed to change and reported separately.
fn preserved_local(repo: &Repository) -> impl FnOnce() + '_ {
    let mut before = snapshot(repo.path());
    before.retain(|path, _| !path.starts_with(".git") && !path.as_os_str().is_empty());
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    let head = fs::read(repo.path().join(".git/HEAD")).unwrap();
    let refs = snapshot(&repo.path().join(".git/refs/heads"));
    let logs = snapshot(&repo.path().join(".git/logs"));
    move || {
        let mut after = snapshot(repo.path());
        after.retain(|path, _| !path.starts_with(".git") && !path.as_os_str().is_empty());
        assert_eq!(after, before, "working tree changed");
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(repo.path().join(".git/HEAD")).unwrap(), head);
        assert_eq!(snapshot(&repo.path().join(".git/refs/heads")), refs);
        // Push can add logs/refs/remotes. Preserve the previously existing logs.
        let after = snapshot(&repo.path().join(".git/logs"));
        for (path, state) in logs {
            if path.starts_with("refs/remotes")
                || path == Path::new("refs")
                || path.as_os_str().is_empty()
            {
                continue;
            }
            assert_eq!(
                after.get(&path),
                Some(&state),
                "local history log changed: {path:?}"
            );
        }
    }
}

#[test]
fn creation_fast_forward_and_unchanged_keep_dirty_work_and_history() {
    isolated(
        "creation_fast_forward_and_unchanged_keep_dirty_work_and_history",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            // Partially staged work and a nonignored untracked file stay local.
            fs::write(repo.path().join("tracked"), "staged\n").unwrap();
            repo.success(&["add", "tracked"]);
            fs::write(repo.path().join("tracked"), "unstaged\n").unwrap();
            fs::write(repo.path().join("untracked"), "local\n").unwrap();
            fs::write(repo.path().join(".git/index.lock"), "existing lock").unwrap();
            let check = preserved_local(&repo);
            let report = publish(&repo);
            assert_success(&report, DestinationState::Created);
            assert_eq!(
                report
                    .tracking
                    .before
                    .as_ref()
                    .unwrap()
                    .upstream
                    .remotes
                    .len(),
                0
            );
            assert_eq!(
                report.tracking.writes,
                [TrackingWrite::Remote, TrackingWrite::MergeRef]
            );
            assert_eq!(report.tracking.state, TrackingState::Matching);
            assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
            check();
            let config = fs::read(repo.path().join(".git/config")).unwrap();
            let check = preserved_local(&repo);
            let report = publish(&repo);
            assert_success(&report, DestinationState::Unchanged);
            assert!(report.tracking.writes.is_empty());
            assert_eq!(fs::read(repo.path().join(".git/config")).unwrap(), config);
            check();
            let first = oid(&remote, REF).unwrap();
            assert_eq!(
                fs::read(repo.path().join(".git/index.lock")).unwrap(),
                b"existing lock"
            );
            fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
            let next = commit(&repo, "next\n");
            let check = preserved_local(&repo);
            assert_success(&publish(&repo), DestinationState::Advanced);
            assert_eq!(oid(&remote, REF).unwrap(), next);
            assert!(
                git::run(&remote, ["merge-base", "--is-ancestor", &first, &next])
                    .unwrap()
                    .status
                    .success()
            );
            check();
        },
    );
}

#[test]
fn remote_ahead_and_divergence_are_confirmed_rejections_without_repair() {
    isolated(
        "remote_ahead_and_divergence_are_confirmed_rejections_without_repair",
        true,
        |root| {
            for diverged in [false, true] {
                let (repo, remote) = fixture(root);
                // Publish base using ordinary fixture Git, leaving upstream absent.
                repo.success(&["push", "origin", &format!("HEAD:{REF}")]);
                let other = Repository::new(root);
                other.success(&[
                    "remote".as_ref(),
                    "add".as_ref(),
                    "origin".as_ref(),
                    remote.as_os_str(),
                ]);
                other.success(&["fetch", "origin"]);
                other.success(&[
                    "switch",
                    "--no-track",
                    "-c",
                    BRANCH,
                    &format!("refs/remotes/origin/{BRANCH}"),
                ]);
                let advanced = commit(&other, "remote next\n");
                other.success(&["push", "origin", &format!("HEAD:{REF}")]);
                if diverged {
                    commit(&repo, "local divergence\n");
                }
                let check = preserved_local(&repo);
                let report = publish(&repo);
                assert_eq!(report.state, PublicationState::Rejected, "{report:?}");
                assert_eq!(report.destinations[0].state, DestinationState::Rejected);
                assert_eq!(report.tracking.state, TrackingState::Absent);
                assert!(report.tracking.writes.is_empty());
                assert_eq!(oid(&remote, REF).unwrap(), advanced);
                assert!(!report.is_complete());
                check();
            }
        },
    );
}

#[test]
fn upstream_revalidation_refuses_incomplete_conflicting_and_duplicate_values() {
    isolated(
        "upstream_revalidation_refuses_incomplete_conflicting_and_duplicate_values",
        true,
        |root| {
            for (remotes, refs) in [
                (vec!["origin"], vec![]),
                (vec![], vec![REF]),
                (vec!["other"], vec![REF]),
                (vec!["origin"], vec!["refs/heads/other"]),
                (vec!["origin", "origin"], vec![REF]),
                (vec!["origin"], vec![REF, REF]),
                (vec!["."], vec![REF]),
                (vec![""], vec![REF]),
            ] {
                let (repo, _) = fixture(root);
                let approved = plan(&repo);
                for value in remotes {
                    repo.success(&["config", "--add", &format!("branch.{BRANCH}.remote"), value]);
                }
                for value in refs {
                    repo.success(&["config", "--add", &format!("branch.{BRANCH}.merge"), value]);
                }
                let before = snapshot(root);
                let report = publication::publish(repo.path(), &approved);
                assert_eq!(report.state, PublicationState::NotAttempted);
                assert!(report.push.is_none());
                assert!(matches!(
                    report.error,
                    Some(PublishError::Save(SaveError::InvalidUpstream))
                ));
                assert_eq!(snapshot(root), before);
            }
            let (repo, _) = fixture(root);
            matching(&repo);
            // A restrictive fetch refmap is compatible with exact upstream config.
            repo.success(&[
                "config",
                "remote.origin.fetch",
                "+refs/heads/main:refs/remotes/origin/main",
            ]);
            let before = fs::read(repo.path().join(".git/config")).unwrap();
            let report = publish(&repo);
            assert_success(&report, DestinationState::Created);
            assert_eq!(fs::read(repo.path().join(".git/config")).unwrap(), before);
            assert!(report.tracking.writes.is_empty());
        },
    );
}

#[test]
fn intended_head_settings_upstream_and_operations_are_revalidated() {
    isolated(
        "intended_head_settings_upstream_and_operations_are_revalidated",
        true,
        |root| {
            for change in [
                "commit",
                "branch",
                "settings",
                "pushurl",
                "upstream",
                "operation",
            ] {
                let (repo, remote) = fixture(root);
                fs::write(repo.path().join("untracked"), "before").unwrap();
                let approved = plan(&repo);
                match change {
                    "commit" => {
                        commit(&repo, "new commit\n");
                    }
                    "branch" => {
                        repo.success(&["switch", "-c", "another"]);
                    }
                    "settings" => fs::write(
                        repo.path().join(".git/twigsmith.config.toml"),
                        "base='trunk'",
                    )
                    .unwrap(),
                    "pushurl" => {
                        repo.success(&["config", "remote.origin.pushurl", "/missing/other"]);
                    }
                    "upstream" => matching(&repo),
                    "operation" => {
                        fs::write(repo.path().join(".git/CHERRY_PICK_HEAD"), "active\n").unwrap()
                    }
                    _ => unreachable!(),
                }
                let before = snapshot(root);
                let report = publication::publish(repo.path(), &approved);
                assert_eq!(
                    report.state,
                    PublicationState::NotAttempted,
                    "{change}: {report:?}"
                );
                assert!(report.push.is_none());
                assert_eq!(oid(&remote, REF), None);
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn publish_only_ignores_dirty_work_and_unrelated_setting_changes() {
    isolated(
        "publish_only_ignores_dirty_work_and_unrelated_setting_changes",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            fs::write(repo.path().join("untracked"), "before").unwrap();
            let approved = plan(&repo);
            fs::write(repo.path().join("untracked"), "after").unwrap();
            fs::write(repo.path().join("tracked"), "uncommitted").unwrap();
            repo.success(&["add", "tracked"]);
            fs::write(repo.path().join("tracked"), "remaining").unwrap();
            fs::write(
                repo.path().join(".git/twigsmith.config.toml"),
                "save-message='Changed message'\nfinish-cleanup='keep'\n",
            )
            .unwrap();
            let check = preserved_local(&repo);
            let report = publication::publish(repo.path(), &approved);
            assert_success(&report, DestinationState::Created);
            assert_eq!(oid(&remote, REF).unwrap().as_bytes(), approved.commit);
            check();
        },
    );
}

#[test]
fn unresolved_input_and_pending_checkpoints_cannot_be_published() {
    isolated(
        "unresolved_input_and_pending_checkpoints_cannot_be_published",
        true,
        |root| {
            for action in [
                SavePlan::Ask,
                SavePlan::Operations(vec![SaveOperation {
                    action: SaveKind::Commit,
                    scope: SaveScope::Staged,
                }]),
                SavePlan::Operations(vec![SaveOperation {
                    action: SaveKind::Commit,
                    scope: SaveScope::All,
                }]),
                SavePlan::Operations(vec![SaveOperation {
                    action: SaveKind::Stash,
                    scope: SaveScope::All,
                }]),
            ] {
                let (repo, _) = fixture(root);
                fs::write(repo.path().join("tracked"), "staged").unwrap();
                repo.success(&["add", "tracked"]);
                let approved = save::assess(
                    repo.path(),
                    &Request {
                        plan: Some(action),
                        ..Request::default()
                    },
                )
                .unwrap();
                let before = snapshot(root);
                let report = publication::publish(repo.path(), &approved);
                assert_eq!(report.state, PublicationState::NotAttempted);
                assert!(matches!(
                    report.error,
                    Some(PublishError::PendingLocalSteps)
                ));
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn separate_push_url_is_the_only_publication_evidence() {
    isolated(
        "separate_push_url_is_the_only_publication_evidence",
        true,
        |root| {
            let (repo, fetch) = fixture(root);
            let push = bare(root, "push-only.git");
            repo.success(&[
                "config".as_ref(),
                "remote.origin.pushurl".as_ref(),
                push.as_os_str(),
            ]);
            // A cached equality against the fetch remote must not imply publication.
            repo.success(&["push", "origin", &format!("HEAD:{REF}")]);
            let ahead = commit(&repo, "unpublished\n");
            repo.success(&[
                "update-ref",
                &format!("refs/remotes/origin/{BRANCH}"),
                &ahead,
            ]);
            let report = publish(&repo);
            assert_success(&report, DestinationState::Advanced);
            assert_eq!(oid(&push, REF).unwrap(), ahead);
            assert_eq!(oid(&fetch, REF), None);
            assert_eq!(
                report.configured_destinations,
                [push.as_os_str().as_encoded_bytes()]
            );
            assert_eq!(
                report.destinations[0].destination,
                push.as_os_str().as_encoded_bytes()
            );
        },
    );
}

#[test]
fn multiple_push_urls_confirm_all_and_preserve_duplicate_destinations() {
    isolated(
        "multiple_push_urls_confirm_all_and_preserve_duplicate_destinations",
        true,
        |root| {
            for duplicate in [false, true] {
                let (repo, first) = fixture(root);
                let other = bare(root, &format!("second-{duplicate}.git"));
                let second = if duplicate { &first } else { &other };
                repo.success(&[
                    "config".as_ref(),
                    "--add".as_ref(),
                    "remote.origin.pushurl".as_ref(),
                    first.as_os_str(),
                ]);
                repo.success(&[
                    "config".as_ref(),
                    "--add".as_ref(),
                    "remote.origin.pushurl".as_ref(),
                    second.as_os_str(),
                ]);
                let report = publish(&repo);
                assert!(report.is_complete(), "{report:?}");
                assert_eq!(report.destinations.len(), 2);
                assert_eq!(report.configured_destinations.len(), 2);
                assert_eq!(report.destinations[0].state, DestinationState::Created);
                assert_eq!(
                    report.destinations[1].state,
                    if duplicate {
                        DestinationState::Unchanged
                    } else {
                        DestinationState::Created
                    }
                );
                assert_eq!(oid(&first, REF), oid(repo.path(), "HEAD"));
                assert_eq!(oid(second, REF), oid(repo.path(), "HEAD"));
            }
        },
    );
}

#[test]
fn multiple_fetch_urls_are_all_push_destinations_when_pushurls_are_absent() {
    isolated(
        "multiple_fetch_urls_are_all_push_destinations_when_pushurls_are_absent",
        true,
        |root| {
            let (repo, first) = fixture(root);
            let second = bare(root, "second-fallback.git");
            repo.success(&[
                "config".as_ref(),
                "--add".as_ref(),
                "remote.origin.url".as_ref(),
                second.as_os_str(),
            ]);
            let report = publish(&repo);
            assert!(report.is_complete(), "{report:?}");
            assert_eq!(report.destinations.len(), 2);
            assert_eq!(oid(&first, REF), oid(repo.path(), "HEAD"));
            assert_eq!(oid(&second, REF), oid(repo.path(), "HEAD"));
        },
    );
}

#[test]
fn expansive_push_settings_do_not_publish_unrelated_branches_or_tags() {
    isolated(
        "expansive_push_settings_do_not_publish_unrelated_branches_or_tags",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            repo.success(&["branch", "unrelated"]);
            repo.success(&["tag", "lightweight"]);
            repo.success(&["tag", "-a", "annotated", "-m", "annotated"]);
            for (key, value) in [
                ("push.default", "matching"),
                ("remote.origin.mirror", "true"),
                ("remote.origin.push", "+refs/heads/*:refs/heads/*"),
                ("push.followTags", "true"),
                ("push.autoSetupRemote", "true"),
                ("push.recurseSubmodules", "on-demand"),
                ("branch.Feature/Exact_Name.pushRemote", "origin"),
                ("remote.pushDefault", "another"),
            ] {
                repo.success(&["config", key, value]);
            }
            // Existing unrelated remote history and tag must survive mirror settings.
            repo.success(&[
                "-c",
                "remote.origin.mirror=false",
                "push",
                "--no-follow-tags",
                "origin",
                "HEAD:refs/heads/remote-only",
                "refs/tags/lightweight:refs/tags/remote-tag",
            ]);
            let check = preserved_local(&repo);
            let report = publish(&repo);
            assert_success(&report, DestinationState::Created);
            assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
            for reference in [
                "refs/heads/main",
                "refs/heads/unrelated",
                "refs/tags/annotated",
                "refs/tags/lightweight",
            ] {
                assert_eq!(oid(&remote, reference), None, "published {reference}");
            }
            assert!(oid(&remote, "refs/heads/remote-only").is_some());
            assert!(oid(&remote, "refs/tags/remote-tag").is_some());
            check();
            // An implicit force in remote.push/mirror must never repair divergence.
            repo.success(&["reset", "--hard", "HEAD"]);
            let old = oid(repo.path(), "HEAD").unwrap();
            let newer = commit(&repo, "published next\n");
            assert_success(&publish(&repo), DestinationState::Advanced);
            repo.success(&["reset", "--hard", &old]);
            let report = publish(&repo);
            assert_eq!(report.state, PublicationState::Rejected, "{report:?}");
            assert_eq!(oid(&remote, REF).unwrap(), newer);
        },
    );
}

#[test]
fn exact_transport_ignores_unsafe_fetch_mappings_and_preserves_symbolic_cache_targets() {
    isolated(
        "exact_transport_ignores_unsafe_fetch_mappings_and_preserves_symbolic_cache_targets",
        true,
        |root| {
            for mapping in [
                "+refs/heads/*:refs/heads/*",
                "+refs/heads/*:refs/tags/*",
                "+refs/heads/*:refs/custom/*",
            ] {
                let (repo, remote) = fixture(root);
                repo.success(&["config", "remote.origin.fetch", mapping]);
                let check = preserved_local(&repo);
                let report = publish(&repo);
                assert_success(&report, DestinationState::Created);
                assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
                assert!(report.caches[0].has_issue(), "{report:#?}");
                check();
            }
            let (repo, remote) = fixture(root);
            let cache = format!("refs/remotes/origin/{BRANCH}");
            repo.success(&["symbolic-ref", &cache, "refs/heads/main"]);
            let check = preserved_local(&repo);
            let report = publish(&repo);
            assert_success(&report, DestinationState::Created);
            assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
            assert_eq!(
                repo.success(&["symbolic-ref", &cache]).stdout,
                b"refs/heads/main\n"
            );
            assert!(report.caches[0].has_issue(), "{report:#?}");
            check();
        },
    );
}

#[test]
fn literal_at_unicode_and_nested_callers_publish_exact_branch_names() {
    isolated(
        "literal_at_unicode_and_nested_callers_publish_exact_branch_names",
        true,
        |root| {
            for branch in ["@", "Feature/Cafe\u{301}", "Feature/Café", "Topic/HEADish"] {
                let (repo, remote) = fixture(root);
                repo.success(&["branch", "-m", branch]);
                let nested = repo.path().join("nested");
                fs::create_dir(&nested).unwrap();
                let approved = save::assess(&nested, &Request::default()).unwrap();
                let report = publication::publish(&nested, &approved);
                assert_success(&report, DestinationState::Created);
                assert_eq!(
                    oid(&remote, &format!("refs/heads/{branch}")),
                    oid(repo.path(), "HEAD")
                );
                assert_eq!(oid(&remote, REF), None);
                let upstream = git::run(
                    repo.path(),
                    ["config", "--get", &format!("branch.{branch}.merge")],
                )
                .unwrap();
                assert_eq!(upstream.stdout, format!("refs/heads/{branch}\n").as_bytes());
            }
        },
    );
}

#[cfg(unix)]
fn hook(directory: &Path, name: &str, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(directory).unwrap();
    let path = directory.join(name);
    fs::write(&path, contents).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(unix)]
fn local_hooks(repo: &Repository) -> PathBuf {
    let path = repo.path().join(".git/task-hooks");
    repo.success(&[
        "config".as_ref(),
        "core.hooksPath".as_ref(),
        path.as_os_str(),
    ]);
    path
}

#[cfg(unix)]
#[test]
fn server_rejection_and_multiple_destination_partial_results_retain_effects() {
    isolated(
        "server_rejection_and_multiple_destination_partial_results_retain_effects",
        true,
        |root| {
            for reject_first in [false, true] {
                let (repo, first) = fixture(root);
                let second = bare(root, &format!("rejected-second-{reject_first}.git"));
                let rejected = if reject_first { &first } else { &second };
                let accepted = if reject_first { &second } else { &first };
                hook(
                    &rejected.join("hooks"),
                    "pre-receive",
                    "#!/bin/sh\necho deliberate rejection >&2\nexit 1\n",
                );
                repo.success(&[
                    "config".as_ref(),
                    "--add".as_ref(),
                    "remote.origin.pushurl".as_ref(),
                    first.as_os_str(),
                ]);
                repo.success(&[
                    "config".as_ref(),
                    "--add".as_ref(),
                    "remote.origin.pushurl".as_ref(),
                    second.as_os_str(),
                ]);
                let check = preserved_local(&repo);
                let report = publish(&repo);
                assert_eq!(report.state, PublicationState::Partial, "{report:?}");
                assert_eq!(report.destinations.len(), 2);
                assert_eq!(
                    report.destinations[usize::from(!reject_first)].state,
                    DestinationState::Rejected
                );
                assert_eq!(report.missing_responses, 0);
                assert_eq!(report.tracking.state, TrackingState::Absent);
                assert!(report.tracking.writes.is_empty());
                assert_eq!(oid(rejected, REF), None);
                assert_eq!(oid(accepted, REF), oid(repo.path(), "HEAD"));
                assert!(has_config(&repo, "remote").is_none());
                // Only the endpoint used for fetching can populate its cache.
                assert_eq!(
                    oid(repo.path(), &format!("refs/remotes/origin/{BRANCH}")),
                    (!reject_first).then(|| oid(repo.path(), "HEAD").unwrap())
                );
                check();
            }
            let (repo, remote) = fixture(root);
            hook(&remote.join("hooks"), "pre-receive", "#!/bin/sh\nexit 1\n");
            let report = publish(&repo);
            assert_eq!(report.state, PublicationState::Rejected, "{report:?}");
            assert_eq!(report.destinations[0].state, DestinationState::Rejected);
            assert_eq!(report.tracking.state, TrackingState::Absent);
        },
    );
}

#[test]
fn transport_failure_is_unconfirmed_and_can_follow_confirmed_publication() {
    isolated(
        "transport_failure_is_unconfirmed_and_can_follow_confirmed_publication",
        true,
        |root| {
            for succeeds_first in [false, true] {
                let (repo, remote) = fixture(root);
                repo.success(&[
                    "config",
                    "remote.origin.pushurl",
                    "/missing/oneflow-task12-transport",
                ]);
                if succeeds_first {
                    repo.success(&["config", "--unset-all", "remote.origin.pushurl"]);
                    repo.success(&[
                        "config".as_ref(),
                        "--add".as_ref(),
                        "remote.origin.pushurl".as_ref(),
                        remote.as_os_str(),
                    ]);
                    repo.success(&[
                        "config",
                        "--add",
                        "remote.origin.pushurl",
                        "/missing/oneflow-task12-transport",
                    ]);
                }
                let check = preserved_local(&repo);
                let report = publish(&repo);
                assert_eq!(
                    report.state,
                    if succeeds_first {
                        PublicationState::Partial
                    } else {
                        PublicationState::Unconfirmed
                    },
                    "{report:?}"
                );
                assert_eq!(report.missing_responses, 1);
                assert_eq!(report.destinations.len(), usize::from(succeeds_first));
                assert!(!report.is_complete());
                assert_eq!(report.tracking.state, TrackingState::Absent);
                assert!(report.tracking.writes.is_empty());
                assert_eq!(oid(&remote, REF).is_some(), succeeds_first);
                check();
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn normal_pre_push_hooks_run_and_failure_does_not_prove_rejection() {
    isolated(
        "normal_pre_push_hooks_run_and_failure_does_not_prove_rejection",
        true,
        |root| {
            for fails in [false, true] {
                let (repo, remote) = fixture(root);
                let hooks = local_hooks(&repo);
                hook(
                    &hooks,
                    "pre-push",
                    &format!(
                        "#!/bin/sh\nprintf '%s\\n' \"$1\" \"$2\" > .git/hook-args\ncat > .git/hook-input\nprintf 'checking fixture\\nDone\\n'\nexit {}\n",
                        u8::from(fails)
                    ),
                );
                let report = publish(&repo);
                let args = fs::read(repo.path().join(".git/hook-args")).unwrap();
                assert_eq!(
                    args,
                    format!("twigsmith-finish-publication\n{}\n", remote.display()).as_bytes()
                );
                let input = fs::read(repo.path().join(".git/hook-input")).unwrap();
                assert!(input.windows(REF.len()).any(|w| w == REF.as_bytes()));
                if fails {
                    assert_eq!(report.state, PublicationState::Unconfirmed, "{report:?}");
                    assert_eq!(report.tracking.state, TrackingState::Absent);
                    assert_eq!(oid(&remote, REF), None);
                } else {
                    assert_success(&report, DestinationState::Created);
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn tracking_failure_after_publication_reports_published_and_absent_tracking() {
    isolated(
        "tracking_failure_after_publication_reports_published_and_absent_tracking",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            let hooks = local_hooks(&repo);
            hook(
                &hooks,
                "pre-push",
                "#!/bin/sh\nprintf 'existing lock' > .git/config.lock\nexit 0\n",
            );
            let report = publish(&repo);
            assert_eq!(report.state, PublicationState::Complete, "{report:?}");
            assert_eq!(report.destinations[0].state, DestinationState::Created);
            assert_eq!(report.tracking.state, TrackingState::Absent);
            assert!(matches!(
                report.error,
                Some(PublishError::TrackingGit { .. })
            ));
            assert!(report.tracking.writes.is_empty());
            assert!(!report.is_complete());
            assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
            assert_eq!(
                fs::read(repo.path().join(".git/config.lock")).unwrap(),
                b"existing lock"
            );
            // Recovery retains publication and retries tracking without another commit.
            fs::remove_file(repo.path().join(".git/config.lock")).unwrap();
            fs::remove_file(hooks.join("pre-push")).unwrap();
            assert_success(&publish(&repo), DestinationState::Unchanged);
        },
    );
}

#[cfg(unix)]
#[test]
fn post_push_context_changes_stop_tracking_without_rolling_back_publication() {
    isolated(
        "post_push_context_changes_stop_tracking_without_rolling_back_publication",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            let intended = oid(repo.path(), "HEAD").unwrap();
            let next = commit(&repo, "next\n");
            repo.success(&["reset", "--hard", &intended]);
            let hooks = local_hooks(&repo);
            hook(
                &hooks,
                "pre-push",
                &format!("#!/bin/sh\ngit update-ref {REF} {next}\nexit 0\n"),
            );
            let report = publish(&repo);
            assert_eq!(report.state, PublicationState::Complete, "{report:?}");
            assert_eq!(
                oid(&remote, REF).unwrap(),
                intended,
                "push must pin the intended commit even if a hook moves the branch"
            );
            assert_eq!(
                oid(repo.path(), "HEAD").unwrap(),
                next,
                "hook effects must remain"
            );
            assert_eq!(report.tracking.state, TrackingState::Absent);
            assert!(matches!(
                report.error,
                Some(PublishError::Save(SaveError::ContextChanged))
            ));
        },
    );
}

#[cfg(unix)]
#[test]
fn dropped_transport_after_remote_update_is_unconfirmed_and_retains_the_branch() {
    isolated(
        "dropped_transport_after_remote_update_is_unconfirmed_and_retains_the_branch",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            // Git commits the ref transaction before sending the push status. Kill
            // only this fixture's receive-pack after commit to simulate a lost reply.
            hook(
                &remote.join("hooks"),
                "reference-transaction",
                "#!/bin/sh\nif test \"$1\" = committed; then kill -KILL \"$PPID\"; fi\nexit 0\n",
            );
            let check = preserved_local(&repo);
            let report = publish(&repo);
            assert_eq!(report.state, PublicationState::Unconfirmed, "{report:?}");
            assert_eq!(report.tracking.state, TrackingState::Absent);
            assert!(report.tracking.writes.is_empty());
            assert!(
                report
                    .destinations
                    .iter()
                    .all(|d| d.state == DestinationState::Unconfirmed)
            );
            assert!(
                !report
                    .push
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .status
                    .success()
            );
            // This independent test observation proves why a failed transport is
            // insufficient evidence for reporting that nothing was published.
            assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
            check();
            fs::remove_file(remote.join("hooks/reference-transaction")).unwrap();
            assert_success(&publish(&repo), DestinationState::Unchanged);
        },
    );
}

#[cfg(unix)]
#[test]
fn second_tracking_write_failure_preserves_and_reports_actual_config_effects() {
    use std::{env, process::Command};
    isolated(
        "second_tracking_write_failure_preserves_and_reports_actual_config_effects",
        true,
        |root| {
            if env::var_os("TWIGSMITH_PUBLICATION_WRAPPER").is_none() {
                let wrapper = root.join("git-wrapper");
                hook(
                    &wrapper,
                    "git",
                    "#!/bin/sh\ncase \"$*\" in\n  *'config --local --add branch.Feature/Exact_Name.merge'*)\n    if test \"$TWIGSMITH_FAIL_AFTER_EFFECT\" = 1; then \"$TWIGSMITH_REAL_GIT\" \"$@\" || exit $?; fi\n    printf 'fixture second config failure\\n' >&2\n    exit 1;;\nesac\nexec \"$TWIGSMITH_REAL_GIT\" \"$@\"\n",
                );
                let real = Command::new("sh")
                    .args(["-c", "command -v git"])
                    .output()
                    .unwrap();
                assert!(real.status.success());
                let real = String::from_utf8(real.stdout).unwrap();
                let path = format!("{}:{}", wrapper.display(), env::var("PATH").unwrap());
                for after_effect in ["0", "1"] {
                    // Environment changes are confined to an isolated subprocess.
                    let output = Command::new(env::current_exe().unwrap())
                    .args(["--exact", "second_tracking_write_failure_preserves_and_reports_actual_config_effects", "--nocapture"])
                    .env("PATH", &path).env("TWIGSMITH_REAL_GIT", real.trim())
                    .env("TWIGSMITH_PUBLICATION_WRAPPER", "1")
                    .env("TWIGSMITH_FAIL_AFTER_EFFECT", after_effect).output().unwrap();
                    assert!(
                        output.status.success(),
                        "{}\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
                return;
            }
            let (repo, remote) = fixture(root);
            let report = publish(&repo);
            assert_eq!(report.state, PublicationState::Complete, "{report:?}");
            assert_eq!(report.tracking.writes, [TrackingWrite::Remote]);
            assert!(matches!(
                report.error,
                Some(PublishError::TrackingGit { .. })
            ));
            assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
            assert_eq!(has_config(&repo, "remote").unwrap(), b"origin\n");
            if env::var("TWIGSMITH_FAIL_AFTER_EFFECT").unwrap() == "1" {
                assert_eq!(report.tracking.state, TrackingState::Matching);
                assert_eq!(
                    has_config(&repo, "merge").unwrap(),
                    format!("{REF}\n").as_bytes()
                );
            } else {
                assert_eq!(report.tracking.state, TrackingState::Incomplete);
                assert!(has_config(&repo, "merge").is_none());
                assert_eq!(
                    report.tracking.after.as_ref().unwrap().upstream.remotes,
                    [b"origin".to_vec()]
                );
                assert!(
                    report
                        .tracking
                        .after
                        .as_ref()
                        .unwrap()
                        .upstream
                        .merge_refs
                        .is_empty()
                );
            }
            assert!(!report.is_complete());
        },
    );
}

#[test]
fn push_url_rewrites_use_gits_resolved_push_destinations() {
    isolated(
        "push_url_rewrites_use_gits_resolved_push_destinations",
        true,
        |root| {
            let (repo, fetch) = fixture(root);
            let push = bare(root, "rewritten-push.git");
            repo.success(&[
                "config",
                &format!("url.{}.pushInsteadOf", push.display()),
                &fetch.display().to_string(),
            ]);
            let report = publish(&repo);
            assert_success(&report, DestinationState::Created);
            assert_eq!(
                report.configured_destinations,
                [push.as_os_str().as_encoded_bytes()]
            );
            assert_eq!(oid(&fetch, REF), None);
            assert_eq!(oid(&push, REF), oid(repo.path(), "HEAD"));
        },
    );
}

#[test]
fn legacy_remote_file_mappings_are_excluded_from_exact_transport() {
    isolated(
        "legacy_remote_file_mappings_are_excluded_from_exact_transport",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            repo.success(&["remote", "remove", "origin"]);
            fs::create_dir(repo.path().join(".git/remotes")).unwrap();
            fs::write(
                repo.path().join(".git/remotes/origin"),
                format!(
                    "URL: {}\nPull: +refs/heads/*:refs/heads/*\n",
                    remote.display()
                ),
            )
            .unwrap();
            let check = preserved_local(&repo);
            let report = publish(&repo);
            assert_success(&report, DestinationState::Created);
            assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
            check();
        },
    );
}

#[test]
fn submodule_push_expansion_is_contained_and_nonmutating_checks_are_respected() {
    isolated(
        "submodule_push_expansion_is_contained_and_nonmutating_checks_are_respected",
        true,
        |root| {
            for policy in ["on-demand", "only", "check", "implicit"] {
                let (parent, parent_remote) = fixture(root);
                let child = Repository::new(root);
                let child_base = commit(&child, "child base\n");
                let child_remote = bare(root, &format!("submodule-{policy}.git"));
                child.success(&[
                    "remote".as_ref(),
                    "add".as_ref(),
                    "origin".as_ref(),
                    child_remote.as_os_str(),
                ]);
                child.success(&["push", "origin", "HEAD:refs/heads/main"]);
                parent.success(&[
                    "-c".as_ref(),
                    "protocol.file.allow=always".as_ref(),
                    "submodule".as_ref(),
                    "add".as_ref(),
                    child.path().as_os_str(),
                    "module".as_ref(),
                ]);
                let module = parent.path().join("module");
                for args in [
                    vec!["config", "user.name", "Fixture"],
                    vec!["config", "user.email", "fixture@example.invalid"],
                ] {
                    assert!(git::run(&module, args).unwrap().status.success());
                }
                assert!(
                    git::run(
                        &module,
                        [
                            "remote".as_ref(),
                            "set-url".as_ref(),
                            "origin".as_ref(),
                            child_remote.as_os_str()
                        ]
                    )
                    .unwrap()
                    .status
                    .success()
                );
                fs::write(module.join("tracked"), "unpublished child\n").unwrap();
                assert!(
                    git::run(&module, ["commit", "-am", "unpublished child"])
                        .unwrap()
                        .status
                        .success()
                );
                let child_work = oid(&module, "HEAD").unwrap();
                parent.success(&["add", ".gitmodules", "module"]);
                parent.success(&["commit", "-m", "new child revision"]);
                parent.success(&["config", "submodule.recurse", "true"]);
                if policy != "implicit" {
                    parent.success(&["config", "push.recurseSubmodules", policy]);
                }
                let report = publish(&parent);
                if policy == "check" {
                    assert_eq!(report.state, PublicationState::Unconfirmed, "{report:?}");
                    assert_eq!(oid(&parent_remote, REF), None);
                } else {
                    assert_success(&report, DestinationState::Created);
                    assert_eq!(oid(&parent_remote, REF), oid(parent.path(), "HEAD"));
                }
                assert_eq!(oid(&child_remote, "refs/heads/main").unwrap(), child_base);
                assert_eq!(oid(&module, "HEAD").unwrap(), child_work);
            }
        },
    );
}

#[test]
fn fetch_refspecs_without_local_destinations_do_not_block_publication() {
    isolated(
        "fetch_refspecs_without_local_destinations_do_not_block_publication",
        true,
        |root| {
            for mapping in [REF.to_owned(), format!("{REF}:")] {
                let (repo, remote) = fixture(root);
                repo.success(&["config", "remote.origin.fetch", &mapping]);
                let check = preserved_local(&repo);
                let report = publish(&repo);
                assert_success(&report, DestinationState::Created);
                assert!(
                    report
                        .tracking
                        .after
                        .as_ref()
                        .unwrap()
                        .remote_refs
                        .is_empty()
                );
                assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
                check();
            }
        },
    );
}

#[test]
fn cache_refresh_honors_custom_mappings_exclusions_and_shared_destinations() {
    isolated(
        "cache_refresh_honors_custom_mappings_exclusions_and_shared_destinations",
        true,
        |root| {
            for case in [
                "pattern",
                "duplicate",
                "excluded",
                "same-remote-alias",
                "other-remote-alias",
            ] {
                let (repo, remote) = fixture(root);
                let cache = "refs/remotes/custom/Exact_Name";
                let mapping = if case == "pattern" {
                    "+refs/heads/Feature/*:refs/remotes/custom/*".to_owned()
                } else {
                    format!("+{REF}:{cache}")
                };
                repo.success(&["config", "remote.origin.fetch", &mapping]);
                match case {
                    "duplicate" => {
                        repo.success(&["config", "--add", "remote.origin.fetch", &mapping]);
                    }
                    "excluded" => {
                        repo.success(&[
                            "config",
                            "--add",
                            "remote.origin.fetch",
                            &format!("^{REF}"),
                        ]);
                    }
                    "same-remote-alias" => {
                        repo.success(&[
                            "config",
                            "--add",
                            "remote.origin.fetch",
                            &format!("+refs/heads/other:{cache}"),
                        ]);
                    }
                    "other-remote-alias" => {
                        repo.success(&["remote", "add", "other", remote.to_str().unwrap()]);
                        repo.success(&["config", "remote.other.fetch", &format!("+{REF}:{cache}")]);
                    }
                    _ => (),
                }
                let report = publish(&repo);
                assert!(report.is_complete(), "{case}: {report:#?}");
                assert_eq!(oid(&remote, REF), oid(repo.path(), "HEAD"));
                assert_eq!(
                    oid(repo.path(), cache),
                    matches!(case, "pattern" | "duplicate")
                        .then(|| oid(repo.path(), "HEAD").unwrap())
                );
                assert_eq!(
                    report.caches[0].has_issue(),
                    case.ends_with("alias"),
                    "{case}: {report:#?}"
                );
                assert_eq!(
                    oid(repo.path(), &format!("refs/remotes/origin/{BRANCH}")),
                    None
                );
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn cache_refresh_preserves_newer_concurrent_and_grafted_observations() {
    isolated(
        "cache_refresh_preserves_newer_concurrent_and_grafted_observations",
        true,
        |root| {
            for case in ["newer", "concurrent", "same-oid-recreated", "grafted"] {
                let (repo, remote) = fixture(root);
                let source = oid(repo.path(), "HEAD").unwrap();
                let newer = commit(&repo, "later observation\n");
                repo.success(&["reset", "--hard", &source]);
                let cache = format!("refs/remotes/origin/{BRANCH}");
                let original = if case == "same-oid-recreated" {
                    &source
                } else {
                    &newer
                };
                if case != "concurrent" {
                    repo.success(&["update-ref", &cache, original]);
                }
                if case == "grafted" {
                    fs::create_dir_all(repo.path().join(".git/info")).unwrap();
                    fs::write(
                        repo.path().join(".git/info/grafts"),
                        format!("{source} {newer}\n"),
                    )
                    .unwrap();
                }
                if matches!(case, "concurrent" | "same-oid-recreated") {
                    let hooks = local_hooks(&repo);
                    let body = if case == "same-oid-recreated" {
                        format!(
                            "git update-ref -d '{cache}'\ngit update-ref -m 'concurrent recreation' '{cache}' {source}"
                        )
                    } else {
                        format!("git update-ref '{cache}' {newer}")
                    };
                    hook(&hooks, "pre-push", &format!("#!/bin/sh\n{body}\n"));
                }
                let report = publish(&repo);
                assert!(report.is_complete(), "{case}: {report:#?}");
                assert_eq!(oid(&remote, REF), Some(source.clone()));
                assert_eq!(oid(repo.path(), &cache), Some(original.clone()));
                assert!(report.caches[0].has_issue(), "{case}: {report:#?}");
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn cache_transaction_refuses_symbolic_replacement_and_survives_lock_failure() {
    isolated(
        "cache_transaction_refuses_symbolic_replacement_and_survives_lock_failure",
        true,
        |root| {
            for case in ["symbolic", "locked", "remote-moved", "workflow-changed"] {
                let (repo, remote) = fixture(root);
                let source = oid(repo.path(), "HEAD").unwrap();
                let cache = format!("refs/remotes/origin/{BRANCH}");
                let path = repo.path().join(".git").join(&cache);
                if case == "locked" {
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path.with_extension("lock"), "held").unwrap();
                } else {
                    let hooks = local_hooks(&repo);
                    let effect = match case {
                        "symbolic" => format!("printf 'ref: refs/heads/main\\n' > '.git/{cache}'"),
                        "remote-moved" => format!(
                            "git --git-dir='{}' update-ref -d '{REF}'",
                            remote.to_str().unwrap().replace('\'', "'\\''")
                        ),
                        "workflow-changed" => "git config remote.pushDefault different".into(),
                        _ => unreachable!(),
                    };
                    // Run only for cache transactions; source/remote publication
                    // and tracking configuration are distinct effects.
                    hook(
                        &hooks,
                        "reference-transaction",
                        &format!(
                            "#!/bin/sh\ninput=$(cat)\nif test \"$1\" = prepared && printf '%s' \"$input\" | grep -q '{cache}'; then\n{effect}\nfi\n"
                        ),
                    );
                }
                let report = publish(&repo);
                assert_eq!(
                    report.state,
                    PublicationState::Complete,
                    "{case}: {report:#?}"
                );
                assert_eq!(oid(repo.path(), "main"), Some(source.clone()));
                assert_eq!(
                    report.caches[0].has_issue(),
                    case != "remote-moved",
                    "{case}: {report:#?}"
                );
                assert_eq!(
                    report.is_complete(),
                    case != "workflow-changed",
                    "{case}: {report:#?}"
                );
                if case == "symbolic" {
                    assert_eq!(
                        repo.success(&["symbolic-ref", &cache]).stdout,
                        b"refs/heads/main\n"
                    );
                } else if case == "remote-moved" {
                    // The acknowledgement is an observation of delivery. A later
                    // remote deletion does not invalidate that cached observation
                    // or establish that it remains the remote's current value.
                    assert_eq!(oid(repo.path(), &cache), Some(source.clone()));
                } else if case != "workflow-changed" {
                    assert_eq!(oid(repo.path(), &cache), None);
                }
                assert_eq!(
                    oid(&remote, REF),
                    (case != "remote-moved").then_some(source.clone())
                );
                if case == "locked" {
                    fs::remove_file(path.with_extension("lock")).unwrap();
                    let retry = publish(&repo);
                    assert!(retry.is_complete(), "{retry:#?}");
                    assert!(!retry.caches[0].has_issue(), "{retry:#?}");
                    assert_eq!(oid(repo.path(), &cache), Some(source));
                }
            }
        },
    );
}
