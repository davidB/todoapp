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

- **Decider pattern (§5a):** all task-local mutations go through pure
  `decide(&TaskState, &Command, &DecideCtx) -> Result<Vec<Event>, Denied>` then
  `apply(&mut TaskState, &Event)`, gated by an ordered list of guards (first
  denial wins). Graph-aware ops (move/link, cycle checks) live in `tda-app`.
  Extending = add a guard + an `apply` arm; touch nothing else.
- **Async boundary (§5):** repository ports are `async` traits (`async-trait`,
  `?Send`), and use cases are `async`. The `decide`/`apply` core and query
  evaluation stay **pure & sync** — hoist async loads before any sort. Recursive
  walks are iterative (no boxed async recursion).
- **Storage (§7):** one component per capability, keyed by task id; *presence is
  the capability*. `TaskRepository::load(id, Projection)` assembles only what a
  caller needs (`Row` = title+status, `Full` = all); `save` decomposes back.
- Errors: `derive_more` (`Display`/`Error`/`From`) in libs, `anyhow` in bins.
  IDs: ULID (opaque `Id` string; tests use a sequence). Time/ids are injected
  (`Clock`/`IdGenerator`) for deterministic tests.

## Commands (mise)

- `mise run build` · `mise run test` · `mise run lint` · `mise run format`
- `mise run ci` — the full gate (lint + test + `fmt --check`); run before commit.
- Snapshot tests use `insta` (`cargo insta review` to accept changes).
- Per global config: use `rg`/`fd`, not grep/find.
