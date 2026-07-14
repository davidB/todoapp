# todoapp-config

Shared config helpers for `tda`, used by both the CLI and TUI: DB path
resolution (`--db` flag > nearest ancestor `.tda/tda.db` > OS-standard data
dir) and generic TOML parsing for the two config files in the OS-standard
config dir — `config.toml` (cross-app settings, e.g. `[workspaces]`) and
`tui.toml` (TUI-only settings). Typed schemas for each file's contents stay
with their owning crate; this crate only owns path resolution and parsing.

This is a library crate — it has no binary of its own. To install and run
`tda`, get [`todoapp-cli`](https://crates.io/crates/todoapp-cli)
(`cargo install todoapp-cli`). Full docs at the
[project repo](https://github.com/davidB/todoapp).
