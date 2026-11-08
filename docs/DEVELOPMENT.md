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
support require qualification beyond the focused native Git journey described
below. Shell completion is supplied by the executables
and uses the same locally installed Git boundary as inspection. Direct
integration's message editor launches the Git-selected command through a temporary
Git shell alias, using Git's own executable and shell resolution. `-m/--message` or
`finish-message = auto` bypasses that launcher. Native interactive rebase delegates
editor launching to Git. Configured finish checks use Linux/macOS process groups or
Windows Job Objects to manage child processes. Linux completion observation needs
readable `/proc`; macOS uses native process-information APIs. Linux tests do not establish
portability of these paths; native tests supply the scoped runtime evidence below.

## Development

For implementation, dependency, configuration, or tooling changes, run
one complete `./dev check` against the final result. Repeat it after subsequent
changes, failures, or unresolved concerns that warrant fresh validation. This is
also the Linux pull-request CI entry point: it checks formatting, builds both
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
their exact targets with `./dev test-groups`. Build the native fixtures before
focused groups, which all include the presentation unit tests, then select a
capability:

```sh
./dev cargo build --example native_check_fixture --example native_editor_fixture
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

Select the group matching the changed behavior. Every group includes library and
binary unit tests; shared targets can appear in several groups, while a full run
executes each suite once. `./dev test-groups` is the authoritative membership list.
Use narrow groups for iteration and complete validation before delivery.

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

Exercise the shared dispatcher through representative native executables and aliases.
Repeat expensive scenarios only when their interaction protects a distinct failure
mode. Retain real-Git interruption and fresh-process recovery tests where an effect
can commit before the application records its acknowledgement; mocked process
results cannot establish that boundary.

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

PowerShell adapter syntax requires 7.3 or newer; cross-platform candidate
qualification selects the maintained 7.6 line.
Completion tests use isolated repositories and profiles. Tests requiring optional
shell runtimes are marked ignored in the regular Cargo suite. Complete
`./dev test`, `./dev check`, and an unfiltered `./dev test-group completion`
then check executable availability and supported runtime versions and execute their ignored cases in separate test
processes. Missing local runtimes are reported as skipped; they count as neither
passes nor failures. `./dev test-shells` runs this environment-aware selection on
its own. Filtered Cargo invocations preserve their arguments; use `-- --ignored`
to select external-runtime cases explicitly after provisioning that runtime.

Native completion CI selects Ubuntu 22.04 and 24.04 x64, macOS 15 ARM64
and Intel, and Windows Server 2025 x64. Linux selects Fish, Zsh and PowerShell
interaction and profile-registration tests, plus Bash terminal coverage. Each
macOS architecture selects stock `/bin/bash` and `/bin/zsh`, Homebrew Bash and
Fish, with actual Tab input through a Unix PTY. Windows selects PowerShell 7.6
LTS through ConPTY and explicit command launch through cmd.exe and Git Bash.
The Windows job also probes adapter syntax and native argument transport using
an isolated historical PowerShell 7.3.12 runtime; this is a compatibility check,
not a maintained-runtime recommendation.

To reproduce the macOS selection, set `TWIGSMITH_TEST_BASH`,
`TWIGSMITH_TEST_ZSH` and `TWIGSMITH_TEST_FISH` to the intended absolute runtime
paths and run `./dev test-shells --require-all`. Repeat with
`TWIGSMITH_TEST_BASH=/bin/bash` for stock Bash. On Windows, build
`cargo build --locked --example native_argument_fixture`, then run
`cargo test --locked --test native_shell_windows -- --ignored --nocapture`.
Set `TWIGSMITH_TEST_PWSH`, `TWIGSMITH_TEST_CMD` and `TWIGSMITH_TEST_BASH`
to the selected PowerShell, cmd.exe and Git Bash executables. The non-Linux
`native_completion` target also checks automatic install/uninstall refusal for
existing and absent isolated profile files.

CI records source/tree, runner image, architecture, native compiler, Git and shell
paths/versions and terminal mechanism. A selected job or an adapter query alone
is not evidence of successful native Tab behavior; inspect the actual test result.
Missing required runtimes fail setup before the selected checks. The workflow
uses GitHub Actions platform timeout limits. Windows uses pinned native Cargo;
Unix jobs use `./dev`. These checks establish bounded shell behavior, separately
from whole-application support and shipped-artifact qualification.

### Native Git journey

Build the native fixtures with
`./dev cargo build --example native_check_fixture --example native_editor_fixture`, then run
`./dev test --test native_journey --test native_checks --test native_check_guards --test native_editor --test native_prompts -- --nocapture`.
Complete checks build the fixtures and include these suites. On Windows, use native
Cargo with the toolchain pinned in `rust-toolchain.toml`:
`cargo build --locked --example native_check_fixture --example native_editor_fixture`, followed by
`cargo test --locked --test native_journey --test native_checks --test native_check_guards --test native_editor --test native_prompts -- --nocapture`.
The tagged and hosted journeys plus filesystem recovery guards run with
`./dev test --test cli_oneflow --test cli_direct_sequence --test cli_hosted_oneflow --test cli_hosted_recovery --test native_recovery -- --nocapture`.
On Windows, use the same Cargo test selectors with `cargo test --locked`.
Build the fixtures before any focused group run; every group includes the
presentation unit tests that use terminal restoration. The presentation keyboard
unit test also uses the terminal restoration fixture. To run that boundary alone
after building the fixtures:
`./dev test --lib presentation::tests::keyboard_suggestions_and_described_menus_keep_literal_and_submission_contracts -- --nocapture`.

The test exercises help/version for both executable names, read-only status,
ordinary branch creation from a freshly advanced local remote, and an unreachable
remote refusal. Save/review and direct-integration cases exercise ordered native
checks and retained-candidate retry. Real Git checks refs, raw index preservation,
index-tree identity, tracked content and checkout cleanliness in isolated repositories with spaces in their
paths and literal punctuation in branch names. Fixtures supply their own Git
identity and configuration and require no credentials or network service.
The [generated-settings journeys](../tests/native_journey/bundles.rs), included in
`native_journey`, exercise GitHub Flow review and trunk direct/review with custom
branch names and separate project/work remotes. They check generation's Git-state
preservation, creation from a freshly advanced project base, save publication,
unfinished-work refusal, review retention, and direct checkout/cleanup. These are
actual CLI processes using generated shared settings. The
[GitFlow cases](../tests/cli_direct_sequence/bundles.rs), included in
`cli_direct_sequence`, exercise generated policies, independent release/hotfix
destinations, production-result version tags, active-release routing, and retry.
The companion process suite checks literal arguments, environment, EOF input,
streaming, output volume and managed descendant cancellation. Guard fixtures check
local mutations, hidden tracked work, ignored output, remote movement, retry,
explicit bypass and streamed credential redaction.

The pull-request `Native Git journey` matrix selects Ubuntu 22.04 x64, Windows
Server 2025 x64, macOS 15 ARM64 and macOS 15 Intel. Jobs verify runner/compiler
architecture, use the pinned native Rust toolchain and record source, image, OS,
Git and compiler versions alongside test observations in their logs. Missing or
failed jobs leave that environment unverified. Successful results cover the
tested journeys only; they do not qualify Windows desktop versions,
real shell Tab completion, packaging or installation. Published
distribution remains Linux x86_64 GNU.
The `native_editor` suite uses real Unix PTYs and Windows ConPTY with a foreground
Rust editor fixture. Both command names exercise merge/squash acceptance, literal
multiline messages, atomic save, cancellation after preparation and retry. Failure
cases check invalid message files and context changes independently through Git.
The Windows acceptance journey limits PATH to the directory containing `git.exe`,
with no `sh.exe` there. The harness bounds waits, cleans its process group/job and
checks terminal line input and echo afterward. This establishes the tested terminal
handoff, not arbitrary detached GUI editor containment or full interactive coverage.

The `native_prompts` suite exercises guided setup and representative save/finish
choices in capable terminals, with literal Unicode input, cancellation, context
revalidation and complete-line fallback. Setup coverage includes the bundle menu,
GitFlow's development default, editable production/active-release roles, and
explicit file creation. The configuration suite adds all-format generation,
supported/refused delivery routes, role validation, private layering, and guided
customization/cancellation; completion tests cover the new vocabulary and values.
The presentation unit test adds keyboard filtering, defaults, in-prompt Tab suggestions, narrow widths and colour suppression.
The native matrix selects both suites explicitly. These tests use the same PTY/ConPTY
harness as editor tests and check usable line input and echo after prompts.
A fallback-only execution does not establish capable keyboard behavior.

The tagged and hosted suites use real Git and disposable local remotes for
release/hotfix routing, fixed version identity, independent integration destinations,
partial publication and fresh-process recovery. Git executes the hook fixtures
through its own shell, including Git for Windows; configured checks use a native
Rust fixture. The `native_recovery` suite covers worktree isolation, settings
checkout and retained recovery storage. Capability-dependent filesystem cases
report their actual observations; an unavailable capability supplies no evidence
that its guard passed. These suites retain platform-specific historical cases
where their process or filesystem mechanism requires it. The native matrix selects
the portable cases alongside the earlier journey, process, editor and prompt tests.
Bundle journeys use those existing source-test targets and runner selections;
adding their cases establishes no new runner, desktop, or shipped-artifact scope.
Judge native execution from each job's actual results, including explicit skips
and unavailable capabilities.

The native suite selection uses `--no-fail-fast` so a failing test binary still
leaves evidence from the other selected binaries; the job remains unsuccessful
if any selected binary fails.

For optimized executables:

```sh
./dev build --release
./target/release/twigsmith --help
./target/release/tgi --version
```

## Native candidate packaging

Pull requests targeting `main` run `./dev check` and native completion checks
against the proposed integration. Superseded runs for the same pull request are
cancelled. Merging an unchanged, validated pull request does not repeat the source
suite or start artifact qualification.

The **Release qualification** workflow prepares the complete four-platform set on
explicit dispatch with `qualification=platform-packages` (the default). It builds
one archive per target and consumes the same Linux bytes on Ubuntu 22.04, 24.04
and 26.04. The additional Linux rows select Git 2.45.4 and 2.56.0 respectively,
retaining the feature-floor and reference checks without rebuilding the product.
It never publishes or attaches assets to a release. Creating a tag or publishing
a GitHub release starts no qualification or publication job.

Publishable native and packaging sources must be the recorded migration baseline
or later first-parent main integrations associated with merged pull requests and
successful foundation pull-request runs for their heads. Acquisition and publication
verify that eligibility against protected main; branch preparation does not establish
publication eligibility. Source validation is reused rather than repeated during
artifact qualification. Fixture preparation compiles harnesses, and qualification
executes the extracted binaries rather than compiling the consumer's application.

For a local development probe, use a fresh output and extraction directory:

```sh
./dev package-native .dev/tmp/native-probe --allow-dirty
./dev verify-native .dev/tmp/native-probe/twigsmith-1.0.0-rc.1-x86_64-unknown-linux-gnu.tar.gz --extract-to "/tmp/twigsmith probe"
./dev test-packaging
```

The version comes from matching `Cargo.toml` and `Cargo.lock` identities. Native
packaging accepts `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
`aarch64-apple-darwin` and `x86_64-apple-darwin` with `--target TARGET`, on a
matching native host. On Windows, invoke `python tools/native_package.py build
OUTPUT --target x86_64-pc-windows-msvc` with the pinned native Rust environment.
Packaging uses a fresh optimized
build with the pinned Rust toolchain and locked dependencies. It checks both
executable versions and publishes a complete output directory only after successful
verification; existing output is never overwritten. Local builds are labelled
`development-probe`; `--allow-dirty` permits uncommitted source for a probe.
CI candidates use a controlled clean checkout of the selected commit and record the
commit, tree, toolchain and CI run. This is provenance information, not independent
publisher authentication or a guarantee of reproducible builds.

The output contains a `.tar.gz` (Windows: `.zip`) with both regular executables,
using `.exe` names on Windows, the MIT license and
extraction guidance, a `.manifest.json` with member and archive hashes and build
metadata, and a `.sha256` file for the archive and manifest. Keep the three files
together. `verify-native` checks identity, hashes, archive members, platform modes and ELF/PE/Mach-O
architecture, and optionally extracts into a new directory. It runs without Rust.
The verifier supports native manifest schemas 2 and 3 for Linux and schema 4 for
Windows/macOS candidates. Schema 4 binds executable inspection to the payload,
including PE imports/subsystem or Mach-O dependencies/deployment information.
New platform candidates explicitly leave product OS support floors unqualified;
compiler deployment metadata and native hosted tests alone do not select a floor.
Runtime requirements are separate from build provenance.

`./dev platform-qualify --help` describes extracted-artifact runtime checks. The
fixture harnesses use real Git and select the extracted executables through an
isolated PATH without Rust or Node/npm. CI checks executable architecture, loader,
dependencies and runtime search paths, then performs actual startup and workflow
checks. It records the selected Git version, source, run and payload hashes along
with failure diagnostics. The Git feature floor is 2.45; the minimum-series sample
is 2.45.4 and the reference sample is 2.56.0. Results on a development host do
not establish compatibility with the selected Ubuntu runners. Preserve the glibc
2.35 build baseline when replacing the Ubuntu 22.04 runner; its retirement is
separate from Ubuntu's maintenance lifecycle.

## npm candidate packaging

`./dev package-npm SPEC OUTPUT` assembles one `twigsmith` installer
package from four verified native archives. `SPEC` is a JSON list with each
`archive`, independently expected `archive_sha256` and `payload_sha256`. Relative
paths are resolved beside the specification. All four targets must share a version
and native source. The output is one tarball and its schema-3 identity manifest;
this command neither publishes nor configures providers.

The package contains installer/launcher code and a generated native table, with no
executable or optional platform dependencies. The table binds exact version/target
release URLs, archive and executable hashes, and member identities to the verified
native inputs. `postinstall` selects and downloads one target when scripts are
approved. `twigsmith-install` provides deliberate selection and repair; the two
application aliases only launch verified installed bytes. See
[installation and script policy](USAGE.md#native-archives-and-npm-installation).
Native execution continues to need neither Node/npm nor Rust.

`./dev platform-qualify --help` describes native and installed npm checks. Prepare
harnesses once, then run the npm boundary with both candidate directories:

```sh
./dev platform-qualify npm --package-set-dir NPM_DIRECTORY --native-dir NATIVE_DIRECTORY --harness-dir HARNESS_DIRECTORY --evidence-dir NEW_EVIDENCE_DIRECTORY
```

Unpublished production URLs cannot supply an online installer yet. Local
qualification independently verifies the four native inputs against the production
package table, then creates a distinctly marked fixture tarball with only its
native URLs rebound to an isolated loopback server. Installer/launcher bytes remain
identical. Production approval rejects fixture metadata; there is no environment
switch that relaxes the installed production package's download-origin rules.
Fixtures establish the changed installer path and native execution, not a published
package's public availability.

For explicit hosted preparation, dispatch `Release qualification` with
`qualification=platform-packages`. It builds once on each native host and qualifies
the installer on Linux x64, Windows Server 2025 x64 and macOS 15 ARM64/Intel. Runtime
pairs are pinned in `platform-packages.yml`: Node 22.23.4/npm 11.20.0,
Node 24.22.2/npm 12.2.1 and Node 26.13.0/npm 11.20.0. Node 24 runs the full real-Git
selection; the other pairs retain launcher, editor and cancellation cases. Record
actual versions, both aliases, local/global installs, separate cold npx caches,
script-blocked refusal and explicit repair. No skipped or zero-child fixture is a
pass. The Node release catalogue and npm client engine requirements must be checked
when updating these pins.

This read-only preparation checks the selected revision; it does not itself confer
protected-main eligibility or publication authority. Native hosted execution also
does not model desktop browser security policy. Keep the actual host and unsigned
Windows/macOS limits in the qualification evidence.

### Integrated offline candidate sets

`tools/release_set.py NATIVE_DIRECTORY NPM_DIRECTORY --approval APPROVAL.json`
verifies a complete schema-3 installer set without contacting providers. The flat native
directory contains exactly four archives and their manifest/checksum sidecars;
the npm directory contains exactly one installer tarball and its manifest. The approval
binds the tag/version, each target's archive/manifest/payload hashes, native
source/tree and CI identities, packaging source/input hashes, requirements snapshots
and all package identities. Each artifact also has an exact run, attempt, artifact
ID, workflow path and downloaded ZIP hash. Review these values independently and
retain them for retries; regenerating expected hashes from changed files supplies
no independent approval.

To stage existing inputs locally, place those directories under `INPUT` and run:

```sh
python3 tools/release_stage.py prepare-platforms APPROVAL.json --input INPUT --output BUNDLE
python3 site/candidate.py BUNDLE > CANDIDATE.json
python3 site/build.py --bundle BUNDLE PREVIEW_DIRECTORY
```

The output directories must be new. Staging verifies before and after copying,
then exposes the completed bundle by renaming the temporary directory. Its
`identity.json` is the verified approval. `inspect-platforms` checks canonical
successful hosted run/attempt/artifact identities; `acquire-platforms` additionally
checks source eligibility, downloads only those artifacts, verifies their ZIP hashes
and exact member allowlists, and stages the set. Both operations use read-only
GitHub acquisition. They neither publish nor configure providers.

The generated integrated descriptor and preview describe local candidate bytes,
including every native target and the npm installer. Frozen candidate requirements can
still contain historical explicitly unqualified OS floors; format verification
does not qualify a desktop. New packaging selects one approved target snapshot
from `site/current-requirements.json`; the four-target envelope is not embedded
in a native manifest. Each manifest retains only its own frozen native/npm
requirements. Changes to current policy never update old packages or descriptors.
Historical 0.1.0 descriptors retain their
original requirements and rendering. The legacy publisher and stable published-site consumers refuse the integrated
schema. The separate RC operation below verifies public native assets before the
single npm version, then requires public consumption before `next` promotion.
Superseded schema-2 five-package approvals are rejected rather than reinterpreted.

### Candidate installation observations

`./dev installation-qualify --help` describes the browser and Windows runtime
probes. Explicit `Release qualification` dispatch with
`qualification=platform-installation` consumes `platform_approval`, the complete
reviewed identity JSON, and `runtime_sha256`, the independently reviewed official
x64 v14 Redistributable installer hash. It uses existing Windows Server 2025,
macOS 15 ARM64 and macOS 15 Intel runners. It creates no Windows 11 runner and
requires only read access to source and Actions artifacts.

The browser probe downloads the exact approved Actions artifact with the installed
headed browser, retains the original downloaded file, checks its ZIP hash and
allowlist, and uses OS extraction tools. On macOS it opens the original native
tarball with Archive Utility and retains the resulting files and attributes. It
records quarantine/Zone.Identifier inheritance and actual execution of both aliases
and a representative Git journey. Windows extraction and policy queries use the
selected PowerShell 7.6, recording actual executable and module versions.
An Actions artifact wraps the native archive in another ZIP; its observed security
metadata and extraction route do not establish the eventual direct release-asset
experience. Qualify that route again on published candidate bytes. The original
browser download must retain Internet-origin metadata; macOS also requires
quarantine after extraction. Windows records extracted Internet streams as present,
observed absent or unavailable. Observed absence limits the launch evidence: it
does not exercise protection triggered by marked payloads. Unavailable queries,
blocked execution or unavailable browser interaction leave explicit unmet checks.
The probe does not manufacture download metadata or disable protections; terminal
launch and signature queries do not establish a SmartScreen UI verdict.

The Windows runtime probe uses a disposable process-isolated official ServerCore
2025 container through the runner's existing Docker daemon. It checks an actual
missing-runtime failure for both aliases, installs the hash-pinned official
Microsoft-signed Redistributable, then requires successful version results. It
never removes host runtime DLLs or provisions a Docker daemon. The workflow starts
an already registered, enabled Docker service when stopped and records before/after
state and engine readiness. It leaves missing or disabled services unchanged.
A missing daemon, preinstalled runtime or different failure remains unmet. This is ServerCore
provisioning evidence; it is not desktop/UAC evidence. Reports record actual host
policies, including any security protections disabled by the hosted image.

The browser profile and short-lived signed download URLs stay outside uploaded
evidence. Review the reports' individual checks and limits, not just the job status.
These probes do not publish native assets, npm packages or the website.

## Release preparation and publication

The release process has three explicit operations: prepare the complete candidate,
publish its exact approved bytes under a staging tag, then separately promote the
publicly verified candidate to `next`. `release.yml` provides read-only preparation
and installation qualification; `publish.yml` provides only `publish-platforms` and
`promote-platforms`. Pushes, merges, tags and release events publish nothing.
There is no release-body request convention or automatic continuation.

Prepare the versioned source through a reviewed pull request and authorized
integration, then dispatch `Release qualification` on main with
`qualification=platform-packages`. Review the exact native and npm original Actions
artifacts, source/tree, run/attempt, hashes and host qualification before authorizing
any tag, release or package publication. Do not rebuild or relabel a candidate to
match an approval. The release tag and non-draft GitHub prerelease must be created
separately at the approved source; the publisher neither creates nor moves them.

A workflow-wide `twigsmith-release` concurrency group serializes publisher runs;
local execution takes the same repository lock. GitHub can replace an older pending
run: inspect actual state and retry the identical reviewed candidate when appropriate.
Pages deployment has its own independent `twigsmith-pages` group.

The hosted publication runtime is Node 24.22.2 with npm 12.2.1. These are verified
reference clients, not patch-level restrictions on users. npm failures report fixed
exit/status/authentication labels while raw provider output stays private. Provider
fixtures use disposable loopback services and real npm, never public publication;
credentials are forbidden with fixture endpoints. `./dev test-packaging` requires
Node/npm, a C compiler, loopback sockets and full canonical Git history for source
eligibility checks. It covers archive and npm integrity, acquisition, recovery,
public-consumption gates and site generation.

For read-only inspection from a full checkout, with an already reviewed request
and locally staged bundle:

```sh
python3 tools/release_platform_dispatch.py plan REQUEST.json --bundle BUNDLE
```

This checks source/artifact eligibility and reports the next pending operation.
It cannot publish or promote. Live effects use only the protected hosted path.

The **Publish approved release candidate** workflow accepts `operation=publish-platforms`
for integrated `1.0.0-rc.x` sets. Its request contains exactly `publisher_revision`
(current protected main SHA) and `approval` (the independently reviewed schema-3
single-installer identity from integrated preparation). Add the `publication` object to that approval:

```json
{
  "source": "EXACT_NATIVE_SOURCE_SHA",
  "release_id": 123456,
  "prerelease": true,
  "channel": "next",
  "staging_channel": "candidate-v1-rc-1"
}
```

The source and release ID are placeholders for the approved real identities. The
release and tag must already exist, with that tag resolving to the exact native
source. The workflow does not create either. Native source, packaging source and
publisher revision each require protected-main eligibility; all four native sources
must match. The approval binds every original Actions run/attempt/artifact and ZIP
hash, archive/manifest/payload hash, npm tarball and frozen requirements snapshot.
Prepare and independently review new versioned bytes; never relabel an older probe.

Preflight acquires the exact five original artifacts read-only and checks the full
bundle. It retains this attempt's initial `twigsmith` `latest`/`next` observations
and the stable Pages index hash with the immutable workflow input. Both
later stages refuse changed protected channels; only promotion may change common
`next` to the approved RC. The Pages index is reobserved at each stage boundary;
this checks the index, not every site asset. The `release` job uploads only absent
matching-approved native assets. All twelve must then be anonymously downloadable
at their exact public release URLs with approved hashes before it publishes the
single npm version under the explicit staging tag. Unknown or conflicting provider
state halts recovery; matching published bytes survive timeouts or lost responses.
`publish-platforms` runs the public installation matrix and stops without moving
`next`.

Read-only Windows Server 2025, macOS 15 ARM64/Intel and Ubuntu 22.04 jobs then fetch
public release assets and exact npm versions anonymously. They verify installed
payloads, both aliases, representative Git workflows, local/global installs and separate
cold-cache exact-version npx commands. Windows and Mac rows repeat direct browser
acquisition, security metadata, OS extraction and unsigned launch observations.
Public Mac browser probes retain only native `HOME`, `USER`, `LOGNAME` and `TMPDIR`
alongside the anonymous allowlist, with a fresh browser profile and isolated npm/Git
configuration. Native and npm/npx probes keep fresh isolated homes; credentials,
proxies and startup configuration are excluded from every public probe.
Browser download targets navigate directly after download behavior is configured;
matching browser completion events, filename and hash establish success. Evidence
records setup and progress operations so a timeout identifies the stalled step.
Missing or failed required checks block readiness; inspect individual child results,
script-policy behavior and host/security limits. npm 12's default denied lifecycle
is observed separately from approved automatic installation and explicit repair.
After reviewing this evidence, obtain separate owner authority for `next` promotion
and dispatch `operation=promote-platforms` with the same exact approval and current
reviewed publisher. This operation cannot upload assets or publish a missing npm
version. It repeats the read-only four-platform public matrix in its own attempt;
the final `release` job authenticates those four successful jobs and fresh reports,
reobserves immutable provider bytes and channels, and moves only `twigsmith@next`.
It never updates stable Pages or `latest`. Full-set stable publication is not enabled.

Retry the identical reviewed request after observing remote state; do not repack
published versions or replace assets. A changed current main requires a reviewed
publisher revision and a new complete attempt. The entire public matrix reruns on
a new attempt; previous-attempt artifacts and success flags cannot authorize
promotion. Reports older than one hour are refused. Source artifact expiry requires
retaining/recovering approved inputs through a separately reviewed path, rather
than substituting current artifacts. Concurrent provider changes cannot be excluded
atomically; a refusal preserves completed publication for explicit reconciliation.

The existing `twigsmith` npm package requires the exact `publish.yml` / `release`
trusted-publisher identity, direct-publish permission and independent dist-tag
permission. Inspect actual settings read-only before dispatch; any necessary
account/trust changes require separate owner authority. No new platform npm names
are needed. Publishing under a staging **tag** immediately exposes the version;
it is neither private staging nor rollback. See
[npm trusted publishers](https://docs.npmjs.com/trusted-publishers/) for current
account requirements. Never put account credentials or signed download URLs into
requests or uploaded evidence.

## Site artifacts and Pages

Create an unpublished four-platform installer preview from the verified bundle:

```sh
python3 site/candidate.py BUNDLE > CANDIDATE.json
python3 site/build.py --bundle BUNDLE PREVIEW_DIRECTORY
```

The descriptor binds all four native archive, manifest, checksum and payload
identities, the single npm installer, exact commands and frozen requirements.
Preview labels distinguish locally verified bytes from public availability.

The independent **Publish website** workflow is manually dispatched on main and has
only repository-read and Pages deployment permissions. It accepts the current
single-installer descriptor format, resolves the public stable GitHub release and
npm `latest`, and verifies their exact identities, native manifests and npm tarball.
Prereleases and obsolete descriptor formats are refused. The already deployed site
and published 0.1.0 assets remain external history; preparing new Pages source does
not deploy it or change `latest`. New stable-site deployment requires a reviewed
current-format descriptor and separately authorized public release and deployment.
The RC publication and promotion workflows never invoke Pages.

Pages jobs serialize independently. They require current main, capture source and
provider observations, stage an ordinary-file artifact, and recheck source, provider
identities and artifact immediately before deployment. Missing or conflicting data
stops the operation. Changes between reobservation and deployment cannot be excluded
atomically; a later authorized dispatch reconciles that race. Pages must already be
configured for GitHub Actions and the github-pages environment must permit deployment.
The public artifact excludes provider snapshots and private qualification evidence.

Local site builds stage output before directory replacement. A failed build preserves
the previous artifact; a failed replacement attempts restoration and retains/names
the backup if restoration also fails. The two renames briefly remove the output path;
this is not a process-crash transaction or an atomic reader-visible directory swap.
Assets use relative paths for the repository Pages base path.

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
