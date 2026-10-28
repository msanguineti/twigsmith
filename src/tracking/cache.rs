//! Narrow maintenance of fetch-mapped caches after independently verified effects.
//! A cache never supplies publication, lease, incorporation, or cleanup authority.
use crate::{git, repository};
use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, Read, Seek, Write},
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Current,
    Updated,
    Unmapped,
    Unavailable,
}
#[derive(Debug)]
pub struct RefOutcome {
    pub reference: Vec<u8>,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
    pub updated: bool,
    pub issue: Option<String>,
}
#[derive(Debug)]
pub struct Report {
    pub remote: String,
    pub reference: Vec<u8>,
    pub state: State,
    pub refs: Vec<RefOutcome>,
    pub issues: Vec<String>,
}
impl Report {
    pub fn has_issue(&self) -> bool {
        !self.issues.is_empty() || self.refs.iter().any(|r| r.issue.is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mapping {
    remote: Vec<u8>,
    source: Vec<u8>,
    target: Option<Vec<u8>>,
    negative: bool,
}
fn substitute(pattern: &[u8], replacement: &[u8], value: &[u8]) -> Option<Vec<u8>> {
    if let Some(star) = pattern.iter().position(|b| *b == b'*') {
        let (prefix, suffix) = (&pattern[..star], &pattern[star + 1..]);
        if !value.starts_with(prefix)
            || !value.ends_with(suffix)
            || value.len() < prefix.len() + suffix.len()
        {
            return None;
        }
        let star = replacement.iter().position(|b| *b == b'*')?;
        Some(
            [
                &replacement[..star],
                &value[prefix.len()..value.len() - suffix.len()],
                &replacement[star + 1..],
            ]
            .concat(),
        )
    } else {
        (pattern == value).then(|| replacement.to_vec())
    }
}
fn excluded(mappings: &[Mapping], remote: &[u8], reference: &[u8]) -> bool {
    mappings.iter().any(|m| {
        m.remote == remote && m.negative && substitute(&m.source, &m.source, reference).is_some()
    })
}
fn read(root: &Path, args: &[OsString]) -> Result<Vec<u8>, String> {
    let output = git::inspect(root, args).map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Git cache inspection failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}
fn os(value: &[u8]) -> Result<OsString, String> {
    repository::os(value).map_err(|e| e.to_string())
}
fn configuration(root: &Path) -> Result<(Vec<u8>, Vec<Mapping>), String> {
    let raw = read(root, &["config".into(), "--null".into(), "--list".into()])?;
    let mut relevant = vec![];
    let mut mappings = vec![];
    for entry in raw.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let Some(split) = entry.iter().position(|b| *b == b'\n') else {
            continue;
        };
        let (key, value) = (&entry[..split], &entry[split + 1..]);
        if key.starts_with(b"remote.") || key.starts_with(b"url.") {
            relevant.extend_from_slice(entry);
            relevant.push(0);
        }
        let Some(remote) = key
            .strip_prefix(b"remote.")
            .and_then(|k| k.strip_suffix(b".fetch"))
        else {
            continue;
        };
        let value = value.strip_prefix(b"+").unwrap_or(value);
        let negative = value.starts_with(b"^");
        let value = value.strip_prefix(b"^").unwrap_or(value);
        let (source, target) = value
            .iter()
            .position(|b| *b == b':')
            .map_or((value, None), |i| (&value[..i], Some(&value[i + 1..])));
        let target = target.filter(|t| !t.is_empty());
        // Abbreviated sources and malformed patterns cannot prove an exact
        // relationship. Source-only mappings intentionally identify no cache.
        if !source.starts_with(b"refs/")
            || source.iter().filter(|b| **b == b'*').count() > 1
            || (negative && target.is_some())
            || target.is_some_and(|t| {
                t.iter().filter(|b| **b == b'*').count()
                    != source.iter().filter(|b| **b == b'*').count()
            })
        {
            return Err(
                "fetch mappings do not identify an unambiguous full-ref cache relationship".into(),
            );
        }
        mappings.push(Mapping {
            remote: remote.to_vec(),
            source: source.to_vec(),
            target: target.filter(|t| !t.is_empty()).map(<[u8]>::to_vec),
            negative,
        });
    }
    Ok((relevant, mappings))
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    oid: Option<Vec<u8>>,
    symbolic: bool,
    log: Option<Vec<u8>>,
}
fn version(
    root: &Path,
    repo: &repository::RepositoryContext,
    reference: &[u8],
) -> Result<Version, String> {
    let output = git::inspect(
        root,
        &["symbolic-ref".into(), "--quiet".into(), os(reference)?],
    )
    .map_err(|e| e.to_string())?;
    let symbolic = output.status.success();
    if !symbolic && output.status.code() != Some(1) {
        return Err("cannot inspect the cache's symbolic relationship".into());
    }
    let output = git::inspect(
        root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            "--end-of-options".into(),
            os(reference)?,
        ],
    )
    .map_err(|e| e.to_string())?;
    let oid = if output.status.success() {
        Some(
            output
                .stdout
                .strip_suffix(b"\n")
                .ok_or("invalid cache object ID")?
                .to_vec(),
        )
    } else if output.status.code() == Some(1) && output.stderr.is_empty() {
        None
    } else {
        return Err("cannot inspect the cache object ID".into());
    };
    let log = match fs::read(repo.common_dir.join("logs").join(os(reference)?)) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    Ok(Version { oid, symbolic, log })
}
struct Binding {
    repository: repository::RepositoryContext,
    config: Vec<u8>,
    endpoint: Vec<u8>,
    refs: Vec<(Vec<u8>, Version)>,
}
pub(crate) struct Prepared {
    remote: String,
    reference: Vec<u8>,
    binding: Result<Binding, String>,
}
impl Prepared {
    pub(crate) fn new(root: &Path, remote: &str, reference: &[u8]) -> Self {
        let binding = (|| {
            let repository = repository::discover(root)
                .map_err(|e| e.to_string())?
                .ok_or("cache repository unavailable")?;
            let (config, mappings) = configuration(root)?;
            let endpoint = read(
                root,
                &[
                    "remote".into(),
                    "get-url".into(),
                    "--".into(),
                    remote.into(),
                ],
            )?
            .strip_suffix(b"\n")
            .ok_or("invalid cache fetch endpoint")?
            .to_vec();
            let mut refs = vec![];
            if !excluded(&mappings, remote.as_bytes(), reference) {
                for mapping in mappings
                    .iter()
                    .filter(|m| m.remote == remote.as_bytes() && !m.negative)
                {
                    let Some(target) = &mapping.target else {
                        continue;
                    };
                    let Some(target) = substitute(&mapping.source, target, reference) else {
                        continue;
                    };
                    if !target.starts_with(b"refs/remotes/") {
                        return Err(
                            "fetch mapping selects a ref outside the remote-tracking namespace"
                                .into(),
                        );
                    }
                    if refs.iter().any(|(r, _)| r == &target) {
                        continue;
                    }
                    read(root, &["check-ref-format".into(), os(&target)?])?;
                    for other in mappings.iter().filter(|m| !m.negative) {
                        let Some(local) = &other.target else { continue };
                        let Some(source) = substitute(local, &other.source, &target) else {
                            continue;
                        };
                        if !excluded(&mappings, &other.remote, &source)
                            && (other.remote != remote.as_bytes() || source != reference)
                        {
                            return Err(
                                "fetch mappings share the cache with another remote or ref".into(),
                            );
                        }
                    }
                    let before = version(root, &repository, &target)?;
                    refs.push((target, before));
                }
            }
            Ok(Binding {
                repository,
                config,
                endpoint,
                refs,
            })
        })();
        Self {
            remote: remote.to_owned(),
            reference: reference.to_vec(),
            binding,
        }
    }
    fn guard(&self, root: &Path, binding: &Binding) -> Result<(), String> {
        if repository::discover(root)
            .map_err(|e| e.to_string())?
            .as_ref()
            != Some(&binding.repository)
            || configuration(root)?.0 != binding.config
        {
            return Err("cache routing or repository changed during delivery".into());
        }
        Ok(())
    }
    fn verify_remote(&self, root: &Path, desired: Option<&[u8]>) -> Result<(), String> {
        let bytes = read(
            root,
            &[
                "ls-remote".into(),
                "--refs".into(),
                "--".into(),
                self.remote.clone().into(),
                os(&self.reference)?,
            ],
        )?;
        let lines: Vec<_> = bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        let actual = match lines.as_slice() {
            [] => None,
            [line] => {
                let split = line
                    .iter()
                    .position(|b| *b == b'\t')
                    .ok_or("invalid remote cache observation")?;
                if line[split + 1..] != self.reference {
                    return Err("remote cache observation names another ref".into());
                }
                Some(&line[..split])
            }
            _ => return Err("remote cache observation is ambiguous".into()),
        };
        if actual != desired {
            return Err(
                "fetch endpoint does not retain the delivered ref value; cache retained".into(),
            );
        }
        Ok(())
    }
    /// The caller selects endpoints from its publication attempt and supplies
    /// original verified remote versions where available. Exact fresh equality
    /// independently verifies the cache value, including partial save whose
    /// transport display URLs cannot be assigned to configured endpoints.
    /// This never upgrades delivery acknowledgements or renews a push lease.
    pub(crate) fn refresh(
        &self,
        root: &Path,
        confirmed: &[(Vec<u8>, Option<Vec<u8>>)],
        desired: Option<&[u8]>,
    ) -> Report {
        let mut report = Report {
            remote: self.remote.clone(),
            reference: self.reference.clone(),
            state: State::Unavailable,
            refs: vec![],
            issues: vec![],
        };
        let result = (|| {
            let binding = self.binding.as_ref().map_err(Clone::clone)?;
            if binding.refs.is_empty() {
                report.state = State::Unmapped;
                return Ok(());
            }
            let (_, previous) = confirmed
                .iter()
                .find(|(endpoint, _)| endpoint == &binding.endpoint)
                .ok_or("confirmed delivery has no matching configured fetch endpoint")?;
            self.guard(root, binding)?;
            self.verify_remote(root, desired)?;
            report.state = State::Current;
            for (reference, before) in &binding.refs {
                let mut outcome = RefOutcome {
                    reference: reference.clone(),
                    before: before.oid.clone(),
                    after: before.oid.clone(),
                    updated: false,
                    issue: None,
                };
                outcome.issue = (|| {
                    self.guard(root, binding)?;
                    if before.symbolic {
                        return Err("cache ref is symbolic; its target was preserved".into());
                    }
                    if version(root, &binding.repository, reference)? != *before {
                        return Err(
                            "cache changed concurrently; newer local observation was preserved"
                                .into(),
                        );
                    }
                    if before.oid.as_deref() == desired {
                        return Ok(());
                    }
                    if let Some(old) = before.oid.as_deref() {
                        let bound = desired
                            .or(previous.as_deref())
                            .ok_or("cache deletion has no verified previous remote version")?;
                        if Some(old) != previous.as_deref() && !ancestor(root, old, bound)? {
                            return Err(
                                "cache contains a newer or unrelated observation; it was preserved"
                                    .into(),
                            );
                        }
                    }
                    transaction(root, reference, before.oid.as_deref(), desired, || {
                        self.guard(root, binding)?;
                        if version(root, &binding.repository, reference)? != *before {
                            return Err(
                                "cache changed or became symbolic while locking; it was preserved"
                                    .into(),
                            );
                        }
                        self.verify_remote(root, desired)?;
                        self.guard(root, binding)?;
                        if version(root, &binding.repository, reference)? != *before {
                            return Err(
                                "cache changed during endpoint observation; it was preserved"
                                    .into(),
                            );
                        }
                        Ok(())
                    })?;
                    outcome.updated = true;
                    Ok(())
                })()
                .err();
                match version(root, &binding.repository, reference) {
                    Ok(after) => outcome.after = after.oid,
                    Err(e) => {
                        outcome.issue.get_or_insert(e);
                    }
                };
                if outcome.issue.is_some() {
                    report.state = State::Unavailable
                } else if outcome.updated && report.state != State::Unavailable {
                    report.state = State::Updated
                }
                report.refs.push(outcome);
            }
            Ok::<(), String>(())
        })();
        if let Err(issue) = result {
            report.issues.push(issue)
        }
        report
    }
}
fn ancestor(root: &Path, old: &[u8], new: &[u8]) -> Result<bool, String> {
    let repo = repository::discover(root)
        .map_err(|e| e.to_string())?
        .ok_or("cache repository unavailable")?;
    match fs::read(repo.common_dir.join("info/grafts")) {
        Ok(bytes) if !bytes.is_empty() => {
            return Err("grafted history cannot establish a safe cache update".into());
        }
        Ok(_) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    let output = git::inspect(
        root,
        &[
            "--no-replace-objects".into(),
            "merge-base".into(),
            "--is-ancestor".into(),
            os(old)?,
            os(new)?,
        ],
    )
    .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(true)
    } else if output.status.code() == Some(1) {
        Ok(false)
    } else {
        Err("local history cannot establish a safe cache update".into())
    }
}
// Git 2.47's no-deref OID comparison alone permits same-OID symbolic refs.
// Prepare locks the actual destination; inspect it after transaction hooks and
// before commit. EOF aborts a refused transaction without writing the cache.
fn transaction(
    root: &Path,
    reference: &[u8],
    old: Option<&[u8]>,
    new: Option<&[u8]>,
    guard: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let mut diagnostics = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut child = Command::new("git")
        .current_dir(root)
        .args([
            "--no-lazy-fetch",
            "-c",
            "core.fsmonitor=false",
            "update-ref",
            "--no-deref",
            "--create-reflog",
            "-m",
            "twigsmith: verified remote cache",
            "--stdin",
            "-z",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(diagnostics.try_clone().map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().expect("piped input");
    let mut reader = io::BufReader::new(child.stdout.take().expect("piped output"));
    let attempt = (|| {
        stdin.write_all(b"start\0").map_err(|e| e.to_string())?;
        let zero = vec![b'0'; old.or(new).ok_or("missing cache object ID")?.len()];
        let operation = if let Some(new) = new {
            [
                b"update ".as_slice(),
                reference,
                b"\0",
                new,
                b"\0",
                old.unwrap_or(&zero),
                b"\0prepare\0",
            ]
            .concat()
        } else {
            [
                b"delete ".as_slice(),
                reference,
                b"\0",
                old.ok_or("missing previous cache version")?,
                b"\0prepare\0",
            ]
            .concat()
        };
        stdin.write_all(&operation).map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;
        loop {
            let mut line = vec![];
            if reader
                .read_until(b'\n', &mut line)
                .map_err(|e| e.to_string())?
                == 0
            {
                return Err("Git could not lock the cache transaction".into());
            }
            if line == b"prepare: ok\n" {
                break;
            }
        }
        guard()?;
        stdin.write_all(b"commit\0").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })();
    drop(stdin);
    let mut tail = vec![];
    let collected = reader.read_to_end(&mut tail);
    let status = child.wait().map_err(|e| e.to_string())?;
    diagnostics.rewind().map_err(|e| e.to_string())?;
    let mut stderr = String::new();
    diagnostics
        .read_to_string(&mut stderr)
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "cache transaction failed ({status}): {}",
            stderr.trim()
        ));
    }
    attempt?;
    collected.map_err(|e| e.to_string())?;
    Ok(())
}
