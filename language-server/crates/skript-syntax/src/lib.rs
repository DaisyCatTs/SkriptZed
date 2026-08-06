//! Skript's syntax-pattern language: parsing, matching and indexing.
//!
//! This crate is pure logic with no I/O, so it can be exercised directly
//! against the 2,117 patterns in Skript's published `docs.json`.
//!
//! The reason it exists: Skript has no grammar. Deciding that `give 1 diamond
//! to player` is the effect `EffGive` means matching that line against every
//! registered pattern and picking the best fit. Doing that naively is
//! 2,117 backtracking matches per line, which is far too slow to run on every
//! keystroke, so [`PatternIndex`] narrows the field first.

mod matcher;
mod pattern;

pub use matcher::{Match, SlotCapture};
pub use pattern::{Node, ParseError, Pattern, Slot};

use std::collections::HashMap;

/// An inverted index from literal word to the patterns that require it.
///
/// A pattern is only a candidate for a line if *every* literal the pattern
/// requires appears in that line. Indexing on the rarest required word turns
/// "match against everything" into "match against a handful", which is what
/// keeps per-line classification inside the millisecond budget.
#[derive(Debug, Default)]
pub struct PatternIndex<T> {
    entries: Vec<Entry<T>>,
    /// Rarest required word -> indices into `entries`.
    by_word: HashMap<String, Vec<usize>>,
    /// Patterns with no required literal at all (e.g. a bare `%objects%`), which
    /// must be tried for every line.
    always: Vec<usize>,
    word_counts: HashMap<String, usize>,
}

#[derive(Debug)]
struct Entry<T> {
    pattern: Pattern,
    value: T,
}

impl<T> PatternIndex<T> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            by_word: HashMap::new(),
            always: Vec::new(),
            word_counts: HashMap::new(),
        }
    }

    /// Adds a pattern. Call [`PatternIndex::finish`] once all patterns are in —
    /// the index cannot choose the rarest word until it has seen them all.
    pub fn insert(&mut self, pattern: Pattern, value: T) {
        for word in pattern.required_literals() {
            *self.word_counts.entry(word.to_string()).or_insert(0) += 1;
        }
        self.entries.push(Entry { pattern, value });
    }

    /// Builds the lookup tables. Cheap: one pass over the patterns.
    pub fn finish(&mut self) {
        self.by_word.clear();
        self.always.clear();

        for (index, entry) in self.entries.iter().enumerate() {
            let required = entry.pattern.required_literals();
            let rarest = required
                .iter()
                .min_by_key(|word| self.word_counts.get(**word).copied().unwrap_or(0));

            match rarest {
                Some(word) => self
                    .by_word
                    .entry(word.to_string())
                    .or_default()
                    .push(index),
                None => self.always.push(index),
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every pattern that could possibly match `line`, without running the
    /// matcher. Used directly by tests to measure how well the filter narrows.
    pub fn candidates(&self, line: &str) -> Vec<usize> {
        let words = matcher::tokenize(line);
        let mut out = self.always.clone();
        for word in &words {
            if let Some(indices) = self.by_word.get(word.text.as_str()) {
                out.extend(indices.iter().copied());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Matches `line` against the index and returns every match, best first.
    ///
    /// "Best" is the pattern's specificity: a pattern with more mandatory
    /// literal words and more concretely-typed slots beats a vaguer one, so
    /// `give %item types% to %players%` wins over a bare `%objects%`.
    pub fn matches(&self, line: &str) -> Vec<(&T, Match)> {
        let mut results: Vec<(i32, &T, Match)> = Vec::new();

        for index in self.candidates(line) {
            let entry = &self.entries[index];
            if let Some(matched) = matcher::match_pattern(&entry.pattern, line) {
                results.push((entry.pattern.specificity(), &entry.value, matched));
            }
        }

        // Most specific first.
        results.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        results
            .into_iter()
            .map(|(_, value, matched)| (value, matched))
            .collect()
    }

    /// The single best match, or `None` when the line matches nothing.
    pub fn best_match(&self, line: &str) -> Option<(&T, Match)> {
        self.matches(line).into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(patterns: &[&str]) -> PatternIndex<String> {
        let mut index = PatternIndex::new();
        for source in patterns {
            index.insert(Pattern::parse(source).unwrap(), source.to_string());
        }
        index.finish();
        index
    }

    #[test]
    fn narrows_candidates_by_rarest_word() {
        let index = index_of(&[
            "give %item types% to %players%",
            "send %texts% to %players%",
            "broadcast %texts%",
            "cancel [the] event",
        ]);

        // `broadcast` is unique, so only that one pattern is even considered.
        let candidates = index.candidates("broadcast \"hi\"");
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn picks_the_most_specific_match() {
        let index = index_of(&["%objects%", "give %item types% to %players%"]);
        let (which, _) = index.best_match("give 1 diamond to player").unwrap();
        assert_eq!(which, "give %item types% to %players%");
    }

    #[test]
    fn returns_nothing_for_an_unknown_line() {
        let index = index_of(&["cancel [the] event"]);
        assert!(index.best_match("totally unknown addon syntax").is_none());
    }
}
