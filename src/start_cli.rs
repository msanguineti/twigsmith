//! Argument parsing and human presentation; workflow decisions live in start.
use std::{env, ffi::OsString, process::ExitCode};

use crate::{
    repository::{Head, InspectionError, Observation},
    start::{self, Action, Effect, Failure, ResolveError, StartError},
};

use crate::status::display;

fn text(value: impl std::fmt::Display) -> String {
    display(value.to_string().as_bytes())
}

pub fn run(args: &[OsString]) -> ExitCode {
    match args {
        [help] if help == "--help" || help == "-h" => {
            help_start();
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let [input] = args else {
        return syntax_error();
    };
    let Some(input) = input
        .to_str()
        .filter(|s| !s.starts_with('-') || s.contains(' '))
    else {
        return syntax_error();
    };
    let directory = match env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("error: cannot determine current directory: {}", text(error));
            return ExitCode::FAILURE;
        }
    };
    let request = match start::prepare(&directory, input) {
        Ok(request) => request,
        Err(error) => {
            resolve_error(&error);
            return ExitCode::FAILURE;
        }
    };
    match start::execute(&directory, &request) {
        Ok(outcome) => {
            let action = match outcome.action {
                Action::Create => "Created",
                Action::ResumeLocal => "Resumed local",
                Action::ResumeRemote => "Resumed remote",
                Action::AlreadyCurrent => "Already current:",
            };
            println!("{action} branch {}", text(&request.target));
            ExitCode::SUCCESS
        }
        Err(failure) => {
            failure_error(&failure, &request.target);
            ExitCode::FAILURE
        }
    }
}

fn syntax_error() -> ExitCode {
    eprintln!(
        "error: expected start <branch-name-or-phrase> (alias: s); use --help or start --help"
    );
    ExitCode::from(2)
}

fn help_start() {
    println!(
        "Usage: git-oneflow start <branch-name-or-phrase>
       git-oneflow s <branch-name-or-phrase>
       gof start <branch-name-or-phrase>
       gof s <branch-name-or-phrase>

Create or resume a workstream from a repository working tree.
Valid Git branch names retain their exact spelling, case, and Unicode normalization.
Quote phrases containing ordinary spaces: canonical Unicode lowercase and hyphenated
separators determine the name, without adding a prefix. Other invalid names fail.
The input determines only the target branch; it is not stored as a description.
Prefixes (including release/ and hotfix/) do not select a different workflow.

An existing local branch is resumed without remote contact or history changes.
Otherwise fetch the exact remote counterpart, or create from the fresh remote base.
Report created, resumed, or already current. Switching requires a clean tree;
an already-current branch is a read-only no-op even when dirty. Active Git operations block start.

Settings: <Git common directory>/oneflow.conf (usually .git/oneflow.conf).
Optional remote = <named-remote> and base = <branch>; defaults: origin and main.
No file is required. Linked worktrees share these repository settings.

  -h, --help       Show this help without Git or repository access

Exit status: 0 success/no-op; 1 repository/settings/workflow failure; 2 invalid syntax."
    );
}

fn inspection_error(error: &InspectionError) {
    match error {
        InspectionError::Git { output, .. } if !output.stderr.is_empty() => {
            eprintln!("Git: {}", display(&output.stderr));
        }
        InspectionError::Io(_) => {
            eprintln!("ensure Git is installed on PATH and repository paths are accessible");
        }
        _ => {}
    }
}

fn resolve_error(error: &ResolveError) {
    eprintln!("error: {}", text(error));
    resolve_guidance(error);
}

fn resolve_guidance(error: &ResolveError) {
    match error {
        ResolveError::Repository(error) => inspection_error(error),
        ResolveError::OutsideRepository => eprintln!("enter a Git working tree and retry"),
        ResolveError::SettingsIo { .. } | ResolveError::InvalidSettings(_) => {
            eprintln!(
                "check oneflow.conf in the Git common directory; only remote and base are supported"
            );
        }
        ResolveError::UnusablePhrase
        | ResolveError::InvalidTarget(_)
        | ResolveError::ProtectedBase(_) => {
            eprintln!(
                "use a valid literal Git branch name or a quoted phrase containing ordinary spaces and letters/numbers; choose a target other than the configured base"
            );
        }
    }
}

fn failure_error(failure: &Failure, target: &str) {
    eprintln!("error: {}", text(&failure.error));
    match &failure.error {
        StartError::Inspection(error) => inspection_error(error),
        StartError::Resolve(error) => resolve_guidance(error),
        StartError::Io(_) => {
            eprintln!("ensure Git is installed on PATH and repository paths are accessible")
        }
        StartError::Git { .. } => eprintln!(
            "check Git diagnostics, repository access, and the configured remote/base in oneflow.conf"
        ),
        _ => {}
    }
    eprintln!("Completed steps: {}", failure.effects.len());
    for effect in &failure.effects {
        match effect {
            Effect::Fetched {
                source,
                destination,
            } => eprintln!("  Fetched {} to {}", text(source), text(destination)),
            Effect::BranchCreated { target, commit } => {
                eprintln!("  Created branch {} at {}", text(target), display(commit))
            }
            Effect::UpstreamRemoteSet { remote } => {
                eprintln!("  Set upstream remote {}", text(remote))
            }
            Effect::UpstreamMergeSet { reference } => {
                eprintln!("  Set upstream merge {}", text(reference))
            }
            Effect::Switched { target } => eprintln!("  Switched to {}", text(target)),
        }
    }
    eprintln!("Remaining state (observed after failure):");
    match &failure.remaining.repository {
        Ok(Observation::Repository(state)) => {
            match &state.head {
                Head::Attached { branch, commit } => eprintln!(
                    "  Current branch: {} at {}",
                    display(branch),
                    display(commit)
                ),
                Head::Unborn { branch } => {
                    eprintln!("  Current branch: {} (unborn)", display(branch))
                }
                Head::Detached { commit } => eprintln!("  HEAD: detached at {}", display(commit)),
            }
            if let Some(tree) = &state.working_tree {
                eprintln!(
                    "  Working tree: {}",
                    if tree.is_dirty() {
                        "contains changes"
                    } else {
                        "clean"
                    }
                );
            }
            eprintln!(
                "  Git operations: {}",
                if state.operations.is_active() {
                    "active"
                } else {
                    "none detected"
                }
            );
        }
        Ok(Observation::OutsideRepository) => eprintln!("  Outside a Git repository"),
        Err(error) => eprintln!("  Repository observation unavailable: {}", text(error)),
    }
    reference_state(
        &format!("Target branch {}", text(target)),
        &failure.remaining.target_commit,
    );
    if let Some(reference) = &failure.remaining.fetched_reference {
        reference_state(
            &format!("Fetched reference {}", text(&reference.reference)),
            &reference.commit,
        );
    }
    match &failure.remaining.target_upstream {
        Ok(upstream) => {
            let values = |items: &[Vec<u8>]| {
                if items.is_empty() {
                    "not configured".into()
                } else {
                    items
                        .iter()
                        .map(|s| display(s))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            };
            eprintln!(
                "  Target upstream: remote {}; merge {}",
                values(&upstream.remotes),
                values(&upstream.merge_refs)
            );
        }
        Err(error) => eprintln!("  Target upstream unavailable: {}", text(error)),
    }
    eprintln!(
        "No rollback was attempted. A failed Git step may itself have effects; inspect state before retrying."
    );
}

fn reference_state(label: &str, result: &Result<Option<Vec<u8>>, StartError>) {
    match result {
        Ok(Some(commit)) => eprintln!("  {label}: {}", display(commit)),
        Ok(None) => eprintln!("  {label}: absent"),
        Err(error) => eprintln!("  {label}: unavailable ({})", text(error)),
    }
}
