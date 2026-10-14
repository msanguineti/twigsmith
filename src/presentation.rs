//! Terminal presentation only; callers retain policy, revalidation and exit status.
use std::io::{self, IsTerminal, Write};

use inquire::{
    Autocomplete, CustomUserError, Select, Text,
    error::InquireError,
    ui::{Attributes, Color, IndexPrefix, RenderConfig, StyleSheet, Styled},
};

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/support/terminal.rs"]
#[allow(dead_code)]
mod terminal_tests;

pub(crate) fn interactive() -> bool {
    io::stdin().is_terminal()
        && io::stderr().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

fn theme() -> RenderConfig<'static> {
    let mut config = RenderConfig::empty();
    config.answered_prompt_prefix = Styled::new("›");
    config.option_index_prefix = IndexPrefix::Simple;
    if coloured(true) {
        let accent = StyleSheet::new().with_fg(Color::LightCyan);
        config.prompt_prefix = Styled::new("?").with_style_sheet(accent);
        config.prompt = StyleSheet::new().with_attr(Attributes::BOLD);
        config.answer = accent;
        config.selected_option = Some(accent.with_attr(Attributes::BOLD));
        config.highlighted_option_prefix = Styled::new(">").with_style_sheet(accent);
        config.help_message = StyleSheet::new().with_fg(Color::DarkGrey);
    }
    config
}

fn answer<T>(result: Result<T, InquireError>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(InquireError::IO(error)) => Err(error),
        Err(error) => Err(io::Error::other(error)),
    }
}

pub(crate) fn input(prompt: &str) -> io::Result<Option<String>> {
    input_with_suggestions(prompt, &[])
}

/// Local hints never validate input or assign a default. Tab completes only an
/// explicitly highlighted value or the common prefix of all matching values.
pub(crate) fn input_with_suggestions(
    prompt: &str,
    suggestions: &[String],
) -> io::Result<Option<String>> {
    let prompt = display(prompt.trim_end());
    if interactive() {
        let prompt = wrap_after_prefix(&prompt, terminal_width(), 2);
        let suggestions = Suggestions::new(suggestions);
        let page_size = menu_page_size(&prompt, suggestions.values.iter().map(String::as_str));
        let has_suggestions = !suggestions.values.is_empty();
        let mut text = Text::new(&prompt)
            .with_render_config(theme())
            .with_formatter(&|value| display(value))
            .with_autocomplete(suggestions)
            .with_page_size(page_size);
        if has_suggestions {
            text = text.with_help_message("Type to filter; ↑/↓ chooses a hint; Tab completes");
        }
        answer(text.prompt())
    } else {
        eprint!("{prompt} ");
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

#[derive(Clone)]
struct Suggestions {
    values: Vec<String>,
}

impl Suggestions {
    fn new(values: &[String]) -> Self {
        // Inquire renders suggestion strings literally and also returns them as
        // answers. Escaping them would make typed escape sequences ambiguous.
        // Unsafe terminal controls therefore are not offered as hints.
        let mut values: Vec<_> = values
            .iter()
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
            .cloned()
            .collect();
        values.sort();
        values.dedup();
        Self { values }
    }
}

impl Autocomplete for Suggestions {
    fn get_suggestions(&mut self, input: &str) -> Result<Vec<String>, CustomUserError> {
        Ok(self
            .values
            .iter()
            .filter(|value| value.starts_with(input))
            .cloned()
            .collect())
    }

    fn get_completion(
        &mut self,
        input: &str,
        highlighted: Option<String>,
    ) -> Result<Option<String>, CustomUserError> {
        if let Some(value) = highlighted {
            return Ok(Some(value));
        }
        let mut matches = self.values.iter().filter(|value| value.starts_with(input));
        let Some(first) = matches.next() else {
            return Ok(None);
        };
        let mut prefix = first.clone();
        for value in matches {
            let bytes = prefix
                .chars()
                .zip(value.chars())
                .take_while(|(left, right)| left == right)
                .map(|(c, _)| c.len_utf8())
                .sum();
            prefix.truncate(bytes);
        }
        Ok((prefix.len() > input.len()).then_some(prefix))
    }
}

struct Choice<'a> {
    token: &'a str,
    label: String,
}

impl std::fmt::Display for Choice<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label)
    }
}

pub(crate) fn select_described(
    prompt: &str,
    choices: &[(&str, &str)],
    default: Option<&str>,
) -> io::Result<Option<String>> {
    if choices.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty menu"));
    }
    let keyboard = interactive();
    let prompt = display(prompt.trim_end());
    let prompt = if keyboard {
        wrap_after_prefix(&prompt, terminal_width(), 2)
    } else {
        prompt
    };
    let overhead = choices.len().to_string().len() + 4;
    let options: Vec<_> = choices
        .iter()
        .map(|(token, description)| Choice {
            token,
            label: {
                let label = format!(
                    "{}{}{}{}",
                    display(token),
                    if description.is_empty() { "" } else { " — " },
                    display(description),
                    if default == Some(*token) {
                        " (default)"
                    } else {
                        ""
                    }
                );
                if keyboard {
                    wrap_after_prefix(&label, terminal_width(), overhead)
                } else {
                    label
                }
            },
        })
        .collect();
    if keyboard {
        let page_size = menu_page_size(&prompt, options.iter().map(|value| value.label.as_str()));
        // Exact numeric input keeps the existing numbered-answer contract. Text
        // filters by prefix; unmatched input cannot submit an unrelated choice.
        answer(
            Select::new(&prompt, options)
                .with_starting_cursor(
                    default
                        .and_then(|value| choices.iter().position(|choice| choice.0 == value))
                        .or_else(|| choices.iter().position(|choice| choice.0 == "cancel"))
                        .unwrap_or(0),
                )
                .with_scorer(&|input, choice, _, index| {
                    let input = input.trim();
                    if input.is_empty()
                        || input.parse::<usize>().ok() == Some(index + 1)
                        || choice.token.starts_with(input)
                    {
                        Some(0)
                    } else {
                        None
                    }
                })
                .with_formatter(&|option| display(option.value.token))
                .with_page_size(page_size)
                .with_render_config(theme())
                .with_help_message("↑/↓ chooses; type name or number")
                .prompt(),
        )
        .map(|value| {
            value.and_then(|choice| (choice.token != "cancel").then(|| choice.token.to_owned()))
        })
    } else {
        for (index, choice) in options.iter().enumerate() {
            eprintln!(
                "{}",
                wrap(&format!("  {}) {choice}", index + 1), terminal_width())
            );
        }
        let tokens: Vec<_> = choices.iter().map(|choice| choice.0).collect();
        loop {
            let Some(value) = input(&prompt)? else {
                return Ok(None);
            };
            if value.trim() == "cancel" {
                return Ok(None);
            }
            let value = crate::cli::menu_answer(value, &tokens);
            let value = if value.trim().is_empty() {
                default.unwrap_or("")
            } else {
                value.trim()
            };
            if tokens.contains(&value) {
                return Ok((value != "cancel").then(|| value.to_owned()));
            }
            eprintln!("Choose a listed name or number, or enter cancel.");
        }
    }
}

fn terminal_width() -> usize {
    usize::from(console::Term::stderr().size().1).max(1)
}

fn menu_page_size<'a>(prompt: &str, labels: impl Iterator<Item = &'a str>) -> usize {
    let (height, width) = console::Term::stderr().size();
    let width = usize::from(width).max(1);
    let prompt_lines: usize = prompt
        .lines()
        .map(|line| console::measure_text_width(line).div_ceil(width).max(1))
        .sum();
    let available = usize::from(height).saturating_sub(prompt_lines + 4);
    let longest = labels
        .map(|label| {
            label
                .lines()
                .enumerate()
                .map(|(index, line)| {
                    (console::measure_text_width(line) + if index == 0 { 6 } else { 0 })
                        .div_ceil(width)
                        .max(1)
                })
                .sum::<usize>()
        })
        .max()
        .unwrap_or(1)
        .max(1);
    // Wrapped choices consume more visual space on narrow screens. Keep their
    // pages shorter so the repository/workflow context stays visible above them.
    let limit = if width < 60 { 3 } else { 5 };
    (available / longest).clamp(1, limit)
}

fn display(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Inquire supplies the first-line marker and number. Continuation lines align
/// with the value, and word wrapping leaves that first-line space available.
fn wrap_after_prefix(text: &str, width: usize, prefix_width: usize) -> String {
    let indent = " ".repeat(prefix_width.min(width.saturating_sub(1)));
    wrap(&format!("{indent}{text}"), width)
        .strip_prefix(&indent)
        .unwrap_or("")
        .to_owned()
}

/// Wrap human prose without truncating literal values. Keep explicit paragraphs
/// and indentation; long words wrap at Unicode character boundaries.
fn wrap(text: &str, width: usize) -> String {
    let width = width.max(1);
    text.split('\n')
        .map(|line| {
            let line = display(line);
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ')
                .take(width.saturating_sub(1))
                .collect();
            let mut out = indent.clone();
            let mut column = indent.len();
            for word in line.split_whitespace() {
                if column > indent.len() {
                    if column + 1 + console::measure_text_width(word) > width {
                        out.push('\n');
                        out.push_str(&indent);
                        column = indent.len();
                    } else {
                        out.push(' ');
                        column += 1;
                    }
                }
                for c in word.chars() {
                    let length = console::measure_text_width(&c.to_string());
                    if column + length > width && column > indent.len() {
                        out.push('\n');
                        out.push_str(&indent);
                        column = indent.len();
                    }
                    out.push(c);
                    column += length;
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn section(title: &str) {
    let title = wrap(title, terminal_width());
    if coloured(true) {
        eprintln!(
            "\n{}",
            console::Style::new()
                .for_stderr()
                .cyan()
                .bold()
                .apply_to(title)
        );
    } else {
        eprintln!("\n{title}");
    }
}

pub(crate) fn prose(text: &str) {
    eprintln!("{}", wrap(text, terminal_width()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_preserve_literals_and_complete_unicode_without_assigning_a_default() {
        let literals = [
            "feature/βeta$'x",
            "feature/βeta$'y",
            "réseau",
            "réseau",
            "bad\x1b[31m",
        ]
        .map(str::to_owned);
        let mut suggestions = Suggestions::new(&literals);
        assert_eq!(suggestions.get_suggestions("ré").unwrap(), ["réseau"]);
        assert_eq!(
            suggestions
                .get_completion("feature/β", None)
                .unwrap()
                .as_deref(),
            Some("feature/βeta$'")
        );
        assert_eq!(suggestions.get_completion("", None).unwrap(), None);
        assert_eq!(
            suggestions.get_suggestions("bad").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            suggestions.get_completion("future/name", None).unwrap(),
            None
        );
        assert_eq!(
            suggestions
                .get_completion("feature/β", Some(literals[0].clone()))
                .unwrap(),
            Some(literals[0].clone())
        );
    }

    #[test]
    fn prose_wraps_unicode_and_long_values_without_truncation_or_terminal_controls() {
        let literal = "feature/β界界界shell$'sensitive";
        let wrapped = wrap(&format!("  Branch: {literal}\n\nChoose work.\x1b[31m"), 14);
        assert!(
            wrapped
                .lines()
                .all(|line| console::measure_text_width(line) <= 14)
        );
        assert!(wrapped.contains("\n\n"));
        assert!(wrapped.replace(['\n', ' '], "").contains(literal));
        assert!(!wrapped.contains('\x1b'));
        assert!(wrapped.replace(['\n', ' '], "").contains("\\u{1b}[31m"));
        let choice = wrap_after_prefix("yes — Create the reviewed shared file", 40, 5);
        assert_eq!(choice, "yes — Create the reviewed shared\n     file");
        assert_eq!(
            wrap_after_prefix("Which destination should receive the work?", 40, 2),
            "Which destination should receive the\n  work?"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminal_prompt_probe() {
        if std::env::var_os("ONEFLOW_PRESENTATION_PROBE").is_none() {
            return;
        }
        section("Repository");
        let suggestions = ["feature/βeta$'x", "feature/βeta$'y"].map(str::to_owned);
        assert_eq!(
            input_with_suggestions("Suggested branch?", &suggestions)
                .unwrap()
                .as_deref(),
            Some("feature/βeta$'y")
        );
        assert_eq!(
            input_with_suggestions("Future branch?", &suggestions)
                .unwrap()
                .as_deref(),
            Some("future/新しい")
        );
        assert_eq!(
            input("Literal argument?").unwrap().as_deref(),
            Some("cancel")
        );
        assert_eq!(
            select_described(
                "Keep default?",
                &[("keep", "Keep the work"), ("cancel", "Stop")],
                Some("keep")
            )
            .unwrap()
            .as_deref(),
            Some("keep")
        );
        assert_eq!(
            select_described(
                "Choose number?",
                &[
                    ("keep", "Keep the work"),
                    ("delete", "Remove the branch"),
                    ("cancel", "Stop")
                ],
                None
            )
            .unwrap()
            .as_deref(),
            Some("delete")
        );
        assert_eq!(
            select_described(
                "Choose name?",
                &[
                    ("keep", "Keep the work"),
                    ("delete", "Remove the branch"),
                    ("cancel", "Stop")
                ],
                None
            )
            .unwrap()
            .as_deref(),
            Some("keep")
        );
        assert_eq!(
            select_described(
                "Next page?",
                &[
                    ("one", "First"),
                    ("two", "Second"),
                    ("three", "Third"),
                    ("four", "Fourth"),
                    ("five", "Fifth"),
                    ("six", "Sixth")
                ],
                None
            )
            .unwrap()
            .as_deref(),
            Some("six")
        );
        assert_eq!(
            select_described(
                "Cancel by default?",
                &[("yes", "Continue"), ("cancel", "Stop")],
                None
            )
            .unwrap(),
            None
        );
        assert_eq!(input("Cancel with keyboard?").unwrap(), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn keyboard_suggestions_and_described_menus_keep_literal_and_submission_contracts() {
        use std::process::Command;
        for (control, preference) in [
            (b"\x1b".as_slice(), None),
            (b"\x03".as_slice(), Some(("NO_COLOR", "1"))),
            (b"\x04".as_slice(), Some(("CLICOLOR", "0"))),
        ] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "presentation::tests::terminal_prompt_probe",
                    "--nocapture",
                ])
                .env("ONEFLOW_PRESENTATION_PROBE", "1")
                .env("TERM", "xterm-256color")
                .env_remove("NO_COLOR")
                .env_remove("CLICOLOR");
            if let Some((key, value)) = preference {
                command.env(key, value);
            }
            let columns = if preference.is_none() { 80 } else { 40 };
            let mut term = terminal_tests::Terminal::start_command_width(command, columns);
            term.until("Suggested branch?");
            term.send("feature/β\t\x1b[B\x1b[B\r".as_bytes());
            term.until("Future branch?");
            term.send("future/新しい\r".as_bytes());
            term.until("Literal argument?");
            term.send(b"cancel\r");
            term.until("Keep default?");
            term.send(b"\r");
            term.until("Choose number?");
            term.send(b"wrong\r\x7f\x7f\x7f\x7f\x7f2\r");
            term.until("Choose name?");
            term.send(b"kee\r");
            term.until("Next page?");
            term.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\r");
            term.until("Cancel by default?");
            term.send(b"\r");
            term.until("Cancel with keyboard?");
            term.send(control);
            assert!(term.finish().success(), "{}", term.output);
            term.assert_restored();
            if preference.is_none() {
                assert!(
                    term.output.contains("\x1b[36m") || term.output.contains("\x1b[96m"),
                    "{}",
                    term.output
                );
            } else {
                assert!(
                    !term.output.contains("\x1b[36m") && !term.output.contains("\x1b[96m"),
                    "{}",
                    term.output
                );
            }
        }
    }
}
