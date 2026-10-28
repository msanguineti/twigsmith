//! Execute an approved save plan without interaction or rollback.
use super::{Plan, RequiredInput, SaveError, groups, publication};
use crate::{
    git, repository,
    settings::{self, SaveKind, SavePlan, SaveScope},
};
use std::{
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Output,
};

/// Observations do not require save eligibility: a failing hook can detach HEAD,
/// leave conflicts, or change configuration. Such effects must remain visible.
#[derive(Debug)]
pub struct LocalObservation {
    pub repository: Result<repository::Observation, repository::InspectionError>,
    pub index: Result<Option<Vec<u8>>, SaveError>,
    pub stash: Result<Option<Vec<u8>>, SaveError>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    CommitIndex { message: String },
    Stash { message: String },
    PrepareIndex { tree: Vec<u8> },
    Reconcile { tree: Vec<u8> },
}
#[derive(Debug)]
pub struct CommandReport {
    pub operation: Operation,
    pub arguments: Vec<OsString>,
    /// Raw output/status, including failures that produced effects.
    pub result: Result<Output, io::Error>,
}
/// Verified local steps. Failed or unexpectedly altered steps can also have
/// effects; those remain visible in command output and the final remaining state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Checkpoint {
        commit: Vec<u8>,
        /// Message submitted to Git; normal hooks/cleanup may edit it.
        message: String,
    },
    /// Local preservation only: this object is never published by branch push.
    LocalStash { commit: Vec<u8>, message: String },
}
#[derive(Debug)]
pub struct Recovery {
    pub commit: Vec<u8>,
    pub reference: String,
    pub retained: bool,
}
#[derive(Debug)]
pub struct Report {
    pub plan: SavePlan,
    pub recovery: Option<Recovery>,
    pub before: LocalObservation,
    pub commands: Vec<CommandReport>,
    pub effects: Vec<Effect>,
    /// None means no publication was attempted. Stash objects are never pushed.
    pub publication: Option<publication::Report>,
    pub after: LocalObservation,
    pub error: Option<ExecutionError>,
}
impl Report {
    pub fn is_complete(&self) -> bool {
        self.error.is_none()
            && self
                .publication
                .as_ref()
                .is_some_and(publication::Report::is_complete)
    }
}
#[derive(Debug)]
pub enum ExecutionError {
    Save(SaveError),
    PublicationPreflight(publication::PublishError),
    RequiredInput(RequiredInput),
    IndexLocked,
    LocalCommandFailed,
}
impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Save(e) => write!(f, "{e}"),
            Self::PublicationPreflight(e) => write!(f, "{e}"),
            Self::RequiredInput(input) => write!(f, "save requires resolved {input:?} input"),
            Self::IndexLocked => write!(f, "save local preservation requires an unlocked index"),
            Self::LocalCommandFailed => write!(
                f,
                "local save step failed; retain observed effects and assess the remaining state before retrying"
            ),
        }
    }
}
impl std::error::Error for ExecutionError {}
impl From<SaveError> for ExecutionError {
    fn from(e: SaveError) -> Self {
        Self::Save(e)
    }
}
impl From<repository::InspectionError> for ExecutionError {
    fn from(e: repository::InspectionError) -> Self {
        Self::Save(e.into())
    }
}
impl From<publication::PublishError> for ExecutionError {
    fn from(e: publication::PublishError) -> Self {
        Self::PublicationPreflight(e)
    }
}

fn stash(directory: &Path) -> Result<Option<Vec<u8>>, SaveError> {
    let args = vec![
        "rev-parse".into(),
        "--verify".into(),
        "--quiet".into(),
        "refs/stash".into(),
    ];
    let output = git::inspect(directory, &args)?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(repository::InspectionError::Git {
            arguments: args,
            output,
        }
        .into());
    }
    Ok(Some(
        output
            .stdout
            .strip_suffix(b"\n")
            .ok_or(repository::InspectionError::InvalidOutput("stash ref"))?
            .to_vec(),
    ))
}
fn observe(directory: &Path, plan: &Plan) -> LocalObservation {
    LocalObservation {
        repository: super::observe_repository(directory, plan),
        // A stale plan can name a relocated repository or an earlier index
        // selection. Report the index Git selects here; resolution/read errors
        // remain unavailable observations rather than bytes from the old path.
        index: super::index_path(directory).and_then(|path| super::read_index(&path)),
        stash: stash(directory),
    }
}
// The caller may hold a stale library plan. Reuse the real before observation
// for local state and compare only the inputs needed by preservation, rather
// than reconstructing actions, settings and observations through assess again.
fn validate_local_entry(
    directory: &Path,
    approved: &Plan,
    before: &LocalObservation,
) -> Result<(), ExecutionError> {
    let state = match &before.repository {
        Ok(repository::Observation::Repository(state)) => state,
        _ => return Err(SaveError::ContextChanged.into()),
    };
    if !approved.snapshot.metadata.matches_environment()
        || **state != approved.snapshot.state
        || before.index.as_ref().ok() != Some(&approved.snapshot.index)
    {
        return Err(SaveError::ContextChanged.into());
    }
    unchanged_files(directory, approved, state, &approved.commit)?;
    unchanged_policy(directory, approved)?;
    let index_path = super::read(
        directory,
        &[
            "rev-parse".into(),
            "--path-format=absolute".into(),
            "--git-path".into(),
            "index".into(),
        ],
    )?;
    if index_path.strip_suffix(b"\n")
        != Some(approved.snapshot.index_path.as_os_str().as_encoded_bytes())
    {
        return Err(SaveError::ContextChanged.into());
    }
    // File snapshots bind ordinary contents. Diff also binds clean submodule
    // pointer changes, which status represents only as a changed flag.
    if super::read(
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
    )? != approved.snapshot.tracked_diff
    {
        return Err(SaveError::ContextChanged.into());
    }
    Ok(())
}
// Publication consumes the selected repository, source OID and routes, and
// validates those facts at its own entry. This private projection carries the
// completed commit and no pending local steps; it is not a new scope assessment
// or a claim about the current dirty state in a public report.
fn publication_plan(approved: &Plan, commit: Vec<u8>) -> Plan {
    let mut publishing = approved.clone();
    publishing.commit = commit;
    publishing.plan = SavePlan::PublishOnly;
    publishing.local_steps.clear();
    publishing
}
// A checkout can run an index hook. Only this path needs a freshly captured
// remaining-work tree before recovery deletion. Reuse selected static inputs;
// observe actual HEAD, status and index for the capture instead of reassessing
// the whole public save workflow and validating unchanged names again.
fn remaining_work(directory: &Path, publishing: &Plan) -> Result<groups::Trees, SaveError> {
    let repository::Observation::Repository(state) = repository::inspect_context_with_paths(
        directory,
        publishing.repository.clone(),
        &publishing.snapshot.metadata,
    )?
    else {
        return Err(SaveError::ContextChanged);
    };
    if state.head
        != (repository::Head::Attached {
            branch: publishing.branch.clone(),
            commit: publishing.commit.clone(),
        })
        || state.operations.is_active()
    {
        return Err(SaveError::ContextChanged);
    }
    let mut remaining = publishing.clone();
    remaining.scope = super::Scope::default();
    for change in &state.working_tree.as_ref().unwrap().changes {
        match &change.kind {
            repository::ChangeKind::Untracked => {
                remaining.scope.untracked.push(change.path.clone())
            }
            repository::ChangeKind::Conflicted { .. } => return Err(SaveError::Conflicts),
            repository::ChangeKind::Tracked {
                staged,
                unstaged,
                submodule,
                ..
            } => {
                if submodule.is_some_and(|s| s.tracked_changes || s.untracked_changes) {
                    return Err(SaveError::DirtySubmodules);
                }
                if *staged != repository::Change::Unchanged {
                    remaining.scope.staged.push(change.path.clone());
                }
                if *unstaged != repository::Change::Unchanged {
                    remaining.scope.remaining_tracked.push(change.path.clone());
                }
            }
        }
    }
    remaining.snapshot.index = match fs::read(&publishing.snapshot.index_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    groups::Trees::capture(directory, &remaining)
}
// Git commits can run hooks which alter both the selected artifact and the
// remaining work. Before the next local mutation, bind the HEAD and original
// work to the approved groups. Status detects newly introduced paths; the file
// snapshots detect edits within an existing path. These are stage observations,
// not a lock against concurrent writers.
fn unchanged_work(directory: &Path, approved: &Plan, commit: &[u8]) -> Result<(), SaveError> {
    let repository::Observation::Repository(state) = repository::inspect_context_with_paths(
        directory,
        approved.repository.clone(),
        &approved.snapshot.metadata,
    )?
    else {
        return Err(SaveError::ContextChanged);
    };
    unchanged_files(directory, approved, &state, commit)
}
fn unchanged_files(
    directory: &Path,
    approved: &Plan,
    state: &repository::RepositoryState,
    commit: &[u8],
) -> Result<(), SaveError> {
    if state.head
        != (repository::Head::Attached {
            branch: approved.branch.clone(),
            commit: commit.to_vec(),
        })
        || state.operations.is_active()
    {
        return Err(SaveError::ContextChanged);
    }
    for (path, expected) in &approved.snapshot.files {
        if super::file_snapshot(
            &directory.join(repository::os(path)?),
            approved.scope.untracked.contains(path),
        )? != *expected
        {
            return Err(SaveError::ContextChanged);
        }
    }
    if state
        .working_tree
        .as_ref()
        .unwrap()
        .changes
        .iter()
        .any(|change| {
            !approved
                .snapshot
                .files
                .iter()
                .any(|(path, _)| path == &change.path)
        })
    {
        return Err(SaveError::ContextChanged);
    }
    Ok(())
}
// A hook may change the selected policy or publication route. Git's commit and
// read-tree do not enforce those workflow choices. Refresh them after commit
// hooks, before continuing local preservation or publishing the captured tree.
fn unchanged_policy(directory: &Path, approved: &Plan) -> Result<(), ExecutionError> {
    if settings::load(directory, &approved.repository).map_err(SaveError::from)?
        != approved.settings
        || crate::tracking::snapshot(directory, &approved.branch)? != approved.snapshot.routing
        || super::read(
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
        return Err(SaveError::ContextChanged.into());
    }
    publication::preflight(directory, approved)?;
    Ok(())
}
#[derive(Default)]
struct Progress {
    recovery: Option<Recovery>,
    commands: Vec<CommandReport>,
    effects: Vec<Effect>,
    publication: Option<publication::Report>,
}
fn command(
    directory: &Path,
    report: &mut Progress,
    operation: Operation,
    arguments: Vec<OsString>,
) -> Result<(), ExecutionError> {
    let result = git::workflow(directory, &arguments);
    let success = result.as_ref().is_ok_and(|output| output.status.success());
    report.commands.push(CommandReport {
        operation,
        arguments,
        result,
    });
    if success {
        Ok(())
    } else {
        Err(ExecutionError::LocalCommandFailed)
    }
}
// An invalidated route is an execution precondition failure, not an attempted
// publication. Keep that distinction without running publication validation
// twice. Once push starts, its complete report owns failures and partial effects.
fn publish(directory: &Path, plan: &Plan, progress: &mut Progress) -> Result<(), ExecutionError> {
    let mut report = publication::publish(directory, plan);
    if report.push.is_none()
        && let Some(error) = report.error.take()
    {
        return Err(error.into());
    }
    progress.publication = Some(report);
    Ok(())
}
fn tree(directory: &Path) -> Result<Vec<u8>, SaveError> {
    Ok(super::read(directory, &["write-tree".into()])?
        .strip_suffix(b"\n")
        .ok_or(repository::InspectionError::InvalidOutput("index tree"))?
        .to_vec())
}
fn scope_mask(scope: SaveScope) -> usize {
    match scope {
        SaveScope::All => 3,
        SaveScope::Staged => 1,
        SaveScope::Unstaged => 2,
    }
}
fn set_index(
    directory: &Path,
    report: &mut Progress,
    expected: &[u8],
    target: &[u8],
) -> Result<bool, ExecutionError> {
    // Reference-transaction and index hooks can stage work that exists nowhere
    // else. read-tree does not protect that work when replacing the index.
    // Compare its tree with the last known stage result before any replacement;
    // an index refresh alone is harmless, while changed staged content is kept.
    let actual = tree(directory)?;
    if actual != expected {
        return Err(SaveError::ContextChanged.into());
    }
    if actual != target {
        command(
            directory,
            report,
            Operation::PrepareIndex {
                tree: target.to_vec(),
            },
            vec!["read-tree".into(), repository::os(target)?],
        )?;
        return Ok(true);
    }
    Ok(false)
}
fn commit(
    directory: &Path,
    approved: &Plan,
    current: &[u8],
    expected_index: &[u8],
    report: &mut Progress,
    message: &str,
    target: &[u8],
) -> Result<Vec<u8>, ExecutionError> {
    set_index(directory, report, expected_index, target)?;
    // read-tree can run a post-index-change hook. Do not commit on a branch
    // switched by that hook; the selected commit tree is checked after commit.
    let reference = super::read(
        directory,
        &["symbolic-ref".into(), "--quiet".into(), "HEAD".into()],
    )?;
    let head = super::read(directory, &["rev-parse".into(), "HEAD".into()])?;
    if reference.strip_suffix(b"\n")
        != Some(
            [b"refs/heads/".as_slice(), &approved.branch]
                .concat()
                .as_slice(),
        )
        || head.strip_suffix(b"\n") != Some(current)
    {
        return Err(SaveError::ContextChanged.into());
    }
    command(
        directory,
        report,
        Operation::CommitIndex {
            message: message.to_owned(),
        },
        vec!["commit".into(), "--message".into(), message.into()],
    )?;
    // Obtain the candidate required by subsequent commits/publication and bind
    // the hook-produced parent/tree to the exact selected work in one read.
    let raw = super::read(
        directory,
        &[
            "show".into(),
            "--no-patch".into(),
            "--format=%H%x00%P%x00%T".into(),
            "HEAD".into(),
            "--".into(),
        ],
    )?;
    let fields = raw
        .strip_suffix(b"\n")
        .ok_or(repository::InspectionError::InvalidOutput("save commit"))?
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>();
    if fields.len() != 3
        || fields[1] != current
        || fields[2] != target
        || tree(directory)? != target
    {
        return Err(SaveError::ContextChanged.into());
    }
    let commit = fields[0].to_vec();
    unchanged_work(directory, approved, &commit)?;
    unchanged_policy(directory, approved)?;
    report.effects.push(Effect::Checkpoint {
        commit: commit.clone(),
        message: message.to_owned(),
    });
    Ok(commit)
}

/// Preserve the invocation's original groups. All necessary tree separations are
/// established before refs/index/worktree effects. A visible recovery ref retains
/// a native-compatible original stash across failures and interruption; it is
/// removed only after the requested local state has been verified.
pub fn execute(directory: impl AsRef<Path>, approved: &Plan) -> Report {
    let directory = directory.as_ref();
    let root = approved
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    let before = observe(root, approved);
    let mut report = Progress::default();
    let result = (|| -> Result<(), ExecutionError> {
        if approved.required_input != RequiredInput::None || approved.publication.is_none() {
            return Err(ExecutionError::RequiredInput(approved.required_input));
        }
        if repository::discover(directory)?.as_ref() != Some(&approved.repository) {
            return Err(SaveError::ContextChanged.into());
        }
        if approved.local_steps.is_empty() {
            return publish(root, approved, &mut report);
        }
        validate_local_entry(root, approved, &before)?;
        let mut index_lock = approved.snapshot.index_path.as_os_str().to_owned();
        index_lock.push(".lock");
        match fs::symlink_metadata(PathBuf::from(index_lock)) {
            Ok(_) => return Err(ExecutionError::IndexLocked),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(SaveError::from(e).into()),
        }
        for step in &approved.local_steps {
            if step.operation.action != SaveKind::Stash {
                continue;
            }
            let selected_gitlink = approved
                .snapshot
                .state
                .working_tree
                .as_ref()
                .unwrap()
                .changes
                .iter()
                .any(|change| {
                    if let repository::ChangeKind::Tracked {
                        staged,
                        unstaged,
                        submodule: Some(_),
                        ..
                    } = &change.kind
                    {
                        (step.operation.scope != SaveScope::Unstaged
                            && *staged != repository::Change::Unchanged)
                            || (step.operation.scope != SaveScope::Staged
                                && *unstaged != repository::Change::Unchanged)
                    } else {
                        false
                    }
                });
            if selected_gitlink {
                return Err(SaveError::InvalidPlan("save cannot remove a selected submodule revision with a stash without changing the child checkout; commit that revision in the parent, or use native Git to stash the parent and manage the child checkout explicitly".into()).into());
            }
        }
        for ident in ["GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"] {
            super::read(root, &["var".into(), ident.into()])?;
        }
        let mut trees = groups::Trees::capture(root, approved)?;
        let active = usize::from(!approved.scope.staged.is_empty())
            | if approved.scope.has_remaining() { 2 } else { 0 };
        let mut selected = 0;
        let mut committed = 0;
        let mut needs_independent = false;
        for step in &approved.local_steps {
            let mask = scope_mask(step.operation.scope) & active;
            selected |= mask;
            if step.operation.action == SaveKind::Commit {
                committed |= mask;
                needs_independent |= committed == 2;
            }
        }
        let final_index = committed | (active & !selected & 1);
        let final_work = committed | (active & !selected);
        needs_independent |= final_index == 2 || final_work == 2;
        if needs_independent {
            trees.separate(root, approved)?;
        }
        // Commit-only saves leave working files in place. Checkout protection
        // is needed only when consuming a stash changes the worktree contents.
        let reconcile = trees.get(final_work) != trees.get(3);
        let protection = if reconcile {
            let protection = groups::CheckoutProtection::new(root, trees.get(final_work))?;
            protection.check(root)?;
            Some(protection)
        } else {
            None
        };
        // Construct artifacts while unreachable; no stash/ref/index/worktree
        // change is allowed until all required separation has succeeded.
        let stash_objects = approved
            .local_steps
            .iter()
            .map(|step| {
                if step.operation.action == SaveKind::Stash {
                    trees
                        .stash_object(root, approved, step.operation.scope, &step.message)
                        .map(Some)
                } else {
                    Ok(None)
                }
            })
            .collect::<Result<Vec<_>, SaveError>>()?;
        let recovery =
            trees.stash_object(root, approved, SaveScope::All, "twigsmith save recovery")?;
        let token = tempfile::Builder::new()
            .prefix("twigsmith-save-recovery-")
            .tempfile_in(&approved.repository.git_dir)
            .map_err(SaveError::from)?;
        let reference = format!(
            "refs/twigsmith/save-recovery/{}",
            token.path().file_name().unwrap().to_string_lossy()
        );
        let args = vec![
            "update-ref".into(),
            "--create-reflog".into(),
            "-m".into(),
            "twigsmith save original work".into(),
            reference.clone().into(),
            repository::os(&recovery)?,
            "".into(),
        ];
        let result = git::workflow(root, &args);
        // Successful conditional update-ref acknowledges retention. Only a
        // failed/missing acknowledgement needs a targeted recovery-ref read.
        let retained = if result.as_ref().is_ok_and(|output| output.status.success()) {
            true
        } else {
            git::inspect(root, ["rev-parse", "--verify", "--quiet", &reference]).is_ok_and(
                |output| {
                    output.status.success()
                        && output.stdout.strip_suffix(b"\n") == Some(recovery.as_slice())
                },
            )
        };
        report.recovery = Some(Recovery {
            commit: recovery,
            reference,
            retained,
        });
        let output = result.map_err(SaveError::from)?;
        if !output.status.success() {
            return Err(SaveError::from(repository::InspectionError::Git {
                arguments: args,
                output,
            })
            .into());
        }
        if !retained {
            return Err(SaveError::ContextChanged.into());
        }
        let mut current = approved.commit.clone();
        let mut current_index = trees.get(1).to_vec();
        let mut committed = 0;
        for (step, stash_object) in approved.local_steps.iter().zip(stash_objects) {
            match step.operation.action {
                SaveKind::Commit => {
                    committed |= scope_mask(step.operation.scope) & active;
                    current = commit(
                        root,
                        approved,
                        &current,
                        &current_index,
                        &mut report,
                        &step.message,
                        trees.get(committed),
                    )?;
                    current_index = trees.get(committed).to_vec();
                }
                SaveKind::Stash => {
                    let object = stash_object.unwrap();
                    command(
                        root,
                        &mut report,
                        Operation::Stash {
                            message: step.message.clone(),
                        },
                        vec![
                            "stash".into(),
                            "store".into(),
                            "--message".into(),
                            step.message.clone().into(),
                            repository::os(&object)?,
                        ],
                    )?;
                    report.effects.push(Effect::LocalStash {
                        commit: object,
                        message: step.message.clone(),
                    });
                }
            }
        }
        if approved
            .local_steps
            .last()
            .is_some_and(|step| step.operation.action == SaveKind::Stash)
        {
            unchanged_work(root, approved, &current)?;
            unchanged_policy(root, approved)?;
        }
        if let Some(protection) = &protection {
            // Align with original working files before consuming the stashed
            // groups. read-tree --reset -u can overwrite ignored obstructions,
            // so check those again after the index hook and before checkout.
            set_index(root, &mut report, &current_index, trees.get(3))?;
            unchanged_work(root, approved, &current)?;
            if tree(root)? != trees.get(3) {
                return Err(SaveError::ContextChanged.into());
            }
            protection.check(root)?;
            command(
                root,
                &mut report,
                Operation::Reconcile {
                    tree: trees.get(final_work).to_vec(),
                },
                vec![
                    "read-tree".into(),
                    "--reset".into(),
                    "-u".into(),
                    repository::os(trees.get(final_work))?,
                ],
            )?;
            current_index = trees.get(final_work).to_vec();
        }
        let index_changed = set_index(root, &mut report, &current_index, trees.get(final_index))?;
        if !reconcile && index_changed {
            unchanged_work(root, approved, &current)?;
        }
        let publishing = publication_plan(approved, current);
        // A successful no-op set_index already observed this exact tree. If it
        // ran read-tree, its hook may have changed the index and must be checked.
        if index_changed && tree(root)? != trees.get(final_index) {
            return Err(SaveError::ContextChanged.into());
        }
        if reconcile {
            unchanged_policy(root, approved)?;
            let observed = remaining_work(root, &publishing)?;
            if observed.get(3) != trees.get(final_work) {
                return Err(SaveError::ContextChanged.into());
            }
        }
        if let Some(recovery) = &mut report.recovery {
            let args = vec![
                "update-ref".into(),
                "-d".into(),
                recovery.reference.clone().into(),
                repository::os(&recovery.commit)?,
            ];
            let output = git::workflow(root, &args).map_err(SaveError::from)?;
            if !output.status.success() {
                return Err(SaveError::from(repository::InspectionError::Git {
                    arguments: args,
                    output,
                })
                .into());
            }
            recovery.retained = false;
        }
        publish(root, &publishing, &mut report)
    })();
    Report {
        plan: approved.plan.clone(),
        recovery: report.recovery,
        before,
        commands: report.commands,
        effects: report.effects,
        publication: report.publication,
        after: observe(root, approved),
        error: result.err(),
    }
}
