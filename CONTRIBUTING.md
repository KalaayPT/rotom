# Contributing

Any contributions to the Rotom project are very welcome! I don't have much time to work on this myself, so I appreciate any help. Please follow the guidelines below to make the process smoother.

The To-Do list in the README can be used as inspiration for what to work on.

## Where Things Live

- `rotom` is the compiler, decompiler, CLI, project tooling, and LSP server.
- `rotom-lsp/` within the rotom repo contains editor-facing LSP features such as diagnostics,
  completion, hover, go-to-definition, inlay hints, and code lenses.
- [`uxie`](https://github.com/KalaayPT/uxie) handles Gen 4 project/workspace data. If the logic belongs to ROM or
  workspace data access, it probably belongs there instead of rotom.
- [`tree-sitter-rotom`](https://github.com/KalaayPT/tree-sitter-rotom) owns the grammar and highlighting queries used by editors.
- Editor extensions for VS Code, Zed, and Neovim are thin wrappers around the LSP server and can be found in `rotom-extensions` (unpublished).

## Building

```bash
cargo build --workspace
```

The build currently downloads the latest script-command database so it can embed
a fallback copy. 

## Testing

Test your changes before sending them.

```bash
cargo test -p rotom
cargo test -p rotom-lsp
cargo clippy --workspace --all-targets
```

For grammar changes, test the grammar repo too:

```bash
tree-sitter test
```

### Test fixtures

Fixtures live under `tests/fixtures/` and are **not** committed (the directory is
gitignored). The test harness obtains them as follows:

- **Decomp fixtures** (`pokeplatinum`, `pokeheartgold`) are cloned automatically at
  pinned commits the first time you run the tests. This needs `git` and network
  access on the first run; later runs reuse the local clones. Pins live in
  `tests/common/fixture_pins.rs`.
  - note: if these ever fail, it can be related to the db being more up to date than the pinned commit. try bumping the commit if that ever happens, and make sure to include this in your PR.
- **DSPRE fixtures** must be supplied manually under
  `tests/fixtures/dspre/<game>_DSPRE_contents/`. Tests that need them are skipped
  when the tree is absent, so a fresh clone still passes the rest of the suite.

The byte-matching lifecycle tests decompile and recompile the full fixture sets, so
they take a while. Run them on release, they are a lot faster than in debug:

```bash
cargo test -p rotom --release
```

If you skip them, say so in your PR and list the commands you did run.

## Pull Requests

- Explain what changed and why.
- List the tests you ran.
- Mention any related changes needed in `uxie`, `tree-sitter-rotom`, or editor
  extension repos.
- Avoid unrelated cleanup in the same PR.
