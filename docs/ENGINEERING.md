# Engineering reference

## Implementation

- [Cargo.toml](../Cargo.toml): `git-oneflow` `0.0.0-dev`, edition 2024, MIT,
  `publish = false`, no runtime dependencies; `tempfile` is a development
  dependency. Resolved dependencies are tracked in `Cargo.lock`.
- [src/main.rs](../src/main.rs) dispatches help/version and status, and owns
  runtime diagnostics. [src/status.rs](../src/status.rs) renders the repository
  observation types; inspection and workflow policy remain separate. Status
  reports repository kind, attached/detached/unborn HEAD, path-category counts,
  submodule dirtiness, operation markers, and configured upstream/local counts.
  Missing or unavailable upstream information never becomes zero divergence.
  Control characters in displayed Git names are escaped; invalid UTF-8 is
  replaced for display. No paths, diffs, logs, JSON, or eligibility assessment
  are exposed. A `gof` executable alias is not yet provided.
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
  provides `repository::inspect(directory)` returns observations
  or distinct Git/I/O/parse errors. Git-resolved paths support linked and bare
  repositories. Porcelain v2/NUL status retains raw path bytes, excludes ignored
  files, inspects submodules, disables rename heuristics, and sorts records.
- [tests/support/mod.rs](../tests/support/mod.rs) isolates real-Git fixtures in
  cleared child environments with controlled configuration, identity, hooks,
  templates, HOME/XDG, and discovery ceiling. [tests/support/snapshot.rs](../tests/support/snapshot.rs)
  shares preservation assertions between inspection and CLI tests:
  file/directory set, bytes, symlink targets, modification times, and permissions.

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
