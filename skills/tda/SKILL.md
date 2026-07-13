---
name: tda
description: Work tasks from the tda task manager as an AI agent — find a claimable task (globally or for the current repo), claim it, pull ancestor context, refine into subtasks, log progress notes, and complete it. Use when asked to "work on the next task", "pick a task", or to create/update tasks in tda.
---

# tda for agents

`tda` is a keyboard-first task manager with a JSON CLI. Every command prints
JSON (except `context`, which prints Markdown). IDs are ULIDs; short unique
prefixes are accepted in the TUI, but pass full ids from JSON output.

## Your identity

Use one actor id per harness+model, shaped `<harness>/<model>`, e.g.
`claude-code/fable-5` or `opencode/gpt-5`. Users assign work to you with
`tda assign <id> <actor>`; an unassigned `todo` task is claimable by anyone.

## The work loop

1. **Find** a task:
   - for the current repo: `tda next --as <me> --claimable --here`
   - globally: `tda next --as <me> --claimable`
   - `--claimable` returns only tasks you may actually claim: `todo`,
     unassigned-or-assigned-to-you, not blocked. First hit = highest priority.
2. **Claim** it: `tda claim <id> --as <me>` → status becomes `wip`, you are
   recorded as the claimer. A denial (already claimed, assigned to someone
   else, blocked) is a normal outcome — pick the next task.
3. **Start clean**: clear your context (or spawn a fresh subagent) and seed it
   with `tda context <id>` — a self-contained Markdown brief: ancestor
   titles+notes (the "why"), the task itself, its children, and the workspace.
4. **Go to the code**: `Workspace: <name> — /path` in the context (or
   `.workspace.path` in `tda show <id>`) is the folder to `cd` into. A task
   without a workspace is folder-independent.
5. **Refine** while working:
   - child task: `tda add "subtask" --parent <id> --status todo`
   - sibling follow-up (e.g. "user review", "todo later"): read `.parent`
     from `tda show <id>`, then `tda add "..." --parent <parent> --status draft`
   - dependency: `tda link <blocker> <blocked> --kind blocks`
6. **Log progress**: `tda note <id> "what happened" --as <me>` — appends a
   timestamped entry to the notes, never overwrites. Use it for decisions,
   blockers, and hand-off state. (`tda set --notes` *replaces* — avoid.)
7. **Finish**: `tda set <id> --status done`. To release without finishing:
   `tda note` the state, then `tda set <id> --status todo` (or `paused` if it
   should not be offered as work).

## Other useful commands

- `tda show <id>` — full task + `parent`, `breadcrumb`, `children`, `blocked`,
  inherited `workspace`. The one-stop read.
- `tda q --status todo --tag x --under <id> | --here` — structured queries.
- `tda find "text"` — free-text search over titles/notes.
- `tda ls [<id>] [--tree]` — children / subtree listing.
- `tda add --batch` — batch-create from stdin, 2-space indent = depth; titles
  support `@assignee`, `#tag`, and `[due/recurrence]` syntax.

## Workspaces (task ↔ repo binding)

- `tda ws init` (run in a repo) creates a root task bound to that folder;
  `tda ws` prints the workspace root for the cwd.
- The task stores the workspace **name** + a default path. On a machine where
  the path differs, override it in `~/.config/tda/tui.toml`:

  ```toml
  [workspaces]
  proj = "/home/me/src/proj"
  ```

## Database

`--db <path>` > nearest ancestor `.tda/tda.db` (`tda db init`) > the global
`~/.local/share/tda/tda.db`. `tda db path` shows what resolves.
