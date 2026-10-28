//! Exact publication of a prepared branch, one independently guarded transport
//! at a time. Durable rewrite proof is retained; retries read destinations again.
use super::*;
use crate::publication::{Status, responses};
use preparation::CommandReport;
use save::publication as shared;
pub use shared::{PublicationState, TrackingReport, TrackingState, TrackingWrite};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    FastForward,
    Rewrite,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationState {
    NotAttempted,
    Created,
    Advanced,
    Rewritten,
    Unchanged,
    Rejected,
    Unconfirmed,
}
impl DestinationState {
    pub fn is_published(self) -> bool {
        matches!(
            self,
            Self::Created | Self::Advanced | Self::Rewritten | Self::Unchanged
        )
    }
}
#[derive(Debug)]
pub struct DestinationOutcome {
    /// Configured, resolved URL; each command selects only this destination.
    pub destination: Vec<u8>,
    /// The original observation, never renewed after rejection.
    pub expected: RefObservation,
    pub observed: Option<RefObservation>,
    pub action: Option<Action>,
    pub state: DestinationState,
    /// Transport display URL may normalize spelling or redact credentials.
    pub response_destination: Option<Vec<u8>>,
    pub detail: Vec<u8>,
    /// Includes URL checks, advertisement, and (when needed) the push result.
    pub commands: Vec<CommandReport>,
    pub error: Option<PublicationError>,
}
#[derive(Debug)]
pub struct Report {
    pub checks: checks::Report,
    pub remote: String,
    pub branch_ref: Vec<u8>,
    pub commit: Vec<u8>,
    pub state: PublicationState,
    /// One outcome per configured URL, in configuration order, including repeats.
    pub destinations: Vec<DestinationOutcome>,
    pub tracking: TrackingReport,
    pub caches: Vec<shared::CacheReport>,
    /// Assessed rewrite evidence; retained on disk even after complete success.
    /// This proves incorporation, never publication or reusable authorization.
    pub recovery: Recovery,
    pub remaining: Result<repository::Observation, InspectionError>,
    pub error: Option<PublicationError>,
}
impl Report {
    pub fn is_complete(&self) -> bool {
        self.state == PublicationState::Complete
            && self.tracking.state == TrackingState::Matching
            && self.error.is_none()
            && self.destinations.iter().all(|d| d.error.is_none())
    }
}
#[derive(Debug)]
pub enum PublicationError {
    Finish(FinishError),
    UnpreparedPlan,
    ChecksFailed,
    DestinationChanged,
    RemoteChanged,
    CommandFailed,
    UnconfirmedResponse,
    DestinationsIncomplete,
    Tracking(shared::PublishError),
}
impl fmt::Display for PublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Finish(e) => write!(f, "{e}"),
            Self::ChecksFailed => f.write_str("finish checks stopped publication; resolve the reported check failure and rerun finish"),
            Self::UnpreparedPlan => f.write_str("finish publication requires a prepared plan with resolved choices and satisfied constraints"),
            Self::DestinationChanged => f.write_str("the destination resolves to a different endpoint; reassess finish"),
            Self::RemoteChanged => f.write_str("the destination changed since observation; publication stopped without renewing its lease"),
            Self::CommandFailed => f.write_str("Git publication command failed or was interrupted; retain confirmed effects and inspect uncertain destinations"),
            Self::UnconfirmedResponse => f.write_str("Git did not confirm the exact branch update; inspect the destination before retrying"),
            Self::DestinationsIncomplete => f.write_str("publication is incomplete; retain completed effects and reassess actual destinations before retrying"),
            Self::Tracking(e) => write!(f, "publication confirmed, but tracking setup failed: {e}"),
        }
    }
}
impl std::error::Error for PublicationError {}
impl From<FinishError> for PublicationError {
    fn from(e: FinishError) -> Self {
        Self::Finish(e)
    }
}
impl From<InspectionError> for PublicationError {
    fn from(e: InspectionError) -> Self {
        Self::Finish(e.into())
    }
}

fn guard(directory: &Path, plan: &Plan) -> Result<(), PublicationError> {
    revalidate_prepared(directory, plan)?;
    Ok(())
}

fn command(
    directory: &Path,
    args: Vec<OsString>,
    outcome: &mut DestinationOutcome,
) -> Result<Vec<u8>, PublicationError> {
    let result = git::workflow(directory, &args);
    let success = result.as_ref().is_ok_and(|o| o.status.success());
    let bytes = result
        .as_ref()
        .map(|o| o.stdout.clone())
        .unwrap_or_default();
    outcome.commands.push(CommandReport {
        arguments: args,
        result,
    });
    if success {
        Ok(bytes)
    } else {
        Err(PublicationError::CommandFailed)
    }
}
fn publish_destination(
    directory: &Path,
    plan: &Plan,
    outcome: &mut DestinationOutcome,
) -> Result<(), PublicationError> {
    let (prefix, name) = super::transport::endpoint(
        &plan.git_config,
        &plan.settings.publish_remote,
        &outcome.destination,
    )?;
    // Once transport inspection begins, failures have an uncertain destination
    // outcome, not a fabricated server rejection.
    outcome.state = DestinationState::Unconfirmed;
    let mut args = prefix.clone();
    args.extend(
        ["ls-remote", "--get-url", "--", &name]
            .iter()
            .map(OsString::from),
    );
    let bytes = command(directory, args, outcome)?;
    // Both url and explicit pushurl undergo the same insteadOf expansion;
    // explicit pushurl disables pushInsteadOf. One read checks both endpoints.
    if bytes.strip_suffix(b"\n") != Some(outcome.destination.as_slice()) {
        return Err(PublicationError::DestinationChanged);
    }
    let mut args = prefix.clone();
    args.extend([
        "ls-remote".into(),
        "--refs".into(),
        "--heads".into(),
        "--".into(),
        name.clone().into(),
        repository::os(&plan.publication.branch_ref)?,
    ]);
    let bytes = command(directory, args, outcome)?;
    let observed =
        preparation::parse_advertisement(&bytes, &plan.publication.branch_ref, plan.commit.len())?;
    outcome.observed = Some(observed.clone());
    if observed == RefObservation::Commit(plan.commit.clone()) {
        outcome.state = DestinationState::Unchanged;
        return Ok(());
    }
    if observed != outcome.expected {
        outcome.state = DestinationState::Rejected;
        return Err(PublicationError::RemoteChanged);
    }
    let expected = match &outcome.expected {
        RefObservation::Absent => b"".as_slice(),
        RefObservation::Commit(oid) => oid.as_slice(),
        _ => return Err(PublicationError::UnpreparedPlan),
    };
    let lease = [
        b"--force-with-lease=".as_slice(),
        &plan.publication.branch_ref,
        b":",
        expected,
    ]
    .concat();
    let refspec = [plan.commit.as_slice(), b":", &plan.publication.branch_ref].concat();
    let mut args = prefix;
    args.extend([
        "--no-replace-objects".into(),
        "push".into(),
        "--porcelain".into(),
        "--no-force".into(),
        "--no-follow-tags".into(),
        shared::submodule_policy(directory)
            .map_err(FinishError::from)?
            .into(),
        repository::os(&lease)?,
        "--".into(),
        name.into(),
        repository::os(&refspec)?,
    ]);
    guard(directory, plan)?;
    let result = command(directory, args, outcome);
    // Even a failed process can contain an explicit acknowledgement. Conversely,
    // a successful exit without the exact per-ref response confirms nothing.
    let push = outcome.commands.last().expect("push command");
    if let Ok(output) = &push.result {
        let (mut parsed, valid) =
            responses(&output.stdout, &plan.commit, &plan.publication.branch_ref);
        if valid && parsed.len() == 1 {
            let response = parsed.remove(0);
            outcome.response_destination = Some(response.destination);
            outcome.detail = response.detail;
            outcome.state = match response.state {
                Status::Created => DestinationState::Created,
                Status::Advanced => DestinationState::Advanced,
                Status::Rewritten if outcome.action == Some(Action::Rewrite) => {
                    DestinationState::Rewritten
                }
                Status::Unchanged => DestinationState::Unchanged,
                Status::Rejected => DestinationState::Rejected,
                _ => DestinationState::Unconfirmed,
            };
        }
    }
    result?;
    if !outcome.state.is_published() {
        return Err(PublicationError::UnconfirmedResponse);
    }
    Ok(())
}
fn publication_state(destinations: &[DestinationOutcome]) -> PublicationState {
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

/// Publish the pinned tip of a ready plan whose preparation is complete
/// (`update == None`). Each destination is reread; only equality with the pinned
/// commit can bypass the original observation. No rejected lease is refreshed.
/// Each call reruns configured checks before any publication attempt.
/// Callers may retry this plan if local context is intact, or refresh/reassess
/// after interruption. Existing durable rebase evidence supports either route.
pub fn publish(directory: impl AsRef<Path>, plan: &Plan) -> Report {
    publish_with_checks(directory, plan, false, |_| {})
}

/// Run checks (or an explicit bypass) anew, then publish with the original guards.
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
    let before = shared::observe(root, &plan.local);
    let mut report = Report {
        checks: checks::Report::default(),
        remote: plan.settings.publish_remote.clone(),
        branch_ref: plan.publication.branch_ref.clone(),
        commit: plan.commit.clone(),
        state: PublicationState::NotAttempted,
        destinations: plan
            .destinations
            .iter()
            .map(|d| DestinationOutcome {
                destination: d.destination.clone(),
                expected: d.observed.clone(),
                observed: None,
                action: match d.mode {
                    PublicationMode::Create => Some(Action::Create),
                    PublicationMode::FastForwardOrUnchanged => Some(Action::FastForward),
                    PublicationMode::VerifiedRewrite if plan.rewrite == FinishRewrite::Allow => {
                        Some(Action::Rewrite)
                    }
                    _ => None,
                },
                state: DestinationState::NotAttempted,
                response_destination: None,
                detail: vec![],
                commands: vec![],
                error: None,
            })
            .collect(),
        tracking: TrackingReport {
            state: shared::tracking_state(&before, &plan.local),
            before,
            after: Err(SaveError::ContextChanged),
            writes: vec![],
        },
        caches: vec![],
        recovery: plan.recovery.clone(),
        remaining: repository::inspect(root),
        error: None,
    };
    report.error = (|| {
        guard(directory, plan)?;
        if !plan.is_ready()
            || plan.update != FinishUpdate::None
            || report.destinations.is_empty()
            || report.destinations.iter().any(|d| d.action.is_none())
        {
            return Err(PublicationError::UnpreparedPlan);
        }
        report.checks = checks::run(directory, plan, skip, event);
        if report.checks.error.is_some() {
            return Err(PublicationError::ChecksFailed);
        }
        let cache = crate::tracking::cache::Prepared::new(
            root,
            &plan.settings.publish_remote,
            &plan.publication.branch_ref,
        );
        for (index, destination) in report.destinations.iter_mut().enumerate() {
            // Local context changes stop subsequent destinations. A transport or
            // remote-only failure stops that destination; independent ones retain
            // their original safeguards and may still complete.
            if index > 0 {
                guard(root, plan)?;
            }
            destination.error = publish_destination(root, plan, destination).err();
        }
        let confirmed = report
            .destinations
            .iter()
            .filter(|d| d.state.is_published())
            .map(|d| {
                (
                    d.destination.clone(),
                    match &d.expected {
                        RefObservation::Commit(oid) => Some(oid.clone()),
                        _ => None,
                    },
                )
            })
            .collect::<Vec<_>>();
        if !confirmed.is_empty() {
            report
                .caches
                .push(cache.refresh(root, &confirmed, Some(&plan.commit)));
        }
        guard(root, plan)?;
        if publication_state(&report.destinations) != PublicationState::Complete {
            return Err(PublicationError::DestinationsIncomplete);
        }
        if plan.publication.establish_upstream {
            shared::establish_tracking(root, &plan.local, &mut report.tracking.writes)
                .map_err(PublicationError::Tracking)?;
        }
        if report.destinations.iter().any(|d| d.error.is_some()) {
            return Err(PublicationError::CommandFailed);
        }
        Ok(())
    })()
    .err();
    report.state = publication_state(&report.destinations);
    report.tracking.after = shared::observe(root, &plan.local);
    report.tracking.state = shared::tracking_state(&report.tracking.after, &plan.local);
    if report.state == PublicationState::Complete
        && report.tracking.state != TrackingState::Matching
        && report.error.is_none()
    {
        report.error = Some(PublicationError::Tracking(shared::PublishError::Save(
            SaveError::ContextChanged,
        )));
    }
    report.remaining = repository::inspect(root);
    report
}
