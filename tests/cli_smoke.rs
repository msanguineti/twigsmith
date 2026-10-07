use std::{path::Path, process::Command};

#[test]
fn cli_runs_outside_git_without_node_or_rust_tools() {
    // The development entry point currently targets Linux. The filesystem root
    // avoids Git's discovery of the repository containing the build directory.
    assert!(
        !Path::new("/.git").exists(),
        "smoke checks require / outside Git"
    );

    for args in [
        vec![],
        vec!["--help"],
        vec!["-h"],
        vec!["--version"],
        vec!["-V"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
            .args(&args)
            .current_dir("/")
            .env_clear()
            .env("PATH", "")
            .output()
            .expect("native executable should run with an empty PATH");

        assert!(output.status.success(), "arguments: {args:?}");
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 CLI output");
        if args == ["--version"] || args == ["-V"] {
            assert_eq!(
                stdout,
                concat!("git-oneflow ", env!("CARGO_PKG_VERSION"), "\n")
            );
        } else {
            assert!(stdout.contains("Usage: git-oneflow [OPTIONS]"));
            assert!(stdout.contains("--help"));
            assert!(stdout.contains("--version"));
        }
    }

    for args in [["start"].as_slice(), ["--help", "start"].as_slice()] {
        let output = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
            .args(args)
            .current_dir("/")
            .env_clear()
            .env("PATH", "")
            .output()
            .expect("native executable should run with an empty PATH");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("use --help"));
    }
}
