#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use git_oneflow::git;
use preservation::snapshot;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use support::{Repository, isolated};

fn run(directory: &Path, args: &[&str]) -> Output {
    let invoke = |executable| {
        Command::new(executable)
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap()
    };
    let output = invoke(env!("CARGO_BIN_EXE_git-oneflow"));
    let alias = invoke(env!("CARGO_BIN_EXE_gof"));
    assert_eq!(output.status.code(), alias.status.code());
    assert_eq!(output.stdout, alias.stdout);
    assert_eq!(output.stderr, alias.stderr);
    output
}

fn status(root: &Path, directory: &Path) -> String {
    let before = snapshot(root);
    let output = run(directory, &["status"]);
    assert_eq!(snapshot(root), before, "CLI changed fixture state");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout).unwrap()
}

fn contains(output: &str, expected: &[&str]) {
    for text in expected {
        assert!(output.contains(text), "missing {text:?} in:\n{output}");
    }
}

fn commit(repo: &Repository, file: &str, content: &str) -> String {
    fs::write(repo.path().join(file), content).unwrap();
    repo.success(&["add", "--", file]);
    repo.success(&["commit", "-m", "fixture"]);
    String::from_utf8(repo.success(&["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .into()
}

#[test]
fn cli_head_changes_and_nested_invocation() {
    isolated("cli_head_changes_and_nested_invocation", true, |root| {
        let repo = Repository::new(root);
        contains(
            &status(root, repo.path()),
            &[
                "Branch: main (unborn; no commits)",
                "Working tree: clean",
                "Operations: none detected",
                "Upstream: not configured",
            ],
        );
        let oid = commit(&repo, "tracked", "base\n");
        let nested = repo.path().join("nested/deeper");
        fs::create_dir_all(&nested).unwrap();
        let clean = status(root, repo.path());
        contains(
            &clean,
            &[
                "Repository: working tree",
                "Branch: main\n",
                "Working tree: clean",
            ],
        );
        assert_eq!(clean, status(root, &nested));
        fs::write(repo.path().join("tracked"), "staged\n").unwrap();
        repo.success(&["add", "tracked"]);
        fs::write(repo.path().join("tracked"), "unstaged\n").unwrap();
        fs::write(repo.path().join("untracked\nname"), "new\n").unwrap();
        let dirty = status(root, &nested);
        contains(
            &dirty,
            &["1 staged, 1 unstaged, 0 conflicted, 1 untracked paths"],
        );
        assert!(!dirty.contains("untracked\nname"));
        assert_eq!(dirty, status(root, repo.path()));
        repo.success(&["reset", "--hard"]);
        repo.success(&["checkout", "--detach"]);
        contains(
            &status(root, repo.path()),
            &[
                &format!("HEAD: detached at {oid}"),
                "Upstream: not applicable (detached HEAD)",
            ],
        );
    });
}

#[test]
fn cli_bare_and_linked_worktrees() {
    isolated("cli_bare_and_linked_worktrees", true, |root| {
        let repo = Repository::new(root);
        commit(&repo, "tracked", "base\n");
        let linked = root.join("linked");
        repo.success(&[
            "worktree".as_ref(),
            "add".as_ref(),
            "-b".as_ref(),
            "linked".as_ref(),
            linked.as_os_str(),
        ]);
        fs::write(linked.join("tracked"), "dirty\n").unwrap();
        contains(
            &status(root, &linked),
            &[
                "Repository: linked worktree",
                "Branch: linked",
                "0 staged, 1 unstaged",
            ],
        );
        let bare = root.join("bare.git");
        repo.success(&[
            "clone".as_ref(),
            "--bare".as_ref(),
            repo.path().as_os_str(),
            bare.as_os_str(),
        ]);
        let output = status(root, &bare);
        contains(
            &output,
            &["Repository: bare (no working tree)", "Branch: main"],
        );
        assert!(!output.contains("Working tree: clean"));
    });
}

#[test]
fn cli_upstream_availability_and_local_divergence() {
    isolated(
        "cli_upstream_availability_and_local_divergence",
        true,
        |root| {
            let repo = Repository::new(root);
            let base = commit(&repo, "base", "base\n");
            // An executable transport marker makes any attempted remote contact visible.
            repo.success(&["remote", "add", "origin", "ext::touch remote-was-contacted"]);
            repo.success(&["config", "protocol.ext.allow", "always"]);
            repo.success(&["config", "branch.main.remote", "origin"]);
            let incomplete = status(root, repo.path());
            contains(
                &incomplete,
                &[
                    "remote origin; merge unspecified",
                    "Upstream comparison: unavailable",
                ],
            );
            assert!(!incomplete.contains("0 ahead"));
            repo.success(&["config", "branch.main.merge", "refs/heads/main"]);
            contains(
                &status(root, repo.path()),
                &[
                    "Local upstream reference: refs/remotes/origin/main",
                    "local reference unavailable",
                ],
            );
            repo.success(&["update-ref", "refs/remotes/origin/main", &base]);
            contains(
                &status(root, repo.path()),
                &["0 ahead, 0 behind (locally available references/history only)"],
            );
            commit(&repo, "local", "local\n");
            contains(&status(root, repo.path()), &["1 ahead, 0 behind"]);
            repo.success(&["checkout", "--detach", &base]);
            let upstream = commit(&repo, "upstream", "upstream\n");
            repo.success(&["update-ref", "refs/remotes/origin/main", &upstream]);
            repo.success(&["checkout", "main"]);
            contains(&status(root, repo.path()), &["1 ahead, 1 behind"]);
            repo.success(&["reset", "--hard", &base]);
            contains(&status(root, repo.path()), &["0 ahead, 1 behind"]);
            repo.success(&["checkout", "--orphan", "unborn"]);
            repo.success(&["config", "branch.unborn.remote", "origin"]);
            repo.success(&["config", "branch.unborn.merge", "refs/heads/main"]);
            let unborn = status(root, repo.path());
            contains(
                &unborn,
                &["Upstream comparison: unavailable (HEAD has no commits)"],
            );
            assert!(!unborn.contains("0 ahead"));
            assert!(!repo.path().join("remote-was-contacted").exists());
        },
    );
}

#[test]
fn cli_operations_are_successful_observations() {
    isolated("cli_operations_are_successful_observations", true, |root| {
        for operation in ["merge", "rebase", "rebase-apply", "cherry-pick", "revert"] {
            let repo = Repository::new(root);
            let base = commit(&repo, "conflict", "base\n");
            repo.success(&["checkout", "-b", "other"]);
            let other = commit(&repo, "conflict", "other\n");
            repo.success(&["checkout", "main"]);
            commit(&repo, "conflict", "main\n");
            let args = match operation {
                "merge" => vec!["merge", "other"],
                "rebase" => vec!["rebase", "other"],
                "rebase-apply" => vec!["rebase", "--apply", "other"],
                "cherry-pick" => vec!["cherry-pick", &other],
                _ => vec!["revert", "--no-edit", &base],
            };
            assert!(!git::run(repo.path(), args).unwrap().status.success());
            let output = status(root, repo.path());
            let operation = if operation == "rebase-apply" {
                "rebase"
            } else {
                operation
            };
            contains(
                &output,
                &["1 conflicted", &format!("Operations: {operation}")],
            );
        }
    });
}

#[test]
fn cli_submodule_changes() {
    isolated("cli_submodule_changes", true, |root| {
        let child = Repository::new(root);
        commit(&child, "tracked", "base\n");
        let parent = Repository::new(root);
        parent.success(&[
            "-c".as_ref(),
            "protocol.file.allow=always".as_ref(),
            "submodule".as_ref(),
            "add".as_ref(),
            child.path().as_os_str(),
            "module".as_ref(),
        ]);
        parent.success(&["commit", "-m", "submodule"]);
        fs::write(parent.path().join("module/tracked"), "changed\n").unwrap();
        fs::write(parent.path().join("module/untracked"), "new\n").unwrap();
        parent.success(&["config", "submodule.module.ignore", "all"]);
        contains(
            &status(root, parent.path()),
            &[
                "0 staged, 1 unstaged",
                "Submodules: 1 with commit or content changes",
            ],
        );
    });
}

#[test]
fn cli_runtime_failures_and_usage_errors() {
    isolated("cli_runtime_failures_and_usage_errors", true, |root| {
        let outside = run(root, &["status"]);
        assert_eq!(outside.status.code(), Some(1));
        assert!(outside.stdout.is_empty());
        contains(
            &String::from_utf8(outside.stderr).unwrap(),
            &["outside a Git repository"],
        );
        let repo = Repository::new(root);
        commit(&repo, "tracked", "base\n");
        let unsupported = run(&repo.path().join(".git"), &["status"]);
        assert_eq!(unsupported.status.code(), Some(1));
        assert!(unsupported.stdout.is_empty());
        contains(
            &String::from_utf8(unsupported.stderr).unwrap(),
            &["work tree"],
        );
        fs::write(repo.path().join(".git/index"), "malformed index").unwrap();
        let before = snapshot(root);
        let failure = run(repo.path(), &["status"]);
        assert_eq!(snapshot(root), before);
        assert_eq!(failure.status.code(), Some(1));
        assert!(failure.stdout.is_empty());
        contains(
            &String::from_utf8(failure.stderr).unwrap(),
            &["repository inspection Git exit", "index"],
        );
        for args in [
            vec!["status", "extra"],
            vec!["status", "--version"],
            vec!["status", "--help", "extra"],
        ] {
            let output = run(repo.path(), &args);
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            contains(&String::from_utf8(output.stderr).unwrap(), &["use --help"]);
        }
    });
}

#[test]
fn cli_missing_git_and_status_help() {
    isolated("cli_missing_git_and_status_help", false, |root| {
        let output = run(root, &["status"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        contains(
            &String::from_utf8(output.stderr).unwrap(),
            &["Git is installed and available on PATH"],
        );
        for args in [["status", "--help"], ["status", "-h"]] {
            let help = run(root, &args);
            assert!(help.status.success());
            assert!(help.stderr.is_empty());
            contains(
                &String::from_utf8(help.stdout).unwrap(),
                &[
                    "Usage: git-oneflow status",
                    "locally available references/history only",
                    "no fetch",
                ],
            );
        }
        let invalid = run(root, &["status", "extra"]);
        assert_eq!(invalid.status.code(), Some(2));
    });
}

#[test]
fn cli_does_not_refresh_index_or_run_fsmonitor() {
    use std::os::unix::fs::PermissionsExt;
    isolated(
        "cli_does_not_refresh_index_or_run_fsmonitor",
        true,
        |root| {
            let repo = Repository::new(root);
            commit(&repo, "tracked", "base\n");
            let hook = root.join("fsmonitor-hook");
            fs::write(
                &hook,
                "#!/bin/sh\nprintf called > fsmonitor-was-called\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            repo.success(&[
                "config".as_ref(),
                "core.fsmonitor".as_ref(),
                hook.as_os_str(),
            ]);
            // Same bytes but fresh stat data: an ordinary Git status refreshes this index.
            fs::write(repo.path().join("tracked"), "base\n").unwrap();
            status(root, repo.path());
            assert!(!repo.path().join("fsmonitor-was-called").exists());
            fs::write(repo.path().join(".git/index.lock"), "existing lock").unwrap();
            status(root, repo.path());
            assert_eq!(
                fs::read(repo.path().join(".git/index.lock")).unwrap(),
                b"existing lock"
            );
        },
    );
}

#[test]
fn cli_missing_promisor_object_fails_without_fetching() {
    isolated(
        "cli_missing_promisor_object_fails_without_fetching",
        true,
        |root| {
            let source = Repository::new(root);
            let oid = commit(&source, "tracked", "base\n");
            let repo = Repository::new(root);
            repo.success(&["fetch".as_ref(), source.path().as_os_str(), "main".as_ref()]);
            repo.success(&["reset", "--hard", &oid]);
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                source.path().as_os_str(),
            ]);
            repo.success(&["config", "remote.origin.promisor", "true"]);
            repo.success(&["config", "remote.origin.partialclonefilter", "blob:none"]);
            repo.success(&[
                "config",
                "remote.origin.uploadpack",
                "touch fetch-was-called; git-upload-pack",
            ]);
            fs::remove_file(
                repo.path()
                    .join(".git/objects")
                    .join(&oid[..2])
                    .join(&oid[2..]),
            )
            .unwrap();
            let before = snapshot(root);
            let failure = run(repo.path(), &["status"]);
            assert_eq!(failure.status.code(), Some(1));
            assert!(failure.stdout.is_empty());
            contains(
                &String::from_utf8(failure.stderr).unwrap(),
                &["repository inspection Git exit"],
            );
            assert_eq!(snapshot(root), before);
            assert!(!repo.path().join("fetch-was-called").exists());
            // Verify that this fixture detects the ordinary lazy-fetch path.
            let _ = git::run(repo.path(), ["cat-file", "-e", &oid]).unwrap();
            assert!(repo.path().join("fetch-was-called").exists());
        },
    );
}
