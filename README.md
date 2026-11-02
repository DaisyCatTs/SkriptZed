# Skript for Zed

Write Minecraft server scripts in Zed, with the tooling Skript has never had.

[![CI](https://github.com/DaisyCatTs/SkriptZed/actions/workflows/ci.yml/badge.svg)](https://github.com/DaisyCatTs/SkriptZed/actions/workflows/ci.yml)
[![Zed Extension](https://img.shields.io/badge/Zed-extension-084CCF?logo=zedindustries&logoColor=white)](https://zed.dev/extensions?query=skript)
[![Skript 2.17](https://img.shields.io/badge/Skript-2.17-8B5CF6)](https://github.com/SkriptLang/Skript)
[![Licence: MIT](https://img.shields.io/badge/licence-MIT-green.svg)](LICENSE)

Full language support for [Skript](https://github.com/SkriptLang/Skript), the
scripting language Minecraft server owners use to write plugins without writing
Java.

Writing Skript has always meant writing it blind. No editor knew the language, so
you found your mistakes by uploading the file, watching the console, and doing it
again. This extension moves that loop into your editor.

- **Stop guessing at syntax.** Hover any line, or any expression inside one, for
  its documentation from Skript's own database: description, examples and the
  version it was added in.
- **Catch the mistakes before the upload.** Indentation Skript would reject,
  unclosed `###` blocks, duplicate declarations, calls to functions that do not
  exist or get the wrong number of arguments, `{@options}` nobody declared,
  `cancel event` where nothing can be cancelled, local variables that are never
  set, deprecated syntax. Optionally, check each saved script against the real
  Skript parser, or reload it on your own server as you save.
- **Navigate a real project.** Go to definition, find every reference, and rename
  across every file, including the ones you have not opened.
- **Completion that understands context.** Only events after `on `, only
  conditions inside `if `, only expressions inside `%…%`, and the variables your
  scripts already use inside `{`. Patterns insert with a tab stop per slot.
- **Your addons, not just core Skript.** It reads your server's `plugins/` folder
  and loads syntax for SkBee, skript-reflect, SkQuery and 170+ others, so addon
  syntax gets the same hover and completion as everything else. Addons that
  change the *shape* of a script are understood structurally: skript-reflect's
  `import:`, Java calls and custom syntax, ClassySK's classes, and oopsk's
  structs.
- **Looks right in your theme.** No colour is defined anywhere in this extension.
  It uses the capture names your theme already styles.
- **Only what you want.** Every editor feature has its own switch. Anything that
  talks to a Minecraft server or an AI tool is off until you turn it on.

Free and MIT licensed. Every change must parse all 587 scripts in SkriptLang's
own repository with zero errors.

<!-- SCREENSHOT: hero.png goes here. See docs/media/README.md for what to capture. -->

---

## Install

**1. Install the extension.** In Zed, open the command palette
(<kbd>Cmd/Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>), run **`zed: extensions`**,
search for **Skript**, and click **Install**. Open any `.sk` file and you are
done. The language server downloads itself the first time you open a Skript
file; there is nothing to install by hand.

**2. Turn on the editor features Zed leaves off. You want this.** Several of
the things the language server provides are switched off in Zed's defaults, so
they never show until you ask for them. Run **`zed: open settings`** and add:

```json
{
  "show_signature_help_after_edits": true,
  "languages": {
    "Skript": {
      "semantic_tokens": "combined",
      "document_folding_ranges": "on",
      "document_symbols": "on",
      "inlay_hints": { "enabled": true }
    }
  }
}
```

| Setting | Zed's default | What it turns on |
|---|---|---|
| `semantic_tokens` | `"off"` | Colour for Skript's effects, conditions and expressions, which is most of what you write. Without it a lot of your file stays grey |
| `document_folding_ranges` | `"off"` | The server's folding, with a placeholder saying what was folded. Otherwise Zed folds by indentation only |
| `document_symbols` | `"off"` | The server's outline, with command entries, subcommands, class members and struct fields nested in place |
| `inlay_hints.enabled` | `false` | Parameter names in front of call arguments, read from the declaration |
| `show_signature_help_after_edits` | `false` | Keeps argument hints open after you accept a completion |

<!-- SCREENSHOT: semantic-tokens.png (off vs combined) goes here. -->

On the bundled showcase file the grammar alone colours 93% of visible
characters; with semantic tokens it is 98%. Prefer `"combined"` over `"full"`:
`"full"` discards what the grammar already knows about strings and variables.

<details>
<summary>Optional: indentation and format-on-save</summary>

```json
{
  "languages": {
    "Skript": {
      "tab_size": 4,
      "hard_tabs": true,
      "formatter": "language_server",
      "format_on_save": "on"
    }
  }
}
```

`"formatter": "language_server"` matters if you have set a global `formatter`.
Zed applies that to every language, so a global chain ending in external
prettier means saving a `.sk` file runs
`prettier --stdin-filepath yourfile.sk`. Prettier has never heard of Skript, it
fails, and the Skript formatter is never asked. Naming the language server here
overrides that for Skript alone.

</details>

Not published to the registry yet? See
[installing from source](CONTRIBUTING.md#running-it-before-it-is-published).

## What you get

| | |
|---|---|
| **Highlighting** | Structures, sections, strings with `%interpolation%`, `<format tags>` and `&6` colour codes, variables by scope, options, commands, functions, literals, comments, ClassySK classes and oopsk structs |
| **Indentation** | Auto-indent after any `:` line, `else` snapping back to its `if`, and Skript's `#-#` marker for a colon that does *not* open a section |
| **Folding & outline** | Every event, command, subcommand, function, class, struct and section, with commands showing their entries nested underneath. skript-reflect custom syntax definitions are listed as their own kind |
| **Navigation** | Go to definition, find references and rename, project-wide, including files you have not opened. A `local function` stays in its file, and `{_x}` is confined to its trigger (event, command, function or ClassySK method) for references and rename. Go to definition also works from a use of your own skript-reflect syntax to its definition |
| **Hover** | Description, syntax, examples, event values, `since`, deprecation and addon requirements, from Skript's own database. Works on the expression under the cursor, not only the whole line, and on your own functions, variables, classes and skript-reflect syntax ("Defined in `file.sk`, line N") |
| **Completion** | Context-aware: only events after `on `, only conditions inside `if `, only expressions inside `%…%`. Inside `{`, the variables your scripts use, one list level at a time. `event-` offers the values of the event you are in, `past` and `future` included. Command entries include Skript 2.17's `prefix:`, `suggestions:` and subcommands, and addon sections offer their entries, required ones first (SkBee's `register shaped recipe:` → `id:`, `result:`, `shape:`). Inserts as snippets with a tab stop per slot |
| **Signature help** | Parameters as you type a call to your functions, Skript's built-ins, addon functions and ClassySK methods, following named arguments |
| **Inlay hints** | Parameter names shown at call sites, so `giveKit(p, 3)` is readable |
| **Call hierarchy** | Who calls a function, events and commands included: `call hierarchy: show incoming calls` (<kbd>Ctrl</kbd>+<kbd>K</kbd> <kbd>Ctrl</kbd>+<kbd>H</kbd>) |
| **Colour swatches** | `&6`, `&x&f&f&5&5&5&5`, `<red>` and `<#ff5555>` show their colour; picking a new one writes it back in the same style |
| **Links** | URLs in `<link:…>` tags, strings and comments are clickable |
| **Occurrence highlighting** | Every use of the symbol under your cursor, within the trigger that owns it |
| **Diagnostics** | Indentation Skript would reject, unclosed `###` blocks, duplicate declarations, calls to functions that do not exist, the wrong number of arguments to a function, undeclared `{@options}`, `cancel event` in an event that cannot be cancelled, local variables read but never set (or set but never read, shown faded), syntax from an addon you have not installed, deprecated syntax. Opt-in: the real Skript parser's errors through [SkAnalyzer](#checking-against-real-skript-skanalyzer), or your own server's through [RCON reload](#reloading-on-your-server-rcon) |
| **Quick fixes** | Correct a mistyped function name, create a missing one, add a missing option to `options:`, remove a `cancel event` that cannot work, fix the file's indentation, close an unterminated `###` block |
| **Formatting** | Re-indents from the parse tree, and refuses to touch a file that does not parse. Formats a selection too, widened to the whole top-level structures it touches. Puts one blank line under a multi-line structure that runs straight into the next |
| **Snippets** | 68, covering every structure, event, loop and common effect, ClassySK classes and methods, and Skript 2.17 subcommands and `suggestions:` |

## Addons

Most Skript projects run addons, so the server detects them: it finds your
`plugins/` directory, reads each JAR's manifest, and loads syntax for what is
installed. It reads `paper-plugin.yml` first and `plugin.yml` second, because
SkBee ships only the former.

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

Running [skript-minestom](https://github.com/skript-minestom/skript-minestom)?
Open its server folder (or its `Skript/scripts` folder) and the server is
recognised by itself: it loads that fork's own syntax and addons instead of
Bukkit Skript's, and says so in the language server log. Detection looks for
skript-minestom's layout (`Skript/` directly in the server folder, and
Minestom's own keys in `server.properties`). If it ever guesses wrong, set
`"platform"` to `"minestom"` or `"bukkit"`.

See [docs/addons.md](docs/addons.md), including what is *not* supported and why
unknown syntax is never treated as an error.

## Settings

All optional. Put them under `lsp.skript-lsp.initialization_options`, which the
language server reads when it starts, or under `lsp.skript-lsp.settings`, where
a change applies without a restart (a change to where syntax comes from reloads
the database). Use one place or the other: a `settings` block replaces the
startup options rather than adding to them. A setting with a misspelt name or a
wrong type is ignored on its own, and the server tells you which.

| Setting | Default | Meaning |
|---|---|---|
| `addons` | `"auto"` | `"auto"` detects from `plugins/`, `"off"` loads none, or a list of names such as `["SkBee", "skript-reflect"]` |
| `serverPath` | – | Where `plugins/` lives, if it is not at or above the workspace |
| `addonSyntaxSource` | `"skripthub"` | Where addon syntax comes from, or `"off"` |
| `customSyntaxPaths` | `[]` | Your own syntax files, in Skript's `docs.json` shape or SkriptHub's. A folder loads every `.json` in it |
| `platform` | auto | `"minestom"` or `"bukkit"`. Unset, a skript-minestom server is detected from its folder |
| `skriptVersion` | detected | The Skript version syntax is judged against, e.g. `"2.15.3"`. Your setting, else the installed `Skript.jar`, else the loaded database |
| `docsPath` | – | A `docs.json` generated on your server with `/sk gen-docs`, to match its exact Skript build |
| `docsUrl` | – | Fetch the database from a mirror |
| `unknownSyntaxDiagnostics` | `false` | Report lines matching no known syntax. Read [below](#three-things-that-are-not-what-you-expect) before enabling |
| `deprecatedSyntaxDiagnostics` | `true` | Warn on syntax upstream has deprecated |
| `localVariableDiagnostics` | `true` | Warn on a `{_local}` read but never set in its trigger, and fade one set but never read. The checks only fire when nothing in the trigger could explain the variable, but they are the ones most likely to be unwanted |
| `formatBlankLines` | `true` | Let the formatter put one blank line under a multi-line structure that runs straight into the next. It never removes a blank line |
| `analyzer.enabled` | on when `analyzer.jar` is set | Check each saved script with [SkAnalyzer](#checking-against-real-skript-skanalyzer). Set `false` to pause it without removing the jar path |
| `analyzer.jar` | – | Path to the SkAnalyzer shell jar. Setting it turns the analyzer on |
| `analyzer.java` | `java` on `PATH` | A Java 25 or newer executable |
| `analyzer.workDir` | `analyzer/` in the cache folder | A folder SkAnalyzer owns. Its `plugins/` must hold a Skratched Skript build |
| `analyzer.plugins` | the detected addon JARs | Addon JARs to load alongside Skript |
| `analyzer.timeoutMs` | `15000` | How long one check may take before the process is stopped |
| `rcon.enabled` | `false` | Reload each saved script on your server and show its errors. See [RCON reload](#reloading-on-your-server-rcon) |
| `rcon.host` | `"127.0.0.1"` | The server's address |
| `rcon.port` | `25575` | Its `rcon.port` from `server.properties` |
| `rcon.password` | – | Its `rcon.password`. Nothing is sent without one. Stored in plain text in your Zed settings |
| `rcon.timeoutMs` | `5000` | How long one reload (connect, log in, reload, read the reply) may take |
| `rcon.scriptsRoot` | found from `plugins/` or the file's path | The server's `plugins/Skript/scripts` folder, when it cannot be found |
| `features.*` | all `true` | One switch per editor feature; see below |

### Feature switches

Every feature the language server provides can be switched off on its own,
under `features`. All are on by default. A switched-off feature answers with
nothing rather than disappearing, so under `lsp.skript-lsp.settings` a change
applies at once. These are separate from Zed's own settings in
[install step 2](#install), which decide whether Zed asks for a feature at all.

```json
{
  "lsp": {
    "skript-lsp": {
      "initialization_options": {
        "features": { "inlayHints": false, "colors": false }
      }
    }
  }
}
```

| Switch | What it turns off |
|---|---|
| `hover` | Hover cards |
| `completion` | All completion |
| `variableCompletion` | Variable names inside `{`, while keeping the rest of completion |
| `signatureHelp` | Argument hints while typing a call |
| `inlayHints` | Parameter names at call sites |
| `semanticTokens` | The language server's colouring. The grammar's own highlighting stays |
| `colors` | Colour swatches and the colour picker |
| `links` | Clickable URLs |
| `folding` | The server's folding ranges (Zed then folds by indentation) |
| `formatting` | Formatting, of the whole file and of a selection |
| `codeActions` | Quick fixes |
| `rename` | Rename |
| `navigation` | Go to definition, find references and occurrence highlighting |
| `callHierarchy` | Incoming and outgoing calls |
| `symbols` | The outline and workspace symbol search |
| `diagnostics` | Every check the server makes itself. SkAnalyzer and RCON findings follow their own settings |

To use a language server you built or installed yourself:

```json
{ "lsp": { "skript-lsp": { "binary": { "path": "/path/to/skript-lsp" } } } }
```

<details>
<summary>Using the Discord presence extension? Skript needs one line</summary>

[zed-discord-presence](https://github.com/xhyrom/zed-discord-presence) builds its
icon URL from the language name, so it asks for a `skript.png` that its icon
repository does not have, and your Skript files show nothing. Point it at an
icon that exists, in **its** settings, not this extension's:

```json
{
  "lsp": {
    "discord_presence": {
      "initialization_options": {
        "languages": {
          "skript": {
            "large_image": "{base_icons_url}/zed.png",
            "large_text": "Skript"
          }
        }
      }
    }
  }
}
```

Swap `large_image` for any image URL you would rather use. This extension
deliberately does not touch Discord presence; that is another extension's job.

</details>

## Checking against real Skript (SkAnalyzer)

Everything above approximates Skript. The server matches each line against
published syntax, which cannot see type checks, section rules, or anything an
addon decides while parsing. [SkAnalyzer](https://github.com/SkEditorTeam/SkAnalyzer)
runs Skript's real parser outside a Minecraft server, so its errors are the ones
your server would print on `/sk reload`. With it enabled, each script is checked
when you save it, and the findings appear as diagnostics with the source
`SkAnalyzer`.

It is off by default, and nothing is downloaded for you. You need:

1. **Java 25 or newer.**
2. **The SkAnalyzer shell jar** (`skanalyzer-shell-bundler-*.jar`) from
   [SkAnalyzer's releases](https://github.com/SkEditorTeam/SkAnalyzer/releases).
3. **A Skratched Skript build** (`Skript-v2.16-skratched.jar` or an older one)
   from [Skratches' releases](https://github.com/SkEditorTeam/Skratches/releases),
   placed in the `plugins/` folder of the analyzer's work directory. By default
   that is `skript-lsp/analyzer/plugins/` inside your cache folder
   (`%LOCALAPPDATA%`, `~/Library/Caches` or `~/.cache`).

```json
{
  "lsp": {
    "skript-lsp": {
      "initialization_options": {
        "analyzer": {
          "jar": "/path/to/skanalyzer-shell-bundler-2.0.0-beta.7.jar"
        }
      }
    }
  }
}
```

Setting `jar` is what turns it on. Add `"enabled": false` to pause it without
losing the path.

SkAnalyzer is LGPL-3.0 and Skratches is GPL-3.0. You install both yourself, and
the language server only starts them as a separate process; neither is bundled
or linked.

Limits worth knowing before you rely on it:

* **Skratches currently stops at Skript 2.16.** Syntax added in 2.17, such as
  subcommands, is unknown to it and can be reported as an error even though
  your server accepts it.
* **Addons that ship only a `paper-plugin.yml` do not load**, which includes
  current SkBee. Lines using their syntax are reported as errors. Leave such
  addons out of `analyzer.plugins`, or accept those findings as noise.
* It runs on save, not as you type: the analyzer reads the file from disk.

## Reloading on your server (RCON)

If you edit the scripts your test server actually runs, the language server can
reload each one as you save it and show what Skript says about it. On save it
sends `skript reload <script>` over RCON, and the errors and warnings in
Skript's reply appear on their lines as diagnostics with the source
`Skript (server)`. It is off by default, because it connects to a server and
changes what that server is running.

This only makes sense when the server reads the files you edit: a server on
your own machine, or one whose `scripts/` folder you have mounted. A file
outside the server's scripts folder is never sent.

What you need:

1. **RCON on in `server.properties`:** `enable-rcon=true`, an `rcon.port`, and
   an `rcon.password`. Restart the server after changing them.
2. **Skript's default `script loader thread size: 0`** in
   `plugins/Skript/config.sk`. With a value above 0, Skript loads scripts in the
   background and its errors arrive after the RCON reply has gone. The reload
   still happens, but no errors are shown and the language server's log says
   the result is unknown.

```json
{
  "lsp": {
    "skript-lsp": {
      "initialization_options": {
        "rcon": {
          "enabled": true,
          "port": 25575,
          "password": "the rcon.password from server.properties"
        }
      }
    }
  }
}
```

**The password sits in your Zed settings in plain text,** and RCON itself sends
it unencrypted. Use a password you use nowhere else, keep RCON to a server on
your own machine or local network, and never open the RCON port to the
internet. The language server never logs the password.

Details worth knowing:

* Only scripts under the server's `plugins/Skript/scripts` are reloaded. The
  folder is found from the detected `plugins/` or from the saved file's own
  path; set `rcon.scriptsRoot` if neither works. Scripts disabled with a `-`
  prefix are skipped, as Skript itself skips them.
* If the server cannot be reached or rejects the password, you are told once.
  After that it stays quiet (saving still works) until you change the `rcon`
  settings.
* The server's verdict stays on the file until the next save, the way a build
  result does, and clears when you close the tab.

## AI helper (optional)

The language server can also answer questions from an AI agent: Zed's Agent
Panel, or any other tool that speaks the Model Context Protocol. It is off and
nothing in this extension starts it. The extension does not register it with
Zed, so if you do not use AI features you will never see it. You turn it on
yourself by adding it to your Zed settings.

It gives the agent four read-only tools, so it can look Skript up instead of
guessing at syntax:

| Tool | What it answers |
|---|---|
| `search_syntax` | Syntax that matches some words, from Skript and your installed addons |
| `get_docs` | One entry's full documentation |
| `classify` | What each line of a piece of Skript resolves to |
| `project_symbols` | The functions, commands, options, classes and structs your project declares |

None of them writes a file or contacts a Minecraft server. The only network use
is the same syntax download the language server makes, into the same cache.

To turn it on, add this to your Zed settings, with the path to your
`skript-lsp` binary:

```json
{
  "context_servers": {
    "skript-knowledge": {
      "command": "/path/to/skript-lsp",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

* **Which binary.** The one the extension downloaded is listed in Zed's log
  (see [Which version am I running?](#troubleshooting)). Its folder name
  changes with every release, and the old folder is removed, so after an update
  the path needs changing. Pointing at a copy you keep elsewhere avoids that.
  It needs a language server release that includes `mcp` (the first one after
  0.2.5); if yours does, `skript-lsp mcp --help` prints its usage.
* **Which project.** Zed starts it in the project's root folder, which is where
  it reads scripts and looks for `plugins/`. Add `"--project", "/path/to/project"`
  to `args` to choose another, and `"--settings", "{\"addons\": [\"SkBee\"]}"`
  to pass the same settings the language server takes.
* Zed starts no context servers at all while `disable_ai` is `true`.

## Three things that are not what you expect

**Ordinary statement prose is not coloured by the grammar.** Skript has no fixed
vocabulary. Every effect, condition and expression is a pattern registered at
runtime by Skript or an addon, and nothing lexical distinguishes `set {_x} to 5`
from `player is op`. The grammar therefore colours only what is structurally
certain, and the language server classifies the rest via semantic tokens. This
is why step 2 of the install matters so much.

**"Unknown syntax" diagnostics are off by default, and should usually stay off.**
Any addon can register syntax this extension has never heard of, so reporting
every unmatched line would light up most scripts on a real server.

Setting `docsPath` does **not** make it safe. The `docs.json` that `/sk gen-docs`
writes for Skript describes *Skript only*: no SkBee syntax, no skript-reflect
syntax, nothing from any addon you have installed. Every addon line in your
project would still be flagged. Turn it on only if your scripts use no addons,
or you supply every addon's syntax through `customSyntaxPaths`.

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
plus [SkriptHub's addon catalog](https://skripthub.net/api/v1/addonsyntaxlist/),
or, with `"platform": "minestom"`, by the databases on
[skript-minestom's docs site](https://smdocs.hapily.me). All are fetched at
runtime and cached. That is partly licensing (Skript's is GPL-3.0, SkriptHub's
states no licence, and the skript-minestom data's licence is unverified, while
this project is MIT) and partly that it keeps documentation matched to the
Skript version you target.

## Without the language server

Everything structural keeps working, because it comes from the grammar rather
than the server:

| Works offline / without the server | Needs the server |
|---|---|
| Highlighting, indentation, folding | Hover, completion, signature help |
| Outline, snippets, bracket matching | Diagnostics, formatting, inlay hints |
| | Go to definition, references, rename |
| | Semantic token colouring, colour swatches, links |

On a first run with no network, a small built-in catalog takes over and the
server tells you so. Behind a proxy, the server's downloads follow the usual
`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` environment variables.

## Troubleshooting

**Everything is grey / only strings are coloured.** `semantic_tokens` is still
`"off"`. See step 2 of the install.

**No inlay hints, flat outline, or folding without placeholders.** Zed's
defaults switch those server features off. See step 2 of the install.

**No hover or completion.** The syntax database has not loaded. Check
**`dev: open language server logs`**: the server logs what it loaded, what it
detected, and what it fell back to.

**Nothing is highlighted at all.** The grammar failed to build. Zed's log
(`zed: open log`) will say why; a missing wasi-sdk or an unreachable grammar
revision are the usual causes.

**Which version am I running?** Two separate things, so check both:

* **The extension**: its version is in the Extensions panel.
* **The language server**: Zed's log (`zed: open log`) records the binary it
  started, and the folder name is the release tag:
  `…/extensions/work/skript/skript-lsp-v0.2.0/skript-lsp`. You can also run that
  binary with `--version`.

The server's update check is cached for a day, so a release published in the
last 24 hours may not have been picked up yet. Quit Zed, delete the
`extensions/work/skript` folder, and reopen to force a fresh download.

**My addon's syntax is not recognised.** Check the log for what was detected. If
your addon is not on SkriptHub, point `customSyntaxPaths` at its syntax file.
[Open an issue](https://github.com/DaisyCatTs/SkriptZed/issues/new/choose) with
the addon name and version.

**SkAnalyzer reports nothing.** It only runs on save, and setup problems
(missing jar, Java older than 25, no Skratched build in `plugins/`) are shown
once as a message. The analyzer then stays off until you change its settings or
restart the language server.

**Saving does not reload the script on my server.** Check
**`dev: open language server logs`**. Each save logs what happened: reloaded,
the error count, why a file was skipped (outside the scripts folder, or a `-`
script), or that the reply did not say (see the `script loader thread size`
requirement under [RCON reload](#reloading-on-your-server-rcon)). A connection
or password problem is shown once and then only logged.

**A feature I want is missing.** Check that nothing under `features` switches
it off, and that the Zed setting for it in install step 2 is on.

## How it works

Skript cannot be described by a context-free grammar, so this is deliberately
two layers. The tree-sitter grammar owns **structure** (lines, indentation,
sections, strings, variables) and never tries to guess what a statement means.
The language server owns **meaning**, matching each line against Skript's 2,703
published patterns plus 13,661 addon patterns, and returning the answer as LSP
semantic tokens.

More in [docs/architecture.md](docs/architecture.md),
[docs/grammar.md](docs/grammar.md) and
[docs/language-server.md](docs/language-server.md).

## Contributing

Bug reports, grammar fixes and addon syntax are all welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md).

The most useful bug report is a `.sk` snippet that Skript accepts and this
grammar does not.

## Licence

MIT. See [LICENSE](LICENSE). Third-party dependency notices are in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Skript itself is GPL-3.0 and is not included here; its syntax database is
downloaded at runtime rather than redistributed. SkAnalyzer and Skratches are
never included either; you install them yourself if you want them.
