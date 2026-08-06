; Bracket matching and rainbow brackets.
;
; Only structural pairs are listed. Bare `(` / `)` inside statement prose are
; separate `punctuation` nodes with no nesting relationship in the tree — the
; grammar cannot pair them, because in Skript they are as likely to be part of
; an addon's English syntax as they are to be a grouping construct. Auto-closing
; those while you type is handled by `brackets` in config.toml instead.
;
; Skript's `{…}` is a variable sigil rather than a block delimiter and `%…%`
; brackets an interpolation; both pair usefully when a variable name is long.

(function_call
  "(" @open
  ")" @close)

(function
  "(" @open
  ")" @close)

(variable
  "{" @open
  "}" @close)

(option_ref
  "{@" @open
  "}" @close)

; String and interpolation delimiters pair, but rainbow-colouring them is noise.
((string
  "\"" @open
  "\"" @close)
  (#set! rainbow.exclude))

((interpolation
  "%" @open
  "%" @close)
  (#set! rainbow.exclude))
