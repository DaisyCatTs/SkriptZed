# Architecture

## The constraint everything follows from

Skript has **no context-free grammar**.

Every effect, condition, expression, event and section is a *pattern string*
registered at runtime by Skript or by an addon, and matched against the line by
`SkriptParser`. Core Skript 2.16 publishes **2,117 patterns across 1,208
entries**; SkBee alone adds hundreds more, and any server may load a dozen
addons.

So nothing lexical distinguishes these:

```sk
set {_x} to 5      # an effect
player is op       # a condition
uuid of player     # an expression
```

They are told apart by *position and meaning*, not by shape. A parser generator
cannot do it, and a parser that pretends to will be confidently wrong on every
addon it has not been taught about.

## The split

```
┌─ tree-sitter-skript ──────────────────────────────────────────┐
│  STRUCTURE                                                     │
│  lines · indentation · sections · strings + interpolation      │
│  variables · options · comments · the fixed-shape headers      │
│                                                                │
│  Never classifies a statement.                                 │
└───────────────────────────┬────────────────────────────────────┘
                            │ parse tree
┌───────────────────────────▼────────────────────────────────────┐
│  skript-index          declarations and references             │
│    needs nothing external — works offline, immediately         │
│    → outline · folding · definition · references · rename      │
├────────────────────────────────────────────────────────────────┤
│  skript-syntax         the pattern engine                      │
│    parses Skript's pattern DSL, matches lines against it       │
├────────────────────────────────────────────────────────────────┤
│  skript-docs           the catalog                             │
│    docs.json fetched at runtime → 2,117 indexed patterns       │
│    → hover · completion · deprecation                          │
└───────────────────────────┬────────────────────────────────────┘
                            │ LSP
┌───────────────────────────▼────────────────────────────────────┐
│  Zed                                                           │
│  highlights.scm colours what is certain                        │
│  semantic tokens colour what the server worked out             │
└────────────────────────────────────────────────────────────────┘
```

The layers fail independently, which is the point:

| If this is unavailable | You still get |
|---|---|
| the network, on first run | everything except hover, completion and semantic classification |
| `docs.json` entirely | highlighting, indentation, folding, outline, definition, references, rename, indentation diagnostics |
| the language server | highlighting, indentation, snippets, indent-based folding |

## Why `highlights.scm` leaves prose uncoloured

Because the alternative is being wrong. Skript's own highlighting guidance says
it plainly:

> It is better not to highlight a structure than to incorrectly identify it.
> Ambiguous elements should not be highlighted — e.g. highlighting only the word
> `loop` will select different uses of it (`loop … times:` and `loop-…`).

The grammar colours structure keywords, strings, variables, options, literals
and comments — everything whose identity is certain. The remaining prose is
classified by the language server and coloured through semantic tokens, where
being wrong is recoverable (the user can turn it off) rather than baked into the
grammar.

## The pattern engine

Matching a line against 2,117 patterns naively is far too slow for a keystroke.
`skript-syntax` does three things:

1. **Parse** each pattern into a small node tree — literals, `[optional]`,
   `(a|b)`, `%type%` slots, `<regex>` holes, with `:tag` and `1¦` parse marks
   stripped.
2. **Invert** on the *rarest required literal*. A pattern is only a candidate
   for a line if every literal it requires appears in that line, so indexing on
   the rarest one narrows ~2,117 patterns to ~170 per line.
3. **Match** the survivors with a lazy backtracker — slots take the fewest
   tokens that let the rest of the pattern fit — and rank by specificity.

Measured on the real database in a debug build: **433 µs per line**. Release is
roughly an order of magnitude faster.

The matcher runs on a step budget. A pathological line exhausts it and reports
"no match", which degrades to an unclassified line rather than a hung editor.

## Where the pieces live

| Path | Contents |
|---|---|
| `tree-sitter-skript/` | `grammar.js`, `src/scanner.c`, the committed generated parser, corpus tests. Destined for its own repository. |
| `language-server/crates/skript-syntax` | Pattern DSL parser, matcher, inverted index. Pure logic, no I/O. |
| `language-server/crates/skript-docs` | `docs.json` model, runtime fetch + cache, hover rendering, the searchable catalog. |
| `language-server/crates/skript-index` | Document parsing, symbol and reference extraction, workspace scope rules, folding. |
| `language-server/crates/skript-lsp` | The stdio server: capabilities, request handlers, position conversion, semantic tokens, diagnostics. |
| `extension/` | `extension.toml`, the Zed query files, `config.toml`, snippets, and the Rust shim that locates the server. |

## Scope rules the index models

* A `local function` is visible only inside its declaring script.
* A `{_local}` variable is confined to its file. (Skript scopes it to one
  *trigger*; the index does not model triggers, and over-reporting within one
  file is far less harmful than missing a definition.)
* `{global}` and `{-ephemeral}` variables are workspace-wide — the `-` affects
  persistence, not visibility.
* A variable whose name is interpolated (`{home::%uuid of player%}`) names a
  different variable per player, so it is deliberately **not** indexed as one
  renameable symbol.
