//! Configuration argument grammar and human presentation.
use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal},
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
        crate::presentation::error(
            "error: expected config show or config [--yes] [--format <toml|yaml|json>]; use config --help",
        );
        return ExitCode::from(2);
    };
    match command {
        Command::Help => {
            help();
            return ExitCode::SUCCESS;
        }
        Command::Guided(..) if !io::stdin().is_terminal() || !io::stderr().is_terminal() => {
            crate::presentation::error(
                "error: guided setup requires terminal input and stderr; use gof config --yes [--format <toml|yaml|json>] for unattended defaults",
            );
            return ExitCode::FAILURE;
        }
        _ => {}
    }
    let directory = match env::current_dir() {
        Ok(path) => path,
        Err(e) => {
            crate::presentation::error(format!(
                "error: cannot determine current directory: {}",
                text(e)
            ));
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
                crate::presentation::report(&format!("Created shared configuration: {}", text(generated.path.display())), true);
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
            crate::presentation::error(format!("error: {}", text(&e)));
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
fn input(prompt: &str) -> Result<Option<String>, ConfigError> {
    crate::presentation::input(prompt).map_err(ConfigError::Input)
}
fn cancelled() -> Result<(), ConfigError> {
    eprintln!("Setup cancelled; no configuration created.");
    Ok(())
}
fn described(
    prompt: &str,
    choices: &[(&str, &str)],
    default: &str,
) -> Result<Option<String>, ConfigError> {
    crate::presentation::select_described(prompt, choices, Some(default))
        .map_err(ConfigError::Input)
}
fn hinted(prompt: &str, suggestions: &[String]) -> Result<Option<String>, ConfigError> {
    crate::presentation::input_with_suggestions(prompt, suggestions).map_err(ConfigError::Input)
}
fn choose<T>(
    prompt: &str,
    default: &str,
    choices: &[(&str, &str)],
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>, ConfigError> {
    loop {
        let Some(answer) = described(prompt, choices, default)? else {
            return Ok(None);
        };
        match parse(&answer) {
            Ok(value) => return Ok(Some(value)),
            Err(reason) => crate::presentation::prose(&reason),
        }
    }
}
fn edit_group(prompt: &str) -> Result<Option<bool>, ConfigError> {
    Ok(described(
        prompt,
        &[
            ("keep", "Keep these preferences"),
            ("edit", "Change these preferences"),
        ],
        "keep",
    )?
    .map(|answer| answer == "edit"))
}
fn repository_group(
    directory: &Path,
    plan: &mut config::GenerationPlan,
) -> Result<bool, ConfigError> {
    crate::presentation::section("Repository");
    let remotes = plan.review().remote_suggestions();
    loop {
        let Some(answer) = hinted(
            &format!(
                "Remote receiving work [{}]: ",
                text(&plan.proposed.publish_remote)
            ),
            &remotes,
        )?
        else {
            return Ok(false);
        };
        let value = if answer.is_empty() {
            plan.proposed.publish_remote.clone()
        } else {
            answer
        };
        if settings::valid_remote(directory, &value)? {
            plan.proposed.publish_remote = value;
            if plan.proposed.sources.base_remote.is_none() {
                plan.proposed.base_remote = plan.proposed.publish_remote.clone();
            }
            break;
        }
        crate::presentation::prose(
            "Use a named Git remote without spaces or a leading '-'. Future names are allowed.",
        );
    }
    crate::presentation::prose(
        "New work and finish preparation use the project source. Enter keeps the displayed preference; naming the remote receiving work makes the source follow publication.",
    );
    loop {
        let current = if plan.proposed.sources.base_remote.is_none() {
            "follow publication"
        } else {
            &plan.proposed.base_remote
        };
        let Some(answer) = hinted(
            &format!("Project source remote [{}]: ", text(current)),
            &remotes,
        )?
        else {
            return Ok(false);
        };
        if answer == plan.proposed.publish_remote
            || (answer.is_empty() && plan.proposed.sources.base_remote.is_none())
        {
            plan.proposed.base_remote = plan.proposed.publish_remote.clone();
            plan.proposed.sources.base_remote = None;
            break;
        }
        let value = if answer.is_empty() {
            plan.proposed.base_remote.clone()
        } else {
            answer
        };
        if settings::valid_remote(directory, &value)? {
            plan.proposed.base_remote = value;
            plan.proposed.sources.base_remote = Some(SettingSource::Shared(plan.destination()));
            break;
        }
        crate::presentation::prose("Use a named Git remote without spaces or a leading dash.");
    }
    let branches = plan.review().branch_suggestions(&plan.proposed.base_remote);
    loop {
        let Some(answer) = hinted(
            &format!("Project base branch [{}]: ", text(&plan.proposed.base)),
            &branches,
        )?
        else {
            return Ok(false);
        };
        let value = if answer.is_empty() {
            plan.proposed.base.clone()
        } else {
            answer
        };
        if settings::valid_branch(directory, &value)? {
            plan.proposed.base = value;
            return Ok(true);
        }
        crate::presentation::prose(
            "Use a literal Git branch name without spaces, HEAD, or a leading '-'. Future names are allowed.",
        );
    }
}
const SAVE_CHOICES: &[(&str, &str)] = &[
    ("ask", "Choose when saving uncommitted work"),
    (
        "publish-only",
        "Publish commits; leave uncommitted work local",
    ),
    ("commit:all", "Commit all changes together, then publish"),
    (
        "commit:staged,commit:unstaged",
        "Commit staged and unstaged changes separately",
    ),
    (
        "stash:all",
        "Stash all local changes together, then publish commits",
    ),
    (
        "stash:staged,stash:unstaged",
        "Stash staged and unstaged changes separately",
    ),
    (
        "commit:staged,stash:unstaged",
        "Commit staged changes, stash unstaged changes",
    ),
    (
        "stash:staged,commit:unstaged",
        "Stash staged changes, commit unstaged changes",
    ),
    (
        "custom",
        "Enter an ordered plan selecting either or both groups",
    ),
];
fn parse_save_input(value: &str) -> Result<settings::SavePlan, String> {
    if value.trim().is_empty() {
        return Ok(settings::SavePlan::Operations(vec![]));
    }
    settings::SavePlan::parse(
        &value
            .split(',')
            .map(|entry| entry.trim().to_owned())
            .collect::<Vec<_>>(),
    )
}
const MODE_CHOICES: &[(&str, &str)] = &[
    ("ask", "Choose review or direct delivery when finishing"),
    ("review", "Prepare and publish work for manual review"),
    (
        "integrate",
        "Deliver to the project target, then apply checkout and cleanup",
    ),
];
const UPDATE_CHOICES: &[(&str, &str)] = &[
    ("ask", "Choose how to bring the project base into work"),
    ("rebase", "Replay work commits onto the project base"),
    (
        "merge",
        "Bring the project base into work with a merge or fast-forward",
    ),
    ("none", "Keep current work history without a base update"),
];
const REWRITE_CHOICES: &[(&str, &str)] = &[
    ("ask", "Ask before replacing already published work commits"),
    (
        "allow",
        "Allow guarded replacement of already published work commits",
    ),
    (
        "deny",
        "Stop when published work history would need replacement",
    ),
];
const INTEGRATION_CHOICES: &[(&str, &str)] = &[
    ("ask", "Choose the delivery method when finishing"),
    (
        "ff-only",
        "Advance the target only when no integration commit is needed",
    ),
    ("merge", "Create a merge commit in the target"),
    ("squash", "Create one target commit containing the work"),
];
const MESSAGE_CHOICES: &[(&str, &str)] = &[
    ("ask", "Offer an editable integration commit message"),
    ("auto", "Use the generated integration commit message"),
];
const CHECKOUT_CHOICES: &[(&str, &str)] = &[
    (
        "target",
        "Check out the local integration target after delivery",
    ),
    ("work", "Keep the work branch checked out after delivery"),
];
const CLEANUP_CHOICES: &[(&str, &str)] = &[
    ("ask", "Choose branch cleanup after delivery"),
    ("keep", "Keep local and published work branches"),
    ("local", "Delete local work; keep the published branch"),
    ("both", "Delete local and published work branches"),
];
const HISTORY_CHOICES: &[(&str, &str)] = &[
    (
        "none",
        "Keep commit selection and messages during preparation",
    ),
    (
        "interactive",
        "Open Git's interactive editor during rebase preparation",
    ),
];
fn save_group(plan: &mut config::GenerationPlan) -> Result<bool, ConfigError> {
    crate::presentation::section("Saving work");
    crate::presentation::prose(&format!(
        "Plan: {}. Clean work publishes existing commits directly.",
        text(plan.proposed.save_plan.summary())
    ));
    let Some(edit) = edit_group("Saving preferences: ")? else {
        return Ok(false);
    };
    if !edit {
        return Ok(true);
    }
    loop {
        let current = plan.proposed.save_plan.tokens().join(",");
        let default = if SAVE_CHOICES.iter().any(|(key, _)| *key == current) {
            current.as_str()
        } else {
            "custom"
        };
        let Some(mut answer) = described("When saving uncommitted work: ", SAVE_CHOICES, default)?
        else {
            return Ok(false);
        };
        if answer == "custom" {
            crate::presentation::prose(
                "Enter ordered commit:all|staged|unstaged or stash:all|staged|unstaged entries separated by commas. Select each group once; all consumes both.",
            );
            let Some(value) = input(&format!("Save plan [{}]: ", text(&current)))? else {
                return Ok(false);
            };
            answer = if value.is_empty() { current } else { value };
        }
        match parse_save_input(&answer) {
            Ok(value) => {
                plan.proposed.save_plan = value;
                break;
            }
            Err(reason) => crate::presentation::prose(&reason),
        }
    }
    for stash in [false, true] {
        loop {
            let (key, value) = if stash {
                ("save-stash-message", &mut plan.proposed.save_stash_message)
            } else {
                ("save-message", &mut plan.proposed.save_message)
            };
            let Some(answer) = input(&format!(
                "{} [{}]: ",
                if stash {
                    "Stash label"
                } else {
                    "Commit message"
                },
                text(&*value)
            ))?
            else {
                return Ok(false);
            };
            if answer.is_empty() {
                break;
            }
            match settings::validate_save_label(key, &answer) {
                Ok(()) => {
                    *value = answer;
                    break;
                }
                Err(reason) => crate::presentation::prose(&reason),
            }
        }
    }
    Ok(true)
}
fn finish_group(plan: &mut config::GenerationPlan) -> Result<bool, ConfigError> {
    crate::presentation::section("Finishing work");
    finish_summary(&plan.proposed);
    let Some(edit) = edit_group("Finishing preferences: ")? else {
        return Ok(false);
    };
    if !edit {
        return Ok(true);
    }
    macro_rules! field {
        ($field:ident, $prompt:expr, $choices:expr, $parse:expr) => {
            let Some(value) = choose($prompt, plan.proposed.$field.as_str(), $choices, $parse)?
            else {
                return Ok(false);
            };
            plan.proposed.$field = value;
        };
    }
    field!(
        finish_mode,
        "How should finished work be delivered? ",
        MODE_CHOICES,
        settings::parse_mode
    );
    field!(
        finish_update,
        "How should work receive the project base? ",
        UPDATE_CHOICES,
        settings::parse_update
    );
    field!(
        finish_rewrite,
        "When preparation replaces published work commits: ",
        REWRITE_CHOICES,
        settings::parse_rewrite
    );
    if plan.proposed.finish_mode != settings::FinishMode::Review {
        field!(
            finish_integration,
            "How should work enter the target? ",
            INTEGRATION_CHOICES,
            settings::parse_integration
        );
        if plan.proposed.finish_integration != settings::FinishIntegration::FfOnly {
            field!(
                finish_message,
                "Integration commit message: ",
                MESSAGE_CHOICES,
                settings::parse_message
            );
        }
        field!(
            finish_checkout,
            "After successful direct delivery: ",
            CHECKOUT_CHOICES,
            settings::parse_checkout
        );
        field!(
            finish_cleanup,
            "Work branches after direct delivery: ",
            CLEANUP_CHOICES,
            settings::parse_cleanup
        );
    }
    let Some(advanced) = described(
        "Advanced history preparation: ",
        &[
            ("keep", "Keep the current history-edit preference"),
            (
                "edit",
                "Choose whether to open Git's interactive history editor",
            ),
        ],
        "keep",
    )?
    else {
        return Ok(false);
    };
    if advanced == "edit" {
        field!(
            history_edit,
            "History editing during rebase preparation: ",
            HISTORY_CHOICES,
            settings::parse_history_edit
        );
    }
    Ok(true)
}
fn checks_group(plan: &mut config::GenerationPlan) -> Result<bool, ConfigError> {
    crate::presentation::section("Finish checks");
    crate::presentation::prose(
        "Checks run in order after preparation and before publication. Setup records literal commands without running or probing them.",
    );
    let mut draft = plan.proposed.finish_checks.clone();
    loop {
        for (index, check) in draft.iter().enumerate() {
            crate::presentation::prose(&format!(
                "{}. {} ({} arguments)",
                index + 1,
                text(&check.name),
                check.command.len().saturating_sub(1)
            ));
        }
        let mut choices = vec![
            ("keep", "Keep this ordered list"),
            ("add", "Add a check"),
            ("clear", "Remove all shared checks"),
        ];
        if !draft.is_empty() {
            choices.extend([("edit", "Edit one check"), ("remove", "Remove one check")]);
        }
        let Some(answer) = described("Finish checks: ", &choices, "keep")? else {
            return Ok(false);
        };
        match answer.as_str() {
            "keep" => {
                plan.proposed.finish_checks = draft;
                return Ok(true);
            }
            "clear" => draft.clear(),
            "edit" | "remove" => {
                let items: Vec<_> = draft
                    .iter()
                    .enumerate()
                    .map(|(index, check)| ((index + 1).to_string(), text(&check.name)))
                    .collect();
                let choices: Vec<_> = items
                    .iter()
                    .map(|(key, label)| (key.as_str(), label.as_str()))
                    .collect();
                let Some(selected) = described("Which check? ", &choices, "1")? else {
                    return Ok(false);
                };
                let index = selected.parse::<usize>().expect("numbered choice") - 1;
                if answer == "remove" {
                    draft.remove(index);
                } else if !edit_check(&mut draft[index])? {
                    return Ok(false);
                }
            }

            "add" => {
                let name = loop {
                    let Some(value) = input("Check display name: ")? else {
                        return Ok(false);
                    };
                    if !value.trim().is_empty() {
                        break value;
                    }
                    crate::presentation::prose("Use a nonblank display name.");
                };
                let executable = loop {
                    let Some(value) = crate::presentation::input("Executable name or path: ")
                        .map_err(ConfigError::Input)?
                    else {
                        return Ok(false);
                    };
                    if !value.is_empty() && !value.contains('\0') {
                        break value;
                    }
                    crate::presentation::prose(
                        "Use a nonempty executable name or path without NUL.",
                    );
                };
                let mut command = vec![executable];
                if !arguments_input(&mut command, false)? {
                    return Ok(false);
                }
                draft.push(settings::FinishCheck { name, command });
            }
            _ => unreachable!(),
        }
    }
}
fn arguments_input(command: &mut Vec<String>, editing: bool) -> Result<bool, ConfigError> {
    loop {
        if editing {
            for (index, argument) in command.iter().skip(1).enumerate() {
                crate::presentation::prose(&format!(
                    "Argument {}: {}",
                    index + 1,
                    serde_json::to_string(argument).expect("argument string")
                ));
            }
        }
        let mut choices = vec![
            ("done", "Finish this command"),
            (
                "add",
                "Add one literal argument, including an empty argument",
            ),
        ];
        if editing {
            choices.push(("clear", "Remove all arguments and keep the executable"));
        }
        let Some(answer) = described("Command arguments: ", &choices, "done")? else {
            return Ok(false);
        };
        if answer == "done" {
            return Ok(true);
        }
        if answer == "clear" {
            command.truncate(1);
            continue;
        }
        loop {
            let Some(value) = input("Literal argument (Enter adds an empty argument): ")? else {
                return Ok(false);
            };
            if !value.contains('\0') {
                command.push(value);
                break;
            }
            crate::presentation::prose(
                "Process arguments cannot contain NUL; enter the argument again.",
            );
        }
    }
}
fn edit_check(check: &mut settings::FinishCheck) -> Result<bool, ConfigError> {
    loop {
        crate::presentation::prose(&format!(
            "{}: executable {}; {} arguments",
            text(&check.name),
            text(&check.command[0]),
            check.command.len() - 1
        ));
        let Some(field) = described(
            "Check field: ",
            &[
                ("done", "Finish editing this check"),
                ("name", "Change the display name"),
                ("executable", "Change the literal executable"),
                ("arguments", "Inspect or change literal arguments"),
            ],
            "done",
        )?
        else {
            return Ok(false);
        };
        if field == "done" {
            return Ok(true);
        }
        if field == "arguments" {
            if !arguments_input(&mut check.command, true)? {
                return Ok(false);
            }
            continue;
        }
        loop {
            let Some(value) = input(if field == "name" {
                "Check display name: "
            } else {
                "Executable name or path: "
            })?
            else {
                return Ok(false);
            };
            if field == "name" && !value.trim().is_empty() {
                check.name = value;
                break;
            }
            if field == "executable" && !value.is_empty() && !value.contains('\0') {
                check.command[0] = value;
                break;
            }
            crate::presentation::prose("Use a nonblank name or a nonempty executable without NUL.");
        }
    }
}
fn description<'a>(choices: &'a [(&str, &str)], value: &str) -> &'a str {
    choices
        .iter()
        .find(|(key, _)| *key == value)
        .map(|(_, label)| *label)
        .unwrap_or("Configured preference")
}
fn finish_summary(value: &settings::Settings) {
    for (label, description) in [
        (
            "Delivery",
            description(MODE_CHOICES, value.finish_mode.as_str()),
        ),
        (
            "Preparation",
            description(UPDATE_CHOICES, value.finish_update.as_str()),
        ),
        (
            "Published history",
            description(REWRITE_CHOICES, value.finish_rewrite.as_str()),
        ),
    ] {
        crate::presentation::prose(&format!("{label}: {description}."));
    }
    if value.history_edit == settings::HistoryEdit::Interactive {
        crate::presentation::prose(
            "History editing: Git's interactive editor during rebase preparation.",
        );
    }
    if value.finish_mode != settings::FinishMode::Review {
        crate::presentation::prose(&format!(
            "Direct delivery: {}; {}; {}.",
            description(INTEGRATION_CHOICES, value.finish_integration.as_str()),
            description(CHECKOUT_CHOICES, value.finish_checkout.as_str()),
            description(CLEANUP_CHOICES, value.finish_cleanup.as_str())
        ));
        if value.finish_integration != settings::FinishIntegration::FfOnly {
            crate::presentation::prose(&format!(
                "Integration message: {}.",
                description(MESSAGE_CHOICES, value.finish_message.as_str())
            ));
        }
    }
}
fn workflow_summary(value: &settings::Settings) {
    use settings::{
        FinishCheckout, FinishCleanup, FinishIntegration, FinishMessage, FinishMode, FinishRewrite,
        FinishUpdate,
    };
    crate::presentation::prose(&format!(
        "Save: {}. Commit message: {}; stash label: {}.",
        text(value.save_plan.summary()),
        text(&value.save_message),
        text(&value.save_stash_message)
    ));
    let delivery = match value.finish_mode {
        FinishMode::Ask => "choose review or direct delivery",
        FinishMode::Review => "prepare and publish for manual review",
        FinishMode::Integrate => "deliver directly to the project target",
    };
    let preparation = match value.finish_update {
        FinishUpdate::Ask => "choose base preparation",
        FinishUpdate::Rebase => "replay work onto the base",
        FinishUpdate::Merge => "merge the base into work",
        FinishUpdate::None => "keep current work history",
    };
    crate::presentation::prose(&format!("Finish: {delivery}; {preparation}."));
    let replacement = match value.finish_rewrite {
        FinishRewrite::Ask => "ask before replacing published work",
        FinishRewrite::Allow => "allow guarded replacement of published work",
        FinishRewrite::Deny => "deny replacement of published work",
    };
    crate::presentation::prose(&format!(
        "History: {replacement}{}.",
        if value.history_edit == settings::HistoryEdit::Interactive {
            "; edit interactively during rebase"
        } else {
            ""
        }
    ));
    if value.finish_mode != FinishMode::Review {
        let method = match value.finish_integration {
            FinishIntegration::Ask => "choose integration method",
            FinishIntegration::FfOnly => "fast-forward only",
            FinishIntegration::Merge => "merge commit",
            FinishIntegration::Squash => "one squash commit",
        };
        let cleanup = match value.finish_cleanup {
            FinishCleanup::Ask => "choose cleanup",
            FinishCleanup::Keep => "keep work branches",
            FinishCleanup::Local => "delete local work branch",
            FinishCleanup::Both => "delete local and published work",
        };
        crate::presentation::prose(&format!(
            "Direct: {method}; {}{}; {cleanup}.",
            if value.finish_checkout == FinishCheckout::Target {
                "check out target"
            } else {
                "stay on work"
            },
            if value.finish_integration == FinishIntegration::FfOnly {
                ""
            } else if value.finish_message == FinishMessage::Auto {
                "; generated message"
            } else {
                "; editable message"
            }
        ));
    }
}
fn guided(
    directory: &Path,
    format: Format,
    oneflow: Option<OneflowBundle>,
    marker: Option<String>,
) -> Result<(), ConfigError> {
    let mut plan = config::prepare_guided(directory, format)?;
    crate::presentation::prose(
        "Set up shared start, save and finish preferences. Names may be configured before they exist.",
    );
    if !repository_group(directory, &mut plan)?
        || !save_group(&mut plan)?
        || !finish_group(&mut plan)?
        || !checks_group(&mut plan)?
    {
        return cancelled();
    }
    if let Some(bundle) = oneflow {
        bundle.apply(&mut plan.proposed, marker);
        if !policies_group(directory, &mut plan)? {
            return cancelled();
        }
    }
    let mut private_shown = Vec::new();
    loop {
        // Audit both proposed and effective templates, including private-only
        // templates inheriting the newly selected shared defaults.
        let kinds: std::collections::BTreeSet<_> = plan
            .proposed
            .workstreams
            .keys()
            .chain(plan.review().overrides.workstreams.keys())
            .copied()
            .collect();
        for kind in kinds {
            if let Err(error) = config::audit_workstream(directory, &plan, kind) {
                crate::presentation::error(format!(
                    "{} preferences need correction: {}",
                    kind.as_str(),
                    text(error)
                ));
                if !edit_policy(directory, &mut plan, kind)? {
                    return cancelled();
                }
            }
        }
        let effective = config::audit(directory, &plan)?;
        config::revalidate_review(directory, &plan)?;
        crate::presentation::section("Review");
        crate::presentation::prose(&format!(
            "Create shared settings in {}",
            text(plan.destination().display())
        ));
        crate::presentation::prose(&format!(
            "Work is published to {}. New work starts from {}/{}.",
            text(&effective.publish_remote),
            text(&effective.base_remote),
            text(&effective.base)
        ));
        workflow_summary(&effective);
        crate::presentation::prose(&format!(
            "Finish checks: {}.",
            if effective.finish_checks.is_empty() {
                "none".into()
            } else {
                effective
                    .finish_checks
                    .iter()
                    .map(|check| text(&check.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        for kind in effective.workstreams.keys() {
            let mut resolved = effective.clone();
            settings::resolve_workstream(&mut resolved, *kind, None)?;
            let policy = resolved.workstream.as_ref().expect("resolved template");
            crate::presentation::prose(&format!(
                "{}: start from {}; prepare from {}/{} ({}); publish work to {}.",
                kind.as_str(),
                text(policy.creation_source.as_deref().unwrap_or(
                    if *kind == settings::WorkstreamKind::Hotfix {
                        "explicit version tag"
                    } else {
                        &resolved.base
                    }
                )),
                text(&policy.base_remote),
                text(&policy.preparation_source),
                resolved.finish_update.as_str(),
                text(&policy.publish_remote)
            ));
            crate::presentation::prose(&format!(
                "{}: {} to {}; version tag {}{}{}.",
                kind.as_str(),
                if resolved.finish_mode == settings::FinishMode::Review {
                    "manual review".into()
                } else {
                    format!("direct {}", resolved.finish_integration.as_str())
                },
                text(policy.integration_targets.join(" → ")),
                if policy.version_tagging {
                    policy.version_tag_identity.as_str()
                } else {
                    "disabled"
                },
                policy
                    .release_marker
                    .as_ref()
                    .map(|marker| format!("; marker {}", text(marker)))
                    .unwrap_or_default(),
                policy
                    .active_release
                    .as_ref()
                    .map(|branch| format!("; active release {}", text(branch)))
                    .unwrap_or_default()
            ));
            if resolved.finish_mode == settings::FinishMode::Integrate {
                crate::presentation::prose(&format!(
                    "{} after delivery: {}; {}.",
                    kind.as_str(),
                    description(CHECKOUT_CHOICES, resolved.finish_checkout.as_str()),
                    description(CLEANUP_CHOICES, resolved.finish_cleanup.as_str())
                ));
            }
            if resolved.finish_mode == settings::FinishMode::Review
                && policy.version_tagging
                && policy.version_tag_identity == settings::VersionTagIdentity::IntegratedResult
            {
                crate::presentation::prose(
                    "Tag the actual integrated result after external review and validation; setup records the policy only.",
                );
            }
            if resolved.finish_mode == settings::FinishMode::Review
                && policy.version_tagging
                && policy.version_tag_identity == settings::VersionTagIdentity::WorkSource
            {
                crate::presentation::prose(&format!(
                    "Version review uses a separate merge-back branch on {}; submission, marker advancement and later cleanup are manual.",
                    text(&policy.review_remote)
                ));
            }
        }
        private_summary(&plan, &effective, &mut private_shown);
        let mut choices = vec![
            ("yes", "Create the reviewed shared file"),
            ("no", "Cancel without creating a file"),
            ("contents", "Inspect exact generated contents"),
            ("repository", "Edit repository choices"),
            ("save", "Edit saving preferences"),
            ("finish", "Edit finishing preferences"),
            ("checks", "Edit finish checks"),
        ];
        if !effective.workstreams.is_empty() {
            choices.push(("policies", "Edit workstream preferences"));
        }
        let Some(answer) = described("Create this file? ", &choices, "no")? else {
            return cancelled();
        };
        let continued = match answer.as_str() {
            "yes" => {
                let generated = config::generate(directory, &plan)?;
                crate::presentation::report(
                    &format!(
                        "Created shared configuration: {}",
                        text(generated.path.display())
                    ),
                    true,
                );
                return Ok(());
            }
            "no" => return cancelled(),
            "contents" => {
                crate::presentation::section("Exact shared contents");
                eprintln!("{}", plan.contents());
                true
            }
            "repository" => repository_group(directory, &mut plan)?,
            "save" => save_group(&mut plan)?,
            "finish" => finish_group(&mut plan)?,
            "checks" => checks_group(&mut plan)?,
            "policies" => policies_group(directory, &mut plan)?,
            _ => unreachable!(),
        };
        if !continued {
            return cancelled();
        }
    }
}
fn private_summary(
    plan: &config::GenerationPlan,
    effective: &settings::Settings,
    shown: &mut Vec<String>,
) {
    let review = plan.review();
    let Some(path) = &review.local else { return };
    let mut changes = Vec::new();
    macro_rules! changed {
        ($field:ident, $label:expr, $value:expr) => {
            if effective.$field != plan.proposed.$field {
                changes.push(format!("{}: {}", $label, $value));
            }
        };
    }
    changed!(
        publish_remote,
        "work remote",
        text(&effective.publish_remote)
    );
    changed!(base_remote, "project remote", text(&effective.base_remote));
    changed!(base, "project base", text(&effective.base));
    changed!(save_plan, "saving", text(effective.save_plan.summary()));
    changed!(
        save_message,
        "commit message",
        text(&effective.save_message)
    );
    changed!(
        save_stash_message,
        "stash label",
        text(&effective.save_stash_message)
    );
    changed!(
        finish_mode,
        "delivery",
        description(MODE_CHOICES, effective.finish_mode.as_str())
    );
    changed!(
        finish_update,
        "preparation",
        description(UPDATE_CHOICES, effective.finish_update.as_str())
    );
    changed!(
        finish_rewrite,
        "published history",
        description(REWRITE_CHOICES, effective.finish_rewrite.as_str())
    );
    changed!(
        history_edit,
        "history editing",
        description(HISTORY_CHOICES, effective.history_edit.as_str())
    );
    if effective.finish_mode != settings::FinishMode::Review {
        changed!(
            finish_integration,
            "direct delivery",
            description(INTEGRATION_CHOICES, effective.finish_integration.as_str())
        );
        if effective.finish_integration != settings::FinishIntegration::FfOnly {
            changed!(
                finish_message,
                "integration message",
                description(MESSAGE_CHOICES, effective.finish_message.as_str())
            );
        }
        changed!(
            finish_checkout,
            "checkout",
            description(CHECKOUT_CHOICES, effective.finish_checkout.as_str())
        );
        changed!(
            finish_cleanup,
            "cleanup",
            description(CLEANUP_CHOICES, effective.finish_cleanup.as_str())
        );
    }
    if effective.finish_checks != plan.proposed.finish_checks {
        changes.push("finish checks replace the shared list".into());
    }
    for kind in effective.workstreams.keys() {
        let resolve = |settings: &settings::Settings| {
            let mut resolved = settings.clone();
            settings::resolve_workstream(&mut resolved, *kind, None)?;
            Ok::<_, SettingsError>((
                resolved.workstream,
                resolved.finish_mode,
                resolved.finish_update,
                resolved.history_edit,
                resolved.finish_rewrite,
                resolved.finish_integration,
                resolved.finish_message,
                resolved.finish_checkout,
                resolved.finish_cleanup,
            ))
        };
        let shared = resolve(&plan.proposed);
        let local = resolve(effective);
        if shared.as_ref().ok() != local.as_ref().ok() {
            changes.push(format!(
                "{} workstream routes or preferences change the shared proposal",
                kind.as_str()
            ));
        }
    }
    if !review.redact_credentials {
        changes.push("HTTP(S) credentials are visible in finish output".into());
    }
    let new_changes: Vec<_> = changes
        .iter()
        .filter(|change| !shown.contains(change))
        .collect();
    if !new_changes.is_empty() {
        crate::presentation::prose(&format!(
            "Private overrides in {} take precedence and stay private:",
            text(path.display())
        ));
        for change in new_changes {
            crate::presentation::prose(&format!("  {change}."));
        }
    }
    *shown = changes;
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
fn policies_group(
    directory: &Path,
    plan: &mut config::GenerationPlan,
) -> Result<bool, ConfigError> {
    crate::presentation::section("Workstream preferences");
    let kinds: std::collections::BTreeSet<_> = plan
        .proposed
        .workstreams
        .keys()
        .chain(plan.review().overrides.workstreams.keys())
        .copied()
        .collect();
    for kind in kinds {
        let Some(edit) = edit_group(&format!("{} workstream: ", kind.as_str()))? else {
            return Ok(false);
        };
        if edit && !edit_policy(directory, plan, kind)? {
            return Ok(false);
        }
    }
    Ok(true)
}
// Presentation metadata addresses the existing sparse schema. Values are edited
// through WorkstreamConfig itself, with no second policy model or new defaults.
const POLICY_FIELDS: &[(&str, &str)] = &[
    ("done", "Finish editing this workstream"),
    ("creation-source", "Branch supplying new work"),
    (
        "preparation-source",
        "Branch brought into work during preparation",
    ),
    ("integration-target", "Single destination branch"),
    (
        "integration-targets",
        "Ordered destination branches for tagged direct delivery",
    ),
    ("publish-remote", "Remote receiving the work branch"),
    (
        "base-remote",
        "Remote supplying project branches and direct destinations",
    ),
    (
        "review-remote",
        "Remote receiving a tagged version's merge-back branch",
    ),
    (
        "merge-back-branch",
        "Separate branch prepared for tagged version review",
    ),
    (
        "version-tagging",
        "Whether finishing requires a version tag",
    ),
    (
        "version-tag-identity",
        "Whether the version identifies work or the integrated result",
    ),
    (
        "release-marker",
        "Optional branch marking the released version",
    ),
    (
        "active-release",
        "Declared active release branch for hotfix routing",
    ),
    ("finish-mode", "Review or direct delivery"),
    ("finish-update", "Bring the preparation base into work"),
    ("finish-history-edit", "Git's interactive history editor"),
    (
        "finish-integration",
        "Method for direct integration into a destination",
    ),
    ("finish-message", "Integration commit message"),
    ("finish-checkout", "Checkout after direct delivery"),
    ("finish-cleanup", "Branch cleanup after direct delivery"),
    (
        "finish-rewrite",
        "Replacement of already published work history",
    ),
];
fn edit_policy(
    directory: &Path,
    plan: &mut config::GenerationPlan,
    kind: settings::WorkstreamKind,
) -> Result<bool, ConfigError> {
    crate::presentation::section(&format!("{} workstream", kind.as_str()));
    crate::presentation::prose(
        "Choose a field to change. Omitted fields inherit shared preferences; 'none' disables optional marker or active-release branches.",
    );
    loop {
        let Some(field) = described("Workstream field: ", POLICY_FIELDS, "done")? else {
            return Ok(false);
        };
        if field == "done" {
            match config::audit_workstream(directory, plan, kind) {
                Ok(()) => return Ok(true),
                Err(error) => {
                    crate::presentation::error(format!(
                        "{} preferences need correction: {}",
                        kind.as_str(),
                        text(error)
                    ));
                    if plan.review().local.is_some() {
                        crate::presentation::prose(
                            "Private workstream fields take precedence. Change complementary shared fields, or cancel to edit the private file.",
                        );
                    }
                    continue;
                }
            }
        }
        let current = plan
            .proposed
            .workstreams
            .get(&kind)
            .cloned()
            .unwrap_or_default();
        let mut values = serde_json::to_value(&current).expect("policy serialization");
        let values = values.as_object_mut().expect("policy object");
        if let Some(value) = values.get(&field) {
            match value {
                serde_json::Value::String(value) => {
                    crate::presentation::prose(&format!("Current value: {}", text(value)))
                }
                serde_json::Value::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        crate::presentation::prose(&format!(
                            "{}. {}",
                            index + 1,
                            text(value.as_str().expect("branch string"))
                        ));
                    }
                }
                _ => unreachable!(),
            }
        } else {
            crate::presentation::prose("Current value: inherited.");
        }
        let choices = match field.as_str() {
            "finish-mode" => Some(MODE_CHOICES),
            "finish-update" => Some(UPDATE_CHOICES),
            "finish-history-edit" => Some(HISTORY_CHOICES),
            "finish-integration" => Some(INTEGRATION_CHOICES),
            "finish-message" => Some(MESSAGE_CHOICES),
            "finish-checkout" => Some(CHECKOUT_CHOICES),
            "finish-cleanup" => Some(CLEANUP_CHOICES),
            "finish-rewrite" => Some(REWRITE_CHOICES),
            "version-tagging" => Some(
                &[
                    ("disabled", "Finish without a version tag"),
                    ("required", "Finish with an explicit version tag"),
                ][..],
            ),
            "version-tag-identity" => Some(
                &[
                    ("work-source", "Tag the completed work before merge-back"),
                    ("integrated-result", "Tag the actual integrated result"),
                ][..],
            ),
            _ => None,
        };
        if let Some(choices) = choices {
            let mut choices = choices.to_vec();
            choices.push(("inherit", "Omit this field and inherit its preference"));
            let default = values
                .get(&field)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("inherit");
            let Some(answer) = described("Workstream preference: ", &choices, default)? else {
                return Ok(false);
            };
            if answer == "inherit" {
                values.remove(&field);
            } else {
                values.insert(field.clone(), answer.into());
            }
        } else {
            let Some(action) = described(
                "Change this field: ",
                &[
                    ("keep", "Keep its current value"),
                    ("set", "Set an explicit value"),
                    ("inherit", "Omit this field and inherit"),
                ],
                "keep",
            )?
            else {
                return Ok(false);
            };
            match action.as_str() {
                "keep" => continue,
                "inherit" => {
                    values.remove(&field);
                }
                "set" if field == "integration-targets" => {
                    let mut targets = Vec::<String>::new();
                    loop {
                        let Some(action) = described(
                            "Destination branches: ",
                            &[
                                ("add", "Add the next branch in delivery order"),
                                ("done", "Finish the destination list"),
                            ],
                            if targets.is_empty() { "add" } else { "done" },
                        )?
                        else {
                            return Ok(false);
                        };
                        if action == "done" {
                            break;
                        }
                        let hints = plan.review().branch_suggestions(
                            current
                                .base_remote
                                .as_deref()
                                .unwrap_or(&plan.proposed.base_remote),
                        );
                        let Some(branch) = hinted("Destination branch: ", &hints)? else {
                            return Ok(false);
                        };
                        targets.push(branch);
                    }
                    values.remove("integration-target");
                    values.insert(
                        field.clone(),
                        serde_json::to_value(targets).expect("branch list"),
                    );
                }
                "set" => {
                    let is_remote = field.ends_with("remote");
                    let hints = if is_remote {
                        plan.review().remote_suggestions()
                    } else {
                        plan.review().branch_suggestions(
                            current
                                .base_remote
                                .as_deref()
                                .unwrap_or(&plan.proposed.base_remote),
                        )
                    };
                    let Some(value) = hinted("Literal value: ", &hints)? else {
                        return Ok(false);
                    };
                    if field == "integration-target" {
                        values.remove("integration-targets");
                    }
                    values.insert(field.clone(), value.into());
                }
                _ => unreachable!(),
            }
        }
        let value: settings::WorkstreamConfig =
            serde_json::from_value(serde_json::Value::Object(values.clone()))
                .expect("schema fields and typed values");
        // Keep incomplete related-field edits correctable. Done audits both
        // shared and effective policy relationships before review can proceed.
        plan.proposed.workstreams.insert(kind, value);
    }
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
        "  save-plan: {} ({})",
        text(settings.save_plan.summary()),
        source(&settings.sources.save_plan)
    );
    for (key, value, origin) in [
        (
            "save-message",
            &settings.save_message,
            &settings.sources.save_message,
        ),
        (
            "save-stash-message",
            &settings.save_stash_message,
            &settings.sources.save_stash_message,
        ),
    ] {
        println!("  {key}: {} ({})", text(value), source(origin));
    }
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
  Guided setup groups preferences and reviews effective behavior before creation.
  Review offers exact contents and editing earlier choices; creation defaults to no.
  Enter keeps the displayed default. Tab offers local remote and branch hints.
  Use arrow keys for terminal choices; plain menus accept a number or name.
  Esc, Ctrl-C, Ctrl-D, or end-of-input cancels. Plain menus also accept cancel.
  Checks use a literal executable and individually entered arguments, including empty ones.
  Cancel or end-of-input creates no file. Changed review context stops creation.
  --yes needs no terminal and retains unresolved ask choices; it creates no checks.
  Setup changes no branches, contacts no remote, and runs no configured checks.
  The OneFlow bundles are alternatives; edit their generated routes to suit the project.
  Direct tagged policies integrate and clean up work; hosted policies prepare manual
  review and retain work. Marker advancement after hosted review remains manual.

Defaults:
  publish-remote: origin; base-remote follows publication; base: main.
  save-plan: ["ask"]. save-message: Save work. save-stash-message: Saved work.
  finish-mode, finish-update, finish-rewrite: ask.
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
