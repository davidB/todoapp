# CLAUDE.md

`tda` — a keyboard-first tool to capture, organize, and refine tasks linearly
*and* as graphs/trees, for humans and AI agents. **`tda-spec.md` is the source
of truth** (decisions are marked `[DECISION]`, open ones in §13); cite its
sections when explaining choices.

**Status:** M0–M2 done and green. M4 (TUI) delivered next (before M3 CLI, per user decision). M3/M5/M6 follow §10.

## Workspace

```
crates/
  todoapp-core/        # domain: model, capabilities, the decider, PORTS (traits). No I/O deps.
  todoapp-app/         # use cases: async orchestration of core + ports
  todoapp-store-mem/   # adapter: in-memory store (per-capability component maps), tests + dev
  todoapp-store-turso/ # adapter: Turso/SQLite persistence (M2)
  todoapp-conformance/ # shared conformance test suite (macro runs against both stores)
  todoapp-tui/         # adapter: ratatui TUI binary `tda` (M4) — DB at ~/.local/share/tda/tda.db
```
Later adapters (per §5): `todoapp-cli`, `todoapp-api`, `todoapp-mcp`, `todoapp-ui-core`.

### todoapp-tui conventions (M4)
- `AppState` owns `TursoStore`; `make_svc(store, clock, ids)` builds `Services` from
  individual field references (field-level borrows, no `Box::leak`).
- `build_visible_items(store, clock, ids, expanded)` is a free async fn for tree rebuild;
  the caller assigns the result to `self.items` after borrows are released.
- `SystemClock` (chrono `Local::now`) + `UlidGen` (ulid crate) are the real impls.
- DB path: `$TDA_DB` env var or `~/.local/share/tda/tda.db`.
- Actor for claim: fixed `Id("me")` — single-user, no auth (spec §2/§13 Q5).

## Inviolable: the dependency rule (§5)

`adapters → app → core`. Nothing in `todoapp-core` may import an adapter, a runtime,
or a framework. Enforced by `mise run lint` (→ `lint:core-no-io`, a denylist
grep over `cargo tree`). serde, derive_more, and async-trait are allowed in core
(serialization / error / async-glue, not I/O).

## Conventions

- **Decider pattern (§5a):** all task-local mutations go through
  `decide<St: ComponentStore>(&St, &Id, &Command, &DecideCtx) -> Result<Vec<Event>, Denied>`
  then `apply<St: ComponentStore>(&St, &Id, &Event)` — **capability-keyed and
  async**: a guard `get`s only the components it inspects, `apply` `set`s only
  what changed. Gated by an ordered list of guards (first denial wins).
  Graph-aware ops (move/link, cycle checks) live in `todoapp-app`. Extending = add a
  `Component` type + its `Command`/`Event` variant + a guard + an `apply` arm.
- **Async boundary (§5):** repository ports are `async` traits (`async-trait`,
  `?Send`), and use cases are `async`. `decide`/`apply` are async **over the
  `ComponentStore` port** (defined in core, so the dependency rule holds); query
  evaluation hoists its async component loads before the sync sort. Recursive
  walks are iterative (no boxed async recursion).
- **Storage (§7):** capability-keyed components, one per capability, keyed by
  task id; *presence is the capability*. `ComponentStore::{get,set,remove}::<C>`
  touch one capability at a time (no whole-task aggregate); `TaskEntityStore`
  holds the minimal id+timestamps entity. `Services::snapshot` assembles a
  read-only `TaskSnapshot` for query results / export only — never fed back to
  `decide`/`apply`.
- Errors: `derive_more` (`Display`/`Error`/`From`) in libs, `anyhow` in bins.
  IDs: ULID (opaque `Id` string; tests use a sequence). Time/ids are injected
  (`Clock`/`IdGenerator`) for deterministic tests.

## Commands (mise)

- `mise run build` · `mise run test` · `mise run lint` · `mise run format`
- `mise run ci` — the full gate (lint + test + `fmt --check`); run before commit.
- Snapshot tests use `insta` (`cargo insta review` to accept changes).
- Per global config: use `rg`/`fd`, not grep/find.
