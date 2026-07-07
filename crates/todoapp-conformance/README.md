# todoapp-conformance

A shared port-conformance test suite: one set of behavioral tests, with
deterministic fixtures (injected `Clock`/`IdGenerator`, not wall-clock time or
random ids), run as a macro against **every** store adapter
([`todoapp-store-mem`](../todoapp-store-mem),
[`todoapp-store-turso`](../todoapp-store-turso)).

Depends only on [`todoapp-core`](../todoapp-core). The goal: a new store
adapter is trustworthy the moment it passes this suite, with no adapter-specific
test-writing required. See [`tda-spec.md` §11](../../tda-spec.md#11-testing-strategy).
