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

pub use matcher::{Match, OpenSlot, SlotCapture};
pub use pattern::{Node, ParseError, Pattern, Slot};

use std::collections::HashMap;

/// The slots `pattern` can have open at the end of `line`: what an argument
/// typed next would fill. Empty when the line is not a start of the pattern.
pub fn open_slots(pattern: &Pattern, line: &str) -> Vec<OpenSlot> {
    matcher::open_slots(pattern, line)
}

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
    /// "Best" is how much of the line the pattern's own words explain, then
    /// the pattern's specificity, less a penalty for each literal sitting in a
    /// slot that cannot hold it. So `give %item types% to %players%` wins over
    /// a bare `%objects%`.
    pub fn matches(&self, line: &str) -> Vec<(&T, Match)> {
        self.matches_scored(line)
            .into_iter()
            .map(|(_, value, matched)| (value, matched))
            .collect()
    }

    /// As [`PatternIndex::matches`], but keeping each pattern's specificity.
    ///
    /// Callers that know something the index does not — that core Skript should
    /// outrank an addon offering an equally specific pattern, say — need the
    /// score to tell "equally good" from "genuinely better".
    pub fn matches_scored(&self, line: &str) -> Vec<(i32, &T, Match)> {
        let mut results: Vec<(i32, &T, Match)> = Vec::new();

        for index in self.candidates(line) {
            let entry = &self.entries[index];
            if let Some(matched) = matcher::match_pattern(&entry.pattern, line) {
                // A `<regex>`'s span is recorded like a slot's, so a pattern
                // using one is credited for its own words and nothing else.
                // Scoring such patterns 0 made ClassySK's `[a] new [instance
                // of] <[A-Z]\w*>` lose to a core pattern that explained only
                // `new`.
                let coverage = literal_coverage(line, &matched);
                let score = coverage
                    .saturating_mul(10)
                    .saturating_add(entry.pattern.specificity())
                    .saturating_sub(IMPLAUSIBLE_LITERAL * implausible_literals(&matched));
                results.push((score, &entry.value, matched));
            }
        }

        // Most specific first.
        results.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        results
    }

    /// The single best match, or `None` when the line matches nothing.
    pub fn best_match(&self, line: &str) -> Option<(&T, Match)> {
        self.matches(line).into_iter().next()
    }
}

/// How much of the line the pattern pinned down with its own words.
///
/// [`Pattern::specificity`] scores the *pattern*, so a literal inside an
/// optional counts for nothing. That reads the wrong way round when two
/// patterns match the same line: core Skript writes
/// `send %objects% [to %audiences%]` while BungeeSK writes
/// `send %bungeeplayer% to %bungeeserver%`, and on `send "hi" to player` both
/// consume exactly the words `send` and `to` — but only BungeeSK's are
/// mandatory, so it scored 22 against core's 7 and won. Anyone running
/// BungeeSK saw core Skript's Message effect documented as "Send bungee player
/// to server".
///
/// Measuring the match instead makes those two equal, and an exact tie already
/// resolves in core Skript's favour. Counts non-whitespace bytes outside the
/// slot captures, so a pattern that explains more of the line ranks higher.
///
/// Regex holes count as unexplained, exactly like slots: the pattern did not
/// spell that text out.
fn literal_coverage(line: &str, matched: &matcher::Match) -> i32 {
    i32::try_from(matched.literal_coverage(line)).unwrap_or(i32::MAX)
}

/// What one literal filling a slot that cannot take it costs a match.
///
/// Worth two mandatory words, or two characters of coverage: it settles a
/// specificity contest without overturning a real difference in how much of
/// the line each pattern explains.
const IMPLAUSIBLE_LITERAL: i32 = 20;

/// How many slots took a literal that the slot cannot hold.
///
/// Slots are untyped here on purpose, because an expression's type is not
/// knowable without evaluating it. A literal's type is: `20` is a number and
/// `"hi"` is text, whatever surrounds them. On `block contains 20
/// cobblestone`, Region Contains (`%regions% contain[s] %directions%
/// %locations%`) outscored Contains (`%objects% contain[s] %objects%`) on the
/// strength of three typed slots, one of which had taken `20` as a direction.
/// A literal can never be changed either, so a `%~by-reference%` slot cannot
/// hold one whatever its type.
///
/// This only ranks and never rejects, so a type missing from
/// [`LiteralKind::accepts`] costs some ranking rather than a match.
fn implausible_literals(matched: &Match) -> i32 {
    let count = matched
        .captures
        .iter()
        .filter(|capture| !capture.regex)
        .filter(|capture| {
            literal_kind(&capture.text).is_some_and(|kind| {
                capture.slot.by_reference || !capture.slot.types.iter().any(|ty| kind.accepts(ty))
            })
        })
        .count();
    i32::try_from(count).unwrap_or(i32::MAX)
}

#[derive(Clone, Copy)]
enum LiteralKind {
    Number,
    Text,
}

impl LiteralKind {
    fn accepts(self, ty: &str) -> bool {
        let singular = ty.strip_suffix('s').unwrap_or(ty);
        let named: &[&str] = match self {
            LiteralKind::Number => &[
                "number", "integer", "long", "double", "float", "short", "byte",
            ],
            LiteralKind::Text => &["text", "string"],
        };
        singular == "object" || named.contains(&singular)
    }
}

/// The type of `text` when it is exactly one number or one quoted string.
fn literal_kind(text: &str) -> Option<LiteralKind> {
    let tokens = matcher::tokenize(text);
    let [token] = tokens.as_slice() else {
        return None;
    };
    if token.text.starts_with('"') {
        return Some(LiteralKind::Text);
    }
    let digits = token.text.strip_prefix('-').unwrap_or(&token.text);
    let is_number = digits.starts_with(|ch: char| ch.is_ascii_digit())
        && digits
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch == '_' || ch == '.')
        && digits.matches('.').count() <= 1;
    is_number.then_some(LiteralKind::Number)
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

    /// Core Skript's own example: `20` cannot be a direction, so the pattern
    /// that needs it to be one must not win on slot count alone.
    #[test]
    fn a_number_in_a_slot_that_cannot_hold_one_ranks_lower() {
        let index = index_of(&[
            "[[the] region] %regions% contain[s] %directions% %locations%",
            "%objects% contain[s] %objects%",
        ]);
        let (which, _) = index.best_match("block contains 20 cobblestone").unwrap();
        assert_eq!(which, "%objects% contain[s] %objects%");

        // Not a rejection: with nothing better on offer it still matches.
        let index = index_of(&["[[the] region] %regions% contain[s] %directions% %locations%"]);
        assert!(index.best_match("block contains 20 cobblestone").is_some());
    }

    #[test]
    fn literal_kinds_know_their_slots() {
        assert!(matches!(literal_kind("20"), Some(LiteralKind::Number)));
        assert!(matches!(literal_kind("-1.5"), Some(LiteralKind::Number)));
        assert!(matches!(literal_kind("1_000"), Some(LiteralKind::Number)));
        assert!(matches!(
            literal_kind(r#""hi there""#),
            Some(LiteralKind::Text)
        ));
        // Several tokens, or not a literal at all: no claim is made.
        assert!(literal_kind("20 cobblestone").is_none());
        assert!(literal_kind(r#""a" and "b""#).is_none());
        assert!(literal_kind("player").is_none());
        assert!(literal_kind("1.2.3").is_none());

        assert!(LiteralKind::Number.accepts("integers"));
        assert!(LiteralKind::Number.accepts("objects"));
        assert!(!LiteralKind::Number.accepts("directions"));
        assert!(LiteralKind::Text.accepts("strings"));
        assert!(!LiteralKind::Text.accepts("players"));
    }

    #[test]
    fn a_literal_cannot_fill_a_by_reference_slot() {
        let index = index_of(&["set %~objects% to %objects%", "set %objects% %objects%"]);
        let scored = index.matches_scored(r#"set "x" to 5"#);
        let by_reference = scored
            .iter()
            .find(|(_, which, _)| which.as_str() == "set %~objects% to %objects%")
            .map(|(score, _, _)| *score)
            .unwrap();
        let unpenalised = index.matches_scored(r#"set {_x} to 5"#)[0].0;
        // Same coverage and specificity; the string in `%~objects%` is the
        // whole difference.
        assert_eq!(unpenalised - by_reference, IMPLAUSIBLE_LITERAL);
    }

    /// The implicit conditional, `implicit:<.+>`, matches anything. Callers
    /// filtering whole lines rely on being able to see that it explained none
    /// of the text.
    #[test]
    fn a_catch_all_match_explains_no_text() {
        let index = index_of(&["implicit:<.+>", "send %objects%"]);
        let ranked = index.matches(r#"send "hi""#);
        let catch_all = ranked
            .iter()
            .find(|(which, _)| which.as_str() == "implicit:<.+>")
            .map(|(_, matched)| matched)
            .unwrap();
        assert!(!catch_all.explains_text(r#"send "hi""#));
        assert!(ranked[0].1.explains_text(r#"send "hi""#));
        assert_eq!(ranked[0].1.literal_coverage(r#"send "hi""#), 4);
    }
}
