//! Finish grammar, terminal input, and presentation. Plans decide which choices
//! are relevant; preparation and publication own all workflow effects and guards.
use crate::{
    finish::{
        self, Constraint, FinishError, Plan, Request, RequiredInput, checks, integration,
        preparation, publication,
    },
    repository::{self, Head, InspectionError, Observation},
    settings::{
        FinishCheckout, FinishCleanup, FinishIntegration, FinishMode, FinishRewrite, FinishUpdate,
        HistoryEdit, VersionTagIdentity,
    },
    status::display as terminal_display,
};
use std::{
    env,
    ffi::OsString,
    fmt::Write as _,
    io::{self, IsTerminal, Write as _},
    path::Path,
    process::ExitCode,
};

#[path = "finish_cli_hosted.rs"]
mod hosted_cli;
#[path = "finish_cli_integration.rs"]
mod integration_cli;

#[derive(Clone, Copy)]
struct Presentation {
    show_credentials: bool,
}

fn resolve_presentation(
    show_credentials_flag: bool,
    redact_credentials: Option<bool>,
) -> Presentation {
    Presentation {
        show_credentials: show_credentials_flag || redact_credentials == Some(false),
    }
}

/// Render finish data for people. URL bytes in workflow plans remain untouched.
fn display(presentation: Presentation, bytes: &[u8]) -> String {
    let rendered = terminal_display(bytes);
    if presentation.show_credentials {
        rendered
    } else {
        redact_url_userinfo(&rendered)
    }
}

fn text(presentation: Presentation, value: impl std::fmt::Display) -> String {
    display(presentation, value.to_string().as_bytes())
}

fn redact_url_userinfo(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        let start = if bytes[cursor..].starts_with(b"https://") {
            Some(cursor + 8)
        } else if bytes[cursor..].starts_with(b"http://") {
            Some(cursor + 7)
        } else {
            None
        };
        let Some(authority_start) = start else {
            let ch = value[cursor..]
                .chars()
                .next()
                .expect("valid character boundary");
            output.push(ch);
            cursor += ch.len_utf8();
            continue;
        };
        output.push_str(&value[cursor..authority_start]);
        let authority_end = bytes[authority_start..]
            .iter()
            .position(|byte| matches!(byte, b'/' | b'?' | b'#' | b' ' | b'\n' | b'\r' | b'\t'))
            .map_or(bytes.len(), |offset| authority_start + offset);
        let authority = &value[authority_start..authority_end];
        if let Some(at) = authority.rfind('@') {
            output.push_str("<redacted>@");
            output.push_str(&authority[at + 1..]);
        } else {
            output.push_str(authority);
        }
        cursor = authority_end;
    }
    output
}
#[derive(Debug, Default)]
struct Completion {
    mode: Option<FinishMode>,
    integration: Option<FinishIntegration>,
    message: Option<String>,
    checkout: Option<FinishCheckout>,
    cleanup: Option<FinishCleanup>,
    release: integration::release::Request,
    merge_back: Option<String>,
}
impl Completion {
    fn validate(&self, mode: FinishMode) -> Result<(), &'static str> {
        if mode == FinishMode::Review
            && (self.integration.is_some()
                || self.message.is_some()
                || self.checkout.is_some()
                || self.cleanup.is_some()
                || !self.release.is_empty())
        {
            return Err(
                "--integration, --message, --checkout, --cleanup, and tag options require integrate mode",
            );
        }
        if self.checkout == Some(FinishCheckout::Work)
            && matches!(
                self.cleanup,
                Some(FinishCleanup::Local | FinishCleanup::Both)
            )
        {
            return Err(
                "--checkout work is incompatible with --cleanup local or both; select --checkout target or --cleanup keep",
            );
        }
        Ok(())
    }
}
fn parse(args: &[OsString]) -> Result<(Request, Completion, bool, bool), &'static str> {
    let mut request = Request::default();
    let mut completion = Completion::default();
    let mut show_credentials = false;
    let mut skip_checks = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--skip-checks") if !skip_checks => skip_checks = true,
            Some("--skip-checks") => return Err("duplicate --skip-checks"),
            Some("--show-credentials") if !show_credentials => show_credentials = true,
            Some("--show-credentials") => return Err("duplicate --show-credentials"),
            Some("--mode") => {
                if completion.mode.is_some() {
                    return Err("duplicate --mode");
                }
                completion.mode = Some(match args.next().and_then(|arg| arg.to_str()) {
                    Some("review") => FinishMode::Review,
                    Some("integrate") => FinishMode::Integrate,
                    _ => return Err("--mode requires review or integrate"),
                });
            }
            Some("--integration") => {
                if completion.integration.is_some() {
                    return Err("duplicate --integration");
                }
                completion.integration = Some(match args.next().and_then(|arg| arg.to_str()) {
                    Some("ff-only") => FinishIntegration::FfOnly,
                    Some("merge") => FinishIntegration::Merge,
                    Some("squash") => FinishIntegration::Squash,
                    _ => return Err("--integration requires ff-only, merge, or squash"),
                });
            }
            Some("--checkout") => {
                if completion.checkout.is_some() {
                    return Err("duplicate --checkout");
                }
                completion.checkout = Some(match args.next().and_then(|arg| arg.to_str()) {
                    Some("target") => FinishCheckout::Target,
                    Some("work") => FinishCheckout::Work,
                    _ => return Err("--checkout requires target or work"),
                });
            }
            Some("--cleanup") => {
                if completion.cleanup.is_some() {
                    return Err("duplicate --cleanup");
                }
                completion.cleanup = Some(match args.next().and_then(|arg| arg.to_str()) {
                    Some("keep") => FinishCleanup::Keep,
                    Some("local") => FinishCleanup::Local,
                    Some("both") => FinishCleanup::Both,
                    _ => return Err("--cleanup requires keep, local, or both"),
                });
            }
            Some("-m" | "--message") => {
                if completion.message.is_some() {
                    return Err("duplicate --message/-m");
                }
                let Some(message) = args
                    .next()
                    .and_then(|arg| arg.to_str())
                    .filter(|value| !value.trim().is_empty() && !value.contains('\0'))
                else {
                    return Err("--message/-m requires nonblank text");
                };
                completion.message = Some(message.into());
            }
            Some("--merge-back") => {
                if completion.merge_back.is_some() {
                    return Err("duplicate --merge-back");
                }
                completion.merge_back = Some(
                    args.next()
                        .and_then(|v| v.to_str())
                        .filter(|v| !v.is_empty())
                        .ok_or("--merge-back requires an exact branch name")?
                        .into(),
                );
            }
            Some("--tag") => {
                if completion.release.tag.is_some() {
                    return Err("duplicate --tag");
                }
                completion.release.tag = Some(
                    args.next()
                        .and_then(|v| v.to_str())
                        .filter(|v| !v.is_empty())
                        .ok_or("--tag requires an explicit version tag name")?
                        .into(),
                );
            }
            Some("--tag-message") => {
                if completion.release.message.is_some() {
                    return Err("duplicate --tag-message");
                }
                completion.release.message = Some(
                    args.next()
                        .and_then(|v| v.to_str())
                        .filter(|v| !v.trim().is_empty() && !v.contains('\0'))
                        .ok_or("--tag-message requires nonblank text")?
                        .into(),
                );
            }
            Some("--sign-tag") => {
                if completion.release.sign {
                    return Err("duplicate --sign-tag");
                }
                completion.release.sign = true;
            }
            Some("--update") => {
                if request.update.is_some() {
                    return Err("duplicate --update");
                }
                request.update = Some(match args.next().and_then(|arg| arg.to_str()) {
                    Some("rebase") => FinishUpdate::Rebase,
                    Some("merge") => FinishUpdate::Merge,
                    Some("none") => FinishUpdate::None,
                    _ => return Err("--update requires rebase, merge, or none"),
                });
            }
            Some("--history-edit") => {
                if request.history_edit.is_some() {
                    return Err("duplicate --history-edit");
                }
                request.history_edit = Some(match args.next().and_then(|arg| arg.to_str()) {
                    Some("none") => HistoryEdit::None,
                    Some("interactive") => HistoryEdit::Interactive,
                    _ => return Err("--history-edit requires none or interactive"),
                });
            }
            Some("--allow-rewrite" | "--no-rewrite") => {
                if request.rewrite.is_some() {
                    return Err("rewrite flags must appear once and are mutually exclusive");
                }
                request.rewrite = Some(if arg == "--allow-rewrite" {
                    FinishRewrite::Allow
                } else {
                    FinishRewrite::Deny
                });
            }
            _ => return Err("unknown or malformed finish argument"),
        }
    }
    if completion.checkout == Some(FinishCheckout::Work)
        && matches!(
            completion.cleanup,
            Some(FinishCleanup::Local | FinishCleanup::Both)
        )
    {
        return Err("--checkout work is incompatible with local cleanup");
    }
    if completion.mode == Some(FinishMode::Review)
        && (completion.message.is_some()
            || completion.checkout == Some(FinishCheckout::Target)
            || completion.cleanup.is_some_and(|v| v != FinishCleanup::Keep)
            || completion
                .integration
                .is_some_and(|v| v != FinishIntegration::Merge))
    {
        return Err("review retains work branches and accepts no final integration message");
    }
    // Tagged review's tag and merge guidance options need branch-policy selection.
    Ok((request, completion, show_credentials, skip_checks))
}

pub fn run(args: &[OsString]) -> ExitCode {
    if matches!(args, [arg] if arg == "--help" || arg == "-h") {
        help();
        return ExitCode::SUCCESS;
    }
    let (mut request, completion, show_credentials, skip_checks) = match parse(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            crate::presentation::error(format!("error: {error}; use finish --help (alias: f)"));
            return ExitCode::from(2);
        }
    };
    let mut presentation = resolve_presentation(show_credentials, None);
    let directory = match env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            crate::presentation::error(format!(
                "error: cannot determine current directory: {}",
                text(presentation, error)
            ));
            return ExitCode::FAILURE;
        }
    };
    // Completion resolves before remote refresh or any workflow effects. Loading
    // validates every supplied layer, even settings inactive for review delivery.
    let configured = match crate::config::inspect(&directory) {
        Ok(configured) => configured,
        Err(error) => {
            crate::presentation::error(format!("error: {}", text(presentation, &error)));
            error_diagnostics(presentation, &error);
            return ExitCode::FAILURE;
        }
    };
    presentation =
        resolve_presentation(show_credentials, Some(configured.loaded.redact_credentials));
    let mut mode = completion
        .mode
        .unwrap_or(configured.loaded.settings.finish_mode);
    let integration_request = integration::Request {
        update: request.update,
        history_edit: request.history_edit,
        rewrite: request.rewrite,
        integration: completion.integration,
        message: completion.message.clone(),
        checkout: completion.checkout,
        cleanup: completion.cleanup,
        release: completion.release.clone(),
    };
    // Branch policy is resolved from the associated source, including retained
    // attempts already checked out on their integration target.
    if let Ok(bound) = integration::assess(&directory, &integration_request, None)
        && completion.mode.is_none()
    {
        mode = bound.settings.finish_mode;
    }
    let integrated_review = configured
        .loaded
        .settings
        .workstream
        .as_ref()
        .is_some_and(|p| {
            p.version_tagging && p.version_tag_identity == VersionTagIdentity::IntegratedResult
        });
    let hosted_request = finish::hosted::Request {
        release: completion.release.clone(),
        merge_back: completion.merge_back.clone(),
        update: request.update,
        history_edit: request.history_edit,
    };
    let hosted_initial = finish::hosted::assess(&directory, &hosted_request);
    if completion.mode.is_none()
        && let Ok(plan) = &hosted_initial
    {
        mode = plan.settings.finish_mode;
    }
    let hosted_selected = match finish::hosted::selected(&directory) {
        Ok(selected) => selected,
        Err(error) => {
            crate::presentation::error(format!("error: {}", text(presentation, error)));
            return ExitCode::FAILURE;
        }
    };
    if finish::hosted::associated(&directory).unwrap_or(true)
        && let Err(error) = &hosted_initial
    {
        crate::presentation::error(format!("error: {}", text(presentation, error)));
        return ExitCode::FAILURE;
    }
    if hosted_initial.as_ref().is_ok_and(|p| p.is_retry()) && mode == FinishMode::Integrate {
        crate::presentation::error(
            "error: a retained hosted preparation must resume in review mode",
        );
        return ExitCode::FAILURE;
    }
    // Snapshot each selectable workflow before asking. The selected workflow's
    // full guard prevents mode input from adopting concurrent unrelated changes.
    // Assessment failure in an unselected workflow does not affect the choice.
    let mode_snapshot = (mode == FinishMode::Ask).then(|| {
        (
            finish::assess(&directory, &request, None),
            integration::assess(&directory, &integration_request, None),
        )
    });
    if mode == FinishMode::Ask {
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            crate::presentation::error(
                "error: unresolved completion mode; select --mode <review|integrate> or configure finish-mode. Terminal input and stderr are required to ask.",
            );
            return ExitCode::FAILURE;
        }
        eprintln!(
            "Review prepares and publishes work for manual review. Integrate delivers to the project base, then completes checkout and cleanup."
        );
        let answer = match read_menu_answer(
            presentation,
            "Completion mode (review/integrate/cancel)",
            &["review", "integrate", "cancel"],
            "Finish cancelled before branch execution; finish made no branch, index, or working-tree changes.",
        ) {
            Ok(answer) => answer,
            Err(error) => {
                error_or_plain(&error);
                return ExitCode::FAILURE;
            }
        };
        mode = match answer.as_str() {
            "review" => FinishMode::Review,
            "integrate" => FinishMode::Integrate,
            _ => {
                crate::presentation::error(
                    "error: invalid completion mode; rerun finish to choose review or integrate.",
                );
                return ExitCode::FAILURE;
            }
        };
    }
    if mode == FinishMode::Review
        && configured
            .loaded
            .settings
            .workstream
            .as_ref()
            .is_some_and(|p| p.integration_targets.len() != 1)
    {
        crate::presentation::error(
            "error: review finish supports one integration destination; select integrate mode for multiple destinations",
        );
        return ExitCode::FAILURE;
    }
    if integrated_review
        && mode == FinishMode::Review
        && (!completion.release.is_empty() || completion.merge_back.is_some())
    {
        crate::presentation::error(
            "error: integrated-result review creates no version tag or merge-back branch; tag the validated merged result after review",
        );
        return ExitCode::from(2);
    }
    if hosted_selected && mode == FinishMode::Review {
        return hosted_cli::run(
            &directory,
            hosted_initial,
            &request,
            &completion,
            presentation,
            skip_checks,
        );
    }
    if completion.merge_back.is_some() {
        crate::presentation::error("error: --merge-back requires tagged review mode");
        return ExitCode::from(2);
    }
    if let Err(error) = completion.validate(mode) {
        crate::presentation::error(format!("error: {error}; use finish --help (alias: f)"));
        return ExitCode::from(2);
    }
    if let Some((review, integrate)) = &mode_snapshot {
        let guarded = match mode {
            FinishMode::Review => {
                review
                    .as_ref()
                    .map_err(|e| text(presentation, e))
                    .and_then(|p| {
                        finish::revalidate(&directory, p, p.evidence.as_ref())
                            .map_err(|e| text(presentation, e))
                    })
            }
            FinishMode::Integrate => integrate
                .as_ref()
                .map_err(|e| text(presentation, e))
                .and_then(|p| {
                    integration::revalidate(&directory, p, p.evidence.as_ref())
                        .map_err(|e| text(presentation, e))
                }),
            FinishMode::Ask => unreachable!("mode resolved"),
        };
        if let Err(error) = guarded {
            crate::presentation::error(format!(
                "error: completion mode context could not be retained: {error}; rerun finish."
            ));
            return ExitCode::FAILURE;
        }
    }
    let (review_initial, integrate_initial) = match mode_snapshot {
        Some((review, integrate)) => (Some(review), Some(integrate)),
        None => (None, None),
    };
    if mode == FinishMode::Integrate {
        let initial = match integrate_initial
            .unwrap_or_else(|| integration::assess(&directory, &integration_request, None))
        {
            Ok(plan) => plan,
            Err(error) => {
                crate::presentation::error(format!("error: {}", text(presentation, &error)));
                error_diagnostics(presentation, &error);
                let mut output = String::new();
                remaining(presentation, &mut output, &repository::inspect(&directory));
                crate::presentation::report(&output, false);
                return ExitCode::FAILURE;
            }
        };
        if initial.repository != configured.repository
            || crate::config::inspect(&directory).map_or(true, |now| {
                now.loaded.settings != configured.loaded.settings
            })
        {
            crate::presentation::error(
                "error: repository or settings changed while resolving completion mode; rerun finish.",
            );
            return ExitCode::FAILURE;
        }
        return integration_cli::run(&directory, initial, presentation, skip_checks);
    }
    let initial = match review_initial.unwrap_or_else(|| finish::assess(&directory, &request, None))
    {
        Ok(plan) => plan,
        Err(error) => {
            crate::presentation::error(format!("error: {}", text(presentation, &error)));
            error_diagnostics(presentation, &error);
            let mut output = String::new();
            remaining(presentation, &mut output, &repository::inspect(&directory));
            output.push_str("Branch publication not attempted.\n");
            crate::presentation::report(&output, false);
            return ExitCode::FAILURE;
        }
    };
    if initial.repository != configured.repository || initial.settings != configured.loaded.settings
    {
        crate::presentation::error(
            "error: repository or settings changed while resolving completion mode; rerun finish.",
        );
        return ExitCode::FAILURE;
    }
    let refresh = preparation::refresh(&directory, &initial);
    let Some(mut plan) = refresh.plan.clone().filter(|_| refresh.error.is_none()) else {
        preparation_failure(presentation, &refresh, None);
        return ExitCode::FAILURE;
    };
    loop {
        if !plan.constraints.is_empty() {
            let mut output = String::new();
            for constraint in &plan.constraints {
                constraint_message(presentation, &mut output, constraint);
            }
            preparation_failure(presentation, &refresh, Some(&output));
            return ExitCode::FAILURE;
        }
        let Some(choice) = plan.required_input.first().copied() else {
            break;
        };
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            let guidance = match choice {
                RequiredInput::Update => {
                    "select --update <rebase|merge|none> or configure finish-update"
                }
                RequiredInput::Rewrite => {
                    "select --allow-rewrite or --no-rewrite, or configure finish-rewrite as allow or deny"
                }
            };
            preparation_failure(
                presentation,
                &refresh,
                Some(&format!(
                    "error: unresolved {} choice; terminal input and stderr are required; {guidance}.\n",
                    match choice {
                        RequiredInput::Update => "update",
                        RequiredInput::Rewrite => "rewrite",
                    }
                )),
            );
            return ExitCode::FAILURE;
        }
        eprintln!(
            "Finish {} at {} to {} (base {}/{}).",
            display(presentation, &plan.branch),
            display(presentation, &plan.commit),
            text(presentation, &plan.settings.publish_remote),
            text(presentation, &plan.settings.base_remote),
            text(
                presentation,
                plan.settings
                    .workstream
                    .as_ref()
                    .map_or(&plan.settings.base, |p| &p.preparation_source)
            )
        );
        let answer = match prompt(presentation, &plan, choice) {
            Ok(answer) => answer,
            Err(error) => {
                preparation_failure(presentation, &refresh, Some(&format!("{error}\n")));
                return ExitCode::FAILURE;
            }
        };
        match choice {
            RequiredInput::Update => {
                request.update = Some(match answer.as_str() {
                    "rebase" => FinishUpdate::Rebase,
                    "merge" => FinishUpdate::Merge,
                    "none" => FinishUpdate::None,
                    _ => {
                        preparation_failure(
                            presentation,
                            &refresh,
                            Some("error: invalid update choice; rerun finish to choose again.\n"),
                        );
                        return ExitCode::FAILURE;
                    }
                })
            }
            RequiredInput::Rewrite => {
                request.rewrite = Some(match answer.as_str() {
                    "allow" => FinishRewrite::Allow,
                    "deny" => FinishRewrite::Deny,
                    _ => {
                        preparation_failure(
                            presentation,
                            &refresh,
                            Some("error: invalid rewrite choice; rerun finish to choose again.\n"),
                        );
                        return ExitCode::FAILURE;
                    }
                })
            }
        }
        plan = match resolve(&directory, &plan, &request) {
            Ok(plan) => plan,
            Err(error) => {
                preparation_failure(
                    presentation,
                    &refresh,
                    Some(&format!("error: {}\n", text(presentation, &error))),
                );
                error_diagnostics(presentation, &error);
                return ExitCode::FAILURE;
            }
        };
    }
    if plan.history_edit == HistoryEdit::Interactive
        && plan.update == FinishUpdate::Rebase
        && let Some(finish::RefObservation::Commit(boundary)) =
            plan.evidence.as_ref().map(|e| &e.base)
    {
        history_edit_notice(presentation, &plan.branch, boundary);
    }
    let mut prepared = preparation::prepare(&directory, &plan);
    if !prepared.is_prepared() {
        let mut effects = refresh.effects.clone();
        effects.append(&mut prepared.effects);
        prepared.effects = effects;
        preparation_failure(presentation, &prepared, None);
        return ExitCode::FAILURE;
    }
    let plan = prepared.plan.as_ref().expect("prepared plan");
    let mut check_output = CheckOutput::new(presentation);
    let published =
        publication::publish_with_checks(&directory, plan, skip_checks, |event| match event {
            checks::Event::Started {
                index,
                total,
                check,
            } => {
                eprintln!(
                    "Check {}/{}: {} — {}",
                    index + 1,
                    total,
                    text(presentation, &check.name),
                    text(
                        presentation,
                        serde_json::to_string(&check.command).expect("command JSON")
                    )
                );
            }
            checks::Event::Output { stream, bytes } => check_output.write(stream, bytes),
            checks::Event::Passed { index } => {
                check_output.flush();
                eprintln!("Check {} passed.", index + 1);
            }
            checks::Event::Skipped { count } => {
                eprintln!("Skipped {count} configured finish check(s) (--skip-checks).")
            }
        });
    check_output.flush();
    eprint!(
        "{}",
        crate::cli::cache_issues(
            &published.caches,
            |bytes| display(presentation, bytes),
            true
        )
    );
    if published.is_complete() {
        crate::presentation::report(
            &format!(
                "Published {} to {} at {}.",
                display(presentation, &plan.branch),
                text(presentation, &published.remote),
                display(presentation, &published.commit)
            ),
            true,
        );
        for destination in &published.destinations {
            println!(
                "  {}: {}",
                display(presentation, &destination.destination),
                destination_state(destination.state)
            );
        }
        println!(
            "Next: submit {}/{} for review into {}/{}.",
            text(presentation, &plan.settings.publish_remote),
            display(presentation, &plan.branch),
            text(presentation, &plan.settings.base_remote),
            text(presentation, &plan.settings.base)
        );
        if integrated_review {
            println!(
                "Ready for manual submission. After merging the named source into the target, validate and tag the actual merged result, then clean up the work branch as appropriate."
            );
            if let Some(marker) = plan
                .settings
                .workstream
                .as_ref()
                .and_then(|p| p.release_marker.as_ref())
            {
                println!(
                    "Then manually fast-forward release marker {} to that released tag commit.",
                    text(presentation, marker)
                );
            }
        }
        ExitCode::SUCCESS
    } else {
        let mut output = String::new();
        let _ = writeln!(
            output,
            "Finish incomplete for {} at {} to {}.",
            display(presentation, &plan.branch),
            display(presentation, &published.commit),
            text(presentation, &published.remote)
        );
        output.push('\n');
        role_summary(presentation, &mut output, &plan.settings);
        preparation_effects(presentation, &mut output, &refresh);
        preparation_effects(presentation, &mut output, &prepared);
        if let Some(error) = &published.checks.error {
            if let Some(index) = published.checks.failed {
                let _ = writeln!(
                    output,
                    "Check {} ({}) stopped: {}",
                    index + 1,
                    text(presentation, &plan.settings.finish_checks[index].name),
                    text(presentation, error)
                );
            } else {
                let _ = writeln!(output, "Check stage stopped: {}", text(presentation, error));
            }
            output.push_str("Branch publication not attempted. Prepared history and check effects are retained. Fix the reported problem, commit or preserve any changes, then rerun finish; all checks run again.\n");
        }
        if let Some(error) = &published.error {
            let _ = writeln!(output, "error: {}", text(presentation, error));
        }
        for destination in &published.destinations {
            let _ = writeln!(
                output,
                "  {}: {}",
                display(presentation, &destination.destination),
                destination_state(destination.state)
            );
            if let Some(error) = &destination.error {
                let _ = writeln!(output, "  error: {}", text(presentation, error));
            }
            command_diagnostics(presentation, &mut output, &destination.commands);
        }
        tracking(presentation, &mut output, &published.tracking);
        remaining(presentation, &mut output, &published.remaining);
        let _ = writeln!(
            output,
            "\nResolve the reported failure, then rerun finish. Completed effects are retained."
        );
        crate::presentation::report(&output, false);
        ExitCode::FAILURE
    }
}

/// Stream output while retaining incomplete UTF-8 and URL authorities. In
/// particular, a credential split across pipe reads must never be printed early.
struct CheckOutput {
    presentation: Presentation,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
impl CheckOutput {
    fn new(presentation: Presentation) -> Self {
        Self {
            presentation,
            stdout: vec![],
            stderr: vec![],
        }
    }
    fn write(&mut self, stream: checks::Stream, bytes: &[u8]) {
        let buffer = match stream {
            checks::Stream::Stdout => &mut self.stdout,
            checks::Stream::Stderr => &mut self.stderr,
        };
        buffer.extend_from_slice(bytes);
        let end = Self::safe_prefix(buffer, self.presentation);
        let ready: Vec<_> = buffer.drain(..end).collect();
        Self::emit(self.presentation, stream, &ready);
    }
    fn safe_prefix(bytes: &[u8], presentation: Presentation) -> usize {
        let mut end = bytes.len();
        if let Err(error) = std::str::from_utf8(bytes)
            && error.error_len().is_none()
        {
            end = error.valid_up_to();
        }
        if presentation.show_credentials {
            return end;
        }
        let mut cursor = 0;
        while cursor < end {
            let tail = &bytes[cursor..end];
            for scheme in [b"https://".as_slice(), b"http://".as_slice()] {
                if scheme.starts_with(tail) {
                    return cursor;
                }
                if let Some(authority) = tail.strip_prefix(scheme) {
                    let Some(stop) = authority.iter().position(|b| {
                        matches!(b, b'/' | b'?' | b'#' | b' ' | b'\n' | b'\r' | b'\t')
                    }) else {
                        return cursor;
                    };
                    cursor += scheme.len() + stop;
                    break;
                }
            }
            cursor += 1;
        }
        end
    }
    fn emit(presentation: Presentation, stream: checks::Stream, bytes: &[u8]) {
        let mut rendered = String::new();
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if let Some(line) = line.strip_suffix(b"\n") {
                rendered.push_str(&display(presentation, line));
                rendered.push('\n');
            } else {
                rendered.push_str(&display(presentation, line));
            }
        }
        match stream {
            checks::Stream::Stdout => {
                let _ = io::stdout().write_all(rendered.as_bytes());
                let _ = io::stdout().flush();
            }
            checks::Stream::Stderr => {
                let _ = io::stderr().write_all(rendered.as_bytes());
                let _ = io::stderr().flush();
            }
        }
    }
    fn flush(&mut self) {
        for (stream, buffer) in [
            (checks::Stream::Stdout, &mut self.stdout),
            (checks::Stream::Stderr, &mut self.stderr),
        ] {
            Self::emit(self.presentation, stream, buffer);
            buffer.clear();
        }
    }
}

fn resolve(directory: &Path, reviewed: &Plan, request: &Request) -> Result<Plan, FinishError> {
    finish::revalidate(directory, reviewed, reviewed.evidence.as_ref())?;
    let resolved = finish::assess(directory, request, reviewed.evidence.as_ref())?;
    finish::revalidate(directory, reviewed, reviewed.evidence.as_ref())?;
    Ok(resolved)
}
fn prompt(
    presentation: Presentation,
    plan: &Plan,
    choice: RequiredInput,
) -> Result<String, String> {
    let prompt = match choice {
        RequiredInput::Update => {
            eprintln!(
                "Rebase replays branch commits onto the observed base. Merge incorporates that base. None publishes current history without updating."
            );
            "Update (rebase/merge/none/cancel)"
        }
        RequiredInput::Rewrite => {
            eprintln!(
                "Published history may require replacement. Rebase selection supplies no replacement authorization."
            );
            for destination in &plan.destinations {
                if matches!(
                    destination.mode,
                    finish::PublicationMode::RecheckAfterRebase
                        | finish::PublicationMode::VerifiedRewrite
                ) && let finish::RefObservation::Commit(oid) = &destination.observed
                {
                    eprintln!(
                        "  {}: observed {}",
                        display(presentation, &destination.destination),
                        display(presentation, oid)
                    );
                }
            }
            eprintln!(
                "Allow authorizes eligible replacement for this finish; incorporation proof and exact remote leases remain required. Deny stops the required replacement."
            );
            "Replace published history (allow/deny/cancel)"
        }
    };
    read_menu_answer(
        presentation,
        prompt,
        match choice {
            RequiredInput::Update => &["rebase", "merge", "none", "cancel"],
            RequiredInput::Rewrite => &["allow", "deny", "cancel"],
        },
        "Finish cancelled before branch execution; finish made no branch, index, or working-tree changes.",
    )
}
fn read_menu_answer(
    presentation: Presentation,
    prompt: &str,
    choices: &[&str],
    cancellation: &str,
) -> Result<String, String> {
    let answer = crate::presentation::select(prompt, choices, None)
        .map_err(|e| format!("error: cannot read finish input: {}", text(presentation, e)))?
        .ok_or_else(|| cancellation.to_owned())?;
    if answer == "cancel" {
        Err(cancellation.into())
    } else {
        Ok(answer)
    }
}

pub(super) fn error_or_plain(message: &str) {
    if message.starts_with("error:") {
        crate::presentation::error(message);
    } else {
        eprintln!("{message}");
    }
}
fn read_answer_with_cancellation(
    presentation: Presentation,
    prompt: &str,
    cancellation: &str,
) -> Result<String, String> {
    let answer = crate::presentation::input(prompt)
        .map_err(|e| format!("error: cannot read finish input: {}", text(presentation, e)))?
        .ok_or_else(|| cancellation.to_owned())?;
    if answer.trim() == "cancel" {
        return Err(cancellation.into());
    }
    Ok(answer.trim().into())
}

/// Validate literal names using the same Git grammar as workflow assessment.
/// Callers revalidate their original plan after input before adopting the name.
fn read_reference_name(
    root: &Path,
    presentation: Presentation,
    prompt: &str,
    prefix: &str,
    cancellation: &str,
) -> Result<String, String> {
    loop {
        let answer = read_answer_with_cancellation(presentation, prompt, cancellation)?;
        if answer.is_empty() || answer.starts_with('-') || answer.contains('\0') {
            eprintln!("Enter a valid Git reference name, or cancel.");
            continue;
        }
        let reference = format!("{prefix}{answer}");
        let result = crate::git::inspect(root, ["check-ref-format", &reference]).map_err(|e| {
            format!(
                "error: cannot validate reference name: {}",
                text(presentation, e)
            )
        })?;
        if result.status.success() {
            return Ok(answer);
        }
        eprintln!("Enter a valid Git reference name, or cancel.");
    }
}

fn constraint_message(presentation: Presentation, out: &mut String, constraint: &Constraint) {
    let message = match constraint {
        Constraint::FreshRemoteEvidenceRequired => "refresh remote evidence before finishing",
        Constraint::BaseUnavailable => {
            "the project base is absent or unavailable; check base-remote/base settings and access"
        }
        Constraint::BaseHistoryUncertain => {
            "base ancestry is uncertain; obtain complete history before finishing"
        }
        Constraint::UnrelatedBase => {
            "base history is unrelated; inspect history or select --update none to preserve it"
        }
        Constraint::GraftedHistory => "resolve legacy Git grafts before finishing",
        Constraint::HistoryEdit(problem) => {
            let _ = writeln!(out, "error: {}.", text(presentation, problem));
            return;
        }
        Constraint::DestinationUnavailable(url) => {
            return destination_constraint(
                presentation,
                out,
                url,
                "history unavailable; check access and fetch complete history",
            );
        }
        Constraint::DestinationAmbiguous(url) => {
            return destination_constraint(
                presentation,
                out,
                url,
                "ambiguous observation; inspect destination configuration",
            );
        }
        Constraint::DestinationHistoryUncertain(url) => {
            return destination_constraint(
                presentation,
                out,
                url,
                "ancestry uncertain; obtain complete history",
            );
        }
        Constraint::RemoteWorkNotIncorporated(url) => {
            return destination_constraint(
                presentation,
                out,
                url,
                "remote work is not incorporated; incorporate it with Git, then rerun finish",
            );
        }
        Constraint::RewriteDenied(url) => {
            return destination_constraint(
                presentation,
                out,
                url,
                "required history replacement denied; use --allow-rewrite to authorize eligible replacement, or choose --update merge or none before preparation",
            );
        }
    };
    let _ = writeln!(out, "error: {message}.");
}
fn history_edit_notice(presentation: Presentation, branch: &[u8], boundary: &[u8]) {
    eprintln!(
        "Edit work history on {} relative to preparation boundary {} using Git's native editors. Shared boundary history remains outside the edit range.",
        display(presentation, branch),
        display(presentation, boundary)
    );
}
fn destination_constraint(presentation: Presentation, out: &mut String, url: &[u8], message: &str) {
    let _ = writeln!(out, "error: {}: {message}.", display(presentation, url));
}
fn destination_state(state: publication::DestinationState) -> &'static str {
    use publication::DestinationState::*;
    match state {
        NotAttempted => "not attempted",
        Created => "created",
        Advanced => "advanced",
        Rewritten => "replaced",
        Unchanged => "unchanged (already published)",
        Rejected => "rejected",
        Unconfirmed => "unconfirmed; remote effects may exist",
    }
}
fn preparation_failure(
    presentation: Presentation,
    report: &preparation::Report,
    message: Option<&str>,
) {
    let mut output = String::from("Finish incomplete.\n");
    if let Some(message) = message {
        output.push_str(message);
    }
    if let Some(error) = &report.error {
        let _ = writeln!(output, "error: {}", text(presentation, error));
    }
    if report.error.is_some() {
        let _ = writeln!(output, "Next: {}", text(presentation, report.guidance()));
    }
    if let Some(plan) = &report.plan {
        role_summary(presentation, &mut output, &plan.settings);
    }
    output.push('\n');
    preparation_effects(presentation, &mut output, report);
    command_diagnostics(presentation, &mut output, &report.commands);
    // Input may have changed context since refresh; report an actual new observation.
    let current = report.plan.as_ref().map(|p| {
        repository::inspect(
            p.repository
                .working_tree_root
                .as_deref()
                .expect("working plan"),
        )
    });
    remaining(
        presentation,
        &mut output,
        current.as_ref().unwrap_or(&report.remaining),
    );
    let _ = writeln!(output, "Branch publication not attempted.");
    crate::presentation::report(&output, false);
    if let Some(preparation::PreparationError::Finish(error)) = &report.error {
        error_diagnostics(presentation, error);
    }
}
fn role_summary(
    presentation: Presentation,
    out: &mut String,
    settings: &crate::settings::Settings,
) {
    let _ = writeln!(
        out,
        "Planned publication remote: {}; project base: {}/{}.",
        text(presentation, &settings.publish_remote),
        text(presentation, &settings.base_remote),
        text(presentation, &settings.base)
    );
}

fn preparation_effects(presentation: Presentation, out: &mut String, report: &preparation::Report) {
    for effect in &report.effects {
        use preparation::Effect::*;
        match effect {
            Observed {
                destination,
                reference,
                value,
            } => {
                let value = match value {
                    finish::RefObservation::Commit(oid) => display(presentation, oid),
                    other => format!("{other:?}"),
                };
                let _ = writeln!(
                    out,
                    "Observed {} at {}: {value}.",
                    display(presentation, reference),
                    display(presentation, destination)
                );
            }
            Fetched {
                destination,
                commit,
            } => {
                let _ = writeln!(
                    out,
                    "Fetched objects for {} from {}.",
                    display(presentation, commit),
                    display(presentation, destination)
                );
            }
            RecoveryRecorded(path) => {
                let _ = writeln!(
                    out,
                    "Retained rebase recovery evidence: {}.",
                    text(presentation, path.display())
                );
            }
            Updated {
                update,
                before,
                after,
            } => {
                let _ = writeln!(
                    out,
                    "Prepared with {}: {} -> {}.",
                    update.as_str(),
                    display(presentation, before),
                    display(presentation, after)
                );
            }
        }
    }
}
fn command_diagnostics(
    presentation: Presentation,
    out: &mut String,
    commands: &[preparation::CommandReport],
) {
    for command in commands {
        match &command.result {
            Ok(result) if !result.status.success() => {
                let _ = writeln!(out, "Git process: {}", result.status);
                for bytes in [&result.stdout, &result.stderr] {
                    if !bytes.is_empty() {
                        out.push_str(&crate::cli::git_diagnostics(bytes, |line| {
                            display(presentation, line)
                        }));
                    }
                }
            }
            Err(error) => {
                let _ = writeln!(out, "Git process: {}", text(presentation, error));
            }
            _ => (),
        }
    }
}
fn tracking(presentation: Presentation, out: &mut String, report: &publication::TrackingReport) {
    use publication::{TrackingState, TrackingWrite};
    let state = match report.state {
        TrackingState::Matching => "matching",
        TrackingState::Absent => "absent; setup incomplete",
        TrackingState::Incomplete => "incomplete",
        TrackingState::Unavailable => "unavailable",
    };
    let _ = writeln!(out, "Observed upstream tracking: {state}.");
    for write in &report.writes {
        let _ = writeln!(
            out,
            "Completed tracking step: {}.",
            match write {
                TrackingWrite::Remote => "set upstream remote",
                TrackingWrite::MergeRef => "set upstream branch",
            }
        );
    }
    match &report.after {
        Ok(observed) => {
            for (label, values) in [
                ("Remote", &observed.upstream.remotes),
                ("Branch ref", &observed.upstream.merge_refs),
            ] {
                for value in values {
                    let _ = writeln!(out, "  {label}: {}", display(presentation, value));
                }
            }
        }
        Err(error) => {
            let _ = writeln!(
                out,
                "  Tracking observation unavailable: {}",
                text(presentation, error)
            );
        }
    }
}
fn remaining(
    presentation: Presentation,
    out: &mut String,
    observation: &Result<Observation, InspectionError>,
) {
    remaining_state(presentation, out, observation, true);
}
fn remaining_state(
    presentation: Presentation,
    out: &mut String,
    observation: &Result<Observation, InspectionError>,
    operation_guidance: bool,
) {
    let _ = writeln!(out, "\nRemaining state (observed):");
    match observation {
        Ok(Observation::Repository(state)) => {
            match &state.head {
                Head::Attached { branch, commit } => {
                    let _ = writeln!(
                        out,
                        "  HEAD: {} at {}",
                        display(presentation, branch),
                        display(presentation, commit)
                    );
                }
                Head::Detached { commit } => {
                    let _ = writeln!(out, "  HEAD: detached at {}", display(presentation, commit));
                }
                Head::Unborn { branch } => {
                    let _ = writeln!(out, "  HEAD: unborn {}", display(presentation, branch));
                }
            }
            if let Some(tree) = &state.working_tree {
                let _ = writeln!(
                    out,
                    "  Index/working tree: {}",
                    if tree.is_dirty() {
                        "contains changes"
                    } else {
                        "clean"
                    }
                );
            }
            let _ = writeln!(
                out,
                "  Git operations: {}",
                if state.operations.is_active() {
                    "active"
                } else {
                    "none detected"
                }
            );
            if operation_guidance && state.operations.rebase {
                let _ = writeln!(
                    out,
                    "Resolve conflicts and use git rebase --continue or git rebase --abort, then rerun finish."
                );
            }
            if operation_guidance && state.operations.merge {
                let _ = writeln!(
                    out,
                    "Resolve conflicts and use git merge --continue or git merge --abort, then rerun finish."
                );
            }
        }
        Ok(Observation::OutsideRepository) => {
            let _ = writeln!(out, "  Outside a Git repository");
        }
        Err(error) => {
            let _ = writeln!(
                out,
                "  Observation unavailable: {}",
                text(presentation, error)
            );
        }
    }
}
fn error_diagnostics(presentation: Presentation, error: &(dyn std::error::Error + 'static)) {
    if let Some(InspectionError::Git { output, .. }) = error.downcast_ref::<InspectionError>()
        && !output.stderr.is_empty()
    {
        eprint!(
            "{}",
            crate::cli::git_diagnostics(&output.stderr, |line| display(presentation, line))
        );
    }
    if let Some(InspectionError::Io(_)) = error.downcast_ref::<InspectionError>() {
        eprintln!("Ensure Git is installed on PATH and repository paths are accessible.");
    }
    if let Some(source) = error.source() {
        error_diagnostics(presentation, source);
    }
}
fn help() {
    println!(
        r#"Prepare and deliver finished work for review or integration. Alias: f.

Usage: git-oneflow finish [OPTIONS]

Options:
  --mode <review|integrate>      Override completion mode
  --update <rebase|merge|none>   Select preparation against the project base
  --history-edit <none|interactive>
                                Edit ordinary work with native interactive rebase
  --integration <ff-only|merge|squash>
                                Select final target integration
  -m, --message <text>          Supply the final integration commit message
  --checkout <target|work>      Select checkout after target delivery
  --cleanup <keep|local|both>   Select work retention after target delivery
  --allow-rewrite               Authorize eligible published-history replacement
  --no-rewrite                  Deny published-history replacement
  --tag <name>                  Select the version tag for tagged work
  --tag-message <text>          Annotate that tag separately from -m
  --sign-tag                    Sign the version tag using Git configuration
  --merge-back <branch>         Name a distinct tagged-review submission branch
  --skip-checks                 Bypass configured checks for this attempt
  --show-credentials            Show HTTP(S) credentials in finish output
  -h, --help                    Show help

Examples:
  gof finish
  gof f --mode review --update merge
  gof finish --mode integrate --integration ff-only --cleanup both
  gof finish --update rebase --history-edit interactive
  gof finish --integration squash -m "Complete parser work" --cleanup keep
  gof finish --tag v1.2 --merge-back review/v1.2

Notes:
  Review prepares and publishes work for manual submission into the project base.
  Integrate delivers to base-remote, then applies checkout and work cleanup.
  --update merge prepares work; --integration merge creates the target merge commit.
  --cleanup local deletes local work; both also deletes published work; keep retains both.
  Checkout work requires cleanup keep. Tag and all required target/marker deliveries
  must be confirmed before cleanup. Failures retain completed effects and report pending work.
  -m supplies a final integration message; work commits and review descriptions remain unchanged.
  Merge/squash may open Git's editor with a suggested message. Save and exit to accept;
  clear all message text or exit unsuccessfully to cancel. ff-only needs no message.
  Integration message editing requires POSIX sh on PATH; -m/--message or auto bypasses it.
  Interactive history editing requires update rebase and ordinary work; native Git editors,
  hooks and signing apply. Without a terminal, configure sequence and commit editors explicitly.
  Finish rebase/merge conflicts with git rebase --continue or git merge --continue;
  use the corresponding --abort to abandon preparation, then rerun finish.
  Rebase and history editing grant no replacement authorization; --allow-rewrite is separate.
  Tagged direct rebase requires explicit --update rebase; validate the prepared version.
  Work-source tagged review preserves the released version and prepares a separate merge-back
  branch for manual no-ff submission. It retains both branches; marker and later cleanup are manual.
  Integrated-result review defers tagging until the actual review result is merged and validated.
  Review supports one target. Multiple tagged destinations require integrate mode.
  Checks run in order from the worktree root after preparation. Failure stops publication.
  --skip-checks bypasses checks for this attempt; Git hooks and publication guards remain active.
  New work needs a clean, attached branch with a commit. Retained integration recovery can
  resume after target checkout or local work deletion. Retry finishes pending effects.
  Existing publication-branch or project-base tracking is preserved.
  Ordinary Git commands use your Git configuration for routing and tracking.
  HTTP(S) credentials are hidden by default; --show-credentials exposes them in output/logs.

Settings:
  Explicit flags override local, shared and built-in settings. Defaults ask for mode,
  update, integration, message, cleanup and eligible rewrite authorization; checkout
  defaults to target, history editing to none, and checks to [].
  Use arrow keys and Enter for finite choices, or type a displayed number or name.
  Invalid choices stay correctable. Esc, Ctrl-C/Ctrl-D, or cancel stops finish;
  completed preparation/check effects remain. Scripts must resolve choices with flags/settings.
  Run gof config for setup or gof config show to inspect effective settings.

Exit status:
  0  Confirmed complete review publication or integration with checkout and cleanup
  1  Failure, unresolved input, cancellation, or incomplete execution
  2  Invalid syntax"#
    );
}

#[cfg(test)]
mod presentation_tests {
    use super::{Presentation, display, redact_url_userinfo, resolve_presentation};

    #[test]
    fn finish_presentation_redacts_http_credentials_and_keeps_endpoints() {
        let raw = b"https://alice:synthetic-password@example.invalid/repo";
        let transport_destination = raw.to_vec();
        let default = resolve_presentation(false, None);
        let local_override = resolve_presentation(false, Some(false));
        let flag_override = resolve_presentation(true, Some(false));
        assert!(!default.show_credentials);
        assert!(local_override.show_credentials);
        assert!(flag_override.show_credentials);
        let presented = display(default, raw);
        assert_eq!(transport_destination, raw);
        assert!(!presented.contains("synthetic-password"));
        assert!(presented.contains("example.invalid/repo"));

        for value in [
            "https://alice:synthetic-password@example.invalid/repo",
            "https://synthetic-token@example.invalid/repo",
            "fatal: unable to access 'https://alice:synthetic-password@example.invalid/repo': denied",
        ] {
            let shown = redact_url_userinfo(value);
            assert!(!shown.contains("synthetic-password"), "{shown}");
            assert!(!shown.contains("synthetic-token"), "{shown}");
            assert!(shown.contains("<redacted>@example.invalid/repo"), "{shown}");
        }
        assert_eq!(
            redact_url_userinfo("git@example.invalid:team/repo"),
            "git@example.invalid:team/repo"
        );
        assert_eq!(redact_url_userinfo("/tmp/repo"), "/tmp/repo");
        assert_eq!(
            display(
                default,
                b"https://alice:synthetic-password@example.invalid/repo"
            ),
            "https://<redacted>@example.invalid/repo"
        );
        assert_eq!(
            redact_url_userinfo("https://example.invalid/repo"),
            "https://example.invalid/repo"
        );

        // Completion, prompt, failure, and Git diagnostic presentation all use
        // this same display boundary for destination and diagnostic text.
        let output = [
            format!("Published branch to {}.", display(default, raw)),
            format!("Prompt destination: {}.", display(default, raw)),
            format!(
                "Finish incomplete for destination {}.",
                display(default, raw)
            ),
            format!(
                "Git: {}",
                display(
                    default,
                    b"fatal: rejected https://synthetic-token@example.invalid/repo"
                )
            ),
        ]
        .join("\n");
        assert!(!output.contains("synthetic-password"));
        assert!(!output.contains("synthetic-token"));
        assert!(output.contains("<redacted>@example.invalid/repo"));
        let stdout = format!("Published to {}.", display(local_override, raw));
        let stderr = format!("Git: {}", display(flag_override, raw));
        assert!(stdout.contains("synthetic-password"));
        assert!(stderr.contains("synthetic-password"));
        let unredacted = display(
            Presentation {
                show_credentials: true,
            },
            raw,
        );
        assert!(unredacted.contains("synthetic-password"));
    }
}

#[cfg(test)]
mod grammar_tests {
    use super::*;
    fn parsed(args: &[&str]) -> Result<(Request, Completion, bool, bool), &'static str> {
        parse(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }
    #[test]
    fn completion_grammar_valid_values_and_literal_messages() {
        for integration in ["ff-only", "merge", "squash"] {
            for checkout in ["target", "work"] {
                for cleanup in ["keep", "local", "both"] {
                    for flag in ["-m", "--message"] {
                        if checkout == "work" && cleanup != "keep" {
                            assert!(
                                parsed(&["--checkout", checkout, "--cleanup", cleanup]).is_err()
                            );
                            continue;
                        }
                        let (_, completion, _, _) = parsed(&[
                            "--integration",
                            integration,
                            flag,
                            "literal\n message",
                            "--cleanup",
                            cleanup,
                            "--checkout",
                            checkout,
                            "--mode",
                            "integrate",
                        ])
                        .unwrap();
                        assert_eq!(completion.mode, Some(FinishMode::Integrate));
                        assert_eq!(completion.integration.unwrap().as_str(), integration);
                        assert_eq!(completion.checkout.unwrap().as_str(), checkout);
                        assert_eq!(completion.cleanup.unwrap().as_str(), cleanup);
                        assert_eq!(completion.message.as_deref(), Some("literal\n message"));
                    }
                }
            }
        }
        assert!(parsed(&["--mode", "review", "--update", "merge", "--no-rewrite"]).is_ok());
        assert!(parsed(&["-m", "-literal"]).is_ok());
    }
    #[test]
    fn completion_grammar_missing_invalid_duplicate_and_conflicting_arguments() {
        for (flag, valid) in [
            ("--mode", "review"),
            ("--integration", "merge"),
            ("--checkout", "target"),
            ("--cleanup", "keep"),
        ] {
            assert!(parsed(&[flag]).is_err());
            assert!(parsed(&[flag, "ask"]).is_err());
            assert!(parsed(&[flag, "INVALID"]).is_err());
            assert!(parsed(&[flag, valid, flag, valid]).is_err());
            assert!(parsed(&[flag, "--mode", "integrate"]).is_err());
        }
        for flag in ["-m", "--message"] {
            assert!(parsed(&[flag]).is_err());
            for invalid in ["", " \n", "nul\0"] {
                assert!(parsed(&[flag, invalid]).is_err());
            }
            for other in ["-m", "--message"] {
                assert!(parsed(&[flag, "first", other, "second"]).is_err());
            }
            assert!(parsed(&["--mode", "review", flag, "message"]).is_err());
        }
        assert!(parsed(&["--mode", "review", "--integration", "merge"]).is_ok());
        for (flag, value) in [
            ("--integration", "squash"),
            ("--checkout", "target"),
            ("--cleanup", "both"),
        ] {
            assert!(parsed(&[flag, value, "--mode", "review"]).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert!(parse(&[OsString::from("-m"), OsString::from_vec(vec![255])]).is_err());
        }
    }
}
