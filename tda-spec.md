# `tda` — Spec & Roadmap

> Working name: **`tda`** (a pun on *trouble de l'attention* / *to-do app*).
> A keyboard-first tool to capture, organize, and refine ideas and tasks — linearly *and* as graphs/trees — for both humans and AI agents.

This document is written to be handed directly to Claude Code. It is opinionated: every place where I made a call that you might disagree with is marked **[DECISION]** with a one-line rationale, and the genuinely open ones are collected in [§13 Open questions](#13-open-questions). Override freely.

---

## 1. Vision

A fast capture-and-organize tool for people (and agents) who think in bursts. The core loop:

1. **Dump** ideas/tasks at the speed of typing, no friction.
2. **Organize** them into hierarchies and lists, reorder by priority, tag.
3. **Refine** drafts until they are claimable units of work.
4. **Delegate** to a human or an AI agent, who can claim and execute with full parent context.

The same domain is reachable through a CLI, a TUI, an HTTP API, and (later) a GUI and an MCP server for agents — all sharing one core and one navigation model.

**First success criterion (dogfooding):** the app is "done enough" the moment it can store *this roadmap* as its own task tree and export it back to Markdown. That is [Milestone M3](#m3--dogfood-milestone-the-self-hosting-roadmap).

---

## 2. Goals & non-goals

**Goals**
- Keyboard-only batch task creation; sub-second navigation.
- Arbitrary-depth hierarchy, and the same task reachable from multiple trees/lists (a DAG, not a single tree).
- Per-view ordering that doubles as priority, manually reorderable.
- Tags, multi-assignment (0–n, humans or agents), claiming, draft lifecycle.
- Aggregation up the hierarchy (status/progress, time spent, ETA, due date).
- Markdown/JSON import & export of any list or branch.
- A clean hexagonal core with no I/O, fully unit-tested before any adapter exists.

**Non-goals (for v1)**
- Real-time multi-user sync / CRDT collaboration. (Single-user, single-machine first.)
- Mobile apps.
- Rich text / attachments beyond plain notes (Markdown text only).
- Account system / auth. (API is local/trusted first; see [§13](#13-open-questions).)
- gRPC. **[DECISION]** Start HTTP+JSON only — universally consumable by agents and tooling; gRPC adds build/codegen cost with no v1 payoff. Revisit if streaming/perf demands it.

---

## 3. Core concepts (glossary)

| Term | Meaning |
|---|---|
| **Task** | The atomic unit: a stable identity (id + timestamps) that **carries a set of capabilities** rather than a fixed field list. Required: `Title`, `Status`. Optional capabilities attach à la carte. |
| **Capability** | A composable unit of data + behaviour attached to a task: `Status`, `Notes` (Markdown), `Schedule` (due date), `Estimate` (ETA), `TimeSpent`, `Tags`, `Assignment`. A capability may define **(a) data**, **(b) an aggregation** (how it rolls up a subtree), and **(c) guards** (which commands it allows/denies). Tasks differ by which capabilities they hold — composition, not an OOP god-struct. Adding a capability touches nothing existing. |
| **Status** | A required capability. Values: `draft` → `todo` → `wip` → `done` (see [§8](#8-status-lifecycle)). Aggregates to subtree progress. `blocked` is **not** a stored value — it's derived from unmet `blocks` deps. |
| **Assignment** | An *optional* capability: 0–n assignees (`Person` or `Agent`). Its presence changes `Claim` semantics (see [§8](#8-status-lifecycle)): absent/empty ⇒ anyone may claim; present ⇒ only a listed assignee may claim. |
| **Link** | A typed, *ordered* directed edge between two tasks. Types: `child` (structure) and `blocks` (dependency). The `child` graph is a **single-parent tree** (each task has exactly one structural home). The `blocks` graph is a **DAG**. |
| **Tree view** | A curated structure rooted at a task (or a virtual root), following `child` links, with manual order. A hand-built "list" is just a shallow tree. |
| **Query view** | A *derived* list: its members are the result of a **filter + sort**, not stored membership. This is what "list" usually means here (e.g. "what next", "due today"). A task appears in any number of query views for free. |
| **Order** | Two distinct notions. **(1) Manual order** = a `child` link's `position` among its siblings = priority in a tree view; reorderable by hand. **(2) Query order** = the `sort` of a query view (default: tree-priority — the path of positions; overridable, e.g. by due date). Query membership itself isn't hand-reordered; you change the underlying priorities. |
| **Path / breadcrumb** | The ancestor chain shown next to a task whenever it appears outside its home tree — i.e. in **every query view** and the `blocks` view. Essential, not cosmetic. (Your notes' "crumbean" = breadcrumb.) |
| **Command** | An intent to change state (`CreateTask`, `Move`, `SetStatus`, `Claim`, `Assign`, `AddTag`, `Link`, …). Run through guards first; applied only if allowed (see [§5a](#5a-commands-the-decider-pattern)). |
| **Claim** | A `Claim` command: an actor takes a `todo` task → `wip`, subject to the `Assignment` capability's guard. |
| **Template** | A reusable skeleton of child tasks ("steps") that can be instantiated under any task. |

**[DECISION] "Are steps sub-tasks?" → Yes.** Steps are modeled as `child` tasks, not a separate concept. A "template for steps" is therefore a template that instantiates a set of child tasks. This keeps one mechanism for everything and lets steps themselves have steps.

**[DECISION] Structure is a single-parent tree.** Each task has exactly one `child` parent (one structural home, one unambiguous breadcrumb). The only DAG is the `blocks` dependency overlay. "A task in multiple lists" is satisfied by *query views*, not by multiple structural parents. Consequences: subtree move = re-point one parent link; `child` cycle check is the simple "new parent isn't a descendant"; aggregation walks one unambiguous subtree.

---

## 4. Functional requirements (deduped from notes)

Grouped and given IDs so the roadmap can reference them.

**Capture & edit**
- `FR-1` Batch-create multiple tasks from the keyboard in one flow (e.g. one task per line).
- `FR-2` Each task: title, notes, tags. Edit any field.
- `FR-3` Templates: instantiate a predefined set of child tasks under a task.

**Structure**
- `FR-4` Hierarchy of arbitrary depth via `child` links.
- `FR-5` Each task has **exactly one** `child` parent (single-parent tree). Multi-membership across "lists" comes from **query views**, not structure.
- `FR-6` Dependency links (`blocks`) form a DAG and drive non-default views (e.g. "blocked by").
- `FR-7` Manual ordering of `child` links = priority in a tree view; query views sort by a sort spec (default tree-priority).
- `FR-8` Move a task **or a whole subtree** to a new parent/position in one fast action.

**Status & delegation**
- `FR-9` Status lifecycle `draft → todo → wip → done` (see [§8](#8-status-lifecycle)); transitions enforced by the `Status` guard. `draft` is not claimable.
- `FR-10` 0–n assignees per task via the optional `Assignment` capability; assignee may be a Person or an Agent.
- `FR-11` `Claim`: from `todo` only; open to anyone if no assignees, else assignee-only (per the `Assignment` capability).
- `FR-12` A parent task supplies context (its title/notes/path) to an assignee working a child.

**Aggregation & views**
- `FR-13` **Each capability defines its own subtree aggregation**, rolled up current+descendants: `Status` → progress %, `TimeSpent` → sum, `Estimate` → sum, `Schedule` → earliest due. Adding a capability adds its roll-up, nothing else.
- `FR-14` Always render a task with its breadcrumb path in any non-default view.
- `FR-15` Fast navigation (jump, fuzzy find, expand/collapse, move focus) — file-manager-like.

**Search & queries** (this is what most "lists" are)
- `FR-23` Free-text search over title/notes.
- `FR-24` Structured queries combining predicates: `status`, `assignee`, `tag`, `within-subtree-of <task>`, `due` (today / overdue / range), `claimed?` — with a sort spec.
- `FR-25` Saved & built-in **parameterized** queries, e.g. *what next* = `status:todo` sorted by priority; *what next for X under Y with tag Z* = `status:todo assignee:X within:Y tag:Z`; *due today* = `due:today`.

**Architecture-level**
- `FR-26` All mutations are `Command`s passed through capability/system **guards** (allow/deny with reason) before being applied ([§5a](#5a-commands-the-decider-pattern)).

**I/O**
- `FR-16` Export any list or branch to Markdown task list and to JSON.
- `FR-17` Import the same formats (round-trip with `FR-16`).

**Interfaces** (delivered across phases, all over the same core)
- `FR-18` CLI usable by humans and scriptable by AI agents.
- `FR-19` TUI with full keyboard navigation.
- `FR-20` HTTP+JSON API.
- `FR-21` MCP server so agents can read/claim/update tasks as tools.
- `FR-22` GUI (later) sharing the TUI's keybindings and navigation model.

---

## 5. Architecture

**[DECISION] Hexagonal (ports & adapters), domain core has zero I/O dependencies.** This is the part of your notes I'd lock in hardest — it's what makes the "core first, adapters later, fully tested" plan possible.

**[DECISION] Composition-first domain (capabilities-as-components), with the *storage engine* as a swappable adapter.** Composition is the modeling principle (see [§3](#3-core-concepts-glossary)) regardless of engine. Whether the in-memory store is plain composed Rust structs or a `bevy_ecs` `World` lives **behind** the repository ports, so it can be chosen — and changed — without touching the core. This is *not* an all-or-nothing, now-or-never decision.

- **Build `tda-store-mem` (plain composed structs) first.** Simplest path to prove the model and get the conformance suite green.
- **Evaluate `bevy_ecs` as `tda-store-ecs` via a spike** ([§10, Spike-ECS](#spike-ecs--evaluate-bevy_ecs-as-a-store-adapter)) once the ports + conformance suite exist, so the comparison is on real code.
- **Updated facts on Bevy** (its relationship story changed since the original notes): custom relationships are now first-class (define `child` and `blocks` as separate relationship types), and child order *is* preserved (a relationship target can be a `Vec<Entity>`). The old "no ordering" worry no longer applies. `bevy_ecs` runs standalone (no renderer/app runner; `no_std`-capable).
- **The two real costs to weigh in the spike**, not Bevy's hierarchy API:
  1. **Many-to-many isn't native** — a relationship points to a single entity, so the multi-parent DAG (`FR-5`) is modeled as *junction/edge entities* carrying `from`/`to`/`kind`/`position`, i.e. the same `link` model as [§7](#7-data-model-libsql-adapter), just expressed in ECS.
  2. **Two-world sync** — Turso is the durable source of truth; a Bevy `World` would be a *second* in-memory model to keep in sync with it. For a load-subtree → mutate → persist workload, that sync is the main cost; ECS pays off most with many entities × systems per tick, which this isn't (yet).
- **Recommendation:** ship v1 on the plain composed store; adopt `bevy_ecs` only if the spike shows a concrete ergonomic/perf win. Full Bevy (app + render) is out of scope for the core.

### Workspace layout (Cargo workspace)

```
tda/
├─ crates/
│  ├─ tda-core/        # domain: entities, value objects, services, PORTS (traits). No I/O deps.
│  ├─ tda-app/         # use cases / application services orchestrating core + ports
│  ├─ tda-store-libsql/# adapter: persistence via libSQL/Turso (impls the repo ports)
│  ├─ tda-store-mem/   # adapter: in-memory store, plain composed structs (tests + fast dev)
│  ├─ tda-store-ecs/   # adapter (OPTIONAL/spike): in-memory store backed by bevy_ecs
│  ├─ tda-cli/         # adapter: clap-based CLI binary
│  ├─ tda-tui/         # adapter: ratatui TUI binary
│  ├─ tda-api/         # adapter: axum HTTP+JSON server
│  ├─ tda-mcp/         # adapter: MCP server for agents
│  └─ tda-ui-core/     # shared presentation logic: keymaps, navigation state (TUI+GUI share)
└─ Cargo.toml          # [workspace]
```

`tda-core` and `tda-app` may be merged early if the boundary feels heavy; keep ports in `tda-core` regardless.

### Ports (traits the core defines, adapters implement)

- `TaskRepository` — CRUD on tasks.
- `LinkRepository` — create/remove/reorder links; query children, parents, dependents.
- `CollectionRepository` — saved trees & saved queries.
- `QueryEngine` — evaluate a `Query` (filter + sort) over the store; returns task refs with their breadcrumb paths. Pure given a store snapshot, so it's directly unit-testable.
- `Clock` — injected time source (deterministic tests).
- `IdGenerator` — injected IDs (deterministic tests).

Use cases (in `tda-app`) are the only callers of ports. Adapters never call each other.

### Dependency rule
`adapters → app → core`. Nothing in `core` imports an adapter or a concrete framework. Enforce with a workspace check (e.g. a CI grep / `cargo-deny`-style rule that `tda-core/Cargo.toml` has no I/O crates).

---

## 5a. Commands: the decider pattern

All mutations flow through one small, pure shape — **decide then apply** — so that capabilities can veto commands and new behaviour is additive.

```rust
// Pure. No I/O. Lives in tda-core / tda-app.
fn decide(state: &TaskState, cmd: Command) -> Result<Vec<Event>, Denied>;
fn apply(state: TaskState, event: Event) -> TaskState;
```

- **`decide`** runs the command through an ordered list of **guards**. A guard is a pure `(&state, &cmd) -> Option<Denied>`. Guards come from:
  - **capabilities** — `Status` guards legal transitions; `Assignment` guards `Claim` (absent/empty ⇒ open; present ⇒ assignee-only);
  - **systems** — cross-cutting rules, e.g. the `blocks` system can deny `SetStatus(wip)` while a blocker isn't `done` (a derived `blocked` check).
  - First denial wins (or collect all — see [§13](#13-open-questions)). If none deny, `decide` returns the resulting `Event`s.
- **`apply`** folds events into new state. Adapters then persist via the ports.

**Right-sized on purpose** (your "fast + simple to extend" constraint). This is the *decider pattern*, **not** full CQRS/event-sourcing:
- one libSQL store, queried directly — **no** separate read/write models or projections;
- events are the internal result of a command, applied straight to state — **no** required event log (add one later for undo/audit without rearchitecting);
- a plain function call — **no** message bus or async command queue.

Cost of the whole mechanism: one `Command` enum, one `Event` enum, two pure functions, and a `Vec` of guards. Extension = add a capability's guard + its `apply` arm; nothing else changes.

---

## 6. Tech stack

| Concern | Choice | Notes |
|---|---|---|
| Language | **Rust** | Per your notes; good for CLI/TUI/perf. |
| Persistence | **libSQL / Turso** (`libsql` crate, embedded/local file) | Embedded SQLite-compatible. Local-first; cloud sync optional much later. |
| Migrations | Embedded SQL migrations (simple runner, e.g. `refinery` or hand-rolled) | Keep schema versioned from M2. |
| IDs | **UUIDv7** (`uuid` crate) or **ULID** (`ulid`) | Sortable, collision-free, agent-friendly. **[DECISION]** UUIDv7. |
| Serialization | `serde` + `serde_json` | Export/import + API. |
| Errors | `thiserror` in libs, `anyhow` in binaries | |
| CLI | `clap` (derive) | `FR-18`. |
| TUI | `ratatui` + `crossterm` | `FR-19`. |
| HTTP API | `axum` | `FR-20`. |
| MCP | `rmcp` (official Rust MCP SDK) | `FR-21`; exposes tasks as agent tools. |
| Testing | std `#[test]`, `insta` (snapshots), `proptest` (ordering/DAG invariants) | |
| In-memory store (optional) | `bevy_ecs` (standalone, no full Bevy) | Only if the [Spike-ECS](#spike-ecs--evaluate-bevy_ecs-as-a-store-adapter) shows a win. Behind a port; not on the v1 critical path. |

---

## 7. Data model (libSQL adapter)

The `blocks` DAG, the single-parent `child` tree, and per-view ordering are the load-bearing decisions; everything else is conventional.

**Composition mapping.** The capabilities from [§3](#3-core-concepts-glossary) map to storage the same way they compose in memory: identity in `task`, and each *optional* capability is sparse — a nullable column (cheap in SQLite) or its own table when it warrants one (`tag`, `assignment` already are). Adding a future capability = a new column/table + its component, with zero change to existing ones. This keeps the plain-struct store and a potential `bevy_ecs` store as straightforward projections of the same component set.

```sql
-- Actors: humans and agents
CREATE TABLE actor (
  id        TEXT PRIMARY KEY,
  kind      TEXT NOT NULL CHECK (kind IN ('person','agent')),
  name      TEXT NOT NULL
);

CREATE TABLE task (
  id          TEXT PRIMARY KEY,
  title       TEXT NOT NULL,
  notes       TEXT,                     -- Markdown
  status      TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft','todo','wip','done')),
  due_date    TEXT,                     -- ISO-8601, optional
  eta_minutes INTEGER,                  -- own estimate, optional
  time_spent_minutes INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL
);

-- Typed, ORDERED edges. `blocks` is many-to-many (DAG).
-- `child` is single-parent (tree): the `one_parent` index enforces ≤1 incoming `child` link per to_id.
-- `position` orders this link within its `from` node.
CREATE TABLE link (
  from_id   TEXT NOT NULL,   -- parent task, virtual-root sentinel, or collection id
  to_id     TEXT NOT NULL,   -- child / dependent task
  kind      TEXT NOT NULL CHECK (kind IN ('child','blocks')),
  position  REAL NOT NULL,   -- fractional index → cheap reorders (see note)
  PRIMARY KEY (from_id, to_id, kind)
);
CREATE INDEX link_from ON link(from_id, kind, position);
CREATE INDEX link_to   ON link(to_id, kind);
CREATE UNIQUE INDEX one_parent ON link(to_id) WHERE kind = 'child';  -- single-parent invariant

CREATE TABLE collection (         -- saved trees & saved queries
  id    TEXT PRIMARY KEY,
  name  TEXT NOT NULL,
  kind  TEXT NOT NULL CHECK (kind IN ('tree','query')),
  spec  TEXT                       -- for kind='query': JSON {filter, sort, params}; NULL for trees
);

CREATE TABLE tag (
  task_id TEXT NOT NULL,
  tag     TEXT NOT NULL,
  PRIMARY KEY (task_id, tag)
);
CREATE INDEX tag_by_name ON tag(tag);

CREATE TABLE assignment (
  task_id  TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  claimed  INTEGER NOT NULL DEFAULT 0,  -- 1 ⇒ this actor claimed it
  PRIMARY KEY (task_id, actor_id)
);
```

Notes:
- **Fractional `position`** (a "fractional index": insert between two neighbors by averaging their positions) gives cheap manual reordering and subtree moves (`FR-7`, `FR-8`) without rewriting siblings. Re-balance lazily when gaps get too small.
- **Cycle prevention:** `child` is a tree — reject a re-parent if the new parent is a descendant of the moved task. `blocks` is a DAG — reject a new `blocks` edge that would close a cycle. Cover both with a `proptest`.
- **Aggregation (`FR-13`)** is computed by traversing `child` links from a task; cache later if needed.
- **Curated list = shallow tree.** A hand-built "list" is a `child`-tree rooted at a virtual root node; no separate membership mechanism.

### Query model (`FR-23`–`FR-25`)

A query is data, not code — stored in `collection.spec` (for saved ones) and evaluated by the `QueryEngine` port:

```jsonc
{
  "filter": {
    "text": "string?",                 // free-text over title/notes
    "status": ["todo"],                // any-of: draft|todo|wip|done
    "assignee": "actor_id?",
    "tag": ["tag"],                    // all-of (or any-of — see §13)
    "within": "task_id?",             // tasks in the subtree of this task
    "due": "today | overdue | {before|on|after: <date>}?",
    "claimed": "true | false?"
  },
  "sort": [{ "key": "priority|due|created|updated", "dir": "asc|desc" }],
  "params": ["assignee", "within", "tag"]   // unbound holes for parameterized/built-in queries
}
```
Built-ins ship as code-defined templates: `what-next` (`status:todo` sort `priority`), `what-next-for` (params `assignee`,`within`,`tag`), `due-today` (`due:today` sort `due,priority`). `priority` sort = tree-priority key (the path of `position`s), so a flat result still respects tree ordering.

---

## 8. Status lifecycle

`Status` is a required capability with four stored values. `blocked` is **not** a value — it's derived from unmet `blocks` deps. Transitions are enforced by the `Status` guard ([§5a](#5a-commands-the-decider-pattern)).

```
        promote                start (Claim)            complete
draft ──────────▶ todo ───────────────────▶ wip ───────────────────▶ done
   ◀──── demote ───┘    ◀──────── release ───┘    ◀──────── reopen ────┘

derived:  blocked = ∃ a `blocks` dependency whose blocker is not `done`.
          Shown as a badge in any view; (optional guard) can deny `start` while blocked.
```

- `draft` — capturable, refinable, commentable; **not** offered as work (excluded from `what-next`).
- `todo` — ready/claimable; appears in "what next" queues. *(synonym in prose: "ready".)*
- `wip` — work in progress (claimed).
- `done` — complete; rolls up into parent progress.

**Claim rule (`FR-11`), driven by the `Assignment` capability** — *closes the earlier open question*:
- allowed only from `todo`;
- if the task has **no** `Assignment` / zero assignees → anyone may claim, and claiming adds the claimer as assignee;
- if it **has** assignees → only a listed assignee may claim;
- on success: set claimed, status → `wip`.

The `blocked` badge is informational by default. Whether it should *hard-deny* `start` is a guard toggle — see [§13](#13-open-questions).

---

## 9. CLI surface (sketch, for `tda-cli`)

A concrete starting surface so Claude Code has a target. Refine as you build.

```
tda add "Write spec" [--parent <id>] [--tag x --tag y] [--status draft]
tda add --batch            # opens $EDITOR / reads stdin; one task per line, indentation = depth
tda ls [<id>] [--tree] [--query <name>] [--status todo]
tda mv <id> --to <parent-id> [--before <id> | --after <id>]
tda link <from> <to> --kind blocks
tda assign <id> <actor>          tda claim <id> --as <actor>
tda set <id> [--title ..] [--notes ..] [--status draft|todo|wip|done] [--due ..]
tda tag <id> <tag>...

# Search & queries (most "lists" live here)
tda find <text>                                  # free-text (FR-23)
tda q [--status todo] [--as <actor>] [--under <task>] [--tag <t>] [--due today|overdue] [--sort priority|due]
tda next [--as <actor>] [--under <task>] [--tag <t>]   # built-in: status:todo, by priority (FR-25)
tda due today|overdue
tda q save <name> <...same flags...>             # save a parameterized query
tda q run <name> [--as <actor>] [--under <task>] [--tag <t>]   # bind params at run time

tda export <id|--query name> --format md|json
tda import <file> --format md|json
tda template save <id> <name>    tda template apply <name> --to <id>
```

Design the CLI so every command emits machine-readable JSON with `--json` — this *is* the agent interface until the MCP server lands (`FR-18`/`FR-21`).

---

## 10. Roadmap

Milestones are ordered, each with deliverables and acceptance criteria Claude Code can check itself. Tackle them in order; each ends in a green test suite.

### M0 — Project skeleton
- Cargo workspace with the crates from [§5](#workspace-layout-cargo-workspace) (empty `lib.rs`/`main.rs` stubs).
- CI: `cargo build`, `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check`.
- Dependency-rule check: `tda-core` has no I/O deps.
- **Done when:** `cargo test` runs (even if empty) green in CI; workspace compiles.

### M1 — Domain core (no adapters) — *the heart of your notes*
- Entities/value objects: `Task` (identity) + capability components `Title`, `Status`, `Notes`, `Schedule`, `Estimate`, `TimeSpent`, `Tags`, `Assignment`; `Actor`, `Link`, `Collection`, fractional `Position`. Capabilities compose à la carte (see [§3](#3-core-concepts-glossary)).
- Ports: the traits in [§5](#ports-traits-the-core-defines-adapters-implement) (incl. `QueryEngine`).
- `tda-store-mem` in-memory adapter for tests.
- **Command machinery ([§5a](#5a-commands-the-decider-pattern)):** `Command` + `Event` enums and `decide`/`apply`, with per-capability guards (`Status` transitions, `Assignment` claim rules, `blocks` start-gate). Statically composed, no dynamic bus.
- Use cases in `tda-app` (each = build command → `decide` → `apply` → persist via ports): create, batch-create, edit, move/reorder (subtree), link (with **cycle rejection**), tag, assign, claim, status transitions, aggregate subtree (per-capability roll-ups), **evaluate query (filter + sort, with breadcrumb paths) + built-in queries (`what-next`, `what-next-for`, `due-today`)**, export-to-{md,json}, import.
- Tests: unit per command incl. **denial paths** (claim a draft, claim by non-assignee, start while blocked); `proptest` for tree/`blocks`-DAG invariants and ordering; query tests per predicate + sort; `insta` snapshots for export.
- **Done when:** `FR-1`–`FR-17`, `FR-23`–`FR-25` provable against the in-memory store with no `libsql`/UI/HTTP dependency anywhere in `core`/`app`.

### M2 — Persistence (libSQL/Turso)
- `tda-store-libsql` implementing all repo ports; versioned migrations matching [§7](#7-data-model-libsql-adapter).
- A test suite that runs the **same** use-case tests from M1 against the libSQL store (port conformance suite).
- **Done when:** every M1 use case passes against an on-disk libSQL database; round-trip export→import is identity.

### Spike-ECS — evaluate `bevy_ecs` as a store adapter
*Optional, time-boxed (~2–3 days). Run only if you want to settle the ECS question with code.* Prereq: the M2 port-conformance suite.
- Implement `tda-store-ecs` against the same repo ports using a standalone `bevy_ecs` `World`: tasks as entities, capabilities as components, `child`/`blocks` as custom relationships, multi-parent via junction/edge entities.
- Run the **same conformance suite** against it; measure: lines/complexity vs `tda-store-mem`, reorder/move ergonomics, and the cost of syncing the `World` with the libSQL source of truth.
- **Done when:** a go/no-go note exists. Adopt `bevy_ecs` for the in-memory store only on a clear win; otherwise keep the plain composed store. Either way the core is untouched.

### M3 — Dogfood milestone (the self-hosting roadmap)
- `tda-cli` exposing the [§9](#9-cli-surface-sketch-for-tda-cli) surface, backed by libSQL, with `--json` everywhere.
- `tda import` ingests this very file (Markdown task list) into a tree; `tda export` reproduces it.
- **Done when:** you can run `tda import tda-spec.md` and then manage these milestones inside `tda` itself. **This is the "working ToDoApp that can store the roadmap" you asked for.**

### M4 — TUI
- `tda-ui-core`: keymap + navigation state model (shared with future GUI).
- `tda-tui` (ratatui): tree/list views, expand/collapse, fuzzy find (`FR-15`), keyboard reorder & move (`FR-8`), breadcrumb rendering in non-default views (`FR-14`), batch capture (`FR-1`).
- **Done when:** full create→organize→refine→claim loop is doable keyboard-only.

### M5 — API + agents
- `tda-api` (axum HTTP+JSON) over the use cases.
- `tda-mcp` (rmcp) exposing read/claim/update/list-ready as agent tools (`FR-21`), so an assigned agent can pull context (`FR-12`) and work `ready` tasks.
- **Done when:** an external agent can list ready tasks, read parent context, claim, and complete — end to end.

### M6 — Advanced & polish
- Templates (`FR-3`), richer dependency/blocked views (`FR-6`), aggregation caching, saved views.
- GUI (`FR-22`) reusing `tda-ui-core` keybindings. **[OPEN: framework — see §13.]**

---

## 11. Testing strategy

- **Core/app:** pure unit tests; deterministic via injected `Clock`/`IdGenerator`. Property tests for the two invariants that will bite hardest: DAG acyclicity and ordering correctness under inserts/moves.
- **Port conformance suite:** one parametrized test set run against *both* `tda-store-mem` and `tda-store-libsql`. Adding a new store later means just passing this suite.
- **Snapshots (`insta`):** Markdown/JSON export — catches format regressions and proves round-trip.
- **Adapters:** thin integration tests; keep logic out of adapters so there's little to test there.

---

## 12. Definition of done — v1

v1 ships at **M3 + M4**: a keyboard-driven CLI **and** TUI over a libSQL store, supporting arbitrary-depth hierarchy (single-parent tree) with a `blocks` dependency DAG, manual ordering, tags, query/search views ("what next", "due today"), assignees/claiming, the draft→done lifecycle, subtree aggregation, and Markdown/JSON round-trip — and able to host its own roadmap. M5/M6 are post-v1.

---

## 13. Open questions

These are *not* resolved by the decisions above — they need your input (Claude Code can proceed on the suggested default and you correct later):

1. **Depth limit** (`FR-4`): your note said "no depth limit?". Suggest: unlimited, but a TUI render-depth cap for sanity. OK?
2. **Denial aggregation in `decide`** ([§5a](#5a-commands-the-decider-pattern)): when several guards would deny a command, return the **first** denial (suggested — simplest, fastest) or **collect all** reasons (better UX for agents/forms)? Cheap to start with first-denial and switch later.
3. **Aggregated status (`FR-13`)**: how does a parent's status derive from `{draft, todo, wip, done}` children — % `done` by count (suggested), ETA-weighted, plus an optional manual parent override?
4. **`done` vs hidden**: do completed tasks stay in the tree (greyed) or move to an archive view? Suggest stay, with a filter.
5. **API trust model** (`FR-20`/`§2`): local-only/no-auth for v1 (suggested), or a token from day one?
6. **GUI framework** (`FR-22`): far off, but candidates are egui (Rust-native, pairs naturally with ratatui's immediate-mode feel) vs Tauri (web UI). No need to decide now.
7. **Batch-create syntax** (`FR-1`): indentation = depth in the batch buffer (suggested), or flat-only with manual nesting after?
8. **Multi-tag filter semantics** (`FR-24`): `tag:[a,b]` = match **all** (suggested) or **any**? And free-text (`FR-23`): substring (suggested) or FTS5 full-text later?

---

## 14. First instruction to give Claude Code

> Read `tda-spec.md`. Execute **M0** then **M1** only. Set up the Cargo workspace per §5, then implement the domain core and use cases per §3–§9 against an in-memory store, with the full test suite from §11. Do **not** add libSQL, CLI, TUI, or HTTP yet. Treat the §5 dependency rule as inviolable: nothing in `tda-core`/`tda-app` may depend on an adapter or I/O crate. Stop after M1 with a green `cargo test` and a short summary of the use cases implemented.
