; Vim-mode text objects.
;
; Zed recognises exactly six capture names here: function.inside,
; function.around, class.inside, class.around, comment.inside and
; comment.around. Anything else is ignored.
;
; Skript's mapping onto them:
;   function.*  the callable / handler units — functions, events and commands,
;               plus any nested section, since `dif` inside a loop body is the
;               motion people actually reach for.
;   class.*     the top-level structure as a whole.

(function
  body: (_) @function.inside) @function.around

(event
  body: (_) @class.inside) @class.around

(command
  body: (_) @class.inside) @class.around

(options
  body: (_) @class.inside) @class.around

(variables
  body: (_) @class.inside) @class.around

(aliases
  body: (_) @class.inside) @class.around

(section
  body: (_) @function.inside) @function.around

(entry_section
  body: (_) @function.inside) @function.around

[
  (comment)
  (block_comment)
] @comment.around

(block_comment) @comment.inside
