# Engineering reference

This document records cross-cutting architecture, decisions, and current
implementation boundaries. User procedures are in the [user guide](USAGE.md);
workflow invariants are in the [start](START.md), [save](SAVE.md), and
[finish](FINISH.md) contracts. Contributor setup and checks are in the
[development guide](DEVELOPMENT.md).

## Current implementation

The native Rust CLI is invoked as `git-oneflow` or `gof`. It provides `start`,
`save`, `status`, and `config`. The finish library provides read-only assessment
and revalidation; finish execution and a finish CLI are not implemented.

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
| Finish planning | `src/finish.rs` |
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

Finish settings and read-only planning are implemented. Later work may add fresh
remote observation, branch preparation, safe publication, retry evidence, and a
CLI. These are future capabilities, not current command behavior. The accepted
direction is to prepare and publish work first; submission to a code review
provider is a separate, explicitly configured integration. Without one, a
successful publication hands the next submission step to the developer. The
finish contract records executor obligations and the limits of current planning.

Provider adapters and their settings remain undecided. Configurable checks,
integration into the base branch, tagging, cleanup, workflow profiles, release
operations, shell completion, installers, and distribution remain future work.
