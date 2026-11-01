//! Actual dispatcher prompts in native PTY/ConPTY, with independent Git oracles.
#[path = "support/snapshot.rs"]
mod preservation;
mod support;
#[path = "support/native_terminal.rs"]
mod terminal;

use preservation::snapshot;
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use support::{Repository, isolated};
use terminal::NativeTerminal;
use twigsmith::{git, repository, settings};

const WORK: &str = "feature/β;$";
fn command(executable: &str, repo: &Repository, args: &[&str]) -> Command {
    let mut command = Command::new(support::executable(executable));
    command
        .current_dir(repo.path())
        .args(args)
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1");
    command
}
fn terminal(executable: &str, repo: &Repository, args: &[&str]) -> NativeTerminal {
    NativeTerminal::start_command(command(executable, repo, args))
}
fn keyboard(term: &NativeTerminal) {
    assert!(
        term.output.contains("\x1b["),
        "no keyboard terminal control: {}",
        term.output
    );
    // Colour suppression still permits the controls needed for keyboard input.
    assert!(
        !term.output.contains("\x1b[38;"),
        "NO_COLOR ignored: {}",
        term.output
    );
}
fn commit(repo: &Repository, path: &str, content: &str) {
    fs::write(repo.path().join(path), content).unwrap();
    repo.success(&["add", path]);
    repo.success(&["commit", "-m", path]);
}
fn oid(repo: &Repository, reference: &str) -> Option<Vec<u8>> {
    let result = git::run(repo.path(), ["rev-parse", "--verify", reference]).unwrap();
    result.status.success().then_some(result.stdout)
}
fn fixture(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base", "base\n");
    remote.success(&["switch", "--detach"]);
    let repo = Repository::new(root);
    repo.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    repo.success(&["fetch", "origin"]);
    repo.success(&["reset", "--hard", "origin/main"]);
    repo.success(&["switch", "-c", WORK]);
    commit(&repo, "work", "work\n");
    (repo, remote)
}
fn load(repo: &Repository) -> settings::Settings {
    settings::load(
        repo.path(),
        &repository::discover(repo.path()).unwrap().unwrap(),
    )
    .unwrap()
}
fn config_review(term: &mut NativeTerminal) {
    for question in [
        "Saving preferences:",
        "Finishing preferences:",
        "Finish checks:",
    ] {
        term.until(question);
        term.send(b"\r");
    }
    term.until("Create this file?");
}
fn finish(term: &mut NativeTerminal, code: i32) {
    assert_eq!(term.finish().code(), Some(code), "{}", term.output);
    term.assert_restored();
}

#[test]
fn native_config_literal_suggestions_and_explicit_creation() {
    isolated(
        "native_config_literal_suggestions_and_explicit_creation",
        true,
        |root| {
            let repo = Repository::new(root);
            commit(&repo, "base", "base\n");
            repo.success(&["remote", "add", "réseau", "/offline"]);
            repo.success(&["branch", WORK]);
            let git_before = snapshot(&repo.path().join(".git"));
            let mut term =
                NativeTerminal::start_command_width(command("twigsmith", &repo, &["config"]), 34);
            term.until("Remote receiving work");
            term.send("ré\t\r".as_bytes());
            term.until("Project source remote");
            term.send(b"\r");
            term.until("Project base branch");
            term.send("feature/\t\r".as_bytes());
            config_review(&mut term);
            keyboard(&term);
            assert!(!repo.path().join(settings::CONFIG_FILES[0]).exists());
            term.send(b"\x1b[A\r");
            finish(&mut term, 0);
            assert_eq!(load(&repo).publish_remote, "réseau");
            assert_eq!(load(&repo).base, WORK);
            assert_eq!(snapshot(&repo.path().join(".git")), git_before);
            // A second setup cannot overwrite recognized editable configuration.
            let before = snapshot(repo.path());
            let mut existing = terminal("tgi", &repo, &["config"]);
            finish(&mut existing, 1);
            assert_eq!(snapshot(repo.path()), before);
        },
    );
}

#[test]
fn native_config_default_is_explicit_and_creation_defaults_to_cancel() {
    isolated(
        "native_config_default_is_explicit_and_creation_defaults_to_cancel",
        true,
        |root| {
            let repo = Repository::new(root);
            let before = snapshot(repo.path());
            let mut term = terminal("tgi", &repo, &["config"]);
            for prompt in [
                "Remote receiving work",
                "Project source remote",
                "Project base branch",
            ] {
                term.until(prompt);
                term.send(b"\r");
            }
            config_review(&mut term);
            keyboard(&term);
            term.send(b"\r");
            finish(&mut term, 0);
            assert!(term.output.contains("Setup cancelled"), "{}", term.output);
            assert_eq!(snapshot(repo.path()), before);
        },
    );
}

#[test]
fn native_keyboard_cancellations_preserve_config_and_lifecycle_work() {
    isolated(
        "native_keyboard_cancellations_preserve_config_and_lifecycle_work",
        true,
        |root| {
            for cancel in [b"\x1b".as_slice(), b"\x03", b"\x04"] {
                let repo = Repository::new(root);
                let before = snapshot(repo.path());
                let mut term = terminal("twigsmith", &repo, &["config"]);
                term.until("Remote receiving work");
                keyboard(&term);
                term.send(cancel);
                finish(&mut term, 0);
                assert!(term.output.contains("Setup cancelled"));
                assert_eq!(snapshot(repo.path()), before);

                let (repo, remote) = fixture(root);
                fs::write(repo.path().join("work"), "pending\n").unwrap();
                let before = snapshot(repo.path());
                let mut term = terminal("tgi", &repo, &["save"]);
                term.until("How would you like to save this work?");
                keyboard(&term);
                term.send(cancel);
                finish(&mut term, 1);
                assert_eq!(snapshot(repo.path()), before);
                assert!(oid(&remote, &format!("refs/heads/{WORK}")).is_none());
            }
        },
    );
}

#[test]
fn native_save_and_finish_submit_real_choices_and_publish_literal_ref() {
    isolated(
        "native_save_and_finish_submit_real_choices_and_publish_literal_ref",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            let initial = oid(&repo, "HEAD");
            fs::write(repo.path().join("work"), "staged\n").unwrap();
            repo.success(&["add", "work"]);
            fs::write(repo.path().join("work"), "remaining\n").unwrap();
            fs::write(repo.path().join("literal λ ;$ file"), "new\n").unwrap();
            let mut term = terminal("tgi", &repo, &["save"]);
            term.until("How would you like to save this work?");
            keyboard(&term);
            // Five Up events move from the initial Cancel to Commit all.
            term.send(b"\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A\r");
            finish(&mut term, 0);
            assert_ne!(oid(&repo, "HEAD"), initial);
            assert_eq!(
                oid(&remote, &format!("refs/heads/{WORK}")),
                oid(&repo, "HEAD")
            );
            assert_eq!(repo.success(&["show", "HEAD:work"]).stdout, b"remaining\n");
            assert_eq!(
                repo.success(&["show", "HEAD:literal λ ;$ file"]).stdout,
                b"new\n"
            );
            assert!(repo.success(&["diff", "--cached"]).stdout.is_empty());
            assert!(repo.success(&["status", "--porcelain"]).stdout.is_empty());
            assert_eq!(
                repo.success(&["config", &format!("branch.{WORK}.remote")])
                    .stdout,
                b"origin\n"
            );

            // A fresh unpublished work commit makes review delivery independently observable.
            commit(&repo, "review", "review\n");
            let candidate = oid(&repo, "HEAD");
            let main = oid(&remote, "refs/heads/main");
            let index = fs::read(repo.path().join(".git/index")).unwrap();
            let mut term = terminal("twigsmith", &repo, &["finish"]);
            term.until("How should this work be delivered?");
            keyboard(&term);
            term.send(b"\x1b[A\x1b[A\r");
            finish(&mut term, 0);
            assert_eq!(oid(&remote, &format!("refs/heads/{WORK}")), candidate);
            assert_eq!(oid(&repo, "HEAD"), candidate);
            assert_eq!(oid(&remote, "refs/heads/main"), main);
            assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
            assert_eq!(fs::read(repo.path().join("review")).unwrap(), b"review\n");
            assert!(repo.success(&["status", "--porcelain"]).stdout.is_empty());
        },
    );
}

#[test]
fn native_prompt_context_mutation_prevents_pending_save() {
    isolated(
        "native_prompt_context_mutation_prevents_pending_save",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            fs::write(repo.path().join("work"), "pending\n").unwrap();
            let head = oid(&repo, "HEAD");
            let index = fs::read(repo.path().join(".git/index")).unwrap();
            let mut term = terminal("tgi", &repo, &["save"]);
            term.until("How would you like to save this work?");
            fs::write(
                repo.path().join(".git/twigsmith.config.toml"),
                "save-message='Changed during prompt'\n",
            )
            .unwrap();
            term.send(b"commit-all\r");
            finish(&mut term, 1);
            assert!(term.output.contains("changed"), "{}", term.output);
            assert_eq!(oid(&repo, "HEAD"), head);
            assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
            assert_eq!(fs::read(repo.path().join("work")).unwrap(), b"pending\n");
            assert!(oid(&remote, &format!("refs/heads/{WORK}")).is_none());
        },
    );
}

#[test]
fn native_operational_failure_restores_keyboard_terminal() {
    isolated(
        "native_operational_failure_restores_keyboard_terminal",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            let head = oid(&repo, "HEAD");
            // This portable Git policy rejects a push to the remote's checked-out branch.
            remote.success(&["switch", "-c", WORK]);
            let target = oid(&remote, "HEAD");
            let mut term = terminal("twigsmith", &repo, &["finish", "--update", "none"]);
            term.until("How should this work be delivered?");
            keyboard(&term);
            term.send(b"review\r");
            finish(&mut term, 1);
            assert!(
                term.output
                    .contains("refusing to update checked out branch"),
                "{}",
                term.output
            );
            assert_eq!(oid(&repo, "HEAD"), head);
            assert_eq!(oid(&remote, "HEAD"), target);
            assert_eq!(fs::read(repo.path().join("work")).unwrap(), b"work\n");
        },
    );
}

#[test]
fn native_dumb_complete_lines_and_eof_do_not_approve_partial_input() {
    isolated(
        "native_dumb_complete_lines_and_eof_do_not_approve_partial_input",
        true,
        |root| {
            // Unix canonical EOF is EOT; Windows console EOF is a different boundary.
            // Portable adapter EOF/incomplete checks live in presentation tests.
            #[cfg(unix)]
            for partial in [false, true] {
                let repo = Repository::new(root);
                let before = snapshot(repo.path());
                let mut cmd = command("twigsmith", &repo, &["config"]);
                cmd.env("TERM", "dumb");
                let mut term = NativeTerminal::start_command(cmd);
                term.until("Remote receiving work");
                if partial {
                    term.send(b"future-remote\x04\x04");
                } else {
                    term.send(b"\x04");
                }
                finish(&mut term, 0);
                assert!(term.output.contains("Setup cancelled"));
                assert!(
                    !term.output.contains("Project source remote"),
                    "partial EOF submitted: {}",
                    term.output
                );
                assert_eq!(snapshot(repo.path()), before);
            }
            let (repo, remote) = fixture(root);
            fs::write(repo.path().join("work"), "pending\n").unwrap();
            let mut cmd = command("tgi", &repo, &["save"]);
            cmd.env("TERM", "dumb");
            let mut term = NativeTerminal::start_command(cmd);
            term.until("How would you like to save this work?");
            term.send(b"unmatched\n");
            term.until("Choose a listed name or number");
            term.send(b"commit-all\n");
            finish(&mut term, 0);
            assert_eq!(repo.success(&["show", "HEAD:work"]).stdout, b"pending\n");
            assert_eq!(
                oid(&repo, "HEAD"),
                oid(&remote, &format!("refs/heads/{WORK}"))
            );

            for answer in [b"".as_slice(), b"commit-all", b"commit-all\n"] {
                let (repo, remote) = fixture(root);
                fs::write(repo.path().join("work"), "unresolved\n").unwrap();
                let before = snapshot(repo.path());
                let mut child = command("tgi", &repo, &["save"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child.stdin.take().unwrap().write_all(answer).unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
                while child.try_wait().unwrap().is_none() {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("nonterminal unresolved save exceeded deadline");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                let output = child.wait_with_output().unwrap();
                assert_eq!(output.status.code(), Some(1), "{output:?}");
                assert!(
                    !output.stdout.contains(&0x1b) && !output.stderr.contains(&0x1b),
                    "{output:?}"
                );
                assert_eq!(snapshot(repo.path()), before);
                assert!(oid(&remote, &format!("refs/heads/{WORK}")).is_none());
            }
        },
    );
}

#[test]
fn native_redirected_output_uses_complete_line_fallback_and_plain_report() {
    isolated(
        "native_redirected_output_uses_complete_line_fallback_and_plain_report",
        true,
        |root| {
            let repo = Repository::new(root);
            commit(&repo, "base", "base\n");
            let before = snapshot(&repo.path().join(".git"));
            let report = root.join("redirected report.txt");
            let fixture = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("examples")
                .join(format!(
                    "native_editor_fixture{}",
                    std::env::consts::EXE_SUFFIX
                ));
            let mut cmd = Command::new(fixture);
            cmd.current_dir(repo.path())
                .arg("--redirect-stdout")
                .arg(&report)
                .arg(support::executable("tgi"))
                .arg("config")
                .env("TERM", "xterm-256color")
                .env("CLICOLOR", "0");
            let mut term = NativeTerminal::start_command(cmd);
            term.until("Remote receiving work");
            // With terminal input/stderr and redirected stdout, complete lines own
            // submission even though TERM advertises keyboard capabilities.
            term.send(b"future\n\nfuture/base\n");
            for question in [
                "Saving preferences:",
                "Finishing preferences:",
                "Finish checks:",
            ] {
                term.until(question);
                term.send(b"keep\n");
            }
            term.until("Create this file?");
            assert!(!repo.path().join(settings::CONFIG_FILES[0]).exists());
            term.send(b"yes\n");
            finish(&mut term, 0);
            assert_eq!(load(&repo).publish_remote, "future");
            assert_eq!(load(&repo).base, "future/base");
            assert_eq!(snapshot(&repo.path().join(".git")), before);
            let report = fs::read(report).unwrap();
            assert!(
                !report.contains(&0x1b),
                "redirected report contains controls: {report:?}"
            );
            // ANSI controls here would reveal accidental Inquire selection on Unix;
            // ConPTY itself may emit cursor/viewport controls, so use semantic help too.
            assert!(
                !term.output.contains("↑/↓ chooses; type name or number"),
                "{}",
                term.output
            );
            assert!(term.output.contains("  1) keep"), "{}", term.output);
        },
    );
}
