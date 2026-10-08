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

## Run

```sh
./target/debug/git-oneflow --help
./target/debug/git-oneflow --version
./target/debug/git-oneflow status
./target/debug/git-oneflow start "add user avatars"
./target/debug/gof start Fix/DeploymentBug
./target/debug/gof s fix/parser_v2
```

Or build and run through Cargo:

```sh
./dev cargo run --locked -- --help
./dev cargo run --locked -- --version
./dev cargo run --locked -- status
```

To build and run the optimized executables:

```sh
./dev build --release
./target/release/git-oneflow --help
./target/release/gof --version
```

`./dev build` produces equivalent native executables named `git-oneflow` and
`gof`; both support the same commands, help, version, and exit statuses.
Cargo defaults to `git-oneflow`; use `./dev cargo run --locked --bin gof -- --help`
to select `gof`. Running without arguments shows help. Available options are
`--help` and `--version` (also `-h` and `-V`). Use `status --help`, `start --help`,
or `s --help` for command help.
All help and version invocations work outside a repository and without Git;
they do not read repository settings.

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

### Start or resume work

From a repository root or nested directory, run any equivalent form:

```sh
git-oneflow start "nasty deployment bug"
git-oneflow s "nasty deployment bug"
gof start "nasty deployment bug"
gof s "nasty deployment bug"
```

Use the built executable's full path when it is not on PATH. Supply one branch
name or phrase; quote spaces. `start` and `s` are exact command spellings.
Valid literal names keep their exact case, underscores, prefixes, and Unicode
normalization form: `feature`, `Fix/DeploymentBug`, and `fix/parser_v2` are
unchanged. Ordinary ASCII spaces trigger canonical Unicode phrase conversion:
lowercase letters/numbers and attached marks, with hyphenated separators.
For example, `nasty deployment bug` becomes `nasty-deployment-bug`.
The input determines only the branch target and is not stored as a description.

No prefix, numbered collision suffix, truncation, or product length limit is
added. Other invalid literal names and unusable phrases fail. Targets cannot be
`HEAD`, begin with `-`, or equal the configured base. `release/V1`,
`hotfix/Urgent`, and `notes` all use the same workstream workflow; prefixes do
not select behaviour. See the [precise naming contract](docs/START_INPUTS.md).

Start reports **created**, **resumed local**, **resumed remote**, or **already
current**. An existing local branch takes priority and is resumed without remote
contact or history/upstream changes. Otherwise start discovers exact remote heads
and fetches the target counterpart, or creates new work from the freshly fetched
remote base. A remote counterpart receives its exact upstream; a new branch has
no upstream. Local base history is untouched. Discovery/fetch errors stop the
operation; cached references are never a fallback.

Start requires a working tree with an attached branch and at least one commit.
Switching requires a clean index, working tree, and submodules. An exact current
branch is a read-only no-op even when dirty. Active Git operations and a target
checked out in another worktree block start. No history synchronisation is
performed. Successful starts/no-ops exit 0; repository, settings, target validation,
eligibility, and Git failures exit 1; malformed command syntax exits 2.

### Repository settings for start

Start defaults to the named remote `origin` and base branch `main`. Optionally
create `oneflow.conf` in the **Git common directory** (usually `.git/oneflow.conf`):

```text
# Choose a different remote/base pair
remote = upstream
base = trunk
```

Find the location with `git rev-parse --git-common-dir`; linked worktrees share
this file. It is local repository metadata, not tracked project content. The
remote must be configured in Git and its base must exist remotely when creating
new work. The base value is a branch name, without a remote prefix.

The file uses UTF-8, one `key = value` per line. Only `remote` and `base` are
accepted, once each. Surrounding whitespace is trimmed; blank lines and whole-line
`#` comments are ignored. Values are literal, without quoting, escapes, inline
comments, expansion, or includes. Missing files/keys use defaults; malformed,
unknown, duplicate, invalid, or unreadable settings fail. There are no global or
legacy settings sources. See the [settings contract](docs/START_INPUTS.md) for
exact validation details.

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
- **Start blocked:** preserve or resolve working-tree/index/submodule changes
  before switching. Complete or abort active Git operations yourself. Attach an
  existing branch if HEAD is detached, establish an initial commit if unborn,
  and use a checkout for bare repositories. If a target is occupied, use the
  reported worktree or free the branch there.
- **Start settings or remote failure:** check `oneflow.conf` in the Git common
  directory, the configured named remote (`git remote -v`), the remote base
  branch, and access credentials. Start never falls back to cached remote state.
- **Start partly completed:** errors list completed steps and observed remaining
  HEAD, target, fetched reference, and upstream configuration. No rollback is
  attempted; even a failing Git command or checkout hook can leave effects.
  Inspect the reported state and Git diagnostics before retrying. A created
  local branch is then a local resume; an already-current branch is a no-op.
  Output escapes control characters in names, paths, and Git diagnostics.
- **Literal `@` target:** Git 2.47.3 accepts this branch name but refuses
  `git switch @`; start reports the refusal and any completed creation/fetch.
  Use another literal name or resolve the branch checkout yourself.
- **Formatting or lint failures:** run `./dev fmt`, address Clippy diagnostics,
  then rerun `./dev check`.
- **Corrupt local tools or stale build outputs:** remove `.dev/` and `target/`
  from the repository root, then repeat bootstrap, build, and checks.
