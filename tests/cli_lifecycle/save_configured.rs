//! Configured save and recovery through the public command lifecycle.
use super::*;
use std::process::Output;

const EXE: &str = env!("CARGO_BIN_EXE_gof");
const SHARED: &str = "git-oneflow.config.toml";
const MESSAGE: &str = "Routine checkpoint";
const STASH_LABEL: &str = "Local remainder";

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
    assert!(output.stderr.is_empty(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}
fn optional_oid(repo: &Repository, reference: &str) -> Option<String> {
    let output = git_oneflow::git::run(repo.path(), ["rev-parse", "--verify", reference]).unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().into())
}

#[test]
fn configured_mixed_save_recovers_native_work_before_plain_finish() {
    isolated(
        "save_configured::configured_mixed_save_recovers_native_work_before_plain_finish",
        true,
        |root| {
            let original = (1..=20)
                .map(|line| format!("line {line}\n"))
                .collect::<String>();
            let staged = original.replacen("line 1\n", "selected first line\n", 1);
            let working = staged.replace("line 20\n", "remaining last line\n");
            let remote = Repository::new(root);
            commit(&remote, "base", &original);
            remote.success(&["config", "receive.denyCurrentBranch", "ignore"]);
            let local = Repository::new(root);
            local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
            local.success(&["fetch", "origin", "main"]);
            local.success(&["reset", "--hard", "FETCH_HEAD"]);

            // Configure the routine once through the same guided setup offered to users.
            let mut setup = terminal::Terminal::start(EXE, local.path(), &["config"]);
            setup.until("Remote receiving work");
            setup.send(format!("\n\n\nedit\ncommit:staged,stash:unstaged\n{MESSAGE}\n{STASH_LABEL}\nedit\nreview\nnone\ndeny\nkeep\nkeep\n").as_bytes());
            setup.until("Create this file?");
            assert!(setup.output.contains("commit:staged"), "{}", setup.output);
            assert!(setup.output.contains("stash:unstaged"), "{}", setup.output);
            assert!(setup.output.contains(MESSAGE), "{}", setup.output);
            assert!(setup.output.contains(STASH_LABEL), "{}", setup.output);
            setup.send(b"yes\n");
            assert!(setup.finish().success(), "{}", setup.output);
            let configured = succeeds(run(&local, &["config", "show"]));
            for expected in ["commit:staged", "stash:unstaged", MESSAGE, STASH_LABEL] {
                assert!(
                    configured.contains(expected),
                    "missing {expected:?}: {configured}"
                );
            }
            // Shared settings belong in the project history before new work starts.
            local.success(&["add", SHARED]);
            local.success(&["commit", "-m", "Project preferences"]);
            local.success(&["push", "origin", "main"]);
            let base = value(&remote, &["rev-parse", "main"]);
            let started = succeeds(run(&local, &["start", BRANCH]));
            assert!(started.contains("Created branch"), "{started}");

            fs::write(local.path().join("base"), &staged).unwrap();
            local.success(&["add", "base"]);
            fs::write(local.path().join("base"), &working).unwrap();
            fs::write(local.path().join("notes"), "remaining new work\n").unwrap();
            let saved = succeeds(run(&local, &["save"]));
            assert!(saved.contains("Save complete"), "{saved}");
            assert!(!saved.contains("How would you like"), "{saved}");
            assert!(!saved.contains("requires"), "{saved}");
            assert_eq!(
                value(&local, &["log", "-1", "--format=%s"]),
                format!("{MESSAGE} [staged]")
            );
            assert_eq!(
                local.success(&["show", "HEAD:base"]).stdout,
                staged.as_bytes()
            );
            assert!(local.success(&["status", "--porcelain"]).stdout.is_empty());
            let checkpoint = value(&local, &["rev-parse", "HEAD"]);
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), checkpoint);
            let stash = value(&local, &["rev-parse", "refs/stash"]);
            assert!(saved.contains(&stash), "{saved}");
            assert!(
                saved.contains(&format!("{STASH_LABEL} [unstaged]")),
                "{saved}"
            );
            assert_eq!(optional_oid(&remote, "refs/stash"), None);

            // Recover using the reported stable identity and ordinary Git, preserving the group meaning.
            local.success(&["stash", "apply", "--index", &stash]);
            assert_eq!(
                fs::read(local.path().join("base")).unwrap(),
                working.as_bytes()
            );
            assert_eq!(
                fs::read(local.path().join("notes")).unwrap(),
                b"remaining new work\n"
            );
            assert_eq!(local.success(&["show", ":base"]).stdout, staged.as_bytes());
            assert!(
                local
                    .success(&["diff", "--cached", "--name-only"])
                    .stdout
                    .is_empty()
            );
            assert_eq!(local.success(&["diff", "--name-only"]).stdout, b"base\n");
            assert_eq!(
                local
                    .success(&["ls-files", "--others", "--exclude-standard"])
                    .stdout,
                b"notes\n"
            );
            let blocked = run(&local, &["finish"]);
            assert_eq!(blocked.status.code(), Some(1), "{blocked:?}");
            assert!(
                String::from_utf8_lossy(&blocked.stderr).contains("contains changes"),
                "{blocked:?}"
            );
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), checkpoint);
            assert_eq!(value(&local, &["rev-parse", "HEAD"]), checkpoint);

            local.success(&["add", "base", "notes"]);
            local.success(&["commit", "-m", "Complete recovered work"]);
            let completed = value(&local, &["rev-parse", "HEAD"]);
            let finished = succeeds(run(&local, &["finish"]));
            assert!(
                finished.contains(&format!(
                    "Next: submit origin/{BRANCH} for review into origin/main."
                )),
                "{finished}"
            );
            assert_eq!(value(&remote, &["rev-parse", WORK_REF]), completed);
            assert_eq!(
                remote
                    .success(&["show", &format!("{WORK_REF}:base")])
                    .stdout,
                working.as_bytes()
            );
            assert_eq!(
                remote
                    .success(&["show", &format!("{WORK_REF}:notes")])
                    .stdout,
                b"remaining new work\n"
            );
            assert_eq!(value(&remote, &["rev-parse", "main"]), base);
            assert_eq!(optional_oid(&remote, "refs/stash"), None);
            assert_eq!(value(&local, &["rev-parse", "refs/stash"]), stash);
            assert!(local.success(&["status", "--porcelain"]).stdout.is_empty());
        },
    );
}

#[test]
fn reported_stashes_reconstruct_in_separate_worktree_without_touching_leftover_work() {
    isolated(
        "save_configured::reported_stashes_reconstruct_in_separate_worktree_without_touching_leftover_work",
        true,
        |root| {
            let original = (1..=20)
                .map(|line| format!("line {line}\n"))
                .collect::<String>();
            let staged = original.replacen("line 1\n", "selected first line\n", 1);
            let working = staged.replace("line 20\n", "remaining last line\n");
            for (number, order) in [
                vec!["staged"],
                vec!["staged", "unstaged"],
                vec!["unstaged", "staged"],
            ]
            .into_iter()
            .enumerate()
            {
                let remote = Repository::new(root);
                commit(&remote, "base", &original);
                let local = Repository::new(root);
                local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
                local.success(&["fetch", "origin", "main"]);
                local.success(&["reset", "--hard", "FETCH_HEAD"]);
                succeeds(run(&local, &["start", BRANCH]));
                let original_head = value(&local, &["rev-parse", "HEAD"]);
                fs::write(local.path().join("base"), &staged).unwrap();
                local.success(&["add", "base"]);
                fs::write(local.path().join("base"), &working).unwrap();
                fs::write(local.path().join("notes"), "remaining new work\n").unwrap();
                let mut args = vec!["save"];
                for group in &order {
                    args.extend(["--stash", *group]);
                }
                let report = succeeds(run(&local, &args));
                assert!(report.contains("git worktree add --detach"), "{report}");
                assert_eq!(value(&remote, &["rev-parse", WORK_REF]), original_head);
                let ids = value(&local, &["stash", "list", "--format=%H"])
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                assert_eq!(ids.len(), order.len());
                let staged_id = &ids[order
                    .iter()
                    .rev()
                    .position(|group| *group == "staged")
                    .unwrap()];
                let unstaged_id = order
                    .iter()
                    .rev()
                    .position(|group| *group == "unstaged")
                    .map(|position| &ids[position]);
                let expected_current = if unstaged_id.is_some() {
                    original.clone()
                } else {
                    original.replace("line 20\n", "remaining last line\n")
                };
                assert_eq!(
                    fs::read(local.path().join("base")).unwrap(),
                    expected_current.as_bytes()
                );
                assert_eq!(
                    local.success(&["show", ":base"]).stdout,
                    original.as_bytes()
                );
                let current_status = local.success(&["status", "--porcelain"]).stdout;
                let recovered = root.join(format!("recovered-{number}"));
                local.success(&[
                    "worktree".as_ref(),
                    "add".as_ref(),
                    "--detach".as_ref(),
                    recovered.as_os_str(),
                    format!("{staged_id}^1").as_ref(),
                ]);
                let native = |args: &[&str]| {
                    let output = git_oneflow::git::run(&recovered, args).unwrap();
                    assert!(output.status.success(), "{args:?}: {output:?}");
                    output.stdout
                };
                native(&["stash", "apply", "--index", staged_id]);
                if let Some(unstaged_id) = unstaged_id {
                    assert!(report.contains("apply the staged stash first"), "{report}");
                    native(&["stash", "apply", "--index", unstaged_id]);
                    assert_eq!(
                        fs::read(recovered.join("base")).unwrap(),
                        working.as_bytes()
                    );
                    assert_eq!(
                        fs::read(recovered.join("notes")).unwrap(),
                        b"remaining new work\n"
                    );
                    assert_eq!(native(&["diff", "--name-only"]), b"base\n");
                } else {
                    assert_eq!(fs::read(recovered.join("base")).unwrap(), staged.as_bytes());
                    assert!(!recovered.join("notes").exists());
                    assert!(native(&["diff", "--name-only"]).is_empty());
                }
                assert_eq!(native(&["show", ":base"]), staged.as_bytes());
                assert_eq!(native(&["diff", "--cached", "--name-only"]), b"base\n");
                assert_eq!(
                    local.success(&["status", "--porcelain"]).stdout,
                    current_status
                );
                assert_eq!(
                    fs::read(local.path().join("base")).unwrap(),
                    expected_current.as_bytes()
                );
                assert_eq!(
                    local.success(&["show", ":base"]).stdout,
                    original.as_bytes()
                );
                assert_eq!(
                    value(&local, &["stash", "list", "--format=%H"])
                        .lines()
                        .collect::<Vec<_>>(),
                    ids.iter().map(String::as_str).collect::<Vec<_>>()
                );
            }
        },
    );
}
