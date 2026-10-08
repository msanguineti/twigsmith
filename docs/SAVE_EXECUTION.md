# Local save execution

Task #13 implements `save::execution::execute(directory, &approved_plan)`.
Callers first obtain a resolved plan through [assessment](SAVE_PLANNING.md).
The executor preserves the selected work and composes the existing
[publication backend](SAVE_PUBLICATION.md). Public command parsing, interaction,
aliases and human presentation remain task #14.

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
settings, raw upstream, resolved push destinations, index path/bytes and approved
change scope/content. Unresolved input, failed eligibility, changed approval,
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

## Evidence and validation limits

Git's [commit manual](https://git-scm.com/docs/git-commit/2.46.1) documents
index-based commits, path operands that would change selection, normal hooks and
signing. Its [stash manual](https://git-scm.com/docs/git-stash/2.43.0) documents
index/working-tree preservation, inclusion of untracked files, local stash
retention and recovery with `apply --index`. These establish Git capabilities;
the separate two-commit WIP grouping is this product's accepted policy.

The isolated real-Git tests exercise all four tree states for applicable actions,
commit contents/order/messages, partial staging in one file, additions/deletions,
untracked/ignored files, literal byte paths, executable files/symlinks, linked
worktrees, alternate index paths, clean submodule revision checkpoints, stale
approval, normal hooks/signing, first/subsequent step failures, effects left by
failed commands, stash cleanup failure/recovery, older stash retention,
publication failure and retries. Only temporary local repositories/remotes are
used; the actual project is never pushed.

Snapshots and Git subprocesses are several observations rather than locks.
Concurrent edits and arbitrary hook/filter/helper effects cannot be atomically
prevented or exhaustively observed. Git's ordinary stash/commit limitations can
leave local work requiring manual attention; failed postconditions stop
publication and preserve observations. Scoped staging passes literal paths as
process arguments; OS argument-size failures are retained as local I/O failures.
Validation ran on Linux with Git 2.47.3; hosted transports, CI, other platforms and
Git 2.45 runtime remain unverified.
