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
#[derive(Clone, Copy)]
enum OneflowBundle {
    Direct,
    Hosted,
}
impl OneflowBundle {
    fn apply(self, settings: &mut settings::Settings, marker: Option<String>) {
        match self {
            Self::Direct => settings::oneflow_bundle(settings, marker),
            Self::Hosted => settings::oneflow_hosted_bundle(settings, marker),
        }
    }
}
enum Command {
    Help,
    Guided(Format, Option<OneflowBundle>, Option<String>),
    Show,
    Generate(Format, Option<OneflowBundle>, Option<String>),
}
fn parse(args: &[OsString]) -> Option<Command> {
    match args {
        [] => return Some(Command::Guided(Format::Toml, None, None)),
        [arg] if arg == "--help" || arg == "-h" => return Some(Command::Help),
        [arg] if arg == "show" => return Some(Command::Show),
        _ => {}
    }
    let mut yes = false;
    let mut oneflow = None;
    let mut marker = None;
    let mut format = None;
    let mut rest = args;
    while let [arg, tail @ ..] = rest {
        if (arg == "--yes" || arg == "-y") && !yes {
            yes = true;
            rest = tail;
        } else if (arg == "--oneflow" || arg == "--oneflow-hosted") && oneflow.is_none() {
            oneflow = Some(if arg == "--oneflow" {
                OneflowBundle::Direct
            } else {
                OneflowBundle::Hosted
            });
            rest = tail;
        } else if arg == "--marker" && marker.is_none() {
            let [value, tail @ ..] = tail else {
                return None;
            };
            marker = Some(value.to_str()?.to_owned());
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
    if marker.is_some() && oneflow.is_none() {
        return None;
    }
    let format = format.unwrap_or(Format::Toml);
    Some(if yes {
        Command::Generate(format, oneflow, marker)
    } else {
        Command::Guided(format, oneflow, marker)
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
        Command::Guided(..) if !io::stdin().is_terminal() || !io::stderr().is_terminal() => {
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
        Command::Generate(format, oneflow, marker) => config::prepare(&directory, format)
            .and_then(|mut plan| {
                if let Some(bundle) = oneflow { bundle.apply(&mut plan.proposed, marker); }
                config::generate(&directory, &plan)
            })
            .map(|generated| {
                println!("Created shared configuration: {}", text(generated.path.display()));
                for path in generated.local_files {
                    println!("Local overrides in {} take precedence over generated values; use gof config show to inspect effective settings.", text(path.display()));
                }
            }),
        Command::Guided(format, oneflow, marker) => guided(&directory, format, oneflow, marker),
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
fn menu_input(prompt: &str, choices: &[&str]) -> Result<Option<String>, ConfigError> {
    let Some(answer) = input(prompt)? else {
        return Ok(None);
    };
    let answer = crate::cli::menu_answer(answer, choices);
    Ok((answer != "cancel").then_some(answer))
}
fn menu(choices: &[&str]) {
    for (index, choice) in choices.iter().enumerate() {
        eprintln!("  {}) {choice}", index + 1);
    }
}
fn cancelled() -> Result<(), ConfigError> {
    eprintln!("Setup cancelled; no configuration created.");
    Ok(())
}
fn policy<T>(
    label: &str,
    default: &str,
    choices: &[&str],
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>, ConfigError> {
    loop {
        let Some(answer) = menu_input(&format!("Default {label} [{default}]: "), choices)? else {
            return Ok(None);
        };
        match parse(if answer.is_empty() { default } else { &answer }) {
            Ok(value) => return Ok(Some(value)),
            Err(reason) => eprintln!("{reason}; or enter cancel."),
        }
    }
}
fn guided(
    directory: &Path,
    format: Format,
    oneflow: Option<OneflowBundle>,
    marker: Option<String>,
) -> Result<(), ConfigError> {
    let mut plan = config::prepare_guided(directory, format)?;
    let defaults = settings::Settings::default();
    eprintln!(
        "Configure shared start/save/finish settings. Enter keeps the built-in default; enter a displayed number or name for choices, or cancel to stop."
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
        "Save with uncommitted changes:\n  1) ask          Choose an action each time at a terminal.\n  2) publish-only Publish existing commits; uncommitted work stays local.\n  3) staged       Commit the staged selection, then publish; requires a message.\n               Unstaged and untracked work stays local.\n  4) wip          Checkpoint staged and remaining work separately, then publish.\n               Ignored files stay local.\n  5) stash        Retain a local stash including untracked work; publish existing commits.\n  6) cancel  Stop setup.\nClean trees publish existing commits directly. Explicit save flags override this preference."
    );

    loop {
        let Some(answer) = menu_input(
            &format!("Default save action [{}]: ", defaults.save_action.as_str()),
            &["ask", "publish-only", "staged", "wip", "stash", "cancel"],
        )?
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
        "Finish update when the branch needs the remote base:\n  1) ask     Choose rebase, merge, or none at a terminal.\n  2) rebase  Replay branch commits onto the observed base.\n  3) merge   Incorporate the observed base with a merge or fast-forward.\n  4) none    Publish current history without updating from the base.\n  5) cancel  Stop setup.\nExplicit finish --update overrides this preference."
    );

    loop {
        let Some(answer) = menu_input(
            "Default finish update [ask]: ",
            &["ask", "rebase", "merge", "none", "cancel"],
        )?
        else {
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
        "Finish replacement of published history:\n  1) ask    Request separate authorization when replacement is required.\n  2) allow  Standing authorization for eligible replacement.\n            Incorporation proof and exact remote leases remain required.\n  3) deny   Deny replacement.\n  4) cancel  Stop setup.\nSelecting rebase supplies no replacement authorization. Explicit rewrite flags override this preference."
    );

    loop {
        let Some(answer) = menu_input(
            "Default finish rewrite [ask]: ",
            &["ask", "allow", "deny", "cancel"],
        )?
        else {
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
        "Finish completion:\n  1) ask        Choose review or integrate at a terminal.\n  2) review     Prepare and publish the working branch for manual review.\n  3) integrate  Deliver to the project target, then complete checkout and cleanup.\n  4) cancel  Stop setup."
    );
    let Some(mode) = policy(
        "finish mode",
        defaults.finish_mode.as_str(),
        &["ask", "review", "integrate", "cancel"],
        settings::parse_mode,
    )?
    else {
        return cancelled();
    };
    plan.proposed.finish_mode = mode;
    if mode != settings::FinishMode::Review {
        eprintln!(
            "Final integration method (separate from finish-update):\n  1) ask      Choose at a terminal.\n  2) ff-only  Fast-forward only; creates no integration commit.\n  3) merge    Create a merge commit, including when fast-forward is possible.\n  4) squash   Create one commit containing the integrated work.\n  5) cancel  Stop setup."
        );
        let Some(integration) = policy(
            "finish integration",
            defaults.finish_integration.as_str(),
            &["ask", "ff-only", "merge", "squash", "cancel"],
            settings::parse_integration,
        )?
        else {
            return cancelled();
        };
        plan.proposed.finish_integration = integration;
        if integration != settings::FinishIntegration::FfOnly {
            eprintln!(
                "Final integration commit message:\n  1) ask   Offer an editable suggestion at a terminal.\n  2) auto  Use the generated message.\n  3) cancel  Stop setup.\nfinish -m/--message supplies the final integration commit message; existing work commits and review descriptions are unchanged."
            );
            let Some(message) = policy(
                "finish message",
                defaults.finish_message.as_str(),
                &["ask", "auto", "cancel"],
                settings::parse_message,
            )?
            else {
                return cancelled();
            };
            plan.proposed.finish_message = message;
        }
        eprintln!(
            "After confirmed target publication:\n  1) target  Check out the local integration target.\n  2) work    Keep the work branch checked out.\n  3) cancel  Stop setup."
        );
        let Some(checkout) = policy(
            "finish checkout",
            defaults.finish_checkout.as_str(),
            &["target", "work", "cancel"],
            settings::parse_checkout,
        )?
        else {
            return cancelled();
        };
        plan.proposed.finish_checkout = checkout;
        eprintln!(
            "Work branch cleanup after confirmed target publication:\n  1) ask     Choose cleanup at a terminal.\n  2) keep    Retain local and published work.\n  3) local   Delete local work; keep published work.\n  4) both    Delete local and published work.\n  5) cancel  Stop setup."
        );
        let Some(cleanup) = policy(
            "finish cleanup",
            defaults.finish_cleanup.as_str(),
            &["ask", "keep", "local", "both", "cancel"],
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
    if let Some(bundle) = oneflow {
        bundle.apply(&mut plan.proposed, marker);
        eprintln!("OneFlow routes are editable as JSON objects; Enter keeps each proposed policy.");
        if matches!(bundle, OneflowBundle::Hosted) {
            eprintln!(
                "Hosted tagged finish publishes the version in the project and a separate merge-back branch for manual review. It retains branches; marker advancement and later cleanup are manual."
            );
        }
        for kind in [
            settings::WorkstreamKind::Ordinary,
            settings::WorkstreamKind::Release,
            settings::WorkstreamKind::Hotfix,
        ] {
            loop {
                let proposed = &plan.proposed.workstreams[&kind];
                let json = serde_json::to_string(proposed).expect("policy serialization");
                let Some(answer) = input(&format!("{} policy [{}]: ", kind.as_str(), json))? else {
                    return cancelled();
                };
                if answer.is_empty() {
                    break;
                }
                match serde_json::from_str::<settings::WorkstreamConfig>(&answer) {
                    Ok(value) => match value.validate_for(directory, kind) {
                        Ok(()) => {
                            plan.proposed.workstreams.insert(kind, value);
                            break;
                        }
                        Err(error) => eprintln!("{error}"),
                    },
                    Err(error) => eprintln!("{error}"),
                }
            }
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
                "finish-history-edit",
                effective.history_edit.as_str(),
                &effective.sources.history_edit,
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
    if !plan.proposed.workstreams.is_empty() {
        let mut effective = reviewed_completion(&plan);
        let roles = reviewed_roles(&plan);
        effective.publish_remote = roles.publish_remote;
        effective.base_remote = roles.base_remote;
        effective.sources.publish_remote = roles.sources.publish_remote;
        effective.sources.base_remote = roles.sources.base_remote;
        let shared = SettingSource::Shared(plan.destination());
        for (kind, config) in &effective.workstreams {
            for key in serde_json::to_value(config)
                .expect("policy serialization")
                .as_object()
                .unwrap()
                .keys()
            {
                effective
                    .sources
                    .workstreams
                    .insert(format!("{}.{}", kind.as_str(), key), shared.clone());
            }
        }
        for (kind, config) in &review.overrides.workstreams {
            let mut values = serde_json::to_value(config).expect("policy serialization");
            values.as_object_mut().unwrap().retain(|key, _| {
                matches!(
                    review
                        .overrides
                        .sources
                        .workstreams
                        .get(&format!("{}.{}", kind.as_str(), key)),
                    Some(SettingSource::Local(_))
                )
            });
            for key in values.as_object().unwrap().keys() {
                let name = format!("{}.{}", kind.as_str(), key);
                effective.sources.workstreams.insert(
                    name.clone(),
                    review.overrides.sources.workstreams[&name].clone(),
                );
            }
            effective
                .workstreams
                .entry(*kind)
                .or_default()
                .overlay(serde_json::from_value(values).expect("validated private policy"));
        }
        for kind in effective.workstreams.keys() {
            let mut resolved = effective.clone();
            settings::resolve_workstream(&mut resolved, *kind, None)?;
            let policy = resolved.workstream.as_ref().unwrap();
            eprintln!(
                "Effective {} policy: publication {} ({}); preparation {}/{}; integration {}/{}; tagging {}; marker {}.",
                kind.as_str(),
                text(&resolved.publish_remote),
                source(&resolved.sources.publish_remote),
                text(&resolved.base_remote),
                text(&policy.preparation_source),
                text(&resolved.base_remote),
                text(policy.integration_targets.join(" -> ")),
                if policy.version_tagging {
                    "required"
                } else {
                    "disabled"
                },
                text(policy.release_marker.as_deref().unwrap_or("none"))
            );
            review_route(&resolved, true);
        }
    }
    menu(&["yes", "no", "cancel"]);
    loop {
        let Some(answer) = menu_input("Create this file? [yes/no]: ", &["yes", "no", "cancel"])?
        else {
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
fn review_route(settings: &settings::Settings, guided: bool) {
    let Some(policy) = &settings.workstream else {
        return;
    };
    let origin = settings
        .sources
        .workstreams
        .get(&format!("{}.review-remote", policy.kind.as_str()))
        .unwrap_or(&settings.sources.publish_remote);
    if settings.finish_mode == settings::FinishMode::Integrate {
        let message = format!(
            "Effective {} routes: work {} ({}); direct integration {}/{}.",
            policy.kind.as_str(),
            text(&policy.publish_remote),
            source(&settings.sources.publish_remote),
            text(&policy.base_remote),
            text(policy.integration_targets.join(" -> ")),
        );
        if guided {
            eprintln!("{message}");
        } else {
            println!("  {message}");
        }
        return;
    }
    let message = format!(
        "Effective {} routes: work {}/{}; review source {} on {} ({}); target {}/{}.",
        policy.kind.as_str(),
        text(&policy.publish_remote),
        if policy.version_tagging {
            "original work branch"
        } else {
            "work branch"
        },
        if policy.version_tagging
            && policy.version_tag_identity == settings::VersionTagIdentity::WorkSource
            && settings.finish_mode == settings::FinishMode::Review
        {
            text(
                policy
                    .merge_back_branch
                    .as_deref()
                    .unwrap_or("explicit --merge-back branch"),
            )
        } else {
            "work branch".into()
        },
        text(
            if policy.version_tagging
                && policy.version_tag_identity == settings::VersionTagIdentity::WorkSource
                && settings.finish_mode == settings::FinishMode::Review
            {
                &policy.review_remote
            } else {
                &policy.publish_remote
            }
        ),
        source(
            if policy.version_tagging
                && policy.version_tag_identity == settings::VersionTagIdentity::WorkSource
                && settings.finish_mode == settings::FinishMode::Review
            {
                origin
            } else {
                &settings.sources.publish_remote
            }
        ),
        text(&policy.base_remote),
        text(policy.integration_targets.join(" -> ")),
    );
    if guided {
        eprintln!("{message}");
    } else {
        println!("  {message}");
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
            "finish-history-edit",
            settings.history_edit.as_str(),
            &settings.sources.history_edit,
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
    if let Some(policy) = &settings.workstream {
        println!("  workstream-policy: {}", policy.kind.as_str());
        println!(
            "  creation-source: {}",
            policy
                .creation_source
                .as_deref()
                .unwrap_or("explicit input or fresh base")
        );
        let directory = inspection
            .repository
            .working_tree_root
            .as_deref()
            .unwrap_or(&inspection.repository.common_dir);
        let association =
            crate::git::inspect(directory, ["symbolic-ref", "--quiet", "--short", "HEAD"])
                .map_err(crate::repository::InspectionError::Io)
                .map_err(SettingsError::from)
                .and_then(|output| {
                    let branch = output.stdout.strip_suffix(b"\n").ok_or_else(|| {
                        SettingsError::InvalidSettings(
                            "creation association changed during inspection".into(),
                        )
                    })?;
                    settings::branch_policy(directory, branch)
                });
        match association {
            Ok(Some(record)) => {
                println!(
                    "  creation-commit: {} (established branch association)",
                    text(record.creation_commit)
                );
                println!(
                    "  creation-input: {}",
                    text(
                        record
                            .creation_input
                            .as_deref()
                            .unwrap_or("configured creation source or resumed branch tip")
                    )
                );
            }
            Ok(None) => println!("  creation-association: unavailable (changed during inspection)"),
            Err(error) => println!("  creation-association: unavailable ({})", text(error)),
        }
        review_route(settings, false);
        println!("  review-remote: {}", text(&policy.review_remote));
        println!(
            "  merge-back-branch: {}",
            text(
                policy
                    .merge_back_branch
                    .as_deref()
                    .unwrap_or("explicit --merge-back input")
            )
        );
        println!("  preparation-source: {}", text(&policy.preparation_source));
        println!(
            "  integration-targets (in order): {}",
            text(policy.integration_targets.join(" -> "))
        );
        println!(
            "  version-tag-identity: {}",
            policy.version_tag_identity.as_str()
        );
        println!(
            "  version-tagging: {}",
            if policy.version_tagging {
                "required"
            } else {
                "disabled"
            }
        );
        println!(
            "  active-release: {}",
            policy.active_release.as_deref().unwrap_or("none")
        );
        println!(
            "  release-marker: {}",
            policy.release_marker.as_deref().unwrap_or("none")
        );
    }
    for (kind, config) in &settings.workstreams {
        let mut effective = inspection.templates.clone();
        if settings::resolve_workstream(&mut effective, *kind, None).is_ok() {
            review_route(&effective, false);
            let policy = effective.workstream.as_ref().expect("resolved workstream");
            println!(
                "  Effective {} version-tag-identity: {}; integration-targets (in order): {}.",
                kind.as_str(),
                policy.version_tag_identity.as_str(),
                text(policy.integration_targets.join(" -> "))
            );
        }
        let values = serde_json::to_value(config).expect("policy serialization");
        for (key, value) in values.as_object().expect("policy object") {
            let name = format!("{}.{}", kind.as_str(), key);
            println!(
                "  workstreams.{name}: {} ({})",
                text(
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                ),
                settings
                    .sources
                    .workstreams
                    .get(&name)
                    .map(source)
                    .unwrap_or_else(|| "built-in".into())
            );
        }
    }
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
        r#"Set up shared workflow settings or inspect effective configuration.

Usage: git-oneflow config [OPTIONS]
       git-oneflow config show

Arguments:
  show                      Inspect settings, their sources, and local routing

Options:
  -y, --yes                 Create shared defaults without terminal input
  --format <toml|yaml|json>  Select the new file format (default: toml)
  --oneflow                 Generate editable direct OneFlow workstream policies
  --oneflow-hosted          Generate editable review OneFlow workstream policies
  --marker <branch>         Enable a release marker with either OneFlow bundle
  -h, --help                Show help

Examples:
  gof config
  gof config show
  gof config --yes --format yaml
  gof config --oneflow --yes --marker stable

Notes:
  Setup creates git-oneflow.config.<format> at the working-tree root, including
  when invoked from a nested directory. An existing recognized shared file is preserved.
  Guided setup reviews exact contents and private overrides before creation.
  Enter keeps the displayed default; finite choices accept a number or name.
  Cancel or end-of-input creates no file. Changed review context stops creation.
  --yes needs no terminal and retains unresolved ask choices; it creates no checks.
  Setup changes no branches, contacts no remote, and runs no configured checks.
  The OneFlow bundles are alternatives; edit their generated routes to suit the project.
  Direct tagged policies integrate and clean up work; hosted policies prepare manual
  review and retain work. Marker advancement after hosted review remains manual.

Defaults:
  publish-remote: origin; base-remote follows publication; base: main.
  save-action, finish-mode, finish-update, finish-rewrite: ask.
  finish-integration, finish-message, finish-cleanup: ask; finish-checkout: target.
  finish-history-edit: none; finish-checks: [].

Configuration:
  One git-oneflow.config.toml, .yaml, .yml, or .json file per layer.
  Shared files live at the worktree root; local overrides live in the Git common directory.
  Explicit command flags override local, shared, and built-in values in that order.
  Local finish-checks replaces the shared list; [] disables inherited checks.
  redact-credentials is local-only; HTTP(S) credentials are hidden by default.

Exit status:
  0  Inspection, generation, help, or cancellation completed
  1  Repository, settings, input, or generation failure
  2  Invalid syntax"#
    );
}
