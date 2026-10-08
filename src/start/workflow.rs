//! Every observation is a snapshot. Recheck before mutations, use Git's safe
//! refusals, and retain effects/state rather than rolling back partial work.

use std::{
    ffi::OsString,
    fmt, io,
    path::{Path, PathBuf},
    process::Output,
};

use super::{PreparedRequest, ResolveError, prepare};
use crate::{
    git,
    repository::{
        self, Head, InspectionError, Observation, Operations, RepositoryKind, RepositoryState,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    AlreadyCurrent,
    ResumeLocal,
    ResumeRemote,
    Create,
}

/// Only successfully completed Git steps are listed. A failed Git step can
/// itself have effects; consult its raw output and the remaining observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Fetched { source: String, destination: String },
    BranchCreated { target: String, commit: Vec<u8> },
    UpstreamRemoteSet { remote: String },
    UpstreamMergeSet { reference: String },
    Switched { target: String },
}

#[derive(Debug)]
pub struct Outcome {
    pub action: Action,
    pub effects: Vec<Effect>,
    pub state: RepositoryState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetUpstream {
    pub remotes: Vec<Vec<u8>>,
    pub merge_refs: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub struct ReferenceState {
    pub reference: String,
    pub commit: Result<Option<Vec<u8>>, StartError>,
}

#[derive(Debug)]
pub struct RemainingState {
    pub repository: Result<Observation, InspectionError>,
    /// None means the exact local branch is absent, never an inferred rollback.
    pub target_commit: Result<Option<Vec<u8>>, StartError>,
    pub fetched_reference: Option<ReferenceState>,
    /// Includes configuration left behind if a later step failed.
    pub target_upstream: Result<TargetUpstream, StartError>,
}

#[derive(Debug)]
pub struct Failure {
    pub error: StartError,
    pub action: Option<Action>,
    pub effects: Vec<Effect>,
    pub remaining: RemainingState,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}; {} completed step(s). Inspect remaining state before retrying; no rollback was attempted",
            self.error,
            self.effects.len()
        )
    }
}
impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Debug)]
pub enum StartError {
    Resolve(ResolveError),
    Inspection(InspectionError),
    OutsideRepository,
    Bare,
    Detached,
    Unborn,
    Operation(Operations),
    Occupied {
        target: String,
        location: PathBuf,
    },
    Dirty,
    Changed,
    BaseUnavailable {
        remote: String,
        base: String,
    },
    ExistingUpstreamConfiguration,
    Io(io::Error),
    Git {
        arguments: Vec<OsString>,
        output: Output,
    },
    InvalidOutput(&'static str),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolve(e) => write!(f, "{e}"),
            Self::Inspection(e) => write!(f, "{e}"),
            Self::OutsideRepository => {
                write!(f, "start requires a Git working tree; enter a checkout")
            }
            Self::Bare => write!(
                f,
                "start requires a working tree; use a checkout instead of this bare repository"
            ),
            Self::Detached => write!(
                f,
                "HEAD is detached; attach an existing branch before starting work"
            ),
            Self::Unborn => write!(
                f,
                "HEAD has no commit; establish an initial branch commit before starting work"
            ),
            Self::Operation(ops) => write!(
                f,
                "a Git operation is active ({ops:?}); complete or abort it yourself before starting work"
            ),
            Self::Occupied { target, location } => write!(
                f,
                "{target:?} is checked out at {}; use that worktree or free the branch there",
                location.display()
            ),
            Self::Dirty => write!(
                f,
                "working tree/index/submodules contain changes; preserve or resolve them before switching branches"
            ),
            Self::Changed => write!(
                f,
                "repository, settings, or target changed during start; inspect the current state and retry"
            ),
            Self::BaseUnavailable { remote, base } => write!(
                f,
                "remote {remote:?} has no exact base branch {base:?}; check repository settings and remote availability"
            ),
            Self::ExistingUpstreamConfiguration => write!(
                f,
                "the absent target already has upstream configuration; inspect that configuration before creating it"
            ),
            Self::Io(e) => write!(f, "start Git execution I/O error: {e}"),
            Self::Git { arguments, output } => write!(
                f,
                "Git refused {arguments:?} ({}): {}; inspect the remaining state and resolve the refusal before retrying",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Self::InvalidOutput(context) => write!(
                f,
                "invalid Git workflow output: {context}; inspect the repository before retrying"
            ),
        }
    }
}
impl std::error::Error for StartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Resolve(error) => Some(error),
            Self::Inspection(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
impl From<InspectionError> for StartError {
    fn from(e: InspectionError) -> Self {
        Self::Inspection(e)
    }
}
impl From<ResolveError> for StartError {
    fn from(e: ResolveError) -> Self {
        Self::Resolve(e)
    }
}

struct Workflow<'a> {
    directory: &'a Path,
    request: &'a PreparedRequest,
    action: Option<Action>,
    effects: Vec<Effect>,
    fetched: Option<String>,
}

/// Execute a prepared ordinary-work request. Revalidate its names, settings,
/// and repository identity rather than trusting a mutable/old PreparedRequest.
/// Local resume never contacts a remote. Remote paths explicitly discover heads
/// and fetch one full head ref, overriding restrictive/default fetch mappings.
/// No CLI output, stash, history rewrite, force switch, or rollback is performed.
pub fn execute(
    directory: impl AsRef<Path>,
    request: &PreparedRequest,
) -> Result<Outcome, Box<Failure>> {
    let directory = directory.as_ref();
    let mut workflow = Workflow {
        directory,
        request,
        action: None,
        effects: Vec::new(),
        fetched: None,
    };
    match workflow.execute() {
        Ok(state) => Ok(Outcome {
            action: workflow.action.unwrap(),
            effects: workflow.effects,
            state,
        }),
        Err(error) => {
            let remaining = RemainingState {
                repository: repository::inspect(workflow.directory),
                target_commit: workflow.reference(&format!("refs/heads/{}", request.target)),
                fetched_reference: workflow.fetched.as_ref().map(|r| ReferenceState {
                    reference: r.clone(),
                    commit: workflow.reference(r),
                }),
                target_upstream: workflow.target_upstream(),
            };
            Err(Box::new(Failure {
                error,
                action: workflow.action,
                effects: workflow.effects,
                remaining,
            }))
        }
    }
}

impl Workflow<'_> {
    fn read(&self, args: &[&str]) -> Result<Vec<u8>, StartError> {
        let output = git::inspect(self.directory, args).map_err(StartError::Io)?;
        checked(args, output)
    }

    fn mutate(&self, args: &[&str]) -> Result<(), StartError> {
        let output = git::workflow(self.directory, args).map_err(StartError::Io)?;
        checked(args, output).map(|_| ())
    }

    fn validated(&self) -> Result<RepositoryState, StartError> {
        let Some(context) = repository::discover(self.directory)? else {
            return Err(StartError::OutsideRepository);
        };
        if context.kind == RepositoryKind::Bare {
            return Err(StartError::Bare);
        }
        let fresh = prepare(self.directory, "", Some(&self.request.target))?;
        if &fresh != self.request {
            return Err(StartError::Changed);
        }
        let state = match repository::inspect(self.directory)? {
            Observation::OutsideRepository => return Err(StartError::OutsideRepository),
            Observation::Repository(state) => *state,
        };
        if state.operations.is_active() {
            return Err(StartError::Operation(state.operations));
        }
        match state.head {
            Head::Detached { .. } => return Err(StartError::Detached),
            Head::Unborn { .. } => return Err(StartError::Unborn),
            Head::Attached { .. } => {}
        }
        self.check_occupation(&state)?;
        Ok(state)
    }

    fn check_occupation(&self, state: &RepositoryState) -> Result<(), StartError> {
        let output = self.read(&["worktree", "list", "--porcelain", "-z"])?;
        let reference = format!("refs/heads/{}", self.request.target);
        let mut location = None;
        for field in output.split(|b| *b == 0) {
            if let Some(path) = field.strip_prefix(b"worktree ") {
                location = Some(PathBuf::from(os(path)?));
            } else if field.strip_prefix(b"branch ") == Some(reference.as_bytes()) {
                let location = location
                    .clone()
                    .ok_or(StartError::InvalidOutput("worktree path"))?;
                if state.working_tree.as_ref().map(|t| &t.root) != Some(&location) {
                    return Err(StartError::Occupied {
                        target: self.request.target.clone(),
                        location,
                    });
                }
            }
        }
        Ok(())
    }

    fn is_current(&self, state: &RepositoryState) -> bool {
        matches!(&state.head, Head::Attached { branch, .. } if branch == self.request.target.as_bytes())
    }

    fn clean(&self, state: &RepositoryState) -> Result<(), StartError> {
        match &state.working_tree {
            Some(tree) if !tree.is_dirty() => Ok(()),
            _ => Err(StartError::Dirty),
        }
    }

    fn recheck(
        &self,
        initial: &RepositoryState,
        target_commit: Option<&[u8]>,
    ) -> Result<(), StartError> {
        let now = self.validated()?;
        self.clean(&now)?;
        if now.head != initial.head
            || self
                .reference(&format!("refs/heads/{}", self.request.target))?
                .as_deref()
                != target_commit
        {
            return Err(StartError::Changed);
        }
        Ok(())
    }

    fn reference(&self, reference: &str) -> Result<Option<Vec<u8>>, StartError> {
        let args = ["show-ref", "--verify", "--hash", reference];
        let output = git::inspect(self.directory, args).map_err(StartError::Io)?;
        // --quiet avoids show-ref's diagnostic for a nonexistent exact ref.
        if !output.status.success() {
            let probe = ["show-ref", "--verify", "--quiet", reference];
            let result = git::inspect(self.directory, probe).map_err(StartError::Io)?;
            if result.status.code() == Some(1) && result.stderr.is_empty() {
                return Ok(None);
            }
            return Err(StartError::Git {
                arguments: args.iter().map(OsString::from).collect(),
                output,
            });
        }
        let oid = output
            .stdout
            .strip_suffix(b"\n")
            .ok_or(StartError::InvalidOutput("ref object ID"))?;
        if !valid_oid(oid) {
            return Err(StartError::InvalidOutput("ref object ID"));
        }
        let oid_text = std::str::from_utf8(oid).unwrap();
        if self.read(&["cat-file", "-t", oid_text])? != b"commit\n" {
            return Err(StartError::InvalidOutput("branch is not a commit"));
        }
        Ok(Some(oid.to_vec()))
    }

    fn config(&self, key: &str) -> Result<Vec<Vec<u8>>, StartError> {
        let args = ["config", "-z", "--get-all", key];
        let output = git::inspect(self.directory, args).map_err(StartError::Io)?;
        if output.status.code() == Some(1) && output.stderr.is_empty() {
            return Ok(Vec::new());
        }
        let bytes = checked(&args, output)?;
        let body = bytes
            .strip_suffix(b"\0")
            .ok_or(StartError::InvalidOutput("config records"))?;
        Ok(body.split(|b| *b == 0).map(<[u8]>::to_vec).collect())
    }

    fn target_upstream(&self) -> Result<TargetUpstream, StartError> {
        let prefix = format!("branch.{}", self.request.target);
        Ok(TargetUpstream {
            remotes: self.config(&format!("{prefix}.remote"))?,
            merge_refs: self.config(&format!("{prefix}.merge"))?,
        })
    }

    fn execute(&mut self) -> Result<RepositoryState, StartError> {
        let initial = self.validated()?;
        if self.is_current(&initial) {
            self.action = Some(Action::AlreadyCurrent);
            return Ok(initial);
        }
        self.clean(&initial)?;
        // A checkout may remove the caller's nested directory. Execute and
        // observe from the discovered root, verifying that Git's inherited
        // repository-selection environment still selects the same context.
        self.directory = self
            .request
            .repository
            .working_tree_root
            .as_deref()
            .ok_or(StartError::Bare)?;
        if repository::discover(self.directory)?.as_ref() != Some(&self.request.repository) {
            return Err(StartError::Changed);
        }
        let local = self.reference(&format!("refs/heads/{}", self.request.target))?;
        let initial_upstream = self.target_upstream()?;
        let expected_commit;
        if let Some(commit) = local {
            expected_commit = commit.clone();
            self.action = Some(Action::ResumeLocal);
            self.recheck(&initial, Some(&commit))?;
        } else {
            // Do not overwrite orphan/inherited branch configuration silently.
            if !initial_upstream.remotes.is_empty() || !initial_upstream.merge_refs.is_empty() {
                return Err(StartError::ExistingUpstreamConfiguration);
            }
            let remote = &self.request.settings.remote;
            self.read(&["remote", "get-url", remote])?; // Named remote, never a path fallback.
            let target_ref = format!("refs/heads/{}", self.request.target);
            let base_ref = format!("refs/heads/{}", self.request.settings.base);
            let advertisement = self.read(&[
                "ls-remote",
                "--heads",
                "--refs",
                "--",
                remote,
                &target_ref,
                &base_ref,
            ])?;
            let mut target_found = false;
            let mut base_found = false;
            for row in advertisement
                .split(|b| *b == b'\n')
                .filter(|r| !r.is_empty())
            {
                let separator = row
                    .iter()
                    .position(|b| *b == b'\t')
                    .ok_or(StartError::InvalidOutput("remote heads"))?;
                let (oid, rest) = row.split_at(separator);
                let reference = &rest[1..];
                if !valid_oid(oid) {
                    return Err(StartError::InvalidOutput("remote head object ID"));
                }
                target_found |= reference == target_ref.as_bytes();
                base_found |= reference == base_ref.as_bytes();
            }
            if !target_found && !base_found {
                return Err(StartError::BaseUnavailable {
                    remote: remote.clone(),
                    base: self.request.settings.base.clone(),
                });
            }
            self.action = Some(if target_found {
                Action::ResumeRemote
            } else {
                Action::Create
            });
            self.recheck(&initial, None)?;
            let source = if target_found { target_ref } else { base_ref };
            let branch = if target_found {
                &self.request.target
            } else {
                &self.request.settings.base
            };
            let destination = format!("refs/remotes/{remote}/{branch}");
            self.fetched = Some(destination.clone());
            let refspec = format!("+{source}:{destination}");
            self.mutate(&[
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
                remote,
                &refspec,
            ])?;
            self.effects.push(Effect::Fetched {
                source: source.clone(),
                destination: destination.clone(),
            });
            let commit = self.reference(&destination)?.ok_or(StartError::Changed)?;

            self.recheck(&initial, None)?;
            if self.target_upstream()? != initial_upstream {
                return Err(StartError::Changed);
            }
            expected_commit = commit.clone();
            // Explicit --no-track defeats autoSetupMerge/autoSetupRebase. Use a
            // verified commit ID, not ambiguous names, tags, or revision input.
            self.mutate(&[
                "branch",
                "--no-track",
                &self.request.target,
                std::str::from_utf8(&commit).unwrap(),
            ])?;
            self.effects.push(Effect::BranchCreated {
                target: self.request.target.clone(),
                commit: commit.clone(),
            });
            if target_found {
                // Set the exact counterpart even when remote.fetch excludes it.
                self.recheck(&initial, Some(&commit))?;
                self.mutate(&[
                    "config",
                    "--local",
                    "--replace-all",
                    &format!("branch.{}.remote", self.request.target),
                    remote,
                ])?;
                self.effects.push(Effect::UpstreamRemoteSet {
                    remote: remote.clone(),
                });
                self.recheck(&initial, Some(&commit))?;
                self.mutate(&[
                    "config",
                    "--local",
                    "--replace-all",
                    &format!("branch.{}.merge", self.request.target),
                    &source,
                ])?;
                self.effects
                    .push(Effect::UpstreamMergeSet { reference: source });
            }
            self.recheck(&initial, Some(&commit))?;
        }
        // No force, DWIM creation, ignored-file overwrite, or recursive submodule
        // checkout. Git still checks the index, files and other worktrees itself.
        self.mutate(&[
            "switch",
            "--no-guess",
            "--no-overwrite-ignore",
            "--no-recurse-submodules",
            &self.request.target,
            "--",
        ])?;
        self.effects.push(Effect::Switched {
            target: self.request.target.clone(),
        });
        let state = self.validated()?;
        let expected_upstream = if matches!(self.action, Some(Action::ResumeRemote)) {
            TargetUpstream {
                remotes: vec![self.request.settings.remote.as_bytes().to_vec()],
                merge_refs: vec![format!("refs/heads/{}", self.request.target).into_bytes()],
            }
        } else {
            initial_upstream
        };
        if !self.is_current(&state)
            || !matches!(&state.head, Head::Attached { commit, .. } if commit == &expected_commit)
            || self.target_upstream()? != expected_upstream
        {
            return Err(StartError::Changed);
        }
        Ok(state)
    }
}

fn checked(args: &[&str], output: Output) -> Result<Vec<u8>, StartError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(StartError::Git {
            arguments: args.iter().map(OsString::from).collect(),
            output,
        })
    }
}
fn valid_oid(oid: &[u8]) -> bool {
    matches!(oid.len(), 40 | 64) && oid.iter().all(u8::is_ascii_hexdigit)
}
fn os(bytes: &[u8]) -> Result<OsString, StartError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        Ok(OsString::from(
            std::str::from_utf8(bytes).map_err(|_| StartError::InvalidOutput("path encoding"))?,
        ))
    }
}
