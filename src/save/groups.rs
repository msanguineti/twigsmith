//! Immutable group trees and native-compatible stash evidence. Alternate indexes
//! never replace the user's index while separability is being established.
use super::{Plan, SaveError, read};
use crate::{repository, settings::SaveScope};
use std::{
    ffi::OsString,
    fs,
    path::Path,
    process::{Command, Output},
};

pub(super) struct Index {
    file: tempfile::NamedTempFile,
    hooks: tempfile::TempDir,
}
impl Index {
    pub fn new(plan: &Plan) -> Result<Self, SaveError> {
        let file = tempfile::Builder::new()
            .prefix("gof-save-index-")
            .tempfile_in(&plan.repository.git_dir)?;
        if let Some(bytes) = &plan.snapshot.index {
            fs::write(file.path(), bytes)?;
        } else {
            fs::remove_file(file.path())?;
        }
        let hooks = tempfile::tempdir()?;
        let index = Self { file, hooks };
        // Resolve the repository using the inherited selection environment;
        // override only this private index, never HOME or repository selection.
        Ok(index)
    }
    pub fn run(&self, directory: &Path, args: &[OsString]) -> Result<Output, SaveError> {
        let mut command = Command::new("git");
        let mut hooks = OsString::from("core.hooksPath=");
        hooks.push(self.hooks.path());
        command
            .current_dir(directory)
            .args(["--no-lazy-fetch", "-c", "core.fsmonitor=false"])
            .arg("-c")
            .arg(hooks)
            .args(args)
            .env("GIT_INDEX_FILE", self.file.path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        Ok(crate::activity::capture(&mut command)?)
    }
    pub fn read(&self, directory: &Path, args: &[OsString]) -> Result<Vec<u8>, SaveError> {
        let output = self.run(directory, args)?;
        if !output.status.success() {
            return Err(repository::InspectionError::Git {
                arguments: args.to_vec(),
                output,
            }
            .into());
        }
        Ok(output.stdout)
    }
    pub fn tree(&self, directory: &Path) -> Result<Vec<u8>, SaveError> {
        oid(self.read(directory, &["write-tree".into()])?)
    }
    fn set(&self, directory: &Path, tree: &[u8]) -> Result<(), SaveError> {
        self.read(directory, &["read-tree".into(), repository::os(tree)?])?;
        Ok(())
    }
}
fn oid(bytes: Vec<u8>) -> Result<Vec<u8>, SaveError> {
    Ok(bytes
        .strip_suffix(b"\n")
        .ok_or(repository::InspectionError::InvalidOutput("save object id"))?
        .to_vec())
}
fn tree_of(directory: &Path, revision: &[u8]) -> Result<Vec<u8>, SaveError> {
    oid(read(
        directory,
        &[
            "rev-parse".into(),
            repository::os(&[revision, b"^{tree}"].concat())?,
        ],
    )?)
}

pub(super) struct Trees {
    /// Masks select the original staged bit (1) and unstaged bit (2).
    pub all: [Option<Vec<u8>>; 4],
    pub tracked_work: Vec<u8>,
    pub untracked_tree: Option<Vec<u8>>,
    pub index: Index,
}
impl Trees {
    pub fn capture(directory: &Path, plan: &Plan) -> Result<Self, SaveError> {
        let index = Index::new(plan)?;
        let head = tree_of(directory, &plan.commit)?;
        let staged = index.tree(directory)?;
        // A byte copy has a newer mtime than the original index and would hide
        // Git's racy-index protection for same-size, same-timestamp edits. Read
        // the original staged tree into a fresh index with no borrowed stat
        // cache before hashing working files.
        fs::remove_file(index.file.path())?;
        index.set(directory, &staged)?;
        if !plan.scope.remaining_tracked.is_empty() {
            let mut args: Vec<OsString> = ["--literal-pathspecs", "add", "--all", "--"]
                .into_iter()
                .map(Into::into)
                .collect();
            for path in &plan.scope.remaining_tracked {
                args.push(repository::os(path)?);
            }
            index.read(directory, &args)?;
        }
        let tracked = index.tree(directory)?;
        if !plan.scope.untracked.is_empty() {
            let mut args: Vec<OsString> = ["--literal-pathspecs", "add", "--all", "--"]
                .into_iter()
                .map(Into::into)
                .collect();
            for path in &plan.scope.untracked {
                args.push(repository::os(path)?);
            }
            index.read(directory, &args)?;
        }
        let all = index.tree(directory)?;
        let untracked_files: Vec<Vec<u8>> = read(
            directory,
            &[
                "ls-files".into(),
                "--others".into(),
                "--exclude-standard".into(),
                "-z".into(),
            ],
        )?
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
        let untracked_tree = if untracked_files.is_empty() {
            None
        } else {
            index.read(directory, &["read-tree".into(), "--empty".into()])?;
            let mut args: Vec<OsString> = ["--literal-pathspecs", "add", "--all", "--"]
                .into_iter()
                .map(Into::into)
                .collect();
            for path in &untracked_files {
                args.push(repository::os(path)?);
            }
            index.read(directory, &args)?;
            Some(index.tree(directory)?)
        };
        Ok(Self {
            all: [Some(head.clone()), Some(staged.clone()), None, Some(all)],
            tracked_work: tracked,
            untracked_tree,
            index,
        })
    }
    pub fn get(&self, mask: usize) -> &[u8] {
        self.all[mask]
            .as_deref()
            .expect("preflight prepared required tree")
    }
    /// Derive with Git's builtin text merger and no user attributes/drivers.
    /// Recompose the original staged delta afterward and demand the exact
    /// original whole-work tree, including modes, additions and literal paths.
    pub fn separate(&mut self, directory: &Path, plan: &Plan) -> Result<(), SaveError> {
        if self.get(0) == self.get(1) {
            self.all[2] = self.all[3].clone();
            return Ok(());
        }
        let empty = {
            self.index
                .read(directory, &["read-tree".into(), "--empty".into()])?;
            self.index.tree(directory)?
        };
        let scratch = Scratch::new(directory, &empty)?;
        let apply = |base: &[u8], from: &[u8], to: &[u8]| -> Result<Vec<u8>, SaveError> {
            let patch = read(
                directory,
                &[
                    "-c".into(),
                    "diff.autoRefreshIndex=false".into(),
                    "diff".into(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                    "--binary".into(),
                    "--full-index".into(),
                    "--no-renames".into(),
                    "--no-relative".into(),
                    "--submodule=short".into(),
                    "--ignore-submodules=none".into(),
                    repository::os(from)?,
                    repository::os(to)?,
                    "--".into(),
                ],
            )?;
            self.index.set(directory, base)?;
            if !patch.is_empty() {
                let patch_file = tempfile::Builder::new()
                    .prefix("gof-save-delta-")
                    .tempfile_in(&plan.repository.git_dir)?;
                fs::write(patch_file.path(), patch)?;
                let output = scratch.apply(directory, &self.index, patch_file.path())?;
                if !output.status.success() {
                    return Err(SaveError::Inseparable);
                }
            }
            self.index
                .tree(directory)
                .map_err(|_| SaveError::Inseparable)
        };
        let independent = apply(self.get(0), self.get(1), self.get(3))?;
        let recomposed = apply(&independent, self.get(0), self.get(1))?;
        if recomposed != self.get(3) {
            return Err(SaveError::Inseparable);
        }
        self.all[2] = Some(independent);
        Ok(())
    }
    pub fn stash_object(
        &self,
        directory: &Path,
        plan: &Plan,
        scope: SaveScope,
        message: &str,
    ) -> Result<Vec<u8>, SaveError> {
        let (base, index_tree, work_tree, untracked) = match scope {
            SaveScope::All => (
                plan.commit.clone(),
                self.get(1),
                self.tracked_work.as_slice(),
                true,
            ),
            SaveScope::Staged => (plan.commit.clone(), self.get(1), self.get(1), false),
            SaveScope::Unstaged => (
                commit_tree(
                    directory,
                    self.get(1),
                    &[&plan.commit],
                    "save unstaged base",
                )?,
                self.get(1),
                self.tracked_work.as_slice(),
                true,
            ),
        };
        let index = commit_tree(directory, index_tree, &[&base], "save original index")?;
        let mut parents: Vec<&[u8]> = vec![&base, &index];
        let extra = if untracked {
            self.untracked_tree
                .as_ref()
                .map(|tree| commit_tree(directory, tree, &[], "save untracked files"))
                .transpose()?
        } else {
            None
        };
        if let Some(extra) = &extra {
            parents.push(extra);
        }
        commit_tree(directory, work_tree, &parents, message)
    }
}
/// Only the mathematical three-way application uses this empty repository.
/// It shares the selected object store and format, but has no attributes,
/// hooks, refs or custom merge-driver policy. It never selects a worktree for
/// a workflow effect; native commits/stash storage retain the real policies.
struct Scratch {
    directory: tempfile::TempDir,
    objects: OsString,
    empty: Vec<u8>,
}
impl Scratch {
    fn new(root: &Path, empty: &[u8]) -> Result<Self, SaveError> {
        let directory = tempfile::tempdir()?;
        let format = read(root, &["rev-parse".into(), "--show-object-format".into()])?;
        let objects = read(
            root,
            &[
                "rev-parse".into(),
                "--path-format=absolute".into(),
                "--git-path".into(),
                "objects".into(),
            ],
        )?;
        let mut command = Command::new("git");
        command
            .current_dir(root)
            .args([
                "--no-lazy-fetch",
                "init",
                "--bare",
                "--quiet",
                "--template=",
            ])
            .arg(format!(
                "--object-format={}",
                String::from_utf8_lossy(
                    format
                        .strip_suffix(b"\n")
                        .ok_or(repository::InspectionError::InvalidOutput("object format"))?
                )
            ))
            .arg("--")
            .arg(directory.path());
        for key in [
            "GIT_DIR",
            "GIT_COMMON_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        ] {
            command.env_remove(key);
        }
        let output = crate::activity::capture(&mut command)?;
        if !output.status.success() {
            return Err(repository::InspectionError::Git {
                arguments: vec!["init isolated save derivation".into()],
                output,
            }
            .into());
        }
        fs::write(directory.path().join("empty-attributes"), b"")?;
        fs::create_dir(directory.path().join("empty-hooks"))?;
        Ok(Self {
            directory,
            objects: repository::os(
                objects
                    .strip_suffix(b"\n")
                    .ok_or(repository::InspectionError::InvalidOutput("objects path"))?,
            )?,
            empty: empty.to_vec(),
        })
    }
    fn apply(&self, root: &Path, index: &Index, patch: &Path) -> Result<Output, SaveError> {
        let mut attrs = OsString::from("core.attributesFile=");
        attrs.push(self.directory.path().join("empty-attributes"));
        let mut hooks = OsString::from("core.hooksPath=");
        hooks.push(self.directory.path().join("empty-hooks"));
        let mut attr_source = OsString::from("--attr-source=");
        attr_source.push(repository::os(&self.empty)?);
        let mut command = Command::new("git");
        command
            .current_dir(root)
            .arg("--no-lazy-fetch")
            .arg("--git-dir")
            .arg(self.directory.path())
            .arg(attr_source)
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "merge.default=text",
                "-c",
                "apply.ignoreWhitespace=false",
                "-c",
                "apply.whitespace=nowarn",
            ])
            .arg("-c")
            .arg(attrs)
            .arg("-c")
            .arg(hooks)
            .args(["apply", "--cached", "--3way", "--binary", "--"])
            .arg(patch)
            .env("GIT_INDEX_FILE", index.file.path())
            .env("GIT_OBJECT_DIRECTORY", &self.objects)
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        for key in ["GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_NAMESPACE"] {
            command.env_remove(key);
        }
        Ok(crate::activity::capture(&mut command)?)
    }
}

pub(super) fn commit_tree(
    directory: &Path,
    tree: &[u8],
    parents: &[&[u8]],
    message: &str,
) -> Result<Vec<u8>, SaveError> {
    let mut args = vec!["commit-tree".into(), repository::os(tree)?];
    for parent in parents {
        args.extend(["-p".into(), repository::os(parent)?]);
    }
    args.extend(["-m".into(), message.into()]);
    let output = crate::git::workflow(directory, &args)?;
    if !output.status.success() {
        return Err(repository::InspectionError::Git {
            arguments: args,
            output,
        }
        .into());
    }
    oid(output.stdout)
}

/// `read-tree --reset -u` may remove ignored obstructions when restoring a
/// tracked file, including an ignored directory or a symlink in its parent
/// path. Bind protection to the desired leaf paths and re-read ignored names
/// before effects and immediately before that checkout. Scope the scan to
/// relevant top-level paths so unrelated ignored build/tool caches stay outside
/// this guard; do not request Git's collapsed-directory output.
pub(super) struct CheckoutProtection {
    paths: Vec<Vec<u8>>,
    roots: Vec<Vec<u8>>,
}
impl CheckoutProtection {
    pub fn new(root: &Path, target: &[u8]) -> Result<Self, SaveError> {
        let paths = read(
            root,
            &[
                "ls-tree".into(),
                "-r".into(),
                "--name-only".into(),
                "--full-tree".into(),
                "--full-name".into(),
                "-z".into(),
                repository::os(target)?,
            ],
        )?
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
        let roots = paths
            .iter()
            .map(|path| path.split(|byte| *byte == b'/').next().unwrap().to_vec())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(Self { paths, roots })
    }
    pub fn check(&self, root: &Path) -> Result<(), SaveError> {
        if self.roots.is_empty() {
            return Ok(());
        }
        let mut arguments: Vec<OsString> = [
            "--literal-pathspecs",
            "ls-files",
            "--full-name",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        for path in &self.roots {
            arguments.push(repository::os(path)?);
        }
        let ignored = read(root, &arguments)?;
        for path in ignored
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            // Git can still expose an embedded ignored repository as a directory
            // entry. Treat that entry as protecting its entire subtree.
            let path = path.strip_suffix(b"/").unwrap_or(path);
            if let Some(target) = self
                .paths
                .iter()
                .find(|target| within(path, target) || within(target, path))
            {
                return Err(SaveError::InvalidPlan(format!(
                    "save would replace ignored untracked work at {} while restoring {}; move that ignored work aside with native filesystem/Git tools, then assess save again",
                    crate::status::display(path),
                    crate::status::display(target)
                )));
            }
        }
        Ok(())
    }
}
fn within(path: &[u8], parent: &[u8]) -> bool {
    path == parent
        || path
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with(b"/"))
}
