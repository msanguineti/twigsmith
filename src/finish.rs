//! Finish assessment, preparation, and publication. Tracking refs are hints, never
//! fresh destination observations. Assessment corroborates durable retry data.
use crate::{
    git, repository,
    repository::InspectionError,
    save::{self, SaveError, TreeState, publication::PublishError},
    settings::{FinishRewrite, FinishUpdate, SettingSource, Settings},
};
use std::{ffi::OsString, fmt, fs, io, path::Path};

pub mod checks;
pub mod hosted;
pub mod integration;
pub mod preparation;
pub mod publication;
mod recovery;
mod transport;
pub use recovery::{Recovery, VerifiedRewrite};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// Explicit request choices take precedence over configuration.
    pub update: Option<FinishUpdate>,
    pub rewrite: Option<FinishRewrite>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceSource {
    Request,
    Setting(SettingSource),
    /// Fresh history proves that the branch already includes the base.
    IncludesBase,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredInput {
    Update,
    Rewrite,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RefObservation {
    Absent,
    /// An exact full object ID, not a revision expression. Objects may still
    /// be unavailable locally; ancestry assessment never lazily fetches them.
    Commit(Vec<u8>),
    Unavailable,
    Ambiguous,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DestinationEvidence {
    /// Exact resolved push URL from the plan, not the fetch URL or upstream.
    pub destination: Vec<u8>,
    pub branch: RefObservation,
}
/// Operation-scoped fresh observations supplied by the execution layer.
/// The caller must observe the exact base/branch refs at the named destinations.
/// This module validates the binding and local history, not network freshness.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteEvidence {
    pub base_remote: String,
    /// Resolved fetch endpoint of the named base remote.
    pub base_destination: Vec<u8>,
    pub publish_remote: String,
    pub base_ref: Vec<u8>,
    pub branch_ref: Vec<u8>,
    pub base: RefObservation,
    /// Exactly one observation per resolved push URL, in configuration order.
    pub destinations: Vec<DestinationEvidence>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relationship {
    Absent,
    Equal,
    /// The working branch includes the observed remote commit.
    Ahead,
    Behind,
    Diverged,
    Unrelated,
    /// Available commits, but incomplete history prevents a negative proof.
    Uncertain,
    Unavailable,
    Ambiguous,
}
impl Relationship {
    pub fn includes_remote(self) -> bool {
        matches!(self, Self::Equal | Self::Ahead)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingHints {
    /// Conventional local tracking refs only; custom fetch mappings may differ.
    pub base: RefObservation,
    pub branch: RefObservation,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    FreshRemoteEvidenceRequired,
    BaseUnavailable,
    BaseHistoryUncertain,
    UnrelatedBase,
    DestinationUnavailable(Vec<u8>),
    DestinationAmbiguous(Vec<u8>),
    DestinationHistoryUncertain(Vec<u8>),
    /// Rewrite permission does not remove this constraint.
    RemoteWorkNotIncorporated(Vec<u8>),
    RewriteDenied(Vec<u8>),
    /// Legacy grafts can invent parentage. Resolve them before approval.
    GraftedHistory,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationMode {
    Create,
    FastForwardOrUnchanged,
    /// Rebase may change published commits. The final graph must be checked
    /// again; allow is permission, never proof of incorporation or push success.
    RecheckAfterRebase,
    /// A verified finish rewrite incorporates the exact observed remote work.
    /// Publication still requires an explicit lease and resolved authorization.
    VerifiedRewrite,
    Blocked,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationPlan {
    pub destination: Vec<u8>,
    pub observed: RefObservation,
    pub relationship: Relationship,
    pub mode: PublicationMode,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub repository: repository::RepositoryContext,
    pub settings: Settings,
    pub branch: Vec<u8>,
    pub commit: Vec<u8>,
    pub base_destination: Vec<u8>,
    /// Exact current branch name on the configured remote; no prefix semantics.
    pub publication: save::Publication,
    pub tracking_hints: TrackingHints,
    pub evidence: Option<RemoteEvidence>,
    pub base_relationship: Option<Relationship>,
    pub update: FinishUpdate,
    pub update_source: ChoiceSource,
    pub rewrite: FinishRewrite,
    pub rewrite_source: ChoiceSource,
    pub required_input: Vec<RequiredInput>,
    pub constraints: Vec<Constraint>,
    pub destinations: Vec<DestinationPlan>,
    pub recovery: Recovery,
    request: Request,
    /// Reuse save's existing eligibility, upstream and preservation snapshot.
    local: save::Plan,
    git_config: Vec<u8>,
    shallow: Option<Vec<u8>>,
    grafts: Option<Vec<u8>>,
    committed_settings: Vec<u8>,
}
impl Plan {
    /// Ready for preparation and subsequent publication assessment.
    /// This is neither an execution result nor permission to force-push.
    pub fn is_ready(&self) -> bool {
        self.required_input.is_empty() && self.constraints.is_empty()
    }
}
#[derive(Debug)]
pub enum FinishError {
    Local(SaveError),
    Publication(PublishError),
    DirtyTree,
    InvalidEvidence(&'static str),
    ContextChanged,
}
impl fmt::Display for FinishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Local(error) => match error {
                SaveError::OutsideRepository => "finish requires a Git repository",
                SaveError::BareRepository => "finish requires a working tree",
                SaveError::DetachedHead => "finish requires an attached branch",
                SaveError::UnbornBranch => "finish requires a branch with an existing commit",
                SaveError::ProtectedBase => {
                    "finish requires a branch different from the configured base"
                }
                SaveError::Conflicts => "resolve conflicts before finishing",
                SaveError::ActiveOperation => {
                    "continue or abort the active Git operation, then reassess finish"
                }
                SaveError::DirtySubmodules => {
                    "commit or preserve submodule changes before finishing"
                }
                _ => return write!(f, "{error}"),
            },
            Self::Publication(error) => return write!(f, "{error}"),
            Self::DirtyTree => {
                "finish requires a clean index, working tree, and submodules; commit or preserve changes first"
            }
            Self::InvalidEvidence(reason) => {
                return write!(
                    f,
                    "remote evidence does not match finish context: {reason}; observe the intended refs and destinations again"
                );
            }
            Self::ContextChanged => {
                "finish context or remote evidence changed; assess and resolve choices again"
            }
        };
        f.write_str(message)
    }
}
impl std::error::Error for FinishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Local(e) => Some(e),
            Self::Publication(e) => Some(e),
            _ => None,
        }
    }
}
impl From<SaveError> for FinishError {
    fn from(e: SaveError) -> Self {
        Self::Local(e)
    }
}
impl From<InspectionError> for FinishError {
    fn from(e: InspectionError) -> Self {
        Self::Local(e.into())
    }
}
impl From<io::Error> for FinishError {
    fn from(e: io::Error) -> Self {
        Self::Local(e.into())
    }
}
fn run(directory: &Path, args: &[OsString]) -> Result<std::process::Output, FinishError> {
    Ok(git::inspect(directory, args)?)
}
fn read(directory: &Path, args: &[&str]) -> Result<Vec<u8>, FinishError> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let output = run(directory, &args)?;
    if !output.status.success() {
        return Err(InspectionError::Git {
            arguments: args,
            output,
        }
        .into());
    }
    Ok(output.stdout)
}
fn metadata(directory: &Path, name: &str) -> Result<Option<Vec<u8>>, FinishError> {
    let bytes = read(
        directory,
        &["rev-parse", "--path-format=absolute", "--git-path", name],
    )?;
    let path = repository::os(
        bytes
            .strip_suffix(b"\n")
            .ok_or(InspectionError::InvalidOutput("finish metadata path"))?,
    )?;
    match fs::read(Path::new(&path)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn valid_oid(oid: &[u8], length: usize) -> bool {
    oid.len() == length && oid.iter().all(u8::is_ascii_hexdigit)
}
fn hint(directory: &Path, reference: &[u8]) -> Result<RefObservation, FinishError> {
    let args = vec![
        "rev-parse".into(),
        "--verify".into(),
        "--quiet".into(),
        "--end-of-options".into(),
        repository::os(reference)?,
    ];
    let output = run(directory, &args)?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(RefObservation::Absent);
    }
    if !output.status.success() {
        return Ok(RefObservation::Unavailable);
    }
    let oid = output
        .stdout
        .strip_suffix(b"\n")
        .ok_or(InspectionError::InvalidOutput("finish tracking object ID"))?;
    Ok(RefObservation::Commit(oid.to_vec()))
}
fn ancestor(directory: &Path, older: &[u8], newer: &[u8]) -> Result<Option<bool>, FinishError> {
    let args = vec![
        "--no-replace-objects".into(),
        "merge-base".into(),
        "--is-ancestor".into(),
        repository::os(older)?,
        repository::os(newer)?,
    ];
    let output = run(directory, &args)?;
    Ok(match output.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        // Missing parents/promisor objects and other traversal failures do not
        // establish ancestry. Preserve an actionable unavailable state.
        _ => None,
    })
}
fn relationship(
    directory: &Path,
    local: &[u8],
    remote: &RefObservation,
    shallow: bool,
) -> Result<Relationship, FinishError> {
    let oid = match remote {
        RefObservation::Absent => return Ok(Relationship::Absent),
        RefObservation::Unavailable => return Ok(Relationship::Unavailable),
        RefObservation::Ambiguous => return Ok(Relationship::Ambiguous),
        RefObservation::Commit(oid) => oid,
    };
    if !valid_oid(oid, local.len()) {
        return Err(FinishError::InvalidEvidence(
            "expected a full commit object ID",
        ));
    }
    // Confirm the object type, not just a rev-parse spelling. Lazy fetch is off.
    let args = vec![
        "--no-replace-objects".into(),
        "cat-file".into(),
        "-t".into(),
        repository::os(oid)?,
    ];
    let output = run(directory, &args)?;
    if !output.status.success() || output.stdout != b"commit\n" {
        return Ok(Relationship::Unavailable);
    }
    if oid == local {
        return Ok(Relationship::Equal);
    }
    let remote_in_local = ancestor(directory, oid, local)?;
    if remote_in_local == Some(true) {
        return Ok(Relationship::Ahead);
    }
    let local_in_remote = ancestor(directory, local, oid)?;
    if remote_in_local.is_none() || local_in_remote.is_none() {
        return Ok(Relationship::Unavailable);
    }
    // A successful positive path is useful in shallow history; negative
    // incorporation is deliberately conservative whenever shallow boundaries exist.
    if shallow {
        return Ok(Relationship::Uncertain);
    }
    if local_in_remote == Some(true) {
        return Ok(Relationship::Behind);
    }
    let args = vec![
        "--no-replace-objects".into(),
        "merge-base".into(),
        repository::os(local)?,
        repository::os(oid)?,
    ];
    let output = run(directory, &args)?;
    Ok(match output.status.code() {
        Some(0) => Relationship::Diverged,
        Some(1) => Relationship::Unrelated,
        _ => Relationship::Unavailable,
    })
}

/// Local eligibility plus optional fresh evidence, without Git mutations,
/// network commands, CLI interaction, or recovery metadata writes.
pub fn assess(
    directory: impl AsRef<Path>,
    request: &Request,
    evidence: Option<&RemoteEvidence>,
) -> Result<Plan, FinishError> {
    let directory = directory.as_ref();
    let local = save::assess(
        directory,
        &save::Request {
            action: Some(crate::settings::SaveAction::PublishOnly),
            message: None,
        },
    )?;
    if local.scope.tree_state() != TreeState::Clean {
        return Err(FinishError::DirtyTree);
    }
    let urls = save::publication::preflight(directory, &local).map_err(FinishError::Publication)?;
    let settings = local.settings.clone();
    // Resolve a configured name offline, never a path fallback. Git fetch uses
    // the first URL; retain the resolved endpoint separately from push URLs.
    let base_url = read(directory, &["remote", "get-url", &settings.base_remote])?;
    let base_destination = base_url
        .strip_suffix(b"\n")
        .filter(|url| !url.is_empty() && !url.contains(&b'\n'))
        .ok_or(FinishError::InvalidEvidence("invalid base fetch endpoint"))?
        .to_vec();
    let publication = local.publication.clone().expect("publish-only plan");
    let preparation_source = settings
        .workstream
        .as_ref()
        .map(|policy| policy.preparation_source.as_str())
        .unwrap_or(&settings.base);
    let base_ref = format!("refs/heads/{preparation_source}").into_bytes();
    let tracking_base = format!("refs/remotes/{}/{preparation_source}", settings.base_remote);
    let tracking_branch = [
        b"refs/remotes/".as_slice(),
        settings.publish_remote.as_bytes(),
        b"/",
        &local.branch,
    ]
    .concat();
    let tracking_hints = TrackingHints {
        base: hint(directory, tracking_base.as_bytes())?,
        branch: hint(directory, &tracking_branch)?,
    };
    // Effective Git configuration can alter updates, mapping and transports.
    // Snapshot all of it conservatively, including includes and URL rewrites.
    let git_config = read(directory, &["config", "--null", "--list"])?;
    let shallow = metadata(directory, "shallow")?;
    let grafts = metadata(directory, "info/grafts")?;
    let mut config_args = vec!["ls-tree", "--full-tree", "-z", "HEAD", "--"];
    config_args.extend(crate::settings::CONFIG_FILES);
    let committed_settings = read(directory, &config_args)?;
    let mut plan = Plan {
        repository: local.repository.clone(),
        settings: settings.clone(),
        branch: local.branch.clone(),
        commit: local.commit.clone(),
        base_destination,
        publication,
        tracking_hints,
        evidence: evidence.cloned(),
        base_relationship: None,
        update: request.update.unwrap_or(settings.finish_update),
        update_source: request.update.map_or_else(
            || ChoiceSource::Setting(settings.sources.finish_update.clone()),
            |_| ChoiceSource::Request,
        ),
        rewrite: request.rewrite.unwrap_or(settings.finish_rewrite),
        rewrite_source: request.rewrite.map_or_else(
            || ChoiceSource::Setting(settings.sources.finish_rewrite.clone()),
            |_| ChoiceSource::Request,
        ),
        required_input: vec![],
        constraints: vec![],
        destinations: vec![],
        recovery: Recovery::Absent,
        request: request.clone(),
        local,
        git_config,
        shallow,
        grafts,
        committed_settings,
    };
    if plan.grafts.as_ref().is_some_and(|bytes| !bytes.is_empty()) {
        plan.constraints.push(Constraint::GraftedHistory);
    }
    let Some(evidence) = evidence else {
        plan.constraints
            .push(Constraint::FreshRemoteEvidenceRequired);
        return Ok(plan);
    };
    if evidence.base_remote != settings.base_remote
        || evidence.base_destination != plan.base_destination
        || evidence.publish_remote != settings.publish_remote
        || evidence.base_ref != base_ref
        || evidence.branch_ref != plan.publication.branch_ref
    {
        return Err(FinishError::InvalidEvidence(
            "remote or exact ref selection changed",
        ));
    }
    if urls.len() != evidence.destinations.len()
        || urls
            .iter()
            .zip(&evidence.destinations)
            .any(|(url, observed)| url != &observed.destination)
    {
        return Err(FinishError::InvalidEvidence(
            "observe every exact push destination in configuration order",
        ));
    }
    let shallow = plan.shallow.as_ref().is_some_and(|bytes| !bytes.is_empty());
    let base = relationship(directory, &plan.commit, &evidence.base, shallow)?;
    plan.base_relationship = Some(base);
    if base.includes_remote() {
        plan.update = FinishUpdate::None;
        plan.update_source = ChoiceSource::IncludesBase;
    } else {
        match base {
            Relationship::Absent | Relationship::Unavailable | Relationship::Ambiguous => {
                plan.constraints.push(Constraint::BaseUnavailable)
            }
            Relationship::Uncertain => plan.constraints.push(Constraint::BaseHistoryUncertain),
            Relationship::Unrelated
                if matches!(plan.update, FinishUpdate::Rebase | FinishUpdate::Merge) =>
            {
                plan.constraints.push(Constraint::UnrelatedBase)
            }
            _ => (),
        }
        if plan.update == FinishUpdate::Ask
            && matches!(
                base,
                Relationship::Behind | Relationship::Diverged | Relationship::Unrelated
            )
        {
            plan.required_input.push(RequiredInput::Update);
        }
    }
    for observed in &evidence.destinations {
        // Repeated push URLs remain supported, as in save. Conflicting
        // observations of the same destination cannot describe one safe plan.
        let relationship = if evidence.destinations.iter().any(|other| {
            other.destination == observed.destination && other.branch != observed.branch
        }) {
            Relationship::Ambiguous
        } else {
            relationship(directory, &plan.commit, &observed.branch, shallow)?
        };
        let destination = observed.destination.clone();
        let mut mode = PublicationMode::Blocked;
        match relationship {
            Relationship::Absent => mode = PublicationMode::Create,
            Relationship::Equal | Relationship::Ahead => {
                mode = PublicationMode::FastForwardOrUnchanged;
                if plan.update == FinishUpdate::Rebase {
                    // Published commits already in the new base remain included.
                    // Otherwise replacement may be necessary after rebase.
                    let in_base = match (&observed.branch, &evidence.base) {
                        (RefObservation::Commit(remote), RefObservation::Commit(base))
                            if valid_oid(base, plan.commit.len()) =>
                        {
                            ancestor(directory, remote, base)?
                        }
                        _ => None,
                    };
                    if in_base != Some(true) {
                        mode = PublicationMode::RecheckAfterRebase;
                        match plan.rewrite {
                            FinishRewrite::Ask => {
                                if !plan.required_input.contains(&RequiredInput::Rewrite) {
                                    plan.required_input.push(RequiredInput::Rewrite);
                                }
                            }
                            FinishRewrite::Deny => plan
                                .constraints
                                .push(Constraint::RewriteDenied(destination.clone())),
                            FinishRewrite::Allow => (),
                        }
                    }
                }
            }
            Relationship::Behind | Relationship::Diverged | Relationship::Unrelated => plan
                .constraints
                .push(Constraint::RemoteWorkNotIncorporated(destination.clone())),
            Relationship::Uncertain => plan
                .constraints
                .push(Constraint::DestinationHistoryUncertain(destination.clone())),
            Relationship::Unavailable => plan
                .constraints
                .push(Constraint::DestinationUnavailable(destination.clone())),
            Relationship::Ambiguous => plan
                .constraints
                .push(Constraint::DestinationAmbiguous(destination.clone())),
        }
        plan.destinations.push(DestinationPlan {
            destination,
            observed: observed.branch.clone(),
            relationship,
            mode,
        });
    }
    recovery::apply(directory, &mut plan)?;
    Ok(plan)
}

/// Reobserve locally and compare both context and caller-supplied remote evidence.
/// No remote is contacted. The caller must reacquire evidence when freshness is
/// required; reusing an observation cannot detect a remote-only change.
pub fn revalidate(
    directory: impl AsRef<Path>,
    approved: &Plan,
    evidence: Option<&RemoteEvidence>,
) -> Result<(), FinishError> {
    if assess(directory, &approved.request, evidence)? != *approved {
        return Err(FinishError::ContextChanged);
    }
    Ok(())
}

// Pushes and background fetches may advance caches. They cannot renew the
// literal observations in the plan, so only non-authoritative hints may differ.
fn revalidate_prepared(directory: &Path, plan: &Plan) -> Result<(), FinishError> {
    let mut fresh = assess(directory, &plan.request, plan.evidence.as_ref())?;
    fresh.tracking_hints = plan.tracking_hints.clone();
    if fresh != *plan {
        return Err(FinishError::ContextChanged);
    }
    Ok(())
}
