# Theming

**No colour is defined anywhere in this extension.** Everything is expressed as
capture names and theme style names, so any theme you install works immediately.

## How Zed resolves a capture

`SyntaxTheme::highlight_id` walks a dotted capture name back along its
dot-prefixes until the theme defines one:

```
string.special.symbol  →  string.special  →  string
```

So a refinement is always safe *provided its root is common*. A **bare invented
name** has no fallback and renders unstyled — `@decorator` would simply not be
coloured. Where a refinement might be missing, this extension lists two captures
and lets Zed resolve them right-to-left as a fallback chain:

```scheme
(loop_value) @variable.builtin @variable
```

Zed tries `variable.builtin` first and falls back to `variable`.

## The capture map

| Skript | Capture |
|---|---|
| structure keywords (`command`, `function`, `on`, `options`…) | `@keyword` |
| event modifier and priority | `@keyword.modifier @keyword` |
| event name, command name, function name | `@function` (+ `.definition` / `.call`) |
| command entry keys (`permission:`, `description:`) | `@property` |
| `trigger:` | `@keyword` |
| types, `import` paths | `@type` |
| `{variable}` | `@variable` |
| `_` / `-` scope sigil | `@punctuation.special` |
| `{@option}`, aliases, experiments | `@constant` |
| `loop-player`, `event-block`, `arg-1` | `@variable.builtin @variable` |
| function parameters, command argument specs | `@variable.parameter @variable` |
| strings | `@string` |
| `""` `%%` escapes, `&6` colour codes | `@string.escape` |
| `<red>` `<#FF00AA>` format tags | `@string.special` |
| `%…%` delimiters | `@punctuation.special` |
| interpolation contents | `@embedded` |
| numbers, durations | `@number` |
| `true` / `false` | `@boolean` |
| `if` / `else` / `then` | `@keyword.conditional @keyword` |
| `loop` / `while` / `every` / `for` | `@keyword.repeat @keyword` |
| `return` / `stop` / `exit` / `continue` | `@keyword.return @keyword` |
| `is` / `and` / `or` / `not` / `contains` / `isn't` | `@keyword.operator @operator` |
| comments, `###` blocks | `@comment` |

Every root used here is defined by both Zed's bundled One theme (46 capture
names) and Catppuccin (101).

## What is deliberately *not* coloured

Ordinary statement prose. Skript has no fixed vocabulary — an effect is whatever
pattern an addon registered — so colouring the first word of every line would be
wrong as often as right. The language server classifies those lines instead and
delivers the result as semantic tokens.

`yes` / `no` / `on` / `off` are also left alone even though Skript accepts them
as booleans: `on` opens every event, and mis-colouring it is worse than leaving
it plain.

## Semantic tokens

**Zed's `semantic_tokens` setting defaults to `"off"`.** To get the full effect:

```json
{
  "languages": {
    "Skript": { "semantic_tokens": "combined" }
  }
}
```

`"combined"` overlays the server's classification on top of tree-sitter.
`"full"` replaces tree-sitter entirely — not recommended here, because the
grammar knows things about strings and variables the server does not repeat.

The extension ships `languages/skript/semantic_token_rules.json`, which Zed
ranks above its own defaults and below your settings. It maps the server's
token types onto **theme style names, never colours**:

```jsonc
{ "token_type": "skriptCondition", "style": ["keyword.conditional", "keyword"] }
```

The `style` array is a fallback chain — the first name the active theme defines
wins, so a minimal theme still gets `keyword`.

Deprecated syntax is struck through rather than recoloured, so it still reads as
whatever it is while being visibly on the way out.

### Overriding a rule

Your own rules take precedence over the extension's:

```json
{
  "global_lsp_settings": {
    "semantic_token_rules": [
      { "token_type": "skriptExpression", "style": ["variable.member", "property"] }
    ]
  }
}
```

To see what is actually being applied, run `dev: open highlights tree view` from
the command palette.

## Checking a theme

Open `examples/sample-project/showcase.sk` — it exercises every construct on
purpose. Worth confirming:

* strings, interpolation, format tags and `&6` codes are visibly distinct
* `{_local}`, `{global}` and `{@option}` read differently from each other
* comments and `###` blocks recede
* nothing is invisible against the background

If something is unstyled, the capture root is probably missing from that theme —
add a fallback capture rather than a colour.
