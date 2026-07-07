# todoapp-core

The domain core of `tda`: entities, capabilities, and the ports (traits) that
adapters implement. **No I/O, no runtime, no framework dependencies** — this
crate compiles without touching a filesystem, a network, or an async runtime
executor.

- Task = a stable identity (id + timestamps) plus a set of *capabilities*
  (`Status`, `Notes`, `Schedule`, `Estimate`, `Tags`, `Assignment`,
  `Recurrence`, `IssueRef`, `Attachments`, `Archived`, `TimeLog`) — composition
  instead of a fixed field list. See [`tda-spec.md` §3](../../tda-spec.md#3-core-concepts-glossary).
- The **decider pattern**: `decide` (capability-keyed guards, first denial
  wins) → `Event`s → `apply` (capability-keyed state changes). See
  [`tda-spec.md` §5a](../../tda-spec.md#5a-commands-the-decider-pattern).
- Ports (`ComponentStore`, `TaskEntityStore`, …) are defined here and
  implemented by adapters (`todoapp-store-mem`, `todoapp-store-turso`).

This crate sits at the bottom of the dependency rule:
`adapters → app → core`. Nothing above may leak into it — this is enforced by
`mise run lint` (`lint:core-no-io`, a `cargo tree` denylist).

See the root [README](../../README.md) and [`tda-spec.md`](../../tda-spec.md)
for the full picture; [`CLAUDE.md`](../../CLAUDE.md) for contributor
conventions.
