# CLAUDE.md

`tda` — a keyboard-first tool to capture, organize, and refine tasks linearly
*and* as graphs/trees, for humans and AI agents. **`tda-spec.md` is the source
of truth** (decisions are marked `[DECISION]`, open ones in §13); cite its
sections when explaining choices.

**Status:** M0 (workspace skeleton) and M1 (domain core + use cases + in-memory
store) are done and green. Next is M2 (Turso persistence). M3–M6 follow §10.

## Workspace

```
crates/
  tda-core/      # domain: model, capabilities, the decider, PORTS (traits). No I/O deps.
  tda-app/       # use cases: async orchestration of core + ports
  tda-store-mem/ # adapter: in-memory store (per-capability component maps), tests + dev
```
Later adapters (per §5): `tda-store-turso`, `tda-cli`, `tda-tui`, `tda-api`,
`tda-mcp`, `tda-ui-core`.

## Inviolable: the dependency rule (§5)

`adapters → app → core`. Nothing in `tda-core` may import an adapter, a runtime,
or a framework. Enforced by `mise run lint` (→ `lint:core-no-io`, a denylist
grep over `cargo tree`). serde, derive_more, and async-trait are allowed in core
(serialization / error / async-glue, not I/O).

## Conventions

- **Decider pattern (§5a):** all task-local mutations go through
  `decide<St: ComponentStore>(&St, &Id, &Command, &DecideCtx) -> Result<Vec<Event>, Denied>`
  then `apply<St: ComponentStore>(&St, &Id, &Event)` — **capability-keyed and
  async**: a guard `get`s only the components it inspects, `apply` `set`s only
  what changed. Gated by an ordered list of guards (first denial wins).
  Graph-aware ops (move/link, cycle checks) live in `tda-app`. Extending = add a
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
