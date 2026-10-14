//! Terminal orchestration for tagged review preparation and manual submission.
use super::*;
use crate::finish::hosted;

pub(super) fn run(
    directory: &Path,
    initial: Result<hosted::Plan, FinishError>,
    review: &Request,
    completion: &Completion,
    presentation: Presentation,
    skip: bool,
    mut context_shown: bool,
) -> ExitCode {
    if completion.message.is_some()
        || completion
            .integration
            .is_some_and(|v| v != FinishIntegration::Merge)
        || completion
            .checkout
            .is_some_and(|v| v != FinishCheckout::Work)
        || completion.cleanup.is_some_and(|v| v != FinishCleanup::Keep)
        || review.rewrite == Some(FinishRewrite::Allow)
    {
        crate::presentation::error(
            "error: tagged review retains both branches; use merge preparation, --checkout work, --cleanup keep, and no rewrite. Final integration messages apply to integrate mode.",
        );
        return ExitCode::from(2);
    }
    let mut plan = match initial {
        Ok(plan) => plan,
        Err(error) => {
            crate::presentation::error(format!("error: {}", text(presentation, &error)));
            error_diagnostics(presentation, &error);
            return ExitCode::FAILURE;
        }
    };
    loop {
        let missing = if plan.tag.is_none() {
            Some((
                "version tag",
                "What version tag should identify this release?",
            ))
        } else if plan.merge_back.is_none() {
            Some((
                "merge-back branch",
                "Which branch should carry merge-back review work?",
            ))
        } else if plan.update == FinishUpdate::Ask {
            Some((
                "preparation method",
                "How should merge-back work be updated?",
            ))
        } else {
            None
        };
        let Some((label, prompt)) = missing else {
            break;
        };
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            crate::presentation::error(format!(
                "error: unresolved {label}; supply --tag, --merge-back, and --update merge|none as needed, or configure the corresponding settings.",
            ));
            return ExitCode::FAILURE;
        }
        if !context_shown {
            crate::presentation::section("Prepare version review");
            let policy = plan.settings.workstream.as_ref().expect("hosted policy");
            crate::presentation::prose(&format!(
                "Released work: {}\nReview into: {}/{}\nThe version stays fixed; a separate branch carries merge-back review work.",
                display(presentation, &plan.source_branch),
                text(presentation, &policy.base_remote),
                text(presentation, &policy.integration_target)
            ));
            context_shown = true;
        }
        let answer = match if label == "preparation method" {
            super::read_menu_answer(
                presentation,
                prompt,
                &[
                    (
                        "merge",
                        "Merge the project target into the merge-back branch",
                    ),
                    ("none", "Keep merge-back history unchanged"),
                    ("cancel", "Cancel"),
                ],
                "Finish cancelled before branch execution; finish made no branch, index, or working-tree changes.",
            )
        } else {
            read_reference_name(
                directory,
                presentation,
                prompt,
                if label == "version tag" {
                    "refs/tags/"
                } else {
                    "refs/heads/"
                },
                "Finish cancelled before branch execution; finish made no branch, index, or working-tree changes.",
            )
        } {
            Ok(answer) => answer,
            Err(error) => {
                super::error_or_plain(&error);
                return ExitCode::FAILURE;
            }
        };
        let mut request = plan.request.clone();
        if plan.tag.is_none() {
            request.release.tag = Some(answer);
        } else if plan.merge_back.is_none() {
            request.merge_back = Some(answer);
        } else {
            request.update = match answer.as_str() {
                "merge" => Some(FinishUpdate::Merge),
                "none" => Some(FinishUpdate::None),
                _ => {
                    crate::presentation::error("error: choose merge or none");
                    return ExitCode::FAILURE;
                }
            };
        }
        if let Err(error) = hosted::revalidate(directory, &plan) {
            crate::presentation::error(format!("error: {}", text(presentation, error)));
            return ExitCode::FAILURE;
        }
        let resolved = hosted::assess(directory, &request);
        if let Err(error) = hosted::revalidate(directory, &plan) {
            crate::presentation::error(format!("error: {}", text(presentation, error)));
            return ExitCode::FAILURE;
        }
        plan = match resolved {
            Ok(plan) => plan,
            Err(FinishError::InvalidEvidence(
                "merge-back branch must differ from source, target, and release marker",
            )) => {
                eprintln!(
                    "Choose a merge-back branch distinct from the source, target, and release marker."
                );
                continue;
            }
            Err(error) => {
                crate::presentation::error(format!("error: {}", text(presentation, error)));
                return ExitCode::FAILURE;
            }
        };
    }
    println!(
        "Released source: {} at {}.",
        display(presentation, &plan.source_branch),
        display(presentation, &plan.source)
    );
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    let mut check_output = CheckOutput::new(presentation);
    let report = hosted::execute(root, &plan, skip, |event| match event {
        hosted::Event::Source(oid) => eprintln!(
            "Checking released source tree at {}.",
            display(presentation, oid)
        ),
        hosted::Event::Candidate(oid) => eprintln!(
            "Checking merge-back candidate tree at {}.",
            display(presentation, oid)
        ),
        hosted::Event::Check(checks::Event::Started {
            index,
            total,
            check,
        }) => eprintln!(
            "Check {}/{}: {}",
            index + 1,
            total,
            text(presentation, &check.name)
        ),
        hosted::Event::Check(checks::Event::Output { stream, bytes }) => {
            check_output.write(stream, bytes)
        }
        hosted::Event::Check(checks::Event::Passed { index }) => {
            check_output.flush();
            eprintln!("Check {} passed.", index + 1);
        }
        hosted::Event::Check(checks::Event::Skipped { count }) => {
            eprintln!("Skipped {count} configured finish check(s) (--skip-checks).")
        }
    });
    check_output.flush();
    eprint!(
        "{}",
        crate::cli::cache_issues(&report.caches, |bytes| display(presentation, bytes), true)
    );
    let mut diagnostics = String::new();
    command_diagnostics(presentation, &mut diagnostics, &report.commands);
    if let Some(error) = &report.error {
        crate::presentation::report(
            &format!(
                "error: {}\nResolve the reported problem, then retry finish; confirmed effects are retained.\n",
                text(presentation, error)
            ),
            false,
        );
    }
    if !report.ready {
        println!(
            "Local version tag object: {}. Merge-back candidate: {}.",
            observation(
                presentation,
                report
                    .local_tag
                    .as_ref()
                    .unwrap_or(&finish::RefObservation::Unavailable)
            ),
            observation(
                presentation,
                report
                    .local_candidate
                    .as_ref()
                    .unwrap_or(&finish::RefObservation::Unavailable)
            )
        );
    }
    for publication in &report.publications {
        let status = if publication
            .expected
            .as_ref()
            .map(|oid| finish::RefObservation::Commit(oid.clone()))
            .as_ref()
            == Some(&publication.observed)
        {
            "confirmed"
        } else if matches!(
            publication.observed,
            finish::RefObservation::Unavailable | finish::RefObservation::Ambiguous
        ) {
            "unavailable"
        } else {
            "pending"
        };
        println!(
            "Publication {status}: {} on {} ({}) — {}.",
            display(presentation, &publication.reference),
            text(presentation, &publication.remote),
            display(presentation, &publication.destination),
            observation(presentation, &publication.observed)
        );
    }
    if report.error.is_some() {
        eprint!("{diagnostics}");
        eprintln!(
            "Retained source: {} at {}. Version tag: {}. Merge-back: {}.",
            display(presentation, &plan.source_branch),
            display(presentation, &plan.source),
            text(presentation, plan.tag.as_deref().unwrap_or("unresolved")),
            text(
                presentation,
                plan.merge_back.as_deref().unwrap_or("unresolved")
            )
        );
        eprintln!(
            "Resolve any preparation merge using git merge --continue, or git merge --abort to abandon that merge. Retry finish to verify and complete pending preparation/publication; confirmed effects are retained."
        );
        let mut output = String::new();
        remaining(presentation, &mut output, &repository::inspect(root));
        eprint!("{output}");
        return ExitCode::FAILURE;
    }
    if !report.ready {
        return ExitCode::FAILURE;
    }
    let plan = report.plan.as_ref().unwrap_or(&plan);
    let policy = plan.settings.workstream.as_ref().expect("hosted policy");
    crate::presentation::report(
        &format!(
            "Ready for manual submission. Version {} identifies {}.",
            text(presentation, plan.tag.as_deref().unwrap()),
            display(presentation, &plan.source)
        ),
        true,
    );
    println!(
        "Original source {} published to {}; exact version tag published to {}.",
        display(presentation, &plan.source_branch),
        text(presentation, &policy.publish_remote),
        text(presentation, &policy.base_remote)
    );
    println!(
        "Submit {} from review remote {} into {}/{} using your chosen tools; use no-ff integration.",
        text(presentation, plan.merge_back.as_deref().unwrap()),
        text(presentation, &policy.review_remote),
        text(presentation, &policy.base_remote),
        text(presentation, &policy.integration_target)
    );
    for destination in hosted::destinations(plan) {
        println!(
            "  Review repository: {}",
            display(presentation, &destination)
        );
    }
    println!(
        "Resolve later review conflicts on the merge-back branch while preserving the tagged version. Both branches are retained; remove temporary branches after appropriate integration."
    );
    if let Some(marker) = &policy.release_marker {
        println!(
            "After appropriate integration, manually fast-forward release marker {} to released tag {}.",
            text(presentation, marker),
            text(presentation, plan.tag.as_deref().unwrap())
        );
    }
    ExitCode::SUCCESS
}

fn observation(presentation: Presentation, value: &finish::RefObservation) -> String {
    match value {
        finish::RefObservation::Commit(oid) => display(presentation, oid),
        finish::RefObservation::Absent => "absent".into(),
        finish::RefObservation::Unavailable => "unavailable".into(),
        finish::RefObservation::Ambiguous => "ambiguous".into(),
    }
}
