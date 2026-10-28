#[path = "support/snapshot.rs"]
mod snapshot;
mod support;

use std::{ffi::OsString, fs, path::Path, process::Command};

use support::{Repository, isolated};
use twigsmith::completion;

fn complete(directory: &Path, words: &[&str]) -> Vec<String> {
    let words: Vec<_> = words.iter().map(OsString::from).collect();
    completion::candidates(&words, words.len() - 1, directory)
}

#[test]
fn grammar_and_values_work_without_repository_or_git() {
    isolated(
        "grammar_and_values_work_without_repository_or_git",
        false,
        |root| {
            let words = complete(root, &["tgi", ""]);
            for word in [
                "start",
                "s",
                "save",
                "v",
                "finish",
                "f",
                "status",
                "config",
                "completion",
            ] {
                assert!(words.contains(&word.into()), "missing {word}: {words:?}");
            }
            for (command, options) in [
                (
                    "s",
                    vec!["--policy", "--from", "--from-tag", "--target", "--help"],
                ),
                (
                    "v",
                    vec![
                        "--publish-only",
                        "--commit",
                        "--stash",
                        "--message",
                        "--stash-message",
                        "--help",
                    ],
                ),
                (
                    "f",
                    vec![
                        "--mode",
                        "--integration",
                        "--checkout",
                        "--cleanup",
                        "--message",
                        "--merge-back",
                        "--tag",
                        "--tag-message",
                        "--sign-tag",
                        "--update",
                        "--history-edit",
                        "--allow-rewrite",
                        "--no-rewrite",
                        "--skip-checks",
                        "--show-credentials",
                        "--help",
                    ],
                ),
                (
                    "config",
                    vec![
                        "--yes",
                        "--oneflow",
                        "--oneflow-hosted",
                        "--marker",
                        "--format",
                        "--help",
                    ],
                ),
                ("status", vec!["--help"]),
            ] {
                assert_eq!(
                    complete(root, &["twigsmith", command, "--"]),
                    options
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                );
            }
            for (command, option, values) in [
                ("start", "--policy", vec!["hotfix", "ordinary", "release"]),
                ("save", "--commit", vec!["all", "staged", "unstaged"]),
                ("save", "--stash", vec!["all", "staged", "unstaged"]),
                ("finish", "--mode", vec!["integrate", "review"]),
                (
                    "finish",
                    "--integration",
                    vec!["ff-only", "merge", "squash"],
                ),
                ("finish", "--checkout", vec!["target", "work"]),
                ("finish", "--cleanup", vec!["both", "keep", "local"]),
                ("finish", "--update", vec!["merge", "none", "rebase"]),
                ("finish", "--history-edit", vec!["interactive", "none"]),
                ("config", "--format", vec!["json", "toml", "yaml"]),
            ] {
                assert_eq!(complete(root, &["tgi", command, option, ""]), values);
            }
            let repeated = complete(root, &["tgi", "save", "--commit", "staged", "--"]);
            assert!(repeated.contains(&"--commit".into()));
            assert!(repeated.contains(&"--stash".into()));
            assert!(!repeated.contains(&"--publish-only".into()));
            assert!(complete(root, &["tgi", "save", "--stash-message", ""]).is_empty());
            assert!(complete(root, &["tgi", "start", "new"]).is_empty());
            assert!(complete(root, &["tgi", "finish", "--mode=int"]).is_empty());
            assert!(complete(root, &["tgi", "save", "--message", ""]).is_empty());
            assert_eq!(
                complete(root, &["tgi", "completion", "install", "p"]),
                ["powershell"]
            );
            assert!(
                !complete(root, &["tgi", "save", "--publish-only", "--"])
                    .iter()
                    .any(|value| ["--commit", "--stash", "--publish-only", "--help"]
                        .contains(&value.as_str()))
            );
            assert!(
                !complete(root, &["tgi", "finish", "--allow-rewrite", "--"])
                    .contains(&"--no-rewrite".into())
            );
            assert!(
                !complete(root, &["tgi", "config", "--oneflow", "--"])
                    .contains(&"--oneflow-hosted".into())
            );
            assert!(
                !complete(root, &["tgi", "start", "new", "--from", "HEAD", "--"])
                    .contains(&"--from-tag".into())
            );
            assert!(complete(root, &["tgi", "config", "show", ""]).is_empty());
            assert!(complete(root, &["tgi", "finish", "--help", ""]).is_empty());
        },
    );
}

#[test]
fn branches_follow_roles_mapping_exclusions_and_live_refs() {
    isolated(
        "branches_follow_roles_mapping_exclusions_and_live_refs",
        true,
        |root| {
            let repo = Repository::new(root);
            repo.success(&["commit", "--allow-empty", "-m", "initial"]);
            for name in [
                "feat/avatars",
                "feat/login",
                "literal/$(echo-pwned);'quote'",
                "中文/修复",
                "topic=literal",
            ] {
                repo.success(&["branch", name]);
            }
            repo.success(&["config", "remote.origin.url", "fake://not-contacted"]);
            repo.success(&[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/cache/origin/*",
            ]);
            repo.success(&[
                "config",
                "--add",
                "remote.origin.fetch",
                "^refs/heads/private/*",
            ]);
            repo.success(&[
                "config",
                "remote.upstream.fetch",
                "+refs/heads/*:refs/cache/upstream/*",
            ]);
            for reference in [
                "refs/cache/origin/feat/remote",
                "refs/cache/origin/private/secret",
                "refs/cache/upstream/target",
                "refs/cache/upstream/unrelated",
            ] {
                repo.success(&["update-ref", reference, "HEAD"]);
            }
            repo.success(&[
                "symbolic-ref",
                "refs/cache/origin/alias",
                "refs/cache/origin/feat/remote",
            ]);
            fs::write(
                repo.path().join("twigsmith.config.toml"),
                "base-remote = 'upstream'\n",
            )
            .unwrap();
            let before = repo.success(&["show-ref"]).stdout;
            let config_before = fs::read(repo.path().join(".git/config")).unwrap();
            let snapshot_before = snapshot::snapshot(repo.path());
            let branches = complete(repo.path(), &["tgi", "start", ""]);
            for expected in [
                "feat/avatars",
                "feat/login",
                "feat/remote",
                "中文/修复",
                "topic=literal",
                "literal/$(echo-pwned);'quote'",
            ] {
                assert!(branches.contains(&expected.into()), "{branches:?}");
            }
            for excluded in [
                "main",
                "private/secret",
                "alias",
                "target",
                "unrelated",
                "origin/feat/remote",
            ] {
                assert!(!branches.contains(&excluded.into()), "{branches:?}");
            }
            assert_eq!(
                complete(repo.path(), &["tgi", "start", "feat/"]),
                ["feat/avatars", "feat/login", "feat/remote"]
            );
            let targets = complete(repo.path(), &["tgi", "start", "new", "--target", ""]);
            assert!(targets.contains(&"target".into()));
            assert!(!targets.contains(&"feat/remote".into()));
            assert_eq!(before, repo.success(&["show-ref"]).stdout);
            assert_eq!(
                config_before,
                fs::read(repo.path().join(".git/config")).unwrap()
            );
            assert_eq!(snapshot_before, snapshot::snapshot(repo.path()));
            repo.success(&["branch", "feat/new"]);
            repo.success(&["branch", "-D", "feat/login"]);
            let nested = repo.path().join("nested/path");
            fs::create_dir_all(&nested).unwrap();
            assert_eq!(
                complete(&nested, &["twigsmith", "s", "feat/"]),
                ["feat/avatars", "feat/new", "feat/remote"]
            );
            // A malformed settings layer limits mapped hints, never local grammar.
            fs::write(
                repo.path().join("twigsmith.config.toml"),
                "invalid = true\n",
            )
            .unwrap();
            assert_eq!(
                complete(&nested, &["tgi", "start", "feat/"]),
                ["feat/avatars", "feat/new"]
            );
            assert!(complete(&nested, &["tgi", "start", "--p"]).contains(&"--policy".into()));
        },
    );
}

#[test]
fn installed_help_and_completion_option_vocabulary_match() {
    use std::collections::BTreeSet;
    let directory = tempfile::tempdir().unwrap();
    for command in ["start", "save", "finish", "status", "config"] {
        let output = Command::new(env!("CARGO_BIN_EXE_tgi"))
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        let mut options = BTreeSet::new();
        for line in help
            .lines()
            .map(str::trim_start)
            .filter(|line| line.starts_with('-'))
        {
            for token in line.split_whitespace().take(2) {
                let token = token.trim_end_matches(',');
                if token.starts_with('-') {
                    options.insert(token.to_owned());
                }
            }
        }
        let completed: BTreeSet<_> = complete(directory.path(), &["tgi", command, "-"])
            .into_iter()
            .collect();
        assert_eq!(completed, options, "{command} help/completion mismatch");
    }
}

#[test]
fn every_completion_git_process_uses_inspection_guards() {
    #[cfg(unix)]
    isolated(
        "every_completion_git_process_uses_inspection_guards",
        true,
        |root| {
            use std::os::unix::fs::PermissionsExt;
            let repo = Repository::new(root);
            repo.success(&["commit", "--allow-empty", "-m", "initial"]);
            repo.success(&["branch", "feature"]);
            repo.success(&["tag", "version"]);
            repo.success(&[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ]);
            repo.success(&["update-ref", "refs/remotes/origin/remote", "HEAD"]);
            let real_git = Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap();
            let real_git = String::from_utf8(real_git.stdout).unwrap();
            let bin = root.join("spy-bin");
            fs::create_dir(&bin).unwrap();
            let spy = bin.join("git");
            fs::write(&spy, "#!/bin/sh\nprintf '%s\\n' CALL \"$GIT_TERMINAL_PROMPT\" \"$LC_ALL\" \"$@\" >> \"$SPY_LOG\"\nexec \"$REAL_GIT\" \"$@\"\n").unwrap();
            fs::set_permissions(&spy, fs::Permissions::from_mode(0o755)).unwrap();
            let log = root.join("git-calls");
            let mut paths = vec![bin];
            paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
            let path = std::env::join_paths(paths).unwrap();
            let before = snapshot::snapshot(repo.path());
            for words in [
                vec!["tgi", "start", ""],
                vec!["tgi", "start", "new", "--from", ""],
                vec!["tgi", "start", "new", "--from-tag", ""],
                vec!["tgi", "finish", "--merge-back", ""],
            ] {
                let output = Command::new(env!("CARGO_BIN_EXE_tgi"))
                    .current_dir(repo.path())
                    .args([
                        "__complete",
                        "--shell",
                        "bash",
                        "--index",
                        &(words.len() - 1).to_string(),
                        "--",
                    ])
                    .args(words)
                    .env("PATH", &path)
                    .env("REAL_GIT", real_git.trim())
                    .env("SPY_LOG", &log)
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert!(output.stderr.is_empty());
                assert!(!output.stdout.is_empty());
            }
            let calls = fs::read_to_string(log).unwrap();
            let mut count = 0;
            for call in calls.split("CALL\n").filter(|call| !call.is_empty()) {
                count += 1;
                let prefix =
                    "0\nC\n--no-optional-locks\n--no-lazy-fetch\n-c\ncore.fsmonitor=false\n";
                let call = call
                    .strip_prefix(prefix)
                    .unwrap_or_else(|| panic!("unguarded inspection: {call}"));
                assert!(
                    matches!(
                        call.lines().next(),
                        Some(
                            "for-each-ref"
                                | "rev-parse"
                                | "config"
                                | "check-ref-format"
                                | "symbolic-ref"
                        )
                    ),
                    "unexpected Git command: {call}"
                );
            }
            assert!(count > 8, "missing real inspection coverage: {calls}");
            assert_eq!(before, snapshot::snapshot(repo.path()));
        },
    );
}

#[test]
fn sources_are_commit_refs_and_tag_names_are_separate() {
    isolated(
        "sources_are_commit_refs_and_tag_names_are_separate",
        true,
        |root| {
            let repo = Repository::new(root);
            repo.success(&["commit", "--allow-empty", "-m", "initial"]);
            repo.success(&["branch", "same"]);
            repo.success(&["tag", "same"]);
            repo.success(&["tag", "-a", "version/1", "-m", "annotated"]);
            fs::write(repo.path().join("blob"), "blob").unwrap();
            let blob = repo.success(&["hash-object", "-w", "blob"]);
            let blob = std::str::from_utf8(&blob.stdout).unwrap().trim();
            repo.success(&["tag", "blob-tag", blob]);
            repo.success(&["tag", "-a", "annotated-blob", blob, "-m", "blob"]);
            let sources = complete(repo.path(), &["tgi", "start", "new", "--from", ""]);
            assert!(sources.contains(&"HEAD".into()));
            assert!(sources.contains(&"refs/heads/same".into()));
            assert!(sources.contains(&"refs/tags/same".into()));
            assert!(!sources.contains(&"same".into()));
            assert!(!sources.iter().any(|source| source.contains("blob")));
            assert_eq!(
                complete(repo.path(), &["tgi", "start", "new", "--from-tag", ""]),
                ["same", "version/1"]
            );
            assert!(complete(repo.path(), &["tgi", "finish", "--tag", ""]).is_empty());
        },
    );
}

#[test]
fn pending_tag_values_preserve_option_spelling() {
    isolated(
        "pending_tag_values_preserve_option_spelling",
        true,
        |root| {
            let repo = Repository::new(root);
            repo.success(&["commit", "--allow-empty", "-m", "initial"]);
            repo.success(&["update-ref", "refs/tags/--version=one", "HEAD"]);
            for prefix in ["", "--version="] {
                assert_eq!(
                    complete(repo.path(), &["tgi", "start", "new", "--from-tag", prefix]),
                    ["--version=one"]
                );
            }
            assert!(complete(repo.path(), &["tgi", "finish", "--mode=int"]).is_empty());
            let result = Command::new(env!("CARGO_BIN_EXE_tgi"))
                .current_dir(repo.path())
                .args([
                    "start",
                    "new",
                    "--from-tag",
                    "--version=one",
                    "--policy",
                    "hotfix",
                ])
                .output()
                .unwrap();
            // Parsing succeeds; this fixture deliberately lacks the required hotfix
            // policy. Its operational error demonstrates literal value consumption.
            assert_eq!(result.status.code(), Some(1));
            assert!(
                String::from_utf8(result.stderr)
                    .unwrap()
                    .contains("configure workstreams.hotfix")
            );
        },
    );
}

#[test]
fn backend_is_silent_and_identical_for_both_executables() {
    isolated(
        "backend_is_silent_and_identical_for_both_executables",
        false,
        |root| {
            for executable in [env!("CARGO_BIN_EXE_tgi"), env!("CARGO_BIN_EXE_twigsmith")] {
                let result = Command::new(executable)
                    .current_dir(root)
                    .args([
                        "__complete",
                        "--shell",
                        "bash",
                        "--index",
                        "3",
                        "--",
                        "tgi",
                        "finish",
                        "--mode",
                        "i",
                    ])
                    .output()
                    .unwrap();
                assert!(result.status.success());
                assert!(result.stderr.is_empty());
                assert_eq!(result.stdout, b"integrate\n");
                let result = Command::new(executable)
                    .current_dir(root)
                    .args(["__complete", "malformed"])
                    .output()
                    .unwrap();
                assert!(result.status.success());
                assert!(result.stderr.is_empty());
                assert!(result.stdout.is_empty());
            }
        },
    );
}

#[test]
fn invalid_utf8_refs_are_omitted_without_lossy_conversion() {
    #[cfg(unix)]
    isolated(
        "invalid_utf8_refs_are_omitted_without_lossy_conversion",
        true,
        |root| {
            use std::os::unix::ffi::OsStringExt;
            let repo = Repository::new(root);
            repo.success(&["commit", "--allow-empty", "-m", "initial"]);
            repo.success(&[
                OsString::from("branch"),
                OsString::from_vec(b"invalid/\xff".to_vec()),
            ]);
            assert!(
                !complete(repo.path(), &["tgi", "start", ""])
                    .iter()
                    .any(|value| value.contains("invalid"))
            );
        },
    );
}
