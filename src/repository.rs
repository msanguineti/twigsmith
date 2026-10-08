//! Read-only Git observations, independent of presentation and workflow policy.
//!
//! Inspection consists of several reads, not an atomic snapshot or a promise
//! that a later mutation is safe. Git's normal repository-selection environment
//! is honored. Names and repository-relative paths remain unquoted bytes.

use std::{
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Output,
};

use crate::git;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    OutsideRepository,
    Repository(Box<RepositoryState>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryContext {
    pub kind: RepositoryKind,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub working_tree_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryState {
    pub kind: RepositoryKind,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub head: Head,
    /// None for a bare repository; never interpreted as a clean working tree.
    pub working_tree: Option<WorkingTree>,
    pub operations: Operations,
    pub upstream: Upstream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryKind {
    Working,
    LinkedWorktree,
    Bare,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Attached { branch: Vec<u8>, commit: Vec<u8> },
    Detached { commit: Vec<u8> },
    Unborn { branch: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingTree {
    pub root: PathBuf,
    pub changes: Vec<PathChange>,
}

impl WorkingTree {
    pub fn is_dirty(&self) -> bool {
        !self.changes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathChange {
    /// Relative to the working tree root, including for nested invocation.
    pub path: Vec<u8>,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    Tracked {
        staged: Change,
        unstaged: Change,
        submodule: Option<SubmoduleState>,
        original_path: Option<Vec<u8>>,
    },
    /// Conflict XY describes merge sides, not staged/unstaged changes.
    Conflicted {
        xy: [u8; 2],
        submodule: Option<SubmoduleState>,
    },
    Untracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Unchanged,
    Modified,
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmoduleState {
    pub commit_changed: bool,
    pub tracked_changes: bool,
    pub untracked_changes: bool,
}

/// Independent facts: for example a rebase can use cherry-pick machinery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Operations {
    pub merge: bool,
    pub rebase: bool,
    pub cherry_pick: bool,
    pub revert: bool,
    pub am: bool,
    pub bisect: bool,
}

impl Operations {
    pub fn is_active(&self) -> bool {
        self.merge || self.rebase || self.cherry_pick || self.revert || self.am || self.bisect
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upstream {
    /// HEAD has no branch whose configuration can be inspected.
    NotApplicable,
    Absent,
    Configured {
        /// Preserve even incomplete configuration without inventing a default.
        remote: Option<Vec<u8>>,
        merge_refs: Vec<Vec<u8>>,
        /// Full local reference, when Git can resolve its tracking mapping.
        local_reference: Option<Vec<u8>>,
        reference: UpstreamReference,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamReference {
    Unavailable,
    Available {
        commit: Vec<u8>,
        /// Unavailable for unborn HEAD; counts use locally available history.
        divergence: Option<Divergence>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Divergence {
    pub ahead: u64,
    pub behind: u64,
}

#[derive(Debug)]
pub enum InspectionError {
    Io(io::Error),
    /// Retain raw diagnostics and OS exit status without terminal rendering.
    Git {
        arguments: Vec<OsString>,
        output: Output,
    },
    InvalidOutput(&'static str),
}

impl fmt::Display for InspectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "repository inspection I/O error: {error}"),
            Self::Git { output, .. } => {
                write!(f, "repository inspection Git exit: {}", output.status)
            }
            Self::InvalidOutput(context) => write!(f, "invalid Git inspection output: {context}"),
        }
    }
}

impl std::error::Error for InspectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for InspectionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

type Result<T> = std::result::Result<T, InspectionError>;

struct Inspector<'a>(&'a Path);

impl Inspector<'_> {
    fn run(&self, args: &[OsString]) -> Result<Output> {
        Ok(git::inspect(self.0, args)?)
    }

    fn checked(&self, args: &[OsString]) -> Result<Vec<u8>> {
        let output = self.run(args)?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(git_error(args, output))
        }
    }

    fn path(&self, args: &[&str]) -> Result<PathBuf> {
        let output = self.checked(&strings(args))?;
        Ok(PathBuf::from(os(line(&output)?)?))
    }

    fn present(&self, name: &str) -> Result<bool> {
        let path = self.path(&["rev-parse", "--path-format=absolute", "--git-path", name])?;
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn ref_exists(&self, reference: &[u8]) -> Result<bool> {
        let args = vec![
            "show-ref".into(),
            "--verify".into(),
            "--quiet".into(),
            os(reference)?,
        ];
        let output = self.run(&args)?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) if output.stderr.is_empty() => Ok(false),
            _ => Err(git_error(&args, output)),
        }
    }

    fn commit(&self, reference: &[u8]) -> Result<Vec<u8>> {
        let mut revision = reference.to_vec();
        revision.extend_from_slice(b"^{commit}");
        let output = self.checked(&[
            "rev-parse".into(),
            "--verify".into(),
            "--end-of-options".into(),
            os(&revision)?,
        ])?;
        let oid = line(&output)?;
        if !matches!(oid.len(), 40 | 64) || !oid.iter().all(u8::is_ascii_hexdigit) {
            return Err(InspectionError::InvalidOutput("commit object ID"));
        }
        Ok(oid.to_vec())
    }

    fn head(&self) -> Result<Head> {
        let args = strings(&["symbolic-ref", "--quiet", "HEAD"]);
        let output = self.run(&args)?;
        if output.status.success() {
            let reference = line(&output.stdout)?;
            let branch = reference
                .strip_prefix(b"refs/heads/")
                .ok_or(InspectionError::InvalidOutput("HEAD branch reference"))?
                .to_vec();
            if self.ref_exists(reference)? {
                Ok(Head::Attached {
                    branch,
                    commit: self.commit(reference)?,
                })
            } else {
                Ok(Head::Unborn { branch })
            }
        } else if output.status.code() == Some(1) && output.stderr.is_empty() {
            Ok(Head::Detached {
                commit: self.commit(b"HEAD")?,
            })
        } else {
            Err(git_error(&args, output))
        }
    }

    fn config(&self, branch: &[u8], suffix: &[u8]) -> Result<Vec<Vec<u8>>> {
        let mut key = b"branch.".to_vec();
        key.extend_from_slice(branch);
        key.extend_from_slice(suffix);
        let args = vec!["config".into(), "-z".into(), "--get-all".into(), os(&key)?];
        let output = self.run(&args)?;
        if output.status.code() == Some(1) && output.stderr.is_empty() {
            return Ok(Vec::new());
        }
        if !output.status.success() {
            return Err(git_error(&args, output));
        }
        Ok(records(&output.stdout)?.map(<[u8]>::to_vec).collect())
    }

    fn upstream(&self, head: &Head) -> Result<Upstream> {
        let (branch, head_commit) = match head {
            Head::Attached { branch, commit } => (branch, Some(commit)),
            Head::Unborn { branch } => (branch, None),
            Head::Detached { .. } => return Ok(Upstream::NotApplicable),
        };
        let remote = self.config(branch, b".remote")?.pop();
        let merge_refs = self.config(branch, b".merge")?;
        if remote.is_none() && merge_refs.is_empty() {
            return Ok(Upstream::Absent);
        }

        // rev-parse can resolve an unborn branch's upstream if its target exists.
        // for-each-ref also exposes mappings for an existing branch's gone target.
        let local_reference = if head_commit.is_some() {
            let mut reference = b"refs/heads/".to_vec();
            reference.extend_from_slice(branch);
            let output = self.checked(&[
                "for-each-ref".into(),
                "--format=%(refname)%00%(upstream)%00".into(),
                os(&reference)?,
            ])?;
            let mut found = None;
            for row in output.split(|b| *b == b'\n').filter(|row| !row.is_empty()) {
                let fields: Vec<_> = records(row)?.collect();
                if fields.len() != 2 {
                    return Err(InspectionError::InvalidOutput("upstream mapping"));
                }
                if fields[0] == reference && !fields[1].is_empty() {
                    found = Some(fields[1].to_vec());
                }
            }
            found
        } else if remote.as_deref() == Some(b".") {
            merge_refs.first().cloned()
        } else if remote.is_some() && !merge_refs.is_empty() {
            let args = strings(&[
                "rev-parse",
                "--symbolic-full-name",
                "--verify",
                "--quiet",
                "@{upstream}",
            ]);
            let output = self.run(&args)?;
            if output.status.success() {
                Some(line(&output.stdout)?.to_vec())
            } else if output.status.code() == Some(1) && output.stderr.is_empty() {
                None
            } else {
                return Err(git_error(&args, output));
            }
        } else {
            None
        };
        let reference = match &local_reference {
            Some(reference) if self.ref_exists(reference)? => {
                let commit = self.commit(reference)?;
                let divergence = if let Some(head_commit) = head_commit {
                    let mut range = head_commit.clone();
                    range.extend_from_slice(b"...");
                    range.extend_from_slice(&commit);
                    let output = self.checked(&[
                        "rev-list".into(),
                        "--left-right".into(),
                        "--count".into(),
                        os(&range)?,
                        "--".into(),
                    ])?;
                    let counts: Vec<_> = line(&output)?.split(|b| *b == b'\t').collect();
                    if counts.len() != 2 {
                        return Err(InspectionError::InvalidOutput("divergence"));
                    }
                    Some(Divergence {
                        ahead: number(counts[0])?,
                        behind: number(counts[1])?,
                    })
                } else {
                    None
                };
                UpstreamReference::Available { commit, divergence }
            }
            _ => UpstreamReference::Unavailable,
        };
        Ok(Upstream::Configured {
            remote,
            merge_refs,
            local_reference,
            reference,
        })
    }

    fn operations(&self) -> Result<Operations> {
        let mut operations = Operations {
            merge: self.present("MERGE_HEAD")?,
            rebase: self.present("rebase-merge")? || self.present("rebase-apply/rebasing")?,
            cherry_pick: self.present("CHERRY_PICK_HEAD")?,
            revert: self.present("REVERT_HEAD")?,
            am: self.present("rebase-apply/applying")?,
            bisect: self.present("BISECT_START")?,
        };
        let todo = self.path(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "sequencer/todo",
        ])?;
        match fs::read(todo) {
            Ok(contents) => {
                // The sequencer may remain after a conflict is committed manually.
                for line in contents.split(|b| *b == b'\n') {
                    operations.cherry_pick |= line.starts_with(b"pick ");
                    operations.revert |= line.starts_with(b"revert ");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(operations)
    }
}

/// Discover repository paths without reading HEAD, the index, or worktree state.
/// Git's normal repository-selection environment is honored.
pub fn discover(directory: impl AsRef<Path>) -> Result<Option<RepositoryContext>> {
    let inspector = Inspector(directory.as_ref());
    let args = strings(&["rev-parse", "--absolute-git-dir"]);
    let output = inspector.run(&args)?;
    if !output.status.success() {
        // Git has no dedicated exit code for non-repositories. Match only its
        // C-locale discovery diagnostic; unsafe ownership, malformed gitfiles,
        // permissions, invalid configuration, etc. remain errors.
        if output.status.code() == Some(128)
            && (output.stderr
                == b"fatal: not a git repository (or any of the parent directories): .git\n"
                || output
                    .stderr
                    .starts_with(b"fatal: not a git repository (or any parent up to mount point "))
        {
            return Ok(None);
        }
        return Err(git_error(&args, output));
    }
    let git_dir = PathBuf::from(os(line(&output.stdout)?)?);
    let common_dir =
        inspector.path(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let bare = inspector.checked(&strings(&["rev-parse", "--is-bare-repository"]))?;
    let kind = match bare.as_slice() {
        b"true\n" => RepositoryKind::Bare,
        b"false\n" if git_dir != common_dir => RepositoryKind::LinkedWorktree,
        b"false\n" => RepositoryKind::Working,
        _ => return Err(InspectionError::InvalidOutput("bare repository flag")),
    };
    let working_tree_root = if kind == RepositoryKind::Bare {
        None
    } else {
        Some(inspector.path(&["rev-parse", "--path-format=absolute", "--show-toplevel"])?)
    };
    Ok(Some(RepositoryContext {
        kind,
        git_dir,
        common_dir,
        working_tree_root,
    }))
}

/// Observe the repository selected by Git at `directory`, without fetching or
/// modifying it. Operational failures remain errors, never clean observations.
pub fn inspect(directory: impl AsRef<Path>) -> Result<Observation> {
    let mut observation = inspect_without_upstream(directory.as_ref())?;
    if let Observation::Repository(state) = &mut observation {
        state.upstream = Inspector(directory.as_ref()).upstream(&state.head)?;
    }
    Ok(observation)
}

/// Save validates raw upstream values itself before any tracking resolution.
/// Upstream is deliberately NotApplicable in this internal observation.
pub(crate) fn inspect_without_upstream(directory: &Path) -> Result<Observation> {
    let inspector = Inspector(directory);
    let Some(context) = discover(directory)? else {
        return Ok(Observation::OutsideRepository);
    };
    let RepositoryContext {
        kind,
        git_dir,
        common_dir,
        working_tree_root,
    } = context;
    let head = inspector.head()?;
    let working_tree = if let Some(root) = working_tree_root {
        let output = inspector.checked(&strings(&[
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--ignored=no",
            "--no-renames",
        ]))?;
        Some(WorkingTree {
            root,
            changes: parse_changes(&output)?,
        })
    } else {
        None
    };
    let operations = inspector.operations()?;
    let upstream = Upstream::NotApplicable;
    Ok(Observation::Repository(Box::new(RepositoryState {
        kind,
        git_dir,
        common_dir,
        head,
        working_tree,
        operations,
        upstream,
    })))
}

fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn git_error(args: &[OsString], output: Output) -> InspectionError {
    InspectionError::Git {
        arguments: args.to_vec(),
        output,
    }
}

fn line(output: &[u8]) -> Result<&[u8]> {
    output
        .strip_suffix(b"\n")
        .filter(|value| !value.is_empty())
        .ok_or(InspectionError::InvalidOutput("newline-terminated value"))
}

fn records(output: &[u8]) -> Result<impl Iterator<Item = &[u8]>> {
    let body = if output.is_empty() {
        output
    } else {
        output
            .strip_suffix(b"\0")
            .ok_or(InspectionError::InvalidOutput("NUL-terminated records"))?
    };
    Ok(body.split(|b| *b == 0).filter(move |_| !output.is_empty()))
}

pub(crate) fn os(bytes: &[u8]) -> Result<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        let value = std::str::from_utf8(bytes)
            .map_err(|_| InspectionError::InvalidOutput("OS string encoding"))?;
        Ok(OsString::from(value))
    }
}

fn number(value: &[u8]) -> Result<u64> {
    std::str::from_utf8(value)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(InspectionError::InvalidOutput("numeric value"))
}

fn change(value: u8) -> Result<Change> {
    match value {
        b'.' => Ok(Change::Unchanged),
        b'M' => Ok(Change::Modified),
        b'T' => Ok(Change::TypeChanged),
        b'A' => Ok(Change::Added),
        b'D' => Ok(Change::Deleted),
        b'R' => Ok(Change::Renamed),
        b'C' => Ok(Change::Copied),
        _ => Err(InspectionError::InvalidOutput("change code")),
    }
}

fn submodule(value: &[u8]) -> Result<Option<SubmoduleState>> {
    match value {
        b"N..." => Ok(None),
        [
            b'S',
            c @ (b'C' | b'.'),
            m @ (b'M' | b'.'),
            u @ (b'U' | b'.'),
        ] => Ok(Some(SubmoduleState {
            commit_changed: *c == b'C',
            tracked_changes: *m == b'M',
            untracked_changes: *u == b'U',
        })),
        _ => Err(InspectionError::InvalidOutput("submodule state")),
    }
}

fn parse_changes(output: &[u8]) -> Result<Vec<PathChange>> {
    let mut result = Vec::new();
    let mut rows = records(output)?;
    while let Some(row) = rows.next() {
        let (path, kind) = match row.first() {
            Some(b'#') => continue, // Porcelain v2 permits new headers.
            Some(b'?') => (
                row.strip_prefix(b"? ")
                    .ok_or(InspectionError::InvalidOutput("untracked entry"))?,
                ChangeKind::Untracked,
            ),
            Some(tag @ (b'1' | b'2' | b'u')) => {
                let count = match tag {
                    b'1' => 9,
                    b'2' => 10,
                    _ => 11,
                };
                // Split only the fixed prefix; spaces/newlines in paths are data.
                let fields: Vec<_> = row.splitn(count, |b| *b == b' ').collect();
                if fields.len() != count || fields[0] != [*tag] || fields[1].len() != 2 {
                    return Err(InspectionError::InvalidOutput("tracked entry"));
                }
                let xy = [fields[1][0], fields[1][1]];
                let submodule = submodule(fields[2])?;
                let kind = if *tag == b'u' {
                    if !matches!(&xy, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU") {
                        return Err(InspectionError::InvalidOutput("conflict code"));
                    }
                    ChangeKind::Conflicted { xy, submodule }
                } else {
                    let original_path = if *tag == b'2' {
                        Some(
                            rows.next()
                                .filter(|path| !path.is_empty())
                                .ok_or(InspectionError::InvalidOutput("original path"))?
                                .to_vec(),
                        )
                    } else {
                        None
                    };
                    ChangeKind::Tracked {
                        staged: change(xy[0])?,
                        unstaged: change(xy[1])?,
                        submodule,
                        original_path,
                    }
                };
                (fields[count - 1], kind)
            }
            _ => return Err(InspectionError::InvalidOutput("status record type")),
        };
        if path.is_empty() {
            return Err(InspectionError::InvalidOutput("empty path"));
        }
        result.push(PathChange {
            path: path.to_vec(),
            kind,
        });
    }
    // Git does not guarantee record order. Keep observations deterministic.
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_status_is_an_error() {
        for input in [
            b"? missing terminator".as_slice(),
            b"1 M\0",
            b"! ignored\0",
            b"? \0",
            b"u ZZ N... 0 0 0 0 a b c file\0",
        ] {
            assert!(parse_changes(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn rename_paths_and_unknown_headers_are_consumed_safely() {
        let changes = parse_changes(
            b"# future.header value\0\
            2 R. N... 100644 100644 100644 a b R100 new\n name\0old\n name\0? next file\0",
        )
        .unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, b"new\n name");
        assert!(
            matches!(&changes[0].kind, ChangeKind::Tracked { staged: Change::Renamed, original_path: Some(path), .. } if path == b"old\n name")
        );
        assert_eq!(changes[1].path, b"next file");
    }
}
