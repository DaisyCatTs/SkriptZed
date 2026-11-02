//! The server's state and the LSP entry points.
//!
//! Each request handler here only takes the lock and hands off to the module
//! that owns the feature; the logic lives there, where it can be tested
//! without an LSP session.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use skript_addons::Detection;
use skript_docs::Catalog;
use skript_index::Workspace;
use tokio::sync::{RwLock, RwLockReadGuard};
use tower_lsp::jsonrpc::Result as RpcResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::convert::Encoding;
use crate::settings::{
    apply_settings, layer_settings, parse_settings, AnalyzerSettings, Features, Settings,
};
use crate::{
    analyzer, capabilities, code_actions, completion, diagnostics, document, hierarchy, hover,
    navigation, outline, signature,
};

pub struct Backend {
    pub client: Client,
    pub state: Arc<RwLock<State>>,
}

pub struct State {
    pub workspace: Workspace,
    pub catalog: Option<Catalog>,
    /// Syntax belonging to addons the server does **not** have installed.
    ///
    /// Kept apart from `catalog` so it never reaches completion or semantic
    /// tokens: its only job is to let a diagnostic say "that line is SkBee
    /// syntax, and SkBee is not installed". Only built when a `plugins/`
    /// directory was found.
    pub uninstalled: Option<Catalog>,
    pub detection: Detection,
    pub encoding: Encoding,
    pub diagnostics: diagnostics::Options,
    /// The settings in effect, from `initialize` and every later change.
    pub settings: Settings,
    /// `initialization_options` as sent, which later `settings` are layered
    /// over rather than replacing.
    pub initialization_options: serde_json::Value,
    /// The editor's workspace folders, where `plugins/` and scripts are found.
    pub roots: Vec<PathBuf>,
    /// Bumped by every catalog load, so a slow or retrying load that has been
    /// superseded by a settings change never installs its stale result.
    pub catalog_generation: u64,
    /// URIs the editor has opened.
    ///
    /// `workspace` also holds every `.sk` found on disk, so it cannot answer
    /// this: republishing over it would push diagnostics for several hundred
    /// files nobody is looking at.
    pub open_in_editor: HashSet<String>,
    pub analyzer: AnalyzerState,
    pub rcon: crate::rcon::RconState,
}

/// SkAnalyzer, which loads the real Skript and reports what it rejects.
#[derive(Default)]
pub struct AnalyzerState {
    pub settings: AnalyzerSettings,
    /// Started on the first save that needs it; dropped after a crash or
    /// timeout so the next save starts a fresh one.
    pub running: Option<Arc<analyzer::Analyzer>>,
    pub starting: bool,
    /// Set after a configuration problem, which no amount of retrying fixes.
    /// The user is told once; it stays off until the analyzer settings change
    /// (see `apply_settings`) or the server restarts.
    pub given_up: bool,
    /// Per document, as Skript reported them. Positioned against the current
    /// text at each publish, so an edit never leaves a range past the end of
    /// a line.
    pub findings: HashMap<String, Vec<analyzer::Finding>>,
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
            settings: Settings::default(),
            initialization_options: serde_json::Value::Null,
            roots: Vec::new(),
            catalog_generation: 0,
            open_in_editor: HashSet::new(),
            analyzer: AnalyzerState::default(),
            rcon: crate::rcon::RconState::default(),
        }
    }
}

impl State {
    /// Where to look for `plugins/`: the configured server folder, or else
    /// the workspace roots.
    pub fn scan_roots(&self) -> Vec<PathBuf> {
        match &self.settings.server_path {
            Some(path) => vec![PathBuf::from(path)],
            None => self.roots.clone(),
        }
    }
}

/// Brings the index up to date with files changed on disk: (URI, deleted,
/// the file as now read).
fn apply_disk_changes(
    state: &mut State,
    changes: Vec<(String, bool, Option<skript_index::Document>)>,
) {
    for (uri, deleted, parsed) in changes {
        // Never clobber a buffer the editor is holding: the version on disk
        // may be older than what the user is looking at. Asking `workspace`
        // instead skipped every change to an indexed file that was not open,
        // so a `git pull` never reached the index.
        if state.open_in_editor.contains(&uri) {
            continue;
        }
        match parsed {
            Some(document) => {
                state.workspace.close(&uri);
                state.workspace.insert_if_absent(document);
            }
            None if deleted => state.workspace.close(&uri),
            // Unreadable for the moment (mid-write, locked): keep what was
            // indexed rather than forget the file.
            None => {}
        }
    }
}

impl Backend {
    /// Rechecks every document open in the editor.
    ///
    /// A file appearing or a setting changing can resolve (or create) a
    /// diagnostic in a file the user already has open, so republishing only
    /// the changed file would leave stale errors on screen.
    pub async fn republish_open(&self) {
        // Every caller follows something that can change the project's
        // skript-reflect syntax: a new catalog (which starts without it), a
        // file appearing on disk, the project index arriving.
        self.sync_custom_syntax().await;
        let uris: Vec<String> = {
            let state = self.state.read().await;
            state.open_in_editor.iter().cloned().collect()
        };
        for uri in uris {
            self.publish(&uri).await;
        }
    }

    /// Republishes after an edit to `uri`: that file alone, unless the edit
    /// changed the project's skript-reflect syntax, which every open file may
    /// use.
    async fn publish_after_edit(&self, uri: &str) {
        if self.sync_custom_syntax().await {
            self.republish_open().await;
        } else {
            self.publish(uri).await;
        }
    }

    /// Reads settings from the client, reporting each value it had to drop.
    async fn read_settings(&self, value: Option<serde_json::Value>) -> Option<Settings> {
        let (settings, problems) = parse_settings(value.as_ref()?);
        if !problems.is_empty() {
            self.client
                .show_message(
                    MessageType::WARNING,
                    format!("skript-lsp ignored some settings: {}.", problems.join("; ")),
                )
                .await;
        }
        settings
    }

    /// The state, when the feature `switch` picks is on.
    ///
    /// Every capability stays advertised whatever the switches say, so turning
    /// one off or on takes effect at once instead of after a restart; a
    /// switched-off request is simply answered with nothing.
    async fn state_if(&self, switch: fn(&Features) -> bool) -> Option<RwLockReadGuard<'_, State>> {
        let state = self.state.read().await;
        switch(&state.settings.features).then_some(state)
    }

    /// Another handle on the same client and state, for a background task.
    pub fn handle(&self) -> Backend {
        Backend {
            client: self.client.clone(),
            state: self.state.clone(),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> RpcResult<InitializeResult> {
        let settings = self
            .read_settings(params.initialization_options.clone())
            .await
            .unwrap_or_default();
        let encoding = Encoding::negotiate(
            params
                .capabilities
                .general
                .as_ref()
                .and_then(|general| general.position_encodings.as_deref()),
        );
        // The workspace roots serve two purposes: finding the project's
        // `plugins/` directory, and indexing its scripts.
        let roots: Vec<PathBuf> = params
            .workspace_folders
            .map(|folders| {
                folders
                    .into_iter()
                    .filter_map(|folder| folder.uri.to_file_path().ok())
                    .collect()
            })
            .unwrap_or_default();

        {
            let mut state = self.state.write().await;
            state.encoding = encoding;
            apply_settings(&mut state, &settings);
            state.settings = settings;
            state.roots = roots.clone();
            state.initialization_options =
                params.initialization_options.clone().unwrap_or_default();
        }

        // Loading the catalog blocks on the network, so it happens off the
        // request path: the editor becomes usable immediately and gains hover
        // and completion a moment later.
        self.spawn_catalog_load().await;
        self.spawn_project_index(roots).await;

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "skript-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: capabilities::server_capabilities(encoding),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // The project index was a one-shot startup snapshot. After a `git pull`
        // that adds a script, calling its function gave a permanent red "no
        // function named … is declared in this project" on correct code, with
        // nothing to suggest that restarting the server would fix it.
        //
        // There is no static capability for this; it has to be registered
        // dynamically, and a client that refuses keeps the old behaviour.
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
                    format!(
                        "no file watching ({error}); scripts changed outside the editor need \
                         a restart to be picked up"
                    ),
                )
                .await;
        }

        self.client
            .log_message(MessageType::INFO, "skript-lsp ready")
            .await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        // Read and parsed before taking the lock: a `git pull` can touch
        // hundreds of files.
        let changes: Vec<(String, bool, Option<skript_index::Document>)> = params
            .changes
            .iter()
            .map(|change| {
                let uri = change.uri.to_string();
                let deleted = change.typ == FileChangeType::DELETED;
                let parsed = (!deleted)
                    .then(|| change.uri.to_file_path().ok())
                    .flatten()
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .map(|text| skript_index::Document::new(uri.clone(), text));
                (uri, deleted, parsed)
            })
            .collect();
        apply_disk_changes(&mut *self.state.write().await, changes);

        // A new file can resolve an `unknown-function` in a file the user
        // already has open, so every open document is rechecked.
        self.republish_open().await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        // The extension forwards `lsp.skript-lsp.settings`, but the server
        // once read only `initialization_options`, so configuring the more
        // idiomatic Zed location did nothing at all, silently.
        if params.settings.is_null() {
            return;
        }
        let base = self.state.read().await.initialization_options.clone();
        let layered = layer_settings(&base, &params.settings);
        let Some(settings) = self.read_settings(Some(layered)).await else {
            return;
        };

        let (reload, switched) = {
            let mut state = self.state.write().await;
            apply_settings(&mut state, &settings);
            let reload = settings.affects_catalog(&state.settings);
            let switched = settings.features != state.settings.features;
            state.settings = settings;
            (reload, switched)
        };
        // Colours and hints are pulled by the editor, which keeps showing the
        // old ones until asked again. Spawned: these are requests to the
        // client, and one that never answers must not stall this handler.
        if switched {
            let client = self.client.clone();
            tokio::spawn(async move {
                let _ = client.semantic_tokens_refresh().await;
                let _ = client.inlay_hint_refresh().await;
            });
        }
        // The catalog was built from the old database, addons and custom
        // files; those settings used to take effect only after a restart.
        if reload {
            self.spawn_catalog_load().await;
        }
        self.republish_open().await;
    }

    async fn shutdown(&self) -> RpcResult<()> {
        // SkAnalyzer is a separate Java process; left running, it would outlive
        // the editor session.
        let running = self.state.write().await.analyzer.running.take();
        if let Some(analyzer) = running.and_then(|shared| Arc::try_unwrap(shared).ok()) {
            analyzer.shutdown().await;
        }
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        {
            let mut state = self.state.write().await;
            state.workspace.open(uri.clone(), params.text_document.text);
            state.open_in_editor.insert(uri.clone());
        }
        self.publish_after_edit(&uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        // Sync is FULL, so the last change carries the whole document.
        if let Some(change) = params.content_changes.into_iter().next_back() {
            self.state.write().await.workspace.update(&uri, change.text);
        }
        self.publish_after_edit(&uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        // Spawns and returns at once, so the analyzer check is not held up.
        self.reload_on_server(params.text_document.uri.clone())
            .await;
        // Spawned too: a JVM start or a check can take seconds, and tower-lsp
        // runs only four handlers at a time, so a few quick saves awaited here
        // froze every request and edit behind them.
        let backend = self.handle();
        tokio::spawn(async move { backend.check_with_analyzer(params.text_document.uri).await });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();

        // Closing a tab must not remove the file from the project index. The
        // same `Workspace` holds every `.sk` found on disk at startup, so
        // evicting the entry made functions in that file "undefined" for every
        // other open script the moment its tab was closed.
        //
        // Re-reading from disk also discards any unsaved edits, which is right:
        // the buffer is gone, the file is what remains.
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
            // Left in the open set, the next republish (after the project
            // index loads, or a settings change) put a closed file's
            // diagnostics straight back into the problems panel.
            state.open_in_editor.remove(&uri);
            state.analyzer.findings.remove(&uri);
            state.rcon.findings.remove(&uri);
        }
        // Clear the file's diagnostics, or they linger in the problems panel.
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
        // Unsaved edits to a skript-reflect definition were just discarded.
        if self.sync_custom_syntax().await {
            self.republish_open().await;
        }
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> RpcResult<Option<DocumentSymbolResponse>> {
        let Some(state) = self.state_if(|on| on.symbols).await else {
            return Ok(None);
        };
        Ok(outline::document_symbol(&state, &params))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> RpcResult<Option<Vec<SymbolInformation>>> {
        let Some(state) = self.state_if(|on| on.symbols).await else {
            return Ok(None);
        };
        Ok(Some(outline::workspace_symbol(&state, &params)))
    }

    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> RpcResult<Option<Vec<FoldingRange>>> {
        let Some(state) = self.state_if(|on| on.folding).await else {
            return Ok(None);
        };
        Ok(outline::folding_range(&state, &params))
    }

    async fn hover(&self, params: HoverParams) -> RpcResult<Option<Hover>> {
        let Some(state) = self.state_if(|on| on.hover).await else {
            return Ok(None);
        };
        let answer = hover::prepare(&state, &params);
        // Not held across the network fetch for examples.
        drop(state);
        Ok(hover::finish(answer).await)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> RpcResult<Option<GotoDefinitionResponse>> {
        let Some(state) = self.state_if(|on| on.navigation).await else {
            return Ok(None);
        };
        Ok(navigation::goto_definition(&state, &params))
    }

    async fn references(&self, params: ReferenceParams) -> RpcResult<Option<Vec<Location>>> {
        let Some(state) = self.state_if(|on| on.navigation).await else {
            return Ok(None);
        };
        Ok(navigation::references(&state, &params))
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> RpcResult<Option<Vec<CallHierarchyItem>>> {
        let Some(state) = self.state_if(|on| on.call_hierarchy).await else {
            return Ok(None);
        };
        Ok(hierarchy::prepare_request(&state, &params))
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> RpcResult<Option<Vec<CallHierarchyIncomingCall>>> {
        let Some(state) = self.state_if(|on| on.call_hierarchy).await else {
            return Ok(None);
        };
        Ok(hierarchy::incoming_request(&state, &params))
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> RpcResult<Option<Vec<CallHierarchyOutgoingCall>>> {
        let Some(state) = self.state_if(|on| on.call_hierarchy).await else {
            return Ok(None);
        };
        Ok(hierarchy::outgoing_request(&state, &params))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> RpcResult<Option<Vec<DocumentHighlight>>> {
        let Some(state) = self.state_if(|on| on.navigation).await else {
            return Ok(None);
        };
        Ok(navigation::document_highlight(&state, &params))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> RpcResult<Option<Vec<InlayHint>>> {
        let Some(state) = self.state_if(|on| on.inlay_hints).await else {
            return Ok(None);
        };
        Ok(signature::inlay_hint(&state, &params))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> RpcResult<Option<PrepareRenameResponse>> {
        let Some(state) = self.state_if(|on| on.rename).await else {
            return Ok(None);
        };
        Ok(navigation::prepare_rename(&state, &params))
    }

    async fn rename(&self, params: RenameParams) -> RpcResult<Option<WorkspaceEdit>> {
        let Some(state) = self.state_if(|on| on.rename).await else {
            return Ok(None);
        };
        Ok(navigation::rename(&state, &params))
    }

    async fn completion(&self, params: CompletionParams) -> RpcResult<Option<CompletionResponse>> {
        let Some(state) = self.state_if(|on| on.completion).await else {
            return Ok(None);
        };
        Ok(completion::completion(&state, &params))
    }

    async fn completion_resolve(&self, item: CompletionItem) -> RpcResult<CompletionItem> {
        let Some(state) = self.state_if(|on| on.completion).await else {
            return Ok(item);
        };
        Ok(completion::completion_resolve(&state, item))
    }

    async fn code_action(&self, params: CodeActionParams) -> RpcResult<Option<CodeActionResponse>> {
        let Some(state) = self.state_if(|on| on.code_actions).await else {
            return Ok(None);
        };
        Ok(code_actions::code_action(&state, &params))
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> RpcResult<Option<Vec<TextEdit>>> {
        let Some(state) = self.state_if(|on| on.formatting).await else {
            return Ok(None);
        };
        Ok(code_actions::formatting(&state, &params))
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> RpcResult<Option<Vec<TextEdit>>> {
        let Some(state) = self.state_if(|on| on.formatting).await else {
            return Ok(None);
        };
        Ok(code_actions::range_formatting(&state, &params))
    }

    async fn document_color(
        &self,
        params: DocumentColorParams,
    ) -> RpcResult<Vec<ColorInformation>> {
        let Some(state) = self.state_if(|on| on.colors).await else {
            return Ok(Vec::new());
        };
        Ok(document::document_color(&state, &params))
    }

    async fn color_presentation(
        &self,
        params: ColorPresentationParams,
    ) -> RpcResult<Vec<ColorPresentation>> {
        let Some(state) = self.state_if(|on| on.colors).await else {
            return Ok(Vec::new());
        };
        Ok(document::color_presentation(&state, &params))
    }

    async fn document_link(
        &self,
        params: DocumentLinkParams,
    ) -> RpcResult<Option<Vec<DocumentLink>>> {
        let Some(state) = self.state_if(|on| on.links).await else {
            return Ok(None);
        };
        Ok(document::document_link(&state, &params))
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> RpcResult<Option<SignatureHelp>> {
        let Some(state) = self.state_if(|on| on.signature_help).await else {
            return Ok(None);
        };
        Ok(signature::signature_help(&state, &params))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> RpcResult<Option<SemanticTokensResult>> {
        let Some(state) = self.state_if(|on| on.semantic_tokens).await else {
            return Ok(None);
        };
        Ok(document::semantic_tokens_full(&state, &params))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_index::Document;

    #[test]
    fn a_change_on_disk_reaches_an_indexed_file_but_not_an_open_buffer() {
        let mut state = State::default();
        state.workspace.open("file:///indexed.sk", "old");
        state.workspace.open("file:///open.sk", "unsaved");
        state.workspace.open("file:///gone.sk", "x");
        state.workspace.open("file:///locked.sk", "kept");
        state.open_in_editor.insert("file:///open.sk".into());

        apply_disk_changes(
            &mut state,
            vec![
                (
                    "file:///indexed.sk".into(),
                    false,
                    Some(Document::new("file:///indexed.sk", "new")),
                ),
                (
                    "file:///open.sk".into(),
                    false,
                    Some(Document::new("file:///open.sk", "disk")),
                ),
                ("file:///gone.sk".into(), true, None),
                ("file:///locked.sk".into(), false, None),
            ],
        );

        let text = |uri| state.workspace.get(uri).map(Document::text);
        assert_eq!(text("file:///indexed.sk"), Some("new"));
        assert_eq!(text("file:///open.sk"), Some("unsaved"));
        assert_eq!(text("file:///gone.sk"), None);
        assert_eq!(text("file:///locked.sk"), Some("kept"));
    }
}
