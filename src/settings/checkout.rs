//! Configuration transitions are selected by an exact commit tree, rather than
//! by newly loaded settings. Untracked files and the private layer retain their
//! original content; checkout does not authorize changing either of them.

use std::{fs, io, path::Path};

use super::{CONFIG_FILES, SettingsError};
use crate::{
    git,
    repository::{InspectionError, RepositoryContext},
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct File {
    mode: Vec<u8>,
    bytes: Vec<u8>,
    symlink_contents: Option<Vec<u8>>,
}

fn files(root: &Path) -> Result<Vec<Option<File>>, SettingsError> {
    CONFIG_FILES
        .iter()
        .map(|name| {
            let path = root.join(name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(source) => return Err(SettingsError::SettingsIo { path, source }),
            };
            let read_error = |source| SettingsError::SettingsIo {
                path: path.clone(),
                source,
            };
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&path).map_err(read_error)?;
                #[cfg(unix)]
                let bytes = {
                    use std::os::unix::ffi::OsStrExt;
                    target.as_os_str().as_bytes().to_vec()
                };
                #[cfg(not(unix))]
                let bytes = target.to_string_lossy().as_bytes().to_vec();
                return Ok(Some(File {
                    mode: b"120000".to_vec(),
                    bytes,
                    symlink_contents: Some(fs::read(&path).map_err(read_error)?),
                }));
            }
            if !metadata.is_file() {
                return Err(SettingsError::InvalidSettings(format!(
                    "{} is not a configuration file",
                    path.display()
                )));
            }
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            Ok(Some(File {
                mode: if executable { b"100755" } else { b"100644" }.to_vec(),
                bytes: fs::read(&path).map_err(read_error)?,
                symlink_contents: None,
            }))
        })
        .collect()
}

fn tree_files(directory: &Path, commit: &str) -> Result<Vec<Option<File>>, SettingsError> {
    let mut arguments = vec![
        "--no-replace-objects",
        "ls-tree",
        "-z",
        "--full-tree",
        commit,
        "--",
    ];
    arguments.extend(CONFIG_FILES);
    let output = git::inspect(directory, &arguments).map_err(InspectionError::Io)?;
    if !output.status.success() {
        return Err(InspectionError::Git {
            arguments: arguments.iter().map(Into::into).collect(),
            output,
        }
        .into());
    }
    let mut result = vec![None; CONFIG_FILES.len()];
    for entry in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let separator = entry
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or(InspectionError::InvalidOutput("configuration tree entry"))?;
        let (header, path) = (&entry[..separator], &entry[separator + 1..]);
        let fields: Vec<_> = header.split(|byte| *byte == b' ').collect();
        if fields.len() != 3 || fields[1] != b"blob" {
            return Err(SettingsError::InvalidSettings(
                "tracked configuration must be a file".into(),
            ));
        }
        let index = CONFIG_FILES
            .iter()
            .position(|name| name.as_bytes() == path)
            .ok_or(InspectionError::InvalidOutput("configuration tree path"))?;
        let oid = std::str::from_utf8(fields[2])
            .map_err(|_| InspectionError::InvalidOutput("configuration blob ID"))?;
        let arguments = ["--no-replace-objects", "cat-file", "blob", oid];
        let blob = git::inspect(directory, arguments).map_err(InspectionError::Io)?;
        if !blob.status.success() {
            return Err(InspectionError::Git {
                arguments: arguments.iter().map(Into::into).collect(),
                output: blob,
            }
            .into());
        }
        result[index] = Some(File {
            mode: fields[0].to_vec(),
            bytes: blob.stdout,
            symlink_contents: None,
        });
    }
    Ok(result)
}

fn same_tree_file(actual: &Option<File>, expected: &Option<File>) -> bool {
    match (actual, expected) {
        (None, None) => true,
        (Some(actual), Some(expected)) => {
            actual.mode == expected.mode && actual.bytes == expected.bytes
        }
        _ => false,
    }
}

fn matches_tree_file(
    directory: &Path,
    commit: Option<&str>,
    index: usize,
    actual: &Option<File>,
    expected: &Option<File>,
) -> Result<bool, SettingsError> {
    if same_tree_file(actual, expected) {
        return Ok(true);
    }
    let (Some(actual), Some(expected)) = (actual, expected) else {
        return Ok(false);
    };
    if actual.mode != expected.mode || expected.mode == b"120000" || expected.bytes.contains(&b'\r')
    {
        return Ok(false);
    }
    // Git's built-in line-ending conversion needs no external filter execution.
    // Keep custom smudge/encoding transformations outside this exact-byte guard.
    let mut arguments = vec!["--no-replace-objects", "check-attr", "-z"];
    if let Some(commit) = commit {
        arguments.extend(["--source", commit]);
    } else {
        arguments.push("--cached");
    }
    arguments.extend(["text", "eol", "--", CONFIG_FILES[index]]);
    let output = git::inspect(directory, &arguments).map_err(InspectionError::Io)?;
    if !output.status.success() {
        return Err(InspectionError::Git {
            arguments: arguments.iter().map(Into::into).collect(),
            output,
        }
        .into());
    }
    let fields: Vec<_> = output.stdout.split(|byte| *byte == 0).collect();
    if fields.len() != 7 {
        return Err(InspectionError::InvalidOutput("configuration line-ending attributes").into());
    }
    let text = fields[2];
    let eol = fields[5];
    let autocrlf = crate::tracking::config(directory, b"core.autocrlf")?;
    let autocrlf = autocrlf.last().map(|value| value.to_ascii_lowercase());
    let crlf = if text == b"unset" || eol == b"lf" {
        false
    } else if eol == b"crlf" {
        true
    } else if autocrlf.as_deref() == Some(b"input") {
        false
    } else if autocrlf
        .as_deref()
        .is_some_and(|value| matches!(value, b"true" | b"yes" | b"on" | b"1"))
    {
        true
    } else {
        let eol = crate::tracking::config(directory, b"core.eol")?;
        (text == b"set" || text == b"auto")
            && (eol.last().is_some_and(|value| value == b"crlf")
                || (cfg!(windows) && eol.last().is_none_or(|value| value == b"native")))
    };
    if !crlf {
        return Ok(false);
    }
    let mut bytes = Vec::with_capacity(expected.bytes.len());
    for byte in &expected.bytes {
        if *byte == b'\n' {
            bytes.push(b'\r')
        }
        bytes.push(*byte);
    }
    Ok(actual.bytes == bytes)
}

/// Short-lived checkout guard. Only paths whose committed identity changes
/// may adopt the target tree; all other shared and private paths stay fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    shared: Vec<Option<File>>,
    private: Vec<Option<File>>,
    tree: Vec<Option<File>>,
}

impl Snapshot {
    /// A retained regular shared layer may cross trees only when the observed
    /// files are explained by the current commit. Symlink referents are external
    /// to its tree and cannot establish this retained configuration binding.
    pub(crate) fn shared_matches_tree(
        &self,
        repository: &RepositoryContext,
        commit: &str,
    ) -> Result<bool, SettingsError> {
        let Some(root) = repository.working_tree_root.as_deref() else {
            return Ok(false);
        };
        for (index, actual) in self.shared.iter().enumerate() {
            if actual
                .as_ref()
                .is_some_and(|file| file.symlink_contents.is_some())
                || !matches_tree_file(root, Some(commit), index, actual, &self.tree[index])?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(crate) fn capture(
        _directory: &Path,
        repository: &RepositoryContext,
        commit: &str,
    ) -> Result<Self, SettingsError> {
        let root = repository.working_tree_root.as_deref().ok_or_else(|| {
            SettingsError::InvalidSettings("configuration checkout requires a worktree".into())
        })?;
        Ok(Self {
            shared: files(root)?,
            private: files(&repository.common_dir)?,
            tree: tree_files(root, commit)?,
        })
    }

    /// A stage whose HEAD has not changed needs only the recognized files,
    /// including ignored shared files and the private layer. No tree query is
    /// needed to establish that the already accepted checkout is unchanged.
    pub(crate) fn matches_files(
        &self,
        repository: &RepositoryContext,
    ) -> Result<bool, SettingsError> {
        let Some(root) = repository.working_tree_root.as_deref() else {
            return Ok(false);
        };
        Ok(files(root)? == self.shared && files(&repository.common_dir)? == self.private)
    }

    /// Accept a Git checkout's tracked configuration transition once, then use
    /// its actual files as the next stage baseline. The effective settings stay
    /// pinned to the caller's original choice.
    pub(crate) fn advance_checkout(
        &self,
        directory: &Path,
        repository: &RepositoryContext,
        commit: &str,
    ) -> Result<Option<Self>, SettingsError> {
        let Some(root) = repository.working_tree_root.as_deref() else {
            return Ok(None);
        };
        let private = files(&repository.common_dir)?;
        if private != self.private {
            return Ok(None);
        }
        let target = tree_files(directory, commit)?;
        let shared = files(root)?;
        for (index, actual) in shared.iter().enumerate() {
            if self.tree[index] == target[index] {
                if actual != &self.shared[index] {
                    return Ok(None);
                }
            } else if !matches_tree_file(root, Some(commit), index, actual, &target[index])? {
                return Ok(None);
            }
        }
        Ok(Some(Self {
            shared,
            private,
            tree: target,
        }))
    }

    pub(crate) fn matches_checkout(
        &self,
        directory: &Path,
        repository: &RepositoryContext,
        commit: &str,
    ) -> Result<bool, SettingsError> {
        Ok(self
            .advance_checkout(directory, repository, commit)?
            .is_some())
    }

    /// An authorized squash stages its candidate before moving HEAD. Verify its
    /// changed shared files against the exact stage-zero blobs, retaining the
    /// private layer and shared paths that the staged result does not change.
    pub(crate) fn matches_index(
        &self,
        repository: &RepositoryContext,
    ) -> Result<bool, SettingsError> {
        let Some(root) = repository.working_tree_root.as_deref() else {
            return Ok(false);
        };
        if files(&repository.common_dir)? != self.private {
            return Ok(false);
        }
        let mut arguments = vec!["ls-files", "--stage", "-z", "--"];
        arguments.extend(CONFIG_FILES);
        let output = git::inspect(root, &arguments).map_err(InspectionError::Io)?;
        if !output.status.success() {
            return Err(InspectionError::Git {
                arguments: arguments.iter().map(Into::into).collect(),
                output,
            }
            .into());
        }
        let mut target = vec![None; CONFIG_FILES.len()];
        for entry in output.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let Some(tab) = entry.iter().position(|b| *b == b'\t') else {
                return Err(InspectionError::InvalidOutput("configuration index entry").into());
            };
            let fields: Vec<_> = entry[..tab].split(|b| *b == b' ').collect();
            if fields.len() != 3 || fields[2] != b"0" {
                return Ok(false);
            }
            let index = CONFIG_FILES
                .iter()
                .position(|name| name.as_bytes() == &entry[tab + 1..])
                .ok_or(InspectionError::InvalidOutput("configuration index path"))?;
            let oid = std::str::from_utf8(fields[1])
                .map_err(|_| InspectionError::InvalidOutput("configuration index blob"))?;
            let arguments = ["--no-replace-objects", "cat-file", "blob", oid];
            let blob = git::inspect(root, arguments).map_err(InspectionError::Io)?;
            if !blob.status.success() {
                return Err(InspectionError::Git {
                    arguments: arguments.iter().map(Into::into).collect(),
                    output: blob,
                }
                .into());
            }
            target[index] = Some(File {
                mode: fields[0].to_vec(),
                bytes: blob.stdout,
                symlink_contents: None,
            });
        }
        for (index, actual) in files(root)?.iter().enumerate() {
            if self.tree[index] == target[index] {
                if actual != &self.shared[index] {
                    return Ok(false);
                }
            } else if !matches_tree_file(root, None, index, actual, &target[index])? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
