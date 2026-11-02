//! Example scripts for addon syntax, from SkriptHub.
//!
//! SkriptHub's bulk catalog (`addonsyntaxlist`) carries no examples at all, so
//! addon hovers would otherwise show none. Examples are served per syntax
//! record instead, keyless:
//!
//! `https://skripthub.net/api/v1/syntaxexample/?syntax=<id>`
//!
//! The response is a JSON array — empty when a record has no examples, and an
//! unknown id is also `200 []`, never a 404. A record looks like:
//!
//! ```json
//! {"id": 1702, "example_author": "Farwl", "syntax_element": 100,
//!  "example_name": "Citizen Click", "example_code": "on npc left click: …",
//!  "score": 3, "official_example": false}
//! ```
//!
//! `score` is the community vote count; `official_example` marks the one the
//! addon author submitted. The array arrives in no useful order.
//!
//! Fetching happens on hover, so it is one small request per syntax at most:
//! answers are cached per id for a week, empty answers included.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::source::{download, read_cached, LoadError};

const EXAMPLES_URL: &str = "https://skripthub.net/api/v1/syntaxexample/";

/// Examples change rarely; a week keeps hovers off the network without letting
/// a newly submitted example stay invisible for long.
const EXAMPLES_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Hover must stay responsive even though the caller runs this off the main
/// thread — a slow SkriptHub should cost one empty hover, not a stuck one.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The largest real responses are a few KB; anything near this is not an
/// example list.
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024;

/// Longer examples are whole scripts pasted in, which drown the hover.
const MAX_EXAMPLE_LINES: usize = 40;

const CACHE_SUBDIR: &str = "skripthub-examples";

/// One record from the `syntaxexample` endpoint. Only the fields used for
/// ranking and display are kept.
#[derive(Debug, Deserialize)]
struct ExampleRecord {
    #[serde(default)]
    example_code: Option<String>,
    #[serde(default)]
    score: Option<i64>,
    #[serde(default)]
    official_example: Option<bool>,
}

/// Up to `limit` example scripts for one SkriptHub syntax record, best first.
pub fn fetch_examples(
    skripthub_id: u64,
    cache_dir: &Path,
    limit: usize,
) -> Result<Vec<String>, LoadError> {
    let records = fetch_records(skripthub_id, cache_dir)?;
    Ok(select(records, limit))
}

/// The SkriptHub docs page for a record, for a "View on SkriptHub" hover link.
pub fn skripthub_page(skripthub_id: u64) -> String {
    format!("https://skripthub.net/docs/?id={skripthub_id}")
}

fn examples_url(skripthub_id: u64) -> String {
    format!("{EXAMPLES_URL}?syntax={skripthub_id}")
}

fn cache_path(cache_dir: &Path, skripthub_id: u64) -> PathBuf {
    cache_dir
        .join(CACHE_SUBDIR)
        .join(format!("{skripthub_id}.json"))
}

fn fetch_records(skripthub_id: u64, cache_dir: &Path) -> Result<Vec<ExampleRecord>, LoadError> {
    let path = cache_path(cache_dir, skripthub_id);
    if let Some(records) = read_cache(&path, Some(EXAMPLES_TTL)) {
        return Ok(records);
    }

    match download(&examples_url(skripthub_id), TIMEOUT, MAX_DOWNLOAD_BYTES) {
        Ok(text) => {
            // Parse before caching: a 200-OK error page written to disk would
            // otherwise be served as the answer for a week.
            let records = parse(&text)?;
            // The raw body is cached rather than the selection, so ranking or
            // normalisation can change without invalidating anything.
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&path, &text);
            Ok(records)
        }
        // A stale answer beats an empty hover when SkriptHub is unreachable.
        Err(error) => read_cache(&path, None).ok_or(error),
    }
}

/// Reads a cached response, ignoring it when older than `max_age`.
///
/// A file that does not parse — a write interrupted by a concurrent hover, say
/// — is treated as missing, so it is simply refetched.
fn read_cache(path: &Path, max_age: Option<Duration>) -> Option<Vec<ExampleRecord>> {
    parse(&read_cached(path, max_age)?).ok()
}

fn parse(text: &str) -> Result<Vec<ExampleRecord>, LoadError> {
    serde_json::from_str(text).map_err(LoadError::malformed("SkriptHub example response"))
}

/// Ranks, normalises and deduplicates records into display-ready examples.
///
/// The addon author's official example comes first: it is maintained alongside
/// the addon, while community examples can predate a syntax change. Votes
/// order the rest. The sort is stable, so ties keep SkriptHub's order.
fn select(mut records: Vec<ExampleRecord>, limit: usize) -> Vec<String> {
    records.sort_by_key(|record| {
        (
            !record.official_example.unwrap_or(false),
            std::cmp::Reverse(record.score.unwrap_or(0)),
        )
    });

    let mut examples: Vec<String> = Vec::new();
    for record in records {
        if examples.len() >= limit {
            break;
        }
        let Some(code) = record.example_code.as_deref().and_then(normalise) else {
            continue;
        };
        // Users resubmit the official example verbatim surprisingly often.
        if !examples.contains(&code) {
            examples.push(code);
        }
    }
    examples
}

/// Cleans one example for display, or `None` when nothing is left.
///
/// Older records use CRLF, and trailing whitespace and blank padding lines are
/// common. Leading indentation is kept: in Skript it is the block structure.
fn normalise(code: &str) -> Option<String> {
    let lines: Vec<&str> = code
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim_end())
        .collect();

    let first = lines.iter().position(|line| !line.is_empty())?;
    let last = lines.iter().rposition(|line| !line.is_empty())?;
    let lines = &lines[first..=last];

    if lines.len() <= MAX_EXAMPLE_LINES {
        return Some(lines.join("\n"));
    }
    let hidden = lines.len() - MAX_EXAMPLE_LINES;
    // A Skript comment, so the truncated example still reads as a script.
    Some(format!(
        "{}\n# … {hidden} more lines",
        lines[..MAX_EXAMPLE_LINES].join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live response for record 100 (Citizens' `npc click` event), copied
    /// verbatim. Note it arrives unsorted: scores 3, 2, 3.
    const NPC_CLICK: &str = r#"[{"id": 1702, "example_author": "Farwl", "syntax_element": 100, "example_name": "Citizen Click", "example_code": "on npc left click:\n\tsend \"You left-clicked on an npc!\"", "score": 3, "official_example": false}, {"id": 2597, "example_author": "Tojefin", "syntax_element": 100, "example_name": "on click and npc name", "example_code": "on npc right click:\n\tif npc name is \"&aMiniGames\":\n\t\tsend \"Soon...\"", "score": 2, "official_example": false}, {"id": 2166, "example_author": "Tutose", "syntax_element": 100, "example_name": "citizen id click", "example_code": "on citizen right click:\n  if npc is 1:\n    #code", "score": 3, "official_example": false}]"#;

    /// The live response for record 11919 (SkBee's Open Real Inventory).
    const OPEN_REAL: &str = r#"[{"id": 8974, "example_author": "ShaneBee", "syntax_element": 11919, "example_name": "Official Example", "example_code": "open real anvil inventory to player\nopen real anvil named \"Mr Anvil\" to player\nopen real anvil at location(1,1,1) named \"Senor Anvil\" to player", "score": 1, "official_example": true}]"#;

    fn record(code: &str, score: i64, official: bool) -> ExampleRecord {
        ExampleRecord {
            example_code: Some(code.to_string()),
            score: Some(score),
            official_example: Some(official),
        }
    }

    /// A fresh directory per test, so parallel tests never share a cache.
    fn temp_cache(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "skript-docs-examples-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn parses_a_real_response() {
        let records = parse(OPEN_REAL).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].official_example, Some(true));
        assert_eq!(
            select(records, 5),
            vec![
                "open real anvil inventory to player\n\
                 open real anvil named \"Mr Anvil\" to player\n\
                 open real anvil at location(1,1,1) named \"Senor Anvil\" to player"
            ]
        );
    }

    #[test]
    fn ranks_by_score_and_keeps_api_order_on_ties() {
        let examples = select(parse(NPC_CLICK).unwrap(), 5);
        assert_eq!(examples.len(), 3);
        assert!(examples[0].starts_with("on npc left click:"));
        assert!(examples[1].starts_with("on citizen right click:"));
        assert!(examples[2].starts_with("on npc right click:"));
    }

    #[test]
    fn the_official_example_outranks_votes() {
        let records = vec![record("community", 9, false), record("official", 0, true)];
        assert_eq!(select(records, 5), vec!["official", "community"]);
    }

    #[test]
    fn respects_the_limit() {
        assert_eq!(select(parse(NPC_CLICK).unwrap(), 1).len(), 1);
        assert!(select(parse(NPC_CLICK).unwrap(), 0).is_empty());
    }

    #[test]
    fn an_empty_response_is_a_valid_answer() {
        assert!(select(parse("[]").unwrap(), 5).is_empty());
    }

    #[test]
    fn tolerates_null_fields() {
        let records =
            parse(r#"[{"example_code": null, "score": null, "official_example": null}]"#).unwrap();
        assert!(select(records, 5).is_empty());
    }

    #[test]
    fn drops_blank_and_duplicate_examples() {
        let records = vec![
            record("send \"hi\"", 2, true),
            record("  \r\n\n ", 5, false),
            record("send \"hi\"\r\n", 1, false),
            record("broadcast \"x\"", 0, false),
        ];
        assert_eq!(select(records, 5), vec!["send \"hi\"", "broadcast \"x\""]);
    }

    #[test]
    fn normalises_crlf_and_padding_but_keeps_indentation() {
        let code = "\r\n\r\non join:  \r\n\tsend \"hi\"\t\r\n\r\n";
        assert_eq!(normalise(code).unwrap(), "on join:\n\tsend \"hi\"");
        assert!(normalise(" \n\t\n").is_none());
    }

    #[test]
    fn caps_long_examples() {
        let code: Vec<String> = (1..=45).map(|n| format!("set {{_x}} to {n}")).collect();
        let out = normalise(&code.join("\n")).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), MAX_EXAMPLE_LINES + 1);
        assert_eq!(lines[MAX_EXAMPLE_LINES - 1], "set {_x} to 40");
        assert_eq!(lines[MAX_EXAMPLE_LINES], "# … 5 more lines");
    }

    #[test]
    fn serves_a_fresh_cache_without_the_network() {
        let dir = temp_cache("fresh");
        let path = cache_path(&dir, 100);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, NPC_CLICK).unwrap();

        // A just-written cache is fresh, so this never reaches the network.
        let examples = fetch_examples(100, &dir, 2).unwrap();
        assert_eq!(examples.len(), 2);
        assert!(examples[0].starts_with("on npc left click:"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cached_empty_answer_is_served_too() {
        let dir = temp_cache("empty");
        let path = cache_path(&dir, 1);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[]").unwrap();

        assert!(read_cache(&path, Some(EXAMPLES_TTL)).is_some());
        assert!(fetch_examples(1, &dir, 3).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_or_expired_cache_is_not_trusted() {
        let dir = temp_cache("corrupt");
        let path = cache_path(&dir, 7);
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        fs::write(&path, "<!doctype html>").unwrap();
        assert!(read_cache(&path, None).is_none());

        fs::write(&path, "[]").unwrap();
        assert!(read_cache(&path, Some(Duration::ZERO)).is_none());
        // Still usable as the offline fallback, which ignores age.
        assert!(read_cache(&path, None).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_paths_are_per_id() {
        let dir = Path::new("cache");
        assert_eq!(
            cache_path(dir, 11919),
            dir.join("skripthub-examples").join("11919.json")
        );
        assert_ne!(cache_path(dir, 1), cache_path(dir, 11));
    }

    #[test]
    fn builds_urls() {
        assert_eq!(
            examples_url(11919),
            "https://skripthub.net/api/v1/syntaxexample/?syntax=11919"
        );
        assert_eq!(
            skripthub_page(11919),
            "https://skripthub.net/docs/?id=11919"
        );
    }

    #[test]
    #[ignore = "hits skripthub.net"]
    fn fetches_live_examples() {
        let dir = temp_cache("live");
        let examples = fetch_examples(11919, &dir, 3).unwrap();
        assert!(examples[0].starts_with("open real anvil"));
        // The answer is now cached on disk.
        assert!(cache_path(&dir, 11919).is_file());
        let _ = fs::remove_dir_all(&dir);
    }
}
