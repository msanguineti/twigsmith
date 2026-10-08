//! Save planning, local preservation, and exact publication.
//! Assessment/revalidation are read-only; execution composes the Git backends.
//! Revalidation is conservative and consists of several reads, not a lock.
use crate::{
    git,
    repository::{
        self, Change, ChangeKind, Head, InspectionError, Observation, RepositoryContext,
        RepositoryKind, RepositoryState,
    },
    settings::{self, SaveAction, Settings, SettingsError},
};
use std::{
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
};

pub mod execution;
pub mod publication;

pub const WIP_STAGED_MESSAGE: &str = "[WIP - Staged]";
pub const WIP_REMAINING_MESSAGE: &str = "[WIP - Unstaged]";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// Explicit selection overrides the repository preference.
    pub action: Option<SaveAction>,
    pub message: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeState {
    Clean,
    StagedOnly,
    UnstagedOnly,
    Mixed,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// May overlap remaining_tracked for partially staged files.
    pub staged: Vec<Vec<u8>>,
    pub remaining_tracked: Vec<Vec<u8>>,
    pub untracked: Vec<Vec<u8>>,
}
impl Scope {
    fn has_remaining(&self) -> bool {
        !self.remaining_tracked.is_empty() || !self.untracked.is_empty()
    }
    pub fn tree_state(&self) -> TreeState {
        match (!self.staged.is_empty(), self.has_remaining()) {
            (false, false) => TreeState::Clean,
            (true, false) => TreeState::StagedOnly,
            (false, true) => TreeState::UnstagedOnly,
            (true, true) => TreeState::Mixed,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredInput {
    None,
    Action,
    Message,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalStep {
    CommitIndex {
        message: String,
    },
    /// Commit the index using WIP_STAGED_MESSAGE, without restaging files.
    CommitStagedWip,
    /// Stage scoped remaining tracked/untracked work and commit using
    /// WIP_REMAINING_MESSAGE; ignored untracked content stays excluded.
    StageAndCommitRemainingWip,
    /// Includes tracked and nonignored untracked work; preservation stays local.
    StashIncludingUntracked,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub remote: String,
    pub branch_ref: Vec<u8>,
    /// Local tracking may be established only after confirmed publication.
    pub establish_upstream: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub repository: RepositoryContext,
    pub settings: Settings,
    pub branch: Vec<u8>,
    pub commit: Vec<u8>,
    pub scope: Scope,
    pub action: SaveAction,
    pub required_input: RequiredInput,
    /// Empty while input is unresolved. No effects are permitted then.
    pub local_steps: Vec<LocalStep>,
    /// None while input is unresolved. Only this branch may be published.
    pub publication: Option<Publication>,
    pub applicable_actions: Vec<SaveAction>,
    request: Request,
    snapshot: ContextSnapshot,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextSnapshot {
    state: RepositoryState,
    upstream: UpstreamSettings,
    push_urls: Vec<u8>,
    index_path: PathBuf,
    index: Option<Vec<u8>>,
    tracked_diff: Vec<u8>,
    files: Vec<(Vec<u8>, FileSnapshot)>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamSettings {
    pub remotes: Vec<Vec<u8>>,
    pub merge_refs: Vec<Vec<u8>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileSnapshot {
    Missing,
    File {
        bytes: Vec<u8>,
        permissions: fs::Permissions,
    },
    Symlink(PathBuf),
    Directory(Vec<(OsString, FileSnapshot)>),
}
#[derive(Debug)]
pub enum SaveError {
    Inspection(InspectionError),
    Settings(SettingsError),
    OutsideRepository,
    BareRepository,
    DetachedHead,
    UnbornBranch,
    ProtectedBase,
    Conflicts,
    ActiveOperation,
    DirtySubmodules,
    InvalidUpstream,
    InvalidRemote {
        name: String,
        source: InspectionError,
    },
    NoStagedChanges,
    BlankMessage,
    MessageNotApplicable,
    ContextChanged,
}
impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Inspection(e) => return write!(f, "{e}"),
            Self::Settings(e) => return write!(f, "{e}"),
            Self::OutsideRepository => "save requires a Git repository",
            Self::BareRepository => "save requires a working tree",
            Self::DetachedHead => "save requires an attached branch",
            Self::UnbornBranch => "save requires a branch with an existing commit",
            Self::ProtectedBase => "save refuses the configured base branch",
            Self::Conflicts => "save refuses unresolved conflicts",
            Self::ActiveOperation => "save refuses an active Git operation",
            Self::DirtySubmodules => "save cannot preserve dirty submodule contents",
            Self::InvalidUpstream => {
                "upstream settings are incomplete, ambiguous, or conflict with the configured remote and exact branch"
            }
            Self::InvalidRemote { name, source } => {
                return write!(
                    f,
                    "configured remote {name:?} cannot be resolved from local configuration: {source}"
                );
            }
            Self::NoStagedChanges => "staged save requires staged changes",
            Self::BlankMessage => "staged save requires a nonblank developer message",
            Self::MessageNotApplicable => "a save message applies only to the staged action",
            Self::ContextChanged => {
                "save context or approved change scope changed; assess and approve again"
            }
        };
        write!(f, "{text}")
    }
}
impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inspection(e) => Some(e),
            Self::Settings(e) => Some(e),
            Self::InvalidRemote { source, .. } => Some(source),
            _ => None,
        }
    }
}
impl From<InspectionError> for SaveError {
    fn from(e: InspectionError) -> Self {
        Self::Inspection(e)
    }
}
impl From<SettingsError> for SaveError {
    fn from(e: SettingsError) -> Self {
        Self::Settings(e)
    }
}
impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        Self::Inspection(InspectionError::Io(e))
    }
}

fn read(directory: &Path, args: &[OsString]) -> Result<Vec<u8>, SaveError> {
    let output = git::inspect(directory, args)?;
    if !output.status.success() {
        return Err(InspectionError::Git {
            arguments: args.to_vec(),
            output,
        }
        .into());
    }
    Ok(output.stdout)
}
fn config(directory: &Path, key: &[u8]) -> Result<Vec<Vec<u8>>, SaveError> {
    let args = vec![
        "config".into(),
        "-z".into(),
        "--get-all".into(),
        repository::os(key)?,
    ];
    let output = git::inspect(directory, &args)?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(vec![]);
    }
    if !output.status.success() {
        return Err(InspectionError::Git {
            arguments: args,
            output,
        }
        .into());
    }
    let bytes = output
        .stdout
        .strip_suffix(b"\0")
        .ok_or(InspectionError::InvalidOutput("save configuration"))?;
    Ok(bytes.split(|b| *b == 0).map(<[u8]>::to_vec).collect())
}
fn file_snapshot(path: &Path, recurse: bool) -> Result<FileSnapshot, SaveError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(FileSnapshot::Missing),
        Err(e) => return Err(e.into()),
    };
    if metadata.file_type().is_symlink() {
        Ok(FileSnapshot::Symlink(fs::read_link(path)?))
    } else if metadata.is_file() {
        Ok(FileSnapshot::File {
            bytes: fs::read(path)?,
            permissions: metadata.permissions(),
        })
    } else if metadata.is_dir() {
        let mut entries = vec![];
        if recurse {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                entries.push((entry.file_name(), file_snapshot(&entry.path(), true)?));
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
        }
        Ok(FileSnapshot::Directory(entries))
    } else {
        Err(InspectionError::InvalidOutput("unsupported save scope file type").into())
    }
}

/// Assess local eligibility and exact action scope without contacting any remote.
/// Publication is an intended effect, not evidence of remote availability/history.
pub fn assess(directory: impl AsRef<Path>, request: &Request) -> Result<Plan, SaveError> {
    assess_with_upstream(directory.as_ref(), request, None)
}

// Only the publisher's second tracking write may validate its own intermediate
// remote-only configuration. Public assessment always enforces the full guard.
fn assess_with_upstream(
    directory: &Path,
    request: &Request,
    expected_upstream: Option<&UpstreamSettings>,
) -> Result<Plan, SaveError> {
    let repository = repository::discover(directory)?.ok_or(SaveError::OutsideRepository)?;
    if repository.kind == RepositoryKind::Bare {
        return Err(SaveError::BareRepository);
    }
    let settings = settings::load(directory, &repository)?;
    let Observation::Repository(state) = repository::inspect_without_upstream(directory)? else {
        return Err(SaveError::OutsideRepository);
    };
    let (branch, commit) = match &state.head {
        Head::Attached { branch, commit } => (branch.clone(), commit.clone()),
        Head::Detached { .. } => return Err(SaveError::DetachedHead),
        Head::Unborn { .. } => return Err(SaveError::UnbornBranch),
    };
    if branch == settings.base.as_bytes() {
        return Err(SaveError::ProtectedBase);
    }
    if state.operations.is_active() {
        return Err(SaveError::ActiveOperation);
    }
    let tree = state
        .working_tree
        .as_ref()
        .ok_or(SaveError::BareRepository)?;
    let mut scope = Scope::default();
    for change in &tree.changes {
        match &change.kind {
            ChangeKind::Conflicted { .. } => return Err(SaveError::Conflicts),
            ChangeKind::Untracked => scope.untracked.push(change.path.clone()),
            ChangeKind::Tracked {
                staged,
                unstaged,
                submodule,
                ..
            } => {
                if submodule.is_some_and(|s| s.tracked_changes || s.untracked_changes) {
                    return Err(SaveError::DirtySubmodules);
                }
                if *staged != Change::Unchanged {
                    scope.staged.push(change.path.clone());
                }
                if *unstaged != Change::Unchanged {
                    scope.remaining_tracked.push(change.path.clone());
                }
            }
        }
    }
    let upstream = upstream_settings(directory, &branch)?;
    let mut branch_ref = b"refs/heads/".to_vec();
    branch_ref.extend_from_slice(&branch);
    let absent = upstream.remotes.is_empty() && upstream.merge_refs.is_empty();
    if expected_upstream.is_some_and(|expected| expected != &upstream)
        || (expected_upstream.is_none()
            && !absent
            && (upstream.remotes != [settings.remote.as_bytes()]
                || upstream.merge_refs != [branch_ref.as_slice()]))
    {
        return Err(SaveError::InvalidUpstream);
    }
    // get-url resolves the named remote and ordinary push URL rewrites locally.
    // Preserve every destination for revalidation; publication handles outcomes later.
    let push_urls = read(
        directory,
        &[
            "remote".into(),
            "get-url".into(),
            "--push".into(),
            "--all".into(),
            settings.remote.clone().into(),
        ],
    )
    .map_err(|error| match error {
        SaveError::Inspection(source @ InspectionError::Git { .. }) => SaveError::InvalidRemote {
            name: settings.remote.clone(),
            source,
        },
        other => other,
    })?;
    let selected = request.action.unwrap_or(settings.save_action);
    if request.message.is_some() && selected != SaveAction::Staged {
        return Err(SaveError::MessageNotApplicable);
    }
    let action = if scope.tree_state() == TreeState::Clean {
        SaveAction::PublishOnly
    } else {
        selected
    };
    let mut applicable_actions = vec![SaveAction::PublishOnly];
    if scope.tree_state() != TreeState::Clean {
        if !scope.staged.is_empty() {
            applicable_actions.push(SaveAction::Staged);
        }
        applicable_actions.extend([SaveAction::Wip, SaveAction::Stash]);
    }
    let mut local_steps = vec![];
    let required_input = match action {
        SaveAction::Ask => RequiredInput::Action,
        SaveAction::PublishOnly => RequiredInput::None,
        SaveAction::Staged => {
            if scope.staged.is_empty() {
                return Err(SaveError::NoStagedChanges);
            }
            match &request.message {
                None => RequiredInput::Message,
                Some(message) if message.trim().is_empty() => return Err(SaveError::BlankMessage),
                Some(message) => {
                    local_steps.push(LocalStep::CommitIndex {
                        message: message.clone(),
                    });
                    RequiredInput::None
                }
            }
        }
        SaveAction::Wip => {
            if !scope.staged.is_empty() {
                local_steps.push(LocalStep::CommitStagedWip);
            }
            if scope.has_remaining() {
                local_steps.push(LocalStep::StageAndCommitRemainingWip);
            }
            RequiredInput::None
        }
        SaveAction::Stash => {
            local_steps.push(LocalStep::StashIncludingUntracked);
            RequiredInput::None
        }
    };
    let publication = (required_input == RequiredInput::None).then(|| Publication {
        remote: settings.remote.clone(),
        branch_ref,
        establish_upstream: absent,
    });
    let index_path = read(
        directory,
        &[
            "rev-parse".into(),
            "--path-format=absolute".into(),
            "--git-path".into(),
            "index".into(),
        ],
    )?;
    let index_path = index_path
        .strip_suffix(b"\n")
        .ok_or(InspectionError::InvalidOutput("index path"))?;
    let index_path = PathBuf::from(repository::os(index_path)?);
    let index = match fs::read(&index_path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut files = vec![];
    for change in &tree.changes {
        files.push((
            change.path.clone(),
            file_snapshot(
                &tree.root.join(repository::os(&change.path)?),
                matches!(change.kind, ChangeKind::Untracked),
            )?,
        ));
    }
    // Also capture worktree gitlink revisions, which status reduces to a flag.
    // Disable external diff/textconv execution and use local binary output.
    let tracked_diff = read(
        directory,
        &[
            "-c".into(),
            "diff.autoRefreshIndex=false".into(),
            "diff".into(),
            "--no-ext-diff".into(),
            "--no-textconv".into(),
            "--binary".into(),
            "--no-renames".into(),
            "--no-relative".into(),
            "--submodule=short".into(),
            "--ignore-submodules=none".into(),
            "HEAD".into(),
            "--".into(),
        ],
    )?;
    Ok(Plan {
        repository,
        settings,
        branch,
        commit,
        scope,
        action,
        required_input,
        local_steps,
        publication,
        applicable_actions,
        request: request.clone(),
        snapshot: ContextSnapshot {
            state: *state,
            upstream,
            push_urls,
            index_path,
            index,
            tracked_diff,
            files,
        },
    })
}

/// Compare a fresh assessment with the approved plan, including index bytes,
/// changed-file contents, exact HEAD/settings/upstream and push destinations.
/// A failed fresh guard is returned directly. No snapshot protects against a
/// concurrent change between this check and a later executor's Git operation.
pub fn revalidate(directory: impl AsRef<Path>, approved: &Plan) -> Result<(), SaveError> {
    if assess(directory, &approved.request)? != *approved {
        return Err(SaveError::ContextChanged);
    }
    Ok(())
}

fn upstream_key(branch: &[u8], suffix: &[u8]) -> Vec<u8> {
    [b"branch.".as_slice(), branch, suffix].concat()
}

fn upstream_settings(directory: &Path, branch: &[u8]) -> Result<UpstreamSettings, SaveError> {
    Ok(UpstreamSettings {
        remotes: config(directory, &upstream_key(branch, b".remote"))?,
        merge_refs: config(directory, &upstream_key(branch, b".merge"))?,
    })
}
