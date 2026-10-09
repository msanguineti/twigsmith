//! Finish grammar, terminal input, and presentation. Plans decide which choices
//! are relevant; preparation and publication own all workflow effects and guards.
use crate::{
    finish::{
        self, Constraint, FinishError, Plan, Request, RequiredInput, preparation, publication,
    },
    repository::{self, Head, InspectionError, Observation},
    settings::{FinishRewrite, FinishUpdate},
    status::display,
};
use std::{
    env,
    ffi::OsString,
    fmt::Write as _,
    io::{self, IsTerminal, Write as _},
    path::Path,
    process::ExitCode,
};

fn text(value: impl std::fmt::Display) -> String {
    display(value.to_string().as_bytes())
}
fn parse(args: &[OsString]) -> Result<Request, &'static str> {
    let mut request = Request::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
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
    Ok(request)
}

pub fn run(args: &[OsString]) -> ExitCode {
    if matches!(args, [arg] if arg == "--help" || arg == "-h") {
        help();
        return ExitCode::SUCCESS;
    }
    let mut request = match parse(args) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("error: {error}; use finish --help (alias: f)");
            return ExitCode::from(2);
        }
    };
    let directory = match env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("error: cannot determine current directory: {}", text(error));
            return ExitCode::FAILURE;
        }
    };
    let initial = match finish::assess(&directory, &request, None) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {}", text(&error));
            error_diagnostics(&error);
            let mut output = String::new();
            remaining(&mut output, &repository::inspect(&directory));
            output.push_str("Branch publication not attempted.\n");
            eprint!("{output}");
            return ExitCode::FAILURE;
        }
    };
    let refresh = preparation::refresh(&directory, &initial);
    let Some(mut plan) = refresh.plan.clone().filter(|_| refresh.error.is_none()) else {
        preparation_failure(&refresh, None);
        return ExitCode::FAILURE;
    };
    loop {
        if !plan.constraints.is_empty() {
            let mut output = String::new();
            for constraint in &plan.constraints {
                constraint_message(&mut output, constraint);
            }
            preparation_failure(&refresh, Some(&output));
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
            display(&plan.branch),
            display(&plan.commit),
            text(&plan.settings.remote),
            text(&plan.settings.base)
        );
        let answer = match prompt(&plan, choice) {
            Ok(answer) => answer,
            Err(error) => {
                preparation_failure(&refresh, Some(&format!("{error}\n")));
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
                preparation_failure(&refresh, Some(&format!("error: {}\n", text(&error))));
                error_diagnostics(&error);
                return ExitCode::FAILURE;
            }
        };
    }
    let mut prepared = preparation::prepare(&directory, &plan);
    if !prepared.is_prepared() {
        let mut effects = refresh.effects.clone();
        effects.append(&mut prepared.effects);
        prepared.effects = effects;
        preparation_failure(&prepared, None);
        return ExitCode::FAILURE;
    }
    let plan = prepared.plan.as_ref().expect("prepared plan");
    let published = publication::publish(&directory, plan);
    if published.is_complete() {
        println!(
            "Published {} to {} at {}.",
            display(&plan.branch),
            text(&published.remote),
            display(&published.commit)
        );
        for destination in &published.destinations {
            println!(
                "  {}: {}",
                display(&destination.destination),
                destination_state(destination.state)
            );
        }
        println!(
            "Next: submit {} for review into {}.",
            display(&plan.branch),
            text(&plan.settings.base)
        );
        ExitCode::SUCCESS
    } else {
        let mut output = String::new();
        let _ = writeln!(
            output,
            "Finish incomplete for {} at {} to {}.",
            display(&plan.branch),
            display(&published.commit),
            text(&published.remote)
        );
        preparation_effects(&mut output, &refresh);
        preparation_effects(&mut output, &prepared);
        if let Some(error) = &published.error {
            let _ = writeln!(output, "error: {}", text(error));
        }
        for destination in &published.destinations {
            let _ = writeln!(
                output,
                "  {}: {}",
                display(&destination.destination),
                destination_state(destination.state)
            );
            if let Some(error) = &destination.error {
                let _ = writeln!(output, "  error: {}", text(error));
            }
            command_diagnostics(&mut output, &destination.commands);
        }
        tracking(&mut output, &published.tracking);
        remaining(&mut output, &published.remaining);
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
fn prompt(plan: &Plan, choice: RequiredInput) -> Result<String, String> {
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
                        display(&destination.destination),
                        display(oid)
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
    io::stderr()
        .flush()
        .map_err(|e| format!("error: cannot write finish prompt: {}", text(e)))?;
    let mut line = String::new();
    let count = io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("error: cannot read finish input: {}", text(e)))?;
    if count == 0 || !line.ends_with('\n') || line.trim() == "cancel" {
        return Err("Finish cancelled before branch execution; finish made no branch, index, or working-tree changes.".into());
    }
    Ok(line.trim().into())
}

fn constraint_message(out: &mut String, constraint: &Constraint) {
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
                out,
                url,
                "history unavailable; check access and fetch complete history",
            );
        }
        Constraint::DestinationAmbiguous(url) => {
            return destination_constraint(
                out,
                url,
                "ambiguous observation; inspect destination configuration",
            );
        }
        Constraint::DestinationHistoryUncertain(url) => {
            return destination_constraint(out, url, "ancestry uncertain; obtain complete history");
        }
        Constraint::RemoteWorkNotIncorporated(url) => {
            return destination_constraint(
                out,
                url,
                "remote work is not incorporated; incorporate it with Git, then rerun finish",
            );
        }
        Constraint::RewriteDenied(url) => {
            return destination_constraint(
                out,
                url,
                "required history replacement denied; use --allow-rewrite to authorize eligible replacement, or choose --update merge or none before preparation",
            );
        }
    };
    let _ = writeln!(out, "error: {message}.");
}
fn destination_constraint(out: &mut String, url: &[u8], message: &str) {
    let _ = writeln!(out, "error: {}: {message}.", display(url));
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
fn preparation_failure(report: &preparation::Report, message: Option<&str>) {
    let mut output = String::from("Finish incomplete.\n");
    if let Some(message) = message {
        output.push_str(message);
    }
    if let Some(error) = &report.error {
        let _ = writeln!(output, "error: {}", text(error));
    }
    preparation_effects(&mut output, report);
    command_diagnostics(&mut output, &report.commands);
    // Input may have changed context since refresh; report an actual new observation.
    let current = report.plan.as_ref().map(|p| {
        repository::inspect(
            p.repository
                .working_tree_root
                .as_deref()
                .expect("working plan"),
        )
    });
    remaining(&mut output, current.as_ref().unwrap_or(&report.remaining));
    let _ = writeln!(output, "Branch publication not attempted.");
    if report.error.is_some() {
        let _ = writeln!(output, "{}", report.guidance());
    }
    eprint!("{output}");
    if let Some(preparation::PreparationError::Finish(error)) = &report.error {
        error_diagnostics(error);
    }
}
fn preparation_effects(out: &mut String, report: &preparation::Report) {
    for effect in &report.effects {
        use preparation::Effect::*;
        match effect {
            Observed {
                destination,
                reference,
                value,
            } => {
                let value = match value {
                    finish::RefObservation::Commit(oid) => display(oid),
                    other => format!("{other:?}"),
                };
                let _ = writeln!(
                    out,
                    "Observed {} at {}: {value}.",
                    display(reference),
                    display(destination)
                );
            }
            Fetched {
                destination,
                commit,
            } => {
                let _ = writeln!(
                    out,
                    "Fetched objects for {} from {}.",
                    display(commit),
                    display(destination)
                );
            }
            RecoveryRecorded(path) => {
                let _ = writeln!(
                    out,
                    "Retained rebase recovery evidence: {}.",
                    text(path.display())
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
                    display(before),
                    display(after)
                );
            }
        }
    }
}
fn command_diagnostics(out: &mut String, commands: &[preparation::CommandReport]) {
    for command in commands {
        match &command.result {
            Ok(result) if !result.status.success() => {
                let _ = writeln!(out, "Git process: {}", result.status);
                for bytes in [&result.stdout, &result.stderr] {
                    if !bytes.is_empty() {
                        let _ = writeln!(out, "Git: {}", display(bytes));
                    }
                }
            }
            Err(error) => {
                let _ = writeln!(out, "Git process: {}", text(error));
            }
            _ => (),
        }
    }
}
fn tracking(out: &mut String, report: &publication::TrackingReport) {
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
                    let _ = writeln!(out, "  {label}: {}", display(value));
                }
            }
        }
        Err(error) => {
            let _ = writeln!(out, "  Tracking observation unavailable: {}", text(error));
        }
    }
}
fn remaining(out: &mut String, observation: &Result<Observation, InspectionError>) {
    let _ = writeln!(out, "Remaining state (observed):");
    match observation {
        Ok(Observation::Repository(state)) => {
            match &state.head {
                Head::Attached { branch, commit } => {
                    let _ = writeln!(out, "  HEAD: {} at {}", display(branch), display(commit));
                }
                Head::Detached { commit } => {
                    let _ = writeln!(out, "  HEAD: detached at {}", display(commit));
                }
                Head::Unborn { branch } => {
                    let _ = writeln!(out, "  HEAD: unborn {}", display(branch));
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
            let _ = writeln!(out, "  Observation unavailable: {}", text(error));
        }
    }
}
fn error_diagnostics(error: &(dyn std::error::Error + 'static)) {
    if let Some(InspectionError::Git { output, .. }) = error.downcast_ref::<InspectionError>()
        && !output.stderr.is_empty()
    {
        eprintln!("Git: {}", display(&output.stderr));
    }
    if let Some(InspectionError::Io(_)) = error.downcast_ref::<InspectionError>() {
        eprintln!("Ensure Git is installed on PATH and repository paths are accessible.");
    }
    if let Some(source) = error.source() {
        error_diagnostics(source);
    }
}
fn help() {
    println!(
        "Prepare and publish the current branch for submission. Alias: f.

Usage: git-oneflow finish [--update <rebase|merge|none>] [--allow-rewrite|--no-rewrite]

Options:
  --update <rebase|merge|none>  Override finish-update
  --allow-rewrite             Authorize eligible published-history replacement
  --no-rewrite                Deny published-history replacement
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
