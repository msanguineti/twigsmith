# Start contract

`start::prepare(directory, input)` returns a `PreparedRequest` with an exact
branch target, resolved remote/base settings, and Git-discovered repository paths.
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

The only source is `<Git common directory>/oneflow.conf`, discovered by Git
from the caller's directory. In a usual checkout this is `.git/oneflow.conf`;
linked worktrees share the main repository's common directory file. Bare
repositories use `oneflow.conf` in the bare directory. No worktree-private,
working-tree-root, global, environment-specific, profile, or legacy settings
file is consulted. Git's normal repository-selection environment is honored.
The file is repository metadata, not tracked project content.

The format is a small, strict UTF-8 line format:

```text
# Optional repository overrides
remote = upstream
base = trunk
```

- Case-sensitive `remote`, `base`, and `save-action` keys are supported, at most
  once each. `save-action` is shared with save planning and does not alter start.
- Each setting is `key = value`, split at the first `=`. Surrounding whitespace
  on the line, key, and value is trimmed. LF and CRLF are supported.
- Blank lines and whole-line `#` comments are ignored. Values are literal:
  no quoting, escapes, inline comments, expansion, includes, or execution.
- An absent file, empty/comment-only file, or absent individual key uses the
  corresponding default: `remote = origin`, `base = main`, `save-action = ask`.
- `save-action` accepts exactly `ask`, `publish-only`, `staged`, `wip`, or `stash`;
  see the [save contract](SAVE.md#assessment-and-effects).
- Empty values, unknown/duplicate keys, malformed lines, invalid UTF-8, and
  unreadable existing files fail. A dangling symlink fails rather than defaulting.
- `base` is a branch name and uses the same syntax validation as targets.
- `remote` is a named remote, validated as the component in
  `refs/remotes/<remote>/base`; it cannot be empty, start with `-`, or be `.`.
  URLs, paths such as `../repo`, and invalid ref characters are rejected.
  Names containing valid slash-separated components are allowed.

Settings syntax validation does not require the remote to exist or the base
branch to be present locally. Those checks need fresh remote information and
belong to the workflow. Preparation accepts bare, detached, and unborn
repository contexts; eligibility for mutation belongs to that same workflow.
Invocation from a non-bare Git metadata directory without a working-tree
context fails discovery, matching the existing inspection boundary.

## Read-only boundary and errors

Preparation discovers repository paths, reads the settings file, and invokes
Git's local ref-syntax checks. It does not read the index, fetch, inspect remote
availability, create/switch/publish branches, or edit repository files. It uses
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
noninteractive transport and C-locale diagnostics. Effects are limited to fetching
the selected remote head, creating or configuring the target, and switching branches.
