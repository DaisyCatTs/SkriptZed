//! Is every line in `examples/sample-project/addons.sk` real addon syntax?
//!
//! That file is the one place this project writes syntax it did not get from
//! Skript, and addon syntax is exactly what a person writes from memory and
//! gets subtly wrong. A wrong line there is worse than no line: the server is
//! silent on unknown syntax *by design*, so a typo looks identical to a missing
//! addon and the file quietly stops demonstrating anything.
//!
//! Asking "did it resolve?" proves nothing — with all 168 published addons
//! loaded, 12,877 patterns will match almost any text, and an invented line
//! lands on whichever addon happened to be closest.
//!
//! So each `# === Addon ===` section is checked against a catalog holding core
//! Skript **and that addon alone**. A line then resolves to the addon only if
//! the addon genuinely publishes syntax matching it. Loading everything and
//! comparing the winner would instead measure ranking between addons, which is
//! arbitrary where two of them publish the same thing — `skript-reflect` and
//! its predecessor `skript-mirror` share syntax outright, and both SkBee and
//! MundoSK publish a tab-complete event.
//!
//! Skips when the catalogs have not been fetched.

use std::path::PathBuf;

use skript_docs::{Catalog, Docs, LineRole};

fn vendor(name: &str) -> Option<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../vendor")
        .join(name);
    // Absent means "not fetched" and skips; present-but-unreadable is a real
    // problem and must surface rather than pass as a green skip.
    if !path.exists() {
        return None;
    }
    Some(
        std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("{} exists but could not be read: {error}", path.display())
        }),
    )
}

/// Core Skript plus exactly one addon.
fn core_plus(addon: &str, core: &Docs, addons_json: &str) -> Catalog {
    let mut docs = core.clone();
    let only = skript_docs::skripthub::parse_filtered(addons_json, |name| name == addon)
        .expect("addons.json parses");
    docs.merge(only.docs.clone());
    Catalog::build(docs)
}

fn example_file() -> Option<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/sample-project/addons.sk");
    path.exists().then(|| {
        std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("{} exists but could not be read: {error}", path.display())
        })
    })
}

/// The statements of `addons.sk`, grouped by the addon heading above them.
fn sections(source: &str) -> Vec<(String, Vec<(String, LineRole)>)> {
    let mut out: Vec<(String, Vec<(String, LineRole)>)> = Vec::new();
    let mut in_block = false;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed == "###" {
            in_block = !in_block;
            continue;
        }
        if in_block {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("# === ") {
            out.push((name.trim_end_matches(" ===").trim().to_string(), Vec::new()));
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((_, lines)) = out.last_mut() else {
            continue;
        };

        let role = if line.starts_with(char::is_whitespace) {
            LineRole::Statement
        } else {
            LineRole::TopLevel
        };
        lines.push((trimmed.trim_end_matches(':').trim_end().to_string(), role));
    }

    out
}

#[test]
fn every_line_is_syntax_the_named_addon_actually_publishes() {
    let (Some(core_json), Some(addons_json), Some(source)) =
        (vendor("docs.json"), vendor("addons.json"), example_file())
    else {
        eprintln!("catalogs or example not available — skipping");
        return;
    };
    let core = Docs::parse(&core_json).expect("docs.json parses");

    let mut wrong: Vec<String> = Vec::new();
    let mut fell_through: Vec<String> = Vec::new();
    let mut report: Vec<(String, usize, usize)> = Vec::new();

    for (addon, lines) in sections(&source) {
        let catalog = core_plus(&addon, &core, &addons_json);
        let (mut theirs, mut scaffolding) = (0usize, 0usize);

        for (code, role) in &lines {
            let from = catalog
                .classify_line(code, *role)
                .and_then(|(id, _)| catalog.entry(id))
                .and_then(|entry| entry.addon.as_ref().map(|addon| addon.name.clone()));

            match from {
                Some(name) if name == addon => theirs += 1,
                // Only core Skript is loaded besides this addon, so anything
                // else resolving means core claimed it — legitimate for
                // scaffolding like `on join:`, and never evidence about the
                // addon either way.
                _ => {
                    scaffolding += 1;
                    // Core Skript legitimately owns the scaffolding that hosts
                    // the addon lines. Anything else falling through means the
                    // line does not demonstrate what it claims to.
                    const SCAFFOLDING: [&str; 4] = ["on join", "on quit", "on chat", "stop"];
                    if !SCAFFOLDING.contains(&code.as_str()) {
                        fell_through.push(format!("{addon}: {code}"));
                    }
                }
            }
        }

        // Every section must show something that is genuinely the addon's.
        if theirs == 0 {
            wrong.push(format!(
                "  {addon}: no line resolves to it — all {} fall through to core Skript 
                       (either the syntax was mis-remembered, or the addon name does not match 
                       the catalog's spelling)",
                lines.len()
            ));
        }
        report.push((addon, theirs, scaffolding));
    }

    for entry in &wrong {
        eprintln!("{entry}");
    }

    eprintln!(
        "
  fell through to core Skript (not demonstrating the addon):"
    );
    for entry in &fell_through {
        eprintln!("    {entry}");
    }

    eprintln!(
        "
  lines resolving to the addon they are filed under:"
    );
    for (addon, theirs, scaffolding) in &report {
        eprintln!("    {theirs:>2} addon + {scaffolding} core   {addon}");
    }

    assert!(
        wrong.is_empty(),
        "{} section(s) demonstrate nothing",
        wrong.len()
    );
    assert!(
        fell_through.is_empty(),
        "{} line(s) claim to be addon syntax but resolve to core Skript",
        fell_through.len()
    );
}
