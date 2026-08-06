# Contributing

Bug reports, grammar fixes and addon syntax are all welcome.

The single most useful contribution is a `.sk` snippet that Skript accepts and
this grammar does not. If Skript itself parses the file and this grammar reports
an `ERROR`, that is a bug regardless of how unusual the syntax looks — the
upstream corpus is full of things that look impossible and are legal.

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

## Running it before it is published

Zed resolves `[grammars.skript]` with `git fetch --depth 1 origin <rev>`, so the
grammar needs a real commit even behind a `file://` URL:

```sh
git clone https://github.com/DaisyCatTs/SkriptZed.git
cd SkriptZed
cd tree-sitter-skript && npm install && cd ..
rustup target add wasm32-wasip2
node scripts/dev-setup.mjs
```

`dev-setup.mjs` regenerates the parser, runs both grammar gates, commits
`tree-sitter-skript/`, and rewrites `rev` in `extension.toml` to match. **Do not
commit that rewrite** — the version checked in points at GitHub and is what every
user installs from.

Then, in Zed: command palette → **`zed: install dev extension`** → pick the
**`extension/`** directory (not the repo root).

A dev extension overrides the published one. Re-run `dev-setup.mjs` after every
grammar change, or Zed keeps building the previously committed parser.

Build the language server too, and point Zed at it:

```sh
cd language-server && cargo build --release -p skript-lsp
```

```json
{ "lsp": { "skript-lsp": { "binary": { "path": "/abs/path/to/skript-lsp" } } } }
```

## Verifying a change

Everything, in order:

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

`node scripts/coverage.mjs` reports how much of a file the grammar and the
language server between them actually explain. Useful for judging a highlighting
change; it is a measurement, not a gate.

## Before changing the grammar

Read [`docs/grammar.md`](docs/grammar.md), especially the token-model traps.
Three of them have already produced real bugs here, and all three fail quietly
rather than loudly.

```sh
cd tree-sitter-skript
./node_modules/.bin/tree-sitter generate      # after ANY change to grammar.js
./node_modules/.bin/tree-sitter test
node ../scripts/parse-corpus.mjs --strict
```

The corpus is ~540 real scripts from SkriptLang's own repository. If a change
breaks even one, that is a genuine regression — those files are what Skript users
actually write.

`src/parser.c` and friends are generated but committed, because Zed compiles
them directly and never runs the CLI. CI fails if they are stale.

Add a corpus test for anything new. `tree-sitter test --update` fills in the
expected tree, but **read what it wrote** — it records whatever the parser
currently does, including bugs. Check the diff contains only what you intended.

## Before changing highlighting

Never name a colour. See [`docs/theming.md`](docs/theming.md) for the capture map
and the dot-prefix fallback rule.

Validate a query before shipping it — a broken one is dropped silently, and
`outline.scm` in particular is discarded entirely if any pattern lacks `@item` or
`@name`:

```sh
cd tree-sitter-skript
./node_modules/.bin/tree-sitter query ../extension/languages/skript/highlights.scm some.sk
```

Do not add `folds.scm` or `locals.scm`. Zed does not read them. Folding comes
from indentation and from the language server.

## Before changing the language server

```sh
cd language-server
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cd .. && node scripts/smoke-lsp.mjs
```

Run `node scripts/fetch-docs.mjs` first, or the tests that exercise every real
pattern will skip instead of running.

New LSP features need a check in `scripts/smoke-lsp.mjs`. Unit tests cover the
pieces; only the smoke test proves the assembled server actually answers.

## Debugging

* `zed --foreground` forwards the extension's stdout to your terminal.
* `zed: open log` for Zed's own log, including query-compilation warnings.
* `dev: open language server logs` for the LSP traffic.
* `dev: open highlights tree view` shows which captures and semantic tokens are
  actually landing on each span.

`skript-lsp` writes **only** protocol traffic to stdout; logs go to stderr. A
stray `println!` in the server corrupts the message stream.

## Style

Match the surrounding code. A few things this codebase does on purpose:

* **Comments explain *why*, not *what*.** The non-obvious constraints — Zed's ten
  query files, token precedence beating match length, `#` being literal inside
  strings — are written down where they bite, because every one of them was
  learned the hard way.
* **Degrade, do not fail.** No network, no syntax database, no language server:
  each should cost exactly the features that need it and nothing more.
* **Be permissive in the parser, strict in the diagnostics.** A parse error
  poisons a whole file's outline, folding and indentation; a diagnostic points at
  one line.

## Commits and pull requests

Conventional commits (`feat:`, `fix:`, `docs:`, `chore:`). Scope by component
where it helps: `feat(grammar):`, `fix(lsp):`.

Do **not** add `Co-Authored-By:` trailers.

If your change is visible to somebody *using* the extension, add a line to
`CHANGELOG.md` under `## [Unreleased]`.

## Reporting a parser bug

```sh
cd tree-sitter-skript
./node_modules/.bin/tree-sitter parse broken.sk
```

Include that output, the snippet, and what you expected. There is an
[issue template](https://github.com/DaisyCatTs/SkriptZed/issues/new/choose) that
asks for exactly this.
