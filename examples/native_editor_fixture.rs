//! Foreground terminal editor used by the native CLI journeys. Not shipped.
use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::Path,
    process::Command,
};

fn git(args: &[&str]) {
    let status = Command::new("git").args(args).status().unwrap();
    assert!(status.success(), "fixture Git failed: {args:?}");
}

fn assert_terminal() {
    assert!(io::stdin().is_terminal(), "editor stdin is not a terminal");
    assert!(
        io::stdout().is_terminal(),
        "editor stdout is not a terminal"
    );
    assert!(
        io::stderr().is_terminal(),
        "editor stderr is not a terminal"
    );
}

fn main() {
    assert_terminal();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--redirect-stdout") {
        let status = Command::new(args.get(2).expect("child executable"))
            .args(&args[3..])
            .stdout(fs::File::create(args.get(1).expect("stdout filename")).unwrap())
            .status()
            .unwrap();
        std::process::exit(status.code().unwrap_or(1));
    }
    if args.first().is_some_and(|arg| arg == "--terminal-probe") {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::Console::{
                ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, GetConsoleMode, GetStdHandle,
                STD_INPUT_HANDLE,
            };
            let mut mode = 0;
            assert_ne!(GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode), 0);
            assert_eq!(
                mode & (ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT),
                ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT,
                "console left without line input/echo: {mode:x}"
            );
        }
        println!("TERMINAL_PROBE_READY");
        io::stdout().flush().unwrap();
        let mut line = String::new();
        assert_ne!(io::stdin().read_line(&mut line).unwrap(), 0);
        assert_eq!(line.trim_end_matches(['\r', '\n']), "line-echo-restored");
        println!("TERMINAL_RESTORED");
        return;
    }
    let path = Path::new(args.last().expect("message filename"));
    let suggested = fs::read(path).unwrap();
    if let Some(destination) = std::env::var_os("TWIGSMITH_EDITOR_SUGGESTION_FILE") {
        fs::write(destination, &suggested).unwrap();
    }
    if let Some(destination) = std::env::var_os("TWIGSMITH_EDITOR_TRACE_FILE") {
        let trace = serde_json::json!({
            "args": args.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>(),
            "cwd": std::env::current_dir().unwrap(),
            "git_pager": std::env::var_os("GIT_PAGER").map(|v| v.to_string_lossy().into_owned()),
            "stdin_terminal": true, "stdout_terminal": true, "stderr_terminal": true,
        });
        fs::write(destination, serde_json::to_vec(&trace).unwrap()).unwrap();
    }
    println!("EDITOR_READY");
    io::stdout().flush().unwrap();
    eprintln!("EDITOR_STDERR_READY");
    let mut line = String::new();
    assert_ne!(
        io::stdin().read_line(&mut line).unwrap(),
        0,
        "editor input EOF"
    );
    match line.trim() {
        "accept" => (),
        "cancel" => std::process::exit(23),
        other => panic!("unexpected editor input {other:?}"),
    }
    let action = std::env::var("TWIGSMITH_EDITOR_ACTION").unwrap_or_else(|_| "accept".into());
    match action.as_str() {
        "cancel" => std::process::exit(23),
        "missing" => fs::remove_file(path).unwrap(),
        "directory" => {
            fs::remove_file(path).unwrap();
            fs::create_dir(path).unwrap();
        }
        "blank" => fs::write(path, b" \n\n").unwrap(),
        "nul" => fs::write(path, b"invalid\0message\n").unwrap(),
        "invalid-utf8" => fs::write(path, b"invalid\xffmessage\n").unwrap(),
        "accept" | "mutate" => {
            let content = std::env::var_os("TWIGSMITH_EDITOR_CONTENT_FILE")
                .map(|source| fs::read(source).unwrap())
                .unwrap_or(suggested);
            if std::env::var_os("TWIGSMITH_EDITOR_ATOMIC").is_some() {
                let replacement = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
                fs::write(replacement.path(), content).unwrap();
                replacement.persist(path).unwrap();
            } else {
                fs::write(path, content).unwrap();
            }
            if action == "mutate" {
                match std::env::var("TWIGSMITH_EDITOR_MUTATION").unwrap().as_str() {
                    "head" => git(&["commit", "--allow-empty", "-m", "editor external commit"]),
                    "index" => {
                        fs::write("editor-external-file", b"editor external work\n").unwrap();
                        git(&["add", "--", "editor-external-file"]);
                    }
                    "settings" => git(&[
                        "config",
                        "remote.fixture.receivepack",
                        "editor-external-value",
                    ]),
                    other => panic!("unknown mutation {other:?}"),
                }
            }
        }
        other => panic!("unknown editor action {other:?}"),
    }
    println!("EDITOR_ACCEPTED");
}
