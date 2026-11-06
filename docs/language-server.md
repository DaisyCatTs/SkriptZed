# The language server

`skript-lsp` speaks LSP 3.17 over **stdio**. Written in Rust, distributed as a
single binary per platform.

Zed leaves several of these features off by default: folding ranges, document
symbols, inlay hints and argument hints after a completion. The README's
[install step 2](../README.md#install) lists the settings that turn them on.

## Capabilities

Every request below except the file and configuration notifications can be
switched off under `features` (see the [README](../README.md#feature-switches)).
The capability stays advertised either way, so a switch applies without a
restart; a switched-off request is answered with nothing.

| Request | Notes |
|---|---|
| `textDocument/documentSymbol` | Shown in Zed's outline with `document_symbols` on. Nested: a command contains its entries and subcommands, an event its sections, a ClassySK class its fields and methods, an oopsk struct its fields. skript-reflect custom syntax definitions have their own symbol kind |
| `workspace/symbol` | Substring match across every indexed file |
| `textDocument/foldingRange` | With `collapsedText`. Zed uses it only with `document_folding_ranges` on, and folds by indentation otherwise |
| `textDocument/hover` | Declarations first (functions, variables, ClassySK classes and members), then the expression under the cursor, then the syntax the whole line resolves to. Addon syntax shows SkriptHub's examples and a link to its page. The project's own skript-reflect syntax says where it is defined, and an addon section's entries (`result:` in SkBee's `register shaped recipe:`) are explained |
| `textDocument/definition` | Functions, commands, options, variables, ClassySK methods and fields, and from a use of a skript-reflect `effect`, `condition`, `expression` or `event` to its definition |
| `textDocument/references` | Same, scope-aware |
| `textDocument/rename` + `prepareRename` | Rewrites a variable's name without disturbing its braces or scope sigil. Refused for ClassySK members, because which class `{_x}::reset()` calls is only known at runtime |
| `textDocument/completion` | Context-aware; patterns insert as snippets. Inside `{` it offers the variables the project uses, one list level at a time. `event-` (from `ev`) offers the enclosing event's values first, for core, addon and custom events, with `past` and `future` forms. Inside a command or subcommand, its entries; inside an addon section SkriptHub documents, that section's entries, required ones not yet written first. The project's skript-reflect syntax completes like any other |
| `completionItem/resolve` | Renders the documentation card only for the item you are looking at, so the list itself stays small |
| `textDocument/formatting` | Re-indents from the parse tree; refuses to touch a file that does not parse |
| `textDocument/rangeFormatting` | The same, for the selected lines, widened to every top-level structure they touch. In Zed: `editor: format selections` |
| `textDocument/signatureHelp` | Triggered by `(` and `,`. Covers your functions, Skript's built-ins, addon functions and ClassySK methods (`Req::get(`); highlights the right parameter for a named argument, and keeps a default like `(1, 7)` in one piece |
| `textDocument/inlayHint` | Parameter names at call sites, read from the declaration. Zed shows them with `inlay_hints.enabled` |
| `textDocument/documentHighlight` | Every occurrence of the symbol under the cursor, scoped to its trigger |
| `textDocument/codeAction` | Quick fixes for unknown functions, unknown options, `cancel event` in an event that cannot be cancelled, indentation and unclosed block comments |
| `textDocument/documentColor` + `colorPresentation` | Swatches on `&6`, `&x&r&r&g&g&b&b`, `&#rrggbb`, `<#rrggbb>` and MiniMessage named colours; a picked colour is written back in the same style |
| `textDocument/documentLink` | `http(s)` URLs in `<link:…>` / `<url:…>` tags and anywhere on a line, comments included. No other scheme is ever linked |
| `textDocument/prepareCallHierarchy` + `incomingCalls` / `outgoingCalls` | Events and commands count as callers. In Zed: `call hierarchy: show incoming calls` |
| `workspace/didChangeWatchedFiles` | Re-indexes when scripts change on disk, so a `git pull` does not leave phantom errors |
| `workspace/didChangeConfiguration` | Applies every setting without a restart. A change to where syntax comes from (`docsPath`, `addons`, `serverPath`, `platform`, …) reloads the database; a change to `features` asks Zed to refresh colours and inlay hints |
| `textDocument/didSave` | Runs SkAnalyzer on the saved file, and reloads it on the server over RCON, when each is enabled |
| `textDocument/semanticTokens/full` | Effects, conditions, expressions, events, and the expressions inside a line's slots and call arguments |
| `textDocument/publishDiagnostics` | See below |

Everything driven by `skript-index` (outline, folding, definition, references,
rename, formatting) works with no network and no `docs.json`.

## Project indexing

On startup the server walks every workspace folder for `.sk` files and indexes
them, so go-to-definition, find-references, rename and completion see the whole
project rather than only the buffers you have open. Build and VCS directories
are skipped; files over 4 MB are ignored. An open document always wins over the
copy on disk, because it may hold unsaved edits.

Scripts disabled with Skript's `-` filename prefix are still indexed. They are
still Skript, and their functions should still resolve.

## Formatting

Deliberately minimal, because Skript lines are free-form English and re-flowing
one would mean guessing at an addon's syntax. The formatter:

* re-indents each line to its **structural** depth from the parse tree, so a
  file indented inconsistently is fixed rather than preserved;
* trims trailing whitespace, collapses blank-line runs, ends with one newline;
* puts one blank line under a multi-line structure that runs straight into the
  next one. It only ever adds that line, never removes one, leaves a comment
  directly above a structure attached to it, and leaves runs of one-line
  structures (`using …`) packed together. `formatBlankLines: false` turns this
  off;
* never alters anything within a line, and never re-indents inside a `###`
  block comment;
* returns no edits at all for a file that does not parse. Skript's indentation
  *is* its syntax, so re-indenting a misparsed file could move code between the
  branches of an `if`.

Formatting a selection widens it to every top-level structure the selection
touches. Skript infers the indent unit once per structure, so re-indenting half
of one with tabs while the other half keeps its spaces would turn working code
into code Skript rejects.

## Settings

Every setting, with its default, is in the
[README's settings table](../README.md#settings). That table is the canonical
list; this page does not repeat it.

## Diagnostics

Reported as **errors**, things that are certainly wrong:

| Code | Meaning | Quick fix |
|---|---|---|
| `mixed-indentation` | Tabs and spaces within one indent | Fix the file's indentation |
| `inconsistent-indentation` | A structure switched indent character partway | Fix the file's indentation |
| `indent-not-a-multiple` | Not a multiple of the structure's inferred indent unit | Fix the file's indentation |
| `unclosed-block-comment` | A `###` that is never closed | Close the block comment |
| `duplicate-declaration` | Two functions, commands or options with one name; links to the first | – |
| `unknown-function` | A call to a function declared nowhere in the project and not built in | Correct the name, or create the function |
| `function-arity` | A call with more arguments than the function takes, or fewer than it needs. Checked against your functions and Skript's built-ins, once the project has been indexed. A name with several declarations passes if any of them fits | – |
| `unknown-option` | A `{@name}`, in code or inside a string, with no entry in the script's `options:` block | Add it to `options:` |

`function-arity` counts only where the count is certain. It skips a function
with a single parameter (Skript folds `f(1, 2)` into one list argument when that
parameter is plural), an argument holding a bare `and` or `or`, an `{@option}`
argument, an empty argument and a call that is not closed.

As **warnings**:

| Code | Meaning | Quick fix |
|---|---|---|
| `deprecated-syntax` | From `docs.json`'s own `deprecated` flag. Can be turned off | – |
| `requires-addon` | The line matches an addon that is not installed, named with its minimum version. Only when a `plugins/` directory was found | – |
| `event-not-cancellable` | `cancel event` in an event Skript's `docs.json` marks as not cancellable. Skript refuses it at load time, but the verdict rests on that flag, so it is a warning. Skipped inside addon sections, which may run their body in another event | Remove `cancel event` |
| `local-never-set` | A `{_local}` read in a trigger that never sets it, so it is always empty there. Can be turned off with `localVariableDiagnostics` | – |
| `no-event-value` | `player`, `event-player`, `attacker` and the like where the trigger has none: in a function, an `every …` event, or a core event whose documented values cannot supply it. Skript's own wording ("There's no player in a function event"). Skipped in commands, ClassySK methods, addon events and slots a literal could fill. `eventValueDiagnostics` turns it off | – |
| `empty-section` | A section header with nothing under it, or only comments. Skript's config reader warns "Empty configuration section!" for every section, `options:` and `variables:` included | – |
| `unreachable-code` | A statement after `stop`, `exit` or a `return` that always runs. Faded, with Skript's wording. Not reported after `stop if …` or where the script suppresses Skript's own warning | – |
| `unknown-event` | An `on …` header nothing explains, one or two letters away from a known event. Only once every detected addon's syntax has loaded. `unknownEventDiagnostics` turns it off | Change to the event it resembles |
| `duplicate-function` / `duplicate-command` / `duplicate-syntax` | The same function signature, command or skript-reflect pattern declared in two scripts; links to the other one | – |
| `requires-newer-skript` | **Off by default** (`newerSyntaxDiagnostics`). Syntax newer than the Skript version read from `Skript.jar` or `skriptVersion`. Off because `docs.json` dates some rewritten syntax later than the older form it still accepts | – |

As **hints**:

| Code | Meaning |
|---|---|
| `doc-unknown-param` | An `@param` in a doc comment that names no parameter of the function |
| `local-never-read` | A `{_local}` set but never read in its trigger. Tagged as unnecessary, so Zed shows it faded, like dead code. Can be turned off with `localVariableDiagnostics` |
| `unknown-syntax` | **Off by default.** Any addon can register syntax this server has never seen, so switching it on by default would flag most lines of most scripts on any real server. Turn it on only if your scripts use no addons, or you supply every addon's syntax through `customSyntaxPaths` |

The two local-variable checks only fire when nothing in the trigger could make
them wrong. Any line that might write the variable counts as a write (a line
starting with `set`, `add`, `loop` and other change verbs, a variable after
`in`/`into`/`to`/`from`, a skript-reflect `{_x}.field` at the start of a line),
list variables and triggers with a dynamic name like `{_%name%}` are skipped,
and only events, commands, functions and ClassySK methods are checked.

`features.diagnostics: false` silences every check above. It does not touch
SkAnalyzer or RCON findings, which have their own settings.

From **SkAnalyzer**, when enabled: Skript's own parser errors, with source
`SkAnalyzer` and code `skanalyzer`. Skript's `SEVERE` becomes an error and
`WARNING` a warning. Skript reports a line but no column, so each covers the
line's code. See the
[README](../README.md#checking-against-real-skript-skanalyzer) for setup and
limits.

From **your server**, when RCON reload is enabled: what Skript printed while
reloading the saved script, with source `Skript (server)` and code
`server-reload`. Skript colours a warning's header differently from an error's,
and that colour decides the severity; when Skript reports the reload as
successful, everything left is a warning. A message with no line attached is
put on the first line. See the
[README](../README.md#reloading-on-your-server-rcon) for setup.

## The MCP helper

`skript-lsp mcp [--project <dir>] [--settings <json>]` runs a different program
from the same binary: a read-only Model Context Protocol server over stdio, for
an AI agent. It loads the catalog exactly as the language server does (same
settings, same cache) and offers four tools: `search_syntax`, `get_docs`,
`classify` and `project_symbols`. No tool writes a file or contacts a game
server. If the real database cannot be loaded, every answer says the built-in
fallback is standing in, so a model is never told "no such syntax" by a
near-empty catalog.

The extension never starts it or registers it. The README's
[AI helper section](../README.md#ai-helper-optional) shows the Zed setting that
opts in.

## Syntax data

Fetched from `https://docs.skriptlang.org/docs.json` at startup (1,235 entries
and 2,703 patterns for Skript 2.17.1, no API key) and cached for 24 hours in the
platform cache directory (`%LOCALAPPDATA%`, `~/Library/Caches`, or
`$XDG_CACHE_HOME`). Version-pinned archives live at
`/archives/<version>/docs.json`. On a skript-minestom server (detected, or `"platform": "minestom"`) the
database comes from `https://smdocs.hapily.me` instead. Downloads follow the
`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` environment variables.

It is **never vendored**: the database is GPL-3.0 and this project is MIT, and
fetching keeps it matched to the Skript version you target. Failures degrade: a
stale cache is preferred to nothing, and a small built-in catalog covering the
structural keywords takes over if there is no cache either.

**`/sk gen-docs` does not include addon syntax.** Skript generates its
documentation with `JSONGenerator.of(Skript.instance())`, which is scoped to
Skript's own addon: one `docs.json` describes exactly one addon, named at the
root by `source.name`. Installing SkBee does not add SkBee syntax to it.

`docsPath` is still worth setting: it pins the catalog to the precise Skript
build your server runs, including any fork or nightly. Addon syntax comes from
SkriptHub instead; see [addons.md](addons.md).

## Performance

| | |
|---|---|
| Startup | Immediate; the catalog loads on a background task |
| Index narrowing | ~170 candidates per line, about 6% of the core catalog |
| Document update | Full reparse of the changed file |

`node scripts/coverage.mjs <file.sk>` times a full semantic-token pass on a real
file, which is the number to look at when judging a matcher change.

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
| `skript-docs` | `docs.json` and SkriptHub models, fetch and cache, version parsing, hover rendering, the catalog |
| `skript-addons` | Reads `paper-plugin.yml` / `plugin.yml` out of plugin JARs to decide which addons are installed |
| `skript-index` | Document parsing, symbols, references, trigger scope, folding, skript-reflect custom syntax definitions |
| `skript-format` | The formatter |
| `skript-lsp` | Server binary: capabilities, handlers, position conversion, semantic tokens, diagnostics, colours, links, the SkAnalyzer bridge, RCON reload, and the `mcp` helper |

## Position encoding

tree-sitter reports **byte** columns; LSP defaults to **UTF-16 code units**. The
server negotiates UTF-8 when the client offers it and converts otherwise.
Getting this wrong shows up only on lines containing `§`, `¦` or non-English
messages, which in real Skript means constantly.
