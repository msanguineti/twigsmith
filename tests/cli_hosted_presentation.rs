//! Hosted terminal input and credential presentation through the canonical CLI.
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;

use git_oneflow::{config, settings};
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};

struct Fixture {
    local: Repository,
    _project: tempfile::TempDir,
}

fn run(local: &Repository, command: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
        .current_dir(local.path())
        .arg(command)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

impl Fixture {
    fn new(root: &Path, update: settings::FinishUpdate, credentials: Option<bool>) -> Self {
        let local = Repository::new(root);
        fs::write(local.path().join("base"), "approved source\n").unwrap();
        local.success(&["add", "base"]);
        local.success(&["commit", "-m", "base"]);
        let project = tempfile::tempdir_in(root).unwrap();
        local.success(&[
            "clone",
            "--bare",
            local.path().to_str().unwrap(),
            project.path().to_str().unwrap(),
        ]);
        local.success(&["remote", "add", "origin", project.path().to_str().unwrap()]);
        let mut configured = settings::Settings::default();
        settings::oneflow_hosted_bundle(&mut configured, None);
        configured
            .workstreams
            .get_mut(&settings::WorkstreamKind::Release)
            .unwrap()
            .finish_update = Some(update.as_str().into());
        if credentials.is_some() {
            configured.finish_checks.push(settings::FinishCheck {
                name: "Credential output".into(),
                command: vec![
                    "sh".into(),
                    "-c".into(),
                    "printf '%s' 'https://alice:hosted-secret@example.invalid/repo'; exit 1".into(),
                ],
            });
        }
        let mut document: serde_json::Value =
            serde_json::from_str(&config::render(config::Format::Json, &configured)).unwrap();
        if let Some(redact) = credentials {
            document["redact-credentials"] = redact.into();
        }
        fs::write(
            local.path().join(".git/git-oneflow.config.json"),
            document.to_string(),
        )
        .unwrap();
        let result = run(
            &local,
            "start",
            &["release/source", "--policy", "release", "--from", "main"],
        );
        assert!(result.status.success(), "{result:?}");
        Self {
            local,
            _project: project,
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn hosted_cancel_eof_and_context_changes_stop_before_effects() {
    isolated(
        "hosted_cancel_eof_and_context_changes_stop_before_effects",
        true,
        |root| {
            for (args, prompt, response, change) in [
                (
                    vec!["finish"],
                    "Version tag (or cancel)",
                    b"cancel\n".as_slice(),
                    false,
                ),
                (
                    vec!["finish", "--tag", "v1"],
                    "Merge-back branch (or cancel)",
                    b"\x04".as_slice(),
                    false,
                ),
                (
                    vec!["finish", "--tag", "v1", "--merge-back", "review/v1"],
                    "Merge-back update (merge/none/cancel)",
                    b"cancel\n".as_slice(),
                    false,
                ),
                (
                    vec!["finish", "--tag", "v1"],
                    "Merge-back branch (or cancel)",
                    b"review/v1\n".as_slice(),
                    true,
                ),
            ] {
                let fixture = Fixture::new(root, settings::FinishUpdate::Ask, None);
                let before = fixture.local.success(&["show-ref"]).stdout;
                let index = fs::read(fixture.local.path().join(".git/index")).unwrap();
                let mut terminal = terminal::Terminal::start(
                    env!("CARGO_BIN_EXE_git-oneflow"),
                    fixture.local.path(),
                    &args,
                );
                terminal.until(prompt);
                if change {
                    fixture
                        .local
                        .success(&["config", "oneflow.test-context", "changed"]);
                }
                terminal.send(response);
                assert_eq!(terminal.finish().code(), Some(1), "{}", terminal.output);
                assert_eq!(fixture.local.success(&["show-ref"]).stdout, before);
                assert_eq!(
                    fs::read(fixture.local.path().join(".git/index")).unwrap(),
                    index
                );
                assert_eq!(
                    fs::read_to_string(fixture.local.path().join("base")).unwrap(),
                    "approved source\n"
                );
                assert!(!fixture.local.path().join(".git/oneflow-hosted").exists());
            }
        },
    );
}

#[test]
fn hosted_streamed_credentials_follow_private_and_flag_preferences() {
    isolated(
        "hosted_streamed_credentials_follow_private_and_flag_preferences",
        true,
        |root| {
            for (redact, show) in [(true, false), (false, false), (true, true)] {
                let fixture = Fixture::new(root, settings::FinishUpdate::None, Some(redact));
                let mut args = vec!["--tag", "v1", "--merge-back", "review/v1"];
                if show {
                    args.push("--show-credentials");
                }
                let output = run(&fixture.local, "finish", &args);
                assert_eq!(output.status.code(), Some(1), "{output:?}");
                let rendered = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(rendered.contains("example.invalid/repo"), "{rendered}");
                assert_eq!(
                    rendered.contains("alice:hosted-secret"),
                    !redact || show,
                    "{rendered}"
                );
                assert!(!fixture.local.path().join(".git/refs/tags/v1").exists());
            }
        },
    );
}
