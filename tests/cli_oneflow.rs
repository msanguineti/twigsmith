//! Public lifecycle wiring and release/hotfix safety against isolated real Git repositories.
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;

use git_oneflow::git;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};

#[derive(Clone, Copy, Debug)]
struct Cli {
    executable: &'static str,
    aliases: bool,
}

impl Cli {
    fn run(self, repository: &Repository, command: &str, arguments: &[&str]) -> Output {
        let command = if self.aliases {
            match command {
                "start" => "s",
                "save" => "v",
                "finish" => "f",
                other => other,
            }
        } else {
            command
        };
        Command::new(self.executable)
            .current_dir(repository.path())
            .arg(command)
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn success(self, repository: &Repository, command: &str, arguments: &[&str]) -> String {
        let output = self.run(repository, command, arguments);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{self:?} {command} {arguments:?} in {}: {output:?}",
            repository.path().display()
        );
        format!(
            "{}{}",
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap()
        )
    }
}

// Safety scenarios use the shared dispatcher once; the focused lifecycle matrix
// below verifies both binaries and the three lifecycle aliases with real effects.
fn cases(name: &str, test: impl Fn(&Path, Cli)) {
    isolated(name, true, |root| {
        let directory = tempfile::tempdir_in(root).unwrap();
        test(
            directory.path(),
            Cli {
                executable: env!("CARGO_BIN_EXE_git-oneflow"),
                aliases: false,
            },
        );
    });
}

fn entrypoint_cases(name: &str, test: impl Fn(&Path, Cli)) {
    isolated(name, true, |root| {
        for executable in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
            for aliases in [false, true] {
                let directory = tempfile::tempdir_in(root).unwrap();
                println!("lifecycle wiring: executable={executable}, aliases={aliases}");
                test(
                    directory.path(),
                    Cli {
                        executable,
                        aliases,
                    },
                );
            }
        }
    });
}

fn status(output: Output, expected: i32) -> String {
    assert_eq!(output.status.code(), Some(expected), "{output:?}");
    format!(
        "{}{}",
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap()
    )
}

fn oid(path: &Path, reference: &str) -> Option<String> {
    let output = git::run(path, ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().into())
}

fn value(repository: &Repository, arguments: &[&str]) -> String {
    String::from_utf8(repository.success(arguments).stdout)
        .unwrap()
        .trim()
        .into()
}

fn commit(repository: &Repository, path: &str, contents: &str) -> String {
    fs::write(repository.path().join(path), contents).unwrap();
    repository.success(&["add", path]);
    repository.success(&["commit", "-m", path]);
    value(repository, &["rev-parse", "HEAD"])
}

struct Fixture {
    local: Repository,
    project: tempfile::TempDir,
    personal: tempfile::TempDir,
    base: String,
}

impl Fixture {
    fn new(root: &Path) -> Self {
        let local = Repository::new(root);
        let base = commit(&local, "base", "initial\n");
        let project = clone_bare(root, &local, local.path());
        let personal = clone_bare(root, &local, local.path());
        for (name, destination) in [("project", &project), ("personal", &personal)] {
            local.success(&["remote", "add", name, destination.path().to_str().unwrap()]);
        }
        Self {
            local,
            project,
            personal,
            base,
        }
    }

    fn advance_development(&self, root: &Path, path: &str, contents: &str) -> String {
        let upstream = Repository::new(root);
        upstream.success(&["fetch", self.project.path().to_str().unwrap(), "main"]);
        upstream.success(&["reset", "--hard", "FETCH_HEAD"]);
        let result = commit(&upstream, path, contents);
        upstream.success(&[
            "push",
            self.project.path().to_str().unwrap(),
            "HEAD:refs/heads/main",
        ]);
        result
    }
}

fn clone_bare(root: &Path, repository: &Repository, source: &Path) -> tempfile::TempDir {
    let destination = tempfile::tempdir_in(root).unwrap();
    repository.success(&[
        "clone".as_ref(),
        "--bare".as_ref(),
        source.as_os_str(),
        destination.path().as_os_str(),
    ]);
    destination
}

#[cfg(unix)]
fn hook(path: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let hooks = path.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let result = hooks.join(name);
    fs::write(&result, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&result, fs::Permissions::from_mode(0o755)).unwrap();
    result
}

fn configure(fixture: &Fixture, marker: &str, active_release: &str, work_remote: &str) {
    fs::write(
        fixture.local.path().join(".git/git-oneflow.config.toml"),
        format!(
            "publish-remote='personal'\nbase-remote='project'\nfinish-message='auto'\n\
             [workstreams.ordinary]\nversion-tagging='disabled'\nfinish-mode='integrate'\n\
             finish-update='merge'\nfinish-integration='merge'\nfinish-cleanup='both'\n\
             [workstreams.release]\ncreation-source='main'\npreparation-source='main'\n\
             integration-target='main'\npublish-remote='{work_remote}'\nbase-remote='project'\n\
             version-tagging='required'\nrelease-marker='{marker}'\nfinish-mode='integrate'\n\
             finish-update='none'\nfinish-integration='merge'\nfinish-cleanup='both'\n\
             finish-rewrite='deny'\nfinish-message='auto'\n\
             [workstreams.hotfix]\npreparation-source='main'\nintegration-target='main'\n\
             publish-remote='{work_remote}'\nbase-remote='project'\nversion-tagging='required'\n\
             active-release='{active_release}'\nrelease-marker='{marker}'\nfinish-mode='integrate'\n\
             finish-update='none'\nfinish-integration='merge'\nfinish-cleanup='both'\n\
             finish-rewrite='deny'\nfinish-message='auto'\n"
        ),
    )
    .unwrap();
}

fn start_release(cli: Cli, fixture: &Fixture, branch: &str) {
    cli.success(
        &fixture.local,
        "start",
        &[branch, "--policy", "release", "--from", &fixture.base],
    );
}

fn refs(path: &Path) -> String {
    String::from_utf8(
        git::run(
            path,
            [
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
                "refs/tags",
            ],
        )
        .unwrap()
        .stdout,
    )
    .unwrap()
}

#[test]
fn ordinary_lifecycle_executable_and_alias_wiring() {
    entrypoint_cases(
        "ordinary_lifecycle_executable_and_alias_wiring",
        |root, cli| {
            let fixture = Fixture::new(root);
            fs::write(
                fixture.local.path().join(".git/git-oneflow.config.toml"),
                "publish-remote='personal'\nbase-remote='project'\nfinish-mode='review'\n\
             finish-update='none'\nfinish-integration='ff-only'\nfinish-message='auto'\n\
             finish-checkout='work'\nfinish-cleanup='keep'\n",
            )
            .unwrap();
            // The explicit source differs from both local and published main.
            let source = commit(&fixture.local, "source", "explicit starting point\n");
            fixture.local.success(&["branch", "source-input"]);
            fixture.local.success(&["reset", "--hard", &fixture.base]);
            let branch = "work/wiring";
            let work_ref = "refs/heads/work/wiring";
            cli.success(
                &fixture.local,
                "start",
                &[branch, "--policy", "ordinary", "--from", "source-input"],
            );
            assert_eq!(oid(fixture.local.path(), "HEAD"), Some(source.clone()));
            assert_eq!(value(&fixture.local, &["branch", "--show-current"]), branch);
            assert_eq!(oid(fixture.personal.path(), work_ref), None);

            fs::write(fixture.local.path().join("work"), "saved through the CLI\n").unwrap();
            fixture.local.success(&["add", "work"]);
            cli.success(
                &fixture.local,
                "save",
                &["--staged", "--message", "Save wiring progress"],
            );
            let saved = oid(fixture.local.path(), "HEAD").unwrap();
            assert_ne!(saved, source);
            assert_eq!(oid(fixture.local.path(), "HEAD^"), Some(source.clone()));
            assert_eq!(
                value(&fixture.local, &["log", "-1", "--format=%B"]),
                "Save wiring progress"
            );
            assert_eq!(
                value(&fixture.local, &["show", "HEAD:work"]),
                "saved through the CLI"
            );
            assert_eq!(oid(fixture.personal.path(), work_ref), Some(saved.clone()));
            assert_eq!(oid(fixture.project.path(), work_ref), None);
            assert_eq!(
                value(&fixture.local, &["config", "branch.work/wiring.remote"]),
                "personal"
            );
            assert_eq!(
                value(&fixture.local, &["config", "branch.work/wiring.merge"]),
                work_ref
            );
            assert!(
                fixture
                    .local
                    .success(&["status", "--porcelain"])
                    .stdout
                    .is_empty()
            );

            // Explicit finish options override review, ff-only, work, and keep.
            cli.success(
                &fixture.local,
                "finish",
                &[
                    "--mode",
                    "integrate",
                    "--update",
                    "none",
                    "--integration",
                    "merge",
                    "-m",
                    "Integrate wiring progress",
                    "--checkout",
                    "target",
                    "--cleanup",
                    "both",
                ],
            );
            let result = oid(fixture.local.path(), "HEAD").unwrap();
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/main"),
                Some(result.clone())
            );
            assert_eq!(
                value(&fixture.local, &["rev-list", "--parents", "-1", "HEAD"]),
                format!("{result} {} {saved}", fixture.base)
            );
            assert_eq!(
                value(&fixture.local, &["log", "-1", "--format=%B"]),
                "Integrate wiring progress"
            );
            assert_eq!(value(&fixture.local, &["branch", "--show-current"]), "main");
            assert_eq!(
                fs::read_to_string(fixture.local.path().join("work")).unwrap(),
                "saved through the CLI\n"
            );
            assert_eq!(oid(fixture.local.path(), work_ref), None);
            assert_eq!(oid(fixture.personal.path(), work_ref), None);
            assert_eq!(oid(fixture.project.path(), work_ref), None);
            assert_eq!(
                oid(fixture.personal.path(), "refs/heads/main"),
                Some(fixture.base.clone())
            );
            assert_eq!(
                oid(fixture.local.path(), "refs/heads/source-input"),
                Some(source)
            );
            assert!(fixture.local.success(&["tag", "--list"]).stdout.is_empty());
        },
    );
}

#[test]
fn earlier_release_preserves_version_marker_messages_and_unrelated_refs() {
    cases(
        "earlier_release_preserves_version_marker_messages_and_unrelated_refs",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "released", "none", "project");
            let newer = fixture.advance_development(root, "development", "new development\n");
            fixture
                .local
                .success(&["branch", "unrelated", &fixture.base]);
            fixture.local.success(&[
                "tag",
                "-a",
                "unrelated-version",
                "-m",
                "Unrelated identity",
                &fixture.base,
            ]);
            fixture
                .local
                .success(&["config", "push.followTags", "true"]);
            fixture
                .local
                .success(&["config", "branch.unrelated.remote", "personal"]);
            fixture
                .local
                .success(&["config", "branch.unrelated.merge", "refs/heads/unrelated"]);
            let personal_refs = refs(fixture.personal.path());
            start_release(cli, &fixture, "release/train");
            assert_eq!(
                oid(fixture.local.path(), "HEAD"),
                Some(fixture.base.clone())
            );
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/release/train"),
                None
            );
            let released = commit(&fixture.local, "release", "approved release\n");
            cli.success(&fixture.local, "save", &[]);
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/release/train"),
                Some(released.clone())
            );
            let tag_message = "Version Train R5\n\nLiteral ; $HOME `text`";
            let integration_message = "Integrate Train R5\n\nDifferent literal ; $HOME `text`";
            let report = cli.success(
                &fixture.local,
                "finish",
                &[
                    "--tag",
                    "Train-R5",
                    "--tag-message",
                    tag_message,
                    "-m",
                    integration_message,
                ],
            );
            assert!(report.contains("Train-R5"), "{report}");
            let result = oid(fixture.project.path(), "main").unwrap();
            assert_ne!(result, released);
            assert_ne!(result, newer);
            assert_eq!(
                oid(fixture.project.path(), "Train-R5^{}"),
                Some(released.clone())
            );
            assert_eq!(
                oid(fixture.project.path(), "released"),
                Some(released.clone())
            );
            assert_eq!(
                oid(fixture.local.path(), "Train-R5^{}"),
                Some(released.clone())
            );
            assert_eq!(
                value(&fixture.local, &["cat-file", "-t", "Train-R5"]),
                "tag"
            );
            assert_eq!(
                value(
                    &fixture.local,
                    &["for-each-ref", "--format=%(contents)", "refs/tags/Train-R5"]
                ),
                tag_message
            );
            assert_eq!(
                value(&fixture.local, &["log", "-1", "--format=%B"]),
                integration_message
            );
            fixture
                .local
                .success(&["merge-base", "--is-ancestor", &released, &result]);
            fixture
                .local
                .success(&["merge-base", "--is-ancestor", &newer, &result]);
            assert_eq!(
                value(&fixture.local, &["symbolic-ref", "--short", "HEAD"]),
                "main"
            );
            assert_eq!(oid(fixture.local.path(), "refs/heads/release/train"), None);
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/release/train"),
                None
            );
            assert_eq!(refs(fixture.personal.path()), personal_refs);
            assert_eq!(
                oid(fixture.local.path(), "unrelated"),
                Some(fixture.base.clone())
            );
            assert_eq!(
                oid(fixture.local.path(), "unrelated-version^{}"),
                Some(fixture.base.clone())
            );
            assert_eq!(oid(fixture.project.path(), "refs/heads/unrelated"), None);
            assert_eq!(
                oid(fixture.project.path(), "refs/tags/unrelated-version"),
                None
            );
            assert_eq!(
                value(&fixture.local, &["config", "branch.unrelated.remote"]),
                "personal"
            );
            assert_eq!(
                value(&fixture.local, &["config", "branch.unrelated.merge"]),
                "refs/heads/unrelated"
            );
            let delivered = refs(fixture.project.path());
            cli.success(&fixture.local, "finish", &[]);
            assert_eq!(refs(fixture.project.path()), delivered);
        },
    );
}

#[test]
fn ordinary_contribution_into_release_preserves_release_policy_without_tagging() {
    cases(
        "ordinary_contribution_into_release_preserves_release_policy_without_tagging",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "released", "none", "project");
            start_release(cli, &fixture, "release/train");
            commit(&fixture.local, "release", "release candidate\n");
            cli.success(&fixture.local, "save", &[]);
            let tags = value(
                &fixture.local,
                &["for-each-ref", "--format=%(refname)", "refs/tags"],
            );
            cli.success(
                &fixture.local,
                "start",
                &[
                    "work/release-fix",
                    "--policy",
                    "ordinary",
                    "--target",
                    "release/train",
                ],
            );
            let contribution = commit(&fixture.local, "contribution", "release contribution\n");
            cli.success(&fixture.local, "save", &[]);
            assert_eq!(
                oid(fixture.personal.path(), "refs/heads/work/release-fix"),
                Some(contribution.clone())
            );
            cli.success(&fixture.local, "finish", &[]);
            let release_result = oid(fixture.project.path(), "refs/heads/release/train").unwrap();
            fixture.local.success(&[
                "merge-base",
                "--is-ancestor",
                &contribution,
                &release_result,
            ]);
            assert_eq!(
                oid(fixture.project.path(), "main"),
                Some(fixture.base.clone())
            );
            assert_eq!(oid(fixture.project.path(), "released"), None);
            assert_eq!(
                value(
                    &fixture.local,
                    &["for-each-ref", "--format=%(refname)", "refs/tags"]
                ),
                tags
            );
            assert_eq!(
                value(&fixture.local, &["symbolic-ref", "--short", "HEAD"]),
                "release/train"
            );
            assert_eq!(
                oid(fixture.personal.path(), "refs/heads/work/release-fix"),
                None
            );
            // The target's release policy survives contribution completion and resumes.
            cli.success(
                &fixture.local,
                "start",
                &["release/train", "--policy", "release"],
            );
            cli.success(
                &fixture.local,
                "finish",
                &["--tag", "release-with-contribution"],
            );
            assert_eq!(
                oid(fixture.project.path(), "release-with-contribution^{}"),
                Some(release_result.clone())
            );
            assert_eq!(
                oid(fixture.project.path(), "released"),
                Some(release_result)
            );
        },
    );
}

#[test]
fn recreated_same_oid_release_cannot_inherit_policy_or_completion_authority() {
    cases(
        "recreated_same_oid_release_cannot_inherit_policy_or_completion_authority",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "none", "none", "project");
            start_release(cli, &fixture, "release/train");
            let released = commit(&fixture.local, "release", "release candidate\n");
            cli.success(&fixture.local, "save", &[]);
            cli.success(&fixture.local, "finish", &["--tag", "train-one"]);
            fixture
                .local
                .success(&["branch", "release/train", &released]);
            fixture.local.success(&["switch", "release/train"]);
            let project = refs(fixture.project.path());
            let local = refs(fixture.local.path());
            status(
                cli.run(&fixture.local, "finish", &["--tag", "train-two"]),
                1,
            );
            assert_eq!(refs(fixture.project.path()), project);
            assert_eq!(refs(fixture.local.path()), local);
            assert_eq!(oid(fixture.local.path(), "release/train"), Some(released));
            status(
                cli.run(
                    &fixture.local,
                    "start",
                    &["release/train", "--policy", "release"],
                ),
                1,
            );
            assert_eq!(refs(fixture.local.path()), local);
        },
    );
}

#[test]
fn annotated_tag_collision_compares_objects_even_at_same_released_commit() {
    cases(
        "annotated_tag_collision_compares_objects_even_at_same_released_commit",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "none", "none", "project");
            start_release(cli, &fixture, "release/train");
            let released = commit(&fixture.local, "release", "release candidate\n");
            cli.success(&fixture.local, "save", &[]);
            fixture
                .local
                .success(&["tag", "-a", "collision", "-m", "Server version identity"]);
            let remote_tag = oid(fixture.local.path(), "refs/tags/collision").unwrap();
            fixture
                .local
                .success(&["push", "project", "refs/tags/collision"]);
            fixture.local.success(&["tag", "-d", "collision"]);
            let before = refs(fixture.project.path());
            status(
                cli.run(
                    &fixture.local,
                    "finish",
                    &[
                        "--tag",
                        "collision",
                        "--tag-message",
                        "Requested version identity",
                    ],
                ),
                1,
            );
            assert_eq!(
                oid(fixture.project.path(), "refs/tags/collision"),
                Some(remote_tag)
            );
            assert_eq!(
                oid(fixture.project.path(), "collision^{}"),
                Some(released.clone())
            );
            assert_eq!(refs(fixture.project.path()), before);
            assert_eq!(oid(fixture.local.path(), "release/train"), Some(released));
        },
    );
}

#[test]
fn hotfix_explicit_tag_routes_to_declared_release_or_development() {
    cases(
        "hotfix_explicit_tag_routes_to_declared_release_or_development",
        |root, cli| {
            for annotated in [false, true] {
                for active in [false, true] {
                    let fixture = Fixture::new(root);
                    configure(
                        &fixture,
                        "released",
                        if active { "release/train" } else { "none" },
                        "project",
                    );
                    if annotated {
                        fixture.local.success(&[
                            "tag",
                            "-a",
                            "production-previous",
                            "-m",
                            "Previous version",
                        ]);
                    } else {
                        fixture.local.success(&["tag", "production-previous"]);
                    }
                    fixture
                        .local
                        .success(&["push", "project", "refs/tags/production-previous"]);
                    let newer = fixture.advance_development(root, "development", "not in hotfix\n");
                    if active {
                        start_release(cli, &fixture, "release/train");
                        commit(&fixture.local, "release", "active release\n");
                        cli.success(&fixture.local, "save", &[]);
                    }
                    cli.success(
                        &fixture.local,
                        "start",
                        &[
                            "hotfix/production",
                            "--policy",
                            "hotfix",
                            "--from-tag",
                            "production-previous",
                        ],
                    );
                    assert_eq!(
                        oid(fixture.local.path(), "HEAD"),
                        Some(fixture.base.clone())
                    );
                    assert!(!fixture.local.path().join("development").exists());
                    let hotfix = commit(&fixture.local, "fix", "production correction\n");
                    cli.success(&fixture.local, "save", &[]);
                    cli.success(&fixture.local, "finish", &["--tag", "production-corrected"]);
                    assert_eq!(
                        oid(fixture.project.path(), "production-corrected^{}"),
                        Some(hotfix.clone())
                    );
                    assert_eq!(
                        oid(fixture.project.path(), "released"),
                        Some(hotfix.clone())
                    );
                    assert_eq!(
                        oid(fixture.project.path(), "refs/heads/hotfix/production"),
                        None
                    );
                    let target = if active { "release/train" } else { "main" };
                    assert_eq!(
                        value(&fixture.local, &["symbolic-ref", "--short", "HEAD"]),
                        target
                    );
                    let integrated = oid(fixture.project.path(), target).unwrap();
                    fixture
                        .local
                        .success(&["merge-base", "--is-ancestor", &hotfix, &integrated]);
                    assert!(
                        !git::run(
                            fixture.local.path(),
                            ["merge-base", "--is-ancestor", &newer, &hotfix]
                        )
                        .unwrap()
                        .status
                        .success()
                    );
                    if active {
                        assert_eq!(oid(fixture.project.path(), "main"), Some(newer.clone()));
                        cli.success(
                            &fixture.local,
                            "start",
                            &["release/train", "--policy", "release"],
                        );
                        cli.success(&fixture.local, "finish", &["--tag", "train-after-hotfix"]);
                        let development = oid(fixture.project.path(), "main").unwrap();
                        fixture.local.success(&[
                            "merge-base",
                            "--is-ancestor",
                            &hotfix,
                            &development,
                        ]);
                        fixture.local.success(&[
                            "merge-base",
                            "--is-ancestor",
                            &newer,
                            &development,
                        ]);
                        assert_eq!(
                            oid(fixture.project.path(), "train-after-hotfix^{}"),
                            Some(integrated.clone())
                        );
                        assert_eq!(oid(fixture.project.path(), "released"), Some(integrated));
                    }
                }
            }
        },
    );
}

#[test]
fn explicit_creation_and_hotfix_context_errors_preserve_refs() {
    cases(
        "explicit_creation_and_hotfix_context_errors_preserve_refs",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "none", "none", "project");
            fixture
                .local
                .success(&["branch", "ambiguous", &fixture.base]);
            fixture.local.success(&["tag", "ambiguous", &fixture.base]);
            let before = refs(fixture.local.path());
            for arguments in [
                vec!["hotfix/production", "--policy", "hotfix"],
                vec![
                    "hotfix/production",
                    "--policy",
                    "hotfix",
                    "--from-tag",
                    "missing",
                ],
                vec![
                    "release/train",
                    "--policy",
                    "release",
                    "--from",
                    "HEAD:base",
                ],
                vec![
                    "release/train",
                    "--policy",
                    "release",
                    "--from",
                    "ambiguous",
                ],
            ] {
                status(cli.run(&fixture.local, "start", &arguments), 1);
                assert_eq!(refs(fixture.local.path()), before);
            }
            start_release(cli, &fixture, "release/train");
            let tip = commit(&fixture.local, "release", "release candidate\n");
            let before = refs(fixture.local.path());
            status(
                cli.run(
                    &fixture.local,
                    "start",
                    &["release/train", "--policy", "release", "--from", &tip],
                ),
                1,
            );
            assert_eq!(refs(fixture.local.path()), before);
            fixture.local.success(&["tag", "previous", &fixture.base]);
            configure(&fixture, "none", "release/missing", "project");
            cli.success(
                &fixture.local,
                "start",
                &[
                    "hotfix/production",
                    "--policy",
                    "hotfix",
                    "--from-tag",
                    "previous",
                ],
            );
            commit(&fixture.local, "hotfix", "production correction\n");
            let before = refs(fixture.project.path());
            status(
                cli.run(
                    &fixture.local,
                    "finish",
                    &["--tag", "invalid-route-version"],
                ),
                1,
            );
            assert_eq!(refs(fixture.project.path()), before);
        },
    );
}

#[test]
#[cfg(unix)]
fn tag_signing_failure_never_delivers_unsigned_version() {
    cases(
        "tag_signing_failure_never_delivers_unsigned_version",
        |root, cli| {
            for preference in [
                "explicit",
                "gpgSign-true",
                "gpgSign-yes",
                "forceSignAnnotated",
            ] {
                let fixture = Fixture::new(root);
                configure(&fixture, "none", "none", "project");
                start_release(cli, &fixture, "release/train");
                let released = commit(&fixture.local, "release", "release candidate\n");
                fixture
                    .local
                    .success(&["config", "gpg.program", "/missing-oneflow-signer"]);
                if preference.starts_with("gpgSign-") {
                    fixture.local.success(&[
                        "config",
                        "tag.gpgSign",
                        preference.strip_prefix("gpgSign-").unwrap(),
                    ]);
                } else if preference == "forceSignAnnotated" {
                    fixture
                        .local
                        .success(&["config", "tag.forceSignAnnotated", "yes"]);
                }
                let before = refs(fixture.project.path());
                let arguments = if preference == "forceSignAnnotated" {
                    vec!["--tag", "signed-version", "--tag-message", "Signed release"]
                } else if preference.starts_with("gpgSign-") {
                    // Git upgrades this lightweight request according to its
                    // effective signing preference.
                    vec!["--tag", "signed-version"]
                } else {
                    vec!["--tag", "signed-version", "--sign-tag"]
                };
                status(cli.run(&fixture.local, "finish", &arguments), 1);
                assert_eq!(oid(fixture.local.path(), "refs/tags/signed-version"), None);
                assert_eq!(refs(fixture.project.path()), before);
                assert_eq!(oid(fixture.local.path(), "release/train"), Some(released));
            }
            let fixture = Fixture::new(root);
            configure(&fixture, "none", "none", "project");
            start_release(cli, &fixture, "release/train");
            let released = commit(&fixture.local, "release", "release candidate\n");
            fixture
                .local
                .success(&["config", "gpg.program", "/missing-oneflow-signer"]);
            for key in ["tag.gpgSign", "tag.forceSignAnnotated"] {
                fixture.local.success(&["config", "--add", key, "yes"]);
                fixture.local.success(&["config", "--add", key, "no"]);
            }
            cli.success(
                &fixture.local,
                "finish",
                &[
                    "--tag",
                    "effective-unsigned",
                    "--tag-message",
                    "Unsigned by effective preference",
                ],
            );
            assert_eq!(
                oid(fixture.project.path(), "effective-unsigned^{}"),
                Some(released)
            );
            assert!(
                !value(&fixture.local, &["cat-file", "tag", "effective-unsigned"])
                    .contains("BEGIN PGP SIGNATURE")
            );
        },
    );
}

#[cfg(unix)]
fn server_hook(fixture: &Fixture, destination: &Path, body: &str) -> std::path::PathBuf {
    fixture.local.success(&[
        "-C",
        destination.to_str().unwrap(),
        "config",
        "core.hooksPath",
        destination.join("hooks").to_str().unwrap(),
    ]);
    hook(destination, "pre-receive", body)
}

#[test]
#[cfg(unix)]
fn partial_multidestination_release_delivery_and_source_deleted_cleanup_resume() {
    cases(
        "partial_multidestination_release_delivery_and_source_deleted_cleanup_resume",
        |root, cli| {
            for cleanup_failure in [false, true] {
                let fixture = Fixture::new(root);
                configure(&fixture, "released", "none", "personal");
                let primary = if cleanup_failure {
                    fixture.personal.path()
                } else {
                    fixture.project.path()
                };
                let remote = if cleanup_failure {
                    "personal"
                } else {
                    "project"
                };
                let second = clone_bare(root, &fixture.local, primary);
                for destination in [primary, second.path()] {
                    fixture.local.success(&[
                        "config",
                        "--add",
                        &format!("remote.{remote}.pushurl"),
                        destination.to_str().unwrap(),
                    ]);
                }
                start_release(cli, &fixture, "release/train");
                let released = commit(&fixture.local, "release", "release candidate\n");
                cli.success(&fixture.local, "save", &[]);
                let reject = server_hook(
                    &fixture,
                    second.path(),
                    if cleanup_failure {
                        "while read old new reference; do case $new in 0000000000000000000000000000000000000000) echo cleanup-blocked >&2; exit 1;; esac; done"
                    } else {
                        "echo delivery-blocked >&2; exit 1"
                    },
                );
                let report = status(
                    cli.run(&fixture.local, "finish", &["--tag", "partial-version"]),
                    1,
                );
                assert!(
                    report.contains(if cleanup_failure {
                        "cleanup-blocked"
                    } else {
                        "delivery-blocked"
                    }),
                    "{report}"
                );
                assert_eq!(
                    oid(fixture.local.path(), "partial-version^{}"),
                    Some(released.clone())
                );
                let result = oid(fixture.local.path(), "main").unwrap();
                if cleanup_failure {
                    assert_eq!(oid(fixture.local.path(), "release/train"), None);
                    assert_eq!(
                        oid(fixture.personal.path(), "refs/heads/release/train"),
                        None
                    );
                    assert_eq!(
                        oid(second.path(), "refs/heads/release/train"),
                        Some(released.clone())
                    );
                    assert_eq!(
                        oid(fixture.project.path(), "partial-version^{}"),
                        Some(released.clone())
                    );
                    // No confirmed project effect may be repeated while only cleanup remains.
                    server_hook(
                        &fixture,
                        fixture.project.path(),
                        "echo duplicate-delivery >&2; exit 1",
                    );
                } else {
                    assert_eq!(
                        oid(fixture.local.path(), "release/train"),
                        Some(released.clone())
                    );
                    assert_eq!(
                        oid(fixture.personal.path(), "refs/heads/release/train"),
                        Some(released.clone())
                    );
                    assert_eq!(oid(second.path(), "main"), Some(fixture.base.clone()));
                }
                fs::remove_file(reject).unwrap();
                cli.success(&fixture.local, "finish", &[]);
                assert_eq!(oid(fixture.project.path(), "main"), Some(result.clone()));
                assert_eq!(
                    oid(fixture.project.path(), "partial-version^{}"),
                    Some(released.clone())
                );
                assert_eq!(
                    oid(fixture.project.path(), "released"),
                    Some(released.clone())
                );
                assert_eq!(oid(fixture.local.path(), "release/train"), None);
                assert_eq!(
                    oid(fixture.personal.path(), "refs/heads/release/train"),
                    None
                );
                if cleanup_failure {
                    assert_eq!(oid(second.path(), "refs/heads/release/train"), None);
                } else {
                    assert_eq!(oid(second.path(), "main"), Some(result));
                    assert_eq!(
                        oid(second.path(), "partial-version^{}"),
                        Some(released.clone())
                    );
                    assert_eq!(oid(second.path(), "released"), Some(released));
                }
                cli.success(&fixture.local, "finish", &[]);
            }
        },
    );
}

#[test]
fn first_destination_conflict_preserves_selected_identity_through_native_resolution() {
    cases(
        "first_destination_conflict_preserves_selected_identity_through_native_resolution",
        |root, cli| {
            for identity in ["work-source", "integrated-result"] {
                let fixture = Fixture::new(root);
                configure(&fixture, "released", "none", "project");
                let config = fixture.local.path().join(".git/git-oneflow.config.toml");
                fs::write(
                    &config,
                    fs::read_to_string(&config).unwrap().replace(
                        "version-tagging='required'",
                        &format!("version-tagging='required'\nversion-tag-identity='{identity}'"),
                    ),
                )
                .unwrap();
                let newer = fixture.advance_development(root, "base", "development edits\n");
                start_release(cli, &fixture, "release/train");
                let released = commit(&fixture.local, "base", "release edits\n");
                cli.success(&fixture.local, "save", &[]);
                let report = status(
                    cli.run(&fixture.local, "finish", &["--tag", "conflicted-version"]),
                    1,
                );
                assert!(report.to_lowercase().contains("conflict"), "{report}");
                assert_eq!(
                    oid(fixture.local.path(), "conflicted-version^{}"),
                    (identity == "work-source").then(|| released.clone())
                );
                assert_eq!(oid(fixture.project.path(), "main"), Some(newer.clone()));
                assert!(fixture.local.path().join(".git/MERGE_HEAD").exists());
                fs::write(
                    fixture.local.path().join("base"),
                    "resolved development and release\n",
                )
                .unwrap();
                fixture.local.success(&["add", "base"]);
                fixture.local.success(&["commit", "--no-edit"]);
                let resolved = oid(fixture.local.path(), "HEAD").unwrap();
                assert_ne!(resolved, released);
                cli.success(&fixture.local, "finish", &[]);
                let version = if identity == "work-source" {
                    &released
                } else {
                    &resolved
                };
                assert_eq!(oid(fixture.project.path(), "main"), Some(resolved.clone()));
                assert_eq!(
                    oid(fixture.project.path(), "conflicted-version^{}"),
                    Some(version.clone())
                );
                assert_eq!(
                    oid(fixture.project.path(), "released"),
                    Some(version.clone())
                );
                fixture
                    .local
                    .success(&["merge-base", "--is-ancestor", &newer, &resolved]);
                fixture
                    .local
                    .success(&["merge-base", "--is-ancestor", &released, &resolved]);
            }
        },
    );
}

#[test]
#[cfg(unix)]
fn retry_rejects_changed_settings_and_same_commit_tag_object_substitution() {
    cases(
        "retry_rejects_changed_settings_and_same_commit_tag_object_substitution",
        |root, cli| {
            for context in ["settings", "tag-object", "marker", "endpoint"] {
                let fixture = Fixture::new(root);
                configure(
                    &fixture,
                    if context == "marker" {
                        "released"
                    } else {
                        "none"
                    },
                    "none",
                    "project",
                );
                if context == "marker" {
                    fixture
                        .local
                        .success(&["branch", "released", &fixture.base]);
                    fixture
                        .local
                        .success(&["push", "project", "refs/heads/released"]);
                }
                start_release(cli, &fixture, "release/train");
                let released = commit(&fixture.local, "release", "release candidate\n");
                cli.success(&fixture.local, "save", &[]);
                let reject = server_hook(
                    &fixture,
                    fixture.project.path(),
                    "echo paused-delivery >&2; exit 1",
                );
                status(
                    cli.run(
                        &fixture.local,
                        "finish",
                        &[
                            "--tag",
                            "retained-version",
                            "--tag-message",
                            "Approved identity",
                        ],
                    ),
                    1,
                );
                let original_tag = oid(fixture.local.path(), "refs/tags/retained-version").unwrap();
                fs::remove_file(reject).unwrap();
                if context == "settings" {
                    let private = fixture.local.path().join(".git/git-oneflow.config.toml");
                    let config = fs::read_to_string(&private).unwrap();
                    fs::write(
                        private,
                        config.replace("finish-message='auto'", "finish-message='ask'"),
                    )
                    .unwrap();
                } else if context == "tag-object" {
                    let other = Repository::new(root);
                    other.success(&[
                        "fetch",
                        "--no-tags",
                        fixture.project.path().to_str().unwrap(),
                        "refs/heads/release/train",
                    ]);
                    other.success(&["reset", "--hard", "FETCH_HEAD"]);
                    other.success(&[
                        "tag",
                        "-a",
                        "retained-version",
                        "-m",
                        "Substituted identity",
                        &released,
                    ]);
                    assert_ne!(
                        oid(other.path(), "refs/tags/retained-version"),
                        Some(original_tag.clone())
                    );
                    other.success(&[
                        "push",
                        fixture.project.path().to_str().unwrap(),
                        "refs/tags/retained-version",
                    ]);
                    // Only the remote identity changes; the original approved
                    // local object and its creation evidence remain untouched.
                    assert_eq!(
                        oid(fixture.local.path(), "refs/tags/retained-version"),
                        Some(original_tag)
                    );
                } else if context == "marker" {
                    let marker = oid(fixture.local.path(), "released").unwrap();
                    fixture.local.success(&["branch", "-D", "released"]);
                    fixture.local.success(&[
                        "update-ref",
                        "--create-reflog",
                        "-m",
                        "Recreated marker",
                        "refs/heads/released",
                        &marker,
                    ]);
                } else {
                    let different = clone_bare(root, &fixture.local, fixture.project.path());
                    fixture.local.success(&[
                        "config",
                        "remote.project.pushurl",
                        different.path().to_str().unwrap(),
                    ]);
                    let remote = refs(fixture.project.path());
                    let local = refs(fixture.local.path());
                    status(cli.run(&fixture.local, "finish", &[]), 1);
                    assert_eq!(refs(fixture.project.path()), remote);
                    assert_eq!(refs(fixture.local.path()), local);
                    continue;
                }
                let remote = refs(fixture.project.path());
                let local = refs(fixture.local.path());
                status(cli.run(&fixture.local, "finish", &[]), 1);
                assert_eq!(refs(fixture.project.path()), remote);
                assert_eq!(refs(fixture.local.path()), local);
                assert_eq!(oid(fixture.local.path(), "release/train"), Some(released));
            }
        },
    );
}

#[test]
#[cfg(unix)]
fn bypassed_release_checks_do_not_authorize_later_unbypassed_retry() {
    cases(
        "bypassed_release_checks_do_not_authorize_later_unbypassed_retry",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "none", "none", "project");
            let private = fixture.local.path().join(".git/git-oneflow.config.toml");
            let config = fs::read_to_string(&private).unwrap();
            // Insert the global check before workstream tables so this checks
            // both the approved released source and its integration candidate.
            let first_table = config.find("[workstreams.ordinary]").unwrap();
            let check = "[[finish-checks]]\nname='Release tree gate'\ncommand=['sh','-c','git rev-parse HEAD >> .git/checked-trees; test -f .git/check-pass']\n";
            fs::write(
                private,
                format!(
                    "{}{check}{}",
                    &config[..first_table],
                    &config[first_table..]
                ),
            )
            .unwrap();
            start_release(cli, &fixture, "release/train");
            let released = commit(&fixture.local, "release", "release candidate\n");
            let reject = server_hook(
                &fixture,
                fixture.project.path(),
                "echo paused-delivery >&2; exit 1",
            );
            status(
                cli.run(
                    &fixture.local,
                    "finish",
                    &["--tag", "bypassed-version", "--skip-checks"],
                ),
                1,
            );
            assert!(!fixture.local.path().join(".git/checked-trees").exists());
            fs::remove_file(reject).unwrap();
            let before = refs(fixture.project.path());
            status(cli.run(&fixture.local, "finish", &[]), 1);
            assert_eq!(refs(fixture.project.path()), before);
            let checked =
                fs::read_to_string(fixture.local.path().join(".git/checked-trees")).unwrap();
            assert!(checked.lines().any(|tree| tree == released), "{checked}");
            fs::write(fixture.local.path().join(".git/check-pass"), "").unwrap();
            cli.success(&fixture.local, "finish", &[]);
            let candidate = oid(fixture.project.path(), "main").unwrap();
            let checked =
                fs::read_to_string(fixture.local.path().join(".git/checked-trees")).unwrap();
            assert!(checked.lines().any(|tree| tree == released), "{checked}");
            assert!(checked.lines().any(|tree| tree == candidate), "{checked}");
        },
    );
}

#[test]
fn ordinary_preparation_source_is_independent_of_integration_target() {
    cases(
        "ordinary_preparation_source_is_independent_of_integration_target",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "none", "none", "project");
            let private = fixture.local.path().join(".git/git-oneflow.config.toml");
            let config = fs::read_to_string(&private).unwrap().replace(
                "[workstreams.ordinary]\n",
                "[workstreams.ordinary]\ncreation-source='main'\npreparation-source='staging'\nintegration-target='main'\n",
            );
            fs::write(private, config).unwrap();
            let staging = Repository::new(root);
            staging.success(&["fetch", fixture.project.path().to_str().unwrap(), "main"]);
            staging.success(&["reset", "--hard", "FETCH_HEAD"]);
            staging.success(&["switch", "-c", "staging"]);
            let prepared_base = commit(&staging, "staging", "preparation-only change\n");
            staging.success(&[
                "push",
                fixture.project.path().to_str().unwrap(),
                "HEAD:refs/heads/staging",
            ]);
            let development =
                fixture.advance_development(root, "development", "integration-only change\n");
            cli.success(
                &fixture.local,
                "start",
                &[
                    "work/separate-preparation",
                    "--policy",
                    "ordinary",
                    "--from",
                    &fixture.base,
                ],
            );
            let original = commit(&fixture.local, "work", "work change\n");
            cli.success(&fixture.local, "save", &[]);
            cli.success(&fixture.local, "finish", &["--cleanup", "keep"]);
            let source = oid(fixture.local.path(), "work/separate-preparation").unwrap();
            let integrated = oid(fixture.project.path(), "main").unwrap();
            fixture
                .local
                .success(&["merge-base", "--is-ancestor", &original, &source]);
            fixture
                .local
                .success(&["merge-base", "--is-ancestor", &prepared_base, &source]);
            assert!(
                !git::run(
                    fixture.local.path(),
                    ["merge-base", "--is-ancestor", &development, &source]
                )
                .unwrap()
                .status
                .success()
            );
            fixture
                .local
                .success(&["merge-base", "--is-ancestor", &source, &integrated]);
            fixture
                .local
                .success(&["merge-base", "--is-ancestor", &development, &integrated]);
            assert_eq!(
                value(&fixture.local, &["symbolic-ref", "--short", "HEAD"]),
                "main"
            );
            assert!(fixture.local.path().join("staging").exists());
            assert!(fixture.local.path().join("development").exists());
            assert!(fixture.local.path().join("work").exists());
            assert_eq!(oid(fixture.project.path(), "staging"), Some(prepared_base));
        },
    );
}

#[test]
#[cfg(unix)]
fn interrupted_tag_creation_recovers_exact_object_without_creating_it_again() {
    use std::os::unix::process::ExitStatusExt;
    cases(
        "interrupted_tag_creation_recovers_exact_object_without_creating_it_again",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "released", "none", "project");
            start_release(cli, &fixture, "release/train");
            let released = commit(&fixture.local, "release", "release candidate\n");
            cli.success(&fixture.local, "save", &[]);
            let git_dir = fixture.local.path().join(".git");
            fixture.local.success(&[
                "config",
                "core.hooksPath",
                git_dir.join("hooks").to_str().unwrap(),
            ]);
            hook(
                &git_dir,
                "reference-transaction",
                "if test \"$1\" = committed; then\n  while read old new reference; do\n    if test \"$reference\" = refs/tags/interrupted-version; then\n      parent=$(ps -o ppid= -p \"$PPID\" | tr -d ' ')\n      kill -KILL \"$parent\"\n    fi\n  done\nfi\nexit 0",
            );
            let interrupted = cli.run(
                &fixture.local,
                "finish",
                &[
                    "--tag",
                    "interrupted-version",
                    "--tag-message",
                    "Approved annotation",
                ],
            );
            assert_eq!(interrupted.status.signal(), Some(9), "{interrupted:?}");
            assert_eq!(
                oid(fixture.local.path(), "interrupted-version^{}"),
                Some(released.clone())
            );
            let tag = oid(fixture.local.path(), "refs/tags/interrupted-version").unwrap();
            let tag_log = fs::read(git_dir.join("logs/refs/tags/interrupted-version")).unwrap();
            // The hook stays installed; another tag creation would interrupt
            // this process too. Corroborated creation is reused on retry.
            cli.success(&fixture.local, "finish", &[]);
            assert_eq!(
                oid(fixture.project.path(), "refs/tags/interrupted-version"),
                Some(tag)
            );
            assert_eq!(
                oid(fixture.project.path(), "interrupted-version^{}"),
                Some(released.clone())
            );
            assert_eq!(oid(fixture.project.path(), "released"), Some(released));
            assert_eq!(
                fs::read(git_dir.join("logs/refs/tags/interrupted-version")).unwrap(),
                tag_log
            );
            cli.success(&fixture.local, "finish", &[]);
        },
    );
}

#[test]
fn divergent_or_aliased_release_marker_preserves_published_refs() {
    cases(
        "divergent_or_aliased_release_marker_preserves_published_refs",
        |root, cli| {
            for marker in ["main", "release/train", "released"] {
                let fixture = Fixture::new(root);
                configure(&fixture, marker, "none", "project");
                if marker != "released" {
                    let local = refs(fixture.local.path());
                    let project = refs(fixture.project.path());
                    status(
                        cli.run(
                            &fixture.local,
                            "start",
                            &[
                                "release/train",
                                "--policy",
                                "release",
                                "--from",
                                &fixture.base,
                            ],
                        ),
                        1,
                    );
                    assert_eq!(refs(fixture.local.path()), local);
                    assert_eq!(refs(fixture.project.path()), project);
                    continue;
                }
                fixture.local.success(&["switch", "-c", "released"]);
                commit(&fixture.local, "marker-side", "marker divergence\n");
                fixture
                    .local
                    .success(&["push", "project", "refs/heads/released"]);
                fixture.local.success(&["switch", "main"]);
                start_release(cli, &fixture, "release/train");
                let released = commit(&fixture.local, "release", "release candidate\n");
                cli.success(&fixture.local, "save", &[]);
                let previous_marker = oid(fixture.project.path(), marker);
                status(
                    cli.run(
                        &fixture.local,
                        "finish",
                        &["--tag", "marker-blocked-version"],
                    ),
                    1,
                );
                assert_eq!(oid(fixture.project.path(), marker), previous_marker);
                assert_eq!(oid(fixture.local.path(), "release/train"), Some(released));
            }
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn interactive_version_name_and_cancellation_preserve_unresolved_release() {
    cases(
        "interactive_version_name_and_cancellation_preserve_unresolved_release",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "none", "none", "project");
            start_release(cli, &fixture, "release/train");
            let released = commit(&fixture.local, "release", "release candidate\n");
            let project_refs = refs(fixture.project.path());
            let local_refs = refs(fixture.local.path());
            let missing = status(cli.run(&fixture.local, "finish", &[]), 1);
            assert!(missing.contains("--tag"), "{missing}");
            assert_eq!(refs(fixture.project.path()), project_refs);
            assert_eq!(refs(fixture.local.path()), local_refs);
            let command = if cli.aliases { "f" } else { "finish" };
            let mut cancelled =
                terminal::Terminal::start(cli.executable, fixture.local.path(), &[command]);
            cancelled.until("Version tag name");
            cancelled.send(b"cancel\n");
            assert_eq!(cancelled.finish().code(), Some(1), "{}", cancelled.output);
            assert!(
                cancelled.output.to_lowercase().contains("cancel"),
                "{}",
                cancelled.output
            );
            assert_eq!(refs(fixture.project.path()), project_refs);
            assert_eq!(refs(fixture.local.path()), local_refs);
            let mut accepted =
                terminal::Terminal::start(cli.executable, fixture.local.path(), &[command]);
            accepted.until("Version tag name");
            accepted.send(b"interactive-version\n");
            assert!(
                accepted
                    .finish_with_timeout(std::time::Duration::from_secs(60))
                    .success(),
                "{}",
                accepted.output
            );
            assert_eq!(
                oid(fixture.project.path(), "interactive-version^{}"),
                Some(released)
            );
            assert!(
                !accepted.output.contains("Integration commit message"),
                "{}",
                accepted.output
            );
        },
    );
}

#[test]
#[cfg(unix)]
fn tagged_marker_and_checkout_failures_retry_only_remaining_completion() {
    cases(
        "tagged_marker_and_checkout_failures_retry_only_remaining_completion",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "released", "none", "project");
            let git_dir = fixture.local.path().join(".git");
            fixture.local.success(&[
                "config",
                "core.hooksPath",
                git_dir.join("hooks").to_str().unwrap(),
            ]);
            start_release(cli, &fixture, "release/train");
            let released = commit(&fixture.local, "release", "release candidate\n");
            cli.success(&fixture.local, "save", &[]);
            let receiver = server_hook(
                &fixture,
                fixture.project.path(),
                "while read old new reference; do\n  if test \"$reference\" = refs/heads/released; then\n    echo marker-delivery-blocked >&2\n    exit 1\n  fi\ndone",
            );
            let first = status(
                cli.run(&fixture.local, "finish", &["--tag", "recovery-version"]),
                1,
            );
            assert!(first.contains("marker-delivery-blocked"), "{first}");
            let result = oid(fixture.project.path(), "main").unwrap();
            let tag = oid(fixture.project.path(), "refs/tags/recovery-version").unwrap();
            assert_eq!(
                oid(fixture.project.path(), "recovery-version^{}"),
                Some(released.clone())
            );
            assert_eq!(oid(fixture.project.path(), "released"), None);
            assert_eq!(
                oid(fixture.local.path(), "release/train"),
                Some(released.clone())
            );
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/release/train"),
                Some(released.clone())
            );
            fixture.local.success(&["switch", "release/train"]);
            fs::remove_file(receiver).unwrap();
            server_hook(
                &fixture,
                fixture.project.path(),
                "while read old new reference; do\n  case $reference in refs/heads/main|refs/tags/recovery-version)\n    echo duplicate-confirmed-delivery >&2\n    exit 1;;\n  esac\ndone",
            );
            let checkout = hook(
                &git_dir,
                "post-checkout",
                "if test \"$(git symbolic-ref --short HEAD)\" = main; then\n  echo checkout >> .git/checkout-count\n  echo final-checkout-blocked >&2\n  exit 1\nfi",
            );
            let second = status(cli.run(&fixture.local, "finish", &[]), 1);
            assert!(second.contains("final-checkout-blocked"), "{second}");
            assert_eq!(oid(fixture.project.path(), "main"), Some(result.clone()));
            assert_eq!(
                oid(fixture.project.path(), "refs/tags/recovery-version"),
                Some(tag.clone())
            );
            assert_eq!(
                oid(fixture.project.path(), "released"),
                Some(released.clone())
            );
            assert_eq!(
                value(&fixture.local, &["symbolic-ref", "--short", "HEAD"]),
                "main"
            );
            assert_eq!(
                oid(fixture.local.path(), "release/train"),
                Some(released.clone())
            );
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/release/train"),
                Some(released.clone())
            );
            fs::remove_file(checkout).unwrap();
            cli.success(&fixture.local, "finish", &[]);
            assert_eq!(oid(fixture.local.path(), "release/train"), None);
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/release/train"),
                None
            );
            assert_eq!(oid(fixture.project.path(), "main"), Some(result));
            assert_eq!(
                oid(fixture.project.path(), "refs/tags/recovery-version"),
                Some(tag)
            );
            assert_eq!(oid(fixture.project.path(), "released"), Some(released));
            assert_eq!(
                fs::read_to_string(git_dir.join("checkout-count")).unwrap(),
                "checkout\n"
            );
            cli.success(&fixture.local, "finish", &[]);
            assert_eq!(
                fs::read_to_string(git_dir.join("checkout-count")).unwrap(),
                "checkout\n"
            );
        },
    );
}

#[test]
fn ordered_release_destinations_integrate_fixed_source_with_selected_tag_identity() {
    cases(
        "ordered_release_destinations_integrate_fixed_source_with_selected_tag_identity",
        |root, cli| {
            for identity in ["work-source", "integrated-result"] {
                let fixture = Fixture::new(root);
                fixture.local.success(&["branch", "stable"]);
                fixture.local.success(&["push", "project", "stable"]);
                configure(&fixture, "released", "none", "personal");
                let config = fixture.local.path().join(".git/git-oneflow.config.toml");
                let check_log = root.join(format!("ordered-{identity}-checks"));
                let checks = format!(
                    "finish-checks=[{{name='record',command=['sh','-c','echo checked >> \"$1\"','sh',{}]}}]\n",
                    serde_json::to_string(check_log.to_str().unwrap()).unwrap()
                );
                let contents = checks + &fs::read_to_string(&config).unwrap().replace(
                "integration-target='main'",
                &format!("integration-targets=['main','stable']\nversion-tag-identity='{identity}'"),
            );
                fs::write(&config, contents).unwrap();
                start_release(cli, &fixture, "release/ordered");
                let source = commit(&fixture.local, "release", "version\n");
                let advanced = fixture.advance_development(root, "development", "after cutoff\n");
                let output = cli.success(&fixture.local, "finish", &["--tag", "ordered-v1"]);
                let first = oid(fixture.project.path(), "refs/heads/main").unwrap();
                let later = oid(fixture.project.path(), "refs/heads/stable").unwrap();
                assert_ne!(first, later);
                assert_eq!(
                    value(&fixture.local, &["rev-parse", &format!("{first}^2")]),
                    source
                );
                assert_eq!(
                    value(&fixture.local, &["rev-parse", &format!("{later}^2")]),
                    source
                );
                assert_eq!(
                    value(&fixture.local, &["rev-parse", &format!("{first}^1")]),
                    advanced
                );
                assert_eq!(
                    value(&fixture.local, &["rev-parse", &format!("{later}^1")]),
                    fixture.base
                );
                let released = if identity == "work-source" {
                    &source
                } else {
                    &first
                };
                assert_eq!(
                    oid(fixture.project.path(), "refs/tags/ordered-v1^{}").as_ref(),
                    Some(released)
                );
                assert_eq!(
                    oid(fixture.project.path(), "refs/heads/released").as_ref(),
                    Some(released)
                );
                assert_eq!(
                    value(&fixture.local, &["symbolic-ref", "HEAD"]),
                    "refs/heads/main"
                );
                assert_eq!(
                    oid(fixture.local.path(), "refs/heads/release/ordered"),
                    None
                );
                assert!(output.contains("stable"), "{output}");
                let local_refs = refs(fixture.local.path());
                let project_refs = refs(fixture.project.path());
                let personal_refs = refs(fixture.personal.path());
                let target_logs = value(
                    &fixture.local,
                    &["reflog", "show", "--format=%H %gs", "main", "stable"],
                );
                let tag_object = oid(fixture.local.path(), "refs/tags/ordered-v1").unwrap();
                let verified_checks = fs::read(&check_log).unwrap();
                cli.success(&fixture.local, "finish", &[]);
                assert_eq!(refs(fixture.local.path()), local_refs);
                assert_eq!(refs(fixture.project.path()), project_refs);
                assert_eq!(refs(fixture.personal.path()), personal_refs);
                assert_eq!(
                    value(
                        &fixture.local,
                        &["reflog", "show", "--format=%H %gs", "main", "stable"]
                    ),
                    target_logs
                );
                assert_eq!(
                    oid(fixture.local.path(), "refs/tags/ordered-v1"),
                    Some(tag_object)
                );
                assert_eq!(fs::read(&check_log).unwrap(), verified_checks);
            }
        },
    );
}

#[test]
fn integrated_result_review_publishes_only_work_and_requires_manual_tagging() {
    cases(
        "integrated_result_review_publishes_only_work_and_requires_manual_tagging",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "released", "none", "personal");
            let config = fixture.local.path().join(".git/git-oneflow.config.toml");
            let content = fs::read_to_string(&config)
                .unwrap()
                .replace(
                    "version-tagging='required'",
                    "version-tagging='required'\nversion-tag-identity='integrated-result'",
                )
                .replace("finish-mode='integrate'", "finish-mode='review'")
                .replace(
                    "finish-cleanup='both'",
                    "finish-cleanup='keep'\nfinish-checkout='work'",
                );
            fs::write(&config, content).unwrap();
            start_release(cli, &fixture, "release/review-result");
            let source = commit(&fixture.local, "release", "version\n");
            let before_local = refs(fixture.local.path());
            let before_project = refs(fixture.project.path());
            let before_personal = refs(fixture.personal.path());
            for (args, code, expected) in [
                (vec!["--tag", "manual-version"], 2, "creates no version tag"),
                (
                    vec!["--merge-back", "review/manual"],
                    2,
                    "creates no version tag",
                ),
                (vec!["--update", "rebase"], 1, "applies to direct finish"),
                (
                    vec!["--history-edit", "interactive"],
                    1,
                    "finish-history-edit = none",
                ),
            ] {
                let report = status(cli.run(&fixture.local, "finish", &args), code);
                assert!(report.contains(expected), "{report}");
                assert_eq!(refs(fixture.local.path()), before_local);
                assert_eq!(refs(fixture.project.path()), before_project);
                assert_eq!(refs(fixture.personal.path()), before_personal);
            }
            let output = cli.success(&fixture.local, "finish", &[]);
            assert!(output.contains("Ready for manual submission"), "{output}");
            assert!(
                output.contains("validate and tag the actual merged result"),
                "{output}"
            );
            assert!(output.contains("released"), "{output}");
            assert_eq!(
                oid(fixture.personal.path(), "refs/heads/release/review-result"),
                Some(source)
            );
            assert_eq!(
                oid(fixture.project.path(), "refs/heads/main"),
                Some(fixture.base)
            );
            assert_eq!(oid(fixture.project.path(), "refs/heads/released"), None);
            assert!(value(&fixture.local, &["tag", "--list"]).is_empty());
            assert!(
                git::run(fixture.project.path(), ["tag", "--list"])
                    .unwrap()
                    .stdout
                    .is_empty()
            );
            assert_eq!(
                value(&fixture.local, &["symbolic-ref", "HEAD"]),
                "refs/heads/release/review-result"
            );
        },
    );
}

#[test]
fn ordered_review_override_fails_before_publication() {
    cases(
        "ordered_review_override_fails_before_publication",
        |root, cli| {
            let fixture = Fixture::new(root);
            configure(&fixture, "none", "none", "personal");
            let config = fixture.local.path().join(".git/git-oneflow.config.toml");
            fs::write(
                &config,
                fs::read_to_string(&config).unwrap().replace(
                    "integration-target='main'",
                    "integration-targets=['main','stable']",
                ),
            )
            .unwrap();
            start_release(cli, &fixture, "release/unsupported-review");
            commit(&fixture.local, "release", "version\n");
            let before = refs(fixture.project.path());
            let output = status(
                cli.run(
                    &fixture.local,
                    "finish",
                    &["--mode", "review", "--tag", "not-created"],
                ),
                1,
            );
            assert!(output.contains("one integration destination"), "{output}");
            assert_eq!(refs(fixture.project.path()), before);
            assert_eq!(
                oid(
                    fixture.personal.path(),
                    "refs/heads/release/unsupported-review"
                ),
                None
            );
            assert_eq!(oid(fixture.local.path(), "refs/tags/not-created"), None);
        },
    );
}
