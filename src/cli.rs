use std::{env, process::ExitCode};

use crate::{
    config_cli,
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
        _ => {
            eprintln!("error: unsupported arguments; use --help for available options");
            return ExitCode::from(2);
        }
    }

    ExitCode::SUCCESS
}

fn print_help() {
    println!(
        "git-oneflow — start workstreams, save progress, and inspect Git state

Usage: git-oneflow [OPTIONS]
       git-oneflow <COMMAND> [ARGS]

Commands:
  start, s         Create or resume a workstream
  save, v          Preserve progress and publish the current branch
  status           Summarize local repository state
  config           Set up or inspect shared settings

Options:
  -h, --help       Show help
  -V, --version    Show version

Examples:
  gof start \"add user avatars\"
  gof save
  gof status
  gof config show

The gof executable accepts the same commands and options.
Use <command> --help for command details. See docs/USAGE.md for the user guide."
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
  2  Invalid syntax

See docs/USAGE.md for settings and recovery."
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
