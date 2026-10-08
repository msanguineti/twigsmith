//! Preparation and safe create/resume for workstreams, independent of CLI.

mod workflow;
pub use workflow::{
    Action, Effect, Failure, Outcome, ReferenceState, RemainingState, StartError, TargetUpstream,
    execute,
};

use std::{fmt, fs, io, path::Path};

use unicode_general_category::{GeneralCategory as Category, get_general_category};
use unicode_normalization::UnicodeNormalization;

use crate::{
    git,
    repository::{self, InspectionError, RepositoryContext},
};

pub const SETTINGS_FILE: &str = "oneflow.conf";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub remote: String,
    pub base: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            remote: "origin".into(),
            base: "main".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    pub repository: RepositoryContext,
    pub target: String,
    pub settings: Settings,
}

#[derive(Debug)]
pub enum ResolveError {
    Repository(InspectionError),
    OutsideRepository,
    SettingsIo {
        path: std::path::PathBuf,
        source: io::Error,
    },
    InvalidSettings(String),
    UnusablePhrase,
    InvalidTarget(String),
    ProtectedBase(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "{error}"),
            Self::OutsideRepository => write!(f, "start preparation requires a Git repository"),
            Self::SettingsIo { path, source } => {
                write!(f, "cannot read settings {}: {source}", path.display())
            }
            Self::InvalidSettings(reason) => {
                write!(f, "invalid {SETTINGS_FILE} settings: {reason}")
            }
            Self::UnusablePhrase => write!(f, "phrase contains no usable letters or numbers"),
            Self::InvalidTarget(name) => write!(f, "invalid target branch name: {name:?}"),
            Self::ProtectedBase(name) => {
                write!(f, "target is the configured base branch: {name:?}")
            }
        }
    }
}

impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Repository(error) => Some(error),
            Self::SettingsIo { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<InspectionError> for ResolveError {
    fn from(error: InspectionError) -> Self {
        Self::Repository(error)
    }
}

/// Generate a canonical, locale-independent name. Marks are retained only
/// after a letter/number (or another retained mark) in the current word.
fn phrase_slug(input: &str) -> Result<String, ResolveError> {
    let normalized: String = input
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .nfc()
        .collect();
    let mut slug = String::new();
    let mut separator = false;
    for ch in normalized.chars() {
        match get_general_category(ch) {
            Category::UppercaseLetter
            | Category::LowercaseLetter
            | Category::TitlecaseLetter
            | Category::ModifierLetter
            | Category::OtherLetter
            | Category::DecimalNumber
            | Category::LetterNumber
            | Category::OtherNumber => {
                if separator && !slug.is_empty() {
                    slug.push('-');
                }
                slug.push(ch);
                separator = false;
            }
            Category::NonspacingMark | Category::SpacingMark | Category::EnclosingMark => {
                if !slug.is_empty() && !separator {
                    slug.push(ch);
                }
            }
            _ => separator = true,
        }
    }
    if slug.is_empty() {
        return Err(ResolveError::UnusablePhrase);
    }
    Ok(slug)
}

/// Resolve repository settings and a target without contacting remotes or
/// changing files, refs, HEAD, or index. Ordinary ASCII spaces select canonical
/// phrase conversion; all other inputs must be safe literal branch names.
pub fn prepare(directory: impl AsRef<Path>, input: &str) -> Result<PreparedRequest, ResolveError> {
    let directory = directory.as_ref();
    let repository = repository::discover(directory)?.ok_or(ResolveError::OutsideRepository)?;
    let settings = read_settings(&repository)?;
    if !valid_remote(directory, &settings.remote)? {
        return Err(ResolveError::InvalidSettings(format!(
            "invalid remote name: {:?}",
            settings.remote
        )));
    }
    if !valid_branch(directory, &settings.base)? {
        return Err(ResolveError::InvalidSettings(format!(
            "invalid base branch name: {:?}",
            settings.base
        )));
    }
    let target = if input.contains(' ') {
        phrase_slug(input)?
    } else {
        input.to_owned()
    };
    if !valid_branch(directory, &target)? {
        return Err(ResolveError::InvalidTarget(target));
    }
    if target == settings.base {
        return Err(ResolveError::ProtectedBase(target));
    }
    Ok(PreparedRequest {
        repository,
        target,
        settings,
    })
}

fn read_settings(repository: &RepositoryContext) -> Result<Settings, ResolveError> {
    let path = repository.common_dir.join(SETTINGS_FILE);
    // Only absence defaults. A dangling symlink or any unreadable existing entry
    // is an error rather than silently discarding repository intent.
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(source) => return Err(ResolveError::SettingsIo { path, source }),
        Ok(_) => {}
    }
    let contents =
        fs::read_to_string(&path).map_err(|source| ResolveError::SettingsIo { path, source })?;
    parse_settings(&contents)
}

fn parse_settings(contents: &str) -> Result<Settings, ResolveError> {
    let mut settings = Settings::default();
    let mut remote_seen = false;
    let mut base_seen = false;
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let invalid =
            |reason: &str| ResolveError::InvalidSettings(format!("line {}: {reason}", index + 1));
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("expected key = value"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(invalid("empty value"));
        }
        let (slot, seen) = match key.trim() {
            "remote" => (&mut settings.remote, &mut remote_seen),
            "base" => (&mut settings.base, &mut base_seen),
            _ => return Err(invalid("unknown key (only remote and base are supported)")),
        };
        if *seen {
            return Err(invalid("duplicate key"));
        }
        *seen = true;
        *slot = value.to_owned();
    }
    Ok(settings)
}

fn valid_branch(directory: &Path, name: &str) -> Result<bool, ResolveError> {
    // --branch expands @{-n}; validating a full ref never interprets revisions.
    if name.starts_with('-') || name == "HEAD" {
        return Ok(false);
    }
    valid_ref(directory, &format!("refs/heads/{name}"))
}

fn valid_remote(directory: &Path, name: &str) -> Result<bool, ResolveError> {
    // A named remote, not a URL, path, local-repository '.', or command option.
    if name.is_empty() || name.starts_with('-') || name == "." {
        return Ok(false);
    }
    valid_ref(directory, &format!("refs/remotes/{name}/base"))
}

fn valid_ref(directory: &Path, reference: &str) -> Result<bool, ResolveError> {
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
        }
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_phrase_slugs() {
        for (input, expected) in [
            (" Fix__ BUG / now... ", "fix-bug-now"),
            ("CAFÉ 123", "café-123"),
            ("Cafe\u{301} 123", "café-123"),
            ("ΑΒΓ МОСКВА 中文１２", "αβγ-москва-中文１２"),
            ("İSTANBUL", "i\u{307}stanbul"),
            ("हिन्दी عَرَبِيّ", "हिन्दी-عَرَبِيّ"),
            ("\u{301}ABC /\u{301}DEF", "abc-def"),
            ("ΟΣ", "ος"),
        ] {
            assert_eq!(phrase_slug(input).unwrap(), expected);
        }
        for input in ["", " \t", "---...🚀", "\u{301}\u{20dd}"] {
            assert!(matches!(
                phrase_slug(input),
                Err(ResolveError::UnusablePhrase)
            ));
        }
        for (a, b) in [
            ("Å", "A\u{30a}"),
            ("각", "각"),
            ("É\u{323}", "E\u{323}\u{301}"),
        ] {
            assert_eq!(phrase_slug(a).unwrap(), phrase_slug(b).unwrap());
        }
    }

    #[test]
    fn strict_settings_parser() {
        assert_eq!(parse_settings("# comment\n").unwrap(), Settings::default());
        assert_eq!(parse_settings("base = trunk\r\n").unwrap().remote, "origin");
        assert_eq!(parse_settings("remote = team\n").unwrap().base, "main");
        for contents in [
            "remote",
            "remote=",
            "base= \n",
            "other=x",
            "base=one\nbase=two",
            "remote=a\nremote=a",
            "[oneflow]",
        ] {
            assert!(matches!(
                parse_settings(contents),
                Err(ResolveError::InvalidSettings(_))
            ));
        }
    }
}
