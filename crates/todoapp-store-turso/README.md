# todoapp-store-turso

The persistence adapter for `tda`, built on [Turso](https://turso.tech/)
(an embedded, SQLite-compatible engine). Implements the same `todoapp-core`
store ports as `todoapp-store-mem` and is exercised against the identical
`todoapp-conformance` suite, so the two stores are interchangeable from the
app's point of view.

Data model: capability-keyed tables, one per capability, keyed by task id —
see [`tda-spec.md` §7](https://github.com/davidB/todoapp/blob/main/tda-spec.md#7-data-model-turso).

The default `tda` binary points this store at the OS-standard data dir (e.g.
`~/.local/share/tda/tda.db` on Linux).

This is a library crate — it has no binary of its own. To install and run
`tda`, get [`todoapp-cli`](https://crates.io/crates/todoapp-cli)
(`cargo install todoapp-cli`). Full docs at the
[project repo](https://github.com/davidB/todoapp).
