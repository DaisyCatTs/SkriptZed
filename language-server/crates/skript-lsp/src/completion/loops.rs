//! `loop-player` and friends: the values the enclosing loops provide.
//!
//! Skript resolves `loop-<what>[-<n>]` against the loops around the line,
//! outermost first (`ExprLoopValue#init`): a loop matches when `<what>` is
//! `value`, names a type its expression returns, or is something the
//! expression answers to itself (`index` for a list variable, `zombie` for
//! `all zombies`). One match is used as is; several are an error unless
//! `-<n>` picks the n-th of them. `loop-iteration[-<n>]` counts every loop,
//! `while` included, without filtering (`ExprLoopIteration#init`).
//!
//! Completion offers exactly the names that resolve, numbered only where
//! Skript would demand it, and hover says which loop a name refers to.

use skript_docs::{Catalog, Category, LineRole};
use skript_index::Document;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, TextEdit,
};

use super::arguments::{key, supertypes};
use super::ranking::{relevance, sort_key};
use crate::convert::{to_lsp_range, Encoding};
use crate::hover::markdown;
use crate::lines;

/// Above every argument and event value once `loop-` is spelt out: nothing
/// else can be meant by a word that starts that way.
const SPELT_OUT_BONUS: i32 = 8_000;
/// For `lo`, `loo`, `loop` past a statement's first word: level with the
/// event values argument completion offers, which is where a looped value
/// competes.
const PREFIX_BONUS: i32 = 5_000;
/// The shortest typed prefix of `loop-` that triggers these suggestions.
const MIN_TYPED: usize = 2;

/// One loop around the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loop {
    pub line: u32,
    /// The header as written, without indentation, comment or colon.
    pub header: String,
    /// The looped expression; empty for a `while` loop.
    pub expression: String,
    /// A `while` loop: counted by `loop-iteration`, matched by nothing else.
    pub is_while: bool,
    /// What `loop-<what>` names this loop by beyond `value`, each with a
    /// display name: `player` / `Player`. Empty when unknown.
    pub types: Vec<(String, String)>,
    /// A list variable, which also provides `loop-index`.
    pub keyed: bool,
}

impl Loop {
    /// Whether `loop-<what>` can mean this loop, as `ExprLoopValue` decides.
    fn matches(&self, what: &str) -> bool {
        if self.is_while {
            return false;
        }
        let what = what.to_ascii_lowercase();
        if what == "value" {
            return true;
        }
        if self.keyed && matches!(what.as_str(), "index" | "var" | "variable") {
            return true;
        }
        let wanted = key(&what);
        self.types.iter().any(|(word, _)| {
            *word == wanted || supertypes(word).iter().any(|parent| *parent == wanted)
        })
    }

    /// Whether this loop's types are known, so that "nothing matches" is an
    /// answer rather than a gap in what the catalog could classify.
    fn fully_known(&self) -> bool {
        self.is_while || self.keyed || !self.types.is_empty()
    }
}

/// The loops enclosing `line`, outermost first, as Skript numbers them.
///
/// Found by indentation rather than the parse tree: the line being typed is
/// often unfinished, and an unclosed string or brace can cut a section's node
/// short.
pub fn enclosing_loops(catalog: Option<&Catalog>, document: &Document, line: u32) -> Vec<Loop> {
    let mut indent = skript_index::indent_width(document.line(line));
    let mut found = Vec::new();
    for at in (0..line).rev() {
        if indent == 0 {
            break;
        }
        let raw = document.line(at);
        let code = lines::code_part(raw).trim();
        if code.is_empty() {
            continue;
        }
        let width = skript_index::indent_width(raw);
        if width >= indent {
            continue;
        }
        indent = width;
        if let Some(found_loop) = read_header(catalog, at, code) {
            found.push(found_loop);
        }
    }
    found.reverse();
    found
}

/// The loop a section header opens, if it opens one.
fn read_header(catalog: Option<&Catalog>, line: u32, code: &str) -> Option<Loop> {
    let header = code.strip_suffix(':')?.trim_end();
    let lower = header.to_ascii_lowercase();
    let make = |expression: &str, is_while: bool| {
        let (types, keyed) = if is_while {
            (Vec::new(), false)
        } else {
            looped_types(catalog, expression)
        };
        Loop {
            line,
            header: header.to_string(),
            expression: expression.to_string(),
            is_while,
            types,
            keyed,
        }
    };
    if lower.starts_with("while ") || lower.starts_with("do while ") {
        return Some(make("", true));
    }
    let rest = lower
        .strip_prefix("loop ")
        .or_else(|| lower.strip_prefix("for "))?;
    let offset = header.len() - rest.len();
    let rest = &header[offset..];
    // `for each {_p} in players` / `loop {_i}, {_v} in {list::*}`: the
    // looped expression follows the variables. `for` is only ever this form.
    if let Some(expression) = for_each_expression(rest) {
        return Some(make(expression, false));
    }
    lower.starts_with("loop ").then(|| make(rest.trim(), false))
}

/// The expression after `in` in a `for each` header, when what precedes it is
/// only variables and the words `SecFor` allows there.
fn for_each_expression(rest: &str) -> Option<&str> {
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix("each ")
        .or_else(|| rest.strip_prefix("Each "))
        .unwrap_or(rest);
    let mut braces = 0u32;
    for (at, ch) in rest.char_indices() {
        match ch {
            '{' => braces += 1,
            '}' => braces = braces.saturating_sub(1),
            ' ' if braces == 0 && rest[at..].to_ascii_lowercase().starts_with(" in ") => {
                let before = &rest[..at];
                if before.contains('{') && only_loop_variables(before) {
                    return Some(rest[at + 4..].trim());
                }
            }
            _ => {}
        }
    }
    None
}

/// `{_i}, {_v}`, `key {_k} and value {_v}`: variables and connecting words.
fn only_loop_variables(text: &str) -> bool {
    let mut braces = 0u32;
    let mut outside = String::new();
    for ch in text.chars() {
        match ch {
            '{' => braces += 1,
            '}' => {
                braces = braces.saturating_sub(1);
                outside.push(' ');
            }
            _ if braces > 0 => {}
            ',' => outside.push(' '),
            _ => outside.push(ch),
        }
    }
    outside.split_whitespace().all(|word| {
        matches!(
            word.to_ascii_lowercase().as_str(),
            "key" | "index" | "value" | "and"
        )
    })
}

/// What a looped expression's values are named by, and whether it is a list
/// variable with indices.
fn looped_types(catalog: Option<&Catalog>, expression: &str) -> (Vec<(String, String)>, bool) {
    let trimmed = expression.trim();
    let lower = trimmed.to_ascii_lowercase();
    // `%number% times` is undocumented (`ExprTimes`) and returns a whole
    // number, which `loop-number` accepts.
    if lower.ends_with(" times") {
        return (vec![("number".into(), "Number".into())], false);
    }
    if is_single_variable(trimmed) {
        return (Vec::new(), trimmed.ends_with("::*}"));
    }
    let Some(catalog) = catalog else {
        return (Vec::new(), false);
    };
    let Some((id, matched)) = catalog.classify_line(trimmed, LineRole::Expression) else {
        return (Vec::new(), false);
    };
    if !matches!(id.category, Category::Expression | Category::Property)
        || !matched.explains_text(trimmed)
    {
        return (Vec::new(), false);
    }
    let Some(entry) = catalog.entry(id) else {
        return (Vec::new(), false);
    };
    let mut types = Vec::new();
    // `all players` and `all zombies` are one expression returning entities;
    // Skript also names the loop by the entity type written
    // (`ExprEntities#isLoopOf`), and for players that is the type it returns.
    for capture in &matched.captures {
        let entity_types = capture
            .slot
            .types
            .iter()
            .any(|ty| key(ty).starts_with("entitytype"));
        if capture.slot.literal_only && entity_types && !capture.text.contains([',', ' ']) {
            let word = singular(&capture.text.to_ascii_lowercase());
            let display = type_display(catalog, &word);
            types.push((word, display));
        }
    }
    if let Some(returns) = &entry.returns {
        let display = returns.display();
        let word = key(display);
        if !word.is_empty() && word != "object" && !types.iter().any(|(known, _)| *known == word) {
            types.push((word, display.to_string()));
        }
    }
    (types, false)
}

/// `{_x}` or `{list::*}` and nothing else.
fn is_single_variable(text: &str) -> bool {
    if !text.starts_with('{') || !text.ends_with('}') {
        return false;
    }
    let mut depth = 0i32;
    for (at, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 && at + 1 != text.len() {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn singular(word: &str) -> String {
    word.strip_suffix("ies")
        .map(|stem| format!("{stem}y"))
        .or_else(|| word.strip_suffix('s').map(str::to_string))
        .unwrap_or_else(|| word.to_string())
}

/// The documented name of the type `word` keys to, or `word` itself.
fn type_display(catalog: &Catalog, word: &str) -> String {
    catalog
        .docs()
        .entries(Category::Type)
        .iter()
        .find(|entry| key(&entry.name) == word || entry.id.eq_ignore_ascii_case(word))
        .map_or_else(|| word.to_string(), |entry| entry.name.clone())
}

/// One name a loop can be referred to by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopName {
    /// `loop-player`, `loop-value-2`.
    pub label: String,
    /// Index into the loops it was built from.
    pub loop_index: usize,
    /// What the name yields: `Player`, `Text` for an index.
    pub type_name: String,
}

/// Every name the `loops` (outermost first) can be referred to by, numbered
/// only where a bare name would be ambiguous. Innermost loop first.
pub fn loop_names(loops: &[Loop]) -> Vec<LoopName> {
    let mut names = Vec::new();
    let numbered = |what: &str, index: usize| -> String {
        let matching: Vec<usize> = (0..loops.len())
            .filter(|&other| loops[other].matches(what))
            .collect();
        match matching.iter().position(|&other| other == index) {
            Some(_) if matching.len() == 1 => format!("loop-{what}"),
            Some(rank) => format!("loop-{what}-{}", rank + 1),
            None => format!("loop-{what}"),
        }
    };
    for (index, found) in loops.iter().enumerate().rev() {
        if !found.is_while {
            let value_type = found
                .types
                .first()
                .map_or_else(|| "object".to_string(), |(_, display)| display.clone());
            for (word, display) in &found.types {
                names.push(LoopName {
                    label: numbered(word, index),
                    loop_index: index,
                    type_name: display.clone(),
                });
            }
            names.push(LoopName {
                label: numbered("value", index),
                loop_index: index,
                type_name: value_type,
            });
            if found.keyed {
                names.push(LoopName {
                    label: numbered("index", index),
                    loop_index: index,
                    type_name: "Text".into(),
                });
            }
        }
        names.push(LoopName {
            label: if loops.len() == 1 {
                "loop-iteration".into()
            } else {
                format!("loop-iteration-{}", index + 1)
            },
            loop_index: index,
            type_name: "Number".into(),
        });
    }
    names
}

/// Whether the lowercased `word` is on its way to `loop-…`.
fn heading_for_loop_value(word: &str) -> bool {
    word.starts_with("loop-") || (word.len() >= MIN_TYPED && "loop-".starts_with(word))
}

/// Loop values for the word being typed, each replacing only that word.
///
/// `prefix` is the line up to `cursor`, which is in index (byte) columns.
pub fn loop_value_items(
    catalog: Option<&Catalog>,
    document: &Document,
    cursor: skript_index::Position,
    prefix: &str,
    encoding: Encoding,
) -> Vec<(i32, CompletionItem)> {
    let word_start = prefix
        .char_indices()
        .rfind(|(_, ch)| ch.is_whitespace() || matches!(ch, '%' | '(' | ',' | '"' | '{' | '}'))
        .map_or(0, |(at, ch)| at + ch.len_utf8());
    let word = prefix[word_start..].to_ascii_lowercase();
    if !heading_for_loop_value(&word) {
        return Vec::new();
    }
    let loops = enclosing_loops(catalog, document, cursor.line);
    if loops.is_empty() {
        return Vec::new();
    }
    let names = loop_names(&loops);
    // A finished name: the popup would only wait for Enter to accept the
    // same text, eating the newline (see `arguments::argument_items`).
    if names.iter().any(|name| name.label == word) {
        return Vec::new();
    }
    let first_word = prefix[..word_start].trim().is_empty();
    let bonus = if word.starts_with("loop-") {
        SPELT_OUT_BONUS
    } else if !first_word {
        PREFIX_BONUS
    } else {
        0
    };
    let range = to_lsp_range(
        document.text(),
        skript_index::Range::new(
            skript_index::Position::new(cursor.line, word_start as u32),
            cursor,
        ),
        encoding,
    );
    names
        .into_iter()
        .enumerate()
        .filter(|(_, name)| name.label.starts_with(&word))
        .map(|(position, name)| {
            let found = &loops[name.loop_index];
            // Among equally good names, the order `loop_names` gives: the
            // innermost loop's first, the type written (`player`) before the
            // type returned (`entity`), then `loop-value`, `loop-iteration`.
            let score = relevance(&name.label, &word).max(1) + bonus - position as i32;
            let item = CompletionItem {
                label: name.label.clone(),
                kind: Some(CompletionItemKind::VALUE),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(format!("{} · line {}", name.type_name, found.line + 1)),
                }),
                detail: Some(format!("loop value · {}", name.type_name)),
                documentation: Some(Documentation::MarkupContent(markdown(describe(
                    &name.label,
                    &name.type_name,
                    found,
                )))),
                filter_text: Some(name.label.clone()),
                sort_text: Some(sort_key(score, &name.label)),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: name.label,
                })),
                ..Default::default()
            };
            (score, item)
        })
        .collect()
}

fn describe(label: &str, type_name: &str, found: &Loop) -> String {
    let what = if label.starts_with("loop-iteration") {
        "How many times this loop has run, counting from 1".to_string()
    } else if label.starts_with("loop-index") {
        "The index of the current element of this loop".to_string()
    } else {
        format!("The current {} of this loop", type_name.to_lowercase())
    };
    format!(
        "**{label}**: {type_name}  \n{what}, on line {}:\n\n```skript\n{}:\n```",
        found.line + 1,
        found.header
    )
}

/// The hover card for a `loop-…` name under the byte `column` of `line`, and
/// the byte span of the name.
pub fn loop_hover(
    catalog: Option<&Catalog>,
    document: &Document,
    line: u32,
    column: usize,
) -> Option<(String, skript_index::Range)> {
    let raw = document.line(line);
    let is_name = |ch: char| ch.is_alphanumeric() || ch == '-' || ch == '_';
    let column = column.min(raw.len());
    let start = raw[..column]
        .char_indices()
        .rev()
        .take_while(|&(_, ch)| is_name(ch))
        .last()
        .map_or(column, |(at, _)| at);
    let end = raw[column..]
        .char_indices()
        .find(|&(_, ch)| !is_name(ch))
        .map_or(raw.len(), |(at, _)| column + at);
    let name = &raw[start..end];
    let what = name
        .get(..5)
        .filter(|head| head.eq_ignore_ascii_case("loop-"))
        .map(|_| &name[5..])?;
    if what.is_empty() {
        return None;
    }
    let span = skript_index::Range::on_line(line, start as u32, end as u32);
    let loops = enclosing_loops(catalog, document, line);

    // `loop-player-2`: the second loop that matches `player`.
    let (what, number) = match what.rsplit_once('-') {
        Some((head, digits)) if !head.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => {
            (head, digits.parse::<usize>().ok())
        }
        _ => (what, None),
    };
    let lower = what.to_ascii_lowercase();
    if lower == "counter" || lower == "iteration" {
        let chosen = match (number, loops.len()) {
            (_, 0) => return Some((no_loop(name), span)),
            (Some(n), _) => loops.get(n.checked_sub(1)?),
            (None, 1) => loops.first(),
            (None, _) => return Some((ambiguous(name, &lower, &loops), span)),
        }?;
        return Some((describe(name, "Number", chosen), span));
    }

    let matching: Vec<&Loop> = loops.iter().filter(|found| found.matches(what)).collect();
    let chosen = match number {
        Some(n) => matching.get(n.checked_sub(1)?).copied(),
        None if matching.len() > 1 => {
            let owned: Vec<Loop> = matching.into_iter().cloned().collect();
            return Some((ambiguous(name, &lower, &owned), span));
        }
        None => matching.first().copied(),
    };
    let Some(chosen) = chosen else {
        // Only claimed when every loop's types are known; otherwise the
        // catalog simply could not say.
        return loops
            .iter()
            .all(Loop::fully_known)
            .then(|| (no_loop(name), span));
    };
    let type_name = if lower == "index" {
        "Text".to_string()
    } else if lower == "value" || lower == "var" || lower == "variable" {
        chosen
            .types
            .first()
            .map_or_else(|| "Object".to_string(), |(_, display)| display.clone())
    } else {
        chosen
            .types
            .iter()
            .find(|(word, _)| *word == key(what))
            .map_or_else(|| what.to_string(), |(_, display)| display.clone())
    };
    Some((describe(name, &type_name, chosen), span))
}

fn no_loop(name: &str) -> String {
    format!("**{name}**  \nNo loop around this line provides it, so Skript will refuse the line.")
}

fn ambiguous(name: &str, what: &str, loops: &[Loop]) -> String {
    let lines: Vec<String> = loops
        .iter()
        .map(|found| format!("line {}", found.line + 1))
        .collect();
    format!(
        "**{name}**  \nMatches more than one loop ({}), which Skript rejects. Write \
         `loop-{what}-1` for the outermost, `loop-{what}-2` for the next.",
        lines.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_docs::{Docs, Entry, Reference};
    use skript_index::Workspace;

    /// The two expressions the tests loop over, shaped as `docs.json` has
    /// them: entities by entity type, and worlds.
    fn catalog() -> Catalog {
        let mut docs = Docs::default();
        docs.push(
            Category::Expression,
            Entry {
                id: "ExprEntities".into(),
                name: "Entities".into(),
                patterns: vec!["[(all [[of] the]|the)] %*entity types%".into()],
                returns: Some(Reference {
                    id: "entity".into(),
                    name: "Entity".into(),
                }),
                ..Default::default()
            },
        );
        docs.push(
            Category::Expression,
            Entry {
                id: "ExprWorlds".into(),
                name: "Worlds".into(),
                patterns: vec!["[(all [[of] the]|the)] worlds".into()],
                returns: Some(Reference {
                    id: "world".into(),
                    name: "World".into(),
                }),
                ..Default::default()
            },
        );
        docs.push(
            Category::Type,
            Entry {
                id: "player".into(),
                name: "Player".into(),
                ..Default::default()
            },
        );
        Catalog::build(docs)
    }

    fn labels(source: &str, line: u32, prefix: &str) -> Vec<String> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let cursor = skript_index::Position::new(line, prefix.len() as u32);
        let mut items =
            loop_value_items(Some(&catalog()), document, cursor, prefix, Encoding::Utf16);
        items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.label.cmp(&b.1.label)));
        items.into_iter().map(|(_, item)| item.label).collect()
    }

    #[test]
    fn a_loop_over_players_offers_its_type_and_value() {
        let source = "on join:\n\tloop all players:\n\t\tsend \"hi\" to loop-\n";
        let found = labels(source, 2, "\t\tsend \"hi\" to loop-");
        assert!(found.contains(&"loop-player".to_string()), "{found:?}");
        assert!(found.contains(&"loop-entity".to_string()), "{found:?}");
        assert!(found.contains(&"loop-value".to_string()), "{found:?}");
        assert!(found.contains(&"loop-iteration".to_string()), "{found:?}");
        assert_eq!(found[0], "loop-player", "the type written leads");
    }

    #[test]
    fn a_list_variable_offers_value_and_index_and_times_offers_number() {
        let list = labels(
            "on join:\n\tloop {_list::*}:\n\t\tbroadcast loop-\n",
            2,
            "\t\tbroadcast loop-",
        );
        assert!(list.contains(&"loop-value".to_string()), "{list:?}");
        assert!(list.contains(&"loop-index".to_string()), "{list:?}");

        let times = labels(
            "on join:\n\tloop 10 times:\n\t\tbroadcast \"%loop-n\n",
            2,
            "\t\tbroadcast \"%loop-n",
        );
        assert_eq!(times, vec!["loop-number"]);
    }

    #[test]
    fn nested_loops_are_numbered_only_where_a_name_is_ambiguous() {
        let source = "on join:\n\tloop all players:\n\t\tloop all worlds:\n\t\t\tsend loop-\n";
        let found = labels(source, 3, "\t\t\tsend loop-");
        // Both loops have a value: Skript demands the number.
        assert!(found.contains(&"loop-value-1".to_string()), "{found:?}");
        assert!(found.contains(&"loop-value-2".to_string()), "{found:?}");
        assert!(!found.contains(&"loop-value".to_string()), "{found:?}");
        // Only one loop has players and only one has worlds.
        assert!(found.contains(&"loop-player".to_string()), "{found:?}");
        assert!(found.contains(&"loop-world".to_string()), "{found:?}");
        assert!(found.contains(&"loop-iteration-2".to_string()), "{found:?}");
        // The inner loop's names lead.
        assert_eq!(found[0], "loop-world");

        let same = "on join:\n\tloop all players:\n\t\tloop all players:\n\t\t\tsend loop-p\n";
        let found = labels(same, 3, "\t\t\tsend loop-p");
        assert_eq!(found, vec!["loop-player-2", "loop-player-1"]);
    }

    #[test]
    fn nothing_outside_a_loop_or_for_a_finished_name() {
        assert!(labels("on join:\n\tsend loop-\n", 1, "\tsend loop-").is_empty());
        let source = "on join:\n\tloop all players:\n\t\tkill loop-player\n";
        assert!(labels(source, 2, "\t\tkill loop-player").is_empty());
        // The loop ended at the dedent.
        let after = "on join:\n\tloop all players:\n\t\tstop\n\tsend loop-\n";
        assert!(labels(after, 3, "\tsend loop-").is_empty());
    }

    #[test]
    fn a_short_prefix_offers_them_but_only_lifts_them_past_the_first_word() {
        let source = "on join:\n\tloop all players:\n\t\tkill lo\n";
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let score = |prefix: &str| {
            let cursor = skript_index::Position::new(2, prefix.len() as u32);
            loop_value_items(Some(&catalog()), document, cursor, prefix, Encoding::Utf16)
                .into_iter()
                .map(|(score, _)| score)
                .max()
                .unwrap_or(0)
        };
        assert!(score("\t\tkill lo") >= PREFIX_BONUS);
        let first = score("\t\tlo");
        assert!(first > 0 && first < PREFIX_BONUS, "{first}");
        assert_eq!(score("\t\tkill l"), 0, "one letter is not enough");
    }

    #[test]
    fn the_edit_replaces_only_the_word() {
        let source = "on join:\n\tloop all players:\n\t\tsend \"%loop-\n";
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let prefix = "\t\tsend \"%loop-";
        let cursor = skript_index::Position::new(2, prefix.len() as u32);
        let items = loop_value_items(Some(&catalog()), document, cursor, prefix, Encoding::Utf16);
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].1.text_edit else {
            panic!("no edit");
        };
        assert_eq!(edit.range.start.character, 9, "after the `%`");
    }

    #[test]
    fn for_each_headers_loop_over_what_follows_in() {
        assert_eq!(
            for_each_expression("each {_p} in all players"),
            Some("all players")
        );
        assert_eq!(
            for_each_expression("key {_k} and value {_v} in {list::*}"),
            Some("{list::*}")
        );
        assert_eq!(for_each_expression("all players in world \"x\""), None);
        let found = read_header(None, 0, "for each {_i} in {list::*}:").unwrap();
        assert!(found.keyed);
        assert!(read_header(None, 0, "loop {_x} times:").is_some());
        assert!(read_header(None, 0, "for {_x}:").is_none());
        assert!(read_header(None, 0, "while true:").unwrap().is_while);
    }

    fn hover(source: &str, line: u32, column: usize) -> Option<String> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        loop_hover(Some(&catalog()), document, line, column).map(|(text, _)| text)
    }

    #[test]
    fn hover_names_the_loop_and_its_type() {
        let source =
            "on join:\n\tloop all players:\n\t\tloop all worlds:\n\t\t\tsend loop-player\n";
        let text = hover(source, 3, 12).unwrap();
        assert!(text.contains("Player"), "{text}");
        assert!(text.contains("line 2"), "{text}");

        let text = hover(
            "on join:\n\tloop all players:\n\t\tloop all worlds:\n\t\t\tsend loop-value-2\n",
            3,
            12,
        )
        .unwrap();
        assert!(text.contains("World") && text.contains("line 3"), "{text}");

        let text = hover(
            "on join:\n\tloop all players:\n\t\tloop all worlds:\n\t\t\tsend loop-value\n",
            3,
            12,
        )
        .unwrap();
        assert!(text.contains("more than one loop"), "{text}");

        let text = hover("on join:\n\tloop {_l::*}:\n\t\tsend loop-index\n", 2, 10).unwrap();
        assert!(text.contains("Text") && text.contains("index"), "{text}");

        assert!(hover("on join:\n\tsend loop-value\n", 1, 9)
            .unwrap()
            .contains("No loop"));
        assert!(hover("on join:\n\tsend player\n", 1, 8).is_none());
    }

    /// Against the real catalog, `all players` is named by `player`, and a
    /// documented expression by its return type. Skips without the file.
    #[test]
    fn real_loop_expressions_are_named_by_their_types() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../vendor/docs.json");
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("vendor/docs.json not present, skipping");
            return;
        };
        let catalog = Catalog::build(Docs::parse(&text).expect("docs.json parses"));
        let words = |expression: &str| -> Vec<String> {
            looped_types(Some(&catalog), expression)
                .0
                .into_iter()
                .map(|(word, _)| word)
                .collect()
        };
        assert!(words("all players").contains(&"player".to_string()));
        assert!(words("all worlds").contains(&"world".to_string()));
        assert_eq!(words("10 times"), vec!["number"]);
    }
}
