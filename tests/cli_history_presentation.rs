#![cfg(unix)]
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};

const WORK: &str = "work/editor";
const WORK_REF: &str = "refs/heads/work/editor";
const REWORD: &str = "sed 's/^pick /reword /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"";
fn script(root: &Path, name: &str, body: &str) -> String {
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}
fn commit(repo: &Repository, name: &str) {
    fs::write(repo.path().join(name), name).unwrap();
    repo.success(&["add", name]);
    repo.success(&["commit", "-m", name]);
}
fn oid(repo: &Repository, name: &str) -> String {
    String::from_utf8(repo.success(&["rev-parse", name]).stdout)
        .unwrap()
        .trim()
        .into()
}
fn fixture(root: &Path, settings: &str, policy: bool) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base");
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin"]);
    local.success(&["switch", "--no-track", "-c", "main", "origin/main"]);
    fs::write(local.path().join(".git/twigsmith.config.toml"), settings).unwrap();
    if policy {
        let output = Command::new(env!("CARGO_BIN_EXE_twigsmith"))
            .current_dir(local.path())
            .args(["start", WORK, "--policy", "ordinary"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    } else {
        local.success(&["switch", "--no-track", "-c", WORK]);
    }
    commit(&local, "first");
    (local, remote)
}
fn command(local: &Repository, binary: &str, alias: &str, extra: &[&str]) -> Command {
    let mut cmd = Command::new(binary);
    cmd.current_dir(local.path())
        .args([
            alias,
            "--mode",
            "review",
            "--update",
            "rebase",
            "--no-rewrite",
        ])
        .args(extra)
        .env_remove("GIT_SEQUENCE_EDITOR")
        .env_remove("GIT_EDITOR")
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .stdin(Stdio::null());
    cmd
}
fn success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}
fn no_delivery(remote: &Repository, base: &str, output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(oid(remote, "main"), base);
    let refs = remote.success(&["for-each-ref", "--format=%(refname)", WORK_REF]);
    assert!(refs.stdout.is_empty(), "{output:?}");
}
#[test]
fn native_history_flags_reach_both_executables_and_aliases_with_real_publication() {
    isolated(
        "native_history_flags_reach_both_executables_and_aliases_with_real_publication",
        true,
        |root| {
            for binary in [env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")] {
                for alias in ["finish", "f"] {
                    let dir = tempfile::tempdir_in(root).unwrap();
                    let (local, remote) = fixture(dir.path(), "", false);
                    let before = oid(&local, "HEAD");
                    let base = oid(&remote, "main");
                    let sequence = script(dir.path(), "sequence", REWORD);
                    let message = script(
                        dir.path(),
                        "commit editor",
                        "printf 'Native subject\\n' > \"$1\"",
                    );
                    let output = command(&local, binary, alias, &["--history-edit", "interactive"])
                        .env("GIT_SEQUENCE_EDITOR", &sequence)
                        .env("GIT_EDITOR", &message)
                        .output()
                        .unwrap();
                    success(&output);
                    let edited = oid(&local, "HEAD");
                    assert_ne!(edited, before);
                    assert_eq!(oid(&remote, WORK_REF), edited);
                    assert_eq!(oid(&remote, "main"), base);
                    assert_eq!(
                        local.success(&["log", "-1", "--format=%s"]).stdout,
                        b"Native subject\n"
                    );
                    let notices = String::from_utf8_lossy(&output.stderr);
                    assert!(
                        notices.contains(WORK) && notices.contains(&base),
                        "{output:?}"
                    );
                }
            }
        },
    );
}
#[test]
fn explicit_history_none_overrides_configured_editing_and_workstream_overrides_global() {
    isolated(
        "explicit_history_none_overrides_configured_editing_and_workstream_overrides_global",
        true,
        |root| {
            for (policy, settings, extra, edited) in [
                (
                    false,
                    "finish-history-edit='interactive'\n",
                    vec!["--history-edit", "none"],
                    false,
                ),
                (
                    true,
                    "finish-history-edit='none'\n[workstreams.ordinary]\nfinish-history-edit='interactive'\n",
                    vec![],
                    true,
                ),
                (
                    true,
                    "finish-history-edit='interactive'\n[workstreams.ordinary]\nfinish-history-edit='none'\n",
                    vec![],
                    false,
                ),
            ] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let (local, remote) = fixture(dir.path(), settings, policy);
                let source = oid(&local, "HEAD");
                let sequence = script(dir.path(), "sequence", REWORD);
                let message = script(
                    dir.path(),
                    "message",
                    "printf 'Configured edit\\n' > \"$1\"",
                );
                let output = command(&local, env!("CARGO_BIN_EXE_twigsmith"), "finish", &extra)
                    .env(
                        "GIT_SEQUENCE_EDITOR",
                        if edited { &sequence } else { "false" },
                    )
                    .env("GIT_EDITOR", if edited { &message } else { "false" })
                    .output()
                    .unwrap();
                success(&output);
                assert_eq!(oid(&local, "HEAD") != source, edited);
                assert_eq!(oid(&remote, WORK_REF), oid(&local, "HEAD"));
            }
        },
    );
}
#[test]
fn native_git_sequence_and_commit_editor_preferences_and_environment_precedence() {
    isolated(
        "native_git_sequence_and_commit_editor_preferences_and_environment_precedence",
        true,
        |root| {
            for selection in ["configured", "environment", "core-fallback"] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let (local, remote) =
                    fixture(dir.path(), "finish-history-edit='interactive'\n", false);
                let sequence = script(dir.path(), "sequence", REWORD);
                let message = script(
                    dir.path(),
                    "message",
                    "printf 'Configured subject\\n' > \"$1\"",
                );
                let fallback = script(
                    dir.path(),
                    "fallback",
                    &format!(
                        "case \"$1\" in *git-rebase-todo) {REWORD} ;; *) printf 'Fallback subject\\n' > \"$1\" ;; esac"
                    ),
                );
                local.success(&[
                    "config",
                    "core.editor",
                    if selection == "core-fallback" {
                        &fallback
                    } else {
                        &message
                    },
                ]);
                if selection != "core-fallback" {
                    local.success(&["config", "sequence.editor", &sequence]);
                }
                let mut cmd = command(&local, env!("CARGO_BIN_EXE_twigsmith"), "finish", &[]);
                if selection == "environment" {
                    local.success(&["config", "core.editor", "false"]);
                    local.success(&["config", "sequence.editor", "false"]);
                    let env_message = script(
                        dir.path(),
                        "environment editor",
                        "printf 'Environment subject\\n' > \"$1\"",
                    );
                    cmd.env("GIT_SEQUENCE_EDITOR", &sequence)
                        .env("GIT_EDITOR", env_message);
                }
                success(&cmd.output().unwrap());
                let expected = match selection {
                    "configured" => "Configured subject",
                    "environment" => "Environment subject",
                    _ => "Fallback subject",
                };
                assert_eq!(
                    local.success(&["log", "-1", "--format=%s"]).stdout,
                    format!("{expected}\n").as_bytes()
                );
                assert_eq!(oid(&remote, WORK_REF), oid(&local, "HEAD"));
            }
        },
    );
}
#[test]
fn failed_or_cancelled_editors_hooks_and_signing_cannot_publish() {
    isolated(
        "failed_or_cancelled_editors_hooks_and_signing_cannot_publish",
        true,
        |root| {
            for failure in ["sequence", "message", "empty-message", "hook", "signing"] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let (local, remote) =
                    fixture(dir.path(), "finish-history-edit='interactive'\n", false);
                let base = oid(&remote, "main");
                let sequence = script(
                    dir.path(),
                    "sequence",
                    if failure == "sequence" {
                        "echo sequence-failed >&2; exit 7"
                    } else {
                        REWORD
                    },
                );
                let message = script(
                    dir.path(),
                    "message",
                    match failure {
                        "message" => "echo message-failed >&2; exit 7",
                        "empty-message" => "echo message-cleared >&2; : > \"$1\"",
                        _ => "printf 'Prepared subject\\n' > \"$1\"",
                    },
                );
                if failure == "hook" {
                    let hooks = dir.path().join("hooks");
                    fs::create_dir(&hooks).unwrap();
                    let path = hooks.join("pre-rebase");
                    fs::write(&path, "#!/bin/sh\necho pre-rebase-failed >&2\nexit 9\n").unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
                    local.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
                }
                if failure == "signing" {
                    local.success(&["config", "commit.gpgSign", "true"]);
                    local.success(&["config", "gpg.program", "false"]);
                }
                let output = command(&local, env!("CARGO_BIN_EXE_twigsmith"), "finish", &[])
                    .env("GIT_SEQUENCE_EDITOR", sequence)
                    .env("GIT_EDITOR", message)
                    .output()
                    .unwrap();
                no_delivery(&remote, &base, &output);
                let diagnostic = match failure {
                    "sequence" => "sequence-failed",
                    "message" => "message-failed",
                    "empty-message" => "message-cleared",
                    "hook" => "pre-rebase-failed",
                    "signing" => "failed to sign",
                    _ => unreachable!(),
                };
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains(diagnostic),
                    "{output:?}"
                );
            }
        },
    );
}
#[test]
fn scripted_native_editor_failure_diagnostics_respect_credential_presentation() {
    isolated(
        "scripted_native_editor_failure_diagnostics_respect_credential_presentation",
        true,
        |root| {
            for show in [false, true] {
                let dir = tempfile::tempdir_in(root).unwrap();
                let (local, remote) =
                    fixture(dir.path(), "finish-history-edit='interactive'\n", false);
                let base = oid(&remote, "main");
                let sequence = script(
                    dir.path(),
                    "diagnostic",
                    "echo 'https://user:synthetic-editor-secret@example.invalid/repo' >&2; exit 7",
                );
                let output = command(
                    &local,
                    env!("CARGO_BIN_EXE_twigsmith"),
                    "finish",
                    if show { &["--show-credentials"] } else { &[] },
                )
                .env("GIT_SEQUENCE_EDITOR", sequence)
                .env("GIT_EDITOR", ":")
                .output()
                .unwrap();
                no_delivery(&remote, &base, &output);
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(text.contains("synthetic-editor-secret"), show, "{text}");
                assert!(text.contains("example.invalid/repo"), "{text}");
                if !show {
                    assert!(text.contains("<redacted>"), "{text}");
                }
            }
        },
    );
}
#[cfg(target_os = "linux")]
#[test]
fn native_editor_receives_terminal_input_without_a_redundant_confirmation() {
    isolated(
        "native_editor_receives_terminal_input_without_a_redundant_confirmation",
        true,
        |root| {
            let (local, remote) = fixture(root, "finish-history-edit='interactive'\n", false);
            let sequence = script(
                root,
                "terminal sequence",
                &format!(
                    "test -t 0 && test -t 1 && test -t 2\nprintf 'Native sequence input: ' >&2\nread choice\ntest \"$choice\" = reword\n{REWORD}"
                ),
            );
            let message = script(
                root,
                "terminal message",
                "printf 'Terminal subject\\n' > \"$1\"",
            );
            local.success(&["config", "sequence.editor", &sequence]);
            local.success(&["config", "core.editor", &message]);
            let mut terminal = terminal::Terminal::start_capable(
                env!("CARGO_BIN_EXE_twigsmith"),
                local.path(),
                &["finish", "--update", "rebase", "--no-rewrite"],
            );
            terminal.until("How should this work be delivered?");
            terminal.send(b"\x1b[A\x1b[A\r");
            terminal.until("Native sequence input:");
            terminal.assert_restored();
            assert!(terminal.output.contains(WORK), "{}", terminal.output);
            terminal.send(b"reword\n");
            assert_eq!(terminal.finish().code(), Some(0), "{}", terminal.output);
            terminal.assert_restored();
            assert_eq!(oid(&remote, WORK_REF), oid(&local, "HEAD"));
            assert_eq!(
                local.success(&["log", "-1", "--format=%s"]).stdout,
                b"Terminal subject\n"
            );
        },
    );
}
