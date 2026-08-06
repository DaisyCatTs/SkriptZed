//! Exercises the pattern engine against Skript's real published syntax.
//!
//! `vendor/docs.json` is Skript's own generated syntax database — 1,208 entries
//! carrying 2,117 patterns, maintained upstream by the people who define the
//! language. It is the largest and most honest test corpus available, and it
//! costs nothing to keep current.
//!
//! It is GPL-3.0 and therefore never committed here. Run `scripts/fetch-docs.mjs`
//! to place it; without it these tests skip rather than fail, so a fresh clone
//! still gets a green `cargo test`.

use std::path::PathBuf;

use serde_json::Value;
use skript_syntax::{Pattern, PatternIndex};

fn docs() -> Option<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../vendor/docs.json")
        .canonicalize()
        .ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Every `(category, syntax id, pattern source)` triple in the database.
fn all_patterns(docs: &Value) -> Vec<(String, String, String)> {
    const CATEGORIES: &[&str] = &[
        "conditions",
        "effects",
        "expressions",
        "events",
        "structures",
        "sections",
    ];

    let mut out = Vec::new();
    for category in CATEGORIES {
        let Some(entries) = docs.get(category).and_then(Value::as_array) else {
            continue;
        };
        for entry in entries {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
                .to_string();
            let Some(patterns) = entry.get("patterns").and_then(Value::as_array) else {
                continue;
            };
            for pattern in patterns.iter().filter_map(Value::as_str) {
                out.push((category.to_string(), id.clone(), pattern.to_string()));
            }
        }
    }
    out
}

#[test]
fn every_published_pattern_parses() {
    let Some(docs) = docs() else {
        eprintln!("skipping: run scripts/fetch-docs.mjs to enable this test");
        return;
    };

    let patterns = all_patterns(&docs);
    assert!(
        patterns.len() > 2000,
        "expected the full database, found only {} patterns",
        patterns.len()
    );

    let mut failures = Vec::new();
    for (category, id, source) in &patterns {
        if let Err(error) = Pattern::parse(source) {
            failures.push(format!("{category}/{id}: {error}\n    {source}"));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} patterns failed to parse:\n{}",
        failures.len(),
        patterns.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );

    eprintln!("parsed {} patterns", patterns.len());
}

#[test]
fn the_index_narrows_the_search_space_hard() {
    let Some(docs) = docs() else {
        eprintln!("skipping: run scripts/fetch-docs.mjs to enable this test");
        return;
    };

    let mut index = PatternIndex::new();
    for (category, id, source) in all_patterns(&docs) {
        if let Ok(pattern) = Pattern::parse(&source) {
            index.insert(pattern, format!("{category}/{id}"));
        }
    }
    index.finish();

    // Lines taken from Skript's own example scripts.
    let lines = [
        "broadcast arg-text",
        "set {homes::%uuid of player%} to player's location",
        "teleport player to {homes::%uuid of player%}",
        "give player an apple named \"Potato\"",
        "break {_source} naturally using an iron pickaxe",
        "wait 1 second",
    ];

    let total = index.len();
    for line in lines {
        let candidates = index.candidates(line).len();
        // The whole point of the inverted index: a line must never fall back to
        // matching against everything.
        assert!(
            candidates * 4 < total,
            "line {line:?} narrowed to {candidates} of {total} patterns — the \
             index is not pulling its weight"
        );
        eprintln!("{candidates:>4} / {total} candidates for {line:?}");
    }
}

#[test]
fn classifies_lines_from_skripts_own_examples() {
    let Some(docs) = docs() else {
        eprintln!("skipping: run scripts/fetch-docs.mjs to enable this test");
        return;
    };

    let mut index = PatternIndex::new();
    for (category, id, source) in all_patterns(&docs) {
        if let Ok(pattern) = Pattern::parse(&source) {
            index.insert(pattern, format!("{category}/{id}"));
        }
    }
    index.finish();

    // Each of these should resolve to the category shown. They are deliberately
    // ordinary: if the engine cannot do these, it cannot do anything.
    let cases = [
        ("cancel event", "effects"),
        ("broadcast \"hello\"", "effects"),
        ("stop", "effects"),
    ];

    for (line, expected_category) in cases {
        let best = index.best_match(line);
        match best {
            Some((id, _)) => assert!(
                id.starts_with(expected_category),
                "{line:?} classified as {id}, expected a {expected_category} entry"
            ),
            None => panic!("{line:?} matched nothing at all"),
        }
    }
}

#[test]
fn matching_a_line_is_fast_enough_for_every_keystroke() {
    let Some(docs) = docs() else {
        eprintln!("skipping: run scripts/fetch-docs.mjs to enable this test");
        return;
    };

    let mut index = PatternIndex::new();
    for (category, id, source) in all_patterns(&docs) {
        if let Ok(pattern) = Pattern::parse(&source) {
            index.insert(pattern, format!("{category}/{id}"));
        }
    }
    index.finish();

    let lines = [
        "set {homes::%uuid of player%} to player's location",
        "give player an apple named \"Potato\"",
        "send \"Set your home to %location of player%\" to player",
        "loop all players",
        "if arg-1 is \"set\"",
    ];

    let start = std::time::Instant::now();
    const ROUNDS: u32 = 20;
    for _ in 0..ROUNDS {
        for line in lines {
            let _ = index.matches(line);
        }
    }
    let per_line = start.elapsed() / (ROUNDS * lines.len() as u32);

    eprintln!("{:?} per line across {} patterns", per_line, index.len());
    // The plan's budget is 5 ms; a debug build is roughly an order of magnitude
    // slower than release, so this asserts the generous end of that.
    assert!(
        per_line < std::time::Duration::from_millis(50),
        "classification took {per_line:?} per line, which is too slow to run on \
         every keystroke"
    );
}
