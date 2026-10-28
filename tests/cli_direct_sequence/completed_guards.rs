use super::*;

fn remote_refs(f: &Fixture) -> Vec<u8> {
    f.local
        .success(&[
            "--git-dir",
            f.remote.path().to_str().unwrap(),
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/tags",
        ])
        .stdout
}

#[test]
fn recreated_completed_target_or_missing_pin_cannot_authorize_pending_delivery() {
    isolated(
        "completed_guards::recreated_completed_target_or_missing_pin_cannot_authorize_pending_delivery",
        true,
        |root| {
            for remove_pin in [false, true] {
                let f = Fixture::new(root, "integrated-result", true, "local");
                let source = commit(&f.local, "shared", "release\n");
                let stopped = cli(&f.local, &["finish", "--tag", "guarded"]);
                assert!(!stopped.status.success(), "{stopped:?}");
                assert!(f.local.path().join(".git/MERGE_HEAD").exists());
                let first = oid(&f.local, "main");
                let tag = oid(&f.local, "guarded");
                fs::write(f.local.path().join("shared"), "resolved\n").unwrap();
                f.local.success(&["add", "shared"]);
                f.local
                    .success(&["-c", "core.editor=true", "merge", "--continue"]);
                if remove_pin {
                    let pins = f.local.success(&[
                        "for-each-ref",
                        "--format=%(refname)",
                        "--points-at",
                        &first,
                        "refs/twigsmith/integration",
                    ]);
                    let pins = String::from_utf8(pins.stdout).unwrap();
                    let pins: Vec<_> = pins.lines().collect();
                    assert_eq!(pins.len(), 1, "{pins:?}");
                    f.local.success(&["update-ref", "-d", pins[0], &first]);
                } else {
                    f.local.success(&["branch", "-D", "main"]);
                    f.local.success(&["branch", "main", &first]);
                    assert_eq!(oid(&f.local, "main"), first);
                }
                let before = remote_refs(&f);
                let blocked = cli(&f.local, &["finish"]);
                assert!(!blocked.status.success(), "{blocked:?}");
                assert_eq!(remote_refs(&f), before);
                assert_eq!(oid(&f.local, "release-work"), source);
                assert_eq!(oid(&f.local, "guarded"), tag);
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn changed_completed_remote_blocks_tag_delivery_and_both_cleanup() {
    use std::os::unix::fs::PermissionsExt;
    isolated(
        "completed_guards::changed_completed_remote_blocks_tag_delivery_and_both_cleanup",
        true,
        |root| {
            let f = Fixture::new(root, "work-source", false, "both");
            let source = commit(&f.local, "version", "released work\n");
            success(&f.local, &["save"]);
            let hook = f.remote.path().join("hooks/update");
            fs::write(
                &hook,
                "#!/bin/sh\ncase \"$1\" in refs/tags/*) exit 1;; esac\nexit 0\n",
            )
            .unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let stopped = cli(&f.local, &["finish", "--tag", "pending-tag"]);
            assert!(!stopped.status.success(), "{stopped:?}");
            let first = oid(&f.local, "main");
            let second = oid(&f.local, "maintenance");
            assert_eq!(f.remote_oid("main"), first);
            assert_eq!(f.remote_oid("maintenance"), second);
            assert_eq!(f.remote_oid("release-work"), source);
            fs::remove_file(hook).unwrap();
            let earlier = oid(&f.local, "release-work^1");
            f.local.success(&[
                "--git-dir",
                f.remote.path().to_str().unwrap(),
                "update-ref",
                "refs/heads/main",
                &earlier,
                &first,
            ]);
            let before = remote_refs(&f);
            let blocked = cli(&f.local, &["finish"]);
            assert!(!blocked.status.success(), "{blocked:?}");
            assert_eq!(remote_refs(&f), before);
            assert_eq!(oid(&f.local, "release-work"), source);
            assert_eq!(f.remote_oid("release-work"), source);
            assert_eq!(f.remote_oid("maintenance"), second);
            assert_eq!(oid(&f.local, "pending-tag"), source);
        },
    );
}
