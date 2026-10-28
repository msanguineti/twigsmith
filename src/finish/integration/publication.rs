//! Exact staged delivery and cache maintenance for a retained integration result.
//! Upstream configuration and final completion remain separate.
use super::*;
use crate::{
    finish::{checks, preparation::CommandReport},
    publication::{Status, responses},
    save::publication as shared,
};
pub use shared::PublicationState;
use std::{ffi::OsString, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationState {
    NotAttempted,
    Advanced,
    Unchanged,
    Rejected,
    Unconfirmed,
}
impl DestinationState {
    pub fn is_published(self) -> bool {
        matches!(self, Self::Advanced | Self::Unchanged)
    }
}
#[derive(Debug)]
pub struct DestinationOutcome {
    pub destination: Vec<u8>,
    pub expected: Vec<u8>,
    pub observed: Option<RefObservation>,
    pub state: DestinationState,
    pub response_destination: Option<Vec<u8>>,
    pub detail: Vec<u8>,
    pub commands: Vec<CommandReport>,
    pub error: Option<Error>,
}
#[derive(Debug)]
pub enum Error {
    Context(FinishError),
    Transport(remote::Error),
    UnverifiedResult,
    ChecksFailed,
    RemoteChanged,
    UnconfirmedResponse,
    DestinationsIncomplete,
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
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(e) => write!(f, "{e}"), Self::Transport(e) => write!(f, "{e}"),
            Self::UnverifiedResult => f.write_str("publication requires a retained verified result containing the original target"),
            Self::ChecksFailed => f.write_str("candidate checks stopped target publication; retain the result and resolve the reported failure"),
            Self::RemoteChanged => f.write_str("the target changed; publication stopped without renewing its lease"),
            Self::UnconfirmedResponse => f.write_str("Git did not confirm the exact target update; refresh before retrying"),
            Self::DestinationsIncomplete => f.write_str("target delivery is incomplete; retain confirmed effects and refresh before retrying"),
        }
    }
}
impl std::error::Error for Error {}
#[derive(Debug)]
pub struct Report {
    pub checks: checks::Report,
    pub remote: String,
    pub reference: Vec<u8>,
    pub commit: Option<Vec<u8>>,
    pub state: PublicationState,
    pub destinations: Vec<DestinationOutcome>,
    pub caches: Vec<shared::CacheReport>,
    /// Recovery with this attempt's acknowledged destination outcomes. An
    /// uncertain response remains unavailable until an explicit refresh.
    pub plan: Result<Plan, FinishError>,
    pub remaining: Result<repository::Observation, repository::InspectionError>,
    pub error: Option<Error>,
}
impl Report {
    pub fn is_complete(&self) -> bool {
        self.state == PublicationState::Complete
            && self.error.is_none()
            && self.destinations.iter().all(|d| d.error.is_none())
    }
}
fn guard(root: &Path, plan: &Plan) -> Result<(), Error> {
    revalidate_stage(root, plan, plan.evidence.as_ref())?;
    let record = plan.record.as_ref().ok_or(Error::UnverifiedResult)?;
    let result = plan.result.as_ref().ok_or(Error::UnverifiedResult)?;
    if !matches!(plan.state, State::PendingPublication | State::Published | State::Complete)
        || (plan.state == State::PendingPublication && !plan.is_ready())
        || hint(root, record.result_ref().as_bytes())? != RefObservation::Commit(result.clone())
        // Explicit leases permit forced updates (git-push). They protect an OID,
        // not shared history: positive ancestry is an independent requirement.
        || ancestor(root, &record.target_before, result)? != Some(true)
    {
        return Err(Error::UnverifiedResult);
    }
    Ok(())
}
fn observe(
    root: &Path,
    plan: &Plan,
    outcome: &mut DestinationOutcome,
) -> Result<(Vec<OsString>, String), Error> {
    outcome.state = DestinationState::Unconfirmed;
    let (prefix, name) = remote::endpoint(
        root,
        plan,
        &plan.target.remote,
        &outcome.destination,
        &mut outcome.commands,
    )?;
    let result = plan.result.as_ref().ok_or(Error::UnverifiedResult)?;
    let value = remote::observe(
        root,
        &prefix,
        &name,
        &plan.target.reference,
        result.len(),
        &mut outcome.commands,
    )?;
    outcome.observed = Some(value.clone());
    if value == RefObservation::Commit(result.clone()) {
        outcome.state = DestinationState::Unchanged;
    } else if value == RefObservation::Commit(outcome.expected.clone()) {
        outcome.state = DestinationState::NotAttempted;
    } else {
        outcome.state = DestinationState::Rejected;
        return Err(Error::RemoteChanged);
    }
    Ok((prefix, name))
}
fn push(
    root: &Path,
    plan: &Plan,
    outcome: &mut DestinationOutcome,
    endpoint: (Vec<OsString>, String),
) -> Result<(), Error> {
    let (mut args, name) = endpoint;
    let result = plan.result.as_ref().ok_or(Error::UnverifiedResult)?;
    let lease = [
        b"--force-with-lease=".as_slice(),
        &plan.target.reference,
        b":",
        &outcome.expected,
    ]
    .concat();
    let refspec = [result.as_slice(), b":", &plan.target.reference].concat();
    args.extend([
        "--no-replace-objects".into(),
        "push".into(),
        "--porcelain".into(),
        "--no-force".into(),
        "--no-follow-tags".into(),
        shared::submodule_policy(root)
            .map_err(FinishError::from)?
            .into(),
        repository::os(&lease)?,
        "--".into(),
        name.into(),
        repository::os(&refspec)?,
    ]);
    // The exact expected-old lease enforces the selected remote observation.
    // Ancestry was established at stage entry: the lease grants no rewrite
    // authority and must not silently renew after checks or a previous push.
    outcome.state = DestinationState::Unconfirmed;
    let status = remote::command(root, args, &mut outcome.commands);
    // Transport status and exact acknowledgement are independent observations.
    if let Ok(output) = &outcome.commands.last().expect("push command").result {
        let (mut parsed, valid) = responses(&output.stdout, result, &plan.target.reference);
        if valid && parsed.len() == 1 {
            let response = parsed.remove(0);
            outcome.response_destination = Some(response.destination);
            outcome.detail = response.detail;
            outcome.state = match response.state {
                Status::Advanced => DestinationState::Advanced,
                Status::Unchanged => DestinationState::Unchanged,
                Status::Rejected => DestinationState::Rejected,
                _ => DestinationState::Unconfirmed,
            };
        }
    }
    status?;
    if !outcome.state.is_published() {
        return Err(Error::UnconfirmedResponse);
    }
    Ok(())
}
fn state(destinations: &[DestinationOutcome]) -> PublicationState {
    let published = destinations
        .iter()
        .filter(|d| d.state.is_published())
        .count();
    if !destinations.is_empty() && published == destinations.len() {
        PublicationState::Complete
    } else if published > 0 {
        PublicationState::Partial
    } else if destinations
        .iter()
        .all(|d| d.state == DestinationState::NotAttempted)
    {
        PublicationState::NotAttempted
    } else if destinations
        .iter()
        .all(|d| d.state == DestinationState::Rejected)
    {
        PublicationState::Rejected
    } else {
        PublicationState::Unconfirmed
    }
}
/// Publish only the retained result of the selected verified attempt. Fresh
/// equality confirms delivery without checks or a duplicate push. Every attempt
/// that can push runs guarded candidate checks; tagged attempts may reuse their
/// corroborated receipt for the same immutable result and configuration.
pub fn publish(directory: impl AsRef<Path>, plan: &Plan) -> Report {
    publish_with_checks(directory, plan, false, |_| {})
}
pub fn publish_with_checks(
    directory: impl AsRef<Path>,
    plan: &Plan,
    skip: bool,
    event: impl FnMut(checks::Event<'_>),
) -> Report {
    let directory = directory.as_ref();
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    let mut working = plan.clone();
    let plan = &mut working;
    let mut report = Report {
        checks: checks::Report::default(),
        remote: plan.target.remote.clone(),
        reference: plan.target.reference.clone(),
        commit: plan.result.clone(),
        state: PublicationState::NotAttempted,
        destinations: plan
            .target
            .push_destinations
            .iter()
            .map(|d| DestinationOutcome {
                destination: d.clone(),
                expected: plan
                    .record
                    .as_ref()
                    .map(|r| r.target_before.clone())
                    .unwrap_or_default(),
                observed: None,
                state: DestinationState::NotAttempted,
                response_destination: None,
                detail: vec![],
                commands: vec![],
                error: None,
            })
            .collect(),
        caches: vec![],
        plan: Ok(plan.clone()),
        remaining: Ok(repository::Observation::Repository(Box::new(
            plan.snapshot.state.clone(),
        ))),
        error: None,
    };
    report.error = (|| {
        // One entry validation binds a supplied library plan to the selected
        // worktree, policy, retained attempt and immutable candidate. Git's
        // conditional push owns races in the destination ref after this point.
        validate_repository(directory, plan)?;
        guard(directory, plan)?;
        if report.destinations.is_empty() {
            return Err(Error::UnverifiedResult);
        }
        let cache = crate::tracking::cache::Prepared::new(
            root,
            &plan.target.remote,
            &plan.target.reference,
        );
        sequence::verify_completed_delivery(root, plan, &mut report.destinations[0].commands)?;
        release::pending_routes(root, plan, &mut report.destinations[0].commands)?;
        // Observe first: a retry delivered elsewhere needs no candidate execution.
        let mut endpoints = Vec::with_capacity(report.destinations.len());
        for d in &mut report.destinations {
            match observe(root, plan, d) {
                Ok(endpoint) => endpoints.push(Some(endpoint)),
                Err(error) => {
                    d.error = Some(error);
                    endpoints.push(None);
                }
            }
        }
        if (release::enabled(plan) && !release::candidate_checked(plan, skip))
            || report
                .destinations
                .iter()
                .any(|d| d.state == DestinationState::NotAttempted && d.error.is_none())
        {
            if release::enabled(plan) && !release::source_checked(plan, skip) {
                return Err(Error::UnverifiedResult);
            }
            // Even an empty check policy binds the actual checked-out artifact;
            // a ready retry may still be sitting on the source branch.
            let head = match &plan.snapshot.state.head {
                repository::Head::Attached { commit, .. }
                | repository::Head::Detached { commit } => Some(commit),
                _ => None,
            };
            if head != plan.result.as_ref() {
                report.checks.error = Some(checks::Error::Unprepared);
                return Err(Error::ChecksFailed);
            }
            if !plan.settings.finish_checks.is_empty() || release::enabled(plan) || skip {
                let checked = execution::check(root, plan, skip, event);
                report.checks = checked.checks;
                if report.checks.error.is_none() {
                    *plan = checked.plan.map_err(Error::from)?;
                }
                if report.checks.error.is_some() {
                    return Err(Error::ChecksFailed);
                }
            }
            let mut hook_boundary = false;
            for (d, endpoint) in report.destinations.iter_mut().zip(endpoints) {
                if d.state == DestinationState::NotAttempted && d.error.is_none() {
                    if hook_boundary {
                        // A prior push invokes trusted hooks. Before delivering
                        // another copy, keep the checked artifact and approved
                        // routes bound; a remote lease cannot enforce those facts.
                        guard(root, plan)?;
                    }
                    d.error = push(root, plan, d, endpoint.expect("observed endpoint")).err();
                    hook_boundary = true;
                }
            }
        }
        let confirmed = report
            .destinations
            .iter()
            .filter(|d| d.state.is_published())
            .map(|d| (d.destination.clone(), Some(d.expected.clone())))
            .collect::<Vec<_>>();
        if !confirmed.is_empty() {
            report.caches.push(cache.refresh_acknowledged(
                root,
                &confirmed,
                plan.result.as_deref(),
            ));
        }
        if state(&report.destinations) != PublicationState::Complete
            || report.destinations.iter().any(|d| d.error.is_some())
        {
            return Err(Error::DestinationsIncomplete);
        }
        Ok(())
    })()
    .err();
    report.state = state(&report.destinations);
    let mut evidence = plan.evidence.clone();
    if let Some(e) = &mut evidence {
        e.targets = report
            .destinations
            .iter()
            .map(|d| DestinationEvidence {
                destination: d.destination.clone(),
                branch: if d.state.is_published() {
                    RefObservation::Commit(plan.result.clone().expect("verified result"))
                } else if d.state == DestinationState::Unconfirmed {
                    RefObservation::Unavailable
                } else {
                    d.observed.clone().unwrap_or(RefObservation::Unavailable)
                },
            })
            .collect();
    }
    // The acknowledgements are observations for the next stage, not a reason
    // to recontact successful destinations. Derive completion effects from the
    // current local context and these exact per-ref results once.
    report.plan = refresh_local(root, plan, false)
        .and_then(|next| refresh_evidence(root, &next, evidence.as_ref()));
    report.remaining = match &report.plan {
        Ok(next) => Ok(repository::Observation::Repository(Box::new(
            next.snapshot.state.clone(),
        ))),
        Err(_) => repository::inspect(root),
    };
    report
}
