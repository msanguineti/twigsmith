# Technical manual

Twigsmith is a Rust application that composes ordinary Git operations into
three workstream commands: start, save, and finish. This manual explains the
implementation and the invariants that matter when diagnosing or changing it.
The [user manual](USAGE.md) covers commands and recovery procedures; the
[development guide](DEVELOPMENT.md) covers setup, builds, and validation.

Read the architecture and Git boundary first, then follow the relevant workflow:

- [Shell completion](#shell-completion)
- [Repository state and the Git boundary](#repository-state-and-the-git-boundary)
- [Configuration, remote roles, and tracking](#configuration-remote-roles-and-tracking)
- [Start](#start-resolve-a-name-then-create-or-resume) and
  [save](#save-preserve-selected-work-then-publish)
- [Finish for review](#finish-for-review-prepare-check-and-deliver)
- [Publication reports and failure diagnosis](#publication-reports-and-failure-diagnosis)
- [Direct integration](#direct-integration) and
  [tagged release delivery](#tagged-release-delivery) and
  [hosted version preparation](#hosted-version-preparation)

## Architecture and execution model

Both executables, `twigsmith` and `tgi`, enter the same
[command dispatcher](../src/cli.rs). The lifecycle aliases `s`, `v`, and `f`
use the same command implementations. CLI modules parse arguments, resolve
interactive choices, render reports, and choose exit status. Workflow libraries
return typed plans, effects, errors, and observations without application prompts.
Explicit interactive preparation delegates history editing to Git.
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
require terminal stdin and stderr and explicit submission; keyboard selection or
a complete line supplies that submission. EOF or cancellation stops the pending
operation. Effects completed before that point
remain reportable.

[`presentation.rs`](../src/presentation.rs) contains the shared Inquire prompt
adapter and semantic heading styling. Crossterm owns keyboard input and terminal
restoration; the CLI retains choices, validation, plan revalidation, effects and
exit status. Descriptive choices return their original tokens; prefix filtering
and exact numbered answers keep unmatched input correctable. Sections and
questions provide hierarchy, while wrapped descriptions and bounded menus keep
narrow terminals usable. Text preserves literal values. Local suggestions use
prefix filtering and Tab completion of a highlighted value or common Unicode
prefix. They assign no defaults, and control-bearing suggestions are omitted
rather than displayed as executable terminal controls. Explicit defaults
select the existing preference; otherwise cancellation is initially selected when
available. Esc, Ctrl-C and Ctrl-D cancel before submission. Line-oriented fallback
requires a complete submitted line and preserves EOF cancellation.

Inquire is used with only its Crossterm feature. Its documented Unix/Windows
backend capability is distinct from application portability. It was selected over
Cliclack because Inquire's input actions handle Ctrl-D cancellation, matching the
CLI's EOF contract; Cliclack 0.5.6 handles Esc and Ctrl-C but ignores Ctrl-D.
See the [Inquire documentation](https://docs.rs/inquire/0.9.4/inquire/) and
[input actions](https://docs.rs/inquire/0.9.4/src/inquire/prompts/action.rs.html).
The adapter uses plain complete-line input when stdout is redirected or
`TERM=dumb`, while command eligibility continues to require terminal stdin/stderr
for unresolved approval. `NO_COLOR` and `CLICOLOR=0` remove colour; redirected
reports contain no styling control sequences. Shared error entry points use red,
success report headings use green, and actual cache-attention headings use yellow.
The caller selects an application heading and its output stream explicitly;
supporting Git diagnostics retain their original presentation/redaction. A cache
warning cannot change confirmed delivery or command exit status. Library rendering handles terminal
width; application byte escaping remains responsible for literal display safety.

[`activity.rs`](../src/activity.rs) scopes activity to CLI invocations and wraps
captured inspection/workflow Git processes. Library calls remain silent. A delayed
indicator starts only after 500 ms, using fixed operation labels without endpoint
arguments that might contain credentials. Indicatif animates when all standard streams are terminals;
redirected, dumb and colour-disabled streams receive one activity line and one
process outcome. Completion joins the indicator worker before reports or prompts
resume. A Git exit describes process completion, never verified delivery. External
application termination retains ordinary abrupt signal semantics; it does not
produce a final activity outcome. The indicator changes neither raw input mode
nor cursor visibility.
Inherited native history/editors and streamed checks bypass this boundary, so no
indicator competes with their output or input. Transport prompting remains governed
by the existing noninteractive Git policy.

### Current capability boundary

Start creates or resumes an exact work branch. Save preserves selected local
progress and publishes the current branch. CLI finish prepares and checks work,
then publishes it for manual review or delivers its integrated result to the
project base. Review mode leaves submission to the developer and identifies the
publication branch and project base in its output. Explicit release and hotfix
policy additionally binds a fixed completed work commit and selected version
identity to ordered direct delivery or review preparation. Direct delivery can
advance an optional
release marker; hosted preparation reports its manual follow-up. Finish creates no
pull request and invokes no provider submission or deployment tool.

Direct integration uses its own workflow library in
[`finish::integration`](../src/finish/integration.rs). It provides read-only
planning and recovery assessment, explicit remote refresh, local work preparation
and target integration, guarded publication of the verified candidate, and
verified final checkout and work-branch cleanup. The CLI coordinates this library
for integrate mode, including relevant choices, message editing, delivery, and
completion. Review plans cannot be passed to the integration executor or used as
target publication authority.

## Shell completion

[`completion.rs`](../src/completion.rs) supplies a shell-neutral Clap command
model and dynamic candidates through `clap_complete` 4.6.11's engine. Workflow
parsers retain literal syntax, semantic validation and exit decisions; completion
models their vocabulary without replacing execution validation. Grammar parity
coverage checks the public option/alias surface against installed help and parser
cases. Shell-specific grammar and Git inventories are absent from the adapters.

[`completion_shells.rs`](../src/completion_shells.rs) and the sourceable
[Bash adapter](../completions/twigsmith.bash) transport literal words to the
private `__complete` entry point, handled before activity and workflow dispatch.
Bash preserves Readline word-break and quote contexts, and suppresses mid-word
replacement because Readline retains a suffix after the cursor; Zsh uses native
completion quoting; Fish decodes shell quoting without expanding input; PowerShell reads AST
literal values and quotes insertion text separately. PowerShell 7.3 or newer is
required for Standard native argument passing; the completer selects that mode
in its local scope for backend queries without changing the caller’s preference.
Actual command execution continues to use the shell’s selected argument mode;
Legacy mode retains its ordinary embedded-quote conversion. See [PowerShell native parsing](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_parsing).
Typed input is never evaluated by completion. Static choices need no repository. Repository candidates use
inspection guards, current local refs and configured fetch mapping inversions,
respect negative exclusions, and omit symbolic or ambiguous cache mappings.
Prefix filters narrow Git enumeration while retaining nested branch names and
selected custom cache destinations. Branch and source/tag contexts remain distinct.
Hints prove neither current remote existence nor workflow eligibility. Unavailable settings limit remote-role
hints while keeping static completion usable; control-bearing and non-UTF-8
names are omitted from shell suggestions.

The [dynamic API](https://docs.rs/clap_complete/4.6.11/clap_complete/engine/index.html)
and [custom value completion](https://docs.rs/clap_complete/4.6.11/clap_complete/engine/struct.ArgValueCompleter.html)
require `unstable-dynamic`, with Clap's `unstable-ext` for argument extensions.
The exact completion-library version is pinned because these APIs can change.
The engine was selected for shared command-context matching and custom values.
Static-only generators cannot supply current repository values. Upstream
[`CompleteEnv`](https://docs.rs/clap_complete/4.6.11/clap_complete/env/index.html)
supports the four shells, but its [4.6.11 shell integration source](https://docs.rs/clap_complete/4.6.11/src/clap_complete/env/shells.rs.html)
shows that the PowerShell adapter evaluates reconstructed
command text, and its Bash adapter lacks this application's established literal
word-break handling. The application therefore owns small transport/insertion
adapters and a private query protocol rather than adopting those integrations.
This is an application containment choice, not a claim that the upstream engine
provides these safeguards. Candidate alias/option expansion uses the selected
engine's candidate IDs; parity tests guard this version-sensitive boundary.

Stable PowerShell native completers fall back to filesystem suggestions when
custom completion returns no matches, as documented by the
[PowerShell issue](https://github.com/PowerShell/PowerShell/issues/19628) and verified
with PowerShell 7.5.4. A scoped `TabExpansion2` wrapper filters only provider
matches in literal tool argument contexts. Application path candidates retain
their explicit completion type. The wrapper retains the prior delegate, static
parameter metadata, common parameters, output types and unrelated results; a
closure and ownership marker prevent repeated loading or subsequent user
replacement from creating delegate cycles. AST inspection identifies the innermost
command without evaluating input, and exact executable basenames keep similarly
named scripts and command-position completion outside the filter. Shell redirection
targets retain the prior delegate's filename suggestions; subsequent command
arguments use the tool's normal completion context.

Microsoft calls [overriding TabExpansion2](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/tabexpansion2)
unsupported customization. This tested host adaptation is chosen to keep default
file inventories out of branch/tag/message contexts on stable PowerShell, without
changing key bindings or relying on internal completion sentinels. It is a material
compatibility boundary, alongside the unstable Rust completion API. The standard
console host routes completion through this function; custom hosts using the
native engine directly retain its fallback. Arbitrary dynamic extensions to a
user-provided host function have not been validated.

[`completion_setup.rs`](../src/completion_setup.rs) prints session code or plans
an explicit shell-specific user registration. Installation/removal reviews the
location and exact managed block before terminal approval; `--yes` supplies
unattended authorization. Automatic profile replacement is supported on Linux. Existing profile bytes,
ordinary permissions and owner/group are retained. Profiles with extended attributes
(including POSIX ACLs), or staging files inheriting such attributes, are refused
before replacement because registration cannot preserve that metadata. Inspection
covers attributes visible to the invoking user; privilege-hidden namespaces such as
Linux `trusted.*` are outside this user-profile preservation boundary. Other
platforms can load session code or add the generated registration manually; automatic
replacement refuses without changing their profiles.
UTF-16/UTF-32 and NUL-containing profiles require a UTF-8 startup file.
Exact blocks make repetition idempotent and removal conservative; edited,
duplicated or malformed markers stop automatic changes. Symlink paths and
multiply linked profiles require an explicit regular-file location. Updates stage
and synchronize a temporary file, revalidate the profile, then publish it. This
protects ordinary failures and observed changes, without claiming a cross-process
profile lock. Startup regenerates adapters from the current executable on PATH,
avoiding persistent code tied to a previous unstable protocol. The user restarts
existing shells after upgrades. Registration performs no shell installation or
execution-policy changes.

The shell boundaries follow the documented
[Bash programmable completion](https://www.gnu.org/software/bash/manual/html_node/Programmable-Completion.html),
[Zsh completion system](https://zsh.sourceforge.io/Doc/Release/Completion-System.html),
[Fish completion](https://fishshell.com/docs/current/cmds/complete.html), and
[PowerShell native argument completion](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/register-argumentcompleter)
interfaces. PowerShell registration uses an explicit
[profile path](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_profiles)
to respect the active host's location. Tests use isolated profiles and actual
shell completion; runtime evidence on Linux establishes no Windows compatibility.

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
so even an unrelated changed value can require reassessment. Save and review
compare settings by resolved values and sources, with finish additionally binding
committed configuration blobs. Start's checkout guard and integration stages
snapshot recognized shared and private file contents and modes; only verified
tracked-tree transitions may change shared files during the selected Git effects.

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
`twigsmith.config.toml`, `.yaml`, `.yml`, or `.json` file. Multiple recognized
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
Hints split record delimiters before display and invert the selected remote's
full-head fetch mappings, respecting exclusions and discarding ambiguous or
symbolic cache refs. Stored branch suggestions are source branch names, independent
of their local cache display names; remote names assign no workflow roles.
Generation validates the proposed shared settings and their effective combination
with private overrides, including workstream field relationships, before staging
and again before publication. Grouped guided input retains existing preferences,
exposes structured workstream fields and literal executable/argument entry, and
keeps invalid combinations correctable. Review summarizes effective behavior and
consequential private changes; exact serialized contents remain available before
explicit creation. Private values and provenance participate in the audit without
being exported into shared contents. Remote, branch and check availability remain
runtime concerns, so valid future/offline configuration can be generated.
Generation stages complete serialized bytes and publishes them without clobbering
an existing recognized file. It creates configuration without changing branches
or probing checks. The local-only `redact-credentials` preference controls finish
presentation and is excluded from shared settings. Redaction changes displayed
HTTP(S) userinfo, including streamed check output, without changing executed
arguments. [`finish_cli.rs`](../src/finish_cli.rs) owns that presentation policy.

### Explicit workstream policy

Settings include sparse `workstreams.ordinary`, `workstreams.release`, and
`workstreams.hotfix` templates. Creation source, preparation source, integration
destinations, work publication, version tagging and identity, active-release
context, marker, and
finish choices are independent data. The explicit `config --oneflow` bundle
serializes editable templates using the same schema as custom settings; there
is no strategy dispatch during lifecycle execution. `config --oneflow-hosted`
serializes tagged review templates with separate work and merge-back publication
routes through that same schema. Built-in ordinary settings
remain the default when the bundle is not selected.

Start resolves explicit policy and retains its association with the exact work
branch in private Git configuration. The association binds resolved routes,
creation commit/input, and branch-instance evidence. Resume preserves this
policy; changed templates, incompatible explicit input, ambiguous associations,
or a recreated branch cannot inherit its effects. Branch prefixes establish no
authority. Work publication uses the policy's publication remote, independently
of the project remote. Ordinary contributions can select a release or hotfix
branch as preparation/integration target without adopting its release policy.

Shared/private policy keys retain sparse layering, validation, and provenance.
Configuration inspection displays effective ordered routes and version identity.
A tagged direct template can select `integration-targets` instead of the singular
`integration-target`; resolution retains the first destination as the primary
target. The singular and plural route are alternatives: a later layer replaces
the earlier route and its effective provenance; both in one layer are ambiguous.
Plural routes require version tagging. A declared active release remains the first
hotfix destination. Empty, duplicate, conflicting, and invalid routes are
rejected, as are source/marker
aliases. Lists do not select per-destination methods or derive routes from history.
Integration recovery selects the original source association after temporary
target checkout and after source
cleanup; the target's own policy cannot replace the contribution's policy.
Private branch association supplies no delivery or deletion proof by itself.

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
contacting remotes. The request accepts one literal branch name and preserves its
spelling, case and Unicode form. Spaces are invalid rather than converted. The CLI rejects
extra positional arguments as syntax errors; explicit source selection uses
`--from` or `--from-tag`.

Targets are checked as full `refs/heads/<name>` refs, avoiding checkout-shorthand
expansion by `check-ref-format --branch`. Leading `-`, reserved `HEAD`, NUL,
revision expressions, and the exact configured base name are rejected. The
[resolution tests](../tests/start_resolution.rs) cover literal names and invalid inputs.

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

Checkout can restore, remove, or change tracked shared configuration. The final
guard compares recognized configuration files with the approved target tree and
keeps the pre-switch private configuration and effective Git configuration bound.
It validates the original request using its original settings; restored settings
grant no additional start effects. Subsequent commands load the current branch's
configuration. Unexpected file changes, including checkout-hook changes, still
stop execution.

## Save: preserve selected work, then publish

[`save::assess`](../src/save.rs) records the attached branch/commit, local scope,
settings, required input, ordered preservation steps, and exact publication
destination. It performs no writes or remote contact. Save protects the configured
base and rejects conflicts, active operations, and dirty submodule contents;
clean submodule commit-pointer changes can be preserved in the parent repository.

Scope separates the original index, remaining tracked changes, and nonignored
untracked paths. A file can belong to both change groups. An ordered plan commits,
stashes, or leaves each group, with `all` selecting both as one artifact. Empty
selected groups produce no artifact. An unresolved dirty-tree plan exposes no
preservation or publication effects.

The local executor validates the requested plan and publication route before
preservation. It captures original trees through a temporary index, and tests
whether a required independent group can be applied without conflict before changing
refs, the index, or the working tree. Dependent edits can be preserved together;
inseparable requested groups fail before preservation. Ordinary commits use Git's
hooks and signing; unexpected changes stop later effects. Stashes retain native Git
object identities and remain local. Ignored untracked work is excluded.

Private index and delta files use the `twigsmith-save-index-` and
`twigsmith-save-delta-` prefixes in the worktree Git directory.

Before reconciling the index and working tree, save retains the original work as a
native-compatible stash object under a visible `refs/twigsmith/save-recovery/` ref.
It removes that ref after verified local preservation; failures or interruption can
leave it for recovery. Completed commits and stashes remain independently visible.
A fresh retry assesses current state rather than replaying the previous plan. See
[save recovery](USAGE.md#resume-a-failed-save-or-publication) for inspection and
reconstruction guidance. Preservation and hook-failure cases are exercised in
[save execution tests](../tests/save_execution.rs).

### Exact save publication

[`save::publication`](../src/save/publication.rs) accepts a resolved plan with no
pending local steps. It permits dirty publish-only work and an existing index
lock because publication performs no staging, commit, stash, or index edit.
It revalidates the selected branch/commit and scope, then pushes through a
command-scoped remote with the configured endpoints and a literal
`<approved-oid>:refs/heads/<branch>` refspec. A pre-push hook moving the local branch cannot change that pinned source.

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
- Ordinary named Git pushes can update fetch-mapped local refs. Positive fetch mappings
  with destinations must have full destinations under `refs/remotes/`; any
  mapping elsewhere is refused, including unrelated mappings. Matching symbolic
  tracking refs are refused. Negative mappings, source-only refspecs, and
  restrictive tracking mappings remain supported.
- Legacy remote definitions without configured URL values are refused because
  their separate mappings cannot be checked through this configuration boundary.

Signing, receive-pack, push options, credentials, helpers, and normal hooks remain
effective. A pre-push hook receives the command-scoped remote name and selected
endpoint. Unsafe configuration is reported rather than rewritten. The containment
rationale is supported by positive-control ordinary Git examples in
[save publication tests](../tests/save_publication.rs): even a non-forced
single-branch push can change local branch history through a fetch mapping.

## Finish for review: prepare, check, and deliver

The CLI resolves completion mode before refresh and revalidates the selected
local plan across the mode prompt. Review composes the following library stages:

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

With history editing disabled, fresh inclusion of the base resolves update to `none`, irrespective of update preference.
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

### Interactive work preparation

`finish-history-edit` and the matching request field resolve independently of
update, integration, message, rewrite, checkout, and cleanup. The built-in choice
is `none`. `interactive` requires ordinary work and update `rebase`; it continues
to require preparation when the work already contains the base. Tagged direct
and hosted preparation reject that choice. Complete history, no replacement refs,
and a unique preparation merge boundary are required before editing. A different
integration target cannot contribute shared commits inside the editable range.

[`finish/history.rs`](../src/finish/history.rs) runs Git's native interactive
rebase with its sequence/commit editor preferences. Before starting it checks
ignored path overlap against the boundary and every replay-range tree, including
file/directory prefixes. This protects ignored content absent from the source's
final tree but present at an intermediate replay. Automatic update-refs,
autosquash, autostash, and fork-point remain disabled; merge structure is preserved.
Native editors, hooks, and explicitly added todo commands are developer-controlled
code. Those commands are not sandboxed; the range and incidental-effect guards
contain the tool's selected operation, not arbitrary editor execution.

Without terminal stdin and stderr, both editor roles must have explicit settings
or environment selections. The developer selects commands suitable for unattended
execution; arbitrary configured commands cannot be proven to terminate. Terminal
Git/editor UI inherits its terminal streams. Captured unattended diagnostics use
normal CLI redaction; native terminal UI, like the integration message editor,
receives original content.

Review records local preparation/target refs and raw reflogs; direct integration
binds its separate preparation ref alongside its existing target evidence. Both
compare pinned remote observations immediately after editing, before later effects.
A changed observation is rejected rather than adopted as fresh authority.

Preparation retains original work before rewriting, binds the attempt to its
source and boundary, and requires a corroborated native finish transition plus
clean attached state. Git can return success at an intentional edit stop; that
state cannot pass checks or authorize delivery. Reorder, combine, reword,
amend/split, and drop use native Git semantics. A complete edit may preserve
commit IDs or remove all work. Reassessment uses the actual result; an empty
contribution requires no integration commit or message. Checks on earlier trees
do not validate a changed result.

Fresh processes reuse verified preparation with matching context. Resets,
recreation, lost evidence, changed settings/routes, or unfinished/aborted operations
cannot supply the earlier edit's delivery authority. Review replacement still
requires current rewrite permission and retained exact remote incorporation proof.
Direct integration preserves its selected ff-only, no-ff merge, or squash method;
source editing alone supplies no remote source publication permission. Direct
cleanup can use the retained original-source incorporation proof for the exact
published version observed before preparation, after verifying the native
preparation and integration transitions.
[Ordinary CLI editing tests](../tests/cli_history.rs) exercise native operations,
actual resulting history, no-work integration, and cross-process retries.
The [ordinary lifecycle fixtures](../tests/cli_lifecycle/ordinary.rs) combine
shared setup, save preservation, branch resume, prepared delivery and work cleanup,
and fresh-process continuation through the public commands.

### Rebase recovery proves incorporation

Before a review rebase, [`finish/recovery.rs`](../src/finish/recovery.rs) writes
an atomic version-3 record at
`<worktree-git-dir>/twigsmith-finish/branches/<branch>/rewrite.json`. Owner-only
files/directories on Unix and symlink rejection protect its private storage.
The record binds repository/worktree identity, refs, remote roles/endpoints,
ordered original destination observations, original commits, fetched bases,
selected remote/URL-rewrite configuration, and branch/HEAD reflog anchors.
Interactive records additionally bind full settings/configuration, local shared
refs/raw logs, exact target evidence, and an original retention ref with its
creation reflog. Raw branch and HEAD prefixes corroborate the pre-edit anchors;
interactive completion accepts only its verified finish, with no later source
reset or rewrite.
It stores operational evidence rather than reusable rewrite permission.
Rebase starts only when both reflogs anchor the current commit.

Assessment corroborates original incorporation and the anchored rebase interval:
the unique marked start, supported successful finish, fetched-base incorporation,
and resulting branch tip. For noninteractive preparation, later commits or merges
must preserve that verified result through positive ancestry; further finish
rebases can extend the verified chain while publication is pending, including
after partial publication. Interactive preparation keeps its exact completed
result and boundary. Invalid retained interactive evidence blocks a new edit
until the bound context is resolved. An abort or unstarted attempt supplies no
new proof.

Git continuation can finish in another process, and verification needs no final
journal acknowledgement after Git succeeds. A covered exact original destination
commit can then qualify for `VerifiedRewrite`, with current allow still required.
New remote work cannot borrow that record. With history editing disabled, ordinary
incorporated destinations use normal ancestry even when a record is invalid.

Expired reflogs, missing objects, changed identities/endpoints/configuration,
discarded result history, unsupported versions, or malformed records stop proof
of replacement. Interactive attempts retain original work in a private Git ref; the metadata
records themselves supply evidence rather than backups or branch roles. Records
remain local and are not automatically cleaned up. Recovery relies on recognizable Git
reflog conventions and cannot authenticate against a party able to forge both
history/logs and private records. Continuation, abortion, invalid evidence, and
cross-process retry are covered in [finish preparation tests](../tests/finish_preparation.rs).

### Checks validate the current candidate

[`finish::checks`](../src/finish/checks.rs) sequences the effective ordered list
after preparation and before destination transport. The review publication API
invokes checks on every attempt, including retries and already-published destinations.
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
command-scoped remote name and actual URL. These transports suppress Git's implicit tracking-cache writes; eligible caches are
maintained separately after verified delivery.

Retries confirm completed destinations through fresh equality and retain original
expectations for pending replacements. The rebase record remains intact; no final
publication journal write is necessary after a killed process or lost response.
Publication pins the prepared base and does not reread it or prepare again.
An advancing base requires fresh assessment/preparation when that newer base is
wanted. [Finish publication tests](../tests/finish_publication.rs) exercise leases,
multiple endpoints, partial success, lost acknowledgements, and cross-process retry.

## Tracking caches after delivery

[`tracking::cache`](../src/tracking/cache.rs) maintains only the remote-tracking
refs identified by configured fetch mappings for an independently confirmed
publication or deletion. Publication transports use the shared
[command-scoped endpoint builder](../src/publication/transport.rs) without fetch mappings so Git cannot write these caches implicitly. A cache supplies no
publication, incorporation, lease or deletion proof.

The helper binds repository identity, routing configuration, the configured fetch
endpoint and each mapped ref's version and raw reflog before delivery. Positive
mappings must identify full destinations under `refs/remotes/`; negative mappings
exclude sources. Inverse mapping collisions, symbolic refs, endpoint mismatch and
changed observations leave caches intact. An exact fresh remote observation must
still match the confirmed effect. Conditional no-deref ref transactions lock the
cache and recheck binding and remote equality after transaction hooks before
committing. Existing newer or unrelated local observations are preserved; history
evidence
must establish safe advancement when exact previous-remote equality does not.
Graft history cannot supply that ancestry proof. Fetch matching/exclusions follow
[Git fetch refspec semantics](https://git-scm.com/docs/git-fetch); the conditional
write and prepare/commit lock boundary follow
[Git update-ref](https://git-scm.com/docs/git-update-ref). The stricter eligibility
and post-hook checks are application containment policy.

Cache results are separate from delivery and upstream configuration. Unavailable
refresh does not erase confirmed effects or authorize another push/deletion. A
retry observes actual remote state through the workflow's normal guards; cache
contents grant no additional authority. Maintenance performs no blanket fetch or
prune and changes no branch tracking configuration. As with publication, the
remote observation and local transaction are separate boundaries; a remote can
move after observation. Tests and journey checks exercise these limits with real
Git and local transports.

## Publication reports and failure diagnosis

Save, review finish, and direct integration share the
[exact-ref porcelain decoder](../src/publication.rs).
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

## Direct integration

Direct integration uses the shared Git observation/ancestry primitives and check
runner, with its own plans and private recovery evidence. It targets the exact
configured base head at `base-remote`, rather than review's work-branch publication
destination. Updating the work branch and integrating into the target are
independent decisions. The target is shared history, so working-branch rewrite
permission cannot broaden allowed target effects.

### CLI coordination and input

[`finish_cli.rs`](../src/finish_cli.rs) selects the workflow before remote effects;
[`finish_cli_integration.rs`](../src/finish_cli_integration.rs) coordinates direct
delivery. Integration resolves only input relevant to the observed graph, prepares
work, reassesses the actual result, publishes through the API that runs candidate
checks, and
completes checkout/cleanup only after an error-free delivery report. Interrupted
result retention is repaired from a corroborated target transition before delivery
or completion; pending delivery restores the candidate checkout when needed for
checks. Already-delivered retries leave checkout to the completion policy. The CLI
uses the verified root after switching branches. Review retains its independent pipeline.

Choice resolution revalidates the presented plan before and after reassessment.
New input changes only its selected choice; concurrent context changes cannot
become fresh authorization through a prompt. Accepted method/message input after
verified preparation is retained through a guarded private-record write before
target effects, even if subsequent input is cancelled. A message editor follows
Git’s editor preference resolution, documented in [git-var](https://git-scm.com/docs/git-var.html).
The editor receives a private message file and inherited terminal I/O for normal
interactive editor operation; message text is never shell input. Finish redacts
its own rendered output, while the external editor controls its terminal display.
The integration message editor resolves `git var GIT_EDITOR` and always launches
that command through POSIX `sh` on the inherited PATH. This existing launcher is
separate from Git's platform-specific shell resolution; Windows availability,
path conversion and quoting are unvalidated. `-m/--message` and automatic messages
bypass it. Native rebase delegates its editor launching to Git itself. Git documents
editor values as shell-interpreted commands in
[git-var](https://git-scm.com/docs/git-var); replacing this launcher with Windows
`cmd.exe` would change those command semantics. The optional shell completion adapters add no shell dependency to ordinary
workflow execution.

Messages are required only for new merge/squash commits. Git commit hooks,
message cleanup, and signing remain effective.

Retained intent supplies omitted integration choices and accepted messages on
retry. The record alone grants no authority: settings/configuration, identity,
refs/reflogs, operation transitions, and retained commits must still corroborate
the attempt. Explicit conflicts remain blockers. Preparation can expose an
integration method or message choice later; only an unchanged pre-integration
target permits binding those choices to the prepared attempt.

### Planning stages

[`integration::assess`](../src/finish/integration.rs) binds exact source/local
target versions, the target fetch endpoint and ordered push endpoints, and fresh
evidence supplied by its caller. Published-source observations are also required
for remote cleanup planning and explicit tagged rebase authorization. Assessment
performs no transport or writes, and
remote-tracking caches supply no authority.

Fresh target observations must agree at fetch and every push endpoint. A missing
target is resolved outside this model. The local target may be absent, equal,
or behind by proven ancestry; unrelated local target work, symbolic refs, and
occupation in another worktree block integration.

With history editing disabled, an already-incorporated source needs no update,
integration method, or message. A source containing the target needs no preparation. Divergence resolves the
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
required before completion effects. Publication and completion have separate
guarded executors; completion reobserves delivery before checkout or deletion.

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
the pinned source or its corroborated retained original before preparation.
An old remote observation alone supplies no incorporation proof. New work,
another worktree's occupation, uncertain publication,
or changed destinations block destructive completion. An absent branch counts as
completed cleanup only within verified attempt evidence.

### Remote refresh and target publication

[`integration::remote::refresh`](../src/finish/integration/remote.rs) explicitly
observes the target's exact head at the configured fetch endpoint and every push
endpoint. When cleanup or tagged rebase requires published-work proof, it also
observes the source head at the publication remote's push endpoints. These roles
retain separate
transport configuration. Assessment itself performs no transport.

Refresh fetches each observed full OID without a destination refspec. Empty
`--refmap=`, disabled tag/prune/submodule handling, and disabled FETCH_HEAD,
commit-graph, and maintenance writes contain acquisition to objects. It verifies
the observed commit is available rather than substituting a newer branch tip or
cached history. An endpoint refusing that exact object stops refresh. These
mapping and metadata controls follow [git-fetch](https://git-scm.com/docs/git-fetch).
Fetch does not advance work/base branches, ordinary tracking refs, or tracking
relationships, and does not change the index or working tree. Completed transports
remain reported when a later endpoint or local guard fails.

[`integration::publication`](../src/finish/integration/publication.rs) requires
the same corroborated attempt and exact retained result. It independently proves
that the original target is an ancestor of the candidate. An explicit exact-OID
lease protects the original target at each destination; it does not prove ancestry.
[Git's lease option](https://git-scm.com/docs/git-push) permits a non-fast-forward
update when the expected OID matches, even with `--no-force`, so both guards are
required. Working-branch rewrite permission supplies no target authority.

Each destination is observed before its effect. Equality with the result confirms
delivery without a push; the original target can receive the pinned candidate;
other movement stops that destination. Before eligible pushes, publication runs
the effective candidate checks anew through the guarded runner, unless the caller
explicitly skips them. An observational retry with every destination already at
the result needs no checks. Local guards remain active between effects, and leases
are never renewed after rejection within an attempt.

Command-scoped transport selects one actual endpoint and excludes ref expansion,
mirror updates, tag following, automatic tracking, and submodule publication.
Each outcome retains command status and exact response validation separately
from observed destination equality. Multi-endpoint publication can be partial or
uncertain; retry refreshes evidence and reuses the retained result, without
preparing work or recreating an integration commit. Confirmed endpoints require no duplicate push.
Publication retains the private record and original/result refs. Fully confirmed
delivery exposes `Published` with remaining completion requirements, or `Complete`
when the chosen completion is already satisfied. This executor performs no final
checkout or branch deletion.

### Guarded checkout and work cleanup

[`integration::completion`](../src/finish/integration/completion.rs) completes
one corroborated, retained attempt from an evidence-bearing assessment. Callers
can obtain that assessment through explicit refresh. Completion observes every
exact target push endpoint at the completion boundary and before subsequent
effects; all must still equal the verified result. A publication report or supplied cache cannot authorize
checkout or deletion. Completion performs no preparation, integration, target
push, or candidate checks. Unresolved policy or changed local binding stops it.

Final checkout selects the verified target or source version. An already-current
branch is verified without another switch. A switch requires a clean index and
worktree, no active operation, and an available branch in this worktree; ignored
files are protected from overwrite. Git hooks remain effective, so a failing hook
can leave the requested branch checked out. Reports retain that observation and
stop cleanup on failure. Execution and final observation use the verified worktree
root, including when checkout removes the caller's nested directory.

Local cleanup deletes the exact verified source after final checkout. Integration
proof supplies its eligibility, including the method-specific squash transition;
ordinary source ancestry in a squash result supplies no proof. The source cannot
be current, symbolic, occupied elsewhere, or changed. A prepared ref transaction
holds Git's source lock while the executor rechecks its type and worktree guards,
then commits an expected-OID deletion. [Git update-ref](https://git-scm.com/docs/git-update-ref)
provides the conditional deletion and transaction boundary. The transaction binds
deletion to the approved version; integration proof supplies eligibility separately.

Deletion removes the source ref and its reflog. Source-associated Git configuration
is retained: completion makes no configuration edits. Verified remote deletion can
remove its eligible mapped tracking cache; unrelated refs remain outside cleanup. The existing private attempt retains the source evidence needed
to corroborate preparation after reflog removal. Original/result refs and private
recovery records remain available throughout completion and retry.

Published-work cleanup uses the publication remote's endpoints independently of
base-remote delivery. Each eligible recorded source version must be proven included
in the integrated source. Verified ordinary preparation can instead corroborate
exact original-publication incorporation in the retained original work and the
native preparation transition. Tagged one-time rebase additionally requires its
bound rewrite authorization. This proof covers only the
originally observed version, with the original retention and branch evidence still
required; newer publication cannot borrow it. The executor observes the exact head
immediately before
deletion and uses `--force-with-lease=<full-head-ref>:<recorded-oid>` with an empty
source refspec. [Git push](https://git-scm.com/docs/git-push) binds that deletion to
the expected remote OID; a newer observation never renews the lease. Command-scoped
transport excludes tag following, mirror/refspec expansion, tracking updates, and
submodule publication. Changed work remains available and cleanup remains incomplete.

Completion reports delivery, final checkout, local refs, per-endpoint cleanup,
command diagnostics, and remaining recovery state separately. Remote destinations
are non-atomic: confirmed deletions survive rejection or interruption at another
endpoint. Missing heads count as completed cleanup only within this verified
attempt's intended scope. Fresh observations recover a lost acknowledgement;
retry skips completed checkout/deletion and executes only remaining effects.
A recreated source, changed target or configuration, invalid private evidence,
dirty tree, or active Git operation cannot inherit old authority. A fully completed
attempt succeeds observationally without duplicate effects. Completion neither
rolls back delivery nor retires its recovery evidence.

The ref lock protects deletion from concurrent source-ref replacement after its
locked check; it does not lock another worktree's checkout or all repository
state. The [snapshot limits](#snapshot-limits) and trusted-hook limits still apply.

### Local execution and retention

[`integration::execution::prepare`](../src/finish/integration/execution.rs)
revalidates the approved plan and rereads the exact target at its fetch/push
endpoints before history changes. The caller supplies objects and initial evidence
from explicit refresh. Ordinary local execution performs no fetch or remote write.
Tagged execution also observes version refs, acquires any required marker objects,
and checks the exact source before creating its tag; publication remains a separate
stage. It
records intent and retains the original source at `refs/twigsmith/integration/<token>/original` before
changing history.

Work preparation preserves merge structure and uses the same bounded rebase
options as review. Preparation merges have Git-generated messages. The actual
prepared graph is reassessed before integration; unresolved final message input
can be returned after preparation. The backend does not prompt. Explicit interactive preparation invokes Git's
native editors; final integration message editing belongs to the CLI.

An absent/behind local target is conditionally created/advanced to the observed
target with a marked reflog transition and exact old-OID expectation. If resuming
on the checked-out target, Git fast-forwards ref, index, and tree together.
The executor checks out the target to construct the candidate; this temporary
workspace choice does not execute the configured final checkout policy. Both
initial and recovered candidate switches protect ignored files from overwrite.

Integration runs Git `--ff-only`, `--no-ff` with message, or squash followed by a
guarded commit. The automatic message is `Integrate <source> into <target>`.
Explicit messages are literal arguments with normal Git cleanup, hooks, and
signing. The verified result is retained at
`refs/twigsmith/integration/<token>/result`. Source/target branches and retention
refs survive failed checks. Reports distinguish new integration from reuse and
retain observed changes even when Git fails afterward.

`execution::check` requires a clean verified candidate checked out at its exact
result OID. It uses the shared ordered process runner with integration-specific
guards over records, refs, settings, history, and context. Check success cannot
authorize a later publication or omission of checks. Candidate failure retains
the result and a retry runs the effective list again.

### Association and recovery proof

[`integration/recovery.rs`](../src/finish/integration/recovery.rs) stores private
version-3 records at
`<worktree-git-dir>/twigsmith-integration/branches/<source>/attempt.json` and an
`active.json` selector for association after target checkout. Atomic writes sync
files and containing directories and reject symlinks; Unix paths must be private.
Records bind identity, exact source/target refs and commits, local target expectation,
endpoints, settings/provenance, Git configuration, choices, reflog anchors,
retention refs, and cleanup-relevant published source versions. Optional result
acknowledgements are corroborated evidence rather than stored success flags.
The current record format is required; malformed or unsupported records block
recovery. Branch-policy schema failures identify the local association key for
deliberate correction after pending work is resolved. Metadata replacement alone
cannot establish prior delivery or cleanup authority.

A retained attempt continues with its original settings, reconstructed from its
retained original regular shared-configuration file (or original absence) and
current private layer. Untracked shared files and symlinked settings retain strict
resolved-settings equality. Expected tracked configuration changes from
preparation, target checkout, and integration
are checked against the current committed tree rather than treated as new policy.
An authorized squash's staged configuration is checked against its exact index
blobs before committing the candidate.
Private settings, Git configuration, routes, endpoints, and branch associations
remain bound to the attempt. A changed or recreated branch cannot use this
configuration handling to inherit another attempt's authority.
The checkout guard supports Git's ordinary LF/CRLF conversion without running
external filters. Custom transformations of shared configuration that cannot be
matched to the selected tree stop this guard conservatively.

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
cover method-specific parentage, worktree association, conflicts, retention,
candidate checks, exact object acquisition, target leases, partial delivery, and
interrupted acknowledgements with cross-process retries.

## Tagged release delivery

[`integration/release.rs`](../src/finish/integration/release.rs) adds version
identity and release-marker delivery to the existing direct integration stages.
It shares the integration attempt's repository/worktree identity, exact source,
target/endpoints, settings/provenance, Git configuration, retained commits,
reflog anchors, choices, and cleanup proof. An optional release record extends
that evidence; ordinary integration retains its independent behavior.

The fixed completed work source and destination integration results have distinct
responsibilities. Each configured destination receives that same source independently,
in order. Its result and conflict resolution do not become the source for later
destinations. Tagged completion uses one history-preserving integration method;
squash is incompatible.

Tag identity is `work-source` by default, including both OneFlow bundles. Source
identity validates and tags the completed work before integration. With
`integrated-result`, the first destination is integrated and its exact validated
result supplies the version; later results supply no additional tags. Annotation,
signing, collisions, retention, and publication bind the selected tag object and
commit. Failure to establish the first result cannot authorize a substitute source
tag. Checks validate the actual completed source and destination candidates.

Tagged direct preparation defaults to none, independently of inherited ordinary
update preferences. An explicit invocation can select rebase against the resolved
preparation source. Persistent tagged rebase preference is rejected. Preparation
runs before source fixation, checks, tag creation, or any target integration and
reuses the existing guarded native rebase/retention machinery. Successful retained
preparation is corroborated on retry rather than replayed onto successive targets.
Preparation can move the release cutoff by importing preparation-base work; that
is an explicit developer choice. Original-work recovery, shared-history protection,
and published-work authorization remain separate from update selection. Native
preparation before finish requires no redundant tool operation. Interactive tagged
editing remains unsupported.

Checks validate each exact candidate before pending target delivery. Completed
work and the selected first result share successful validation when they are the
same exact commit. A candidate equal to an already checked source
needs no duplicate check.
Retained successful receipts are usable only with their corroborated unchanged
commit and complete policy/configuration binding. Failed or state-changing checks
stop delivery and retain the attempt. Receipts distinguish successful checks from
explicit bypass; a later unbypassed pending delivery validates bypassed trees.
A source-deleted retry can only verify existing delivery and finish cleanup.
Explicit check bypass remains reportable.

The integration attempt retains the ordered destination set and completed entries
alongside its current destination. Each completed entry binds the exact result,
local ref and reflog, and result-retention creation evidence.
[`integration::advance`](../src/finish/integration/sequence.rs) verifies prior
delivery, acquires the next destination's exact history, and binds its
independent transition through the same integration machinery. Changed completed
refs or missing pins cannot inherit completion authority. The fixed source and
corroborated preparation remain shared; candidate transitions remain per destination.
Fresh-process retries reuse the active and completed evidence. Publication and
cleanup reobserve the full required delivery set before effects.

Tag input is explicit and independent of the integration message. Git creates
lightweight or requested annotated/signed tags at the selected released commit,
with normal Git signing preferences and no unsigned fallback after failure.
Tagged completion rejects replacement refs in Git's effective replacement
namespace. Git can present a replacement tree as clean at the original commit
ID; that cleanliness cannot establish validation of the original released tree.
This restriction is supported by a real-Git regression and
[Git replacement semantics](https://git-scm.com/docs/git-replace).
The tag's exact object identity and attempt-marked creation reflog corroborate
creation even when the final acknowledgement was interrupted. An annotated tag's
peeled commit alone cannot establish identity. Existing local/remote tags outside
that corroborated attempt are collisions; no replacement is authorized.

Hotfix routing uses explicit active-release data: the exact named release branch
or declared absence. No branch scan or tag ordering supplies this context. An
active release receives the fix and carries it into development when that release
is later completed. The original OneFlow article documents earlier-commit release
creation, source tagging before merge-back, this hotfix route, and an optional
marker at the released commit. These are documented model capabilities, not
claims about universal project practice. The application's explicit context,
exact selected-tag publication, and guarded recovery are chosen product policy.
See [OneFlow's description](https://www.endoflineblog.com/oneflow-a-git-branching-model-and-workflow),
[Git tag](https://git-scm.com/docs/git-tag), and
[Git push](https://git-scm.com/docs/git-push).

An enabled marker is distinct from source and every integration destination.
Planning binds its initial local and per-endpoint versions, acquires exact missing objects, and
proves that each existing marker can fast-forward to the selected released commit.
Delivery initializes or conditionally advances the marker at that commit. Its
identity follows the tag selection independently of later integration results.
Disabled markers have no effect. Divergence, changed context, symbolic refs, and occupied worktrees block marker effects.

Target integration, selected version-tag delivery, and marker delivery are
nontransactional effects. Each required endpoint is observed independently. Pending target delivery
first verifies retained tag and marker expectations; a conflicting version cannot
borrow the earlier target authority. A push exit or acknowledgement alone cannot establish completion. Final checkout
and every work-cleanup effect reobserve all required delivery. Tag, development,
active-release target, marker, and unrelated refs stay outside work cleanup.
Rewrite permission cannot replace a tag or broaden target, marker, or deletion
scope. Partial effects survive failure and fresh-process retry completes pending
effects, including after local source deletion. Conflicting input, changed refs,
settings, endpoints, or associations block reuse. Native merge continuation or
abandonment retains the original released version identity.

The default checkout is the first integration destination. Required delivery
includes the selected tag, every target result, and any enabled marker; no individual
publication establishes complete finish. Work cleanup starts only after verification
of the entire delivery set. CLI reports distinguish selected version and fixed work
source, integration results/destinations, marker, cleanup, and pending work.
Hosted version preparation uses the separate review path described below. Provider
submission and deployment remain external to both paths. The [CLI OneFlow fixtures](../tests/cli_oneflow.rs)
and [ordered delivery fixtures](../tests/cli_direct_sequence.rs)
exercise release/hotfix safety and recovery with real Git through the canonical
command entry point. A focused ordinary lifecycle matrix verifies both executables
and their start/save/finish aliases with actual publication and integration effects.

## Hosted version preparation

[`finish::hosted`](../src/finish/hosted.rs) handles tagged work with
`finish-mode = review` and work-source tag identity;
[`finish_cli_hosted.rs`](../src/finish_cli_hosted.rs)
resolves terminal input and reports a version and separate merge-back branch
ready for manual submission. The selected template's `publish-remote`
receives original work, `base-remote` receives the version tag and supplies the
project target, and `review-remote` receives merge-back work. An omitted review
remote follows the effective work publication remote. `merge-back-branch` or
explicit finish input names the distinct branch; no prefix infers policy or
ownership.

Hosted assessment requires complete history without replacement refs or grafts;
missing history cannot establish this version-preparation evidence. The released
source and merge-back candidate remain separate identities. Source
checks validate the exact released tree before tag delivery. Tag identity includes
the exact tag object, its released commit, and corroborated attempt creation;
annotation/signing cannot fall back to another version. The merge-back branch
starts at the released source. Merge preparation imports the pinned target only
into that branch; none retains the source history. Rebase cannot preserve the
released history for this path. Candidate checks identify the prepared commit.
Before merge, explicit path-overlap checks protect ignored local content from
the retained target tree. Git 2.47.3 fixtures show that merge machinery can replace
ignored paths despite `--no-overwrite-ignore`; this application guard complements
Git's checkout and index protections. Retained successful receipts require unchanged
corroborated commits and bound
context; bypass receipts cannot authorize unbypassed pending delivery.

Hosted policy keeps work branches, denies history replacement, and selects
no-ff merge-back guidance independently of ordinary contribution preferences.
It neither integrates into the target nor advances the optional marker. The
marker is follow-up information, not a pending executor effect. Source, tag, and
merge-back publication select only their intended exact refs at their bound
endpoints, and independently verified equality recovers completed delivery.
Command errors remain reportable after effects and no rollback discards work.

Recovery covers the tool's own preparation and publication across processes.
Version-1 private records live under `<worktree-git-dir>/twigsmith-hosted/`, with
separate attempts and merge-back associations keyed by the exact source/branch
bytes. Private attempt evidence binds repository/worktree, original association,
settings/provenance and Git configuration, endpoints, source/tag identities,
merge-back creation and preparation transitions, and check receipts. Private refs
at `refs/twigsmith/hosted/<token>/target` and `/prepare` retain the exact target and
corroborate preparation start through attempt-marked creation reflogs. Missing or
changed acknowledged retention blocks reuse. A branch at
the same OID is insufficient proof of ownership after recreation. A conflict
resolution must establish the recorded source/target merge transition before
candidate publication; it cannot redefine the released version. Native Git merge
continuation and abandonment retain the original version identity. Changed or
missing evidence blocks affected operations rather than adopting existing work.

Successful output reports verified readiness for manual submission, names the
review source and target, and gives no-ff and manual follow-up instructions.
Readiness ends this attempt's responsibility. Later PR activity is neither
observed nor reconstructed from Git history. No provider API, external workflow
tool, submission-state polling, post-review cleanup, or marker completion stage
exists in this path. The developer resolves later review conflicts on merge-back
work, preserves the tagged version, and removes temporary branches only after
appropriate integration.

The separate source and no-ff guidance are documented in
[Ruka's hosted OneFlow article](https://www.endoflineblog.com/implementing-oneflow-on-github-bitbucket-and-gitlab).
The article describes a workflow, not universal project practice. Exact selected
tag delivery, declared active-release hotfix routing, guarded recovery, and the
readiness boundary are this application's policy. The hosted article does not
restate the original active-release hotfix case; explicit settings preserve that
routing rather than inferring release context from hosted submission state.
[Hosted lifecycle tests](../tests/cli_hosted_oneflow.rs) cover routes, immutable
version history, native merge recovery, collision containment, and entrypoint
wiring; [recovery tests](../tests/cli_hosted_recovery.rs) cover interrupted effects,
partial destinations, check receipts, and changed evidence.
[Presentation tests](../tests/cli_hosted_presentation.rs) cover terminal
cancellation, prompt revalidation, and streamed credential protection.

### Review with deferred version identity

Integrated-result tagged review uses the existing work-review preparation/checks/
publication pipeline for one target. The eventual merged result belongs to the
external review operation, so this path creates no tag, merge-back branch, or
marker update. It reports the exact published source and target with instructions
to merge, validate and tag the actual integrated result, then perform applicable
cleanup and marker advancement. Readiness supplies no proof of external completion
or authority for later effects. Multiple-destination review is rejected before
publication. The direct-only source-rebase extension does not change work-source
hosted merge-back preparation semantics.

## OneFlow model coverage

The [2017 OneFlow model](https://www.endoflineblog.com/oneflow-a-git-branching-model-and-workflow)
describes feature completion by interactive rebase followed by fast-forward or
no-ff integration, no-ff integration alone, and releases/hotfixes tagged before
merge-back, with optional release markers and declared active-release routing.
The application composes those operations through independent explicit choices.
The [2021 hosted adaptation](https://www.endoflineblog.com/implementing-oneflow-on-github-bitbucket-and-gitlab)
uses provider review and separate version merge-back branches with no-ff guidance.
Review finish prepares ordinary work for submission; tagged hosted finish prepares
that separate merge-back branch and preserves the released version.

These articles document models, not universal practice or provider compatibility.
Submission, provider merge settings, deployment, later review conflicts, and
post-submission cleanup remain manual/external. Hosted readiness ends at verified
publication, so no provider API or later submission-state verification is implied.

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
