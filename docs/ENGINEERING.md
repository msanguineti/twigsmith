# Engineering reference

## Implementation

- [Cargo.toml](../Cargo.toml): `git-oneflow` `0.0.0-dev`, edition 2024, MIT,
  `publish = false`, no runtime dependencies. `tempfile` remains a development
  dependency; resolved dependencies are tracked in `Cargo.lock`.
- [src/main.rs](../src/main.rs): help/version and no arguments succeed;
  unsupported arguments exit 2. The executable does not invoke Git or expose
  inspection/workflow commands. A `gof` executable alias is not yet provided.
- [dev](../dev) and [rust-toolchain.toml](../rust-toolchain.toml): local Rust
  1.99.0, rustfmt, and Clippy; tools/caches/temporary files in `.dev/`, outputs in
  `target/`, both ignored. The launcher sets process-local environment variables.
  [Foundation CI](../.github/workflows/foundation.yml) on Ubuntu 24.04 runs
  `./dev bootstrap` and `./dev check`.
- [src/git.rs](../src/git.rs): public `git::run` retains the inherited
  environment, literal `OsStr` arguments, and raw `io::Result<Output>` contract.
  Its internal inspection entry point adds `--no-optional-locks`,
  `--no-lazy-fetch`, `core.fsmonitor=false`, noninteractive transport, and the
  C locale. Unsupported safeguards fail as Git errors rather than being ignored.
- [src/repository.rs](../src/repository.rs), exported by [src/lib.rs](../src/lib.rs):
  `repository::inspect(directory)` returns `Observation` or `InspectionError`.
  Explicit types distinguish outside Git, ordinary/linked/bare repositories,
  attached/detached/unborn HEAD, tracked staged/unstaged changes, conflicts,
  untracked paths, submodule commit/content dirtiness, operation markers, and
  upstream configuration/reference availability/divergence. Bare repositories
  have no working-tree observation; detached HEAD has no applicable upstream.
  Git errors retain arguments and raw output/status; I/O and parse errors remain
  distinct. No rendering, fetch, workflow eligibility, or mutation is added.
- Discovery uses Git-resolved absolute root, per-worktree Git directory, common
  directory, and operation paths. Status uses porcelain v2 with NUL termination,
  all untracked files, no ignored entries, and explicit submodule inspection.
  Names/paths retain raw bytes, including non-UTF-8 on Unix. Records are sorted
  by path because Git does not guarantee their order.
- [tests/support/mod.rs](../tests/support/mod.rs): tests re-execute in cleared
  child environments with controlled config, HOME/XDG, identity, templates, and
  hooks. A discovery ceiling at the temporary fixture parent prevents outside
  tests from finding this checkout, since `./dev` places temporary files inside it.
- Preservation assertions compare the set of files/directories, bytes/symlink
  targets, modification times, and permissions across inspection, including Git
  metadata and submodules. Dedicated tests prove ordinary status refreshes the
  fixture index and executes its fsmonitor hook while inspection does neither;
  an existing index lock is preserved. A missing promisor-object fixture proves
  ordinary lookup attempts a lazy fetch while inspection fails without invoking
  upload-pack or changing repository state. All fetch setup/probes use isolated
  local repositories; inspection tests do not contact a network remote.

## Design constraints

- Rebuild in Rust inside this repository, preserving Git history and the existing
  [MIT license](../LICENSE). The existing project remains the continuity point;
  the legacy implementation is recoverable from Git rather than retained in the
  active source tree.
- Keep Rust development tools and caches inside the repository and use `./dev`.
  This provides a contained, reproducible development environment without
  permanent shell changes.
- Focus the application on OneFlow workflow intent rather than general Git
  command wrapping. A Git execution layer is supporting infrastructure, not a
  decision to expose arbitrary Git commands.
- Preserve the intention to continue the `git-oneflow` npm identity and both
  `gof` and `git-oneflow` executable aliases, maintaining recognizable project
  and invocation names. Native distribution mechanics remain undecided.
- The Git boundary uses standard-library process results rather than a custom error or
  subprocess framework. Git failures are completed processes, distinct from
  execution errors; stdout/stderr remain bytes and status retains OS termination
  information. The application boundary intentionally inherits normal Git
  environment/configuration; isolation is a test-fixture responsibility.
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
  This capability requirement is documented in README setup; broader Git/platform
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
