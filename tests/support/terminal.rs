//! Real Linux PTYs, using the rustix dependency already present through tempfile.
//! Reads and process waits have deadlines; Drop kills a stuck test child.
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process},
    pty::{OpenptFlags, grantpt, ioctl_tiocgptpeer, openpt, unlockpt},
};

pub struct Terminal {
    child: Child,
    master: File,
    pub output: String,
}
impl Terminal {
    pub fn start(executable: &str, directory: &Path, args: &[&str]) -> Self {
        let mut command = Command::new(executable);
        command
            .current_dir(directory)
            .args(args)
            .env("TERM", "dumb");
        Self::start_command(command)
    }
    #[allow(dead_code)]
    pub fn start_capable(executable: &str, directory: &Path, args: &[&str]) -> Self {
        let mut command = Command::new(executable);
        command
            .current_dir(directory)
            .args(args)
            .env("TERM", "xterm-256color")
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR");
        Self::start_command(command)
    }
    #[allow(dead_code)]
    pub fn start_capable_narrow(executable: &str, directory: &Path, args: &[&str]) -> Self {
        let mut command = Command::new(executable);
        command
            .current_dir(directory)
            .args(args)
            .env("TERM", "xterm-256color")
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR");
        Self::start_command_width(command, 40)
    }
    pub fn start_command(command: Command) -> Self {
        Self::start_command_width(command, 80)
    }
    pub fn start_command_width(mut command: Command, columns: u16) -> Self {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::CLOEXEC).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let slave = File::from(
            ioctl_tiocgptpeer(&master, OpenptFlags::RDWR | OpenptFlags::CLOEXEC).unwrap(),
        );
        fcntl_setfl(&master, fcntl_getfl(&master).unwrap() | OFlags::NONBLOCK).unwrap();
        rustix::termios::tcsetwinsize(
            &master,
            rustix::termios::Winsize {
                ws_row: 24,
                ws_col: columns,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        let child = command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave))
            .spawn()
            .unwrap();
        Self {
            child,
            master: File::from(master),
            output: String::new(),
        }
    }
    #[allow(dead_code)]
    pub fn assert_restored(&self) {
        use rustix::termios::LocalModes;
        let modes = rustix::termios::tcgetattr(&self.master)
            .unwrap()
            .local_modes;
        assert!(
            modes.contains(LocalModes::ICANON | LocalModes::ECHO),
            "terminal left in raw mode: {modes:?}"
        );
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
    fn read(&mut self) {
        let mut bytes = [0; 8192];
        loop {
            match self.master.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => self.output.push_str(&String::from_utf8_lossy(&bytes[..n])),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                // Linux reports EIO after the last slave descriptor is closed.
                Err(error) if error.raw_os_error() == Some(5) => break,
                Err(error) => panic!("PTY read: {error}"),
            }
        }
    }
    pub fn until(&mut self, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.read();
            if self.output.contains(expected) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "exited before {expected:?}: {}",
                self.output
            );
            assert!(
                Instant::now() < deadline,
                "timeout awaiting {expected:?}: {}",
                self.output
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn read_with_cursor_reports(&mut self, scan: &mut usize) {
        self.read();
        let queries = self.output.as_bytes()[*scan..]
            .windows(4)
            .filter(|bytes| *bytes == b"\x1b[6n")
            .count();
        for _ in 0..queries {
            self.send(b"\x1b[1;1R");
        }
        *scan = self.output.len().saturating_sub(3);
    }
    /// PowerShell's console queries cursor position before reading input. A
    /// minimal terminal response lets PTY tests exercise its native editor.
    #[allow(dead_code)]
    pub fn until_with_cursor_reports(&mut self, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut scan = self.output.len().saturating_sub(3);
        loop {
            self.read_with_cursor_reports(&mut scan);
            if self.output.contains(expected) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "exited before {expected:?}: {}",
                self.output
            );
            assert!(
                Instant::now() < deadline,
                "timeout awaiting {expected:?}: {}",
                self.output
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[allow(dead_code)]
    pub fn finish_with_cursor_reports(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut scan = self.output.len().saturating_sub(3);
        loop {
            self.read_with_cursor_reports(&mut scan);
            if let Some(status) = self.child.try_wait().unwrap() {
                self.read();
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "PTY process did not finish: {}",
                self.output
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn send(&mut self, input: &[u8]) {
        self.master.write_all(input).unwrap();
    }
    pub fn interrupt(&self) {
        kill_process(Pid::from_raw(self.child.id() as i32).unwrap(), Signal::INT).unwrap();
    }
    pub fn finish(&mut self) -> ExitStatus {
        self.finish_with_timeout(Duration::from_secs(10))
    }
    pub fn finish_with_timeout(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            self.read();
            if let Some(status) = self.child.try_wait().unwrap() {
                self.read();
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "PTY process did not finish: {}",
                self.output
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
