# git-oneflow

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
it checks formatting, builds the executable, runs tests,
and runs Clippy with warnings treated as errors.

| Command | Purpose |
| --- | --- |
| `./dev build` | Build the debug executable |
| `./dev test` | Run tests |
| `./dev fmt` | Format Rust source |
| `./dev fmt --check` | Check formatting |
| `./dev lint` | Run Clippy with warnings treated as errors |
| `./dev check` | Check formatting, build, test, and lint |
| `./dev cargo <args>` | Run Cargo in the local environment |
| `./dev rustc --version` | Show the compiler version |

Build, test, and lint use the committed `Cargo.lock`.

## Run

```sh
./target/debug/git-oneflow --help
./target/debug/git-oneflow --version
./target/debug/git-oneflow status
```

Or build and run through Cargo:

```sh
./dev cargo run --locked -- --help
./dev cargo run --locked -- --version
./dev cargo run --locked -- status
```

To build and run the optimized executable:

```sh
./dev build --release
./target/release/git-oneflow --help
./target/release/git-oneflow --version
```

Running without arguments shows help. Available options are `--help` and
`--version` (also `-h` and `-V`). Use `status --help` for command help.

Run `git-oneflow status` from a repository root or nested directory to summarize
branch/HEAD state, staged and unstaged path counts, conflicts, untracked paths,
submodule changes, detected operations, and upstream information. Counts can
overlap: a path can have both staged and unstaged changes. Ignored paths are
excluded. Bare repositories are identified as having no working tree.

Status is read-only and does not fetch or contact remotes. Upstream comparisons
use locally available references and history (including shallow-history limits);
absent configuration or unavailable comparison data is distinct from zero
ahead/behind counts. Observations do not establish current remote state or
workflow readiness. Output is intended for people, with no machine-readable
format contract.

Dirty trees, detached HEAD, unborn branches, and detected operations are successful
observations (exit 0). Missing Git, locations outside Git, unsupported working-tree
contexts such as a non-bare `.git` directory, and inspection failures exit 1 with
a diagnostic. Invalid syntax exits 2. Help and version require no Git installation.
Use the built executable to inspect another checkout; `./dev cargo run` always
runs from this project's root.

## Troubleshooting

- **Local Rust missing or pinned components unavailable:** run `./dev bootstrap`
  again, especially after changing `rust-toolchain.toml`.
- **Download or certificate errors:** check internet access, proxy settings, and
  system CA certificates. Bootstrap downloads from `sh.rustup.rs` and
  `static.rust-lang.org`; retry after restoring access.
- **`cc` missing or linker errors:** install the native compiler, linker, system
  headers, and libc development files (on Debian/Ubuntu, `build-essential`).
- **Status fails:** ensure Git 2.45 or newer is on PATH and invoke the executable
  from a repository working tree or a bare repository. Git diagnostics identify
  inspection problems such as corrupt metadata or ownership restrictions.
- **Formatting or lint failures:** run `./dev fmt`, address Clippy diagnostics,
  then rerun `./dev check`.
- **Corrupt local tools or stale build outputs:** remove `.dev/` and `target/`
  from the repository root, then repeat bootstrap, build, and checks.
