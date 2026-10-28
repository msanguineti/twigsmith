//! Save planning, local preservation, and exact publication.
//! Assessment/revalidation are read-only; execution composes the Git backends.
//! Revalidation is conservative and consists of several reads, not a lock.
use crate::{
    git,
    repository::{
        self, Change, ChangeKind, Head, InspectionError, Observation, RepositoryContext,
        RepositoryKind, RepositoryState,
    },
    settings::{
        self, SaveKind, SaveOperation, SavePlan, SaveScope, SettingSource, Settings, SettingsError,
    },
    tracking,
};
use std::{
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
};

pub mod execution;
mod groups;
pub mod publication;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// Explicit selection overrides the repository preference.
    pub plan: Option<SavePlan>,
    pub message: Option<String>,
    pub stash_message: Option<String>,
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
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStep {
    pub operation: SaveOperation,
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub remote: String,
    pub branch_ref: Vec<u8>,
    /// Local tracking may be established only after confirmed publication.
    pub establish_upstream: bool,
}
/// Source of the planned action, including save's existing clean-tree rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionSource {
    /// An explicit Request action, supplied by a flag or interactive choice.
    ExplicitSelection,
    Setting(SettingSource),
    CleanTree,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub repository: RepositoryContext,
    pub settings: Settings,
    pub branch: Vec<u8>,
    pub commit: Vec<u8>,
    pub scope: Scope,
    pub plan: SavePlan,
    pub action_source: ActionSource,
    pub required_input: RequiredInput,
    /// Empty while input is unresolved. No effects are permitted then.
    pub local_steps: Vec<LocalStep>,
    /// None while input is unresolved. Only this branch may be published.
    pub publication: Option<Publication>,
    snapshot: ContextSnapshot,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextSnapshot {
    // Git-resolved paths belong to this selected worktree and environment.
    // Every observation rereads their contents; no HEAD/status/file state caches.
    metadata: repository::MetadataPaths,
    state: RepositoryState,
    routing: tracking::Snapshot,
    push_urls: Vec<u8>,
    index_path: PathBuf,
    index: Option<Vec<u8>>,
    tracked_diff: Vec<u8>,
    files: Vec<(Vec<u8>, FileSnapshot)>,
}
pub use crate::tracking::UpstreamSettings;
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
    ConflictingPushRemote,
    InvalidRemote {
        name: String,
        source: InspectionError,
    },
    BlankMessage,
    InvalidPlan(String),
    Inseparable,
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
            Self::InvalidUpstream => return write!(f, "{}", tracking::Error::InvalidUpstream),
            Self::ConflictingPushRemote => {
                return write!(f, "{}", tracking::Error::ConflictingPushRemote);
            }
            Self::InvalidRemote { name, source } => {
                return write!(
                    f,
                    "configured remote {name:?} cannot be resolved from local configuration: {source}"
                );
            }
            Self::BlankMessage => "a commit requires a nonblank message",
            Self::InvalidPlan(message) => return write!(f, "{message}"),
            Self::Inseparable => {
                "the original staged and unstaged changes cannot be separated faithfully for this plan; use native Git to commit the staged prerequisite first, or preserve all work together"
            }
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
    Ok(tracking::config(directory, key)?)
}
impl From<tracking::Error> for SaveError {
    fn from(error: tracking::Error) -> Self {
        match error {
            tracking::Error::InvalidUpstream => Self::InvalidUpstream,
            tracking::Error::ConflictingPushRemote => Self::ConflictingPushRemote,
        }
    }
}
fn file_snapshot(path: &Path, recurse: bool) -> Result<FileSnapshot, SaveError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(FileSnapshot::Missing);
        }
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

struct Action {
    plan: SavePlan,
    action_source: ActionSource,
    required_input: RequiredInput,
    local_steps: Vec<LocalStep>,
    publication: Option<Publication>,
}
// Action selection consumes established facts and performs no repository reads.
// Both initial assessment and prompt resolution apply the same grammar/defaults.
fn select_action(
    settings: &Settings,
    scope: &Scope,
    branch: &[u8],
    absent: bool,
    request: &Request,
) -> Result<Action, SaveError> {
    let branch_ref = [b"refs/heads/".as_slice(), branch].concat();
    let selected = request
        .plan
        .clone()
        .unwrap_or_else(|| settings.save_plan.clone());
    selected.validate().map_err(SaveError::InvalidPlan)?;
    for (key, value, kind) in [
        ("--message", &request.message, SaveKind::Commit),
        ("--stash-message", &request.stash_message, SaveKind::Stash),
    ] {
        if let Some(value) = value {
            settings::validate_save_label(key, value).map_err(SaveError::InvalidPlan)?;
            if !(selected == SavePlan::Ask && scope.tree_state() != TreeState::Clean)
                && !matches!(&selected, SavePlan::Operations(ops) if ops.iter().any(|op| op.action == kind))
            {
                return Err(SaveError::InvalidPlan(format!(
                    "{key} requires a selected {} operation",
                    kind.as_str()
                )));
            }
        }
    }
    let plan = if scope.tree_state() == TreeState::Clean && selected == SavePlan::Ask {
        SavePlan::PublishOnly
    } else {
        selected
    };
    let action_source = if scope.tree_state() == TreeState::Clean
        && request.plan.is_none()
        && settings.save_plan == SavePlan::Ask
    {
        ActionSource::CleanTree
    } else if request.plan.is_some() {
        ActionSource::ExplicitSelection
    } else {
        ActionSource::Setting(settings.sources.save_plan.clone())
    };
    let mut local_steps = vec![];
    let required_input = if plan == SavePlan::Ask {
        RequiredInput::Action
    } else {
        if let SavePlan::Operations(operations) = &plan {
            for operation in operations {
                let empty = match operation.scope {
                    SaveScope::All => scope.tree_state() == TreeState::Clean,
                    SaveScope::Staged => scope.staged.is_empty(),
                    SaveScope::Unstaged => !scope.has_remaining(),
                };
                if empty {
                    continue;
                }
                let label = match operation.action {
                    SaveKind::Commit => request.message.as_ref().unwrap_or(&settings.save_message),
                    SaveKind::Stash => request
                        .stash_message
                        .as_ref()
                        .unwrap_or(&settings.save_stash_message),
                };
                let message = match operation.scope {
                    SaveScope::All => label.clone(),
                    scope => format!("{label} [{}]", scope.as_str()),
                };
                local_steps.push(LocalStep {
                    operation: *operation,
                    message,
                });
            }
        }
        RequiredInput::None
    };
    let publication = (required_input == RequiredInput::None).then(|| Publication {
        remote: settings.publish_remote.clone(),
        branch_ref,
        establish_upstream: absent,
    });
    Ok(Action {
        plan,
        action_source,
        required_input,
        local_steps,
        publication,
    })
}

/// Assess local eligibility and exact action scope without contacting any remote.
/// Publication is an intended effect, not evidence of remote availability/history.
pub fn assess(directory: impl AsRef<Path>, request: &Request) -> Result<Plan, SaveError> {
    let directory = directory.as_ref();
    let repository = repository::discover(directory)?.ok_or(SaveError::OutsideRepository)?;
    if repository.kind == RepositoryKind::Bare {
        return Err(SaveError::BareRepository);
    }
    let settings = settings::load(directory, &repository)?;
    let metadata = repository::MetadataPaths::operations(directory)?;
    let Observation::Repository(state) =
        repository::inspect_context_with_paths(directory, repository.clone(), &metadata)?
    else {
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
    let routing = tracking::snapshot(directory, &branch)?;
    routing.validate(&settings, &branch)?;
    // get-url resolves the named remote and ordinary push URL rewrites locally.
    // Preserve every destination for revalidation; publication handles outcomes later.
    let push_urls = read(
        directory,
        &[
            "remote".into(),
            "get-url".into(),
            "--push".into(),
            "--all".into(),
            settings.publish_remote.clone().into(),
        ],
    )
    .map_err(|error| match error {
        SaveError::Inspection(source @ InspectionError::Git { .. }) => SaveError::InvalidRemote {
            name: settings.publish_remote.clone(),
            source,
        },
        other => other,
    })?;
    let action = select_action(
        &settings,
        &scope,
        &branch,
        routing.upstream.absent(),
        request,
    )?;
    let index_path = index_path(directory)?;
    let index = read_index(&index_path)?;
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
    let tracked_diff = tracked_diff(directory)?;
    Ok(Plan {
        repository,
        settings,
        branch,
        commit,
        scope,
        plan: action.plan,
        action_source: action.action_source,
        required_input: action.required_input,
        local_steps: action.local_steps,
        publication: action.publication,
        snapshot: ContextSnapshot {
            metadata,
            state: *state,
            routing,
            push_urls,
            index_path,
            index,
            tracked_diff,
            files,
        },
    })
}

// Reports observe the repository actually selected at this boundary. Reuse
// only paths whose worktree and environment still match; an invalidated plan
// must not make the report describe its earlier repository as the current one.
fn observe_repository(directory: &Path, plan: &Plan) -> Result<Observation, InspectionError> {
    let Some(context) = repository::discover(directory)? else {
        return Ok(Observation::OutsideRepository);
    };
    if context == plan.repository && plan.snapshot.metadata.matches_environment() {
        repository::inspect_context_with_paths(directory, context, &plan.snapshot.metadata)
    } else {
        repository::inspect_context_without_upstream(directory, context)
    }
}

fn index_path(directory: &Path) -> Result<PathBuf, SaveError> {
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
    Ok(PathBuf::from(repository::os(index_path)?))
}
fn read_index(index_path: &Path) -> Result<Option<Vec<u8>>, SaveError> {
    match fs::read(index_path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn tracked_diff(directory: &Path) -> Result<Vec<u8>, SaveError> {
    read(
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
    )
}

/// Validate the selected local work and save inputs after a caller-controlled
/// boundary. This reads the facts needed by preservation/publication, without
/// rebuilding the plan or comparing settings used only by finish. The reads do
/// not lock against a concurrent writer; execution validates its next effects.
pub fn revalidate(directory: impl AsRef<Path>, approved: &Plan) -> Result<(), SaveError> {
    validate_context(directory.as_ref(), approved).map(|_| ())
}
fn validate_context(directory: &Path, approved: &Plan) -> Result<Settings, SaveError> {
    let repository = repository::discover(directory)?.ok_or(SaveError::OutsideRepository)?;
    if repository != approved.repository {
        return Err(SaveError::ContextChanged);
    }
    let Observation::Repository(state) = repository::inspect_context_with_paths(
        directory,
        repository.clone(),
        &approved.snapshot.metadata,
    )?
    else {
        return Err(SaveError::ContextChanged);
    };
    if *state != approved.snapshot.state {
        return Err(SaveError::ContextChanged);
    }
    let current = settings::load_for_branch(directory, &repository, &approved.branch)?;
    if !same_save_settings(&current, &approved.settings) {
        return Err(SaveError::ContextChanged);
    }
    let routing = tracking::snapshot(directory, &approved.branch)?;
    routing.validate(&current, &approved.branch)?;
    if routing != approved.snapshot.routing
        || read(
            directory,
            &[
                "remote".into(),
                "get-url".into(),
                "--push".into(),
                "--all".into(),
                approved.settings.publish_remote.clone().into(),
            ],
        )? != approved.snapshot.push_urls
    {
        return Err(SaveError::ContextChanged);
    }
    let current_index = index_path(directory)?;
    if current_index != approved.snapshot.index_path
        || read_index(&current_index)? != approved.snapshot.index
        || tracked_diff(directory)? != approved.snapshot.tracked_diff
    {
        return Err(SaveError::ContextChanged);
    }
    let root = repository
        .working_tree_root
        .as_ref()
        .ok_or(SaveError::BareRepository)?;
    for (path, expected) in &approved.snapshot.files {
        if file_snapshot(
            &root.join(repository::os(path)?),
            approved.scope.untracked.contains(path),
        )? != *expected
        {
            return Err(SaveError::ContextChanged);
        }
    }
    Ok(current)
}
// These settings determine the presented save choice, labels, protected branch
// or routing eligibility. Finish-only preferences/checks cannot change those
// effects. Preserve provenance for the relevant layered choices.
pub(crate) fn same_save_settings(current: &Settings, approved: &Settings) -> bool {
    current.publish_remote == approved.publish_remote
        && current.base_remote == approved.base_remote
        && current.base == approved.base
        && current.save_plan == approved.save_plan
        && current.save_message == approved.save_message
        && current.save_stash_message == approved.save_stash_message
        && current.sources.publish_remote == approved.sources.publish_remote
        && current.sources.base_remote == approved.sources.base_remote
        && current.sources.base == approved.sources.base
        && current.sources.save_plan == approved.sources.save_plan
        && current.sources.save_message == approved.sources.save_message
        && current.sources.save_stash_message == approved.sources.save_stash_message
        && current.workstream.as_ref().map(|p| &p.preparation_source)
            == approved.workstream.as_ref().map(|p| &p.preparation_source)
}
/// Resolve a choice against the work and routes already presented to the caller.
/// Only that boundary is validated; constructing the selected action is pure.
pub fn resolve(
    directory: impl AsRef<Path>,
    approved: &Plan,
    request: &Request,
) -> Result<Plan, SaveError> {
    let action = select_action(
        &approved.settings,
        &approved.scope,
        &approved.branch,
        approved.snapshot.routing.upstream.absent(),
        request,
    )?;
    let current = validate_context(directory.as_ref(), approved)?;
    let mut resolved = approved.clone();
    // Preserve the selected save inputs, while carrying the already validated
    // current informational settings into execution rather than stale finish fields.
    resolved.settings = current;
    resolved.plan = action.plan;
    resolved.action_source = action.action_source;
    resolved.required_input = action.required_input;
    resolved.local_steps = action.local_steps;
    resolved.publication = action.publication;
    Ok(resolved)
}

fn upstream_key(branch: &[u8], suffix: &[u8]) -> Vec<u8> {
    tracking::key(branch, suffix)
}
fn upstream_settings(directory: &Path, branch: &[u8]) -> Result<UpstreamSettings, SaveError> {
    Ok(tracking::upstream(directory, branch)?)
}
