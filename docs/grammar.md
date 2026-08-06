# The grammar

`tree-sitter-skript` models Skript's **structure**. It never decides whether a
statement is an effect, a condition or an expression — see
[architecture.md](architecture.md) for why that is impossible in a grammar.

## Node reference

| Node | Shape |
|---|---|
| `event` | `[on] [cancelled\|…] <name> [with priority …]:` — the modifier follows `on` |
| `command` | `command [/]name <arg>… :` with `command_argument` slots |
| `function` | name, `parameter_list`, `return_marker` (`::`, `->`, `returns`), `type` |
| `options` `variables` `aliases` `import` | keyword + body |
| `using` `auto_reload` | bodyless, newline-terminated |
| `section` | any line ending in a section colon, plus its indented block |
| `statement` | any other line |
| `entry` / `entry_section` | `key: value` and `key:` + block — command entries, `options:`, config files |
| `assignment` | `{var} = value` / `alias = items` inside `variables:` and `aliases:` |
| `string` | `escape_sequence` (`""`, `%%`), `interpolation`, `format_tag`, `legacy_color` |
| `variable` | `variable_scope` (`_`/`-`), `variable_name`, `list_separator` (`::`) |
| `option_ref` | `{@name}` |
| `loop_value` `event_value` `command_arg_ref` | `loop-player`, `event-block`, `arg-1` |
| `number` `boolean` `duration` | `5`, `-2.5`; `true`/`false`; `5 seconds` |
| `comment` `block_comment` | `# …` and a `###`-delimited region |

## The external scanner

`src/scanner.c` — **C, never C++**. Zed compiles `src/scanner.c` and silently
ignores `scanner.cc`/`.cpp`, so a C++ scanner produces a grammar with missing
symbols at runtime and no error at build time.

It emits five tokens:

| Token | Meaning |
|---|---|
| `_newline` `_indent` `_dedent` | Layout, from the indentation stack |
| `_section_colon` | A `:` that actually opens a section |
| `block_comment` | A whole `###` … `###` region |

### Invariants

Breaking any of these corrupts incremental reparsing, usually intermittently:

* `mark_end` is called **once**, before the whitespace loop, so layout tokens
  are zero-width and the lexer rewinds after each. That rewind is what lets
  several `DEDENT`s be emitted at one position, and it makes the whole
  whitespace loop pure lookahead.
* Only **one** `INDENT` or `DEDENT` per `scan()` call.
* The indent stack is seeded with a `0` sentinel that is never serialized;
  `deserialize` re-seeds it and must fully reinitialise, because it is called
  with `(NULL, 0)` to reset.
* `serialize` must never write past `TREE_SITTER_SERIALIZATION_BUFFER_SIZE`.
  Zed had to fork `tree-sitter-markdown` over exactly this overflow.

### `_section_colon`

A `:` opens a section only when the rest of the line is whitespace and/or a
comment — otherwise `permission: skript.home` would open one. And it does *not*
open a section when the line carries Skript's `#-#` marker:

```sk
send "ratio 3:" to player #-#     # a statement, despite the trailing colon
```

The scanner advances past the `:`, marks the token end there, then looks ahead
over the rest of the line. If the lookahead fails it returns `false` and
tree-sitter resets the position, so the `:` is lexed as ordinary punctuation.

### Block comments

A line whose *trimmed* content is exactly `###` toggles a block comment
(Skript 2.9+). `### foo` is an ordinary comment. The whole region becomes one
token, which keeps folding and highlighting simple.

## Token model traps

**Precedence beats match length.** tree-sitter compares token precedence
*before* length, so two tokens that can match the same text must never both be
candidates. `word` therefore requires at least one non-identifier character;
without that split, `identifier` (precedence 0, 6 chars) beat `word` on
`skript.home` and produced `skript` + `.home`.

**Keyword extraction.** `boolean` is `choice('true','false')` as plain strings,
not `token(prec(…))`, so tree-sitter's keyword extraction — enabled by
`word: $ => $.identifier` — applies and `truely` never matches `true`.

**`comment` is not in `extras`.** Skript treats `#` as literal inside a string
and inside a variable name (`Node.splitLine`). With `comment` in `extras`, the
longest-match lexer swallows the rest of the line at the first `#` in
`"item #1"` or `{count##}`. It is declared external only so the scanner runs
before layout-relevant tokens; the internal token is what actually matches.

**`<` is both.** Skript uses `<` as a comparison operator *and* as a
command-argument delimiter. `command_argument` gives its delimiters an explicit
precedence, which applies only in the command-header state — so
`while {_x} < 5:` and `usage: /home <name>` both lex `<` as an operator.

## Deliberate permissiveness

A parse error poisons outline, folding and indentation for the *whole file*, so
the grammar accepts more than Skript does and lets the language server report
the real problem. The clearest case is the function header: the parentheses are
optional, because Skript expands `{@option}` before parsing and
`function {@sig}) :: boolean:` is legal input.

Skript's stricter rules — one indent unit per file, no mixing tabs and spaces,
exact multiples — are enforced as **diagnostics**, not as parse failures.

## Testing

```sh
cd tree-sitter-skript
./node_modules/.bin/tree-sitter test                       # 33 corpus tests
./node_modules/.bin/tree-sitter test -f "Block comment"    # one by name
node ../scripts/parse-corpus.mjs --strict                  # 540 real files
```

The corpus gate parses every `.sk` file in SkriptLang's own repository. It is
maintained upstream by the people who define the language, which makes it a far
better test than anything written here — it caught the `#`-in-strings bug, the
`<`-as-operator bug, and the nested-string-in-interpolation case.

## Regenerating

`src/parser.c`, `src/grammar.json`, `src/node-types.json` and
`src/tree_sitter/*.h` are generated but **committed**. Zed clones the grammar at
a pinned revision and runs clang on `parser.c` directly; it never runs
`tree-sitter generate`. CI fails if the committed output is stale.

```sh
./node_modules/.bin/tree-sitter generate   # after ANY change to grammar.js
```

Generate with `tree-sitter-cli` 0.25.x–0.26.x, which emits ABI 15. Zed pins
`tree-sitter = 0.26.9` and accepts ABI 13–15.
