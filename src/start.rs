//! Preparation and safe create/resume for workstreams, independent of CLI.

mod workflow;
pub use workflow::{
    Action, Effect, Failure, Outcome, ReferenceState, RemainingState, StartError, TargetUpstream,
    execute,
};

use std::{fmt, path::Path};

use crate::{
    repository::{self, InspectionError, RepositoryContext},
    settings::{self, Settings, SettingsError, WorkstreamKind, valid_branch},
    tracking,
};

pub use crate::tracking::Error as TrackingError;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    pub repository: RepositoryContext,
    pub target: String,
    pub settings: Settings,
    routing: tracking::Snapshot,
    pub options: Options,
    pub source_commit: Option<Vec<u8>>,
    creation_error: Option<String>,
    association: Option<settings::BranchPolicy>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub policy: Option<WorkstreamKind>,
    pub from: Option<String>,
    pub from_tag: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug)]
pub enum ResolveError {
    Repository(InspectionError),
    OutsideRepository,
    Settings(SettingsError),
    InvalidTarget(String),
    ProtectedBase(String),
    Input(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(f, "{error}"),
            Self::OutsideRepository => write!(f, "start preparation requires a Git repository"),
            Self::Settings(error) => write!(f, "{error}"),
            Self::InvalidTarget(name) => write!(f, "invalid target branch name: {name:?}"),
            Self::Input(reason) => f.write_str(reason),
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
            Self::Settings(error) => Some(error),
            _ => None,
        }
    }
}

impl From<InspectionError> for ResolveError {
    fn from(error: InspectionError) -> Self {
        Self::Repository(error)
    }
}
impl From<SettingsError> for ResolveError {
    fn from(error: SettingsError) -> Self {
        Self::Settings(error)
    }
}

/// Resolve repository settings and an exact literal branch name without
/// contacting remotes or changing files, refs, HEAD, or index.
pub fn prepare(directory: impl AsRef<Path>, input: &str) -> Result<PreparedRequest, ResolveError> {
    prepare_with_options(directory, input, &Options::default())
}

pub fn prepare_with_options(
    directory: impl AsRef<Path>,
    input: &str,
    options: &Options,
) -> Result<PreparedRequest, ResolveError> {
    let directory = directory.as_ref();
    let repository = repository::discover(directory)?.ok_or(ResolveError::OutsideRepository)?;
    let target = input.to_owned();
    if !valid_branch(directory, &target)? {
        return Err(ResolveError::InvalidTarget(target));
    }
    if options.from.is_some() && options.from_tag.is_some() {
        return Err(ResolveError::Input(
            "choose --from or --from-tag, not both".into(),
        ));
    }
    if let Some(value) = &options.target
        && !valid_branch(directory, value)?
    {
        return Err(ResolveError::Input(
            "--target requires an exact branch name".into(),
        ));
    }
    let association = settings::branch_policy(directory, target.as_bytes())?;
    let mut settings = settings::load_for_branch(directory, &repository, target.as_bytes())?;
    if let Some(record) = &association {
        if options
            .policy
            .is_some_and(|kind| kind != record.policy.kind)
            || options
                .target
                .as_ref()
                .is_some_and(|value| Some(value) != record.target_override.as_ref())
        {
            return Err(ResolveError::Input("resume preserves established policy and routes; supplied policy/target is incompatible".into()));
        }
    } else if options.policy.is_some()
        || options.target.is_some()
        || options.from_tag.is_some()
        || options.from.is_some()
    {
        let kind = options.policy.unwrap_or(WorkstreamKind::Ordinary);
        settings::resolve_workstream(&mut settings, kind, options.target.as_deref())?;
    }
    let kind = settings
        .workstream
        .as_ref()
        .map(|p| p.kind)
        .unwrap_or(WorkstreamKind::Ordinary);
    if options.target.is_some() && kind != WorkstreamKind::Ordinary {
        return Err(ResolveError::Input("--target selects an ordinary contribution target; release/hotfix routes are configured in workstreams".into()));
    }
    if options.from_tag.is_some() && kind != WorkstreamKind::Hotfix {
        return Err(ResolveError::Input(
            "--from-tag requires explicit --policy hotfix".into(),
        ));
    }
    if kind == WorkstreamKind::Hotfix && options.from.is_some() {
        return Err(ResolveError::Input(
            "hotfix creation requires --from-tag with an exact version tag".into(),
        ));
    }
    if target == settings.base
        || settings
            .workstream
            .as_ref()
            .and_then(|p| p.release_marker.as_deref())
            == Some(&target)
    {
        return Err(ResolveError::ProtectedBase(target));
    }
    let local_ref = format!("refs/heads/{target}");
    let local = crate::git::inspect(
        directory,
        ["show-ref", "--verify", "--hash", local_ref.as_str()],
    )
    .map_err(InspectionError::Io)?;
    let local_exists = local.status.success();
    if association.is_some() && !local_exists {
        return Err(ResolveError::Input("established work branch is absent; complete its retained finish attempt or resolve the old association before recreating work".into()));
    }
    let selected = options
        .from
        .as_deref()
        .map(str::to_owned)
        .or_else(|| {
            options
                .from_tag
                .as_ref()
                .map(|tag| format!("refs/tags/{tag}"))
        })
        .or_else(|| {
            if association.is_none() && !local_exists {
                settings
                    .workstream
                    .as_ref()
                    .and_then(|p| p.creation_source.clone())
            } else {
                None
            }
        });
    if let Some(tag) = &options.from_tag {
        let reference = format!("refs/tags/{tag}");
        let output = crate::git::inspect(directory, ["check-ref-format", reference.as_str()])
            .map_err(InspectionError::Io)?;
        if !output.status.success() {
            return Err(ResolveError::Input(
                "--from-tag requires an exact valid tag name".into(),
            ));
        }
    }
    // Creation defaults apply only after remote discovery proves this is new
    // work. A remote resume can preserve history without locally available
    // default-source objects; explicit CLI source input still resolves now.
    let (source_commit, creation_error) = match selected
        .as_deref()
        .map(|source| resolve_commit(directory, source))
        .transpose()
    {
        Ok(commit) => (commit, None),
        Err(error) if options.from.is_none() && options.from_tag.is_none() => {
            (None, Some(error.to_string()))
        }
        Err(error) => return Err(error),
    };
    if let (Some(record), Some(commit)) = (&association, &source_commit)
        && commit != record.creation_commit.as_bytes()
    {
        return Err(ResolveError::Input(
            "explicit creation source differs from the established workstream creation commit"
                .into(),
        ));
    }
    if association.is_none()
        && local_exists
        && source_commit
            .as_ref()
            .is_some_and(|commit| local.stdout.strip_suffix(b"\n") != Some(commit.as_slice()))
    {
        return Err(ResolveError::Input("existing local work has no creation association matching the explicit request; select policy without creation input to preserve its history".into()));
    }
    let routing = tracking::snapshot(directory, target.as_bytes())?;
    Ok(PreparedRequest {
        repository,
        target,
        settings,
        routing,
        options: options.clone(),
        source_commit,
        creation_error,
        association,
    })
}

fn resolve_commit(directory: &Path, source: &str) -> Result<Vec<u8>, ResolveError> {
    if source.is_empty() || source.starts_with('-') || source.contains('\0') {
        return Err(ResolveError::Input(
            "creation source must be a valid commit reference".into(),
        ));
    }
    let expression = format!("{source}^{{commit}}");
    let output = crate::git::inspect(
        directory,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            expression.as_str(),
        ],
    )
    .map_err(InspectionError::Io)?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(ResolveError::Input(format!(
            "creation source {source:?} is missing, ambiguous, or does not identify a locally available commit"
        )));
    }
    let commit = output
        .stdout
        .strip_suffix(b"\n")
        .ok_or(InspectionError::InvalidOutput("creation commit"))?;
    if !matches!(commit.len(), 40 | 64) || !commit.iter().all(u8::is_ascii_hexdigit) {
        return Err(InspectionError::InvalidOutput("creation commit").into());
    }
    Ok(commit.to_vec())
}
