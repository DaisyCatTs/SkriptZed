//! skript-reflect's custom syntax: effects, expressions, conditions and events
//! a script defines for itself.
//!
//! ```skript
//! effect [the] greet %player%:
//!     trigger:
//!         send "hi" to expr-1
//! ```
//!
//! The grammar has no idea these exist (nothing lexical sets them apart from
//! an event header), so they arrive as `event` nodes, and the outline listed
//! `effect [the] greet %player%` as an event. Recognising them here gives the
//! language server the patterns to classify every use of them with.
//!
//! The header forms are skript-reflect's own, as registered in its
//! `StructCustom*` classes and published on SkriptHub:
//!
//! * `[local] effect <pattern>` / `[local] effect`
//! * `[local] condition <pattern>` / `[local] condition` /
//!   `[local] <types> property condition <pattern>`
//! * `[local] [plural|non-single] expression <pattern>` / `… expression` /
//!   `… <types> property <pattern>`
//! * `[local] [custom] event "<id>"`
//!
//! The bare forms list their patterns in a `patterns:` block; an event takes
//! `pattern:` or `patterns:`.

use tree_sitter::Node;

use crate::symbols::{body_sections, field_range, field_source, first_line, Symbol, SymbolKind};
use crate::text::{Position, Range};

/// Which kind of syntax a definition registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxKind {
    Effect,
    Expression,
    Condition,
    Event,
}

impl SyntaxKind {
    pub fn label(self) -> &'static str {
        match self {
            SyntaxKind::Effect => "effect",
            SyntaxKind::Expression => "expression",
            SyntaxKind::Condition => "condition",
            SyntaxKind::Event => "event",
        }
    }
}

/// One custom syntax definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxDefinition {
    pub kind: SyntaxKind,
    /// The patterns as the script writes them.
    pub declared: Vec<String>,
    /// What a use is matched against: `declared`, or for a property the two
    /// forms skript-reflect expands it into (`[the] balance of %players%` and
    /// `%players%'[s] balance`).
    pub patterns: Vec<String>,
    /// `local` syntax is only usable in the script that defines it.
    pub local: bool,
    /// The types a property belongs to: `players` in `players property …`.
    pub property_of: Option<String>,
    /// An event's identifier, `on_greet` in `event "on_greet":`.
    pub event_id: Option<String>,
    /// An event's `event-values: player, location`.
    pub event_values: Vec<String>,
    /// An expression's `return type:`.
    pub return_type: Option<String>,
    /// The `#` comment lines directly above the header, without their `#`.
    pub comment: Vec<String>,
    pub range: Range,
    /// The header line, from column 0 to the end of its pattern.
    pub selection_range: Range,
}

impl SyntaxDefinition {
    /// A name for the definition: its first pattern, or an event's id.
    pub fn name(&self) -> String {
        match (&self.event_id, self.declared.first()) {
            (Some(id), None) => id.clone(),
            (_, Some(pattern)) => pattern.clone(),
            (None, None) => self.kind.label().to_string(),
        }
    }

    /// Identifies the definition across rebuilds, independent of where in the
    /// file it sits: the kind, the property owner and the first pattern.
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.kind.label(),
            self.property_of.as_deref().unwrap_or_default(),
            self.name()
        )
    }

    /// How the outline labels it: `custom effect`, `local players property`.
    pub fn describe(&self) -> String {
        let local = if self.local { "local " } else { "" };
        match (&self.property_of, self.kind) {
            (Some(types), SyntaxKind::Condition) => {
                format!("{local}{types} property condition")
            }
            (Some(types), _) => format!("{local}{types} property"),
            (None, kind) => format!("{local}custom {}", kind.label()),
        }
    }
}

/// What a header line says, before the body is read.
#[derive(Debug, PartialEq, Eq)]
struct Header {
    kind: SyntaxKind,
    local: bool,
    property_of: Option<String>,
    /// The single pattern, for the one-line forms.
    pattern: Option<String>,
    event_id: Option<String>,
}

/// Reads a header line, `local effect greet %player%:` and the like.
fn parse_header(line: &str) -> Option<Header> {
    let text = line.trim().trim_end_matches(':').trim_end();
    // ASCII lowercasing keeps byte offsets, so `lower` indexes `text`.
    let lower = text.to_ascii_lowercase();
    let mut at = 0;
    let eat = |word: &str, at: &mut usize| -> bool {
        let found = lower[*at..].starts_with(word);
        if found {
            *at += word.len();
        }
        found
    };

    let local = eat("local ", &mut at);
    let header = |kind, property_of, pattern: Option<&str>, event_id| {
        Some(Header {
            kind,
            local,
            property_of,
            pattern: pattern
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .map(str::to_string),
            event_id,
        })
    };

    // `[custom] event "id"`.
    let event_start = at;
    eat("custom ", &mut at);
    if eat("event ", &mut at) {
        let id = text[at..].trim();
        let quoted = id.len() >= 2 && id.starts_with('"') && id.ends_with('"');
        return quoted
            .then(|| {
                header(
                    SyntaxKind::Event,
                    None,
                    None,
                    Some(id[1..id.len() - 1].to_string()),
                )
            })
            .flatten();
    }
    at = event_start;

    for (word, kind) in [
        ("effect", SyntaxKind::Effect),
        ("condition", SyntaxKind::Condition),
    ] {
        if lower[at..] == *word {
            return header(kind, None, None, None);
        }
        if eat(&format!("{word} "), &mut at) {
            let pattern = Some(&text[at..]).filter(|p| !p.trim().is_empty())?;
            return header(kind, None, Some(pattern), None);
        }
    }

    // `(plural|non(-|[ ])single)` only qualifies an expression.
    let plural = ["plural ", "non-single ", "non single ", "nonsingle "]
        .iter()
        .any(|word| eat(word, &mut at));
    if lower[at..] == *"expression" {
        return header(SyntaxKind::Expression, None, None, None);
    }
    if eat("expression ", &mut at) {
        let pattern = Some(&text[at..]).filter(|p| !p.trim().is_empty())?;
        return header(SyntaxKind::Expression, None, Some(pattern), None);
    }

    // `<types> property [condition] <pattern>`. The types are a classinfo
    // list, never a slot or a string.
    let rest = &lower[at..];
    let (types_end, kind, pattern_start) = match rest.find(" property condition ") {
        Some(found) if !plural => (found, SyntaxKind::Condition, found + 20),
        _ => {
            let found = rest.find(" property ")?;
            (found, SyntaxKind::Expression, found + 10)
        }
    };
    let types = text[at..at + types_end].trim();
    if types.is_empty() || types.contains(['%', '"', '{']) {
        return None;
    }
    let pattern = &text[at + pattern_start..];
    if pattern.trim().is_empty() {
        return None;
    }
    header(kind, Some(types.to_string()), Some(pattern), None)
}

/// The patterns a property's uses are matched against.
///
/// skript-reflect registers a property expression the way Skript's own
/// `SimplePropertyExpression` does, and a property condition the way
/// `PropertyCondition` does with the `be` verb.
fn property_patterns(kind: SyntaxKind, types: &str, pattern: &str) -> Vec<String> {
    match kind {
        SyntaxKind::Condition => vec![
            format!("%{types}% (is|are) {pattern}"),
            format!("%{types}% (isn't|is not|aren't|are not) {pattern}"),
        ],
        _ => vec![
            format!("[the] {pattern} of %{types}%"),
            format!("%{types}%'[s] {pattern}"),
        ],
    }
}

/// Entry keys that hold data on their own line rather than open a section.
const VALUE_KEYS: &[&str] = &[
    "pattern",
    "return type",
    "loop of",
    "usable in",
    "event-values",
    "event values",
];

/// Sections whose lines are data (one pattern or one type per line), not code.
const DATA_SECTIONS: &[&str] = &["patterns", "usable in", "event-values", "event values"];

/// Reads a custom syntax definition, when `node` (an `event` from the
/// grammar's point of view) is one.
pub(crate) fn definition(node: Node<'_>, source: &str) -> Option<(Symbol, SyntaxDefinition)> {
    let name = node.child_by_field_name("name")?;
    // The header is read from the start of its line, which takes in the
    // grammar's leading `local` keyword. Anything else before the node (an
    // ERROR from a half-typed line) means this is not a definition header.
    let line_start = source[..node.start_byte()]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let before = &source[line_start..node.start_byte()];
    if !(before.trim().is_empty() || before.trim().eq_ignore_ascii_case("local")) {
        return None;
    }
    let name_end = {
        let mut cursor = node.walk();
        let last = node
            .children_by_field_name("name", &mut cursor)
            .last()
            .unwrap_or(name);
        last.end_byte()
    };
    let header = parse_header(&source[line_start..name_end])?;

    let row = node.start_position().row as u32;
    let selection_range = Range::new(
        Position::new(row, 0),
        Position::new(row, (name_end - line_start) as u32),
    );
    let range = Range::new(Position::new(row, 0), Range::from(node).end);

    let mut definition = SyntaxDefinition {
        kind: header.kind,
        declared: header.pattern.into_iter().collect(),
        patterns: Vec::new(),
        local: header.local,
        property_of: header.property_of,
        event_id: header.event_id,
        event_values: Vec::new(),
        return_type: None,
        comment: comment_above(source, line_start),
        range,
        selection_range,
    };
    let children = read_body(node, source, &mut definition);

    definition.patterns = match &definition.property_of {
        Some(types) => definition
            .declared
            .iter()
            .flat_map(|pattern| property_patterns(definition.kind, types, pattern))
            .collect(),
        None => definition.declared.clone(),
    };

    let symbol = Symbol {
        kind: SymbolKind::Syntax,
        name: definition.name(),
        detail: definition.describe(),
        range,
        selection_range,
        children,
    };
    Some((symbol, definition))
}

/// Walks a definition's body, filling in what its entries say and returning
/// them as outline children.
///
/// Every entry line becomes an `Entry` symbol, the data lines of `patterns:`
/// and `usable in:` included: none of them is a statement, and the server
/// skips entry lines when it looks for syntax it does not recognise.
fn read_body(node: Node<'_>, source: &str, definition: &mut SyntaxDefinition) -> Vec<Symbol> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "section" | "entry_section" => {
                let key = field_source(child, "header", source)
                    .or_else(|| field_source(child, "key", source))
                    .map(|header| first_line(&header))
                    .unwrap_or_default();
                let selection = field_range(child, "header")
                    .or_else(|| field_range(child, "key"))
                    .unwrap_or_else(|| child.into());
                let lower = key.to_ascii_lowercase();
                let children = if DATA_SECTIONS.contains(&lower.as_str()) {
                    data_lines(child, source, &lower, definition)
                } else {
                    body_sections(child, source)
                };
                out.push(entry(key, child.into(), selection, children));
            }
            "statement" | "entry" => {
                let line = code_of(child, source);
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let lower = key.trim().to_ascii_lowercase();
                if !VALUE_KEYS.contains(&lower.as_str()) {
                    continue;
                }
                record(definition, &lower, value.trim());
                let start = Position::new(
                    child.start_position().row as u32,
                    child.start_position().column as u32,
                );
                let selection = Range::new(
                    start,
                    Position::new(start.line, start.character + key.trim_end().len() as u32),
                );
                out.push(entry(
                    key.trim().to_string(),
                    child.into(),
                    selection,
                    Vec::new(),
                ));
            }
            _ => {}
        }
    }
    out
}

/// The lines of a `patterns:` or `usable in:` block, recorded and returned as
/// entries.
fn data_lines(
    section: Node<'_>,
    source: &str,
    key: &str,
    definition: &mut SyntaxDefinition,
) -> Vec<Symbol> {
    let Some(body) = section.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = body.walk();
    for line in body.children(&mut cursor) {
        if line.kind() == "comment" {
            continue;
        }
        let text = code_of(line, source);
        if text.is_empty() {
            continue;
        }
        match key {
            "patterns" => definition.declared.push(text.clone()),
            "usable in" => {}
            _ => record(definition, key, &text),
        }
        let range: Range = line.into();
        let selection = Range::new(
            range.start,
            Position::new(range.start.line, range.start.character + text.len() as u32),
        );
        out.push(entry(text, range, selection, Vec::new()));
    }
    out
}

/// Stores what one `key: value` entry says.
fn record(definition: &mut SyntaxDefinition, key: &str, value: &str) {
    match key {
        "pattern" if !value.is_empty() => definition.declared.push(value.to_string()),
        "return type" if !value.is_empty() => definition.return_type = Some(value.to_string()),
        "event-values" | "event values" => definition.event_values.extend(
            value
                .split(',')
                .flat_map(|part| part.split(" and "))
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string),
        ),
        _ => {}
    }
}

fn entry(name: String, range: Range, selection_range: Range, children: Vec<Symbol>) -> Symbol {
    Symbol {
        kind: SymbolKind::Entry,
        name,
        detail: String::new(),
        range,
        selection_range,
        children,
    }
}

/// A node's text up to any comment it carries, trimmed, on one line.
fn code_of(node: Node<'_>, source: &str) -> String {
    let mut cursor = node.walk();
    let end = node
        .children(&mut cursor)
        .find(|child| child.kind() == "comment")
        .map_or(node.end_byte(), |comment| comment.start_byte());
    first_line(&source[node.start_byte()..end])
}

/// The run of `#` comment lines directly above the line starting at `start`.
fn comment_above(source: &str, start: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in source[..start].lines().rev() {
        let trimmed = line.trim();
        // `###` delimits a block comment, whose prose is not this
        // definition's description.
        if !trimmed.starts_with('#') || trimmed == "###" {
            break;
        }
        out.push(trimmed.trim_start_matches('#').trim().to_string());
    }
    out.reverse();
    // A banner (`# ====`) says nothing about the definition.
    out.retain(|line| line.chars().any(char::is_alphanumeric));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn definitions(source: &str) -> Vec<SyntaxDefinition> {
        Document::new("file:///t.sk", source)
            .symbols()
            .syntax
            .clone()
    }

    #[test]
    fn reads_every_header_form() {
        let effect = parse_header("effect [the] greet %player%:").unwrap();
        assert_eq!(effect.kind, SyntaxKind::Effect);
        assert_eq!(effect.pattern.as_deref(), Some("[the] greet %player%"));
        assert!(!effect.local);

        let local = parse_header("local effect:").unwrap();
        assert!(local.local && local.pattern.is_none());

        let plural = parse_header("plural expression all %strings%:").unwrap();
        assert_eq!(plural.kind, SyntaxKind::Expression);

        let property = parse_header("local players property balance thing:").unwrap();
        assert_eq!(property.kind, SyntaxKind::Expression);
        assert_eq!(property.property_of.as_deref(), Some("players"));
        assert_eq!(property.pattern.as_deref(), Some("balance thing"));

        let condition = parse_header("player property condition cool:").unwrap();
        assert_eq!(condition.kind, SyntaxKind::Condition);

        let event = parse_header("local custom event \"on_greet\":").unwrap();
        assert_eq!(event.event_id.as_deref(), Some("on_greet"));
    }

    #[test]
    fn ordinary_events_are_not_custom_syntax() {
        for header in [
            "on join:",
            "join:",
            "every 5 seconds:",
            "on potion effect:",
            "effect:x",
            "event join:",
            "on effect apply:",
        ] {
            assert_eq!(parse_header(header), None, "{header}");
        }
    }

    #[test]
    fn a_one_line_effect_carries_its_pattern() {
        let found = definitions("effect [the] greet %player%:\n\ttrigger:\n\t\tstop\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].patterns, vec!["[the] greet %player%"]);
        assert_eq!(found[0].selection_range, Range::on_line(0, 0, 27));
    }

    /// `local` is an ERROR node to the grammar; the definition must survive it.
    #[test]
    fn a_local_expression_lists_its_patterns() {
        let source = "local expression:\n\tpatterns:\n\t\t[the] answer\n\t\tanswer to everything # why\n\treturn type: number\n\tget:\n\t\treturn 42\n";
        let found = definitions(source);
        assert_eq!(found.len(), 1);
        let def = &found[0];
        assert!(def.local);
        assert_eq!(def.kind, SyntaxKind::Expression);
        assert_eq!(def.patterns, vec!["[the] answer", "answer to everything"]);
        assert_eq!(def.return_type.as_deref(), Some("number"));
        assert_eq!(def.range.start, Position::new(0, 0));
    }

    #[test]
    fn a_property_expands_into_the_forms_it_is_used_in() {
        let found = definitions("players property balance thing:\n\tget:\n\t\treturn 1\n");
        assert_eq!(
            found[0].patterns,
            vec![
                "[the] balance thing of %players%",
                "%players%'[s] balance thing"
            ]
        );
        let found = definitions("player property condition cool:\n\tcheck:\n\t\tcontinue\n");
        assert_eq!(found[0].kind, SyntaxKind::Condition);
        assert!(found[0].patterns[0].starts_with("%player% (is|are) cool"));
    }

    #[test]
    fn an_event_reads_its_pattern_and_values() {
        let found = definitions(
            "# Fired when someone is greeted.\nevent \"on_greet\":\n\tpattern: greeting [of] %player%\n\tevent-values: player, location\n\tcheck:\n\t\tcontinue\n",
        );
        let def = &found[0];
        assert_eq!(def.kind, SyntaxKind::Event);
        assert_eq!(def.event_id.as_deref(), Some("on_greet"));
        assert_eq!(def.patterns, vec!["greeting [of] %player%"]);
        assert_eq!(def.event_values, vec!["player", "location"]);
        assert_eq!(def.comment, vec!["Fired when someone is greeted."]);
    }

    /// The outline shows the definition, not an event, and every entry line,
    /// pattern lines included, is an entry the diagnostics will skip.
    #[test]
    fn the_outline_shows_the_definition_and_its_entries() {
        let document = Document::new(
            "file:///t.sk",
            "effect:\n\tpatterns:\n\t\twave at %player%\n\tusable in:\n\t\tjoin\n\ttrigger:\n\t\tif 1 is 1:\n\t\t\tstop\n",
        );
        let symbol = &document.symbols().symbols[0];
        assert_eq!(symbol.kind, SymbolKind::Syntax);
        assert_eq!(symbol.name, "wave at %player%");
        assert_eq!(symbol.detail, "custom effect");
        let keys: Vec<&str> = symbol.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(keys, vec!["patterns", "usable in", "trigger"]);
        assert_eq!(symbol.children[0].children[0].name, "wave at %player%");
        assert_eq!(symbol.children[1].children[0].name, "join");
        // Code under `trigger:` keeps its ordinary section nesting.
        assert_eq!(symbol.children[2].children[0].kind, SymbolKind::Section);
    }
}
