#![cfg(unix)]

#[path = "support/snapshot.rs"]
mod preservation;
mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;

use std::{fs, path::Path, process::Command};
use support::{Repository, isolated};

fn artifact() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/completions/git-oneflow.bash")
}

fn complete(directory: &Path, line: &str) -> Vec<String> {
    let executable = line.split_whitespace().next().unwrap();
    let output = Command::new("bash")
        .args([
            "--noprofile",
            "--norc",
            "-c",
            r#"source "$1"
COMP_LINE=$2; COMP_POINT=${#COMP_LINE}
_git_oneflow "$3"
if ((${#COMPREPLY[@]})); then printf '%s\0' "${COMPREPLY[@]}"; fi"#,
            "completion-test",
            artifact(),
            line,
            executable,
        ])
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|word| !word.is_empty())
        .map(|word| String::from_utf8(word.to_vec()).unwrap())
        .collect()
}

#[test]
fn static_completion_is_contextual_outside_a_repository() {
    isolated(
        "static_completion_is_contextual_outside_a_repository",
        true,
        |root| {
            for executable in ["git-oneflow", "gof"] {
                assert_eq!(
                    complete(root, &format!("{executable} st")),
                    ["start", "status"]
                );
                for (command, option, values) in [
                    ("s", "--policy", vec!["ordinary", "release", "hotfix"]),
                    ("config", "--format", vec!["toml", "yaml", "json"]),
                    ("f", "--mode", vec!["review", "integrate"]),
                    (
                        "finish",
                        "--integration",
                        vec!["ff-only", "merge", "squash"],
                    ),
                    ("f", "--checkout", vec!["target", "work"]),
                    ("finish", "--cleanup", vec!["keep", "local", "both"]),
                    ("f", "--update", vec!["rebase", "merge", "none"]),
                    ("finish", "--history-edit", vec!["none", "interactive"]),
                ] {
                    assert_eq!(
                        complete(root, &format!("{executable} {command} {option} ")),
                        values
                    );
                }
            }
            assert_eq!(complete(root, "gof config sh"), ["show"]);
            assert_eq!(complete(root, "gof v --sta"), ["--staged", "--stash"]);
            assert_eq!(complete(root, "gof start --from main --pol"), ["--policy"]);
            assert!(complete(root, "gof start --policy ordinary ").contains(&"--target".into()));
            for line in [
                "gof config show ",
                "gof status --help ",
                "gof start chosen second",
                "gof save --message ",
                "gof finish -m ",
                "gof finish --tag ",
                "gof finish --tag-message ",
                "gof start --from-tag ",
            ] {
                assert!(complete(root, line).is_empty(), "{line}");
            }
        },
    );
}

fn refs(root: &Path) -> Repository {
    let repository = Repository::new(root);
    repository.success(&["commit", "--allow-empty", "-m", "base"]);
    repository.success(&["branch", "local-work"]);
    repository.success(&["tag", "v1.2.3"]);
    repository.success(&["config", "remote.company/upstream.url", "unused://offline"]);
    repository.success(&[
        "config",
        "remote.company/upstream.fetch",
        "+refs/heads/*:refs/remotes/cache/*",
    ]);
    for branch in ["remote-work", "remote&work", "excluded", "local-work"] {
        repository.success(&[
            "update-ref",
            &format!("refs/remotes/cache/{branch}"),
            "HEAD",
        ]);
    }
    repository.success(&[
        "config",
        "--add",
        "remote.company/upstream.fetch",
        "^refs/heads/excluded",
    ]);
    repository.success(&[
        "symbolic-ref",
        "refs/remotes/cache/HEAD",
        "refs/remotes/cache/remote-work",
    ]);
    repository
}

#[test]
fn branch_tag_and_source_candidates_use_local_refs_without_effects() {
    isolated(
        "branch_tag_and_source_candidates_use_local_refs_without_effects",
        true,
        |root| {
            let repository = refs(root);
            let before = preservation::snapshot(root);
            assert_eq!(
                complete(repository.path(), "gof start remote-"),
                ["remote-work"]
            );
            assert_eq!(
                complete(repository.path(), "git-oneflow s local"),
                ["local-work"]
            );
            assert_eq!(
                complete(repository.path(), "gof s remote\\&"),
                ["remote\\&work"]
            );
            assert!(complete(repository.path(), "gof s excluded").is_empty());
            assert!(complete(repository.path(), "gof s HEAD").is_empty());
            assert_eq!(
                complete(repository.path(), "gof start --target remote-"),
                ["remote-work"]
            );
            assert_eq!(
                complete(repository.path(), "gof finish --merge-back local"),
                ["local-work"]
            );
            assert_eq!(
                complete(repository.path(), "gof config --marker local"),
                ["local-work"]
            );
            assert_eq!(
                complete(repository.path(), "gof start --from-tag v"),
                ["v1.2.3"]
            );
            assert_eq!(
                complete(repository.path(), "gof start --from cache/remote-"),
                ["cache/remote-work"]
            );
            assert_eq!(
                complete(repository.path(), "gof start --from v"),
                ["v1.2.3"]
            );
            assert!(complete(repository.path(), "gof start work --from-tag local").is_empty());
            assert!(complete(repository.path(), "gof finish --tag local").is_empty());
            assert_eq!(preservation::snapshot(root), before);
        },
    );
}

#[test]
fn ref_inspection_is_bounded_and_static_choices_require_no_git() {
    isolated(
        "ref_inspection_is_bounded_and_static_choices_require_no_git",
        true,
        |root| {
            use std::os::unix::fs::PermissionsExt;
            let bin = root.join("bin");
            fs::create_dir(&bin).unwrap();
            let spy = bin.join("git");
            fs::write(
                &spy,
                "#!/bin/sh\nprintf '%s\\n' \"$GIT_TERMINAL_PROMPT\" \"$LC_ALL\" \"$@\" >> \"$SPY_LOG\"\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(&spy, fs::Permissions::from_mode(0o755)).unwrap();
            let log = root.join("git-calls");
            let script = r#"source "$1"
COMP_LINE='gof finish --mode '; COMP_POINT=${#COMP_LINE}
_git_oneflow gof
[[ ${COMPREPLY[*]} == 'review integrate' ]] || exit 1
[[ ! -e $SPY_LOG ]] || exit 2
COMP_LINE='gof start work'; COMP_POINT=${#COMP_LINE}
_git_oneflow gof
[[ ${#COMPREPLY[@]} == 0 ]]"#;
            let output = Command::new("/bin/bash")
                .args(["--noprofile", "--norc", "-c", script, "test", artifact()])
                .env("PATH", &bin)
                .env("SPY_LOG", &log)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            let calls = fs::read_to_string(log).unwrap();
            assert_eq!(
                calls,
                "0\nC\n--no-optional-locks\n--no-lazy-fetch\n-c\ncore.fsmonitor=false\nconfig\n--null\n--get-regexp\n^remote\\..*\\.fetch$\n0\nC\n--no-optional-locks\n--no-lazy-fetch\n-c\ncore.fsmonitor=false\nfor-each-ref\n--format=%(refname)%09%(symref)\nrefs/heads\nrefs/remotes\nrefs/tags\n"
            );
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn readline_tab_preserves_literal_unicode_quotes_and_word_breaks() {
    isolated(
        "readline_tab_preserves_literal_unicode_quotes_and_word_breaks",
        true,
        |root| {
            let repository = refs(root);
            for branch in [
                "中文工作",
                "shell;$'\"`&(){}!work",
                "equals=work",
                "mail@work",
                "directory",
            ] {
                repository.success(&["branch", branch]);
            }
            fs::create_dir(repository.path().join("directory")).unwrap();
            let capture = root.join("arguments");
            let rc = root.join("bashrc");
            fs::write(
                &rc,
                format!(
                    "source {}\nPS1='ONEFLOW_READY> '\ncount=0\ngof() {{ printf '%s\\0' \"$@\" > {}; ((count+=1)); printf 'DONE:%s\\n' \"$count\"; }}\ngit-oneflow() {{ gof \"$@\"; }}\n",
                    shell_quote(artifact()),
                    shell_quote(capture.to_str().unwrap()),
                ),
            )
            .unwrap();
            let mut terminal = terminal::Terminal::start(
                "bash",
                repository.path(),
                &["--noprofile", "--rcfile", rc.to_str().unwrap(), "-i"],
            );
            terminal.until("ONEFLOW_READY> ");
            for (index, (line, expected_arguments)) in [
                ("gof s 中", "s\0中文工作\0"),
                ("git-oneflow start shell", "start\0shell;$'\"`&(){}!work\0"),
                ("gof s 'shell", "s\0shell;$'\"`&(){}!work\0"),
                ("gof s \"shell", "s\0shell;$'\"`&(){}!work\0"),
                ("gof s equals=", "s\0equals=work\0"),
                ("gof s mail@", "s\0mail@work\0"),
                ("gof s dire", "s\0directory\0"),
                ("gof s remote\\&", "s\0remote&work\0"),
                ("gof stat", "status\0"),
                (
                    "git-oneflow finish --mo\treview",
                    "finish\0--mode\0review\0",
                ),
                ("gof f --mode int", "f\0--mode\0integrate\0"),
            ]
            .iter()
            .enumerate()
            {
                terminal.send(format!("{line}\t\n").as_bytes());
                terminal.until(&format!("DONE:{}", index + 1));
                let arguments = fs::read(&capture).unwrap();
                assert_eq!(
                    arguments,
                    expected_arguments.as_bytes(),
                    "{line}: {}",
                    terminal.output
                );
            }
            terminal.send(b"exit\n");
            assert!(terminal.finish().success(), "{}", terminal.output);
        },
    );
}

#[cfg(target_os = "linux")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
