#[path = "support/snapshot.rs"]
mod preservation;
mod support;
use git_oneflow::{
    finish::{
        DestinationEvidence, RefObservation,
        integration::{
            self, Blocker, Effect, Integration, Plan, RemoteEvidence, Request, RequiredInput,
            State,
            recovery::{self, Problem, Record, Selection},
        },
    },
    git,
    settings::{FinishCheckout, FinishCleanup, FinishIntegration, FinishUpdate},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::{Repository, isolated};
use tempfile::TempDir;

struct Fixture {
    repo: Repository,
    project: TempDir,
    fork: TempDir,
}
fn oid(repo: &Repository, reference: &str) -> Vec<u8> {
    repo.success(&["rev-parse", reference])
        .stdout
        .strip_suffix(b"\n")
        .unwrap()
        .to_vec()
}
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}
fn commit(repo: &Repository, file: &str, content: &str) -> Vec<u8> {
    fs::write(repo.path().join(file), content).unwrap();
    repo.success(&["add", file]);
    repo.success(&["commit", "-m", content]);
    oid(repo, "HEAD")
}
fn fixture(root: &Path) -> Fixture {
    let repo = Repository::new(root);
    commit(&repo, "base", "base");
    let project = tempfile::tempdir_in(root).unwrap();
    let fork = tempfile::tempdir_in(root).unwrap();
    for remote in [&project, &fork] {
        assert!(
            git::run(
                root,
                [
                    "clone".as_ref(),
                    "--bare".as_ref(),
                    repo.path().as_os_str(),
                    remote.path().as_os_str()
                ]
            )
            .unwrap()
            .status
            .success()
        );
    }
    repo.success(&["remote", "add", "project", project.path().to_str().unwrap()]);
    repo.success(&["remote", "add", "fork", fork.path().to_str().unwrap()]);
    fs::write(repo.path().join(".git/git-oneflow.config.toml"), "base-remote='project'\npublish-remote='fork'\nfinish-message='auto'\nfinish-mode='integrate'\n").unwrap();
    repo.success(&["switch", "-c", "A"]);
    commit(&repo, "work", "work A");
    Fixture {
        repo,
        project,
        fork,
    }
}
fn request(method: FinishIntegration) -> Request {
    Request {
        integration: Some(method),
        update: Some(FinishUpdate::None),
        cleanup: Some(FinishCleanup::Keep),
        ..Request::default()
    }
}
fn observed(path: &Path, reference: &[u8]) -> RefObservation {
    let output = git::run(path, ["rev-parse", "--verify", "--quiet", text(reference)]).unwrap();
    if output.status.success() {
        RefObservation::Commit(output.stdout.strip_suffix(b"\n").unwrap().to_vec())
    } else {
        RefObservation::Absent
    }
}
fn evidence(f: &Fixture, req: &Request) -> RemoteEvidence {
    let plan = integration::assess(f.repo.path(), req, None).unwrap();
    RemoteEvidence {
        target: plan.target.clone(),
        fetched: observed(f.project.path(), &plan.target.reference),
        targets: plan
            .target
            .push_destinations
            .iter()
            .map(|d| DestinationEvidence {
                destination: d.clone(),
                branch: observed(Path::new(text(d)), &plan.target.reference),
            })
            .collect(),
        work_remote: "fork".into(),
        work_ref: plan.source_ref.clone(),
        work: vec![DestinationEvidence {
            destination: f.fork.path().as_os_str().as_encoded_bytes().to_vec(),
            branch: observed(f.fork.path(), &plan.source_ref),
        }],
    }
}
fn assess(root: &Path, f: &Fixture, req: &Request, e: Option<&RemoteEvidence>) -> Plan {
    let before = preservation::snapshot(root);
    let plan = integration::assess(f.repo.path(), req, e).unwrap();
    assert_eq!(
        before,
        preservation::snapshot(root),
        "assessment wrote repository contents/metadata"
    );
    plan
}
fn revalidate(root: &Path, f: &Fixture, plan: &Plan, e: Option<&RemoteEvidence>) -> bool {
    let before = preservation::snapshot(root);
    let valid = integration::revalidate(f.repo.path(), plan, e).is_ok();
    assert_eq!(
        before,
        preservation::snapshot(root),
        "revalidation wrote repository contents/metadata"
    );
    valid
}
fn private_write(root: &Path, path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    let mut current = path.to_owned();
    while current != root {
        fs::set_permissions(
            &current,
            fs::Permissions::from_mode(if current.is_dir() { 0o700 } else { 0o600 }),
        )
        .unwrap();
        current.pop();
    }
}
fn record(_f: &Fixture, plan: &Plan) -> (Record, PathBuf) {
    let record = Record::for_plan(plan, "fixture-attempt").unwrap();
    let path = plan.recovery_path.clone().unwrap();
    private_write(
        &plan.repository.git_dir,
        &path,
        &serde_json::to_vec(&record).unwrap(),
    );
    (record, path)
}
fn select(f: &Fixture, plan: &Plan) {
    let selection = Selection::observe(
        f.repo.path(),
        text(&plan.source_ref[b"refs/heads/".len()..]),
    )
    .unwrap();
    private_write(
        &plan.repository.git_dir,
        &recovery::selection_path(&plan.repository),
        &serde_json::to_vec(&selection).unwrap(),
    );
}
fn marked(f: &Fixture, record: &Record, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(f.repo.path())
        .args(args)
        .env("GIT_REFLOG_ACTION", record.marker())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn integrate(f: &Fixture, plan: &Plan, record: &Record) -> Vec<u8> {
    let source = text(&plan.source_ref[b"refs/heads/".len()..]);
    f.repo.success(&["switch", "main"]);
    match plan.method {
        FinishIntegration::FfOnly => marked(f, record, &["merge", "--ff-only", source]),
        FinishIntegration::Merge => marked(
            f,
            record,
            &["merge", "--no-ff", "-m", "integrate work", source],
        ),
        FinishIntegration::Squash => {
            marked(f, record, &["merge", "--squash", source]);
            marked(f, record, &["commit", "-m", "squash A"]);
        }
        _ => panic!("resolved method"),
    }
    select(f, plan);
    oid(&f.repo, "HEAD")
}
fn publish(f: &Fixture) {
    f.repo
        .success(&["push", "project", "refs/heads/main:refs/heads/main"]);
}

#[test]
fn integration_methods_pin_exact_roles_and_leave_everything_unchanged() {
    isolated(
        "integration_methods_pin_exact_roles_and_leave_everything_unchanged",
        true,
        |root| {
            for (method, shape) in [
                (FinishIntegration::FfOnly, Integration::FastForward),
                (FinishIntegration::Merge, Integration::MergeCommit),
                (FinishIntegration::Squash, Integration::SquashCommit),
            ] {
                let f = fixture(root);
                let req = request(method);
                let e = evidence(&f, &req);
                let plan = assess(root, &f, &req, Some(&e));
                assert!(plan.is_ready(), "{plan:?}");
                assert_eq!(plan.integration, shape);
                assert_eq!(plan.target.remote, "project");
                assert_eq!(plan.source_ref, b"refs/heads/A");
                assert_eq!(plan.target.reference, b"refs/heads/main");
                assert_eq!(
                    plan.target.push_destinations,
                    [f.project.path().as_os_str().as_encoded_bytes()]
                );
                assert!(matches!(
                    plan.permitted_effects(),
                    [Effect::Integrate { .. }]
                ));
                assert!(revalidate(root, &f, &plan, Some(&e)));
                let (rec, _) = record(&f, &plan);
                let result = integrate(&f, &plan, &rec);
                let pending = assess(root, &f, &req, Some(&e));
                assert_eq!(pending.state, State::PendingPublication, "{pending:?}");
                assert!(pending.is_ready());
                assert_eq!(pending.result, Some(result.clone()));
                assert!(
                    matches!(pending.effects.as_slice(), [Effect::CheckResult { .. }, Effect::PublishTarget { remote, reference, .. }] if remote == "project" && reference == b"refs/heads/main")
                );
                if method == FinishIntegration::Squash {
                    assert_ne!(result, oid(&f.repo, "A"));
                    assert_eq!(
                        git::run(f.repo.path(), ["merge-base", "--is-ancestor", "A", "main"])
                            .unwrap()
                            .status
                            .code(),
                        Some(1)
                    );
                }
                publish(&f);
                let fresh = evidence(&f, &req);
                let completed = assess(root, &f, &req, Some(&fresh));
                assert_eq!(completed.state, State::Complete, "{completed:?}");
                assert!(!revalidate(root, &f, &pending, Some(&fresh)));
            }
        },
    );
}
#[test]
fn divergent_history_requires_independent_update_and_ff_proof() {
    isolated(
        "divergent_history_requires_independent_update_and_ff_proof",
        true,
        |root| {
            let f = fixture(root);
            f.repo.success(&["switch", "main"]);
            commit(&f.repo, "base", "advanced base");
            publish(&f);
            f.repo.success(&["switch", "A"]);
            let mut req = request(FinishIntegration::FfOnly);
            let e = evidence(&f, &req);
            let blocked = assess(root, &f, &req, Some(&e));
            assert!(blocked.blockers.contains(&Blocker::FastForwardRequired));
            assert!(blocked.permitted_effects().is_empty());
            req.update = None;
            let unresolved = assess(root, &f, &req, Some(&e));
            assert!(unresolved.required_input.contains(&RequiredInput::Update));
            req.update = Some(FinishUpdate::Rebase);
            let prepare = assess(root, &f, &req, Some(&e));
            assert_eq!(prepare.integration, Integration::AfterPreparation);
            assert!(matches!(
                prepare.effects.as_slice(),
                [Effect::UpdateWork {
                    method: FinishUpdate::Rebase,
                    ..
                }]
            ));
            assert!(Record::for_plan(&prepare, "early").is_err());
            // Real preparation confirms the relationship needed for ff-only.
            f.repo.success(&["rebase", "main"]);
            let prepared = assess(root, &f, &req, Some(&e));
            assert_eq!(prepared.integration, Integration::FastForward);
            assert_eq!(prepared.update, FinishUpdate::None);
            // Work can also be integrated directly with a non-ff merge or squash.
            for method in [FinishIntegration::Merge, FinishIntegration::Squash] {
                let f = fixture(root);
                f.repo.success(&["switch", "main"]);
                commit(&f.repo, "base", "new base");
                publish(&f);
                f.repo.success(&["switch", "A"]);
                let req = request(method);
                let e = evidence(&f, &req);
                let p = assess(root, &f, &req, Some(&e));
                assert!(p.is_ready());
                assert_eq!(p.update, FinishUpdate::None);
                let (rec, _) = record(&f, &p);
                integrate(&f, &p, &rec);
                assert_eq!(
                    assess(root, &f, &req, Some(&e)).state,
                    State::PendingPublication
                );
            }
        },
    );
}
#[test]
fn no_op_and_meaningful_choices() {
    isolated("no_op_and_meaningful_choices", true, |root| {
        let f = fixture(root);
        let mut req = request(FinishIntegration::Ask);
        f.repo.success(&["reset", "--hard", "main"]);
        let e = evidence(&f, &req);
        let p = assess(root, &f, &req, Some(&e));
        assert!(p.is_ready());
        assert_eq!(p.integration, Integration::AlreadyIntegrated);
        assert!(p.required_input.is_empty());
        f.repo.success(&["switch", "main"]);
        commit(&f.repo, "base", "base ahead");
        publish(&f);
        f.repo.success(&["switch", "A"]);
        let e = evidence(&f, &req);
        assert_eq!(
            assess(root, &f, &req, Some(&e)).integration,
            Integration::AlreadyIntegrated
        );
        commit(&f.repo, "work", "new work");
        req.integration = Some(FinishIntegration::Merge);
        req.update = Some(FinishUpdate::None);
        req.cleanup = None;
        let config = f.repo.path().join(".git/git-oneflow.config.toml");
        fs::write(
            config,
            "base-remote='project'\npublish-remote='fork'\nfinish-message='ask'\n",
        )
        .unwrap();
        let e = evidence(&f, &req);
        let p = assess(root, &f, &req, Some(&e));
        assert_eq!(
            p.required_input,
            [RequiredInput::Message, RequiredInput::Cleanup]
        );
        req.message = Some("final message".into());
        req.cleanup = Some(FinishCleanup::Local);
        req.checkout = Some(FinishCheckout::Work);
        assert!(
            assess(root, &f, &req, Some(&e))
                .blockers
                .contains(&Blocker::CheckoutCleanupConflict)
        );
    });
}
#[test]
fn shared_history_and_unrelated_local_target_work_are_protected() {
    isolated(
        "shared_history_and_unrelated_local_target_work_are_protected",
        true,
        |root| {
            let f = fixture(root);
            let req = request(FinishIntegration::Merge);
            let e = evidence(&f, &req);
            f.repo.success(&["switch", "main"]);
            commit(&f.repo, "private", "unpublished local target work");
            f.repo.success(&["switch", "A"]);
            assert!(
                assess(root, &f, &req, Some(&e))
                    .blockers
                    .contains(&Blocker::LocalTargetWork)
            );
            f.repo
                .success(&["branch", "-f", "main", text(commit_oid(&e.fetched))]);
            f.repo.success(&["switch", "--orphan", "unrelated"]);
            commit(&f.repo, "unrelated", "unrelated");
            let other = oid(&f.repo, "HEAD");
            f.repo.success(&["switch", "A"]);
            let mut changed = e.clone();
            changed.fetched = RefObservation::Commit(other.clone());
            changed.targets[0].branch = changed.fetched.clone();
            assert!(
                assess(root, &f, &req, Some(&changed))
                    .blockers
                    .contains(&Blocker::UnrelatedHistory)
            );
            changed = e.clone();
            changed.targets[0].branch = RefObservation::Commit(other);
            assert!(
                assess(root, &f, &req, Some(&changed))
                    .blockers
                    .iter()
                    .any(|b| matches!(b, Blocker::TargetChanged(_)))
            );
            let worktree = root.join("target-worktree");
            f.repo
                .success(&["worktree", "add", worktree.to_str().unwrap(), "main"]);
            assert!(
                assess(root, &f, &req, Some(&e))
                    .blockers
                    .contains(&Blocker::TargetInAnotherWorktree)
            );
        },
    );
}
fn commit_oid(o: &RefObservation) -> &[u8] {
    let RefObservation::Commit(oid) = o else {
        panic!("commit")
    };
    oid
}

#[test]
fn interrupted_a_does_not_intercept_b_and_target_requires_association() {
    isolated(
        "interrupted_a_does_not_intercept_b_and_target_requires_association",
        true,
        |root| {
            let f = fixture(root);
            let req = request(FinishIntegration::Squash);
            let e = evidence(&f, &req);
            let p = assess(root, &f, &req, Some(&e));
            let (rec, path) = record(&f, &p);
            integrate(&f, &p, &rec);
            let bytes = fs::read(&path).unwrap();
            f.repo.success(&["switch", "-c", "B", "A"]);
            commit(&f.repo, "B", "work B");
            let eb = evidence(&f, &req);
            let b = assess(root, &f, &req, Some(&eb));
            assert_eq!(b.state, State::Fresh);
            assert_eq!(b.source_ref, b"refs/heads/B");
            assert_ne!(b.recovery_path, p.recovery_path);
            assert_eq!(fs::read(&path).unwrap(), bytes);
            // B sees local unpublished target work, rather than borrowing A's result.
            assert!(b.blockers.contains(&Blocker::LocalTargetWork));
            f.repo.success(&["switch", "main"]);
            assert!(
                assess(root, &f, &req, Some(&e))
                    .blockers
                    .contains(&Blocker::Recovery(Problem::UnassociatedHead))
            );
            // Simply rewriting a private selector cannot make a B->main checkout A's.
            select(&f, &p);
            assert!(
                assess(root, &f, &req, Some(&e))
                    .blockers
                    .contains(&Blocker::Recovery(Problem::UnassociatedHead))
            );
            f.repo.success(&["switch", "A"]);
            f.repo.success(&["switch", "main"]);
            select(&f, &p);
            assert_eq!(
                assess(root, &f, &req, Some(&e)).state,
                State::PendingPublication
            );
            fs::remove_file(&path).unwrap();
            assert!(
                assess(root, &f, &req, Some(&e))
                    .blockers
                    .contains(&Blocker::Recovery(Problem::MissingRecord))
            );
        },
    );
}
#[test]
fn recovery_rejects_missing_stale_malformed_and_inconsistent_proof() {
    isolated(
        "recovery_rejects_missing_stale_malformed_and_inconsistent_proof",
        true,
        |root| {
            for case in [
                "malformed",
                "version",
                "wrong-result",
                "source-version",
                "target-version",
                "missing-log",
                "missing-source-object",
                "settings",
                "destination",
                "wrong-source",
                "squash-without-marker",
            ] {
                let f = fixture(root);
                let req = request(FinishIntegration::Squash);
                let e = evidence(&f, &req);
                let p = assess(root, &f, &req, Some(&e));
                let (mut rec, path) = record(&f, &p);
                let result = integrate(&f, &p, &rec);
                match case {
                    "malformed" => fs::write(&path, b"{broken").unwrap(),
                    "version" => {
                        let mut value = serde_json::to_value(&rec).unwrap();
                        value["version"] = 999.into();
                        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
                    }
                    "wrong-result" => {
                        rec.result = Some(oid(&f.repo, "A"));
                        fs::write(&path, serde_json::to_vec(&rec).unwrap()).unwrap();
                    }
                    "source-version" => {
                        f.repo.success(&[
                            "update-ref",
                            "refs/heads/A",
                            text(commit_oid(&e.fetched)),
                        ]);
                        f.repo.success(&[
                            "update-ref",
                            "refs/heads/A",
                            text(commit_oid(&p.source)),
                        ]);
                    }
                    "target-version" => {
                        f.repo
                            .success(&["reset", "--hard", text(commit_oid(&e.fetched))]);
                        f.repo.success(&["reset", "--hard", text(&result)]);
                        select(&f, &p);
                    }
                    "missing-log" => {
                        fs::remove_file(f.repo.path().join(".git/logs/refs/heads/main")).unwrap()
                    }
                    "missing-source-object" => {
                        let source = text(commit_oid(&p.source));
                        fs::remove_file(
                            f.repo
                                .path()
                                .join(".git/objects")
                                .join(&source[..2])
                                .join(&source[2..]),
                        )
                        .unwrap();
                    }
                    "settings" => {
                        fs::write(
                            f.repo.path().join(".git/git-oneflow.config.toml"),
                            "base-remote='project'\npublish-remote='fork'\nfinish-message='ask'\n",
                        )
                        .unwrap();
                    }
                    "destination" => {
                        f.repo.success(&[
                            "config",
                            "remote.project.pushurl",
                            f.fork.path().to_str().unwrap(),
                        ]);
                    }
                    "wrong-source" => {
                        let mut value = serde_json::to_value(&rec).unwrap();
                        value["source"] = serde_json::to_value(commit_oid(&e.fetched)).unwrap();
                        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
                    }
                    _ => {
                        let log = f.repo.path().join(".git/logs/refs/heads/main");
                        let bytes = fs::read_to_string(&log)
                            .unwrap()
                            .replace(&rec.marker(), "commit");
                        fs::write(log, bytes).unwrap();
                    }
                }
                let plan = assess(root, &f, &req, Some(&e));
                assert!(!plan.is_ready(), "{case}: {plan:?}");
                assert!(plan.permitted_effects().is_empty());
                assert!(
                    plan.blockers
                        .iter()
                        .any(|b| matches!(b, Blocker::Recovery(_))),
                    "{case}: {plan:?}"
                );
            }
        },
    );
}
#[test]
fn incomplete_merge_and_squash_are_observed_without_mutation() {
    isolated(
        "incomplete_merge_and_squash_are_observed_without_mutation",
        true,
        |root| {
            for method in [FinishIntegration::Merge, FinishIntegration::Squash] {
                let f = fixture(root);
                let req = request(method);
                let e = evidence(&f, &req);
                let p = assess(root, &f, &req, Some(&e));
                let (rec, _) = record(&f, &p);
                f.repo.success(&["switch", "main"]);
                marked(
                    &f,
                    &rec,
                    if method == FinishIntegration::Merge {
                        &["merge", "--no-ff", "--no-commit", "A"]
                    } else {
                        &["merge", "--squash", "A"]
                    },
                );
                select(&f, &p);
                let plan = assess(root, &f, &req, Some(&e));
                assert_eq!(plan.state, State::IncompleteOperation, "{plan:?}");
                assert!(!plan.is_ready());
                assert_eq!(
                    f.repo.path().join(".git/MERGE_HEAD").exists(),
                    method == FinishIntegration::Merge
                );
            }
        },
    );
}
#[test]
fn confirmed_publication_resumes_only_verified_remaining_steps() {
    isolated(
        "confirmed_publication_resumes_only_verified_remaining_steps",
        true,
        |root| {
            let f = fixture(root);
            f.repo.success(&["push", "fork", "A"]);
            let mut req = request(FinishIntegration::Squash);
            req.cleanup = Some(FinishCleanup::Both);
            let e = evidence(&f, &req);
            let p = assess(root, &f, &req, Some(&e));
            let (rec, _) = record(&f, &p);
            integrate(&f, &p, &rec);
            let without_remote = assess(root, &f, &req, None);
            assert_eq!(without_remote.state, State::PreparedResult);
            assert!(
                without_remote
                    .blockers
                    .contains(&Blocker::FreshRemoteEvidenceRequired)
            );
            publish(&f);
            let e = evidence(&f, &req);
            let delivered = assess(root, &f, &req, Some(&e));
            assert_eq!(delivered.state, State::Published);
            assert!(delivered.is_ready(), "{delivered:?}");
            assert!(
                matches!(delivered.effects.as_slice(), [Effect::DeleteLocalWork { .. }, Effect::DeletePublishedWork { remote, .. }] if remote == "fork")
            );
            f.repo.success(&["branch", "-D", "A"]);
            f.repo.success(&["push", "fork", ":refs/heads/A"]);
            let e = evidence(&f, &req);
            assert_eq!(assess(root, &f, &req, Some(&e)).state, State::Complete);
        },
    );
}
#[test]
fn fresh_remote_and_branch_changes_revoke_authority() {
    isolated(
        "fresh_remote_and_branch_changes_revoke_authority",
        true,
        |root| {
            let f = fixture(root);
            let req = request(FinishIntegration::FfOnly);
            let e = evidence(&f, &req);
            let p = assess(root, &f, &req, Some(&e));
            f.repo.success(&[
                "update-ref",
                "refs/remotes/project/main",
                text(commit_oid(&e.fetched)),
            ]);
            let no_evidence = assess(root, &f, &req, None);
            assert!(
                no_evidence
                    .blockers
                    .contains(&Blocker::FreshRemoteEvidenceRequired)
            );
            assert!(no_evidence.required_input.is_empty());
            assert!(revalidate(root, &f, &p, Some(&e)), "cache is not authority");
            for observation in [
                RefObservation::Unavailable,
                RefObservation::Ambiguous,
                RefObservation::Absent,
            ] {
                let mut changed = e.clone();
                changed.fetched = observation;
                assert!(!assess(root, &f, &req, Some(&changed)).is_ready());
                assert!(!revalidate(root, &f, &p, Some(&changed)));
            }
            let (rec, _) = record(&f, &p);
            integrate(&f, &p, &rec);
            let pending = assess(root, &f, &req, Some(&e));
            let mut changed = e.clone();
            changed.targets[0].branch = RefObservation::Unavailable;
            assert!(!assess(root, &f, &req, Some(&changed)).is_ready());
            assert!(!revalidate(root, &f, &pending, Some(&changed)));
            f.repo
                .success(&["update-ref", "refs/heads/A", text(commit_oid(&e.fetched))]);
            assert!(
                assess(root, &f, &req, Some(&e))
                    .blockers
                    .contains(&Blocker::Recovery(Problem::ChangedSource))
            );
        },
    );
}

#[test]
fn b_can_finish_while_a_has_an_interrupted_attempt() {
    isolated(
        "b_can_finish_while_a_has_an_interrupted_attempt",
        true,
        |root| {
            let f = fixture(root);
            let req = request(FinishIntegration::Merge);
            let ea = evidence(&f, &req);
            let a = assess(root, &f, &req, Some(&ea));
            let (_, path_a) = record(&f, &a);
            let a_bytes = fs::read(&path_a).unwrap();
            // Interruption after the intent record, before any target change.
            f.repo.success(&["switch", "-c", "B", "main"]);
            commit(&f.repo, "B", "work B");
            let eb = evidence(&f, &req);
            let b = assess(root, &f, &req, Some(&eb));
            assert!(b.is_ready());
            assert_eq!(b.state, State::Fresh);
            let (record_b, path_b) = record(&f, &b);
            assert_ne!(path_a, path_b);
            integrate(&f, &b, &record_b);
            publish(&f);
            let eb = evidence(&f, &req);
            assert_eq!(assess(root, &f, &req, Some(&eb)).state, State::Complete);
            assert_eq!(fs::read(path_a).unwrap(), a_bytes);
            f.repo.success(&["switch", "A"]);
            let ea = evidence(&f, &req);
            let a = assess(root, &f, &req, Some(&ea));
            assert_eq!(a.source_ref, b"refs/heads/A");
            assert!(!a.is_ready());
        },
    );
}
#[test]
fn multiple_target_endpoints_and_cleanup_use_exact_observed_versions() {
    isolated(
        "multiple_target_endpoints_and_cleanup_use_exact_observed_versions",
        true,
        |root| {
            let f = fixture(root);
            let replica = tempfile::tempdir_in(root).unwrap();
            assert!(
                git::run(
                    root,
                    [
                        "clone".as_ref(),
                        "--bare".as_ref(),
                        f.project.path().as_os_str(),
                        replica.path().as_os_str()
                    ]
                )
                .unwrap()
                .status
                .success()
            );
            f.repo.success(&[
                "config",
                "--add",
                "remote.project.pushurl",
                f.project.path().to_str().unwrap(),
            ]);
            f.repo.success(&[
                "config",
                "--add",
                "remote.project.pushurl",
                replica.path().to_str().unwrap(),
            ]);
            f.repo.success(&["push", "fork", "A"]);
            let mut req = request(FinishIntegration::Squash);
            req.cleanup = Some(FinishCleanup::Both);
            let e = evidence(&f, &req);
            let p = assess(root, &f, &req, Some(&e));
            let (rec, _) = record(&f, &p);
            integrate(&f, &p, &rec);
            // Only the first endpoint receives the result. The fetch endpoint now
            // shows the result while the other publication endpoint is still pending.
            f.repo
                .success(&["push", f.project.path().to_str().unwrap(), "main"]);
            let e = evidence(&f, &req);
            let pending = assess(root, &f, &req, Some(&e));
            assert_eq!(pending.state, State::PendingPublication);
            assert!(pending.is_ready());
            assert!(
                matches!(pending.effects.as_slice(), [Effect::CheckResult { .. }, Effect::PublishTarget { destination, .. }] if destination == replica.path().as_os_str().as_encoded_bytes())
            );
            f.repo
                .success(&["push", replica.path().to_str().unwrap(), "main"]);
            let mut e = evidence(&f, &req);
            e.work[0].branch = RefObservation::Unavailable;
            let blocked = assess(root, &f, &req, Some(&e));
            assert_eq!(blocked.state, State::Published);
            assert!(!blocked.is_ready());
            assert!(blocked.permitted_effects().is_empty());
            // A new published work version cannot be removed by the old record.
            f.repo.success(&["switch", "A"]);
            commit(&f.repo, "later", "later work");
            f.repo.success(&["push", "fork", "A"]);
            let e = evidence(&f, &req);
            assert!(!assess(root, &f, &req, Some(&e)).is_ready());
        },
    );
}
#[test]
fn local_versions_settings_and_worktree_scope_are_revalidated() {
    isolated(
        "local_versions_settings_and_worktree_scope_are_revalidated",
        true,
        |root| {
            for case in [
                "source",
                "target",
                "index",
                "dirty",
                "settings-source",
                "pushurl",
                "shallow",
                "git-config",
            ] {
                let f = fixture(root);
                let req = request(FinishIntegration::Merge);
                let e = evidence(&f, &req);
                let p = assess(root, &f, &req, Some(&e));
                match case {
                    "source" => {
                        commit(&f.repo, "new", "new work");
                    }
                    "target" => {
                        f.repo.success(&["branch", "-f", "main", "A"]);
                    }
                    "index" => {
                        f.repo
                            .success(&["update-index", "--assume-unchanged", "work"]);
                    }
                    "dirty" => {
                        fs::write(f.repo.path().join("work"), "uncommitted").unwrap();
                    }
                    "settings-source" => {
                        use std::io::Write;
                        let mut file = fs::OpenOptions::new()
                            .append(true)
                            .open(f.repo.path().join(".git/git-oneflow.config.toml"))
                            .unwrap();
                        writeln!(file, "finish-rewrite='ask'").unwrap();
                    }
                    "pushurl" => {
                        f.repo.success(&[
                            "config",
                            "remote.project.pushurl",
                            f.fork.path().to_str().unwrap(),
                        ]);
                    }
                    "shallow" => {
                        fs::write(
                            f.repo.path().join(".git/shallow"),
                            [commit_oid(&p.source), b"\n"].concat(),
                        )
                        .unwrap();
                    }
                    _ => {
                        f.repo.success(&["config", "merge.ff", "false"]);
                    }
                }
                assert!(!revalidate(root, &f, &p, Some(&e)), "{case}");
            }
            let f = fixture(root);
            let req = request(FinishIntegration::Merge);
            let e = evidence(&f, &req);
            let p = assess(root, &f, &req, Some(&e));
            let (_, path) = record(&f, &p);
            f.repo.success(&["switch", "-c", "B", "main"]);
            let other = root.join("linked-A");
            f.repo
                .success(&["worktree", "add", other.to_str().unwrap(), "A"]);
            let linked = integration::assess(&other, &req, None).unwrap();
            assert_eq!(linked.state, State::Fresh);
            assert_ne!(linked.recovery_path, Some(path));
            assert!(
                !linked
                    .blockers
                    .iter()
                    .any(|b| matches!(b, Blocker::Recovery(_)))
            );
        },
    );
}
#[test]
fn recovery_requires_private_paths_and_genuine_result_parentage() {
    isolated(
        "recovery_requires_private_paths_and_genuine_result_parentage",
        true,
        |root| {
            for case in ["permissions", "symlink", "wrong-parents", "unknown-field"] {
                let f = fixture(root);
                let req = request(FinishIntegration::Squash);
                let e = evidence(&f, &req);
                let p = assess(root, &f, &req, Some(&e));
                let (rec, path) = record(&f, &p);
                match case {
                    "permissions" => {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                    }
                    "symlink" => {
                        let saved = path.with_extension("saved");
                        fs::rename(&path, &saved).unwrap();
                        std::os::unix::fs::symlink(saved, &path).unwrap();
                    }
                    "wrong-parents" => {
                        // A correctly marked ordinary merge is not a squash result.
                        f.repo.success(&["switch", "main"]);
                        marked(&f, &rec, &["merge", "--no-ff", "-m", "wrong shape", "A"]);
                        select(&f, &p);
                    }
                    _ => {
                        let mut value = serde_json::to_value(&rec).unwrap();
                        value["publication_confirmed"] = true.into();
                        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
                    }
                }
                assert!(!assess(root, &f, &req, Some(&e)).is_ready(), "{case}");
            }
        },
    );
}

#[test]
fn ordinary_git_merge_continuation_keeps_the_target_attempt_association() {
    isolated(
        "ordinary_git_merge_continuation_keeps_the_target_attempt_association",
        true,
        |root| {
            let f = fixture(root);
            commit(&f.repo, "base", "source edit");
            f.repo.success(&["switch", "main"]);
            commit(&f.repo, "base", "target edit");
            publish(&f);
            f.repo.success(&["switch", "A"]);
            let req = request(FinishIntegration::Merge);
            let e = evidence(&f, &req);
            let p = assess(root, &f, &req, Some(&e));
            let (_, _) = record(&f, &p);
            f.repo.success(&["switch", "main"]);
            select(&f, &p);
            assert!(
                !git::run(f.repo.path(), ["merge", "--no-ff", "-m", "merge A", "A"])
                    .unwrap()
                    .status
                    .success()
            );
            assert_eq!(
                assess(root, &f, &req, Some(&e)).state,
                State::IncompleteOperation
            );
            fs::write(f.repo.path().join("base"), "resolved edits").unwrap();
            f.repo.success(&["add", "base"]);
            f.repo
                .success(&["-c", "core.editor=true", "merge", "--continue"]);
            // Neither the intent record nor selector is rewritten after continuation.
            let pending = assess(root, &f, &req, Some(&e));
            assert_eq!(pending.state, State::PendingPublication, "{pending:?}");
            assert!(pending.is_ready());
        },
    );
}

#[test]
fn confirmed_delivery_preserves_work_checked_out_elsewhere() {
    isolated(
        "confirmed_delivery_preserves_work_checked_out_elsewhere",
        true,
        |root| {
            let f = fixture(root);
            let mut req = request(FinishIntegration::Merge);
            req.cleanup = Some(FinishCleanup::Local);
            let e = evidence(&f, &req);
            let p = assess(root, &f, &req, Some(&e));
            let (rec, _) = record(&f, &p);
            integrate(&f, &p, &rec);
            publish(&f);
            let other = root.join("source-worktree");
            f.repo
                .success(&["worktree", "add", other.to_str().unwrap(), "A"]);
            let e = evidence(&f, &req);
            let delivered = assess(root, &f, &req, Some(&e));
            assert_eq!(delivered.state, State::Published);
            assert!(
                delivered
                    .blockers
                    .contains(&Blocker::SourceInAnotherWorktree)
            );
            assert!(delivered.permitted_effects().is_empty());
        },
    );
}
