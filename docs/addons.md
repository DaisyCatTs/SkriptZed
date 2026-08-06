# Addons

Most real Skript projects run addons. SkBee alone publishes **664 syntax
elements** — more than a quarter of core Skript's own. Without addon support, a
line of SkBee gets no hover, no completion and no classification.

## How it works

```
plugins/*.jar  ──read paper-plugin.yml / plugin.yml──>  detected addons
                                                              │
docs.skriptlang.org/docs.json ─┐                              │
skripthub.net addonsyntaxlist ─┼── filtered to those addons ──┴──> one catalog
your own JSON files ───────────┘        merged, deduped by id
```

Three things follow from that shape:

**Only detected addons are loaded.** SkriptHub covers 168 addons and 12,877
patterns. Loading all of them would fill completion with syntax for plugins you
do not run and make "unknown syntax" meaningless. A realistic project — SkBee
plus skript-reflect — costs about 1,055 patterns instead.

**Detection reads the manifest inside the JAR, never the filename.** Real
filenames look like `EssentialsX-2.22.0 (1).jar` (a browser's duplicate suffix)
and `LuckPerms-Bukkit-5.5.50.jar` (a platform infix). The manifest is
authoritative; filename parsing is guesswork.

**`paper-plugin.yml` is tried first.** SkBee — the most popular Skript addon
there is — ships *no* `plugin.yml` at all, only Paper's newer descriptor with a
different dependency schema. A `plugin.yml`-only reader silently misses it.

## Setting it up

Usually nothing. If your scripts live under `plugins/Skript/scripts/`, the
server walks up, finds `plugins/`, and reads what is there.

When the scripts are somewhere else — a git repo separate from the server —
point it at the server:

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

`"addons": "off"` loads none.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `addons` | `"auto"` | `"auto"` detect · `"off"` · or a list of names |
| `serverPath` | – | Where `plugins/` lives |
| `addonSyntaxSource` | `"skripthub"` | `"skripthub"` or `"off"` |
| `customSyntaxPaths` | `[]` | Extra syntax files, in either schema |
| `skriptVersion` | detected | Target Skript version |

## Unknown and private addons

An addon nobody has documented is never an error. A line that matches nothing
stays unclassified and silent — the language server does not assume a mistake,
because any addon can register syntax it has never heard of.

For a private or unpublished addon, hand it the syntax directly:

```json
{ "customSyntaxPaths": ["./my-addon-syntax.json"] }
```

The file may be in **either** shape the ecosystem already uses — Skript's own
`docs.json` object, or a SkriptHub-style array of records. The shape is sniffed,
so there is no third format to learn.

## The `requires-addon` diagnostic

When a line matches an addon you do *not* have installed, the server says so
by name:

> This is `SkBee` syntax, and `SkBee` 3.6.0 or newer is not installed on this
> server.

This only ever appears when a `plugins/` directory was actually found. If the
server does not know your environment it says nothing — claiming an addon is
missing when we cannot see the server would be a guess, and a noisy one.

## Version awareness

Hover and completion label syntax your target Skript cannot run:

> ⚠️ needs Skript 2.14+

It is **labelled and sorted down, never hidden.** Skript's `since` field is free
text — only 75% of values are plain version numbers, the rest look like
`"2.5.2, 2.9.0 (enforce, offline players)"`, `"2.2-dev36"` or
`"unknown (before 2.1)"`. Hiding syntax that actually works, because a note
failed to parse, is a worse failure than an unnecessary label. The earliest
version mentioned is taken as the minimum, which resolves **98.1%** of entries.

The target version comes from `skriptVersion`, else the version in your
installed `Skript.jar`, else the database that was loaded.

Two quirks worth knowing, both handled: `2.2-dev28` is a build *before* 2.2,
while `2.2-Fixes-V10` is a fork release *after* it — ordinary semver ordering
gets the second wrong.

## What is not supported

**`Skellett` and `SkStuff` have no syntax data anywhere.** Neither appears in
SkriptHub's catalog; both have been dead for years. They will behave like any
other unknown addon — silent, never an error — and `customSyntaxPaths` is the
way in if you still run them.

**Experiment gating is not automatic.** Skript's `using <experiment>` features
(`for loop`, `queues`, `type hints`, …) are not marked in `docs.json` at all —
there is no field for it, and only three of the queue syntaxes mention it in
free text. So the server cannot tell you that a line needs an experimental flag.

**Removal is not tracked.** `docs.json` records that something is deprecated but
never when, and SkriptHub's `mark_as_removed` field is unpopulated across all
8,210 records. Detecting removal would mean diffing archives.

## Where the data comes from

| Source | Covers | Notes |
|---|---|---|
| [`docs.skriptlang.org/docs.json`](https://docs.skriptlang.org/docs.json) | Core Skript | Official, generated from source. GPL-3.0 |
| [SkriptHub `addonsyntaxlist`](https://skripthub.net/api/v1/addonsyntaxlist/) | 168 addons | Keyless. No per-addon endpoint or pagination — the whole 7.3 MB (1.2 MB gzipped) is fetched and filtered locally |
| Your own files | Anything | Either schema |

Neither database is bundled. Both are fetched at runtime and cached for 24
hours, which keeps this project MIT and keeps the syntax matched to the Skript
you actually run. SkriptHub's data is community-contributed with no stated
licence, which is a second reason not to redistribute it.

**`/sk gen-docs` does not produce addon syntax.** Skript generates its
documentation with `JSONGenerator.of(Skript.instance())` — scoped to one addon,
named at the root by `source.name`. Installing SkBee does not add SkBee syntax
to `docs.json`. It is still worth setting `docsPath`, but for pinning the exact
Skript build, not for addons.
