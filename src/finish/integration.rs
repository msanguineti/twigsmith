//! Ordinary-work local integration. These plans cannot be passed to the
//! review executor. Remote observations are supplied by the caller, never caches.
use super::{DestinationEvidence, FinishError, RefObservation, Relationship};
use super::{ancestor, hint, metadata, read, relationship, run, valid_oid};
use crate::{
    repository,
    save::SaveError,
    settings::{
        self, FinishCheckout, FinishCleanup, FinishIntegration, FinishMessage, FinishUpdate,
        Settings,
    },
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod execution;
pub mod recovery;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub update: Option<FinishUpdate>,
    pub integration: Option<FinishIntegration>,
    pub message: Option<String>,
    pub checkout: Option<FinishCheckout>,
    pub cleanup: Option<FinishCleanup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub remote: String,
    pub reference: Vec<u8>,
    pub fetch_destination: Vec<u8>,
    pub push_destinations: Vec<Vec<u8>>,
}

/// Fresh observations of the exact target at its fetch and push endpoints.
/// Work observations are required only for published-work cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEvidence {
    pub target: Target,
    pub fetched: RefObservation,
    pub targets: Vec<DestinationEvidence>,
    pub work_remote: String,
    pub work_ref: Vec<u8>,
    pub work: Vec<DestinationEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredInput {
    Update,
    Integration,
    Message,
    Cleanup,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    FreshRemoteEvidenceRequired,
    InvalidEvidence,
    TargetUnavailable,
    HistoryUnavailable,
    UnrelatedHistory,
    FastForwardRequired,
    LocalTargetWork,
    TargetInAnotherWorktree,
    SourceInAnotherWorktree,
    SymbolicReference,
    DirtyTree,
    GitOperation,
    GraftedHistory,
    CheckoutCleanupConflict,
    TargetChanged(Vec<u8>),
    WorkBranchChanged(Vec<u8>),
    Recovery(recovery::Problem),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Fresh,
    IncompleteOperation,
    PreparedResult,
    PendingPublication,
    Published,
    Complete,
    ChangedOrUnverifiable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integration {
    Unresolved,
    /// No new commit; the target already contains this source.
    AlreadyIntegrated,
    FastForward,
    MergeCommit,
    SquashCommit,
    /// Only preparation is permitted; assess the actual prepared graph again.
    AfterPreparation,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    UpdateWork {
        reference: Vec<u8>,
        expected: Vec<u8>,
        base: Vec<u8>,
        method: FinishUpdate,
    },
    Integrate {
        reference: Vec<u8>,
        local_expected: RefObservation,
        target: Vec<u8>,
        source: Vec<u8>,
        method: Integration,
    },
    CheckResult {
        commit: Vec<u8>,
    },
    PublishTarget {
        remote: String,
        destination: Vec<u8>,
        reference: Vec<u8>,
        expected: Vec<u8>,
        commit: Vec<u8>,
    },
    Checkout {
        reference: Vec<u8>,
        expected: Vec<u8>,
    },
    DeleteLocalWork {
        reference: Vec<u8>,
        expected: Vec<u8>,
    },
    DeletePublishedWork {
        remote: String,
        destination: Vec<u8>,
        reference: Vec<u8>,
        expected: Vec<u8>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub repository: repository::RepositoryContext,
    pub settings: Settings,
    pub source_ref: Vec<u8>,
    pub source: RefObservation,
    pub target: Target,
    pub local_target: RefObservation,
    pub update: FinishUpdate,
    pub method: FinishIntegration,
    pub checkout: FinishCheckout,
    pub cleanup: FinishCleanup,
    pub integration: Integration,
    pub state: State,
    pub result: Option<Vec<u8>>,
    pub required_input: Vec<RequiredInput>,
    pub blockers: Vec<Blocker>,
    /// Prospective effects; use permitted_effects to require all plan guards.
    pub effects: Vec<Effect>,
    pub recovery_path: Option<PathBuf>,
    pub evidence: Option<RemoteEvidence>,
    request: Request,
    snapshot: Snapshot,
    record: Option<recovery::Record>,
}
impl Plan {
    pub fn is_ready(&self) -> bool {
        self.blockers.is_empty() && self.required_input.is_empty()
    }
    pub fn permitted_effects(&self) -> &[Effect] {
        if self.is_ready() { &self.effects } else { &[] }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    state: repository::RepositoryState,
    config: Vec<u8>,
    index: Option<Vec<u8>>,
    shallow: Option<Vec<u8>>,
    grafts: Option<Vec<u8>>,
    head_log: Option<Vec<u8>>,
    source_log: Option<Vec<u8>>,
    target_log: Option<Vec<u8>>,
    worktrees: Vec<u8>,
    records: Vec<Option<Vec<u8>>>,
    committed_settings: Vec<u8>,
    pins: Vec<(RefObservation, Option<Vec<u8>>)>,
}
fn branch_ref(branch: &[u8]) -> Vec<u8> {
    [b"refs/heads/", branch].concat()
}
fn log(directory: &Path, reference: &[u8]) -> Result<Option<Vec<u8>>, FinishError> {
    let name = std::str::from_utf8(reference)
        .map_err(|_| FinishError::InvalidEvidence("non-UTF-8 recovery reference"))?;
    metadata(directory, &format!("logs/{name}"))
}
fn urls(directory: &Path, remote: &str, push: bool) -> Result<Vec<Vec<u8>>, FinishError> {
    let mut args = vec!["remote", "get-url"];
    if push {
        args.extend(["--push", "--all"]);
    }
    args.push(remote);
    let bytes = read(directory, &args)?;
    let values: Vec<_> = bytes
        .strip_suffix(b"\n")
        .unwrap_or_default()
        .split(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    if values.iter().any(|v| v.is_empty() || v.contains(&b'\r')) {
        return Err(FinishError::InvalidEvidence("ambiguous remote endpoint"));
    }
    Ok(values)
}
fn symbolic(directory: &Path, reference: &[u8]) -> Result<bool, FinishError> {
    let output = run(
        directory,
        &[
            "symbolic-ref".into(),
            "--quiet".into(),
            repository::os(reference)?,
        ],
    )?;
    Ok(output.status.code() != Some(1))
}
fn current_ref(state: &repository::RepositoryState) -> Option<Vec<u8>> {
    match &state.head {
        repository::Head::Attached { branch, .. } => Some(branch_ref(branch)),
        _ => None,
    }
}
fn commit(observation: &RefObservation) -> Option<&[u8]> {
    match observation {
        RefObservation::Commit(oid) => Some(oid),
        _ => None,
    }
}

/// No network, helper execution, object writes, or metadata writes. Calling this
/// API selects direct integration; mode dispatch remains the caller's concern.
pub fn assess(
    directory: impl AsRef<Path>,
    request: &Request,
    evidence: Option<&RemoteEvidence>,
) -> Result<Plan, FinishError> {
    let directory = directory.as_ref();
    let repository = repository::discover(directory)?.ok_or(SaveError::OutsideRepository)?;
    if repository.kind == repository::RepositoryKind::Bare {
        return Err(SaveError::BareRepository.into());
    }
    let settings = settings::load(directory, &repository).map_err(SaveError::from)?;
    if request
        .message
        .as_ref()
        .is_some_and(|m| m.trim().is_empty() || m.contains('\0'))
    {
        return Err(SaveError::BlankMessage.into());
    }
    let repository::Observation::Repository(state) =
        repository::inspect_without_upstream(directory)?
    else {
        return Err(SaveError::OutsideRepository.into());
    };
    let target = Target {
        remote: settings.base_remote.clone(),
        reference: branch_ref(settings.base.as_bytes()),
        fetch_destination: urls(directory, &settings.base_remote, false)?.remove(0),
        push_destinations: urls(directory, &settings.base_remote, true)?,
    };
    let selection = recovery::select(directory, &repository, &state, &target.reference)?;
    let source_ref = selection
        .source_ref
        .clone()
        .or_else(|| current_ref(&state))
        .unwrap_or_default();
    let source = if source_ref.is_empty() {
        RefObservation::Unavailable
    } else {
        hint(directory, &source_ref)?
    };
    let local_target = hint(directory, &target.reference)?;
    let mut config_args = vec!["ls-tree", "--full-tree", "-z", "HEAD", "--"];
    config_args.extend(settings::CONFIG_FILES);
    let snapshot = Snapshot {
        state: *state,
        config: read(directory, &["config", "--null", "--list"])?,
        index: metadata(directory, "index")?,
        shallow: metadata(directory, "shallow")?,
        grafts: metadata(directory, "info/grafts")?,
        head_log: metadata(directory, "logs/HEAD")?,
        source_log: if source_ref.is_empty() {
            None
        } else {
            log(directory, &source_ref)?
        },
        target_log: log(directory, &target.reference)?,
        worktrees: read(directory, &["worktree", "list", "--porcelain", "-z"])?,
        pins: if let Some(record) = &selection.record {
            [record.original_ref(), record.result_ref()]
                .iter()
                .map(|r| {
                    Ok((
                        hint(directory, r.as_bytes())?,
                        log(directory, r.as_bytes())?,
                    ))
                })
                .collect::<Result<_, FinishError>>()?
        } else {
            vec![]
        },
        records: selection.bytes,
        committed_settings: read(directory, &config_args)?,
    };
    let mut plan = Plan {
        repository,
        settings: settings.clone(),
        source_ref,
        source,
        target,
        local_target,
        update: request.update.unwrap_or(settings.finish_update),
        method: request.integration.unwrap_or(settings.finish_integration),
        checkout: request.checkout.unwrap_or(settings.finish_checkout),
        cleanup: request.cleanup.unwrap_or(settings.finish_cleanup),
        integration: Integration::Unresolved,
        state: State::Fresh,
        result: None,
        required_input: vec![],
        blockers: vec![],
        effects: vec![],
        recovery_path: selection.path,
        evidence: evidence.cloned(),
        request: request.clone(),
        snapshot,
        record: selection.record,
    };
    if let Some(problem) = selection.problem {
        plan.blockers.push(Blocker::Recovery(problem));
        plan.state = State::ChangedOrUnverifiable;
    }
    if plan.snapshot.grafts.as_ref().is_some_and(|b| !b.is_empty()) {
        plan.blockers.push(Blocker::GraftedHistory);
    }
    if symbolic(directory, &plan.target.reference)?
        || (!plan.source_ref.is_empty() && symbolic(directory, &plan.source_ref)?)
    {
        plan.blockers.push(Blocker::SymbolicReference);
    }
    if plan
        .snapshot
        .state
        .working_tree
        .as_ref()
        .is_some_and(|t| t.is_dirty())
    {
        plan.blockers.push(Blocker::DirtyTree);
    }
    if plan.snapshot.state.operations.is_active() {
        plan.state = State::IncompleteOperation;
        plan.blockers.push(Blocker::GitOperation);
    }
    // A target checked out elsewhere cannot be advanced behind its worktree.
    if in_another_worktree(&plan, &plan.target.reference) {
        plan.blockers.push(Blocker::TargetInAnotherWorktree);
    }
    if let Some(record) = plan.record.clone() {
        recovery::assess(directory, &mut plan, &record)?;
    } else if !plan
        .blockers
        .iter()
        .any(|b| matches!(b, Blocker::Recovery(_)))
    {
        assess_fresh(directory, &mut plan)?;
    }
    Ok(plan)
}

fn in_another_worktree(plan: &Plan, reference: &[u8]) -> bool {
    let here = plan
        .repository
        .working_tree_root
        .as_ref()
        .expect("working repository")
        .as_os_str()
        .as_encoded_bytes();
    let branch = [b"branch ", reference].concat();
    let worktree = [b"worktree ", here].concat();
    plan.snapshot
        .worktrees
        .split(|b| *b == 0)
        .collect::<Vec<_>>()
        .split(|v| v.is_empty())
        .any(|entry| entry.contains(&branch.as_slice()) && !entry.contains(&worktree.as_slice()))
}
fn validate_evidence(plan: &mut Plan) -> bool {
    let Some(e) = &plan.evidence else {
        plan.blockers.push(Blocker::FreshRemoteEvidenceRequired);
        return false;
    };
    if e.target != plan.target
        || e.work_remote != plan.settings.publish_remote
        || e.work_ref != plan.source_ref
        || e.targets.len() != plan.target.push_destinations.len()
        || e.targets
            .iter()
            .zip(&plan.target.push_destinations)
            .any(|(e, u)| &e.destination != u)
    {
        plan.blockers.push(Blocker::InvalidEvidence);
        return false;
    }
    if e.targets.iter().any(|a| {
        e.targets
            .iter()
            .any(|b| a.destination == b.destination && a.branch != b.branch)
    }) {
        plan.blockers.push(Blocker::InvalidEvidence);
        return false;
    }
    true
}
// Deletion has a distinct destination role and requires its own observations.
fn validate_work(directory: &Path, plan: &mut Plan) -> Result<(), FinishError> {
    let e = plan.evidence.as_ref().expect("validated evidence");
    let destinations = urls(directory, &plan.settings.publish_remote, true)?;
    if e.work.len() != destinations.len()
        || e.work
            .iter()
            .zip(&destinations)
            .any(|(d, u)| &d.destination != u)
        || e.work.iter().any(|a| {
            e.work
                .iter()
                .any(|b| a.destination == b.destination && a.branch != b.branch)
        })
    {
        plan.blockers.push(Blocker::InvalidEvidence);
        return Ok(());
    }
    for d in &e.work {
        if d.branch == RefObservation::Absent {
            continue;
        }
        let incorporated = match (commit(&plan.source), commit(&d.branch)) {
            (Some(source), Some(remote)) if valid_oid(remote, source.len()) => {
                ancestor(directory, remote, source)? == Some(true)
            }
            _ => false,
        };
        if !incorporated {
            plan.blockers
                .push(Blocker::WorkBranchChanged(d.destination.clone()));
        }
    }
    Ok(())
}
fn assess_fresh(directory: &Path, plan: &mut Plan) -> Result<(), FinishError> {
    if plan.source_ref == plan.target.reference || plan.source_ref.is_empty() {
        plan.blockers
            .push(Blocker::Recovery(recovery::Problem::UnassociatedHead));
        return Ok(());
    }
    let Some(source) = commit(&plan.source).map(<[u8]>::to_vec) else {
        plan.blockers.push(Blocker::HistoryUnavailable);
        return Ok(());
    };
    if !validate_evidence(plan) {
        return Ok(());
    }
    if plan.cleanup == FinishCleanup::Both {
        validate_work(directory, plan)?;
    }
    let e = plan.evidence.as_ref().expect("validated");
    let Some(base) = commit(&e.fetched).map(<[u8]>::to_vec) else {
        plan.blockers.push(Blocker::TargetUnavailable);
        return Ok(());
    };
    let shallow = plan.snapshot.shallow.is_some();
    let relation = relationship(directory, &source, &e.fetched, shallow)?;
    for destination in &e.targets {
        // A split target is not silently reconciled, even if a rewrite was allowed
        // for working-branch publication. Every target must match the selected base.
        if destination.branch != e.fetched {
            plan.blockers
                .push(Blocker::TargetChanged(destination.destination.clone()));
        }
    }
    match &plan.local_target {
        RefObservation::Absent => (),
        RefObservation::Commit(local)
            if matches!(
                relationship(
                    directory,
                    &base,
                    &RefObservation::Commit(local.clone()),
                    shallow
                )?,
                Relationship::Equal | Relationship::Ahead
            ) => {}
        _ => plan.blockers.push(Blocker::LocalTargetWork),
    }
    match relation {
        Relationship::Equal | Relationship::Behind => {
            plan.update = FinishUpdate::None;
            plan.integration = Integration::AlreadyIntegrated;
            plan.result = Some(base.clone());
            // No integration choice or message is needed for already delivered work.
        }
        Relationship::Ahead | Relationship::Diverged => {
            if relation == Relationship::Ahead {
                plan.update = FinishUpdate::None;
            }
            if plan.update == FinishUpdate::Ask {
                plan.required_input.push(RequiredInput::Update);
            }
            if plan.method == FinishIntegration::Ask {
                plan.required_input.push(RequiredInput::Integration);
            }
            if matches!(plan.update, FinishUpdate::Merge | FinishUpdate::Rebase) {
                plan.integration = Integration::AfterPreparation;
                if in_another_worktree(plan, &plan.source_ref) {
                    plan.blockers.push(Blocker::SourceInAnotherWorktree);
                }
                plan.effects.push(Effect::UpdateWork {
                    reference: plan.source_ref.clone(),
                    expected: source.clone(),
                    base: base.clone(),
                    method: plan.update,
                });
                // Prepared work requires its own assessment; no target/publication
                // authority is attached to a predicted update result.
                return Ok(());
            }
            plan.integration = match plan.method {
                FinishIntegration::Ask => Integration::Unresolved,
                FinishIntegration::FfOnly if relation == Relationship::Ahead => {
                    Integration::FastForward
                }
                FinishIntegration::FfOnly => {
                    plan.blockers.push(Blocker::FastForwardRequired);
                    Integration::Unresolved
                }
                FinishIntegration::Merge => Integration::MergeCommit,
                FinishIntegration::Squash => Integration::SquashCommit,
            };
            if matches!(
                plan.integration,
                Integration::MergeCommit | Integration::SquashCommit
            ) && plan.request.message.is_none()
                && plan.settings.finish_message == FinishMessage::Ask
            {
                plan.required_input.push(RequiredInput::Message);
            }
        }
        Relationship::Unrelated => plan.blockers.push(Blocker::UnrelatedHistory),
        _ => plan.blockers.push(Blocker::HistoryUnavailable),
    }
    completion_choices(plan);
    if plan.integration != Integration::Unresolved {
        plan.effects.push(Effect::Integrate {
            reference: plan.target.reference.clone(),
            local_expected: plan.local_target.clone(),
            target: base,
            source,
            method: plan.integration,
        });
    }
    // Publication and cleanup are planned only after a pinned result is verified.
    Ok(())
}
fn completion_choices(plan: &mut Plan) {
    if plan.cleanup == FinishCleanup::Ask {
        plan.required_input.push(RequiredInput::Cleanup);
    }
    if plan.checkout == FinishCheckout::Work
        && matches!(plan.cleanup, FinishCleanup::Local | FinishCleanup::Both)
    {
        plan.blockers.push(Blocker::CheckoutCleanupConflict);
    }
}

/// Reobserve the full local binding and compare caller-supplied fresh evidence.
/// This is observational, not a lock; repeated old evidence cannot detect movement
/// at a remote. Errors and changed observations revoke this particular plan.
pub fn revalidate(
    directory: impl AsRef<Path>,
    plan: &Plan,
    evidence: Option<&RemoteEvidence>,
) -> Result<(), FinishError> {
    if assess(directory, &plan.request, evidence)? != *plan {
        return Err(FinishError::ContextChanged);
    }
    Ok(())
}
