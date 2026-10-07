# Engineering reference

## Implementation

- [Cargo.toml](../Cargo.toml): package `git-oneflow`, version `0.0.0-dev`,
  edition 2024, MIT, `publish = false`, and no runtime dependencies. `tempfile`
  is a development-only dependency for safely created, automatically cleaned
  test directories; its resolved dependencies are tracked in `Cargo.lock`.
- [src/main.rs](../src/main.rs): no arguments, `--help`/`-h`, and
  `--version`/`-V` succeed; unsupported arguments exit 2. No Git execution,
  repository inspection, configuration, or workflow commands exist. The current
  build produces `git-oneflow`; a `gof` executable alias is not yet provided.
- [dev](../dev) and [rust-toolchain.toml](../rust-toolchain.toml): Rust 1.99.0,
  rustfmt, and Clippy; tools, dependency caches, and temporary files under `.dev/`,
  build outputs under `target/`, both ignored. The launcher sets process-local
  environment variables and does not modify shell profiles. See the
  [README](../README.md) for prerequisites and command usage.
- [Foundation CI](../.github/workflows/foundation.yml): pushes and pull requests
  on Ubuntu 24.04 run `./dev bootstrap` then `./dev check`.
- [src/git.rs](../src/git.rs), exposed by [src/lib.rs](../src/lib.rs):
  `git::run(directory, arguments) -> io::Result<Output>` invokes installed `git`
  directly through `Command`, with explicit cwd and separate `OsStr` arguments.
  `Ok` preserves raw stdout/stderr and `ExitStatus` even on unsuccessful exit;
  `Err` preserves the I/O error when execution cannot start or output collection
  fails. Callers decide presentation and application exit; this helper does
  neither. CLI help/version do not invoke Git.
- [tests/support/mod.rs](../tests/support/mod.rs): fixtures re-execute each test
  in a child with a cleared environment, controlled Git config files, temporary
  HOME/XDG directories, and no inherited repository-selection variables. Real
  repositories use explicit branch/template, local identity, and empty hooks.
  Tests do not change global environment or the project's Git state.
- An additional `./dev test` run passed with inherited hostile system/global
  configuration (commit signing, hooks, template), empty author/committer
  identity, injected config, and repository/worktree/index/object overrides.
  The hostile hook marker and unrelated index were not created.

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
- Runtime assumption: installed Git is resolved through PATH. Development
  tests require Git; CLI help/version continue to work without it. No Git version
  compatibility policy or new public workflow behaviour is established.

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
