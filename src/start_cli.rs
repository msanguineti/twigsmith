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
    let mut input = None;
    let mut options = start::Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--policy") if options.policy.is_none() => {
                let Some(value) = args
                    .next()
                    .and_then(|v| v.to_str())
                    .and_then(|v| crate::settings::WorkstreamKind::parse(v).ok())
                else {
                    return syntax_error();
                };
                options.policy = Some(value);
            }
            Some("--from") if options.from.is_none() => {
                let Some(value) = args.next().and_then(|v| v.to_str()) else {
                    return syntax_error();
                };
                options.from = Some(value.into());
            }
            Some("--from-tag") if options.from_tag.is_none() => {
                let Some(value) = args.next().and_then(|v| v.to_str()) else {
                    return syntax_error();
                };
                options.from_tag = Some(value.into());
            }
            Some("--target") if options.target.is_none() => {
                let Some(value) = args.next().and_then(|v| v.to_str()) else {
                    return syntax_error();
                };
                options.target = Some(value.into());
            }
            Some(value) if input.is_none() && !value.starts_with('-') => input = Some(value),
            _ => return syntax_error(),
        }
    }
    let Some(input) = input else {
        return syntax_error();
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
    let request = match start::prepare_with_options(&directory, input, &options) {
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
            match outcome.action {
                Action::Create if request.source_commit.is_some() => crate::presentation::report(
                    &format!(
                        "{action} branch {} from commit {}",
                        text(&request.target),
                        display(request.source_commit.as_ref().unwrap())
                    ),
                    true,
                ),
                Action::Create => crate::presentation::report(
                    &format!(
                        "{action} branch {} from {}/{}",
                        text(&request.target),
                        text(&request.settings.base_remote),
                        text(&request.settings.base)
                    ),
                    true,
                ),
                Action::ResumeRemote => crate::presentation::report(
                    &format!(
                        "{action} branch {} from {}/{}",
                        text(&request.target),
                        text(&request.settings.publish_remote),
                        text(&request.target)
                    ),
                    true,
                ),
                _ => crate::presentation::report(
                    &format!("{action} branch {}", text(&request.target)),
                    true,
                ),
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            failure_error(&failure, &request.target);
            ExitCode::FAILURE
        }
    }
}

fn syntax_error() -> ExitCode {
    crate::presentation::error(
        "error: expected start <branch-name> (alias: s); supply exactly one branch name.\nUse --from <ref> for a creation source; use --help or start --help for options",
    );
    ExitCode::from(2)
}

fn help_start() {
    println!(
        r#"Create or resume an exact work branch. Alias: s.

Usage: twigsmith start <branch-name> [OPTIONS]

Arguments:
  <branch-name>                      One literal Git branch name

Options:
  --policy <ordinary|release|hotfix>  Select and retain explicit work policy
  --from <commit-ref>                Create from a locally available commit
  --from-tag <tag>                   Create hotfix work from an exact local tag
  --target <branch>                  Set an ordinary contribution target
  -h, --help                         Show help

Examples:
  tgi start add-user-avatars
  tgi s fix/parser_v2
  tgi start 中文/修复
  tgi start release/winter --policy release --from main~1
  tgi start hotfix/parser --policy hotfix --from-tag v1.2

Notes:
  Names retain spelling, case, punctuation and Unicode form. Spaces are invalid,
  including inside shell quotes. Supply one name; start performs no conversion.
  Exact local names resume offline. Otherwise start resumes from publish-remote
  or creates from the fresh base on base-remote. --from and --from-tag are alternatives.
  Sources must exist locally. New hotfix work needs --from-tag or a configured tag source.
  Resume preserves history and established policy; incompatible source/route input fails.
  Switching requires a clean tree and attached HEAD with a commit. An already-current
  branch succeeds with local changes. Active Git operations and occupied worktrees block start.

Settings:
  publish-remote: origin; base-remote follows publication; base: main (defaults).
  Run tgi config for setup or tgi config show to inspect effective settings.

Exit status:
  0  Created, resumed, or already current
  1  Input, repository, settings, or workflow failure
  2  Invalid syntax"#
    );
}

fn inspection_error(error: &InspectionError) {
    match error {
        InspectionError::Git { output, .. } if !output.stderr.is_empty() => {
            eprint!("{}", crate::cli::git_diagnostics(&output.stderr, display));
        }
        InspectionError::Io(_) => {
            eprintln!("ensure Git is installed on PATH and repository paths are accessible");
        }
        _ => {}
    }
}

fn resolve_error(error: &ResolveError) {
    crate::presentation::error(format!("error: {}", text(error)));
    resolve_guidance(error);
}

fn resolve_guidance(error: &ResolveError) {
    match error {
        ResolveError::Repository(error) => inspection_error(error),
        ResolveError::OutsideRepository => eprintln!("enter a Git working tree and retry"),
        ResolveError::Settings(crate::settings::SettingsError::Repository(error)) => {
            inspection_error(error);
        }
        ResolveError::Settings(_) => {
            eprintln!(
                "check the reported configuration file; run tgi config show to inspect settings and their sources"
            );
        }
        ResolveError::Input(_) => {}
        ResolveError::ProtectedBase(_) => {
            eprintln!("choose a work branch distinct from the configured base or release marker")
        }
        ResolveError::InvalidTarget(_) => {
            eprintln!(
                "use one literal Git branch name without spaces, such as fix/parser; shell quotes preserve the name"
            );
        }
    }
}

fn failure_error(failure: &Failure, target: &str) {
    if let StartError::Git { arguments, output } = &failure.error {
        crate::presentation::error(format!(
            "error: Git refused {} ({})",
            text(format!("{arguments:?}")),
            output.status
        ));
        for bytes in [&output.stdout, &output.stderr] {
            eprint!("{}", crate::cli::git_diagnostics(bytes, display));
        }
    } else {
        crate::presentation::error(format!("error: {}", text(&failure.error)));
    }
    match &failure.error {
        StartError::Inspection(error) => inspection_error(error),
        StartError::Resolve(error) => resolve_guidance(error),
        StartError::Io(_) => {
            eprintln!("ensure Git is installed on PATH and repository paths are accessible")
        }
        StartError::Git { .. } => eprintln!(
            "check Git diagnostics, repository access, and the configured publication remote and project base in repository settings"
        ),
        _ => {}
    }
    eprintln!("\nCompleted steps: {}", failure.effects.len());
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
            Effect::PreviousSelectionCleared => eprintln!(
                "  Cleared previous integration selection; retained attempts remain available"
            ),
            Effect::PolicyEstablished { policy } => {
                eprintln!("  Established {} policy", text(policy))
            }
            Effect::Switched { target } => eprintln!("  Switched to {}", text(target)),
        }
    }
    eprintln!("\nRemaining state (observed after failure):");
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
        "\nNo rollback was attempted. Resolve the reported problem and retry from the observed state."
    );
}

fn reference_state(label: &str, result: &Result<Option<Vec<u8>>, StartError>) {
    match result {
        Ok(Some(commit)) => eprintln!("  {label}: {}", display(commit)),
        Ok(None) => eprintln!("  {label}: absent"),
        Err(error) => eprintln!("  {label}: unavailable ({})", text(error)),
    }
}
