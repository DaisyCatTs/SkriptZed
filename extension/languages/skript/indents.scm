; Indentation anchors.
;
; Skript's block indentation is driven by `increase_indent_pattern` and
; `decrease_indent_patterns` in config.toml, NOT from this file. That is
; deliberate: Zed's `Buffer::suggest_autoindents` throws away tree-sitter indent
; suggestions that fall inside an ERROR range but keeps regex-derived ones
; (`within_error && !from_regex`), and a Skript block is a syntax error for as
; long as you are still typing its header. Zed's own YAML ships no indents.scm
; at all for the same reason.
;
; This file therefore does two much smaller jobs:
;
;   1. `@indent` / `@end` for bracketed constructs that may span lines.
;   2. `@start.<suffix>` anchors, which are the targets that
;      `decrease_indent_patterns[].valid_after` in config.toml matches against.
;      They are what makes a typed `else:` snap back to the column of its `if`
;      rather than merely losing one level.

(_
  "("
  ")" @end) @indent

(_
  "["
  "]" @end) @indent

; Anchors for `valid_after`. A section whose header begins with `if` is the
; basis row that a later `else` / `else if` aligns to.
((section
  header: (identifier) @_kw)
  (#eq? @_kw "if")) @start.if

((section
  header: (identifier) @_kw)
  (#eq? @_kw "else")) @start.else-if

((section
  header: (identifier) @_kw)
  (#any-of? @_kw "loop" "while" "every" "for")) @start.loop
