# Architecture

## The constraint everything follows from

Skript has **no context-free grammar**.

Every effect, condition, expression, event and section is a *pattern string*
registered at runtime by Skript or by an addon, and matched against the line by
`SkriptParser`. Core Skript 2.17.1 publishes **2,703 patterns across 1,235
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
┌─ tree-sitter-skript ───────────────────────────────────────────┐
│  STRUCTURE                                                     │
│  lines · indentation · sections · strings + interpolation      │
│  variables · options · comments · the fixed-shape headers      │
│                                                                │
│  Never classifies a statement.                                 │
└───────────────────────────┬────────────────────────────────────┘
                            │ parse tree
┌───────────────────────────▼────────────────────────────────────┐
│  skript-index          declarations, references, trigger scope │
│    needs nothing external: works offline, immediately          │
│    → outline · folding · definition · references · rename      │
├────────────────────────────────────────────────────────────────┤
│  skript-format         re-indents from the parse tree          │
├────────────────────────────────────────────────────────────────┤
│  skript-syntax         the pattern engine                      │
│    parses Skript's pattern DSL, matches lines against it       │
├────────────────────────────────────────────────────────────────┤
│  skript-docs           the catalog                             │
│    docs.json + SkriptHub fetched at runtime → every pattern    │
│    → hover · completion · deprecation · version labels         │
├────────────────────────────────────────────────────────────────┤
│  skript-addons         reads plugin manifests out of JARs      │
│    → which addons' syntax to load                              │
├────────────────────────────────────────────────────────────────┤
│  skript-lsp            the server binary                       │
│    handlers · semantic tokens · diagnostics · colours · links  │
│    optional SkAnalyzer bridge (a separate Java process)        │
│    optional RCON reload on save · `skript-lsp mcp` AI helper   │
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
> Ambiguous elements should not be highlighted, e.g. highlighting only the word
> `loop` will select different uses of it (`loop … times:` and `loop-…`).

The grammar colours structure keywords, strings, variables, options, literals
and comments: everything whose identity is certain. The remaining prose is
classified by the language server and coloured through semantic tokens, where
being wrong is recoverable (the user can turn it off) rather than baked into the
grammar.

## The pattern engine

Matching a line against every published pattern naively is far too slow for a
keystroke. `skript-syntax` does three things:

1. **Parse** each pattern into a small node tree: literals, `[optional]`,
   `(a|b)`, `%type%` slots, `<regex>` holes, with `:tag` and `1¦` parse marks
   stripped.
2. **Invert** on the *rarest required literal*. A pattern is only a candidate
   for a line if every literal it requires appears in that line, so indexing on
   the rarest one narrows the whole catalog to roughly 170 candidates per line,
   about 6% of it. (Measured against Skript 2.16.1's database.)
3. **Match** the survivors with a lazy backtracker (slots take the fewest
   tokens that let the rest of the pattern fit) and rank by specificity.

`node scripts/coverage.mjs <file.sk>` times a whole-file pass against the real
catalog; use it rather than a quoted figure when judging a matcher change.

The matcher runs on a step budget. A pathological line exhausts it and reports
"no match", which degrades to an unclassified line rather than a hung editor.

## Where the pieces live

| Path | Contents |
|---|---|
| `tree-sitter-skript/` | `grammar.js`, `src/scanner.c`, the committed generated parser, corpus tests. Zed builds it straight from this subdirectory (see [publishing.md](publishing.md)); it is kept self-contained, with no imports from the rest of the repository, so it can also be used on its own. |
| `language-server/crates/skript-syntax` | Pattern DSL parser, matcher, inverted index. Pure logic, no I/O. |
| `language-server/crates/skript-docs` | `docs.json` and SkriptHub models, runtime fetch and cache, version parsing, hover rendering, the searchable catalog. |
| `language-server/crates/skript-addons` | Reads `paper-plugin.yml` / `plugin.yml` out of plugin JARs to decide which addons are installed. |
| `language-server/crates/skript-index` | Document parsing, symbol and reference extraction, workspace and trigger scope rules, folding, skript-reflect custom syntax definitions. |
| `language-server/crates/skript-format` | The formatter. |
| `language-server/crates/skript-lsp` | The stdio server: capabilities, request handlers, position conversion, semantic tokens, diagnostics, colours, links, the SkAnalyzer bridge, RCON reload, and the read-only `skript-lsp mcp` helper. |
| `extension/` | `extension.toml`, the Zed query files, `config.toml`, snippets, and the Rust shim that locates the server. |

## Scope rules the index models

* A `local function` is visible only inside its declaring script.
* `{_x}` is confined to its trigger (event, command, function or ClassySK
  method) for references and rename. Each ClassySK method runs as its own
  trigger, so two methods of one class do not share a `{_x}`.
* An `{@option}` is visible only inside its own script, because options are
  substituted before parsing.
* `{global}` and `{-ephemeral}` variables are workspace-wide; the `-` affects
  persistence, not visibility.
* A variable whose name is interpolated (`{home::%uuid of player%}`) names a
  different variable per player, so it is deliberately **not** indexed as one
  renameable symbol.
