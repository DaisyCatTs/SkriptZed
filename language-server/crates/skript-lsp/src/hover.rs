//! `textDocument/hover`.

use skript_index::{Document, Range, SymbolKind, Workspace};
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
        let mut text = crate::custom_syntax::card(id.category, entry);
        text.push_str(&defined_at(state, document, id));
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
    let mut text = crate::custom_syntax::card(id.category, entry);
    text.push_str(&defined_at(state, document, id));
    Some((text, definition.selection_range))
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
    let owner = flat
        .iter()
        .find(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::Function | SymbolKind::LocalFunction | SymbolKind::Method
            ) && symbol
                .children
                .iter()
                .any(|child| std::ptr::eq(child, parameter))
        })
        .map(|owner| format!(" of `{}`", owner.name))
        .unwrap_or_default();
    format!(
        "\n\nParameter `{}: {}`{owner}.",
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
    let mut lines = Vec::new();
    for (document, member) in member_definitions(workspace, from, reference) {
        let class = enclosing_class(document, member.range)
            .map(|class| format!("{}::", class.name))
            .unwrap_or_default();
        // A field is shown as it is used, `Req::extra`, with its type beside
        // it; the declaration's `extra: integer` read as a typo for `::`.
        let line = if member.kind == SymbolKind::Field {
            format!("`{class}{}` · {}", member.name, member.detail.trim())
        } else {
            format!("`{class}{}{}`", member.name, member.detail)
        };
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    (!lines.is_empty()).then(|| lines.join("  \n"))
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
