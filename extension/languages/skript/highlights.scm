; Skript syntax highlighting for Zed.
;
; THEME PORTABILITY IS THE POINT OF THIS FILE.
;
; No colour is ever named here. Every capture uses a name that Zed's bundled
; One theme and the popular community themes (Catppuccin, Material, the OLED
; family) all define, or a dotted refinement of one — Zed's
; `SyntaxTheme::highlight_id` walks a dotted capture back along its dot-prefixes
; (`string.special.symbol` -> `string.special` -> `string`), so a refinement is
; always safe as long as its root is common. Bare invented names have no
; fallback and are deliberately avoided.
;
; Where two captures are listed on one node, Zed resolves them right-to-left:
; it tries the rightmost first and falls back leftwards until the theme has a
; style. Later patterns in this file win over earlier ones.
;
; WHAT IS NOT HIGHLIGHTED, ON PURPOSE
;
; Skript has no fixed keyword set: effects, conditions and expressions are
; runtime-registered patterns (2,117 of them in core Skript alone, plus every
; addon). This file therefore colours only what is structurally certain, and
; leaves ordinary statement prose uncoloured. The language server classifies the
; rest and paints it via LSP semantic tokens — see semantic_token_rules.json.
; Skript's own grammar guidance puts it well: "It is better not to highlight a
; structure than to incorrectly identify it."

; ------------------------------------------------------------------ comments

(comment) @comment
(block_comment) @comment

; --------------------------------------------------------------- punctuation

(punctuation) @punctuation.delimiter

(punctuation
  [
    "("
    ")"
    "["
    "]"
  ] @punctuation.bracket)

(punctuation "%" @punctuation.special)

; Parentheses that belong to a rule rather than to a `punctuation` node.
(function_call
  [
    "("
    ")"
  ] @punctuation.bracket)

(function
  [
    "("
    ")"
  ] @punctuation.bracket)

; The `<` and `>` around a command argument bracket a slot, they do not
; separate a list.
(command_argument
  (punctuation) @punctuation.bracket)

(operator) @operator

(list_separator) @punctuation.delimiter

; ----------------------------------------------------------------- structures

; `command`, `function`, `local`, `options`, `variables`, `aliases`, `import`,
; `using`, `on`, `auto reload` — the words Skript itself reserves.
(keyword) @keyword

(event_modifier) @keyword.modifier @keyword
(event_priority) @keyword.modifier @keyword

; An event header names the event, so the whole name is coloured. This is safe
; in a way that colouring statement prose is not: the header of a top-level
; section is always an event name and never a user expression.
; The `name:` field repeats — `on first join` has two `name:` children, one per
; word. Without the `+` the pattern matched once and coloured only `first`,
; leaving `join` plain on every multi-word event in the language.
(event
  name: [
    (identifier)
    (word)
  ]+ @function)

(command
  name: (command_name) @function)

(function
  name: (identifier) @function.definition @function)

(function_call
  name: (identifier) @function.call @function)

(return_marker) @operator

(type) @type

(experiment) @constant

(import_path) @type

; --------------------------------------------------------------------- entries

; `permission:`, `description:`, `cooldown:`, and every key in `options:` or a
; Skript config file.
(entry
  key: (entry_key) @property)

(entry_section
  key: (entry_key) @property)

; `trigger:` is the one entry that opens executable code, so it reads as a
; keyword rather than a property.
((entry_section
  key: (entry_key) @keyword)
  (#eq? @keyword "trigger"))

(assignment
  target: (alias_name) @constant)

; ------------------------------------------------------------------- commands

(command_argument
  spec: (argument_spec) @variable.parameter @variable)

(command_arg_ref) @variable.builtin @variable

; ------------------------------------------------------------------ variables

; Every variable is a variable. The two refinements below are *positive*
; `#match?` tests, so a theme defining neither still renders every scope as
; `@variable` — the failure mode is today's appearance, not a blank one.
;
; Global is the *absence* of a scope sigil, and a tree-sitter query cannot ask
; for a missing child; anchors are no help either, since they ignore anonymous
; nodes. Testing the variable's own text is the only positive form available.
(variable) @variable

; The three scopes are *not* given separate captures, and that is deliberate.
;
; An earlier version marked globals `@variable.special` and ephemerals
; `@variable.member`. Both are semantically wrong: `variable.special`
; conventionally means `this`/`self`, and `variable.member` means a struct
; field. A Skript global is neither — so themes styled them by their own
; meaning, which in several themes includes *italics*, and variables started
; leaning over for no reason a reader could infer.
;
; The distinction is already visible anyway: the sigil itself is captured below,
; so `{_x}` and `{-x}` differ from `{x}` right where the difference lives. That
; is honest styling — it marks what is actually there rather than recolouring a
; whole token on a guess about what the theme means by "special".

; `_` marks a local and `-` an ephemeral variable; both are scope sigils rather
; than part of the name.
(variable_scope) @punctuation.special

; `{@option}` is substituted before parsing, so it is a compile-time constant
; rather than a variable.
(option_ref) @constant
(option_ref
  name: (option_name) @constant)

; `loop-player`, `loop-index`, `event-block` — provided by the surrounding
; section rather than declared by the user.
(loop_value) @variable.builtin @variable
(event_value) @variable.builtin @variable

(parameter
  name: (identifier) @variable.parameter @variable)

; -------------------------------------------------------------------- literals

(number) @number
(duration) @number
(boolean) @boolean

(string) @string

; `""` and `%%` — Skript escapes by doubling. These are escapes.
(escape_sequence) @string.escape

; `<red>`, `<bold>`, `<#FF00AA>`, `<link:…>`.
(format_tag) @string.special

; A tag carrying a URL or a command rather than a colour. Refinement of
; `string.special`, so it degrades to it wherever `url` is undefined.
((format_tag) @string.special.url
  (#match? @string.special.url "^<(link|url|cmd|sgt|suggest_command|run_command|open_url|insertion):"))

; Legacy `&6` colour codes and `&#RRGGBB`. These are *formatting*, not escapes —
; sharing `@string.escape` with the doubling escapes above made a colour code
; and a literal quote indistinguishable. Both roots fall back to `string`.
(legacy_color) @string.special.symbol

; `%…%` re-enters code context inside a string, so its contents read as code and
; its delimiters as the boundary between the two.
(interpolation
  "%" @punctuation.special)

(interpolation
  (interpolation_text) @embedded)

; `%arg-1%`, `%loop-value%`, `%event-player%`. The grammar keeps an
; interpolation's contents as one blob, but these are the same builtins that get
; `@variable.builtin` outside a string and should read the same way inside one.
((interpolation
   (interpolation_text) @variable.builtin)
  (#match? @variable.builtin "^(arg(ument)?-|loop-|event-)"))

; ---------------------------------------------------------------- control flow

; The only bare English words this file colours. Each of these is structural in
; Skript and cannot be redefined by an addon, and each is restricted to the
; position where it can only mean the control-flow construct.

(section
  header: (identifier) @keyword.conditional @keyword
  (#any-of? @keyword.conditional "if" "else" "then"))

; `do` is here because `do while …` gives the section two `header:` children.
; Matching only the first meant neither word coloured: the predicate was tested
; against `do`, failed, and the match was dropped before `while` was reached.
(section
  header: (identifier) @keyword.repeat @keyword
  (#any-of? @keyword.repeat "loop" "while" "every" "for" "each" "do"))

; `return`, `stop`, `exit` and `continue` only ever start a line.
(statement
  .
  (identifier) @keyword.return @keyword
  (#any-of? @keyword.return "return" "stop" "exit" "continue"))

; Comparison and boolean connectives. `loop-x` and `event-x` are separate tokens
; already, so colouring the bare word here cannot bleed into them.
((identifier) @keyword.operator @operator
  (#any-of? @keyword.operator
    "is" "are" "was" "were" "and" "or" "not" "contains"))

; The negated forms carry an apostrophe, so the grammar lexes them as `word`
; rather than `identifier`.
((word) @keyword.operator @operator
  (#match? @keyword.operator "^(isn't|aren't|wasn't|weren't|doesn't|don't)$"))

; -------------------------------------------------------- structural punctuation

; The `:` separating an entry key from its value, the `=` of a `variables:` or
; `aliases:` assignment, and a parameter list's separators. These are structural
; in Skript the way a comma in an argument list is; leaving them plain made
; `permission: skript.home` render as two unrelated fragments.
(entry ":" @punctuation.delimiter)

(assignment "=" @operator)

(parameter ":" @punctuation.delimiter)
(parameter "=" @operator)
(parameter_list "," @punctuation.delimiter)

; `amount: number = 1`, `xs: integers = (1, 7)`.
;
; The grammar keeps the value as one node on purpose: splitting `(1, 7)` into
; real literals collides with `parameter_list`'s own comma, so it is coloured
; whole rather than structured.
(default_value) @constant

(default_value
  [
    "("
    ")"
  ] @punctuation.bracket)

; ------------------------------------------------------- command entry values

; The literal-text entries of a command. Skript parses these as a
; `VariableString` rather than as code, so colouring them as text is a statement
; about the language, not a coverage trick.
;
; Nesting under `command` is what keeps them apart from an `options:` block,
; whose values are pasted in before parsing and genuinely may be code — an
; option may even be *named* `description`. `value:` repeats once per token, so
; the quantifier is required or only the first word colours.
((command
   body: (entry_body
     (entry
       key: (entry_key) @_entry_key
       value: (_)+ @string)))
  (#any-of? @_entry_key
    "description" "usage" "permission message" "cooldown message"
    "prefix" "aliases" "executable by"))

; An alias definition names item types rather than executing anything.
(aliases
  body: (assignment_body
    (assignment
      value: (_)+ @constant)))

; --------------------------------------------------------- skript-reflect
;
; skript-reflect lets a script call Java directly, and those calls are the one
; thing in a `.sk` file that is not Skript at all — no pattern in `docs.json` or
; SkriptHub describes `HashMap.put`, so the language server correctly says
; nothing about them and they were left colourless. In a reflect-heavy script
; that is most of the file.
;
; The grammar cannot classify them, but it does not need to: unlike Skript
; prose, Java interop has a *shape*. Both forms below occur exactly zero times
; across the 540 scripts Skript itself ships, so matching them cannot recolour
; ordinary Skript.
;
; `new HashMap()` needs nothing — the parser already reads `HashMap` as a
; `function_call`, which is captured above.
;
; Note `[.]` rather than `\.`: a backslash escape inside a query string is not
; processed the way it looks, and `"^\.[a-z]"` silently loses its anchor, so
; `Bukkit.getVersion` matched a rule meant only for `.getVersion`.

; `Material.DIAMOND`, `NamedTextColor.AQUA` — an enum constant reached through
; its class. Checked before the general form below, which would also match.
((word) @constant
  (#match? @constant "^[A-Z][A-Za-z0-9_]*[.][A-Z_][A-Z0-9_]*$"))

; `Bukkit.getVersion`, `UUID.randomUUID`, `Arrays.asList` — a static call or
; field on a named class.
((word) @type
  (#match? @type "^[A-Z][A-Za-z0-9_]*[.]"))

; `.put`, `.getItemMeta`, `.forEach` — a method on whatever precedes it, which
; is usually a variable: `{_item}.getItemMeta()`.
((word) @function.call
  (#match? @function.call "^[.][a-z]"))
