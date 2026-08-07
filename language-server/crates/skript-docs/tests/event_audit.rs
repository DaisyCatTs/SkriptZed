//! How many real event lines do we identify *specifically*?
//!
//! Skript registers a catch-all event structure —
//! `[on] [uncancelled|cancelled|(any|all)] <.+> [with priority (…)]` — whose
//! `<.+>` matches any text at all. So at the top level of a file *every* line
//! classifies, and "did it classify?" is not a question worth asking: a
//! completely unmatched event silently lands on the catch-all and reports as a
//! success.
//!
//! That is exactly how `on right click` stayed broken. This audit asks the only
//! useful question instead: did the line resolve to the *specific* event, or
//! did it fall through? Anything falling through has no hover, no event values
//! and no documentation, however green the coverage number looks.
//!
//! Skips when the corpus has not been fetched; see the README.

use std::path::{Path, PathBuf};

use skript_docs::{Catalog, Docs, LineRole};

/// The generic structure every unmatched top-level line lands on.
const CATCH_ALL: &str = "Event";

fn corpus() -> Option<Vec<PathBuf>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../vendor/skript-corpus");
    if !root.exists() {
        return None;
    }
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "sk") {
                out.push(path);
            }
        }
    }
    walk(&root, &mut out);
    (!out.is_empty()).then_some(out)
}

fn catalog() -> Option<Catalog> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../vendor/docs.json");
    // Absent means "not fetched" and skips. Present-but-unreadable is a real
    // problem and must surface — `real_data.rs` learned this the hard way, when
    // a helper that returned `None` for both hid the fact that `Docs::parse`
    // had never once succeeded on the real file.
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} exists but could not be read: {error}", path.display()));
    Some(Catalog::build(
        Docs::parse(&text).expect("vendor/docs.json parses"),
    ))
}

#[test]
fn most_real_event_lines_resolve_to_a_specific_event() {
    let (Some(files), Some(catalog)) = (corpus(), catalog()) else {
        eprintln!("corpus or docs.json not fetched — skipping");
        return;
    };

    let mut specific = 0usize;
    let mut fell_through: Vec<String> = Vec::new();

    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines() {
            // Top-level only, and only lines that are plainly events: a header
            // opens a section, and `on ` is how every event is written.
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            // `on join: # note` is still a header. A `#` cannot be inside a
            // string here — an event header has no strings before its colon.
            let code = match line.find('#') {
                Some(hash) => line[..hash].trim(),
                None => line.trim(),
            };
            if !code.ends_with(':') {
                continue;
            }
            if !code.starts_with("on ") {
                continue;
            }
            match catalog
                .classify_line(code, LineRole::TopLevel)
                .and_then(|(id, _)| catalog.entry(id))
            {
                Some(entry) if entry.name != CATCH_ALL => specific += 1,
                _ => fell_through.push(code.trim_end_matches(':').to_string()),
            }
        }
    }

    let total = specific + fell_through.len();
    // The corpus is cloned `--depth 1` from upstream HEAD, so this count drifts
    // as Skript edits its own tests. Low enough that ordinary churn cannot trip
    // it, high enough that an empty or broken checkout still fails loudly.
    assert!(total >= 50, "expected a meaningful sample, got {total}");

    fell_through.sort();
    fell_through.dedup();
    let rate = specific as f64 / total as f64 * 100.0;
    eprintln!("\n  event lines: {specific}/{total} resolved to a specific event ({rate:.1}%)");
    eprintln!(
        "  distinct fall-throughs to the catch-all: {}",
        fell_through.len()
    );
    for line in fell_through.iter().take(25) {
        eprintln!("     {line}");
    }

    // A floor, not a target. Addons register events we have no database for, so
    // this can never be 100% — but a drop means the matcher lost something.
    assert!(
        rate >= 80.0,
        "only {rate:.1}% of event lines resolved specifically"
    );
}

/// The events people actually write, spelled the way they actually write them.
///
/// The corpus audit above cannot catch a spelling bug on its own: 540 upstream
/// files contain 103 event lines and not one of them says `on right click`.
/// This list is the complement — hand-written real-world spellings, including
/// the spaced/hyphenated/glued variants Skript accepts via `[( |-)]`, which is
/// precisely the construct that broke.
#[test]
fn the_events_people_actually_write_all_resolve() {
    let Some(catalog) = catalog() else {
        eprintln!("docs.json not fetched — skipping");
        return;
    };

    const COMMON: &[&str] = &[
        "on join",
        "on first join",
        "on quit",
        "on death",
        "on respawn",
        "on click",
        "on right click",
        "on left click",
        "on right-click",
        "on rightclick",
        "on right click on a sign",
        "on click with a stick",
        // The exact spellings examples/sample-project/showcase.sk claims
        // resolve to On Click. If these ever fall through, that file is lying.
        "on left-click on a sign",
        "on rightclick with a stick",
        "on break",
        "on place",
        "on chat",
        "on command",
        "on damage",
        "on craft",
        "on drop",
        "on pickup",
        "on inventory click",
        "on explode",
        "on burn",
        "on ignite",
        "on teleport",
        "on sneak toggle",
        "on flight toggle",
        "on toggle sneak",
        "on world change",
        "on sign change",
        "on bed enter",
        "on bucket fill",
        "on level up",
        "on portal",
        "on projectile hit",
        "on shoot",
        "on tame",
        "on spawn",
        "on load",
        "on unload",
        "on script load",
        "on consume",
        "on smelt",
        "on enchant",
        "on leash",
        "on jump",
        "on fish caught",
        "on fishing line cast",
        "on hunger meter change",
        "on item spawn",
        "on villager career change",
        "on player trade",
        "on player world change",
    ];

    let mut missed = Vec::new();
    for line in COMMON {
        let name = catalog
            .classify_line(line, LineRole::TopLevel)
            .and_then(|(id, _)| catalog.entry(id))
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| "<unclassified>".into());
        if name == CATCH_ALL || name == "<unclassified>" {
            missed.push(*line);
        }
    }

    eprintln!(
        "\n  common events: {}/{} resolved specifically",
        COMMON.len() - missed.len(),
        COMMON.len()
    );
    for line in &missed {
        eprintln!("     MISSED  {line}");
    }
    assert!(
        missed.is_empty(),
        "{} common events fall through",
        missed.len()
    );
}
