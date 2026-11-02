//! Native filesystem and recovery boundaries using disposable real Git repositories.
#[allow(dead_code)]
#[path = "support/native.rs"]
mod native;
mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use support::{Repository, isolated};
use twigsmith::{
    finish::integration::{
        self, Blocker, Plan, Request, State,
        recovery::{self, Problem, Record, Selection},
    },
    settings::{FinishCheckout, FinishCleanup, FinishIntegration, FinishUpdate},
};

const LOCAL_WORK: &str = "κόσμος 世界 local notes.txt";
const SETTINGS: &str = "base='main'\nbase-remote='project'\npublish-remote='project'\nfinish-mode='integrate'\nfinish-update='none'\nfinish-integration='merge'\nfinish-message='auto'\nfinish-cleanup='keep'\nfinish-checkout='target'\n";

struct Fixture {
    local: Repository,
    project: tempfile::TempDir,
}
fn oid(repo: &Repository, name: &str) -> Vec<u8> {
    repo.success(&["rev-parse", name]).stdout
}
fn commit(repo: &Repository, path: &str, contents: &str) {
    fs::write(repo.path().join(path), contents).unwrap();
    repo.success(&["add", path]);
    repo.success(&["commit", "-m", path]);
}
fn fixture(root: &Path) -> Fixture {
    let local = Repository::new(root);
    commit(&local, "base", "base\n");
    let project = tempfile::tempdir_in(root).unwrap();
    local.success(&[
        "clone".as_ref(),
        "--bare".as_ref(),
        local.path().as_os_str(),
        project.path().as_os_str(),
    ]);
    local.success(&["remote", "add", "project", project.path().to_str().unwrap()]);
    fs::write(local.path().join(".git/twigsmith.config.toml"), SETTINGS).unwrap();
    fs::create_dir_all(local.path().join(".git/info")).unwrap();
    local.success(&["switch", "-c", "work"]);
    commit(&local, "work", "work\n");
    Fixture { local, project }
}
fn request() -> Request {
    Request {
        integration: Some(FinishIntegration::Merge),
        update: Some(FinishUpdate::None),
        cleanup: Some(FinishCleanup::Keep),
        checkout: Some(FinishCheckout::Target),
        ..Default::default()
    }
}
fn refreshed(directory: &Path) -> Plan {
    let plan = integration::assess(directory, &request(), None).unwrap();
    let report = integration::remote::refresh(directory, &plan);
    assert!(report.error.is_none(), "{report:#?}");
    report.plan.unwrap()
}
fn finish(directory: &Path) -> Output {
    let output = native::output(
        Command::new(support::executable("tgi"))
            .current_dir(directory)
            .arg("finish"),
    );
    println!(
        "finish: {}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn private_write(root: &Path, path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut current = path.to_owned();
        while current != root {
            fs::set_permissions(
                &current,
                fs::Permissions::from_mode(if current.is_dir() { 0o700 } else { 0o600 }),
            )
            .unwrap();
            current.pop();
        }
    }
    #[cfg(not(unix))]
    let _ = root;
}
fn interrupted(f: &Fixture) -> (Plan, PathBuf) {
    let plan = refreshed(f.local.path());
    assert!(plan.is_ready(), "{plan:#?}");
    let record = Record::for_plan(&plan, "native-lost-acknowledgement").unwrap();
    let path = plan.recovery_path.clone().unwrap();
    private_write(
        &plan.repository.git_dir,
        &path,
        &serde_json::to_vec(&record).unwrap(),
    );
    f.local.success(&["switch", "main"]);
    let output = Command::new("git")
        .current_dir(f.local.path())
        .args([
            "merge",
            "--no-ff",
            "-m",
            "interrupted local transition",
            "work",
        ])
        .env("GIT_REFLOG_ACTION", record.marker())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let selection = Selection::observe(f.local.path(), "work").unwrap();
    private_write(
        &plan.repository.git_dir,
        &recovery::selection_path(&plan.repository),
        &serde_json::to_vec(&selection).unwrap(),
    );
    // Native Git completed the operation; the on-disk record intentionally has
    // no result acknowledgement. Recovery must corroborate it independently.
    assert_eq!(record.result, None);
    (plan, path)
}

#[test]
fn native_interrupted_transition_continues_and_missing_or_malformed_evidence_refuses() {
    isolated(
        "native_interrupted_transition_continues_and_missing_or_malformed_evidence_refuses",
        true,
        |root| {
            for problem in ["none", "missing", "malformed"] {
                let f = fixture(root);
                let original = oid(&f.local, "work");
                let (_, path) = interrupted(&f);
                let result = oid(&f.local, "main");
                let refs = f.local.success(&["show-ref"]).stdout;
                match problem {
                    "missing" => fs::remove_file(&path).unwrap(),
                    "malformed" => fs::write(&path, b"{broken").unwrap(),
                    _ => (),
                }
                let bytes = fs::read(&path).ok();
                let assessment = integration::assess(f.local.path(), &request(), None).unwrap();
                if problem == "none" {
                    assert_eq!(assessment.state, State::PreparedResult);
                } else {
                    assert!(
                        assessment
                            .blockers
                            .contains(&Blocker::Recovery(if problem == "missing" {
                                Problem::MissingRecord
                            } else {
                                Problem::MalformedRecord
                            })),
                        "{assessment:#?}"
                    );
                }
                let output = finish(f.local.path());
                assert_eq!(output.status.success(), problem == "none", "{output:?}");
                assert_eq!(oid(&f.local, "work"), original);
                assert_eq!(oid(&f.local, "main"), result);
                if problem == "none" {
                    assert_eq!(
                        twigsmith::git::run(f.project.path(), ["rev-parse", "main"])
                            .unwrap()
                            .stdout,
                        result
                    );
                } else {
                    assert_eq!(f.local.success(&["show-ref"]).stdout, refs);
                    assert_eq!(fs::read(&path).ok(), bytes);
                    assert_ne!(
                        twigsmith::git::run(f.project.path(), ["rev-parse", "main"])
                            .unwrap()
                            .stdout,
                        result
                    );
                }
                println!("CAPABILITY interrupted transition {problem}: verified");
            }
        },
    );
}

#[test]
fn native_linked_worktree_attempts_are_isolated_and_occupied_target_refuses() {
    isolated(
        "native_linked_worktree_attempts_are_isolated_and_occupied_target_refuses",
        true,
        |root| {
            let f = fixture(root);
            let main = oid(&f.local, "main");
            f.local.success(&["switch", "-c", "independent", "main"]);
            let linked = root.join("linked worktree κόσμος");
            f.local
                .success(&["worktree", "add", linked.to_str().unwrap(), "work"]);
            let selected = refreshed(&linked);
            let prepared = integration::execution::prepare(&linked, &selected);
            assert!(prepared.is_prepared(), "{prepared:#?}");
            let attempt = prepared.plan.unwrap().recovery_path.unwrap();
            assert!(
                fs::canonicalize(&attempt)
                    .unwrap()
                    .starts_with(fs::canonicalize(f.local.path().join(".git/worktrees")).unwrap()),
                "{}",
                attempt.display()
            );
            let original_attempt = fs::read(&attempt).unwrap();
            assert_eq!(
                f.local.success(&["branch", "--show-current"]).stdout,
                b"independent\n"
            );
            let independent = integration::assess(f.local.path(), &request(), None).unwrap();
            assert_eq!(independent.state, State::Fresh);
            assert_ne!(independent.recovery_path, Some(attempt.clone()));
            assert!(
                !independent
                    .blockers
                    .iter()
                    .any(|b| matches!(b, Blocker::Recovery(_)))
            );
            assert_eq!(fs::read(&attempt).unwrap(), original_attempt);
            // The linked integration currently owns main; a fresh workstream cannot
            // change it from this worktree, even though both share refs.
            assert_ne!(oid(&f.local, "main"), main);
            assert!(
                independent
                    .blockers
                    .contains(&Blocker::TargetInAnotherWorktree),
                "{independent:#?}"
            );
            let source = oid(&f.local, "HEAD");
            assert!(!finish(f.local.path()).status.success());
            assert_eq!(oid(&f.local, "HEAD"), source);
            assert_eq!(fs::read(&attempt).unwrap(), original_attempt);
        },
    );
}

#[test]
fn native_checkout_preserves_unicode_ignored_and_untracked_work() {
    isolated(
        "native_checkout_preserves_unicode_ignored_and_untracked_work",
        true,
        |root| {
            for ignored in [false, true] {
                let f = fixture(root);
                f.local.success(&["switch", "main"]);
                commit(&f.local, LOCAL_WORK, "tracked target contents\n");
                f.local.success(&["push", "project", "main"]);
                f.local.success(&["switch", "work"]);
                if ignored {
                    fs::write(
                        f.local.path().join(".git/info/exclude"),
                        format!("/{LOCAL_WORK}\n"),
                    )
                    .unwrap();
                }
                fs::write(
                    f.local.path().join(LOCAL_WORK),
                    "private developer contents\n",
                )
                .unwrap();
                let source = oid(&f.local, "work");
                let target = oid(&f.local, "main");
                assert!(!finish(f.local.path()).status.success());
                assert_eq!(
                    fs::read(f.local.path().join(LOCAL_WORK)).unwrap(),
                    b"private developer contents\n"
                );
                assert_eq!(oid(&f.local, "work"), source);
                assert_eq!(oid(&f.local, "main"), target);
                assert_eq!(
                    f.local.success(&["branch", "--show-current"]).stdout,
                    b"work\n"
                );
                println!(
                    "CAPABILITY Unicode/space {} collision: preserved",
                    if ignored { "ignored" } else { "untracked" }
                );
            }
        },
    );
}

#[test]
fn native_tracked_settings_line_endings_and_untracked_private_layers_are_preserved() {
    isolated(
        "native_tracked_settings_line_endings_and_untracked_private_layers_are_preserved",
        true,
        |root| {
            for eol in ["lf", "crlf"] {
                let f = fixture(root);
                f.local.success(&["config", "core.autocrlf", "false"]);
                commit(
                    &f.local,
                    ".gitattributes",
                    &format!("twigsmith.config.toml text eol={eol}\n"),
                );
                commit(
                    &f.local,
                    "twigsmith.config.toml",
                    "base='main'\nfinish-message='auto'\n",
                );
                // Force Git's configured checkout conversion, instead of merely
                // writing the expected bytes ourselves.
                f.local.success(&["switch", "main"]);
                f.local.success(&["switch", "work"]);
                let expected = fs::read(f.local.path().join("twigsmith.config.toml")).unwrap();
                assert_eq!(expected.contains(&b'\r'), eol == "crlf");
                let private = fs::read(f.local.path().join(".git/twigsmith.config.toml")).unwrap();
                assert!(finish(f.local.path()).status.success());
                assert_eq!(
                    fs::read(f.local.path().join("twigsmith.config.toml")).unwrap(),
                    expected
                );
                assert_eq!(
                    fs::read(f.local.path().join(".git/twigsmith.config.toml")).unwrap(),
                    private
                );
                assert!(
                    f.local
                        .success(&["status", "--porcelain"])
                        .stdout
                        .is_empty()
                );
                println!("CAPABILITY tracked settings eol={eol}: verified");
            }
            for ignored in [false, true] {
                let f = fixture(root);
                fs::write(
                    f.local.path().join("twigsmith.config.json"),
                    b"{\"finish-message\":\"auto\"}\n",
                )
                .unwrap();
                if ignored {
                    fs::write(
                        f.local.path().join(".git/info/exclude"),
                        "twigsmith.config.json\n",
                    )
                    .unwrap();
                }
                let shared = fs::read(f.local.path().join("twigsmith.config.json")).unwrap();
                let private = fs::read(f.local.path().join(".git/twigsmith.config.toml")).unwrap();
                // The unignored shared file is local work; ignored shared
                // settings may cross this checkout with their exact bytes.
                let output = finish(f.local.path());
                assert_eq!(output.status.success(), ignored, "{output:?}");
                assert_eq!(
                    fs::read(f.local.path().join("twigsmith.config.json")).unwrap(),
                    shared
                );
                assert_eq!(
                    fs::read(f.local.path().join(".git/twigsmith.config.toml")).unwrap(),
                    private
                );
            }
        },
    );
}

#[test]
fn native_unverifiable_settings_smudge_stops_before_integration_or_publication() {
    isolated(
        "native_unverifiable_settings_smudge_stops_before_integration_or_publication",
        true,
        |root| {
            let f = fixture(root);
            f.local.success(&["switch", "main"]);
            commit(
                &f.local,
                ".gitattributes",
                "twigsmith.config.toml filter=fixture\n",
            );
            commit(&f.local, "twigsmith.config.toml", "base='main'\n");
            f.local.success(&["push", "project", "main"]);
            f.local.success(&["switch", "work"]);
            // Git executes the filter only at its normal checkout boundary. The
            // recovery guard cannot explain these bytes as the exact tracked blob
            // or its built-in LF/CRLF conversion.
            f.local
                .success(&["config", "filter.fixture.smudge", "git --version"]);
            let original = oid(&f.local, "work");
            let target = oid(&f.local, "main");
            let private = fs::read(f.local.path().join(".git/twigsmith.config.toml")).unwrap();
            assert!(!finish(f.local.path()).status.success());
            assert_eq!(oid(&f.local, "work"), original);
            assert_eq!(oid(&f.local, "main"), target);
            assert_eq!(
                twigsmith::git::run(f.project.path(), ["rev-parse", "main"])
                    .unwrap()
                    .stdout,
                target
            );
            assert_eq!(
                fs::read(f.local.path().join(".git/twigsmith.config.toml")).unwrap(),
                private
            );
            assert_eq!(
                fs::read(f.local.path().join("twigsmith.config.toml")).unwrap(),
                f.local.success(&["--version"]).stdout
            );
            println!(
                "CAPABILITY unverifiable settings smudge: refused after checkout, before integration/publication"
            );
        },
    );
}

fn linked_directory(link: &Path, target: &Path, junction: bool) -> bool {
    #[cfg(unix)]
    {
        assert!(!junction);
        std::os::unix::fs::symlink(target, link).unwrap();
    }
    #[cfg(windows)]
    {
        let result = if junction {
            let output = Command::new("cmd")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            println!(
                "CAPABILITY junction creation: {} {}{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if output.status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other("mklink /J failed"))
            }
        } else {
            std::os::windows::fs::symlink_dir(target, link)
        };
        if let Err(error) = result {
            println!(
                "CAPABILITY {}: unavailable: {error}",
                if junction {
                    "junction"
                } else {
                    "directory symlink"
                }
            );
            return false;
        }
        use std::os::windows::fs::MetadataExt;
        let metadata = fs::symlink_metadata(link).unwrap();
        println!(
            "CAPABILITY reparse metadata: attributes={:#x}, is_symlink={}",
            metadata.file_attributes(),
            metadata.file_type().is_symlink()
        );
        assert_ne!(metadata.file_attributes() & 0x400, 0);
    }
    true
}
fn unlink_directory(path: &Path) {
    #[cfg(unix)]
    fs::remove_file(path).unwrap();
    #[cfg(windows)]
    fs::remove_dir(path).unwrap();
}

#[test]
fn native_recovery_storage_symlinks_and_windows_junctions_refuse() {
    isolated(
        "native_recovery_storage_symlinks_and_windows_junctions_refuse",
        true,
        |root| {
            let variants: &[bool] = if cfg!(windows) {
                &[false, true]
            } else {
                &[false]
            };
            for junction in variants {
                let f = fixture(root);
                let (_, path) = interrupted(&f);
                // Git reports canonical worktree identity; macOS temporary paths
                // can enter through /var while the same directory is /private/var.
                let namespace =
                    fs::canonicalize(f.local.path().join(".git/twigsmith-integration")).unwrap();
                let path = fs::canonicalize(&path).unwrap();
                let relative = path.strip_prefix(&namespace).unwrap();
                let target = namespace.parent().unwrap().join("saved recovery storage");
                fs::rename(&namespace, &target).unwrap();
                let bytes = fs::read(target.join(relative)).unwrap();
                if !linked_directory(&namespace, &target, *junction) {
                    continue;
                }
                let assessment = integration::assess(f.local.path(), &request(), None).unwrap();
                assert!(
                    assessment
                        .blockers
                        .contains(&Blocker::Recovery(Problem::UnsafeRecord)),
                    "redirected recovery supplied authority: {assessment:#?}"
                );
                let refs = f.local.success(&["show-ref"]).stdout;
                assert!(!finish(f.local.path()).status.success());
                assert_eq!(f.local.success(&["show-ref"]).stdout, refs);
                assert_eq!(fs::read(target.join(relative)).unwrap(), bytes);
                unlink_directory(&namespace);
                fs::rename(&target, &namespace).unwrap();
                assert!(
                    finish(f.local.path()).status.success(),
                    "restored evidence must remain usable"
                );
                println!(
                    "CAPABILITY {} recovery refusal and restored continuation: verified",
                    if *junction {
                        "junction"
                    } else {
                        "directory symlink"
                    }
                );
            }
        },
    );
}
