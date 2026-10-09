//! Finish grammar, terminal input, and presentation. Plans decide which choices
//! are relevant; preparation and publication own all workflow effects and guards.
use crate::{
    finish::{
        self, Constraint, FinishError, Plan, Request, RequiredInput, preparation, publication,
    },
    repository::{self, Head, InspectionError, Observation},
    settings::{self, FinishRewrite, FinishUpdate},
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
fn parse(args: &[OsString]) -> Result<(Request, bool), &'static str> {
    let mut request = Request::default();
    let mut show_credentials = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--show-credentials") if !show_credentials => show_credentials = true,
            Some("--show-credentials") => return Err("duplicate --show-credentials"),
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
    Ok((request, show_credentials))
}

pub fn run(args: &[OsString]) -> ExitCode {
    if matches!(args, [arg] if arg == "--help" || arg == "-h") {
        help();
        return ExitCode::SUCCESS;
    }
    let (mut request, show_credentials) = match parse(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("error: {error}; use finish --help (alias: f)");
            return ExitCode::from(2);
        }
    };
    let mut presentation = resolve_presentation(show_credentials, None);
    let directory = match env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!(
                "error: cannot determine current directory: {}",
                text(presentation, error)
            );
            return ExitCode::FAILURE;
        }
    };
    if !show_credentials
        && let Ok(Some(context)) = repository::discover(&directory)
        && let Ok(configured) = settings::load_with_files(&directory, &context)
    {
        presentation = resolve_presentation(show_credentials, Some(configured.redact_credentials));
    }
    let initial = match finish::assess(&directory, &request, None) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {}", text(presentation, &error));
            error_diagnostics(presentation, &error);
            let mut output = String::new();
            remaining(presentation, &mut output, &repository::inspect(&directory));
            output.push_str("Branch publication not attempted.\n");
            eprint!("{output}");
            return ExitCode::FAILURE;
        }
    };
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
            "Finish {} at {} to {} (base {}).",
            display(presentation, &plan.branch),
            display(presentation, &plan.commit),
            text(presentation, &plan.settings.publish_remote),
            text(presentation, &plan.settings.base)
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
    let mut prepared = preparation::prepare(&directory, &plan);
    if !prepared.is_prepared() {
        let mut effects = refresh.effects.clone();
        effects.append(&mut prepared.effects);
        prepared.effects = effects;
        preparation_failure(presentation, &prepared, None);
        return ExitCode::FAILURE;
    }
    let plan = prepared.plan.as_ref().expect("prepared plan");
    let published = publication::publish(&directory, plan);
    if published.is_complete() {
        println!(
            "Published {} to {} at {}.",
            display(presentation, &plan.branch),
            text(presentation, &published.remote),
            display(presentation, &published.commit)
        );
        for destination in &published.destinations {
            println!(
                "  {}: {}",
                display(presentation, &destination.destination),
                destination_state(destination.state)
            );
        }
        println!(
            "Next: submit {} for review into {}.",
            display(presentation, &plan.branch),
            text(presentation, &plan.settings.base)
        );
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
        preparation_effects(presentation, &mut output, &refresh);
        preparation_effects(presentation, &mut output, &prepared);
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
            "Inspect reported destinations and Git state, resolve the failure, then rerun finish. Completed effects are retained."
        );
        eprint!("{output}");
        ExitCode::FAILURE
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
                "  rebase  Replay branch commits onto the observed remote base.\n  merge   Incorporate the observed remote base with a merge or fast-forward.\n  none    Publish current history without updating from the base.\n  cancel  Stop before branch execution."
            );
            "Update (rebase/merge/none/cancel): "
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
                "  allow   Authorize eligible replacement for this finish; incorporation proof and exact remote leases remain required.\n  deny    Stop the required replacement.\n  cancel  Stop before branch execution."
            );
            "Replace published history (allow/deny/cancel): "
        }
    };
    eprint!("{prompt}");
    io::stderr().flush().map_err(|e| {
        format!(
            "error: cannot write finish prompt: {}",
            text(presentation, e)
        )
    })?;
    let mut line = String::new();
    let count = io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("error: cannot read finish input: {}", text(presentation, e)))?;
    if count == 0 || !line.ends_with('\n') || line.trim() == "cancel" {
        return Err("Finish cancelled before branch execution; finish made no branch, index, or working-tree changes.".into());
    }
    Ok(line.trim().into())
}

fn constraint_message(presentation: Presentation, out: &mut String, constraint: &Constraint) {
    let message = match constraint {
        Constraint::FreshRemoteEvidenceRequired => "refresh remote evidence before finishing",
        Constraint::BaseUnavailable => {
            "the remote base is absent or unavailable; check remote/base settings and access"
        }
        Constraint::BaseHistoryUncertain => {
            "base ancestry is uncertain; obtain complete history before finishing"
        }
        Constraint::UnrelatedBase => {
            "base history is unrelated; inspect history or select --update none to preserve it"
        }
        Constraint::GraftedHistory => "resolve legacy Git grafts before finishing",
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
    if report.error.is_some() {
        let _ = writeln!(output, "{}", text(presentation, report.guidance()));
    }
    eprint!("{output}");
    if let Some(preparation::PreparationError::Finish(error)) = &report.error {
        error_diagnostics(presentation, error);
    }
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
                        let _ = writeln!(out, "Git: {}", display(presentation, bytes));
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
    let _ = writeln!(out, "Remaining state (observed):");
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
            if state.operations.rebase {
                let _ = writeln!(
                    out,
                    "Resolve conflicts and use git rebase --continue or git rebase --abort, then rerun finish."
                );
            }
            if state.operations.merge {
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
        eprintln!("Git: {}", display(presentation, &output.stderr));
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
        "Prepare and publish the current branch for submission. Alias: f.

Usage: git-oneflow finish [--update <rebase|merge|none>] [--allow-rewrite|--no-rewrite] [--show-credentials]

Options:
  --update <rebase|merge|none>  Override finish-update
  --allow-rewrite             Authorize eligible published-history replacement
  --no-rewrite                Deny published-history replacement
  --show-credentials          Show HTTP(S) URL credentials in finish output
  -h, --help                  Show help

Examples:
  gof finish
  gof f --update merge
  git-oneflow finish --update rebase --allow-rewrite
  gof finish --update none --no-rewrite

Details:
  Requires a clean, attached working branch with a commit, separate from the base.
  Observes and fetches the remote base and each exact push destination.
  Rebase replays commits onto the remote base; merge incorporates it; none retains history.
  Branches already including the base need no update choice.
  Explicit flags override settings; rewrite flags are mutually exclusive.
  Prompts require terminal input and stderr. Enter a displayed choice or cancel.
  HTTP(S) URL credentials are redacted by default; redact-credentials is a private local setting.
  --show-credentials overrides that setting for this invocation. Output and captured logs can expose credentials.
  Rebase selection and replacement authorization are separate choices.
  Required choices resolve before branch execution; changed context stops execution.
  Cancellation retains completed remote observations/fetches and leaves branch work unchanged.
  Scripts must resolve relevant choices with flags or settings.
  Publication uses incorporation proof and exact destination leases for replacement.
  Success reports branch, destination, commit, and a manual submission next step.
  Recover conflicts with Git, then rerun finish. Failures retain completed effects.
  This increment prepares and publishes; checks, integrations, base integration,
  tagging, and cleanup are deferred.

Settings:
  finish-update: ask (default), rebase, merge, none
  finish-rewrite: ask (default), allow, deny
  allow is standing authorization for eligible replacement; remote safeguards remain.
  Precedence: explicit flags > local overrides > shared settings > defaults.
  Use gof config for guided setup or gof config show to inspect values.

Exit status:
  0  Confirmed complete publication, including unchanged publication
  1  Failure, unresolved input, cancellation, or incomplete execution
  2  Invalid syntax

The gof executable accepts identical commands and options.
See docs/USAGE.md for configuration, automation, and Git recovery."
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
