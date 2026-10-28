//! Registration fixtures always pass an isolated HOME/profile to the executable.
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;

use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};

fn command(executable: &str, root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .env("ZDOTDIR", root.join("zsh"))
        .env("TERM", "dumb")
        .env("NO_COLOR", "1");
    command
}
fn run(executable: &str, root: &Path, args: &[&str]) -> Output {
    command(executable, root, args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn check(output: &Output, code: i32) {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
}
fn executables() -> [&'static str; 2] {
    [env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")]
}

#[test]
fn session_generation_and_installed_help_work_outside_repository() {
    let root = tempfile::tempdir().unwrap();
    for executable in executables() {
        for shell in ["bash", "zsh", "fish", "powershell"] {
            let output = run(executable, root.path(), &["completion", shell]);
            check(&output, 0);
            assert!(!output.stdout.is_empty());
            assert!(output.stderr.is_empty(), "{output:?}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("registered"));
        }
        let output = run(executable, root.path(), &["completion", "--help"]);
        check(&output, 0);
        let help = String::from_utf8(output.stdout).unwrap();
        for value in [
            "Usage:",
            "Arguments:",
            "Options",
            "Examples",
            "Exit status:",
            "--file",
            "$PROFILE",
            "ZDOTDIR",
            "XDG_CONFIG_HOME",
            "PATH",
            "compinit",
            "Bash login",
        ] {
            assert!(help.contains(value), "missing {value}: {help}");
        }
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn explicit_shell_and_unattended_authorization_are_required() {
    let root = tempfile::tempdir().unwrap();
    for executable in executables() {
        for args in [
            vec!["completion"],
            vec!["completion", "install"],
            vec!["completion", "install", "bash", "--file"],
            vec!["completion", "install", "powershell"],
            vec!["completion", "bash", "--yes"],
            vec!["completion", "install", "bash", "--yes", "--yes"],
        ] {
            check(&run(executable, root.path(), &args), 2);
        }
        let output = run(executable, root.path(), &["completion", "install", "bash"]);
        check(&output, 1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Exact block") && stderr.contains("--yes"),
            "{stderr}"
        );
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn repeated_install_and_uninstall_preserve_unrelated_bytes_and_permissions() {
    for executable in executables() {
        for shell in ["bash", "zsh", "fish", "powershell"] {
            let root = tempfile::tempdir().unwrap();
            let profile = root.path().join("custom profile");
            let path = profile.to_str().unwrap();
            let original = b"# unrelated\nexport CUSTOM=value\n# byte \xff without final newline";
            fs::write(&profile, original).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&profile, fs::Permissions::from_mode(0o640)).unwrap();
            }
            let args = ["completion", "install", shell, "--file", path, "--yes"];
            check(&run(executable, root.path(), &args), 0);
            let installed = fs::read(&profile).unwrap();
            assert!(installed.starts_with(original));
            assert!(String::from_utf8_lossy(&installed).contains("completion "));
            check(&run(executable, root.path(), &args), 0);
            assert_eq!(fs::read(&profile).unwrap(), installed);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
                    0o640
                );
            }
            // Subsequent unrelated edits stay present while the owned block is removed.
            let mut edited = installed;
            edited.extend_from_slice(b"# later unrelated content\n");
            fs::write(&profile, &edited).unwrap();
            check(
                &run(
                    executable,
                    root.path(),
                    &["completion", "uninstall", shell, "--file", path, "--yes"],
                ),
                0,
            );
            let mut expected = original.to_vec();
            expected.extend_from_slice(b"# later unrelated content\n");
            assert_eq!(fs::read(&profile).unwrap(), expected);
            check(
                &run(
                    executable,
                    root.path(),
                    &["completion", "uninstall", shell, "--file", path, "--yes"],
                ),
                0,
            );
            assert_eq!(fs::read(&profile).unwrap(), expected);
        }
    }
}

// Linux POSIX ACL xattr ABI: version 2 followed by little-endian tag,
// permission and ID entries (linux/posix_acl_xattr.h). A named user makes this
// an extended ACL rather than one reducible to ordinary permission bits.
#[cfg(target_os = "linux")]
fn access_acl() -> Vec<u8> {
    let mut acl = 2_u32.to_le_bytes().to_vec();
    for (tag, permissions, id) in [
        (1_u16, 6_u16, u32::MAX),
        (2, 4, 65534),
        (4, 0, u32::MAX),
        (16, 4, u32::MAX),
        (32, 0, u32::MAX),
    ] {
        acl.extend(tag.to_le_bytes());
        acl.extend(permissions.to_le_bytes());
        acl.extend(id.to_le_bytes());
    }
    acl
}

#[cfg(target_os = "linux")]
fn metadata_state(path: &Path) -> (u64, u32, u32, u32) {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path).unwrap();
    (
        metadata.ino(),
        metadata.uid(),
        metadata.gid(),
        metadata.mode(),
    )
}

#[cfg(target_os = "linux")]
#[test]
fn extended_profile_metadata_refuses_install_and_uninstall_without_changes() {
    use rustix::fs::{XattrFlags, getxattr, setxattr};
    for (attribute, value) in [
        ("user.oneflow-test", b"unrelated metadata".to_vec()),
        ("system.posix_acl_access", access_acl()),
    ] {
        for action in ["install", "uninstall"] {
            let root = tempfile::tempdir().unwrap();
            let profile = root.path().join("profile");
            fs::write(&profile, b"# unrelated content\n").unwrap();
            if action == "uninstall" {
                check(
                    &run(
                        executables()[0],
                        root.path(),
                        &[
                            "completion",
                            "install",
                            "bash",
                            "--file",
                            profile.to_str().unwrap(),
                            "--yes",
                        ],
                    ),
                    0,
                );
            }
            setxattr(&profile, attribute, &value, XattrFlags::empty()).unwrap();
            let before = metadata_state(&profile);
            let bytes = fs::read(&profile).unwrap();
            // Matching install / already-absent uninstall performs no replacement.
            check(
                &run(
                    executables()[0],
                    root.path(),
                    &[
                        "completion",
                        if action == "install" {
                            "uninstall"
                        } else {
                            "install"
                        },
                        "bash",
                        "--file",
                        profile.to_str().unwrap(),
                        "--yes",
                    ],
                ),
                0,
            );
            let output = run(
                executables()[0],
                root.path(),
                &[
                    "completion",
                    action,
                    "bash",
                    "--file",
                    profile.to_str().unwrap(),
                    "--yes",
                ],
            );
            check(&output, 1);
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("extended metadata"),
                "{output:?}"
            );
            assert_eq!(fs::read(&profile).unwrap(), bytes);
            assert_eq!(metadata_state(&profile), before);
            let mut actual = [0_u8; 256];
            let length = getxattr(&profile, attribute, &mut actual).unwrap();
            assert_eq!(&actual[..length], value);
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn inherited_acl_refuses_new_and_existing_profile_replacement() {
    use rustix::fs::{XattrFlags, getxattr, setxattr};
    for action in ["new", "install", "uninstall"] {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        if action != "new" {
            fs::write(&profile, b"# unrelated content\n").unwrap();
        }
        if action == "uninstall" {
            check(
                &run(
                    executables()[0],
                    root.path(),
                    &[
                        "completion",
                        "install",
                        "bash",
                        "--file",
                        profile.to_str().unwrap(),
                        "--yes",
                    ],
                ),
                0,
            );
        }
        let bytes = fs::read(&profile).ok();
        let before = bytes.as_ref().map(|_| metadata_state(&profile));
        let acl = access_acl();
        setxattr(
            root.path(),
            "system.posix_acl_default",
            &acl,
            XattrFlags::empty(),
        )
        .unwrap();
        let output = run(
            executables()[0],
            root.path(),
            &[
                "completion",
                if action == "uninstall" {
                    action
                } else {
                    "install"
                },
                "bash",
                "--file",
                profile.to_str().unwrap(),
                "--yes",
            ],
        );
        check(&output, 1);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("extended metadata"),
            "{output:?}"
        );
        assert_eq!(fs::read(&profile).ok(), bytes);
        assert_eq!(profile.exists().then(|| metadata_state(&profile)), before);
        let mut actual = [0_u8; 256];
        let length = getxattr(root.path(), "system.posix_acl_default", &mut actual).unwrap();
        assert_eq!(&actual[..length], acl);
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            usize::from(action != "new")
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn registration_preserves_profile_owner_and_nondefault_group() {
    use rustix::{
        fs::fchown,
        process::{getegid, getgroups},
    };
    use std::os::unix::fs::PermissionsExt;
    let Some(group) = getgroups()
        .unwrap()
        .into_iter()
        .find(|group| *group != getegid())
    else {
        eprintln!("SKIP nondefault profile group: no supplementary group available");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile");
    fs::write(&profile, b"# unrelated content\n").unwrap();
    fchown(fs::File::open(&profile).unwrap(), None, Some(group)).unwrap();
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o640)).unwrap();
    let (_, uid, gid, mode) = metadata_state(&profile);
    for action in ["install", "uninstall"] {
        check(
            &run(
                executables()[0],
                root.path(),
                &[
                    "completion",
                    action,
                    "bash",
                    "--file",
                    profile.to_str().unwrap(),
                    "--yes",
                ],
            ),
            0,
        );
        let (_, current_uid, current_gid, current_mode) = metadata_state(&profile);
        assert_eq!((current_uid, current_gid, current_mode), (uid, gid, mode));
    }
    assert_eq!(fs::read(profile).unwrap(), b"# unrelated content\n");
}

#[cfg(target_os = "linux")]
#[test]
fn metadata_changes_during_approval_refuse_replacement() {
    use rustix::{
        fs::{XattrFlags, fchown, setxattr},
        process::{getegid, getgroups},
    };
    for change in ["xattr", "group"] {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        fs::write(&profile, b"# original\n").unwrap();
        let group = getgroups()
            .unwrap()
            .into_iter()
            .find(|group| *group != getegid());
        if change == "group" && group.is_none() {
            eprintln!("SKIP approval group change: no supplementary group available");
            continue;
        }
        let mut term = terminal::Terminal::start_command(command(
            executables()[0],
            root.path(),
            &[
                "completion",
                "install",
                "bash",
                "--file",
                profile.to_str().unwrap(),
            ],
        ));
        term.until("Apply completion registration?");
        if change == "xattr" {
            setxattr(
                &profile,
                "user.oneflow-test",
                b"concurrent",
                XattrFlags::empty(),
            )
            .unwrap();
        } else {
            fchown(fs::File::open(&profile).unwrap(), None, group).unwrap();
        }
        let before = metadata_state(&profile);
        term.send(b"install\n");
        assert_eq!(term.finish().code(), Some(1), "{}", term.output);
        assert!(
            term.output.contains("changed after review"),
            "{}",
            term.output
        );
        assert_eq!(fs::read(&profile).unwrap(), b"# original\n");
        assert_eq!(metadata_state(&profile), before);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn defaults_custom_fish_directory_and_explicit_powershell_profile() {
    for executable in executables() {
        let root = tempfile::tempdir().unwrap();
        for (shell, relative) in [
            ("bash", ".bashrc"),
            ("zsh", "zsh/.zshrc"),
            ("fish", "xdg/fish/conf.d/twigsmith.fish"),
        ] {
            check(
                &run(
                    executable,
                    root.path(),
                    &["completion", "install", shell, "--yes"],
                ),
                0,
            );
            assert!(root.path().join(relative).is_file());
        }
        let config = root.path().join("fish-custom");
        fs::create_dir(&config).unwrap();
        check(
            &run(
                executable,
                root.path(),
                &[
                    "completion",
                    "install",
                    "fish",
                    "--file",
                    config.to_str().unwrap(),
                    "--yes",
                ],
            ),
            0,
        );
        assert!(config.join("conf.d/twigsmith.fish").is_file());
        let profile = root.path().join("custom-profile.ps1");
        check(
            &run(
                executable,
                root.path(),
                &[
                    "completion",
                    "install",
                    "powershell",
                    "--file",
                    profile.to_str().unwrap(),
                    "--yes",
                ],
            ),
            0,
        );
        assert!(profile.is_file());
    }
}

#[test]
fn legacy_registrations_are_preserved_across_install_and_uninstall() {
    for executable in executables() {
        for shell in ["bash", "zsh", "fish", "powershell"] {
            let root = tempfile::tempdir().unwrap();
            let profile = root.path().join("profile");
            let path = profile.to_str().unwrap();
            let legacy = format!(
                "# unrelated\n# >>> git-oneflow completion {shell} >>>\nlegacy loader\n# <<< git-oneflow completion {shell} <<<\n"
            );
            fs::write(&profile, &legacy).unwrap();
            for action in ["install", "install", "uninstall", "uninstall"] {
                check(
                    &run(
                        executable,
                        root.path(),
                        &["completion", action, shell, "--file", path, "--yes"],
                    ),
                    0,
                );
                assert!(fs::read(&profile).unwrap().starts_with(legacy.as_bytes()));
            }
            assert_eq!(fs::read(&profile).unwrap(), legacy.as_bytes());
        }
    }
}

#[test]
fn custom_shell_locations_work_without_home() {
    let root = tempfile::tempdir().unwrap();
    for shell in ["zsh", "fish"] {
        let mut invocation = command(
            executables()[0],
            root.path(),
            &["completion", "install", shell, "--yes"],
        );
        invocation.env_remove("HOME");
        check(&invocation.output().unwrap(), 0);
    }
}

#[test]
fn edited_duplicated_and_malformed_blocks_are_never_replaced_or_removed() {
    for executable in executables() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        let path = profile.to_str().unwrap();
        check(
            &run(
                executable,
                root.path(),
                &["completion", "install", "bash", "--file", path, "--yes"],
            ),
            0,
        );
        let installed = fs::read(&profile).unwrap();
        let edited = String::from_utf8(installed.clone())
            .unwrap()
            .replace("completion bash", "completion zsh")
            .into_bytes();
        let duplicate = [installed.as_slice(), installed.as_slice()].concat();
        for bytes in [
            edited,
            duplicate,
            b"# >>> twigsmith completion bash >>>\n# unfinished\n".to_vec(),
            b"# <<< twigsmith completion bash <<<\n".to_vec(),
        ] {
            fs::write(&profile, &bytes).unwrap();
            for action in ["install", "uninstall"] {
                let output = run(
                    executable,
                    root.path(),
                    &["completion", action, "bash", "--file", path, "--yes"],
                );
                check(&output, 1);
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("manual"),
                    "{output:?}"
                );
                assert_eq!(fs::read(&profile).unwrap(), bytes);
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn symlinks_and_unusable_parent_fail_without_touching_profile() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::write(&target, b"# intact\n").unwrap();
    let link = root.path().join("link");
    symlink(&target, &link).unwrap();
    for executable in executables() {
        let output = run(
            executable,
            root.path(),
            &[
                "completion",
                "install",
                "bash",
                "--file",
                link.to_str().unwrap(),
                "--yes",
            ],
        );
        check(&output, 1);
        assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
        assert_eq!(fs::read(&target).unwrap(), b"# intact\n");
        let blocked = target.join("profile");
        check(
            &run(
                executable,
                root.path(),
                &[
                    "completion",
                    "install",
                    "bash",
                    "--file",
                    blocked.to_str().unwrap(),
                    "--yes",
                ],
            ),
            1,
        );
        assert_eq!(fs::read(&target).unwrap(), b"# intact\n");
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn cancellation_and_review_revalidation_preserve_isolated_profiles() {
    for executable in executables() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("profile");
        let profile = path.to_str().unwrap();
        fs::write(&path, b"# original\n").unwrap();
        for answer in [b"cancel\n".as_slice(), b"\x04".as_slice()] {
            let mut term = terminal::Terminal::start_command(command(
                executable,
                root.path(),
                &["completion", "install", "bash", "--file", profile],
            ));
            term.until("Apply completion registration?");
            assert!(term.output.contains("Exact block") && term.output.contains(profile));
            term.send(answer);
            assert!(term.finish().success(), "{}", term.output);
            assert_eq!(fs::read(&path).unwrap(), b"# original\n");
        }
        let mut term = terminal::Terminal::start_command(command(
            executable,
            root.path(),
            &["completion", "install", "bash", "--file", profile],
        ));
        term.until("Apply completion registration?");
        fs::write(&path, b"# concurrently edited\n").unwrap();
        term.send(b"install\n");
        assert_eq!(term.finish().code(), Some(1), "{}", term.output);
        assert!(term.output.contains("changed after review"));
        assert_eq!(fs::read(&path).unwrap(), b"# concurrently edited\n");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[cfg(unix)]
#[test]
fn fresh_bash_loads_registration_through_path_and_tracks_executable_changes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    // Only the short executable name is on PATH, exercising startup fallback.
    let executable = bin.join("tgi");
    symlink(executables()[1], &executable).unwrap();
    check(
        &run(
            executables()[0],
            root.path(),
            &["completion", "install", "bash", "--yes"],
        ),
        0,
    );
    let profile = root.path().join(".bashrc");
    let registered = fs::read(&profile).unwrap();
    let shell = || {
        let mut invocation = Command::new("/bin/bash");
        invocation
            .args([
                "--noprofile",
                "--rcfile",
                profile.to_str().unwrap(),
                "-i",
                "-c",
                "complete -p twigsmith tgi",
            ])
            .env("HOME", root.path())
            .env("PATH", &bin)
            .stdin(Stdio::null());
        invocation.output().unwrap()
    };
    let output = shell();
    check(&output, 0);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("twigsmith") && stdout.contains("tgi"),
        "{output:?}"
    );
    // Startup reads the executable afresh, rather than a pinned generated script
    // or original installation path. The replacement fixture registers a marker.
    fs::remove_file(&executable).unwrap();
    fs::write(
        &executable,
        b"#!/bin/bash\nprintf '%s\\n' 'complete -W upgraded twigsmith tgi'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let output = shell();
    check(&output, 0);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("upgraded"),
        "{output:?}"
    );
    assert_eq!(fs::read(&profile).unwrap(), registered);
    check(
        &run(
            executables()[0],
            root.path(),
            &["completion", "uninstall", "bash", "--yes"],
        ),
        0,
    );
    assert!(fs::read(&profile).unwrap().is_empty());
    check(&shell(), 1);
}

#[cfg(unix)]
#[test]
fn failed_atomic_staging_leaves_existing_profile_and_directory_intact() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("readonly");
    fs::create_dir(&directory).unwrap();
    let profile = directory.join("profile");
    fs::write(&profile, b"# unchanged\n").unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500)).unwrap();
    // Root can write despite mode restrictions; exercise this failure when the
    // runtime enforces ordinary user directory permissions.
    let probe = directory.join("probe");
    if fs::write(&probe, b"").is_ok() {
        fs::remove_file(&probe).unwrap();
    } else {
        let output = run(
            executables()[0],
            root.path(),
            &[
                "completion",
                "install",
                "bash",
                "--file",
                profile.to_str().unwrap(),
                "--yes",
            ],
        );
        check(&output, 1);
        assert_eq!(fs::read(&profile).unwrap(), b"# unchanged\n");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
}

#[allow(dead_code)]
mod support;

fn runtime(name: &str) -> Option<std::path::PathBuf> {
    let variable = format!("TWIGSMITH_TEST_{}", name.to_uppercase());
    if let Some(path) = std::env::var_os(variable) {
        return Some(path.into());
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}

fn completion_path() -> std::ffi::OsString {
    let mut paths = vec![Path::new(executables()[0]).parent().unwrap().to_owned()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

fn registration_fixture(root: &Path) -> support::Repository {
    let repository = support::Repository::new(root);
    repository.success(&["commit", "--allow-empty", "-m", "initial"]);
    repository.success(&["branch", "feat/first"]);
    repository
}

fn native_cases(output: &Output, powershell: bool) {
    check(output, 0);
    assert!(output.stderr.is_empty(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    let cases: Vec<_> = text.split("CASE\n").skip(1).collect();
    assert_eq!(cases.len(), 4, "{text}");
    for (case, expected) in cases
        .iter()
        .zip(["start", "integrate", "feat/first", "feat/second"])
    {
        let actual: Vec<_> = case
            .lines()
            .map(|line| line.split('\t').next().unwrap())
            .collect();
        let expected = if powershell {
            format!("'{expected}'")
        } else {
            expected.into()
        };
        assert_eq!(actual, [expected.as_str()], "{text}");
    }
}

#[cfg(unix)]
#[test]
#[ignore = "requires fish; run explicitly on a provisioned machine"]
fn fresh_fish_reads_persistent_registration_and_live_refs() {
    let shell = runtime("fish").expect("required fish runtime is unavailable");
    support::isolated(
        "fresh_fish_reads_persistent_registration_and_live_refs",
        true,
        |root| {
            let repository = registration_fixture(root);
            let shell = relocated_fish(root, &shell);
            check(
                &run(
                    executables()[0],
                    root,
                    &["completion", "install", "fish", "--yes"],
                ),
                0,
            );
            let output = command(
                shell.to_str().unwrap(),
                root,
                &[
                    "-c",
                    r#"
for line in 'tgi star' 'twigsmith finish --mode int' 'tgi s feat/'
    echo CASE
    complete -C "$line"
end
command git branch feat/second
echo CASE
complete -C 'tgi s feat/sec'
"#,
                ],
            )
            .current_dir(repository.path())
            .env("PATH", completion_path())
            .output()
            .unwrap();
            native_cases(&output, false);
        },
    );
}

#[test]
#[ignore = "requires pwsh; run explicitly on a provisioned machine"]
fn fresh_powershell_reads_selected_profile_and_live_refs() {
    let shell = runtime("pwsh").expect("required pwsh runtime is unavailable");
    support::isolated(
        "fresh_powershell_reads_selected_profile_and_live_refs",
        true,
        |root| {
            let repository = registration_fixture(root);
            let profile = root.join("custom profile.ps1");
            check(
                &run(
                    executables()[0],
                    root,
                    &[
                        "completion",
                        "install",
                        "powershell",
                        "--file",
                        profile.to_str().unwrap(),
                        "--yes",
                    ],
                ),
                0,
            );
            let test = root.join("registration-test.ps1");
            fs::write(
                &test,
                r#"
param($ProfilePath)
. $ProfilePath
foreach ($line in @('tgi star', 'twigsmith finish --mode int', 'tgi s feat/')) {
    Write-Output 'CASE'
    (TabExpansion2 $line $line.Length).CompletionMatches | ForEach-Object { $_.CompletionText }
}
git branch feat/second
Write-Output 'CASE'
$line = 'tgi s feat/sec'
(TabExpansion2 $line $line.Length).CompletionMatches | ForEach-Object { $_.CompletionText }
"#,
            )
            .unwrap();
            let output = command(
                shell.to_str().unwrap(),
                root,
                &[
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-File",
                    test.to_str().unwrap(),
                    profile.to_str().unwrap(),
                ],
            )
            .current_dir(repository.path())
            .env("PATH", completion_path())
            .output()
            .unwrap();
            native_cases(&output, true);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires zsh; run explicitly on a provisioned machine"]
fn fresh_zsh_reads_persistent_registration_and_live_refs() {
    let shell = runtime("zsh").expect("required zsh runtime is unavailable");
    support::isolated(
        "fresh_zsh_reads_persistent_registration_and_live_refs",
        true,
        |root| {
            let repository = registration_fixture(root);
            let zshdir = root.join("zsh");
            fs::create_dir(&zshdir).unwrap();
            let profile = zshdir.join(".zshrc");
            let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
            let mut init = String::new();
            if let Some(path) = std::env::var_os("TWIGSMITH_TEST_ZSH_MODULE_PATH") {
                init.push_str(&format!(
                    "module_path=({} $module_path)\n",
                    quote(path.to_str().unwrap())
                ));
            }
            if let Some(path) = std::env::var_os("TWIGSMITH_TEST_ZSH_FUNCTION_PATH") {
                init.push_str(&format!(
                    "fpath=({}/**/*(/) $fpath)\n",
                    quote(path.to_str().unwrap())
                ));
            }
            init.push_str("autoload -Uz compinit\ncompinit -D\n");
            fs::write(&profile, init).unwrap();
            check(
                &run(
                    executables()[0],
                    root,
                    &["completion", "install", "zsh", "--yes"],
                ),
                0,
            );
            let capture = root.join("arguments");
            let mut contents = fs::read(&profile).unwrap();
            contents.extend_from_slice(format!("\nPS1='REGISTRATION_READY> '\ncount=0\ntgi() {{ printf '%s\\0' \"$@\" > {}; ((count+=1)); printf 'DONE:%s\\n' \"$count\"; }}\ntwigsmith() {{ tgi \"$@\"; }}\n", quote(capture.to_str().unwrap())).as_bytes());
            fs::write(&profile, contents).unwrap();
            let mut invocation = command(shell.to_str().unwrap(), root, &["-d", "-i"]);
            invocation
                .current_dir(repository.path())
                .env("PATH", completion_path())
                .env("TERM", "xterm-256color");
            let mut terminal = terminal::Terminal::start_command(invocation);
            terminal.until("REGISTRATION_READY> ");
            for (index, (line, expected)) in [
                ("tgi star", "start\0"),
                ("twigsmith finish --mode int", "finish\0--mode\0integrate\0"),
                ("tgi s feat/fir", "s\0feat/first\0"),
                ("tgi s feat/sec", "s\0feat/second\0"),
            ]
            .iter()
            .enumerate()
            {
                if index == 3 {
                    repository.success(&["branch", "feat/second"]);
                }
                terminal.send(format!("{line}\t\n").as_bytes());
                terminal.until(&format!("DONE:{}", index + 1));
                assert_eq!(
                    fs::read(&capture).unwrap(),
                    expected.as_bytes(),
                    "{}",
                    terminal.output
                );
            }
            terminal.send(b"exit\n");
            assert!(terminal.finish().success(), "{}", terminal.output);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn interactive_install_applies_only_submitted_install_choice() {
    let root = tempfile::tempdir().unwrap();
    for executable in executables() {
        let profile = root.path().join("profile");
        fs::write(&profile, b"# original\n").unwrap();
        let mut term = terminal::Terminal::start_command(command(
            executable,
            root.path(),
            &[
                "completion",
                "install",
                "bash",
                "--file",
                profile.to_str().unwrap(),
            ],
        ));
        term.until("Apply completion registration?");
        term.send(b"install\n");
        assert!(term.finish().success(), "{}", term.output);
        assert!(
            fs::read_to_string(&profile)
                .unwrap()
                .contains("# >>> twigsmith completion bash >>>")
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[cfg(unix)]
fn relocated_fish(root: &Path, shell: &Path) -> std::path::PathBuf {
    // Extracted distro packages split /usr/share and /etc. Fish requires both
    // beneath its runtime prefix for relocation; provide that tree in the fixture
    // so startup reads ordinary configuration rather than a manual source call.
    let prefix = shell.parent().and_then(Path::parent).unwrap();
    let data = prefix.join("share/fish");
    if data.join("config.fish").is_file() && !prefix.join("etc/fish").is_dir() {
        let fixture = root.join("fish-runtime");
        fs::create_dir_all(fixture.join("bin")).unwrap();
        fs::create_dir_all(fixture.join("etc/fish")).unwrap();
        fs::create_dir_all(fixture.join("share")).unwrap();
        std::os::unix::fs::symlink(data, fixture.join("share/fish")).unwrap();
        fs::copy(shell, fixture.join("bin/fish")).unwrap();
        fixture.join("bin/fish")
    } else {
        shell.to_owned()
    }
}

#[test]
fn encoded_profiles_are_rejected_without_changes_and_utf8_bom_is_preserved() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile.ps1");
    for original in [
        b"\xff\xfe#\0 \0".as_slice(),
        b"\xfe\xff\0#\0 ".as_slice(),
        b"\0\0\xfe\xff\0\0\0#".as_slice(),
        b"# NUL\0 profile".as_slice(),
    ] {
        fs::write(&profile, original).unwrap();
        for action in ["install", "uninstall"] {
            let output = run(
                executables()[0],
                root.path(),
                &[
                    "completion",
                    action,
                    "powershell",
                    "--file",
                    profile.to_str().unwrap(),
                    "--yes",
                ],
            );
            check(&output, 1);
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("UTF-8"),
                "{output:?}"
            );
            assert_eq!(fs::read(&profile).unwrap(), original);
        }
    }
    let original = b"\xef\xbb\xbf# UTF-8 profile\n";
    fs::write(&profile, original).unwrap();
    for action in ["install", "uninstall"] {
        check(
            &run(
                executables()[0],
                root.path(),
                &[
                    "completion",
                    action,
                    "powershell",
                    "--file",
                    profile.to_str().unwrap(),
                    "--yes",
                ],
            ),
            0,
        );
        assert!(fs::read(&profile).unwrap().starts_with(original));
    }
    assert_eq!(fs::read(&profile).unwrap(), original);
}
