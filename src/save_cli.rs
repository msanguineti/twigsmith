//! Save grammar and bounded terminal interaction. Policies and effects live in save.
use std::{
    env,
    ffi::OsString,
    fmt::Write as _,
    io::{self, IsTerminal, Write as _},
    path::Path,
    process::ExitCode,
};

use crate::{
    repository::{Change, ChangeKind, Head, InspectionError, Observation},
    save::{
        self, Plan, Request, RequiredInput, SaveError,
        execution::{self, Effect, ExecutionError, LocalObservation, Operation, Report},
        publication::{DestinationState, PublicationState, PublishError, TrackingState},
    },
    settings::SaveAction,
    status::display,
};

fn text(value: impl std::fmt::Display) -> String {
    display(value.to_string().as_bytes())
}

fn parse(args: &[OsString]) -> Result<Request, &'static str> {
    let mut request = Request::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let action = match arg.to_str() {
            Some("--publish-only") => SaveAction::PublishOnly,
            Some("--staged") => SaveAction::Staged,
            Some("--wip") => SaveAction::Wip,
            Some("--stash") => SaveAction::Stash,
            Some("--message") => {
                if request.message.is_some() {
                    return Err("duplicate --message");
                }
                let message = args
                    .next()
                    .and_then(|arg| arg.to_str())
                    .ok_or("--message requires a literal UTF-8 value")?;
                if message.contains('\0') {
                    return Err("--message cannot contain NUL");
                }
                request.message = Some(message.to_owned());
                continue;
            }
            _ => return Err("unknown or malformed save argument"),
        };
        if request.action.replace(action).is_some() {
            return Err("save action flags must appear once and are mutually exclusive");
        }
    }
    if request.message.is_some()
        && request
            .action
            .is_some_and(|action| action != SaveAction::Staged)
    {
        return Err("--message applies only when the effective save action is staged");
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
        Err(error) => return syntax_error(error),
    };
    let directory = match env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("error: cannot determine current directory: {}", text(error));
            return ExitCode::FAILURE;
        }
    };
    let mut plan = match save::assess(&directory, &request) {
        Ok(plan) => plan,
        Err(error) => return assessment_error(&error),
    };
    if plan.required_input != RequiredInput::None {
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            eprintln!(
                "error: save requires {}; no interactive terminal is available",
                if plan.required_input == RequiredInput::Action {
                    "an action"
                } else {
                    "a staged commit message"
                }
            );
            eprintln!(
                "select --publish-only, --staged --message <message>, --wip, or --stash; or configure save-action in git-oneflow.config.toml"
            );
            return ExitCode::FAILURE;
        }
        scope_summary(&plan);
        let approved = plan.clone();
        let input = (|| -> Result<(), InputError> {
            if plan.required_input == RequiredInput::Action {
                for action in &plan.applicable_actions {
                    eprintln!("  {}", choice(*action));
                }
                eprintln!("  cancel       Leave the repository unchanged");
                let actions = plan
                    .applicable_actions
                    .iter()
                    .map(|action| action_name(*action))
                    .chain(std::iter::once("cancel"))
                    .collect::<Vec<_>>()
                    .join("/");
                let action = read_input(&format!("Action ({actions}): "))?;
                if action.trim() == "cancel" {
                    return Err(InputError::Cancelled);
                }
                request.action = Some(
                    plan.applicable_actions
                        .iter()
                        .copied()
                        .find(|candidate| action_name(*candidate) == action.trim())
                        .ok_or(InputError::InvalidChoice)?,
                );
                plan = resolve(&directory, &approved, &request)?;
            }
            if plan.required_input == RequiredInput::Message {
                request.message = Some(read_input("Staged commit message (one line): ")?);
                plan = resolve(&directory, &approved, &request)?;
            }
            Ok(())
        })();
        if let Err(error) = input {
            match error {
                InputError::Save(error) => return assessment_error(&error),
                InputError::Cancelled => eprintln!("Save cancelled; no repository changes made."),
                InputError::InvalidChoice => eprintln!(
                    "error: invalid save choice; no repository changes made; retry to choose again"
                ),
                InputError::Io(error) => eprintln!(
                    "error: cannot read save input: {}; no repository changes made",
                    text(error)
                ),
            }
            return ExitCode::FAILURE;
        }
    }
    let report = execution::execute(&directory, &plan);
    let output = render(&plan, &report);
    if report.is_complete() {
        print!("{output}");
        ExitCode::SUCCESS
    } else {
        eprint!("{output}");
        ExitCode::FAILURE
    }
}

#[derive(Debug)]
enum InputError {
    Cancelled,
    InvalidChoice,
    Io(io::Error),
    Save(SaveError),
}
impl From<SaveError> for InputError {
    fn from(error: SaveError) -> Self {
        Self::Save(error)
    }
}
fn read_input(prompt: &str) -> Result<String, InputError> {
    eprint!("{prompt}");
    io::stderr().flush().map_err(InputError::Io)?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line).map_err(InputError::Io)? == 0 {
        return Err(InputError::Cancelled);
    }
    // EOF must not approve a partial action or message that was never submitted.
    if !line.ends_with('\n') {
        return Err(InputError::Cancelled);
    }
    // Remove only the terminal line ending, retaining literal message whitespace.
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    Ok(line)
}

fn resolve(directory: &Path, approved: &Plan, request: &Request) -> Result<Plan, SaveError> {
    // Input resolves the same approved scope, never silently approves a new one.
    save::revalidate(directory, approved)?;
    let resolved = save::assess(directory, request)?;
    save::revalidate(directory, approved)?;
    Ok(resolved)
}
fn action_name(action: SaveAction) -> &'static str {
    match action {
        SaveAction::Ask => "ask",
        SaveAction::PublishOnly => "publish-only",
        SaveAction::Staged => "staged",
        SaveAction::Wip => "wip",
        SaveAction::Stash => "stash",
    }
}
fn choice(action: SaveAction) -> &'static str {
    match action {
        SaveAction::PublishOnly => {
            "publish-only Publish existing commits; all uncommitted work stays local"
        }
        SaveAction::Staged => {
            "staged       Commit/publish exactly the index; unstaged/untracked work stays local"
        }
        SaveAction::Wip => {
            "wip          Commit/publish staged and remaining work separately; ignored files stay local"
        }
        SaveAction::Stash => {
            "stash        Retain a local stash including untracked work; publish existing commits only"
        }
        SaveAction::Ask => unreachable!("ask is not an applicable execution action"),
    }
}
fn scope_summary(plan: &Plan) {
    eprintln!(
        "Save branch {} to remote {}: {} staged, {} unstaged tracked, {} untracked paths (counts may overlap).",
        display(&plan.branch),
        text(&plan.settings.publish_remote),
        plan.scope.staged.len(),
        plan.scope.remaining_tracked.len(),
        plan.scope.untracked.len()
    );
    eprintln!("Ignored files are excluded. Stash contents are never remotely published.");
}
fn syntax_error(error: &str) -> ExitCode {
    eprintln!("error: {error}; use save --help (alias: v)");
    ExitCode::from(2)
}
fn assessment_error(error: &SaveError) -> ExitCode {
    if matches!(error, SaveError::MessageNotApplicable) {
        return syntax_error("--message applies only when the effective save action is staged");
    }
    eprintln!("error: {}", text(error));
    match error {
        SaveError::Inspection(error) | SaveError::InvalidRemote { source: error, .. } => {
            inspection_diagnostics(error);
        }
        SaveError::Settings(_) => eprintln!(
            "check git-oneflow.config.{{toml,yaml,yml,json}} at the working-tree root and in the Git common directory (legacy: oneflow.conf); supported keys: remote, base, save-action, finish-update, finish-rewrite"
        ),
        SaveError::NoStagedChanges => eprintln!(
            "stage the intended selection yourself, or select --publish-only, --wip, or --stash"
        ),
        SaveError::BlankMessage => {
            eprintln!("supply a nonblank --message for staged work; no editor is launched")
        }
        SaveError::ContextChanged => eprintln!(
            "inspect the changed branch/settings/index/work and rerun save to approve its current scope"
        ),
        SaveError::InvalidUpstream => eprintln!(
            "inspect branch upstream configuration and repository settings; save will not overwrite conflicting tracking settings"
        ),
        _ => eprintln!(
            "inspect repository state with status and resolve the reported blocker before retrying"
        ),
    }
    ExitCode::FAILURE
}
fn inspection_diagnostics(error: &InspectionError) {
    match error {
        InspectionError::Git { output, .. } => {
            if !output.stderr.is_empty() {
                eprintln!("Git: {}", display(&output.stderr));
            }
        }
        InspectionError::Io(_) => {
            eprintln!("ensure Git is installed on PATH and repository paths are accessible")
        }
        _ => {}
    }
}

fn diagnostics(out: &mut String, label: &str, result: &Result<std::process::Output, io::Error>) {
    match result {
        Ok(output) => {
            let _ = writeln!(out, "{label}: {}", output.status);
            for bytes in [&output.stdout, &output.stderr] {
                if !bytes.is_empty() {
                    let _ = writeln!(out, "  Git: {}", display(bytes));
                }
            }
        }
        Err(error) => {
            let _ = writeln!(out, "{label}: {}", text(error));
        }
    }
}
fn save_diagnostics(out: &mut String, error: &SaveError) {
    if let SaveError::Inspection(error) | SaveError::InvalidRemote { source: error, .. } = error {
        match error {
            InspectionError::Git { output, .. } => {
                for bytes in [&output.stdout, &output.stderr] {
                    if !bytes.is_empty() {
                        let _ = writeln!(out, "  Git: {}", display(bytes));
                    }
                }
            }
            InspectionError::Io(_) => {
                let _ = writeln!(
                    out,
                    "Ensure Git is installed on PATH and repository paths are accessible."
                );
            }
            _ => {}
        }
    }
}
fn local_state(out: &mut String, observation: &LocalObservation) {
    match &observation.repository {
        Ok(Observation::Repository(state)) => {
            match &state.head {
                Head::Attached { branch, commit } => {
                    let _ = writeln!(
                        out,
                        "Observed HEAD: {} at {}",
                        display(branch),
                        display(commit)
                    );
                }
                Head::Detached { commit } => {
                    let _ = writeln!(out, "Observed HEAD: detached at {}", display(commit));
                }
                Head::Unborn { branch } => {
                    let _ = writeln!(out, "Observed HEAD: unborn {}", display(branch));
                }
            }
            if let Some(tree) = &state.working_tree {
                let (mut staged, mut remaining, mut untracked, mut conflicts) = (0, 0, 0, 0);
                for change in &tree.changes {
                    match change.kind {
                        ChangeKind::Tracked {
                            staged: s,
                            unstaged: u,
                            ..
                        } => {
                            staged += usize::from(s != Change::Unchanged);
                            remaining += usize::from(u != Change::Unchanged);
                        }
                        ChangeKind::Untracked => untracked += 1,
                        ChangeKind::Conflicted { .. } => conflicts += 1,
                    }
                }
                let _ = writeln!(
                    out,
                    "Work still local: {staged} staged, {remaining} unstaged tracked, {untracked} untracked, {conflicts} conflicted paths (counts may overlap). Ignored files remain excluded."
                );
            }
            if state.operations.is_active() {
                let _ = writeln!(out, "Observed Git operation remains active.");
            }
        }
        Ok(Observation::OutsideRepository) => {
            let _ = writeln!(out, "Repository observation: outside Git.");
        }
        Err(error) => {
            let _ = writeln!(out, "Repository observation unavailable: {}", text(error));
        }
    }
    if let Err(error) = &observation.index {
        let _ = writeln!(out, "Index observation unavailable: {}", text(error));
    }
    match &observation.stash {
        Ok(Some(commit)) => {
            let _ = writeln!(
                out,
                "Observed local stash: {} (contents not remotely published).",
                display(commit)
            );
        }
        Ok(None) => {}
        Err(error) => {
            let _ = writeln!(out, "Stash observation unavailable: {}", text(error));
        }
    }
}
fn render(plan: &Plan, report: &Report) -> String {
    let mut out = String::new();
    if report.is_complete() {
        let _ = writeln!(out, "Save complete for branch {}.", display(&plan.branch));
    } else {
        let _ = writeln!(out, "Save incomplete for branch {}.", display(&plan.branch));
        if let Some(error) = &report.error {
            let _ = writeln!(out, "error: {}", text(error));
            match error {
                ExecutionError::Save(error)
                | ExecutionError::PublicationPreflight(PublishError::Save(error)) => {
                    save_diagnostics(&mut out, error);
                }
                _ => {}
            }
        }
    }
    for effect in &report.effects {
        match effect {
            Effect::Checkpoint { commit, message } => {
                let _ = writeln!(
                    out,
                    "Local checkpoint retained: {} (submitted message: {}).",
                    display(commit),
                    text(message)
                );
            }
            Effect::StagedRemaining { scope } => {
                let _ = writeln!(
                    out,
                    "Staged remaining work: {} tracked and {} untracked paths.",
                    scope.remaining_tracked.len(),
                    scope.untracked.len()
                );
            }
            Effect::LocalStash { commit } => {
                let _ = writeln!(
                    out,
                    "Retained local stash: {} (contents not remotely published).",
                    display(commit)
                );
                let _ = writeln!(
                    out,
                    "Recover when ready with git stash apply --index {} after inspecting git stash list/show; save does not restore or drop it.",
                    display(commit)
                );
            }
        }
    }
    match &report.publication {
        None => {
            let _ = writeln!(
                out,
                "Branch publication not attempted; publication remains unfinished."
            );
        }
        Some(publication) => {
            let state = match publication.state {
                PublicationState::NotAttempted => "not attempted",
                PublicationState::Complete => "confirmed at every destination",
                PublicationState::Partial => {
                    "partly confirmed; other destinations rejected or unconfirmed"
                }
                PublicationState::Rejected => "rejected at every destination",
                PublicationState::Unconfirmed => "unconfirmed; remote effects may exist",
            };
            let _ = writeln!(
                out,
                "Branch publication: {} at {} to remote {} — {state}.",
                display(&publication.branch_ref),
                display(&publication.commit),
                text(&publication.remote)
            );
            for destination in &publication.destinations {
                let state = match destination.state {
                    DestinationState::Created => "created",
                    DestinationState::Advanced => "advanced",
                    DestinationState::Unchanged => "unchanged (already published)",
                    DestinationState::Rejected => "rejected",
                    DestinationState::Unconfirmed => "unconfirmed",
                };
                let _ = writeln!(
                    out,
                    "  {}: {state} ({})",
                    display(&destination.destination),
                    display(&destination.detail)
                );
            }
            if publication.missing_responses > 0 {
                let _ = writeln!(
                    out,
                    "  Destinations without confirmation: {}",
                    publication.missing_responses
                );
            }
            if !publication.is_complete() {
                if let Some(error) = &publication.error {
                    let _ = writeln!(out, "error: {}", text(error));
                    if let PublishError::Save(error) = error {
                        save_diagnostics(&mut out, error);
                    }
                }
                let tracking = match publication.tracking.state {
                    TrackingState::Matching => "matching",
                    TrackingState::Absent => "absent; setup unfinished",
                    TrackingState::Incomplete => "incomplete; setup unfinished",
                    TrackingState::Unavailable => "unavailable; setup unconfirmed",
                };
                let _ = writeln!(out, "Observed upstream tracking: {tracking}.");
                match &publication.tracking.after {
                    Ok(observed) => {
                        let _ = writeln!(
                            out,
                            "  Remote values: {:?}; merge refs: {:?}",
                            observed
                                .upstream
                                .remotes
                                .iter()
                                .map(|v| display(v))
                                .collect::<Vec<_>>(),
                            observed
                                .upstream
                                .merge_refs
                                .iter()
                                .map(|v| display(v))
                                .collect::<Vec<_>>()
                        );
                    }
                    Err(error) => {
                        let _ =
                            writeln!(out, "  Tracking observation unavailable: {}", text(error));
                    }
                }
                if let Some(push) = &publication.push {
                    diagnostics(&mut out, "Push process", push);
                }
            }
        }
    }
    if !report.is_complete() {
        for command in &report.commands {
            let label = match &command.operation {
                Operation::CommitIndex { .. } => "Local commit process",
                Operation::StageRemaining { .. } => "Local staging process",
                Operation::Stash { .. } => "Local stash process",
            };
            diagnostics(&mut out, label, &command.result);
        }
    }
    local_state(&mut out, &report.after);
    if !report.is_complete() {
        let _ = writeln!(
            out,
            "No rollback was attempted. Failed/interrupted Git steps can leave additional effects beyond verified checkpoints. Inspect HEAD, git status, git stash list and upstream settings before retrying from actual state. Check each push destination if publication is partial or unconfirmed; resolve access or history rejection yourself. Retrying save does not create empty checkpoints."
        );
    }
    out
}

fn help() {
    println!(
        "Preserve progress and publish the current branch. Alias: v.

Usage: git-oneflow save [ACTION] [--message <message>]

Options:
  --publish-only       Publish existing commits; keep uncommitted work local
  --staged             Commit the index selection, then publish
  --wip                Checkpoint staged and remaining work separately, then publish
  --stash              Stash tracked/untracked work locally; publish existing commits
  --message <message>  Supply the staged commit message
  -h, --help           Show help

Examples:
  gof save
  gof v --wip
  gof save --staged --message \"selected parser changes\"
  git-oneflow save --stash

Details:
  Choose one action flag. Explicit selection overrides save-action.
  Clean trees publish directly. Dirty trees default to ask.
  Prompts require terminal input and stderr; enter a displayed action or cancel.
  Staged checkpoints need a nonblank message, supplied or requested at a terminal.
  Scripts need an explicit/configured action and any required message.
  WIP messages are [WIP - Staged] and [WIP - Unstaged] for groups with changes.
  Stash contents remain local. Ignored untracked files remain outside checkpoints.
  Publication creates or fast-forwards the same branch on the configured remote.
  Save requires an attached working branch with a commit and matching/absent upstream.
  The configured base, conflicts, active operations, and dirty submodules block save.
  Failures retain completed effects; inspect the report before retrying.

Settings:
  git-oneflow.config.toml, .yaml, .yml, or .json: one file per layer
  Shared: working-tree root; local overrides: Git common directory
  Precedence: explicit action flag > local > shared > defaults
  Legacy local oneflow.conf remains supported when no new local file exists
  remote = origin; base = main; save-action = ask (defaults)
  save-action: ask, publish-only, staged, wip, stash

Exit status:
  0  Selected action completed, including unchanged publication
  1  Failure, unresolved input, cancellation, or incomplete execution
  2  Invalid syntax

See docs/USAGE.md for settings and recovery."
    );
}
