# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Read first

`PROGRESS.md` at the repo root is the live status document: what is built, what
is verified, the source-verified Zed/Skript facts the design rests on, and the
ordered next steps. `docs/PLAN.md` is the approved plan. Read `PROGRESS.md`
before starting work — most "how do I…" and "why is it like this?" questions are
already answered there, and re-deriving that research is expensive.

## Commands

All grammar commands run from `tree-sitter-skript/`; the CLI is a devDependency
of that package, not a global tool.

```sh
cd tree-sitter-skript

# Regenerate src/parser.c after ANY change to grammar.js. Never edit parser.c.
./node_modules/.bin/tree-sitter generate

./node_modules/.bin/tree-sitter test                  # all corpus tests
./node_modules/.bin/tree-sitter test -f "Block comment"   # one test by name
./node_modules/.bin/tree-sitter test --update          # rewrite expected trees

./node_modules/.bin/tree-sitter parse file.sk          # dump a parse tree
./node_modules/.bin/tree-sitter query ../extension/languages/skript/highlights.scm file.sk
```

The second, larger gate is the upstream corpus — ~540 real `.sk` files from
`SkriptLang/Skript`, which must all parse with zero ERROR/MISSING nodes:

```sh
node scripts/parse-corpus.mjs            # summary + first failures
node scripts/parse-corpus.mjs --all      # every failing file
node scripts/parse-corpus.mjs --strict   # exit 1 unless 100% clean (use in CI)
```

The corpus lives in `vendor/skript-corpus/` (gitignored). Recreate it with:

```sh
cd vendor && git clone --depth 1 --filter=blob:none --sparse \
  https://github.com/SkriptLang/Skript.git skript-corpus
cd skript-corpus && git sparse-checkout set src/test/skript src/main/resources/scripts
```

### Verification discipline

Both gates must be green before claiming a grammar change works. Two traps have
already produced false "all clean" results here:

* `tree-sitter generate | head` masks a non-zero exit — the old parser then
  silently gets tested. Redirect to a log and check `$?` separately.
* Passing ~540 paths as argv overflows on Windows and the CLI returns nothing.
  `scripts/parse-corpus.mjs` uses `--paths <file>` and asserts that the parsed
  count equals the discovered file count before believing any result. Keep that
  assertion.

Node cannot spawn `node_modules/.bin/tree-sitter` (POSIX shell script) or
`tree-sitter.cmd` (blocked since CVE-2024-27980). Scripts must invoke
`node_modules/tree-sitter-cli/tree-sitter.exe` directly.

### Language server

```sh
cd language-server
cargo test                       # whole workspace
cargo test -p skript-syntax      # one crate
cargo test -p skript-docs model  # one module
cargo test -p skript-format      # the formatter
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check

node scripts/fetch-docs.mjs          # places vendor/docs.json (GPL-3.0, gitignored)
                                 # so the 2,117-pattern tests run instead of skipping
node scripts/smoke-lsp.mjs       # end-to-end LSP session against the real binary
```

## Architecture

Three components, deliberately layered:

```
tree-sitter-skript/  →  grammar + external scanner   (structure)
language-server/     →  skript-lsp, Rust, stdio      (semantics)
extension/           →  Zed glue: queries + config   (presentation)
```

The language server is a cargo workspace of six crates: `skript-syntax` (the
pattern DSL), `skript-docs` (syntax databases + version parsing),
`skript-addons` (reads plugin manifests out of JARs), `skript-index` (documents
and symbols), `skript-format`, and `skript-lsp` (the binary).

### The central constraint

Skript has **no context-free grammar**. Every effect, condition, expression,
event and section is a pattern string registered at *runtime* by Skript or an
addon — 2,117 patterns in core Skript 2.16 alone. Nothing lexical distinguishes
`set {_x} to 5` (an effect) from `player is op` (a condition).

Everything follows from that split:

* **tree-sitter owns structure only** — lines, indentation, sections, strings
  and their interpolation, variables, comments, and the handful of headers with
  a fixed shape (`command`, `function`, `options`, `variables`, `aliases`,
  `import`, `using`, `auto reload`). It must never try to classify a statement.
* **The language server owns semantics** — it matches each line against
  `docs.json` and returns the classification as LSP semantic tokens.
* `highlights.scm` therefore leaves ordinary statement prose uncoloured on
  purpose. That is not an omission to "fix"; see the header comment in that file.

### Grammar internals worth knowing before editing `grammar.js`

* `src/scanner.c` (C, **never** `.cc`/`.cpp` — Zed only compiles `scanner.c`)
  emits `_newline`, `_indent`, `_dedent`, `_section_colon` and `block_comment`.
  Its invariants are listed at the top of that file; the important one is that
  layout tokens are zero-width, so the lexer rewinds after each and the
  whitespace loop is pure lookahead.
* `_section_colon` exists because a `:` only opens a section when the rest of
  the line is whitespace and/or a comment — and *not* when the line carries
  Skript's `#-#` suppressor.
* **`comment` is not in `extras`,** and must not be. `#` is literal inside
  strings and variable names, so an `extras` comment makes the longest-match
  lexer swallow the rest of the line at the first `#` in `"item #1"`.
* **Token precedence beats match length** in tree-sitter. `word` and
  `identifier` are kept disjoint (`word` requires ≥1 non-identifier character)
  precisely so they are never both candidates. Adding a `prec()` to a token that
  can match the same text as another is how `skript.home` got split into two
  tokens once already.
* Keyword-like tokens (`boolean`) are plain string choices so tree-sitter's
  keyword extraction (via the `word: $ => $.identifier` property) applies —
  otherwise `truely` matches `true`.
* The grammar is deliberately **permissive** about malformed headers. A parse
  error poisons outline, folding and indentation for the whole file; the
  language server reports the real diagnostic instead.

### Zed specifics that contradict most tutorials

* Zed loads **exactly ten** query files: `highlights brackets outline indents
  injections overrides redactions runnables debugger textobjects`.
  **`folds.scm` and `locals.scm` are NOT loaded** — do not add them. Folding
  comes from indentation and from the LSP's `textDocument/foldingRange`.
* **Indentation is regex-first.** Zed discards tree-sitter indent suggestions
  inside ERROR ranges but keeps regex ones, and a Skript block is a parse error
  while its header is still being typed. `increase_indent_pattern` /
  `decrease_indent_patterns` in `config.toml` do the work; `indents.scm` only
  supplies bracket pairs and the `@start.<suffix>` anchors that `valid_after`
  matches against. Zed compiles these regexes with the Rust `regex` crate —
  **no lookaround is available**.
* Every scope named in a `not_in = [...]` in `config.toml` must be captured in
  `overrides.scm`, or Zed refuses to load the language.
* `outline.scm` requires both `@item` and `@name` on every pattern; omitting
  either drops the whole query.
* Unrecognised capture names are logged as warnings — prefix helper captures
  with `_`.
* Semantic tokens are real but `semantic_tokens` defaults to `"off"`, so
  `highlights.scm` must stand on its own.

### Syntax databases

Two are fetched at runtime and cached, never bundled: Skript's `docs.json` and
SkriptHub's addon catalog. Both are third-party data, so `vendor/` is
gitignored, and `crates/skript-docs/tests/real_data.rs` skips when they are
absent — but **panics when one is present and unparseable**. That distinction
matters: an earlier helper returned `None` for both, and hid the fact that
`Docs::parse` had never once succeeded on the real file.

Schema traps that have already bitten, all in `model.rs`:

* `requirements` and `keywords` are **arrays**, not strings.
* `description` is an array on 1,207 entries and a **bare string** on 14.
* `#[serde(default)]` does not cover an explicit `null`, and the generator emits
  `null` on seven different fields. Use the `nullable` / `string_list` helpers.

### Theming rules (non-negotiable)

No colour is ever named anywhere in this repo. Zed resolves a dotted capture by
walking back along its dot-prefixes (`string.special.symbol` → `string.special`
→ `string`), so refinements are safe when the root is common — but a **bare
invented capture name has no fallback** and will render unstyled. Two captures
on one node are resolved right-to-left as a fallback chain. Prefer roots that
both Zed's bundled One theme and Catppuccin define.

## Conventions

* **No `Co-Authored-By:` trailers in commits.** Daisy asked for this
  explicitly; it overrides the global default.

* `src/parser.c`, `src/grammar.json`, `src/node-types.json` and
  `src/tree_sitter/*.h` are generated but **committed on purpose**: Zed clones
  the grammar repo at a pinned rev and runs clang on `parser.c` directly. It
  never runs `tree-sitter generate`.
* `tree-sitter-skript/` is destined to become its own GitHub repository before
  publishing. Keep it self-contained — no imports from the rest of the monorepo.
* The Skript `docs.json` syntax database is GPL-3.0 and is fetched at runtime,
  never vendored, so this project can stay MIT.
