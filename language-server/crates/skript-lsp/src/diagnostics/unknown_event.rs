//! An event header that names no event, when it is a near-miss of one:
//! `on jion:` for `on join:`.
//!
//! Skript refuses such a header with "Can't understand this structure". The
//! catalog can only say an event is unknown when it knows every event the
//! server has, so the caller runs this only once the core database is loaded
//! and every detected addon's syntax is in the catalog. Even then a header is
//! reported only when it is a typo of an event's complete wording (one edit
//! for a short word, two for a longer one, a swap of two letters counting as
//! one), which an unknown addon's event is very unlikely to be:
//!
//! * only headers written with `on`, the one word that says "this is an
//!   event";
//! * skipped when anything explains it: a core, addon or project-defined
//!   event, a structure, or an addon the server does not have (which
//!   `requires-addon` already names).

use skript_docs::{Catalog, Category, EntryId, LineRole};
use skript_index::{Document, Range, SymbolKind};

use super::event_values::event_span;
use super::layout::Lines;
use super::{Diagnostic, Severity};

pub(super) const CODE: &str = "unknown-event";

/// The generic event structure, `[on] … <.+> …`, which every top-level line
/// matches. Landing on it means no specific event did.
const CATCH_ALL: &str = "StructEvent";

/// Expansions kept per pattern; the rest are dropped rather than enumerated.
const EXPANSION_LIMIT: usize = 64;

/// Whether `id` names something specific rather than the catch-all.
fn is_specific(catalog: &Catalog, id: EntryId) -> bool {
    match id.category {
        Category::Event => true,
        Category::Structure => catalog.entry(id).is_some_and(|entry| entry.id != CATCH_ALL),
        _ => false,
    }
}

fn explains(catalog: &Catalog, header: &str) -> bool {
    catalog
        .classify_line(header, LineRole::TopLevel)
        .is_some_and(|(id, _)| is_specific(catalog, id))
}

/// Every complete wording of a pattern: its optional parts in or out, one
/// branch of each choice, and nothing that needs a `%slot%` or `<regex>`.
/// Parse tags (`1:`, `:uncancelled`) are dropped.
fn expansions(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut position = 0;
    let found = sequence(&chars, &mut position, false);
    let mut out: Vec<String> = found
        .into_iter()
        .map(|text| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|text| !text.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Parses until the end, a `]`, a `)` or (inside a group) a `|`. Returns the
/// wordings of what it read; empty when every one needs a slot.
fn sequence(chars: &[char], position: &mut usize, in_group: bool) -> Vec<String> {
    let mut wordings = vec![String::new()];
    let mut word = String::new();
    let extend = |wordings: Vec<String>, tails: &[String]| -> Vec<String> {
        let mut next = Vec::new();
        for head in &wordings {
            for tail in tails {
                if next.len() < EXPANSION_LIMIT {
                    next.push(format!("{head}{tail}"));
                }
            }
        }
        next
    };
    while let Some(&ch) = chars.get(*position) {
        match ch {
            ']' | ')' => break,
            '|' if in_group => break,
            '\\' => {
                if let Some(&escaped) = chars.get(*position + 1) {
                    word.push(escaped);
                }
                *position += 2;
                continue;
            }
            // A tag ends at `:`; what came before it in this word is a name.
            ':' => {
                let keep = word
                    .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
                    .len();
                word.truncate(keep);
            }
            '[' | '(' => {
                wordings = extend(wordings, &[std::mem::take(&mut word)]);
                *position += 1;
                let mut branches = Vec::new();
                loop {
                    branches.extend(sequence(chars, position, true));
                    match chars.get(*position) {
                        Some('|') => *position += 1,
                        _ => break,
                    }
                }
                *position += 1; // the closing bracket
                if ch == '[' {
                    branches.push(String::new());
                }
                wordings = extend(wordings, &branches);
                if wordings.is_empty() {
                    skip_rest(chars, position, in_group);
                    return Vec::new();
                }
                continue;
            }
            '%' | '<' => {
                // A slot or regex makes this wording incomplete. Skip to its
                // end so the rest of the group still parses.
                let close = if ch == '%' { '%' } else { '>' };
                *position += 1;
                while chars.get(*position).is_some_and(|&c| c != close) {
                    *position += 1;
                }
                *position += 1;
                skip_rest(chars, position, in_group);
                return Vec::new();
            }
            _ => word.push(ch),
        }
        *position += 1;
    }
    extend(wordings, &[word])
}

/// Moves past the rest of the current sequence, nested groups included.
fn skip_rest(chars: &[char], position: &mut usize, in_group: bool) {
    let mut depth = 0usize;
    while let Some(&ch) = chars.get(*position) {
        match ch {
            '\\' => *position += 1,
            '[' | '(' => depth += 1,
            ']' | ')' if depth == 0 => return,
            ']' | ')' => depth -= 1,
            '|' if depth == 0 && in_group => return,
            _ => {}
        }
        *position += 1;
    }
}

/// Optimal string alignment distance: Levenshtein, plus a swap of two
/// neighbouring letters as one edit.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in table.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in table[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (table[i - 1][j] + 1)
                .min(table[i][j - 1] + 1)
                .min(table[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(table[i - 2][j - 2] + 1);
            }
            table[i][j] = best;
        }
    }
    table[a.len()][b.len()]
}

/// The event wordings nearest `bare`, when one is near enough to be a typo.
pub(crate) fn suggestions(catalog: &Catalog, bare: &str) -> Vec<String> {
    let typed = bare
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if typed.is_empty() {
        return Vec::new();
    }
    let limit = if typed.chars().count() <= 4 { 1 } else { 2 };
    let base = catalog.docs().entries(Category::Event).iter();
    let own = catalog
        .workspace_entries()
        .filter(|(id, _)| id.category == Category::Event);
    let mut best: Option<(usize, Vec<String>)> = None;

    for entry in base.chain(own.map(|(_, entry)| entry)) {
        for pattern in &entry.patterns {
            for wording in expansions(pattern) {
                if wording.len().abs_diff(typed.len()) > limit {
                    continue;
                }
                let found = distance(&typed, &wording);
                if found == 0 || found > limit {
                    continue;
                }
                match &mut best {
                    Some((closest, list)) if found == *closest => {
                        if !list.contains(&wording) {
                            list.push(wording);
                        }
                    }
                    Some((closest, _)) if found > *closest => {}
                    _ => best = Some((found, vec![wording])),
                }
            }
        }
    }
    let mut list = best.map(|(_, list)| list).unwrap_or_default();
    list.sort();
    list.truncate(3);
    list
}

pub(super) fn check(
    document: &Document,
    catalog: &Catalog,
    uninstalled: Option<&Catalog>,
    out: &mut Vec<Diagnostic>,
) {
    let lines = Lines::new(document.text());
    for symbol in &document.symbols().symbols {
        if symbol.kind != SymbolKind::Event {
            continue;
        }
        let number = symbol.range.start.line as usize;
        let Some(code) = lines.code(number) else {
            continue;
        };
        let header = code.trim_end().trim_end_matches(':').trim_end();
        if skript_index::indent_width(header) != 0
            || !header.to_ascii_lowercase().starts_with("on ")
            || explains(catalog, header)
            || uninstalled.is_some_and(|rest| explains(rest, header))
        {
            continue;
        }
        let (start, end) = event_span(header);
        let Some(suggestion) = suggestions(catalog, &header[start..end]).into_iter().next() else {
            continue;
        };
        out.push(Diagnostic {
            range: Range::on_line(number as u32, start as u32, end as u32),
            severity: Severity::Warning,
            message: format!(
                "Can't understand this structure: {header}. Did you mean `on {suggestion}`?"
            ),
            code: CODE,
            related: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_index::Workspace;

    const DOCS: &str = r#"{
      "structures": [
        { "id": "StructEvent", "name": "Event", "patterns": ["[on] [uncancelled|cancelled|(any|all)] <.+> [with priority ((lowest|low|normal|high|highest|monitor))]"] },
        { "id": "StructCommand", "name": "Command", "patterns": ["command <.+>"] }
      ],
      "events": [
        { "id": "join", "name": "On Join", "patterns": ["[player] (join|log[ ]in)[ing]"] },
        { "id": "first_join", "name": "On First Join", "patterns": ["first (join|login)"] },
        { "id": "quit", "name": "On Quit", "patterns": ["(quit[ting]|disconnect[ing]|log[ ]out|logging out|leav(e|ing))"] },
        { "id": "damage", "name": "On Damage", "patterns": ["damag(e|ing) [of %entitydata%] [by %entitydata%]"] },
        { "id": "tp", "name": "On Teleport", "patterns": ["[%entitydata%] teleport[ing]"] }
      ]
    }"#;

    fn catalog() -> Catalog {
        Catalog::build(skript_docs::Docs::parse(DOCS).unwrap())
    }

    fn found(source: &str) -> Vec<Diagnostic> {
        let catalog = catalog();
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let mut out = Vec::new();
        check(
            workspace.get("file:///t.sk").unwrap(),
            &catalog,
            None,
            &mut out,
        );
        out
    }

    #[test]
    fn expansions_cover_optionals_choices_and_glued_parts() {
        assert_eq!(
            expansions("[player] (join|log[ ]in)[ing]"),
            vec![
                "join",
                "joining",
                "log in",
                "log ining",
                "login",
                "logining",
                "player join",
                "player joining",
                "player log in",
                "player log ining",
                "player login",
                "player logining"
            ]
        );
        assert_eq!(
            expansions("damag(e|ing) [of %entitydata%]"),
            vec!["damage", "damaging"]
        );
        assert_eq!(
            expansions("[%entitydata%] teleport[ing]"),
            vec!["teleport", "teleporting"]
        );
        assert!(expansions("every %timespan%").is_empty());
        // A branch that needs a slot drops out; its siblings still parse.
        assert_eq!(expansions("(a %x%|b) c"), vec!["b c"]);
        assert_eq!(expansions("(((x %y%) z)|w) v"), vec!["w v"]);
        assert_eq!(
            expansions("(exit|stop) [1:loop]"),
            vec!["exit", "exit loop", "stop", "stop loop"]
        );
    }

    #[test]
    fn swapped_letters_are_one_edit() {
        assert_eq!(distance("jion", "join"), 1);
        assert_eq!(distance("kitten", "sitting"), 3);
    }

    #[test]
    fn a_typo_of_an_event_is_a_warning_with_the_fix_named() {
        let out = found("on jion:\n\tstop\n");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].severity, Severity::Warning);
        assert_eq!(out[0].range, Range::on_line(0, 3, 7));
        assert_eq!(
            out[0].message,
            "Can't understand this structure: on jion. Did you mean `on join`?"
        );
        assert_eq!(found("on frist join:\n\tstop\n").len(), 1);
        assert_eq!(
            found("on cancelled damagee with priority high:\n\tstop\n")[0].range,
            Range::on_line(0, 13, 20)
        );
    }

    #[test]
    fn known_or_distant_headers_are_silent() {
        for header in [
            "on join",
            "on first login",
            "on damage of zombie",
            "on zombie teleport",
            "on something entirely unknown",
            // One letter short of a four-letter word is two edits away.
            "on jo",
            "command /home",
            "every 5 seconds",
            // Written without `on`, nothing says it is an event.
            "jion",
        ] {
            let source = format!("{header}:\n\tstop\n");
            assert!(
                found(&source).is_empty(),
                "{header:?}: {:?}",
                found(&source)
            );
        }
    }

    #[test]
    fn an_uninstalled_addon_event_is_left_to_requires_addon() {
        let catalog = catalog();
        let mut rest = skript_docs::Docs::parse(
            r#"{ "events": [{ "id": "jian", "name": "On Jian", "patterns": ["jian"] }] }"#,
        )
        .unwrap();
        rest.attribute_to(&skript_docs::model::AddonRef {
            name: "SomeAddon".into(),
            ..Default::default()
        });
        let rest = Catalog::build(rest);
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", "on jian:\n\tstop\n");
        let mut out = Vec::new();
        check(
            workspace.get("file:///t.sk").unwrap(),
            &catalog,
            Some(&rest),
            &mut out,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn suggestions_name_the_nearest_wording() {
        assert_eq!(suggestions(&catalog(), "jion"), vec!["join"]);
        assert_eq!(suggestions(&catalog(), "quitt"), vec!["quit"]);
        assert!(suggestions(&catalog(), "nothing like it").is_empty());
    }
}
