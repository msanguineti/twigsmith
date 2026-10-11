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
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::CLOEXEC).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let slave = File::from(
            ioctl_tiocgptpeer(&master, OpenptFlags::RDWR | OpenptFlags::CLOEXEC).unwrap(),
        );
        fcntl_setfl(&master, fcntl_getfl(&master).unwrap() | OFlags::NONBLOCK).unwrap();
        let child = Command::new(executable)
            .current_dir(directory)
            .args(args)
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
