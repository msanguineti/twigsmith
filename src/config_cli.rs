//! Configuration argument grammar and human presentation.
use std::{env, ffi::OsString, path::Path, process::ExitCode};

use crate::{
    config::{self, ConfigError, Format},
    repository::InspectionError,
    settings::{SettingSource, SettingsError},
    status::display,
};

fn text(value: impl std::fmt::Display) -> String {
    display(value.to_string().as_bytes())
}
enum Command {
    Help,
    Guidance,
    Show,
    Generate(Format),
}
fn parse(args: &[OsString]) -> Option<Command> {
    match args {
        [] => return Some(Command::Guidance),
        [arg] if arg == "--help" || arg == "-h" => return Some(Command::Help),
        [arg] if arg == "show" => return Some(Command::Show),
        _ => {}
    }
    let mut yes = false;
    let mut format = None;
    let mut rest = args;
    while let [arg, tail @ ..] = rest {
        if (arg == "--yes" || arg == "-y") && !yes {
            yes = true;
            rest = tail;
        } else if arg == "--format" && format.is_none() {
            let [value, tail @ ..] = tail else {
                return None;
            };
            format = Some(match value.to_str()? {
                "toml" => Format::Toml,
                "yaml" => Format::Yaml,
                "json" => Format::Json,
                _ => return None,
            });
            rest = tail;
        } else {
            return None;
        }
    }
    yes.then_some(Command::Generate(format.unwrap_or(Format::Toml)))
}
pub fn run(args: &[OsString]) -> ExitCode {
    let Some(command) = parse(args) else {
        eprintln!(
            "error: expected config show or config --yes [--format <toml|yaml|json>]; use config --help"
        );
        return ExitCode::from(2);
    };
    match command {
        Command::Help => {
            help();
            return ExitCode::SUCCESS;
        }
        Command::Guidance => {
            println!(
                "Inspect effective settings with gof config show.\nGenerate shared built-in defaults with gof config --yes [--format <toml|yaml|json>].\nGuided setup is not available yet. Use gof config --help for details."
            );
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let directory = match env::current_dir() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("error: cannot determine current directory: {}", text(e));
            return ExitCode::FAILURE;
        }
    };
    let result = match command {
        Command::Show => config::inspect(&directory).map(|inspection| show(&inspection)),
        Command::Generate(format) => config::prepare(&directory, format)
            .and_then(|plan| config::generate(&directory, &plan))
            .map(|generated| {
                println!("Created shared configuration: {}", text(generated.path.display()));
                for path in generated.local_files {
                    println!("Local overrides in {} take precedence over generated values; use gof config show to inspect effective settings.", text(path.display()));
                }
            }),
        Command::Help | Command::Guidance => unreachable!(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", text(&e));
            let inspection = match &e {
                ConfigError::Repository(e)
                | ConfigError::Settings(SettingsError::Repository(e)) => Some(e),
                _ => None,
            };
            match inspection {
                Some(InspectionError::Git { output, .. }) if !output.stderr.is_empty() => {
                    eprintln!("Git: {}", display(&output.stderr).trim_end())
                }
                Some(InspectionError::Io(_)) => {
                    eprintln!("ensure Git is installed on PATH and repository paths are accessible")
                }
                _ => {}
            }
            ExitCode::FAILURE
        }
    }
}
fn source(source: &SettingSource) -> String {
    match source {
        SettingSource::BuiltIn => "built-in default".into(),
        SettingSource::Shared(path) => format!("shared: {}", text(path.display())),
        SettingSource::Local(path) => format!("local: {}", text(path.display())),
    }
}
fn layer(label: &str, file: Option<&Path>, root: Option<&Path>) {
    match (file, root) {
        (Some(file), _) => println!("{label}: {}", text(file.display())),
        (None, Some(root)) => println!("{label}: none (directory: {})", text(root.display())),
        (None, None) => println!("{label}: unavailable (bare repository has no working tree)"),
    }
}
fn show(inspection: &config::Inspection) {
    let loaded = &inspection.loaded;
    let settings = &loaded.settings;
    println!("Effective configuration:");
    println!(
        "  remote: {} ({})",
        text(&settings.remote),
        source(&settings.sources.remote)
    );
    println!(
        "  base: {} ({})",
        text(&settings.base),
        source(&settings.sources.base)
    );
    println!(
        "  save-action: {} ({})",
        settings.save_action.as_str(),
        source(&settings.sources.save_action)
    );
    layer(
        "Shared file",
        loaded.shared.as_deref(),
        inspection.repository.working_tree_root.as_deref(),
    );
    layer(
        "Local file",
        loaded.local.as_deref(),
        Some(&inspection.repository.common_dir),
    );
    println!(
        "Recognized files: git-oneflow.config.{{toml,yaml,yml,json}}; legacy local: oneflow.conf\nPrecedence per key: local overrides > shared settings > built-in defaults.\nSave action flags override save-action when saving."
    );
}
fn help() {
    println!(
        "Inspect configuration or generate shared built-in defaults.

Usage: git-oneflow config show
       git-oneflow config --yes [--format <toml|yaml|json>]
       git-oneflow config

Arguments:
  show             Display effective remote, base, save-action, and their sources

Options:
  -y, --yes        Create shared defaults without interactive input
  --format <toml|yaml|json>
                   Generated format (default: toml; requires --yes)
  -h, --help       Show help

Examples:
  gof config show
  gof config -y
  gof config --yes --format yaml
  git-oneflow config --yes --format json

Details:
  Inspection reports effective values, file locations, and validation errors.
  Precedence per key: explicit save action > local > shared > built-in defaults.
  Run from the root or any nested directory. Bare repositories support inspection.
  Generation creates git-oneflow.config.toml, .yaml, or .json at the working-tree root.
  Generated values: remote = origin; base = main; save-action = ask.
  Private overrides take precedence; generation reports their file locations.
  Every recognized shared file or symlink, including .yml, prevents generation.
  --yes permits unattended creation; existing files are preserved.
  Generation works offline with dirty, unborn, or detached working trees.
  It leaves existing work, the index, refs, and Git configuration unchanged.
  Run config without --yes for guidance; it leaves files unchanged.

Exit status:
  0  Inspection, generation, help, or guidance completed
  1  Repository, settings, or write failure
  2  Invalid syntax

See docs/USAGE.md for settings and recovery."
    );
}
