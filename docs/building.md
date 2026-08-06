# Building

## Prerequisites

| Tool | Why |
|---|---|
| Rust (stable) + `wasm32-wasip2` | The extension is a WASM component; the language server is native |
| Node 20+ | `tree-sitter-cli` and the test harnesses |
| git | Zed fetches the grammar by cloning it, even locally |

```sh
rustup target add wasm32-wasip2
cd tree-sitter-skript && npm install
```

Zed downloads **wasi-sdk 25** itself to compile the grammar. Set `WASI_SDK_PATH`
to override that.

## Grammar

```sh
cd tree-sitter-skript
./node_modules/.bin/tree-sitter generate      # after ANY change to grammar.js
./node_modules/.bin/tree-sitter test
```

`src/parser.c` and friends are generated but committed — Zed compiles them
directly and never runs the CLI. CI fails if they are stale.

## Language server

```sh
cd language-server
cargo build --release -p skript-lsp
cargo test
```

`scripts/fetch-docs.mjs` places `vendor/docs.json` so the pattern-engine tests run
against all 2,117 real Skript patterns. It is GPL-3.0 and gitignored; without it
those tests skip rather than fail.

## Extension

```sh
cd extension
cargo build --release --target wasm32-wasip2
```

A host-target build proves nothing — Zed only ever loads `wasm32-wasip2`.

## Installing it in Zed

Zed resolves `[grammars.skript]` by `git fetch --depth 1 origin <rev>`, so the
grammar needs a real commit even behind a `file://` URL:

```sh
node scripts/dev-setup.mjs
```

That regenerates the parser, runs both grammar gates, commits `tree-sitter-skript/`,
and rewrites `rev` in `extension.toml` to match. Then, in Zed:

1. Command palette → **`zed: install dev extension`**
2. Pick the `extension/` directory.

Re-run `dev-setup.mjs` after every grammar change, or Zed keeps building the
previously committed parser.

## Debugging

* `zed --foreground` forwards the extension's stdout to your terminal.
* `zed: open log` for Zed's own log, including query-compilation warnings.
* `dev: open language server logs` for the LSP traffic.
* `dev: open highlights tree view` shows which captures and semantic tokens are
  actually landing on each span.

`skript-lsp` writes **only** protocol traffic to stdout; logs go to stderr. A
stray `println!` in the server corrupts the message stream.

## Verifying a change

```sh
cd tree-sitter-skript && ./node_modules/.bin/tree-sitter test
node scripts/parse-corpus.mjs --strict
cd language-server && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
node scripts/smoke-lsp.mjs
cd extension && cargo build --release --target wasm32-wasip2
```

Two traps that have already produced false passes here: `tree-sitter generate |
head` hides a non-zero exit, and passing ~540 paths as argv silently overflows on
Windows. `scripts/parse-corpus.mjs` asserts the parsed count matches the
discovered file count before believing any result — keep that assertion.
