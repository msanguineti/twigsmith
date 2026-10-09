#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use git_oneflow::{
    config::{self, ConfigError, Format},
    finish::{self, FinishError, preparation, publication},
    repository, save,
    settings::{
        self, CONFIG_FILES, FinishCheck, FinishRewrite, FinishUpdate, SettingSource, Settings,
    },
    start,
};
use preservation::snapshot;
use std::{fs, path::Path, process::Command};
use support::{Repository, isolated};

fn load(directory: &Path) -> Result<Settings, settings::SettingsError> {
    settings::load(
        directory,
        &repository::discover(directory).unwrap().unwrap(),
    )
}
fn format(index: usize) -> Format {
    match index {
        0 => Format::Toml,
        1 | 2 => Format::Yaml,
        3 => Format::Json,
        _ => unreachable!(),
    }
}
fn document(index: usize, toml: &str, json: &str) -> String {
    match index {
        0 => format!("finish-checks = {toml}\n"),
        1 | 2 => format!("finish-checks: {json}\n"),
        3 => format!("{{\"finish-checks\":{json}}}"),
        _ => unreachable!(),
    }
}
fn check(name: &str, args: &[&str]) -> FinishCheck {
    FinishCheck {
        name: name.into(),
        command: args.iter().map(|s| (*s).into()).collect(),
    }
}
fn write(root: &Path, index: usize, checks: &[FinishCheck]) {
    fs::write(
        root.join(CONFIG_FILES[index]),
        config::render(
            format(index),
            &Settings {
                finish_checks: checks.to_vec(),
                ..Settings::default()
            },
        ),
    )
    .unwrap();
}

#[test]
fn equivalent_formats_and_whole_list_layering_preserve_order_and_sources() {
    isolated(
        "equivalent_formats_and_whole_list_layering_preserve_order_and_sources",
        true,
        |root| {
            let repo = Repository::new(root);
            assert!(load(repo.path()).unwrap().finish_checks.is_empty());
            assert_eq!(
                load(repo.path()).unwrap().sources.finish_checks,
                SettingSource::BuiltIn
            );
            let shared_checks = vec![
                check("Tests", &["cargo", "test", "--locked"]),
                check(
                    "Custom validation",
                    &["sh", "-c", "npm run lint && npm run check"],
                ),
            ];
            // Independent handwritten representations exercise the public schema.
            let documents = [
                "[[finish-checks]]\nname='Tests'\ncommand=['cargo','test','--locked']\n[[finish-checks]]\nname='Custom validation'\ncommand=['sh','-c','npm run lint && npm run check']\n",
                "finish-checks:\n  - name: Tests\n    command: [cargo, test, --locked]\n  - name: Custom validation\n    command: [sh, '-c', 'npm run lint && npm run check']\n",
                "finish-checks:\n  - name: Tests\n    command: [cargo, test, --locked]\n  - name: Custom validation\n    command: [sh, '-c', 'npm run lint && npm run check']\n",
                r#"{"finish-checks":[{"name":"Tests","command":["cargo","test","--locked"]},{"name":"Custom validation","command":["sh","-c","npm run lint && npm run check"]}]}"#,
            ];
            for (shared, filename) in CONFIG_FILES.iter().enumerate() {
                fs::write(repo.path().join(filename), documents[shared]).unwrap();
                let shared_source = SettingSource::Shared(repo.path().join(filename));
                for (local, local_file) in CONFIG_FILES.iter().enumerate() {
                    let common = repo.path().join(".git");
                    let path = common.join(local_file);
                    fs::write(&path, if local == 0 { "" } else { "{}" }).unwrap();
                    let inherited = load(repo.path()).unwrap();
                    assert_eq!(inherited.finish_checks, shared_checks);
                    assert_eq!(inherited.sources.finish_checks, shared_source);
                    for replacement in [vec![check("Only local", &["missing-program", ""])], vec![]]
                    {
                        write(&common, local, &replacement);
                        let before = snapshot(root);
                        let replaced = load(repo.path()).unwrap();
                        assert_eq!(replaced.finish_checks, replacement);
                        assert_eq!(
                            replaced.sources.finish_checks,
                            SettingSource::Local(path.clone())
                        );
                        assert_eq!(snapshot(root), before);
                    }
                    fs::remove_file(path).unwrap();
                }
                assert_eq!(load(repo.path()).unwrap().finish_checks, shared_checks);
                fs::remove_file(repo.path().join(filename)).unwrap();
            }
        },
    );
}

#[test]
fn every_supplied_layer_rejects_malformed_checks_in_every_format() {
    isolated(
        "every_supplied_layer_rejects_malformed_checks_in_every_format",
        true,
        |root| {
            let repo = Repository::new(root);
            let common = repo.path().join(".git");
            // TOML and JSON/YAML flow values for the same invalid structures.
            let invalid = [
                ("'cargo test'", "\"cargo test\""),
                ("1", "1"),
                ("true", "true"),
                ("{}", "{}"),
                ("['cargo']", "[\"cargo\"]"),
                ("[[]]", "[[]]"),
                ("[['Tests',['cargo']]]", r#"[["Tests",["cargo"]]]"#),
                ("[{name='Tests'}]", r#"[{"name":"Tests"}]"#),
                ("[{command=['cargo']}]", r#"[{"command":["cargo"]}]"#),
                (
                    "[{name='',command=['cargo']}]",
                    r#"[{"name":"","command":["cargo"]}]"#,
                ),
                (
                    "[{name=' \t ',command=['cargo']}]",
                    r#"[{"name":" \t ","command":["cargo"]}]"#,
                ),
                (
                    "[{name=42,command=['cargo']}]",
                    r#"[{"name":42,"command":["cargo"]}]"#,
                ),
                (
                    "[{name=true,command=['cargo']}]",
                    r#"[{"name":true,"command":["cargo"]}]"#,
                ),
                (
                    "[{name=[],command=['cargo']}]",
                    r#"[{"name":[],"command":["cargo"]}]"#,
                ),
                (
                    "[{name='Tests',command=[]}]",
                    r#"[{"name":"Tests","command":[]}]"#,
                ),
                (
                    "[{name='Tests',command=['']}]",
                    r#"[{"name":"Tests","command":[""]}]"#,
                ),
                (
                    "[{name='Tests',command='cargo test'}]",
                    r#"[{"name":"Tests","command":"cargo test"}]"#,
                ),
                (
                    "[{name='Tests',command={}}]",
                    r#"[{"name":"Tests","command":{}}]"#,
                ),
                (
                    "[{name='Tests',command=[42]}]",
                    r#"[{"name":"Tests","command":[42]}]"#,
                ),
                (
                    "[{name='Tests',command=['cargo',false]}]",
                    r#"[{"name":"Tests","command":["cargo",false]}]"#,
                ),
                (
                    "[{name='Tests',command=['cargo',[]]}]",
                    r#"[{"name":"Tests","command":["cargo",[]]}]"#,
                ),
                (
                    "[{name='Tests',command=['cargo',{}]}]",
                    r#"[{"name":"Tests","command":["cargo",{}]}]"#,
                ),
                (
                    r#"[{name='Tests',command=["bad\u0000exe"]}]"#,
                    r#"[{"name":"Tests","command":["bad\u0000exe"]}]"#,
                ),
                (
                    r#"[{name='Tests',command=['cargo',"bad\u0000arg"]}]"#,
                    r#"[{"name":"Tests","command":["cargo","bad\u0000arg"]}]"#,
                ),
                (
                    "[{name='Tests',command=['cargo'],shell=true}]",
                    r#"[{"name":"Tests","command":["cargo"],"shell":true}]"#,
                ),
                (
                    "[{name='A',name='B',command=['cargo']}]",
                    r#"[{"name":"A","name":"B","command":["cargo"]}]"#,
                ),
                (
                    "[{name='A',command=['cargo'],command=['git']}]",
                    r#"[{"name":"A","command":["cargo"],"command":["git"]}]"#,
                ),
            ];
            for (index, filename) in CONFIG_FILES.iter().enumerate() {
                let mut malformed = invalid
                    .iter()
                    .map(|(toml, json)| document(index, toml, json))
                    .collect::<Vec<_>>();
                malformed.push(match index {
                    0 => "finish-checks=[]\nfinish-checks=[]".into(),
                    1 | 2 => "finish-checks: []\nfinish-checks: []".into(),
                    3 => r#"{"finish-checks":[],"finish-checks":[]}"#.into(),
                    _ => unreachable!(),
                });
                if index != 0 {
                    for json in [
                        "null",
                        r#"[{"name":null,"command":["cargo"]}]"#,
                        r#"[{"name":"Tests","command":null}]"#,
                        r#"[{"name":"Tests","command":["cargo",null]}]"#,
                    ] {
                        malformed.push(document(index, "", json));
                    }
                }
                if index == 1 || index == 2 {
                    malformed
                        .push("finish-checks:\n  - name: Tests\n    command: [cargo, 12]\n".into());
                    malformed.push("finish-checks:\n  - name: 12\n    command: [cargo]\n".into());
                }
                for local in [false, true] {
                    let layer = if local { &common } else { repo.path() };
                    let other = if local { repo.path() } else { &common };
                    write(other, index, &[]); // invalid overridden shared values still fail
                    for contents in &malformed {
                        fs::write(layer.join(filename), contents).unwrap();
                        let before = snapshot(root);
                        let error = load(repo.path()).expect_err(contents).to_string();
                        assert!(error.contains(filename), "{error}");
                        assert_eq!(snapshot(root), before);
                    }
                    fs::remove_file(layer.join(filename)).unwrap();
                    fs::remove_file(other.join(filename)).unwrap();
                }
            }
        },
    );
}

#[test]
fn rendering_round_trips_exact_arguments_including_control_characters() {
    isolated(
        "rendering_round_trips_exact_arguments_including_control_characters",
        true,
        |root| {
            let repo = Repository::new(root);
            let checks = vec![
                check(
                    "  Tests \"École\"\n  ",
                    &[
                        "./tools/my checker",
                        "",
                        " ",
                        "two words",
                        "'single'",
                        "\"double\"",
                        "C:\\tools\\check",
                        "$HOME",
                        "${value}",
                        "$(touch sentinel)",
                        "*.rs",
                        "a && b",
                        "--",
                        "true",
                        "null",
                        "123",
                        "line\nnext\r\n",
                        "\t\u{1}\u{8}\u{b}\u{c}\u{1b}\u{1f}\u{7f}\u{85}\u{9f}",
                        "École Ελληνικά 😀\u{2028}\u{2029}",
                    ],
                ),
                check(
                    "Shell",
                    &["sh", "-c", "printf '%s' \"$HOME\" && test -f 'with spaces'"],
                ),
                check("Whitespace executable remains literal", &[" ", ""]),
            ];
            for (index, filename) in CONFIG_FILES.iter().enumerate() {
                write(repo.path(), index, &checks);
                let before = snapshot(root);
                assert_eq!(
                    load(repo.path()).unwrap().finish_checks,
                    checks,
                    "{filename}"
                );
                assert_eq!(snapshot(root), before);
                fs::remove_file(repo.path().join(filename)).unwrap();
            }
        },
    );
}

#[test]
fn generation_validates_typed_checks_and_review_binds_values_and_provenance() {
    isolated(
        "generation_validates_typed_checks_and_review_binds_values_and_provenance",
        true,
        |root| {
            let repo = Repository::new(root);
            for format in [Format::Toml, Format::Yaml, Format::Json] {
                for invalid in [
                    check(" ", &["cargo"]),
                    check("Tests", &[]),
                    check("Tests", &[""]),
                    check("Tests", &["cargo", "\0"]),
                ] {
                    let mut plan = config::prepare(repo.path(), format).unwrap();
                    plan.proposed.finish_checks = vec![invalid];
                    let before = snapshot(root);
                    assert!(matches!(
                        config::generate(repo.path(), &plan),
                        Err(ConfigError::Settings(_))
                    ));
                    assert_eq!(snapshot(root), before);
                }
                let checks = vec![check("Tests", &["cargo", "test", ""])];
                let mut plan = config::prepare(repo.path(), format).unwrap();
                plan.proposed.finish_checks = checks.clone();
                config::generate(repo.path(), &plan).unwrap();
                assert_eq!(load(repo.path()).unwrap().finish_checks, checks);
                fs::remove_file(plan.destination()).unwrap();
            }
            let common = repo.path().join(".git");
            let original = vec![
                check("Tests", &["cargo", "test"]),
                check("Lint", &["cargo", "clippy"]),
            ];
            for changed in [
                vec![check("Renamed", &["cargo", "test"]), original[1].clone()],
                vec![check("Tests", &["cargo", "test", ""]), original[1].clone()],
                vec![original[1].clone(), original[0].clone()],
                vec![],
            ] {
                write(&common, 0, &original);
                let plan = config::prepare_guided(repo.path(), Format::Toml).unwrap();
                write(&common, 0, &changed);
                assert!(matches!(
                    config::revalidate_review(repo.path(), &plan),
                    Err(ConfigError::ReviewChanged)
                ));
            }
            write(&common, 0, &original);
            let plan = config::prepare_guided(repo.path(), Format::Toml).unwrap();
            fs::remove_file(common.join(CONFIG_FILES[0])).unwrap();
            write(&common, 3, &original);
            assert!(matches!(
                config::revalidate_review(repo.path(), &plan),
                Err(ConfigError::ReviewChanged)
            ));
        },
    );
}

fn commit(repo: &Repository, filename: &str) {
    fs::write(repo.path().join(filename), filename).unwrap();
    repo.success(&["add", filename]);
    repo.success(&["commit", "-m", filename]);
}
fn fixture(root: &Path) -> (Repository, Repository) {
    let remote = Repository::new(root);
    commit(&remote, "base");
    let local = Repository::new(root);
    local.success(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    local.success(&["fetch", "origin"]);
    local.success(&["switch", "--no-track", "-c", "work", "origin/main"]);
    commit(&local, "work");
    (local, remote)
}

#[test]
fn check_values_and_sources_invalidate_workflow_plans_before_effects() {
    isolated(
        "check_values_and_sources_invalidate_workflow_plans_before_effects",
        true,
        |root| {
            let (repo, _) = fixture(root);
            let common = repo.path().join(".git");
            let first = vec![
                check("Tests", &["missing-check", "first"]),
                check("Lint", &["missing-lint"]),
            ];
            for replacement in [
                vec![
                    check("Renamed", &["missing-check", "first"]),
                    first[1].clone(),
                ],
                vec![
                    check("Tests", &["missing-check", "second"]),
                    first[1].clone(),
                ],
                vec![first[1].clone(), first[0].clone()],
                vec![],
            ] {
                write(&common, 0, &first);
                let start_plan = start::prepare(repo.path(), "new-work").unwrap();
                let save_plan = save::assess(repo.path(), &save::Request::default()).unwrap();
                write(&common, 0, &replacement);
                let before = snapshot(root);
                assert!(save::revalidate(repo.path(), &save_plan).is_err());
                let failure = start::execute(repo.path(), &start_plan).unwrap_err();
                assert!(failure.effects.is_empty());
                let failure = save::execution::execute(repo.path(), &save_plan);
                assert!(failure.effects.is_empty());
                assert!(failure.commands.is_empty());
                assert!(failure.publication.is_none());
                assert_eq!(snapshot(root), before);
            }
            for checks in [&first[..], &[][..]] {
                write(&common, 0, checks);
                let start_plan = start::prepare(repo.path(), "new-work").unwrap();
                let save_plan = save::assess(repo.path(), &save::Request::default()).unwrap();
                fs::remove_file(common.join(CONFIG_FILES[0])).unwrap();
                write(&common, 3, checks);
                let before = snapshot(root);
                assert!(save::revalidate(repo.path(), &save_plan).is_err());
                assert!(start::execute(repo.path(), &start_plan).is_err());
                assert_eq!(snapshot(root), before);
                fs::remove_file(common.join(CONFIG_FILES[3])).unwrap();
            }
        },
    );
}

#[test]
fn finish_api_guards_block_new_checks_and_changed_empty_list_provenance() {
    isolated(
        "finish_api_guards_block_new_checks_and_changed_empty_list_provenance",
        true,
        |root| {
            for prepared in [false, true] {
                for checks in [vec![], vec![check("Tests", &["missing-check"])]] {
                    let (repo, remote) = fixture(root);
                    commit(&remote, "advanced-base");
                    let request = finish::Request {
                        update: Some(FinishUpdate::Rebase),
                        rewrite: Some(FinishRewrite::Allow),
                    };
                    let initial = finish::assess(repo.path(), &request, None).unwrap();
                    let fresh = preparation::refresh(repo.path(), &initial);
                    assert!(fresh.error.is_none(), "{fresh:?}");
                    let mut plan = fresh.plan.unwrap();
                    assert!(plan.is_ready());
                    if prepared {
                        let report = preparation::prepare(repo.path(), &plan);
                        assert!(report.is_prepared(), "{report:?}");
                        plan = report.plan.unwrap();
                    }
                    write(&repo.path().join(".git"), 0, &checks);
                    let before = snapshot(root);
                    assert!(
                        finish::revalidate(repo.path(), &plan, plan.evidence.as_ref()).is_err()
                    );
                    for report in [
                        preparation::refresh(repo.path(), &plan),
                        preparation::prepare(repo.path(), &plan),
                    ] {
                        assert!(report.error.is_some());
                        assert!(report.commands.is_empty());
                        assert!(report.effects.is_empty());
                    }
                    let report = publication::publish(repo.path(), &plan);
                    assert!(report.error.is_some());
                    assert!(report.destinations.iter().all(|d| d.commands.is_empty()));
                    assert!(report.tracking.writes.is_empty());
                    if !checks.is_empty() {
                        assert!(matches!(
                            finish::assess(repo.path(), &request, None),
                            Err(FinishError::FinishChecksUnavailable)
                        ));
                    }
                    assert_eq!(snapshot(root), before);
                }
            }
        },
    );
}

#[test]
fn cli_inspects_literal_checks_start_and_save_ignore_them_and_finish_refuses() {
    isolated(
        "cli_inspects_literal_checks_start_and_save_ignore_them_and_finish_refuses",
        true,
        |root| {
            for executable in [env!("CARGO_BIN_EXE_git-oneflow"), env!("CARGO_BIN_EXE_gof")] {
                for (index, filename) in CONFIG_FILES.iter().enumerate() {
                    let (repo, remote) = fixture(root);
                    let checks = vec![
                        check("Side effect", &["sh", "-c", "touch check-was-executed"]),
                        check(
                            "Missing executable",
                            &["/no/such/check", "two words", "", "\"quoted\"", "École\n"],
                        ),
                    ];
                    write(&repo.path().join(".git"), index, &checks);
                    let run = |args: &[&str]| {
                        Command::new(executable)
                            .current_dir(repo.path())
                            .args(args)
                            .output()
                            .unwrap()
                    };
                    let before = snapshot(root);
                    let inspected = run(&["config", "show"]);
                    assert!(inspected.status.success(), "{inspected:?}");
                    let output = String::from_utf8(inspected.stdout).unwrap();
                    assert!(
                        output.contains(&format!(
                            "finish-checks: {} (local:",
                            serde_json::to_string(&checks).unwrap()
                        )),
                        "{output}"
                    );
                    let refused = run(&["finish", "--update", "none", "--allow-rewrite"]);
                    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
                    assert!(
                        String::from_utf8_lossy(&refused.stderr)
                            .contains("check execution is not implemented")
                    );
                    assert_eq!(snapshot(root), before);
                    let started = run(&["start", "work"]); // local resume remains offline
                    assert!(started.status.success(), "{started:?}");
                    let saved = run(&["save"]);
                    assert!(saved.status.success(), "{saved:?}");
                    assert_eq!(
                        repo.success(&["rev-parse", "HEAD"]).stdout,
                        remote.success(&["rev-parse", "work"]).stdout
                    );
                    assert!(!repo.path().join("check-was-executed").exists());
                    // A local empty list explicitly disables an inherited shared list.
                    write(repo.path(), index, &checks);
                    repo.success(&["add", filename]);
                    repo.success(&["commit", "-m", "shared checks"]);
                    write(&repo.path().join(".git"), index, &[]);
                    let finished = run(&["finish", "--update", "none"]);
                    assert!(finished.status.success(), "{finished:?}");
                    assert!(!repo.path().join("check-was-executed").exists());
                }
            }
        },
    );
}
