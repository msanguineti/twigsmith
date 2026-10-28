//! Interactive direct integration orchestration and reports. Workflow libraries
//! retain all authority for local effects, delivery, checkout, and deletion.
use super::*;
use integration::{Blocker, Plan, RequiredInput, State, completion, execution, remote};
use std::{
    fs,
    process::{Command, Stdio},
};

enum Progress {
    Finished(ExitCode),
    Destination(Box<Plan>, String),
}

pub(super) fn run(
    directory: &Path,
    initial: Plan,
    presentation: Presentation,
    skip_checks: bool,
    mut context_shown: bool,
) -> ExitCode {
    let mut history = String::new();
    if let Some(policy) = &initial.settings.workstream
        && policy.version_tagging
    {
        let _ = writeln!(
            history,
            "Integration destinations (in order): {}. Version tag identity: {}.",
            text(presentation, policy.integration_targets.join(" -> ")),
            policy.version_tag_identity.as_str()
        );
    }
    let root = initial
        .repository
        .working_tree_root
        .clone()
        .unwrap_or_else(|| directory.to_path_buf());
    let mut next = initial;
    loop {
        match run_retained(
            &root,
            next,
            presentation,
            skip_checks,
            history,
            context_shown,
        ) {
            Progress::Finished(code) => return code,
            Progress::Destination(plan, retained_history) => {
                next = *plan;
                history = retained_history;
                context_shown = false;
            }
        }
    }
}

fn run_retained(
    directory: &Path,
    initial: Plan,
    presentation: Presentation,
    skip_checks: bool,
    mut history: String,
    mut context_shown: bool,
) -> Progress {
    let root = initial
        .repository
        .working_tree_root
        .clone()
        .expect("working repository");
    let Some(mut plan) = refresh(directory, &initial, presentation, &mut history) else {
        return Progress::Finished(ExitCode::FAILURE);
    };
    // Preparation may expose new choices on the actual graph. It occurs at most
    // once before those choices, then once to create/retain the integration result.
    let mut preparations = 0;
    loop {
        if !plan.blockers.is_empty() {
            for blocker in &plan.blockers {
                blocker_message(presentation, &mut history, blocker);
            }
            let observed = repository::inspect(&root);
            let report = execution::Report {
                plan: Some(plan.clone()),
                commands: vec![],
                effects: vec![],
                remaining: observed,
                error: None,
            };
            let _ = writeln!(history, "{}", text(presentation, report.guidance()));
            return Progress::Finished(failure(
                presentation,
                &history,
                "preparation/integration",
                &report.remaining,
            ));
        }
        if let Some(choice) = plan.required_input.first().copied() {
            if !context_shown && io::stdin().is_terminal() && io::stderr().is_terminal() {
                crate::presentation::section("Integrate work");
                crate::presentation::prose(&format!(
                    "Work: {}\nDeliver to: {}/{}",
                    display(
                        presentation,
                        plan.source_ref
                            .strip_prefix(b"refs/heads/")
                            .unwrap_or(&plan.source_ref)
                    ),
                    text(presentation, &plan.target.remote),
                    display(
                        presentation,
                        plan.target
                            .reference
                            .strip_prefix(b"refs/heads/")
                            .unwrap_or(&plan.target.reference)
                    )
                ));
                if let Some(preparation) =
                    plan.evidence.as_ref().and_then(|e| e.preparation.as_ref())
                {
                    crate::presentation::prose(&format!(
                        "Update from: {}/{}",
                        text(presentation, &plan.settings.base_remote),
                        display(
                            presentation,
                            preparation
                                .reference
                                .strip_prefix(b"refs/heads/")
                                .unwrap_or(&preparation.reference)
                        )
                    ));
                }
                context_shown = true;
            }
            let request = match choose(&root, &plan, choice, presentation) {
                Ok(request) => request,
                Err(error) => {
                    let _ = writeln!(history, "{error}");
                    return Progress::Finished(failure(
                        presentation,
                        &history,
                        "input",
                        &repository::inspect(&root),
                    ));
                }
            };
            plan = match integration::resolve(&root, &plan, &request) {
                Ok(plan) => plan,
                Err(error) => {
                    let _ = writeln!(history, "error: {}", text(presentation, error));
                    return Progress::Finished(failure(
                        presentation,
                        &history,
                        "input",
                        &repository::inspect(&root),
                    ));
                }
            };
            if plan.can_retain_choices() {
                plan = match integration::retain_choices(&root, &plan) {
                    Ok(plan) => plan,
                    Err(error) => {
                        let _ = writeln!(history, "error: {}", text(presentation, error));
                        return Progress::Finished(failure(
                            presentation,
                            &history,
                            "retaining input",
                            &repository::inspect(&root),
                        ));
                    }
                };
            }
            // Choosing published-work cleanup introduces another remote role.
            // Only refresh that newly required evidence after the input guard.
            if choice == RequiredInput::Cleanup && plan.cleanup == FinishCleanup::Both {
                let previous_evidence = plan.evidence.clone();
                let Some(refreshed) = refresh(&root, &plan, presentation, &mut history) else {
                    return Progress::Finished(ExitCode::FAILURE);
                };
                if previous_evidence
                    .as_ref()
                    .zip(refreshed.evidence.as_ref())
                    .is_none_or(|(old, new)| {
                        old.preparation != new.preparation
                            || old.target != new.target
                            || old.fetched != new.fetched
                            || old.targets != new.targets
                    })
                {
                    history.push_str("error: target observations changed while resolving cleanup; rerun finish.\n");
                    return Progress::Finished(failure(
                        presentation,
                        &history,
                        "input",
                        &repository::inspect(&root),
                    ));
                }
                plan = refreshed;
            }
            continue;
        }
        // An interrupted merge can leave a corroborated result before its pin
        // or record acknowledgement was written. Finish retaining that result
        // before either delivery or completion can acquire authority.
        if !plan.needs_result_retention()
            && !plan.needs_candidate_checkout()
            && (!integration::release::enabled(&plan)
                || integration::release::source_checked(&plan, skip_checks)
                || plan.source == finish::RefObservation::Absent)
        {
            if matches!(plan.state, State::Published | State::Complete)
                && (!integration::release::enabled(&plan)
                    || integration::release::candidate_checked(&plan, skip_checks)
                    || plan.source == finish::RefObservation::Absent)
            {
                return complete(&root, &plan, presentation, history, skip_checks);
            }
            if plan.state == State::PendingPublication
                || (integration::release::enabled(&plan)
                    && matches!(plan.state, State::Published | State::Complete))
            {
                break;
            }
        }
        if preparations == 2 {
            history.push_str("error: local integration did not reach a deliverable result; inspect retained state before retrying.\n");
            return Progress::Finished(failure(
                presentation,
                &history,
                "preparation/integration",
                &repository::inspect(&root),
            ));
        }
        preparations += 1;
        if plan.history_edit == HistoryEdit::Interactive
            && plan.integration == integration::Integration::AfterPreparation
            && let Some(evidence) = &plan.evidence
        {
            let boundary = evidence
                .preparation
                .as_ref()
                .map_or(&evidence.fetched, |p| &p.branch);
            if let finish::RefObservation::Commit(boundary) = boundary {
                history_edit_notice(presentation, &plan.source_ref, boundary);
            }
        }
        let mut output = CheckOutput::new(presentation);
        let prepared = execution::prepare_with_checks(&root, &plan, skip_checks, |event| {
            check_event(presentation, &mut output, event);
        });
        output.flush();
        for effect in &prepared.effects {
            if let execution::Effect::WorkChanged { before, after } = effect {
                println!(
                    "Prepared work: {} -> {}.",
                    observation(presentation, before),
                    observation(presentation, after)
                );
            }
        }
        preparation_effects(presentation, &mut history, &prepared);
        command_diagnostics(presentation, &mut history, &prepared.commands);
        // Reassessment after preparation can expose either a normal choice or
        // a concrete blocker. Let the same loop present both, rather than hiding
        // that assessment behind the executor's generic unresolved-plan error.
        let reassessed = matches!(prepared.error, Some(execution::Error::UnresolvedPlan))
            && prepared.plan.as_ref().is_some_and(|p| !p.is_ready());
        if let Some(error) = &prepared.error
            && !reassessed
        {
            let _ = writeln!(history, "error: {}", text(presentation, error));
            let _ = writeln!(history, "{}", text(presentation, prepared.guidance()));
            return Progress::Finished(failure(
                presentation,
                &history,
                "preparation/integration",
                &prepared.remaining,
            ));
        }
        let Some(next) = prepared.plan else {
            history.push_str("error: prepared state could not be assessed.\n");
            return Progress::Finished(failure(
                presentation,
                &history,
                "preparation/integration",
                &prepared.remaining,
            ));
        };
        plan = next;
    }
    let mut output = CheckOutput::new(presentation);
    let delivered =
        integration::publication::publish_with_checks(&root, &plan, skip_checks, |event| {
            check_event(presentation, &mut output, event);
        });
    output.flush();
    let cache_issues = crate::cli::cache_issues(
        &delivered.caches,
        |bytes| display(presentation, bytes),
        true,
    );
    eprint!("{cache_issues}");
    let _ = writeln!(history, "Target delivery: {:?}.", delivered.state);
    for destination in &delivered.destinations {
        let _ = writeln!(
            history,
            "  {}: {}",
            display(presentation, &destination.destination),
            match destination.state {
                integration::publication::DestinationState::NotAttempted => "not attempted",
                integration::publication::DestinationState::Advanced => "advanced",
                integration::publication::DestinationState::Unchanged =>
                    "unchanged; delivery confirmed",
                integration::publication::DestinationState::Rejected => "rejected",
                integration::publication::DestinationState::Unconfirmed => "unconfirmed",
            }
        );
        if let Some(error) = &destination.error {
            let _ = writeln!(history, "  error: {}", text(presentation, error));
        }
        if !destination.detail.is_empty() {
            let _ = writeln!(
                history,
                "  Git: {}",
                display(presentation, &destination.detail)
            );
        }
        command_diagnostics(presentation, &mut history, &destination.commands);
    }
    if let Some(error) = &delivered.checks.error {
        let _ = writeln!(history, "Checks stopped: {}", text(presentation, error));
    }
    if !delivered.is_complete() {
        if let Some(error) = &delivered.error {
            let _ = writeln!(history, "error: {}", text(presentation, error));
        }
        let stage = if delivered.checks.error.is_some() {
            "checks"
        } else {
            "target delivery"
        };
        return Progress::Finished(failure(presentation, &history, stage, &delivered.remaining));
    }
    match delivered.plan {
        Ok(plan) => complete(&root, &plan, presentation, history, skip_checks),
        Err(error) => {
            let _ = writeln!(history, "error: {}", text(presentation, error));
            Progress::Finished(failure(
                presentation,
                &history,
                "post-delivery assessment",
                &delivered.remaining,
            ))
        }
    }
}

fn refresh(
    directory: &Path,
    plan: &Plan,
    presentation: Presentation,
    history: &mut String,
) -> Option<Plan> {
    let report = remote::refresh(directory, plan);
    // Successful exact-ref observations and object acquisition are backend
    // evidence. Failure diagnostics below retain the useful Git command output.
    command_diagnostics(presentation, history, &report.commands);
    if let Some(error) = &report.error {
        let _ = writeln!(history, "error: {}", text(presentation, error));
        failure(presentation, history, "remote refresh", &report.remaining);
        return None;
    }
    if report.plan.is_none() {
        history.push_str("error: refreshed integration state is unavailable.\n");
        failure(presentation, history, "remote refresh", &report.remaining);
    }
    report.plan
}

fn choose(
    root: &Path,
    plan: &Plan,
    choice: RequiredInput,
    presentation: Presentation,
) -> Result<integration::Request, String> {
    let guidance = match choice {
        RequiredInput::Update => "select --update <rebase|merge|none> or configure finish-update",
        RequiredInput::Integration => {
            "select --integration <ff-only|merge|squash> or configure finish-integration"
        }
        RequiredInput::Message => "supply -m/--message <text> or configure finish-message = auto",
        RequiredInput::Cleanup => "select --cleanup <keep|local|both> or configure finish-cleanup",
        RequiredInput::TagName => "supply --tag <name>",
        RequiredInput::Rewrite => "select --allow-rewrite or --no-rewrite",
    };
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(format!(
            "error: unresolved {} input; terminal input and stderr are required; {guidance}.",
            match choice {
                RequiredInput::Update => "update",
                RequiredInput::Integration => "integration",
                RequiredInput::Message => "message",
                RequiredInput::Cleanup => "cleanup",
                RequiredInput::TagName => "version tag",
                RequiredInput::Rewrite => "published-history replacement",
            }
        ));
    }
    let mut request = plan.request().clone();
    if choice == RequiredInput::TagName {
        request.release.tag = Some(read_reference_name(
            root,
            presentation,
            "What version tag should identify this release?",
            "refs/tags/",
            "Finish cancelled; retained work remains available.",
        )?);
        return Ok(request);
    }
    if choice == RequiredInput::Message {
        request.message = Some(edit_message(root, plan, presentation)?);
        return Ok(request);
    }
    let (question, choices): (&str, &[(&str, &str)]) = match choice {
        RequiredInput::Rewrite => {
            crate::presentation::prose(
                "Publishing the updated work may replace commits already on the remote work branch.",
            );
            (
                "Allow replacing published work history?",
                &[
                    (
                        "allow",
                        "Allow replacement for this finish; stop if remote history changes",
                    ),
                    (
                        "deny",
                        "Keep published history; stop if replacement is required",
                    ),
                    ("cancel", "Cancel"),
                ],
            )
        }
        RequiredInput::Update => (
            "How should the work branch be updated?",
            &[
                ("rebase", "Rebase work commits onto the update source"),
                ("merge", "Merge the update source into the work branch"),
                ("none", "Keep current work history"),
                ("cancel", "Cancel"),
            ],
        ),
        RequiredInput::Integration => (
            "How should the work be integrated into the target?",
            &[
                (
                    "ff-only",
                    "Fast-forward the target; require compatible history",
                ),
                ("merge", "Create a merge commit on the target"),
                ("squash", "Create one target commit from the work changes"),
                ("cancel", "Cancel"),
            ],
        ),
        RequiredInput::Cleanup => (
            "Which work branches should remain after delivery?",
            &[
                ("keep", "Keep local and published work branches"),
                (
                    "local",
                    "Delete the local work branch after verified delivery",
                ),
                (
                    "both",
                    "Delete local and published work branches after verified delivery",
                ),
                ("cancel", "Cancel"),
            ],
        ),
        RequiredInput::Message | RequiredInput::TagName => unreachable!(),
    };
    let answer = super::read_menu_answer(
        presentation,
        question,
        choices,
        "Finish cancelled; completed effects and retained work remain available.",
    )?;
    let invalid = || format!("error: invalid choice; rerun finish and {guidance}.");
    match choice {
        RequiredInput::Rewrite => {
            request.rewrite = Some(match answer.as_str() {
                "allow" => FinishRewrite::Allow,
                "deny" => FinishRewrite::Deny,
                _ => return Err(invalid()),
            });
        }
        RequiredInput::Update => {
            request.update = Some(match answer.as_str() {
                "rebase" => FinishUpdate::Rebase,
                "merge" => FinishUpdate::Merge,
                "none" => FinishUpdate::None,
                _ => return Err(invalid()),
            })
        }
        RequiredInput::Integration => {
            request.integration = Some(match answer.as_str() {
                "ff-only" => FinishIntegration::FfOnly,
                "merge" => FinishIntegration::Merge,
                "squash" => FinishIntegration::Squash,
                _ => return Err(invalid()),
            })
        }
        RequiredInput::Cleanup => {
            request.cleanup = Some(match answer.as_str() {
                "keep" => FinishCleanup::Keep,
                "local" => FinishCleanup::Local,
                "both" => FinishCleanup::Both,
                _ => return Err(invalid()),
            })
        }
        RequiredInput::Message | RequiredInput::TagName => unreachable!(),
    }
    Ok(request)
}

fn edit_message(root: &Path, plan: &Plan, presentation: Presentation) -> Result<String, String> {
    eprintln!(
        "Edit the integration message with your Git editor. Save and exit to accept; delete all message text or exit unsuccessfully to cancel. Use -m/--message to supply text without editing."
    );
    let fail = |error: &dyn std::fmt::Display| {
        format!(
            "error: integration message editing failed: {}; supply -m/--message or rerun finish.",
            text(presentation, error)
        )
    };
    let editor = crate::git::inspect(root, ["var", "GIT_EDITOR"]).map_err(|e| fail(&e))?;
    if !editor.status.success() {
        return Err(fail(&display(presentation, &editor.stderr)));
    }
    let editor = String::from_utf8(editor.stdout).map_err(|e| fail(&e))?;
    let editor = editor.strip_suffix('\n').unwrap_or(&editor);
    if editor.trim().is_empty() || editor.contains('\0') {
        return Err(fail(&"Git selected an empty or invalid editor"));
    }
    integration::revalidate(root, plan, plan.evidence.as_ref()).map_err(|e| fail(&e))?;
    let mut file = tempfile::Builder::new()
        .prefix("twigsmith-message-")
        .tempfile_in(&plan.repository.git_dir)
        .map_err(|e| fail(&e))?;
    writeln!(file, "{}", plan.message_suggestion()).map_err(|e| fail(&e))?;
    file.flush().map_err(|e| fail(&e))?;
    // Git editor values are shell commands. Only that configured value is shell
    // syntax; the pathname is a positional argument and message bytes never enter
    // the command. The temporary file also permits editor atomic-save replacement.
    // This launcher requires POSIX sh on the inherited PATH, independently of
    // Git's own platform shell resolution. Windows path/quoting is unvalidated.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg("twigsmith-editor")
        .arg(file.path())
        .current_dir(root)
        .status()
        .map_err(|e| fail(&e))?;
    if !status.success() {
        return Err(fail(&status));
    }
    integration::revalidate(root, plan, plan.evidence.as_ref()).map_err(|e| fail(&e))?;
    if !fs::symlink_metadata(file.path())
        .map_err(|e| fail(&e))?
        .file_type()
        .is_file()
    {
        return Err(fail(&"the editor did not leave a regular message file"));
    }
    let bytes = fs::read(file.path()).map_err(|e| fail(&e))?;
    let raw = String::from_utf8(bytes).map_err(|e| fail(&e))?;
    if raw.contains('\0') {
        return Err(fail(&"the message contains NUL"));
    }
    let message = cleanup_message(root, raw).map_err(|e| fail(&e))?;
    if message.trim().is_empty() {
        return Err("error: integration message is blank; finish cancelled. Supply -m/--message or rerun finish.".into());
    }
    Ok(message)
}

fn config_value(root: &Path, key: &str) -> Result<Option<String>, String> {
    let output = crate::git::inspect(root, ["config", "--get", key]).map_err(|e| e.to_string())?;
    match output.status.code() {
        Some(1) => Ok(None),
        Some(0) => String::from_utf8(output.stdout)
            .map(|v| Some(v.strip_suffix('\n').unwrap_or(&v).to_owned()))
            .map_err(|e| e.to_string()),
        _ => Err(String::from_utf8_lossy(&output.stderr).into_owned()),
    }
}

// These keys are ordered aliases in Git; the last effective occurrence wins.
fn comment_prefix(root: &Path) -> Result<String, String> {
    let output = crate::git::inspect(
        root,
        [
            "config",
            "--null",
            "--get-regexp",
            r"^core\.(commentchar|commentstring)$",
        ],
    )
    .map_err(|e| e.to_string())?;
    if output.status.code() == Some(1) {
        return Ok("#".into());
    }
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let entry = output
        .stdout
        .split(|byte| *byte == 0)
        .rfind(|entry| !entry.is_empty())
        .ok_or("Git returned no comment configuration")?;
    let separator = entry
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or("Git returned malformed comment configuration")?;
    let value = String::from_utf8(entry[separator + 1..].to_vec()).map_err(|e| e.to_string())?;
    // Git chooses auto's marker before editing. Our generated suggestion has no
    // comment lines, so its first available marker is '#'.
    Ok(
        if &entry[..separator] == b"core.commentchar" && value == "auto" {
            "#".into()
        } else {
            value
        },
    )
}

fn cleanup_message(root: &Path, mut raw: String) -> Result<String, String> {
    let cleanup = config_value(root, "commit.cleanup")?.unwrap_or_else(|| "default".into());
    if cleanup == "verbatim" {
        return Ok(raw);
    }
    if cleanup == "scissors" {
        let comment = comment_prefix(root)?;
        let marker = format!("{comment} ------------------------ >8 ------------------------");
        if let Some(start) = raw
            .split_inclusive('\n')
            .scan(0, |offset, line| {
                let start = *offset;
                *offset += line.len();
                Some((start, line))
            })
            .find_map(|(offset, line)| {
                (line.trim_end_matches(['\r', '\n']) == marker).then_some(offset)
            })
        {
            raw.truncate(start);
        }
    }
    let mut command = Command::new("git");
    command.current_dir(root).arg("stripspace");
    match cleanup.as_str() {
        "default" | "strip" => {
            command.arg("--strip-comments");
        }
        "whitespace" | "scissors" => {}
        _ => return Err(format!("invalid commit.cleanup value: {cleanup}")),
    }
    // A file avoids a pipe deadlock for large messages while collecting Git's output.
    use std::io::{Seek, SeekFrom};
    let mut input = tempfile::tempfile().map_err(|e| e.to_string())?;
    input.write_all(raw.as_bytes()).map_err(|e| e.to_string())?;
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let output = command
        .stdin(Stdio::from(input))
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn check_event(presentation: Presentation, output: &mut CheckOutput, event: checks::Event<'_>) {
    match event {
        checks::Event::Started {
            index,
            total,
            check,
        } => eprintln!(
            "Check {}/{}: {} — {}",
            index + 1,
            total,
            text(presentation, &check.name),
            text(
                presentation,
                serde_json::to_string(&check.command).expect("command JSON")
            )
        ),
        checks::Event::Output { stream, bytes } => output.write(stream, bytes),
        checks::Event::Passed { index } => {
            output.flush();
            eprintln!("Check {} passed.", index + 1);
        }
        checks::Event::Skipped { count } => {
            eprintln!("Skipped {count} configured finish check(s) (--skip-checks).")
        }
    }
}

fn complete(
    root: &Path,
    plan: &Plan,
    presentation: Presentation,
    mut history: String,
    skip_checks: bool,
) -> Progress {
    if plan.has_remaining_destinations() {
        let mut commands = Vec::new();
        let next = integration::advance(root, plan, &mut commands);
        command_diagnostics(presentation, &mut history, &commands);
        return match next {
            Ok(next) => {
                for (reference, result) in next.completed_destinations() {
                    let _ = writeln!(
                        history,
                        "Verified completed destination {} at {}.",
                        display(presentation, reference),
                        display(presentation, result)
                    );
                }
                Progress::Destination(Box::new(next), history)
            }
            Err(error) => {
                let _ = writeln!(history, "error: {}", text(presentation, error));
                Progress::Finished(failure(
                    presentation,
                    &history,
                    "next integration destination",
                    &repository::inspect(root),
                ))
            }
        };
    }
    let mut commands = Vec::new();
    let mut caches = Vec::new();
    // Ordinary work has no version ref to deliver. Completion performs its
    // own entry and delivery checks before any checkout or cleanup.
    let delivered = if integration::release::enabled(plan) {
        integration::release::deliver_with_caches(
            root,
            plan,
            skip_checks,
            &mut commands,
            &mut caches,
        )
    } else {
        Ok(plan.clone())
    };
    eprint!(
        "{}",
        crate::cli::cache_issues(&caches, |bytes| display(presentation, bytes), true)
    );
    command_diagnostics(presentation, &mut history, &commands);
    let release_destinations = integration::release::delivery_observations(plan, &commands);
    for destination in &release_destinations {
        let _ = writeln!(
            history,
            "Release delivery at {}: tag {}; marker {}.",
            display(presentation, &destination.destination),
            observation(presentation, &destination.tag),
            destination
                .marker
                .as_ref()
                .map(|v| observation(presentation, v))
                .unwrap_or_else(|| "disabled".into())
        );
    }
    let plan = match delivered {
        Ok(plan) => plan,
        Err(error) => {
            let _ = writeln!(history, "error: {}", text(presentation, error));
            return Progress::Finished(failure(
                presentation,
                &history,
                "version tag/marker delivery",
                &repository::inspect(root),
            ));
        }
    };
    let plan = &plan;
    let report = completion::complete(root, plan);
    eprint!(
        "{}",
        crate::cli::cache_issues(&report.caches, |bytes| display(presentation, bytes), true)
    );
    for effect in &report.effects {
        match effect {
            completion::Effect::CheckedOut { after, .. } => {
                let _ = writeln!(history, "Final checkout: {}.", head(presentation, after));
            }
            completion::Effect::SourceEvidenceRetained => {}
            completion::Effect::LocalWorkDeleted {
                reference,
                expected,
            } => {
                let _ = writeln!(
                    history,
                    "Deleted local work {} at {}.",
                    display(presentation, reference),
                    display(presentation, expected)
                );
            }
        }
    }
    command_diagnostics(presentation, &mut history, &report.commands);
    for destination in &report.destinations {
        let _ = writeln!(
            history,
            "Published-work cleanup at {}: {}.",
            display(presentation, &destination.destination),
            match destination.state {
                completion::DestinationState::NotAttempted => "not attempted",
                completion::DestinationState::Deleted => "deleted",
                completion::DestinationState::Absent => "already absent",
                completion::DestinationState::Rejected => "rejected",
                completion::DestinationState::Unconfirmed => "unconfirmed",
            }
        );
        if let Some(error) = &destination.error {
            let _ = writeln!(history, "  error: {}", text(presentation, error));
        }
        command_diagnostics(presentation, &mut history, &destination.commands);
    }
    if !report.is_complete() {
        let blocked = if let Ok(plan) = &report.plan {
            for blocker in &plan.blockers {
                blocker_message(presentation, &mut history, blocker);
            }
            !plan.blockers.is_empty()
        } else {
            false
        };
        if let Some(error) = &report.error
            && !(blocked && matches!(error, completion::Error::UnverifiedAttempt))
        {
            let _ = writeln!(history, "error: {}", text(presentation, error));
        }
        if let Err(error) = &report.plan {
            let _ = writeln!(
                history,
                "Completion assessment unavailable: {}",
                text(presentation, error)
            );
        }
        for (label, value) in [("Work ref", &report.source), ("Target ref", &report.target)] {
            match value {
                Ok(value) => {
                    let _ = writeln!(history, "{label}: {}.", observation(presentation, value));
                }
                Err(error) => {
                    let _ = writeln!(
                        history,
                        "{label} observation unavailable: {}",
                        text(presentation, error)
                    );
                }
            }
        }
        let _ = writeln!(history, "{}", text(presentation, report.guidance()));
        return Progress::Finished(failure(
            presentation,
            &history,
            "checkout/cleanup",
            &report.remaining,
        ));
    }
    for (reference, result) in plan.completed_destinations() {
        println!(
            "Integrated {} at {}.",
            display(presentation, reference),
            display(presentation, result)
        );
    }
    if let Some((tag, source)) = integration::release::identity(plan) {
        println!(
            "Released {} at {}.",
            text(presentation, tag),
            display(presentation, source)
        );
        for destination in &release_destinations {
            println!(
                "  {}: tag {}{}.",
                display(presentation, &destination.destination),
                observation(presentation, &destination.tag),
                destination
                    .marker
                    .as_ref()
                    .map(|v| format!("; marker {}", observation(presentation, v)))
                    .unwrap_or_default()
            );
        }
        if let Some(marker) = plan
            .settings
            .workstream
            .as_ref()
            .and_then(|p| p.release_marker.as_ref())
        {
            println!(
                "Release marker {}: {} (delivery confirmed).",
                text(presentation, marker),
                display(presentation, source)
            );
        }
    }
    if !plan.completed_destinations().is_empty() || integration::release::identity(plan).is_some() {
        println!();
    }
    crate::presentation::report(
        &format!(
            "Integrated {} at {} into {}/{}.",
            display(presentation, &plan.source_ref),
            display(
                presentation,
                plan.result.as_deref().expect("verified result")
            ),
            text(presentation, &plan.target.remote),
            display(
                presentation,
                plan.target
                    .reference
                    .strip_prefix(b"refs/heads/")
                    .unwrap_or(&plan.target.reference)
            )
        ),
        true,
    );
    for destination in &report.targets {
        println!(
            "  {}: delivery confirmed at {}.",
            display(presentation, &destination.destination),
            observation(presentation, &destination.branch)
        );
    }
    if !report.destinations.is_empty() {
        println!();
    }
    for destination in &report.destinations {
        println!(
            "  Published work at {}: {}.",
            display(presentation, &destination.destination),
            match destination.state {
                completion::DestinationState::Deleted => "deleted",
                completion::DestinationState::Absent => "already absent",
                _ => unreachable!("complete cleanup"),
            }
        );
    }
    if let Ok(Observation::Repository(state)) = &report.remaining {
        println!("\nFinal checkout: {}.", head(presentation, &state.head));
    }
    println!(
        "Work branches: {}.",
        match plan.cleanup {
            FinishCleanup::Keep => "local and published retained",
            FinishCleanup::Local => "local deleted; published retained",
            FinishCleanup::Both => "local and published deleted or already absent",
            FinishCleanup::Ask => unreachable!("resolved cleanup"),
        }
    );
    Progress::Finished(ExitCode::SUCCESS)
}

fn failure(
    presentation: Presentation,
    history: &str,
    stage: &str,
    observed: &Result<Observation, InspectionError>,
) -> ExitCode {
    let mut output = format!(
        "Finish integration incomplete during {stage}.\nResolve the reported problem, then rerun finish --mode integrate.\n\n{history}"
    );
    if let Ok(Observation::Repository(state)) = observed
        && let Some(tree) = &state.working_tree
        && let Ok(plan) = integration::assess(&tree.root, &integration::Request::default(), None)
    {
        match integration::release::status(&tree.root, &plan) {
            Ok(Some(release)) => {
                let selected_commit = if release.source.is_empty()
                    || (integration::release::integrated_identity(&plan)
                        && plan.result.is_none()
                        && release.tag_object == finish::RefObservation::Absent)
                {
                    "pending validated version result".into()
                } else {
                    display(presentation, &release.source)
                };
                let _ = writeln!(
                    output,
                    "Version tag: {}; selected commit: {}; local tag object: {}.",
                    text(presentation, release.tag.as_deref().unwrap_or("unresolved")),
                    selected_commit,
                    observation(presentation, &release.tag_object)
                );
                let _ = writeln!(
                    output,
                    "Integration destination: {}/{}; result: {}.",
                    text(presentation, &plan.target.remote),
                    display(presentation, &plan.target.reference),
                    plan.result
                        .as_deref()
                        .map(|v| display(presentation, v))
                        .unwrap_or_else(|| "not yet verified".into())
                );
                for (reference, result) in plan.completed_destinations() {
                    let _ = writeln!(
                        output,
                        "Verified completed destination {} at {}.",
                        display(presentation, reference),
                        display(presentation, result)
                    );
                }
                if let Some(policy) = &plan.settings.workstream {
                    let _ = writeln!(
                        output,
                        "Required destinations (in order): {}. Current destination: {} of {}.",
                        text(presentation, policy.integration_targets.join(" -> ")),
                        plan.active_destination_index() + 1,
                        policy.integration_targets.len()
                    );
                }
                if let Some((name, value)) = release.marker {
                    let _ = writeln!(
                        output,
                        "Release marker {}: {} locally; required endpoint delivery remains subject to verification.",
                        text(presentation, name),
                        observation(presentation, &value)
                    );
                }
                if release.source_checks_bypassed || release.candidate_checks_bypassed {
                    output.push_str("Retained check evidence includes an explicit --skip-checks bypass; pending delivery without that flag requires the applicable checks.\n");
                }
            }
            Err(error) => {
                let _ = writeln!(
                    output,
                    "Release observation unavailable: {}.",
                    text(presentation, error)
                );
            }
            Ok(None) => {}
        }
    }

    remaining_state(presentation, &mut output, observed, false);
    output.push_str("\nCompleted effects and retained work remain available; retries complete the pending steps.\n");
    crate::presentation::report(&output, false);
    ExitCode::FAILURE
}

fn head(presentation: Presentation, value: &Head) -> String {
    match value {
        Head::Attached { branch, commit } => format!(
            "{} at {}",
            display(presentation, branch),
            display(presentation, commit)
        ),
        Head::Detached { commit } => format!("detached at {}", display(presentation, commit)),
        Head::Unborn { branch } => format!("unborn {}", display(presentation, branch)),
    }
}
fn observation(presentation: Presentation, value: &finish::RefObservation) -> String {
    match value {
        finish::RefObservation::Commit(oid) => display(presentation, oid),
        finish::RefObservation::Absent => "absent".into(),
        finish::RefObservation::Unavailable => "unavailable".into(),
        finish::RefObservation::Ambiguous => "ambiguous".into(),
    }
}
fn preparation_effects(
    presentation: Presentation,
    output: &mut String,
    report: &execution::Report,
) {
    for effect in &report.effects {
        use execution::Effect::*;
        match effect {
            Recorded(_) | Retained { .. } => {}
            WorkChanged { before, after } => {
                let _ = writeln!(
                    output,
                    "Prepared work: {} -> {}.",
                    observation(presentation, before),
                    observation(presentation, after)
                );
            }
            TargetChanged { before, after } => {
                let _ = writeln!(
                    output,
                    "Local target: {} -> {}.",
                    observation(presentation, before),
                    observation(presentation, after)
                );
            }
            CheckedOut { reference } => {
                let _ = writeln!(output, "Checked out {}.", display(presentation, reference));
            }
            Integrated { commit } => {
                let _ = writeln!(
                    output,
                    "Retained integration result: {}.",
                    display(presentation, commit)
                );
            }
            Reused { commit } => {
                let _ = writeln!(
                    output,
                    "Reused integration result: {}.",
                    display(presentation, commit)
                );
            }
        }
    }
}
fn blocker_message(presentation: Presentation, output: &mut String, blocker: &Blocker) {
    let message = match blocker {
        Blocker::FreshRemoteEvidenceRequired => "fresh target observations are required; rerun finish".into(),
        Blocker::InvalidEvidence => "remote observations do not match the selected attempt; inspect destinations and retry".into(),
        Blocker::TargetUnavailable => "the target branch is unavailable at its configured remote".into(),
        Blocker::HistoryUnavailable => "required commit history is unavailable; obtain the history before retrying".into(),
        Blocker::UnrelatedHistory => "work and target have unrelated histories".into(),
        Blocker::FastForwardRequired => "ff-only requires the target to be an ancestor of the work; choose a work update or another integration method".into(),
        Blocker::LocalTargetWork => "the local target contains work outside the remote target; preserve and reconcile it before retrying".into(),
        Blocker::TargetInAnotherWorktree => "the target is checked out in another worktree".into(),
        Blocker::SourceInAnotherWorktree => "the work branch is checked out in another worktree".into(),
        Blocker::SymbolicReference => "a source or target reference is symbolic; inspect the branch refs".into(),
        Blocker::DirtyTree => "the index or working tree contains changes; commit or preserve them before retrying".into(),
        Blocker::GitOperation => "a Git operation is active; complete or abandon that operation before retrying".into(),
        Blocker::GraftedHistory => "grafted history cannot establish integration authority".into(),
        Blocker::CheckoutCleanupConflict => "checkout work is incompatible with cleanup local or both; select --checkout target or --cleanup keep".into(),
        Blocker::TargetChanged(destination) => format!("target destinations disagree or changed at {}; inspect the target before retrying", display(presentation, destination)),
        Blocker::WorkBranchChanged(destination) => format!("published work changed or is not incorporated at {}; retain it and inspect the work branch", display(presentation, destination)),
        Blocker::Release(problem) => problem.clone(),
        Blocker::HistoryEdit(problem) => problem.clone(),
        Blocker::Recovery(problem) => {
            use integration::recovery::Problem;
            match problem {
                Problem::MissingRecord => "the pending finish record is missing; retain the current branches and inspect the earlier attempt".into(),
                Problem::MalformedRecord | Problem::UnsafeRecord => "the pending finish record cannot be read safely; inspect the repository's private finish metadata and retain the current branches".into(),
                Problem::UnassociatedHead => "the current checkout no longer identifies the pending finish; return to its unchanged work branch before retrying".into(),
                Problem::ChangedBinding => "settings, Git configuration, destinations, or attempt choices changed; restore the intended context before retrying".into(),
                Problem::ChangedSource => "the work branch or its history changed after preparation; retain that work and inspect its Git reflog before retrying".into(),
                Problem::ChangedTarget => "the integration target changed outside the pending finish; retain the candidate and reconcile the target before retrying".into(),
                Problem::MissingHistory => "commit or reflog history needed to verify the pending finish is missing; restore that history before retrying".into(),
                Problem::InvalidResult => "the current Git history cannot verify the prepared or integrated result; inspect the branch history and retained commits before retrying".into(),
            }
        },
    };
    let _ = writeln!(output, "error: {message}.");
}
