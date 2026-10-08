#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use git_oneflow::{
    git,
    repository::RepositoryKind,
    start::{self, PreparedRequest, ResolveError, Settings},
};
use preservation::snapshot;
use std::{fs, path::Path};
use support::{Repository, isolated};

fn preserved(
    root: &Path,
    directory: &Path,
    description: &str,
    name: Option<&str>,
) -> Result<PreparedRequest, ResolveError> {
    let before = snapshot(root);
    let result = start::prepare(directory, description, name);
    assert_eq!(snapshot(root), before, "preparation modified fixture state");
    result
}

#[test]
fn defaults_nested_and_literal_targets() {
    isolated("defaults_nested_and_literal_targets", true, |root| {
        let repo = Repository::new(root);
        let nested = repo.path().join("nested directory");
        fs::create_dir(&nested).unwrap();
        // Preparation does not depend on reading a healthy index or born HEAD.
        fs::write(repo.path().join(".git/index"), "invalid index").unwrap();
        let request = preserved(root, repo.path(), "Fix bug", None).unwrap();
        assert_eq!(request.target, "feature/fix-bug");
        assert_eq!(request.settings, Settings::default());
        assert_eq!(request.repository.kind, RepositoryKind::Working);
        assert_eq!(request, preserved(root, &nested, "Fix bug", None).unwrap());
        for name in [
            "plain",
            "release/V1",
            "hotfix/Urgent",
            "docs/École",
            "fix/E\u{301}",
            "@",
            "a;echo-pwned",
            "feature/-option",
        ] {
            assert_eq!(
                preserved(root, &nested, "🚀", Some(name)).unwrap().target,
                name
            );
        }
        for name in [
            "",
            "-option",
            "HEAD",
            "@{-1}",
            "@{upstream}",
            "main~1",
            "main^",
            "main:other",
            "a..b",
            "a b",
            "a\n",
            "a\0b",
            "/a",
            "a/",
            "a//b",
            ".hidden",
            "a/.hidden",
            "a.lock",
            "a\\b",
            "a?b",
            "a*b",
            "a[b",
            "a.",
        ] {
            assert!(
                matches!(
                    preserved(root, &nested, "okay", Some(name)),
                    Err(ResolveError::InvalidTarget(_))
                ),
                "{name:?}"
            );
        }
        assert!(matches!(
            preserved(root, &nested, "main", Some("main")),
            Err(ResolveError::ProtectedBase(_))
        ));
        assert!(matches!(
            preserved(root, &nested, "🚀", None),
            Err(ResolveError::EmptySuffix)
        ));
    });
}

#[test]
fn settings_overrides_and_errors() {
    isolated("settings_overrides_and_errors", true, |root| {
        let repo = Repository::new(root);
        let path = repo.path().join(".git/oneflow.conf");
        fs::write(
            &path,
            "# repository settings\nremote = team/upstream\nbase = Feature/École\n",
        )
        .unwrap();
        let request = preserved(root, repo.path(), "okay", None).unwrap();
        assert_eq!(
            request.settings,
            Settings {
                remote: "team/upstream".into(),
                base: "Feature/École".into()
            }
        );
        assert!(matches!(
            preserved(root, repo.path(), "okay", Some("Feature/École")),
            Err(ResolveError::ProtectedBase(_))
        ));
        // Generated names receive the same protected-base check.
        fs::write(&path, "base=feature/okay").unwrap();
        assert!(matches!(
            preserved(root, repo.path(), "okay", None),
            Err(ResolveError::ProtectedBase(_))
        ));
        for contents in [
            "remote=",
            "base=",
            "remote=x\nremote=y",
            "base=x\nbase=y",
            "typo=x",
            "not settings",
            "remote=-option",
            "remote=.",
            "remote=https://example.invalid/repo",
            "remote=../repo",
            "remote=bad remote",
            "remote=bad\0name",
            "base=HEAD",
            "base=-option",
            "base=@{-1}",
            "base=bad..name",
            "base=\"main\" # comment",
        ] {
            fs::write(&path, contents).unwrap();
            assert!(
                matches!(
                    preserved(root, repo.path(), "okay", None),
                    Err(ResolveError::InvalidSettings(_))
                ),
                "{contents:?}"
            );
        }
        fs::write(&path, b"remote=\xff").unwrap();
        assert!(matches!(
            preserved(root, repo.path(), "okay", None),
            Err(ResolveError::SettingsIo { .. })
        ));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            preserved(root, repo.path(), "okay", None),
            Err(ResolveError::SettingsIo { .. })
        ));
        fs::remove_dir(&path).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("absent", &path).unwrap();
            assert!(matches!(
                preserved(root, repo.path(), "okay", None),
                Err(ResolveError::SettingsIo { .. })
            ));
        }
    });
}

#[test]
fn common_settings_in_linked_and_bare_contexts() {
    isolated(
        "common_settings_in_linked_and_bare_contexts",
        true,
        |root| {
            let repo = Repository::new(root);
            fs::write(repo.path().join("tracked"), "base").unwrap();
            repo.success(&["add", "tracked"]);
            repo.success(&["commit", "-m", "base"]);
            let linked = root.join("linked tree");
            repo.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "-b".as_ref(),
                "linked".as_ref(),
                linked.as_os_str(),
            ]);
            fs::create_dir(linked.join("nested")).unwrap();
            fs::write(
                repo.path().join(".git/oneflow.conf"),
                "remote=upstream\nbase=trunk",
            )
            .unwrap();
            repo.success(&["branch", "feature/fix"]);
            repo.success(&["checkout", "--detach"]);
            let local = preserved(root, repo.path(), "Fix", None).unwrap();
            assert_eq!(local.target, "feature/fix"); // Existing names never gain a suffix.
            let other = preserved(root, &linked.join("nested"), "Fix", None).unwrap();
            assert_eq!(other.repository.kind, RepositoryKind::LinkedWorktree);
            assert_eq!(local.repository.common_dir, other.repository.common_dir);
            assert_eq!(local.settings, other.settings);
            // A worktree-private lookalike is ignored by the common-dir contract.
            fs::write(other.repository.git_dir.join("oneflow.conf"), "base=wrong").unwrap();
            assert_eq!(
                preserved(root, &linked, "Fix", None).unwrap().settings,
                local.settings
            );
            let bare = root.join("bare.git");
            repo.success(&[
                "clone".as_ref(),
                "--bare".as_ref(),
                repo.path().as_os_str(),
                bare.as_os_str(),
            ]);
            fs::write(bare.join("oneflow.conf"), "base=trunk").unwrap();
            let request = preserved(root, &bare, "Fix", None).unwrap();
            assert_eq!(request.repository.kind, RepositoryKind::Bare);
            assert_eq!(request.settings.base, "trunk");
            // Bare eligibility belongs to the workflow task, not input resolution.
            assert!(matches!(
                preserved(root, &repo.path().join(".git"), "Fix", None),
                Err(ResolveError::Repository(_))
            ));
            assert!(matches!(
                preserved(root, root, "Fix", None),
                Err(ResolveError::OutsideRepository)
            ));
        },
    );
}

#[test]
fn preparation_never_contacts_remote_or_runs_fsmonitor() {
    isolated(
        "preparation_never_contacts_remote_or_runs_fsmonitor",
        true,
        |root| {
            let repo = Repository::new(root);
            fs::write(repo.path().join("tracked"), "base").unwrap();
            repo.success(&["add", "tracked"]);
            repo.success(&["commit", "-m", "base"]);
            let script = repo.path().join(".git/probe");
            fs::write(
                &script,
                "#!/bin/sh\nprintf called > fsmonitor-was-called\nexit 1\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            }
            repo.success(&["config", "core.fsmonitor", ".git/probe"]);
            repo.success(&["config", "protocol.ext.allow", "always"]);
            repo.success(&["remote", "add", "origin", "ext::touch remote-was-contacted"]);
            fs::write(repo.path().join(".git/index.lock"), "existing lock").unwrap();
            preserved(root, repo.path(), "ordinary", None).unwrap();
            assert!(!repo.path().join("fsmonitor-was-called").exists());
            assert!(!repo.path().join("remote-was-contacted").exists());
            // Positive controls prove both probes work in the isolated fixture.
            fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
            repo.success(&["status", "--porcelain=v2"]);
            assert!(repo.path().join("fsmonitor-was-called").exists());
            let output = git::run(repo.path(), ["ls-remote", "origin"]).unwrap();
            assert!(!output.status.success());
            assert!(repo.path().join("remote-was-contacted").exists());
        },
    );
}

#[test]
fn missing_git_is_explicit() {
    isolated("missing_git_is_explicit", false, |root| {
        assert!(matches!(
            preserved(root, root, "Fix", None),
            Err(ResolveError::Repository(_))
        ));
        assert!(git::run(root, ["--version"]).is_err());
    });
}
