//! Shared branch relationships and explicit Git push-selection guards.
//! Raw values are retained for conservative revalidation; Git's scalar push
//! settings use their last value and branch.pushRemote overrides pushDefault.
use crate::{
    git,
    repository::{self, InspectionError},
    settings::Settings,
};
use std::{fmt, path::Path};

pub(crate) mod cache;

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
            || settings.workstream.as_ref().is_some_and(|policy| {
                self.matches(&settings.base_remote, policy.preparation_source.as_bytes())
            })
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
    pub policy_association: Vec<Vec<u8>>,
}
impl Snapshot {
    pub(crate) fn from_config(config: &[u8], branch: &[u8]) -> Result<Self, InspectionError> {
        if !config.is_empty() && !config.ends_with(b"\0") {
            return Err(InspectionError::InvalidOutput("Git configuration records"));
        }
        // Git config --null --list normalizes section/variable names while
        // retaining subsection bytes, includes and duplicate-value order.
        let values = |key: &[u8]| {
            config
                .split(|b| *b == 0)
                .filter_map(|entry| {
                    let split = entry.iter().position(|b| *b == b'\n');
                    let (name, value) =
                        split.map_or((entry, b"".as_slice()), |i| (&entry[..i], &entry[i + 1..]));
                    (name == key).then(|| value.to_vec())
                })
                .collect()
        };
        Ok(Self {
            upstream: UpstreamSettings {
                remotes: values(&key(branch, b".remote")),
                merge_refs: values(&key(branch, b".merge")),
            },
            push_remote: values(&key(branch, b".pushremote")),
            push_default: values(b"remote.pushdefault"),
            policy_association: values(&key(branch, b".twigsmith-policy")),
        })
    }
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
    let arguments = vec!["config".into(), "--null".into(), "--list".into()];
    let output = git::inspect(directory, &arguments)?;
    if !output.status.success() {
        return Err(InspectionError::Git { arguments, output });
    }
    Snapshot::from_config(&output.stdout, branch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsString,
        process::{Command, Output},
    };

    #[test]
    fn batched_snapshot_matches_native_key_reads_with_includes_duplicates_and_literal_subsections()
    {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config");
        let included = directory.path().join("included");
        std::fs::write(&path, "").unwrap();
        std::fs::write(&included, "").unwrap();
        let run = |path: &Path, args: &[OsString]| -> Output {
            Command::new("git")
                .args(["config", "--file"])
                .arg(path)
                .arg("--includes")
                .args(args)
                .output()
                .unwrap()
        };
        let add = |path: &Path, key: &[u8], value: &str| {
            let output = run(
                path,
                &["--add".into(), repository::os(key).unwrap(), value.into()],
            );
            assert!(output.status.success(), "{output:?}");
        };
        let mut branches = vec![b"Feature/@Cafe\xc3\xa9/\"Odd".to_vec(), b"absent".to_vec()];
        #[cfg(unix)]
        branches.push(b"Topic/non-UTF8-\xfe".to_vec());
        for branch in branches.iter().filter(|b| b.as_slice() != b"absent") {
            for suffix in [
                b".ReMoTe".as_slice(),
                b".MeRgE",
                b".PuShReMoTe",
                b".twigsmith-policy",
            ] {
                let mut mixed_key = key(branch, suffix);
                mixed_key[..6].copy_from_slice(b"BrAnCh");
                add(&path, &mixed_key, "first\nvalue");
                add(&included, &mixed_key, "");
            }
        }
        add(&path, b"include.path", included.to_str().unwrap());
        add(&path, b"ReMoTe.PuShDeFaUlT", "first");
        add(&path, b"remote.pushDefault", "last");
        let raw = run(&path, &["--null".into(), "--list".into()]);
        assert!(raw.status.success(), "{raw:?}");
        for branch in branches {
            let values = |key: &[u8]| {
                let output = run(
                    &path,
                    &[
                        "--null".into(),
                        "--get-all".into(),
                        repository::os(key).unwrap(),
                    ],
                );
                if output.status.code() == Some(1) {
                    return vec![];
                }
                assert!(output.status.success(), "{output:?}");
                output
                    .stdout
                    .strip_suffix(b"\0")
                    .unwrap()
                    .split(|b| *b == 0)
                    .map(<[u8]>::to_vec)
                    .collect()
            };
            let expected = Snapshot {
                upstream: UpstreamSettings {
                    remotes: values(&key(&branch, b".remote")),
                    merge_refs: values(&key(&branch, b".merge")),
                },
                push_remote: values(&key(&branch, b".pushRemote")),
                push_default: values(b"remote.pushDefault"),
                policy_association: values(&key(&branch, b".twigsmith-policy")),
            };
            assert_eq!(
                Snapshot::from_config(&raw.stdout, &branch).unwrap(),
                expected
            );
        }
    }
}
