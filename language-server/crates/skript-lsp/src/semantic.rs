//! Semantic tokens: the classification tree-sitter cannot make.
//!
//! The tree-sitter grammar cannot tell an effect from a condition from an
//! expression, because all three are runtime-registered patterns. So
//! `highlights.scm` leaves statement prose uncoloured, and this module supplies
//! the classification instead: each executable line is matched against the
//! catalog, and the *literal* parts of the winning pattern are emitted as a
//! token of that syntax's category.
//!
//! Only the literals are coloured. The `%…%` slots are left alone so that the
//! expressions inside them keep their own tree-sitter colours: `{_x}` stays a
//! variable and `"hi"` stays a string, exactly as a reader expects.
//!
//! The token type names here must match
//! `extension/languages/skript/semantic_token_rules.json`.

use std::collections::HashMap;

use skript_docs::{Catalog, Category};
use skript_index::{FileSymbols, Symbol, SymbolKind};
use tower_lsp::lsp_types as lsp;

use crate::{calls, lines};

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

/// Index of the `function` token type in [`TOKEN_TYPES`].
const FUNCTION: u32 = 7;

/// Index of `skriptType` in [`TOKEN_TYPES`].
const TYPE: u32 = 6;

/// Index of `skriptStructure` in [`TOKEN_TYPES`].
const STRUCTURE: u32 = 5;

/// How deep slot classification recurses. Three levels covers
/// `the helmet of the target entity of player` without letting a pathological
/// line cost more than a handful of cached lookups.
const MAX_FRAGMENT_DEPTH: usize = 3;

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

/// A piece of a line being coloured: its text, and the byte offset at which
/// that text starts in the raw line.
#[derive(Debug, Clone, Copy)]
struct Text<'t> {
    code: &'t str,
    offset: usize,
}

/// Collects the tokens of one line.
struct Painter<'p> {
    tokens: &'p mut Vec<Token>,
    line: u32,
}

impl Painter<'_> {
    /// One token for an already-known byte span of the raw line.
    ///
    /// Columns stay in bytes here; [`encode`] converts every token at the end.
    fn token(&mut self, span: (u32, u32), token_type: u32, modifiers: u32) {
        let (start, end) = span;
        if end > start {
            self.tokens.push(Token {
                line: self.line,
                start,
                length: end - start,
                token_type,
                modifiers,
            });
        }
    }

    /// Colours `text.code[start..end]` word by word.
    fn span(&mut self, text: Text<'_>, start: usize, end: usize, token_type: u32, modifiers: u32) {
        let code = text.code;
        let span = &code[start.min(code.len())..end.min(code.len())];
        // Colour the words, not the whitespace between them, so a folded or
        // wrapped line does not grow a coloured tail.
        let mut cursor = start;
        for word in span.split_inclusive(char::is_whitespace) {
            let trimmed = word.trim_end();
            if !trimmed.is_empty() {
                self.tokens.push(Token {
                    line: self.line,
                    start: (text.offset + cursor) as u32,
                    length: trimmed.len() as u32,
                    token_type,
                    modifiers,
                });
            }
            cursor += word.len();
        }
    }

    /// Colours `text`'s literal parts (everything outside `holes`) as one
    /// token type.
    fn literals(
        &mut self,
        text: Text<'_>,
        holes: &[(usize, usize)],
        token_type: u32,
        modifiers: u32,
    ) {
        let mut cursor = 0usize;
        for (start, end) in holes
            .iter()
            .copied()
            .chain([(text.code.len(), text.code.len())])
        {
            if start > cursor {
                self.span(text, cursor, start, token_type, modifiers);
            }
            cursor = cursor.max(end);
        }
    }
}

/// Declarations indexed by line, built once per request.
///
/// Looking them up by walking the whole symbol tree on every line made a
/// semantic-tokens request quadratic in the size of the file.
struct LineSymbols<'s> {
    /// The ClassySK class, field or method declared on each line.
    class_declarations: HashMap<u32, &'s Symbol>,
    /// The structure entry, option, alias or variable default keyed on each
    /// line.
    entry_keys: HashMap<u32, &'s Symbol>,
}

impl<'s> LineSymbols<'s> {
    fn new(symbols: &'s FileSymbols) -> Self {
        let mut class_declarations = HashMap::new();
        let mut entry_keys = HashMap::new();
        // First in tree order wins, as the per-line search it replaces did.
        for symbol in symbols.flat() {
            match symbol.kind {
                SymbolKind::Class | SymbolKind::Field | SymbolKind::Method => {
                    class_declarations
                        .entry(symbol.range.start.line)
                        .or_insert(symbol);
                }
                SymbolKind::Entry
                | SymbolKind::Option
                | SymbolKind::Alias
                | SymbolKind::GlobalVariable
                | SymbolKind::LocalVariable => {
                    entry_keys
                        .entry(symbol.selection_range.start.line)
                        .or_insert(symbol);
                }
                _ => {}
            }
        }
        Self {
            class_declarations,
            entry_keys,
        }
    }

    /// The ClassySK declaration starting on `line` and its name span, if any.
    fn class_declaration(&self, line: u32) -> Option<(SymbolKind, (u32, u32))> {
        self.class_declarations
            .get(&line)
            .map(|symbol| (symbol.kind, selection_columns(symbol)))
    }

    /// The key span of a declaration made on `line` inside a structure.
    ///
    /// Covers a command's entries, an `options:` name, an `aliases:` name and
    /// a `variables:` default. None of these is a syntax pattern, so the
    /// catalog can never explain them, but the parse tree knows exactly what
    /// each one is, and the index already resolves references to them.
    fn entry_key(&self, line: u32) -> Option<(u32, u32)> {
        self.entry_keys
            .get(&line)
            .map(|symbol| selection_columns(symbol))
    }
}

fn selection_columns(symbol: &Symbol) -> (u32, u32) {
    (
        symbol.selection_range.start.character,
        symbol.selection_range.end.character,
    )
}

/// Classifies every executable line of `text` and returns LSP semantic tokens.
///
/// `to_column` converts a (line, byte column) pair into the column the client
/// expects; the caller owns encoding negotiation.
pub fn tokens(
    catalog: &Catalog,
    text: &str,
    symbols: &FileSymbols,
    mut to_column: impl FnMut(u32, u32) -> u32,
) -> Vec<lsp::SemanticToken> {
    let mut tokens = Vec::new();
    let by_line = LineSymbols::new(symbols);

    // A `###` block's interior is prose; see `lines::code_lines`.
    for (line_number, raw) in lines::code_lines(text) {
        let Some(code) = executable_part(raw) else {
            continue;
        };
        let mut painter = Painter {
            tokens: &mut tokens,
            line: line_number,
        };
        classify_line(&mut painter, catalog, symbols, &by_line, raw, code);
    }

    encode(tokens, &mut to_column)
}

/// Colours one executable line. `code` is `raw` without indentation or
/// comment.
fn classify_line(
    painter: &mut Painter<'_>,
    catalog: &Catalog,
    symbols: &FileSymbols,
    by_line: &LineSymbols<'_>,
    raw: &str,
    code: &str,
) {
    let line_number = painter.line;

    // A ClassySK class header, field or method header declares rather than
    // runs, and the grammar owns its shape. Matching it against the catalog
    // anyway painted the `=` of `private active: boolean = false` as a
    // condition and a parameter name as a structure.
    if let Some((kind, name)) = by_line.class_declaration(line_number) {
        match kind {
            SymbolKind::Class => painter.token(name, TYPE, 0),
            SymbolKind::Method => painter.token(name, FUNCTION, 0),
            _ => {}
        }
        return;
    }

    // Byte offset of `code` within `raw`, so spans line up with the file.
    let text = Text {
        code,
        offset: skript_index::indent_width(raw),
    };

    // A `function` or `command` header is shaped by the grammar, which colours
    // its name, parameters, types and argument slots separately. The catalog's
    // patterns for them end in a `<.+>` regex, and a regex is not a capture,
    // so matching them painted the entire header one colour,
    // `fullEquipUpdate(p:player,` and all. Only the keyword is claimed.
    if let Some(keyword) = structure_keyword(symbols, line_number, code) {
        painter.span(text, 0, keyword, STRUCTURE, 0);
        return;
    }

    // Expressions inside call arguments (`updateEquip(p: {_p}'s helmet)`) are
    // classified whether or not the line itself is understood.
    classify_call_arguments(painter, catalog, symbols, text);

    // A line's indentation decides which categories can explain it: an
    // expression is only ever *part* of a line, and Skript's three catch-all
    // expressions would otherwise claim every line in the file.
    let role = skript_docs::LineRole::from_indent(text.offset);
    // A pattern made of nothing but optional words and one slot (real addons
    // publish them, `[skeb] [reply|response] %httpexchange% …` for one)
    // matches any line at all while explaining none of it. Taking that as an
    // answer coloured `updateEquip(…)` as nothing, and hid the call fallback
    // below that knows exactly what the line is.
    let classified = catalog
        .classify_line(code, role)
        .filter(|(_, matched)| explains_some_text(code, matched));
    let Some((id, matched)) = classified else {
        // A statement that is just a call (`giveKit(player)`) is real Skript,
        // but its effect is registered internally and appears in no published
        // pattern, so the catalog can never explain it. The index already
        // found the call for go-to-definition; reusing that keeps colour,
        // hover and navigation agreeing on what the line is.
        if let Some(call) = call_statement(symbols, line_number) {
            painter.token(call, FUNCTION, 0);
        } else if let Some(key) = by_line.entry_key(line_number) {
            // A structure entry: `permission:`, `cooldown:`, `trigger:`. These
            // are 12% of the lines in Skript's own example scripts and are not
            // syntax patterns, so the catalog can never explain them; the
            // parse tree can.
            painter.token(key, STRUCTURE, 0);
        }
        return;
    };
    let Some(entry) = catalog.entry(id) else {
        return;
    };
    let Some(token_type) = token_type_index(id.category) else {
        return;
    };
    let modifiers = modifiers_for(entry);

    // What fills each slot is itself an expression: `{_p}'s helmet` in
    // `send "hi" to {_p}'s helmet`. Classifying it is what stops a line from
    // being one coloured keyword followed by plain prose. An event header's
    // slots are the exception: they are claimed whole as a type filter, just
    // below.
    if id.category != Category::Event {
        for capture in &matched.captures {
            colour_capture(painter, catalog, text, capture, 0);
        }
    }

    // The captured slots are holes; everything between them is the syntax's
    // own literal text and is what gets coloured.
    painter.literals(text, &holes_of(&matched), token_type, modifiers);

    // An event header's slots are claimed too, as the type they filter on.
    // Elsewhere a hole is left to the grammar on purpose (a string inside
    // `send "hi" to player` should stay string-coloured), but an event header
    // is the one place the grammar captures the *whole* name, slot included,
    // as `@function`. Leaving the hole there meant `on left-click on a sign:`
    // came out two-tone, with `a sign` keeping a stale colour that also
    // claimed it was a function.
    //
    // `skriptType` is accurate here rather than a convenience: an event
    // pattern's slot is a type filter by construction (`%entity types%`,
    // `%item types%`, `%worlds%`).
    if id.category == Category::Event {
        for (start, end) in holes_of(&matched) {
            if end > start {
                painter.span(text, start, end, TYPE, modifiers);
            }
        }
    }
}

/// The most specific syntax under `column` (a byte column in `raw`): an
/// expression in a slot or a call argument when the cursor is inside one, the
/// line's own match otherwise, with the byte span in `raw` that it covers.
///
/// Hover used to describe only the whole line, so pointing at `self` in
/// `return Req::get({_p}, self::function)` explained *Return*. This walks the
/// same structure the colouring does, so what hover names is what is coloured.
/// The span lets the editor keep the popup open while the mouse stays on the
/// thing described, instead of re-asking at every word on the way to it.
pub fn innermost_at(
    catalog: &Catalog,
    symbols: &FileSymbols,
    raw: &str,
    line: u32,
    column: usize,
) -> Option<(skript_docs::EntryId, usize, usize)> {
    let code = executable_part(raw)?;
    let offset = skript_index::indent_width(raw);
    let cursor = column.checked_sub(offset)?;
    let in_raw =
        |(id, start, end): (skript_docs::EntryId, usize, usize)| (id, start + offset, end + offset);

    // A call's argument first: a call statement matches no pattern at all.
    for (start, end) in call_arguments(symbols, line, code, offset) {
        if (start..=end).contains(&cursor) {
            if let Some(found) = fragment_at(catalog, code, start, end, cursor, 1) {
                return Some(in_raw(found));
            }
        }
    }

    let role = skript_docs::LineRole::from_indent(offset);
    let (id, matched) = catalog
        .classify_line(code, role)
        .filter(|(_, matched)| explains_some_text(code, matched))?;
    if id.category != Category::Event {
        for capture in &matched.captures {
            if !capture.slot.literal_only
                && !capture.regex
                && (capture.start..=capture.end).contains(&cursor)
            {
                if let Some(deeper) =
                    fragment_at(catalog, code, capture.start, capture.end, cursor, 1)
                {
                    return Some(in_raw(deeper));
                }
            }
        }
    }
    let end = code.trim_end().trim_end_matches(':').trim_end().len();
    Some(in_raw((id, 0, end)))
}

/// [`classify_fragment`]'s rules, answering "what is under the cursor" instead
/// of colouring. The span returned is in `code`.
fn fragment_at(
    catalog: &Catalog,
    code: &str,
    start: usize,
    end: usize,
    cursor: usize,
    depth: usize,
) -> Option<(skript_docs::EntryId, usize, usize)> {
    if depth > MAX_FRAGMENT_DEPTH || start >= end || end > code.len() {
        return None;
    }
    let raw = &code[start..end];
    let text = raw.trim();
    if !worth_classifying(text) {
        return None;
    }
    let base = start + skript_index::indent_width(raw);
    let (id, matched) = catalog.classify_line(text, skript_docs::LineRole::Expression)?;
    catalog.entry(id)?;
    if !matched.explains_text(text) {
        return None;
    }
    let local = cursor.checked_sub(base)?;
    for capture in &matched.captures {
        if !capture.slot.literal_only
            && !capture.regex
            && (capture.start..=capture.end).contains(&local)
        {
            if let Some((deeper, deeper_start, deeper_end)) =
                fragment_at(catalog, text, capture.start, capture.end, local, depth + 1)
            {
                return Some((deeper, deeper_start + base, deeper_end + base));
            }
        }
    }
    Some((id, base, base + text.len()))
}

fn modifiers_for(entry: &skript_docs::Entry) -> u32 {
    let mut modifiers = 0;
    if entry.is_deprecated() {
        modifiers |= MODIFIER_DEPRECATED;
    }
    if entry.requirements.is_empty() && entry.addon.is_none() {
        modifiers |= MODIFIER_DEFAULT_LIBRARY;
    }
    modifiers
}

fn holes_of(matched: &skript_syntax::Match) -> Vec<(usize, usize)> {
    let mut holes: Vec<(usize, usize)> = matched
        .captures
        .iter()
        .map(|capture| (capture.start, capture.end))
        .collect();
    holes.sort_unstable();
    holes
}

/// The byte length of the keyword opening a `function` or `command` header on
/// `line`, if that is what the line is.
fn structure_keyword(symbols: &FileSymbols, line: u32, code: &str) -> Option<usize> {
    let is_header = symbols.symbols.iter().any(|symbol| {
        matches!(
            symbol.kind,
            SymbolKind::Function | SymbolKind::LocalFunction | SymbolKind::Command
        ) && symbol.range.start.line == line
    });
    if !is_header {
        return None;
    }
    ["local function", "function", "command"]
        .into_iter()
        .find(|keyword| code.starts_with(keyword))
        .map(str::len)
}

/// Classifies `text.code[start..end]` as an expression and colours its
/// literals, then does the same for each of its own slots.
///
/// Deliberately conservative about what it will try, because a wrong colour
/// is worse than none:
///
/// * a match that explains none of the text itself is discarded: the
///   catch-all `[the] [event-]<.+>` would otherwise claim anything at all;
/// * a fragment holding a call or a ClassySK `::` access is left alone: the
///   grammar already colours those precisely, and a call's arguments are
///   classified on their own by [`classify_call_arguments`];
/// * a fragment with no prose (just a string, a variable or a number) has
///   nothing for a pattern to explain.
fn classify_fragment(
    painter: &mut Painter<'_>,
    catalog: &Catalog,
    outer: Text<'_>,
    start: usize,
    end: usize,
    depth: usize,
) {
    if depth > MAX_FRAGMENT_DEPTH || start >= end || end > outer.code.len() {
        return;
    }
    let raw = &outer.code[start..end];
    let code = raw.trim();
    if !worth_classifying(code) {
        return;
    }
    let text = Text {
        code,
        offset: outer.offset + start + skript_index::indent_width(raw),
    };

    let Some((id, matched)) = catalog.classify_line(code, skript_docs::LineRole::Expression) else {
        return;
    };
    let Some(entry) = catalog.entry(id) else {
        return;
    };
    let Some(token_type) = token_type_index(id.category) else {
        return;
    };

    // A match that explains none of the text itself (the catch-all
    // `[the] %*entity types%` taking a bare `player`) asserts nothing worth
    // colouring, and would claim every lone word as that one expression.
    if !matched.explains_text(code) {
        return;
    }
    painter.literals(text, &holes_of(&matched), token_type, modifiers_for(entry));

    for capture in &matched.captures {
        colour_capture(painter, catalog, text, capture, depth);
    }
}

/// Colours what filled one hole of a matched pattern.
fn colour_capture(
    painter: &mut Painter<'_>,
    catalog: &Catalog,
    text: Text<'_>,
    capture: &skript_syntax::SlotCapture,
    depth: usize,
) {
    if capture.regex {
        // A single word in a regex hole is a name the pattern asks for
        // (ClassySK's class in `new instance of Wallet`, a packet field) and
        // reads as a type. Anything longer is arbitrary text the regex
        // swallowed, and is left alone.
        let is_name = !capture.text.is_empty()
            && capture
                .text
                .chars()
                .all(|ch| ch.is_alphanumeric() || ch == '_');
        if is_name {
            painter.span(text, capture.start, capture.end, TYPE, 0);
        }
    } else if capture.slot.literal_only {
        // `%*equipment slots%` takes a literal of that type (the `helmet` in
        // `{_p}'s helmet`), not an expression, so there is nothing to recurse
        // into. It is a type by construction, as an event header's slots are.
        painter.span(text, capture.start, capture.end, TYPE, 0);
    } else {
        classify_fragment(
            painter,
            catalog,
            text,
            capture.start,
            capture.end,
            depth + 1,
        );
    }
}

/// Whether a match accounts for any of `code` with its own words, rather
/// than handing the whole of it to slots. `classify_line` drops a trailing
/// `:` before matching, so the comparison does too: the colon is not a word
/// the pattern spelled out.
fn explains_some_text(code: &str, matched: &skript_syntax::Match) -> bool {
    matched.explains_text(code.trim_end().trim_end_matches(':'))
}

/// See [`classify_fragment`] for why each of these is refused.
fn worth_classifying(text: &str) -> bool {
    let mut has_prose = false;
    let mut in_string = false;
    let mut braces = 0u32;
    let bytes = text.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'"' if braces == 0 => in_string = !in_string,
            _ if in_string => {}
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            _ if braces > 0 => {}
            b'(' => return false,
            b':' if bytes.get(index + 1) == Some(&b':') => return false,
            byte if byte.is_ascii_alphabetic() => has_prose = true,
            _ => {}
        }
    }
    has_prose
}

/// The argument spans of every closed call on `line`, in `code`, each with a
/// named argument's `p:` already skipped; the grammar colours that.
///
/// An unclosed call is still being typed, and nothing in it is reliable yet.
fn call_arguments(
    symbols: &FileSymbols,
    line: u32,
    code: &str,
    offset: usize,
) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for reference in &symbols.references {
        if !matches!(reference.kind, SymbolKind::Function | SymbolKind::Method)
            || reference.name_range.start.line != line
        {
            continue;
        }
        let Some(open) = (reference.name_range.end.character as usize).checked_sub(offset) else {
            continue;
        };
        if code.as_bytes().get(open) != Some(&b'(') {
            continue;
        }
        let arguments = calls::arguments(code, open);
        if !arguments.closed {
            continue;
        }
        spans.extend(arguments.spans.into_iter().map(|(start, end)| {
            let skip = calls::named_argument(&code[start..end]).map_or(0, |(_, prefix)| prefix);
            (start + skip, end)
        }));
    }
    spans
}

/// Classifies each argument of every call on the painter's line as an
/// expression.
///
/// `updateEquip(p: {_p}'s helmet)` is the commonest shape of modern Skript, and
/// a call's arguments are never inside a pattern's slot (a call statement
/// matches no pattern at all), so without this they are never classified.
fn classify_call_arguments(
    painter: &mut Painter<'_>,
    catalog: &Catalog,
    symbols: &FileSymbols,
    text: Text<'_>,
) {
    for (start, end) in call_arguments(symbols, painter.line, text.code, text.offset) {
        classify_fragment(painter, catalog, text, start, end, 1);
    }
}

/// The name span of a function or method call on `line`, when the index found
/// one.
///
/// Deliberately reuses the reference the index already built rather than
/// re-scanning the text: it is the same data go-to-definition and rename act on,
/// so a line cannot end up coloured as a call that navigation then disagrees
/// about. A ClassySK `self::reset()` is the same shape of statement, and no
/// catalog pattern can explain it either.
fn call_statement(symbols: &FileSymbols, line: u32) -> Option<(u32, u32)> {
    symbols
        .references
        .iter()
        .find(|reference| {
            matches!(reference.kind, SymbolKind::Function | SymbolKind::Method)
                && reference.name_range.start.line == line
        })
        .map(|reference| {
            (
                reference.name_range.start.character,
                reference.name_range.end.character,
            )
        })
}

/// Trims a raw line down to the code Skript would parse, or `None` when there
/// is nothing executable on it.
fn executable_part(raw: &str) -> Option<&str> {
    let code = lines::code_part(raw).trim();
    (!code.is_empty()).then_some(code)
}

/// LSP wants tokens delta-encoded, sorted, and non-overlapping.
fn encode(
    mut tokens: Vec<Token>,
    to_column: &mut impl FnMut(u32, u32) -> u32,
) -> Vec<lsp::SemanticToken> {
    tokens.sort_by_key(|token| (token.line, token.start));
    // LSP forbids overlapping tokens. Slot and argument classification are
    // built to stay inside their own spans, but a pattern whose regex swallowed
    // a call could still reach into it; the first token wins.
    let mut kept: Vec<Token> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if let Some(last) = kept.last() {
            if last.line == token.line && token.start < last.start + last.length {
                continue;
            }
        }
        kept.push(token);
    }

    let mut out = Vec::with_capacity(kept.len());
    let mut previous_line = 0u32;
    let mut previous_start = 0u32;

    for token in kept {
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
    /// Every `token_type` the extension maps must be one the server can emit.
    ///
    /// These two files are edited independently and nothing links them, so a
    /// rule naming a token that does not exist is silently dead config — it
    /// looks like the colour is configured and it can never apply.
    /// `skriptEventValue` sat there exactly that way.
    #[test]
    fn every_mapped_token_type_is_one_the_server_emits() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../extension/languages/skript/semantic_token_rules.json"
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("extension not present, skipping");
            return;
        };

        // Strip the `//` comments the file is documented with; Zed's parser
        // accepts them, `serde_json` does not.
        let stripped: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        let rules: Vec<serde_json::Value> =
            serde_json::from_str(&stripped).expect("semantic_token_rules.json parses");

        let known: Vec<&str> = super::TOKEN_TYPES.to_vec();
        // The standard LSP types the legend does not carry are still legitimate
        // to map: another server, or a future version of this one, may emit
        // them. Only a `skript*` name is ours to get wrong.
        for rule in &rules {
            let name = rule["token_type"].as_str().expect("token_type is a string");
            if name.starts_with("skript") {
                assert!(
                    known.contains(&name),
                    "semantic_token_rules.json maps `{name}`, which the server never emits"
                );
            }
        }
    }

    /// The `FUNCTION` / `TYPE` / `STRUCTURE` constants are hand-written indices
    /// into `TOKEN_TYPES`. Nothing else ties them to the array, so reordering
    /// or inserting a type would silently recolour whole categories — every
    /// event slot rendering as a function, say. This is the only thing standing
    /// between that and a release.
    #[test]
    fn the_hand_written_token_indices_match_the_legend() {
        for (name, index) in [
            ("skriptStructure", super::STRUCTURE),
            ("skriptType", super::TYPE),
            ("function", super::FUNCTION),
        ] {
            assert_eq!(
                super::TOKEN_TYPES[index as usize],
                name,
                "TOKEN_TYPES[{index}] should be {name}"
            );
        }
    }

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
            executable_part(r#"send "item #1" to player"#),
            Some(r#"send "item #1" to player"#)
        );
        assert_eq!(executable_part("	stop # done"), Some("stop"));
        assert_eq!(
            executable_part("set {count##} to 1"),
            Some("set {count##} to 1")
        );
        assert_eq!(executable_part("  # only a comment"), None);
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
mod expression_classification {
    use super::*;
    use skript_docs::{fallback_docs, Catalog};

    /// Decodes tokens to `(line, start, text, token type)`.
    fn decoded(catalog: &Catalog, text: &str) -> Vec<(u32, String, u32)> {
        let document = skript_index::Document::new("file:///t.sk", text);
        let lines: Vec<&str> = text.lines().collect();
        let (mut line, mut start) = (0u32, 0u32);
        let mut out = Vec::new();
        for token in tokens(catalog, text, document.symbols(), |_, column| column) {
            if token.delta_line > 0 {
                start = 0;
            }
            line += token.delta_line;
            start += token.delta_start;
            let word = &lines[line as usize][start as usize..(start + token.length) as usize];
            out.push((line, word.to_string(), token.token_type));
        }
        out
    }

    /// Only a closed call's arguments are classified, each without its
    /// `name:` prefix.
    #[test]
    fn call_arguments_skip_named_prefixes_and_unclosed_calls() {
        let document = skript_index::Document::new(
            "file:///t.sk",
            "on join:
	f(p: {_a, b}, x)
	g(a, b
",
        );
        let code = "f(p: {_a, b}, x)";
        let spans: Vec<&str> = call_arguments(document.symbols(), 1, code, 1)
            .into_iter()
            .map(|(start, end)| &code[start..end])
            .collect();
        assert_eq!(spans, vec![" {_a, b}", " x"]);
        assert!(call_arguments(document.symbols(), 2, "g(a, b", 1).is_empty());
    }

    #[test]
    fn classifies_only_fragments_with_prose_and_no_call() {
        assert!(worth_classifying("{_p}'s helmet"));
        assert!(!worth_classifying("{_p}"));
        assert!(!worth_classifying(r#""helmet""#));
        assert!(!worth_classifying("5"));
        assert!(!worth_classifying(r#"Equipment::get("x")"#));
        assert!(!worth_classifying("size of f(x)"));
    }

    /// `[local] function <.+>` ends in a regex, and a regex is not a capture,
    /// so matching it painted the whole header — name, parameters, types — one
    /// colour over the grammar's.
    #[test]
    fn a_function_header_claims_only_its_keyword() {
        let catalog = Catalog::build(fallback_docs());
        let found = decoded(
            &catalog,
            "function f(p: player, s: boolean = true):\n\tstop\n",
        );
        let header: Vec<&str> = found
            .iter()
            .filter(|(line, _, _)| *line == 0)
            .map(|(_, word, _)| word.as_str())
            .collect();
        assert_eq!(header, vec!["function"]);
    }

    /// Against the real database: slot contents and call arguments are
    /// classified, and a literal-only slot reads as a type.
    #[test]
    fn slots_and_call_arguments_are_classified_against_real_data() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../vendor/docs.json");
        let Ok(json) = std::fs::read_to_string(path) else {
            eprintln!("skipping: run node scripts/fetch-docs.mjs");
            return;
        };
        let docs = skript_docs::Docs::parse(&json).expect("vendor/docs.json parses");
        let catalog = Catalog::build(docs);
        let expression = token_type_index(Category::Expression).unwrap();

        let found = decoded(
            &catalog,
            concat!(
                "on join:\n",
                "\tsend \"hi\" to {_p}'s helmet\n",
                "\tf(p: {_p}'s held item, e: future event-item)\n",
                "\tgive player diamond sword\n",
            ),
        );
        let on = |line: u32| -> Vec<(&str, u32)> {
            found
                .iter()
                .filter(|(at, _, _)| *at == line)
                .map(|(_, word, ty)| (word.as_str(), *ty))
                .collect()
        };

        let send = on(1);
        assert!(send.contains(&("'s", expression)), "{send:?}");
        assert!(send.contains(&("helmet", TYPE)), "{send:?}");

        let call = on(2);
        for word in ["held", "item", "future"] {
            assert!(call.contains(&(word, expression)), "{word}: {call:?}");
        }
        // The named argument's `p` belongs to the grammar.
        assert!(!call.iter().any(|(word, _)| *word == "p"), "{call:?}");

        // `player` and `diamond sword` are explained only by a catch-all that
        // matches any text, so nothing may claim them.
        let give = on(3);
        assert!(
            !give
                .iter()
                .any(|(word, _)| ["player", "diamond", "sword"].contains(word)),
            "{give:?}"
        );
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

    /// A ClassySK declaration is shaped by the grammar. Run through the catalog,
    /// the `=` of a field default came out as a condition.
    #[test]
    fn classysk_declarations_are_never_matched_against_the_catalog() {
        let text = concat!(
            "class Wallet:\n",
            "\tprivate active: boolean = false\n",
            "\tpublic reset(value: number):\n",
            "\t\tself::reset(1)\n",
        );
        let document = skript_index::Document::new("file:///t.sk", text);
        let catalog = Catalog::build(fallback_docs());
        let raw = tokens(&catalog, text, document.symbols(), |_, column| column);

        let (mut line, mut start) = (0, 0);
        let mut found = Vec::new();
        for token in raw {
            if token.delta_line > 0 {
                start = 0;
            }
            line += token.delta_line;
            start += token.delta_start;
            found.push((line, start, token.length, token.token_type));
        }
        assert_eq!(
            found,
            vec![
                (0, 6, 6, TYPE),     // Wallet
                (2, 8, 5, FUNCTION), // reset
                (3, 8, 5, FUNCTION), // self::reset(1)
            ]
        );
    }

    #[test]
    fn a_line_with_no_call_and_no_match_stays_uncoloured() {
        // The fallback must not fire on every unrecognised line: no colour is
        // the correct answer when we do not know.
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
