# ![git-OneFlow](g1-logo.png)

git-oneflow is a native Rust CLI project for the
[OneFlow branching model](https://www.endoflineblog.com/oneflow-a-git-branching-model-and-workflow).

## Setup

On Linux, you need a POSIX shell, standard Unix utilities, `curl` with HTTPS
support, and a C compiler/linker driver available as `cc`, with system headers
and libc development files. Bootstrap requires internet access.
On Debian/Ubuntu, install the native prerequisites with
`sudo apt-get install build-essential curl ca-certificates` (and `git` to clone
the repository).

From the repository root:

```sh
./dev bootstrap
./dev build
```

Bootstrap installs Rust **1.99.0**, rustfmt, and Clippy, pinned in
`rust-toolchain.toml`. It is safe to repeat.

Use `./dev` for development commands. It uses repository-local tools without
changing shell profiles or your permanent PATH.

Tools, dependency caches, and utilities live in `.dev/`; build outputs live in
`target/`. Both directories are ignored by Git.

## Development

Run `./dev check` before handing off changes. This is also the CI entry point:
it checks formatting, builds the executable, runs tests and CLI smoke checks,
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

Build, test, and lint use the committed `Cargo.lock`. When changing dependencies,
update and commit the lockfile alongside `Cargo.toml`. To change the toolchain,
edit `rust-toolchain.toml`, then run `./dev bootstrap` and `./dev check`.

## Run

```sh
./target/debug/git-oneflow --help
./target/debug/git-oneflow --version
```

Or build and run through Cargo:

```sh
./dev cargo run --locked -- --help
./dev cargo run --locked -- --version
```

To build and run the optimized executable:

```sh
./dev build --release
./target/release/git-oneflow --help
./target/release/git-oneflow --version
```

Running without arguments shows help. Available options are `--help` and
`--version` (also `-h` and `-V`).

## Troubleshooting

- **Local Rust missing or pinned components unavailable:** run `./dev bootstrap`
  again, especially after changing `rust-toolchain.toml`.
- **Download or certificate errors:** check internet access, proxy settings, and
  system CA certificates. Bootstrap downloads from `sh.rustup.rs` and
  `static.rust-lang.org`; retry after restoring access.
- **`cc` missing or linker errors:** install the native compiler, linker, system
  headers, and libc development files (on Debian/Ubuntu, `build-essential`).
- **Formatting or lint failures:** run `./dev fmt`, address Clippy diagnostics,
  then rerun `./dev check`.
- **Corrupt local tools or stale build outputs:** remove `.dev/` and `target/`
  from the repository root, then repeat bootstrap, build, and checks.

Created by Mirco Sanguineti. Released under the [MIT License](LICENSE).
