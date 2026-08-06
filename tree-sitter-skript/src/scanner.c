// External scanner for tree-sitter-skript.
//
// Skript is an indentation-delimited language: a line ending in `:` opens a
// section, and the section body is everything indented one level deeper. There
// is no `end` keyword and there are no braces. This scanner turns that layout
// into INDENT / DEDENT / NEWLINE tokens, the same way tree-sitter-python and
// tree-sitter-gdscript do.
//
// It also owns three Skript-specific lexical decisions that the context-free
// grammar cannot make:
//
//   SECTION_COLON  A `:` only opens a section when the rest of the line is
//                  whitespace and/or a comment. `permission: skript.foo` is an
//                  entry, not a section. A line carrying the `#-#` marker is
//                  explicitly NOT a section, even though it ends in `:`.
//
//   BLOCK_COMMENT  A line whose trimmed content is exactly `###` toggles a
//                  block comment (Skript 2.9+). `### foo` is an ordinary
//                  comment. The whole region is emitted as one token.
//
//   COMMENT        Declared external only so that this scanner runs before
//                  every token, which is what lets it keep emitting DEDENT
//                  during error recovery. It is never returned from here; the
//                  grammar's internal `comment` token matches instead.
//
// INVARIANTS (violating any of these corrupts incremental reparsing):
//   * `mark_end` is called once, before the whitespace loop, so NEWLINE /
//     INDENT / DEDENT are zero-width and the lexer position rewinds after each.
//     That rewind is what allows several DEDENTs to be emitted at one position.
//   * Only one INDENT or DEDENT is emitted per call.
//   * The indent stack is seeded with a `0` sentinel that is never serialized.
//   * `serialize` must never write past TREE_SITTER_SERIALIZATION_BUFFER_SIZE.
//   * `deserialize` is called with (NULL, 0) to reset and must fully
//     reinitialise rather than append.

#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"

// Must match the order of `externals` in grammar.js.
enum TokenType {
  NEWLINE,
  INDENT,
  DEDENT,
  SECTION_COLON,
  BLOCK_COMMENT,
  COMMENT,
  ERROR_SENTINEL,
};

// A tab counts as 8 columns and a space as 1. Skript infers a single indent
// unit per file, so any monotonic measure works; this one matches Python's and
// keeps comparisons stable for both tab- and space-indented scripts.
#define TAB_WIDTH 8

typedef struct {
  Array(uint16_t) indents;
} Scanner;

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }
static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

static inline bool is_horizontal_space(int32_t c) {
  return c == ' ' || c == '\t' || c == '\r' || c == '\f';
}

// Consumes the rest of a block comment, starting just after the opening `###`
// line's hashes have been advanced over. Stops after the closing `###` line, or
// at EOF if the comment is never closed (Skript reports that as an error; we
// still produce a well-formed tree).
static bool consume_block_comment_body(TSLexer *lexer) {
  for (;;) {
    // Skip to the end of the current line.
    while (lexer->lookahead != '\n' && !lexer->eof(lexer)) {
      advance(lexer);
    }
    if (lexer->eof(lexer)) {
      lexer->mark_end(lexer);
      return true;
    }
    advance(lexer); // the newline

    // Measure this line: leading whitespace, then content.
    while (is_horizontal_space(lexer->lookahead)) {
      advance(lexer);
    }

    uint8_t hashes = 0;
    while (lexer->lookahead == '#') {
      hashes++;
      advance(lexer);
    }
    if (hashes != 3) {
      continue;
    }
    // A closing delimiter only if nothing but whitespace follows.
    while (is_horizontal_space(lexer->lookahead)) {
      advance(lexer);
    }
    if (lexer->lookahead == '\n' || lexer->eof(lexer)) {
      lexer->mark_end(lexer);
      return true;
    }
  }
}

// `:` that terminates a section header.
static bool scan_section_colon(TSLexer *lexer) {
  advance(lexer);
  lexer->mark_end(lexer); // the token is exactly ":"; everything below is lookahead

  while (is_horizontal_space(lexer->lookahead)) {
    advance(lexer);
  }

  if (lexer->lookahead == '#') {
    advance(lexer);
    if (lexer->lookahead == '-') {
      advance(lexer);
      if (lexer->lookahead == '#') {
        // `#-#` — Skript's explicit "this colon does not open a section" marker.
        return false;
      }
    }
    lexer->result_symbol = SECTION_COLON;
    return true;
  }

  if (lexer->lookahead == '\n' || lexer->eof(lexer)) {
    lexer->result_symbol = SECTION_COLON;
    return true;
  }

  return false;
}

bool tree_sitter_skript_external_scanner_scan(void *payload, TSLexer *lexer,
                                             const bool *valid_symbols) {
  Scanner *scanner = (Scanner *)payload;
  bool error_recovery = valid_symbols[ERROR_SENTINEL];

  if (valid_symbols[SECTION_COLON] && !error_recovery && lexer->lookahead == ':') {
    return scan_section_colon(lexer);
  }

  // Everything below is zero-width lookahead: mark the end first so that
  // emitting a token rewinds the lexer to exactly here.
  lexer->mark_end(lexer);

  bool at_column_zero = lexer->get_column(lexer) == 0;
  bool found_end_of_line = false;
  uint16_t indent_length = 0;
  int32_t first_comment_indent_length = -1;
  bool block_comment_ahead = false;

  for (;;) {
    if (lexer->lookahead == '\n') {
      found_end_of_line = true;
      indent_length = 0;
      skip(lexer);
    } else if (lexer->lookahead == ' ') {
      indent_length++;
      skip(lexer);
    } else if (lexer->lookahead == '\t') {
      indent_length += TAB_WIDTH;
      skip(lexer);
    } else if (lexer->lookahead == '\r' || lexer->lookahead == '\f') {
      skip(lexer);
    } else if (lexer->lookahead == '#') {
      // A comment that trails real code is just a token — let the internal
      // lexer have it, and do not disturb the indent stack.
      if (!found_end_of_line && !at_column_zero) {
        return false;
      }
      // A standalone comment line. Remember the indent of the first one so a
      // comment that is outdented relative to the block does not trigger a
      // premature DEDENT (see the guard further down).
      if (first_comment_indent_length == -1) {
        first_comment_indent_length = (int32_t)indent_length;
      }
      block_comment_ahead = true;
      break;
    } else if (lexer->eof(lexer)) {
      // EOF drains the whole indent stack.
      indent_length = 0;
      found_end_of_line = true;
      break;
    } else {
      break;
    }
  }

  if (found_end_of_line && scanner->indents.size > 0) {
    uint16_t current_indent_length = *array_back(&scanner->indents);

    if (valid_symbols[INDENT] && indent_length > current_indent_length) {
      array_push(&scanner->indents, indent_length);
      lexer->result_symbol = INDENT;
      return true;
    }

    if (valid_symbols[DEDENT] && indent_length < current_indent_length &&
        // Hold the DEDENT back until we have passed any comment lines that are
        // still indented at least as far as the block we are inside.
        first_comment_indent_length < (int32_t)current_indent_length) {
      array_pop(&scanner->indents);
      lexer->result_symbol = DEDENT;
      return true;
    }
  }

  if (found_end_of_line && valid_symbols[NEWLINE] && !error_recovery) {
    lexer->result_symbol = NEWLINE;
    return true;
  }

  // No layout token was due. If a standalone comment line is ahead and it is a
  // bare `###`, take the whole block comment as one token. Anything else falls
  // through to the internal lexer, which produces an ordinary `comment`.
  if (block_comment_ahead && valid_symbols[BLOCK_COMMENT]) {
    uint8_t hashes = 0;
    while (lexer->lookahead == '#') {
      hashes++;
      advance(lexer);
    }
    if (hashes == 3) {
      while (is_horizontal_space(lexer->lookahead)) {
        advance(lexer);
      }
      if (lexer->lookahead == '\n' || lexer->eof(lexer)) {
        if (consume_block_comment_body(lexer)) {
          lexer->result_symbol = BLOCK_COMMENT;
          return true;
        }
      }
    }
  }

  return false;
}

unsigned tree_sitter_skript_external_scanner_serialize(void *payload, char *buffer) {
  Scanner *scanner = (Scanner *)payload;
  size_t size = 0;

  // indents[0] is the implicit 0 sentinel and is re-seeded by deserialize.
  for (uint32_t i = 1; i < scanner->indents.size &&
                       size + 2 <= TREE_SITTER_SERIALIZATION_BUFFER_SIZE;
       i++) {
    uint16_t indent_value = *array_get(&scanner->indents, i);
    buffer[size++] = (char)(indent_value & 0xFF);
    buffer[size++] = (char)((indent_value >> 8) & 0xFF);
  }

  return (unsigned)size;
}

void tree_sitter_skript_external_scanner_deserialize(void *payload, const char *buffer,
                                                     unsigned length) {
  Scanner *scanner = (Scanner *)payload;

  array_delete(&scanner->indents);
  array_init(&scanner->indents);
  array_push(&scanner->indents, 0);

  for (unsigned size = 0; size + 1 < length; size += 2) {
    uint16_t indent_value =
        (uint16_t)((unsigned char)buffer[size] | ((unsigned char)buffer[size + 1] << 8));
    array_push(&scanner->indents, indent_value);
  }
}

void *tree_sitter_skript_external_scanner_create(void) {
  Scanner *scanner = (Scanner *)ts_calloc(1, sizeof(Scanner));
  array_init(&scanner->indents);
  tree_sitter_skript_external_scanner_deserialize(scanner, NULL, 0);
  return scanner;
}

void tree_sitter_skript_external_scanner_destroy(void *payload) {
  Scanner *scanner = (Scanner *)payload;
  array_delete(&scanner->indents);
  ts_free(scanner);
}
