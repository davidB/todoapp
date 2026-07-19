# Changelog

## [0.7.0] - 2026-07-19

### Bug Fixes

- Don't drop typed text on alt+right/left in add dialog ([fa667e0](https://github.com/davidB/todoapp/commit/fa667e08b3f6865fc3d7122f18cb90524eb23e0d))

### Documentation

- Document add-dialog scratchpad and clear_input key ([efc7053](https://github.com/davidB/todoapp/commit/efc705369a667c6a54a774c0f14f2bf3318ea32c))

### Features

- Configure visible columns from the TUI, synced to tui.toml ([534b811](https://github.com/davidB/todoapp/commit/534b8114188a060ac1163796e12d9fbc38b23604))
- Remember session state (expansion, cursor, details pane) ([e0834e2](https://github.com/davidB/todoapp/commit/e0834e21f35ba3ec3959b25f0536270c13fb6c81))
- Keep cancelled add-task draft as scratchpad + clear_input key ([10b307c](https://github.com/davidB/todoapp/commit/10b307c7fbd274d22c43b26624f556d6be074ae7))
- Add submit_on_enter option for the add/input dialog ([e7a17be](https://github.com/davidB/todoapp/commit/e7a17be27def3437170ccedbf64351ad26ce0f61))
- Multi-select tasks for batch operations ([f7af3f8](https://github.com/davidB/todoapp/commit/f7af3f854a8b42a3a93ff3f5465a31b965c3016c))


## [0.6.0] - 2026-07-15

### Documentation

- Rework the README ([e00a392](https://github.com/davidB/todoapp/commit/e00a39241f7f42cfeea8ba629a53b10ec070f9ec))

### Features

- [**breaking**] Chaining `add task` enabled by default ([92f5983](https://github.com/davidB/todoapp/commit/92f5983b85d7c9c9b6e5efd86ec6fcba6b2593ee))
- [**breaking**] `tui` becomes the default subcommand (previously `help`) ([72fb15e](https://github.com/davidB/todoapp/commit/72fb15e2a8b58eedccaae28bf71fd7883affc10b))

### Revert

- Chaining `add task` disabled by default ([9abe9ec](https://github.com/davidB/todoapp/commit/9abe9eca214a70faf031601587516a831414a664))


## [0.5.0] - 2026-07-14

### Documentation

- Document concurrent CLI/TUI, agent identity, and skill install ([eec7837](https://github.com/davidB/todoapp/commit/eec78378d3e22a37a4b80c42476800d72bb59f9d))

### Features

- Shorten ancestor breadcrumb titles to first line ([a9535f2](https://github.com/davidB/todoapp/commit/a9535f20ed6cc74b41535289cb838cb5db8efe27))
- Non-modal live details pane, toggled with `v` ([861804e](https://github.com/davidB/todoapp/commit/861804e6cdcc414e97609575df853bd7d0b56c18))
- Proxy CLI commands to a running TUI over the socket ([ddbd4fe](https://github.com/davidB/todoapp/commit/ddbd4fe6f9c7839d1ff953fa3773e6e7c06c0aa7))
- TUI serves CLI commands over a Unix socket ([315bde1](https://github.com/davidB/todoapp/commit/315bde165c6f88a0e84f8f8f63229f98a8195d9f))

### Refactoring

- Share breadcrumb rendering between list view and details pane ([6eb6a6a](https://github.com/davidB/todoapp/commit/6eb6a6a220520df3d7a73225ac6aa9aa4a077ab4))
- Breadcrumb crumbs reuse render_inline (DRY) ([3b28516](https://github.com/davidB/todoapp/commit/3b2851660d0f4aef16c095de2850072787c286c1))
- Extract Cmd + run_command into command.rs, add ipc wire types ([f6342d5](https://github.com/davidB/todoapp/commit/f6342d5c656b46a0e9ac9c32820e1903b21846e4))
- Merge todoapp-tui crate into todoapp-cli behind `tui` feature ([72d03ae](https://github.com/davidB/todoapp/commit/72d03ae4e8a0b769554de00c9dae8a45b3c6d7b4))


## [0.4.1] - 2026-07-14

### Bug Fixes

- Rework READMEs & Cargo.toml to be work on github & crates.io ([5e3f76f](https://github.com/davidB/todoapp/commit/5e3f76fafd6efb00cce50e2081a8fb338e1a6558))


## [0.4.0] - 2026-07-12

### Documentation

- Update README ([64363ee](https://github.com/davidB/todoapp/commit/64363ee52dd02419a0356fad34fe5414140690c4))
- Add animated gif to illustrate ([ae4ab4b](https://github.com/davidB/todoapp/commit/ae4ab4b3f1255bbff7049e8e57cb0868d51fd8a2))
- Fix the `brew` install instruction ([46bdc6a](https://github.com/davidB/todoapp/commit/46bdc6aac6684b4dfc5bf70207fea4af618dd674))
- Absolute url for assets in README to be visible outside of the repo (eg crates.io) ([830584e](https://github.com/davidB/todoapp/commit/830584e978b13bce506a9360185a73129e073279))

### Features

- Add --parent to `tda import` ([ca8ce7d](https://github.com/davidB/todoapp/commit/ca8ce7dcf24a89d8e7c2854986f16b93780a8808))
- Add delete-task command (core/app/cli/tui) ([b74b534](https://github.com/davidB/todoapp/commit/b74b534f6bb59e4d7eebf119f91802205109dd20))

### Refactoring

- Merge config.toml + keybindings.toml into tui.toml ([2b660ef](https://github.com/davidB/todoapp/commit/2b660ef6b1e48ffc7d8e821bf9e3af2ad43e7d68))


## [0.1.0] - 2026-07-07

### Documentation

- Add README install/usage instructions, wire up Homebrew tap publishing ([07b09a4](https://github.com/davidB/todoapp/commit/07b09a4af7f9b48cdb1553f12da4de44801589f5))
- Add READMEs ([09c6d8d](https://github.com/davidB/todoapp/commit/09c6d8d8b697d58130609a8d80a6bf852738f3e7))

### Features

- Add release-plz + cargo-dist CI/CD ([52a5189](https://github.com/davidB/todoapp/commit/52a51892846bb93680e708cf48e8afbfbae195b9))

### Refactoring

- Remove code duplication ([b38a1fb](https://github.com/davidB/todoapp/commit/b38a1fba54dea390f23c3306452fc6fc0e0241bc))
- Rename crates from tda-* to todoapp-* prefix ([10c959f](https://github.com/davidB/todoapp/commit/10c959febdf0c25cba4c4b2f63ec4713b7fc22f5))

