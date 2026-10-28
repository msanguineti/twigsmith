//! Final checkout and exact work-ref deletion for a delivered, retained attempt.
use super::*;
use crate::{
    finish::preparation::CommandReport,
    publication::{Status, responses},
};
use std::{
    ffi::OsString,
    fmt,
    io::{self, BufRead, Read, Seek, Write},
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationState {
    NotAttempted,
    Deleted,
    Absent,
    Rejected,
    Unconfirmed,
}
impl DestinationState {
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Deleted | Self::Absent)
    }
}
#[derive(Debug)]
pub struct DestinationOutcome {
    pub destination: Vec<u8>,
    pub expected: RefObservation,
    pub observed: Option<RefObservation>,
    pub state: DestinationState,
    pub commands: Vec<CommandReport>,
    pub error: Option<Error>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    CheckedOut {
        before: repository::Head,
        after: repository::Head,
    },
    SourceEvidenceRetained,
    LocalWorkDeleted {
        reference: Vec<u8>,
        expected: Vec<u8>,
    },
}
#[derive(Debug)]
pub enum Error {
    Context(FinishError),
    Transport(remote::Error),
    UnverifiedAttempt,
    TargetDeliveryIncomplete,
    WorkChanged,
    CommandFailed,
    UnconfirmedDeletion,
    Incomplete,
}
impl From<FinishError> for Error {
    fn from(e: FinishError) -> Self {
        Self::Context(e)
    }
}
impl From<repository::InspectionError> for Error {
    fn from(e: repository::InspectionError) -> Self {
        Self::Context(e.into())
    }
}
impl From<remote::Error> for Error {
    fn from(e: remote::Error) -> Self {
        Self::Transport(e)
    }
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Context(e.into())
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(e) => write!(f, "{e}"), Self::Transport(e) => write!(f, "{e}"),
            Self::UnverifiedAttempt => f.write_str("completion requires the same retained, verified integration attempt and resolved choices"),
            Self::TargetDeliveryIncomplete => f.write_str("every target destination must still contain the exact integration result before completion"),
            Self::WorkChanged => f.write_str("the work branch changed; retain it and inspect the remaining cleanup"),
            Self::CommandFailed => f.write_str("Git stopped completion; inspect the command output and remaining state"),
            Self::UnconfirmedDeletion => f.write_str("the exact work-branch deletion was not confirmed; reobserve before retrying"),
            Self::Incomplete => f.write_str("completion is incomplete; retain completed effects and reassess the same attempt"),
        }
    }
}
impl std::error::Error for Error {}
#[derive(Debug)]
pub struct Report {
    pub commands: Vec<CommandReport>,
    pub effects: Vec<Effect>,
    pub targets: Vec<DestinationEvidence>,
    pub destinations: Vec<DestinationOutcome>,
    pub caches: Vec<crate::save::publication::CacheReport>,
    pub source: Result<RefObservation, FinishError>,
    pub target: Result<RefObservation, FinishError>,
    pub plan: Result<Plan, FinishError>,
    pub remaining: Result<repository::Observation, repository::InspectionError>,
    pub error: Option<Error>,
}
impl Report {
    pub fn is_complete(&self) -> bool {
        self.error.is_none()
            && self
                .plan
                .as_ref()
                .is_ok_and(|p| p.state == State::Complete && p.is_ready())
            && self
                .destinations
                .iter()
                .all(|d| d.state.is_complete() && d.error.is_none())
    }
    pub fn guidance(&self) -> &'static str {
        if self.is_complete() {
            "The selected checkout and cleanup are verified. Integration evidence and retained commits remain available."
        } else {
            "Inspect the reported output and remaining state, then reassess and retry completion. Delivered commits and completed cleanup remain in place."
        }
    }
}
fn guard(root: &Path, plan: &Plan) -> Result<(), Error> {
    revalidate_stage(root, plan, plan.evidence.as_ref())?;
    plan.record.as_ref().ok_or(Error::UnverifiedAttempt)?;
    let result = plan.result.as_ref().ok_or(Error::UnverifiedAttempt)?;
    if !matches!(
        plan.state,
        State::PreparedResult | State::PendingPublication | State::Published | State::Complete
    ) || plan
        .snapshot
        .pins
        .get(1)
        .is_none_or(|(pin, _)| *pin != RefObservation::Commit(result.clone()))
    {
        return Err(Error::UnverifiedAttempt);
    }
    Ok(())
}
fn observe(
    root: &Path,
    plan: &Plan,
    remote: &str,
    destination: &[u8],
    reference: &[u8],
    commands: &mut Vec<CommandReport>,
) -> Result<RefObservation, Error> {
    let (prefix, name) = remote::endpoint(root, plan, remote, destination, commands)?;
    let value = remote::observe(
        root,
        &prefix,
        &name,
        reference,
        plan.result.as_ref().ok_or(Error::UnverifiedAttempt)?.len(),
        commands,
    )?;
    Ok(value)
}
// Completion is a destructive stage: establish its complete delivery set once
// at entry. Hooks are separate boundaries; ordinary read-only calls do not revoke
// acknowledged delivery or require another transport round.
fn delivery(root: &Path, plan: &Plan, report: &mut Report) -> Result<(), Error> {
    if plan.has_remaining_destinations() {
        return Err(Error::TargetDeliveryIncomplete);
    }
    sequence::verify_completed_delivery(root, plan, &mut report.commands)?;
    // A failed or short observation round must not leave stale delivery evidence
    // in the remaining plan. Unvisited endpoints are explicitly unavailable.
    report.targets = plan
        .target
        .push_destinations
        .iter()
        .map(|destination| DestinationEvidence {
            destination: destination.clone(),
            branch: RefObservation::Unavailable,
        })
        .collect();
    for (index, destination) in plan.target.push_destinations.iter().enumerate() {
        let value = observe(
            root,
            plan,
            &plan.target.remote,
            destination,
            &plan.target.reference,
            &mut report.commands,
        )?;
        report.targets[index].branch = value.clone();
        if Some(&value)
            != plan
                .result
                .as_ref()
                .map(|r| RefObservation::Commit(r.clone()))
                .as_ref()
        {
            return Err(Error::TargetDeliveryIncomplete);
        }
    }
    release::verify_delivery(root, plan, &mut report.commands)?;
    if report.targets.is_empty() {
        return Err(Error::UnverifiedAttempt);
    }
    Ok(())
}
fn with_evidence(root: &Path, previous: &Plan, evidence: &RemoteEvidence) -> Result<Plan, Error> {
    if previous.cleanup == FinishCleanup::Both {
        let record = previous.record.as_ref().ok_or(Error::UnverifiedAttempt)?;
        if evidence.work.len() != record.work.len()
            || evidence
                .work
                .iter()
                .zip(&record.work)
                .any(|(now, expected)| {
                    now.destination != expected.destination
                        || (now.branch != RefObservation::Absent && now.branch != expected.branch)
                })
        {
            return Err(Error::WorkChanged);
        }
    }
    let mut next = previous.clone();
    next.evidence = Some(evidence.clone());
    if !matches!(previous.state, State::Published | State::Complete) {
        // A caller can enter completion with a formerly pending publication
        // plan after another process delivered its exact result. Derive cleanup
        // once at this stage transition, rather than replaying checks or pushes.
        next.integration = Integration::Unresolved;
        next = refresh_local(root, &next, false)?;
    }
    next.effects.retain(|effect| match effect {
        super::Effect::PublishTarget { destination, .. } => !evidence.targets.iter().any(|d| {
            &d.destination == destination
                && Some(&d.branch)
                    == next
                        .result
                        .as_ref()
                        .map(|oid| RefObservation::Commit(oid.clone()))
                        .as_ref()
        }),
        super::Effect::DeletePublishedWork { destination, .. } => !evidence
            .work
            .iter()
            .any(|d| &d.destination == destination && d.branch == RefObservation::Absent),
        _ => true,
    });
    if next.effects.is_empty() && next.is_ready() {
        next.state = State::Complete;
    } else if !next
        .effects
        .iter()
        .any(|e| matches!(e, super::Effect::PublishTarget { .. }))
    {
        next.state = State::Published;
    }
    Ok(next)
}
fn ready(plan: &Plan) -> Result<(), Error> {
    if plan.is_ready() && matches!(plan.state, State::Published | State::Complete) {
        Ok(())
    } else {
        Err(Error::UnverifiedAttempt)
    }
}
// Only Git effects completed in this stage advance its plan. Local refresh is
// needed after checkout hooks and ref transactions; it does not reconstruct the
// complete recovery graph or re-prove acknowledged remote delivery.
fn successor(root: &Path, previous: &Plan, effect: &super::Effect) -> Result<Plan, Error> {
    // Conditional deletion changes only the source ref/reflog. Its integration
    // proof and retained ownership were established at stage entry; committed
    // transaction hooks still need an identity/cleanliness guard, not a replay
    // of the complete recovery proof after our own acknowledged deletion.
    let deleting = matches!(effect, super::Effect::DeleteLocalWork { .. });
    let mut next = if deleting {
        let mut next = previous.clone();
        next.source = RefObservation::Absent;
        next.snapshot.source_log = None;
        next
    } else {
        refresh_local(root, previous, false)?
    };
    match effect {
        super::Effect::Checkout {
            reference,
            expected,
        } => {
            if current_ref(&next.snapshot.state).as_deref() != Some(reference)
                || commit(&hint(root, reference)?) != Some(expected.as_slice())
                || next
                    .snapshot
                    .state
                    .working_tree
                    .as_ref()
                    .is_none_or(|t| t.is_dirty())
            {
                return Err(FinishError::ContextChanged.into());
            }
        }
        super::Effect::DeleteLocalWork { reference, .. } => {
            if hint(root, reference)? != RefObservation::Absent {
                return Err(Error::WorkChanged);
            }
        }
        _ => return Err(Error::UnverifiedAttempt),
    }
    next.effects.retain(|pending| pending != effect);
    if next.effects.is_empty() && next.is_ready() {
        next.state = State::Complete;
    }
    if deleting {
        guard(root, &next)?;
    }
    ready(&next)?;
    Ok(next)
}
fn retain_source(plan: &Plan, report: &mut Report) -> Result<Plan, Error> {
    let mut record = plan.record.clone().ok_or(Error::UnverifiedAttempt)?;
    if record.cleanup_source_log.is_some() {
        return Ok(plan.clone());
    }
    record.cleanup_source_log = plan.snapshot.source_log.clone();
    if record.cleanup_source_log.is_none() {
        return Err(Error::UnverifiedAttempt);
    }
    recovery::write(
        &plan.repository.git_dir,
        plan.recovery_path
            .as_ref()
            .ok_or(Error::UnverifiedAttempt)?,
        &record,
        true,
    )?;
    report.effects.push(Effect::SourceEvidenceRetained);
    let mut next = plan.clone();
    *next
        .snapshot
        .records
        .last_mut()
        .ok_or(Error::UnverifiedAttempt)? =
        Some(serde_json::to_vec(&record).map_err(io::Error::other)?);
    next.record = Some(record);
    Ok(next)
}
// Git 2.47's no-deref deletion still compares a symbolic ref's resolved OID.
// Prepare holds the ref lock, then inspect the actual ref and worktree occupancy
// after transaction hooks, before committing. Ordinary Git cannot replace the
// source with a same-OID symbolic ref during this interval. Occupancy remains a
// snapshot: Git has no shared transaction spanning every worktree's HEAD.
fn delete_local(
    root: &Path,
    plan: &Plan,
    expected: &[u8],
    report: &mut Report,
) -> Result<(), Error> {
    ready(plan)?;
    let arguments: Vec<OsString> = ["update-ref", "--no-deref", "--stdin"]
        .iter()
        .map(OsString::from)
        .collect();
    let mut stderr = tempfile::tempfile()?;
    let spawned = Command::new("git")
        .current_dir(root)
        .args(["--no-lazy-fetch", "-c", "core.fsmonitor=false"])
        .args(&arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr.try_clone()?)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            report.commands.push(CommandReport {
                arguments,
                result: Err(e),
            });
            return Err(Error::CommandFailed);
        }
    };
    let mut stdin = child.stdin.take().expect("piped input");
    let mut reader = io::BufReader::new(child.stdout.take().expect("piped output"));
    let mut stdout = vec![];
    let attempt = (|| {
        stdin.write_all(b"start\n")?;
        stdin.write_all(
            &[
                b"delete ".as_slice(),
                &plan.source_ref,
                b" ",
                expected,
                b"\nprepare\n",
            ]
            .concat(),
        )?;
        stdin.flush()?;
        loop {
            let mut line = vec![];
            if reader.read_until(b'\n', &mut line)? == 0 {
                return Err(Error::CommandFailed);
            }
            stdout.extend(&line);
            if line == b"prepare: ok\n" {
                break;
            }
        }
        guard(root, plan)?;
        ready(plan)?;
        if symbolic(root, &plan.source_ref)?
            || current_ref(&plan.snapshot.state).as_ref() == Some(&plan.source_ref)
            || in_another_worktree(plan, &plan.source_ref)
            || plan.source_ref == plan.target.reference
        {
            return Err(Error::WorkChanged);
        }
        stdin.write_all(b"commit\n")?;
        stdin.flush()?;
        Ok(())
    })();
    // Closing stdin without commit aborts the prepared transaction.
    drop(stdin);
    let remaining = reader.read_to_end(&mut stdout);
    let status = child.wait();
    stderr.rewind()?;
    let mut diagnostics = vec![];
    stderr.read_to_end(&mut diagnostics)?;
    let result = status.map(|status| std::process::Output {
        status,
        stdout,
        stderr: diagnostics,
    });
    let success = result.as_ref().is_ok_and(|o| o.status.success());
    report.commands.push(CommandReport { arguments, result });
    attempt?;
    remaining?;
    if !success {
        return Err(Error::CommandFailed);
    }
    Ok(())
}
fn delete_remote(root: &Path, plan: &Plan, outcome: &mut DestinationOutcome) -> Result<(), Error> {
    outcome.state = DestinationState::Unconfirmed;
    let now = observe(
        root,
        plan,
        &plan.settings.publish_remote,
        &outcome.destination,
        &plan.source_ref,
        &mut outcome.commands,
    )?;
    outcome.observed = Some(now.clone());
    if now == RefObservation::Absent {
        outcome.state = DestinationState::Absent;
        return Ok(());
    }
    let expected = commit(&outcome.expected).ok_or(Error::WorkChanged)?;
    if now != outcome.expected {
        outcome.state = DestinationState::Rejected;
        return Err(Error::WorkChanged);
    }
    let record = plan.record.as_ref().ok_or(Error::UnverifiedAttempt)?;
    if !record.incorporates_published_work(root, plan, expected)? {
        return Err(Error::WorkChanged);
    }
    let (mut args, name) = remote::endpoint(
        root,
        plan,
        &plan.settings.publish_remote,
        &outcome.destination,
        &mut outcome.commands,
    )?;
    args.extend([
        "--no-replace-objects".into(),
        "push".into(),
        "--porcelain".into(),
        "--no-force".into(),
        "--no-follow-tags".into(),
        crate::save::publication::submodule_policy(root)
            .map_err(FinishError::from)?
            .into(),
        repository::os(
            &[
                b"--force-with-lease=".as_slice(),
                &plan.source_ref,
                b":",
                expected,
            ]
            .concat(),
        )?,
        "--".into(),
        name.into(),
        repository::os(&[b":".as_slice(), &plan.source_ref].concat())?,
    ]);
    guard(root, plan)?;
    ready(plan)?;
    let status = remote::command(root, args, &mut outcome.commands);
    let mut acknowledged = false;
    if let Ok(output) = &outcome.commands.last().expect("push command").result {
        let (parsed, valid) = responses(&output.stdout, b"", &plan.source_ref);
        if valid && parsed.len() == 1 {
            acknowledged = parsed[0].state == Status::Deleted;
            if parsed[0].state == Status::Rejected {
                outcome.state = DestinationState::Rejected;
            }
        }
    }
    // Git's exact per-ref acknowledgement establishes deletion. Only an
    // uncertain response needs a readback; command failure remains reportable
    // even when that readback establishes a completed effect.
    if acknowledged {
        outcome.observed = Some(RefObservation::Absent);
        outcome.state = DestinationState::Deleted;
        status?;
        return Ok(());
    }
    let observed = observe(
        root,
        plan,
        &plan.settings.publish_remote,
        &outcome.destination,
        &plan.source_ref,
        &mut outcome.commands,
    );
    if let Ok(value) = &observed {
        outcome.observed = Some(value.clone());
        if *value == RefObservation::Absent {
            outcome.state = DestinationState::Absent;
        }
    }
    status?;
    if observed? != RefObservation::Absent {
        return Err(Error::UnconfirmedDeletion);
    }
    Ok(())
}
/// Complete only a corroborated retained attempt. Target delivery is reobserved
/// at the destructive-stage boundary. No target publication, checks, or integration.
pub fn complete(directory: impl AsRef<Path>, approved: &Plan) -> Report {
    let directory = directory.as_ref();
    let root = approved
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    let mut report = Report {
        commands: vec![],
        effects: vec![],
        targets: vec![],
        destinations: vec![],
        caches: vec![],
        source: Ok(approved.source.clone()),
        target: Ok(approved.local_target.clone()),
        plan: Ok(approved.clone()),
        remaining: Ok(repository::Observation::Repository(Box::new(
            approved.snapshot.state.clone(),
        ))),
        error: None,
    };
    let mut plan = approved.clone();
    let work_cache = crate::tracking::cache::Prepared::new(
        root,
        &plan.settings.publish_remote,
        &plan.source_ref,
    );
    let target_cache =
        crate::tracking::cache::Prepared::new(root, &plan.target.remote, &plan.target.reference);
    let completed_caches = plan.record.as_ref().map_or_else(Vec::new, |record| {
        record
            .completed
            .iter()
            .map(|done| {
                (
                    done.clone(),
                    crate::tracking::cache::Prepared::new(
                        root,
                        &done.target.remote,
                        &done.target.reference,
                    ),
                )
            })
            .collect::<Vec<_>>()
    });
    report.error = (|| {
        validate_repository(directory, &plan)?;
        guard(directory, &plan)?;
        delivery(root, &plan, &mut report)?;
        let mut evidence = plan.evidence.clone().ok_or(Error::UnverifiedAttempt)?;
        evidence.targets = report.targets.clone();
        if plan.cleanup == FinishCleanup::Both {
            evidence.work.clear();
            for destination in urls(root, &plan.settings.publish_remote, true)? {
                let branch = observe(
                    root,
                    &plan,
                    &plan.settings.publish_remote,
                    &destination,
                    &plan.source_ref,
                    &mut report.commands,
                )?;
                evidence.work.push(DestinationEvidence {
                    destination,
                    branch,
                });
            }
        }
        plan = with_evidence(root, &plan, &evidence)?;
        ready(&plan)?;
        if plan.cleanup == FinishCleanup::Both {
            report.destinations = plan
                .record
                .as_ref()
                .ok_or(Error::UnverifiedAttempt)?
                .work
                .iter()
                .map(|d| DestinationOutcome {
                    destination: d.destination.clone(),
                    expected: d.branch.clone(),
                    observed: None,
                    state: DestinationState::NotAttempted,
                    commands: vec![],
                    error: None,
                })
                .collect();
        }
        if let Some(effect @ super::Effect::Checkout { .. }) = plan
            .effects
            .iter()
            .find(|e| matches!(e, super::Effect::Checkout { .. }))
            .cloned()
        {
            let super::Effect::Checkout { reference, .. } = &effect else {
                unreachable!()
            };
            let mut args: Vec<OsString> = [
                "-c",
                "submodule.recurse=false",
                "switch",
                "--no-guess",
                "--no-overwrite-ignore",
                "--",
            ]
            .iter()
            .map(OsString::from)
            .collect();
            args.push(repository::os(&reference[b"refs/heads/".len()..])?);
            let next = remote::command(root, args, &mut report.commands)
                .map_err(Error::from)
                .and_then(|_| successor(root, &plan, &effect));
            // Successor already observes the checkout after its hooks. Reuse
            // that observation for the effect report; only an uncertain or
            // rejected transition needs a separate remaining-state inspection.
            let after = match &next {
                Ok(next) => Some(next.snapshot.state.head.clone()),
                Err(_) => match repository::inspect(root) {
                    Ok(repository::Observation::Repository(after)) => Some(after.head),
                    _ => None,
                },
            };
            if let Some(after) = after
                && after != plan.snapshot.state.head
            {
                report.effects.push(Effect::CheckedOut {
                    before: plan.snapshot.state.head.clone(),
                    after,
                });
            }
            plan = next?;
        }
        if let Some(effect @ super::Effect::DeleteLocalWork { .. }) = plan
            .effects
            .iter()
            .find(|e| matches!(e, super::Effect::DeleteLocalWork { .. }))
            .cloned()
        {
            plan = retain_source(&plan, &mut report)?;
            let super::Effect::DeleteLocalWork {
                reference,
                expected,
            } = &effect
            else {
                unreachable!()
            };
            let status = delete_local(root, &plan, expected, &mut report);
            if status.is_ok() || hint(root, reference)? == RefObservation::Absent {
                report.effects.push(Effect::LocalWorkDeleted {
                    reference: reference.clone(),
                    expected: expected.clone(),
                });
            }
            status?;
            plan = successor(root, &plan, &effect)?;
        }
        for i in 0..report.destinations.len() {
            let outcome = &mut report.destinations[i];
            outcome.error = delete_remote(root, &plan, outcome).err();
            let mut evidence = plan.evidence.clone().ok_or(Error::UnverifiedAttempt)?;
            for d in &mut evidence.work {
                if d.destination == outcome.destination {
                    d.branch = if outcome.state.is_complete() {
                        RefObservation::Absent
                    } else {
                        RefObservation::Unavailable
                    };
                }
            }
            plan = with_evidence(root, &plan, &evidence)?;
            if outcome.error.is_some() {
                return Err(Error::Incomplete);
            }
            ready(&plan)?;
        }
        if plan.state != State::Complete {
            return Err(Error::Incomplete);
        }
        Ok(())
    })()
    .err();
    let targets = report
        .targets
        .iter()
        .filter(|d| d.branch == RefObservation::Commit(plan.result.clone().unwrap_or_default()))
        .map(|d| {
            (
                d.destination.clone(),
                plan.record.as_ref().map(|r| r.target_before.clone()),
            )
        })
        .collect::<Vec<_>>();
    if !targets.is_empty() {
        report.caches.push(target_cache.refresh_acknowledged(
            root,
            &targets,
            plan.result.as_deref(),
        ));
    }
    if !report.targets.is_empty() {
        for (done, cache) in &completed_caches {
            let endpoints = done
                .target
                .push_destinations
                .iter()
                .map(|endpoint| (endpoint.clone(), None))
                .collect::<Vec<_>>();
            report
                .caches
                .push(cache.refresh_acknowledged(root, &endpoints, Some(&done.result)));
        }
    }
    let deleted = report
        .destinations
        .iter()
        .filter(|d| d.state.is_complete())
        .map(|d| {
            (
                d.destination.clone(),
                commit(&d.expected).map(<[u8]>::to_vec),
            )
        })
        .collect::<Vec<_>>();
    if !deleted.is_empty() {
        report
            .caches
            .push(work_cache.refresh_acknowledged(root, &deleted, None));
    }
    if !report.caches.is_empty() && report.error.is_none() {
        report.error = guard(root, &plan).err();
    }
    report.source = hint(root, &approved.source_ref);
    report.target = hint(root, &approved.target.reference);
    let mut final_evidence = plan.evidence.clone();
    if !report.targets.is_empty()
        && let Some(evidence) = &mut final_evidence
    {
        evidence.targets = report.targets.clone();
    }
    report.plan = if report.error.is_none() {
        plan.evidence = final_evidence;
        Ok(plan)
    } else {
        // A failure can leave Git effects without acknowledgements. Reconstruct
        // recovery only here, using the small remote uncertainty collected above.
        assess(root, &approved.request, final_evidence.as_ref())
    };
    report.remaining = repository::inspect(root);
    report
}
