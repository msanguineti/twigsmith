use super::*;
use twigsmith::{
    config,
    settings::{self, Settings},
};

struct GitflowFixture {
    local: Repository,
    remote: tempfile::TempDir,
    base: String,
    develop: String,
}
impl GitflowFixture {
    fn new(root: &Path, active: bool, conflict: bool) -> Self {
        let local = Repository::new(root);
        let base = commit(&local, "shared", "released\n");
        local.success(&["tag", "v-base"]);
        local.success(&["switch", "-c", "develop"]);
        let develop = commit(&local, "development", "next version\n");
        if active {
            local.success(&["switch", "-c", "release/train"]);
            commit(
                &local,
                if conflict { "shared" } else { "release" },
                "active release\n",
            );
        }
        local.success(&["switch", "main"]);
        let remote = tempfile::tempdir_in(root).unwrap();
        local.success(&[
            "clone".as_ref(),
            "--bare".as_ref(),
            local.path().as_os_str(),
            remote.path().as_os_str(),
        ]);
        local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        let mut proposed = Settings {
            base: "develop".into(),
            ..Settings::default()
        };
        settings::gitflow_bundle(
            &mut proposed,
            "main".into(),
            active.then(|| "release/train".into()),
        );
        fs::write(
            local.path().join(".git/twigsmith.config.toml"),
            config::render(config::Format::Toml, &proposed),
        )
        .unwrap();
        Self {
            local,
            remote,
            base,
            develop,
        }
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
    fn assert_develop_unchanged(&self) {
        assert_eq!(oid(&self.local, "develop"), self.develop);
        assert_eq!(self.remote_oid("develop"), self.develop);
    }
    fn assert_work_removed(&self, branch: &str) {
        assert!(
            self.local
                .success(&["branch", "--list", branch])
                .stdout
                .is_empty()
        );
        assert!(
            self.local
                .success(&[
                    "--git-dir",
                    self.remote.path().to_str().unwrap(),
                    "branch",
                    "--list",
                    branch
                ])
                .stdout
                .is_empty()
        );
    }
}

#[test]
fn gitflow_generated_feature_release_and_no_active_hotfix_deliver_expected_refs() {
    isolated(
        "bundles::gitflow_generated_feature_release_and_no_active_hotfix_deliver_expected_refs",
        true,
        |root| {
            for kind in ["ordinary", "release", "hotfix"] {
                let f = GitflowFixture::new(root, false, false);
                let mut start = vec!["start", "work", "--policy", kind];
                if kind == "hotfix" {
                    start.extend(["--from-tag", "v-base"]);
                }
                success(&f.local, &start);
                assert_eq!(
                    oid(&f.local, "HEAD"),
                    if kind == "hotfix" {
                        f.base.as_str()
                    } else {
                        f.develop.as_str()
                    }
                );
                let source = commit(&f.local, "change", "finished work\n");
                success(&f.local, &["save"]);
                if kind == "ordinary" {
                    success(&f.local, &["finish"]);
                    let result = oid(&f.local, "develop");
                    assert_ne!(result, source);
                    assert_eq!(oid(&f.local, "develop^2"), source);
                    assert_eq!(f.remote_oid("develop"), result);
                    assert_eq!(f.remote_oid("main"), f.base);
                    assert_eq!(oid(&f.local, "HEAD"), result);
                    assert!(
                        f.local
                            .success(&["tag", "--list", "v-result"])
                            .stdout
                            .is_empty()
                    );
                } else {
                    success(&f.local, &["finish", "--tag", "v-result"]);
                    let production = oid(&f.local, "main");
                    let development = oid(&f.local, "develop");
                    assert_ne!(production, source);
                    assert_ne!(production, development);
                    assert_eq!(oid(&f.local, "main^2"), source);
                    assert_eq!(oid(&f.local, "develop^2"), source);
                    assert_eq!(oid(&f.local, "v-result^{}"), production);
                    assert_eq!(f.remote_oid("v-result^{}"), production);
                    assert_eq!(f.remote_oid("main"), production);
                    assert_eq!(f.remote_oid("develop"), development);
                    assert_eq!(oid(&f.local, "HEAD"), production);
                }
                f.assert_work_removed("work");
            }
        },
    );
}

#[test]
fn gitflow_active_release_later_conflict_retry_preserves_production_tag_and_develop() {
    isolated(
        "bundles::gitflow_active_release_later_conflict_retry_preserves_production_tag_and_develop",
        true,
        |root| {
            let f = GitflowFixture::new(root, true, true);
            success(
                &f.local,
                &["start", "fix", "--policy", "hotfix", "--from-tag", "v-base"],
            );
            assert_eq!(oid(&f.local, "HEAD"), f.base);
            let source = commit(&f.local, "shared", "fixed release\n");
            success(&f.local, &["save"]);
            let active_before = f.remote_oid("release/train");
            let stopped = cli(
                &f.local,
                &[
                    "finish",
                    "--tag",
                    "v-fixed",
                    "--tag-message",
                    "fixed version",
                ],
            );
            assert!(!stopped.status.success(), "{stopped:?}");
            assert!(
                f.local.path().join(".git/MERGE_HEAD").is_file(),
                "{stopped:?}"
            );
            let production = oid(&f.local, "main");
            let tag = oid(&f.local, "v-fixed");
            assert_ne!(production, source);
            assert_eq!(oid(&f.local, "main^2"), source);
            assert_eq!(oid(&f.local, "v-fixed^{}"), production);
            assert_eq!(f.remote_oid("main"), production);
            assert_eq!(f.remote_oid("release/train"), active_before);
            assert_eq!(f.remote_oid("fix"), source);
            f.assert_develop_unchanged();
            let production_reflog =
                fs::read(f.local.path().join(".git/logs/refs/heads/main")).unwrap();
            fs::write(f.local.path().join("shared"), "resolved release with fix\n").unwrap();
            f.local.success(&["add", "shared"]);
            f.local
                .success(&["-c", "core.editor=true", "merge", "--continue"]);
            let active = oid(&f.local, "HEAD");
            success(&f.local, &["finish"]);
            assert_eq!(oid(&f.local, "main"), production);
            assert_eq!(
                fs::read(f.local.path().join(".git/logs/refs/heads/main")).unwrap(),
                production_reflog
            );
            assert_eq!(oid(&f.local, "release/train^2"), source);
            assert_eq!(f.remote_oid("release/train"), active);
            assert_eq!(oid(&f.local, "v-fixed"), tag);
            assert_eq!(f.remote_oid("v-fixed"), tag);
            assert_eq!(f.remote_oid("v-fixed^{}"), production);
            assert_eq!(oid(&f.local, "HEAD"), production);
            f.assert_develop_unchanged();
            f.assert_work_removed("fix");
        },
    );
}

#[test]
fn gitflow_active_release_later_push_retry_preserves_production_tag_and_develop() {
    isolated(
        "bundles::gitflow_active_release_later_push_retry_preserves_production_tag_and_develop",
        true,
        |root| {
            let f = GitflowFixture::new(root, true, false);
            success(
                &f.local,
                &["start", "fix", "--policy", "hotfix", "--from-tag", "v-base"],
            );
            let source = commit(&f.local, "fix", "fixed release\n");
            success(&f.local, &["save"]);
            let hook = f.remote.path().join("hooks/pre-receive");
            fs::write(&hook, "#!/bin/sh\nwhile read old new ref; do\n if test \"$ref\" = refs/heads/release/train; then echo active-release-push-refused >&2; exit 1; fi\ndone\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let active_before = f.remote_oid("release/train");
            let stopped = cli(
                &f.local,
                &[
                    "finish",
                    "--tag",
                    "v-fixed",
                    "--tag-message",
                    "fixed version",
                ],
            );
            assert!(!stopped.status.success(), "{stopped:?}");
            assert!(
                String::from_utf8_lossy(&stopped.stderr).contains("active-release-push-refused"),
                "{stopped:?}"
            );
            let production = oid(&f.local, "main");
            let active = oid(&f.local, "release/train");
            let tag = oid(&f.local, "v-fixed");
            assert_ne!(production, source);
            assert_ne!(active, production);
            assert_eq!(oid(&f.local, "main^2"), source);
            assert_eq!(oid(&f.local, "release/train^2"), source);
            assert_eq!(oid(&f.local, "v-fixed^{}"), production);
            assert_eq!(f.remote_oid("main"), production);
            assert_eq!(f.remote_oid("release/train"), active_before);
            assert_eq!(f.remote_oid("fix"), source);
            assert_eq!(oid(&f.local, "HEAD"), active);
            f.assert_develop_unchanged();
            let production_reflog =
                fs::read(f.local.path().join(".git/logs/refs/heads/main")).unwrap();
            // Refuse any replay of the already confirmed first publication.
            fs::write(&hook, "#!/bin/sh\nwhile read old new ref; do\n if test \"$ref\" = refs/heads/main; then echo repeated-production-push >&2; exit 1; fi\ndone\n").unwrap();
            success(&f.local, &["finish"]);
            assert_eq!(oid(&f.local, "main"), production);
            assert_eq!(
                fs::read(f.local.path().join(".git/logs/refs/heads/main")).unwrap(),
                production_reflog
            );
            assert_eq!(f.remote_oid("release/train"), active);
            assert_eq!(oid(&f.local, "v-fixed"), tag);
            assert_eq!(f.remote_oid("v-fixed"), tag);
            assert_eq!(f.remote_oid("v-fixed^{}"), production);
            assert_eq!(oid(&f.local, "HEAD"), production);
            f.assert_develop_unchanged();
            f.assert_work_removed("fix");
        },
    );
}
