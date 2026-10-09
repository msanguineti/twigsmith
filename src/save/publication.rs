//! Exact, non-forced publication, independently of checkpoints and the CLI.
//! Push acknowledgements are evidence from the actual push transports, never
//! inferred from fetch URLs or local tracking refs. No rollback is attempted.
use super::{Plan, SaveError, UpstreamSettings};
use crate::{git, repository};
use std::{ffi::OsString, fmt, io, path::Path, process::Output};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationState {
    NotAttempted,
    /// Every configured destination acknowledged publication (or equality).
    Complete,
    /// At least one destination confirmed publication; others did not.
    Partial,
    /// Every destination explicitly rejected the intended ref update.
    Rejected,
    /// No confirmed publication, and at least one missing/uncertain response.
    Unconfirmed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationState {
    Created,
    Advanced,
    Unchanged,
    Rejected,
    Unconfirmed,
}
impl DestinationState {
    pub fn is_published(self) -> bool {
        matches!(self, Self::Created | Self::Advanced | Self::Unchanged)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationOutcome {
    /// Git's transport display URL, which may redact credentials or normalize
    /// spelling. This is not an inferred fetch URL or an index into push URLs.
    pub destination: Vec<u8>,
    pub state: DestinationState,
    pub detail: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingRef {
    pub reference: Vec<u8>,
    pub commit: Vec<u8>,
    pub symbolic_target: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingObservation {
    pub upstream: UpstreamSettings,
    /// Git can update remote-tracking refs even if another destination fails.
    /// These local observations are not evidence of remote publication.
    pub remote_refs: Vec<TrackingRef>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingState {
    Matching,
    Absent,
    Incomplete,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingWrite {
    Remote,
    MergeRef,
}
#[derive(Debug)]
pub struct TrackingReport {
    pub before: Result<TrackingObservation, SaveError>,
    pub after: Result<TrackingObservation, SaveError>,
    pub state: TrackingState,
    /// Successful configuration commands; `after` also captures effects of a
    /// failed command or hook. Neither implies a transactional guarantee.
    pub writes: Vec<TrackingWrite>,
}
#[derive(Debug)]
pub struct Report {
    pub remote: String,
    pub branch_ref: Vec<u8>,
    pub commit: Vec<u8>,
    pub state: PublicationState,
    /// Resolved configured push URLs, in Git's selection order.
    pub configured_destinations: Vec<Vec<u8>>,
    /// Only explicit per-ref transport responses. Missing responses are not
    /// manufactured as rejections or assigned to a guessed URL.
    pub destinations: Vec<DestinationOutcome>,
    pub missing_responses: usize,
    /// Raw completed process or I/O error, including failed/interrupted output.
    pub push: Option<Result<Output, io::Error>>,
    pub tracking: TrackingReport,
    pub error: Option<PublishError>,
}
impl Report {
    pub fn is_complete(&self) -> bool {
        self.state == PublicationState::Complete
            && self.tracking.state == TrackingState::Matching
            && self.error.is_none()
    }
}
#[derive(Debug)]
pub enum PublishError {
    Save(SaveError),
    PendingLocalSteps,
    UnsafeFetchMapping(Vec<u8>),
    LegacyRemote,
    SymbolicTrackingRef(Vec<u8>),
    InvalidDestinations,
    PushFailed,
    UnconfirmedResponse,
    TrackingIo(io::Error),
    TrackingGit {
        arguments: Vec<OsString>,
        output: Output,
    },
}
impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Save(e) => write!(f, "{e}"),
            Self::PendingLocalSteps => write!(
                f,
                "publication requires resolved input and completed local steps; obtain a fresh publish-only plan"
            ),
            Self::UnsafeFetchMapping(r) => write!(
                f,
                "remote fetch mapping {:?} can update local refs outside remote tracking; publication refused",
                String::from_utf8_lossy(r)
            ),
            Self::LegacyRemote => write!(
                f,
                "publication requires a remote defined in Git configuration; legacy remote files cannot be safely inspected"
            ),
            Self::SymbolicTrackingRef(r) => write!(
                f,
                "tracking ref {:?} is symbolic; publication refused to protect local history",
                String::from_utf8_lossy(r)
            ),
            Self::InvalidDestinations => write!(
                f,
                "push destinations are empty or cannot be represented unambiguously"
            ),
            Self::PushFailed => write!(
                f,
                "Git push failed; retain confirmed destination effects and inspect uncertain destinations before retrying"
            ),
            Self::UnconfirmedResponse => write!(
                f,
                "Git did not confirm the intended publication at every destination; inspect transport output and destinations before retrying"
            ),
            Self::TrackingIo(e) => write!(
                f,
                "branch published, but tracking setup encountered an I/O error: {e}"
            ),
            Self::TrackingGit { output, .. } => write!(
                f,
                "branch published, but tracking setup failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }
}
impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Save(e) => Some(e),
            Self::TrackingIo(e) => Some(e),
            _ => None,
        }
    }
}
impl From<SaveError> for PublishError {
    fn from(error: SaveError) -> Self {
        Self::Save(error)
    }
}

pub(crate) fn observe(directory: &Path, plan: &Plan) -> Result<TrackingObservation, SaveError> {
    let upstream = super::upstream_settings(directory, &plan.branch)?;
    let raw = super::read(
        directory,
        &[
            "for-each-ref".into(),
            "--format=%(refname)%00%(objectname)%00%(symref)".into(),
            "refs/remotes/".into(),
        ],
    )?;
    let mut remote_refs = vec![];
    for line in raw.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let fields: Vec<_> = line.split(|b| *b == 0).collect();
        if fields.len() != 3 {
            return Err(
                repository::InspectionError::InvalidOutput("publication tracking refs").into(),
            );
        }
        remote_refs.push(TrackingRef {
            reference: fields[0].to_vec(),
            commit: fields[1].to_vec(),
            symbolic_target: fields[2].to_vec(),
        });
    }
    Ok(TrackingObservation {
        upstream,
        remote_refs,
    })
}
pub(crate) fn tracking_state(
    observation: &Result<TrackingObservation, SaveError>,
    plan: &Plan,
) -> TrackingState {
    let Ok(observation) = observation else {
        return TrackingState::Unavailable;
    };
    let upstream = &observation.upstream;
    if upstream.remotes == [plan.settings.remote.as_bytes()]
        && upstream.merge_refs == [branch_ref(plan).as_slice()]
    {
        TrackingState::Matching
    } else if upstream.remotes.is_empty() && upstream.merge_refs.is_empty() {
        TrackingState::Absent
    } else {
        TrackingState::Incomplete
    }
}
fn branch_ref(plan: &Plan) -> Vec<u8> {
    [b"refs/heads/".as_slice(), &plan.branch].concat()
}

// A normal push also writes fetch-mapped tracking refs. Refuse mappings into
// local history/tags/custom namespaces; do not silently rewrite user config.
// Conservatively validate all positive destinations, even nonmatching ones.
fn protect_tracking(directory: &Path, plan: &Plan) -> Result<(), PublishError> {
    // Legacy $GIT_DIR/remotes and $GIT_DIR/branches files have their own fetch
    // mappings, which config --get-all cannot observe. Do not bypass that guard.
    if super::config(
        directory,
        format!("remote.{}.url", plan.settings.remote).as_bytes(),
    )?
    .is_empty()
        && super::config(
            directory,
            format!("remote.{}.pushurl", plan.settings.remote).as_bytes(),
        )?
        .is_empty()
    {
        return Err(PublishError::LegacyRemote);
    }
    let key = format!("remote.{}.fetch", plan.settings.remote);
    for raw in super::config(directory, key.as_bytes())? {
        let mapping = raw.strip_prefix(b"+").unwrap_or(&raw);
        if mapping.starts_with(b"^") {
            continue;
        }
        let Some((source, target)) = mapping.split_once_byte(b':') else {
            // A source-only fetch refspec has no local tracking destination.
            continue;
        };
        if target.is_empty() {
            continue;
        }
        if !target.starts_with(b"refs/remotes/") {
            return Err(PublishError::UnsafeFetchMapping(raw));
        }
        let reference = branch_ref(plan);
        let mapped = if let Some((prefix, suffix)) = source.split_once_byte(b'*') {
            if !reference.starts_with(prefix)
                || !reference.ends_with(suffix)
                || reference.len() < prefix.len() + suffix.len()
            {
                continue;
            }
            let Some((left, right)) = target.split_once_byte(b'*') else {
                return Err(PublishError::UnsafeFetchMapping(raw));
            };
            [
                left,
                &reference[prefix.len()..reference.len() - suffix.len()],
                right,
            ]
            .concat()
        } else if source == reference {
            target.to_vec()
        } else {
            continue;
        };
        let args = vec![
            "symbolic-ref".into(),
            "-q".into(),
            repository::os(&mapped).map_err(SaveError::from)?,
        ];
        let output = git::inspect(directory, &args).map_err(SaveError::from)?;
        if output.status.success() {
            return Err(PublishError::SymbolicTrackingRef(mapped));
        }
        if output.status.code() != Some(1) {
            return Err(SaveError::from(repository::InspectionError::Git {
                arguments: args,
                output,
            })
            .into());
        }
    }
    Ok(())
}

pub(crate) fn submodule_policy(directory: &Path) -> Result<&'static str, SaveError> {
    // Respect a user's non-mutating publication check. Recursive publication
    // (including submodule.recurse's implicit on-demand) would exceed this intent.
    let values = super::config(directory, b"push.recurseSubmodules")?;
    Ok(if values.last().is_some_and(|value| value == b"check") {
        "--recurse-submodules=check"
    } else {
        "--recurse-submodules=no"
    })
}

// Shared read-only local guards. Checkpoint execution uses these before writes;
// publication repeats them because hooks/configuration can change in between.
pub(crate) fn preflight(directory: &Path, plan: &Plan) -> Result<Vec<Vec<u8>>, PublishError> {
    let urls = plan
        .snapshot
        .push_urls
        .strip_suffix(b"\n")
        .ok_or(PublishError::InvalidDestinations)?;
    let destinations: Vec<_> = urls.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect();
    if destinations
        .iter()
        .any(|url| url.is_empty() || url.contains(&b'\r'))
    {
        return Err(PublishError::InvalidDestinations);
    }
    protect_tracking(directory, plan)?;
    submodule_policy(directory)?;
    Ok(destinations)
}
// Keep this byte-oriented on every platform; URLs/refnames are not decoded.
trait SplitByte {
    fn split_once_byte(&self, byte: u8) -> Option<(&[u8], &[u8])>;
}
impl SplitByte for [u8] {
    fn split_once_byte(&self, byte: u8) -> Option<(&[u8], &[u8])> {
        let index = self.iter().position(|b| *b == byte)?;
        Some((&self[..index], &self[index + 1..]))
    }
}

fn responses(output: &[u8], source: &[u8], target: &[u8]) -> (Vec<DestinationOutcome>, bool) {
    use crate::publication::Status;
    let (responses, valid) = crate::publication::responses(output, source, target);
    (
        responses
            .into_iter()
            .map(|r| DestinationOutcome {
                destination: r.destination,
                state: match r.state {
                    Status::Created => DestinationState::Created,
                    Status::Advanced => DestinationState::Advanced,
                    Status::Unchanged => DestinationState::Unchanged,
                    Status::Rejected => DestinationState::Rejected,
                    // Save never authorizes replacement, including a surprising response.
                    Status::Rewritten | Status::Unconfirmed => DestinationState::Unconfirmed,
                },
                detail: r.detail,
            })
            .collect(),
        valid,
    )
}

fn revalidate_tracking(
    directory: &Path,
    plan: &Plan,
    remote_written: bool,
) -> Result<(), SaveError> {
    if !remote_written {
        return super::revalidate(directory, plan);
    }
    let mut expected = plan.clone();
    expected.snapshot.upstream.remotes = vec![plan.settings.remote.as_bytes().to_vec()];
    expected.publication.as_mut().unwrap().establish_upstream = false;
    let fresh =
        super::assess_with_upstream(directory, &plan.request, Some(&expected.snapshot.upstream))?;
    if fresh != expected {
        return Err(SaveError::ContextChanged);
    }
    Ok(())
}
pub(crate) fn establish_tracking(
    directory: &Path,
    plan: &Plan,
    writes: &mut Vec<TrackingWrite>,
) -> Result<(), PublishError> {
    for (suffix, value, effect) in [
        (
            b".remote".as_slice(),
            plan.settings.remote.as_bytes(),
            TrackingWrite::Remote,
        ),
        (
            b".merge".as_slice(),
            branch_ref(plan).as_slice(),
            TrackingWrite::MergeRef,
        ),
    ] {
        revalidate_tracking(directory, plan, !writes.is_empty())?;
        let arguments = vec![
            "config".into(),
            "--local".into(),
            "--add".into(),
            repository::os(&super::upstream_key(&plan.branch, suffix)).map_err(SaveError::from)?,
            repository::os(value).map_err(SaveError::from)?,
        ];
        let output = git::workflow(directory, &arguments).map_err(PublishError::TrackingIo)?;
        if !output.status.success() {
            return Err(PublishError::TrackingGit { arguments, output });
        }
        writes.push(effect);
    }
    Ok(())
}

/// Publish a fully resolved plan with no pending local steps. Checkpoint
/// composition assesses its resulting branch/commit afresh before calling.
/// Dirty publish-only work stays untouched. Snapshots do not lock concurrent
/// edits or arbitrary hook effects; the literal OID pins what Git publishes.
pub fn publish(directory: impl AsRef<Path>, plan: &Plan) -> Report {
    let directory = directory.as_ref();
    let before = observe(directory, plan);
    let mut report = Report {
        remote: plan.settings.remote.clone(),
        branch_ref: branch_ref(plan),
        commit: plan.commit.clone(),
        state: PublicationState::NotAttempted,
        configured_destinations: vec![],
        destinations: vec![],
        missing_responses: 0,
        push: None,
        tracking: TrackingReport {
            state: tracking_state(&before, plan),
            before,
            // Filled with a fresh observation after execution, including failure.
            after: Err(SaveError::ContextChanged),
            writes: vec![],
        },
        error: None,
    };
    let result = (|| -> Result<(), PublishError> {
        super::revalidate(directory, plan)?;
        if plan.publication.is_none() || !plan.local_steps.is_empty() {
            return Err(PublishError::PendingLocalSteps);
        }
        let root = plan
            .snapshot
            .state
            .working_tree
            .as_ref()
            .unwrap()
            .root
            .as_path();
        super::revalidate(root, plan)?;
        report.configured_destinations = preflight(root, plan)?;
        let submodules = submodule_policy(root)?;
        let refspec = [&plan.commit, b":".as_slice(), &report.branch_ref].concat();
        let arguments = vec![
            "-c".into(),
            format!("remote.{}.mirror=false", plan.settings.remote).into(),
            "-c".into(),
            "push.autoSetupRemote=false".into(),
            "push".into(),
            "--porcelain".into(),
            "--no-force".into(),
            "--no-follow-tags".into(),
            submodules.into(),
            "--".into(),
            plan.settings.remote.clone().into(),
            repository::os(&refspec).map_err(SaveError::from)?,
        ];
        let push = git::workflow(root, &arguments);
        let mut valid = false;
        let mut process_success = false;
        if let Ok(output) = &push {
            (report.destinations, valid) =
                responses(&output.stdout, &plan.commit, &report.branch_ref);
            process_success = output.status.success();
        }
        report.push = Some(push);
        let count = report.configured_destinations.len();
        report.missing_responses = count.saturating_sub(report.destinations.len());
        let published = report
            .destinations
            .iter()
            .filter(|r| r.state.is_published())
            .count();
        let rejected = report
            .destinations
            .iter()
            .filter(|r| r.state == DestinationState::Rejected)
            .count();
        report.state = if valid && published == count && report.destinations.len() == count {
            PublicationState::Complete
        } else if published > 0 {
            PublicationState::Partial
        } else if valid && rejected == count && report.destinations.len() == count {
            PublicationState::Rejected
        } else {
            PublicationState::Unconfirmed
        };
        if report.state != PublicationState::Complete {
            return Err(if process_success {
                PublishError::UnconfirmedResponse
            } else {
                PublishError::PushFailed
            });
        }
        // Git may have written remote-tracking refs while pushing. Assessment
        // intentionally ignores those caches, so fresh guards remain meaningful.
        if plan.publication.as_ref().unwrap().establish_upstream {
            establish_tracking(root, plan, &mut report.tracking.writes)?;
        }
        if !process_success {
            return Err(PublishError::PushFailed);
        }
        Ok(())
    })();
    report.error = result.err();
    let root = plan
        .snapshot
        .state
        .working_tree
        .as_ref()
        .map(|tree| tree.root.as_path())
        .unwrap_or(directory);
    report.tracking.after = observe(root, plan);
    report.tracking.state = tracking_state(&report.tracking.after, plan);
    // A concurrent configuration change or failed observation cannot become
    // complete tracking merely because both config subprocesses returned zero.
    if report.state == PublicationState::Complete
        && report.tracking.state != TrackingState::Matching
        && report.error.is_none()
    {
        report.error = Some(PublishError::Save(SaveError::ContextChanged));
    }
    report
}
