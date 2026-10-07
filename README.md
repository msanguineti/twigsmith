# ![git-OneFlow](g1-logo.png)

git-oneflow is a native Rust CLI project for the
[OneFlow branching model](https://www.endoflineblog.com/oneflow-a-git-branching-model-and-workflow).

## Setup

On Linux, you need a POSIX shell, standard Unix utilities, `curl` with HTTPS
support, and a C compiler/linker driver available as `cc`, with system headers
and libc development files. Bootstrap requires internet access.

From the repository root:

```sh
./dev bootstrap
./dev build
```

Bootstrap installs Rust **1.99.0**, rustfmt, and Clippy, pinned in
`rust-toolchain.toml`. It is safe to repeat.

Use `./dev` for development commands so tools run with repository-local
`RUSTUP_HOME` and `CARGO_HOME`. Execute the script directly. It leaves shell
profiles and your permanent PATH unchanged.

Tools, dependency caches, and utilities live in `.dev/`; build outputs live in
`target/`. Both directories are ignored by Git. Delete them and bootstrap again
to reset the environment.

## Development

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

Tests currently cover help, version output, and argument errors.

Build, test, and lint use the committed `Cargo.lock`. When changing dependencies,
update and commit the lockfile alongside `Cargo.toml`. To change the toolchain,
edit `rust-toolchain.toml`, then run `./dev bootstrap` and `./dev check`.

## Run

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

The executable reports development version **0.0.0-dev**. It runs independently
of Node and Rust development tools, using the host's native runtime libraries.
Running it without arguments shows help.

Created by Mirco Sanguineti. Released under the [MIT License](LICENSE).
