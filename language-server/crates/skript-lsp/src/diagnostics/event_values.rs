//! `player` where the trigger has no player: Skript's most common load error,
//! "There's no player in a function event."
//!
//! Skript resolves `player`, `event-<type>`, `attacker` and `victim` against
//! the events the trigger runs in, and refuses the line when none of them
//! supplies the value. Reported only where that is certain from the text:
//!
//! * the trigger is a function, whose event has no values at all, or an event
//!   from Skript's own database that the header names unambiguously and whose
//!   documented values are all of types that can never stand for the one
//!   asked for. Skript downcasts and converts event values: an `on load`
//!   event's command sender passes for `player` (it is just empty at run
//!   time), and a location converts to a block. So `on load` is never judged,
//!   and nor is any event with no documented values, or any addon event;
//! * the word fills a slot as a whole expression (`send "hi" to player`, or
//!   `player` inside `player's name`), found through the catalog's own slot
//!   captures. A slot that a literal could fill instead (`%objects%`, an
//!   entity or item type, `%*type%`) is skipped: there Skript may read
//!   `player` as the entity type and succeed;
//! * no addon or project-defined expression could claim the word first;
//! * every section between the trigger and the line is core control flow. A
//!   section such as `spawn a zombie:` runs its body with values of its own.
//!
//! Commands, ClassySK methods and skript-reflect sections are never checked.
//! Still a warning rather than an error: the verdict rests on the database's
//! description of each event.

use skript_docs::{Catalog, Category, Entry, EntryId, LineRole};
use skript_index::{Document, Range, SymbolKind};
use skript_syntax::Slot;

use super::layout::{is_control_flow, Lines};
use super::{Diagnostic, Severity};

pub(super) const CODE: &str = "no-event-value";

/// Event value types that never become a player or an entity, by cast or by
/// any of Skript's converters.
const NOT_AN_ENTITY: &[&str] = &[
    "world",
    "chunk",
    "location",
    "worldborder",
    "number",
    "integer",
    "timespan",
    "time",
    "date",
    "biome",
    "weathertype",
];

/// The same for a block. A location converts to its block, and a chunk is
/// left out because nothing here proves it does not.
const NOT_A_BLOCK: &[&str] = &[
    "world",
    "worldborder",
    "number",
    "integer",
    "timespan",
    "time",
    "date",
    "biome",
    "weathertype",
];

/// Slot types a literal may fill, where Skript tries `player` as an entity
/// or item type when the event value fails.
const LITERAL_TYPES: &[&str] = &["object", "entitytype", "entitydata", "entity type", "item"];

/// What a word asks the trigger for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Player,
    Entity,
    Block,
    /// An event-restricted expression, named as in the database.
    Restricted(&'static str),
}

impl Wanted {
    fn of(text: &str) -> Option<Wanted> {
        let lower = text.trim().to_ascii_lowercase();
        let bare = lower.strip_prefix("the ").unwrap_or(&lower).trim();
        Some(match bare {
            "player" | "event-player" => Wanted::Player,
            "event-entity" => Wanted::Entity,
            "event-block" => Wanted::Block,
            "attacker" => Wanted::Restricted("Attacker"),
            "victim" => Wanted::Restricted("Attacked"),
            _ => return None,
        })
    }

    fn type_name(self) -> &'static str {
        match self {
            Wanted::Player => "player",
            Wanted::Entity => "entity",
            Wanted::Block => "block",
            Wanted::Restricted(_) => "",
        }
    }
}

/// What the trigger runs in.
enum Trigger<'c> {
    /// Skript's `FunctionEvent`, which carries no values.
    Function {
        local: bool,
    },
    Event(&'c Entry),
}

impl Trigger<'_> {
    /// The name Skript prints: `ParserInstance.getCurrentEventName`.
    fn name(&self) -> String {
        match self {
            Trigger::Function { local: false } => "function".into(),
            Trigger::Function { local: true } => "local function".into(),
            Trigger::Event(entry) => entry.name.to_lowercase(),
        }
    }

    fn lacks(&self, wanted: Wanted) -> bool {
        let Trigger::Event(entry) = self else {
            return true;
        };
        let inert = match wanted {
            Wanted::Block => NOT_A_BLOCK,
            _ => NOT_AN_ENTITY,
        };
        entry.event_values.iter().all(|value| {
            let id = value.id.trim().to_ascii_lowercase();
            inert.contains(&id.as_str())
        })
    }
}

/// Skript's `Utils.a`.
fn with_article(word: &str) -> String {
    let article = match word.chars().next() {
        Some(first) if "aeiouAEIOU".contains(first) => "an",
        _ => "a",
    };
    format!("{article} {word}")
}

fn is_core(catalog: &Catalog, id: EntryId, entry: &Entry) -> bool {
    entry.addon.is_none() && entry.skripthub_id.is_none() && !catalog.is_workspace(id)
}

/// The byte span of the event inside `header`. `on`, a listening modifier
/// and a priority are Skript's event wrapper, not part of any event's
/// pattern.
pub(super) fn event_span(header: &str) -> (usize, usize) {
    // ASCII lowercasing keeps byte offsets.
    let lower = header.to_ascii_lowercase();
    let mut start = 0;
    if lower.starts_with("on ") {
        start = 3;
        start += lower[start..].len() - lower[start..].trim_start().len();
        for modifier in ["uncancelled ", "cancelled ", "any ", "all "] {
            if lower[start..].starts_with(modifier) {
                start += modifier.len();
                break;
            }
        }
    }
    let end = lower[start..]
        .rfind(" with priority ")
        .map_or(header.len(), |at| start + at);
    let inner = &header[start..end];
    let start = start + (inner.len() - inner.trim_start().len());
    (start, start + inner.trim().len())
}

fn bare_event(header: &str) -> &str {
    let (start, end) = event_span(header);
    &header[start..end]
}

/// The core event `header` names, when it names exactly one and documents
/// what it carries.
fn trigger_event<'c>(catalog: &'c Catalog, header: &str) -> Option<&'c Entry> {
    let (id, _) = catalog.classify_line(header, LineRole::TopLevel)?;
    if id.category != Category::Event {
        return None;
    }
    let entry = catalog.entry(id)?;
    if !is_core(catalog, id, entry) || entry.event_values.is_empty() {
        return None;
    }
    let bare = bare_event(header);
    let ambiguous = catalog.classify(bare).into_iter().any(|(other, matched)| {
        other != id && other.category == Category::Event && matched.explains_text(bare)
    });
    (!ambiguous).then_some(entry)
}

fn literal_could_fill(slot: &Slot) -> bool {
    slot.literal_only
        || slot.types.is_empty()
        || slot.types.iter().any(|ty| {
            let ty = ty.to_ascii_lowercase();
            LITERAL_TYPES.iter().any(|prefix| ty.starts_with(prefix))
        })
}

/// Whether syntax that is not core Skript's could read `text` first.
fn claimed_elsewhere(catalog: &Catalog, text: &str) -> bool {
    catalog.classify(text).into_iter().any(|(id, matched)| {
        id.category == Category::Expression
            && matched.explains_text(text)
            && catalog
                .entry(id)
                .is_some_and(|entry| !is_core(catalog, id, entry))
    })
}

/// One word found filling a slot: its byte span in the line, and what it
/// asks for.
struct Found {
    start: usize,
    end: usize,
    wanted: Wanted,
    text: String,
}

/// Walks the slots `text` fills when read in `role`, descending into the
/// expressions inside them. `offset` places `text` in the line.
fn visit(
    catalog: &Catalog,
    text: &str,
    offset: usize,
    role: LineRole,
    depth: u8,
    out: &mut Vec<Found>,
) {
    let Some((_, matched)) = catalog.classify_line(text, role) else {
        return;
    };
    for capture in &matched.captures {
        if capture.regex || literal_could_fill(&capture.slot) {
            continue;
        }
        let trimmed = capture.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lead = capture.text.len() - capture.text.trim_start().len();
        let start = offset + capture.start + lead;
        if let Some(wanted) = Wanted::of(trimmed) {
            out.push(Found {
                start,
                end: start + trimmed.len(),
                wanted,
                text: trimmed.to_string(),
            });
            continue;
        }
        // A string's interpolations and a variable's name are not slots.
        if depth < 3 && !trimmed.starts_with(['"', '{']) {
            visit(
                catalog,
                trimmed,
                start,
                LineRole::Expression,
                depth + 1,
                out,
            );
        }
    }
}

/// Skript's `supportedEventsNames`, from the expression's documented events.
fn restricted_message(
    catalog: &Catalog,
    name: &str,
    typed: &str,
    trigger: &Trigger<'_>,
) -> Option<String> {
    let (id, entry) = catalog.find_by_name(name)?;
    if id.category != Category::Expression
        || !is_core(catalog, id, entry)
        || entry.events.is_empty()
    {
        return None;
    }
    if let Trigger::Event(event) = trigger {
        if entry.events.iter().any(|allowed| allowed.id == event.id) {
            return None;
        }
    }
    let names: Vec<String> = entry
        .events
        .iter()
        .map(|event| format!("the {} event", event.display().to_lowercase()))
        .collect();
    let list = match names.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => return None,
    };
    Some(format!("'{typed}' can only be used in {list}"))
}

pub(super) fn check(document: &Document, catalog: &Catalog, out: &mut Vec<Diagnostic>) {
    // A broken parse may have put a line in the wrong trigger.
    if document.has_errors() {
        return;
    }
    let lines = Lines::new(document.text());

    for owner in &document.symbols().symbols {
        let start = owner.range.start.line as usize;
        let trigger = match owner.kind {
            SymbolKind::Function => Trigger::Function { local: false },
            SymbolKind::LocalFunction => Trigger::Function { local: true },
            SymbolKind::Event => {
                let Some(header) = lines.code(start) else {
                    continue;
                };
                match trigger_event(catalog, header.trim().trim_end_matches(':').trim_end()) {
                    Some(entry) => Trigger::Event(entry),
                    None => continue,
                }
            }
            _ => continue,
        };

        let end = (owner.range.end.line as usize).min(lines.lines.len().saturating_sub(1));
        for number in start + 1..=end {
            let Some(code) = lines.code(number) else {
                continue;
            };
            let in_plain_flow = lines
                .enclosing_headers(number)
                .split_last()
                .is_some_and(|(_, inner)| inner.iter().all(|header| is_control_flow(header)));
            if !in_plain_flow {
                continue;
            }
            let indent = skript_index::indent_width(code);
            let mut found = Vec::new();
            visit(
                catalog,
                code.trim(),
                indent,
                LineRole::Statement,
                0,
                &mut found,
            );

            for word in found {
                if !trigger.lacks(word.wanted) || claimed_elsewhere(catalog, &word.text) {
                    continue;
                }
                let message = match word.wanted {
                    Wanted::Restricted(name) => {
                        match restricted_message(catalog, name, &word.text, &trigger) {
                            Some(message) => message,
                            None => continue,
                        }
                    }
                    wanted => format!(
                        "There's no {} in {} event.",
                        wanted.type_name(),
                        with_article(&trigger.name())
                    ),
                };
                out.push(Diagnostic {
                    range: Range::on_line(number as u32, word.start as u32, word.end as u32),
                    severity: Severity::Warning,
                    message,
                    code: CODE,
                    related: Vec::new(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_index::Workspace;

    const DOCS: &str = r#"{
      "effects": [
        { "id": "send", "name": "Message", "patterns": ["send %strings% [to %commandsenders%]"] },
        { "id": "spawn", "name": "Spawn", "patterns": ["spawn %entitytypes% [at %locations%]"] },
        { "id": "kill", "name": "Kill", "patterns": ["kill %entities%"] },
        { "id": "set", "name": "Change", "patterns": ["set %~objects% to %objects%"] },
        { "id": "break", "name": "Break", "patterns": ["break %blocks%"] }
      ],
      "expressions": [
        { "id": "ExprEntity", "name": "Creature/Entity/Player", "patterns": ["[the] [event-]<.+>"] },
        { "id": "ExprName", "name": "Name", "patterns": ["[the] name of %players%", "%players%'[s] name"] },
        { "id": "ExprAllPlayers", "name": "All Players", "patterns": ["all players"] },
        { "id": "ExprAttacker", "name": "Attacker", "patterns": ["[the] (attacker|damager)"],
          "events": [{ "id": "damage", "name": "On Damage" }, { "id": "death", "name": "On Death" }] }
      ],
      "events": [
        { "id": "periodical", "name": "Periodical", "patterns": ["every %timespan%"],
          "eventValues": [{ "id": "world", "name": "world" }] },
        { "id": "world_load", "name": "On World Load", "patterns": ["world load[ing]"],
          "eventValues": [{ "id": "world", "name": "world" }] },
        { "id": "border", "name": "On Border Move", "patterns": ["border move"],
          "eventValues": [{ "id": "location", "name": "location" }] },
        { "id": "script_load", "name": "On Script Load/Unload", "patterns": ["[script] load"],
          "eventValues": [{ "id": "commandsender", "name": "command sender" }] },
        { "id": "join", "name": "On Join", "patterns": ["[player] join[ing]"],
          "eventValues": [{ "id": "player", "name": "player" }] },
        { "id": "ping", "name": "On Server List Ping", "patterns": ["server list ping"] }
      ]
    }"#;

    fn catalog() -> Catalog {
        Catalog::build(skript_docs::Docs::parse(DOCS).unwrap())
    }

    fn run(catalog: &Catalog, source: &str) -> Vec<Diagnostic> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let mut out = Vec::new();
        check(workspace.get("file:///t.sk").unwrap(), catalog, &mut out);
        out
    }

    fn found(source: &str) -> Vec<(u32, String)> {
        run(&catalog(), source)
            .into_iter()
            .map(|d| (d.range.start.line, d.message))
            .collect()
    }

    #[test]
    fn player_in_a_function_is_skripts_error_as_a_warning() {
        let out = run(&catalog(), "function f():\n\tsend \"hi\" to player\n");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].severity, Severity::Warning);
        assert_eq!(out[0].message, "There's no player in a function event.");
        assert_eq!(out[0].range, Range::on_line(1, 14, 20));
        assert_eq!(
            found("local function f():\n\tsend \"hi\" to the player\n")[0].1,
            "There's no player in a local function event."
        );
    }

    #[test]
    fn periodical_and_world_events_have_no_player() {
        assert_eq!(
            found("every 5 seconds:\n\tkill player\n")[0].1,
            "There's no player in a periodical event."
        );
        assert_eq!(
            found("on world load:\n\tif x:\n\t\tsend \"a\" to event-player\n")[0].1,
            "There's no player in an on world load event."
        );
        assert_eq!(
            found("every 1 minute:\n\tbreak event-block\n")[0].1,
            "There's no block in a periodical event."
        );
    }

    /// `player's name` still evaluates `player`.
    #[test]
    fn a_player_inside_another_expression_counts() {
        let out = found("function f():\n\tsend player's name\n");
        assert_eq!(out.len(), 1, "{out:?}");
        let out = run(&catalog(), "function f():\n\tsend name of player\n");
        assert_eq!(out[0].range, Range::on_line(1, 14, 20));
    }

    #[test]
    fn near_misses_are_silent() {
        for line in [
            "send \"player\"",
            "send {_player}",
            "send \"hi\" to all players",
            "send \"%player%\"",
            // A literal may fill these slots, as the entity type `player`.
            "spawn player",
            "set {_p} to player",
            // Not the word itself.
            "send \"a\" to players",
            "send \"a\" to playerlist",
        ] {
            let source = format!("function f():\n\t{line}\n");
            assert!(found(&source).is_empty(), "{line:?}: {:?}", found(&source));
        }
    }

    #[test]
    fn events_that_carry_or_may_carry_a_player_are_silent() {
        // The command sender downcasts to a player.
        assert!(found("on load:\n\tsend \"a\" to player\n").is_empty());
        assert!(found("on join:\n\tsend \"a\" to player\n").is_empty());
        // No documented values: nothing is known.
        assert!(found("on server list ping:\n\tsend \"a\" to player\n").is_empty());
        // Unknown to the catalog.
        assert!(found("on frobnicate:\n\tsend \"a\" to player\n").is_empty());
        // A location converts to a block, but never to a player.
        assert!(found("on border move:\n\tbreak event-block\n").is_empty());
        assert_eq!(found("on border move:\n\tkill player\n").len(), 1);
    }

    #[test]
    fn commands_and_other_triggers_are_never_judged() {
        assert!(found("command /a:\n\ttrigger:\n\t\tsend \"a\" to player\n").is_empty());
        assert!(found("class A:\n\tpublic m():\n\t\tsend \"a\" to player\n").is_empty());
    }

    /// `spawn … :` and addon sections run their body with values of their own.
    #[test]
    fn a_line_under_a_non_control_flow_section_is_skipped() {
        assert!(found("function f():\n\tspawn zombie:\n\t\tkill event-entity\n").is_empty());
        assert_eq!(
            found("function f():\n\tloop 3 times:\n\t\tkill player\n").len(),
            1
        );
    }

    #[test]
    fn attacker_outside_its_events_uses_skripts_message() {
        assert_eq!(
            found("function f():\n\tkill attacker\n")[0].1,
            "'attacker' can only be used in the on damage event or the on death event"
        );
    }

    #[test]
    fn an_addon_expression_that_claims_the_word_silences_it() {
        let mut addon = skript_docs::Docs::parse(
            r#"{ "expressions": [{ "id": "p", "name": "Plain Player", "patterns": ["player"] }] }"#,
        )
        .unwrap();
        addon.attribute_to(&skript_docs::model::AddonRef {
            name: "SomeAddon".into(),
            ..Default::default()
        });
        let mut docs = skript_docs::Docs::parse(DOCS).unwrap();
        docs.merge(addon);
        let catalog = Catalog::build(docs);
        assert!(run(&catalog, "function f():\n\tsend \"a\" to player\n").is_empty());
    }

    #[test]
    fn an_addon_event_is_never_judged() {
        let mut docs = skript_docs::Docs::parse(DOCS).unwrap();
        docs.attribute_to(&skript_docs::model::AddonRef {
            name: "SomeAddon".into(),
            ..Default::default()
        });
        let catalog = Catalog::build(docs);
        assert!(run(&catalog, "every 5 seconds:\n\tkill player\n").is_empty());
    }
}
