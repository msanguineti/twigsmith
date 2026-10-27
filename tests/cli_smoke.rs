use std::{path::Path, process::Command};

#[test]
fn cli_runs_outside_git_without_node_or_rust_tools() {
    // The development entry point currently targets Linux. The filesystem root
    // avoids Git's discovery of the repository containing the build directory.
    assert!(
        !Path::new("/.git").exists(),
        "smoke checks require / outside Git"
    );

    for executable in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
        for args in [
            vec![],
            vec!["--help"],
            vec!["-h"],
            vec!["--version"],
            vec!["-V"],
        ] {
            let output = Command::new(executable)
                .args(&args)
                .current_dir("/")
                .env_clear()
                .env("PATH", "")
                .output()
                .expect("native executable should run with an empty PATH");

            assert!(output.status.success(), "arguments: {args:?}");
            assert!(output.stderr.is_empty());
            let stdout = String::from_utf8(output.stdout).expect("UTF-8 CLI output");
            if args == ["--version"] || args == ["-V"] {
                assert_eq!(
                    stdout,
                    concat!("git-oneflow ", env!("CARGO_PKG_VERSION"), "\n")
                );
            } else {
                assert!(stdout.contains("Usage: git-oneflow [OPTIONS]"));
                assert!(stdout.contains("--help"));
                assert!(stdout.contains("--version"));
                assert!(stdout.contains("Commands:"));
                assert!(stdout.contains("Options:"));
                assert!(stdout.contains("Examples:"));
            }
        }

        for command in ["start", "save", "finish", "status", "config", "completion"] {
            let output = Command::new(executable)
                .args([command, "--help"])
                .current_dir("/")
                .env_clear()
                .env("PATH", "")
                .output()
                .expect("command help should run without Git");
            assert!(output.status.success(), "command: {command}");
            assert!(output.stderr.is_empty());
            let help = String::from_utf8(output.stdout).unwrap();
            for section in ["Usage:", "Options:", "Examples:", "Exit status:"] {
                assert!(help.contains(section), "{command}: missing {section}");
            }
            assert!(
                !help.contains("docs/"),
                "installed help must be self-contained: {command}"
            );
        }

        for args in [["start"].as_slice(), ["--help", "start"].as_slice()] {
            let output = Command::new(executable)
                .args(args)
                .current_dir("/")
                .env_clear()
                .env("PATH", "")
                .output()
                .expect("native executable should run with an empty PATH");
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("use --help"));
        }
    }
}

#[cfg(target_os = "linux")]
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;

#[cfg(target_os = "linux")]
#[test]
fn ordinary_errors_use_red_headings_and_preserve_plain_diagnostics_and_exit_status() {
    support::isolated(
        "ordinary_errors_use_red_headings_and_preserve_plain_diagnostics_and_exit_status",
        true,
        |root| {
            let executable = env!("CARGO_BIN_EXE_git-oneflow");
            let repo = support::Repository::new(root);
            for preference in [
                None,
                Some(("NO_COLOR", "1")),
                Some(("CLICOLOR", "0")),
                Some(("TERM", "dumb")),
            ] {
                for (args, code) in [
                    (vec!["unknown"], 2),
                    (vec!["start", "HEAD"], 1),
                    (vec!["save", "--publish-only", "--commit", "all"], 2),
                    (vec!["config", "show"], 1),
                ] {
                    let path = repo.path().join("git-oneflow.config.json");
                    if args[0] == "config" {
                        std::fs::write(&path, "invalid JSON").unwrap();
                    }
                    let mut command = Command::new(executable);
                    command
                        .current_dir(repo.path())
                        .args(&args)
                        .env("TERM", "xterm-256color");
                    if let Some((name, value)) = preference {
                        command.env(name, value);
                    }
                    let mut tty = terminal::Terminal::start_command(command);
                    assert_eq!(tty.finish().code(), Some(code), "{}", tty.output);
                    tty.assert_restored();
                    if preference.is_none() {
                        assert!(
                            tty.output.contains("\x1b[31merror:"),
                            "error heading is not red: {}",
                            tty.output
                        );
                    } else {
                        assert!(tty.output.contains("error:"), "{}", tty.output);
                        assert!(
                            !tty.output.contains('\x1b'),
                            "disabled styling: {}",
                            tty.output
                        );
                    }
                    let output = Command::new(executable)
                        .current_dir(repo.path())
                        .args(&args)
                        .env("TERM", "xterm-256color")
                        .env("CLICOLOR_FORCE", "1")
                        .output()
                        .unwrap();
                    assert_eq!(output.status.code(), Some(code));
                    assert!(output.stdout.is_empty());
                    let plain = String::from_utf8(output.stderr).unwrap();
                    assert!(plain.starts_with("error:"), "{plain}");
                    assert!(!plain.contains('\x1b'), "{plain}");
                    let terminal_plain = tty
                        .output
                        .replace("\x1b[31m", "")
                        .replace("\x1b[0m", "")
                        .replace("\r\n", "\n");
                    assert_eq!(terminal_plain, plain, "styling changed diagnostic meaning");
                    if path.exists() {
                        std::fs::remove_file(path).unwrap();
                    }
                }
            }
            assert!(repo.success(&["status", "--porcelain"]).stdout.is_empty());
        },
    );
}
