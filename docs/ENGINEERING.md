# Technical manual

git-oneflow is a Rust application that composes ordinary Git operations into
three workstream commands: start, save, and finish. This manual explains the
implementation and the invariants that matter when diagnosing or changing it.
The [user manual](USAGE.md) covers commands and recovery procedures; the
[development guide](DEVELOPMENT.md) covers setup, builds, and validation.

Read the architecture and Git boundary first, then follow the relevant workflow:

- [Repository state and the Git boundary](#repository-state-and-the-git-boundary)
- [Configuration, remote roles, and tracking](#configuration-remote-roles-and-tracking)
- [Start](#start-resolve-a-name-then-create-or-resume) and
  [save](#save-preserve-selected-work-then-publish)
- [Finish for review](#finish-for-review-prepare-check-and-deliver)
- [Publication reports and failure diagnosis](#publication-reports-and-failure-diagnosis)
- [Direct integration library](#direct-integration-library)

## Architecture and execution model

Both executables, `git-oneflow` and `gof`, enter the same
[command dispatcher](../src/cli.rs). The lifecycle aliases `s`, `v`, and `f`
use the same command implementations. CLI modules parse arguments, resolve
interactive choices, render reports, and choose exit status. Workflow libraries
return typed plans, effects, errors, and observations without terminal prompts.
Help is handled before repository discovery or configuration loading.

The implementation has three main boundaries:

| Boundary | Responsibility |
| --- | --- |
| Observation | [Repository inspection](../src/repository.rs) discovers Git paths, HEAD, working-tree changes, active operations, and local upstream information. [Settings](../src/settings.rs) resolves repository configuration. |
| Planning | Start resolves a target; save assesses local preservation and publication; finish assesses history and destinations. Plans retain the context and choices on which their proposed effects depend. |
| Execution | Workflow-specific executors revalidate plans before effects, run Git or configured checks, and report both completed effects and remaining state. |

This separation makes assessment usable without performing the operation. A
ready plan means its current choices and constraints are resolved. It is neither
an execution result nor lasting authority to push, overwrite, or delete a ref.
Changing a choice produces a new assessment; changing relevant context invalidates
the existing one. Execution owns additional checks between its steps.

Operations are deliberately nontransactional. A fetch can obtain objects, a
commit can succeed before a hook fails, and a push can succeed before its response
is lost. Automatic rollback could discard work or undo an effect another process
has already used. Reports therefore retain command status and diagnostics,
confirmed effects, and independently observed remaining state. An unavailable
observation stays unavailable rather than being represented as unchanged or clean.

CLI syntax errors return status `2`; operational failure and unresolved input
return `1`. Successful command completion returns `0`. Save and review finish
require complete publication and valid tracking, with no execution error, for
successful completion; cancellation or incomplete execution returns `1`.
Guided configuration cancellation succeeds without creating a file. Resolved
lifecycle flags and settings require no further confirmation. Interactive choices
require terminal stdin and stderr and a complete submitted line; EOF or
cancellation stops the pending operation. Effects completed before that point
remain reportable.

### Current capability boundary

Start creates or resumes an exact work branch. Save preserves selected local
progress and publishes the current branch. CLI finish prepares, checks, and
publishes that branch for manual review. It leaves review submission to the
developer and identifies the publication branch and project base in its handoff.
It creates no pull request, tag, or release and invokes no provider submission tool.

Direct integration is a separate library capability in
[`finish::integration`](../src/finish/integration.rs). It provides read-only
planning and recovery assessment, local work preparation and target integration,
and validation of the resulting candidate. It has no target publication,
completion checkout/cleanup, or remote-refresh executor. Although the CLI parses
integration options and configuration, selecting integrate mode stops before
workflow effects. Review plans cannot be passed to the integration executor or
used as target publication authority.

## Repository state and the Git boundary

The application uses the installed Git executable with literal process arguments.
It honors Git's normal repository-selection environment, configuration, hooks,
signing, credential helpers, and transport helpers. Application execution does
not substitute an isolated HOME or ignore the developer's Git configuration;
environment isolation belongs to [test fixtures](../tests/support/mod.rs).

[`git.rs`](../src/git.rs) provides a general raw executor and two internal policies:

- Inspection disables optional locks, lazy object fetching, and fsmonitor, and
  uses noninteractive credentials and C-locale diagnostics. Planning performs no
  fetch, hook execution, ref/configuration write, or worktree/index mutation.
- Workflow execution permits required locks and writes while retaining the
  no-lazy-fetch, fsmonitor-disabled, noninteractive-transport boundary. Workflows
  explicitly select their fetch and publication effects.

Save's diff observation also disables index auto-refresh, external diff,
textconv, relative output, and submodule-ignore settings. Optional locks alone
do not prevent ordinary diff from refreshing an index; explicit
`diff.autoRefreshIndex=false` keeps assessment observational. This Git behavior
is documented in [git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-diffautoRefreshIndex)
and covered by [save planning fixtures](../tests/save_planning.rs).

Repository discovery distinguishes the working-tree root, worktree Git directory,
and common Git directory. Linked worktrees have separate indexes, HEADs, operation
state, and private recovery records, while sharing local configuration and branch
refs. Execution and final observation use the verified worktree root so a switch
that removes the caller's nested directory does not lose the recovery report.
Git's inherited selection environment must still select the prepared repository
from that root; otherwise execution stops.

The observation model distinguishes attached, detached, and unborn HEAD; bare
repositories have no working-tree observation. Changes retain staged and unstaged
sides separately, so one path can appear in both scopes. Conflicts carry merge-side
status rather than masquerading as staged/unstaged work. Submodule state separates
commit-pointer changes from dirty child contents. Observed operation families
include merge, rebase, cherry-pick, revert, am, bisect, and relevant sequencer state.

Repository inspection supports contexts that mutation workflows reject. In
particular, `status` can describe detached, unborn, and bare repositories. Its
upstream comparisons use local refs and locally available history only; they
provide no remote freshness. Workflow eligibility is checked separately.

Git names, paths, object IDs, and diagnostics remain bytes where their semantics
require it. CLI presentation escapes control characters and backslashes and uses
lossy UTF-8 only for display. Literal path operands use `--literal-pathspecs` and
`--` when staging. Full head refs and verified commit IDs keep revision-expression
and tag ambiguity out of branch selection and publication.

### Snapshot limits

Revalidation repeats observation and compares the relevant approved context.
Save snapshots include HEAD, repository identity, settings and sources, raw routing
values, resolved push URLs, index path/bytes, scope, tracked diff, and included
file bytes, permissions, or symlink targets. Untracked directories are inspected
recursively without following symlinks. Changes to contents can invalidate a plan
even when path categories remain the same; an index refresh can conservatively
invalidate it even when selected content is unchanged.

Finish additionally binds effective Git configuration, committed shared-settings
blob IDs, history boundaries, fresh remote evidence, and recovery state. Integration
also binds source/target/HEAD reflogs, worktree occupancy, selected private records,
and retention refs. The full effective Git configuration is compared conservatively,
so even an unrelated changed value can require reassessment. Settings-file formatting
alone does not invalidate equal resolved values and sources, unless the committed
configuration blob changes.

These are several reads, not an atomic snapshot or cross-process lock. Restored
transient changes can escape observation, and hooks can act after a guard. Remote
observations require separate refresh; revalidation with old supplied evidence
cannot detect remote-only movement. Content snapshots use memory proportional
to included work, and literal staging arguments remain subject to OS argument-size
limits. The executors report these failures without broadening their scope.

## Configuration, remote roles, and tracking

All lifecycle commands share the [settings model](../src/settings.rs). Shared
settings live at the discovered worktree root; private overrides live in the
common Git directory and apply across linked worktrees. Bare repositories have
only the local layer. Each layer independently permits one recognized
`git-oneflow.config.toml`, `.yaml`, `.yml`, or `.json` file. Multiple recognized
files in one layer are ambiguous; extensions have no priority. There is no
parent/home inheritance, custom path selection, or external import.

Resolution is sparse per key: explicit supported request choices override local,
shared, and built-in values. Every supplied layer is parsed and validated,
including overridden and inactive values. Unknown or duplicate keys, invalid
types/UTF-8, malformed syntax, unreadable files, and dangling symlinks fail.
YAML retains scalar types and rejects merge keys and unsupported tags. Only
absence supplies defaults; an empty mapping inherits all settings.

Resolved settings retain their sources, including exact shared/local paths.
Provenance participates in equality and revalidation. The ordered `finish-checks`
list is one setting: a local list replaces the shared list completely, omission
inherits it, and an explicit empty list disables inherited checks. Loading and
inspection validate check vectors without executing or probing executables.

The two remote roles are independent of branch-update and integration policy:

- `publish-remote` supplies remote work-branch resume and receives save/review
  publication.
- `base-remote` supplies the project base for creation and finish preparation;
  direct integration's target is the configured base branch on this remote.

The roles may name the same remote. An omitted base remote follows the final
publication selection after layering; an explicit base selection stays independent.
Remote names such as `origin`, `upstream`, and `fork` carry no special meaning.
Settings validate names as ref components rather than URLs or paths, without
requiring that a remote or branch already exist. Lifecycle execution supplies
those availability checks. Branch prefixes likewise carry no operational meaning
or persistent role metadata.

[`config.rs`](../src/config.rs) separates inspection, a proposed generation plan,
guided review, and file creation. Guided review resolves private overrides without
exporting them, retains offline branch/remote hints, and revalidates before writing.
Generation stages complete serialized bytes and publishes them without clobbering
an existing recognized file. It creates configuration without changing branches
or probing checks. The local-only `redact-credentials` preference controls finish
presentation and is excluded from shared settings. Redaction changes displayed
HTTP(S) userinfo, including streamed check output, without changing executed
arguments. [`finish_cli.rs`](../src/finish_cli.rs) owns that presentation policy.

### Tracking is a relationship, not destination authority

[`tracking.rs`](../src/tracking.rs) validates raw upstream and explicit push-routing
values for start, save, and review finish. Tracking may be absent, or contain
exactly one remote and one full merge ref matching either the work branch on the
publication remote or the configured base branch on the base remote. Incomplete,
duplicate (even equal), empty, local `.` and unrelated relationships fail. A
missing cache or restrictive fetch mapping does not invalidate an otherwise
accepted raw relationship.

The effective explicit Git push selection must agree with the publication role.
Git's last `branch.<name>.pushRemote` value overrides its last `remote.pushDefault`
value; both raw lists are snapshotted, including masked values. The precedence
comes from [Git configuration](https://git-scm.com/docs/git-config#Documentation/git-config.txt-branchltnamegtpushRemote)
and real-Git fixtures in [remote-role tests](../tests/remote_roles.rs). Rejecting
conflicting overrides and duplicate upstream values is this application's
containment policy, rather than a restriction Git imposes universally.

Existing valid tracking stays unchanged, including base tracking when publication
uses another remote. Only absent tracking is established after every publication
destination confirms success. Two separate local configuration writes set the
publication remote and exact work-branch merge ref, with guards between writes.
A partial write stays visible for explicit repair; public assessment rejects
that incomplete relationship. Publication, tracking, and execution errors remain
independent report facts.

## Start: resolve a name, then create or resume

[`start::prepare`](../src/start.rs) discovers context, loads settings, resolves a
target, and snapshots its routing configuration without reading the index or
contacting remotes. Input containing ASCII space is a phrase; other input is an
exact literal branch name. Phrase conversion normalizes to NFC, applies Rust's
locale-independent Unicode lowercase, normalizes again, retains Unicode letters
and numbers plus attached combining marks, and collapses other runs to ASCII
hyphens. It adds no prefix, suffix, truncation, or collision repair. Literal names
retain case and Unicode normalization form.

Targets are checked as full `refs/heads/<name>` refs, avoiding checkout-shorthand
expansion by `check-ref-format --branch`. Leading `-`, reserved `HEAD`, NUL,
revision expressions, and the exact configured base name are rejected. Unicode
dependency/toolchain changes can affect phrase names; the
[resolution tests](../tests/start_resolution.rs) cover literal and canonical behavior.

[`start::execute`](../src/start/workflow.rs) requires a working tree and attached,
existing HEAD, with no active Git operation or occupation of the target by another
worktree. It chooses one action in order:

| Action | Selection and effect |
| --- | --- |
| Already current | Exact target is current; success can retain a dirty tree and performs no remote contact. |
| Resume local | Exact local branch exists; switch without remote contact, history update, or tracking changes. |
| Resume remote | Exact publication-remote head exists; fetch it, create its local counterpart, set exact publication tracking, and switch. |
| Create | Publication-remote head is absent; fetch the exact base head from the base remote, create an untracked work branch there, and switch. |

Every switch path requires a clean index/worktree, including nonignored untracked
work and submodule dirtiness. Git switching also refuses to overwrite ignored
files and retains normal worktree/index safety checks. A target on the base remote
is not a remote-resume candidate when publication uses another remote.

Remote discovery accepts only full exact head refs from `ls-remote`; tags and
tracking caches cannot establish existence or absence. Fetch selects the head
with an explicit forced head-to-`refs/remotes/` refspec and empty refmap, overriding
restrictive/negative fetch mappings. It refreshes only that tracking ref and
creates the branch at the actually fetched commit, so a base advancing between
discovery and fetch supplies its newer tip. Deletion or fetch refusal stops the
selected path without cache fallback. Symbolic tracking destinations are refused.

The forced fetch updates a cache, never existing local branch history. Fetch
disables tags, pruning, recursive submodule fetching, FETCH_HEAD writes,
commit-graph writes, and automatic maintenance. New branch creation explicitly
disables automatic tracking. Remote resume installs the exact publication
remote/merge pair even if Git's fetch mapping cannot resolve a local upstream
comparison. Orphan upstream configuration on an absent local target blocks
creation before fetch rather than being silently reused.

Before each effect, start rechecks context/settings, attached HEAD, operation and
worktree guards, expected target version, cleanliness, raw routing values, and
resolved fetch destinations. Its own tracking writes advance only the expected
routing snapshot. Fetch, branch creation, tracking writes, and switching are
separate effects. After partial failure, an existing created target is treated
as local resume on retry. A post-checkout hook failure can leave HEAD switched;
the remaining-state report preserves that fact. Git's switch parser can also
refuse a syntactically valid literal such as `@`; start reports that refusal
without renaming or adding a detached-checkout workaround. These cases are covered
by [start workflow tests](../tests/start_workflow.rs).

## Save: preserve selected work, then publish

[`save::assess`](../src/save.rs) records the attached branch/commit, local scope,
settings, required input, ordered preservation steps, and exact publication
destination. It performs no writes or remote contact. Save protects the configured
base and rejects conflicts, active operations, and dirty submodule contents;
clean submodule commit-pointer changes can be preserved in the parent repository.

Scope separates the current index, remaining tracked changes, and nonignored
untracked paths. Partially staged files can occur in both the staged and remaining
groups. A clean tree always resolves to publish-only, regardless of preference,
and creates no empty checkpoint or message request.

| Dirty-tree action | Local preservation before publication |
| --- | --- |
| Publish-only | Leave all uncommitted work unchanged and publish existing commits. |
| Staged | Commit the current index with the supplied nonblank message; remaining work stays local. |
| WIP | Commit the current index as `[WIP - Staged]`, then stage and commit remaining included work as `[WIP - Unstaged]`; omit empty groups. |
| Stash | Use an include-untracked local stash and publish existing commits; retain the stash without applying or dropping it. |

Unresolved action/message input exposes no executable local steps or publication.
Staged selection on a dirty tree requires staged changes; a dirty staged commit
also requires a nonblank message.
The [local executor](../src/save/execution.rs) runs publication preflight before
checkpoint writes, checks identity and known index-lock failures, and revalidates
the approved scope. Staged commits use ordinary index-based Git commit semantics;
WIP staging passes only approved root-relative literal byte paths. Ignored
untracked work stays excluded. Normal commit cleanup, signing, and hooks apply.

After each commit, execution checks the expected single parent, the tree change
against the previous index selection, the resulting index, and preservation of
remaining work. After staging, it checks unchanged context/content and absence
of paths outside the approved scope. Legitimate transitions such as a staged
deletion followed by a recreated untracked file are handled. Hook-created or
hook-modified work stops later steps while retaining the hook's effects.
Stash completion requires observed clearance of included work and retained stash
history; an identical already-retained stash OID is acceptable.

Each successor is freshly assessed as publish-only before the next step. Stashing
a changed shared configuration can therefore complete preservation but stop
publication when effective settings change. No automatic stash restoration, WIP
cleanup, history rewrite, or rollback is performed. A retry assesses actual state:
completed checkpoints do not repeat, while remaining staged work is treated as
the current index rather than remembered as an earlier WIP group. Preservation
and hook-failure cases are exercised in [save execution tests](../tests/save_execution.rs).

### Exact save publication

[`save::publication`](../src/save/publication.rs) accepts a resolved plan with no
pending local steps. It permits dirty publish-only work and an existing index
lock because publication performs no staging, commit, stash, or index edit.
It revalidates the selected branch/commit and scope, then pushes through the
configured remote name with a literal `<approved-oid>:refs/heads/<branch>`
refspec. A pre-push hook moving the local branch cannot change that pinned source.

Save uses Git's ordinary non-forced creation/fast-forward/equality rules. It
does not fetch, repair history, or use a lease. All resolved push URLs remain
selected, including repeats and Git's fallback to multiple fetch URLs when no
push URLs exist. An inaccessible base remote does not affect save.

Exact-ref containment also requires controlling Git's incidental effects:

- An explicit refspec, command-scoped `mirror=false`, `--no-force`, and
  `--no-follow-tags` exclude expansive ref selection and history replacement.
- `push.autoSetupRemote=false` prevents incidental upstream writes. Recursive
  submodule publication is disabled, including implicit recursion, while the
  nonmutating `push.recurseSubmodules=check` policy stays effective.
- Git can update fetch-mapped local refs after pushing. Positive fetch mappings
  with destinations must have full destinations under `refs/remotes/`; any
  mapping elsewhere is refused, including unrelated mappings. Matching symbolic
  tracking refs are refused. Negative mappings, source-only refspecs, and
  restrictive tracking mappings remain supported.
- Legacy remote definitions without configured URL values are refused because
  their separate mappings cannot be checked through this configuration boundary.

Signing, receive-pack, push options, credentials, helpers, and normal hooks remain
effective. Unsafe configuration is reported rather than rewritten. The containment
rationale is supported by positive-control ordinary Git examples in
[save publication tests](../tests/save_publication.rs): even a non-forced
single-branch push can change local branch history through a fetch mapping.

## Finish for review: prepare, check, and deliver

The CLI resolves completion mode before refresh. Integrate stops at the capability
boundary; review composes the following library stages:

```text
assess local readiness → refresh exact remote evidence → resolve required choices
    → prepare branch → run configured checks → revalidate → publish exact commit
```

[`finish::assess`](../src/finish.rs) reuses save's local eligibility, snapshot,
tracking, and publication preflight. It additionally requires a clean index,
worktree, and submodules, and resolves the base remote's fetch endpoint. WIP
commit messages do not make a clean branch ineligible.

### Remote evidence and history

Local conventional tracking refs are diagnostic hints. `RemoteEvidence` binds
the exact base and work refs, both remote roles, the resolved base fetch endpoint,
and one observation per publication push URL in configuration order. The caller
must supply fresh truthful observations; assessment validates their bindings
and local history, without authenticating or contacting the remote. Absent,
unavailable, and ambiguous refs are distinct. Repeated URLs require agreeing
observations. Full object IDs must match the repository hash length.

The relationship model distinguishes equality, proven incorporation, behind,
diverged, unrelated, uncertain, and unavailable history. Equality and positive
ancestry remain useful in shallow repositories. Negative ancestry conclusions
are conservatively uncertain when shallow boundaries exist; traversal errors
stay unavailable. Ancestry disables replacement objects, and legacy grafts block
approval because they can invent parentage. These safeguards are application
policy, with the model covered by [finish planning tests](../tests/finish_planning.rs).

Fresh inclusion of the base resolves update to `none`, irrespective of preference.
Otherwise the chosen update is rebase, merge, or none. Missing or uncertain
evidence prevents history-dependent choices. Unrelated base history prevents
rebase/merge approval; explicit none can retain it when observations are complete.
The remote base remains required even for none.

Published work must already be incorporated before preparation may replace it.
If a destination has work missing locally, rewrite `allow` cannot clear the
blocker. Rebase can require a separate rewrite choice when incorporated published
commits may be replaced. A commit already contained in both source and new base
survives without that choice. Preparation reassesses the actual resulting graph;
it does not treat a predicted rebase as proof. Merge and none preserve incorporated
history. A configured allow supplies standing permission, while input supplies
permission for the reviewed invocation; both still require exact incorporation
evidence and destination guards.

### Controlled refresh and branch preparation

[`finish::preparation`](../src/finish/preparation.rs) observes the exact base
through the named base remote and work ref at every resolved publication push
URL. It checks that a resolved push URL will not be redirected by another rewrite
when read. Every endpoint must be readable; transport failure never establishes
absence. Advertised commits are fetched by full OID with source-only refspecs and
an empty refmap. This obtains objects without advancing tracking refs, tags, or
local branches. A changed advertised ref cannot silently substitute a different
commit during fetch, and refusal to serve the pinned object stops refresh.
Tags, pruning, submodule recursion, FETCH_HEAD, maintenance, and commit-graph
writes are disabled. Git documents these controls in
[git-fetch](https://git-scm.com/docs/git-fetch).

Preparation requires a ready plan, repeats refresh, and rejects changed context
or observations before updating the branch. Rebase uses the fetched base OID,
preserves merge structure with `--rebase-merges=no-rebase-cousins`, and disables
update-refs, fork-point, autosquash, and autostash. Merge uses that base with Git's
normal generated message and no recursive submodule update. None performs no
history update. Merge recreation follows [Git rebase semantics](https://git-scm.com/docs/git-rebase)
and can require resolving earlier conflicts again.

After preparation, assessment checks the actual clean attached branch and graph.
Changed settings, committed configuration, Git configuration, branch, routing,
or endpoints stop later execution. Reports retain observations, fetched objects,
private recovery writes, verified branch changes, command output, and remaining
state. Preparation never publishes or establishes tracking.

### Rebase recovery proves incorporation

Before a review rebase, [`finish/recovery.rs`](../src/finish/recovery.rs) writes
an atomic version-2 record at
`<worktree-git-dir>/oneflow-finish/branches/<branch>/rewrite.json`. Owner-only
files/directories on Unix and symlink rejection protect its private storage.
The record binds repository/worktree identity, refs, remote roles/endpoints,
ordered original destination observations, original commits, fetched bases,
selected remote/URL-rewrite configuration, and branch/HEAD reflog anchors.
It stores operational evidence rather than reusable rewrite permission.
Rebase starts only when both reflogs anchor the current commit.

Assessment corroborates original incorporation and the anchored rebase interval:
the unique marked start, supported successful finish, fetched-base incorporation,
and resulting branch tip. Later commits or merges must preserve that verified
result through positive ancestry. An abort or unstarted attempt supplies no new
proof. Further finish rebases can extend the verified chain while publication
is pending, including after partial publication.

Git continuation can finish in another process, and verification needs no final
journal acknowledgement after Git succeeds. A covered exact original destination
commit can then qualify for `VerifiedRewrite`, with current allow still required.
New remote work cannot borrow that record. Ordinary incorporated destinations
continue to use normal ancestry even when a record is invalid.

Expired reflogs, missing objects, changed identities/endpoints/configuration,
discarded result history, unsupported versions, or malformed records stop proof
of replacement. Records remain local, are not automatically cleaned up, and are
neither a backup nor a branch-role database. Recovery relies on recognizable Git
reflog conventions and cannot authenticate against a party able to forge both
history/logs and private records. Continuation, abortion, invalid evidence, and
cross-process retry are covered in [finish preparation tests](../tests/finish_preparation.rs).

### Checks validate the current candidate

[`finish::checks`](../src/finish/checks.rs) sequences the effective ordered list
after preparation and before destination transport. The publication API invokes
checks on every attempt, including retries and already-published destinations.
Past success is an observation, never authority to omit this attempt's checks.
The event-aware API accepts an explicit per-call bypass; CLI `--skip-checks`
supplies it without changing hooks, preparation, rewrite permission, or leases.
Start, save, and configuration inspection execute no finish checks.

Each check is a nonblank display name plus a nonempty exact argument vector.
The first element is the executable; empty later arguments are retained and NUL
is rejected. There is no shell splitting or interpolation. Shell behavior requires
an explicit shell executable in the vector. Checks run sequentially from the
worktree root, inherit environment/PATH, receive EOF on stdin, and stream stdout
and stderr without requiring newlines.

The shared runner guards prepared context before execution and after each
successful check. Mutated branch/content/index, active operations, settings,
routing, or recovery evidence stop later checks and publication. Ordinary ignored
build outputs remain permitted by cleanliness rules. Changed tracking caches
cannot invalidate or renew authoritative remote evidence. Remote movement during
checks is caught by publication observations and leases.

[`check_process.rs`](../src/check_process.rs) manages Unix process groups and
catchable cancellation signals across the stage. It stops ordinary descendants
when their leader exits or cancellation occurs and reaps the leader before
returning. Concurrent library check stages serialize ownership of this process-wide
signal facility. Non-Unix configured execution is unsupported. SIGKILL and deliberate
process-group escape are outside the lifecycle guarantee; checks are project code,
not sandboxed work. Failed launch, exit, signal, collection, cancellation, or guard
stops execution without automatic fixes, commits, stashes, retries, or rollback.
Preparation and check effects remain visible; retry runs the whole effective list.
[Finish check tests](../tests/finish_checks.rs) cover this boundary and live output.

### Publication pins each destination separately

[`finish::publication`](../src/finish/publication.rs) consumes a ready prepared
plan whose effective update is none. After checks it revalidates local context
and rereads each exact destination in configuration order. Equality with the
pinned result confirms unchanged publication without a push. Any other departure
from the approved observation rejects that destination; unreadable/ambiguous
advertisement stays unconfirmed. Independent destinations may proceed, while
a local context change stops later work.

Each push uses a literal result-OID-to-head refspec and explicit
`--force-with-lease=<ref>:<expected>` with `--no-force`. The expected value is empty
for creation and the original observed full OID otherwise. Ordinary updates also
require ancestry; replacement requires verified rewrite proof and allow. No `+`
refspec or blanket force is used. A rejected lease is never renewed and retried
within the operation. [Git's explicit lease semantics](https://git-scm.com/docs/git-push)
provide the ref guard; a lease establishes no incorporation of earlier remote work.

A command-scoped remote selects one explicit push URL and copies transport options
while excluding fetch/push refspec and mirror expansion. No remote is written to
configuration. Explicit push URL selection prevents another `pushInsteadOf`
expansion. Tag following, automatic tracking setup, and submodule publication are
contained as in save; hooks/helpers remain effective. A pre-push hook sees the
command-scoped remote name and actual URL. These transports do not update the
configured remote's tracking caches.

Retries confirm completed destinations through fresh equality and retain original
expectations for pending replacements. The rebase record remains intact; no final
publication journal write is necessary after a killed process or lost response.
Publication pins the prepared base and does not reread it or prepare again.
An advancing base requires fresh assessment/preparation when that newer base is
wanted. [Finish publication tests](../tests/finish_publication.rs) exercise leases,
multiple endpoints, partial success, lost acknowledgements, and cross-process retry.

## Publication reports and failure diagnosis

Save and review finish share the [exact-ref porcelain decoder](../src/publication.rs).
It requires the intended OID/refspec and a relevant ref response per destination
block. Normal hook progress on stdout is retained without erasing acknowledgements.
Display URLs remain as Git emitted them; normalized or redacted text is not guessed
back into a configured URL index.

| Aggregate state | Meaning |
| --- | --- |
| Not attempted | Local guards or unresolved steps prevented publication. |
| Complete | Every destination confirmed creation, advancement, replacement where permitted, or equality of the intended ref. |
| Partial | Some destinations confirmed publication and others did not. |
| Rejected | Every destination explicitly rejected the update or, for finish, its approved observation changed. |
| Unconfirmed | None confirmed publication and at least one response is uncertain. |

Missing/malformed replies, interrupted transport, I/O errors, and remote failure
remain uncertain, even after a successful exit. A valid acknowledgement can
confirm an effect despite a subsequent nonzero exit; the error is still retained.
Caches and process status alone prove neither success nor rejection. Complete
publication does not imply complete tracking or an error-free command.

Diagnosis starts from command output and remaining observations, rather than
rerunning an assumed original transaction. Resolve incomplete tracking explicitly;
inspect uncertain destinations before retry; preserve completed commits, stashes,
fetches, and remote effects. Review rebase/merge conflicts use ordinary Git
continuation or abort followed by fresh finish assessment. There is no
finish-specific continuation or abort command.

Multi-destination publication is sequential and non-atomic. Confirmation describes
the response or observation at that time; another writer can subsequently move
the ref. Exact OID leases cannot detect movement away and back to the same OID.
Hooks, filters, servers, and helpers are trusted Git components whose arbitrary
effects cannot be prevented or exhaustively observed. No completed effect is
automatically rolled back.

## Direct integration library

Direct integration uses the shared Git observation/ancestry primitives and check
runner, with its own plans and private recovery evidence. It targets the exact
configured base head at `base-remote`, rather than review's work-branch publication
destination. Updating the work branch and integrating into the target are
independent decisions. The target is shared history, so working-branch rewrite
permission cannot broaden allowed target effects.

### Planning stages

[`integration::assess`](../src/finish/integration.rs) binds exact source/local
target versions, the target fetch endpoint and ordered push endpoints, and fresh
evidence supplied by its caller. Published-source observations are also required
for remote cleanup planning. Assessment performs no transport or writes, and
remote-tracking caches supply no authority.

Fresh target observations must agree at fetch and every push endpoint. A missing
target is resolved outside this model. The local target may be absent, equal,
or behind by proven ancestry; unrelated local target work, symbolic refs, and
occupation in another worktree block integration.

An already-incorporated source needs no update, integration method, or message.
A source containing the target needs no preparation. Divergence resolves the
independent none/merge/rebase update policy. A preparation stage permits only
`UpdateWork`; it must reassess the actual graph before target integration.
Fast-forward-only requires target ancestry and never falls back to another method.
Merge plans a two-parent non-fast-forward commit; squash plans a single-parent
commit on the target. A new commit requires literal message input or automatic
message policy; fast-forward and already-integrated work need none. Cleanup ask
is independent, and keeping the work branch checked out conflicts with local
deletion.

Required input or any blocker empties `permitted_effects()`. Fresh resolved plans
permit only preparation or local integration. A corroborated result is required
before publication effects, and confirmed delivery at every target endpoint is
required before completion effects. These latter effects are assessed plans;
there is no corresponding transport or cleanup executor.

| Recovery state | Current assessment |
| --- | --- |
| Fresh | No selected attempt, or verified intent with no Git effect yet. |
| Incomplete operation | Active Git machinery or unfinished recorded index/integration work prevents execution. |
| Prepared result | A verified local result requires fresh destination evidence. |
| Pending publication | Some exact target endpoints remain at the original target. |
| Published | Every target endpoint confirms the result; only selected completion remains. |
| Complete | Delivery and selected completion effects are observed complete. |
| Changed or unverifiable | Context/history/evidence cannot establish this attempt. |

During recovery, target endpoints may be at the original target or exact result;
other movement blocks the attempt. The fetch endpoint may lag at the original
target. Planning pins checkout and local/remote deletion to expected versions.
Remote cleanup requires proof that the published work version is contained in
the pinned source; rewritten source history cannot inherit that proof from an
old observation. New work, another worktree's occupation, uncertain publication,
or changed destinations block destructive completion. An absent branch counts as
completed cleanup only within verified attempt evidence.

### Local execution and retention

[`integration::execution::prepare`](../src/finish/integration/execution.rs)
revalidates the approved plan and rereads the exact target at its fetch/push
endpoints before history changes. The caller supplies fetched objects and initial
evidence; execution performs no fetch or remote write. It records intent and
retains the original source at `refs/oneflow/integration/<token>/original` before
changing history.

Work preparation preserves merge structure and uses the same bounded rebase
options as review. Preparation merges have Git-generated messages. The actual
prepared graph is reassessed before integration; unresolved final message input
can be returned after preparation. The backend neither prompts nor launches an
editor.

An absent/behind local target is conditionally created/advanced to the observed
target with a marked reflog transition and exact old-OID expectation. If resuming
on the checked-out target, Git fast-forwards ref, index, and tree together.
The executor checks out the target to construct the candidate; this temporary
workspace choice does not execute the configured final checkout policy.

Integration runs Git `--ff-only`, `--no-ff` with message, or squash followed by a
guarded commit. The automatic message is `Integrate <source> into <target>`.
Explicit messages are literal arguments with normal Git cleanup, hooks, and
signing. The verified result is retained at
`refs/oneflow/integration/<token>/result`. Source/target branches and retention
refs survive failed checks. Reports distinguish new integration from reuse and
retain observed changes even when Git fails afterward.

`execution::check` requires a clean verified candidate checked out at its exact
result OID. It uses the shared ordered process runner with integration-specific
guards over records, refs, settings, history, and context. Check success cannot
authorize a later publication or omission of checks. Candidate failure retains
the result and a retry runs the effective list again.

### Association and recovery proof

[`integration/recovery.rs`](../src/finish/integration/recovery.rs) stores private
version-2 records at
`<worktree-git-dir>/oneflow-integration/branches/<source>/attempt.json` and an
`active.json` selector for association after target checkout. Atomic writes sync
files and containing directories and reject symlinks; Unix paths must be private.
Records bind identity, exact source/target refs and commits, local target expectation,
endpoints, settings/provenance, Git configuration, choices, reflog anchors,
retention refs, and cleanup-relevant published source versions. Optional result
acknowledgements are corroborated evidence rather than stored success flags.
Acknowledged retention requires the matching private ref and its creation reflog;
an acknowledged result also requires the verified commit and result ref.

An attached source selects only its own worktree's branch record. An unfinished
attempt on A cannot intercept B. A target name alone does not identify an attempt:
the selector must retain a valid HEAD anchor and corroborate checkout from that
same source. Later checkout from another work branch breaks association.
Recorded rebase can select a detached source only through Git's matching
`head-name` and surviving HEAD anchor. Unassociated HEAD or a selected missing
record cannot authorize recovery.

Recovery checks anchored source preparation, target alignment, and the exact
integration transition. A preparation merge must retain original-source and
target ancestry; a preparation rebase requires its marked start, supported finish,
target incorporation, and no intervening abort/restart. Fast-forward requires
source equality and target ancestry;
merge requires exact target/source parents. Squash requires the exact target as
sole parent and an attempt-specific action marker binding source, target, token,
and method. Squash source ancestry cannot prove integration. Completed transitions
can establish results without a final acknowledgement; ordinary merge continuation
can be verified through exact parentage and anchored transition. Missing objects,
expired/changed anchors, forged commit shape, or inconsistent retention refs stop
recovery. A reset that leaves the original tip unchanged can permit retry; abort
or history-changing reset cannot establish integration.

Merge/rebase recovery uses ordinary Git continuation and abort. If a verified
merge commit remains with stale `MERGE_HEAD`, the execution report directs the
developer to `git merge --quit` to retain the commit and clear machinery.
Squash has no `MERGE_HEAD`, as documented
by [git-merge](https://git-scm.com/docs/git-merge). Its report supplies the exact
`GIT_REFLOG_ACTION` needed for a manual commit, or the tracked-tree restoration
and `SQUASH_MSG` cleanup needed to abandon the attempt. An unmarked manual squash
commit cannot borrow ancestry as proof. The [integration fixtures](../tests/finish_integration.rs)
cover method-specific parentage, worktree association, conflicts, interrupted
acknowledgements, retention, and candidate checks.

## Evidence and maintenance limits

The tests exercise isolated real Git repositories and local transports, including
hooks, signing failures, linked worktrees, unusual byte paths, remote races, and
interrupted processes. Whole-fixture snapshot tests compare file content,
permissions, timestamps, and directory entries around assessment/revalidation,
including metadata; [snapshot support](../tests/support/snapshot.rs) implements
those checks. Terminal fixtures exercise complete-line input and live output.
These tests establish the covered behavior; they do not establish hosted-provider
authentication compatibility or universal Git-version/platform support. The
documented development path is Linux, with recovery fixtures grounded in Git
2.47.3. Git's installed command semantics and reflog conventions remain relevant
compatibility boundaries.

Extensions depend on preserving read-only assessment, exact ref/endpoint binding,
revalidation at effect boundaries, and partial-completion reporting. Shared
settings, tracking, inspection, and check-process changes can affect more than
one command. Workflow-specific plans preserve those shared primitives while
keeping review delivery and target integration separate.
