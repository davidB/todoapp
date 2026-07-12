# Changelog

## [0.3.0] - 2026-07-12

### Bug Fixes

- New tasks start on first enabled status, not hardcoded draft ([e6c5010](https://github.com/davidB/todoapp/commit/e6c5010232ef4484dd1584ba75e004427e56c2ea))

### Features

- Add quick-assign action + `@mention` title syntax (FR-32) ([9b170b3](https://github.com/davidB/todoapp/commit/9b170b32eb0cc03d32f7559bd971c12a92e512fa))
- Add chain_add config for batch task insertion ([c1f8e80](https://github.com/davidB/todoapp/commit/c1f8e809595966e9dac16691e8648423a99dc686))
- Add delete-task command (core/app/cli/tui) ([b74b534](https://github.com/davidB/todoapp/commit/b74b534f6bb59e4d7eebf119f91802205109dd20))
- Keep tree column at least 30% wide, hide columns to fit ([8030291](https://github.com/davidB/todoapp/commit/803029187081858f479848fd8c38589566fce528))

### Refactoring

- Merge config.toml + keybindings.toml into tui.toml ([2b660ef](https://github.com/davidB/todoapp/commit/2b660ef6b1e48ffc7d8e821bf9e3af2ad43e7d68))


## [0.1.0] - 2026-07-07

### Bug Fixes

- The test's expectation was swapped, not the keymap. alt+left/alt+right follow standard tree-editor convention (right=indent, left=outdent); updated the assertion to match. ([1aa36fb](https://github.com/davidB/todoapp/commit/1aa36fb7c16154cb085e7a5c78985316805bd97b))

### Documentation

- Add READMEs ([09c6d8d](https://github.com/davidB/todoapp/commit/09c6d8d8b697d58130609a8d80a6bf852738f3e7))

### Features

- Overrun-aware eta projection, suppress eta when nothing to project from ([82684bf](https://github.com/davidB/todoapp/commit/82684bf6951697a928dc3e9c9d2bb124d9e14694))
- Select/yank title text and paste, with a resilient clipboard ([84f5a2c](https://github.com/davidB/todoapp/commit/84f5a2cd0c4f77667ee62ec9d2726e4989db2505))
- Real text editing for the add/edit dialogs ([86f3530](https://github.com/davidB/todoapp/commit/86f35302721a71d65180abbe42cb49cb9b36c4ca))
- Render title/notes as Markdown, add a detail view ([821e575](https://github.com/davidB/todoapp/commit/821e575a2b007b4f65851ec8654f91aae4597e11))
- Add release-plz + cargo-dist CI/CD ([52a5189](https://github.com/davidB/todoapp/commit/52a51892846bb93680e708cf48e8afbfbae195b9))

### Refactoring

- Remove code duplication ([b38a1fb](https://github.com/davidB/todoapp/commit/b38a1fba54dea390f23c3306452fc6fc0e0241bc))
- Rename crates from tda-* to todoapp-* prefix ([10c959f](https://github.com/davidB/todoapp/commit/10c959febdf0c25cba4c4b2f63ec4713b7fc22f5))

