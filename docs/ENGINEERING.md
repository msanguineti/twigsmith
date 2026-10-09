# Engineering reference

This document records cross-cutting architecture, decisions, and current
implementation boundaries. User procedures are in the [user guide](USAGE.md);
workflow invariants are in the [start](START.md), [save](SAVE.md), and
[finish](FINISH.md) contracts. Contributor setup and checks are in the
[development guide](DEVELOPMENT.md).

## Current implementation

The native Rust CLI is invoked as `git-oneflow` or `gof`. It provides `start`,
`save`, `finish`, `status`, and `config`, with lifecycle aliases `s`, `v`, and `f`.
The finish library provides read-only
assessment, controlled remote refresh, branch preparation, verified durable rebase recovery,
and exact publication with destination-specific safeguards. The finish CLI
composes these backends and resolves consequential choices through flags, settings,
or terminal input. Guided setup exposes both finish policies with `ask` defaults.

The application separates repository observation and planning from effects and
command presentation. Plans are revalidated before mutations. Multi-step
operations report observed partial completion; they do not roll back completed
effects. Exact workflow requirements and limitations belong in the corresponding
contract.

| Area | Main modules |
| --- | --- |
| Entry points and command dispatch | `src/main.rs`, `src/bin/gof.rs`, `src/cli.rs` |
| Command interaction | `src/start_cli.rs`, `src/save_cli.rs`, `src/finish_cli.rs`, `src/config_cli.rs`, `src/status.rs` |
| Configuration | `src/settings.rs`, `src/config.rs` |
| Git process boundary and repository observation | `src/git.rs`, `src/repository.rs` |
| Start | `src/start.rs`, `src/start/workflow.rs` |
| Save and publication | `src/save.rs`, `src/save/execution.rs`, `src/save/publication.rs` |
| Finish planning, preparation, publication, and recovery | `src/finish.rs`, `src/finish/preparation.rs`, `src/finish/publication.rs`, `src/finish/recovery.rs` |
| Shared branch tracking and push selection | `src/tracking.rs` |
| Shared exact-ref push response decoding | `src/publication.rs` |
| Isolated Git and terminal fixtures | `tests/support/` |

## Cross-cutting decisions

- Rebuild the native tool in Rust in this repository, preserving the existing
  Git history, project identity, and MIT license. Native/npm distribution and
  release automation are separate future work.
- Provide a small lifecycle CLI. OneFlow informs the design; branch prefixes
  are naming conventions and have no operational role or persistent metadata.
- Use the developer's normal Git configuration, environment, hooks, and helpers
  for application operations. Test isolation belongs in fixtures.
- Keep planning read-only and separate from CLI presentation and effects.
  Revalidate before mutations and report partial completion rather than
  automatically undoing completed work.
- Resolve consequential workflow choices from explicit requests or settings.
  Ask when an interactive choice is needed; unattended execution must stop with
  actionable guidance when required input is unresolved. Routine implementation
  choices and established safety invariants do not require new prompts.

The workflow contracts define the exact start, save, and finish behavior. The
user guide defines supported commands and configuration. Keep those documents
authoritative for their respective details rather than duplicating their
technical specifications here.

## Finish architecture and limits

Finish preparation and publication are library APIs separate from CLI policy.
Refresh observes the configured remote base and exact push destinations, fetches the observed objects
without changing branch/tag refs, and supplies evidence to assessment. Preparation
revalidates a resolved plan, repeats remote observation, and rebases or merges
against the observed base. Effective settings and committed configuration changes
stop subsequent execution pending fresh assessment.

Private per-worktree rebase records bind original incorporation to Git reflog
anchors and verified operation outcomes. Assessment reads these records; only
preparation writes them. They carry recovery/publication evidence, not branch
roles, reusable permission, or a workflow database. Missing, expired, or changed
history cannot authorize replacement. The finish contract defines the precise
verification and retry boundaries.

Publication consumes a prepared plan, revalidates local context, and rereads each
exact destination before publishing the pinned commit. Literal per-destination
leases protect creation and updates; verified rewrite evidence and resolved
permission are required for replacement. Each transport uses a command-scoped
remote with one explicit push URL, preserving transport options while excluding
expansive selection defaults. Tracking setup and porcelain decoding reuse save's
primitives; save's non-forced publication policy is unchanged.

Retries corroborate actual destination state and retain the existing rewrite
record. Publication requires no final journal write to survive a lost
acknowledgement or killed executor: fresh equality proves completed destinations,
and the original verified observations constrain outstanding replacements.
Reports separate confirmed effects, rejection, uncertainty, and tracking errors.
Cross-destination publication is sequential and non-atomic; details and transport
limits are in the finish contract.

The CLI performs initial assessment and refresh, resolves only plan-required
choices, then composes preparation and publication. It revalidates the reviewed
plan around each input-driven reassessment; preparation repeats remote observation
before branch execution. Rewrite authorization is independent of update selection.
Input requires terminal stdin and stderr with a complete submitted line. Unresolved
unattended input and cancellation return failure before branch execution, reporting
completed observations/fetches. Resolved flags/settings introduce no confirmation.
Completion follows the publication report, including tracking setup, and identifies
the branch, each destination, pinned commit, and manual submission next step.
Failure presentation retains backend effects and observed remaining state; Git
handles conflicts and continuation/abort before rerunning finish.

Finish-specific continuation/abort
commands remain deferred. Submission is manual; provider integration remains
accepted future direction only when explicitly configured.

Provider adapters and their settings remain undecided. Configurable checks,
integration into the base branch, tagging, cleanup, workflow profiles, release
operations, shell completion, installers, and distribution remain future work.

## Remote-role configuration

Fork topology is independent of branching strategy and update/integration policy.
The tool should distinguish the remote supplying the project base from the remote
receiving working-branch publication. These roles can refer to the same remote;
remote names such as origin, upstream, and fork have no built-in special meaning.
Configuration implements `publish-remote` and `base-remote` as effective named
roles in one settings model. `remote` is an input compatibility alias for
publication, not a separate effective remote. Within a layer, `publish-remote`
wins over `remote`; layers apply from shared to local, so a local `remote` also
wins over a shared `publish-remote`. Validate every supplied value before merging.
An omitted base remote derives from the final publication selection; an explicit
base remote resolves independently and stays selected across publication overrides.

Settings retain publication file/layer and alias provenance, plus explicit base
provenance or the fact that the base follows publication. These participate in
settings equality and existing plan/setup revalidation. Inspection reports both
roles and their sources without checking availability. Generation emits the
publication key and preserves explicit base selection when rendering it; guided
setup retains its single-remote question and leaves base selection derived.

Start and save execute with matching or separate roles. Start discovers the
exact publication branch for remote resume and otherwise fetches the project
base from the base remote. Local resume remains offline. Save publishes only
the exact current branch through the publication remote and needs no base-remote
access. Neither workflow synchronizes local base history or the fork base.

Finish assessment retains a matching-role guard until its remote refresh and
recovery support separate roles. Refresh, preparation, and publication revalidate
through that assessment before affected mutations, including changed or
caller-modified plans. Matching roles retain the existing finish execution path.

The accepted workflow direction is: for a fork, new work starts from the project
base, published work resumes from the publication remote, save publishes there,
and finish prepares against the project
base before publishing to the publication remote. Manual submission guidance must
identify the project base separately from the published branch. Fork synchronization
and provider integration are separate future capabilities.

Remote-role changes must participate in revalidation and verified rewrite recovery;
new publication destinations cannot inherit proof or authorization from old ones.

Shared tracking validation preserves both existing publication-branch tracking and
project-base tracking with separate publication. New branches keep the existing
no-tracking-until-publication behaviour; remote resume tracks the exact branch on
the publication remote. Successful publication can establish missing publication
tracking, but must not silently rewrite valid existing base tracking. Role
settings, raw tracking and explicit push-selection values, and required resolved
destinations participate in revalidation. The last branch pushRemote
overrides the last remote.pushDefault; a conflicting effective explicit value
is rejected. The configured publication role remains destination authority.
Implicit defaults do not broaden exact operations, and user-owned Git routing
settings remain unchanged. The [save contract](SAVE.md#preconditions-and-local-evidence)
records the Git evidence and compatibility effects of this policy.
