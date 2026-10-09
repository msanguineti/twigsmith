#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use git_oneflow::{
    finish, git,
    save::{self, Request, SaveError, execution, publication},
    settings::SaveAction,
    start::{self, Action, StartError},
};
use preservation::snapshot;
use std::{
    fs,
    path::{Path, PathBuf},
};
use support::{Repository, isolated};

const PROJECT: &str = "team/project";
const FORK: &str = "personal/publish";
const BRANCH: &str = "Feature/Exact_Name";
const REF: &str = "refs/heads/Feature/Exact_Name";

fn commit(repo: &Repository, content: &str) -> Vec<u8> {
    fs::write(repo.path().join("tracked"), content).unwrap();
    repo.success(&["add", "tracked"]);
    repo.success(&["commit", "-m", content]);
    oid(repo.path(), "HEAD").unwrap()
}
fn oid(path: &Path, reference: &str) -> Option<Vec<u8>> {
    let output = git::run(path, ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| output.stdout.strip_suffix(b"\n").unwrap().to_vec())
}
fn bare(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    assert!(
        git::run(root, ["init".as_ref(), "--bare".as_ref(), path.as_os_str()])
            .unwrap()
            .status
            .success()
    );
    path
}
struct Fixture {
    local: Repository,
    project: PathBuf,
    fork: PathBuf,
    initial: Vec<u8>,
    base: Vec<u8>,
    fork_base: Vec<u8>,
}
impl Fixture {
    fn new(root: &Path) -> Self {
        let local = Repository::new(root);
        let initial = commit(&local, "local base");
        let project = bare(
            root,
            &format!(
                "project-{}.git",
                local.path().file_name().unwrap().to_string_lossy()
            ),
        );
        let fork = bare(
            root,
            &format!(
                "fork-{}.git",
                local.path().file_name().unwrap().to_string_lossy()
            ),
        );
        for (name, path) in [(PROJECT, &project), (FORK, &fork)] {
            local.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                name.as_ref(),
                path.as_os_str(),
            ]);
        }
        local.success(&["switch", "-c", "seed"]);
        let base = commit(&local, "fresh project base");
        local.success(&["push", PROJECT, "HEAD:refs/heads/trunk"]);
        let fork_base = commit(&local, "independent fork base");
        local.success(&["push", FORK, "HEAD:refs/heads/trunk"]);
        local.success(&["switch", "main"]);
        fs::write(
            local.path().join(".git/git-oneflow.config.toml"),
            format!("publish-remote = {FORK:?}\nbase-remote = {PROJECT:?}\nbase = \"trunk\"\n"),
        )
        .unwrap();
        Self {
            local,
            project,
            fork,
            initial,
            base,
            fork_base,
        }
    }
    fn start(&self) -> Result<start::Outcome, Box<start::Failure>> {
        start::execute(
            self.local.path(),
            &start::prepare(self.local.path(), BRANCH).unwrap(),
        )
    }
    fn work(&self) {
        self.local.success(&["switch", "-c", BRANCH]);
    }
    fn upstream(&self, remote: &str, reference: &str) {
        self.local
            .success(&["config", &format!("branch.{BRANCH}.remote"), remote]);
        self.local
            .success(&["config", &format!("branch.{BRANCH}.merge"), reference]);
    }
    fn plan(&self) -> save::Plan {
        save::assess(
            self.local.path(),
            &Request {
                action: Some(SaveAction::PublishOnly),
                message: None,
            },
        )
        .unwrap()
    }
    fn assert_bases(&self) {
        assert_eq!(
            oid(self.local.path(), "refs/heads/main"),
            Some(self.initial.clone())
        );
        assert_eq!(
            oid(&self.project, "refs/heads/trunk"),
            Some(self.base.clone())
        );
        assert_eq!(
            oid(&self.fork, "refs/heads/trunk"),
            Some(self.fork_base.clone())
        );
    }
}

#[test]
fn new_work_and_exact_fork_resume_keep_all_base_history() {
    isolated(
        "new_work_and_exact_fork_resume_keep_all_base_history",
        true,
        |root| {
            for (project_branch, fork_branch) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let f = Fixture::new(root);
                if project_branch {
                    f.local.success(&["push", PROJECT, &format!("seed:{REF}")]);
                }
                let published = if fork_branch {
                    f.local.success(&["switch", "seed"]);
                    let published = commit(&f.local, "published work");
                    f.local.success(&["push", FORK, &format!("HEAD:{REF}")]);
                    f.local.success(&["switch", "main"]);
                    Some(published)
                } else {
                    None
                };
                let project_refs = git::run(&f.project, ["show-ref"]).unwrap().stdout;
                let fork_refs = git::run(&f.fork, ["show-ref"]).unwrap().stdout;
                // Restrictive fetch mappings and automatic tracking cannot select a
                // different branch or supply tracking to newly created work.
                f.local.success(&[
                    "config",
                    &format!("remote.{PROJECT}.fetch"),
                    "+refs/heads/unrelated:refs/remotes/team/project/unrelated",
                ]);
                f.local
                    .success(&["config", "branch.autoSetupMerge", "always"]);
                let outcome = f.start().unwrap();
                assert_eq!(
                    outcome.action,
                    if fork_branch {
                        Action::ResumeRemote
                    } else {
                        Action::Create
                    }
                );
                assert_eq!(
                    oid(f.local.path(), "HEAD"),
                    Some(published.unwrap_or(f.base.clone()))
                );
                let tracking = git::run(
                    f.local.path(),
                    ["config", "--get", &format!("branch.{BRANCH}.remote")],
                )
                .unwrap();
                if fork_branch {
                    assert_eq!(tracking.stdout, format!("{FORK}\n").as_bytes());
                    assert_eq!(
                        f.local
                            .success(&["config", "--get", &format!("branch.{BRANCH}.merge")])
                            .stdout,
                        format!("{REF}\n").as_bytes()
                    );
                } else {
                    assert_eq!(tracking.status.code(), Some(1));
                }
                f.assert_bases();
                assert_eq!(
                    git::run(&f.project, ["show-ref"]).unwrap().stdout,
                    project_refs
                );
                assert_eq!(git::run(&f.fork, ["show-ref"]).unwrap().stdout, fork_refs);
            }
        },
    );
}

#[test]
fn local_resume_and_current_branch_work_offline_with_both_tracking_styles() {
    isolated(
        "local_resume_and_current_branch_work_offline_with_both_tracking_styles",
        true,
        |root| {
            for tracking in [None, Some((FORK, REF)), Some((PROJECT, "refs/heads/trunk"))] {
                let f = Fixture::new(root);
                f.work();
                let work = commit(&f.local, "local work");
                if let Some((remote, reference)) = tracking {
                    f.upstream(remote, reference);
                }
                f.local
                    .success(&["config", &format!("branch.{BRANCH}.rebase"), "true"]);
                for remote in [PROJECT, FORK] {
                    f.local
                        .success(&["remote", "set-url", remote, "/missing/offline-remote"]);
                }
                let config = fs::read(f.local.path().join(".git/config")).unwrap();
                f.local.success(&["switch", "main"]);
                assert_eq!(f.start().unwrap().action, Action::ResumeLocal);
                assert_eq!(oid(f.local.path(), "HEAD"), Some(work));
                assert_eq!(
                    fs::read(f.local.path().join(".git/config")).unwrap(),
                    config
                );
                fs::write(f.local.path().join("tracked"), "uncommitted").unwrap();
                let before = snapshot(root);
                assert_eq!(f.start().unwrap().action, Action::AlreadyCurrent);
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn discovery_failure_is_not_absence_and_fork_resume_needs_no_project_access() {
    isolated(
        "discovery_failure_is_not_absence_and_fork_resume_needs_no_project_access",
        true,
        |root| {
            for unavailable in [PROJECT, FORK] {
                let f = Fixture::new(root);
                f.local.success(&[
                    "remote",
                    "set-url",
                    unavailable,
                    "/missing/discovery-remote",
                ]);
                let before = snapshot(root);
                let failure = f.start().unwrap_err();
                assert!(matches!(failure.error, StartError::Git { .. }));
                assert!(failure.effects.is_empty());
                assert_eq!(snapshot(root), before);
            }
            let f = Fixture::new(root);
            f.local.success(&["push", FORK, &format!("seed:{REF}")]);
            f.local.success(&["remote", "remove", PROJECT]);
            assert_eq!(f.start().unwrap().action, Action::ResumeRemote);
        },
    );
}

#[test]
fn save_preserves_both_styles_and_establishes_only_missing_tracking_without_base_access() {
    isolated(
        "save_preserves_both_styles_and_establishes_only_missing_tracking_without_base_access",
        true,
        |root| {
            for tracking in [None, Some((FORK, REF)), Some((PROJECT, "refs/heads/trunk"))] {
                let f = Fixture::new(root);
                f.work();
                if let Some((remote, reference)) = tracking {
                    f.upstream(remote, reference);
                }
                f.local
                    .success(&["remote", "set-url", PROJECT, "/missing/project-remote"]);
                f.local.success(&["branch", "unrelated"]);
                f.local.success(&["tag", "unrelated-tag"]);
                f.local.success(&[
                    "config",
                    &format!("remote.{FORK}.push"),
                    "+refs/heads/*:refs/heads/*",
                ]);
                f.local.success(&["config", "push.default", "upstream"]);
                let config = fs::read(f.local.path().join(".git/config")).unwrap();
                let report = publication::publish(f.local.path(), &f.plan());
                assert!(report.is_complete(), "{report:?}");
                assert_eq!(oid(&f.fork, REF), Some(f.initial.clone()));
                assert_eq!(oid(&f.project, REF), None);
                assert_eq!(oid(&f.fork, "refs/heads/unrelated"), None);
                assert_eq!(oid(&f.fork, "refs/tags/unrelated-tag"), None);
                if tracking.is_some() {
                    assert_eq!(
                        fs::read(f.local.path().join(".git/config")).unwrap(),
                        config
                    );
                    assert!(report.tracking.writes.is_empty());
                    assert_eq!(
                        report.tracking.before.as_ref().unwrap().upstream,
                        report.tracking.after.as_ref().unwrap().upstream
                    );
                } else {
                    assert_eq!(
                        report.tracking.writes,
                        [
                            publication::TrackingWrite::Remote,
                            publication::TrackingWrite::MergeRef
                        ]
                    );
                    assert_eq!(
                        report.tracking.after.as_ref().unwrap().upstream.remotes,
                        [FORK.as_bytes()]
                    );
                }
                f.assert_bases();
            }
        },
    );
}

#[test]
fn shared_tracking_guards_refuse_incomplete_duplicate_and_unrelated_settings() {
    isolated(
        "shared_tracking_guards_refuse_incomplete_duplicate_and_unrelated_settings",
        true,
        |root| {
            for (remotes, refs) in [
                (vec![FORK], vec![]),
                (vec![], vec![REF]),
                (vec![FORK, FORK], vec![REF]),
                (vec![PROJECT], vec!["refs/heads/trunk", "refs/heads/trunk"]),
                (vec![PROJECT], vec![REF]),
                (vec![FORK], vec!["refs/heads/trunk"]),
                (vec!["other"], vec![REF]),
                (vec!["."], vec![REF]),
            ] {
                let f = Fixture::new(root);
                f.work();
                for remote in remotes {
                    f.local.success(&[
                        "config",
                        "--add",
                        &format!("branch.{BRANCH}.remote"),
                        remote,
                    ]);
                }
                for reference in refs {
                    f.local.success(&[
                        "config",
                        "--add",
                        &format!("branch.{BRANCH}.merge"),
                        reference,
                    ]);
                }
                let before = snapshot(root);
                assert!(matches!(
                    save::assess(f.local.path(), &Request::default()),
                    Err(SaveError::InvalidUpstream)
                ));
                assert!(matches!(
                    f.start().unwrap_err().error,
                    StartError::Tracking(start::TrackingError::InvalidUpstream)
                ));
                assert_eq!(snapshot(root), before);
                f.local.success(&["switch", "main"]);
                let before = snapshot(root);
                assert!(matches!(
                    f.start().unwrap_err().error,
                    StartError::Tracking(start::TrackingError::InvalidUpstream)
                ));
                assert_eq!(snapshot(root), before);
            }
        },
    );
}

#[test]
fn explicit_push_selection_obeys_real_git_precedence_without_changing_git_routing() {
    isolated(
        "explicit_push_selection_obeys_real_git_precedence_without_changing_git_routing",
        true,
        |root| {
            for (defaults, branch, effective) in [
                (vec![], vec![], PROJECT),
                (vec![FORK], vec![], FORK),
                (vec![PROJECT], vec![FORK], FORK),
                (vec![FORK], vec![PROJECT], PROJECT),
                (vec![PROJECT, FORK], vec![], FORK),
                (vec![FORK], vec![PROJECT, FORK], FORK),
                (vec![FORK, PROJECT], vec![], PROJECT),
            ] {
                let f = Fixture::new(root);
                f.work();
                f.upstream(PROJECT, "refs/heads/trunk");
                f.local.success(&["config", "push.default", "current"]);
                for value in &defaults {
                    f.local
                        .success(&["config", "--add", "remote.pushDefault", value]);
                }
                for value in &branch {
                    f.local.success(&[
                        "config",
                        "--add",
                        &format!("branch.{BRANCH}.pushRemote"),
                        value,
                    ]);
                }
                let ordinary = f.local.success(&["push", "--dry-run", "--porcelain"]);
                let destination = if effective == FORK {
                    &f.fork
                } else {
                    &f.project
                };
                assert!(
                    String::from_utf8_lossy(&ordinary.stdout)
                        .contains(destination.to_str().unwrap()),
                    "{ordinary:?}"
                );
                let before = snapshot(root);
                // An implicit branch.remote/default fallback is compatible with
                // separate publication; only an effective explicit override conflicts.
                let valid = defaults.is_empty() && branch.is_empty() || effective == FORK;
                let assessed = save::assess(f.local.path(), &Request::default());
                if valid {
                    let report = publication::publish(f.local.path(), &assessed.unwrap());
                    assert!(report.is_complete(), "{report:?}");
                    assert_eq!(oid(&f.fork, REF), Some(f.initial.clone()));
                    assert_eq!(oid(&f.project, REF), None);
                } else {
                    assert!(matches!(assessed, Err(SaveError::ConflictingPushRemote)));
                    assert!(matches!(
                        f.start().unwrap_err().error,
                        StartError::Tracking(start::TrackingError::ConflictingPushRemote)
                    ));
                    assert_eq!(snapshot(root), before);
                }
            }
            for invalid in ["", ".", "unknown", "/a/repository/url"] {
                let f = Fixture::new(root);
                f.work();
                f.local
                    .success(&["config", &format!("branch.{BRANCH}.pushRemote"), invalid]);
                assert!(matches!(
                    save::assess(f.local.path(), &Request::default()),
                    Err(SaveError::ConflictingPushRemote)
                ));
            }
        },
    );
}

#[test]
fn routing_changes_invalidate_start_save_and_checkpoint_approval() {
    isolated(
        "routing_changes_invalidate_start_save_and_checkpoint_approval",
        true,
        |root| {
            for (key, value) in [
                (format!("branch.{BRANCH}.remote"), PROJECT),
                (format!("branch.{BRANCH}.pushRemote"), FORK),
                ("remote.pushDefault".into(), FORK),
                (format!("remote.{FORK}.pushurl"), "/missing/new-publication"),
            ] {
                let f = Fixture::new(root);
                f.work();
                fs::write(f.local.path().join("tracked"), "checkpoint").unwrap();
                let save = save::assess(
                    f.local.path(),
                    &Request {
                        action: Some(SaveAction::Wip),
                        message: None,
                    },
                )
                .unwrap();
                let start = start::prepare(f.local.path(), BRANCH).unwrap();
                f.local.success(&["config", &key, value]);
                let before = snapshot(root);
                assert!(save::revalidate(f.local.path(), &save).is_err());
                let report = execution::execute(f.local.path(), &save);
                assert!(report.error.is_some());
                assert!(report.commands.is_empty());
                assert!(report.effects.is_empty());
                if !key.ends_with("pushurl") {
                    assert!(
                        start::execute(f.local.path(), &start)
                            .unwrap_err()
                            .effects
                            .is_empty()
                    );
                }
                assert_eq!(snapshot(root), before);
            }
            // Both styles are valid, but changing between them is still a new plan.
            let f = Fixture::new(root);
            f.work();
            f.upstream(PROJECT, "refs/heads/trunk");
            let plan = f.plan();
            f.upstream(FORK, REF);
            assert!(matches!(
                save::revalidate(f.local.path(), &plan),
                Err(SaveError::ContextChanged)
            ));
            // Even a masked lower-precedence setting is part of revalidation.
            f.local
                .success(&["config", &format!("branch.{BRANCH}.pushRemote"), FORK]);
            let plan = f.plan();
            f.local.success(&["config", "remote.pushDefault", PROJECT]);
            assert!(matches!(
                save::revalidate(f.local.path(), &plan),
                Err(SaveError::ContextChanged)
            ));
        },
    );
}

#[test]
fn equal_roles_accept_base_tracking_and_split_roles_still_guard_finish() {
    isolated(
        "equal_roles_accept_base_tracking_and_split_roles_still_guard_finish",
        true,
        |root| {
            let f = Fixture::new(root);
            f.work();
            let before = snapshot(root);
            assert!(
                finish::assess(f.local.path(), &finish::Request::default(), None)
                    .unwrap_err()
                    .to_string()
                    .contains("matching remote roles")
            );
            assert_eq!(snapshot(root), before);
            fs::write(
                f.local.path().join(".git/git-oneflow.config.toml"),
                format!("publish-remote = {FORK:?}\nbase = \"trunk\"\n"),
            )
            .unwrap();
            f.upstream(FORK, "refs/heads/trunk");
            assert!(publication::publish(f.local.path(), &f.plan()).is_complete());
            assert_eq!(
                f.local
                    .success(&["config", "--get", &format!("branch.{BRANCH}.merge")])
                    .stdout,
                b"refs/heads/trunk\n"
            );
            assert!(finish::assess(f.local.path(), &finish::Request::default(), None).is_ok());
        },
    );
}

#[cfg(unix)]
fn executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(unix)]
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[cfg(unix)]
#[test]
fn start_revalidates_routes_and_destinations_after_discovery_and_fetch() {
    isolated(
        "start_revalidates_routes_and_destinations_after_discovery_and_fetch",
        true,
        |root| {
            for during_fetch in [false, true] {
                for change in [
                    "tracking",
                    "branch-push",
                    "default-push",
                    "base-url",
                    "publish-url",
                    "roles",
                ] {
                    let f = Fixture::new(root);
                    let script = f.local.path().join(".git/upload-probe");
                    let counter = f.local.path().join(".git/count");
                    let command = match change {
                        "tracking" => format!("git config branch.{BRANCH}.remote {FORK}"),
                        "branch-push" => format!("git config branch.{BRANCH}.pushRemote {FORK}"),
                        "default-push" => format!("git config remote.pushDefault {FORK}"),
                        "base-url" => format!("git remote set-url {PROJECT} /missing/changed-base"),
                        "publish-url" => {
                            format!("git remote set-url {FORK} /missing/changed-publish")
                        }
                        _ => format!(
                            "printf 'publish-remote = \"{FORK}\"\\nbase-remote = \"{FORK}\"\\nbase = \"trunk\"\\n' > .git/git-oneflow.config.toml"
                        ),
                    };
                    executable(
                        &script,
                        &format!(
                            "#!/bin/sh\ncount=0\nif test -f {counter}; then count=$(cat {counter}); fi\ncount=$((count + 1))\nprintf '%s' \"$count\" > {counter}\nif test \"$count\" = {when}; then\n{command}\nfi\nexec git-upload-pack \"$@\"\n",
                            counter = quoted(&counter),
                            when = if during_fetch { 2 } else { 1 }
                        ),
                    );
                    // Base discovery is followed by base fetch. Publication discovery
                    // uses its own untouched transport, including for new work.
                    f.local.success(&[
                        "config",
                        &format!("remote.{PROJECT}.uploadpack"),
                        &quoted(&script),
                    ]);
                    let failure = f.start().unwrap_err();
                    assert!(
                        matches!(failure.error, StartError::Changed),
                        "{change}: {failure:?}"
                    );
                    assert_eq!(failure.effects.len(), usize::from(during_fetch));
                    assert_eq!(oid(f.local.path(), REF), None);
                    f.assert_bases();
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn save_revalidates_push_selection_after_checkpoints_and_before_tracking_writes() {
    isolated(
        "save_revalidates_push_selection_after_checkpoints_and_before_tracking_writes",
        true,
        |root| {
            for checkpoint in [false, true] {
                for tracked in [false, true] {
                    let f = Fixture::new(root);
                    f.work();
                    if tracked {
                        f.upstream(PROJECT, "refs/heads/trunk");
                    }
                    let hooks = f.local.path().join(".git/probe-hooks");
                    fs::create_dir(&hooks).unwrap();
                    f.local.success(&[
                        "config".as_ref(),
                        "core.hooksPath".as_ref(),
                        hooks.as_os_str(),
                    ]);
                    executable(
                        &hooks.join(if checkpoint {
                            "post-commit"
                        } else {
                            "pre-push"
                        }),
                        &format!("#!/bin/sh\ngit config branch.{BRANCH}.pushRemote {PROJECT}\n"),
                    );
                    if checkpoint {
                        fs::write(f.local.path().join("tracked"), "checkpoint work").unwrap();
                        let plan = save::assess(
                            f.local.path(),
                            &Request {
                                action: Some(SaveAction::Wip),
                                message: None,
                            },
                        )
                        .unwrap();
                        let report = execution::execute(f.local.path(), &plan);
                        assert!(report.error.is_some(), "{report:?}");
                        assert!(report.publication.is_none());
                        assert_eq!(oid(&f.fork, REF), None);
                        assert_ne!(oid(f.local.path(), "HEAD"), Some(f.initial.clone()));
                    } else {
                        let report = publication::publish(f.local.path(), &f.plan());
                        assert_eq!(report.state, publication::PublicationState::Complete);
                        assert!(
                            matches!(
                                report.error,
                                Some(publication::PublishError::Save(
                                    SaveError::ConflictingPushRemote
                                ))
                            ),
                            "{report:?}"
                        );
                        assert!(report.tracking.writes.is_empty());
                        assert_eq!(oid(&f.fork, REF), Some(f.initial.clone()));
                    }
                    f.assert_bases();
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn valid_tracking_changed_during_push_is_reported_without_repair() {
    isolated(
        "valid_tracking_changed_during_push_is_reported_without_repair",
        true,
        |root| {
            let f = Fixture::new(root);
            f.work();
            f.upstream(PROJECT, "refs/heads/trunk");
            let hooks = f.local.path().join(".git/probe-hooks");
            fs::create_dir(&hooks).unwrap();
            f.local.success(&[
                "config".as_ref(),
                "core.hooksPath".as_ref(),
                hooks.as_os_str(),
            ]);
            executable(
                &hooks.join("pre-push"),
                &format!(
                    "#!/bin/sh\ngit config branch.{BRANCH}.remote {FORK}\ngit config branch.{BRANCH}.merge {REF}\n"
                ),
            );
            let report = publication::publish(f.local.path(), &f.plan());
            assert_eq!(report.state, publication::PublicationState::Complete);
            assert!(matches!(
                report.error,
                Some(publication::PublishError::Save(SaveError::ContextChanged))
            ));
            assert!(report.tracking.writes.is_empty());
            assert_eq!(
                report.tracking.after.as_ref().unwrap().upstream.remotes,
                [FORK.as_bytes()]
            );
            assert_eq!(oid(&f.fork, REF), Some(f.initial));
        },
    );
}

#[test]
fn symbolic_fetch_destinations_preserve_local_base_and_existing_work() {
    isolated(
        "symbolic_fetch_destinations_preserve_local_base_and_existing_work",
        true,
        |root| {
            for resume in [false, true] {
                let f = Fixture::new(root);
                if resume {
                    f.local.success(&["push", FORK, &format!("seed:{REF}")]);
                }
                f.local.success(&["branch", "precious"]);
                let destination = if resume {
                    format!("refs/remotes/{FORK}/{BRANCH}")
                } else {
                    format!("refs/remotes/{PROJECT}/trunk")
                };
                f.local
                    .success(&["symbolic-ref", &destination, "refs/heads/precious"]);
                let before = snapshot(root);
                assert!(matches!(
                    f.start().unwrap_err().error,
                    StartError::SymbolicTrackingRef(_)
                ));
                assert_eq!(snapshot(root), before);
                assert_eq!(
                    oid(f.local.path(), "refs/heads/precious"),
                    Some(f.initial.clone())
                );
                f.assert_bases();
            }
        },
    );
}

#[test]
fn cli_start_and_save_use_split_roles_and_report_explicit_push_conflicts() {
    isolated(
        "cli_start_and_save_use_split_roles_and_report_explicit_push_conflicts",
        true,
        |root| {
            use std::process::{Command, Stdio};
            for executable in [env!("CARGO_BIN_EXE_gof"), env!("CARGO_BIN_EXE_git-oneflow")] {
                for (start_command, save_command) in [("start", "save"), ("s", "v")] {
                    let f = Fixture::new(root);
                    let run = |args: &[&str]| {
                        Command::new(executable)
                            .current_dir(f.local.path())
                            .args(args)
                            .stdin(Stdio::null())
                            .output()
                            .unwrap()
                    };
                    let started = run(&[start_command, BRANCH]);
                    assert!(started.status.success(), "{started:?}");
                    assert_eq!(oid(f.local.path(), "HEAD"), Some(f.base.clone()));
                    f.upstream(PROJECT, "refs/heads/trunk");
                    f.local
                        .success(&["remote", "set-url", PROJECT, "/missing/project-remote"]);
                    fs::write(f.local.path().join("tracked"), "checkpoint via CLI").unwrap();
                    let saved = run(&[save_command, "--wip"]);
                    assert!(saved.status.success(), "{saved:?}");
                    assert_eq!(oid(&f.fork, REF), oid(f.local.path(), "HEAD"));
                    assert_eq!(
                        f.local
                            .success(&["config", "--get", &format!("branch.{BRANCH}.remote")])
                            .stdout,
                        format!("{PROJECT}\n").as_bytes()
                    );
                    f.local.success(&["config", "remote.pushDefault", PROJECT]);
                    let before = snapshot(root);
                    for args in [vec![start_command, BRANCH], vec![save_command, "--wip"]] {
                        let refused = run(&args);
                        assert_eq!(refused.status.code(), Some(1), "{refused:?}");
                        assert!(
                            String::from_utf8_lossy(&refused.stderr)
                                .contains("explicit Git push remote conflicts")
                        );
                        assert_eq!(snapshot(root), before);
                    }
                    f.assert_bases();
                }
            }
        },
    );
}

#[test]
fn split_role_save_rejects_remote_work_without_forcing_or_changing_base_tracking() {
    isolated(
        "split_role_save_rejects_remote_work_without_forcing_or_changing_base_tracking",
        true,
        |root| {
            let f = Fixture::new(root);
            f.local.success(&["push", FORK, &format!("seed:{REF}")]);
            f.work();
            f.upstream(PROJECT, "refs/heads/trunk");
            let config = fs::read(f.local.path().join(".git/config")).unwrap();
            fs::write(f.local.path().join("tracked"), "precious unpublished work").unwrap();
            let report = publication::publish(f.local.path(), &f.plan());
            assert_eq!(
                report.state,
                publication::PublicationState::Rejected,
                "{report:?}"
            );
            assert!(report.tracking.writes.is_empty());
            assert_eq!(oid(&f.fork, REF), Some(f.fork_base.clone()));
            assert_eq!(
                fs::read(f.local.path().join(".git/config")).unwrap(),
                config
            );
            assert_eq!(
                fs::read_to_string(f.local.path().join("tracked")).unwrap(),
                "precious unpublished work"
            );
            f.assert_bases();
        },
    );
}
