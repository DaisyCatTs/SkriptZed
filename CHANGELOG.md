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

## [0.2.0] — 2026-08-07

### Added

- **Call hierarchy**, in the language server. Events and commands appear as
  callers alongside other functions, because in Skript they are the places a
  function actually runs from. Two calls in one trigger collapse into one entry,
  and a `local function`'s callers stay in its own file.

  **Zed cannot show this yet** — it does not implement the call hierarchy
  requests, so there is no menu item for it. This is listed because the server
  answers it for any client that does ask, and so that it is not mistaken for
  a missing feature when Zed adds support.
- **A duplicate declaration now links to the first one.** The diagnostic on the
  second `function payout()` points back at the first, so the fix is one click
  rather than a search.
- **`skript-lsp --version`.** Previously the binary had no flags at all, so
  running it by hand looked like it had hung — it was waiting for LSP traffic
  on stdin. It now prints its version and exits.

### Changed

- **Completion items read as two columns.** The pattern sits next to the name
  where it works as a signature, and the addon and category are right-aligned
  out of the way. Everything used to be crammed into one line, which made a
  long list unscannable. Clients that do not support this still get the old
  single string.

## [0.1.0] — 2026-08-07

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
- A nested outline — commands contain their entries, events contain their
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
  Requires `"semantic_tokens": "combined"` in your Zed settings — see the README.
- Diagnostics: indentation Skript would reject, unclosed `###` blocks, duplicate
  declarations, calls to functions that do not exist, and deprecated syntax.
- Formatting that re-indents from the parse tree and refuses to touch a file
  that does not parse.

#### Addons and syntax data

- Automatic addon detection: the server finds your `plugins/` directory and
  reads each JAR's manifest — `paper-plugin.yml` first, then `plugin.yml`, which
  is what makes SkBee detectable — and loads syntax only for what is installed.
- 2,660 core Skript patterns across 1,222 entries from `docs.skriptlang.org`,
  and 12,877 addon patterns across 168 addons from SkriptHub. Both are
  downloaded at runtime and cached, never bundled: the databases are GPL-3.0 and
  unlicensed respectively, and this project is MIT.
- Version awareness — syntax your target Skript cannot run is labelled and
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

#### Performance

- Classifying a line is remembered, so an edit costs only the lines that
  changed. An 800-line script went from ~145 ms per pass to ~1 ms.

- Structure entries (`permission:`, `cooldown:`, `trigger:`, `options:` names)
  now classify, hover and complete. Inside a command, completion offers its
  entries rather than the effects that cannot legally appear there. Every line
  of every example script Skript ships is now understood.
- A space was registered as a completion trigger, so the popup was open while
  typing ordinary prose and the editor gave Enter to the completion instead of
  to a newline.

- Alternatives inside a group that carry a space — `(is|are)(n't| not)`,
  `ha(s|ve)[(n't| not)]`, `off[ |-]hand` — matched only their contracted
  spelling, so everyday conditions like `{_x} is not set` and `{_p} is IP
  banned` resolved to the wrong syntax or to nothing.
- A structure's entries (`description:`, `prefix:`, alias and variable
  declarations) were reported as unknown syntax when that diagnostic was
  enabled, which made the setting unusable on any script with a command block.

### Known limitations

- Ordinary statement prose is not coloured by the grammar, only by the language
  server's semantic tokens. Skript has no fixed vocabulary, so nothing lexical
  distinguishes an effect from a condition; the grammar colours only what is
  structurally certain.
- "Unknown syntax" diagnostics are off by default and should usually stay off.
  Setting `docsPath` does **not** make them safe — see the README.
- Renaming a command does not update places that invoke it. In Skript a command
  is called from inside a string (`execute player command "/home"`), so
  rewriting those would mean editing arbitrary text.
- Experimental features behind Skript's `using <experiment>` are not detected;
  `docs.json` has no field for them.
- Removed — as opposed to deprecated — syntax is not tracked. Neither data
  source records when something was removed.
- `Skellett` and `SkStuff` have no syntax data in any public source. They behave
  like any other unknown addon: silent, never an error.
- When several published syntaxes match one line, the most specific wins — but
  "specific" is scored from the pattern's mandatory words, which occasionally
  picks the wrong one. `{_x} is not set` is matched by both `Exists/Is Set` and
  the general `Comparison`, and hover shows the latter.

[Unreleased]: https://github.com/DaisyCatTs/SkriptZed/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/DaisyCatTs/SkriptZed/releases/tag/v0.1.0
