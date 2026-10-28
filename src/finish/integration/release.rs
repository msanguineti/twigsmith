//! Version identity and the additional delivery boundary of tagged workstreams.
//! The integration record supplies repository, source, route and check-policy
//! binding. Tag creation is corroborated by its exact object and marked reflog.
use super::*;
use crate::finish::{checks, preparation::CommandReport};
use std::ffi::OsString;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub tag: Option<String>,
    pub message: Option<String>,
    pub sign: bool,
}
impl Request {
    pub fn is_empty(&self) -> bool {
        self.tag.is_none() && self.message.is_none() && !self.sign
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    request: Request,
    pub(super) source: Vec<u8>,
    action: String,
    tag_object: Option<Vec<u8>>,
    source_check: Option<Vec<u8>>,
    work_check: Option<Vec<u8>>,
    work_bypassed: bool,
    source_bypassed: bool,
    candidate_check: Option<Vec<u8>>,
    candidate_bypassed: bool,
    marker: Option<Marker>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    reference: Vec<u8>,
    local_before: RefObservation,
    local_log: Option<Vec<u8>>,
    destinations: Option<Vec<DestinationEvidence>>,
}
impl Record {
    pub(super) fn request(&self, supplied: &Request) -> Request {
        Request {
            tag: supplied.tag.clone().or_else(|| self.request.tag.clone()),
            message: supplied
                .message
                .clone()
                .or_else(|| self.request.message.clone()),
            sign: supplied.sign || self.request.sign,
        }
    }
    fn reference(&self) -> Vec<u8> {
        [
            b"refs/tags/",
            self.request.tag.as_ref().expect("tag name").as_bytes(),
        ]
        .concat()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    tag: RefObservation,
    tag_log: Option<Vec<u8>>,
    marker: Option<(RefObservation, Option<Vec<u8>>)>,
}
pub(super) fn snapshot(
    root: &Path,
    request: &Request,
    record: Option<&recovery::Record>,
) -> Result<Option<Snapshot>, FinishError> {
    let Some(tag) = &request.tag else {
        return Ok(None);
    };
    if !checked_name(root, tag, "refs/tags/")? || tag.starts_with('-') {
        return Err(invalid("invalid version tag name"));
    }
    let reference = [b"refs/tags/", tag.as_bytes()].concat();
    Ok(Some(Snapshot {
        tag: hint(root, &reference)?,
        tag_log: log(root, &reference)?,
        marker: record
            .and_then(|r| r.release.as_ref())
            .and_then(|r| r.marker.as_ref())
            .map(|m| Ok::<_, FinishError>((hint(root, &m.reference)?, log(root, &m.reference)?)))
            .transpose()?,
    }))
}
pub fn enabled(plan: &Plan) -> bool {
    plan.settings
        .workstream
        .as_ref()
        .is_some_and(|p| p.version_tagging)
}
pub fn integrated_identity(plan: &Plan) -> bool {
    plan.settings
        .workstream
        .as_ref()
        .is_some_and(|p| p.version_tag_identity == settings::VersionTagIdentity::IntegratedResult)
}
pub fn identity(plan: &Plan) -> Option<(&str, &[u8])> {
    let r = plan.record.as_ref()?.release.as_ref()?;
    Some((r.request.tag.as_deref()?, &r.source))
}
#[derive(Debug, Clone)]
pub struct Status {
    pub tag: Option<String>,
    pub source: Vec<u8>,
    pub tag_object: RefObservation,
    pub marker: Option<(String, RefObservation)>,
    pub source_checks_bypassed: bool,
    pub candidate_checks_bypassed: bool,
}
pub fn status(root: &Path, plan: &Plan) -> Result<Option<Status>, FinishError> {
    if !enabled(plan) {
        return Ok(None);
    }
    let record = plan.record.as_ref().and_then(|r| r.release.as_ref());
    let source = record
        .map(|r| r.source.clone())
        .or_else(|| commit(&plan.source).map(<[u8]>::to_vec))
        .unwrap_or_default();
    let tag = plan.request.release.tag.clone();
    let tag_object = tag
        .as_ref()
        .map(|name| hint(root, &[b"refs/tags/", name.as_bytes()].concat()))
        .transpose()?
        .unwrap_or(RefObservation::Unavailable);
    let marker = plan
        .settings
        .workstream
        .as_ref()
        .and_then(|p| p.release_marker.as_ref())
        .map(|name| Ok::<_, FinishError>((name.clone(), hint(root, &branch_ref(name.as_bytes()))?)))
        .transpose()?;
    Ok(Some(Status {
        tag,
        source,
        tag_object,
        marker,
        source_checks_bypassed: record.is_some_and(|r| r.source_bypassed),
        candidate_checks_bypassed: record.is_some_and(|r| r.candidate_bypassed),
    }))
}
fn invalid(reason: &'static str) -> FinishError {
    FinishError::InvalidEvidence(reason)
}
fn checked_name(root: &Path, name: &str, prefix: &str) -> Result<bool, FinishError> {
    Ok(run(
        root,
        &["check-ref-format".into(), format!("{prefix}{name}").into()],
    )?
    .status
    .success())
}
pub(super) fn assess(root: &Path, plan: &mut Plan) -> Result<(), FinishError> {
    if !enabled(plan) {
        if !plan.request.release.is_empty()
            || plan.record.as_ref().is_some_and(|r| r.release.is_some())
        {
            plan.blockers.push(Blocker::Release(
                "tag options require an explicitly established tagged workstream policy".into(),
            ));
        }
        return Ok(());
    }
    if destination_refs(&plan.settings).contains(&plan.source_ref) {
        plan.blockers.push(Blocker::Release(
            "work source must differ from every integration destination".into(),
        ));
    }
    // Replacement objects can change Git's checked-out tree while HEAD still
    // displays the original OID. Tagged identity requires original objects.
    let replacements = run(
        root,
        &[
            "--no-replace-objects".into(),
            "for-each-ref".into(),
            "--format=%(refname)".into(),
            "--".into(),
            std::env::var_os("GIT_REPLACE_REF_BASE").unwrap_or_else(|| "refs/replace/".into()),
        ],
    )?;
    if !replacements.status.success() || !replacements.stdout.is_empty() {
        plan.blockers.push(Blocker::Release("tagged completion requires original Git objects; remove replacement refs before checking the released source".into()));
    }
    if !matches!(
        plan.request.update.unwrap_or(plan.settings.finish_update),
        FinishUpdate::None | FinishUpdate::Rebase
    ) || (plan.update == FinishUpdate::Rebase
        && plan.request.update != Some(FinishUpdate::Rebase))
        || plan.method == FinishIntegration::Squash
    {
        plan.blockers.push(Blocker::Release("tagged completion permits explicit --update rebase or --update none and requires history-preserving integration (merge or ff-only)".into()));
    }
    if plan
        .request
        .release
        .message
        .as_ref()
        .is_some_and(|m| m.trim().is_empty() || m.contains('\0'))
    {
        plan.blockers.push(Blocker::Release(
            "tag annotation must be nonblank literal text without NUL".into(),
        ));
    }
    if let Some(tag) = &plan.request.release.tag {
        if !checked_name(root, tag, "refs/tags/")? || tag.starts_with('-') {
            plan.blockers
                .push(Blocker::Release("invalid version tag name".into()));
        }
    } else {
        plan.required_input.push(RequiredInput::TagName);
    }
    if let Some(marker) = plan
        .settings
        .workstream
        .as_ref()
        .and_then(|p| p.release_marker.as_ref())
    {
        let reference = branch_ref(marker.as_bytes());
        if reference == plan.source_ref
            || destination_refs(&plan.settings).contains(&reference)
            || !checked_name(root, marker, "refs/heads/")?
        {
            plan.blockers.push(Blocker::Release("release marker must be a distinct valid branch, separate from work and integration target".into()));
        }
        if symbolic(root, &reference)?
            || in_another_worktree(plan, &reference)
            || current_ref(&plan.snapshot.state).as_ref() == Some(&reference)
        {
            plan.blockers.push(Blocker::Release(
                "release marker is symbolic or checked out in a worktree".into(),
            ));
        }
    }
    if let Some(record) = plan.record.as_ref().and_then(|r| r.release.as_ref()) {
        let expected_marker = plan
            .settings
            .workstream
            .as_ref()
            .and_then(|p| p.release_marker.as_ref())
            .map(|name| branch_ref(name.as_bytes()));
        if record.action
            != plan
                .record
                .as_ref()
                .expect("release attempt")
                .release_action()
            || record.marker.as_ref().map(|m| m.reference.clone()) != expected_marker
            || record
                .marker
                .as_ref()
                .and_then(|m| m.destinations.as_ref())
                .is_some_and(|destinations| {
                    destinations.len() != plan.target.push_destinations.len()
                        || destinations
                            .iter()
                            .zip(&plan.target.push_destinations)
                            .any(|(d, expected)| &d.destination != expected)
                })
        {
            plan.blockers.push(Blocker::Release(
                "retained tag action or marker routes do not match the attempt".into(),
            ));
        }
        if record.request != plan.request.release
            || (!record.source.is_empty()
                && if integrated_identity(plan) {
                    plan.record
                        .as_ref()
                        .and_then(|r| {
                            r.completed
                                .first()
                                .map(|d| &d.result)
                                .or(plan.result.as_ref())
                        })
                        .is_none_or(|oid| *oid != record.source)
                } else {
                    plan.record
                        .as_ref()
                        .is_none_or(|r| r.source != record.source)
                })
            || record
                .source_check
                .as_ref()
                .is_some_and(|v| v != &record.source)
            || record.work_check.as_ref().is_some_and(|oid| {
                plan.record
                    .as_ref()
                    .is_none_or(|attempt| oid != &attempt.source)
            })
            || record
                .candidate_check
                .as_ref()
                .is_some_and(|v| Some(v) != plan.result.as_ref())
        {
            plan.blockers.push(Blocker::Release(
                "retained version choices or checked commits changed".into(),
            ));
        }
        if let Err(error) = verify_tag(root, plan, record, false) {
            plan.blockers.push(Blocker::Release(error.to_string()));
        }
        if let Some(marker) = &record.marker {
            let now = hint(root, &marker.reference)?;
            if now != marker.local_before && now != RefObservation::Commit(record.source.clone()) {
                plan.blockers.push(Blocker::Release(
                    "release marker changed since this attempt".into(),
                ));
            }
            let bytes = log(root, &marker.reference)?;
            let unchanged = now == marker.local_before && bytes == marker.local_log;
            let before = marker.local_log.as_deref().unwrap_or_default();
            let after = bytes.as_deref().unwrap_or_default();
            let old = commit(&marker.local_before)
                .map(<[u8]>::to_vec)
                .unwrap_or_else(|| vec![b'0'; record.source.len()]);
            let prefix = [old.as_slice(), b" ", &record.source, b" "].concat();
            let tail = after.strip_prefix(before).unwrap_or_default();
            let own_update = now == RefObservation::Commit(record.source.clone())
                && now != marker.local_before
                && tail
                    .split(|b| *b == b'\n')
                    .filter(|l| !l.is_empty())
                    .count()
                    == 1
                && tail.starts_with(&prefix)
                && tail.split(|b| *b == b'\t').nth(1)
                    == Some([record.action.as_bytes(), b"\n"].concat().as_slice());
            if !unchanged && !own_update {
                plan.blockers.push(Blocker::Release(
                    "release marker was recreated or its reflog changed".into(),
                ));
            }
        }
    } else if plan
        .snapshot
        .release
        .as_ref()
        .is_some_and(|s| s.tag != RefObservation::Absent || s.tag_log.is_some())
    {
        plan.blockers.push(Blocker::Release(
            "version tag already exists locally; choose another explicit name".into(),
        ));
    }
    Ok(())
}
pub(super) fn for_plan(plan: &Plan, token: &str) -> Result<Option<Record>, FinishError> {
    if !enabled(plan) {
        return Ok(None);
    }
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .expect("worktree");
    Ok(Some(Record {
        request: plan.request.release.clone(),
        source: if integrated_identity(plan) || plan.integration == Integration::AfterPreparation {
            vec![]
        } else {
            commit(&plan.source)
                .ok_or(invalid("release source missing"))?
                .to_vec()
        },
        action: format!("twigsmith-release:{token}"),
        tag_object: None,
        source_check: None,
        work_check: None,
        work_bypassed: false,
        candidate_check: None,
        source_bypassed: false,
        candidate_bypassed: false,
        marker: plan
            .settings
            .workstream
            .as_ref()
            .and_then(|p| p.release_marker.as_ref())
            .map(|name| {
                let reference = branch_ref(name.as_bytes());
                Ok::<_, FinishError>(Marker {
                    local_before: hint(root, &reference)?,
                    local_log: log(root, &reference)?,
                    reference,
                    destinations: None,
                })
            })
            .transpose()?,
    }))
}
pub(super) fn next_destination(record: &mut recovery::Record) {
    if let Some(r) = &mut record.release {
        r.candidate_check = None;
        r.candidate_bypassed = false;
    }
}
fn verify_tag(
    root: &Path,
    plan: &Plan,
    record: &Record,
    required: bool,
) -> Result<Option<Vec<u8>>, FinishError> {
    let snapshot = plan
        .snapshot
        .release
        .as_ref()
        .ok_or(invalid("release tag snapshot missing"))?;
    if snapshot.tag == RefObservation::Absent && record.tag_object.is_none() && !required {
        if snapshot.tag_log.is_some() {
            return Err(invalid("unexpected tag reflog"));
        }
        return Ok(None);
    }
    let oid = commit(&snapshot.tag).ok_or(invalid("retained version tag missing"))?;
    if symbolic(root, &record.reference())? || record.tag_object.as_ref().is_some_and(|v| v != oid)
    {
        return Err(invalid("version tag object changed"));
    }
    let lines: Vec<_> = snapshot
        .tag_log
        .as_deref()
        .unwrap_or_default()
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    let prefix = [
        vec![b'0'; oid.len()],
        b" ".to_vec(),
        oid.to_vec(),
        b" ".to_vec(),
    ]
    .concat();
    let action = format!("{} (", record.action);
    if lines.len() != 1
        || !lines[0].starts_with(&prefix)
        || !lines[0]
            .split(|b| *b == b'\t')
            .nth(1)
            .is_some_and(|m| m.starts_with(action.as_bytes()))
    {
        return Err(invalid(
            "tag collision: creation is not corroborated by this retained attempt",
        ));
    }
    if hint(root, &[record.reference(), b"^{commit}".to_vec()].concat())?
        != RefObservation::Commit(record.source.clone())
    {
        return Err(invalid(
            "version tag does not identify the retained released source",
        ));
    }
    Ok(Some(oid.to_vec()))
}
fn guard(root: &Path, plan: &Plan) -> Result<(), FinishError> {
    revalidate(root, plan, plan.evidence.as_ref())?;
    if !plan.is_ready() {
        return Err(invalid("release plan is unresolved"));
    }
    Ok(())
}
fn store(root: &Path, plan: &Plan, release: Record) -> Result<Plan, FinishError> {
    guard(root, plan)?;
    let mut record = plan
        .record
        .clone()
        .ok_or(invalid("release attempt missing"))?;
    if record.release.as_ref() == Some(&release) {
        return Ok(plan.clone());
    }
    record.release = Some(release);
    recovery::write(
        &plan.repository.git_dir,
        plan.recovery_path
            .as_ref()
            .ok_or(invalid("release path missing"))?,
        &record,
        true,
    )?;
    let next = assess_after(root, plan)?;
    guard(root, &next)?;
    Ok(next)
}
fn assess_after(root: &Path, previous: &Plan) -> Result<Plan, FinishError> {
    let next = super::assess(root, &previous.request, previous.evidence.as_ref())?;
    if next.repository != previous.repository
        || next.settings != previous.settings
        || next.source_ref != previous.source_ref
        || next.source != previous.source
        || next.target != previous.target
        || next.local_target != previous.local_target
        || next.snapshot.config != previous.snapshot.config
        || next.snapshot.state != previous.snapshot.state
        || next.snapshot.index != previous.snapshot.index
        || next.snapshot.head_log != previous.snapshot.head_log
        || next.snapshot.source_log != previous.snapshot.source_log
        || next.snapshot.target_log != previous.snapshot.target_log
        || next.snapshot.worktrees != previous.snapshot.worktrees
        || next.snapshot.configuration_files != previous.snapshot.configuration_files
    {
        return Err(FinishError::ContextChanged);
    }
    Ok(next)
}
fn observe(
    root: &Path,
    plan: &Plan,
    destination: &[u8],
    reference: &[u8],
    commands: &mut Vec<CommandReport>,
) -> Result<RefObservation, FinishError> {
    guard(root, plan)?;
    let (mut args, name) = remote::endpoint(root, plan, &plan.target.remote, destination, commands)
        .map_err(|_| invalid("release endpoint unavailable or changed"))?;
    args.extend([
        "ls-remote".into(),
        "--refs".into(),
        "--".into(),
        name.into(),
        repository::os(reference)?,
    ]);
    let bytes = remote::command(root, args, commands)
        .map_err(|_| invalid("release ref observation failed"))?;
    guard(root, plan)?;
    crate::finish::preparation::parse_advertisement(
        &bytes,
        reference,
        plan.record.as_ref().expect("record").source.len(),
    )
}
fn acquire(
    root: &Path,
    plan: &Plan,
    destination: &[u8],
    oid: &[u8],
    commands: &mut Vec<CommandReport>,
) -> Result<(), FinishError> {
    if ancestor(root, oid, oid)? == Some(true) {
        return Ok(());
    }
    let (mut args, name) = remote::endpoint(root, plan, &plan.target.remote, destination, commands)
        .map_err(|_| invalid("marker endpoint unavailable"))?;
    args.extend(
        [
            "--no-replace-objects",
            "fetch",
            "--no-tags",
            "--no-prune",
            "--no-prune-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--no-auto-maintenance",
            "--no-write-commit-graph",
            "--refmap=",
            "--",
        ]
        .iter()
        .map(OsString::from),
    );
    args.extend([name.into(), repository::os(oid)?]);
    remote::command(root, args, commands)
        .map_err(|_| invalid("cannot obtain exact marker history"))?;
    guard(root, plan)
}
/// Run source checks on the exact release tree, create its tag, and retain its
/// object identity before target checkout. Recovered receipts bind unchanged
/// commits and the record's full settings/configuration snapshot.
pub(super) fn prepare(
    root: &Path,
    initial: &Plan,
    skip: bool,
    event: &mut impl FnMut(checks::Event<'_>),
    commands: &mut Vec<CommandReport>,
) -> Result<Plan, FinishError> {
    let mut plan = initial.clone();
    let Some(mut r) = plan.record.as_ref().and_then(|r| r.release.clone()) else {
        return Ok(plan);
    };
    guard(root, &plan)?;
    if plan.integration == Integration::AfterPreparation {
        return Ok(plan);
    }
    if integrated_identity(&plan) && plan.source != RefObservation::Absent {
        let work = commit(&plan.source)
            .ok_or(invalid("completed work source missing"))?
            .to_vec();
        if r.work_check.as_ref() != Some(&work) || (r.work_bypassed && !skip) {
            let original_head = current_ref(&plan.snapshot.state);
            if original_head.as_ref() != Some(&plan.source_ref) {
                plan = checkout(root, &plan, &plan.source_ref, commands)?;
            }
            let checked = checks::run_guarded(
                root,
                &plan.settings.finish_checks,
                skip,
                &mut *event,
                || {
                    guard(root, &plan)?;
                    if current_ref(&plan.snapshot.state).as_ref() != Some(&plan.source_ref)
                        || commit(&plan.source) != Some(work.as_slice())
                    {
                        return Err(checks::Error::Unprepared);
                    }
                    Ok(())
                },
            );
            if checked.error.is_some() {
                return Err(invalid(
                    "completed-work checks failed or changed context; resolve and retry",
                ));
            }
            r.work_check = Some(work);
            r.work_bypassed = skip;
            plan = store(root, &plan, r.clone())?;
            if let Some(reference) = original_head
                && current_ref(&plan.snapshot.state).as_ref() != Some(&reference)
            {
                plan = checkout(root, &plan, &reference, commands)?;
            }
        }
    }
    if r.source.is_empty() {
        let selected = if integrated_identity(&plan) {
            plan.record
                .as_ref()
                .and_then(|r| r.completed.first().map(|d| d.result.clone()))
                .or(plan.result.clone())
        } else {
            commit(&plan.source).map(<[u8]>::to_vec)
        };
        let Some(selected) = selected else {
            return Ok(plan);
        };
        r.source = selected;
        plan = store(root, &plan, r.clone())?;
    }
    let known = verify_tag(root, &plan, &r, false)?;
    let mut tag_endpoints = vec![plan.target.fetch_destination.clone()];
    for endpoint in &plan.target.push_destinations {
        if !tag_endpoints.contains(endpoint) {
            tag_endpoints.push(endpoint.clone());
        }
    }
    for endpoint in &tag_endpoints {
        let observed = observe(root, &plan, endpoint, &r.reference(), commands)?;
        if observed != RefObservation::Absent
            && known
                .as_ref()
                .is_none_or(|oid| observed != RefObservation::Commit(oid.clone()))
        {
            return Err(invalid("version tag collision at a project destination"));
        }
    }
    if let Some(marker) = &mut r.marker {
        let mut values = Vec::new();
        for endpoint in &plan.target.push_destinations {
            let value = observe(root, &plan, endpoint, &marker.reference, commands)?;
            if let Some(previous) = &marker.destinations
                && !previous
                    .iter()
                    .any(|d| d.destination == *endpoint && d.branch == value)
                && value != RefObservation::Commit(r.source.clone())
            {
                return Err(invalid("published release marker changed"));
            }
            if let RefObservation::Commit(oid) = &value {
                acquire(root, &plan, endpoint, oid, commands)?;
                if ancestor(root, oid, &r.source)? != Some(true) {
                    return Err(invalid(
                        "release marker would not fast-forward to the released source",
                    ));
                }
            } else if value != RefObservation::Absent {
                return Err(invalid("release marker observation unavailable"));
            }
            values.push(DestinationEvidence {
                destination: endpoint.clone(),
                branch: value,
            });
        }
        if let RefObservation::Commit(oid) = &marker.local_before
            && ancestor(root, oid, &r.source)? != Some(true)
        {
            return Err(invalid("local release marker would not fast-forward"));
        }
        if marker.destinations.is_none() {
            marker.destinations = Some(values);
            plan = store(root, &plan, r.clone())?;
        }
    }
    let original_head = current_ref(&plan.snapshot.state);
    if r.source_check.as_ref() != Some(&r.source) || (r.source_bypassed && !skip) {
        // The guarded completed-work receipt also validates an identical first
        // result. A bypass remains a bypass and cannot supply successful evidence.
        let reuse_work = r.work_check.as_ref() == Some(&r.source) && !r.work_bypassed;
        if !reuse_work {
            let check_ref = if integrated_identity(&plan) {
                plan.record
                    .as_ref()
                    .and_then(|r| r.completed.first())
                    .map_or(&plan.target.reference, |d| &d.target.reference)
            } else {
                &plan.source_ref
            }
            .clone();
            if current_ref(&plan.snapshot.state).as_ref() != Some(&check_ref) {
                plan = checkout(root, &plan, &check_ref, commands)?;
            }
            let checks = checks::run_guarded(
                root,
                &plan.settings.finish_checks,
                skip,
                &mut *event,
                || {
                    guard(root, &plan)?;
                    let head = match &plan.snapshot.state.head {
                        repository::Head::Attached { commit, .. } => commit,
                        _ => return Err(checks::Error::Unprepared),
                    };
                    if head != &r.source
                        || current_ref(&plan.snapshot.state).as_ref() != Some(&check_ref)
                    {
                        return Err(checks::Error::Unprepared);
                    }
                    Ok(())
                },
            );
            if checks.error.is_some() {
                return Err(invalid(
                    "released-source checks failed or changed context; resolve the check failure and retry",
                ));
            }
        }
        r.source_check = Some(r.source.clone());
        r.source_bypassed = !reuse_work && skip;
        plan = store(root, &plan, r.clone())?;
    }
    if known.is_none() {
        guard(root, &plan)?;
        let mut args: Vec<OsString> = ["tag", "--create-reflog"]
            .iter()
            .map(OsString::from)
            .collect();
        if r.request.sign {
            args.push("--sign".into());
        }
        if let Some(message) = &r.request.message {
            args.extend(["--message".into(), message.into()]);
        } else {
            // A message prevents Git from opening an editor when signing policy
            // upgrades a lightweight request into an annotated tag.
            let signing = run(
                root,
                &[
                    "config".into(),
                    "--bool".into(),
                    "--get".into(),
                    "tag.gpgSign".into(),
                ],
            )?;
            if !signing.status.success() && signing.status.code() != Some(1) {
                return Err(invalid("invalid Git tag signing preference"));
            }
            let signs = signing.stdout == b"true\n";
            if r.request.sign || signs {
                args.extend(["--message".into(), r.request.tag.as_ref().unwrap().into()]);
            }
        }
        args.extend([
            "--".into(),
            r.request.tag.as_ref().unwrap().into(),
            repository::os(&r.source)?,
        ]);
        let result = crate::git::workflow_with_reflog(root, &args, Some(&r.action));
        let success = result.as_ref().is_ok_and(|o| o.status.success());
        commands.push(CommandReport {
            arguments: args,
            result,
            after: repository::inspect(root),
        });
        if !success {
            return Err(invalid(
                "tag creation/signing failed; no unsigned fallback is permitted",
            ));
        }
        plan = assess_after(root, &plan)?;
        guard(root, &plan)?;
    }
    r.tag_object = verify_tag(root, &plan, &r, true)?;
    plan = store(root, &plan, r)?;
    if let Some(reference) = original_head
        && current_ref(&plan.snapshot.state).as_ref() != Some(&reference)
    {
        plan = checkout(root, &plan, &reference, commands)?;
    }
    Ok(plan)
}
/// Check receipts only suppress duplicate execution while the complete retained
/// binding and exact immutable result are corroborated again.
pub(crate) fn source_checked(plan: &Plan, skip: bool) -> bool {
    plan.record
        .as_ref()
        .and_then(|r| r.release.as_ref())
        .is_some_and(|r| {
            r.tag_object.is_some()
                && r.source_check.as_ref() == Some(&r.source)
                && (!r.source_bypassed || skip)
                && (!integrated_identity(plan)
                    || (plan
                        .record
                        .as_ref()
                        .is_some_and(|attempt| r.work_check.as_ref() == Some(&attempt.source))
                        && (!r.work_bypassed || skip)))
        })
}
pub(crate) fn candidate_checked(plan: &Plan, skip: bool) -> bool {
    plan.record
        .as_ref()
        .and_then(|r| r.release.as_ref())
        .is_some_and(|r| {
            (r.candidate_check.as_ref() == plan.result.as_ref()
                && r.candidate_check.is_some()
                && (!r.candidate_bypassed || skip))
                || (r.source_check.as_ref() == plan.result.as_ref()
                    && r.source_check.is_some()
                    && (!r.source_bypassed || skip))
        })
}
pub(super) fn record_candidate(root: &Path, plan: &Plan, skip: bool) -> Result<Plan, FinishError> {
    if candidate_checked(plan, false) {
        return Ok(plan.clone());
    }
    let Some(mut r) = plan.record.as_ref().and_then(|r| r.release.clone()) else {
        return Ok(plan.clone());
    };
    r.candidate_check = plan.result.clone();
    r.candidate_bypassed = skip;
    store(root, plan, r)
}
fn checkout(
    root: &Path,
    plan: &Plan,
    reference: &[u8],
    commands: &mut Vec<CommandReport>,
) -> Result<Plan, FinishError> {
    guard(root, plan)?;
    if reference != plan.source_ref
        && !destination_refs(&plan.settings)
            .iter()
            .any(|r| r == reference)
    {
        return Err(invalid("invalid release check checkout"));
    }
    let args = vec![
        "-c".into(),
        "submodule.recurse=false".into(),
        "switch".into(),
        "--no-guess".into(),
        "--no-overwrite-ignore".into(),
        "--".into(),
        repository::os(
            reference
                .strip_prefix(b"refs/heads/")
                .ok_or(invalid("not a branch"))?,
        )?,
    ];
    remote::command(root, args, commands).map_err(|_| invalid("release check checkout failed"))?;
    let next = super::assess(root, &plan.request, plan.evidence.as_ref())?;
    if next.repository != plan.repository
        || next.settings != plan.settings
        || next.source != plan.source
        || next.local_target != plan.local_target
        || next.snapshot.config != plan.snapshot.config
        || !plan
            .snapshot
            .configuration_files
            .matches_checkout(root, &next.repository, "HEAD")
            .map_err(SaveError::from)?
        || next.record != plan.record
        || current_ref(&next.snapshot.state).as_deref() != Some(reference)
    {
        return Err(FinishError::ContextChanged);
    }
    guard(root, &next)?;
    Ok(next)
}
fn push(
    root: &Path,
    plan: &Plan,
    endpoint: &[u8],
    reference: &[u8],
    expected: &RefObservation,
    oid: &[u8],
    commands: &mut Vec<CommandReport>,
) -> Result<(), FinishError> {
    pending_routes(root, plan, commands)?;
    let current = observe(root, plan, endpoint, reference, commands)?;
    if current == RefObservation::Commit(oid.to_vec()) {
        return Ok(());
    }
    if &current != expected {
        return Err(invalid(
            "release destination changed; retained lease is not renewed",
        ));
    }
    let old = match expected {
        RefObservation::Absent => Vec::new(),
        RefObservation::Commit(oid) => oid.clone(),
        _ => return Err(invalid("unavailable release destination")),
    };
    let (mut args, name) = remote::endpoint(root, plan, &plan.target.remote, endpoint, commands)
        .map_err(|_| invalid("release endpoint unavailable"))?;
    args.extend(
        [
            "--no-replace-objects",
            "push",
            "--porcelain",
            "--no-force",
            "--no-follow-tags",
            "--recurse-submodules=no",
        ]
        .iter()
        .map(OsString::from),
    );
    args.extend([
        repository::os(&[b"--force-with-lease=", reference, b":", &old].concat())?,
        "--".into(),
        name.into(),
        repository::os(&[oid, b":", reference].concat())?,
    ]);
    guard(root, plan)?;
    remote::command(root, args, commands)
        .map_err(|_| invalid("release ref push failed; retain partial effects and retry"))?;
    if observe(root, plan, endpoint, reference, commands)? != RefObservation::Commit(oid.to_vec()) {
        return Err(invalid("release delivery is unconfirmed"));
    }
    Ok(())
}
/// Recheck release-specific destinations before each pending target push. A
/// conflicting version or marker must not inherit earlier target authority.
pub(super) fn pending_routes(
    root: &Path,
    plan: &Plan,
    commands: &mut Vec<CommandReport>,
) -> Result<(), FinishError> {
    let Some(record) = plan.record.as_ref().and_then(|r| r.release.as_ref()) else {
        return Ok(());
    };
    guard(root, plan)?;
    let tag = verify_tag(root, plan, record, true)?.ok_or(invalid("version tag missing"))?;
    let mut endpoints = vec![plan.target.fetch_destination.clone()];
    for endpoint in &plan.target.push_destinations {
        if !endpoints.contains(endpoint) {
            endpoints.push(endpoint.clone());
        }
    }
    for endpoint in &endpoints {
        let value = observe(root, plan, endpoint, &record.reference(), commands)?;
        if value != RefObservation::Absent && value != RefObservation::Commit(tag.clone()) {
            return Err(invalid("version tag collision at a project destination"));
        }
    }
    if let Some(marker) = &record.marker {
        for destination in marker
            .destinations
            .as_ref()
            .ok_or(invalid("marker expectations missing"))?
        {
            let value = observe(
                root,
                plan,
                &destination.destination,
                &marker.reference,
                commands,
            )?;
            if value != destination.branch && value != RefObservation::Commit(record.source.clone())
            {
                return Err(invalid("published release marker changed"));
            }
        }
    }
    Ok(())
}
/// Deliver the exact version tag and enabled marker after verified integration
/// delivery. Every endpoint is freshly reobserved again by completion.
pub fn deliver(
    root: &Path,
    initial: &Plan,
    skip: bool,
    commands: &mut Vec<CommandReport>,
) -> Result<Plan, FinishError> {
    deliver_with_caches(root, initial, skip, commands, &mut vec![])
}

/// Deliver the selected release refs and report independent local cache issues.
pub fn deliver_with_caches(
    root: &Path,
    initial: &Plan,
    skip: bool,
    commands: &mut Vec<CommandReport>,
    caches: &mut Vec<crate::save::publication::CacheReport>,
) -> Result<Plan, FinishError> {
    let record = initial.record.as_ref().and_then(|r| r.release.as_ref());
    let prepared = record.map(|record| {
        let tag = crate::tracking::cache::Prepared::new(
            root,
            &initial.target.remote,
            &record.reference(),
        );
        let marker = record.marker.as_ref().map(|marker| {
            crate::tracking::cache::Prepared::new(root, &initial.target.remote, &marker.reference)
        });
        (tag, marker)
    });
    let result = deliver_inner(root, initial, skip, commands);
    if let Some((tag, marker)) = prepared
        && let Some(record) = record
    {
        let observations = delivery_observations(initial, commands);
        let desired_tag = record.tag_object.as_deref();
        let tags = observations
            .iter()
            .filter(|d| {
                desired_tag.is_some_and(|oid| d.tag == RefObservation::Commit(oid.to_vec()))
            })
            .map(|d| (d.destination.clone(), None))
            .collect::<Vec<_>>();
        if !tags.is_empty() {
            caches.push(tag.refresh(root, &tags, desired_tag))
        }
        if let Some(cache) = marker
            && let Some(marker) = &record.marker
        {
            let markers = observations
                .iter()
                .filter(|d| d.marker == Some(RefObservation::Commit(record.source.clone())))
                .map(|d| {
                    (
                        d.destination.clone(),
                        marker
                            .destinations
                            .as_ref()
                            .and_then(|ds| ds.iter().find(|old| old.destination == d.destination))
                            .and_then(|d| commit(&d.branch).map(<[u8]>::to_vec)),
                    )
                })
                .collect::<Vec<_>>();
            if !markers.is_empty() {
                caches.push(cache.refresh(root, &markers, Some(&record.source)))
            }
        }
    }
    if let Ok(plan) = &result {
        // Cache hooks must preserve the returned plan. Release readiness is
        // checked by delivery itself; ordinary plans can retain cleanup blockers.
        revalidate(root, plan, plan.evidence.as_ref())?
    }
    result
}

fn deliver_inner(
    root: &Path,
    initial: &Plan,
    skip: bool,
    commands: &mut Vec<CommandReport>,
) -> Result<Plan, FinishError> {
    let mut plan = initial.clone();
    if plan.has_remaining_destinations() {
        return Err(invalid("integration destinations remain pending"));
    }
    super::sequence::verify_completed_delivery(root, &plan, commands)?;
    let Some(r) = plan.record.as_ref().and_then(|r| r.release.clone()) else {
        return Ok(plan);
    };
    guard(root, &plan)?;
    let tag = verify_tag(root, &plan, &r, true)?.ok_or(invalid("version tag missing"))?;
    if plan.source == RefObservation::Absent {
        verify_delivery(root, &plan, commands)?;
        return Ok(plan);
    }
    pending_routes(root, &plan, commands)?;
    if !candidate_checked(&plan, skip) {
        return Err(invalid("integration candidate has not passed its checks"));
    }
    for endpoint in &plan.target.push_destinations {
        if observe(root, &plan, endpoint, &plan.target.reference, commands)?
            != RefObservation::Commit(plan.result.clone().ok_or(invalid("result missing"))?)
        {
            return Err(invalid("integration delivery is incomplete"));
        }
    }
    for endpoint in &plan.target.push_destinations {
        push(
            root,
            &plan,
            endpoint,
            &r.reference(),
            &RefObservation::Absent,
            &tag,
            commands,
        )?;
    }
    if let Some(marker) = &r.marker {
        let destinations = marker
            .destinations
            .as_ref()
            .ok_or(invalid("marker expectations missing"))?;
        for destination in destinations {
            push(
                root,
                &plan,
                &destination.destination,
                &marker.reference,
                &destination.branch,
                &r.source,
                commands,
            )?;
        }
        let current = hint(root, &marker.reference)?;
        if current != RefObservation::Commit(r.source.clone()) {
            if current != marker.local_before {
                return Err(invalid("local marker changed"));
            }
            guard(root, &plan)?;
            let old = commit(&current)
                .map(<[u8]>::to_vec)
                .unwrap_or_else(|| vec![b'0'; r.source.len()]);
            let args = vec![
                "update-ref".into(),
                "--no-deref".into(),
                "--create-reflog".into(),
                "-m".into(),
                r.action.clone().into(),
                repository::os(&marker.reference)?,
                repository::os(&r.source)?,
                repository::os(&old)?,
            ];
            remote::command(root, args, commands)
                .map_err(|_| invalid("local marker update failed"))?;
            plan = assess_after(root, &plan)?;
            guard(root, &plan)?;
        }
    }
    verify_delivery(root, &plan, commands)?;
    Ok(plan)
}
pub(super) fn verify_delivery(
    root: &Path,
    plan: &Plan,
    commands: &mut Vec<CommandReport>,
) -> Result<(), FinishError> {
    let Some(r) = plan.record.as_ref().and_then(|r| r.release.as_ref()) else {
        return Ok(());
    };
    let tag = verify_tag(root, plan, r, true)?.ok_or(invalid("version tag missing"))?;
    for endpoint in &plan.target.push_destinations {
        if observe(root, plan, endpoint, &r.reference(), commands)?
            != RefObservation::Commit(tag.clone())
        {
            return Err(invalid("version tag delivery is incomplete"));
        }
        if let Some(marker) = &r.marker {
            if observe(root, plan, endpoint, &marker.reference, commands)?
                != RefObservation::Commit(r.source.clone())
            {
                return Err(invalid("release marker delivery is incomplete"));
            }
            if hint(root, &marker.reference)? != RefObservation::Commit(r.source.clone()) {
                return Err(invalid("local release marker is incomplete"));
            }
        }
    }
    Ok(())
}

/// Endpoint observations collected during delivery, including uncertain command
/// outcomes. These are presentation data; completion reobserves every ref.
#[derive(Debug, Clone)]
pub struct DestinationStatus {
    pub destination: Vec<u8>,
    pub tag: RefObservation,
    pub marker: Option<RefObservation>,
}
pub fn delivery_observations(plan: &Plan, commands: &[CommandReport]) -> Vec<DestinationStatus> {
    let Some(record) = plan.record.as_ref().and_then(|r| r.release.as_ref()) else {
        return vec![];
    };
    let mut destinations: Vec<_> = plan
        .target
        .push_destinations
        .iter()
        .map(|destination| DestinationStatus {
            destination: destination.clone(),
            tag: RefObservation::Unavailable,
            marker: record.marker.as_ref().map(|_| RefObservation::Unavailable),
        })
        .collect();
    let tag_ref = record.reference();
    for command in commands {
        let args: Vec<_> = command
            .arguments
            .iter()
            .map(|arg| arg.as_encoded_bytes())
            .collect();
        let Some(reference) = args.last() else {
            continue;
        };
        let push = args.contains(&b"push".as_slice());
        let observe =
            args.contains(&b"ls-remote".as_slice()) && args.contains(&b"--refs".as_slice());
        if !push && !observe {
            continue;
        }
        for destination in &mut destinations {
            if !args
                .iter()
                .any(|arg| arg.ends_with(&[b".url=".as_slice(), &destination.destination].concat()))
            {
                continue;
            }
            for (selected, slot) in std::iter::once((tag_ref.as_slice(), &mut destination.tag))
                .chain(
                    record
                        .marker
                        .as_ref()
                        .zip(destination.marker.as_mut())
                        .map(|(marker, value)| (marker.reference.as_slice(), value)),
                )
            {
                if (observe && *reference == selected)
                    || (push && reference.ends_with(&[b":", selected].concat()))
                {
                    *slot = if observe {
                        command
                            .result
                            .as_ref()
                            .ok()
                            .filter(|r| r.status.success())
                            .and_then(|r| {
                                crate::finish::preparation::parse_advertisement(
                                    &r.stdout,
                                    selected,
                                    record.source.len(),
                                )
                                .ok()
                            })
                            .unwrap_or(RefObservation::Unavailable)
                    } else {
                        RefObservation::Unavailable
                    };
                }
            }
        }
    }
    destinations
}
