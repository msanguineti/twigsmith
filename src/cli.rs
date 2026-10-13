use std::{env, process::ExitCode};

use crate::{
    config_cli, finish_cli,
    repository::{self, InspectionError, Observation},
    save_cli, start_cli, status,
};

pub fn run() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();

    match args.as_slice() {
        [] => print_help(),
        [arg] if arg == "--help" || arg == "-h" => print_help(),
        [arg] if arg == "--version" || arg == "-V" => {
            println!("git-oneflow {}", env!("CARGO_PKG_VERSION"));
        }
        [arg] if arg == "status" => return run_status(),
        [command, arg] if command == "status" && (arg == "--help" || arg == "-h") => {
            print_status_help();
        }
        [command, rest @ ..] if command == "start" || command == "s" => {
            return start_cli::run(rest);
        }
        [command, rest @ ..] if command == "save" || command == "v" => {
            return save_cli::run(rest);
        }
        [command, rest @ ..] if command == "config" => {
            return config_cli::run(rest);
        }
        [command, rest @ ..] if command == "finish" || command == "f" => {
            return finish_cli::run(rest);
        }
        _ => {
            eprintln!("error: unsupported arguments; use --help for available options");
            return ExitCode::from(2);
        }
    }

    ExitCode::SUCCESS
}

fn print_help() {
    println!(
        "git-oneflow — start workstreams, save progress, and publish finished work

Usage: git-oneflow [OPTIONS]
       git-oneflow <COMMAND> [ARGS]

Commands:
  start, s         Create or resume a workstream
  save, v          Preserve progress and publish the current branch
  finish, f        Deliver finished work for review or integration
  status           Summarize local repository state
  config           Set up or inspect shared settings

Options:
  -h, --help       Show help
  -V, --version    Show version

Examples:
  gof start add-user-avatars
  gof save
  gof finish
  gof status
  gof config show

The gof executable accepts the same commands and options.
Use <command> --help for command details."
    );
}

fn print_status_help() {
    println!(
        "Summarize local Git repository state.

Usage: git-oneflow status

Options:
  -h, --help       Show help

Examples:
  gof status

Details:
  Report HEAD/branch, change counts, active operations, and upstream information.
  Run from a repository root or nested directory; bare repositories are supported.
  A path can count as both staged and unstaged. Ignored paths are excluded.
  Upstream comparisons use locally available references/history only.
  Unavailable comparison data is reported explicitly.

Exit status:
  0  Observation completed
  1  Repository or inspection failure
  2  Invalid syntax"
    );
}

fn run_status() -> ExitCode {
    let directory = match env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!(
                "error: cannot determine current directory: {}",
                status::display(error.to_string().as_bytes())
            );
            return ExitCode::FAILURE;
        }
    };
    match repository::inspect(&directory) {
        Ok(Observation::Repository(state)) => {
            status::print(&state);
            ExitCode::SUCCESS
        }
        Ok(Observation::OutsideRepository) => {
            eprintln!("error: current directory is outside a Git repository");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("error: {}", status::display(error.to_string().as_bytes()));
            match &error {
                InspectionError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!(
                        "ensure Git is installed and available on PATH, and repository paths exist"
                    );
                }
                InspectionError::Git { output, .. } if !output.stderr.is_empty() => {
                    eprintln!("{}", status::display(&output.stderr).trim_end());
                }
                _ => {}
            }
            ExitCode::FAILURE
        }
    }
}

/// Numeric answers resolve only within a finite menu. Names and messages never
/// pass through this mapping; invalid numbers remain invalid textual answers.
pub(crate) fn menu_answer(answer: String, choices: &[&str]) -> String {
    answer
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| choices.get(index))
        .map(|choice| (*choice).into())
        .unwrap_or(answer)
}

/// Cache observations do not change confirmed remote outcomes. Keep normal
/// completion brief and explain only refresh problems requiring attention.
pub(crate) fn cache_issues(
    caches: &[crate::save::publication::CacheReport],
    present: impl Fn(&[u8]) -> String,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for cache in caches.iter().filter(|cache| cache.has_issue()) {
        let _ = writeln!(
            out,
            "\nLocal tracking cache needs attention for {} on {}.",
            present(&cache.reference),
            present(cache.remote.as_bytes())
        );
        for issue in &cache.issues {
            let _ = writeln!(out, "  {}", present(issue.as_bytes()));
        }
        for reference in &cache.refs {
            if let Some(issue) = &reference.issue {
                let _ = writeln!(
                    out,
                    "  {}: {}",
                    present(&reference.reference),
                    present(issue.as_bytes())
                );
            }
        }
    }
    if !out.is_empty() {
        out.push_str("Confirmed remote effects are retained. Inspect the listed local refs and remote fetch mappings before relying on status or log comparisons.\n");
    }
    out
}

/// Keep Git's diagnostic lines readable while escaping terminal controls within
/// each line. The caller supplies its credential presentation policy.
pub(crate) fn git_diagnostics(bytes: &[u8], present: impl Fn(&[u8]) -> String) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if !bytes.is_empty() {
        for line in bytes.split(|byte| *byte == b'\n') {
            let _ = writeln!(out, "  Git: {}", present(line));
        }
    }
    out
}
