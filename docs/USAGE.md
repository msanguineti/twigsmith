# User manual

Git OneFlow helps you create or resume a working branch, preserve progress, and
deliver finished work for review or direct integration. Use your normal Git commands to edit history,
stage changes, resolve conflicts, and submit the published branch to your hosting
service. Branch prefixes such as `feature/`, `release/`, and `hotfix/` are naming
conventions; they do not select different operations.

Use `git-oneflow` or the equivalent executable `gof` from a Git working tree.
Both accept the same commands and options. Git 2.45 or newer must be on PATH.
The [development guide](DEVELOPMENT.md) explains how to build the executables;
use their full paths when they are not on PATH.

In this manual: [a working session](#a-working-session),
[commands](#command-reference), [settings](#repository-settings),
[guarantees and limits](#guarantees-and-practical-limits),
[automation](#automation), and [troubleshooting](#troubleshooting-and-recovery).

## A working session

Configure the Git remotes and the project base before starting remote-dependent
work. The defaults use `origin` for both publication and the base, with `main`
as the base branch. Inspect those settings or create an editable shared file:

```sh
gof config show
gof config
```

Then create a branch, make changes, save progress, and publish for review:

```sh
gof start "add user avatars"
# Edit files, then select the changes for a commit.
git add src/avatars.rs
gof save --staged --message "Add user avatars"
# Finish with a clean working tree.
gof finish --mode review --update merge --no-rewrite
```

The phrase creates `add-user-avatars`. Save commits your staged selection and
publishes that branch. Finish incorporates the current remote base when needed,
runs configured checks, publishes the prepared branch, and reports where to
submit it manually for review. The branch remains checked out and available.

| Command | Main effects |
| --- | --- |
| `start`, `s` | Switch to existing local work, resume published work, or create new work from the fresh remote base |
| `save`, `v` | Preserve the selected local work and publish existing or resulting branch commits |
| `finish`, `f` | Prepare and check work, then publish for review or integrate into the project base |
| `status` | Read local repository state |
| `config show` | Read effective settings and their sources |
| `config` | Create a shared settings file after guided review |
| `config --yes` | Create shared built-in defaults without prompts |

Finish supports manual review delivery and direct integration into the project base.
Direct integration can retain or delete the work branch after verified delivery.
Explicit release and hotfix policy delivers a version tag and either a hosted
merge-back branch or direct integration with optional release-marker delivery.
Pull request creation, provider submission, and build/deployment packaging remain
outside these operations.

## Command reference

Running with no arguments shows help. Help and version work outside a repository,
including without Git installed:

```sh
gof --help
gof --version
gof start --help
gof save --help
gof finish --help
gof status --help
gof config --help
```

`-h` also shows help; `-V` also shows the version. Each command accepts `-h` or
`--help` on its own. Commands operate from the repository root or a nested
directory. Start, save, and finish require a working tree; status and configuration
inspection also support bare repositories.

### Start or resume work

```text
gof start <branch-name-or-phrase> [OPTIONS]
gof s <branch-name-or-phrase> [OPTIONS]
```

Supply one branch name or quoted phrase. Ordinary start without options uses the
existing defaults; explicit workstream options select policy and source: Literal Git names retain their exact
spelling, case, prefixes, and Unicode form. An ordinary ASCII space selects phrase
conversion: Unicode words are normalized, lowercased, and joined with hyphens.
The input determines only the branch name; no prefix or numbered suffix is added.

```sh
gof start "nasty deployment bug"    # nasty-deployment-bug
gof s Fix/DeploymentBug            # exact literal name
gof start fix/parser_v2
```

| Option | Effect |
| --- | --- |
| `--policy <ordinary\|release\|hotfix>` | Establish explicit workstream policy |
| `--from <reference>` | Select an exact creation commit |
| `--from-tag <tag>` | Select an exact version tag for hotfix creation |
| `--target <branch>` | Select preparation and integration target for ordinary contributions |

See [direct releases and hotfixes](#direct-releases-and-hotfixes) for policy,
source selection, publication, and completion.

Names must be valid for Git, begin with a character other than `-`, and differ
from `HEAD` and the configured base branch. Revision expressions such as
`main~1` and `@{-1}` are not branch selectors.

With the ordinary default source, start chooses these actions:

| Target | Action |
| --- | --- |
| Already current | Report success and keep the checkout, including uncommitted work |
| Exists locally | Switch using the branch's current local history; works offline |
| Exists on the publication remote, with no local branch | Fetch the exact branch and create a tracking local branch |
| Absent locally and on the publication remote | Create from the freshly fetched project base on the base remote |

Remote-dependent actions stop if discovery or fetching fails. A same-named branch
on the base remote alone does not select remote resume. New work begins without
upstream tracking. Existing valid tracking is preserved. Start refreshes the
selected remote-tracking reference; local base history and the fork base retain
their history.

Start requires an attached branch with at least one commit. Switching requires
a clean index, working tree, and submodules. Git also refuses a switch that would
overwrite ignored files. Active Git operations or a target checked out in another
worktree block start, including an already-current target. See
[tracking and routing](#tracking-and-git-routing) for accepted branch settings.

### Save progress

```text
gof save [ACTION] [--message <message>]
gof v [ACTION] [--message <message>]
```

A clean tree publishes existing commits directly. With uncommitted work, the
default `save-action = ask` lets you choose what to preserve.

| Option | Action | Work retained locally |
| --- | --- | --- |
| `--publish-only` | Publish existing commits | All uncommitted changes |
| `--staged` | Commit the index selection with your message, then publish | Unstaged changes and untracked files |
| `--wip` | Commit staged and remaining work separately, then publish | Ignored untracked files |
| `--stash` | Stash tracked and nonignored untracked work, then publish existing commits | The stash and ignored untracked files |
| `--message <message>` | Supply the staged commit message | Applies only to the staged action |

Choose one action flag; it overrides `save-action`. `--message` also works with
a configured staged preference. Quote it as one argument. Save does not accept
`-m`. A clean tree needs no checkpoint message.

```sh
gof save
gof v --publish-only
gof save --staged --message "selected parser changes"
gof save --wip
gof save --stash
```

Staged save respects partial staging, including selected portions of one file.
Git's normal message cleanup, hooks, and signing settings apply. WIP creates
`[WIP - Staged]` and `[WIP - Unstaged]` commits for the groups containing changes;
the second group includes nonignored untracked files. A stash stays local and
is left stashed. Save reports its object ID for [recovery](#restore-a-retained-stash).

Save publishes the current branch to its exact name at the publication remote's
push destinations. It creates or fast-forwards the branch; an unchanged remote
tip is successful too. It performs no fetch and needs no base-remote access.
Divergent or rewritten remote history requires Git history repair before save
can publish it. Use finish when you want its supported preparation and guarded
rewrite workflow.

The branch must have an existing commit and differ from the configured base.
Conflicts, active Git operations, detached HEAD, and dirty submodule contents
block save. Preserve dirty submodule work within its own repository. Clean
submodule commit-pointer changes can be saved in the parent repository.

### Finish work

```text
gof finish [OPTIONS]
gof f [OPTIONS]
```

For ordinary work, review mode prepares the current clean branch against `base-remote`, runs the
configured checks, and publishes through `publish-remote`. Completion identifies
the branch, each destination, the pinned commit, and the project base for manual
submission. It keeps the working branch checked out.

| Option | Effect |
| --- | --- |
| `--mode <review\|integrate>` | Publish work for review or integrate it into the project base |
| `--update <rebase\|merge\|none>` | Override preparation against the observed remote base |
| `--allow-rewrite` | Permit eligible replacement of published branch history |
| `--no-rewrite` | Deny replacement; mutually exclusive with `--allow-rewrite` |
| `--skip-checks` | Bypass configured checks for this invocation |
| `--show-credentials` | Display HTTP(S) URL credentials for this invocation |

```sh
gof finish --mode review --update merge --no-rewrite
gof f --mode review --update rebase --allow-rewrite
gof finish --mode review --update none --no-rewrite
```

The default mode `ask` prompts before remote refresh. Review requires a clean
index, working tree, and submodules, an attached branch with an existing commit,
and no conflicts or active Git operation. The branch must differ from the
configured base. Commit or preserve uncommitted work first. Clean branches with
WIP commits are eligible.

Finish observes the base and the exact working branch at every publication push
destination, then fetches the observed objects. Read access to each push destination
is required. An unavailable project base stops finish even with `--update none`.
Refresh leaves local base history and remote-tracking references unchanged; it
prepares against the project base even when a fork base is stale or divergent.

| Update policy | Preparation when the branch needs the remote base |
| --- | --- |
| `ask` | Choose rebase, merge, none, or cancellation at a terminal |
| `rebase` | Replay branch commits onto the observed base, preserving merge structure |
| `merge` | Incorporate the observed base with a merge or fast-forward |
| `none` | Publish current history without updating from the base |

A branch already including the base needs no update choice. Unrelated base history
requires `none` or Git history repair; uncertain or unavailable history stops finish.

In review mode, selecting rebase resolves only the update choice. If published
history may need replacement, `finish-rewrite = ask` requests separate authorization.
`--allow-rewrite` or configured `allow` supplies permission; configured `allow`
is standing authorization for eligible replacement. Denial stops required
replacement. Permission alone cannot overwrite remote work missing from the local
history: finish must verify incorporation of the observed published work and
protect each exact destination against movement. Merge and none retain incorporated
published history and need no replacement authorization.

The sequence is refresh → prepare → checks → revalidate → publish. Checks run
even when preparation needs no history update, and every retry runs the effective
list again. `--skip-checks` bypasses that list without an additional confirmation;
Git hooks, revalidation, and publication safeguards remain active. See
[finish checks](#finish-checks) for configuring commands and handling failures.

Resolved flags or settings require no additional confirmation. Only relevant
unresolved choices prompt. Review’s required choices resolve before branch changes.
Review cancellation or unresolved input leaves branch history, the index, and
working-tree content unchanged by finish; completed remote observations and fetches remain and
are reported. Changed local or remote context stops execution.

#### Integrate into the project base

Integrate mode delivers the finished result to the base branch on `base-remote`,
then completes the selected checkout and work-branch cleanup. It runs candidate
checks before pending delivery and leaves the result available after failure.

```sh
gof finish --mode integrate --integration ff-only --cleanup keep
gof f --mode integrate --update merge --integration merge --message "Integrate parser fix" --cleanup local
gof finish --mode integrate --integration squash --message "Fix parser" --cleanup both
```

| Option | Effect |
| --- | --- |
| `--integration <ff-only\|merge\|squash>` | Fast-forward only, create a merge commit, or create a squash commit |
| `-m`, `--message <text>` | Supply literal nonblank final integration message text, including multiline text |
| `--checkout <target\|work>` | Select the final checkout; default is target |
| `--cleanup <keep\|local\|both>` | Retain work, delete local work, or delete local and published work |

These options do not imply a mode choice. Ordinary review rejects integration,
message, checkout, and cleanup options; its integration settings remain inactive.
Tagged hosted review accepts only merge guidance, work checkout, and keep cleanup;
final integration messages require integrate mode. All supplied settings are
validated.

Work preparation and target integration are independent. `--update merge`
incorporates the observed target into work; `--integration merge` creates the final
merge commit on the target. Fast-forward-only requires target ancestry and stops
when a fast-forward is impossible. Work already containing the target needs no
update choice; work already contained in the target needs no integration method
or message. Unrelated or unverifiable history blocks direct integration.

`finish-message = auto` uses `Integrate <source> into <target>`. The default ask
policy offers an editable suggestion through your Git editor when a new merge or
squash commit needs a message. Supply `-m` to avoid editing. Fast-forward,
already-integrated work, and reuse of a retained result need no editor. Editor
failure, cancellation, invalid output, or a blank message stops the pending stage.
Message text remains literal and Git hooks, cleanup, and signing apply. A message
may first become necessary after preparation; stopping then retains prepared work.

Checkout and deletion follow verified delivery to every target destination.
`--checkout work` requires `--cleanup keep`. Local cleanup deletes the verified
local work branch; both also deletes its exact observed published version at every
`publish-remote` push destination. The published version must be contained in the
verified source. New work, another worktree’s checkout, or changed destinations
blocks deletion. Target delivery and published-work cleanup use their independent
remote roles.

Ordinary integration publishes the target result and preserves work tracking.
Release and hotfix completion additionally deliver their explicitly named version tag.
Working-history replacement permission supplies no target-publication or cleanup
authority. A successful local integration is only a candidate: checks, target
publication, final checkout, and selected cleanup must all complete for success.
A command error remains a failure even when Git completed its effect.

Only relevant unresolved choices prompt. Settings and explicit flags need no
additional confirmation. Prompts and editing bind the reviewed context; changed
refs, settings, endpoints, or recovery evidence stop the affected operation.
See [integration recovery](#recover-direct-integration) for retained-result retries.

### Direct releases and hotfixes

A workstream has an explicit policy: `ordinary`, `release`, or `hotfix`. Its name
is independent of that policy. A branch named `release/spring` can carry ordinary
contribution policy; a branch named `stabilize` can carry release policy.
Creation, preparation, integration, work publication, version tagging, and release
marker delivery have separate roles. Inspect the effective routes with
`gof config show` before delivery.

Select an editable direct OneFlow configuration with setup:

```sh
gof config --oneflow --yes
```

The bundle writes ordinary, release, and hotfix settings into the shared file.
Edit those values for your project. Equivalent custom settings use the same
workstream mapping; execution does not depend on a strategy name. Selecting this
bundle is explicit; ordinary built-in defaults remain unchanged.

Start a release at the development commit you intend to ship, including an
older commit. Start a hotfix at an explicitly selected version tag:

```sh
gof start release/spring --policy release --from <development-commit>
# Stabilize and commit the release.
gof save
gof finish --tag spring --tag-message "Spring release"

gof start hotfix/spring-fix --policy hotfix --from-tag spring
# Fix and commit the released version.
gof save
gof finish --tag spring-fix
```

Version names are literal Git tag names and need not follow Semantic Versioning.
`--from-tag` accepts an exact lightweight or annotated tag pointing to a commit;
new hotfix creation never selects a latest tag or substitutes development history. A configured
hotfix `creation-source` must be an explicit full `refs/tags/<name>` reference;
`--from-tag` overrides that selection. Explicit
creation sources resolve to exact locally available commits. Full commit IDs,
Git commit expressions, and full refs can select a commit; ambiguous short names
and noncommit objects fail. Local branches and tracking refs reflect local
knowledge, so fetch the intended source with Git before selecting it when remote
freshness matters. The ordinary default creation path fetches the configured
remote base. Selecting an explicit source still requires publication-remote
discovery to distinguish new creation from published resume. Creation does not
publish work.
Local and published work resume without resetting history. A new checkout can
resume published hotfix work with explicit policy and no creation source; a tag
selection is required when creating new hotfix work. An explicit source
incompatible with the recorded creation request stops resume.

Ordinary contributions can target a release or hotfix branch:

```sh
gof start fix-release-test --policy ordinary --target release/spring
gof save
gof finish --mode integrate --integration merge --message "Fix release test" --cleanup both
```

New contribution work starts at the freshly fetched target unless `--from`
selects a locally available creation commit. It prepares against and integrates
into the selected target.
It creates no version tag and advances no release marker. A release can publish
its work to the project while an ordinary contribution publishes to a fork.

| Finish option | Effect |
| --- | --- |
| `--tag <name>` | Name the version tag for release/hotfix completion |
| `--tag-message <text>` | Create an annotated tag with literal message text |
| `--sign-tag` | Request signing; signing failure stops completion |

Tag and integration messages have separate meanings. Git signing preferences
apply to tag creation. A pre-existing local or remote tag is a collision unless
it is corroborated as the exact tag from this retained attempt. For annotated
tags this includes the tag object, not just the peeled commit. Finish never
replaces a version tag.

Release completion checks and tags the approved source commit, then integrates
its history into the target without importing newer development work into the
released source. Squash and source rewriting are incompatible with this delivery.
The tag remains at the released commit through merge conflicts and retries.
Tagged completion requires original Git objects and stops while replacement
refs are present; restore the original source tree before validation.
Checks validate the released source and the integration candidate. A candidate
identical to the checked source needs no duplicate check. Retries can reuse
successful checks for the exact unchanged commits while the retained settings
and repository context remain verified. A check failure or a check changing
guarded state stops remaining delivery. `--skip-checks` bypasses configured checks
for that invocation; a later retry without it must validate any bypassed tree
before completing pending delivery.

Hotfix routing requires declared active-release context: a specific release
branch or explicit `none`. The developer supplies and maintains that declaration.
Finish does not infer an active release from prefixes, branch enumeration, tag
dates, or version ordering. With an active release, the hotfix integrates there;
otherwise it integrates into development. Completing the release later carries
the fix into development. Invalid or changed declared context stops routing.

An enabled release marker is a distinct branch pointing at the released tag's
commit, even when the integration result includes newer development. Finish
initializes an absent marker and advances an existing marker only by
fast-forward. Divergence and aliases with work or integration target block it.
A disabled marker has no effect. The default final checkout is the integration
target.

Tag, target, and enabled marker must all be verified at their required publication
destinations before checkout or cleanup. Only the selected tag and branch refs
are delivered. Partial effects remain available after failure; retry confirms
completed destinations and completes pending effects. A failing Git command
still returns failure when its effect occurred.

Resolve merge conflicts with native Git commands, then rerun the same finish
request. Finish retains the released source, exact tag identity, integration
result, routes, and resolved choices across fresh processes, including after
local source deletion. A removed established local work branch is reserved for
its retained completion; start stops before recreating it with an old association.
Use a distinct branch for new work or resolve the old association explicitly.
Conflicting retry input or changed refs, settings,
endpoints, or associations stop reuse. Inspect the reported completed effects,
remaining work, and next action; do not delete retention evidence during recovery.
After local source deletion, recovery verifies existing delivery and completes
remaining cleanup; it cannot publish missing release refs from an absent source.
Hosted release/hotfix merge-back and post-review cleanup remain outside direct
completion.

### Hosted releases and hotfixes

Hosted finish publishes the approved version and a distinct merge-back branch,
then reports readiness for manual submission. It retains both branches. The
project target and optional release marker remain for later manual integration.

```sh
gof config --oneflow-hosted --yes
# Edit the generated project and review routes for your repository.
gof start release/spring --policy release --from <development-commit>
# Stabilize and commit the release.
gof save
gof finish --tag spring --merge-back merge-back/spring

gof start hotfix/spring-fix --policy hotfix --from-tag spring
# Fix and commit the released version.
gof save
gof finish --tag spring-fix --merge-back merge-back/spring-fix
```

The editable hosted bundle uses review mode for releases and hotfixes. Their
`publish-remote` receives original work; `base-remote` receives the selected
version tag; `review-remote` receives the merge-back branch. The bundle selects
the project remote for original publication and the ordinary publication remote
for review. Remotes may coincide, and their names confer no role. Equivalent
custom templates select the same behavior with `version-tagging = "required"`
and `finish-mode = "review"`.

Supply `--merge-back <branch>` or configure `merge-back-branch` in the selected
template. An unresolved tag, merge-back name, or relevant preparation choice asks
at a terminal. The merge-back branch must differ from the original source,
project target, and optional marker. Existing branches and tags are collisions
unless private evidence corroborates them as effects of this same attempt.

Hosted finish requires complete original Git history; shallow history, replacement
refs, and grafts must be resolved before preparation. Finish checks the exact
released source before creating and delivering its tag.
The original published branch must match that version. The merge-back branch
starts at the released commit. Hosted preparation uses the selected review target;
`preparation-source` supplies ordinary work preparation. `--update merge` incorporates the pinned target
into that separate branch; `--update none` retains the released history unchanged.
Rebase is incompatible with preserving that history for merge-back. Checks also
validate the prepared merge-back candidate; an identical already-checked commit
needs no duplicate check. Annotation and signing use the tag options described
above, without unsigned fallback. A check bypass remains explicit and a later
unbypassed retry must validate bypassed trees before pending delivery.

Hosted policy uses `finish-integration = "merge"` to express the required no-ff
merge-back guidance, `finish-checkout = "work"`, `finish-cleanup = "keep"`, and
`finish-rewrite = "deny"`. Direct integration and cleanup options do not perform
later review work. Ordinary contributions can still review into a release or
hotfix with `--policy ordinary --target <branch>`; they adopt no version-tag or
marker effects and retain their own integration preferences.

Successful output names the version tag and commit, original source, published
merge-back source, review repository, and target. Submit that merge-back branch
into the named project target through your chosen interface and request a
non-fast-forward merge (`--no-ff`). Resolve later review conflicts on the
merge-back branch, preserving the tagged version. Remove temporary branches only
after the appropriate integration. If configured, advance the release marker
manually by fast-forward to the released tag after integration; do not point it
at a newer merge-back result.

Hotfix review targets the explicitly declared active release when present, or
development when `active-release = "none"`. Keep that declaration current before
starting work. Changed or invalid context stops affected operations.

The separate merge-back source and no-ff guidance follow
[Ruka's hosted OneFlow description](https://www.endoflineblog.com/implementing-oneflow-on-github-bitbucket-and-gitlab).
This is the selected workflow, not a claim about every project's review practice.
Finish creates no PR, invokes no provider tool, and observes no submission status.
Its attempt ends at verified requested publication. Submission, approval,
integration, deployment, branch cleanup, and marker advancement are external
follow-up; finish does not monitor or complete them. See
[hosted preparation recovery](#recover-hosted-preparation) for interrupted work
before readiness.

### Inspect state

```sh
gof status
```

Status reports HEAD/branch, staged and unstaged path counts, conflicts, untracked
paths, submodule changes, active operations, and upstream information. Bare
repositories are identified as having no working tree.

Counts can overlap when a path has staged and unstaged changes. Ignored paths are
excluded. Upstream comparisons use locally available references and history;
unavailable comparisons are identified explicitly. Status is a read-only local
observation. Its text is intended for people.

### Inspect or create configuration

```text
gof config show
gof config [--format <toml|yaml|json>]
gof config --yes [--format <toml|yaml|json>]
gof config --oneflow [--yes] [--marker <branch>] [--format <toml|yaml|json>]
gof config --oneflow-hosted [--yes] [--marker <branch>] [--format <toml|yaml|json>]
```

`config show` displays effective settings and each value's shared file, local
file, or built-in default. It identifies an inherited base remote's publication
source, discovered files, and searched directories. Check commands are displayed
as argument lists without execution or executable availability checks. Inspection
is local, read-only, and intended for people.

`config` guides you through remote roles, the base branch, save and finish
policies, and optional finish checks. Press Enter to keep a suggested default.
Remote names and branch references provide offline hints; they assign no roles
and do not change the built-in defaults. Valid names can be configured before
the remote or branch exists.

At the base-source prompt, Enter or the same remote name keeps the base following
publication; a different name writes an explicit `base-remote`. Choosing review
omits integration-specific questions and settings. Other mode choices ask those
policies; fast-forward-only omits message policy.

For each check, enter a display name and a JSON executable-and-argument array:

```text
Finish check name [Enter to finish; default: no checks]: Tests
Command as JSON executable-and-argument array: ["cargo", "test", "--locked"]
Finish check name [Enter to finish]:
```

A blank name finishes the list. Setup records the literal commands without running
them or detecting project tools. It shows the destination, proposed contents,
ordered checks, and effective values after private overrides. Private values stay
local. Enter `yes` to create the file. Enter `no`, leave the confirmation blank,
type `cancel`, or end input to stop without creating it. Invalid answers can be
corrected. Changed review context stops creation; inspect it and rerun setup.

`--yes` (also `-y`) creates built-in shared defaults without prompts. It writes
`publish-remote = origin`, omits `base-remote` to keep the derived relationship,
retains unresolved `ask` policies, and writes an empty check list. It reports
private file locations without parsing or copying their contents. Run
`config show` afterward to validate private overrides and inspect effective values.

```sh
gof config --yes
gof config -y --format yaml
gof config --yes --format json
```

The default format is TOML. Generation writes `git-oneflow.config.toml`,
`git-oneflow.config.yaml`, or `git-oneflow.config.json` at the current working-tree
root. It works offline with uncommitted work, detached HEAD, or an unborn branch.
Existing work, the index, refs, and Git configuration stay unchanged. The resulting
file is ready to edit and commit using Git.

Every recognized shared entry prevents generation, including another supported
extension, an invalid or empty file, a directory, or a dangling symlink. Inspect or
edit the existing file and keep one recognized file per layer. Guided setup
validates private files before review; repair invalid private files before retrying.
Generation requires a working tree; bare repositories support inspection only.

## Repository settings

Commit shared settings at the Git working-tree root. Keep private overrides in
the Git common directory, usually `.git`. Resolve these directories with
`git rev-parse --show-toplevel` and `git rev-parse --git-common-dir`.
Each layer accepts one of:

- `git-oneflow.config.toml`
- `git-oneflow.config.yaml`
- `git-oneflow.config.yml`
- `git-oneflow.config.json`

Extensions have equal priority. Multiple recognized files in one layer fail with
a diagnostic listing the conflicting paths. Layers may use different formats.
Nested commands discover the root file; parent and home directories supply no
additional settings. Linked worktrees have their own shared root files and the
same common-directory private overrides. Bare inspection uses local settings and
defaults.

Values resolve through **explicit command flags > local overrides > shared
settings > defaults**. Explicit start policy and source choices apply to the selected workstream. Omitted keys inherit;
an empty TOML document or empty YAML/JSON mapping (`{}`) inherits all keys.
`config show` identifies the effective values and their provenance.

| Setting | Values or role | Default |
| --- | --- | --- |
| `publish-remote` | Named Git remote receiving working branches | `origin` |
| `base-remote` | Named Git remote supplying the base branch | Follows publication |
| `base` | Project base branch name | `main` |
| `save-action` | `ask`, `publish-only`, `staged`, `wip`, `stash` | `ask` |
| `finish-mode` | `ask`, `review`, `integrate` | `ask` |
| `finish-update` | `ask`, `rebase`, `merge`, `none` | `ask` |
| `finish-rewrite` | `ask`, `allow`, `deny` | `ask` |
| `finish-checks` | Ordered list of check objects | `[]` |
| `finish-integration` | `ask`, `ff-only`, `merge`, `squash`; hosted tagged review requires `merge` | `ask` |
| `finish-message` | `ask`, `auto`; inactive in review | `ask` |
| `finish-checkout` | `target`, `work`; hosted tagged review requires `work` | `target` |
| `finish-cleanup` | `ask`, `keep`, `local`, `both`; hosted tagged review requires `keep` | `ask` |
| `redact-credentials` | Private local-only boolean for finish output | `true` |

Use a mapping of case-sensitive keys. Remote names, branch names, and policies
are strings. These equivalent examples select review delivery with a shared
repository remote:

```toml
publish-remote = "team"
base = "trunk"
finish-mode = "review"
finish-update = "merge"
finish-rewrite = "deny"
```

```yaml
publish-remote: "team"
base: "trunk"
finish-mode: "review"
finish-update: "merge"
finish-rewrite: "deny"
```

```json
{
  "publish-remote": "team",
  "base": "trunk",
  "finish-mode": "review",
  "finish-update": "merge",
  "finish-rewrite": "deny"
}
```

Every present file must be valid UTF-8. Unknown or duplicate keys, malformed
syntax, invalid values or types, and unreadable files fail, including inactive
or overridden values. Quote YAML values that could be interpreted as another type,
such as `"true"` or `"123"`. YAML merge keys and unsupported tags are rejected.
Validation reads configuration as data; the workflow checks remote availability
when needed.

### Workstream policy and routes

The optional `workstreams` mapping contains `ordinary`, `release`, and `hotfix`
policy templates. Each template is sparse; private values overlay shared values
per key. All supplied values are validated even when inactive or overridden.
Policy is selected explicitly with start; a template name is data, not code.

| Template setting | Role |
| --- | --- |
| `creation-source` | Reference used to create new work |
| `preparation-source` | Branch supplying work preparation |
| `integration-target` | Branch receiving direct integration or manual merge-back review |
| `publish-remote` | Work-branch publication and remote-resume destination |
| `base-remote` | Project source and version/direct-delivery remote |
| `review-remote` | Hosted merge-back publication remote; defaults to the effective work publication remote |
| `merge-back-branch` | Exact hosted merge-back branch name; finish can supply `--merge-back` |
| `version-tagging` | `required` or `disabled` |
| `release-marker` | Marker branch name, or `none` |
| `active-release` | Explicit active release branch, or `none`, for hotfix routing |
| `finish-mode`, `finish-update`, `finish-integration`, `finish-message`, `finish-rewrite`, `finish-checkout`, `finish-cleanup` | Policy-specific finish choices, using the values above |

`gof config --oneflow --yes` writes a direct OneFlow bundle. Add
`--marker stable` to enable a marker. Guided `gof config --oneflow` reviews the
proposed bundle. It writes editable ordinary/release/hotfix templates rather
than a strategy selector. The top-level base remains the development branch.
Ordinary work can publish to a fork while releases and hotfixes use the project
remote. For example, these custom values select the same direct release behavior
without choosing a bundle:

```toml
publish-remote = "personal"
base-remote = "project"
base = "main"

[workstreams.release]
creation-source = "main"
preparation-source = "main"
integration-target = "main"
publish-remote = "project"
base-remote = "project"
version-tagging = "required"
release-marker = "stable"
finish-mode = "integrate"
finish-update = "none"
finish-integration = "merge"
finish-message = "auto"
finish-rewrite = "deny"
finish-checkout = "target"
finish-cleanup = "both"
```

The corresponding YAML/JSON structures are nested mappings. A private template
can override only `workstreams.hotfix.active-release`, for example; it need not
repeat other settings. To route a hotfix through an active release, set that value
to the exact branch name before start. Keep the declaration current; a changed
route during retained completion cannot inherit the earlier authority.

Start retains the established policy for a branch. Resume uses that association
and rejects incompatible explicit policy, source, or target input. Changed policy
settings and recreated branches require resolution before dependent effects.
`config show` displays effective routes and their sources, plus the established
creation commit and input. The configured creation route is the default source;
the retained creation commit identifies where this work actually began. Policy storage remains
local; a new checkout must supply explicit policy for remote work.

### Remote roles and fork workflows

`publish-remote` selects where working branches publish and where remote resume
looks for them. `base-remote` selects the project base for new work and finish
preparation. Remote names such as `origin`, `upstream`, and `fork` have no special
meaning. Configure named remotes using Git before remote-dependent operations.

Each role resolves independently. When `base-remote` is absent in both layers,
it follows the final publication selection. An explicit shared base remote stays
selected when a private override changes only publication.

| Settings | Publication remote | Base remote |
| --- | --- | --- |
| No remote settings | `origin` | `origin` |
| `publish-remote = personal` | `personal` | `personal` |
| `publish-remote = personal`, `base-remote = project` | `personal` | `project` |

For a fork, a configuration can publish to your repository while preparing against
the project's base:

```toml
publish-remote = "personal"
base-remote = "project"
base = "main"
finish-mode = "review"
```

New work starts from the fresh project base. Local work resumes locally;
published work without a local branch resumes from `personal`. Save publishes
there. Finish prepares against `project/main`, publishes to `personal`, and reports:

```text
Next: submit personal/work/parser-fix for review into project/main.
```

Neither workflow synchronizes the local base or the fork's base. Fork topology
and branch naming remain independent choices.

### Tracking and Git routing

Start, save, and finish accept absent upstream tracking or exactly one of these
relationships:

- The current branch's exact counterpart on `publish-remote`.
- The configured `base` branch on `base-remote`.

New branches initially have no tracking; remote resume establishes publication
tracking. Save and finish establish missing publication tracking only after all
push destinations confirm publication. Existing valid tracking is preserved.
An upstream comparison may remain unavailable if Git's fetch mappings exclude
the tracked branch.

For example, after fetching a project-base reference with Git, you can select it
as the upstream:

```sh
git branch --set-upstream-to=project/main work/parser-fix
```

Incomplete, duplicate, empty, local `.` or unrelated remote/merge settings require
repair. An explicit effective Git push remote must equal `publish-remote`:
`branch.<name>.pushRemote` takes precedence over `remote.pushDefault`, and each
uses its last configured value. Conflicting overrides stop the command.

Gof preserves user-owned routing settings and scopes publication to the exact
working branch regardless of expansive push defaults. Ordinary Git commands
continue to use your Git configuration. With separate publication and project-base
tracking, configure Git's push routing for your own ordinary Git usage.

### Finish checks

`finish-checks` is an ordered list of objects containing exactly `name` and
`command`. A name is a nonblank display label. A command is a nonempty string
list whose first element is a nonempty executable name or path; remaining elements
are individual arguments. Spaces, quotes, Unicode, and empty argument strings
are preserved. Command elements cannot contain NUL.

```toml
[[finish-checks]]
name = "Tests"
command = ["cargo", "test", "--locked"]

[[finish-checks]]
name = "Custom validation"
command = ["sh", "-c", "npm run lint && npm run check"]
```

```yaml
finish-checks:
  - name: "Tests"
    command: ["cargo", "test", "--locked"]
  - name: "Custom validation"
    command: ["sh", "-c", "npm run lint && npm run check"]
```

```json
{
  "finish-checks": [
    {"name": "Tests", "command": ["cargo", "test", "--locked"]},
    {"name": "Custom validation", "command": ["sh", "-c", "npm run lint && npm run check"]}
  ]
}
```

Commands use exact executable-and-argument lists. Shell features require an
explicit shell, as in the second check; there is no implicit shell interpretation,
interpolation, or splitting. Setup and inspection never run the commands or probe
executables. Start and save never execute finish checks.

An omitted private list inherits the shared list. A supplied private list replaces
the whole shared list; `[]` disables inherited checks. Every supplied list is
validated. `config show` reports the effective ordered list and its source.

Checks run sequentially from the discovered working-tree root with your environment
and PATH. They receive EOF on stdin. Their stdout and stderr appear as they run,
along with each check's name and command. A failed exit, launch error, or interruption
stops publication and later checks. Ctrl-C stops the running check and its ordinary
child processes. Check commands run as project code with your permissions.

Preparation and check effects stay in place. Fix the problem, commit or preserve
any generated changes, and rerun finish. Review reruns all checks. Ordinary direct
integration reruns checks while target delivery remains pending and skips them
after every target confirms delivery. Tagged completion can reuse successful
checks for verified unchanged source and candidate commits; a bypassed tree needs
validation on a later retry without `--skip-checks` before pending delivery completes.
Checks that change the branch, committed work, index, working tree, effective
settings, or routing stop the attempt before another check or publication.
Ignored build outputs are permitted. Remote movement during checks can still stop
publication.

### Credential display

Finish redacts credentials in HTTP(S) URLs by default across its prompts, completion,
failures, displayed Git diagnostics, and check output. Your message editor receives
the terminal directly and controls its own display. Redaction filters URL credentials
in finish-rendered output; it does not filter arbitrary secrets.

Set `redact-credentials = false` in the private common-directory file or use
`gof finish --show-credentials` for one invocation. The flag takes precedence.
Shared files reject this key and setup never copies it to a shared file. Disabling
redaction exposes credentials in terminal output and captured logs. Display
settings do not change transport URLs or executed check arguments.

## Guarantees and practical limits

Gof rechecks repository context, approved work, settings, and routing before
affected operations. Changed context stops execution so you can inspect and retry
against the current state. These checks are observations rather than locks against
other Git processes, editors, hooks, or helpers.

Publication pins the intended commit and exact branch. Save uses ordinary
non-forced publication; finish also guards each destination with an exact lease
and requires verified incorporation evidence plus permission for review replacements.
Direct integration only advances the target through verified ancestry.
Gof does not publish unrelated branches, tags, or submodule branches. Normal Git
hooks, signing, credential helpers, and transport settings remain active.

Multi-step workflows and publication to multiple destinations are non-atomic.
A failed command may have completed a local commit, stash, checkout, or remote
update. Reports distinguish confirmed effects, rejection, uncertainty, tracking
errors, and observed remaining state. Gof retains completed effects rather than
automatically rolling them back. Acknowledgement proves publication at that time;
another writer may subsequently move the remote ref.

Save and finish refuse fetch mappings that could let Git update local branch
history during push: positive mappings with local destinations must point under
`refs/remotes/`, and matching symbolic tracking references are refused. Negative
and source-only mappings remain supported. Legacy remote definitions without
configured URL values are also refused. Start refuses symbolic fetch destinations.
Repair the reported Git configuration explicitly; gof does not rewrite it for you.

Git credentials must be available through the configured environment and helpers;
workflow subprocesses disable terminal credential prompts. Observation and publication
can fail when history or objects are unavailable. Arbitrary hook/helper effects,
operating-system interruption, and concurrent changes can limit the remaining-state
report. Inspect actual Git state when a command ends without a report.

## Automation

Use explicit flags or repository settings to resolve required choices:

```sh
gof save --staged --message "selected changes"
gof save --wip
gof finish --mode review --update rebase --allow-rewrite
gof finish --mode review --update merge --no-rewrite
gof finish --mode integrate --integration squash --message "Fix parser" --cleanup local
gof config --yes --format json
```

Prompting requires terminal stdin and stderr and a complete submitted line.
Piped responses cannot supply interactive approval. Unresolved save input stops
before local effects. Unresolved update input stops before branch changes but can follow reported remote
observations and fetches. Integration method/message input can first become necessary
after work preparation; cancellation retains that prepared work. Guided configuration
requires a terminal; `--yes` authorizes prompt-free default generation.

For save and finish, enter a displayed choice or `cancel`; invalid choices and
blank required messages stop with retry guidance. Resolved selections add no
confirmation. Finish has no blanket `--yes` or dry-run option. Duplicate options,
unsupported values, and positional finish/save arguments are syntax errors.

Status, configuration inspection, and completion text are intended for people.
Use exit codes for success handling and Git commands for structured repository
queries.

| Code | Meaning |
| --- | --- |
| `0` | Command completed, including unchanged publication; config cancellation also succeeds |
| `1` | Operational failure, unresolved input, save/finish cancellation, or incomplete execution |
| `2` | Invalid command syntax |

Save and review finish success includes valid upstream tracking after confirmed publication.
Integration success includes verified delivery and selected checkout/cleanup.
A remote update can be confirmed even when a later tracking error makes the command
fail. Status successfully reports dirty, conflicted, detached, unborn, and bare
states. Operating-system interruption is unsuccessful and may prevent a final report.

## Troubleshooting and recovery

Start with the failure's completed effects and remaining-state report. Inspect
local state and effective settings:

```sh
git status
gof config show
git log -5 --oneline
git stash list
git remote -v
```

### Readiness, settings, and access

| Reported problem | Next step |
| --- | --- |
| Git unavailable | Install Git 2.45 or newer and make it available on PATH |
| Start cannot switch | Commit or preserve changes; complete or abort the reported Git operation |
| Target occupied by another worktree | Continue in the reported worktree or release its checkout with Git |
| Detached or unborn HEAD | Attach a branch or create an initial commit before start/save/finish |
| Dirty submodule contents | Preserve or commit work inside the submodule repository |
| Invalid or conflicting configuration | Repair the reported file; keep one recognized file per layer |
| Configuration creation blocked | Inspect/edit the existing shared file; check destination directory access |
| Scope or context changed | Inspect branch, index, files, tracking, and settings; rerun against actual state |
| Remote or base unavailable | Check effective roles, named Git remotes, base existence, and destination access |
| Required input unavailable | Resolve relevant action/mode/update/integration/cleanup choices with flags or settings; supply a message when required |

To inspect endpoints using your actual remote and branch names:

```sh
git remote get-url --all project
git remote get-url --push --all personal
git ls-remote project refs/heads/main
git ls-remote <push-destination> refs/heads/work/parser-fix
```

Inspect every push destination after a partial or uncertain result. A local
remote-tracking cache does not prove the remote update succeeded or failed.

Configuration creation leaves the recognized file absent or complete. After
interruption, inspect and remove any leftover unrecognized
`.git-oneflow-config-*.tmp` file at the working-tree root before retrying.
Coordinate configuration writers: concurrently created recognized filenames can
produce multiple files, which the loader will report.

### Resume a failed save or publication

Completed commits, stashes, and remote updates remain available. Repair the
reported hook, transport, or configuration problem and retry from the actual
state. A clean retry publishes without making empty WIP checkpoints. If the
second WIP commit failed after staging, inspect the index: the remaining work
may now be staged, and a fresh WIP save uses that current selection.

An uncertain push requires remote inspection. Already published tips can succeed
as unchanged on retry. New or divergent remote work needs incorporation with Git;
save never forces it away. With multiple destinations, retain confirmed updates
and repair the outstanding destinations.

A confirmed publication can coexist with incomplete upstream setup. Inspect the
branch's raw settings, substituting its actual name:

```sh
git config --get-all branch.work/parser-fix.remote
git config --get-all branch.work/parser-fix.merge
git config --get-all branch.work/parser-fix.pushRemote
git config --get-all remote.pushDefault
```

Repair incomplete or duplicate values to exactly one supported tracking
relationship before retrying. `git branch --set-upstream-to` can establish the
intended relationship when its remote-tracking reference is available. Gof does
not silently replace an invalid existing upstream.

### Restore a retained stash

Use the stash object ID reported by save:

```sh
git stash list
git stash show <reported-id>
git stash apply --index <reported-id>
```

Resolve any conflicts reported by Git. Successful application retains the stash
entry; remove it separately with Git when you no longer need it.

### Recover review preparation or checks

For rebase or merge conflicts, resolve files, stage resolutions, and complete the
active Git operation:

```sh
git add <resolved-paths>
git rebase --continue   # when a rebase is active
# or: git merge --continue
```

To abandon that preparation, use `git rebase --abort` or `git merge --abort` as
appropriate. Then rerun finish. Finish verifies retained rebase evidence before
permitting eligible publication after recovery; it has no separate continue or
abort command.

For failed or interrupted checks, prepared history and check effects stay local.
Fix the reported check or executable problem, commit or preserve changed files,
and rerun finish. Review reruns every configured check; ordinary direct integration
reruns checks when target delivery remains pending. Tagged completion can reuse
verified successful checks of unchanged source and candidate commits.
`--skip-checks` is a per-invocation bypass, not successful validation for a later
retry without that flag.

### Retry review publication

The prepared branch stays local and confirmed destination updates remain. Repair
access or the reported problem and rerun finish. Fresh observations identify
already completed updates; verified rebase evidence can authorize outstanding
eligible replacements with current permission.

New remote work requires incorporation with Git. Missing, expired, or changed
recovery evidence cannot authorize replacement. Recovery binds the original remote
roles and destinations; changing them can require incorporating the new destination's
history with Git before retrying. Retain the repository's Git metadata and reflogs
while recovering a finish rebase. Do not bypass a failed verification by treating
`--allow-rewrite` as unconditional force permission.

### Recover hosted preparation

After repairing the reported problem, rerun `gof finish` in the same worktree.
Retained evidence supplies the approved tag, merge-back name, and preparation
choice when omitted. Explicit conflicting input, changed settings/endpoints,
recreated branches, changed tag objects, occupied worktrees, or missing evidence
block affected effects. Keep private Git metadata and reflogs during recovery.

Resolve preparation conflicts on the merge-back branch with native Git commands:
stage the resolution and use `git merge --continue`, or use `git merge --abort`
to abandon the pending merge. Preserve the original released branch and tag.
Rerun finish to verify the preparation transition and complete pending checks and
publication. A conflict-resolved candidate is merge-back work; it does not become
the released version.

Partial and uncertain source, tag, or review publication retain completed effects.
A retry freshly verifies exact destinations, skips confirmed publication, and
continues pending work without extra branches or retagging. Successful checks can
be reused only for corroborated unchanged commits and context. Git command errors
remain failures even when their effects occurred. Reports name completed work,
pending preparation/publication, and the next action.

Once readiness has been reported, continue review and integration using your own
tools. The retained preparation record supplies no authority to reconstruct PR
state, verify later integration, advance a marker, or remove branches.

### Recover direct integration

Rerun `gof finish --mode integrate` in the same worktree after repairing the
reported problem. The associated verified attempt supplies omitted choices and
an accepted message, including when settings specify ask. Explicit conflicting
choices or changed settings, Git configuration, endpoints, history, or association
block recovery. Preserve private Git metadata, retained refs, and reflogs.

A failed check or publication retains the candidate for reuse. Ordinary direct
integration reruns configured checks for pending target delivery. After every
target destination freshly confirms delivery, its retry completes remaining
checkout/cleanup without integrating, checking, or publishing the target again.

Tagged completion can reuse successful source and candidate checks for the exact
unchanged commits and verified retained context. A retry without `--skip-checks`
validates bypassed trees before completing pending delivery. Target delivery can
leave tag or marker delivery outstanding; those refs must also be verified before
final checkout and cleanup. Confirmed delivery is not repeated.

A fully completed attempt succeeds without duplicate effects. Recovery can continue
from target checkout after local source deletion; tagged recovery then verifies
existing delivery and completes remaining cleanup. Recreated or changed work
cannot inherit the old attempt's deletion authority.

Resolve merge/rebase conflicts with ordinary Git continuation or abort. A verified
merge commit with stale merge machinery can require `git merge --quit`; follow
the report to retain that commit. Squash has no `MERGE_HEAD`: use the exact
`GIT_REFLOG_ACTION` reported by finish when manually committing the resolved
squash. An unmarked manual commit cannot establish the retained result. To abandon
an incomplete squash, follow the report’s tracked-tree restoration and
`SQUASH_MSG` cleanup instructions. Finish has no dedicated continue/abort command.

Reports separate local preparation, checks, target delivery, checkout, and cleanup.
Partial or uncertain delivery retains per-destination outcomes. Inspect those
exact destinations, fix access or the reported problem, then retry. A publication
command error stops completion even if the update was confirmed; a later invocation
can verify delivery and finish remaining work. Completed effects remain in place.

### Unusual branch names

Literal `@` is a valid target name, but tested Git 2.47.3 refuses `git switch @`.
Choose another name or resolve that checkout with Git. Other literal names retain
their exact spelling; phrase conversion applies only when the input contains an
ordinary ASCII space.
