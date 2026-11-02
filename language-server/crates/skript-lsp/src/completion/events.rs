//! `event-block` and friends: the values the enclosing event provides.
//!
//! The generic list cannot know these. `event-block` is only meaningful in an
//! event that has a block, and the docs say exactly which values each event
//! carries, so inside `on break:` typing `event-` offers that event's values
//! first rather than every expression whose name happens to contain "event".

use skript_docs::{Catalog, Category, LineRole};
use skript_index::Document;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, TextEdit,
};

use super::ranking::{relevance, sort_key};
use crate::convert::{to_lsp_range, Encoding};
use crate::hover::markdown;
use crate::lines;

/// Lifts an event value above everything in the generic list, whose best
/// possible score is an exact word plus the declared and exact-name bonuses.
const EVENT_VALUE_BONUS: i32 = 2_000;

/// The shortest typed prefix of `event-` that triggers these suggestions.
/// One letter would put them first for every word starting with `e`.
const MIN_TYPED: usize = 2;

/// Event values of the event `line` sits in, when the word being typed is
/// heading for `event-…`.
///
/// `prefix` is the line up to `cursor`, which is in index (byte) columns.
pub fn event_value_items(
    catalog: &Catalog,
    document: &Document,
    cursor: skript_index::Position,
    prefix: &str,
    encoding: Encoding,
) -> Vec<(i32, CompletionItem)> {
    let line = cursor.line;
    let word_start = prefix
        .char_indices()
        .rfind(|(_, ch)| ch.is_whitespace() || matches!(ch, '%' | '(' | ',' | '"' | '{' | '}'))
        .map_or(0, |(at, ch)| at + ch.len_utf8());
    let word = prefix[word_start..].to_ascii_lowercase();
    let heading_there = word.starts_with("event-")
        || (word.len() >= MIN_TYPED && "event-".starts_with(word.as_str()));
    if !heading_there {
        return Vec::new();
    }
    let Some(values) = enclosing_event_values(catalog, document, line) else {
        return Vec::new();
    };

    // `past event-` / `future event-`: the qualifier is part of what is being
    // replaced, so accepting `past event-block` does not leave `past past`.
    let before = prefix[..word_start].to_ascii_lowercase();
    let qualifier = ["past ", "future "]
        .into_iter()
        .find(|qualifier| before.ends_with(qualifier));
    let replace_from = word_start - qualifier.map_or(0, str::len);
    let range = to_lsp_range(
        document.text(),
        skript_index::Range::new(
            skript_index::Position::new(line, replace_from as u32),
            cursor,
        ),
        encoding,
    );
    let typed = &prefix[replace_from..];

    let mut seen = Vec::new();
    let mut out = Vec::new();
    for (label, type_name) in values {
        if seen.contains(&label) {
            continue;
        }
        seen.push(label.clone());
        // Only what the qualifier allows: after `past `, the past values.
        if let Some(qualifier) = qualifier {
            if !label.starts_with(qualifier) {
                continue;
            }
        }
        let score = relevance(&label, typed).max(1) + EVENT_VALUE_BONUS;
        out.push((
            score,
            CompletionItem {
                label: label.clone(),
                kind: Some(CompletionItemKind::VALUE),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some("event value".into()),
                }),
                detail: Some(format!("event value · {type_name}")),
                documentation: Some(Documentation::MarkupContent(markdown(format!(
                    "**{label}**  \nThe {type_name} this event provides."
                )))),
                filter_text: Some(label.clone()),
                sort_text: Some(sort_key(score, &label)),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: label,
                })),
                ..Default::default()
            },
        ));
    }
    out
}

/// The `event-…` labels the enclosing event documents, each with its type.
///
/// Skript's database names a value by its type (`block`, `past number`);
/// SkriptHub writes the expression itself (`event-player`). Both end up as
/// `event-block`, `past event-number`, `future event-x`.
pub fn enclosing_event_values(
    catalog: &Catalog,
    document: &Document,
    line: u32,
) -> Option<Vec<(String, String)>> {
    // The header is the nearest column-0 line above, found by indentation and
    // not the parse tree: `send "%ev` has an unclosed string, which is exactly
    // the moment this runs, and it was enough to cut the event's node short.
    let header = (0..line).rev().find_map(|at| {
        let raw = document.line(at);
        let code = lines::code_part(raw);
        (!code.trim().is_empty() && !raw.starts_with(char::is_whitespace)).then_some(code)
    })?;
    let (id, _) = catalog.classify_line(header.trim(), LineRole::TopLevel)?;
    if id.category != Category::Event {
        return None;
    }
    let entry = catalog.entry(id)?;
    let values: Vec<(String, String)> = entry
        .event_values
        .iter()
        .filter_map(|value| label_for(value.display()))
        .collect();
    (!values.is_empty()).then_some(values)
}

/// `block` → `event-block`, `past number` → `past event-number`,
/// `event-player` → `event-player`. The second element is the bare type.
fn label_for(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim().to_ascii_lowercase();
    let (qualifier, rest) = ["past ", "future "]
        .into_iter()
        .find_map(|qualifier| raw.strip_prefix(qualifier).map(|rest| (qualifier, rest)))
        .unwrap_or(("", raw.as_str()));
    let name = rest.trim().trim_start_matches("event-").trim();
    if name.is_empty() {
        return None;
    }
    Some((format!("{qualifier}event-{name}"), name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_docs::{Docs, Entry, Reference};
    use skript_index::Workspace;

    fn value(name: &str) -> Reference {
        Reference {
            id: name.into(),
            name: name.into(),
        }
    }

    /// A catalog holding a break event with Skript-style values and an addon
    /// event with SkriptHub-style ones.
    fn catalog() -> Catalog {
        let mut docs = Docs::default();
        docs.push(
            Category::Event,
            Entry {
                id: "SimpleEvent-Break".into(),
                name: "Break / Mine".into(),
                patterns: vec!["[block] (break[ing]|min(e|ing))".into()],
                event_values: vec![value("block"), value("player"), value("past number")],
                ..Default::default()
            },
        );
        docs.push(
            Category::Event,
            Entry {
                id: "skessentials:afk".into(),
                name: "On Essentials Jail".into(),
                patterns: vec!["essentials jail".into()],
                event_values: vec![value("event-player"), value("event-text")],
                addon: Some(Default::default()),
                ..Default::default()
            },
        );
        Catalog::build(docs)
    }

    fn labels(source: &str, line: u32, prefix: &str) -> Vec<String> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let cursor = skript_index::Position::new(line, prefix.len() as u32);
        let mut items = event_value_items(&catalog(), document, cursor, prefix, Encoding::Utf16);
        items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.label.cmp(&b.1.label)));
        items.into_iter().map(|(_, item)| item.label).collect()
    }

    #[test]
    fn offers_the_enclosing_events_values() {
        let source = "on break:\n\tsend event-\n";
        let found = labels(source, 1, "\tsend event-");
        assert_eq!(
            found,
            vec!["event-block", "event-player", "past event-number"]
        );
    }

    #[test]
    fn an_addon_events_prefixed_values_are_not_doubled() {
        let source = "on essentials jail:\n\tsend \"%ev\n";
        let found = labels(source, 1, "\tsend \"%ev");
        assert_eq!(found, vec!["event-player", "event-text"]);
    }

    #[test]
    fn past_narrows_to_past_values_and_is_replaced_with_them() {
        let source = "on break:\n\tsend past event-\n";
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let prefix = "\tsend past event-";
        let items = event_value_items(
            &catalog(),
            document,
            skript_index::Position::new(1, prefix.len() as u32),
            prefix,
            Encoding::Utf16,
        );
        assert_eq!(items.len(), 1);
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].1.text_edit else {
            panic!("no edit");
        };
        assert_eq!(edit.range.start.character, 6, "the `past ` is replaced too");
        assert_eq!(edit.new_text, "past event-number");
    }

    #[test]
    fn nothing_outside_an_event_or_for_unrelated_words() {
        assert!(labels("function f():\n\tsend event-\n", 1, "\tsend event-").is_empty());
        assert!(labels("on break:\n\tsend e\n", 1, "\tsend e").is_empty());
        assert!(labels("on break:\n\tsend player\n", 1, "\tsend player").is_empty());
    }
}
