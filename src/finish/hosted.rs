//! Preparation of tagged work for manual hosted review. The private attempt ends
//! at verified publication; it contains no external submission or cleanup state.
use super::*;
use crate::settings::{self, FinishCheckout, FinishCleanup, FinishIntegration};
use serde::{Deserialize, Serialize};
use std::{io::Write, path::PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub release: integration::release::Request,
    pub merge_back: Option<String>,
    pub update: Option<FinishUpdate>,
    pub history_edit: Option<HistoryEdit>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Route {
    remote: String,
    endpoint: Vec<u8>,
    reference: Vec<u8>,
    before: RefObservation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    source_branch: Vec<u8>,
    source: Vec<u8>,
    source_log: Vec<u8>,
    head_log: Vec<u8>,
    settings: Settings,
    config: Vec<u8>,
    committed_settings: Vec<u8>,
    release: integration::release::Request,
    merge_back: String,
    update: FinishUpdate,
    token: String,
    target: Route,
    target_oid: Vec<u8>,
    target_retained: bool,
    preparation_log: Option<Vec<u8>>,
    preparation_retained: bool,
    routes: Vec<Route>,
    tag_object: Option<Vec<u8>>,
    branch_created: bool,
    candidate: Option<Vec<u8>>,
    source_checked: bool,
    candidate_checked: Option<Vec<u8>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub repository: repository::RepositoryContext,
    pub settings: Settings,
    pub source_branch: Vec<u8>,
    pub source: Vec<u8>,
    pub request: Request,
    pub update: FinishUpdate,
    pub tag: Option<String>,
    pub merge_back: Option<String>,
    record: Option<Record>,
    state: repository::RepositoryState,
    config: Vec<u8>,
    committed_settings: Vec<u8>,
    source_log: Vec<u8>,
    head_log: Vec<u8>,
    branch_log: Option<Vec<u8>>,
    tag_log: Option<Vec<u8>>,
    tag_object: RefObservation,
    candidate: RefObservation,
    worktrees: Vec<u8>,
    record_bytes: Option<Vec<u8>>,
    association: Option<Vec<u8>>,
}
#[derive(Debug, Default)]
pub struct Report {
    pub local_tag: Option<RefObservation>,
    pub local_candidate: Option<RefObservation>,
    pub publications: Vec<PublicationStatus>,
    pub caches: Vec<save::publication::CacheReport>,
    pub plan: Option<Plan>,
    pub commands: Vec<preparation::CommandReport>,
    pub error: Option<FinishError>,
    pub ready: bool,
}
#[derive(Debug)]
pub struct PublicationStatus {
    pub remote: String,
    pub destination: Vec<u8>,
    pub reference: Vec<u8>,
    pub observed: RefObservation,
    pub expected: Option<Vec<u8>>,
}
pub enum Event<'a> {
    Source(&'a [u8]),
    Candidate(&'a [u8]),
    Check(checks::Event<'a>),
}
impl Plan {
    pub fn tag_object(&self) -> &RefObservation {
        &self.tag_object
    }
    pub fn candidate(&self) -> &RefObservation {
        &self.candidate
    }
    pub fn is_retry(&self) -> bool {
        self.record.is_some()
    }
}
fn invalid(message: &'static str) -> FinishError {
    FinishError::InvalidEvidence(message)
}
fn branch_ref(branch: &[u8]) -> Vec<u8> {
    [b"refs/heads/", branch].concat()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn path(repo: &repository::RepositoryContext, branch: &[u8], association: bool) -> PathBuf {
    repo.git_dir
        .join("twigsmith-hosted")
        .join(if association {
            "associations"
        } else {
            "attempts"
        })
        .join(hex(branch))
}
fn private_read(
    repo: &repository::RepositoryContext,
    path: &Path,
) -> Result<Option<Vec<u8>>, FinishError> {
    let mut current = repo.git_dir.clone();
    for component in path
        .strip_prefix(&repo.git_dir)
        .map_err(|_| invalid("unsafe hosted record path"))?
        .components()
    {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(invalid("unsafe hosted record path"));
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(invalid("unsafe hosted recovery symlink"));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o077 != 0 {
                        return Err(invalid("hosted recovery evidence must be private"));
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Some(fs::read(path)?))
}
fn write_private(
    repo: &repository::RepositoryContext,
    destination: &Path,
    bytes: &[u8],
    expected: Option<&[u8]>,
) -> Result<(), FinishError> {
    if private_read(repo, destination)?.as_deref() != expected {
        return Err(FinishError::ContextChanged);
    }
    let parent = destination
        .parent()
        .ok_or(invalid("missing recovery parent"))?;
    let mut dir = repo.git_dir.clone();
    for component in parent
        .strip_prefix(&repo.git_dir)
        .map_err(|_| invalid("unsafe recovery directory"))?
        .components()
    {
        dir.push(component);
        if !dir.exists() {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&dir)?;
        }
    }
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    if private_read(repo, destination)?.as_deref() != expected {
        return Err(FinishError::ContextChanged);
    }
    file.persist(destination)
        .map_err(|e| FinishError::from(e.error))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
fn log(root: &Path, reference: &[u8]) -> Result<Option<Vec<u8>>, FinishError> {
    let name =
        std::str::from_utf8(reference).map_err(|_| invalid("hosted refs require UTF-8 names"))?;
    metadata(root, &format!("logs/{name}"))
}
fn urls(root: &Path, remote: &str, push: bool) -> Result<Vec<Vec<u8>>, FinishError> {
    let mut args = vec!["remote", "get-url"];
    if push {
        args.extend(["--push", "--all"]);
    }
    args.push(remote);
    let bytes = read(root, &args)?;
    let values: Vec<_> = bytes
        .strip_suffix(b"\n")
        .unwrap_or_default()
        .split(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    if values.iter().any(|v| v.is_empty() || v.contains(&b'\r')) {
        return Err(invalid("ambiguous hosted endpoint"));
    }
    Ok(values)
}
fn symbolic(root: &Path, reference: &[u8]) -> Result<bool, FinishError> {
    Ok(run(
        root,
        &[
            "symbolic-ref".into(),
            "--quiet".into(),
            repository::os(reference)?,
        ],
    )?
    .status
    .code()
        != Some(1))
}
fn name(root: &Path, prefix: &str, value: &str) -> Result<(), FinishError> {
    if value.starts_with('-')
        || !run(
            root,
            &["check-ref-format".into(), format!("{prefix}{value}").into()],
        )?
        .status
        .success()
    {
        return Err(invalid("invalid explicit hosted reference name"));
    }
    Ok(())
}
fn commit(value: &RefObservation) -> Result<Vec<u8>, FinishError> {
    match value {
        RefObservation::Commit(oid) => Ok(oid.clone()),
        _ => Err(invalid("required hosted ref is unavailable")),
    }
}
fn clean(state: &repository::RepositoryState) -> Result<(), FinishError> {
    if state.operations.is_active() {
        return Err(invalid(
            "continue the merge with git merge --continue, or abandon preparation with git merge --abort, then retry finish; preserve the released tag",
        ));
    }
    if state
        .working_tree
        .as_ref()
        .is_none_or(|tree| tree.is_dirty())
    {
        return Err(FinishError::DirtyTree);
    }
    Ok(())
}
/// Read-only selection, including retries while checked out on the owned merge-back branch.
/// No branch prefix establishes policy or recovery ownership.
pub fn assess(root: &Path, supplied: &Request) -> Result<Plan, FinishError> {
    let repo = repository::discover(root)?.ok_or(SaveError::OutsideRepository)?;
    let repository::Observation::Repository(state) = repository::inspect_without_upstream(root)?
    else {
        return Err(SaveError::OutsideRepository.into());
    };
    let repository::Head::Attached { branch, .. } = &state.head else {
        return Err(SaveError::DetachedHead.into());
    };
    settings::legacy_branch_guard(root, branch).map_err(SaveError::from)?;
    let association = private_read(&repo, &path(&repo, branch, true))?;
    if association.is_none() && hosted_branch(root, branch)? {
        return Err(invalid(
            "owned merge-back association is missing; restore the hosted attempt evidence before retrying finish",
        ));
    }
    let source_branch = association.clone().unwrap_or_else(|| branch.clone());
    let record_bytes = private_read(&repo, &path(&repo, &source_branch, false))?;
    let record: Option<Record> = record_bytes
        .as_ref()
        .map(|b| serde_json::from_slice(b).map_err(|_| invalid("malformed hosted attempt")))
        .transpose()?;
    if association.is_some() && record.is_none() {
        return Err(invalid(
            "hosted attempt missing for this merge-back association",
        ));
    }
    let settings =
        settings::load_for_branch(root, &repo, &source_branch).map_err(SaveError::from)?;
    if supplied.history_edit.unwrap_or(settings.history_edit) == HistoryEdit::Interactive {
        return Err(invalid(
            "interactive history editing is unavailable for tagged hosted preparation",
        ));
    }
    if !settings.workstream.as_ref().is_some_and(|p| {
        p.version_tagging && p.version_tag_identity == settings::VersionTagIdentity::WorkSource
    }) {
        return Err(invalid(
            "hosted finish requires explicit tagged workstream policy",
        ));
    }
    if settings
        .workstream
        .as_ref()
        .is_some_and(|p| p.integration_targets.len() != 1)
    {
        return Err(invalid(
            "review finish supports one integration destination",
        ));
    }
    let request = if let Some(r) = &record {
        let request = Request {
            release: integration::release::Request {
                tag: supplied
                    .release
                    .tag
                    .clone()
                    .or_else(|| r.release.tag.clone()),
                message: supplied
                    .release
                    .message
                    .clone()
                    .or_else(|| r.release.message.clone()),
                sign: supplied.release.sign || r.release.sign,
            },
            merge_back: supplied
                .merge_back
                .clone()
                .or_else(|| Some(r.merge_back.clone())),
            update: supplied.update.or(Some(r.update)),
            history_edit: supplied.history_edit,
        };
        if request.release != r.release
            || request.merge_back.as_ref() != Some(&r.merge_back)
            || request.update != Some(r.update)
        {
            return Err(invalid(
                "explicit hosted choices conflict with the retained attempt",
            ));
        }
        request
    } else {
        supplied.clone()
    };
    let tag = request.release.tag.clone();
    let merge_back = request.merge_back.clone().or_else(|| {
        settings
            .workstream
            .as_ref()
            .and_then(|p| p.merge_back_branch.clone())
    });
    let update = request.update.unwrap_or(settings.finish_update);
    if update == FinishUpdate::Rebase {
        return Err(invalid(
            "tagged review preparation supports merge or none; preserve released history",
        ));
    }
    if let Some(tag) = &tag {
        name(root, "refs/tags/", tag)?;
    }
    if let Some(back) = &merge_back {
        name(root, "refs/heads/", back)?;
        if back.as_bytes() == source_branch
            || back == &settings.base
            || settings
                .workstream
                .as_ref()
                .and_then(|p| p.release_marker.as_ref())
                == Some(back)
        {
            return Err(invalid(
                "merge-back branch must differ from source, target, and release marker",
            ));
        }
    }
    if request
        .release
        .message
        .as_ref()
        .is_some_and(|m| m.trim().is_empty() || m.contains('\0'))
    {
        return Err(invalid("tag annotation must be nonblank text"));
    }
    let source_ref = branch_ref(&source_branch);
    if symbolic(root, &source_ref)? {
        return Err(invalid("symbolic hosted source is unsupported"));
    }
    let source = commit(&hint(root, &source_ref)?)?;
    let config = read(root, &["config", "--null", "--list"])?;
    let mut config_args = vec!["ls-tree", "--full-tree", "-z", "HEAD", "--"];
    config_args.extend(settings::CONFIG_FILES);
    let committed_settings = read(root, &config_args)?;
    let source_log = log(root, &source_ref)?.ok_or(invalid("source branch reflog is required"))?;
    let head_log = metadata(root, "logs/HEAD")?.ok_or(invalid("HEAD reflog is required"))?;
    let mut plan = Plan {
        repository: repo,
        settings,
        source_branch,
        source,
        request,
        update,
        tag,
        merge_back,
        record,
        state: *state,
        config,
        committed_settings,
        source_log,
        head_log,
        branch_log: None,
        tag_log: None,
        tag_object: RefObservation::Absent,
        candidate: RefObservation::Absent,
        worktrees: read(root, &["worktree", "list", "--porcelain", "-z"])?,
        record_bytes,
        association,
    };
    original_history(root)?;
    if let Some(back) = &plan.merge_back {
        let reference = branch_ref(back.as_bytes());
        if symbolic(root, &reference)? {
            return Err(invalid("symbolic merge-back branch is unsupported"));
        }
        plan.candidate = hint(root, &reference)?;
        plan.branch_log = log(root, &reference)?;
    }
    if let Some(tag) = &plan.tag {
        let reference = [b"refs/tags/", tag.as_bytes()].concat();
        if symbolic(root, &reference)? {
            return Err(invalid("symbolic version tag is unsupported"));
        }
        plan.tag_object = hint(root, &reference)?;
        plan.tag_log = log(root, &reference)?;
    }
    check_worktrees(&plan)?;
    if let Some(record) = &plan.record {
        verify_record(root, &plan, record)?;
    } else if plan.candidate != RefObservation::Absent
        || plan.branch_log.is_some()
        || plan.tag_object != RefObservation::Absent
        || plan.tag_log.is_some()
    {
        return Err(invalid(
            "tag or merge-back branch collision; choose unused explicit names",
        ));
    }
    Ok(plan)
}
// Version-source checks require Git's original complete graph. Checks and hooks
// can introduce these overrides, so stage validation retains this narrow rule.
fn original_history(root: &Path) -> Result<(), FinishError> {
    let replacements = read(
        root,
        &[
            "--no-replace-objects",
            "for-each-ref",
            "--format=%(refname)",
            "--",
            std::env::var("GIT_REPLACE_REF_BASE")
                .as_deref()
                .unwrap_or("refs/replace/"),
        ],
    )?;
    if !replacements.is_empty()
        || metadata(root, "info/grafts")?.is_some_and(|v| !v.is_empty())
        || metadata(root, "shallow")?.is_some_and(|v| !v.is_empty())
    {
        return Err(invalid(
            "hosted version checks require original, complete Git history without replacements or grafts",
        ));
    }
    Ok(())
}
fn check_worktrees(plan: &Plan) -> Result<(), FinishError> {
    let source = branch_ref(&plan.source_branch);
    let back = plan.merge_back.as_ref().map(|b| branch_ref(b.as_bytes()));
    let root = plan
        .repository
        .working_tree_root
        .as_ref()
        .ok_or(SaveError::BareRepository)?;
    let mut current = None;
    for item in plan.worktrees.split(|b| *b == 0) {
        if let Some(path) = item.strip_prefix(b"worktree ") {
            current = Some(path);
        }
        if let Some(branch) = item.strip_prefix(b"branch ")
            && (branch == source || back.as_deref() == Some(branch))
            && current != Some(root.as_os_str().as_encoded_bytes())
        {
            return Err(invalid(
                "source or merge-back branch is occupied in another worktree",
            ));
        }
    }
    Ok(())
}
fn entries(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .collect()
}
fn transition(entry: &[u8], old: &[u8], new: &[u8]) -> bool {
    entry.starts_with(&[old, b" ", new, b" "].concat())
}
fn verify_record(root: &Path, plan: &Plan, r: &Record) -> Result<(), FinishError> {
    if r.version != 1
        || plan.repository.working_tree_root.as_ref() != Some(&r.root)
        || plan.repository.git_dir != r.git_dir
        || plan.repository.common_dir != r.common_dir
        || plan.source_branch != r.source_branch
        || plan.source != r.source
        || plan.source_log != r.source_log
        || plan.settings != r.settings
        || route_config(&plan.config, &hosted_roles(plan), &plan.source_branch)
            != route_config(&r.config, &hosted_roles(plan), &plan.source_branch)
        || plan.committed_settings != r.committed_settings
        || !plan.head_log.starts_with(&r.head_log)
    {
        return Err(invalid(
            "hosted source, branch instance, settings, configuration, or worktree changed",
        ));
    }
    let policy = plan
        .settings
        .workstream
        .as_ref()
        .ok_or(invalid("missing hosted policy"))?;
    if r.token.is_empty()
        || !r
            .token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || r.target.remote != policy.base_remote
        || r.target.reference != branch_ref(policy.integration_target.as_bytes())
    {
        return Err(invalid("hosted target or attempt identity is inconsistent"));
    }
    if r.target_retained || hint(root, pin_ref(r, "target").as_bytes())? != RefObservation::Absent {
        verify_pin(root, r, "target", &r.target_oid)?;
    }
    if read(
        root,
        &[
            "--no-replace-objects",
            "cat-file",
            "-t",
            std::str::from_utf8(&r.target_oid).map_err(|_| invalid("invalid retained target"))?,
        ],
    )? != b"commit\n"
    {
        return Err(invalid("retained hosted target object is missing"));
    }
    if r.preparation_retained {
        verify_pin(root, r, "prepare", &r.source)?;
    }
    let branch = match &plan.state.head {
        repository::Head::Attached { branch, .. } => branch,
        _ => return Err(invalid("hosted HEAD is unassociated")),
    };
    if branch != &r.source_branch && branch != r.merge_back.as_bytes() {
        return Err(invalid("hosted HEAD is unassociated"));
    }
    if branch == r.merge_back.as_bytes() && plan.association.as_ref() != Some(&r.source_branch) {
        return Err(invalid("merge-back association missing"));
    }
    if plan.tag_object != RefObservation::Absent || r.tag_object.is_some() {
        let oid = commit(&plan.tag_object)?;
        let lines = entries(plan.tag_log.as_deref().unwrap_or_default());
        let action = format!("twigsmith-hosted-tag:{} (", r.token);
        if r.tag_object
            .as_ref()
            .is_some_and(|expected| expected != &oid)
            || lines.len() != 1
            || !transition(lines[0], &vec![b'0'; oid.len()], &oid)
            || !lines[0]
                .split(|b| *b == b'\t')
                .nth(1)
                .is_some_and(|m| m.starts_with(action.as_bytes()))
        {
            return Err(invalid(
                "version tag creation/object is not corroborated by this attempt",
            ));
        }
        let tag = r
            .release
            .tag
            .as_ref()
            .ok_or(invalid("missing retained tag name"))?;
        if hint(root, format!("refs/tags/{tag}^{{commit}}").as_bytes())?
            != RefObservation::Commit(r.source.clone())
        {
            return Err(invalid(
                "tag does not identify the original released source",
            ));
        }
    } else if plan.tag_log.is_some() {
        return Err(invalid("unexplained version-tag reflog"));
    }
    if plan.candidate == RefObservation::Absent {
        if r.branch_created || r.candidate.is_some() || plan.branch_log.is_some() {
            return Err(invalid("owned merge-back branch or evidence is missing"));
        }
    } else {
        let oid = commit(&plan.candidate)?;
        let lines: Vec<_> = entries(plan.branch_log.as_deref().unwrap_or_default())
            .into_iter()
            .filter(|entry| {
                !(transition(entry, &r.source, &r.source)
                    && entry.split(|b| *b == b'\t').nth(1).is_some_and(|m| {
                        m == b"reset: moving to HEAD" || m == b"reset: moving to ORIG_HEAD"
                    }))
            })
            .collect();
        let action = format!("twigsmith-hosted-branch:{}", r.token);
        if lines.is_empty()
            || lines.len() > 2
            || !transition(lines[0], &vec![b'0'; r.source.len()], &r.source)
            || lines[0].split(|b| *b == b'\t').nth(1) != Some(action.as_bytes())
        {
            return Err(invalid(
                "merge-back branch creation/instance is not corroborated",
            ));
        }
        if oid == r.source {
            if lines.len() != 1 {
                return Err(invalid("merge-back history was reset or recreated"));
            }
        } else {
            if r.update != FinishUpdate::Merge
                || lines.len() != 2
                || !transition(lines[1], &r.source, &oid)
            {
                return Err(invalid("merge-back transition changed"));
            }
            let before = r.preparation_log.as_ref().ok_or(invalid(
                "merge-back candidate has no retained preparation start",
            ))?;
            if !plan
                .branch_log
                .as_deref()
                .unwrap_or_default()
                .starts_with(before)
            {
                return Err(invalid("merge-back preparation anchor changed"));
            }
            verify_pin(root, r, "prepare", &r.source)?;
            let action = lines[1].split(|b| *b == b'\t').nth(1).unwrap_or_default();
            let marker = format!("twigsmith-hosted-merge:{}", r.token);
            if action != marker.as_bytes()
                && !action.starts_with(format!("{marker}: ").as_bytes())
                && !action.starts_with(b"commit (merge): ")
            {
                return Err(invalid(
                    "merge-back result was not created by the owned merge or native continuation",
                ));
            }
            let parents = read(
                root,
                &[
                    "--no-replace-objects",
                    "show",
                    "-s",
                    "--format=%P",
                    std::str::from_utf8(&oid).map_err(|_| invalid("invalid candidate OID"))?,
                ],
            )?;
            if parents.strip_suffix(b"\n")
                != Some(
                    [r.source.as_slice(), b" ", &r.target_oid]
                        .concat()
                        .as_slice(),
                )
            {
                return Err(invalid(
                    "merge-back result must have exact released-source and retained-target parents",
                ));
            }
        }
        if r.candidate
            .as_ref()
            .is_some_and(|expected| expected != &oid)
        {
            return Err(invalid("checked merge-back candidate changed"));
        }
    }
    if !r.routes.is_empty() && r.routes != route_specs(root, plan, &r.routes)? {
        return Err(invalid("hosted publication endpoints changed"));
    }
    if urls(root, &r.target.remote, false)?.first() != Some(&r.target.endpoint) {
        return Err(invalid("hosted target endpoint changed"));
    }
    Ok(())
}
fn route_specs(root: &Path, plan: &Plan, previous: &[Route]) -> Result<Vec<Route>, FinishError> {
    let policy = plan
        .settings
        .workstream
        .as_ref()
        .ok_or(invalid("missing hosted policy"))?;
    let tag = plan
        .tag
        .as_ref()
        .ok_or(invalid("explicit version tag required (--tag)"))?;
    let back = plan.merge_back.as_ref().ok_or(invalid(
        "explicit merge-back branch required (--merge-back)",
    ))?;
    let mut routes = vec![];
    for (remote, reference) in [
        (&policy.publish_remote, branch_ref(&plan.source_branch)),
        (&policy.base_remote, format!("refs/tags/{tag}").into_bytes()),
        (&policy.review_remote, branch_ref(back.as_bytes())),
    ] {
        for endpoint in urls(root, remote, true)? {
            let before = previous
                .iter()
                .find(|p| p.remote == *remote && p.endpoint == endpoint && p.reference == reference)
                .map_or(RefObservation::Absent, |p| p.before.clone());
            routes.push(Route {
                remote: remote.clone(),
                endpoint,
                reference: reference.clone(),
                before,
            });
        }
    }
    Ok(routes)
}
// Native commands protect their own refs. This stage guard protects the owned
// source/attempt and checked candidate, plus clean work and selected routes that
// a pinned push or tag creation does not check. Refresh after checks and hooks,
// never merely because a read-only Git subprocess completed.
fn local_stage(root: &Path, plan: &Plan) -> Result<Plan, FinishError> {
    let repo = repository::discover(root)?.ok_or(SaveError::OutsideRepository)?;
    if repo != plan.repository {
        return Err(FinishError::ContextChanged);
    }
    let repository::Observation::Repository(state) =
        repository::inspect_context_without_upstream(root, repo.clone())?
    else {
        return Err(FinishError::ContextChanged);
    };
    let repository::Head::Attached { branch, .. } = &state.head else {
        return Err(FinishError::ContextChanged);
    };
    original_history(root)?;
    let mut next = plan.clone();
    next.settings =
        settings::load_for_branch(root, &repo, &plan.source_branch).map_err(SaveError::from)?;
    next.config = read(root, &["config", "--null", "--list"])?;
    let source_ref = branch_ref(&plan.source_branch);
    if symbolic(root, &source_ref)? {
        return Err(invalid("symbolic hosted source is unsupported"));
    }
    next.source = commit(&hint(root, &source_ref)?)?;
    next.source_log = log(root, &branch_ref(&plan.source_branch))?
        .ok_or(invalid("source branch reflog missing"))?;
    next.head_log = metadata(root, "logs/HEAD")?.ok_or(invalid("HEAD reflog missing"))?;
    next.association = private_read(&repo, &path(&repo, branch, true))?;
    next.record_bytes = private_read(&repo, &path(&repo, &plan.source_branch, false))?;
    next.worktrees = read(root, &["worktree", "list", "--porcelain", "-z"])?;
    next.state = *state;
    if let Some(back) = &plan.merge_back {
        let reference = branch_ref(back.as_bytes());
        if symbolic(root, &reference)? {
            return Err(invalid("symbolic merge-back branch is unsupported"));
        }
        next.candidate = hint(root, &reference)?;
        next.branch_log = log(root, &reference)?;
    }
    if let Some(tag) = &plan.tag {
        let reference = [b"refs/tags/", tag.as_bytes()].concat();
        if symbolic(root, &reference)? {
            return Err(invalid("symbolic version tag is unsupported"));
        }
        next.tag_object = hint(root, &reference)?;
        next.tag_log = log(root, &reference)?;
    }
    let roles = hosted_roles(plan);
    if next.settings != plan.settings
        || next.source != plan.source
        || next.source_log != plan.source_log
        || next.record_bytes != plan.record_bytes
        || route_config(&next.config, &roles, &plan.source_branch)
            != route_config(&plan.config, &roles, &plan.source_branch)
    {
        return Err(FinishError::ContextChanged);
    }
    check_worktrees(&next)?;
    Ok(next)
}
fn hosted_roles(plan: &Plan) -> Vec<&str> {
    plan.settings
        .workstream
        .as_ref()
        .map_or_else(Vec::new, |p| {
            vec![
                p.base_remote.as_str(),
                p.publish_remote.as_str(),
                p.review_remote.as_str(),
            ]
        })
}
pub fn revalidate(root: &Path, plan: &Plan) -> Result<(), FinishError> {
    let next = local_stage(root, plan)?;
    if next.state != plan.state
        || next.source_log != plan.source_log
        || next.head_log != plan.head_log
        || next.association != plan.association
        || next.candidate != plan.candidate
        || next.branch_log != plan.branch_log
        || next.tag_object != plan.tag_object
        || next.tag_log != plan.tag_log
    {
        return Err(FinishError::ContextChanged);
    }
    if let Some(record) = &plan.record {
        if record.target_retained {
            verify_pin(root, record, "target", &record.target_oid)?;
        }
        if record.preparation_retained {
            verify_pin(root, record, "prepare", &record.source)?;
        }
    }
    Ok(())
}
fn command(
    root: &Path,
    args: Vec<OsString>,
    action: Option<&str>,
    reports: &mut Vec<preparation::CommandReport>,
) -> Result<Vec<u8>, FinishError> {
    let result = git::workflow_with_reflog(root, &args, action);
    let success = result.as_ref().is_ok_and(|o| o.status.success());
    let bytes = result
        .as_ref()
        .map(|o| o.stdout.clone())
        .unwrap_or_default();
    reports.push(preparation::CommandReport {
        arguments: args,
        result,
    });
    if success {
        Ok(bytes)
    } else {
        Err(invalid(
            "Git command failed; retain completed effects, resolve the reported failure, then retry finish",
        ))
    }
}
fn endpoint(
    root: &Path,
    plan: &Plan,
    route: &Route,
    commands: &mut Vec<preparation::CommandReport>,
) -> Result<(Vec<OsString>, String), FinishError> {
    let (prefix, remote) = transport::endpoint(&plan.config, &route.remote, &route.endpoint)?;
    let mut args = prefix.clone();
    args.extend([
        "ls-remote".into(),
        "--get-url".into(),
        "--".into(),
        remote.clone().into(),
    ]);
    if command(root, args, None, commands)?.strip_suffix(b"\n") != Some(route.endpoint.as_slice()) {
        return Err(invalid("hosted endpoint resolves differently"));
    }
    Ok((prefix, remote))
}
fn observe(
    root: &Path,
    plan: &Plan,
    route: &Route,
    commands: &mut Vec<preparation::CommandReport>,
) -> Result<RefObservation, FinishError> {
    let (mut args, remote) = endpoint(root, plan, route, commands)?;
    args.extend([
        "ls-remote".into(),
        "--refs".into(),
        "--".into(),
        remote.into(),
        repository::os(&route.reference)?,
    ]);
    let output = command(root, args, None, commands)?;
    preparation::parse_advertisement(&output, &route.reference, plan.source.len())
}
fn acquire(
    root: &Path,
    plan: &Plan,
    route: &Route,
    oid: &[u8],
    commands: &mut Vec<preparation::CommandReport>,
) -> Result<(), FinishError> {
    let (mut args, remote) = endpoint(root, plan, route, commands)?;
    args.extend(
        [
            "--no-replace-objects",
            "fetch",
            "--no-tags",
            "--no-prune",
            "--no-prune-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--no-write-commit-graph",
            "--no-auto-maintenance",
            "--refmap=",
        ]
        .map(OsString::from),
    );
    args.extend(["--".into(), remote.into(), repository::os(oid)?]);
    command(root, args, None, commands)?;
    if read(
        root,
        &[
            "--no-replace-objects",
            "cat-file",
            "-t",
            std::str::from_utf8(oid).map_err(|_| invalid("invalid remote OID"))?,
        ],
    )? != b"commit\n"
    {
        return Err(invalid("advertised commit object is unavailable"));
    }
    Ok(())
}
fn store(_root: &Path, plan: &Plan, record: &Record) -> Result<Plan, FinishError> {
    let bytes = serde_json::to_vec(record).map_err(|_| invalid("cannot encode hosted attempt"))?;
    write_private(
        &plan.repository,
        &path(&plan.repository, &plan.source_branch, false),
        &bytes,
        plan.record_bytes.as_deref(),
    )?;
    let source_path = path(&plan.repository, &plan.source_branch, true);
    let association = private_read(&plan.repository, &source_path)?;
    if association
        .as_ref()
        .is_some_and(|value| value != &plan.source_branch)
    {
        return Err(invalid("source hosted association changed"));
    }
    if association.is_none() {
        write_private(&plan.repository, &source_path, &plan.source_branch, None)?;
    }
    let mut next = plan.clone();
    next.record = Some(record.clone());
    next.record_bytes = Some(bytes);
    if matches!(&plan.state.head, repository::Head::Attached { branch, .. } if branch == &plan.source_branch)
    {
        next.association = Some(plan.source_branch.clone());
    }
    Ok(next)
}
fn after(root: &Path, plan: &Plan) -> Result<Plan, FinishError> {
    let next = local_stage(root, plan)?;
    if let Some(record) = &next.record {
        verify_record(root, &next, record)?;
    }
    Ok(next)
}
fn switch(
    root: &Path,
    plan: &Plan,
    branch: &[u8],
    commands: &mut Vec<preparation::CommandReport>,
) -> Result<Plan, FinishError> {
    revalidate(root, plan)?;
    clean(&plan.state)?;
    command(
        root,
        vec![
            "-c".into(),
            "submodule.recurse=false".into(),
            "switch".into(),
            "--no-guess".into(),
            "--no-overwrite-ignore".into(),
            "--".into(),
            repository::os(branch)?,
        ],
        None,
        commands,
    )?;
    let next = after(root, plan)?;
    if !matches!(&next.state.head, repository::Head::Attached { branch: selected, .. } if selected == branch)
    {
        return Err(FinishError::ContextChanged);
    }
    clean(&next.state)?;
    Ok(next)
}
fn checked(
    root: &Path,
    plan: &Plan,
    oid: &[u8],
    skip: bool,
    events: &mut impl FnMut(checks::Event<'_>),
) -> Result<(), FinishError> {
    clean(&plan.state)?;
    let report = checks::run_guarded(root, &plan.settings.finish_checks, skip, events, || {
        revalidate(root, plan)?;
        clean(&plan.state)?;
        if !matches!(&plan.state.head, repository::Head::Attached {commit, ..} if commit == oid) {
            return Err(checks::Error::Unprepared);
        }
        Ok(())
    });
    if report.error.is_some() {
        return Err(invalid(
            "hosted checks failed, were interrupted, or changed context; resolve the failure and retry finish",
        ));
    }
    Ok(())
}
fn desired(plan: &Plan, route: &Route) -> Result<Option<Vec<u8>>, FinishError> {
    if route.reference.starts_with(b"refs/tags/") {
        Ok(match &plan.tag_object {
            RefObservation::Commit(oid) => Some(oid.clone()),
            _ => None,
        })
    } else if route.reference == branch_ref(&plan.source_branch) {
        Ok(Some(plan.source.clone()))
    } else {
        Ok(plan.record.as_ref().and_then(|r| r.candidate.clone()))
    }
}
fn pending_routes(
    root: &Path,
    plan: &Plan,
    commands: &mut Vec<preparation::CommandReport>,
) -> Result<(), FinishError> {
    let r = plan
        .record
        .as_ref()
        .ok_or(invalid("hosted attempt missing"))?;
    if observe(root, plan, &r.target, commands)? != RefObservation::Commit(r.target_oid.clone()) {
        return Err(invalid(
            "review target changed during preparation; retained preparation cannot adopt a new target",
        ));
    }
    let mut tag_route = r.target.clone();
    tag_route.reference = format!(
        "refs/tags/{}",
        r.release
            .tag
            .as_ref()
            .ok_or(invalid("missing retained tag name"))?
    )
    .into_bytes();
    let tag = observe(root, plan, &tag_route, commands)?;
    if tag != RefObservation::Absent && tag != plan.tag_object {
        return Err(invalid(
            "version tag collision at the project fetch endpoint",
        ));
    }
    for route in &r.routes {
        let value = observe(root, plan, route, commands)?;
        if value != route.before
            && desired(plan, route)?.map(RefObservation::Commit).as_ref() != Some(&value)
        {
            return Err(invalid(
                "hosted publication ref changed or collided; retained expectations are not renewed",
            ));
        }
    }
    Ok(())
}
/// Execute only this attempt's source/tag and merge-back preparation/publication.
pub fn execute(
    root: &Path,
    initial: &Plan,
    skip: bool,
    mut events: impl FnMut(Event<'_>),
) -> Report {
    let mut report = Report::default();
    let mut plan = initial.clone();
    let mut caches = vec![];
    let result = (|| -> Result<(), FinishError> {
        revalidate(root, &plan)?;
        clean(&plan.state)?;
        if plan.update == FinishUpdate::Ask || plan.tag.is_none() || plan.merge_back.is_none() {
            return Err(invalid(
                "resolve version tag, merge-back name, and preparation method before execution",
            ));
        }
        if plan.settings.finish_checkout != FinishCheckout::Work
            || plan.settings.finish_cleanup != FinishCleanup::Keep
            || plan.settings.finish_integration != FinishIntegration::Merge
            || plan.settings.finish_rewrite == FinishRewrite::Allow
        {
            return Err(invalid(
                "hosted settings require checkout work, cleanup keep, merge guidance, and no rewrite",
            ));
        }
        if plan.record.is_none() {
            let policy = plan
                .settings
                .workstream
                .as_ref()
                .ok_or(invalid("missing hosted policy"))?;
            let target = Route {
                remote: policy.base_remote.clone(),
                endpoint: urls(root, &policy.base_remote, false)?.remove(0),
                reference: branch_ref(policy.integration_target.as_bytes()),
                before: RefObservation::Absent,
            };
            let mut tag_route = target.clone();
            tag_route.reference = format!(
                "refs/tags/{}",
                plan.tag
                    .as_ref()
                    .ok_or(invalid("explicit version tag required"))?
            )
            .into_bytes();
            if observe(root, &plan, &tag_route, &mut report.commands)? != RefObservation::Absent {
                return Err(invalid(
                    "version tag collision at the project fetch endpoint",
                ));
            }
            let target_oid = commit(&observe(root, &plan, &target, &mut report.commands)?)?;
            acquire(root, &plan, &target, &target_oid, &mut report.commands)?;
            if ancestor(root, &target_oid, &plan.source)?.is_none() {
                return Err(invalid("target history unavailable"));
            }
            let mut routes = route_specs(root, &plan, &[])?;
            for route in &mut routes {
                route.before = observe(root, &plan, route, &mut report.commands)?;
                if route.reference == branch_ref(&plan.source_branch) {
                    if let RefObservation::Commit(oid) = &route.before {
                        acquire(root, &plan, route, oid, &mut report.commands)?;
                        if ancestor(root, oid, &plan.source)? != Some(true) {
                            return Err(invalid(
                                "published original source contains work outside the approved version",
                            ));
                        }
                    } else if route.before != RefObservation::Absent {
                        return Err(invalid("original source observation unavailable"));
                    }
                } else if route.before != RefObservation::Absent {
                    return Err(invalid("remote version tag or merge-back branch collision"));
                }
            }
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid("clock unavailable"))?
                .as_nanos();
            let mut release = plan.request.release.clone();
            release.tag = plan.tag.clone();
            let record = Record {
                version: 1,
                root: plan
                    .repository
                    .working_tree_root
                    .clone()
                    .ok_or(SaveError::BareRepository)?,
                git_dir: plan.repository.git_dir.clone(),
                common_dir: plan.repository.common_dir.clone(),
                source_branch: plan.source_branch.clone(),
                source: plan.source.clone(),
                source_log: plan.source_log.clone(),
                head_log: plan.head_log.clone(),
                settings: plan.settings.clone(),
                config: plan.config.clone(),
                committed_settings: plan.committed_settings.clone(),
                release,
                merge_back: plan.merge_back.clone().unwrap(),
                update: plan.update,
                token: format!("{}-{stamp}", std::process::id()),
                target,
                target_oid,
                target_retained: false,
                preparation_log: None,
                preparation_retained: false,
                routes,
                tag_object: None,
                branch_created: false,
                candidate: None,
                source_checked: false,
                candidate_checked: None,
            };
            plan = store(root, &plan, &record)?;
        }
        pending_routes(root, &plan, &mut report.commands)?;
        let mut record = plan.record.clone().unwrap();
        for route in &record.routes {
            if !caches.iter().any(|(remote, reference, _)| {
                remote == &route.remote && reference == &route.reference
            }) {
                caches.push((
                    route.remote.clone(),
                    route.reference.clone(),
                    crate::tracking::cache::Prepared::new(root, &route.remote, &route.reference),
                ));
            }
        }
        retain_pin(
            root,
            &plan,
            &record,
            "target",
            &record.target_oid,
            &mut report.commands,
        )?;
        record.target_retained = true;
        plan = store(root, &plan, &record)?;
        if !record.source_checked {
            if !matches!(&plan.state.head, repository::Head::Attached {branch, ..} if branch == &plan.source_branch)
            {
                plan = switch(
                    root,
                    &plan,
                    &plan.source_branch.clone(),
                    &mut report.commands,
                )?;
            }
            events(Event::Source(&record.source));
            checked(root, &plan, &record.source, skip, &mut |event| {
                events(Event::Check(event))
            })?;
            if !skip {
                record.source_checked = true;
                plan = store(root, &plan, &record)?;
            }
        }
        if plan.tag_object == RefObservation::Absent {
            revalidate(root, &plan)?;
            let mut args = vec!["tag".into(), "--create-reflog".into()];
            if record.release.sign {
                args.push("--sign".into());
            }
            if let Some(message) = &record.release.message {
                args.extend(["--message".into(), message.into()]);
            } else {
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
                    return Err(invalid("invalid Git signing preference"));
                }
                if record.release.sign || signing.stdout == b"true\n" {
                    args.extend([
                        "--message".into(),
                        record.release.tag.as_ref().unwrap().into(),
                    ]);
                }
            }
            args.extend([
                "--".into(),
                record.release.tag.as_ref().unwrap().into(),
                repository::os(&record.source)?,
            ]);
            command(
                root,
                args,
                Some(&format!("twigsmith-hosted-tag:{}", record.token)),
                &mut report.commands,
            )?;
            plan = after(root, &plan)?;
        }
        record.tag_object = Some(commit(&plan.tag_object)?);
        plan = store(root, &plan, &record)?;
        if plan.candidate == RefObservation::Absent {
            let association_path = path(&plan.repository, record.merge_back.as_bytes(), true);
            let existing = private_read(&plan.repository, &association_path)?;
            if existing
                .as_ref()
                .is_some_and(|bytes| bytes != &record.source_branch)
            {
                return Err(invalid("merge-back association collision"));
            }
            revalidate(root, &plan)?;
            write_private(
                &plan.repository,
                &association_path,
                &record.source_branch,
                existing.as_deref(),
            )?;
            command(
                root,
                vec![
                    "update-ref".into(),
                    "--no-deref".into(),
                    "--create-reflog".into(),
                    "-m".into(),
                    format!("twigsmith-hosted-branch:{}", record.token).into(),
                    format!("refs/heads/{}", record.merge_back).into(),
                    repository::os(&record.source)?,
                    repository::os(&vec![b'0'; record.source.len()])?,
                ],
                None,
                &mut report.commands,
            )?;
            plan = after(root, &plan)?;
        }
        record.branch_created = true;
        plan = store(root, &plan, &record)?;
        if !matches!(&plan.state.head, repository::Head::Attached {branch, ..} if branch == record.merge_back.as_bytes())
        {
            plan = switch(
                root,
                &plan,
                record.merge_back.as_bytes(),
                &mut report.commands,
            )?;
        }
        if record.update == FinishUpdate::Merge
            && plan.candidate == RefObservation::Commit(record.source.clone())
            && ancestor(root, &record.target_oid, &record.source)? != Some(true)
        {
            pending_routes(root, &plan, &mut report.commands)?;
            revalidate(root, &plan)?;
            clean(&plan.state)?;
            protect_ignored(root, &record.target_oid)?;
            if record.preparation_log.is_none() {
                record.preparation_log = plan.branch_log.clone();
                plan = store(root, &plan, &record)?;
            }
            retain_pin(
                root,
                &plan,
                &record,
                "prepare",
                &record.source,
                &mut report.commands,
            )?;
            record.preparation_retained = true;
            plan = store(root, &plan, &record)?;
            revalidate(root, &plan)?;
            clean(&plan.state)?;
            protect_ignored(root, &record.target_oid)?;
            command(
                root,
                vec![
                    "-c".into(),
                    "submodule.recurse=false".into(),
                    "merge".into(),
                    "--no-ff".into(),
                    "--commit".into(),
                    "--no-autostash".into(),
                    "--no-overwrite-ignore".into(),
                    "--no-edit".into(),
                    "-m".into(),
                    format!(
                        "Merge {} into {} for review",
                        plan.settings.base, record.merge_back
                    )
                    .into(),
                    "--".into(),
                    repository::os(&record.target_oid)?,
                ],
                Some(&format!("twigsmith-hosted-merge:{}", record.token)),
                &mut report.commands,
            )?;
            plan = after(root, &plan)?;
        }
        clean(&plan.state)?;
        record.candidate = Some(commit(&plan.candidate)?);
        plan = store(root, &plan, &record)?;
        let candidate = record.candidate.as_ref().unwrap();
        if record.candidate_checked.as_ref() != Some(candidate)
            && !(record.source_checked && candidate == &record.source)
        {
            events(Event::Candidate(candidate));
            checked(root, &plan, candidate, skip, &mut |event| {
                events(Event::Check(event))
            })?;
            if !skip {
                record.candidate_checked = Some(candidate.clone());
                plan = store(root, &plan, &record)?;
            }
        }
        // Refresh remote workflow conditions once after candidate checks. Each
        // pending write then uses its original exact lease; successful porcelain
        // acknowledgements supply delivery evidence without remote readback.
        pending_routes(root, &plan, &mut report.commands)?;
        for route in &record.routes {
            let oid = desired(&plan, route)?.ok_or(invalid("publication candidate missing"))?;
            if observe(root, &plan, route, &mut report.commands)?
                == RefObservation::Commit(oid.clone())
            {
                report.publications.push(PublicationStatus {
                    remote: route.remote.clone(),
                    destination: route.endpoint.clone(),
                    reference: route.reference.clone(),
                    observed: RefObservation::Commit(oid.clone()),
                    expected: Some(oid),
                });
                continue;
            }
            let old = match &route.before {
                RefObservation::Absent => Vec::new(),
                RefObservation::Commit(oid) => oid.clone(),
                _ => return Err(invalid("unavailable publication lease")),
            };
            let (mut args, remote) = endpoint(root, &plan, route, &mut report.commands)?;
            args.extend(
                [
                    "--no-replace-objects",
                    "push",
                    "--porcelain",
                    "--no-force",
                    "--no-follow-tags",
                ]
                .map(OsString::from),
            );
            args.push(save::publication::submodule_policy(root)?.into());
            args.extend([
                repository::os(
                    &[
                        b"--force-with-lease=".as_slice(),
                        &route.reference,
                        b":",
                        &old,
                    ]
                    .concat(),
                )?,
                "--".into(),
                remote.into(),
                repository::os(&[oid.as_slice(), b":", &route.reference].concat())?,
            ]);
            revalidate(root, &plan)?;
            clean(&plan.state)?;
            let pushed = command(root, args, None, &mut report.commands);
            let confirmed = report
                .commands
                .last()
                .and_then(|command| command.result.as_ref().ok())
                .is_some_and(|output| {
                    let (responses, valid) =
                        crate::publication::responses(&output.stdout, &oid, &route.reference);
                    valid
                        && responses.len() == 1
                        && matches!(
                            responses[0].state,
                            crate::publication::Status::Created
                                | crate::publication::Status::Advanced
                                | crate::publication::Status::Unchanged
                        )
                });
            if confirmed {
                report.publications.push(PublicationStatus {
                    remote: route.remote.clone(),
                    destination: route.endpoint.clone(),
                    reference: route.reference.clone(),
                    observed: RefObservation::Commit(oid.clone()),
                    expected: Some(oid),
                });
            }
            pushed?;
            if !confirmed {
                return Err(invalid(
                    "hosted publication is unconfirmed; retry finish to observe pending delivery",
                ));
            }
        }
        report.ready = true;
        Ok(())
    })();
    report.local_tag = plan.tag.as_ref().map(|tag| {
        hint(root, format!("refs/tags/{tag}").as_bytes()).unwrap_or(RefObservation::Unavailable)
    });
    report.local_candidate = plan.merge_back.as_ref().map(|branch| {
        hint(root, &branch_ref(branch.as_bytes())).unwrap_or(RefObservation::Unavailable)
    });
    report.error = result.err();
    report.plan = if report.error.is_some() {
        assess(root, &plan.request).ok()
    } else {
        None
    }
    .or(Some(plan));
    if let Some(current) = &report.plan
        && let Some(record) = &current.record
    {
        // Failure may leave a completed effect without acknowledgement. Reconcile
        // only destinations that have no known exact outcome; retain prior success.
        for route in &record.routes {
            if report.publications.iter().any(|p| {
                p.remote == route.remote
                    && p.destination == route.endpoint
                    && p.reference == route.reference
            }) {
                continue;
            }
            let observed = observe(root, current, route, &mut report.commands)
                .unwrap_or(RefObservation::Unavailable);
            let expected = desired(current, route).ok().flatten();
            report.publications.push(PublicationStatus {
                remote: route.remote.clone(),
                destination: route.endpoint.clone(),
                reference: route.reference.clone(),
                observed,
                expected,
            });
        }
        for (remote, reference, cache) in &caches {
            let confirmed = report
                .publications
                .iter()
                .filter(|p| {
                    p.remote == *remote
                        && p.reference == *reference
                        && p.expected
                            .as_ref()
                            .is_some_and(|oid| p.observed == RefObservation::Commit(oid.clone()))
                })
                .map(|p| {
                    (
                        p.destination.clone(),
                        record
                            .routes
                            .iter()
                            .find(|route| {
                                route.remote == p.remote
                                    && route.reference == p.reference
                                    && route.endpoint == p.destination
                            })
                            .and_then(|route| match &route.before {
                                RefObservation::Commit(oid) => Some(oid.clone()),
                                _ => None,
                            }),
                    )
                })
                .collect::<Vec<_>>();
            if !confirmed.is_empty() {
                let desired = report
                    .publications
                    .iter()
                    .find(|p| p.remote == *remote && p.reference == *reference)
                    .and_then(|p| p.expected.as_deref());
                report.caches.push(cache.refresh(root, &confirmed, desired));
            }
        }
        if !report.caches.is_empty()
            && report.error.is_none()
            && let Err(error) = revalidate(root, current)
        {
            report.ready = false;
            report.error = Some(error);
        }
    }
    report
}
/// Confirmed identity and routes for submission instructions, without external state.
pub fn destinations(plan: &Plan) -> Vec<Vec<u8>> {
    plan.record.as_ref().map_or_else(Vec::new, |r| {
        r.routes
            .iter()
            .filter(|route| route.reference == branch_ref(r.merge_back.as_bytes()))
            .map(|route| route.endpoint.clone())
            .collect()
    })
}
/// Cheap workflow dispatch from the command's already selected repository and
/// policy. Full hosted assessment belongs only to tagged review or its retry.
/// Associations and creation reflogs are still checked before another workflow
/// can run: Git cannot recognize ownership of an unfinished hosted preparation.
pub(crate) struct Routing {
    pub branch: Option<Vec<u8>>,
    pub source_branch: Option<Vec<u8>>,
    pub settings: Settings,
    pub selected: bool,
    pub retained: bool,
}
pub(crate) fn routing(
    root: &Path,
    repo: &repository::RepositoryContext,
    settings: &Settings,
) -> Result<Routing, FinishError> {
    let output =
        git::inspect(root, ["symbolic-ref", "--quiet", "HEAD"]).map_err(InspectionError::Io)?;
    if !output.status.success() {
        if output.status.code() != Some(1) {
            return Err(InspectionError::Git {
                arguments: ["symbolic-ref", "--quiet", "HEAD"]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
                output,
            }
            .into());
        }
        return Ok(Routing {
            branch: None,
            source_branch: None,
            settings: settings.clone(),
            selected: false,
            retained: false,
        });
    }
    let branch = output
        .stdout
        .strip_suffix(b"\n")
        .and_then(|reference| reference.strip_prefix(b"refs/heads/"))
        .ok_or(invalid("invalid HEAD branch"))?
        .to_vec();
    let association = private_read(repo, &path(repo, &branch, true))?;
    if association.is_none() && hosted_branch(root, &branch)? {
        return Err(invalid(
            "owned merge-back association is missing; restore hosted recovery evidence",
        ));
    }
    let source_branch = association.clone().unwrap_or_else(|| branch.clone());
    let record = private_read(repo, &path(repo, &source_branch, false))?;
    if association.is_some() && record.is_none() {
        return Err(invalid(
            "hosted attempt missing for this merge-back association",
        ));
    }
    let settings = if source_branch != branch {
        settings::load_for_branch(root, repo, &source_branch).map_err(SaveError::from)?
    } else {
        settings.clone()
    };
    let selected = association.is_some()
        || settings.workstream.as_ref().is_some_and(|p| {
            p.version_tagging && p.version_tag_identity == settings::VersionTagIdentity::WorkSource
        });
    Ok(Routing {
        branch: Some(branch),
        source_branch: Some(source_branch),
        settings,
        selected,
        retained: record.is_some(),
    })
}

fn hosted_branch(root: &Path, branch: &[u8]) -> Result<bool, FinishError> {
    Ok(log(root, &branch_ref(branch))?.is_some_and(|bytes| {
        entries(&bytes).iter().any(|line| {
            line.split(|b| *b == b'\t')
                .nth(1)
                .is_some_and(|message| message.starts_with(b"twigsmith-hosted-branch:"))
        })
    }))
}

fn pin_ref(record: &Record, kind: &str) -> String {
    format!("refs/twigsmith/hosted/{}/{kind}", record.token)
}
fn verify_pin(root: &Path, record: &Record, kind: &str, oid: &[u8]) -> Result<(), FinishError> {
    let reference = pin_ref(record, kind);
    if symbolic(root, reference.as_bytes())?
        || hint(root, reference.as_bytes())? != RefObservation::Commit(oid.to_vec())
    {
        return Err(invalid("hosted retained object ref is missing or changed"));
    }
    let bytes =
        log(root, reference.as_bytes())?.ok_or(invalid("hosted retained object reflog missing"))?;
    let lines = entries(&bytes);
    let action = format!("twigsmith-hosted-{kind}:{}", record.token);
    if lines.len() != 1
        || !transition(lines[0], &vec![b'0'; oid.len()], oid)
        || lines[0].split(|b| *b == b'\t').nth(1) != Some(action.as_bytes())
    {
        return Err(invalid(
            "hosted retained object creation is not corroborated",
        ));
    }
    Ok(())
}
fn retain_pin(
    root: &Path,
    _plan: &Plan,
    record: &Record,
    kind: &str,
    oid: &[u8],
    commands: &mut Vec<preparation::CommandReport>,
) -> Result<(), FinishError> {
    let reference = pin_ref(record, kind);
    if hint(root, reference.as_bytes())? == RefObservation::Absent {
        if log(root, reference.as_bytes())?.is_some()
            || (kind == "target" && record.target_retained)
            || (kind == "prepare" && record.preparation_retained)
        {
            return Err(invalid("hosted retained object evidence was removed"));
        }
        command(
            root,
            vec![
                "update-ref".into(),
                "--no-deref".into(),
                "--create-reflog".into(),
                "-m".into(),
                format!("twigsmith-hosted-{kind}:{}", record.token).into(),
                reference.clone().into(),
                repository::os(oid)?,
                repository::os(&vec![b'0'; oid.len()])?,
            ],
            None,
            commands,
        )?;
    }
    verify_pin(root, record, kind, oid)
}
// Git's non-fast-forward merge machinery can overwrite ignored paths even with
// --no-overwrite-ignore. Refuse target path overlap explicitly before the merge.
fn protect_ignored(root: &Path, target: &[u8]) -> Result<(), FinishError> {
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
    let paths = read(
        root,
        &[
            "--no-replace-objects",
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            std::str::from_utf8(target).map_err(|_| invalid("invalid target OID"))?,
        ],
    )?;
    for ignored in ignored.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        for tracked in paths.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            if ignored == tracked
                || ignored.starts_with(&[tracked, b"/"].concat())
                || tracked.starts_with(&[ignored, b"/"].concat())
            {
                return Err(invalid(
                    "merge-back preparation would overwrite an ignored local path; preserve or move it before retrying",
                ));
            }
        }
    }
    Ok(())
}
