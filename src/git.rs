use std::{ffi::OsStr, io, path::Path, process::Command, process::Output};

/// Execute the installed Git executable in `directory` with literal arguments.
///
/// A completed process returns its raw stdout, stderr, and exit status, including
/// unsuccessful exits or termination by a signal. Failure to start or collect
/// the process returns an I/O error. No output is printed or exit decision made.
/// Git inherits the caller's environment and configuration.
pub fn run<I, S>(directory: impl AsRef<Path>, arguments: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .output()
}

/// Inspection-specific execution: prevent optional writes, lazy network fetches,
/// and user fsmonitor hooks. Keep repository selection and other Git semantics.
pub(crate) fn inspect<I, S>(directory: &Path, arguments: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .current_dir(directory)
        // Use an option rather than only an environment variable so Git versions
        // lacking the lazy-fetch safeguard fail instead of silently ignoring it.
        .args([
            "--no-optional-locks",
            "--no-lazy-fetch",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .output()
}

/// Workflow execution keeps lazy fetching and fsmonitor disabled, but permits
/// required locks/writes. Workflows explicitly delimit fetches and publication.
pub(crate) fn workflow<I, S>(directory: &Path, arguments: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .current_dir(directory)
        .args(["--no-lazy-fetch", "-c", "core.fsmonitor=false"])
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .output()
}
