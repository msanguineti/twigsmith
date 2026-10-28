//! Native-host session contracts. Linux owns interactive terminal coverage;
//! these tests check the adapters and literal shell syntax on their native OS.
#![cfg(any(target_os = "macos", windows))]

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::{Repository, isolated};

const LITERAL: &str = "literal;$'&()work";

fn runtime(name: &str) -> PathBuf {
    let variable = format!("TWIGSMITH_TEST_{}", name.to_uppercase());
    if let Some(path) = std::env::var_os(variable) {
        return path.into();
    }
    let filename = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(&filename))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("required native {name} runtime is unavailable"))
}

fn backend_path() -> std::ffi::OsString {
    let mut paths = vec![
        Path::new(env!("CARGO_BIN_EXE_twigsmith"))
            .parent()
            .unwrap()
            .to_owned(),
    ];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

fn fixture(root: &Path) -> Repository {
    let repository = Repository::new(root);
    repository.success(&["commit", "--allow-empty", "-m", "base"]);
    for branch in ["feat/first", "feat/second", LITERAL, "ps$(ni(SENTINEL))"] {
        repository.success(&["branch", branch]);
    }
    repository.success(&["tag", "v1.2.3"]);
    fs::write(repository.path().join("irrelevant"), "unrelated file").unwrap();
    repository
}

fn artifact(root: &Path, shell: &str) -> PathBuf {
    let mut scripts = Vec::new();
    for executable in [env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")] {
        let output = Command::new(executable)
            .args(["completion", shell])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        scripts.push(output.stdout);
    }
    assert_eq!(scripts[0], scripts[1], "executable session scripts differ");
    let path = root.join("completion-script.ps1");
    fs::write(&path, &scripts[0]).unwrap();
    path
}

fn run_script(root: &Path, repository: &Repository, shell: &str, script: &str) {
    let adapter = artifact(root, shell);
    let test = root.join("session-test.ps1");
    fs::write(&test, script).unwrap();
    let mut command = Command::new(runtime(if shell == "powershell" { "pwsh" } else { shell }));
    match shell {
        "bash" => {
            command.args(["--noprofile", "--norc"]);
        }
        "zsh" => {
            command.arg("-f");
        }
        "fish" => {
            command.arg("--no-config");
        }
        "powershell" => {
            command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"]);
        }
        _ => unreachable!(),
    }
    let output = command
        .arg(test)
        .arg(adapter)
        .env("PATH", backend_path())
        .env("TWIGSMITH_LITERAL", LITERAL)
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{shell}: {output:?}");
    assert!(output.stderr.is_empty(), "{shell}: {output:?}");
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "NATIVE_OK\n"
    );
    assert!(!repository.path().join("SENTINEL").exists());
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires native Bash environment; selected by ./dev"]
fn macos_bash_session_candidates_and_literal_syntax() {
    isolated(
        "macos_bash_session_candidates_and_literal_syntax",
        true,
        |root| {
            let repository = fixture(root);
            run_script(
                root,
                &repository,
                "bash",
                r#"
source "$1" || exit 1
COMP_WORDBREAKS=$' \t\n"\'@><=;|&(:'
check() {
    COMP_LINE=$1; COMP_POINT=${#COMP_LINE}
    _twigsmith "${COMP_LINE%% *}"
    local actual
    actual=$(printf '%s\n' "${COMPREPLY[@]}")
    [[ $actual == "$2" ]] || { printf 'unexpected candidates: %s\n' "$actual" >&2; exit 1; }
}
check 'tgi st' $'start\nstatus'
check 'twigsmith f --mode int' integrate
check 'tgi s feat/' $'feat/first\nfeat/second'
check 'twigsmith start --from-tag v' v1.2.3
check 'tgi s literal' "literal\\;\\\$\\'\\&\\(\\)work"
# Evaluate only fixture-owned returned insertion syntax, never a typed command.
eval "set -- ${COMPREPLY[0]}"
[[ $# == 1 && $1 == "$TWIGSMITH_LITERAL" ]] || exit 1
check 'tgi save --message irr' ''
check 'tgi start irr' ''
check 'tgi start --from-tag irr' ''
check 'tgi save --message $(touch SENTINEL)' ''
for name in tgi twigsmith; do
    registration=$(complete -p "$name")
    [[ $registration == "complete -o noquote -F _twigsmith $name" ]] || exit 1
done
printf 'NATIVE_OK\n'
"#,
            );
        },
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires native Zsh environment; selected by ./dev"]
fn macos_zsh_session_candidates_and_literal_syntax() {
    isolated(
        "macos_zsh_session_candidates_and_literal_syntax",
        true,
        |root| {
            let repository = fixture(root);
            run_script(
                root,
                &repository,
                "zsh",
                r#"
autoload -Uz compinit
compinit -u -D
source "$1" || exit 1
[[ ${_comps[tgi]} == _twigsmith && ${_comps[twigsmith]} == _twigsmith ]] || exit 1
# Capture candidate transport at compadd; Linux PTY tests exercise the editor.
compadd() { shift; printf '%s\n' "$@"; }
check() {
    words=("${(@s: :)1}"); CURRENT=${#words}; PREFIX=$words[-1]
    local actual=$(_twigsmith)
    [[ $actual == "$2" ]] || { print -ru2 -- "unexpected candidates: $actual"; exit 1; }
}
check 'tgi st' $'start\nstatus'
check 'twigsmith f --mode int' integrate
check 'tgi s feat/' $'feat/first\nfeat/second'
check 'twigsmith start --from-tag v' v1.2.3
check 'tgi s literal' "$TWIGSMITH_LITERAL"
# Use Zsh's literal quoting and parse that fixture-owned insertion as one word.
value=$TWIGSMITH_LITERAL
quoted=${(q)value}
eval "set -- $quoted"
[[ $# == 1 && $1 == "$TWIGSMITH_LITERAL" ]] || exit 1
check 'tgi save --message irr' ''
check 'tgi start irr' ''
check 'tgi start --from-tag irr' ''
words=(tgi save --message '$(touch SENTINEL)'); CURRENT=4; PREFIX=$words[-1]
[[ -z $(_twigsmith) ]] || exit 1
print -r -- NATIVE_OK
"#,
            );
        },
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires native Fish environment; selected by ./dev"]
fn macos_fish_session_candidates_and_literal_syntax() {
    isolated(
        "macos_fish_session_candidates_and_literal_syntax",
        true,
        |root| {
            let repository = fixture(root);
            run_script(
                root,
                &repository,
                "fish",
                r#"
source $argv[1]; or exit 1
function check
    set -l actual (complete -C "$argv[1]" | string split -f 1 \t)
    set -e argv[1]
    test (count $actual) -eq (count $argv); or exit 1
    set -l i 1
    while test $i -le (count $argv)
        test "$actual[$i]" = "$argv[$i]"; or exit 1
        set i (math $i + 1)
    end
end
check 'tgi st' start status
check 'twigsmith f --mode int' integrate
check 'tgi s feat/' feat/first feat/second
check 'twigsmith start --from-tag v' v1.2.3
check 'tgi s literal' "$TWIGSMITH_LITERAL"
set -l insertion (string escape -- "$TWIGSMITH_LITERAL")
set -l actual (eval "printf '%s' $insertion")
test "$actual" = "$TWIGSMITH_LITERAL"; or exit 1
check 'tgi save --message irr'
check 'tgi start irr'
check 'tgi start --from-tag irr'
check 'tgi save --message (touch SENTINEL)'
printf 'NATIVE_OK\n'
"#,
            );
        },
    );
}

#[cfg(windows)]
#[test]
#[ignore = "requires native PowerShell environment; selected after runtime detection"]
fn windows_powershell_session_candidates_and_literal_insertion() {
    isolated(
        "windows_powershell_session_candidates_and_literal_insertion",
        true,
        |root| {
            let repository = fixture(root);
            run_script(
                root,
                &repository,
                "powershell",
                r#"
param($Adapter)
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion -lt [version]'7.3') { throw 'PowerShell 7.3 or newer required' }
. $Adapter
$global:PSNativeCommandArgumentPassing = 'Legacy'
function Capture-Oneflow { $args }
function Assert-Candidates([string]$Line, [string[]]$Expected) {
    $matches = @((TabExpansion2 $Line $Line.Length).CompletionMatches)
    if ($matches.Count -ne $Expected.Count) { throw "unexpected candidate count: $Line" }
    for ($i = 0; $i -lt $matches.Count; $i++) {
        if ($matches[$i].ListItemText -cne $Expected[$i]) { throw "unexpected candidate: $Line" }
        # Evaluate only fixture-owned insertion syntax, never the completed command.
        $actual = @([scriptblock]::Create('Capture-Oneflow ' + $matches[$i].CompletionText).Invoke())
        if ($actual.Count -ne 1 -or [string]$actual[0] -cne $Expected[$i]) { throw 'literal insertion changed argument' }
    }
}
Assert-Candidates 'tgi.exe st' @('start', 'status')
Assert-Candidates 'twigsmith.exe f --mode int' @('integrate')
Assert-Candidates 'tgi s feat/' @('feat/first', 'feat/second')
Assert-Candidates 'twigsmith start --from-tag v' @('v1.2.3')
Assert-Candidates 'tgi s literal' @($env:TWIGSMITH_LITERAL)
Assert-Candidates 'tgi s ps' @('ps$(ni(SENTINEL))')
foreach ($line in @('tgi save --message irr', 'tgi start irr', 'twigsmith start --from-tag irr', 'tgi save --message $(New-Item SENTINEL)')) {
    Assert-Candidates $line @()
}
if ($global:PSNativeCommandArgumentPassing -ne 'Legacy') { throw 'changed native passing preference' }
Write-Output 'NATIVE_OK'
"#,
            );
        },
    );
}

#[test]
fn native_profile_install_refuses_without_changing_bytes() {
    isolated(
        "native_profile_install_refuses_without_changing_bytes",
        false,
        |root| {
            // macOS temporary roots can pass through /var -> /private/var.
            // Select the real regular-file location so the platform refusal is tested.
            let profile = root.canonicalize().unwrap().join("profile with spaces");
            let original = b"# user profile\r\n# preserved without a final newline";
            fs::write(&profile, original).unwrap();
            for executable in [env!("CARGO_BIN_EXE_twigsmith"), env!("CARGO_BIN_EXE_tgi")] {
                for shell in ["bash", "zsh", "fish", "powershell"] {
                    let output = Command::new(executable)
                        .args(["completion", "install", shell, "--file"])
                        .arg(&profile)
                        .arg("--yes")
                        .output()
                        .unwrap();
                    assert!(!output.status.success(), "{output:?}");
                    assert!(
                        String::from_utf8_lossy(&output.stderr)
                            .contains("requires Linux metadata inspection"),
                        "{output:?}"
                    );
                    assert_eq!(fs::read(&profile).unwrap(), original);
                }
            }
        },
    );
}
