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
(event
  name: [
    (identifier)
    (word)
  ] @function)

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

(variable) @variable

; `_` marks a local and `-` an ephemeral variable; both are scope sigils.
(variable_scope) @punctuation.special

(variable
  name: (variable_name
    (variable_text) @variable))

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

(escape_sequence) @string.escape

; `<red>`, `<bold>`, `<#FF00AA>`, `<link:…>` inside a string.
(format_tag) @string.special

; Legacy `&6` colour codes and `&#RRGGBB`.
(legacy_color) @string.escape

; `%…%` re-enters code context inside a string, so its contents read as code and
; its delimiters as the boundary between the two.
(interpolation
  "%" @punctuation.special)

(interpolation
  (interpolation_text) @embedded)

; ---------------------------------------------------------------- control flow

; The only bare English words this file colours. Each of these is structural in
; Skript and cannot be redefined by an addon, and each is restricted to the
; position where it can only mean the control-flow construct.

(section
  header: (identifier) @keyword.conditional @keyword
  (#any-of? @keyword.conditional "if" "else" "then"))

(section
  header: (identifier) @keyword.repeat @keyword
  (#any-of? @keyword.repeat "loop" "while" "every" "for" "each"))

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
