# ZedSkript — build status & handoff

Living checkpoint. `docs/PLAN.md` holds the approved plan; this file holds
**what is actually done, what is verified, and exactly what comes next**.

Last updated: 2026-08-06. **All seven workstreams complete.**

Repository: https://github.com/DaisyCatTs/SkriptZed

Phase 8 added addon support: detection from plugin manifests, the SkriptHub
catalog (168 addons / 12,877 patterns), version awareness, and the
`requires-addon` diagnostic. It also uncovered the catalog-deserialisation bug
documented below.

Since the first pass: project-wide indexing (definitions now resolve into files
you never opened), a formatter (`skript-format`), signature help, and both
helper scripts rewritten in Node so Windows needs no shell.

---

## Status at a glance

| # | Workstream | State |
|---|---|---|
| 1 | Monorepo scaffold + toolchain | ✅ done |
| 2 | `tree-sitter-skript` grammar + external scanner | ✅ done, 540/540 corpus clean |
| 3 | Grammar test corpus | ✅ done, 33/33 passing |
| 4 | Zed extension editor layer (queries, config, snippets) | ✅ done, builds for wasm32-wasip2 |
| 5 | `skript-lsp` Rust workspace | ✅ done — 115 unit tests + a 34-check end-to-end LSP session |
| 6 | Extension `src/lib.rs` LSP wiring | ✅ done (degrades cleanly with no server installed) |
| 7 | Docs, examples, CI | ✅ done |
| 8 | **Addon ecosystem + version awareness** | ✅ done — 168 tests + 40 end-to-end checks |
| 9 | **Classification accuracy pass** | ✅ done — 193 tests; see below |
| 10 | **Highlighting + IntelliSense pass** | ✅ done — 198 tests, 35 grammar tests; see below |
| 11 | **Production readiness / v0.1.0** | ✅ released — 201 tests, all five binaries published |

---

## v0.1.0 released (2026-08-07)

Tagged on `main`, five platform binaries published, `draft=false`
`prerelease=false` `assets=5` — exactly what the extension's
`latest_github_release` query requires. The "finding a prerelease" failure is
gone.

### The defect that would have broken every install

The grammar pin and the `v0.1.0` tag both pointed at commits reachable only from
`fix/classification-accuracy`, with `main` 15 commits behind. Zed resolves a
grammar by `git fetch --depth 1 origin <rev>`; deleting that branch after merge
— the normal flow — would have made the SHA unreachable and left every install
with no highlighting and no usable error. Branch merged, rev re-pinned to a
`main` commit, tag moved, and the fetch replayed from an empty directory to
prove it.

### Two independent reasons the release had been empty

1. The workflow only triggered on `main`/`master` and pull requests, so the
   release job's `refs/tags/v` gate was unreachable. Fixed with `tags: ['v*']`.
2. The release job had **no `permissions:` block**, and
   `softprops/action-gh-release` needs `contents: write`. The default token has
   been read-only for new repos since 2023, so the upload would have 403'd after
   three gate jobs and five cross-builds passed.

**Standing quirk:** on this repository, `push` events still create no workflow
runs, while `pull_request` and `workflow_dispatch` do. Cause unknown. Cut a
release with `gh workflow run ci.yml --ref v0.1.0` — dispatching against a tag
ref makes `github.ref` satisfy the release gate. That is how v0.1.0 shipped.

### Robustness

* **A panic that killed the server.** Completion and signature help sliced a
  line by the client's column with only a length clamp; under the UTF-8 encoding
  that column is a raw byte offset, and slicing mid-character panics. Skript
  strings are full of `§` and emoji. Both now go through `convert::line_prefix`,
  tested across every byte offset of a line containing both.
* **The cache left `/tmp`.** `/tmp/skript-lsp` was a predictable name in a
  world-writable directory — another local user could pre-create it and serve a
  crafted `docs-latest.json` as the authoritative syntax database — and was swept
  by systemd-tmpfiles, defeating the 24-hour TTL. Now uses the platform cache
  directory.
* **The offline fallback stopped being silent.** With no network the catalog
  drops to 17 entries with zero events, expressions or conditions, and said so
  only as an INFO log line. It now shows a warning naming what still works.

### Documentation

The README had no installation section at all. Install is now step one and
`semantic_tokens` step two, because Zed ships it `"off"` and it is what colours
effects and conditions — the most likely first impression otherwise is "half my
file is grey, this is broken".

Corrected the one claim that actively misled: `docsPath` does **not** make
`unknownSyntaxDiagnostics` safe, because `/sk gen-docs` emits one docs.json per
addon and Skript's describes Skript only. `docs/language-server.md` had this
right; the README contradicted it.

Also corrected `docs/publishing.md`, which told a maintainer to split the grammar
into its own repository — Zed supports a grammar subdirectory via the `path` key
(its own `extension_builder.rs`; Civet, cfml and `zed-extensions/php` all ship
that way). Deleted `docs/PLAN.md` (three components never built, plus an absolute
local path). Merged the two contributor docs into a root `CONTRIBUTING.md`.
Added CHANGELOG, SECURITY.md, issue forms, PR template, THIRD-PARTY-NOTICES.

### Still outstanding

* **Screenshots.** `docs/media/README.md` says exactly what to capture; the
  README has marked placeholders. Only Daisy can take them.
* **Registry submission.** Everything is verified and ready — see
  `docs/publishing.md`. Needs a fork of `zed-industries/extensions` to a
  *personal* account and a dev-extension test on a clean machine first; the
  registry closes untested submissions without feedback.

---

## Highlighting + IntelliSense pass (2026-08-06)

### The finding that reframed everything

Daisy had **never once run the language server**. The Zed log showed
`Could not find or download skript-lsp … finding a prerelease`, and all three
resolution paths missed at once: the repo had zero releases, there was no
`skript-lsp` entry anywhere in Zed's settings, and the binary was not on PATH.
Every judgement about this extension up to that point — "the intellisense is
ass", "some of the highlighting is very ass" — was made against **tree-sitter
alone with the entire semantic layer absent**.

`.zed/settings.json` (gitignored, per-machine) now points at the local release
build. That is the fix for testing; the release below is the fix for everyone
else.

### Highlighting: 85% → 90% grammar, 92% → 97% combined

Measured on `showcase.sk` with `scripts/coverage.mjs`.

Two of these were outright bugs, both from the same misreading of **repeated
fields**:

* Only the **first word of a multi-word event name** was coloured. `on first
  join` gives the event two `name:` children; the pattern matched once and left
  `join` plain — every multi-word event in the language. Fixed with a `+`
  quantifier.
* `do while …` coloured **neither** word: the predicate was tested against the
  first `header:` child, failed on `do`, and the match was dropped before
  `while`. Adding `do` to the list fixes the first word. `while` inside
  `do while` is still plain — one match per pattern per node — and that is an
  accepted limitation, not an oversight.

Grammar change: **colour codes outside strings**. `&6[Server]&r` in an
`options:` value or a `description:` entry was *mis-tokenised*, not merely
uncoloured — it lexed as `operator(&) number(6) punctuation([) …`. `legacy_color`
needed no change, only a new use site. Format tags needed a **separate stricter
token**: `format_tag`'s body is safe inside a string where `<` cannot be an
operator, but in bare content it would swallow `< 5 and {_b} >` out of
`if {_a} < 5 and {_b} > 3:`. `_bare_format_tag` sits at prec(1) — above
`operator` at 0, below `command_argument`'s `<` at 3.

Query-only additions: command entry values as text (nested under `command` so an
`options:` value, which may genuinely be code, is excluded — verified against
showcase.sk:13); the three variable scopes distinguished by positive `#match?`
tests that degrade to plain `@variable`; `default_value` and the structural
`:` `=` `,`; `legacy_color` off `@string.escape`, which had made `&6` and a
literal `""` indistinguishable.

### Indentation: two real defects

`increase_indent_pattern` began `^[^#]*`, forbidding `#` anywhere on the line —
but `#` is literal inside a string and a variable name, so `send "Item #1":` and
`set {count##} to 1:` never opened a block, though `scanner.c` correctly emitted
SECTION_COLON for both. The `$` anchor also failed on CRLF.

Replaced with a pattern that spells out the code portion the way
`Node.splitLine` does. **Validated old-vs-new over all 19,391 lines of the
upstream corpus: zero regressions, four genuine fixes in Skript's own
EvtFish.sk.**

### IntelliSense

* **Completion was offering unresolvable names.** It drew from
  `workspace_symbols`, which exists to back the project-wide picker; so another
  file's `{_local}` variables and options appeared in every trigger. Now uses
  `symbols_in_scope`, applying the same `is_file_local` rule as `definitions`.
* **Inlay hints** — parameter names at call sites. Two things the tests caught
  that reasoning had not: the matching close paren must be found by depth, or
  `= (1, 7)` truncates every later parameter; and a comma inside a string or a
  nested call must not split an argument.
* **Document highlight** — resolved through the same `symbol_under_cursor` path
  as go-to-definition, so the editor cannot highlight one symbol while F12
  navigates to another.

**Deliberately not done — command references.** In Skript a command is invoked
from inside a string (`execute player command "/home"`), so indexing those would
let a rename rewrite arbitrary text. Absent beats unsafe.

**Deliberately not done — self-injecting `skript` into `interpolation_text`.**
An expression fragment like `uuid of player` parsed as a whole document yields
an ERROR-rooted subtree with a MISSING section colon: strictly worse than the
current flat `@embedded`. A `#match?` on the existing node gives `%arg-1%` and
`%loop-value%` their builtin colour for free instead.

### GitHub Actions is not running — needs Daisy

`v0.1.0` is tagged and pushed, and the workflow now triggers on `v*` tags (it
only listened to `main`/`master` before, which is why a tag-gated release job
was unreachable). But **no run is created for any event** — tag, branch or PR —
and the one run that ever existed died with *"The job was not acquired by Runner
of type hosted even after multiple attempts"*. Actions is enabled and the
workflow is active, so this is an account-level runner/billing problem that has
to be resolved in GitHub's settings. Until then there are no release binaries
and the extension cannot self-install its server.

---

## Classification accuracy pass (2026-08-06)

Written after building `scripts/coverage.mjs`, which measures what fraction of a
real script the extension actually explains. The first run said **68%** of
executable lines were classified. Three defects accounted for the gap, and all
three were invisible to the existing tests because those tests only exercised
patterns that happened to avoid them.

**1. The whole-line rule sat outside the search.** `match_pattern` ran
`match_nodes` once and then tested `end == tokens.len()`. A pattern ending in a
slot therefore succeeded on the first token that slot could take, failed the
length test, and could not backtrack. `wait %timespan%` never matched
`wait 3 seconds`; `loop %objects%` matched `loop {_x::*}` but not
`loop all players`. Fixed by threading the continuation through the search as a
stack of node lists, so exhausting the pattern demands exhausting the line.

**2. Groups glued to a word never matched — 38% of all published patterns.**
`cancel[l]ed`, `block[s]`, `[right|left]click`, `ha(s|ve)`, `toggl(e|ing)`.
Matching is token-based, so `Literal("cancel")` was compared against the whole
token `cancelled`. Worse, the inverted index keyed those patterns on a word that
can never appear in a line, so they were never offered to the matcher at all.
The parser now records which nodes are written with no space between them and
spells the run out at parse time, fixing the index and the match together.

**3. Expressions were allowed to explain whole lines.** Skript ships three
expressions that match literally any text — `[the] [event-]<.+>` foremost. They
are correct as expressions, because an expression is only ever *part* of a line.
Once (1) was fixed they began winning whole lines, and nothing was reportable as
unknown syntax any more. `LineRole` now rules categories out by indentation:
column 0 is a structure or an event, an indented line is an effect, section or
condition, and an expression is never either.

Two things fell out of having the role available:

* An event's registered pattern does **not** contain the `on` — Skript's event
  structure wraps every one in `[on] … [with priority …]` before matching.
  Undoing that wrapper lets `on first join` reach `first (join|login)` and its
  documentation instead of the generic structure.
* Structures are keyword-introduced, so at the top level they get the first
  look. Otherwise the "on command" event outranks the command *structure* on
  `command /home <text>:`.

Also: a bare function-call statement (`giveKit(player)`) is real Skript with no
published pattern, and is now classified from the reference index that already
powers go-to-definition; and prose inside `###` blocks is no longer classified,
matching what `skript-format` and the indentation diagnostics already did.

### Measured before → after

| File | Lines classified | Note |
|---|---|---|
| `examples/…/showcase.sk` | 68% → **85%** | remainder is command/options *entries* |
| `…/tests/misc/EntityData.sk` | 5% → **100%** | ~1,000 bare function-call statements |
| `…/expressions/ExprArithmetic.sk` | 100% → **100%** | already clean |
| `…/general/EquippableComponents.sk` | 72% → 72% | remainder is the test-only `assert` effect |

Latency, measured end to end through the LSP: **12 ms** for a 67-line script,
**169 ms** for a 1,023-line one. Matching costs 272 µs/line against all 2,117
core patterns — the budget in the plan was 5 ms.

`coverage.mjs` also reports a **combined** figure (tree-sitter ∪ semantic),
because either layer alone understates what a reader sees: the grammar
deliberately leaves statement prose uncoloured, since only the server can tell
an effect from a condition. showcase.sk is **92%** combined.

> Its own first version undercounted multi-line captures, so a `###` block read
> as uncoloured and showcase.sk scored 78% when it was really 85%. Check the
> harness before believing the harness.

---

## Critical bug found and fixed (2026-08-06, during addon work)

``Docs::parse`` had **never succeeded on the real `docs.json`.** Three schema
mismatches, each of which aborts deserialisation of the whole file:

| Field | Modelled as | Actually is |
|---|---|---|
| `requirements` | `Option<String>` | `array<string>` on 105 entries |
| `keywords` | `Option<String>` | `array<string>` on 39, `null` on 992 |
| `description` | `Vec<String>` | array on 1,207 but a **bare string** on 14 |

On top of that, `#[serde(default)]` only covers a **missing** key, not an
explicit `null` — and the generator emits `null` on `requirements` (1,050),
`events` (771), `examples` (65), `patterns` (42), `eventValues` (3),
`description` (1) and `since` (2).

**Consequence:** `load_catalog` always took its error branch, so the shipped
server ran on the ~20-entry built-in fallback. Hover, completion and semantic
tokens were all running on almost no data. Nothing failed loudly, because the
fallback is by design a silent degradation.

**Why the tests missed it:** the smoke test only exercised features that do not
need the catalog (symbols, folding, definition, rename), and the pattern-engine
tests read `docs.json` with `serde_json::Value`, never through the typed model.

**Fixed by** correcting the three field types, adding a `nullable` deserializer
to 18 fields and a `string_list` deserializer that accepts string|array|null,
and adding `crates/skript-docs/tests/real_data.rs`, which now asserts the real
file deserialises, the catalog indexes **2,660 patterns with 0 unparsable**, and
known lines classify. Its helper **panics on a present-but-unparseable file**
rather than skipping — the earlier helper returned `None` for both cases, which
is exactly how this stayed hidden.

---

## Verified environment facts (re-checked on this machine, not from memory)

* rustc/cargo **1.96.1**; node **v24.16.0**; JDK 21 (Corretto); git 2.54.
* Installed rust targets: **only `x86_64-pc-windows-msvc`** — `wasm32-wasip2`
  still needs `rustup target add wasm32-wasip2`.
* `tree-sitter-cli` **0.26.11** installed as a devDependency inside
  `tree-sitter-skript/`. Generated `src/parser.c` is **ABI 15** (verified via
  `#define LANGUAGE_VERSION 15`).
* Zed is installed; the user's active theme is **Catppuccin Mocha / Latte**.
* `zed_extension_api` latest published stable is **0.7.0**. `0.8.0` exists in
  the Zed repo with `publish = false`, and Zed stable caps extensions at 0.7.0
  (`wasm_host/wit.rs`: Stable/Preview → `since_v0_6_0::MAX_VERSION`).

### Node/Windows gotchas already hit and solved

* `execFile` cannot run `node_modules/.bin/tree-sitter` (POSIX shell script) nor
  `tree-sitter.cmd` (Node blocks `.cmd` since CVE-2024-27980). Call
  `node_modules/tree-sitter-cli/tree-sitter.exe` directly — see
  `scripts/parse-corpus.mjs`.
* Passing ~540 paths as argv silently overflows on Windows and the CLI returns
  nothing, which made an early version of the corpus script report a **false
  clean run**. It now uses `--paths <file>` and *asserts* that the parsed count
  equals the discovered file count before believing any result.
* `tree-sitter generate | head` masks a non-zero exit. Always capture to a log
  and check `$?` separately.

---

## Verified Zed facts (read from Zed's source, not from docs)

These are the load-bearing ones; full detail in `docs/PLAN.md`.

* **Zed loads exactly ten query files** (`crates/language_core/src/queries.rs`,
  `QUERY_FILENAME_PREFIXES`): `highlights brackets outline indents injections
  overrides redactions runnables debugger textobjects`.
  **`folds.scm` and `locals.scm` are NOT loaded** — the Java extension ships a
  `folds.scm` and Zed silently ignores it. Do not write one.
* **Folding** reaches Zed from indentation/brackets and from the language server
  via `textDocument/foldingRange` (`crates/project/src/lsp_store/folding_ranges.rs`,
  which also honours `collapsedText`).
* **Semantic tokens are supported** (`docs/src/semantic-tokens.md`,
  `crates/editor/src/semantic_tokens.rs`) but the `semantic_tokens` setting
  **defaults to `"off"`**. An extension may ship
  `languages/<lang>/semantic_token_rules.json`; Zed loads it in
  `extension_host.rs` (~L1400) and ranks it above Zed defaults, below user
  settings.
* **Indentation**: `Buffer::suggest_autoindents` merges `indents.scm` with
  `increase_indent_pattern` / `decrease_indent_patterns`. Tree-sitter
  suggestions are discarded inside ERROR ranges but regex ones survive
  (`within_error && !from_regex`) — so an indentation-sensitive language must
  lead with regex. Zed's own YAML has no `indents.scm` at all.
  `decrease_indent_patterns[].valid_after` entries are matched against
  `@start.<suffix>` captures in `indents.scm`.
* **Grammars** are fetched with `git remote add origin <repository>` +
  `git fetch --depth 1 origin <rev>`, then `src/parser.c` (and `src/scanner.c`
  if present) are compiled with wasi-sdk-25 clang. Zed **never runs
  `tree-sitter generate`**, so `src/parser.c`, `src/grammar.json`,
  `src/node-types.json` and `src/tree_sitter/*.h` must all be committed.
  Only `scanner.c` is picked up — **`scanner.cc`/`.cpp` are silently ignored**.
  A `file://` URL is a valid `repository` for local development.
* Extension crate target is **`wasm32-wasip2`**.
* `snippets = ["./snippets/skript.json"]` must be declared explicitly in
  `extension.toml`; the file's basename must be the lowercased language name.
* A `LICENSE` file at the extension root is **required** by the Zed registry
  (since 2025-10-01).
* Theme portability: `SyntaxTheme::highlight_id` degrades a dotted capture to
  its longest defined dot-prefix (`string.special.symbol` → `string.special` →
  `string`). Inventing a **bare** top-level capture name has no fallback.
  Catppuccin defines 101 capture names; bundled One defines 46.

---

## Verified Skript facts

* Current Skript is **2.16.1**. Docs schema version 2.0.
* **`https://docs.skriptlang.org/docs.json`** — 1,127,162 bytes, HTTP 200, no
  auth, no API key. Contains 522 expressions, 182 events, 166 conditions,
  144 effects, 124 types, 45 functions, 9 sections, 8 structures, 8 experiments
  (**1,208 entries / 2,117 patterns**). Per-version archives at
  `/archives/<version>/docs.json`. Repo is **GPL-3.0** → fetch at runtime, never
  vendor.
* SkriptHub `https://skripthub.net/api/v1/addonsyntaxlist/` — 7.7 MB, keyless,
  covers addons and carries a `usage_score`. Its `/terms/` page 404s, so there
  is no explicit redistribution grant: runtime fetch, opt-in only.
* skUnity requires a 32-char API key — **skipped**.
* No usable Skript LSP exists: IntelliSkript (GPL-3.0, 32.8k VS Code installs)
  is the only real one; `nlaocs/Skript-LSP`'s README says its binary "is still a
  scaffold"; `skript-studio/skript-lsp` is WebSocket-only; SkriptInsight is
  archived since 2020; `r4tsk` is all-rights-reserved.
* No Skript extension exists for Zed (checked `zed-industries/extensions`,
  1,355 entries).

### Language details the grammar had to honour

* Indentation unit is inferred **once per file** from the first depth-1 indent;
  tabs and spaces may not be mixed within one indent; every line must be an
  exact multiple. (Enforce in LSP diagnostics, be lenient in the scanner.)
* A line opens a section iff its code portion ends in `:` — **unless** the line
  carries the `#-#` marker, which explicitly suppresses that.
* A line whose *trimmed* content is exactly `###` toggles a block comment
  (Skript 2.9+). `### foo` is an ordinary comment.
* `#` is **literal inside a string and inside a variable name**
  (`Node.splitLine` state machine). This is why `comment` must not be a
  tree-sitter `extra`.
* Inside `%…%` Skript re-enters code context, so `"%uuid of world "world"%"` —
  a string nested inside an interpolation inside a string — is legal.
* A variable name may nest another variable: `{_test::{_x}}`.
* Function return markers: **`::`, `->`, and ` returns `** are all live.
  `local function` since 2.7. The `(` … `)` may come from an option expansion.
* Event structure is `[on] [uncancelled|cancelled|(any|all)] <.+> [with priority …]`
  — the modifier follows `on`.
* Command leading `/` is optional. Entries: `usage description prefix permission
  "permission message" aliases "executable by" cooldown "cooldown message"
  "cooldown bypass" "cooldown storage"` + the `trigger:` section.
* Effects / conditions / expressions are **syntactically indistinguishable**.

---

## What exists on disk now

```
ZedSkript/
├─ LICENSE                     MIT
├─ .gitignore
├─ PROGRESS.md                 this file
├─ docs/PLAN.md                the approved plan
├─ scripts/parse-corpus.mjs    540-file regression harness (self-verifying)
├─ vendor/skript-corpus/       sparse clone of SkriptLang/Skript (540 .sk files)
├─ tree-sitter-skript/         ✅ COMPLETE
│  ├─ grammar.js               fully commented; the design rationale lives here
│  ├─ tree-sitter.json         ABI 15 metadata
│  ├─ package.json .gitattributes .gitignore
│  ├─ src/{parser.c,grammar.json,node-types.json,tree_sitter/*}   generated, must be committed
│  ├─ src/scanner.c            external scanner (INDENT/DEDENT/NEWLINE/SECTION_COLON/BLOCK_COMMENT)
│  └─ test/corpus/{structures,literals,layout}.txt                33 tests, all passing
├─ extension/                  🔨 skeleton dirs only
└─ language-server/            ⬜ empty
```

### Grammar quality gates (both currently green)

```sh
cd tree-sitter-skript && ./node_modules/.bin/tree-sitter test      # 33/33
node scripts/parse-corpus.mjs --strict                             # 540/540 clean
```

---

## Design decisions already locked in

1. **tree-sitter models structure; the LSP models semantics.** The grammar never
   classifies a line as effect/condition/expression — 2,117 runtime-registered
   patterns make that impossible in a CFG. The LSP does it and returns semantic
   tokens.
2. **No `folds.scm`** — Zed does not read it. Folding comes from the LSP's
   `textDocument/foldingRange`.
3. **Indentation is regex-first** in `config.toml`, with `indents.scm` carrying
   only bracket pairs and `@start.if` / `@start.else-if` anchors.
4. **No hardcoded colours anywhere**; only theme-portable capture roots, relying
   on Zed's dot-prefix fallback.
5. **`docs.json` is downloaded at runtime and cached**, never vendored, so the
   extension stays MIT while Skript's docs stay GPL-3.0.
6. **Grammar splits into its own GitHub repo** before publishing; during
   development `extension.toml` points `repository` at a `file://` URL.

---

## Next steps, in order

### 4. Zed extension editor layer (in progress)

Already written and validated with `tree-sitter query` against a sample script:

* `languages/skript/config.toml` — regex-first indentation, `###` block comment,
  brackets with `not_in` scopes, `word_characters` covering `loop-player`.
* `languages/skript/highlights.scm` — validated; colours structure, literals,
  variables, options and a small closed set of control-flow words only.
* `languages/skript/brackets.scm` — note a `(punctuation "(" @open ")" @close)`
  pattern is an "Impossible pattern": a `punctuation` node holds exactly one
  token. Only structural pairs can be matched.
* `languages/skript/indents.scm`, `outline.scm` (verified against the outline
  captures Zed requires), `overrides.scm`, `textobjects.scm`.
* `languages/skript/semantic_token_rules.json`.

Still to write:

* `extension/extension.toml` — `schema_version = 1`, `[grammars.skript]` with a
  `file://` repository + local rev, `[language_servers.skript-lsp]`,
  `snippets = ["./snippets/skript.json"]`, `[[capabilities]] kind = "download_file"`.
* `extension/Cargo.toml` — `crate-type = ["cdylib"]`, `zed_extension_api = "0.7.0"`.
* `extension/LICENSE` — copy of the MIT file (registry requirement).
* `snippets/skript.json` — ~60 snippets.
* No `injections.scm` is planned: Skript embeds no other language, and an empty
  query file is worse than none.
* `languages/skript-config/` is **deferred** — see the known limitation below.

Note the grammar repo must be committed before the extension can load it: Zed
resolves `[grammars.skript]` by `git fetch --depth 1 origin <rev>`, so `rev`
must name a real commit even for a `file://` repository.

Node types the queries can target (from `src/node-types.json`): `keyword,
command_name, command_argument, argument_spec, punctuation, identifier, word,
operator, string, escape_sequence, interpolation, interpolation_text,
format_tag, legacy_color, variable, variable_scope, variable_name,
variable_text, list_separator, option_ref, option_name, number, boolean,
duration, loop_value, event_value, command_arg_ref, comment, block_comment,
entry, entry_key, entry_section, assignment, alias_name, type, return_marker,
default_value, experiment, import_path, parameter, parameter_list,
function_call, event_modifier, event_priority, section, statement, block,
entry_body, assignment_body, import_body, source_file` plus the structures
`event command function options variables aliases import using auto_reload`.

### 5. `skript-lsp` (Rust workspace)

**`skript-syntax` is complete and verified.** `language-server/` is a cargo
workspace; `crates/skript-syntax` parses Skript's pattern DSL (`[optional]`,
`(a|b)`, `%~type/other%`, `<regex>`, `:tag` and `1¦` parse marks, `\|` escapes)
and matches lines against it with a lazy, budget-bounded backtracker. A
`PatternIndex` inverts patterns on their rarest required literal.

Measured against the real database (`cargo test -p skript-syntax`, 32 tests):

| Metric | Result |
|---|---|
| Patterns parsed | **2,117 / 2,117** |
| Index narrowing | ~**170 of 2,117** candidates per line (12×) |
| Classification | **433 µs / line** in a *debug* build (budget was 5 ms) |

Two bugs the real corpus caught, both fixed and regression-tested:
* `<` is Skript's less-than operator as well as the regex delimiter —
  `CondCompare` registers `(…|<) %objects%`. A `<` only opens a regex when a
  `>` follows before the enclosing group ends.
* `(exit|stop) [trigger]` has no individually-required word, so it scored zero
  specificity and lost to any expression accepting a bare `%objects%`. A
  mandatory choice between fixed words now scores like a literal.

Run `scripts/fetch-docs.mjs` to place `vendor/docs.json` (GPL-3.0, gitignored);
without it those tests skip rather than fail.

**Still stubs:** `skript-docs` (fetch/cache + typed model), `skript-index`
(workspace symbols, incremental invalidation), `skript-lsp` (the tower-lsp
binary). Targets: <500 ms to index 1,000 files.

Diagnostic tiers are specified in `docs/PLAN.md` §3 — note especially that
**"unknown syntax" must default to off**, because any addon can register syntax
we do not know about.

### 6. Extension ↔ LSP wiring
Resolution chain `settings.binary.path → worktree.which("skript-lsp") →
GitHub release download`, with a version-stamped cache dir, sibling pruning, a
24-hour update-check TTL marker, and **fallback to an existing local install on
network failure** (the `zed-extensions/java` `downloadable.rs` pattern).

### 7. Docs, examples, CI
`docs/{installation,building,architecture,grammar,language-server,contributing,publishing,theming}.md`,
`examples/`, and GitHub Actions running `tree-sitter test`,
`node scripts/parse-corpus.mjs --strict`, `cargo test`, plus a cross-platform
release job for the five LSP targets.

---

## Known limitations to document in the README

* The grammar treats an indent as any self-consistent whitespace run; Skript's
  stricter per-file rule is reported by the language server instead.
* `<` and `>` are Skript comparison operators *and* command-argument
  delimiters. The grammar resolves this by token precedence, which is only
  correct inside a command header — `usage: /home <name>` lexes `<` as an
  operator, which is the right call for an entry value.
* A `command_argument`'s inner text is kept as one `argument_spec` token;
  splitting `<x:number>` / `<text="success 2">` into name/type/default needs
  lookahead LR does not have, so the language server does it.
* `yes`/`no`/`on`/`off` are not highlighted as booleans: `on` opens every event
  and mis-colouring it is worse than leaving it plain.
* **Skript's own `config.sk` / `features.sk` parse without error but produce a
  poor tree.** Those files are top-level `key: value` lines, and because a
  newline is an ordinary `extra`, `_content` in an event header runs straight
  past it — the whole file collapses into one `event` whose name spans every
  line. Real scripts are unaffected (every top-level line ends in `:`), which is
  why the 540-file corpus stays clean.

  **An attempt to fix this by adding `$.entry` to `_top_level` was reverted.**
  Lowering `entry_key`'s precedence so it stops outranking structure keywords
  makes GLR prefer the `entry` reading everywhere: the corpus fell to 2/540 and
  every unit test failed, because `variables:` and `on join:` both started
  parsing as entries. A working fix has to stop `_content` from crossing a
  newline in the first place — most likely a scanner-emitted line terminator
  that the header rules require — rather than adding an ambiguous alternative at
  the top level. Until then, treat Skript config files as out of scope.
