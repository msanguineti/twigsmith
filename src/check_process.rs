//! Literal project-command execution. No Git, workflow policy, or presentation.
use std::{io, path::Path, process::ExitStatus};

#[derive(Debug, Clone, Copy)]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Interrupted,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot execute or collect check: {e}"),
            Self::Interrupted => f.write_str("check interrupted; managed check processes stopped"),
        }
    }
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use rustix::{
        fs::{OFlags, fcntl_getfl, fcntl_setfl},
        process::{Pid, Signal, kill_process_group},
    };
    use signal_hook::{
        consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM},
        flag,
    };
    use std::{
        io::Read,
        os::{fd::AsFd, unix::process::CommandExt},
        process::{Child, Command, Stdio},
        sync::{
            Arc, Mutex, MutexGuard, OnceLock,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    struct Signals {
        cancelled: Arc<AtomicBool>,
        default: Arc<AtomicBool>,
    }
    static SIGNALS: OnceLock<io::Result<Signals>> = OnceLock::new();
    static EXECUTION: Mutex<()> = Mutex::new(());

    /// Signal ownership spans the whole check stage, including revalidation.
    /// Outside it, signals retain their default terminating behavior. The
    /// registrations persist: unregister alone would not restore that behavior.
    pub struct Cancellation {
        signals: &'static Signals,
        _lock: MutexGuard<'static, ()>,
    }
    impl Cancellation {
        pub fn new() -> Result<Self, Error> {
            let lock = EXECUTION
                .lock()
                .map_err(|_| io::Error::other("check executor poisoned"))?;
            let signals = SIGNALS
                .get_or_init(|| {
                    let signals = Signals {
                        cancelled: Arc::new(AtomicBool::new(false)),
                        default: Arc::new(AtomicBool::new(true)),
                    };
                    for signal in [SIGINT, SIGTERM, SIGHUP, SIGQUIT] {
                        flag::register_conditional_default(signal, signals.default.clone())?;
                        flag::register(signal, signals.cancelled.clone())?;
                    }
                    Ok(signals)
                })
                .as_ref()
                .map_err(|e| io::Error::new(e.kind(), e.to_string()))?;
            signals.cancelled.store(false, Ordering::SeqCst);
            signals.default.store(false, Ordering::SeqCst);
            Ok(Self {
                signals,
                _lock: lock,
            })
        }
        pub fn check(&self) -> Result<(), Error> {
            if self.signals.cancelled.load(Ordering::SeqCst) {
                Err(Error::Interrupted)
            } else {
                Ok(())
            }
        }
        pub fn complete(self) -> Result<(), Error> {
            self.signals.default.store(true, Ordering::SeqCst);
            self.check()
        }
    }
    impl Drop for Cancellation {
        fn drop(&mut self) {
            self.signals.default.store(true, Ordering::SeqCst);
        }
    }

    // The group is killed before reaping its leader, so its ID cannot be reused.
    // This also cleans up ordinary background descendants after a shell exits.
    struct ManagedChild {
        child: Child,
        group: Option<Pid>,
    }
    impl Drop for ManagedChild {
        fn drop(&mut self) {
            if let Some(group) = self.group.take() {
                let _ = kill_process_group(group, Signal::KILL);
            }
            let _ = self.child.wait();
        }
    }
    fn nonblocking(pipe: &impl AsFd) -> io::Result<()> {
        fcntl_setfl(pipe, fcntl_getfl(pipe)? | OFlags::NONBLOCK)?;
        Ok(())
    }
    fn drain(
        pipe: &mut impl Read,
        stream: Stream,
        output: &mut impl FnMut(Stream, &[u8]),
    ) -> io::Result<bool> {
        // Bound each drain so continuous output cannot starve cancellation.
        for _ in 0..16 {
            let mut bytes = [0; 8192];
            match pipe.read(&mut bytes) {
                Ok(0) => return Ok(false),
                Ok(n) => output(stream, &bytes[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
    pub fn run(
        directory: &Path,
        command: &[String],
        cancellation: &Cancellation,
        mut output: impl FnMut(Stream, &[u8]),
    ) -> Result<ExitStatus, Error> {
        cancellation.check()?;
        let child = Command::new(&command[0])
            .args(&command[1..])
            .current_dir(directory)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut child = ManagedChild {
            group: Pid::from_raw(child.id() as i32),
            child,
        };
        let mut stdout = child.child.stdout.take().expect("piped stdout");
        let mut stderr = child.child.stderr.take().expect("piped stderr");
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        loop {
            cancellation.check()?;
            drain(&mut stdout, Stream::Stdout, &mut output)?;
            drain(&mut stderr, Stream::Stderr, &mut output)?;
            // WNOWAIT preserves the leader PID until group cleanup in Drop.
            let exited = rustix::process::waitid(
                rustix::process::WaitId::Pid(
                    Pid::from_raw(child.child.id() as i32).expect("child PID"),
                ),
                rustix::process::WaitIdOptions::EXITED
                    | rustix::process::WaitIdOptions::NOHANG
                    | rustix::process::WaitIdOptions::NOWAIT,
            )
            .map_err(io::Error::from)?;
            if exited.is_some() {
                let group = Pid::from_raw(child.child.id() as i32).expect("child PID");
                kill_process_group(group, Signal::KILL).map_err(io::Error::from)?;
                child.group = None;
                // Collect all available buffered output, including enlarged pipes,
                // without waiting on descriptors held by escaped descendants.
                while drain(&mut stdout, Stream::Stdout, &mut output)? {
                    cancellation.check()?;
                }
                while drain(&mut stderr, Stream::Stderr, &mut output)? {
                    cancellation.check()?;
                }
                cancellation.check()?;
                return Ok(child.child.wait()?);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use super::*;
    pub struct Cancellation;
    impl Cancellation {
        pub fn new() -> Result<Self, Error> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "finish checks require Unix process-group management",
            )
            .into())
        }
        pub fn check(&self) -> Result<(), Error> {
            Ok(())
        }
        pub fn complete(self) -> Result<(), Error> {
            Ok(())
        }
    }
    pub fn run(
        _: &Path,
        _: &[String],
        _: &Cancellation,
        _: impl FnMut(Stream, &[u8]),
    ) -> Result<ExitStatus, Error> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "finish checks require Unix process-group management",
        )
        .into())
    }
}
pub use platform::{Cancellation, run};
