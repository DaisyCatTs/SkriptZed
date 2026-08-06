# Contributing

## Before changing the grammar

Read [`grammar.md`](grammar.md), especially the token-model traps. Three of them
have already produced real bugs here, and all three fail quietly rather than
loudly.

Both gates must be green:

```sh
cd tree-sitter-skript && ./node_modules/.bin/tree-sitter test
node scripts/parse-corpus.mjs --strict
```

The corpus is ~540 real scripts from SkriptLang's own repository. If a change
breaks even one, that is a genuine regression — those files are what Skript
users actually write.

Add a corpus test for anything new. `tree-sitter test --update` fills in the
expected tree, but **read what it wrote**: it records whatever the parser
currently does, including bugs.

## Before changing highlighting

Never name a colour. See [`theming.md`](theming.md) for the capture map and the
dot-prefix fallback rule.

Validate a query before shipping it — a broken one is dropped silently, and
`outline.scm` in particular is discarded entirely if any pattern lacks `@item`
or `@name`:

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

Run `scripts/fetch-docs.mjs` first, or the tests that exercise all 2,117 real
patterns will skip instead of running.

New LSP features need a check in `scripts/smoke-lsp.mjs`. Unit tests cover the
pieces; only the smoke test proves the assembled server actually answers.

## Style

Match the surrounding code. A few things this codebase does on purpose:

* **Comments explain *why*, not *what*.** The non-obvious constraints — Zed's
  ten query files, precedence beating match length, `#` being literal inside
  strings — are written down where they bite, because every one of them was
  learned the hard way.
* **Degrade, do not fail.** No network, no syntax database, no language server:
  each should cost exactly the features that need it and nothing more.
* **Be permissive in the parser, strict in the diagnostics.** A parse error
  poisons a whole file's outline, folding and indentation; a diagnostic points
  at one line.

## Commits

Conventional commits (`feat:`, `fix:`, `docs:`, `chore:`). Scope by component
where it helps: `feat(grammar):`, `fix(lsp):`.

Do **not** add `Co-Authored-By:` trailers.

## Reporting a parser bug

The most useful report is a `.sk` snippet plus what you expected:

```sh
cd tree-sitter-skript
./node_modules/.bin/tree-sitter parse broken.sk
```

If Skript itself accepts the file and this grammar reports an `ERROR`, that is a
bug regardless of how unusual the syntax looks — the corpus is full of things
that look impossible and are legal.
