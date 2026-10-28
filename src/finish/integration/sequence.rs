//! Ordered destinations share one immutable prepared source and release identity.
//! Only a verified delivery can move the retained attempt to its next target.
use super::*;
use crate::finish::preparation::CommandReport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Completed {
    pub target: Target,
    pub result: Vec<u8>,
    log: recovery::LogVersion,
    pin: String,
}
fn invalid(reason: &'static str) -> FinishError {
    FinishError::InvalidEvidence(reason)
}

pub(super) fn resolve_logs(root: &Path, plan: &mut Plan) -> Result<(), FinishError> {
    let references: Vec<_> = plan.record.as_ref().map_or_else(Vec::new, |record| {
        record
            .completed
            .iter()
            .flat_map(|done| [done.target.reference.clone(), done.pin.as_bytes().to_vec()])
            .collect()
    });
    plan.resolve_logs(
        root,
        &references.iter().map(Vec::as_slice).collect::<Vec<_>>(),
    )
}
pub(super) fn assess_completed(root: &Path, plan: &mut Plan) -> Result<(), FinishError> {
    let Some(record) = &plan.record else {
        return Ok(());
    };
    let refs = destination_refs(&plan.settings);
    if record.completed.len() >= refs.len()
        || record.target.reference != refs[record.completed.len()]
    {
        plan.blockers
            .push(Blocker::Recovery(recovery::Problem::ChangedBinding));
        return Ok(());
    }
    for (index, done) in record.completed.iter().enumerate() {
        let pin_log = plan.log(done.pin.as_bytes())?;
        let entries: Vec<_> = pin_log
            .as_deref()
            .unwrap_or_default()
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        let prefix = [
            vec![b'0'; done.result.len()],
            b" ".to_vec(),
            done.result.clone(),
            b" ".to_vec(),
        ]
        .concat();
        if done.target.reference != refs[index]
            || done.target.remote != plan.target.remote
            || done.target.fetch_destination != plan.target.fetch_destination
            || done.target.push_destinations != plan.target.push_destinations
            || symbolic(root, &done.target.reference)?
            || hint(root, &done.target.reference)? != RefObservation::Commit(done.result.clone())
            || recovery::LogVersion::of(plan.log(&done.target.reference)?.as_deref()) != done.log
            || symbolic(root, done.pin.as_bytes())?
            || hint(root, done.pin.as_bytes())? != RefObservation::Commit(done.result.clone())
            || entries.len() != 1
            || !entries[0].starts_with(&prefix)
            || entries[0].split(|b| *b == b'\t').nth(1) != Some(b"twigsmith retain")
        {
            plan.blockers
                .push(Blocker::Recovery(recovery::Problem::ChangedTarget));
        }
    }
    // The first destination is the final checkout even after later integrations.
    if plan.checkout == FinishCheckout::Target
        && !plan.has_remaining_destinations()
        && in_another_worktree(plan, &refs[0])
    {
        plan.blockers.push(Blocker::TargetInAnotherWorktree);
    }
    Ok(())
}

pub(super) fn verify_completed_delivery(
    root: &Path,
    plan: &Plan,
    commands: &mut Vec<CommandReport>,
) -> Result<(), FinishError> {
    let Some(record) = plan.record.as_ref().filter(|r| !r.completed.is_empty()) else {
        return Ok(());
    };
    if !plan.is_ready() {
        return Err(invalid("ordered integration plan is unresolved"));
    }
    for done in &record.completed {
        for destination in &done.target.push_destinations {
            let (prefix, name) =
                remote::endpoint(root, plan, &done.target.remote, destination, commands)
                    .map_err(|_| invalid("completed destination endpoint changed"))?;
            let value = remote::observe(
                root,
                &prefix,
                &name,
                &done.target.reference,
                done.result.len(),
                commands,
            )
            .map_err(|_| invalid("completed destination observation unavailable"))?;
            if value != RefObservation::Commit(done.result.clone()) {
                return Err(invalid(
                    "a previously completed integration destination changed; retain work and inspect delivery",
                ));
            }
        }
    }
    Ok(())
}

/// Move to the next configured destination after the current result is checked,
/// retained and delivered. The same prepared source remains bound to the attempt.
/// Transport observations are reported even when acquisition or validation fails.
pub fn advance(
    root: &Path,
    plan: &Plan,
    commands: &mut Vec<CommandReport>,
) -> Result<Plan, FinishError> {
    revalidate(root, plan, plan.evidence.as_ref())?;
    if !plan.is_ready()
        || !plan.has_remaining_destinations()
        || !matches!(plan.state, State::Published | State::Complete)
        || !release::candidate_checked(plan, true)
    {
        return Err(invalid(
            "advancing requires a checked, delivered destination and pending configured destinations",
        ));
    }
    verify_completed_delivery(root, plan, commands)?;
    release::pending_routes(root, plan, commands)?;
    let result = plan
        .result
        .as_ref()
        .ok_or(invalid("integration result missing"))?;
    let mut record = plan
        .record
        .clone()
        .ok_or(invalid("integration record missing"))?;
    if record.result.as_ref() != Some(result) {
        return Err(invalid("result retention is incomplete"));
    }
    for endpoint in &plan.target.push_destinations {
        let (prefix, name) = remote::endpoint(root, plan, &plan.target.remote, endpoint, commands)
            .map_err(|_| invalid("target endpoint changed"))?;
        let observed = remote::observe(
            root,
            &prefix,
            &name,
            &plan.target.reference,
            result.len(),
            commands,
        )
        .map_err(|_| invalid("target delivery unavailable"))?;
        if observed != RefObservation::Commit(result.clone()) {
            return Err(invalid("current integration delivery is incomplete"));
        }
    }
    let reference = destination_refs(&plan.settings)[record.completed.len() + 1].clone();
    let mut report = remote::Report {
        evidence: None,
        plan: None,
        commands: vec![],
        effects: vec![],
        remaining: repository::inspect(root),
        error: None,
    };
    let acquired = (|| {
        let fetched = remote::acquire(
            root,
            plan,
            &plan.target.remote,
            &plan.target.fetch_destination,
            &reference,
            &mut report,
        )
        .map_err(|_| invalid("next integration destination fetch failed"))?;
        let mut evidence = plan
            .evidence
            .clone()
            .ok_or(invalid("target evidence missing"))?;
        evidence.target.reference = reference.clone();
        evidence.fetched = fetched;
        evidence.preparation = None;
        evidence.targets.clear();
        for endpoint in &plan.target.push_destinations {
            let branch = remote::acquire(
                root,
                plan,
                &plan.target.remote,
                endpoint,
                &reference,
                &mut report,
            )
            .map_err(|_| invalid("next integration destination observation failed"))?;
            evidence.targets.push(DestinationEvidence {
                destination: endpoint.clone(),
                branch,
            });
        }
        Ok::<_, FinishError>(evidence)
    })();
    commands.extend(report.commands);
    let evidence = acquired?;
    record.completed.push(Completed {
        target: plan.target.clone(),
        result: result.clone(),
        log: recovery::LogVersion::of(plan.snapshot.target_log.as_deref()),
        pin: record.result_ref(),
    });
    record.target = evidence.target.clone();
    record.target_before = commit(&evidence.fetched)
        .ok_or(invalid("next integration target is missing"))?
        .to_vec();
    record.local_before = hint(root, &reference)?;
    let mut next = plan.clone();
    let next_pin = record.result_ref();
    next.resolve_logs(root, &[&reference, next_pin.as_bytes()])?;
    let target_log = next.log(&reference)?;
    record.target_version = recovery::LogVersion::of(target_log.as_deref());
    record.result = None;
    release::next_destination(&mut record);
    next.target = record.target.clone();
    next.local_target = record.local_before.clone();
    next.snapshot.target_log = target_log;
    next.record = Some(record.clone());
    next.evidence = Some(evidence.clone());
    next.result = None;
    next.state = State::Fresh;
    next.update = FinishUpdate::None;
    next.integration = Integration::Unresolved;
    next.effects.clear();
    next.required_input.clear();
    next.blockers.clear();
    if symbolic(root, &reference)? {
        return Err(invalid("next integration target is symbolic"));
    }
    if in_another_worktree(&next, &reference) {
        return Err(invalid(
            "next integration target is checked out in another worktree",
        ));
    }
    assess_fresh(root, &mut next)?;
    if !next.blockers.is_empty() {
        return Err(invalid(
            "next integration destination is unavailable, changed, or incompatible with the selected integration method",
        ));
    }
    // Input may be needed on the next graph; binding that destination is not
    // authority to integrate until the normal planner resolves those choices.
    recovery::write(
        &plan.repository.git_dir,
        plan.recovery_path
            .as_ref()
            .ok_or(invalid("attempt path missing"))?,
        &record,
        true,
    )?;
    *next
        .snapshot
        .records
        .last_mut()
        .ok_or(invalid("attempt snapshot missing"))? =
        Some(serde_json::to_vec(&record).map_err(std::io::Error::other)?);
    refresh_local(root, &next, false)
}
