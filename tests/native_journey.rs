//! A deliberately portable application slice, executed natively by the CI matrix.
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
    let output = Command::new(executable)
        .current_dir(repo.path())
        .args(args)
        .output()
        .unwrap();
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
