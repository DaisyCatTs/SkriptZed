# Changelog

All notable changes to the Skript extension for Zed are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
The version here is the one in `extension.toml`, which is the version the Zed
registry shows you.

House rule: if a change is not visible to somebody *using* the extension, it does
not belong in this file. Refactors, test additions and CI changes live in
`git log`.

## [Unreleased]

### Added

- **Argument completion.** Once a statement's start identifies its syntax,
  completion offers what fits the slot being typed: `kill pl` offers the
  event's `player` and other entity expressions, `teleport player to sp`
  offers Spawn (a location) and no entity-only expressions, a game mode slot
  offers `survival` and friends. Expressions are matched on their documented
  return type, with a small table of well-known supertypes (a player is an
  entity, a command sender and an offline player), and the trigger's locals
  are offered when you ask with Ctrl-Space after a space. Accepting one
  replaces only the word under the cursor, never the statement, and a word
  that already spells out a value (`kill player`) opens no popup, so Enter
  still starts a new line.
- **Doc comments.** The `#` lines directly above a function, command,
  ClassySK method or skript-reflect definition are its documentation, shown
  when you hover the declaration or any call, in completion, and in argument
  hints, where each parameter carries its own text. Both conventions other
  Skript editors use are read, so a documented file moves between editors
  unchanged: `@param`, `@return`, `@deprecated`, `@since` and `@example`, and
  the `#d`, `#dp`, `#dr`, `#dex` and `#ddep` family. A blank line detaches a
  comment, and a `###` block counts when it closes directly above. A
  `@param` naming no parameter is hinted, and a call to a `@deprecated`
  function is struck through with the reason (switched off with the other
  deprecation warnings by `deprecatedSyntaxDiagnostics`).
- **Option values inline.** With inlay hints on, `{@prefix}` is followed by
  the value the script's `options:` block gives it, `= "&6[Shop]"`, cut short
  past 30 characters.
- **"There's no player in a function event."** Skript's most common load
  error is now a warning as you type, in Skript's own words: `player`,
  `event-player`, `event-entity`, `event-block`, `attacker` and `victim`
  inside a function, or in an event whose documented values can never supply
  them (`every 5 seconds`, `on world load`). It stays silent wherever Skript
  might accept the line: commands, `on load` (whose command sender passes
  for a player), events with no documented values, addon events, sections
  that run their body in an event of their own, and slots where `player`
  could be read as an entity type. `eventValueDiagnostics: false` turns it
  off.
- **Empty sections.** A line ending in `:` with nothing under it gets
  Skript's own "Empty configuration section!" warning, following Skript's
  reader exactly: a comment-only body counts as empty, and a `#-#` line
  opens no section.
- **Unreachable code.** Statements after a `stop` or a `return` in the same
  block are faded and warned about, as Skript does on load. A script that
  suppresses Skript's unreachable code warnings is left alone.
- **Mistyped events.** `on jion:` is reported as a structure Skript cannot
  understand, with a quick fix to change it to `on join`. Only near-misses
  of a known event are reported, and only once every addon in your
  `plugins/` folder has its syntax loaded, so an addon's event is never
  mistaken for a typo. `unknownEventDiagnostics: false` turns it off.
- **Syntax newer than your server, opt-in.** With
  `newerSyntaxDiagnostics: true` and the Skript version known, from the
  detected `Skript.jar` or the `skriptVersion` setting, a line whose syntax
  arrived in a later Skript is warned about with the version it needs. Older
  syntax that also fits the line keeps it quiet. Off by default because
  syntax a later Skript rewrote (Play Effect in 2.14) still reads as new.
- **`loop-` completes the loop's own values.** Inside `loop all players:`,
  typing `loop-` offers `loop-player`, `loop-entity`, `loop-value` and
  `loop-iteration`; a list variable adds `loop-index`, `loop 10 times`
  offers `loop-number`. Nested loops are numbered exactly where Skript
  demands it (`loop-value-1`, `loop-value-2`) and nowhere else. Hovering a
  `loop-…` name says which loop it refers to and what type it is, or that it
  matches several loops, which Skript rejects.
- **Every listed value in argument completion.** A slot whose type lists its
  values offers them, ranked by what you typed: damage causes, biomes,
  potion effect types, enchantments (also for `%enchantment type%` slots,
  as in `sharpness 5`), villager professions, game rules, teleport causes
  and some forty more. Item and entity types stay out: their names are
  open-ended aliases.
- **Diagnostics for scripts you do not have open.** When a save, an edit, a
  new or deleted file or a `git pull` changes what the project declares, the
  closed scripts that call or redeclare it are rechecked for unknown
  functions, wrong argument counts and duplicates, so the project
  diagnostics panel shows a caller you just broke, and clears it once fixed.
  Only scripts inside the workspace are reported. Measured at about 12 ms for
  a full pass over 300 scripts, so it is on by default;
  `features.projectDiagnostics: false` turns it off.
- **Duplicates across scripts.** Two global functions with the same name and
  the same parameter types (an overload by type is fine), two commands with
  the same name, or two skript-reflect definitions with the same pattern are
  warned about in both places, each linking to the other.
- **TODO, FIXME, XXX and NOTE stand out in comments**, the tag word alone,
  with semantic tokens on.

### Fixed

- **Pressing Enter at the end of a line could replace it.** The completion
  popup was still open with a suggestion selected, and that suggestion stood
  in for the whole statement although only its last word matched:
  `kill player` became `player from uuids`, `teleport player to spawn`
  became `spawn entity types`. A syntax suggestion now has to fit every word
  of the statement from its start, and none is offered once a string,
  variable or number has been written, so a finished line shows no popup and
  Enter starts a new line.
- **Suggestions rank the syntax you are typing first.** `tel` offers Teleport
  before Display Teleport Duration, and `cancel ev` offers Cancel Event
  before `event-player`.
- **An option's value was cut short.** Completion and hover showed
  `prefix: &6[Shop] of the year` as `&6`; they now show the whole value.

## [0.3.0] - 2026-11-02

### Added

#### Skript 2.17

- **Subcommands.** `subcommand <number>:` inside a command was a parse error,
  which broke the outline and folding for the whole file. Subcommands now
  parse, directly in a command or inside a `subcommands:` group, and show in
  the outline under their pattern.
- **The new command entries.** `prefix:`, `suggestions:` (also spelt
  `tab completions:`), `subcommand` and `subcommands:` complete and hover inside
  a command, and inside each subcommand, which takes the same entries. Hovering
  `permission message:` says it is deprecated since 2.17: a command the player
  may not run is no longer sent to their client, so the message is never seen.
  Four new snippets: `commandsub`, `subcommand`, `subcommands` and
  `suggestions`.

#### Addons

- **ClassySK support.** [ClassySK](https://github.com/novystar/ClassySK) adds
  classes to Skript, and a ClassySK script used to be full of errors that could
  not be fixed: every method, from its declaration to `self::start()` and
  `{_bar}::play(…)`, was reported as a call to an undeclared function. Now:
  - `class`, fields, methods and their parameters appear in the outline, and a
    class and each method fold.
  - Hovering a method or field shows its declaration
    (`SmartActionBar::play(value: string, duration: timespan)`), and
    go-to-definition and find-references follow it to the class. Members are
    deliberately not renamed: which class `{_x}::reset()` calls is only known
    at runtime, so renaming one class's `reset` could silently rewrite
    another's.
  - `{_x}` inside a method belongs to that method, so renaming it no longer
    touches a variable of the same name in a sibling method.
  - Formatting keeps a class body indented instead of flattening it to
    column 0.
  - Highlighting for `public`/`private`/`static`/`const`, field and method
    names, `self` and `::` member access, plus `class`, `method` and
    `newinstance` snippets.
- **oopsk support.** `struct Message:` templates parse as structures, with
  their fields in the outline and folding, instead of being read as an event
  whose fields were run through the statement matcher.
- **skript-minestom support.** Open a skript-minestom server and it is
  recognised from its folder layout: the server loads skript-minestom's own
  syntax and every addon its docs site publishes (skript-gui, SkCheese,
  skript-combat and more) instead of Bukkit Skript's. `"platform"` set to
  `"minestom"` or `"bukkit"` overrides the guess.
- **skript-reflect custom syntax is understood.** An `effect`, `condition`,
  `expression`, property or custom `event` your project defines, `local` ones
  included, is read as you type. Every use of it is then coloured, completed
  and hovered like core syntax ("Defined in `rewards.sk`, line 12"), and
  go-to-definition jumps to the definition. The outline lists definitions as
  their own kind instead of as events, and `local effect …:` no longer breaks
  the parse.
- **Addon section entries.** Inside an addon section that SkriptHub documents,
  such as SkBee's `register shaped recipe:`, completion offers its entries
  (`id:`, `result:`, `shape:` and the rest) with the required ones you have not written
  yet first, and hover explains each. Lines like `result: diamond` are no
  longer reported as unknown syntax.
- **Check scripts against the real Skript** (opt-in, needs SkAnalyzer and
  Java 25). Set `analyzer.jar` and each saved script is run through Skript's
  own parser, and its errors appear as diagnostics from `SkAnalyzer`.
  `"enabled": false` pauses it without losing the path. You install SkAnalyzer
  and a Skratched Skript build yourself; see the README.
- **Reload on your own server as you save** (opt-in, off by default). With
  `rcon.enabled` and your server's RCON password, each save sends
  `skript reload <script>` over RCON, and the errors Skript prints come back as
  diagnostics from `Skript (server)`. Meant for a server on your machine or one
  whose scripts folder you have mounted; see the README for what
  `server.properties` and Skript's config need. The password is never logged.
- **`customSyntaxPaths` accepts folders**, loading every `.json` in them.

#### Editor

- **Hover explains what is under the cursor.** Pointing at `{_p}'s helmet`
  inside a `set` describes Armor Slot, not Set. A function call shows that
  function: your own function's signature, or a built-in like `floor` with its
  documentation, where it used to describe whatever pattern the whole line
  matched. `self` and a class name show the class and its members, and addon
  syntax shows examples from SkriptHub and a link to its page.
- **Expressions inside lines are coloured, not just the line's keywords.** What
  fills a pattern's slots, and every argument of a function call, is classified
  too: `{_p}'s helmet`, `future event-item`, `the target entity of player`. A
  literal-only slot such as the `helmet` in `{_p}'s helmet` reads as a type.
  Named arguments (`updateEquip(p: {_p})`), the class in `Equipment::get(…)`
  and SkBee's `switch`/`case`/`default` are coloured too.
- **Variable completion.** Typing `{_` offers the local variables used in the
  same trigger, plus the function's parameters; `{` offers those and every
  global the project uses. A plain word works too: `vis` offers `{_visitor}`
  ahead of any syntax. List variables complete one level at a time: after `{data::`
  the choices are what your scripts put at that level (`%player's uuid%`,
  `settings`) plus `*`.
- **Colour swatches.** `&6`, `&x&f&f&5&5&5&5`, `<red>`, `<#ff5555>` and the
  rest show the colour they produce, and picking a new one writes it back in
  the same style.
- **Clickable links** in `<link:…>` / `<url:…>` tags and bare URLs, comments
  included.
- **Argument hints for more calls.** Signature help covers ClassySK methods
  (`Req::get(`), Skript's built-ins and addon functions, highlights the right
  parameter for a named argument, and no longer splits a default like
  `(1, 7)` in two. Zed hides these hints after a completion unless
  `"show_signature_help_after_edits": true` is set.
- **Unknown options are errors.** `{@typo}` with no matching entry in the
  script's `options:` block is reported, with a quick fix that adds it.
- **Wrong argument counts are errors.** `reward()` when `reward` needs a
  player, or `giveCoins(p, 5, 6)` when it takes two, is reported, checked
  against your own functions and Skript's built-ins. Calls whose count cannot be
  known for certain are left alone, and a function declared more than once
  passes if any declaration fits.
- **`cancel event` where nothing can be cancelled.** In an event Skript's
  documentation marks as not cancellable, such as `on join:`, it is a warning,
  with a quick fix that removes the line. Skript refuses to load it.
- **Local variables that are never set, or never used.** `{_x}` read in a
  trigger that never sets it is a warning, since it is always empty there; one
  set but never read is shown faded. Both stay quiet whenever any line could
  explain the variable. `localVariableDiagnostics: false` turns them off.
- **`event-` completes the event's own values.** Inside `on break:`, typing
  `event-` offers `event-block`, `event-player` and the rest of that event's
  values first, with their `past` and `future` forms, for core, addon and
  custom events.
- **Format a selection.** `editor: format selections` re-indents just the
  selected code, widened to the whole top-level structures it touches so a
  structure never ends up half tabs, half spaces.
- **A blank line between structures.** Formatting puts one blank line under a
  multi-line structure that runs straight into the next. It never removes one.
  `formatBlankLines: false` turns it off.
- **Every feature has a switch.** A `features` block in the settings turns off
  hover, completion, variable completion, signature help, inlay hints,
  semantic tokens, colours, links, folding, formatting, code actions, rename,
  navigation, call hierarchy, the outline or diagnostics, one at a time. All
  are on by default, and a change applies without a restart.
- **Every setting applies without a restart.** Under `lsp.skript-lsp.settings`,
  changing `docsPath`, `addons`, `serverPath`, `platform` or another setting
  that decides where syntax comes from now reloads the syntax database; it used
  to need a restart.
- **An AI helper, for those who want one** (opt-in, never started by the
  extension). `skript-lsp mcp` is a read-only Model Context Protocol server
  that lets an AI agent search Skript syntax, read its documentation, classify
  lines and list the project's functions and commands. The extension does not
  register it with Zed, so nobody sees it who has not added it to their
  settings; the README shows how.
- **Proxies are honoured.** Syntax downloads follow the `HTTPS_PROXY`,
  `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` environment variables.

### Fixed

- **Settings under `lsp.skript-lsp.settings` no longer wipe the others.** They
  replaced `initialization_options` whole, so turning one feature off there
  silently dropped a `serverPath` set in the other block. The two are now
  merged, the later winning key by key.
- **A crash while typing deeply nested optional groups.** A skript-reflect
  pattern like `effect [[[[…]]]]` thousands deep overflowed the stack and took
  the server down. Nesting beyond 32 is now reported as an error.
- **Hover could crash on `§`, `é` or an emoji.** A cursor position inside a
  multi-byte character was used to slice the line.
- **Saving with SkAnalyzer on could freeze the editor.** The check, up to a
  minute while Java starts, held one of the server's few request slots per
  save; a few quick saves stalled every request. It now runs in the background.
- **A numeric `rcon.password` was shown in a warning.** An invalid setting is
  reported with its value; the password no longer is.
- **Files changed on disk were ignored when not open.** After a `git pull`,
  scripts that were not open in the editor kept their old contents in the
  index.
- **`prefix:` is no longer offered inside a subcommand,** which Skript only
  accepts on the command itself, and SkBee's `ingredients:` is no longer
  reported as unknown syntax.
- **With no settings at all, addon syntax never loaded.** A user who had not
  configured anything got an empty addon source, so SkBee and every other
  detected addon stayed plain, and deprecated syntax was not flagged. No
  settings and empty settings now mean the same defaults.
- **One bad setting no longer throws away the rest.** A single value of the
  wrong type (`"addons": true`) used to reset every setting, the analyzer
  included, and a misspelt key was silently ignored. Each bad or unknown key
  is now dropped on its own, and one message names them.
- **Hover on a SkriptHub event listed `event-event-player`.** Event values from
  SkriptHub records already carry the `event-` prefix, and it was added again.
- **Addon syntax containing a regex character class never matched.** A pattern
  such as ClassySK's `<[A-Z]\w*>` was misread, so ClassySK, oopsk and packet
  field patterns could never apply to any line.
- **181 addon syntaxes were dropped as duplicates of core Skript.** They share
  an internal id with a core entry (Lusk, SkQuery and SkBee all have some), and
  were discarded as copies. They now load.
- **`{@option}` inside a string is recognised.** It was not seen at all, so it
  could be neither checked nor navigated.
- **A closed file's diagnostics no longer come back.** Closing a tab cleared
  its problems, but the next refresh (when the project finished indexing, or a
  setting changed) put them back in the problems panel.
- **A function header is no longer one flat colour.** The language server
  painted the whole of `function name(p: player, s: boolean = true):` with a
  single colour, over the grammar's separate colours for the name, parameters
  and types. `command` headers had the same problem.
- **Diagnostics refresh once the syntax database arrives.** A file opened
  while the database was still downloading kept showing built-ins like
  `floor(…)` as undeclared until it was edited.
- **A line no addon pattern explains is no longer mislabelled.** Patterns made
  only of optional words and one slot match any line; taking them as an answer
  hid the function-call colouring for lines like `updateEquip(…)`.

## [0.2.5] - 2026-08-07

### Added

- **skript-reflect's Java calls are coloured.** `Bukkit.getVersion`,
  `Material.DIAMOND` and `{_item}.getItemMeta()` were the one thing in a `.sk`
  file that nothing described (no Skript pattern covers them), so they sat as
  plain text, which in a reflect-heavy script is most of the file. They cannot
  be classified, but they have a recognisable shape, and neither form occurs
  anywhere in the 540 scripts Skript itself ships, so ordinary Skript is
  untouched. A reflect-heavy file goes from 83% to 94% coloured.

## [0.2.4] - 2026-08-07

### Fixed

- **Skript's own functions are no longer reported as missing** ([#3]). `floor`,
  `abs`, `mod`, `uuid` and the other 41 built-ins are registered at runtime and
  declared in no script, so calling one was an error. Functions registered by
  addons are covered by the same fix.
- **Indentation is judged per structure, not per file** ([#2]). Skript infers
  the indent unit for each top-level structure separately: 7 of the 540 scripts
  Skript itself ships switch between tabs and spaces between structures. Mixing
  *inside* one structure is still reported.
- **A comment no longer decides the file's indentation.** One comment aligned
  with spaces in a tab-indented script made every correct line below it an
  error.
- **skript-reflect constructors are not missing functions.** `new ArrayList()`
  was read as a call to a Skript function named `ArrayList`, which nothing could
  ever declare.

[#2]: https://github.com/DaisyCatTs/SkriptZed/issues/2
[#3]: https://github.com/DaisyCatTs/SkriptZed/issues/3

## [0.2.3] - 2026-08-07

### Fixed

- **An event header with a filter reads as one thing again.** In
  `on left-click on a sign:`, the words `a sign` kept a leftover colour from the
  grammar that also claimed they were a function. The event's own words and the
  type it filters on are now coloured deliberately and differently, so the line
  reads as "the click event, filtered to signs" in any theme.
- Removed a styling rule for a token the language server never emits, which
  looked like configuration and could never apply.

## [0.2.2] - 2026-08-07

### Fixed

- **No more phantom "No function named `x` is declared in this project".** An
  editor opens your file the instant the server starts, before the project's
  scripts have been read off disk, so a call to a function in a file you had
  not opened was reported as missing, and the error stayed until you edited
  something. The check now waits until the project has been read, and
  diagnostics refresh once it has.

## [0.2.1] - 2026-08-07

### Fixed

- **Core Skript syntax is no longer mistaken for an addon's.** Ranking scored
  what a pattern *demanded* rather than what it *explained*, so a pattern whose
  words were mandatory beat one whose words were optional even when both matched
  the same line identically. On a server running BungeeSK, `send "hi" to player`
  was documented as BungeeSK's "Send bungee player to server" instead of core
  Skript's Message effect. Classification accuracy against Skript's own examples
  rose from 96.9% to 99.4%.

- **`on right click` and `on left click` are recognised again.** Skript accepts
  a separator as a space, a hyphen or nothing, so all of `right click`,
  `right-click` and `rightclick` are the same event, but only the last two were
  matching. The spaced spelling was being deleted while the pattern was
  compiled, so two of the most-written lines in Skript resolved to nothing and
  silently fell back to the generic event structure: no hover, no event values,
  no documentation. 43 patterns use that construct, including `off hand`,
  `IP banned` and `left handed`.

## [0.2.0] - 2026-08-07

### Added

- **Call hierarchy**, in the language server. Events and commands appear as
  callers alongside other functions, because in Skript they are the places a
  function runs from. Two calls in one trigger collapse into one entry,
  and a `local function`'s callers stay in its own file.

  In Zed, run `call hierarchy: show incoming calls` (Zed added the call
  hierarchy view after this release).
- **A duplicate declaration now links to the first one.** The diagnostic on the
  second `function payout()` points back at the first, so the fix is one click
  rather than a search.
- **`skript-lsp --version`.** Previously the binary had no flags at all, so
  running it by hand looked like it had hung: it was waiting for LSP traffic
  on stdin. It now prints its version and exits.

### Changed

- **Completion items read as two columns.** The pattern sits next to the name
  where it works as a signature, and the addon and category are right-aligned
  out of the way. Everything used to be crammed into one line, which made a
  long list unscannable. Clients that do not support this still get the old
  single string.

## [0.1.0] - 2026-08-07

First release. Zed had no Skript support of any kind before this, so everything
below is new.

### Added

#### Editing

- Syntax highlighting for structures, sections, strings with `%interpolation%`,
  `<format tags>` and `&6` colour codes, variables distinguished by scope,
  options, commands, functions, literals and comments.
- Indentation that follows Skript's own rules: auto-indent after any
  `:`-terminated line, `else` and `else if` snapping back to their `if`, and
  support for Skript's `#-#` marker for a line that ends in a colon without
  opening a section.
- Folding for every event, command, function, section, `options:` / `variables:`
  block and `###` comment, each with a placeholder describing what was folded.
- A nested outline: commands contain their entries, events contain their
  sections.
- 61 snippets covering every structure, event, loop and common effect.

#### Language server

- Go to definition, find references and rename for functions, commands, options
  and variables, across the whole project including files you have not opened,
  respecting `local function` scope and confining `{_local}` variables to the
  file that declares them.
- Hover with description, syntax, examples, event values, `since`, deprecation
  and addon requirements, taken from Skript's own documentation database.
- Context-aware completion: only events after `on `, only conditions inside
  `if `, only expressions inside `%…%`. Patterns insert as snippets with a tab
  stop per slot.
- Signature help, and inlay hints showing parameter names at call sites.
- Occurrence highlighting for the symbol under the cursor.
- Semantic tokens classifying effects, conditions, expressions and events.
  Requires `"semantic_tokens": "combined"` in your Zed settings; see the README.
- Diagnostics: indentation Skript would reject, unclosed `###` blocks, duplicate
  declarations, calls to functions that do not exist, and deprecated syntax.
- Formatting that re-indents from the parse tree and refuses to touch a file
  that does not parse.

#### Addons and syntax data

- Automatic addon detection: the server finds your `plugins/` directory and
  reads each JAR's manifest (`paper-plugin.yml` first, then `plugin.yml`, which
  is what makes SkBee detectable) and loads syntax only for what is installed.
- 2,660 core Skript patterns across 1,222 entries from `docs.skriptlang.org`,
  and 12,877 addon patterns across 168 addons from SkriptHub. Both are
  downloaded at runtime and cached, never bundled: the databases are GPL-3.0 and
  unlicensed respectively, and this project is MIT.
- Version awareness: syntax your target Skript cannot run is labelled and
  sorted down, never hidden.
- `customSyntaxPaths` for private or undocumented addons, in either the
  `docs.json` or the SkriptHub schema; the shape is detected.

#### Themes

- No colour is defined anywhere in the extension. Highlighting uses capture
  names your theme already styles, with fallback chains for refinements a
  minimal theme may not define. Checked against Catppuccin, One Dark, Material
  Theme Darker and OLED themes.

#### Also added

- Quick fixes: correct a mistyped function name, create a missing one, fix the
  file's indentation, close an unterminated `###` block.
- The project re-indexes when scripts change on disk, so pulling a teammate's
  new script no longer leaves permanent "no function named …" errors on correct
  code.
- Settings apply without restarting the language server, including the
  `lsp.skript-lsp.settings` location that was previously read by nothing.
- Deprecated syntax is tagged as such, so it strikes through in any theme.

#### Fixed before release

- **Completion could not find most syntax by the word you type.** The list
  matched on Skript's documentation *title* while you type the *pattern*, so
  `send` never surfaced the effect filed under "Message". Effects reachable by
  their own keyword went from 58% to 100%, conditions from 33% to 100%.
- **Renaming a function parameter silently broke the function.** The body was
  rewritten and the signature left behind, producing something that still ran
  and did nothing. Go-to-definition on a parameter now works too; it previously
  returned nothing at all.
- **Renaming a local variable rewrote every trigger in the file.** Skript scopes
  `{_x}` to the running trigger; renaming `{_i}` onto a name a sibling trigger
  already used silently merged two unrelated variables.
- `if`, `else if` and `while` hid the condition they introduce, so hovering
  `if plugin "Vault" is enabled:` described the generic conditional section
  rather than the condition.
- Variables are no longer italicised in themes that style `variable.member`.
- Structure entries (`permission:`, `cooldown:`, `trigger:`, `options:` names)
  now classify, hover and complete. Inside a command, completion offers its
  entries rather than the effects that cannot legally appear there. Every line
  of every example script Skript ships is now understood.
- A space was registered as a completion trigger, so the popup was open while
  typing ordinary prose and the editor gave Enter to the completion instead of
  to a newline.
- Alternatives inside a group that carry a space (`(is|are)(n't| not)`,
  `ha(s|ve)[(n't| not)]`, `off[ |-]hand`) matched only their contracted
  spelling, so everyday conditions like `{_x} is not set` and `{_p} is IP
  banned` resolved to the wrong syntax or to nothing.
- A structure's entries (`description:`, `prefix:`, alias and variable
  declarations) were reported as unknown syntax when that diagnostic was
  enabled, which made the setting unusable on any script with a command block.

#### Performance

- Classifying a line is remembered, so an edit costs only the lines that
  changed. An 800-line script went from ~145 ms per pass to ~1 ms.

### Known limitations

- Ordinary statement prose is not coloured by the grammar, only by the language
  server's semantic tokens. Skript has no fixed vocabulary, so nothing lexical
  distinguishes an effect from a condition; the grammar colours only what is
  structurally certain.
- "Unknown syntax" diagnostics are off by default and should usually stay off.
  Setting `docsPath` does **not** make them safe; see the README.
- Renaming a command does not update places that invoke it. In Skript a command
  is called from inside a string (`execute player command "/home"`), so
  rewriting those would mean editing arbitrary text.
- Experimental features behind Skript's `using <experiment>` are not detected;
  `docs.json` has no field for them.
- Removed (as opposed to deprecated) syntax is not tracked. Neither data
  source records when something was removed.
- `Skellett` and `SkStuff` have no syntax data in any public source. They behave
  like any other unknown addon: silent, never an error.
- When several published syntaxes match one line, the most specific wins, but
  "specific" is scored from the pattern's mandatory words, which occasionally
  picks the wrong one. `{_x} is not set` is matched by both `Exists/Is Set` and
  the general `Comparison`, and hover shows the latter.

[Unreleased]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/DaisyCatTs/SkriptZed/releases/tag/v0.1.0
