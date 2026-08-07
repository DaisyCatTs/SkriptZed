# Sample project

Open this folder in Zed to see everything the extension does, in one place.

```
showcase.sk   every construct the grammar handles
library.sk    a second file, so cross-file navigation has somewhere to go
addons.sk     syntax from SkBee and skript-reflect, plus a deliberately unknown addon
```

Make sure semantic tokens are on first, or half of what follows will not show:

```json
{ "languages": { "Skript": { "semantic_tokens": "combined" } } }
```

## What to try

**Colour.** Open `showcase.sk` and switch themes — One Dark, Catppuccin,
Material Theme Darker. No colour is defined anywhere in this extension, so
everything should follow the theme. Anything that renders unstyled is a bug.

**Hover.** Put the cursor on:

| Line | You should see |
|---|---|
| `on first join:` | *On First Join*, with its description and examples |
| `wait 3 seconds` | *Delay* |
| `cooldown: 3 seconds` | the command entry, explaining what a cooldown does |
| `set {_x} to 5` | *Change: Set/Add/Remove* |

**Completion.** Type on a blank line:

- `on ` then a letter — only events
- inside `command /home:` — its entries (`description:`, `permission:`,
  `cooldown:`, `trigger:`), not effects, because effects cannot go there
- inside `trigger:` — effects and your own functions, not entries
- `{@` — the options declared at the top of the file

Press <kbd>Ctrl</kbd>+<kbd>Space</kbd> to ask for the list at any time.

**Navigation.** <kbd>F12</kbd> on `greet(...)` jumps to its declaration.
<kbd>F12</kbd> on `format_points(...)` jumps into `library.sk`, which you never
opened. <kbd>Shift</kbd>+<kbd>F12</kbd> lists every use. <kbd>F2</kbd> renames a
function or a `{score::*}` variable across both files — the braces and the `::*`
survive.

**Inlay hints.** `add_points(player, 5)` should show `who:` and `amount:` in
front of its arguments, read from the declaration.

**Diagnostics.** Indent one line with spaces where the rest of the file uses
tabs, or call a function that does not exist. Both are reported. Delete the
closing `###` of the block comment at the top and the whole file is flagged.

**Outline.** <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd> — commands contain
their entries, events contain their sections.

**Folding.** Fold an event, a command, and the `###` block. Each collapses to a
placeholder saying what it was.

**Addons.** `addons.sk` holds SkBee and skript-reflect syntax. With those addons
absent, those lines are simply plain — never errors. Point the server at a real
server directory to see them light up:

```json
{ "lsp": { "skript-lsp": { "initialization_options": {
  "serverPath": "/srv/minecraft/my-server"
} } } }
```

The last event in that file is from an addon that does not exist anywhere. It
stays silent on purpose: with 12,877 published addon patterns, guessing would
mean a wall of false errors on every real server.

## Measuring it

```sh
node scripts/coverage.mjs examples/sample-project/showcase.sk
```

Reports how much of a file the grammar and the language server each explain, and
lists any line neither could. On `showcase.sk` both numbers should be 100%.
