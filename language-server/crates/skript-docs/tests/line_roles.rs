//! Classification against the real published database, by line role.
//!
//! These pin the behaviour that decides what a user actually sees: the category
//! a line is coloured as, and the documentation hover shows for it. They run
//! against `vendor/docs.json` when it is present and skip otherwise, because the
//! database is GPL-3.0 and is fetched rather than vendored into the repo.

use skript_docs::{Catalog, Category, LineRole};

fn catalog() -> Option<Catalog> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../vendor/docs.json");
    let text = std::fs::read_to_string(path).ok()?;
    let mut docs = skript_docs::Docs::parse(&text)
        .expect("vendor/docs.json is present but did not parse — that is a bug, not a skip");
    docs.resolve_versions();
    Some(Catalog::build(docs))
}

/// `(line, role, expected category, expected entry name)`.
const EXPECTED: &[(&str, LineRole, Category, &str)] = &[
    // Events reach their own documentation only because `classify_line` undoes
    // Skript's `[on] … [with priority …]` structure wrapper first.
    (
        "on first join:",
        LineRole::TopLevel,
        Category::Event,
        "On First Join",
    ),
    (
        "on sneak toggle:",
        LineRole::TopLevel,
        Category::Event,
        "On Sneak Toggle",
    ),
    (
        "on rightclick with a diamond:",
        LineRole::TopLevel,
        Category::Event,
        "On Click",
    ),
    // Structures win over the `on command` event for a `command` line, which is
    // what the tiering in `LineRole::tiers` exists for.
    (
        "command /home <text>:",
        LineRole::TopLevel,
        Category::Structure,
        "Command",
    ),
    (
        "function greet(name: text) :: text:",
        LineRole::TopLevel,
        Category::Structure,
        "Function",
    ),
    (
        "options:",
        LineRole::TopLevel,
        Category::Structure,
        "Options",
    ),
    // Statements.
    (
        "wait 3 seconds",
        LineRole::Statement,
        Category::Section,
        "Delay",
    ),
    (
        "loop all players:",
        LineRole::Statement,
        Category::Section,
        "Loop",
    ),
    (
        "teleport player to {_home}",
        LineRole::Statement,
        Category::Effect,
        "Teleport",
    ),
    (
        "broadcast \"hi\"",
        LineRole::Statement,
        Category::Effect,
        "Broadcast",
    ),
];

#[test]
fn ordinary_lines_classify_as_the_right_thing() {
    let Some(catalog) = catalog() else {
        eprintln!("skipped: vendor/docs.json not fetched");
        return;
    };

    let mut wrong = Vec::new();
    for (line, role, category, name) in EXPECTED {
        match catalog.classify_line(line, *role) {
            Some((id, _)) => {
                let entry = catalog.entry(id).expect("classified id must resolve");
                if id.category != *category || !entry.name.eq_ignore_ascii_case(name) {
                    wrong.push(format!(
                        "{line:?} -> {:?}/{} (wanted {category:?}/{name})",
                        id.category, entry.name
                    ));
                }
            }
            None => wrong.push(format!("{line:?} -> nothing (wanted {category:?}/{name})")),
        }
    }
    assert!(wrong.is_empty(), "misclassified:\n  {}", wrong.join("\n  "));
}

#[test]
fn an_unknown_statement_is_not_dressed_up_as_an_expression() {
    let Some(catalog) = catalog() else { return };

    // Skript ships three expressions that match literally any text —
    // `[the] [event-]<.+>` foremost among them. They are correct as
    // expressions, but an expression is never a whole line, and letting one win
    // means every unrecognised statement gets a confident, wrong label.
    for line in [
        "flurgle the wombat",
        "on first join", // an event pattern, but this is not the top level
    ] {
        if let Some((id, _)) = catalog.classify_line(line, LineRole::Statement) {
            assert_ne!(
                id.category,
                Category::Expression,
                "{line:?} was classified as the expression {:?}",
                catalog.entry(id).map(|e| e.name.clone()),
            );
        }
    }
}

#[test]
fn a_structure_line_is_never_an_effect_or_condition() {
    let Some(catalog) = catalog() else { return };

    for line in ["on join:", "command /kit:", "function f():"] {
        let Some((id, _)) = catalog.classify_line(line, LineRole::TopLevel) else {
            panic!("{line:?} did not classify at all");
        };
        assert!(
            matches!(id.category, Category::Event | Category::Structure),
            "{line:?} classified as {:?}",
            id.category,
        );
    }
}

#[test]
fn glued_groups_reach_their_documentation() {
    let Some(catalog) = catalog() else { return };

    // Each of these only resolves because `fuse_glued` spells out a group that
    // is written flush against a word. Before that, the inverted index keyed
    // them on a word that can never appear in a line, so they were never even
    // offered to the matcher.
    for line in ["the event is cancelled", "the event is canceled"] {
        let hit = catalog.classify_line(line, LineRole::Statement);
        assert!(hit.is_some(), "{line:?} did not classify");
    }
}

/// A condition must be reachable however the user spells its negation.
///
/// `(is|are)(n't| not)` writes the negation as two branches: `n't`, glued to the
/// word before it, and ` not`, which carries a leading space. Collapsing that
/// space away fused `is` + `not` into `isnot`, so `{_x} isn't set` found
/// `Exists/Is Set` and `{_x} is not set` did not find it at all — the same
/// condition, reachable only if you happened to use the contraction.
///
/// This asserts the entry is among the matches for both spellings. It does not
/// assert it ranks *first*: `Comparison` publishes many patterns and outscores
/// it on the spelled-out form, which is a separate question about specificity
/// scoring rather than about whether the pattern matches at all.
#[test]
fn a_negation_is_reachable_either_way() {
    let Some(catalog) = catalog() else {
        eprintln!("skipped: vendor/docs.json not fetched");
        return;
    };

    let reaches = |line: &str, wanted: &str| {
        catalog.classify(line).into_iter().any(|(id, _)| {
            catalog
                .entry(id)
                .is_some_and(|entry| entry.name.eq_ignore_ascii_case(wanted))
        })
    };

    for line in ["{_x} isn't set", "{_x} is not set", "{_x} is set"] {
        assert!(
            reaches(line, "Exists/Is Set"),
            "{line:?} never reaches Exists/Is Set"
        );
    }
    for line in ["{_d} hasn't passed", "{_d} has not passed"] {
        assert!(
            reaches(line, "In The Past/Future"),
            "{line:?} never reaches In The Past/Future"
        );
    }
}
