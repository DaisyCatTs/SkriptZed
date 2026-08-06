# The Definitive Skript Extension for Zed

## Context

**Why:** Skript (the Minecraft server scripting language, `SkriptLang/Skript`, currently **2.16.1**) has no Zed support at all — verified against `zed-industries/extensions` (1,355 registered extensions; the only Minecraft-adjacent one is `mcfunction`). Across *every* editor the tooling is weak: the two existing tree-sitter grammars are single-day, unlicensed, ~70–180 line lexical toys with no indentation model; every existing language server is GPL-3.0, archived, or explicitly a scaffold. This is a genuinely open slot, and the goal is not a highlighter — it's the best Skript development experience in any editor.

**Outcome:** A monorepo producing (a) `tree-sitter-skript` — the first real Skript grammar with an INDENT/DEDENT external scanner; (b) `skript-lsp` — a permissively-licensed standalone Rust language server; (c) a Zed extension that wires them together and works beautifully under any theme.

**Decisions made:** build everything before publishing · Rust language server · fetch `docs.json` at runtime (no GPL redistribution) · monorepo + split grammar repo.

---

## The one architectural insight everything follows from

Skript has **no context-free grammar**. Every effect, condition, expression, event and section is a *runtime-registered pattern string* matched by `SkriptParser`; addons register hundreds more. Core Skript 2.16.1 alone has **2,117 patterns** across 1,208 syntax entries. Given `set {_x} to 5`, nothing lexical says "effect". Given `player is op`, nothing says "condition". Disambiguation is positional and semantic, not syntactic.

This gives a hard split, and it is the whole design:

| Layer | Owns | Never does |
|---|---|---|
| **tree-sitter** | Structure: lines, sections, INDENT/DEDENT, strings + `%interpolation%`, variables, comments, literals, command/function/options headers | Classify a statement as effect vs condition vs expression |
| **LSP** | Semantics: pattern-match each line against `docs.json`, resolve types, index the workspace | Re-lex the file |
| **Semantic tokens** | Paint the semantic classification back onto the buffer | — |

This is also the answer to *"the grammar should understand Skript's English-like syntax instead of matching keywords"*: it can't, and shouldn't try — the LSP does, and hands the result back as semantic tokens. `SkriptLang/skript-grammar`'s own `GRAMMAR.md` (MIT) states the rule: *"It is better not to highlight a structure than to incorrectly identify it."*

---

## Repository layout

```
ZedSkript/                          # monorepo, MIT
├─ extension/                       # the Zed extension (published root)
│  ├─ extension.toml
│  ├─ Cargo.toml                    # crate-type = ["cdylib"], zed_extension_api = "0.7.0"
│  ├─ LICENSE                       # REQUIRED by Zed registry since 2025-10-01
│  ├─ src/{lib.rs, download.rs, settings.rs}
│  ├─ languages/skript/             # config.toml + 8 .scm + semantic_token_rules.json
│  ├─ languages/skript-config/      # config.sk / features.sk dialect
│  └─ snippets/skript.json
├─ language-server/                 # Rust workspace → skript-lsp binary
│  └─ crates/{skript-syntax, skript-docs, skript-index, skript-format, skript-lsp}
├─ tree-sitter-skript/              # → split to its own GitHub repo before release
│  ├─ grammar.js, tree-sitter.json, package.json, Cargo.toml
│  ├─ src/{grammar.json, node-types.json, parser.c, scanner.c, tree_sitter/*.h}   # ALL COMMITTED
│  └─ test/corpus/*.txt, test/highlight/*.sk
├─ examples/                        # sample Skript projects for manual QA
├─ docs/                            # architecture, grammar, LSP, contributing, publishing
└─ .github/workflows/               # CI: grammar tests, LSP tests, cross-platform release
```

**Why the grammar must split out:** Zed's `ExtensionBuilder::compile_grammar` does `git remote add origin <repository>` + `git fetch --depth 1 origin <rev>` + `clang` on the checked-out `src/parser.c`. It **never runs `tree-sitter generate`**. So `src/parser.c`, `src/grammar.json`, `src/node-types.json` and `src/tree_sitter/*.h` must be committed. Only `src/scanner.c` is picked up — `scanner.cc`/`.cpp` are silently ignored, so **the external scanner must be C**.

**Dev loop:** `extension.toml` points `repository` at a `file:///C:/Users/Daisy/Desktop/ZedSkript/tree-sitter-skript` URL with a local commit SHA as `rev` (Zed's docs confirm `file://` works). Flip to the GitHub URL only at release. `extension/grammars/` is a build artifact → gitignore it.

---

## 1. `tree-sitter-skript`

### Node model

```
source_file      := (structure | comment | blank)*
structure        := event | command | function | options | variables | aliases | using | auto_reload
section          := header ':' NEWLINE INDENT body DEDENT
statement        := <free text with typed sub-nodes> NEWLINE
```

Typed sub-nodes (these are what `highlights.scm` colours):

| Node | Shape |
|---|---|
| `string` | `"…"` containing `interpolation` (`%expr%`), `escape` (`""`, `%%`), `format_tag` (`<red>`, `<#FF0000>`, `<link:…>`), `legacy_color` (`&6`) |
| `variable` | `{name}` / `{_local}` / `{-ephemeral}` / `{list::*}`; name may itself contain `interpolation`; `::` is `list_separator` |
| `option_ref` | `{@name}` |
| `comment` | `# …`, `##` escape, and `block_comment` (a line whose trimmed content is exactly `###` toggles) |
| `command_header` | `command [/]name <arg>…` with `command_argument` (`<name: type = default>`) |
| `function_signature` | name, `parameter` (`n: type = default`), return marker `::` \| `->` \| `returns`, return type |
| `entry` | `key: raw-value` — used by command entries, `options:`, and `config.sk` |
| `loop_value` / `event_value` / `command_arg_ref` | `loop-player`, `loop-index`, `event-block`, `arg-1`, `arg-text` |
| number, boolean, timespan | `5`, `2.5`; `true/false/yes/no/on/off`; `5 seconds`, `2 ticks` |

### External scanner (`src/scanner.c`, C)

Modelled on `tree-sitter-python`'s and `tree-sitter-gdscript`'s scanners.

```js
externals: $ => [$._newline, $._indent, $._dedent, $.comment, ']', ')', '}']
```

- `comment` is declared external **on purpose** — it guarantees the scanner runs on every token so DEDENT still fires during error recovery (this is the documented reason in tree-sitter-python).
- Indent stack seeded with a `0` sentinel that is never serialized; `deserialize` re-seeds it and must fully reset (it is called with `(NULL, 0)`).
- `lexer->mark_end()` **before** the whitespace loop; DEDENT is zero-width; one INDENT/DEDENT per `scan()` call.
- Bound the `serialize` loop at `TREE_SITTER_SERIALIZATION_BUFFER_SIZE` — Zed had to fork `tree-sitter-markdown` over exactly this overflow.
- Track "indent of the most recent line with real content" so blank lines inside triggers don't emit premature DEDENT (the GDScript refinement).
- **Skript-specific:** be *lenient* — accept any self-consistent indent. Skript's real rule (`Config.java` / `SectionNode.load_i`) infers the unit once per file from the first depth-1 indent, forbids mixing tabs and spaces within an indent, and requires exact multiples. Enforcing that in the scanner would make every mid-edit buffer unparseable; enforce it in **LSP diagnostics** instead.

### Two parser traps that must be handled

1. **`#-#` magic comment.** A line matching `([^#]|##)*#-#(\s.*)?` does *not* open a section even though it ends in `:` — e.g. `send "a:" to player #-#`.
2. **`###` block comments** (Skript 2.9+). Only a line whose *trimmed* content is exactly `###` toggles. `### foo` is a normal comment.

Plus the comment/string/variable state machine from `Node.splitLine`: `#` inside `"…"` or inside `{…}` is literal; `%` toggles into code context from inside a string and returns to the *previous* state on close.

### Grammar tests

- `test/corpus/*.txt` (80-`=`/80-`-` separators) per construct.
- **Regression corpus:** the **543 `.sk` files** in `SkriptLang/Skript` (`src/test/skript/**`, `src/main/resources/scripts/-examples/**`). CI asserts zero ERROR nodes across all of them. This is the single best quality gate available and nobody else has used it.
- Generate with `tree-sitter-cli` **0.25.x–0.26.x** → ABI 15. Zed pins `tree-sitter = "0.26.9"` (accepts ABI 13–15) and compiles with wasi-sdk-25 clang.

---

## 2. Editor layer (`extension/languages/skript/`)

### `config.toml` — indentation is regex-first, deliberately

Zed merges two indent systems in `Buffer::suggest_autoindents`. The decisive detail: tree-sitter suggestions are discarded inside ERROR ranges (`within_error && !from_regex`) but **regex suggestions survive**. While you're mid-typing an incomplete Skript block the tree is broken — so regex must carry indentation. This is exactly why Zed's own YAML has **no `indents.scm` at all**.

```toml
name = "Skript"
grammar = "skript"
path_suffixes = ["sk"]
line_comments = ["# "]
tab_size = 4
hard_tabs = true                            # Skript convention is tabs
autoclose_before = ",;:)]}>\"' \n\t"
word_characters = ["-", "_"]                # loop-player, arg-1, event-block are single words
completion_query_characters = ["-", "_", "{", "}", "%", "@", ":"]

auto_indent_using_last_non_empty_line = false   # required for indent-sensitive languages
auto_indent_on_paste = false                    # re-indenting pasted Skript destroys it

# a non-comment line ending in ':' opens a section — unless it carries the #-# marker
increase_indent_pattern = "^(?:[^#\"]|\"[^\"]*\"|##)*:[ \\t]*(?:#(?!-#).*)?$"

decrease_indent_patterns = [
  { pattern = "^\\s*else\\s+if\\b.*:", valid_after = ["if", "else-if"] },
  { pattern = "^\\s*else\\s*:",        valid_after = ["if", "else-if"] },
]

brackets = [
  { start = "(", end = ")", close = true,  newline = false },
  { start = "[", end = "]", close = true,  newline = false },
  { start = "{", end = "}", close = true,  newline = false, not_in = ["comment"] },
  { start = "\"", end = "\"", close = true, newline = false, not_in = ["string", "comment"] },
  { start = "%", end = "%", close = true,  newline = false, not_in = ["comment"] },
]
```

`indents.scm` carries only bracket pairs plus `@start.if` / `@start.else-if` anchors, which exist purely to feed `valid_after`.

### Query files — exactly the ten Zed loads

Verified against `crates/language_core/src/queries.rs`:
`highlights · brackets · outline · indents · injections · overrides · redactions · runnables · debugger · textobjects`

> **`folds.scm` does not exist in Zed and never has.** The Java extension ships one and it is silently ignored. Folding comes from indentation + brackets, and — the good part — from **LSP `textDocument/foldingRange`**, which `crates/project/src/lsp_store/folding_ranges.rs` consumes including `collapsed_text`. So every folding requirement in the brief (events, commands, functions, loops, if/else, while, sections, options) is delivered by the language server, with proper collapsed placeholders. No `folds.scm` will be written.

### Theme portability

`SyntaxTheme::highlight_id` degrades a dotted capture to its longest defined dot-prefix: `@string.special.symbol` → `string.special` → `string`. So dotted names are always safe if the root is safe. Rules:

- Use only roots present in both Zed's docs list and the bundled One theme: `keyword function variable string constant type property comment number boolean operator punctuation tag attribute label constructor preproc embedded enum title emphasis`.
- **Never invent a bare top-level name** (`@decorator` has no fallback). Use the explicit right-to-left form instead: `(node) @function.decorator @attribute`.
- Prefix helper captures with `_` or Zed logs "unrecognized capture name".
- **Zero hardcoded colours anywhere.** Cross-check against the 101 capture names Catppuccin defines (extracted locally during research) plus One's 46 — target the intersection.

Mapping sketch: structure keywords → `@keyword`; `on`/event names → `@keyword` + `@function` fallback; command name → `@function`; function name → `@function.definition @function`; `{var}` → `@variable`, `{_local}` → `@variable.parameter @variable`, `{-eph}` → `@variable.special @variable`, `{@opt}` → `@constant`; `%…%` delimiters → `@punctuation.special`; `<red>` → `@string.special`; `&6` → `@string.escape`; timespans → `@number`; `loop-player`/`event-block`/`arg-1` → `@variable.builtin @variable`.

### `injections.scm`

Inject `regex` into `<…>` command-argument regex slots. Skript has no other embedded language (`skript-reflect` `import:` bodies are Java *type names*, not code — leave them plain).

### `semantic_token_rules.json`

Zed loads `languages/<lang>/semantic_token_rules.json` from extensions (`extension_host.rs` ~L1400) and ranks it above Zed defaults, below user settings. This is where the LSP's real understanding lands:

```jsonc
[
  { "token_type": "skriptEffect",     "style": ["function", "keyword"] },
  { "token_type": "skriptCondition",  "style": ["keyword.conditional", "keyword"] },
  { "token_type": "skriptExpression", "style": ["property", "variable"] },
  { "token_type": "skriptEvent",      "style": ["function.builtin", "function"] },
  { "token_type": "skriptType",       "style": ["type"] },
  { "token_type": "variable", "token_modifiers": ["deprecated"], "strikethrough": true },
  { "token_type": "function", "token_modifiers": ["defaultLibrary"], "style": ["function.builtin", "function"] }
]
```

Honest caveat to document in the README: Zed's `semantic_tokens` setting defaults to **`"off"`**. The extension will ship the rules and the README will tell users to set `"semantic_tokens": "combined"` for the full experience — but `highlights.scm` must look excellent on its own, because most users will never flip it.

### Second language: `skript-config`

`config.sk` and `features.sk` use the same node format but contain no code. Same grammar, separate `languages/skript-config/config.toml` with `first_line_pattern` / filename matching, `hidden = false`, no language server attached.

### Snippets

`snippets = ["./snippets/skript.json"]` in `extension.toml` (snippets are the one thing that must be explicitly declared). Filename must be the lowercased language name. ~60 snippets covering everything in the brief plus: `command` (full entry scaffold), `function` (all three return markers), `options`, `variables`, `aliases`, every common `on …` event, `loop`/`for each`/`while`/`do while`, `if`/`else if`/`else`, multiline `if all:`/`if any:`, `send`/`broadcast`/`set`/`add`/`delete`/`wait`/`every`/`teleport`/`give`, `using <experiment>`.

### `runnables.scm` + `tasks.json`

Tag commands and functions so Zed's runnable gutter can offer "reload this script" — emitting `/sk reload <script>` via RCON or a user-configured command. Low cost, high delight.

---

## 3. `skript-lsp` (Rust)

### Crates

| Crate | Responsibility |
|---|---|
| `skript-syntax` | Skript **pattern DSL** parser: `[optional]`, `(a\|b)`, `%type%`, `%~type%`, `<regex>`, `:parse-marks`. Compiles the 2,117 patterns into a matcher. Pure, no I/O. |
| `skript-docs` | Fetch + cache + parse `docs.json`. Version detection, schema-v2 model. |
| `skript-index` | Workspace index: functions, commands, options, variables, aliases, per-file symbol tables, incremental invalidation. |
| `skript-format` | Formatter (idempotent, comment-preserving). |
| `skript-lsp` | `tower-lsp` server binary over **stdio**. |

### The pattern matcher is the hard part

2,117 patterns, expanding combinatorially through optionals and choices. Naive matching is O(patterns × line). Design:

1. **Compile** each pattern into a small NFA over literal-token / type-slot / regex nodes.
2. **Pre-filter** with a keyword→pattern inverted index built from the rarest literal token in each pattern (this is what IntelliSkript's "frequency matrix" does; we implement the idea independently — its code is GPL-3.0 and will not be read for copying).
3. **Rank** survivors by specificity, then by SkriptHub `usage_score` if the user has opted into that source.
4. **Cache** per-line results keyed by line hash; invalidate on edit only for touched lines.

Target: **< 5 ms** to classify a line, **< 500 ms** to index a 1,000-file script folder.

### Feature map to LSP methods

| Brief requirement | Method | Notes |
|---|---|---|
| Completion (events/effects/conditions/expressions/functions/locals/globals/commands/options) | `textDocument/completion` + `completionItem/resolve` | Context-aware: after `on ` → events only; inside `if `/`while ` → conditions; inside `%…%` → expressions of the slot's type; statement position → effects. Pattern → snippet with tab stops at each `%slot%`. |
| Hover: description, syntax, examples, params, return, `since`, deprecation, addon | `textDocument/hover` | Rendered straight from `docs.json` fields (`description`, `patterns`, `examples`, `since`, `deprecated`, `returns`, `eventValues`, `requirements`). |
| Diagnostics | `textDocument/publishDiagnostics` | See tiers below. |
| Go to definition (functions, options, commands, variables) | `textDocument/definition` | |
| Find references | `textDocument/references` | |
| Rename | `textDocument/prepareRename` + `rename` | Functions, commands, options, and variables (incl. `%…%`-interpolated names, matched structurally). |
| Document symbols | `textDocument/documentSymbol` | Hierarchical: structure → section → nested sections. Drives Zed's outline. |
| Workspace symbols | `workspace/symbol` | |
| Formatting | `textDocument/formatting` + `rangeFormatting` | |
| **Folding** | `textDocument/foldingRange` | Every construct in the brief, with `collapsedText`. This is how folding actually reaches Zed. |
| Semantic tokens | `textDocument/semanticTokens/full` + `/delta` | Custom types listed above; `deprecated` modifier from `docs.json`. |
| Signature help | `textDocument/signatureHelp` | Function calls and `%slot%` positions. |
| Code actions / quick fixes | `textDocument/codeAction` | Phase-designed now, implemented as capacity allows. |

### Diagnostics tiering (the brief asks for informative, not noisy)

- **Error** — only what is *certainly* wrong: mixed tabs/spaces in one indent, wrong indent multiple, unterminated `###`, unterminated string, unknown function call, function arg count/type mismatch, duplicate function/command/option definition, `return` in a void function.
- **Warning** — deprecated syntax (`docs.json` `deprecated: true`), unreachable code after `stop`/`return`, reserved variable prefix (`~ . + $ ! & ^ *` — Skript itself warns), empty section, `local function` referenced from another script.
- **Hint / off by default** — "unknown syntax" (a line that matched no pattern). **This must default to off.** Any addon can register syntax we don't know about; flagging those would make the extension unusable on real servers. Surfaces only when the user has loaded addon docs.

### Docs data pipeline

- Primary: `https://docs.skriptlang.org/docs.json` (verified live: HTTP 200, `application/json`, 1,127,162 bytes, no auth, no key).
- Version-pinned: `https://docs.skriptlang.org/archives/<version>/docs.json` (verified for 2.15.3, 2.10.0; ~30 versions available).
- Cached under the OS cache dir keyed by version; ETag/`If-Modified-Since` revalidation; configurable via `lsp.skript-lsp.settings.skript_version` and `docs_url`.
- **Never vendored** — `skript-docs` is GPL-3.0 and this keeps the extension MIT. A tiny built-in fallback (structure keywords, control flow, types) ships so the editor is useful offline on first run.
- Optional, opt-in second source: SkriptHub `https://skripthub.net/api/v1/addonsyntaxlist/` (verified: 200, 7.7 MB, keyless) for addon syntax + `usage_score` ranking. Off by default — its terms page 404s, so there is no explicit redistribution grant; runtime fetch on user opt-in only. **skUnity is skipped entirely** (32-char API key required; unacceptable onboarding).

### Distribution

`cargo build --release` on CI for `x86_64-pc-windows-msvc`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` → GitHub Releases.

---

## 4. Extension `src/lib.rs`

Resolution chain, following `zed-extensions/lua`'s `language_server_binary` verbatim in shape:

```
lsp.skript-lsp.binary.path setting  →  worktree.which("skript-lsp")  →  download from GitHub release
```

Download logic follows `zed-extensions/java`'s `downloadable.rs`, which is the most robust pattern in the ecosystem:

- version-stamped directory as the on-disk cache key; prune sibling dirs after a successful download
- `set_language_server_installation_status(CheckingForUpdate | Downloading | Failed)`
- `make_file_executable` after extraction
- **24-hour update-check TTL marker** + `check_updates: always|once|never` setting
- **on network/rate-limit failure, fall back to any existing local install** rather than erroring — this is what stops GitHub API rate limits from killing the LSP on restart

```toml
[[capabilities]]
kind = "download_file"
host = "github.com"
path = ["<owner>", "ZedSkript", "**"]
```

`std::env::var` does not work in WASM — use `worktree.shell_env()`. `label_for_completion` / `label_for_symbol` are implemented so completion items render with proper syntax highlighting in the popup.

---

## 5. Testing

| Layer | Test |
|---|---|
| Grammar | `tree-sitter test` on `test/corpus/*.txt` |
| Grammar regression | **Parse all 543 `.sk` files from `SkriptLang/Skript`, assert zero ERROR nodes** (vendored as a git submodule or fetched in CI) |
| Highlights | `tree-sitter highlight` on `test/highlight/*.sk` with assertion comments |
| Pattern engine | Unit tests over all 2,117 patterns: every pattern must compile; every `examples` block in `docs.json` must classify correctly (this is a free, enormous, upstream-maintained test corpus) |
| Index | Unit tests for definition/reference/rename across multi-file fixtures |
| Formatter | Idempotency (`format(format(x)) == format(x)`) + comment preservation on the full 543-file corpus |
| LSP integration | Spawn the binary over stdio, drive real LSP sessions against `examples/` |
| Manual | Zed dev extension against `examples/`, checked under Catppuccin Mocha/Latte, Material Theme Darker, One Dark, and an OLED theme |

---

## 6. Documentation (`docs/`)

`installation.md` · `building.md` (rustup, `wasm32-wasip2`, wasi-sdk-25, `tree-sitter-cli` 0.25/0.26) · `architecture.md` (the tree-sitter/LSP split above) · `grammar.md` (node reference + scanner invariants) · `language-server.md` (settings, docs pipeline, diagnostic tiers) · `contributing.md` · `publishing.md` (grammar repo split, submodule PR, HTTPS-only URLs, LICENSE requirement, version bump in both `extension.toml` and `extensions.toml`) · `theming.md` (capture map, why no hardcoded colours, how to enable semantic tokens).

`README.md` gets an honest capability table — including that semantic tokens are opt-in and that unknown-syntax diagnostics are off by default and why.

---

## 7. Future-proofing (designed for, not built now)

`skript-docs` is provider-shaped from day one (SkriptLang / SkriptHub / local dump / generated-from-jar), which is what makes **addon support** a config change rather than a rewrite. `skript-index` is built as an incremental workspace index, which is what **dead-code detection, unused-variable warnings, workspace diagnostics, and refactoring** hang off. Code actions/quick fixes get their plumbing (`textDocument/codeAction` registered, empty provider) so adding one later is a single match arm. Debugger support: `debugger.scm` capture names (`@debug-variable`, `@debug-scope`) are documented in the plan but left unimplemented — there is no Skript debug adapter to target yet.

---

## Verification

```bash
# 1. Grammar
cd tree-sitter-skript && npx tree-sitter generate && npx tree-sitter test
npx tree-sitter parse '../vendor/Skript/src/**/*.sk' --quiet --stat     # expect 0 errors / 543 files

# 2. Language server
cd language-server && cargo test --workspace && cargo clippy -- -D warnings
cargo run -p skript-lsp -- --version

# 3. Extension builds as WASM
rustup target add wasm32-wasip2
cd extension && cargo build --release --target wasm32-wasip2

# 4. Load in Zed
#    command palette → "zed: install dev extension" → pick ZedSkript/extension
#    launch `zed --foreground` to see extension stdout in the terminal
```

**Manual acceptance pass** — open `examples/` in Zed and confirm:
- highlighting is correct and attractive under Catppuccin Mocha, Catppuccin Latte, One Dark, Material Theme Darker, and an OLED theme, with **no colour defined by us**
- pressing Enter after `on join:` / `command /x:` / `trigger:` / `if …:` indents; typing `else:` outdents and aligns to its `if`
- `#-#` lines do not indent; `###` blocks grey out entirely
- outline panel shows commands → entries → trigger, functions, events
- fold arrows appear on every section and collapse with sensible placeholder text
- hover on `broadcast` shows description + patterns + example + `since`
- completion after `on ` lists events only; inside `%…%` lists type-matching expressions
- go-to-definition on a function call jumps to its `function` line; rename updates every call site
- a deliberately mis-indented file reports an indentation error and nothing else noisy
- a file using an unknown addon syntax reports **no** diagnostics by default
