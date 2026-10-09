#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use git_oneflow::{
    git, repository,
    save::{self, ActionSource, Request, SaveError, execution},
    settings::{
        self, CONFIG_FILES, FinishRewrite, FinishUpdate, SaveAction, SettingSource, Settings,
        SettingsError,
    },
    start,
};
use preservation::snapshot;
use std::{fs, path::Path};
use support::{Repository, isolated};

fn load(directory: &Path) -> Result<Settings, SettingsError> {
    settings::load(
        directory,
        &repository::discover(directory).unwrap().unwrap(),
    )
}
fn load_all(directory: &Path) -> settings::LoadedSettings {
    settings::load_with_files(
        directory,
        &repository::discover(directory).unwrap().unwrap(),
    )
    .unwrap()
}
fn config(format: usize, pairs: &[(&str, &str)]) -> String {
    match format {
        0 => pairs
            .iter()
            .map(|(k, v)| format!("{k} = {}\n", serde_json::to_string(v).unwrap()))
            .collect(),
        1 | 2 => {
            if pairs.is_empty() {
                "{}".into()
            } else {
                pairs
                    .iter()
                    .map(|(k, v)| format!("{k}: {}\n", serde_json::to_string(v).unwrap()))
                    .collect()
            }
        }
        3 => format!(
            "{{{}}}",
            pairs
                .iter()
                .map(|(k, v)| format!("{k:?}: {}", serde_json::to_string(v).unwrap()))
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => unreachable!(),
    }
}
fn write(root: &Path, format: usize, pairs: &[(&str, &str)]) {
    fs::write(root.join(CONFIG_FILES[format]), config(format, pairs)).unwrap();
}
fn commit(repo: &Repository) {
    repo.success(&["add", "."]);
    repo.success(&["commit", "-m", "fixture"]);
}

#[test]
fn formats_share_sparse_resolution_defaults_and_sources() {
    isolated(
        "formats_share_sparse_resolution_defaults_and_sources",
        true,
        |root| {
            let repo = Repository::new(root);
            assert_eq!(load(repo.path()).unwrap(), Settings::default());
            for (shared, shared_file) in CONFIG_FILES.iter().enumerate() {
                write(repo.path(), shared, &[]);
                assert_eq!(load(repo.path()).unwrap(), Settings::default());
                write(
                    repo.path(),
                    shared,
                    &[
                        ("remote", "team/upstream"),
                        ("base", "Feature/École"),
                        ("save-action", "stash"),
                    ],
                );
                for (local, local_file) in CONFIG_FILES.iter().enumerate() {
                    let common = repo.path().join(".git");
                    write(
                        &common,
                        local,
                        &[("remote", "private"), ("save-action", "wip")],
                    );
                    let before = snapshot(root);
                    let resolved = load(repo.path()).unwrap();
                    assert_eq!(snapshot(root), before);
                    assert_eq!(resolved.publish_remote, "private");
                    assert_eq!(resolved.base, "Feature/École");
                    assert_eq!(resolved.save_action, SaveAction::Wip);
                    assert_eq!(
                        resolved.sources.publish_remote,
                        SettingSource::Local(common.join(local_file))
                    );
                    assert_eq!(
                        resolved.sources.save_action,
                        resolved.sources.publish_remote
                    );
                    assert_eq!(
                        resolved.sources.base,
                        SettingSource::Shared(repo.path().join(shared_file))
                    );
                    fs::remove_file(common.join(local_file)).unwrap();
                }
                fs::remove_file(repo.path().join(shared_file)).unwrap();
                write(repo.path(), shared, &[("base", "trunk")]);
                let resolved = load(repo.path()).unwrap();
                assert_eq!(resolved.publish_remote, "origin");
                assert_eq!(resolved.save_action, SaveAction::Ask);
                assert_eq!(resolved.sources.publish_remote, SettingSource::BuiltIn);
                assert_eq!(resolved.sources.save_action, SettingSource::BuiltIn);
                fs::remove_file(repo.path().join(shared_file)).unwrap();
            }
        },
    );
}

#[test]
fn strict_formats_reject_bad_data_even_when_overridden() {
    isolated(
        "strict_formats_reject_bad_data_even_when_overridden",
        true,
        |root| {
            let repo = Repository::new(root);
            write(
                &repo.path().join(".git"),
                3,
                &[
                    ("remote", "origin"),
                    ("base", "main"),
                    ("save-action", "wip"),
                ],
            );
            let invalid = [
                vec![
                    "remote =",
                    "other = 'x'",
                    "base='a'\nbase='a'",
                    "remote='a'\nremote='b'",
                    "save-action='wip'\nsave-action='wip'",
                    "remote=42",
                    "base=true",
                    "save-action=[]",
                    "base={nested='x'}",
                    "base=2026-10-08",
                    "[nested]\nbase='main'",
                ],
                vec![
                    "base: [",
                    "other: x",
                    "base: a\nbase: a",
                    "remote: a\nremote: b",
                    "save-action: wip\nsave-action: wip",
                    "remote: 42",
                    "base: true",
                    "save-action: []",
                    "base: {nested: x}",
                    "base: null",
                    "base: \n",
                    "[]",
                    "null",
                    "",
                    "base: main\n---\nbase: trunk",
                    "<<: {base: main}",
                    "base: !include file",
                    "base: !custom main",
                ],
                vec![
                    "base: [",
                    "other: x",
                    "base: a\nbase: a",
                    "remote: a\nremote: b",
                    "save-action: wip\nsave-action: wip",
                    "remote: 42",
                    "base: true",
                    "save-action: []",
                    "base: {nested: x}",
                    "base: null",
                ],
                vec![
                    "{",
                    "{\"other\":\"x\"}",
                    "{\"base\":\"a\",\"base\":\"a\"}",
                    "{\"remote\":\"a\",\"remote\":\"b\"}",
                    "{\"save-action\":\"wip\",\"save-action\":\"wip\"}",
                    "{\"remote\":42}",
                    "{\"base\":true}",
                    "{\"save-action\":[]}",
                    "{\"base\":{\"nested\":\"x\"}}",
                    "{\"base\":null}",
                    "[]",
                    "null",
                    "{} {}",
                ],
            ];
            for (format, _) in CONFIG_FILES.iter().enumerate() {
                let path = repo.path().join(CONFIG_FILES[format]);
                for data in &invalid[format] {
                    fs::write(&path, data).unwrap();
                    let before = snapshot(root);
                    let error = load(repo.path())
                        .expect_err(&format!("format {format}: {data:?}"))
                        .to_string();
                    assert!(error.contains(CONFIG_FILES[format]), "{error}");
                    assert_eq!(snapshot(root), before);
                }
                for (key, value) in [
                    ("remote", ""),
                    ("remote", "-option"),
                    ("remote", "."),
                    ("remote", "../repo"),
                    ("base", ""),
                    ("base", "HEAD"),
                    ("base", "@{-1}"),
                    ("base", "a..b"),
                    ("base", "-branch"),
                    ("base", "bad\0name"),
                    ("save-action", ""),
                    ("save-action", "WIP"),
                    ("save-action", "commit"),
                ] {
                    write(repo.path(), format, &[(key, value)]);
                    assert!(load(repo.path()).is_err(), "{format} {key} {value:?}");
                }
                for action in ["ask", "publish-only", "staged", "wip", "stash"] {
                    write(repo.path(), format, &[("save-action", action)]);
                    assert!(load(repo.path()).is_ok());
                }
                fs::remove_file(path).unwrap();
            }
            // YAML 1.2 boolean spelling leaves these valid Git names as strings.
            fs::write(
                repo.path().join(CONFIG_FILES[1]),
                "remote: on\nbase: yes\nsave-action: ask\n",
            )
            .unwrap();
            let shared = load(repo.path()).unwrap();
            // The local layer still overrides all three keys; inspect the shared
            // layer directly after removing the override to verify plain strings.
            assert_eq!(shared.publish_remote, "origin");
            fs::remove_file(repo.path().join(".git").join(CONFIG_FILES[3])).unwrap();
            let shared = load(repo.path()).unwrap();
            assert_eq!(shared.publish_remote, "on");
            assert_eq!(shared.base, "yes");
            fs::remove_file(repo.path().join(CONFIG_FILES[1])).unwrap();
            // Invalid high-priority files fail too, even with an explicit save flag.
            fs::write(repo.path().join("tracked"), "initial").unwrap();
            commit(&repo);
            repo.success(&["switch", "-c", "work"]);
            fs::write(
                repo.path().join(".git").join(CONFIG_FILES[3]),
                "{\"save-action\":null}",
            )
            .unwrap();
            assert!(matches!(
                save::assess(
                    repo.path(),
                    &Request {
                        action: Some(SaveAction::PublishOnly),
                        message: None
                    }
                ),
                Err(SaveError::Settings(_))
            ));
        },
    );
}

#[test]
fn ambiguity_is_per_layer_without_extension_priority() {
    isolated(
        "ambiguity_is_per_layer_without_extension_priority",
        true,
        |root| {
            let repo = Repository::new(root);
            for layer in [repo.path().to_path_buf(), repo.path().join(".git")] {
                for (a, a_file) in CONFIG_FILES.iter().enumerate() {
                    for (b, b_file) in CONFIG_FILES.iter().enumerate().skip(a + 1) {
                        write(&layer, a, &[]);
                        write(&layer, b, &[]);
                        let error = load(repo.path()).unwrap_err().to_string();
                        assert!(error.contains("multiple configuration files"));
                        assert!(error.contains(a_file) && error.contains(b_file));
                        assert!(error.contains("exactly one"));
                        fs::remove_file(layer.join(a_file)).unwrap();
                        fs::remove_file(layer.join(b_file)).unwrap();
                    }
                }
            }
        },
    );
}

#[test]
fn legacy_is_a_sparse_local_fallback_with_explicit_migration_conflicts() {
    isolated(
        "legacy_is_a_sparse_local_fallback_with_explicit_migration_conflicts",
        true,
        |root| {
            let repo = Repository::new(root);
            let legacy = repo.path().join(".git/oneflow.conf");
            for (format, _) in CONFIG_FILES.iter().enumerate() {
                write(
                    repo.path(),
                    format,
                    &[
                        ("remote", "team"),
                        ("base", "trunk"),
                        ("save-action", "wip"),
                    ],
                );
                fs::write(
                    &legacy,
                    "# private\r\nremote = private\r\nsave-action = stash\n",
                )
                .unwrap();
                let resolved = load(repo.path()).unwrap();
                assert_eq!(resolved.publish_remote, "private");
                assert_eq!(resolved.base, "trunk");
                assert_eq!(resolved.save_action, SaveAction::Stash);
                assert_eq!(
                    resolved.sources.publish_remote,
                    SettingSource::Local(legacy.clone())
                );
                fs::write(&legacy, "# no overrides\n").unwrap();
                assert_eq!(
                    load(repo.path()).unwrap().sources.publish_remote,
                    SettingSource::Shared(repo.path().join(CONFIG_FILES[format]))
                );
                write(&repo.path().join(".git"), format, &[]);
                let error = load(repo.path()).unwrap_err().to_string();
                assert!(
                    error.contains("Migrate oneflow.conf") && error.contains("remove or rename")
                );
                fs::remove_file(repo.path().join(".git").join(CONFIG_FILES[format])).unwrap();
                fs::remove_file(repo.path().join(CONFIG_FILES[format])).unwrap();
            }
            for data in [
                "remote",
                "remote=",
                "other=x",
                "base=a\nbase=b",
                "save-action=wip\nsave-action=wip",
                "save-action=commit",
                "save-action=wip # inline",
                "base=HEAD",
            ] {
                fs::write(&legacy, data).unwrap();
                assert!(load(repo.path()).is_err(), "{data}");
            }
        },
    );
}

#[test]
fn discovery_uses_git_roots_and_common_dir_without_inheritance() {
    isolated(
        "discovery_uses_git_roots_and_common_dir_without_inheritance",
        true,
        |root| {
            let repo = Repository::new(root);
            // Malformed parent/home files and legacy files in working trees are ignored.
            for (format, _) in CONFIG_FILES.iter().enumerate() {
                fs::write(root.join(CONFIG_FILES[format]), "malformed").unwrap();
                fs::write(root.join("home").join(CONFIG_FILES[format]), "malformed").unwrap();
            }
            fs::write(repo.path().join("oneflow.conf"), "malformed").unwrap();
            let nested = repo.path().join("deep/nested");
            fs::create_dir_all(&nested).unwrap();
            write(&nested, 3, &[("base", "ignored")]);
            assert_eq!(load(&nested).unwrap(), Settings::default());
            write(repo.path(), 0, &[("base", "trunk")]);
            commit(&repo);
            let linked = root.join("linked");
            repo.success(&[
                "worktree".as_ref(),
                "add".as_ref(),
                "-b".as_ref(),
                "other".as_ref(),
                linked.as_os_str(),
            ]);
            write(&linked, 0, &[("base", "linked-base")]);
            let linked_nested = linked.join("deep/child");
            fs::create_dir_all(&linked_nested).unwrap();
            assert_eq!(load(&nested).unwrap(), load(repo.path()).unwrap());
            assert_eq!(load(&linked_nested).unwrap(), load(&linked).unwrap());
            assert_eq!(load(&linked).unwrap().base, "linked-base");
            let context = repository::discover(&linked).unwrap().unwrap();
            write(&context.git_dir, 3, &[("base", "ignored")]);
            write(&context.common_dir, 1, &[("remote", "private")]);
            assert_eq!(load(&linked).unwrap().publish_remote, "private");
            assert_eq!(load(repo.path()).unwrap().publish_remote, "private");
            assert_eq!(
                load(&linked).unwrap().sources.publish_remote,
                load(repo.path()).unwrap().sources.publish_remote
            );
            assert_eq!(load(repo.path()).unwrap().base, "trunk");
            // Bare common-directory files form only the local layer, never both layers.
            let bare = root.join("bare.git");
            assert!(
                git::run(root, ["init".as_ref(), "--bare".as_ref(), bare.as_os_str()])
                    .unwrap()
                    .status
                    .success()
            );
            assert_eq!(load(&bare).unwrap(), Settings::default());
            for (format, _) in CONFIG_FILES.iter().enumerate() {
                write(&bare, format, &[("base", "bare-base")]);
                let resolved = load(&bare).unwrap();
                assert_eq!(resolved.base, "bare-base");
                assert_eq!(
                    resolved.sources.base,
                    SettingSource::Local(bare.join(CONFIG_FILES[format]))
                );
                fs::remove_file(bare.join(CONFIG_FILES[format])).unwrap();
            }
            fs::write(bare.join("oneflow.conf"), "base=legacy-bare").unwrap();
            assert_eq!(load(&bare).unwrap().base, "legacy-bare");
        },
    );
}

#[test]
fn existing_unreadable_configuration_entries_fail() {
    isolated(
        "existing_unreadable_configuration_entries_fail",
        true,
        |root| {
            let repo = Repository::new(root);
            for layer in [repo.path().to_path_buf(), repo.path().join(".git")] {
                for name in CONFIG_FILES.into_iter().chain(["oneflow.conf"]) {
                    if name == "oneflow.conf" && layer == repo.path() {
                        continue;
                    }
                    let path = layer.join(name);
                    fs::create_dir(&path).unwrap();
                    assert!(matches!(
                        load(repo.path()),
                        Err(SettingsError::SettingsIo { .. })
                    ));
                    fs::remove_dir(&path).unwrap();
                    fs::write(&path, [0xff]).unwrap();
                    assert!(matches!(
                        load(repo.path()),
                        Err(SettingsError::SettingsIo { .. })
                    ));
                    fs::remove_file(&path).unwrap();
                    #[cfg(unix)]
                    {
                        std::os::unix::fs::symlink(root.join("missing"), &path).unwrap();
                        assert!(matches!(
                            load(repo.path()),
                            Err(SettingsError::SettingsIo { .. })
                        ));
                        fs::remove_file(&path).unwrap();
                        // Exercise permission failures where the runner lacks
                        // root's ability to read mode-000 files.
                        use std::os::unix::fs::PermissionsExt;
                        fs::write(&path, "").unwrap();
                        let original = fs::metadata(&path).unwrap().permissions();
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
                        if fs::read(&path).is_err() {
                            assert!(matches!(
                                load(repo.path()),
                                Err(SettingsError::SettingsIo { .. })
                            ));
                        }
                        fs::set_permissions(&path, original).unwrap();
                        fs::remove_file(path).unwrap();
                    }
                }
            }
        },
    );
}

#[test]
fn start_and_save_consume_each_format_and_existing_action_overrides() {
    isolated(
        "start_and_save_consume_each_format_and_existing_action_overrides",
        true,
        |root| {
            for (format, _) in CONFIG_FILES.iter().enumerate() {
                let remote = Repository::new(root);
                remote.success(&["switch", "-c", "trunk"]);
                write(
                    remote.path(),
                    format,
                    &[
                        ("remote", "team"),
                        ("base", "trunk"),
                        ("save-action", "stash"),
                    ],
                );
                fs::write(remote.path().join("tracked"), "base").unwrap();
                commit(&remote);
                let local = Repository::new(root);
                local.success(&[
                    "remote".as_ref(),
                    "add".as_ref(),
                    "team".as_ref(),
                    remote.path().as_os_str(),
                ]);
                local.success(&["fetch", "team"]);
                local.success(&["reset", "--hard", "refs/remotes/team/trunk"]);
                let request = start::prepare(local.path(), "work").unwrap();
                assert_eq!(request.settings.publish_remote, "team");
                assert_eq!(request.settings.base, "trunk");
                let result = start::execute(local.path(), &request).unwrap();
                assert_eq!(result.action, start::Action::Create);
                assert_eq!(
                    local.success(&["rev-parse", "HEAD"]).stdout,
                    remote.success(&["rev-parse", "trunk"]).stdout
                );
                write(
                    &local.path().join(".git"),
                    (format + 1) % 4,
                    &[("save-action", "wip")],
                );
                fs::write(local.path().join("tracked"), "progress").unwrap();
                let configured = save::assess(local.path(), &Request::default()).unwrap();
                assert_eq!(configured.action, SaveAction::Wip);
                assert_eq!(
                    configured.action_source,
                    ActionSource::Setting(configured.settings.sources.save_action.clone())
                );
                for flag in [SaveAction::PublishOnly, SaveAction::Stash, SaveAction::Wip] {
                    let explicit = save::assess(
                        local.path(),
                        &Request {
                            action: Some(flag),
                            message: None,
                        },
                    )
                    .unwrap();
                    assert_eq!(explicit.action, flag);
                    assert_eq!(explicit.action_source, ActionSource::ExplicitSelection);
                }
                local.success(&["add", "tracked"]);
                let staged = save::assess(
                    local.path(),
                    &Request {
                        action: Some(SaveAction::Staged),
                        message: Some("exact".into()),
                    },
                )
                .unwrap();
                assert_eq!(staged.action_source, ActionSource::ExplicitSelection);
                let plan = save::assess(local.path(), &Request::default()).unwrap();
                let report = execution::execute(local.path(), &plan);
                assert!(report.is_complete(), "{report:?}");
                assert_eq!(
                    local.success(&["rev-parse", "HEAD"]).stdout,
                    remote.success(&["rev-parse", "work"]).stdout
                );
                assert_eq!(
                    local.success(&["log", "-1", "--format=%s"]).stdout,
                    b"[WIP - Staged]\n"
                );
                assert_eq!(
                    save::assess(local.path(), &Request::default())
                        .unwrap()
                        .action_source,
                    ActionSource::CleanTree
                );
            }
        },
    );
}

#[test]
fn layered_settings_changes_are_revalidated_before_effects() {
    isolated(
        "layered_settings_changes_are_revalidated_before_effects",
        true,
        |root| {
            let repo = Repository::new(root);
            write(repo.path(), 0, &[("save-action", "wip")]);
            fs::write(repo.path().join("tracked"), "base").unwrap();
            commit(&repo);
            repo.success(&["switch", "-c", "work"]);
            let remote = root.join("publication.git");
            assert!(
                git::run(
                    root,
                    ["init".as_ref(), "--bare".as_ref(), remote.as_os_str()]
                )
                .unwrap()
                .status
                .success()
            );
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                remote.as_os_str(),
            ]);
            fs::write(repo.path().join("tracked"), "work").unwrap();
            for local in [false, true] {
                for (key, value) in [
                    ("remote", "missing"),
                    ("base", "work"),
                    ("save-action", "stash"),
                ] {
                    let plan = save::assess(repo.path(), &Request::default()).unwrap();
                    let layer = if local {
                        repo.path().join(".git")
                    } else {
                        repo.path().to_path_buf()
                    };
                    let format = if local { 3 } else { 0 };
                    write(&layer, format, &[(key, value)]);
                    let before = snapshot(root);
                    assert!(save::revalidate(repo.path(), &plan).is_err());
                    let report = execution::execute(repo.path(), &plan);
                    assert!(
                        report.error.is_some()
                            && report.commands.is_empty()
                            && report.effects.is_empty()
                    );
                    assert_eq!(snapshot(root), before);
                    if local {
                        fs::remove_file(layer.join(CONFIG_FILES[format])).unwrap();
                    } else {
                        write(repo.path(), 0, &[("save-action", "wip")]);
                    }
                }
            }
            // Start uses the same layer reload before mutation.
            let request = start::prepare(repo.path(), "another").unwrap();
            write(&repo.path().join(".git"), 1, &[("base", "trunk")]);
            let before = snapshot(root);
            assert!(start::execute(repo.path(), &request).is_err());
            assert_eq!(snapshot(root), before);
        },
    );
}

#[test]
fn stashing_changed_shared_settings_stops_before_publication() {
    isolated(
        "stashing_changed_shared_settings_stops_before_publication",
        true,
        |root| {
            let repo = Repository::new(root);
            write(repo.path(), 0, &[("save-action", "ask")]);
            fs::write(repo.path().join("tracked"), "base").unwrap();
            commit(&repo);
            repo.success(&["switch", "-c", "work"]);
            let remote = root.join("stash-publication.git");
            assert!(
                git::run(
                    root,
                    ["init".as_ref(), "--bare".as_ref(), remote.as_os_str()]
                )
                .unwrap()
                .status
                .success()
            );
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                remote.as_os_str(),
            ]);
            write(repo.path(), 0, &[("save-action", "stash")]);
            fs::write(repo.path().join("tracked"), "progress").unwrap();
            let plan = save::assess(repo.path(), &Request::default()).unwrap();
            assert_eq!(plan.action, SaveAction::Stash);
            let report = execution::execute(repo.path(), &plan);
            assert!(report.error.is_some(), "{report:?}");
            assert!(report.publication.is_none());
            assert_eq!(report.commands.len(), 1);
            assert!(report.commands[0].result.as_ref().unwrap().status.success());
            // The context guard stops verification of the whole stash step;
            // the successful command and independent observations retain its OID.
            assert!(report.after.stash.as_ref().unwrap().is_some());
            assert_eq!(load(repo.path()).unwrap().save_action, SaveAction::Ask);
            assert_eq!(fs::read(repo.path().join("tracked")).unwrap(), b"base");
            repo.success(&["rev-parse", "--verify", "refs/stash"]);
            assert!(
                git::run(&remote, ["show-ref", "--verify", "refs/heads/work"])
                    .unwrap()
                    .status
                    .code()
                    .is_some_and(|code| code != 0)
            );
        },
    );
}

#[test]
fn finish_settings_sparse_layers_legacy_and_sources() {
    isolated(
        "finish_settings_sparse_layers_legacy_and_sources",
        true,
        |root| {
            let repo = Repository::new(root);
            let defaults = load(repo.path()).unwrap();
            assert_eq!(defaults.finish_update, FinishUpdate::Ask);
            assert_eq!(defaults.finish_rewrite, FinishRewrite::Ask);
            assert!(load_all(repo.path()).redact_credentials);
            assert_eq!(defaults.sources.finish_update, SettingSource::BuiltIn);
            assert_eq!(defaults.sources.finish_rewrite, SettingSource::BuiltIn);
            for (shared, shared_file) in CONFIG_FILES.iter().enumerate() {
                write(
                    repo.path(),
                    shared,
                    &[("finish-update", "merge"), ("finish-rewrite", "deny")],
                );
                for (local, local_file) in CONFIG_FILES.iter().enumerate() {
                    let common = repo.path().join(".git");
                    write(&common, local, &[("finish-update", "rebase")]);
                    let before = snapshot(root);
                    let resolved = load(repo.path()).unwrap();
                    assert_eq!(snapshot(root), before);
                    assert_eq!(resolved.finish_update, FinishUpdate::Rebase);
                    assert_eq!(resolved.finish_rewrite, FinishRewrite::Deny);
                    assert_eq!(
                        resolved.sources.finish_update,
                        SettingSource::Local(common.join(local_file))
                    );
                    assert_eq!(
                        resolved.sources.finish_rewrite,
                        SettingSource::Shared(repo.path().join(shared_file))
                    );
                    write(&common, local, &[("finish-rewrite", "allow")]);
                    let resolved = load(repo.path()).unwrap();
                    assert_eq!(resolved.finish_update, FinishUpdate::Merge);
                    assert_eq!(resolved.finish_rewrite, FinishRewrite::Allow);
                    assert_eq!(
                        resolved.sources.finish_update,
                        SettingSource::Shared(repo.path().join(shared_file))
                    );
                    assert_eq!(
                        resolved.sources.finish_rewrite,
                        SettingSource::Local(common.join(local_file))
                    );
                    fs::remove_file(common.join(local_file)).unwrap();
                }
                let legacy = repo.path().join(".git/oneflow.conf");
                fs::write(&legacy, "finish-update=none\n").unwrap();
                let resolved = load(repo.path()).unwrap();
                assert_eq!(resolved.finish_update, FinishUpdate::None);
                assert_eq!(
                    resolved.sources.finish_update,
                    SettingSource::Local(legacy.clone())
                );
                assert_eq!(resolved.finish_rewrite, FinishRewrite::Deny);
                fs::write(&legacy, "finish-rewrite=ask\n").unwrap();
                let resolved = load(repo.path()).unwrap();
                assert_eq!(resolved.finish_update, FinishUpdate::Merge);
                assert_eq!(resolved.finish_rewrite, FinishRewrite::Ask);
                assert_eq!(
                    resolved.sources.finish_rewrite,
                    SettingSource::Local(legacy.clone())
                );
                fs::remove_file(legacy).unwrap();
                fs::remove_file(repo.path().join(shared_file)).unwrap();
            }
            for (format, filename) in CONFIG_FILES.iter().enumerate() {
                for update in ["ask", "rebase", "merge", "none"] {
                    for rewrite in ["ask", "allow", "deny"] {
                        write(
                            repo.path(),
                            format,
                            &[("finish-update", update), ("finish-rewrite", rewrite)],
                        );
                        let resolved = load(repo.path()).unwrap();
                        assert_eq!(resolved.finish_update.as_str(), update);
                        assert_eq!(resolved.finish_rewrite.as_str(), rewrite);
                    }
                }
                fs::remove_file(repo.path().join(filename)).unwrap();
            }
            let legacy = repo.path().join(".git/oneflow.conf");
            for update in ["ask", "rebase", "merge", "none"] {
                for rewrite in ["ask", "allow", "deny"] {
                    fs::write(
                        &legacy,
                        format!("finish-update={update}\nfinish-rewrite={rewrite}\n"),
                    )
                    .unwrap();
                    let resolved = load(repo.path()).unwrap();
                    assert_eq!(resolved.finish_update.as_str(), update);
                    assert_eq!(resolved.finish_rewrite.as_str(), rewrite);
                }
            }
            fs::remove_file(legacy).unwrap();
        },
    );
}

#[test]
fn finish_settings_strict_validation_in_every_layer() {
    isolated(
        "finish_settings_strict_validation_in_every_layer",
        true,
        |root| {
            let repo = Repository::new(root);
            for (format, filename) in CONFIG_FILES.iter().enumerate() {
                for key in ["finish-update", "finish-rewrite"] {
                    for value in ["", "ASK", "true", "squash", "allow # comment"] {
                        for local in [false, true] {
                            let directory = if local {
                                repo.path().join(".git")
                            } else {
                                repo.path().to_owned()
                            };
                            write(&directory, format, &[(key, value)]);
                            // A valid higher-precedence layer does not hide invalid values.
                            if !local {
                                write(&repo.path().join(".git"), format, &[(key, "ask")]);
                            }
                            let before = snapshot(root);
                            let error = load(repo.path()).unwrap_err().to_string();
                            assert_eq!(snapshot(root), before);
                            assert!(error.contains(key) && error.contains(filename), "{error}");
                            fs::remove_file(directory.join(filename)).unwrap();
                            if !local {
                                fs::remove_file(repo.path().join(".git").join(filename)).unwrap();
                            }
                        }
                    }
                    for duplicate in [true, false] {
                        let invalid = match (format, duplicate) {
                            (0, true) => format!("{key}='ask'\n{key}='ask'\n"),
                            (0, false) => format!("{key}=false\n"),
                            (1 | 2, true) => format!("{key}: ask\n{key}: ask\n"),
                            (1 | 2, false) => format!("{key}: false\n"),
                            (3, true) => format!("{{{key:?}:\"ask\",{key:?}:\"ask\"}}"),
                            (3, false) => format!("{{{key:?}:false}}"),
                            _ => unreachable!(),
                        };
                        for directory in [repo.path().to_owned(), repo.path().join(".git")] {
                            let path = directory.join(filename);
                            fs::write(&path, &invalid).unwrap();
                            assert!(load(repo.path()).is_err(), "{invalid}");
                            fs::remove_file(path).unwrap();
                        }
                    }
                    let legacy = repo.path().join(".git/oneflow.conf");
                    for invalid in [
                        format!("{key}=ASK\n"),
                        format!("{key}=ask\n{key}=ask\n"),
                        format!("{key}=\n"),
                    ] {
                        fs::write(&legacy, invalid).unwrap();
                        assert!(load(repo.path()).is_err());
                    }
                    fs::remove_file(legacy).unwrap();
                }
            }
        },
    );
}

#[test]
fn credential_redaction_setting_is_private_and_strict() {
    isolated(
        "credential_redaction_setting_is_private_and_strict",
        true,
        |root| {
            let repo = Repository::new(root);
            let workflow_settings = load(repo.path()).unwrap();
            for (format, filename) in CONFIG_FILES.iter().enumerate() {
                let common = repo.path().join(".git");
                for (value, expected) in [("true", true), ("false", false)] {
                    let contents = match format {
                        0 => format!("redact-credentials = {value}\n"),
                        1 | 2 => format!("redact-credentials: {value}\n"),
                        _ => format!("{{\"redact-credentials\":{value}}}"),
                    };
                    fs::write(common.join(filename), contents).unwrap();
                    let settings = load_all(repo.path());
                    assert_eq!(settings.redact_credentials, expected);
                    assert_eq!(
                        settings::load(
                            repo.path(),
                            &repository::discover(repo.path()).unwrap().unwrap()
                        )
                        .unwrap(),
                        workflow_settings
                    );
                    assert_eq!(
                        settings.redact_credentials_source,
                        SettingSource::Local(common.join(filename))
                    );
                }
                fs::remove_file(common.join(filename)).unwrap();

                let shared = match format {
                    0 => "redact-credentials = true\n".to_owned(),
                    1 | 2 => "redact-credentials: true\n".to_owned(),
                    _ => "{\"redact-credentials\":true}".to_owned(),
                };
                fs::write(repo.path().join(filename), shared).unwrap();
                let error = load(repo.path()).unwrap_err().to_string();
                assert!(error.contains("private local setting"), "{error}");
                fs::remove_file(repo.path().join(filename)).unwrap();

                fs::write(common.join(filename), "redact-credentials = \"false\"\n").unwrap();
                assert!(load(repo.path()).is_err());
                fs::remove_file(common.join(filename)).unwrap();
            }
            let legacy = repo.path().join(".git/oneflow.conf");
            fs::write(&legacy, "redact-credentials=false\n").unwrap();
            assert!(!load_all(repo.path()).redact_credentials);
        },
    );
}

#[test]
fn remote_roles_resolve_aliases_per_layer_and_derive_base_across_all_formats() {
    isolated(
        "remote_roles_resolve_aliases_per_layer_and_derive_base_across_all_formats",
        true,
        |root| {
            use settings::PublicationKey::{PublishRemote, Remote};
            let repo = Repository::new(root);
            let common = repo.path().join(".git");
            type Case<'a> = (
                &'a [(&'a str, &'a str)],
                &'a [(&'a str, &'a str)],
                &'a str,
                &'a str,
                Option<settings::PublicationKey>,
                bool,
                Option<bool>,
            );
            let cases: &[Case<'_>] = &[
                (&[], &[], "origin", "origin", None, false, None),
                (
                    &[("base-remote", "project")],
                    &[],
                    "origin",
                    "project",
                    None,
                    false,
                    Some(false),
                ),
                (
                    &[("publish-remote", "personal"), ("remote", "team")],
                    &[],
                    "personal",
                    "personal",
                    Some(PublishRemote),
                    false,
                    None,
                ),
                (
                    &[("remote", "team")],
                    &[],
                    "team",
                    "team",
                    Some(Remote),
                    false,
                    None,
                ),
                (
                    &[("publish-remote", "personal")],
                    &[],
                    "personal",
                    "personal",
                    Some(PublishRemote),
                    false,
                    None,
                ),
                (
                    &[("remote", "team"), ("publish-remote", "personal")],
                    &[],
                    "personal",
                    "personal",
                    Some(PublishRemote),
                    false,
                    None,
                ),
                (
                    &[("publish-remote", "personal"), ("base-remote", "project")],
                    &[],
                    "personal",
                    "project",
                    Some(PublishRemote),
                    false,
                    Some(false),
                ),
                (
                    &[("publish-remote", "shared")],
                    &[("remote", "local")],
                    "local",
                    "local",
                    Some(Remote),
                    true,
                    None,
                ),
                (
                    &[("remote", "shared")],
                    &[("publish-remote", "local")],
                    "local",
                    "local",
                    Some(PublishRemote),
                    true,
                    None,
                ),
                (
                    &[("publish-remote", "shared"), ("base-remote", "project")],
                    &[("remote", "local"), ("publish-remote", "personal")],
                    "personal",
                    "project",
                    Some(PublishRemote),
                    true,
                    Some(false),
                ),
                (
                    &[("publish-remote", "shared"), ("base-remote", "project")],
                    &[("base-remote", "local-base")],
                    "shared",
                    "local-base",
                    Some(PublishRemote),
                    false,
                    Some(true),
                ),
                (
                    &[("base-remote", "project")],
                    &[("publish-remote", "personal")],
                    "personal",
                    "project",
                    Some(PublishRemote),
                    true,
                    Some(false),
                ),
                (
                    &[],
                    &[("remote", "team")],
                    "team",
                    "team",
                    Some(Remote),
                    true,
                    None,
                ),
            ];
            for (shared_format, shared_file) in CONFIG_FILES.iter().enumerate() {
                for (local_format, local_file) in CONFIG_FILES
                    .iter()
                    .copied()
                    .chain(std::iter::once(settings::SETTINGS_FILE))
                    .enumerate()
                {
                    for &(shared, local, publication, base, key, publication_local, base_local) in
                        cases
                    {
                        write(repo.path(), shared_format, shared);
                        if local_format == CONFIG_FILES.len() {
                            fs::write(
                                common.join(local_file),
                                local
                                    .iter()
                                    .map(|(k, v)| format!("{k} = {v}\n"))
                                    .collect::<String>(),
                            )
                            .unwrap();
                        } else {
                            write(&common, local_format, local);
                        }
                        let before = snapshot(root);
                        let resolved = load(repo.path()).unwrap();
                        assert_eq!(snapshot(root), before);
                        assert_eq!(resolved.publish_remote, publication);
                        assert_eq!(resolved.base_remote, base);
                        assert_eq!(resolved.sources.publication_key, key);
                        assert_eq!(
                            resolved.sources.publish_remote,
                            if key.is_none() {
                                SettingSource::BuiltIn
                            } else if publication_local {
                                SettingSource::Local(common.join(local_file))
                            } else {
                                SettingSource::Shared(repo.path().join(shared_file))
                            }
                        );
                        assert_eq!(
                            resolved.sources.base_remote,
                            base_local.map(|local| if local {
                                SettingSource::Local(common.join(local_file))
                            } else {
                                SettingSource::Shared(repo.path().join(shared_file))
                            })
                        );
                        fs::remove_file(common.join(local_file)).unwrap();
                        fs::remove_file(repo.path().join(shared_file)).unwrap();
                    }
                }
            }
        },
    );
}

#[test]
fn remote_role_values_and_syntax_are_strict_even_when_overridden() {
    isolated(
        "remote_role_values_and_syntax_are_strict_even_when_overridden",
        true,
        |root| {
            let repo = Repository::new(root);
            let common = repo.path().join(".git");
            write(
                &common,
                3,
                &[
                    ("remote", "valid"),
                    ("publish-remote", "valid"),
                    ("base-remote", "valid"),
                ],
            );
            for (format, file) in CONFIG_FILES.iter().enumerate() {
                for key in ["remote", "publish-remote", "base-remote"] {
                    for value in [
                        "",
                        "-option",
                        ".",
                        "../repo",
                        "with space",
                        "https://example.test/repo",
                        "bad\0name",
                    ] {
                        write(
                            repo.path(),
                            format,
                            &[
                                (key, value),
                                (
                                    if key == "remote" {
                                        "publish-remote"
                                    } else {
                                        "base"
                                    },
                                    "valid",
                                ),
                            ],
                        );
                        let error = load(repo.path()).unwrap_err().to_string();
                        assert!(
                            error.contains(&repo.path().join(file).display().to_string()),
                            "{error}"
                        );
                        assert!(error.contains(&format!("invalid {key} name")), "{error}");
                    }
                    for (case, malformed) in [
                        config(format, &[(key, "a"), (key, "b")]),
                        match format {
                            0 => format!("{key} = 42"),
                            1 | 2 => format!("{key}: 42"),
                            _ => format!("{{\"{key}\":42}}"),
                        },
                        match format {
                            0 => format!("{key} ="),
                            1 | 2 => format!("{key}: ["),
                            _ => format!("{{\"{key}\":"),
                        },
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        fs::write(repo.path().join(file), malformed).unwrap();
                        let error = load(repo.path()).unwrap_err().to_string();
                        if case == 0 {
                            assert!(error.contains("duplicate"), "{error}");
                        }
                        assert!(
                            error.contains(&repo.path().join(file).display().to_string()),
                            "{error}"
                        );
                    }
                }
                fs::remove_file(repo.path().join(file)).unwrap();
            }
            fs::remove_file(common.join(CONFIG_FILES[3])).unwrap();
            write(
                repo.path(),
                0,
                &[("publish-remote", "valid"), ("base-remote", "valid")],
            );
            for key in ["remote", "publish-remote", "base-remote"] {
                for contents in [
                    format!("{key} = bad name"),
                    format!("{key} = a\n{key} = b"),
                    format!("{key} ="),
                    key.into(),
                ] {
                    fs::write(common.join(settings::SETTINGS_FILE), contents).unwrap();
                    assert!(
                        load(repo.path())
                            .unwrap_err()
                            .to_string()
                            .contains("oneflow.conf")
                    );
                }
            }
        },
    );
}

#[test]
fn remote_role_changes_revalidate_workflow_apis() {
    isolated(
        "remote_role_changes_revalidate_workflow_apis",
        true,
        |root| {
            use git_oneflow::finish;
            let repo = Repository::new(root);
            fs::write(repo.path().join("tracked"), "initial").unwrap();
            commit(&repo);
            repo.success(&["switch", "-c", "work"]);
            let destination = root.join("publication.git");
            assert!(
                git::run(
                    root,
                    ["init".as_ref(), "--bare".as_ref(), destination.as_os_str()]
                )
                .unwrap()
                .status
                .success()
            );
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                destination.as_os_str(),
            ]);
            repo.success(&[
                "remote".as_ref(),
                "add".as_ref(),
                "project".as_ref(),
                destination.as_os_str(),
            ]);
            let local = repo.path().join(".git");
            write(&local, 0, &[("remote", "origin")]);
            let start_plan = start::prepare(repo.path(), "new-work").unwrap();
            let save_plan = save::assess(
                repo.path(),
                &Request {
                    action: Some(SaveAction::PublishOnly),
                    message: None,
                },
            )
            .unwrap();
            let finish_plan =
                finish::assess(repo.path(), &finish::Request::default(), None).unwrap();

            // Equal effective values still preserve alias and explicit-base provenance.
            write(&local, 0, &[("publish-remote", "origin")]);
            assert!(save::revalidate(repo.path(), &save_plan).is_err());
            assert!(finish::revalidate(repo.path(), &finish_plan, None).is_err());
            assert_ne!(start::prepare(repo.path(), "new-work").unwrap(), start_plan);
            write(
                &local,
                0,
                &[("remote", "origin"), ("base-remote", "origin")],
            );
            assert!(save::revalidate(repo.path(), &save_plan).is_err());
            assert!(finish::revalidate(repo.path(), &finish_plan, None).is_err());

            write(
                &local,
                0,
                &[("publish-remote", "origin"), ("base-remote", "project")],
            );
            let before = snapshot(root);
            assert!(start::prepare(repo.path(), "new-work").is_ok());
            assert!(save::assess(repo.path(), &Request::default()).is_ok());
            assert!(finish::assess(repo.path(), &finish::Request::default(), None).is_ok());
            let failure = start::execute(repo.path(), &start_plan).unwrap_err();
            assert!(failure.effects.is_empty());
            let report = execution::execute(repo.path(), &save_plan);
            assert!(report.error.is_some());
            assert!(report.effects.is_empty());
            assert!(report.commands.is_empty());
            assert!(report.publication.is_none());
            let report = save::publication::publish(repo.path(), &save_plan);
            assert!(report.error.is_some());
            assert!(report.push.is_none());
            assert!(report.tracking.writes.is_empty());
            for report in [
                finish::preparation::refresh(repo.path(), &finish_plan),
                finish::preparation::prepare(repo.path(), &finish_plan),
            ] {
                assert!(report.error.is_some());
                assert!(report.commands.is_empty());
                assert!(report.effects.is_empty());
            }
            let report = finish::publication::publish(repo.path(), &finish_plan);
            assert!(report.error.is_some());
            assert!(report.destinations.iter().all(|d| d.commands.is_empty()));
            assert!(report.tracking.writes.is_empty());
            assert_eq!(snapshot(root), before);
        },
    );
}
