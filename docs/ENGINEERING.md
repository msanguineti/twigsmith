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
- Repository settings remain a strict UTF-8 `remote = value` / `base = value`
  file named `oneflow.conf` in the Git common directory. Missing file/keys default
  to `origin`/`main`; malformed, unreadable, duplicate, unknown, or invalid values
  fail. Linked worktrees share settings. Existence/freshness and eligibility
  belong to execution; profiles, global sources, includes, executable settings,
  and legacy migration are excluded.
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
