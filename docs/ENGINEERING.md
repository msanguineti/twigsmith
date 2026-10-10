# Engineering reference

This document records cross-cutting architecture, decisions, and current
implementation boundaries. User procedures are in the [user guide](USAGE.md);
workflow invariants are in the [start](START.md), [save](SAVE.md), and
[finish](FINISH.md) contracts. Contributor setup and checks are in the
[development guide](DEVELOPMENT.md).

## Current implementation

The native Rust CLI is invoked as `git-oneflow` or `gof`. It provides `start`,
`save`, `finish`, `status`, and `config`, with lifecycle aliases `s`, `v`, and `f`.
The finish library provides separate read-only review and direct-integration
assessment, controlled review remote refresh, branch preparation, verified durable rebase recovery,
configured checks of prepared work, and exact publication with destination-specific
safeguards. The finish CLI composes these backends and resolves consequential choices through flags, settings,
or terminal input. Guided setup exposes completion and preparation policies with
conditional integration questions and `ask` defaults.

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
| Finish planning, preparation, checks, publication, and recovery | `src/finish.rs`, `src/finish/integration.rs`, `src/finish/integration/recovery.rs`, `src/finish/preparation.rs`, `src/finish/checks.rs`, `src/check_process.rs`, `src/finish/publication.rs`, `src/finish/recovery.rs` |
| Shared branch tracking and push selection | `src/tracking.rs` |
| Shared exact-ref push response decoding | `src/publication.rs` |
| Isolated Git and terminal fixtures | `tests/support/` |

## Cross-cutting decisions

- Rebuild the native tool in Rust in this repository, preserving the existing
  Git history, project identity, and MIT license. Distribution and
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

Finish preparation and publication are review library APIs separate from CLI policy.
Direct integration has its own read-only plan and private-record verification model
in `finish::integration`. It shares Git observation/ancestry primitives with review,
not its working-branch publication assumptions. Plans separate preparation, local
target integration, pinned-result publication, and confirmed-delivery completion.
The [finish contract](FINISH.md#direct-integration-plans) defines stage effects,
recovery selection, exact target/version guards, and evidence limits. Integration
execution, record writing, refresh, and CLI enablement remain deferred.

Refresh observes the base remote and the publication remote's exact push
destinations, fetches the observed objects without changing branch/tag refs, and
supplies evidence to assessment. Preparation revalidates a resolved plan, repeats
remote observation, and rebases or merges
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

The CLI loads and resolves completion mode before refresh; review performs initial
assessment and refresh, resolves only plan-required
choices, then composes preparation, checks, and publication. It revalidates the
reviewed plan around each input-driven reassessment; preparation repeats remote observation
before branch execution. Rewrite authorization is independent of update selection.
Input requires terminal stdin and stderr with a complete submitted line. Unresolved
unattended input and cancellation return failure before branch execution, reporting
completed observations/fetches. Resolved flags/settings introduce no confirmation.
Completion follows the publication report, including tracking setup, and identifies
the branch, each destination, pinned commit, and manual submission next step.
Failure presentation retains backend effects and observed remaining state; Git
handles conflicts and continuation/abort before rerunning finish.

Finish-specific continuation/abort commands remain deferred. Submission is manual.
After completing its Git operations, the tool reports confirmed effects and concise
workflow-specific next steps. Provider APIs, PR creation, and post-completion
external-tool delegation are outside the accepted boundary. Configured validation
checks remain part of finish before publication.

Finish-check configuration is an ordered typed list of display labels and exact
process argument vectors. The local list replaces the shared list as a whole;
omission inherits and an explicit empty list disables inherited checks. Every
supplied layer is validated. Values and whole-list provenance participate in
settings equality and workflow/setup revalidation. Rendering uses each format's
serializer to preserve argument contents; loading preserves scalar types and
rejects unknown/duplicate fields and NUL in command elements. Inspection performs
no command execution or executable availability checks.

Finish publication executes the effective checks on every attempt, after
preparation and before any destination transport. `check_process` owns literal
process execution, streamed output events, and cancellation cleanup;
`finish::checks` owns sequencing and prepared-context revalidation; the CLI owns
progress, credential presentation, and `--skip-checks`. Publication invokes the
stage itself so library retries cannot reuse successful results. Check reports
are observations, never reusable publication authority. The default publication
API enforces checks; the event-aware API accepts an explicit per-call bypass.
Start and save never execute finish checks. Guided setup captures optional check
names and JSON process argument arrays in execution order, validates entries with
the settings validator, and reviews numbered literal commands. Review shows a
private replacement list as effective settings without exporting it. Setup never
executes checks or probes executables; unattended generation retains the empty
built-in list.

The supported Linux execution path uses Unix process groups and catchable
termination signals. The executor inherits environment variables and PATH,
provides EOF on stdin, and drains stdout/stderr without waiting for newline.
Checks are finite validation commands; ordinary background descendants are
stopped when their leader exits or execution is cancelled. Signal handlers span
the stage and restore default termination behavior outside it; concurrent library
check stages serialize ownership of this process-wide facility. SIGKILL and
commands deliberately escaping their process group cannot be managed; this is
process lifecycle management, not a sandbox. Other platforms require equivalent
process-tree cancellation before enabling configured check execution.

Prepared-context guards share publication's existing treatment of tracking refs
as non-authoritative hints. Relevant local mutations invalidate the attempt;
remote-only movement is detected by publication's destination observations and
exact leases. Checks cannot renew those observations or rewrite evidence.

Integration into the base branch, tagging, cleanup, workflow bundles, release
operations, shell completion, installers, and distribution remain future work.
Custom
configuration paths and `--config` are not implemented.

## Remote-role configuration

Fork topology is independent of branching strategy and update/integration policy.
The tool distinguishes the remote supplying the project base from the remote
receiving working-branch publication. These roles can refer to the same remote;
remote names such as origin, upstream, and fork have no built-in special meaning.
Configuration implements `publish-remote` and `base-remote` as effective named
roles in one settings model. Layers apply from shared to local. Validate every
supplied value before merging. An omitted base remote derives from the final
publication selection; an explicit base remote resolves independently and stays
selected across publication overrides.

Settings retain publication file/layer provenance, plus explicit base
provenance or the fact that the base follows publication. These participate in
settings equality and existing plan/setup revalidation. Inspection reports both
roles and their sources without checking availability. Generation emits
`publish-remote` and preserves explicit base selection when rendering it. Guided
setup asks for publication and a base source that follows publication or selects
a different remote. Equal choices retain the derived relationship and omit
`base-remote`; unattended generation uses `origin` with a
derived base. Local remote names and refs are offline hints, never role assignments.
Review resolves the proposed shared roles with validated private overrides and
reports provenance without exporting private values. Existing files remain
create-protected and are not rewritten.

Start and save execute with matching or separate roles. Start discovers the
exact publication branch for remote resume and otherwise fetches the project
base from the base remote. Local resume remains offline. Save publishes only
the exact current branch through the publication remote and needs no base-remote
access. Neither workflow synchronizes local base history or the fork base.

Finish refreshes and prepares against `base-remote`, then publishes the exact
working branch through `publish-remote`. Plans bind both names, the resolved base
fetch endpoint, and ordered publication destinations. Version-2 durable rebase
records bind both roles, endpoints, remote configuration, and URL-rewrite
configuration. Unsupported or malformed records cannot authorize history
replacement. The [finish contract](FINISH.md#durable-rewrite-verification) defines
the verification boundaries. Project-base observations cannot replace
incorporation evidence at publication destinations.

The accepted workflow direction is: for a fork, new work starts from the project
base, published work resumes from the publication remote, save publishes there,
and finish prepares against the project
base before publishing to the publication remote. Manual submission guidance must
identify both the publication-remote/branch and project-base-remote/branch.
The configured publication role governs gof operations; ordinary Git commands
continue to use the developer's Git configuration. Fork synchronization remains
separate future work. Provider submission follows the manual completion boundary
described above.

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
records the Git evidence behind this policy.

## Workflow design references

### Evidence informing the design

The authoritative references are Ruka's
[2017 OneFlow description](https://www.endoflineblog.com/oneflow-a-git-branching-model-and-workflow)
and [2021 hosted adaptation](https://www.endoflineblog.com/implementing-oneflow-on-github-bitbucket-and-gitlab).
They describe variants, not one mandatory integration method.

- The original ordinary-work options are rebase plus fast-forward integration,
  merge without fast-forward, and rebase plus merge without fast-forward.
- Releases start at a selected development-line commit; hotfixes start from a
  released version. Both tag their tip before merge-back. The optional second
  long-lived branch advances to the released tag as a marker.
- The original hotfix guidance routes integration to an active release when one
  exists. The hosted article does not restate that case; target selection needs
  an explicit policy rather than branch-name inference.
- Hosted ordinary review also supports squash integration. Tagged release/hotfix
  merge-back uses a separate branch and non-fast-forward integration, preserving
  the tagged source while conflicts are resolved.
- Ordinary contributions can target a release or hotfix branch. Their target
  differs from the primary development line without making the contribution
  itself a release operation.

Updating the work branch and integrating it into a target are separate decisions.
Git's [merge contract](https://git-scm.com/docs/git-merge) distinguishes
fast-forward-only integration, a merge commit, and squash followed by a new
commit; message handling depends on whether a new commit is created. Existing
finish update policies describe work-branch preparation, not these integration
choices.

The accepted command boundary is one `finish` command, with completion behavior
selected through configuration and explicit per-run overrides. Ordinary work can
end at delivery for manual review or at direct Git integration and publication of
the project target. Unresolved required choices use the established interactive
input policy. `finish-mode` selects `ask`, `review`, or `integrate`; explicit mode
flags override
local/shared/default settings. Review uses the existing backends. Direct integration
is execution-blocked before refresh or workflow effects pending its complete backend.
`finish-integration`, `finish-message`, `finish-checkout`, and `finish-cleanup` expose
its invocation contract; every layer validates them even when inactive. Explicit
integration options require integrate mode and do not imply completion selection.
Settings and sources participate in equality/revalidation. Guided review omits
integration-specific settings; unattended generation retains unresolved policies.

Accepted ordinary-work completion policy:

- Offer fast-forward-only, merge-commit (non-fast-forward), and squash integration.
  Configuration or flags select the method; an unresolved required selection
  prompts rather than imposing a universal method.
- `-m/--message` supplies the final integration commit message. Preparation merges
  retain their own Git-generated messages. When a final integration commit is
  needed and no message is supplied, offer an editable suggestion under the
  established input policy. Fast-forward integration creates no commit and needs
  no message. Message policy remains configurable.
- After direct integration and confirmed target publication, finish on the local
  integration target by default. Checkout behavior remains configurable.
- The OneFlow bundle defaults to deleting completed local and published work
  branches after confirmed target publication. Retention remains configurable.
  Delivery for review keeps its source branch checked out and available.

Rerunning `finish` should resume a verified unfinished direct-integration attempt,
including pending publication and cleanup, instead of blindly creating another
integration. A small private operation record binds the source, target, integration
result, and remaining steps. Resume must corroborate the record against current
Git and remote state; it supplies neither reusable permission nor authority to
delete changed branches. Git remains responsible for conflict resolution and
continuation/abort of operations it supports. The read-only integration model
implements these recovery assessment boundaries separately from review's durable
rebase evidence. Execution remains deferred.

Automatic recovery selection is scoped to the current work branch and worktree.
An unfinished attempt for branch A must not intercept `finish` on branch B, nor
may starting B's attempt overwrite A's recovery evidence. Records in another
worktree do not select the current operation. A shared target branch such as
`main` is insufficient by itself to identify an attempt; recovery after target
checkout must retain an unambiguous association with the same interrupted attempt.

Accepted ordinary-work failure policy:

- Readiness failures stop before integration. Preserve actual Git state on
  conflicts, hook failures, and interruption, and report completed effects.
- Failed checks stop publication and cleanup. Validate the effective result
  again before a later publication attempt.
- A moving remote target stops the attempt pending fresh assessment. Preserve
  shared target history and retain the recoverable integration result.
- Rejected pushes retain local results and work branches. An uncertain push
  requires remote observation before retry; confirmed publication resumes only
  remaining completion steps.
- Checkout or cleanup failure after publication reports delivered integration
  with incomplete completion. Return nonzero while required steps remain.
- Cleanup verifies the exact source/result and target publication, and deletes
  only expected branch versions. Retain branches containing new work. Resume
  interrupted cleanup without repeating integration.
- Completed effects remain completed; do not automatically undo publication,
  discard conflict resolutions, or reset unrelated local work.

Recovery design must account for squash semantics. Git's
[merge contract](https://git-scm.com/docs/git-merge) specifies that squash does not
record `MERGE_HEAD`; normal merge continuation/abort instructions therefore do
not describe squash recovery. A squash result cannot generally be verified through
source ancestry. Git's [branch deletion check](https://git-scm.com/docs/git-branch) tests
merged ancestry against upstream or HEAD, which is not proof of squash integration
into the intended target. Recovery and cleanup need evidence connecting the exact
source to its actual integration result and confirmed target publication.

The finish contract specifies implemented integration planning, recovery
verification, and prospective cleanup safeguards. Integration execution and its
publication sequencing remain future work. The existing contracts remain
authoritative for implemented behavior.
