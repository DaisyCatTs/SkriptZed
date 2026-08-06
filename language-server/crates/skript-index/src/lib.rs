//! Parses Skript documents and indexes what they declare and use.
//!
//! Everything here works without `docs.json`: functions, commands, options and
//! variables are declared by the script itself, so go-to-definition,
//! find-references, rename and the outline are available offline and instantly.
//! The catalog in `skript-docs` layers *meaning* on top of this.

pub mod folding;
pub mod symbols;
pub mod text;

use std::collections::HashMap;

use tree_sitter::{Parser, Tree};

pub use symbols::{FileSymbols, Reference, Symbol, SymbolKind};
pub use text::{Position, Range};

/// One open or on-disk Skript file, kept parsed.
pub struct Document {
    uri: String,
    text: String,
    tree: Tree,
    symbols: FileSymbols,
}

impl Document {
    pub fn new(uri: impl Into<String>, text: impl Into<String>) -> Self {
        let uri = uri.into();
        let text = text.into();
        let tree = parse(&text, None);
        let symbols = symbols::extract(&tree, &text);
        Self {
            uri,
            text,
            tree,
            symbols,
        }
    }

    /// Replaces the whole document and reparses it.
    ///
    /// The old tree is deliberately **not** handed to the parser. Reusing a tree
    /// is only valid after `Tree::edit` has been told exactly which byte range
    /// changed; passing an unedited tree alongside different text makes
    /// tree-sitter reuse nodes that no longer correspond to anything, and the
    /// result is a silently wrong parse rather than an error.
    ///
    /// A full reparse is affordable: Skript files are small — the largest in
    /// Skript's own repository is under 100 KB — and the grammar parses at
    /// roughly 9 MB/s.
    pub fn update(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.tree = parse(&self.text, None);
        self.symbols = symbols::extract(&self.tree, &self.text);
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn symbols(&self) -> &FileSymbols {
        &self.symbols
    }

    /// The line at `line`, without its terminator.
    pub fn line(&self, line: u32) -> &str {
        self.text.lines().nth(line as usize).unwrap_or("")
    }

    /// Foldable regions, derived from the parse tree.
    pub fn folding_ranges(&self) -> Vec<folding::Fold> {
        folding::ranges(&self.tree, &self.text)
    }

    /// True when the file failed to parse cleanly anywhere.
    pub fn has_errors(&self) -> bool {
        self.tree.root_node().has_error()
    }
}

fn parse(text: &str, old: Option<&Tree>) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_skript::LANGUAGE.into())
        .expect("the bundled Skript grammar must load");
    parser
        .parse(text, old)
        .expect("tree-sitter only returns None when parsing is cancelled")
}

/// Every open document, plus the cross-file lookups the LSP needs.
#[derive(Default)]
pub struct Workspace {
    documents: HashMap<String, Document>,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, uri: impl Into<String>, text: impl Into<String>) {
        let uri = uri.into();
        self.documents.insert(uri.clone(), Document::new(uri, text));
    }

    pub fn update(&mut self, uri: &str, text: impl Into<String>) {
        match self.documents.get_mut(uri) {
            Some(document) => document.update(text),
            None => self.open(uri.to_string(), text),
        }
    }

    pub fn close(&mut self, uri: &str) {
        self.documents.remove(uri);
    }

    pub fn get(&self, uri: &str) -> Option<&Document> {
        self.documents.get(uri)
    }

    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Finds where `name` is declared.
    ///
    /// Scope rules that matter here:
    ///   * a `local function` is only visible in its own file;
    ///   * a `{_local}` variable is only visible in its own file (really only
    ///     in its own trigger, but the index does not model triggers, and
    ///     over-reporting inside one file is far less harmful than missing a
    ///     definition);
    ///   * everything else is workspace-wide.
    pub fn definitions(
        &self,
        kind: SymbolKind,
        name: &str,
        from_uri: &str,
    ) -> Vec<(&Document, &Symbol)> {
        let mut out = Vec::new();
        for document in self.documents.values() {
            let same_file = document.uri() == from_uri;
            for symbol in document.symbols().flat() {
                if !kinds_match(kind, symbol.kind) || symbol.name != name {
                    continue;
                }
                let file_local = matches!(
                    symbol.kind,
                    SymbolKind::LocalFunction | SymbolKind::LocalVariable
                );
                if file_local && !same_file {
                    continue;
                }
                out.push((document, symbol));
            }
        }
        out
    }

    /// Finds every use of `name`, across the workspace where scope allows.
    pub fn references(
        &self,
        kind: SymbolKind,
        name: &str,
        from_uri: &str,
    ) -> Vec<(&Document, &Reference)> {
        let file_local = matches!(kind, SymbolKind::LocalVariable);
        let mut out = Vec::new();
        for document in self.documents.values() {
            if file_local && document.uri() != from_uri {
                continue;
            }
            for reference in &document.symbols().references {
                if kinds_match(kind, reference.kind) && reference.name == name {
                    out.push((document, reference));
                }
            }
        }
        out
    }

    /// Declarations across the whole workspace matching `query`.
    pub fn workspace_symbols(&self, query: &str) -> Vec<(&Document, &Symbol)> {
        let needle = query.to_ascii_lowercase();
        let mut out = Vec::new();
        for document in self.documents.values() {
            for symbol in document.symbols().flat() {
                if needle.is_empty() || symbol.name.to_ascii_lowercase().contains(&needle) {
                    out.push((document, symbol));
                }
            }
        }
        out
    }
}

/// `Function` and `LocalFunction` are the same symbol from a lookup's point of
/// view, as are the two variable scopes.
fn kinds_match(wanted: SymbolKind, found: SymbolKind) -> bool {
    if wanted.is_function() && found.is_function() {
        return true;
    }
    if wanted.is_variable() && found.is_variable() {
        return wanted == found;
    }
    wanted == found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_documents() {
        let mut workspace = Workspace::new();
        workspace.open("file:///a.sk", "on join:\n\tstop\n");
        assert_eq!(workspace.len(), 1);
        workspace.close("file:///a.sk");
        assert!(workspace.is_empty());
    }

    #[test]
    fn resolves_a_function_across_files() {
        let mut workspace = Workspace::new();
        workspace.open("file:///lib.sk", "function helper():\n\treturn 1\n");
        workspace.open("file:///use.sk", "on join:\n\tset {_x} to helper()\n");

        let definitions = workspace.definitions(SymbolKind::Function, "helper", "file:///use.sk");
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].0.uri(), "file:///lib.sk");

        let references = workspace.references(SymbolKind::Function, "helper", "file:///use.sk");
        assert_eq!(references.len(), 1);
    }

    #[test]
    fn a_local_function_is_invisible_from_another_file() {
        let mut workspace = Workspace::new();
        workspace.open("file:///lib.sk", "local function helper():\n\treturn 1\n");
        workspace.open("file:///use.sk", "on join:\n\tset {_x} to helper()\n");

        assert!(workspace
            .definitions(SymbolKind::Function, "helper", "file:///use.sk")
            .is_empty());
        assert_eq!(
            workspace
                .definitions(SymbolKind::Function, "helper", "file:///lib.sk")
                .len(),
            1
        );
    }

    #[test]
    fn local_variables_do_not_leak_between_files() {
        let mut workspace = Workspace::new();
        workspace.open("file:///a.sk", "on join:\n\tset {_temp} to 1\n");
        workspace.open("file:///b.sk", "on quit:\n\tset {_temp} to 2\n");

        let references = workspace.references(SymbolKind::LocalVariable, "temp", "file:///a.sk");
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].0.uri(), "file:///a.sk");
    }

    #[test]
    fn a_global_variable_is_shared_across_files() {
        let mut workspace = Workspace::new();
        workspace.open("file:///a.sk", "on join:\n\tset {score} to 1\n");
        workspace.open("file:///b.sk", "on quit:\n\tadd 1 to {score}\n");

        let references = workspace.references(SymbolKind::GlobalVariable, "score", "file:///a.sk");
        assert_eq!(references.len(), 2);
    }

    #[test]
    fn workspace_symbols_search_by_substring() {
        let mut workspace = Workspace::new();
        workspace.open("file:///a.sk", "function give_apple():\n\treturn 1\n");
        assert_eq!(workspace.workspace_symbols("apple").len(), 1);
        assert!(workspace.workspace_symbols("banana").is_empty());
    }

    #[test]
    fn updating_a_document_refreshes_its_symbols() {
        let mut workspace = Workspace::new();
        workspace.open("file:///a.sk", "function old():\n\treturn 1\n");
        workspace.update("file:///a.sk", "function new_name():\n\treturn 1\n");

        assert!(workspace
            .definitions(SymbolKind::Function, "old", "file:///a.sk")
            .is_empty());
        assert_eq!(
            workspace
                .definitions(SymbolKind::Function, "new_name", "file:///a.sk")
                .len(),
            1
        );
    }
}

/// Recursively collects every `.sk` file under `root`.
///
/// Used to index a project on startup: without it, go-to-definition and
/// find-references would only ever see files the user happens to have open,
/// which in a real script folder is almost none of them.
pub fn discover_scripts(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    /// A script larger than this is not hand-written Skript; skipping it keeps
    /// a stray data dump from stalling startup.
    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    /// Guards against a symlink loop, and against indexing an entire drive if
    /// the user opens `/`.
    const MAX_DEPTH: usize = 24;
    const MAX_FILES: usize = 20_000;

    fn skip(name: &str) -> bool {
        // `-` prefixed scripts are Skript's own "disabled" convention, but they
        // are still valid Skript and still worth indexing — only build and VCS
        // directories are skipped.
        matches!(
            name,
            ".git" | ".svn" | ".hg" | "node_modules" | "target" | "build" | ".zed" | ".vscode"
        )
    }

    fn walk(dir: &std::path::Path, depth: usize, out: &mut Vec<std::path::PathBuf>) {
        if depth > MAX_DEPTH || out.len() >= MAX_FILES {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(kind) = entry.file_type() else {
                continue;
            };

            if kind.is_dir() {
                if !skip(&name) {
                    walk(&path, depth + 1, out);
                }
            } else if kind.is_file()
                && path.extension().is_some_and(|ext| ext == "sk")
                && entry.metadata().is_ok_and(|meta| meta.len() <= MAX_BYTES)
            {
                out.push(path);
            }
        }
    }

    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out.sort();
    out
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_scripts_recursively_and_skips_build_directories() {
        let root = std::env::temp_dir().join("skript-index-discovery-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();

        fs::write(root.join("a.sk"), "on join:\n\tstop\n").unwrap();
        fs::write(root.join("nested/b.sk"), "on quit:\n\tstop\n").unwrap();
        fs::write(root.join("-disabled.sk"), "on load:\n\tstop\n").unwrap();
        fs::write(root.join("notes.txt"), "ignored").unwrap();
        fs::write(root.join("node_modules/c.sk"), "on join:\n\tstop\n").unwrap();

        let found = discover_scripts(&root);
        let names: Vec<String> = found
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        assert!(names.contains(&"a.sk".to_string()));
        assert!(names.contains(&"b.sk".to_string()));
        // Skript disables a script by prefixing `-`; it is still Skript, and
        // still worth indexing so its functions resolve.
        assert!(names.contains(&"-disabled.sk".to_string()));
        assert!(!names.contains(&"notes.txt".to_string()));
        assert!(!names.contains(&"c.sk".to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_directory_yields_nothing_rather_than_panicking() {
        assert!(discover_scripts(std::path::Path::new("/no/such/place")).is_empty());
    }
}
