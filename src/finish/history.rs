//! Native history editing shares Git's editors; no alternate todo language.
use super::*;
use std::{
    io::IsTerminal,
    process::{Command, Output, Stdio},
};

pub(super) fn unavailable(root: &Path) -> Result<Option<String>, FinishError> {
    if metadata(root, "shallow")?.is_some_and(|b| !b.is_empty()) {
        return Ok(Some(
            "interactive history editing requires complete history".into(),
        ));
    }
    let replacements = read(
        root,
        &["for-each-ref", "--format=%(refname)", "refs/replace/"],
    )?;
    let namespace = std::env::var("GIT_REPLACE_REF_BASE").ok();
    let custom = namespace
        .as_deref()
        .map(|r| read(root, &["for-each-ref", "--format=%(refname)", r]))
        .transpose()?;
    if !replacements.is_empty() || custom.is_some_and(|b| !b.is_empty()) {
        return Ok(Some(
            "interactive history editing requires history without replacement refs".into(),
        ));
    }
    Ok(None)
}
pub(super) fn unique_boundary(
    root: &Path,
    source: &[u8],
    base: &[u8],
) -> Result<bool, FinishError> {
    let output = run(
        root,
        &[
            "--no-replace-objects".into(),
            "merge-base".into(),
            "--all".into(),
            repository::os(source)?,
            repository::os(base)?,
        ],
    )?;
    Ok(output.status.success()
        && output
            .stdout
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .count()
            == 1)
}
fn configured(root: &Path, key: &str) -> bool {
    run(root, &["config".into(), "--get".into(), key.into()])
        .is_ok_and(|o| o.status.success() && !o.stdout.iter().all(u8::is_ascii_whitespace))
}
fn environment(key: &str) -> bool {
    std::env::var_os(key).is_some_and(|v| !v.is_empty())
}
pub(super) fn editor_ready(root: &Path) -> Result<(), FinishError> {
    if io::stdin().is_terminal() && io::stderr().is_terminal() {
        return Ok(());
    }
    let commit = environment("GIT_EDITOR")
        || configured(root, "core.editor")
        || environment("VISUAL")
        || environment("EDITOR");
    let sequence =
        environment("GIT_SEQUENCE_EDITOR") || configured(root, "sequence.editor") || commit;
    if !sequence || !commit {
        return Err(FinishError::InvalidEvidence(
            "unattended interactive history editing requires explicit sequence and commit editors; configure GIT_SEQUENCE_EDITOR and GIT_EDITOR (or Git editor settings)",
        ));
    }
    Ok(())
}
pub(super) fn execute(root: &Path, args: &[OsString], action: Option<&str>) -> io::Result<Output> {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args(["--no-lazy-fetch", "-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped());
    if io::stdin().is_terminal() && io::stderr().is_terminal() {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        command.stderr(Stdio::piped());
    }
    if let Some(action) = action {
        command.env("GIT_REFLOG_ACTION", action);
    }
    command.output()
}

/// The sequencer's intermediate checkouts can replace ignored paths even when
/// the original work tip no longer tracks them. Inspect every replay tree.
pub(super) fn protect_ignored(root: &Path, source: &[u8], base: &[u8]) -> Result<(), FinishError> {
    let ignored = read(
        root,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
    )?;
    if ignored.is_empty() {
        return Ok(());
    }
    let range = [base, b"..", source].concat();
    let commits = run(
        root,
        &[
            "--no-replace-objects".into(),
            "rev-list".into(),
            repository::os(&range)?,
        ],
    )?;
    if !commits.status.success() {
        return Err(FinishError::InvalidEvidence(
            "history editing replay trees are unavailable",
        ));
    }
    for commit in std::iter::once(base).chain(
        commits
            .stdout
            .split(|b| *b == b'\n')
            .filter(|c| !c.is_empty()),
    ) {
        let tree = run(
            root,
            &[
                "--no-replace-objects".into(),
                "ls-tree".into(),
                "-r".into(),
                "--name-only".into(),
                "-z".into(),
                repository::os(commit)?,
            ],
        )?;
        if !tree.status.success() {
            return Err(FinishError::InvalidEvidence(
                "history editing replay tree is unavailable",
            ));
        }
        for local in ignored.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            if tree
                .stdout
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .any(|tracked| {
                    local == tracked
                        || local.starts_with(&[tracked, b"/"].concat())
                        || tracked.starts_with(&[local, b"/"].concat())
                })
            {
                return Err(FinishError::InvalidEvidence(
                    "interactive history editing would overwrite an ignored local path; preserve or move it before retrying",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn shares_target_work(
    root: &Path,
    source: &[u8],
    base: &[u8],
    target: &[u8],
) -> Result<bool, FinishError> {
    let common = run(
        root,
        &[
            "--no-replace-objects".into(),
            "merge-base".into(),
            "--all".into(),
            repository::os(source)?,
            repository::os(target)?,
        ],
    )?;
    if !common.status.success() {
        return Ok(true);
    }
    for oid in common
        .stdout
        .split(|b| *b == b'\n')
        .filter(|c| !c.is_empty())
    {
        if ancestor(root, oid, base)? != Some(true) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) type SharedRefs = Vec<(String, RefObservation, Option<Vec<u8>>)>;
pub(super) fn shared_refs(root: &Path, settings: &Settings) -> Result<SharedRefs, FinishError> {
    let prep = settings
        .workstream
        .as_ref()
        .map_or(settings.base.as_str(), |p| p.preparation_source.as_str());
    let target = settings
        .workstream
        .as_ref()
        .map_or(settings.base.as_str(), |p| p.integration_target.as_str());
    let mut refs = vec![];
    for name in [prep, target] {
        let reference = format!("refs/heads/{name}");
        if refs.iter().any(|(r, _, _)| r == &reference) {
            continue;
        }
        if run(
            root,
            &[
                "symbolic-ref".into(),
                "--quiet".into(),
                reference.clone().into(),
            ],
        )?
        .status
        .success()
        {
            return Err(FinishError::InvalidEvidence(
                "interactive history editing requires direct shared preparation and integration refs",
            ));
        }
        let oid = hint(root, reference.as_bytes())?;
        let log = metadata(root, &format!("logs/{reference}"))?;
        refs.push((reference, oid, log));
    }
    Ok(refs)
}
