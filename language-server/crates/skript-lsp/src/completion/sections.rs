//! Entries of addon sections: `result:` and `shape:` inside SkBee's
//! `register shaped recipe:`.
//!
//! SkriptHub records which entries 87 sections and structures take, so these
//! come from the catalog rather than a table here. The record says only each
//! entry's name, whether it is required, whether it opens a section and its
//! default; that is all the hover card can honestly say.

use skript_docs::{Catalog, Entry, EntryId, LineRole, SectionEntry};
use skript_index::{indent_width, Document};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Documentation, InsertTextFormat,
};

use super::ranking::{relevance, sort_key};
use crate::hover::markdown;
use crate::lines;

/// Lifts a required entry the section does not have yet above the rest.
const MISSING_REQUIRED_BONUS: i32 = 200;

/// The section whose entries belong on `line`, when its header is syntax that
/// documents entries.
///
/// The parent is the nearest line above with less indentation, which is the
/// header the line belongs to. Read from indentation rather than the parse
/// tree, because a line still being typed is exactly when the tree is least
/// reliable. `indent` is the line's own indentation.
pub fn enclosing_section<'c>(
    catalog: &'c Catalog,
    document: &Document,
    line: u32,
    indent: usize,
) -> Option<(EntryId, &'c Entry, u32)> {
    if indent == 0 {
        return None;
    }
    let (header_line, header) = (0..line).rev().find_map(|at| {
        let raw = document.line(at);
        let code = lines::code_part(raw);
        if code.trim().is_empty() {
            return None;
        }
        (indent_width(raw) < indent).then_some((at, raw))
    })?;
    let code = lines::code_part(header).trim();
    if !code.ends_with(':') {
        return None;
    }
    let role = LineRole::from_indent(indent_width(header));
    let (id, _) = catalog.classify_line(code, role)?;
    let entry = catalog.entry(id)?;
    (!entry.section_entries.is_empty()).then_some((id, entry, header_line))
}

/// Whether `line` is one of the entries its enclosing addon section
/// documents, `result: diamond` or a section entry like `ingredients:`.
///
/// The index already marks `key: value` lines as entries by their shape, but
/// an entry that opens a block of its own parses as a section, and was
/// reported as unknown syntax.
pub fn is_documented_entry(catalog: &Catalog, document: &Document, line: u32) -> bool {
    let raw = document.line(line);
    let Some((key, _)) = lines::code_part(raw).trim().split_once(':') else {
        return false;
    };
    let key = key.trim();
    enclosing_section(catalog, document, line, indent_width(raw)).is_some_and(|(_, entry, _)| {
        entry
            .section_entries
            .iter()
            .any(|documented| documented.name.eq_ignore_ascii_case(key))
    })
}

/// Completion items for the entries `section` takes.
///
/// Entries already written in the section sink, and required ones still
/// missing rise: what is left to write is what is wanted.
pub fn entry_items(
    document: &Document,
    section: &Entry,
    header_line: u32,
    line: u32,
    typed: &str,
) -> Vec<(i32, CompletionItem)> {
    let present = present_keys(document, header_line, line);
    section
        .section_entries
        .iter()
        .map(|entry| {
            let written = present
                .iter()
                .any(|key| key.eq_ignore_ascii_case(&entry.name));
            let base = relevance(&entry.name, typed);
            let score = match (written, entry.required) {
                _ if base == 0 => 0,
                (true, _) => 1,
                (false, true) => base + MISSING_REQUIRED_BONUS,
                (false, false) => base,
            };
            let item = CompletionItem {
                label: format!("{}:", entry.name),
                kind: Some(CompletionItemKind::PROPERTY),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(qualities(entry)),
                }),
                detail: Some(format!("{} entry", section.provider())),
                documentation: Some(Documentation::MarkupContent(markdown(entry_card(
                    section, entry,
                )))),
                filter_text: Some(entry.name.clone()),
                insert_text: Some(if entry.section {
                    format!("{}:\n\t$0", entry.name)
                } else {
                    format!("{}: $0", entry.name)
                }),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                sort_text: Some(sort_key(score, &entry.name)),
                ..Default::default()
            };
            (score, item)
        })
        .collect()
}

/// The hover card for the entry on `line`, when it is one of its section's.
pub fn entry_hover(catalog: &Catalog, document: &Document, line: u32) -> Option<String> {
    let raw = document.line(line);
    let key = skript_index::symbols::entry_key(lines::code_part(raw))?;
    let (_, section, _) = enclosing_section(catalog, document, line, indent_width(raw))?;
    let entry = section.section_entry(key)?;
    Some(entry_card(section, entry))
}

/// `required · section`, as the completion list's right-hand column.
fn qualities(entry: &SectionEntry) -> String {
    let mut parts = vec![if entry.required {
        "required"
    } else {
        "optional"
    }];
    if entry.section {
        parts.push("section");
    }
    parts.join(" · ")
}

fn entry_card(section: &Entry, entry: &SectionEntry) -> String {
    let mut out = format!(
        "**{}** · entry of *{}* ({})\n\n",
        entry.name,
        section.name,
        section.provider()
    );
    out.push_str(if entry.required {
        "Required."
    } else {
        "Optional."
    });
    out.push_str(if entry.section {
        " Opens a section: its lines go underneath, indented."
    } else {
        " Takes its value on the same line."
    });
    if let Some(default) = &entry.default_value {
        out.push_str(&format!(" Defaults to `{default}`."));
    }
    let all: Vec<String> = section
        .section_entries
        .iter()
        .map(|other| {
            let mark = if other.required { "" } else { "?" };
            format!("`{}{mark}`", other.name)
        })
        .collect();
    out.push_str(&format!("\n\nEntries: {}", all.join(", ")));
    out
}

/// The keys already written directly in the section opened on `header_line`,
/// other than on `line` itself.
fn present_keys(document: &Document, header_line: u32, line: u32) -> Vec<String> {
    let header_indent = indent_width(document.line(header_line));
    let mut keys = Vec::new();
    let mut entry_indent = None;
    let total = document.text().lines().count() as u32;
    for at in header_line + 1..total {
        let raw = document.line(at);
        if lines::code_part(raw).trim().is_empty() {
            continue;
        }
        let indent = indent_width(raw);
        if indent <= header_indent {
            break;
        }
        let first = *entry_indent.get_or_insert(indent);
        if indent != first || at == line {
            continue;
        }
        if let Some(key) = skript_index::symbols::entry_key(lines::code_part(raw)) {
            keys.push(key.to_string());
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_docs::{Category, Docs};
    use skript_index::Workspace;

    fn catalog() -> Catalog {
        let mut docs = Docs::default();
        let entry = |name: &str, required: bool, section: bool| SectionEntry {
            name: name.into(),
            required,
            section,
            default_value: None,
        };
        docs.push(
            Category::Section,
            Entry {
                id: "skbee:shaped".into(),
                name: "Recipe - Register Shaped Recipe".into(),
                patterns: vec!["register [a] [new] shaped recipe".into()],
                addon: Some(skript_docs::model::AddonRef {
                    name: "SkBee".into(),
                    ..Default::default()
                }),
                section_entries: vec![
                    entry("id", true, false),
                    entry("result", true, false),
                    entry("group", false, false),
                    entry("ingredients", true, true),
                ],
                ..Default::default()
            },
        );
        Catalog::build(docs)
    }

    const SOURCE: &str = concat!(
        "on load:\n",
        "\tregister shaped recipe:\n",
        "\t\tid: \"custom:sword\"\n",
        "\t\t\n",
        "\t\tingredients:\n",
        "\t\t\tset {_x} to 1\n",
    );

    /// `ingredients:` opens a block, so it parses as a section rather than
    /// an entry, and was the one recipe line reported as unknown syntax.
    #[test]
    fn a_documented_section_entry_is_recognised() {
        let catalog = catalog();
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", SOURCE);
        let document = workspace.get("file:///t.sk").unwrap();
        assert!(is_documented_entry(&catalog, document, 2), "id: …");
        assert!(is_documented_entry(&catalog, document, 4), "ingredients:");
        assert!(!is_documented_entry(&catalog, document, 5), "a statement");
        assert!(
            !is_documented_entry(&catalog, document, 1),
            "the section header"
        );
    }

    #[test]
    fn finds_the_section_a_line_belongs_to() {
        let catalog = catalog();
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", SOURCE);
        let document = workspace.get("file:///t.sk").unwrap();

        let (_, section, header) = enclosing_section(&catalog, document, 3, 2).unwrap();
        assert_eq!(section.name, "Recipe - Register Shaped Recipe");
        assert_eq!(header, 1);
        // Inside `ingredients:`, the parent is that entry, not the recipe.
        assert!(enclosing_section(&catalog, document, 5, 3).is_none());
        // The section header itself sits in the event.
        assert!(enclosing_section(&catalog, document, 1, 1).is_none());
    }

    #[test]
    fn missing_required_entries_rise_and_written_ones_sink() {
        let catalog = catalog();
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", SOURCE);
        let document = workspace.get("file:///t.sk").unwrap();
        let (_, section, header) = enclosing_section(&catalog, document, 3, 2).unwrap();

        let mut items = entry_items(document, section, header, 3, "");
        items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.label.cmp(&b.1.label)));
        let labels: Vec<&str> = items.iter().map(|(_, item)| item.label.as_str()).collect();
        // `result` is required and missing; `id` and `ingredients` are written.
        assert_eq!(labels, vec!["result:", "group:", "id:", "ingredients:"]);
        let ingredients = &items[3].1;
        assert_eq!(
            ingredients.insert_text.as_deref(),
            Some("ingredients:\n\t$0")
        );
    }

    #[test]
    fn an_entry_line_hovers_with_what_skripthub_knows() {
        let catalog = catalog();
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", SOURCE);
        let document = workspace.get("file:///t.sk").unwrap();
        let card = entry_hover(&catalog, document, 2).expect("a card");
        assert!(
            card.starts_with("**id** · entry of *Recipe - Register Shaped Recipe* (SkBee)"),
            "{card}"
        );
        assert!(card.contains("Required."));
        assert!(card.contains("`group?`"), "{card}");
        assert!(entry_hover(&catalog, document, 5).is_none());
    }
}
