//! Reading the project's scripts off disk at startup.

use std::path::PathBuf;

use skript_index::Document;
use tower_lsp::lsp_types::{MessageType, Url};

use crate::server::Backend;

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
        });
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
