//! Repository-shared configuration for start and save.
use crate::{
    git,
    repository::{InspectionError, RepositoryContext},
};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveAction {
    Ask,
    PublishOnly,
    Staged,
    Wip,
    Stash,
}

#[derive(Debug)]
pub enum SettingsError {
    Repository(InspectionError),
    SettingsIo { path: PathBuf, source: io::Error },
    InvalidSettings(String),
}
impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(e) => write!(f, "{e}"),
            Self::SettingsIo { path, source } => {
                write!(f, "cannot read settings {}: {source}", path.display())
            }
            Self::InvalidSettings(reason) => {
                write!(f, "invalid {SETTINGS_FILE} settings: {reason}")
            }
        }
    }
}
impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Repository(e) => Some(e),
            Self::SettingsIo { source, .. } => Some(source),
            _ => None,
        }
    }
}
impl From<InspectionError> for SettingsError {
    fn from(e: InspectionError) -> Self {
        Self::Repository(e)
    }
}

/// Read and validate syntax locally; remote existence belongs to the workflow.
pub fn load(directory: &Path, repository: &RepositoryContext) -> Result<Settings, SettingsError> {
    let settings = read(repository)?;
    if !valid_remote(directory, &settings.remote)? {
        return Err(SettingsError::InvalidSettings(format!(
            "invalid remote name: {:?}",
            settings.remote
        )));
    }
    if !valid_branch(directory, &settings.base)? {
        return Err(SettingsError::InvalidSettings(format!(
            "invalid base branch name: {:?}",
            settings.base
        )));
    }
    Ok(settings)
}
pub const SETTINGS_FILE: &str = "oneflow.conf";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub remote: String,
    pub base: String,
    pub save_action: SaveAction,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            remote: "origin".into(),
            base: "main".into(),
            save_action: SaveAction::Ask,
        }
    }
}

fn read(repository: &RepositoryContext) -> Result<Settings, SettingsError> {
    let path = repository.common_dir.join(SETTINGS_FILE);
    // Only absence defaults. A dangling symlink or any unreadable existing entry
    // is an error rather than silently discarding repository intent.
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(source) => return Err(SettingsError::SettingsIo { path, source }),
        Ok(_) => {}
    }
    let contents =
        fs::read_to_string(&path).map_err(|source| SettingsError::SettingsIo { path, source })?;
    parse_settings(&contents)
}

fn parse_settings(contents: &str) -> Result<Settings, SettingsError> {
    let mut settings = Settings::default();
    let mut remote_seen = false;
    let mut base_seen = false;
    let mut action_seen = false;
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let invalid =
            |reason: &str| SettingsError::InvalidSettings(format!("line {}: {reason}", index + 1));
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("expected key = value"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(invalid("empty value"));
        }
        if key.trim() == "save-action" {
            if action_seen {
                return Err(invalid("duplicate key"));
            }
            action_seen = true;
            settings.save_action = match value {
                "ask" => SaveAction::Ask,
                "publish-only" => SaveAction::PublishOnly,
                "staged" => SaveAction::Staged,
                "wip" => SaveAction::Wip,
                "stash" => SaveAction::Stash,
                _ => {
                    return Err(invalid(
                        "save-action must be ask, publish-only, staged, wip, or stash",
                    ));
                }
            };
            continue;
        }
        let (slot, seen) = match key.trim() {
            "remote" => (&mut settings.remote, &mut remote_seen),
            "base" => (&mut settings.base, &mut base_seen),
            _ => {
                return Err(invalid(
                    "unknown key (supported keys: remote, base, save-action)",
                ));
            }
        };
        if *seen {
            return Err(invalid("duplicate key"));
        }
        *seen = true;
        *slot = value.to_owned();
    }
    Ok(settings)
}

pub(crate) fn valid_branch(directory: &Path, name: &str) -> Result<bool, InspectionError> {
    // --branch expands @{-n}; validating a full ref never interprets revisions.
    if name.starts_with('-') || name == "HEAD" {
        return Ok(false);
    }
    valid_ref(directory, &format!("refs/heads/{name}"))
}

fn valid_remote(directory: &Path, name: &str) -> Result<bool, InspectionError> {
    // A named remote, not a URL, path, local-repository '.', or command option.
    if name.is_empty() || name.starts_with('-') || name == "." {
        return Ok(false);
    }
    valid_ref(directory, &format!("refs/remotes/{name}/base"))
}

fn valid_ref(directory: &Path, reference: &str) -> Result<bool, InspectionError> {
    if reference.contains('\0') {
        return Ok(false);
    }
    let args = ["check-ref-format", reference];
    let output = git::inspect(directory, args).map_err(InspectionError::Io)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) if output.stderr.is_empty() => Ok(false),
        _ => Err(InspectionError::Git {
            arguments: args.iter().map(std::ffi::OsString::from).collect(),
            output,
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_settings_and_save_preference() {
        assert_eq!(parse_settings("# comment\n").unwrap(), Settings::default());
        assert_eq!(parse_settings("base = trunk\r\n").unwrap().remote, "origin");
        assert_eq!(parse_settings("remote = team\n").unwrap().base, "main");
        for (value, action) in [
            ("ask", SaveAction::Ask),
            ("publish-only", SaveAction::PublishOnly),
            ("staged", SaveAction::Staged),
            ("wip", SaveAction::Wip),
            ("stash", SaveAction::Stash),
        ] {
            assert_eq!(
                parse_settings(&format!("save-action = {value}\n"))
                    .unwrap()
                    .save_action,
                action
            );
        }
        for contents in [
            "remote",
            "remote=",
            "base= \n",
            "other=x",
            "base=one\nbase=two",
            "remote=a\nremote=a",
            "[oneflow]",
            "save-action=",
            "save-action=WIP",
            "save-action=commit",
            "save-action=wip # comment",
            "save-action=wip\nsave-action=wip",
        ] {
            assert!(
                matches!(
                    parse_settings(contents),
                    Err(SettingsError::InvalidSettings(_))
                ),
                "{contents}"
            );
        }
    }
}
