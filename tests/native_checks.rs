//! Managed commands exercised with native executables, signals and process trees.
#[path = "support/native.rs"]
mod native;
mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use support::{Repository, isolated};
#[path = "../src/check_process.rs"]
mod check_process;
use check_process::{Cancellation, Error, Stream};

fn wait_until(description: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {description}");
        thread::sleep(Duration::from_millis(10));
    }
}

// Cleanup owns only recorded fixture PIDs. It runs even if a readiness or
// containment assertion fails, so a broken executor cannot leak infinite writers.
struct Tree {
    root: PathBuf,
}
impl Tree {
    fn pids(&self) -> Vec<u32> {
        Self::pids_at(&self.root)
    }
    fn pids_at(root: &Path) -> Vec<u32> {
        ["leader", "ordinary", "stubborn-pipe-holder"]
            .into_iter()
            .filter_map(|name| fs::read_to_string(root.join(format!("{name}.pid"))).ok())
            .map(|pid| pid.parse().unwrap())
            .collect()
    }
    fn observe(&self) -> Vec<ObservedProcess> {
        Self::observe_at(&self.root)
    }
    fn observe_at(root: &Path) -> Vec<ObservedProcess> {
        let pids = Self::pids_at(root);
        assert_eq!(
            pids.len(),
            3,
            "all fixture processes must have become ready"
        );
        pids.into_iter().map(ObservedProcess::new).collect()
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        for pid in self.pids() {
            terminate(pid);
        }
    }
}

// Killing a stalled isolated test must also clean its managed fixture processes.
// A test-only watchdog bounds bugs inside run(), not just the happy-path helpers.
struct Deadline(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Deadline {
    fn new(root: &Path) -> Self {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let finished = Arc::new(AtomicBool::new(false));
        let watch = finished.clone();
        let root = root.to_owned();
        thread::spawn(move || {
            let expires = Instant::now() + Duration::from_secs(30);
            while !watch.load(Ordering::SeqCst) {
                if Instant::now() >= expires {
                    eprintln!(
                        "native managed-process test exceeded 30 seconds; cleaning fixture processes"
                    );
                    drop(Tree { root: root.clone() });
                    for name in ["record", "stream", "cli", "controller"] {
                        if let Ok(pid) = fs::read_to_string(root.join(format!("{name}.pid"))) {
                            terminate(pid.parse().unwrap());
                        }
                    }
                    std::process::exit(124);
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        Self(finished)
    }
}
impl Drop for Deadline {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn terminate(pid: u32) {
    if !running(pid) {
        return;
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}

struct ObservedProcess {
    pid: u32,
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}
// A process handle is independent of the thread which opened it.
#[cfg(windows)]
unsafe impl Send for ObservedProcess {}
impl ObservedProcess {
    fn new(pid: u32) -> Self {
        assert!(
            running(pid),
            "fixture {pid} must be live before interruption"
        );
        Self {
            pid,
            #[cfg(windows)]
            handle: unsafe {
                use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
                let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
                assert!(
                    !handle.is_null(),
                    "open live process {pid}: {}",
                    std::io::Error::last_os_error()
                );
                handle
            },
        }
    }
    fn assert_stopped(&self) {
        #[cfg(unix)]
        assert!(
            !running(self.pid),
            "fixture {} survived executor return",
            self.pid
        );
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{
                Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject,
            };
            assert_eq!(
                WaitForSingleObject(self.handle, 0),
                WAIT_OBJECT_0,
                "fixture {} survived executor return",
                self.pid
            );
        }
        println!(
            "independently observed process {} stopped on executor return",
            self.pid
        );
    }
}
#[cfg(windows)]
impl Drop for ObservedProcess {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(unix)]
fn running(pid: u32) -> bool {
    // Orphaned killed children may remain zombies until the host reaper runs.
    // ps independently observes execution state rather than treating zombies as
    // live just because kill(pid, 0) still finds a process-table entry.
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}
#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let result = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        result == WAIT_TIMEOUT
    }
}

struct BoundedChild(Child);
impl BoundedChild {
    fn wait(&mut self) -> std::process::ExitStatus {
        let mut status = None;
        wait_until("native child exits", || {
            status = self.0.try_wait().unwrap();
            status.is_some()
        });
        status.unwrap()
    }
}
impl Drop for BoundedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn native_literal_arguments_environment_eof_and_concurrent_streams() {
    isolated(
        "native_literal_arguments_environment_eof_and_concurrent_streams",
        true,
        |root| {
            let _deadline = Deadline::new(root);
            let observation = root.join("record.json");
            let arguments = [
                "has spaces",
                "quote'\"",
                "Καλημέρα 世界",
                "",
                "$HOME;|&<>*?()",
            ];
            let mut command = native::command("record", &[observation.to_str().unwrap()]);
            command.extend(arguments.iter().map(|arg| (*arg).to_owned()));
            let cancellation = Cancellation::new().unwrap();
            let status = check_process::run(root, &command, &cancellation, |_, _| {}).unwrap();
            assert!(status.success());
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(&observation).unwrap()).unwrap();
            assert_eq!(value["args"], serde_json::json!(arguments));
            assert_eq!(value["stdin"], serde_json::json!([]));
            assert_eq!(value["environment"], std::env::var("HOME").unwrap());
            assert_eq!(
                Path::new(value["cwd"].as_str().unwrap())
                    .canonicalize()
                    .unwrap(),
                root.canonicalize().unwrap()
            );

            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let status = check_process::run(
                root,
                &native::command("stream", &[root.to_str().unwrap()]),
                &cancellation,
                |stream, bytes| {
                    match stream {
                        Stream::Stdout => stdout.extend_from_slice(bytes),
                        Stream::Stderr => stderr.extend_from_slice(bytes),
                    }
                    if stdout.starts_with(b"before-exit stdout\n")
                        && stderr.starts_with(b"before-exit stderr\n")
                    {
                        fs::write(
                            root.join("output-observed"),
                            b"both streams arrived before exit",
                        )
                        .unwrap();
                    }
                },
            )
            .unwrap();
            assert!(status.success());
            cancellation.complete().unwrap();
            let expected = |prefix: &str, line: &str, byte: &str| -> Vec<u8> {
                let mut bytes = format!("before-exit {prefix}\n").into_bytes();
                for i in 0..8192 {
                    bytes.extend_from_slice(
                        format!("{line}-{i:05}:{}\n", byte.repeat(128)).as_bytes(),
                    );
                }
                bytes
            };
            assert_eq!(stdout, expected("stdout", "out", "o"));
            assert_eq!(stderr, expected("stderr", "err", "e"));
            println!(
                "complete ordered per-stream output: stdout={} stderr={} bytes; pre-exit handshake passed",
                stdout.len(),
                stderr.len()
            );
        },
    );
}

#[test]
fn native_missing_executable_nonzero_and_leader_exit_cleanup() {
    isolated(
        "native_missing_executable_nonzero_and_leader_exit_cleanup",
        true,
        |root| {
            let _deadline = Deadline::new(root);
            let cancellation = Cancellation::new().unwrap();
            let error = check_process::run(
                root,
                &[root.join("missing executable").to_str().unwrap().into()],
                &cancellation,
                |_, _| {},
            )
            .unwrap_err();
            assert!(matches!(error, Error::Io(_)), "{error:?}");
            let status = check_process::run(
                root,
                &native::command("exit", &["23"]),
                &cancellation,
                |_, _| {},
            )
            .unwrap();
            assert_eq!(status.code(), Some(23));
            let _tree = Tree {
                root: root.to_owned(),
            };
            let observer_root = root.to_owned();
            let observer = thread::spawn(move || {
                wait_until("leader ready before exit", || {
                    observer_root.join("leader.ready").exists()
                });
                let processes = Tree::observe_at(&observer_root);
                fs::write(
                    observer_root.join("leader-may-exit"),
                    b"observed live identities",
                )
                .unwrap();
                processes
            });
            let start = Instant::now();
            let status = check_process::run(
                root,
                &native::command("tree", &[root.to_str().unwrap(), "exit"]),
                &cancellation,
                |_, _| {},
            )
            .unwrap();
            assert!(status.success());
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "pipe holders delayed leader collection"
            );
            for process in observer.join().unwrap() {
                process.assert_stopped();
            }
            cancellation.complete().unwrap();
        },
    );
}

#[test]
fn native_cli_interrupt_contains_tree_and_preserves_retry() {
    isolated(
        "native_cli_interrupt_contains_tree_and_preserves_retry",
        true,
        |root| {
            let _deadline = Deadline::new(root);
            let remote = Repository::new(root);
            fs::write(remote.path().join("base"), b"base\n").unwrap();
            remote.success(&["add", "."]);
            remote.success(&["commit", "-m", "base"]);
            let repo = Repository::new(root);
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                remote.path().as_os_str(),
            ]);
            repo.success(&["fetch", "origin"]);
            repo.success(&["reset", "--hard", "origin/main"]);
            repo.success(&["switch", "-c", "work"]);
            fs::write(repo.path().join("work"), b"work\n").unwrap();
            repo.success(&["add", "."]);
            repo.success(&["commit", "-m", "work"]);
            let head = repo.success(&["rev-parse", "HEAD"]).stdout;
            let configure = |first: Vec<String>| {
                fs::write(repo.path().join(".git/twigsmith.config.json"), serde_json::to_vec(&serde_json::json!({
                "base": "main", "base-remote": "origin", "publish-remote": "origin", "finish-mode": "review",
                "finish-checks": [
                    {"name": "Native tree", "command": first},
                    {"name": "Later", "command": native::command("ordered", &[root.join("later.log").to_str().unwrap(), "later"])}
                ]
            })).unwrap()).unwrap();
            };
            configure(native::command("tree", &[root.to_str().unwrap()]));
            let tree = Tree {
                root: root.to_owned(),
            };
            let stdout = root.join("cli.stdout");
            let stderr = root.join("cli.stderr");
            let mut command = Command::new(env!("CARGO_BIN_EXE_twigsmith"));
            command
                .current_dir(repo.path())
                .arg("finish")
                .stdin(Stdio::null())
                .stdout(fs::File::create(&stdout).unwrap())
                .stderr(fs::File::create(&stderr).unwrap());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                use windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;
                command.creation_flags(CREATE_NEW_CONSOLE);
            }
            let mut child = BoundedChild(command.spawn().unwrap());
            fs::write(root.join("cli.pid"), child.0.id().to_string()).unwrap();
            wait_until("managed tree ready", || root.join("leader.ready").exists());
            let processes = tree.observe();
            // Independently observe the continuous output before cancellation.
            wait_until("continuous stdout", || {
                fs::metadata(&stdout).unwrap().len() >= 8192
            });
            #[cfg(unix)]
            {
                use rustix::process::{Pid, Signal, kill_process};
                kill_process(Pid::from_raw(child.0.id() as i32).unwrap(), Signal::INT).unwrap();
            }
            #[cfg(windows)]
            let mut controller = BoundedChild(
                Command::new(native::helper())
                    .args([
                        "ctrl-c",
                        &child.0.id().to_string(),
                        root.join("cancel-observed").to_str().unwrap(),
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap(),
            );
            #[cfg(windows)]
            fs::write(root.join("controller.pid"), controller.0.id().to_string()).unwrap();
            assert_eq!(child.wait().code(), Some(1));
            for process in processes {
                process.assert_stopped();
            }
            #[cfg(windows)]
            {
                fs::write(root.join("cancel-observed"), b"finished").unwrap();
                assert!(controller.wait().success());
            }
            let diagnostic = fs::read_to_string(&stderr).unwrap();
            assert!(diagnostic.contains("check interrupted"), "{diagnostic}");
            assert!(!root.join("later.log").exists());
            assert_eq!(repo.success(&["rev-parse", "HEAD"]).stdout, head);
            assert!(
                !twigsmith::git::run(remote.path(), ["show-ref", "--verify", "refs/heads/work"])
                    .unwrap()
                    .status
                    .success()
            );
            assert!(
                repo.success(&["status", "--porcelain=v1"])
                    .stdout
                    .is_empty()
            );
            configure(native::command("exit", &["0"]));
            let retry = native::output(
                Command::new(env!("CARGO_BIN_EXE_tgi"))
                    .current_dir(repo.path())
                    .arg("finish"),
            );
            assert!(retry.status.success(), "{retry:?}");
            assert_eq!(remote.success(&["rev-parse", "work"]).stdout, head);
            assert!(root.join("later.log").exists());
            println!(
                "native interrupt stopped continuous output and all descendants; publication blocked; preserved work delivered on retry"
            );
        },
    );
}

// An actual signal in the same process also checks persistent registration state:
// a successful second Cancellation must not inherit the first cancelled flag.
#[test]
fn native_signal_registration_resets_between_stages() {
    isolated(
        "native_signal_registration_resets_between_stages",
        true,
        |root| {
            let _deadline = Deadline::new(root);
            #[cfg(windows)]
            unsafe {
                use windows_sys::Win32::System::Console::{AllocConsole, FreeConsole};
                // This isolated test gets a private console before any managed
                // stage; its broadcast cannot reach Cargo or other tests.
                FreeConsole();
                assert_ne!(AllocConsole(), 0, "{}", std::io::Error::last_os_error());
            }
            let _tree = Tree {
                root: root.to_owned(),
            };
            let observer_root = root.to_owned();
            let signal = thread::spawn(move || {
                wait_until("library tree ready", || {
                    observer_root.join("leader.ready").exists()
                });
                let processes = Tree::observe_at(&observer_root);
                #[cfg(unix)]
                {
                    use rustix::process::{Signal, getpid, kill_process};
                    kill_process(getpid(), Signal::TERM).unwrap();
                }
                #[cfg(windows)]
                unsafe {
                    use windows_sys::Win32::System::Console::{
                        CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent,
                    };
                    assert_ne!(GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0), 0);
                }
                processes
            });
            let cancellation = Cancellation::new().unwrap();
            let result = check_process::run(
                root,
                &native::command("tree", &[root.to_str().unwrap()]),
                &cancellation,
                |_, _| {},
            );
            let processes = signal.join().unwrap();
            assert!(matches!(result, Err(Error::Interrupted)), "{result:?}");
            assert!(cancellation.complete().is_err());
            for process in processes {
                process.assert_stopped();
            }
            let second = Cancellation::new().unwrap();
            assert!(
                check_process::run(root, &native::command("exit", &["0"]), &second, |_, _| {})
                    .unwrap()
                    .success()
            );
            second.complete().unwrap();
        },
    );
}
