//! Regression for qualification controls across the real isolated-child boundary.
#[allow(dead_code)]
mod support;

use std::{env, fs, path::PathBuf, process::Command};

#[test]
fn installed_shim_controls_reach_the_actual_isolated_child() {
    const NAME: &str = "installed_shim_controls_reach_the_actual_isolated_child";
    if env::var_os("TWIGSMITH_BOUNDARY_REGRESSION_OUTER").is_some()
        || env::var("TWIGSMITH_ISOLATED_TEST").as_deref() == Ok(NAME)
    {
        support::isolated(NAME, false, |_| {
            assert_eq!(env::var("TWIGSMITH_NPM_SHIM_KIND").as_deref(), Ok("ps1"));
            let pwsh =
                env::var_os("TWIGSMITH_NPM_POWERSHELL").expect("forward selected PowerShell");
            assert_eq!(
                fs::canonicalize(pwsh).unwrap(),
                fs::canonicalize(env::current_exe().unwrap()).unwrap()
            );
            let trace =
                env::var_os("TWIGSMITH_NPM_SHIM_TRACE").expect("forward controlled shim trace");
            assert_eq!(fs::read(trace).unwrap(), b"qualification trace sentinel");
            assert_eq!(env::var("TWIGSMITH_ISOLATED_TEST").as_deref(), Ok(NAME));
            assert!(env::var_os("TWIGSMITH_BOUNDARY_REGRESSION_OUTER").is_none());
        });
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let trace = directory.path().join("controlled trace.jsonl");
    fs::write(&trace, b"qualification trace sentinel").unwrap();
    let output = Command::new(env::current_exe().unwrap())
        .args(["--exact", NAME, "--nocapture"])
        .env("TWIGSMITH_BOUNDARY_REGRESSION_OUTER", "1")
        .env("TWIGSMITH_NPM_SHIM_KIND", "ps1")
        .env(
            "TWIGSMITH_NPM_POWERSHELL",
            fs::canonicalize(env::current_exe().unwrap()).unwrap(),
        )
        .env("TWIGSMITH_NPM_SHIM_TRACE", &trace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "actual isolated child dropped qualification controls: {output:?}"
    );
}

#[test]
fn actual_bridge_records_selected_shim_with_no_powershell_on_path() {
    let directory = tempfile::tempdir().unwrap();
    let source = env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("native_npm_fixture{}", env::consts::EXE_SUFFIX));
    assert!(source.is_file(), "build --example native_npm_fixture");
    let bridge = directory
        .path()
        .join(format!("twigsmith{}", env::consts::EXE_SUFFIX));
    fs::copy(source, &bridge).unwrap();
    let trace = directory.path().join("actual bridge trace.jsonl");
    fs::write(&trace, b"").unwrap();
    let mut command = Command::new(bridge);
    let arguments = [
        "--version",
        "literal with spaces",
        "'single' and \"double\"",
        "",
        "$(throw 'must stay literal')",
        "λ 世界",
        "trailing\\",
        "a‘; Write-Output INJECTED; #",
        "a’; Write-Output INJECTED; #",
        "a‚; Write-Output INJECTED; #",
        "a‛; Write-Output INJECTED; #",
    ];
    command
        .args(arguments)
        .env("PATH", directory.path())
        .env("TWIGSMITH_NPM_SHIM_DIR", directory.path())
        .env("TWIGSMITH_NPM_SHIM_TRACE", &trace)
        .env("TWIGSMITH_QUALIFICATION_CHILD", "1")
        .env(
            "TWIGSMITH_ISOLATED_TEST",
            "actual_bridge_records_selected_shim_with_no_powershell_on_path",
        );
    #[cfg(windows)]
    let (shim, pwsh, kind) = {
        let observed = Command::new("pwsh")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-Command",
                "Join-Path $PSHOME pwsh.exe",
            ])
            .output()
            .expect("Windows qualification requires PowerShell");
        assert!(observed.status.success());
        let selected = PathBuf::from(String::from_utf8(observed.stdout).unwrap().trim());
        command
            .env("TWIGSMITH_NPM_SHIM_KIND", "ps1")
            .env("TWIGSMITH_NPM_POWERSHELL", &selected);
        let shims = directory
            .path()
            .join("installed λ ' ‘ ’ ‚ ‛ ; Write-Output INJECTED ; #");
        fs::create_dir(&shims).unwrap();
        command.env("TWIGSMITH_NPM_SHIM_DIR", &shims);
        let shim = shims.join("twigsmith.ps1");
        fs::write(&shim, "[Console]::Out.WriteLine('ACTUAL_SHIM_OK')\nforeach ($item in $args) { [Console]::Out.WriteLine($item) }\nexit 7\n").unwrap();
        (shim, Some(fs::canonicalize(selected).unwrap()), "ps1")
    };
    #[cfg(unix)]
    let (shim, pwsh, kind) = {
        use std::os::unix::fs::PermissionsExt;
        command.env_remove("TWIGSMITH_NPM_SHIM_KIND");
        let shim = directory.path().join("twigsmith");
        // Keep the bridge separate from the actual shim under test.
        let shims = directory.path().join("installed");
        fs::create_dir(&shims).unwrap();
        let shim = shims.join(shim.file_name().unwrap());
        fs::write(
            &shim,
            "#!/bin/sh\nprintf 'ACTUAL_SHIM_OK\\n'\nprintf '%s\\n' \"$@\"\nexit 7\n",
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        command.env("TWIGSMITH_NPM_SHIM_DIR", shims);
        (shim, None::<PathBuf>, "cmd")
    };
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        format!("ACTUAL_SHIM_OK\n{}\n", arguments.join("\n"))
    );
    assert!(output.stderr.is_empty(), "{output:?}");
    let rows: Vec<serde_json::Value> = fs::read_to_string(&trace)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], kind);
    assert_eq!(rows[0]["alias"], "twigsmith");
    assert_eq!(
        rows[0]["isolated_test"],
        "actual_bridge_records_selected_shim_with_no_powershell_on_path"
    );
    assert_eq!(
        PathBuf::from(rows[0]["shim"].as_str().unwrap()),
        fs::canonicalize(shim).unwrap()
    );
    assert_eq!(rows[0]["pwsh"], serde_json::to_value(pwsh).unwrap());
    assert_eq!(
        rows[0]["pwsh_mode"],
        if cfg!(windows) {
            serde_json::json!("Command")
        } else {
            serde_json::Value::Null
        }
    );
}
