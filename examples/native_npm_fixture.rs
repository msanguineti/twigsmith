//! Qualification-only bridge from native terminal harnesses to installed npm shims.
//! The Windows terminal fixture uses CreateProcess, which cannot launch .cmd.
use std::process::Command;
#[cfg(windows)]
use std::process::exit;
use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn selected_powershell(path: &Path) -> PathBuf {
    assert!(path.is_absolute(), "selected PowerShell must be absolute");
    let metadata = fs::symlink_metadata(path).expect("selected PowerShell metadata");
    assert!(
        metadata.is_file() && metadata.len() > 0,
        "selected PowerShell must be a nonempty regular file"
    );
    fs::canonicalize(path).expect("selected PowerShell real path")
}

fn powershell_command(
    pwsh: &Path,
    shim: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> Command {
    // -File returns 0 when its pipeline is interrupted by Ctrl-C. Invoke the
    // installed shim with the call operator, as in a PowerShell session, and
    // preserve its ordinary status explicitly. Keep paths and arguments out of
    // PowerShell source, including every quote character its lexer recognizes.
    let invocation = serde_json::json!({"shim": shim, "arguments": arguments.into_iter()
        .map(|value| value.into_string().expect("UTF-8 qualification argument"))
        .collect::<Vec<_>>()});
    const SCRIPT: &str = "$invocation = ConvertFrom-Json -InputObject $env:TWIGSMITH_NPM_BRIDGE_INVOCATION; \
        [string]$shim = $invocation.shim; [string[]]$shimArguments = @($invocation.arguments); \
        & $shim @shimArguments; exit $LASTEXITCODE";
    let mut command = Command::new(pwsh);
    command.args(["-NoLogo", "-NoProfile", "-Command", SCRIPT]);
    command.env("TWIGSMITH_NPM_BRIDGE_INVOCATION", invocation.to_string());
    command
}

fn trace_invocation(kind: &str, shim: &Path, pwsh: Option<&Path>, alias: &str) {
    let Some(trace) = std::env::var_os("TWIGSMITH_NPM_SHIM_TRACE") else {
        return;
    };
    assert_eq!(
        std::env::var("TWIGSMITH_QUALIFICATION_CHILD").as_deref(),
        Ok("1"),
        "shim trace requires qualification control"
    );
    let isolated_test = std::env::var("TWIGSMITH_ISOLATED_TEST")
        .expect("shim trace requires an isolated qualification case");
    let trace = Path::new(&trace);
    assert!(trace.is_absolute(), "shim trace must be absolute");
    assert!(
        fs::symlink_metadata(trace)
            .expect("prepared qualification trace")
            .is_file(),
        "shim trace must be a regular file"
    );
    let value = serde_json::json!({"kind": kind, "shim": fs::canonicalize(shim).expect("installed shim real path"),
        "pwsh": pwsh, "pwsh_mode": pwsh.map(|_| "Command"), "isolated_test": isolated_test, "alias": alias});
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(trace)
        .expect("append qualification shim trace");
    let mut row = serde_json::to_vec(&value).expect("serialize shim trace");
    row.push(b'\n');
    file.write_all(&row).expect("write shim trace");
}

fn main() {
    let executable = std::env::current_exe().expect("fixture executable");
    let name = executable.file_stem().unwrap().to_str().unwrap();
    assert!(
        matches!(name, "twigsmith" | "tgi"),
        "stage the npm bridge under a command name"
    );
    let directory =
        std::env::var_os("TWIGSMITH_NPM_SHIM_DIR").expect("installed npm shim directory");
    let kind = std::env::var("TWIGSMITH_NPM_SHIM_KIND").unwrap_or_else(|_| "cmd".into());
    assert!(
        matches!(kind.as_str(), "cmd" | "ps1"),
        "unknown npm shim kind"
    );
    let suffix = if cfg!(windows) {
        if kind == "ps1" { ".ps1" } else { ".cmd" }
    } else {
        ""
    };
    let shim = std::path::Path::new(&directory).join(format!("{name}{suffix}"));
    let pwsh = (cfg!(windows) && kind == "ps1").then(|| {
        selected_powershell(Path::new(
            &std::env::var_os("TWIGSMITH_NPM_POWERSHELL")
                .expect("absolute selected npm PowerShell"),
        ))
    });
    trace_invocation(&kind, &shim, pwsh.as_deref(), name);
    let mut command = if let Some(pwsh) = pwsh {
        powershell_command(&pwsh, &shim, std::env::args_os().skip(1))
    } else {
        let mut command = Command::new(shim);
        command.args(std::env::args_os().skip(1));
        command
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        panic!("start the actual installed npm shim: {}", command.exec());
    }
    #[cfg(windows)]
    {
        // Console events reach the actual shim/Node/native process too. Let the
        // native handler finish recovery and return its status through the shim.
        unsafe extern "system" fn observe(_: u32) -> i32 {
            1
        }
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(observe), 1);
        }
        let status = command
            .status()
            .expect("start the actual installed npm shim");
        exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires actual PowerShell selected by TWIGSMITH_TEST_PWSH"]
    fn powershell_call_operator_preserves_literal_arguments_and_explicit_status() {
        let executable = selected_powershell(Path::new(
            &std::env::var_os("TWIGSMITH_TEST_PWSH").expect("selected test PowerShell"),
        ));
        let directory = tempfile::tempdir().unwrap();
        let shim = directory
            .path()
            .join("installed λ ' ‘ ’ ‚ ‛ ; Write-Output INJECTED ; #.ps1");
        fs::write(
            &shim,
            "foreach ($item in $args) { [Console]::Out.WriteLine($item) }\nexit 7\n",
        )
        .unwrap();
        let arguments = [
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
        let output = powershell_command(&executable, &shim, arguments.map(OsString::from))
            .env("PATH", directory.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(7), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
            format!("{}\n", arguments.join("\n"))
        );
        assert!(output.stderr.is_empty(), "{output:?}");
    }

    #[test]
    fn absolute_selected_runtime_launches_with_no_shell_on_path() {
        let directory = tempfile::tempdir().unwrap();
        let executable = selected_powershell(&std::env::current_exe().unwrap());
        let output = Command::new(executable)
            .arg("--list")
            .env("PATH", directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "absolute runtime must not depend on PATH: {output:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("absolute_selected_runtime_launches_with_no_shell_on_path")
        );
    }

    #[test]
    fn selected_runtime_rejects_relative_empty_or_linked_files() {
        assert!(std::panic::catch_unwind(|| selected_powershell(Path::new("pwsh"))).is_err());
        let directory = tempfile::tempdir().unwrap();
        let empty = directory.path().join("empty");
        fs::write(&empty, b"").unwrap();
        assert!(std::panic::catch_unwind(|| selected_powershell(&empty)).is_err());
        assert!(std::panic::catch_unwind(|| selected_powershell(directory.path())).is_err());
        #[cfg(unix)]
        {
            let linked = directory.path().join("linked");
            std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &linked).unwrap();
            assert!(std::panic::catch_unwind(|| selected_powershell(&linked)).is_err());
        }
    }
}
