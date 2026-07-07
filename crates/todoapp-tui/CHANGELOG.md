# Changelog

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

