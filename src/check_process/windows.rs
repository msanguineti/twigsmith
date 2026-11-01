//! Windows process ownership stays with one non-inheritable job until every
//! managed process has stopped. Spawn suspended so assignment precedes user code.
use super::*;
use std::{
    io::Read,
    mem::size_of,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_NO_MORE_FILES, ERROR_PIPE_NOT_CONNECTED, HANDLE,
        INVALID_HANDLE_VALUE,
    },
    System::{
        Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler},
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Pipes::PeekNamedPipe,
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, GetProcessIdOfThread, OpenThread,
            ResumeThread, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
        },
    },
};

static EXECUTION: Mutex<()> = Mutex::new(());
static REGISTERED: OnceLock<io::Result<()>> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CANCELLED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn control_event(event: u32) -> i32 {
    if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) && ACTIVE.load(Ordering::SeqCst) {
        CANCELLED.store(true, Ordering::SeqCst);
        1
    } else {
        // Continue the handler chain, including Windows' default termination,
        // outside the check stage. Console-close/logoff have separate OS limits.
        0
    }
}

/// Console control ownership spans all checks and their revalidation.
pub struct Cancellation {
    _lock: MutexGuard<'static, ()>,
}
impl Cancellation {
    pub fn new() -> Result<Self, Error> {
        let lock = EXECUTION
            .lock()
            .map_err(|_| io::Error::other("check executor poisoned"))?;
        REGISTERED
            .get_or_init(|| {
                // SAFETY: the callback only accesses static atomics and remains
                // valid for the process lifetime; Windows owns its invocation.
                if unsafe { SetConsoleCtrlHandler(Some(control_event), 1) } == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            })
            .as_ref()
            .map_err(|e| io::Error::new(e.kind(), e.to_string()))?;
        CANCELLED.store(false, Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
        Ok(Self { _lock: lock })
    }
    pub fn check(&self) -> Result<(), Error> {
        if CANCELLED.load(Ordering::SeqCst) {
            Err(Error::Interrupted)
        } else {
            Ok(())
        }
    }
    pub fn complete(self) -> Result<(), Error> {
        ACTIVE.store(false, Ordering::SeqCst);
        self.check()
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::SeqCst);
    }
}

fn owned_handle(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: each caller passes a fresh owned handle returned by Win32.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

struct Job(OwnedHandle);
impl Job {
    fn new() -> io::Result<Self> {
        // SAFETY: null security attributes make the unnamed job non-inheritable.
        let job = Self(owned_handle(unsafe { CreateJobObjectW(null(), null()) })?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // No breakaway flags: descendants remain in the job, including processes
        // creating new consoles or new process groups. Nested host jobs may impose
        // additional constraints; assignment failure stops the suspended leader.
        // SAFETY: valid owned job and correctly sized initialized input structure.
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn stop(&self) -> io::Result<()> {
        // SAFETY: the non-inheritable job handle is valid for this whole call.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // Termination is asynchronous. A stopped leader alone does not establish
        // that descendants (including pipe holders) have finished termination.
        loop {
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            // SAFETY: valid job, writable correctly sized output structure.
            if unsafe {
                QueryInformationJobObject(
                    self.0.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            if accounting.ActiveProcesses == 0 {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

struct ManagedChild {
    child: Child,
    job: Job,
    assigned: bool,
    stopped: bool,
}
impl ManagedChild {
    fn stop(&mut self) -> io::Result<()> {
        if !self.stopped {
            if self.assigned {
                self.job.stop()?;
            } else {
                // Assignment can fail under host job restrictions. The child is
                // still suspended and cannot have created descendants.
                self.child.kill()?;
            }
            self.child.wait()?;
            self.stopped = true;
        }
        Ok(())
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.stop();
        // Closing the last job handle additionally covers unwinding/API errors.
    }
}

fn resume(child: &Child) -> io::Result<()> {
    // Stable Rust exposes no primary-thread handle on Child. A suspended process
    // has not run its loader or check code, so its single initial thread can be
    // located without a spawn/assignment race using a thread snapshot.
    // SAFETY: flags request a system thread snapshot; no pointers are supplied.
    let snapshot = owned_handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut initial_thread = None;
    // SAFETY: the snapshot is valid and entry is a correctly sized output buffer.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    while found != 0 {
        if entry.th32OwnerProcessID == child.id()
            && initial_thread.replace(entry.th32ThreadID).is_some()
        {
            return Err(io::Error::other(
                "suspended check has multiple initial threads",
            ));
        }
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // SAFETY: same valid snapshot and writable entry as above.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
        return Err(error);
    }
    let id =
        initial_thread.ok_or_else(|| io::Error::other("cannot find suspended check thread"))?;
    // SAFETY: id came from the snapshot; identity is verified on the retained
    // thread handle before resuming, including if an external actor killed the
    // suspended child and Windows reused its thread ID after the snapshot.
    let thread = owned_handle(unsafe {
        OpenThread(
            THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
            0,
            id,
        )
    })?;
    // SAFETY: valid thread handle with query access. Retaining the child process
    // handle keeps that process ID from being reused during this comparison.
    let owner = unsafe { GetProcessIdOfThread(thread.as_raw_handle()) };
    if owner == 0 {
        return Err(io::Error::last_os_error());
    }
    if owner != child.id() {
        return Err(io::Error::other("suspended check thread identity changed"));
    }
    // SAFETY: owned thread handle has the requested suspend/resume access.
    match unsafe { ResumeThread(thread.as_raw_handle()) } {
        1 => Ok(()),
        u32::MAX => Err(io::Error::last_os_error()),
        _ => Err(io::Error::other("unexpected suspended check thread state")),
    }
}

fn drain(
    pipe: &mut (impl Read + AsRawHandle),
    stream: Stream,
    output: &mut impl FnMut(Stream, &[u8]),
) -> io::Result<bool> {
    // Only this thread reads these handles. Peek precedes each bounded read, so
    // no read waits for new output or EOF from a still-running descendant. There
    // are no competing synchronous operations on the handle to block the peek.
    for _ in 0..16 {
        let mut available = 0;
        // SAFETY: valid pipe read handle and a writable availability output; all
        // optional buffers are null because no bytes are copied by this call.
        if unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(code) if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32)
            {
                return Ok(false);
            }
            return Err(error);
        }
        if available == 0 {
            return Ok(false);
        }
        let mut bytes = [0; 8192];
        let length = bytes.len().min(available as usize);
        match pipe.read(&mut bytes[..length]) {
            Ok(0) => return Ok(false),
            Ok(n) => output(stream, &bytes[..n]),
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
    // Rust automatically routes batch files through cmd.exe. Require that shell
    // to be explicit, preserving the executable-vector contract on Windows too.
    let executable = command[0].trim_end_matches(['.', ' ']).to_ascii_lowercase();
    if executable.ends_with(".bat") || executable.ends_with(".cmd") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "finish checks require an explicit shell executable to run a Windows batch file",
        )
        .into());
    }
    let job = Job::new()?;
    let child = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(directory)
        .creation_flags(CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut child = ManagedChild {
        child,
        job,
        assigned: false,
        stopped: false,
    };
    // Every error after spawning runs explicit cleanup, so an interruption is
    // reported as managed processes stopped only after cleanup actually succeeds.
    let result = (|| {
        // SAFETY: both owned handles are valid; the child is still suspended.
        if unsafe {
            AssignProcessToJobObject(child.job.0.as_raw_handle(), child.child.as_raw_handle())
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        child.assigned = true;
        cancellation.check()?;
        resume(&child.child)?;
        let mut stdout = child.child.stdout.take().expect("piped stdout");
        let mut stderr = child.child.stderr.take().expect("piped stderr");
        loop {
            cancellation.check()?;
            drain(&mut stdout, Stream::Stdout, &mut output)?;
            drain(&mut stderr, Stream::Stderr, &mut output)?;
            if let Some(status) = child.child.try_wait()? {
                child.stop()?;
                // All job writers are now stopped; consume every buffered byte.
                while drain(&mut stdout, Stream::Stdout, &mut output)? {
                    cancellation.check()?;
                }
                while drain(&mut stderr, Stream::Stderr, &mut output)? {
                    cancellation.check()?;
                }
                cancellation.check()?;
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    child.stop()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_files_require_an_explicit_shell() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["unsafe.cmd", "unsafe.bat"] {
            std::fs::write(directory.path().join(name), "@echo implicit>ran\r\n").unwrap();
        }
        let cancellation = Cancellation::new().unwrap();
        for executable in [
            "unsafe.cmd",
            "unsafe.CMD",
            "unsafe.cmd. ",
            "unsafe.BAT",
            "unsafe.bat. ",
        ] {
            let mut output = Vec::new();
            let result = run(
                directory.path(),
                &[executable.into()],
                &cancellation,
                |_, bytes| output.extend_from_slice(bytes),
            );
            assert!(matches!(result, Err(Error::Io(e)) if e.kind() == io::ErrorKind::InvalidInput));
            assert!(output.is_empty());
            assert!(!directory.path().join("ran").exists());
        }

        std::fs::write(
            directory.path().join("explicit.cmd"),
            "@echo explicit>ran\r\n@exit /b 0\r\n",
        )
        .unwrap();
        let status = run(
            directory.path(),
            &[
                "cmd.exe".into(),
                "/d".into(),
                "/c".into(),
                "explicit.cmd".into(),
            ],
            &cancellation,
            |_, _| {},
        )
        .unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("ran"))
                .unwrap()
                .trim(),
            "explicit"
        );
        cancellation.complete().unwrap();
    }
}
