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
| `./dev fmt` | Format Rust source |
| `./dev fmt --check` | Check formatting |
| `./dev lint` | Run Clippy with warnings treated as errors |
| `./dev check` | Check formatting, build, test, and lint |
| `./dev cargo <args>` | Run Cargo in the local environment |
| `./dev rustc --version` | Show the compiler version |

Build, test, and lint use the committed `Cargo.lock`.

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
