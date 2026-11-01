//! Native foreground terminal harness. Deadlines bound interaction; Drop owns
//! the Unix process group / Windows job, including the foreground editor.
//! Detached editors that leave those containers are outside this fixture.
use std::{
    process::{Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

pub struct NativeTerminal {
    inner: platform::Terminal,
    pub output: String,
}

impl NativeTerminal {
    pub fn start_command(mut command: Command) -> Self {
        command.env("TERM", "dumb");
        Self {
            inner: platform::Terminal::start(command),
            output: String::new(),
        }
    }

    fn read(&mut self) {
        self.inner.read(&mut self.output);
    }

    pub fn until(&mut self, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.read();
            if self.output.contains(expected) {
                return;
            }
            if self.inner.try_wait().is_some() {
                self.read();
                assert!(
                    self.output.contains(expected),
                    "exited before {expected:?}: {}",
                    self.output
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timeout awaiting {expected:?}: {}",
                self.output
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn send(&mut self, input: &[u8]) {
        self.inner.send(input);
    }

    pub fn finish(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.read();
            if let Some(status) = self.inner.try_wait() {
                // ConPTY's output pump is asynchronous with process exit.
                // A short quiet period collects the terminal's final report.
                let mut quiet = Instant::now();
                while quiet.elapsed() < Duration::from_millis(100) && Instant::now() < deadline {
                    let previous = self.output.len();
                    self.read();
                    if self.output.len() != previous {
                        quiet = Instant::now();
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "terminal process did not finish: {}",
                self.output
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn assert_restored(&mut self) {
        self.inner.assert_restored(&mut self.output);
        if let Some(hidden) = self.output.rfind("\x1b[?25l") {
            assert!(
                self.output
                    .rfind("\x1b[?25h")
                    .is_some_and(|shown| shown > hidden),
                "cursor left hidden: {}",
                self.output
            );
        }
    }
}

#[cfg(unix)]
mod platform {
    use rustix::{
        fs::{OFlags, fcntl_getfl, fcntl_setfl},
        process::{Pid, Signal, kill_process_group},
        pty::{OpenptFlags, grantpt, openpt, unlockpt},
        termios::{LocalModes, Winsize, tcgetattr, tcsetwinsize},
    };
    use std::{
        fs::File,
        io::{self, Read, Write},
        os::unix::process::CommandExt,
        process::{Child, Command, ExitStatus, Stdio},
    };

    pub struct Terminal {
        child: Child,
        master: File,
    }
    impl Terminal {
        pub fn start(mut command: Command) -> Self {
            let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
            rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC).unwrap();
            grantpt(&master).unwrap();
            unlockpt(&master).unwrap();
            #[cfg(target_os = "linux")]
            let slave = File::from(
                rustix::pty::ioctl_tiocgptpeer(&master, OpenptFlags::RDWR | OpenptFlags::CLOEXEC)
                    .unwrap(),
            );
            #[cfg(not(target_os = "linux"))]
            let slave = {
                let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
                File::from(
                    rustix::fs::open(
                        name.as_c_str(),
                        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
                        rustix::fs::Mode::empty(),
                    )
                    .unwrap(),
                )
            };
            fcntl_setfl(&master, fcntl_getfl(&master).unwrap() | OFlags::NONBLOCK).unwrap();
            tcsetwinsize(
                &master,
                Winsize {
                    ws_row: 24,
                    ws_col: 100,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                },
            )
            .unwrap();
            let child = command
                .process_group(0)
                .stdin(Stdio::from(slave.try_clone().unwrap()))
                .stdout(Stdio::from(slave.try_clone().unwrap()))
                .stderr(Stdio::from(slave))
                .spawn()
                .unwrap();
            Self {
                child,
                master: File::from(master),
            }
        }
        pub fn read(&mut self, output: &mut String) {
            let mut bytes = [0; 8192];
            loop {
                match self.master.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => output.push_str(&String::from_utf8_lossy(&bytes[..n])),
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            || error.raw_os_error() == Some(5) =>
                    {
                        break;
                    }
                    Err(error) => panic!("native PTY read: {error}"),
                }
            }
        }
        pub fn send(&mut self, input: &[u8]) {
            self.master.write_all(input).unwrap();
        }
        pub fn try_wait(&mut self) -> Option<ExitStatus> {
            self.child.try_wait().unwrap()
        }
        pub fn assert_restored(&mut self, _: &mut String) {
            let modes = tcgetattr(&self.master).unwrap().local_modes;
            assert!(
                modes.contains(LocalModes::ICANON | LocalModes::ECHO),
                "terminal left in raw mode: {modes:?}"
            );
        }
    }
    impl Drop for Terminal {
        fn drop(&mut self) {
            if let Some(group) = Pid::from_raw(self.child.id() as i32) {
                let _ = kill_process_group(group, Signal::KILL);
            }
            let _ = self.child.wait();
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::{
        collections::BTreeMap,
        ffi::{OsStr, OsString},
        fs::File,
        io::{self, Read, Write},
        mem::{size_of, zeroed},
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::ExitStatusExt,
        },
        process::{Command, ExitStatus},
        ptr::{null, null_mut},
        sync::mpsc::{self, Receiver},
        thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON},
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject,
            },
            Pipes::CreatePipe,
            Threading::{
                CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
                DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
                InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                PROCESS_INFORMATION, ResumeThread, STARTUPINFOEXW, UpdateProcThreadAttribute,
                WaitForSingleObject,
            },
        },
    };

    fn check(ok: i32) {
        assert_ne!(
            ok,
            0,
            "Windows terminal API: {}",
            io::Error::last_os_error()
        );
    }
    unsafe fn own(handle: HANDLE) -> OwnedHandle {
        assert!(
            !handle.is_null(),
            "Windows handle: {}",
            io::Error::last_os_error()
        );
        unsafe { OwnedHandle::from_raw_handle(handle) }
    }
    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    // CreateProcess receives a command line, not an argv array. This is the
    // standard CRT quoting rule, including trailing backslashes before quotes.
    fn quote(value: &OsStr) -> Vec<u16> {
        let mut result = vec![b'"' as u16];
        let mut slashes = 0;
        for unit in value.encode_wide() {
            if unit == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            if unit == b'"' as u16 {
                result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
            } else {
                result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
            }
            slashes = 0;
            result.push(unit);
        }
        result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        result.push(b'"' as u16);
        result
    }

    struct Console(HPCON);
    impl Drop for Console {
        fn drop(&mut self) {
            unsafe {
                ClosePseudoConsole(self.0);
            }
        }
    }

    pub struct Terminal {
        process: OwnedHandle,
        job: OwnedHandle,
        console: Console,
        input: File,
        output: Receiver<Vec<u8>>,
    }
    impl Terminal {
        pub fn start(command: Command) -> Self {
            unsafe {
                let mut input_read = null_mut();
                let mut input_write = null_mut();
                check(CreatePipe(&mut input_read, &mut input_write, null(), 0));
                let input_read = own(input_read);
                let input = File::from(own(input_write));
                let mut output_read = null_mut();
                let mut output_write = null_mut();
                check(CreatePipe(&mut output_read, &mut output_write, null(), 0));
                let mut reader = File::from(own(output_read));
                let output_write = own(output_write);
                let mut console = 0;
                let result = CreatePseudoConsole(
                    COORD { X: 100, Y: 24 },
                    input_read.as_raw_handle(),
                    output_write.as_raw_handle(),
                    0,
                    &mut console,
                );
                assert!(result >= 0, "CreatePseudoConsole HRESULT: {result:x}");
                let console = Console(console);
                drop(input_read);
                drop(output_write);
                // Keep draining while ClosePseudoConsole tears down its host;
                // synchronous closure can wait for outstanding console output.
                let (send, output) = mpsc::channel();
                thread::spawn(move || {
                    let mut bytes = [0; 8192];
                    while let Ok(n) = reader.read(&mut bytes) {
                        if n == 0 {
                            break;
                        }
                        let _ = send.send(bytes[..n].to_vec());
                    }
                });
                let job = own(CreateJobObjectW(null(), null()));
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                check(SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ));
                let process = Self::spawn(&command, console.0, &job);
                Self {
                    process,
                    job,
                    console,
                    input,
                    output,
                }
            }
        }

        unsafe fn spawn(command: &Command, console: HPCON, job: &OwnedHandle) -> OwnedHandle {
            unsafe {
                let mut bytes = 0;
                InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
                // usize storage supplies the alignment required by the API.
                let mut attributes = vec![0usize; bytes.div_ceil(size_of::<usize>())];
                let list = attributes.as_mut_ptr().cast();
                check(InitializeProcThreadAttributeList(list, 1, 0, &mut bytes));
                struct AttributeList(
                    windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST,
                );
                impl Drop for AttributeList {
                    fn drop(&mut self) {
                        unsafe {
                            DeleteProcThreadAttributeList(self.0);
                        }
                    }
                }
                let _delete = AttributeList(list);
                check(UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    console as *const _,
                    size_of::<HPCON>(),
                    null_mut(),
                    null(),
                ));
                let mut startup: STARTUPINFOEXW = zeroed();
                startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
                startup.lpAttributeList = list;
                let application = wide(command.get_program());
                let mut line = quote(command.get_program());
                for arg in command.get_args() {
                    line.push(b' ' as u16);
                    line.extend(quote(arg));
                }
                line.push(0);
                // Command exposes explicit env changes. Start with this test's
                // inherited environment, then apply removes/overrides. Sorting
                // keys without case preserves Windows' environment convention.
                let mut environment: BTreeMap<String, (OsString, OsString)> = std::env::vars_os()
                    .map(|(key, value)| (key.to_string_lossy().to_uppercase(), (key, value)))
                    .collect();
                for (key, value) in command.get_envs() {
                    let canonical = key.to_string_lossy().to_uppercase();
                    if let Some(value) = value {
                        environment.insert(canonical, (key.into(), value.into()));
                    } else {
                        environment.remove(&canonical);
                    }
                }
                let mut block = Vec::new();
                for (key, value) in environment.values() {
                    block.extend(key.encode_wide());
                    block.push(b'=' as u16);
                    block.extend(value.encode_wide());
                    block.push(0);
                }
                block.push(0);
                let cwd = command.get_current_dir().map(|path| wide(path.as_os_str()));
                let mut process: PROCESS_INFORMATION = zeroed();
                check(CreateProcessW(
                    application.as_ptr(),
                    line.as_mut_ptr(),
                    null(),
                    null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
                    block.as_ptr().cast(),
                    cwd.as_ref().map_or(null(), |path| path.as_ptr()),
                    &startup.StartupInfo,
                    &mut process,
                ));
                let handle = own(process.hProcess);
                let thread = own(process.hThread);
                if AssignProcessToJobObject(job.as_raw_handle(), handle.as_raw_handle()) == 0 {
                    windows_sys::Win32::System::Threading::TerminateProcess(
                        handle.as_raw_handle(),
                        1,
                    );
                    panic!("AssignProcessToJobObject: {}", io::Error::last_os_error());
                }
                assert_ne!(ResumeThread(thread.as_raw_handle()), u32::MAX);
                handle
            }
        }

        pub fn read(&mut self, output: &mut String) {
            for bytes in self.output.try_iter() {
                output.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        pub fn send(&mut self, input: &[u8]) {
            let input: Vec<_> = input
                .iter()
                .map(|&byte| if byte == b'\n' { b'\r' } else { byte })
                .collect();
            self.input.write_all(&input).unwrap();
        }
        pub fn try_wait(&mut self) -> Option<ExitStatus> {
            unsafe {
                match WaitForSingleObject(self.process.as_raw_handle(), 0) {
                    WAIT_OBJECT_0 => {
                        let mut code = 0;
                        check(GetExitCodeProcess(self.process.as_raw_handle(), &mut code));
                        Some(ExitStatus::from_raw(code))
                    }
                    WAIT_TIMEOUT => None,
                    other => panic!("WaitForSingleObject: {other:x}"),
                }
            }
        }
        pub fn assert_restored(&mut self, output: &mut String) {
            assert!(
                self.try_wait().is_some(),
                "restoration probe requires finished CLI"
            );
            let fixture = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("examples")
                .join("native_editor_fixture.exe");
            let mut command = Command::new(fixture);
            command.arg("--terminal-probe");
            let probe = unsafe { Self::spawn(&command, self.console.0, &self.job) };
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                self.read(output);
                if unsafe { WaitForSingleObject(probe.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
                    let mut code = 0;
                    unsafe {
                        check(GetExitCodeProcess(probe.as_raw_handle(), &mut code));
                    }
                    assert_eq!(code, 0, "console restoration probe failed: {output}");
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "console restoration probe timed out: {output}"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    impl Drop for Terminal {
        fn drop(&mut self) {
            unsafe {
                TerminateJobObject(self.job.as_raw_handle(), 1);
            }
            // The output reader remains alive while the pseudoconsole closes.
        }
    }
}
