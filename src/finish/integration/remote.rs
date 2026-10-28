//! Explicit exact-ref observation and contained object acquisition.
use super::*;
use crate::{finish::preparation::CommandReport, git};
use std::{ffi::OsString, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Observed {
        destination: Vec<u8>,
        reference: Vec<u8>,
        value: RefObservation,
    },
    Fetched {
        destination: Vec<u8>,
        commit: Vec<u8>,
    },
}
#[derive(Debug)]
pub enum Error {
    Context(FinishError),
    CommandFailed,
    EndpointChanged,
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
            Self::CommandFailed => {
                f.write_str("Git transport failed; inspect its output and retain completed effects")
            }
            Self::EndpointChanged => {
                f.write_str("the selected endpoint resolves differently; reassess before transport")
            }
        }
    }
}
impl std::error::Error for Error {}
#[derive(Debug)]
pub struct Report {
    /// Complete evidence is exposed only after all required observations and fetches.
    pub evidence: Option<RemoteEvidence>,
    pub plan: Option<Plan>,
    pub commands: Vec<CommandReport>,
    pub effects: Vec<Effect>,
    pub remaining: Result<repository::Observation, repository::InspectionError>,
    pub error: Option<Error>,
}

pub(super) fn command(
    root: &Path,
    args: Vec<OsString>,
    commands: &mut Vec<CommandReport>,
) -> Result<Vec<u8>, Error> {
    let result = git::workflow(root, &args);
    let success = result.as_ref().is_ok_and(|o| o.status.success());
    let bytes = result
        .as_ref()
        .map(|o| o.stdout.clone())
        .unwrap_or_default();
    commands.push(CommandReport {
        arguments: args,
        result,
    });
    if success {
        Ok(bytes)
    } else {
        Err(Error::CommandFailed)
    }
}

pub(super) fn endpoint(
    root: &Path,
    plan: &Plan,
    remote: &str,
    destination: &[u8],
    commands: &mut Vec<CommandReport>,
) -> Result<(Vec<OsString>, String), Error> {
    let (prefix, name) =
        crate::finish::transport::endpoint(&plan.snapshot.config, remote, destination)?;
    let mut args = prefix.clone();
    args.extend(
        ["ls-remote", "--get-url", "--", &name]
            .iter()
            .map(OsString::from),
    );
    let resolved = command(root, args, commands)?;
    if resolved.strip_suffix(b"\n") != Some(destination) {
        return Err(Error::EndpointChanged);
    }
    Ok((prefix, name))
}

pub(super) fn observe(
    root: &Path,
    prefix: &[OsString],
    name: &str,
    reference: &[u8],
    oid_length: usize,
    commands: &mut Vec<CommandReport>,
) -> Result<RefObservation, Error> {
    let mut args = prefix.to_vec();
    args.extend([
        "ls-remote".into(),
        "--refs".into(),
        "--heads".into(),
        "--".into(),
        name.into(),
        repository::os(reference)?,
    ]);
    let bytes = command(root, args, commands)?;
    Ok(crate::finish::preparation::parse_advertisement(
        &bytes, reference, oid_length,
    )?)
}

pub(super) fn acquire(
    root: &Path,
    plan: &Plan,
    remote: &str,
    destination: &[u8],
    reference: &[u8],
    report: &mut Report,
) -> Result<RefObservation, Error> {
    let (prefix, name) = endpoint(root, plan, remote, destination, &mut report.commands)?;
    let length = commit(&plan.source)
        .or_else(|| plan.record.as_ref().map(|r| r.target_before.as_slice()))
        .ok_or(FinishError::InvalidEvidence("missing source object ID"))?
        .len();
    let value = observe(
        root,
        &prefix,
        &name,
        reference,
        length,
        &mut report.commands,
    )?;
    report.effects.push(Effect::Observed {
        destination: destination.to_vec(),
        reference: reference.to_vec(),
        value: value.clone(),
    });
    if let RefObservation::Commit(oid) = &value
        && !report
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Fetched { commit, .. } if commit == oid))
    {
        // Pin the advertisement's object, not a moving branch name. An endpoint
        // refusing that OID stops the operation; it cannot substitute a new base.
        let mut args = prefix;
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
        command(root, args, &mut report.commands)?;
        report.effects.push(Effect::Fetched {
            destination: destination.to_vec(),
            commit: oid.clone(),
        });
    }
    Ok(value)
}

/// Observe the configured target and acquire the exact advertised objects without
/// writing branches, tracking refs, FETCH_HEAD, the index, or the working tree.
/// The input may be an initial assessment without remote evidence or choices.
/// A refreshed plan can remain blocked (for example by disagreeing endpoints).
pub fn refresh(directory: impl AsRef<Path>, plan: &Plan) -> Report {
    let directory = directory.as_ref();
    let root = plan
        .repository
        .working_tree_root
        .as_deref()
        .unwrap_or(directory);
    let mut report = Report {
        evidence: None,
        plan: None,
        commands: vec![],
        effects: vec![],
        remaining: Ok(repository::Observation::Repository(Box::new(
            plan.snapshot.state.clone(),
        ))),
        error: None,
    };
    report.error = (|| {
        revalidate(directory, plan, plan.evidence.as_ref())?;
        let fetched = acquire(
            root,
            plan,
            &plan.target.remote,
            &plan.target.fetch_destination,
            &plan.target.reference,
            &mut report,
        )?;
        let mut evidence = RemoteEvidence {
            target: plan.target.clone(),
            fetched,
            preparation: None,
            targets: vec![],
            work_remote: plan.settings.publish_remote.clone(),
            work_ref: plan.source_ref.clone(),
            work: vec![],
        };
        if let Some(reference) = preparation_ref(plan) {
            let branch = acquire(
                root,
                plan,
                &plan.target.remote,
                &plan.target.fetch_destination,
                &reference,
                &mut report,
            )?;
            evidence.preparation = Some(PreparationEvidence { reference, branch });
        }
        for destination in &plan.target.push_destinations {
            let branch = if destination == &plan.target.fetch_destination {
                evidence.fetched.clone()
            } else {
                acquire(
                    root,
                    plan,
                    &plan.target.remote,
                    destination,
                    &plan.target.reference,
                    &mut report,
                )?
            };
            evidence.targets.push(DestinationEvidence {
                destination: destination.clone(),
                branch,
            });
        }
        if plan.cleanup == FinishCleanup::Both
            || (release::enabled(plan) && plan.request.update == Some(FinishUpdate::Rebase))
        {
            for destination in urls(root, &plan.settings.publish_remote, true)? {
                let branch = acquire(
                    root,
                    plan,
                    &plan.settings.publish_remote,
                    &destination,
                    &plan.source_ref,
                    &mut report,
                )?;
                evidence.work.push(DestinationEvidence {
                    destination,
                    branch,
                });
            }
        }
        // Fetch imports objects without changing workflow refs or policy. Reuse
        // its acknowledgements and refresh the graph only at this stage exit.
        report.plan = Some(refresh_acquired(root, plan, &evidence)?);
        report.evidence = Some(evidence);
        Ok::<_, Error>(())
    })()
    .err();
    // Object-only fetch has no branch, checkout or index effect. Its successful
    // acknowledgement supplies the next graph's evidence; transport failures
    // retain the last known local stage rather than inventing a new snapshot.
    if report.error.is_some() {
        report.remaining = repository::inspect(root);
    } else if let Some(next) = &report.plan {
        report.remaining = Ok(repository::Observation::Repository(Box::new(
            next.snapshot.state.clone(),
        )));
    }
    report
}
