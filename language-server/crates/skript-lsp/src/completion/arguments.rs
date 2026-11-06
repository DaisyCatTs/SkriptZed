//! What can fill the slot at the cursor: `kill pl` offers `player`.
//!
//! The rest of completion offers whole syntax, which replaces the statement
//! from its start. Once the start of a statement already identifies its
//! syntax (`teleport player to `), the useful suggestions are the values for
//! the `%slot%` being typed, and accepting one must replace only the word
//! under the cursor: replacing the phrase is how pressing Enter on a finished
//! `kill player` once turned it into `player from uuids`.

use std::collections::{HashMap, HashSet};

use skript_docs::{Catalog, Category, Entry};
use skript_index::Document;
use skript_syntax::{OpenSlot, Pattern};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    InsertTextFormat, TextEdit,
};

use super::events;
use super::ranking::{filter_text_for, leading_keywords, relevance, sort_key, TOP_SCORE};
use super::snippet::snippet_for_typed;
use crate::convert::{to_lsp_range, Encoding};

/// Above every whole-phrase item: past the first word those are already
/// filtered down to syntax that covers the phrase, and none of them can be
/// what fills a slot.
const ARGUMENT_BASE: i32 = 3_000;
/// An event value of a fitting type. The likeliest argument in a trigger:
/// `kill pl` inside `on death:` almost always means the event's own entity.
const EVENT_VALUE_BASE: i32 = 5_000;
/// Taken off an expression that fits only through a supertype, so `entity`
/// expressions lead in an `%entities%` slot and `player` ones follow.
const SUBTYPE_PENALTY: i32 = 200;
/// Taken off an expression whose documented return type is `object`: it may
/// fit, but says nothing about whether it does.
const OBJECT_PENALTY: i32 = 1_000;
/// Taken off addon syntax, as the whole-phrase items do.
const ADDON_PENALTY: i32 = 60;

/// Supertypes and the conversions Skript performs implicitly, by type id.
///
/// Skript's `docs.json` publishes no type hierarchy, so without this a
/// `player` expression would never fit an `%entities%` slot. Deliberately
/// small: only relations every Skript version has. Conversions that are real
/// but surprising stay out (an entity is accepted where a location is
/// expected), because `teleport player to ` should offer locations, not
/// every entity expression in the catalog.
const SUPERTYPES: &[(&str, &[&str])] = &[
    (
        "player",
        &[
            "offlineplayer",
            "livingentity",
            "entity",
            "commandsender",
            "audience",
        ],
    ),
    ("livingentity", &["entity"]),
    ("projectile", &["entity"]),
    ("commandsender", &["audience"]),
    ("itemstack", &["itemtype"]),
];

/// Types whose literal is another type's value with something after it:
/// `sharpness 5` is an enchantment type, written as an enchantment and a
/// level. Without this an `%enchantment types%` slot offered nothing.
const VALUES_OF: &[(&str, &str)] = &[("enchantmenttype", "enchantment")];

/// Lists longer than this are left out. Particles (333 values) would fill the
/// capped list on their own, and nobody browses that many by prefix.
const MAX_LISTED_VALUES: usize = 150;

/// The catch-all type: a slot of this type takes anything.
const OBJECT: &str = "object";
/// `%*classinfo%`: a type name, such as `player` in `parsed as player`.
const CLASSINFO: &str = "classinfo";

pub struct Request<'a> {
    pub catalog: &'a Catalog,
    pub document: &'a Document,
    pub cursor: skript_index::Position,
    /// The line up to the cursor.
    pub prefix: &'a str,
    /// The client asked for completions explicitly (Ctrl-Space or a trigger
    /// character) rather than because a word character was typed.
    pub explicit: bool,
    /// Whether to offer the trigger's locals (`features.variable_completion`).
    pub locals: bool,
    pub encoding: Encoding,
}

/// Items for the slot the cursor is starting, each replacing only the word
/// being typed. Empty when the line does not put the cursor at a slot.
pub fn argument_items(request: &Request) -> Vec<(i32, CompletionItem)> {
    let Some(at) = Position::of(request.prefix) else {
        return Vec::new();
    };
    // A request with nothing typed only ever comes from the user asking: Zed
    // requests on word characters, never on a space. Answering one that did
    // not ask (another client, or a request with no context) would open a
    // popup that Enter then accepts.
    if at.word.is_empty() && !request.explicit {
        return Vec::new();
    }
    let slots = open_slots(request.catalog, at.statement);
    if slots.is_empty() {
        return Vec::new();
    }

    let types = TypeNames::new(request.catalog);
    let wanted = Wanted::from_slots(&slots, &types);
    let word = at.word.to_lowercase();
    let mut candidates = Vec::new();
    if wanted.expressions {
        candidates.extend(expressions(request.catalog, &wanted, &types, &word));
        candidates.extend(event_values(request, &wanted, &types, &word));
        if word.is_empty() {
            candidates.extend(locals(request));
        }
    }
    candidates.extend(literals(request.catalog, &wanted, &word));

    // A word that already spells out one of the offered values means the
    // argument is written and the line is probably finished. Offering the
    // rest there put a popup under every finished `kill player`, and Enter
    // accepts whatever it shows first.
    if !word.is_empty()
        && candidates
            .iter()
            .any(|candidate| candidate.insert.eq_ignore_ascii_case(&word))
    {
        return Vec::new();
    }

    let range = to_lsp_range(
        request.document.text(),
        skript_index::Range::new(
            skript_index::Position::new(request.cursor.line, at.word_start as u32),
            request.cursor,
        ),
        request.encoding,
    );
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert((candidate.label.clone(), candidate.insert.clone())))
        .map(|candidate| (candidate.score, candidate.into_item(range)))
        .collect()
}

/// Where the cursor is, in the terms argument completion needs.
struct Position<'a> {
    /// The statement before the word being typed, without its indentation.
    statement: &'a str,
    /// The word being typed: the text an accepted item replaces.
    word: &'a str,
    /// Byte column where `word` starts.
    word_start: usize,
}

impl<'a> Position<'a> {
    fn of(prefix: &'a str) -> Option<Self> {
        let word_start = prefix
            .char_indices()
            .rev()
            .take_while(|&(_, c)| c.is_alphanumeric() || c == '_' || c == '-')
            .last()
            .map_or(prefix.len(), |(index, _)| index);
        let indent = skript_index::indent_width(prefix);
        // Only an indented statement: column 0 opens a structure, whose
        // header is completed elsewhere.
        if indent == 0 || word_start < indent {
            return None;
        }
        let statement = &prefix[indent..word_start];
        // The word must stand on its own (`player's`, `{_x`, `%pla` are
        // something else), after at least one word of the statement.
        if statement.is_empty() || !statement.ends_with([' ', '\t']) {
            return None;
        }
        if !balanced(statement) {
            return None;
        }
        Some(Self {
            statement,
            word: &prefix[word_start..],
            word_start,
        })
    }
}

/// Whether `text` closes every string, brace and paren it opens. Inside one,
/// the cursor is in a string or a nested expression, not at a slot of the
/// statement.
fn balanced(text: &str) -> bool {
    let mut in_string = false;
    let (mut braces, mut parens) = (0i32, 0i32);
    for ch in text.chars() {
        match ch {
            '"' => in_string = !in_string,
            _ if in_string => {}
            '{' => braces += 1,
            '}' => braces -= 1,
            '(' => parens += 1,
            ')' => parens -= 1,
            _ => {}
        }
    }
    !in_string && braces == 0 && parens == 0
}

/// The slots the statement's likeliest syntax leaves open at its end.
///
/// `if`, `else if`, `while` and `do while` wrap a condition whose own pattern
/// never contains the keyword, so it is stripped and only conditions are
/// tried; for those, a pattern may open with its slot (`%players% are op`).
/// Any other statement must open with a keyword of the syntax, which keeps
/// the parse to a few dozen patterns per keystroke.
fn open_slots(catalog: &Catalog, statement: &str) -> Vec<OpenSlot> {
    let lower = statement.to_ascii_lowercase();
    let condition = ["else if ", "do while ", "while ", "if "]
        .into_iter()
        .find(|keyword| lower.starts_with(keyword));
    let (categories, rest): (&[Category], &str) = match condition {
        Some(keyword) => (
            &[Category::Condition],
            statement[keyword.len()..].trim_start(),
        ),
        None => (
            &[Category::Effect, Category::Section, Category::Condition],
            statement,
        ),
    };
    let first = rest
        .split_whitespace()
        .next()
        .map(str::to_lowercase)
        .unwrap_or_default();

    let mut found: Vec<OpenSlot> = Vec::new();
    for &category in categories {
        for (_, entry) in catalog.search(category, "") {
            for source in &entry.patterns {
                let keywords = leading_keywords(source);
                let plausible = if keywords.is_empty() {
                    condition.is_some()
                } else {
                    !first.is_empty()
                        && keywords
                            .iter()
                            .any(|keyword| first.starts_with(&keyword.to_lowercase()))
                };
                if !plausible {
                    continue;
                }
                if let Ok(pattern) = Pattern::parse(source) {
                    found.extend(skript_syntax::open_slots(&pattern, rest));
                }
            }
        }
    }
    // Only the syntax that explains most of the statement. An addon's
    // `teleport %objects%` must not widen `teleport player to `'s location
    // slot into one that takes anything.
    let best = found.iter().map(|open| open.literals).max().unwrap_or(0);
    found.retain(|open| open.literals == best);
    found
}

/// Type names as `docs.json` knows them, for resolving slot and return types
/// to one id.
///
/// Slots spell a type as the user reads it (`living entities`, `item types`),
/// return types by id or display name (`livingentity`, `Living Entity`), and
/// a few ids differ from their name entirely (`entitydata` is "Entity Type").
struct TypeNames {
    by_key: HashMap<String, String>,
}

impl TypeNames {
    fn new(catalog: &Catalog) -> Self {
        let mut by_key = HashMap::new();
        for entry in catalog.docs().entries(Category::Type) {
            let id = entry.id.to_lowercase();
            by_key.insert(key(&entry.name), id.clone());
            by_key.insert(key(&entry.id), id);
        }
        Self { by_key }
    }

    /// The type id `name` refers to, singular or plural.
    fn resolve(&self, name: &str) -> String {
        let name = key(name);
        let mut forms = vec![name.clone()];
        if let Some(stem) = name.strip_suffix("ies") {
            forms.push(format!("{stem}y"));
        }
        if let Some(stem) = name.strip_suffix('s') {
            forms.push(stem.to_string());
        }
        if let Some(stem) = name.strip_suffix("es") {
            forms.push(stem.to_string());
        }
        if let Some(id) = forms.iter().find_map(|form| self.by_key.get(form)) {
            return id.clone();
        }
        // Not a documented type (a catalog without types, or an addon's):
        // the singular form is the best guess at its id.
        forms.get(1).cloned().unwrap_or(name)
    }
}

/// The types a value of type `id` also is, by [`SUPERTYPES`].
pub(super) fn supertypes(id: &str) -> &'static [&'static str] {
    SUPERTYPES
        .iter()
        .find(|(ty, _)| *ty == id)
        .map_or(&[][..], |(_, supertypes)| supertypes)
}

/// `Living Entity`, `living entities` and `living-entity` all key alike.
pub(super) fn key(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// What the open slots accept.
struct Wanted {
    /// Resolved type ids.
    types: Vec<String>,
    /// Whether an expression may fill the slot, rather than only a literal
    /// (`%*type%`).
    expressions: bool,
}

impl Wanted {
    fn from_slots(slots: &[OpenSlot], names: &TypeNames) -> Self {
        let mut types = Vec::new();
        for open in slots {
            for name in &open.slot.types {
                let id = names.resolve(name);
                if !types.contains(&id) {
                    types.push(id);
                }
            }
        }
        Self {
            types,
            expressions: slots.iter().any(|open| !open.slot.literal_only),
        }
    }

    /// How well a value of type `id` fills the slot, as a score adjustment,
    /// or `None` when it does not fit.
    fn fit(&self, id: &str) -> Option<i32> {
        if self.types.iter().any(|ty| ty == OBJECT || ty == id) {
            return Some(0);
        }
        let supertypes = supertypes(id);
        if self
            .types
            .iter()
            .any(|ty| supertypes.contains(&ty.as_str()))
        {
            return Some(-SUBTYPE_PENALTY);
        }
        (id == OBJECT).then_some(-OBJECT_PENALTY)
    }
}

/// One suggestion before it becomes a completion item.
struct Candidate {
    label: String,
    /// Inserted in place of the word being typed; may be a snippet.
    insert: String,
    snippet: bool,
    kind: CompletionItemKind,
    detail: String,
    description: String,
    filter: String,
    score: i32,
    /// `completionItem/resolve` data, for catalog entries.
    data: Option<serde_json::Value>,
}

impl Candidate {
    fn into_item(self, range: tower_lsp::lsp_types::Range) -> CompletionItem {
        CompletionItem {
            sort_text: Some(sort_key(self.score, &self.label)),
            label: self.label,
            kind: Some(self.kind),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(self.description),
            }),
            detail: Some(self.detail),
            filter_text: Some(self.filter),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: self.insert,
            })),
            insert_text_format: Some(if self.snippet {
                InsertTextFormat::SNIPPET
            } else {
                InsertTextFormat::PLAIN_TEXT
            }),
            data: self.data,
            ..Default::default()
        }
    }
}

/// Whether `word` (lowercase) starts `text` or one of its words.
fn starts_a_word(text: &str, word: &str) -> bool {
    word.is_empty()
        || text
            .to_lowercase()
            .split(|c: char| c.is_whitespace() || c == '-')
            .any(|part| part.starts_with(word))
}

/// Expressions whose return type fits, inserted as the pattern the user is
/// typing the opening keyword of.
fn expressions(
    catalog: &Catalog,
    wanted: &Wanted,
    types: &TypeNames,
    word: &str,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    for category in [Category::Expression, Category::Property] {
        for (id, entry) in catalog.search(category, "") {
            let Some(returns) = &entry.returns else {
                continue;
            };
            let fit = [returns.id.as_str(), returns.name.as_str()]
                .into_iter()
                .filter(|name| !name.is_empty())
                .find_map(|name| wanted.fit(&types.resolve(name)));
            let Some(fit) = fit else {
                continue;
            };
            let Some(pattern) = typed_pattern(entry, word) else {
                continue;
            };
            let insert = snippet_for_typed(pattern, word);
            let filter = filter_text_for(&entry.name, pattern);
            let penalty = if entry.addon.is_some() {
                ADDON_PENALTY
            } else {
                0
            };
            out.push(Candidate {
                label: entry.name.clone(),
                snippet: insert.contains('$'),
                kind: CompletionItemKind::FUNCTION,
                detail: format!("{} · {pattern}", returns.display()),
                description: returns.display().to_string(),
                score: ARGUMENT_BASE + relevance(&insert, word) + fit - penalty,
                filter,
                insert,
                data: Some(serde_json::json!([category.label(), id.index])),
            });
        }
    }
    out
}

/// The pattern of `entry` whose opening keyword `word` begins, or with
/// nothing typed the first that opens with a keyword at all.
///
/// A pattern that opens with a slot (`%worlds%'[s] spawn`) is never chosen:
/// what is being typed is a word, and inserting a tab stop in its place would
/// throw the word away.
fn typed_pattern<'e>(entry: &'e Entry, word: &str) -> Option<&'e str> {
    entry
        .patterns
        .iter()
        .find(|pattern| {
            leading_keywords(pattern)
                .iter()
                .any(|keyword| keyword.to_lowercase().starts_with(word))
        })
        .map(String::as_str)
}

/// The enclosing event's values of a fitting type, written the short way
/// (`player`, which Skript reads as `event-player`).
///
/// Once the word heads for `event-` the event-value completion offers them
/// itself, so they are left to it rather than listed twice.
fn event_values(
    request: &Request,
    wanted: &Wanted,
    types: &TypeNames,
    word: &str,
) -> Vec<Candidate> {
    if events::heading_for_event_value(word) {
        return Vec::new();
    }
    let Some(values) =
        events::enclosing_event_values(request.catalog, request.document, request.cursor.line)
    else {
        return Vec::new();
    };
    values
        .into_iter()
        // `past event-x` has no short form that means the same thing.
        .filter(|(label, _)| label.starts_with("event-"))
        .filter_map(|(label, type_name)| {
            let fit = wanted.fit(&types.resolve(&type_name))?;
            starts_a_word(&type_name, word).then(|| Candidate {
                score: EVENT_VALUE_BASE + relevance(&type_name, word) + fit,
                detail: format!("event value · {label}"),
                description: "event value".into(),
                filter: format!("{type_name} {label}"),
                insert: type_name.clone(),
                label: type_name,
                snippet: false,
                kind: CompletionItemKind::VALUE,
                data: None,
            })
        })
        .collect()
}

/// The trigger's locals, when nothing is typed yet. Once a word is typed the
/// bare-word local completion offers them, matched on the name.
fn locals(request: &Request) -> Vec<Candidate> {
    if !request.locals {
        return Vec::new();
    }
    super::trigger_locals(request.document, request.cursor.line)
        .iter()
        .filter(|name| name.starts_with('_'))
        .map(|name| {
            let text = format!("{{{name}}}");
            Candidate {
                label: text.clone(),
                insert: text.clone(),
                snippet: false,
                kind: CompletionItemKind::VARIABLE,
                detail: "local variable".into(),
                description: "local variable".into(),
                filter: text,
                score: TOP_SCORE,
                data: None,
            }
        })
        .collect()
}

/// Literal values for a slot whose type lists them: a game mode's
/// `survival`, a type name for `%*classinfo%`.
///
/// Read from each type's documented patterns, and only from a plain list of
/// words: item types and entity types are open-ended (`[<number> [of]]
/// <alias>`, `…, etc.`) and listing them would be both huge and wrong.
fn literals(catalog: &Catalog, wanted: &Wanted, word: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    for type_id in &wanted.types {
        let values: Vec<String> = if type_id == CLASSINFO {
            catalog
                .docs()
                .entries(Category::Type)
                .iter()
                .map(|entry| entry.name.to_lowercase())
                .collect()
        } else {
            let listed = VALUES_OF
                .iter()
                .find(|(ty, _)| ty == type_id)
                .map_or(type_id.as_str(), |(_, values)| values);
            catalog
                .docs()
                .entries(Category::Type)
                .iter()
                .find(|entry| entry.id.eq_ignore_ascii_case(listed))
                .map(|entry| enum_values(&entry.patterns))
                .filter(|values| values.len() <= MAX_LISTED_VALUES)
                .unwrap_or_default()
        };
        for value in values {
            if !starts_a_word(&value, word) {
                continue;
            }
            out.push(Candidate {
                score: ARGUMENT_BASE + relevance(&value, word),
                detail: format!("{type_id} value"),
                description: type_id.clone(),
                filter: value.clone(),
                insert: value.clone(),
                label: value,
                snippet: false,
                kind: CompletionItemKind::ENUM_MEMBER,
                data: None,
            });
        }
    }
    out
}

/// The values a type's patterns list, when they are a plain list.
///
/// `adventure, survival, spectator, creative` is; `player, op, wolf, …, etc.`
/// and `[<number> [of]] <alias>` are not.
fn enum_values(patterns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for pattern in patterns {
        let lower = pattern.to_lowercase();
        // Matched as whole items: a substring test for `etc` threw away the
        // villager professions, because `fletcher` contains it.
        let open_ended = lower.starts_with("see ")
            || lower
                .split(',')
                .any(|item| matches!(item.trim(), "etc" | "etc." | "..." | "…"));
        // Parentheses and `:` are judged per value below: `leashed
        // (gone|disappeared)` and `minecraft:entity_drops` are values, while
        // these only ever appear in a description or a syntax sketch.
        let markup = pattern.contains(['<', '>', '[', ']', '#', '"', '{', '%', '&', ';', '$']);
        // One value per pattern is a list too (particles are written that
        // way); one pattern with no comma is a description such as boolean's
        // `true/yes/on or false/no/off`.
        let listed = pattern.contains(',') || patterns.len() > 1;
        if open_ended || markup || !listed {
            continue;
        }
        for item in pattern.split(',') {
            for value in expand_item(item) {
                if !value.is_empty() && !out.contains(&value) {
                    out.push(value);
                }
            }
        }
    }
    out
}

/// The values one comma-separated item spells: `a/b` is two, `leashed
/// (gone|disappeared)` is two, `damage animation (non-functional)` is one
/// with its note dropped. A leading `and`/`or`/`a`/`an` is not part of the
/// value: `a lightning` and `lightning` are the same damage cause.
fn expand_item(item: &str) -> Vec<String> {
    let item = item.trim();
    let (head, choices, tail) = match (item.find('('), item.find(')')) {
        (Some(open), Some(close)) if open < close => {
            let inner = &item[open + 1..close];
            let choices: Vec<&str> = if inner.contains('|') {
                inner.split('|').collect()
            } else {
                vec![""]
            };
            (&item[..open], choices, &item[close + 1..])
        }
        _ => (item, vec![""], ""),
    };
    let mut out = Vec::new();
    for choice in choices {
        let joined = format!("{head} {choice} {tail}");
        for value in joined.split('/') {
            let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
            let value = ["and ", "or ", "a ", "an "]
                .iter()
                .find_map(|lead| value.strip_prefix(lead))
                .unwrap_or(&value)
                .to_string();
            out.push(value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_list_of_values_is_read_and_a_description_is_not() {
        let values = |patterns: &[&str]| {
            enum_values(&patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(
            values(&["adventure, survival, spectator, creative"]),
            vec!["adventure", "survival", "spectator", "creative"]
        );
        assert_eq!(
            values(&["clear/sun, rain, and thunder"]),
            vec!["clear", "sun", "rain", "thunder"]
        );
        assert!(values(&["player, op, wolf, zombie, etc."]).is_empty());
        assert!(values(&["[<number> [of]] [all/every] <alias>"]).is_empty());
        assert!(values(&["true/yes/on or false/no/off"]).is_empty());
        assert!(values(&["&lt;number&gt; $ or $ &lt;number&gt;, where"]).is_empty());
    }

    #[test]
    fn a_word_containing_etc_does_not_make_a_list_open_ended() {
        let values = enum_values(&["butcher, fletcher, librarian".to_string()]);
        assert_eq!(values, vec!["butcher", "fletcher", "librarian"]);
    }

    #[test]
    fn choices_notes_articles_and_namespaces_are_read_per_value() {
        let values = enum_values(&[
            "leashed (gone|disappeared), damage animation (non-functional), a lightning, \
             lightning, minecraft:entity_drops"
                .to_string(),
        ]);
        assert_eq!(
            values,
            vec![
                "leashed gone",
                "leashed disappeared",
                "damage animation",
                "lightning",
                "minecraft:entity_drops"
            ]
        );
    }

    /// Every type in the real `docs.json` that lists its values, at most
    /// [`MAX_LISTED_VALUES`] of them, yields that list, and none of the open
    /// ended ones (items, entities) does. Skips without the file.
    #[test]
    fn every_listed_type_in_the_real_database_yields_its_values() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../vendor/docs.json");
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("vendor/docs.json not present, skipping");
            return;
        };
        let docs = skript_docs::Docs::parse(&text).expect("docs.json parses");
        let count = |id: &str| {
            docs.entries(Category::Type)
                .iter()
                .find(|entry| entry.id == id)
                .map_or(0, |entry| enum_values(&entry.patterns).len())
        };
        for id in [
            "damagecause",
            "biome",
            "potioneffecttype",
            "enchantment",
            "villagerprofession",
            "gamerule",
            "unleashreason",
            "entityeffect",
            "gamemode",
            "teleportcause",
        ] {
            let found = count(id);
            assert!(
                (1..=MAX_LISTED_VALUES).contains(&found),
                "{id} lists {found} values"
            );
        }
        for id in ["itemtype", "itemstack", "entity", "entitydata", "boolean"] {
            assert_eq!(count(id), 0, "{id} is not a plain list");
        }
    }

    #[test]
    fn slot_and_return_type_spellings_resolve_to_one_id() {
        let catalog = Catalog::build(
            skript_docs::Docs::parse(
                r#"{ "types": [
                    { "id": "livingentity", "name": "Living Entity" },
                    { "id": "entitydata", "name": "Entity Type" },
                    { "id": "itemtype", "name": "Item Type" }
                ] }"#,
            )
            .unwrap(),
        );
        let names = TypeNames::new(&catalog);
        assert_eq!(names.resolve("living entities"), "livingentity");
        assert_eq!(names.resolve("Living Entity"), "livingentity");
        assert_eq!(names.resolve("entity types"), "entitydata");
        assert_eq!(names.resolve("item types"), "itemtype");
        // Undocumented: the singular is the guess.
        assert_eq!(names.resolve("players"), "player");
        assert_eq!(names.resolve("entities"), "entity");
    }

    #[test]
    fn a_player_fits_an_entity_slot_but_an_entity_does_not_fit_a_location() {
        let wanted = |types: &[&str]| Wanted {
            types: types.iter().map(|t| t.to_string()).collect(),
            expressions: true,
        };
        assert_eq!(wanted(&["entity"]).fit("entity"), Some(0));
        assert!(wanted(&["entity"]).fit("player").is_some());
        assert!(wanted(&["location"]).fit("entity").is_none());
        assert!(wanted(&["object"]).fit("location").is_some());
        // Unknown is offered, but last.
        assert!(wanted(&["location"]).fit("object").unwrap() < -SUBTYPE_PENALTY);
    }

    #[test]
    fn the_cursor_must_start_a_word_inside_a_statement() {
        assert!(Position::of("\tkill pl").is_some());
        assert!(Position::of("\tkill ").is_some());
        assert!(Position::of("\tkill").is_none(), "still the first word");
        assert!(Position::of("kill pl").is_none(), "column 0");
        assert!(Position::of("\tsend \"hi pl").is_none(), "inside a string");
        assert!(
            Position::of("\tset {_x} to f(pl").is_none(),
            "inside a call"
        );
        assert!(Position::of("\tsend player's").is_none());
    }
}
