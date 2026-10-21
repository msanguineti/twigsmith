//! Executable-owned user completion registration. Profile bytes remain opaque;
//! only an exact current managed block supplies authority to remove registration.
use std::{
    env,
    ffi::OsString,
    fs::{self, Permissions},
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

const BEGIN: &str = "# >>> git-oneflow completion";
const END: &str = "# <<< git-oneflow completion";
const SHELLS: [&str; 4] = ["bash", "zsh", "fish", "powershell"];

struct Request {
    action: Action,
    shell: String,
    file: Option<PathBuf>,
    yes: bool,
}
#[derive(Clone, Copy, PartialEq)]
enum Action {
    Session,
    Install,
    Uninstall,
}

pub fn run(args: &[OsString]) -> ExitCode {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }
    let request = match parse(args) {
        Ok(request) => request,
        Err(error) => {
            crate::presentation::error(format!("error: {error}; use completion --help"));
            return ExitCode::from(2);
        }
    };
    if request.action == Action::Session {
        return match crate::completion_shells::script(&request.shell) {
            Ok(script) => match io::stdout().lock().write_all(script.as_bytes()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => fail(error),
            },
            Err(error) => fail(error),
        };
    }
    match setup(&request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(error),
    }
}

fn fail(error: impl std::fmt::Display) -> ExitCode {
    crate::presentation::error(format!("error: {error}"));
    ExitCode::FAILURE
}

fn parse(args: &[OsString]) -> Result<Request, String> {
    let (action, args) = match args.first().and_then(|arg| arg.to_str()) {
        Some("install") => (Action::Install, &args[1..]),
        Some("uninstall") => (Action::Uninstall, &args[1..]),
        _ => (Action::Session, args),
    };
    let mut shell = None;
    let mut file = None;
    let mut yes = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--yes" && action != Action::Session && !yes {
            yes = true;
        } else if arg == "--file" && action != Action::Session && file.is_none() {
            let value = args.next().ok_or("--file requires a path")?;
            if value.is_empty() || value.to_str().is_some_and(|value| value.starts_with("--")) {
                return Err("--file requires a path".into());
            }
            file = Some(PathBuf::from(value));
        } else if let Some(value) = arg.to_str().and_then(|arg| arg.strip_prefix("--file=")) {
            if action == Action::Session || file.is_some() || value.is_empty() {
                return Err("invalid --file option".into());
            }
            file = Some(PathBuf::from(value));
        } else if shell.is_none() && arg.to_str().is_some_and(|arg| SHELLS.contains(&arg)) {
            shell = arg.to_str().map(str::to_owned);
        } else {
            return Err(format!("unsupported argument {}", display_os(arg)));
        }
    }
    let shell = shell.ok_or("select bash, zsh, fish, or powershell explicitly")?;
    if shell == "powershell" && action != Action::Session && file.is_none() {
        return Err("PowerShell requires --file PATH; pass --file $PROFILE in PowerShell".into());
    }
    Ok(Request {
        action,
        shell,
        file,
        yes,
    })
}

fn display_os(value: &std::ffi::OsStr) -> String {
    crate::status::display(value.as_encoded_bytes())
}

fn home() -> io::Result<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("HOME is unavailable; select a profile with --file PATH"))
}

fn profile(request: &Request) -> io::Result<PathBuf> {
    let path = if let Some(path) = &request.file {
        if request.shell == "fish" && path.is_dir() {
            path.join("conf.d/git-oneflow.fish")
        } else {
            path.clone()
        }
    } else {
        match request.shell.as_str() {
            "bash" => home()?.join(".bashrc"),
            "zsh" => env::var_os("ZDOTDIR")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .map_or_else(home, Ok)?
                .join(".zshrc"),
            "fish" => env::var_os("XDG_CONFIG_HOME")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .map_or_else(|| home().map(|home| home.join(".config")), Ok)?
                .join("fish/conf.d/git-oneflow.fish"),
            _ => unreachable!("explicit PowerShell profile parsed above"),
        }
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn block(shell: &str) -> Vec<u8> {
    let loader = match shell {
        "bash" | "zsh" => format!(
            "if command -v git-oneflow >/dev/null 2>&1; then\n    eval \"$(command git-oneflow completion {shell} 2>/dev/null)\"\nelif command -v gof >/dev/null 2>&1; then\n    eval \"$(command gof completion {shell} 2>/dev/null)\"\nfi"
        ),
        "fish" => "if command -sq git-oneflow\n    command git-oneflow completion fish | source\nelse if command -sq gof\n    command gof completion fish | source\nend".into(),
        "powershell" => "if (Get-Command git-oneflow -CommandType Application -ErrorAction SilentlyContinue) {\n    & (Get-Command git-oneflow -CommandType Application).Source completion powershell | Out-String | Invoke-Expression\n} elseif (Get-Command gof -CommandType Application -ErrorAction SilentlyContinue) {\n    & (Get-Command gof -CommandType Application).Source completion powershell | Out-String | Invoke-Expression\n}".into(),
        _ => unreachable!(),
    };
    // Own the preceding separator too, so removing an appended block restores
    // files that originally lacked a final newline exactly.
    format!("\n{BEGIN} {shell} >>>\n{loader}\n{END} {shell} <<<\n").into_bytes()
}

fn occurrences(bytes: &[u8], needle: &[u8]) -> Vec<usize> {
    bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, value)| (value == needle).then_some(index))
        .collect()
}

fn replacement(bytes: &[u8], owned: &[u8], action: Action) -> io::Result<Option<Vec<u8>>> {
    let starts = occurrences(bytes, BEGIN.as_bytes());
    let ends = occurrences(bytes, END.as_bytes());
    if starts.is_empty() && ends.is_empty() {
        return if action == Action::Install {
            let mut result = bytes.to_vec();
            result.extend_from_slice(owned);
            Ok(Some(result))
        } else {
            Ok(None)
        };
    }
    let exact = occurrences(bytes, owned);
    if starts.len() != 1 || ends.len() != 1 || exact.len() != 1 {
        return Err(io::Error::other(
            "completion markers are edited, duplicated, malformed, or belong to another shell; inspect the profile and remove or repair that registration manually",
        ));
    }
    if action == Action::Install {
        return Ok(None);
    }
    let index = exact[0];
    let mut result = bytes[..index].to_vec();
    result.extend_from_slice(&bytes[index + owned.len()..]);
    Ok(Some(result))
}

struct Snapshot {
    bytes: Option<Vec<u8>>,
    permissions: Option<Permissions>,
    #[cfg(unix)]
    identity: Option<(u64, u64, u32, u32, u32)>,
    #[cfg(target_os = "linux")]
    extended_metadata: bool,
}

fn reject_symlinks(path: &Path) -> io::Result<()> {
    for component in path.ancestors() {
        match fs::symlink_metadata(component) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "{} is a symlink; use --file with its actual regular-file location",
                    display_os(component.as_os_str())
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn snapshot(path: &Path) -> io::Result<Snapshot> {
    reject_symlinks(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Opening a substituted FIFO must not wait for a writer; opening a
        // substituted symlink must not follow it before revalidation.
        options.custom_flags(
            (rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW).bits() as i32,
        );
    }
    #[cfg(not(target_os = "linux"))]
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(io::Error::other(
            "the profile location is not a regular file; select --file PATH",
        ));
    }
    let mut file = match options.open(path) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let metadata = file.as_ref().map(fs::File::metadata).transpose()?;
    if metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.is_file())
    {
        return Err(io::Error::other(
            "the profile location is not a regular file; select --file PATH",
        ));
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        if metadata
            .as_ref()
            .is_some_and(|metadata| metadata.nlink() > 1)
        {
            return Err(io::Error::other(
                "the profile has multiple hard links; select an independent regular file with --file PATH",
            ));
        }
        metadata.as_ref().map(|metadata| {
            (
                metadata.dev(),
                metadata.ino(),
                metadata.mode(),
                metadata.uid(),
                metadata.gid(),
            )
        })
    };
    #[cfg(target_os = "linux")]
    let extended_metadata = file
        .as_ref()
        .map(has_extended_metadata)
        .transpose()?
        .unwrap_or(false);
    let bytes = file
        .as_mut()
        .map(|file| {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        })
        .transpose()?;
    Ok(Snapshot {
        bytes,
        permissions: metadata.map(|metadata| metadata.permissions()),
        #[cfg(unix)]
        identity,
        #[cfg(target_os = "linux")]
        extended_metadata,
    })
}

fn revalidate(path: &Path, approved: &Snapshot) -> io::Result<()> {
    let current = snapshot(path)?;
    let equal = current.bytes == approved.bytes
        && current.permissions.as_ref().map(Permissions::readonly)
            == approved.permissions.as_ref().map(Permissions::readonly);
    #[cfg(unix)]
    let equal = equal && current.identity == approved.identity;
    #[cfg(target_os = "linux")]
    let equal = equal && current.extended_metadata == approved.extended_metadata;
    if equal {
        Ok(())
    } else {
        Err(io::Error::other(
            "the profile changed after review; no registration was written; review it and run the command again",
        ))
    }
}

#[cfg(target_os = "linux")]
fn has_extended_metadata(file: &fs::File) -> io::Result<bool> {
    // A zero-length initialized buffer queries the required name-list length.
    // POSIX access ACLs are included in Linux's extended attribute list.
    rustix::fs::flistxattr(file, &mut [0_u8; 0])
        .map(|length| length != 0)
        .map_err(|error| io::Error::other(format!(
            "could not inspect profile extended metadata: {error}; use session completion or select an ordinary UTF-8 startup file with --file PATH"
        )))
}

#[cfg(target_os = "linux")]
fn reject_extended_metadata(present: bool) -> io::Result<()> {
    if present {
        Err(io::Error::other(
            "the profile or staged replacement has extended metadata (including ACLs); use session completion or select an ordinary UTF-8 startup file with --file PATH",
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
fn update(_path: &Path, _approved: &Snapshot, _bytes: &[u8]) -> io::Result<()> {
    Err(io::Error::other(
        "automatic profile replacement requires Linux metadata inspection; use session completion or add its command to your startup file manually",
    ))
}

#[cfg(target_os = "linux")]
fn update(path: &Path, approved: &Snapshot, bytes: &[u8]) -> io::Result<()> {
    use rustix::{
        fs::fchown,
        process::{Gid, Uid},
    };
    use std::os::unix::fs::MetadataExt;

    revalidate(path, approved)?;
    reject_extended_metadata(approved.extended_metadata)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("profile has no parent directory"))?;
    fs::create_dir_all(parent)?;
    reject_symlinks(path)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    // Default directory ACLs can affect new files even when the existing profile
    // has no ACL. Refuse before chmod could obscure that inherited access policy.
    reject_extended_metadata(has_extended_metadata(staged.as_file())?)?;
    staged.write_all(bytes)?;
    if let Some((_, _, _, uid, gid)) = approved.identity {
        let metadata = staged.as_file().metadata()?;
        if (metadata.uid(), metadata.gid()) != (uid, gid) {
            fchown(
                staged.as_file(),
                Some(Uid::from_raw(uid)),
                Some(Gid::from_raw(gid)),
            )
            .map_err(|error| {
                io::Error::other(format!("could not preserve profile owner/group: {error}"))
            })?;
        }
        let metadata = staged.as_file().metadata()?;
        if (metadata.uid(), metadata.gid()) != (uid, gid) {
            return Err(io::Error::other("could not preserve profile owner/group"));
        }
    }
    // chown may clear set-ID bits; restore the approved mode afterward.
    if let Some(permissions) = &approved.permissions {
        staged.as_file().set_permissions(permissions.clone())?;
    }
    staged.as_file().sync_all()?;
    reject_extended_metadata(has_extended_metadata(staged.as_file())?)?;
    revalidate(path, approved)?;
    if approved.bytes.is_some() {
        staged.persist(path).map_err(|error| error.error)?;
    } else {
        staged
            .persist_noclobber(path)
            .map_err(|error| error.error)?;
    }
    Ok(())
}

fn setup(request: &Request) -> io::Result<()> {
    let path = profile(request)?;
    let approved = snapshot(&path)?;
    if approved.bytes.as_ref().is_some_and(|bytes| {
        bytes.contains(&0) || bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff])
    }) {
        return Err(io::Error::other(
            "the profile uses UTF-16/UTF-32 or contains NUL bytes; select a UTF-8 shell profile with --file, or convert this profile explicitly before registration",
        ));
    }
    let owned = block(&request.shell);
    let Some(bytes) = replacement(
        approved.bytes.as_deref().unwrap_or_default(),
        &owned,
        request.action,
    )?
    else {
        println!(
            "{} completion is already {} at {}.",
            request.shell,
            if request.action == Action::Install {
                "registered"
            } else {
                "absent"
            },
            display_os(path.as_os_str())
        );
        if request.action == Action::Install {
            activation(&request.shell, &path);
        }
        return Ok(());
    };
    let action = if request.action == Action::Install {
        "install"
    } else {
        "remove"
    };
    eprintln!(
        "{} completion: {action} registration at {}\n\nExact block to {action}:{}",
        request.shell,
        display_os(path.as_os_str()),
        String::from_utf8_lossy(&owned)
    );
    if !request.yes {
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            return Err(io::Error::other(
                "registration requires terminal approval; use --yes to authorize this profile change",
            ));
        }
        let choices = [
            (action, "Apply the displayed profile change"),
            ("cancel", "Keep the profile unchanged"),
        ];
        if crate::presentation::select_described("Apply completion registration?", &choices, None)?
            .as_deref()
            != Some(action)
        {
            println!("Completion registration cancelled; profile unchanged.");
            return Ok(());
        }
    }
    update(&path, &approved, &bytes).map_err(|error| io::Error::other(format!(
        "could not replace the profile at {}; existing profile contents were preserved: {error}. Resolve the path or permission problem and review the command again",
        display_os(path.as_os_str())
    )))?;
    println!(
        "{} completion registration {} at {}.",
        request.shell,
        if request.action == Action::Install {
            "installed"
        } else {
            "removed"
        },
        display_os(path.as_os_str())
    );
    if request.action == Action::Install {
        activation(&request.shell, &path);
        println!(
            "Each shell startup loads completion from git-oneflow or gof on PATH, including executable upgrades. After moving the executable, update PATH before opening the shell."
        );
    } else {
        println!("Open a new shell to stop using the previously loaded completion.");
    }
    Ok(())
}

fn activation(shell: &str, path: &Path) {
    println!(
        "Open a new {shell} session that reads {}. To enable completion in this session, use the command in completion --help.",
        display_os(path.as_os_str())
    );
}

fn print_help() {
    println!(
        "Generate, register, or remove shell completion for git-oneflow and gof.

Usage: git-oneflow completion <SHELL>
       git-oneflow completion install <SHELL> [OPTIONS]
       git-oneflow completion uninstall <SHELL> [OPTIONS]

Arguments:
  SHELL            bash, zsh, fish, or powershell; select your active shell

Options:
  --file PATH      Select the exact startup/profile file (install/uninstall)
                   For Fish, an existing configuration directory selects
                   its conf.d/git-oneflow.fish file
  --yes            Authorize the profile change without a prompt (install/uninstall)
  -h, --help       Show help

Examples:
  Enable in the current session:
  Bash:        eval \"$(gof completion bash)\"
  Zsh:         eval \"$(gof completion zsh)\"
  Fish:        gof completion fish | source
  PowerShell:  gof completion powershell | Out-String | Invoke-Expression

  Persist or remove registration:
  gof completion install bash
  gof completion install zsh --file ~/.config/zsh/.zshrc --yes
  gof completion install fish
  gof completion install powershell --file \"$PROFILE\"
  gof completion uninstall bash

Details:
  Session generation prints only a shell script and changes no profile.
  PowerShell completion requires PowerShell 7.3 or newer.
  It wraps TabExpansion2 to suppress filename fallback for this tool, delegates
  other commands to the prior function, and preserves general key bindings.
  Microsoft treats this host customization as unsupported. Hosts that bypass
  TabExpansion2 retain their native fallback. Runtime coverage is on Linux;
  Windows and macOS have not been validated.
  In Bash, complete with the cursor at the word end; Tab leaves trailing text
  after the cursor unchanged.
  Bash defaults to $HOME/.bashrc; Zsh uses $ZDOTDIR/.zshrc or $HOME/.zshrc.
  Fish uses $XDG_CONFIG_HOME/fish/conf.d/git-oneflow.fish, falling back to
  $HOME/.config/fish/conf.d/git-oneflow.fish. PowerShell requires --file:
  pass the actual $PROFILE, including any custom location.
  Installation shows the exact managed block and location before approval.
  Repeat installation preserves a matching registration. Removal deletes only
  an exact owned block; edited or duplicate markers require manual repair.
  Automatic profile changes require Linux metadata inspection and preserve
  owner, group and permission bits. Extended attributes or ACLs on the profile
  or inherited by its replacement require session completion or an ordinary
  UTF-8 startup file selected with --file. Other platforms can use session
  generation and add the command to their startup file manually.
  Symlink profiles require --file with the actual regular-file location.
  UTF-16/UTF-32 and NUL-containing profiles require explicit conversion or
  selection of a UTF-8 profile; their contents are preserved on refusal.
  Open a new shell that reads the selected file, or run the session command above.
  Bash login shells must source .bashrc from their login profile to read it.
  Custom files must already be sourced by the shell. Zsh's native
  completion system must be initialized first with your existing compinit setup.
  If needed, run autoload -Uz compinit; compinit before enabling Zsh completion.
  General shell key bindings stay unchanged.
  Startup regenerates completion from git-oneflow or gof on PATH. Upgrades use
  the new executable automatically; after moving it, update PATH. If neither
  name is on PATH, registration is inactive. No shell or execution policy changes
  are installed.

Exit status:
  0  Script generated, registration applied/already matching, or cancelled
  1  Profile, script generation, or approval failure
  2  Invalid syntax"
    );
}
