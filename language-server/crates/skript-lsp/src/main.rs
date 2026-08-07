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
mod entries;
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
                    // A pop-up, not a log line: the log pane is not somewhere a
                    // first-time user thinks to look when completion is empty.
                    if let Some(warning) = &loaded.degraded {
                        client.show_message(MessageType::WARNING, warning).await;
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
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".into(), ",".into()]),
                    retrigger_characters: Some(vec![",".into()]),
                    work_done_progress_options: Default::default(),
                }),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(true),
                    // Deliberately **not** a space. Skript is written in prose,
                    // so a space-triggered popup is open almost all the time —
                    // and while it is open the editor gives Enter to the
                    // completion instead of to the newline. Typing an ordinary
                    // sentence then fights back, which is far worse than having
                    // to ask for the list.
                    //
                    // These three each open something a plain word cannot
                    // continue: a variable, an option reference, an
                    // interpolation. Word characters still bring the list up on
                    // their own through the editor's own behaviour.
                    trigger_characters: Some(vec!["{".into(), "@".into(), "%".into()]),
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
        // The project index was a one-shot startup snapshot. After a `git pull`
        // that adds a script, calling its function gave a permanent red "no
        // function named … is declared in this project" on correct code, with
        // nothing to suggest that restarting the server would fix it.
        //
        // There is no static capability for this — it has to be registered
        // dynamically, and a client that refuses simply keeps the old
        // behaviour.
        let watchers = vec![FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*.sk".into()),
            kind: None,
        }];
        let registration = Registration {
            id: "skript-watch-scripts".into(),
            method: "workspace/didChangeWatchedFiles".into(),
            register_options: serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                watchers,
            })
            .ok(),
        };
        if let Err(error) = self.client.register_capability(vec![registration]).await {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("no file watching ({error}); scripts changed outside the editor will                              need a restart to be picked up"),
                )
                .await;
        }

        self.client
            .log_message(MessageType::INFO, "skript-lsp ready")
            .await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        {
            let mut state = self.state.write().await;
            for change in &params.changes {
                let uri = change.uri.to_string();
                match change.typ {
                    FileChangeType::DELETED => state.workspace.close(&uri),
                    _ => {
                        // Never clobber a buffer the editor is holding: the
                        // version on disk may be older than what the user is
                        // looking at.
                        if state.workspace.get(&uri).is_some() {
                            continue;
                        }
                        if let Ok(path) = change.uri.to_file_path() {
                            if let Ok(text) = std::fs::read_to_string(path) {
                                state.workspace.update(&uri, text);
                            }
                        }
                    }
                }
            }
        }

        // A new file can resolve an `unknown-function` in a file the user
        // already has open, so every open document is rechecked.
        self.republish_open().await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        // The extension forwards `lsp.skript-lsp.settings`, but the server only
        // ever read `initialization_options` — so configuring the more
        // idiomatic Zed location did nothing at all, silently.
        let Ok(settings) = serde_json::from_value::<Settings>(params.settings.clone()) else {
            return;
        };

        {
            let mut state = self.state.write().await;
            state.diagnostics = diagnostics::Options {
                unknown_syntax: settings.unknown_syntax_diagnostics,
                deprecated_syntax: settings.deprecated_syntax_diagnostics,
            };
        }

        self.republish_open().await;
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

        // A structure entry is not a syntax pattern, so the catalog can never
        // explain it. `docs.json` describes no entries at all, which is why
        // these are the one curated table in the server — see `entries`.
        if in_command_entry_position(document, position.line) {
            if let Some(entry) = entries::lookup(entries::COMMAND, code) {
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(markdown(entries::hover(entry))),
                    range: None,
                }));
            }
        }

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
        let Some((kind, name, scope)) = scoped_symbol_under_cursor(document, position) else {
            return Ok(None);
        };

        let mut locations = Vec::new();
        for (target, reference) in state
            .workspace
            .references_in_scope(kind, &name, &uri, scope)
        {
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
        let Some((kind, name, scope)) = scoped_symbol_under_cursor(document, position) else {
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
            // A trigger-local only highlights within its own trigger. Lighting
            // up every `{_i}` in the file is the thing an experienced developer
            // notices first and reads as "this does not understand the
            // language".
            if kind == SymbolKind::LocalVariable && scope.is_some() && reference.scope != scope {
                continue;
            }
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
            // The declaration's own parameter symbols, rather than re-parsing
            // the rendered signature string.
            let names: Vec<String> = symbol
                .children
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect();
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
        let Some((kind, name, scope)) = scoped_symbol_under_cursor(document, position) else {
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

        for (target, reference) in state
            .workspace
            .references_in_scope(kind, &name, &uri, scope)
        {
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
        let prefix = convert::line_prefix(line, position.character);

        // What an accepted completion should replace.
        //
        // Skript syntax is multi-word, and the client's idea of "the word being
        // typed" stops at a space. So typing `send m` and accepting Message
        // replaced only the `m`, leaving `send message %objects%` — the keyword
        // duplicated. The server knows where the fragment really starts, so it
        // says so rather than letting the client guess.
        let replace = Range::new(
            Position::new(position.line, fragment_start(prefix) as u32),
            params.text_document_position.position,
        );
        let typed_fragment = &prefix[fragment_start(prefix).min(prefix.len())..];
        let mut items = Vec::new();

        // Inside a command, the useful suggestions are its entries — not the
        // 1,200 effects that cannot legally appear there. Offering those was
        // the single most misleading thing completion did.
        if in_command_entry_position(document, position.line) && !prefix.contains(':') {
            for entry in entries::COMMAND {
                items.push(CompletionItem {
                    label: format!("{}:", entry.key),
                    kind: Some(CompletionItemKind::PROPERTY),
                    detail: Some("command entry".into()),
                    documentation: Some(Documentation::MarkupContent(markdown(entries::hover(
                        entry,
                    )))),
                    insert_text: Some(match entry.key {
                        // The only entry that opens a section.
                        "trigger" => "trigger:
	$0"
                        .to_string(),
                        key => format!("{key}: $0"),
                    }),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    sort_text: Some(format!("0{}", entry.key)),
                    ..Default::default()
                });
            }
            return Ok(Some(CompletionResponse::Array(items)));
        }

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
                        // The label is the documentation *title*; the user types
                        // the *pattern*. Zed filters on the label alone, so
                        // without this, typing `send` never surfaces the effect
                        // filed under "Message" — and 60 of 139 effects are
                        // unreachable by the very keyword they begin with.
                        // Matching on both is the whole point of completion in a
                        // language you write as prose.
                        filter_text: Some(filter_text_for(&entry.name, pattern)),
                        sort_text: Some(format!("{rank}{}", entry.name)),
                        // Rendering every entry's Markdown card here meant
                        // thousands of documents rebuilt on each keystroke —
                        // and " " is a completion trigger, so that was every
                        // space typed. The client asks for the one item it
                        // actually shows via `completionItem/resolve`.
                        data: Some(serde_json::json!([category.label(), id.index])),
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                            range: replace,
                            new_text: snippet_for_typed(pattern, typed_fragment),
                        })),
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

    /// Quick fixes for the diagnostics the client hands back.
    ///
    /// No `data` round-trip is needed: `CodeActionParams` returns each
    /// diagnostic with its `range` and `code` intact, which is everything these
    /// fixes require.
    async fn code_action(&self, params: CodeActionParams) -> RpcResult<Option<CodeActionResponse>> {
        let state = self.state.read().await;
        let uri = params.text_document.uri.clone();
        let Some(document) = state.workspace.get(uri.as_ref()) else {
            return Ok(None);
        };

        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        let mut offered_indent_fix = false;
        let line_count = document.text().lines().count() as u32;
        let end_of_file = Range::new(Position::new(line_count, 0), Position::new(line_count, 0));

        for diagnostic in &params.context.diagnostics {
            let Some(NumberOrString::String(code)) = &diagnostic.code else {
                continue;
            };

            match code.as_str() {
                // All three indentation codes share one real fix, and the
                // formatter already knows how to produce it. One action rather
                // than three, offered once however many lines are flagged.
                "mixed-indentation" | "inconsistent-indentation" | "indent-not-a-multiple"
                    if !offered_indent_fix =>
                {
                    offered_indent_fix = true;
                    if let Some(formatted) =
                        skript_format::format(document, skript_format::Options::default())
                    {
                        actions.push(quick_fix(
                            "Fix indentation in this file",
                            uri.clone(),
                            vec![TextEdit {
                                range: Range::new(
                                    Position::new(0, 0),
                                    Position::new(line_count, 0),
                                ),
                                new_text: formatted,
                            }],
                            diagnostic.clone(),
                        ));
                    }
                }

                "unclosed-block-comment" => actions.push(quick_fix(
                    "Close the block comment",
                    uri.clone(),
                    vec![TextEdit {
                        range: end_of_file,
                        new_text: "###\n".into(),
                    }],
                    diagnostic.clone(),
                )),

                "unknown-function" => {
                    let name = document
                        .symbols()
                        .references
                        .iter()
                        .find(|reference| {
                            reference.kind == SymbolKind::Function
                                && reference.range.start.line == diagnostic.range.start.line
                        })
                        .map(|reference| reference.name.clone());
                    let Some(name) = name else { continue };

                    // A near-miss first: accepting a typo correction is far more
                    // often what was meant than declaring a new function.
                    if let Some(suggestion) = closest_function(&state.workspace, &name) {
                        actions.push(quick_fix(
                            &format!("Change to `{suggestion}`"),
                            uri.clone(),
                            vec![TextEdit {
                                range: diagnostic.range,
                                new_text: suggestion,
                            }],
                            diagnostic.clone(),
                        ));
                    }

                    actions.push(quick_fix(
                        &format!("Create function `{name}`"),
                        uri.clone(),
                        vec![TextEdit {
                            range: end_of_file,
                            new_text: format!("\nfunction {name}():\n\treturn\n"),
                        }],
                        diagnostic.clone(),
                    ));
                }

                _ => {}
            }
        }

        Ok((!actions.is_empty()).then_some(actions))
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
        let prefix = convert::line_prefix(line, position.character);

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
    /// Rechecks every indexed document.
    ///
    /// A file appearing or a setting changing can resolve — or create — a
    /// diagnostic in a file the user already has open, so republishing only the
    /// changed file would leave stale errors on screen.
    async fn republish_open(&self) {
        let uris: Vec<String> = {
            let state = self.state.read().await;
            state
                .workspace
                .documents()
                .map(|document| document.uri().to_string())
                .collect()
        };
        for uri in uris {
            self.publish(&uri).await;
        }
    }

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
                    // Semantic tokens carry a `deprecated` modifier, but only on
                    // the syntax's literal spans and only if the theme opts in.
                    // The tag is the theme-independent path, and it is what
                    // gives the line a strikethrough rather than just a colour.
                    tags: (diagnostic.code == "deprecated-syntax")
                        .then(|| vec![DiagnosticTag::DEPRECATED]),
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

/// Whether `line` is a position where a command *entry* belongs.
///
/// True directly inside a `command` body, and false again once inside one of
/// its sections — `trigger:` holds statements, not entries, and offering entry
/// keys there would replace exactly the suggestions the user needs.
///
/// Read from the index rather than by scanning upwards for the `command`
/// keyword, so a command mentioned in a comment or a string is never mistaken
/// for one, and from the symbol tree rather than from indent width, which
/// varies per file.
fn in_command_entry_position(document: &skript_index::Document, line: u32) -> bool {
    for symbol in &document.symbols().symbols {
        if symbol.kind != SymbolKind::Command
            || line <= symbol.range.start.line
            || line > symbol.range.end.line
        {
            continue;
        }
        // Inside the command. Now rule out its sections. An entry that spans
        // more than its own line is one that opened a body — `trigger:` — and
        // everything below that line is code, not entries.
        //
        // Deliberately not testing whether the child has children of its own: a
        // trigger whose body is plain statements has none, which made an
        // earlier version of this offer entry keys in the middle of a trigger
        // and replace the suggestions the user actually needed.
        let inside_body = symbol
            .children
            .iter()
            .any(|child| line > child.range.start.line && line <= child.range.end.line);
        return !inside_body;
    }
    false
}

/// Byte column where the fragment the user is typing begins.
///
/// For an ordinary statement that is the first non-blank character: Skript
/// syntax is a whole multi-word phrase, so accepting `Message` after `send m`
/// has to replace both words, not just the `m`.
///
/// Inside `%…%`, or after an opening paren or a comma, the fragment starts
/// there instead — an expression completion must not eat the effect around it.
fn fragment_start(prefix: &str) -> usize {
    let indent = prefix.len() - prefix.trim_start().len();

    // The last thing that opens a nested context, if any.
    let boundary = prefix
        .char_indices()
        .rfind(|(_, ch)| matches!(ch, '%' | '(' | ',' | '{'))
        .map(|(at, ch)| at + ch.len_utf8());

    match boundary {
        // Skip whitespace after the delimiter so the replacement does not
        // swallow the space the user typed.
        Some(at) => {
            let rest = &prefix[at..];
            at + (rest.len() - rest.trim_start().len())
        }
        None => indent,
    }
}

/// Builds a quick-fix action carrying one file's edits.
fn quick_fix(
    title: &str,
    uri: Url,
    edits: Vec<TextEdit>,
    diagnostic: Diagnostic,
) -> CodeActionOrCommand {
    let mut changes = std::collections::HashMap::new();
    changes.insert(uri, edits);
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// The declared function whose name is nearest `name`, when one is near enough
/// to be worth offering.
///
/// The threshold scales with length, so a three-letter name matches nothing and
/// a long one tolerates a couple of typos. Suggesting a wildly different name is
/// worse than suggesting none.
fn closest_function(workspace: &Workspace, name: &str) -> Option<String> {
    let limit = (name.len() / 3).clamp(1, 3);
    let mut best: Option<(usize, String)> = None;

    for document in workspace.documents() {
        for symbol in document.symbols().flat() {
            if !symbol.kind.is_function() || symbol.name == name {
                continue;
            }
            let distance = edit_distance(name, &symbol.name);
            if distance <= limit && best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
                best = Some((distance, symbol.name.clone()));
            }
        }
    }
    best.map(|(_, name)| name)
}

/// Levenshtein distance, kept to two rows.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];

    for (i, left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(left != right);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// The renameable symbol under the cursor, and which trigger a local belongs to.
///
/// The scope is what confines a `{_x}` rename to the trigger that owns it,
/// rather than every trigger in the file.
fn scoped_symbol_under_cursor(
    document: &skript_index::Document,
    position: skript_index::Position,
) -> Option<(SymbolKind, String, Option<skript_index::Range>)> {
    if let Some(symbol) = document.symbols().declaration_at(position) {
        if renameable(symbol.kind) {
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
    renameable(reference.kind).then(|| (reference.kind, reference.name.clone(), reference.scope))
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

/// What the client should match the user's typing against.
///
/// The entry name plus the pattern's literal words, so either finds the item:
/// somebody who knows it as "Message" and somebody who just types `send` both
/// get there. Slots and tab stops are stripped — nobody types `%objects%`.
fn filter_text_for(name: &str, pattern: &str) -> String {
    // Every alternative, not just the first. `snippet_for` picks one branch of
    // `(message|send)` because it has to insert *something*; filtering wants the
    // opposite — `send` must be matchable even though the snippet inserts
    // `message`.
    let mut text = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            // Slots and regex holes are not typed by anyone.
            '%' => {
                for next in chars.by_ref() {
                    if next == '%' {
                        break;
                    }
                }
                text.push(' ');
            }
            '<' => {
                for next in chars.by_ref() {
                    if next == '>' {
                        break;
                    }
                }
                text.push(' ');
            }
            // Group and alternation syntax is structure, not text.
            '(' | ')' | '[' | ']' | '|' => text.push(' '),
            _ => text.push(ch),
        }
    }

    let mut words: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        if !words.contains(&word) {
            words.push(word);
        }
    }

    if words.is_empty() {
        name.to_string()
    } else {
        format!("{name} {}", words.join(" "))
    }
}

/// Turns a Skript pattern into an LSP snippet, with a tab stop per slot.
///
/// The form inserted when the user has typed nothing to disambiguate a choice.
#[cfg(test)]
fn snippet_for(pattern: &str) -> String {
    snippet_for_typed(pattern, "")
}

/// As [`snippet_for`], but preferring whichever alternative the user has begun
/// to type, so accepting `Message` after typing `send` does not rewrite the word
/// under the cursor into `message`.
fn snippet_for_typed(pattern: &str, typed: &str) -> String {
    let mut out = String::new();
    let mut stop = 1;
    expand(pattern, typed, &mut stop, &mut out);
    // Collapse the double spaces left by dropped optionals.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Expands one level of pattern text into `out`.
///
/// Recursive, because a chosen branch is itself pattern text: Skript writes
/// `(message|send [message[s]])`, so the optional lives *inside* the
/// alternative. Pushing a branch verbatim leaked `[message[s]]` into the snippet
/// as literal characters.
fn expand(pattern: &str, typed: &str, stop: &mut usize, out: &mut String) {
    let mut chars = pattern.chars().peekable();
    let mut depth = 0i32;

    while let Some(ch) = chars.next() {
        match ch {
            // Optional parts are dropped: the shortest correct form is the best
            // starting point, and the user can add the rest.
            '[' => depth += 1,
            ']' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            '(' => {
                let mut branches: Vec<String> = vec![String::new()];
                let mut inner = 0i32;
                for ch in chars.by_ref() {
                    match ch {
                        '(' => {
                            inner += 1;
                            branches.last_mut().expect("never empty").push(ch);
                        }
                        ')' if inner == 0 => break,
                        ')' => {
                            inner -= 1;
                            branches.last_mut().expect("never empty").push(ch);
                        }
                        '|' if inner == 0 => branches.push(String::new()),
                        _ => branches.last_mut().expect("never empty").push(ch),
                    }
                }
                let chosen = pick_branch(&branches, typed).to_string();
                expand(&chosen, typed, stop, out);
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
                *stop += 1;
            }
            '<' => {
                for ch in chars.by_ref() {
                    if ch == '>' {
                        break;
                    }
                }
                out.push_str(&format!("${stop}"));
                *stop += 1;
            }
            _ => out.push(ch),
        }
    }
}

/// Which branch of a choice to insert.
///
/// The one the user has started typing, when there is one — otherwise the first,
/// which is the shortest correct form.
///
/// Both sides are reduced to their leading word. A branch carries its own tail —
/// Skript writes `(message|send [message[s]])` — and `typed` is the whole
/// fragment entered so far, which may already have moved past the keyword.
/// Comparing leading words is the only thing that lines the two up.
fn pick_branch<'a>(branches: &'a [String], typed: &str) -> &'a str {
    let first_word = |text: &str| {
        text.split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| !ch.is_alphanumeric())
            .to_ascii_lowercase()
    };

    let wanted = first_word(typed);
    if !wanted.is_empty() {
        for branch in branches {
            let candidate = branch.trim();
            let head = first_word(candidate);
            if !head.is_empty() && head.starts_with(&wanted) {
                return candidate;
            }
        }
    }
    branches
        .first()
        .map(|branch| branch.trim())
        .unwrap_or_default()
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
    /// Set when the syntax database could not be loaded and the tiny built-in
    /// catalog took over. Carried separately from `messages` because this one
    /// has to be *shown* to the user, not logged: the fallback has no events,
    /// expressions or conditions at all, so hover and completion go quiet, and
    /// a silent degradation reads exactly like a broken extension.
    degraded: Option<String>,
}

/// Where the downloaded syntax databases are cached.
///
/// Deliberately not the temp directory. `std::env::temp_dir()` is `/tmp` on
/// Linux unless `$TMPDIR` says otherwise, and `/tmp/skript-lsp` is a fixed,
/// predictable name in a world-writable sticky directory: any other local user
/// can create it first and leave a crafted `docs-latest.json` there, which the
/// server would then load as the authoritative description of the language.
/// `/tmp` is also swept by systemd-tmpfiles, which defeats the 24-hour cache TTL
/// and re-downloads roughly 9 MB far more often than intended.
///
/// Falls back to the temp directory only when the platform's cache location
/// cannot be determined, since a poor cache still beats refusing to start.
fn cache_directory() -> std::path::PathBuf {
    // Every branch checks `is_absolute`, not just the XDG one: a relative value
    // would put the cache wherever the server happened to be started from.
    let absolute = |path: std::path::PathBuf| path.is_absolute().then_some(path);

    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .and_then(absolute)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .and_then(absolute)
            .map(|home| home.join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .and_then(absolute)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(std::path::PathBuf::from)
                    .and_then(absolute)
                    .map(|home| home.join(".cache"))
            })
    };

    base.unwrap_or_else(std::env::temp_dir).join("skript-lsp")
}

/// Detects the project's addons, then builds the catalog from every source.
///
/// Order matters: Skript's own database goes in first so that SkriptHub's 1,237
/// duplicate copies of core syntax lose the id-based dedup to the authoritative
/// entries.
fn load_everything(settings: &Settings, roots: &[std::path::PathBuf]) -> Loaded {
    let cache_dir = cache_directory();
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
    let (mut docs, degraded) = match skript_docs::source::load(&settings.docs_source(), &cache_dir)
    {
        Ok(docs) => (docs, None),
        Err(error) => {
            let warning = format!(
                "Skript syntax database unavailable ({error}). Highlighting, indentation, \
                 folding, outline, go-to-definition and rename all still work — but hover and \
                 completion need the database and will be nearly empty until it downloads. \
                 Check your connection and restart the language server."
            );
            (skript_docs::fallback_docs(), Some(warning))
        }
    };

    if degraded.is_none() {
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
        degraded,
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
    use super::argument_offsets;

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

#[cfg(test)]
mod entry_position_tests {
    use super::in_command_entry_position;
    use skript_index::Workspace;

    /// Entry keys belong directly in a command's body — and nowhere else.
    /// Offering them inside `trigger:` replaces the effect and function
    /// suggestions the user actually needs there, which is worse than offering
    /// nothing at all.
    #[test]
    fn entries_belong_in_the_command_body_but_not_in_its_trigger() {
        let source = "command /hello <text>:\n\tpermission: skript.hello\n\ttrigger:\n\t\tsend \"hi\" to player\n\t\tstop\n\non join:\n\tstop\n";
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();

        // Directly inside the command.
        assert!(in_command_entry_position(document, 1), "the entry line");

        // Inside the trigger's body. An earlier version tested whether the
        // trigger had *child sections*; a trigger of plain statements has none,
        // so it wrongly reported an entry position here.
        assert!(!in_command_entry_position(document, 3), "inside trigger");
        assert!(!in_command_entry_position(document, 4), "inside trigger");

        // Outside the command entirely.
        assert!(!in_command_entry_position(document, 0), "the header itself");
        assert!(!in_command_entry_position(document, 7), "inside an event");
    }
}

#[cfg(test)]
mod completion_findability {
    use super::filter_text_for;

    /// Completion must be reachable by the word a user actually types.
    ///
    /// The label is Skript's documentation *title*, and Zed filters on the label
    /// alone. Measured against the published database, 60 of 139 effects have
    /// their pattern's leading keyword nowhere in their name — so typing `send`
    /// did not surface "Message", and typing `make` did not surface "Consume
    /// Brewing Fuel". For a language written as English prose that inverts the
    /// point of completion.
    #[test]
    fn the_keyword_a_user_types_is_matchable() {
        for (name, pattern, typed) in [
            (
                "Message",
                "(message|send) [message[s]] %objects% [to %commandsenders%]",
                "send",
            ),
            (
                "Consume Brewing Fuel",
                "make %blocks% [not] consume [the] fuel",
                "make",
            ),
            (
                "Apply Fishing Lure",
                "(reel|pull) in [the] hook[ed] entity",
                "reel",
            ),
            (
                "Teleport",
                "teleport %entities% (to|%direction%) %location%",
                "teleport",
            ),
        ] {
            let filter = filter_text_for(name, pattern).to_lowercase();
            assert!(
                filter.contains(typed),
                "typing {typed:?} cannot reach {name:?}; filter text was {filter:?}"
            );
            // The name must still work for anybody who knows it.
            assert!(
                filter.contains(&name.to_lowercase()),
                "{name:?} is no longer findable by its own name"
            );
        }
    }

    #[test]
    fn slots_and_tab_stops_are_not_matchable_text() {
        // Nobody types `%objects%` or `${1:...}`.
        let filter = filter_text_for("Message", "(message|send) %objects% [to %commandsenders%]");
        assert!(!filter.contains('%'), "a slot leaked in: {filter:?}");
        assert!(!filter.contains('$'), "a tab stop leaked in: {filter:?}");
    }

    #[test]
    fn a_pattern_with_no_literals_still_yields_the_name() {
        assert_eq!(filter_text_for("Entities", "%*entity types%"), "Entities");
    }
}

#[cfg(test)]
mod code_action_helpers {
    use super::{closest_function, edit_distance};
    use skript_index::Workspace;

    #[test]
    fn edit_distance_is_the_usual_one() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn a_typo_is_suggested_but_an_unrelated_name_is_not() {
        let mut workspace = Workspace::new();
        workspace.open(
            "file:///t.sk",
            "function giveKit(p: player):\n\treturn\n\nfunction teleportHome(p: player):\n\treturn\n",
        );

        // One transposed letter — worth offering.
        assert_eq!(
            closest_function(&workspace, "giveKti"),
            Some("giveKit".to_string())
        );

        // Nothing like either. Suggesting a wildly different name is worse than
        // suggesting none, so the threshold scales with length.
        assert_eq!(closest_function(&workspace, "payout"), None);
    }

    #[test]
    fn a_short_name_does_not_match_everything() {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", "function ab(p: player):\n\treturn\n");
        // `xy` is two edits from `ab`, which for a two-letter name is the whole
        // word; the limit is 1 so nothing is offered.
        assert_eq!(closest_function(&workspace, "xy"), None);
    }
}

#[cfg(test)]
mod completion_insertion {
    use super::{fragment_start, pick_branch, snippet_for, snippet_for_typed};

    /// Skript's real Message pattern, where the optional lives *inside* the
    /// alternative. Matching on whole branch text meant `send [message[s]]`
    /// never matched somebody typing `send`, and pushing the branch verbatim
    /// leaked `[message[s]]` into the snippet as literal characters.
    const MESSAGE: &str = "(message|send [message[s]]) %objects% [to %audiences%]";

    #[test]
    fn a_typed_keyword_survives_and_its_optional_does_not_leak() {
        for typed in ["send", "se", "send m"] {
            let snippet = snippet_for_typed(MESSAGE, typed);
            assert!(
                snippet.starts_with("send"),
                "typing {typed:?} inserted {snippet:?}"
            );
            assert!(!snippet.contains('['), "an optional leaked: {snippet:?}");
        }
    }

    #[test]
    fn the_default_is_still_the_shortest_form() {
        assert_eq!(snippet_for(MESSAGE), "message ${1:objects}");
    }

    #[test]
    fn tab_stops_stay_sequential_through_a_chosen_branch() {
        // A slot inside the branch must not restart the numbering.
        let snippet = snippet_for_typed("(give %items% to|hand) %players%", "give");
        assert!(snippet.contains("${1:"), "{snippet:?}");
        assert!(snippet.contains("${2:"), "{snippet:?}");
    }

    #[test]
    fn an_unrelated_word_falls_back_to_the_first_branch() {
        let list = vec!["message".to_string(), "send".to_string()];
        assert_eq!(pick_branch(&list, "teleport"), "message");
        assert_eq!(pick_branch(&list, ""), "message");
    }

    /// Skript syntax is multi-word and the client's "current word" stops at a
    /// space, so accepting after `send m` used to replace only the `m` and leave
    /// the keyword duplicated.
    #[test]
    fn a_statement_fragment_covers_the_whole_phrase() {
        assert_eq!(fragment_start("\t\tsend m"), 2);
        assert_eq!(fragment_start("send"), 0);
    }

    #[test]
    fn a_nested_fragment_starts_after_its_delimiter() {
        for line in [
            "\tsend \"%pla",
            "\tset {_x} to greet(player, am",
            "\tset {_x} to f(pl",
        ] {
            let start = fragment_start(line);
            assert!(
                !line[start..].contains(['%', '(', ',']),
                "{line:?} -> {:?} still spans a delimiter",
                &line[start..]
            );
        }
    }
}
