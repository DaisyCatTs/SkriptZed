# Skript for Zed

Write Minecraft server scripts in Zed, with the tooling Skript has never had.

[![CI](https://github.com/DaisyCatTs/SkriptZed/actions/workflows/ci.yml/badge.svg)](https://github.com/DaisyCatTs/SkriptZed/actions/workflows/ci.yml)
[![Zed Extension](https://img.shields.io/badge/Zed-extension-084CCF?logo=zedindustries&logoColor=white)](https://zed.dev/extensions?query=skript)
[![Skript 2.16](https://img.shields.io/badge/Skript-2.16-8B5CF6)](https://github.com/SkriptLang/Skript)
[![Licence: MIT](https://img.shields.io/badge/licence-MIT-green.svg)](LICENSE)

**Skript for Zed** is full language support for
[Skript](https://github.com/SkriptLang/Skript) — the scripting language Minecraft
server owners use to write plugins without writing Java. Before this extension,
Zed had no Skript support at all.

Install it and `.sk` files stop being plain text. You get highlighting that
understands Skript's indentation rules, an outline of every command, event and
function, go-to-definition and rename across your whole project, hover
documentation straight from Skript's own database, completion that knows only
events can follow `on `, and diagnostics for the indentation Skript would reject
— before you upload the file.

It knows your addons too: it reads your server's `plugins/` folder and loads
syntax for SkBee, skript-reflect, SkQuery and 160+ others.

No colour is defined anywhere in it, so it looks right in whatever theme you
already use.

<!-- SCREENSHOT: hero.png goes here. See docs/media/README.md for what to capture. -->

---

## Install

**1. Install the extension.** In Zed, open the command palette
(<kbd>Cmd/Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>), run **`zed: extensions`**,
search for **Skript**, and click **Install**. Open any `.sk` file — that is it.
The language server downloads itself the first time you open a Skript file;
there is nothing to install by hand.

**2. Turn on semantic tokens. You want this.** Zed ships with `semantic_tokens`
set to `"off"`, and semantic tokens are what colour Skript's effects, conditions
and expressions — which is most of what you actually write. Without this setting
the extension works, but a lot of your file stays grey.

Run **`zed: open settings`** and add:

```json
{
  "languages": {
    "Skript": { "semantic_tokens": "combined" }
  }
}
```

<!-- SCREENSHOT: semantic-tokens.png (off vs combined) goes here. -->

On the bundled showcase file the grammar alone colours 90% of visible
characters; with semantic tokens it is 97%. Prefer `"combined"` over `"full"` —
`"full"` discards what the grammar already knows about strings and variables.

<details>
<summary>Optional: indentation and format-on-save</summary>

```json
{
  "languages": {
    "Skript": {
      "semantic_tokens": "combined",
      "tab_size": 4,
      "hard_tabs": true,
      "format_on_save": "on"
    }
  }
}
```
</details>

Not published to the registry yet? See
[installing from source](CONTRIBUTING.md#running-it-before-it-is-published).

## What you get

| | |
|---|---|
| **Highlighting** | Structures, sections, strings with `%interpolation%`, `<format tags>` and `&6` colour codes, variables by scope, options, commands, functions, literals, comments |
| **Indentation** | Auto-indent after any `:` line, `else` snapping back to its `if`, and Skript's `#-#` marker for a colon that does *not* open a section |
| **Folding & outline** | Every event, command, function and section, with commands showing their entries nested underneath |
| **Navigation** | Go to definition, find references and rename — project-wide, including files you have not opened, respecting `local function` and file-scoped `{_variables}` |
| **Hover** | Description, syntax, examples, event values, `since`, deprecation and addon requirements, from Skript's own database |
| **Completion** | Context-aware: only events after `on `, only conditions inside `if `, only expressions inside `%…%`. Inserts as snippets with a tab stop per slot |
| **Signature help** | Parameters as you type a function call |
| **Inlay hints** | Parameter names shown at call sites, so `giveKit(p, 3)` is readable |
| **Occurrence highlighting** | Every use of the symbol under your cursor |
| **Diagnostics** | Indentation Skript would reject, unclosed `###` blocks, duplicate declarations, calls to functions that do not exist, deprecated syntax |
| **Formatting** | Re-indents from the parse tree, and refuses to touch a file that does not parse |
| **Snippets** | 61, covering every structure, event, loop and common effect |

## Addons

Most Skript projects run addons, so the server detects them: it finds your
`plugins/` directory, reads each JAR's manifest, and loads syntax for what is
actually installed. SkBee ships only a `paper-plugin.yml`, which is why the
manifest is read rather than the filename.

Nothing to configure if your scripts live under `plugins/Skript/scripts/`.
Otherwise point at the server:

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
why unknown syntax is never treated as an error.

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
| `unknownSyntaxDiagnostics` | `false` | Report lines matching no known syntax — see below before enabling |
| `deprecatedSyntaxDiagnostics` | `true` | Warn on syntax upstream has deprecated |

To use a language server you built or installed yourself:

```json
{ "lsp": { "skript-lsp": { "binary": { "path": "/path/to/skript-lsp" } } } }
```

## Themes

No colour is named anywhere in this extension. Highlighting uses capture names
your theme already defines, and refinements fall back along their dot-prefixes,
so a minimal theme still gets sensible colours rather than none.

Tested against Catppuccin (Mocha and Latte), One Dark, Material Theme Darker and
the OLED themes. If something looks wrong in your theme, that is a bug worth
reporting. See [docs/theming.md](docs/theming.md) to override individual
captures.

## Three things that are not what you expect

**Ordinary statement prose is not coloured by the grammar.** Skript has no fixed
vocabulary — every effect, condition and expression is a pattern registered at
runtime by Skript or an addon, and nothing lexical distinguishes `set {_x} to 5`
from `player is op`. The grammar therefore colours only what is structurally
certain, and the language server classifies the rest via semantic tokens. This
is why step 2 of the install matters so much.

**"Unknown syntax" diagnostics are off by default, and should usually stay off.**
Any addon can register syntax this extension has never heard of, so reporting
every unmatched line would light up most scripts on a real server.

Setting `docsPath` does **not** make it safe, and an earlier version of this
README wrongly said it did. `/sk gen-docs` produces one `docs.json` per addon,
and the one Skript writes describes *Skript only* — no SkBee syntax, no
skript-reflect syntax, nothing from any addon you have installed. Every addon
line in your project would still be flagged. Turn it on only if your scripts use
core Skript and no addons, or if you have supplied every addon's syntax yourself
through `customSyntaxPaths`.

`docsPath` is still worth setting, for a different reason: it pins the catalog
to the exact Skript build your server runs, forks and nightlies included.

```json
{
  "lsp": {
    "skript-lsp": {
      "initialization_options": {
        "docsPath": "/path/to/plugins/Skript/docs/docs.json"
      }
    }
  }
}
```

**The syntax database is downloaded, never bundled.** Hover and completion are
driven by [Skript's own generated database](https://docs.skriptlang.org/docs.json)
plus [SkriptHub's addon catalog](https://skripthub.net/api/v1/addonsyntaxlist/).
Both are fetched at runtime and cached. That is partly licensing — they are
GPL-3.0 and no-licence respectively, and this project is MIT — and partly that
it keeps documentation matched to the Skript version you actually target.

## Without the language server

Everything structural keeps working, because it comes from the grammar rather
than the server:

| Works offline / without the server | Needs the server |
|---|---|
| Highlighting, indentation, folding | Hover, completion, signature help |
| Outline, snippets, bracket matching | Diagnostics, formatting, inlay hints |
| | Go to definition, references, rename |
| | Semantic token colouring |

On a first run with no network, a small built-in catalog takes over and the
server tells you so.

## Troubleshooting

**Everything is grey / only strings are coloured.** `semantic_tokens` is still
`"off"`. See step 2 of the install.

**No hover or completion.** The syntax database has not loaded. Check
**`dev: open language server logs`** — the server logs what it loaded, what it
detected, and what it fell back to.

**Nothing is highlighted at all.** The grammar failed to build. Zed's log
(`zed: open log`) will say why; a missing wasi-sdk or an unreachable grammar
revision are the usual causes.

**My addon's syntax is not recognised.** Check the log for what was detected. If
your addon is not on SkriptHub, point `customSyntaxPaths` at its syntax file.
[Open an issue](https://github.com/DaisyCatTs/SkriptZed/issues/new/choose) with
the addon name and version.

## How it works

Skript cannot be described by a context-free grammar, so this is deliberately
two layers. The tree-sitter grammar owns **structure** — lines, indentation,
sections, strings, variables — and never tries to guess what a statement means.
The language server owns **meaning**, matching each line against 2,660 published
Skript patterns plus 12,877 addon patterns, and returning the answer as LSP
semantic tokens.

The grammar is held to SkriptLang's own repository: roughly 540 real `.sk` files
must parse with zero errors on every change.

More in [docs/architecture.md](docs/architecture.md),
[docs/grammar.md](docs/grammar.md) and
[docs/language-server.md](docs/language-server.md).

## Contributing

Bug reports, grammar fixes and addon syntax are all welcome — see
[CONTRIBUTING.md](CONTRIBUTING.md).

The most useful bug report is a `.sk` snippet that Skript accepts and this
grammar does not.

## Licence

MIT — see [LICENSE](LICENSE). Third-party dependency notices are in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Skript itself is GPL-3.0 and is not included here; its syntax database is
downloaded at runtime rather than redistributed.
