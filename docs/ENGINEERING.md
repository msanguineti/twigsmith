# Engineering reference

## Implementation

- [Cargo.toml](../Cargo.toml): `git-oneflow` `0.0.0-dev`, edition 2024, MIT,
  `publish = false`. Runtime dependencies are `unicode-normalization` and
  `unicode-general-category`; `tempfile` is a development dependency. The
  lockfile and package version are unchanged. `default-run = "git-oneflow"`
  preserves the existing Cargo invocation when two binary targets are present.
- [src/main.rs](../src/main.rs) and [src/bin/gof.rs](../src/bin/gof.rs) are thin
  native entry points calling [src/cli.rs](../src/cli.rs). Shared dispatch,
  help/version, status, start parsing, presentation, and exit semantics prevent
  executable-specific behaviour. Help/version keep the canonical product name.
- [dev](../dev) and [rust-toolchain.toml](../rust-toolchain.toml) use local Rust
  1.99.0, rustfmt, and Clippy. Tools/caches/temporary files live in `.dev/`,
  outputs in `target/`, both ignored. The launcher sets process-local variables
  and runs from the project root. Normal debug/release builds include both
  binaries. [Foundation CI](../.github/workflows/foundation.yml) runs
  bootstrap/check on Ubuntu 24.04.
- [src/git.rs](../src/git.rs): public `git::run` retains literal `OsStr`
  arguments, explicit cwd, inherited environment, and raw `io::Result<Output>`.
  Inspection disables optional locks, lazy fetch, and fsmonitor, and uses
  noninteractive transport and C locale. Unsupported safeguards remain errors.
- [src/repository.rs](../src/repository.rs) shares lightweight discovery with
  preparation. Inspection observes repository kind, attached/detached/unborn
  HEAD, porcelain v2/NUL changes, submodule dirtiness, all six operation families
  (merge, rebase, cherry-pick, revert, am, bisect), and upstream/local divergence.
  Paths and process output remain bytes. Missing information never becomes zero
  divergence. Rename heuristics are disabled and submodule ignore settings are
  overridden to expose dirtiness.
- [src/status.rs](../src/status.rs) presents those facts without eligibility or
  remote-freshness judgments. It prints no paths, diffs, logs, or JSON. CLI names,
  paths, and diagnostics escape controls/backslashes; invalid UTF-8 is replaced
  for display. Output is for humans, without a machine-readable contract.
- [src/start.rs](../src/start.rs) exposes `prepare(directory, input)` independently
  of CLI/workflow presentation. It returns repository context, an exact target,
  and settings; discovery does not read HEAD/index or contact remotes.
  [START_INPUTS.md](START_INPUTS.md) defines the complete naming/settings contract.
- Inputs containing ASCII space (`U+0020`) use the existing Unicode slug rules:
  NFC → locale-independent lowercase → NFC, letters/numbers and attached marks,
  separator runs hyphenated, and no added prefix. All other inputs must pass
  literal validation and retain exact spelling, including normalization form.
  Empty/unusable results fail; no repair of invalid literals, summarization,
  truncation, product length limit, or collision suffix is applied. Full-ref
  syntax checks avoid revision expansion; leading `-`, reserved `HEAD`, and the
  exact configured base are rejected as targets.
- Repository settings in [src/settings.rs](../src/settings.rs) remain a strict
  UTF-8 `key = value` file named `oneflow.conf` in the Git common directory.
  Missing file/keys default to `remote = origin`, `base = main`, and
  `save-action = ask`; malformed, unreadable, duplicate, unknown, or invalid
  values fail. Linked worktrees share settings. The new `save-action` accepts exactly
  `ask`, `publish-only`, `staged`, `wip`, or `stash`. Start and save use one
  parser and syntax validator; start retains its error categories and public
  behavior. Remote existence/freshness for start belongs to execution; profiles,
  global sources, includes, executable settings, and legacy migration are
  excluded.
- [src/save.rs](../src/save.rs) exposes read-only `assess`/`revalidate` with
  typed eligibility errors, clean/staged-only/unstaged-only/mixed scope, explicit
  required input, applicable actions, ordered local steps and exact publication
  intent. Nonignored untracked work is separate; partially staged paths overlap
  index/remaining lists. Raw upstream values must be absent or exactly one
  matching remote and merge ref; duplicate equal values are also refused.
  Save shares repository observation but skips tracking resolution: an isolated
  Git 2.47.3 fixture crashed resolving an empty branch remote, whereas raw config
  reads allow a clear preflight rejection. No Git tracking behavior is repaired.
- Save validates the named remote locally with `remote get-url --push --all`,
  retaining all resolved destinations for revalidation. HEAD/settings/upstream,
  scope, index bytes, changed-file content/permissions/symlink targets and tracked
  diff/gitlink revisions are compared before later execution. Diff capture
  disables external drivers/textconv, relative paths and auto-refresh explicitly:
  the stale-stat regression proved ordinary diff could otherwise rewrite the
  index despite optional-lock safeguards. Git documents this auto-refresh in
  [git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-diffautoRefreshIndex).
  These are engineering safeguards for the accepted read-only contract, not
  atomic concurrency guarantees. [SAVE_PLANNING.md](SAVE_PLANNING.md) records
  the API, effect/input semantics, authoritative Git evidence and limits.
- [src/save/publication.rs](../src/save/publication.rs) publishes a resolved plan
  without pending local steps using a pinned commit OID, exact branch ref and the
  configured remote name. It revalidates local context and contains mirror,
  configured push mappings, tags and recursive submodule publication. Non-mutating
  submodule publication checks remain effective. Git's non-forced transport rules
  reject remote-ahead/divergent history without repair. All push destinations are
  handled by Git, with raw per-ref acknowledgements retained independently of
  process status and local tracking caches. Outcomes distinguish not-attempted,
  complete, partial, rejected and unconfirmed publication. Matching upstream is
  preserved; missing upstream is established only after all destinations confirm.
  Tracking observations retain partial config writes and remote-tracking updates;
  failed commands do not erase observed effects. Public save planning's upstream
  guard remains strict; only a private second-write check admits the publisher's
  expected intermediate configuration. [SAVE_PUBLICATION.md](SAVE_PUBLICATION.md)
  records the API, evidence, recovery guidance and limits.
- Publication conservatively refuses positive fetch destinations outside
  `refs/remotes/`, matching symbolic tracking refs and legacy remote-file
  definitions that configuration reads cannot inspect. Real-Git positive controls
  proved ordinary push can otherwise change local branch history through tracking
  updates. These guards are engineering containment of the accepted preservation
  contract. No user config is repaired. Normal hooks, authentication and relevant
  push configuration remain effective within explicit publication boundaries.
- [src/save/execution.rs](../src/save/execution.rs) executes approved, resolved
  plans independently of CLI presentation. Clean/publish-only actions produce no
  local checkpoints. Staged commits use exactly the index and developer message;
  WIP commits the staged selection before scoped literal-path staging/commit of
  remaining work. Stash includes nonignored untracked work and retains an
  identifiable local stash without restoration/drop. Shared publisher preflight,
  index-lock and identity guards precede local writes. Successor validation checks
  context/files, commit parent/index content, remaining diff/gitlink revisions
  and expected staging transitions before reassessment/publication. Reports
  separate verified local effects, raw command results, per-command/final
  observations, and independent publication outcomes. Failed commands/hooks can
  leave commits/stash/index effects; none are rolled back. Actual-state retries
  omit empty checkpoints and use the current index grouping. Git can retain an
  identical stash entry when contents/messages/timestamps give the same OID;
  the verified retention check admits that case without disturbing older entries.
  Assessment now retains the Git-resolved index path for alternate-index lock
  guards and observation. [SAVE_EXECUTION.md](SAVE_EXECUTION.md) records the API,
  evidence, failure/retry semantics and limits, including scoped staging's OS
  argument-size boundary. Public `save`/`v` behavior remains unimplemented.
- [src/start/workflow.rs](../src/start/workflow.rs) retains the backend, with only
  its preparation call adapted. `execute(directory, &PreparedRequest)` revalidates
  names/settings/context and returns typed actions, completed effects, and either
  an observed outcome or failure with remaining repository/target/fetched-ref/
  upstream observations. [START_WORKFLOW.md](START_WORKFLOW.md) records the
  detailed safety and remote-discovery contract.
- Exact current targets are read-only no-ops even when dirty. Other actions
  require a clean index/tree/submodules and attached, born HEAD. Bare/outside,
  active operations, protected bases, and other-worktree occupation are refused.
  Local-first resume contacts no remote and alters no history/upstream. Otherwise
  start discovers exact remote heads afresh and fetches only the selected full
  branch ref, overriding restrictive/negative refmaps. Remote counterparts get
  exact remote/merge configuration; new branches use the freshly fetched remote
  base with no upstream. Local bases, unrelated refs/tags, and fetch mappings
  remain untouched. Cached remote state is never a fallback.
- Execution rechecks safety and expected HEAD/target before each mutation and
  operates from the verified tree root, preserving reporting if a nested caller
  directory disappears. Orphan upstream configuration on an absent local target
  is refused. Snapshots are not cross-process locks; normal Git hooks/configuration
  apply. Restricted fetch mappings can leave local upstream comparison unavailable
  despite exact remote/merge configuration.
- [src/start_cli.rs](../src/start_cli.rs) accepts one UTF-8 operand after `start`
  or `s`, and only `-h`/`--help` command help. Help dispatches before operational
  access. Missing/extra arguments, option-like literal operands, removed options,
  and command prefixes are syntax errors (2); quoted phrases may begin with
  separator hyphens. Input/settings/repository/workflow failures exit 1;
  success/no-op exits 0. Failures report completed effects and observed remaining
  state truthfully, including unavailable observations and failed-step effects.
  No automatic synchronization, stashing, commits, or rollback is performed.
- [tests/support/mod.rs](../tests/support/mod.rs) clears child environments and
  controls config, identity, hooks, templates, HOME/XDG, and discovery ceiling.
  [Snapshot assertions](../tests/support/snapshot.rs) preserve file/directory
  sets, bytes, symlinks, timestamps, and permissions. Tests cover create, local/
  remote resume, dirty current no-op, nested/custom settings, grammar rejection,
  missing Git, all safety blockers, fresh discovery, refmaps/races, partial fetch/
  config/switch failures, exact Unicode branches, canonical phrases, separators,
  long inputs, unusable results, and read-only status without fetching/fsmonitor.
- [README.md](../README.md) remains limited to setup, development checks,
  invocation/settings, exit statuses, and troubleshooting.

## Design constraints

- Rebuild in Rust inside this repository, preserving Git history and the
  [MIT license](../LICENSE). Recover legacy code from Git; the Rust development
  version is not a new npm release. Distribution, installation, and release
  automation remain deferred; no global installation is required for aliases.
- Facilitate workstreams through a small set of lifecycle intents, without
  branch taxonomy or persistent branch-role metadata. OneFlow informs design
  without locking every project to Ruka's procedure. Naming prefixes have no operational meaning.
- The current configuration and fresh configured-remote-base policy remain
  accepted for start. Shared create/resume mechanics do not settle future
  starting-reference or release-completion policy.
- Git subprocess failures are completed processes, distinct from I/O errors;
  preserve stdout/stderr bytes and OS status. Application execution inherits
  normal Git environment/configuration; isolation belongs to fixtures.
- Status accepts only `status` and `status -h/--help`. Dirty/conflicted,
  detached/unborn, and bare observations exit 0; inspection/context failures
  exit 1; syntax errors exit 2. Staged/unstaged counts overlap, conflicts are
  separate, and dirty submodules receive an additional count. Upstream counts
  describe locally available history, including shallow boundaries, with no
  fetching or invented zero values. Inspection is several reads, not atomic.
- Assumption retained: Git resolves through PATH and supports inspection
  safeguards. `--no-lazy-fetch` requires Git 2.45 or newer; older Git fails
  without bypassing it. README states this prerequisite. Git lacks a distinct
  non-repository exit code, so discovery alone recognizes its C-locale outside-
  repository diagnostic; other discovery failures remain errors.
- Known compatibility limit retained: Git 2.47.3 accepts the literal branch `@`
  but `git switch @` interprets it as HEAD and refuses. Start reports that refusal
  and any effects safely. A workaround requires separately reviewed switching
  design; preparation still preserves the literal name.

## Save contract (planning/checkpoints/publication implemented; CLI pending)

Purpose: preserve or publish progress on the current workstream without
requiring finished work. Both executable names will accept `save`, with `v`
as its exact command alias; `s` remains assigned to `start`. No branch operand
or workflow category is required.

### Tree states and actions

The main states are clean, staged-only, unstaged-only, and mixed. Count
non-ignored untracked files as remaining work for this interaction, displaying
them separately. Conflicts, active operations, and dirty submodules are safety
checks, not additional user-selectable save modes.

A clean tree publishes existing commits without asking or making an empty
commit, regardless of the selected dirty-work action. Existing commit messages
are never relabelled. For dirty trees the available actions are:

| Action | Local effect before publication | Work left only locally |
| --- | --- | --- |
| Publish only | No commit, staging, or stash changes. | All uncommitted work remains untouched. |
| Staged commit | Commit exactly the index selection with a developer-supplied message, preserving partially staged file boundaries. | Unstaged edits and untracked files remain untouched. |
| WIP checkpoint | Commit the staged selection first when present, then remaining included work separately when present. | Ignored untracked files and other explicitly excluded content are not checkpointed. |
| Stash | Stash uncommitted work, including non-ignored untracked files, leaving it stashed; publish existing branch commits. | The stash is local and is not published by the branch push. |

Cancel makes no changes. Offer only applicable actions; staged-only on a dirty
tree without staged changes fails rather than inventing a commit or falling
back to another action. Staging is not proof that work is ready for review.
The WIP path uses `[WIP - Staged]` and `[WIP - Unstaged]` message labels. It
includes all tracked changes and non-ignored untracked files in its scope.
This preserves staged/remaining grouping in commit history, not the original
index state afterward. No empty checkpoint is manufactured. Stash is the
alternative for setting work aside while recording index/working-tree state;
restoration uses ordinary Git and is not performed automatically by save.

### Interaction and automation

Command forms:

- `save` / `v`: use the repository's preferred dirty-work action, defaulting to
  ask. Clean trees publish directly.
- `save --publish-only`: publish existing commits and leave unfinished work
  untouched, reporting that it remains local.
- `save --staged --message <message>`: commit/publish the staged selection.
  Without a supplied message, an interactive session can request one;
  unattended execution stops before changes rather than opening an editor.
- `save --wip`: create the applicable WIP checkpoints and publish without asking.
- `save --stash`: stash locally and publish existing commits without asking;
  report that the stash remains local and included changes were cleared
  (ignored untracked files remain untouched).

The four action switches are mutually exclusive. A configured action can be
used without its switch; a supplied message applies to the staged action.
An explicit action overrides the configured preference. Reuse the repository
configuration source; `save-action` defaults to `ask`,
without adding global sources or profiles. Custom message-prefix configuration
has not been included in this contract. A default ask action without an
interactive terminal fails before mutation with actionable guidance; it must
not hang or choose for the developer. Interactive cancellation also precedes
mutation. Showing a concise scope summary is part of informed checkpoint/stash
confirmation; agents and scripts use the same action semantics explicitly.

### Safeguards and results

Require an attached, born working branch; refuse outside/bare contexts, the
configured base, conflicts, active Git operations, and dirty submodules. No
parent-repository checkpoint can silently claim to preserve uncommitted
submodule content. Preserve normal Git hooks/configuration; no automatic hook
bypass, forced update, merge, rebase, reset, or destructive rollback. No mandatory
tests or review-readiness assessment is added to saving unfinished work.

Publication creates an absent destination branch or advances it without losing
its history. Already-published unchanged commits are a successful no-op.
Publish only the intended branch, not incidental tags or unrelated branches.
Report published work separately from edits/stashes left locally. Failure after
a local commit or stash retains that checkpoint and reports actual completed
steps and remaining state. A dropped connection may leave publication uncertain;
do not invent either success or absence of remote effects. Retrying a clean
checkpoint must not produce another empty WIP commit.

Publication destination is agreed: the exact current branch name on the
configured named remote (default `origin`). Existing upstream settings must
match that remote and exact branch; conflicts are refused before local checkpoint
or stash mutations, rather than silently followed or overwritten. Incomplete
or ambiguous upstream settings must be diagnosed, not guessed. An absent
upstream is established after confirmed successful publication; failure of that
local configuration step reports the already-published branch accurately. The
configured base remains protected, and the exact destination cannot be replaced
by implicit Git push mappings, mirror behavior, tag publication, or another
branch. Respect the selected named remote's ordinary push-URL selection,
including a separate push URL; do not use a fetch URL to infer publication.
Multiple configured push destinations require truthful aggregate/partial results,
not a success claim based on only one destination. No new remote-URL selector is
introduced. This does not change start's accepted local-resume policy.

Read-only preflight and interactive cancellation do not mutate repository state
or contact the remote. Cancel/EOF/interruption must not be reported as saved.
Syntax errors exit 2; operational failures, unresolved input, and cancellation
exit 1; confirmed completion of the selected action exits 0. Message options
apply only to the staged action; blank messages fail rather than opening an
editor. A clean tree needs no commit message because no commit is created.
Revalidate branch, settings, upstream, and relevant change scope before mutations;
if the approved context changes, stop rather than silently include different work.
Known local blockers are diagnosed before checkpoint/stash writes. No mandatory
trunk fetch or trunk integration is part of save.

The behavior above is the product contract agreed in discussion, not a claim
of universal developer practice. Git's documented foundations are selected
index commits, interrupted-work WIP/stash patterns, index-aware stash recovery,
and fast-forward publication:
[git-commit](https://git-scm.com/docs/git-commit),
[git-stash](https://git-scm.com/docs/git-stash),
[git-push](https://git-scm.com/docs/git-push).

## Historical baseline

Commit `c6c9f709bb5197a70815e15d7d6fb644dd6f35d3`, immediately before removal in
`2510bcd`, contains the legacy TypeScript/Node baseline. Its `package.json`
declares npm package `git-oneflow` version **3.1.0**, MIT, Node `>=18`, npm
`>=10`, and both `gof` and `git-oneflow` mapped to `bin/cli.js`. These are
historical manifest facts, not a verification of the live npm registry.

The legacy CLI supplied `init`, `start`, and `finish`, including feature,
release, and hotfix workflows, configuration, and dry-run support. Its behaviour
is reference material, not an accepted compatibility contract for the rewrite.
Recover details with `git show c6c9f70:package.json`,
`git show c6c9f70:README.md`, and the source/tests at that commit. The current
Rust development version does not represent a new npm release; the license
text is unchanged from the baseline.

Historical configuration experiments were inspected without checking out or
executing the legacy branches on 2026-10-08. `next-gen` at `56b0e7e` has
`src/presets.js` with `classic`, `ci`, and `prWorkflow` settings.
`task/task-command` at `adb52f7` has `src/config/presets.js` with OneFlow,
GitFlow, GitHub Flow, trunk/CI, mainline, feature-branching, and
environment-driven presets, selected through `init`. These are bundles of
configuration values, not separately implemented branching strategies.
At that tip, OneFlow inherits defaults that do not enforce prefixes;
`start` validates optional prefix policy, and `finish` uses an explicit
`--tag` rather than dispatching release behaviour by branch prefix.
Its PR path prints manual instructions. Treat this as design reference,
not verified full support for the strategies named by the presets.
Reusing the legacy preset
list or configuration API is not an accepted implementation commitment.
