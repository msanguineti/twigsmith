# Read-only save planning

Task #11 provides library planning. Task #12 adds the independent
[publication backend](SAVE_PUBLICATION.md); task #13 composes it with
[local checkpoint execution](SAVE_EXECUTION.md). The `save`/`v` commands remain
for task #14. Public CLI behavior is unchanged.

## Shared settings

`settings::load` is the single parser/validator used by start and save. The
existing strict UTF-8 `<Git common directory>/oneflow.conf` format and remote/base
defaults are retained; linked worktrees share that file. Add the optional key:

```text
save-action = ask
```

Its case-sensitive values are exactly `ask`, `publish-only`, `staged`, `wip`, and
`stash`. Absence defaults to `ask`. Duplicate keys, empty/unknown values and
unreadable files fail, including when an explicit action is supplied. Start
validates this shared preference but otherwise ignores it. There are no global
settings, profiles or configurable WIP labels.

## Assessment and effects

`save::assess(directory, &Request)` returns a `Plan` or a typed `SaveError`,
without writes or remote contact. `Request.action` overrides `save-action`;
`Request.message` applies only to a staged selection. On a clean tree even a
staged selection needs no message. A supplied message is retained verbatim;
a dirty staged selection rejects Unicode-whitespace-only messages.

`Scope` lists staged, remaining tracked and nonignored untracked paths as bytes
relative to the repository root. Staged and remaining tracked lists can overlap:
partially staged files retain both sides of the selection. Ignored untracked
files are excluded. `tree_state()` distinguishes clean, staged-only,
unstaged-only (including untracked-only), and mixed trees.

A plan explicitly contains its attached branch/commit, repository context,
settings, scope, applicable actions, required input, ordered local steps and
publication destination. The action semantics are:

| Selected action | Planned local steps | Publication / locally retained work |
| --- | --- | --- |
| Clean tree, any preference/action | None; effective action is publish-only. | Publish existing commits; no empty checkpoint or message request. |
| Ask on dirty tree | None; action input required. | No effects permitted until a fresh selected plan is obtained and approved. |
| Publish-only | None. | Publish existing commits; all uncommitted work remains local and unchanged. |
| Staged | Commit exactly the current index with the developer message. | Publish the resulting branch; remaining tracked and untracked work stays untouched. Missing message requires input; no steps/publication are permitted yet. No staged changes is an error. |
| WIP | Commit the index using `[WIP - Staged]` when present, then stage/commit remaining work using `[WIP - Unstaged]` when present. | Publish the resulting branch; ignored untracked work is excluded. Original index state is not retained afterward. |
| Stash | Preserve tracked and nonignored untracked work in a local stash. | Publish existing branch commits. The stash is local preservation, never branch publication, and is left stashed. |

Local steps always precede publication. `publication = None` and empty steps
mean unresolved input permits no effects. Publication names only the configured
remote and exact `refs/heads/<current branch>`; absent upstream is marked for
establishment **after confirmed successful publication**. The plan does not
claim server availability, fast-forward eligibility, or successful publication.
The publication backend executes resolved plans without pending local steps and
reports its outcomes. The local executor implements preservation/composition;
interaction remains for task #14. WIP grouping represents index/remaining scope,
not readiness.

## Preconditions and local evidence

Assessment requires a working repository and attached branch with a commit,
and refuses the exact configured base, unresolved conflicts, dirty submodule
contents and all observed operation families: merge, rebase, cherry-pick,
revert, am and bisect (including sequencer state). Clean submodule commit-pointer
changes can be included; parent commits cannot preserve dirty child contents.

All effective `branch.<name>.remote` and `.merge` values are read with NUL-delimited
`git config --get-all`. Both absent is acceptable. Otherwise **one remote value
and one exact full merge ref** must match the configured remote/current branch.
Even repeated equal values are rejected as ambiguous. Empty values, missing
halves, local `.` upstreams and other branches/remotes fail. Tracking ref
existence, fetch refmaps and locally cached divergence are not prerequisites.
Save bypasses status's tracking resolution so malformed upstream configuration
can be diagnosed before Git tries to resolve it.

`git remote get-url --push --all <name>` validates the named remote locally and
captures all resolved push destinations, including ordinary URL rewriting.
The remote may be inaccessible; no fetch, server query, trunk update or trunk
integration is performed. Later publication must handle multiple destinations
and partial/uncertain outcomes truthfully.

Git documents upstream configuration and multi-valued merge behavior in
[git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-branchltnamegtmerge),
and push-URL expansion/selection in
[git-remote](https://git-scm.com/docs/git-remote#Documentation/git-remote.txt-get-url).
Rejecting duplicate upstream values is our accepted conservative product policy,
not a claim that Git prohibits them or that this workflow is universal.

## Revalidation and limits

`save::revalidate(directory, &approved_plan)` repeats assessment and compares
context, HEAD, settings, raw upstream values, resolved push URLs, scope, index
path/bytes, changed-file bytes/permissions or symlink targets, and local tracked diff
content (including changed gitlink revisions). It catches changed content even
when the same paths/categories remain dirty. Untracked directory observations
are inspected recursively without following symlinks. Snapshot content remains
in memory; it is not a persisted plan or lock. Index refreshes by another process
can conservatively invalidate approval even if the selected content is unchanged.
A new failed safety guard is returned directly; other differences report changed
context. Callers must obtain and approve a new plan after changed context and
revalidate before execution; later executors need checks between their own steps.

The Git inspection executor disables optional locks, lazy fetching and fsmonitor.
Diff capture additionally disables external diff/textconv, index auto-refresh,
relative path output and submodule ignore settings. A real-Git regression proved
that ordinary porcelain diff can refresh a stale index even with optional locks
disabled; this is why `diff.autoRefreshIndex=false` is explicit. The behavior is
also documented in
[git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-diffautoRefreshIndex).

These observations are several local reads. They neither guarantee a consistent
snapshot during concurrent edits nor atomically protect a later Git mutation.
They follow existing Git observation semantics and do not assert preservation of
content Git excludes from the scope. Normal Git configuration remains in effect
apart from the documented inspection safeguards. Content snapshots can consume
memory proportional to included work; streaming fingerprints can be considered
later if concrete large-worktree requirements justify them.
