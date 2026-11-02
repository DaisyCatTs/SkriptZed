//! Skript's published syntax database: typed model, runtime fetch, and the
//! searchable catalog the language server queries.
//!
//! The database is **not** vendored — it is GPL-3.0 and this project is MIT, so
//! it is downloaded and cached at runtime. See [`source`] for the details and
//! the fallback behaviour when there is no network.

pub mod examples;
pub mod hover;
pub mod model;
pub mod skripthub;
pub mod source;
pub mod version;

pub use model::{Category, Docs, Entry, Reference, SectionEntry};
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
    /// Memo for [`Catalog::classify_line`].
    ///
    /// Matching a line against the index costs roughly 200 µs, and the same
    /// lines are classified twice per edit — once for diagnostics, once for
    /// semantic tokens — so a 800-line file cost ~145 ms *per consumer*.
    /// `classify_line` is pure, so the answer can simply be remembered.
    ///
    /// Skript is repetitive enough that the hit rate is high even within a
    /// single pass: `stop`, `cancel event` and `return` recur constantly.
    cache: std::sync::RwLock<ClassificationCache>,
    /// Syntax the project's own scripts define. See
    /// [`Catalog::set_workspace_entries`].
    workspace: Overlay,
    /// The final answer of [`Catalog::classify_line`] while an overlay is
    /// installed. Cleared whenever the overlay changes; `cache` is not, which
    /// is what keeps editing a custom effect's pattern from re-matching every
    /// line against the 2,000-odd core patterns.
    merged: std::sync::RwLock<MergedCache>,
}

/// What [`Catalog::classify_line`] has already worked out against the main
/// index, keyed by the line text and the role it was asked about.
type ClassificationCache = HashMap<(String, LineRole), Option<Ranked>>;

/// [`Catalog::classify_line`]'s final answers while an overlay is installed.
type MergedCache = HashMap<(String, LineRole), Option<(EntryId, Match)>>;

/// Workspace-defined syntax, kept in its own small index.
///
/// Folding it into the main index would mean rebuilding the whole catalog,
/// about a second with addons loaded, whenever someone edits a skript-reflect
/// pattern. Its ids continue each category's numbering after the main
/// database, so an `EntryId` stays one plain number and every caller keeps
/// working unchanged.
struct Overlay {
    docs: Docs,
    index: PatternIndex<EntryId>,
    tag: Option<u64>,
}

/// A classification with what it takes to compare it against one from the
/// other index: how early in [`Catalog::resolve`] it was found, and its score.
#[derive(Clone)]
struct Ranked {
    stage: usize,
    score: i32,
    id: EntryId,
    matched: Match,
}

impl Ranked {
    /// Whether `self` should be preferred over `other`. An earlier stage wins;
    /// within a stage the higher score does, and a tie goes to `self`.
    fn beats(&self, other: &Ranked) -> bool {
        self.stage < other.stage || (self.stage == other.stage && self.score >= other.score)
    }

    fn into_pair(self) -> (EntryId, Match) {
        (self.id, self.matched)
    }
}

/// Upper bound on remembered lines.
///
/// Generous — a very large project is tens of thousands of distinct lines — but
/// finite, so a long-running server cannot grow without limit. On overflow the
/// cache is cleared rather than evicted one by one: classification is cheap
/// enough that a rebuild costs less than tracking recency.
const CACHE_LIMIT: usize = 50_000;

/// Stores one classification in a memo, emptying it first when it is full.
fn remember<V>(
    memo: &std::sync::RwLock<HashMap<(String, LineRole), V>>,
    key: (String, LineRole),
    value: V,
) {
    if let Ok(mut memo) = memo.write() {
        if memo.len() >= CACHE_LIMIT {
            memo.clear();
        }
        memo.insert(key, value);
    }
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
            cache: std::sync::RwLock::new(HashMap::new()),
            workspace: Overlay {
                docs: Docs::default(),
                index: PatternIndex::new(),
                tag: None,
            },
            merged: std::sync::RwLock::new(HashMap::new()),
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
        let base = self.docs.entries(id.category);
        match id.index.checked_sub(base.len()) {
            None => base.get(id.index),
            Some(own) => self.workspace.docs.entries(id.category).get(own),
        }
    }

    pub fn find_by_name(&self, name: &str) -> Option<(EntryId, &Entry)> {
        if let Some(id) = self.by_name.get(&name.to_ascii_lowercase()) {
            return Some((*id, self.entry(*id)?));
        }
        // The overlay is a few dozen entries at most; a scan is cheaper than
        // keeping a second map in step with it.
        self.workspace_entries()
            .find(|(_, entry)| entry.name.eq_ignore_ascii_case(name))
    }

    /// Classifies a line, best match first.
    pub fn classify(&self, line: &str) -> Vec<(EntryId, Match)> {
        // The project's own syntax first, so that an equal score resolves to
        // it, as it does in `classify_line`; the sort is stable.
        let mut ranked: Vec<(i32, EntryId, Match)> = self
            .workspace
            .index
            .matches_scored(line)
            .into_iter()
            .chain(self.index.matches_scored(line))
            .map(|(score, id, matched)| (score, *id, matched))
            .collect();
        ranked.sort_by_key(|(score, _, _)| std::cmp::Reverse(*score));
        ranked
            .into_iter()
            .map(|(_, id, matched)| (id, matched))
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
        let key = (code.to_string(), role);
        if self.workspace.index.is_empty() {
            return self.classify_main(key).map(Ranked::into_pair);
        }

        if let Ok(merged) = self.merged.read() {
            if let Some(hit) = merged.get(&key) {
                return hit.clone();
            }
        }
        let main = self.classify_main(key.clone());
        let own = self.resolve(&self.workspace.index, code, role);
        // The project's own syntax wins a tie: the user wrote it for exactly
        // this line, and skript-reflect parses `local` syntax first.
        let answer = match (own, main) {
            (Some(own), Some(main)) => Some(if own.beats(&main) { own } else { main }),
            (own, main) => own.or(main),
        }
        .map(Ranked::into_pair);
        remember(&self.merged, key, answer.clone());
        answer
    }

    /// [`Catalog::resolve`] against the main index, memoised.
    fn classify_main(&self, key: (String, LineRole)) -> Option<Ranked> {
        if let Ok(cache) = self.cache.read() {
            if let Some(hit) = cache.get(&key) {
                return hit.clone();
            }
        }
        let answer = self.resolve(&self.index, &key.0, key.1);
        remember(&self.cache, key, answer.clone());
        answer
    }

    /// The best classification `index` offers for a whole line in `role`.
    fn resolve(&self, index: &PatternIndex<EntryId>, code: &str, role: LineRole) -> Option<Ranked> {
        // Skript's event structure wraps every event pattern in
        // `[on] [cancelled|…] <.+> [with priority …]` before matching, so a
        // registered event's own pattern never contains the `on`. Without
        // undoing that wrapper here, `on first join` can only ever reach the
        // generic structure — never `first (join|login)` and its documentation.
        if role.allows(Category::Event) {
            if let Some((bare, offset)) = strip_event_wrapper(code) {
                if let Some(hit) = self.shifted(index, bare, &[Category::Event], offset, 0) {
                    return Some(hit);
                }
            }
        }

        // `if`, `else if`, `while` and `do while` wrap a condition exactly as
        // `on` wraps an event: the condition's own registered pattern never
        // contains the keyword. Without undoing it, `if plugin "Vault" is
        // enabled` resolves to the generic "Conditionals" section and the real
        // condition is unreachable — measured against Skript's own examples,
        // this accounted for 16 of the 20 entries that matched none of them.
        if role.allows(Category::Condition) {
            if let Some((bare, offset)) = strip_condition_keyword(code) {
                if let Some(hit) = self.shifted(index, bare, &[Category::Condition], offset, 1) {
                    return Some(hit);
                }
            }
        }

        // Matched once and filtered per tier. Running the matcher again for each
        // tier would triple the cost of the hottest call in the server for an
        // answer already sitting in this list.
        let ranked = index.matches_scored(code);
        let explained_only = role.needs_explained_text();
        role.tiers()
            .iter()
            .enumerate()
            .find_map(|(tier_rank, tier)| {
                ranked
                    .iter()
                    .find(|(_, id, matched)| {
                        tier.contains(&id.category)
                            && (!explained_only || matched.explains_text(code))
                    })
                    .map(|(score, id, matched)| Ranked {
                        stage: 2 + tier_rank,
                        score: *score,
                        id: **id,
                        matched: matched.clone(),
                    })
            })
    }

    /// A match against `bare`, with its captures moved back onto the full line.
    ///
    /// Both wrapper-stripping paths need this: the captures come back relative
    /// to the stripped text, and every caller — semantic tokens especially —
    /// indexes them against the original.
    fn shifted(
        &self,
        index: &PatternIndex<EntryId>,
        bare: &str,
        allowed: &[Category],
        offset: usize,
        stage: usize,
    ) -> Option<Ranked> {
        let (score, id, mut matched) = self.first_in(index, bare, allowed)?;
        for capture in &mut matched.captures {
            capture.start += offset;
            capture.end += offset;
        }
        Some(Ranked {
            stage,
            score,
            id,
            matched,
        })
    }

    /// The best match whose category is one of `allowed`.
    ///
    /// Filtering after ranking rather than before keeps the specificity order
    /// intact across categories: `allowed` is a membership test, not a priority
    /// list.
    fn first_in(
        &self,
        index: &PatternIndex<EntryId>,
        line: &str,
        allowed: &[Category],
    ) -> Option<(i32, EntryId, Match)> {
        let ranked = index.matches_scored(line);
        let mut best: Option<(i32, EntryId, Match)> = None;

        for (score, id, matched) in ranked {
            // Only ever called for a whole line minus its keyword, so the
            // catch-all rule of `LineRole::needs_explained_text` applies.
            if !allowed.contains(&id.category) || !matched.explains_text(line) {
                continue;
            }
            match &best {
                // Nothing yet, or this pattern is strictly more specific.
                None => best = Some((score, *id, matched)),
                Some((top, _, _)) if score > *top => best = Some((score, *id, matched)),
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

        best
    }

    /// Entries in `category` whose name or patterns contain `query`, the
    /// project's own syntax included.
    ///
    /// Deliberately simple substring matching: the LSP client does the fuzzy
    /// ranking, and doing it twice fights the editor.
    pub fn search(&self, category: Category, query: &str) -> Vec<(EntryId, &Entry)> {
        let needle = query.to_ascii_lowercase();
        let base = self.docs.entries(category);
        base.iter()
            .chain(self.workspace.docs.entries(category))
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

    /// Replaces the syntax the project's own scripts define.
    ///
    /// skript-reflect lets a script declare effects, expressions, conditions
    /// and events; those lines are as much a part of the language as anything
    /// in `docs.json`, but they change as the user types. They live in a small
    /// index of their own, so installing them costs a few dozen pattern parses
    /// and never a rebuild of the main catalog.
    ///
    /// `tag` identifies the set: the caller computes it from whatever it
    /// considers significant, and skips the call when
    /// [`Catalog::workspace_tag`] already matches. A freshly built catalog has
    /// no tag, so a reload is picked up on the next comparison.
    pub fn set_workspace_entries(
        &mut self,
        entries: impl IntoIterator<Item = (Category, Entry)>,
        tag: u64,
    ) {
        let mut docs = Docs::default();
        for (category, entry) in entries {
            docs.push(category, entry);
        }
        let mut index = PatternIndex::new();
        for category in Category::ALL.iter().copied() {
            let first = self.docs.entries(category).len();
            for (position, entry) in docs.entries(category).iter().enumerate() {
                let id = EntryId {
                    category,
                    index: first + position,
                };
                // An unparsable pattern is one the user is halfway through
                // typing; it simply matches nothing until it parses.
                for pattern in entry.patterns.iter().filter_map(|p| Pattern::parse(p).ok()) {
                    index.insert(pattern, id);
                }
            }
        }
        index.finish();
        self.workspace = Overlay {
            docs,
            index,
            tag: Some(tag),
        };
        if let Ok(merged) = self.merged.get_mut() {
            merged.clear();
        }
    }

    /// The tag passed to the last [`Catalog::set_workspace_entries`].
    pub fn workspace_tag(&self) -> Option<u64> {
        self.workspace.tag
    }

    /// Whether `id` names syntax the project defines rather than the database.
    pub fn is_workspace(&self, id: EntryId) -> bool {
        id.index >= self.docs.entries(id.category).len()
    }

    /// Every entry the project defines, with its id.
    pub fn workspace_entries(&self) -> impl Iterator<Item = (EntryId, &Entry)> {
        Category::ALL.iter().copied().flat_map(move |category| {
            let first = self.docs.entries(category).len();
            self.workspace
                .docs
                .entries(category)
                .iter()
                .enumerate()
                .map(move |(position, entry)| {
                    (
                        EntryId {
                            category,
                            index: first + position,
                        },
                        entry,
                    )
                })
        })
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineRole {
    /// Column 0. Opens a structure: an event, a command, a function.
    TopLevel,
    /// Indented. A statement inside a trigger or section body.
    Statement,
    /// Part of a line: what fills a `%slot%` of an already-matched pattern, or
    /// a function argument. Only an expression can sit there, which is exactly
    /// the position where the catch-alls described above are honest.
    Expression,
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
            LineRole::Expression => &[&[Category::Expression]],
            LineRole::Any => &[Category::ALL],
        }
    }

    /// Whether a match must spell out some of the line to count here.
    ///
    /// A whole line is only explained by a pattern that says something about
    /// it. Skript registers the implicit conditional as `implicit:<.+>`, and
    /// once parse tags were read correctly that section matched every
    /// indented line, so nothing could be reported as unknown syntax. Inside a
    /// slot (`Expression`) the catch-alls are honest, so they stay.
    fn needs_explained_text(self) -> bool {
        matches!(self, LineRole::TopLevel | LineRole::Statement)
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

/// Removes the `if` / `else if` / `while` / `do while` that introduces a
/// condition, returning the bare condition and its byte offset in `line`.
///
/// `else` alone is not included: it takes no condition, so stripping it would
/// leave an empty string that matches nothing useful.
fn strip_condition_keyword(line: &str) -> Option<(&str, usize)> {
    // Longest first — `else if` must win over `if`, and `do while` over `while`.
    const KEYWORDS: [&str; 4] = ["do while ", "else if ", "while ", "if "];

    // Lowercasing ASCII preserves byte length, so offsets stay valid.
    let lower = line.to_ascii_lowercase();
    let keyword = KEYWORDS
        .iter()
        .find(|keyword| lower.starts_with(*keyword))?;

    let rest = &line[keyword.len()..];
    let trimmed = rest.trim_start();
    let offset = line.len() - trimmed.len();
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

    fn custom(name: &str, patterns: &[&str]) -> Entry {
        Entry {
            id: format!("custom:{name}"),
            name: name.into(),
            patterns: patterns.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        }
    }

    /// skript-reflect syntax defined in the project classifies like any other,
    /// without rebuilding the catalog, and its ids resolve back to the entry.
    #[test]
    fn workspace_entries_classify_alongside_the_database() {
        let mut catalog = Catalog::build(fallback_docs());
        assert!(catalog
            .classify_line("greet player", LineRole::Statement)
            .is_none());

        catalog.set_workspace_entries(
            [
                (Category::Effect, custom("greet", &["[the] greet %player%"])),
                (
                    Category::Event,
                    custom("greeting", &["greeting [of] %player%"]),
                ),
                (Category::Expression, custom("answer", &["[the] answer"])),
            ],
            1,
        );
        assert_eq!(catalog.workspace_tag(), Some(1));

        let (id, _) = catalog
            .classify_line("greet player", LineRole::Statement)
            .expect("the custom effect");
        assert!(catalog.is_workspace(id));
        assert_eq!(catalog.entry(id).unwrap().name, "greet");

        // The `on` wrapper is undone for custom events exactly as for core ones.
        let (id, matched) = catalog
            .classify_line("on greeting of player:", LineRole::TopLevel)
            .expect("the custom event");
        assert_eq!(catalog.entry(id).unwrap().name, "greeting");
        let slot = &matched.captures[0];
        assert_eq!(&"on greeting of player"[slot.start..slot.end], "player");

        let (id, _) = catalog
            .classify_line("the answer", LineRole::Expression)
            .expect("the custom expression");
        assert_eq!(id.category, Category::Expression);

        // Core syntax is untouched, and completion sees the project's entries.
        assert!(catalog.classify_line("stop", LineRole::Statement).is_some());
        assert!(catalog
            .search(Category::Effect, "")
            .iter()
            .any(|(_, entry)| entry.name == "greet"));
        assert_eq!(catalog.workspace_entries().count(), 3);
        assert_eq!(
            catalog.find_by_name("Greet").map(|(i, _)| i.category),
            Some(Category::Effect)
        );
    }

    /// Replacing the set must forget answers given under the old one.
    #[test]
    fn replacing_workspace_entries_invalidates_earlier_answers() {
        let mut catalog = Catalog::build(fallback_docs());
        catalog.set_workspace_entries([(Category::Effect, custom("a", &["wave at %player%"]))], 1);
        assert!(catalog
            .classify_line("wave at player", LineRole::Statement)
            .is_some());

        catalog.set_workspace_entries([(Category::Effect, custom("a", &["wave to %player%"]))], 2);
        assert!(catalog
            .classify_line("wave at player", LineRole::Statement)
            .is_none());
        assert!(catalog
            .classify_line("wave to player", LineRole::Statement)
            .is_some());

        catalog.set_workspace_entries([], 3);
        assert!(catalog
            .classify_line("wave to player", LineRole::Statement)
            .is_none());
        assert!(catalog.classify_line("stop", LineRole::Statement).is_some());
    }

    #[test]
    fn search_matches_names_and_patterns() {
        let catalog = Catalog::build(fallback_docs());
        assert!(!catalog.search(Category::Structure, "command").is_empty());
        assert!(catalog.search(Category::Structure, "zzzz").is_empty());
    }
}
