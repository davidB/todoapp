# tda

[![CI](https://github.com/davidB/todoapp/actions/workflows/ci.yml/badge.svg)](https://github.com/davidB/todoapp/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
![status: work in progress](https://img.shields.io/badge/status-work%20in%20progress-orange)

A keyboard-first tool to capture, organize, and refine tasks — linearly *and*
as graphs/trees — for both humans and AI agents.

## Why tda

Todoist and Superproductivity are great personal task managers, but they're
built for a single person clicking around a list. Jira is built for teams,
but the account/permissions/workflow ceremony makes it painful for fast,
personal capture. `tda` tries to sit between them:

- **Keyboard-first, capture at typing speed** — batch-add tasks, navigate like
  a file manager, no mouse required.
- **Team-aware, without accounts.** There's no login, no auth system — it's a
  local-first, single trusted store — but assignment is still a first-class,
  optional, multi-valued capability: a task can have 0–n assignees, and each
  one can be a person *or* an agent. Not a single-user tool, just one without
  account ceremony.
- **A tree *and* a graph.** Tasks live in one structural hierarchy (for
  breadcrumbs and priority) plus an independent `blocks` dependency DAG (for
  "what's blocking what") — see [backlog.md](https://github.com/MrLesk/Backlog.md)
  and [breads](https://github.com/nnja/breads) for kindred ideas on
  file/git-native task graphs.
- **Built for AI agents as first-class users**, not just humans with a
  chatbot bolted on: agents can be assignees, claim work, and get full parent
  context — see [AI agent integration](#ai-agent-integration) below.

The full design rationale lives in [`tda-spec.md`](tda-spec.md).

## Features

- Batch keyboard capture — one task per line, no per-task dialog friction.
- Arbitrary-depth hierarchy (single-parent `child` tree) plus a `blocks`
  dependency DAG for cross-cutting "blocked by" relationships.
- Manual ordering (drag-free reorder = priority) and saved/derived query
  views (e.g. "what next", "due today") with their own sort.
- À la carte capabilities per task: `Status`, `Notes` (Markdown), `Schedule`,
  `Estimate`, `Tags`, `Assignment` (0–n, human or agent), `Recurrence`,
  `IssueRef`, `Attachments`, `TimeLog`.
- Claim/delegate: a `todo` task can be claimed by anyone (or only its
  assignee, if one is set), handing off with full parent context.
- Aggregation up the tree: subtree progress %, summed estimate/time-spent,
  earliest due date — each capability defines its own roll-up.
- Markdown and JSON import/export of any list or branch.

See [`tda-spec.md` §2](tda-spec.md#2-goals--non-goals) and
[§4](tda-spec.md#4-functional-requirements-deduped-from-notes) for the full
requirements list.

## Screenshot

![tda TUI: a task tree with status bars, due dates, and an ETA column](https://github.com/davidB/todoapp/blob/main/docs/screenshot-tui.png)

*Early build of the TUI — tree view with per-task status/progress bars, due
dates, and ETA projection.*

## Install

**Homebrew** (macOS/Linux):

```sh
brew install davidB/tap/todoapp-cli

```

**Shell installer** (macOS/Linux):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/davidB/todoapp/releases/latest/download/todoapp-cli-installer.sh | sh
```

**mise** (via the [`github`](https://mise.jdx.dev/dev-tools/backends/github.html)
backend, pulls the prebuilt GitHub release binary):

```sh
mise use -g "github:davidB/todoapp[exe=tda]"
```

**From source:**

```sh
cargo install --path crates/todoapp-cli
```

Prebuilt binaries for macOS (Apple Silicon/Intel) and Linux are also available
on the [releases page](https://github.com/davidB/todoapp/releases).

## TUI

The `tda` binary (built from [`todoapp-cli`](crates/todoapp-cli)) launches a
full-screen, keyboard-only TUI by default:

```sh
tda                             # or: tda tui
```

| Key | Action |
|---|---|
| `j`/`k`, ↑/↓ | move up/down |
| `h`/`l`, ←/→ | collapse / expand |
| `g` / `G` | jump to first / last |
| `a` | add sibling of cursor · at root: add root task |
| `e` | edit task title/notes |
| `space` | cycle status |
| `c` | claim (→ `wip`, actor `me`) |
| `alt+↑` / `alt+↓` | reorder among siblings |
| `alt+→` / `alt+←` | reparent in / out |
| `/` | text search |
| `n` | jump to "what next" |
| `y` | yank (copy) title to clipboard |
| `enter` | view detail |
| `?` | toggle help |
| `esc` | quit / back |

Data is stored via the [Turso](https://turso.tech/) adapter at `$TDA_DB`, or
`~/.local/share/tda/tda.db` if unset.

The `tda` binary can also be driven non-interactively for scripting and
agents:

```sh
tda ls                 # list tasks (tree, JSON or Markdown)
tda add "buy milk"      # capture a task
tda claim <id>          # claim → wip
tda export > backup.md  # export a branch to Markdown/JSON
tda import < backup.md  # round-trip it back in
```

## AI agent integration

The capability model and parent-context propagation (an assignee working a
child task can see its ancestors' titles/notes) were designed with agents in
mind from the start, not bolted on:

- **Today**: the `tda` CLI is scriptable and gives structured JSON output,
  usable by any agent that can shell out.
- **Planned** ([`tda-spec.md` §10](tda-spec.md#10-roadmap), milestone M5): an
  HTTP API (`todoapp-api`) and an MCP server (`todoapp-mcp`) for agents that
  speak those protocols directly.

## Status / Roadmap

Pre-release (`0.0.0`, not yet published). Current state, per
[`tda-spec.md` §10](tda-spec.md#10-roadmap):

- ✅ **M0** — workspace skeleton, CI gates.
- ✅ **M1** — domain core, in-memory store, decider machinery, full test coverage.
- ✅ **M2** — Turso persistence adapter, shared conformance suite.
- 🚧 **M4** — TUI (in progress, delivered ahead of M3 by design).
- ⏳ **M3** — CLI dogfood milestone (`tda` self-hosts `tda-spec.md` as its own task tree).
- ⏳ **M5** — HTTP API + MCP server for agents.
- ⏳ **M6** — templates, richer dependency views, aggregation caching, GUI.

## Architecture

Hexagonal: `adapters → app → core`, enforced by `mise run lint`. Nothing in
`todoapp-core` may import an adapter, a runtime, or a framework.

| Crate | Role |
|---|---|
| [`todoapp-core`](crates/todoapp-core) | Domain: entities, capabilities, ports. No I/O. |
| [`todoapp-app`](crates/todoapp-app) | Use cases: async orchestration of core + ports. |
| [`todoapp-store-mem`](crates/todoapp-store-mem) | Adapter: in-memory store for tests/dev. |
| [`todoapp-store-turso`](crates/todoapp-store-turso) | Adapter: Turso/SQLite persistence. |
| [`todoapp-conformance`](crates/todoapp-conformance) | Shared port-conformance suite, run against every store. |
| [`todoapp-tui`](crates/todoapp-tui) | Adapter: the ratatui TUI (library, consumed by `todoapp-cli`). |
| [`todoapp-cli`](crates/todoapp-cli) | Adapter: the `tda` binary — CLI + launches the TUI. |

See [`tda-spec.md` §5](tda-spec.md#5-architecture) for the full rationale,
including the planned `todoapp-api`/`todoapp-mcp`/`todoapp-ui-core` adapters.

## Development

```sh
mise run build   # build the workspace
mise run test    # run all tests (insta snapshots, proptest, conformance)
mise run lint    # clippy + the core-no-io dependency-rule check
mise run ci      # the full gate — run before committing
```

See [`CLAUDE.md`](CLAUDE.md) for conventions and
[`tda-spec.md`](tda-spec.md) for the full spec.

## License

Apache-2.0 — see [LICENSE](LICENSE).
