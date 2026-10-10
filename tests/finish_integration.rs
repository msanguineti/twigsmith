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
            assert!(Record::for_plan(&prepare, "early").is_ok());
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

use integration::execution;
fn prepare_local(f: &Fixture, req: &Request) -> execution::Report {
    let e = evidence(f, req);
    let p = integration::assess(f.repo.path(), req, Some(&e)).unwrap();
    execution::prepare(f.repo.path(), &p)
}
fn hook(f: &Fixture, name: &str, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let hooks = f.repo.success(&["config", "core.hooksPath"]);
    let path = Path::new(text(hooks.stdout.strip_suffix(b"\n").unwrap())).join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn diverge(f: &Fixture, conflict: bool) {
    f.repo.success(&["switch", "main"]);
    commit(
        &f.repo,
        if conflict { "work" } else { "target" },
        "target work",
    );
    publish(f);
    f.repo.success(&["switch", "A"]);
}
#[test]
fn local_executor_prepares_all_methods_and_reuses_results() {
    isolated(
        "local_executor_prepares_all_methods_and_reuses_results",
        true,
        |root| {
            for method in [
                FinishIntegration::FfOnly,
                FinishIntegration::Merge,
                FinishIntegration::Squash,
            ] {
                for update in [
                    FinishUpdate::None,
                    FinishUpdate::Merge,
                    FinishUpdate::Rebase,
                ] {
                    let f = fixture(root);
                    if update != FinishUpdate::None {
                        diverge(&f, false);
                    }
                    let original = oid(&f.repo, "A");
                    let base = oid(&f.repo, "main");
                    let remote_before = preservation::snapshot(f.project.path());
                    let fork_before = preservation::snapshot(f.fork.path());
                    let mut req = request(method);
                    req.update = Some(update);
                    req.message = Some("Final message\n\nLiteral `text` $value".into());
                    let report = prepare_local(&f, &req);
                    assert!(report.is_prepared(), "{report:#?}");
                    let p = report.plan.unwrap();
                    let result = p.result.clone().unwrap();
                    assert_eq!(result, oid(&f.repo, "HEAD"));
                    assert_eq!(
                        fs::read_to_string(f.repo.path().join("work")).unwrap(),
                        "work A"
                    );
                    if update != FinishUpdate::None {
                        assert!(f.repo.path().join("target").exists());
                    }
                    let parents = f
                        .repo
                        .success(&["show", "-s", "--format=%P", text(&result)])
                        .stdout;
                    let prepared = oid(&f.repo, "A");
                    match method {
                        FinishIntegration::FfOnly => assert_eq!(result, prepared),
                        FinishIntegration::Merge => assert_eq!(
                            text(&parents).trim(),
                            format!("{} {}", text(&base), text(&prepared))
                        ),
                        FinishIntegration::Squash => assert_eq!(text(&parents).trim(), text(&base)),
                        _ => unreachable!(),
                    }
                    if method != FinishIntegration::FfOnly {
                        assert_eq!(
                            text(
                                &f.repo
                                    .success(&["show", "-s", "--format=%B", "HEAD"])
                                    .stdout
                            )
                            .trim(),
                            req.message.as_deref().unwrap()
                        );
                    }
                    let record: Record = serde_json::from_slice(
                        &fs::read(p.recovery_path.as_ref().unwrap()).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(oid(&f.repo, &record.original_ref()), original);
                    assert_eq!(oid(&f.repo, &record.result_ref()), result);
                    assert!(
                        execution::check(f.repo.path(), &p, false, |_| {})
                            .checks
                            .error
                            .is_none()
                    );
                    let retried = prepare_local(&f, &req);
                    assert!(retried.is_prepared(), "{retried:#?}");
                    assert_eq!(retried.plan.unwrap().result, Some(result));
                    assert_eq!(preservation::snapshot(f.project.path()), remote_before);
                    assert_eq!(preservation::snapshot(f.fork.path()), fork_before);
                }
            }
        },
    );
}
#[test]
fn local_executor_handles_missing_behind_and_already_integrated_targets() {
    isolated(
        "local_executor_handles_missing_behind_and_already_integrated_targets",
        true,
        |root| {
            for case in ["missing", "behind", "already"] {
                let f = fixture(root);
                let req = request(FinishIntegration::Merge);
                match case {
                    "missing" => {
                        f.repo.success(&["branch", "-D", "main"]);
                    }
                    "behind" => {
                        let old = oid(&f.repo, "main");
                        diverge(&f, false);
                        f.repo.success(&["merge", "--no-edit", "main"]);
                        f.repo.success(&["branch", "-f", "main", text(&old)]);
                    }
                    _ => {
                        f.repo.success(&["push", "project", "A:main"]);
                    }
                }
                let r = prepare_local(&f, &req);
                assert!(r.is_prepared(), "{case}: {r:#?}");
                assert!(
                    execution::check(f.repo.path(), r.plan.as_ref().unwrap(), false, |_| {})
                        .checks
                        .error
                        .is_none()
                );
            }
        },
    );
}

#[test]
fn local_executor_conflicts_use_git_continuation_and_retain_original_work() {
    isolated(
        "local_executor_conflicts_use_git_continuation_and_retain_original_work",
        true,
        |root| {
            for (update, method) in [
                (FinishUpdate::None, FinishIntegration::Merge),
                (FinishUpdate::None, FinishIntegration::Squash),
                (FinishUpdate::Rebase, FinishIntegration::FfOnly),
                (FinishUpdate::Merge, FinishIntegration::Squash),
            ] {
                let f = fixture(root);
                diverge(&f, true);
                let original = oid(&f.repo, "A");
                let mut req = request(method);
                req.update = Some(update);
                let failed = prepare_local(&f, &req);
                assert!(failed.error.is_some(), "{failed:#?}");
                assert_eq!(
                    failed.plan.as_ref().unwrap().state,
                    State::IncompleteOperation,
                    "{failed:#?}"
                );
                assert!(
                    failed
                        .guidance()
                        .contains(if update == FinishUpdate::Rebase {
                            "git rebase --continue"
                        } else if update == FinishUpdate::Merge
                            || method == FinishIntegration::Merge
                        {
                            "git merge --continue"
                        } else {
                            "Squash has no merge continuation"
                        })
                );
                fs::write(f.repo.path().join("work"), "resolved work").unwrap();
                f.repo.success(&["add", "work"]);
                if update == FinishUpdate::Rebase {
                    f.repo
                        .success(&["-c", "core.editor=true", "rebase", "--continue"]);
                } else if update == FinishUpdate::Merge || method == FinishIntegration::Merge {
                    f.repo
                        .success(&["-c", "core.editor=true", "merge", "--continue"]);
                } else {
                    let record: Record = serde_json::from_slice(
                        &fs::read(
                            failed
                                .plan
                                .as_ref()
                                .unwrap()
                                .recovery_path
                                .as_ref()
                                .unwrap(),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    marked(&f, &record, &["commit", "-m", "resolved squash"]);
                }
                let complete = prepare_local(&f, &req);
                assert!(complete.is_prepared(), "{complete:#?}");
                assert_eq!(
                    fs::read_to_string(f.repo.path().join("work")).unwrap(),
                    "resolved work"
                );
                let record: Record = serde_json::from_slice(
                    &fs::read(complete.plan.unwrap().recovery_path.unwrap()).unwrap(),
                )
                .unwrap();
                assert_eq!(oid(&f.repo, &record.original_ref()), original);
            }
        },
    );
}
#[test]
fn local_executor_abort_never_counts_as_integration() {
    isolated(
        "local_executor_abort_never_counts_as_integration",
        true,
        |root| {
            for update in [
                FinishUpdate::None,
                FinishUpdate::Merge,
                FinishUpdate::Rebase,
            ] {
                let f = fixture(root);
                diverge(&f, true);
                let mut req = request(FinishIntegration::Merge);
                req.update = Some(update);
                let failed = prepare_local(&f, &req);
                assert!(failed.error.is_some());
                f.repo.success(&[
                    if update == FinishUpdate::Rebase {
                        "rebase"
                    } else {
                        "merge"
                    },
                    "--abort",
                ]);
                let e = evidence(&f, &req);
                let p = integration::assess(f.repo.path(), &req, Some(&e)).unwrap();
                assert!(p.result.is_none(), "{p:#?}");
                assert!(!matches!(
                    p.state,
                    State::PendingPublication | State::Complete | State::PreparedResult
                ));
                assert!(
                    !p.effects
                        .iter()
                        .any(|e| matches!(e, Effect::PublishTarget { .. }))
                );
                assert!(p.is_ready(), "{:?} {:?}", p.state, p.blockers);
                let retry = execution::prepare(f.repo.path(), &p);
                assert_eq!(
                    retry.plan.as_ref().unwrap().state,
                    State::IncompleteOperation
                );
                assert!(
                    retry
                        .commands
                        .iter()
                        .any(|c| c.arguments.iter().any(|a| a == "merge" || a == "rebase"))
                );
            }
        },
    );
}
#[test]
fn local_executor_hooks_messages_and_unresolved_input_preserve_state() {
    isolated(
        "local_executor_hooks_messages_and_unresolved_input_preserve_state",
        true,
        |root| {
            for method in [FinishIntegration::Merge, FinishIntegration::Squash] {
                let f = fixture(root);
                let req = request(method);
                let path = hook(&f, "commit-msg", "exit 1");
                let failed = prepare_local(&f, &req);
                assert!(failed.error.is_some());
                assert_eq!(
                    failed.plan.as_ref().unwrap().state,
                    State::IncompleteOperation,
                    "{failed:#?}"
                );
                assert!(
                    failed
                        .effects
                        .iter()
                        .any(|e| matches!(e, execution::Effect::Retained { .. }))
                );
                fs::remove_file(path).unwrap();
                if method == FinishIntegration::Merge {
                    f.repo
                        .success(&["-c", "core.editor=true", "merge", "--continue"]);
                } else {
                    let record: Record = serde_json::from_slice(
                        &fs::read(failed.plan.unwrap().recovery_path.unwrap()).unwrap(),
                    )
                    .unwrap();
                    marked(&f, &record, &["commit", "-m", "finished squash"]);
                }
                assert!(prepare_local(&f, &req).is_prepared());
            }
            let f = fixture(root);
            fs::write(
                f.repo.path().join(".git/git-oneflow.config.toml"),
                "base-remote='project'\npublish-remote='fork'\nfinish-message='ask'\n",
            )
            .unwrap();
            let mut req = request(FinishIntegration::Merge);
            let e = evidence(&f, &req);
            let p = integration::assess(f.repo.path(), &req, Some(&e)).unwrap();
            assert!(p.required_input.contains(&RequiredInput::Message));
            let before = preservation::snapshot(root);
            let failed = execution::prepare(f.repo.path(), &p);
            assert!(failed.commands.is_empty());
            assert_eq!(preservation::snapshot(root), before);
            req.integration = Some(FinishIntegration::FfOnly);
            assert!(prepare_local(&f, &req).is_prepared());
        },
    );
}
#[test]
fn local_executor_interrupted_a_can_be_followed_by_b() {
    isolated(
        "local_executor_interrupted_a_can_be_followed_by_b",
        true,
        |root| {
            let f = fixture(root);
            let req = request(FinishIntegration::Merge);
            let path = hook(&f, "post-checkout", "exit 1");
            let stopped = prepare_local(&f, &req);
            assert!(stopped.error.is_some());
            assert!(
                stopped
                    .effects
                    .iter()
                    .any(|e| matches!(e, execution::Effect::CheckedOut { .. }))
            );
            fs::remove_file(path).unwrap();
            let record_path = stopped.plan.unwrap().recovery_path.unwrap();
            let bytes = fs::read(&record_path).unwrap();
            f.repo.success(&["switch", "-c", "B", "main"]);
            commit(&f.repo, "B", "work B");
            let b = prepare_local(&f, &req);
            assert!(b.is_prepared(), "{b:#?}");
            assert_eq!(b.plan.unwrap().source_ref, b"refs/heads/B");
            assert_eq!(fs::read(record_path).unwrap(), bytes);
            assert!(!f.repo.path().join("work").exists());
            assert!(observed(f.repo.path(), b"refs/heads/A") != RefObservation::Absent);
        },
    );
}
fn configure_checks(f: &Fixture, commands: &[&str]) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(f.repo.path().join(".git/git-oneflow.config.toml"))
        .unwrap();
    for command in commands {
        writeln!(
            file,
            "[[finish-checks]]\nname='Check'\ncommand=['sh', '-c', {}]",
            serde_json::to_string(command).unwrap()
        )
        .unwrap();
    }
}
#[test]
fn candidate_checks_inspect_the_result_and_retain_it_on_failure() {
    isolated(
        "candidate_checks_inspect_the_result_and_retain_it_on_failure",
        true,
        |root| {
            for script in [
                "test -f target && test -f work",
                "exit 9",
                "git switch A",
                "echo changed >> work",
                "git config finish.changed true",
                "git update-index --assume-unchanged work",
                "git commit --allow-empty -m unexpected",
                "git update-ref refs/heads/A HEAD",
            ] {
                let f = fixture(root);
                diverge(&f, false);
                configure_checks(&f, &[script, "exit 0"]);
                let req = request(FinishIntegration::Squash);
                let report = prepare_local(&f, &req);
                assert!(report.is_prepared(), "{report:#?}");
                let p = report.plan.unwrap();
                let result = p.result.clone().unwrap();
                let remote_before = preservation::snapshot(f.project.path());
                let checks = execution::check(f.repo.path(), &p, false, |_| {}).checks;
                if script.starts_with("test ") {
                    assert_eq!(checks.completed, 2, "{checks:#?}");
                    assert!(checks.error.is_none());
                    assert_eq!(
                        execution::check(f.repo.path(), &p, false, |_| {})
                            .checks
                            .completed,
                        2
                    );
                } else {
                    assert!(checks.error.is_some(), "{script}: {checks:#?}");
                    assert_eq!(checks.completed, 0);
                }
                let record: Record =
                    serde_json::from_slice(&fs::read(p.recovery_path.unwrap()).unwrap()).unwrap();
                assert_eq!(oid(&f.repo, &record.result_ref()), result);
                assert_eq!(preservation::snapshot(f.project.path()), remote_before);
            }
        },
    );
}

fn evidence_at(path: &Path, req: &Request) -> RemoteEvidence {
    let plan = integration::assess(path, req, None).unwrap();
    RemoteEvidence {
        fetched: observed(
            Path::new(text(&plan.target.fetch_destination)),
            &plan.target.reference,
        ),
        targets: plan
            .target
            .push_destinations
            .iter()
            .map(|d| DestinationEvidence {
                destination: d.clone(),
                branch: observed(Path::new(text(d)), &plan.target.reference),
            })
            .collect(),
        target: plan.target,
        work_remote: plan.settings.publish_remote,
        work_ref: plan.source_ref,
        work: vec![],
    }
}
#[test]
fn local_executor_new_process_recovers_a_completed_unacknowledged_commit() {
    if let Some(path) = std::env::var_os("ONEFLOW_RESUME_REPO") {
        let path = Path::new(&path);
        let req = request(
            if std::env::var("ONEFLOW_RESUME_METHOD").unwrap() == "merge" {
                FinishIntegration::Merge
            } else {
                FinishIntegration::Squash
            },
        );
        let e = evidence_at(path, &req);
        let p = integration::assess(path, &req, Some(&e)).unwrap();
        assert_eq!(p.state, State::PendingPublication, "{p:#?}");
        let before = p.result.clone();
        let p = if !p.is_ready() {
            let stopped = execution::prepare(path, &p);
            assert!(stopped.guidance().contains("git merge --quit"));
            assert!(
                git::run(path, ["merge", "--quit"])
                    .unwrap()
                    .status
                    .success()
            );
            integration::assess(path, &req, Some(&e)).unwrap()
        } else {
            p
        };
        let r = execution::prepare(path, &p);
        assert!(
            r.is_prepared(),
            "{:?} {:?}",
            r.error,
            r.plan.as_ref().map(|p| (&p.state, &p.blockers))
        );
        assert_eq!(r.plan.unwrap().result, before);
        return;
    }
    isolated(
        "local_executor_new_process_recovers_a_completed_unacknowledged_commit",
        true,
        |root| {
            for method in [FinishIntegration::Merge, FinishIntegration::Squash] {
                let f = fixture(root);
                let req = request(method);
                let hook = hook(
                    &f,
                    if method == FinishIntegration::Merge {
                        "post-merge"
                    } else {
                        "post-commit"
                    },
                    "kill -TERM \"$PPID\"",
                );
                let failed = prepare_local(&f, &req);
                assert!(failed.error.is_some(), "{failed:#?}");
                let p = failed.plan.unwrap();
                assert_eq!(p.state, State::PendingPublication, "{p:#?}");
                assert!(
                    failed
                        .effects
                        .iter()
                        .any(|e| matches!(e, execution::Effect::Integrated { .. }))
                );
                fs::remove_file(hook).unwrap();
                let target_log = fs::read(f.repo.path().join(".git/logs/refs/heads/main")).unwrap();
                let output = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "local_executor_new_process_recovers_a_completed_unacknowledged_commit",
                        "--nocapture",
                    ])
                    .env("ONEFLOW_RESUME_REPO", f.repo.path())
                    .env("ONEFLOW_RESUME_METHOD", method.as_str())
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}\n{}",
                    text(&output.stdout),
                    text(&output.stderr)
                );
                assert_eq!(
                    fs::read(f.repo.path().join(".git/logs/refs/heads/main")).unwrap(),
                    target_log
                );
            }
        },
    );
}
#[test]
fn local_executor_interruption_before_history_and_changed_evidence_stop_safely() {
    isolated(
        "local_executor_interruption_before_history_and_changed_evidence_stop_safely",
        true,
        |root| {
            let f = fixture(root);
            let req = request(FinishIntegration::Merge);
            let hook = hook(
                &f,
                "reference-transaction",
                "if test \"$1\" = prepared; then exit 1; fi",
            );
            let failed = prepare_local(&f, &req);
            assert!(failed.error.is_some());
            assert!(
                failed
                    .effects
                    .iter()
                    .any(|e| matches!(e, execution::Effect::Recorded(_)))
            );
            assert_eq!(
                text(&f.repo.success(&["branch", "--show-current"]).stdout).trim(),
                "A"
            );
            fs::remove_file(hook).unwrap();
            let retried = prepare_local(&f, &req);
            assert!(retried.is_prepared(), "{retried:#?}");
            let p = retried.plan.unwrap();
            let rec: Record =
                serde_json::from_slice(&fs::read(p.recovery_path.as_ref().unwrap()).unwrap())
                    .unwrap();
            f.repo.success(&["update-ref", &rec.original_ref(), "main"]);
            let before = preservation::snapshot(root);
            let stopped = execution::prepare(f.repo.path(), &p);
            assert!(stopped.error.is_some());
            assert!(stopped.commands.is_empty());
            assert_eq!(preservation::snapshot(root), before);
            let fresh =
                integration::assess(f.repo.path(), &req, Some(&evidence(&f, &req))).unwrap();
            assert!(
                fresh
                    .blockers
                    .contains(&Blocker::Recovery(Problem::InvalidResult))
            );
        },
    );
}
#[test]
fn local_executor_revalidates_remote_and_unrelated_local_target_history() {
    isolated(
        "local_executor_revalidates_remote_and_unrelated_local_target_history",
        true,
        |root| {
            for remote_change in [false, true] {
                let f = fixture(root);
                let req = request(FinishIntegration::Merge);
                let e = evidence(&f, &req);
                let p = integration::assess(f.repo.path(), &req, Some(&e)).unwrap();
                if remote_change {
                    f.repo.success(&["push", "project", "A:main"]);
                } else {
                    f.repo.success(&["switch", "main"]);
                    commit(&f.repo, "unrelated", "unrelated local target work");
                    f.repo.success(&["switch", "A"]);
                }
                let target = oid(&f.repo, "main");
                let stopped = execution::prepare(f.repo.path(), &p);
                assert!(stopped.error.is_some());
                assert!(stopped.effects.is_empty());
                assert_eq!(oid(&f.repo, "main"), target);
                assert!(!p.recovery_path.unwrap().exists());
            }
        },
    );
}
#[test]
fn candidate_checks_cancel_skip_and_revalidate_each_attempt() {
    isolated(
        "candidate_checks_cancel_skip_and_revalidate_each_attempt",
        true,
        |root| {
            let f = fixture(root);
            configure_checks(
                &f,
                &[
                    "kill -TERM $PPID; while :; do sleep 1; done",
                    "touch .git/later",
                ],
            );
            let req = request(FinishIntegration::Squash);
            let p = prepare_local(&f, &req).plan.unwrap();
            let r = execution::check(f.repo.path(), &p, false, |_| {}).checks;
            assert!(r.error.is_some(), "{r:#?}");
            assert_eq!(r.completed, 0);
            assert!(!f.repo.path().join(".git/later").exists());
            assert_eq!(oid(&f.repo, "main"), p.result.clone().unwrap());
            let skipped = execution::check(f.repo.path(), &p, true, |_| {}).checks;
            assert!(skipped.skipped && skipped.error.is_none());
            // Check observations do not modify the plan or become stored authority.
            integration::revalidate(f.repo.path(), &p, p.evidence.as_ref()).unwrap();
        },
    );
}
#[test]
fn preparation_can_resolve_final_message_after_git_recovery() {
    isolated(
        "preparation_can_resolve_final_message_after_git_recovery",
        true,
        |root| {
            let f = fixture(root);
            diverge(&f, false);
            fs::write(
                f.repo.path().join(".git/git-oneflow.config.toml"),
                "base-remote='project'\npublish-remote='fork'\nfinish-message='ask'\n",
            )
            .unwrap();
            let mut req = request(FinishIntegration::Merge);
            req.update = Some(FinishUpdate::Rebase);
            let prepared = prepare_local(&f, &req);
            assert!(prepared.error.is_some());
            assert!(
                prepared
                    .plan
                    .as_ref()
                    .unwrap()
                    .required_input
                    .contains(&RequiredInput::Message)
            );
            assert_eq!(
                oid(&f.repo, "main"),
                commit_oid(&evidence(&f, &req).fetched)
            );
            req.message = Some("Resolved final message".into());
            let done = prepare_local(&f, &req);
            assert!(done.is_prepared(), "{done:#?}");
        },
    );
}

#[test]
fn local_executor_keeps_signing_effective_and_refuses_non_ff_history() {
    isolated(
        "local_executor_keeps_signing_effective_and_refuses_non_ff_history",
        true,
        |root| {
            let f = fixture(root);
            diverge(&f, false);
            let req = request(FinishIntegration::FfOnly);
            let p = integration::assess(f.repo.path(), &req, Some(&evidence(&f, &req))).unwrap();
            let before = preservation::snapshot(root);
            let r = execution::prepare(f.repo.path(), &p);
            assert!(r.error.is_some());
            assert!(r.commands.is_empty());
            assert_eq!(preservation::snapshot(root), before);
            let signer = hook(
                &f,
                "fixture-signer",
                "echo fixture-signing-attempt >&2; exit 1",
            );
            f.repo
                .success(&["config", "gpg.program", signer.to_str().unwrap()]);
            f.repo.success(&["config", "commit.gpgsign", "true"]);
            f.repo.success(&["config", "user.signingkey", "fixture"]);
            let r = prepare_local(&f, &request(FinishIntegration::Merge));
            assert!(r.error.is_some());
            assert!(r.commands.iter().any(|c| {
                c.result
                    .as_ref()
                    .is_ok_and(|o| text(&o.stderr).contains("fixture-signing-attempt"))
            }));
            assert_eq!(r.plan.unwrap().state, State::IncompleteOperation);
        },
    );
}
#[test]
fn local_executor_recovers_preparation_when_git_stops_before_target_work() {
    isolated(
        "local_executor_recovers_preparation_when_git_stops_before_target_work",
        true,
        |root| {
            let f = fixture(root);
            diverge(&f, false);
            let original = oid(&f.repo, "A");
            let mut req = request(FinishIntegration::Merge);
            req.update = Some(FinishUpdate::Rebase);
            // post-checkout is reached only after the rebase has been verified and
            // before integration. It deterministically loses the checkout acknowledgement.
            let h = hook(&f, "post-checkout", "exit 1");
            let stopped = prepare_local(&f, &req);
            assert!(stopped.error.is_some());
            fs::remove_file(h).unwrap();
            let updated = oid(&f.repo, "A");
            assert_ne!(updated, original);
            let source_log = fs::read(f.repo.path().join(".git/logs/refs/heads/A")).unwrap();
            let done = prepare_local(&f, &req);
            assert!(
                done.is_prepared(),
                "{:?} {:?}",
                done.error,
                done.plan.as_ref().map(|p| (&p.state, &p.blockers))
            );
            assert_eq!(
                fs::read(f.repo.path().join(".git/logs/refs/heads/A")).unwrap(),
                source_log
            );
        },
    );
}
#[test]
fn local_executor_records_are_scoped_to_linked_worktrees() {
    isolated(
        "local_executor_records_are_scoped_to_linked_worktrees",
        true,
        |root| {
            let f = fixture(root);
            f.repo.success(&["switch", "-c", "B", "main"]);
            let linked = root.join("linked integration");
            f.repo
                .success(&["worktree", "add", linked.to_str().unwrap(), "A"]);
            let req = request(FinishIntegration::Squash);
            let e = evidence_at(&linked, &req);
            let p = integration::assess(&linked, &req, Some(&e)).unwrap();
            let done = execution::prepare(&linked, &p);
            assert!(
                done.is_prepared(),
                "{:?} {:?}",
                done.error,
                done.plan.as_ref().map(|p| (&p.state, &p.blockers))
            );
            let selected = done.plan.unwrap();
            assert!(
                selected
                    .recovery_path
                    .unwrap()
                    .starts_with(f.repo.path().join(".git/worktrees"))
            );
            assert_eq!(
                text(&f.repo.success(&["branch", "--show-current"]).stdout).trim(),
                "B"
            );
            assert_eq!(
                integration::assess(f.repo.path(), &req, None)
                    .unwrap()
                    .source_ref,
                b"refs/heads/B"
            );
        },
    );
}

#[test]
fn local_effects_survive_a_hook_invalidating_configuration() {
    isolated(
        "local_effects_survive_a_hook_invalidating_configuration",
        true,
        |root| {
            let f = fixture(root);
            let req = request(FinishIntegration::Merge);
            let original = oid(&f.repo, "main");
            hook(
                &f,
                "post-merge",
                "printf 'invalid = [' > \"$(git rev-parse --git-common-dir)/git-oneflow.config.toml\"",
            );
            let r = prepare_local(&f, &req);
            assert!(r.error.is_some());
            assert!(r.plan.is_none());
            let result = oid(&f.repo, "main");
            assert_ne!(result, original);
            assert!(r.effects.contains(&execution::Effect::TargetChanged {
                before: RefObservation::Commit(original),
                after: RefObservation::Commit(result),
            }));
        },
    );
}

#[test]
fn local_target_alignment_updates_its_worktree_when_resuming_on_target() {
    isolated(
        "local_target_alignment_updates_its_worktree_when_resuming_on_target",
        true,
        |root| {
            let f = fixture(root);
            let old = oid(&f.repo, "main");
            diverge(&f, false);
            let base = oid(&f.repo, "main");
            f.repo.success(&["branch", "-f", "main", text(&old)]);
            let req = request(FinishIntegration::Merge);
            let e = evidence(&f, &req);
            let p = integration::assess(f.repo.path(), &req, Some(&e)).unwrap();
            record(&f, &p);
            f.repo.success(&["switch", "main"]);
            select(&f, &p);
            let r = prepare_local(&f, &req);
            assert!(
                r.is_prepared(),
                "{:?} {:?}",
                r.error,
                r.plan.as_ref().map(|p| (&p.state, &p.blockers))
            );
            assert_eq!(oid(&f.repo, "main^1"), base);
            assert!(f.repo.path().join("target").exists());
            assert!(f.repo.success(&["status", "--porcelain"]).stdout.is_empty());
        },
    );
}
