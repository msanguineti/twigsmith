//! Versioned private operation evidence. Reflog corroboration
//! protects against stale/inconsistent records, not malicious local fabrication.
use super::*;
use std::{fs, io};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    MissingRecord,
    MalformedRecord,
    UnsafeRecord,
    UnassociatedHead,
    ChangedBinding,
    ChangedSource,
    ChangedTarget,
    MissingHistory,
    InvalidResult,
}
/// A reflog position, including the entire final raw entry (old/new IDs and
/// action). Expiration and additional updates invalidate version equality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogVersion {
    count: usize,
    last: Vec<u8>,
}
impl LogVersion {
    pub(super) fn of(bytes: Option<&[u8]>) -> Self {
        let lines: Vec<_> = bytes
            .unwrap_or_default()
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        Self {
            count: lines.len(),
            last: lines.last().copied().unwrap_or_default().to_vec(),
        }
    }
    fn prefix(&self, bytes: Option<&[u8]>) -> bool {
        let lines: Vec<_> = bytes
            .unwrap_or_default()
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        self.count <= lines.len()
            && self.last.as_slice()
                == self
                    .count
                    .checked_sub(1)
                    .and_then(|i| lines.get(i).copied())
                    .unwrap_or_default()
    }
}
/// Stored separately per worktree and source branch. No unbound success flags or reusable
/// rewrite permission. Remaining effects are derived from actual observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    version: u32,
    git_dir: PathBuf,
    common_dir: PathBuf,
    root: PathBuf,
    source_ref: Vec<u8>,
    pub(super) source: Vec<u8>,
    source_version: LogVersion,
    pub(super) target: Target,
    pub(super) target_before: Vec<u8>,
    pub(super) local_before: RefObservation,
    pub(super) target_version: LogVersion,
    pub(super) head_version: LogVersion,
    settings: Settings,
    git_config: Vec<u8>,
    method: FinishIntegration,
    requested_update: Option<FinishUpdate>,
    rewrite: Option<FinishRewrite>,
    pub(super) completed: Vec<super::sequence::Completed>,
    history_edit: HistoryEdit,
    shared_preparation: super::super::history::SharedRefs,
    checkout: FinishCheckout,
    cleanup: FinishCleanup,
    message: Option<String>,
    /// Exact originally observed published source versions, for conditional deletion.
    pub(super) work: Vec<DestinationEvidence>,
    token: String,
    preparation: Option<Preparation>,
    pub(super) retained: bool,
    /// Optional acknowledgement. A lost write can be recovered from the exact
    /// operation-marked target reflog transition and the actual commit graph.
    pub result: Option<Vec<u8>>,
    /// Source reflog captured before conditional deletion; preserves preparation
    /// evidence after Git removes the branch reflog. Never replaces a live log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cleanup_source_log: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) release: Option<release::Record>,
}
impl Record {
    pub(super) fn has_shared_settings(&self) -> bool {
        use settings::SettingSource;
        let sources = &self.settings.sources;
        [
            &sources.publish_remote,
            &sources.base,
            &sources.save_plan,
            &sources.save_message,
            &sources.save_stash_message,
            &sources.finish_mode,
            &sources.finish_integration,
            &sources.finish_message,
            &sources.finish_checkout,
            &sources.finish_cleanup,
            &sources.finish_update,
            &sources.history_edit,
            &sources.finish_rewrite,
            &sources.finish_checks,
        ]
        .into_iter()
        .chain(sources.base_remote.iter())
        .chain(sources.workstreams.values())
        .any(|source| matches!(source, SettingSource::Shared(_)))
    }
    /// Called only with a corroborated attempt. Native preparation can replace
    /// published commits without publishing the rewritten source. Cleanup still
    /// binds only the exact initially observed version retained before editing.
    pub(super) fn incorporates_published_work(
        &self,
        root: &Path,
        plan: &Plan,
        published: &[u8],
    ) -> Result<bool, FinishError> {
        if ancestor(root, published, &self.source)? == Some(true) {
            return Ok(true);
        }
        Ok(self.retained
            && self
                .work
                .iter()
                .any(|d| commit(&d.branch) == Some(published))
            && self
                .preparation
                .as_ref()
                .is_some_and(|p| p.method == FinishUpdate::Rebase)
            && self.preparation_completed(plan)
            && (!release::enabled(plan)
                || self.rewrite.unwrap_or(self.settings.finish_rewrite) == FinishRewrite::Allow)
            && ancestor(root, published, self.original())? == Some(true))
    }
    pub(super) fn preparation_completed(&self, plan: &Plan) -> bool {
        self.preparation
            .as_ref()
            .is_some_and(|p| self.source != p.original || self.history_prepared(plan))
    }
    pub(super) fn history_prepared(&self, plan: &Plan) -> bool {
        self.preparation.as_ref().is_some_and(|p| {
            let head = entries(plan.snapshot.head_log.as_deref());
            head.get(p.head_version.count..).is_some_and(|tail| {
                tail.iter().any(|e| {
                    ["rebase".to_owned(), self.prepare_marker()]
                        .iter()
                        .any(|action| {
                            e.message
                                == [
                                    action.as_bytes(),
                                    b" (finish): returning to ",
                                    &self.source_ref,
                                ]
                                .concat()
                        })
                })
            })
        })
    }
    pub(super) fn request(&self, request: &Request) -> Request {
        Request {
            update: request.update.or(Some(self.update())),
            rewrite: request.rewrite.or(self.rewrite),
            history_edit: request.history_edit.or(Some(self.history_edit)),
            integration: request.integration.or(Some(self.method)),
            message: request.message.clone().or_else(|| self.message.clone()),
            checkout: request.checkout.or(Some(self.checkout)),
            cleanup: request.cleanup.or(Some(self.cleanup)),
            release: self
                .release
                .as_ref()
                .map_or_else(|| request.release.clone(), |r| r.request(&request.release)),
        }
    }
    fn update(&self) -> FinishUpdate {
        self.preparation
            .as_ref()
            .map_or(FinishUpdate::None, |p| p.method)
    }
    /// Build evidence in memory. Execution persists it before effects.
    pub fn for_plan(plan: &Plan, token: &str) -> Result<Self, FinishError> {
        if !plan.is_ready()
            || plan.state != State::Fresh
            || plan.record.is_some()
            || token.is_empty()
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(FinishError::InvalidEvidence(
                "record requires a resolved integration plan and unique token",
            ));
        }
        let e = plan.evidence.as_ref().ok_or(FinishError::InvalidEvidence(
            "record requires remote observations",
        ))?;
        // Capture cleanup destinations before integration; a later destination
        // or branch version can never inherit deletion authority.
        Ok(Self {
            version: 3,
            git_dir: plan.repository.git_dir.clone(),
            common_dir: plan.repository.common_dir.clone(),
            root: plan.repository.working_tree_root.clone().expect("worktree"),
            source_ref: plan.source_ref.clone(),
            source: commit(&plan.source)
                .ok_or(FinishError::InvalidEvidence("source missing"))?
                .to_vec(),
            source_version: LogVersion::of(plan.snapshot.source_log.as_deref()),
            target: plan.target.clone(),
            target_before: commit(&e.fetched)
                .ok_or(FinishError::InvalidEvidence("target missing"))?
                .to_vec(),
            local_before: plan.local_target.clone(),
            target_version: LogVersion::of(plan.snapshot.target_log.as_deref()),
            head_version: LogVersion::of(plan.snapshot.head_log.as_deref()),
            settings: plan.settings.clone(),
            git_config: plan.snapshot.config.clone(),
            method: plan.method,
            requested_update: plan.request.update,
            rewrite: plan.request.rewrite,
            completed: vec![],
            history_edit: plan.history_edit,
            shared_preparation: plan.snapshot.shared_preparation.clone(),
            checkout: plan.checkout,
            cleanup: plan.cleanup,
            message: plan.request.message.clone(),
            work: e.work.clone(),
            token: token.into(),
            preparation: (plan.integration == Integration::AfterPreparation).then(|| Preparation {
                method: plan.update,
                base: commit(e.preparation.as_ref().map_or(&e.fetched, |p| &p.branch))
                    .expect("ready preparation source")
                    .to_vec(),
                original: commit(&plan.source).expect("ready source").to_vec(),
                source_version: LogVersion::of(plan.snapshot.source_log.as_deref()),
                head_version: LogVersion::of(plan.snapshot.head_log.as_deref()),
            }),
            result: None,
            cleanup_source_log: None,
            retained: false,
            release: release::for_plan(plan, token)?,
        })
    }
    pub fn marker(&self) -> String {
        format!(
            "twigsmith-integrate:{}:{}:{}:{}",
            self.token,
            String::from_utf8_lossy(&self.source),
            String::from_utf8_lossy(&self.target_before),
            self.method.as_str()
        )
    }
}
/// Association after leaving the source branch. Its HEAD anchor must survive,
/// and the latest checkout must still associate this source with this target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    source_ref: Vec<u8>,
    head: LogVersion,
}
impl Selection {
    pub fn observe(directory: impl AsRef<Path>, source_branch: &str) -> Result<Self, FinishError> {
        Ok(Self {
            source_ref: branch_ref(source_branch.as_bytes()),
            head: LogVersion::of(metadata(directory.as_ref(), "logs/HEAD")?.as_deref()),
        })
    }
}
fn valid_ref(directory: &Path, reference: &[u8]) -> Result<bool, FinishError> {
    if !reference.starts_with(b"refs/heads/") {
        return Ok(false);
    }
    Ok(run(
        directory,
        &["check-ref-format".into(), repository::os(reference)?],
    )?
    .status
    .success())
}
/// The exact branch namespace keeps A and B independent, including in linked
/// worktrees. It is not a single replaceable 'current attempt' journal.
pub fn record_path(repository: &repository::RepositoryContext, source_branch: &str) -> PathBuf {
    repository
        .git_dir
        .join("twigsmith-integration/branches")
        .join(source_branch)
        .join("attempt.json")
}
pub fn selection_path(repository: &repository::RepositoryContext) -> PathBuf {
    repository.git_dir.join("twigsmith-integration/active.json")
}
/// Explicit policy start establishes new workstream intent on a named branch.
/// Retain all attempt records; retire only a different source's active selector.
pub(crate) fn clear_other_selection(
    repository: &repository::RepositoryContext,
    branch: &[u8],
) -> Result<bool, FinishError> {
    let path = selection_path(repository);
    let read = || {
        private_read(&repository.git_dir, &path)
            .map_err(|_| FinishError::InvalidEvidence("unsafe integration association"))
    };
    let Some(bytes) = read()? else {
        return Ok(false);
    };
    let selection: Selection = serde_json::from_slice(&bytes)
        .map_err(|_| FinishError::InvalidEvidence("invalid integration association"))?;
    if selection.source_ref == branch_ref(branch) {
        return Ok(false);
    }
    if read()?.as_ref() != Some(&bytes) {
        return Err(FinishError::ContextChanged);
    }
    fs::remove_file(&path)?;
    fs::File::open(path.parent().expect("selector parent"))?.sync_all()?;
    Ok(true)
}
fn private_read(root: &Path, path: &Path) -> Result<Option<Vec<u8>>, Problem> {
    let relative = path.strip_prefix(root).map_err(|_| Problem::UnsafeRecord)?;
    let mut current = root.to_owned();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(Problem::UnsafeRecord);
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(Problem::UnsafeRecord);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o077 != 0 {
                        return Err(Problem::UnsafeRecord);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Problem::UnsafeRecord),
        }
    }
    fs::read(path).map(Some).map_err(|_| Problem::UnsafeRecord)
}
pub(super) struct Selected {
    pub source_ref: Option<Vec<u8>>,
    pub path: Option<PathBuf>,
    pub record: Option<Record>,
    pub problem: Option<Problem>,
    pub bytes: Vec<Option<Vec<u8>>>,
}
pub(super) fn select(
    directory: &Path,
    repository: &repository::RepositoryContext,
    state: &repository::RepositoryState,
    target: &[u8],
) -> Result<Selected, FinishError> {
    // A legacy selector can associate an interrupted source with the currently
    // checked-out destination. Do not mistake that destination for fresh work.
    let legacy_selector = repository.git_dir.join("oneflow-integration/active.json");
    let legacy = private_read(&repository.git_dir, &legacy_selector)
        .map_err(|_| FinishError::InvalidEvidence("unsafe legacy integration evidence; recover with the prior Rust version and preserve evidence"))?;
    if let Some(bytes) = legacy {
        let selection: Selection = serde_json::from_slice(&bytes)
            .map_err(|_| FinishError::InvalidEvidence("invalid legacy integration evidence; recover with the prior Rust version and preserve evidence"))?;
        let branch = std::str::from_utf8(selection.source_ref.strip_prefix(b"refs/heads/").ok_or(
            FinishError::InvalidEvidence("invalid legacy integration source"),
        )?)
        .map_err(|_| FinishError::InvalidEvidence("invalid legacy integration source"))?;
        let path = repository
            .git_dir
            .join("oneflow-integration/branches")
            .join(branch)
            .join("attempt.json");
        let record = private_read(&repository.git_dir, &path)
            .map_err(|_| FinishError::InvalidEvidence("unsafe legacy integration record; recover with the prior Rust version and preserve evidence"))?
            .and_then(|data| serde_json::from_slice::<Record>(&data).ok());
        let head = current_ref(state);
        let relevant = head.as_ref().is_some_and(|head| {
            *head == selection.source_ref
                || record.as_ref().is_some_and(|record| {
                    *head == record.target.reference
                        || record
                            .completed
                            .iter()
                            .any(|item| *head == item.target.reference)
                })
                || (record.is_none() && head.as_slice() == target)
        });
        if relevant {
            return Err(FinishError::InvalidEvidence(
                "unfinished legacy integration: recover with the prior Rust version and preserve its evidence before establishing Twigsmith associations",
            ));
        }
    }
    let mut selected = Selected {
        source_ref: current_ref(state),
        path: None,
        record: None,
        problem: None,
        bytes: vec![],
    };
    // A branch-bound route can differ from the currently checked-out target's
    // own policy. The selector may supply the target only after its record names
    // this exact HEAD; the existing checkout/reflog proof below still applies.
    let active_target = private_read(&repository.git_dir, &selection_path(repository))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Selection>(&bytes).ok())
        .and_then(|selection| {
            let branch =
                std::str::from_utf8(selection.source_ref.strip_prefix(b"refs/heads/")?).ok()?;
            let bytes =
                private_read(&repository.git_dir, &record_path(repository, branch)).ok()??;
            let record: Record = serde_json::from_slice(&bytes).ok()?;
            current_ref(state).filter(|r| {
                *r == record.target.reference
                    || record.completed.iter().any(|d| d.target.reference == *r)
            })
        });
    let target = active_target.as_deref().unwrap_or(target);
    let on_source = selected.source_ref.as_deref().is_some_and(|r| r != target);
    let mut association = None;
    if !on_source {
        let bytes = match private_read(&repository.git_dir, &selection_path(repository)) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                selected.problem = Some(Problem::UnassociatedHead);
                return Ok(selected);
            }
            Err(p) => {
                selected.problem = Some(p);
                return Ok(selected);
            }
        };
        selected.bytes.push(Some(bytes.clone()));
        let Ok(selection) = serde_json::from_slice::<Selection>(&bytes) else {
            selected.problem = Some(Problem::MalformedRecord);
            return Ok(selected);
        };
        if !selection
            .head
            .prefix(metadata(directory, "logs/HEAD")?.as_deref())
        {
            selected.problem = Some(Problem::UnassociatedHead);
            return Ok(selected);
        }
        selected.source_ref = Some(selection.source_ref.clone());
        association = Some(selection);
    }
    let Some(reference) = &selected.source_ref else {
        selected.problem = Some(Problem::UnassociatedHead);
        return Ok(selected);
    };
    if !valid_ref(directory, reference)? {
        selected.problem = Some(Problem::MalformedRecord);
        return Ok(selected);
    }
    let Ok(branch) = std::str::from_utf8(&reference[b"refs/heads/".len()..]) else {
        selected.problem = Some(Problem::MalformedRecord);
        return Ok(selected);
    };
    let path = record_path(repository, branch);
    selected.path = Some(path.clone());
    let bytes = match private_read(&repository.git_dir, &path) {
        Ok(bytes) => bytes,
        Err(p) => {
            selected.problem = Some(p);
            return Ok(selected);
        }
    };
    selected.bytes.push(bytes.clone());
    let Some(bytes) = bytes else {
        if association.is_some() {
            selected.problem = Some(Problem::MissingRecord);
        } else if let Ok(Some(active)) =
            private_read(&repository.git_dir, &selection_path(repository))
            && let Ok(selection) = serde_json::from_slice::<Selection>(&active)
            && selection.source_ref == *reference
        {
            // Returning to A cannot hide a lost A record. An unrelated selector
            // (including one that cannot be decoded) never intercepts B.
            selected.bytes.push(Some(active));
            selected.problem = Some(Problem::MissingRecord);
        }
        return Ok(selected);
    };
    let Ok(record) = serde_json::from_slice::<Record>(&bytes) else {
        selected.problem = Some(Problem::MalformedRecord);
        return Ok(selected);
    };
    if !valid_ref(directory, &record.target.reference)?
        || record.version != 3
        || record.source_ref != *reference
        || record.source_ref == record.target.reference
    {
        selected.problem = Some(Problem::MalformedRecord);
        return Ok(selected);
    }
    if association.is_some() {
        let head = metadata(directory, "logs/HEAD")?.unwrap_or_default();
        let allowed: Vec<_> = std::iter::once(record.source_ref.as_slice())
            .chain(
                record
                    .completed
                    .iter()
                    .map(|d| d.target.reference.as_slice()),
            )
            .chain(std::iter::once(record.target.reference.as_slice()))
            .collect();
        let checkout_valid = |message: &[u8]| {
            allowed.iter().any(|from| {
                allowed.iter().any(|to| {
                    message
                        == [
                            b"checkout: moving from ",
                            &from[b"refs/heads/".len()..],
                            b" to ",
                            &to[b"refs/heads/".len()..],
                        ]
                        .concat()
                        && current_ref(state).as_deref() == Some(*to)
                })
            })
        };
        let last_checkout = head
            .split(|b| *b == b'\n')
            .skip(record.head_version.count)
            .filter_map(|l| l.split(|b| *b == b'\t').nth(1))
            .filter(|m| m.starts_with(b"checkout: "))
            .last();
        let rebase_source = metadata(directory, "rebase-merge/head-name")?
            .or(metadata(directory, "rebase-apply/head-name")?);
        let rebasing = state.operations.rebase
            && record.preparation.is_some()
            && rebase_source.as_deref()
                == Some([record.source_ref.as_slice(), b"\n"].concat().as_slice());
        if !record.head_version.prefix(Some(&head))
            || (!rebasing && (last_checkout.is_none_or(|m| !checkout_valid(m))))
        {
            selected.problem = Some(Problem::UnassociatedHead);
            return Ok(selected);
        }
    }
    selected.record = Some(record);
    Ok(selected)
}
fn fail(plan: &mut Plan, problem: Problem) {
    plan.state = State::ChangedOrUnverifiable;
    plan.blockers.push(Blocker::Recovery(problem));
    plan.effects.clear();
}
fn base_transition(message: &[u8], record: &Record) -> bool {
    message == record.base_marker().as_bytes()
        || message == format!("{}: Fast-forward", record.base_marker()).as_bytes()
}
fn transition(record: &Record, plan: &Plan, result: &[u8]) -> bool {
    if !record
        .target_version
        .prefix(plan.snapshot.target_log.as_deref())
    {
        return false;
    }
    let entries = entries(plan.snapshot.target_log.as_deref());
    let Some(mut tail) = entries.get(record.target_version.count..) else {
        return false;
    };
    let mut old = commit(&record.local_before)
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| vec![b'0'; result.len()]);
    if old != record.target_before
        && tail.first().is_some_and(|e| {
            e.old == old && e.new == record.target_before && base_transition(e.message, record)
        })
    {
        old = record.target_before.clone();
        tail = &tail[1..];
    }
    let tail: Vec<_> = tail
        .iter()
        .filter(|e| !aborted_reset(e, &record.target_before))
        .collect();
    tail.len() == 1
        && tail[0].old == old
        && tail[0].new == result
        && (matches!(
            record.method,
            FinishIntegration::FfOnly | FinishIntegration::Merge
        ) || tail[0].message == record.marker().as_bytes()
            || tail[0]
                .message
                .starts_with(format!("{}: ", record.marker()).as_bytes()))
}
fn valid_result(directory: &Path, record: &Record, result: &[u8]) -> Result<bool, FinishError> {
    if !valid_oid(result, record.source.len()) {
        return Ok(false);
    }
    let output = run(
        directory,
        &[
            "--no-replace-objects".into(),
            "cat-file".into(),
            "commit".into(),
            repository::os(result)?,
        ],
    )?;
    if !output.status.success() {
        return Ok(false);
    }
    let parents: Vec<_> = output
        .stdout
        .split(|b| *b == b'\n')
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.strip_prefix(b"parent "))
        .collect();
    Ok(match record.method {
        FinishIntegration::FfOnly => {
            result == record.source
                && ancestor(directory, &record.target_before, result)? == Some(true)
        }
        FinishIntegration::Merge => {
            parents == [record.target_before.as_slice(), record.source.as_slice()]
        }
        // Squash result is an exact operation-marked commit, NOT the original
        // work tip and NOT an ancestry inference from the original source.
        FinishIntegration::Squash => {
            result != record.source && parents == [record.target_before.as_slice()]
        }
        FinishIntegration::Ask => false,
    })
}
pub(super) fn assess(
    directory: &Path,
    plan: &mut Plan,
    record: &Record,
) -> Result<(), FinishError> {
    // Preparation can reveal which integration method/message is needed. These
    // unresolved choices may be bound only while no target effect has happened.
    let preparing = record.preparation.is_some()
        && record.result.is_none()
        && plan.local_target == record.local_before
        && record.target_version == LogVersion::of(plan.snapshot.target_log.as_deref());
    if record.git_dir != plan.repository.git_dir
        || record.common_dir != plan.repository.common_dir
        || Some(&record.root) != plan.repository.working_tree_root.as_ref()
        || record.settings != plan.settings
        || record.git_config != plan.snapshot.config
        || record.target != plan.target
        || (record.method != plan.method && !(record.method == FinishIntegration::Ask && preparing))
        || plan.request.update.is_some_and(|choice| {
            choice != record.update() && Some(choice) != record.requested_update
        })
        || record.shared_preparation != plan.snapshot.shared_preparation
        || record.rewrite != plan.request.rewrite
        || record.history_edit != plan.history_edit
        || record.checkout != plan.checkout
        || record.cleanup != plan.cleanup
        || (record.message != plan.request.message && !(record.message.is_none() && preparing))
        || !valid_oid(&record.source, record.target_before.len())
        || !valid_oid(&record.target_before, record.source.len())
        || !matches!(record.source.len(), 40 | 64)
        || record.token.is_empty()
        || !record
            .token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        fail(plan, Problem::ChangedBinding);
        return Ok(());
    }
    if [&record.source, &record.target_before]
        .into_iter()
        .any(|oid| !valid_oid(oid, record.source.len()))
    {
        fail(plan, Problem::MissingHistory);
        return Ok(());
    }
    let source_available = relationship(
        directory,
        &record.source,
        &RefObservation::Commit(record.source.clone()),
        plan.snapshot.shallow.is_some(),
    )? == Relationship::Equal;
    let target_available = relationship(
        directory,
        &record.target_before,
        &RefObservation::Commit(record.target_before.clone()),
        plan.snapshot.shallow.is_some(),
    )? == Relationship::Equal;
    if !source_available || !target_available {
        fail(plan, Problem::MissingHistory);
        return Ok(());
    }
    for (reference, expected, required) in [
        (record.original_ref(), record.original(), record.retained),
        (
            record.result_ref(),
            record.result.as_deref().unwrap_or_default(),
            record.result.is_some(),
        ),
    ] {
        if !required {
            continue;
        }
        let pin = hint(directory, reference.as_bytes())?;
        let bytes = log(directory, reference.as_bytes())?;
        let entries = entries(bytes.as_deref());
        if symbolic(directory, reference.as_bytes())?
            || pin != RefObservation::Commit(expected.to_vec())
            || entries.len() != 1
            || entries[0].old != vec![b'0'; expected.len()]
            || entries[0].new != expected
            || entries[0].message != b"twigsmith retain"
        {
            fail(plan, Problem::InvalidResult);
            return Ok(());
        }
    }
    let mut verified = record.clone();
    if let Some(saved) = &record.cleanup_source_log
        && (record.source_version != LogVersion::of(Some(saved))
            || (plan.source != RefObservation::Absent
                && plan.snapshot.source_log.as_ref() != Some(saved)))
    {
        fail(plan, Problem::ChangedSource);
        return Ok(());
    }
    if let Some(preparation) = &record.preparation {
        if plan
            .evidence
            .as_ref()
            .and_then(|e| e.preparation.as_ref())
            .is_some_and(|p| p.branch != RefObservation::Commit(preparation.base.clone()))
        {
            fail(plan, Problem::ChangedBinding);
            return Ok(());
        }
        let mut proof = plan.clone();
        if plan.source == RefObservation::Absent {
            proof.source = RefObservation::Commit(record.source.clone());
            proof.snapshot.source_log = record.cleanup_source_log.clone();
        }
        if !verify_preparation(directory, &proof, record, preparation)? {
            fail(plan, Problem::ChangedSource);
            return Ok(());
        }
        verified.source = commit(&plan.source).unwrap_or(&record.source).to_vec();
        verified.source_version = LogVersion::of(proof.snapshot.source_log.as_deref());
    }
    let record = &verified;
    // Keep the corroborated source available to execution without requiring an
    // acknowledgement write after Git finishes.
    plan.record = Some(verified.clone());
    let source_absent = plan.source == RefObservation::Absent;
    if (!source_absent
        && (plan.source != RefObservation::Commit(record.source.clone())
            || record.source_version != LogVersion::of(plan.snapshot.source_log.as_deref())))
        || (source_absent && !matches!(plan.cleanup, FinishCleanup::Local | FinishCleanup::Both))
    {
        fail(plan, Problem::ChangedSource);
        return Ok(());
    }
    let no_op = (record.history_edit != HistoryEdit::Interactive || record.history_prepared(plan))
        && plan.local_target == RefObservation::Commit(record.target_before.clone())
        && ancestor(directory, &record.source, &record.target_before)? == Some(true);
    let unchanged_target = target_unintegrated(record, plan);
    let result = commit(&plan.local_target).map(<[u8]>::to_vec);
    if !(no_op && unchanged_target) && result.as_ref().is_none_or(|r| !transition(record, plan, r))
    {
        if unchanged_target && record.result.is_none() && !source_absent {
            if plan.snapshot.state.operations.is_active()
                || plan
                    .snapshot
                    .state
                    .working_tree
                    .as_ref()
                    .is_some_and(|t| t.is_dirty())
                || metadata(directory, "SQUASH_MSG")?.is_some()
            {
                plan.state = State::IncompleteOperation;
                if !plan.blockers.contains(&Blocker::GitOperation) {
                    plan.blockers.push(Blocker::GitOperation);
                }
            } else {
                // A recorded attempt with no Git effect can be assessed afresh,
                // but cannot adopt a newly observed target.
                if plan.evidence.as_ref().is_some_and(|e| {
                    e.fetched != RefObservation::Commit(record.target_before.clone())
                }) {
                    fail(plan, Problem::ChangedTarget);
                } else {
                    assess_fresh(directory, plan)?;
                }
            }
        } else {
            fail(plan, Problem::InvalidResult);
        }
        return Ok(());
    }
    let result = result.expect("verified target");
    if record.result.as_ref().is_some_and(|r| r != &result)
        || (!no_op && !valid_result(directory, record, &result)?)
    {
        fail(plan, Problem::InvalidResult);
        return Ok(());
    }
    plan.result = Some(result.clone());
    plan.state = State::PreparedResult;
    if !validate_evidence(plan) {
        return Ok(());
    }
    let e = plan.evidence.clone().expect("validated");
    let before = RefObservation::Commit(record.target_before.clone());
    let integrated = RefObservation::Commit(result.clone());
    // Fetch endpoints can lag publication endpoints, but cannot introduce other history.
    if e.fetched != before && e.fetched != integrated {
        plan.blockers.push(Blocker::TargetChanged(
            plan.target.fetch_destination.clone(),
        ));
    }
    let mut pending = false;
    for d in &e.targets {
        if d.branch == integrated {
            continue;
        }
        if d.branch != before {
            plan.blockers
                .push(Blocker::TargetChanged(d.destination.clone()));
            continue;
        }
        pending = true;
        plan.effects.push(Effect::PublishTarget {
            remote: plan.target.remote.clone(),
            destination: d.destination.clone(),
            reference: plan.target.reference.clone(),
            expected: record.target_before.clone(),
            commit: result.clone(),
        });
    }
    if plan
        .blockers
        .iter()
        .any(|b| matches!(b, Blocker::TargetChanged(_)))
    {
        plan.state = State::ChangedOrUnverifiable;
        return Ok(());
    }
    if pending {
        plan.state = State::PendingPublication;
        plan.effects
            .insert(0, Effect::CheckResult { commit: result });
        if source_absent {
            fail(plan, Problem::ChangedSource);
        }
        return Ok(());
    }
    plan.state = State::Published;
    if plan.has_remaining_destinations() {
        return Ok(());
    }
    completion_choices(plan);
    if (plan.checkout == FinishCheckout::Work
        || matches!(plan.cleanup, FinishCleanup::Local | FinishCleanup::Both))
        && in_another_worktree(plan, &plan.source_ref)
    {
        plan.blockers.push(Blocker::SourceInAnotherWorktree);
    }

    let checkout_ref = if plan.checkout == FinishCheckout::Target {
        record.completed.first().map_or_else(
            || plan.target.reference.clone(),
            |d| d.target.reference.clone(),
        )
    } else {
        plan.source_ref.clone()
    };
    if current_ref(&plan.snapshot.state).as_ref() != Some(&checkout_ref) {
        plan.effects.push(Effect::Checkout {
            reference: checkout_ref,
            expected: if plan.checkout == FinishCheckout::Target {
                record
                    .completed
                    .first()
                    .map_or(result, |d| d.result.clone())
            } else {
                record.source.clone()
            },
        });
    }
    if matches!(plan.cleanup, FinishCleanup::Local | FinishCleanup::Both) && !source_absent {
        plan.effects.push(Effect::DeleteLocalWork {
            reference: plan.source_ref.clone(),
            expected: record.source.clone(),
        });
    }
    if plan.cleanup == FinishCleanup::Both {
        let destinations = urls(directory, &plan.settings.publish_remote, true)?;
        if e.work.iter().any(|a| {
            e.work
                .iter()
                .any(|b| a.destination == b.destination && a.branch != b.branch)
        }) || e.work.len() != destinations.len()
            || record.work.len() != destinations.len()
            || e.work
                .iter()
                .zip(&destinations)
                .any(|(d, u)| &d.destination != u)
            || record
                .work
                .iter()
                .zip(&destinations)
                .any(|(d, u)| &d.destination != u)
        {
            plan.blockers.push(Blocker::InvalidEvidence);
            return Ok(());
        }
        for (now, original) in e.work.iter().zip(&record.work) {
            if now.branch == RefObservation::Absent {
                continue;
            }
            let Some(expected) = commit(&original.branch) else {
                plan.blockers
                    .push(Blocker::WorkBranchChanged(now.destination.clone()));
                continue;
            };
            if now.branch != original.branch
                || !record.incorporates_published_work(directory, plan, expected)?
            {
                plan.blockers
                    .push(Blocker::WorkBranchChanged(now.destination.clone()));
                continue;
            }
            plan.effects.push(Effect::DeletePublishedWork {
                remote: plan.settings.publish_remote.clone(),
                destination: now.destination.clone(),
                reference: plan.source_ref.clone(),
                expected: expected.to_vec(),
            });
        }
    }
    if plan.effects.is_empty() && plan.blockers.is_empty() && plan.required_input.is_empty() {
        plan.state = State::Complete;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    base: Vec<u8>,
    method: FinishUpdate,
    original: Vec<u8>,
    source_version: LogVersion,
    head_version: LogVersion,
}
struct Entry<'a> {
    old: &'a [u8],
    new: &'a [u8],
    message: &'a [u8],
}
fn entries(bytes: Option<&[u8]>) -> Vec<Entry<'_>> {
    bytes
        .unwrap_or_default()
        .split(|b| *b == b'\n')
        .filter_map(|line| {
            // Native linked-worktree creation can log an entry without an
            // action. Retain it so offsets agree with raw LogVersion counts.
            let (fields, message) =
                line.iter()
                    .position(|b| *b == b'\t')
                    .map_or((line, &b""[..]), |index| {
                        let (fields, message) = line.split_at(index);
                        (fields, &message[1..])
                    });
            let mut ids = fields.split(|b| *b == b' ');
            Some(Entry {
                old: ids.next()?,
                new: ids.next()?,
                message,
            })
        })
        .collect()
}
fn verify_preparation(
    directory: &Path,
    plan: &Plan,
    record: &Record,
    prep: &Preparation,
) -> Result<bool, FinishError> {
    if !matches!(prep.method, FinishUpdate::Merge | FinishUpdate::Rebase) {
        return Ok(false);
    }
    if !prep
        .source_version
        .prefix(plan.snapshot.source_log.as_deref())
        || !prep.head_version.prefix(plan.snapshot.head_log.as_deref())
    {
        return Ok(false);
    }
    let branch = entries(plan.snapshot.source_log.as_deref());
    let Some(tail) = branch.get(prep.source_version.count..) else {
        return Ok(false);
    };
    let tail: Vec<_> = tail
        .iter()
        .filter(|e| {
            record.history_edit == HistoryEdit::Interactive || !aborted_reset(e, &prep.original)
        })
        .collect();
    let Some(source) = commit(&plan.source) else {
        return Ok(false);
    };
    let no_change = record.history_edit == HistoryEdit::Interactive
        && tail.is_empty()
        && source == prep.original
        && record.history_prepared(plan);
    if tail.is_empty() && !no_change {
        return Ok(source == prep.original);
    }
    if !no_change
        && (tail.len() != 1
            || tail[0].old != prep.original
            || tail[0].new != source
            || ancestor(directory, &prep.base, source)? != Some(true))
    {
        return Ok(false);
    }
    if prep.method == FinishUpdate::Merge {
        return Ok(ancestor(directory, &prep.original, source)? == Some(true)
            && (tail[0]
                .message
                .starts_with(record.prepare_marker().as_bytes())
                || tail[0].message.starts_with(b"commit (merge): ")));
    }
    if prep.method != FinishUpdate::Rebase {
        return Ok(false);
    }
    let finish = |action: &str| {
        [
            action.as_bytes(),
            b" (finish): ",
            &record.source_ref,
            b" onto ",
            &prep.base,
        ]
        .concat()
    };
    if !no_change
        && tail[0].message != finish("rebase")
        && tail[0].message != finish(&record.prepare_marker())
    {
        return Ok(false);
    }
    let head = entries(plan.snapshot.head_log.as_deref());
    let Some(interval) = head.get(prep.head_version.count..) else {
        return Ok(false);
    };
    let start = [
        record.prepare_marker().as_bytes(),
        b" (start): checkout ",
        &prep.base,
    ]
    .concat();
    let mut begin = None;
    for (index, entry) in interval.iter().enumerate() {
        if entry.old == prep.original
            && entry.message == start
            && (entry.new == prep.base
                || (record.history_edit == HistoryEdit::Interactive
                    && ancestor(directory, &prep.base, entry.new)? == Some(true)
                    && ancestor(directory, entry.new, &prep.original)? == Some(true)))
        {
            begin = Some(index);
        }
    }
    let Some(begin) = begin else {
        return Ok(false);
    };
    let end_message = |action: &str| {
        [
            action.as_bytes(),
            b" (finish): returning to ",
            &record.source_ref,
        ]
        .concat()
    };
    let Some(end) = interval[begin + 1..].iter().position(|e| {
        e.new == source
            && (e.message == end_message("rebase")
                || e.message == end_message(&record.prepare_marker()))
    }) else {
        return Ok(false);
    };
    if record.history_edit == HistoryEdit::Interactive {
        let mut on_source = true;
        let source_name = record
            .source_ref
            .strip_prefix(b"refs/heads/")
            .unwrap_or(&record.source_ref);
        for entry in &interval[begin + 2 + end..] {
            if entry.message.starts_with(b"checkout: moving from ") {
                on_source = entry.message.ends_with(&[b" to ", source_name].concat());
            } else if on_source {
                return Ok(false);
            }
        }
    }
    Ok(!interval[begin + 1..begin + 1 + end].iter().any(|e| {
        e.message
            .windows(8)
            .any(|w| w == b"(abort):" || w == b"(start):")
    }))
}
fn aborted_reset(entry: &Entry<'_>, oid: &[u8]) -> bool {
    entry.old == oid
        && entry.new == oid
        && matches!(
            entry.message,
            b"reset: moving to ORIG_HEAD" | b"reset: moving to HEAD"
        )
}
fn target_unintegrated(record: &Record, plan: &Plan) -> bool {
    if !record
        .target_version
        .prefix(plan.snapshot.target_log.as_deref())
    {
        return false;
    }
    let log = entries(plan.snapshot.target_log.as_deref());
    let Some(mut tail) = log.get(record.target_version.count..) else {
        return false;
    };
    let old = commit(&record.local_before)
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| vec![b'0'; record.source.len()]);
    let aligned = old != record.target_before
        && tail.first().is_some_and(|e| {
            e.old == old && e.new == record.target_before && base_transition(e.message, record)
        });
    if aligned {
        tail = &tail[1..];
    }
    if tail.is_empty() && !aligned {
        return plan.local_target == record.local_before;
    }
    plan.local_target == RefObservation::Commit(record.target_before.clone())
        && tail.iter().all(|e| aborted_reset(e, &record.target_before))
}

impl Record {
    pub(super) fn release_action(&self) -> String {
        format!("twigsmith-release:{}", self.token)
    }
    pub(super) fn preparation_base(&self) -> &[u8] {
        self.preparation
            .as_ref()
            .map_or(&self.target_before, |p| &p.base)
    }
    pub(super) fn prepare_marker(&self) -> String {
        format!("twigsmith-prepare-{}", self.token)
    }
    pub(super) fn base_marker(&self) -> String {
        format!("twigsmith-base-{}", self.token)
    }
    pub fn original_ref(&self) -> String {
        format!("refs/twigsmith/integration/{}/original", self.token)
    }
    pub fn result_ref(&self) -> String {
        if self.completed.is_empty() {
            format!("refs/twigsmith/integration/{}/result", self.token)
        } else {
            format!(
                "refs/twigsmith/integration/{}/result-{}",
                self.token,
                self.completed.len()
            )
        }
    }
    pub(super) fn original(&self) -> &[u8] {
        self.preparation
            .as_ref()
            .map_or(&self.source, |p| &p.original)
    }
    pub(super) fn bind_choices(&mut self, plan: &Plan) {
        self.method = plan.method;
        self.message = plan.request.message.clone();
    }
    pub(super) fn retain_choices(&mut self, plan: &Plan) -> Result<(), FinishError> {
        if self.preparation.is_none()
            || self.result.is_some()
            || plan.state != State::Fresh
            || plan.integration == Integration::AfterPreparation
            || plan.local_target != self.local_before
            || self.target_version != LogVersion::of(plan.snapshot.target_log.as_deref())
            || !plan.blockers.is_empty()
            || plan
                .required_input
                .iter()
                .any(|input| *input != RequiredInput::Message)
            || plan.method == FinishIntegration::Ask
        {
            return Err(FinishError::InvalidEvidence(
                "retaining choices requires unresolved target effects after verified preparation",
            ));
        }
        self.bind_choices(plan);
        Ok(())
    }
}

/// Write a complete private record before Git effects. Callers revalidate the
/// selected bytes first. Temporary files and parent directories are synced.
pub(super) fn write<T: Serialize>(
    root: &Path,
    path: &Path,
    value: &T,
    replace: bool,
) -> Result<(), FinishError> {
    use std::io::Write;
    private_read(root, path).map_err(|_| FinishError::InvalidEvidence("unsafe recovery path"))?;
    let relative = path
        .parent()
        .unwrap()
        .strip_prefix(root)
        .map_err(|_| FinishError::InvalidEvidence("invalid recovery path"))?;
    let mut current = root.to_owned();
    for part in relative.components() {
        current.push(part);
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&current) {
            Ok(()) => {
                fs::File::open(current.parent().unwrap())?.sync_all()?;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
    }
    private_read(root, path).map_err(|_| FinishError::InvalidEvidence("unsafe recovery path"))?;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(&serde_json::to_vec(value).map_err(io::Error::other)?)?;
    file.as_file().sync_all()?;
    if replace {
        file.persist(path).map_err(|e| e.error)?;
    } else {
        file.persist_noclobber(path).map_err(|e| e.error)?;
    }
    fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
