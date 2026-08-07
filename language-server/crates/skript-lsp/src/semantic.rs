//! Semantic tokens — the payoff for everything else in this workspace.
//!
//! The tree-sitter grammar cannot tell an effect from a condition from an
//! expression, because all three are runtime-registered patterns. So
//! `highlights.scm` leaves statement prose uncoloured, and this module supplies
//! the classification instead: each executable line is matched against the
//! catalog, and the *literal* parts of the winning pattern are emitted as a
//! token of that syntax's category.
//!
//! Only the literals are coloured. The `%…%` slots are left alone so that the
//! expressions inside them keep their own tree-sitter colours — `{_x}` stays a
//! variable and `"hi"` stays a string, exactly as a reader expects.
//!
//! The token type names here must match
//! `extension/languages/skript/semantic_token_rules.json`.

use skript_docs::{Catalog, Category};
use skript_index::{FileSymbols, SymbolKind};
use tower_lsp::lsp_types as lsp;

/// The token types this server emits, in the order the LSP legend indexes them.
pub const TOKEN_TYPES: &[&str] = &[
    "skriptEffect",
    "skriptCondition",
    "skriptExpression",
    "skriptEvent",
    "skriptSection",
    "skriptStructure",
    "skriptType",
    "function",
];

pub const TOKEN_MODIFIERS: &[&str] = &["deprecated", "defaultLibrary"];

const MODIFIER_DEPRECATED: u32 = 1 << 0;
const MODIFIER_DEFAULT_LIBRARY: u32 = 1 << 1;

pub fn legend() -> lsp::SemanticTokensLegend {
    lsp::SemanticTokensLegend {
        token_types: TOKEN_TYPES
            .iter()
            .map(|name| lsp::SemanticTokenType::new(name))
            .collect(),
        token_modifiers: TOKEN_MODIFIERS
            .iter()
            .map(|name| lsp::SemanticTokenModifier::new(name))
            .collect(),
    }
}

fn token_type_index(category: Category) -> Option<u32> {
    let name = category.semantic_token_type();
    TOKEN_TYPES
        .iter()
        .position(|candidate| *candidate == name)
        .map(|index| index as u32)
}

/// One classified span, before delta encoding.
#[derive(Debug, Clone, Copy)]
struct Token {
    line: u32,
    /// Byte column, converted by the caller.
    start: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

/// Classifies every executable line of `text` and returns LSP semantic tokens.
///
/// `to_utf16` converts a (line, byte column) pair into the column the client
/// expects; the caller owns encoding negotiation.
pub fn tokens(
    catalog: &Catalog,
    text: &str,
    symbols: &FileSymbols,
    mut to_column: impl FnMut(u32, u32) -> u32,
) -> Vec<lsp::SemanticToken> {
    let mut tokens = Vec::new();
    // A `###` block's interior is prose. Classifying it would paint arbitrary
    // English as syntax, and it is what `skript-format` and the indentation
    // diagnostics already treat as verbatim — the three must agree.
    let mut in_block_comment = false;

    for (number, raw) in text.lines().enumerate() {
        let line_number = number as u32;
        if raw.trim() == "###" {
            in_block_comment = !in_block_comment;
            continue;
        }
        if in_block_comment {
            continue;
        }
        let Some(code) = executable_part(raw) else {
            continue;
        };

        // Byte offset of `code` within `raw`, so spans line up with the file.
        let offset = raw.len() - raw.trim_start().len();

        // A line's indentation decides which categories can explain it: an
        // expression is only ever *part* of a line, and Skript's three
        // catch-all expressions would otherwise claim every line in the file.
        let role = skript_docs::LineRole::from_indent(offset);
        let Some((id, matched)) = catalog.classify_line(code, role) else {
            // A statement that is just a call — `giveKit(player)` — is real
            // Skript, but its effect is registered internally and appears in no
            // published pattern, so the catalog can never explain it. The index
            // already found the call for go-to-definition; reusing that keeps
            // colour, hover and navigation agreeing on what the line is.
            if let Some(call) = call_statement(symbols, line_number) {
                push_token(&mut tokens, line_number, call, FUNCTION, 0);
            } else if let Some(key) = entry_key(symbols, line_number) {
                // A structure entry — `permission:`, `cooldown:`, `trigger:`.
                // These are 12% of the lines in Skript's own example scripts and
                // are not syntax patterns, so the catalog can never explain
                // them; the parse tree can. Classifying them is what takes a
                // real script from "mostly understood" to fully understood.
                push_token(&mut tokens, line_number, key, STRUCTURE, 0);
            }
            continue;
        };
        let Some(entry) = catalog.entry(id) else {
            continue;
        };
        let Some(token_type) = token_type_index(id.category) else {
            continue;
        };

        let mut modifiers = 0;
        if entry.is_deprecated() {
            modifiers |= MODIFIER_DEPRECATED;
        }
        if entry.requirements.is_empty() && entry.addon.is_none() {
            modifiers |= MODIFIER_DEFAULT_LIBRARY;
        }

        // The captured slots are holes; everything between them is the syntax's
        // own literal text and is what gets coloured.
        let mut holes: Vec<(usize, usize)> = matched
            .captures
            .iter()
            .map(|capture| (capture.start, capture.end))
            .collect();
        holes.sort_unstable();

        let mut cursor = 0usize;
        for (start, end) in holes.iter().copied().chain([(code.len(), code.len())]) {
            if start > cursor {
                push_span(
                    &mut tokens,
                    code,
                    line_number,
                    offset,
                    cursor,
                    start,
                    token_type,
                    modifiers,
                );
            }
            cursor = cursor.max(end);
        }
    }

    encode(tokens, &mut to_column)
}

/// Index of the `function` token type in [`TOKEN_TYPES`].
const FUNCTION: u32 = 7;

/// Index of `skriptStructure` in [`TOKEN_TYPES`].
const STRUCTURE: u32 = 5;

/// The key span of a declaration made on `line` inside a structure.
///
/// Covers a command's entries, an `options:` name, an `aliases:` name and a
/// `variables:` default. None of these is a syntax pattern, so the catalog can
/// never explain them — but the parse tree knows exactly what each one is, and
/// the index already resolves references to them.
fn entry_key(symbols: &FileSymbols, line: u32) -> Option<(u32, u32)> {
    symbols
        .flat()
        .into_iter()
        .find(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::Entry
                    | SymbolKind::Option
                    | SymbolKind::Alias
                    | SymbolKind::GlobalVariable
                    | SymbolKind::LocalVariable
            ) && symbol.selection_range.start.line == line
        })
        .map(|symbol| {
            (
                symbol.selection_range.start.character,
                symbol.selection_range.end.character,
            )
        })
}

/// The name span of a function call on `line`, when the index found one.
///
/// Deliberately reuses the reference the index already built rather than
/// re-scanning the text: it is the same data go-to-definition and rename act on,
/// so a line cannot end up coloured as a call that navigation then disagrees
/// about.
fn call_statement(symbols: &FileSymbols, line: u32) -> Option<(u32, u32)> {
    symbols
        .references
        .iter()
        .find(|reference| {
            reference.kind == SymbolKind::Function && reference.name_range.start.line == line
        })
        .map(|reference| {
            (
                reference.name_range.start.character,
                reference.name_range.end.character,
            )
        })
}

/// Emits one token for an already-known byte span on `line`.
///
/// Columns stay in bytes here; [`encode`] converts every token at the end.
fn push_token(
    tokens: &mut Vec<Token>,
    line: u32,
    span: (u32, u32),
    token_type: u32,
    modifiers: u32,
) {
    let (start, end) = span;
    if end > start {
        tokens.push(Token {
            line,
            start,
            length: end - start,
            token_type,
            modifiers,
        });
    }
}

/// Trims a raw line down to the code Skript would parse, or `None` when there
/// is nothing executable on it.
fn executable_part(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    // A comment may follow code, but `#` inside a string or a variable name is
    // literal — walking the line is the only way to tell them apart.
    let code = strip_trailing_comment(trimmed);
    let code = code.trim_end();
    (!code.is_empty()).then_some(code)
}

/// Mirrors Skript's own `Node.splitLine` state machine.
fn strip_trailing_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut in_variable = false;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'"' if !in_variable => in_string = !in_string,
            b'{' if !in_string => in_variable = true,
            b'}' if !in_string => in_variable = false,
            b'#' if !in_string && !in_variable => {
                // `##` is an escaped literal hash, not a comment.
                if bytes.get(index + 1) == Some(&b'#') {
                    index += 2;
                    continue;
                }
                return &line[..index];
            }
            _ => {}
        }
        index += 1;
    }
    line
}

#[allow(clippy::too_many_arguments)]
fn push_span(
    tokens: &mut Vec<Token>,
    code: &str,
    line: u32,
    offset: usize,
    start: usize,
    end: usize,
    token_type: u32,
    modifiers: u32,
) {
    let span = &code[start.min(code.len())..end.min(code.len())];
    // Colour the words, not the whitespace between them, so a folded or wrapped
    // line does not grow a coloured tail.
    let mut cursor = start;
    for word in span.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end();
        if !trimmed.is_empty() {
            tokens.push(Token {
                line,
                start: (offset + cursor) as u32,
                length: trimmed.len() as u32,
                token_type,
                modifiers,
            });
        }
        cursor += word.len();
    }
}

/// LSP wants tokens delta-encoded, sorted, and non-overlapping.
fn encode(
    mut tokens: Vec<Token>,
    to_column: &mut impl FnMut(u32, u32) -> u32,
) -> Vec<lsp::SemanticToken> {
    tokens.sort_by_key(|token| (token.line, token.start));

    let mut out = Vec::with_capacity(tokens.len());
    let mut previous_line = 0u32;
    let mut previous_start = 0u32;

    for token in tokens {
        let start = to_column(token.line, token.start);
        let end = to_column(token.line, token.start + token.length);
        let length = end.saturating_sub(start);
        if length == 0 {
            continue;
        }

        let delta_line = token.line - previous_line;
        let delta_start = if delta_line == 0 {
            start.saturating_sub(previous_start)
        } else {
            start
        };

        out.push(lsp::SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: token.token_type,
            token_modifiers_bitset: token.modifiers,
        });

        previous_line = token.line;
        previous_start = start;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_docs::{fallback_docs, Catalog};

    fn catalog() -> Catalog {
        Catalog::build(fallback_docs())
    }

    fn tokens_for(text: &str) -> Vec<lsp::SemanticToken> {
        tokens(&catalog(), text, &FileSymbols::default(), |_, column| {
            column
        })
    }

    #[test]
    fn the_legend_covers_every_category() {
        for category in Category::ALL {
            // Every category must map to a legend entry, or its tokens are
            // silently dropped by the client.
            assert!(
                token_type_index(*category).is_some(),
                "{category:?} has no legend entry"
            );
        }
    }

    #[test]
    fn classifies_an_effect_line() {
        let produced = tokens_for("on join:\n\tcancel the event\n");
        assert!(!produced.is_empty());
        let effect = TOKEN_TYPES
            .iter()
            .position(|t| *t == "skriptEffect")
            .unwrap() as u32;
        assert!(produced.iter().any(|token| token.token_type == effect));
    }

    #[test]
    fn skips_comments_and_blank_lines() {
        assert!(tokens_for("# just a comment\n\n   \n").is_empty());
    }

    #[test]
    fn does_not_treat_a_hash_inside_a_string_as_a_comment() {
        assert_eq!(
            strip_trailing_comment(r#"send "item #1" to player"#),
            r#"send "item #1" to player"#
        );
        assert_eq!(strip_trailing_comment("stop # done"), "stop ");
        assert_eq!(
            strip_trailing_comment("set {count##} to 1"),
            "set {count##} to 1"
        );
    }

    #[test]
    fn tokens_are_delta_encoded_in_order() {
        let produced = tokens_for("on join:\n\tcancel the event\n\tstop\n");
        // Deltas are non-negative by construction; a negative one is impossible
        // to represent, so an unsorted list would corrupt the whole response.
        let mut line = 0u32;
        for token in &produced {
            line += token.delta_line;
            assert!(line < 3);
        }
    }

    #[test]
    fn never_emits_a_zero_length_token() {
        for token in tokens_for("on join:\n\tcancel the event\n") {
            assert!(token.length > 0);
        }
    }
}

#[cfg(test)]
mod coverage_regressions {
    use super::*;
    use skript_docs::{fallback_docs, Catalog};
    use skript_index::text::Position;

    fn lines_with_tokens(text: &str, symbols: &FileSymbols) -> Vec<u32> {
        let catalog = Catalog::build(fallback_docs());
        let raw = tokens(&catalog, text, symbols, |_, column| column);
        // Undo the delta encoding so the assertions can talk about line numbers.
        let mut line = 0;
        let mut seen = Vec::new();
        for token in raw {
            line += token.delta_line;
            if !seen.contains(&line) {
                seen.push(line);
            }
        }
        seen
    }

    #[test]
    fn prose_inside_a_block_comment_is_never_classified() {
        // `###` blocks hold English, and classifying it would paint arbitrary
        // prose as syntax. `skript-format` and the indentation diagnostics
        // already treat these lines as verbatim; all three have to agree.
        let text = "###\nstop the server immediately\n###\non join:\n\tstop\n";
        let lines = lines_with_tokens(text, &FileSymbols::default());
        assert!(
            !lines.contains(&1),
            "block comment prose was classified: {lines:?}"
        );
    }

    #[test]
    fn a_bare_function_call_statement_is_coloured_as_a_call() {
        // `giveKit(player)` is a real statement, but Skript registers its effect
        // internally and publishes no pattern for it, so the catalog can never
        // explain the line. The index already found the call for
        // go-to-definition, and reusing that keeps colour and navigation
        // agreeing about what the line is.
        let mut symbols = FileSymbols::default();
        symbols.references.push(skript_index::Reference {
            kind: SymbolKind::Function,
            name: "giveKit".into(),
            range: skript_index::Range::new(Position::new(1, 1), Position::new(1, 16)),
            name_range: skript_index::Range::new(Position::new(1, 1), Position::new(1, 8)),
            scope: None,
        });

        let text = "on join:\n\tgiveKit(player)\n";
        let lines = lines_with_tokens(text, &symbols);
        assert!(
            lines.contains(&1),
            "the call statement got no token: {lines:?}"
        );
    }

    #[test]
    fn a_line_with_no_call_and_no_match_stays_uncoloured() {
        // The fallback must not fire on every unrecognised line — an honest
        // absence of colour is the correct answer when we do not know.
        let text = "on join:\n\tflurgle the wombat\n";
        let lines = lines_with_tokens(text, &FileSymbols::default());
        assert!(
            !lines.contains(&1),
            "an unknown line was coloured: {lines:?}"
        );
    }
}

#[cfg(test)]
mod entry_classification {
    use super::*;
    use skript_docs::{fallback_docs, Catalog};
    use skript_index::Workspace;

    /// Structure entries are 12% of the lines in Skript's own example scripts.
    /// They are not syntax patterns, so the catalog can never explain them —
    /// but the parse tree knows exactly what each one is, and leaving them
    /// unclassified was the whole of the remaining gap on a real script.
    #[test]
    fn every_line_of_a_real_command_is_classified() {
        let source = "command /home <text>:\n\tdescription: Go home\n\tpermission: skript.home\n\tcooldown: 15 seconds\n\ttrigger:\n\t\tstop\n";

        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let catalog = Catalog::build(fallback_docs());

        let raw = tokens(&catalog, source, document.symbols(), |_, column| column);
        let mut line = 0;
        let mut seen = Vec::new();
        for token in raw {
            line += token.delta_line;
            if !seen.contains(&line) {
                seen.push(line);
            }
        }

        for expected in 0..=5 {
            assert!(
                seen.contains(&expected),
                "line {expected} was not classified; got {seen:?}"
            );
        }
    }
}
