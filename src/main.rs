use std::{env, process::ExitCode};

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();

    match args.as_slice() {
        [] => print_help(),
        [arg] if arg == "--help" || arg == "-h" => print_help(),
        [arg] if arg == "--version" || arg == "-V" => {
            println!("git-oneflow {}", env!("CARGO_PKG_VERSION"));
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
        "git-oneflow — native Rust development skeleton

Usage: git-oneflow [OPTIONS]

Options:
  -h, --help       Show this help
  -V, --version    Show the development version

Workflow commands are not implemented yet."
    );
}
