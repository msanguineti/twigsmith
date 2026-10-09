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
| Exists only on the configured remote | Fetch it and create a tracking local branch |
| New branch | Create it from the freshly fetched configured remote base |

Start reports the action taken. Remote-dependent actions stop if discovery or
fetching fails. Local resume works offline; new branches begin without upstream
tracking. The local base branch retains its current history.

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

Save publishes the current branch to its exact name on the configured remote,
using that remote's push destinations. It creates or fast-forwards the branch;
an unchanged remote tip is also successful. Rewritten or divergent remote history
requires attention before publication can succeed.

Existing upstream settings must match the selected remote and branch. Missing
tracking is established after all push destinations confirm publication.
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

`finish` (shorthand: `f`) prepares and publishes the current clean branch to the configured remote,
then reports the branch, destinations, and commit with a manual submission next
step into the configured base. This is the preparation/publication increment:
checks, external integrations, pull request creation, base integration, tagging,
and cleanup are deferred.

Finish requires a clean index, working tree, and submodules, an attached branch
with an existing commit, and no active Git operation or conflicts. Commit or
preserve uncommitted work first. The branch must differ from the configured base.
Upstream settings must match the configured remote/current branch or be absent;
finish establishes absent tracking after confirmed publication.

Finish observes the remote base and the exact branch at every push destination,
then fetches their observed objects. The local base and tracking refs retain their
history during refresh. Read access to each push destination is required.

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

`config show` displays the effective `remote`, `base`, `save-action`,
`finish-update`, and `finish-rewrite`, identifying
each value's shared file, local file, or built-in default. It lists discovered
files and the directories searched when a layer is absent. Each key resolves
from local overrides, shared settings, then defaults. Explicit save/finish flags take
precedence over their corresponding settings. Inspection uses shared workflow
validation and diagnostics. Its output is intended for people.

`config` guides you through the remote used by start/save/finish, the base branch,
the default save action, and the finish update and rewrite policies. Each proposal
starts with its built-in default: `origin`, `main`, and `ask` for all three policies.
Press Enter to keep it, or enter a replacement. Local remote configuration and branch references provide
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

Setup shows the destination, exact proposed file, and effective private overrides
before asking for confirmation. Enter `yes` to create it. Enter `no`, leave the
confirmation blank, type `cancel` at any prompt, or end input to stop without
creating a file. Invalid answers receive guidance and can be corrected. Guided
setup requires terminal input and stderr; piped answers cannot approve creation.
Changes to the repository selection, observed remote configuration/branch names,
shared file presence, or effective private override values/sources stop creation.
Inspect the reported change and rerun setup.

`config --yes` (or `-y`) creates explicit built-in defaults without any prompts,
independently of private values and Git settings. Use it for unattended generation.
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

Defaults are `remote = origin`, `base = main`, `save-action = ask`,
`finish-update = ask`, and `finish-rewrite = ask`.
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

Each key resolves independently: **explicit command flags > local overrides >
shared settings > defaults**. Save's existing action flags override `save-action`.
Finish's update and rewrite flags override their respective settings.
Start has no setting override flags. Parent directories and the home directory
supply no additional settings.

Use a flat mapping of case-sensitive keys with string values. These examples
are equivalent; save each under its matching filename:

```toml
remote = "upstream"
base = "trunk"
save-action = "ask"
finish-update = "ask"
finish-rewrite = "ask"
```

```yaml
remote: "upstream"
base: "trunk"
save-action: "ask"
finish-update: "ask"
finish-rewrite: "ask"
```

```json
{"remote": "upstream", "base": "trunk", "save-action": "ask", "finish-update": "ask", "finish-rewrite": "ask"}
```

`remote` is a Git remote name. Configure it in Git; its base branch must exist
remotely when creating new work. `base` is a branch name such as `trunk`.
`save-action` accepts `ask`, `publish-only`, `staged`, `wip`, or `stash`.
`finish-update` accepts `ask`, `rebase`, `merge`, or `none`. `finish-rewrite`
accepts `ask`, `allow`, or `deny`. Update selects branch preparation against the
remote base; rewrite supplies independent permission for eligible replacement.
Guided setup offers both with `ask` defaults, and `config --yes` writes those
explicit defaults. See [finish](#prepare-and-publish-finished-work) and the
[finish contract](FINISH.md).
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
settings require strings; `redact-credentials` requires a boolean in the private
local file. Configuration is read locally as data;
remote availability is checked by the workflow that needs it. Save rechecks
settings before affected operations, including after local preservation. If a
stash or hook changes effective settings, save stops with completed effects
retained; inspect the report and current settings before retrying.

### Legacy migration

`<Git common directory>/oneflow.conf` continues to work as local overrides when
that directory has no new-format file, including alongside shared configuration.
It uses literal, unquoted `key = value` lines, with surrounding whitespace trimmed,
blank lines and whole-line `#` comments accepted. Omitted keys inherit shared
settings and defaults.

To migrate, copy its settings into one new-format file in the Git common
directory, using the matching format's string syntax, then remove or rename
`oneflow.conf`. Coexisting legacy and new-format local files fail with migration
guidance. Keep shared project settings in the working-tree root file and private
values in the local file.

## Recovery

- **Git unavailable:** install Git 2.45 or newer and make it available on PATH.
- **Start blocked:** preserve changes before switching; complete or abort the
  reported Git operation. Use the reported worktree if another checkout owns
  the target. For detached or unborn HEAD, attach a branch or create an initial
  commit before starting.
- **Remote/settings failure:** inspect the reported configuration files,
  `git remote -v`, the remote base, push destinations, and access credentials.
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
- **Finish publication failed:** the prepared branch stays local and confirmed
  destination updates remain. Repair access or the reported problem and rerun
  finish. Retrying uses fresh observations and verified rebase evidence; it does
  not repeat an already completed update. New remote work requires incorporation
  with Git. Missing/expired recovery evidence cannot authorize replacement.
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
