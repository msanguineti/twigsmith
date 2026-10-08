# Finish contract

Finish prepares and publishes the current working branch for delivery. This
contract is currently implemented through read-only assessment/revalidation.
Execution, fetches, branch updates, publication, durable retry evidence, the CLI,
and additional setup questions remain future work. Configurable checks,
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
execution, and recovery metadata.

## Evidence and relationships

A plan records two separate forms of observation:

- `TrackingHints` reads conventional `refs/remotes/<remote>/<base|branch>` locally.
  These may be stale; custom fetch mappings may use other refs. They never
  establish remote availability, incorporation, or publication readiness.
- `RemoteEvidence` is operation-scoped fresh evidence supplied by a future executor.
  It identifies the configured remote, exact base and working-branch refs, the
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
| `Blocked` | Remote work is missing locally, history is uncertain/unavailable, or destination evidence is ambiguous |

These modes are prospective. Required input or any constraint prevents overall
approval even if individual destinations have usable modes. Unresolved update
input must be resolved through a new assessment before acting on destination modes.
`Plan::is_ready()` means no unresolved input or constraint; it does not authorize
a push command or claim that preparation, checks, or publication occurred.

A published commit included in both the working branch and new base survives
preparation without separate rewrite input. Other incorporated published commits
may require replacement after rebase: the planner conservatively resolves rewrite
permission before approval and records `RecheckAfterRebase`. The future executor
must assess the actual prepared graph to distinguish ordinary publication from
replacement. Merge and `none` preserve incorporated published history.

A destination commit behind which the working branch trails, diverges, or has
unrelated history produces `RemoteWorkNotIncorporated`. `allow` never clears it.
Uncertain/unavailable history and ambiguous destinations have separate constraints.
Unrelated base history prevents rebase/merge approval; explicit `none` retains
history when complete observations establish that relationship. An absent or
unavailable remote base prevents approval.

Later execution must preserve merge structure during rebase, limit tool-initiated
branch changes to the working branch, and reassess after preparation. Replacement
requires both resolved authorization and proof that the originally observed remote
work was incorporated before rewriting. Verified durable recovery evidence may
supply that proof in future execution; the current planner reads or writes none.
Replacement must use a lease for the exact observed commit at the exact
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
implemented by the read-only planning phase.

Publication results must distinguish confirmation per actual destination,
rejection, unconfirmed responses, partial effects, and tracking establishment.
Preparation can leave conflicts or interrupted Git operations for the developer
to continue/abort with Git. No automatic rollback is implied. After confirmed
publication, report branch/destination/commit and hand off the project's manual
submission step unless an explicitly configured future provider adapter handles
it.
These are future executor obligations, not current result types or effects.

## Revalidation and read-only invariants

`finish::revalidate(directory, approved, evidence)` repeats assessment using the
original request and compares the entire plan, including caller-supplied evidence.
It returns `ContextChanged` on a changed valid context; new guard failures retain
their specific errors. Resolve choices again after invalidation.

Snapshots include repository/worktree selection, exact HEAD, index bytes, Git
status/operations, upstream values, effective settings and sources, tracking hints,
resolved destinations, effective Git configuration, shallow/graft metadata, and
remote evidence. The full effective Git configuration is compared conservatively;
an unrelated configuration value can invalidate approval. Configuration-file
formatting with unchanged effective settings and sources does not invalidate it.
Changes to remote evidence invalidate approval even when local refs are unchanged.

Both functions use the inspection boundary, with optional locks, lazy fetch, and
fsmonitor disabled. Save's diff snapshot also disables index auto-refresh,
external diff, and textconv. They perform no fetch, push, branch update, ref write,
configuration write, content/index mutation, hook execution, or recovery-state
write. Whole-fixture tests compare content, permissions, timestamps, and entries
across successful/failed assessment and revalidation, including Git metadata.

Observation is several reads, not an atomic snapshot or cross-process lock.
Revalidation with the same supplied evidence cannot detect remote-only changes.
Future execution must acquire fresh evidence where needed, revalidate before
mutations, and use exact publication guards. The current planning model introduces
no general workflow engine or long-lived approval/recovery store.
