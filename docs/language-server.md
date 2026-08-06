# The language server

`skript-lsp` speaks LSP 3.17 over **stdio**. Written in Rust, distributed as a
single static binary per platform.

## Capabilities

| Request | Notes |
|---|---|
| `textDocument/documentSymbol` | Nested — a command contains its entries, an event its sections |
| `workspace/symbol` | Substring match across every open file |
| `textDocument/foldingRange` | With `collapsedText`; this is how folding reaches Zed at all |
| `textDocument/hover` | Declarations first, then the syntax the line resolves to |
| `textDocument/definition` | Functions, commands, options, variables |
| `textDocument/references` | Same, scope-aware |
| `textDocument/rename` + `prepareRename` | Rewrites a variable's name without disturbing its braces or scope sigil |
| `textDocument/completion` | Context-aware; patterns insert as snippets |
| `textDocument/formatting` | Re-indents from the parse tree; refuses to touch a file that does not parse |
| `textDocument/signatureHelp` | Parameter hints inside a function call |
| `textDocument/inlayHint` | Parameter names at call sites, read from the declaration |
| `textDocument/documentHighlight` | Every occurrence of the symbol under the cursor |
| `textDocument/semanticTokens/full` | Effects, conditions, expressions, events |
| `textDocument/publishDiagnostics` | See below |

Everything driven by `skript-index` — outline, folding, definition, references,
rename, formatting — works with no network and no `docs.json`.

## Project indexing

On startup the server walks every workspace folder for `.sk` files and indexes
them, so go-to-definition, find-references, rename and completion see the whole
project rather than only the buffers you happen to have open. Build and VCS
directories are skipped; files over 4 MB are ignored. An open document always
wins over the copy on disk, because it may hold unsaved edits.

Scripts disabled with Skript's `-` filename prefix are still indexed — they are
still Skript, and their functions should still resolve.

## Formatting

Deliberately minimal, because Skript lines are free-form English and re-flowing
one would mean guessing at an addon's syntax. The formatter:

* re-indents each line to its **structural** depth from the parse tree, so a
  file indented inconsistently is fixed rather than preserved;
* trims trailing whitespace, collapses blank-line runs, ends with one newline;
* never alters anything within a line, and never re-indents inside a `###`
  block comment;
* returns no edits at all for a file that does not parse — Skript's indentation
  *is* its syntax, so re-indenting a misparsed file could move code between the
  branches of an `if`.

## Settings

Under `lsp.skript-lsp.initialization_options` in Zed:

| Setting | Default | Meaning |
|---|---|---|
| `skriptVersion` | latest | Pin to a Skript version, e.g. `"2.15.3"` |
| `docsPath` | – | A `docs.json` generated on your server, to match its exact Skript build |
| `docsUrl` | – | Fetch from a mirror |
| `unknownSyntaxDiagnostics` | `false` | Report lines matching no known syntax |
| `deprecatedSyntaxDiagnostics` | `true` | Warn on syntax upstream has deprecated |

## Diagnostics

Reported as **errors** — things that are certainly wrong:

| Code | Meaning |
|---|---|
| `mixed-indentation` | Tabs and spaces within one indent |
| `inconsistent-indentation` | The file switched indent character partway |
| `indent-not-a-multiple` | Not a multiple of the file's inferred indent unit |
| `unclosed-block-comment` | A `###` that is never closed |
| `duplicate-declaration` | Two functions, commands or options with one name |
| `unknown-function` | A call to a function declared nowhere in the project |

As **warnings**: `deprecated-syntax`, from `docs.json`'s own `deprecated` flag.

As a **hint, off by default**: `unknown-syntax`. Any addon can register syntax
this server has never seen, so switching it on by default would flag most lines
of most scripts on any real server. Turn it on only alongside `docsPath`.

## Syntax data

Fetched from `https://docs.skriptlang.org/docs.json` at startup — 1,222 entries,
2,660 patterns, no API key — and cached for 24 hours in the platform cache
directory (`%LOCALAPPDATA%`, `~/Library/Caches`, or `$XDG_CACHE_HOME`).
Version-pinned archives live at `/archives/<version>/docs.json`.

It is **never vendored**: the database is GPL-3.0 and this project is MIT, and
fetching keeps it matched to the Skript version you target. Failures degrade —
a stale cache is preferred to nothing, and a small built-in catalog covering the
structural keywords takes over if there is no cache either.

**`/sk gen-docs` does not include addon syntax.** Skript generates its
documentation with `JSONGenerator.of(Skript.instance())`, which is scoped to
Skript's own addon — one `docs.json` describes exactly one addon, named at the
root by `source.name`. Installing SkBee does not add SkBee syntax to it.

`docsPath` is still worth setting: it pins the catalog to the precise Skript
build your server runs, including any fork or nightly. Addon syntax comes from
SkriptHub instead — see [addons.md](addons.md).

## Performance

| | |
|---|---|
| Startup | Immediate; the catalog loads on a background task |
| Line classification | ~272 µs in a release build, against the full core catalog |
| Index narrowing | ~170 candidates per line, about 6% of the catalog |
| Document update | Full reparse, ~9 MB/s |

The matcher runs on a step budget, so a pathological line reports "no match"
instead of hanging the editor.

Updates deliberately reparse the whole file rather than doing an incremental
edit. Reusing a tree-sitter tree is only valid after `Tree::edit` has described
the exact byte range that changed; passing an unedited tree alongside new text
produces a silently wrong parse. Skript files are small enough that the tradeoff
is not close.

## Crate layout

| Crate | Responsibility |
|---|---|
| `skript-syntax` | Pattern DSL parser, matcher, inverted index. No I/O. |
| `skript-docs` | `docs.json` model, fetch + cache, hover rendering, catalog |
| `skript-index` | Document parsing, symbols, references, scope, folding |
| `skript-lsp` | Server binary: capabilities, handlers, position conversion, semantic tokens, diagnostics |

## Position encoding

tree-sitter reports **byte** columns; LSP defaults to **UTF-16 code units**. The
server negotiates UTF-8 when the client offers it and converts otherwise.
Getting this wrong shows up only on lines containing `§`, `¦` or non-English
messages — which is to say, constantly, in real Skript.
