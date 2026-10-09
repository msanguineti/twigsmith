//! Public CLI lifecycle with real project and publication repositories.
mod support;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};
use support::{Repository, isolated};

const BRANCH: &str = "work/parser-fix";
const WORK_REF: &str = "refs/heads/work/parser-fix";
fn commit(repo: &Repository, path: &str, content: &str) {
    fs::write(repo.path().join(path), content).unwrap();
    repo.success(&["add", path]);
    repo.success(&["commit", "-m", path]);
}
fn value(repo: &Repository, args: &[&str]) -> String {
    String::from_utf8(repo.success(args).stdout)
        .unwrap()
        .trim()
        .into()
}
fn cli(exe: &str, repo: &Repository, args: &[&str], expected: &str) {
    let output = Command::new(exe)
        .current_dir(repo.path())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(expected), "missing {expected:?}: {text}");
}
fn lifecycle(root: &Path, exe: &str, split: bool, base_tracking: bool) {
    let project = Repository::new(root);
    commit(&project, "base", "initial project\n");
    let fork = Repository::new(root);
    fork.success(&["fetch", project.path().to_str().unwrap(), "main"]);
    fork.success(&["reset", "--hard", "FETCH_HEAD"]);
    let fork_base = value(&fork, &["rev-parse", "main"]);
    // The fork base is stale; new work must use the project role.
    commit(&project, "new-base", "project advanced\n");
    project.success(&["branch", BRANCH]);
    let local = Repository::new(root);
    commit(&local, "local", "local base stays unchanged\n");
    let local_base = value(&local, &["rev-parse", "main"]);
    let (publish, base, destination) = if split {
        ("personal", "project", &fork)
    } else {
        ("team", "team", &project)
    };
    // Same-name project work is a decoy only in the fork case.
    if !split {
        project.success(&["branch", "-D", BRANCH]);
    }
    local.success(&[
        "remote",
        "add",
        publish,
        destination.path().to_str().unwrap(),
    ]);
    if split {
        local.success(&["remote", "add", base, project.path().to_str().unwrap()]);
    }
    fs::write(
        local.path().join(".git/git-oneflow.config.toml"),
        if split {
            "publish-remote='personal'\nbase-remote='project'\n"
        } else {
            "publish-remote='team'\n"
        },
    )
    .unwrap();
    cli(
        exe,
        &local,
        &["start", BRANCH],
        &format!("Created branch {BRANCH} from {base}/main"),
    );
    assert_eq!(
        value(&local, &["rev-parse", "HEAD"]),
        value(&project, &["rev-parse", "main"])
    );
    assert_eq!(
        value(&local, &["rev-parse", &format!("refs/remotes/{base}/main")]),
        value(&project, &["rev-parse", "main"])
    );
    assert!(
        !git_oneflow::git::run(
            local.path(),
            ["config", "--get", "branch.work/parser-fix.remote"]
        )
        .unwrap()
        .status
        .success()
    );
    commit(&local, "parser", "normal developer commit\n");
    let saved = value(&local, &["rev-parse", "HEAD"]);
    cli(exe, &local, &["save"], &format!("to remote {publish}"));
    assert_eq!(value(&local, &["rev-parse", "HEAD"]), saved);
    assert_eq!(value(&local, &["log", "-1", "--format=%s"]), "parser");
    assert_eq!(value(destination, &["rev-parse", WORK_REF]), saved);
    if split {
        assert_eq!(
            value(&project, &["rev-parse", WORK_REF]),
            value(&project, &["rev-parse", "main"])
        );
    }
    // Remove only the local published branch, then resume through the public CLI.
    local.success(&["switch", "main"]);
    local.success(&["branch", "-D", BRANCH]);
    commit(&project, "finish-base", "later project update\n");
    cli(
        exe,
        &local,
        &["start", BRANCH],
        &format!("Resumed remote branch {BRANCH} from {publish}/{BRANCH}"),
    );
    assert_eq!(value(&local, &["rev-parse", "HEAD"]), saved);
    assert_eq!(
        value(
            &local,
            &["rev-parse", &format!("refs/remotes/{publish}/{BRANCH}")]
        ),
        saved
    );
    assert_eq!(
        value(
            &local,
            &["config", "--get", "branch.work/parser-fix.remote"]
        ),
        publish
    );
    assert_eq!(
        value(&local, &["config", "--get", "branch.work/parser-fix.merge"]),
        WORK_REF
    );
    let (tracking_remote, tracking_ref) = if base_tracking {
        (base, "refs/heads/main")
    } else {
        (publish, WORK_REF)
    };
    if base_tracking {
        local.success(&["config", "branch.work/parser-fix.remote", base]);
        local.success(&["config", "branch.work/parser-fix.merge", tracking_ref]);
        local.success(&["switch", "main"]);
        cli(
            exe,
            &local,
            &["start", BRANCH],
            &format!("Resumed local branch {BRANCH}"),
        );
    }
    // Save preserves either established tracking style.
    cli(exe, &local, &["save"], "unchanged (already published)");
    let project_base = value(&project, &["rev-parse", "main"]);
    // Unresolved unattended update reports both roles before successful finish.
    let blocked = Command::new(exe)
        .current_dir(local.path())
        .arg("finish")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(blocked.status.code(), Some(1), "{blocked:?}");
    assert!(blocked.stdout.is_empty());
    let recovery = String::from_utf8(blocked.stderr).unwrap();
    assert!(
        recovery.contains(&format!(
            "Planned publication remote: {publish}; project base: {base}/main."
        )),
        "{recovery}"
    );
    assert_eq!(value(destination, &["rev-parse", WORK_REF]), saved);
    cli(
        exe,
        &local,
        &["finish", "--update", "merge", "--no-rewrite"],
        &format!("Next: submit {publish}/{BRANCH} for review into {base}/main."),
    );
    assert_eq!(
        value(destination, &["rev-parse", WORK_REF]),
        value(&local, &["rev-parse", "HEAD"])
    );
    local.success(&[
        "merge-base",
        "--is-ancestor",
        &value(&project, &["rev-parse", "main"]),
        "HEAD",
    ]);
    assert_eq!(
        value(
            &local,
            &["config", "--get", "branch.work/parser-fix.remote"]
        ),
        tracking_remote
    );
    assert_eq!(
        value(&local, &["config", "--get", "branch.work/parser-fix.merge"]),
        tracking_ref
    );
    assert_eq!(value(&local, &["rev-parse", "main"]), local_base);
    assert_eq!(value(&project, &["rev-parse", "main"]), project_base);
    assert_eq!(value(&fork, &["rev-parse", "main"]), fork_base);
    if split {
        assert_ne!(
            value(&project, &["rev-parse", WORK_REF]),
            value(&local, &["rev-parse", "HEAD"])
        );
    }
}

#[test]
fn public_cli_project_and_fork_lifecycles_preserve_both_tracking_styles() {
    isolated(
        "public_cli_project_and_fork_lifecycles_preserve_both_tracking_styles",
        true,
        |root| {
            for exe in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
                for split in [false, true] {
                    for base_tracking in [false, true] {
                        lifecycle(root, exe, split, base_tracking);
                    }
                }
            }
        },
    );
}
