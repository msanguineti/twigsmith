# Engineering reference

This document records cross-cutting architecture, decisions, and current
implementation boundaries. User procedures are in the [user guide](USAGE.md);
workflow invariants are in the [start](START.md), [save](SAVE.md), and
[finish](FINISH.md) contracts. Contributor setup and checks are in the
[development guide](DEVELOPMENT.md).

## Current implementation

The native Rust CLI is invoked as `git-oneflow` or `gof`. It provides `start`,
`save`, `status`, and `config`. The finish library provides read-only assessment,
controlled remote refresh, branch preparation, verified durable rebase recovery,
and exact publication with destination-specific safeguards. A finish CLI remains
deferred.

The application separates repository observation and planning from effects and
command presentation. Plans are revalidated before mutations. Multi-step
operations report observed partial completion; they do not roll back completed
effects. Exact workflow requirements and limitations belong in the corresponding
contract.

| Area | Main modules |
| --- | --- |
| Entry points and command dispatch | `src/main.rs`, `src/bin/gof.rs`, `src/cli.rs` |
| Command interaction | `src/start_cli.rs`, `src/save_cli.rs`, `src/config_cli.rs`, `src/status.rs` |
| Configuration | `src/settings.rs`, `src/config.rs` |
| Git process boundary and repository observation | `src/git.rs`, `src/repository.rs` |
| Start | `src/start.rs`, `src/start/workflow.rs` |
| Save and publication | `src/save.rs`, `src/save/execution.rs`, `src/save/publication.rs` |
| Finish planning, preparation, publication, and recovery | `src/finish.rs`, `src/finish/preparation.rs`, `src/finish/publication.rs`, `src/finish/recovery.rs` |
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

Finish preparation is a library API separate from the CLI. Refresh observes the
configured remote base and exact push destinations, fetches the observed objects
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

CLI interaction and finish-specific continuation/abort commands remain deferred;
developers recover with Git and reassess finish. The accepted delivery direction
is preparation and publication followed by a manual submission handoff, unless
an explicitly configured provider integration handles that step.

Provider adapters and their settings remain undecided. Configurable checks,
integration into the base branch, tagging, cleanup, workflow profiles, release
operations, shell completion, installers, and distribution remain future work.
