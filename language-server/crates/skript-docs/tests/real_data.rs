//! Exercises the docs layer against the real published databases.
//!
//! Two files, neither committed (both are third-party data):
//!
//! * `vendor/docs.json`  — Skript's own database. `node scripts/fetch-docs.mjs`
//! * `vendor/addons.json` — SkriptHub's addon catalog. `node scripts/fetch-addons.mjs`
//!
//! Without them these tests skip, so a fresh clone still gets a green
//! `cargo test`.

use std::path::PathBuf;

use skript_docs::model::Category;
use skript_docs::version::{parse_since, Version};
use skript_docs::Docs;

/// Reads a vendored file, or `None` when it has simply not been fetched.
///
/// Deliberately does *not* swallow read errors beyond absence: a file that
/// exists but cannot be read is a real problem and should surface.
fn vendor(name: &str) -> Option<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../vendor")
        .join(name);
    if !path.exists() {
        return None;
    }
    Some(
        std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("{} exists but could not be read: {error}", path.display())
        }),
    )
}

/// The real Skript database, or `None` when it has not been fetched.
///
/// A **present but unparseable** file panics rather than skipping. An earlier
/// version of this helper returned `None` for both cases, so a model regression
/// that broke deserialisation showed up as five quietly skipped tests and a
/// green run.
fn skript_docs() -> Option<Docs> {
    let text = vendor("docs.json")?;
    Some(
        Docs::parse(&text)
            .unwrap_or_else(|error| panic!("vendor/docs.json failed to deserialise: {error}")),
    )
}

#[test]
fn every_category_deserialises_including_properties() {
    let Some(docs) = skript_docs() else {
        eprintln!("skipping: run node scripts/fetch-docs.mjs");
        return;
    };

    // Counts as published for Skript 2.16.1. `properties` was added in 2.13 and
    // was missing from our model until addon support went in.
    assert!(
        docs.expressions.len() > 500,
        "expressions: {}",
        docs.expressions.len()
    );
    assert!(docs.events.len() > 150);
    assert!(docs.conditions.len() > 150);
    assert!(docs.effects.len() > 100);
    assert!(docs.types.len() > 100);
    assert!(!docs.functions.is_empty());
    assert!(
        !docs.properties.is_empty(),
        "the properties category is missing"
    );

    assert_eq!(docs.source.name, "Skript");
    eprintln!(
        "Skript {}: {} entries, {} patterns",
        docs.source.version,
        docs.total_entries(),
        docs.total_patterns()
    );
}

#[test]
fn since_yields_a_minimum_version_for_almost_every_entry() {
    let Some(docs) = skript_docs() else {
        eprintln!("skipping: run node scripts/fetch-docs.mjs");
        return;
    };

    let mut total = 0usize;
    let mut resolved = 0usize;
    let mut unresolved = Vec::new();

    for (category, entry) in docs.all() {
        total += 1;
        let text = entry.since.joined();
        if parse_since(&text).is_some() {
            resolved += 1;
        } else {
            unresolved.push(format!("{category:?}/{} {text:?}", entry.id));
        }
    }

    let percent = 100.0 * resolved as f64 / total as f64;
    eprintln!("resolved a minimum version for {resolved}/{total} entries ({percent:.1}%)");
    for line in unresolved.iter().take(25) {
        eprintln!("   unresolved: {line}");
    }

    // Research measured 98.3%; the remainder are all `unknown`/`before 2.1`
    // entries that predate 2.2 and are treated as always available.
    assert!(
        percent > 97.0,
        "only {percent:.1}% of `since` values yielded a version — the parser has regressed"
    );
}

#[test]
fn parsed_versions_stay_within_skripts_real_range() {
    let Some(docs) = skript_docs() else {
        eprintln!("skipping: run node scripts/fetch-docs.mjs");
        return;
    };

    let current = Version::new(2, 16, 1);
    for (category, entry) in docs.all() {
        let Some(version) = parse_since(&entry.since.joined()) else {
            continue;
        };
        // Nothing may claim to be newer than the database it came from — that
        // would mean the parser picked a note or a stray number.
        assert!(
            version <= current,
            "{category:?}/{} parsed as {version} from {:?}",
            entry.id,
            entry.since.joined()
        );
        // Skript has only ever had major versions 1 and 2. A larger number
        // would mean the parser latched onto a note or a stray figure.
        // No lower bound beyond that: `1.0 pre-5` legitimately sorts *below*
        // 1.0, which is the whole point of the stage ordering.
        assert!(
            version.major <= 2,
            "{category:?}/{} parsed as {version} from {:?}",
            entry.id,
            entry.since.joined()
        );
    }
}

#[test]
fn the_two_deprecated_entries_are_found() {
    let Some(docs) = skript_docs() else {
        eprintln!("skipping: run node scripts/fetch-docs.mjs");
        return;
    };

    let deprecated: Vec<&str> = docs
        .all()
        .filter(|(_, entry)| entry.is_deprecated())
        .map(|(_, entry)| entry.id.as_str())
        .collect();

    eprintln!("deprecated entries: {deprecated:?}");
    // Skript 2.16.1 marks exactly two. If this grows, the deprecation warning
    // becomes more useful; if it drops to zero, the field stopped being read.
    assert!(
        !deprecated.is_empty(),
        "no deprecated entries found — is the `deprecated` field still being read?"
    );
}

#[test]
fn category_coverage_matches_the_published_shape() {
    let Some(docs) = skript_docs() else {
        eprintln!("skipping: run node scripts/fetch-docs.mjs");
        return;
    };

    for category in Category::ALL {
        eprintln!("  {:?}: {}", category, docs.entries(*category).len());
    }
    // Every category the database publishes must have somewhere to land, or its
    // entries are silently dropped.
    let modelled: usize = Category::ALL
        .iter()
        .map(|category| docs.entries(*category).len())
        .sum();
    assert_eq!(modelled, docs.total_entries());
}

#[test]
fn the_catalog_builds_from_the_real_database_and_classifies() {
    let Some(mut docs) = skript_docs() else {
        eprintln!("skipping: run node scripts/fetch-docs.mjs");
        return;
    };
    docs.resolve_versions();

    let catalog = skript_docs::Catalog::build(docs);
    eprintln!(
        "catalog: {} patterns indexed, {} unparsable",
        catalog.pattern_count(),
        catalog.unparsable_patterns()
    );

    // Core Skript publishes ~2,660 patterns across every category.
    assert!(
        catalog.pattern_count() > 2000,
        "only {} patterns indexed — the catalog is running on the fallback",
        catalog.pattern_count()
    );
    assert_eq!(
        catalog.unparsable_patterns(),
        0,
        "core Skript's own patterns should all parse"
    );

    // Lines from Skript's own example scripts must resolve to the right kind.
    for (line, expected) in [
        ("cancel event", skript_docs::Category::Effect),
        ("broadcast \"hi\"", skript_docs::Category::Effect),
    ] {
        let (id, _) = catalog
            .classify_best(line)
            .unwrap_or_else(|| panic!("{line:?} matched nothing"));
        assert_eq!(
            id.category, expected,
            "{line:?} classified as {:?}",
            id.category
        );
    }

    // And hover must render for a well-known effect.
    let (id, _) = catalog
        .find_by_name("Broadcast")
        .expect("Broadcast should exist");
    let hover = catalog.hover(id).expect("hover should render");
    assert!(hover.contains("broadcast"), "hover looked wrong:\n{hover}");
}

// ---------------------------------------------------------------- SkriptHub

fn addon_json() -> Option<String> {
    vendor("addons.json")
}

#[test]
fn the_whole_addon_catalog_converts() {
    let Some(json) = addon_json() else {
        eprintln!("skipping: run node scripts/fetch-addons.mjs");
        return;
    };

    let catalog = skript_docs::skripthub::parse_filtered(&json, |_| true)
        .expect("vendor/addons.json failed to deserialise");

    let entries = catalog.docs.total_entries();
    let patterns = catalog.docs.total_patterns();
    let addons = catalog.docs.addons().len();
    eprintln!(
        "SkriptHub: {entries} entries, {patterns} patterns, {addons} addons, {} skipped",
        catalog.skipped
    );

    // Measured against the live API: 8,210 records, 168 addons, 12,877 patterns.
    assert!(entries > 7500, "only {entries} entries converted");
    assert!(patterns > 12000, "only {patterns} patterns converted");
    assert!(addons > 150, "only {addons} addons found");
}

#[test]
fn nearly_every_addon_pattern_parses() {
    let Some(json) = addon_json() else {
        eprintln!("skipping: run node scripts/fetch-addons.mjs");
        return;
    };

    let catalog = skript_docs::skripthub::parse_filtered(&json, |_| true).unwrap();
    let mut total = 0usize;
    let mut failed = Vec::new();

    for (_, entry) in catalog.docs.all() {
        for pattern in &entry.patterns {
            total += 1;
            if skript_syntax::Pattern::parse(pattern).is_err() {
                failed.push(format!("{}: {pattern}", entry.provider()));
            }
        }
    }

    let rate = 100.0 * (total - failed.len()) as f64 / total as f64;
    eprintln!(
        "addon patterns: {}/{total} parse ({rate:.2}%)",
        total - failed.len()
    );
    for line in failed.iter().take(10) {
        eprintln!("   unparsable: {}", &line[..line.len().min(110)]);
    }

    // The ~28 failures are upstream data problems, not parser gaps:
    //
    //  * community typos — `[including sub dir[ectorie]s)]` has a stray `)`;
    //  * **dropped backslash escapes** — SkriptHub stores Skript's own
    //    `ExprLocationAt` as `[(][x…` where the official database has `[\(][x…`.
    //    The escape means "an optional literal parenthesis"; without it the
    //    brackets genuinely do not balance. Core Skript's own file parses at
    //    100%, which is what proves the parser is right and the copy is lossy.
    //
    // `Catalog::build` counts these rather than failing, so this assertion only
    // guards against a regression that would break the DSL wholesale.
    assert!(
        rate > 99.5,
        "only {rate:.2}% of addon patterns parse — the pattern DSL has regressed"
    );
}

#[test]
fn merging_prefers_skripts_own_entry_over_skripthubs_copy() {
    let (Some(docs_json), Some(addons)) = (vendor("docs.json"), addon_json()) else {
        eprintln!("skipping: run both fetch scripts");
        return;
    };

    let mut docs = Docs::parse(&docs_json).unwrap();
    let official = docs
        .expressions
        .iter()
        .find(|entry| entry.id == "ExprVersion")
        .cloned()
        .expect("ExprVersion should exist in Skript's own database");
    assert!(
        official.addon.is_none(),
        "core syntax must not be attributed to an addon"
    );

    // SkriptHub ships its own copy of core Skript — 1,237 entries sharing ids
    // like `ExprVersion`. Merging must keep the authoritative one.
    let hub = skript_docs::skripthub::parse_filtered(&addons, |_| true).unwrap();
    docs.merge(hub.docs);

    let matches: Vec<&skript_docs::Entry> = docs
        .expressions
        .iter()
        .filter(|entry| entry.id == "ExprVersion")
        .collect();

    assert_eq!(matches.len(), 1, "ExprVersion was duplicated by the merge");
    assert!(
        matches[0].addon.is_none(),
        "the merge kept SkriptHub's copy instead of Skript's own"
    );
}

#[test]
fn loading_only_detected_addons_stays_small() {
    let Some(json) = addon_json() else {
        eprintln!("skipping: run node scripts/fetch-addons.mjs");
        return;
    };

    let wanted = ["SkBee", "skript-reflect"];
    let catalog =
        skript_docs::skripthub::parse_filtered(&json, |name| wanted.contains(&name)).unwrap();

    let patterns = catalog.docs.total_patterns();
    eprintln!("SkBee + skript-reflect: {patterns} patterns");

    // The whole point of detection: a realistic project pays for what it runs,
    // not for all 168 addons.
    assert!(
        patterns > 500,
        "expected SkBee's syntax, got {patterns} patterns"
    );
    assert!(
        patterns < 3000,
        "{patterns} patterns is far more than two addons"
    );

    let names: Vec<String> = catalog.docs.addons().into_iter().map(|a| a.name).collect();
    assert_eq!(names.len(), 2, "loaded {names:?}");
}

/// Skript writes an optional separator as its own group — `[( |-)]` — so
/// `right click`, `right-click` and `rightclick` are all the same event. Fusing
/// that group *nested inside another optional* used to trim the spelled form's
/// trailing space, which then deduped against the no-separator form and deleted
/// the spaced spelling outright. `on right click` — one of the most-used lines
/// in Skript — fell through to the catch-all `<.+>` event structure instead.
///
/// The catch-all is why this cannot be tested by asking "did it classify?": at
/// the top level everything classifies. It has to assert the entry by name.
#[test]
fn a_separator_group_keeps_its_spaced_spelling() {
    let Some(docs) = skript_docs() else {
        return;
    };
    let catalog = skript_docs::Catalog::build(docs);

    for line in [
        "on right click",
        "on left click",
        "on right-click",
        "on rightclick",
        "on left click on a sign",
    ] {
        let entry = catalog
            .classify_line(line, skript_docs::LineRole::TopLevel)
            .and_then(|(id, _)| catalog.entry(id))
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| "<unclassified>".into());
        assert_eq!(entry, "On Click", "`{line}` should be the click event");
    }
}
