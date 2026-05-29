# Installing Rotom

## Prerequisites

- A recent stable Rust toolchain (edition 2024, Rust 1.85 or newer). Install via [rustup](https://rustup.rs).
- Network access on the first build. The build downloads the latest script-command
  database and embeds it as a fallback copy.

## Install the CLI

```bash
cargo install --path .
```

This builds the `rotom` binary and places it on your `PATH`. Verify with:

```bash
rotom --version
```

## Build without installing

```bash
cargo build --release
```

The binary is written to `target/release/rotom`.

## Language server (`rotom-lsp`)

The LSP server powers editor features (diagnostics, completion, hover, go-to-definition,
inlay hints, signature help, code lenses).

```bash
cargo install --path rotom-lsp
```

Or build it alongside the CLI:

```bash
cargo build --release --workspace
```

The server binary is written to `target/release/rotom-lsp`.

## Editor extensions

TBD — VS Code, Zed, and Neovim integrations are published separately and will be
documented here once their repository is public. Tree-sitter grammar and highlighting
live in [`tree-sitter-rotom`](https://github.com/KalaayPT/tree-sitter-rotom).
