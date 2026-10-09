# Save contract

The [user guide](USAGE.md#save-progress) covers invocation, settings, and recovery.
This document records the implementation boundaries and invariants needed when
changing save or designing adjacent lifecycle commands.

| Boundary | Entry point and responsibility |
| --- | --- |
| Planning | `save::assess(directory, &Request)` and `save::revalidate`: read-only approval |
| Local execution | `save::execution::execute(directory, &Plan)`: preserve work and compose publication |
| Publication | `save::publication::publish(directory, &Plan)`: publish a resolved plan with no pending local steps |
| CLI | `src/save_cli.rs`: resolve input and present typed reports |

`settings::load` is shared with start. The layered, multi-format configuration
contract is specified in the [start contract](START.md#repository-settings).
`save-action` defaults to `ask`; invalid settings in either layer fail even with
an explicit action. Plans retain settings sources and distinguish action selection
by explicit selection (flag or interactive choice), resolved setting, or the
existing clean-tree publication rule.
Revalidation reloads both layers and compares resolved settings and their sources
before affected operations. Local preservation can itself change shared settings
(for example, stashing a changed configuration); changed settings stop subsequent
operations while preserving and reporting completed effects.

## Assessment and effects

`save::assess(directory, &Request)` returns a `Plan` or a typed `SaveError`,
without writes or remote contact. `Request.action` overrides `save-action`;
`Request.message` applies only to a staged selection. On a clean tree even a
staged selection needs no message. A supplied message is retained verbatim;
a dirty staged selection rejects Unicode-whitespace-only messages.

`Scope` lists staged, remaining tracked and nonignored untracked paths as bytes
relative to the repository root. Staged and remaining tracked lists can overlap:
partially staged files retain both sides of the selection. Ignored untracked
files are excluded. `tree_state()` distinguishes clean, staged-only,
unstaged-only (including untracked-only), and mixed trees.

A plan explicitly contains its attached branch/commit, repository context,
settings, scope, applicable actions, required input, ordered local steps and
publication destination. The action semantics are:

| Selected action | Planned local steps | Publication / locally retained work |
| --- | --- | --- |
| Clean tree, any preference/action | None; effective action is publish-only. | Publish existing commits; no empty checkpoint or message request. |
| Ask on dirty tree | None; action input required. | No effects permitted until a fresh selected plan is obtained and approved. |
| Publish-only | None. | Publish existing commits; all uncommitted work remains local and unchanged. |
| Staged | Commit exactly the current index with the developer message. | Publish the resulting branch; remaining tracked and untracked work stays untouched. Missing message requires input; no steps/publication are permitted yet. No staged changes is an error. |
| WIP | Commit the index using `[WIP - Staged]` when present, then stage/commit remaining work using `[WIP - Unstaged]` when present. | Publish the resulting branch; ignored untracked work is excluded. Original index state is not retained afterward. |
| Stash | Preserve tracked and nonignored untracked work in a local stash. | Publish existing branch commits. The stash is local preservation, never branch publication, and is left stashed. |

Local steps always precede publication. `publication = None` and empty steps
mean unresolved input permits no effects. Publication names only the configured
publication remote and exact `refs/heads/<current branch>`; absent upstream is
marked for
establishment **after confirmed successful publication**. The plan does not
claim server availability, fast-forward eligibility, or successful publication.
The publication backend executes resolved plans without pending local steps and
reports its outcomes. The local executor implements preservation/composition;
interaction uses these plans through the shared CLI. WIP grouping represents
index/remaining scope, not readiness.

## Preconditions and local evidence

Assessment requires a working repository and attached branch with a commit,
and refuses the exact configured base, unresolved conflicts, dirty submodule
contents and all observed operation families: merge, rebase, cherry-pick,
revert, am and bisect (including sequencer state). Clean submodule commit-pointer
changes can be included; parent commits cannot preserve dirty child contents.

All effective `branch.<name>.remote` and `.merge` values are read with NUL-delimited
`git config --get-all`. Both absent is acceptable. Otherwise **one remote value
and one exact full merge ref** must match either:

- The publication remote and current branch.
- The base remote and configured base branch.

Even repeated equal values are rejected as ambiguous. Empty values, missing
halves, local `.` upstreams and unrelated branches/remotes fail. Tracking ref
existence, fetch refmaps and locally cached divergence are not prerequisites.
Start and save share this relationship validation. Save bypasses status's tracking
resolution so malformed upstream configuration can be diagnosed before Git
tries to resolve it.

The configured publication role selects gof's destination. Explicit Git push
selection is checked using Git's precedence: the last
`branch.<name>.pushRemote` value overrides the last `remote.pushDefault` value.
The effective explicit value must equal the publication remote name; a conflicting
value (including a URL or another remote name) fails before checkpoints or push.
A masked lower-precedence value does not conflict. Both raw value lists are
snapshotted, so even a masked change requires reassessment. Ordinary upstream
fallbacks, `push.default`, and expansive refspecs do not broaden the tool's
explicit exact-branch publication, and no user routing configuration is rewritten.

This expands the earlier publication-only tracking guard to accept project-base
tracking. Existing valid tracking is retained through publication. It also changes
compatibility for explicit push overrides: conflicting overrides now fail instead
of being ignored. Start validates these same relationships on local resume and
current-branch success, while preserving offline operation.

`git remote get-url --push --all <name>` validates the named remote locally and
captures all resolved push destinations, including ordinary URL rewriting.
The publication remote may be inaccessible at assessment. Save does not resolve
or contact the base remote; no fetch, server query, trunk update or trunk
integration is performed. Later publication must handle multiple destinations
and partial/uncertain outcomes truthfully.

Git documents upstream configuration and multi-valued merge behavior in
[git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-branchltnamegtmerge),
and push-URL expansion/selection in
[git-remote](https://git-scm.com/docs/git-remote#Documentation/git-remote.txt-get-url).
Git documents push-remote precedence in
[branch.pushRemote](https://git-scm.com/docs/git-config#Documentation/git-config.txt-branchltnamegtpushRemote)
and [remote.pushDefault](https://git-scm.com/docs/git-config#Documentation/git-config.txt-remotepushDefault).
Isolated ordinary Git push dry runs verify precedence, including the last-value
behavior of repeated scalar settings. Requiring an explicit override to name
the publication role and rejecting duplicate upstream values are our accepted
product policies, not a claim that Git prohibits them or that this workflow
is universal.

## Revalidation and limits

`save::revalidate(directory, &approved_plan)` repeats assessment and compares
context, HEAD, settings, raw upstream and explicit push-selection values, resolved
push URLs, scope, index path/bytes, changed-file bytes/permissions or symlink targets, and local tracked diff
content (including changed gitlink revisions). It catches changed content even
when the same paths/categories remain dirty. Untracked directory observations
are inspected recursively without following symlinks. Snapshot content remains
in memory; it is not a persisted plan or lock. Index refreshes by another process
can conservatively invalidate approval even if the selected content is unchanged.
A new failed safety guard is returned directly; other differences report changed
context. Callers must obtain and approve a new plan after changed context and
revalidate before execution; later executors need checks between their own steps.

The Git inspection executor disables optional locks, lazy fetching and fsmonitor.
Diff capture additionally disables external diff/textconv, index auto-refresh,
relative path output and submodule ignore settings. A real-Git regression proved
that ordinary porcelain diff can refresh a stale index even with optional locks
disabled; this is why `diff.autoRefreshIndex=false` is explicit. The behavior is
also documented in
[git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-diffautoRefreshIndex).

These observations are several local reads. They neither guarantee a consistent
snapshot during concurrent edits nor atomically protect a later Git mutation.
They follow existing Git observation semantics and do not assert preservation of
content Git excludes from the scope. Normal Git configuration remains in effect
apart from the documented inspection safeguards. Content snapshots can consume
memory proportional to included work; streaming fingerprints can be considered
later if concrete large-worktree requirements justify them.

## Local actions

Clean plans have no local steps, regardless of the selected preference. Dirty
publish-only plans also leave the index, working tree and stash untouched.
Both publish existing branch commits directly.

A staged action runs ordinary `git commit --message <developer message>` with
no path operands or automatic staging. It records the index selection, including
partially staged files, and leaves remaining tracked and untracked work local.
Missing input and blank messages cannot reach mutation. Git's normal message
cleanup, hooks and signing configuration apply.

WIP executes the planned groups in order: commit the index with
`[WIP - Staged]`, then stage the approved remaining paths and commit with
`[WIP - Unstaged]`. Only groups with content create commits. Staging uses explicit
root-relative byte paths with `--literal-pathspecs add --all --`; deletions,
leading hyphens and pathspec-looking filenames retain their literal meaning.
Ignored untracked files remain excluded. A staged deletion followed by a
recreated file can move the remaining work into the untracked category; the
executor preserves that legitimate transition.

Stash uses ordinary `git stash push --include-untracked --message
"git-oneflow save"`. The resulting stash OID is reported as `Effect::LocalStash`,
which represents local preservation and is never branch publication. The stash
is retained; no apply, pop, drop or restoration is performed. Successful execution
requires observed clearance of included work and retention of the stash history.
Identical contents/messages/timestamps can produce an existing stash OID; a
real-Git fixture verified Git can retain the existing entry in that case. Either
one new entry or that identical retained entry is accepted, preserving older ones.

## Guards and transitions

Before mutation, revalidate the original approval against HEAD, repository,
settings, raw upstream and explicit push-selection values, resolved push destinations,
index path/bytes and approved change scope/content. Unresolved input, failed eligibility, changed approval,
known index locks and identity failures stop local execution. Publication's
read-only destination/tracking/configuration guards are shared and run before
checkpoint writes, then repeated before later mutations/publication.

After a commit, verify its single parent is the preceding approved commit and
its raw tree change equals the preceding index selection. Verify the resulting
index matches that commit, remaining tracked diff (including gitlink revisions)
is unchanged, approved file contents survive, and no new changed paths appear.
After staging, verify unchanged HEAD/context/files/gitlink revisions, no remaining
included work, and no staged paths outside the approved remaining scope. Expected
index refreshes and category changes are permitted; changed work is not silently
approved. Hooks that alter the selection or remaining work stop execution while
retaining their effects.

Each successor is reassessed with an explicit publish-only request. After all
required local steps succeed, revalidate the final successor and pass its exact
branch/commit to the existing publisher. No publisher, transport parser or broad
workflow abstraction is duplicated. There is no history rewrite, forced push,
hook bypass, rollback, stash restoration or WIP cleanup.

## Outcomes and retry

`Report` separates verified local `effects`, per-command operation/arguments/raw
output or I/O failure, observations after each command, final remaining state,
and the independent publication report. `is_complete()` requires successful local
execution and the publisher's confirmed complete result. A publication failure
is retained in `Report.publication.error`; it does not erase local effects.

Observations use repository inspection without save eligibility, so detached
HEAD, conflicts, partial index writes and stash effects remain visible even when
a new assessment would refuse execution. They retain index bytes and stash OID
alongside actual repository state, including unavailable observations. A failed
command may have produced a commit/stash or staging changes without becoming a
verified effect; inspect its post-command/final observations and raw diagnostics.

Retry always requires fresh assessment of actual remaining state. A completed
WIP or stash followed by failed publication leaves a clean retry with no empty
duplicate checkpoints. A failed second WIP commit can leave its remaining work
staged; a fresh WIP assessment then commits that actual index with
`[WIP - Staged]`. It does not infer the earlier grouping or pretend staging was
rolled back. A staged action can intentionally leave remaining work; use an
explicit action appropriate to that newly assessed state. Ordinary Git handles
stash recovery separately.

## Exact publication and preservation

Before pushing, repeat save assessment and compare the intended branch, commit,
repository, settings, upstream, push destinations, scope, index and included file
contents. Retain the existing conflict/operation/submodule/base guards. Execute
from the verified worktree root so disappearance of a nested caller directory
does not prevent reporting. Dirty publish-only work and an existing index lock
are permitted; the publisher neither stages, commits, stashes nor edits the index
or working tree.

The push uses the configured **remote name**, a literal
`<approved commit OID>:refs/heads/<exact branch>` refspec, and porcelain output.
Using the OID pins the intended commit even if a pre-push hook moves the local
branch. Hooks see Git's ordinary explicit-OID refspec input, rather than an
implicit branch-name source. No branch-name revision expansion is involved,
including for `@`, Unicode normalization variants, or names resembling refs.

Git's ordinary non-forced branch rules decide whether to create, fast-forward,
report equality, or reject the update against the actual destination's advertised
history. No fetch, cached divergence, fetch-URL query, repair, lease, forced
update, history rewrite, or rollback is used. The named remote retains normal
push URL selection/rewrite rules and all push destinations, including repeated
URLs and the fallback to multiple fetch URLs when push URLs are absent.

The explicit refspec excludes configured push mappings and defaults. A
command-scoped `remote.<name>.mirror=false` overrides mirror expansion;
`--no-force` and `--no-follow-tags` keep the operation within its history/ref
boundaries. `push.autoSetupRemote=false` prevents incidental upstream writes.
Recursive submodule publication is suppressed, including `on-demand`, `only`
and the implicit `submodule.recurse` behavior, because it could publish unrelated
branches. A user's non-mutating `push.recurseSubmodules=check` remains effective.
Other push settings (including signing, receive-pack, push options, transport and
credentials) and ordinary local/remote hooks remain effective. The existing
workflow executor disables lazy fetch/fsmonitor and terminal credential prompts;
credential helpers and configured authentication still apply.

Git can update fetch-mapped **local tracking refs** after a push. Isolated positive
controls demonstrated that mappings into `refs/heads/`, or symbolic tracking refs
pointing into local branches, can change local history even with a non-forced
single-branch push. Therefore positive fetch mappings with local destinations must have full
destinations under `refs/remotes/`; any other positive mapping, even an unrelated one, is
conservatively refused. Matching symbolic tracking refs are refused too. Negative mappings, source-only
fetch refspecs and restrictive remote-tracking mappings remain supported. Legacy
`$GIT_DIR/remotes`/`branches` definitions without configured URL values are refused:
config inspection cannot validate their separate tracking mappings. These are
engineering containment policies for this publisher, not claims that Git itself
prohibits those configurations. No user configuration is rewritten to repair them.

## Results and tracking

CLI scope and completion output identify the publication remote. The project
base is a separate tracking relationship and does not select save's destination.
Ordinary Git commands continue to use the developer's Git routing configuration.

`Report` keeps the exact intent, configured push URLs, per-destination transport
responses, missing response count, raw push output/status or I/O error, typed
failure, and before/after upstream and remote-tracking-ref observations. Display
URLs are retained as Git emitted them; they can be normalized or redact credentials
and are not guessed associations with configured URL indices. A missing response
is not assigned a rejection or an invented destination result.

| Publication state | Evidence |
| --- | --- |
| `NotAttempted` | A local guard failed; no push subprocess was invoked. |
| `Complete` | Every configured destination acknowledged the exact intended ref as created, advanced or unchanged. |
| `Partial` | At least one confirmed publication, without complete confirmation across all destinations. |
| `Rejected` | Every destination explicitly rejected the intended update. |
| `Unconfirmed` | No confirmed publication and at least one failed, missing or uncertain response. |

An ordinary Git rejection and a remote hook rejection are explicit rejections.
`[remote failure]`, missing/unrecognized reports, I/O errors and interrupted
transports remain uncertain. A process exit alone is insufficient confirmation.
The parser requires the exact source/destination refspec and one relevant response
per destination block; ordinary hook progress on stdout does not erase valid
acknowledgements. All expected acknowledgements can confirm publication even
when the process subsequently fails; that process failure is still retained.
Remote-tracking caches are observations of local effects, never publication proof.

An absent upstream is written only after **all** destinations confirm publication.
Both accepted tracking styles are preserved byte-for-byte. After pushing, context
is revalidated before completion or tracking setup; changed routing remains
a reported failure even when the exact branch was published. Two ordinary local
Git config writes add the exact remote and merge ref. Revalidation before each write refuses
changed context; the private second-write check admits only the publisher's own
expected remote-only intermediate state. Public assessment continues to reject
all incomplete, ambiguous or conflicting upstreams. There is no automatic cleanup
of a first write after second-write failure. The result exposes successful config
commands plus actual remaining config, including effects left by a failed command.

Publication and tracking states are independent. `Report::is_complete()` requires
confirmed publication, observed valid tracking in either supported style, and no
execution error. `TrackingState::Matching` represents either accepted relationship.
A published branch with absent/incomplete tracking remains published, with a tracking
failure; a failed command that actually finished the config writes reports observed
matching tracking plus its error. Callers must present these separate facts and
escape raw bytes appropriately. To recover, inspect uncertain push destinations,
preserve completed remote/config effects, resolve the transport/configuration
problem and assess again. A clean retry at an identical remote tip is unchanged
publication. An incomplete upstream requires explicit user repair before reassessment;
it is never silently overwritten.

## Evidence and limits

Git documents index-based commits, normal hooks and signing in
[git-commit](https://git-scm.com/docs/git-commit/2.46.1), and local index/worktree
preservation and recovery in [git-stash](https://git-scm.com/docs/git-stash/2.43.0).
The separate WIP grouping and strict upstream policy are accepted product choices.

The Git 2.45 manuals for [push](https://git-scm.com/docs/git-push/2.45.0),
[remote](https://git-scm.com/docs/git-remote/2.45.0), and
[config](https://git-scm.com/docs/git-config/2.45.0) document explicit refspecs,
history preservation, push URL selection, expansion, upstream setup, and recursion.
Local preservation tests verify contents, partial staging, stash recovery, locks,
hooks/signing, and retries. Publication tests include a fixture transport killed
after a remote ref update: missing acknowledgements remain unconfirmed even when
independent inspection proves publication occurred.

Validation uses isolated local transports on Linux with Git 2.47.3. Hosted
providers/authentication, other platforms, and the Git 2.45 runtime remain unverified.
Push acknowledgements describe an operation; another writer may subsequently move
the remote ref. Snapshots/subprocesses are not locks, and arbitrary hook/filter/helper
effects cannot be prevented or exhaustively observed.

Approval snapshots consume memory proportional to included work. Scoped staging
passes literal paths as process arguments; OS argument-size failures are local I/O
failures. These are current implementation limits, not approved expansion tasks.
