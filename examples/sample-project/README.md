# Sample project

Open this folder in Zed to see everything the extension does, in one place.

```
showcase.sk   every construct the grammar handles
library.sk    a second file, so cross-file navigation has somewhere to go
addons.sk     syntax from SkBee and skript-reflect, plus a deliberately unknown addon
reflect.sk    skript-reflect's Java interop and custom syntax in a realistic script
classysk.sk   a ClassySK class: fields, methods, self:: and new instance
equipment.sk  named arguments, a ClassySK static method and SkBee's switch/case
features.sk   a guided walkthrough of everything new since 0.2.5
TESTING.md    a numbered checklist for testing all of it by hand
```

Turn on the editor settings from the main README's
[install step 2](../../README.md#install) first: semantic tokens, folding
ranges, document symbols, inlay hints and argument hints after edits are all
off in Zed's defaults, and half of what follows will not show without them.

`equipment.sk`, `addons.sk` and the oopsk part of `features.sk` also need their
addons detected; see [Seeing the addon syntax work](#seeing-the-addon-syntax-work).

## What to try

**New since 0.2.5.** Open `features.sk` and work down it. Each numbered block
says what to do and what you should see:

1. Variable completion: `{_` offers the locals of the current trigger only;
   `{` offers every global in the project; `{data::` completes one level at a
   time, `*` included.
2. Colour swatches on `&6`, `&x&f&f&5&5&5&5`, `<red>` and `<#55ffff>`; picking
   a new colour writes it back in the same style.
3. Clickable links in `<link:…>` tags, bare URLs and comments.
4. The one deliberate error: an unknown option, with a quick fix that adds it
   to `options:`.
5. Argument hints that show defaults whole and follow named arguments.
6. Hover on a variable, on an expression inside a line (`{_p}'s helmet` is
   Armor Slot, not Set), on the built-in `floor()` and on your own functions.
7. Skript 2.17 `subcommand` lines, in the outline and folding.
8. An oopsk `struct` template with a `const` field and `converts to:`.

`classysk.sk` and `equipment.sk` cover the ClassySK work: outline, folding,
hover and go-to-definition on methods and fields, and argument colouring.

The custom syntax section of `reflect.sk` defines an effect, an expression, a
condition and a local effect, and `command /custom:` uses each one: hover a use
to see where it is defined, and <kbd>F12</kbd> to jump there.

The newer checks (wrong argument counts, `cancel event` in an event that cannot
be cancelled, local variables never set or never read), formatting a selection,
`event-` completion, addon section entries, the feature switches and the
optional RCON reload and AI helper are walked through step by step in
[TESTING.md](TESTING.md).

**Colour.** Open `showcase.sk` and switch themes: One Dark, Catppuccin,
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

- `on ` then a letter: only events
- inside `command /home:`: its entries (`description:`, `permission:`,
  `cooldown:`, `trigger:`), not effects, because effects cannot go there
- inside `trigger:`: effects and your own functions, not entries
- `{@`: the options declared at the top of the file

Press <kbd>Ctrl</kbd>+<kbd>Space</kbd> to ask for the list at any time.

**Navigation.** <kbd>F12</kbd> on `greet(...)` jumps to its declaration.
<kbd>F12</kbd> on `format_points(...)` jumps into `library.sk`, which you never
opened. <kbd>Shift</kbd>+<kbd>F12</kbd> lists every use. <kbd>F2</kbd> renames a
function or a `{score::*}` variable across both files; the braces and the `::*`
survive.

**Inlay hints.** `add_points(player, 5)` should show `who:` and `amount:` in
front of its arguments, read from the declaration.

**Diagnostics.** Indent one line with spaces where the rest of the file uses
tabs, or call a function that does not exist, or call `add_points()` with no
arguments. All are reported. Delete the
closing `###` of the block comment at the top and the whole file is flagged.

**Outline.** <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>O</kbd>: commands contain
their entries, events contain their sections.

**Folding.** Fold an event, a command, and the `###` block. Each collapses to a
placeholder saying what it was.

**Addons.** `addons.sk` holds SkBee and skript-reflect syntax. With those addons
absent, those lines are plain, never errors. The last event in that file is from
an addon that does not exist anywhere. It stays silent on purpose: with 13,661
published addon patterns, guessing would mean a wall of false errors on every
real server.

## Measuring it

```sh
node scripts/coverage.mjs examples/sample-project/showcase.sk
```

Reports how much of a file the grammar and the language server each explain, and
lists any line neither could. On `showcase.sk` the language server should
classify every executable line.

## Seeing the addon syntax work

`addons.sk`, `equipment.sk` and the oopsk block in `features.sk` stay plain
unless the server can see their addons installed. That silence is deliberate,
not a bug. Point `serverPath` at a real server directory:

```json
{ "lsp": { "skript-lsp": { "initialization_options": {
  "serverPath": "/srv/minecraft/my-server"
} } } }
```

If you do not run a server with all of them, generate a throwaway one:

```sh
node scripts/make-test-server.mjs
```

It writes stub JARs carrying only a manifest, which is all the detector reads,
into `examples/sample-project/test-server`, and prints the `serverPath` to paste
into this folder's `.zed/settings.json` (it must be absolute). Restart the
language server and every line in `addons.sk` hovers, colours and completes. The
generated directory is gitignored and rebuilt in a second.
