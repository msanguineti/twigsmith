# Finish contract

Finish prepares and publishes the current working branch for delivery. This
contract is implemented through read-only assessment/revalidation, controlled
remote refresh, branch preparation, durable rebase recovery evidence, and exact
publication, exposed identically by `git-oneflow finish` and `gof finish`.
Structured check configuration is supported. Check execution, external integrations,
provider submission, integration into the base, tagging, and cleanup are deferred.

## Configuration and choices

Finish output redacts credentials in HTTP(S) URLs by default, including prompts,
completion output, failures, and displayed Git diagnostics. The local-only
`redact-credentials` boolean defaults to `true`; it is excluded from committed
shared configuration and generated shared files. `finish --show-credentials`
temporarily enables display regardless of the local value. Disabling redaction
exposes credentials in terminal output and captured logs, and requires no
confirmation.

The configuration schema includes:

| Setting | Values | Default |
| --- | --- | --- |
| `finish-update` | `ask`, `rebase`, `merge`, `none` | `ask` |
| `finish-rewrite` | `ask`, `allow`, `deny` | `ask` |
| `finish-checks` | Ordered list of `{ name, command }` objects | `[]` |

Shared and common-directory local TOML/YAML/YML/JSON files
use the same strict validation and sparse per-key precedence. Every present value
is validated, including overridden values. Sources retain defaults or exact
shared/local paths. Explicit `finish::Request` choices override settings; their
provenance is `Request`. The CLI accepts `finish` (alias: `f`), `finish --help` (also `-h`),
`--update <rebase|merge|none>`, and either `--allow-rewrite` or `--no-rewrite`.
Update and rewrite flags combine in either order. Rewrite flags are mutually
exclusive; duplicate options, unsupported values, and positional arguments are
syntax errors. The alias accepts identical grammar and behavior. There is no
blanket `--yes` or dry-run.

Each check contains exactly a nonblank string `name` and a nonempty string list
`command`. The first command element is a nonempty executable name or path;
remaining elements are literal arguments, including empty strings. NUL is rejected
in every command element. No shell interpretation, interpolation, splitting,
executable lookup, or command execution occurs during loading or inspection.
Shell features require an explicit shell in the command vector. Names and
arguments retain their supplied contents and list order across rendering in every
supported format.

An omitted local list inherits the shared list. A supplied local list replaces
it completely; an empty list disables inherited checks. Both layers are validated
even when overridden. The effective ordered list and its whole-list source are
part of settings equality and plan/setup revalidation. Generated defaults contain
an explicit empty list; guided entry and CLI bypass options are deferred.

Until check execution is implemented, `finish::assess` returns
`FinishChecksUnavailable` for a nonempty effective list. The CLI stops before
remote refresh, branch preparation, or publication. Library preparation and
publication revalidation also enforce this guard, including checks added after
assessment. Start, save, and configuration inspection never execute finish checks.

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
destination, and lease requirements still apply. Configured `allow` is standing
authorization for eligible replacement; an interactive `allow` authorizes the
reviewed invocation only. A plan can require input and
contain independent constraints at the same time.

## CLI interaction and outcomes

The CLI assesses local readiness, refreshes remote observations/objects, and then
presents only choices required by the resulting plan. Independent constraints
stop execution without asking choices that cannot make the plan ready. Update
`ask` offers rebase, merge, none, and cancellation with brief effects. A required
replacement with rewrite `ask` receives separate allow/deny/cancel input, including
when rebase was selected explicitly. Resolved flags/settings require no further
confirmation. If the branch includes the base, update input is irrelevant.

All required choices resolve before branch execution. Each answer is assessed
against the same evidence, with revalidation of the reviewed plan before and
after assessment. Preparation repeats remote observation before affected branch
mutations; publication rereads exact destinations and retains its lease guards.
Changed context stops execution without silently adopting the change.

Prompting requires terminal stdin and stderr, following save/config conventions.
EOF, a partial line at EOF, or `cancel` stops before branch execution. Invalid
answers fail with retry guidance. Without a suitable terminal, unresolved input
fails with the applicable flags and setting names. Cancellation and unresolved
input leave branch history, index, and working-tree content unchanged by finish;
already completed remote observations/fetches are retained and reported.

Guided `config` exposes both finish settings, defaulting to `ask`; every generated
format includes them. `config --yes` stays prompt-free and writes explicit defaults.

Exit status is `0` only for confirmed complete publication (including unchanged
publication and matching tracking), `1` for operational failure, unresolved input,
cancellation, or incomplete execution, and `2` for invalid syntax. Help works
without Git or a repository. Failures report completed effects, destination
outcomes, process diagnostics, and observed remaining state. Conflicts direct the
developer to Git recovery followed by rerunning finish; there is no automatic
rollback or finish-specific continuation/abort command.

## Local readiness

`finish::assess(directory, request, evidence)` requires:

- A Git working tree, including linked worktrees.
- An attached branch with an existing commit, different from the configured base.
- A clean index, working tree, and submodules, as observed through Git status and
  save's existing snapshot machinery. Nonignored untracked work counts as dirty;
  ignored files remain outside the scope. Changed submodule commits also count.
- No unresolved conflict or active merge, rebase, am, cherry-pick, revert, bisect,
  or relevant sequencer operation.
- A configured `base-remote` with a locally resolvable fetch endpoint and a
  configured `publish-remote` with locally resolvable push destinations. The roles
  may name the same remote or different remotes.
- Either absent upstream settings or exactly one remote and one full merge ref
  for the published counterpart or configured project base. Incomplete, duplicate,
  and unrelated values fail.
- An effective explicit Git push override that agrees with the publication role,
  using the shared [save precedence and snapshot guard](SAVE.md#preconditions-and-local-evidence).

Publication intent is the current branch's exact `refs/heads/<name>` on the
publication remote, never the configured base or an inferred upstream destination.
Missing tracking is permitted; establishing it is an intent for after confirmed
publication. Save's read-only publication preflight is reused: ambiguous URL
representations, legacy remote files, unsafe fetch mappings, and symbolic mapped
tracking refs fail before approval. This keeps save's existing policy intact.

Local guard failures return typed `FinishError` values. Readiness failure has no
finish effects. The model stays independent of prompts, terminal rendering,
execution, and recovery metadata writes.

## Evidence and relationships

A plan records two separate forms of observation:

- `TrackingHints` reads conventional `refs/remotes/<base-remote>/<base>` and
  `refs/remotes/<publish-remote>/<branch>` locally.
  These may be stale; custom fetch mappings may use other refs. They never
  establish remote availability, incorporation, or publication readiness.
- `RemoteEvidence` is operation-scoped fresh evidence acquired by
  `preparation::refresh`. It identifies the base remote and its resolved fetch
  endpoint, the publication remote, exact base and working-branch refs, the
  fetched project base commit, and
  an exact branch observation for every resolved publication push URL
  in configuration order. Push destinations can differ from the fetch URL and
  from each other. Repeated push URLs remain supported; their observations must
  agree, or the destination is ambiguous. Fetching only the base is insufficient
  publication evidence.

Assessment without fresh evidence returns `FreshRemoteEvidenceRequired`, with no
history-dependent prompts. Evidence for different refs, either remote role, base
endpoint, destination order, or destination set fails with `InvalidEvidence`.
Commit observations accept full
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
Publication uses a lease for the exact observed commit at the exact destination.
A lease protects against later destination changes; it establishes no
incorporation of earlier remote work. Creation protects the observed absence.

The accepted delivery direction is publication to the configured publication remote by
default, followed by a clear handoff to the developer for any submission step.
Provider submission is opt-in through configuration that explicitly selects a
supported CLI. Do not require or automatically invoke `gh`, infer a provider from
the remote URL, or ask about provider integration during ordinary publication.
Keep integrations to a small curated set; the supported set and selection settings
remain undecided. Do not add an arbitrary executable/plugin mechanism. If guided
setup later offers provider submission, manual submission is the default. These
are design constraints for future work; no provider adapter or setup flow is
implemented by the preparation library.

Publication reports identify the branch, pinned commit, and outcome at each
actual destination. The CLI follows confirmed publication with a concise manual
submission next step naming both `<publish-remote>/<working-branch>` and
`<base-remote>/<base>` as the review target. Recovery output identifies publication
and the project base separately. It creates no pull request, runs no validation
checks, and performs no base integration.

## Controlled refresh and preparation

`finish::preparation::refresh(directory, plan)` accepts a locally assessed plan,
including one awaiting fresh evidence or input. It repeats readiness and
destination guards before transport commands. It observes the exact base head
through `base-remote` and the exact working-branch head at every resolved
`publish-remote` push URL, preserving configuration order. The base endpoint is
the first resolved fetch URL, following [Git fetch selection](https://git-scm.com/docs/git-fetch#REMOTES);
[`git remote get-url`](https://git-scm.com/docs/git-remote) resolves URL rewrites
locally for plan binding. Fetch uses the named base remote with its transport
settings. A stale, divergent, or unavailable fork base does not supply preparation
history. The required project base must be available even for update `none`;
there is no fallback to a publication remote. A push URL that would resolve to another
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

## Exact publication and retry

`finish::publication::publish(directory, plan)` accepts a ready, prepared plan:
its resolved update is `none`, with no outstanding choices or constraints. Use
the resulting plan from preparation, or assess already prepared history with
fresh evidence. Publication revalidates repository/worktree selection, the clean
attached branch and pinned commit, settings and Git configuration, upstream,
destinations, history, and rewrite proof before transport operations. Local
tracking hints may change during publication; they never supply a lease or
confirmation. Publication preserves the prepared branch and working tree.

Each configured push destination is handled independently in configuration order.
The executor rereads that destination's exact branch. Equality with the prepared
commit confirms `Unchanged` without a push, including on retries and repeated
URLs. Any other difference from the approved observation stops that destination
with `Rejected`; the executor keeps its original expectation. An unreadable or
ambiguous advertisement is `Unconfirmed`, never confirmed absence. Independent
destinations can still proceed. A local context change stops subsequent work.

A push supplies one literal `<prepared-oid>:refs/heads/<current-branch>` refspec,
`--no-force`, and `--force-with-lease=<exact-ref>:<expected>`. The expectation is
empty for creation and the originally observed full commit ID for an existing
branch. Ordinary updates require proven ancestry, so they create or fast-forward
without replacing history. Replacement requires `VerifiedRewrite` and resolved
`allow`. An explicit lease protects even an absence observed before a competing
creation; a background fetch cannot renew it. Neither `--force` nor a `+` refspec
is used. A rejected lease is never refreshed and retried within publication.
Git documents explicit and empty lease expectations and the limitations of
tracking-based leases in [git-push](https://git-scm.com/docs/git-push).

Transport selection uses a temporary, command-scoped remote with one explicit
push URL. The URL must still resolve to the observed endpoint. This supports
multiple destinations and fetch/push differences while preventing a second
`pushInsteadOf` expansion. Git documents that explicit push URLs bypass
`pushInsteadOf` in [git-config](https://git-scm.com/docs/git-config).
Configured remote transport options, such as receive-pack and proxy settings,
are copied; push/fetch refspecs and mirror defaults are excluded. Automatic
tracking setup and tag following are disabled. Submodule publication is disabled,
while an explicit `push.recurseSubmodules=check` remains effective. Hooks and
helpers run normally; a pre-push hook receives the command-scoped remote name
and actual destination URL. No temporary remote is written to configuration.
These transports do not update the configured remote's tracking caches; a later
fetch refreshes those refs. Upstream establishment follows the configuration
contract below, independently of cache existence.

Per-destination outcomes retain expected and freshly observed values, selected
action, raw command results, response details, and confirmed effects. Exact-ref
porcelain acknowledgements distinguish `Created`, `Advanced`, `Rewritten`,
`Unchanged`, and `Rejected`. Missing, malformed, unexpected, or interrupted
responses remain `Unconfirmed`, even after exit status zero. A nonzero process
status retains an otherwise valid acknowledgement and its process error. The
aggregate publication state is:

| State | Evidence |
| --- | --- |
| `NotAttempted` | Local guards or unresolved preparation prevented destination work |
| `Complete` | Every destination confirmed the pinned commit |
| `Partial` | Some destinations confirmed it and others did not |
| `Rejected` | Every destination rejected the intended update or changed observation |
| `Unconfirmed` | No destination confirmed it, and at least one outcome remains uncertain |

`Complete` describes publication evidence; `Report::is_complete()` additionally
requires valid upstream settings in either accepted style and no reported errors.
Only after all destinations confirm publication does missing tracking use save's existing
contract: revalidate and add the publication remote and exact branch merge ref. Existing project-base
tracking remains unchanged.
Tracking writes, final settings, and errors are reported separately. A tracking
failure retains confirmed publication; partial configuration writes remain for
Git recovery. Conflicting or incomplete tracking must be resolved before a fresh
finish assessment. Local remote-tracking caches prove neither publication nor
failure at an endpoint.

On retry, refresh and assess actual destinations with the current choices. An
unchanged local context also permits retrying the prepared plan; publication
still rereads every destination and keeps its original lease expectations.
Already completed destinations need no replacement. Pending replacements must
still match verified incorporation evidence and current rewrite authorization.
New remote work cannot borrow an older rewrite's permission or proof. Further
preparation, if required by an advancing base at the same bound source, uses
the existing verified rebase chain.

Publication leaves the durable rebase record intact. No publication journal or
final write is needed to recover from a process dying after a successful push:
actual remote equality confirms completion, and the original record verifies
outstanding rewrites. Lost acknowledgements therefore retain uncertain reports
until a retry observes the endpoint. In-memory reports are not permission or a
substitute for remote observation.

Cross-destination publication is non-atomic. Confirmation describes the response
or observation at that time, not a lock against later remote changes. Leases do
not detect a ref that moved away and then returned to the same OID. Servers,
helpers, and hooks remain trusted parts of Git execution, and hooks can change
state between revalidation and a command; the literal OID still pins publication.
Read access to every destination remains necessary; replacement also requires
intact recovery evidence.
Publication does not reread the base or prepare again: the plan pins the base
already selected during preparation. Reassess and prepare when a newer base is
wanted. No successful destination or local hook effect is rolled back.

## Durable rewrite verification

Before rebase, preparation atomically writes a versioned record beneath the
worktree's Git directory at `oneflow-finish/branches/<branch>/rewrite.json`.
Files are owner-only and directories owner-access-only on Unix; symlinked or
nonprivate recovery paths are rejected. The record contains repository/worktree
identity, exact refs and destinations, original incorporated observations,
pre-rebase commits, fetched base commits, and HEAD/branch reflog anchors. An
operation-specific reflog action identifies the finish-initiated rebase start.
No permission flag alone establishes rewrite eligibility.

Records use version 2. In addition to the publication remote, exact refs,
and ordered destination observations, they bind the base remote and resolved
fetch endpoint. They snapshot both selected remotes' `remote.<name>.*` entries
and all `url.*` rewrite entries, preserving configuration order and duplicates.
A changed role, endpoint, or recorded configuration invalidates this proof.
Other effective configuration and settings remain part of plan revalidation;
update/rewrite choices are resolved anew on retry and are not stored permission.

Unsupported versions, missing bindings, and malformed records cannot authorize
replacement; ordinary ancestry is assessed independently.

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
Preparation repeats remote observation and revalidates before mutations.
Publication revalidates local context, rereads destinations, and uses exact
leases; tracking hints alone may change without invalidating publication.
Recovery records
are operational evidence with strict verification, not long-lived approval.
