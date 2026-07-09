# todoapp-store-turso

The persistence adapter for `tda`, built on [Turso](https://turso.tech/)
(an embedded, SQLite-compatible engine). Implements the same
[`todoapp-core`](../todoapp-core) store ports as
[`todoapp-store-mem`](../todoapp-store-mem) and is exercised against the
identical [`todoapp-conformance`](../todoapp-conformance) suite, so the two
stores are interchangeable from the app's point of view.

Data model: capability-keyed tables, one per capability, keyed by task id —
see [`tda-spec.md` §7](../../tda-spec.md#7-data-model-turso).

The default `tda` binary ([`todoapp-cli`](../todoapp-cli)) points this store
at the OS-standard data dir (e.g. `~/.local/share/tda/tda.db` on Linux).
