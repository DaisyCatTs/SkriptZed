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
    mut to_column: impl FnMut(u32, u32) -> u32,
) -> Vec<lsp::SemanticToken> {
    let mut tokens = Vec::new();

    for (number, raw) in text.lines().enumerate() {
        let line_number = number as u32;
        let Some(code) = executable_part(raw) else {
            continue;
        };

        // Byte offset of `code` within `raw`, so spans line up with the file.
        let offset = raw.len() - raw.trim_start().len();

        let Some((id, matched)) = catalog.classify_best(code.trim_end_matches(':')) else {
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
        tokens(&catalog(), text, |_, column| column)
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
