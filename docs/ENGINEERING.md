# Engineering reference

## Implementation

- [Cargo.toml](../Cargo.toml): `git-oneflow` `0.0.0-dev`, edition 2024, MIT,
  `publish = false`; runtime dependencies are `unicode-normalization` and
  `unicode-general-category` for canonical Unicode naming. `tempfile` remains a
  development dependency; resolved dependencies are tracked in `Cargo.lock`.
- [src/main.rs](../src/main.rs) dispatches help/version, status, and start. [src/status.rs](../src/status.rs) renders the repository
  observation types; inspection and workflow policy remain separate. Status
  reports repository kind, attached/detached/unborn HEAD, path-category counts,
  submodule dirtiness, operation markers (including `am` and bisect), and configured upstream/local counts.
  Missing or unavailable upstream information never becomes zero divergence.
  Control characters and backslashes in CLI names/paths/diagnostics are escaped;
  invalid UTF-8 is replaced for display. Status exposes no paths, diffs, logs,
  JSON, or eligibility assessment. A `gof` executable alias is not yet provided.
- [dev](../dev) and [rust-toolchain.toml](../rust-toolchain.toml): local Rust
  1.99.0, rustfmt, and Clippy; tools/caches/temporary files in `.dev/`, outputs in
  `target/`, both ignored. The launcher sets process-local environment variables
  and runs commands from the project root. [Foundation CI](../.github/workflows/foundation.yml)
  on Ubuntu 24.04 runs bootstrap/check.
- [src/git.rs](../src/git.rs): public `git::run` retains literal `OsStr`
  arguments, explicit cwd, inherited environment, and raw `io::Result<Output>`.
  Inspection adds `--no-optional-locks`, `--no-lazy-fetch`, `core.fsmonitor=false`,
  noninteractive transport, and C locale. Unsupported safeguards remain errors.
- [src/repository.rs](../src/repository.rs), exported by [src/lib.rs](../src/lib.rs),
  shares lightweight `repository::discover(directory)` with request
  preparation. `repository::inspect(directory)` still returns observations
  or distinct Git/I/O/parse errors. Git-resolved paths support linked and bare
  repositories. Porcelain v2/NUL status retains raw path bytes, excludes ignored
  files, inspects submodules, disables rename heuristics, and sorts records.
- [tests/support/mod.rs](../tests/support/mod.rs) isolates real-Git fixtures in
  cleared child environments with controlled configuration, identity, hooks,
  templates, HOME/XDG, and discovery ceiling. [tests/support/snapshot.rs](../tests/support/snapshot.rs)
  shares preservation assertions between inspection and CLI tests:
  file/directory set, bytes, symlink targets, modification times, and permissions.
- [src/start.rs](../src/start.rs) exposes `prepare(directory, description, name)`
  independently of CLI presentation and Git workflow. It returns repository
  context, exact target, and validated settings, with distinct input/settings,
  I/O, and repository errors. Discovery does not read HEAD or the index.
- [Naming/settings contract](START_INPUTS.md): generated `feature/` suffixes use
  NFC → locale-independent lowercase → NFC, Unicode letters/numbers and attached
  marks, and hyphenated separators. Overrides remain literal. Full-ref syntax
  checks avoid revision expansion; leading `-`, reserved `HEAD`, and the exact
  configured base are rejected as targets. No collision avoidance is applied.
- Repository configuration is a strict UTF-8 `remote = value` /
  `base = value` file named `oneflow.conf` in the Git common directory. This
  provides one repository-wide source for nested invocation and linked worktrees,
  without includes or executable configuration. Missing keys/file default to
  `origin`/`main`; malformed, unreadable, duplicate, unknown, or invalid values
  fail. Remote names and base syntax are checked; existence/freshness and bare/detached/unborn eligibility are enforced by
  the workflow rather than preparation.
- [Workflow contract](START_WORKFLOW.md) and
  [src/start/workflow.rs](../src/start/workflow.rs): `start::execute(directory,
  &PreparedRequest)` revalidates prepared names/settings/context and returns
  typed actions, completed effects, and either an observed outcome or failure
  with remaining repository/target/fetched-ref/upstream observations.
  Git refusals retain literal arguments and raw process
  output; no rollback disguises partially completed work.
- Start blocks outside/bare/detached/unborn contexts, all six operation families,
  protected bases, other-worktree occupation, and dirtiness before switching.
  Exact-current requests are read-only no-ops even when dirty. Local resume
  neither contacts remotes nor alters branch history/configuration. Remote paths
  discover exact heads afresh and fetch only the selected full branch ref,
  overriding restrictive/negative refmaps; new branches use `--no-track` and
  remote counterparts receive their exact remote/merge pair. Local base refs,
  unrelated refs/tags, and remote fetch mappings remain untouched.
- Engineering choices: recheck safety and expected HEAD/target before each
  mutation; execute from the verified working-tree root so removal of a nested
  caller directory does not lose state reporting; reject orphan upstream
  configuration on an absent local target rather than reusing/overwriting it.
  A restricted remote fetch mapping can leave local upstream comparison
  unavailable, while counterpart remote/merge configuration remains exact.
  Snapshots are not cross-process locks; normal Git hooks/configuration apply.
- [src/start_cli.rs](../src/start_cli.rs) parses only the dispatched grammar
  and presents typed library results. Success distinguishes created, resumed
  local/remote, and already current. Failure lists the blocker/guidance, completed
  effects, and observed HEAD/tree/operations, target, attempted fetched reference,
  and upstream configuration, including unavailable observations. It does not
  claim a failed Git command had no effects or perform rollback.
- [tests/cli_start.rs](../tests/cli_start.rs) adds 10 executable-level tests with
  isolated real Git/local remotes: fresh generated/literal creation, all ordinary
  prefixes (including release/hotfix) and unprefixed targets, local/remote resume,
  dirty current no-op, nested invocation, custom settings, syntax preservation,
  missing Git, bare/unborn/detached/dirty/operation/occupation blockers,
  settings/name/protected-base/remote/base errors, fetch-only refusal, config-lock
  partial creation, and a failed hook leaving HEAD switched. Help/version and
  both command help paths succeed with no Git, outside a repository, and with
  malformed settings. Existing status no-write/no-fetch tests still pass.
  Library suites retain broader submodule/operation/race/refmap preservation
  coverage; CLI code adds no workflow policy or runtime dependencies.
- [README.md](../README.md) documents executable invocation, exact literal
  overrides, repository settings, exit statuses, and recovery from blockers and
  partial completion; library contracts now reference the implemented CLI.
- Known Git compatibility limitation: on tested Git 2.47.3, `git switch @`
  interprets the valid literal branch name as HEAD and refuses. The workflow
  preserves and surfaces that refusal; preparation's naming contract is unchanged.
  A workaround would need a separately reviewed safe switching design.

## Design constraints

- Rebuild in Rust inside this repository, preserving Git history and the existing
  [MIT license](../LICENSE). The existing project remains the continuity point;
  the legacy implementation is recoverable from Git rather than retained in the
  active source tree.
- Keep Rust development tools and caches inside the repository and use `./dev`.
  This provides a contained, reproducible development environment without
  permanent shell changes.
- Focus the application on a small set of Git workflow/lifecycle intents
  rather than general Git command wrapping. OneFlow informs the design; it is
  not the only flow the tool may accommodate. A Git execution layer is
  supporting infrastructure, not a decision to expose arbitrary Git commands.
- The tool should accommodate several flows, including OneFlow, without
  locking projects into every part of Ruka's published procedure. Neither trunk tagging nor the classic
  release-branch procedure has been selected as the product default. The
  supported variations, detailed release policies, default release behaviour,
  and any workflow-profile selection remain open. Historical presets do not commit the rewrite to their named flows.
- Preserve the intention to continue the `git-oneflow` npm identity and both
  `gof` and `git-oneflow` executable aliases, maintaining recognizable project
  and invocation names. Native distribution mechanics remain undecided.
- The Git boundary uses standard-library process results rather than a custom error or
  subprocess framework. Git failures are completed processes, distinct from
  execution errors; stdout/stderr remain bytes and status retains OS termination
  information. The application boundary intentionally inherits normal Git
  environment/configuration; isolation is a test-fixture responsibility.
- The CLI accepts `status` and `status -h/--help` with no other arguments.
  Observed states (including dirty/conflicted, detached/unborn, and bare) exit 0;
  outside Git, missing Git, unsupported working-tree contexts, and genuine
  inspection errors exit 1 with diagnostics; syntax errors retain exit 2.
  Help/version remain independent of Git. Human-readable formatting is an
  implementation choice, not a public machine-readable contract.
- The CLI accepts only `start feature <description> [--name <branch>]`, with
  `--name` after the single description argument. UTF-8 operands are required;
  option-like operands and extra/duplicate arguments are syntax errors (exit 2).
  No extra option spellings or terminator grammar were introduced. Resolution,
  eligibility, repository/settings, and Git failures exit 1; success/no-op exits 0.
  Global help/version and `start`/`start feature -h/--help` dispatch before any
  operational access. Presentation is for humans, without a machine-readable
  contract. These parser/presentation choices implement the bounded grammar;
  they do not establish deferred commands or workflow policies.
- Status counts paths by category; staged/unstaged categories can overlap,
  conflicts are separate, and submodule changes receive an additional count.
  Configured upstream output preserves remote/merge information and identifies
  comparisons as locally available references/history only. No remote freshness
  or workflow readiness conclusion is drawn; no fetch or mutation is performed.
- Repository inspection observes facts only: no branch taxonomy, base/protected-branch rules,
  persistent records, finish assessment, or operation recovery. Observation is
  several reads, not an atomic snapshot or a guarantee about later mutations.
- Inspection fixes rename detection off so display/heuristic settings cannot
  change its meaning; moves appear as additions/deletions. Submodule ignore
  settings are overridden to expose dirtiness. Git's repository-selection
  environment and normal content semantics remain applicable.
- Upstream configuration is retained even if incomplete or its local tracking
  reference is missing. An available target with unborn HEAD has no divergence;
  absent/unavailable information never implies zero. Counts describe only local
  history, including Git's shallow-history boundaries, without fetching.
- Assumption for this implementation: installed Git is resolved through PATH
  and supports the inspection options. `--no-lazy-fetch` requires Git 2.45 or
  newer; older Git returns an inspection error without bypassing the safeguard.
  README setup states this prerequisite directly. Broader Git/platform
  support policy is deferred. CLI help/version continue to work without Git.
- Git does not supply a distinct non-repository discovery exit code. Inspection
  recognizes only its C-locale outside-repository diagnostic (including mount
  boundaries); other discovery failures remain errors. This exception to machine
  output parsing is confined to discovery.

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
