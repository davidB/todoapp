# todoapp-app

The use-case layer of `tda`: async orchestration on top of `todoapp-core`.
This is where `decide`/`apply` calls get sequenced into full operations
(create, move, claim, link, query, …) against whatever store adapter is
wired in.

- Repository ports are `async` traits (`async-trait`, `?Send`); use cases here
  are `async` too — the only place in the workspace where the async boundary
  is crossed above the core. See
  [`tda-spec.md` §5](https://github.com/davidB/todoapp/blob/main/tda-spec.md#5-architecture).
- Graph-aware operations (move/reparent, cycle checks, subtree walks) live
  here rather than in core, since they need the store port to traverse.
- Depends only on `todoapp-core` for production code; store adapters are
  dev-dependencies used in this crate's own tests.

This is a library crate — it has no binary of its own. To install and run
`tda`, get [`todoapp-cli`](https://crates.io/crates/todoapp-cli)
(`cargo install todoapp-cli`). Full docs at the
[project repo](https://github.com/davidB/todoapp).
