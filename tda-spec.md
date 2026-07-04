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
- Account system / auth. (API is local/trusted first; see [§13](#13-open-questions).)
- gRPC. **[DECISION]** Start HTTP+JSON only — universally consumable by agents and tooling; gRPC adds build/codegen cost with no v1 payoff. Revisit if streaming/perf demands it.
- Rich text beyond plain Markdown notes. **[DECISION]** Attachments (link or actual file/image bytes) are, however, in scope — see `Attachments`/`BlobStore` in §3/§7. Superseded the earlier "no attachments" non-goal to support importing tasks (e.g. from Super Productivity) that carry file/image references.

---

## 3. Core concepts (glossary)

| Term | Meaning |
|---|---|
| **Task** | The atomic unit: a stable identity (id + timestamps) that **carries a set of capabilities** rather than a fixed field list. Required: `Title`, `Status`. Optional capabilities attach à la carte. |
| **Capability** | A composable unit of data + behaviour attached to a task: `Status`, `Notes` (Markdown), `Schedule` (due date, optionally with time-of-day), `Estimate` (ETA), `TimeSpent`, `TimeLog` (per-day breakdown), `Tags`, `Assignment`, `Recurrence`, `IssueRef`, `Attachments`, `Archived`. A capability may define **(a) data**, **(b) an aggregation** (how it rolls up a subtree), and **(c) guards** (which commands it allows/denies). Tasks differ by which capabilities they hold — composition, not an OOP god-struct. Adding a capability touches nothing existing. |
| **Status** | A required capability. Values: `draft`, `todo`, `wip`, `paused`, `done` (see [§8](#8-status-lifecycle)) — `[DECISION]` any value may be set to any other, freely, no transition guard. Aggregates to subtree progress. `blocked` is **not** a stored value — it's derived from unmet `blocks` deps. |
| **Assignment** | An *optional* capability: 0–n assignees (`Person` or `Agent`). Its presence changes `Claim` semantics (see [§8](#8-status-lifecycle)): absent/empty ⇒ anyone may claim; present ⇒ only a listed assignee may claim. |
| **Recurrence** | An *optional* capability: a repeat rule (`daily`/`weekly`-by-weekday/`monthly`). **[DECISION]** No per-occurrence task spawning — a recurring task **resets in place**: completing it (`SetStatus(done)`) recomputes its `Schedule` from the rule and its own current due date, and flips `Status` back to `todo`, instead of staying `done`. A task with no `Schedule` can't meaningfully recur, so it just completes normally. |
| **IssueRef** | An *optional* capability: a static `{provider, id, url}` reference to an external issue tracker (e.g. a GitHub/Jira issue) — no live sync, no computed URL. Mainly populated by imports. |
| **Attachments** | An *optional* capability: a list of `{id, kind (link/file/image), title, url, blob, mime}`. `link` never has a `blob`; `file`/`image` may reference actual bytes stored via the `BlobStore` port (content-addressed) or just carry a source `url`/path when bytes aren't available (e.g. an import that only has metadata). |
| **Archived** | An *optional*, presence-only capability, **orthogonal to `Status`** (a task can be `done` and archived, or archived without being `done`) — resolves [§13](#13-open-questions) Q4: archived tasks stay in the store (not moved to a separate structure) and are hidden from default views (`what-next`, `due-today`) by those views passing `archived:false`; `Filter.archived` itself stays neutral (`None` = no restriction). |
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
- `FR-9` Status values `draft, todo, wip, paused, done` (see [§8](#8-status-lifecycle)); `[DECISION]` `SetStatus` accepts any value, no transition guard — a person may drop straight back from `wip` to `draft`, for instance. `draft` is not claimable.
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

**Extended capabilities** (added to support importing tasks from other tools, e.g. Super Productivity)
- `FR-27` `Recurrence` capability: completing a recurring task recomputes its `Schedule` from the rule and resets `Status` to `todo`, instead of staying `done` — no per-occurrence task spawning.
- `FR-28` `IssueRef` capability: a static external issue-tracker reference (`provider`/`id`/`url`) on a task, for context — no live sync.
- `FR-29` `Attachments` capability + `BlobStore` port: link or file/image attachments, with actual bytes stored content-addressed when available.
- `FR-30` `Archived` capability: orthogonal to `Status`; hidden from default views via a filter, not a separate structure (resolves [§13](#13-open-questions) Q4).
- `FR-31` `TimeLog` capability: per-day time-spent breakdown; `TimeSpent` (`FR-13`'s aggregate) is recomputed as its sum whenever set.

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

**[DECISION] Composition-first domain (capabilities-as-components), ECS as *inspiration*, not a dependency.** Composition is the modeling principle (see [§3](#3-core-concepts-glossary)): a minimal `Task` identity with capability components attached à la carte, mirrored by the table-per-capability storage in [§7](#7-data-model-turso-adapter). This is the good half of ECS — composition over inheritance — taken as a design pattern. We do **not** pull in an ECS engine (`bevy_ecs` or otherwise): no second in-memory `World` to sync against the durable store, no engine runtime in the core.

- The in-memory store (`tda-store-mem`) is plain composed Rust — component maps keyed by `TaskId` (`HashMap<TaskId, Status>`, …), which is the ECS data shape without the framework. It sits behind the same repository ports as the Turso store, so it stays a swappable adapter.
- If a real need for ECS-style systems/queries ever appears, it can be added later as just another store adapter behind the ports — the composition model already leaves the door open, with nothing to undo.

### Workspace layout (Cargo workspace)

```
tda/
├─ crates/
│  ├─ tda-core/        # domain: entities, value objects, services, PORTS (traits). No I/O deps.
│  ├─ tda-app/         # use cases / application services orchestrating core + ports
│  ├─ tda-store-turso/ # adapter: persistence via the `turso` crate (impls the repo ports)
│  ├─ tda-store-mem/   # adapter: in-memory store, plain composed components (tests + fast dev)
│  ├─ tda-cli/         # adapter: clap-based CLI binary
│  ├─ tda-tui/         # adapter: ratatui TUI binary
│  ├─ tda-api/         # adapter: axum HTTP+JSON server
│  ├─ tda-mcp/         # adapter: MCP server for agents
│  └─ tda-ui-core/     # shared presentation logic: keymaps, navigation state (TUI+GUI share)
└─ Cargo.toml          # [workspace]
```

`tda-core` and `tda-app` may be merged early if the boundary feels heavy; keep ports in `tda-core` regardless.

### Ports (traits the core defines, adapters implement)

- `ComponentStore` — **capability-keyed access** (ECS/column-store style): `get<C>(id)`, `set<C>(id, value)`, `remove<C>(id)` read/write/detach **one capability at a time**, keyed by task `Id`. Presence of a component *is* the capability. There is **no** whole-task load/save aggregate — a caller (or a guard) touches only the capabilities it needs, so heavy components like `Notes` are read only when named. This is what makes the [§7](#7-data-model-turso-adapter) table-split pay off. **[DECISION]** capability-keyed over an assembled `Task`/projection: a partial monolith is a mess to mutate and reconcile; `store.get::<Status>(id)` is the genuine column shape. Generic `get<C>` is not object-safe, so use cases hold a concrete store (`Services<St>`), not `&dyn`.
- `TaskEntityStore` — the minimal `task` entity: `create`/`delete` (delete cascades every component), `touch` (`updated_at`), `meta` (timestamps), `all` (ids).
- `LinkRepository` — create/remove/reorder links; query children, parents, dependents.
- `CollectionRepository` — saved trees & saved queries.
- `QueryEngine` — evaluate a `Query` (filter + sort) over the store; returns task refs with their breadcrumb paths. Pure given a store snapshot, so it's directly unit-testable.
- `Clock` — injected time source (deterministic tests).
- `IdGenerator` — injected IDs (deterministic tests).
- `BlobStore` — content-addressed byte storage for `Attachment` file/image content (`FR-29`); separate from `ComponentStore` since blobs aren't a per-task-capability row.

Use cases (in `tda-app`) are the only callers of ports. Adapters never call each other.

**Async boundary.** The `turso` crate is async (tokio), so the **repository ports are `async` traits** and use cases are `async`. **[DECISION — supersedes the earlier "core stays sync & pure" call]** Because capabilities are read capability-keyed on demand (`ComponentStore`, above), `decide`/`apply` ([§5a](#5a-commands-the-decider-pattern)) are themselves **`async` and take the store**: a guard `get`s only the components it inspects, and `apply` `set`s only what changed — no assembled snapshot to fold. `QueryEngine`-style evaluation still hoists its async component loads before the sync sort. The **dependency rule is unaffected**: `ComponentStore` is a *port* defined in `tda-core`; adapters implement it, so the core still imports no runtime/adapter. (Cost accepted: more loads and a non-pure `decide`; negligible for a local tool — ECS engines do far more per frame.) The `tda-store-mem` adapter implements these async traits trivially.

### Dependency rule
`adapters → app → core`. Nothing in `core` imports an adapter or a concrete framework. Enforce with a workspace check (e.g. a CI grep / `cargo-deny`-style rule that `tda-core/Cargo.toml` has no I/O crates).

---

## 5a. Commands: the decider pattern

All mutations flow through one small, pure shape — **decide then apply** — so that capabilities can veto commands and new behaviour is additive.

```rust
// Capability-keyed. Async over the ComponentStore port (a tda-core trait — no
// runtime/adapter in core). Lives in tda-core; orchestrated by tda-app.
async fn decide<St: ComponentStore>(store: &St, id: &Id, cmd: &Command, ctx: &DecideCtx)
    -> Result<Vec<Event>, Denied>;
async fn apply<St: ComponentStore>(store: &St, id: &Id, event: &Event);
```

- **`decide`** runs the command through an ordered list of **guards**. A guard reads the capabilities it needs via `store.get::<C>(id)` and returns `Option<Denied>`. Guards come from:
  - **capabilities** — `Status` guards legal transitions; `Assignment` guards `Claim` (absent/empty ⇒ open; present ⇒ assignee-only);
  - **systems** — cross-cutting rules, e.g. the `blocks` system can deny `SetStatus(wip)` while a blocker isn't `done` (a derived `blocked` check).
  - First denial wins (or collect all — see [§13](#13-open-questions)). If none deny, `decide` returns the resulting `Event`s.
- **`apply`** writes each event back as components via `store.set`/`store.remove` (a collection that becomes empty is removed — presence-as-capability, [§7](#7-data-model-turso-adapter)). The event seam is kept (a future audit/undo log can subscribe), it just isn't folded into an aggregate.

**Right-sized on purpose** (your "fast + simple to extend" constraint). This is the *decider pattern*, **not** full CQRS/event-sourcing:
- one Turso store, queried directly — **no** separate read/write models or projections;
- events are the internal result of a command, applied straight to state — **no** required event log (add one later for undo/audit without rearchitecting);
- a plain function call — **no** message bus or async command queue.

Cost of the whole mechanism: one `Command` enum, one `Event` enum, two async functions, and an ordered list of guards. Extension = add the capability's `Component` type (the generic store needs no change) + its `Command`/`Event` variant + a guard + an `apply` arm; nothing else changes.

---

## 6. Tech stack

| Concern | Choice | Notes |
|---|---|---|
| Language | **Rust** | Per your notes; good for CLI/TUI/perf. |
| Persistence | **Turso** (`turso` crate, embedded in-process, local file) | The Rust rewrite of SQLite (MVCC, async I/O); SQLite-compatible SQL so the schema is unchanged. Async (needs `tokio`). Currently **beta** — acceptable for a personal dogfood tool; watch for rough edges. |
| Migrations | Embedded SQL migrations (simple runner, e.g. `refinery` or hand-rolled) | Keep schema versioned from M2. |
| IDs | **ULID** (`ulid` crate) + git-style short display prefix | 128-bit, time-sortable, compact (26-char base32). **Stable random** id assigned at creation — *not* a content hash/CID (content mutates; a content hash would change on every edit). Random 128-bit ⇒ safe DB merge. **[DECISION]** ULID; UUIDv7 is the interop-safe fallback. |
| Serialization | `serde` + `serde_json` | Export/import + API. |
| Errors | `derive_more` (`Error`/`Display`/`From` derives) in libs, `anyhow` in binaries | Replaces `thiserror`; same ergonomics via derives, plus the other `derive_more` conveniences. |
| CLI | `clap` (derive) | `FR-18`. |
| TUI | `ratatui` + `crossterm` | `FR-19`. |
| HTTP API | `axum` | `FR-20`. |
| MCP | `rmcp` (official Rust MCP SDK) | `FR-21`; exposes tasks as agent tools. |
| Testing | std `#[test]`, `insta` (snapshots), `proptest` (ordering/DAG invariants) | |
| Async runtime | `tokio` | Required by `turso`; also used by `axum` (API) and the MCP server. |

---

## 7. Data model (Turso adapter)

The `blocks` DAG, the single-parent `child` tree, and per-view ordering are the load-bearing decisions; everything else is conventional.

**Composition mapping — one table per capability.** The capabilities from [§3](#3-core-concepts-glossary) are stored the same way they compose in memory: a **minimal `task` entity** (id + timestamps) plus **one component table per capability**, keyed by `task_id`. **The presence of a row *is* the capability** — there is no "absent vs unknown" ambiguity, and add/remove = insert/delete. A new capability is a new table, touching nothing existing. List/tree views join only the components they render (`task ⋈ c_title ⋈ c_status`) and never load `c_notes`. The `tda-store-mem` component maps and these capability tables are two projections of the same component set (component map ⟷ component table) — the ECS-inspired shape, no engine required.

```sql
-- Actors: humans and agents
CREATE TABLE actor (
  id        TEXT PRIMARY KEY,
  kind      TEXT NOT NULL CHECK (kind IN ('person','agent')),
  name      TEXT NOT NULL
);

-- Minimal entity: identity only. Everything else is a capability component.
CREATE TABLE task (
  id          TEXT PRIMARY KEY,   -- random immutable id; short unique prefix shown (see §6 / ID note)
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL
);

-- Capability components: 1:1 with task, row exists iff the task HAS the capability.
CREATE TABLE c_title    ( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE, title TEXT NOT NULL );
CREATE TABLE c_status   ( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE,
                          status TEXT NOT NULL CHECK (status IN ('draft','todo','wip','paused','done')) );
CREATE TABLE c_notes    ( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE, notes TEXT NOT NULL ); -- Markdown, lazy-loaded
CREATE TABLE c_schedule ( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE, due_date TEXT NOT NULL ); -- ISO-8601
CREATE TABLE c_estimate ( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE, eta_minutes INTEGER NOT NULL );
CREATE TABLE c_timespent( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE, minutes INTEGER NOT NULL );
-- Multi-valued capability components:
CREATE TABLE c_tag        ( task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE, tag TEXT NOT NULL,
                            PRIMARY KEY (task_id, tag) );
CREATE INDEX tag_by_name ON c_tag(tag);
CREATE TABLE c_assignment ( task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE, actor_id TEXT NOT NULL,
                            claimed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (task_id, actor_id) );
CREATE TABLE c_timelog ( task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE, day TEXT NOT NULL,
                         minutes INTEGER NOT NULL, PRIMARY KEY (task_id, day) ); -- per-day breakdown; c_timespent stays the cached sum
CREATE TABLE c_attachment ( task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE, attachment_id TEXT NOT NULL,
                            kind TEXT NOT NULL CHECK (kind IN ('link','file','image')), title TEXT NOT NULL,
                            url TEXT, blob_id TEXT REFERENCES blob(id), mime TEXT,
                            PRIMARY KEY (task_id, attachment_id) );

-- Nested/structured capability components: one JSON-serialized column each,
-- rather than one column per variant field.
CREATE TABLE c_recurrence ( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE, data TEXT NOT NULL );
CREATE TABLE c_issueref   ( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE, data TEXT NOT NULL );

-- Presence-only capability component: no data column, the row IS the flag.
CREATE TABLE c_archived ( task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE );

-- Blob storage (`BlobStore` port): content-addressed, referenced by c_attachment.blob_id.
CREATE TABLE blob ( id TEXT PRIMARY KEY, content BLOB NOT NULL, created_at INTEGER NOT NULL );

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
```

Notes:
- **Fractional `position`** (a "fractional index": insert between two neighbors by averaging their positions) gives cheap manual reordering and subtree moves (`FR-7`, `FR-8`) without rewriting siblings. Re-balance lazily when gaps get too small.
- **Cycle prevention:** `child` is a tree — reject a re-parent if the new parent is a descendant of the moved task. `blocks` is a DAG — reject a new `blocks` edge that would close a cycle. Cover both with a `proptest`.
- **Aggregation (`FR-13`)** is computed by traversing `child` links from a task; cache later if needed.
- **Curated list = shallow tree.** A hand-built "list" is a `child`-tree rooted at a virtual root node; no separate membership mechanism.
- **Identity & mergeability.** `task.id` is an immutable random ULID — assigned once, never derived from content, so edits never change it and links never dangle (the jj *change-id* model, vs a content hash that would churn). For the multi-DB merge case (e.g. CDviz across repos), prefix the id with a short **per-database namespace** (`cdviz_01J9Z…`): random ULIDs already won't collide, but the namespace carries provenance and keeps short-prefix lookups unambiguous across merged sources. A future optional `content_hash` (blake3 of the task's canonical content) would let a merge detect *divergent edits* of the same id — that's a content hash used for conflict detection, kept separate from identity. Merge logic itself is post-v1, but none of these choices foreclose it.

### Query model (`FR-23`–`FR-25`)

A query is data, not code — stored in `collection.spec` (for saved ones) and evaluated by the `QueryEngine` port:

```jsonc
{
  "filter": {
    "text": "string?",                 // free-text over title/notes
    "status": ["todo"],                // any-of: draft|todo|wip|paused|done
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

`Status` is a required capability with five stored values. `blocked` is **not** a value — it's derived from unmet `blocks` deps. `[DECISION]` `SetStatus` is **not** guarded by a transition rule — any value may be set to any other directly (no stepping through intermediate states). The diagram below shows the *common* path and its usual verbs, but none of them are mechanically enforced.

```
        promote                start (Claim)            complete
draft ──────────▶ todo ───────────────────▶ wip ───────────────────▶ done
   ◀──── demote ───┘    ◀──────── release ───┘    ◀──────── reopen ────┘

                    paused: a manual, off-path state a `wip` (or any) task can move
                    to and back from freely — e.g. blocked on something external.

derived:  blocked = ∃ a `blocks` dependency whose blocker is not `done`.
          Shown as a badge in any view; (optional guard) can deny `start` while blocked.
```

- `draft` — capturable, refinable, commentable; **not** offered as work (excluded from `what-next`).
- `todo` — ready/claimable; appears in "what next" queues. *(synonym in prose: "ready".)*
- `wip` — work in progress (claimed).
- `paused` — manually set aside; not offered as work. No dedicated verb/aggregation — just another value in free rotation.
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
tda set <id> [--title ..] [--notes ..] [--status draft|todo|wip|paused|done] [--due ..]
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
- **Done when:** `FR-1`–`FR-17`, `FR-23`–`FR-25` provable against the in-memory store with no `turso`/UI/HTTP dependency anywhere in `core`/`app`.

### M2 — Persistence (Turso)
- `tda-store-turso` implementing all (async) repo ports via the `turso` crate; versioned migrations matching [§7](#7-data-model-turso-adapter).
- A test suite that runs the **same** use-case tests from M1 against the Turso store (port conformance suite).
- **Done when:** every M1 use case passes against an on-disk Turso database; round-trip export→import is identity.

### M3 — Dogfood milestone (the self-hosting roadmap)
- `tda-cli` exposing the [§9](#9-cli-surface-sketch-for-tda-cli) surface, backed by Turso, with `--json` everywhere.
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
- **Port conformance suite:** one parametrized test set run against *both* `tda-store-mem` and `tda-store-turso`. Adding a new store later means just passing this suite.
- **Snapshots (`insta`):** Markdown/JSON export — catches format regressions and proves round-trip.
- **Adapters:** thin integration tests; keep logic out of adapters so there's little to test there.

---

## 12. Definition of done — v1

v1 ships at **M3 + M4**: a keyboard-driven CLI **and** TUI over a Turso store, supporting arbitrary-depth hierarchy (single-parent tree) with a `blocks` dependency DAG, manual ordering, tags, query/search views ("what next", "due today"), assignees/claiming, the draft→done lifecycle, subtree aggregation, and Markdown/JSON round-trip — and able to host its own roadmap. M5/M6 are post-v1.

---

## 13. Open questions

These are *not* resolved by the decisions above — they need your input (Claude Code can proceed on the suggested default and you correct later):

1. **Depth limit** (`FR-4`): your note said "no depth limit?". Suggest: unlimited, but a TUI render-depth cap for sanity. OK?
2. **Denial aggregation in `decide`** ([§5a](#5a-commands-the-decider-pattern)): when several guards would deny a command, return the **first** denial (suggested — simplest, fastest) or **collect all** reasons (better UX for agents/forms)? Cheap to start with first-denial and switch later.
3. **Aggregated status (`FR-13`)**: how does a parent's status derive from `{draft, todo, wip, paused, done}` children — % `done` by count (suggested), ETA-weighted, plus an optional manual parent override?
4. ~~**`done` vs hidden**: do completed tasks stay in the tree (greyed) or move to an archive view? Suggest stay, with a filter.~~ **Resolved:** the orthogonal `Archived` capability ([§3](#3-core-concepts-glossary)/[§7](#7-data-model-turso-adapter)) — tasks stay in the store and tree, hidden from default views (`what-next`, `due-today`) via a filter, not moved to a separate structure.
5. **API trust model** (`FR-20`/`§2`): local-only/no-auth for v1 (suggested), or a token from day one?
6. **GUI framework** (`FR-22`): far off, but candidates are egui (Rust-native, pairs naturally with ratatui's immediate-mode feel) vs Tauri (web UI). No need to decide now.
7. **Batch-create syntax** (`FR-1`): indentation = depth in the batch buffer (suggested), or flat-only with manual nesting after?
8. **Multi-tag filter semantics** (`FR-24`): `tag:[a,b]` = match **all** (suggested) or **any**? And free-text (`FR-23`): substring (suggested) or FTS5 full-text later?

---

## 14. First instruction to give Claude Code

> Read `tda-spec.md`. Execute **M0** then **M1** only. Set up the Cargo workspace per §5, then implement the domain core and use cases per §3–§9 against an in-memory store, with the full test suite from §11. Do **not** add Turso, CLI, TUI, or HTTP yet. Treat the §5 dependency rule as inviolable: nothing in `tda-core`/`tda-app` may depend on an adapter or I/O crate. Stop after M1 with a green `cargo test` and a short summary of the use cases implemented.
