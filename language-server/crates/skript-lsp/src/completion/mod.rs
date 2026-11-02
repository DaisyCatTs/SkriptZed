//! `textDocument/completion` and `completionItem/resolve`.

mod events;
mod ranking;
mod sections;
mod snippet;

pub use sections::entry_hover as section_entry_hover;
pub use sections::is_documented_entry;

use std::collections::HashSet;

use skript_docs::{Catalog, Category};
use skript_index::{Document, SymbolKind, Workspace};
use tower_lsp::lsp_types::{
    Command, CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionItemTag,
    CompletionList, CompletionParams, CompletionResponse, CompletionTextEdit, Documentation,
    InsertTextFormat, Position, Range, TextEdit,
};

use crate::convert::{self, from_lsp_position, to_lsp_range, Encoding};
use crate::hover::markdown;
use crate::server::State;
use crate::{entries, variables};
use ranking::{filter_text_for, relevance, relevance_to_keywords, sort_key};
use snippet::snippet_for_typed;

/// Most items returned when the user has typed something to narrow by.
///
/// Small, because this is the per-keystroke path and relevance has already put
/// the useful items first.
const COMPLETION_LIMIT: usize = 200;

/// Most items returned when nothing has been typed.
///
/// Deliberately far larger. An empty query is somebody pressing Ctrl-Space to
/// *browse*, which happens once rather than per keystroke, and every item ties
/// on relevance, so a small cap would truncate alphabetically and silently hide
/// whole addons behind the letter it happened to stop at.
const COMPLETION_BROWSE_LIMIT: usize = 2_000;

/// Added to a matching project symbol's score. The user's own declarations
/// outrank anything from the catalog: they are far fewer, and far more likely
/// to be what was meant.
const DECLARED_BONUS: i32 = 400;

/// Added when the typed text is a catalog entry's exact name.
const EXACT_NAME_BONUS: i32 = 250;

/// Taken off a catalog entry per step down its rank: core Skript, then
/// addons, then syntax the target version cannot run.
const RANK_PENALTY: i32 = 60;

/// Characters Zed counts as part of the word being completed: word
/// characters plus `completion_query_characters` in the extension's
/// `config.toml`. Keep the two in step.
const QUERY_CHARACTERS: &[char] = &['-', '_', '{', '}', '@', '%', ':', '.'];

pub fn completion(state: &State, params: &CompletionParams) -> Option<CompletionResponse> {
    let mut response = complete(state, params)?;
    let at = &params.text_document_position;
    if let Some(document) = state.workspace.get(at.text_document.uri.as_str()) {
        let position = from_lsp_position(document.text(), at.position, state.encoding);
        let prefix = convert::line_prefix(document.line(position.line), position.character);
        let items = match &mut response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => &mut list.items,
        };
        for item in items.iter_mut() {
            align_with_editor_query(item, document.text(), position.line, prefix, state.encoding);
        }
    }
    Some(response)
}

/// Where the editor's filter query starts in `prefix`.
fn editor_query_start(prefix: &str) -> usize {
    prefix
        .char_indices()
        .rev()
        .take_while(|&(_, c)| c.is_alphanumeric() || QUERY_CHARACTERS.contains(&c))
        .last()
        .map_or(prefix.len(), |(index, _)| index)
}

/// Makes `item`'s filter text start with everything the editor will filter
/// it by.
///
/// Zed filters by the whole "word" before the cursor, and `{`, `%` and `:`
/// count as word characters there. After `{_` it filtered `_count` by
/// `{_`, found no `{` in it, and hid every variable: completion worked on
/// the wire and showed nothing on screen. Text between the start of that word
/// and the start of the item's edit is therefore put in front of its filter
/// text.
fn align_with_editor_query(
    item: &mut CompletionItem,
    text: &str,
    line: u32,
    prefix: &str,
    encoding: Encoding,
) {
    let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        return;
    };
    if edit.range.start.line != line {
        return;
    }
    let edit_start = from_lsp_position(text, edit.range.start, encoding).character as usize;
    let query_start = editor_query_start(prefix);
    if query_start >= edit_start || edit_start > prefix.len() {
        return;
    }
    let lead = &prefix[query_start..edit_start];
    let base = item.filter_text.as_deref().unwrap_or(&item.label);
    if !base.starts_with(lead) {
        item.filter_text = Some(format!("{lead}{base}"));
    }
}

fn complete(state: &State, params: &CompletionParams) -> Option<CompletionResponse> {
    let at = &params.text_document_position;
    let uri = at.text_document.uri.as_str();
    let document = state.workspace.get(uri)?;
    let position = from_lsp_position(document.text(), at.position, state.encoding);

    let line = document.line(position.line);
    let prefix = convert::line_prefix(line, position.character);

    // Inside a variable's braces only variable names make sense, and the
    // catalog's patterns would bury them.
    if let Some(open) = variables::open_brace(prefix) {
        if !state.settings.features.variable_completion {
            return None;
        }
        let items = variable_completions(
            &state.workspace,
            document,
            position,
            open,
            line,
            state.encoding,
        );
        return (!items.is_empty()).then_some(CompletionResponse::Array(items));
    }
    // `:` is a trigger only so that typing `{data::` opens the next level.
    // Anywhere else (the end of every section header) it must stay silent.
    let triggered_by = params
        .context
        .as_ref()
        .and_then(|context| context.trigger_character.as_deref());
    if triggered_by == Some(":") {
        return None;
    }

    let typed = &prefix[fragment_start(prefix).min(prefix.len())..];

    // Inside a command, the useful suggestions are its entries, not the
    // effects that cannot legally appear there. Offering those was the single
    // most misleading thing completion did.
    if let Some(scope) = entries::entry_scope(document, position.line) {
        if !prefix.contains(':') {
            let items = sorted(command_entry_items(typed, scope));
            return Some(CompletionResponse::Array(items));
        }
    }

    // The same holds inside an addon section that takes entries: SkBee's
    // `register shaped recipe:` holds `result:` and `shape:`, not effects.
    if let Some(catalog) = state.catalog.as_ref().filter(|_| !prefix.contains(':')) {
        let indent = skript_index::indent_width(line);
        if let Some((_, section, header)) =
            sections::enclosing_section(catalog, document, position.line, indent)
        {
            let items = sections::entry_items(document, section, header, position.line, typed);
            return Some(CompletionResponse::Array(sorted(items)));
        }
    }

    // What an accepted completion should replace.
    //
    // Skript syntax is multi-word, and the client's idea of "the word being
    // typed" stops at a space. So typing `send m` and accepting Message
    // replaced only the `m`, leaving `send message %objects%` with the keyword
    // duplicated. The server knows where the fragment really starts, so it
    // says so rather than letting the client guess.
    let replace = Range::new(
        Position::new(position.line, fragment_start(prefix) as u32),
        at.position,
    );
    let mut items = declared_items(&state.workspace, uri, typed);
    if let Some(catalog) = &state.catalog {
        items.extend(catalog_items(catalog, prefix, typed, replace));
        items.extend(events::event_value_items(
            catalog,
            document,
            position,
            prefix,
            state.encoding,
        ));
    }

    if state.settings.features.variable_completion {
        items.extend(bare_local_items(document, position, prefix, state.encoding));
    }

    // Anything that does not match at all is dropped once the user has typed
    // something: a list containing every effect is not a list.
    let narrowing = !typed.trim().is_empty();
    if narrowing {
        items.retain(|(score, _)| *score > 0);
    }

    // Capped, and the client is told the list is partial so it asks again as
    // the query narrows. Without a cap this is ~190 KiB per keystroke, and
    // several times that once a plugins directory merges addon syntax.
    let limit = if narrowing {
        COMPLETION_LIMIT
    } else {
        COMPLETION_BROWSE_LIMIT
    };
    let mut items = sorted(items);
    let truncated = items.len() > limit;
    items.truncate(limit);

    Some(CompletionResponse::List(CompletionList {
        is_incomplete: truncated,
        items,
    }))
}

/// Best score first, then by label, scores dropped.
fn sorted(mut items: Vec<(i32, CompletionItem)>) -> Vec<CompletionItem> {
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    items.into_iter().map(|(_, item)| item).collect()
}

/// The entries a command's body accepts.
fn command_entry_items(typed: &str, scope: entries::EntryScope) -> Vec<(i32, CompletionItem)> {
    entries::COMMAND
        .iter()
        .filter(|entry| entries::allowed_in(entry, scope))
        .map(|entry| {
            let score = relevance(entry.key, typed);
            let item = CompletionItem {
                label: entries::label(entry),
                kind: Some(CompletionItemKind::PROPERTY),
                detail: Some("command entry".into()),
                documentation: Some(Documentation::MarkupContent(markdown(entries::hover(
                    entry,
                )))),
                insert_text: Some(entries::snippet(entry)),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                sort_text: Some(sort_key(score, entry.key)),
                ..Default::default()
            };
            (score, item)
        })
        .collect()
}

/// The functions, commands, options and variables this file can see.
///
/// Scoped to what Skript would resolve from this file. Using the project-wide
/// symbol list here offered another script's `{_local}` variables and options
/// in every trigger: names that cannot resolve, so accepting one produced code
/// that silently did nothing.
fn declared_items(workspace: &Workspace, uri: &str, typed: &str) -> Vec<(i32, CompletionItem)> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for (_, symbol) in workspace.symbols_in_scope(uri) {
        let kind = match symbol.kind {
            SymbolKind::Function | SymbolKind::LocalFunction => CompletionItemKind::FUNCTION,
            SymbolKind::Command => CompletionItemKind::METHOD,
            SymbolKind::Option => CompletionItemKind::CONSTANT,
            SymbolKind::GlobalVariable | SymbolKind::LocalVariable => CompletionItemKind::VARIABLE,
            _ => continue,
        };
        // The same global is declared in as many files as assign to it.
        if !seen.insert((symbol.kind, symbol.name.clone())) {
            continue;
        }
        // Boosted only when it matches. Boosting every symbol kept all of them
        // above zero, so none was ever filtered out: a project with 300
        // functions filled the whole capped list for `eleport`, and Teleport
        // never appeared.
        let base = relevance(&symbol.name, typed);
        let score = if base > 0 { base + DECLARED_BONUS } else { 0 };
        let has_detail = !symbol.detail.is_empty();
        items.push((
            score,
            CompletionItem {
                label: symbol.name.clone(),
                kind: Some(kind),
                label_details: Some(CompletionItemLabelDetails {
                    detail: has_detail.then(|| format!(" {}", symbol.detail)),
                    description: Some("this project".into()),
                }),
                detail: has_detail.then(|| symbol.detail.clone()),
                sort_text: Some(sort_key(score, &symbol.name)),
                ..Default::default()
            },
        ));
    }
    items
}

/// The catalog syntax that can appear at this point in the line.
fn catalog_items(
    catalog: &Catalog,
    prefix: &str,
    typed: &str,
    replace: Range,
) -> Vec<(i32, CompletionItem)> {
    let mut items = Vec::new();
    // Context narrows the categories that can possibly be valid here, which is
    // the difference between a useful list and every entry in the catalog.
    for category in categories_for(prefix) {
        for (id, entry) in catalog.search(category, "") {
            let Some(pattern) = entry.patterns.first() else {
                continue;
            };
            // Split across the two slots a client renders separately: the
            // pattern sits next to the name, where it reads as a signature,
            // and provenance is right-aligned out of the way. One crammed
            // string made a long list unscannable.
            let signature = format!(" {pattern}");

            // Where it comes from matters more than the category when several
            // addons are loaded, so it leads.
            let mut provenance = match &entry.addon {
                Some(addon) => format!("{} · {}", addon.name, category.label()),
                None => category.label().to_string(),
            };

            // Syntax the target Skript cannot run is labelled and sunk, never
            // hidden: a quarter of `since` values are free text, and hiding
            // working syntax on a misparse is the worse failure.
            let availability = catalog.availability(id);
            if let Some(note) = &availability {
                provenance = format!("{note} · {provenance}");
            }

            // `detail` is still filled, because a client that ignores
            // `labelDetails` would otherwise show nothing at all.
            let detail = format!("{provenance} ·{signature}");

            // The project's own skript-reflect syntax ranks as its functions
            // do: above the catalog, because it was written for this project.
            let own = catalog.is_workspace(id);
            let rank = match (&entry.addon, availability.is_some()) {
                _ if own => 0,
                (_, true) => 3,
                (None, false) => 0,
                (Some(_), false) => 1,
            };

            let filter = filter_text_for(&entry.name, pattern);
            // Scored against the name and the pattern's *opening* keywords
            // separately, taking the best.
            //
            // Skript writes `(message|send …)`, so `send` is a leading keyword
            // of the Message effect, but in the flattened filter text it lands
            // third, and the positional penalty dropped Message below every
            // entry merely *named* "Send …". Typing the keyword should find the
            // syntax that starts with it.
            //
            // Typing a syntax's exact name should surface that syntax, not one
            // that merely contains the word: `teleport` and `Display Teleport
            // Duration` otherwise tie at a whole-word match and the alphabet
            // decides.
            let named_exactly = entry.name.eq_ignore_ascii_case(typed.trim());
            let matched = relevance(&filter, typed).max(relevance_to_keywords(pattern, typed));
            let score = matched
                + if named_exactly { EXACT_NAME_BONUS } else { 0 }
                + if own && matched > 0 {
                    DECLARED_BONUS
                } else {
                    0
                }
                - rank * RANK_PENALTY;
            items.push((
                score,
                CompletionItem {
                    label: entry.name.clone(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    label_details: Some(CompletionItemLabelDetails {
                        detail: Some(signature),
                        description: Some(provenance),
                    }),
                    detail: Some(detail),
                    // The label is the documentation *title*; the user types
                    // the *pattern*. Zed filters on the label alone, so without
                    // this, typing `send` never surfaces the effect filed under
                    // "Message", and 60 of 139 effects are unreachable by the
                    // very keyword they begin with.
                    filter_text: Some(filter),
                    sort_text: Some(sort_key(score, &entry.name)),
                    // Rendering every entry's Markdown card here meant
                    // thousands of documents rebuilt on each keystroke. The
                    // client asks for the one item it shows via
                    // `completionItem/resolve`.
                    data: Some(serde_json::json!([category.label(), id.index])),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: replace,
                        new_text: snippet_for_typed(pattern, typed),
                    })),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    tags: entry
                        .is_deprecated()
                        .then(|| vec![CompletionItemTag::DEPRECATED]),
                    ..Default::default()
                },
            ));
        }
    }
    items
}

/// Fills in the documentation of the one item the user is looking at.
pub fn completion_resolve(state: &State, mut item: CompletionItem) -> CompletionItem {
    let Some(data) = item.data.take() else {
        return item;
    };
    let Some(catalog) = &state.catalog else {
        return item;
    };
    let Ok((label, index)) = serde_json::from_value::<(String, usize)>(data) else {
        return item;
    };
    let Some(category) = Category::from_label(&label) else {
        return item;
    };

    let id = skript_docs::EntryId { category, index };
    if let Some(entry) = catalog.entry(id) {
        let card = if catalog.is_workspace(id) {
            crate::custom_syntax::card(category, entry)
        } else {
            skript_docs::hover::render(category, entry)
        };
        item.documentation = Some(Documentation::MarkupContent(markdown(card)));
    }
    item
}

/// Whether `line` is a position where a command *entry* belongs.
///
/// True directly inside a `command` body, and false again once inside one of
/// its sections: `trigger:` holds statements, not entries, and offering entry
/// keys there would replace exactly the suggestions the user needs.
///
/// Read from the index rather than by scanning upwards for the `command`
/// keyword, so a command mentioned in a comment or a string is never mistaken
/// for one, and from the symbol tree rather than from indent width, which
/// varies per file.
///
/// Each `subcommand` (Skript 2.17) takes the same entries, so the rule lives
/// in `entries`, beside the table it serves.
pub fn in_command_entry_position(document: &Document, line: u32) -> bool {
    entries::in_entry_position(document, line)
}

/// Byte column where the fragment the user is typing begins.
///
/// For an ordinary statement that is the first non-blank character: Skript
/// syntax is a whole multi-word phrase, so accepting `Message` after `send m`
/// has to replace both words, not just the `m`.
///
/// Inside `%…%`, or after an opening paren or a comma, the fragment starts
/// there instead: an expression completion must not eat the effect around it.
fn fragment_start(prefix: &str) -> usize {
    // The last thing that opens a nested context, if any.
    let boundary = prefix
        .char_indices()
        .rfind(|(_, ch)| matches!(ch, '%' | '(' | ',' | '{'))
        .map(|(at, ch)| at + ch.len_utf8());

    match boundary {
        // Skip whitespace after the delimiter so the replacement does not
        // swallow the space the user typed.
        Some(at) => at + skript_index::indent_width(&prefix[at..]),
        None => skript_index::indent_width(prefix),
    }
}

/// Which categories can appear at this point in the line.
///
/// This is what makes completion usable: after `on `, only events are possible;
/// inside an `if`, only conditions; at the start of a line inside a trigger,
/// only effects and sections.
fn categories_for(prefix: &str) -> Vec<Category> {
    let lower = prefix.trim_start().to_ascii_lowercase();

    if prefix.trim().is_empty() && !prefix.starts_with([' ', '\t']) {
        return vec![Category::Structure, Category::Event];
    }
    if lower.starts_with("on ") || lower == "on" {
        return vec![Category::Event];
    }
    if ["if ", "else if ", "while ", "do while "]
        .iter()
        .any(|keyword| lower.starts_with(keyword))
    {
        return vec![Category::Condition, Category::Expression];
    }
    // Inside a `%…%` slot only an expression can appear.
    if lower.matches('%').count() % 2 == 1 {
        return vec![Category::Expression];
    }
    vec![
        Category::Effect,
        Category::Section,
        Category::Condition,
        Category::Expression,
    ]
}

/// Completion inside `{…}`: the choices for the level being typed.
///
/// A `{_local}` draws only on the trigger it is typed in (Skript scopes it
/// there, so another trigger's `{_x}` would be code that silently does
/// nothing) plus the enclosing function's parameters. A global draws on every
/// script in the project.
fn variable_completions(
    workspace: &Workspace,
    document: &Document,
    position: skript_index::Position,
    open: usize,
    line: &str,
    encoding: Encoding,
) -> Vec<CompletionItem> {
    let cursor = position.character as usize;
    let typed = &line[open + 1..cursor.min(line.len())];
    let closes = line.get(cursor..).is_some_and(|rest| rest.starts_with('}'));
    // The name being typed is itself an occurrence once its brace is closed.
    let not_this_one = |occurrence: &variables::Occurrence, same_file: bool| {
        !(same_file
            && occurrence.line == position.line
            && (occurrence.start as usize) <= open
            && cursor < occurrence.end as usize)
    };

    let locals = || -> Vec<String> {
        let scope = local_scope(document, position.line);
        let (first, last) = scope.as_ref().map_or((0, u32::MAX), |(range, _)| {
            (range.start.line, range.end.line)
        });
        let mut names: Vec<String> = variables::occurrences(document.text())
            .into_iter()
            .filter(|occurrence| (first..=last).contains(&occurrence.line))
            .filter(|occurrence| not_this_one(occurrence, true))
            .map(|occurrence| occurrence.name)
            .collect();
        if let Some((_, parameters)) = scope {
            names.extend(parameters.into_iter().map(|name| format!("_{name}")));
        }
        names
    };

    // After a bare `{` (or `{vi`) the user may be after a local as much as a
    // global: offering only globals there hid `_visitor` from someone typing
    // `{v`. The trigger's locals come first, being the likelier.
    let top_level_global = !variables::is_local(typed) && !typed.contains("::");
    let local_first: Vec<variables::Suggestion> = if top_level_global {
        variables::suggest("_", locals().iter().map(String::as_str), closes).1
    } else {
        Vec::new()
    };

    let mut names: Vec<String> = Vec::new();
    if variables::is_local(typed) {
        names = locals();
    } else {
        for other in workspace.documents() {
            let same_file = other.uri() == document.uri();
            names.extend(
                variables::occurrences(other.text())
                    .into_iter()
                    .filter(|occurrence| not_this_one(occurrence, same_file))
                    .map(|occurrence| occurrence.name),
            );
        }
    }

    let (level_start, globals) =
        variables::suggest(typed, names.iter().map(String::as_str), closes);
    // A local replaces the whole name typed so far, from the brace on.
    let suggestions: Vec<variables::Suggestion> = local_first.into_iter().chain(globals).collect();
    let replace = skript_index::Range::new(
        skript_index::Position::new(position.line, (open + 1 + level_start) as u32),
        position,
    );
    let lsp_range = to_lsp_range(document.text(), replace, encoding);

    suggestions
        .into_iter()
        .enumerate()
        .map(|(rank, suggestion)| {
            let continues = suggestion.insert.ends_with("::");
            CompletionItem {
                label: suggestion.segment.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(suggestion.detail.into()),
                filter_text: Some(suggestion.segment.clone()),
                sort_text: Some(format!("{rank:04}")),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: lsp_range,
                    new_text: suggestion.insert,
                })),
                // Going a level deeper should offer that level straight away.
                command: continues.then(|| Command {
                    title: "Next level".into(),
                    command: "editor.action.triggerSuggest".into(),
                    arguments: None,
                }),
                ..Default::default()
            }
        })
        .collect()
}

/// The trigger a line belongs to (a ClassySK method counts as its own) and
/// the parameters it declares.
/// The trigger's locals, offered whole (`{_visitor}`) while a bare word is
/// typed: `to vis` should reach `{_visitor}` without first typing `{_`.
///
/// Matched by prefix on the name without its `_`, so a short word does not
/// drag in every local that merely contains its letters. Only the word under
/// the cursor is replaced, not the whole phrase the catalog items replace.
fn bare_local_items(
    document: &Document,
    position: skript_index::Position,
    prefix: &str,
    encoding: Encoding,
) -> Vec<(i32, CompletionItem)> {
    let word_start = prefix
        .char_indices()
        .rev()
        .take_while(|&(_, c)| c.is_alphanumeric() || c == '_')
        .last()
        .map_or(prefix.len(), |(index, _)| index);
    let word = prefix[word_start..].trim_start_matches('_').to_lowercase();
    // Nothing typed, or a word glued to something else (`player's`, `%x`):
    // only a word standing on its own can be a variable about to be written.
    let standalone = prefix[..word_start]
        .chars()
        .next_back()
        .is_none_or(|c| c == ' ' || c == '\t' || c == '(' || c == ',');
    if word.is_empty() || !standalone {
        return Vec::new();
    }
    let scope = local_scope(document, position.line);
    let (first, last) = scope.as_ref().map_or((0, u32::MAX), |(range, _)| {
        (range.start.line, range.end.line)
    });
    let mut names: Vec<String> = variables::occurrences(document.text())
        .into_iter()
        .filter(|occurrence| (first..=last).contains(&occurrence.line))
        .filter(|occurrence| occurrence.line != position.line)
        .map(|occurrence| occurrence.name)
        .filter(|name| !name.contains("::") && !name.contains('%'))
        .collect();
    if let Some((_, parameters)) = scope {
        names.extend(parameters.into_iter().map(|name| format!("_{name}")));
    }
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup_by_key(|name| name.to_lowercase());

    let replace = to_lsp_range(
        document.text(),
        skript_index::Range::new(
            skript_index::Position::new(position.line, word_start as u32),
            position,
        ),
        encoding,
    );
    names
        .into_iter()
        .filter(|name| {
            name.trim_start_matches('_')
                .to_lowercase()
                .starts_with(&word)
        })
        .map(|name| {
            let text = format!("{{{name}}}");
            // Above everything: a local the trigger already uses is a far
            // likelier next word than any syntax that happens to share letters,
            // and catalog scores run high enough that a mere bonus left
            // `{_visitor}` below forty "Visibility" entries.
            let score = ranking::TOP_SCORE;
            let item = CompletionItem {
                label: text.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some("local variable".into()),
                filter_text: Some(name.trim_start_matches('_').to_string()),
                sort_text: Some(sort_key(score, &text)),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace,
                    new_text: text,
                })),
                ..Default::default()
            };
            (score, item)
        })
        .collect()
}

pub fn local_scope(document: &Document, line: u32) -> Option<(skript_index::Range, Vec<String>)> {
    // The structure a line belongs to, read from indentation alone: from the
    // nearest column-0 line at or above it to the next one below. A `{_x`
    // still being typed (before the editor closes the brace) can upset the
    // parse tree enough that its structures run together, and completion then
    // offered every local in the file. Indentation never runs together.
    let lines: Vec<&str> = document.text().lines().collect();
    let opens = |text: &&str| {
        !text.is_empty() && !text.starts_with(char::is_whitespace) && !text.starts_with('#')
    };
    let first = (0..=line as usize)
        .rev()
        .find(|&at| lines.get(at).is_some_and(opens))
        .unwrap_or(0) as u32;
    let last = (line as usize + 1..lines.len())
        .find(|&at| opens(&lines[at]))
        .map_or(lines.len().saturating_sub(1) as u32, |next| next as u32 - 1);
    let trigger = skript_index::Range::new(
        skript_index::Position::new(first, 0),
        skript_index::Position::new(last, u32::MAX),
    );

    let Some(top) = document.symbols().enclosing_line(line, |_| true) else {
        return Some((trigger, Vec::new()));
    };
    let scope = top
        .children
        .iter()
        .find(|child| child.kind == SymbolKind::Method && child.range.spans_line(line))
        .unwrap_or(top);
    let parameters = match scope.kind {
        SymbolKind::Function | SymbolKind::LocalFunction | SymbolKind::Method => scope
            .children
            .iter()
            .filter(|child| child.kind == SymbolKind::LocalVariable)
            .map(|child| child.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    // Never wider than the indentation says.
    let range = skript_index::Range::new(
        scope.range.start.max(trigger.start),
        scope.range.end.min(trigger.end),
    );
    Some((range, parameters))
}

#[cfg(test)]
mod tests;
