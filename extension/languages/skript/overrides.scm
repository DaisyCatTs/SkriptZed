; Scope overrides.
;
; Every name used in a `not_in = [...]` entry in config.toml must be captured
; here, otherwise Zed refuses to load the language with
; "language ... has overrides in config not in query".
;
; `.inclusive` extends the scope to cover the node's delimiters, so typing a
; quote immediately after the closing `"` of a string is not treated as being
; inside it, while typing anywhere within `"…"` is.

[
  (comment)
  (block_comment)
] @comment.inclusive

(string) @string
