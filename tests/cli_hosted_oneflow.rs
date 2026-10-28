//! Tagged hosted review preparation through real Git and fresh CLI processes.
mod support;

use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};
use twigsmith::git;

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
                executable: env!("CARGO_BIN_EXE_twigsmith"),
                aliases: false,
            },
        );
    });
}

fn entrypoint_cases(name: &str, test: impl Fn(&Path, Cli)) {
    isolated(name, true, |root| {
        for executable in [env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")] {
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

fn configure(f: &Fixture, review: &str, active: &str) {
    fs::write(f.local.path().join(".git/twigsmith.config.toml"), format!(
        "publish-remote='personal'\nbase-remote='project'\nfinish-mode='review'\nfinish-update='none'\nfinish-integration='ff-only'\nfinish-checkout='work'\nfinish-cleanup='keep'\n\
         [workstreams.ordinary]\nversion-tagging='disabled'\n\
         [workstreams.release]\ncreation-source='main'\nintegration-target='main'\npublish-remote='project'\nbase-remote='project'\nreview-remote='{review}'\nversion-tagging='required'\nrelease-marker='released'\nfinish-mode='review'\nfinish-update='merge'\nfinish-integration='merge'\nfinish-cleanup='keep'\nfinish-checkout='work'\nfinish-rewrite='deny'\n\
         [workstreams.hotfix]\nintegration-target='main'\nactive-release='{active}'\npublish-remote='project'\nbase-remote='project'\nreview-remote='{review}'\nversion-tagging='required'\nrelease-marker='released'\nfinish-mode='review'\nfinish-update='merge'\nfinish-integration='merge'\nfinish-cleanup='keep'\nfinish-checkout='work'\nfinish-rewrite='deny'\n"
    )).unwrap();
}
fn release(cli: Cli, f: &Fixture) -> String {
    cli.success(
        &f.local,
        "start",
        &["release/current", "--policy", "release", "--from", &f.base],
    );
    commit(&f.local, "release", "approved\n")
}
#[test]
fn hosted_release_project_fork_and_exact_version_history() {
    cases(
        "hosted_release_project_fork_and_exact_version_history",
        |root, cli| {
            let f = Fixture::new(root);
            cli.success(
                &f.local,
                "config",
                &["--oneflow-hosted", "--yes", "--marker", "released"],
            );
            let config_path = f.local.path().join("twigsmith.config.toml");
            let generated = fs::read_to_string(&config_path).unwrap();
            let configured = generated
                .replace("origin", "project")
                .replacen(
                    "publish-remote = \"project\"",
                    "publish-remote = \"personal\"\nbase-remote = \"project\"",
                    1,
                )
                .replace(
                    "review-remote = \"project\"",
                    "review-remote = \"personal\"",
                );
            fs::write(&config_path, configured).unwrap();
            fs::create_dir_all(f.local.path().join(".git/info")).unwrap();
            fs::write(
                f.local.path().join(".git/info/exclude"),
                "twigsmith.config.toml\n",
            )
            .unwrap();
            let settings = cli.success(&f.local, "config", &["show"]);
            assert!(
                settings.contains("workstreams.release.review-remote: personal"),
                "{settings}"
            );
            assert!(
                settings.contains("workstreams.release.release-marker: released"),
                "{settings}"
            );
            f.local.success(&[
                "push",
                "project",
                &format!("{}:refs/heads/released", f.base),
            ]);
            let source = release(cli, &f);
            cli.success(&f.local, "save", &[]);
            let target = f.advance_development(root, "new-development", "newer\n");
            let output = cli.success(
                &f.local,
                "finish",
                &[
                    "--tag",
                    "v1",
                    "--tag-message",
                    "Approved release",
                    "--merge-back",
                    "review/v1",
                ],
            );
            assert!(output.contains("Ready for manual submission"), "{output}");
            assert!(output.contains("no-ff"));
            assert!(output.contains("manually fast-forward release marker released"));
            let tag = oid(f.local.path(), "refs/tags/v1").unwrap();
            assert_ne!(tag, source);
            assert_eq!(oid(f.project.path(), "refs/tags/v1"), Some(tag));
            assert_eq!(
                oid(f.project.path(), "refs/heads/release/current"),
                Some(source.clone())
            );
            assert_eq!(oid(f.local.path(), "refs/tags/v1^{}"), Some(source.clone()));
            assert_eq!(
                oid(f.local.path(), "refs/heads/release/current"),
                Some(source.clone())
            );
            let candidate = oid(f.personal.path(), "refs/heads/review/v1").unwrap();
            assert_eq!(
                oid(f.local.path(), "refs/remotes/project/release/current"),
                Some(source.clone())
            );
            assert_eq!(
                oid(f.local.path(), "refs/remotes/personal/review/v1"),
                Some(candidate.clone())
            );
            assert_eq!(
                value(&f.local, &["show", "-s", "--format=%P", &candidate]),
                format!("{source} {target}")
            );
            assert_eq!(
                oid(f.project.path(), "refs/heads/main"),
                Some(target.clone())
            );
            assert_eq!(oid(f.project.path(), "refs/heads/review/v1"), None);
            assert_eq!(oid(f.local.path(), "refs/remotes/project/review/v1"), None);
            assert_eq!(oid(f.personal.path(), "refs/tags/v1"), None);
            assert_eq!(
                oid(f.project.path(), "refs/heads/released"),
                Some(f.base.clone())
            );
            assert_eq!(value(&f.local, &["branch", "--show-current"]), "review/v1");
            cli.success(&f.local, "finish", &[]);
            assert_eq!(
                oid(f.personal.path(), "refs/heads/review/v1"),
                Some(candidate.clone())
            );

            // Simulate the provider's no-ff merge and the later manual marker step
            // in a separate project checkout. Neither action is owned by finish.
            let maintainer = Repository::new(root);
            maintainer.success(&[
                "fetch",
                f.project.path().to_str().unwrap(),
                "refs/tags/v1:refs/tags/v1",
            ]);
            maintainer.success(&["fetch", f.project.path().to_str().unwrap(), "main"]);
            maintainer.success(&["reset", "--hard", "FETCH_HEAD"]);
            maintainer.success(&["fetch", f.personal.path().to_str().unwrap(), "review/v1"]);
            maintainer.success(&["merge", "--no-ff", "-m", "Integrate release", "FETCH_HEAD"]);
            let merged = value(&maintainer, &["rev-parse", "HEAD"]);
            assert_eq!(
                value(&maintainer, &["show", "-s", "--format=%P", &merged]),
                format!("{target} {candidate}")
            );
            maintainer.success(&[
                "push",
                f.project.path().to_str().unwrap(),
                "HEAD:refs/heads/main",
            ]);
            maintainer.success(&["fetch", f.project.path().to_str().unwrap(), "released"]);
            maintainer.success(&["checkout", "-b", "released", "FETCH_HEAD"]);
            maintainer.success(&["merge", "--ff-only", "v1"]);
            maintainer.success(&[
                "push",
                f.project.path().to_str().unwrap(),
                "HEAD:refs/heads/released",
            ]);
            assert_eq!(oid(f.project.path(), "refs/heads/main"), Some(merged));
            assert_eq!(
                oid(f.project.path(), "refs/heads/released"),
                Some(source.clone())
            );
            assert_eq!(oid(f.project.path(), "refs/tags/v1^{}"), Some(source));
        },
    );
}
#[test]
fn hosted_executable_alias_and_shared_remote_wiring() {
    entrypoint_cases(
        "hosted_executable_alias_and_shared_remote_wiring",
        |root, cli| {
            let f = Fixture::new(root);
            configure(&f, "project", "none");
            let source = release(cli, &f);
            cli.success(&f.local, "save", &[]);
            cli.success(
                &f.local,
                "finish",
                &["--tag", "v-shared", "--merge-back", "back/shared"],
            );
            assert_eq!(
                oid(f.project.path(), "refs/heads/back/shared"),
                Some(source.clone())
            );
            assert_eq!(oid(f.project.path(), "refs/tags/v-shared"), Some(source));
        },
    );
}
#[test]
fn hosted_hotfix_routes_to_declared_active_release_or_development() {
    cases(
        "hosted_hotfix_routes_to_declared_active_release_or_development",
        |root, cli| {
            for active in ["none", "release/active"] {
                let f = Fixture::new(root);
                configure(&f, "personal", active);
                f.local.success(&["tag", "previous", &f.base]);
                if active != "none" {
                    f.local.success(&[
                        "push",
                        "project",
                        &format!("{}:refs/heads/{active}", f.base),
                    ]);
                }
                cli.success(
                    &f.local,
                    "start",
                    &[
                        "hotfix/current",
                        "--policy",
                        "hotfix",
                        "--from-tag",
                        "previous",
                    ],
                );
                let source = commit(&f.local, "fix", "corrected\n");
                let output = cli.success(
                    &f.local,
                    "finish",
                    &["--tag", "v-fixed", "--merge-back", "back/fixed"],
                );
                let target = if active == "none" { "main" } else { active };
                assert!(
                    output.contains(&format!("into project/{target}")),
                    "{output}"
                );
                assert_eq!(oid(f.project.path(), "refs/tags/v-fixed"), Some(source));
                assert_eq!(
                    oid(f.project.path(), &format!("refs/heads/{target}")),
                    Some(f.base)
                );
            }
        },
    );
}
#[test]
fn hosted_conflict_continue_and_abort_preserve_original_version() {
    cases(
        "hosted_conflict_continue_and_abort_preserve_original_version",
        |root, cli| {
            for abort_first in [false, true] {
                let f = Fixture::new(root);
                configure(&f, "personal", "none");
                cli.success(
                    &f.local,
                    "start",
                    &["release/current", "--policy", "release", "--from", &f.base],
                );
                let source = commit(&f.local, "base", "released\n");
                let target = f.advance_development(root, "base", "development\n");
                let args = ["--tag", "v-conflict", "--merge-back", "back/conflict"];
                let output = status(cli.run(&f.local, "finish", &args), 1);
                assert!(output.contains("git merge --continue"), "{output}");
                let tag = oid(f.local.path(), "refs/tags/v-conflict").unwrap();
                assert_eq!(tag, source);
                assert_eq!(oid(f.project.path(), "refs/tags/v-conflict"), None);
                if abort_first {
                    f.local.success(&["merge", "--abort"]);
                    status(cli.run(&f.local, "finish", &[]), 1);
                    assert!(f.local.path().join(".git/MERGE_HEAD").exists());
                }
                fs::write(f.local.path().join("base"), "resolved on merge-back\n").unwrap();
                f.local.success(&["add", "base"]);
                f.local.success(&["commit", "--no-edit"]);
                cli.success(&f.local, "finish", &[]);
                assert_eq!(oid(f.local.path(), "refs/tags/v-conflict"), Some(tag));
                assert_eq!(
                    oid(f.local.path(), "refs/heads/release/current"),
                    Some(source.clone())
                );
                let candidate = oid(f.personal.path(), "refs/heads/back/conflict").unwrap();
                assert_eq!(
                    value(&f.local, &["show", "-s", "--format=%P", &candidate]),
                    format!("{source} {target}")
                );
            }
        },
    );
}
#[test]
fn ordinary_contribution_into_release_or_hotfix_has_no_tagged_effects() {
    cases(
        "ordinary_contribution_into_release_or_hotfix_has_no_tagged_effects",
        |root, cli| {
            for target in ["release/active", "hotfix/active"] {
                let f = Fixture::new(root);
                configure(&f, "personal", "none");
                f.local.success(&[
                    "push",
                    "project",
                    &format!("{}:refs/heads/{target}", f.base),
                ]);
                cli.success(
                    &f.local,
                    "start",
                    &["contribution", "--policy", "ordinary", "--target", target],
                );
                let source = commit(&f.local, "contribution", "change\n");
                let output = cli.success(&f.local, "finish", &[]);
                assert!(output.contains(target), "{output}");
                assert_eq!(
                    oid(f.personal.path(), "refs/heads/contribution"),
                    Some(source)
                );
                assert_eq!(
                    oid(f.project.path(), &format!("refs/heads/{target}")),
                    Some(f.base)
                );
                assert!(value(&f.local, &["tag", "--list"]).is_empty());
            }
        },
    );
}
#[cfg(unix)]
#[test]
fn hosted_dirty_checkout_hook_stops_before_merge_or_publication() {
    cases(
        "hosted_dirty_checkout_hook_stops_before_merge_or_publication",
        |root, cli| {
            let f = Fixture::new(root);
            configure(&f, "personal", "none");
            release(cli, &f);
            f.advance_development(root, "target", "newer\n");
            f.local.success(&["config", "core.hooksPath", ".git/hooks"]);
            hook(
                &f.local.path().join(".git"),
                "post-checkout",
                "echo dirty > untracked-hook-output",
            );
            status(
                cli.run(
                    &f.local,
                    "finish",
                    &["--tag", "v-hook", "--merge-back", "back/hook"],
                ),
                1,
            );
            assert!(!f.local.path().join(".git/MERGE_HEAD").exists());
            assert_eq!(oid(f.project.path(), "refs/tags/v-hook"), None);
            assert_eq!(oid(f.personal.path(), "refs/heads/back/hook"), None);
        },
    );
}

#[test]
fn hosted_fresh_collisions_and_aliases_do_not_adopt_existing_refs() {
    cases(
        "hosted_fresh_collisions_and_aliases_do_not_adopt_existing_refs",
        |root, cli| {
            for collision in [
                "local-tag",
                "remote-tag",
                "local-back",
                "remote-back",
                "source",
                "target",
                "marker",
            ] {
                let f = Fixture::new(root);
                configure(&f, "personal", "none");
                let source = release(cli, &f);
                let mut back = "back/collision";
                match collision {
                    "local-tag" => {
                        f.local.success(&["tag", "v-collision", &source]);
                    }
                    "remote-tag" => {
                        f.local.success(&[
                            "push",
                            "project",
                            &format!("{source}:refs/tags/v-collision"),
                        ]);
                    }
                    "local-back" => {
                        f.local.success(&["branch", back, &source]);
                    }
                    "remote-back" => {
                        f.local.success(&[
                            "push",
                            "personal",
                            &format!("{source}:refs/heads/{back}"),
                        ]);
                    }
                    "source" => back = "release/current",
                    "target" => back = "main",
                    "marker" => back = "released",
                    _ => unreachable!(),
                }
                status(
                    cli.run(
                        &f.local,
                        "finish",
                        &["--tag", "v-collision", "--merge-back", back],
                    ),
                    1,
                );
                assert_eq!(oid(f.local.path(), "HEAD"), Some(source));
                assert_eq!(oid(f.project.path(), "refs/heads/release/current"), None);
            }
        },
    );
}
#[cfg(unix)]
#[test]
fn hosted_exact_parent_candidate_without_preparation_start_is_rejected() {
    cases(
        "hosted_exact_parent_candidate_without_preparation_start_is_rejected",
        |root, cli| {
            let f = Fixture::new(root);
            configure(&f, "personal", "none");
            let source = release(cli, &f);
            let target = f.advance_development(root, "target", "newer\n");
            f.local.success(&["config", "core.hooksPath", ".git/hooks"]);
            hook(
                &f.local.path().join(".git"),
                "post-checkout",
                &format!(
                    "tree=$(git rev-parse HEAD^{{tree}})\ncandidate=$(printf 'Forged candidate\\n' | git commit-tree \"$tree\" -p {source} -p {target})\ngit update-ref refs/heads/back/forged \"$candidate\" {source}"
                ),
            );
            let output = status(
                cli.run(
                    &f.local,
                    "finish",
                    &["--tag", "v-forged", "--merge-back", "back/forged"],
                ),
                1,
            );
            assert!(output.contains("preparation start"), "{output}");
            assert_eq!(oid(f.project.path(), "refs/tags/v-forged"), None);
            assert_eq!(oid(f.personal.path(), "refs/heads/back/forged"), None);
            assert_eq!(
                oid(f.local.path(), "refs/heads/release/current"),
                Some(source)
            );
        },
    );
}
#[test]
fn hosted_merge_preserves_ignored_file_that_target_would_overwrite() {
    cases(
        "hosted_merge_preserves_ignored_file_that_target_would_overwrite",
        |root, cli| {
            let f = Fixture::new(root);
            configure(&f, "personal", "none");
            release(cli, &f);
            fs::create_dir_all(f.local.path().join(".git/info")).unwrap();
            fs::write(f.local.path().join(".git/info/exclude"), "local-secret\n").unwrap();
            fs::write(
                f.local.path().join("local-secret"),
                "preserve local bytes\n",
            )
            .unwrap();
            f.advance_development(root, "local-secret", "tracked development content\n");
            status(
                cli.run(
                    &f.local,
                    "finish",
                    &["--tag", "v-ignored", "--merge-back", "back/ignored"],
                ),
                1,
            );
            assert_eq!(
                fs::read_to_string(f.local.path().join("local-secret")).unwrap(),
                "preserve local bytes\n"
            );
            assert_eq!(oid(f.project.path(), "refs/tags/v-ignored"), None);
        },
    );
}

#[test]
fn hosted_forged_parent_candidate_after_abort_is_rejected() {
    cases(
        "hosted_forged_parent_candidate_after_abort_is_rejected",
        |root, cli| {
            let f = Fixture::new(root);
            configure(&f, "personal", "none");
            cli.success(
                &f.local,
                "start",
                &["release/current", "--policy", "release", "--from", &f.base],
            );
            let source = commit(&f.local, "base", "released\n");
            let target = f.advance_development(root, "base", "development\n");
            status(
                cli.run(
                    &f.local,
                    "finish",
                    &["--tag", "v-reset", "--merge-back", "back/reset"],
                ),
                1,
            );
            f.local.success(&["merge", "--abort"]);
            let tree = value(&f.local, &["rev-parse", &format!("{source}^{{tree}}")]);
            let forged = value(
                &f.local,
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    &source,
                    "-p",
                    &target,
                    "-m",
                    "Forged result",
                ],
            );
            f.local.success(&["reset", "--hard", &forged]);
            let output = status(cli.run(&f.local, "finish", &[]), 1);
            assert!(
                output.contains("owned merge or native continuation"),
                "{output}"
            );
            assert_eq!(oid(f.project.path(), "refs/tags/v-reset"), None);
            assert_eq!(oid(f.personal.path(), "refs/heads/back/reset"), None);
        },
    );
}

#[test]
fn hosted_tag_collision_at_fetch_only_project_endpoint_blocks_delivery() {
    cases(
        "hosted_tag_collision_at_fetch_only_project_endpoint_blocks_delivery",
        |root, cli| {
            let f = Fixture::new(root);
            configure(&f, "personal", "none");
            let source = release(cli, &f);
            let publication = clone_bare(root, &f.local, f.project.path());
            f.local
                .success(&["push", "project", &format!("{source}:refs/tags/v-fetch")]);
            f.local.success(&[
                "config",
                "remote.project.pushurl",
                publication.path().to_str().unwrap(),
            ]);
            let output = status(
                cli.run(
                    &f.local,
                    "finish",
                    &["--tag", "v-fetch", "--merge-back", "back/fetch"],
                ),
                1,
            );
            assert!(output.contains("project fetch endpoint"), "{output}");
            assert_eq!(oid(f.local.path(), "refs/tags/v-fetch"), None);
            assert_eq!(oid(publication.path(), "refs/tags/v-fetch"), None);
            assert_eq!(oid(publication.path(), "refs/heads/release/current"), None);
            assert_eq!(oid(f.personal.path(), "refs/heads/back/fetch"), None);
        },
    );
}
