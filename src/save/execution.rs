//! Execute an approved save plan without interaction or rollback.
use super::{Plan, Request, RequiredInput, SaveError, groups, publication};
use crate::{
    git, repository,
    settings::{SaveKind, SavePlan, SaveScope},
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
fn fresh(directory: &Path) -> Result<Plan, SaveError> {
    super::assess(
        directory,
        &Request {
            plan: Some(SavePlan::PublishOnly),
            message: None,
            stash_message: None,
        },
    )
}
fn unchanged_context(before: &Plan, after: &Plan) -> Result<(), SaveError> {
    if before.repository != after.repository
        || before.branch != after.branch
        || before.settings != after.settings
        || before.snapshot.routing != after.snapshot.routing
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
    approved: &Plan,
    report: &mut Report,
    target: &[u8],
) -> Result<(), ExecutionError> {
    if tree(directory)? != target {
        command(
            directory,
            approved,
            report,
            Operation::PrepareIndex {
                tree: target.to_vec(),
            },
            vec!["read-tree".into(), repository::os(target)?],
        )?;
    }
    Ok(())
}
fn commit(
    directory: &Path,
    original: &Plan,
    current: &Plan,
    report: &mut Report,
    message: &str,
    target: &[u8],
) -> Result<Plan, ExecutionError> {
    guard(directory, current)?;
    set_index(directory, original, report, target)?;
    let prepared = fresh(directory)?;
    unchanged_context(current, &prepared)?;
    unchanged_files(directory, current, &prepared)?;
    if prepared.commit != current.commit || tree(directory)? != target {
        return Err(SaveError::ContextChanged.into());
    }
    guard(directory, &prepared)?;
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
    let committed_tree = super::read(
        directory,
        &[
            "rev-parse".into(),
            repository::os(&[after.commit.as_slice(), b"^{tree}"].concat())?,
        ],
    )?;
    if parents.strip_suffix(b"\n") != Some(current.commit.as_slice())
        || committed_tree.strip_suffix(b"\n") != Some(target)
        || tree(directory)? != target
    {
        return Err(SaveError::ContextChanged.into());
    }
    report.effects.push(Effect::Checkpoint {
        commit: after.commit.clone(),
        message: message.to_owned(),
    });
    Ok(after)
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
    let mut report = Report {
        plan: approved.plan.clone(),
        recovery: None,
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
        if approved.local_steps.is_empty() {
            let publishing = fresh(root)?;
            report.publication = Some(publication::publish(root, &publishing));
            return Ok(());
        }
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
        super::revalidate(root, approved)?;
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
        let protection = groups::CheckoutProtection::new(root, trees.get(final_work))?;
        protection.check(root)?;
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
        guard(root, approved)?;
        protection.check(root)?;
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
        let retained = git::inspect(root, ["rev-parse", "--verify", "--quiet", &reference])
            .is_ok_and(|output| {
                output.status.success()
                    && output.stdout.strip_suffix(b"\n") == Some(recovery.as_slice())
            });
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
        super::revalidate(root, approved)?;
        let mut current = approved.clone();
        let mut committed = 0;
        for (step, stash_object) in approved.local_steps.iter().zip(stash_objects) {
            guard(root, &current)?;
            protection.check(root)?;
            match step.operation.action {
                SaveKind::Commit => {
                    committed |= scope_mask(step.operation.scope) & active;
                    current = commit(
                        root,
                        approved,
                        &current,
                        &mut report,
                        &step.message,
                        trees.get(committed),
                    )?;
                }
                SaveKind::Stash => {
                    let object = stash_object.unwrap();
                    let before_stash = stash(root)?;
                    let before_history = stash_history(root)?;
                    command(
                        root,
                        approved,
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
                    let after = fresh(root)?;
                    unchanged_context(&current, &after)?;
                    unchanged_files(root, &current, &after)?;
                    if current.commit != after.commit
                        || current.snapshot.index != after.snapshot.index
                        || stash(root)?.as_deref() != Some(object.as_slice())
                    {
                        return Err(SaveError::ContextChanged.into());
                    }
                    let history = stash_history(root)?;
                    if history != [object.as_slice(), b"\n", &before_history].concat()
                        && !(before_stash.as_ref() == Some(&object) && history == before_history)
                    {
                        return Err(SaveError::ContextChanged.into());
                    }
                    report.effects.push(Effect::LocalStash {
                        commit: object,
                        message: step.message.clone(),
                    });
                    current = after;
                }
            }
        }
        guard(root, &current)?;
        // First align the index with the actual original worktree, so read-tree
        // can safely remove consumed tracked/addition paths. Eligible original
        // untracked files become index entries here and are excluded again by
        // the final index when their group was left local.
        set_index(root, approved, &mut report, trees.get(3))?;
        let staged = fresh(root)?;
        unchanged_context(&current, &staged)?;
        unchanged_files(root, &current, &staged)?;
        if staged.commit != current.commit {
            return Err(SaveError::ContextChanged.into());
        }
        protection.check(root)?;
        command(
            root,
            approved,
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
        let reconciled = fresh(root)?;
        unchanged_context(&current, &reconciled)?;
        if reconciled.commit != current.commit || tree(root)? != trees.get(final_work) {
            return Err(SaveError::ContextChanged.into());
        }
        let observed_work = groups::Trees::capture(root, &reconciled)?;
        if observed_work.get(3) != trees.get(final_work) {
            return Err(SaveError::ContextChanged.into());
        }
        super::revalidate(root, &reconciled)?;
        set_index(root, approved, &mut report, trees.get(final_index))?;
        let publishing = fresh(root)?;
        unchanged_context(&current, &publishing)?;
        if publishing.commit != current.commit || tree(root)? != trees.get(final_index) {
            return Err(SaveError::ContextChanged.into());
        }
        let observed = groups::Trees::capture(root, &publishing)?;
        if observed.get(3) != trees.get(final_work) {
            return Err(SaveError::ContextChanged.into());
        }
        super::revalidate(root, &publishing)?;
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
        super::revalidate(root, &publishing)?;
        report.publication = Some(publication::publish(root, &publishing));
        Ok(())
    })();
    report.error = result.err();
    report.after = observe(root, approved);
    report
}
