//! Local preparation and recoverable target integration. No publication or cleanup.
use super::*;
use crate::{
    finish::{checks, preparation::CommandReport},
    git,
};
use std::{ffi::OsString, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Recorded(PathBuf),
    Retained {
        reference: String,
        commit: Vec<u8>,
    },
    WorkChanged {
        before: RefObservation,
        after: RefObservation,
    },
    TargetChanged {
        before: RefObservation,
        after: RefObservation,
    },
    CheckedOut {
        reference: Vec<u8>,
    },
    Integrated {
        commit: Vec<u8>,
    },
    Reused {
        commit: Vec<u8>,
    },
}
#[derive(Debug)]
pub enum Error {
    Context(FinishError),
    UnresolvedPlan,
    CommandFailed,
    RemoteChanged,
    UnverifiedResult,
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
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(e) => write!(f, "{e}"),
            Self::UnresolvedPlan => {
                f.write_str("local integration requires resolved input and satisfied constraints")
            }
            Self::CommandFailed => {
                f.write_str("Git stopped; inspect its output and remaining state")
            }
            Self::RemoteChanged => {
                f.write_str("target observations changed; reassess before continuing")
            }
            Self::UnverifiedResult => {
                f.write_str("the local result could not be verified; retain the current Git state")
            }
        }
    }
}
impl std::error::Error for Error {}
#[derive(Debug)]
pub struct Report {
    pub plan: Option<Plan>,
    pub commands: Vec<CommandReport>,
    pub effects: Vec<Effect>,
    pub remaining: Result<repository::Observation, repository::InspectionError>,
    pub error: Option<Error>,
}
impl Report {
    pub fn is_prepared(&self) -> bool {
        self.error.is_none()
            && self
                .plan
                .as_ref()
                .is_some_and(|p| p.is_ready() && p.result.is_some())
    }
    pub fn guidance(&self) -> String {
        if let Ok(repository::Observation::Repository(state)) = &self.remaining {
            if state.operations.rebase {
                return "Complete the stopped edit or resolve and stage conflicts, then use git rebase --continue, or use git rebase --abort; reassess finish afterward.".into();
            }
            if state.operations.merge
                && self
                    .plan
                    .as_ref()
                    .is_some_and(|p| p.result.is_some() && p.state != State::Fresh)
            {
                return "The integration commit is verified, and Git still has merge state. Use git merge --quit to clear that state while retaining the commit, then reassess finish.".into();
            }
            if state.operations.merge {
                return "Resolve and stage conflicts, then use git merge --continue, or use git merge --abort; reassess finish afterward.".into();
            }
        }
        if let Some(plan) = &self.plan
            && plan.state == State::IncompleteOperation
            && plan.method == FinishIntegration::Squash
            && let Some(record) = &plan.record
        {
            return format!(
                "Squash has no merge continuation or abort state. Resolve and stage the result, then commit with GIT_REFLOG_ACTION='{}' git commit -m <message>. To abandon the tracked squash changes, from the working-tree root use git restore --source={} --staged --worktree -- . after retaining any edits you need, and remove Git's SQUASH_MSG file; reassess finish afterward.",
                record.marker(),
                String::from_utf8_lossy(&record.target_before)
            );
        }
        if self.is_prepared() {
            "The local integration result is retained. Run candidate checks before publication; publication and completion have not been attempted.".into()
        } else {
            "Inspect the reported Git output and remaining state, then reassess. Completed effects and retained work remain available.".into()
        }
    }
}
fn command(
    root: &Path,
    args: Vec<OsString>,
    marker: Option<&str>,
    report: &mut Report,
) -> Result<Vec<u8>, Error> {
    let result = if args.iter().any(|a| a == "--interactive") {
        crate::finish::history::execute(root, &args, marker)
    } else {
        git::workflow_with_reflog(root, &args, marker)
    };
    let success = result.as_ref().is_ok_and(|o| o.status.success());
    let bytes = result
        .as_ref()
        .map(|o| o.stdout.clone())
        .unwrap_or_default();
    report.commands.push(CommandReport {
        arguments: args,
        result,
    });
    if success {
        Ok(bytes)
    } else {
        Err(Error::CommandFailed)
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn guard(root: &Path, plan: &Plan) -> Result<(), Error> {
    revalidate_stage(root, plan, plan.evidence.as_ref())?;
    if !plan.is_ready() {
        return Err(Error::UnresolvedPlan);
    }
    Ok(())
}
fn fresh(root: &Path, previous: &Plan) -> Result<Plan, Error> {
    fresh_after(root, previous, false)
}
fn fresh_after(root: &Path, previous: &Plan, staged_squash: bool) -> Result<Plan, Error> {
    Ok(refresh_local(root, previous, staged_squash)?)
}
// Private writes acknowledge facts already established by this stage. They do
// not change Git's graph, selected policy, or the checked-out candidate.
fn recorded(root: &Path, plan: &mut Plan, record: &recovery::Record) -> Result<(), Error> {
    let bytes = serde_json::to_vec(record)
        .map_err(std::io::Error::other)
        .map_err(FinishError::from)?;
    *plan
        .snapshot
        .records
        .last_mut()
        .ok_or(Error::UnverifiedResult)? = Some(bytes);
    plan.resolve_logs(
        root,
        &[
            record.original_ref().as_bytes(),
            record.result_ref().as_bytes(),
        ],
    )?;
    let prior_release_refs = release::binding_refs(plan);
    plan.record = Some(record.clone());
    if release::binding_refs(plan) != prior_release_refs {
        plan.snapshot.release = release::snapshot_from_plan(root, plan)?;
    }
    Ok(())
}
fn selection_written(_root: &Path, plan: &mut Plan) -> Result<(), Error> {
    recovery::bind_selection(plan)?;
    Ok(())
}
fn observe(root: &Path, plan: &Plan, report: &mut Report) -> Result<(), Error> {
    guard(root, plan)?;
    sequence::verify_completed_delivery(root, plan, &mut report.commands)?;
    let e = plan.evidence.as_ref().ok_or(Error::UnresolvedPlan)?;
    for (url, expected) in std::iter::once((&plan.target.fetch_destination, &e.fetched))
        .chain(e.targets.iter().map(|d| (&d.destination, &d.branch)))
    {
        let mut a = args(&["ls-remote", "--get-url", "--"]);
        a.push(repository::os(url)?);
        if command(root, a, None, report)?.strip_suffix(b"\n") != Some(url.as_slice()) {
            return Err(Error::RemoteChanged);
        }
        let mut a = args(&["ls-remote", "--refs", "--heads", "--"]);
        a.extend([
            repository::os(url)?,
            repository::os(&plan.target.reference)?,
        ]);
        let output = command(root, a, None, report)?;
        let value = crate::finish::preparation::parse_advertisement(
            &output,
            &plan.target.reference,
            commit(&plan.source).ok_or(Error::UnresolvedPlan)?.len(),
        )?;
        if &value != expected {
            return Err(Error::RemoteChanged);
        }
    }
    if let Some(prep) = &e.preparation {
        let (prefix, name) = remote::endpoint(
            root,
            plan,
            &plan.target.remote,
            &plan.target.fetch_destination,
            &mut report.commands,
        )
        .map_err(|_| Error::RemoteChanged)?;
        let value = remote::observe(
            root,
            &prefix,
            &name,
            &prep.reference,
            commit(&plan.source).ok_or(Error::UnresolvedPlan)?.len(),
            &mut report.commands,
        )
        .map_err(|_| Error::RemoteChanged)?;
        if value != prep.branch {
            return Err(Error::RemoteChanged);
        }
    }
    if release::enabled(plan)
        && plan.integration == Integration::AfterPreparation
        && plan.update == FinishUpdate::Rebase
    {
        for work in &e.work {
            let (prefix, name) = remote::endpoint(
                root,
                plan,
                &plan.settings.publish_remote,
                &work.destination,
                &mut report.commands,
            )
            .map_err(|_| Error::RemoteChanged)?;
            let observed = remote::observe(
                root,
                &prefix,
                &name,
                &plan.source_ref,
                commit(&plan.source).ok_or(Error::UnresolvedPlan)?.len(),
                &mut report.commands,
            )
            .map_err(|_| Error::RemoteChanged)?;
            if observed != work.branch {
                return Err(Error::RemoteChanged);
            }
        }
    }
    Ok(())
}
fn retain(root: &Path, reference: &str, oid: &[u8], report: &mut Report) -> Result<(), Error> {
    let existing = hint(root, reference.as_bytes())?;
    if existing == RefObservation::Commit(oid.to_vec()) {
        return Ok(());
    }
    if existing != RefObservation::Absent {
        return Err(Error::UnverifiedResult);
    }
    let mut a = args(&[
        "update-ref",
        "--no-deref",
        "--create-reflog",
        "-m",
        "twigsmith retain",
        reference,
    ]);
    a.extend([
        repository::os(oid)?,
        repository::os(&vec![b'0'; oid.len()])?,
    ]);
    let outcome = command(root, a, None, report);
    if outcome.is_ok() || hint(root, reference.as_bytes())? == RefObservation::Commit(oid.to_vec())
    {
        report.effects.push(Effect::Retained {
            reference: reference.into(),
            commit: oid.to_vec(),
        });
    }
    outcome?;
    Ok(())
}
fn execute(
    root: &Path,
    approved: &Plan,
    report: &mut Report,
    skip: bool,
    event: &mut impl FnMut(checks::Event<'_>),
) -> Result<(), Error> {
    if !approved.is_ready() {
        return Err(Error::UnresolvedPlan);
    }
    let mut plan = approved.clone();
    let mut record = if let Some(record) = &plan.record {
        record.clone()
    } else {
        // tempfile supplies a collision-resistant token without global mutable state.
        let token_file = tempfile::Builder::new()
            .prefix("attempt-")
            .tempfile_in(&plan.repository.git_dir)
            .map_err(FinishError::from)?;
        let token = token_file
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace('.', "-");
        let record = recovery::Record::for_plan(&plan, &token)?;
        let path = plan.recovery_path.as_ref().ok_or(Error::UnresolvedPlan)?;
        recovery::write(&plan.repository.git_dir, path, &record, false)?;
        report.effects.push(Effect::Recorded(path.clone()));
        recorded(root, &mut plan, &record)?;
        record
    };
    retain(root, &record.original_ref(), record.original(), report)?;
    plan.snapshot.pins = vec![
        (
            RefObservation::Commit(record.original().to_vec()),
            plan.log(record.original_ref().as_bytes())?,
        ),
        (
            hint(root, record.result_ref().as_bytes())?,
            plan.log(record.result_ref().as_bytes())?,
        ),
    ];
    if !record.retained {
        record.retained = true;
        recovery::write(
            &plan.repository.git_dir,
            plan.recovery_path.as_ref().unwrap(),
            &record,
            true,
        )?;
        recorded(root, &mut plan, &record)?;
    }
    plan = release::prepare(root, &plan, skip, event, &mut report.commands)?;
    record = plan.record.clone().ok_or(Error::UnverifiedResult)?;
    if plan.result.is_some() && plan.state != State::Fresh {
        return finish_result(root, plan, report, skip, event);
    }
    let selection = recovery::Selection::observe(
        root,
        std::str::from_utf8(&plan.source_ref[b"refs/heads/".len()..])
            .map_err(|_| FinishError::InvalidEvidence("non-UTF-8 source"))?,
    )?;
    recovery::write(
        &plan.repository.git_dir,
        &recovery::selection_path(&plan.repository),
        &selection,
        true,
    )?;
    selection_written(root, &mut plan)?;
    if plan.integration == Integration::AfterPreparation {
        if plan.history_edit == HistoryEdit::Interactive {
            crate::finish::history::editor_ready(root)?;
        }
        if plan.update == FinishUpdate::Rebase {
            crate::finish::history::protect_ignored(
                root,
                record.original(),
                record.preparation_base(),
            )?;
        }
        let mut a = args(&[
            "--no-replace-objects",
            "-c",
            "core.logAllRefUpdates=true",
            "-c",
            "submodule.recurse=false",
        ]);
        a.extend(args(match plan.update {
            FinishUpdate::Rebase => &[
                "rebase",
                "--rebase-merges=no-rebase-cousins",
                "--no-update-refs",
                "--no-autostash",
                "--no-autosquash",
                "--no-fork-point",
            ],
            FinishUpdate::Merge => &[
                "merge",
                "--no-edit",
                "--no-squash",
                "--commit",
                "--ff",
                "--no-autostash",
            ],
            _ => return Err(Error::UnresolvedPlan),
        }));
        if plan.history_edit == HistoryEdit::Interactive {
            a.push("--interactive".into());
        }
        a.push(repository::os(record.preparation_base())?);
        // Retaining /original invokes reference-transaction hooks. Rebase and
        // merge act on the current checkout, so a pinned base does not protect
        // the approved source, routes or attempt from changes made by that hook.
        // Validate those bindings before the first history mutation.
        guard(root, &plan)?;
        command(root, a, Some(&record.prepare_marker()), report)?;
        plan = fresh(root, &plan)?;
        report.plan = Some(plan.clone());
        if !plan.is_ready() {
            return Err(Error::UnresolvedPlan);
        }
        if plan.history_edit == HistoryEdit::Interactive {
            observe(root, &plan, report)?;
        }
    }
    plan = release::prepare(root, &plan, skip, event, &mut report.commands)?;
    record = plan.record.clone().ok_or(Error::UnverifiedResult)?;
    if !plan.is_ready() {
        return Err(Error::UnresolvedPlan);
    }
    // Persist the corroborated prepared source and resolved message before target effects.
    record.bind_choices(&plan);
    recovery::write(
        &plan.repository.git_dir,
        plan.recovery_path.as_ref().unwrap(),
        &record,
        true,
    )?;
    recorded(root, &mut plan, &record)?;
    // Advance only the approved local target version; never replace unrelated work.
    if plan.local_target != RefObservation::Commit(record.target_before.clone()) {
        if current_ref(&plan.snapshot.state).as_ref() == Some(&plan.target.reference) {
            // Recovery can start with the target already checked out. Let Git
            // advance its index/worktree together with the ref in this case.
            let mut a = args(&[
                "--no-replace-objects",
                "-c",
                "core.logAllRefUpdates=true",
                "-c",
                "submodule.recurse=false",
                "merge",
                "--ff-only",
                "--no-squash",
                "--no-autostash",
            ]);
            a.push(repository::os(&record.target_before)?);
            command(root, a, Some(&record.base_marker()), report)?;
        } else {
            // Retention invokes reference-transaction hooks. Before plumbing
            // advances a branch, refresh its stage binding: update-ref's lease
            // protects the old OID, but cannot protect an occupied worktree or
            // establish that the selected source/candidate still owns this stage.
            guard(root, &plan)?;
            let old = commit(&plan.local_target)
                .map(<[u8]>::to_vec)
                .unwrap_or_else(|| vec![b'0'; record.target_before.len()]);
            let mut a = args(&[
                "update-ref",
                "--no-deref",
                "--create-reflog",
                "-m",
                &record.base_marker(),
            ]);
            a.extend([
                repository::os(&plan.target.reference)?,
                repository::os(&record.target_before)?,
                repository::os(&old)?,
            ]);
            command(root, a, None, report)?;
            // This conditional update acknowledged the selected base. Carry it
            // into the checkout stage rather than reassessing its unchanged
            // integration graph. Switch enforces occupancy and its hook exit is
            // followed by the local candidate/configuration refresh below.
            plan.local_target = RefObservation::Commit(record.target_before.clone());
            plan.snapshot.target_log = plan.log(&plan.target.reference)?;
        }
        if current_ref(&plan.snapshot.state).as_ref() == Some(&plan.target.reference) {
            plan = fresh(root, &plan)?;
        }
    }
    if current_ref(&plan.snapshot.state).as_ref() != Some(&plan.target.reference) {
        let mut a = args(&[
            "-c",
            "submodule.recurse=false",
            "switch",
            "--no-guess",
            "--no-overwrite-ignore",
            "--",
        ]);
        a.push(repository::os(
            &plan.target.reference[b"refs/heads/".len()..],
        )?);
        command(root, a, None, report)?;
        plan = fresh(root, &plan)?;
    }
    if !plan.is_ready() {
        return Err(Error::UnresolvedPlan);
    }
    if plan.result.is_some() && plan.state != State::Fresh {
        return finish_result(root, plan, report, skip, event);
    }
    let message = plan
        .request
        .message
        .clone()
        .unwrap_or_else(|| plan.message_suggestion());
    let mut a = args(&[
        "--no-replace-objects",
        "-c",
        "core.logAllRefUpdates=true",
        "-c",
        "submodule.recurse=false",
        "merge",
        "--no-edit",
        "--no-autostash",
    ]);
    match plan.integration {
        Integration::FastForward => a.extend(args(&["--ff-only", "--no-squash"])),
        Integration::MergeCommit => a.extend(args(&[
            "--no-ff",
            "--no-squash",
            "--commit",
            "-m",
            &message,
        ])),
        Integration::SquashCommit => a.extend(args(&["--squash", "--ff"])),
        Integration::AlreadyIntegrated => return finish_result(root, plan, report, skip, event),
        _ => return Err(Error::UnresolvedPlan),
    }
    crate::finish::history::protect_ignored(root, &record.source, &record.target_before)?;
    a.push(repository::os(&record.source)?);
    command(root, a, Some(&record.marker()), report)?;
    if plan.integration == Integration::SquashCommit {
        // Squash intentionally leaves a dirty index and no MERGE_HEAD. Revalidate
        // all stable bindings, target and source before committing that index.
        let pending = fresh_after(root, &plan, true)?;
        if pending.source != plan.source
            || pending.local_target != plan.local_target
            || pending.snapshot.state.operations.is_active()
            || current_ref(&pending.snapshot.state) != current_ref(&plan.snapshot.state)
            || pending
                .blockers
                .iter()
                .any(|b| !matches!(b, Blocker::DirtyTree | Blocker::GitOperation))
        {
            return Err(Error::UnverifiedResult);
        }
        let a = args(&[
            "-c",
            "core.logAllRefUpdates=true",
            "commit",
            "--no-edit",
            "-m",
            &message,
        ]);
        command(root, a, Some(&record.marker()), report)?;
    }
    let plan = fresh(root, &plan)?;
    finish_result(root, plan, report, skip, event)
}
fn finish_result(
    root: &Path,
    mut plan: Plan,
    report: &mut Report,
    skip: bool,
    event: &mut impl FnMut(checks::Event<'_>),
) -> Result<(), Error> {
    let result = plan.result.clone().ok_or(Error::UnverifiedResult)?;
    let mut record = plan.record.clone().ok_or(Error::UnverifiedResult)?;
    retain(root, &record.result_ref(), &result, report)?;
    if plan.snapshot.pins.len() != 2 {
        return Err(Error::UnverifiedResult);
    }
    plan.snapshot.pins[1] = (
        RefObservation::Commit(result.clone()),
        plan.log(record.result_ref().as_bytes())?,
    );
    if record.result.is_none() {
        record.result = Some(result);
        recovery::write(
            &plan.repository.git_dir,
            plan.recovery_path.as_ref().unwrap(),
            &record,
            true,
        )?;
        recorded(root, &mut plan, &record)?;
    }
    if (!matches!(plan.state, State::Published | State::Complete)
        || (release::enabled(&plan) && !release::candidate_checked(&plan, false)))
        && current_ref(&plan.snapshot.state).as_ref() != Some(&plan.target.reference)
    {
        let selection = recovery::Selection::observe(
            root,
            std::str::from_utf8(&plan.source_ref[b"refs/heads/".len()..])
                .map_err(|_| FinishError::InvalidEvidence("non-UTF-8 source"))?,
        )?;
        recovery::write(
            &plan.repository.git_dir,
            &recovery::selection_path(&plan.repository),
            &selection,
            true,
        )?;
        selection_written(root, &mut plan)?;
        let mut a = args(&[
            "-c",
            "submodule.recurse=false",
            "switch",
            "--no-guess",
            "--no-overwrite-ignore",
            "--",
        ]);
        a.push(repository::os(
            &plan.target.reference[b"refs/heads/".len()..],
        )?);
        command(root, a, None, report)?;
        plan = fresh(root, &plan)?;
    }
    plan = release::prepare(root, &plan, skip, event, &mut report.commands)?;
    report.plan = Some(plan);
    Ok(())
}
/// Prepare or resume verified local work. Tagged work first checks and tags its
/// exact source; candidate checks follow integration. No publication or cleanup.
pub fn prepare(directory: impl AsRef<Path>, plan: &Plan) -> Report {
    prepare_with_checks(directory, plan, false, |_| {})
}

pub fn prepare_with_checks(
    directory: impl AsRef<Path>,
    plan: &Plan,
    skip: bool,
    mut event: impl FnMut(checks::Event<'_>),
) -> Report {
    let directory = directory.as_ref();
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    let mut report = Report {
        plan: Some(plan.clone()),
        commands: vec![],
        effects: vec![],
        remaining: Ok(repository::Observation::Repository(Box::new(
            plan.snapshot.state.clone(),
        ))),
        error: None,
    };
    let validated = revalidate(directory, plan, plan.evidence.as_ref());
    let approved = validated.is_ok();
    report.error = validated
        .map_err(Error::from)
        .and_then(|()| execute(root, plan, &mut report, skip, &mut event))
        .err();
    report.remaining = repository::inspect(root);
    // Ref observations remain reportable even if a hook invalidates settings
    // and prevents full assessment. These are observed changes, not success flags.
    if approved {
        if let Ok(after) = hint(root, &plan.source_ref)
            && after != plan.source
        {
            report.effects.push(Effect::WorkChanged {
                before: plan.source.clone(),
                after,
            });
        }
        if let Ok(after) = hint(root, &plan.target.reference)
            && after != plan.local_target
        {
            report.effects.push(Effect::TargetChanged {
                before: plan.local_target.clone(),
                after,
            });
        }
        if let Ok(repository::Observation::Repository(state)) = &report.remaining
            && current_ref(state) != current_ref(&plan.snapshot.state)
            && let Some(reference) = current_ref(state)
        {
            report.effects.push(Effect::CheckedOut { reference });
        }
    }
    // Normal success already carries the stage result. Only a failed Git/hook
    // boundary needs recovery assessment to identify an unacknowledged commit.
    if report.error.is_some() {
        report.plan = assess(root, &plan.request, plan.evidence.as_ref()).ok();
        if let Some(after) = report.plan.clone()
            && let Some(commit) = &after.result
            && approved
            && after.state != State::Fresh
            && after
                .blockers
                .iter()
                .all(|b| matches!(b, Blocker::DirtyTree | Blocker::GitOperation))
            && let Some(record) = &after.record
        {
            let _ = retain(root, &record.result_ref(), commit, &mut report);
        }
    }
    if let Some(after) = &report.plan
        && let Some(commit) = &after.result
        && approved
        && after.state != State::Fresh
    {
        report.effects.push(
            if plan.result.as_ref() == Some(commit) && plan.state != State::Fresh {
                Effect::Reused {
                    commit: commit.clone(),
                }
            } else {
                Effect::Integrated {
                    commit: commit.clone(),
                }
            },
        );
    }
    report
}
#[derive(Debug)]
pub struct CheckReport {
    pub checks: checks::Report,
    pub remaining: Result<repository::Observation, repository::InspectionError>,
    /// Reassessment can itself fail when a check changes repository selection,
    /// configuration, or recovery evidence. Check success never grants authority.
    pub plan: Result<Plan, FinishError>,
}
/// Inspect the actual checked-out candidate, with the shared process lifecycle
/// and presentation events. Tagged work retains exact-commit check receipts;
/// each use still requires complete context and immutable-result corroboration.
pub fn check(
    directory: impl AsRef<Path>,
    plan: &Plan,
    skip: bool,
    event: impl FnMut(checks::Event<'_>),
) -> CheckReport {
    let directory = directory.as_ref();
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    let effective_checks = if release::candidate_checked(plan, skip) {
        &[][..]
    } else {
        &plan.settings.finish_checks[..]
    };
    let mut entry = true;
    let mut checks = checks::run_guarded(root, effective_checks, skip, event, || {
        if entry {
            revalidate(directory, plan, plan.evidence.as_ref())?;
            entry = false;
        } else {
            revalidate_stage(root, plan, plan.evidence.as_ref())?;
        }
        let head = match &plan.snapshot.state.head {
            repository::Head::Attached { commit, .. } | repository::Head::Detached { commit } => {
                Some(commit)
            }
            _ => None,
        };
        if !plan.is_ready()
            || plan.state == State::Fresh
            || plan.result.as_ref() != head
            || plan.result.is_none()
        {
            return Err(checks::Error::Unprepared);
        }
        Ok(())
    });
    let next = if checks.error.is_none() && release::enabled(plan) {
        release::record_candidate(plan, skip)
    } else if checks.error.is_none() && effective_checks.is_empty() {
        // No process or callback ran: the entry binding remains current.
        Ok(plan.clone())
    } else {
        refresh_local(root, plan, false)
    };
    if next.is_err() && checks.error.is_none() {
        checks.error = Some(checks::Error::Context(FinishError::ContextChanged));
    }
    let remaining = if checks.error.is_none() {
        let next = next
            .as_ref()
            .expect("successful checks have a refreshed candidate");
        Ok(repository::Observation::Repository(Box::new(
            next.snapshot.state.clone(),
        )))
    } else {
        // Checks are arbitrary processes: a failure can leave edits or a new
        // checkout even when the refreshed candidate binding is rejected.
        repository::inspect(root)
    };
    CheckReport {
        checks,
        remaining,
        plan: next,
    }
}
