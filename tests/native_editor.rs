//! Real terminal editor handoff and literal message/recovery checks on each native row.
#[path = "support/native.rs"]
mod native;
mod support;
#[path = "support/native_terminal.rs"]
mod terminal;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::{Repository, isolated};
use terminal::NativeTerminal;

const WORK: &str = "feature/editor";
const MESSAGE: &[u8] =
    "Edited λ subject\n\nLiteral $(touch injected); $HOME `false`\n# discarded\n".as_bytes();
fn oid(repo: &Repository, name: &str) -> Vec<u8> {
    repo.success(&["rev-parse", name]).stdout
}
fn commit(repo: &Repository, name: &str) {
    fs::write(repo.path().join(name), name).unwrap();
    repo.success(&["add", name]);
    repo.success(&["commit", "-m", name]);
}
fn fixture(root: &Path, prepare: bool) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base");
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
    local.success(&["switch", "-c", WORK]);
    commit(&local, "work");
    if prepare {
        remote.success(&["switch", "main"]);
        commit(&remote, "advanced");
        remote.success(&["switch", "--detach"]);
    }
    fs::write(local.path().join(".git/twigsmith.config.json"), serde_json::to_vec(&serde_json::json!({
        "base":"main", "base-remote":"fixture", "publish-remote":"fixture", "finish-mode":"integrate",
        "finish-update":if prepare {"merge"} else {"none"}, "finish-integration":"merge", "finish-message":"ask",
        "finish-checkout":"target", "finish-cleanup":"local"
    })).unwrap()).unwrap();
    (local, remote)
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "/").replace('\'', "'\\''"))
}
fn editor(root: &Path) -> PathBuf {
    let source = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!(
            "native_editor_fixture{}",
            std::env::consts::EXE_SUFFIX
        ));
    assert!(source.is_file(), "build --example native_editor_fixture");
    let target = root.join(format!(
        "editor λ 'quoted' ;${}",
        std::env::consts::EXE_SUFFIX
    ));
    fs::copy(source, &target).unwrap();
    target
}
fn command(exe: &str, local: &Repository, root: &Path, action: &str) -> Command {
    let content = root.join("literal message.txt");
    fs::write(&content, MESSAGE).unwrap();
    let mut command = Command::new(exe);
    command
        .current_dir(local.path())
        .args(["finish"])
        .env("TERM", "dumb")
        .env("TWIGSMITH_EDITOR_CONTENT_FILE", content)
        .env("TWIGSMITH_EDITOR_ACTION", action)
        .env("TWIGSMITH_EDITOR_SUGGESTION_FILE", root.join("suggestion"))
        .env("TWIGSMITH_EDITOR_TRACE_FILE", root.join("trace"))
        .env("TWIGSMITH_EDITOR_MUTATION", "settings")
        .env("GIT_PAGER", "native-editor-pager-sentinel");
    command
}
fn no_message_files(local: &Repository) {
    assert!(!fs::read_dir(local.path().join(".git")).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("twigsmith-message-")
    }));
}
fn finish_editor(command: Command, response: &[u8], code: i32) -> String {
    let mut term = NativeTerminal::start_command(command);
    term.until("EDITOR_READY");
    term.send(response);
    assert_eq!(term.finish().code(), Some(code), "{}", term.output);
    term.assert_restored();
    println!("{}", term.output);
    term.output
}

#[test]
fn native_git_editor_accept_and_cancel_retry() {
    isolated("native_git_editor_accept_and_cancel_retry", true, |root| {
        let editor = format!(
            "{} --fixture-argument {}",
            quote(editor(root).to_str().unwrap()),
            quote("literal ;$λ")
        );
        println!(
            "native editor: {editor}; Git shell: {:?}",
            support::Repository::new(root).success(&["var", "GIT_SHELL_PATH"])
        );
        for (exe, method, atomic, override_editor) in [
            (env!("CARGO_BIN_EXE_tgi"), "merge", false, false),
            (env!("CARGO_BIN_EXE_twigsmith"), "squash", true, true),
        ] {
            let (local, remote) = fixture(root, false);
            local.success(&[
                "config",
                "core.editor",
                if override_editor {
                    "missing-editor"
                } else {
                    &editor
                },
            ]);
            let work = oid(&local, "HEAD");
            let tree = oid(&local, "HEAD^{tree}");
            let mut cmd = command(exe, &local, root, "accept");
            cmd.args(["--integration", method]);
            if override_editor {
                cmd.env("GIT_EDITOR", &editor);
            }
            if atomic {
                cmd.env("TWIGSMITH_EDITOR_ATOMIC", "1");
            }
            #[cfg(windows)]
            {
                let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                    .map(|dir| dir.join("git.exe"))
                    .find(|path| path.is_file())
                    .expect("Git on PATH");
                let directory = git.parent().unwrap();
                assert!(
                    !directory.join("sh.exe").exists(),
                    "qualify Git-only PATH without sh.exe"
                );
                cmd.env("PATH", directory);
                println!("Git-only PATH: {}", directory.display());
            }
            finish_editor(cmd, b"accept\n", 0);
            let trace: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join("trace")).unwrap()).unwrap();
            assert_eq!(trace["git_pager"], "native-editor-pager-sentinel");
            assert_eq!(trace["args"][0], "--fixture-argument");
            assert_eq!(trace["args"][1], "literal ;$λ");
            assert_eq!(Path::new(trace["cwd"].as_str().unwrap()), local.path());
            let suggested = fs::read_to_string(root.join("suggestion")).unwrap();
            assert!(
                suggested.contains(WORK) && suggested.contains("main"),
                "{suggested}"
            );
            assert_eq!(oid(&remote, "main"), oid(&local, "HEAD"));
            assert_ne!(oid(&local, "HEAD"), work);
            assert_eq!(oid(&local, "HEAD^{tree}"), tree);
            assert_eq!(
                local.success(&["log", "-1", "--format=%B"]).stdout,
                "Edited λ subject\n\nLiteral $(touch injected); $HOME `false`\n\n".as_bytes()
            );
            assert_eq!(
                local.success(&["symbolic-ref", "--short", "HEAD"]).stdout,
                b"main\n"
            );
            assert!(!local.path().join("injected").exists());
            no_message_files(&local);
            assert!(
                !twigsmith::git::run(
                    local.path(),
                    ["show-ref", "--verify", "refs/heads/feature/editor"]
                )
                .unwrap()
                .status
                .success()
            );
        }
        // Cancellation follows actual preparation. Retry must reuse that prepared history.
        let (local, remote) = fixture(root, true);
        local.success(&["config", "core.editor", &editor]);
        let target = oid(&remote, "main");
        let original = oid(&local, "HEAD");
        finish_editor(
            command(env!("CARGO_BIN_EXE_tgi"), &local, root, "accept"),
            b"cancel\n",
            1,
        );
        let prepared = oid(&local, "HEAD");
        assert_ne!(prepared, original);
        assert_eq!(oid(&remote, "main"), target);
        no_message_files(&local);
        let retry = command(env!("CARGO_BIN_EXE_tgi"), &local, root, "accept");
        finish_editor(retry, b"accept\n", 0);
        assert_eq!(oid(&local, "HEAD^2"), prepared);
        assert_eq!(oid(&remote, "main"), oid(&local, "HEAD"));
        no_message_files(&local);
    });
}

#[test]
fn native_git_editor_failures_and_bypass() {
    isolated("native_git_editor_failures_and_bypass", true, |root| {
        let editor = quote(editor(root).to_str().unwrap());
        for action in [
            "cancel",
            "blank",
            "missing",
            "directory",
            "nul",
            "invalid-utf8",
            "settings",
            "head",
            "index",
        ] {
            let (local, remote) = fixture(root, false);
            local.success(&["config", "core.editor", &editor]);
            let target = oid(&remote, "main");
            let work = oid(&local, "HEAD");
            let mutation = matches!(action, "settings" | "head" | "index");
            let mut cmd = command(
                env!("CARGO_BIN_EXE_twigsmith"),
                &local,
                root,
                if mutation { "mutate" } else { action },
            );
            if mutation {
                cmd.env("TWIGSMITH_EDITOR_MUTATION", action);
            }
            let output = finish_editor(cmd, b"accept\n", 1);
            if mutation {
                assert!(output.contains("changed"), "{output}");
            }
            assert_eq!(oid(&remote, "main"), target);
            if action == "head" {
                assert_ne!(oid(&local, "HEAD"), work);
                assert_eq!(oid(&local, "HEAD^"), work);
            } else {
                assert_eq!(oid(&local, "HEAD"), work);
            }
            if action == "index" {
                assert_eq!(
                    fs::read(local.path().join("editor-external-file")).unwrap(),
                    b"editor external work\n"
                );
                assert!(
                    !local
                        .success(&["diff", "--cached", "--name-only"])
                        .stdout
                        .is_empty()
                );
            }
            if action == "settings" {
                assert_eq!(
                    local
                        .success(&["config", "remote.fixture.receivepack"])
                        .stdout,
                    b"editor-external-value\n"
                );
            }
            assert_eq!(fs::read(local.path().join("work")).unwrap(), b"work");
            // A directory deliberately substituted by the editor is its external effect.
            if action != "directory" {
                no_message_files(&local);
            }
        }
        let (local, remote) = fixture(root, false);
        local.success(&["config", "core.editor", "absent-editor-executable"]);
        let target = oid(&remote, "main");
        let mut term = NativeTerminal::start_command(command(
            env!("CARGO_BIN_EXE_tgi"),
            &local,
            root,
            "accept",
        ));
        assert_eq!(term.finish().code(), Some(1), "{}", term.output);
        term.assert_restored();
        assert!(term.output.contains("integration message editing failed"));
        assert_eq!(oid(&remote, "main"), target);
        no_message_files(&local);
        // Git's ':' editor is explicitly a no-op, even with no external ':' executable.
        local.success(&["config", "core.editor", ":"]);
        let mut term = NativeTerminal::start_command(command(
            env!("CARGO_BIN_EXE_tgi"),
            &local,
            root,
            "accept",
        ));
        assert_eq!(term.finish().code(), Some(0), "{}", term.output);
        term.assert_restored();
        assert_eq!(oid(&remote, "main"), oid(&local, "HEAD"));
        no_message_files(&local);
        for explicit in [false, true] {
            let (local, remote) = fixture(root, false);
            local.success(&["config", "core.editor", "absent-editor-executable"]);
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_tgi"));
            cmd.current_dir(local.path()).args(["finish"]);
            if explicit {
                cmd.args(["-m", "Explicit intent"]);
            } else {
                let config = local.path().join(".git/twigsmith.config.json");
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
                value["finish-message"] = "auto".into();
                fs::write(config, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            let result = native::output(&mut cmd);
            assert!(result.status.success(), "{result:?}");
            assert_eq!(oid(&remote, "main"), oid(&local, "HEAD"));
        }
    });
}
