//! Generated shared settings exercised through actual native command processes.
use super::support::{Repository, isolated};
#[path = "../support/native_terminal.rs"]
mod terminal;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};

fn cli(repo: &Repository, args: &[&str]) -> Output {
    Command::new(super::support::executable("tgi"))
        .current_dir(repo.path())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn success(repo: &Repository, args: &[&str]) {
    let output = cli(repo, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
}
fn oid(repo: &Repository, reference: &str) -> Vec<u8> {
    repo.success(&["rev-parse", reference]).stdout
}
fn commit(repo: &Repository, name: &str) {
    fs::write(repo.path().join(name), format!("{name}\n")).unwrap();
    repo.success(&["add", name]);
    repo.success(&["commit", "-m", name]);
}
fn remote_oid(repo: &Repository, remote: &Path, reference: &str) -> Vec<u8> {
    repo.success(&[
        "--git-dir".as_ref(),
        remote.as_os_str(),
        "rev-parse".as_ref(),
        reference.as_ref(),
    ])
    .stdout
}

#[test]
fn generated_github_and_trunk_settings_drive_split_remote_journeys() {
    isolated(
        "bundles::generated_github_and_trunk_settings_drive_split_remote_journeys",
        true,
        |root| {
            for (bundle, delivery) in [
                ("github-flow", "review"),
                ("trunk", "review"),
                ("trunk", "direct"),
            ] {
                let local = Repository::new(root);
                commit(&local, "base");
                local.success(&["branch", "-m", "trunk"]);
                let project = tempfile::tempdir_in(root).unwrap();
                let fork = tempfile::tempdir_in(root).unwrap();
                for (name, remote) in [("project", &project), ("workfork", &fork)] {
                    local.success(&[
                        "clone".as_ref(),
                        "--bare".as_ref(),
                        local.path().as_os_str(),
                        remote.path().as_os_str(),
                    ]);
                    local.success(&[
                        "remote".as_ref(),
                        "add".as_ref(),
                        name.as_ref(),
                        remote.path().as_os_str(),
                    ]);
                }
                let before_head = oid(&local, "HEAD");
                let before_refs = local.success(&["show-ref"]).stdout;
                let before_index = fs::read(local.path().join(".git/index")).unwrap();
                success(
                    &local,
                    &[
                        "config",
                        "--yes",
                        "--bundle",
                        bundle,
                        "--delivery",
                        delivery,
                        "--base",
                        "trunk",
                        "--base-remote",
                        "project",
                        "--publish-remote",
                        "workfork",
                    ],
                );
                // Generation only creates the shared document, never Git state.
                assert_eq!(oid(&local, "HEAD"), before_head);
                assert_eq!(local.success(&["show-ref"]).stdout, before_refs);
                assert_eq!(
                    fs::read(local.path().join(".git/index")).unwrap(),
                    before_index
                );
                local.success(&["add", "twigsmith.config.toml"]);
                local.success(&["commit", "-m", "Configure workflow"]);
                local.success(&["push", "project", "trunk"]);
                let old = oid(&local, "HEAD");
                let seed = Repository::new(root);
                seed.success(&[
                    "remote".as_ref(),
                    "add".as_ref(),
                    "project".as_ref(),
                    project.path().as_os_str(),
                ]);
                seed.success(&["fetch", "project"]);
                seed.success(&["switch", "-c", "trunk", "project/trunk"]);
                commit(&seed, "advanced");
                seed.success(&["push", "project", "trunk"]);
                let advanced = oid(&seed, "HEAD");
                assert_ne!(old, advanced);
                success(&local, &["start", "small-change"]);
                assert_eq!(oid(&local, "HEAD"), advanced);
                assert_eq!(oid(&local, "trunk"), old);
                fs::write(local.path().join("change"), "staged change\n").unwrap();
                local.success(&["add", "change"]);
                fs::write(local.path().join("keep.txt"), "unfinished work\n").unwrap();
                if bundle == "trunk" {
                    success(&local, &["save"]);
                } else {
                    success(&local, &["save", "--commit", "staged"]);
                }
                let source = oid(&local, "HEAD");
                assert_eq!(remote_oid(&local, fork.path(), "small-change"), source);
                assert_eq!(
                    fs::read(local.path().join("keep.txt")).unwrap(),
                    b"unfinished work\n"
                );
                let refused = cli(&local, &["finish"]);
                assert!(!refused.status.success(), "{refused:?}");
                assert_eq!(remote_oid(&local, project.path(), "trunk"), advanced);
                assert_eq!(oid(&local, "HEAD"), source);
                // The fixture's unfinished file was preserved; removing it makes the
                // configured finish operation eligible without adding CLI preferences.
                fs::remove_file(local.path().join("keep.txt")).unwrap();
                success(&local, &["finish"]);
                if delivery == "review" {
                    assert_eq!(remote_oid(&local, project.path(), "trunk"), advanced);
                    assert_eq!(oid(&local, "trunk"), old);
                    assert_eq!(oid(&local, "small-change"), source);
                    assert_eq!(
                        local.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                        b"small-change\n"
                    );
                } else {
                    assert_eq!(remote_oid(&local, project.path(), "trunk"), source);
                    assert_eq!(oid(&local, "trunk"), source);
                    assert_eq!(
                        local.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                        b"trunk\n"
                    );
                    assert!(
                        local
                            .success(&["branch", "--list", "small-change"])
                            .stdout
                            .is_empty()
                    );
                    assert!(
                        local
                            .success(&[
                                "--git-dir".as_ref(),
                                fork.path().as_os_str(),
                                "branch".as_ref(),
                                "--list".as_ref(),
                                "small-change".as_ref()
                            ])
                            .stdout
                            .is_empty()
                    );
                }
                assert!(local.success(&["tag", "--list"]).stdout.is_empty());
            }
        },
    );
}

#[test]
fn guided_gitflow_repository_correction_updates_roles_and_preserves_custom_route() {
    isolated(
        "bundles::guided_gitflow_repository_correction_updates_roles_and_preserves_custom_route",
        true,
        |root| {
            let repo = Repository::new(root);
            let mut command = Command::new(super::support::executable("tgi"));
            command.current_dir(repo.path()).args([
                "config",
                "--bundle",
                "gitflow",
                "--publish-remote",
                "fork",
                "--base-remote",
                "project",
                "--base",
                "development",
                "--production",
                "production",
            ]);
            let mut term = terminal::NativeTerminal::start_command(command);
            term.until("Remote receiving work");
            term.send(b"\n\n\n\n\nkeep\nkeep\nkeep\nedit\npreparation-source\nset\nspecial-base\ndone\nkeep\nkeep\n");
            term.until("Create this file?");
            term.send(b"repository\nfork2\nproject2\ndevelopment2\nyes\n");
            assert!(term.finish().success(), "{}", term.output);
            term.assert_restored();
            let repo_context = twigsmith::repository::discover(repo.path())
                .unwrap()
                .unwrap();
            let loaded = twigsmith::settings::load(repo.path(), &repo_context).unwrap();
            assert_eq!(loaded.base, "development2");
            assert_eq!(loaded.publish_remote, "fork2");
            assert_eq!(loaded.base_remote, "project2");
            for kind in [
                twigsmith::settings::WorkstreamKind::Ordinary,
                twigsmith::settings::WorkstreamKind::Release,
                twigsmith::settings::WorkstreamKind::Hotfix,
            ] {
                let mut resolved = loaded.clone();
                twigsmith::settings::resolve_workstream(&mut resolved, kind, None).unwrap();
                let policy = resolved.workstream.unwrap();
                if kind == twigsmith::settings::WorkstreamKind::Ordinary {
                    assert_eq!(policy.integration_targets, ["development2"]);
                    assert_eq!(policy.preparation_source, "special-base");
                    assert_eq!(policy.publish_remote, "fork2");
                } else {
                    assert_eq!(policy.integration_targets, ["production", "development2"]);
                    assert_eq!(policy.base_remote, "project2");
                    assert_eq!(policy.publish_remote, "project2");
                    if kind == twigsmith::settings::WorkstreamKind::Release {
                        assert_eq!(policy.creation_source.as_deref(), Some("development2"));
                        assert_eq!(policy.preparation_source, "development2");
                    }
                }
            }
            assert!(repo.success(&["for-each-ref"]).stdout.is_empty());
        },
    );
}
