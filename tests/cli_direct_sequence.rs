//! Ordered tagged integration and one-time preparation through fresh CLI processes.
#[path = "cli_direct_sequence/completed_guards.rs"]
mod completed_guards;
mod support;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use support::{Repository, isolated};

fn cli(repo: &Repository, args: &[&str]) -> Output {
    Command::new(support::executable("tgi"))
        .current_dir(repo.path())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn success(repo: &Repository, args: &[&str]) -> String {
    let output = cli(repo, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn oid(repo: &Repository, reference: &str) -> String {
    String::from_utf8(repo.success(&["rev-parse", reference]).stdout)
        .unwrap()
        .trim()
        .into()
}
fn commit(repo: &Repository, path: &str, content: &str) -> String {
    fs::write(repo.path().join(path), content).unwrap();
    repo.success(&["add", path]);
    repo.success(&["commit", "-m", path]);
    oid(repo, "HEAD")
}
struct Fixture {
    local: Repository,
    remote: tempfile::TempDir,
}
impl Fixture {
    fn new(root: &Path, identity: &str, conflict: bool, cleanup: &str) -> Self {
        Self::with_policy(root, identity, conflict, cleanup, "release")
    }
    fn with_policy(
        root: &Path,
        identity: &str,
        conflict: bool,
        cleanup: &str,
        policy: &str,
    ) -> Self {
        let local = Repository::new(root);
        let base = commit(&local, "shared", "base\n");
        local.success(&["switch", "-c", "maintenance"]);
        commit(
            &local,
            if conflict { "shared" } else { "maintenance" },
            "maintenance\n",
        );
        local.success(&["switch", "main"]);
        commit(&local, "development", "new development\n");
        local.success(&["branch", "preparation"]);
        let creation = if policy == "hotfix" {
            local.success(&["tag", "v-base", &base]);
            "refs/tags/v-base"
        } else {
            "main"
        };
        let remote = tempfile::tempdir_in(root).unwrap();
        local.success(&[
            "clone".as_ref(),
            "--bare".as_ref(),
            local.path().as_os_str(),
            remote.path().as_os_str(),
        ]);
        local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        let active = if policy == "hotfix" {
            "active-release='none'\n"
        } else {
            ""
        };
        fs::write(local.path().join(".git/twigsmith.config.toml"), format!(
            "publish-remote='origin'\nbase-remote='origin'\nfinish-message='auto'\n[workstreams.{policy}]\n{active}creation-source='{creation}'\npreparation-source='preparation'\nintegration-targets=['main','maintenance']\nversion-tagging='required'\nversion-tag-identity='{identity}'\nrelease-marker='stable'\nfinish-mode='integrate'\nfinish-update='none'\nfinish-integration='merge'\nfinish-cleanup='{cleanup}'\nfinish-message='auto'\n"
        )).unwrap();
        if policy == "hotfix" {
            success(&local, &["start", "release-work", "--policy", policy]);
        } else {
            success(
                &local,
                &["start", "release-work", "--policy", policy, "--from", &base],
            );
        }
        Self { local, remote }
    }
    fn remote_oid(&self, reference: &str) -> String {
        String::from_utf8(
            self.local
                .success(&[
                    "--git-dir",
                    self.remote.path().to_str().unwrap(),
                    "rev-parse",
                    reference,
                ])
                .stdout,
        )
        .unwrap()
        .trim()
        .into()
    }
}
#[test]
fn later_conflict_continues_without_reintegrating_or_retagging_first_destination() {
    isolated(
        "later_conflict_continues_without_reintegrating_or_retagging_first_destination",
        true,
        |root| {
            for identity in ["work-source", "integrated-result"] {
                let f = Fixture::new(root, identity, true, "local");
                let source = commit(&f.local, "shared", "release\n");
                let stopped = cli(
                    &f.local,
                    &["finish", "--tag", "v-conflict", "--tag-message", "version"],
                );
                assert!(!stopped.status.success(), "{stopped:?}");
                let first = oid(&f.local, "main");
                let tag = oid(&f.local, "v-conflict");
                assert_eq!(f.remote_oid("main"), first);
                assert_eq!(oid(&f.local, "release-work"), source);
                assert!(
                    f.local.path().join(".git/MERGE_HEAD").exists(),
                    "{stopped:?}"
                );
                fs::write(f.local.path().join("shared"), "resolved later\n").unwrap();
                f.local.success(&["add", "shared"]);
                f.local
                    .success(&["-c", "core.editor=true", "merge", "--continue"]);
                let second = oid(&f.local, "HEAD");
                success(&f.local, &["finish"]);
                assert_eq!(oid(&f.local, "main"), first);
                assert_eq!(oid(&f.local, "maintenance"), second);
                assert_eq!(oid(&f.local, "maintenance^2"), source);
                assert_eq!(oid(&f.local, "v-conflict"), tag);
                assert_eq!(oid(&f.local, "HEAD"), first);
            }
        },
    );
}
#[test]
fn explicit_rebase_prepares_once_before_tagging_and_integrates_fixed_source() {
    isolated(
        "explicit_rebase_prepares_once_before_tagging_and_integrates_fixed_source",
        true,
        |root| {
            let f = Fixture::new(root, "work-source", true, "keep");
            let original = commit(&f.local, "shared", "release\n");
            let stopped = cli(
                &f.local,
                &["finish", "--tag", "v-rebased", "--update", "rebase"],
            );
            assert!(!stopped.status.success(), "{stopped:?}");
            let prepared = oid(&f.local, "release-work");
            assert_ne!(prepared, original);
            assert_eq!(oid(&f.local, "v-rebased"), prepared);
            assert_eq!(oid(&f.local, "main^2"), prepared);
            assert!(
                f.local.path().join(".git/MERGE_HEAD").exists(),
                "{stopped:?}"
            );
            fs::write(f.local.path().join("shared"), "resolved\n").unwrap();
            f.local.success(&["add", "shared"]);
            f.local
                .success(&["-c", "core.editor=true", "merge", "--continue"]);
            success(&f.local, &["finish"]);
            assert_eq!(oid(&f.local, "release-work"), prepared);
            assert_eq!(oid(&f.local, "maintenance^2"), prepared);
            assert_eq!(oid(&f.local, "v-rebased"), prepared);
            let log = String::from_utf8(
                f.local
                    .success(&["reflog", "show", "--format=%gs", "release-work"])
                    .stdout,
            )
            .unwrap();
            assert_eq!(
                log.lines().filter(|line| line.contains("(finish)")).count(),
                1,
                "{log}"
            );
        },
    );
}

#[test]
fn native_rebase_conflict_continuation_checks_actual_prepared_source_once() {
    isolated(
        "native_rebase_conflict_continuation_checks_actual_prepared_source_once",
        true,
        |root| {
            let f = Fixture::new(root, "work-source", false, "keep");
            f.local.success(&["switch", "preparation"]);
            commit(&f.local, "shared", "preparation\n");
            f.local.success(&["push", "origin", "preparation"]);
            f.local.success(&["switch", "release-work"]);
            let original = commit(&f.local, "shared", "release\n");
            let first_before = oid(&f.local, "main");
            let later_before = oid(&f.local, "maintenance");
            let stopped = cli(
                &f.local,
                &["finish", "--tag", "v-rebased", "--update", "rebase"],
            );
            assert!(!stopped.status.success(), "{stopped:?}");
            assert!(
                f.local.path().join(".git/rebase-merge").is_dir(),
                "{stopped:?}"
            );
            assert_eq!(oid(&f.local, "release-work"), original);
            assert_eq!(f.remote_oid("main"), first_before);
            assert_eq!(f.remote_oid("maintenance"), later_before);
            assert!(
                String::from_utf8(f.local.success(&["tag", "--list"]).stdout)
                    .unwrap()
                    .is_empty()
            );
            let still_stopped = cli(&f.local, &["finish"]);
            assert!(!still_stopped.status.success(), "{still_stopped:?}");
            fs::write(f.local.path().join("shared"), "prepared resolution\n").unwrap();
            f.local.success(&["add", "shared"]);
            f.local
                .success(&["-c", "core.editor=true", "rebase", "--continue"]);
            let prepared = oid(&f.local, "HEAD");
            let source_log =
                fs::read(f.local.path().join(".git/logs/refs/heads/release-work")).unwrap();
            success(&f.local, &["finish"]);
            assert_ne!(prepared, original);
            assert_eq!(oid(&f.local, "v-rebased"), prepared);
            assert_eq!(oid(&f.local, "main^2"), prepared);
            assert_eq!(oid(&f.local, "maintenance^2"), prepared);
            assert_eq!(
                fs::read(f.local.path().join(".git/logs/refs/heads/release-work")).unwrap(),
                source_log
            );
            assert_eq!(
                String::from_utf8(f.local.success(&["show", "v-rebased:shared"]).stdout).unwrap(),
                "prepared resolution\n"
            );
        },
    );
}

#[test]
fn hotfix_accepts_native_preparation_before_independent_destinations() {
    isolated(
        "hotfix_accepts_native_preparation_before_independent_destinations",
        true,
        |root| {
            let f = Fixture::with_policy(root, "work-source", false, "keep", "hotfix");
            let original = commit(&f.local, "fix", "hotfix\n");
            f.local.success(&["rebase", "preparation"]);
            let prepared = oid(&f.local, "HEAD");
            assert_ne!(prepared, original);
            let source_log =
                fs::read(f.local.path().join(".git/logs/refs/heads/release-work")).unwrap();
            success(&f.local, &["finish", "--tag", "v-hotfix"]);
            assert_eq!(oid(&f.local, "v-hotfix"), prepared);
            assert_eq!(oid(&f.local, "main^2"), prepared);
            assert_eq!(oid(&f.local, "maintenance^2"), prepared);
            assert_eq!(f.remote_oid("stable"), prepared);
            let subject = String::from_utf8(
                f.local
                    .success(&["show", "-s", "--format=%s", "maintenance"])
                    .stdout,
            )
            .unwrap();
            assert_eq!(subject.trim(), "Integrate release-work into maintenance");
            assert_eq!(
                fs::read(f.local.path().join(".git/logs/refs/heads/release-work")).unwrap(),
                source_log
            );
        },
    );
}

#[test]
fn failed_work_first_result_and_later_checks_preserve_selected_version() {
    isolated(
        "failed_work_first_result_and_later_checks_preserve_selected_version",
        true,
        |root| {
            let f = Fixture::new(root, "integrated-result", false, "local");
            let config = f.local.path().join(".git/twigsmith.config.toml");
            let settings = fs::read_to_string(&config).unwrap();
            fs::write(&config, format!("{settings}\n[[finish-checks]]\nname='Result gate'\ncommand=['sh','.git/check']\n")).unwrap();
            fs::write(f.local.path().join(".git/check"), "branch=$(git symbolic-ref --short HEAD)\necho \"$branch $(git rev-parse HEAD)\" >> .git/check-log\ntest -f \".git/allow-$branch\"\n").unwrap();
            let source = commit(&f.local, "version", "release\n");
            let first_before = oid(&f.local, "main");
            let later_before = oid(&f.local, "maintenance");
            let source_failed = cli(
                &f.local,
                &[
                    "finish",
                    "--tag",
                    "v-checks",
                    "--tag-message",
                    "checked version",
                ],
            );
            assert!(!source_failed.status.success(), "{source_failed:?}");
            assert_eq!(oid(&f.local, "main"), first_before);
            assert_eq!(oid(&f.local, "maintenance"), later_before);
            assert!(f.local.success(&["tag", "--list"]).stdout.is_empty());
            fs::write(f.local.path().join(".git/allow-release-work"), "").unwrap();
            let first_failed = cli(&f.local, &["finish"]);
            assert!(!first_failed.status.success(), "{first_failed:?}");
            let first = oid(&f.local, "main");
            assert_ne!(first, first_before);
            assert_eq!(f.remote_oid("main"), first_before);
            assert!(f.local.success(&["tag", "--list"]).stdout.is_empty());
            fs::write(f.local.path().join(".git/allow-main"), "").unwrap();
            let later_failed = cli(&f.local, &["finish"]);
            assert!(!later_failed.status.success(), "{later_failed:?}");
            let second = oid(&f.local, "maintenance");
            let tag = oid(&f.local, "v-checks");
            assert_eq!(f.remote_oid("main"), first);
            assert_eq!(f.remote_oid("maintenance"), later_before);
            assert_eq!(oid(&f.local, "v-checks^{commit}"), first);
            assert_eq!(oid(&f.local, "release-work"), source);
            fs::write(f.local.path().join(".git/allow-maintenance"), "").unwrap();
            success(&f.local, &["finish"]);
            assert_eq!(oid(&f.local, "main"), first);
            assert_eq!(oid(&f.local, "maintenance"), second);
            assert_eq!(oid(&f.local, "v-checks"), tag);
            assert_eq!(f.remote_oid("maintenance"), second);
            assert_eq!(f.remote_oid("v-checks"), tag);
            let checks = fs::read_to_string(f.local.path().join(".git/check-log")).unwrap();
            assert_eq!(
                checks
                    .lines()
                    .filter(|line| line.starts_with("release-work "))
                    .count(),
                2,
                "{checks}"
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn later_push_rejection_retries_exact_result_before_any_cleanup() {
    isolated(
        "later_push_rejection_retries_exact_result_before_any_cleanup",
        true,
        |root| {
            use std::os::unix::fs::PermissionsExt;
            let f = Fixture::new(root, "work-source", false, "both");
            let hook = f.remote.path().join("hooks/pre-receive");
            fs::create_dir_all(hook.parent().unwrap()).unwrap();
            fs::write(&hook, "#!/bin/sh\nwhile read old new ref; do\n if test \"$ref\" = refs/heads/maintenance && ! test -f allow-maintenance; then exit 1; fi\ndone\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let source = commit(&f.local, "version", "release\n");
            success(&f.local, &["save"]);
            let later_before = f.remote_oid("maintenance");
            let failed = cli(
                &f.local,
                &[
                    "finish",
                    "--tag",
                    "v-push",
                    "--tag-message",
                    "released version",
                ],
            );
            assert!(!failed.status.success(), "{failed:?}");
            let first = oid(&f.local, "main");
            let second = oid(&f.local, "maintenance");
            let tag = oid(&f.local, "v-push");
            assert_eq!(f.remote_oid("main"), first);
            assert_eq!(f.remote_oid("maintenance"), later_before);
            assert_eq!(oid(&f.local, "release-work"), source);
            assert_eq!(f.remote_oid("release-work"), source);
            assert!(
                f.local
                    .success(&[
                        "--git-dir",
                        f.remote.path().to_str().unwrap(),
                        "tag",
                        "--list"
                    ])
                    .stdout
                    .is_empty()
            );
            fs::write(f.remote.path().join("allow-maintenance"), "").unwrap();
            success(&f.local, &["finish"]);
            assert_eq!(oid(&f.local, "main"), first);
            assert_eq!(oid(&f.local, "maintenance"), second);
            assert_eq!(f.remote_oid("maintenance"), second);
            assert_eq!(oid(&f.local, "v-push"), tag);
            assert_eq!(f.remote_oid("v-push"), tag);
            assert!(
                f.local
                    .success(&["branch", "--list", "release-work"])
                    .stdout
                    .is_empty()
            );
            assert!(
                f.local
                    .success(&[
                        "--git-dir",
                        f.remote.path().to_str().unwrap(),
                        "branch",
                        "--list",
                        "release-work"
                    ])
                    .stdout
                    .is_empty()
            );
        },
    );
}

#[test]
fn authorized_published_rebase_cleans_only_originally_observed_work() {
    isolated(
        "authorized_published_rebase_cleans_only_originally_observed_work",
        true,
        |root| {
            let f = Fixture::new(root, "work-source", true, "both");
            let original = commit(&f.local, "shared", "release\n");
            success(&f.local, &["save"]);
            let refused = cli(
                &f.local,
                &[
                    "finish",
                    "--tag",
                    "v-shared",
                    "--update",
                    "rebase",
                    "--no-rewrite",
                ],
            );
            assert!(!refused.status.success(), "{refused:?}");
            assert_eq!(oid(&f.local, "release-work"), original);
            assert!(f.local.success(&["tag", "--list"]).stdout.is_empty());
            let stopped = cli(
                &f.local,
                &[
                    "finish",
                    "--tag",
                    "v-shared",
                    "--update",
                    "rebase",
                    "--allow-rewrite",
                ],
            );
            assert!(!stopped.status.success(), "{stopped:?}");
            assert!(
                f.local.path().join(".git/MERGE_HEAD").exists(),
                "{stopped:?}"
            );
            let prepared = oid(&f.local, "release-work");
            assert_ne!(original, prepared);
            assert_eq!(oid(&f.local, "v-shared"), prepared);
            fs::write(f.local.path().join("shared"), "resolved\n").unwrap();
            f.local.success(&["add", "shared"]);
            f.local
                .success(&["-c", "core.editor=true", "merge", "--continue"]);
            // A newly published version cannot inherit the original deletion lease.
            f.local.success(&[
                "--git-dir",
                f.remote.path().to_str().unwrap(),
                "update-ref",
                "refs/heads/release-work",
                &prepared,
                &original,
            ]);
            let blocked = cli(&f.local, &["finish"]);
            assert!(!blocked.status.success(), "{blocked:?}");
            assert_eq!(oid(&f.local, "release-work"), prepared);
            assert_eq!(f.remote_oid("release-work"), prepared);
            f.local.success(&[
                "--git-dir",
                f.remote.path().to_str().unwrap(),
                "update-ref",
                "refs/heads/release-work",
                &original,
                &prepared,
            ]);
            success(&f.local, &["finish"]);
            assert_eq!(oid(&f.local, "v-shared"), prepared);
            assert_eq!(oid(&f.local, "maintenance^2"), prepared);
            assert!(
                f.local
                    .success(&["branch", "--list", "release-work"])
                    .stdout
                    .is_empty()
            );
            assert!(
                f.local
                    .success(&[
                        "--git-dir",
                        f.remote.path().to_str().unwrap(),
                        "branch",
                        "--list",
                        "release-work"
                    ])
                    .stdout
                    .is_empty()
            );
            let pins = f.local.success(&[
                "for-each-ref",
                "--format=%(refname)",
                "--points-at",
                &original,
                "refs/twigsmith/integration",
            ]);
            let pins = String::from_utf8(pins.stdout).unwrap();
            let original_pin = pins
                .lines()
                .find(|name| name.ends_with("/original"))
                .unwrap();
            // Missing original recovery authority must also invalidate a completed retry.
            f.local
                .success(&["update-ref", "-d", original_pin, &original]);
            let blocked = cli(&f.local, &["finish"]);
            assert!(!blocked.status.success(), "{blocked:?}");
            assert_eq!(oid(&f.local, "v-shared"), prepared);
        },
    );
}

#[cfg(unix)]
#[test]
fn identical_first_result_reuses_successful_work_checks_but_not_bypass() {
    isolated(
        "identical_first_result_reuses_successful_work_checks_but_not_bypass",
        true,
        |root| {
            use std::os::unix::fs::PermissionsExt;
            for bypass in [false, true] {
                let local = Repository::new(root);
                let base = commit(&local, "base", "base\n");
                let remote = tempfile::tempdir_in(root).unwrap();
                local.success(&[
                    "clone".as_ref(),
                    "--bare".as_ref(),
                    local.path().as_os_str(),
                    remote.path().as_os_str(),
                ]);
                local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
                fs::write(local.path().join(".git/twigsmith.config.toml"),
                    "publish-remote='origin'\nfinish-checks=[{name='Exact commit gate',command=['sh','.git/check']}]\n[workstreams.release]\ncreation-source='main'\nintegration-target='main'\nversion-tagging='required'\nversion-tag-identity='integrated-result'\nfinish-mode='integrate'\nfinish-update='none'\nfinish-integration='ff-only'\nfinish-cleanup='keep'\n"
                ).unwrap();
                fs::write(
                    local.path().join(".git/check"),
                    "git rev-parse HEAD >> .git/check-log\ntest -f .git/check-pass\n",
                )
                .unwrap();
                if !bypass {
                    fs::write(local.path().join(".git/check-pass"), "").unwrap();
                }
                success(&local, &["start", "release-work", "--policy", "release"]);
                let source = commit(&local, "release", "version\n");
                let hook = remote.path().join("hooks/pre-receive");
                fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
                fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
                let mut args = vec!["finish", "--tag", "v-identical"];
                if bypass {
                    args.push("--skip-checks");
                }
                let stopped = cli(&local, &args);
                assert!(!stopped.status.success(), "{stopped:?}");
                assert_eq!(oid(&local, "main"), source);
                assert_eq!(oid(&local, "v-identical"), source);
                let remote_main = || {
                    String::from_utf8(
                        local
                            .success(&[
                                "--git-dir",
                                remote.path().to_str().unwrap(),
                                "rev-parse",
                                "main",
                            ])
                            .stdout,
                    )
                    .unwrap()
                    .trim()
                    .to_owned()
                };
                assert_eq!(remote_main(), base);
                let log = local.path().join(".git/check-log");
                if bypass {
                    assert!(!log.exists());
                } else {
                    assert_eq!(fs::read_to_string(&log).unwrap(), format!("{source}\n"));
                }
                fs::remove_file(hook).unwrap();
                if bypass {
                    let failed = cli(&local, &["finish"]);
                    assert!(!failed.status.success(), "{failed:?}");
                    assert_eq!(remote_main(), base);
                    assert_eq!(fs::read_to_string(&log).unwrap(), format!("{source}\n"));
                    fs::write(local.path().join(".git/check-pass"), "").unwrap();
                }
                success(&local, &["finish"]);
                assert_eq!(remote_main(), source);
                assert_eq!(oid(&local, "v-identical"), source);
                let expected = if bypass { 2 } else { 1 };
                assert_eq!(
                    fs::read_to_string(&log).unwrap(),
                    format!("{source}\n").repeat(expected)
                );
            }
        },
    );
}
