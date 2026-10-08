#[path = "support/snapshot.rs"]
mod preservation;
mod support;
use git_oneflow::{
    finish::{
        self, ChoiceSource, Constraint, DestinationEvidence, FinishError, PublicationMode,
        RefObservation, Relationship, RemoteEvidence, Request, RequiredInput,
    },
    git,
    save::SaveError,
    settings::{FinishRewrite, FinishUpdate},
};
use preservation::snapshot;
use std::{fs, path::Path};
use support::{Repository, isolated};

fn commit(repo: &Repository, text: &str) -> Vec<u8> {
    fs::write(repo.path().join("tracked"), text).unwrap();
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", text]);
    oid(repo, "HEAD")
}
fn oid(repo: &Repository, reference: &str) -> Vec<u8> {
    let bytes = repo.success(&["rev-parse", reference]).stdout;
    bytes.strip_suffix(b"\n").unwrap().to_vec()
}
fn fixture(root: &Path) -> (Repository, Vec<u8>) {
    let repo = Repository::new(root);
    let base = commit(&repo, "base");
    repo.success(&["switch", "-c", "Feature/Exact_Name"]);
    // No server is available. Assessment/revalidation must remain offline.
    repo.success(&["remote", "add", "origin", "/missing/finish-fetch"]);
    repo.success(&["config", "remote.origin.pushurl", "/missing/finish-push"]);
    (repo, base)
}
fn evidence(base: Vec<u8>, branch: RefObservation) -> RemoteEvidence {
    RemoteEvidence {
        remote: "origin".into(),
        base_ref: b"refs/heads/main".to_vec(),
        branch_ref: b"refs/heads/Feature/Exact_Name".to_vec(),
        base: RefObservation::Commit(base),
        destinations: vec![DestinationEvidence {
            destination: b"/missing/finish-push".to_vec(),
            branch,
        }],
    }
}
fn assess(
    root: &Path,
    repo: &Repository,
    request: &Request,
    evidence: Option<&RemoteEvidence>,
) -> Result<finish::Plan, FinishError> {
    let before = snapshot(root);
    let result = finish::assess(repo.path(), request, evidence);
    assert_eq!(snapshot(root), before, "assessment mutated fixture");
    result
}
fn revalidate(
    root: &Path,
    repo: &Repository,
    plan: &finish::Plan,
    evidence: Option<&RemoteEvidence>,
) -> Result<(), FinishError> {
    let before = snapshot(root);
    let result = finish::revalidate(repo.path(), plan, evidence);
    assert_eq!(snapshot(root), before, "revalidation mutated fixture");
    result
}
fn request(update: FinishUpdate, rewrite: FinishRewrite) -> Request {
    Request {
        update: Some(update),
        rewrite: Some(rewrite),
    }
}

#[test]
fn local_hints_cannot_establish_freshness_and_wip_is_ready() {
    isolated(
        "local_hints_cannot_establish_freshness_and_wip_is_ready",
        true,
        |root| {
            let (repo, base) = fixture(root);
            commit(&repo, "[WIP - Staged]");
            repo.success(&[
                "update-ref",
                "refs/remotes/origin/main",
                std::str::from_utf8(&base).unwrap(),
            ]);
            let plan = assess(root, &repo, &Request::default(), None).unwrap();
            assert_eq!(
                plan.tracking_hints.base,
                RefObservation::Commit(base.clone())
            );
            assert_eq!(plan.constraints, [Constraint::FreshRemoteEvidenceRequired]);
            assert_eq!(plan.base_relationship, None);
            assert!(plan.required_input.is_empty());
            assert!(!plan.is_ready());
            revalidate(root, &repo, &plan, None).unwrap();
            let remote = evidence(base, RefObservation::Absent);
            for update in [
                FinishUpdate::Ask,
                FinishUpdate::Rebase,
                FinishUpdate::Merge,
                FinishUpdate::None,
            ] {
                let plan = assess(
                    root,
                    &repo,
                    &request(update, FinishRewrite::Ask),
                    Some(&remote),
                )
                .unwrap();
                assert!(plan.is_ready());
                assert_eq!(plan.update, FinishUpdate::None);
                assert_eq!(plan.update_source, ChoiceSource::IncludesBase);
                assert_eq!(plan.base_relationship, Some(Relationship::Ahead));
                assert_eq!(plan.publication.branch_ref, remote.branch_ref);
                assert!(plan.publication.establish_upstream);
                assert_eq!(plan.destinations[0].mode, PublicationMode::Create);
                revalidate(root, &repo, &plan, Some(&remote)).unwrap();
            }
        },
    );
}

#[test]
fn finish_readiness_guards_preserve_every_path() {
    isolated(
        "finish_readiness_guards_preserve_every_path",
        true,
        |root| {
            let repo = Repository::new(root);
            assert!(matches!(
                assess(root, &repo, &Request::default(), None),
                Err(FinishError::Local(SaveError::UnbornBranch))
            ));
            let outside = root.join("outside");
            fs::create_dir(&outside).unwrap();
            let before = snapshot(root);
            assert!(matches!(
                finish::assess(&outside, &Request::default(), None),
                Err(FinishError::Local(SaveError::OutsideRepository))
            ));
            assert_eq!(snapshot(root), before);
            let bare = root.join("bare");
            git::run(root, ["init", "--bare", bare.to_str().unwrap()]).unwrap();
            let before = snapshot(root);
            assert!(matches!(
                finish::assess(&bare, &Request::default(), None),
                Err(FinishError::Local(SaveError::BareRepository))
            ));
            assert_eq!(snapshot(root), before);
            for case in [
                "base",
                "detached",
                "staged",
                "unstaged",
                "untracked",
                "conflict",
                "merge",
                "rebase",
                "am",
                "cherry-pick",
                "revert",
                "bisect",
                "sequencer",
            ] {
                let (repo, _) = fixture(root);
                match case {
                    "base" => {
                        repo.success(&["switch", "main"]);
                    }
                    "detached" => {
                        repo.success(&["switch", "--detach"]);
                    }
                    "staged" | "unstaged" => {
                        fs::write(repo.path().join("tracked"), "changed").unwrap();
                        if case == "staged" {
                            repo.success(&["add", "."]);
                        }
                    }
                    "untracked" => {
                        fs::write(repo.path().join("new"), "new").unwrap();
                    }
                    "conflict" => {
                        repo.success(&["switch", "main"]);
                        commit(&repo, "base update");
                        repo.success(&["switch", "Feature/Exact_Name"]);
                        commit(&repo, "work update");
                        assert!(
                            !git::run(repo.path(), ["merge", "main"])
                                .unwrap()
                                .status
                                .success()
                        );
                        fs::remove_file(repo.path().join(".git/MERGE_HEAD")).unwrap();
                    }
                    "rebase" => {
                        fs::create_dir(repo.path().join(".git/rebase-merge")).unwrap();
                    }
                    "am" => {
                        fs::create_dir(repo.path().join(".git/rebase-apply")).unwrap();
                        fs::write(repo.path().join(".git/rebase-apply/applying"), "").unwrap();
                    }
                    "sequencer" => {
                        fs::create_dir(repo.path().join(".git/sequencer")).unwrap();
                        fs::write(
                            repo.path().join(".git/sequencer/todo"),
                            "pick deadbeef work\n",
                        )
                        .unwrap();
                    }
                    other => {
                        let name = match other {
                            "merge" => "MERGE_HEAD",
                            "cherry-pick" => "CHERRY_PICK_HEAD",
                            "revert" => "REVERT_HEAD",
                            _ => "BISECT_START",
                        };
                        fs::write(repo.path().join(".git").join(name), "deadbeef\n").unwrap();
                    }
                }
                let result = assess(root, &repo, &Request::default(), None);
                match case {
                    "base" => assert!(matches!(
                        result,
                        Err(FinishError::Local(SaveError::ProtectedBase))
                    )),
                    "detached" => assert!(matches!(
                        result,
                        Err(FinishError::Local(SaveError::DetachedHead))
                    )),
                    "staged" | "unstaged" | "untracked" => {
                        assert!(matches!(result, Err(FinishError::DirtyTree)))
                    }
                    "conflict" => assert!(matches!(
                        result,
                        Err(FinishError::Local(SaveError::Conflicts))
                    )),
                    _ => assert!(matches!(
                        result,
                        Err(FinishError::Local(SaveError::ActiveOperation))
                    )),
                }
            }
        },
    );
}

#[test]
fn finish_submodule_cleanliness_includes_gitlinks_and_content() {
    isolated(
        "finish_submodule_cleanliness_includes_gitlinks_and_content",
        true,
        |root| {
            let sub = Repository::new(root);
            commit(&sub, "sub base");
            for case in ["tracked", "untracked", "commit", "staged"] {
                let (repo, _) = fixture(root);
                repo.success(&[
                    "-c",
                    "protocol.file.allow=always",
                    "submodule",
                    "add",
                    sub.path().to_str().unwrap(),
                    "sub",
                ]);
                repo.success(&["commit", "-am", "add submodule"]);
                assess(root, &repo, &Request::default(), None).unwrap();
                let path = repo.path().join("sub");
                match case {
                    "untracked" => fs::write(path.join("new"), "new").unwrap(),
                    "tracked" => fs::write(path.join("tracked"), "dirty").unwrap(),
                    _ => {
                        fs::write(path.join("tracked"), "new commit").unwrap();
                        for args in [
                            vec!["config", "user.name", "Fixture"],
                            vec!["config", "user.email", "fixture@example.invalid"],
                            vec!["commit", "-am", "sub work"],
                        ] {
                            assert!(git::run(&path, args).unwrap().status.success());
                        }
                        if case == "staged" {
                            repo.success(&["add", "sub"]);
                        }
                    }
                }
                assert!(matches!(
                    assess(root, &repo, &Request::default(), None),
                    Err(FinishError::DirtyTree | FinishError::Local(SaveError::DirtySubmodules))
                ));
            }
        },
    );
}

#[test]
fn finish_preserves_strict_upstream_and_mapping_guards() {
    isolated(
        "finish_preserves_strict_upstream_and_mapping_guards",
        true,
        |root| {
            for (remotes, merges, valid) in [
                (vec![], vec![], true),
                (vec!["origin"], vec!["refs/heads/Feature/Exact_Name"], true),
                (vec!["origin"], vec![], false),
                (vec![], vec!["refs/heads/Feature/Exact_Name"], false),
                (vec!["other"], vec!["refs/heads/Feature/Exact_Name"], false),
                (vec!["origin"], vec!["refs/heads/main"], false),
                (
                    vec!["origin", "origin"],
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
            ] {
                let (repo, _) = fixture(root);
                for remote in remotes {
                    repo.success(&[
                        "config",
                        "--add",
                        "branch.Feature/Exact_Name.remote",
                        remote,
                    ]);
                }
                for merge in merges {
                    repo.success(&["config", "--add", "branch.Feature/Exact_Name.merge", merge]);
                }
                let result = assess(root, &repo, &Request::default(), None);
                if valid {
                    assert!(result.is_ok());
                } else {
                    assert!(matches!(
                        result,
                        Err(FinishError::Local(SaveError::InvalidUpstream))
                    ));
                }
            }
            let (repo, _) = fixture(root);
            repo.success(&[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/heads/*",
            ]);
            assert!(matches!(
                assess(root, &repo, &Request::default(), None),
                Err(FinishError::Publication(_))
            ));
        },
    );
}

#[test]
fn update_and_rewrite_are_independent_choices() {
    isolated("update_and_rewrite_are_independent_choices", true, |root| {
        let (repo, base) = fixture(root);
        let work = commit(&repo, "work");
        repo.success(&["switch", "main"]);
        let new_base = commit(&repo, "base update");
        repo.success(&["switch", "Feature/Exact_Name"]);
        let remote = evidence(new_base, RefObservation::Commit(work));
        let plan = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
        assert_eq!(plan.base_relationship, Some(Relationship::Diverged));
        assert_eq!(plan.required_input, [RequiredInput::Update]);
        for update in [
            FinishUpdate::None,
            FinishUpdate::Merge,
            FinishUpdate::Rebase,
        ] {
            for rewrite in [
                FinishRewrite::Ask,
                FinishRewrite::Allow,
                FinishRewrite::Deny,
            ] {
                let plan = assess(root, &repo, &request(update, rewrite), Some(&remote)).unwrap();
                assert_eq!(plan.update_source, ChoiceSource::Request);
                assert_eq!(plan.rewrite_source, ChoiceSource::Request);
                if update == FinishUpdate::Rebase {
                    assert_eq!(
                        plan.destinations[0].mode,
                        PublicationMode::RecheckAfterRebase
                    );
                    assert_eq!(
                        plan.required_input,
                        if rewrite == FinishRewrite::Ask {
                            vec![RequiredInput::Rewrite]
                        } else {
                            vec![]
                        }
                    );
                    assert_eq!(plan.is_ready(), rewrite == FinishRewrite::Allow);
                } else {
                    assert!(plan.is_ready());
                    assert_eq!(
                        plan.destinations[0].mode,
                        PublicationMode::FastForwardOrUnchanged
                    );
                }
            }
        }
        // Published history in the new base requires no rewrite authorization.
        let remote = evidence(oid(&repo, "main"), RefObservation::Commit(base));
        let plan = assess(
            root,
            &repo,
            &request(FinishUpdate::Rebase, FinishRewrite::Ask),
            Some(&remote),
        )
        .unwrap();
        assert!(plan.is_ready());
        assert_eq!(
            plan.destinations[0].mode,
            PublicationMode::FastForwardOrUnchanged
        );
    });
}

#[test]
fn remote_work_missing_locally_blocks_even_authorized_rewrites() {
    isolated(
        "remote_work_missing_locally_blocks_even_authorized_rewrites",
        true,
        |root| {
            let (repo, base) = fixture(root);
            repo.success(&["switch", "-c", "remote-work"]);
            let remote_tip = commit(&repo, "remote work");
            repo.success(&["switch", "Feature/Exact_Name"]);
            for expected in [Relationship::Behind, Relationship::Diverged] {
                if expected == Relationship::Diverged {
                    commit(&repo, "local work");
                }
                let remote = evidence(base.clone(), RefObservation::Commit(remote_tip.clone()));
                for update in [
                    FinishUpdate::None,
                    FinishUpdate::Merge,
                    FinishUpdate::Rebase,
                ] {
                    let plan = assess(
                        root,
                        &repo,
                        &request(update, FinishRewrite::Allow),
                        Some(&remote),
                    )
                    .unwrap();
                    assert_eq!(plan.destinations[0].relationship, expected);
                    assert_eq!(plan.destinations[0].mode, PublicationMode::Blocked);
                    assert_eq!(
                        plan.constraints,
                        [Constraint::RemoteWorkNotIncorporated(
                            b"/missing/finish-push".to_vec()
                        )]
                    );
                    assert!(!plan.is_ready());
                }
            }
        },
    );
}

#[test]
fn unavailable_ambiguous_unrelated_and_shallow_history_are_explicit() {
    isolated(
        "unavailable_ambiguous_unrelated_and_shallow_history_are_explicit",
        true,
        |root| {
            let (repo, base) = fixture(root);
            for observation in [
                RefObservation::Absent,
                RefObservation::Unavailable,
                RefObservation::Ambiguous,
                RefObservation::Commit(vec![b'a'; 40]),
            ] {
                let mut remote = evidence(base.clone(), observation.clone());
                remote.base = observation;
                let plan = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
                assert!(plan.constraints.contains(&Constraint::BaseUnavailable));
                assert!(!plan.is_ready());
            }
            repo.success(&["switch", "--orphan", "unrelated"]);
            let unrelated = commit(&repo, "unrelated");
            repo.success(&["switch", "Feature/Exact_Name"]);
            let remote = evidence(unrelated.clone(), RefObservation::Commit(unrelated));
            let plan = assess(
                root,
                &repo,
                &request(FinishUpdate::Merge, FinishRewrite::Allow),
                Some(&remote),
            )
            .unwrap();
            assert_eq!(plan.base_relationship, Some(Relationship::Unrelated));
            assert!(plan.constraints.contains(&Constraint::UnrelatedBase));
            assert_eq!(plan.destinations[0].relationship, Relationship::Unrelated);
            repo.success(&["switch", "main"]);
            let new_base = commit(&repo, "new base");
            repo.success(&["switch", "Feature/Exact_Name"]);
            let work = commit(&repo, "work");
            // A real shallow boundary hides the shared parent from traversal.
            fs::write(
                repo.path().join(".git/shallow"),
                [work.as_slice(), b"\n"].concat(),
            )
            .unwrap();
            let remote = evidence(new_base, RefObservation::Commit(base.clone()));
            let plan = assess(
                root,
                &repo,
                &request(FinishUpdate::Rebase, FinishRewrite::Allow),
                Some(&remote),
            )
            .unwrap();
            assert_eq!(plan.base_relationship, Some(Relationship::Uncertain));
            assert_eq!(plan.destinations[0].relationship, Relationship::Uncertain);
            assert!(plan.constraints.contains(&Constraint::BaseHistoryUncertain));
            assert!(!plan.is_ready());
            // Equality remains provable even in a shallow repository.
            let remote = evidence(work.clone(), RefObservation::Commit(work));
            assert!(
                assess(root, &repo, &Request::default(), Some(&remote))
                    .unwrap()
                    .is_ready()
            );
        },
    );
}

#[test]
fn exact_multiple_destinations_and_evidence_binding() {
    isolated(
        "exact_multiple_destinations_and_evidence_binding",
        true,
        |root| {
            let (repo, base) = fixture(root);
            repo.success(&[
                "config",
                "--add",
                "remote.origin.pushurl",
                "/missing/second-push",
            ]);
            let mut remote = evidence(base.clone(), RefObservation::Absent);
            assert!(matches!(
                assess(root, &repo, &Request::default(), Some(&remote)),
                Err(FinishError::InvalidEvidence(_))
            ));
            remote.destinations.push(DestinationEvidence {
                destination: b"/missing/second-push".to_vec(),
                branch: RefObservation::Commit(base.clone()),
            });
            let plan = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
            assert!(plan.is_ready());
            assert_eq!(plan.destinations[1].relationship, Relationship::Equal);
            remote.destinations[1].branch = RefObservation::Ambiguous;
            let plan = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
            assert!(plan.constraints.contains(&Constraint::DestinationAmbiguous(
                b"/missing/second-push".to_vec()
            )));
            for case in ["remote", "base", "branch", "order", "oid"] {
                let mut changed = remote.clone();
                match case {
                    "remote" => changed.remote = "other".into(),
                    "base" => changed.base_ref = b"refs/heads/trunk".to_vec(),
                    "branch" => changed.branch_ref = b"refs/heads/feature/exact_name".to_vec(),
                    "order" => changed.destinations.reverse(),
                    _ => changed.base = RefObservation::Commit(b"HEAD".to_vec()),
                }
                assert!(matches!(
                    assess(root, &repo, &Request::default(), Some(&changed)),
                    Err(FinishError::InvalidEvidence(_))
                ));
            }
            repo.success(&["config", "--unset-all", "remote.origin.pushurl"]);
            for _ in 0..2 {
                repo.success(&[
                    "config",
                    "--add",
                    "remote.origin.pushurl",
                    "/missing/finish-push",
                ]);
            }
            remote.destinations[1].destination = remote.destinations[0].destination.clone();
            remote.destinations[1].branch = RefObservation::Commit(base);
            let contradictory = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
            assert!(!contradictory.is_ready());
            assert!(
                contradictory
                    .destinations
                    .iter()
                    .all(|destination| destination.relationship == Relationship::Ambiguous)
            );
            remote.destinations[1].branch = RefObservation::Absent;
            assert!(
                assess(root, &repo, &Request::default(), Some(&remote))
                    .unwrap()
                    .is_ready()
            );
        },
    );
}

#[test]
fn meaningful_local_or_remote_changes_invalidate_plans() {
    isolated(
        "meaningful_local_or_remote_changes_invalidate_plans",
        true,
        |root| {
            for case in [
                "head",
                "branch",
                "index",
                "dirty",
                "settings",
                "source",
                "upstream",
                "pushurl",
                "fetch",
                "git-config",
                "shallow",
                "tracking",
                "remote-base",
                "remote-tip",
                "evidence-lost",
            ] {
                let (repo, base) = fixture(root);
                let mut remote = evidence(base.clone(), RefObservation::Absent);
                let plan = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
                match case {
                    "head" => {
                        commit(&repo, "new tip");
                    }
                    "branch" => {
                        repo.success(&["switch", "-c", "other"]);
                    }
                    "index" => {
                        repo.success(&["update-index", "--assume-unchanged", "tracked"]);
                    }
                    "dirty" => {
                        fs::write(repo.path().join("new"), "new").unwrap();
                    }
                    "settings" => {
                        fs::write(
                            repo.path().join(".git/oneflow.conf"),
                            "finish-rewrite=allow\n",
                        )
                        .unwrap();
                    }
                    "source" => {
                        fs::write(repo.path().join(".git/oneflow.conf"), "finish-update=ask\n")
                            .unwrap();
                    }
                    "upstream" => {
                        repo.success(&["config", "branch.Feature/Exact_Name.remote", "other"]);
                    }
                    "pushurl" => {
                        repo.success(&["config", "remote.origin.pushurl", "/missing/changed"]);
                    }
                    "fetch" => {
                        repo.success(&[
                            "config",
                            "remote.origin.fetch",
                            "+refs/heads/*:refs/remotes/custom/*",
                        ]);
                    }
                    "git-config" => {
                        repo.success(&["config", "rebase.autoStash", "true"]);
                    }
                    "shallow" => {
                        fs::write(
                            repo.path().join(".git/shallow"),
                            [base.as_slice(), b"\n"].concat(),
                        )
                        .unwrap();
                    }
                    "tracking" => {
                        repo.success(&[
                            "update-ref",
                            "refs/remotes/origin/main",
                            std::str::from_utf8(&base).unwrap(),
                        ]);
                    }
                    "remote-base" => remote.base = RefObservation::Unavailable,
                    "remote-tip" => {
                        remote.destinations[0].branch = RefObservation::Commit(base.clone())
                    }
                    _ => (),
                }
                let current_evidence = (case != "evidence-lost").then_some(&remote);
                assert!(
                    revalidate(root, &repo, &plan, current_evidence).is_err(),
                    "{case}"
                );
            }
            // Formatting-only changes preserve settings values and sources.
            let (repo, base) = fixture(root);
            let path = repo.path().join(".git/oneflow.conf");
            fs::write(&path, "finish-update=none\n").unwrap();
            let remote = evidence(base, RefObservation::Absent);
            let plan = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
            fs::write(path, "# formatting\nfinish-update = none\n").unwrap();
            revalidate(root, &repo, &plan, Some(&remote)).unwrap();
        },
    );
}

#[test]
fn finish_request_overrides_config_without_hiding_invalid_settings() {
    isolated(
        "finish_request_overrides_config_without_hiding_invalid_settings",
        true,
        |root| {
            let (repo, _) = fixture(root);
            let work = commit(&repo, "work");
            repo.success(&["switch", "main"]);
            let base = commit(&repo, "base update");
            repo.success(&["switch", "Feature/Exact_Name"]);
            let path = repo.path().join(".git/oneflow.conf");
            fs::write(&path, "finish-update=rebase\nfinish-rewrite=allow\n").unwrap();
            let remote = evidence(base, RefObservation::Commit(work));
            let configured = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
            assert!(configured.is_ready());
            assert_eq!(configured.update, FinishUpdate::Rebase);
            assert_eq!(configured.rewrite, FinishRewrite::Allow);
            assert_eq!(
                configured.update_source,
                ChoiceSource::Setting(configured.settings.sources.finish_update.clone())
            );
            assert_eq!(
                configured.rewrite_source,
                ChoiceSource::Setting(configured.settings.sources.finish_rewrite.clone())
            );
            let explicit = assess(
                root,
                &repo,
                &request(FinishUpdate::None, FinishRewrite::Deny),
                Some(&remote),
            )
            .unwrap();
            assert!(explicit.is_ready());
            assert_eq!(explicit.update, FinishUpdate::None);
            assert_eq!(explicit.rewrite, FinishRewrite::Deny);
            fs::write(path, "finish-update=invalid\n").unwrap();
            assert!(matches!(
                assess(
                    root,
                    &repo,
                    &request(FinishUpdate::None, FinishRewrite::Deny),
                    Some(&remote)
                ),
                Err(FinishError::Local(SaveError::Settings(_)))
            ));
        },
    );
}

#[test]
fn a_branch_behind_base_requires_only_an_update_choice() {
    isolated(
        "a_branch_behind_base_requires_only_an_update_choice",
        true,
        |root| {
            let (repo, old_base) = fixture(root);
            repo.success(&["switch", "main"]);
            let base = commit(&repo, "base update");
            repo.success(&["switch", "Feature/Exact_Name"]);
            let remote = evidence(base, RefObservation::Commit(old_base));
            let plan = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
            assert_eq!(plan.base_relationship, Some(Relationship::Behind));
            assert_eq!(plan.required_input, [RequiredInput::Update]);
            for update in [
                FinishUpdate::Rebase,
                FinishUpdate::Merge,
                FinishUpdate::None,
            ] {
                let plan = assess(
                    root,
                    &repo,
                    &request(update, FinishRewrite::Ask),
                    Some(&remote),
                )
                .unwrap();
                assert!(plan.is_ready());
                assert_eq!(plan.update, update);
                assert_eq!(
                    plan.destinations[0].mode,
                    PublicationMode::FastForwardOrUnchanged
                );
            }
        },
    );
}

#[test]
fn synthetic_history_does_not_prove_remote_incorporation() {
    isolated(
        "synthetic_history_does_not_prove_remote_incorporation",
        true,
        |root| {
            let (repo, base) = fixture(root);
            let work = commit(&repo, "work");
            repo.success(&["switch", "-c", "remote-work", "main"]);
            let remote_tip = commit(&repo, "remote work");
            repo.success(&["switch", "Feature/Exact_Name"]);
            repo.success(&[
                "replace",
                "--graft",
                std::str::from_utf8(&work).unwrap(),
                std::str::from_utf8(&remote_tip).unwrap(),
            ]);
            let remote = evidence(base, RefObservation::Commit(remote_tip.clone()));
            let plan = assess(
                root,
                &repo,
                &request(FinishUpdate::None, FinishRewrite::Allow),
                Some(&remote),
            )
            .unwrap();
            assert_eq!(plan.destinations[0].relationship, Relationship::Diverged);
            assert!(!plan.is_ready());
            fs::create_dir_all(repo.path().join(".git/info")).unwrap();
            fs::write(
                repo.path().join(".git/info/grafts"),
                [work.as_slice(), b" ", remote_tip.as_slice(), b"\n"].concat(),
            )
            .unwrap();
            let plan = assess(
                root,
                &repo,
                &request(FinishUpdate::None, FinishRewrite::Allow),
                Some(&remote),
            )
            .unwrap();
            assert!(plan.constraints.contains(&Constraint::GraftedHistory));
            assert!(!plan.is_ready());
        },
    );
}

#[test]
fn inspection_disables_helpers_and_lazy_fetch() {
    isolated("inspection_disables_helpers_and_lazy_fetch", true, |root| {
        let (repo, _) = fixture(root);
        repo.success(&["config", "core.fsmonitor", "touch helper-ran"]);
        repo.success(&["config", "remote.origin.promisor", "true"]);
        let remote = evidence(vec![b'a'; 40], RefObservation::Commit(vec![b'b'; 40]));
        let plan = assess(root, &repo, &Request::default(), Some(&remote)).unwrap();
        assert_eq!(plan.base_relationship, Some(Relationship::Unavailable));
        assert_eq!(plan.destinations[0].relationship, Relationship::Unavailable);
        revalidate(root, &repo, &plan, Some(&remote)).unwrap();
        assert!(!repo.path().join("helper-ran").exists());
    });
}
