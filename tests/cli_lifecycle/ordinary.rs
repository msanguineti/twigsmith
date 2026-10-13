//! Branching ordinary journeys: each starts through the public command.
use super::*;
use std::process::Output;

const EXE: &str = env!("CARGO_BIN_EXE_gof");
const SHARED: &str = "git-oneflow.config.toml";

fn run(repo: &Repository, args: &[&str]) -> Output {
    Command::new(EXE)
        .current_dir(repo.path())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn succeeds(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn fails(output: Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn fixture(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base", "initial\n");
    remote.success(&["config", "receive.denyCurrentBranch", "ignore"]);
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin", "main"]);
    local.success(&["reset", "--hard", "FETCH_HEAD"]);
    succeeds(run(&local, &["start", BRANCH]));
    assert_eq!(value(&local, &["branch", "--show-current"]), BRANCH);
    (local, remote)
}
fn optional_oid(repo: &Repository, reference: &str) -> Option<String> {
    let output = git_oneflow::git::run(repo.path(), ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().into())
}
fn clean(repo: &Repository) {
    assert!(repo.success(&["status", "--porcelain"]).stdout.is_empty());
}
fn generated_wip(local: &Repository, remote: &Repository) -> String {
    succeeds(run(local, &["config", "--yes"]));
    assert!(local.path().join(SHARED).is_file());
    fs::write(local.path().join("feature"), "feature contents\n").unwrap();
    succeeds(run(local, &["save", "--wip"]));
    clean(local);
    let saved = value(local, &["rev-parse", "HEAD"]);
    assert_eq!(value(remote, &["rev-parse", WORK_REF]), saved);
    assert!(value(local, &["log", "-1", "--format=%s"]).contains("WIP"));
    assert_eq!(
        local.success(&["show", &format!("HEAD:{SHARED}")]).stdout,
        fs::read(local.path().join(SHARED)).unwrap()
    );
    saved
}

#[cfg(unix)]
fn editor(root: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}
#[cfg(unix)]
fn reword_editors(root: &Path, local: &Repository) {
    let sequence = editor(
        root,
        "sequence",
        "printf sequence >> .git/editor-count; sed 's/^pick /reword /' \"$1\" > \"$1.new\"; mv \"$1.new\" \"$1\"",
    );
    let message = editor(
        root,
        "message",
        "printf message >> .git/editor-count; printf 'Prepared feature\\n' > \"$1\"",
    );
    local.success(&["config", "sequence.editor", &sequence]);
    local.success(&["config", "core.editor", &message]);
}
#[cfg(unix)]
fn forbid_work_republication(remote: &Repository) {
    let hooks = remote.path().join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    remote.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    // Deletion is permitted; any update of the saved work ref would hide an
    // unwanted source publication before cleanup removes it.
    editor(
        &hooks,
        "pre-receive",
        &format!(
            "while read old new reference; do\n  if [ \"$reference\" = '{WORK_REF}' ]; then\n    case \"$new\" in *[!0]*) echo work-republication-forbidden >&2; exit 1 ;; esac\n  fi\ndone"
        ),
    );
}
fn assert_completed(local: &Repository, remote: &Repository, original: &str) {
    assert_eq!(value(local, &["branch", "--show-current"]), "main");
    let result = value(local, &["rev-parse", "main"]);
    assert_ne!(result, original);
    assert_eq!(result, value(remote, &["rev-parse", "main"]));
    assert_eq!(
        result,
        value(local, &["rev-parse", "refs/remotes/origin/main"])
    );
    assert_eq!(
        optional_oid(local, "refs/remotes/origin/work/parser-fix"),
        None
    );
    assert_eq!(
        value(local, &["log", "-1", "--format=%s"]),
        "Prepared feature"
    );
    assert_eq!(
        local.success(&["show", "main:feature"]).stdout,
        b"feature contents\n"
    );
    assert!(local.path().join(SHARED).is_file());
    assert_eq!(optional_oid(local, WORK_REF), None);
    assert_eq!(optional_oid(remote, WORK_REF), None);
    clean(local);
}
fn assert_delivery_and_cleanup(text: &str) {
    for expected in [
        "into origin/main",
        "delivery confirmed",
        "Final checkout: main",
        "Work branches: local and published deleted",
    ] {
        assert!(text.contains(expected), "missing {expected:?}: {text}");
    }
    assert!(!text.contains("Fetched objects"), "{text}");
    assert!(
        !text.contains("Retained integration recovery evidence"),
        "{text}"
    );
}

#[test]
fn staged_save_retains_residual_work_then_commit_resume_and_squash_local_cleanup() {
    isolated(
        "ordinary::staged_save_retains_residual_work_then_commit_resume_and_squash_local_cleanup",
        true,
        |root| {
            let (local, remote) = fixture(root);
            fs::write(local.path().join("base"), "selected\n").unwrap();
            local.success(&["add", "base"]);
            fs::write(local.path().join("base"), "remaining\n").unwrap();
            fs::write(local.path().join("notes"), "untracked notes\n").unwrap();
            let text = succeeds(run(
                &local,
                &["save", "--staged", "--message", "Selected change"],
            ));
            assert!(text.contains("1 unstaged tracked, 1 untracked"), "{text}");
            assert_eq!(local.success(&["show", "HEAD:base"]).stdout, b"selected\n");
            let selected = value(&local, &["rev-parse", "HEAD"]);
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), selected);
            let blocked = fails(run(
                &local,
                &[
                    "finish",
                    "--mode",
                    "integrate",
                    "--integration",
                    "squash",
                    "-m",
                    "Complete change",
                    "--cleanup",
                    "local",
                ],
            ));
            assert!(blocked.contains("contains changes"), "{blocked}");
            assert!(blocked.contains("commit or preserve"), "{blocked}");
            assert_eq!(fs::read(local.path().join("base")).unwrap(), b"remaining\n");
            assert_eq!(
                fs::read(local.path().join("notes")).unwrap(),
                b"untracked notes\n"
            );
            local.success(&["add", "."]);
            local.success(&["commit", "-m", "Complete residual work"]);
            local.success(&["switch", "main"]);
            succeeds(run(&local, &["start", BRANCH]));
            succeeds(run(&local, &["save"]));
            let published = value(&remote, &["rev-parse", WORK_REF]);
            let base = value(&remote, &["rev-parse", "main"]);
            succeeds(run(
                &local,
                &[
                    "finish",
                    "--mode",
                    "integrate",
                    "--update",
                    "none",
                    "--integration",
                    "squash",
                    "-m",
                    "Complete change",
                    "--cleanup",
                    "local",
                ],
            ));
            assert_eq!(value(&local, &["rev-parse", "main^"]), base);
            assert_eq!(
                value(&local, &["log", "-1", "--format=%s"]),
                "Complete change"
            );
            assert_eq!(
                value(&local, &["rev-parse", "main"]),
                value(&remote, &["rev-parse", "main"])
            );
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), published);
            assert_eq!(optional_oid(&local, WORK_REF), None);
            assert_eq!(fs::read(local.path().join("base")).unwrap(), b"remaining\n");
            assert_eq!(
                fs::read(local.path().join("notes")).unwrap(),
                b"untracked notes\n"
            );
            clean(&local);
        },
    );
}

#[test]
fn stash_switch_resume_restore_index_save_again_and_merge_both_cleanup() {
    isolated(
        "ordinary::stash_switch_resume_restore_index_save_again_and_merge_both_cleanup",
        true,
        |root| {
            let (local, remote) = fixture(root);
            fs::create_dir_all(local.path().join(".git/info")).unwrap();
            fs::write(local.path().join(".git/info/exclude"), "ignored\n").unwrap();
            fs::write(local.path().join("ignored"), "private scratch\n").unwrap();
            fs::write(local.path().join("base"), "selected\n").unwrap();
            local.success(&["add", "base"]);
            fs::write(local.path().join("base"), "remaining\n").unwrap();
            fs::write(local.path().join("new"), "new work\n").unwrap();
            let saved = value(&local, &["rev-parse", "HEAD"]);
            let text = succeeds(run(&local, &["save", "--stash"]));
            assert!(text.contains("git stash apply --index"), "{text}");
            let stash = value(&local, &["rev-parse", "refs/stash"]);
            clean(&local);
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), saved);
            local.success(&["switch", "main"]);
            succeeds(run(&local, &["start", BRANCH]));
            local.success(&["stash", "apply", "--index", &stash]);
            assert_eq!(local.success(&["show", ":base"]).stdout, b"selected\n");
            assert_eq!(fs::read(local.path().join("base")).unwrap(), b"remaining\n");
            assert_eq!(fs::read(local.path().join("new")).unwrap(), b"new work\n");
            succeeds(run(&local, &["save", "--wip"]));
            let source = value(&local, &["rev-parse", "HEAD"]);
            let base = value(&remote, &["rev-parse", "main"]);
            succeeds(run(
                &local,
                &[
                    "finish",
                    "--mode",
                    "integrate",
                    "--update",
                    "none",
                    "--integration",
                    "merge",
                    "-m",
                    "Restored work",
                    "--cleanup",
                    "both",
                ],
            ));
            assert_eq!(value(&local, &["rev-parse", "HEAD^1"]), base);
            assert_eq!(value(&local, &["rev-parse", "HEAD^2"]), source);
            assert_eq!(
                value(&remote, &["rev-parse", "main"]),
                value(&local, &["rev-parse", "HEAD"])
            );
            assert_eq!(optional_oid(&local, WORK_REF), None);
            assert_eq!(optional_oid(&remote, WORK_REF), None);
            assert_eq!(value(&local, &["rev-parse", "refs/stash"]), stash);
            assert_eq!(
                fs::read(local.path().join("ignored")).unwrap(),
                b"private scratch\n"
            );
            clean(&local);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn publish_only_retains_local_work_cancel_then_wip_review_readiness() {
    isolated(
        "ordinary::publish_only_retains_local_work_cancel_then_wip_review_readiness",
        true,
        |root| {
            let (local, remote) = fixture(root);
            commit(&local, "feature", "existing commit\n");
            fs::write(local.path().join("base"), "selected\n").unwrap();
            local.success(&["add", "base"]);
            fs::write(local.path().join("base"), "remaining\n").unwrap();
            fs::write(local.path().join("notes"), "notes\n").unwrap();
            let source = value(&local, &["rev-parse", "HEAD"]);
            let index = fs::read(local.path().join(".git/index")).unwrap();
            succeeds(run(&local, &["save", "--publish-only"]));
            assert_eq!(value(&local, &["rev-parse", "HEAD"]), source);
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), source);
            assert_eq!(fs::read(local.path().join(".git/index")).unwrap(), index);
            assert_eq!(fs::read(local.path().join("base")).unwrap(), b"remaining\n");
            assert_eq!(fs::read(local.path().join("notes")).unwrap(), b"notes\n");
            succeeds(run(&local, &["save", "--wip"]));
            let saved = value(&local, &["rev-parse", "HEAD"]);
            let base = value(&remote, &["rev-parse", "main"]);
            let mut term = terminal::Terminal::start(EXE, local.path(), &["finish"]);
            term.until("Completion mode (");
            term.send(b"cancel\n");
            assert_eq!(term.finish().code(), Some(1), "{}", term.output);
            assert!(
                term.output.to_lowercase().contains("cancel"),
                "{}",
                term.output
            );
            assert_eq!(value(&local, &["rev-parse", "HEAD"]), saved);
            assert_eq!(value(&remote, &["rev-parse", "main"]), base);
            let text = succeeds(run(
                &local,
                &[
                    "finish",
                    "--mode",
                    "review",
                    "--update",
                    "none",
                    "--no-rewrite",
                ],
            ));
            assert!(text.contains("submit origin/work/parser-fix"), "{text}");
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), saved);
            assert_eq!(value(&remote, &["rev-parse", "main"]), base);
            assert_eq!(value(&local, &["branch", "--show-current"]), BRANCH);
            clean(&local);
        },
    );
}

#[test]
fn wip_then_further_commit_rebase_advanced_base_merge_and_keep_work() {
    isolated(
        "ordinary::wip_then_further_commit_rebase_advanced_base_merge_and_keep_work",
        true,
        |root| {
            let (local, remote) = fixture(root);
            fs::write(local.path().join("feature"), "draft\n").unwrap();
            succeeds(run(&local, &["save", "--wip"]));
            let published = value(&remote, &["rev-parse", WORK_REF]);
            commit(&local, "feature", "finished\n");
            commit(&remote, "incoming", "project update\n");
            let base = value(&remote, &["rev-parse", "main"]);
            succeeds(run(
                &local,
                &[
                    "finish",
                    "--mode",
                    "integrate",
                    "--update",
                    "rebase",
                    "--integration",
                    "merge",
                    "-m",
                    "Completed feature",
                    "--cleanup",
                    "keep",
                ],
            ));
            let source = value(&local, &["rev-parse", WORK_REF]);
            assert_ne!(source, published);
            assert_eq!(value(&local, &["rev-parse", "main^1"]), base);
            assert_eq!(value(&local, &["rev-parse", "main^2"]), source);
            assert_eq!(
                value(&remote, &["rev-parse", "main"]),
                value(&local, &["rev-parse", "main"])
            );
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), published);
            assert_eq!(
                fs::read(local.path().join("feature")).unwrap(),
                b"finished\n"
            );
            assert_eq!(
                fs::read(local.path().join("incoming")).unwrap(),
                b"project update\n"
            );
            clean(&local);
        },
    );
}

#[cfg(unix)]
#[test]
fn reworded_published_config_check_failure_retries_without_reediting_or_duplicate_integration() {
    isolated(
        "ordinary::reworded_published_config_check_failure_retries_without_reediting_or_duplicate_integration",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let original = generated_wip(&local, &remote);
            reword_editors(root, &local);
            fs::write(local.path().join(".git/git-oneflow.config.toml"), "[[finish-checks]]\nname='Candidate gate'\ncommand=['sh','-c','echo check >> .git/check-count; test -f .git/check-pass']\n").unwrap();
            let args = [
                "finish",
                "--mode",
                "integrate",
                "--update",
                "rebase",
                "--history-edit",
                "interactive",
                "--integration",
                "ff-only",
                "--cleanup",
                "both",
            ];
            let base = value(&remote, &["rev-parse", "main"]);
            let text = fails(run(&local, &args));
            assert!(text.contains("Candidate gate"), "{text}");
            assert!(text.contains("rerun finish"), "{text}");
            assert_eq!(value(&remote, &["rev-parse", "main"]), base);
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), original);
            let prepared = value(&local, &["rev-parse", WORK_REF]);
            let editor_count = fs::read(local.path().join(".git/editor-count")).unwrap();
            assert_ne!(prepared, original);
            fs::write(local.path().join(".git/check-pass"), "").unwrap();
            succeeds(run(&local, &["finish", "--mode", "integrate"]));
            assert_eq!(
                fs::read(local.path().join(".git/editor-count")).unwrap(),
                editor_count
            );
            assert_eq!(
                fs::read_to_string(local.path().join(".git/check-count")).unwrap(),
                "check\ncheck\n"
            );
            assert_eq!(value(&remote, &["rev-parse", "main"]), prepared);
            assert_completed(&local, &remote, &original);
            let result = value(&local, &["rev-parse", "HEAD"]);
            succeeds(run(&local, &["finish", "--mode", "integrate"]));
            assert_eq!(value(&local, &["rev-parse", "HEAD"]), result);
            assert_eq!(
                fs::read_to_string(local.path().join(".git/check-count")).unwrap(),
                "check\ncheck\n"
            );
            assert_eq!(
                fs::read(local.path().join(".git/editor-count")).unwrap(),
                editor_count
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn another_developer_publication_after_reword_preserves_both_work_versions() {
    isolated(
        "ordinary::another_developer_publication_after_reword_preserves_both_work_versions",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let original = generated_wip(&local, &remote);
            reword_editors(root, &local);
            fs::write(local.path().join(".git/git-oneflow.config.toml"), "[[finish-checks]]\nname='Candidate gate'\ncommand=['sh','-c','test -f .git/check-pass']\n").unwrap();
            let args = [
                "finish",
                "--mode",
                "integrate",
                "--update",
                "rebase",
                "--history-edit",
                "interactive",
                "--integration",
                "ff-only",
                "--cleanup",
                "both",
            ];
            let base = value(&remote, &["rev-parse", "main"]);
            let first = fails(run(&local, &args));
            assert!(first.contains("Candidate gate"), "{first}");
            let prepared = value(&local, &["rev-parse", WORK_REF]);
            let count = fs::read(local.path().join(".git/editor-count")).unwrap();
            let colleague = Repository::new(root);
            colleague.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
            colleague.success(&["fetch", "origin", BRANCH]);
            colleague.success(&["reset", "--hard", "FETCH_HEAD"]);
            assert_eq!(value(&colleague, &["rev-parse", "HEAD"]), original);
            commit(&colleague, "colleague", "new published work\n");
            colleague.success(&["push", "origin", &format!("HEAD:{WORK_REF}")]);
            let changed = value(&colleague, &["rev-parse", "HEAD"]);
            fs::write(local.path().join(".git/check-pass"), "").unwrap();
            let blocked = fails(run(&local, &["finish", "--mode", "integrate"]));
            assert!(blocked.contains("published work changed"), "{blocked}");
            assert!(blocked.contains("inspect"), "{blocked}");
            assert!(
                !blocked.contains("requires resolved input and satisfied constraints"),
                "{blocked}"
            );
            assert!(
                !blocked.contains(
                    "requires the same retained, verified integration attempt and resolved choices"
                ),
                "{blocked}"
            );
            assert!(blocked.contains("Target delivery: Complete"), "{blocked}");
            assert!(blocked.contains("checkout/cleanup"), "{blocked}");
            assert_eq!(value(&local, &["rev-parse", WORK_REF]), prepared);
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), changed);
            assert_ne!(prepared, base);
            assert_eq!(value(&remote, &["rev-parse", "main"]), prepared);
            assert_eq!(
                fs::read(local.path().join(".git/editor-count")).unwrap(),
                count
            );
            clean(&local);
        },
    );
}

#[test]
fn saved_work_merge_preparation_conflict_native_resolution_and_review_retry() {
    isolated(
        "ordinary::saved_work_merge_preparation_conflict_native_resolution_and_review_retry",
        true,
        |root| {
            let (local, remote) = fixture(root);
            commit(&local, "base", "work version\n");
            succeeds(run(&local, &["save"]));
            let saved = value(&local, &["rev-parse", "HEAD"]);
            commit(&remote, "base", "project version\n");
            let base = value(&remote, &["rev-parse", "main"]);
            let args = [
                "finish",
                "--mode",
                "review",
                "--update",
                "merge",
                "--no-rewrite",
            ];
            let text = fails(run(&local, &args));
            assert!(text.to_lowercase().contains("conflict"), "{text}");
            assert!(text.contains("git"), "{text}");
            assert!(local.path().join(".git/MERGE_HEAD").exists());
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), saved);
            assert_eq!(value(&remote, &["rev-parse", "main"]), base);
            fs::write(local.path().join("base"), "resolved together\n").unwrap();
            local.success(&["add", "base"]);
            local.success(&["commit", "-m", "Resolve project update"]);
            let resolved = value(&local, &["rev-parse", "HEAD"]);
            succeeds(run(&local, &args));
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), resolved);
            assert_eq!(value(&remote, &["rev-parse", "main"]), base);
            assert_eq!(
                fs::read(local.path().join("base")).unwrap(),
                b"resolved together\n"
            );
            clean(&local);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn default_config_wip_switch_resume_native_reword_and_both_cleanup() {
    isolated(
        "ordinary::default_config_wip_switch_resume_native_reword_and_both_cleanup",
        true,
        |root| {
            let (local, remote) = fixture(root);
            local.success(&["config", "branch.main.remote", "origin"]);
            local.success(&["config", "branch.main.merge", "refs/heads/main"]);
            let original = generated_wip(&local, &remote);
            local.success(&["switch", "main"]);
            assert!(!local.path().join(SHARED).exists());
            succeeds(run(&local, &["start", BRANCH]));
            assert!(local.path().join(SHARED).is_file());
            assert_eq!(value(&local, &["rev-parse", "HEAD"]), original);
            forbid_work_republication(&remote);
            reword_editors(root, &local);
            let mut term = terminal::Terminal::start(
                EXE,
                local.path(),
                &[
                    "finish",
                    "--history-edit",
                    "interactive",
                    "--mode",
                    "integrate",
                ],
            );
            term.until("Update (rebase/merge/none/cancel):");
            term.send(b"1\n");
            term.until("Cleanup (keep/local/both/cancel):");
            term.send(b"3\n");
            term.until("Integration (ff-only/merge/squash/cancel):");
            term.send(b"1\n");
            assert!(term.finish().success(), "{}", term.output);
            assert_delivery_and_cleanup(&term.output);
            assert_completed(&local, &remote, &original);
            assert_eq!(
                value(
                    &local,
                    &["rev-list", "--left-right", "--count", "main...origin/main"]
                ),
                "0\t0"
            );
            let status = value(&local, &["status", "--short", "--branch"]);
            assert!(
                !status.contains("ahead") && !status.contains("behind"),
                "{status}"
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn published_default_config_stopped_reword_fresh_process_resolves_method_without_reediting() {
    isolated(
        "ordinary::published_default_config_stopped_reword_fresh_process_resolves_method_without_reediting",
        true,
        |root| {
            let (local, remote) = fixture(root);
            let original = generated_wip(&local, &remote);
            forbid_work_republication(&remote);
            reword_editors(root, &local);
            let base = value(&remote, &["rev-parse", "main"]);
            let text = fails(run(
                &local,
                &[
                    "finish",
                    "--mode",
                    "integrate",
                    "--update",
                    "rebase",
                    "--history-edit",
                    "interactive",
                    "--cleanup",
                    "both",
                ],
            ));
            assert!(text.contains("unresolved integration input"), "{text}");
            assert!(text.contains("--integration"), "{text}");
            assert!(
                !text.contains("requires resolved input and satisfied constraints"),
                "{text}"
            );
            let prepared = value(&local, &["rev-parse", "HEAD"]);
            assert_ne!(prepared, original);
            assert_eq!(
                value(&local, &["log", "-1", "--format=%s"]),
                "Prepared feature"
            );
            assert_eq!(value(&remote, &["rev-parse", "main"]), base);
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), original);
            let count = fs::read(local.path().join(".git/editor-count")).unwrap();
            clean(&local);
            let text = succeeds(run(
                &local,
                &["finish", "--mode", "integrate", "--integration", "ff-only"],
            ));
            assert_delivery_and_cleanup(&text);
            assert!(text.lines().count() <= 10, "{text}");
            assert_eq!(
                fs::read(local.path().join(".git/editor-count")).unwrap(),
                count
            );
            assert_eq!(value(&remote, &["rev-parse", "main"]), prepared);
            assert_completed(&local, &remote, &original);
        },
    );
}
