/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

/**
 * tree-sitter-skript
 *
 * Skript (https://github.com/SkriptLang/Skript) is a Minecraft server scripting
 * language with no context-free grammar: every effect, condition, expression,
 * event and section is a *pattern string* registered at runtime by Skript or an
 * addon, and matched against the line by `SkriptParser`. Core Skript 2.16 alone
 * registers 2,117 such patterns; addons register thousands more.
 *
 * Therefore this grammar deliberately models STRUCTURE, not SEMANTICS:
 *
 *   - lines, sections, and indentation (via the external scanner)
 *   - strings and their `%interpolation%` / `<format tags>` / `&c` colour codes
 *   - variables `{x}` `{_x}` `{-x}` `{x::*}` and options `{@x}`
 *   - comments, including `###` block comments and the `#-#` section suppressor
 *   - the structure headers that DO have a fixed shape: command, function,
 *     options, variables, aliases, using, auto reload
 *
 * It never tries to decide whether `set {_x} to 5` is an effect or whether
 * `player is op` is a condition — that is the language server's job, and the
 * result comes back to the editor as LSP semantic tokens.
 *
 * Two design notes that are easy to get wrong:
 *
 *  1. NEWLINE / INDENT / DEDENT come from the external scanner and are
 *     zero-width; the literal newline character is an ordinary extra.
 *
 *  2. `comment` is deliberately NOT in `extras`. Skript's lexer (see
 *     `Node.splitLine`) treats `#` as literal inside a string and inside a
 *     variable name, so a comment may only appear where a line can end. Putting
 *     it in `extras` makes tree-sitter's longest-match lexer swallow the rest
 *     of the line at the first `#` in `"item #1"` or `{count##}`.
 *
 * See ../docs/grammar.md.
 */

// Characters that terminate a bare word. Everything else is word material,
// because Skript syntax is English prose. `<` and `>` are excluded so they stay
// available as the comparison operators and as `<command argument>` delimiters.
const WORD_CHAR = /[^\s"{}%#:,()\[\]<>&]/;

// A word character that `identifier` would NOT accept. `word` requires at least
// one of these, which is what keeps the two tokens from overlapping:
// tree-sitter compares token precedence BEFORE match length, so two tokens that
// can match the same text must never both be candidates. With this split,
// `player` is only ever an identifier and `skript.home` is only ever a word.
const NON_IDENT_CHAR = /[^\s"{}%#:,()\[\]<>&a-zA-Z0-9_À-￿]/;

module.exports = grammar({
  name: 'skript',

  externals: $ => [
    $._newline,
    $._indent,
    $._dedent,
    $._section_colon,
    $.block_comment,
    // Declared external so the scanner runs before layout-relevant tokens —
    // that is what lets it keep emitting DEDENT during error recovery. The
    // scanner never returns it; the internal `comment` token below matches.
    $.comment,
    // Never emitted. Its presence in `valid_symbols` tells the scanner that
    // tree-sitter is in error recovery.
    $._error_sentinel,
  ],

  extras: _ => [/[ \t\r\f\n]/],

  word: $ => $.identifier,

  // A comment after a section colon (`on join: # note`) is ambiguous with a
  // standalone comment that follows a body-less section. Both readings are
  // legal; GLR settles it on the next token (an INDENT means the section had a
  // body after all).
  conflicts: $ => [
    [$.command],
    [$.function],
    [$.options],
    [$.variables],
    [$.aliases],
    [$.import],
    [$.event],
    [$.section],
    [$.entry_section],
  ],

  supertypes: $ => [
    $._structure,
    $._content_item,
  ],

  rules: {
    source_file: $ => repeat($._top_level),

    _top_level: $ => choice($._structure, $.comment, $.block_comment),

    // ---------------------------------------------------------------- structures

    _structure: $ => choice(
      $.command,
      $.function,
      $.options,
      $.variables,
      $.aliases,
      $.import,
      $.using,
      $.auto_reload,
      $.event,
    ),

    // `command /home <text> [<text>]:` — the leading slash is optional.
    // The pattern after the name is free-form: it may contain literal brackets,
    // quotes and alternations as well as `<argument>` slots.
    command: $ => seq(
      alias('command', $.keyword),
      field('name', $.command_name),
      repeat(choice(
        field('argument', $.command_argument),
        $._content_item,
      )),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.entry_body)),
    ),

    command_name: _ => token(prec(2, seq(optional('/'), /[^\s<>:#\[\]"]+/))),

    // `<text>`, `<name: text>`, `<text = default>`, `<name: text = default>`.
    // The delimiters carry an explicit precedence so they beat the `<` / `>`
    // comparison operators, which Skript genuinely uses (`while {_x} < 5:`).
    // Because that only applies in the command-header state, `usage: /home
    // <name>` still lexes `<` as an operator.
    //
    // The inside is kept as one token rather than being split into
    // name/type/default: `<x:number>` and `<text="success 2">` are both legal
    // and pulling them apart in the grammar needs lookahead that LR does not
    // have. The language server splits the spec when it indexes the command.
    command_argument: $ => seq(
      alias(token(prec(3, '<')), $.punctuation),
      optional(field('spec', alias(token(prec(1, /[^<>\n]+/)), $.argument_spec))),
      alias(token(prec(3, '>')), $.punctuation),
    ),

    // `function name(a: text, b: number = 1) :: item:`
    // All three return markers are live in Skript: `::`, `->`, and ` returns `.
    function: $ => seq(
      optional(alias('local', $.keyword)),
      alias('function', $.keyword),
      // An option may stand in for any part of the header — Skript substitutes
      // `{@…}` before parsing, so `function {@sig}) :: boolean:` is legal. The
      // parentheses are therefore optional rather than required; a header that
      // is genuinely malformed is reported by the language server, not by a
      // parse error that would poison the whole file's outline and folding.
      field('name', choice($.identifier, $.option_ref)),
      optional('('),
      optional(field('parameters', $.parameter_list)),
      optional(')'),
      optional(seq(
        alias(choice('::', '->', 'returns'), $.return_marker),
        field('return_type', alias(/[^:#\n]+/, $.type)),
      )),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.block)),
    ),

    parameter_list: $ => seq($.parameter, repeat(seq(',', $.parameter))),

    parameter: $ => seq(
      field('name', $.identifier),
      ':',
      field('type', alias(/[^,)=]+/, $.type)),
      optional(seq('=', field('default', $.default_value))),
    ),

    // Defaults may be list literals: `(xs: integers = (1, 7))`.
    default_value: _ => repeat1(choice(
      token(/[^,()]+/),
      seq('(', optional(token(/[^()]+/)), ')'),
    )),

    options: $ => seq(
      alias('options', $.keyword),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.entry_body)),
    ),

    variables: $ => seq(
      alias('variables', $.keyword),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.assignment_body)),
    ),

    aliases: $ => seq(
      alias('aliases', $.keyword),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.assignment_body)),
    ),

    // skript-reflect's `import:` section; its body is Java type names.
    import: $ => seq(
      alias('import', $.keyword),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.import_body)),
    ),

    import_body: $ => seq(
      $._indent,
      repeat1(choice(
        seq(alias(/[^\s#][^#\n]*/, $.import_path), optional($.comment), $._newline),
        $.comment,
      )),
      $._dedent,
    ),

    // `using script reflection` — a bodyless top-level structure.
    using: $ => seq(
      alias('using', $.keyword),
      field('feature', alias(/[^#\n]+/, $.experiment)),
      optional($.comment),
      $._newline,
    ),

    auto_reload: $ => seq(
      alias(/auto(matically)?[ \t]+reload/, $.keyword),
      optional(alias(/(this|the)[ \t]+script/, $.keyword)),
      optional($.comment),
      $._newline,
    ),

    // Any other top-level section is an event: `on join:`, `first join:`,
    // `on damage of player with priority high:`.
    // Skript registers this as
    //   `[on] [uncancelled|cancelled|(any|all)] <.+> [with priority …]`
    // so the modifier follows `on` rather than preceding it.
    event: $ => seq(
      optional(alias('on', $.keyword)),
      optional(field('modifier', $.event_modifier)),
      field('name', $._content),
      optional(field('priority', $.event_priority)),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.block)),
    ),

    event_modifier: _ => token(prec(2, /(uncancelled|cancelled|any|all)[ \t]+/)),

    event_priority: _ => token(prec(2, /with[ \t]+priority[ \t]+(lowest|low|normal|high|highest|monitor)/)),

    // ------------------------------------------------------------------ bodies

    // A generic indented block: triggers, if/else, loops, while, addon sections.
    block: $ => seq(
      $._indent,
      repeat1($._block_item),
      $._dedent,
    ),

    _block_item: $ => choice($.section, $.statement, $.comment, $.block_comment),

    // Any line ending in a section colon opens a nested block. We do NOT
    // enumerate section keywords: any addon can register an effect-section.
    section: $ => seq(
      field('header', $._content),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.block)),
    ),

    statement: $ => seq($._content, optional($.comment), $._newline),

    // `key: value` bodies — command entries, `options:`, Skript config files.
    entry_body: $ => seq(
      $._indent,
      repeat1(choice($.entry, $.entry_section, $.section, $.statement, $.comment, $.block_comment)),
      $._dedent,
    ),

    entry: $ => seq(
      field('key', $.entry_key),
      ':',
      optional(field('value', $._content)),
      optional($.comment),
      $._newline,
    ),

    // Nested `key:` sections — legal in `options:` since Skript 2.7, the shape
    // of Skript's own config.sk, and how `trigger:` sits inside a command.
    entry_section: $ => seq(
      field('key', $.entry_key),
      $._section_colon,
      optional($.comment),
      optional(field('body', $.block)),
    ),

    entry_key: _ => token(prec(1, /[A-Za-z_][A-Za-z0-9 _.\-]*/)),

    // `variables:` and `aliases:` bodies use `=`, not `:`.
    assignment_body: $ => seq(
      $._indent,
      repeat1(choice($.assignment, $.statement, $.comment, $.block_comment)),
      $._dedent,
    ),

    // Higher precedence than a bare statement so that `{score} = 100` inside a
    // `variables:` block is an assignment rather than content containing an
    // `=` operator.
    assignment: $ => prec(1, seq(
      field('target', choice($.variable, alias(token(prec(1, /[A-Za-z_][^=#\n]*/)), $.alias_name))),
      '=',
      field('value', $._content),
      optional($.comment),
      $._newline,
    )),

    // ----------------------------------------------------------------- content

    _content: $ => repeat1($._content_item),

    _content_item: $ => choice(
      $.string,
      $.variable,
      $.option_ref,
      $.function_call,
      $.duration,
      $.number,
      $.boolean,
      $.loop_value,
      $.event_value,
      $.command_arg_ref,
      $.identifier,
      $.word,
      $.operator,
      $.punctuation,
    ),

    // `giveApple("Banana", 4)` — the paren must be immediate.
    function_call: $ => prec(2, seq(
      field('name', $.identifier),
      token.immediate('('),
      optional(field('arguments', $._content)),
      ')',
    )),

    // ------------------------------------------------------------------ atoms

    string: $ => seq(
      '"',
      repeat(choice(
        $.escape_sequence,
        $.interpolation,
        $.format_tag,
        $.legacy_color,
        $._string_text,
      )),
      '"',
    ),

    // Skript escapes by doubling: `""` is a literal quote, `%%` a literal percent.
    escape_sequence: _ => token(prec(3, choice('""', '%%'))),

    // `%player's tool%`. Inside the percent signs Skript switches back to code
    // context, so a nested string is legal: `"%uuid of world "world"%"`.
    interpolation: $ => seq(
      '%',
      repeat(choice(
        $.string,
        $.variable,
        $.option_ref,
        alias(token(prec(-1, /[^%"{}\n]+/)), $.interpolation_text),
      )),
      '%',
    ),

    // `<red>`, `<bold>`, `<#FF00AA>`, `<link:https://…>`, `<tooltip:hi>`
    format_tag: _ => token(prec(2, seq('<', /[^<>%"\n]*/, '>'))),

    // Legacy `&6` codes and `&#RRGGBB` hex colours.
    legacy_color: _ => token(prec(2, choice(
      seq('&', /[0-9a-fk-orA-FK-OR]/),
      seq('&#', /[0-9a-fA-F]{6}/),
    ))),

    _string_text: _ => token(prec(-1, choice(/[^"%<&\n]+/, '<', '&'))),

    // `{x}` global · `{_x}` local · `{-x}` ephemeral · `{x::*}` list
    variable: $ => seq(
      '{',
      optional(field('scope', $.variable_scope)),
      optional(field('name', $.variable_name)),
      '}',
    ),

    variable_scope: _ => token.immediate(choice('_', '-')),

    // A variable name may nest another variable — `{_test::{_x}}` is legal —
    // and may interpolate — `{home::%uuid of player%}`.
    variable_name: $ => repeat1(choice(
      $.interpolation,
      $.variable,
      $.list_separator,
      alias(token(prec(-1, /[^{}%:\n]+/)), $.variable_text),
      alias(':', $.variable_text),
    )),

    list_separator: _ => token(prec(2, '::')),

    // `{@my option}` — compile-time option substitution, not a variable.
    option_ref: $ => seq(
      token(prec(3, '{@')),
      field('name', alias(/[^{}\n]+/, $.option_name)),
      '}',
    ),

    // `5 seconds`, `2 ticks`, `1.5 minutes`
    duration: _ => token(prec(2, /\d+(\.\d+)?[ \t]+(tick|second|minute|hour|day|week|month|year)s?/)),

    number: _ => token(prec(1, /-?\d+(\.\d+)?/)),

    // Plain string alternatives so that tree-sitter's keyword extraction (see
    // the `word` property above) applies: `truely` lexes as an identifier and
    // never as `true` + `ly`.
    // `on`/`off`/`yes`/`no` are deliberately excluded: `on` opens every event,
    // and mis-colouring them is worse than not colouring them.
    boolean: _ => choice('true', 'false'),

    // `loop-player`, `loop-value-2`, `loop-index`
    loop_value: _ => token(prec(2, /loop-[a-zA-Z][a-zA-Z0-9\-]*/)),

    // `event-block`, `event-player`
    event_value: _ => token(prec(2, /event-[a-zA-Z][a-zA-Z0-9\-]*/)),

    // `arg-1`, `arg-text`, `argument-2`
    command_arg_ref: _ => token(prec(2, /arg(ument)?-[a-zA-Z0-9]+/)),

    identifier: _ => /[a-zA-Z_À-￿][a-zA-Z0-9_À-￿]*/,

    // Anything else that is not whitespace or structural punctuation. Must
    // contain at least one non-identifier character — see NON_IDENT_CHAR.
    word: _ => token(seq(repeat(WORD_CHAR), NON_IDENT_CHAR, repeat(WORD_CHAR))),

    // Plain strings, no precedence: a one-character operator and a
    // one-character word tie on length, and tree-sitter then prefers the
    // string token, so `5 - 3` yields an operator while `non-persistent`
    // stays a single word.
    operator: _ => choice('+', '-', '*', '/', '^', '=', '!', '&', '<', '>'),

    punctuation: _ => choice(',', '(', ')', '[', ']', ':', '%'),

    // `# comment`. `##` is an escaped literal hash and does not open one, but
    // that only matters inside strings and variable names — and this token is
    // not an `extra`, so it is never reachable there.
    comment: _ => token(seq('#', /[^\n]*/)),
  },
});
