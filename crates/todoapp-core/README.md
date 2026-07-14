# todoapp-core

The domain core of `tda`: entities, capabilities, and the ports (traits) that
adapters implement. **No I/O, no runtime, no framework dependencies** — this
crate compiles without touching a filesystem, a network, or an async runtime
executor.

- Task = a stable identity (id + timestamps) plus a set of *capabilities*
  (`Status`, `Notes`, `Schedule`, `Estimate`, `Tags`, `Assignment`,
  `Recurrence`, `IssueRef`, `Attachments`, `Archived`, `TimeLog`) — composition
  instead of a fixed field list. See [`tda-spec.md` §3](https://github.com/davidB/todoapp/blob/main/tda-spec.md#3-core-concepts-glossary).
- The **decider pattern**: `decide` (capability-keyed guards, first denial
  wins) → `Event`s → `apply` (capability-keyed state changes). See
  [`tda-spec.md` §5a](https://github.com/davidB/todoapp/blob/main/tda-spec.md#5a-commands-the-decider-pattern).
- Ports (`ComponentStore`, `TaskEntityStore`, …) are defined here and
  implemented by adapters (`todoapp-store-mem`, `todoapp-store-turso`).

This crate sits at the bottom of the dependency rule:
`adapters → app → core`. Nothing above may leak into it.

This is a library crate — it has no binary of its own. To install and run
`tda`, get [`todoapp-cli`](https://crates.io/crates/todoapp-cli)
(`cargo install todoapp-cli`). Full docs at the
[project repo](https://github.com/davidB/todoapp).
