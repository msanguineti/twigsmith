# Finish contract

Finish prepares and publishes the current working branch for delivery. This
contract is implemented through read-only assessment/revalidation, controlled
remote refresh, branch preparation, and durable rebase recovery evidence.
Publication, the CLI, and additional setup questions remain future work.
Configurable checks,
provider submission, integration into the base, tagging, and cleanup are deferred.

## Configuration and choices

The flat configuration schema includes:

| Setting | Values | Default |
| --- | --- | --- |
| `finish-update` | `ask`, `rebase`, `merge`, `none` | `ask` |
| `finish-rewrite` | `ask`, `allow`, `deny` | `ask` |

Shared TOML/YAML/YML/JSON, common-directory local files, and legacy `oneflow.conf`
use the same strict validation and sparse per-key precedence. Every present value
is validated, including overridden values. Sources retain defaults or exact
shared/local paths. Explicit `finish::Request` choices override settings; their
provenance is `Request`. CLI flag grammar is deferred.

Fresh evidence that the branch includes the remote base resolves update to `none`
with source `IncludesBase`, regardless of the selected update preference. Otherwise
`ask` requires update input when the relationship is known: rebase onto the fetched
remote base, merge that base into the working branch, or `none` to retain history.
An unavailable or uncertain comparison requires better evidence before resolution.
Names/prefixes have no operational role. Clean branches with WIP commit messages
are accepted.

Selecting rebase resolves only the update choice. When published commits may be
replaced, rewrite `ask` requires separate input and `deny` prevents approval of
that prospective replacement. `allow` supplies permission while all incorporation,
destination, and lease requirements still apply. A plan can require input and
contain independent constraints at the same time.

## Local readiness

`finish::assess(directory, request, evidence)` requires:

- A Git working tree, including linked worktrees.
- An attached branch with an existing commit, different from the configured base.
- A clean index, working tree, and submodules, as observed through Git status and
  save's existing snapshot machinery. Nonignored untracked work counts as dirty;
  ignored files remain outside the scope. Changed submodule commits also count.
- No unresolved conflict or active merge, rebase, am, cherry-pick, revert, bisect,
  or relevant sequencer operation.
- A configured remote with locally resolvable push destinations.
- Either absent upstream settings or exactly one matching remote and one merge
  ref for the current branch. Incomplete, duplicate, and conflicting values fail.

Publication intent is the current branch's exact `refs/heads/<name>` on the
configured remote, never the configured base or an inferred upstream destination.
Missing tracking is permitted; establishing it is an intent for after confirmed
publication. Save's read-only publication preflight is reused: ambiguous URL
representations, legacy remote files, unsafe fetch mappings, and symbolic mapped
tracking refs fail before approval. This keeps save's existing policy intact.

Local guard failures return typed `FinishError` values. Readiness failure has no
finish effects. The model stays independent of prompts, terminal rendering,
execution, and recovery metadata writes.

## Evidence and relationships

A plan records two separate forms of observation:

- `TrackingHints` reads conventional `refs/remotes/<remote>/<base|branch>` locally.
  These may be stale; custom fetch mappings may use other refs. They never
  establish remote availability, incorporation, or publication readiness.
- `RemoteEvidence` is operation-scoped fresh evidence acquired by
  `preparation::refresh`. It identifies the configured remote, exact base and working-branch refs, the
  fetched base commit, and an exact branch observation for every resolved push URL
  in configuration order. Push destinations can differ from the fetch URL and
  from each other. Repeated push URLs remain supported; their observations must
  agree, or the destination is ambiguous. Fetching only the base is insufficient
  publication evidence.

Assessment without fresh evidence returns `FreshRemoteEvidenceRequired`, with no
history-dependent prompts. Evidence for different refs, remote, destination order,
or destination set fails with `InvalidEvidence`. Commit observations accept full
object IDs of the repository's hash length, never revision expressions. Absent,
unavailable, and ambiguous observations remain distinct.

The caller is responsible for the freshness and truth of supplied observations.
The assessor validates their binding to current selections and traverses only
locally available history. It cannot independently authenticate that a fetch or
remote observation occurred. Missing objects cause actionable unavailable states;
inspection never lazily fetches them.

Relationships describe the working branch relative to the observed commit:

| Relationship | Meaning |
| --- | --- |
| `Equal` | Same commit |
| `Ahead` | Observed remote commit is an ancestor of the working branch |
| `Behind` | Working branch is an ancestor of the remote commit, with complete history |
| `Diverged` | Neither contains the other; a common ancestor exists |
| `Unrelated` | Complete traversable histories have no common ancestor |
| `Uncertain` | Shallow boundaries prevent a reliable negative ancestry conclusion |
| `Unavailable` | Observation, commit object, or traversal is unavailable |
| `Absent` / `Ambiguous` | Remote ref is absent / destination evidence is ambiguous |

Equality and proven positive incorporation paths remain useful in shallow
repositories. Negative incorporation conclusions are conservatively uncertain
whenever shallow boundaries exist, even if unrelated repository history caused
the shallowness. Traversal errors are unavailable, never guesses. Ancestry ignores
replacement objects; legacy grafts constrain approval because they can invent
parentage. These choices are engineering safeguards, not claims about workflow
prevalence.

Git documents the ancestry predicate and its distinct error exit statuses in
[git-merge-base](https://git-scm.com/docs/git-merge-base), and describes shallow
commits as artificial roots in [shallow](https://git-scm.com/docs/shallow).
These capabilities support the relationship model; our conservative handling of
negative shallow results is product policy.

## Preparation and publication constraints

Each destination retains its observed ref, relationship, and prospective mode:

| Mode | Meaning |
| --- | --- |
| `Create` | Destination branch observed absent |
| `FastForwardOrUnchanged` | Published history is incorporated and the resolved preparation preserves it |
| `RecheckAfterRebase` | Incorporated published commits may be replaced; final history must be reassessed |
| `VerifiedRewrite` | Verified finish recovery proves incorporation before rewriting the exact observed remote commit; resolved permission and a publication lease are still required |
| `Blocked` | Remote work is missing locally, history is uncertain/unavailable, or destination evidence is ambiguous |

These modes are prospective. Required input or any constraint prevents overall
approval even if individual destinations have usable modes. Unresolved update
input must be resolved through a new assessment before acting on destination modes.
`Plan::is_ready()` means no unresolved input or constraint; it does not authorize
a push command or claim that preparation, checks, or publication occurred.

A published commit included in both the working branch and new base survives
preparation without separate rewrite input. Other incorporated published commits
may require replacement after rebase: the planner conservatively resolves rewrite
permission before approval and records `RecheckAfterRebase`. Preparation
assesses the actual prepared graph to distinguish ordinary publication from
replacement. Merge and `none` preserve incorporated published history.

A destination commit behind which the working branch trails, diverges, or has
unrelated history produces `RemoteWorkNotIncorporated`. `allow` never clears it.
Uncertain/unavailable history and ambiguous destinations have separate constraints.
Unrelated base history prevents rebase/merge approval; explicit `none` retains
history when complete observations establish that relationship. An absent or
unavailable remote base prevents approval.

Preparation preserves merge structure during rebase, limits tool-initiated branch
changes to the working branch, and reassesses its result. Replacement requires
both resolved authorization and proof that the originally observed remote work
was incorporated before rewriting. Verified durable recovery evidence can supply
that proof; assessment reads it without mutations.
Future publication must use a lease for the exact observed commit at the exact
destination. Git's [git-push documentation](https://git-scm.com/docs/git-push)
explains explicit lease expectations. A lease protects against later destination
changes; it establishes no incorporation of earlier remote work. Creation must
likewise protect the observed absence in the future publication implementation.

The accepted delivery direction is publication to the configured remote by
default, followed by a clear handoff to the developer for any submission step.
Provider submission is opt-in through configuration that explicitly selects a
supported CLI. Do not require or automatically invoke `gh`, infer a provider from
the remote URL, or ask about provider integration during ordinary publication.
Keep integrations to a small curated set; the supported set and selection settings
remain undecided. Do not add an arbitrary executable/plugin mechanism. If guided
setup later offers provider submission, manual submission is the default. These
are design constraints for future work; no provider adapter or setup flow is
implemented by the preparation library.

Publication results must distinguish confirmation per actual destination,
rejection, unconfirmed responses, partial effects, and tracking establishment.
After confirmed
publication, report branch/destination/commit and hand off the project's manual
submission step unless an explicitly configured future provider adapter handles
it.
These are obligations for the future publication executor.

## Controlled refresh and preparation

`finish::preparation::refresh(directory, plan)` accepts a locally assessed plan,
including one awaiting fresh evidence or input. It repeats readiness and
destination guards before transport commands. It observes the exact base head
through the configured remote and the exact working-branch head at every resolved
push URL, preserving configuration order. A push URL that would resolve to another
fetch endpoint is rejected. Read access to each publication destination is
required; an unreadable endpoint is a transport failure, never confirmed absence.

An advertised commit is fetched by its full object ID. Ref changes between
advertisement and fetch cannot substitute another commit. A server refusal to
serve that object stops refresh. Fetch uses explicit source-only refspecs, an
empty refmap, and disables tags, pruning, recursive submodule fetching,
FETCH_HEAD writes, automatic maintenance, and commit-graph writes. It obtains
objects without advancing local bases, unrelated branches, tags, or tracking
refs. Existing objects and transport effects are retained on failure.
Git documents these controls in [git-fetch](https://git-scm.com/docs/git-fetch).

Refresh returns exact observations plus a new plan. Resolve outstanding choices
by assessing the chosen request against those observations.
`preparation::prepare(directory, plan)` requires a ready plan, repeats refresh,
and stops if observations or context changed. It runs from the discovered
worktree root. Rebase uses the fetched base OID, `--rebase-merges=no-rebase-cousins`,
`--no-update-refs`, `--no-fork-point`, `--no-autosquash`, and `--no-autostash`.
Merge uses that same base, committing a normal merge or fast-forward. Neither
operation recursively updates submodule worktrees. `none`, including the
already-includes-base case, runs no history update.
[git-rebase](https://git-scm.com/docs/git-rebase) documents merge recreation and
update-ref behavior. Merge recreation can require conflict resolution again;
Git's merge-preserving mode defines the topology behavior.

After an update, preparation reassesses the clean attached branch and resulting
ancestry. Changes to repository selection, branch, upstream, destinations,
effective settings or Git configuration, or committed shared configuration stop
further execution pending fresh assessment. Changes made by hooks remain visible.
Reports include transport observations, successful fetches, recovery writes,
verified branch updates, raw process results, and remaining repository state.
A failed process can have effects beyond the completed-effect list. No report
claims transactional rollback. Preparation never pushes or establishes tracking.

On a rebase conflict or interruption, retain Git state and use
`git rebase --continue` after resolving conflicts, or `git rebase --abort`.
For a merge, use `git merge --continue` or `git merge --abort`. Reassess finish
after Git recovery. The report's guidance follows the observed active operation;
when none remains, inspect the process output and resulting repository state.
There are no finish-specific continuation or abort commands.

## Durable rewrite verification

Before rebase, preparation atomically writes a versioned record beneath the
worktree's Git directory at `oneflow-finish/branches/<branch>/rewrite.json`.
Files are owner-only and directories owner-access-only on Unix; symlinked or
nonprivate recovery paths are rejected. The record contains repository/worktree
identity, exact refs and destinations, original incorporated observations,
pre-rebase commits, fetched base commits, and HEAD/branch reflog anchors. An
operation-specific reflog action identifies the finish-initiated rebase start.
No permission flag alone establishes rewrite eligibility.

Assessment corroborates records against Git. It verifies original ancestry,
each attempt's observed destinations, anchored branch transitions, the unique
start and matching successful rebase finish, incorporation of the fetched base,
and the resulting branch tip. Subsequent branch transitions must preserve the
verified result through positive ancestry; ordinary commits and merges can
extend it. An aborted or unstarted attempt supplies no
new rewrite proof. Git continuation can complete an attempt in another process;
a process dying after Git completion requires no final journal write to recover.
Further finish rebases can extend the verified chain when the base advances while
publication is pending, including after partial publication at different URLs.

`Recovery` distinguishes absent, unchanged, verified, and invalid evidence.
Only an exact destination observation covered by a verified rewrite can replace
`RemoteWorkNotIncorporated` with `VerifiedRewrite`. Current rewrite `ask` or `deny`
still requires resolution. New remote commits cannot use old observations as a
shortcut. Destinations already incorporated in the current graph use normal
ancestry assessment, including destinations where publication already succeeded.

Verification requires intact branch and HEAD reflogs. Rebase stops before changing
history if those logs do not anchor the current commit. Expired logs, missing
objects, history changes that discard the verified result, mismatched identities,
and malformed records fail closed for replacement. Ordinary incorporated history can still be
assessed independently of an invalid record. Reflog expiration and object cleanup
can therefore remove retry eligibility; records are not a backup or a general
workflow database. Records remain local and are not automatically cleaned up.

Reflog start/finish conventions are verified against real Git and the
[Git rebase implementation](https://github.com/git/git/blob/v2.47.3/builtin/rebase.c)
and [sequencer](https://github.com/git/git/blob/v2.47.3/sequencer.c). If a Git version
or manually altered operation produces unrecognized evidence, verification stops
replacement. These checks protect against stale or inconsistent local evidence;
they do not authenticate against someone able to forge both Git history/logs and
private metadata. Repository observation remains a sequence of reads, not a lock
against concurrent external mutation.

## Revalidation and read-only invariants

`finish::revalidate(directory, approved, evidence)` repeats assessment using the
original request and compares the entire plan, including caller-supplied evidence.
It returns `ContextChanged` on a changed valid context; new guard failures retain
their specific errors. Resolve choices again after invalidation.

Snapshots include repository/worktree selection, exact HEAD, index bytes, Git
status/operations, upstream values, effective settings and sources, tracking hints,
resolved destinations, effective Git configuration, committed shared configuration
blob IDs, shallow/graft metadata, remote evidence, and verified recovery state.
The full effective Git configuration is compared conservatively;
an unrelated configuration value can invalidate approval. Configuration-file
formatting with unchanged effective settings and sources does not invalidate it
unless the committed configuration blob changes.
Changes to remote evidence invalidate approval even when local refs are unchanged.

Both functions use the inspection boundary, with optional locks, lazy fetch, and
fsmonitor disabled. Save's diff snapshot also disables index auto-refresh,
external diff, and textconv. They perform no fetch, push, branch update, ref write,
configuration write, content/index mutation, hook execution, or recovery-state
write. Whole-fixture tests compare content, permissions, timestamps, and entries
across successful/failed assessment and revalidation, including Git metadata.

Observation is several reads, not an atomic snapshot or cross-process lock.
Revalidation with the same supplied evidence cannot detect remote-only changes.
Preparation repeats remote observation and revalidates before mutations. Future
publication must revalidate and use exact destination guards. Recovery records
are operational evidence with strict verification, not long-lived approval.
