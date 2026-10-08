//! Preparation and safe create/resume for workstreams, independent of CLI.

mod workflow;
pub use workflow::{
    Action, Effect, Failure, Outcome, ReferenceState, RemainingState, StartError, TargetUpstream,
    execute,
};

use std::{fmt, io, path::Path};

use unicode_general_category::{GeneralCategory as Category, get_general_category};
use unicode_normalization::UnicodeNormalization;

use crate::{
    repository::{self, InspectionError, RepositoryContext},
    settings::{self, valid_branch},
};

pub use crate::settings::{SETTINGS_FILE, Settings};
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
                write!(f, "invalid settings: {reason}")
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
    let settings = settings::load(directory, &repository).map_err(|e| match e {
        settings::SettingsError::Repository(e) => ResolveError::Repository(e),
        settings::SettingsError::SettingsIo { path, source } => {
            ResolveError::SettingsIo { path, source }
        }
        settings::SettingsError::InvalidSettings(e) => ResolveError::InvalidSettings(e),
    })?;
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
}
