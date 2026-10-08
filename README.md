# git-oneflow

A Git workflow CLI for starting workstreams, saving progress, and inspecting
repository state. Invoke it as `git-oneflow` or `gof`.

The native Rust version is under development. It currently provides `start`, `save`, `status`, and `config`. Build it from this repository using
the [development guide](docs/DEVELOPMENT.md).

## Quick start

Once a built executable is on PATH, run these commands from your Git checkout:

```sh
gof start "add user avatars"
# Work and make normal Git commits
gof save
gof status
```

`start` creates or resumes a branch. `save` publishes progress and offers
checkpoint choices for uncommitted work. `status` summarizes the current checkout.

| Command | Shorthand | Purpose |
| --- | --- | --- |
| `start <branch-name-or-phrase>` | `s` | Create or resume a workstream |
| `save [options]` | `v` | Preserve progress and publish the branch |
| `status` | | Inspect repository state |
| `config` / `config show` / `config --yes` | | Set up or inspect shared settings |

Use `gof --help` or `gof <command> --help` for a quick reference.

## Documentation

- [User guide](docs/USAGE.md): commands, settings, automation, and recovery.
- [Contributor guide](docs/DEVELOPMENT.md): setup, builds, checks, and
  troubleshooting.
- [Architecture and design](docs/ENGINEERING.md): project structure, decisions,
  and current implementation boundaries.

Licensed under [MIT](LICENSE).
