//! What the server tells the client it can do.

use tower_lsp::lsp_types::*;

use crate::convert::Encoding;

pub fn server_capabilities(encoding: Encoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(encoding.kind()),
        // Saves are wanted for SkAnalyzer, which reads the file from disk; the
        // text itself is already in hand.
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
                ..Default::default()
            },
        )),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        document_formatting_provider: Some(OneOf::Left(true)),
        // Zed's "format selections". The edit may widen to whole structures;
        // see `code_actions::range_formatting`.
        document_range_formatting_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        color_provider: Some(ColorProviderCapability::Simple(true)),
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        }),
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            retrigger_characters: Some(vec![",".into()]),
            work_done_progress_options: Default::default(),
        }),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(true),
            completion_item: Some(CompletionOptionsCompletionItem {
                label_details_support: Some(true),
            }),
            // Deliberately **not** a space. Skript is written in prose, so a
            // space-triggered popup is open almost all the time, and while it
            // is open the editor gives Enter to the completion instead of to
            // the newline. Typing an ordinary sentence then fights back, which
            // is far worse than having to ask for the list.
            //
            // `{`, `@` and `%` each open something a plain word cannot
            // continue: a variable, an option reference, an interpolation.
            // Word characters still bring the list up through the editor's own
            // behaviour.
            //
            // `:` joins them for list variables: `{data::` asks for the next
            // level. Outside a variable a `:` trigger is answered with
            // nothing, so section headers never open a popup.
            trigger_characters: Some(vec!["{".into(), "@".into(), "%".into(), ":".into()]),
            ..Default::default()
        }),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: crate::semantic::legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}
