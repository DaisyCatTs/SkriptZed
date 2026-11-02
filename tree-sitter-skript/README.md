# tree-sitter-skript

A [tree-sitter](https://tree-sitter.github.io) grammar for
[Skript](https://github.com/SkriptLang/Skript), the Minecraft server scripting
language.

It is the structural half of [Skript for Zed](https://github.com/DaisyCatTs/SkriptZed)
and is usable on its own.

## What it parses, and what it deliberately does not

Skript has **no context-free grammar**. Every effect, condition, expression,
event and section is a pattern string registered at *runtime* by Skript or an
addon (2,703 of them in core Skript 2.17.1 alone), and nothing lexical
distinguishes `set {_x} to 5` (an effect) from `player is op` (a condition).

So this grammar owns **structure only**: lines and indentation, sections,
strings and their `%…%` interpolation, variables and their scope sigils,
comments, colour codes, and the handful of headers with a fixed shape:
`command` and its Skript 2.17 `subcommand`s, `function`, `options`,
`variables`, `aliases`, `using`, `auto reload`, and three from addons:
skript-reflect's `import`, ClassySK's `class` (fields, methods, `::` member
access) and oopsk's `struct`. It never tries to classify a statement, because
it cannot.

Classification is a separate problem, solved by a language server that matches
each line against Skript's published syntax database. Highlighting ordinary
statement prose is therefore *not* this grammar's job; see
[the architecture notes](https://github.com/DaisyCatTs/SkriptZed/blob/main/docs/architecture.md).

## Layout

```
grammar.js       the grammar
src/scanner.c    external scanner: indentation, section colons, block comments
src/parser.c     generated — committed on purpose, see below
test/corpus/     grammar tests
```

`src/parser.c`, `src/grammar.json`, `src/node-types.json` and
`src/tree_sitter/*.h` are generated but **committed deliberately**: Zed clones
this directory at a pinned revision and runs clang on `parser.c` directly. It
never runs `tree-sitter generate`. Regenerate after any change to `grammar.js`
or `scanner.c`, and commit the result.

There is no `queries/` directory here. The highlight, indent, outline and other
queries are Zed-specific and live in
[`extension/languages/skript/`](https://github.com/DaisyCatTs/SkriptZed/tree/main/extension/languages/skript)
so there is exactly one authoritative copy.

## Building and testing

```sh
npm install
npx tree-sitter generate     # after any grammar.js or scanner.c change
npx tree-sitter test
```

The larger gate is the upstream corpus: all 587 `.sk` files in SkriptLang's
own repository (as of Skript 2.17.1) must parse with zero ERROR or MISSING
nodes. The harness lives in the parent repository:

```sh
node ../scripts/parse-corpus.mjs --strict
```

## Licence

MIT. See [LICENSE](LICENSE).
