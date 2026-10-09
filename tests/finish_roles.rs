#![cfg(unix)]
mod support;

use git_oneflow::{
    finish::{
        self, Constraint, Plan, PublicationMode, Recovery, RefObservation, Request, RequiredInput,
        preparation, publication,
    },
    git,
    settings::{FinishRewrite, FinishUpdate},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use support::{Repository, isolated};

const PROJECT: &str = "team/project";
const PUBLISH: &str = "personal/publish";
const BRANCH: &str = "feature/work";
const REF: &str = "refs/heads/feature/work";
fn oid(repo: &Repository, reference: &str) -> Vec<u8> {
    repo.success(&["rev-parse", reference])
        .stdout
        .strip_suffix(b"\n")
        .unwrap()
        .to_vec()
}
fn commit(repo: &Repository, file: &str, content: &str) -> Vec<u8> {
    fs::write(repo.path().join(file), content).unwrap();
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", content]);
    oid(repo, "HEAD")
}
fn refs(repo: &Repository) -> Vec<u8> {
    repo.success(&["for-each-ref", "--format=%(refname) %(objectname)"])
        .stdout
}
fn seed(local: &Repository, remote: &Repository) {
    remote.success(&[
        "fetch",
        local.path().to_str().unwrap(),
        &format!("{REF}:{REF}"),
    ]);
}
fn request(update: FinishUpdate) -> Request {
    Request {
        update: Some(update),
        rewrite: Some(FinishRewrite::Allow),
    }
}
fn fresh(local: &Repository, req: &Request) -> Plan {
    let initial = finish::assess(local.path(), req, None).unwrap();
    let report = preparation::refresh(local.path(), &initial);
    assert!(report.error.is_none(), "{report:#?}");
    report.plan.unwrap()
}
fn prepared(local: &Repository, update: FinishUpdate) -> Plan {
    let report = preparation::prepare(local.path(), &fresh(local, &request(update)));
    assert!(report.is_prepared(), "{report:#?}");
    report.plan.unwrap()
}
fn cli(local: &Repository, update: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
        .current_dir(local.path())
        .args(["finish", "--update", update, "--allow-rewrite"])
        .output()
        .unwrap()
}
fn succeeds(out: Output) -> String {
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}
fn hook(repo: &Repository, name: &str, body: &str) -> PathBuf {
    let directory = repo.path().join(".git/role-hooks");
    fs::create_dir_all(&directory).unwrap();
    repo.success(&["config", "core.hooksPath", directory.to_str().unwrap()]);
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}
struct Fixture {
    local: Repository,
    project: Repository,
    fork: Repository,
    base: Vec<u8>,
    original: Vec<u8>,
}
impl Fixture {
    fn new(root: &Path, equal_roles: bool, divergent_fork: bool, conflict: bool) -> Self {
        let project = Repository::new(root);
        commit(&project, "base", "initial\n");
        let local = Repository::new(root);
        local.success(&["remote", "add", PROJECT, project.path().to_str().unwrap()]);
        local.success(&["fetch", PROJECT]);
        local.success(&[
            "switch",
            "--no-track",
            "-c",
            BRANCH,
            &format!("{PROJECT}/main"),
        ]);
        local.success(&["branch", "main", "HEAD"]);
        let fork = Repository::new(root);
        fork.success(&["fetch", project.path().to_str().unwrap(), "main"]);
        fork.success(&["reset", "--hard", "FETCH_HEAD"]);
        if divergent_fork {
            commit(&fork, "fork-only", "fork base diverges\n");
        }
        local.success(&[
            "remote",
            "add",
            PUBLISH,
            if equal_roles {
                project.path()
            } else {
                fork.path()
            }
            .to_str()
            .unwrap(),
        ]);
        // Equal roles also exercise different fetch and push endpoints.
        local.success(&[
            "config",
            &format!("remote.{PUBLISH}.pushurl"),
            fork.path().to_str().unwrap(),
        ]);
        let original = commit(
            &local,
            if conflict { "base" } else { "work" },
            "working change\n",
        );
        seed(&local, &fork);
        // A project working ref must never substitute for the fork observation.
        seed(&local, &project);
        let base = commit(
            &project,
            if conflict { "base" } else { "project-only" },
            "advancing project\n",
        );
        let f = Self {
            local,
            project,
            fork,
            base,
            original,
        };
        f.settings(if equal_roles { PUBLISH } else { PROJECT }, PUBLISH);
        f
    }
    fn settings(&self, base: &str, publish: &str) {
        fs::write(
            self.local.path().join(".git/git-oneflow.config.toml"),
            format!("base-remote = {base:?}\npublish-remote = {publish:?}\n"),
        )
        .unwrap();
    }
    fn record(&self) -> PathBuf {
        self.local
            .path()
            .join(".git/oneflow-finish/branches/feature/work/rewrite.json")
    }
    fn legacy(&self) {
        // Exact v1 shape from the preceding implementation: retain observations,
        // identity, attempts and reflog anchors; no fabricated role bindings.
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(self.record()).unwrap()).unwrap();
        assert_eq!(record["version"], 2);
        record["version"] = 1.into();
        record.as_object_mut().unwrap().remove("roles");
        fs::write(self.record(), serde_json::to_vec(&record).unwrap()).unwrap();
    }
}

#[test]
fn policies_and_tracking_use_project_base_and_only_publish_work() {
    isolated(
        "policies_and_tracking_use_project_base_and_only_publish_work",
        true,
        |root| {
            for equal in [false, true] {
                for divergent in [false, true] {
                    for update in [
                        FinishUpdate::Rebase,
                        FinishUpdate::Merge,
                        FinishUpdate::None,
                    ] {
                        for tracking in ["absent", "base", "publication"] {
                            let f = Fixture::new(root, equal, divergent, false);
                            let base_remote = if equal { PUBLISH } else { PROJECT };
                            if tracking != "absent" {
                                f.local.success(&[
                                    "config",
                                    &format!("branch.{BRANCH}.remote"),
                                    if tracking == "base" {
                                        base_remote
                                    } else {
                                        PUBLISH
                                    },
                                ]);
                                f.local.success(&[
                                    "config",
                                    &format!("branch.{BRANCH}.merge"),
                                    if tracking == "base" {
                                        "refs/heads/main"
                                    } else {
                                        REF
                                    },
                                ]);
                            }
                            let config = fs::read(f.local.path().join(".git/config")).unwrap();
                            let project_refs = refs(&f.project);
                            let fork_base = oid(&f.fork, "main");
                            let local_base = oid(&f.local, "main");
                            let asks = fresh(&f.local, &Request::default());
                            assert!(asks.required_input.contains(&RequiredInput::Update));
                            let p = prepared(&f.local, update);
                            let e = p.evidence.as_ref().unwrap();
                            assert_eq!(e.base_remote, base_remote);
                            assert_eq!(e.publish_remote, PUBLISH);
                            assert_eq!(
                                e.base_destination,
                                f.project.path().as_os_str().as_encoded_bytes()
                            );
                            assert_eq!(e.base, RefObservation::Commit(f.base.clone()));
                            assert_eq!(
                                e.destinations[0].destination,
                                f.fork.path().as_os_str().as_encoded_bytes()
                            );
                            let includes = git::run(
                                f.local.path(),
                                [
                                    "merge-base",
                                    "--is-ancestor",
                                    std::str::from_utf8(&f.base).unwrap(),
                                    "HEAD",
                                ],
                            )
                            .unwrap()
                            .status
                            .success();
                            assert_eq!(includes, update != FinishUpdate::None);
                            assert!(!f.local.path().join("fork-only").exists());
                            if update == FinishUpdate::None {
                                assert_eq!(p.commit, f.original);
                            }
                            assert_eq!(
                                fs::read(f.local.path().join(".git/config")).unwrap(),
                                config
                            );
                            let report = publication::publish(f.local.path(), &p);
                            assert!(report.is_complete(), "{report:#?}");
                            assert_eq!(oid(&f.fork, REF), p.commit);
                            assert_eq!(refs(&f.project), project_refs);
                            assert_eq!(oid(&f.fork, "main"), fork_base);
                            assert_eq!(oid(&f.local, "main"), local_base);
                            if tracking == "absent" {
                                assert_eq!(report.tracking.writes.len(), 2);
                                assert_eq!(
                                    f.local
                                        .success(&[
                                            "config",
                                            "--get",
                                            &format!("branch.{BRANCH}.remote")
                                        ])
                                        .stdout,
                                    format!("{PUBLISH}\n").as_bytes()
                                );
                                assert_eq!(
                                    f.local
                                        .success(&[
                                            "config",
                                            "--get",
                                            &format!("branch.{BRANCH}.merge")
                                        ])
                                        .stdout,
                                    format!("{REF}\n").as_bytes()
                                );
                            } else {
                                assert!(report.tracking.writes.is_empty());
                                assert_eq!(
                                    fs::read(f.local.path().join(".git/config")).unwrap(),
                                    config
                                );
                            }
                        }
                    }
                }
            }
        },
    );
}

#[test]
fn required_project_base_never_falls_back_to_fork() {
    isolated(
        "required_project_base_never_falls_back_to_fork",
        true,
        |root| {
            for missing in [false, true] {
                for update in [
                    FinishUpdate::Rebase,
                    FinishUpdate::Merge,
                    FinishUpdate::None,
                ] {
                    let f = Fixture::new(root, false, false, false);
                    if missing {
                        f.project.success(&["update-ref", "-d", "refs/heads/main"]);
                    } else {
                        f.local
                            .success(&["remote", "set-url", PROJECT, "/missing/project"]);
                    }
                    let initial = finish::assess(f.local.path(), &request(update), None).unwrap();
                    let report = preparation::refresh(f.local.path(), &initial);
                    if missing {
                        assert!(
                            report
                                .plan
                                .unwrap()
                                .constraints
                                .contains(&Constraint::BaseUnavailable)
                        );
                    } else {
                        assert!(report.error.is_some());
                    }
                    assert_eq!(oid(&f.local, "HEAD"), f.original);
                    assert_eq!(oid(&f.fork, REF), f.original);
                }
            }
        },
    );
}

#[test]
fn changes_to_roles_endpoints_and_configuration_invalidate_plans_and_new_records() {
    isolated(
        "changes_to_roles_endpoints_and_configuration_invalidate_plans_and_new_records",
        true,
        |root| {
            for change in [
                "base-role",
                "publish-role",
                "base-url",
                "push-url",
                "base-config",
                "publish-config",
                "url-rewrite",
            ] {
                let f = Fixture::new(root, false, false, false);
                let p = prepared(&f.local, FinishUpdate::Rebase);
                let other = Repository::new(root);
                other.success(&["fetch", f.project.path().to_str().unwrap(), "main"]);
                seed(&f.fork, &other);
                other.success(&["reset", "--hard", "FETCH_HEAD"]);
                // New destination advertises exactly the old work: equal OIDs do
                // not transfer incorporation evidence to a different endpoint.
                other.success(&["update-ref", REF, std::str::from_utf8(&f.original).unwrap()]);
                match change {
                    "base-role" => {
                        f.local.success(&[
                            "remote",
                            "add",
                            "new-base",
                            f.project.path().to_str().unwrap(),
                        ]);
                        f.settings("new-base", PUBLISH);
                    }
                    "publish-role" => {
                        f.local.success(&[
                            "remote",
                            "add",
                            "new-publish",
                            f.fork.path().to_str().unwrap(),
                        ]);
                        f.settings(PROJECT, "new-publish");
                    }
                    "base-url" => {
                        f.local.success(&[
                            "remote",
                            "set-url",
                            PROJECT,
                            other.path().to_str().unwrap(),
                        ]);
                    }
                    "push-url" => {
                        f.local.success(&[
                            "remote",
                            "set-url",
                            "--push",
                            PUBLISH,
                            other.path().to_str().unwrap(),
                        ]);
                    }
                    "base-config" => {
                        f.local.success(&[
                            "config",
                            &format!("remote.{PROJECT}.tagOpt"),
                            "--no-tags",
                        ]);
                    }
                    "publish-config" => {
                        f.local.success(&[
                            "config",
                            &format!("remote.{PUBLISH}.receivepack"),
                            "git-receive-pack",
                        ]);
                    }
                    _ => {
                        f.local.success(&[
                            "config",
                            &format!("url.{}.insteadOf", f.project.path().display()),
                            "unused-project-alias:",
                        ]);
                    }
                }
                assert!(
                    finish::revalidate(f.local.path(), &p, p.evidence.as_ref()).is_err(),
                    "{change}"
                );
                let stopped = publication::publish(f.local.path(), &p);
                assert!(!stopped.is_complete());
                assert!(stopped.destinations.iter().all(|d| d.commands.is_empty()));
                let retry = fresh(&f.local, &request(FinishUpdate::None));
                assert_eq!(retry.recovery, Recovery::Invalid, "{change}");
                assert!(!retry.is_ready(), "{change}: {retry:#?}");
                assert_eq!(oid(&f.fork, REF), f.original);
                // Invalid recovery never prevents independently proven ancestry.
                f.local.success(&[
                    "merge",
                    "--no-edit",
                    std::str::from_utf8(&f.original).unwrap(),
                ]);
                let ordinary = fresh(&f.local, &request(FinishUpdate::None));
                assert_eq!(ordinary.recovery, Recovery::Invalid);
                assert!(ordinary.is_ready(), "{change}: {ordinary:#?}");
                assert_eq!(
                    ordinary.destinations[0].mode,
                    PublicationMode::FastForwardOrUnchanged
                );
            }
        },
    );
}

#[test]
fn split_role_conflict_continue_abort_and_interruption_recover_across_processes() {
    isolated(
        "split_role_conflict_continue_abort_and_interruption_recover_across_processes",
        true,
        |root| {
            for mode in ["continue", "abort", "interrupted"] {
                let f = Fixture::new(root, false, true, mode != "interrupted");
                let interrupt = (mode == "interrupted")
                    .then(|| hook(&f.local, "post-checkout", "kill -TERM \"$PPID\""));
                let output = cli(&f.local, "rebase");
                assert!(!output.status.success());
                assert!(f.record().exists());
                assert_eq!(oid(&f.fork, REF), f.original);
                assert!(
                    !git::run(
                        f.local.path(),
                        ["config", "--get", &format!("branch.{BRANCH}.remote")]
                    )
                    .unwrap()
                    .status
                    .success()
                );
                if mode == "abort" {
                    f.local.success(&["rebase", "--abort"]);
                    assert_eq!(
                        fresh(&f.local, &request(FinishUpdate::None)).recovery,
                        Recovery::Unchanged
                    );
                    succeeds(cli(&f.local, "none"));
                } else {
                    if let Some(path) = interrupt {
                        fs::remove_file(path).unwrap();
                    } else {
                        fs::write(f.local.path().join("base"), "resolved both changes\n").unwrap();
                        f.local.success(&["add", "base"]);
                    }
                    f.local
                        .success(&["-c", "core.editor=true", "rebase", "--continue"]);
                    assert!(matches!(
                        fresh(&f.local, &request(FinishUpdate::None)).recovery,
                        Recovery::Verified(_)
                    ));
                    let out = succeeds(cli(&f.local, "none"));
                    assert!(out.contains("team/project/main"));
                }
                assert_eq!(oid(&f.fork, REF), oid(&f.local, "HEAD"));
            }
        },
    );
}

#[test]
fn partial_publication_and_failed_push_retain_exact_proof_across_processes() {
    isolated(
        "partial_publication_and_failed_push_retain_exact_proof_across_processes",
        true,
        |root| {
            for multiple in [false, true] {
                let f = Fixture::new(root, false, true, false);
                let second = Repository::new(root);
                seed(&f.local, &second);
                if multiple {
                    f.local.success(&[
                        "config",
                        "--add",
                        &format!("remote.{PUBLISH}.pushurl"),
                        second.path().to_str().unwrap(),
                    ]);
                }
                let rejected = if multiple { &second } else { &f.fork };
                let reject = hook(rejected, "pre-receive", "exit 1");
                assert!(!cli(&f.local, "rebase").status.success());
                let prepared = oid(&f.local, "HEAD");
                assert_ne!(prepared, f.original);
                assert_eq!(oid(rejected, REF), f.original);
                if multiple {
                    assert_eq!(oid(&f.fork, REF), prepared);
                }
                let record = fs::read(f.record()).unwrap();
                fs::remove_file(reject).unwrap();
                let retry = fresh(&f.local, &request(FinishUpdate::None));
                assert!(retry.is_ready(), "{retry:#?}");
                assert_eq!(
                    retry.destinations.last().unwrap().mode,
                    PublicationMode::VerifiedRewrite
                );
                // The record proves history, but carries no reusable approval.
                let ask = fresh(
                    &f.local,
                    &Request {
                        update: Some(FinishUpdate::None),
                        rewrite: None,
                    },
                );
                assert!(ask.required_input.contains(&RequiredInput::Rewrite));
                succeeds(cli(&f.local, "none"));
                assert_eq!(oid(rejected, REF), prepared);
                assert_eq!(fs::read(f.record()).unwrap(), record);
                assert_eq!(oid(&f.project, REF), f.original);
            }
        },
    );
}

#[test]
fn new_fork_work_cannot_borrow_project_or_rewrite_observations() {
    isolated(
        "new_fork_work_cannot_borrow_project_or_rewrite_observations",
        true,
        |root| {
            for rewritten in [false, true] {
                let f = Fixture::new(root, false, false, false);
                if rewritten {
                    prepared(&f.local, FinishUpdate::Rebase);
                }
                f.fork.success(&["switch", BRANCH]);
                let new_work = commit(&f.fork, "remote-only", "unincorporated fork work");
                let plan = fresh(&f.local, &request(FinishUpdate::Rebase));
                assert!(!plan.is_ready());
                assert_eq!(
                    plan.destinations[0].observed,
                    RefObservation::Commit(new_work)
                );
                assert!(
                    plan.constraints
                        .iter()
                        .any(|c| matches!(c, Constraint::RemoteWorkNotIncorporated(_)))
                );
            }
        },
    );
}

#[test]
fn legacy_records_retry_only_with_original_equal_roles_and_destinations() {
    isolated(
        "legacy_records_retry_only_with_original_equal_roles_and_destinations",
        true,
        |root| {
            for change in [
                "compatible",
                "base-role",
                "publish-role",
                "destination",
                "original",
                "reflog",
                "missing-bindings",
                "unknown-version",
            ] {
                let f = Fixture::new(root, true, false, false);
                prepared(&f.local, FinishUpdate::Rebase);
                f.legacy();
                let legacy = fs::read(f.record()).unwrap();
                let other = Repository::new(root);
                match change {
                    "base-role" => f.settings(PROJECT, PUBLISH),
                    "publish-role" => {
                        f.local.success(&[
                            "remote",
                            "add",
                            "new-publish",
                            f.fork.path().to_str().unwrap(),
                        ]);
                        f.settings("new-publish", "new-publish");
                    }
                    "destination" => {
                        other.success(&[
                            "fetch",
                            f.fork.path().to_str().unwrap(),
                            &format!("{REF}:{REF}"),
                        ]);
                        f.local.success(&[
                            "remote",
                            "set-url",
                            "--push",
                            PUBLISH,
                            other.path().to_str().unwrap(),
                        ]);
                    }
                    "original" => {
                        let mut value: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
                        value["original"] = serde_json::to_value(oid(&f.local, "HEAD")).unwrap();
                        fs::write(f.record(), serde_json::to_vec(&value).unwrap()).unwrap();
                    }
                    "missing-bindings" | "unknown-version" => {
                        let mut value: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
                        value["version"] = if change == "missing-bindings" { 2 } else { 99 }.into();
                        fs::write(f.record(), serde_json::to_vec(&value).unwrap()).unwrap();
                    }
                    "reflog" => {
                        f.local
                            .success(&["reflog", "expire", "--expire=now", "--all"]);
                    }
                    _ => (),
                }
                let plan = fresh(&f.local, &request(FinishUpdate::None));
                if change == "compatible" {
                    assert!(matches!(plan.recovery, Recovery::Verified(_)), "{plan:#?}");
                    assert_eq!(plan.destinations[0].mode, PublicationMode::VerifiedRewrite);
                    succeeds(cli(&f.local, "none"));
                    assert_eq!(fs::read(f.record()).unwrap(), legacy);
                } else {
                    assert_eq!(plan.recovery, Recovery::Invalid, "{change}");
                    assert!(!plan.is_ready(), "{change}: {plan:#?}");
                    assert_eq!(oid(&f.fork, REF), f.original);
                }
            }
        },
    );
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
#[test]
fn split_role_executor_death_and_lost_acknowledgement_need_no_final_write() {
    isolated(
        "split_role_executor_death_and_lost_acknowledgement_need_no_final_write",
        true,
        |root| {
            for mode in ["rebase-killed", "push-killed", "push-lost-ack"] {
                let f = Fixture::new(root, false, true, false);
                let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                    .map(|p| p.join("git"))
                    .find(|p| p.is_file())
                    .unwrap();
                let wrapper = f.local.path().join(".git/wrapper");
                fs::create_dir(&wrapper).unwrap();
                let command = if mode == "rebase-killed" {
                    "rebase"
                } else {
                    "push"
                };
                let after = if mode.ends_with("killed") {
                    "kill -KILL \"$PPID\""
                } else {
                    "exit 1"
                };
                fs::write(wrapper.join("git"), format!(
                "#!/bin/sh\nfor arg do\n if [ \"$arg\" = {command} ]; then\n  {} \"$@\" >/dev/null\n  code=$?\n  if [ \"$code\" = 0 ]; then {after}; fi\n  exit \"$code\"\n fi\ndone\nexec {} \"$@\"\n",
                quote(real_git.to_str().unwrap()), quote(real_git.to_str().unwrap())
            )).unwrap();
                fs::set_permissions(wrapper.join("git"), fs::Permissions::from_mode(0o755))
                    .unwrap();
                let paths = std::env::join_paths(
                    std::iter::once(wrapper)
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap();
                let out = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
                    .current_dir(f.local.path())
                    .args(["finish", "--update", "rebase", "--allow-rewrite"])
                    .env("PATH", paths)
                    .output()
                    .unwrap();
                assert!(!out.status.success(), "{mode}: {out:?}");
                let p = fresh(&f.local, &request(FinishUpdate::None));
                assert!(
                    matches!(p.recovery, Recovery::Verified(_)),
                    "{mode}: {p:#?}"
                );
                let record = fs::read(f.record()).unwrap();
                if mode == "rebase-killed" {
                    assert_eq!(oid(&f.fork, REF), f.original);
                    assert_eq!(p.destinations[0].mode, PublicationMode::VerifiedRewrite);
                } else {
                    assert_eq!(oid(&f.fork, REF), p.commit);
                }
                succeeds(cli(&f.local, "none"));
                assert_eq!(oid(&f.fork, REF), p.commit);
                assert_eq!(fs::read(f.record()).unwrap(), record);
            }
        },
    );
}

#[test]
fn advancing_project_extends_split_and_legacy_recovery_without_upgrading_old_bindings() {
    isolated(
        "advancing_project_extends_split_and_legacy_recovery_without_upgrading_old_bindings",
        true,
        |root| {
            for legacy in [false, true] {
                let f = Fixture::new(root, legacy, true, false);
                let second = Repository::new(root);
                seed(&f.local, &second);
                f.local.success(&[
                    "config",
                    "--add",
                    &format!("remote.{PUBLISH}.pushurl"),
                    second.path().to_str().unwrap(),
                ]);
                prepared(&f.local, FinishUpdate::Rebase);
                if legacy {
                    f.legacy();
                }
                second.success(&[
                    "fetch",
                    f.local.path().to_str().unwrap(),
                    &format!("+{REF}:{REF}"),
                ]);
                let base = commit(
                    &f.project,
                    "project-again",
                    "base advances after partial publication",
                );
                let retry = fresh(&f.local, &request(FinishUpdate::Rebase));
                assert!(retry.is_ready(), "{retry:#?}");
                assert_eq!(
                    retry.evidence.as_ref().unwrap().base,
                    RefObservation::Commit(base)
                );
                let p = prepared(&f.local, FinishUpdate::Rebase);
                assert!(
                    p.destinations
                        .iter()
                        .all(|d| d.mode == PublicationMode::VerifiedRewrite)
                );
                let record: serde_json::Value =
                    serde_json::from_slice(&fs::read(f.record()).unwrap()).unwrap();
                assert_eq!(record["version"], if legacy { 1 } else { 2 });
                assert_eq!(record["attempts"].as_array().unwrap().len(), 2);
                assert_eq!(record.get("roles").is_some(), !legacy);
                succeeds(cli(&f.local, "none"));
                assert_eq!(oid(&f.fork, REF), p.commit);
                assert_eq!(oid(&second, REF), p.commit);
            }
        },
    );
}

#[test]
fn base_endpoint_rewrites_and_role_diagnostics_keep_credentials_redacted() {
    isolated(
        "base_endpoint_rewrites_and_role_diagnostics_keep_credentials_redacted",
        true,
        |root| {
            let f = Fixture::new(root, false, true, false);
            f.local.success(&[
                "config",
                &format!("url.{}.insteadOf", f.project.path().display()),
                "project-alias:",
            ]);
            f.local
                .success(&["remote", "set-url", PROJECT, "project-alias:"]);
            // Extra base URLs and push URLs must never choose the fork base.
            f.local.success(&[
                "config",
                "--add",
                &format!("remote.{PROJECT}.url"),
                f.fork.path().to_str().unwrap(),
            ]);
            f.local.success(&[
                "config",
                &format!("remote.{PROJECT}.pushurl"),
                "/unavailable/project-push",
            ]);
            let p = prepared(&f.local, FinishUpdate::Rebase);
            assert_eq!(
                p.base_destination,
                f.project.path().as_os_str().as_encoded_bytes()
            );
            assert_eq!(
                p.evidence.unwrap().base,
                RefObservation::Commit(f.base.clone())
            );
            let helper = f.local.path().join(".git/failing-upload-pack");
            fs::write(&helper, "#!/bin/sh\necho 'fatal: project https://role-user:role-secret@example.invalid/project' >&2\nexit 1\n").unwrap();
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
            f.local.success(&[
                "config",
                &format!("remote.{PROJECT}.uploadpack"),
                &quote(helper.to_str().unwrap()),
            ]);
            let output = cli(&f.local, "none");
            assert!(!output.status.success());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stderr.contains("role-secret"), "{stderr}");
            assert!(
                stderr.contains("<redacted>@example.invalid/project"),
                "{stderr}"
            );
            let output = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
                .current_dir(f.local.path())
                .args(["finish", "--update", "none", "--show-credentials"])
                .output()
                .unwrap();
            assert!(String::from_utf8_lossy(&output.stderr).contains("role-secret"));
        },
    );
}
