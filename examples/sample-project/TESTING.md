# Manual test checklist

Everything in the changelog's *Unreleased* section, tested by hand in Zed.
Each step says what to open, what to do, and what you should see. If what you
see differs, that step failed; note the step number.

## Before you start

1. Build the language server: `cd language-server && cargo build -p skript-lsp`
   (or `--release`), and point Zed at it in your user settings:
   `{ "lsp": { "skript-lsp": { "binary": { "path": "/abs/path/to/skript-lsp" } } } }`.
2. Run `node scripts/dev-setup.mjs`, then in Zed: command palette →
   **zed: install dev extension** → pick the `extension/` directory. Do this
   again after any grammar change.
3. Add to your Zed user settings:
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
   Zed ships every one of these off. Without them statement colouring, the
   server's outline and folding, inlay hints, and argument hints after a
   completion do not show. The README's install step 2 explains each.
4. Run `node scripts/make-test-server.mjs` from the repo root. Check that
   `examples/sample-project/.zed/settings.json` sets `serverPath` to the
   absolute path it prints. This is what makes SkBee, ClassySK, oopsk and the
   other addons count as installed.
5. Open the `examples/sample-project` **folder** in Zed (not a single file).
   Open `zed: open log` and confirm the server reports 9 Skript addons,
   oopsk among them, and Skript 2.17.1.

## features.sk: the walkthrough

Open `features.sk`. Where a step says "the marked line", it means the line
ending in `# <- press Enter here` in that block: put the cursor at the end of
that line, press Enter, and type. Undo afterwards.

6. **Diagnostics baseline.** Open the diagnostics panel. Exactly one error:
   *No option named `farewel`*, in block 4. Nothing else in the file.
7. **Local variables (block 1).** On the marked line in `giveCoins`, type `{_`
   (Zed closes the brace for you). Offered: `_balance`, `_p`, `_amount`,
   nothing else. `_visitor`, `_hat` and the other locals in the file must not
   appear.
8. **List variables (block 1).** On the marked line in `on join:`, type
   `{data::`. Offered: `settings`, `%player's uuid%`, `*`. Accept `settings`:
   the list reopens with `motd`, `volume`, `*`.
9. **Globals.** On the same line, type just `{`. Offered: every global in the
   project, including `coins::`, `stats::` and `homes::` from other files.
10. **Colour swatches (block 2 and `options:`).** A swatch sits on each colour
    code: `&8`, `&6`, `&7` in the options, `&6`, `&x&f&f&5&5&5&5`, `<red>`,
    `<#55ffff>`, `&a`, `<dark_purple>`. None on `&l` or `&r`.
11. **Colour picker.** Click the swatch on `&6` and pick another colour: it is
    rewritten as `&x&…` legacy hex. Do the same on `<red>`: it becomes
    `<#rrggbb>`. Undo both.
12. **Links (block 3).** Ctrl+click (Cmd+click on macOS) each of: the URL in
    the block 3 comment, the `<link:…>` URL, the bare URL in the third
    `send`, and the `docs` option's URL at the top. Each opens in the browser.
13. **Unknown option quick fix (block 4).** Put the cursor on `{@farewel}` and
    open code actions (Ctrl+. / Cmd+.). The action *Add `farewel` to options*
    adds `farewel:` to the `options:` block and the error disappears. Undo.
14. **Signature help (block 5).** On the marked line, type `reward(`. The hint
    shows `reward(p: player, amount: number = 10, offset: vector = vector(1, 7, 0), loud: boolean = true)`
    with `p` highlighted, and the default `vector(1, 7, 0)` is one piece.
15. Continue typing `player, `: `amount` is highlighted. Type `loud: `:
    `loud` is highlighted, not `offset`.
16. **Hover on a variable (block 6).** Hover `{_p}` in `set {_hat} to {_p}'s helmet`:
    a local variable, used 5 times in this trigger. Hover `{_offset}` in block
    5: it says it is the `offset: vector` parameter of `reward`.
17. **Hover inside a line (block 6).** Hover `helmet` in the same line:
    *Armor Slot*, not *Change: Set/Add/Remove*. Hover `set`: the Set effect.
18. **Hover on functions (block 6).** Hover `floor`: Skript's built-in
    `floor(n: number)` with its documentation. Hover `reward` and `giveCoins`:
    their signatures from this file.
19. **Inlay hints (block 6).** `reward({_p}, {_amount})` shows `p:` and
    `amount:` before its arguments.
20. **Subcommands (block 7).** No parse error anywhere around `/bank`. Open the
    outline (Ctrl+Shift+O): `/bank` contains `subcommand balance`, the
    `subcommands` group and, inside it, `subcommand give <target: player> <amount: number>`.
    Fold `subcommand balance` and the `subcommands` group separately.
21. **oopsk struct (block 8).** `Message` is coloured as a type and its fields
    as properties. The outline shows `Message` with `contents`, `sender` and
    `timestamp` under it, and the struct folds. Hover `timestamp`: its type and
    default, `date = now`.
22. **oopsk syntax.** Hover `struct copy` and `template of` in the `on chat:`
    trigger: oopsk's *Struct Copy* and *Template of Struct*, each marked
    "from oopsk". No warning on any line saying oopsk is not installed.

## ClassySK: classysk.sk and equipment.sk

23. **No false errors.** Neither file shows any diagnostic. Method calls such
    as `self::start()` and `{-actionBar::%player%}::play(…)` are not reported
    as unknown functions.
24. **Outline and folding.** In `classysk.sk` the outline shows
    `SmartActionBar` with its four fields and the `play` and `start` methods,
    with parameters. The class and each method fold.
25. **Hover and navigation.** Hover `play` in `{-actionBar::%player%}::play(…)`:
    `SmartActionBar::play(value: string, duration: timespan)`. F12 jumps to the
    method; Shift+F12 lists its uses.
26. **No member rename.** F2 on `play` or `start` is refused. F2 on `{_value}`
    inside `play` renames only within `play`.
27. **Formatting.** Format `classysk.sk` (Ctrl+Shift+I / Cmd+Shift+I). Nothing
    moves; the class body stays indented.
28. **Highlighting.** `public`, `private`, `static` and `const` are coloured as
    modifiers; field names, method names, `self` and `::` have their own
    colours.
29. **Snippets.** On an empty top-level line in `scratch.sk`, type `class`,
    `method` and `newinstance` in turn and accept each snippet.
30. **Argument hints for methods.** In `equipment.sk`, type
    `Equipment::get(` on a new line inside `fullEquipUpdate`: the hint shows
    `slot: string, item: item`. Undo.
31. **Expression colouring.** In `equipment.sk`, `{_p}'s helmet` and
    `future event-item` are coloured inside the call arguments, the `p:` /
    `e:` / `s:` names of named arguments are coloured, so is `Equipment` in
    `Equipment::get(…)`, and `switch` / `case` / `default` are keywords.
32. **Function headers.** In `function updateEquip(p: player, e: object, s: boolean = true):`
    the name, each parameter, each type and the default have separate
    colours, not one colour across the whole line.

## Other files

33. **Hover on a call.** In `showcase.sk`, hover `greet` in
    `send greet(player's name) to player`: `greet(name: text) :: text`, not
    the Send effect. (`reflect.sk` declares a `greet(p: player)` too, so that
    signature is listed as well.)
34. **Regression sweep.** `showcase.sk`, `addons.sk`, `reflect.sk` and
    `library.sk` show no errors, and the outline of each looks as before. The
    only diagnostics in them are the local-variable ones listed in step 35.

## New checks

35. **Local variables.** These are the only ones in the sample project:
    * `addons.sk` line 51, `call event {_event}`: a warning, *`{_event}` is
      never set in this trigger, so it is always empty here.* Line 105,
      `{_gui}`: the same.
    * `reflect.sk` lines 140 (`{_future}`) and 144 (`{_date}`), and
      `showcase.sk` lines 164 to 166 and 169 (`{_number}`, `{_decimal}`,
      `{_flag}`, `{_percent}`): the variable is drawn faded, and hovering it
      says *set but never read in this trigger*. These are hints, not
      warnings.
36. **Wrong argument count.** In `scratch.sk`, add two lines under `on join:`:
    `reward()` and `giveCoins(player, 5, 6)`. Both are errors:
    *`reward` takes 1 to 4 arguments, but this call passes 0 arguments.* and
    *`giveCoins` takes 1 to 2 arguments, but this call passes 3 arguments.*
    Change them to `reward(player)` and `giveCoins(player, 5)`: both clear.
    Undo.
37. **`cancel event` that cannot work.** In `scratch.sk`, add `cancel event`
    under `on join:`. A warning: *This event cannot be cancelled: `On Join` is
    not cancellable, so Skript refuses `cancel event` here.* Open code actions
    on it: *Remove `cancel event`* deletes the line. The `cancel event` under
    `on chat:` in `reflect.sk` (line 120) has no warning, because chat can be
    cancelled.

## Settings

The next steps change settings live. Put them in
`examples/sample-project/.zed/settings.json`, in a `settings` block next to
`initialization_options`, and repeat `serverPath` there: a `settings` block
replaces the startup options, so without it the addons stop being detected.

```json
"settings": {
  "serverPath": "<the same path as in initialization_options>",
  "features": { "hover": false }
}
```

38. **A feature switch.** Save the file with `"hover": false`. In
    `features.sk`, hover `helmet` on line 113: nothing appears. Completion,
    colours and diagnostics still work. Change it to `"hover": true` and save:
    hover works again, without restarting anything. (Set it back to `true`
    rather than deleting the block: deleting it sends nothing, and the server
    keeps the last settings it was given.)
39. **localVariableDiagnostics.** With `addons.sk` open, add
    `"localVariableDiagnostics": false` to the same block and save. The warnings on `addons.sk` lines 51 and 105 and the
    faded variables from step 35 disappear. Set it to `true`: they return.
    Remove the `settings` block afterwards and run
    **editor: restart language server**.

## Skript 2.17 command entries

40. **Subcommand completion.** In `features.sk`, put the cursor at the end of
    line 133 (the `send` under `subcommand balance`), press Enter, then
    Backspace once so the cursor sits level with `trigger:`. Press Ctrl+Space:
    the list holds command entries (`trigger:`, `suggestions:`, `permission:`,
    `executable by:`, `subcommand`, `subcommands:` and the rest), and no
    effects. Accept `suggestions:`: it inserts `suggestions:` with the cursor
    on an indented line below it.
41. **Entry hover.** Hover the `suggestions` you just inserted: it explains the
    entry and mentions the `tab completions` spelling. Undo. In `reflect.sk`,
    hover `permission message` on line 67: it says the entry is deprecated
    since Skript 2.17.
42. **Snippets.** On an empty top-level line in `scratch.sk`, type
    `commandsub`, `subcommand`, `subcommands` and `suggestions` in turn and
    accept each snippet. Undo.

## Formatting

43. **Format selection.** In `scratch.sk`, below the existing trigger, type
    this, indented with two spaces whatever Zed suggests:
    ```
    on quit:
      send "a"
    on respawn:
      send "b"
    ```
    Select only the `send "a"` line and run **editor: format selections**. The
    `on quit:` block is re-indented with your tab settings; `on respawn:` and
    its line keep their two spaces.
44. **Blank line between structures.** Format the whole file
    (Ctrl+Shift+I / Cmd+Shift+I). `on respawn:` is re-indented too, and one
    blank line appears between the `on quit:` block and `on respawn:`.
    Formatting again changes nothing. Undo the format, put
    `"formatBlankLines": false` in a `settings` block as in step 38, and
    format again: the indentation is fixed but no blank line is added. Remove
    the setting and the test lines afterwards.

## skript-reflect custom syntax

`reflect.sk` defines four pieces of syntax (lines 160 to 177) and uses them in
`command /custom:` (lines 178 to 184).

45. **Hover.** Hover `greet everyone` on line 180: *Custom effect defined with
    skript-reflect in `reflect.sk`*, ending *Defined in `reflect.sk`, line
    160.* Hover `the coins of player` on line 181 (an expression, line 165),
    `player is rich` on line 182 (a condition, line 169) and `announce` on
    line 183 (a local effect, "only usable in that script", line 174).
46. **Go to definition.** F12 on `greet everyone` jumps to line 160; F12 on
    `announce` jumps to line 174.
47. **Outline and colour.** The outline lists `greet everyone`,
    `[the] coins of %player%`, `%player% is rich` and `announce %string%` as
    definitions, not as events. The four uses in `command /custom:` are
    coloured like core syntax.
48. **Completion.** In `scratch.sk`, under `on join:`, type `greet ev`:
    `greet everyone` is offered, labelled *this project · effect*. Undo.

## Completion in context

49. **Event values.** In `scratch.sk`, add a trigger `on break:` with the line
    `send ev` under it. With the cursor after `ev`, the list starts with
    `event-block`, `event-entity`, `event-location`, `event-player`,
    `event-world`, `future event-block` and `past event-block`. Change the line
    to `send past event-`: `past event-block` is the only event value offered.
    Undo.
50. **Addon section entries** (needs the test server from step 4). In
    `scratch.sk`, add a trigger `on load:` with `register shaped recipe:` under
    it, then press Enter so the cursor is indented under that line. Press
    Ctrl+Space: `id:`, `ingredients:`, `result:` and `shape:` come first,
    marked *required*, then `category:` and `group:`, marked *optional*.
    Accept `result:` and hover it: the card explains the entry. Undo.

## Optional

51. **Diagnostics refresh.** Close Zed, delete `%LOCALAPPDATA%\skript-lsp`
    (Windows) or `~/Library/Caches/skript-lsp` / `~/.cache/skript-lsp`, reopen
    the folder and immediately open a file calling `floor(…)`. Once the log says
    the database loaded, any *unknown function* error on `floor` disappears
    without editing.
52. **customSyntaxPaths folders.** Put one or more syntax `.json` files in a
    folder and set `"customSyntaxPaths": ["/abs/path/to/folder"]`. After a
    restart, the log lists them and their syntax hovers.
53. **skript-minestom.** Make a folder with an empty `Skript/scripts/` and a
    `server.properties` containing `auth-type=ONLINE`, put a `.sk` file in
    `Skript/scripts/`, and open that folder in Zed. The language server log
    (command palette, **dev: open language server logs**) says a
    skript-minestom server was found and its syntax loaded, and typing
    `instance container` completes. Setting `"platform": "bukkit"` switches
    back to Skript's.
54. **SkAnalyzer.** With Java 25, the SkAnalyzer shell jar and a
    `Skript-v2.16-skratched.jar` in the analyzer work directory's `plugins/`
    (see the README), set
    `"analyzer": { "jar": "/abs/path/to/skanalyzer-shell-bundler-….jar" }`, with
    no `enabled`. Add `send "hi" to playerr` to a trigger in `scratch.sk` and
    save: an error from `SkAnalyzer` appears on that line. Fix it and save
    again: it clears. Add `"enabled": false` to the `analyzer` block, restart
    the language server, put the mistake back and save: nothing from
    SkAnalyzer.
55. **RCON reload.** Needs a real Paper server with Skript, on your machine.
    In its `server.properties` set `enable-rcon=true`, `rcon.port=25575` and
    an `rcon.password`, check that `plugins/Skript/config.sk` has
    `script loader thread size: 0`, and start the server. Open the server's
    `plugins/Skript/scripts` folder in Zed and set
    `"rcon": { "enabled": true, "password": "<the password>" }`.
    * Create `zedtest.sk` with `on join:` and, under it,
      `frobnicate the player`. Save: an error from `Skript (server)` appears
      on line 2, *Can't understand this condition/effect: frobnicate the
      player*.
    * Change the line to `send "hi" to player` and save: the error clears, and
      **dev: open language server logs** shows *reloaded zedtest.sk on the
      server*.
    * Rename the file to `-zedtest.sk` and save: nothing is sent, and the log
      says Skript treats `-` scripts as disabled.
    * Set a wrong password, restart the language server and save: one message
      says the password was rejected. Save again: no second message, only a
      line in the log.
56. **AI helper.** Run the binary from step 1 as `skript-lsp mcp --help`: it
    prints its usage and exits. Then add to your Zed user settings:
    ```json
    "context_servers": {
      "skript-knowledge": {
        "command": "/abs/path/to/skript-lsp",
        "args": ["mcp"],
        "env": {}
      }
    }
    ```
    With the sample-project folder open, Zed's agent settings list
    `skript-knowledge` as running. Ask the agent to "use search_syntax to find
    the Skript syntax for an action bar": it calls `search_syntax` and its
    answer names Skript's *Action Bar* effect. Remove the setting afterwards:
    nothing from this extension starts it otherwise.


## Doc comments

`docs.sk` documents three functions and a command, in both comment styles.

57. **Hover.** Hover `welcomeKit` on line 63: the signature, then *Gives a
    player the starter kit and tells them about it.*, a **Parameters** list
    with `p: player` and `loud: boolean` and their text (no `amount`), the
    return value beside `boolean`, **Since** 1.2 and the example as code.
    Hovering the declaration's name on line 35 shows the same card. Hover
    `rollDrop` on line 66: the `#d` lines render the same way.
58. **Deprecation.** `oldKit` on line 64 is struck through; its hint reads
    *deprecated: use welcomeKit() instead, which can announce the kit*, and
    its hover card opens with **Deprecated:**. Set
    `"deprecatedSyntaxDiagnostics": false` in a `settings` block as in step
    38: the strikethrough goes and the hover stays. Remove the setting.
59. **Stale `@param`.** `amount` on line 29 has a hint, *`@param amount`
    names no parameter of `welcomeKit`*. These two hints are the only
    diagnostics in the file.
60. **Argument hints and completion.** On a new line under line 64, type
    `welcomeKit(`: the popup shows the description and return value, and the
    `p` parameter carries *the player who receives the kit*. Type `player, `:
    `loud` is highlighted with its own text. Delete the line, type `welcomeK`
    and open the documentation beside the completion: the same card. Undo.
61. **Option values.** On line 68, `{@prefix}` is followed by an inlay hint
    `= "&8[&6Kits&8]&r"`; hovering the hint names the option and its line.
    `{@cooldown}` on line 58 shows `= "10 minutes"`. With `"inlayHints":
    false` under `features`, neither appears.
