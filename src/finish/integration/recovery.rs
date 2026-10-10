//! Versioned private evidence, with no writer or executor. Reflog corroboration
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
    fn of(bytes: Option<&[u8]>) -> Self {
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
/// Stored separately per worktree and source branch. No success flags or reusable
/// rewrite permission. Remaining effects are derived from actual observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    version: u32,
    git_dir: PathBuf,
    common_dir: PathBuf,
    root: PathBuf,
    source_ref: Vec<u8>,
    source: Vec<u8>,
    source_version: LogVersion,
    target: Target,
    target_before: Vec<u8>,
    local_before: RefObservation,
    target_version: LogVersion,
    head_version: LogVersion,
    settings: Settings,
    git_config: Vec<u8>,
    method: FinishIntegration,
    requested_update: Option<FinishUpdate>,
    checkout: FinishCheckout,
    cleanup: FinishCleanup,
    message: Option<String>,
    /// Exact originally observed published source versions, for conditional deletion.
    work: Vec<DestinationEvidence>,
    token: String,
    /// Optional acknowledgement. A lost write can be recovered from the exact
    /// operation-marked target reflog transition and the actual commit graph.
    pub result: Option<Vec<u8>>,
}
impl Record {
    /// Build evidence in memory only. Future execution must persist it privately
    /// before effects and use marker() for the target transition's reflog action.
    pub fn for_plan(plan: &Plan, token: &str) -> Result<Self, FinishError> {
        if !plan.is_ready()
            || plan.state != State::Fresh
            || plan.record.is_some()
            || plan.integration == Integration::AfterPreparation
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
            version: 1,
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
            checkout: plan.checkout,
            cleanup: plan.cleanup,
            message: plan.request.message.clone(),
            work: e.work.clone(),
            token: token.into(),
            result: None,
        })
    }
    pub fn marker(&self) -> String {
        format!(
            "oneflow-integrate:{}:{}:{}:{}",
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
        .join("oneflow-integration/branches")
        .join(source_branch)
        .join("attempt.json")
}
pub fn selection_path(repository: &repository::RepositoryContext) -> PathBuf {
    repository.git_dir.join("oneflow-integration/active.json")
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
    let mut selected = Selected {
        source_ref: current_ref(state),
        path: None,
        record: None,
        problem: None,
        bytes: vec![],
    };
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
        || record.version != 1
        || record.source_ref != *reference
        || record.source_ref == record.target.reference
    {
        selected.problem = Some(Problem::MalformedRecord);
        return Ok(selected);
    }
    if association.is_some() {
        let head = metadata(directory, "logs/HEAD")?.unwrap_or_default();
        let checkout = [
            b"checkout: moving from ",
            branch.as_bytes(),
            b" to ",
            &record.target.reference[b"refs/heads/".len()..],
        ]
        .concat();
        let last_checkout = head
            .split(|b| *b == b'\n')
            .skip(record.head_version.count)
            .filter_map(|l| l.split(|b| *b == b'\t').nth(1))
            .filter(|m| m.starts_with(b"checkout: "))
            .last();
        if !record.head_version.prefix(Some(&head))
            || last_checkout != Some(checkout.as_slice())
            || current_ref(state).as_deref() != Some(record.target.reference.as_slice())
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
fn transition(record: &Record, plan: &Plan, result: &[u8]) -> bool {
    let current = LogVersion::of(plan.snapshot.target_log.as_deref());
    if !record
        .target_version
        .prefix(plan.snapshot.target_log.as_deref())
        || current.count != record.target_version.count + 1
    {
        return false;
    }
    let Some(tab) = current.last.iter().position(|b| *b == b'\t') else {
        return false;
    };
    let (fields, message) = (&current.last[..tab], &current.last[tab + 1..]);
    let mut ids = fields.split(|b| *b == b' ');
    let old = commit(&record.local_before)
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| vec![b'0'; result.len()]);
    ids.next() == Some(old.as_slice())
        && ids.next() == Some(result)
        && (matches!(
            record.method,
            FinishIntegration::FfOnly | FinishIntegration::Merge
        ) || message == record.marker().as_bytes()
            || message.starts_with(format!("{}: ", record.marker()).as_bytes()))
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
    if record.git_dir != plan.repository.git_dir
        || record.common_dir != plan.repository.common_dir
        || Some(&record.root) != plan.repository.working_tree_root.as_ref()
        || record.settings != plan.settings
        || record.git_config != plan.snapshot.config
        || record.target != plan.target
        || record.method != plan.method
        || record.requested_update != plan.request.update
        || record.checkout != plan.checkout
        || record.cleanup != plan.cleanup
        || record.message != plan.request.message
        || !valid_oid(&record.source, record.target_before.len())
        || !valid_oid(&record.target_before, record.source.len())
        || !matches!(record.source.len(), 40 | 64)
        || record.token.is_empty()
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
    let source_absent = plan.source == RefObservation::Absent;
    if (!source_absent
        && (plan.source != RefObservation::Commit(record.source.clone())
            || record.source_version != LogVersion::of(plan.snapshot.source_log.as_deref())))
        || (source_absent && !matches!(plan.cleanup, FinishCleanup::Local | FinishCleanup::Both))
    {
        fail(plan, Problem::ChangedSource);
        return Ok(());
    }
    let no_op = plan.local_target == RefObservation::Commit(record.target_before.clone())
        && ancestor(directory, &record.source, &record.target_before)? == Some(true);
    let unchanged_target = plan.local_target == record.local_before
        && record.target_version == LogVersion::of(plan.snapshot.target_log.as_deref());
    let result = commit(&plan.local_target).map(<[u8]>::to_vec);
    if !(no_op && unchanged_target) && result.as_ref().is_none_or(|r| !transition(record, plan, r))
    {
        if plan.local_target == record.local_before
            && record.target_version == LogVersion::of(plan.snapshot.target_log.as_deref())
            && record.result.is_none()
            && !source_absent
        {
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
    completion_choices(plan);
    if (plan.checkout == FinishCheckout::Work
        || matches!(plan.cleanup, FinishCleanup::Local | FinishCleanup::Both))
        && in_another_worktree(plan, &plan.source_ref)
    {
        plan.blockers.push(Blocker::SourceInAnotherWorktree);
    }

    let checkout_ref = if plan.checkout == FinishCheckout::Target {
        plan.target.reference.clone()
    } else {
        plan.source_ref.clone()
    };
    if current_ref(&plan.snapshot.state).as_ref() != Some(&checkout_ref) {
        plan.effects.push(Effect::Checkout {
            reference: checkout_ref,
            expected: if plan.checkout == FinishCheckout::Target {
                result
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
                || ancestor(directory, expected, &record.source)? != Some(true)
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
