//! Go-to-definition, find-references, document highlight and rename.
//!
//! All four resolve the symbol under the cursor the same way, through
//! [`scoped_symbol_under_cursor`], so the editor never highlights a different
//! symbol than F12 navigates to or rename rewrites.

use std::collections::HashMap;

use skript_index::{Document, Range, Reference, Symbol, SymbolKind, Workspace};
use tower_lsp::lsp_types::{
    DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams, GotoDefinitionParams,
    GotoDefinitionResponse, Location, PrepareRenameResponse, ReferenceParams, RenameParams,
    TextDocumentPositionParams, TextEdit, Url, WorkspaceEdit,
};

use crate::convert::{from_lsp_position, to_lsp_range, Encoding};
use crate::server::State;

/// The document a request names and the cursor in index coordinates.
fn at_cursor<'s>(
    state: &'s State,
    position: &TextDocumentPositionParams,
) -> Option<(&'s Document, skript_index::Position)> {
    let document = state.workspace.get(position.text_document.uri.as_str())?;
    let cursor = from_lsp_position(document.text(), position.position, state.encoding);
    Some((document, cursor))
}

/// An LSP location for `range` in `document`, when its URI parses.
fn location(document: &Document, range: Range, encoding: Encoding) -> Option<Location> {
    Some(Location {
        uri: Url::parse(document.uri()).ok()?,
        range: to_lsp_range(document.text(), range, encoding),
    })
}

pub fn goto_definition(
    state: &State,
    params: &GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let (document, position) = at_cursor(state, &params.text_document_position_params)?;
    let locations: Vec<Location> = match document.symbols().reference_at(position) {
        Some(reference) => member_definitions(&state.workspace, document, reference)
            .into_iter()
            .filter_map(|(target, symbol)| location(target, symbol.selection_range, state.encoding))
            .collect(),
        None => custom_syntax_definitions(state, document, position),
    };
    (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
}

/// Where the skript-reflect syntax used under the cursor is defined.
///
/// The innermost syntax at the cursor is tried first, so `greet the answer`
/// can reach the expression `the answer`; then the whole line, so the cursor
/// on `player` in `greet player` still reaches the custom effect rather than
/// stopping at the core expression `player`.
fn custom_syntax_definitions(
    state: &State,
    document: &Document,
    position: skript_index::Position,
) -> Vec<Location> {
    let Some(catalog) = &state.catalog else {
        return Vec::new();
    };
    let raw = document.line(position.line);
    let innermost = crate::semantic::innermost_at(
        catalog,
        document.symbols(),
        raw,
        position.line,
        position.character as usize,
    )
    .map(|(id, _, _)| id);
    let code = crate::lines::code_part(raw).trim();
    let role = skript_docs::LineRole::from_indent(skript_index::indent_width(raw));
    let whole_line = catalog.classify_line(code, role).map(|(id, _)| id);

    [innermost, whole_line]
        .into_iter()
        .flatten()
        .find(|id| catalog.is_workspace(*id))
        .map(|id| {
            crate::custom_syntax::definitions_of(&state.workspace, catalog, id, document.uri())
                .into_iter()
                .filter_map(|(target, definition)| {
                    location(target, definition.selection_range, state.encoding)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn references(state: &State, params: &ReferenceParams) -> Option<Vec<Location>> {
    let (document, position) = at_cursor(state, &params.text_document_position)?;
    let (kind, name, scope) = scoped_symbol_under_cursor(document, position)?;
    let uri = document.uri();

    let mut locations: Vec<Location> = state
        .workspace
        .references_in_scope(kind, &name, uri, scope)
        .into_iter()
        .filter_map(|(target, reference)| location(target, reference.range, state.encoding))
        .collect();
    if params.context.include_declaration {
        locations.extend(
            state
                .workspace
                .definitions(kind, &name, uri)
                .into_iter()
                .filter_map(|(target, symbol)| {
                    location(target, symbol.selection_range, state.encoding)
                }),
        );
    }
    Some(locations)
}

/// Highlights every other use of the symbol under the cursor.
///
/// Restricted to the current file by definition of the request, so it does
/// not need the workspace scope rules, but it does need the same resolution as
/// go-to-definition, or the editor would highlight a different symbol than F12
/// navigates to.
pub fn document_highlight(
    state: &State,
    params: &DocumentHighlightParams,
) -> Option<Vec<DocumentHighlight>> {
    let (document, position) = at_cursor(state, &params.text_document_position_params)?;
    let (kind, name, scope) = scoped_symbol_under_cursor(document, position)?;
    let highlight = |range: Range, kind: DocumentHighlightKind| DocumentHighlight {
        range: to_lsp_range(document.text(), range, state.encoding),
        kind: Some(kind),
    };

    let mut out = Vec::new();
    for symbol in document.symbols().flat() {
        if kinds_alike(kind, symbol.kind) && symbol.name == name {
            out.push(highlight(
                symbol.selection_range,
                DocumentHighlightKind::WRITE,
            ));
        }
    }
    for reference in &document.symbols().references {
        // A trigger-local only highlights within its own trigger. Lighting up
        // every `{_i}` in the file is the thing an experienced developer
        // notices first and reads as "this does not understand the language".
        if kind == SymbolKind::LocalVariable && scope.is_some() && reference.scope != scope {
            continue;
        }
        if kinds_alike(kind, reference.kind) && reference.name == name {
            out.push(highlight(reference.range, DocumentHighlightKind::READ));
        }
    }
    (!out.is_empty()).then_some(out)
}

pub fn prepare_rename(
    state: &State,
    params: &TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
    let (document, position) = at_cursor(state, params)?;
    let range = |range: Range| {
        PrepareRenameResponse::Range(to_lsp_range(document.text(), range, state.encoding))
    };

    // Both branches offer the *name* range, so the box the editor opens is
    // pre-filled with `score` rather than `{score::*}`.
    if let Some(symbol) = document.symbols().declaration_at(position) {
        if renameable(symbol.kind) {
            return Some(range(symbol.selection_range));
        }
    }
    let reference = document.symbols().reference_at(position)?;
    renameable(reference.kind).then(|| range(reference.name_range))
}

pub fn rename(state: &State, params: &RenameParams) -> Option<WorkspaceEdit> {
    let (document, position) = at_cursor(state, &params.text_document_position)?;
    let (kind, name, scope) = scoped_symbol_under_cursor(document, position)?;
    // `prepare_rename` already refuses these, but a client may skip it.
    if !renameable(kind) {
        return None;
    }
    let uri = document.uri();

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    let mut edit = |target: &Document, range: Range| {
        if let Ok(target_uri) = Url::parse(target.uri()) {
            changes.entry(target_uri).or_default().push(TextEdit {
                range: to_lsp_range(target.text(), range, state.encoding),
                new_text: params.new_name.clone(),
            });
        }
    };

    // Declarations and references are edited identically, because both carry
    // a range covering just the name. Reconstructing a variable's syntax from
    // a prefix instead used to write `total = 0` over `{score} = 0`, and to
    // drop the `::*` from a list variable.
    for (target, symbol) in state.workspace.definitions(kind, &name, uri) {
        edit(target, symbol.selection_range);
    }
    for (target, reference) in state.workspace.references_in_scope(kind, &name, uri, scope) {
        edit(target, reference.name_range);
    }

    Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

pub fn renameable(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::LocalFunction
            | SymbolKind::Command
            | SymbolKind::Option
            | SymbolKind::GlobalVariable
            | SymbolKind::LocalVariable
    )
}

/// Symbols that find-references and highlighting can follow.
///
/// Wider than [`renameable`] by exactly the ClassySK members. `{_w}::reset()`
/// names a method on whatever class `{_w}` holds at runtime, so two classes
/// that both declare `reset` cannot be told apart from the text. Listing both
/// as references is accurate; renaming both would silently rewrite the wrong
/// one.
fn navigable(kind: SymbolKind) -> bool {
    renameable(kind) || matches!(kind, SymbolKind::Field | SymbolKind::Method)
}

/// Whether two symbol kinds refer to the same thing for highlighting.
///
/// Mirrors `Workspace`'s own rule: a `function` and a `local function` are one
/// symbol from a lookup's point of view, as are the two variable scopes.
fn kinds_alike(wanted: SymbolKind, found: SymbolKind) -> bool {
    if wanted.is_function() && found.is_function() {
        return true;
    }
    if wanted.is_variable() && found.is_variable() {
        return wanted == found;
    }
    wanted == found
}

/// The navigable symbol under the cursor, and which trigger a local belongs
/// to.
///
/// The scope is what confines a `{_x}` rename to the trigger that owns it,
/// rather than every trigger in the file.
fn scoped_symbol_under_cursor(
    document: &Document,
    position: skript_index::Position,
) -> Option<(SymbolKind, String, Option<Range>)> {
    if let Some(symbol) = document.symbols().declaration_at(position) {
        if navigable(symbol.kind) {
            // A declaration's own scope is whatever a reference inside it
            // reports; for a parameter that is the enclosing function.
            let scope = document
                .symbols()
                .references
                .iter()
                .find(|reference| reference.kind == symbol.kind && reference.name == symbol.name)
                .and_then(|reference| reference.scope);
            return Some((symbol.kind, symbol.name.clone(), scope));
        }
    }
    let reference = document.symbols().reference_at(position)?;
    navigable(reference.kind).then(|| (reference.kind, reference.name.clone(), reference.scope))
}

/// The ClassySK class or oopsk struct whose body contains `range`.
pub fn enclosing_class(document: &Document, range: Range) -> Option<&Symbol> {
    document
        .symbols()
        .enclosing(range, |kind| kind == SymbolKind::Class)
}

/// Where a reference may be declared. For a ClassySK member whose receiver is
/// written out (`Req::get(…)`, `self::extra`), only that class's member; for
/// `{_x}::get(…)`, whose class is a runtime value, every class's.
pub fn member_definitions<'a>(
    workspace: &'a Workspace,
    from: &Document,
    reference: &Reference,
) -> Vec<(&'a Document, &'a Symbol)> {
    let found = workspace.definitions(reference.kind, &reference.name, from.uri());
    if !matches!(reference.kind, SymbolKind::Field | SymbolKind::Method) {
        return found;
    }
    let Some(receiver) = receiver_class(from, reference) else {
        return found;
    };
    found
        .into_iter()
        .filter(|(document, member)| {
            enclosing_class(document, member.range)
                .is_some_and(|class| class.name.eq_ignore_ascii_case(&receiver))
        })
        .collect()
}

/// The class a member access names: `Req` in `Req::get`, the enclosing class
/// for `self::get`. `None` when the receiver is a value.
fn receiver_class(document: &Document, reference: &Reference) -> Option<String> {
    let line = document.line(reference.range.start.line);
    let before = line
        .get(..reference.range.start.character as usize)?
        .strip_suffix("::")?;
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_alphanumeric() || *ch == '_')
        .last()
        .map_or(before.len(), |(at, _)| at);
    match &before[start..] {
        "self" => enclosing_class(document, reference.range).map(|class| class.name.clone()),
        // ClassySK class names start upper-case; a value never does here.
        name if name.starts_with(|ch: char| ch.is_ascii_uppercase()) => Some(name.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSES: &str = concat!(
        "class Req:\n",
        "\tpublic extra: integer\n",
        "\tpublic get():\n",
        "\t\treturn self::extra\n",
        "class Other:\n",
        "\tpublic extra: text\n",
        "\tpublic get():\n",
        "\t\treturn 1\n",
        "on join:\n",
        "\tReq::get()\n",
        "\t{_x}::get()\n",
    );

    fn workspace() -> Workspace {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", CLASSES);
        workspace
    }

    fn reference_on(document: &Document, line: u32, kind: SymbolKind) -> &Reference {
        document
            .symbols()
            .references
            .iter()
            .find(|reference| reference.kind == kind && reference.range.start.line == line)
            .expect("the reference is indexed")
    }

    #[test]
    fn a_named_receiver_resolves_to_its_class() {
        let workspace = workspace();
        let document = workspace.get("file:///t.sk").unwrap();
        let call = reference_on(document, 9, SymbolKind::Method);
        assert_eq!(receiver_class(document, call).as_deref(), Some("Req"));
    }

    #[test]
    fn self_resolves_to_the_enclosing_class() {
        let workspace = workspace();
        let document = workspace.get("file:///t.sk").unwrap();
        let field = reference_on(document, 3, SymbolKind::Field);
        assert_eq!(receiver_class(document, field).as_deref(), Some("Req"));
    }

    #[test]
    fn a_value_receiver_names_no_class() {
        let workspace = workspace();
        let document = workspace.get("file:///t.sk").unwrap();
        let call = reference_on(document, 10, SymbolKind::Method);
        assert_eq!(receiver_class(document, call), None);
    }

    #[test]
    fn member_definitions_narrow_to_the_named_class_only() {
        let workspace = workspace();
        let document = workspace.get("file:///t.sk").unwrap();
        let classes = |line: u32, kind: SymbolKind| -> Vec<String> {
            let reference = reference_on(document, line, kind);
            member_definitions(&workspace, document, reference)
                .into_iter()
                .filter_map(|(target, member)| enclosing_class(target, member.range))
                .map(|class| class.name.clone())
                .collect()
        };
        assert_eq!(classes(9, SymbolKind::Method), vec!["Req"]);
        assert_eq!(classes(3, SymbolKind::Field), vec!["Req"]);
        // `{_x}` could hold either, so both declarations are offered.
        let mut both = classes(10, SymbolKind::Method);
        both.sort();
        assert_eq!(both, vec!["Other", "Req"]);
    }

    fn definition_lines(state: &State, uri: &str, line: u32, character: u32) -> Vec<(String, u32)> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: tower_lsp::lsp_types::TextDocumentIdentifier {
                    uri: Url::parse(uri).unwrap(),
                },
                position: tower_lsp::lsp_types::Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        match goto_definition(state, &params) {
            Some(GotoDefinitionResponse::Array(locations)) => locations
                .into_iter()
                .map(|location| (location.uri.to_string(), location.range.start.line))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A line using skript-reflect syntax jumps to its definition, from the
    /// keyword and from inside one of its slots alike.
    #[test]
    fn a_custom_syntax_use_jumps_to_its_definition() {
        let mut state = State {
            catalog: Some(skript_docs::Catalog::build(skript_docs::fallback_docs())),
            ..Default::default()
        };
        state.workspace.open(
            "file:///lib.sk",
            "expression [the] answer:\n\tget:\n\t\treturn 42\n\neffect [the] greet %player%:\n\ttrigger:\n\t\tstop\n",
        );
        state.workspace.open(
            "file:///use.sk",
            "on join:\n\tgreet player\n\treturn the answer\n",
        );
        crate::custom_syntax::sync(&mut state);

        let lib = "file:///lib.sk".to_string();
        assert_eq!(
            definition_lines(&state, "file:///use.sk", 1, 2),
            vec![(lib.clone(), 4)]
        );
        assert_eq!(
            definition_lines(&state, "file:///use.sk", 1, 9),
            vec![(lib.clone(), 4)]
        );
        assert_eq!(
            definition_lines(&state, "file:///use.sk", 2, 10),
            vec![(lib, 0)]
        );
        // Core syntax has nowhere in the project to go.
        assert!(definition_lines(&state, "file:///use.sk", 2, 2).is_empty());
    }

    #[test]
    fn members_are_navigable_but_never_renameable() {
        assert!(navigable(SymbolKind::Method));
        assert!(!renameable(SymbolKind::Method));
        assert!(renameable(SymbolKind::LocalVariable));
        assert!(!navigable(SymbolKind::Event));
    }

    #[test]
    fn function_kinds_are_alike_but_variable_scopes_are_not() {
        assert!(kinds_alike(SymbolKind::Function, SymbolKind::LocalFunction));
        assert!(!kinds_alike(
            SymbolKind::LocalVariable,
            SymbolKind::GlobalVariable
        ));
    }
}
