//! Shared branch relationships and explicit Git push-selection guards.
//! Raw values are retained for conservative revalidation; Git's scalar push
//! settings use their last value and branch.pushRemote overrides pushDefault.
use crate::{
    git,
    repository::{self, InspectionError},
    settings::Settings,
};
use std::{fmt, path::Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamSettings {
    pub remotes: Vec<Vec<u8>>,
    pub merge_refs: Vec<Vec<u8>>,
}
impl UpstreamSettings {
    pub(crate) fn absent(&self) -> bool {
        self.remotes.is_empty() && self.merge_refs.is_empty()
    }
    pub(crate) fn valid(&self, settings: &Settings, branch: &[u8]) -> bool {
        self.absent()
            || self.matches(&settings.publish_remote, branch)
            || self.matches(&settings.base_remote, settings.base.as_bytes())
    }
    fn matches(&self, remote: &str, branch: &[u8]) -> bool {
        self.remotes == [remote.as_bytes()]
            && self.merge_refs == [[b"refs/heads/".as_slice(), branch].concat()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub upstream: UpstreamSettings,
    pub push_remote: Vec<Vec<u8>>,
    pub push_default: Vec<Vec<u8>>,
}
impl Snapshot {
    pub(crate) fn validate(&self, settings: &Settings, branch: &[u8]) -> Result<(), Error> {
        if !self.upstream.valid(settings, branch) {
            return Err(Error::InvalidUpstream);
        }
        let effective = self.push_remote.last().or_else(|| self.push_default.last());
        if effective.is_some_and(|value| value != settings.publish_remote.as_bytes()) {
            return Err(Error::ConflictingPushRemote);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidUpstream,
    ConflictingPushRemote,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUpstream => write!(
                f,
                "upstream settings must be absent or one exact publication-branch or project-base relationship; incomplete, duplicate, or unrelated values require repair"
            ),
            Self::ConflictingPushRemote => write!(
                f,
                "effective explicit Git push remote conflicts with publish-remote; inspect branch.<name>.pushRemote and remote.pushDefault"
            ),
        }
    }
}
impl std::error::Error for Error {}

pub(crate) fn key(branch: &[u8], suffix: &[u8]) -> Vec<u8> {
    [b"branch.".as_slice(), branch, suffix].concat()
}
pub(crate) fn config(directory: &Path, key: &[u8]) -> Result<Vec<Vec<u8>>, InspectionError> {
    let args = vec![
        "config".into(),
        "-z".into(),
        "--get-all".into(),
        repository::os(key)?,
    ];
    let output = git::inspect(directory, &args)?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(vec![]);
    }
    if !output.status.success() {
        return Err(InspectionError::Git {
            arguments: args,
            output,
        });
    }
    let bytes = output
        .stdout
        .strip_suffix(b"\0")
        .ok_or(InspectionError::InvalidOutput("branch configuration"))?;
    Ok(bytes.split(|b| *b == 0).map(<[u8]>::to_vec).collect())
}
pub(crate) fn upstream(
    directory: &Path,
    branch: &[u8],
) -> Result<UpstreamSettings, InspectionError> {
    Ok(UpstreamSettings {
        remotes: config(directory, &key(branch, b".remote"))?,
        merge_refs: config(directory, &key(branch, b".merge"))?,
    })
}
pub(crate) fn snapshot(directory: &Path, branch: &[u8]) -> Result<Snapshot, InspectionError> {
    Ok(Snapshot {
        upstream: upstream(directory, branch)?,
        push_remote: config(directory, &key(branch, b".pushRemote"))?,
        push_default: config(directory, b"remote.pushDefault")?,
    })
}
