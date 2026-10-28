mod support;

use std::{fs, io};

use support::{Repository, isolated};
use twigsmith::git;

#[test]
fn working_directory_and_literal_arguments() {
    isolated("working_directory_and_literal_arguments", true, |root| {
        let first = Repository::new(root);
        let second = Repository::new(root);
        for repository in [&first, &second] {
            let output = repository.success(&["rev-parse", "--show-toplevel"]);
            let expected = fs::canonicalize(repository.path()).unwrap();
            assert_eq!(
                output.stdout,
                format!("{}\n", expected.to_str().unwrap()).as_bytes()
            );
            assert_eq!(output.status.code(), Some(0));
            assert!(output.stderr.is_empty());
        }

        let literal = "spaces 'single' \"double\" ; $(touch shell-was-run) & | $HOME";
        let option = format!("test.literal={literal}");
        let output = first.success(&["-c", &option, "config", "--get", "test.literal"]);
        assert_eq!(output.stdout, format!("{literal}\n").as_bytes());
        assert!(!first.path().join("shell-was-run").exists());

        let filename = "file 'quoted' ;$ [literal].txt";
        fs::write(first.path().join(filename), "fixture content\n").unwrap();
        first.success(&["add", "--", filename]);
        first.success(&["commit", "-m", literal]);
        assert_eq!(
            first.success(&["log", "-1", "--format=%B"]).stdout,
            format!("{literal}\n\n").as_bytes()
        );
        assert_eq!(
            first.success(&["ls-files", "-z"]).stdout,
            format!("{filename}\0").as_bytes()
        );

        // Arguments and output must not require valid UTF-8.
        #[cfg(unix)]
        {
            use std::{ffi::OsString, os::unix::ffi::OsStringExt};
            let option = OsString::from_vec(b"test.bytes=raw\xff".to_vec());
            let output = first.success(&[
                OsString::from("-c"),
                option,
                OsString::from("config"),
                OsString::from("--get"),
                OsString::from("test.bytes"),
            ]);
            assert_eq!(output.stdout, b"raw\xff\n");
        }
    });
}

#[test]
fn unsuccessful_git_preserves_status_and_output() {
    isolated(
        "unsuccessful_git_preserves_status_and_output",
        true,
        |root| {
            let repository = Repository::new(root);
            let output = git::run(repository.path(), ["rev-parse", "--verify", "HEAD"])
                .expect("Git command failure is a completed process");
            assert!(!output.status.success());
            assert_eq!(output.status.code(), Some(128));
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("fatal:"));

            // rev-parse writes an earlier answer before failing on a later argument.
            let output = git::run(
                repository.path(),
                ["rev-parse", "--is-inside-work-tree", "HEAD"],
            )
            .unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.starts_with(b"true\n"));
            assert!(!output.stderr.is_empty());
        },
    );
}

#[test]
fn invalid_working_directory_is_an_execution_error() {
    isolated(
        "invalid_working_directory_is_an_execution_error",
        true,
        |root| {
            let error = git::run(root.join("does-not-exist"), ["--version"]).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::NotFound);
            assert!(error.raw_os_error().is_some());
        },
    );
}

#[test]
fn missing_git_is_an_execution_error() {
    isolated("missing_git_is_an_execution_error", false, |root| {
        let error = git::run(root, ["--version"]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.raw_os_error().is_some());
    });
}
