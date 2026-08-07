//! Skript's published syntax database: typed model, runtime fetch, and the
//! searchable catalog the language server queries.
//!
//! The database is **not** vendored — it is GPL-3.0 and this project is MIT, so
//! it is downloaded and cached at runtime. See [`source`] for the details and
//! the fallback behaviour when there is no network.

pub mod hover;
pub mod model;
pub mod skripthub;
pub mod source;
pub mod version;

pub use model::{Category, Docs, Entry, Reference};
pub use source::{DocsSource, LoadError};

use std::collections::HashMap;

use skript_syntax::{Match, Pattern, PatternIndex};

/// Identifies one entry in the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryId {
    pub category: Category,
    /// Index into `Docs::entries(category)`.
    pub index: usize,
}

/// The database with its patterns compiled and indexed for matching.
///
/// Building this is the expensive part of startup — 2,117 patterns parsed and
/// inverted — so it happens once and is then shared read-only.
pub struct Catalog {
    docs: Docs,
    index: PatternIndex<EntryId>,
    /// Lowercased entry name -> id, for resolving `%type%` slots and for
    /// looking an entry up by name from a completion item.
    by_name: HashMap<String, EntryId>,
    unparsable_patterns: usize,
    /// The Skript version the user targets, when known.
    target_version: Option<crate::version::Version>,
}

impl Catalog {
    pub fn build(docs: Docs) -> Self {
        let mut index = PatternIndex::new();
        let mut by_name = HashMap::new();
        let mut unparsable_patterns = 0;

        for category in Category::ALL.iter().copied() {
            for (position, entry) in docs.entries(category).iter().enumerate() {
                let id = EntryId {
                    category,
                    index: position,
                };

                if !entry.name.is_empty() {
                    by_name.insert(entry.name.to_ascii_lowercase(), id);
                }

                for source in &entry.patterns {
                    match Pattern::parse(source) {
                        Ok(pattern) => index.insert(pattern, id),
                        // One unparsable pattern must not cost us the other
                        // 2,116. The count is surfaced so a regression is
                        // visible rather than silent.
                        Err(_) => unparsable_patterns += 1,
                    }
                }
            }
        }

        index.finish();

        Self {
            docs,
            index,
            by_name,
            unparsable_patterns,
            target_version: None,
        }
    }

    pub fn docs(&self) -> &Docs {
        &self.docs
    }

    /// The Skript version this catalog describes, e.g. `2.16.1`.
    pub fn version(&self) -> &str {
        &self.docs.source.version
    }

    pub fn pattern_count(&self) -> usize {
        self.index.len()
    }

    pub fn unparsable_patterns(&self) -> usize {
        self.unparsable_patterns
    }

    pub fn entry(&self, id: EntryId) -> Option<&Entry> {
        self.docs.entries(id.category).get(id.index)
    }

    pub fn find_by_name(&self, name: &str) -> Option<(EntryId, &Entry)> {
        let id = *self.by_name.get(&name.to_ascii_lowercase())?;
        Some((id, self.entry(id)?))
    }

    /// Classifies a line, best match first.
    pub fn classify(&self, line: &str) -> Vec<(EntryId, Match)> {
        self.index
            .matches(line)
            .into_iter()
            .map(|(id, matched)| (*id, matched))
            .collect()
    }

    /// The single best classification for a line.
    pub fn classify_best(&self, line: &str) -> Option<(EntryId, Match)> {
        self.classify(line).into_iter().next()
    }

    /// Classifies a whole line, using its position in the script to rule out
    /// categories that cannot possibly explain it.
    ///
    /// Prefer this over [`Catalog::classify_best`] for anything derived from a
    /// real line; the unfiltered version exists for fragments and for callers
    /// that genuinely have no structural context.
    pub fn classify_line(&self, line: &str, role: LineRole) -> Option<(EntryId, Match)> {
        let code = line.trim_end().trim_end_matches(':').trim_end();

        // Skript's event structure wraps every event pattern in
        // `[on] [cancelled|…] <.+> [with priority …]` before matching, so a
        // registered event's own pattern never contains the `on`. Without
        // undoing that wrapper here, `on first join` can only ever reach the
        // generic structure — never `first (join|login)` and its documentation.
        if role.allows(Category::Event) {
            if let Some((bare, offset)) = strip_event_wrapper(code) {
                if let Some((id, mut matched)) = self.first_in(bare, &[Category::Event]) {
                    for capture in &mut matched.captures {
                        capture.start += offset;
                        capture.end += offset;
                    }
                    return Some((id, matched));
                }
            }
        }

        // Matched once and filtered per tier. Running the matcher again for each
        // tier would triple the cost of the hottest call in the server for an
        // answer already sitting in this list.
        let ranked = self.index.matches(code);
        role.tiers().iter().find_map(|tier| {
            ranked
                .iter()
                .find(|(id, _)| tier.contains(&id.category))
                .map(|(id, matched)| (**id, matched.clone()))
        })
    }

    /// The best match whose category is one of `allowed`.
    ///
    /// Filtering after ranking rather than before keeps the specificity order
    /// intact across categories: `allowed` is a membership test, not a priority
    /// list.
    fn first_in(&self, line: &str, allowed: &[Category]) -> Option<(EntryId, Match)> {
        let ranked = self.index.matches_scored(line);
        let mut best: Option<(i32, EntryId, Match)> = None;

        for (score, id, matched) in ranked {
            if !allowed.contains(&id.category) {
                continue;
            }
            match &best {
                // Nothing yet, or this pattern is strictly more specific.
                None => best = Some((score, *id, matched)),
                Some((top, top_id, _)) if score > *top => {
                    let _ = top_id;
                    best = Some((score, *id, matched));
                }
                // Equally specific: core Skript wins. With 168 addons loaded,
                // `send "hi" to player` otherwise resolved to an addon's
                // "send bungee player to server" rather than to Message —
                // whichever happened to sort first. The user's own server runs
                // Skript; an addon has to be *more* specific to outrank it.
                Some((top, _, _)) if score == *top => {
                    let incumbent_is_addon = best
                        .as_ref()
                        .and_then(|(_, id, _)| self.entry(*id))
                        .is_some_and(|entry| entry.addon.is_some());
                    let challenger_is_core =
                        self.entry(*id).is_some_and(|entry| entry.addon.is_none());
                    if incumbent_is_addon && challenger_is_core {
                        best = Some((score, *id, matched));
                    }
                }
                _ => {}
            }
        }

        best.map(|(_, id, matched)| (id, matched))
    }

    /// Entries in `category` whose name or patterns contain `query`.
    ///
    /// Deliberately simple substring matching: the LSP client does the fuzzy
    /// ranking, and doing it twice fights the editor.
    pub fn search(&self, category: Category, query: &str) -> Vec<(EntryId, &Entry)> {
        let needle = query.to_ascii_lowercase();
        self.docs
            .entries(category)
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                needle.is_empty()
                    || entry.name.to_ascii_lowercase().contains(&needle)
                    || entry
                        .patterns
                        .iter()
                        .any(|pattern| pattern.to_ascii_lowercase().contains(&needle))
            })
            .map(|(index, entry)| (EntryId { category, index }, entry))
            .collect()
    }

    /// Renders the hover card for an entry.
    pub fn hover(&self, id: EntryId) -> Option<String> {
        Some(hover::render(id.category, self.entry(id)?))
    }
}

/// Where a line sits in a script, which decides what can explain it.
///
/// Skript's categories are not interchangeable, and three core expressions —
/// `[the] [event-]<.+>`, `[all [[of] the]|the|every] %*type%` and the entity
/// list — match *literally any text*. They are correct as expressions, because
/// an expression is only ever part of a line. Letting them compete with effects
/// for a whole line means every statement the catalog does not recognise comes
/// back confidently mislabelled as "Creature/Entity/Player/…" — worse than no
/// answer, because hover and semantic colour then assert something false.
///
/// Ruling categories out by position is what keeps classification honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineRole {
    /// Column 0. Opens a structure: an event, a command, a function.
    TopLevel,
    /// Indented. A statement inside a trigger or section body.
    Statement,
    /// Position unknown — a fragment, or a caller with no tree to consult.
    Any,
}

impl LineRole {
    /// Whether the line's indentation puts it at the top level.
    pub fn from_indent(indent: usize) -> LineRole {
        if indent == 0 {
            LineRole::TopLevel
        } else {
            LineRole::Statement
        }
    }

    /// The categories that can explain a whole line here, in tiers.
    ///
    /// Every category in a tier is tried together and ranked by pattern
    /// specificity; a later tier is consulted only when an earlier one has
    /// nothing. Tiers exist because specificity alone cannot separate
    /// `command <.+>` (the structure) from `command [%text%]` (the "on command"
    /// event) — both match `command /home <text>:` and the event happens to
    /// score higher. Structures are introduced by a keyword, so at the top level
    /// they get the first look and events pick up everything else.
    pub fn tiers(self) -> &'static [&'static [Category]] {
        match self {
            LineRole::TopLevel => &[&[Category::Structure], &[Category::Event]],
            LineRole::Statement => &[&[Category::Effect, Category::Section, Category::Condition]],
            LineRole::Any => &[Category::ALL],
        }
    }

    fn allows(self, category: Category) -> bool {
        self.tiers().iter().any(|tier| tier.contains(&category))
    }
}

/// Undoes the `[on] [cancelled|…] … [with priority …]` wrapper that Skript's
/// event structure puts around every event pattern.
///
/// Returns the bare event text and its byte offset within `line`, so captures
/// taken against it can be shifted back onto the original.
fn strip_event_wrapper(line: &str) -> Option<(&str, usize)> {
    // Lowercasing ASCII preserves byte length, so offsets stay valid.
    let lower = line.to_ascii_lowercase();
    let mut offset = line.len() - lower.strip_prefix("on ")?.len();

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

    let trimmed = bare.trim_start();
    offset += bare.len() - trimmed.len();
    let trimmed = trimmed.trim_end();

    (!trimmed.is_empty()).then_some((trimmed, offset))
}

/// A tiny built-in catalog used when the real database cannot be fetched.
///
/// It covers only what Skript itself cannot change — the structure keywords and
/// control flow — so that a first run with no network still offers something
/// useful rather than nothing. It is explicitly not a substitute for the real
/// database, and the server tells the user so.
pub fn fallback_docs() -> Docs {
    const JSON: &str = include_str!("fallback.json");
    Docs::parse(JSON).expect("the built-in fallback catalog must always parse")
}

impl Catalog {
    /// Records the Skript version the user is targeting.
    ///
    /// Used to label syntax that needs something newer. Never used to hide it:
    /// a quarter of `since` values are free text, and hiding working syntax on
    /// a misparse is worse than showing an unnecessary label.
    pub fn with_target_version(mut self, version: Option<crate::version::Version>) -> Self {
        self.target_version = version;
        self
    }

    pub fn target_version(&self) -> Option<crate::version::Version> {
        self.target_version
    }

    /// Every addon represented in this catalog.
    pub fn addons(&self) -> Vec<crate::model::AddonRef> {
        self.docs.addons()
    }

    /// How this entry's availability should be described, if at all.
    ///
    /// `None` when it is available in the target version, or when we have no
    /// target or no parsed minimum to compare.
    pub fn availability(&self, id: EntryId) -> Option<String> {
        let target = self.target_version?;
        let entry = self.entry(id)?;
        let minimum = entry.min_version?;
        (minimum > target).then(|| format!("needs Skript {minimum}+"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_catalog_is_valid_and_useful() {
        let catalog = Catalog::build(fallback_docs());
        assert_eq!(catalog.unparsable_patterns(), 0);
        assert!(catalog.pattern_count() > 10);
        // The things a user types before anything else works.
        assert!(catalog.find_by_name("command").is_some());
        assert!(catalog.find_by_name("function").is_some());
    }

    #[test]
    fn classifies_against_the_fallback() {
        let catalog = Catalog::build(fallback_docs());
        let (id, _) = catalog
            .classify_best("stop")
            .expect("`stop` should classify");
        assert_eq!(id.category, Category::Effect);
    }

    #[test]
    fn search_matches_names_and_patterns() {
        let catalog = Catalog::build(fallback_docs());
        assert!(!catalog.search(Category::Structure, "command").is_empty());
        assert!(catalog.search(Category::Structure, "zzzz").is_empty());
    }
}
