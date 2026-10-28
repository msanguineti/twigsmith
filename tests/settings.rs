#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use preservation::snapshot;
use std::{fs, path::Path};
use support::{Repository, isolated};
use twigsmith::{
    git, repository,
    save::{self, ActionSource, Request, SaveError, execution},
    settings::{
        self, CONFIG_FILES, FinishRewrite, FinishUpdate, HistoryEdit, SavePlan, SettingSource,
        Settings, SettingsError,
    },
    start,
};

fn save_plan(value: &str) -> SavePlan {
    SavePlan::parse(&value.split(',').map(str::to_owned).collect::<Vec<_>>()).unwrap()
}
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
    fn value(key: &str, value: &str) -> String {
        if key == "save-plan" {
            serde_json::to_string(&value.split(',').collect::<Vec<_>>()).unwrap()
        } else {
            serde_json::to_string(value).unwrap()
        }
    }
    match format {
        0 => pairs
            .iter()
            .map(|(k, v)| format!("{k} = {}\n", value(k, v)))
            .collect(),
        1 | 2 => {
            if pairs.is_empty() {
                "{}".into()
            } else {
                pairs
                    .iter()
                    .map(|(k, v)| format!("{k}: {}\n", value(k, v)))
                    .collect()
            }
        }
        3 => format!(
            "{{{}}}",
            pairs
                .iter()
                .map(|(k, v)| format!("{k:?}: {}", value(k, v)))
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
                        ("publish-remote", "team/upstream"),
                        ("base", "Feature/École"),
                        ("save-plan", "stash:all"),
                    ],
                );
                for (local, local_file) in CONFIG_FILES.iter().enumerate() {
                    let common = repo.path().join(".git");
                    write(
                        &common,
                        local,
                        &[
                            ("publish-remote", "private"),
                            ("save-plan", "commit:staged,commit:unstaged"),
                        ],
                    );
                    let before = snapshot(root);
                    let resolved = load(repo.path()).unwrap();
                    assert_eq!(snapshot(root), before);
                    assert_eq!(resolved.publish_remote, "private");
                    assert_eq!(resolved.base, "Feature/École");
                    assert_eq!(
                        resolved.save_plan,
                        SavePlan::parse(&["commit:staged".into(), "commit:unstaged".into()])
                            .unwrap()
                    );
                    assert_eq!(
                        resolved.sources.publish_remote,
                        SettingSource::Local(common.join(local_file))
                    );
                    assert_eq!(resolved.sources.save_plan, resolved.sources.publish_remote);
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
                assert_eq!(resolved.save_plan, SavePlan::Ask);
                assert_eq!(resolved.sources.publish_remote, SettingSource::BuiltIn);
                assert_eq!(resolved.sources.save_plan, SettingSource::BuiltIn);
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
                    ("publish-remote", "origin"),
                    ("base", "main"),
                    ("save-plan", "commit:staged,commit:unstaged"),
                ],
            );
            let invalid = [
                vec![
                    "publish-remote =",
                    "other = 'x'",
                    "base='a'\nbase='a'",
                    "publish-remote='a'\npublish-remote='b'",
                    "save-plan=['commit:all']\nsave-plan=['commit:all']",
                    "publish-remote=42",
                    "base=true",
                    "save-plan=42",
                    "base={nested='x'}",
                    "base=2026-10-08",
                    "[nested]\nbase='main'",
                ],
                vec![
                    "base: [",
                    "other: x",
                    "base: a\nbase: a",
                    "publish-remote: a\npublish-remote: b",
                    "save-plan: [commit:all]\nsave-plan: [commit:all]",
                    "publish-remote: 42",
                    "base: true",
                    "save-plan: 42",
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
                    "publish-remote: a\npublish-remote: b",
                    "save-plan: [commit:all]\nsave-plan: [commit:all]",
                    "publish-remote: 42",
                    "base: true",
                    "save-plan: 42",
                    "base: {nested: x}",
                    "base: null",
                ],
                vec![
                    "{",
                    "{\"other\":\"x\"}",
                    "{\"base\":\"a\",\"base\":\"a\"}",
                    "{\"publish-remote\":\"a\",\"publish-remote\":\"b\"}",
                    "{\"save-plan\":[\"commit:all\"],\"save-plan\":[\"commit:all\"]}",
                    "{\"publish-remote\":42}",
                    "{\"base\":true}",
                    "{\"save-plan\":42}",
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
                    ("publish-remote", ""),
                    ("publish-remote", "-option"),
                    ("publish-remote", "."),
                    ("publish-remote", "../repo"),
                    ("base", ""),
                    ("base", "HEAD"),
                    ("base", "@{-1}"),
                    ("base", "a..b"),
                    ("base", "-branch"),
                    ("base", "bad\0name"),
                    ("save-plan", ""),
                    ("save-plan", "WIP"),
                    ("save-plan", "commit"),
                ] {
                    write(repo.path(), format, &[(key, value)]);
                    assert!(load(repo.path()).is_err(), "{format} {key} {value:?}");
                }
                for action in [
                    "ask",
                    "publish-only",
                    "commit:staged",
                    "commit:staged,commit:unstaged",
                    "stash:all",
                ] {
                    write(repo.path(), format, &[("save-plan", action)]);
                    assert!(load(repo.path()).is_ok());
                }
                fs::remove_file(path).unwrap();
            }
            // YAML 1.2 boolean spelling leaves these valid Git names as strings.
            fs::write(
                repo.path().join(CONFIG_FILES[1]),
                "publish-remote: on\nbase: yes\nsave-plan: [ask]\n",
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
                "{\"save-plan\":null}",
            )
            .unwrap();
            assert!(matches!(
                save::assess(
                    repo.path(),
                    &Request {
                        plan: Some(SavePlan::PublishOnly),
                        message: None,
                        stash_message: None,
                    }
                ),
                Err(SaveError::Settings(_))
            ));
        },
    );
}

#[test]
fn unsupported_publication_keys_fail_in_every_format_and_layer() {
    isolated(
        "unsupported_publication_keys_fail_in_every_format_and_layer",
        true,
        |root| {
            let repo = Repository::new(root);
            let common = repo.path().join(".git");
            for local in [false, true] {
                let directory = if local { &common } else { repo.path() };
                for (format, filename) in CONFIG_FILES.iter().enumerate() {
                    for canonical in [false, true] {
                        let mut pairs = vec![("remote", "private")];
                        if canonical {
                            pairs.push(("publish-remote", "origin"));
                        }
                        write(directory, format, &pairs);
                        if !local {
                            write(&common, format, &[("publish-remote", "origin")]);
                        }
                        let before = snapshot(root);
                        let error = load(repo.path()).unwrap_err().to_string();
                        assert!(error.contains("unknown key \"remote\""), "{error}");
                        assert!(error.contains(filename), "{error}");
                        assert_eq!(snapshot(root), before);
                        fs::remove_file(directory.join(filename)).unwrap();
                        if !local {
                            fs::remove_file(common.join(filename)).unwrap();
                        }
                    }
                }
            }
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
fn discovery_uses_git_roots_and_common_dir_without_inheritance() {
    isolated(
        "discovery_uses_git_roots_and_common_dir_without_inheritance",
        true,
        |root| {
            let repo = Repository::new(root);
            // Malformed parent/home files are ignored.
            for (format, _) in CONFIG_FILES.iter().enumerate() {
                fs::write(root.join(CONFIG_FILES[format]), "malformed").unwrap();
                fs::write(root.join("home").join(CONFIG_FILES[format]), "malformed").unwrap();
            }
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
            write(&context.common_dir, 1, &[("publish-remote", "private")]);
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
                for name in CONFIG_FILES {
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
                        ("publish-remote", "team"),
                        ("base", "trunk"),
                        ("save-plan", "stash:all"),
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
                    &[("save-plan", "commit:staged,commit:unstaged")],
                );
                fs::write(local.path().join("tracked"), "progress").unwrap();
                let configured = save::assess(local.path(), &Request::default()).unwrap();
                assert_eq!(configured.plan, save_plan("commit:staged,commit:unstaged"));
                assert_eq!(
                    configured.action_source,
                    ActionSource::Setting(configured.settings.sources.save_plan.clone())
                );
                for flag in [
                    SavePlan::PublishOnly,
                    save_plan("stash:all"),
                    save_plan("commit:staged,commit:unstaged"),
                ] {
                    let explicit = save::assess(
                        local.path(),
                        &Request {
                            plan: Some(flag.clone()),
                            message: None,
                            stash_message: None,
                        },
                    )
                    .unwrap();
                    assert_eq!(explicit.plan, flag);
                    assert_eq!(explicit.action_source, ActionSource::ExplicitSelection);
                }
                local.success(&["add", "tracked"]);
                let staged = save::assess(
                    local.path(),
                    &Request {
                        plan: Some(save_plan("commit:staged")),
                        message: Some("exact".into()),
                        stash_message: None,
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
                    b"Save work [staged]\n"
                );
                assert_eq!(
                    save::assess(local.path(), &Request::default())
                        .unwrap()
                        .action_source,
                    ActionSource::Setting(load(local.path()).unwrap().sources.save_plan)
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
            write(
                repo.path(),
                0,
                &[("save-plan", "commit:staged,commit:unstaged")],
            );
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
                    ("publish-remote", "missing"),
                    ("base", "work"),
                    ("save-plan", "stash:all"),
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
                        write(
                            repo.path(),
                            0,
                            &[("save-plan", "commit:staged,commit:unstaged")],
                        );
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
            write(repo.path(), 0, &[("save-plan", "ask")]);
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
            write(repo.path(), 0, &[("save-plan", "stash:all")]);
            fs::write(repo.path().join("tracked"), "progress").unwrap();
            let plan = save::assess(repo.path(), &Request::default()).unwrap();
            assert_eq!(plan.plan, save_plan("stash:all"));
            let report = execution::execute(repo.path(), &plan);
            assert!(report.error.is_some(), "{report:?}");
            assert!(report.publication.is_none());
            assert!(
                report.commands.iter().all(|command| command
                    .result
                    .as_ref()
                    .unwrap()
                    .status
                    .success())
            );
            // Applying the saved result restores the earlier shared settings.
            // The context guard stops publication and retains both the requested
            // stash identity and a recovery artifact for any incomplete reconcile.
            let stash = report
                .effects
                .iter()
                .find_map(|effect| match effect {
                    execution::Effect::LocalStash { commit, .. } => Some(commit),
                    _ => None,
                })
                .expect("verified saved stash");
            assert_eq!(report.after.stash.as_ref().unwrap().as_ref(), Some(stash));
            assert!(
                report
                    .recovery
                    .as_ref()
                    .is_some_and(|recovery| recovery.retained)
            );
            assert_eq!(load(repo.path()).unwrap().save_plan, SavePlan::Ask);
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
fn finish_settings_sparse_layers_and_sources() {
    isolated("finish_settings_sparse_layers_and_sources", true, |root| {
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
    });
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
        },
    );
}

#[test]
fn remote_roles_resolve_sparse_layers_and_derive_base_across_all_formats() {
    isolated(
        "remote_roles_resolve_sparse_layers_and_derive_base_across_all_formats",
        true,
        |root| {
            let repo = Repository::new(root);
            let common = repo.path().join(".git");
            type Case<'a> = (
                &'a [(&'a str, &'a str)],
                &'a [(&'a str, &'a str)],
                &'a str,
                &'a str,
                Option<bool>,
                Option<bool>,
            );
            let cases: &[Case<'_>] = &[
                (&[], &[], "origin", "origin", None, None),
                (
                    &[("base-remote", "project")],
                    &[],
                    "origin",
                    "project",
                    None,
                    Some(false),
                ),
                (
                    &[("publish-remote", "personal")],
                    &[],
                    "personal",
                    "personal",
                    Some(false),
                    None,
                ),
                (
                    &[("publish-remote", "personal"), ("base-remote", "project")],
                    &[],
                    "personal",
                    "project",
                    Some(false),
                    Some(false),
                ),
                (
                    &[("publish-remote", "shared")],
                    &[("publish-remote", "local")],
                    "local",
                    "local",
                    Some(true),
                    None,
                ),
                (
                    &[("publish-remote", "shared"), ("base-remote", "project")],
                    &[("publish-remote", "personal")],
                    "personal",
                    "project",
                    Some(true),
                    Some(false),
                ),
                (
                    &[("publish-remote", "shared"), ("base-remote", "project")],
                    &[("base-remote", "local-base")],
                    "shared",
                    "local-base",
                    Some(false),
                    Some(true),
                ),
                (
                    &[("base-remote", "project")],
                    &[("publish-remote", "personal")],
                    "personal",
                    "project",
                    Some(true),
                    Some(false),
                ),
                (
                    &[],
                    &[("publish-remote", "team")],
                    "team",
                    "team",
                    Some(true),
                    None,
                ),
                (
                    &[],
                    &[("publish-remote", "team"), ("base-remote", "team")],
                    "team",
                    "team",
                    Some(true),
                    Some(true),
                ),
            ];
            for (shared_format, shared_file) in CONFIG_FILES.iter().enumerate() {
                for (local_format, local_file) in CONFIG_FILES.iter().enumerate() {
                    for &(shared, local, publication, base, publication_local, base_local) in cases
                    {
                        write(repo.path(), shared_format, shared);
                        write(&common, local_format, local);
                        let before = snapshot(root);
                        let resolved = load(repo.path()).unwrap();
                        assert_eq!(snapshot(root), before);
                        assert_eq!(resolved.publish_remote, publication);
                        assert_eq!(resolved.base_remote, base);
                        let source = |local| {
                            if local {
                                SettingSource::Local(common.join(local_file))
                            } else {
                                SettingSource::Shared(repo.path().join(shared_file))
                            }
                        };
                        assert_eq!(
                            resolved.sources.publish_remote,
                            publication_local
                                .map(source)
                                .unwrap_or(SettingSource::BuiltIn)
                        );
                        assert_eq!(resolved.sources.base_remote, base_local.map(source));
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
                &[("publish-remote", "valid"), ("base-remote", "valid")],
            );
            for (format, file) in CONFIG_FILES.iter().enumerate() {
                for key in ["publish-remote", "base-remote"] {
                    for value in [
                        "",
                        "-option",
                        ".",
                        "../repo",
                        "with space",
                        "https://example.test/repo",
                        "bad\0name",
                    ] {
                        write(repo.path(), format, &[(key, value), ("base", "valid")]);
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
        },
    );
}

#[test]
fn remote_role_changes_revalidate_workflow_apis() {
    isolated(
        "remote_role_changes_revalidate_workflow_apis",
        true,
        |root| {
            use twigsmith::finish;
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
            write(&local, 0, &[("publish-remote", "origin")]);
            let start_plan = start::prepare(repo.path(), "new-work").unwrap();
            let save_plan = save::assess(
                repo.path(),
                &Request {
                    plan: Some(SavePlan::PublishOnly),
                    message: None,
                    stash_message: None,
                },
            )
            .unwrap();
            let finish_plan =
                finish::assess(repo.path(), &finish::Request::default(), None).unwrap();

            // Equal values still preserve file and explicit-base provenance.
            fs::remove_file(local.join(CONFIG_FILES[0])).unwrap();
            write(&local, 3, &[("publish-remote", "origin")]);
            assert!(save::revalidate(repo.path(), &save_plan).is_err());
            assert!(finish::revalidate(repo.path(), &finish_plan, None).is_err());
            assert_ne!(start::prepare(repo.path(), "new-work").unwrap(), start_plan);
            fs::remove_file(local.join(CONFIG_FILES[3])).unwrap();
            write(
                &local,
                0,
                &[("publish-remote", "origin"), ("base-remote", "origin")],
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

fn completion_values(settings: &Settings) -> [(&str, &SettingSource); 5] {
    [
        (settings.finish_mode.as_str(), &settings.sources.finish_mode),
        (
            settings.finish_integration.as_str(),
            &settings.sources.finish_integration,
        ),
        (
            settings.finish_message.as_str(),
            &settings.sources.finish_message,
        ),
        (
            settings.finish_checkout.as_str(),
            &settings.sources.finish_checkout,
        ),
        (
            settings.finish_cleanup.as_str(),
            &settings.sources.finish_cleanup,
        ),
    ]
}
const COMPLETION_SETTINGS: [(&str, &[&str]); 5] = [
    ("finish-mode", &["ask", "review", "integrate"]),
    ("finish-integration", &["ask", "ff-only", "merge", "squash"]),
    ("finish-message", &["ask", "auto"]),
    ("finish-checkout", &["target", "work"]),
    ("finish-cleanup", &["ask", "keep", "local", "both"]),
];

#[test]
fn completion_settings_round_trip_layer_precedence_and_provenance() {
    isolated(
        "completion_settings_round_trip_layer_precedence_and_provenance",
        true,
        |root| {
            let repo = Repository::new(root);
            let defaults = load(repo.path()).unwrap();
            for ((value, source), expected) in completion_values(&defaults)
                .iter()
                .zip(["ask", "ask", "ask", "target", "ask"])
            {
                assert_eq!(*value, expected);
                assert_eq!(**source, SettingSource::BuiltIn);
            }
            for (format, filename) in CONFIG_FILES.iter().enumerate() {
                for (index, (key, values)) in COMPLETION_SETTINGS.iter().enumerate() {
                    for value in *values {
                        write(repo.path(), format, &[(key, value)]);
                        let loaded = load(repo.path()).unwrap();
                        assert_eq!(completion_values(&loaded)[index].0, *value);
                        // General rendering preserves inactive settings too.
                        let rendered = twigsmith::config::render(
                            match format {
                                0 => twigsmith::config::Format::Toml,
                                1 | 2 => twigsmith::config::Format::Yaml,
                                _ => twigsmith::config::Format::Json,
                            },
                            &loaded,
                        );
                        fs::write(repo.path().join(filename), rendered).unwrap();
                        assert_eq!(
                            completion_values(&load(repo.path()).unwrap()).map(|(value, _)| value),
                            completion_values(&loaded).map(|(value, _)| value)
                        );
                    }
                }
                fs::remove_file(repo.path().join(filename)).unwrap();
            }
            for (shared, shared_file) in CONFIG_FILES.iter().enumerate() {
                let pairs: Vec<_> = COMPLETION_SETTINGS
                    .iter()
                    .map(|(key, values)| (*key, values[1]))
                    .collect();
                write(repo.path(), shared, &pairs);
                for (local, local_file) in CONFIG_FILES.iter().enumerate() {
                    for (index, (key, values)) in COMPLETION_SETTINGS.iter().enumerate() {
                        let common = repo.path().join(".git");
                        write(&common, local, &[(key, values[0])]);
                        let before = snapshot(root);
                        let loaded = load(repo.path()).unwrap();
                        assert_eq!(snapshot(root), before);
                        for (i, (value, source)) in completion_values(&loaded).iter().enumerate() {
                            assert_eq!(*value, if i == index { values[0] } else { pairs[i].1 });
                            assert_eq!(
                                **source,
                                if i == index {
                                    SettingSource::Local(common.join(local_file))
                                } else {
                                    SettingSource::Shared(repo.path().join(shared_file))
                                }
                            );
                        }
                        fs::remove_file(common.join(local_file)).unwrap();
                    }
                }
                fs::remove_file(repo.path().join(shared_file)).unwrap();
            }
        },
    );
}

#[test]
fn completion_settings_validate_inactive_overridden_duplicate_and_typed_fields() {
    isolated(
        "completion_settings_validate_inactive_overridden_duplicate_and_typed_fields",
        true,
        |root| {
            let repo = Repository::new(root);
            for (format, filename) in CONFIG_FILES.iter().enumerate() {
                for (key, values) in COMPLETION_SETTINGS {
                    for local in [false, true] {
                        let directory = if local {
                            repo.path().join(".git")
                        } else {
                            repo.path().to_owned()
                        };
                        // All integration policies must validate even in review mode,
                        // and valid local overrides cannot hide invalid shared values.
                        for invalid_value in ["", "ASK", "unknown", "true", "review # comment"] {
                            write(&directory, format, &[(key, invalid_value)]);
                            if !local {
                                let overrides = if key == "finish-mode" {
                                    vec![("finish-mode", "review")]
                                } else {
                                    vec![("finish-mode", "review"), (key, values[0])]
                                };
                                write(&repo.path().join(".git"), format, &overrides);
                            }
                            let before = snapshot(root);
                            let error = load(repo.path()).unwrap_err().to_string();
                            assert!(error.contains(key) && error.contains(filename), "{error}");
                            assert_eq!(snapshot(root), before);
                            fs::remove_file(directory.join(filename)).unwrap();
                            if !local {
                                fs::remove_file(repo.path().join(".git").join(filename)).unwrap();
                            }
                        }
                        for invalid in [
                            config(format, &[(key, values[0]), (key, values[0])]),
                            match format {
                                0 => format!("{key}=false"),
                                1 | 2 => format!("{key}: false"),
                                _ => format!("{{{key:?}:false}}"),
                            },
                            match format {
                                0 => format!("{key}=[]"),
                                1 | 2 => format!("{key}: null"),
                                _ => format!("{{{key:?}:null}}"),
                            },
                        ] {
                            fs::write(directory.join(filename), &invalid).unwrap();
                            assert!(load(repo.path()).is_err(), "{invalid}");
                            fs::remove_file(directory.join(filename)).unwrap();
                        }
                    }
                }
            }
        },
    );
}

#[test]
fn completion_settings_and_sources_revalidate_workflows_and_guided_review() {
    isolated(
        "completion_settings_and_sources_revalidate_workflows_and_guided_review",
        true,
        |root| {
            for (key, values) in COMPLETION_SETTINGS {
                for value in [values[0], values[1]] {
                    let repo = Repository::new(root);
                    fs::write(repo.path().join("tracked"), "initial").unwrap();
                    commit(&repo);
                    repo.success(&["switch", "-c", "work"]);
                    repo.success(&["remote", "add", "origin", "/offline"]);
                    let start = start::prepare(repo.path(), "another").unwrap();
                    let save = save::assess(repo.path(), &Request::default()).unwrap();
                    let finish =
                        twigsmith::finish::assess(repo.path(), &Default::default(), None).unwrap();
                    let setup = twigsmith::config::prepare_guided(
                        repo.path(),
                        twigsmith::config::Format::Toml,
                    )
                    .unwrap();
                    write(&repo.path().join(".git"), 0, &[(key, value)]);
                    let before = snapshot(root);
                    assert!(start::execute(repo.path(), &start).is_err());
                    assert!(save::revalidate(repo.path(), &save).is_err());
                    assert!(twigsmith::finish::revalidate(repo.path(), &finish, None).is_err());
                    assert!(twigsmith::config::revalidate_review(repo.path(), &setup).is_err());
                    assert_eq!(snapshot(root), before);
                }
            }
        },
    );
}

#[test]
fn workstream_bundle_round_trips_and_sparse_private_routes_preserve_provenance() {
    use twigsmith::{config::Format, settings::WorkstreamKind};
    isolated(
        "workstream_bundle_round_trips_and_sparse_private_routes_preserve_provenance",
        true,
        |root| {
            let repo = Repository::new(root);
            let mut proposed = Settings::default();
            settings::oneflow_bundle(&mut proposed, Some("released".into()));
            for (format, filename) in [
                (Format::Toml, CONFIG_FILES[0]),
                (Format::Yaml, CONFIG_FILES[1]),
                (Format::Json, CONFIG_FILES[3]),
            ] {
                let shared = repo.path().join(filename);
                fs::write(&shared, twigsmith::config::render(format, &proposed)).unwrap();
                let private = repo.path().join(".git/twigsmith.config.json");
                fs::write(&private, r#"{"workstreams":{"release":{"publish-remote":"project"},"hotfix":{"active-release":"release/train"}}}"#).unwrap();
                let loaded = load(repo.path()).unwrap();
                let mut release = loaded.clone();
                settings::resolve_workstream(&mut release, WorkstreamKind::Release, None).unwrap();
                assert_eq!(release.publish_remote, "project");
                assert_eq!(release.base_remote, "origin");
                assert_eq!(release.base, "main");
                assert_eq!(release.finish_update, FinishUpdate::None);
                let policy = release.workstream.unwrap();
                assert!(policy.version_tagging);
                assert_eq!(policy.release_marker.as_deref(), Some("released"));
                assert_eq!(
                    release.sources.publish_remote,
                    SettingSource::Local(private.clone())
                );
                assert_eq!(
                    loaded.sources.workstreams["release.integration-target"],
                    SettingSource::Shared(shared.clone())
                );
                let mut hotfix = loaded.clone();
                settings::resolve_workstream(&mut hotfix, WorkstreamKind::Hotfix, None).unwrap();
                assert_eq!(hotfix.base, "release/train");
                assert_eq!(
                    hotfix.workstream.unwrap().active_release.as_deref(),
                    Some("release/train")
                );
                let mut ordinary = loaded;
                settings::resolve_workstream(
                    &mut ordinary,
                    WorkstreamKind::Ordinary,
                    Some("release/train"),
                )
                .unwrap();
                let policy = ordinary.workstream.unwrap();
                assert_eq!(policy.preparation_source, "release/train");
                assert_eq!(policy.integration_target, "release/train");
                assert!(!policy.version_tagging);
                assert_eq!(policy.creation_source, None);
                assert_eq!(policy.release_marker, None);
                fs::remove_file(shared).unwrap();
                fs::remove_file(private).unwrap();
            }
        },
    );
}

#[test]
fn workstream_configuration_rejects_typed_duplicate_and_inactive_invalid_values() {
    isolated(
        "workstream_configuration_rejects_typed_duplicate_and_inactive_invalid_values",
        true,
        |root| {
            let repo = Repository::new(root);
            let json = repo.path().join(CONFIG_FILES[3]);
            for content in [
                r#"{"workstreams":null}"#,
                r#"{"workstreams":[]}"#,
                r#"{"workstreams":{"unknown":{}}}"#,
                r#"{"workstreams":{"ordinary":{},"ordinary":{}}}"#,
                r#"{"workstreams":{"ordinary":[]}}"#,
                r#"{"workstreams":{"ordinary":{"creation-source":null}}}"#,
                r#"{"workstreams":{"ordinary":{"creation-source":1}}}"#,
                r#"{"workstreams":{"ordinary":{"publish-remote":"one","publish-remote":"two"}}}"#,
                r#"{"workstreams":{"release":{"version-tagging":"required","finish-integration":"squash"}}}"#,
                r#"{"workstreams":{"release":{"version-tagging":"required","finish-update":"merge"}}}"#,
                r#"{"workstreams":{"release":{"integration-target":"bad..branch"}}}"#,
                r#"{"workstreams":{"ordinary":{"preparation-source":"bad..branch"}}}"#,
                r#"{"workstreams":{"hotfix":{}}}"#,
                r#"{"workstreams":{"ordinary":{"version-tagging":"required"}}}"#,
            ] {
                fs::write(&json, content).unwrap();
                assert!(load(repo.path()).is_err(), "accepted {content}");
            }
            fs::remove_file(&json).unwrap();
            let yaml = repo.path().join(CONFIG_FILES[1]);
            for content in [
                "workstreams:\n  ordinary:\n    creation-source: 1\n",
                "workstreams:\n  ordinary: {}\n  ordinary: {}\n",
                "workstreams:\n  ordinary:\n    version-tagging: false\n",
            ] {
                fs::write(&yaml, content).unwrap();
                assert!(load(repo.path()).is_err(), "accepted {content}");
            }
            fs::remove_file(yaml).unwrap();
            fs::write(
                json,
                r#"{"workstreams":{"ordinary":{"preparation-source":"none","integration-target":"none"}}}"#,
            )
            .unwrap();
            let mut resolved = load(repo.path()).unwrap();
            settings::resolve_workstream(&mut resolved, settings::WorkstreamKind::Ordinary, None)
                .unwrap();
            let policy = resolved.workstream.unwrap();
            assert_eq!(policy.preparation_source, "none");
            assert_eq!(policy.integration_target, "none");
        },
    );
}

#[test]
fn hosted_workstream_routes_round_trip_and_private_fields_layer_independently() {
    use twigsmith::{
        config::Format,
        settings::{FinishCheckout, FinishCleanup, FinishMode, WorkstreamKind},
    };
    isolated(
        "hosted_workstream_routes_round_trip_and_private_fields_layer_independently",
        true,
        |root| {
            let repo = Repository::new(root);
            let mut proposed = Settings {
                publish_remote: "fork".into(),
                base_remote: "project".into(),
                ..Settings::default()
            };
            settings::oneflow_hosted_bundle(&mut proposed, Some("stable".into()));
            for (format, filename) in [
                (Format::Toml, CONFIG_FILES[0]),
                (Format::Yaml, CONFIG_FILES[1]),
                (Format::Json, CONFIG_FILES[3]),
            ] {
                let shared = repo.path().join(filename);
                fs::write(&shared, twigsmith::config::render(format, &proposed)).unwrap();
                let private = repo.path().join(".git/twigsmith.config.json");
                fs::write(&private, r#"{"redact-credentials":false,"workstreams":{"release":{"review-remote":"review","merge-back-branch":"review/version"},"hotfix":{"active-release":"release/train"}}}"#).unwrap();
                let loaded = load(repo.path()).unwrap();
                let mut release = loaded.clone();
                settings::resolve_workstream(&mut release, WorkstreamKind::Release, None).unwrap();
                assert_eq!(release.finish_mode, FinishMode::Review);
                assert_eq!(release.finish_update, FinishUpdate::Merge);
                assert_eq!(release.finish_checkout, FinishCheckout::Work);
                assert_eq!(release.finish_cleanup, FinishCleanup::Keep);
                let policy = release.workstream.unwrap();
                assert_eq!(policy.publish_remote, "project");
                assert_eq!(policy.base_remote, "project");
                assert_eq!(policy.review_remote, "review");
                assert_eq!(policy.merge_back_branch.as_deref(), Some("review/version"));
                assert_eq!(policy.release_marker.as_deref(), Some("stable"));
                assert!(policy.version_tagging);
                assert_eq!(
                    release.sources.workstreams["release.review-remote"],
                    SettingSource::Local(private.clone())
                );
                assert_eq!(
                    release.sources.workstreams["release.publish-remote"],
                    SettingSource::Shared(shared.clone())
                );
                let mut hotfix = loaded.clone();
                settings::resolve_workstream(&mut hotfix, WorkstreamKind::Hotfix, None).unwrap();
                let policy = hotfix.workstream.unwrap();
                assert_eq!(policy.integration_target, "release/train");
                assert_eq!(policy.review_remote, "fork");
                let mut ordinary = loaded;
                settings::resolve_workstream(
                    &mut ordinary,
                    WorkstreamKind::Ordinary,
                    Some("release/train"),
                )
                .unwrap();
                let policy = ordinary.workstream.unwrap();
                assert_eq!(policy.publish_remote, "fork");
                assert_eq!(policy.integration_target, "release/train");
                assert!(!policy.version_tagging);
                assert_eq!(policy.release_marker, None);
                fs::remove_file(shared).unwrap();
                fs::remove_file(private).unwrap();
            }
            // Custom configuration uses exactly the bundle schema; omitted review follows work publication.
            let mut custom = proposed;
            custom
                .workstreams
                .get_mut(&WorkstreamKind::Release)
                .unwrap()
                .review_remote = None;
            settings::resolve_workstream(&mut custom, WorkstreamKind::Release, None).unwrap();
            assert_eq!(custom.workstream.unwrap().review_remote, "project");
        },
    );
}

#[test]
fn hosted_settings_reject_invalid_fields_and_incompatible_effective_policies() {
    use twigsmith::{config::Format, settings::WorkstreamKind};
    isolated(
        "hosted_settings_reject_invalid_fields_and_incompatible_effective_policies",
        true,
        |root| {
            let repo = Repository::new(root);
            let shared = repo.path().join(CONFIG_FILES[3]);
            for content in [
                r#"{"workstreams":{"release":{"review-remote":null}}}"#,
                r#"{"workstreams":{"release":{"review-remote":1}}}"#,
                r#"{"workstreams":{"release":{"review-remote":"one","review-remote":"two"}}}"#,
                r#"{"workstreams":{"release":{"review-remote":"bad..remote"}}}"#,
                r#"{"workstreams":{"release":{"merge-back-branch":null}}}"#,
                r#"{"workstreams":{"release":{"merge-back-branch":true}}}"#,
                r#"{"workstreams":{"release":{"merge-back-branch":"one","merge-back-branch":"two"}}}"#,
                r#"{"workstreams":{"release":{"merge-back-branch":"bad..branch"}}}"#,
            ] {
                fs::write(&shared, content).unwrap();
                assert!(load(repo.path()).is_err(), "accepted {content}");
            }
            fs::remove_file(&shared).unwrap();
            for filename in [CONFIG_FILES[0], CONFIG_FILES[1], CONFIG_FILES[3]] {
                let file = repo.path().join(filename);
                for key in ["review-remote", "merge-back-branch"] {
                    for (scalar, duplicate) in [("true", false), ("1", false), ("\"first\"", true)]
                    {
                        let content = match filename {
                            "twigsmith.config.toml" => format!(
                                "[workstreams.release]\n{key}={scalar}\n{}",
                                if duplicate {
                                    format!("{key}='second'\n")
                                } else {
                                    String::new()
                                }
                            ),
                            "twigsmith.config.yaml" => format!(
                                "workstreams:\n  release:\n    {key}: {scalar}\n{}",
                                if duplicate {
                                    format!("    {key}: second\n")
                                } else {
                                    String::new()
                                }
                            ),
                            _ => format!(
                                "{{\"workstreams\":{{\"release\":{{\"{key}\":{scalar}{}}}}}}}",
                                if duplicate {
                                    format!(",\"{key}\":\"second\"")
                                } else {
                                    String::new()
                                }
                            ),
                        };
                        fs::write(&file, &content).unwrap();
                        assert!(load(repo.path()).is_err(), "accepted {filename}: {content}");
                    }
                }
                fs::remove_file(file).unwrap();
            }
            let mut proposed = Settings::default();
            settings::oneflow_hosted_bundle(&mut proposed, Some("stable".into()));
            for (key, value) in [
                ("finish-update", "rebase"),
                ("finish-integration", "squash"),
                ("finish-integration", "ff-only"),
                ("finish-checkout", "target"),
                ("finish-cleanup", "both"),
                ("finish-cleanup", "ask"),
                ("finish-rewrite", "allow"),
                ("merge-back-branch", "main"),
                ("merge-back-branch", "stable"),
            ] {
                fs::write(&shared, twigsmith::config::render(Format::Json, &proposed)).unwrap();
                let private = repo.path().join(".git/twigsmith.config.json");
                fs::write(
                    &private,
                    serde_json::json!({"workstreams":{"release":{key:value}}}).to_string(),
                )
                .unwrap();
                assert!(load(repo.path()).is_err(), "accepted {key}={value}");
                fs::remove_file(private).unwrap();
            }
            for update in ["none", "merge", "ask"] {
                let mut allowed = proposed.clone();
                allowed
                    .workstreams
                    .get_mut(&WorkstreamKind::Release)
                    .unwrap()
                    .finish_update = Some(update.into());
                fs::write(&shared, twigsmith::config::render(Format::Json, &allowed)).unwrap();
                assert!(load(repo.path()).is_ok(), "rejected {update}");
            }
            // Validate supplied layers even when a private layer would repair them.
            proposed
                .workstreams
                .get_mut(&WorkstreamKind::Release)
                .unwrap()
                .finish_cleanup = Some("both".into());
            fs::write(&shared, twigsmith::config::render(Format::Json, &proposed)).unwrap();
            fs::write(
                repo.path().join(".git/twigsmith.config.json"),
                r#"{"workstreams":{"release":{"finish-cleanup":"keep"}}}"#,
            )
            .unwrap();
            assert!(load(repo.path()).is_err());
        },
    );
}

#[test]
fn history_edit_settings_preserve_sparse_layers_and_validate_every_value() {
    isolated("history_edit_settings", true, |root| {
        let repo = Repository::new(root);
        let common = repo.path().join(".git");
        assert_eq!(load(repo.path()).unwrap().history_edit, HistoryEdit::None);
        for (format, filename) in CONFIG_FILES.iter().enumerate() {
            write(
                repo.path(),
                format,
                &[("finish-history-edit", "interactive")],
            );
            let loaded = load(repo.path()).unwrap();
            assert_eq!(loaded.history_edit, HistoryEdit::Interactive);
            assert_eq!(
                loaded.sources.history_edit,
                SettingSource::Shared(repo.path().join(filename))
            );
            write(&common, format, &[("finish-history-edit", "none")]);
            let before = snapshot(root);
            let loaded = load(repo.path()).unwrap();
            assert_eq!(snapshot(root), before);
            assert_eq!(loaded.history_edit, HistoryEdit::None);
            assert_eq!(
                loaded.sources.history_edit,
                SettingSource::Local(common.join(filename))
            );
            for invalid in ["", "ask", "INTERACTIVE", "true", "rebase"] {
                write(repo.path(), format, &[("finish-history-edit", invalid)]);
                let error = load(repo.path()).unwrap_err().to_string();
                assert!(error.contains("finish-history-edit"), "{error}");
            }
            fs::remove_file(repo.path().join(filename)).unwrap();
            fs::remove_file(common.join(filename)).unwrap();
            for malformed in match format {
                0 => vec![
                    "finish-history-edit=false",
                    "finish-history-edit='none'\nfinish-history-edit='none'",
                ],
                1 | 2 => vec![
                    "finish-history-edit: false",
                    "finish-history-edit: none\nfinish-history-edit: none",
                ],
                3 => vec![
                    r#"{"finish-history-edit":false}"#,
                    r#"{"finish-history-edit":"none","finish-history-edit":"none"}"#,
                ],
                _ => unreachable!(),
            } {
                fs::write(common.join(filename), malformed).unwrap();
                assert!(load(repo.path()).is_err(), "accepted {malformed}");
            }
            fs::remove_file(common.join(filename)).unwrap();
        }
        fs::write(repo.path().join(CONFIG_FILES[3]),
            r#"{"finish-history-edit":"none","workstreams":{"ordinary":{"finish-history-edit":"interactive"}}}"#).unwrap();
        let mut loaded = load(repo.path()).unwrap();
        settings::resolve_workstream(&mut loaded, settings::WorkstreamKind::Ordinary, None)
            .unwrap();
        assert_eq!(loaded.history_edit, HistoryEdit::Interactive);
        assert_eq!(
            loaded.sources.history_edit,
            SettingSource::Shared(repo.path().join(CONFIG_FILES[3]))
        );
        fs::write(repo.path().join(CONFIG_FILES[3]),
            r#"{"workstreams":{"release":{"version-tagging":"required","finish-mode":"integrate","finish-update":"none","finish-history-edit":"interactive"}}}"#).unwrap();
        fs::write(
            common.join(CONFIG_FILES[3]),
            r#"{"workstreams":{"release":{"finish-history-edit":"none"}}}"#,
        )
        .unwrap();
        let error = load(repo.path()).unwrap_err().to_string();
        assert!(error.contains("finish-history-edit = none"), "{error}");
    });
}

#[test]
fn ordered_tagged_routes_layer_replace_and_select_identity() {
    use settings::{VersionTagIdentity, WorkstreamKind};
    isolated(
        "ordered_tagged_routes_layer_replace_and_select_identity",
        true,
        |root| {
            let repo = Repository::new(root);
            let mut proposed = Settings::default();
            settings::oneflow_bundle(&mut proposed, None);
            let shared = repo.path().join(CONFIG_FILES[3]);
            fs::write(
                &shared,
                twigsmith::config::render(twigsmith::config::Format::Json, &proposed),
            )
            .unwrap();
            let private = repo.path().join(".git/twigsmith.config.json");
            fs::write(&private, r#"{"workstreams":{"release":{"integration-targets":["main","stable"],"version-tag-identity":"integrated-result"},"hotfix":{"active-release":"release/train","integration-targets":["release/train","main"]}}}"#).unwrap();
            let loaded = load(repo.path()).unwrap();
            let mut release = loaded.clone();
            settings::resolve_workstream(&mut release, WorkstreamKind::Release, None).unwrap();
            assert_eq!(release.sources.base, SettingSource::Local(private.clone()));
            assert!(
                !release
                    .sources
                    .workstreams
                    .contains_key("release.integration-target")
            );
            let policy = release.workstream.unwrap();
            assert_eq!(policy.integration_targets, ["main", "stable"]);
            assert_eq!(policy.integration_target, "main");
            assert_eq!(
                policy.version_tag_identity,
                VersionTagIdentity::IntegratedResult
            );
            let mut hotfix = loaded;
            settings::resolve_workstream(&mut hotfix, WorkstreamKind::Hotfix, None).unwrap();
            assert_eq!(
                hotfix.workstream.unwrap().integration_targets,
                ["release/train", "main"]
            );
            assert!(
                settings::resolve_workstream(
                    &mut load(repo.path()).unwrap(),
                    WorkstreamKind::Release,
                    Some("other")
                )
                .is_err()
            );
            fs::remove_file(&private).unwrap();
            proposed
                .workstreams
                .get_mut(&WorkstreamKind::Release)
                .unwrap()
                .integration_target = None;
            proposed
                .workstreams
                .get_mut(&WorkstreamKind::Release)
                .unwrap()
                .integration_targets = Some(vec!["main".into(), "stable".into()]);
            fs::write(
                &shared,
                twigsmith::config::render(twigsmith::config::Format::Json, &proposed),
            )
            .unwrap();
            fs::write(
                &private,
                r#"{"workstreams":{"release":{"integration-target":"other"}}}"#,
            )
            .unwrap();
            let mut release = load(repo.path()).unwrap();
            settings::resolve_workstream(&mut release, WorkstreamKind::Release, None).unwrap();
            assert_eq!(release.sources.base, SettingSource::Local(private.clone()));
            assert!(
                !release
                    .sources
                    .workstreams
                    .contains_key("release.integration-targets")
            );
            assert_eq!(release.workstream.unwrap().integration_targets, ["other"]);
        },
    );
}

#[test]
fn ordered_routes_reject_ambiguity_aliases_and_review_before_effects() {
    isolated(
        "ordered_routes_reject_ambiguity_aliases_and_review_before_effects",
        true,
        |root| {
            let repo = Repository::new(root);
            let path = repo.path().join(CONFIG_FILES[3]);
            for policy in [
                r#""integration-targets":[]"#,
                r#""integration-targets":["main","main"]"#,
                r#""integration-targets":["main","bad..branch"]"#,
                r#""integration-targets":null"#,
                r#""integration-targets":[1]"#,
                r#""integration-targets":["main"],"integration-target":"main""#,
                r#""integration-targets":["main","stable"],"version-tagging":"required","finish-mode":"integrate","finish-update":"none","release-marker":"stable""#,
                r#""integration-targets":["main"],"version-tagging":"disabled""#,
                r#""version-tag-identity":"target""#,
                r#""integration-targets":["main","stable"],"version-tagging":"required","finish-mode":"review""#,
            ] {
                fs::write(
                    &path,
                    format!("{{\"workstreams\":{{\"release\":{{{policy}}}}}}}"),
                )
                .unwrap();
                let before = snapshot(root);
                assert!(load(repo.path()).is_err(), "accepted {policy}");
                assert_eq!(snapshot(root), before);
            }
            fs::write(&path, r#"{"workstreams":{"hotfix":{"active-release":"release/train","integration-targets":["main","release/train"]}}}"#).unwrap();
            assert!(load(repo.path()).is_err());
            fs::write(
                &path,
                r#"{"workstreams":{"ordinary":{"integration-targets":["main"]}}}"#,
            )
            .unwrap();
            assert!(load(repo.path()).is_err());
            fs::remove_file(&path).unwrap();
            for filename in CONFIG_FILES {
                let path = repo.path().join(filename);
                let invalid_values = if filename.ends_with(".toml") {
                    vec![
                        "[workstreams.release]\nintegration-targets='main'",
                        "[workstreams.release]\nintegration-targets=[1]",
                        "[workstreams.release]\nintegration-targets=[true]",
                    ]
                } else if filename.ends_with(".json") {
                    vec![
                        r#"{"workstreams":{"release":{"integration-targets":null}}}"#,
                        r#"{"workstreams":{"release":{"integration-targets":"main"}}}"#,
                        r#"{"workstreams":{"release":{"integration-targets":[true]}}}"#,
                    ]
                } else {
                    vec![
                        "workstreams:\n  release:\n    integration-targets: null",
                        "workstreams:\n  release:\n    integration-targets:",
                        "workstreams:\n  release:\n    integration-targets: main",
                        "workstreams:\n  release:\n    integration-targets: [1]",
                        "workstreams:\n  release:\n    integration-targets: [true]",
                    ]
                };
                for value in invalid_values {
                    fs::write(&path, value).unwrap();
                    let before = snapshot(root);
                    assert!(load(repo.path()).is_err(), "accepted {filename}: {value}");
                    assert_eq!(snapshot(root), before);
                }
                fs::remove_file(path).unwrap();
            }
        },
    );
}

#[test]
fn tagged_direct_defaults_ignore_ordinary_update_preferences() {
    use settings::WorkstreamKind;
    isolated(
        "tagged_direct_defaults_ignore_ordinary_update_preferences",
        true,
        |root| {
            let repo = Repository::new(root);
            let path = repo.path().join(CONFIG_FILES[3]);
            fs::write(&path, r#"{"finish-update":"rebase","finish-mode":"integrate","finish-integration":"merge","workstreams":{"release":{"version-tagging":"required"}}}"#).unwrap();
            let mut release = load(repo.path()).unwrap();
            settings::resolve_workstream(&mut release, WorkstreamKind::Release, None).unwrap();
            assert_eq!(release.finish_update, FinishUpdate::None);
            assert_eq!(release.sources.finish_update, SettingSource::BuiltIn);
            fs::write(&path, r#"{"finish-mode":"integrate","workstreams":{"release":{"version-tagging":"required","finish-update":"rebase"}}}"#).unwrap();
            assert!(
                load(repo.path())
                    .unwrap_err()
                    .to_string()
                    .contains("--update rebase")
            );
        },
    );
}

#[test]
fn invalid_policy_correction_quotes_literal_git_configuration_key() {
    isolated(
        "invalid_policy_correction_quotes_literal_git_configuration_key",
        true,
        |root| {
            let repo = Repository::new(root);
            let branch = "release/quoted'$;branch";
            let key = format!("branch.{branch}.twigsmith-policy");
            repo.success(&["config", "--local", &key, "invalid-json"]);
            let repository = twigsmith::repository::discover(repo.path())
                .unwrap()
                .unwrap();
            let message = settings::load_for_branch(repo.path(), &repository, branch.as_bytes())
                .unwrap_err()
                .to_string();
            let correction = "git config --local --unset-all 'branch.release/quoted'\\''$;branch.twigsmith-policy'";
            assert!(message.contains(correction), "{message}");
            // The suggested command removes exactly this local key, with shell
            // metacharacters treated as literal branch bytes.
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(correction)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(settings::load_for_branch(repo.path(), &repository, branch.as_bytes()).is_ok());
        },
    );
}

#[test]
fn composable_save_settings_are_strict_equivalent_and_replace_whole_plans() {
    isolated("composable_save_settings", true, |root| {
        let repo = Repository::new(root);
        let shared =
            settings::SavePlan::parse(&["commit:staged".into(), "stash:unstaged".into()]).unwrap();
        for (format, filename) in CONFIG_FILES.iter().enumerate() {
            write(
                repo.path(),
                format,
                &[
                    ("save-plan", "commit:staged,stash:unstaged"),
                    ("save-message", "literal $x `text`"),
                    ("save-stash-message", "local label"),
                ],
            );
            let loaded = load(repo.path()).unwrap();
            assert_eq!(loaded.save_plan, shared);
            assert_eq!(loaded.save_message, "literal $x `text`");
            assert_eq!(loaded.save_stash_message, "local label");
            assert_eq!(
                loaded.sources.save_plan,
                SettingSource::Shared(repo.path().join(filename))
            );
            let local = repo.path().join(".git");
            write(
                &local,
                0,
                &[
                    ("save-plan", "stash:staged"),
                    ("save-stash-message", "private label"),
                ],
            );
            let loaded = load(repo.path()).unwrap();
            assert_eq!(
                loaded.save_plan,
                settings::SavePlan::parse(&["stash:staged".into()]).unwrap()
            );
            assert_eq!(loaded.save_message, "literal $x `text`");
            assert_eq!(loaded.save_stash_message, "private label");
            for invalid in [
                "commit:staged,commit:staged",
                "commit:all,stash:unstaged",
                "stash:all,stash:staged",
                "ask,commit:staged",
                "publish-only,stash:all",
                "commit:unknown",
                "unknown:all",
            ] {
                write(repo.path(), format, &[("save-plan", invalid)]);
                assert!(
                    load(repo.path()).is_err(),
                    "overridden invalid layer accepted: {invalid}"
                );
            }
            fs::remove_file(local.join(CONFIG_FILES[0])).unwrap();
            for key in ["save-message", "save-stash-message"] {
                for label in ["", "  \t", "bad\0label"] {
                    write(repo.path(), format, &[(key, label)]);
                    assert!(load(repo.path()).is_err(), "{key}: {label:?}");
                }
            }
            fs::remove_file(repo.path().join(filename)).unwrap();
        }
        for (filename, malformed) in [
            (CONFIG_FILES[0], "save-plan='commit:all'"),
            (CONFIG_FILES[0], "save-plan=[42]"),
            (CONFIG_FILES[0], "save-plan=['ask']\nsave-plan=['ask']"),
            (CONFIG_FILES[1], "save-plan: null"),
            (CONFIG_FILES[1], "save-plan: [true]"),
            (CONFIG_FILES[3], "{\"save-plan\":null}"),
            (
                CONFIG_FILES[3],
                "{\"save-plan\":[\"ask\"],\"save-plan\":[\"ask\"]}",
            ),
        ] {
            let path = repo.path().join(filename);
            fs::write(&path, malformed).unwrap();
            assert!(load(repo.path()).is_err(), "{malformed}");
            fs::remove_file(path).unwrap();
        }
        fs::write(repo.path().join(CONFIG_FILES[0]), "save-plan=[]").unwrap();
        assert_eq!(
            load(repo.path()).unwrap().save_plan,
            settings::SavePlan::Operations(vec![])
        );
    });
}
