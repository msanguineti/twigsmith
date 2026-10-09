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
    let mut report = Report::default();
    report.error = (|| -> Result<(), Error> {
        let root = plan
            .repository
            .working_tree_root
            .as_deref()
            .unwrap_or(directory);
        revalidate_prepared(directory, plan)?;
        revalidate_prepared(root, plan)?;
        if !plan.is_ready() || plan.update != FinishUpdate::None {
            return Err(Error::Unprepared);
        }
        if skip {
            report.skipped = true;
            if !plan.settings.finish_checks.is_empty() {
                event(Event::Skipped {
                    count: plan.settings.finish_checks.len(),
                });
            }
            return Ok(());
        }
        if plan.settings.finish_checks.is_empty() {
            return Ok(());
        }
        let cancellation = check_process::Cancellation::new()?;
        for (index, check) in plan.settings.finish_checks.iter().enumerate() {
            report.failed = Some(index);
            cancellation.check()?;
            revalidate_prepared(root, plan)?;
            event(Event::Started {
                index,
                total: plan.settings.finish_checks.len(),
                check,
            });
            let status =
                check_process::run(root, &check.command, &cancellation, |stream, bytes| {
                    event(Event::Output { stream, bytes })
                })?;
            if !status.success() {
                return Err(Error::Failed(status));
            }
            revalidate_prepared(root, plan)?;
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
