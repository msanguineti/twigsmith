//! Hosted preparation retries operate only on their retained version and publication refs.
mod support;

use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};
use twigsmith::{
    config::{self, Format},
    git,
    settings::{self, FinishCheck, Settings},
};

fn run(repo: &Repository, command: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_twigsmith"))
        .current_dir(repo.path())
        .arg(command)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn status(output: Output, expected: i32) -> String {
    assert_eq!(output.status.code(), Some(expected), "{output:?}");
    format!(
        "{}{}",
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap()
    )
}
fn value(repo: &Repository, args: &[&str]) -> String {
    String::from_utf8(repo.success(args).stdout)
        .unwrap()
        .trim()
        .into()
}
fn oid(path: &Path, reference: &str) -> Option<String> {
    let output = git::run(path, ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().into())
}
fn refs(path: &Path) -> Vec<u8> {
    let output = git::run(path, ["for-each-ref", "--format=%(refname) %(objectname)"]).unwrap();
    assert!(output.status.success());
    output.stdout
}
fn commit(repo: &Repository, filename: &str) -> String {
    fs::write(repo.path().join(filename), format!("{filename}\n")).unwrap();
    repo.success(&["add", filename]);
    repo.success(&["commit", "-m", filename]);
    value(repo, &["rev-parse", "HEAD"])
}
fn clone_bare(root: &Path, local: &Repository, source: &Path) -> tempfile::TempDir {
    let result = tempfile::tempdir_in(root).unwrap();
    local.success(&[
        "clone".as_ref(),
        "--bare".as_ref(),
        source.as_os_str(),
        result.path().as_os_str(),
    ]);
    result
}
#[cfg(unix)]
fn hook(path: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let hooks = path.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let result = hooks.join(name);
    fs::write(&result, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&result, fs::Permissions::from_mode(0o755)).unwrap();
    let output = git::run(
        path,
        [
            "config".as_ref(),
            "core.hooksPath".as_ref(),
            hooks.as_os_str(),
        ],
    )
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    result
}
#[cfg(unix)]
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}
struct Fixture {
    local: Repository,
    project: tempfile::TempDir,
    review: tempfile::TempDir,
    settings: Settings,
}
impl Fixture {
    fn new(root: &Path, checks: Vec<FinishCheck>) -> Self {
        let local = Repository::new(root);
        commit(&local, "base");
        let project = clone_bare(root, &local, local.path());
        let review = clone_bare(root, &local, local.path());
        for (name, remote) in [("project", &project), ("review", &review)] {
            local.success(&["remote", "add", name, remote.path().to_str().unwrap()]);
        }
        let mut settings = Settings {
            publish_remote: "review".into(),
            base_remote: "project".into(),
            finish_checks: checks,
            ..Settings::default()
        };
        settings::oneflow_hosted_bundle(&mut settings, Some("stable".into()));
        let result = Self {
            local,
            project,
            review,
            settings,
        };
        result.write_settings();
        result
    }
    fn write_settings(&self) {
        fs::write(
            self.local.path().join(".git/twigsmith.config.toml"),
            config::render(Format::Toml, &self.settings),
        )
        .unwrap();
    }
    fn start(&self) -> String {
        status(
            run(
                &self.local,
                "start",
                &["release/train", "--policy", "release"],
            ),
            0,
        );
        commit(&self.local, "version")
    }
    fn first(&self, extra: &[&str], expected: i32) -> String {
        let mut args = vec![
            "--tag",
            "v1",
            "--tag-message",
            "Approved version",
            "--merge-back",
            "review/v1",
        ];
        args.extend(extra);
        status(run(&self.local, "finish", &args), expected)
    }
    fn advance_target(&self, root: &Path) -> String {
        let writer = Repository::new(root);
        writer.success(&["fetch", self.project.path().to_str().unwrap(), "main"]);
        writer.success(&["reset", "--hard", "FETCH_HEAD"]);
        let result = commit(&writer, "development");
        writer.success(&[
            "push",
            self.project.path().to_str().unwrap(),
            "HEAD:refs/heads/main",
        ]);
        result
    }
}

#[cfg(unix)]
#[test]
fn hosted_partial_multiple_destination_retry_preserves_confirmed_effects() {
    isolated(
        "hosted_partial_multiple_destination_retry_preserves_confirmed_effects",
        true,
        |root| {
            let fixture = Fixture::new(root, vec![]);
            let project_second = clone_bare(root, &fixture.local, fixture.project.path());
            let review_second = clone_bare(root, &fixture.local, fixture.review.path());
            for (name, first, second) in [
                ("project", &fixture.project, &project_second),
                ("review", &fixture.review, &review_second),
            ] {
                fixture.local.success(&[
                    "config",
                    "--add",
                    &format!("remote.{name}.pushurl"),
                    first.path().to_str().unwrap(),
                ]);
                fixture.local.success(&[
                    "config",
                    "--add",
                    &format!("remote.{name}.pushurl"),
                    second.path().to_str().unwrap(),
                ]);
            }
            let released = fixture.start();
            let logs: Vec<_> = (0..4)
                .map(|index| root.join(format!("pushes-{index}")))
                .collect();
            for (destination, log) in [
                fixture.project.path(),
                project_second.path(),
                fixture.review.path(),
                review_second.path(),
            ]
            .into_iter()
            .zip(&logs)
            {
                hook(
                    destination,
                    "pre-receive",
                    &format!("cat >> {}; exit 0", quote(log)),
                );
            }
            let gate = root.join("reject-review");
            fs::write(&gate, "").unwrap();
            hook(
                review_second.path(),
                "pre-receive",
                &format!(
                    "cat >> {}; if test -f {}; then echo paused-review >&2; exit 1; fi",
                    quote(&logs[3]),
                    quote(&gate)
                ),
            );
            assert!(fixture.first(&[], 1).contains("paused-review"));
            let tag = oid(fixture.local.path(), "refs/tags/v1").unwrap();
            for project in [&fixture.project, &project_second] {
                assert_eq!(
                    oid(project.path(), "refs/heads/release/train"),
                    Some(released.clone())
                );
                assert_eq!(oid(project.path(), "refs/tags/v1"), Some(tag.clone()));
                assert_eq!(oid(project.path(), "refs/heads/stable"), None);
            }
            assert_eq!(
                oid(fixture.review.path(), "refs/heads/review/v1"),
                Some(released.clone())
            );
            assert_eq!(oid(review_second.path(), "refs/heads/review/v1"), None);
            let before: Vec<_> = logs.iter().map(|log| fs::read(log).unwrap()).collect();
            let tag_log = fs::read(fixture.local.path().join(".git/logs/refs/tags/v1")).unwrap();
            let back_log =
                fs::read(fixture.local.path().join(".git/logs/refs/heads/review/v1")).unwrap();
            fs::remove_file(gate).unwrap();
            assert!(
                status(run(&fixture.local, "finish", &[]), 0)
                    .contains("Ready for manual submission")
            );
            for index in 0..3 {
                assert_eq!(fs::read(&logs[index]).unwrap(), before[index]);
            }
            assert_eq!(fs::read_to_string(&logs[3]).unwrap().lines().count(), 2);
            assert_eq!(
                oid(review_second.path(), "refs/heads/review/v1"),
                Some(released.clone())
            );
            assert_eq!(
                fs::read(fixture.local.path().join(".git/logs/refs/tags/v1")).unwrap(),
                tag_log
            );
            assert_eq!(
                fs::read(fixture.local.path().join(".git/logs/refs/heads/review/v1")).unwrap(),
                back_log
            );
            let before: Vec<_> = logs.iter().map(|log| fs::read(log).unwrap()).collect();
            status(run(&fixture.local, "finish", &[]), 0);
            for (log, bytes) in logs.iter().zip(before) {
                assert_eq!(fs::read(log).unwrap(), bytes);
            }
            assert_eq!(
                oid(fixture.local.path(), "refs/heads/release/train"),
                Some(released)
            );
            assert_eq!(oid(fixture.local.path(), "refs/heads/stable"), None);
        },
    );
}

#[cfg(unix)]
#[test]
fn hosted_bypass_does_not_validate_source_or_distinct_candidate_on_retry() {
    isolated(
        "hosted_bypass_does_not_validate_source_or_distinct_candidate_on_retry",
        true,
        |root| {
            let fixture = Fixture::new(
                root,
                vec![FinishCheck {
                    name: "version and candidate gate".into(),
                    command: vec![
                        "sh".into(),
                        "-c".into(),
                        "git rev-parse HEAD >> .git/checked-trees; test -f .git/check-pass".into(),
                    ],
                }],
            );
            let released = fixture.start();
            fixture.advance_target(root);
            let reject = hook(
                fixture.review.path(),
                "pre-receive",
                "echo paused-review >&2; exit 1",
            );
            let output = fixture.first(&["--skip-checks"], 1);
            assert!(
                output.contains("Skipped 1 configured finish check"),
                "{output}"
            );
            let candidate = oid(fixture.local.path(), "refs/heads/review/v1").unwrap();
            assert_ne!(candidate, released);
            assert!(!fixture.local.path().join(".git/checked-trees").exists());
            let project_before = refs(fixture.project.path());
            fs::remove_file(reject).unwrap();
            status(run(&fixture.local, "finish", &[]), 1);
            assert_eq!(refs(fixture.project.path()), project_before);
            assert_eq!(
                fs::read_to_string(fixture.local.path().join(".git/checked-trees"))
                    .unwrap()
                    .lines()
                    .collect::<Vec<_>>(),
                [released.as_str()]
            );
            fs::write(fixture.local.path().join(".git/check-pass"), "").unwrap();
            status(run(&fixture.local, "finish", &[]), 0);
            let checked =
                fs::read_to_string(fixture.local.path().join(".git/checked-trees")).unwrap();
            assert_eq!(
                checked.lines().collect::<Vec<_>>(),
                [released.as_str(), released.as_str(), candidate.as_str()]
            );
            status(run(&fixture.local, "finish", &[]), 0);
            assert_eq!(
                fs::read_to_string(fixture.local.path().join(".git/checked-trees")).unwrap(),
                checked
            );
            assert_eq!(oid(fixture.project.path(), "v1^{}"), Some(released));
            assert_eq!(
                oid(fixture.review.path(), "refs/heads/review/v1"),
                Some(candidate)
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn hosted_signing_failure_preserves_unsigned_and_publication_boundary() {
    isolated(
        "hosted_signing_failure_preserves_unsigned_and_publication_boundary",
        true,
        |root| {
            for preference in [false, true] {
                let fixture = Fixture::new(root, vec![]);
                fixture
                    .local
                    .success(&["config", "gpg.program", "/missing-oneflow-hosted-signer"]);
                if preference {
                    fixture.local.success(&["config", "tag.gpgSign", "true"]);
                }
                let released = fixture.start();
                let project_before = refs(fixture.project.path());
                let review_before = refs(fixture.review.path());
                fixture.first(if preference { &[] } else { &["--sign-tag"] }, 1);
                assert_eq!(oid(fixture.local.path(), "refs/tags/v1"), None);
                assert_eq!(oid(fixture.local.path(), "refs/heads/review/v1"), None);
                assert_eq!(
                    oid(fixture.local.path(), "refs/heads/release/train"),
                    Some(released)
                );
                assert_eq!(refs(fixture.project.path()), project_before);
                assert_eq!(refs(fixture.review.path()), review_before);
                // Missing retry flags retain the exact signing request rather than falling back.
                status(run(&fixture.local, "finish", &[]), 1);
                assert_eq!(oid(fixture.local.path(), "refs/tags/v1"), None);
                assert_eq!(refs(fixture.project.path()), project_before);
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn hosted_retry_refuses_changed_instances_tag_objects_settings_and_endpoints() {
    isolated(
        "hosted_retry_refuses_changed_instances_tag_objects_settings_and_endpoints",
        true,
        |root| {
            for mutation in [
                "source",
                "merge-back",
                "tag-object",
                "settings",
                "endpoint",
                "missing-record",
                "worktree",
                "target-ref",
                "prepare-ref",
            ] {
                let mut fixture = Fixture::new(root, vec![]);
                let released = fixture.start();
                if mutation == "prepare-ref" {
                    fixture.advance_target(root);
                }
                let reject = hook(fixture.review.path(), "pre-receive", "exit 1");
                fixture.first(&[], 1);
                let tag = oid(fixture.local.path(), "refs/tags/v1").unwrap();
                fs::remove_file(reject).unwrap();
                match mutation {
                    "source" => {
                        fixture.local.success(&["branch", "-D", "release/train"]);
                        fixture
                            .local
                            .success(&["branch", "release/train", &released]);
                    }
                    "merge-back" => {
                        fixture.local.success(&["switch", "release/train"]);
                        fixture.local.success(&["branch", "-D", "review/v1"]);
                        fixture.local.success(&["branch", "review/v1", &released]);
                    }
                    "tag-object" => {
                        fixture.local.success(&["tag", "-d", "v1"]);
                        fixture.local.success(&[
                            "tag",
                            "-a",
                            "-m",
                            "different object at same commit",
                            "v1",
                            &released,
                        ]);
                        assert_ne!(oid(fixture.local.path(), "refs/tags/v1"), Some(tag));
                    }
                    "settings" => {
                        fixture
                            .settings
                            .workstreams
                            .get_mut(&settings::WorkstreamKind::Release)
                            .unwrap()
                            .finish_message = Some("ask".into());
                        fixture.write_settings();
                    }
                    "target-ref" | "prepare-ref" => {
                        let kind = if mutation == "target-ref" {
                            "/target"
                        } else {
                            "/prepare"
                        };
                        let pins = value(
                            &fixture.local,
                            &[
                                "for-each-ref",
                                "--format=%(refname)",
                                "refs/twigsmith/hosted/",
                            ],
                        );
                        let reference = pins
                            .lines()
                            .find(|reference| reference.ends_with(kind))
                            .unwrap();
                        fixture.local.success(&["update-ref", "-d", reference]);
                    }
                    "endpoint" => {
                        fixture.local.success(&[
                            "config",
                            "remote.review.pushurl",
                            fixture.project.path().to_str().unwrap(),
                        ]);
                    }
                    "missing-record" => {
                        let attempts = fixture.local.path().join(".git/twigsmith-hosted/attempts");
                        let record = fs::read_dir(attempts)
                            .unwrap()
                            .next()
                            .unwrap()
                            .unwrap()
                            .path();
                        fs::remove_file(record).unwrap();
                    }
                    "worktree" => {
                        fixture.local.success(&["switch", "release/train"]);
                        let worktree = tempfile::tempdir_in(root).unwrap();
                        fixture.local.success(&[
                            "worktree".as_ref(),
                            "add".as_ref(),
                            worktree.path().as_os_str(),
                            "review/v1".as_ref(),
                        ]);
                    }
                    _ => unreachable!(),
                }
                let local_before = refs(fixture.local.path());
                let project_before = refs(fixture.project.path());
                let review_before = refs(fixture.review.path());
                let output = status(run(&fixture.local, "finish", &[]), 1);
                assert!(
                    !output.contains("Ready for manual submission"),
                    "{mutation}: {output}"
                );
                assert_eq!(refs(fixture.local.path()), local_before, "{mutation}");
                assert_eq!(refs(fixture.project.path()), project_before, "{mutation}");
                assert_eq!(refs(fixture.review.path()), review_before, "{mutation}");
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn hosted_interrupted_tag_and_branch_creation_recover_exact_owned_objects() {
    use std::os::unix::process::ExitStatusExt;
    isolated(
        "hosted_interrupted_tag_and_branch_creation_recover_exact_owned_objects",
        true,
        |root| {
            for interrupted_ref in ["refs/tags/v1", "refs/heads/review/v1"] {
                let fixture = Fixture::new(root, vec![]);
                let released = fixture.start();
                let git_dir = fixture.local.path().join(".git");
                hook(
                    &git_dir,
                    "reference-transaction",
                    &format!(
                        "if test \"$1\" = committed; then\n  while read old new reference; do\n    if test \"$reference\" = {interrupted_ref}; then\n      parent=$(ps -o ppid= -p \"$PPID\" | tr -d ' ')\n      kill -KILL \"$parent\"\n    fi\n  done\nfi\nexit 0"
                    ),
                );
                let interrupted = run(
                    &fixture.local,
                    "finish",
                    &[
                        "--tag",
                        "v1",
                        "--tag-message",
                        "Approved version",
                        "--merge-back",
                        "review/v1",
                    ],
                );
                assert_eq!(
                    interrupted.status.signal(),
                    Some(9),
                    "{interrupted_ref}: {interrupted:?}"
                );
                let tag = oid(fixture.local.path(), "refs/tags/v1").unwrap();
                assert_eq!(oid(fixture.local.path(), "v1^{}"), Some(released.clone()));
                assert_eq!(oid(fixture.project.path(), "refs/tags/v1"), None);
                assert_eq!(oid(fixture.review.path(), "refs/heads/review/v1"), None);
                let tag_log = fs::read(git_dir.join("logs/refs/tags/v1")).unwrap();
                let back_log = fs::read(git_dir.join("logs/refs/heads/review/v1")).ok();
                // The interruption hook remains: any repeated creation kills this retry too.
                status(run(&fixture.local, "finish", &[]), 0);
                assert_eq!(oid(fixture.project.path(), "refs/tags/v1"), Some(tag));
                assert_eq!(
                    oid(fixture.review.path(), "refs/heads/review/v1"),
                    Some(released.clone())
                );
                assert_eq!(
                    fs::read(git_dir.join("logs/refs/tags/v1")).unwrap(),
                    tag_log
                );
                if let Some(back_log) = back_log {
                    assert_eq!(
                        fs::read(git_dir.join("logs/refs/heads/review/v1")).unwrap(),
                        back_log
                    );
                }
                assert_eq!(
                    oid(fixture.local.path(), "refs/heads/release/train"),
                    Some(released)
                );
            }
        },
    );
}

#[test]
fn hosted_missing_recovery_evidence_cannot_fall_through_to_ordinary_or_direct_finish() {
    isolated(
        "hosted_missing_recovery_evidence_cannot_fall_through_to_ordinary_or_direct_finish",
        true,
        |root| {
            for missing in ["association", "record"] {
                let mut fixture = Fixture::new(root, vec![]);
                fixture.settings.finish_mode = settings::FinishMode::Integrate;
                fixture.write_settings();
                let released = fixture.start();
                fixture.first(&[], 0);
                let git_dir = fixture.local.path().join(".git/twigsmith-hosted");
                let directory = git_dir.join(if missing == "association" {
                    "associations"
                } else {
                    "attempts"
                });
                let branch = if missing == "association" {
                    "review/v1"
                } else {
                    "release/train"
                };
                let filename: String = branch.bytes().map(|byte| format!("{byte:02x}")).collect();
                let selected = directory.join(filename);
                fs::remove_file(selected).unwrap();
                let local_before = refs(fixture.local.path());
                let project_before = refs(fixture.project.path());
                let review_before = refs(fixture.review.path());
                for args in [
                    &[][..],
                    &["--mode", "review", "--update", "none", "--no-rewrite"],
                    &[
                        "--mode",
                        "integrate",
                        "--integration",
                        "merge",
                        "--cleanup",
                        "both",
                    ],
                ] {
                    status(run(&fixture.local, "finish", args), 1);
                    assert_eq!(
                        refs(fixture.local.path()),
                        local_before,
                        "missing {missing}"
                    );
                    assert_eq!(
                        refs(fixture.project.path()),
                        project_before,
                        "missing {missing}"
                    );
                    assert_eq!(
                        refs(fixture.review.path()),
                        review_before,
                        "missing {missing}"
                    );
                }
                assert_eq!(
                    oid(fixture.local.path(), "refs/heads/release/train"),
                    Some(released)
                );
                // Evidence on a different workstream must leave an unrelated ordinary contribution usable.
                status(
                    run(
                        &fixture.local,
                        "start",
                        &["feature/unrelated", "--policy", "ordinary"],
                    ),
                    0,
                );
                commit(&fixture.local, "ordinary");
                let output = status(
                    run(
                        &fixture.local,
                        "finish",
                        &["--update", "none", "--no-rewrite"],
                    ),
                    0,
                );
                assert!(output.contains("Published feature/unrelated"), "{output}");
                assert_eq!(
                    oid(fixture.review.path(), "refs/heads/feature/unrelated"),
                    oid(fixture.local.path(), "refs/heads/feature/unrelated")
                );
            }
        },
    );
}

#[test]
fn hosted_original_work_tag_and_review_destinations_resolve_independently() {
    isolated(
        "hosted_original_work_tag_and_review_destinations_resolve_independently",
        true,
        |root| {
            let mut fixture = Fixture::new(root, vec![]);
            let work = clone_bare(root, &fixture.local, fixture.project.path());
            fixture
                .local
                .success(&["remote", "add", "work", work.path().to_str().unwrap()]);
            fixture
                .settings
                .workstreams
                .get_mut(&settings::WorkstreamKind::Release)
                .unwrap()
                .publish_remote = Some("work".into());
            fixture.write_settings();
            let released = fixture.start();
            status(run(&fixture.local, "save", &[]), 0);
            assert_eq!(
                oid(work.path(), "refs/heads/release/train"),
                Some(released.clone())
            );
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/release/train"),
                None
            );
            assert_eq!(oid(fixture.review.path(), "refs/heads/release/train"), None);
            let output = fixture.first(&[], 0);
            assert!(
                output.contains("Original source release/train published to work"),
                "{output}"
            );
            assert!(
                output.contains("version tag published to project"),
                "{output}"
            );
            assert!(
                output.contains("from review remote review into project/main"),
                "{output}"
            );
            let tag = oid(fixture.local.path(), "refs/tags/v1").unwrap();
            assert_eq!(oid(fixture.project.path(), "refs/tags/v1"), Some(tag));
            assert_eq!(oid(fixture.project.path(), "v1^{}"), Some(released.clone()));
            assert_eq!(
                oid(fixture.review.path(), "refs/heads/review/v1"),
                Some(released.clone())
            );
            assert_eq!(oid(work.path(), "refs/tags/v1"), None);
            assert_eq!(oid(work.path(), "refs/heads/review/v1"), None);
            assert_eq!(oid(fixture.review.path(), "refs/tags/v1"), None);
            assert_eq!(oid(fixture.project.path(), "refs/heads/review/v1"), None);
            assert_eq!(
                oid(fixture.local.path(), "refs/heads/release/train"),
                Some(released)
            );
        },
    );
}
