//! The outline, workspace symbol search and folding.

use skript_index::SymbolKind;
use tower_lsp::lsp_types::{
    self as lsp, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, FoldingRange,
    FoldingRangeKind, FoldingRangeParams, Location, SymbolInformation, Url, WorkspaceSymbolParams,
};

use crate::convert::{self, to_lsp_range, Encoding};
use crate::server::State;

pub fn document_symbol(
    state: &State,
    params: &DocumentSymbolParams,
) -> Option<DocumentSymbolResponse> {
    let document = state.workspace.get(params.text_document.uri.as_str())?;
    let lines = convert::LineIndex::new(document.text());
    let symbols = document
        .symbols()
        .symbols
        .iter()
        .map(|symbol| to_document_symbol(symbol, &lines, state.encoding))
        .collect();
    Some(DocumentSymbolResponse::Nested(symbols))
}

pub fn workspace_symbol(state: &State, params: &WorkspaceSymbolParams) -> Vec<SymbolInformation> {
    state
        .workspace
        .workspace_symbols(&params.query)
        .into_iter()
        .filter_map(|found| {
            let (document, symbol) = (found.document, found.symbol);
            #[allow(deprecated)]
            Some(SymbolInformation {
                name: symbol.name.clone(),
                kind: lsp_symbol_kind(symbol.kind),
                tags: None,
                deprecated: None,
                location: Location {
                    uri: Url::parse(document.uri()).ok()?,
                    range: to_lsp_range(document.text(), symbol.selection_range, state.encoding),
                },
                container_name: Some(found.container),
            })
        })
        .collect()
}

pub fn folding_range(state: &State, params: &FoldingRangeParams) -> Option<Vec<FoldingRange>> {
    let document = state.workspace.get(params.text_document.uri.as_str())?;
    let ranges = document
        .folding_ranges()
        .into_iter()
        .map(|fold| FoldingRange {
            start_line: fold.start_line,
            end_line: fold.end_line,
            kind: Some(match fold.kind {
                skript_index::folding::FoldKind::Comment => FoldingRangeKind::Comment,
                skript_index::folding::FoldKind::Region => FoldingRangeKind::Region,
            }),
            collapsed_text: Some(fold.collapsed_text),
            ..Default::default()
        })
        .collect();
    Some(ranges)
}

pub fn lsp_symbol_kind(kind: SymbolKind) -> lsp::SymbolKind {
    use lsp::SymbolKind as Lsp;
    match kind {
        SymbolKind::Event => Lsp::EVENT,
        SymbolKind::Command | SymbolKind::Method => Lsp::METHOD,
        SymbolKind::Entry => Lsp::PROPERTY,
        SymbolKind::Function | SymbolKind::LocalFunction => Lsp::FUNCTION,
        SymbolKind::Option | SymbolKind::Alias => Lsp::CONSTANT,
        SymbolKind::GlobalVariable | SymbolKind::LocalVariable => Lsp::VARIABLE,
        SymbolKind::Section => Lsp::NAMESPACE,
        SymbolKind::Structure => Lsp::MODULE,
        SymbolKind::Class => Lsp::CLASS,
        SymbolKind::Field => Lsp::FIELD,
        SymbolKind::Syntax => Lsp::INTERFACE,
    }
}

#[allow(deprecated)]
fn to_document_symbol(
    symbol: &skript_index::Symbol,
    lines: &convert::LineIndex<'_>,
    encoding: Encoding,
) -> DocumentSymbol {
    DocumentSymbol {
        name: symbol.name.clone(),
        detail: (!symbol.detail.is_empty()).then(|| symbol.detail.clone()),
        kind: lsp_symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        range: lines.to_lsp_range(symbol.range, encoding),
        selection_range: lines.to_lsp_range(symbol.selection_range, encoding),
        children: Some(
            symbol
                .children
                .iter()
                .map(|child| to_document_symbol(child, lines, encoding))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_kind_maps_to_an_lsp_kind() {
        // A missing arm would be a compile error; this pins the intent that
        // commands read as methods and structures as modules in the outline.
        assert_eq!(
            lsp_symbol_kind(SymbolKind::Command),
            lsp::SymbolKind::METHOD
        );
        assert_eq!(
            lsp_symbol_kind(SymbolKind::Function),
            lsp::SymbolKind::FUNCTION
        );
        assert_eq!(
            lsp_symbol_kind(SymbolKind::Alias),
            lsp::SymbolKind::CONSTANT
        );
    }
}
