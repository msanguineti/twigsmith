# Twigsmith

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/msanguineti/twigsmith/main/site/assets/logo-dark.svg">
  <img src="https://raw.githubusercontent.com/msanguineti/twigsmith/main/site/assets/logo.svg" alt="Twigsmith anvil and Git branch logo" width="420">
</picture>

A Git workflow CLI for starting workstreams, saving progress, and delivering
finished work, releases, and hotfixes. Invoke it as `twigsmith` or the equivalent
shorthand `tgi`; both share settings and recovery state.

[![CI](https://github.com/msanguineti/twigsmith/actions/workflows/foundation.yml/badge.svg?event=pull_request)](https://github.com/msanguineti/twigsmith/actions/workflows/foundation.yml)
[![npm version](https://img.shields.io/npm/v/twigsmith?label=npm)](https://www.npmjs.com/package/twigsmith)
[![Stable release](https://img.shields.io/github/v/release/msanguineti/twigsmith?label=release)](https://github.com/msanguineti/twigsmith/releases/latest)
[![MIT license](https://img.shields.io/github/license/msanguineti/twigsmith)](LICENSE)

Twigsmith 1.0 supports Linux x86_64 GNU, Windows x64, and macOS ARM64 and Intel.
Git 2.45 or newer must be on PATH. Install the stable npm package with Node.js LTS
and npm:

```sh
npm install --global twigsmith@1.0.0
tgi --version
```

To run it without a global installation:

```sh
npx --yes --package=twigsmith@1.0.0 tgi --help
```

One npm installer detects your host, downloads its matching native executable from
the same version's GitHub release, and verifies the archive and executable hashes.
If npm blocks installation scripts, run `twigsmith-install` after installation.
The installer also accepts explicit target selection; application commands use the
installed executable without downloading. See [installation and repair](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#native-archives-and-npm-installation).

[Native archives and checksums](https://github.com/msanguineti/twigsmith/releases/latest)
need neither Node.js nor Rust. Linux needs glibc 2.35+; Windows needs the Microsoft
x64 v14 Visual C++ Redistributable. Windows 11 and macOS Sequoia 15 are the selected
support targets; Windows qualification uses Server 2025 CI. Windows/macOS archives
are unsigned, and macOS archives are not notarized. The [user guide](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#native-archives-and-npm-installation)
explains requirements, verification, shells and host-security limits.
To build from source, use the [development guide](https://github.com/msanguineti/twigsmith/blob/main/docs/DEVELOPMENT.md).

Coming from the Node.js git-oneflow package? Read the
[migration guide](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#moving-from-the-nodejs-git-oneflow-package)
for command, configuration and shell-completion differences.

## Quick start

Once either command is on PATH, run these commands from your Git checkout:

```sh
tgi start add-user-avatars
# Work and make normal Git commits
tgi save
tgi finish
tgi status
```

`start` creates or resumes a branch; `save` preserves and publishes progress.
`finish` prepares work, runs configured checks, and either publishes the branch for
manual review or integrates it into the configured target. It creates no pull
request and performs no deployment. `status` summarizes the current checkout.

| Command | Shorthand | Purpose |
| --- | --- | --- |
| `start <branch-name>` | `s` | Create or resume a workstream |
| `save [options]` | `v` | Preserve progress and publish the branch |
| `finish [options]` | `f` | Deliver finished work for review or integration |
| `status` | | Inspect repository state |
| `config` / `config show` / `config --yes` | | Set up or inspect shared settings |

Run `tgi config` to choose an editable settings bundle, publication remote, and
base source for a single repository or a fork. Bundles cover OneFlow direct or
hosted review, GitHub Flow review, GitFlow direct, and lightweight trunk direct or
review. They generate ordinary settings that you can customize; no workflow mode
is stored. For unattended setup, use, for example,
`tgi config --yes --bundle github-flow --publish-remote fork --base-remote upstream`.
See [settings bundles](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#settings-bundles)
for routes and defaults. Use `tgi config show` to inspect effective roles and sources.
See the [user guide](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#remote-roles-and-fork-workflows) for both
workflows and tracking styles.

For releases and hotfixes, select explicit policy with an editable
[direct OneFlow](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#direct-releases-and-hotfixes)
or [hosted OneFlow](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#hosted-releases-and-hotfixes)
setup, or choose [GitFlow](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#gitflow-journeys)
for separate production and development lines. Branch prefixes alone select no
policy. The user guide also covers
[interactive history editing](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#edit-ordinary-work-history).

Terminal menus support arrow keys and Enter; slow Git operations show activity.
`NO_COLOR` disables colour, and redirected output stays plain. See the
[user manual](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#start-or-resume-work) for command details.

Use `tgi --help` or `tgi <command> --help` for a quick reference.
Enable Tab completion for Bash, Zsh, Fish or PowerShell with the
[shell completion guide](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md#shell-completion).

## Documentation

- [User manual](https://github.com/msanguineti/twigsmith/blob/main/docs/USAGE.md): commands, settings, automation, and recovery.
- [Contributor guide](https://github.com/msanguineti/twigsmith/blob/main/docs/DEVELOPMENT.md): setup, builds, checks, and
  troubleshooting.
- [Technical manual](https://github.com/msanguineti/twigsmith/blob/main/docs/ENGINEERING.md): architecture, workflow invariants,
  failure handling, and implementation diagnosis.

Licensed under [MIT](https://github.com/msanguineti/twigsmith/blob/main/LICENSE).
