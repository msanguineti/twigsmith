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
    pub(super) fn metadata(&self, name: &str) -> Result<Option<Vec<u8>>, FinishError> {
        Ok(self.snapshot.paths.read(name)?)
    }
    pub(super) fn log(&self, reference: &[u8]) -> Result<Option<Vec<u8>>, FinishError> {
        let name = std::str::from_utf8(reference)
            .map_err(|_| FinishError::InvalidEvidence("non-UTF-8 recovery reference"))?;
        self.metadata(&format!("logs/{name}"))
    }
    pub(super) fn resolve_logs(
        &mut self,
        root: &Path,
        references: &[&[u8]],
    ) -> Result<(), FinishError> {
        let names: Vec<_> = references
            .iter()
            .map(|reference| {
                std::str::from_utf8(reference)
                    .map(|name| format!("logs/{name}"))
                    .map_err(|_| FinishError::InvalidEvidence("non-UTF-8 recovery reference"))
            })
            .collect::<Result<_, _>>()?;
        self.snapshot
            .paths
            .extend(root, &names.iter().map(String::as_str).collect::<Vec<_>>())?;
        Ok(())
    }
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
    paths: repository::MetadataPaths,
    record_path: Option<PathBuf>,
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
/// Observe only refs that bind this attempt, in one Git ref-store read. Full
/// names are filtered exactly because for-each-ref patterns also match children.
fn stage_refs(
    root: &Path,
    plan: &Plan,
) -> Result<std::collections::BTreeMap<Vec<u8>, RefObservation>, FinishError> {
    let mut references = vec![plan.source_ref.clone(), plan.target.reference.clone()];
    if let Some(record) = &plan.record {
        references.extend([
            record.original_ref().into_bytes(),
            record.result_ref().into_bytes(),
        ]);
    }
    references.extend(release::binding_refs(plan));
    references.retain(|reference| !reference.is_empty());
    references.sort();
    references.dedup();
    let mut values: std::collections::BTreeMap<_, _> = references
        .iter()
        .map(|reference| (reference.clone(), RefObservation::Absent))
        .collect();
    if references.is_empty() {
        return Ok(values);
    }
    let mut args = vec![
        "for-each-ref".into(),
        "--format=%(refname)%00%(objectname)%00%(symref)".into(),
        "--".into(),
    ];
    args.extend(
        references
            .iter()
            .map(|reference| repository::os(reference))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let output = run(root, &args)?;
    if !output.status.success() {
        return Err(FinishError::ContextChanged);
    }
    for row in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
    {
        let fields: Vec<_> = row.split(|byte| *byte == 0).collect();
        if fields.len() != 3 {
            return Err(FinishError::InvalidEvidence("selected ref observation"));
        }
        if let Some(value) = values.get_mut(fields[0]) {
            *value = if fields[2].is_empty() {
                RefObservation::Commit(fields[1].to_vec())
            } else {
                RefObservation::Unavailable
            };
        }
    }
    Ok(values)
}
fn stage_ref(
    values: &std::collections::BTreeMap<Vec<u8>, RefObservation>,
    reference: &[u8],
) -> RefObservation {
    values
        .get(reference)
        .cloned()
        .unwrap_or(RefObservation::Unavailable)
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
    let mut paths = repository::MetadataPaths::operations(directory)?;
    paths.extend(
        directory,
        &["index", "shallow", "info/grafts", "logs/HEAD", "SQUASH_MSG"],
    )?;
    let repository::Observation::Repository(state) =
        repository::inspect_context_with_paths(directory, repository.clone(), &paths)?
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
            let reconstruct_absent = !record.has_shared_settings()
                && (pending_tree
                    || configuration_files
                        .shared_matches_tree(&repository, "HEAD")
                        .map_err(SaveError::from)?);
            settings::load_for_branch_from_shared_tree(
                directory,
                &repository,
                branch,
                record.original(),
                reconstruct_absent,
            )
            .map_err(SaveError::from)?
            .map(|loaded| loaded.settings)
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
    release::validate_request(directory, &request.release)?;
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
    let log_name = |reference: &[u8]| -> Result<String, FinishError> {
        let reference = std::str::from_utf8(reference)
            .map_err(|_| FinishError::InvalidEvidence("non-UTF-8 recovery reference"))?;
        Ok(format!("logs/{reference}"))
    };
    let source_log_name = log_name(&source_ref)?;
    let target_log_name = log_name(&target.reference)?;
    let mut log_names = vec![source_log_name.clone(), target_log_name.clone()];
    if let Some(record) = &selection.record {
        log_names.extend([
            format!("logs/{}", record.original_ref()),
            format!("logs/{}", record.result_ref()),
        ]);
    }
    if let Some(tag) = &request.release.tag {
        log_names.push(format!("logs/refs/tags/{tag}"));
    }
    if let Some(marker) = settings
        .workstream
        .as_ref()
        .and_then(|policy| policy.release_marker.as_ref())
    {
        log_names.push(format!("logs/refs/heads/{marker}"));
    }
    paths.extend(
        directory,
        &log_names.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    let snapshot = Snapshot {
        record_path: selection.path.clone(),
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
        config: binding_config(directory)?,
        index: paths.read("index")?,
        shallow: paths.read("shallow")?,
        grafts: paths.read("info/grafts")?,
        head_log: paths.read("logs/HEAD")?,
        source_log: if source_ref.is_empty() {
            None
        } else {
            paths.read(&source_log_name)?
        },
        target_log: paths.read(&target_log_name)?,
        worktrees: read(directory, &["worktree", "list", "--porcelain", "-z"])?,
        pins: if let Some(record) = &selection.record {
            [record.original_ref(), record.result_ref()]
                .iter()
                .map(|r| {
                    Ok((
                        hint(directory, r.as_bytes())?,
                        paths.read(&format!("logs/{r}"))?,
                    ))
                })
                .collect::<Result<_, FinishError>>()?
        } else {
            vec![]
        },
        release: release::snapshot(directory, &request.release, selection.record.as_ref())?,
        records: selection.bytes,
        paths,
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
    sequence::resolve_logs(directory, &mut plan)?;
    assess_observed(directory, plan, pinned_shared, selection.problem, true)
}

fn assess_observed(
    directory: &Path,
    mut plan: Plan,
    pinned_shared: bool,
    problem: Option<recovery::Problem>,
    validate_inputs: bool,
) -> Result<Plan, FinishError> {
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
    if let Some(problem) = problem {
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
    if validate_inputs {
        release::assess(directory, &mut plan)?;
    } else {
        release::reassess(directory, &mut plan)?;
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

/// Configuration that can affect Git's operations or selected transport. Arbitrary
/// application keys do not invalidate an attempt; Git still reads its own settings.
/// Store the same projection in recovery records so unrelated configuration edits
/// remain unrelated after a process restart.
fn binding_config(directory: &Path) -> Result<Vec<u8>, FinishError> {
    let config = read(directory, &["config", "--null", "--list"])?;
    Ok(config
        .split(|b| *b == 0)
        .filter(|entry| {
            let key = entry.split(|b| *b == b'\n').next().unwrap_or_default();
            [
                "remote.",
                "url.",
                "branch.",
                "core.",
                "extensions.",
                "user.",
                "gpg.",
                "commit.",
                "merge.",
                "rebase.",
                "push.",
                "fetch.",
                "tag.",
                "protocol.",
                "credential.",
                "http.",
                "https.",
                "ssh.",
                "transfer.",
                "submodule.",
                "include.",
                "includeif.",
            ]
            .iter()
            .any(|prefix| key.starts_with(prefix.as_bytes()))
        })
        .flat_map(|entry| entry.iter().copied().chain(std::iter::once(0)))
        .collect())
}

/// Refresh changing local stage facts while retaining the selected settings and
/// routes. Git/hook/check boundaries need candidate, cleanliness and ownership
/// observations, not repository rediscovery or settings reconstruction.
pub(super) fn refresh_local(
    root: &Path,
    previous: &Plan,
    staged_squash: bool,
) -> Result<Plan, FinishError> {
    refresh_context(root, previous, staged_squash, previous.evidence.as_ref())
}

/// Newly acquired evidence can resolve a new stage obligation, such as source
/// publication proof after choosing both cleanup scopes. Keep the prior evidence
/// separate so its derived blockers and effects cannot survive that transition.
pub(super) fn refresh_acquired(
    root: &Path,
    previous: &Plan,
    evidence: &RemoteEvidence,
) -> Result<Plan, FinishError> {
    let mut next = previous.clone();
    next.evidence = Some(evidence.clone());
    next.update = next.request.update.unwrap_or(next.settings.finish_update);
    next.state = State::Fresh;
    next.result = None;
    next.integration = Integration::Unresolved;
    next.blockers.clear();
    next.required_input.clear();
    next.effects.clear();
    // This stage only imported pinned objects. Resolve the newly available graph
    // against the selected local facts, without re-observing an unchanged tree.
    assess_observed(root, next, false, None, false)
}

fn refresh_context(
    root: &Path,
    previous: &Plan,
    staged_squash: bool,
    evidence: Option<&RemoteEvidence>,
) -> Result<Plan, FinishError> {
    let configuration = if staged_squash {
        previous
            .snapshot
            .configuration_files
            .matches_index(&previous.repository)
            .map(|matches| matches.then(|| previous.snapshot.configuration_files.clone()))
    } else {
        previous
            .snapshot
            .configuration_files
            .advance_checkout(root, &previous.repository, "HEAD")
    }
    .map_err(SaveError::from)?;
    let Some(configuration) = configuration else {
        return Err(FinishError::ContextChanged);
    };
    if binding_config(root)? != previous.snapshot.config {
        return Err(FinishError::ContextChanged);
    }
    let repository::Observation::Repository(state) = repository::inspect_context_with_paths(
        root,
        previous.repository.clone(),
        &previous.snapshot.paths,
    )?
    else {
        return Err(FinishError::ContextChanged);
    };
    recovery::verify_records(previous)?;
    let mut next = previous.clone();
    next.snapshot.configuration_files = configuration;
    next.evidence = evidence.cloned();
    let references = stage_refs(root, &next)?;
    next.source = stage_ref(&references, &next.source_ref);
    next.local_target = stage_ref(&references, &next.target.reference);
    next.snapshot.state = *state;
    next.snapshot.index = previous.metadata("index")?;
    next.snapshot.shallow = previous.metadata("shallow")?;
    next.snapshot.grafts = previous.metadata("info/grafts")?;
    next.snapshot.head_log = previous.metadata("logs/HEAD")?;
    next.snapshot.source_log = next.log(&next.source_ref)?;
    next.snapshot.target_log = next.log(&next.target.reference)?;
    next.snapshot.worktrees = read(root, &["worktree", "list", "--porcelain", "-z"])?;
    next.snapshot.pins = if let Some(record) = &next.record {
        [record.original_ref(), record.result_ref()]
            .iter()
            .map(|reference| {
                Ok((
                    stage_ref(&references, reference.as_bytes()),
                    next.log(reference.as_bytes())?,
                ))
            })
            .collect::<Result<_, FinishError>>()?
    } else {
        vec![]
    };
    next.snapshot.release = release::snapshot_with_refs(&next, &references)?;
    if !matches!(previous.integration, Integration::Unresolved | Integration::AfterPreparation)
        // Preparation resolves the next integration choices even when native
        // editing preserves every commit. Its completed HEAD-reflog interval,
        // rather than an OID change, establishes that the editor ran once.
        // Aligning an already-incorporated target completes a real stage even
        // though it creates no commit. Its Fresh result must become delivery
        // effects before candidate checks or completion can use it.
        && !(previous.integration == Integration::AlreadyIntegrated && previous.state == State::Fresh)
        && !previous
            .blockers
            .contains(&Blocker::FreshRemoteEvidenceRequired)
        && next.evidence == previous.evidence
        && next.source == previous.source
        && next.local_target == previous.local_target
        && next.record == previous.record
        && next.snapshot.source_log == previous.snapshot.source_log
        && next.snapshot.target_log == previous.snapshot.target_log
        && next.snapshot.pins == previous.snapshot.pins
    {
        // A checkout, tag or metadata acknowledgement did not change the
        // selected graph. Keep its established integration/recovery outcome;
        // only cleanliness, occupancy and release identity can differ here.
        next.blockers.retain(|blocker| {
            !matches!(
                blocker,
                Blocker::DirtyTree
                    | Blocker::GitOperation
                    | Blocker::TargetInAnotherWorktree
                    | Blocker::SourceInAnotherWorktree
            )
        });
        if next
            .snapshot
            .state
            .working_tree
            .as_ref()
            .is_some_and(|tree| tree.is_dirty())
        {
            next.blockers.push(Blocker::DirtyTree);
        }
        if next.snapshot.state.operations.is_active() {
            next.blockers.push(Blocker::GitOperation);
        }
        if in_another_worktree(&next, &next.target.reference) {
            next.blockers.push(Blocker::TargetInAnotherWorktree);
        }
        if next.integration == Integration::AfterPreparation
            && in_another_worktree(&next, &next.source_ref)
        {
            next.blockers.push(Blocker::SourceInAnotherWorktree);
        }
        // Checkouts invoke hooks. Neither an unchanged branch ref nor a
        // successful switch proves HEAD still names that selected branch.
        let head = current_ref(&next.snapshot.state);
        if head.as_ref() != Some(&next.source_ref)
            && head.as_ref() != Some(&next.target.reference)
            && head != current_ref(&previous.snapshot.state)
        {
            return Err(FinishError::ContextChanged);
        }
        release::reassess(root, &mut next)?;
        return Ok(next);
    }
    next.update = next.request.update.unwrap_or(next.settings.finish_update);
    next.state = State::Fresh;
    next.result = None;
    next.integration = Integration::Unresolved;
    next.blockers.clear();
    next.required_input.clear();
    next.effects.clear();
    assess_observed(root, next, false, None, false)
}

/// Apply known per-ref delivery outcomes to an established local stage. The
/// caller validates entry and hook boundaries; an acknowledgement does not
/// require rediscovering settings or reproving the retained candidate graph.
pub(super) fn refresh_evidence(
    root: &Path,
    previous: &Plan,
    evidence: Option<&RemoteEvidence>,
) -> Result<Plan, FinishError> {
    let mut next = previous.clone();
    next.evidence = evidence.cloned();
    next.effects.clear();
    let result = next
        .result
        .clone()
        .ok_or(FinishError::InvalidEvidence("retained result missing"))?;
    let record = next
        .record
        .clone()
        .ok_or(FinishError::InvalidEvidence("integration record missing"))?;
    recovery::assess_delivery(root, &mut next, &record, result)?;
    release::reassess(root, &mut next)?;
    Ok(next)
}

/// Validate a caller's stage binding. Git owns command execution; this guard
/// protects candidate identity, selected routing and private-attempt ownership
/// across prompts/checks/hooks, which a later merge or push cannot establish.
/// No graph assessment or remote observation is repeated here.
pub fn revalidate(
    directory: impl AsRef<Path>,
    plan: &Plan,
    evidence: Option<&RemoteEvidence>,
) -> Result<(), FinishError> {
    let directory = directory.as_ref();
    validate_repository(directory, plan)?;
    revalidate_stage(directory, plan, evidence)
}

pub(super) fn validate_repository(directory: &Path, plan: &Plan) -> Result<(), FinishError> {
    if repository::discover(directory)?.as_ref() != Some(&plan.repository) {
        return Err(FinishError::ContextChanged);
    }
    Ok(())
}

/// Within an executing stage, repository selection is already established.
/// MetadataPaths checks selection-related environment changes; the operations
/// below keep the candidate, routing and attempt ownership bound across hooks.
/// Read-only calls and private record writes do not create another entry point.
pub(super) fn revalidate_stage(
    directory: &Path,
    plan: &Plan,
    evidence: Option<&RemoteEvidence>,
) -> Result<(), FinishError> {
    if evidence != plan.evidence.as_ref() || binding_config(directory)? != plan.snapshot.config {
        return Err(FinishError::ContextChanged);
    }
    let repository::Observation::Repository(state) = repository::inspect_context_with_paths(
        directory,
        plan.repository.clone(),
        &plan.snapshot.paths,
    )?
    else {
        return Err(FinishError::ContextChanged);
    };
    let references = stage_refs(directory, plan)?;
    // Index bytes bind check results even when a check hides modifications with
    // assume-unchanged. Reflogs and records bind retained attempts to their owner.
    if *state != plan.snapshot.state
        || stage_ref(&references, &plan.source_ref) != plan.source
        || stage_ref(&references, &plan.target.reference) != plan.local_target
        || plan.metadata("index")? != plan.snapshot.index
        || plan.metadata("shallow")? != plan.snapshot.shallow
        || plan.metadata("info/grafts")? != plan.snapshot.grafts
        || plan.metadata("logs/HEAD")? != plan.snapshot.head_log
        || plan.log(&plan.source_ref)? != plan.snapshot.source_log
        || plan.log(&plan.target.reference)? != plan.snapshot.target_log
        || read(directory, &["worktree", "list", "--porcelain", "-z"])? != plan.snapshot.worktrees
        || !plan
            .snapshot
            .configuration_files
            .matches_files(&plan.repository)
            .map_err(SaveError::from)?
    {
        return Err(FinishError::ContextChanged);
    }
    recovery::verify_records(plan)?;
    if let Some(record) = &plan.record {
        for (reference, expected) in [record.original_ref(), record.result_ref()]
            .iter()
            .zip(&plan.snapshot.pins)
        {
            if stage_ref(&references, reference.as_bytes()) != expected.0
                || plan.log(reference.as_bytes())? != expected.1
            {
                return Err(FinishError::ContextChanged);
            }
        }
    }
    if release::snapshot_with_refs(plan, &references)? != plan.snapshot.release {
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
    let request = reviewed
        .record
        .as_ref()
        .map_or_else(|| request.clone(), |record| record.request(request));
    if request
        .message
        .as_ref()
        .is_some_and(|message| message.trim().is_empty() || message.contains('\0'))
    {
        return Err(SaveError::BlankMessage.into());
    }
    if request.release.tag != reviewed.request.release.tag {
        release::validate_request(directory, &request.release)?;
    }
    let mut resolved = reviewed.clone();
    resolved.update = request.update.unwrap_or(resolved.settings.finish_update);
    resolved.history_edit = request
        .history_edit
        .unwrap_or(resolved.settings.history_edit);
    resolved.method = request
        .integration
        .unwrap_or(resolved.settings.finish_integration);
    resolved.checkout = request
        .checkout
        .unwrap_or(resolved.settings.finish_checkout);
    resolved.cleanup = request.cleanup.unwrap_or(resolved.settings.finish_cleanup);
    resolved.request = request;
    if let Some(tag) = &resolved.request.release.tag {
        let reference = format!("refs/tags/{tag}").into_bytes();
        resolved.resolve_logs(directory, &[&reference])?;
    }
    resolved.snapshot.release = release::snapshot_from_plan(directory, &resolved)?;
    if resolved.history_edit == HistoryEdit::Interactive
        && reviewed.history_edit != HistoryEdit::Interactive
    {
        resolved.snapshot.shared_preparation =
            super::history::shared_refs(directory, &resolved.settings)?
                .into_iter()
                .filter(|(reference, _, _)| {
                    !destination_refs(&resolved.settings)
                        .iter()
                        .any(|destination| destination == reference.as_bytes())
                })
                .collect();
    }
    resolved.state = State::Fresh;
    resolved.result = None;
    resolved.integration = Integration::Unresolved;
    resolved.required_input.clear();
    resolved.blockers.clear();
    resolved.effects.clear();
    assess_observed(directory, resolved, false, None, true)
}

/// Retain accepted input after verified preparation, including a method selected
/// before message editing is cancelled. This writes private intent only; pending
/// input continues to prohibit target effects. Fresh attempts are recorded by
/// the executor rather than this API.
pub fn retain_choices(directory: impl AsRef<Path>, plan: &Plan) -> Result<Plan, FinishError> {
    let directory = directory.as_ref();
    revalidate(directory, plan, plan.evidence.as_ref())?;
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
    let mut next = plan.clone();
    // The successful private write only acknowledges accepted choices; Git
    // refs, graph and candidate are unchanged.
    *next
        .snapshot
        .records
        .last_mut()
        .ok_or(FinishError::InvalidEvidence("recovery snapshot missing"))? = Some(bytes);
    next.record = Some(record);
    Ok(next)
}
