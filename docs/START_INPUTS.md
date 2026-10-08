# Start request preparation contract

`start::prepare(directory, input)` returns a `PreparedRequest` with an exact
branch target, resolved remote/base settings, and Git-discovered repository paths.
The executable accepts `start <branch-name-or-phrase>` and the exact alias
`s <branch-name-or-phrase>` through either `git-oneflow` or `gof`;
see [invocation and troubleshooting](../README.md). The input determines only
the target branch and is not stored as a description. Preparation feeds the
separate [create/resume workflow](START_WORKFLOW.md).

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

- Only case-sensitive `remote` and `base` keys are supported, at most once each.
- Each setting is `key = value`, split at the first `=`. Surrounding whitespace
  on the line, key, and value is trimmed. LF and CRLF are supported.
- Blank lines and whole-line `#` comments are ignored. Values are literal:
  no quoting, escapes, inline comments, expansion, includes, or execution.
- An absent file, empty/comment-only file, or absent individual key uses the
  corresponding default: `remote = origin`, `base = main`.
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
