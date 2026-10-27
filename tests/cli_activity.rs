#![cfg(unix)]
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use support::{Repository, isolated};

struct SlowGit {
    wrapper: PathBuf,
    real: PathBuf,
    marker: PathBuf,
}

impl SlowGit {
    fn new(root: &Path) -> Self {
        let real = env::split_paths(&env::var_os("PATH").unwrap())
            .map(|p| p.join("git"))
            .find(|p| p.is_file())
            .unwrap()
            .canonicalize()
            .unwrap();
        let wrapper = root.join("git-wrapper");
        fs::create_dir(&wrapper).unwrap();
        let path = wrapper.join("git");
        fs::write(
            &path,
            r#"#!/bin/sh
set -eu
for argument in "$@"; do
    if [ "$argument" = "$ONEFLOW_ACTIVITY_COMMAND" ] && [ ! -e "$ONEFLOW_ACTIVITY_MARKER" ]; then
        : > "$ONEFLOW_ACTIVITY_MARKER"
        sleep 1
        if [ "$ONEFLOW_ACTIVITY_FAILURE" = 1 ]; then
            echo 'controlled Git failure' >&2
            exit 42
        fi
        if [ "$ONEFLOW_ACTIVITY_FAILURE" = signal ]; then
            kill -TERM "$$"
        fi
        break
    fi
done
exec "$ONEFLOW_REAL_GIT" "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            wrapper,
            real,
            marker: root.join("slow-command-started"),
        }
    }

    fn wrap(&self, command: &mut Command, operation: &str, failure: bool) {
        let _ = fs::remove_file(&self.marker);
        let mut paths = vec![self.wrapper.clone()];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap()));
        command
            .env("PATH", env::join_paths(paths).unwrap())
            .env("ONEFLOW_REAL_GIT", &self.real)
            .env("ONEFLOW_ACTIVITY_COMMAND", operation)
            .env("ONEFLOW_ACTIVITY_MARKER", &self.marker)
            .env("ONEFLOW_ACTIVITY_FAILURE", if failure { "1" } else { "0" });
    }

    fn cli(
        &self,
        repository: &Repository,
        args: &[&str],
        operation: &str,
        failure: bool,
    ) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_git-oneflow"));
        command
            .current_dir(repository.path())
            .args(args)
            .stdin(Stdio::null());
        self.wrap(&mut command, operation, failure);
        command
    }
}

fn fixture(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    fs::write(remote.path().join("base"), "base\n").unwrap();
    remote.success(&["add", "base"]);
    remote.success(&["commit", "-m", "base"]);
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin"]);
    local.success(&["switch", "--no-track", "-c", "main", "origin/main"]);
    (local, remote)
}

fn oid(repository: &Repository, reference: &str) -> Vec<u8> {
    repository.success(&["rev-parse", reference]).stdout
}

#[test]
fn redirected_slow_remote_success_and_failure_have_one_plain_transition() {
    isolated(
        "redirected_slow_remote_success_and_failure_have_one_plain_transition",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let slow = SlowGit::new(root);
            let output = slow
                .cli(&local, &["start", "work/日本語"], "ls-remote", false)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                "Inspecting remote references...\nGit remote inspection completed.\n"
            );
            assert_eq!(oid(&local, "HEAD"), oid(&remote, "main"));
            assert_eq!(
                local.success(&["branch", "--show-current"]).stdout,
                "work/日本語\n".as_bytes()
            );

            local.success(&["switch", "main"]);
            let output = slow
                .cli(&local, &["start", "work/refused"], "ls-remote", true)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.starts_with("Inspecting remote references...\nGit remote inspection exited with status 42.\n"), "{stderr}");
            assert!(stderr.contains("controlled Git failure"), "{stderr}");
            assert!(!stderr.contains('\x1b'), "{stderr}");
            assert_eq!(stderr.matches("Inspecting remote references...").count(), 1);
            assert_eq!(
                local.success(&["branch", "--show-current"]).stdout,
                b"main\n"
            );
            assert!(
                local
                    .success(&["for-each-ref", "refs/heads/work/refused"])
                    .stdout
                    .is_empty()
            );
        },
    );
}

#[test]
fn fast_cli_and_slow_library_inspection_stay_quiet() {
    isolated(
        "fast_cli_and_slow_library_inspection_stay_quiet",
        true,
        |root| {
            if let Some(path) = env::var_os("ONEFLOW_ACTIVITY_LIBRARY_REPO") {
                assert!(matches!(
                    git_oneflow::repository::inspect(Path::new(&path)).unwrap(),
                    git_oneflow::repository::Observation::Repository(_)
                ));
                return;
            }
            let (local, _remote) = fixture(root);
            let fast = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
                .current_dir(local.path())
                .arg("status")
                .output()
                .unwrap();
            assert!(fast.status.success(), "{fast:?}");
            assert!(fast.stderr.is_empty(), "{fast:?}");

            let slow = SlowGit::new(root);
            let mut command = Command::new(env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "fast_cli_and_slow_library_inspection_stay_quiet",
                    "--nocapture",
                ])
                .env("ONEFLOW_ACTIVITY_LIBRARY_REPO", local.path());
            slow.wrap(&mut command, "rev-parse", false);
            let output = command.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(slow.marker.exists());
            assert!(output.stderr.is_empty(), "{output:?}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("Inspecting repository"));
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn terminal_slow_push_clears_activity_and_reports_process_outcome() {
    isolated(
        "terminal_slow_push_clears_activity_and_reports_process_outcome",
        true,
        |root| {
            let (local, remote) = fixture(root);
            local.success(&["switch", "--no-track", "-c", "work/activity"]);
            let slow = SlowGit::new(root);
            for mode in ["success", "failure", "signal"] {
                let failure = mode != "success";
                if failure {
                    fs::write(local.path().join("next"), format!("{mode}\n")).unwrap();
                    local.success(&["add", "next"]);
                    local.success(&["commit", "-m", "next"]);
                }
                let before = oid(&remote, "main");
                let mut command = slow.cli(&local, &["save", "--publish-only"], "push", failure);
                if mode == "signal" {
                    command.env("ONEFLOW_ACTIVITY_FAILURE", "signal");
                }
                command
                    .env("TERM", "xterm-256color")
                    .env_remove("NO_COLOR")
                    .env_remove("CLICOLOR");
                let mut term = terminal::Terminal::start_command(command);
                term.until("Publishing selected references");
                assert!(
                    !term.output.contains("Git push completed."),
                    "{}",
                    term.output
                );
                let status = term.finish();
                term.assert_restored();
                assert_eq!(
                    status.code(),
                    Some(if failure { 1 } else { 0 }),
                    "{}",
                    term.output
                );
                let outcome = match mode {
                    "failure" => "Git push exited with status 42.",
                    "signal" => "Git push terminated before completion.",
                    _ => "Git push completed.",
                };
                assert_eq!(term.output.matches(outcome).count(), 1, "{}", term.output);
                assert!(term.output.contains("\x1b["), "{}", term.output);
                assert_eq!(oid(&remote, "main"), before);
                if failure {
                    if mode == "failure" {
                        assert!(
                            term.output.contains("controlled Git failure"),
                            "{}",
                            term.output
                        );
                    }
                    assert_ne!(oid(&remote, "work/activity"), oid(&local, "HEAD"));
                } else {
                    assert_eq!(oid(&remote, "work/activity"), oid(&local, "HEAD"));
                }
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn colour_disabled_terminal_activity_has_no_animation_or_ansi() {
    isolated(
        "colour_disabled_terminal_activity_has_no_animation_or_ansi",
        true,
        |root| {
            let (local, _remote) = fixture(root);
            let slow = SlowGit::new(root);
            for (index, preference) in ["NO_COLOR", "CLICOLOR"].iter().enumerate() {
                let name = format!("work/plain-{index}");
                let mut command = slow.cli(&local, &["start", &name], "fetch", false);
                command.env("TERM", "xterm-256color").env(preference, "0");
                let mut term = terminal::Terminal::start_command(command);
                term.until("Fetching repository objects...");
                assert!(term.finish().success(), "{}", term.output);
                term.assert_restored();
                assert_eq!(
                    term.output
                        .matches("Fetching repository objects...")
                        .count(),
                    1,
                    "{}",
                    term.output
                );
                assert_eq!(
                    term.output.matches("Git fetch completed.").count(),
                    1,
                    "{}",
                    term.output
                );
                assert!(!term.output.contains('\x1b'), "{}", term.output);
                local.success(&["switch", "main"]);
            }
        },
    );
}

#[test]
fn slow_commit_activity_preserves_the_selected_and_remaining_work() {
    isolated(
        "slow_commit_activity_preserves_the_selected_and_remaining_work",
        true,
        |root| {
            let (local, remote) = fixture(root);
            local.success(&["switch", "--no-track", "-c", "work/commit"]);
            fs::write(local.path().join("selected"), "selected\n").unwrap();
            local.success(&["add", "selected"]);
            fs::write(local.path().join("base"), "remaining local work\n").unwrap();
            let slow = SlowGit::new(root);
            let output = slow
                .cli(
                    &local,
                    &["save", "--commit", "staged", "--message", "Selected work"],
                    "commit",
                    false,
                )
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                "Creating commit...\nGit commit completed.\n"
            );
            assert_eq!(oid(&remote, "work/commit"), oid(&local, "HEAD"));
            assert_eq!(
                local.success(&["show", "HEAD:selected"]).stdout,
                b"selected\n"
            );
            assert_eq!(
                fs::read(local.path().join("base")).unwrap(),
                b"remaining local work\n"
            );
            assert_eq!(
                local.success(&["status", "--porcelain"]).stdout,
                b" M base\n"
            );
        },
    );
}
