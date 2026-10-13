//! Direct integration with read-only assessment, explicit remote refresh, local
//! preparation, guarded target publication, and verified checkout and cleanup.
//! Remote evidence comes from exact endpoint observations, never tracking caches.
//! Review uses a separate executor.
use super::{DestinationEvidence, FinishError, RefObservation, Relationship};
use super::{ancestor, hint, metadata, read, relationship, run, valid_oid};
use crate::{
    repository,
    save::SaveError,
    settings::{
        self, FinishCheckout, FinishCleanup, FinishIntegration, FinishMessage, FinishRewrite,
        FinishUpdate, HistoryEdit, Settings,
    },
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod sequence;
pub use sequence::advance;
pub mod completion;
pub mod execution;
pub mod publication;
pub mod recovery;
pub mod release;
pub mod remote;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub update: Option<FinishUpdate>,
    pub rewrite: Option<FinishRewrite>,
    pub history_edit: Option<HistoryEdit>,
    pub integration: Option<FinishIntegration>,
    pub message: Option<String>,
    pub checkout: Option<FinishCheckout>,
    pub cleanup: Option<FinishCleanup>,
    pub release: release::Request,
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
pub struct PreparationEvidence {
    pub reference: Vec<u8>,
    pub branch: RefObservation,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEvidence {
    pub target: Target,
    pub fetched: RefObservation,
    pub preparation: Option<PreparationEvidence>,
    pub targets: Vec<DestinationEvidence>,
    pub work_remote: String,
    pub work_ref: Vec<u8>,
    pub work: Vec<DestinationEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredInput {
    Update,
    Rewrite,
    Integration,
    Message,
    Cleanup,
    TagName,
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
    Release(String),
    HistoryEdit(String),
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
    pub history_edit: HistoryEdit,
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
    pub fn active_destination_index(&self) -> usize {
        self.record.as_ref().map_or(0, |r| r.completed.len())
    }
    pub fn has_remaining_destinations(&self) -> bool {
        self.active_destination_index() + 1 < destination_refs(&self.settings).len()
    }
    pub fn completed_destinations(&self) -> Vec<(&[u8], &[u8])> {
        self.record.as_ref().map_or_else(Vec::new, |r| {
            r.completed
                .iter()
                .map(|d| (d.target.reference.as_slice(), d.result.as_slice()))
                .collect()
        })
    }

    pub fn is_ready(&self) -> bool {
        self.blockers.is_empty() && self.required_input.is_empty()
    }
    pub fn permitted_effects(&self) -> &[Effect] {
        if self.is_ready() { &self.effects } else { &[] }
    }
    /// Effective request, including intent recovered from the associated attempt.
    pub fn request(&self) -> &Request {
        &self.request
    }
    pub fn message(&self) -> Option<&str> {
        self.request.message.as_deref()
    }
    pub fn message_suggestion(&self) -> String {
        format!(
            "Integrate {} into {}",
            String::from_utf8_lossy(
                self.source_ref
                    .strip_prefix(b"refs/heads/")
                    .unwrap_or(&self.source_ref)
            ),
            String::from_utf8_lossy(
                self.target
                    .reference
                    .strip_prefix(b"refs/heads/")
                    .unwrap_or(&self.target.reference)
            )
        )
    }
    /// Whether accepted input can be recorded before remaining message input.
    pub fn can_retain_choices(&self) -> bool {
        self.record
            .clone()
            .is_some_and(|mut record| record.retain_choices(self).is_ok())
    }
    /// A corroborated transition can survive a process stopping before its
    /// retention ref or private result acknowledgement was written.
    pub fn needs_result_retention(&self) -> bool {
        self.result.as_ref().is_some_and(|result| {
            self.record.as_ref().is_some_and(|record| {
                record.result.as_ref() != Some(result)
                    || self
                        .snapshot
                        .pins
                        .get(1)
                        .is_none_or(|(pin, _)| *pin != RefObservation::Commit(result.clone()))
            })
        })
    }
    /// Pending delivery checks inspect the exact candidate, even when a retry
    /// starts on the unchanged source. Delivered attempts need only completion.
    pub fn needs_candidate_checkout(&self) -> bool {
        let head = match &self.snapshot.state.head {
            repository::Head::Attached { commit, .. } | repository::Head::Detached { commit } => {
                Some(commit)
            }
            _ => None,
        };
        (self.state == State::PendingPublication
            || (release::enabled(self)
                && !release::candidate_checked(self, false)
                && self.source != RefObservation::Absent))
            && self.result.is_some()
            && self.result.as_ref() != head
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
    configuration_files: settings::checkout::Snapshot,
    pins: Vec<(RefObservation, Option<Vec<u8>>)>,
    release: Option<release::Snapshot>,
    shared_preparation: super::history::SharedRefs,
}
fn destination_refs(settings: &Settings) -> Vec<Vec<u8>> {
    settings
        .workstream
        .as_ref()
        .filter(|p| p.version_tagging)
        .map_or_else(
            || vec![branch_ref(settings.base.as_bytes())],
            |p| {
                p.integration_targets
                    .iter()
                    .map(|name| branch_ref(name.as_bytes()))
                    .collect()
            },
        )
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
    let mut settings = settings::load_with_files(directory, &repository)
        .map_err(SaveError::from)?
        .settings;
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
    let configuration_files = settings::checkout::Snapshot::capture(directory, &repository, "HEAD")
        .map_err(SaveError::from)?;
    let pending_tree = state.operations.is_active()
        || state
            .working_tree
            .as_ref()
            .is_some_and(|tree| tree.is_dirty());
    let selection = recovery::select(
        directory,
        &repository,
        &state,
        &branch_ref(settings.base.as_bytes()),
    )?;
    let mut pinned_shared = false;
    if let Some(source) = &selection.source_ref {
        let branch = source.strip_prefix(b"refs/heads/").unwrap_or(source);
        let retained = if let Some(record) = &selection.record {
            // A regular committed shared layer stays bound to the attempt's
            // original tree. Target checkout and native preparation may import
            // another tree's settings, but cannot approve different effects.
            settings::load_for_branch_from_shared_tree(
                directory,
                &repository,
                branch,
                record.original(),
            )
            .map_err(SaveError::from)?
            .map(|loaded| -> Result<_, FinishError> {
                let reconstruct = loaded.shared.is_some()
                    || (!record.has_shared_settings()
                        && (pending_tree
                            || configuration_files
                                .shared_matches_tree(&repository, "HEAD")
                                .map_err(SaveError::from)?));
                Ok(reconstruct.then_some(loaded.settings))
            })
            .transpose()?
            .flatten()
        } else {
            None
        };
        settings = if let Some(retained) = retained {
            pinned_shared = true;
            retained
        } else {
            settings::load_for_branch(directory, &repository, branch).map_err(SaveError::from)?
        };
    }
    let mut target = Target {
        remote: settings.base_remote.clone(),
        reference: branch_ref(settings.base.as_bytes()),
        fetch_destination: urls(directory, &settings.base_remote, false)?.remove(0),
        push_destinations: urls(directory, &settings.base_remote, true)?,
    };
    if let Some(record) = &selection.record {
        // Only the ordinal chooses the active configured route; record equality
        // below still verifies its endpoint and full settings binding.
        if let Some(reference) = destination_refs(&settings).get(record.completed.len()) {
            target.reference = reference.clone();
        }
    }
    // Stored intent supplies omitted input, never proof. Recovery below must
    // corroborate its bindings and Git transitions before permitting any effect.
    let request = selection
        .record
        .as_ref()
        .map_or_else(|| request.clone(), |record| record.request(request));
    if request
        .message
        .as_ref()
        .is_some_and(|m| m.trim().is_empty() || m.contains('\0'))
    {
        return Err(SaveError::BlankMessage.into());
    }
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
    let snapshot = Snapshot {
        configuration_files,
        shared_preparation: if request.history_edit.unwrap_or(settings.history_edit)
            == HistoryEdit::Interactive
            || (settings
                .workstream
                .as_ref()
                .is_some_and(|p| p.version_tagging)
                && request.update == Some(FinishUpdate::Rebase))
        {
            super::history::shared_refs(directory, &settings)?
                .into_iter()
                .filter(|(r, _, _)| {
                    !destination_refs(&settings)
                        .iter()
                        .any(|d| d == r.as_bytes())
                })
                .collect()
        } else {
            vec![]
        },
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
        release: release::snapshot(directory, &request.release, selection.record.as_ref())?,
        records: selection.bytes,
    };
    let mut plan = Plan {
        repository,
        settings: settings.clone(),
        source_ref,
        source,
        target,
        local_target,
        update: request.update.unwrap_or(settings.finish_update),
        history_edit: request.history_edit.unwrap_or(settings.history_edit),
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
        request,
        snapshot,
        record: selection.record,
    };
    if plan.history_edit == HistoryEdit::Interactive {
        if plan
            .snapshot
            .shared_preparation
            .iter()
            .any(|(reference, _, _)| reference.as_bytes() == plan.source_ref)
        {
            plan.blockers.push(Blocker::HistoryEdit(
                "the work branch must differ from its shared preparation and integration refs"
                    .into(),
            ));
        }
        if release::enabled(&plan) {
            plan.blockers.push(Blocker::HistoryEdit("interactive history editing requires ordinary work; tagged versions retain their source history".into()));
        }
        if !matches!(plan.update, FinishUpdate::Rebase | FinishUpdate::Ask) {
            plan.blockers.push(Blocker::HistoryEdit(
                "interactive history editing requires finish-update = rebase".into(),
            ));
        }
        if let Some(reason) = super::history::unavailable(directory)? {
            plan.blockers.push(Blocker::HistoryEdit(reason));
        }
    }
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
    if pinned_shared
        && !plan.snapshot.state.operations.is_active()
        && plan
            .snapshot
            .state
            .working_tree
            .as_ref()
            .is_some_and(|t| !t.is_dirty())
        && !plan
            .snapshot
            .configuration_files
            .shared_matches_tree(&plan.repository, "HEAD")
            .map_err(SaveError::from)?
    {
        // A fresh process has no prior worktree snapshot. A clean current tree
        // must still explain its shared files; ignored additions cannot borrow
        // the retained settings as authority.
        plan.blockers
            .push(Blocker::Recovery(recovery::Problem::ChangedBinding));
        plan.state = State::ChangedOrUnverifiable;
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
    sequence::assess_completed(directory, &mut plan)?;
    release::assess(directory, &mut plan)?;
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
fn preparation_ref(plan: &Plan) -> Option<Vec<u8>> {
    if plan.active_destination_index() > 0 {
        return None;
    }
    if plan.request.update.unwrap_or(plan.settings.finish_update) == FinishUpdate::None {
        return None;
    }
    plan.settings
        .workstream
        .as_ref()
        .map(|p| branch_ref(p.preparation_source.as_bytes()))
        .filter(|r| r != &plan.target.reference)
}
fn validate_evidence(plan: &mut Plan) -> bool {
    let Some(e) = &plan.evidence else {
        plan.blockers.push(Blocker::FreshRemoteEvidenceRequired);
        return false;
    };
    if e.preparation.as_ref().map(|p| p.reference.clone()) != preparation_ref(plan)
        || e.target != plan.target
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
                if ancestor(directory, remote, source)? == Some(true) {
                    true
                } else if let Some(record) = &plan.record {
                    record.work.contains(d)
                        && record.incorporates_published_work(directory, plan, remote)?
                } else {
                    false
                }
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
    let preparation_base = e.preparation.as_ref().map_or(&e.fetched, |p| &p.branch);
    let prep_relation = relationship(directory, &source, preparation_base, shallow)?;
    if plan.active_destination_index() > 0
        || plan
            .record
            .as_ref()
            .is_some_and(|r| r.preparation_completed(plan))
    {
        plan.update = FinishUpdate::None;
    }
    let editing = plan.history_edit == HistoryEdit::Interactive
        && !plan
            .record
            .as_ref()
            .is_some_and(|r| r.history_prepared(plan));
    if editing
        && let Some(boundary) = commit(preparation_base)
        && !super::history::unique_boundary(directory, &source, boundary)?
    {
        plan.blockers.push(Blocker::HistoryEdit(
            "interactive history editing requires one complete, unambiguous preparation boundary"
                .into(),
        ));
    }
    if editing
        && let Some(boundary) = commit(preparation_base)
        && super::history::shares_target_work(directory, &source, boundary, &base)?
    {
        plan.blockers.push(Blocker::HistoryEdit("the selected work range contains shared integration-target history; select a preparation boundary that includes that history".into()));
    }
    if !editing
        && (matches!(prep_relation, Relationship::Equal | Relationship::Ahead)
            || (e.preparation.is_none() && relation == Relationship::Behind))
    {
        plan.update = FinishUpdate::None;
    } else if plan.update != FinishUpdate::None {
        if !matches!(prep_relation, Relationship::Behind | Relationship::Diverged)
            && !(editing && prep_relation.includes_remote())
        {
            plan.blockers.push(Blocker::HistoryUnavailable);
            return Ok(());
        }
        if plan.update == FinishUpdate::Ask {
            plan.required_input.push(RequiredInput::Update);
        }
        if matches!(plan.update, FinishUpdate::Merge | FinishUpdate::Rebase) {
            let prep = commit(preparation_base)
                .ok_or(FinishError::InvalidEvidence("preparation source missing"))?
                .to_vec();
            if release::enabled(plan) && plan.update == FinishUpdate::Rebase {
                validate_work(directory, plan)?;
                let evidence = plan.evidence.as_ref().expect("validated");
                let published = evidence.work.iter().filter_map(|d| commit(&d.branch));
                for oid in published {
                    if ancestor(directory, oid, &prep)? != Some(true) {
                        match plan.request.rewrite.unwrap_or(plan.settings.finish_rewrite) {
                            FinishRewrite::Allow => (),
                            FinishRewrite::Ask => {
                                if !plan.required_input.contains(&RequiredInput::Rewrite) {
                                    plan.required_input.push(RequiredInput::Rewrite);
                                }
                            }
                            FinishRewrite::Deny => plan.blockers.push(Blocker::Release(
                                "rebasing published work requires --allow-rewrite".into(),
                            )),
                        }
                    }
                }
            }
            plan.integration = Integration::AfterPreparation;
            completion_choices(plan);
            if in_another_worktree(plan, &plan.source_ref) {
                plan.blockers.push(Blocker::SourceInAnotherWorktree);
            }
            plan.effects.push(Effect::UpdateWork {
                reference: plan.source_ref.clone(),
                expected: source,
                base: prep,
                method: plan.update,
            });
            return Ok(());
        }
    }
    match relation {
        Relationship::Equal | Relationship::Behind => {
            plan.integration = Integration::AlreadyIntegrated;
            plan.result = Some(base.clone());
        }
        Relationship::Ahead | Relationship::Diverged => {
            if plan.method == FinishIntegration::Ask {
                plan.required_input.push(RequiredInput::Integration);
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

/// Resolve input against the exact context presented to the caller. Answering a
/// prompt or editing a message cannot approve concurrent local context changes.
/// Supplied remote evidence remains the caller's observation, not a remote lock.
pub fn resolve(
    directory: impl AsRef<Path>,
    reviewed: &Plan,
    request: &Request,
) -> Result<Plan, FinishError> {
    let directory = directory.as_ref();
    revalidate(directory, reviewed, reviewed.evidence.as_ref())?;
    let resolved = assess(directory, request, reviewed.evidence.as_ref())?;
    revalidate(directory, reviewed, reviewed.evidence.as_ref())?;
    Ok(resolved)
}

/// Retain accepted input after verified preparation, including a method selected
/// before message editing is cancelled. This writes private intent only; pending
/// input continues to prohibit target effects. Fresh attempts are recorded by
/// the executor rather than this API.
pub fn retain_choices(directory: impl AsRef<Path>, plan: &Plan) -> Result<Plan, FinishError> {
    let directory = directory.as_ref();
    revalidate(directory, plan, plan.evidence.as_ref())?;
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .expect("worktree");
    revalidate(root, plan, plan.evidence.as_ref())?;
    let mut record = plan.record.clone().ok_or(FinishError::InvalidEvidence(
        "retaining choices requires verified preparation",
    ))?;
    record.retain_choices(plan)?;
    let bytes = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
    recovery::write(
        &plan.repository.git_dir,
        plan.recovery_path
            .as_ref()
            .ok_or(FinishError::InvalidEvidence("recovery path missing"))?,
        &record,
        true,
    )?;
    let next = assess(root, &plan.request, plan.evidence.as_ref())?;
    let mut expected_snapshot = plan.snapshot.clone();
    // Selection reads its association before the record when HEAD has left the
    // source. The branch record is always the last selected snapshot entry.
    *expected_snapshot
        .records
        .last_mut()
        .ok_or(FinishError::InvalidEvidence("recovery snapshot missing"))? = Some(bytes);
    if next.repository != plan.repository
        || next.settings != plan.settings
        || next.source_ref != plan.source_ref
        || next.source != plan.source
        || next.target != plan.target
        || next.local_target != plan.local_target
        || next.snapshot != expected_snapshot
        || next.record.as_ref() != Some(&record)
    {
        return Err(FinishError::ContextChanged);
    }
    Ok(next)
}
