//! Configuration argument grammar and human presentation.
use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::Path,
    process::ExitCode,
};

use crate::{
    config::{self, ConfigError, Format},
    repository::InspectionError,
    settings::{self, SettingSource, SettingsError},
    status::display,
};

fn text(value: impl std::fmt::Display) -> String {
    display(value.to_string().as_bytes())
}
enum Command {
    Help,
    Guided(Format),
    Show,
    Generate(Format),
}
fn parse(args: &[OsString]) -> Option<Command> {
    match args {
        [] => return Some(Command::Guided(Format::Toml)),
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
    let format = format.unwrap_or(Format::Toml);
    Some(if yes {
        Command::Generate(format)
    } else {
        Command::Guided(format)
    })
}
pub fn run(args: &[OsString]) -> ExitCode {
    let Some(command) = parse(args) else {
        eprintln!(
            "error: expected config show or config [--yes] [--format <toml|yaml|json>]; use config --help"
        );
        return ExitCode::from(2);
    };
    match command {
        Command::Help => {
            help();
            return ExitCode::SUCCESS;
        }
        Command::Guided(_) if !io::stdin().is_terminal() || !io::stderr().is_terminal() => {
            eprintln!(
                "error: guided setup requires terminal input and stderr; use gof config --yes [--format <toml|yaml|json>] for unattended defaults"
            );
            return ExitCode::FAILURE;
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
        Command::Guided(format) => guided(&directory, format),
        Command::Help => unreachable!(),
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
// Match save's terminal convention: a complete submitted line is required;
// EOF (including a partial line) cancels and never approves publication.
fn input(prompt: &str) -> Result<Option<String>, ConfigError> {
    eprint!("{prompt}");
    io::stderr().flush().map_err(ConfigError::Input)?;
    let mut line = String::new();
    if io::stdin()
        .read_line(&mut line)
        .map_err(ConfigError::Input)?
        == 0
        || !line.ends_with('\n')
    {
        return Ok(None);
    }
    line.pop();
    if line.ends_with('\r') {
        line.pop();
    }
    if line == "cancel" {
        Ok(None)
    } else {
        Ok(Some(line))
    }
}
fn cancelled() -> Result<(), ConfigError> {
    eprintln!("Setup cancelled; no configuration created.");
    Ok(())
}
fn guided(directory: &Path, format: Format) -> Result<(), ConfigError> {
    let mut plan = config::prepare_guided(directory, format)?;
    let defaults = settings::Settings::default();
    eprintln!(
        "Configure shared start/save/finish settings. Enter keeps the built-in default; type cancel to stop."
    );
    for (label, bytes) in [
        ("Local remotes", &plan.review().remotes),
        ("Local branch references", &plan.review().branches),
    ] {
        let shown = display(bytes);
        let lines: Vec<_> = shown.lines().collect();
        eprintln!(
            "{label} (offline hint): {}",
            if lines.is_empty() {
                "none".into()
            } else {
                lines
                    .iter()
                    .take(10)
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );
        if lines.len() > 10 {
            eprintln!("  {} more entries available locally.", lines.len() - 10);
        }
    }
    eprintln!("Names can be configured before remotes or branches exist.");
    loop {
        let Some(answer) = input(&format!(
            "Remote used by start, save, and finish [{}]: ",
            text(&defaults.publish_remote)
        ))?
        else {
            return cancelled();
        };
        let value = if answer.is_empty() {
            &defaults.publish_remote
        } else {
            &answer
        };
        if settings::valid_remote(directory, value)? {
            plan.proposed.publish_remote = value.into();
            plan.proposed.base_remote = value.into();
            break;
        }
        eprintln!(
            "Use a valid named Git remote, such as origin or upstream (no URL, spaces, or leading '-')."
        );
    }
    loop {
        let Some(answer) = input(&format!(
            "Base branch for new work and finish [{}]: ",
            text(&defaults.base)
        ))?
        else {
            return cancelled();
        };
        let value = if answer.is_empty() {
            &defaults.base
        } else {
            &answer
        };
        if settings::valid_branch(directory, value)? {
            plan.proposed.base = value.into();
            break;
        }
        eprintln!(
            "Use a valid literal Git branch name, such as main or feature/base (no spaces, HEAD, or leading '-')."
        );
    }
    eprintln!(
        "Save with uncommitted changes:\n  ask          Choose an action each time at a terminal.\n  publish-only Publish existing commits; uncommitted work stays local.\n  staged       Commit exactly the selected staged work, then publish; requires a message. Unstaged/untracked work stays local.\n  wip          Create separate WIP commits for staged and remaining work, then publish; ignored files stay local.\n  stash        Retain work, including untracked files, in a local stash; publish existing commits. Ignored files stay local.\nClean trees publish existing commits directly. Explicit save flags override this preference."
    );
    loop {
        let Some(answer) = input(&format!(
            "Default save action [{}]: ",
            defaults.save_action.as_str()
        ))?
        else {
            return cancelled();
        };
        match settings::parse_action(if answer.is_empty() {
            defaults.save_action.as_str()
        } else {
            &answer
        }) {
            Ok(action) => {
                plan.proposed.save_action = action;
                break;
            }
            Err(reason) => eprintln!("{reason}; or enter cancel."),
        }
    }
    eprintln!(
        "Finish update when the branch needs the remote base:\n  ask     Choose rebase, merge, or none at a terminal.\n  rebase  Replay branch commits onto the observed base.\n  merge   Incorporate the observed base with a merge or fast-forward.\n  none    Publish current history without updating from the base.\nExplicit finish --update overrides this preference."
    );
    loop {
        let Some(answer) = input("Default finish update [ask]: ")? else {
            return cancelled();
        };
        match settings::parse_update(if answer.is_empty() {
            defaults.finish_update.as_str()
        } else {
            &answer
        }) {
            Ok(update) => {
                plan.proposed.finish_update = update;
                break;
            }
            Err(reason) => eprintln!("{reason}; or enter cancel."),
        }
    }
    eprintln!(
        "Finish replacement of published history:\n  ask    Request separate authorization when replacement is required.\n  allow  Standing authorization for eligible replacement; incorporation proof and exact remote leases remain required.\n  deny   Deny replacement.\nSelecting rebase supplies no replacement authorization. Explicit rewrite flags override this preference."
    );
    loop {
        let Some(answer) = input("Default finish rewrite [ask]: ")? else {
            return cancelled();
        };
        match settings::parse_rewrite(if answer.is_empty() {
            defaults.finish_rewrite.as_str()
        } else {
            &answer
        }) {
            Ok(rewrite) => {
                plan.proposed.finish_rewrite = rewrite;
                break;
            }
            Err(reason) => eprintln!("{reason}; or enter cancel."),
        }
    }
    config::revalidate_review(directory, &plan)?;
    eprintln!(
        "Destination: {}\nExact proposed shared configuration:\n{}",
        text(plan.destination().display()),
        plan.contents()
    );
    let review = plan.review();
    if let Some(path) = &review.local {
        eprintln!(
            "Private overrides: {} (take precedence; values are not copied).",
            text(path.display())
        );
        let effective = &review.overrides;
        for (key, value, origin) in [
            (
                "publish-remote",
                effective.publish_remote.as_str(),
                &effective.sources.publish_remote,
            ),
            ("base", effective.base.as_str(), &effective.sources.base),
            (
                "save-action",
                effective.save_action.as_str(),
                &effective.sources.save_action,
            ),
            (
                "finish-update",
                effective.finish_update.as_str(),
                &effective.sources.finish_update,
            ),
            (
                "finish-rewrite",
                effective.finish_rewrite.as_str(),
                &effective.sources.finish_rewrite,
            ),
        ] {
            if matches!(origin, SettingSource::Local(_)) {
                eprintln!("  Effective {key}: {} ({})", text(value), source(origin));
            }
        }
        if matches!(effective.sources.base_remote, Some(SettingSource::Local(_)))
            || (effective.sources.base_remote.is_none()
                && matches!(effective.sources.publish_remote, SettingSource::Local(_)))
        {
            eprintln!(
                "  Effective base-remote: {} ({})",
                text(&effective.base_remote),
                base_remote_source(effective)
            );
        }
        if matches!(review.redact_credentials_source, SettingSource::Local(_)) {
            eprintln!(
                "  Effective redact-credentials: {} ({})",
                review.redact_credentials,
                source(&review.redact_credentials_source)
            );
        }
    } else {
        eprintln!("Private overrides: none.");
    }
    loop {
        let Some(answer) = input("Create this file? [yes/no]: ")? else {
            return cancelled();
        };
        match answer.as_str() {
            "yes" => break,
            "no" | "" => return cancelled(),
            _ => eprintln!("Enter yes to create the reviewed file, or no/cancel to stop."),
        }
    }
    let generated = config::generate(directory, &plan)?;
    println!(
        "Created shared configuration: {}",
        text(generated.path.display())
    );
    Ok(())
}

fn source(source: &SettingSource) -> String {
    match source {
        SettingSource::BuiltIn => "built-in default".into(),
        SettingSource::Shared(path) => format!("shared: {}", text(path.display())),
        SettingSource::Local(path) => format!("local: {}", text(path.display())),
    }
}
fn publication_source(settings: &settings::Settings) -> String {
    let origin = source(&settings.sources.publish_remote);
    match settings.sources.publication_key {
        Some(key) => format!("{origin}; setting: {}", key.as_str()),
        None => origin,
    }
}
fn base_remote_source(settings: &settings::Settings) -> String {
    match &settings.sources.base_remote {
        Some(origin) => source(origin),
        None => format!("follows publish-remote; {}", publication_source(settings)),
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
        "  publish-remote: {} ({})",
        text(&settings.publish_remote),
        publication_source(settings)
    );
    println!(
        "  base-remote: {} ({})",
        text(&settings.base_remote),
        base_remote_source(settings)
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
    for (key, value, origin) in [
        (
            "finish-update",
            settings.finish_update.as_str(),
            &settings.sources.finish_update,
        ),
        (
            "finish-rewrite",
            settings.finish_rewrite.as_str(),
            &settings.sources.finish_rewrite,
        ),
    ] {
        println!("  {key}: {value} ({})", source(origin));
    }
    println!(
        "  redact-credentials: {} ({})",
        loaded.redact_credentials,
        source(&loaded.redact_credentials_source)
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
        "Recognized files: git-oneflow.config.{{toml,yaml,yml,json}}; legacy local: oneflow.conf\nPrecedence: local overrides > shared settings > built-in defaults.\nAt one layer, publish-remote overrides remote; base-remote follows publication when absent from both layers.\nExplicit save/finish flags override their corresponding settings."
    );
}
fn help() {
    println!(
        "Inspect configuration or set up shared start/save/finish settings.

Usage: git-oneflow config show
       git-oneflow config --yes [--format <toml|yaml|json>]
       git-oneflow config [--format <toml|yaml|json>]

Arguments:
  show             Display effective settings and their sources

Options:
  -y, --yes        Create shared defaults without interactive input
  --format <toml|yaml|json>
                   Generated format (default: toml)
  -h, --help       Show help

Examples:
  gof config
  gof config show
  gof config -y
  gof config --yes --format yaml
  git-oneflow config --yes --format json

Details:
  Inspection reports effective values, file locations, and validation errors.
  Precedence per key: explicit workflow flags > local > shared > built-in defaults.
  Run from the root or any nested directory. Bare repositories support inspection.
  Generation creates git-oneflow.config.toml, .yaml, or .json at the working-tree root.
  Built-in defaults: publish-remote = origin; base-remote follows publication; base = main; save-action = ask.
  remote is a publication compatibility setting; publish-remote takes precedence at the same layer.
  Local publication settings override shared publication settings, including through remote.
  Explicit base-remote resolves independently; start, save, and finish support separate roles.
  Finish defaults: finish-update = ask; finish-rewrite = ask.
  Finish output redacts HTTP(S) URL credentials by default. redact-credentials is private and local-only.
  Use gof finish --show-credentials for one invocation when debugging; output and captured logs can expose credentials.
  finish-update: ask, rebase, merge, none; finish-rewrite: ask, allow, deny.
  Rewrite allow is standing authorization for eligible replacement; remote safeguards remain.
  Private overrides take precedence; guided review shows their effective values.
  Guided setup validates private files; --yes reports their locations without parsing.
  Every recognized shared file or symlink, including .yml, prevents generation.
  --yes permits unattended creation; existing files are preserved.
  Generation works offline with dirty, unborn, or detached working trees.
  It leaves existing work, the index, refs, and Git configuration unchanged.
  Run config for guided remote, base, save-action, finish-update, and finish-rewrite choices at a terminal.
  Setup starts with built-in defaults and offers offline repository hints.
  Review the destination, exact contents, and private overrides; enter yes to create.
  Cancellation or end-of-input creates no file. Changed review context stops setup.

Exit status:
  0  Inspection, generation, help, or cancellation completed
  1  Terminal, repository, settings, or write failure
  2  Invalid syntax

See docs/USAGE.md for settings and recovery."
    );
}
