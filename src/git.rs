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
