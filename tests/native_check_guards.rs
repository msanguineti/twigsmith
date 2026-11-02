//! Native process execution at the existing guarded review-publication boundary.
#[allow(dead_code)]
#[path = "support/native.rs"]
mod native;
mod support;

use std::{fs, io::Write, path::Path, process::Command, thread, time::Duration};
use support::{Repository, isolated};
use twigsmith::{
    config,
    settings::{FinishCheck, Settings},
};

fn head(repo: &Repository, reference: &str) -> Vec<u8> {
    repo.success(&["rev-parse", reference]).stdout
}
fn fixture(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    fs::write(remote.path().join("tracked"), "base\n").unwrap();
    fs::write(remote.path().join(".gitignore"), "build-output\n").unwrap();
    remote.success(&["add", "."]);
    remote.success(&["commit", "-m", "base"]);
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin"]);
    local.success(&["switch", "--no-track", "-c", "work", "origin/main"]);
    fs::write(local.path().join("tracked"), "work\n").unwrap();
    local.success(&["commit", "-am", "work"]);
    (local, remote)
}
fn helper(name: &str) -> Vec<String> {
    vec![
        std::env::current_exe().unwrap().to_str().unwrap().into(),
        "--ignored".into(),
        "--exact".into(),
        name.into(),
        "--nocapture".into(),
    ]
}
fn configure(repo: &Repository, commands: Vec<Vec<String>>) {
    let settings = Settings {
        finish_checks: commands
            .into_iter()
            .enumerate()
            .map(|(index, command)| FinishCheck {
                name: format!("native guard {index}"),
                command,
            })
            .collect(),
        ..Default::default()
    };
    fs::write(
        repo.path().join(".git/twigsmith.config.toml"),
        config::render(config::Format::Toml, &settings),
    )
    .unwrap();
}
fn finish(repo: &Repository, extra: &[&str]) -> std::process::Output {
    let result = native::output(
        Command::new(support::executable("tgi"))
            .current_dir(repo.path())
            .args(["finish", "--mode", "review", "--update", "none"])
            .args(extra),
    );
    println!(
        "finish: {}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}
fn unpublished(remote: &Repository) {
    assert!(
        !twigsmith::git::run(remote.path(), ["show-ref", "--verify", "refs/heads/work"])
            .unwrap()
            .status
            .success()
    );
}

fn guarded_refusal(output: &std::process::Output) {
    assert!(!output.status.success(), "{output:?}");
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    // A process-collection error must not masquerade as a successful guard:
    // macOS originally refused publication for the wrong reason in these cases.
    assert!(diagnostic.contains("stopped:"), "{output:?}");
    assert!(
        !diagnostic.contains("cannot execute or collect check")
            && !diagnostic.contains("check failed with"),
        "{output:?}"
    );
}

#[test]
fn native_mutations_stop_later_checks_and_publication() {
    isolated(
        "native_mutations_stop_later_checks_and_publication",
        true,
        |root| {
            for mutation in [
                helper("mutate_tracked"),
                helper("mutate_settings"),
                vec![
                    "git".into(),
                    "rm".into(),
                    "--cached".into(),
                    "tracked".into(),
                ],
                vec![
                    "git".into(),
                    "reset".into(),
                    "--hard".into(),
                    "HEAD^".into(),
                ],
            ] {
                let (repo, remote) = fixture(root);
                let original = head(&repo, "HEAD");
                configure(&repo, vec![mutation.clone(), helper("later_check")]);
                let output = finish(&repo, &[]);
                guarded_refusal(&output);
                assert!(!repo.path().join(".git/later").exists());
                unpublished(&remote);
                if mutation == helper("mutate_tracked") {
                    assert_eq!(
                        fs::read(repo.path().join("tracked")).unwrap(),
                        b"changed by check\n"
                    );
                    assert_eq!(head(&repo, "HEAD"), original);
                }
            }
        },
    );
}

#[test]
fn native_hidden_work_is_guarded_and_ignored_output_is_allowed() {
    isolated(
        "native_hidden_work_is_guarded_and_ignored_output_is_allowed",
        true,
        |root| {
            for hint in ["--assume-unchanged", "--skip-worktree"] {
                let (repo, remote) = fixture(root);
                repo.success(&["update-index", hint, "tracked"]);
                configure(&repo, vec![helper("mutate_tracked"), helper("later_check")]);
                guarded_refusal(&finish(&repo, &[]));
                assert!(!repo.path().join(".git/later").exists());
                unpublished(&remote);
            }
            let (repo, remote) = fixture(root);
            configure(&repo, vec![helper("ignored_output"), helper("later_check")]);
            assert!(finish(&repo, &[]).status.success());
            assert_eq!(
                fs::read(repo.path().join("build-output")).unwrap(),
                b"build\n"
            );
            assert!(repo.path().join(".git/later").exists());
            assert_eq!(head(&repo, "HEAD"), head(&remote, "work"));
        },
    );
}

#[test]
fn native_remote_movement_refuses_then_retry_reruns_checks() {
    isolated(
        "native_remote_movement_refuses_then_retry_reruns_checks",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            let base = String::from_utf8(head(&remote, "main")).unwrap();
            configure(
                &repo,
                vec![
                    vec![
                        "git".into(),
                        "-C".into(),
                        remote.path().to_str().unwrap().into(),
                        "update-ref".into(),
                        "refs/heads/work".into(),
                        base.trim().into(),
                    ],
                    helper("later_check"),
                ],
            );
            let before = head(&repo, "HEAD");
            assert!(!finish(&repo, &[]).status.success());
            assert_eq!(head(&remote, "work"), base.as_bytes());
            assert_eq!(head(&repo, "HEAD"), before);
            fs::remove_file(repo.path().join(".git/later")).unwrap();
            assert!(finish(&repo, &[]).status.success());
            assert!(repo.path().join(".git/later").exists());
            assert_eq!(head(&remote, "work"), before);
        },
    );
}

#[test]
fn native_skip_and_streamed_redaction_preserve_check_policy() {
    isolated(
        "native_skip_and_streamed_redaction_preserve_check_policy",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            configure(
                &repo,
                vec![helper("fragmented_secret"), helper("later_check")],
            );
            let output = finish(&repo, &[]);
            assert!(output.status.success());
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!text.contains("sensitive-password"), "{text}");
            assert!(text.contains("<redacted>@example.invalid/path"), "{text}");
            assert_eq!(head(&remote, "work"), head(&repo, "HEAD"));
            fs::remove_file(repo.path().join(".git/later")).unwrap();
            assert!(finish(&repo, &["--skip-checks"]).status.success());
            assert!(!repo.path().join(".git/later").exists());
        },
    );
}

// Invoked as configured native checks, never as ordinary standalone tests.
#[test]
#[ignore]
fn mutate_tracked() {
    fs::write("tracked", "changed by check\n").unwrap();
}
#[test]
#[ignore]
fn mutate_settings() {
    fs::write(".git/twigsmith.config.toml", "base = 'different'\n").unwrap();
}
#[test]
#[ignore]
fn later_check() {
    fs::write(".git/later", "ran\n").unwrap();
}
#[test]
#[ignore]
fn ignored_output() {
    fs::write("build-output", "build\n").unwrap();
}
#[test]
#[ignore]
fn fragmented_secret() {
    for part in [
        "https://fixture:",
        "sensitive-",
        "password@example.invalid/path\n",
    ] {
        print!("{part}");
        std::io::stdout().flush().unwrap();
        thread::sleep(Duration::from_millis(60));
    }
}
