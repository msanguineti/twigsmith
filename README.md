# ![git-OneFlow](g1-logo.png)

git-oneflow is preparing for a fresh Rust implementation on `rewrite/rust`.
This branch contains a native Rust development skeleton. It currently supports
help and version output only; workflow commands are not implemented.

The previous TypeScript implementation and its documentation remain available
in Git history. Installation and command examples from that implementation do
not describe this branch. npm packaging, public executable aliases, and release
strategy remain undecided.

## Develop on Linux

Prerequisites: a POSIX shell, standard Unix utilities, `curl` with HTTPS support,
and a system C compiler/linker driver available as `cc`, with libc development
files and system headers. The current machine already provides these. Bootstrap
requires internet access to the official Rust download services. It does not
install system packages.

Run commands from the repository root, or invoke `dev` by its path from another
directory. Execute the script; do not source it or source Rustup's generated
environment file.

```sh
./dev bootstrap
./dev build
./dev test
./dev fmt
./dev fmt --check
./dev lint
./dev check
```

`bootstrap` is safe to repeat. It installs Rust **1.99.0**, with the minimal
profile plus rustfmt and Clippy, as pinned in `rust-toolchain.toml`. This stable
release is the development compiler pin, not a platform support commitment.
Update the pin and validate with `./dev bootstrap` and `./dev check` when changing
toolchains. The Rustup installation manager comes from the official installer;
its self-updates are disabled locally after bootstrap.

The wrapper sets `RUSTUP_HOME`, `CARGO_HOME`, and the toolchain explicitly,
invokes local tool executables, and overrides inherited compiler paths and
compiler cache wrappers. It sets the local tool directory on PATH only for the
command's child processes. Bootstrap uses `--no-modify-path`: no shell profile,
home-level Rust installation, or permanent PATH change is required. Use `./dev`
for all development operations; bare `cargo`, `rustc`, or an editor launching
them directly does not establish this contained environment.

| Location | Contents |
| --- | --- |
| `.dev/rustup/` | Toolchain installations and Rustup settings |
| `.dev/cargo/` | Cargo registry/Git dependency caches and installed utilities |
| `.dev/tmp/`, `.dev/cache/` | Development temporary files and utility caches |
| `target/` | Executables, test binaries, and incremental/build artifacts |

These paths are ignored by Git. `Cargo.toml`, `Cargo.lock`, and
`rust-toolchain.toml` are tracked to reproduce the application and compiler.
The skeleton has **no third-party Rust dependencies**. Build, test, and lint
commands use `--locked` so dependency changes require an intentional lockfile
update. No extra development utilities are needed; if adding a Cargo utility,
use `./dev cargo install --version <exact-version> --locked <utility>` to keep
its default installation in `.dev/cargo/bin/` and document its version.

Additional examples:

```sh
./dev rustc --version
./dev cargo --version
./dev cargo run --locked -- --help
./dev build --release
./target/release/git-oneflow --version
```

The development executable is named `git-oneflow` and reports **0.0.0-dev**.
This is a placeholder, not the eventual public release version. With no
arguments it shows help; unsupported arguments exit with status 2.

The built executable needs neither Node nor Rust tooling at runtime. It still
uses the host's normal native runtime libraries. To check the optimized binary
outside a Git repository with all tools absent from PATH:

```sh
binary="$PWD/target/release/git-oneflow"
(cd / && env -i PATH= "$binary" --help && env -i PATH= "$binary" --version)
```

`./dev test` runs an integration smoke check of help, version, and unsupported
arguments from `/` with an empty environment and PATH. There are no substantive
application tests yet because no workflow behavior has been implemented.
`./dev check` verifies formatting, builds, runs those checks, and runs Clippy
with warnings treated as errors.

Delete `.dev/` and `target/` to remove the local development environment and
build artifacts, then bootstrap again to recreate them.

The OneFlow branching model was proposed by [Adam Ruka](https://github.com/skinny85)
in [OneFlow – a Git branching model and workflow](https://www.endoflineblog.com/oneflow-a-git-branching-model-and-workflow).

Created by Mirco Sanguineti. Released under the [MIT License](LICENSE).
