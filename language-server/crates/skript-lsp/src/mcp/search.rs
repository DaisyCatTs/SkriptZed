//! Ranking catalog entries against a free-text query.
//!
//! The editor's completion leaves fuzzy ranking to the client. A model asking
//! "send action bar" has no client doing that, and Skript names rarely match
//! the words people use (`send action bar` is the entry "Action Bar", whose
//! pattern spells `action[ ]bar`), so patterns and keywords count as much as
//! the name does.

use skript_docs::{Catalog, Category, Entry, EntryId};

/// Score for the query matching an entry's name exactly.
const EXACT_NAME: i64 = 1000;
const NAME_PREFIX: i64 = 600;
const NAME_PHRASE: i64 = 400;
const PATTERN_PHRASE: i64 = 350;
/// `actionbar` against `action[ ]bar`: the same words with the spacing lost.
const COMPACT_PHRASE: i64 = 150;
const EVERY_WORD: i64 = 100;
const MISSING_WORD: i64 = -80;
const DEPRECATED: i64 = -50;
/// Core Skript outranks an addon when they are otherwise equal, as the
/// classifier does: the user's server always runs Skript.
const CORE: i64 = 5;

/// The best `limit` entries for `query`, best first.
pub fn search(
    catalog: &Catalog,
    query: &str,
    category: Option<Category>,
    limit: usize,
) -> Vec<EntryId> {
    let query = Query::new(query);
    if query.words.is_empty() {
        return Vec::new();
    }
    let categories: &[Category] = match &category {
        Some(category) => std::slice::from_ref(category),
        None => Category::ALL,
    };

    let mut scored: Vec<(i64, &str, EntryId)> = Vec::new();
    for &category in categories {
        for (index, entry) in catalog.docs().entries(category).iter().enumerate() {
            if let Some(score) = query.score(entry) {
                scored.push((score, entry.name.as_str(), EntryId { category, index }));
            }
        }
    }
    // Ties go to the shorter name: "Message" before "Message Format".
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.len().cmp(&b.1.len()))
            .then(a.1.cmp(b.1))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, id)| id)
        .collect()
}

struct Query {
    phrase: String,
    compact: String,
    words: Vec<String>,
}

impl Query {
    fn new(text: &str) -> Self {
        let phrase = normalise(text);
        let mut words: Vec<String> = Vec::new();
        for word in phrase.split_whitespace() {
            if !words.iter().any(|seen| seen == word) {
                words.push(word.to_string());
            }
        }
        Self {
            compact: phrase.replace(' ', ""),
            phrase,
            words,
        }
    }

    /// `None` when nothing in the entry relates to the query at all.
    fn score(&self, entry: &Entry) -> Option<i64> {
        let name = normalise(&entry.name);
        let patterns: Vec<String> = entry.patterns.iter().map(|p| normalise(p)).collect();
        let keywords = normalise(&entry.keywords.join(" "));
        let description = entry
            .description
            .first()
            .map(|line| normalise(line))
            .unwrap_or_default();

        let mut score = 0;
        if name == self.phrase {
            score += EXACT_NAME;
        } else if name.starts_with(&self.phrase) {
            score += NAME_PREFIX;
        } else if name.contains(&self.phrase) {
            score += NAME_PHRASE;
        }
        if patterns
            .iter()
            .any(|pattern| pattern.contains(&self.phrase))
        {
            score += PATTERN_PHRASE;
        }
        if self.compact.len() >= 4 {
            let compact_hit = name.replace(' ', "").contains(&self.compact)
                || patterns
                    .iter()
                    .any(|pattern| pattern.replace(' ', "").contains(&self.compact));
            if compact_hit {
                score += COMPACT_PHRASE;
            }
        }

        let mut found = 0;
        for word in &self.words {
            let word_score = word_score(word, &name, &patterns, &keywords, &description);
            if word_score > 0 {
                found += 1;
                score += word_score;
            } else {
                score += MISSING_WORD;
            }
        }
        if found == 0 && score <= 0 {
            return None;
        }
        if found == self.words.len() {
            score += EVERY_WORD;
        }
        if entry.is_deprecated() {
            score += DEPRECATED;
        }
        if entry.addon.is_none() {
            score += CORE;
        }
        (score > 0).then_some(score)
    }
}

/// How strongly one query word is present in an entry.
fn word_score(
    word: &str,
    name: &str,
    patterns: &[String],
    keywords: &str,
    description: &str,
) -> i64 {
    let has_word = |text: &str| text.split_whitespace().any(|w| w == word);
    let mut score = 0;
    if has_word(name) {
        score += 60;
    } else if name.contains(word) {
        score += 40;
    }
    if patterns.iter().any(|p| has_word(p)) {
        score += 30;
    } else if patterns.iter().any(|p| p.contains(word)) {
        score += 20;
    }
    if keywords.contains(word) {
        score += 25;
    }
    if score == 0 && description.contains(word) {
        score += 5;
    }
    // A typo (`teleprot`) is still worth something, but only for words long
    // enough that one edit does not turn them into a different word.
    if score == 0 && word.len() >= 4 {
        let near = |text: &str| text.split_whitespace().any(|w| within_one_edit(word, w));
        if near(name) || patterns.iter().any(|p| near(p)) {
            score += 15;
        }
    }
    score
}

/// Lowercase words and digits, single-spaced. Pattern syntax (`[ ]`, `(a|b)`,
/// `%type%`) becomes spacing, so `action[ ]bar` reads as `action bar`.
pub fn normalise(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether one insertion, deletion, substitution or swap of neighbours turns
/// `a` into `b`. The swap is the commonest typo of all (`teleprot`).
fn within_one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len() == b.len() {
        let differing: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        if let [first, second] = differing[..] {
            if second == first + 1 && a[first] == b[second] && a[second] == b[first] {
                return true;
            }
        }
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if long.len() - short.len() > 1 {
        return false;
    }
    let mut edits = 0;
    let (mut i, mut j) = (0, 0);
    while i < short.len() && j < long.len() {
        if short[i] == long[j] {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return false;
        }
        if short.len() == long.len() {
            i += 1;
        }
        j += 1;
    }
    edits + (long.len() - j) + (short.len() - i) <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_syntax_becomes_spacing() {
        assert_eq!(
            normalise("send [the] action[ ]bar %string%"),
            "send the action bar string"
        );
    }

    #[test]
    fn one_edit_apart() {
        assert!(within_one_edit("teleprt", "teleport"));
        assert!(within_one_edit("teleport", "telepurt"));
        assert!(within_one_edit("same", "same"));
        assert!(within_one_edit("teleprot", "teleport"));
        assert!(!within_one_edit("teleport", "telport_x"));
        assert!(!within_one_edit("abcd", "dcba"));
    }
}
