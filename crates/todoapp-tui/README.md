# todoapp-tui

The keyboard-first [ratatui](https://ratatui.rs/) TUI for `tda` — a **library
crate**, not a standalone binary. It's consumed by the `tda` binary in
`todoapp-cli`, which wires it to a real store, clock, and id generator and
launches it via `tda tui` (or by default).

Key building blocks:
- `make_svc(store, clock, ids)` — builds a `Services` handle from individual
  field references (no `Box::leak`).
- `build_visible_items(store, clock, ids, expanded)` — free async fn that
  rebuilds the visible tree; callers assign the result after borrows are
  released.
- `SystemClock` / `UlidGen` — the real `Clock`/`IdGenerator` impls (test code
  elsewhere injects fakes instead).
- `keymap.rs` — the action ↔ keybinding table (see the cheat-sheet in the
  [root README](https://github.com/davidB/todoapp#tui)); `config.rs` and
  `keymap.rs` both load their tables from the same `tui.toml`.

To install and run `tda` (the screenshot and full keybinding list live
there), get [`todoapp-cli`](https://crates.io/crates/todoapp-cli)
(`cargo install todoapp-cli`) — full docs at the
[project repo](https://github.com/davidB/todoapp).
