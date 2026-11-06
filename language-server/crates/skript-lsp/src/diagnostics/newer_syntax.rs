//! Syntax that needs a newer Skript than the server runs.
//!
//! On an older Skript the line is simply not registered, so it fails as
//! "Can't understand this condition/effect". Only reported when the target
//! version is the server's own (the detected `Skript.jar` or the
//! `skriptVersion` setting, decided by the caller), never the version of the
//! database that happened to load. Even then it stays quiet unless it is sure:
//!
//! * the line's syntax is core Skript, from a database whose `since` values
//!   are Skript versions. An addon's `since` is the addon's own version, and
//!   a version newer than the loaded database itself cannot be Skript's;
//! * no other syntax old enough for the target explains the line too. A new
//!   pattern often overlaps an old one, and Skript would then pick the old;
//! * versions compare by number only, so a `2.10.0-pre1` build is not told
//!   it lacks 2.10 syntax.
//!
//! What it cannot see is syntax a later Skript rewrote: the current database
//! no longer holds the old entry. Play or Draw an Effect is `since 2.14`, but
//! the Play Effect it replaced took `play flame at player` long before, and
//! run against Skript's own scripts with a 2.10 target this check calls 39
//! such lines new. So it is opt-in (`newerSyntaxDiagnostics`) until the
//! target version's own database can be consulted instead.

use skript_docs::version::Version;
use skript_docs::{Catalog, Category, Entry, EntryId, LineRole};
use skript_index::{Document, Range};

use super::{Diagnostic, Severity};
use crate::lines;

pub(super) const CODE: &str = "requires-newer-skript";

fn number(version: Version) -> (u32, u32, u32) {
    (version.major, version.minor, version.patch)
}

fn is_core(catalog: &Catalog, id: EntryId, entry: &Entry) -> bool {
    entry.addon.is_none() && entry.skripthub_id.is_none() && !catalog.is_workspace(id)
}

/// Whether the target Skript has this entry: always for anything that is not
/// core, whose `since` says nothing about Skript.
fn available(catalog: &Catalog, id: EntryId, entry: &Entry, target: Version) -> bool {
    !is_core(catalog, id, entry)
        || entry
            .min_version
            .is_none_or(|minimum| number(minimum) <= number(target))
}

/// `line` without the keyword that wraps the syntax it names, and the
/// categories that may explain what is left.
fn unwrapped(line: &str, role: LineRole) -> (&str, &'static [Category]) {
    let lower = line.to_ascii_lowercase();
    match role {
        LineRole::TopLevel => match lower.strip_prefix("on ") {
            Some(_) => (line[3..].trim(), &[Category::Event]),
            None => (line, &[Category::Structure, Category::Event]),
        },
        _ => {
            for keyword in ["do while ", "else if ", "while ", "if "] {
                if lower.starts_with(keyword) {
                    return (line[keyword.len()..].trim(), &[Category::Condition]);
                }
            }
            (
                line,
                &[Category::Effect, Category::Section, Category::Condition],
            )
        }
    }
}

/// Whether syntax the target has could explain `line` instead of `id`.
fn older_alternative(
    catalog: &Catalog,
    line: &str,
    role: LineRole,
    id: EntryId,
    target: Version,
) -> bool {
    let (bare, allowed) = unwrapped(line, role);
    catalog.classify(bare).into_iter().any(|(other, matched)| {
        other != id
            && allowed.contains(&other.category)
            && matched.explains_text(bare)
            && catalog
                .entry(other)
                .is_some_and(|entry| available(catalog, other, entry, target))
    })
}

pub(super) fn check(document: &Document, catalog: &Catalog, out: &mut Vec<Diagnostic>) {
    let Some(target) = catalog.target_version() else {
        return;
    };
    let Some(database) = Version::parse(catalog.version()) else {
        return;
    };
    let entries = super::structure_entry_lines(document);

    for (number_of_line, raw) in lines::code_lines(document.text()) {
        let trimmed = lines::code_part(raw).trim();
        if trimmed.is_empty()
            || entries.contains(&number_of_line)
            || crate::completion::is_documented_entry(catalog, document, number_of_line)
        {
            continue;
        }
        let code = trimmed.trim_end_matches(':').trim_end();
        let indent = skript_index::indent_width(raw);
        let role = LineRole::from_indent(indent);
        let Some((id, _)) = catalog.classify_line(code, role) else {
            continue;
        };
        let Some(entry) = catalog.entry(id) else {
            continue;
        };
        let Some(minimum) = entry.min_version else {
            continue;
        };
        if !is_core(catalog, id, entry)
            || number(minimum) <= number(target)
            || number(minimum) > number(database)
            || older_alternative(catalog, code, role, id, target)
        {
            continue;
        }
        out.push(Diagnostic {
            range: Range::on_line(
                number_of_line,
                indent as u32,
                (indent + trimmed.len()) as u32,
            ),
            severity: Severity::Warning,
            message: format!(
                "`{}` requires Skript {minimum} or newer, but this server runs Skript {target}.",
                entry.name
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
      "source": { "name": "Skript", "version": "2.17.1" },
      "effects": [
        { "id": "send", "name": "Message", "patterns": ["send %strings% [to %commandsenders%]"], "since": "1.0" },
        { "id": "shout", "name": "Shout", "patterns": ["shout %strings%"], "since": "2.15" },
        { "id": "wave", "name": "Wave", "patterns": ["wave at %players%"], "since": "2.16" },
        { "id": "wave_old", "name": "Old Wave", "patterns": ["wave at %objects%"], "since": "2.1" },
        { "id": "future", "name": "Future", "patterns": ["time travel"], "since": "9.0" }
      ],
      "events": [
        { "id": "jump", "name": "On Jump", "patterns": ["[player] jump"], "since": "2.16" }
      ]
    }"#;

    fn found(source: &str, target: &str) -> Vec<(u32, String)> {
        let mut docs = skript_docs::Docs::parse(DOCS).unwrap();
        docs.resolve_versions();
        let catalog = Catalog::build(docs).with_target_version(Version::parse(target));
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let mut out = Vec::new();
        check(workspace.get("file:///t.sk").unwrap(), &catalog, &mut out);
        out.into_iter()
            .map(|d| (d.range.start.line, d.message))
            .collect()
    }

    #[test]
    fn syntax_newer_than_the_server_is_a_warning() {
        let out = found("on join:\n\tshout \"hi\"\n", "2.14.0");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, 1);
        assert!(out[0].1.contains("`Shout` requires Skript 2.15"), "{out:?}");
        assert!(out[0].1.contains("runs Skript 2.14"), "{out:?}");
    }

    #[test]
    fn events_are_checked_too() {
        let out = found("on jump:\n\tsend \"hi\"\n", "2.15");
        assert_eq!(out.iter().map(|(l, _)| *l).collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn syntax_the_server_has_is_fine() {
        assert!(found("on join:\n\tshout \"hi\"\n", "2.15").is_empty());
        assert!(found("on join:\n\tsend \"hi\"\n", "2.0").is_empty());
        // A pre-release of the version that added it is not told it lacks it.
        assert!(found("on join:\n\tshout \"hi\"\n", "2.15.0-pre1").is_empty());
    }

    /// The old `%objects%` pattern still takes the line on 2.15.
    #[test]
    fn an_older_pattern_that_also_fits_silences_it() {
        assert!(found("on join:\n\twave at player\n", "2.15").is_empty());
    }

    /// Newer than the database itself: not a Skript version at all.
    #[test]
    fn a_since_newer_than_the_database_is_ignored() {
        assert!(found("on join:\n\ttime travel\n", "2.14").is_empty());
    }

    #[test]
    fn addon_syntax_is_never_judged_by_its_since() {
        let mut docs = skript_docs::Docs::parse(DOCS).unwrap();
        docs.attribute_to(&skript_docs::model::AddonRef {
            name: "SomeAddon".into(),
            ..Default::default()
        });
        docs.resolve_versions();
        let catalog = Catalog::build(docs).with_target_version(Version::parse("2.1"));
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", "on join:\n\tshout \"hi\"\n");
        let mut out = Vec::new();
        check(workspace.get("file:///t.sk").unwrap(), &catalog, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}
