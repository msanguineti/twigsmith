# Engineering reference

## Implementation

- [Cargo.toml](../Cargo.toml): package `git-oneflow`, version `0.0.0-dev`,
  edition 2024, MIT, `publish = false`, and no dependencies. `Cargo.lock` is tracked.
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
