//! Whole-document decorations: colour swatches, links and semantic tokens.

use skript_index::Range;
use tower_lsp::lsp_types::{
    Color, ColorInformation, ColorPresentation, ColorPresentationParams, DocumentColorParams,
    DocumentLink, DocumentLinkParams, SemanticTokens, SemanticTokensParams, SemanticTokensResult,
    TextEdit, Url,
};

use crate::convert::{self, from_lsp_position, to_lsp_range};
use crate::server::State;
use crate::{colors, links, semantic};

/// A swatch beside every colour the script writes: `&6`, `&x&f&f…`, `<red>`,
/// `<#ff5555>`.
pub fn document_color(state: &State, params: &DocumentColorParams) -> Vec<ColorInformation> {
    let Some(document) = state.workspace.get(params.text_document.uri.as_str()) else {
        return Vec::new();
    };
    colors::document_colors(document.text())
        .into_iter()
        .map(|found| ColorInformation {
            range: to_lsp_range(
                document.text(),
                Range::on_line(found.line, found.start, found.end),
                state.encoding,
            ),
            color: Color {
                red: found.red,
                green: found.green,
                blue: found.blue,
                alpha: 1.0,
            },
        })
        .collect()
}

/// A colour picked in the editor, written back in the original's style.
pub fn color_presentation(
    state: &State,
    params: &ColorPresentationParams,
) -> Vec<ColorPresentation> {
    let Some(document) = state.workspace.get(params.text_document.uri.as_str()) else {
        return Vec::new();
    };
    let start = from_lsp_position(document.text(), params.range.start, state.encoding);
    let end = from_lsp_position(document.text(), params.range.end, state.encoding);
    let original = document
        .line(start.line)
        .get(start.character as usize..end.character as usize)
        .unwrap_or("");
    let color = params.color;
    colors::presentations(original, color.red, color.green, color.blue)
        .into_iter()
        .map(|label| ColorPresentation {
            text_edit: Some(TextEdit::new(params.range, label.clone())),
            label,
            additional_text_edits: None,
        })
        .collect()
}

/// `<link:https://…>` and bare URLs, clickable.
pub fn document_link(state: &State, params: &DocumentLinkParams) -> Option<Vec<DocumentLink>> {
    let document = state.workspace.get(params.text_document.uri.as_str())?;
    let links = links::document_links(document.text())
        .into_iter()
        .filter_map(|link| {
            let range = Range::on_line(link.line, link.start, link.end);
            Some(DocumentLink {
                range: to_lsp_range(document.text(), range, state.encoding),
                target: Some(Url::parse(&link.target).ok()?),
                tooltip: None,
                data: None,
            })
        })
        .collect();
    Some(links)
}

pub fn semantic_tokens_full(
    state: &State,
    params: &SemanticTokensParams,
) -> Option<SemanticTokensResult> {
    let document = state.workspace.get(params.text_document.uri.as_str())?;
    let catalog = state.catalog.as_ref()?;

    let text = document.text();
    let encoding = state.encoding;
    // Indexed once for the whole response rather than rescanned per token.
    let lines = convert::LineIndex::new(text);
    let data = semantic::tokens(catalog, text, document.symbols(), |line, byte| {
        lines.to_column(line, byte, encoding)
    });

    Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data,
    }))
}
