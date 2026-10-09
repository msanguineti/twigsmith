# Development

## Setup

On Linux, you need a POSIX shell, standard Unix utilities, `curl` with HTTPS
support, and a C compiler/linker driver available as `cc`, with system headers
and libc development files. Bootstrap requires internet access.
On Debian/Ubuntu, install the native prerequisites with
`sudo apt-get install build-essential curl ca-certificates git`.
Git on PATH with `--no-lazy-fetch` support is required (Git 2.45 or newer).
Check your installed version with `git --version`.

From the repository root:

```sh
./dev bootstrap
./dev build
```

Bootstrap installs Rust **1.99.0**, rustfmt, and Clippy, pinned in
`rust-toolchain.toml`. It is safe to repeat.

Use `./dev` for development commands.

Tools, dependency caches, and utilities live in `.dev/`; build outputs live in
`target/`. Both directories are ignored by Git.

## Development

Run `./dev check` before handing off changes. This is also the CI entry point:
it checks formatting, builds both executables, runs tests,
and runs Clippy with warnings treated as errors.

| Command | Purpose |
| --- | --- |
| `./dev build` | Build both debug executables |
| `./dev test` | Run tests |
| `./dev test-groups` | List focused groups with their included Cargo targets |
| `./dev test-group NAME [args]` | Run a focused group with additional Cargo test arguments |
| `./dev fmt` | Format Rust source |
| `./dev fmt --check` | Check formatting |
| `./dev lint` | Run Clippy with warnings treated as errors |
| `./dev check` | Check formatting, build, test, and lint |
| `./dev cargo <args>` | Run Cargo in the local environment |
| `./dev rustc --version` | Show the compiler version |

Build, test, and lint use the committed `Cargo.lock`.

## Select tests during development

Run focused tests while implementing a change. List the available groups and
their exact targets with `./dev test-groups`, then select a capability:

```sh
./dev test-group start
./dev test-group config
./dev test-group shared
```

The groups are `start`, `save`, `finish`, `config`, `status`, `shared`, and
`lifecycle`. Command groups include backend and CLI coverage. `shared` covers
the development launcher, Git boundary, repository observation, settings,
tracking, remote roles, finish-check configuration, and exact save/finish
publication. `lifecycle`
includes CLI smoke checks and end-to-end project/fork workflows. Every group
also runs library and binary unit tests (`--lib --bins`). Targets overlap where
they exercise shared behaviour.

Choose additional targets or groups when a change affects shared components,
dependencies, configuration, or behaviour across commands. For example, settings
changes can affect planning and execution in all three lifecycle commands;
`config` alone covers settings and setup plus finish-check integration, rather
than every settings consumer. Changes to `tests/support/` can affect any target
using those fixtures. Group membership is an iteration aid, not an exhaustive
dependency map.

Use Cargo selection directly for a narrower scope. `./dev test` forwards all
arguments unchanged, including positional test-name filters, target selectors,
and test-harness arguments after `--`:

```sh
./dev test --test cli_finish
./dev test --test finish_preparation --test finish_publication
./dev test --test settings
./dev test config
./dev test --test settings remote -- --nocapture
./dev test-group finish recovery -- --nocapture
```

`./dev test config` remains a Cargo test-name filter. Groups use the separate
`test-group` command, accept one group name, and prepend Cargo target selectors
before forwarding the remaining arguments. Additional selectors can expand the
selection. An unknown or missing group fails with usage guidance.

Run one complete `./dev check` against the final implementation before handoff.
Repeat complete validation when subsequent changes, failures, or unresolved
concerns warrant it. Focused tests support iteration and preserve comprehensive
final validation: `./dev test` without arguments and `./dev check` run every
Cargo-discovered test target, including CLI smoke and end-to-end tests. New tests
participate in full runs regardless of group membership; update the group
definitions in `dev` when a target is useful for focused iteration.

## Run a build

```sh
./target/debug/git-oneflow --help
./target/debug/gof status
./dev cargo run --locked -- start "add user avatars"
./dev cargo run --locked --bin gof -- --help
```

Cargo defaults to `git-oneflow`. The development launcher runs from this
project's root. To use another checkout, run a built executable from that checkout.

For optimized executables:

```sh
./dev build --release
./target/release/git-oneflow --help
./target/release/gof --version
```

## Troubleshooting

- **Missing Rust tools or components:** repeat `./dev bootstrap`, especially
  after a toolchain change.
- **Download/certificate failure:** check network access, proxy settings, and
  system CA certificates. Bootstrap uses `sh.rustup.rs` and `static.rust-lang.org`.
- **Linker failure:** install the native compiler, linker, headers, and libc
  development files; on Debian/Ubuntu these are supplied by `build-essential`.
- **Formatting/lint failure:** run `./dev fmt`, address Clippy diagnostics, and
  repeat `./dev check`.
- **Corrupt tools/build outputs:** remove `.dev/` and `target/` from the project
  root, then bootstrap, build, and check again.

## Contributing

Use `./dev check` before handing off changes. The [engineering reference](ENGINEERING.md)
summarizes the architecture and current implementation boundaries. Detailed
workflow invariants are documented in the [start](START.md), [save](SAVE.md),
and [finish](FINISH.md) contracts.
