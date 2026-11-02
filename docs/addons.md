# Addons

Most real Skript projects run addons. SkBee alone publishes **663 syntax
elements**, more than half as many as core Skript's 1,235. Without addon
support, a line of SkBee gets no hover, no completion and no classification.

## How it works

```
plugins/*.jar  ──read paper-plugin.yml / plugin.yml──>  detected addons
                                                              │
docs.skriptlang.org/docs.json ─┐                              │
skripthub.net addonsyntaxlist ─┼── filtered to those addons ──┴──> one catalog
your own JSON files ───────────┘        merged, deduped by id
```

Three things follow from that shape.

**Only detected addons are loaded.** SkriptHub covers 174 addons and 13,661
patterns. Loading all of them would fill completion with syntax for plugins you
do not run and make "unknown syntax" meaningless. A realistic project, SkBee
plus skript-reflect, costs about 1,050 patterns instead.

**Detection reads the manifest inside the JAR, never the filename.** Real
filenames look like `EssentialsX-2.22.0 (1).jar` (a browser's duplicate suffix)
and `LuckPerms-Bukkit-5.5.50.jar` (a platform infix). The manifest is
authoritative; filename parsing is guesswork.

**`paper-plugin.yml` is tried first.** SkBee, the most popular Skript addon,
ships *no* `plugin.yml` at all, only Paper's newer descriptor with a different
dependency schema. A `plugin.yml`-only reader silently misses it.

## Setting it up

Usually nothing. If your scripts live under `plugins/Skript/scripts/`, the
server walks up, finds `plugins/`, and reads what is there.

When the scripts are somewhere else, such as a git repository separate from the
server, point it at the server:

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

Or skip detection and say what you use:

```json
{ "addons": ["SkBee", "skript-reflect"] }
```

`"addons": "off"` loads none. Every addon-related setting (`addons`,
`serverPath`, `addonSyntaxSource`, `customSyntaxPaths`, `platform`,
`skriptVersion`) is described in the
[README's settings table](../README.md#settings).

## Unknown and private addons

An addon nobody has documented is never an error. A line that matches nothing
stays unclassified and silent. The language server does not assume a mistake,
because any addon can register syntax it has never heard of.

For a private or unpublished addon, hand it the syntax directly:

```json
{ "customSyntaxPaths": ["./my-addon-syntax.json"] }
```

The file may be in **either** shape the ecosystem already uses: Skript's own
`docs.json` object, or a SkriptHub-style array of records. The shape is sniffed,
so there is no third format to learn. A folder in the list loads every `.json`
directly inside it, which suits a server's folder of generated docs.

## The `requires-addon` diagnostic

When a line matches an addon you do *not* have installed, the server says so
by name:

> This is `SkBee` syntax, and `SkBee` 3.6.0 or newer is not installed on this
> server.

This only appears when a `plugins/` directory was found. If the server does not
know your environment it says nothing: claiming an addon is missing when it
cannot see the server would be a guess, and a noisy one.

## Version awareness

Hover and completion label syntax your target Skript cannot run:

> needs Skript 2.14+

It is **labelled and sorted down, never hidden.** Skript's `since` field is free
text. Only 75% of values are plain version numbers; the rest look like
`"2.5.2, 2.9.0 (enforce, offline players)"`, `"2.2-dev36"` or
`"unknown (before 2.1)"`. Hiding syntax that works because a note failed to
parse is a worse failure than an unnecessary label. The earliest version
mentioned is taken as the minimum, which resolves **98.1%** of entries.

The target version is your `skriptVersion` setting, else the version in your
installed `Skript.jar`, else the database that was loaded.

Two quirks, both handled: `2.2-dev28` is a build *before* 2.2, while
`2.2-Fixes-V10` is a fork release *after* it. Ordinary semver ordering gets the
second wrong.

## skript-minestom

[skript-minestom](https://github.com/skript-minestom/skript-minestom) is a
fork of Skript for Minestom servers, with its own syntax and its own addons.
Its server folder is recognised by itself: Skript's data folder sits directly
in it (`<server>/Skript/scripts`, not under `plugins/`), and Minestom writes
keys into `server.properties` that Bukkit never uses (`auth-type`,
`dispatcher-threads`). Both have to be present, and a `bukkit.yml`,
`spigot.yml` or `paper.yml` beside them rules it out. When the guess is wrong,
or the scripts live elsewhere, say so:

```json
{ "platform": "minestom", "addons": ["oopsk"] }
```

(`"bukkit"` turns detection off the other way.) The server then loads skript-minestom's database and every addon published on
[its docs site](https://smdocs.hapily.me) (skript-gui, SkCheese, skript-combat,
skript-nbs and the rest) instead of Bukkit Skript's. Those are fetched at
runtime and cached, never bundled. `addons` still pulls the Bukkit addons
skript-minestom supports from SkriptHub (oopsk; its own skript-reflect build
comes from the docs site).

The docs site states no licence for its data, and this project has not been
able to verify one. That is one more reason it is fetched rather than
redistributed.

## Structural addons

Most addons only add patterns, and the catalog covers them. A few change the
*shape* of a script, and the grammar understands those directly:

| Addon | What the grammar knows |
|---|---|
| skript-reflect | `import:` blocks, `new Class()` constructors, and Java calls such as `Bukkit.getVersion` and `{_item}.getItemMeta()` |
| ClassySK | `class Name:` with fields, methods and modifiers; `self::member` and `{_x}::method()` access; `new instance of Name` |
| oopsk | `struct Name:` templates and their fields |

ClassySK methods are hovered, navigated and argument-hinted like functions.
They are not renamed: which class `{_x}::reset()` calls is only known at
runtime, so renaming one class's `reset` could silently rewrite another's.

### skript-reflect custom syntax

skript-reflect lets a script define its own syntax:

```skript
effect greet everyone:
    trigger:
        loop all players:
            send "Hello" to loop-player

local expression [the] coins of %player%:
    get:
        return {coins::%expr-1's uuid%}
```

The language server reads every `effect`, `condition`, `expression`, property
and custom `event` definition in the project, `local` ones included, and adds
their patterns to the catalog. A use such as `greet everyone` is then coloured,
completed and hovered like core syntax (the hover says "Defined in `file.sk`,
line N"), and go-to-definition jumps to the definition. A `local` definition
counts only in its own file. The outline lists definitions as their own kind
rather than as events. Edits apply as you type, with no save needed.

## Addon section entries

Some addon sections take entries rather than code, such as SkBee's
`register shaped recipe:` with its `id:`, `result:` and `shape:`. SkriptHub
records these for 87 sections and structures, so inside one, completion offers
its entries, with the required ones you have not written yet first, and hover
explains each. SkriptHub records only an entry's name, whether it is required,
whether it opens a section and its default, so that is all the hover can say.

## Checking with the real Skript

Everything above matches lines against published syntax. To have Skript's own
parser check a saved script, with your addons loaded, enable SkAnalyzer; see
[the README](../README.md#checking-against-real-skript-skanalyzer). By default it
loads the addon JARs detected in your `plugins/`.

## What is not supported

**`Skellett` and `SkStuff` have no syntax data anywhere.** Neither appears in
SkriptHub's catalog; both have been dead for years. They behave like any other
unknown addon (silent, never an error), and `customSyntaxPaths` is the way in
if you still run them.

**Experiment gating is not automatic.** Skript's `using <experiment>` features
(`for loop`, `queues`, `type hints`, …) are listed in `docs.json`, but no syntax
entry says which experiment it needs, and only three of the queue syntaxes
mention it in free text. So the server cannot tell you that a line needs an
experimental flag.

**Removal is not tracked.** `docs.json` records that something is deprecated but
never when, and SkriptHub's `mark_as_removed` field is unpopulated across all
8,712 records. Detecting removal would mean diffing archives.

## Where the data comes from

| Source | Covers | Notes |
|---|---|---|
| [`docs.skriptlang.org/docs.json`](https://docs.skriptlang.org/docs.json) | Core Skript | Official, generated from source. GPL-3.0 |
| [SkriptHub `addonsyntaxlist`](https://skripthub.net/api/v1/addonsyntaxlist/) | 174 addons | Keyless. No per-addon endpoint or pagination, so the whole catalog (about 8 MB) is fetched and filtered locally |
| [smdocs.hapily.me](https://smdocs.hapily.me) | skript-minestom and its addons | Only with `"platform": "minestom"`. Licence unverified |
| Your own files | Anything | Either schema |

None of these is bundled. All are fetched at runtime and cached for 24 hours,
which keeps this project MIT and keeps the syntax matched to the Skript you run.
SkriptHub's data is community-contributed with no stated licence, which is a
second reason not to redistribute it.

**`/sk gen-docs` does not produce addon syntax in Skript's file.** Skript
generates its documentation with `JSONGenerator.of(Skript.instance())`, scoped to
one addon and named at the root by `source.name`. Installing SkBee does not add
SkBee syntax to that `docs.json`. It is still worth setting `docsPath`, but for
pinning the exact Skript build, not for addons.
