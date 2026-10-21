#![cfg(unix)]

mod support;
#[cfg(target_os = "linux")]
#[path = "support/terminal.rs"]
#[allow(dead_code)]
mod terminal;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::{Repository, isolated};

fn shell(name: &str) -> Option<PathBuf> {
    let variable = format!("ONEFLOW_TEST_{}", name.to_uppercase());
    if let Some(path) = std::env::var_os(variable) {
        return Some(path.into());
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}

fn backend_path() -> std::ffi::OsString {
    let mut paths = vec![
        Path::new(env!("CARGO_BIN_EXE_git-oneflow"))
            .parent()
            .unwrap()
            .to_owned(),
    ];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

fn artifact(root: &Path, name: &str) -> PathBuf {
    let output = Command::new(env!("CARGO_BIN_EXE_git-oneflow"))
        .args(["completion", name])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let extension = if name == "powershell" { "ps1" } else { name };
    let path = root.join(format!("completion.{extension}"));
    fs::write(&path, output.stdout).unwrap();
    path
}

fn fixture(root: &Path) -> Repository {
    let repository = Repository::new(root);
    repository.success(&["commit", "--allow-empty", "-m", "base"]);
    for branch in [
        "feat/add-avatars",
        "feat/login-sequence",
        "中文工作",
        "shell;$'\"`&(){}!work",
        "equals=work",
        "mail@work",
        "ps$(ni(SENTINEL))",
    ] {
        repository.success(&["branch", branch]);
    }
    repository.success(&["tag", "v1.2.3"]);
    repository
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[test]
fn fish_native_completion_preserves_literals_and_context() {
    let Some(shell) = shell("fish") else {
        eprintln!("fish runtime unavailable");
        return;
    };
    isolated(
        "fish_native_completion_preserves_literals_and_context",
        true,
        |root| {
            let repository = fixture(root);
            let script = artifact(root, "fish");
            let output = Command::new(&shell).args(["--no-config", "-c", r#"
source "$argv[1]"
for line in 'gof st' 'git-oneflow f --mode int' 'gof s feat/' 'gof start --from-tag v' 'gof s 中' 'gof s \'shell' 'gof s "shell' 'gof s equals=' 'gof s mail@'
    echo CASE
    complete -C "$line"
end
complete -C 'gof save --message (touch SENTINEL)' >/dev/null
"#, script.to_str().unwrap()]).env("PATH", backend_path()).current_dir(repository.path()).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            let cases: Vec<&str> = text.split("CASE\n").skip(1).collect();
            assert_eq!(cases.len(), 9, "{text}");
            for (case, expected) in cases.iter().zip([
                vec!["start", "status"],
                vec!["integrate"],
                vec!["feat/add-avatars", "feat/login-sequence"],
                vec!["v1.2.3"],
                vec!["中文工作"],
                vec!["shell;$'\"`&(){}!work"],
                vec!["shell;$'\"`&(){}!work"],
                vec!["equals=work"],
                vec!["mail@work"],
            ]) {
                let candidates: Vec<&str> = case
                    .lines()
                    .map(|line| line.split('\t').next().unwrap())
                    .collect();
                assert_eq!(candidates, expected, "{text}");
            }
            assert!(!repository.path().join("SENTINEL").exists());
        },
    );
}

#[test]
fn powershell_native_completion_uses_literal_ast_values_and_cursor() {
    let Some(shell) = shell("pwsh") else {
        eprintln!("PowerShell runtime unavailable");
        return;
    };
    isolated(
        "powershell_native_completion_uses_literal_ast_values_and_cursor",
        true,
        |root| {
            let repository = fixture(root);
            let script = artifact(root, "powershell");
            fs::write(repository.path().join("irrelevant"), b"unrelated file").unwrap();
            let test = root.join("test.ps1");
            fs::write(&test, r#"
param($Script)
. $Script
$global:PSNativeCommandArgumentPassing = 'Legacy'
function Capture-Oneflow { $args[0] }
$cases = @('gof st', 'git-oneflow f --mode int', 'gof s feat/', 'gof start --from-tag v', 'gof s 中', "gof s 'shell", 'gof s "shell', 'gof s equals=', 'gof s mail@', 'gof finish --mode ', 'gof s ps')
foreach ($line in $cases) {
    Write-Output 'CASE'
    (TabExpansion2 $line $line.Length).CompletionMatches | ForEach-Object {
        # Test the inserted syntax as an argument to a test-owned capture function.
        # Candidates come from the fixture; no user command line is evaluated.
        $actual = [scriptblock]::Create('Capture-Oneflow ' + $_.CompletionText).Invoke()
        if ($actual.Count -ne 1 -or [string]$actual[0] -cne $_.ListItemText) { throw 'literal insertion changed argument' }
        $_.CompletionText
    }
}
Write-Output 'CASE'
(TabExpansion2 'gof finish --mode int --cleanup keep' 21).CompletionMatches | ForEach-Object { $_.CompletionText }
foreach ($line in @('gof s equals=wZZ tail', 'gof s "equals=wZZ" tail')) {
    $cursor = $line.IndexOf('ZZ')
    $result = TabExpansion2 $line $cursor
    if ($result.CompletionMatches.Count -ne 1) { throw 'missing middle-token candidate' }
    $inserted = $line.Remove($result.ReplacementIndex, $result.ReplacementLength).Insert($result.ReplacementIndex, $result.CompletionMatches[0].CompletionText)
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($inserted, [ref]$tokens, [ref]$errors)
    $elements = $ast.EndBlock.Statements[0].PipelineElements[0].CommandElements
    if ($elements.Count -ne 4 -or $elements[2].Value -cne 'equals=work' -or $elements[3].Value -cne 'tail') { throw 'middle-token replacement changed arguments' }
}
(TabExpansion2 'gof save --message $(New-Item SENTINEL)' 39) | Out-Null
if ($global:PSNativeCommandArgumentPassing -ne 'Legacy') { throw 'completion changed native passing preference' }
foreach ($line in @('gof save --message irr', 'gof start irr', 'git-oneflow start --from-tag irr', 'gof save --message ')) {
    $result = TabExpansion2 $line $line.Length
    if ($result.CompletionMatches.Count -ne 0) { throw "unrelated completion fallback: $line" }
}
"#).unwrap();
            let output = Command::new(&shell)
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-File",
                    test.to_str().unwrap(),
                    script.to_str().unwrap(),
                ])
                .env("PATH", backend_path())
                .current_dir(repository.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            let cases: Vec<&str> = text.split("CASE\n").skip(1).collect();
            assert_eq!(cases.len(), 12, "{text}");
            for (case, expected) in cases.iter().zip([
                vec!["start", "status"],
                vec!["integrate"],
                vec!["feat/add-avatars", "feat/login-sequence"],
                vec!["v1.2.3"],
                vec!["中文工作"],
                vec!["shell;$'\"`&(){}!work"],
                vec!["shell;$'\"`&(){}!work"],
                vec!["equals=work"],
                vec!["mail@work"],
                vec!["integrate", "review"],
                vec!["ps$(ni(SENTINEL))"],
                vec!["integrate"],
            ]) {
                let expected: Vec<String> = expected
                    .into_iter()
                    .map(|value| format!("'{}'", value.replace('\'', "''")))
                    .collect();
                assert_eq!(case.lines().collect::<Vec<_>>(), expected, "{text}");
            }
            assert!(!repository.path().join("SENTINEL").exists());
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn zsh_tab_preserves_literal_names_with_native_completion() {
    let Some(shell) = shell("zsh") else {
        eprintln!("Zsh runtime unavailable");
        return;
    };
    isolated(
        "zsh_tab_preserves_literal_names_with_native_completion",
        true,
        |root| {
            let repository = fixture(root);
            let script = artifact(root, "zsh");
            let capture = root.join("arguments");
            let rc = root.join(".zshrc");
            let mut init = String::new();
            // Relocated test runtimes may carry their own modules and functions.
            // Installed shells use their normal compiled-in search locations.
            let prefix = shell.parent().and_then(Path::parent).unwrap();
            let modules = prefix.join("lib/x86_64-linux-gnu/zsh");
            if let Ok(entries) = fs::read_dir(modules) {
                for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
                    init.push_str(&format!(
                        "module_path=({} $module_path)\n",
                        quote(entry.path().to_str().unwrap())
                    ));
                }
            }
            let functions = prefix.join("share/zsh/functions");
            if functions.is_dir() {
                init.push_str(&format!(
                    "fpath=({}/**/*(/) $fpath)\n",
                    quote(functions.to_str().unwrap())
                ));
            }
            init.push_str(&format!("autoload -Uz compinit\ncompinit -D\nsource {}\nPS1='ONEFLOW_READY> '\ncount=0\ngof() {{ printf '%s\\0' \"$@\" > {}; ((count+=1)); printf 'DONE:%s\\n' \"$count\"; }}\ngit-oneflow() {{ gof \"$@\"; }}\n", quote(script.to_str().unwrap()), quote(capture.to_str().unwrap())));
            fs::write(&rc, init).unwrap();
            let mut command = Command::new("setsid");
            command
                .args(["--ctty", shell.to_str().unwrap(), "-d", "-i"])
                .env("ZDOTDIR", root)
                .env("PATH", backend_path())
                .env("TERM", "xterm-256color")
                .env("LC_ALL", "C.UTF-8")
                .current_dir(repository.path());
            let mut terminal = terminal::Terminal::start_command(command);
            terminal.until("ONEFLOW_READY> ");
            for (index, (line, expected)) in [
                ("gof s 中", "s\0中文工作\0"),
                ("git-oneflow start shell", "start\0shell;$'\"`&(){}!work\0"),
                ("gof s 'shell", "s\0shell;$'\"`&(){}!work\0"),
                ("gof s \"shell", "s\0shell;$'\"`&(){}!work\0"),
                ("gof s equals=", "s\0equals=work\0"),
                ("gof s mail@", "s\0mail@work\0"),
                ("gof f --mode int", "f\0--mode\0integrate\0"),
            ]
            .iter()
            .enumerate()
            {
                terminal.output.clear();
                terminal.send(format!("{line}\t").as_bytes());
                let completed = if index == 0 {
                    "中文工作"
                } else if index == 6 {
                    "integrate"
                } else {
                    "work"
                };
                terminal.until(completed);
                terminal.send(b"\r");
                terminal.until(&format!("DONE:{}", index + 1));
                assert_eq!(
                    fs::read(&capture).unwrap(),
                    expected.as_bytes(),
                    "{line}: {}",
                    terminal.output
                );
            }
            terminal.output.clear();
            terminal.send(b"gof s equals=wZZ tail");
            terminal.send(b"\x01");
            for _ in 0..14 {
                terminal.send(b"\x1b[C");
            }
            terminal.send(b"\t\x05\r");
            terminal.until("DONE:8");
            assert_eq!(
                fs::read(&capture).unwrap(),
                b"s\0equals=wZZ\0tail\0",
                "{}",
                terminal.output
            );
            terminal.output.clear();
            terminal.send(b"gof s \"equals=w\" tail");
            terminal.send(b"\x01");
            for _ in 0..15 {
                terminal.send(b"\x1b[C");
            }
            terminal.send(b"\t");
            terminal.until("ork");
            terminal.send(b"\x05\r");
            terminal.until("DONE:9");
            assert_eq!(
                fs::read(&capture).unwrap(),
                b"s\0equals=work\0tail\0",
                "{}",
                terminal.output
            );
            terminal.send(b"exit\n");
            assert!(terminal.finish().success(), "{}", terminal.output);
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn fish_tab_inserts_literal_names() {
    let Some(shell) = shell("fish") else {
        eprintln!("Fish runtime unavailable");
        return;
    };
    isolated("fish_tab_inserts_literal_names", true, |root| {
        let repository = fixture(root);
        let script = artifact(root, "fish");
        let capture = root.join("arguments");
        let resources = shell
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("share/fish/functions");
        let setup = if resources.is_dir() {
            format!(
                "set -p fish_function_path {}; fish_default_key_bindings; ",
                quote(resources.to_str().unwrap())
            )
        } else {
            String::new()
        };
        let init = format!(
            "{setup}source {}; function fish_prompt; printf 'ONEFLOW_READY> '; end; set -g count 0; function gof; printf '%s\\0' $argv > {}; set -g count (math $count + 1); printf 'DONE:%s\\n' $count; end; function git-oneflow; gof $argv; end",
            quote(script.to_str().unwrap()),
            quote(capture.to_str().unwrap())
        );
        let mut command = Command::new("setsid");
        command
            .args([
                "--ctty",
                shell.to_str().unwrap(),
                "--no-config",
                "-i",
                "-C",
                &init,
            ])
            .env("PATH", backend_path())
            .env("TERM", "xterm-256color")
            .env("LC_ALL", "C.UTF-8")
            .current_dir(repository.path());
        let mut terminal = terminal::Terminal::start_command(command);
        terminal.until("ONEFLOW_READY> ");
        for (index, (line, expected)) in [
            ("gof s 中", "s\0中文工作\0"),
            ("git-oneflow start shell", "start\0shell;$'\"`&(){}!work\0"),
            ("gof s 'shell", "s\0shell;$'\"`&(){}!work\0"),
            ("gof s \"shell", "s\0shell;$'\"`&(){}!work\0"),
            ("gof s equals=", "s\0equals=work\0"),
            ("gof s mail@", "s\0mail@work\0"),
            ("gof f --mode int", "f\0--mode\0integrate\0"),
        ]
        .iter()
        .enumerate()
        {
            terminal.output.clear();
            terminal.send(format!("{line}\t").as_bytes());
            let completed = if index == 0 {
                "中文工作"
            } else if index == 6 {
                "integrate"
            } else {
                "work"
            };
            terminal.until(completed);
            terminal.send(b"\r");
            terminal.until(&format!("DONE:{}", index + 1));
            assert_eq!(
                fs::read(&capture).unwrap(),
                expected.as_bytes(),
                "{line}: {}",
                terminal.output
            );
        }
        terminal.output.clear();
        terminal.send(b"gof s equals=wZZ tail");
        terminal.send(b"\x01");
        for _ in 0..14 {
            terminal.send(b"\x1b[C");
        }
        terminal.send(b"\t\x05\r");
        terminal.until("DONE:8");
        assert_eq!(
            fs::read(&capture).unwrap(),
            b"s\0equals=wZZ\0tail\0",
            "{}",
            terminal.output
        );
        terminal.output.clear();
        terminal.send(b"gof s \"equals=w\" tail");
        terminal.send(b"\x01");
        for _ in 0..15 {
            terminal.send(b"\x1b[C");
        }
        terminal.send(b"\t");
        terminal.until("ork");
        terminal.send(b"\x05\r");
        terminal.until("DONE:9");
        assert_eq!(
            fs::read(&capture).unwrap(),
            b"s\0equals=work\0tail\0",
            "{}",
            terminal.output
        );
        terminal.send(b"exit\n");
        assert!(terminal.finish().success(), "{}", terminal.output);
    });
}

#[test]
fn powershell_host_filter_preserves_custom_delegation_and_registration() {
    let Some(shell) = shell("pwsh") else {
        eprintln!("PowerShell runtime unavailable");
        return;
    };
    isolated(
        "powershell_host_filter_preserves_custom_delegation_and_registration",
        true,
        |root| {
            let repository = fixture(root);
            fs::write(repository.path().join("irrelevant"), b"file").unwrap();
            for name in ["Cargo.toml", "Cargo.lock", "Cargo name"] {
                fs::write(repository.path().join(name), b"redirection target").unwrap();
            }
            let script = artifact(root, "powershell");
            let test = root.join("host-contract.ps1");
            fs::write(&test, r#"
param($Script, $Executable)
Set-StrictMode -Version Latest
$global:__TestOriginalExpansion = (Get-Command TabExpansion2).ScriptBlock
$global:__TestExpansionCalls = 0
function global:TabExpansion2 {
    [CmdletBinding(DefaultParameterSetName='ScriptInputSet')]
    [OutputType([System.Management.Automation.CommandCompletion], [pscustomobject])]
    param(
        [Parameter(ParameterSetName='ScriptInputSet', Mandatory=$true, Position=0)][AllowEmptyString()][string]$inputScript,
        [Parameter(ParameterSetName='ScriptInputSet', Position=1)][int]$cursorColumn = $inputScript.Length,
        [Parameter(ParameterSetName='AstInputSet', Mandatory=$true, Position=0)][System.Management.Automation.Language.Ast]$ast,
        [Parameter(ParameterSetName='AstInputSet', Mandatory=$true, Position=1)][System.Management.Automation.Language.Token[]]$tokens,
        [Parameter(ParameterSetName='AstInputSet', Mandatory=$true, Position=2)][System.Management.Automation.Language.IScriptPosition]$positionOfCursor,
        [Parameter(ParameterSetName='AstInputSet', Position=3)][Parameter(ParameterSetName='ScriptInputSet', Position=2)][hashtable]$options,
        [string]$Marker = 'default'
    )
    $global:__TestExpansionCalls++
    $global:__TestMarker = $Marker
    $global:__TestOptions = $options
    $global:__TestCommonParameter = $PSBoundParameters['ErrorAction']
    if ($Marker -eq 'object') { return [pscustomobject]@{ Value = 'preserved' } }
    $forward = @{} + $PSBoundParameters
    $forward.Remove('Marker')
    & $global:__TestOriginalExpansion @forward
}
# Redirection targets belong to PowerShell. Capture its native results before
# registration rather than prescribing filenames, quoting or replacement ranges.
$redirectionCases = @(
    @{ Line = 'gof status > Cargo'; Cursor = -1 },
    @{ Line = 'gof status > '; Cursor = -1 },
    @{ Line = 'git-oneflow status 2>> Cargo'; Cursor = -1 },
    @{ Line = 'gof status > "Cargo name"'; Cursor = 19 },
    @{ Line = 'gof status > Cargo.lock trailing'; Cursor = 16 }
)
foreach ($case in $redirectionCases) {
    if ($case.Cursor -lt 0) { $case.Cursor = $case.Line.Length }
    $case.Expected = & $global:__TestOriginalExpansion $case.Line $case.Cursor
    if ($case.Expected.CompletionMatches.Count -eq 0) { throw "missing native redirection fixture: $($case.Line)" }
}
. $Script
foreach ($case in $redirectionCases) {
    $result = TabExpansion2 $case.Line $case.Cursor
    $expected = $case.Expected
    $before = @($expected.CompletionMatches | Select-Object CompletionText,ListItemText,ResultType,ToolTip) | ConvertTo-Json -Compress
    $after = @($result.CompletionMatches | Select-Object CompletionText,ListItemText,ResultType,ToolTip) | ConvertTo-Json -Compress
    if ($before -cne $after -or $result.ReplacementIndex -ne $expected.ReplacementIndex -or
        $result.ReplacementLength -ne $expected.ReplacementLength -or
        $result.CurrentMatchIndex -ne $expected.CurrentMatchIndex) { throw "redirection completion changed: $($case.Line) at $($case.Cursor)" }
}
# A completed redirection does not turn later application arguments into paths.
$result = TabExpansion2 'gof > output sta'
if ((@($result.CompletionMatches | ForEach-Object ListItemText) -join ',') -cne 'start,status') { throw 'argument completion after redirection changed' }
$result = TabExpansion2 'gof > output start irr'
if ($result.CompletionMatches.Count -ne 0) { throw 'argument fallback after redirection' }
$first = (Get-Command TabExpansion2).ScriptBlock
. $Script
if (-not [object]::ReferenceEquals($first, (Get-Command TabExpansion2).ScriptBlock)) { throw 'duplicate wrapper' }
if (-not (Get-Command TabExpansion2).Parameters.ContainsKey('Marker')) { throw 'custom parameter lost' }
if ((Get-Command TabExpansion2).OutputType.Count -ne 2) { throw 'output type metadata lost' }
# Similar names and a new command after a newline retain native file suggestions.
Set-Content -LiteralPath 'gof.txt' -Value 'fixture'
Set-Content -LiteralPath 'gof.ps1' -Value 'fixture'
foreach ($line in @('./gof.txt irr', './gof.ps1 irr', "gof start irr`n", 'gof save --message $(echo irr')) {
    $expected = & $global:__TestOriginalExpansion $line $line.Length
    $result = TabExpansion2 $line $line.Length
    $before = @($expected.CompletionMatches | ForEach-Object { $_.CompletionText }) -join "`0"
    $after = @($result.CompletionMatches | ForEach-Object { $_.CompletionText }) -join "`0"
    if ($before -cne $after -or $result.ReplacementIndex -ne $expected.ReplacementIndex -or
        $result.ReplacementLength -ne $expected.ReplacementLength) { throw "unrelated context changed: $line" }
}
$line = 'gof start irr'
$opts = @{ RelativePaths = $false }
$result = TabExpansion2 $line -options $opts -Marker custom -ErrorAction Stop
if ($result.CompletionMatches.Count -ne 0 -or $global:__TestMarker -ne 'custom' -or
    -not [object]::ReferenceEquals($opts, $global:__TestOptions) -or $global:__TestCommonParameter -ne 'Stop') { throw 'custom delegation changed' }
$parsed = [System.Management.Automation.CommandCompletion]::MapStringInputToParsedInput($line, $line.Length)
$result = TabExpansion2 -ast $parsed.Item1 -tokens $parsed.Item2 -positionOfCursor $parsed.Item3 -Marker ast
if ($result.CompletionMatches.Count -ne 0 -or $global:__TestMarker -ne 'ast') { throw 'AST overload changed' }
$line = "& '$($Executable.Replace("'", "''"))' start irr"
$result = TabExpansion2 $line
if ($result.CompletionMatches.Count -ne 0) { throw 'full path fallback' }
$line = 'echo irr'
$result = TabExpansion2 $line
if (-not ($result.CompletionMatches | Where-Object ResultType -eq ProviderItem)) { throw 'unrelated file completion changed' }
$line = "& './irr'"
$result = TabExpansion2 $line
if (-not ($result.CompletionMatches | Where-Object ResultType -eq ProviderItem)) { throw 'command-position completion changed' }
$result = TabExpansion2 'gof start irr' -Marker object
if ($result.Value -ne 'preserved') { throw 'custom output changed' }
# A subsequent user replacement is retained as the next wrapper's delegate.
$previous = (Get-Command TabExpansion2).ScriptBlock
$global:__TestReplacementCalls = 0
$replacement = {
    [CmdletBinding()]
    param([string]$inputScript, [int]$cursorColumn = $inputScript.Length, [hashtable]$options, [string]$Marker = 'default')
    $global:__TestReplacementCalls++
    & $previous @PSBoundParameters
}.GetNewClosure()
Set-Item Function:global:TabExpansion2 $replacement
. $Script
$before = $global:__TestExpansionCalls
$result = TabExpansion2 'gof start irr' -Marker replacement
if ($result.CompletionMatches.Count -ne 0 -or $global:__TestReplacementCalls -ne 1 -or
    $global:__TestExpansionCalls -ne $before + 1 -or $global:__TestMarker -ne 'replacement') { throw 'replacement delegate recursion or loss' }
"#).unwrap();
            let output = Command::new(shell)
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-File",
                    test.to_str().unwrap(),
                    script.to_str().unwrap(),
                    env!("CARGO_BIN_EXE_git-oneflow"),
                ])
                .env("PATH", backend_path())
                .current_dir(repository.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
fn powershell_console_tab_preserves_native_arguments_and_empty_results() {
    let Some(shell) = shell("pwsh") else {
        eprintln!("PowerShell runtime unavailable");
        return;
    };
    isolated(
        "powershell_console_tab_preserves_native_arguments_and_empty_results",
        true,
        |root| {
            use std::os::unix::fs::PermissionsExt;
            let repository = fixture(root);
            fs::write(repository.path().join("irrelevant"), b"file").unwrap();
            let script = artifact(root, "powershell");
            let capture = root.join("arguments");
            let bin = root.join("bin");
            fs::create_dir(&bin).unwrap();
            let stub = bin.join("gof");
            fs::write(&stub, "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$ONEFLOW_COMPLETION_CAPTURE\"\nprintf 'CAPTURE_DONE\\n'\n").unwrap();
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
            let mut paths = vec![bin];
            paths.extend(std::env::split_paths(&backend_path()));
            let init = root.join("console.ps1");
            fs::write(
                &init,
                format!(
                    ". '{}'\nfunction global:prompt {{ 'ONEFLOW_READY> ' }}\n",
                    script.to_str().unwrap().replace('\'', "''")
                ),
            )
            .unwrap();
            let mut command = Command::new("setsid");
            command
                .args([
                    "--ctty",
                    shell.to_str().unwrap(),
                    "-NoLogo",
                    "-NoProfile",
                    "-NoExit",
                    "-File",
                    init.to_str().unwrap(),
                ])
                .env("PATH", std::env::join_paths(paths).unwrap())
                .env("ONEFLOW_COMPLETION_CAPTURE", &capture)
                .env("TERM", "xterm-256color")
                .env("LC_ALL", "C.UTF-8")
                .current_dir(repository.path());
            let mut terminal = terminal::Terminal::start_command(command);
            terminal.until_with_cursor_reports("ONEFLOW_READY> ");
            for (line, expected) in [
                ("gof s 中", "s\0中文工作\0"),
                ("gof s shell", "s\0shell;$'\"`&(){}!work\0"),
                ("gof start irr", "start\0irr\0"),
            ] {
                terminal.output.clear();
                terminal.send(format!("{line}\t\r").as_bytes());
                terminal.until_with_cursor_reports("CAPTURE_DONE");
                assert_eq!(
                    fs::read(&capture).unwrap(),
                    expected.as_bytes(),
                    "{}",
                    terminal.output
                );
                terminal.until_with_cursor_reports("ONEFLOW_READY> ");
            }
            terminal.send(b"exit\r");
            assert!(
                terminal.finish_with_cursor_reports().success(),
                "{}",
                terminal.output
            );
        },
    );
}
