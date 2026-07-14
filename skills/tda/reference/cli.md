# tda CLI reference

Full command surface. Read this when you need a flag you don't remember; the
main `SKILL.md` covers the everyday loop. Every command prints JSON except
`context` (Markdown) and `export`/`import` (their chosen format). Pass full
ULIDs from JSON output — short prefixes only work in the TUI.

`--db <path>` is accepted on every command (overrides discovery). Global:
`--db` flag > nearest ancestor `.tda/tda.db` > `~/.local/share/tda/tda.db`.

```
tda [--db <path>] <command>

  tui                          Launch the interactive TUI
  add [TITLE]                  Create a task (--batch reads a tree from stdin)
  ls [ID]                      List tasks (roots, or children of ID)
  mv <ID> --to <PARENT>        Move a task to a new parent
  rm <ID>                      Delete a task (--recursive for a subtree)
  link <FROM> <TO>             Add a dependency edge (--kind blocks)
  assign <ID> <ACTOR>          Assign a task to an actor (additive)
  claim <ID> --as <ACTOR>      Claim a task → wip
  set <ID> [fields...]         Edit fields on a task
  tag <ID> [TAGS...]           Add tags to a task
  show <ID>                    Full task: snapshot + parent/breadcrumb/children/blocked/workspace
  context <ID>                 Prompt-ready Markdown brief (ancestor notes + task + children)
  note <ID> <TEXT> --as <me>   Append a timestamped progress note (never clobbers)
  find <TEXT>                  Free-text search over titles + notes
  q [filters...]               Query with filters
  next [filters...]            What to work on next (status:todo, by priority)
  due <today|overdue>          Tasks due today or overdue
  export [ID] --format <fmt>   Export a subtree (default: all roots)
  import <FILE> [--parent ID]  Import a tree from a file
  attach <ID> <FILE>           Attach a file's contents to a task
  db <init|path>               Create / locate the local .tda/tda.db
  ws [init]                    Workspace binding (task subtree ↔ project folder)
```

## Command details (non-obvious flags only)

**add** `[TITLE] [--parent <id>] [--tag <t>]... [--status draft|todo|wip|paused|done] [--batch]`
Default status is `draft`. `--batch` reads one title per line from stdin,
2-space indent = child depth. Titles support `@assignee`, `#tag`, and
`[due]`/`[recurrence]` inline syntax (works here, in the TUI, and in import).

**ls** `[ID] [--tree]` — `--tree` gives the full subtree in flat DFS order with a
`depth` field.

**mv** `<ID> --to <PARENT> [--before <id>|--after <id>]` — reparent, optionally
positioned among the new siblings.

**set** `<ID> [--title] [--notes] [--status] [--due] [--recurrence]`
`--notes` **replaces** the whole notes field — use `note` to append instead.
`--due`: `YYYY-MM-DD`, `"YYYY-MM-DD HH:MM"`, `HH:MM`, a weekday name (next
occurrence), or `none` to clear. `--recurrence`: `daily`, `every N days`,
`every mon,wed,fri`, `monthly`, `every N months`, or `none`.

**q** `[--status] [--as <assignee>] [--under <id>] [--tag] [--due today|overdue] [--sort priority|due|created|updated] [--here]`
`--here` scopes to the workspace containing the current directory.

**next** `[--as <assignee>] [--under <id>] [--tag] [--claimable] [--here]`
`--claimable` returns only tasks the `--as` actor may claim: unassigned or
assigned to them, and not blocked. First hit = highest priority.

**export/import** `--format md|json|sp`. Default `md`. `sp` = Super Productivity
JSON backup, **import-only**, and must be requested explicitly (auto-detection
can't distinguish it from tda's own `json` by extension). `import` without
`--parent` wraps everything in a new root task named after the file; `--parent
root` attaches top-level items at the root; `--parent <id>` attaches under that
task.

**db init** — creates `./.tda/tda.db`, used by any `tda` run in this dir or
below (git-style). Idempotent. **db path** — prints what the cwd resolves to.

**ws init** `[--root <id>]` — binds a folder (default cwd) to a workspace root
task, creating it unless `--root` points at an existing task. The stored path
is a default; override per machine via the config's `[workspaces]` table.

## Concurrency: TUI and CLI at the same time

The Turso/SQLite store takes an exclusive cross-process lock, so only one
process can open the db at a time. When a `tda tui` is running it owns the db
**and** listens on a Unix socket (`tda.sock`) next to the db file. Any other
`tda <command>` (e.g. you shelling out while a human has the TUI open)
transparently sends its command to that socket, runs in-process there, and the
TUI rebuilds so external changes appear live. With no TUI running, `tda` opens
the db directly. Nothing to configure — it just works, and your CLI writes show
up in the human's open TUI immediately.
