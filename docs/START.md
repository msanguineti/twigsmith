# Start contract

`start::prepare(directory, input)` returns a `PreparedRequest` with an exact
branch target, resolved remote/base settings, Git-discovered repository paths,
and raw target tracking and explicit Git push-selection settings.
The executable accepts `start <branch-name-or-phrase>` and the exact alias
`s <branch-name-or-phrase>` through either `git-oneflow` or `gof`;
see the [user guide](USAGE.md#start-or-resume-work). The input determines only
the target branch and is not stored as a description. Preparation feeds the
[create/resume workflow](#eligibility-and-selection) below.

## Literal names and phrases

An input containing an ordinary ASCII space (`U+0020`) is a phrase. Every other
input is a literal branch name, preserved exactly: case, underscores, prefixes,
Unicode text, and normalization form. Tabs or other whitespace alone do not
trigger phrase conversion. Unicode spaces valid in Git names remain literal.
For example, `feature`, `Fix/DeploymentBug`, and `fix/parser_v2` remain unchanged.
Prefixes including `release/` and `hotfix/` do not select workflow behaviour.

Phrase conversion uses the canonical Unicode slug algorithm:

1. Normalize to NFC, apply Rust's Unicode locale-independent string lowercase
   conversion, then normalize to NFC again.
2. Retain Unicode general-category letters (`L`) and numbers (`N`). Retain
   combining marks (`M`) only after a retained letter, number, or mark in the
   same word; discard unattached marks.
3. Treat all other characters as separators. Collapse separator runs to one
   ASCII hyphen between words; omit leading/trailing separators.
4. Reject an empty slug, then validate the complete branch name. Add no prefix.

For example, `nasty deployment bug` becomes `nasty-deployment-bug`, and
`Fix__BUG / now` becomes `fix-bug-now`. `CAFÉ work` and `CAFE` followed by a
combining acute accent and ` work` both become `café-work`. Literal `CAFÉ` and
`CAFE` followed by a combining acute accent retain their distinct exact spelling.
Hindi vowel signs and Arabic combining marks remain part of phrase words.
This is canonical normalization, not transliteration, compatibility folding,
case folding, or locale-specific casing. Dependency tables and the pinned Rust
toolchain define the Unicode version; changes should be reviewed for naming
effects. No summarization, truncation, product length limit, collision lookup,
numbered suffix, or arbitrary repair of invalid literal names is applied.

## Safe target validation

Git validates `refs/heads/<target>` with `check-ref-format`, rather than
`--branch` (which expands checkout shorthand). Leading `-` and the reserved
branch name `HEAD` are also rejected. NUL is rejected before process execution.
Revision expressions such as `@{-1}`, `main~1`, and `main^{commit}` are never
resolved. A literal `@` is a valid branch name. The target must differ **exactly**
from the configured base, for both phrase slugs and literal names. No case or
Unicode-equivalence comparison is imposed on literal Git names.

The CLI requires one UTF-8 operand. Missing/extra arguments, option-like literal
operands, removed naming options, and command prefixes are syntax errors (exit 2).
Quoted phrases may begin with separators, including hyphens. Command help is
parsed before any repository/Git/settings access. Invalid target names and
unusable phrases are operational input failures (exit 1).

## Repository settings

`settings::load` supplies the same sparse model to start and save. Git discovers
the current working-tree root and common directory from the caller's directory;
its normal repository-selection environment is honored. Shared configuration
lives at that root. Local overrides live in the common directory and apply across
linked worktrees. Bare repositories have no shared layer. Parent/home inheritance,
custom paths, environment-specific settings, and external imports are absent.
See the [user guide](USAGE.md#repository-settings) for format examples and migration.

Each layer recognizes `git-oneflow.config.toml`, `git-oneflow.config.yaml`,
`git-oneflow.config.yml`, and `git-oneflow.config.json`, independently. At most one
recognized file may exist per layer; extensions have no priority. Local legacy
`oneflow.conf` is a fallback only when no new local file exists. Coexistence is an
error with explicit migration guidance. Legacy values remain literal unquoted
`key = value` lines, split at the first `=`, with whitespace trimmed, LF/CRLF,
blank lines, and whole-line `#` comments accepted.

- New formats require a flat mapping of string values under case-sensitive
  supported settings keys, each at most once; see the
  [settings reference](USAGE.md#repository-settings). YAML uses scalar
  types rather than coercing numbers/booleans/null to strings; merge keys and
  unsupported tags are rejected. Parser import/expression features are disabled.
- Keys resolve individually: explicit existing flags > local > shared > built-in
  defaults (`origin`, `main`, `ask`). Start has no settings flags. `save-action`
  accepts `ask`, `publish-only`, `staged`, `wip`, or `stash` and does not alter start.
- Every present layer is parsed and validated, including overridden values.
  Unknown/duplicate keys, invalid types, malformed syntax, empty/invalid values,
  invalid UTF-8, and unreadable files fail. Only absent files default; dangling
  symlinks fail. Empty TOML or empty YAML/JSON mappings inherit all keys.
- `base` uses the same branch-name validation as targets. Each remote role is
  validated as the component in `refs/remotes/<remote>/base`; it cannot be empty, start
  with `-`, or be `.`. URLs, paths such as `../repo`, and invalid ref characters
  fail. Valid slash-separated remote names remain supported.
- Resolved settings retain a source per key: built-in default, shared file path,
  or local file path (including legacy). Prepared requests carry this information.
  Revalidation compares resolved values and sources; comments/formatting changes
  with identical values and sources do not change the settings snapshot.

`publish-remote` selects publication and remote resume; `base-remote` selects
the project base. An omitted base remote follows the final publication selection.
`remote` remains a publication input alias, with the precedence described in the
[user guide](USAGE.md#repository-settings). Remote names have no special meaning
and both roles may use the same remote.

Settings syntax validation does not require either remote to exist or the base
branch to be present locally. Those checks need fresh remote information and
belong to the workflow. Preparation accepts bare, detached, and unborn
repository contexts; eligibility for mutation belongs to that same workflow.
Invocation from a non-bare Git metadata directory without a working-tree
context fails discovery, matching the existing inspection boundary.

## Read-only boundary and errors

Preparation discovers repository paths, reads configuration layers, and invokes
Git's local ref-syntax checks and raw target tracking/push-selection settings.
It does not read the index, fetch, inspect remote availability, create/switch/publish branches, or edit repository files. It uses
the existing no-optional-locks, no-lazy-fetch, fsmonitor-disabled Git executor.
Existing branch names and collisions do not change resolution.

`ResolveError` distinguishes outside-repository context, Git discovery/execution
errors, settings I/O, malformed/invalid settings, unusable phrases, invalid
target names, and protected-base targets. Git process errors retain raw output
through `InspectionError`; presentation and exit codes belong to the CLI (operational errors exit 1).
Repository settings resolve and validate before target resolution/validation.
Preparation is several reads, not an atomic snapshot or authorization for a
later mutation; the workflow must recheck its own safety conditions.

## Eligibility and selection

`start::execute(directory, &PreparedRequest)` returns an `Outcome` or boxed
`Failure`. The library records typed actions/effects; the CLI presents them.

Execution revalidates the prepared target, settings, and repository identity.
A prepared request is not permission to mutate an old snapshot. Start requires
a working tree and an attached, existing HEAD. Outside, bare, detached, and unborn
contexts fail with recovery guidance. Targets equal to the configured base fail. Active merge, rebase,
cherry-pick, revert, `git am`, and bisect states block every action, including a
current-branch no-op. Another worktree's occupation of the exact target blocks
start and reports that worktree's path.

`Action` records the selected intent:

| Action | Behaviour |
| --- | --- |
| `AlreadyCurrent` | Exact branch already current: read-only success, including a dirty tree. |
| `ResumeLocal` | Switch to the exact existing local branch; do not contact any remote or alter history/upstream configuration. |
| `ResumeRemote` | Discover the exact publication-remote head afresh, fetch it explicitly, create its local counterpart without automatic tracking, configure its exact remote/merge pair, and switch. |
| `Create` | Discover that the exact publication-remote target is absent, explicitly fetch the configured base head from the base remote, create the target at that fetched commit without tracking, and switch. |

Every action except `AlreadyCurrent` requires a clean index and working tree,
including conflicts and submodule commit, tracked, and untracked dirtiness.
Ignored files do not make status dirty, but switching explicitly refuses to
overwrite them. Git retains its normal file/index/worktree safety checks.

## Exact references and remote freshness

Local existence checks use `show-ref --verify refs/heads/<target>` and verify a
commit object. Remote discovery uses configured named remotes and `ls-remote`
for full head names, accepting only exact returned refs. The publication remote
is queried for `refs/heads/<target>`. When absent there, the base remote supplies
`refs/heads/<base>`; a same-named working branch on the base remote is not resumed.
When roles match, both exact heads can be discovered in one query. Tags and
cached remote-tracking refs never prove existence or
absence. A missing remote/base, discovery failure, or fetch refusal stops the
remote-dependent path. In particular, deletion between discovery and fetch
fails rather than using a cached ref or changing the selected action.

Fetching uses a single explicit full-head-to-remote-tracking refspec and an
empty `--refmap`, overriding restrictive and negative fetch mappings. Only the
selected remote-tracking ref is refreshed; unrelated refs/tags and local base
history remain untouched. The fork's base branch is not synchronized. Symbolic
fetch destinations are refused because they can redirect writes into existing
local history. Automatic tag fetching, pruning, recursive submodule
fetching, FETCH_HEAD writes, commit-graph writes, and auto-maintenance are
explicitly disabled. The forced refspec permits remote-tracking updates after
remote history changes; it never forces a local branch update or switch.

A remote counterpart receives exactly `branch.<target>.remote = <publish-remote>` and
`branch.<target>.merge = refs/heads/<target>`. Existing remote fetch mappings are
preserved. If they exclude that head, Git's local upstream-comparison mapping
can remain unavailable despite the configured remote/merge pair; inspection
continues to report that distinction honestly. New work has no upstream,
regardless of automatic-tracking settings. An absent local target with existing
upstream configuration is refused before fetching rather than silently reusing
or overwriting that configuration.

## Tracking and push selection

Local resume and current-branch success accept absent tracking, the exact target
on the publication remote, or the configured base branch on the base remote.
Each configured relationship requires exactly one remote and one full merge ref.
Incomplete, duplicate, empty, local `.` and unrelated relationships fail. Valid
existing tracking and other branch settings remain unchanged; local actions need
no configured or reachable remote.

The shared [save tracking and push-selection guard](SAVE.md#preconditions-and-local-evidence)
also validates the target's effective explicit push override. The last
`branch.<target>.pushRemote` value takes precedence over the last
`remote.pushDefault` value; when present it must equal the publication remote
name. Implicit routing defaults do not select start's source. Start neither
edits these settings nor changes ordinary Git routing.

## Snapshots, effects, and failures

Before each mutation, execution rechecks operation/occupation guards, attached
HEAD identity, cleanliness, settings/context, the expected local target ref,
and raw target tracking/push-selection settings. Each required remote's resolved
fetch URLs are captured before discovery and rechecked before subsequent
operations. Local actions omit remote URL resolution. Only start's own confirmed
tracking writes advance the expected snapshot; concurrent tracking changes stop
the next step. These snapshots are not locks against other Git processes.
Git refusals remain errors. Mutations and subsequent observation run from the verified working-tree
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
noninteractive transport and C-locale diagnostics. Effects are limited to fetching
the selected remote head, creating or configuring the target, and switching branches.
