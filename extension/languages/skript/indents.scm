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

; There is deliberately no `@start.loop` anchor. An anchor is only reachable
; through a `valid_after` in config.toml, and Skript has no loop-else and no
; construct that dedents relative to a loop header — so nothing can ever name
; one. The anchor that used to sit here was inert, and inert-but-plausible is
; how a future reader gets talked into inventing a decrease pattern with no
; referent.
