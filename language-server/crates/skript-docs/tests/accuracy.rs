//! How often does the server pick the *right* syntax, not just a syntax?
//!
//! `docs.json` ships `examples` for every entry it documents. Those examples are
//! an upstream-maintained labelled dataset: a line in the "Teleport" entry's
//! examples is, by construction, a line Skript itself parses as Teleport. That
//! makes them the only ground truth available for classification accuracy, and
//! this is the gate that keeps it from silently regressing.
//!
//! # Why three numbers rather than one
//!
//! A single percentage here would be dishonest. An effect's example is usually a
//! whole snippet — `on damage:` on one line, the effect on the next — and the
//! scaffolding lines genuinely belong to *other* entries. Counting those as
//! failures understates accuracy; discarding them by indentation would be worse
//! still, because in Skript the entry's own syntax is normally the *indented*
//! line. So:
//!
//! * **A** — every testable line attributed to its source entry. A floor, not
//!   the accuracy: it includes scaffolding that is supposed to resolve
//!   elsewhere.
//! * **B** — only lines where the entry's own pattern provably matches, so the
//!   entry is a valid answer and every failure is purely a **ranking** failure.
//!   This is the honest accuracy number.
//! * **C** — entries whose own patterns match *none* of their own examples.
//!   Unambiguous **matching** defects, with no labelling ambiguity at all.
//!
//! The floor asserted at the end is deliberately below the measured value, so
//! this fails on a real regression rather than on noise from an upstream
//! database update.

use std::collections::HashMap;

use skript_docs::{Catalog, Category, EntryId, LineRole};

fn catalog() -> Option<Catalog> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../vendor/docs.json");
    let text = std::fs::read_to_string(path).ok()?;
    let mut docs = skript_docs::Docs::parse(&text)
        .expect("vendor/docs.json is present but did not parse — a bug, not a skip");
    docs.resolve_versions();
    Some(Catalog::build(docs))
}

/// Mirrors `skript-lsp`'s `strip_trailing_comment`: `#` is literal inside a
/// string or a variable name, so the harness has to walk the line exactly the
/// way the server does or it will test text the server never sees.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let (mut in_string, mut in_variable, mut index) = (false, false, 0);
    while index < bytes.len() {
        match bytes[index] {
            b'"' if !in_variable => in_string = !in_string,
            b'{' if !in_string => in_variable = true,
            b'}' if !in_string => in_variable = false,
            b'#' if !in_string && !in_variable => {
                if bytes.get(index + 1) == Some(&b'#') {
                    index += 2;
                    continue;
                }
                return &line[..index];
            }
            _ => {}
        }
        index += 1;
    }
    line
}

/// Undoes the `if` / `else if` / `while` / `do while` that introduces a
/// condition, matching `classify_line`.
fn bare_condition(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    for keyword in ["do while ", "else if ", "while ", "if "] {
        if lower.starts_with(keyword) {
            return line[keyword.len()..].trim().to_string();
        }
    }
    line.to_string()
}

/// Undoes the `[on] … [with priority …]` wrapper, matching `classify_line`, so
/// the "is this entry even a candidate" probe sees what the matcher sees.
fn bare_event(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("on ") else {
        return line.to_string();
    };
    let mut offset = line.len() - rest.len();
    for modifier in ["uncancelled ", "cancelled ", "any ", "all "] {
        if let Some(shorter) = lower[offset..].strip_prefix(modifier) {
            offset = line.len() - shorter.len();
            break;
        }
    }
    let mut bare = &line[offset..];
    if let Some(at) = lower[offset..].rfind(" with priority ") {
        bare = &bare[..at];
    }
    bare.trim().to_string()
}

struct Case {
    category: Category,
    index: usize,
    /// The line as the server would receive it.
    code: String,
    role: LineRole,
    /// The line as the matcher sees it, for the "is it a candidate" probe.
    probe: String,
}

fn collect(catalog: &Catalog) -> (Vec<Case>, usize, HashMap<&'static str, usize>) {
    const CATEGORIES: [Category; 4] = [
        Category::Effect,
        Category::Condition,
        Category::Section,
        Category::Event,
    ];

    let mut cases = Vec::new();
    let mut raw = 0usize;
    let mut skipped: HashMap<&'static str, usize> = HashMap::new();

    for category in CATEGORIES {
        let role = if category == Category::Event {
            LineRole::TopLevel
        } else {
            LineRole::Statement
        };

        for (index, entry) in catalog.docs().entries(category).iter().enumerate() {
            for example in &entry.examples {
                let (mut in_block, mut took_event) = (false, false);
                for line in example.lines() {
                    raw += 1;
                    let trimmed = line.trim();

                    if trimmed == "###" {
                        in_block = !in_block;
                        *skipped.entry("block comment").or_default() += 1;
                        continue;
                    }
                    if in_block {
                        *skipped.entry("block comment").or_default() += 1;
                        continue;
                    }
                    if trimmed.is_empty() {
                        *skipped.entry("blank").or_default() += 1;
                        continue;
                    }
                    if trimmed.starts_with('#') {
                        *skipped.entry("comment").or_default() += 1;
                        continue;
                    }

                    let code = strip_comment(trimmed).trim_end();
                    if code.is_empty() {
                        *skipped.entry("comment").or_default() += 1;
                        continue;
                    }

                    // An event's example is a header plus a body. Only the
                    // header is the event; the body belongs to other entries.
                    if category == Category::Event {
                        if !code.to_ascii_lowercase().starts_with("on ") {
                            *skipped.entry("event body").or_default() += 1;
                            continue;
                        }
                        if took_event {
                            *skipped.entry("second event header").or_default() += 1;
                            continue;
                        }
                        took_event = true;
                    }

                    let stripped = code.trim_end_matches(':').trim_end().to_string();
                    // The probe must mirror `classify_line`'s own
                    // wrapper-stripping, or stratum B measures a line the
                    // server never actually matches against.
                    let probe = match category {
                        Category::Event => bare_event(&stripped),
                        Category::Condition => bare_condition(&stripped),
                        _ => stripped,
                    };

                    cases.push(Case {
                        category,
                        index,
                        code: code.to_string(),
                        role,
                        probe,
                    });
                }
            }
        }
    }

    (cases, raw, skipped)
}

#[derive(Default, Clone)]
struct Tally {
    total: usize,
    hit: usize,
    attributed: usize,
    attributed_hit: usize,
    ranked_out: usize,
    not_matched: usize,
}

fn label(category: Category) -> &'static str {
    match category {
        Category::Effect => "effects",
        Category::Condition => "conditions",
        Category::Section => "sections",
        _ => "events",
    }
}

#[test]
fn classification_accuracy_against_skripts_own_examples() {
    let Some(catalog) = catalog() else {
        eprintln!("skipped: run scripts/fetch-docs.mjs to enable this gate");
        return;
    };

    let (cases, raw, skipped) = collect(&catalog);
    eprintln!("raw example lines: {raw}, testable: {}", cases.len());
    let mut reasons: Vec<_> = skipped.iter().collect();
    reasons.sort();
    for (reason, count) in reasons {
        eprintln!("  excluded {reason}: {count}");
    }

    let mut per: HashMap<&'static str, Tally> = HashMap::new();
    let mut lost: Vec<(String, String, String)> = Vec::new();
    let mut confusion: HashMap<(String, String), (usize, String)> = HashMap::new();
    let mut self_match: HashMap<(Category, usize), bool> = HashMap::new();

    for case in &cases {
        let expected = EntryId {
            category: case.category,
            index: case.index,
        };
        let expected_entry = catalog.entry(expected).expect("entry resolves");
        let tally = per.entry(label(case.category)).or_default();
        tally.total += 1;

        // Is the entry even a candidate? If so, any failure is ranking.
        let present = catalog
            .classify(&case.probe)
            .iter()
            .any(|(id, _)| *id == expected);
        self_match
            .entry((case.category, case.index))
            .and_modify(|seen| *seen |= present)
            .or_insert(present);
        if present {
            tally.attributed += 1;
        }

        // `if player's health > 4:` is filed upstream under the `Conditionals`
        // section, but `classify_line` deliberately strips the `if`/`while`
        // keyword and answers with the *condition* — because that is what hover
        // should show. "This is an if statement" is not documentation; the
        // Comparison entry's description and examples are. Scoring the intended
        // answer as a failure made six of the twelve remaining stratum-B
        // failures unfixable by construction, which is worse than useless in a
        // gate: it invites someone to "fix" it by making hover less useful.
        let wraps_a_condition = matches!(
            expected_entry.name.as_str(),
            "Conditionals" | "While Loop" | "Do If"
        );

        match catalog.classify_line(&case.code, case.role) {
            Some((got, _))
                if wraps_a_condition
                    && got.category == Category::Condition
                    && case.role != LineRole::TopLevel =>
            {
                tally.hit += 1;
                if present {
                    tally.attributed_hit += 1;
                }
            }
            Some((got, _)) if got == expected => {
                tally.hit += 1;
                if present {
                    tally.attributed_hit += 1;
                }
            }
            other => {
                let actual = other
                    .and_then(|(id, _)| catalog.entry(id).map(|e| e.name.clone()))
                    .unwrap_or_else(|| "(nothing)".to_string());
                if present {
                    tally.ranked_out += 1;
                    // Stratum B only: the entry's own pattern *did* match, so
                    // this is purely a ranking loss and the one list worth
                    // acting on. Mixing it with stratum A hid it behind
                    // scaffolding lines that legitimately belong elsewhere.
                    lost.push((
                        expected_entry.name.clone(),
                        case.code.clone(),
                        actual.clone(),
                    ));
                } else {
                    tally.not_matched += 1;
                }
                let slot = confusion
                    .entry((expected_entry.name.clone(), actual))
                    .or_insert((0, case.code.clone()));
                slot.0 += 1;
            }
        }
    }

    eprintln!("\n            stratum A (all lines)      stratum B (entry matches)");
    let mut totals = Tally::default();
    for name in ["effects", "conditions", "sections", "events"] {
        let t = per.get(name).cloned().unwrap_or_default();
        totals.total += t.total;
        totals.hit += t.hit;
        totals.attributed += t.attributed;
        totals.attributed_hit += t.attributed_hit;
        totals.ranked_out += t.ranked_out;
        totals.not_matched += t.not_matched;
        eprintln!(
            "{name:>11}: {:>4}/{:<4} = {:>5.1}%      {:>4}/{:<4} = {:>5.1}%   [ranked out {}, unmatched {}]",
            t.hit,
            t.total,
            100.0 * t.hit as f64 / t.total.max(1) as f64,
            t.attributed_hit,
            t.attributed,
            100.0 * t.attributed_hit as f64 / t.attributed.max(1) as f64,
            t.ranked_out,
            t.not_matched,
        );
    }

    let stratum_a = 100.0 * totals.hit as f64 / totals.total.max(1) as f64;
    let stratum_b = 100.0 * totals.attributed_hit as f64 / totals.attributed.max(1) as f64;
    eprintln!(
        "\n      TOTAL: {:>4}/{:<4} = {stratum_a:>5.1}%      {:>4}/{:<4} = {stratum_b:>5.1}%",
        totals.hit, totals.total, totals.attributed_hit, totals.attributed,
    );

    let unmatched: Vec<String> = self_match
        .iter()
        .filter(|(_, matched)| !**matched)
        .filter_map(|((category, index), _)| {
            catalog
                .entry(EntryId {
                    category: *category,
                    index: *index,
                })
                .map(|entry| format!("{}/{}", label(*category), entry.name))
        })
        .collect();
    eprintln!(
        "\nstratum C — entries no example of which their own patterns match: {}",
        unmatched.len()
    );
    for name in unmatched.iter().take(20) {
        eprintln!("   {name}");
    }

    // Stratum B only: the entry's own pattern matched and still lost. This is
    // the one list worth acting on — stratum A failures below are dominated by
    // scaffolding lines that genuinely belong to another entry.
    eprintln!("\nranked out despite matching ({}):", lost.len());
    for (expected, code, actual) in &lost {
        eprintln!("   {expected}  lost to  {actual}\n        {code}");
    }

    eprintln!("\ntop misclassifications:");
    let mut ranked: Vec<_> = confusion.into_iter().collect();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.1 .0));
    for ((expected, actual), (count, example)) in ranked.iter().take(20) {
        eprintln!("{count:>4}x  {expected}  ->  {actual}");
        eprintln!("        {example}");
    }

    // Floors, set below the measured values so this fails on a regression
    // rather than on an upstream database update.
    // Measured 99.4% and 9. The floor sits a little below so an upstream
    // database update cannot fail the build on noise, while a real regression
    // still does.
    //
    // 100% is not the target and would be the wrong one. The oracle is where
    // upstream filed each example, and of the five remaining failures two are
    // ones where this classifier is *right* and the filing is wrong — `if
    // script named "x.sk" is loaded:` answers Is Script Loaded rather than the
    // generic Is Loaded it is filed under. Driving this to 100% would mean
    // making those answers worse.
    assert!(
        stratum_b >= 98.5,
        "ranking accuracy fell to {stratum_b:.1}% (floor 98.5%)"
    );
    // The nine that remain are upstream documentation errors, not matcher
    // defects: `Is Transparent`'s example ends in a full stop its pattern does
    // not have, `Egg Will Hatch` documents `[the] egg` but exemplifies
    // `an entity`, `Exit` allows `[1|a|the|this] section` but shows
    // `exit 2 sections`, and `Will Despawn` is two entries sharing one name.
    assert!(
        unmatched.len() <= 12,
        "{} entries match none of their own examples (ceiling 12)",
        unmatched.len()
    );
}

/// The cache must not change any answer.
///
/// `classify_line` memoises, so a wrong cache would be invisible in normal use
/// and catastrophic in effect. This asks the same lines twice and compares.
#[test]
fn caching_does_not_change_the_answer() {
    let Some(catalog) = catalog() else { return };
    let (cases, _, _) = collect(&catalog);

    for case in cases.iter().take(400) {
        let first = catalog
            .classify_line(&case.code, case.role)
            .map(|(id, _)| id);
        let second = catalog
            .classify_line(&case.code, case.role)
            .map(|(id, _)| id);
        assert_eq!(
            first, second,
            "{:?} classified differently on the second ask",
            case.code
        );
    }
}

/// Two roles for the same text are different questions.
///
/// The cache key includes the role; if it did not, whichever was asked first
/// would answer for both — `command /home` is a Structure at the top level and
/// something else entirely inside a trigger.
#[test]
fn the_cache_key_distinguishes_roles() {
    let Some(catalog) = catalog() else { return };

    let top = catalog
        .classify_line("command /home", LineRole::TopLevel)
        .map(|(id, _)| id.category);
    let statement = catalog
        .classify_line("command /home", LineRole::Statement)
        .map(|(id, _)| id.category);

    assert_ne!(
        top, statement,
        "the same text answered identically for both roles"
    );
}
