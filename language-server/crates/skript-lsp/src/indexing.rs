//! Reading the project's scripts off disk at startup, and keeping the
//! diagnostics of the scripts nobody has open in step with the index.

use std::path::PathBuf;
use std::time::Duration;

use skript_index::Document;
use tower_lsp::lsp_types::{MessageType, Url};

use crate::diagnostics::project::{self, FILES_PER_PASS};
use crate::server::Backend;

/// How long the index must stay still before closed files are rechecked.
/// Typing changes it on every keystroke, and a caller broken mid-word is not
/// worth a panel entry that the next keystroke takes back.
const PROJECT_DEBOUNCE: Duration = Duration::from_millis(500);

impl Backend {
    /// Indexes every script in the project, not only the open files. Without
    /// this, go-to-definition and find-references silently miss most of a
    /// real script folder.
    pub async fn spawn_project_index(&self, roots: Vec<PathBuf>) {
        if roots.is_empty() {
            // Single-file mode: there is no folder to read, so nothing will
            // ever arrive to change the answer and the check is valid now.
            self.state.write().await.diagnostics.project_indexed = true;
            return;
        }
        let backend = self.handle();
        tokio::spawn(async move {
            let scripts = tokio::task::spawn_blocking(move || read_scripts(&roots))
                .await
                .unwrap_or_default();

            let count = scripts.len();
            {
                let mut state = backend.state.write().await;
                state.diagnostics.project_indexed = true;
                state.project.everything();
                for document in scripts {
                    // An open document is authoritative: it may carry unsaved
                    // edits the file on disk does not.
                    state.workspace.insert_if_absent(document);
                }
            }
            backend
                .client
                .log_message(MessageType::INFO, format!("indexed {count} script(s)"))
                .await;

            // An editor opens its buffer the instant the server starts, long
            // before several hundred files have been read off disk.
            // Diagnostics published in that window cannot see a function
            // declared in a file nobody has opened, so the first thing a user
            // saw on opening a real project was a false "No function named `x`
            // is declared in this project", which then stayed until they
            // edited the file, because nothing republished afterwards.
            backend.republish_open().await;
            backend.schedule_project_diagnostics().await;
        });
    }

    /// Rechecks the closed files an index change may affect, once the index
    /// has been still for [`PROJECT_DEBOUNCE`]. See [`project`].
    pub async fn schedule_project_diagnostics(&self) {
        let generation = {
            let mut state = self.state.write().await;
            if !project::enabled(&state) || !state.project.has_work() {
                return;
            }
            state.project.generation += 1;
            state.project.generation
        };
        let backend = self.handle();
        tokio::spawn(async move {
            tokio::time::sleep(PROJECT_DEBOUNCE).await;
            // A later change rescheduled: that timer runs the pass instead.
            if backend.state.read().await.project.generation == generation {
                backend.project_pass().await;
            }
        });
    }

    /// Runs passes until the work is done, [`FILES_PER_PASS`] files at a
    /// time, letting queued requests in between.
    async fn project_pass(&self) {
        let lock = self.state.read().await.project.pass.clone();
        let _held = lock.lock().await;
        loop {
            let work = {
                let mut state = self.state.write().await;
                if !project::enabled(&state) || !state.project.has_work() {
                    return;
                }
                state.project.take_work()
            };
            let outcome = {
                let state = self.state.read().await;
                project::run_pass(&state, work, FILES_PER_PASS)
            };
            let more = outcome.has_deferred();
            let checked = outcome.checked;
            let send: Vec<(String, Vec<tower_lsp::lsp_types::Diagnostic>)> = {
                let mut guard = self.state.write().await;
                let state = &mut *guard;
                let send = state.project.apply(outcome);
                // Opened while the pass ran: its own publish owns it now.
                send.into_iter()
                    .filter(|(uri, _)| {
                        let open = state.open_in_editor.contains(uri);
                        if open {
                            state.project.opened(uri);
                        }
                        !open
                    })
                    .collect()
            };
            if checked > 0 {
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!(
                            "rechecked {checked} closed script(s), {} changed",
                            send.len()
                        ),
                    )
                    .await;
            }
            for (uri, found) in send {
                if let Ok(url) = Url::parse(&uri) {
                    self.client.publish_diagnostics(url, found, None).await;
                }
            }
            if !more {
                return;
            }
            tokio::task::yield_now().await;
        }
    }
}

/// Every script under `roots`, parsed.
///
/// Parsed here, on the blocking pool, rather than under the state's write
/// lock: a few hundred files take most of a second, and every request waited
/// on that lock meanwhile.
fn read_scripts(roots: &[PathBuf]) -> Vec<Document> {
    let mut found = Vec::new();
    for root in roots {
        for path in skript_index::discover_scripts(root) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(uri) = Url::from_file_path(&path) else {
                continue;
            };
            found.push(Document::new(uri.to_string(), text));
        }
    }
    found
}
