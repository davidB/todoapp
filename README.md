# tda

[![CI](https://github.com/davidB/todoapp/actions/workflows/ci.yml/badge.svg)](https://github.com/davidB/todoapp/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/todoapp-cli.svg)](https://crates.io/crates/todoapp-cli)
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
- `@name`/`#tag`/`[...]` title syntax: typing `fix @alice bug #urgent` in a
  title auto-assigns `alice` and tags `urgent`; `Ship it [2026-07-20]` or
  `Standup [09:00] [mon,tue,wed,thu,fri]` sets a due date/time or recurrence —
  no separate step, works from CLI, TUI, or import.
- Aggregation up the tree: subtree progress %, summed estimate/time-spent,
  earliest due date — each capability defines its own roll-up.
- Markdown and JSON import/export of any list or branch.

See [`tda-spec.md` §2](tda-spec.md#2-goals--non-goals) and
[§4](tda-spec.md#4-functional-requirements-deduped-from-notes) for the full
requirements list.

## Screenshot

![tda TUI: a task tree with status bars, due dates, and an ETA column](https://raw.githubusercontent.com/davidB/todoapp/main/docs/demo.gif)

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
| `s` | assign to actor(s) (additive, comma-separated) |
| `alt+↑` / `alt+↓` | reorder among siblings |
| `alt+→` / `alt+←` | reparent in / out |
| `/` | text search |
| `n` | jump to "what next" |
| `v` | toggle live details pane (non-modal, follows the cursor) |
| `y` | yank (copy) title to clipboard |
| `enter` | view detail |
| `?` | toggle help |
| `esc` | quit / back |

Data is stored via the [Turso](https://turso.tech/) adapter in the
OS-standard data dir (e.g. `~/.local/share/tda/tda.db` on Linux).

**Concurrent CLI while the TUI is open.** Turso takes an exclusive
cross-process lock on the db, so normally only one process can open it. To let
you (or an agent) keep using the `tda` CLI while a TUI is running, the TUI owns
the db and listens on a Unix socket next to it (`tda.sock`); any other `tda`
command transparently forwards itself to that socket, runs in-process, and the
TUI rebuilds so external writes appear live. No server to start, nothing to
configure — with no TUI running, `tda` just opens the db directly.

### Configuration

Config is split by scope, both optional TOML files in the OS-standard config
dir — only the fields/tables you set are overridden, everything else keeps
its embedded default:

| File | Scope | Contents |
|---|---|---|
| `~/.config/tda/tui.toml` | TUI only | `[columns]` (order/visibility), `[schedule]` (hours/days used to project the `eta` column), `[status]` (enabled statuses, cycle order, glyphs, spinner), `[styles]` (colors and text styles), `[keybindings]` (action → key chords, e.g. `move_down = ["j", "down"]`), `[behavior]` (e.g. `chain_add = true` keeps the add-task dialog open at the same level after `alt+enter`, for fast batch entry; defaults to `false`) |
| `~/.config/tda/config.toml` | Cross-app (CLI + TUI) | `[workspaces]` — per-machine local-path overrides for `tda ws init` bindings, keyed by workspace name |

`tui.toml`'s default, used as the template to copy from, lives at
[`crates/todoapp-cli/src/tui.default.toml`](crates/todoapp-cli/src/tui.default.toml).

The `tda` binary can also be driven non-interactively for scripting and
agents:

```sh
tda ls                 # list tasks (tree, JSON or Markdown)
tda add "buy milk"      # capture a task
tda claim <id>          # claim → wip
tda export > backup.md  # export a branch to Markdown/JSON
tda import < backup.md  # round-trip it back in
tda import --parent <id> < backup.md   # attach import under an existing task
tda import --parent root < backup.md   # attach import's top-level items at the root
```

## AI agent integration

The capability model and parent-context propagation (an assignee working a
child task can see its ancestors' titles/notes) were designed with agents in
mind from the start, not bolted on:

- **Today**: the `tda` CLI is scriptable and gives structured JSON output,
  usable by any agent that can shell out. The agent loop is first-class:
  `tda next --claimable --here` (find work for the current repo), `tda claim`,
  `tda context` (prompt-ready Markdown brief with ancestor notes and the
  workspace folder), `tda note` (append-only progress log), `tda show`.
  [`skills/tda/SKILL.md`](skills/tda/SKILL.md) documents the whole workflow
  (with a full command reference in
  [`skills/tda/reference/cli.md`](skills/tda/reference/cli.md)). Install it into
  the current project with:

  ```sh
  bunx skills add https://github.com/davidB/todoapp --skill tda
  ```

  or copy the `skills/tda/` folder into `.claude/skills/` (Claude Code) by hand,
  or quote it in your AGENTS.md.
- **Runs alongside a human's TUI**: an agent can keep shelling out to `tda`
  while someone has the TUI open — commands forward over a Unix socket to the
  running TUI, which applies them and refreshes live (see [TUI](#tui) above), so
  human and agent share one store without stepping on the db lock.
- **Workspaces**: `tda ws init` binds a task subtree to a repo/folder, so
  agents can scope searches to the current project and `cd` to a task's code.
- **Planned** ([`tda-spec.md` §10](tda-spec.md#10-roadmap), milestone M5): an
  HTTP API (`todoapp-api`) and an MCP server (`todoapp-mcp`) for agents that
  speak those protocols directly.

## Status / Roadmap

Early releases published to [crates.io](https://crates.io/crates/todoapp-cli).
Current state, per [`tda-spec.md` §10](tda-spec.md#10-roadmap):

- ✅ **M0** — workspace skeleton, CI gates.
- ✅ **M1** — domain core, in-memory store, decider machinery, full test coverage.
- ✅ **M2** — Turso persistence adapter, shared conformance suite.
- ✅ **M4** — TUI (delivered ahead of M3 by design), with a scriptable JSON CLI
  that runs concurrently against a live TUI over a Unix socket.
- 🚧 **M3** — CLI dogfood milestone (`tda` self-hosts `tda-spec.md` as its own task tree).
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
| [`todoapp-config`](crates/todoapp-config) | Config path resolution + TOML parsing, shared by CLI and TUI. |
| [`todoapp-cli`](crates/todoapp-cli) | Adapter: the `tda` binary — CLI plus the ratatui TUI, behind a default-on `tui` feature. |

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
