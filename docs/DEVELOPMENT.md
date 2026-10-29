# Development

## Setup

On Linux, you need Python 3.10 or newer for packaging regression checks, a POSIX
shell, standard Unix utilities, `curl` with HTTPS
support, and a C compiler/linker driver available as `cc`, with system headers
and libc development files. Bootstrap requires internet access.
On Debian/Ubuntu, install the native prerequisites with
`sudo apt-get install build-essential curl ca-certificates git python3`.
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

The Inquire/Crossterm prompt backend documents Unix and Windows terminal support,
including ordinary Windows console use. Application Windows and macOS build/runtime
support have not been validated. Shell completion is supplied by the executables
and uses the same locally installed Git boundary as inspection. Direct
integration's message editor currently launches the
Git-selected editor through POSIX `sh` on PATH. `-m/--message` or
`finish-message = auto` bypasses that launcher. Native interactive rebase delegates
editor launching to Git. Configured finish checks use Unix process-group management
and are unsupported on non-Unix systems. Linux tests do not establish portability
of these paths.

## Development

For implementation, dependency, configuration, or tooling changes, run
one complete `./dev check` against the final result. Repeat it after subsequent
changes, failures, or unresolved concerns that warrant fresh validation. This is
also the Linux foundation CI entry point: it checks formatting, builds both
executables, runs Clippy with warnings treated as errors, runs the regular suite,
and then selects external-shell tests whose runtimes are available locally.
Formatting, build, and lint failures stop the check before tests start. Once
running tests locally, complete `./dev test` and `./dev check` use Cargo's
`--no-fail-fast`: test failures do not stop later suites. Full runs execute
CLI smoke/activity, configuration, shared foundations/status/completion, start,
save, finish/history, and finally end-to-end lifecycle/direct/hosted suites.
Library and binary unit tests run with the shared foundations; doctests and every
available external-shell runtime follow the regular integration suites.
Suites run sequentially while each suite retains Rust's normal internal test
parallelism. New auto-discovered suites in `tests/*.rs` or `tests/*/main.rs` append
after the preferred sequence until assigned a position in `dev`. Explicit Cargo
manifest test targets using other paths require updating that discovery. Cargo
compiles its ordinary targets, including examples, before the ordered run. `./dev test-shells` and an unfiltered
`./dev test-group completion` also collect failures across their test stages.
Cargo reports failed targets and individual tests; the launcher ends with a
failed-stage/runtime summary and exits unsuccessfully if any stage failed.
CI sets `TWIGSMITH_DEV_FAIL_FAST=1` for Unix launcher invocations: full runs,
native-shell runs, and an unfiltered `test-group completion` stop after the first
failed test suite or stage, report that failure, and exit unsuccessfully. Each
suite keeps its normal internal test parallelism and completes its test harness;
later Cargo targets and native runtimes do not start. Successful runs retain the
same coverage. Reproduce that mode locally with
`TWIGSMITH_DEV_FAIL_FAST=1 ./dev check`; leave the variable unset or set it to `0`
to collect failures across test stages. The native-completion OS matrix keeps
independent jobs running when another platform fails.
Focused test invocations preserve Cargo's default failure behavior and forward
caller arguments unchanged; pass `--no-fail-fast` when collecting all failures
in a focused selection. Compilation errors can still prevent Cargo from starting
its tests.

For documentation-only changes, check links and anchors, examples and command
syntax, technical claims against source and existing tests, and whitespace with
`git diff --check`. Builds, application tests, Rust formatting, and Clippy are
unnecessary when executable behaviour and tooling are unchanged.

| Command | Purpose |
| --- | --- |
| `./dev build` | Build both debug executables |
| `./dev test` | Run tests |
| `./dev test-shells` | Run native shell tests available on this host |
| `./dev test-shells --require-all` | Require the host’s complete native shell environment before testing |
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
./dev test-group smoke
./dev test-group completion
./dev test-group finish-planning
./dev test-group oneflow
./dev test-group hosted-oneflow
./dev test-group history-edit
```

The groups are `start`, `save`, `finish`, `config`, `status`, `shared`,
`lifecycle`, `smoke`, `completion`, `finish-planning`, `history-edit`, `oneflow`, and `hosted-oneflow`. Command groups include
backend and CLI coverage. `completion` checks candidate contexts, registration and shell insertion, including
literal Readline insertion.
`smoke` pairs CLI smoke checks with development
launcher selection and completion. `finish-planning` selects review finish planning and
eligibility coverage. `oneflow` selects direct release and hotfix command lifecycles,
ordered destination delivery, selected version identity, and one-time preparation
recovery.
`history-edit` selects ordinary native history editing and fresh-process recovery.
`hosted-oneflow` selects version publication, separate merge-back preparation, and
fresh-process recovery for manual submission. `shared` covers the development launcher, native packaging/qualification tooling, Git boundary, repository observation,
settings, tracking, remote roles, CLI activity, finish-check configuration, and exact save/finish
publication. `lifecycle` includes CLI smoke checks, end-to-end project/fork
workflows, and direct and hosted release/hotfix delivery and recovery; this broad group
includes expensive lifecycle coverage. Prefer the narrower groups during fast
iteration, then run full validation before submitting changes. Every group also runs
library and binary unit tests (`--lib --bins`). Targets overlap where they
exercise shared behaviour. Overlap selects shared tests for relevant development
work; a full run executes the ordered suites once rather than executing each group.

The `finish` group does not include `cli_oneflow`, `cli_direct_sequence`, `cli_hosted_oneflow`, or
`cli_hosted_recovery`, or `cli_hosted_presentation`. For tagged release/hotfix
completion changes, also select `oneflow` and `hosted-oneflow`, or select the affected targets directly
with Cargo. Neither focused group replaces complete validation.

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

Focused tests support iteration. The full commands, `./dev test` without
arguments and `./dev check`, run the ordered sequence described above, including
CLI smoke and end-to-end tests. New conventional integration suites participate
regardless of group membership; update the sequence and group definitions in `dev`
when assigning a suite its logical position and focused selections.

## Test quality

Tests should detect meaningful regressions with reliable results, clear failure
diagnostics, and reasonable execution and maintenance cost. Use these principles
when adding or changing coverage:

- Verify observable behavior, documented contracts, or explicitly justified
  architectural invariants; each test should protect a meaningful case.
- Keep tests deterministic and independent of order, use the simplest adequate
  setup, and avoid coupling assertions to incidental implementation details.
- Do not duplicate equivalent assertions without a distinct regression purpose.
  Update tests when behavior changes. Defect fixes should include a regression
  case that fails with the defect and passes with the fix when practical.
- Separate expensive tests from fast iteration where practical while retaining
  them in full validation. Correct or remove obsolete, redundant, flaky, or
  consistently low-value tests. Keep reliable protection for important contracts
  when replacing flaky tests.

Detailed CLI finish and OneFlow safety scenarios exercise the shared canonical
dispatcher. Focused matrices verify both native executables and lifecycle aliases
with real effects; repeat expensive scenarios across entry points when they
protect a distinct failure mode.

For expensive workflow matrices, combine factors when their interaction changes
behavior. Keep preparation mode and remote-role combinations that distinguish
base selection from publication. Check existing base/publication tracking through
representative finish executions with each remote layout; preserve all outcome and
configuration assertions without repeating every preparation combination.
Retain real-Git interruption and fresh-process recovery tests where the contract
depends on an effect committing before the application records its acknowledgement.
Mocks of process results cannot establish that boundary.

Judge the suite by the important defects it catches, its reliability and
diagnostic value, and its total cost. Test count and raw coverage percentage are
not goals; keep full validation comprehensive and focused selections useful.

## Run a build

```sh
./target/debug/twigsmith --help
./target/debug/tgi status
./dev cargo run --locked -- start add-user-avatars
./dev cargo run --locked --bin tgi -- --help
```

Cargo defaults to `twigsmith`. The development launcher runs from this
project's root. To use another checkout, run a built executable from that checkout.

### Shell completion

Build with `./dev build` and put `target/debug` on PATH. Follow the
[user manual](USAGE.md#shell-completion) to load or register completion for Bash,
Zsh, Fish or PowerShell. The sourceable `completions/twigsmith.bash` is the Bash
adapter used by executable-generated session setup; it needs a built executable
on PATH. Grammar and repository candidates live in Rust.

Completion runtime coverage on Linux includes Bash 5.2.37, Zsh 5.9, Fish 4.0.2
and PowerShell 7.5.4. PowerShell completion requires 7.3 or newer.
Completion tests use isolated repositories and profiles. Tests requiring optional
shell runtimes are marked ignored in the regular Cargo suite. Complete
`./dev test`, `./dev check`, and an unfiltered `./dev test-group completion`
then check executable availability and supported runtime versions and execute their ignored cases in separate test
processes. Missing local runtimes are reported as skipped; they count as neither
passes nor failures. `./dev test-shells` runs this environment-aware selection on
its own. Filtered Cargo invocations preserve their arguments; use `-- --ignored`
to select external-runtime cases explicitly after provisioning that runtime.

Native completion CI provisions Linux, macOS, and Windows runners. Linux executes
Fish, Zsh and PowerShell interaction and profile-registration tests; Bash coverage
also runs in the foundation suite. macOS tests Bash, Fish and Zsh session adapters;
Windows tests PowerShell session completion. Both non-Linux jobs verify that
persistent profile installation refuses changes, matching the current Linux-only
metadata-safety contract. macOS Zsh adapter checks do not establish interactive
Tab behavior; Linux retains the terminal interaction tests.

CI checks its required runtime environment before selecting tests. A missing
required runtime is a setup error with tests not started, rather than a failed
native behavior assertion. The workflow has no explicit job timeout; GitHub
Actions applies its platform limits. Windows uses the pinned Rust toolchain
with native Cargo; the Unix jobs use `./dev`. Real interaction evidence and
measured latency belong to the review evidence for a completion change. Native
completion checks establish their tested shell contracts, rather than
whole-application platform compatibility.

For optimized executables:

```sh
./dev build --release
./target/release/twigsmith --help
./target/release/tgi --version
```

## Native candidate packaging

Normal push and pull-request CI runs `./dev check`, packaging regression tests,
and the native completion checks. Manual candidate preparation builds a single
Linux x86_64 GNU archive on Ubuntu 22.04 and qualifies those same bytes on Ubuntu
22.04, 24.04 and 26.04. It does not publish them. Manual dispatch requires the
workflow to be enabled for dispatch on the repository's default branch.

For a local development probe, use a fresh output and extraction directory:

```sh
./dev package-native .dev/tmp/native-probe --allow-dirty
./dev verify-native .dev/tmp/native-probe/twigsmith-0.1.0-beta.1-x86_64-unknown-linux-gnu.tar.gz --extract-to "/tmp/twigsmith probe"
./dev test-packaging
```

The version comes from matching `Cargo.toml` and `Cargo.lock` identities. The
only target is `x86_64-unknown-linux-gnu`. Packaging uses a fresh optimized
build with the pinned Rust toolchain and locked dependencies. It checks both
executable versions and publishes a complete output directory only after successful
verification; existing output is never overwritten. Local builds are labelled
`development-probe`; `--allow-dirty` permits uncommitted source for a probe.
CI candidates use a controlled clean checkout of the selected commit and record the
commit, tree, toolchain and CI run. This is provenance information, not independent
publisher authentication or a guarantee of reproducible builds.

The output contains a `.tar.gz` with both regular executables, the MIT license and
extraction guidance, a `.manifest.json` with member and archive hashes and build
metadata, and a `.sha256` file for the archive and manifest. Keep the three files
together. `verify-native` checks identity, hashes, archive members, modes and ELF
architecture, and optionally extracts into a new directory. It runs without Rust.
The current verifier handles the current manifest format; earlier retained
candidates can be verified with the tooling from their source revision.

`./dev native-qualify --help` describes extracted-artifact runtime checks. The
fixture harnesses use real Git and select the extracted executables through an
isolated PATH without Rust or Node/npm. CI checks executable architecture, loader,
dependencies and runtime search paths, then performs actual startup and workflow
checks. It records the selected Git version, source, run and payload hashes along
with failure diagnostics. The Git feature floor is 2.45; the minimum-series sample
is 2.45.4 and the reference sample is 2.56.0. Results on a development host do
not establish compatibility with the selected Ubuntu runners. Preserve the glibc
2.35 build baseline when replacing the Ubuntu 22.04 runner; its retirement is
separate from Ubuntu's maintenance lifecycle.

## Troubleshooting

- **Missing Rust tools or components:** repeat `./dev bootstrap`, especially
  after a toolchain change.
- **Download/certificate failure:** check network access, proxy settings, and
  system CA certificates. Bootstrap uses `sh.rustup.rs` and `static.rust-lang.org`.
- **Linker failure:** install the native compiler, linker, headers, and libc
  development files; on Debian/Ubuntu these are supplied by `build-essential`.
- **Formatting/lint failure:** run `./dev fmt`, address Clippy diagnostics, and
  repeat `./dev check`.
- **Corrupt tools/build outputs:** remove the affected tool/cache directories
  under `.dev/` and build outputs under `target/`, then bootstrap, build, and check
  again. Remove only the affected cache or tool directory; `.dev/` may also contain
  unrelated local files.

## Contributing

Keep documentation aligned with substantive changes. The [user manual](USAGE.md)
explains observable command behaviour, configuration, and recovery; the
[technical manual](ENGINEERING.md) explains implementation responsibilities,
invariants, and rationale. Update or remove superseded explanations instead of
adding change histories. Keep setup and validation instructions in this guide.

Use the validation appropriate to the change described above.
