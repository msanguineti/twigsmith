#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

fn launcher() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("dev"), include_str!("../dev")).unwrap();
    fs::write(
        root.path().join("rust-toolchain.toml"),
        include_str!("../rust-toolchain.toml"),
    )
    .unwrap();
    let bin = root.path().join(".dev/cargo/bin");
    fs::create_dir_all(&bin).unwrap();
    for (name, body) in [
        ("rustup", "#!/bin/sh\nexit 0\n"),
        ("cargo", "#!/bin/sh\nprintf 'ARG:%s\\n' \"$@\"\n"),
    ] {
        let path = bin.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    root
}

fn run(root: &tempfile::TempDir, args: &[&str]) -> std::process::Output {
    Command::new("sh")
        .arg(root.path().join("dev"))
        .args(args)
        .output()
        .unwrap()
}

fn cargo_args(output: &std::process::Output) -> Vec<String> {
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| line.strip_prefix("ARG:").unwrap().to_owned())
        .collect()
}

#[test]
fn cargo_filters_selectors_and_harness_arguments_are_preserved() {
    let root = launcher();
    let forwarded = [
        "config",
        "--test",
        "settings",
        "--test",
        "finish_checks",
        "--",
        "--nocapture",
        "--skip",
        "literal filter with spaces",
    ];
    let mut command = vec!["test"];
    command.extend(forwarded);
    let mut expected = vec!["test", "--locked"];
    expected.extend(forwarded);
    assert_eq!(cargo_args(&run(&root, &command)), expected);
}

#[test]
fn discovered_groups_execute_the_advertised_existing_targets() {
    let root = launcher();
    let output = run(&root, &["test-groups"]);
    assert!(output.status.success());
    let listing = String::from_utf8(output.stdout).unwrap();
    let mut lines = listing.lines();
    assert_eq!(
        lines.next().unwrap(),
        "Every group includes --lib --bins and the following --test targets:"
    );
    let mut groups = 0;
    while let Some(line) = lines.next() {
        if line.is_empty() {
            continue;
        }
        let (name, _) = line.split_once(':').unwrap();
        let targets: Vec<_> = lines.next().unwrap().split_whitespace().collect();
        assert!(!targets.is_empty());
        for target in &targets {
            assert!(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/{target}.rs"))
                    .is_file(),
                "{name} advertises missing target {target}"
            );
        }
        let args = cargo_args(&run(
            &root,
            &[
                "test-group",
                name,
                "literal filter with spaces",
                "--test",
                "cli_smoke",
                "--",
                "--nocapture",
            ],
        ));
        assert_eq!(&args[..4], ["test", "--locked", "--lib", "--bins"]);
        let end = 4 + targets.len() * 2;
        let mut selected = Vec::new();
        for pair in args[4..end].as_chunks::<2>().0 {
            assert_eq!(pair[0], "--test");
            selected.push(pair[1].as_str());
        }
        selected.sort_unstable();
        let mut advertised = targets;
        advertised.sort_unstable();
        assert_eq!(selected, advertised);
        assert_eq!(
            &args[end..],
            [
                "literal filter with spaces",
                "--test",
                "cli_smoke",
                "--",
                "--nocapture"
            ]
        );
        groups += 1;
    }
    assert!(groups > 0);
}

#[test]
fn invalid_group_requests_fail_before_invoking_cargo() {
    let root = launcher();
    for args in [
        vec!["test-group", "unknown"],
        vec!["test-group"],
        vec!["test-groups", "unexpected"],
    ] {
        let output = run(&root, &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("error:"));
        assert!(error.contains("test-groups"));
    }
    // Discovery and invalid selection work before bootstrap, too.
    fs::remove_dir_all(root.path().join(".dev")).unwrap();
    assert!(run(&root, &["test-groups"]).status.success());
    assert_eq!(
        run(&root, &["test-group", "unknown"]).status.code(),
        Some(2)
    );
}

#[test]
fn complete_runs_use_cargo_discovery_without_group_selectors() {
    let root = launcher();
    assert_eq!(cargo_args(&run(&root, &["test"])), ["test", "--locked"]);
    assert_eq!(
        cargo_args(&run(&root, &["check"])),
        [
            "fmt",
            "--all",
            "--check",
            "build",
            "--locked",
            "clippy",
            "--locked",
            "--all-targets",
            "--",
            "-D",
            "warnings",
            "test",
            "--locked"
        ]
    );
}
