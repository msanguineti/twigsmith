//! Controlled refresh and working-branch preparation. No push or CLI policy.
use super::*;
use std::{path::PathBuf, process::Output};

#[derive(Debug)]
pub struct CommandReport {
    pub arguments: Vec<OsString>,
    pub result: Result<Output, io::Error>,
    pub after: Result<repository::Observation, InspectionError>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Observed {
        destination: Vec<u8>,
        reference: Vec<u8>,
        value: RefObservation,
    },
    /// Objects were fetched without local branch, tag, or tracking-ref updates.
    Fetched {
        destination: Vec<u8>,
        commit: Vec<u8>,
    },
    RecoveryRecorded(PathBuf),
    Updated {
        update: FinishUpdate,
        before: Vec<u8>,
        after: Vec<u8>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Refreshed,
    Prepared,
}
#[derive(Debug)]
pub struct Report {
    pub state: State,
    pub evidence: Option<RemoteEvidence>,
    /// Fresh assessment, including unresolved choices/constraints when stopped.
    pub plan: Option<Plan>,
    pub commands: Vec<CommandReport>,
    pub effects: Vec<Effect>,
    pub remaining: Result<repository::Observation, InspectionError>,
    pub error: Option<PreparationError>,
}
impl Report {
    pub fn is_prepared(&self) -> bool {
        self.state == State::Prepared && self.error.is_none()
    }
    /// Recovery follows the observed operation, even when a process failed after
    /// changing state. Commands that completed can still have hook/filter effects.
    pub fn guidance(&self) -> &'static str {
        if let Ok(repository::Observation::Repository(state)) = &self.remaining {
            if state.operations.rebase {
                return "Resolve conflicts and use git rebase --continue, or use git rebase --abort; then rerun finish.";
            }
            if state.operations.merge {
                return "Resolve conflicts and use git merge --continue, or use git merge --abort; then rerun finish.";
            }
        }
        if self.error.is_some() {
            "Inspect the reported Git output and remaining repository state, then reassess finish. Completed effects are retained."
        } else if self.state == State::Refreshed {
            "Remote history is refreshed; inspect the plan before preparing the branch."
        } else {
            "Branch preparation is complete; publication has not been attempted."
        }
    }
}
#[derive(Debug)]
pub enum PreparationError {
    Finish(FinishError),
    UnresolvedPlan,
    CommandFailed,
    DestinationChanged,
    RecoveryUnverified,
}
impl fmt::Display for PreparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Finish(e) => write!(f, "{e}"),
            Self::UnresolvedPlan => f.write_str("finish preparation requires resolved choices and satisfied constraints"),
            Self::CommandFailed => f.write_str("Git preparation command failed or was interrupted; inspect its output and remaining state"),
            Self::DestinationChanged => f.write_str("a resolved destination would select a different fetch endpoint; fresh destination assessment is required"),
            Self::RecoveryUnverified => f.write_str("the rebase outcome could not be verified; retain Git state and reassess finish"),
        }
    }
}
impl std::error::Error for PreparationError {}
impl From<FinishError> for PreparationError {
    fn from(e: FinishError) -> Self {
        Self::Finish(e)
    }
}
impl From<InspectionError> for PreparationError {
    fn from(e: InspectionError) -> Self {
        Self::Finish(e.into())
    }
}
impl From<io::Error> for PreparationError {
    fn from(e: io::Error) -> Self {
        Self::Finish(e.into())
    }
}
fn report(directory: &Path) -> Report {
    Report {
        state: State::Stopped,
        evidence: None,
        plan: None,
        commands: vec![],
        effects: vec![],
        remaining: repository::inspect(directory),
        error: None,
    }
}
fn command(
    directory: &Path,
    args: Vec<OsString>,
    action: Option<&str>,
    report: &mut Report,
) -> Result<Vec<u8>, PreparationError> {
    let result = git::workflow_with_reflog(directory, &args, action);
    let success = result.as_ref().is_ok_and(|o| o.status.success());
    let bytes = result
        .as_ref()
        .map(|o| o.stdout.clone())
        .unwrap_or_default();
    report.commands.push(CommandReport {
        arguments: args,
        result,
        after: repository::inspect(directory),
    });
    if success {
        Ok(bytes)
    } else {
        Err(PreparationError::CommandFailed)
    }
}
fn guard(directory: &Path, plan: &Plan) -> Result<(), PreparationError> {
    revalidate(directory, plan, plan.evidence.as_ref())?;
    Ok(())
}
fn observe(
    directory: &Path,
    plan: &Plan,
    endpoint: &[u8],
    reference: &[u8],
    report: &mut Report,
) -> Result<RefObservation, PreparationError> {
    guard(directory, plan)?;
    let bytes = command(
        directory,
        vec![
            "ls-remote".into(),
            "--refs".into(),
            "--heads".into(),
            "--".into(),
            repository::os(endpoint)?,
            repository::os(reference)?,
        ],
        None,
        report,
    )?;
    guard(directory, plan)?;
    let value = parse_advertisement(&bytes, reference, plan.commit.len())?;
    report.effects.push(Effect::Observed {
        destination: endpoint.to_vec(),
        reference: reference.to_vec(),
        value: value.clone(),
    });
    if let RefObservation::Commit(oid) = &value {
        guard(directory, plan)?;
        // Fetch the advertised OID itself: a ref moving between advertisement
        // and fetch cannot silently substitute different history. An endpoint
        // that refuses the exact object stops this run rather than guessing.
        let mut args: Vec<OsString> = [
            "--no-replace-objects",
            "fetch",
            "--no-tags",
            "--no-prune",
            "--no-prune-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--no-auto-maintenance",
            "--no-write-commit-graph",
            "--refmap=",
            "--",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        args.extend([repository::os(endpoint)?, repository::os(oid)?]);
        command(directory, args, None, report)?;
        report.effects.push(Effect::Fetched {
            destination: endpoint.to_vec(),
            commit: oid.clone(),
        });
        guard(directory, plan)?;
    }
    Ok(value)
}
fn refresh_inner(
    directory: &Path,
    plan: &Plan,
    report: &mut Report,
) -> Result<(), PreparationError> {
    guard(directory, plan)?;
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .expect("working plan");
    guard(root, plan)?;
    let base_ref = format!("refs/heads/{}", plan.settings.base).into_bytes();
    let destinations =
        save::publication::preflight(root, &plan.local).map_err(FinishError::Publication)?;
    let base = observe(
        root,
        plan,
        plan.settings.remote.as_bytes(),
        &base_ref,
        report,
    )?;
    let mut evidence = RemoteEvidence {
        remote: plan.settings.remote.clone(),
        base_ref,
        branch_ref: plan.publication.branch_ref.clone(),
        base,
        destinations: vec![],
    };
    report.evidence = Some(evidence.clone());
    for destination in destinations {
        // URLs returned by `remote get-url --push` are already rewritten. Fail
        // closed if using one for reads would rewrite it again or select a
        // same-named remote. Reading another endpoint cannot prove push history.
        guard(root, plan)?;
        let resolved = command(
            root,
            vec![
                "ls-remote".into(),
                "--get-url".into(),
                "--".into(),
                repository::os(&destination)?,
            ],
            None,
            report,
        )?;
        if resolved.strip_suffix(b"\n") != Some(destination.as_slice()) {
            return Err(PreparationError::DestinationChanged);
        }
        let branch = observe(root, plan, &destination, &evidence.branch_ref, report)?;
        evidence.destinations.push(DestinationEvidence {
            destination,
            branch,
        });
        report.evidence = Some(evidence.clone());
    }
    guard(root, plan)?;
    report.plan = Some(assess(root, &plan.request, Some(&evidence))?);
    report.state = State::Refreshed;
    Ok(())
}
/// Refresh after readiness/destination guards. The supplied plan can be the
/// initial read-only assessment without evidence or resolved update choices.
pub fn refresh(directory: impl AsRef<Path>, plan: &Plan) -> Report {
    let directory = directory.as_ref();
    let mut report = report(directory);
    report.error = refresh_inner(directory, plan, &mut report).err();
    if report.error.is_some() {
        report.state = State::Stopped;
    }
    report.remaining = repository::inspect(
        plan.repository
            .working_tree_root
            .as_deref()
            .unwrap_or(directory),
    );
    report
}
fn stable_after(before: &Plan, after: &Plan) -> bool {
    before.repository == after.repository
        && before.settings == after.settings
        && before.branch == after.branch
        && before.publication == after.publication
        && before.git_config == after.git_config
        && before.shallow == after.shallow
        && before.grafts == after.grafts
        && before.tracking_hints == after.tracking_hints
        && before.committed_settings == after.committed_settings
}
fn prepare_inner(
    directory: &Path,
    plan: &Plan,
    report: &mut Report,
) -> Result<(), PreparationError> {
    guard(directory, plan)?;
    if !plan.is_ready() {
        report.plan = Some(plan.clone());
        return Err(PreparationError::UnresolvedPlan);
    }
    // Reobserve destinations before acting, even for caller-supplied evidence.
    refresh_inner(directory, plan, report)?;
    if report.plan.as_ref() != Some(plan) {
        return Err(FinishError::ContextChanged.into());
    }
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .expect("working plan");
    if plan.update == FinishUpdate::None {
        report.state = State::Prepared;
        return Ok(());
    }
    let RefObservation::Commit(base) = &plan.evidence.as_ref().unwrap().base else {
        unreachable!("ready base")
    };
    guard(root, plan)?;
    let action = if plan.update == FinishUpdate::Rebase {
        let (path, action) = recovery::record(root, plan)?;
        report.effects.push(Effect::RecoveryRecorded(path));
        // Recovery metadata is the sole expected difference before execution.
        let fresh = assess(root, &plan.request, plan.evidence.as_ref())?;
        let mut expected = plan.clone();
        expected.recovery = fresh.recovery.clone();
        if fresh != expected {
            return Err(FinishError::ContextChanged.into());
        }
        Some(action)
    } else {
        None
    };
    let options = match plan.update {
        FinishUpdate::Rebase => vec![
            "--no-replace-objects",
            "-c",
            "core.logAllRefUpdates=true",
            "-c",
            "submodule.recurse=false",
            "rebase",
            "--rebase-merges=no-rebase-cousins",
            "--no-update-refs",
            "--no-autostash",
            "--no-autosquash",
            "--no-fork-point",
        ],
        FinishUpdate::Merge => vec![
            "--no-replace-objects",
            "-c",
            "submodule.recurse=false",
            "merge",
            "--no-edit",
            "--no-squash",
            "--commit",
            "--ff",
            "--no-autostash",
        ],
        _ => unreachable!("resolved update"),
    };
    let mut args: Vec<OsString> = options.iter().map(OsString::from).collect();
    args.push(repository::os(base)?);
    command(root, args, action.as_deref(), report)?;
    let after = assess(root, &plan.request, plan.evidence.as_ref())?;
    report.plan = Some(after.clone());
    if after.commit != plan.commit {
        report.effects.push(Effect::Updated {
            update: plan.update,
            before: plan.commit.clone(),
            after: after.commit.clone(),
        });
    }
    if !stable_after(plan, &after) {
        return Err(FinishError::ContextChanged.into());
    }
    if !after
        .base_relationship
        .is_some_and(Relationship::includes_remote)
        || (plan.update == FinishUpdate::Merge
            && ancestor(root, &plan.commit, &after.commit)? != Some(true))
    {
        return Err(FinishError::ContextChanged.into());
    }
    if plan.update == FinishUpdate::Rebase && !matches!(after.recovery, Recovery::Verified(_)) {
        return Err(PreparationError::RecoveryUnverified);
    }
    if !after.is_ready() {
        return Err(PreparationError::UnresolvedPlan);
    }
    report.state = State::Prepared;
    Ok(())
}
/// Execute resolved preparation. Conflicts/interruption and all completed
/// effects remain in place. Publication must use this report's exact evidence,
/// reassess its final graph and guard every destination with an explicit lease.
pub fn prepare(directory: impl AsRef<Path>, plan: &Plan) -> Report {
    let directory = directory.as_ref();
    let mut report = report(directory);
    report.error = prepare_inner(directory, plan, &mut report).err();
    if report.error.is_some() {
        report.state = State::Stopped;
    }
    report.remaining = repository::inspect(
        plan.repository
            .working_tree_root
            .as_deref()
            .unwrap_or(directory),
    );
    report
}

pub(super) fn parse_advertisement(
    bytes: &[u8],
    reference: &[u8],
    oid_length: usize,
) -> Result<RefObservation, FinishError> {
    let mut found = None;
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let Some(index) = line.iter().position(|b| *b == b'\t') else {
            return Err(FinishError::InvalidEvidence("invalid remote advertisement"));
        };
        let (oid, name) = (&line[..index], &line[index + 1..]);
        if name != reference {
            continue;
        } // ls-remote patterns can match ref tails.
        if !valid_oid(oid, oid_length) || found.is_some() {
            return Err(FinishError::InvalidEvidence(
                "ambiguous remote advertisement",
            ));
        }
        found = Some(oid.to_vec());
    }
    Ok(found.map_or(RefObservation::Absent, RefObservation::Commit))
}
