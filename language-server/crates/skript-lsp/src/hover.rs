//! `textDocument/hover`.

use skript_index::symbols::DocComment;
use skript_index::{Document, Range, Symbol, SymbolKind, Workspace};
use tower_lsp::lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};

use crate::completion::{in_command_entry_position, local_scope};
use crate::convert::{from_lsp_position, to_lsp_range};
use crate::navigation::{enclosing_class, member_definitions};
use crate::server::State;
use crate::{entries, semantic, variables};

/// How many SkriptHub examples an addon syntax's hover card shows.
const HOVER_EXAMPLE_LIMIT: usize = 2;

pub fn markdown(value: String) -> MarkupContent {
    MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    }
}

/// A Markdown hover card, optionally anchored to the span it describes.
fn markdown_hover(text: String, range: Option<tower_lsp::lsp_types::Range>) -> Hover {
    Hover {
        contents: HoverContents::Markup(markdown(text)),
        range,
    }
}

/// What hover can answer while holding the state lock.
pub enum Answer {
    Ready(Option<Hover>),
    /// An addon syntax whose card still needs its examples fetched, which is
    /// done after the lock is released: a slow first fetch must never hold up
    /// the edits queued behind it.
    NeedsExamples {
        text: String,
        span: tower_lsp::lsp_types::Range,
        skripthub_id: u64,
    },
}

/// Completes an [`Answer`], fetching examples when it needs them.
pub async fn finish(answer: Answer) -> Option<Hover> {
    let (mut text, span, skripthub_id) = match answer {
        Answer::Ready(hover) => return hover,
        Answer::NeedsExamples {
            text,
            span,
            skripthub_id,
        } => (text, span, skripthub_id),
    };
    let fetched = tokio::task::spawn_blocking(move || {
        skript_docs::examples::fetch_examples(
            skripthub_id,
            &crate::loading::cache_directory(),
            HOVER_EXAMPLE_LIMIT,
        )
    })
    .await;
    if let Ok(Ok(examples)) = fetched {
        if !examples.is_empty() {
            text.push_str("\n\n**Examples**");
            for example in examples {
                text.push_str(&format!("\n\n```skript\n{example}\n```"));
            }
        }
    }
    text.push_str(&format!(
        "\n\n[View on SkriptHub]({})",
        skript_docs::examples::skripthub_page(skripthub_id)
    ));
    Some(markdown_hover(text, Some(span)))
}

/// Everything hover can decide from the state alone.
pub fn prepare(state: &State, params: &HoverParams) -> Answer {
    let position = &params.text_document_position_params;
    let Some(document) = state.workspace.get(position.text_document.uri.as_str()) else {
        return Answer::Ready(None);
    };
    let position = from_lsp_position(document.text(), position.position, state.encoding);
    let range = |range: Range| Some(to_lsp_range(document.text(), range, state.encoding));

    // A declaration or reference the cursor sits on wins: it is the more
    // specific answer, and it works without the catalog.
    //
    // Only for things the script itself names. An event, a section or an
    // entry is "declared" only in the sense of having an outline entry; its
    // card was just its own text back (`**packet with packetevents priority
    // high**`) while the catalog has the real documentation. And because only
    // the first word of an event's name is its selection, hovering `first` in
    // `on first join:` got that empty card while hovering `join` got the docs.
    let named_by_script = |kind: SymbolKind| {
        !matches!(
            kind,
            SymbolKind::Event
                | SymbolKind::Section
                | SymbolKind::Entry
                | SymbolKind::Structure
                | SymbolKind::Syntax
        )
    };
    // A skript-reflect definition's header shows the same card as its uses.
    if let Some((text, span)) = definition_hover(state, document, position) {
        return Answer::Ready(Some(markdown_hover(text, range(span))));
    }
    if let Some(symbol) = document
        .symbols()
        .declaration_at(position)
        .filter(|symbol| named_by_script(symbol.kind))
    {
        let mut text = format!("**{}**", symbol.name);
        if !symbol.detail.is_empty() {
            text.push_str(&format!("  \n`{}`", symbol.detail));
        }
        text.push_str(&documentation(document, symbol));
        // A parameter in the signature carries its `@param` text.
        if let Some(note) = owner_of(document, symbol)
            .and_then(|owner| document.symbols().doc_for(owner.selection_range))
            .and_then(|doc| doc.param(&symbol.name))
            .filter(|note| !note.is_empty())
        {
            text.push_str(&format!("\n\n{note}"));
        }
        return Answer::Ready(Some(markdown_hover(text, range(symbol.selection_range))));
    }

    // A variable is described as itself, never as the expression around it.
    if let Some((text, span)) = variable_hover(&state.workspace, document, position) {
        return Answer::Ready(Some(markdown_hover(text, range(span))));
    }

    // A call to the user's own function, or a ClassySK `self::play(…)` /
    // `{_bar}::player`: show the declaration. Without this, hovering a call
    // described whatever catch-all pattern the *line* happened to match
    // (skript-reflect's `[(await)] %javaobject%`, in a real project). Members
    // are matched by name alone (see `navigable`), so every class declaring
    // one is listed under its own name rather than guessing.
    if let Some(reference) = document.symbols().reference_at(position) {
        // `floor(2.5)` is a function Skript or an addon registers. The catalog
        // documents it; the line's own classification does not.
        let builtin = (reference.kind == SymbolKind::Function)
            .then_some(state.catalog.as_ref())
            .flatten()
            .and_then(|catalog| {
                let category = skript_docs::Category::Function;
                let index = catalog
                    .docs()
                    .entries(category)
                    .iter()
                    .position(|entry| entry.name == reference.name)?;
                catalog.hover(skript_docs::EntryId { category, index })
            });
        if let Some(text) = reference_hover(&state.workspace, document, reference).or(builtin) {
            return Answer::Ready(Some(markdown_hover(text, range(reference.name_range))));
        }
    }

    catalog_hover(state, document, position)
}

/// The syntax the line resolves to, from the catalog.
fn catalog_hover(state: &State, document: &Document, position: skript_index::Position) -> Answer {
    let Some(catalog) = &state.catalog else {
        return Answer::Ready(None);
    };
    let line = document.line(position.line);

    // A structure entry is not a syntax pattern, so the catalog can never
    // explain it. `docs.json` describes no entries at all, which is why these
    // are the one curated table in the server; see `entries`.
    //
    // Only the key is looked up. Passing the whole line meant
    // `permission: home.use` never matched `permission`, so of all the
    // entries only a bare `trigger:` ever had a card.
    if in_command_entry_position(document, position.line) {
        let key = line.split(':').next().unwrap_or_default();
        if let Some(entry) = entries::lookup(entries::COMMAND, key) {
            return Answer::Ready(Some(markdown_hover(entries::hover(entry), None)));
        }
    }

    // An addon section's entries are not patterns either, but SkriptHub
    // documents which ones each section takes.
    if let Some(text) = crate::completion::section_entry_hover(catalog, document, position.line) {
        return Answer::Ready(Some(markdown_hover(text, None)));
    }

    // `self` and a ClassySK class name are the script's own, so they describe
    // the class rather than any pattern.
    if let Some(text) = class_hover(&state.workspace, document, position) {
        return Answer::Ready(Some(markdown_hover(text, None)));
    }

    // The innermost syntax under the cursor: `{_p}'s helmet` inside a `set`
    // names Armor Slot, not Set. Same role filter as semantic tokens and
    // diagnostics, so a line is never claimed by the catch-all
    // "Creature/Entity/Player/…" pattern that matches anything.
    let Some((id, span_start, span_end)) = semantic::innermost_at(
        catalog,
        document.symbols(),
        line,
        position.line,
        position.character as usize,
    ) else {
        return Answer::Ready(None);
    };
    let span = to_lsp_range(
        document.text(),
        Range::on_line(position.line, span_start as u32, span_end as u32),
        state.encoding,
    );
    // The project's own syntax: its card, and where it is defined.
    if catalog.is_workspace(id) {
        let Some(entry) = catalog.entry(id) else {
            return Answer::Ready(None);
        };
        let text = workspace_card(state, document, id, entry);
        return Answer::Ready(Some(markdown_hover(text, Some(span))));
    }
    let Some(mut text) = catalog.hover(id) else {
        return Answer::Ready(None);
    };

    // Availability is the first thing a reader wants and the last thing the
    // rendered card carries, so it goes at the top.
    let notes = provenance_notes(state, catalog, id);
    if !notes.is_empty() {
        text = format!("{}\n\n{text}", notes.join(" · "));
    }

    // SkriptHub's catalog carries no examples, so addon syntax hovered without
    // any. They are fetched per syntax and cached for a week.
    let examples_for = catalog
        .entry(id)
        .filter(|entry| entry.examples.is_empty())
        .and_then(|entry| entry.skripthub_id);
    match examples_for {
        Some(skripthub_id) => Answer::NeedsExamples {
            text,
            span,
            skripthub_id,
        },
        None => Answer::Ready(Some(markdown_hover(text, Some(span)))),
    }
}

/// The card for the skript-reflect definition whose header the cursor is on.
fn definition_hover(
    state: &State,
    document: &Document,
    position: skript_index::Position,
) -> Option<(String, Range)> {
    let catalog = state.catalog.as_ref()?;
    let definition = document
        .symbols()
        .syntax
        .iter()
        .find(|definition| definition.selection_range.touches(position))?;
    let key = definition.key();
    let (id, entry) = catalog
        .workspace_entries()
        .find(|(_, entry)| entry.id == key)?;
    let text = workspace_card(state, document, id, entry);
    Some((text, definition.selection_range))
}

/// The card for one of the project's own syntax entries: its doc comment,
/// patterns, and where it is defined.
///
/// The catalog entry's description already holds the comment above the
/// definition, joined into one line with its tags still in it (`@since 1.2`
/// read as prose). When the definition has a doc comment, that line is swapped
/// for the parsed description and the tags are rendered after the card.
fn workspace_card(
    state: &State,
    document: &Document,
    id: skript_docs::EntryId,
    entry: &skript_docs::Entry,
) -> String {
    let documented = state.catalog.as_ref().and_then(|catalog| {
        crate::custom_syntax::definitions_of(&state.workspace, catalog, id, document.uri())
            .into_iter()
            .find_map(|(target, definition)| {
                let doc = target.symbols().doc_for(definition.selection_range)?;
                Some((doc, definition.comment.join(" ")))
            })
    });
    let Some((doc, raw)) = documented else {
        let mut text = crate::custom_syntax::card(id.category, entry);
        text.push_str(&defined_at(state, document, id));
        return text;
    };
    let mut shown = entry.clone();
    if shown.description.first() == Some(&raw) {
        shown.description.remove(0);
    }
    if !doc.description.is_empty() {
        shown.description.insert(0, doc.description.clone());
    }
    let mut text = crate::custom_syntax::card(id.category, &shown);
    let tags = DocComment {
        description: String::new(),
        ..doc.clone()
    };
    let rendered = doc_markdown(&tags, None, None);
    if !rendered.is_empty() {
        text.push_str(&format!("\n\n{rendered}"));
    }
    text.push_str(&defined_at(state, document, id));
    text
}

/// The rendered doc comment of `symbol`, a function, method or command,
/// with a leading blank line; empty when it has none.
fn documentation(document: &Document, symbol: &Symbol) -> String {
    let Some(doc) = document.symbols().doc_for(symbol.selection_range) else {
        return String::new();
    };
    let parameters = (symbol.kind != SymbolKind::Command).then_some(symbol.children.as_slice());
    let rendered = doc_markdown(doc, parameters, return_type(&symbol.detail));
    if rendered.is_empty() {
        String::new()
    } else {
        format!("\n\n{rendered}")
    }
}

/// The documentation of the project function a completion item offers, for
/// `completionItem/resolve`.
///
/// Those items carry no `data` (it is the catalog's index), so the function is
/// found again by name and signature, both of which the item shows. The
/// signature picks between same-named local functions in different scripts.
pub fn declared_function_documentation(
    workspace: &Workspace,
    item: &tower_lsp::lsp_types::CompletionItem,
) -> Option<String> {
    use tower_lsp::lsp_types::CompletionItemKind;
    let from_project = item
        .label_details
        .as_ref()
        .and_then(|details| details.description.as_deref())
        == Some("this project");
    if item.kind != Some(CompletionItemKind::FUNCTION) || !from_project {
        return None;
    }
    let detail = item.detail.as_deref().unwrap_or_default();
    workspace.documents().find_map(|document| {
        let function = document.symbols().symbols.iter().find(|symbol| {
            symbol.kind.is_function() && symbol.name == item.label && symbol.detail == detail
        })?;
        let rendered = documentation(document, function);
        (!rendered.is_empty()).then(|| rendered.trim_start().to_string())
    })
}

/// `boolean` from a signature `(p: player) :: boolean`.
fn return_type(signature: &str) -> Option<&str> {
    signature
        .rsplit_once("::")
        .map(|(_, ty)| ty.trim())
        .filter(|ty| !ty.is_empty())
}

/// The function or method whose parameter `parameter` is.
fn owner_of<'d>(document: &'d Document, parameter: &Symbol) -> Option<&'d Symbol> {
    if parameter.kind != SymbolKind::LocalVariable {
        return None;
    }
    document.symbols().flat().into_iter().find(|symbol| {
        matches!(
            symbol.kind,
            SymbolKind::Function | SymbolKind::LocalFunction | SymbolKind::Method
        ) && symbol
            .children
            .iter()
            .any(|child| std::ptr::eq(child, parameter))
    })
}

/// Markdown for a doc comment.
///
/// `parameters` are the declaration's own parameter symbols, so each
/// `@param` is shown with the type the signature declares, in signature
/// order, and an `@param` naming none of them is left out (the
/// `doc-unknown-param` hint reports it). `None` is for declarations whose
/// arguments have no names, a command's or a custom syntax's, where every
/// `@param` is shown as written.
pub fn doc_markdown(
    doc: &DocComment,
    parameters: Option<&[Symbol]>,
    returns: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    // First, as a deprecation is the one thing a caller must not miss.
    match doc.deprecated.as_deref() {
        Some("") => parts.push("**Deprecated.**".to_string()),
        Some(note) => parts.push(format!("**Deprecated:** {note}")),
        None => {}
    }
    if !doc.description.is_empty() {
        parts.push(doc.description.clone());
    }
    let lines: Vec<String> = match parameters {
        _ if doc.params.is_empty() => Vec::new(),
        Some(parameters) => parameters
            .iter()
            .map(|parameter| {
                let ty = parameter.detail.trim();
                let label = if ty.is_empty() {
                    format!("`{}`", parameter.name)
                } else {
                    format!("`{}: {ty}`", parameter.name)
                };
                match doc.param(&parameter.name).filter(|text| !text.is_empty()) {
                    Some(text) => format!("- {label}: {text}"),
                    None => format!("- {label}"),
                }
            })
            .collect(),
        None => doc
            .params
            .iter()
            .map(|param| match param.text.as_str() {
                "" => format!("- `{}`", param.name),
                text => format!("- `{}`: {text}", param.name),
            })
            .collect(),
    };
    if !lines.is_empty() {
        parts.push(format!("**Parameters**\n{}", lines.join("\n")));
    }
    match (doc.returns.as_deref(), returns) {
        (Some(text), Some(ty)) if !text.is_empty() => {
            parts.push(format!("**Returns** `{ty}`: {text}"))
        }
        (Some(text), None) if !text.is_empty() => parts.push(format!("**Returns:** {text}")),
        _ => {}
    }
    if let Some(since) = doc.since.as_deref().filter(|since| !since.is_empty()) {
        parts.push(format!("**Since** {since}"));
    }
    if !doc.examples.is_empty() {
        let blocks: Vec<String> = doc
            .examples
            .iter()
            .map(|example| format!("```skript\n{example}\n```"))
            .collect();
        let title = if blocks.len() == 1 {
            "Example"
        } else {
            "Examples"
        };
        parts.push(format!("**{title}**\n\n{}", blocks.join("\n\n")));
    }
    parts.join("\n\n")
}

/// "Defined in `file.sk`, line 3." for each place the project syntax `id` is
/// defined.
fn defined_at(state: &State, document: &Document, id: skript_docs::EntryId) -> String {
    let Some(catalog) = &state.catalog else {
        return String::new();
    };
    let places: Vec<String> =
        crate::custom_syntax::definitions_of(&state.workspace, catalog, id, document.uri())
            .into_iter()
            .map(|(target, definition)| {
                let file = target.uri().rsplit('/').next().unwrap_or(target.uri());
                format!(
                    "`{file}`, line {}",
                    definition.selection_range.start.line + 1
                )
            })
            .collect();
    if places.is_empty() {
        String::new()
    } else {
        format!("\n\nDefined in {}.", places.join("; "))
    }
}

/// Whether the target Skript can run a syntax, and which addon it is from.
fn provenance_notes(
    state: &State,
    catalog: &skript_docs::Catalog,
    id: skript_docs::EntryId,
) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(note) = catalog.availability(id) {
        notes.push(format!("**Note:** {note}"));
    }
    let Some(addon) = catalog.entry(id).and_then(|entry| entry.addon.as_ref()) else {
        return notes;
    };
    let installed = state
        .detection
        .addon_names()
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&addon.name));
    let label = if addon.url.is_empty() {
        addon.name.clone()
    } else {
        format!("[{}]({})", addon.name, addon.url)
    };
    let version = addon
        .since_version
        .as_deref()
        .map(|v| format!(" ≥ {v}"))
        .unwrap_or_default();
    let state_note = if state.detection.is_known() && !installed {
        ", not installed"
    } else {
        ""
    };
    notes.push(format!("from {label}{version}{state_note}"));
    notes
}

/// Hover for the variable under the cursor: what kind it is, where it comes
/// from, how much it is used. Read from the text, so
/// `{data::%player%::coins}`, which the index skips because its name is
/// dynamic, gets one too.
///
/// Without this, hovering `{_p}` described whatever expression the variable
/// sat inside (`{_p}'s helmet` → "Armor Slot"), which says nothing about `_p`.
pub fn variable_hover(
    workspace: &Workspace,
    document: &Document,
    position: skript_index::Position,
) -> Option<(String, Range)> {
    let cursor = position.character;
    // The innermost `{…}` around the cursor: in `{_a::{_k}}`, `{_k}`.
    let here = variables::occurrences(document.line(position.line))
        .into_iter()
        .filter(|occurrence| occurrence.start <= cursor && cursor < occurrence.end)
        .min_by_key(|occurrence| occurrence.end - occurrence.start)?;
    let name = here.name.as_str();
    let range = Range::on_line(position.line, here.start, here.end);
    let same = |other: &str| other.eq_ignore_ascii_case(name);
    let levels = variables::levels(name);
    let is_list = levels.len() > 1;
    let list_kind = if is_list { "list " } else { "" };

    let mut text = String::new();
    if variables::is_local(name) {
        text.push_str(&format!("**`{{{name}}}`** · local {list_kind}variable"));
        let scope = local_scope(document, position.line);
        let (first, last) = scope.as_ref().map_or((0, u32::MAX), |(range, _)| {
            (range.start.line, range.end.line)
        });
        text.push_str(&parameter_note(document, name, first, last));
        let uses: Vec<u32> = variables::occurrences(document.text())
            .into_iter()
            .filter(|occurrence| {
                (first..=last).contains(&occurrence.line) && same(&occurrence.name)
            })
            .map(|occurrence| occurrence.line)
            .collect();
        let first_use = uses
            .first()
            .map(|line| format!(", first on line {}", line + 1));
        text.push_str(&format!(
            "\n\nUsed {} time{} in this trigger{}.",
            uses.len(),
            plural(uses.len()),
            first_use.unwrap_or_default()
        ));
        text.push_str(" Local variables exist only while the trigger runs.");
    } else {
        let ephemeral = name.starts_with('-');
        text.push_str(&format!(
            "**`{{{name}}}`** · global {list_kind}variable{}",
            if ephemeral { ", not saved to disk" } else { "" }
        ));
        let (mut uses, mut files) = (0usize, 0usize);
        for other in workspace.documents() {
            let count = variables::occurrences(other.text())
                .iter()
                .filter(|occurrence| same(&occurrence.name))
                .count();
            if count > 0 {
                uses += count;
                files += 1;
            }
        }
        text.push_str(&format!(
            "\n\nUsed {uses} time{} across {files} script{}.",
            plural(uses),
            plural(files)
        ));
    }
    if is_list {
        let root = format!("{{{}::*}}", levels[..levels.len() - 1].join("::"));
        text.push_str(&format!(" Every value in the list: `{root}`."));
    }
    Some((text, range))
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// "Parameter `name: type` of `f`." when the local `name` is a parameter of
/// the function or method declared on lines `first..=last`.
fn parameter_note(document: &Document, name: &str, first: u32, last: u32) -> String {
    let flat = document.symbols().flat();
    let Some(parameter) = flat.iter().copied().find(|symbol| {
        symbol.kind == SymbolKind::LocalVariable
            && symbol
                .name
                .eq_ignore_ascii_case(name.trim_start_matches('_'))
            && symbol.range.start.line >= first
            && symbol.range.end.line <= last
    }) else {
        return String::new();
    };
    let owner = owner_of(document, parameter);
    let note = owner
        .and_then(|owner| document.symbols().doc_for(owner.selection_range))
        .and_then(|doc| doc.param(&parameter.name))
        .filter(|note| !note.is_empty())
        .map(|note| format!(": {note}"))
        .unwrap_or_else(|| ".".into());
    let owner = owner
        .map(|owner| format!(" of `{}`", owner.name))
        .unwrap_or_default();
    format!(
        "\n\nParameter `{}: {}`{owner}{note}",
        parameter.name,
        parameter.detail.trim()
    )
}

/// Hover for `self` or a ClassySK / oopsk class name under the cursor: the
/// class and what it holds.
pub fn class_hover(
    workspace: &Workspace,
    document: &Document,
    position: skript_index::Position,
) -> Option<String> {
    let line = document.line(position.line);
    let cursor = (position.character as usize).min(line.len());
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let start = line[..cursor]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_word(*ch))
        .last()
        .map_or(cursor, |(at, _)| at);
    let end = line[cursor..]
        .char_indices()
        .find(|(_, ch)| !is_word(*ch))
        .map_or(line.len(), |(at, _)| cursor + at);
    let word = &line[start..end];
    if word.is_empty() {
        return None;
    }

    let (class, intro) = if word == "self" {
        // The class whose body this line is in.
        let class = document
            .symbols()
            .enclosing_line(position.line, |kind| kind == SymbolKind::Class)?;
        (
            class,
            format!("`self` is the current **{}** instance.", class.name),
        )
    } else {
        let (_, class) = workspace
            .definitions(SymbolKind::Class, word, document.uri())
            .into_iter()
            .next()?;
        let kind = if class.detail == "struct" {
            "oopsk struct"
        } else {
            "ClassySK class"
        };
        (class, format!("**{}** · {kind}", class.name))
    };

    // Members are listed the way they are used at the cursor (`self::player`),
    // not as the declaration's `player: player`, which read as a typo for `::`.
    let access = if word == "self" { "self::" } else { "" };
    let section = |kind: SymbolKind, title: &str| {
        let lines: Vec<String> = class
            .children
            .iter()
            .filter(|member| member.kind == kind)
            .map(|member| match kind {
                SymbolKind::Method => format!("- `{access}{}{}`", member.name, member.detail),
                _ => format!("- `{access}{}` · {}", member.name, member.detail.trim()),
            })
            .collect();
        (!lines.is_empty()).then(|| format!("\n\n**{title}**\n{}", lines.join("\n")))
    };
    let mut text = intro;
    text.extend(section(SymbolKind::Field, "Fields"));
    text.extend(section(SymbolKind::Method, "Methods"));
    Some(text)
}

/// Hover text for a call to a declared function or a ClassySK member: one line
/// per declaration. A Skript built-in (`floor`) has none in the workspace and
/// falls through to the catalog, which documents it.
pub fn reference_hover(
    workspace: &Workspace,
    from: &Document,
    reference: &skript_index::Reference,
) -> Option<String> {
    if !matches!(
        reference.kind,
        SymbolKind::Function | SymbolKind::Field | SymbolKind::Method
    ) {
        return None;
    }
    let mut cards = Vec::new();
    for (document, member) in member_definitions(workspace, from, reference) {
        let class = enclosing_class(document, member.range)
            .map(|class| format!("{}::", class.name))
            .unwrap_or_default();
        // A field is shown as it is used, `Req::extra`, with its type beside
        // it; the declaration's `extra: integer` read as a typo for `::`.
        let mut card = if member.kind == SymbolKind::Field {
            format!("`{class}{}` · {}", member.name, member.detail.trim())
        } else {
            format!("`{class}{}{}`", member.name, member.detail)
        };
        card.push_str(&documentation(document, member));
        if !cards.contains(&card) {
            cards.push(card);
        }
    }
    // One line per declaration reads as a list; once any carries its
    // documentation, a rule keeps each one's text with its own signature.
    let separator = if cards.iter().any(|card| card.contains('\n')) {
        "\n\n---\n\n"
    } else {
        "  \n"
    };
    (!cards.is_empty()).then(|| cards.join(separator))
}

#[cfg(test)]
fn hover_text(state: &State, uri: &str, line: u32, character: u32) -> Option<String> {
    let params = HoverParams {
        text_document_position_params: tower_lsp::lsp_types::TextDocumentPositionParams {
            text_document: tower_lsp::lsp_types::TextDocumentIdentifier {
                uri: uri.parse().ok()?,
            },
            position: tower_lsp::lsp_types::Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
    };
    match prepare(state, &params) {
        Answer::Ready(Some(Hover {
            contents: HoverContents::Markup(markup),
            ..
        })) => Some(markup.value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(text: &str) -> State {
        let mut state = State::default();
        state.workspace.open("file:///t.sk", text);
        state
    }

    #[test]
    fn a_local_variable_is_described_with_its_uses_in_this_trigger() {
        let state =
            state_with("on join:\n\tset {_x} to 1\n\tsend {_x}\n\non quit:\n\tset {_x} to 2\n");
        let text = hover_text(&state, "file:///t.sk", 1, 6).expect("a hover");
        assert!(text.contains("local variable"), "{text}");
        assert!(text.contains("Used 2 times in this trigger"), "{text}");
    }

    #[test]
    fn a_parameter_names_its_function() {
        let state = state_with("function greet(who: player):\n\tsend \"hi\" to {_who}\n");
        let text = hover_text(&state, "file:///t.sk", 1, 16).expect("a hover");
        assert!(
            text.contains("Parameter `who: player` of `greet`"),
            "{text}"
        );
    }

    #[test]
    fn a_global_list_variable_points_at_its_root() {
        let state = state_with("on join:\n\tadd 1 to {kills::%player%}\n");
        let text = hover_text(&state, "file:///t.sk", 1, 12).expect("a hover");
        assert!(text.contains("global list variable"), "{text}");
        assert!(text.contains("`{kills::*}`"), "{text}");
    }

    #[test]
    fn self_describes_the_enclosing_class() {
        let mut state = state_with(
            "class Wallet:\n\tpublic balance: number\n\tpublic reset():\n\t\tset self::balance to 0\n",
        );
        // Class hover sits behind the catalog, as the rest of line hover does.
        state.catalog = Some(skript_docs::Catalog::build(skript_docs::fallback_docs()));
        let text = hover_text(&state, "file:///t.sk", 3, 6).expect("a hover");
        assert!(
            text.contains("`self` is the current **Wallet** instance"),
            "{text}"
        );
        assert!(text.contains("`self::balance` · number"), "{text}");
    }

    /// Every entry with a value used to get no card at all.
    #[test]
    fn an_entry_with_a_value_has_a_card() {
        let mut state = state_with(
            "command /home:\n\tpermission: home.use\n\tcooldown: 15 seconds\n\tpermission message: no\n\ttrigger:\n\t\tstop\n",
        );
        state.catalog = Some(skript_docs::Catalog::build(skript_docs::fallback_docs()));
        for (line, key) in [
            (1, "permission"),
            (2, "cooldown"),
            (3, "permission message"),
        ] {
            let text = hover_text(&state, "file:///t.sk", line, 3).expect("a hover");
            assert!(
                text.starts_with(&format!("**{key}** · command entry")),
                "line {line}: {text}"
            );
        }
    }

    const CUSTOM: &str = concat!(
        "effect [the] greet %player%:\n",
        "\ttrigger:\n",
        "\t\tstop\n",
        "on join:\n",
        "\tgreet player\n",
    );

    fn state_with_custom_syntax() -> State {
        let mut state = state_with(CUSTOM);
        state.catalog = Some(skript_docs::Catalog::build(skript_docs::fallback_docs()));
        crate::custom_syntax::sync(&mut state);
        state
    }

    /// A use of the project's own effect hovers with its card and location,
    /// and without the internal key the catalog uses as its id.
    #[test]
    fn a_custom_effect_hovers_with_where_it_is_defined() {
        let state = state_with_custom_syntax();
        let text = hover_text(&state, "file:///t.sk", 4, 2).expect("a hover");
        assert!(text.starts_with("**[the] greet %player%**"), "{text}");
        assert!(text.contains("Defined in `t.sk`, line 1."), "{text}");
        assert!(!text.contains("effect::"), "{text}");
        assert!(!text.contains("not installed"), "{text}");
    }

    #[test]
    fn the_definitions_header_shows_the_same_card() {
        let state = state_with_custom_syntax();
        let text = hover_text(&state, "file:///t.sk", 0, 10).expect("a hover");
        assert!(text.starts_with("**[the] greet %player%**"), "{text}");
    }

    const DOCUMENTED: &str = concat!(
        "# Gives a welcome kit.\n",
        "# @param p who gets it\n",
        "# @param stale renamed long ago\n",
        "# @return whether it fit\n",
        "# @since 1.2\n",
        "# @example kit(player, 1)\n",
        "function kit(p: player, n: number) :: boolean:\n",
        "\tsend \"hi\" to {_p}\n",
        "\treturn true\n",
        "on join:\n",
        "\tkit(player, 1)\n",
    );

    const KIT_DOCS: &str = concat!(
        "Gives a welcome kit.\n\n",
        "**Parameters**\n",
        "- `p: player`: who gets it\n",
        "- `n: number`\n\n",
        "**Returns** `boolean`: whether it fit\n\n",
        "**Since** 1.2\n\n",
        "**Example**\n\n",
        "```skript\nkit(player, 1)\n```",
    );

    /// The declaration and every call show the same documentation; the stale
    /// `@param` is left out, since it names nothing in the signature.
    #[test]
    fn a_documented_function_hovers_with_its_docs_at_declaration_and_call() {
        let state = state_with(DOCUMENTED);
        let declaration = hover_text(&state, "file:///t.sk", 6, 10).expect("a hover");
        assert_eq!(
            declaration,
            format!("**kit**  \n`(p: player, n: number) :: boolean`\n\n{KIT_DOCS}")
        );
        let call = hover_text(&state, "file:///t.sk", 10, 2).expect("a hover");
        assert_eq!(
            call,
            format!("`kit(p: player, n: number) :: boolean`\n\n{KIT_DOCS}")
        );
    }

    #[test]
    fn a_parameter_carries_its_at_param_text() {
        let state = state_with(DOCUMENTED);
        let text = hover_text(&state, "file:///t.sk", 7, 16).expect("a hover");
        assert!(
            text.contains("Parameter `p: player` of `kit`: who gets it"),
            "{text}"
        );
    }

    #[test]
    fn a_deprecated_method_says_so_first() {
        let state = state_with(concat!(
            "class Wallet:\n",
            "\t#d Empties the wallet.\n",
            "\t#ddep use clear() instead\n",
            "\tpublic reset():\n",
            "\t\tstop\n",
            "on join:\n",
            "\tWallet::reset()\n",
        ));
        let text = hover_text(&state, "file:///t.sk", 6, 11).expect("a hover");
        assert_eq!(
            text,
            "`Wallet::reset()`\n\n**Deprecated:** use clear() instead\n\nEmpties the wallet."
        );
    }

    #[test]
    fn a_command_lists_its_params_as_written() {
        let state = state_with(concat!(
            "# Teleports home.\n",
            "# @param name which home\n",
            "command /home <text>:\n",
            "\ttrigger:\n",
            "\t\tstop\n",
        ));
        let text = hover_text(&state, "file:///t.sk", 2, 10).expect("a hover");
        assert!(
            text.ends_with("Teleports home.\n\n**Parameters**\n- `name`: which home"),
            "{text}"
        );
    }

    /// The comment's tags are parsed out of the custom syntax card's prose.
    #[test]
    fn a_documented_custom_effect_renders_its_tags() {
        let mut state = state_with(concat!(
            "# Greets someone.\n",
            "# @since 2.0\n",
            "effect [the] greet %player%:\n",
            "\ttrigger:\n",
            "\t\tstop\n",
            "on join:\n",
            "\tgreet player\n",
        ));
        state.catalog = Some(skript_docs::Catalog::build(skript_docs::fallback_docs()));
        crate::custom_syntax::sync(&mut state);
        let text = hover_text(&state, "file:///t.sk", 6, 2).expect("a hover");
        assert!(text.contains("Greets someone."), "{text}");
        assert!(!text.contains("@since"), "{text}");
        assert!(text.contains("**Since** 2.0"), "{text}");
        assert!(text.contains("Defined in `t.sk`, line 3."), "{text}");
    }

    #[test]
    fn completion_resolve_documents_a_project_function() {
        use tower_lsp::lsp_types::{
            CompletionItem, CompletionItemKind, CompletionItemLabelDetails,
        };
        let state = state_with(DOCUMENTED);
        let item = CompletionItem {
            label: "kit".into(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("(p: player, n: number) :: boolean".into()),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some("this project".into()),
            }),
            ..Default::default()
        };
        let resolved = crate::completion::completion_resolve(&state, item);
        let Some(tower_lsp::lsp_types::Documentation::MarkupContent(markup)) =
            resolved.documentation
        else {
            panic!("no documentation");
        };
        assert_eq!(markup.value, KIT_DOCS);
    }

    #[test]
    fn a_command_entry_has_no_em_dash_in_its_card() {
        let state = {
            let mut state = state_with("command /a:\n\tpermission: x\n\ttrigger:\n\t\tstop\n");
            state.catalog = Some(skript_docs::Catalog::build(skript_docs::fallback_docs()));
            state
        };
        let text = hover_text(&state, "file:///t.sk", 2, 3).expect("a hover");
        assert!(text.starts_with("**trigger** · command entry"), "{text}");
    }
}
