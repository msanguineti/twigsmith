//! Windows process ownership stays with one non-inheritable job until every
//! managed process has stopped. Spawn suspended so assignment precedes user code.
use super::*;
use std::{
    io::Read,
    mem::{offset_of, size_of},
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
        ERROR_BROKEN_PIPE, ERROR_MORE_DATA, ERROR_NO_MORE_FILES, ERROR_PIPE_NOT_CONNECTED, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    System::{
        Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler},
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_PROCESS_ID_LIST,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject,
        },
        Pipes::PeekNamedPipe,
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, GetProcessIdOfThread, INFINITE,
            OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE, ResumeThread, THREAD_QUERY_LIMITED_INFORMATION,
            THREAD_SUSPEND_RESUME, TerminateProcess, WaitForSingleObject,
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
        let result = self.stop_members();
        if result.is_err() {
            // SAFETY: valid job. Request containment even when acquiring or
            // waiting an individual member failed; preserve that cleanup error.
            // Last-handle closure remains the additional unwinding fallback.
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        }
        result
    }

    fn process_ids(&self) -> io::Result<Vec<u32>> {
        // The Win32 structure ends with a variable-length ULONG_PTR array. A
        // usize buffer gives both the header and array their required alignment.
        const HEADER: usize =
            offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList) / size_of::<usize>();
        let mut buffer = vec![0usize; HEADER + 32];
        loop {
            let bytes = u32::try_from(buffer.len() * size_of::<usize>())
                .map_err(|_| io::Error::other("check job process list is too large"))?;
            // SAFETY: valid job and aligned writable storage including the
            // variable process-ID array, whose byte length is supplied exactly.
            let result = unsafe {
                QueryInformationJobObject(
                    self.0.as_raw_handle(),
                    JobObjectBasicProcessIdList,
                    buffer.as_mut_ptr().cast(),
                    bytes,
                    null_mut(),
                )
            };
            if result == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
                    return Err(error);
                }
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            // SAFETY: successful query initialized the fixed header; storage is
            // aligned and large enough for the structure and its first array slot.
            let information =
                unsafe { &*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
            let count = information.NumberOfProcessIdsInList as usize;
            if count < information.NumberOfAssignedProcesses as usize {
                buffer.resize(HEADER + information.NumberOfAssignedProcesses as usize, 0);
                continue;
            }
            if count > buffer.len() - HEADER {
                return Err(io::Error::other("invalid check job process list length"));
            }
            return buffer[HEADER..HEADER + count]
                .iter()
                .map(|&pid| {
                    u32::try_from(pid)
                        .map_err(|_| io::Error::other("invalid check job process identifier"))
                })
                .collect();
        }
    }

    fn stop_members(&self) -> io::Result<()> {
        loop {
            let pids = self.process_ids()?;
            if pids.is_empty() {
                return Ok(());
            }
            let mut members = Vec::new();
            for pid in pids {
                // Retain handles BEFORE requesting termination. TerminateJobObject
                // can remove members from the job list/accounting before their
                // process objects become signaled; querying after it loses the
                // handles needed to observe actual completion.
                // SAFETY: request an owned, non-inheritable process handle.
                let process = owned_handle(unsafe {
                    OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                        0,
                        pid,
                    )
                })?;
                let mut member = 0;
                // SAFETY: valid queryable process and job, writable BOOL output.
                if unsafe {
                    IsProcessInJob(process.as_raw_handle(), self.0.as_raw_handle(), &mut member)
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if member != 0 {
                    members.push(process);
                } else if !process_stopped(&process, 0)? {
                    return Err(io::Error::other(
                        "check process membership changed before termination",
                    ));
                }
                // A recycled PID may name an unrelated process. Never terminate
                // it, and do not infer completion from absent membership alone.
            }
            for process in &members {
                if !process_stopped(process, 0)? {
                    // SAFETY: retained handle has terminate access. A natural
                    // exit can race this call; only a signaled handle makes a
                    // failed termination request safe to treat as completed.
                    if unsafe { TerminateProcess(process.as_raw_handle(), 1) } == 0 {
                        let error = io::Error::last_os_error();
                        if !process_stopped(process, 0)? {
                            return Err(error);
                        }
                    }
                }
            }
            for process in &members {
                process_stopped(process, INFINITE)?;
            }
            // A captured member could create a child before its termination was
            // requested. Once all captured parents have signaled, acquire the
            // remaining members and repeat; do not assume one snapshot is final.
        }
    }
}

fn process_stopped(process: &OwnedHandle, timeout: u32) -> io::Result<bool> {
    // SAFETY: valid retained process handle with synchronization access.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), timeout) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        _ => Err(io::Error::other("unexpected check process wait result")),
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
