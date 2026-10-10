# git-oneflow

A Git workflow CLI for starting workstreams, saving progress, preparing finished work,
and inspecting repository state. Invoke it as `git-oneflow` or `gof`.

The native Rust version is under development. It currently provides `start`,
`save`, `finish`, `status`, and `config`. Build it from this repository using the [development guide](docs/DEVELOPMENT.md).

## Quick start

Once a built executable is on PATH, run these commands from your Git checkout:

```sh
gof start "add user avatars"
# Work and make normal Git commits
gof save
gof finish --mode review
gof status
```

`start` creates or resumes a branch. `save` publishes progress and offers
checkpoint choices for uncommitted work. `finish` prepares, runs configured checks,
and publishes a clean branch for manual submission in review mode. Completion mode
comes from configuration or `--mode`; an unresolved mode asks at a terminal.
`status` summarizes the current checkout.

| Command | Shorthand | Purpose |
| --- | --- | --- |
| `start <branch-name-or-phrase>` | `s` | Create or resume a workstream |
| `save [options]` | `v` | Preserve progress and publish the branch |
| `finish [options]` | `f` | Prepare and publish the branch for submission |
| `status` | | Inspect repository state |
| `config` / `config show` / `config --yes` | | Set up or inspect shared settings |

Run `gof config` to choose a publication remote and a base source for a single
repository or a fork. Use `gof config show` to inspect effective roles and sources.
See the [user guide](docs/USAGE.md#remote-roles-and-fork-workflows) for both
workflows and tracking styles.

Use `gof --help` or `gof <command> --help` for a quick reference.

## Documentation

- [User manual](docs/USAGE.md): commands, settings, automation, and recovery.
- [Contributor guide](docs/DEVELOPMENT.md): setup, builds, checks, and
  troubleshooting.
- [Technical manual](docs/ENGINEERING.md): architecture, workflow invariants,
  failure handling, and implementation diagnosis.

Licensed under [MIT](LICENSE).
