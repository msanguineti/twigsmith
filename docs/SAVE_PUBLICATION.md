# Save publication backend

Task #12 implements `save::publication::publish(directory, &Plan)` independently
of interaction and local checkpoint creation. It accepts an assessed, resolved
plan with no pending local steps. Task #13's [local executor](SAVE_EXECUTION.md)
completes preservation and obtains a fresh publish-only plan for the resulting
branch/commit. It shares publication's read-only configuration guards before
checkpoint writes, so known local blockers precede preservation effects.
Task #14 will connect the public `save`/`v` command.

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
single-branch push. Therefore positive fetch mappings with local destinations must have full destinations
under `refs/remotes/`; any other positive mapping, even an unrelated one, is
conservatively refused. Matching symbolic tracking refs are refused too. Negative mappings, source-only fetch refspecs and restrictive remote-tracking
mappings remain supported. Legacy
`$GIT_DIR/remotes`/`branches` definitions without configured URL values are refused:
config inspection cannot validate their separate tracking mappings. These are
engineering containment policies for this publisher, not claims that Git itself
prohibits those configurations. No user configuration is rewritten to repair them.

## Results and tracking

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
Matching settings are preserved byte-for-byte. Two ordinary local Git config
writes add the exact remote and merge ref. Revalidation before each write refuses
changed context; the private second-write check admits only the publisher's own
expected remote-only intermediate state. Public assessment continues to reject
all incomplete, ambiguous or conflicting upstreams. There is no automatic cleanup
of a first write after second-write failure. The result exposes successful config
commands plus actual remaining config, including effects left by a failed command.

Publication and tracking states are independent. `Report::is_complete()` requires
confirmed publication, observed matching tracking, and no execution error. A
published branch with absent/incomplete tracking remains published, with a tracking
failure; a failed command that actually finished the config writes reports observed
matching tracking plus its error. Callers must present these separate facts and
escape raw bytes appropriately. To recover, inspect uncertain push destinations,
preserve completed remote/config effects, resolve the transport/configuration
problem and assess again. A clean retry at an identical remote tip is unchanged
publication. An incomplete upstream requires explicit user repair before reassessment;
it is never silently overwritten.

## Evidence and limits

The [Git 2.45 push manual](https://git-scm.com/docs/git-push/2.45.0) documents
explicit source/destination refspecs, fast-forward history preservation, mirror/tag
expansion, normal hooks, multiple push URLs, and the distinction between rejection
and remote failure in porcelain output. The
[Git 2.45 remote manual](https://git-scm.com/docs/git-remote/2.45.0) documents
push URL resolution and URL rewriting; the
[Git 2.45 config manual](https://git-scm.com/docs/git-config/2.45.0) documents
mirror, automatic upstream setup, submodule recursion and receive-pack settings.
These establish capabilities and safety boundaries, not prevalence of a workflow.
Publication uses options available at the repository's existing Git 2.45 minimum.

The 23 isolated real-Git publication tests passed on Linux with Git 2.47.3. They
cover creation/fast-forward/equality, remote-ahead/divergence rejection, strict
upstream validation/preservation, local revalidation and pending-step guards,
separate/rewritten/multiple/duplicate push destinations, expansion containment,
index/tree/history preservation, local and remote hooks, tracking lock and second
write failures (including failed-command effects), and transport interruption.
A reference-transaction hook kills only the fixture receive-pack **after** the ref
update; the publisher correctly reports unconfirmed publication while an independent
fixture observation proves the branch reached the remote. This is evidence against
transactional rollback claims. No actual project remote or external account is used.

Snapshots and Git subprocesses are not cross-process locks. Concurrent changes or
arbitrary hook/helper side effects cannot be prevented or exhaustively observed.
Push acknowledgements describe the operation, not perpetual remote state; a third
party can move the destination afterward. Tests use local Git transports; hosted
providers, network authentication, Git 2.45 runtime, CI and other platforms have
not been tested in this task.
