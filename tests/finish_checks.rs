#[path = "support/snapshot.rs"]
mod preservation;
mod support;

use preservation::snapshot;
use std::{fs, path::Path, process::Command};
use support::{Repository, isolated};
use twigsmith::{
    config::{self, ConfigError, Format},
    finish::{self, preparation, publication},
    repository, save,
    settings::{
        self, CONFIG_FILES, FinishCheck, FinishRewrite, FinishUpdate, SettingSource, Settings,
    },
    start,
};

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
            let (repo, _remote) = fixture(root);
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
                        ..Default::default()
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
                    assert_eq!(snapshot(root), before);
                }
            }
        },
    );
}

#[test]
fn cli_inspects_literal_checks_start_and_save_ignore_them() {
    isolated(
        "cli_inspects_literal_checks_start_and_save_ignore_them",
        true,
        |root| {
            for executable in [env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")] {
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
                    let finished = run(&["finish", "--mode", "review", "--update", "none"]);
                    assert!(finished.status.success(), "{finished:?}");
                    assert!(!repo.path().join("check-was-executed").exists());
                }
            }
        },
    );
}

fn finish_cli(repo: &Repository, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tgi"))
        .current_dir(repo.path())
        .arg("finish")
        .args(["--mode", "review"])
        .args(args)
        .output()
        .unwrap()
}
fn absent(repo: &Repository, reference: &str) -> bool {
    !twigsmith::git::run(repo.path(), ["rev-parse", "--verify", reference])
        .unwrap()
        .status
        .success()
}
fn configured(repo: &Repository, checks: &[FinishCheck]) {
    write(&repo.path().join(".git"), 0, checks);
}
fn shell(name: &str, script: &str) -> FinishCheck {
    check(name, &["sh", "-c", script])
}

#[test]
fn real_checks_use_literal_arguments_root_environment_and_order() {
    isolated(
        "real_checks_use_literal_arguments_root_environment_and_order",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            let args = [
                "",
                "two words",
                "'quotes'",
                "$HOME",
                "$(touch unwanted)",
                "*",
                "a;b",
                "line\nbreak",
                "École",
            ];
            let mut command = vec![
                "sh",
                "-c",
                "printf '%s\\0' \"$@\" > .git/args; pwd -P > .git/cwd; printf '%s' \"$CHECK_ENV\" > .git/env; printf first > .git/order; printf 'live stdout\\n'; printf 'live stderr\\n' >&2",
                "check",
            ];
            command.extend(args);
            configured(
                &repo,
                &[
                    check("Exact arguments", &command),
                    shell(
                        "Explicit shell",
                        "test \"$(cat .git/order)\" = first && printf second >> .git/order",
                    ),
                ],
            );
            let nested = repo.path().join("nested/deeper");
            fs::create_dir_all(&nested).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_tgi"))
                .current_dir(&nested)
                .env("CHECK_ENV", "inherited value")
                .arg("f")
                .args(["--mode", "review"])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let expected: Vec<_> = args.iter().flat_map(|arg| arg.bytes().chain([0])).collect();
            assert_eq!(fs::read(repo.path().join(".git/args")).unwrap(), expected);
            assert_eq!(
                fs::read_to_string(repo.path().join(".git/cwd"))
                    .unwrap()
                    .trim(),
                repo.path().canonicalize().unwrap().to_str().unwrap()
            );
            assert_eq!(
                fs::read_to_string(repo.path().join(".git/env")).unwrap(),
                "inherited value"
            );
            assert_eq!(
                fs::read_to_string(repo.path().join(".git/order")).unwrap(),
                "firstsecond"
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("live stdout"));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("Check 1/2: Exact arguments"), "{stderr}");
            assert!(stderr.contains("live stderr"));
            assert_eq!(
                repo.success(&["rev-parse", "HEAD"]).stdout,
                remote.success(&["rev-parse", "work"]).stdout
            );
        },
    );
}

#[test]
fn checks_see_prepared_rebase_and_merge_and_retry_preserves_rewrite_evidence() {
    isolated(
        "checks_see_prepared_rebase_and_merge_and_retry_preserves_rewrite_evidence",
        true,
        |root| {
            for update in ["rebase", "merge"] {
                let (repo, remote) = fixture(root);
                remote.success(&[
                    "fetch",
                    repo.path().to_str().unwrap(),
                    "HEAD:refs/heads/work",
                ]);
                let original = repo.success(&["rev-parse", "HEAD"]).stdout;
                commit(&remote, "advanced-base");
                configured(
                    &repo,
                    &[
                        shell(
                            "Prepared work",
                            "test -f advanced-base && test -f work && git rev-parse HEAD >> .git/checked-heads; test -f .git/permit",
                        ),
                        shell("Later", "touch .git/later"),
                    ],
                );
                let first = finish_cli(&repo, &["--update", update, "--allow-rewrite"]);
                assert_eq!(first.status.code(), Some(1), "{first:?}");
                let prepared = repo.success(&["rev-parse", "HEAD"]).stdout;
                assert_ne!(original, prepared);
                assert_eq!(
                    fs::read(repo.path().join(".git/checked-heads")).unwrap(),
                    prepared
                );
                assert_eq!(remote.success(&["rev-parse", "work"]).stdout, original);
                assert!(!repo.path().join(".git/later").exists());
                let stderr = String::from_utf8_lossy(&first.stderr);
                assert!(
                    stderr.contains("Prepared history and check effects are retained"),
                    "{stderr}"
                );
                fs::write(repo.path().join(".git/permit"), "").unwrap();
                let retry = finish_cli(&repo, &["--update", update, "--allow-rewrite"]);
                assert!(retry.status.success(), "{retry:?}");
                assert_eq!(repo.success(&["rev-parse", "HEAD"]).stdout, prepared);
                assert_eq!(remote.success(&["rev-parse", "work"]).stdout, prepared);
                assert_eq!(
                    fs::read(repo.path().join(".git/checked-heads")).unwrap(),
                    [prepared.clone(), prepared].concat()
                );
            }
        },
    );
}

#[test]
fn failed_launch_exit_and_signal_stop_later_checks_and_publication() {
    isolated(
        "failed_launch_exit_and_signal_stop_later_checks_and_publication",
        true,
        |root| {
            for first in [
                check("Missing", &["/no/such/finish-check"]),
                check("Not executable", &["./work"]),
                check("No implicit shell", &["true && touch .git/unwanted"]),
                shell("Nonzero", "touch .git/retained; exit 23"),
                shell("Signal", "kill -TERM $$"),
            ] {
                let (repo, remote) = fixture(root);
                configured(&repo, &[first.clone(), shell("Later", "touch .git/later")]);
                let output = finish_cli(&repo, &[]);
                assert_eq!(output.status.code(), Some(1), "{output:?}");
                assert!(absent(&remote, "work"));
                assert!(!repo.path().join(".git/later").exists());
                assert!(!repo.path().join(".git/unwanted").exists());
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(
                    stderr.contains(&format!("Check 1 ({}) stopped", first.name)),
                    "{stderr}"
                );
                assert!(stderr.contains("Branch publication not attempted"));
                if first.name == "Nonzero" {
                    assert!(stderr.contains("23"));
                    assert!(repo.path().join(".git/retained").exists());
                }
            }
        },
    );
}

#[test]
fn successful_state_mutations_stop_before_next_check_and_publication() {
    isolated(
        "successful_state_mutations_stop_before_next_check_and_publication",
        true,
        |root| {
            for script in [
                "printf changed > work",
                "touch untracked",
                "printf changed > work; git add work",
                "git commit --allow-empty -m check-change",
                "git switch -c different",
                "printf 'finish-checks=[]\\n' > .git/twigsmith.config.toml",
                "git config remote.origin.pushurl /changed/destination",
                "git config branch.work.pushRemote changed",
            ] {
                let (repo, remote) = fixture(root);
                configured(
                    &repo,
                    &[
                        shell("Mutation", script),
                        shell("Later", "touch .git/later"),
                    ],
                );
                let output = finish_cli(&repo, &[]);
                assert_eq!(output.status.code(), Some(1), "{script}: {output:?}");
                assert!(absent(&remote, "work"));
                assert!(!repo.path().join(".git/later").exists());
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("Check 1 (Mutation) stopped")
                );
            }
            let (repo, remote) = fixture(root);
            fs::create_dir_all(repo.path().join(".git/info")).unwrap();
            fs::write(repo.path().join(".git/info/exclude"), "build/\n").unwrap();
            configured(
                &repo,
                &[shell(
                    "Ignored output",
                    "mkdir -p build; touch build/artifact",
                )],
            );
            let output = finish_cli(&repo, &[]);
            assert!(output.status.success(), "{output:?}");
            assert!(!absent(&remote, "work"));
            assert!(repo.path().join("build/artifact").exists());
        },
    );
}

#[test]
fn remote_movement_during_checks_keeps_original_publication_observation() {
    isolated(
        "remote_movement_during_checks_keeps_original_publication_observation",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            configured(
                &repo,
                &[check(
                    "Advance destination",
                    &[
                        "git",
                        "-C",
                        remote.path().to_str().unwrap(),
                        "update-ref",
                        "refs/heads/work",
                        "HEAD",
                    ],
                )],
            );
            let output = finish_cli(&repo, &[]);
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            assert_eq!(
                remote.success(&["rev-parse", "work"]).stdout,
                remote.success(&["rev-parse", "main"]).stdout
            );
            assert_ne!(
                repo.success(&["rev-parse", "HEAD"]).stdout,
                remote.success(&["rev-parse", "work"]).stdout
            );
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("destination changed since observation")
            );
        },
    );
}

#[test]
#[cfg(unix)]
fn skip_checks_all_tool_aliases_reports_bypass_and_keeps_hooks_active() {
    use std::os::unix::fs::PermissionsExt;
    isolated(
        "skip_checks_all_tool_aliases_reports_bypass_and_keeps_hooks_active",
        true,
        |root| {
            for executable in [env!("CARGO_BIN_EXE_tgi"), env!("CARGO_BIN_EXE_twigsmith")] {
                for alias in ["finish", "f"] {
                    let (repo, remote) = fixture(root);
                    configured(&repo, &[shell("Skipped", "touch .git/check-ran; exit 1")]);
                    let hooks = repo.path().join(".git/hooks");
                    fs::create_dir_all(&hooks).unwrap();
                    repo.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
                    let hook = hooks.join("pre-push");
                    fs::write(&hook, "#!/bin/sh\ntouch .git/hook-ran\nexit 1\n").unwrap();
                    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
                    let run = || {
                        Command::new(executable)
                            .current_dir(repo.path())
                            .args([alias, "--mode", "review", "--skip-checks"])
                            .output()
                            .unwrap()
                    };
                    let rejected = run();
                    assert_eq!(rejected.status.code(), Some(1), "{rejected:?}");
                    assert!(absent(&remote, "work"));
                    assert!(repo.path().join(".git/hook-ran").exists());
                    assert!(!repo.path().join(".git/check-ran").exists());
                    assert!(
                        String::from_utf8_lossy(&rejected.stderr)
                            .contains("Skipped 1 configured finish check(s) (--skip-checks)")
                    );
                    fs::remove_file(hook).unwrap();
                    let accepted = run();
                    assert!(accepted.status.success(), "{accepted:?}");
                    let duplicate = Command::new(executable)
                        .args([alias, "--skip-checks", "--skip-checks"])
                        .output()
                        .unwrap();
                    assert_eq!(duplicate.status.code(), Some(2));
                }
            }
        },
    );
}

#[test]
#[cfg(unix)]
fn fork_publication_retry_reruns_checks_and_preserves_rebase_recovery() {
    use std::os::unix::fs::PermissionsExt;
    isolated(
        "fork_publication_retry_reruns_checks_and_preserves_rebase_recovery",
        true,
        |root| {
            let (repo, upstream) = fixture(root);
            let fork = Repository::new(root);
            fork.success(&[
                "fetch",
                repo.path().to_str().unwrap(),
                "HEAD:refs/heads/work",
            ]);
            repo.success(&["remote", "rename", "origin", "upstream"]);
            repo.success(&["remote", "add", "origin", fork.path().to_str().unwrap()]);
            commit(&upstream, "project-update");
            configured(
                &repo,
                &[shell(
                    "Prepared fork work",
                    "test -f project-update && printf checked\\n >> .git/runs",
                )],
            );
            let config_path = repo.path().join(".git/twigsmith.config.toml");
            let config = fs::read_to_string(&config_path).unwrap();
            fs::write(config_path, format!("base-remote='upstream'\n{config}")).unwrap();
            let hooks = fork.path().join(".git/hooks");
            fs::create_dir_all(&hooks).unwrap();
            fork.success(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
            let hook = hooks.join("pre-receive");
            fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let original = fork.success(&["rev-parse", "work"]).stdout;
            let first = finish_cli(&repo, &["--update", "rebase", "--allow-rewrite"]);
            assert_eq!(first.status.code(), Some(1), "{first:?}");
            let prepared = repo.success(&["rev-parse", "HEAD"]).stdout;
            assert_ne!(original, prepared, "{first:?}");
            assert_eq!(fork.success(&["rev-parse", "work"]).stdout, original);
            fs::remove_file(hook).unwrap();
            let retry = finish_cli(&repo, &["--allow-rewrite"]);
            assert!(retry.status.success(), "{retry:?}");
            assert_eq!(fork.success(&["rev-parse", "work"]).stdout, prepared);
            assert!(absent(&upstream, "work"));
            assert_eq!(
                fs::read_to_string(repo.path().join(".git/runs"))
                    .unwrap()
                    .matches("checked")
                    .count(),
                2
            );
        },
    );
}

#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;
#[test]
#[cfg(target_os = "linux")]
fn cancellation_stops_check_group_retains_preparation_and_blocks_publication() {
    isolated(
        "cancellation_stops_check_group_retains_preparation_and_blocks_publication",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            commit(&remote, "advanced-base");
            let original = repo.success(&["rev-parse", "HEAD"]).stdout;
            configured(
                &repo,
                &[
                    shell(
                        "Long check",
                        "echo $$ > .git/leader; sh -c 'trap \"\" INT TERM; echo $$ > .git/descendant; printf \"CHECK-RE%s\\n\" ADY; while :; do sleep 1; done' & wait",
                    ),
                    shell("Later", "touch .git/later"),
                ],
            );
            let mut term = terminal::Terminal::start(
                env!("CARGO_BIN_EXE_tgi"),
                repo.path(),
                &["finish", "--mode", "review", "--update", "rebase"],
            );
            term.until("CHECK-READY");
            term.interrupt();
            assert_eq!(term.finish().code(), Some(1), "{}", term.output);
            assert!(term.output.contains("check interrupted"), "{}", term.output);
            assert!(
                term.output
                    .contains("Prepared history and check effects are retained")
            );
            assert_ne!(repo.success(&["rev-parse", "HEAD"]).stdout, original);
            assert!(absent(&remote, "work"));
            assert!(!repo.path().join(".git/later").exists());
            for name in ["leader", "descendant"] {
                let pid = fs::read_to_string(repo.path().join(format!(".git/{name}"))).unwrap();
                if let Ok(status) = fs::read_to_string(format!("/proc/{}/status", pid.trim())) {
                    assert!(
                        status
                            .lines()
                            .any(|line| line.starts_with("State:") && line.contains("Z (zombie)")),
                        "check process still running: {status}"
                    );
                }
            }
        },
    );
}

#[test]
fn library_publication_retries_run_checks_again_without_reusable_success() {
    isolated(
        "library_publication_retries_run_checks_again_without_reusable_success",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            // Existing tracking avoids a successful publish legitimately changing the plan.
            repo.success(&["config", "branch.work.remote", "origin"]);
            repo.success(&["config", "branch.work.merge", "refs/heads/work"]);
            configured(
                &repo,
                &[shell(
                    "Every attempt",
                    "printf x >> .git/runs; test -f .git/permit",
                )],
            );
            let initial = finish::assess(repo.path(), &finish::Request::default(), None).unwrap();
            let refreshed = preparation::refresh(repo.path(), &initial).plan.unwrap();
            let prepared = preparation::prepare(repo.path(), &refreshed).plan.unwrap();
            let first = publication::publish(repo.path(), &prepared);
            assert!(first.checks.error.is_some(), "{first:?}");
            assert!(first.destinations.iter().all(|d| d.commands.is_empty()));
            fs::write(repo.path().join(".git/permit"), "").unwrap();
            let second = publication::publish(repo.path(), &prepared);
            assert!(second.is_complete(), "{second:?}");
            fs::remove_file(repo.path().join(".git/permit")).unwrap();
            let third = publication::publish(repo.path(), &prepared);
            assert!(third.checks.error.is_some(), "{third:?}");
            assert!(third.destinations.iter().all(|d| d.commands.is_empty()));
            assert_eq!(fs::read(repo.path().join(".git/runs")).unwrap(), b"xxx");
            assert_eq!(
                remote.success(&["rev-parse", "work"]).stdout,
                repo.success(&["rev-parse", "HEAD"]).stdout
            );
        },
    );
}

#[test]
fn check_command_and_output_redaction_preserves_exact_executed_arguments() {
    isolated(
        "check_command_and_output_redaction_preserves_exact_executed_arguments",
        true,
        |root| {
            for show in [false, true] {
                let (repo, _remote) = fixture(root);
                let secret = "https://user:synthetic-check-secret@example.invalid/path";
                configured(
                    &repo,
                    &[check(
                        "URL check",
                        &[
                            "sh",
                            "-c",
                            "printf '%s' \"$1\" > .git/exact; printf '%s\\n' \"$1\"; printf '%s\\n' \"$1\" >&2",
                            "check",
                            secret,
                        ],
                    )],
                );
                let output = finish_cli(&repo, if show { &["--show-credentials"] } else { &[] });
                assert!(output.status.success(), "{output:?}");
                assert_eq!(
                    fs::read_to_string(repo.path().join(".git/exact")).unwrap(),
                    secret
                );
                for bytes in [&output.stdout, &output.stderr] {
                    let text = String::from_utf8_lossy(bytes);
                    assert_eq!(text.contains("synthetic-check-secret"), show, "{text}");
                    assert!(text.contains("example.invalid/path"));
                }
            }
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn check_output_without_newline_is_live_and_fragmented_credentials_are_redacted() {
    isolated(
        "check_output_without_newline_is_live_and_fragmented_credentials_are_redacted",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            configured(
                &repo,
                &[shell(
                    "Streaming",
                    "printf 'REA%s' DY; while ! test -f .git/continue; do sleep 0.01; done; printf 'https://user:synthetic-fragment'; sleep 0.05; printf 'ed-secret@example.invalid/path\\n'; exit 1",
                )],
            );
            let mut term = terminal::Terminal::start(
                env!("CARGO_BIN_EXE_tgi"),
                repo.path(),
                &["finish", "--mode", "review"],
            );
            term.until("READY");
            fs::write(repo.path().join(".git/continue"), "").unwrap();
            assert_eq!(term.finish().code(), Some(1), "{}", term.output);
            assert!(
                !term.output.contains("synthetic-fragmented-secret"),
                "{}",
                term.output
            );
            assert!(term.output.contains("<redacted>@example.invalid/path"));
            assert!(absent(&remote, "work"));
        },
    );
}

#[test]
#[cfg(unix)]
fn relative_executable_runs_from_discovered_linked_worktree_root() {
    use std::os::unix::fs::PermissionsExt;
    isolated(
        "relative_executable_runs_from_discovered_linked_worktree_root",
        true,
        |root| {
            let (repo, remote) = fixture(root);
            let executable = repo.path().join("validate");
            fs::write(&executable, "#!/bin/sh\npwd -P > \"$1\"\n").unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            repo.success(&["add", "validate"]);
            repo.success(&["commit", "-m", "validation executable"]);
            let result = root.join("checked-working-directory");
            configured(
                &repo,
                &[check(
                    "Relative executable",
                    &["./validate", result.to_str().unwrap()],
                )],
            );
            let linked = root.join("linked worktree");
            repo.success(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
            let nested = linked.join("nested");
            fs::create_dir(&nested).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_tgi"))
                .current_dir(nested)
                .arg("finish")
                .args(["--mode", "review"])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                fs::read_to_string(result).unwrap().trim(),
                linked.canonicalize().unwrap().to_str().unwrap()
            );
            assert_eq!(
                remote.success(&["rev-parse", "linked"]).stdout,
                repo.success(&["rev-parse", "linked"]).stdout
            );
        },
    );
}
