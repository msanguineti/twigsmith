# Start workstream workflow

The library exposes `start::execute(directory, &PreparedRequest)` after the read-only
[request preparation](START_INPUTS.md) boundary. It returns an `Outcome` or a
boxed `Failure`; the library does not print output. The executable exposes
`start <branch-name-or-phrase>` (alias `s`) through both `git-oneflow` and `gof`,
and presents these typed results. All prefixes use the same workstream workflow.

## Eligibility and selection

Execution revalidates the prepared target, settings, and repository identity.
A prepared request is not permission to mutate an old snapshot. Start requires
a working tree and an attached, existing HEAD. Outside, bare, detached, unborn,
and protected-base contexts fail with recovery guidance. Active merge, rebase,
cherry-pick, revert, `git am`, and bisect states block every action, including a
current-branch no-op. Another worktree's occupation of the exact target blocks
start and reports that worktree's path.

`Action` records the selected intent:

| Action | Behaviour |
| --- | --- |
| `AlreadyCurrent` | Exact branch already current: read-only success, including a dirty tree. |
| `ResumeLocal` | Switch to the exact existing local branch; do not contact any remote or alter history/upstream configuration. |
| `ResumeRemote` | Discover the exact remote head afresh, fetch it explicitly, create its local counterpart without automatic tracking, configure its exact remote/merge pair, and switch. |
| `Create` | Discover that the exact remote target is absent, explicitly fetch the configured remote base head, create the target at that fetched commit without tracking, and switch. |

Every action except `AlreadyCurrent` requires a clean index and working tree,
including conflicts and submodule commit, tracked, and untracked dirtiness.
Ignored files do not make status dirty, but switching explicitly refuses to
overwrite them. Git retains its normal file/index/worktree safety checks.

## Exact references and remote freshness

Local existence checks use `show-ref --verify refs/heads/<target>` and verify a
commit object. Remote discovery uses a configured named remote and `ls-remote`
for full `refs/heads/<target>` / `refs/heads/<base>` names, accepting only exact
returned refs. Tags and cached remote-tracking refs never prove existence or
absence. A missing remote/base, discovery failure, or fetch refusal stops the
remote-dependent path. In particular, deletion between discovery and fetch
fails rather than using a cached ref or changing the selected action.

Fetching uses a single explicit full-head-to-remote-tracking refspec and an
empty `--refmap`, overriding restrictive and negative fetch mappings. Only the
selected remote-tracking ref is refreshed; unrelated refs/tags and local base
history remain untouched. Automatic tag fetching, pruning, recursive submodule
fetching, FETCH_HEAD writes, commit-graph writes, and auto-maintenance are
explicitly disabled. The forced refspec permits remote-tracking updates after
remote history changes; it never forces a local branch update or switch.

A remote counterpart receives exactly `branch.<target>.remote` and
`branch.<target>.merge = refs/heads/<target>`. Existing remote fetch mappings are
preserved. If they exclude that head, Git's local upstream-comparison mapping
can remain unavailable despite the configured remote/merge pair; inspection
continues to report that distinction honestly. New work has no upstream,
regardless of automatic-tracking settings. An absent local target with existing
upstream configuration is refused before fetching rather than silently reusing
or overwriting that configuration.

## Snapshots, effects, and failures

Before each mutation, execution rechecks operation/occupation guards, attached
HEAD identity, cleanliness, settings/context, and the expected local target
ref. It does not claim to lock out other Git processes. Git refusals remain
errors. Mutations and subsequent observation run from the verified working-tree
root so switching away from a branch that contains the caller's nested directory
can remove that directory without losing the remaining-state report. Inherited
Git repository-selection semantics must still select the prepared context from
that root; otherwise execution stops.

`Outcome` contains the action, completed `Effect` records, and the observed
repository state. `Failure` contains the error, selected action when available,
completed effects, and `RemainingState`: repository observation, exact local
target commit, the fetched reference when a fetch was attempted, and target
remote/merge configuration. Each remaining observation retains its own error;
unavailable inspection is never invented as a clean or unchanged state.

Effects record successfully completed Git steps, not a transaction. Branch
creation, the two tracking-config writes, and switching are separate steps.
A failed Git command can itself have effects: a nonzero post-checkout hook can
leave HEAD already switched, and a failed fetch can have written objects. Raw
arguments, process status and diagnostics plus remaining observations are kept.
No rollback removes a branch, resets history, edits interrupted-operation state,
or hides partial completion. After a partial failure, inspect that state before
retrying; an existing target is subsequently treated as a local resume.

Git's installed command semantics remain a compatibility limit. On tested Git
2.47.3, a literal `@` is a valid ref name but `git switch @` interprets it as HEAD
and refuses the switch. Preparation still accepts it; execution exposes the
Git refusal safely rather than forcing, renaming, or using a multi-step detached
checkout workaround. An already-current literal `@` remains eligible for no-op.

Workflow Git execution keeps lazy object fetching and fsmonitor disabled,
permits required locks/writes, and retains normal hooks/configuration with
noninteractive transport and C-locale diagnostics. There is no stash, commit,
merge, rebase, operation recovery, push, tag, automatic branch synchronisation,
release/hotfix behaviour.

## Validation

`tests/start_workflow.rs` uses isolated real Git repositories, local temporary
remotes, and controlled child environments. Coverage includes the four actions,
fresh/custom bases, unchanged local histories/configuration, automatic tracking,
restricted/negative fetch mappings, deleted remote heads, unreachable remotes,
fetch-only transport failures, missing bases/tags, dirty/conflicted/submodule
states, linked/occupied worktrees, all six operation families (including both
rebase backends), ignored-file collisions, index/config locks, and failing
checkout hooks. Local transport probes deterministically change state between
inspection and execution without sleeps or network services. Preservation
snapshots include bytes, symlinks, permissions, timestamps, and file sets;
status remains read-only and reports `am` / `bisect` alongside existing markers.
