//! `cancel event` in an event that cannot be cancelled.
//!
//! Skript rejects this at load time ("An on join event cannot be
//! cancelled"), so it is certainly wrong. It is still only a warning here,
//! because the verdict rests on the catalog's `cancellable` flag, and the
//! check only trusts that flag where it is known to be explicit:
//!
//! * the header resolves to exactly one event, and that event comes from a
//!   Skript `docs.json`, whose generator writes `cancellable` on every event.
//!   SkriptHub's records and addon catalogs may simply omit it, and a missing
//!   flag reads as `false`;
//! * every section between the event and the line is core control flow. An
//!   addon section (a GUI slot's click handler) can run its body in another
//!   event that *is* cancellable.

use skript_docs::{Catalog, Category, Entry, EntryId, LineRole};
use skript_index::{Document, Range, SymbolKind};

use super::layout::{is_control_flow, Lines};
use super::{Diagnostic, Severity};

pub(super) const CODE: &str = "event-not-cancellable";

/// Whether an entry's `cancellable` flag is known to be explicit. Syntax the
/// project defines itself (a skript-reflect custom event) has none.
fn from_skript_docs(catalog: &Catalog, id: EntryId, entry: &Entry) -> bool {
    entry.addon.is_none() && entry.skripthub_id.is_none() && !catalog.is_workspace(id)
}

/// The event `header` names, when it names exactly one non-cancellable event.
fn uncancellable_event<'c>(catalog: &'c Catalog, header: &str) -> Option<&'c Entry> {
    let (id, _) = catalog.classify_line(header, LineRole::from_indent(0))?;
    if id.category != Category::Event {
        return None;
    }
    let entry = catalog.entry(id)?;
    if entry.cancellable || !from_skript_docs(catalog, id, entry) {
        return None;
    }
    // Another event whose pattern also fits would make the verdict depend on
    // ranking. `on` is part of Skript's event wrapper, not of any pattern.
    let bare = header
        .get(..3)
        .filter(|start| start.eq_ignore_ascii_case("on "))
        .map_or(header, |_| &header[3..]);
    let ambiguous = catalog.classify(bare).into_iter().any(|(other, matched)| {
        other != id
            && other.category == Category::Event
            && matched.explains_text(bare)
            && catalog
                .entry(other)
                .is_some_and(|entry| entry.cancellable || !from_skript_docs(catalog, other, entry))
    });
    (!ambiguous).then_some(entry)
}

fn is_cancel_event(code: &str) -> bool {
    let words: Vec<&str> = code.split_whitespace().collect();
    matches!(words.as_slice(), [cancel, event] | [cancel, _, event]
        if cancel.eq_ignore_ascii_case("cancel")
            && event.eq_ignore_ascii_case("event")
            && (words.len() == 2 || words[1].eq_ignore_ascii_case("the")))
}

pub(super) fn check(document: &Document, catalog: &Catalog, out: &mut Vec<Diagnostic>) {
    let lines = Lines::new(document.text());

    for event in &document.symbols().symbols {
        if event.kind != SymbolKind::Event {
            continue;
        }
        let start = event.range.start.line as usize;
        let Some(header) = lines.code(start) else {
            continue;
        };
        let header = header.trim().trim_end_matches(':').trim_end();
        let mut found: Option<&Entry> = None;

        for number in start + 1..=event.range.end.line as usize {
            let Some(code) = lines.code(number) else {
                continue;
            };
            if !is_cancel_event(code) {
                continue;
            }
            let headers = lines.enclosing_headers(number);
            let nested_only_in_control_flow = headers
                .split_last()
                .is_some_and(|(_, inner)| inner.iter().all(|h| is_control_flow(h)));
            if !nested_only_in_control_flow {
                continue;
            }
            // Classified once per event, and only if it has a `cancel event`.
            let entry = match found {
                Some(entry) => entry,
                None => match uncancellable_event(catalog, header) {
                    Some(entry) => *found.insert(entry),
                    None => break,
                },
            };
            let indent = skript_index::indent_width(code) as u32;
            out.push(Diagnostic {
                range: Range::on_line(number as u32, indent, indent + code.trim().len() as u32),
                severity: Severity::Warning,
                message: format!(
                    "This event cannot be cancelled: `{}` is not cancellable, so Skript \
                     refuses `cancel event` here.",
                    entry.name
                ),
                code: CODE,
                related: Vec::new(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_index::Workspace;

    const EVENTS: &str = r#"{
      "events": [
        { "id": "join", "name": "Join", "patterns": ["[player] join[ing]"], "cancellable": false },
        { "id": "chat", "name": "Chat", "patterns": ["chat"], "cancellable": true }
      ]
    }"#;

    fn found(source: &str) -> Vec<Diagnostic> {
        let catalog = Catalog::build(skript_docs::Docs::parse(EVENTS).unwrap());
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let mut out = Vec::new();
        check(document, &catalog, &mut out);
        out
    }

    #[test]
    fn cancelling_an_uncancellable_event_is_a_warning() {
        let out = found("on join:\n\tif player is op:\n\t\tcancel the event\n");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].severity, Severity::Warning);
        assert_eq!(out[0].range, Range::on_line(2, 2, 18));
        assert!(out[0].message.contains("`Join`"));
    }

    #[test]
    fn a_cancellable_event_is_fine() {
        assert!(found("on chat:\n\tcancel event\n").is_empty());
    }

    /// A GUI slot handler, or any addon section, can run in another event.
    #[test]
    fn an_addon_section_hides_the_line() {
        assert!(found(
            "on join:\n\tformat slot 1 of player with stone to run:\n\t\tcancel event\n"
        )
        .is_empty());
    }

    #[test]
    fn only_the_cancel_effect_itself_is_matched() {
        let out = found("on join:\n\tsend \"cancel event\"\n\tuncancel event\n\t# cancel event\n");
        assert!(out.is_empty(), "{out:?}");
    }

    /// SkriptHub and addon records may omit the flag, which reads as `false`.
    #[test]
    fn an_addon_event_is_never_judged() {
        let mut docs = skript_docs::Docs::parse(EVENTS).unwrap();
        docs.attribute_to(&skript_docs::model::AddonRef {
            name: "SomeAddon".into(),
            ..Default::default()
        });
        let catalog = Catalog::build(docs);
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", "on join:\n\tcancel event\n");
        let mut out = Vec::new();
        check(workspace.get("file:///t.sk").unwrap(), &catalog, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}
