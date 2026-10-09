#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use git_oneflow::{
    git,
    repository::RepositoryKind,
    settings::{Settings, SettingsError},
    start::{self, PreparedRequest, ResolveError},
};
use preservation::snapshot;
use std::{fs, path::Path};
use support::{Repository, isolated};

fn preserved(root: &Path, directory: &Path, input: &str) -> Result<PreparedRequest, ResolveError> {
    let before = snapshot(root);
    let result = start::prepare(directory, input);
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
        let request = preserved(root, repo.path(), "Fix bug").unwrap();
        assert_eq!(request.target, "fix-bug");
        assert_eq!(request.settings, Settings::default());
        assert_eq!(request.repository.kind, RepositoryKind::Working);
        assert_eq!(request, preserved(root, &nested, "Fix bug").unwrap());
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
            assert_eq!(preserved(root, &nested, name).unwrap().target, name);
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
                    preserved(root, &nested, name),
                    Err(ResolveError::InvalidTarget(_))
                ),
                "{name:?}"
            );
        }
        assert!(matches!(
            preserved(root, &nested, "main"),
            Err(ResolveError::ProtectedBase(_))
        ));
        assert!(matches!(
            preserved(root, &nested, " 🚀 "),
            Err(ResolveError::UnusablePhrase)
        ));
    });
}

#[test]
fn phrase_conversion_and_literal_unicode_are_distinct_contracts() {
    isolated(
        "phrase_conversion_and_literal_unicode_are_distinct_contracts",
        true,
        |root| {
            let repo = Repository::new(root);
            for (input, expected) in [
                ("nasty deployment bug", "nasty-deployment-bug"),
                (" Fix__ BUG / now... ", "fix-bug-now"),
                ("CAFÉ 123", "café-123"),
                ("Cafe\u{301} 123", "café-123"),
                ("ΑΒΓ МОСКВА 中文１２", "αβγ-москва-中文１２"),
                ("İSTANBUL work", "i\u{307}stanbul-work"),
                ("हिन्दी عَرَبِيّ", "हिन्दी-عَرَبِيّ"),
                ("\u{301}ABC /\u{301}DEF", "abc-def"),
                ("a\t/_.🚀\u{a0} b", "a-b"),
            ] {
                assert_eq!(
                    preserved(root, repo.path(), input).unwrap().target,
                    expected
                );
            }
            for (a, b) in [
                ("Å", "A\u{30a}"),
                ("각", "각"),
                ("É\u{323}", "E\u{323}\u{301}"),
            ] {
                // Git accepts both exact spellings; preparation must not merge them.
                assert_ne!(a, b);
                assert_eq!(preserved(root, repo.path(), a).unwrap().target, a);
                assert_eq!(preserved(root, repo.path(), b).unwrap().target, b);
                assert_eq!(
                    preserved(root, repo.path(), &format!("{a} work"))
                        .unwrap()
                        .target,
                    preserved(root, repo.path(), &format!("{b} work"))
                        .unwrap()
                        .target,
                );
            }
            // No arbitrary product length limit or summarization.
            let input = "Long_word ".repeat(200);
            let expected = vec!["long-word"; 200].join("-");
            assert_eq!(
                preserved(root, repo.path(), &input).unwrap().target,
                expected
            );
            let literal = "É_".repeat(300);
            assert_eq!(
                preserved(root, repo.path(), &literal).unwrap().target,
                literal
            );
            for input in [" ", " \t", " ---...🚀 ", " \u{301}\u{20dd}"] {
                assert!(matches!(
                    preserved(root, repo.path(), input),
                    Err(ResolveError::UnusablePhrase)
                ));
            }
            // Tabs alone do not trigger repair; Unicode spaces valid in Git stay literal.
            for input in ["a\tb", "\t", "a\nb"] {
                assert!(matches!(
                    preserved(root, repo.path(), input),
                    Err(ResolveError::InvalidTarget(_))
                ));
            }
            for input in [
                "CAFÉ",
                "Cafe\u{301}",
                "a\u{a0}b",
                "🚀",
                "Fix/DeploymentBug",
                "fix/parser_v2",
                "feature",
            ] {
                assert_eq!(preserved(root, repo.path(), input).unwrap().target, input);
            }
        },
    );
}

#[test]
fn settings_overrides_and_errors() {
    isolated("settings_overrides_and_errors", true, |root| {
        let repo = Repository::new(root);
        let path = repo.path().join(".git/git-oneflow.config.toml");
        fs::write(
            &path,
            "# repository settings\npublish-remote = 'team/upstream'\nbase = 'Feature/École'\n",
        )
        .unwrap();
        let request = preserved(root, repo.path(), "okay").unwrap();
        assert_eq!(
            request.settings,
            Settings {
                publish_remote: "team/upstream".into(),
                base_remote: "team/upstream".into(),
                base: "Feature/École".into(),
                sources: git_oneflow::settings::SettingSources {
                    publish_remote: git_oneflow::settings::SettingSource::Local(path.clone()),
                    base: git_oneflow::settings::SettingSource::Local(path.clone()),
                    ..Default::default()
                },
                ..Settings::default()
            }
        );
        assert!(matches!(
            preserved(root, repo.path(), "Feature/École"),
            Err(ResolveError::ProtectedBase(_))
        ));
        // Phrase names receive the same protected-base check.
        fs::write(&path, "base='okay-work'").unwrap();
        assert!(matches!(
            preserved(root, repo.path(), "okay work"),
            Err(ResolveError::ProtectedBase(_))
        ));
        for contents in [
            "publish-remote=",
            "base=",
            "publish-remote='x'\npublish-remote='y'",
            "base='x'\nbase='y'",
            "typo=x",
            "not settings",
            "publish-remote='-option'",
            "publish-remote='.'",
            "publish-remote='https://example.invalid/repo'",
            "publish-remote='../repo'",
            "publish-remote='bad remote'",
            "publish-remote='bad\0name'",
            "base='HEAD'",
            "base='-option'",
            "base='@{-1}'",
            "base='bad..name'",
        ] {
            fs::write(&path, contents).unwrap();
            assert!(
                matches!(
                    preserved(root, repo.path(), "okay"),
                    Err(ResolveError::Settings(SettingsError::InvalidSettings(_)))
                ),
                "{contents:?}"
            );
        }
        fs::write(&path, b"publish-remote=\xff").unwrap();
        assert!(matches!(
            preserved(root, repo.path(), "okay"),
            Err(ResolveError::Settings(SettingsError::SettingsIo { .. }))
        ));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            preserved(root, repo.path(), "okay"),
            Err(ResolveError::Settings(SettingsError::SettingsIo { .. }))
        ));
        fs::remove_dir(&path).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("absent", &path).unwrap();
            assert!(matches!(
                preserved(root, repo.path(), "okay"),
                Err(ResolveError::Settings(SettingsError::SettingsIo { .. }))
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
                repo.path().join(".git/git-oneflow.config.toml"),
                "publish-remote='upstream'\nbase='trunk'",
            )
            .unwrap();
            repo.success(&["branch", "Fix"]);
            repo.success(&["checkout", "--detach"]);
            let local = preserved(root, repo.path(), "Fix").unwrap();
            assert_eq!(local.target, "Fix"); // Existing names never gain a suffix.
            let other = preserved(root, &linked.join("nested"), "Fix").unwrap();
            assert_eq!(other.repository.kind, RepositoryKind::LinkedWorktree);
            assert_eq!(local.repository.common_dir, other.repository.common_dir);
            assert_eq!(local.settings, other.settings);
            // A worktree-private lookalike is ignored by the common-dir contract.
            fs::write(
                other.repository.git_dir.join("git-oneflow.config.toml"),
                "base='wrong'",
            )
            .unwrap();
            assert_eq!(
                preserved(root, &linked, "Fix").unwrap().settings,
                local.settings
            );
            let bare = root.join("bare.git");
            repo.success(&[
                "clone".as_ref(),
                "--bare".as_ref(),
                repo.path().as_os_str(),
                bare.as_os_str(),
            ]);
            fs::write(bare.join("git-oneflow.config.toml"), "base='trunk'").unwrap();
            let request = preserved(root, &bare, "Fix").unwrap();
            assert_eq!(request.repository.kind, RepositoryKind::Bare);
            assert_eq!(request.settings.base, "trunk");
            // Bare eligibility belongs to the workflow task, not input resolution.
            assert!(matches!(
                preserved(root, &repo.path().join(".git"), "Fix"),
                Err(ResolveError::Repository(_))
            ));
            assert!(matches!(
                preserved(root, root, "Fix"),
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
            preserved(root, repo.path(), "ordinary").unwrap();
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
            preserved(root, root, "Fix"),
            Err(ResolveError::Repository(_))
        ));
        assert!(git::run(root, ["--version"]).is_err());
    });
}
