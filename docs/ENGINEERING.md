# Engineering reference

## Documentation map

| Document | Audience and purpose |
| --- | --- |
| [README](../README.md) | Project introduction, quick start, documentation links |
| [User guide](USAGE.md) | Commands, settings, automation, and recovery |
| [Development guide](DEVELOPMENT.md) | Local setup, builds, checks, troubleshooting |
| [Start contract](START.md) | Naming, settings validation, create/resume invariants |
| [Save contract](SAVE.md) | Planning, local preservation, publication, failure semantics |

## Implementation map

| Area | Modules and responsibility |
| --- | --- |
| Native entry points | `src/main.rs`, `src/bin/gof.rs`: shared CLI entry |
| CLI/UI | `src/cli.rs`, `start_cli.rs`, `save_cli.rs`, `status.rs`: parsing, interaction, presentation |
| Configuration | `src/settings.rs`: sparse layered configuration, schema, sources, legacy boundary |
| Git/process boundary | `src/git.rs`: literal arguments, explicit cwd, raw output/status |
| Repository observation | `src/repository.rs`: HEAD, changes, operations, upstream/local history |
| Start | `src/start.rs`, `start/workflow.rs`: prepare, select create/resume intent, execute |
| Save | `src/save.rs`, `save/execution.rs`, `save/publication.rs`: assess/revalidate, preserve, publish |
| Test fixtures | `tests/support/`: isolated real Git, preservation snapshots, Linux PTYs |

Rust 1.99.0 is pinned; use `./dev`. Tools/caches/temp files are local to `.dev/`,
builds to `target/`. Cargo lockfile is committed. Runtime dependencies are the two
Unicode crates plus Serde, serde_json, toml, and serde-saphyr. Parsing dependencies
use only the required deserialization features; YAML import/property/expression
features are disabled. Dev dependencies are tempfile and Linux-only rustix for PTYs.
CI bootstrap/check targets Ubuntu 24.04. Package version is `0.0.0-dev`,
`publish = false`, edition 2024, MIT.

## Design decisions

- Rebuild in Rust within this repository, preserving history, identity, and MIT
  license. Native/npm distribution and release automation are separate future work.
- Provide a small lifecycle CLI. OneFlow informs its design; branch prefixes are
  naming conventions, with no operational role or persistent role metadata.
- Keep normal application Git environment/configuration and hooks. Fixture
  isolation is confined to tests. Preserve raw bytes and process/I/O distinctions.
- Separate read-only planning from effects and CLI presentation. Revalidate before
  mutations and preserve observations of partial completion rather than rolling back.
- Start is one branch-name-or-phrase operand. Literal names retain exact spelling;
  ASCII-space phrases use canonical Unicode lowercase/hyphenation. Exact collisions
  mean resume. Local resume is offline; creation uses the freshly fetched configured
  remote base, preserving local base history. See [start contract](START.md).
- Save has four dirty-tree actions: publish-only, exact staged commit, separate
  staged/remaining WIP commits, or local stash. Clean trees publish directly.
  Explicit actions override `save-action`; unresolved unattended input fails.
  This grouping is product policy informed by Git capabilities, not a prevalence claim.
- Save publishes only the exact current branch to the configured remote's push
  destinations through non-forced updates. Strict upstream guards precede local
  preservation; missing tracking follows confirmed publication. Local preservation,
  remote acknowledgements, and tracking outcomes remain separate. See [save contract](SAVE.md).

## Limits to retain

- Inspection disables optional locks, lazy fetch, and fsmonitor. Git 2.45 is the
  minimum because of `--no-lazy-fetch`; tested runtime is Git 2.47.3.
- Save diff capture also disables index auto-refresh, external diff/textconv, and
  relative output. Ordinary diff can otherwise refresh an index during inspection.
- Snapshots are several reads, not atomic cross-process locks. Arbitrary hooks,
  filters, and helpers can leave effects. Signal termination can prevent reporting.
- Save checks content/index/context changes after input and between mutations.
  Content snapshots use memory proportional to included work. Scoped staging can
  encounter the operating system's argument-size limit. See the save contract.
- Publication contains expansive push configuration and unsafe fetch-to-local-head
  mappings. Nonstandard mapping/configuration failures need explicit user attention.
- Git 2.47.3 accepts literal branch `@` but refuses `git switch @`. Start safely
  reports the refusal; save uses an exact full ref for that branch.
- Status compares locally available history, including shallow boundaries. Missing
  comparison data is reported as unavailable; human text has no machine-output contract.

## Configuration implementation

Configuration uses one flat string schema for `remote`, `base`, and `save-action`
in every supported format. Each layer accepts at most one recognized file, without
extension priority. Legacy/new local coexistence is a migration error, including
empty files. Omitted legacy keys inherit shared settings and then defaults.
Every present value is validated even when overridden. Loading remains local and
read-only, with remote availability left to the workflow. Shared configuration
is worktree-specific; local overrides are common to linked worktrees. Parent/home
inheritance, custom paths, and external imports are absent.

Routine implementation choices: use [Serde](https://serde.rs/),
[serde_json](https://docs.rs/serde_json/), [toml](https://docs.rs/toml/), and the
maintained [serde-saphyr](https://github.com/bourumir-wyngs/serde-saphyr) parser.
Deserialize directly into one sparse schema so duplicate keys cannot disappear
into an intermediate map. Preserve scalar types rather than YAML string coercion;
empty YAML/JSON mappings are valid, while null/empty YAML documents fail the
mapping contract. YAML follows 1.2 boolean spelling, rejects merge keys and
unsupported tags, and keeps parser resource limits. Revalidation compares effective
values and their source paths; formatting alone does not change the settings
snapshot. Save action provenance distinguishes explicit Request selection,
configuration/defaults, and the clean-tree rule. Shared files remain ordinary Git
content: a stash or hook changing effective settings stops subsequent operations,
with completed preservation retained and reported.

Workflow research: [Ruka's 2021 hosted-provider OneFlow description](https://www.endoflineblog.com/implementing-oneflow-on-github-bitbucket-and-gitlab)
allows either merge or rebase for updating ordinary work and prefers squash PR
integration. It distinguishes upstream from fork publication and defines release/
hotfix tagging and merge-back procedures. A OneFlow bundle must not conflate branch
update policy, review submission, and final integration. Bundle selection is still
open; release/hotfix implementation remains deferred. The current single-remote
setting does not model separate upstream and fork roles.

Explicit starting references, release/hotfix operations, tagging policy, workflow
profiles, shell completion, extra aliases, installers, platform support, and npm
distribution remain deferred. A tagged starting point alone does not determine hotfix intent; releasing
may operate on an existing reference rather than a new branch.

## Historical reference

Legacy npm metadata/code is recoverable at `c6c9f70`, before removal in `2510bcd`:
`git-oneflow` 3.1.0, Node >=18, npm >=10, MIT, with both executable names.
These are manifest facts, not a live registry check or rewrite compatibility promise.

Configuration experiments at `next-gen` (`56b0e7e`) and `task/task-command`
(`adb52f7`) contain preset bundles. The latter uses explicit `finish --tag` and
manual PR instructions. They are design references, not verified implementations
of every named branching strategy or an accepted preset/API commitment.
