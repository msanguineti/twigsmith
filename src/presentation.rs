//! Terminal presentation only; callers retain policy, revalidation and exit status.
use std::io::{self, IsTerminal, Write};

use inquire::{
    Select, Text,
    error::InquireError,
    ui::{IndexPrefix, RenderConfig},
};

pub(crate) fn interactive() -> bool {
    io::stdin().is_terminal()
        && io::stderr().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

fn theme() -> RenderConfig<'static> {
    let mut config = if std::env::var_os("NO_COLOR").is_some()
        || std::env::var_os("CLICOLOR").is_some_and(|value| value == "0")
    {
        RenderConfig::empty()
    } else {
        RenderConfig::default()
    };
    config.option_index_prefix = IndexPrefix::Simple;
    config
}

fn answer(result: Result<String, InquireError>) -> io::Result<Option<String>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(InquireError::IO(error)) => Err(error),
        Err(error) => Err(io::Error::other(error)),
    }
}

pub(crate) fn input(prompt: &str) -> io::Result<Option<String>> {
    if interactive() {
        answer(
            Text::new(prompt.trim_end())
                .with_render_config(theme())
                .with_help_message("Enter submits; Esc/Ctrl-C/Ctrl-D cancels")
                .prompt(),
        )
    } else {
        eprint!("{} ", prompt.trim_end());
        io::stderr().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 || !line.ends_with('\n') {
            return Ok(None);
        }
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
        Ok(Some(line))
    }
}

pub(crate) fn select(
    prompt: &str,
    choices: &[&str],
    default: Option<&str>,
) -> io::Result<Option<String>> {
    if choices.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty menu"));
    }
    if interactive() {
        let options = choices.iter().map(|value| (*value).to_owned()).collect();
        // Exact numeric input keeps the existing numbered-answer contract. Text
        // filters by prefix; unmatched input cannot submit an unrelated choice.
        answer(
            Select::new(prompt.trim_end(), options)
                .with_starting_cursor(
                    default
                        .and_then(|value| choices.iter().position(|choice| *choice == value))
                        .or_else(|| choices.iter().position(|choice| *choice == "cancel"))
                        .unwrap_or(0),
                )
                .with_scorer(&|input, _, label, index| {
                    let input = input.trim();
                    if input.is_empty()
                        || input.parse::<usize>().ok() == Some(index + 1)
                        || label.starts_with(input)
                    {
                        Some(0)
                    } else {
                        None
                    }
                })
                .with_render_config(theme())
                .with_help_message("↑/↓ moves; Enter selects; type name/number; Esc cancels")
                .prompt(),
        )
        .map(|value| value.filter(|value| value != "cancel"))
    } else {
        for (index, choice) in choices.iter().enumerate() {
            eprintln!("  {}) {choice}", index + 1);
        }
        loop {
            let Some(value) = input(prompt)? else {
                return Ok(None);
            };
            if value.trim() == "cancel" {
                return Ok(None);
            }
            let value = crate::cli::menu_answer(value, choices);
            let value = if value.trim().is_empty() {
                default.unwrap_or("")
            } else {
                value.trim()
            };
            if choices.contains(&value) {
                return Ok((value != "cancel").then(|| value.to_owned()));
            }
            eprintln!("Choose a listed name or number, or enter cancel.");
        }
    }
}

#[derive(Clone, Copy)]
enum Level {
    Success,
    Error,
    Warning,
}

fn coloured(stderr: bool) -> bool {
    (if stderr {
        io::stderr().is_terminal()
    } else {
        io::stdout().is_terminal()
    }) && std::env::var_os("NO_COLOR").is_none()
        && std::env::var_os("TERM").is_none_or(|value| value != "dumb")
        && std::env::var_os("CLICOLOR").is_none_or(|value| value != "0")
}

/// Only caller-selected application headings receive styling. Supporting lines,
/// including Git diagnostics, retain their content and display policy.
fn heading(output: &str, level: Level, stderr: bool) -> String {
    if !coloured(stderr) {
        return output.to_owned();
    }
    let Some(start) = output
        .char_indices()
        .find_map(|(index, c)| (!c.is_whitespace()).then_some(index))
    else {
        return output.to_owned();
    };
    let end = output[start..]
        .find('\n')
        .map_or(output.len(), |offset| start + offset);
    let style = console::Style::new();
    let style = if stderr { style.for_stderr() } else { style };
    let style = match level {
        Level::Success => style.green(),
        Level::Error => style.red(),
        Level::Warning => style.yellow(),
    };
    format!(
        "{}{}{}",
        &output[..start],
        style.apply_to(&output[start..end]),
        &output[end..]
    )
}

pub(crate) fn error(message: impl std::fmt::Display) {
    eprintln!("{}", heading(&message.to_string(), Level::Error, true));
}

pub(crate) fn warning_text(output: &str, stderr: bool) -> String {
    heading(output, Level::Warning, stderr)
}

/// Style the outcome heading while retaining every diagnostic and literal display.
pub(crate) fn report(output: &str, success: bool) {
    let output = heading(
        output,
        if success {
            Level::Success
        } else {
            Level::Error
        },
        !success,
    );
    if success {
        print!("{output}");
        if !output.ends_with('\n') {
            println!();
        }
    } else {
        eprint!("{output}");
        if !output.ends_with('\n') {
            eprintln!();
        }
    }
}
