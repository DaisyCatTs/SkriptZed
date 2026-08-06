# Skript for Zed

Language support for [Skript](https://github.com/SkriptLang/Skript), the
Minecraft server scripting language — a tree-sitter grammar with real
indentation handling, and a language server that understands Skript's syntax
rather than guessing at it.

Before this, Zed had no Skript support at all. Elsewhere the picture was not
much better: every existing Skript language server is GPL-3.0, archived, or an
admitted scaffold, and the two existing tree-sitter grammars are single-day
experiments with no indentation model.

---

## What works

| | |
|---|---|
| **Syntax highlighting** | Structures, sections, strings with `%interpolation%` / `<format tags>` / `&6` colour codes, variables by scope, options, commands, functions, literals, comments |
| **Indentation** | Auto-indent after any `:`-terminated line; `else` / `else if` snap back to their `if`; honours Skript's `#-#` marker |
| **Folding** | Every event, command, function, section, `options:`/`variables:` block and `###` comment, each with a placeholder saying what was folded |
| **Outline** | Nested — commands contain their entries, events contain their sections |
| **Go to definition** | Functions, commands, options, variables — across the whole project, including files you never opened, respecting `local function` scope |
| **Find references / rename** | Same, with `{_local}` variables correctly confined to their own file |
| **Hover** | Description, syntax, examples, event values, `since`, deprecation and addon requirements, straight from Skript's own database |
| **Completion** | Context-aware — after `on ` only events, inside `if ` only conditions, inside `%…%` only expressions; patterns insert as snippets with a tab stop per slot |
| **Diagnostics** | Indentation Skript would reject, unclosed `###`, duplicate declarations, calls to functions that do not exist, deprecated syntax |
| **Formatting** | Re-indents from the parse tree, so an inconsistently indented file comes out correct rather than consistently wrong; never touches anything inside a line |
| **Signature help** | Parameter hints while typing a function call |
| **Semantic tokens** | Effects, conditions, expressions and events coloured by what they actually are |
| **Snippets** | ~60, covering every structure, event, loop and common effect |
| **Addons** | Detects your server's addons from their plugin manifests and loads their syntax — SkBee, skript-reflect, SkQuery, TuSKe, MundoSK and 160+ others |
| **Version awareness** | Labels syntax your target Skript cannot run, and never hides it |

## What to know before you install

Three things are deliberately not what you might expect.

**Statement prose is not coloured by the grammar.** Skript has no fixed
vocabulary: every effect, condition and expression is a pattern registered at
runtime — 2,117 of them in core Skript alone, and any addon adds more. Nothing
lexical distinguishes `set {_x} to 5` from `player is op`. The grammar therefore
colours only what is structurally certain, and the language server supplies the
rest as semantic tokens. Skript's own highlighting guidance puts it well: *"It
is better not to highlight a structure than to incorrectly identify it."*

**Semantic tokens are off by default in Zed.** To get the full colouring:

```json
{
  "languages": {
    "Skript": { "semantic_tokens": "combined" }
  }
}
```

**"Unknown syntax" diagnostics are off by default.** Any addon can register
syntax this extension has never heard of, so reporting unmatched lines would
light up every script on a real server. Turn it on only if you have pointed the
server at a `docs.json` generated from your own server:

```json
{
  "lsp": {
    "skript-lsp": {
      "initialization_options": {
        "docsPath": "/path/to/your/docs.json",
        "unknownSyntaxDiagnostics": true
      }
    }
  }
}
```

## Themes

**No colour is defined anywhere in this extension.** Highlighting uses capture
names your theme already styles, and Zed resolves a dotted capture back along
its prefixes (`string.special.symbol` → `string.special` → `string`), so a theme
that defines less still gets sensible results. Tested against Catppuccin
(Mocha/Latte), One Dark, Material Theme Darker, and OLED themes.

If you use a theme with `semantic_tokens` enabled, the language server's
classification is mapped onto theme style names — never colours — in
`languages/skript/semantic_token_rules.json`.

## Addons

Most Skript projects run addons, so the server detects them: it finds your
`plugins/` directory, reads each JAR's manifest, and loads syntax for what is
actually installed. SkBee ships only a `paper-plugin.yml`, which is why the
manifest is read rather than the filename.

Nothing to configure if your scripts live under `plugins/Skript/scripts/`.
Otherwise point at the server, or name your addons directly:

```json
{
  "lsp": {
    "skript-lsp": {
      "initialization_options": {
        "serverPath": "/srv/minecraft/my-server"
      }
    }
  }
}
```

See [docs/addons.md](docs/addons.md) — including what is *not* supported, and
why "unknown syntax" is never treated as an error.

## Settings

All optional, under `lsp.skript-lsp.initialization_options`:

| Setting | Default | Meaning |
|---|---|---|
| `addons` | `"auto"` | `"auto"` detect from `plugins/` · `"off"` · or a list of names |
| `serverPath` | – | Where `plugins/` lives, if not above the workspace |
| `addonSyntaxSource` | `"skripthub"` | Where addon syntax comes from, or `"off"` |
| `customSyntaxPaths` | `[]` | Your own syntax files, for private addons |
| `skriptVersion` | latest | Pin the syntax database to a Skript version, e.g. `"2.15.3"` |
| `docsPath` | – | A `docs.json` generated on your server with `/sk gen-docs`, to match its exact Skript build |
| `docsUrl` | – | Fetch the database from a mirror |
| `unknownSyntaxDiagnostics` | `false` | Report lines matching no known syntax |
| `deprecatedSyntaxDiagnostics` | `true` | Warn on syntax upstream has deprecated |

Format on save, if you want it:

```json
{ "languages": { "Skript": { "format_on_save": "on" } } }
```

To use a language server you built or installed yourself:

```json
{ "lsp": { "skript-lsp": { "binary": { "path": "/path/to/skript-lsp" } } } }
```

## Where the syntax data comes from

Hover and completion are driven by
[`docs.skriptlang.org/docs.json`](https://docs.skriptlang.org/docs.json) —
Skript's own generated database, 1,222 entries and 2,660 patterns, no API key —
plus [SkriptHub's addon catalog](https://skripthub.net/api/v1/addonsyntaxlist/),
which covers 168 addons and 12,877 patterns.

It is **downloaded at runtime and cached**, never bundled. That is partly
licensing — the database is GPL-3.0 and this project is MIT — and partly that it
keeps the docs matched to the Skript version you actually target. With no
network on first run, a small built-in catalog takes over and everything that
does not need it (highlighting, outline, folding, go-to-definition, rename)
still works.

## Building

Every script here is Node, so the same commands work on Windows, macOS and
Linux — no shell required.

```sh
# Grammar
cd tree-sitter-skript && npm install
./node_modules/.bin/tree-sitter generate
./node_modules/.bin/tree-sitter test

# Language server
cd language-server && cargo build --release -p skript-lsp

# Extension, and a local dev install
rustup target add wasm32-wasip2
node scripts/dev-setup.mjs          # commits the grammar and points extension.toml at it
# then in Zed: "zed: install dev extension" -> pick ./extension
```

See [`docs/`](docs/) for architecture, the grammar's design, theming and
publishing, and [`CLAUDE.md`](CLAUDE.md) for the constraints that shape the code.

## Testing

```sh
cd tree-sitter-skript && ./node_modules/.bin/tree-sitter test   # grammar unit tests
node scripts/parse-corpus.mjs --strict                          # 540 real Skript files
cd language-server && cargo test                                # language server
node scripts/smoke-lsp.mjs                                      # end-to-end LSP session
```

`scripts/fetch-docs.mjs` and `scripts/fetch-addons.mjs` place the two syntax
databases so the tests can run against all 2,660 core and 12,877 addon patterns.
Neither file is committed — both are third-party data.

The corpus gate parses every `.sk` file in SkriptLang's own repository and
requires zero errors — it is maintained by the people who define the language,
which makes it the most honest test available. `scripts/fetch-docs.mjs` places
the syntax database so the pattern-engine tests can run against all 2,117 real
patterns.

## Licence

MIT. Skript itself, and its documentation database, are GPL-3.0 — neither is
bundled here.
