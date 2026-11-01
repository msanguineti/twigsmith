//! The native fixture is an example rather than a shipped application binary.
use std::path::PathBuf;

/// Drain both streams while waiting, with a deadline independent of the product
/// executor. A regressed process runner must fail qualification without hanging CI.
#[allow(dead_code)]
pub fn output(command: &mut std::process::Command) -> std::process::Output {
    use std::{
        io::Read,
        process::Stdio,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (send, receive) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    for (is_stdout, mut stream) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let send = send.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream.read_to_end(&mut bytes).map(|_| bytes);
            let _ = send.send((is_stdout, result));
        });
    }
    drop(send);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native CLI exceeded 20 seconds: {command:?}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    for _ in 0..2 {
        let (is_stdout, bytes) = receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("native CLI output handles remained open after exit");
        if is_stdout {
            stdout = bytes.unwrap();
        } else {
            stderr = bytes.unwrap();
        }
    }
    std::process::Output {
        status,
        stdout,
        stderr,
    }
}

pub fn helper() -> PathBuf {
    let path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!(
            "native_check_fixture{}",
            std::env::consts::EXE_SUFFIX
        ));
    assert!(
        path.is_file(),
        "build the fixture with cargo build --locked --example native_check_fixture: {}",
        path.display()
    );
    path
}

#[allow(dead_code)]
pub fn command(mode: &str, args: &[&str]) -> Vec<String> {
    std::iter::once(helper().to_str().unwrap().to_owned())
        .chain(std::iter::once(mode.to_owned()))
        .chain(args.iter().map(|arg| (*arg).to_owned()))
        .collect()
}
