//! Windows host input and native argv/launch boundaries. Required runtime tests
//! are selected explicitly by native CI; absence is a failure, never a skip.
#![cfg(windows)]

#[path = "support/snapshot.rs"]
mod preservation;
mod support;
#[path = "support/native_terminal.rs"]
#[allow(dead_code)]
mod terminal;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::{Repository, isolated};
use terminal::NativeTerminal;

const LITERAL: &str = "literal/β;$'&()work";

fn runtime(name: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(format!("TWIGSMITH_TEST_{}", name.to_uppercase())) {
        return path.into();
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path| path.join(format!("{name}.exe")))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("required native {name} runtime is unavailable"))
}

fn quote_ps(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "''"))
}

fn binaries(root: &Path) -> PathBuf {
    let bin = root.join("actual binaries with spaces");
    fs::create_dir(&bin).unwrap();
    for (name, source) in [
        ("twigsmith", env!("CARGO_BIN_EXE_twigsmith")),
        ("tgi", env!("CARGO_BIN_EXE_tgi")),
    ] {
        fs::copy(source, bin.join(format!("{name}.exe"))).unwrap();
    }
    bin
}

fn with_path(bin: &Path) -> std::ffi::OsString {
    std::env::join_paths(std::iter::once(bin.to_owned()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap()
}

fn fixture(root: &Path) -> Repository {
    let repo = Repository::new(root);
    fs::write(repo.path().join("tracked file"), "base\n").unwrap();
    fs::write(repo.path().join("unrelated file"), "preserve me\n").unwrap();
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", "base"]);
    repo.success(&["branch", LITERAL]);
    repo.success(&["branch", "ps$(ni(SENTINEL))"]);
    fs::create_dir(repo.path().join("nested directory")).unwrap();
    repo
}

fn captured(term: &mut NativeTerminal, capture: &Path, line: &str, expected: &[&str]) {
    let _ = fs::remove_file(capture);
    term.output.clear();
    term.send(format!("{line}\r").as_bytes());
    term.until("NATIVE_ARGUMENTS_RECORDED");
    term.until("NATIVE_SHELL_READY> ");
    let actual: Vec<String> = serde_json::from_slice(&fs::read(capture).unwrap()).unwrap();
    assert_eq!(actual, expected, "{}", term.output);
}

#[test]
#[ignore = "requires native PowerShell 7.6 and ConPTY; selected by native CI"]
fn windows_powershell_actual_tab_and_native_arguments() {
    isolated(
        "windows_powershell_actual_tab_and_native_arguments",
        true,
        |root| {
            let pwsh = runtime("pwsh");
            let version = Command::new(&pwsh).args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "if ($PSVersionTable.PSVersion.Major -ne 7 -or $PSVersionTable.PSVersion.Minor -ne 6) { exit 1 }; $PSVersionTable.PSVersion.ToString()"]).output().unwrap();
            assert!(
                version.status.success(),
                "required maintained PowerShell 7.6: {version:?}"
            );
            println!(
                "Windows actual Tab: ConPTY; {}: {}",
                pwsh.display(),
                String::from_utf8_lossy(&version.stdout)
            );
            let actual = binaries(root);
            let capture_bin = root.join("argv recorder with spaces");
            fs::create_dir(&capture_bin).unwrap();
            let recorder = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("examples/native_argument_fixture.exe");
            for name in ["twigsmith", "tgi"] {
                fs::copy(&recorder, capture_bin.join(format!("{name}.exe")))
                    .expect("build native_argument_fixture before native shell tests");
            }
            let adapter = root.join("session.ps1");
            let output = Command::new(actual.join("twigsmith.exe"))
                .args(["completion", "powershell"])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            fs::write(&adapter, output.stdout).unwrap();
            let init = root.join("interactive.ps1");
            fs::write(&init, format!("$ErrorActionPreference = 'Stop'\nImport-Module PSReadLine\nSet-PSReadLineOption -HistorySaveStyle SaveNothing\n. {}\n. {}\nfunction global:prompt {{ 'NATIVE_SHELL_READY> ' }}\n", quote_ps(&adapter), quote_ps(&adapter))).unwrap();
            let capture = root.join("arguments.json");
            let repo = fixture(root);
            let mut command = Command::new(&pwsh);
            command
                .args(["-NoLogo", "-NoProfile", "-NoExit", "-File"])
                .arg(&init)
                .current_dir(root)
                .env("PATH", with_path(&capture_bin))
                .env("TWIGSMITH_ARGUMENT_BACKEND", actual.join("twigsmith.exe"))
                .env("TWIGSMITH_ARGUMENT_CAPTURE", &capture)
                .env("TERM", "xterm-256color");
            let mut term = NativeTerminal::start_command_width(command, 180);
            term.until("NATIVE_SHELL_READY> ");
            captured(&mut term, &capture, "tgi stat\t", &["status"]);
            captured(
                &mut term,
                &capture,
                "twigsmith start --he\t",
                &["start", "--help"],
            );
            term.output.clear();
            term.send(
                format!(
                    "Set-Location -LiteralPath {}\r",
                    quote_ps(&repo.path().join("nested directory"))
                )
                .as_bytes(),
            );
            term.until("NATIVE_SHELL_READY> ");
            let before = preservation::snapshot(repo.path());
            captured(&mut term, &capture, "tgi stat\t", &["status"]);
            assert_eq!(
                preservation::snapshot(repo.path()),
                before,
                "completion/status wrote repository state"
            );
            // Observe Tab independently of the eventual command execution.
            // Only the completed candidate contains "work", so echo of the
            // typed prefix cannot satisfy this synchronization point.
            let before_tab = preservation::snapshot(repo.path());
            term.output.clear();
            term.send(b"twigsmith start literal/\t");
            term.until("work");
            assert_eq!(preservation::snapshot(repo.path()), before_tab);
            term.output.clear();
            term.send(b"\x03");
            term.until("NATIVE_SHELL_READY> ");
            for name in ["twigsmith", "tgi"] {
                repo.success(&["switch", "main"]);
                let old_tree = repo.success(&["write-tree"]).stdout;
                captured(
                    &mut term,
                    &capture,
                    &format!("{name} start literal/\t"),
                    &["start", LITERAL],
                );
                assert_eq!(
                    repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                    format!("{LITERAL}\n").as_bytes()
                );
                assert_eq!(repo.success(&["write-tree"]).stdout, old_tree);
                assert_eq!(
                    fs::read(repo.path().join("unrelated file")).unwrap(),
                    b"preserve me\n"
                );
            }
            // A later query must see a newly created ref, without restarting the shell.
            repo.success(&["branch", "later/新;$'work"]);
            captured(
                &mut term,
                &capture,
                "tgi start later/\t",
                &["start", "later/新;$'work"],
            );
            captured(
                &mut term,
                &capture,
                "tgi start ps\t",
                &["start", "ps$(ni(SENTINEL))"],
            );
            assert_eq!(
                repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                b"ps$(ni(SENTINEL))\n"
            );
            fs::write(
                repo.path().join("nested directory/irrelevant"),
                "no filename fallback",
            )
            .unwrap();
            let before_empty = preservation::snapshot(repo.path());
            captured(
                &mut term,
                &capture,
                "twigsmith start --from-tag irr\t",
                &["start", "--from-tag", "irr"],
            );
            assert_eq!(preservation::snapshot(repo.path()), before_empty);
            assert!(!repo.path().join("SENTINEL").exists());
            assert!(!repo.path().join("nested directory/SENTINEL").exists());
            // A free-form token can contain spaces/double quotes even though
            // a Windows ref filename cannot. The incomplete start command
            // records argv and then fails CLI parsing without Git effects.
            for mode in ["Standard", "Windows", "Legacy"] {
                term.output.clear();
                term.send(
                    format!("$global:PSNativeCommandArgumentPassing = '{mode}'\r").as_bytes(),
                );
                term.until("NATIVE_SHELL_READY> ");
                let expected = if mode == "Legacy" {
                    "space quoted value"
                } else {
                    "space \"quoted\" value"
                };
                captured(
                    &mut term,
                    &capture,
                    "tgi start --from-tag 'space \"quoted\" value'",
                    &["start", "--from-tag", expected],
                );
            }
            term.send(b"exit 0\r");
            assert!(term.finish().success(), "{}", term.output);
        },
    );
}

fn shell_script(root: &Path, bin: &Path, repo: &Repository, shell: &str, text: &str) {
    let extension = match shell {
        "cmd" => "cmd",
        "pwsh" => "ps1",
        "bash" => "bash",
        _ => unreachable!(),
    };
    let script = root.join(format!("launch.{extension}"));
    fs::write(&script, text).unwrap();
    let runtime = runtime(shell);
    let mut command = Command::new(&runtime);
    match shell {
        "cmd" => {
            command.args(["/d", "/c"]).arg(&script);
        }
        "bash" => {
            command.args(["--noprofile", "--norc"]).arg(&script);
        }
        "pwsh" => {
            command
                .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
                .arg(&script);
        }
        _ => unreachable!(),
    }
    let output = command
        .env("PATH", with_path(bin))
        .env("TWIGSMITH_LAUNCH_BIN", bin)
        .current_dir(repo.path())
        .output()
        .unwrap();
    println!("native launch via {}: {output:?}", runtime.display());
    assert!(output.status.success(), "{output:?}");
}

fn launch(root: &Path, shell: &str) {
    let bin = binaries(root);
    let repo = fixture(root);
    let original_tree = repo.success(&["write-tree"]).stdout;
    let base = repo.success(&["rev-parse", "HEAD"]).stdout;
    for name in ["twigsmith", "tgi"] {
        for explicit in [false, true] {
            let invocation = match (shell, explicit) {
                ("cmd", false) | ("bash", false) => name.to_owned(),
                ("cmd", true) => format!("\"{}\"", bin.join(format!("{name}.exe")).display()),
                // MSYS understands its inherited Windows path; quoting the
                // expansion preserves spaces and punctuation without eval.
                ("bash", true) => format!("\"$TWIGSMITH_LAUNCH_BIN/{name}.exe\""),
                ("pwsh", false) => format!("& {name}"),
                ("pwsh", true) => format!("& {}", quote_ps(&bin.join(format!("{name}.exe")))),
                _ => unreachable!(),
            };
            let script = |arguments: &str| match shell {
                "cmd" => format!(
                    "@echo off\r\nchcp 65001 >nul\r\nif errorlevel 1 exit /b 1\r\n{invocation} {arguments}\r\nif errorlevel 1 exit /b 1\r\nexit /b 0\r\n"
                ),
                "bash" => format!("set -e\n{invocation} {arguments}\n"),
                "pwsh" => format!(
                    "$ErrorActionPreference = 'Stop'\n{invocation} {arguments}\nexit $LASTEXITCODE\n"
                ),
                _ => unreachable!(),
            };
            let before = preservation::snapshot(repo.path());
            shell_script(root, &bin, &repo, shell, &script("status"));
            assert_eq!(
                preservation::snapshot(repo.path()),
                before,
                "shell status launch changed repository"
            );
            let branch = format!(
                "launch/{name}-{}-β;$'&()work",
                if explicit { "path" } else { "lookup" }
            );
            // Resume an exact local branch offline. The test does not need a
            // transport provider to establish the shell's literal argv seam.
            repo.success(&["branch", &branch]);
            let quoted = match shell {
                "cmd" => format!("\"{branch}\""),
                "bash" => format!("'{}'", branch.replace('\'', "'\\''")),
                "pwsh" => format!("'{}'", branch.replace('\'', "''")),
                _ => unreachable!(),
            };
            shell_script(
                root,
                &bin,
                &repo,
                shell,
                &script(&format!("start {quoted}")),
            );
            assert_eq!(
                repo.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                format!("{branch}\n").as_bytes()
            );
            assert_eq!(repo.success(&["rev-parse", "HEAD"]).stdout, base);
            assert_eq!(repo.success(&["write-tree"]).stdout, original_tree);
            assert_eq!(
                fs::read(repo.path().join("tracked file")).unwrap(),
                b"base\n"
            );
            assert_eq!(
                fs::read(repo.path().join("unrelated file")).unwrap(),
                b"preserve me\n"
            );
        }
    }
}

#[test]
#[ignore = "requires cmd.exe; selected by native CI"]
fn windows_cmd_both_binary_launch_routes() {
    isolated("windows_cmd_both_binary_launch_routes", true, |root| {
        launch(root, "cmd")
    });
}

#[test]
#[ignore = "requires Git-for-Windows Bash; selected by native CI"]
fn windows_bash_both_binary_launch_routes() {
    isolated("windows_bash_both_binary_launch_routes", true, |root| {
        launch(root, "bash")
    });
}

#[test]
#[ignore = "requires PowerShell 7.6; selected by native CI"]
fn windows_powershell_both_binary_launch_routes() {
    isolated(
        "windows_powershell_both_binary_launch_routes",
        true,
        |root| launch(root, "pwsh"),
    );
}
