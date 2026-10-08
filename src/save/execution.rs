//! Execute an approved save plan without interaction or rollback.
use super::{
    LocalStep, Plan, Request, RequiredInput, SaveError, Scope, WIP_REMAINING_MESSAGE,
    WIP_STAGED_MESSAGE, publication,
};
use crate::{git, repository, settings::SaveAction};
use std::{
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Output,
};

pub const STASH_MESSAGE: &str = "git-oneflow save";

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
    StageRemaining { scope: Scope },
    Stash { message: String },
}
#[derive(Debug)]
pub struct CommandReport {
    pub operation: Operation,
    pub arguments: Vec<OsString>,
    /// Raw output/status, including failures that produced effects.
    pub result: Result<Output, io::Error>,
    pub after: LocalObservation,
}
/// Verified local steps. Failed or unexpectedly altered steps can also have
/// effects; those remain in command observations and the final remaining state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Checkpoint {
        commit: Vec<u8>,
        /// Message submitted to Git; normal hooks/cleanup may edit it.
        message: String,
    },
    StagedRemaining {
        scope: Scope,
    },
    /// Local preservation only: this object is never published by branch push.
    LocalStash {
        commit: Vec<u8>,
    },
}
#[derive(Debug)]
pub struct Report {
    pub action: SaveAction,
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
fn stash_history(directory: &Path) -> Result<Vec<u8>, SaveError> {
    if stash(directory)?.is_none() {
        return Ok(vec![]);
    }
    super::read(
        directory,
        &[
            "reflog".into(),
            "show".into(),
            "--format=%H".into(),
            "refs/stash".into(),
        ],
    )
}
fn observe(directory: &Path, plan: &Plan) -> LocalObservation {
    LocalObservation {
        repository: repository::inspect_without_upstream(directory),
        index: match fs::read(&plan.snapshot.index_path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        },
        stash: stash(directory),
    }
}
fn selected_diff(
    directory: &Path,
    parent: &[u8],
    commit: Option<&[u8]>,
) -> Result<Vec<u8>, SaveError> {
    let mut args: Vec<OsString> = [
        "-c",
        "diff.autoRefreshIndex=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--no-relative",
        "--ignore-submodules=none",
        "--raw",
        "--no-abbrev",
        "-z",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if commit.is_none() {
        args.push("--cached".into());
    }
    args.push(repository::os(parent)?);
    if let Some(commit) = commit {
        args.push(repository::os(commit)?);
    }
    args.push("--".into());
    super::read(directory, &args)
}
// Index-to-worktree comparison survives an index-only commit unchanged, and
// records clean submodule HEAD movement that directory snapshots cannot see.
fn remaining_diff(directory: &Path) -> Result<Vec<u8>, SaveError> {
    super::read(
        directory,
        &[
            "-c".into(),
            "diff.autoRefreshIndex=false".into(),
            "diff".into(),
            "--no-ext-diff".into(),
            "--no-textconv".into(),
            "--binary".into(),
            "--full-index".into(),
            "--no-renames".into(),
            "--no-relative".into(),
            "--submodule=short".into(),
            "--ignore-submodules=none".into(),
            "--".into(),
        ],
    )
}
// Add can change index categories, but must not approve a different child
// revision. Compare HEAD-to-worktree gitlink content across that step.
fn gitlink_diff(directory: &Path, plan: &Plan) -> Result<Vec<u8>, SaveError> {
    let paths: Vec<_> = plan
        .snapshot
        .state
        .working_tree
        .as_ref()
        .unwrap()
        .changes
        .iter()
        .filter(|change| {
            matches!(
                change.kind,
                repository::ChangeKind::Tracked {
                    submodule: Some(_),
                    ..
                }
            )
        })
        .map(|change| &change.path)
        .collect();
    if paths.is_empty() {
        return Ok(vec![]);
    }
    let mut args: Vec<OsString> = [
        "--literal-pathspecs",
        "-c",
        "diff.autoRefreshIndex=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--binary",
        "--full-index",
        "--no-renames",
        "--no-relative",
        "--submodule=short",
        "--ignore-submodules=none",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    args.push(repository::os(&plan.commit)?);
    args.push("--".into());
    for path in paths {
        args.push(repository::os(path)?);
    }
    super::read(directory, &args)
}
fn fresh(directory: &Path) -> Result<Plan, SaveError> {
    super::assess(
        directory,
        &Request {
            action: Some(SaveAction::PublishOnly),
            message: None,
        },
    )
}
fn unchanged_context(before: &Plan, after: &Plan) -> Result<(), SaveError> {
    if before.repository != after.repository
        || before.branch != after.branch
        || before.settings != after.settings
        || before.snapshot.upstream != after.snapshot.upstream
        || before.snapshot.push_urls != after.snapshot.push_urls
        || before.snapshot.index_path != after.snapshot.index_path
    {
        return Err(SaveError::ContextChanged);
    }
    Ok(())
}
// Commit/add may refresh the index and move paths between scope categories.
// Permit those expected effects only: all approved file contents must survive,
// and no new changed path may silently enter the next checkpoint/publication.
fn unchanged_files(directory: &Path, before: &Plan, after: &Plan) -> Result<(), SaveError> {
    for (path, expected) in &before.snapshot.files {
        if super::file_snapshot(
            &directory.join(repository::os(path)?),
            before.scope.untracked.contains(path),
        )? != *expected
        {
            return Err(SaveError::ContextChanged);
        }
    }
    if after
        .snapshot
        .files
        .iter()
        .any(|(path, _)| !before.snapshot.files.iter().any(|(old, _)| old == path))
    {
        return Err(SaveError::ContextChanged);
    }
    Ok(())
}
fn command(
    directory: &Path,
    plan: &Plan,
    report: &mut Report,
    operation: Operation,
    arguments: Vec<OsString>,
) -> Result<(), ExecutionError> {
    let result = git::workflow(directory, &arguments);
    let success = result.as_ref().is_ok_and(|output| output.status.success());
    report.commands.push(CommandReport {
        operation,
        arguments,
        result,
        after: observe(directory, plan),
    });
    if success {
        Ok(())
    } else {
        Err(ExecutionError::LocalCommandFailed)
    }
}
fn guard(directory: &Path, plan: &Plan) -> Result<(), ExecutionError> {
    super::revalidate(directory, plan)?;
    publication::preflight(directory, plan)?;
    Ok(())
}
fn commit(
    directory: &Path,
    original: &Plan,
    current: &Plan,
    report: &mut Report,
    message: &str,
) -> Result<Plan, ExecutionError> {
    guard(directory, current)?;
    let selection = selected_diff(directory, &current.commit, None)?;
    let remaining = remaining_diff(directory)?;
    if selection.is_empty() {
        return Err(SaveError::ContextChanged.into());
    }
    command(
        directory,
        original,
        report,
        Operation::CommitIndex {
            message: message.to_owned(),
        },
        vec!["commit".into(), "--message".into(), message.into()],
    )?;
    let after = fresh(directory)?;
    unchanged_context(current, &after)?;
    unchanged_files(directory, current, &after)?;
    let parents = super::read(
        directory,
        &[
            "show".into(),
            "--no-patch".into(),
            "--format=%P".into(),
            repository::os(&after.commit)?,
            "--".into(),
        ],
    )?;
    if parents.strip_suffix(b"\n") != Some(current.commit.as_slice())
        || selected_diff(directory, &current.commit, Some(&after.commit))? != selection
        || !selected_diff(directory, &after.commit, None)?.is_empty()
        || remaining_diff(directory)? != remaining
    {
        return Err(SaveError::ContextChanged.into());
    }
    report.effects.push(Effect::Checkpoint {
        commit: after.commit.clone(),
        message: message.to_owned(),
    });
    Ok(after)
}

/// Revalidate the approval, preserve locally, then obtain and validate an exact
/// publish-only plan. Retry requires a new assessment of the actual repository.
pub fn execute(directory: impl AsRef<Path>, approved: &Plan) -> Report {
    let directory = directory.as_ref();
    let root = approved
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    let mut report = Report {
        action: approved.action,
        before: observe(root, approved),
        commands: vec![],
        effects: vec![],
        publication: None,
        after: observe(root, approved),
        error: None,
    };
    let result = (|| -> Result<(), ExecutionError> {
        if approved.required_input != RequiredInput::None || approved.publication.is_none() {
            return Err(ExecutionError::RequiredInput(approved.required_input));
        }
        super::revalidate(directory, approved)?;
        guard(root, approved)?;
        if !approved.local_steps.is_empty() {
            let mut index_lock = approved.snapshot.index_path.as_os_str().to_owned();
            index_lock.push(".lock");
            match fs::symlink_metadata(PathBuf::from(index_lock)) {
                Ok(_) => return Err(ExecutionError::IndexLocked),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(SaveError::from(e).into()),
            }
            // These known identity errors must precede add/stash effects.
            for ident in ["GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"] {
                super::read(root, &["var".into(), ident.into()])?;
            }
        }
        let mut current = approved.clone();
        for step in &approved.local_steps {
            guard(root, &current)?;
            match step {
                LocalStep::CommitIndex { message } => {
                    current = commit(root, approved, &current, &mut report, message)?
                }
                LocalStep::CommitStagedWip => {
                    current = commit(root, approved, &current, &mut report, WIP_STAGED_MESSAGE)?
                }
                LocalStep::StageAndCommitRemainingWip => {
                    let mut arguments: Vec<OsString> =
                        ["--literal-pathspecs", "add", "--all", "--"]
                            .into_iter()
                            .map(Into::into)
                            .collect();
                    let paths: Vec<_> = current
                        .scope
                        .remaining_tracked
                        .iter()
                        .chain(&current.scope.untracked)
                        .collect();
                    if paths.is_empty() {
                        continue;
                    }
                    for path in &paths {
                        arguments.push(repository::os(path)?);
                    }
                    let gitlinks = gitlink_diff(root, &current)?;
                    command(
                        root,
                        approved,
                        &mut report,
                        Operation::StageRemaining {
                            scope: current.scope.clone(),
                        },
                        arguments,
                    )?;
                    let staged = fresh(root)?;
                    unchanged_context(&current, &staged)?;
                    unchanged_files(root, &current, &staged)?;
                    if staged.commit != current.commit
                        || gitlink_diff(root, &current)? != gitlinks
                        || staged.scope.has_remaining()
                        || staged
                            .scope
                            .staged
                            .iter()
                            .any(|path| !paths.contains(&path))
                    {
                        return Err(SaveError::ContextChanged.into());
                    }
                    report.effects.push(Effect::StagedRemaining {
                        scope: staged.scope.clone(),
                    });
                    current = if staged.scope.staged.is_empty() {
                        staged
                    } else {
                        commit(root, approved, &staged, &mut report, WIP_REMAINING_MESSAGE)?
                    };
                }
                LocalStep::StashIncludingUntracked => {
                    let before_stash = stash(root)?;
                    let before_history = stash_history(root)?;
                    command(
                        root,
                        approved,
                        &mut report,
                        Operation::Stash {
                            message: STASH_MESSAGE.to_owned(),
                        },
                        vec![
                            "stash".into(),
                            "push".into(),
                            "--include-untracked".into(),
                            "--message".into(),
                            STASH_MESSAGE.into(),
                        ],
                    )?;
                    let after = fresh(root)?;
                    unchanged_context(&current, &after)?;
                    let after_stash = stash(root)?;
                    if after.commit != current.commit
                        || after.scope.tree_state() != super::TreeState::Clean
                        || after_stash.is_none()
                    {
                        return Err(SaveError::ContextChanged.into());
                    }
                    let stash_commit = after_stash.unwrap();
                    // Identical contents/timestamps can produce the same OID;
                    // Git can retain the existing entry in that case. Either
                    // retention or a single new entry must preserve older ones.
                    let history = stash_history(root)?;
                    let new_entry = [stash_commit.as_slice(), b"\n", &before_history].concat();
                    if history != new_entry
                        && !(before_stash.as_ref() == Some(&stash_commit)
                            && history == before_history)
                    {
                        return Err(SaveError::ContextChanged.into());
                    }
                    report.effects.push(Effect::LocalStash {
                        commit: stash_commit,
                    });
                    current = after;
                }
            }
        }
        let publishing = fresh(root)?;
        unchanged_context(&current, &publishing)?;
        // Reassessing must not approve unexpected edits after the last step.
        super::revalidate(root, &current)?;
        report.publication = Some(publication::publish(root, &publishing));
        Ok(())
    })();
    report.error = result.err();
    report.after = observe(root, approved);
    report
}
