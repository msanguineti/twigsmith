//! CLI activity around captured Git processes. Native terminal processes stay
//! outside this boundary, and library callers do not opt into presentation.
use std::{
    cell::Cell,
    io::{self, IsTerminal, Write},
    process::{Command, Output},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

const DELAY: Duration = Duration::from_millis(500);
const TICK: Duration = Duration::from_millis(100);

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) struct CliActivity(bool);

/// Scope the opt-in to this CLI invocation, including nested invocations.
pub(crate) fn enable() -> CliActivity {
    CliActivity(ENABLED.replace(true))
}

impl Drop for CliActivity {
    fn drop(&mut self) {
        ENABLED.set(self.0);
    }
}

pub(crate) fn capture(command: &mut Command) -> io::Result<Output> {
    let activity = if ENABLED.get() {
        Activity::start(command)
    } else {
        None
    };
    let result = command.output();
    if let Some(activity) = activity {
        activity.finish(&result);
    }
    result
}

struct Activity {
    completion: Sender<String>,
    worker: Option<JoinHandle<()>>,
    operation: &'static str,
}

impl Activity {
    fn start(command: &Command) -> Option<Self> {
        let (label, operation) = description(command);
        let (completion, receiver) = mpsc::channel();
        let animated = io::stdin().is_terminal()
            && io::stdout().is_terminal()
            && io::stderr().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var_os("CLICOLOR").is_none_or(|value| value != "0")
            && std::env::var_os("TERM").is_none_or(|value| value != "dumb");
        // A presentation failure must not prevent or change the Git operation.
        let worker = thread::Builder::new()
            .name("twigsmith-activity".into())
            .spawn(move || present(receiver, label, animated))
            .ok()?;
        Some(Self {
            completion,
            worker: Some(worker),
            operation,
        })
    }

    fn finish(mut self, result: &io::Result<Output>) {
        let outcome = match result {
            Ok(output) if output.status.success() => {
                format!("Git {} completed.", self.operation)
            }
            Ok(output) => match output.status.code() {
                Some(code) => format!("Git {} exited with status {code}.", self.operation),
                None => format!("Git {} terminated before completion.", self.operation),
            },
            Err(_) => format!("Git {} could not complete.", self.operation),
        };
        self.stop(outcome);
    }

    fn stop(&mut self, outcome: String) {
        let _ = self.completion.send(outcome);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.stop(format!("Git {} outcome unavailable.", self.operation));
        }
    }
}

fn present(receiver: Receiver<String>, label: &'static str, animated: bool) {
    if !matches!(
        receiver.recv_timeout(DELAY),
        Err(mpsc::RecvTimeoutError::Timeout)
    ) {
        return;
    }
    let spinner = animated.then(|| {
        let spinner = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
        spinner.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .expect("fixed activity template")
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        spinner.set_message(label);
        spinner.tick();
        spinner
    });
    if spinner.is_none() {
        let _ = writeln!(io::stderr(), "{label}...");
    }
    let outcome = loop {
        match receiver.recv_timeout(TICK) {
            Ok(outcome) => break outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(spinner) = &spinner {
                    spinner.tick();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break "Git operation outcome unavailable.".into();
            }
        }
    };
    if let Some(spinner) = spinner {
        spinner.finish_and_clear();
    }
    let _ = writeln!(io::stderr(), "{outcome}");
}

/// Read only the subcommand, skipping global options and their operands. Labels
/// never interpolate process arguments: endpoints may contain credentials.
fn description(command: &Command) -> (&'static str, &'static str) {
    let mut args = command.get_args();
    let subcommand = loop {
        let Some(arg) = args.next() else {
            return ("Inspecting repository", "inspection");
        };
        if [
            "-c",
            "-C",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "--config-env",
        ]
        .contains(&arg.to_str().unwrap_or(""))
        {
            args.next();
        } else if !arg.as_encoded_bytes().starts_with(b"-") {
            break arg.to_str();
        }
    };
    match subcommand {
        Some("ls-remote") => ("Inspecting remote references", "remote inspection"),
        Some("fetch") => ("Fetching repository objects", "fetch"),
        Some("push") => ("Publishing selected references", "push"),
        Some("commit") => ("Creating commit", "commit"),
        Some("stash") => ("Preserving local work in a stash", "stash"),
        Some("switch" | "checkout") => ("Switching branch", "branch switch"),
        Some("rebase") => ("Rebasing work", "rebase"),
        Some("merge") => ("Merging work", "merge"),
        Some("add") => ("Staging selected work", "staging"),
        Some("tag") => ("Preparing version tag", "tag operation"),
        Some("branch" | "update-ref") => ("Updating local references", "reference operation"),
        _ => ("Inspecting repository", "inspection"),
    }
}
