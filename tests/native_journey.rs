//! A deliberately portable application slice, executed natively by the CI matrix.
#[path = "native_journey/bundles.rs"]
mod bundles;
mod support;

use std::{fs, path::Path, process::Command};
use support::{Repository, isolated};

const BRANCH: &str = "feature/literal-$;work";

fn oid(repo: &Repository, reference: &str) -> Vec<u8> {
    repo.success(&["rev-parse", reference]).stdout
}

fn commit(repo: &Repository, content: &str) -> Vec<u8> {
    fs::write(repo.path().join("tracked content.txt"), content).unwrap();
    repo.success(&["add", "tracked content.txt"]);
    repo.success(&["commit", "-m", "fixture content"]);
    oid(repo, "HEAD")
}

fn cli(executable: &str, repo: &Repository, args: &[&str]) -> std::process::Output {
    let output = native::output(Command::new(executable).current_dir(repo.path()).args(args));
    println!(
        "{executable} {args:?}: {}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

// Use independent Git observations for refs and tracked content, plus raw index
// bytes: a status implementation that refreshes the index must fail this check.
#[derive(Debug, PartialEq, Eq)]
struct State {
    head: Vec<u8>,
    refs: Vec<u8>,
    index: Vec<u8>,
    tracked: Vec<u8>,
    worktree: Vec<u8>,
}

fn state(repo: &Repository) -> State {
    State {
        head: repo.success(&["symbolic-ref", "HEAD"]).stdout,
        refs: repo.success(&["show-ref"]).stdout,
        index: fs::read(repo.path().join(".git/index")).unwrap(),
        tracked: fs::read(repo.path().join("tracked content.txt")).unwrap(),
        worktree: repo
            .success(&["--no-optional-locks", "status", "--porcelain=v1", "-uall"])
            .stdout,
    }
}

fn journey(root: &Path, starter: &str, inspector: &str) {
    let remote = Repository::new(root);
    let old = commit(&remote, "original\n");
    let local = Repository::new(root);
    // Exercise explicit settings rather than inferring roles from origin's name.
    local.success(&[
        "remote".as_ref(),
        "add".as_ref(),
        "fixture".as_ref(),
        remote.path().as_os_str(),
    ]);
    local.success(&["fetch", "fixture"]);
    local.success(&["reset", "--hard", "fixture/main"]);
    fs::write(
        local.path().join(".git/twigsmith.config.toml"),
        "base = 'main'\npublish-remote = 'fixture'\nbase-remote = 'fixture'\n",
    )
    .unwrap();
    let advanced = commit(&remote, "advanced remote content\n");
    assert_ne!(old, advanced);
    assert_eq!(oid(&local, "HEAD"), old);
    assert_eq!(oid(&local, "refs/remotes/fixture/main"), old);

    let initial = state(&local);
    println!(
        "initial HEAD: {}initial refs:\n{}advanced remote tip: {}",
        String::from_utf8_lossy(&old),
        String::from_utf8_lossy(&initial.refs),
        String::from_utf8_lossy(&advanced)
    );
    let status = cli(inspector, &local, &["status"]);
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("main"));
    assert_eq!(state(&local), initial, "status changed repository state");

    let start = cli(starter, &local, &["start", BRANCH]);
    assert!(start.status.success());
    assert_eq!(oid(&local, "HEAD"), advanced);
    assert_eq!(oid(&local, "refs/remotes/fixture/main"), advanced);
    assert_eq!(oid(&local, "refs/heads/main"), old);
    assert_eq!(
        local.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
        format!("{BRANCH}\n").as_bytes()
    );
    assert_eq!(
        local.success(&["write-tree"]).stdout,
        oid(&remote, "HEAD^{tree}"),
        "index differs from the fetched tree"
    );
    assert_eq!(
        fs::read(local.path().join("tracked content.txt")).unwrap(),
        b"advanced remote content\n"
    );
    let started = state(&local);
    assert!(started.worktree.is_empty(), "{started:?}");
    let status = cli(inspector, &local, &["status"]);
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains(BRANCH));
    assert_eq!(state(&local), started);
    println!(
        "verified fetched HEAD, index and clean tracked worktree; local main preserved; refs:\n{}",
        String::from_utf8_lossy(&started.refs)
    );

    local.success(&[
        "remote".as_ref(),
        "set-url".as_ref(),
        "fixture".as_ref(),
        root.join("absent remote with spaces").as_os_str(),
    ]);
    let before_refusal = state(&local);
    let refused = cli(starter, &local, &["start", "feature/unreachable"]);
    assert!(!refused.status.success());
    assert_eq!(state(&local), before_refusal, "refusal changed checkout");
}

#[test]
fn native_start_status_journey() {
    isolated("native_start_status_journey", true, |root| {
        println!(
            "native host: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let binaries = [env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")];
        for executable in binaries {
            for argument in ["--version", "--help"] {
                let output = Command::new(executable).arg(argument).output().unwrap();
                assert!(output.status.success(), "{output:?}");
                assert!(String::from_utf8_lossy(&output.stdout).contains("twigsmith"));
                println!("{executable} {argument}: {output:?}");
            }
        }
        for (starter, inspector) in [(binaries[0], binaries[1]), (binaries[1], binaries[0])] {
            journey(root, starter, inspector);
        }
    });
}

#[path = "support/native.rs"]
mod native;

fn delivery_fixture(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base\n");
    // A detached remote permits ordinary pushes to main without relying on a
    // server hook or a checked-out-branch exception.
    remote.success(&["switch", "--detach"]);
    let local = Repository::new(root);
    local.success(&[
        "remote".as_ref(),
        "add".as_ref(),
        "fixture".as_ref(),
        remote.path().as_os_str(),
    ]);
    local.success(&["fetch", "fixture"]);
    local.success(&["reset", "--hard", "fixture/main"]);
    (local, remote)
}

fn configure_delivery(repo: &Repository, direct: bool, checks: serde_json::Value) {
    fs::write(
        repo.path().join(".git/twigsmith.config.json"),
        serde_json::to_vec(&serde_json::json!({
            "base": "main", "base-remote": "fixture", "publish-remote": "fixture",
            "finish-mode": if direct { "integrate" } else { "review" },
            "finish-update": "none", "finish-message": "auto", "finish-integration": "squash",
            "finish-checkout": "target", "finish-cleanup": "local", "finish-checks": checks,
        }))
        .unwrap(),
    )
    .unwrap();
}

fn assert_tracking(local: &Repository, branch: &str, expected: &[u8]) {
    assert_eq!(
        oid(local, &format!("refs/remotes/fixture/{branch}")),
        expected
    );
    assert_eq!(
        local
            .success(&["config", &format!("branch.{branch}.remote")])
            .stdout,
        b"fixture\n"
    );
    assert_eq!(
        local
            .success(&["config", &format!("branch.{branch}.merge")])
            .stdout,
        format!("refs/heads/{branch}\n").as_bytes()
    );
}

#[test]
fn native_selected_save_and_ordered_review_journey() {
    isolated(
        "native_selected_save_and_ordered_review_journey",
        true,
        |root| {
            for (starter, finisher) in [
                (env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")),
                (env!("CARGO_BIN_EXE_tgi"), env!("CARGO_BIN_EXE_twigsmith")),
            ] {
                let (local, remote) = delivery_fixture(root);
                let initial_target = oid(&remote, "main");
                let log = local.path().join(".git/check-order");
                let observation = local.path().join(".git/check-observation.json");
                let arguments = [
                    "with spaces",
                    "single' double\"",
                    "κόσμος 世界",
                    "",
                    "$HOME;|&<>*?()",
                ];
                let mut record = native::command("record", &[observation.to_str().unwrap()]);
                record.extend(arguments.iter().map(|s| (*s).to_owned()));
                configure_delivery(
                    &local,
                    false,
                    serde_json::json!([
                        {"name":"Literal invocation", "command":record},
                        {"name":"First", "command":native::command("ordered", &[log.to_str().unwrap(), "first"])},
                        {"name":"Second", "command":native::command("ordered", &[log.to_str().unwrap(), "second"])}
                    ]),
                );
                assert!(cli(starter, &local, &["start", BRANCH]).status.success());
                assert!(!log.exists(), "start ran finish checks");
                fs::write(
                    local.path().join("tracked content.txt"),
                    b"selected tracked\n",
                )
                .unwrap();
                fs::write(
                    local.path().join("selected new.txt"),
                    b"selected untracked\n",
                )
                .unwrap();
                local.success(&["add", "tracked content.txt", "selected new.txt"]);
                let selected_tree = local.success(&["write-tree"]).stdout;
                fs::write(
                    local.path().join("tracked content.txt"),
                    b"remaining unstaged\n",
                )
                .unwrap();
                fs::write(
                    local.path().join("unselected new.txt"),
                    b"remaining untracked\n",
                )
                .unwrap();
                assert!(
                    cli(
                        starter,
                        &local,
                        &[
                            "save",
                            "--commit",
                            "staged",
                            "--message",
                            "Selected native work"
                        ]
                    )
                    .status
                    .success()
                );
                let saved = oid(&local, "HEAD");
                assert_eq!(oid(&local, "HEAD^{tree}"), selected_tree);
                assert_eq!(
                    local.success(&["show", "HEAD:tracked content.txt"]).stdout,
                    b"selected tracked\n"
                );
                assert_eq!(
                    local.success(&["show", "HEAD:selected new.txt"]).stdout,
                    b"selected untracked\n"
                );
                assert_eq!(
                    local.success(&["log", "-1", "--format=%s"]).stdout,
                    b"Selected native work [staged]\n"
                );
                assert_eq!(oid(&remote, BRANCH), saved);
                assert_tracking(&local, BRANCH, &saved);
                assert_eq!(
                    fs::read(local.path().join("tracked content.txt")).unwrap(),
                    b"remaining unstaged\n"
                );
                assert_eq!(
                    fs::read(local.path().join("unselected new.txt")).unwrap(),
                    b"remaining untracked\n"
                );
                assert!(
                    local
                        .success(&["diff", "--cached", "--name-only"])
                        .stdout
                        .is_empty()
                );
                assert!(!log.exists(), "save ran finish checks");
                // Preserve the independently verified unselected work before finish.
                local.success(&[
                    "stash",
                    "push",
                    "--include-untracked",
                    "-m",
                    "preserved unselected work",
                ]);
                assert_eq!(
                    local.success(&["show", "stash:tracked content.txt"]).stdout,
                    b"remaining unstaged\n"
                );
                assert_eq!(
                    local
                        .success(&["show", "stash^3:unselected new.txt"])
                        .stdout,
                    b"remaining untracked\n"
                );
                let nested = local.path().join("nested/deeper");
                fs::create_dir_all(&nested).unwrap();
                let output = native::output(
                    Command::new(finisher)
                        .current_dir(&nested)
                        .arg("finish")
                        .env("NATIVE_CHECK_VALUE", "inherited value 世界"),
                );
                assert!(output.status.success(), "{output:?}");
                let value: serde_json::Value =
                    serde_json::from_slice(&fs::read(observation).unwrap()).unwrap();
                assert_eq!(value["args"], serde_json::json!(arguments));
                assert_eq!(value["environment"], "inherited value 世界");
                assert_eq!(value["stdin"], serde_json::json!([]));
                assert_eq!(
                    Path::new(value["cwd"].as_str().unwrap())
                        .canonicalize()
                        .unwrap(),
                    local.path().canonicalize().unwrap()
                );
                let head = String::from_utf8(saved.clone()).unwrap();
                assert_eq!(
                    fs::read_to_string(log).unwrap(),
                    format!("first {head}second {head}")
                );
                assert_eq!(oid(&remote, BRANCH), saved);
                assert_eq!(
                    oid(&remote, "main"),
                    initial_target,
                    "review integrated target"
                );
                assert_eq!(oid(&local, "HEAD"), saved);
                assert_tracking(&local, BRANCH, &saved);
                assert_eq!(
                    local.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                    format!("{BRANCH}\n").as_bytes()
                );
                assert!(
                    local
                        .success(&["status", "--porcelain=v1"])
                        .stdout
                        .is_empty()
                );
                println!(
                    "{starter} save -> {finisher} review: exact selected tree, preserved unselected work, literal root check invocation and ordered checks; source published; target unchanged"
                );
            }
        },
    );
}

#[test]
fn native_direct_candidate_failure_and_retry_journey() {
    isolated(
        "native_direct_candidate_failure_and_retry_journey",
        true,
        |root| {
            for (starter, finisher) in [
                (env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")),
                (env!("CARGO_BIN_EXE_tgi"), env!("CARGO_BIN_EXE_twigsmith")),
            ] {
                let (local, remote) = delivery_fixture(root);
                let target = oid(&remote, "main");
                let log = local.path().join(".git/candidate-checks");
                let permit = local.path().join(".git/permit");
                configure_delivery(
                    &local,
                    true,
                    serde_json::json!([
                        {"name":"Candidate gate", "command":native::command("ordered", &[log.to_str().unwrap(), "candidate", permit.to_str().unwrap()])},
                        {"name":"Later", "command":native::command("ordered", &[log.to_str().unwrap(), "later"])}
                    ]),
                );
                assert!(cli(starter, &local, &["start", BRANCH]).status.success());
                let first_source = commit(&local, "source step one\n");
                let source = commit(&local, "source step two\n");
                let source_tree = oid(&local, "HEAD^{tree}");
                let failed = cli(finisher, &local, &["finish"]);
                assert_eq!(failed.status.code(), Some(1));
                let candidate = oid(&local, "main");
                assert_ne!(candidate, source, "squash must create a distinct candidate");
                assert_ne!(candidate, target);
                assert_eq!(oid(&local, "main^{tree}"), source_tree);
                assert_eq!(oid(&local, "main^"), target);
                assert_eq!(
                    local
                        .success(&["rev-list", "--parents", "-n", "1", "main"])
                        .stdout
                        .split(|b| *b == b' ')
                        .count(),
                    2,
                    "squash must have exactly one parent"
                );
                assert_eq!(
                    oid(&local, BRANCH),
                    source,
                    "failed check changed source history"
                );
                assert_eq!(
                    oid(&local, "HEAD"),
                    candidate,
                    "failed candidate must remain checked out for inspection"
                );
                assert_eq!(
                    oid(&remote, "main"),
                    target,
                    "failed check delivered candidate"
                );
                let candidate_text = String::from_utf8(candidate.clone()).unwrap();
                assert_eq!(
                    fs::read_to_string(&log).unwrap(),
                    format!("candidate {candidate_text}")
                );
                fs::write(&permit, b"allow retry").unwrap();
                assert!(cli(finisher, &local, &["finish"]).status.success());
                assert_eq!(oid(&local, "HEAD"), candidate, "retry rebuilt candidate");
                assert_eq!(oid(&remote, "main"), candidate);
                assert_eq!(oid(&remote, "main^{tree}"), source_tree);
                assert_eq!(
                    local.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                    b"main\n"
                );
                assert!(
                    !twigsmith::git::run(
                        local.path(),
                        ["show-ref", "--verify", &format!("refs/heads/{BRANCH}")]
                    )
                    .unwrap()
                    .status
                    .success(),
                    "local cleanup retained work branch"
                );
                assert_eq!(
                    fs::read_to_string(log).unwrap(),
                    format!(
                        "candidate {candidate_text}candidate {candidate_text}later {candidate_text}"
                    )
                );
                assert!(
                    local
                        .success(&["status", "--porcelain=v1"])
                        .stdout
                        .is_empty()
                );
                // Squash intentionally omits source ancestry; source objects remain
                // readable and its content, rather than duplicate commits, is delivered.
                assert_eq!(
                    twigsmith::git::run(
                        local.path(),
                        [
                            "merge-base",
                            "--is-ancestor",
                            String::from_utf8(first_source).unwrap().trim(),
                            "main"
                        ]
                    )
                    .unwrap()
                    .status
                    .code(),
                    Some(1)
                );
                assert_eq!(oid(&remote, "main^"), target);
                println!(
                    "{starter} source -> {finisher} direct: retained candidate {}, checked twice at same OID; exact tree delivered once; main checked out and source branch cleaned",
                    candidate_text.trim()
                );
            }
        },
    );
}
