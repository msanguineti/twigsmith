//! Shell-free process fixture for native application qualification. Not shipped.
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn ready(root: &Path, name: &str) {
    fs::write(
        root.join(format!("{name}.pid")),
        std::process::id().to_string(),
    )
    .unwrap();
    fs::write(root.join(format!("{name}.ready")), "ready").unwrap();
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "no readiness: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn ignore_interruption() {
    #[cfg(unix)]
    {
        use std::sync::{Arc, atomic::AtomicBool};
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            signal_hook::flag::register(signal, Arc::new(AtomicBool::new(false))).unwrap();
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        unsafe extern "system" fn ignore(_: u32) -> i32 {
            1
        }
        assert_ne!(SetConsoleCtrlHandler(Some(ignore), 1), 0);
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    // Git's GPG signer protocol cannot supply an application-specific mode.
    // Keep this deliberately failing signer distinct from ordinary fixture modes.
    if args
        .first()
        .is_some_and(|arg| arg.starts_with("--status-fd"))
    {
        eprintln!("native-fixture-signing-refused");
        std::process::exit(23);
    }
    match args[0].as_str() {
        "exit" => std::process::exit(args[1].parse().unwrap()),
        "record" => {
            ready(Path::new(&args[1]).parent().unwrap(), "record");
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).unwrap();
            let observation = serde_json::json!({
                "args": &args[2..],
                "cwd": std::env::current_dir().unwrap(),
                "environment": std::env::var("NATIVE_CHECK_VALUE").unwrap_or_else(|_| std::env::var("HOME").unwrap()),
                "stdin": input,
            });
            fs::write(&args[1], serde_json::to_vec(&observation).unwrap()).unwrap();
            println!("recorded stdout");
            eprintln!("recorded stderr");
        }
        "ordered" => {
            let head = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            assert!(head.status.success());
            let mut log = OpenOptions::new()
                .append(true)
                .create(true)
                .open(&args[1])
                .unwrap();
            writeln!(
                log,
                "{} {}",
                args[2],
                String::from_utf8(head.stdout).unwrap().trim()
            )
            .unwrap();
            if args
                .get(3)
                .is_some_and(|permit| !Path::new(permit).exists())
            {
                std::process::exit(23);
            }
        }
        "branch-gate" => {
            let git = |args: &[&str]| {
                let output = Command::new("git").args(args).output().unwrap();
                assert!(output.status.success());
                String::from_utf8(output.stdout).unwrap().trim().to_owned()
            };
            let branch = git(&["symbolic-ref", "--short", "HEAD"]);
            let head = git(&["rev-parse", "HEAD"]);
            let mut log = OpenOptions::new()
                .append(true)
                .create(true)
                .open(&args[1])
                .unwrap();
            writeln!(log, "{branch} {head}").unwrap();
            if !Path::new(&args[2]).join(branch).is_file() {
                std::process::exit(23);
            }
        }
        "stream" => {
            let root = Path::new(&args[1]);
            ready(root, "stream");
            // A reader must observe these bytes and open the gate while this
            // process is still alive. Output collected only at exit deadlocks.
            println!("before-exit stdout");
            io::stdout().flush().unwrap();
            eprintln!("before-exit stderr");
            io::stderr().flush().unwrap();
            wait_for(&root.join("output-observed"));
            let out = thread::spawn(|| {
                let mut out = io::stdout().lock();
                for i in 0..8192 {
                    writeln!(out, "out-{i:05}:{}", "o".repeat(128)).unwrap();
                }
            });
            let err = thread::spawn(|| {
                let mut err = io::stderr().lock();
                for i in 0..8192 {
                    writeln!(err, "err-{i:05}:{}", "e".repeat(128)).unwrap();
                }
            });
            out.join().unwrap();
            err.join().unwrap();
        }
        "tree" => {
            let root = Path::new(&args[1]);
            for name in ["ordinary", "stubborn-pipe-holder"] {
                // Intentionally inherit both pipes. The managed executor must
                // contain these descendants even after this leader exits.
                // The enclosing native harness owns termination and independently
                // observes these children. Waiting here would erase the required
                // leader-exits-first/pipe-holder condition.
                #[allow(clippy::zombie_processes)]
                let _descendant = Command::new(std::env::current_exe().unwrap())
                    .args(["hold", &args[1], name])
                    .stdin(Stdio::null())
                    .spawn()
                    .unwrap();
                wait_for(&root.join(format!("{name}.ready")));
            }
            ready(root, "leader");
            if args.get(2).is_some_and(|mode| mode == "exit") {
                wait_for(&root.join("leader-may-exit"));
                return;
            }
            let mut output = io::stdout().lock();
            let bytes = vec![b'x'; 8192];
            loop {
                output.write_all(&bytes).unwrap();
            }
        }
        "hold" => {
            let root = Path::new(&args[1]);
            if args[2] == "stubborn-pipe-holder" {
                ignore_interruption();
            }
            ready(root, &args[2]);
            loop {
                fs::write(root.join(format!("{}.heartbeat", args[2])), b"alive").unwrap();
                thread::sleep(Duration::from_millis(20));
            }
        }
        #[cfg(windows)]
        "ctrl-c" => {
            use windows_sys::Win32::System::Console::{
                AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent,
                SetConsoleCtrlHandler,
            };
            let pid = args[1].parse().unwrap();
            // Ctrl-C broadcasts to the shared console; a nonzero process-group
            // argument cannot deliver Ctrl-C. Attach also resets handler state.
            // https://learn.microsoft.com/windows/console/generateconsolectrlevent
            unsafe {
                FreeConsole();
                assert_ne!(AttachConsole(pid), 0, "{}", io::Error::last_os_error());
                assert_ne!(SetConsoleCtrlHandler(None, 1), 0);
                assert_ne!(GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0), 0);
            }
            // The target acknowledges cancellation by finishing. Stay attached
            // until then, without ending its console via a controller timeout.
            wait_for(Path::new(&args[2]));
            unsafe {
                FreeConsole();
            }
        }
        other => panic!("unknown fixture mode {other}"),
    }
}
