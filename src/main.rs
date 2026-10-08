mod start_cli;
mod status;

use std::{env, process::ExitCode};

use git_oneflow::repository::{self, InspectionError, Observation};

fn main() -> ExitCode {
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
        [command, rest @ ..] if command == "start" => return start_cli::run(rest),
        _ => {
            eprintln!("error: unsupported arguments; use --help for available options");
            return ExitCode::from(2);
        }
    }

    ExitCode::SUCCESS
}

fn print_help() {
    println!(
        "git-oneflow — start work and observe Git repository state

Usage: git-oneflow [OPTIONS]
       git-oneflow status
       git-oneflow start feature <description> [--name <branch>]

Commands:
  start            Create or resume ordinary work (feature)
  status           Summarize observed repository state (read-only)

Options:
  -h, --help       Show this help
  -V, --version    Show the development version

Status compares locally available references/history only; it does not fetch.
Use start --help or start feature --help for workflow help."
    );
}

fn print_status_help() {
    println!(
        "Usage: git-oneflow status

Report HEAD/branch, working-tree change counts, operations, and upstream information.
Run from a repository root or nested directory. Bare repositories have no working tree.
Comparisons use locally available references/history only; no fetch is performed.
Observations do not assess remote freshness or workflow readiness.

  -h, --help    Show this help"
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
