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
    revalidate(root, plan, plan.evidence.as_ref())?;
    let record = plan.record.as_ref().ok_or(Error::UnverifiedAttempt)?;
    let result = plan.result.as_ref().ok_or(Error::UnverifiedAttempt)?;
    if !matches!(
        plan.state,
        State::PreparedResult | State::PendingPublication | State::Published | State::Complete
    ) || hint(root, record.result_ref().as_bytes())? != RefObservation::Commit(result.clone())
        || ancestor(root, &record.target_before, result)? != Some(true)
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
    guard(root, plan)?;
    let (prefix, name) = remote::endpoint(root, plan, remote, destination, commands)?;
    guard(root, plan)?;
    let value = remote::observe(
        root,
        &prefix,
        &name,
        reference,
        plan.result.as_ref().ok_or(Error::UnverifiedAttempt)?.len(),
        commands,
    )?;
    guard(root, plan)?;
    Ok(value)
}
// No fetch is needed: completion only compares exact advertised IDs. Reobserve
// every target before each effect; a cached publication report grants no authority.
fn delivery(root: &Path, plan: &Plan, report: &mut Report) -> Result<(), Error> {
    guard(root, plan)?;
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
    if report.targets.is_empty() {
        return Err(Error::UnverifiedAttempt);
    }
    Ok(())
}
fn with_evidence(root: &Path, previous: &Plan, evidence: &RemoteEvidence) -> Result<Plan, Error> {
    let next = assess(root, &previous.request, Some(evidence))?;
    if next.snapshot != previous.snapshot
        || next.record != previous.record
        || next.repository != previous.repository
        || next.settings != previous.settings
        || next.source != previous.source
        || next.source_ref != previous.source_ref
        || next.target != previous.target
        || next.local_target != previous.local_target
    {
        return Err(FinishError::ContextChanged.into());
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
// Advance only fields explained by one exact completed effect. Recovery's graph
// proofs remain necessary, but cannot authorize unrelated hook mutations.
fn successor(root: &Path, previous: &Plan, effect: &super::Effect) -> Result<Plan, Error> {
    let next = assess(root, &previous.request, previous.evidence.as_ref())?;
    let mut expected = previous.snapshot.clone();
    let mut source = previous.source.clone();
    match effect {
        super::Effect::Checkout {
            reference,
            expected: oid,
        } => {
            let head = repository::Head::Attached {
                branch: reference[b"refs/heads/".len()..].to_vec(),
                commit: oid.clone(),
            };
            if next.snapshot.state.head != head
                || next
                    .snapshot
                    .state
                    .working_tree
                    .as_ref()
                    .is_none_or(|t| t.is_dirty())
            {
                return Err(FinishError::ContextChanged.into());
            }
            let before = previous.snapshot.head_log.as_deref().unwrap_or_default();
            let after = next.snapshot.head_log.as_deref().unwrap_or_default();
            let tail = after
                .strip_prefix(before)
                .ok_or(FinishError::ContextChanged)?;
            let from = current_ref(&previous.snapshot.state).ok_or(Error::UnverifiedAttempt)?;
            let message = [
                b"checkout: moving from ".as_slice(),
                &from[b"refs/heads/".len()..],
                b" to ",
                &reference[b"refs/heads/".len()..],
            ]
            .concat();
            let lines: Vec<_> = tail
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .collect();
            if lines.len() != 1
                || lines[0].split(|b| *b == b'\t').nth(1) != Some(message.as_slice())
            {
                return Err(FinishError::ContextChanged.into());
            }
            expected.state.head = head;
            expected.index = next.snapshot.index.clone();
            expected.head_log = next.snapshot.head_log.clone();
            // The current worktree's HEAD/branch fields change, but other worktrees
            // and all their occupation information must remain identical.
            expected.worktrees =
                switched_worktrees(&previous.snapshot.worktrees, previous, reference, oid)?;
            // Selection adds/removes active.json when leaving/returning to source.
            if previous.snapshot.records.last() != next.snapshot.records.last() {
                return Err(FinishError::ContextChanged.into());
            }
            expected.records = next.snapshot.records.clone();
        }
        super::Effect::DeleteLocalWork { .. } => {
            source = RefObservation::Absent;
            expected.source_log = None;
        }
        _ => return Err(Error::UnverifiedAttempt),
    }
    if next.snapshot != expected
        || next.source != source
        || next.record != previous.record
        || next.repository != previous.repository
        || next.settings != previous.settings
        || next.source_ref != previous.source_ref
        || next.target != previous.target
        || next.local_target != previous.local_target
    {
        return Err(FinishError::ContextChanged.into());
    }
    ready(&next)?;
    Ok(next)
}
fn switched_worktrees(
    bytes: &[u8],
    plan: &Plan,
    reference: &[u8],
    oid: &[u8],
) -> Result<Vec<u8>, Error> {
    let here = [
        b"worktree ".as_slice(),
        plan.repository
            .working_tree_root
            .as_ref()
            .ok_or(Error::UnverifiedAttempt)?
            .as_os_str()
            .as_encoded_bytes(),
    ]
    .concat();
    let mut current = false;
    let mut result = vec![];
    for field in bytes.split_inclusive(|b| *b == 0) {
        let value = field.strip_suffix(&[0]).unwrap_or(field);
        if value.starts_with(b"worktree ") {
            current = value == here;
        }
        let replacement = if current && value.starts_with(b"HEAD ") {
            Some([b"HEAD ".as_slice(), oid].concat())
        } else if current && value.starts_with(b"branch ") {
            Some([b"branch ".as_slice(), reference].concat())
        } else {
            None
        };
        if let Some(value) = replacement {
            result.extend(value);
            result.push(0);
        } else {
            result.extend(field);
        }
    }
    Ok(result)
}
fn retain_source(root: &Path, plan: &Plan, report: &mut Report) -> Result<Plan, Error> {
    let mut record = plan.record.clone().ok_or(Error::UnverifiedAttempt)?;
    if record.cleanup_source_log.is_some() {
        return Ok(plan.clone());
    }
    guard(root, plan)?;
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
    let next = assess(root, &plan.request, plan.evidence.as_ref())?;
    let mut expected = plan.snapshot.clone();
    *expected
        .records
        .last_mut()
        .ok_or(Error::UnverifiedAttempt)? =
        Some(serde_json::to_vec(&record).map_err(io::Error::other)?);
    if next.snapshot != expected || next.record.as_ref() != Some(&record) {
        return Err(FinishError::ContextChanged.into());
    }
    ready(&next)?;
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
    guard(root, plan)?;
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
                after: repository::inspect(root),
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
    report.commands.push(CommandReport {
        arguments,
        result,
        after: repository::inspect(root),
    });
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
    if ancestor(root, expected, &record.source)? != Some(true) {
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
    // Keep command failure/uncertain acknowledgement separate from actual absence.
    // A fresh retry can confirm absence without repeating deletion.
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
    }
    status?;
    if !acknowledged || observed? != RefObservation::Absent {
        return Err(Error::UnconfirmedDeletion);
    }
    outcome.state = DestinationState::Deleted;
    Ok(())
}
/// Complete only a corroborated retained attempt. Target delivery is reobserved
/// before each effect. No target publication, candidate checks, or integration.
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
        source: hint(root, &approved.source_ref),
        target: hint(root, &approved.target.reference),
        plan: assess(root, &approved.request, approved.evidence.as_ref()),
        remaining: repository::inspect(root),
        error: None,
    };
    let mut plan = approved.clone();
    report.error = (|| {
        guard(directory, &plan)?;
        guard(root, &plan)?;
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
            delivery(root, &plan, &mut report)?;
            guard(root, &plan)?;
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
            let status = remote::command(root, args, &mut report.commands);
            if let Ok(repository::Observation::Repository(after)) = repository::inspect(root)
                && after.head != plan.snapshot.state.head
            {
                report.effects.push(Effect::CheckedOut {
                    before: plan.snapshot.state.head.clone(),
                    after: after.head.clone(),
                });
            }
            status?;
            plan = successor(root, &plan, &effect)?;
        }
        if let Some(effect @ super::Effect::DeleteLocalWork { .. }) = plan
            .effects
            .iter()
            .find(|e| matches!(e, super::Effect::DeleteLocalWork { .. }))
            .cloned()
        {
            delivery(root, &plan, &mut report)?;
            plan = retain_source(root, &plan, &mut report)?;
            delivery(root, &plan, &mut report)?;
            let super::Effect::DeleteLocalWork {
                reference,
                expected,
            } = &effect
            else {
                unreachable!()
            };
            let status = delete_local(root, &plan, expected, &mut report);
            if hint(root, reference)? == RefObservation::Absent {
                report.effects.push(Effect::LocalWorkDeleted {
                    reference: reference.clone(),
                    expected: expected.clone(),
                });
            }
            status?;
            plan = successor(root, &plan, &effect)?;
        }
        for i in 0..report.destinations.len() {
            delivery(root, &plan, &mut report)?;
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
            guard(root, &plan)?;
            plan = with_evidence(root, &plan, &evidence)?;
            if outcome.error.is_some() {
                return Err(Error::Incomplete);
            }
            ready(&plan)?;
        }
        delivery(root, &plan, &mut report)?;
        if plan.state != State::Complete {
            return Err(Error::Incomplete);
        }
        Ok(())
    })()
    .err();
    report.source = hint(root, &approved.source_ref);
    report.target = hint(root, &approved.target.reference);
    let mut final_evidence = plan.evidence.clone();
    if !report.targets.is_empty()
        && let Some(evidence) = &mut final_evidence
    {
        evidence.targets = report.targets.clone();
    }
    report.plan = assess(root, &approved.request, final_evidence.as_ref());
    report.remaining = repository::inspect(root);
    report
}
