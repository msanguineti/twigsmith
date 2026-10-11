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
fn policy<T>(
    label: &str,
    default: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>, ConfigError> {
    loop {
        let Some(answer) = input(&format!("Default {label} [{default}]: "))? else {
            return Ok(None);
        };
        match parse(if answer.is_empty() { default } else { &answer }) {
            Ok(value) => return Ok(Some(value)),
            Err(reason) => eprintln!("{reason}; or enter cancel."),
        }
    }
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
            "Publication remote for working branches [{}]: ",
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
    eprintln!(
        "The base source supplies new work and finish updates. Enter follows publication; enter another remote to select a separate source."
    );
    loop {
        let Some(answer) = input("Base source remote [follow publication]: ")? else {
            return cancelled();
        };
        if answer.is_empty() || answer == plan.proposed.publish_remote {
            plan.proposed.base_remote = plan.proposed.publish_remote.clone();
            plan.proposed.sources.base_remote = None;
            break;
        }
        if settings::valid_remote(directory, &answer)? {
            plan.proposed.base_remote = answer;
            plan.proposed.sources.base_remote = Some(SettingSource::Shared(plan.destination()));
            break;
        }
        eprintln!(
            "Use a valid named Git remote (no URL, spaces, or leading '-'), or Enter to follow publication."
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
    eprintln!(
        "Finish completion:\n  ask        Choose review or integrate at a terminal.\n  review     Prepare and publish the working branch for manual review.\n  integrate  Integrate into the project target, publish it, and complete checkout and cleanup."
    );
    let Some(mode) = policy(
        "finish mode",
        defaults.finish_mode.as_str(),
        settings::parse_mode,
    )?
    else {
        return cancelled();
    };
    plan.proposed.finish_mode = mode;
    if mode != settings::FinishMode::Review {
        eprintln!(
            "Final integration method (separate from finish-update):\n  ask      Choose at a terminal.\n  ff-only  Fast-forward only; creates no integration commit.\n  merge    Create a merge commit, including when fast-forward is possible.\n  squash   Create one commit containing the integrated work."
        );
        let Some(integration) = policy(
            "finish integration",
            defaults.finish_integration.as_str(),
            settings::parse_integration,
        )?
        else {
            return cancelled();
        };
        plan.proposed.finish_integration = integration;
        if integration != settings::FinishIntegration::FfOnly {
            eprintln!(
                "Final integration commit message:\n  ask   Offer an editable suggestion at a terminal.\n  auto  Use the generated message.\nfinish -m/--message supplies the final integration commit message; existing work commits and review descriptions are unchanged."
            );
            let Some(message) = policy(
                "finish message",
                defaults.finish_message.as_str(),
                settings::parse_message,
            )?
            else {
                return cancelled();
            };
            plan.proposed.finish_message = message;
        }
        eprintln!(
            "After confirmed target publication, finish checkout selects target (the local integration target) or work (the working branch)."
        );
        let Some(checkout) = policy(
            "finish checkout",
            defaults.finish_checkout.as_str(),
            settings::parse_checkout,
        )?
        else {
            return cancelled();
        };
        plan.proposed.finish_checkout = checkout;
        eprintln!(
            "After confirmed target publication, finish cleanup selects ask (choose at a terminal), keep (retain work branches), local (delete the local work branch), or both (delete local and published work branches)."
        );
        let Some(cleanup) = policy(
            "finish cleanup",
            defaults.finish_cleanup.as_str(),
            settings::parse_cleanup,
        )?
        else {
            return cancelled();
        };
        plan.proposed.finish_cleanup = cleanup;
    }
    eprintln!(
        "Optional finish checks run in order after preparation and before publication. Setup only records commands; it never runs them or probes executables.\nEnter a name, then a JSON array of strings: [\"cargo\", \"test\", \"--locked\"]. The first string is the executable; each remaining string is one literal argument. Shell features require an explicit shell."
    );
    loop {
        let prompt = if plan.proposed.finish_checks.is_empty() {
            "Finish check name [Enter to finish; default: no checks]: "
        } else {
            "Finish check name [Enter to finish]: "
        };
        let Some(name) = input(prompt)? else {
            return cancelled();
        };
        if name.is_empty() {
            break;
        }
        if name.trim().is_empty() {
            eprintln!("Use a nonblank display name, or Enter to finish.");
            continue;
        }
        loop {
            let Some(answer) = input("Command as JSON executable-and-argument array: ")? else {
                return cancelled();
            };
            let command = match serde_json::from_str::<Vec<String>>(&answer) {
                Ok(command) => command,
                Err(_) => {
                    eprintln!(
                        "Use a JSON array of strings, such as [\"cargo\", \"test\"]. Keep each argument in its own string."
                    );
                    continue;
                }
            };
            let check = settings::FinishCheck {
                name: name.clone(),
                command,
            };
            if let Err(reason) = settings::validate_finish_checks(std::slice::from_ref(&check)) {
                eprintln!("{reason}; enter the command again.");
                continue;
            }
            plan.proposed.finish_checks.push(check);
            break;
        }
    }
    config::revalidate_review(directory, &plan)?;
    eprintln!(
        "Destination: {}\nExact proposed shared configuration:\n{}",
        text(plan.destination().display()),
        plan.contents()
    );
    review_checks(
        "Proposed shared finish checks",
        &plan.proposed.finish_checks,
    );
    let review = plan.review();
    let roles = reviewed_roles(&plan);
    eprintln!(
        "Effective publish-remote: {} ({})",
        text(&roles.publish_remote),
        source(&roles.sources.publish_remote)
    );
    eprintln!(
        "Effective base-remote: {} ({})",
        text(&roles.base_remote),
        base_remote_source(&roles)
    );
    let effective_completion = reviewed_completion(&plan);
    for (key, value, origin) in completion_values(&effective_completion) {
        eprintln!("Effective {key}: {value} ({})", source(origin));
    }
    if effective_completion.finish_mode == settings::FinishMode::Review {
        eprintln!(
            "Integration method, message, checkout, and cleanup settings are inactive for review delivery."
        );
    } else if effective_completion.finish_integration == settings::FinishIntegration::FfOnly {
        eprintln!("Message policy is inactive for fast-forward-only integration.");
    }
    if let Some(path) = &review.local {
        eprintln!(
            "Private overrides: {} (take precedence; values are not copied).",
            text(path.display())
        );
        let effective = &review.overrides;
        for (key, value, origin) in [
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
        if matches!(review.redact_credentials_source, SettingSource::Local(_)) {
            eprintln!(
                "  Effective redact-credentials: {} ({})",
                review.redact_credentials,
                source(&review.redact_credentials_source)
            );
        }
        if matches!(effective.sources.finish_checks, SettingSource::Local(_)) {
            eprintln!(
                "  Effective finish-checks: {} ({})",
                checks(&effective.finish_checks),
                source(&effective.sources.finish_checks)
            );
            review_checks(
                "Effective private finish checks (replace the shared list)",
                &effective.finish_checks,
            );
        }
    } else {
        eprintln!("Private overrides: none.");
    }
    if !matches!(
        review.overrides.sources.finish_checks,
        SettingSource::Local(_)
    ) {
        review_checks(
            "Effective finish checks (shared)",
            &plan.proposed.finish_checks,
        );
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

// Preview the generated roles with the already validated private layer applied.
// Private values are observation only and never enter the generated proposal.
fn reviewed_roles(plan: &config::GenerationPlan) -> settings::Settings {
    let mut roles = plan.proposed.clone();
    let shared = SettingSource::Shared(plan.destination());
    roles.sources.publish_remote = shared.clone();
    if roles.sources.base_remote.is_some() || roles.base_remote != roles.publish_remote {
        roles.sources.base_remote = Some(shared);
    }
    let private = &plan.review().overrides;
    if matches!(private.sources.publish_remote, SettingSource::Local(_)) {
        roles.publish_remote = private.publish_remote.clone();
        roles.sources.publish_remote = private.sources.publish_remote.clone();
    }
    if matches!(private.sources.base_remote, Some(SettingSource::Local(_))) {
        roles.base_remote = private.base_remote.clone();
        roles.sources.base_remote = private.sources.base_remote.clone();
    }
    if roles.sources.base_remote.is_none() {
        roles.base_remote = roles.publish_remote.clone();
    }
    roles
}

fn completion_values(
    settings: &settings::Settings,
) -> [(&'static str, &'static str, &SettingSource); 5] {
    [
        (
            "finish-mode",
            settings.finish_mode.as_str(),
            &settings.sources.finish_mode,
        ),
        (
            "finish-integration",
            settings.finish_integration.as_str(),
            &settings.sources.finish_integration,
        ),
        (
            "finish-message",
            settings.finish_message.as_str(),
            &settings.sources.finish_message,
        ),
        (
            "finish-checkout",
            settings.finish_checkout.as_str(),
            &settings.sources.finish_checkout,
        ),
        (
            "finish-cleanup",
            settings.finish_cleanup.as_str(),
            &settings.sources.finish_cleanup,
        ),
    ]
}
fn reviewed_completion(plan: &config::GenerationPlan) -> settings::Settings {
    let mut effective = plan.proposed.clone();
    let private = &plan.review().overrides;
    let shared = SettingSource::Shared(plan.destination());
    effective.sources.finish_mode = shared.clone();
    if matches!(private.sources.finish_mode, SettingSource::Local(_)) {
        effective.finish_mode = private.finish_mode;
        effective.sources.finish_mode = private.sources.finish_mode.clone();
    }
    effective.sources.finish_integration =
        if plan.proposed.finish_mode != settings::FinishMode::Review {
            shared.clone()
        } else {
            SettingSource::BuiltIn
        };
    if matches!(private.sources.finish_integration, SettingSource::Local(_)) {
        effective.finish_integration = private.finish_integration;
        effective.sources.finish_integration = private.sources.finish_integration.clone();
    }
    effective.sources.finish_message = if plan.proposed.finish_mode != settings::FinishMode::Review
        && plan.proposed.finish_integration != settings::FinishIntegration::FfOnly
    {
        shared.clone()
    } else {
        SettingSource::BuiltIn
    };
    if matches!(private.sources.finish_message, SettingSource::Local(_)) {
        effective.finish_message = private.finish_message;
        effective.sources.finish_message = private.sources.finish_message.clone();
    }
    effective.sources.finish_checkout = if plan.proposed.finish_mode != settings::FinishMode::Review
    {
        shared.clone()
    } else {
        SettingSource::BuiltIn
    };
    if matches!(private.sources.finish_checkout, SettingSource::Local(_)) {
        effective.finish_checkout = private.finish_checkout;
        effective.sources.finish_checkout = private.sources.finish_checkout.clone();
    }
    effective.sources.finish_cleanup = if plan.proposed.finish_mode != settings::FinishMode::Review
    {
        shared.clone()
    } else {
        SettingSource::BuiltIn
    };
    if matches!(private.sources.finish_cleanup, SettingSource::Local(_)) {
        effective.finish_cleanup = private.finish_cleanup;
        effective.sources.finish_cleanup = private.sources.finish_cleanup.clone();
    }
    effective
}

fn source(source: &SettingSource) -> String {
    match source {
        SettingSource::BuiltIn => "built-in default".into(),
        SettingSource::Shared(path) => format!("shared: {}", text(path.display())),
        SettingSource::Local(path) => format!("local: {}", text(path.display())),
    }
}
// JSON preserves argument boundaries and escapes terminal controls for inspection.
fn checks(checks: &[settings::FinishCheck]) -> String {
    serde_json::to_string(checks).expect("finish checks serialization")
}
// Numbering shows execution order; JSON strings preserve argument boundaries
// and escape control characters rather than letting them affect the terminal.
fn review_checks(label: &str, checks: &[settings::FinishCheck]) {
    eprintln!(
        "{label}: {}",
        if checks.is_empty() {
            "none"
        } else {
            "in execution order"
        }
    );
    for (index, check) in checks.iter().enumerate() {
        eprintln!(
            "  {}. {}: {}",
            index + 1,
            serde_json::to_string(&check.name).expect("check name serialization"),
            serde_json::to_string(&check.command).expect("check command serialization")
        );
    }
}
fn base_remote_source(settings: &settings::Settings) -> String {
    match &settings.sources.base_remote {
        Some(origin) => source(origin),
        None => format!(
            "follows publish-remote; {}",
            source(&settings.sources.publish_remote)
        ),
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
        source(&settings.sources.publish_remote)
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
    for (key, value, origin) in completion_values(settings) {
        println!("  {key}: {value} ({})", source(origin));
    }
    println!(
        "  redact-credentials: {} ({})",
        loaded.redact_credentials,
        source(&loaded.redact_credentials_source)
    );
    println!(
        "  finish-checks: {} ({})",
        checks(&settings.finish_checks),
        source(&settings.sources.finish_checks)
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
        "Recognized files: git-oneflow.config.{{toml,yaml,yml,json}}\nPrecedence: local overrides > shared settings > built-in defaults.\nBase remote follows publication when base-remote is absent from both layers.\nExplicit save/finish flags override their corresponding settings."
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
  Local publication settings override shared publication settings.
  Explicit base-remote resolves independently; start, save, and finish support separate roles.
  Finish defaults: finish-mode = ask; finish-update = ask; finish-rewrite = ask.
  finish-mode: ask, review, integrate. Review prepares and publishes for manual review.
  Integrate delivers to base-remote, then completes selected checkout and work cleanup.
  finish-integration: ask (default), ff-only, merge (merge commit), squash.
  finish-message: ask (default; editable suggestion), auto (generated message).
  finish-checkout: target (default), work; applies after target publication.
  finish-cleanup: ask (default), keep, local, both; applies after target publication.
  local deletes local work; both deletes local and published work; keep retains branches.
  Guided review delivery omits integration settings and questions.
  Other modes ask integration policies; ff-only omits the message policy.
  --yes retains unresolved ask policies and the target checkout default.
  finish-checks is an ordered list of objects with a name string and command list.
  command is a nonempty executable-and-argument list; shell syntax requires an explicit shell.
  Defaults contain no checks. A local list replaces the shared list; [] disables inherited checks.
  Inspection displays checks and their source without running commands or checking executables.
  Finish runs checks after preparation and before publication; --skip-checks bypasses them explicitly.
  Start and save never execute finish checks.
  Finish output redacts HTTP(S) URL credentials by default. redact-credentials is private and local-only.
  Use gof finish --show-credentials for one invocation when debugging; output and captured logs can expose credentials.
  finish-update: ask, rebase, merge, none; finish-rewrite: ask, allow, deny.
  Rewrite allow is standing authorization for eligible replacement; remote safeguards remain.
  Private overrides take precedence; guided review shows effective roles, checks, and their sources.
  Equal role choices omit base-remote so the base follows publication.
  Remote names provide hints only; setup assigns no roles from their names.
  publish-remote governs gof operations; ordinary Git commands use your Git configuration.
  Guided setup validates private files; --yes reports their locations without parsing.
  Every recognized shared file or symlink, including .yml, prevents generation.
  --yes permits unattended creation; existing files are preserved.
  Generation works offline with dirty, unborn, or detached working trees.
  It leaves existing work, the index, refs, and Git configuration unchanged.
  Run config to choose publication, a base source that follows publication or a separate remote,
  the base branch, save action, finish policies, and optional ordered checks at a terminal.
  For each check, enter a name and a JSON array such as [\"cargo\", \"test\", \"--locked\"].
  Enter a blank name for no checks or to finish the list. Each string is one literal argument.
  For shell features, enter an explicit shell: [\"sh\", \"-c\", \"npm run lint && npm test\"].
  Setup never runs checks or probes executables; --yes always generates no checks.
  Failed checks stop publication; fix the problem and rerun finish to run all checks again.
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
