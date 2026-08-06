; Zed's outline panel, breadcrumbs and `editor: toggle outline`.
;
; Zed requires both @item and @name on every pattern — if either is missing the
; whole query is dropped with an error. @context adds the words shown before the
; name (the `on`, `command`, `function` keyword), and @annotation attaches a
; preceding doc comment to the entry.

(comment) @annotation

; `on join:` / `first join:`
(event
  (keyword)? @context
  (event_modifier)? @context
  name: (identifier) @name) @item

; `command /home <text>:`
(command
  (keyword) @context
  name: (command_name) @name) @item

; `function give_apple(name: text) :: item:`
(function
  (keyword) @context
  (keyword)? @context
  name: (identifier) @name) @item

(function
  (keyword) @context
  (keyword)? @context
  name: (option_ref) @name) @item

; The structure keyword is the whole name for these.
(options
  (keyword) @name) @item

(variables
  (keyword) @name) @item

(aliases
  (keyword) @name) @item

(import
  (keyword) @name) @item

(using
  (keyword) @context
  feature: (experiment) @name) @item

; Command entries — `permission:`, `trigger:`, `cooldown:` — and the keys of an
; `options:` block, so the outline mirrors the file's real shape.
(entry_section
  key: (entry_key) @name) @item

(entry
  key: (entry_key) @name) @item

; A named variable default in a `variables:` block.
(assignment
  target: (variable) @name) @item

(assignment
  target: (alias_name) @name) @item

; Nested sections inside a trigger: `if`, `loop`, `while` and any addon section.
; Only the leading word is used as the name so the outline stays readable.
(section
  header: (identifier) @name) @item
