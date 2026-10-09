# User guide

Use `git-oneflow` or its equivalent executable `gof` from a Git working tree.
Both accept the same commands and options. Git 2.45 or newer must be on PATH.
The [development guide](DEVELOPMENT.md) explains how to build the executables;
use their full paths when they are not on PATH.

## Command reference

```sh
gof --help
gof --version
gof start --help
gof save --help
gof finish --help
gof status --help
gof config --help
```

Running with no arguments shows help. Help and version are available anywhere,
including without Git installed.

## Start or resume work

```sh
gof start "nasty deployment bug"
gof s fix/deployment-bug
```

Supply one branch name or quoted phrase. Valid literal Git names retain their
exact spelling: `feature`, `Fix/DeploymentBug`, and `fix/parser_v2` are examples.
An ordinary space selects phrase conversion: Unicode letters and numbers are
normalized, lowercased, and joined with hyphens. For example, `nasty deployment
bug` becomes `nasty-deployment-bug`. The input determines the branch name.

Branch prefixes are naming conventions. All names use the same start behaviour.
Names must be valid for Git, begin with a character other than `-`, and differ
from `HEAD` and the configured base branch. Existing exact names mean resume.

Start chooses among four actions:

| Target | Action |
| --- | --- |
| Already current | Report success and keep the current checkout |
| Exists locally | Switch to that branch using its current local history |
| Exists on the publication remote, with no local branch | Fetch it and create a tracking local branch |
| Absent locally and on the publication remote | Create it from the freshly fetched project base on the base remote |

Start reports the action taken. Remote-dependent actions stop if discovery or
fetching fails. Local resume works offline; new branches begin without upstream
tracking. Existing tracking can follow the published counterpart or the configured
project base and is preserved. The local base and fork base retain their history.
A same-named branch on the base remote alone does not select remote resume.

Start requires an attached branch with at least one commit. Switching requires
a clean index, working tree, and submodules. An already-current target succeeds
with uncommitted work. Active Git operations or a target checked out in another
worktree block start.

## Save progress

```sh
gof save
gof v --publish-only
gof save --staged --message "selected parser changes"
gof save --wip
gof save --stash
```

A clean tree publishes existing commits directly. With uncommitted work, the
default action is to ask what you want to preserve:

| Option | Action | Work retained locally |
| --- | --- | --- |
| `--publish-only` | Publish existing commits | All uncommitted changes |
| `--staged` | Commit the index selection with your message, then publish | Unstaged changes and untracked files |
| `--wip` | Commit staged and remaining work separately, then publish | Ignored untracked files |
| `--stash` | Stash tracked and nonignored untracked work, then publish existing commits | The stash and ignored untracked files |

Staged save respects partial staging, including selected portions of one file.
WIP creates `[WIP - Staged]` and `[WIP - Unstaged]` commits for the groups that
contain changes. The second group includes nonignored untracked files.

A stash stays in the local repository. Keep it there until you are ready to
recover the work using Git. Save reports its object ID; see [recovery](#recovery).

Choose one action flag. It overrides the repository's `save-action` preference.
`--message` applies to the staged action, including a configured staged preference.
Quote the message as one argument. Git's normal message cleanup, hooks, and signing
settings apply. A clean tree needs no checkpoint message.

### Interactive use

When the action is `ask`, save displays the affected scope and applicable action
names. Enter a displayed name or `cancel`. Staged is offered when staged changes
exist; a missing staged message is requested as one line.

Input and stderr must be terminals for prompting. Cancellation or end-of-input
before execution leaves the repository untouched. Invalid choices or blank
messages stop the command; rerun to choose again. Save rechecks the approved
context after input and stops if it changed.

### Scripts and agents

Select an explicit action for unattended execution:

```sh
gof save --wip
gof save --staged --message "selected changes"
```

Alternatively, set a repository preference such as `save-action = staged` and
supply `--message` on each invocation. Unresolved action or message input fails
before changes when a terminal is unavailable. Piped responses are not accepted
as interactive approval.

### Publication

Save publishes the current branch to its exact name on the publication remote,
using that remote's push destinations. It creates or fast-forwards the branch;
an unchanged remote tip is also successful. Rewritten or divergent remote history
requires attention before publication can succeed.

Tracking can be absent, follow the exact published counterpart, or follow the
configured project base. Existing valid tracking is preserved. Missing tracking
is established to the publication branch after all push destinations confirm
publication. Save needs no base-remote access and does not fetch. Incomplete,
duplicate, or unrelated tracking requires repair.

For start, save, and finish, an effective explicit Git push remote must equal
`publish-remote`: `branch.<name>.pushRemote` takes precedence over
`remote.pushDefault`, and each uses its last configured value. Conflicting
overrides stop the command; gof preserves user-owned Git routing settings.
Implicit push defaults and expansive mappings leave gof's publication scoped
to the exact current branch.

The branch must have an existing commit and differ from the configured base.
Conflicts, active Git operations, detached HEAD, and dirty submodules block save.
Preserve dirty submodule work within its own repository.

## Prepare and publish finished work

```sh
gof finish
gof f --update merge
gof finish --update rebase --allow-rewrite
gof finish --update none --no-rewrite
```

`finish` (shorthand: `f`) prepares the current clean branch against `base-remote`
and publishes it through `publish-remote`, then reports the branch, destinations,
and commit with a manual submission next step into the configured base remote
and branch. It runs configured validation checks after preparing the branch and
before publishing. External integrations, pull request creation, base integration,
tagging, and cleanup are deferred.

The sequence is refresh → prepare → configured checks → revalidate → publish.
Checks run even when the branch already includes the base. Each attempt runs the
whole effective list again, including retries after failed publication. An empty
list continues without a prompt. `--skip-checks` bypasses configured checks for
that attempt without confirmation and reports the bypass. Git hooks and all other
finish safeguards remain active.

Finish requires a clean index, working tree, and submodules, an attached branch
with an existing commit, and no active Git operation or conflicts. Commit or
preserve uncommitted work first. The branch must differ from the configured base.
Tracking can follow the published counterpart or the configured project base,
or be absent. Explicit push-selection validation follows the same rules as save;
finish establishes absent tracking after confirmed publication and preserves
existing project-base tracking.

Finish observes the base at `base-remote` and the exact branch at every
`publish-remote` push destination, then fetches their observed objects.
The roles can differ, with preparation using the project base even when the fork
base is stale or divergent. An unavailable project base stops finish, including
with `--update none`. The local base and tracking refs retain their history
during refresh. Read access to each push destination is required.

`--update` overrides `finish-update`:

| Value | Preparation when the branch needs the remote base |
| --- | --- |
| `ask` (configuration default) | Choose rebase, merge, none, or cancellation at a terminal |
| `rebase` | Replay branch commits onto the observed remote base, preserving merge structure |
| `merge` | Incorporate the observed remote base with a merge or fast-forward |
| `none` | Publish current history without updating from the base |

A branch already including the base needs no update choice. Unrelated base history
requires `none` or Git history repair; uncertain or unavailable history stops finish.

`--allow-rewrite` permits eligible replacement of published branch history;
`--no-rewrite` denies it. They override `finish-rewrite` and are mutually exclusive.
Selecting rebase resolves only the update choice. Rewrite `ask` requests separate
authorization when published history may need replacement. Configured `allow` is
standing authorization for eligible replacement; `deny` stops required replacement.
All replacement still requires verified incorporation of the observed remote work
and an exact destination lease. Permission alone cannot overwrite remote work
missing from the local history. Merge and none preserve incorporated published
history and need no replacement authorization.

Only relevant unresolved choices prompt. Input and stderr must be terminals; enter
a displayed choice or `cancel`. Cancellation, EOF, or unresolved unattended input
stops before branch execution, leaving branch history, the index, and working-tree
content unchanged by finish. Completed remote observations/fetches remain and are
reported. Choices resolve before affected branch changes; changed local or remote
context stops execution. Resolved flags/settings require no additional confirmation.
Duplicate options, unsupported values, and positional arguments are syntax errors.

For unattended execution, resolve relevant choices through flags or configuration:

```sh
gof finish --update rebase --allow-rewrite
gof finish --update merge --no-rewrite
```

Publication targets the same branch at each configured push URL. Each destination
is guarded independently against changes after observation; replacement requires
verified finish rebase evidence. Multiple destinations can complete only partly.
Exit `0` confirms complete publication, including unchanged publication; an
operational failure, unresolved input, cancellation, or incomplete execution exits
`1`. Syntax errors exit `2`. Failures report completed effects and remaining state.

Finish redacts credentials in HTTP(S) URLs across prompts, completion output,
failures, and displayed Git diagnostics by default. The private local setting
`redact-credentials = false` disables redaction for all finish output. Use
`gof finish --show-credentials` to show credentials for one invocation; the flag
takes precedence over the local setting. Disabling redaction exposes credentials
in terminal output and captured logs. No confirmation prompt is added.
Use Git to recover conflicts, then rerun finish as described in [recovery](#recovery).

## Inspect state

```sh
gof status
```

Status reports HEAD/branch, staged and unstaged path counts, conflicts, untracked
paths, submodule changes, active operations, and upstream information. Run it
from the repository root or a nested directory. Bare repositories are identified
as having no working tree.

Counts can overlap when a path has both staged and unstaged changes. Ignored paths
are excluded. Upstream comparisons use locally available references and history;
unavailable comparisons are identified explicitly. Status is a read-only local
observation. Its text is intended for people.

## Inspect and set up configuration

```sh
gof config
gof config --format yaml
gof config show
gof config --yes
gof config -y --format yaml
gof config --yes --format json
```

`config show` displays the effective `publish-remote`, `base-remote`, `base`,
`save-action`, `finish-update`, `finish-rewrite`, and ordered `finish-checks`,
identifying each value's shared file, local file, or built-in default. An inherited base remote identifies
the publication source it follows. Inspection lists discovered files and the
directories searched when a layer is absent.
Settings resolve from local overrides, shared settings, then defaults. Explicit
save/finish flags take precedence over their corresponding settings. Inspection uses shared workflow
validation and diagnostics. It displays check commands as argument lists without
executing them or checking executable availability. Its output is intended for people.

`config` guides you through the publication remote for working branches, the
base source remote, the base branch, the default save action, and the finish
update and rewrite policies. Publication starts at `origin`; the base source
follows publication. Press Enter at the base-source prompt to keep that
relationship, or enter another remote for a separate source. Entering the same
remote also keeps the follow-publication relationship. The base branch starts
at `main`, and all three policies start at `ask`. Press Enter to keep a default,
or enter a replacement. Local remote configuration and branch references provide
offline hints; they never change the defaults. Valid names can be configured before
the remote or branch exists.

Save action choices describe what will happen:

| Value | Save with uncommitted changes |
| --- | --- |
| `ask` | Choose an action each time at a terminal |
| `publish-only` | Publish existing commits; keep uncommitted work local |
| `staged` | Commit exactly the selected staged work with a message, then publish; keep unstaged/untracked work local |
| `wip` | Create separate WIP commits for staged and remaining work, then publish |
| `stash` | Retain work, including untracked files, in a local stash; publish existing commits |

WIP and stash exclude ignored files. A stash stays local. Clean trees publish
existing commits directly; explicit save flags override the configured preference.

Finish update choices are `ask`, `rebase`, `merge`, and `none`. Rewrite choices are
`ask`, `allow`, and `deny`; setup describes `allow` as standing authorization for
eligible replacement with remote safeguards retained. See
[finish](#prepare-and-publish-finished-work) for their effects.

Setup shows the destination, exact proposed file, and effective roles with their
sources after applying private overrides before asking for confirmation. When
the base follows publication, review identifies the publication source it follows.
Private values remain local. Enter `yes` to create it. Enter `no`, leave the
confirmation blank, type `cancel` at any prompt, or end input to stop without
creating a file. Invalid answers receive guidance and can be corrected. Guided
setup requires terminal input and stderr; piped answers cannot approve creation.
Changes to the repository selection, observed remote configuration/branch names,
shared file presence, or effective private override values/sources stop creation.
Inspect the reported change and rerun setup.

`config --yes` (or `-y`) creates built-in defaults without any prompts,
independently of private values and Git settings. It writes `publish-remote = origin`
and omits `base-remote`, preserving the follow-publication relationship. Use it for unattended generation.
The default format is TOML. Both guided and unattended setup accept `--format toml`,
`--format yaml`, or `--format json`, creating `git-oneflow.config.toml`,
`git-oneflow.config.yaml`, or `git-oneflow.config.json`. The loader also accepts
`.yml`. Options can appear in either order.

All config commands work from nested directories. Generation writes at the current
working-tree root, including linked worktrees. It works offline with uncommitted
work, detached HEAD, or an unborn branch. Existing work, the index, refs, and Git
configuration stay unchanged. Success reports the full file path; the file is
ready to edit and commit using Git.

Every recognized shared entry prevents generation, including another supported
extension, an empty/invalid file, a directory, or a dangling symlink. Use
`config show` to inspect existing settings or edit that file. Private values are
never implicitly copied into shared settings. Guided setup validates private files
and shows their effective overrides during review; repair invalid private files
before rerunning. `--yes` reports private file locations without parsing them.
Run `config show` afterward to validate private files and inspect effective settings.

Bare repositories support `config show` using local settings and defaults;
generation requires a working tree. Use `config --help` or `config -h` anywhere,
including without Git.

Generation stages and syncs complete content before publishing the recognized
filename. It rechecks the repository and shared filenames before publication,
and protects the destination against concurrent replacement. Coordinate other
configuration writers: a different supported filename can appear after the final
check, resulting in multiple shared files that the loader will report. After
interruption, an unrecognized `.git-oneflow-config-*.tmp` file may remain at the
root; inspect and remove that leftover file before retrying. The recognized
configuration is either absent or complete.

## Repository settings

Defaults are `publish-remote = origin`, `base-remote` following publication,
`base = main`, `save-action = ask`,
`finish-update = ask`, `finish-rewrite = ask`, and `finish-checks = []`.
Commit shared settings at the Git working-tree root. Keep private overrides in
the Git common directory, usually `.git`. Each directory accepts one of:

- `git-oneflow.config.toml`
- `git-oneflow.config.yaml`
- `git-oneflow.config.yml`
- `git-oneflow.config.json`

Choose one file per layer. Multiple recognized files in the same directory fail
with a diagnostic listing the conflicting paths; extensions have equal priority.
The two layers can use different formats. Resolve the directories with
`git rev-parse --show-toplevel` and `git rev-parse --git-common-dir`.
Commands find the same shared file from any subdirectory. Linked worktrees use
their own shared root files and the same private common-directory overrides.
Bare repositories have the local layer and defaults available for `config show`;
start/save/finish and configuration generation require a working tree.

Settings resolve through **explicit command flags > local overrides >
shared settings > defaults**. Each remote role resolves independently; an omitted
base remote follows publication. Save's action flags override `save-action`.
Finish's update and rewrite flags override their respective settings.
Start has no setting override flags. Parent directories and the home directory
supply no additional settings.

Use a mapping of case-sensitive keys. Remote names, the base branch, and workflow
policies are strings; `finish-checks` is a list of objects. These examples
are equivalent; save each under its matching filename:

```toml
publish-remote = "team"
base = "trunk"
save-action = "ask"
finish-update = "ask"
finish-rewrite = "ask"
finish-checks = []
```

```yaml
publish-remote: "team"
base: "trunk"
save-action: "ask"
finish-update: "ask"
finish-rewrite: "ask"
finish-checks: []
```

```json
{"publish-remote": "team", "base": "trunk", "save-action": "ask", "finish-update": "ask", "finish-rewrite": "ask", "finish-checks": []}
```

`publish-remote` names the Git remote receiving working branches. `base-remote`
names the Git remote supplying the configured `base` branch. The roles can select
the same remote or different remotes. All names are literal named Git remotes;
`origin`, `upstream`, and `fork` have no special role or provider meaning.
`base` is a branch name such as `trunk`.

`publish-remote` resolves from local overrides, then shared settings, and defaults
to `origin`.

`base-remote` resolves independently from local, then shared settings. When it
is absent everywhere, the base remote follows the resolved publication remote.
An explicit shared base remote stays selected when a local override changes only
publication; only a `base-remote` override changes that selection.

| Settings | Publication remote | Base remote |
| --- | --- | --- |
| No remote settings | `origin` | `origin` |
| `publish-remote = personal` | `personal` | `personal` |
| `publish-remote = personal`, `base-remote = project` | `personal` | `project` |

Both named settings work in every shared/local format. Every present value is
validated, including values overridden by a higher-priority layer. Configuration
inspection is local and read-only; configure remote availability in Git for the workflows that use it.

Start creates new work from `base-remote` and resumes published work from
`publish-remote`; save publishes through `publish-remote`. The roles may match
or differ. Finish prepares against `base-remote` and publishes through
`publish-remote`. Use `gof config show` to inspect both roles and
their sources. Guided setup generates `publish-remote` and writes `base-remote`
only when you select a different base source. Setup preserves existing files.

### Single-repository and fork workflows

For one repository, select its remote as publication and let the base follow it:

```toml
publish-remote = "team"
base = "main"
```

For a fork, select the fork for publication and the project for the base:

```toml
publish-remote = "personal"
base-remote = "project"
base = "main"
```

Configure these named remotes using Git before running remote-dependent commands.
Setup uses locally available names as hints and assigns no role based on names
such as `origin`, `upstream`, or `fork`. Fork topology is independent of branching
strategy: role selection changes where work starts and publishes, while branch
naming and finish update/rewrite policies remain separate choices.

With either configuration, the lifecycle is:

```sh
gof start work/parser-fix
# Edit, stage, and make normal Git commits
git add parser.rs
git commit -m "Fix parser"
gof save
gof finish --update merge
```

New work uses the fresh project base. Local work resumes from local history;
published work with no local branch resumes from the publication remote. Save
publishes the current branch. Finish incorporates the project base as needed
using your selected update policy, then publishes the branch. Submit it manually
for review; for the fork example, finish reports:

```text
Next: submit personal/work/parser-fix for review into project/main.
```

Tracking supports two styles: the exact published branch (`personal/work/parser-fix`)
or the project base (`project/main`). Start's remote resume establishes publication
tracking; new work initially has no tracking. Save and finish establish missing
publication tracking after successful publication and retain either valid existing
style. To choose project-base tracking using Git after the base has been fetched:

```sh
git branch --set-upstream-to=project/main work/parser-fix
```

The configured publication role governs gof operations. Ordinary Git commands
continue to use your Git configuration, including upstream and push routing.
For separate publication with project-base tracking, configure Git's push routing
as appropriate for your own Git usage; an explicit effective push remote must
agree with gof's publication role. Neither gof role configuration nor publication
synchronizes your local base or fork base.

`save-action` accepts `ask`, `publish-only`, `staged`, `wip`, or `stash`.
`finish-update` accepts `ask`, `rebase`, `merge`, or `none`. `finish-rewrite`
accepts `ask`, `allow`, or `deny`. Update selects branch preparation against the
remote base; rewrite supplies independent permission for eligible replacement.
Guided setup offers both with `ask` defaults, and `config --yes` writes those
explicit defaults. See [finish](#prepare-and-publish-finished-work) and the
[finish contract](FINISH.md).

### Finish checks

`finish-checks` is an ordered list of objects containing exactly `name` and
`command`. `name` is a nonblank string used as a display label. `command` is a
nonempty list of strings whose first element is a nonempty executable name or
path. Remaining elements are individual arguments. Spaces, quotes, Unicode,
and empty argument strings are preserved exactly. Command strings cannot contain
NUL, which process arguments cannot represent.

These check lists are equivalent in TOML, YAML/YML, and JSON:

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

Commands are exact executable-and-argument lists. Shell features require an
explicit shell invocation, as in the second check. There is no implicit shell
interpretation, interpolation, or argument splitting.

An omitted local `finish-checks` inherits the shared list. A supplied local list
replaces the entire shared list, preserving its order; `[]` explicitly disables
inherited checks. Every supplied list is validated, including overridden shared
lists. `config show` reports the effective list and its source.

Checks execute sequentially in configuration order from the discovered
working-tree root, using your environment and PATH. The executable and each
argument are passed literally. Use an explicit shell command for shell features,
as in the examples above. Checks receive EOF on stdin; their stdout and stderr
are displayed as they run, along with the check name and command. Command display
and diagnostics follow finish's credential-redaction settings; executed arguments
are unchanged.

Every check must succeed. A failed exit, launch error, or interruption stops
finish before publication and prevents later checks. Ctrl-C stops the running
check and its ordinary child processes. Preparation and check effects stay in
place. Fix the reported problem, commit or preserve any generated changes, then
rerun finish. Finish does not commit, stash, undo, or automatically retry check
work. Checks that change the branch, committed work, index, working tree, effective
settings, or routing stop the attempt before another check or publication.
Ordinary ignored build outputs are permitted.

Use `gof finish --skip-checks` (or `gof f --skip-checks`) for an explicit bypass.
A passing or skipped check stage preserves the destination observations and exact
leases that guard publication. Remote movement during checks can still stop
publication. Start, save, and configuration inspection never execute finish
checks. Generated defaults contain an empty list; guided setup leaves check entry
to editing the configuration file.

### Validation and private output settings

Omit keys to inherit them from the next layer. An empty TOML document or an empty
mapping (`{}`) in YAML/JSON inherits every key. Quote YAML values that could be
interpreted as another type, such as `"true"` or `"123"`.

`redact-credentials` is a private local-only boolean, defaulting to `true`. Set
it in the common-directory local file to disable credential redaction in finish
output. Shared files reject this key, and guided setup never copies it into the
generated shared file. `finish --show-credentials` enables credential display for
one invocation regardless of the local value. Disabling redaction exposes
credentials in terminal output and captured logs.

All present files must be valid UTF-8 and satisfy the same validation rules.
Malformed syntax, unknown or duplicate keys, invalid values, and unreadable files
fail even when another layer or an explicit action overrides them. String-valued
settings require strings; check labels and command elements also require strings.
`redact-credentials` requires a boolean in the private
local file. Configuration is read locally as data;
remote availability is checked by the workflow that needs it. Save rechecks
settings before affected operations, including after local preservation. If a
stash or hook changes effective settings, save stops with completed effects
retained; inspect the report and current settings before retrying.

## Recovery

- **Git unavailable:** install Git 2.45 or newer and make it available on PATH.
- **Start blocked:** preserve changes before switching; complete or abort the
  reported Git operation. Use the reported worktree if another checkout owns
  the target. For detached or unborn HEAD, attach a branch or create an initial
  commit before starting.
- **Remote/settings failure:** inspect `gof config show`, the reported files,
  and `git remote -v`. Check the project base at `base-remote/base` and publication
  access at the `publish-remote` push destinations.
- **Configuration creation blocked:** use `config show` to inspect existing settings.
  Keep or edit the reported shared file; select one recognized file per layer.
  For a write failure, check the reported working-tree path and directory access.
- **Save needs input:** select an action explicitly or configure `save-action`.
  Supply a nonblank `--message` for an unattended staged checkpoint.
- **Finish needs input:** select `--update rebase|merge|none` and, when replacement
  is required, `--allow-rewrite` or `--no-rewrite`; or configure the corresponding
  finish settings. Rebase selection supplies no replacement authorization.
- **Finish conflicts:** resolve the reported conflicts, stage resolutions, and use
  `git rebase --continue` or `git merge --continue`. Use `git rebase --abort` or
  `git merge --abort` to abandon that update. Then rerun finish. Finish verifies
  retained rebase evidence before authorizing eligible publication after recovery.
- **Finish check failed or was interrupted:** prepared history and check effects
  stay local. Fix the reported check or launch error, commit or preserve changed
  files, then rerun finish. All configured checks run again.
- **Finish publication failed:** the prepared branch stays local and confirmed
  destination updates remain. Repair access or the reported problem and rerun
  finish. Retrying uses fresh observations and verified rebase evidence; it does
  not repeat an already completed update. New remote work requires incorporation
  with Git. Missing/expired recovery evidence cannot authorize replacement.
  Recovery binds the original remote roles and destinations; changing them can
  require incorporating the destination history with Git before retrying.
- **Scope changed:** inspect the branch, index, files, upstream, and settings,
  then rerun to approve the current state.
- **Retained stash:** inspect `git stash list` and `git stash show <reported-id>`.
  Restore it with `git stash apply --index <reported-id>` when ready. Resolve any
  conflicts reported by Git; successful application retains the stash entry.
- **Partial or interrupted execution:** inspect the reported effects, HEAD,
  `git status`, stash list, upstream settings, and each push destination. Completed
  commits, stashes, and remote updates remain available. Repair the reported
  problem and retry from the actual state. A clean retry creates no empty WIP
  checkpoint. After an interruption without a report, verify remote state too.
- **Incomplete upstream:** repair the reported remote/merge configuration before
  retrying. A confirmed remote update can coexist with failed local tracking setup.
- **Literal `@` branch:** tested Git 2.47.3 accepts the name but refuses
  `git switch @`. Use another name or resolve the checkout through Git.

## Exit status

| Code | Meaning |
| --- | --- |
| `0` | Successful command, observation, or unchanged publication |
| `1` | Operational failure, unresolved input, cancellation, or incomplete execution |
| `2` | Invalid command syntax |

Status successfully reports dirty, conflicted, detached, unborn, and bare states.
Operating-system interruption is unsuccessful and may prevent a final report.
