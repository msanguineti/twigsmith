//! Private, versioned rebase evidence. Reflogs are corroborating observations,
//! not authentication against a process that can edit this repository itself.
use super::*;
use serde::{Deserialize, Serialize};
use std::{io::Write, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    Absent,
    /// No completed rewrite beyond the original tip (not started or aborted).
    Unchanged,
    Verified(VerifiedRewrite),
    /// Ignored for authorization; ordinary ancestry constraints still apply.
    Invalid,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRewrite {
    pub original: Vec<u8>,
    pub prepared: Vec<u8>,
    pub destinations: Vec<DestinationEvidence>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogEntry {
    commit: Vec<u8>,
    message: Vec<u8>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Anchor {
    count: usize,
    tip: Option<LogEntry>,
}
impl Anchor {
    fn new(log: &[LogEntry]) -> Self {
        Self {
            count: log.len(),
            tip: log.last().cloned(),
        }
    }
    fn matches(&self, log: &[LogEntry]) -> bool {
        self.count <= log.len()
            && self.tip.as_ref() == self.count.checked_sub(1).and_then(|i| log.get(i))
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    before: Vec<u8>,
    base: Vec<u8>,
    destinations: Vec<DestinationEvidence>,
    action: String,
    branch_log: Anchor,
    head_log: Anchor,
}
/// Original observations used to re-prove incorporation at each destination.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedEvidence {
    remote: String,
    base_ref: Vec<u8>,
    branch_ref: Vec<u8>,
    base: RefObservation,
    destinations: Vec<DestinationEvidence>,
}
impl From<&RemoteEvidence> for RecordedEvidence {
    fn from(evidence: &RemoteEvidence) -> Self {
        Self {
            remote: evidence.publish_remote.clone(),
            base_ref: evidence.base_ref.clone(),
            branch_ref: evidence.branch_ref.clone(),
            base: evidence.base.clone(),
            destinations: evidence.destinations.clone(),
        }
    }
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleBinding {
    base_remote: String,
    base_destination: Vec<u8>,
    // Publication name, refs and resolved destinations are in RecordedEvidence.
    // Keep role transport/routing configuration in order, including duplicates.
    remote_config: Vec<Vec<u8>>,
}
impl RoleBinding {
    fn new(plan: &Plan) -> Self {
        let prefixes = [
            format!("remote.{}.", plan.settings.base_remote),
            format!("remote.{}.", plan.settings.publish_remote),
        ];
        let remote_config = plan
            .git_config
            .split(|b| *b == 0)
            .filter(|entry| {
                let key = entry.split(|b| *b == b'\n').next().unwrap_or_default();
                key.starts_with(b"url.")
                    || prefixes.iter().any(|prefix| {
                        key.strip_prefix(prefix.as_bytes())
                            .is_some_and(|suffix| !suffix.contains(&b'.'))
                    })
            })
            .map(<[u8]>::to_vec)
            .collect();
        Self {
            base_remote: plan.settings.base_remote.clone(),
            base_destination: plan.base_destination.clone(),
            remote_config,
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    version: u32,
    // OS paths remain bytes, like repository/ref observations elsewhere.
    repository: Vec<Vec<u8>>,
    evidence: RecordedEvidence,
    roles: RoleBinding,
    original: Vec<u8>,
    attempts: Vec<Attempt>,
}
fn identity(plan: &Plan) -> Vec<Vec<u8>> {
    [
        &plan.repository.git_dir,
        &plan.repository.common_dir,
        plan.repository
            .working_tree_root
            .as_ref()
            .expect("working plan"),
    ]
    .iter()
    .map(|p| p.as_os_str().as_encoded_bytes().to_vec())
    .collect()
}
pub(super) fn path(plan: &Plan) -> Result<PathBuf, FinishError> {
    // Names came from an attached Git ref; valid refs have no dot/dot-dot path
    // components. Keeping the hierarchy avoids filename-length amplification.
    Ok(plan
        .repository
        .git_dir
        .join("oneflow-finish")
        .join("branches")
        .join(repository::os(&plan.branch)?)
        .join("rewrite.json"))
}
fn logs(directory: &Path, reference: &[u8]) -> Result<Vec<LogEntry>, FinishError> {
    let output = run(
        directory,
        &[
            "--no-replace-objects".into(),
            "reflog".into(),
            "show".into(),
            "--no-show-signature".into(),
            "--no-decorate".into(),
            "--no-color".into(),
            "--no-notes".into(),
            "--format=%H%x00%gs".into(),
            repository::os(reference)?,
            "--".into(),
        ],
    )?;
    if !output.status.success() {
        return Err(FinishError::InvalidEvidence("reflog unavailable"));
    }
    let mut entries = vec![];
    for line in output
        .stdout
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
    {
        let Some(index) = line.iter().position(|b| *b == 0) else {
            return Err(FinishError::InvalidEvidence("invalid reflog record"));
        };
        entries.push(LogEntry {
            commit: line[..index].to_vec(),
            message: line[index + 1..].to_vec(),
        });
    }
    entries.reverse();
    Ok(entries)
}
fn private_path(plan: &Plan) -> Result<(), FinishError> {
    let path = path(plan)?;
    let mut current = plan.repository.git_dir.clone();
    for part in path
        .strip_prefix(&current)
        .expect("private path")
        .components()
    {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(FinishError::InvalidEvidence("symbolic recovery path"));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o077 != 0 {
                        return Err(FinishError::InvalidEvidence(
                            "recovery metadata is not private",
                        ));
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn load(plan: &Plan) -> Result<Option<Record>, FinishError> {
    private_path(plan)?;
    match fs::read(path(plan)?) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| FinishError::InvalidEvidence("invalid recovery record")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn finish_message(action: &str, branch: &[u8], base: &[u8]) -> Vec<u8> {
    [action.as_bytes(), b" (finish): ", branch, b" onto ", base].concat()
}
impl Record {
    fn verify(&self, directory: &Path, plan: &Plan) -> Result<Recovery, FinishError> {
        let invalid = || Ok(Recovery::Invalid);
        let evidence = plan.evidence.as_ref().expect("fresh evidence");
        if self.version != 2
            || self.roles != RoleBinding::new(plan)
            || self.repository != identity(plan)
            || self.evidence.remote != evidence.publish_remote
            || self.evidence.base_ref != evidence.base_ref
            || self.evidence.branch_ref != evidence.branch_ref
            || self
                .evidence
                .destinations
                .iter()
                .map(|d| &d.destination)
                .collect::<Vec<_>>()
                != evidence
                    .destinations
                    .iter()
                    .map(|d| &d.destination)
                    .collect::<Vec<_>>()
            || !valid_oid(&self.original, plan.commit.len())
            || self.attempts.is_empty()
            || self.attempts.first().is_some_and(|a| {
                RefObservation::Commit(a.base.clone()) != self.evidence.base
                    || a.destinations != self.evidence.destinations
            })
            || plan.grafts.as_ref().is_some_and(|b| !b.is_empty())
        {
            return invalid();
        }
        // Re-prove original incorporation. A permission bit or journal entry is
        // never a substitute, including when the observed destination changed.
        for dest in &self.evidence.destinations {
            if self
                .evidence
                .destinations
                .iter()
                .any(|other| other.destination == dest.destination && other.branch != dest.branch)
            {
                return invalid();
            }
            match &dest.branch {
                RefObservation::Absent => (),
                RefObservation::Commit(oid)
                    if valid_oid(oid, plan.commit.len())
                        && ancestor(directory, oid, &self.original)? == Some(true) => {}
                _ => return invalid(),
            }
        }
        let branch = logs(directory, &evidence.branch_ref)?;
        let head = logs(directory, b"HEAD")?;
        let mut before = self.original.clone();
        let mut destinations = self.evidence.destinations.clone();
        let mut completed = 0;
        for (i, attempt) in self.attempts.iter().enumerate() {
            if attempt.before != before
                || !valid_oid(&attempt.base, plan.commit.len())
                || !attempt.action.starts_with("oneflow-finish-")
                || !attempt.branch_log.matches(&branch)
                || !attempt.head_log.matches(&head)
                || attempt.branch_log.tip.as_ref().map(|e| &e.commit) != Some(&before)
                || attempt.head_log.tip.as_ref().map(|e| &e.commit) != Some(&before)
            {
                return invalid();
            }
            if attempt.destinations.len() != destinations.len() {
                return invalid();
            }
            for (observed, previous) in attempt.destinations.iter().zip(&destinations) {
                if observed.destination != previous.destination {
                    return invalid();
                }
                match &observed.branch {
                    RefObservation::Absent => (),
                    RefObservation::Commit(oid)
                        if valid_oid(oid, plan.commit.len())
                            && (ancestor(directory, oid, &before)? == Some(true)
                                || (completed > 0 && observed == previous)) => {}
                    _ => return invalid(),
                }
            }
            let end_branch = self
                .attempts
                .get(i + 1)
                .map_or(branch.len(), |a| a.branch_log.count);
            let head_limit = self
                .attempts
                .get(i + 1)
                .map_or(head.len(), |a| a.head_log.count);
            if end_branch == attempt.branch_log.count
                && i + 1 == self.attempts.len()
                && plan.commit == before
            {
                // An aborted/unstarted final attempt cannot supply new proof.
                break;
            }
            if end_branch <= attempt.branch_log.count
                || end_branch > branch.len()
                || head_limit > head.len()
                || head_limit < attempt.head_log.count + 2
            {
                return invalid();
            }
            let result = &branch[attempt.branch_log.count];
            if result.message
                != finish_message(&attempt.action, &evidence.branch_ref, &attempt.base)
                && result.message != finish_message("rebase", &evidence.branch_ref, &attempt.base)
            {
                return invalid();
            }
            let finishes = |entry: &LogEntry| {
                entry.commit == result.commit
                    && [attempt.action.as_str(), "rebase"].iter().any(|action| {
                        entry.message
                            == [
                                action.as_bytes(),
                                b" (finish): returning to ",
                                &evidence.branch_ref,
                            ]
                            .concat()
                    })
            };
            let Some(finish) = head[attempt.head_log.count..head_limit]
                .iter()
                .position(finishes)
            else {
                return invalid();
            };
            let end_head = attempt.head_log.count + finish + 1;
            if finish == 0 {
                return invalid();
            }
            let interval = &head[attempt.head_log.count..end_head];
            if interval[0].commit != attempt.base
                || interval[0].message
                    != [
                        attempt.action.as_bytes(),
                        b" (start): checkout ",
                        &attempt.base,
                    ]
                    .concat()
                || interval[1..interval.len() - 1].iter().any(|entry| {
                    entry
                        .message
                        .windows(8)
                        .any(|w| w == b"(abort):" || w == b"(start):")
                })
                || ancestor(directory, &attempt.base, &result.commit)? != Some(true)
            {
                return invalid();
            }
            // Commits/merges after this rebase may extend its result. Every
            // recorded branch transition must preserve that history; a reset,
            // manual rewrite, or branch recreation cannot borrow the proof.
            let mut preserved = &result.commit;
            for entry in &branch[attempt.branch_log.count + 1..end_branch] {
                if ancestor(directory, preserved, &entry.commit)? != Some(true) {
                    return invalid();
                }
                preserved = &entry.commit;
            }
            if head[head_limit - 1].commit != *preserved {
                return invalid();
            }
            before = preserved.clone();
            destinations = attempt.destinations.clone();
            completed += 1;
        }
        if plan.commit != before {
            return invalid();
        }
        if completed == 0 {
            return Ok(Recovery::Unchanged);
        }
        Ok(Recovery::Verified(VerifiedRewrite {
            original: self.original.clone(),
            prepared: before,
            destinations,
        }))
    }
}

pub(super) fn apply(directory: &Path, plan: &mut Plan) -> Result<(), FinishError> {
    plan.recovery = match load(plan) {
        Ok(Some(record)) => record.verify(directory, plan).unwrap_or(Recovery::Invalid),
        Ok(None) => Recovery::Absent,
        Err(_) => Recovery::Invalid,
    };
    let Recovery::Verified(proof) = &plan.recovery else {
        return Ok(());
    };
    for destination in &mut plan.destinations {
        // Only the exact original remote commit can use rewrite evidence.
        // A newly published or independently changed destination uses normal
        // ancestry, even if its commit was also in the old branch's history.
        if !matches!(
            destination.relationship,
            Relationship::Behind | Relationship::Diverged | Relationship::Unrelated
        ) || !proof
            .destinations
            .iter()
            .any(|d| d.destination == destination.destination && d.branch == destination.observed)
        {
            continue;
        }
        plan.constraints.retain(|c| !matches!(c, Constraint::RemoteWorkNotIncorporated(url) if url == &destination.destination));
        destination.mode = PublicationMode::VerifiedRewrite;
        match plan.rewrite {
            FinishRewrite::Ask => {
                if !plan.required_input.contains(&RequiredInput::Rewrite) {
                    plan.required_input.push(RequiredInput::Rewrite);
                }
            }
            FinishRewrite::Deny => plan
                .constraints
                .push(Constraint::RewriteDenied(destination.destination.clone())),
            FinishRewrite::Allow => (),
        }
    }
    Ok(())
}

/// Persist before launching Git. Appended attempts retain the original proof
/// when the remote base advances again before a previous rewrite is published.
pub(super) fn record(directory: &Path, plan: &Plan) -> Result<(PathBuf, String), FinishError> {
    private_path(plan)?;
    let evidence = plan.evidence.clone().expect("ready plan");
    let branch_log = Anchor::new(&logs(directory, &evidence.branch_ref)?);
    let head_log = Anchor::new(&logs(directory, b"HEAD")?);
    if branch_log.tip.as_ref().map(|e| &e.commit) != Some(&plan.commit)
        || head_log.tip.as_ref().map(|e| &e.commit) != Some(&plan.commit)
    {
        return Err(FinishError::InvalidEvidence(
            "finish rebase requires HEAD and branch reflogs at the current commit",
        ));
    }
    let mut record = match (&plan.recovery, load(plan)) {
        (Recovery::Verified(_), Ok(Some(mut record))) => {
            // Discard an aborted/unstarted tail; it has no completed rewrite.
            if record
                .attempts
                .last()
                .is_some_and(|a| a.before == plan.commit)
            {
                record.attempts.pop();
            }
            record
        }
        _ => Record {
            version: 2,
            repository: identity(plan),
            evidence: RecordedEvidence::from(&evidence),
            roles: RoleBinding::new(plan),
            original: plan.commit.clone(),
            attempts: vec![],
        },
    };
    let path = path(plan)?;
    let parent = path.parent().unwrap();
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    private_path(plan)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    let action = format!(
        "oneflow-finish-{}",
        file.path().file_name().unwrap().to_string_lossy()
    );
    let RefObservation::Commit(base) = evidence.base else {
        unreachable!("ready base")
    };
    record.attempts.push(Attempt {
        before: plan.commit.clone(),
        base,
        destinations: evidence.destinations,
        action: action.clone(),
        branch_log,
        head_log,
    });
    serde_json::to_writer(&mut file, &record).map_err(io::Error::other)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(&path).map_err(|e| e.error)?;
    // Directory syncing makes the rename durable on supported platforms.
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok((path, action))
}
