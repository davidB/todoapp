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
  todoapp-cli/         # adapter: the `tda` binary — CLI, plus the ratatui TUI
                       #   behind a default-on `tui` feature (its own `tui`
                       #   module). DB via resolve_db_path (see below).
```
Later adapters (per §5): `todoapp-api`, `todoapp-mcp`, `todoapp-ui-core`.

### todoapp-cli / TUI conventions (M4)
- The TUI lives in `todoapp-cli/src/tui/` behind the `tui` cargo feature
  (ratatui/crossterm/etc. are optional); `--no-default-features` builds a
  headless CLI. Command dispatch is `command::run_command(&svc, &req) -> Reply`
  (writes output to a buffer), shared by the direct CLI path and the TUI server.
- **Concurrent access (single-writer socket).** turso takes an exclusive
  cross-process db lock, so while `tda tui` runs it is the only process that can
  open the db. It binds a Unix socket next to the db file (`tda.sock`) and, once
  per terminal-poll cycle, drains pending connections: runs the sent command
  in-process and `rebuild()`s so external changes show live. Other `tda`
  invocations send their command to that socket (`ipc::send`); with no server
  (or connection refused) they open the db directly. Transport is in
  `todoapp-cli/src/ipc.rs` (unix; server bits gated on `tui`).
- `AppState` owns `TursoStore`; `make_svc(store, clock, ids)` builds `Services` from
  individual field references (field-level borrows, no `Box::leak`).
- `build_visible_items(store, clock, ids, expanded)` is a free async fn for tree rebuild;
  the caller assigns the result to `self.items` after borrows are released.
- `SystemClock` (`SystemTime` + jiff) + `UlidGen` (ulid crate) are the real
  impls, in `todoapp-cli/src/svc.rs` (always compiled — the headless CLI needs
  them without ratatui), alongside `make_svc`.
- DB path (`resolve_db_path`, shared by CLI + TUI): `--db` flag > nearest
  ancestor `.tda/tda.db` (created by `tda db init`, git-style walk from cwd)
  > OS-standard data dir, e.g. `~/.local/share/tda/tda.db` on Linux.
  Config is split by scope, both in the OS-standard config dir, with path
  resolution + generic TOML parsing colocated in `todoapp-config`
  (`config_path`/`tui_config_path`/`read_toml`, returning `toml::Value`) so
  typed schemas stay with their owning crate: `config.toml` for cross-app
  settings shared with the CLI (currently just `[workspaces]`), `tui.toml`
  for TUI-only settings (columns/schedule/status/styles/keybindings/behavior,
  parsed into the `tui` module's `Config`/`Keymap` via `serde::Deserialize` over
  the shared `toml::Value`). No env var overrides (dropped as YAGNI).
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
