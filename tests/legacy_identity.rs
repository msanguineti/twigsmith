mod support;

use std::fs;
use support::{Repository, isolated};
use twigsmith::{config, repository, settings};

#[test]
fn legacy_configuration_in_either_layer_blocks_loading_and_creation() {
    isolated(
        "legacy_configuration_in_either_layer_blocks_loading_and_creation",
        true,
        |root| {
            let repo = Repository::new(root);
            let context = repository::discover(repo.path()).unwrap().unwrap();
            for layer in [repo.path(), context.common_dir.as_path()] {
                let old = layer.join("git-oneflow.config.toml");
                fs::write(&old, "base = 'main'\n").unwrap();
                let error = settings::load_with_files(repo.path(), &context)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("legacy configuration"), "{error}");
                if layer == repo.path() {
                    assert!(config::prepare(repo.path(), config::Format::Toml).is_err());
                }
                assert!(old.exists());
                fs::remove_file(old).unwrap();
            }
            assert!(settings::load_with_files(repo.path(), &context).is_ok());
        },
    );
}

#[test]
fn legacy_branch_records_block_only_affected_branch_and_are_preserved() {
    isolated(
        "legacy_branch_records_block_only_affected_branch_and_are_preserved",
        true,
        |root| {
            let repo = Repository::new(root);
            repo.success(&["commit", "--allow-empty", "-m", "initial"]);
            let context = repository::discover(repo.path()).unwrap().unwrap();
            let records = [
                "oneflow-finish/branches/main/rewrite.json",
                "oneflow-integration/branches/main/attempt.json",
                "oneflow-hosted/attempts/6d61696e",
                "oneflow-hosted/associations/6d61696e",
            ];
            for name in records {
                let path = context.git_dir.join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, "retained").unwrap();
                let error = settings::load_for_branch(repo.path(), &context, b"main")
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("legacy branch association or unfinished recovery"),
                    "{error}"
                );
                assert!(settings::load_for_branch(repo.path(), &context, b"other").is_ok());
                assert_eq!(fs::read(&path).unwrap(), b"retained");
                fs::remove_file(path).unwrap();
            }
            repo.success(&["config", "branch.main.oneflow-policy", "legacy"]);
            assert!(
                settings::load_for_branch(repo.path(), &context, b"main")
                    .unwrap_err()
                    .to_string()
                    .contains("legacy branch association")
            );
            assert!(settings::load_for_branch(repo.path(), &context, b"other").is_ok());
            repo.success(&["config", "--unset", "branch.main.oneflow-policy"]);
            assert!(settings::load_for_branch(repo.path(), &context, b"main").is_ok());
        },
    );
}

#[test]
#[cfg(unix)]
fn legacy_selector_blocks_destination_resume_and_unreadable_evidence() {
    use std::os::unix::fs::PermissionsExt;
    use twigsmith::finish::integration::{self, Request, recovery::Selection};
    isolated(
        "legacy_selector_blocks_destination_resume_and_unreadable_evidence",
        true,
        |root| {
            let repo = Repository::new(root);
            repo.success(&["commit", "--allow-empty", "-m", "initial"]);
            let remote = root.join("remote.git");
            let output = std::process::Command::new("git")
                .args(["clone", "--bare"])
                .arg(repo.path())
                .arg(&remote)
                .output()
                .unwrap();
            assert!(output.status.success());
            repo.success(&["remote", "add", "origin", remote.to_str().unwrap()]);
            repo.success(&["switch", "-c", "feature"]);
            repo.success(&["commit", "--allow-empty", "-m", "feature"]);
            let request = Request::default();
            let selection = Selection::observe(repo.path(), "feature").unwrap();
            let context = repository::discover(repo.path()).unwrap().unwrap();
            let dir = context.git_dir.join("oneflow-integration");
            let branch_dir = dir.join("branches/feature");
            fs::create_dir_all(&branch_dir).unwrap();
            for path in [&dir, &dir.join("branches"), &branch_dir] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let selector = dir.join("active.json");
            let record_path = branch_dir.join("attempt.json");
            fs::write(&selector, serde_json::to_vec(&selection).unwrap()).unwrap();
            fs::write(&record_path, b"invalid record").unwrap();
            for path in [&selector, &record_path] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            repo.success(&["switch", "main"]);
            let error = integration::assess(repo.path(), &request, None)
                .unwrap_err()
                .to_string();
            assert!(error.contains("legacy integration"), "{error}");
            assert!(record_path.exists());
            fs::remove_file(&record_path).unwrap();
            let error = integration::assess(repo.path(), &request, None)
                .unwrap_err()
                .to_string();
            assert!(error.contains("legacy integration"), "{error}");
            fs::write(&selector, b"invalid").unwrap();
            let error = integration::assess(repo.path(), &request, None)
                .unwrap_err()
                .to_string();
            assert!(error.contains("legacy integration"), "{error}");
            assert_eq!(fs::read(&selector).unwrap(), b"invalid");
        },
    );
}
