//! Save grammar and bounded terminal interaction. Policies and effects live in save.
use std::{
    env,
    ffi::OsString,
    fmt::Write as _,
    io::{self, IsTerminal},
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
    settings::{SaveKind, SaveOperation, SavePlan, SaveScope},
    status::display,
};

fn text(value: impl std::fmt::Display) -> String {
    display(value.to_string().as_bytes())
}

fn parse(args: &[OsString]) -> Result<Request, String> {
    let mut request = Request::default();
    let mut operations = Vec::new();
    let mut publish_only = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--publish-only") => {
                if publish_only {
                    return Err("duplicate --publish-only".into());
                }
                publish_only = true;
            }
            Some("--commit" | "--stash") => {
                let action = if arg == "--commit" {
                    SaveKind::Commit
                } else {
                    SaveKind::Stash
                };
                let scope = match args.next().and_then(|value| value.to_str()) {
                    Some("all") => SaveScope::All,
                    Some("staged") => SaveScope::Staged,
                    Some("unstaged") => SaveScope::Unstaged,
                    _ => {
                        return Err(format!(
                            "{} requires all, staged, or unstaged",
                            arg.to_string_lossy()
                        ));
                    }
                };
                operations.push(SaveOperation { action, scope });
            }
            Some("--message" | "--stash-message") => {
                let label = if arg == "--message" {
                    &mut request.message
                } else {
                    &mut request.stash_message
                };
                if label.is_some() {
                    return Err(format!("duplicate {}", arg.to_string_lossy()));
                }
                let message = args
                    .next()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| {
                        format!("{} requires a literal UTF-8 value", arg.to_string_lossy())
                    })?;
                if message.contains('\0') || message.trim().is_empty() {
                    return Err(format!(
                        "{} requires a nonblank value without NUL",
                        arg.to_string_lossy()
                    ));
                }
                *label = Some(message.to_owned());
            }
            Some("--staged" | "--wip") => {
                return Err(
                    "retired save action; use --commit staged or --commit staged --commit unstaged"
                        .into(),
                );
            }
            _ => return Err("unknown or malformed save argument".into()),
        }
    }
    if publish_only && !operations.is_empty() {
        return Err("--publish-only cannot be combined with --commit or --stash".into());
    }
    request.plan = if publish_only {
        Some(SavePlan::PublishOnly)
    } else if operations.is_empty() {
        None
    } else {
        Some(SavePlan::Operations(operations))
    };
    if let Some(plan) = &request.plan {
        plan.validate()?;
        if request.message.is_some() && !uses(plan, SaveKind::Commit) {
            return Err("--message requires a plan containing a commit action".into());
        }
        if request.stash_message.is_some() && !uses(plan, SaveKind::Stash) {
            return Err("--stash-message requires a plan containing a stash action".into());
        }
    }
    Ok(request)
}

fn uses(plan: &SavePlan, action: SaveKind) -> bool {
    matches!(plan, SavePlan::Operations(operations) if operations.iter().any(|operation| operation.action == action))
}

pub fn run(args: &[OsString]) -> ExitCode {
    if matches!(args, [arg] if arg == "--help" || arg == "-h") {
        help();
        return ExitCode::SUCCESS;
    }
    let mut request = match parse(args) {
        Ok(request) => request,
        Err(error) => return syntax_error(&error),
    };
    let directory = match env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            crate::presentation::error(format!(
                "error: cannot determine current directory: {}",
                text(error)
            ));
            return ExitCode::FAILURE;
        }
    };
    let mut plan = match save::assess(&directory, &request) {
        Ok(plan) => plan,
        Err(error) => return assessment_error(&error),
    };
    if plan.required_input != RequiredInput::None {
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            crate::presentation::error(
                "error: save requires an action; no interactive terminal is available",
            );
            eprintln!(
                "select --commit <scope>, --stash <scope>, or --publish-only; or configure save-plan in git-oneflow.config.toml"
            );
            return ExitCode::FAILURE;
        }
        scope_summary(&plan);
        let approved = plan.clone();
        let input = (|| -> Result<(), InputError> {
            request.plan = Some(select_plan()?);
            plan = resolve(&directory, &approved, &request)?;
            Ok(())
        })();
        if let Err(error) = input {
            match error {
                InputError::Save(error) => return assessment_error(&error),
                InputError::Cancelled => eprintln!("Save cancelled; no repository changes made."),
                InputError::InvalidChoice => crate::presentation::error(
                    "error: invalid save choice; no repository changes made; retry to choose again",
                ),
                InputError::Io(error) => crate::presentation::error(format!(
                    "error: cannot read save input: {}; no repository changes made",
                    text(error)
                )),
            }
            return ExitCode::FAILURE;
        }
    }
    let report = execution::execute(&directory, &plan);
    let output = render(&plan, &report);
    crate::presentation::report(&output, report.is_complete());
    if report.is_complete() {
        ExitCode::SUCCESS
    } else {
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
fn select(prompt: &str, choices: &[(&str, &str)]) -> Result<String, InputError> {
    let value = crate::presentation::select_described(prompt, choices, None)
        .map_err(InputError::Io)?
        .ok_or(InputError::Cancelled)?;
    if value == "cancel" {
        return Err(InputError::Cancelled);
    }
    Ok(value)
}
fn select_plan() -> Result<SavePlan, InputError> {
    let choice = select(
        "How would you like to save this work?",
        &[
            ("commit-all", "Commit staged and unstaged work together"),
            (
                "stash-all",
                "Stash staged and unstaged work together locally",
            ),
            ("staged-first", "Choose each group separately, staged first"),
            (
                "unstaged-first",
                "Choose each group separately, unstaged first",
            ),
            (
                "publish-only",
                "Publish existing commits; keep uncommitted work local",
            ),
            ("cancel", "Cancel"),
        ],
    )?;
    let together = |action| {
        SavePlan::Operations(vec![SaveOperation {
            action,
            scope: SaveScope::All,
        }])
    };
    match choice.as_str() {
        "commit-all" => Ok(together(SaveKind::Commit)),
        "stash-all" => Ok(together(SaveKind::Stash)),
        "publish-only" => Ok(SavePlan::PublishOnly),
        "staged-first" | "unstaged-first" => {
            let scopes = if choice == "staged-first" {
                [SaveScope::Staged, SaveScope::Unstaged]
            } else {
                [SaveScope::Unstaged, SaveScope::Staged]
            };
            let mut operations = Vec::new();
            for scope in scopes {
                let prompt = if scope == SaveScope::Staged {
                    "How should staged changes be preserved?"
                } else {
                    "How should unstaged changes be preserved?"
                };
                let action = select(
                    prompt,
                    &[
                        ("commit", "Commit this group, then publish branch commits"),
                        ("stash", "Stash this group locally"),
                        ("leave", "Leave this group local"),
                        ("cancel", "Cancel the whole save"),
                    ],
                )?;
                match action.as_str() {
                    "commit" => operations.push(SaveOperation {
                        action: SaveKind::Commit,
                        scope,
                    }),
                    "stash" => operations.push(SaveOperation {
                        action: SaveKind::Stash,
                        scope,
                    }),
                    "leave" => {}
                    _ => return Err(InputError::InvalidChoice),
                }
            }
            if operations.is_empty() {
                Ok(SavePlan::PublishOnly)
            } else {
                Ok(SavePlan::Operations(operations))
            }
        }
        _ => Err(InputError::InvalidChoice),
    }
}

fn resolve(directory: &Path, approved: &Plan, request: &Request) -> Result<Plan, SaveError> {
    // Input resolves the same approved scope, never silently approves a new one.
    save::revalidate(directory, approved)?;
    let resolved = save::assess(directory, request)?;
    save::revalidate(directory, approved)?;
    Ok(resolved)
}
fn scope_summary(plan: &Plan) {
    crate::presentation::section("Save work");
    crate::presentation::prose(&format!(
        "Branch: {} · Publish to: {}\n{} staged, {} unstaged tracked, {} untracked paths (counts may overlap).",
        display(&plan.branch),
        text(&plan.settings.publish_remote),
        plan.scope.staged.len(),
        plan.scope.remaining_tracked.len(),
        plan.scope.untracked.len()
    ));
    crate::presentation::prose("Ignored untracked files stay local.");
}
fn syntax_error(error: &str) -> ExitCode {
    crate::presentation::error(format!("error: {error}; use save --help (alias: v)"));
    ExitCode::from(2)
}
fn assessment_error(error: &SaveError) -> ExitCode {
    crate::presentation::error(format!("error: {}", text(error)));
    match error {
        SaveError::Inspection(error) | SaveError::InvalidRemote { source: error, .. } => {
            inspection_diagnostics(error);
        }
        SaveError::Settings(_) => eprintln!(
            "check the reported configuration file; run gof config show to inspect settings and their sources"
        ),
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
                eprint!("{}", crate::cli::git_diagnostics(&output.stderr, display));
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
                    out.push_str(&crate::cli::git_diagnostics(bytes, display));
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
                        out.push_str(&crate::cli::git_diagnostics(bytes, display));
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
fn local_state(out: &mut String, observation: &LocalObservation, report: &Report, branch: &[u8]) {
    match &observation.repository {
        Ok(Observation::Repository(state)) => {
            let published_head = report.publication.as_ref().is_some_and(|publication| {
                matches!(&state.head, Head::Attached { branch: observed, commit }
                    if observed == branch && commit == &publication.commit)
            });
            if !report.is_complete() || !published_head {
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
                    "Work still local: {staged} staged, {remaining} unstaged tracked, {untracked} untracked, {conflicts} conflicted paths (counts may overlap)."
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
        Ok(Some(commit)) if !report.is_complete() => {
            let _ = writeln!(
                out,
                "Observed local stash: {} (contents not remotely published).",
                display(commit)
            );
        }
        Ok(_) => {}
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
    if !report.effects.is_empty() {
        out.push('\n');
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
            Effect::LocalStash { commit, message } => {
                let _ = writeln!(
                    out,
                    "Retained local stash: {} (label: {}; contents not remotely published).",
                    display(commit),
                    text(message)
                );
                let _ = writeln!(
                    out,
                    "Inspect git stash show {}. Restore this artifact in a separate directory with git worktree add --detach <new-directory> {}^1, then git -C <new-directory> stash apply --index {}.",
                    display(commit),
                    display(commit),
                    display(commit)
                );
            }
        }
    }
    if report
        .effects
        .iter()
        .filter(|effect| matches!(effect, Effect::LocalStash { .. }))
        .count()
        > 1
    {
        let _ = writeln!(
            out,
            "To recover both groups together, create one worktree at the staged stash's first parent; apply the staged stash first, then the unstaged stash, both with --index, regardless of their creation order."
        );
    }
    if report
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::LocalStash { .. }))
    {
        let _ = writeln!(
            out,
            "Applying a staged stash over remaining unstaged edits can conflict. Separate-worktree recovery preserves the current checkout; save keeps the stash entries until you remove them."
        );
    }
    out.push('\n');
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
                "Branch publication: {} to remote {} — {state}.\n  Commit: {}",
                display(&publication.branch_ref),
                text(&publication.remote),
                display(&publication.commit)
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
            out.push_str(&crate::cli::cache_issues(
                &publication.caches,
                display,
                !report.is_complete(),
            ));
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
                Operation::Stash { .. } => "Local stash process",
                Operation::PrepareIndex { .. } => "Group preparation process",
                Operation::Reconcile { .. } => "Remaining work reconciliation process",
            };
            diagnostics(&mut out, label, &command.result);
        }
    }
    if let Some(recovery) = &report.recovery
        && recovery.retained
    {
        let _ = writeln!(
            out,
            "\nRetained save recovery: {} at {}.",
            display(&recovery.commit),
            text(&recovery.reference)
        );
        let _ = writeln!(
            out,
            "This snapshot preserves the original staged/unstaged work. Inspect git stash show {}. Reconstruct the original work in a separate directory with git worktree add --detach <new-directory> {}^1, then git -C <new-directory> stash apply --index {}. Recovery references are listed by git for-each-ref refs/git-oneflow/save-recovery/; remove one only after recovery is verified.",
            display(&recovery.commit),
            display(&recovery.commit),
            display(&recovery.commit)
        );
    }
    out.push('\n');
    local_state(&mut out, &report.after, report, &plan.branch);
    if !report.is_complete() {
        let _ = writeln!(
            out,
            "\nNo rollback was attempted. Failed/interrupted Git steps can leave additional effects.\nResolve the reported failure and rerun save from the observed state.\nCheck any unconfirmed push destination before retrying; completed local checkpoints are retained."
        );
    }
    out
}

fn help() {
    println!(
        r#"Preserve selected work and publish the current branch. Alias: v.

Usage: git-oneflow save [ACTION ...] [LABELS]

Options:
  --commit <scope>         Commit all, staged, or unstaged changes
  --stash <scope>          Stash all, staged, or unstaged changes locally
  --publish-only          Publish existing commits; keep uncommitted work local
  --message <message>     Override the commit message
  --stash-message <label> Override the local stash label
  -h, --help              Show help

Examples:
  gof save
  gof v --commit all
  gof save --commit staged --stash unstaged
  gof save --stash staged --commit unstaged --message "Preserve progress"
  gof save --stash staged --stash unstaged

Notes:
  Repeat --commit or --stash to select separate staged and unstaged artifacts.
  Operations run in the supplied order. all selects both groups in one artifact;
  all overlaps either separate group. Duplicate and overlapping selections fail.
  --publish-only is exclusive. Action flags replace the configured plan as a whole.
  Label-only options retain the configured plan; each requires its action type.
  A label applies to every artifact of that type. Separate groups append
  " [staged]" or " [unstaged]"; all uses the label unchanged. Git's normal
  commit-message cleanup, hooks, and signing remain active. Stash labels stay literal.
  Empty selected groups are skipped; remaining branch commits are still published.
  Dirty trees default to ask; clean trees publish without asking. Resolved plans
  use useful default labels and need no confirmation or message-entry prompt.
  Use arrow keys and Enter, or type a displayed name or number. Invalid choices
  stay correctable. Esc, Ctrl-C/Ctrl-D, end-of-input, or cancel stops unresolved
  selection without save effects. Prompts require terminal input and stderr.
  Changes refer to the original index and working tree. Unselected work and ignored
  untracked files remain local. Inseparable changes fail before preservation.
  Stashes remain local; reports give object identities for native Git recovery.
  Publication creates or fast-forwards the same branch on publish-remote without
  fetching the project base. Existing valid tracking is preserved; absent tracking
  is set after confirmed publication. Conflicts, active Git operations, dirty
  submodules and conflicting routing block save. Failures retain completed effects.

Settings:
  save-plan: ["ask"] (default), ["publish-only"], or ordered action:scope entries,
    for example ["commit:staged", "stash:unstaged"].
  save-message and save-stash-message: nonblank literal default labels.
  publish-remote: origin; base-remote follows publication; base: main (defaults).
  Run gof config for setup or gof config show to inspect effective settings.

Exit status:
  0  Selected plan completed, including unchanged publication
  1  Failure, unresolved input, cancellation, or incomplete execution
  2  Invalid syntax"#
    );
}
