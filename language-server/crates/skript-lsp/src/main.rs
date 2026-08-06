//! `skript-lsp` — a language server for Skript.
//!
//! Speaks LSP 3.17 over **stdio**, which is what Zed (and every other editor)
//! expects. Nothing is written to stdout except protocol traffic; logs go to
//! stderr, because a stray `println!` corrupts the message stream.
//!
//! The server is built in layers so that each one degrades independently:
//!
//! * `skript-index` parses the open files and finds what they declare and use.
//!   It needs nothing external, so outline, folding, go-to-definition,
//!   find-references and rename work offline and immediately.
//! * `skript-docs` supplies meaning — hover text, completion and the
//!   effect/condition/expression classification behind semantic tokens. It is
//!   downloaded at runtime; if that fails, a small built-in catalog takes over
//!   and everything above still works.

mod convert;
mod diagnostics;
mod semantic;

use std::sync::Arc;

use skript_addons::Detection;
use skript_docs::{Catalog, Category, DocsSource};
use skript_index::{SymbolKind, Workspace};
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as RpcResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use convert::{from_lsp_position, to_lsp_range, Encoding};

struct Backend {
    client: Client,
    state: Arc<RwLock<State>>,
}

struct State {
    workspace: Workspace,
    catalog: Option<Catalog>,
    /// Syntax belonging to addons the server does **not** have installed.
    ///
    /// Kept apart from `catalog` so it never reaches completion or semantic
    /// tokens — its only job is to let a diagnostic say "that line is SkBee
    /// syntax, and SkBee is not installed". Only built when a `plugins/`
    /// directory was actually found.
    uninstalled: Option<Catalog>,
    detection: Detection,
    encoding: Encoding,
    diagnostics: diagnostics::Options,
}

impl Default for State {
    fn default() -> Self {
        Self {
            workspace: Workspace::new(),
            catalog: None,
            uninstalled: None,
            detection: Detection::default(),
            encoding: Encoding::Utf16,
            diagnostics: diagnostics::Options::default(),
        }
    }
}

/// User-facing settings, passed through Zed's
/// `lsp.skript-lsp.initialization_options`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    /// Pin the syntax database to a specific Skript version.
    skript_version: Option<String>,
    /// A `docs.json` generated on your own server, to match its exact Skript
    /// build. Note this covers Skript itself only: `/sk gen-docs` is scoped to
    /// one addon, so it never contains a third-party addon's syntax.
    docs_path: Option<String>,
    /// Fetch the database from somewhere other than docs.skriptlang.org.
    docs_url: Option<String>,
    /// Report lines that match no known syntax. Off by default.
    unknown_syntax_diagnostics: bool,
    /// Report syntax upstream has marked deprecated. On by default.
    #[serde(default = "default_true")]
    deprecated_syntax_diagnostics: bool,

    /// Which addons to load syntax for.
    ///
    /// `"auto"` reads the plugin manifests in the project's `plugins/`
    /// directory, `"off"` loads none, and a list names them explicitly.
    #[serde(default)]
    addons: AddonSetting,

    /// Where `plugins/` lives, when it is not at or above the workspace root.
    #[serde(default)]
    server_path: Option<String>,

    /// Where addon syntax comes from. `"skripthub"` or `"off"`.
    #[serde(default = "default_addon_source")]
    addon_syntax_source: String,

    /// Extra syntax files, in either Skript's or SkriptHub's schema. The escape
    /// hatch for private or unpublished addons.
    #[serde(default)]
    custom_syntax_paths: Vec<String>,
}

/// How addons are chosen.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
enum AddonSetting {
    /// `"auto"` or `"off"`.
    Mode(String),
    /// An explicit list of addon names.
    Named(Vec<String>),
}

impl Default for AddonSetting {
    fn default() -> Self {
        AddonSetting::Mode("auto".to_string())
    }
}

impl AddonSetting {
    fn is_off(&self) -> bool {
        matches!(self, AddonSetting::Mode(mode) if mode.eq_ignore_ascii_case("off"))
    }

    /// The explicitly named addons, if the user listed them.
    fn explicit(&self) -> Option<&[String]> {
        match self {
            AddonSetting::Named(names) => Some(names),
            _ => None,
        }
    }
}

fn default_addon_source() -> String {
    "skripthub".to_string()
}

fn default_true() -> bool {
    true
}

impl Settings {
    fn docs_source(&self) -> DocsSource {
        if let Some(path) = &self.docs_path {
            return DocsSource::Local(path.into());
        }
        if let Some(url) = &self.docs_url {
            return DocsSource::Url(url.clone());
        }
        match &self.skript_version {
            Some(version) => DocsSource::Version(version.clone()),
            None => DocsSource::Latest,
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> RpcResult<InitializeResult> {
        let settings: Settings = params
            .initialization_options
            .clone()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();

        let encoding = Encoding::negotiate(
            params
                .capabilities
                .general
                .as_ref()
                .and_then(|general| general.position_encodings.as_deref()),
        );

        {
            let mut state = self.state.write().await;
            state.encoding = encoding;
            state.diagnostics = diagnostics::Options {
                unknown_syntax: settings.unknown_syntax_diagnostics,
                deprecated_syntax: settings.deprecated_syntax_diagnostics,
            };
        }

        // The workspace roots serve two purposes: finding the project's
        // `plugins/` directory, and indexing its scripts.
        let roots: Vec<std::path::PathBuf> = params
            .workspace_folders
            .map(|folders| {
                folders
                    .into_iter()
                    .filter_map(|folder| folder.uri.to_file_path().ok())
                    .collect()
            })
            .unwrap_or_default();

        // Loading the catalog blocks on the network, so it happens off the
        // request path: the editor becomes usable immediately and gains hover
        // and completion a moment later.
        let state = self.state.clone();
        let client = self.client.clone();
        let scan_roots: Vec<std::path::PathBuf> = settings
            .server_path
            .as_ref()
            .map(|path| vec![std::path::PathBuf::from(path)])
            .unwrap_or_else(|| roots.clone());

        tokio::spawn(async move {
            let loaded =
                tokio::task::spawn_blocking(move || load_everything(&settings, &scan_roots)).await;
            match loaded {
                Ok(loaded) => {
                    for message in &loaded.messages {
                        client.log_message(MessageType::INFO, message).await;
                    }
                    let mut state = state.write().await;
                    state.catalog = Some(loaded.catalog);
                    state.uninstalled = loaded.uninstalled;
                    state.detection = loaded.detection;
                }
                Err(error) => {
                    client
                        .log_message(
                            MessageType::ERROR,
                            format!("loading the Skript syntax database panicked: {error}"),
                        )
                        .await;
                }
            }
        });

        // Index every script in the project, not only the files that happen to
        // be open. Without this, go-to-definition and find-references silently
        // miss most of a real script folder.
        if !roots.is_empty() {
            let state = self.state.clone();
            let client = self.client.clone();
            tokio::spawn(async move {
                let scripts = tokio::task::spawn_blocking(move || {
                    let mut found = Vec::new();
                    for root in &roots {
                        for path in skript_index::discover_scripts(root) {
                            let Ok(text) = std::fs::read_to_string(&path) else {
                                continue;
                            };
                            let Ok(uri) = Url::from_file_path(&path) else {
                                continue;
                            };
                            found.push((uri.to_string(), text));
                        }
                    }
                    found
                })
                .await
                .unwrap_or_default();

                let count = scripts.len();
                {
                    let mut state = state.write().await;
                    for (uri, text) in scripts {
                        // An open document is authoritative - it may carry
                        // unsaved edits the file on disk does not.
                        if state.workspace.get(&uri).is_none() {
                            state.workspace.open(uri, text);
                        }
                    }
                }
                client
                    .log_message(MessageType::INFO, format!("indexed {count} script(s)"))
                    .await;
            });
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "skript-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                position_encoding: Some(encoding.kind()),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
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
                document_highlight_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".into(), ",".into()]),
                    retrigger_characters: Some(vec![",".into()]),
                    work_done_progress_options: Default::default(),
                }),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(true),
                    trigger_characters: Some(vec!["{".into(), "@".into(), "%".into(), " ".into()]),
                    ..Default::default()
                }),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: semantic::legend(),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..Default::default()
                        },
                    ),
                ),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "skript-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        self.state
            .write()
            .await
            .workspace
            .open(uri.clone(), params.text_document.text);
        self.publish(&uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        // Sync is FULL, so the last change carries the whole document.
        if let Some(change) = params.content_changes.into_iter().next_back() {
            self.state.write().await.workspace.update(&uri, change.text);
        }
        self.publish(&uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();

        // Closing a tab must not remove the file from the project index. The
        // same `Workspace` holds every `.sk` found on disk at startup, so
        // evicting the entry made functions in that file "undefined" for every
        // other open script the moment its tab was closed.
        //
        // Re-reading from disk also discards any unsaved edits, which is
        // exactly right: the buffer is gone, the file is what remains.
        let on_disk = params
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok());

        {
            let mut state = self.state.write().await;
            match on_disk {
                Some(text) => state.workspace.update(&uri, text),
                None => state.workspace.close(&uri),
            }
        }
        // Clear the file's diagnostics, or they linger in the problems panel.
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> RpcResult<Option<DocumentSymbolResponse>> {
        let state = self.state.read().await;
        let uri = params.text_document.uri.to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };

        let lines = convert::LineIndex::new(document.text());
        let symbols = document
            .symbols()
            .symbols
            .iter()
            .map(|symbol| to_document_symbol(symbol, &lines, state.encoding))
            .collect();

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> RpcResult<Option<Vec<SymbolInformation>>> {
        let state = self.state.read().await;
        let mut out = Vec::new();

        for (document, symbol) in state.workspace.workspace_symbols(&params.query) {
            let Ok(uri) = Url::parse(document.uri()) else {
                continue;
            };
            #[allow(deprecated)]
            out.push(SymbolInformation {
                name: symbol.name.clone(),
                kind: lsp_symbol_kind(symbol.kind),
                tags: None,
                deprecated: None,
                location: Location {
                    uri,
                    range: to_lsp_range(document.text(), symbol.selection_range, state.encoding),
                },
                container_name: None,
            });
        }

        Ok(Some(out))
    }

    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> RpcResult<Option<Vec<FoldingRange>>> {
        let state = self.state.read().await;
        let Some(document) = state.workspace.get(params.text_document.uri.as_ref()) else {
            return Ok(None);
        };

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

        Ok(Some(ranges))
    }

    async fn hover(&self, params: HoverParams) -> RpcResult<Option<Hover>> {
        let state = self.state.read().await;
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };

        let position = from_lsp_position(
            document.text(),
            params.text_document_position_params.position,
            state.encoding,
        );

        // A declaration or reference the cursor sits on wins: it is the more
        // specific answer, and it works without the catalog.
        if let Some(symbol) = document.symbols().declaration_at(position) {
            let mut text = format!("**{}**", symbol.name);
            if !symbol.detail.is_empty() {
                text.push_str(&format!("  \n`{}`", symbol.detail));
            }
            return Ok(Some(Hover {
                contents: HoverContents::Markup(markdown(text)),
                range: Some(to_lsp_range(
                    document.text(),
                    symbol.selection_range,
                    state.encoding,
                )),
            }));
        }

        // Otherwise describe the syntax the whole line resolves to.
        let Some(catalog) = &state.catalog else {
            return Ok(None);
        };
        let line = document.line(position.line);
        let code = line.trim().trim_end_matches(':');
        // Same role filter as semantic tokens and diagnostics — hovering a line
        // must never claim it is the "Creature/Entity/Player/…" expression just
        // because that pattern is `[the] [event-]<.+>` and matches anything.
        let role = skript_docs::LineRole::from_indent(line.len() - line.trim_start().len());
        let Some((id, _)) = catalog.classify_line(code, role) else {
            return Ok(None);
        };
        let Some(mut text) = catalog.hover(id) else {
            return Ok(None);
        };

        // Availability is the first thing a reader wants and the last thing the
        // rendered card carries, so it goes at the top.
        let mut notes = Vec::new();
        if let Some(note) = catalog.availability(id) {
            notes.push(format!("⚠️ {note}"));
        }
        if let Some(entry) = catalog.entry(id) {
            if let Some(addon) = &entry.addon {
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
                    " — not installed"
                } else {
                    ""
                };
                notes.push(format!("from {label}{version}{state_note}"));
            }
        }
        if !notes.is_empty() {
            text = format!("{}\n\n{text}", notes.join(" · "));
        }

        Ok(Some(Hover {
            contents: HoverContents::Markup(markdown(text)),
            range: None,
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> RpcResult<Option<GotoDefinitionResponse>> {
        let state = self.state.read().await;
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };

        let position = from_lsp_position(
            document.text(),
            params.text_document_position_params.position,
            state.encoding,
        );
        let Some(reference) = document.symbols().reference_at(position) else {
            return Ok(None);
        };

        let mut locations = Vec::new();
        for (target, symbol) in state
            .workspace
            .definitions(reference.kind, &reference.name, &uri)
        {
            if let Ok(target_uri) = Url::parse(target.uri()) {
                locations.push(Location {
                    uri: target_uri,
                    range: to_lsp_range(target.text(), symbol.selection_range, state.encoding),
                });
            }
        }

        Ok((!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations)))
    }

    async fn references(&self, params: ReferenceParams) -> RpcResult<Option<Vec<Location>>> {
        let state = self.state.read().await;
        let uri = params.text_document_position.text_document.uri.to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };

        let position = from_lsp_position(
            document.text(),
            params.text_document_position.position,
            state.encoding,
        );
        let Some((kind, name)) = symbol_under_cursor(document, position) else {
            return Ok(None);
        };

        let mut locations = Vec::new();
        for (target, reference) in state.workspace.references(kind, &name, &uri) {
            if let Ok(target_uri) = Url::parse(target.uri()) {
                locations.push(Location {
                    uri: target_uri,
                    range: to_lsp_range(target.text(), reference.range, state.encoding),
                });
            }
        }
        if params.context.include_declaration {
            for (target, symbol) in state.workspace.definitions(kind, &name, &uri) {
                if let Ok(target_uri) = Url::parse(target.uri()) {
                    locations.push(Location {
                        uri: target_uri,
                        range: to_lsp_range(target.text(), symbol.selection_range, state.encoding),
                    });
                }
            }
        }

        Ok(Some(locations))
    }

    /// Highlights every other use of the symbol under the cursor.
    ///
    /// Restricted to the current file by definition of the request, so it does
    /// not need the workspace scope rules — but it does need the same
    /// `symbol_under_cursor` resolution as go-to-definition, or the editor would
    /// highlight a different symbol than F12 navigates to.
    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> RpcResult<Option<Vec<DocumentHighlight>>> {
        let state = self.state.read().await;
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };

        let position = from_lsp_position(
            document.text(),
            params.text_document_position_params.position,
            state.encoding,
        );
        let Some((kind, name)) = symbol_under_cursor(document, position) else {
            return Ok(None);
        };

        let mut out = Vec::new();
        for symbol in document.symbols().flat() {
            if kinds_alike(kind, symbol.kind) && symbol.name == name {
                out.push(DocumentHighlight {
                    range: to_lsp_range(document.text(), symbol.selection_range, state.encoding),
                    kind: Some(DocumentHighlightKind::WRITE),
                });
            }
        }
        for reference in &document.symbols().references {
            if kinds_alike(kind, reference.kind) && reference.name == name {
                out.push(DocumentHighlight {
                    range: to_lsp_range(document.text(), reference.range, state.encoding),
                    kind: Some(DocumentHighlightKind::READ),
                });
            }
        }

        Ok((!out.is_empty()).then_some(out))
    }

    /// Parameter-name hints at function call sites.
    ///
    /// Skript's call syntax carries no argument names, so `giveKit(p, 3, true)`
    /// is unreadable without opening the declaration. This is the one place the
    /// index already knows something the source does not show.
    async fn inlay_hint(&self, params: InlayHintParams) -> RpcResult<Option<Vec<InlayHint>>> {
        let state = self.state.read().await;
        let uri = params.text_document.uri.to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };

        let from = from_lsp_position(document.text(), params.range.start, state.encoding);
        let to = from_lsp_position(document.text(), params.range.end, state.encoding);

        let mut hints = Vec::new();
        for reference in &document.symbols().references {
            if reference.kind != SymbolKind::Function {
                continue;
            }
            let line = reference.range.start.line;
            if line < from.line || line > to.line {
                continue;
            }

            // The declaration is the only source of parameter names. A call to
            // an unknown function gets no hints rather than invented ones.
            let Some((target, symbol)) = state
                .workspace
                .definitions(SymbolKind::Function, &reference.name, &uri)
                .into_iter()
                .next()
            else {
                continue;
            };
            let names = parameter_names(&symbol.detail);
            if names.is_empty() {
                continue;
            }
            let _ = target;

            let text = document.text();
            for (index, offset) in argument_offsets(text, line, reference.range.end.character)
                .into_iter()
                .enumerate()
            {
                let Some(name) = names.get(index) else { break };
                hints.push(InlayHint {
                    position: convert::to_lsp_position(
                        text,
                        skript_index::Position::new(line, offset),
                        state.encoding,
                    ),
                    label: InlayHintLabel::String(format!("{name}:")),
                    kind: Some(InlayHintKind::PARAMETER),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(false),
                    padding_right: Some(true),
                    data: None,
                });
            }
        }

        Ok(Some(hints))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> RpcResult<Option<PrepareRenameResponse>> {
        let state = self.state.read().await;
        let Some(document) = state.workspace.get(params.text_document.uri.as_ref()) else {
            return Ok(None);
        };
        let position = from_lsp_position(document.text(), params.position, state.encoding);

        // Both branches offer the *name* range, so the box the editor opens is
        // pre-filled with `score` rather than `{score::*}`.
        if let Some(symbol) = document.symbols().declaration_at(position) {
            if renameable(symbol.kind) {
                return Ok(Some(PrepareRenameResponse::Range(to_lsp_range(
                    document.text(),
                    symbol.selection_range,
                    state.encoding,
                ))));
            }
        }
        if let Some(reference) = document.symbols().reference_at(position) {
            if renameable(reference.kind) {
                return Ok(Some(PrepareRenameResponse::Range(to_lsp_range(
                    document.text(),
                    reference.name_range,
                    state.encoding,
                ))));
            }
        }
        Ok(None)
    }

    async fn rename(&self, params: RenameParams) -> RpcResult<Option<WorkspaceEdit>> {
        let state = self.state.read().await;
        let uri = params.text_document_position.text_document.uri.to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };
        let position = from_lsp_position(
            document.text(),
            params.text_document_position.position,
            state.encoding,
        );
        let Some((kind, name)) = symbol_under_cursor(document, position) else {
            return Ok(None);
        };

        let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> = Default::default();

        // Declarations and references are edited identically, because both now
        // carry a range covering just the name. Reconstructing a variable's
        // syntax from a prefix instead used to write `total = 0` over
        // `{score} = 0`, and to drop the `::*` from a list variable.
        for (target, symbol) in state.workspace.definitions(kind, &name, &uri) {
            if let Ok(target_uri) = Url::parse(target.uri()) {
                changes.entry(target_uri).or_default().push(TextEdit {
                    range: to_lsp_range(target.text(), symbol.selection_range, state.encoding),
                    new_text: params.new_name.clone(),
                });
            }
        }

        for (target, reference) in state.workspace.references(kind, &name, &uri) {
            if let Ok(target_uri) = Url::parse(target.uri()) {
                changes.entry(target_uri).or_default().push(TextEdit {
                    range: to_lsp_range(target.text(), reference.name_range, state.encoding),
                    new_text: params.new_name.clone(),
                });
            }
        }

        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }))
    }

    async fn completion(&self, params: CompletionParams) -> RpcResult<Option<CompletionResponse>> {
        let state = self.state.read().await;
        let uri = params.text_document_position.text_document.uri.to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };
        let position = from_lsp_position(
            document.text(),
            params.text_document_position.position,
            state.encoding,
        );

        let line = document.line(position.line);
        let prefix = &line[..(position.character as usize).min(line.len())];
        let mut items = Vec::new();

        // Declared symbols always come first: they are the user's own code.
        //
        // Scoped to what Skript would actually resolve from this file. Using the
        // project-wide symbol list here offered another script's `{_local}`
        // variables and options in every trigger — names that cannot resolve, so
        // accepting one produced code that silently did nothing.
        let mut seen = std::collections::HashSet::new();
        for (_, symbol) in state.workspace.symbols_in_scope(&uri) {
            let kind = match symbol.kind {
                SymbolKind::Function | SymbolKind::LocalFunction => CompletionItemKind::FUNCTION,
                SymbolKind::Command => CompletionItemKind::METHOD,
                SymbolKind::Option => CompletionItemKind::CONSTANT,
                SymbolKind::GlobalVariable | SymbolKind::LocalVariable => {
                    CompletionItemKind::VARIABLE
                }
                _ => continue,
            };
            // The same global is declared in as many files as assign to it.
            if !seen.insert((symbol.kind, symbol.name.clone())) {
                continue;
            }
            items.push(CompletionItem {
                label: symbol.name.clone(),
                kind: Some(kind),
                detail: (!symbol.detail.is_empty()).then(|| symbol.detail.clone()),
                ..Default::default()
            });
        }

        if let Some(catalog) = &state.catalog {
            // Context narrows the categories that can possibly be valid here,
            // which is the difference between a useful list and 1,208 entries.
            for category in categories_for(prefix) {
                for (id, entry) in catalog.search(category, "") {
                    let Some(pattern) = entry.patterns.first() else {
                        continue;
                    };
                    // Where it comes from matters more than the category when
                    // several addons are loaded, so it leads the detail line.
                    let mut detail = match &entry.addon {
                        Some(addon) => format!("{} · {}", addon.name, category.label()),
                        None => category.label().to_string(),
                    };
                    detail.push_str(&format!(" · {pattern}"));

                    // Syntax the target Skript cannot run is labelled and sunk,
                    // never hidden: a quarter of `since` values are free text,
                    // and hiding working syntax on a misparse is the worse
                    // failure.
                    let availability = catalog.availability(id);
                    if let Some(note) = &availability {
                        detail = format!("{note} · {detail}");
                    }

                    // Core Skript first, then addons, then anything the target
                    // version cannot run.
                    let rank = match (&entry.addon, availability.is_some()) {
                        (_, true) => 3,
                        (None, false) => 0,
                        (Some(_), false) => 1,
                    };

                    items.push(CompletionItem {
                        label: entry.name.clone(),
                        kind: Some(CompletionItemKind::SNIPPET),
                        detail: Some(detail),
                        sort_text: Some(format!("{rank}{}", entry.name)),
                        // Rendering every entry's Markdown card here meant
                        // thousands of documents rebuilt on each keystroke —
                        // and " " is a completion trigger, so that was every
                        // space typed. The client asks for the one item it
                        // actually shows via `completionItem/resolve`.
                        data: Some(serde_json::json!([category.label(), id.index])),
                        insert_text: Some(snippet_for(pattern)),
                        insert_text_format: Some(InsertTextFormat::SNIPPET),
                        tags: entry
                            .is_deprecated()
                            .then(|| vec![CompletionItemTag::DEPRECATED]),
                        ..Default::default()
                    });
                }
            }
        }

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> RpcResult<Option<Vec<TextEdit>>> {
        let state = self.state.read().await;
        let Some(document) = state.workspace.get(params.text_document.uri.as_ref()) else {
            return Ok(None);
        };

        let options = skript_format::Options {
            hard_tabs: !params.options.insert_spaces,
            tab_size: params.options.tab_size as usize,
            ..Default::default()
        };

        let Some(formatted) = skript_format::format(document, options) else {
            // Already formatted, or the file does not parse - either way,
            // changing nothing is the right answer.
            return Ok(None);
        };

        // One edit spanning the document. Skript's indentation is its syntax,
        // so partial edits applied out of order could change which block a line
        // belongs to.
        //
        // The end is the start of the line *after* the last one, which for a
        // document of N lines is line N. Going further (N + 1) is out of range,
        // and a client that does not clamp rejects the edit outright — so
        // formatting would silently do nothing.
        let line_count = document.text().lines().count() as u32;
        Ok(Some(vec![TextEdit {
            range: Range {
                start: Position::new(0, 0),
                end: Position::new(line_count, 0),
            },
            new_text: formatted,
        }]))
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> RpcResult<Option<SignatureHelp>> {
        let state = self.state.read().await;
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let Some(document) = state.workspace.get(&uri) else {
            return Ok(None);
        };

        let position = from_lsp_position(
            document.text(),
            params.text_document_position_params.position,
            state.encoding,
        );
        let line = document.line(position.line);
        let prefix = &line[..(position.character as usize).min(line.len())];

        let Some((name, argument)) = enclosing_call(prefix) else {
            return Ok(None);
        };

        let definitions = state
            .workspace
            .definitions(SymbolKind::Function, &name, &uri);
        let Some((_, symbol)) = definitions.first() else {
            return Ok(None);
        };

        let parameters: Vec<ParameterInformation> = symbol
            .detail
            .trim_start_matches('(')
            .split(')')
            .next()
            .unwrap_or("")
            .split(',')
            .filter(|part| !part.trim().is_empty())
            .map(|part| ParameterInformation {
                label: ParameterLabel::Simple(part.trim().to_string()),
                documentation: None,
            })
            .collect();

        Ok(Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: format!("{name}{}", symbol.detail),
                documentation: None,
                parameters: Some(parameters),
                active_parameter: Some(argument),
            }],
            active_signature: Some(0),
            active_parameter: Some(argument),
        }))
    }

    async fn completion_resolve(&self, mut item: CompletionItem) -> RpcResult<CompletionItem> {
        // Only the item the user is actually looking at gets a rendered card.
        let Some(data) = item.data.take() else {
            return Ok(item);
        };
        let state = self.state.read().await;
        let Some(catalog) = &state.catalog else {
            return Ok(item);
        };

        let parsed: Option<(String, usize)> = serde_json::from_value(data).ok();
        let Some((label, index)) = parsed else {
            return Ok(item);
        };
        let Some(category) = Category::from_label(&label) else {
            return Ok(item);
        };

        let id = skript_docs::EntryId { category, index };
        if let Some(entry) = catalog.entry(id) {
            item.documentation = Some(Documentation::MarkupContent(markdown(
                skript_docs::hover::render(category, entry),
            )));
        }
        Ok(item)
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> RpcResult<Option<SemanticTokensResult>> {
        let state = self.state.read().await;
        let Some(document) = state.workspace.get(params.text_document.uri.as_ref()) else {
            return Ok(None);
        };
        let Some(catalog) = &state.catalog else {
            return Ok(None);
        };

        let text = document.text();
        let encoding = state.encoding;
        // Indexed once for the whole response rather than rescanned per token.
        let lines = convert::LineIndex::new(text);
        let data = semantic::tokens(catalog, text, document.symbols(), |line, byte| {
            lines.to_column(line, byte, encoding)
        });

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }
}

impl Backend {
    async fn publish(&self, uri: &str) {
        let (found, encoding) = {
            let state = self.state.read().await;
            let Some(document) = state.workspace.get(uri) else {
                return;
            };
            let found = diagnostics::check(
                document,
                &state.workspace,
                state.catalog.as_ref(),
                // Only offered when a `plugins/` directory was found: without
                // one we do not know what is installed, and saying an addon is
                // missing would be a guess.
                state
                    .detection
                    .is_known()
                    .then_some(())
                    .and(state.uninstalled.as_ref()),
                state.diagnostics,
            );
            let text = document.text().to_string();
            let converted: Vec<Diagnostic> = found
                .into_iter()
                .map(|diagnostic| Diagnostic {
                    range: to_lsp_range(&text, diagnostic.range, state.encoding),
                    severity: Some(match diagnostic.severity {
                        diagnostics::Severity::Error => DiagnosticSeverity::ERROR,
                        diagnostics::Severity::Warning => DiagnosticSeverity::WARNING,
                        diagnostics::Severity::Hint => DiagnosticSeverity::HINT,
                    }),
                    code: Some(NumberOrString::String(diagnostic.code.to_string())),
                    source: Some("skript".into()),
                    message: diagnostic.message,
                    ..Default::default()
                })
                .collect();
            (converted, state.encoding)
        };
        let _ = encoding;

        if let Ok(parsed) = Url::parse(uri) {
            self.client.publish_diagnostics(parsed, found, None).await;
        }
    }
}

// ------------------------------------------------------------------ helpers

fn markdown(value: String) -> MarkupContent {
    MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    }
}

fn renameable(kind: SymbolKind) -> bool {
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

/// Parameter names out of a function's rendered signature.
///
/// The index stores the signature as text — `(who: player, amount: number = 1)
/// :: text` — because Skript has no type model worth building one for. Reading
/// the names back out is cheaper than threading a structured parameter list
/// through the whole index for this one feature.
fn parameter_names(detail: &str) -> Vec<String> {
    let Some(open) = detail.find('(') else {
        return Vec::new();
    };
    // The *matching* close paren, not the first one: a parenthesised default
    // such as `xs: integers = (1, 7)` closes before the parameter list does, and
    // stopping there silently truncated every parameter after it.
    let mut depth = 0usize;
    let mut close = None;
    for (offset, ch) in detail[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return Vec::new();
    };
    let inside = &detail[open + 1..close];

    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for ch in inside.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            // Only a top-level comma separates parameters; one inside a
            // parenthesised default such as `= (1, 7)` does not.
            ',' if depth == 0 => {
                names.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    names.push(current);

    names
        .into_iter()
        .map(|part| {
            part.split(':')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// Byte columns at which each argument of a call starts.
///
/// `after_name` is the column just past the function name, so the `(` is the
/// next thing on the line. Strings and nested parens are skipped so that a comma
/// inside `"a, b"` or inside `f(1, 2)` does not split an argument.
fn argument_offsets(text: &str, line: u32, after_name: u32) -> Vec<u32> {
    let Some(source) = text.lines().nth(line as usize) else {
        return Vec::new();
    };
    let bytes = source.as_bytes();
    let mut index = after_name as usize;

    while index < bytes.len() && bytes[index] != b'(' {
        // Anything other than whitespace between the name and the paren means
        // this is not the call we were told it is.
        if !bytes[index].is_ascii_whitespace() {
            return Vec::new();
        }
        index += 1;
    }
    if index >= bytes.len() {
        return Vec::new();
    }
    index += 1;

    let mut offsets = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut expecting = true;

    while index < bytes.len() {
        let ch = bytes[index];
        if in_string {
            if ch == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match ch {
            b'"' => in_string = true,
            b'(' | b'[' | b'{' => depth += 1,
            b')' if depth == 0 => break,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => expecting = true,
            _ => {}
        }
        if expecting && !ch.is_ascii_whitespace() && !matches!(ch, b',') {
            offsets.push(index as u32);
            expecting = false;
        }
        index += 1;
    }

    offsets
}

fn symbol_under_cursor(
    document: &skript_index::Document,
    position: skript_index::Position,
) -> Option<(SymbolKind, String)> {
    if let Some(symbol) = document.symbols().declaration_at(position) {
        if renameable(symbol.kind) {
            return Some((symbol.kind, symbol.name.clone()));
        }
    }
    let reference = document.symbols().reference_at(position)?;
    renameable(reference.kind).then(|| (reference.kind, reference.name.clone()))
}

/// Finds the function call the cursor is inside, and which argument it is on.
///
/// Scans backwards counting parentheses, so a nested call or a `(` inside a
/// string does not confuse it.
fn enclosing_call(prefix: &str) -> Option<(String, u32)> {
    let bytes = prefix.as_bytes();
    let mut depth = 0i32;
    let mut commas = 0u32;
    let mut in_string = false;
    let mut index = bytes.len();

    while index > 0 {
        index -= 1;
        match bytes[index] {
            b'"' => in_string = !in_string,
            _ if in_string => {}
            b')' => depth += 1,
            b',' if depth == 0 => commas += 1,
            b'(' => {
                if depth == 0 {
                    // The identifier immediately before the paren names the call.
                    //
                    // Walking chars rather than adding 1 to `rfind`'s byte
                    // offset: a multi-byte delimiter such as `§` or an em dash
                    // would otherwise leave `start` mid-character and panic on
                    // the slice.
                    let head = &prefix[..index];
                    let start = head
                        .char_indices()
                        .rev()
                        .find(|(_, ch)| !(ch.is_alphanumeric() || *ch == '_'))
                        .map(|(at, ch)| at + ch.len_utf8())
                        .unwrap_or(0);
                    let name = &head[start..];
                    return (!name.is_empty()).then(|| (name.to_string(), commas));
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Which categories can appear at this point in the line.
///
/// This is what makes completion usable: after `on `, only events are possible;
/// inside an `if`, only conditions; at the start of a line inside a trigger,
/// only effects and sections.
fn categories_for(prefix: &str) -> Vec<Category> {
    let trimmed = prefix.trim_start();
    let lower = trimmed.to_ascii_lowercase();

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

/// Turns a Skript pattern into an LSP snippet, with a tab stop per slot.
fn snippet_for(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut stop = 1;
    let mut chars = pattern.chars().peekable();
    let mut depth = 0i32;

    while let Some(ch) = chars.next() {
        match ch {
            // Optional parts are dropped: the shortest correct form is the
            // best starting point, and the user can add the rest.
            '[' => depth += 1,
            ']' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            '(' => {
                // Take the first alternative of a choice, then skip the rest of
                // the group — otherwise the remaining branches leak into the
                // snippet as literal text.
                let mut branch = String::new();
                let mut taken = false;
                let mut inner = 0i32;
                for ch in chars.by_ref() {
                    match ch {
                        '(' => {
                            inner += 1;
                            if !taken {
                                branch.push(ch);
                            }
                        }
                        ')' if inner == 0 => break,
                        ')' => {
                            inner -= 1;
                            if !taken {
                                branch.push(ch);
                            }
                        }
                        '|' if inner == 0 => taken = true,
                        _ if !taken => branch.push(ch),
                        _ => {}
                    }
                }
                out.push_str(branch.trim());
            }
            '%' => {
                let mut name = String::new();
                for ch in chars.by_ref() {
                    if ch == '%' {
                        break;
                    }
                    name.push(ch);
                }
                let name = name.trim_start_matches(['~', '-', '*']);
                out.push_str(&format!("${{{stop}:{name}}}"));
                stop += 1;
            }
            '<' => {
                for ch in chars.by_ref() {
                    if ch == '>' {
                        break;
                    }
                }
                out.push_str(&format!("${stop}"));
                stop += 1;
            }
            _ => out.push(ch),
        }
    }

    // Collapse the double spaces left by dropped optionals.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn lsp_symbol_kind(kind: SymbolKind) -> tower_lsp::lsp_types::SymbolKind {
    use tower_lsp::lsp_types::SymbolKind as Lsp;
    match kind {
        SymbolKind::Event => Lsp::EVENT,
        SymbolKind::Command => Lsp::METHOD,
        SymbolKind::Entry => Lsp::PROPERTY,
        SymbolKind::Function | SymbolKind::LocalFunction => Lsp::FUNCTION,
        SymbolKind::Option => Lsp::CONSTANT,
        SymbolKind::GlobalVariable | SymbolKind::LocalVariable => Lsp::VARIABLE,
        SymbolKind::Alias => Lsp::CONSTANT,
        SymbolKind::Section => Lsp::NAMESPACE,
        SymbolKind::Structure => Lsp::MODULE,
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

/// Everything the background load produces.
struct Loaded {
    catalog: Catalog,
    /// Syntax for addons that are *not* installed, for the `requires-addon`
    /// diagnostic. `None` unless we actually know what is installed.
    uninstalled: Option<Catalog>,
    detection: Detection,
    messages: Vec<String>,
}

/// Detects the project's addons, then builds the catalog from every source.
///
/// Order matters: Skript's own database goes in first so that SkriptHub's 1,237
/// duplicate copies of core syntax lose the id-based dedup to the authoritative
/// entries.
fn load_everything(settings: &Settings, roots: &[std::path::PathBuf]) -> Loaded {
    let cache_dir = std::env::temp_dir().join("skript-lsp");
    let mut messages = Vec::new();

    // ---- 1. what is installed ------------------------------------------
    let detection = if settings.addons.is_off() {
        Detection::default()
    } else {
        skript_addons::detect(roots)
    };

    if let Some(dir) = &detection.plugins_dir {
        messages.push(format!(
            "found {} plugin(s) in {}: {} Skript addon(s){}",
            detection.plugins.len(),
            dir.display(),
            detection.addons().count(),
            match detection.addon_names().as_slice() {
                [] => String::new(),
                names => format!(" — {}", names.join(", ")),
            }
        ));
    }

    // ---- 2. core Skript -------------------------------------------------
    let (mut docs, mut fell_back) =
        match skript_docs::source::load(&settings.docs_source(), &cache_dir) {
            Ok(docs) => (docs, false),
            Err(error) => {
                messages.push(format!(
                    "could not load the Skript syntax database ({error}); falling back to the \
                 built-in catalog. Highlighting, outline, folding, go-to-definition and rename \
                 still work; hover and completion will be limited."
                ));
                (skript_docs::fallback_docs(), true)
            }
        };

    if !fell_back {
        messages.push(format!(
            "loaded Skript {} syntax: {} entries, {} patterns",
            docs.source.version,
            docs.total_entries(),
            docs.total_patterns(),
        ));
    }

    // ---- 3. the target version ------------------------------------------
    // An explicit setting wins, then the installed Skript JAR, then whatever
    // database we actually loaded.
    let target_version = settings
        .skript_version
        .as_deref()
        .or_else(|| detection.skript_version())
        .or(Some(docs.source.version.as_str()))
        .and_then(skript_docs::version::Version::parse);

    // ---- 4. addon syntax --------------------------------------------------
    let wanted: Vec<String> = match settings.addons.explicit() {
        Some(names) => names.to_vec(),
        None if settings.addons.is_off() => Vec::new(),
        None => detection.addon_names(),
    };

    let mut uninstalled = None;

    if !wanted.is_empty()
        && settings
            .addon_syntax_source
            .eq_ignore_ascii_case("skripthub")
    {
        match skript_docs::source::fetch_text(&DocsSource::SkriptHub, &cache_dir) {
            Ok(text) => {
                let matches = |name: &str| wanted.iter().any(|w| w.eq_ignore_ascii_case(name));

                match skript_docs::skripthub::parse_filtered(&text, matches) {
                    Ok(addon_catalog) => {
                        messages.push(format!(
                            "loaded addon syntax for {}: {} patterns",
                            wanted.join(", "),
                            addon_catalog.docs.total_patterns(),
                        ));
                        docs.merge(addon_catalog.docs);
                    }
                    Err(error) => messages.push(format!("could not read addon syntax: {error}")),
                }

                // Only worth building when we know what is installed — without
                // that, "this addon is missing" is a guess.
                if detection.is_known() {
                    if let Ok(rest) = skript_docs::skripthub::parse_filtered(&text, |name| {
                        !name.is_empty() && !matches(name) && !name.eq_ignore_ascii_case("Skript")
                    }) {
                        let mut rest_docs = rest.docs;
                        rest_docs.resolve_versions();
                        uninstalled = Some(Catalog::build(rest_docs));
                    }
                }
            }
            Err(error) => messages.push(format!("could not fetch addon syntax: {error}")),
        }
    }

    // ---- 5. user-supplied syntax -----------------------------------------
    for path in &settings.custom_syntax_paths {
        match skript_docs::source::load(&DocsSource::Custom(path.into()), &cache_dir) {
            Ok(custom) => {
                messages.push(format!(
                    "loaded custom syntax from {path}: {} entries",
                    custom.total_entries()
                ));
                docs.merge(custom);
            }
            Err(error) => messages.push(format!("could not load custom syntax {path}: {error}")),
        }
    }

    fell_back = false;
    let _ = fell_back;

    docs.resolve_versions();
    let catalog = Catalog::build(docs).with_target_version(target_version);

    if let Some(version) = target_version {
        messages.push(format!("targeting Skript {version}"));
    }

    Loaded {
        catalog,
        uninstalled,
        detection,
        messages,
    }
}

#[tokio::main]
async fn main() {
    // stdout is the protocol channel and must carry nothing else.
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        state: Arc::new(RwLock::new(State::default())),
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_context_narrows_by_position() {
        assert_eq!(categories_for("on "), vec![Category::Event]);
        assert!(categories_for("\tif ").contains(&Category::Condition));
        assert!(categories_for("\tsend \"%").contains(&Category::Expression));
        assert!(categories_for("").contains(&Category::Structure));
    }

    #[test]
    fn snippets_drop_optionals_and_number_the_slots() {
        assert_eq!(
            snippet_for("give %item types% to %players%"),
            "give ${1:item types} to ${2:players}"
        );
        assert_eq!(snippet_for("cancel [the] event"), "cancel event");
        assert_eq!(snippet_for("(spawn|summon) %number%"), "spawn ${1:number}");
    }

    #[test]
    fn snippets_strip_slot_modifiers() {
        assert_eq!(snippet_for("filter %~objects%"), "filter ${1:objects}");
    }

    #[test]
    fn settings_choose_the_right_docs_source() {
        let local = Settings {
            docs_path: Some("/tmp/docs.json".into()),
            ..Default::default()
        };
        assert!(matches!(local.docs_source(), DocsSource::Local(_)));

        let pinned = Settings {
            skript_version: Some("2.15.3".into()),
            ..Default::default()
        };
        assert!(matches!(pinned.docs_source(), DocsSource::Version(_)));

        assert!(matches!(
            Settings::default().docs_source(),
            DocsSource::Latest
        ));
    }

    #[test]
    fn finds_the_enclosing_call_and_argument_index() {
        assert_eq!(
            enclosing_call("\tset {_x} to greet("),
            Some(("greet".to_string(), 0))
        );
        assert_eq!(
            enclosing_call("\tset {_x} to greet(\"a\", "),
            Some(("greet".to_string(), 1))
        );
        // A closed call is no longer enclosing.
        assert_eq!(enclosing_call("\tset {_x} to greet(\"a\") "), None);
        // A paren inside a string must not open a call.
        assert_eq!(enclosing_call("\tsend \"a (b\" "), None);
        // Nested calls report the innermost.
        assert_eq!(
            enclosing_call("\tset {_x} to outer(inner("),
            Some(("inner".to_string(), 0))
        );
    }

    #[test]
    fn every_symbol_kind_maps_to_an_lsp_kind() {
        // A missing arm would be a compile error; this pins the intent that
        // commands read as methods and structures as modules in the outline.
        assert_eq!(
            lsp_symbol_kind(SymbolKind::Command),
            tower_lsp::lsp_types::SymbolKind::METHOD
        );
        assert_eq!(
            lsp_symbol_kind(SymbolKind::Function),
            tower_lsp::lsp_types::SymbolKind::FUNCTION
        );
    }
}

#[cfg(test)]
mod inlay_hint_helpers {
    use super::{argument_offsets, parameter_names};

    #[test]
    fn parameter_names_come_out_of_a_rendered_signature() {
        assert_eq!(
            parameter_names("(who: player, amount: number = 1) :: text"),
            vec!["who", "amount"]
        );
        assert_eq!(parameter_names("() :: boolean"), Vec::<String>::new());
        assert_eq!(parameter_names("no parens at all"), Vec::<String>::new());
    }

    #[test]
    fn a_comma_inside_a_parenthesised_default_is_not_a_separator() {
        // `(xs: integers = (1, 7), flag: boolean)` is two parameters, not three.
        assert_eq!(
            parameter_names("(xs: integers = (1, 7), flag: boolean)"),
            vec!["xs", "flag"]
        );
    }

    #[test]
    fn argument_offsets_point_at_each_argument() {
        //          0         1         2
        //          0123456789012345678901234
        let line = "\tset {_x} to greet(a, b)";
        // `greet` ends at column 18.
        let offsets = argument_offsets(line, 0, 18);
        assert_eq!(offsets.len(), 2);
        assert_eq!(&line[offsets[0] as usize..offsets[0] as usize + 1], "a");
        assert_eq!(&line[offsets[1] as usize..offsets[1] as usize + 1], "b");
    }

    #[test]
    fn a_comma_inside_a_string_or_a_nested_call_does_not_split() {
        let line = "greet(\"a, b\", f(1, 2), c)";
        let offsets = argument_offsets(line, 0, 5);
        assert_eq!(offsets.len(), 3, "got {offsets:?}");
    }

    #[test]
    fn a_name_not_followed_by_a_paren_yields_nothing() {
        // Guards against hinting on something that only looked like a call.
        assert!(argument_offsets("set {_x} to 5", 0, 3).is_empty());
    }
}
