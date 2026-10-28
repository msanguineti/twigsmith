//! Per-attempt validation of prepared work. Events carry data, never terminal policy.
use super::*;
pub use crate::check_process::Stream;
use crate::{check_process, settings::FinishCheck};

pub enum Event<'a> {
    Started {
        index: usize,
        total: usize,
        check: &'a FinishCheck,
    },
    Output {
        stream: Stream,
        bytes: &'a [u8],
    },
    Passed {
        index: usize,
    },
    Skipped {
        count: usize,
    },
}
#[derive(Debug, Default)]
pub struct Report {
    pub completed: usize,
    pub skipped: bool,
    /// Zero-based configured check, when a failure belongs to a particular step.
    pub failed: Option<usize>,
    pub error: Option<Error>,
}
#[derive(Debug)]
pub enum Error {
    Context(FinishError),
    Unprepared,
    Process(check_process::Error),
    Failed(std::process::ExitStatus),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(e) => write!(f, "{e}"),
            Self::Unprepared => f.write_str("checks require a prepared finish plan"),
            Self::Process(e) => write!(f, "{e}"),
            Self::Failed(status) if status.code().is_none() => {
                write!(f, "check interrupted: {status}")
            }
            Self::Failed(status) => write!(f, "check failed with {status}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<FinishError> for Error {
    fn from(e: FinishError) -> Self {
        Self::Context(e)
    }
}
impl From<check_process::Error> for Error {
    fn from(e: check_process::Error) -> Self {
        Self::Process(e)
    }
}

/// Results are reports only; publication never accepts them as reusable authority.
/// Every publish invocation executes this stage again, unless explicitly skipped.
pub fn run(directory: &Path, plan: &Plan, skip: bool, mut event: impl FnMut(Event<'_>)) -> Report {
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    run_guarded(root, &plan.settings.finish_checks, skip, &mut event, || {
        // Checks execute at the selected worktree root. Validate that context
        // directly, including inherited Git selection; a nested caller directory
        // does not need a second identical assessment before every check.
        revalidate_prepared(root, plan)?;
        if !plan.is_ready() || plan.update != FinishUpdate::None {
            return Err(Error::Unprepared);
        }
        Ok(())
    })
}

// Porcelain status honors assume-unchanged and skip-worktree. Those hints cannot
// establish what configured checks actually read. When present, compare through
// a fresh disposable index for the already guarded HEAD candidate: Git then owns
// filters, executable/symlink handling and submodule comparison. The real index
// and its flags remain untouched. Missing sparse paths fail this content guard.
pub(crate) fn guard_hidden_work(root: &Path) -> Result<(), FinishError> {
    let entries = read(root, &["ls-files", "-v", "-z"])?;
    if !entries.split(|byte| *byte == 0).any(|entry| {
        entry
            .first()
            .is_some_and(|flag| flag.is_ascii_lowercase() || *flag == b'S')
    }) {
        return Ok(());
    }
    let scratch = tempfile::tempdir()?;
    let index = scratch.path().join("index");
    let mut hooks = OsString::from("core.hooksPath=");
    hooks.push(scratch.path());
    let execute = |arguments: &[&str]| -> Result<std::process::Output, FinishError> {
        let mut command = std::process::Command::new("git");
        command
            .current_dir(root)
            .args([
                "--no-optional-locks",
                "--no-lazy-fetch",
                "--no-replace-objects",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.ignorestat=false",
                "-c",
                "core.sparseCheckout=false",
                "-c",
                "core.sparseCheckoutCone=false",
                "-c",
                "core.splitIndex=false",
                "-c",
                "index.sparse=false",
            ])
            .arg("-c")
            .arg(&hooks)
            .args(arguments)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        Ok(crate::activity::capture(&mut command)?)
    };
    let arguments = ["read-tree", "--no-sparse-checkout", "HEAD"];
    let output = execute(&arguments)?;
    if !output.status.success() {
        return Err(InspectionError::Git {
            arguments: arguments.iter().map(OsString::from).collect(),
            output,
        }
        .into());
    }
    // diff-files alone can report zero-stat fresh entries as changed. Porcelain
    // diff against HEAD resolves their content while using only the scratch index.
    let arguments = [
        "diff",
        "--quiet",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--ignore-submodules=none",
        "HEAD",
        "--",
    ];
    let output = execute(&arguments)?;
    match output.status.code() {
        Some(0) => Ok(()),
        Some(1) => Err(FinishError::DirtyTree),
        _ => Err(InspectionError::Git {
            arguments: arguments.iter().map(OsString::from).collect(),
            output,
        }
        .into()),
    }
}

pub(crate) fn run_guarded(
    root: &Path,
    checks: &[FinishCheck],
    skip: bool,
    mut event: impl FnMut(Event<'_>),
    mut guard: impl FnMut() -> Result<(), Error>,
) -> Report {
    let mut report = Report::default();
    report.error = (|| -> Result<(), Error> {
        if skip {
            guard()?;
            report.skipped = true;
            if !checks.is_empty() {
                event(Event::Skipped {
                    count: checks.len(),
                });
            }
            return Ok(());
        }
        if checks.is_empty() {
            guard()?;
            return Ok(());
        }
        let cancellation = check_process::Cancellation::new()?;
        for (index, check) in checks.iter().enumerate() {
            report.failed = Some(index);
            cancellation.check()?;
            guard()?;
            guard_hidden_work(root)?;
            event(Event::Started {
                index,
                total: checks.len(),
                check,
            });
            let status =
                check_process::run(root, &check.command, &cancellation, |stream, bytes| {
                    event(Event::Output { stream, bytes })
                })?;
            if !status.success() {
                return Err(Error::Failed(status));
            }
            guard()?;
            guard_hidden_work(root)?;
            cancellation.check()?;
            report.completed += 1;
            event(Event::Passed { index });
        }
        cancellation.complete()?;
        report.failed = None;
        Ok(())
    })()
    .err();
    report
}
