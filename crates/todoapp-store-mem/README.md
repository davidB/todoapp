# todoapp-store-mem

An in-memory implementation of the [`todoapp-core`](../todoapp-core) store
ports (`ComponentStore`, `TaskEntityStore`), used for fast unit tests and
local development. Storage is capability-keyed — one map per capability,
keyed by task id — mirroring the on-disk layout in
[`todoapp-store-turso`](../todoapp-store-turso) so behavior stays consistent
across adapters.

Exercised against the same [`todoapp-conformance`](../todoapp-conformance)
suite as every other store, so it can't silently drift from the persisted
adapter's semantics.

See the root [README](../../README.md) and [`tda-spec.md` §7](../../tda-spec.md#7-data-model-turso)
for the data model.
