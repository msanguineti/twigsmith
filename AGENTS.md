# Working guidance

## Sources and scope

- [README.md](README.md): product introduction and documentation navigation.
- [docs/USAGE.md](docs/USAGE.md): commands, settings and recovery contracts.
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): setup, builds, checks and test selection.
- [docs/ENGINEERING.md](docs/ENGINEERING.md): architecture, workflow invariants and
  implementation diagnosis. For implementation, read the architecture and Git
  boundary, then the sections relevant to the affected behavior.

Verify relevant claims against source, tests and Git history. Read only the context
needed for the task. Keep public documentation usable without private notes.
For owner-directed engineering work, also read the local coordination instructions
in ignored `.dev/engineering-context.md`, when present, and any handoff named by the
assignment. Those records provide context and accepted direction, not authorization
for additional work. Ordinary contributions do not require private files.

Implement only the accepted task or current authorized phase. Proposed and deferred
work does not authorize execution. Distinguish approved product decisions from
personal usage examples, proposals and assumptions. Repository development policy
must not implicitly change the product's supported Git workflows.

Base consequential product and workflow choices on concrete developer use cases,
authoritative Git documentation and inspected practice in established projects or
tools. Verify relevant Git behavior in source or isolated real-Git fixtures. Cite
material evidence and its limits; one project's practice does not establish general
adoption. Routine engineering choices need no research or approval ceremony.

## Development and verification

Use the existing `./dev` entry points. Discover focused scopes with
`./dev test-groups`; iterate with `./dev test-group NAME` or Cargo selectors through
`./dev test`. Expand coverage for shared components, dependencies, configuration
and cross-command behavior.

For executable, dependency, configuration or development-tooling changes, run one
complete `./dev check` against the final combined implementation before handoff.
Repeat it only when later changes, failures or unresolved concerns require fresh
validation. Focused tests support iteration and do not replace this final check.

For documentation-only changes, verify links/anchors, examples, option names,
technical claims against source and existing tests, and whitespace. Do not run
builds, application tests, Rust formatting, Clippy or `./dev check` when executable
behavior and tooling are unchanged.

Preserve unrelated changes and serialize shared-file writers and Git index
operations. Isolated disposable repositories may perform Git operations needed by
fixtures. Before the new release, obsolete product compatibility need not be kept.

## Repository delivery

The authoritative GitHub repository is `msanguineti/twigsmith`. Use this repository's
actual checkout; do not hard-code a personal filesystem path.

Use short-lived task branches from up-to-date `origin/main`, normally named
`codex/<task>`. Do not implement directly on `main`. Keep changes small and integrate
frequently; do not introduce permanent development or feature-integration branches.
Worktrees are optional when isolation or concurrent work warrants them.

After the required checks and review, selectively commit authorized
content and inspect the resulting commit. Push branches and open pull requests
when remote delivery is authorized by the task. All normal integration into `main`
goes through a PR. Prefer squash merging ordinary task PRs and remove task branches
after successful authorized integration.

Merge only when the owner authorizes that integration and required CI has passed
against up-to-date `main`. Passing CI alone is not merge authorization. Mandatory
human PR approval is not required; technical review and owner acceptance remain
separate. Do not bypass protection, force-push, reset or rewrite history without
explicit authorization. Emergency exceptions must identify their scope and reason.
Verify actual server enforcement rather than assuming these instructions enable it.

PR CI validates the proposed integration. Do not repeat the full source suite solely
because an unchanged validated PR was merged. Artifact building and platform
qualification belong to explicit release preparation, triggered manually or by the
selected release event; reuse enforced source validation. Do not introduce a merge
queue or custom validation ledger without a demonstrated need. Changes to GitHub
settings, visibility, release publication or tags require explicit owner direction.

## Documentation and presentation

Keep documentation organized by enduring responsibility: README introduces and
navigates; USAGE explains commands, settings, automation and recovery; DEVELOPMENT
explains contributor setup and checks; ENGINEERING explains component responsibilities,
workflow/state models, invariants, failure mechanisms and design rationale. Keep
local algorithm explanations near code and use selective source/test references.

Substantive changes must update affected human documentation. Describe current
implemented behavior, replace superseded explanations, and distinguish CLI operations
from library-only capabilities. Presentation changes must preserve command grammar
and execution behavior.

Write direct, affirmative help and documentation. Explain effects and essential
qualifications plainly. Structure help with a brief description, usage, applicable
arguments/options, examples and essential notes. Treat users as responsible
developers: avoid redundant confirmations, warnings and responsibility lectures.
Escalate unresolved consequential decisions; make routine choices autonomously.

Keep public files free of private coordination, task status, session summaries,
transcripts, validation logs and backlogs. Stable repository-wide guidance belongs
here; owner-specific execution arrangements and accepted future direction belong
in ignored `.dev/` records. Consolidate lasting implemented knowledge into the
existing manuals rather than adding a document for each task. Keep necessary local
path examples home-relative, such as `~/...`, without personal usernames.
