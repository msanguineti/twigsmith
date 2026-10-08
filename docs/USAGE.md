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

## Inspect and generate configuration

```sh
gof config show
gof config --yes
gof config -y --format yaml
gof config --yes --format json
```

`config show` displays the effective `remote`, `base`, and `save-action`, identifying
each value's shared file, local file, or built-in default. It lists discovered
files and the directories searched when a layer is absent. Each key resolves
from local overrides, shared settings, then defaults. Save's action flags take
precedence over `save-action` when saving. Inspection uses the same validation
and diagnostics as start/save. Its output is intended for people.

`config --yes` (or `-y`) creates a shared configuration containing explicit
built-in defaults: `remote = origin`, `base = main`, and `save-action = ask`.
The default format is TOML. Select a format with `--format toml`, `--format yaml`,
or `--format json`; generation creates `git-oneflow.config.toml`,
`git-oneflow.config.yaml`, or `git-oneflow.config.json`, respectively. The loader
also accepts `.yml`. Options can appear in either order.

Both commands work from nested directories. Generation writes at the current
working-tree root, including in linked worktrees. It works without a terminal
or network access, with uncommitted work, detached HEAD, or an unborn branch.
It creates valid defaults even when `origin` or `main` is absent. Existing work,
the index, refs, and Git configuration stay unchanged. Success reports the full
file path; the new file is ready to review, edit, and commit using Git.

Every recognized shared configuration entry prevents generation, including a
file with another supported extension, an empty or invalid file, a directory,
or a dangling symlink. `--yes` permits unattended creation and preserves existing
entries. Private override files permit shared generation. Their paths are reported
because their present keys take precedence over generated values. Generation
writes built-in defaults independently of private values and Git settings.
Run `config show` afterward to validate private files and inspect effective settings.

Bare repositories support `config show` using local settings and defaults.
Generation requires a working tree. Running `config` with no options displays
inspection and generation guidance and leaves files unchanged; guided setup is
not available yet. Use `config --help` or `config -h` anywhere, including without Git.

Generation stages and syncs complete content before publishing the recognized
filename. It rechecks the repository and shared filenames before publication,
and protects the destination against concurrent replacement. Coordinate other
configuration writers: a different supported filename can appear after the final
check, resulting in multiple shared files that the loader will report. After
interruption, an unrecognized `.git-oneflow-config-*.tmp` file may remain at the
root; inspect and remove that leftover file before retrying. The recognized
configuration is either absent or complete.

## Repository settings

Defaults are `remote = origin`, `base = main`, and `save-action = ask`.
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
start/save and configuration generation require a working tree.

Each key resolves independently: **explicit command flags > local overrides >
shared settings > defaults**. Save's existing action flags override `save-action`.
Start has no setting override flags. Parent directories and the home directory
supply no additional settings.

Use a flat mapping of case-sensitive keys with string values. These examples
are equivalent; save each under its matching filename:

```toml
remote = "upstream"
base = "trunk"
save-action = "ask"
```

```yaml
remote: "upstream"
base: "trunk"
save-action: "ask"
```

```json
{"remote": "upstream", "base": "trunk", "save-action": "ask"}
```

`remote` is a Git remote name. Configure it in Git; its base branch must exist
remotely when creating new work. `base` is a branch name such as `trunk`.
`save-action` accepts `ask`, `publish-only`, `staged`, `wip`, or `stash`.
Omit keys to inherit them from the next layer. An empty TOML document or an empty
mapping (`{}`) in YAML/JSON inherits every key. Quote YAML values that could be
interpreted as another type, such as `"true"` or `"123"`.

All present files must be valid UTF-8 and satisfy the same validation rules.
Malformed syntax, unknown or duplicate keys, non-string values (including null),
invalid names/actions, and unreadable files fail even when another layer or an
explicit action overrides their values. Configuration is read locally as data;
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
