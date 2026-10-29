use std::process::Command;

#[test]
fn native_archive_format_integrity_and_rejection_contracts() {
    let result = Command::new("python3")
        .args([
            "-m",
            "unittest",
            "discover",
            "-s",
            "tools",
            "-p",
            "test_*native*.py",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("native package checks require Python 3.10 or newer");
    assert!(
        result.status.success(),
        "native packaging checks failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
