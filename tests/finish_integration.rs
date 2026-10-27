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
        preparation: None,
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
fn guarded_choice_resolution_rejects_changes_during_input() {
    isolated(
        "guarded_choice_resolution_rejects_changes_during_input",
        true,
        |root| {
            for changed in [false, true] {
                let f = fixture(root);
                let req = Request::default();
                let p = assess(root, &f, &req, Some(&evidence(&f, &req)));
                assert!(p.required_input.contains(&RequiredInput::Integration));
                if changed {
                    f.repo.success(&["config", "oneflow.concurrent", "changed"]);
                }
                let resolved =
                    integration::resolve(f.repo.path(), &p, &request(FinishIntegration::FfOnly));
                if changed {
                    assert!(matches!(
                        resolved,
                        Err(git_oneflow::finish::FinishError::ContextChanged)
                    ));
                } else {
                    assert!(resolved.unwrap().is_ready());
                }
            }
        },
    );
}

#[test]
fn preparation_resolves_integration_and_message_against_actual_graph() {
    isolated(
        "preparation_resolves_integration_and_message_against_actual_graph",
        true,
        |root| {
            let f = fixture(root);
            diverge(&f, false);
            fs::write(
                f.repo.path().join(".git/git-oneflow.config.toml"),
                "base-remote='project'\npublish-remote='fork'\nfinish-message='ask'\n",
            )
            .unwrap();
            let req = Request {
                update: Some(FinishUpdate::Rebase),
                cleanup: Some(FinishCleanup::Keep),
                ..Request::default()
            };
            let initial = assess(root, &f, &req, Some(&evidence(&f, &req)));
            assert!(initial.is_ready(), "{initial:#?}");
            assert_eq!(initial.integration, Integration::AfterPreparation);
            let prepared = execution::prepare(f.repo.path(), &initial);
            let prepared = prepared.plan.unwrap();
            assert_eq!(prepared.required_input, [RequiredInput::Integration]);
            let mut choices = prepared.request().clone();
            choices.integration = Some(FinishIntegration::Merge);
            let reviewed = integration::resolve(f.repo.path(), &prepared, &choices).unwrap();
            assert_eq!(reviewed.required_input, [RequiredInput::Message]);
            assert_eq!(reviewed.message_suggestion(), "Integrate A into main");
            assert!(reviewed.can_retain_choices());
            let reviewed = integration::retain_choices(f.repo.path(), &reviewed).unwrap();
            let omitted = integration::assess(
                f.repo.path(),
                &Request::default(),
                reviewed.evidence.as_ref(),
            )
            .unwrap();
            assert_eq!(omitted.method, FinishIntegration::Merge);
            assert_eq!(omitted.required_input, [RequiredInput::Message]);
            choices.message = Some("Subject\n\nLiteral body with $() and `text`.".into());
            let resolved = integration::resolve(f.repo.path(), &reviewed, &choices).unwrap();
            assert!(resolved.is_ready(), "{resolved:#?}");
            let done = execution::prepare(f.repo.path(), &resolved);
            assert!(done.is_prepared(), "{done:#?}");
            let retry = integration::assess(f.repo.path(), &Request::default(), None).unwrap();
            assert!(retry.required_input.is_empty(), "{retry:#?}");
            assert_eq!(retry.message(), choices.message.as_deref());
            assert_eq!(retry.method, FinishIntegration::Merge);
        },
    );
}

#[test]
fn retained_intent_supports_omitted_choices_and_blocks_conflicts_or_changed_settings() {
    isolated(
        "retained_intent_supports_omitted_choices_and_blocks_conflicts_or_changed_settings",
        true,
        |root| {
            for method in [
                FinishIntegration::FfOnly,
                FinishIntegration::Merge,
                FinishIntegration::Squash,
            ] {
                let f = fixture(root);
                fs::write(
                    f.repo.path().join(".git/git-oneflow.config.toml"),
                    "base-remote='project'\npublish-remote='fork'\nfinish-message='ask'\n",
                )
                .unwrap();
                let mut req = request(method);
                req.message = Some("Accepted integration\n\nRetained body".into());
                let prepared = prepare_local(&f, &req);
                assert!(prepared.is_prepared(), "{prepared:#?}");
                let retry = refreshed(&f, &Request::default());
                assert!(retry.is_ready(), "{retry:#?}");
                assert_eq!(retry.result, prepared.plan.unwrap().result);
                assert_eq!(retry.message(), req.message.as_deref());
                assert_eq!(retry.method, method);
                assert_eq!(retry.cleanup, FinishCleanup::Keep);
                for conflict in [
                    Request {
                        integration: Some(if method == FinishIntegration::Merge {
                            FinishIntegration::Squash
                        } else {
                            FinishIntegration::Merge
                        }),
                        ..Request::default()
                    },
                    Request {
                        update: Some(FinishUpdate::Merge),
                        ..Request::default()
                    },
                    Request {
                        checkout: Some(FinishCheckout::Work),
                        ..Request::default()
                    },
                    Request {
                        cleanup: Some(FinishCleanup::Local),
                        ..Request::default()
                    },
                    Request {
                        message: Some("Different intent".into()),
                        ..Request::default()
                    },
                ] {
                    let blocked = assess(root, &f, &conflict, retry.evidence.as_ref());
                    assert!(
                        blocked
                            .blockers
                            .contains(&Blocker::Recovery(Problem::ChangedBinding)),
                        "{blocked:#?}"
                    );
                    assert!(blocked.permitted_effects().is_empty());
                }
                fs::write(
                    f.repo.path().join(".git/git-oneflow.config.toml"),
                    "base-remote='project'\npublish-remote='fork'\nfinish-message='auto'\n",
                )
                .unwrap();
                let blocked = assess(root, &f, &Request::default(), retry.evidence.as_ref());
                assert!(
                    blocked
                        .blockers
                        .contains(&Blocker::Recovery(Problem::ChangedBinding))
                );
            }
        },
    );
}

#[test]
fn completed_attempt_reuses_omitted_intent_after_local_and_remote_cleanup() {
    isolated(
        "completed_attempt_reuses_omitted_intent_after_local_and_remote_cleanup",
        true,
        |root| {
            let f = fixture(root);
            f.repo.success(&["push", "fork", "A"]);
            let req = completion_request(
                FinishIntegration::Squash,
                FinishCleanup::Both,
                FinishCheckout::Target,
            );
            let delivered = delivered(&f, &req);
            let completed = completion::complete(f.repo.path(), &delivered);
            assert!(completed.is_complete(), "{completed:#?}");
            let retry = refreshed(&f, &Request::default());
            assert!(retry.is_ready(), "{retry:#?}");
            assert_eq!(retry.state, State::Complete);
            assert_eq!(retry.cleanup, FinishCleanup::Both);
            assert_eq!(retry.source, RefObservation::Absent);
            let done = completion::complete(f.repo.path(), &retry);
            assert!(done.is_complete(), "{done:#?}");
            assert!(done.effects.is_empty());
            assert_eq!(completion_pushes(&done), 0);
        },
    );
}

#[test]
fn corroborated_result_normalization_preserves_delivered_final_checkout() {
    isolated(
        "corroborated_result_normalization_preserves_delivered_final_checkout",
        true,
        |root| {
            let f = fixture(root);
            let req = completion_request(
                FinishIntegration::Merge,
                FinishCleanup::Keep,
                FinishCheckout::Work,
            );
            let e = evidence(&f, &req);
            let original = assess(root, &f, &req, Some(&e));
            let (record, _) = record(&f, &original);
            let result = integrate(&f, &original, &record);
            let pending = refreshed(&f, &Request::default());
            assert!(pending.needs_result_retention());
            assert!(!pending.needs_candidate_checkout());
            publish(&f);
            f.repo.success(&["switch", "A"]);
            let delivered = refreshed(&f, &Request::default());
            assert_eq!(delivered.state, State::Complete);
            assert!(delivered.needs_result_retention());
            assert!(!delivered.needs_candidate_checkout());
            let log = fs::read(f.repo.path().join(".git/logs/HEAD")).unwrap();
            let normalized = execution::prepare(f.repo.path(), &delivered);
            assert!(normalized.is_prepared(), "{normalized:#?}");
            assert!(
                !normalized
                    .commands
                    .iter()
                    .any(|command| command.arguments.iter().any(|arg| arg == "switch"))
            );
            let plan = normalized.plan.unwrap();
            assert_eq!(plan.result, Some(result));
            assert!(!plan.needs_result_retention());
            assert_eq!(fs::read(f.repo.path().join(".git/logs/HEAD")).unwrap(), log);
            assert_eq!(f.repo.success(&["branch", "--show-current"]).stdout, b"A\n");
        },
    );
}

#[test]
fn candidate_checkout_preserves_ignored_files_initially_and_on_pending_retry() {
    isolated(
        "candidate_checkout_preserves_ignored_files_initially_and_on_pending_retry",
        true,
        |root| {
            for retry in [false, true] {
                let f = fixture(root);
                let req = request(FinishIntegration::Merge);
                let path = if retry {
                    diverge(&f, false);
                    let prepared = prepare_local(&f, &req);
                    assert!(prepared.is_prepared(), "{prepared:#?}");
                    f.repo.success(&["switch", "A"]);
                    "target"
                } else {
                    f.repo.success(&["rm", "base"]);
                    f.repo.success(&["commit", "-m", "remove base"]);
                    "base"
                };
                fs::create_dir_all(f.repo.path().join(".git/info")).unwrap();
                fs::write(
                    f.repo.path().join(".git/info/exclude"),
                    format!("/{path}\n"),
                )
                .unwrap();
                fs::write(f.repo.path().join(path), "protected ignored content\n").unwrap();
                let p = refreshed(&f, &req);
                assert!(p.is_ready(), "{p:#?}");
                assert_eq!(p.needs_candidate_checkout(), retry);
                let target = oid(&f.repo, "main");
                let source = oid(&f.repo, "A");
                let stopped = execution::prepare(f.repo.path(), &p);
                assert!(stopped.error.is_some(), "{stopped:#?}");
                assert_eq!(
                    fs::read_to_string(f.repo.path().join(path)).unwrap(),
                    "protected ignored content\n"
                );
                assert_eq!(f.repo.success(&["branch", "--show-current"]).stdout, b"A\n");
                assert_eq!(oid(&f.repo, "main"), target);
                assert_eq!(oid(&f.repo, "A"), source);
            }
        },
    );
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
#[test]
fn ordinary_published_reword_cleans_only_the_retained_original_version() {
    isolated(
        "ordinary_published_reword_cleans_only_the_retained_original_version",
        true,
        |root| {
            let f = fixture(root);
            f.repo.success(&["push", "fork", "A:A"]);
            f.repo
                .success(&["config", "sequence.editor", "sed -i 's/^pick /reword /'"]);
            f.repo.success(&[
                "config",
                "core.editor",
                "sh -c 'printf \"Prepared work\\n\" > \"$1\"' -",
            ]);
            let original = oid(&f.repo, "A");
            let req = Request {
                update: Some(FinishUpdate::Rebase),
                history_edit: Some(git_oneflow::settings::HistoryEdit::Interactive),
                cleanup: Some(FinishCleanup::Both),
                ..request(FinishIntegration::FfOnly)
            };
            let prepared = prepare_local(&f, &req);
            assert!(
                prepared.is_prepared(),
                "error={:?}; blockers={:?}",
                prepared.error,
                prepared.plan.as_ref().map(|p| &p.blockers)
            );
            let source = oid(&f.repo, "A");
            assert_ne!(source, original);
            assert_eq!(oid(&f.repo, "main"), source);
            assert_eq!(
                observed(f.fork.path(), b"refs/heads/A"),
                RefObservation::Commit(original.clone())
            );
            let plan = prepared.plan.unwrap();
            let record: Record =
                serde_json::from_slice(&fs::read(plan.recovery_path.as_ref().unwrap()).unwrap())
                    .unwrap();
            assert_eq!(oid(&f.repo, &record.original_ref()), original);
            let delivered = integration::publication::publish(f.repo.path(), &plan);
            assert!(delivered.is_complete(), "{delivered:#?}");
            // Even the prepared, integrated version is a changed publication:
            // cleanup cannot replace its originally observed deletion lease.
            f.repo
                .success(&["push", "--force", "fork", "main:refs/heads/A"]);
            let changed =
                integration::assess(f.repo.path(), &req, Some(&evidence(&f, &req))).unwrap();
            assert!(
                changed
                    .blockers
                    .iter()
                    .any(|b| matches!(b, Blocker::WorkBranchChanged(_)))
            );
            let refused = integration::completion::complete(f.repo.path(), &changed);
            assert!(!refused.is_complete());
            assert_eq!(
                observed(f.repo.path(), b"refs/heads/A"),
                RefObservation::Commit(source.clone())
            );
            assert_eq!(
                observed(f.fork.path(), b"refs/heads/A"),
                RefObservation::Commit(source.clone())
            );
            f.repo.success(&[
                "push",
                "--force",
                "fork",
                &format!("{}:refs/heads/A", text(&original)),
            ]);
            let restored =
                integration::assess(f.repo.path(), &req, Some(&evidence(&f, &req))).unwrap();
            let completed = integration::completion::complete(f.repo.path(), &restored);
            assert!(completed.is_complete(), "{completed:#?}");
            assert_eq!(
                observed(f.repo.path(), b"refs/heads/A"),
                RefObservation::Absent
            );
            assert_eq!(
                observed(f.fork.path(), b"refs/heads/A"),
                RefObservation::Absent
            );
            assert_eq!(
                observed(f.project.path(), b"refs/heads/main"),
                RefObservation::Commit(source)
            );
        },
    );
}

#[test]
fn ordinary_integration_preserves_settings_across_tracked_configuration_checkout() {
    isolated(
        "ordinary_integration_preserves_settings_across_tracked_configuration_checkout",
        true,
        |root| {
            for method in [
                FinishIntegration::FfOnly,
                FinishIntegration::Merge,
                FinishIntegration::Squash,
            ] {
                let f = fixture(root);
                // The work commit introduces the shared routes; main has no file.
                let config = fs::read(f.repo.path().join(".git/git-oneflow.config.toml")).unwrap();
                fs::remove_file(f.repo.path().join(".git/git-oneflow.config.toml")).unwrap();
                fs::write(f.repo.path().join("git-oneflow.config.toml"), &config).unwrap();
                f.repo.success(&["add", "git-oneflow.config.toml"]);
                f.repo.success(&["commit", "-m", "Shared project settings"]);
                let req = request(method);
                let before = integration::assess(f.repo.path(), &req, None)
                    .unwrap()
                    .settings;
                let prepared = prepare_local(&f, &req);
                assert!(
                    prepared.is_prepared(),
                    "{method:?}: error={:?}; blockers={:?}",
                    prepared.error,
                    prepared.plan.as_ref().map(|p| &p.blockers)
                );
                assert_eq!(prepared.plan.as_ref().unwrap().settings, before);
                assert_eq!(
                    fs::read(f.repo.path().join("git-oneflow.config.toml")).unwrap(),
                    config
                );
                let delivered = integration::publication::publish(
                    f.repo.path(),
                    prepared.plan.as_ref().unwrap(),
                );
                assert!(delivered.is_complete(), "{delivered:#?}");
            }
        },
    );
}

#[test]
fn ordinary_preparation_imports_shared_configuration_without_changing_accepted_checks() {
    isolated(
        "ordinary_preparation_imports_shared_configuration_without_changing_accepted_checks",
        true,
        |root| {
            for update in [FinishUpdate::Merge, FinishUpdate::Rebase] {
                let f = fixture(root);
                f.repo.success(&["switch", "main"]);
                commit(
                    &f.repo,
                    "git-oneflow.config.toml",
                    "[[finish-checks]]\nname='New project check'\ncommand=['sh','-c','exit 99']\n",
                );
                publish(&f);
                f.repo.success(&["switch", "A"]);
                let req = Request {
                    update: Some(update),
                    ..request(FinishIntegration::FfOnly)
                };
                let prepared = prepare_local(&f, &req);
                assert!(
                    prepared.is_prepared(),
                    "{update:?}: error={:?}; blockers={:?}",
                    prepared.error,
                    prepared.plan.as_ref().map(|p| &p.blockers)
                );
                assert!(
                    prepared
                        .plan
                        .as_ref()
                        .unwrap()
                        .settings
                        .finish_checks
                        .is_empty()
                );
                assert!(f.repo.path().join("git-oneflow.config.toml").exists());
                let delivered = integration::publication::publish(
                    f.repo.path(),
                    prepared.plan.as_ref().unwrap(),
                );
                assert!(delivered.is_complete(), "{delivered:#?}");
            }
        },
    );
}

#[test]
fn ordinary_integration_keeps_ignored_shared_configuration_masked_by_private_settings() {
    isolated(
        "ordinary_integration_keeps_ignored_shared_configuration_masked_by_private_settings",
        true,
        |root| {
            let f = fixture(root);
            fs::create_dir_all(f.repo.path().join(".git/info")).unwrap();
            fs::write(
                f.repo.path().join(".git/info/exclude"),
                "git-oneflow.config.toml\n",
            )
            .unwrap();
            let shared = "publish-remote='other'\nbase-remote='other'\n";
            fs::write(f.repo.path().join("git-oneflow.config.toml"), shared).unwrap();
            let req = request(FinishIntegration::FfOnly);
            let prepared = prepare_local(&f, &req);
            assert!(
                prepared.is_prepared(),
                "error={:?}; blockers={:?}",
                prepared.error,
                prepared.plan.as_ref().map(|p| &p.blockers)
            );
            assert_eq!(
                fs::read_to_string(f.repo.path().join("git-oneflow.config.toml")).unwrap(),
                shared
            );
            let refreshed = integration::assess(f.repo.path(), &Request::default(), None).unwrap();
            assert!(
                !refreshed
                    .blockers
                    .iter()
                    .any(|b| matches!(b, Blocker::Recovery(Problem::ChangedBinding)))
            );
            let delivered =
                integration::publication::publish(f.repo.path(), prepared.plan.as_ref().unwrap());
            assert!(delivered.is_complete(), "{delivered:#?}");
        },
    );
}
#[test]
fn tagged_integration_binds_ignored_shared_configuration_across_recovery() {
    isolated(
        "tagged_integration_binds_ignored_shared_configuration_across_recovery",
        true,
        |root| {
            let f = fixture(root);
            fs::remove_file(f.repo.path().join(".git/git-oneflow.config.toml")).unwrap();
            fs::create_dir_all(f.repo.path().join(".git/info")).unwrap();
            fs::write(
                f.repo.path().join(".git/info/exclude"),
                "git-oneflow.config.toml\n",
            )
            .unwrap();
            let path = f.repo.path().join("git-oneflow.config.toml");
            let config = "base-remote='project'\npublish-remote='fork'\nfinish-message='auto'\nfinish-mode='integrate'\n[workstreams.release]\ncreation-source='main'\nintegration-target='main'\nversion-tagging='required'\nfinish-mode='integrate'\nfinish-update='none'\nfinish-integration='merge'\n";
            fs::write(&path, config).unwrap();
            let started = Command::new(env!("CARGO_BIN_EXE_gof"))
                .current_dir(f.repo.path())
                .args(["start", "A", "--policy", "release"])
                .output()
                .unwrap();
            assert!(started.status.success(), "{started:?}");
            let req = Request {
                release: integration::release::Request {
                    tag: Some("v-ignored".into()),
                    ..Default::default()
                },
                ..request(FinishIntegration::Merge)
            };
            let prepared = prepare_local(&f, &req);
            assert!(prepared.is_prepared(), "{prepared:#?}");
            let retry = integration::assess(f.repo.path(), &Request::default(), None).unwrap();
            assert!(
                !retry
                    .blockers
                    .contains(&Blocker::Recovery(Problem::ChangedBinding)),
                "{retry:#?}"
            );
            fs::write(&path, format!("{config}\nfinish-cleanup='both'\n")).unwrap();
            let changed = integration::assess(f.repo.path(), &Request::default(), None).unwrap();
            assert!(
                changed
                    .blockers
                    .contains(&Blocker::Recovery(Problem::ChangedBinding)),
                "{changed:#?}"
            );
            assert!(changed.permitted_effects().is_empty());
            fs::write(&path, config).unwrap();
            let recovered = integration::assess(f.repo.path(), &Request::default(), None).unwrap();
            assert!(
                !recovered
                    .blockers
                    .contains(&Blocker::Recovery(Problem::ChangedBinding)),
                "{recovered:#?}"
            );
            let delivered =
                integration::publication::publish(f.repo.path(), prepared.plan.as_ref().unwrap());
            assert!(delivered.is_complete(), "{delivered:#?}");
        },
    );
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
        preparation: None,
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

use integration::{publication as target_publication, remote as target_remote};
fn refreshed(f: &Fixture, req: &Request) -> Plan {
    let p = integration::assess(f.repo.path(), req, None).unwrap();
    let r = target_remote::refresh(f.repo.path(), &p);
    assert!(r.error.is_none(), "{r:#?}");
    r.plan.unwrap()
}
fn remote_candidate(f: &Fixture, req: &Request) -> Plan {
    let p = refreshed(f, req);
    let r = execution::prepare(f.repo.path(), &p);
    assert!(r.is_prepared(), "{r:#?}");
    r.plan.unwrap()
}
fn pushes(r: &target_publication::Report) -> usize {
    r.destinations
        .iter()
        .flat_map(|d| &d.commands)
        .filter(|c| c.arguments.iter().any(|a| a == "push"))
        .count()
}
fn second_target(root: &Path, f: &Fixture) -> TempDir {
    let second = tempfile::tempdir_in(root).unwrap();
    assert!(
        git::run(
            root,
            [
                "clone".as_ref(),
                "--bare".as_ref(),
                f.project.path().as_os_str(),
                second.path().as_os_str()
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
        second.path().to_str().unwrap(),
    ]);
    second
}
fn advance_project(root: &Path, f: &Fixture) -> Vec<u8> {
    let other = Repository::new(root);
    other.success(&["fetch", f.project.path().to_str().unwrap(), "main"]);
    other.success(&["switch", "-C", "main", "FETCH_HEAD"]);
    let tip = commit(&other, "remote-only", "new remote work");
    other.success(&["push", f.project.path().to_str().unwrap(), "main"]);
    tip
}
fn refs_without_project_main_cache(f: &Fixture) -> Vec<u8> {
    f.repo
        .success(&["show-ref"])
        .stdout
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| !line.ends_with(b" refs/remotes/project/main\n"))
        .flatten()
        .copied()
        .collect()
}
#[test]
fn remote_refresh_and_publication_cover_methods_roles_and_retention() {
    isolated(
        "remote_refresh_and_publication_cover_methods_roles_and_retention",
        true,
        |root| {
            for method in [
                FinishIntegration::FfOnly,
                FinishIntegration::Merge,
                FinishIntegration::Squash,
            ] {
                let f = fixture(root);
                let req = request(method);
                configure_checks(
                    &f,
                    &[
                        "test \"$(git symbolic-ref --short HEAD)\" = main; echo check >> .git/candidate-checks",
                    ],
                );
                let fork = preservation::snapshot(f.fork.path());
                let p = remote_candidate(&f, &req);
                let result = p.result.clone().unwrap();
                let before_refs = refs_without_project_main_cache(&f);
                let r = target_publication::publish(f.repo.path(), &p);
                assert!(r.is_complete(), "{r:#?}");
                assert_eq!(r.checks.completed, 1);
                assert_eq!(pushes(&r), 1);
                assert_eq!(
                    observed(f.project.path(), b"refs/heads/main"),
                    RefObservation::Commit(result)
                );
                assert_eq!(fork, preservation::snapshot(f.fork.path()));
                assert_eq!(before_refs, refs_without_project_main_cache(&f));
                assert_eq!(
                    observed(f.repo.path(), b"refs/remotes/project/main"),
                    RefObservation::Commit(p.result.clone().unwrap())
                );
                assert_eq!(oid(&f.repo, "A"), commit_of(&p.source));
                assert!(matches!(
                    r.plan.as_ref().unwrap().state,
                    State::Published | State::Complete
                ));
                let retry = refreshed(&f, &req);
                let r = target_publication::publish(f.repo.path(), &retry);
                assert!(r.is_complete(), "{r:#?}");
                assert_eq!(pushes(&r), 0);
                assert_eq!(r.checks.completed, 0);
                assert_eq!(
                    fs::read_to_string(f.repo.path().join(".git/candidate-checks")).unwrap(),
                    "check\n"
                );
            }
        },
    );
}
fn commit_of(value: &RefObservation) -> Vec<u8> {
    if let RefObservation::Commit(oid) = value {
        oid.clone()
    } else {
        panic!("missing commit")
    }
}
#[test]
fn remote_refresh_acquires_exact_objects_and_preserves_local_state() {
    isolated(
        "remote_refresh_acquires_exact_objects_and_preserves_local_state",
        true,
        |root| {
            let f = fixture(root);
            let remote = advance_project(root, &f);
            assert!(
                !git::run(f.repo.path(), ["cat-file", "-e", text(&remote)])
                    .unwrap()
                    .status
                    .success()
            );
            f.repo.success(&[
                "update-ref",
                "refs/remotes/project/main",
                text(&oid(&f.repo, "A")),
            ]);
            f.repo.success(&[
                "config",
                "remote.project.fetch",
                "+refs/heads/*:refs/heads/*",
            ]);
            f.repo.success(&["config", "remote.project.mirror", "true"]);
            f.repo.success(&["config", "fetch.prune", "true"]);
            f.repo
                .success(&["config", "remote.project.tagOpt", "--tags"]);
            let req = request(FinishIntegration::Merge);
            let refs = f.repo.success(&["show-ref"]).stdout;
            let index = fs::read(f.repo.path().join(".git/index")).unwrap();
            let config = fs::read(f.repo.path().join(".git/config")).unwrap();
            let tree = fs::read(f.repo.path().join("work")).unwrap();
            let p = refreshed(&f, &req);
            assert!(p.is_ready(), "{p:#?}");
            assert_eq!(
                p.evidence.unwrap().fetched,
                RefObservation::Commit(remote.clone())
            );
            assert!(
                git::run(f.repo.path(), ["cat-file", "-e", text(&remote)])
                    .unwrap()
                    .status
                    .success()
            );
            assert_eq!(refs, f.repo.success(&["show-ref"]).stdout);
            assert_eq!(index, fs::read(f.repo.path().join(".git/index")).unwrap());
            assert_eq!(config, fs::read(f.repo.path().join(".git/config")).unwrap());
            assert_eq!(tree, fs::read(f.repo.path().join("work")).unwrap());
            assert!(!f.repo.path().join(".git/FETCH_HEAD").exists());
        },
    );
}
#[test]
fn remote_refresh_missing_and_disagreeing_targets_and_cleanup_evidence() {
    isolated(
        "remote_refresh_missing_and_disagreeing_targets_and_cleanup_evidence",
        true,
        |root| {
            let f = fixture(root);
            let second = second_target(root, &f);
            advance_project(root, &f);
            let req = request(FinishIntegration::Merge);
            let p = refreshed(&f, &req);
            assert!(
                p.blockers
                    .iter()
                    .any(|b| matches!(b, Blocker::TargetChanged(_)))
            );
            assert!(p.permitted_effects().is_empty());
            git::run(second.path(), ["update-ref", "-d", "refs/heads/main"]).unwrap();
            let p = refreshed(&f, &req);
            assert!(p.permitted_effects().is_empty());
            let f = fixture(root);
            let mut req = request(FinishIntegration::FfOnly);
            req.cleanup = Some(FinishCleanup::Both);
            req.checkout = Some(FinishCheckout::Target);
            f.repo.success(&["push", "fork", "A"]);
            let p = refreshed(&f, &req);
            assert_eq!(p.evidence.as_ref().unwrap().work.len(), 1);
            assert_eq!(p.evidence.as_ref().unwrap().work[0].branch, p.source);
            assert!(p.is_ready());
            let p = remote_candidate(&f, &req);
            assert!(target_publication::publish(f.repo.path(), &p).is_complete());
            assert_eq!(observed(f.fork.path(), b"refs/heads/A"), p.source);
            assert_eq!(observed(f.repo.path(), b"refs/heads/A"), p.source);
        },
    );
}
#[test]
fn target_publication_checks_fail_mutate_cancel_and_skip() {
    isolated(
        "target_publication_checks_fail_mutate_cancel_and_skip",
        true,
        |root| {
            for script in [
                "exit 9",
                "echo dirty > work",
                "git config oneflow.changed true",
                "kill -TERM $PPID; sleep 10",
            ] {
                let f = fixture(root);
                configure_checks(&f, &[script]);
                let p = remote_candidate(&f, &request(FinishIntegration::FfOnly));
                let old = observed(f.project.path(), b"refs/heads/main");
                let r = target_publication::publish(f.repo.path(), &p);
                assert!(
                    matches!(r.error, Some(target_publication::Error::ChecksFailed)),
                    "{script}: {r:#?}"
                );
                assert_eq!(pushes(&r), 0);
                assert_eq!(old, observed(f.project.path(), b"refs/heads/main"));
                assert!(p.recovery_path.unwrap().exists());
            }
            let f = fixture(root);
            configure_checks(&f, &["exit 9"]);
            let p = remote_candidate(&f, &request(FinishIntegration::FfOnly));
            let r = target_publication::publish_with_checks(f.repo.path(), &p, true, |_| {});
            assert!(r.is_complete(), "{r:#?}");
        },
    );
}
#[test]
fn target_publication_partial_rejection_and_retry_do_not_repeat_results() {
    isolated(
        "target_publication_partial_rejection_and_retry_do_not_repeat_results",
        true,
        |root| {
            let f = fixture(root);
            let second = second_target(root, &f);
            // A server rejection is independent of the other target endpoint.
            use std::os::unix::fs::PermissionsExt;
            let reject = second.path().join("hooks/pre-receive");
            fs::write(&reject, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(&reject, fs::Permissions::from_mode(0o700)).unwrap();
            // Bare fixtures inherit the isolated hooksPath; set an explicit server path.
            git::run(
                second.path(),
                [
                    "config",
                    "core.hooksPath",
                    second.path().join("hooks").to_str().unwrap(),
                ],
            )
            .unwrap();
            let req = request(FinishIntegration::Squash);
            let p = remote_candidate(&f, &req);
            let result = p.result.clone();
            let r = target_publication::publish(f.repo.path(), &p);
            assert_eq!(
                r.state,
                target_publication::PublicationState::Partial,
                "{r:#?}"
            );
            assert_eq!(
                r.destinations[1].state,
                target_publication::DestinationState::Rejected
            );
            fs::remove_file(reject).unwrap();
            let p = refreshed(&f, &req);
            assert_eq!(p.result, result);
            let r = target_publication::publish(f.repo.path(), &p);
            assert!(r.is_complete(), "{r:#?}");
            assert_eq!(pushes(&r), 1);
            assert_eq!(
                r.destinations[0].state,
                target_publication::DestinationState::Unchanged
            );
        },
    );
}
#[test]
fn target_publication_external_delivery_needs_no_checks_or_completion() {
    isolated(
        "target_publication_external_delivery_needs_no_checks_or_completion",
        true,
        |root| {
            let f = fixture(root);
            configure_checks(&f, &["exit 1"]);
            let mut req = request(FinishIntegration::Merge);
            req.checkout = Some(FinishCheckout::Work);
            let p = remote_candidate(&f, &req);
            f.repo.success(&["push", "project", "main"]);
            let r = target_publication::publish(f.repo.path(), &p);
            assert!(r.is_complete(), "{r:#?}");
            assert_eq!(pushes(&r), 0);
            assert_eq!(r.checks.completed, 0);
            assert_eq!(r.plan.unwrap().state, State::Published);
            assert_eq!(
                f.repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                b"main\n"
            );
        },
    );
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
// Wrap only a child test's Git executable; concurrent tests keep their own PATH.
fn target_child(f: &Fixture, operation: &str, command: &str, body: &str) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;
    let wrapper = tempfile::tempdir().unwrap();
    let executable = wrapper.path().join("git");
    fs::write(&executable, format!("#!/bin/sh\nfor arg do\n if [ \"$arg\" = {command} ]; then\n{body}\n fi\ndone\nexec \"$ONEFLOW_REAL_GIT\" \"$@\"\n")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::env::var_os("PATH").unwrap();
    let real = std::env::split_paths(&path)
        .map(|p| p.join("git"))
        .find(|p| p.is_file())
        .unwrap();
    let paths = std::env::join_paths(
        std::iter::once(wrapper.path().to_path_buf()).chain(std::env::split_paths(&path)),
    )
    .unwrap();
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "target_transport_child", "--nocapture"])
        .env("ONEFLOW_TARGET_DIR", f.repo.path())
        .env("ONEFLOW_TARGET_OPERATION", operation)
        .env("ONEFLOW_REAL_GIT", real)
        .env("PATH", paths)
        .output()
        .unwrap()
}
fn target_child_result(
    f: &Fixture,
    operation: &str,
    command: &str,
    body: &str,
) -> serde_json::Value {
    let output = target_child(f, operation, command, body);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&fs::read(f.repo.path().join(".git/target-result.json")).unwrap())
        .unwrap()
}
#[test]
fn target_transport_child() {
    let Some(directory) = std::env::var_os("ONEFLOW_TARGET_DIR") else {
        return;
    };
    let directory = Path::new(&directory);
    let operation = std::env::var("ONEFLOW_TARGET_OPERATION").unwrap();
    let req = if operation.starts_with("complete") {
        let mut req = completion_request(
            FinishIntegration::Squash,
            if operation == "complete-work" {
                FinishCleanup::Keep
            } else if operation.starts_with("complete-local") {
                FinishCleanup::Local
            } else {
                FinishCleanup::Both
            },
            if operation == "complete-work" {
                FinishCheckout::Work
            } else {
                FinishCheckout::Target
            },
        );
        if operation.ends_with("-merge") {
            req.update = Some(FinishUpdate::Merge);
        }
        if operation.ends_with("-rebase") {
            req.update = Some(FinishUpdate::Rebase);
        }
        req
    } else {
        request(FinishIntegration::Squash)
    };
    let initial = integration::assess(directory, &req, None).unwrap();
    let refreshed = target_remote::refresh(directory, &initial);
    let data = if std::env::var("ONEFLOW_TARGET_OPERATION").unwrap() == "refresh" {
        serde_json::json!({
            "error": format!("{:?}", refreshed.error),
            "evidence": refreshed.evidence.is_some(),
            "ready": refreshed.plan.as_ref().is_some_and(Plan::is_ready),
            "effects": refreshed.effects.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>(),
        })
    } else if operation.starts_with("complete") {
        assert!(refreshed.error.is_none(), "{refreshed:#?}");
        fs::write(directory.join(".git/completion-boundary"), "ready").unwrap();
        let r = completion::complete(directory, &refreshed.plan.unwrap());
        serde_json::json!({
            "complete": r.is_complete(), "error": format!("{:?}", r.error),
            "plan_ready": r.plan.as_ref().is_ok_and(Plan::is_ready),
            "plan_state": r.plan.as_ref().ok().map(|p| format!("{:?}", p.state)),
            "effects": r.effects.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>(),
            "pushes": completion_pushes(&r),
            "destinations": r.destinations.iter().map(|d| serde_json::json!({ "state": format!("{:?}",d.state), "error": format!("{:?}",d.error) })).collect::<Vec<_>>()
        })
    } else {
        assert!(refreshed.error.is_none(), "{refreshed:#?}");
        let p = refreshed.plan.unwrap();
        let r = target_publication::publish(directory, &p);
        serde_json::json!({
            "state": format!("{:?}", r.state), "complete": r.is_complete(),
            "error": format!("{:?}", r.error), "result": r.commit.as_ref().map(|c| text(c)),
            "checks": r.checks.completed,
            "destinations": r.destinations.iter().map(|d| serde_json::json!({
                "state": format!("{:?}", d.state), "detail": String::from_utf8_lossy(&d.detail),
                "error": format!("{:?}", d.error),
                "pushes": d.commands.iter().filter(|c| c.arguments.iter().any(|a| a == "push")).count(),
                "arguments": d.commands.last().map(|c| c.arguments.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>()),
            })).collect::<Vec<_>>(),
        })
    };
    fs::write(
        directory.join(".git/target-result.json"),
        serde_json::to_vec(&data).unwrap(),
    )
    .unwrap();
}
#[test]
fn remote_refresh_refuses_unavailable_objects_and_detects_observation_fetch_races() {
    isolated(
        "remote_refresh_refuses_unavailable_objects_and_detects_observation_fetch_races",
        true,
        |root| {
            for body in ["exit 1", "exit 0"] {
                let f = fixture(root);
                advance_project(root, &f);
                let refs = f.repo.success(&["show-ref"]).stdout;
                let r = target_child_result(&f, "refresh", "fetch", body);
                assert_eq!(r["evidence"], false, "{r}");
                assert_eq!(r["ready"], false);
                assert!(
                    r["error"].as_str().unwrap().contains(if body == "exit 0" {
                        "ObjectUnavailable"
                    } else {
                        "CommandFailed"
                    }),
                    "{r}"
                );
                assert_eq!(refs, f.repo.success(&["show-ref"]).stdout);
            }
            let f = fixture(root);
            let old = oid(&f.repo, "main");
            let moved = advance_project(root, &f);
            assert!(
                git::run(
                    f.project.path(),
                    ["update-ref", "refs/heads/main", text(&old)]
                )
                .unwrap()
                .status
                .success()
            );
            let body = format!(
                "\"$ONEFLOW_REAL_GIT\" -C {} update-ref refs/heads/main {} || exit 91\nexec \"$ONEFLOW_REAL_GIT\" \"$@\"",
                shell_quote(f.project.path().to_str().unwrap()),
                text(&moved)
            );
            let r = target_child_result(&f, "refresh", "fetch", &body);
            assert_eq!(r["error"], "None", "{r}");
            assert_eq!(r["evidence"], true);
            assert_eq!(
                r["ready"], false,
                "fetch must not substitute the moved target"
            );
            assert_eq!(oid(&f.repo, "main"), old);
        },
    );
}
#[test]
fn target_publication_lost_and_nonzero_acknowledgements_retry_in_another_process() {
    isolated(
        "target_publication_lost_and_nonzero_acknowledgements_retry_in_another_process",
        true,
        |root| {
            for (body, delivered, acknowledged) in [
                (
                    "\"$ONEFLOW_REAL_GIT\" \"$@\" >/dev/null\nexit 0",
                    true,
                    false,
                ),
                (
                    "\"$ONEFLOW_REAL_GIT\" \"$@\" >/dev/null\nexit 1",
                    true,
                    false,
                ),
                ("\"$ONEFLOW_REAL_GIT\" \"$@\"\nexit 1", true, true),
                ("exit 1", false, false),
            ] {
                let f = fixture(root);
                configure_checks(&f, &["echo check >> .git/publication-checks"]);
                let req = request(FinishIntegration::Squash);
                let p = remote_candidate(&f, &req);
                let log = fs::read(f.repo.path().join(".git/logs/refs/heads/main")).unwrap();
                let r = target_child_result(&f, "publish", "push", body);
                assert_eq!(
                    r["state"],
                    if acknowledged {
                        "Complete"
                    } else {
                        "Unconfirmed"
                    },
                    "{r}"
                );
                assert_eq!(r["complete"], false);
                assert_eq!(
                    observed(f.project.path(), b"refs/heads/main")
                        == RefObservation::Commit(p.result.clone().unwrap()),
                    delivered
                );
                let retry =
                    target_child_result(&f, "publish", "push", "exec \"$ONEFLOW_REAL_GIT\" \"$@\"");
                assert_eq!(retry["complete"], true, "{retry}");
                assert_eq!(
                    retry["destinations"][0]["pushes"],
                    if delivered { 0 } else { 1 }
                );
                assert_eq!(retry["checks"], if delivered { 0 } else { 1 });
                assert_eq!(
                    fs::read(f.repo.path().join(".git/logs/refs/heads/main")).unwrap(),
                    log
                );
                assert_eq!(retry["result"], text(p.result.as_ref().unwrap()));
                assert!(p.recovery_path.unwrap().exists());
            }
        },
    );
}
#[test]
fn target_publication_killed_between_destinations_reuses_retained_result() {
    isolated(
        "target_publication_killed_between_destinations_reuses_retained_result",
        true,
        |root| {
            let f = fixture(root);
            let second = second_target(root, &f);
            let p = remote_candidate(&f, &request(FinishIntegration::Squash));
            let before = refs_without_project_main_cache(&f);
            let output = target_child(
                &f,
                "publish",
                "push",
                "\"$ONEFLOW_REAL_GIT\" \"$@\" >/dev/null\nkill -KILL \"$PPID\"\nexit 0",
            );
            assert!(!output.status.success());
            assert_eq!(
                observed(f.project.path(), b"refs/heads/main"),
                RefObservation::Commit(p.result.clone().unwrap())
            );
            assert_ne!(
                observed(second.path(), b"refs/heads/main"),
                RefObservation::Commit(p.result.clone().unwrap())
            );
            let r = target_child_result(&f, "publish", "push", "exec \"$ONEFLOW_REAL_GIT\" \"$@\"");
            assert_eq!(r["complete"], true, "{r}");
            assert_eq!(r["destinations"][0]["pushes"], 0);
            assert_eq!(r["destinations"][1]["pushes"], 1);
            assert_eq!(before, refs_without_project_main_cache(&f));
            assert_eq!(
                observed(f.repo.path(), b"refs/remotes/project/main"),
                RefObservation::Commit(p.result.clone().unwrap())
            );
        },
    );
}
#[test]
fn target_publication_exact_lease_rejects_remote_race_even_after_tracking_refresh() {
    isolated(
        "target_publication_exact_lease_rejects_remote_race_even_after_tracking_refresh",
        true,
        |root| {
            let f = fixture(root);
            let old = oid(&f.repo, "main");
            let moved = advance_project(root, &f);
            assert!(
                git::run(
                    f.project.path(),
                    ["update-ref", "refs/heads/main", text(&old)]
                )
                .unwrap()
                .status
                .success()
            );
            let p = remote_candidate(&f, &request(FinishIntegration::Squash));
            let body = format!(
                "\"$ONEFLOW_REAL_GIT\" -C {} update-ref refs/heads/main {} || exit 91\n\"$ONEFLOW_REAL_GIT\" fetch project >&2 || exit 92\nexec \"$ONEFLOW_REAL_GIT\" \"$@\"",
                shell_quote(f.project.path().to_str().unwrap()),
                text(&moved)
            );
            let r = target_child_result(&f, "publish", "push", &body);
            assert_eq!(r["state"], "Rejected", "{r}");
            assert_eq!(r["destinations"][0]["pushes"], 1);
            assert!(
                r["destinations"][0]["detail"]
                    .as_str()
                    .unwrap()
                    .contains("stale info")
            );
            assert!(
                r["destinations"][0]["arguments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| a == &format!("--force-with-lease=refs/heads/main:{}", text(&old)))
            );
            assert_eq!(
                observed(f.project.path(), b"refs/heads/main"),
                RefObservation::Commit(moved)
            );
            assert_eq!(oid(&f.repo, "main"), p.result.unwrap());
        },
    );
}
#[test]
fn target_publication_stops_between_effects_when_hook_invalidates_context() {
    isolated(
        "target_publication_stops_between_effects_when_hook_invalidates_context",
        true,
        |root| {
            for script in [
                "git commit --allow-empty -m hook-change",
                "rm .git/oneflow-integration/branches/A/attempt.json",
            ] {
                let f = fixture(root);
                let second = second_target(root, &f);
                hook(&f, "pre-push", script);
                let p = remote_candidate(&f, &request(FinishIntegration::Squash));
                let old = observed(second.path(), b"refs/heads/main");
                let r = target_publication::publish(f.repo.path(), &p);
                assert_eq!(
                    r.state,
                    target_publication::PublicationState::Partial,
                    "{r:#?}"
                );
                assert!(r.error.is_some());
                assert_eq!(pushes(&r), 1);
                assert_eq!(
                    observed(f.project.path(), b"refs/heads/main"),
                    RefObservation::Commit(p.result.unwrap())
                );
                assert_eq!(observed(second.path(), b"refs/heads/main"), old);
            }
        },
    );
}
#[test]
fn target_publication_invalid_retention_and_independent_branch_cannot_borrow_attempt() {
    isolated(
        "target_publication_invalid_retention_and_independent_branch_cannot_borrow_attempt",
        true,
        |root| {
            let f = fixture(root);
            let req = request(FinishIntegration::Squash);
            let p = remote_candidate(&f, &req);
            let old = observed(f.project.path(), b"refs/heads/main");
            let record = fs::read(p.recovery_path.as_ref().unwrap()).unwrap();
            f.repo.success(&["switch", "-c", "B", "A"]);
            commit(&f.repo, "independent", "work B");
            let b = refreshed(&f, &req);
            assert_eq!(b.source_ref, b"refs/heads/B");
            assert_eq!(b.state, State::Fresh);
            let r = target_publication::publish(f.repo.path(), &p);
            assert!(r.error.is_some());
            assert_eq!(pushes(&r), 0);
            assert_eq!(fs::read(p.recovery_path.as_ref().unwrap()).unwrap(), record);
            assert_eq!(observed(f.project.path(), b"refs/heads/main"), old);

            let f = fixture(root);
            let p = remote_candidate(&f, &req);
            let old = observed(f.project.path(), b"refs/heads/main");
            let record: Record =
                serde_json::from_slice(&fs::read(p.recovery_path.as_ref().unwrap()).unwrap())
                    .unwrap();
            f.repo.success(&["update-ref", "-d", &record.result_ref()]);
            let r = target_publication::publish(f.repo.path(), &p);
            assert!(r.error.is_some());
            assert_eq!(pushes(&r), 0);
            assert_eq!(observed(f.project.path(), b"refs/heads/main"), old);
        },
    );
}

#[test]
fn remote_refresh_rejects_ambiguous_advertisements_and_changed_endpoint_resolution() {
    isolated(
        "remote_refresh_rejects_ambiguous_advertisements_and_changed_endpoint_resolution",
        true,
        |root| {
            let f = fixture(root);
            let old = oid(&f.repo, "main");
            for output in [
                "not-an-oid\trefs/heads/main\n".to_owned(),
                format!(
                    "{}\trefs/heads/main\n{}\trefs/heads/main\n",
                    text(&old),
                    text(&old)
                ),
            ] {
                let body = format!(
                    "for option do\n [ \"$option\" != --get-url ] || exec \"$ONEFLOW_REAL_GIT\" \"$@\"\ndone\nprintf '%s' {}\nexit 0",
                    shell_quote(&output)
                );
                let r = target_child_result(&f, "refresh", "ls-remote", &body);
                assert_eq!(r["evidence"], false, "{r}");
                assert_ne!(r["error"], "None");
            }
            let r = target_child_result(
                &f,
                "refresh",
                "ls-remote",
                "for option do\n if [ \"$option\" = --get-url ]; then printf '%s\\n' wrong-endpoint; exit 0; fi\ndone\nexec \"$ONEFLOW_REAL_GIT\" \"$@\"",
            );
            assert_eq!(r["evidence"], false);
            assert_eq!(r["error"], "Some(EndpointChanged)");
            assert_eq!(r["effects"].as_array().unwrap().len(), 0);
        },
    );
}
#[test]
fn target_publication_contains_refspec_mirror_tags_and_tracking_configuration() {
    isolated(
        "target_publication_contains_refspec_mirror_tags_and_tracking_configuration",
        true,
        |root| {
            let f = fixture(root);
            f.repo
                .success(&["tag", "-a", "candidate-tag", "-m", "candidate tag", "main"]);
            f.repo.success(&["branch", "unrelated", "A"]);
            f.repo.success(&["config", "remote.project.mirror", "true"]);
            f.repo
                .success(&["config", "remote.project.push", "refs/heads/*:refs/heads/*"]);
            f.repo.success(&["config", "push.followTags", "true"]);
            f.repo.success(&["config", "push.autoSetupRemote", "true"]);
            let p = remote_candidate(&f, &request(FinishIntegration::Squash));
            let refs = refs_without_project_main_cache(&f);
            let config = fs::read(f.repo.path().join(".git/config")).unwrap();
            let r = target_publication::publish(f.repo.path(), &p);
            assert!(r.is_complete(), "{r:#?}");
            let remote_refs = git::run(f.project.path(), ["show-ref"]).unwrap();
            assert!(remote_refs.status.success());
            assert_eq!(
                remote_refs.stdout,
                format!("{} refs/heads/main\n", text(p.result.as_ref().unwrap())).as_bytes()
            );
            assert_eq!(refs, refs_without_project_main_cache(&f));
            assert_eq!(
                observed(f.repo.path(), b"refs/remotes/project/main"),
                RefObservation::Commit(p.result.clone().unwrap())
            );
            assert_eq!(config, fs::read(f.repo.path().join(".git/config")).unwrap());
        },
    );
}
#[test]
fn target_publication_rejects_forged_non_descendant_even_when_work_rewrite_is_allowed() {
    isolated(
        "target_publication_rejects_forged_non_descendant_even_when_work_rewrite_is_allowed",
        true,
        |root| {
            let f = fixture(root);
            use std::io::Write;
            writeln!(
                fs::OpenOptions::new()
                    .append(true)
                    .open(f.repo.path().join(".git/git-oneflow.config.toml"))
                    .unwrap(),
                "finish-rewrite='allow'"
            )
            .unwrap();
            let mut p = remote_candidate(&f, &request(FinishIntegration::Squash));
            let old = observed(f.project.path(), b"refs/heads/main");
            let tree = oid(&f.repo, "HEAD^{tree}");
            let forged = f
                .repo
                .success(&["commit-tree", text(&tree), "-m", "unrelated root"]);
            let forged = forged.stdout.strip_suffix(b"\n").unwrap();
            assert!(
                !git::run(
                    f.repo.path(),
                    [
                        "merge-base",
                        "--is-ancestor",
                        text(&commit_of(&old)),
                        text(forged)
                    ]
                )
                .unwrap()
                .status
                .success()
            );
            p.result = Some(forged.to_vec());
            let r = target_publication::publish(f.repo.path(), &p);
            assert!(r.error.is_some(), "{r:#?}");
            assert_eq!(pushes(&r), 0);
            assert_eq!(observed(f.project.path(), b"refs/heads/main"), old);
        },
    );
}

use integration::completion;
fn completion_request(
    method: FinishIntegration,
    cleanup: FinishCleanup,
    checkout: FinishCheckout,
) -> Request {
    Request {
        cleanup: Some(cleanup),
        checkout: Some(checkout),
        ..request(method)
    }
}
fn delivered(f: &Fixture, req: &Request) -> Plan {
    let p = remote_candidate(f, req);
    let r = target_publication::publish(f.repo.path(), &p);
    assert!(r.is_complete(), "{r:#?}");
    r.plan.unwrap()
}
fn completion_pushes(r: &completion::Report) -> usize {
    r.destinations
        .iter()
        .flat_map(|d| &d.commands)
        .filter(|c| c.arguments.iter().any(|a| a == "push"))
        .count()
}

#[test]
fn completion_updates_only_eligible_target_and_deleted_work_caches() {
    isolated(
        "completion_updates_only_eligible_target_and_deleted_work_caches",
        true,
        |root| {
            for case in ["ordinary", "custom", "newer", "symbolic", "locked"] {
                let f = fixture(root);
                let work_cache = if case == "custom" {
                    "refs/remotes/cache/work-A"
                } else {
                    "refs/remotes/fork/A"
                };
                let target_cache = if case == "custom" {
                    "refs/remotes/cache/project-main"
                } else {
                    "refs/remotes/project/main"
                };
                if case == "custom" {
                    f.repo.success(&[
                        "config",
                        "remote.fork.fetch",
                        "+refs/heads/*:refs/remotes/cache/work-*",
                    ]);
                    f.repo.success(&[
                        "config",
                        "remote.project.fetch",
                        "+refs/heads/main:refs/remotes/cache/project-main",
                    ]);
                }
                f.repo.success(&["push", "fork", "A"]);
                let req = completion_request(
                    FinishIntegration::Squash,
                    FinishCleanup::Both,
                    FinishCheckout::Target,
                );
                let p = delivered(&f, &req);
                let result = p.result.clone().unwrap();
                assert_eq!(
                    observed(f.repo.path(), target_cache.as_bytes()),
                    RefObservation::Commit(result.clone())
                );
                match case {
                    "newer" => {
                        f.repo.success(&["update-ref", work_cache, text(&result)]);
                    }
                    "symbolic" => {
                        f.repo
                            .success(&["symbolic-ref", work_cache, "refs/heads/main"]);
                    }
                    "locked" => {
                        fs::write(
                            f.repo
                                .path()
                                .join(".git")
                                .join(format!("{work_cache}.lock")),
                            "held",
                        )
                        .unwrap();
                    }
                    _ => (),
                }
                let completed = completion::complete(f.repo.path(), &p);
                assert!(completed.is_complete(), "{case}: {completed:#?}");
                assert_eq!(
                    observed(f.project.path(), b"refs/heads/main"),
                    RefObservation::Commit(result.clone())
                );
                assert_eq!(
                    observed(f.fork.path(), b"refs/heads/A"),
                    RefObservation::Absent
                );
                assert_eq!(
                    observed(f.repo.path(), b"refs/heads/A"),
                    RefObservation::Absent
                );
                assert_eq!(oid(&f.repo, "main"), result.clone());
                assert_eq!(
                    completed.caches.iter().any(|c| c.has_issue()),
                    matches!(case, "newer" | "symbolic" | "locked"),
                    "{case}: {completed:#?}"
                );
                if matches!(case, "newer" | "symbolic") {
                    assert_eq!(
                        observed(f.repo.path(), work_cache.as_bytes()),
                        RefObservation::Commit(result)
                    );
                } else if case == "locked" {
                    assert_eq!(observed(f.repo.path(), work_cache.as_bytes()), p.source);
                    fs::remove_file(
                        f.repo
                            .path()
                            .join(".git")
                            .join(format!("{work_cache}.lock")),
                    )
                    .unwrap();
                    let retry = completion::complete(f.repo.path(), &refreshed(&f, &req));
                    assert!(retry.is_complete(), "{retry:#?}");
                    assert_eq!(completion_pushes(&retry), 0);
                    assert_eq!(
                        observed(f.repo.path(), work_cache.as_bytes()),
                        RefObservation::Absent
                    );
                } else {
                    assert_eq!(
                        observed(f.repo.path(), work_cache.as_bytes()),
                        RefObservation::Absent
                    );
                }
            }
        },
    );
}
#[test]
fn completion_methods_policies_and_observational_retry() {
    isolated(
        "completion_methods_policies_and_observational_retry",
        true,
        |root| {
            for method in [
                FinishIntegration::FfOnly,
                FinishIntegration::Merge,
                FinishIntegration::Squash,
            ] {
                for (cleanup, checkout) in [
                    (FinishCleanup::Keep, FinishCheckout::Target),
                    (FinishCleanup::Keep, FinishCheckout::Work),
                    (FinishCleanup::Local, FinishCheckout::Target),
                    (FinishCleanup::Both, FinishCheckout::Target),
                ] {
                    let f = fixture(root);
                    f.repo
                        .success(&["config", "branch.A.description", "retained metadata"]);
                    f.repo.success(&["push", "fork", "A"]);
                    let req = completion_request(method, cleanup, checkout);
                    let p = delivered(&f, &req);
                    let pins = f.repo.success(&["for-each-ref", "refs/oneflow/"]).stdout;
                    let result = p.result.clone().unwrap();
                    let r = completion::complete(f.repo.path(), &p);
                    assert!(
                        r.is_complete(),
                        "method={method:?} cleanup={cleanup:?} checkout={checkout:?} {r:#?}"
                    );
                    assert_eq!(
                        f.repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                        if checkout == FinishCheckout::Work {
                            b"A\n".as_slice()
                        } else {
                            b"main\n".as_slice()
                        }
                        .to_vec()
                    );
                    assert_eq!(
                        observed(f.repo.path(), b"refs/heads/A"),
                        if cleanup == FinishCleanup::Keep {
                            p.source.clone()
                        } else {
                            RefObservation::Absent
                        }
                    );
                    assert_eq!(
                        observed(f.fork.path(), b"refs/heads/A"),
                        if cleanup == FinishCleanup::Both {
                            RefObservation::Absent
                        } else {
                            p.source.clone()
                        }
                    );
                    assert_eq!(
                        observed(f.project.path(), b"refs/heads/main"),
                        RefObservation::Commit(result)
                    );
                    assert_eq!(
                        pins,
                        f.repo.success(&["for-each-ref", "refs/oneflow/"]).stdout
                    );
                    assert_eq!(
                        f.repo.success(&["config", "branch.A.description"]).stdout,
                        b"retained metadata\n"
                    );
                    let retry = completion::complete(f.repo.path(), r.plan.as_ref().unwrap());
                    assert!(retry.is_complete(), "{retry:#?}");
                    assert!(retry.effects.is_empty());
                    assert_eq!(completion_pushes(&retry), 0);
                }
            }
        },
    );
}

#[test]
fn completion_recovers_prepared_source_after_deletion_and_rejects_recreation() {
    isolated(
        "completion_recovers_prepared_source_after_deletion_and_rejects_recreation",
        true,
        |root| {
            for update in [FinishUpdate::Merge, FinishUpdate::Rebase] {
                let f = fixture(root);
                diverge(&f, false);
                let mut req = completion_request(
                    FinishIntegration::Squash,
                    FinishCleanup::Local,
                    FinishCheckout::Target,
                );
                req.update = Some(update);
                let p = delivered(&f, &req);
                let source = commit_of(&p.source);
                let r = completion::complete(f.repo.path(), &p);
                assert!(r.is_complete(), "{update:?}: {r:#?}");
                let operation = if update == FinishUpdate::Merge {
                    "complete-local-merge"
                } else {
                    "complete-local-rebase"
                };
                let data =
                    target_child_result(&f, operation, "push", "exec \"$ONEFLOW_REAL_GIT\" \"$@\"");
                assert_eq!(data["complete"], true, "{data}");
                assert!(data["effects"].as_array().unwrap().is_empty());
                f.repo.success(&["branch", "A", text(&source)]);
                let retry = refreshed(&f, &req);
                assert!(
                    !retry.is_ready(),
                    "recreated source acquired cleanup authority: {retry:#?}"
                );
                assert!(!completion::complete(f.repo.path(), &retry).is_complete());
                assert_eq!(oid(&f.repo, "A"), source);
            }
        },
    );
}
#[test]
fn completion_rejects_stale_delivery_and_context_without_effects() {
    isolated(
        "completion_rejects_stale_delivery_and_context_without_effects",
        true,
        |root| {
            for change in [
                "pending",
                "partial",
                "changed-target",
                "dirty",
                "operation",
                "config",
                "symbolic",
                "source",
                "occupied",
                "record",
                "independent",
            ] {
                let f = fixture(root);
                f.repo.success(&["push", "fork", "A"]);
                let req = completion_request(
                    FinishIntegration::Squash,
                    FinishCleanup::Both,
                    FinishCheckout::Target,
                );
                let second = if change == "partial" {
                    Some(second_target(root, &f))
                } else {
                    None
                };
                let p = if change == "pending" {
                    remote_candidate(&f, &req)
                } else {
                    delivered(&f, &req)
                };
                match change {
                    "partial" => {
                        git::run(
                            second.as_ref().unwrap().path(),
                            [
                                "update-ref",
                                "refs/heads/main",
                                text(&oid(&f.repo, "main^")),
                            ],
                        )
                        .unwrap();
                    }
                    "changed-target" => {
                        advance_project(root, &f);
                    }
                    "dirty" => fs::write(f.repo.path().join("work"), "dirty").unwrap(),
                    "operation" => {
                        fs::write(f.repo.path().join(".git/MERGE_HEAD"), oid(&f.repo, "A")).unwrap()
                    }
                    "config" => {
                        f.repo.success(&["config", "oneflow.unrelated", "changed"]);
                    }
                    "symbolic" => {
                        f.repo
                            .success(&["symbolic-ref", "refs/heads/A", "refs/heads/main"]);
                    }
                    "source" => {
                        f.repo.success(&[
                            "update-ref",
                            "refs/heads/A",
                            text(&oid(&f.repo, "main")),
                        ]);
                    }
                    "occupied" => {
                        f.repo.success(&[
                            "worktree",
                            "add",
                            root.join("occupied").to_str().unwrap(),
                            "A",
                        ]);
                    }
                    "record" => fs::write(p.recovery_path.as_ref().unwrap(), "invalid").unwrap(),
                    "independent" => {
                        f.repo.success(&["switch", "-c", "B"]);
                    }
                    _ => {}
                }
                let before = f.repo.success(&["show-ref"]).stdout;
                let r = completion::complete(f.repo.path(), &p);
                assert!(!r.is_complete(), "{change}: {r:#?}");
                if matches!(change, "partial" | "changed-target") {
                    let remaining = r.plan.as_ref().unwrap();
                    assert!(
                        !remaining.is_ready()
                            || !matches!(remaining.state, State::Published | State::Complete),
                        "{change}: stale delivery report"
                    );
                }
                assert!(r.effects.is_empty(), "{change}: {:?}", r.effects);
                assert_eq!(before, f.repo.success(&["show-ref"]).stdout, "{change}");
                assert_eq!(completion_pushes(&r), 0);
            }
        },
    );
}
#[test]
fn completion_checkout_hook_failure_and_ignored_collision_preserve_work() {
    isolated(
        "completion_checkout_hook_failure_and_ignored_collision_preserve_work",
        true,
        |root| {
            for mode in ["failure", "dirty-hook", "ignored"] {
                let f = fixture(root);
                // Squash and source have different trees to exercise checkout collisions.
                let base = oid(&f.repo, "main");
                f.repo.success(&["switch", "main"]);
                commit(&f.repo, "target-only", "target");
                f.repo.success(&["rm", "base"]);
                f.repo.success(&["commit", "-m", "remove base"]);
                publish(&f);
                f.repo.success(&["switch", "A"]);
                let req = completion_request(
                    FinishIntegration::Squash,
                    FinishCleanup::Keep,
                    FinishCheckout::Work,
                );
                let p = delivered(&f, &req);
                assert_ne!(base, oid(&f.repo, "main"));
                let h = match mode {
                    "failure" => Some(hook(&f, "post-checkout", "exit 1")),
                    "dirty-hook" => Some(hook(&f, "post-checkout", "echo changed >> work")),
                    _ => {
                        fs::create_dir_all(f.repo.path().join(".git/info")).unwrap();
                        fs::write(f.repo.path().join(".git/info/exclude"), "base\n").unwrap();
                        fs::write(f.repo.path().join("base"), "keep me").unwrap();
                        None
                    }
                };
                let r = completion::complete(f.repo.path(), &p);
                if mode == "ignored" {
                    assert!(!r.is_complete(), "{r:#?}");
                    assert!(r.effects.is_empty());
                    assert_eq!(
                        fs::read_to_string(f.repo.path().join("base")).unwrap(),
                        "keep me"
                    );
                    assert_eq!(
                        f.repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                        b"main\n"
                    );
                } else {
                    assert!(!r.is_complete(), "{r:#?}");
                    assert!(
                        r.effects
                            .iter()
                            .any(|e| matches!(e, completion::Effect::CheckedOut { .. }))
                    );
                    fs::remove_file(h.unwrap()).unwrap();
                    if mode == "dirty-hook" {
                        f.repo.success(&["restore", "work"]);
                    }
                    assert!(
                        completion::complete(f.repo.path(), &refreshed(&f, &req)).is_complete()
                    );
                }
            }
        },
    );
}

fn second_work(root: &Path, f: &Fixture) -> TempDir {
    let second = tempfile::tempdir_in(root).unwrap();
    assert!(
        git::run(
            root,
            [
                "clone".as_ref(),
                "--bare".as_ref(),
                f.fork.path().as_os_str(),
                second.path().as_os_str()
            ]
        )
        .unwrap()
        .status
        .success()
    );
    for path in [f.fork.path(), second.path()] {
        f.repo.success(&[
            "config",
            "--add",
            "remote.fork.pushurl",
            path.to_str().unwrap(),
        ]);
    }
    second
}
#[test]
fn completion_process_interruptions_and_lost_acknowledgements_resume() {
    isolated(
        "completion_process_interruptions_and_lost_acknowledgements_resume",
        true,
        |root| {
            for point in ["checkout", "local", "remote", "lost-remote", "lost-local"] {
                let f = fixture(root);
                let second = second_work(root, &f);
                f.repo.success(&["push", "fork", "A"]);
                let checkout = point == "checkout";
                let req = completion_request(
                    FinishIntegration::Squash,
                    if checkout {
                        FinishCleanup::Keep
                    } else {
                        FinishCleanup::Both
                    },
                    if checkout {
                        FinishCheckout::Work
                    } else {
                        FinishCheckout::Target
                    },
                );
                delivered(&f, &req);
                let operation = if checkout {
                    "complete-work"
                } else {
                    "complete"
                };
                let command = if checkout {
                    "switch"
                } else if point.contains("local") {
                    "update-ref"
                } else {
                    "push"
                };
                let body = if point == "lost-local" {
                    "\"$ONEFLOW_REAL_GIT\" \"$@\" | while IFS= read -r line; do case \"$line\" in commit:*) ;; *) printf '%s\\n' \"$line\" ;; esac; done; exit 1"
                } else if point.starts_with("lost") {
                    "\"$ONEFLOW_REAL_GIT\" \"$@\" >/dev/null; exit 1"
                } else {
                    "\"$ONEFLOW_REAL_GIT\" \"$@\"; kill -KILL \"$PPID\"; exit 0"
                };
                let output = target_child(&f, operation, command, body);
                if point.starts_with("lost") {
                    assert!(output.status.success(), "{output:?}");
                    let data: serde_json::Value = serde_json::from_slice(
                        &fs::read(f.repo.path().join(".git/target-result.json")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(data["complete"], false);
                } else {
                    assert!(!output.status.success(), "{point}: {output:?}");
                }
                if !checkout {
                    assert_eq!(
                        observed(f.repo.path(), b"refs/heads/A"),
                        RefObservation::Absent
                    );
                }
                if point == "remote" || point == "lost-remote" {
                    assert_eq!(
                        observed(f.fork.path(), b"refs/heads/A"),
                        RefObservation::Absent
                    );
                    assert!(matches!(
                        observed(second.path(), b"refs/heads/A"),
                        RefObservation::Commit(_)
                    ));
                }
                let data =
                    target_child_result(&f, operation, "push", "exec \"$ONEFLOW_REAL_GIT\" \"$@\"");
                assert_eq!(data["complete"], true, "{point}: {data}");
                assert!(
                    !data["effects"].as_array().unwrap().iter().any(|v| v
                        .as_str()
                        .unwrap()
                        .starts_with("CheckedOut")
                        || v.as_str().unwrap().starts_with("LocalWorkDeleted")),
                    "{point}: {data}"
                );
                assert_eq!(
                    data["pushes"],
                    if checkout {
                        0
                    } else if point.contains("remote") {
                        1
                    } else {
                        2
                    }
                );
            }
        },
    );
}
#[test]
fn completion_conditional_local_delete_rejects_races_and_transaction_hooks() {
    isolated(
        "completion_conditional_local_delete_rejects_races_and_transaction_hooks",
        true,
        |root| {
            for race in [
                "advance",
                "symbolic",
                "prepared-change",
                "prepared-reject",
                "verbose",
            ] {
                let f = fixture(root);
                f.repo.success(&["push", "fork", "A"]);
                f.repo.success(&["branch", "alias", "A"]);
                let req = completion_request(
                    FinishIntegration::Squash,
                    FinishCleanup::Both,
                    FinishCheckout::Target,
                );
                delivered(&f, &req);
                let h = match race {
                    "prepared-change" => Some(hook(
                        &f,
                        "reference-transaction",
                        "if [ \"$1\" = prepared ]; then git config oneflow.race changed; fi",
                    )),
                    "prepared-reject" => Some(hook(
                        &f,
                        "reference-transaction",
                        "if [ \"$1\" = prepared ]; then exit 1; fi",
                    )),
                    "verbose" => Some(hook(
                        &f,
                        "reference-transaction",
                        "if [ \"$1\" = prepared ]; then head -c 131072 /dev/zero >&2; fi",
                    )),
                    _ => None,
                };
                let body = match race {
                    "advance" => {
                        "\"$ONEFLOW_REAL_GIT\" update-ref refs/heads/A refs/heads/main; exec \"$ONEFLOW_REAL_GIT\" \"$@\""
                    }
                    "symbolic" => {
                        "\"$ONEFLOW_REAL_GIT\" symbolic-ref refs/heads/A refs/heads/alias; exec \"$ONEFLOW_REAL_GIT\" \"$@\""
                    }
                    _ => "exec \"$ONEFLOW_REAL_GIT\" \"$@\"",
                };
                let data = target_child_result(&f, "complete", "update-ref", body);
                assert_eq!(data["complete"], race == "verbose", "{race}: {data}");
                assert!(!f.repo.path().join(".git/refs/heads/A.lock").exists());
                if race != "verbose" {
                    assert!(matches!(
                        observed(f.repo.path(), b"refs/heads/A"),
                        RefObservation::Commit(_)
                    ));
                    assert_eq!(data["pushes"], 0);
                }
                if let Some(h) = h {
                    fs::remove_file(h).unwrap();
                }
                if race == "prepared-reject" {
                    assert!(
                        completion::complete(f.repo.path(), &refreshed(&f, &req)).is_complete()
                    );
                }
            }
        },
    );
}
#[test]
fn completion_remote_lease_preserves_concurrent_work_and_partial_deletions() {
    isolated(
        "completion_remote_lease_preserves_concurrent_work_and_partial_deletions",
        true,
        |root| {
            for mode in ["race", "reject", "already-changed", "absent"] {
                let f = fixture(root);
                let second = second_work(root, &f);
                f.repo.success(&["push", "fork", "A"]);
                f.repo.success(&["branch", "unrelated", "A"]);
                f.repo.success(&["tag", "keep-tag"]);
                let req = completion_request(
                    FinishIntegration::Squash,
                    FinishCleanup::Both,
                    FinishCheckout::Target,
                );
                let p = delivered(&f, &req);
                f.repo.success(&[
                    "push",
                    second.path().to_str().unwrap(),
                    "main:refs/heads/race",
                ]);
                let newer = oid(&f.repo, "main");
                if mode == "already-changed" {
                    assert!(
                        git::run(second.path(), ["update-ref", "refs/heads/A", text(&newer)])
                            .unwrap()
                            .status
                            .success()
                    );
                }
                if mode == "absent" {
                    assert!(
                        git::run(f.fork.path(), ["update-ref", "-d", "refs/heads/A"])
                            .unwrap()
                            .status
                            .success()
                    );
                }
                let hook_path = second.path().join("hooks/pre-receive");
                if mode == "reject" {
                    use std::os::unix::fs::PermissionsExt;
                    fs::write(&hook_path, "#!/bin/sh\nexit 1\n").unwrap();
                    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o700)).unwrap();
                    assert!(
                        git::run(
                            second.path(),
                            [
                                "config",
                                "core.hooksPath",
                                second.path().join("hooks").to_str().unwrap()
                            ]
                        )
                        .unwrap()
                        .status
                        .success()
                    );
                }
                let prepush = if mode == "race" {
                    Some(hook(
                        &f,
                        "pre-push",
                        &format!(
                            "if [ \"$2\" = {} ]; then git --git-dir={} update-ref refs/heads/A {}; fi",
                            shell_quote(second.path().to_str().unwrap()),
                            shell_quote(second.path().to_str().unwrap()),
                            text(&newer)
                        ),
                    ))
                } else {
                    None
                };
                let r = completion::complete(f.repo.path(), &p);
                assert_eq!(r.is_complete(), mode == "absent", "{mode}: {r:#?}");
                if mode == "race" || mode == "reject" {
                    assert_eq!(
                        r.destinations[0].state,
                        completion::DestinationState::Deleted
                    );
                    assert_eq!(
                        r.destinations[1].state,
                        completion::DestinationState::Rejected
                    );
                    assert_eq!(
                        observed(f.fork.path(), b"refs/heads/A"),
                        RefObservation::Absent
                    );
                    assert!(matches!(
                        observed(second.path(), b"refs/heads/A"),
                        RefObservation::Commit(_)
                    ));
                }
                if mode == "already-changed" {
                    assert!(r.effects.is_empty());
                }
                assert!(matches!(
                    observed(f.repo.path(), b"refs/heads/unrelated"),
                    RefObservation::Commit(_)
                ));
                assert!(matches!(
                    observed(f.repo.path(), b"refs/tags/keep-tag"),
                    RefObservation::Commit(_)
                ));
                if let Some(path) = prepush {
                    fs::remove_file(path).unwrap();
                }
                if mode == "reject" {
                    fs::remove_file(hook_path).unwrap();
                    let retry = completion::complete(f.repo.path(), &refreshed(&f, &req));
                    assert!(retry.is_complete(), "{retry:#?}");
                    assert_eq!(completion_pushes(&retry), 1);
                }
            }
        },
    );
}

#[test]
fn completion_metadata_write_failure_and_linked_worktree_evidence_are_contained() {
    isolated(
        "completion_metadata_write_failure_and_linked_worktree_evidence_are_contained",
        true,
        |root| {
            use std::os::unix::fs::PermissionsExt;
            let f = fixture(root);
            f.repo.success(&["push", "fork", "A"]);
            let req = completion_request(
                FinishIntegration::Squash,
                FinishCleanup::Both,
                FinishCheckout::Target,
            );
            let p = delivered(&f, &req);
            let parent = p.recovery_path.as_ref().unwrap().parent().unwrap();
            fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
            let r = completion::complete(f.repo.path(), &p);
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(!r.is_complete());
            assert!(r.effects.is_empty());
            assert_eq!(observed(f.repo.path(), b"refs/heads/A"), p.source);
            assert_eq!(observed(f.fork.path(), b"refs/heads/A"), p.source);
            let r = completion::complete(f.repo.path(), &refreshed(&f, &req));
            assert!(r.is_complete(), "{r:#?}");
            let linked = root.join("linked");
            f.repo.success(&[
                "worktree",
                "add",
                "-b",
                "independent",
                linked.to_str().unwrap(),
            ]);
            let retry = completion::complete(&linked, r.plan.as_ref().unwrap());
            assert!(!retry.is_complete());
            assert!(retry.effects.is_empty());
            let independent = integration::assess(&linked, &req, None).unwrap();
            assert_eq!(independent.source_ref, b"refs/heads/independent");
            assert_ne!(independent.recovery_path, p.recovery_path);
            f.repo.success(&["switch", "-c", "B"]);
            let b = integration::assess(f.repo.path(), &req, None).unwrap();
            assert_eq!(b.source_ref, b"refs/heads/B");
            assert_ne!(b.recovery_path, p.recovery_path);
        },
    );
}
#[test]
fn completion_final_target_collision_and_removed_caller_directory() {
    isolated(
        "completion_final_target_collision_and_removed_caller_directory",
        true,
        |root| {
            for mode in ["collision", "nested"] {
                let f = fixture(root);
                f.repo.success(&["switch", "main"]);
                fs::create_dir(f.repo.path().join("target-dir")).unwrap();
                commit(&f.repo, "target-dir/file", "target only");
                publish(&f);
                f.repo.success(&["switch", "A"]);
                let req = completion_request(
                    FinishIntegration::Squash,
                    if mode == "collision" {
                        FinishCleanup::Local
                    } else {
                        FinishCleanup::Keep
                    },
                    if mode == "collision" {
                        FinishCheckout::Target
                    } else {
                        FinishCheckout::Work
                    },
                );
                let p = delivered(&f, &req);
                if mode == "nested" {
                    let nested = f.repo.path().join("target-dir");
                    let r = completion::complete(&nested, &p);
                    assert!(r.is_complete(), "{r:#?}");
                    assert!(!nested.exists());
                    assert!(r.remaining.is_ok());
                } else {
                    f.repo.success(&["switch", "A"]);
                    fs::create_dir_all(f.repo.path().join(".git/info")).unwrap();
                    fs::write(f.repo.path().join(".git/info/exclude"), "target-dir/\n").unwrap();
                    fs::create_dir(f.repo.path().join("target-dir")).unwrap();
                    fs::write(f.repo.path().join("target-dir/file"), "ignored work").unwrap();
                    let p = refreshed(&f, &req);
                    let r = completion::complete(f.repo.path(), &p);
                    assert!(!r.is_complete(), "{r:#?}");
                    assert!(r.effects.is_empty());
                    assert_eq!(
                        fs::read_to_string(f.repo.path().join("target-dir/file")).unwrap(),
                        "ignored work"
                    );
                    assert_eq!(observed(f.repo.path(), b"refs/heads/A"), p.source);
                }
            }
        },
    );
}
#[test]
fn completion_unresolved_choices_and_missing_incorporation_proof_do_not_authorize_effects() {
    isolated(
        "completion_unresolved_choices_and_missing_incorporation_proof_do_not_authorize_effects",
        true,
        |root| {
            for mode in ["missing", "contradictory", "unrecorded", "rewritten"] {
                let f = fixture(root);
                let mut req = completion_request(
                    FinishIntegration::Squash,
                    FinishCleanup::Both,
                    FinishCheckout::Target,
                );
                f.repo.success(&["push", "fork", "A"]);
                match mode {
                    "missing" => req.cleanup = Some(FinishCleanup::Ask),
                    "contradictory" => req.checkout = Some(FinishCheckout::Work),
                    "rewritten" => {
                        f.repo
                            .success(&["commit", "--amend", "-m", "rewritten source"]);
                    }
                    _ => {}
                }
                let p = refreshed(&f, &req);
                if mode != "unrecorded" {
                    assert!(!p.is_ready(), "{mode}: {p:#?}");
                }
                let before = f.repo.success(&["show-ref"]).stdout;
                let r = completion::complete(f.repo.path(), &p);
                assert!(!r.is_complete(), "{mode}: {r:#?}");
                assert!(r.effects.is_empty());
                assert_eq!(completion_pushes(&r), 0);
                assert_eq!(before, f.repo.success(&["show-ref"]).stdout);
            }
        },
    );
}

#[test]
fn completion_unavailable_target_observation_does_not_reuse_delivery_report() {
    isolated(
        "completion_unavailable_target_observation_does_not_reuse_delivery_report",
        true,
        |root| {
            let f = fixture(root);
            f.repo.success(&["push", "fork", "A"]);
            let req = completion_request(
                FinishIntegration::Squash,
                FinishCleanup::Both,
                FinishCheckout::Target,
            );
            let p = delivered(&f, &req);
            let data = target_child_result(
                &f,
                "complete",
                "ls-remote",
                "if [ -f .git/completion-boundary ]; then exit 1; fi; exec \"$ONEFLOW_REAL_GIT\" \"$@\"",
            );
            assert_eq!(data["complete"], false, "{data}");
            assert_eq!(data["plan_ready"], false, "{data}");
            assert_eq!(data["plan_state"], "ChangedOrUnverifiable", "{data}");
            assert_eq!(data["pushes"], 0);
            assert!(data["effects"].as_array().unwrap().is_empty());
            assert_eq!(observed(f.repo.path(), b"refs/heads/A"), p.source);
            assert_eq!(observed(f.fork.path(), b"refs/heads/A"), p.source);
        },
    );
}

#[test]
fn completion_target_checkout_stops_cleanup_on_hook_failure_or_changed_binding() {
    isolated(
        "completion_target_checkout_stops_cleanup_on_hook_failure_or_changed_binding",
        true,
        |root| {
            for mode in ["success", "failure", "config"] {
                let f = fixture(root);
                f.repo.success(&["push", "fork", "A"]);
                let req = completion_request(
                    FinishIntegration::Squash,
                    FinishCleanup::Both,
                    FinishCheckout::Target,
                );
                delivered(&f, &req);
                f.repo.success(&["switch", "A"]);
                let p = refreshed(&f, &req);
                let h = match mode {
                    "failure" => Some(hook(&f, "post-checkout", "exit 1")),
                    "config" => Some(hook(&f, "post-checkout", "git config oneflow.hook changed")),
                    _ => None,
                };
                let r = completion::complete(f.repo.path(), &p);
                assert_eq!(r.is_complete(), mode == "success", "{mode}: {r:#?}");
                assert!(
                    r.effects
                        .iter()
                        .any(|e| matches!(e, completion::Effect::CheckedOut { .. }))
                );
                assert_eq!(
                    f.repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                    b"main\n"
                );
                if mode != "success" {
                    assert_eq!(observed(f.repo.path(), b"refs/heads/A"), p.source);
                    assert_eq!(observed(f.fork.path(), b"refs/heads/A"), p.source);
                    assert_eq!(completion_pushes(&r), 0);
                    fs::remove_file(h.unwrap()).unwrap();
                    if mode == "config" {
                        f.repo.success(&["config", "--unset", "oneflow.hook"]);
                    }
                    let retry = completion::complete(f.repo.path(), &refreshed(&f, &req));
                    assert!(retry.is_complete(), "{retry:#?}");
                    assert!(
                        !retry
                            .effects
                            .iter()
                            .any(|e| matches!(e, completion::Effect::CheckedOut { .. }))
                    );
                }
            }
        },
    );
}

#[test]
fn tagged_library_completion_requires_tag_and_marker_at_every_endpoint() {
    isolated(
        "tagged_library_completion_requires_tag_and_marker_at_every_endpoint",
        true,
        |root| {
            let f = fixture(root);
            let path = f.repo.path().join(".git/git-oneflow.config.toml");
            let mut config = fs::read_to_string(&path).unwrap();
            config.push_str("\n[workstreams.release]\ncreation-source='main'\npreparation-source='main'\nintegration-target='main'\nversion-tagging='required'\nrelease-marker='stable'\nfinish-mode='integrate'\nfinish-update='none'\nfinish-integration='merge'\nfinish-cleanup='both'\nfinish-message='auto'\n");
            fs::write(&path, config).unwrap();
            let started = Command::new(env!("CARGO_BIN_EXE_gof"))
                .current_dir(f.repo.path())
                .args(["start", "A", "--policy", "release"])
                .output()
                .unwrap();
            assert!(started.status.success(), "{started:?}");
            let source = oid(&f.repo, "HEAD");
            let req = Request {
                cleanup: Some(FinishCleanup::Both),
                release: integration::release::Request {
                    tag: Some("version-library".into()),
                    message: Some("Exact source version".into()),
                    sign: false,
                },
                ..request(FinishIntegration::Merge)
            };
            let initial = integration::assess(f.repo.path(), &req, None).unwrap();
            let refreshed = integration::remote::refresh(f.repo.path(), &initial);
            assert!(refreshed.error.is_none(), "{refreshed:?}");
            let prepared = integration::execution::prepare(f.repo.path(), &refreshed.plan.unwrap());
            assert!(prepared.error.is_none(), "{prepared:?}");
            let prepared = prepared.plan.unwrap();
            assert_eq!(oid(&f.repo, "version-library^{commit}"), source);
            assert_ne!(prepared.result.as_ref(), Some(&source));
            let delivered = integration::publication::publish(f.repo.path(), &prepared);
            assert!(delivered.is_complete(), "{delivered:?}");
            let delivered = delivered.plan.unwrap();
            let record_path = delivered.recovery_path.as_ref().unwrap();
            let original_record = fs::read(record_path).unwrap();
            let mut corrupt: serde_json::Value = serde_json::from_slice(&original_record).unwrap();
            corrupt["release"]["marker"] = serde_json::Value::Null;
            fs::write(record_path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
            let invalid =
                integration::assess(f.repo.path(), &req, delivered.evidence.as_ref()).unwrap();
            assert!(
                invalid
                    .blockers
                    .iter()
                    .any(|b| matches!(b, Blocker::Release(_)))
            );
            assert!(!integration::completion::complete(f.repo.path(), &invalid).is_complete());
            fs::write(record_path, &original_record).unwrap();
            let premature = integration::completion::complete(f.repo.path(), &delivered);
            assert!(!premature.is_complete(), "{premature:?}");
            assert_eq!(
                observed(f.repo.path(), b"refs/heads/A"),
                RefObservation::Commit(source.clone())
            );
            assert_eq!(
                observed(f.project.path(), b"refs/tags/version-library"),
                RefObservation::Absent
            );
            let mut commands = vec![];
            let all =
                integration::release::deliver(f.repo.path(), &delivered, false, &mut commands)
                    .unwrap();
            assert_eq!(
                observed(f.project.path(), b"refs/heads/stable"),
                RefObservation::Commit(source.clone())
            );
            assert_eq!(
                observed(f.repo.path(), b"refs/remotes/project/stable"),
                RefObservation::Commit(source.clone())
            );
            assert_eq!(
                observed(f.project.path(), b"refs/tags/version-library"),
                observed(f.repo.path(), b"refs/tags/version-library")
            );
            let completed = integration::completion::complete(f.repo.path(), &all);
            assert!(completed.is_complete(), "{completed:?}");
            assert_eq!(
                observed(f.repo.path(), b"refs/heads/A"),
                RefObservation::Absent
            );
            let initial = integration::assess(f.repo.path(), &Request::default(), None).unwrap();
            let refreshed = integration::remote::refresh(f.repo.path(), &initial);
            assert!(refreshed.error.is_none(), "{refreshed:?}");
            let retry = integration::completion::complete(f.repo.path(), &refreshed.plan.unwrap());
            assert!(retry.is_complete(), "{retry:?}");
            assert!(retry.effects.is_empty());
        },
    );
}

#[test]
fn tagged_source_rejects_replacement_tree_even_with_clean_matching_head() {
    isolated(
        "tagged_source_rejects_replacement_tree_even_with_clean_matching_head",
        true,
        |root| {
            let f = fixture(root);
            let path = f.repo.path().join(".git/git-oneflow.config.toml");
            let mut config = fs::read_to_string(&path).unwrap();
            config.push_str("\n[workstreams.release]\ncreation-source='main'\npreparation-source='main'\nintegration-target='main'\nversion-tagging='required'\nfinish-mode='integrate'\nfinish-update='none'\nfinish-integration='merge'\nfinish-cleanup='both'\nfinish-message='auto'\n");
            fs::write(&path, config).unwrap();
            let started = Command::new(env!("CARGO_BIN_EXE_gof"))
                .current_dir(f.repo.path())
                .args(["start", "A", "--policy", "release"])
                .output()
                .unwrap();
            assert!(started.status.success(), "{started:?}");
            let source = oid(&f.repo, "HEAD");
            f.repo.success(&["switch", "-c", "replacement"]);
            let replacement = commit(&f.repo, "work", "substituted tree");
            f.repo.success(&["switch", "main"]);
            f.repo
                .success(&["replace", text(&source), text(&replacement)]);
            f.repo.success(&["switch", "A"]);
            assert_eq!(oid(&f.repo, "HEAD"), source);
            assert!(f.repo.success(&["status", "--porcelain"]).stdout.is_empty());
            assert!(
                !f.repo
                    .success(&["--no-replace-objects", "diff", "--name-only", text(&source)])
                    .stdout
                    .is_empty()
            );
            let req = Request {
                release: integration::release::Request {
                    tag: Some("replacement-version".into()),
                    ..Default::default()
                },
                ..request(FinishIntegration::Merge)
            };
            let plan = integration::assess(f.repo.path(), &req, None).unwrap();
            assert!(
                plan.blockers.iter().any(
                    |b| matches!(b, Blocker::Release(message) if message.contains("replacement"))
                ),
                "{plan:#?}"
            );
            let result = integration::execution::prepare(f.repo.path(), &plan);
            assert!(result.error.is_some(), "{result:?}");
            assert!(result.commands.is_empty());
            assert_eq!(
                observed(f.repo.path(), b"refs/tags/replacement-version"),
                RefObservation::Absent
            );
            assert_eq!(
                observed(f.repo.path(), b"refs/heads/A"),
                RefObservation::Commit(source)
            );
        },
    );
}

#[test]
fn tagged_creation_rejects_same_source_tag_on_separate_fetch_endpoint() {
    isolated(
        "tagged_creation_rejects_same_source_tag_on_separate_fetch_endpoint",
        true,
        |root| {
            let f = fixture(root);
            f.repo
                .success(&["push", "project", "HEAD:refs/tags/fetch-collision"]);
            f.repo.success(&[
                "remote",
                "set-url",
                "--add",
                "--push",
                "project",
                f.fork.path().to_str().unwrap(),
            ]);
            let path = f.repo.path().join(".git/git-oneflow.config.toml");
            let mut config = fs::read_to_string(&path).unwrap();
            config.push_str("\n[workstreams.release]\ncreation-source='main'\npreparation-source='main'\nintegration-target='main'\nversion-tagging='required'\nfinish-mode='integrate'\nfinish-update='none'\nfinish-integration='merge'\nfinish-cleanup='both'\nfinish-message='auto'\n");
            fs::write(&path, config).unwrap();
            let started = Command::new(env!("CARGO_BIN_EXE_gof"))
                .current_dir(f.repo.path())
                .args(["start", "A", "--policy", "release"])
                .output()
                .unwrap();
            assert!(started.status.success(), "{started:?}");
            let target_before = oid(&f.repo, "main");
            let req = Request {
                release: integration::release::Request {
                    tag: Some("fetch-collision".into()),
                    ..Default::default()
                },
                ..request(FinishIntegration::Merge)
            };
            let initial = integration::assess(f.repo.path(), &req, None).unwrap();
            let refreshed = integration::remote::refresh(f.repo.path(), &initial);
            assert!(refreshed.error.is_none(), "{refreshed:?}");
            let result = integration::execution::prepare(f.repo.path(), &refreshed.plan.unwrap());
            assert!(
                result
                    .error
                    .as_ref()
                    .is_some_and(|e| e.to_string().contains("collision")),
                "{result:?}"
            );
            assert_eq!(
                observed(f.repo.path(), b"refs/tags/fetch-collision"),
                RefObservation::Absent
            );
            assert_eq!(
                observed(f.repo.path(), b"refs/heads/main"),
                RefObservation::Commit(target_before.clone())
            );
            assert_eq!(
                observed(f.fork.path(), b"refs/heads/main"),
                RefObservation::Commit(target_before)
            );
            assert_eq!(
                observed(f.fork.path(), b"refs/tags/fetch-collision"),
                RefObservation::Absent
            );
        },
    );
}
