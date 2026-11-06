//! Checks that need the whole project, and keeping files nobody has open up
//! to date with them.
//!
//! Two jobs:
//!
//! * **Duplicates across files.** Two global functions with the same name and
//!   the same parameter types (Skript 2.17 overloads by type, so a name alone
//!   is no conflict), two commands with the same name, two skript-reflect
//!   syntax definitions with the same pattern. Skript loads one and rejects
//!   the other, in whichever order it reads the files, so both are warned and
//!   each points at the other.
//! * **Closed files.** An edit to one script can break a caller in another
//!   that is not open. After the index changes, the files that reference or
//!   declare what changed are rechecked for the cross-file errors
//!   (`unknown-function`, `function-arity`, duplicates) and published, so the
//!   project diagnostics panel shows them. Only those checks: a closed file's
//!   own layout and syntax were as true before the change as after it, and
//!   rerunning the catalog over hundreds of files would cost far more.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use skript_index::{Document, Range, SymbolKind, Workspace};
use tower_lsp::lsp_types::{self as lsp, Url};

use super::{Diagnostic, Severity};
use crate::calls;
use crate::convert::{to_lsp_range, Encoding};
use crate::server::State;

/// Most closed files checked in one pass. The rest wait for the next pass,
/// which follows at once, so the lock is never held for a whole large
/// project and edits queued behind it go through between passes.
pub const FILES_PER_PASS: usize = 200;

/// What makes two declarations the same one, as far as Skript is concerned.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    /// Name, then parameter types: Skript 2.17 tells overloads apart by type.
    Function(String, Vec<String>),
    /// Without the slash, lower case: Bukkit registers commands that way.
    Command(String),
    /// Kind, property owner and pattern, whitespace and case folded.
    Syntax(String),
}

impl Key {
    /// The name a change to this declaration is known by, for finding the
    /// files it affects: `f:name`, `c:name`, `s:pattern`.
    fn topic(&self) -> String {
        match self {
            Key::Function(name, _) => format!("f:{name}"),
            Key::Command(name) => format!("c:{name}"),
            Key::Syntax(pattern) => format!("s:{pattern}"),
        }
    }
}

/// One declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Site {
    uri: String,
    range: Range,
}

/// What one file declares, keyed for comparison.
fn declarations_of(document: &Document) -> Vec<(Key, Range)> {
    let mut out = Vec::new();
    for symbol in document.symbols().flat() {
        match symbol.kind {
            // A `local function` is invisible to other files, and a
            // signature spliced in from an option cannot be compared.
            SymbolKind::Function if !symbol.detail.contains("{@") => {
                let types = calls::signature_parameters(&symbol.detail)
                    .iter()
                    .map(|parameter| parameter_type(parameter))
                    .collect();
                out.push((
                    Key::Function(symbol.name.clone(), types),
                    symbol.selection_range,
                ));
            }
            SymbolKind::Command => out.push((
                Key::Command(symbol.name.trim_start_matches('/').to_lowercase()),
                symbol.selection_range,
            )),
            _ => {}
        }
    }
    for definition in &document.symbols().syntax {
        if definition.local {
            continue;
        }
        for pattern in &definition.declared {
            let folded = pattern.split_whitespace().collect::<Vec<_>>().join(" ");
            out.push((
                Key::Syntax(format!(
                    "{}:{}:{}",
                    definition.kind.label(),
                    definition.property_of.as_deref().unwrap_or_default(),
                    folded.to_lowercase()
                )),
                definition.selection_range,
            ));
        }
    }
    out
}

/// `amount: number = 1` → `number`; `name?: text` → `text`.
fn parameter_type(parameter: &str) -> String {
    let ty = parameter.split_once(':').map_or("", |(_, ty)| ty);
    let ty = ty.split('=').next().unwrap_or_default();
    ty.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Every declaration in the project, by key. Built once per publish or pass
/// and shared by every file checked in it.
#[derive(Default)]
pub struct Declarations {
    by_key: HashMap<Key, Vec<Site>>,
}

impl Declarations {
    pub fn of(workspace: &Workspace) -> Self {
        let mut by_key: HashMap<Key, Vec<Site>> = HashMap::new();
        for document in workspace.documents() {
            for (key, range) in declarations_of(document) {
                by_key.entry(key).or_default().push(Site {
                    uri: document.uri().to_string(),
                    range,
                });
            }
        }
        Self { by_key }
    }
}

/// A duplicate, with where the others are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub diagnostic: Diagnostic,
    /// Each other declaration: URI and range.
    pub others: Vec<(String, Range)>,
}

/// Duplicates of what `document` declares. Functions and commands are only
/// compared across files, since the same-file check already reports those
/// (as errors); skript-reflect syntax has no same-file check, so a repeat in
/// one file counts too.
pub fn duplicates(document: &Document, declarations: &Declarations) -> Vec<Duplicate> {
    let mut out = Vec::new();
    for (key, range) in declarations_of(document) {
        let Some(sites) = declarations.by_key.get(&key) else {
            continue;
        };
        let others: Vec<&Site> = sites
            .iter()
            .filter(|site| match key {
                Key::Syntax(_) => !(site.uri == document.uri() && site.range == range),
                _ => site.uri != document.uri(),
            })
            .collect();
        if others.is_empty() {
            continue;
        }
        let (code, message) = match &key {
            Key::Function(name, types) => (
                "duplicate-function",
                format!(
                    "Another script also declares `{name}({})`. Skript loads only one of them \
                     and rejects the other.",
                    types.join(", ")
                ),
            ),
            Key::Command(name) => (
                "duplicate-command",
                format!(
                    "Another script also declares the command `/{name}`. Skript registers only \
                     one of them."
                ),
            ),
            Key::Syntax(_) => (
                "duplicate-syntax",
                "This pattern is already defined by another skript-reflect definition, and \
                 only one of them can be used."
                    .to_string(),
            ),
        };
        out.push(Duplicate {
            diagnostic: Diagnostic {
                range,
                severity: Severity::Warning,
                message,
                code,
                related: Vec::new(),
            },
            others: others
                .into_iter()
                .map(|site| (site.uri.clone(), site.range))
                .collect(),
        });
    }
    out
}

/// A duplicate as an LSP diagnostic, each other declaration a link to it.
fn duplicate_to_lsp(
    duplicate: Duplicate,
    document: &Document,
    workspace: &Workspace,
    url: &Url,
    encoding: Encoding,
) -> lsp::Diagnostic {
    let related: Vec<lsp::DiagnosticRelatedInformation> = duplicate
        .others
        .iter()
        .filter_map(|(uri, range)| {
            let other = workspace.get(uri)?;
            Some(lsp::DiagnosticRelatedInformation {
                location: lsp::Location {
                    uri: Url::parse(uri).ok()?,
                    range: to_lsp_range(other.text(), *range, encoding),
                },
                message: "also declared here".into(),
            })
        })
        .collect();
    let mut converted =
        crate::publish::to_lsp_diagnostic(document.text(), url, duplicate.diagnostic, encoding);
    converted.related_information = (!related.is_empty()).then_some(related);
    converted
}

/// The cross-file duplicates of an open document, for its ordinary publish.
pub fn duplicate_diagnostics(
    state: &State,
    document: &Document,
    url: &Url,
) -> Vec<lsp::Diagnostic> {
    let declarations = Declarations::of(&state.workspace);
    duplicates(document, &declarations)
        .into_iter()
        .map(|duplicate| {
            duplicate_to_lsp(duplicate, document, &state.workspace, url, state.encoding)
        })
        .collect()
}

/// What a closed file is told: its cross-file errors and duplicates.
fn closed_file_diagnostics(
    state: &State,
    document: &Document,
    url: &Url,
    shared: &(Declarations, super::CrossFile<'_>),
) -> Vec<lsp::Diagnostic> {
    let (declarations, cross_file) = shared;
    let mut found: Vec<lsp::Diagnostic> = Vec::new();
    if state.diagnostics.project_indexed {
        found.extend(
            super::check_cross_file(document, cross_file)
                .into_iter()
                .map(|diagnostic| {
                    crate::publish::to_lsp_diagnostic(
                        document.text(),
                        url,
                        diagnostic,
                        state.encoding,
                    )
                }),
        );
    }
    found.extend(
        duplicates(document, declarations)
            .into_iter()
            .map(|duplicate| {
                duplicate_to_lsp(duplicate, document, &state.workspace, url, state.encoding)
            }),
    );
    found.sort_by_key(|diagnostic| {
        (
            diagnostic.range.start.line,
            diagnostic.range.start.character,
        )
    });
    found
}

/// Whether project diagnostics are switched on.
pub fn enabled(state: &State) -> bool {
    state.settings.features.diagnostics && state.settings.features.project_diagnostics
}

/// Whether `uri` is a file under one of the workspace roots. A script opened
/// from elsewhere stays in the index after its tab closes, and must never be
/// published for: it is not part of the project the panel describes.
fn in_workspace(uri: &str, roots: &[PathBuf]) -> bool {
    let Some(path) = Url::parse(uri).ok().and_then(|url| url.to_file_path().ok()) else {
        return false;
    };
    roots.iter().any(|root| path.starts_with(root))
}

/// The topics a file's declarations are known by, as of its current text.
fn provided(document: &Document) -> HashSet<String> {
    declarations_of(document)
        .into_iter()
        .map(|(key, _)| match &key {
            // Changing a signature changes what callers can be told, so the
            // types are part of what is compared, not of the topic.
            Key::Function(name, types) => format!("f:{name}({})", types.join(",")),
            _ => key.topic(),
        })
        .collect()
}

/// `f:name(types)` → `f:name`; other topics are already bare.
fn topic_of(provided: &str) -> &str {
    match provided.find('(') {
        Some(open) if provided.starts_with("f:") => &provided[..open],
        _ => provided,
    }
}

/// The bookkeeping between passes.
#[derive(Default)]
pub struct ProjectState {
    /// What each closed file was last sent, so a pass sends only what
    /// changed, and a file that became clean is cleared rather than left.
    published: HashMap<String, Vec<lsp::Diagnostic>>,
    /// Each file's declarations as of the last pass, to tell what an edit
    /// changed.
    provides: HashMap<String, HashSet<String>>,
    /// Files whose text changed since the last pass.
    dirty: HashSet<String>,
    /// Closed files waiting to be checked: deferred by [`FILES_PER_PASS`], or
    /// just closed.
    pending: BTreeSet<String>,
    /// The next pass checks every closed file: the index just arrived, or
    /// the feature was switched on.
    full: bool,
    /// Bumped by every schedule; a debounced pass that wakes to a newer one
    /// gives way to it.
    pub generation: u64,
    /// Held for the length of a pass, so two never interleave and publish out
    /// of order.
    pub pass: Arc<tokio::sync::Mutex<()>>,
}

impl ProjectState {
    /// `uri`'s text changed, or it appeared or went away.
    pub fn changed(&mut self, uri: &str) {
        self.dirty.insert(uri.to_string());
    }

    /// Check every closed file on the next pass.
    pub fn everything(&mut self) {
        self.full = true;
    }

    /// `uri` was closed: it now needs its own project diagnostics.
    pub fn closed(&mut self, uri: &str) {
        self.pending.insert(uri.to_string());
    }

    /// `uri` was opened: its ordinary publish replaces whatever was sent.
    pub fn opened(&mut self, uri: &str) {
        self.published.remove(uri);
        self.pending.remove(uri);
    }

    /// Forgets everything sent, returning the files to clear: the feature
    /// was switched off.
    pub fn clear(&mut self) -> Vec<String> {
        self.pending.clear();
        self.dirty.clear();
        self.full = false;
        self.published.drain().map(|(uri, _)| uri).collect()
    }

    /// Whether anything is waiting for a pass.
    pub fn has_work(&self) -> bool {
        self.full || !self.dirty.is_empty() || !self.pending.is_empty()
    }

    /// Takes the work for one pass.
    pub fn take_work(&mut self) -> Work {
        Work {
            dirty: std::mem::take(&mut self.dirty),
            pending: std::mem::take(&mut self.pending),
            full: std::mem::take(&mut self.full),
        }
    }

    /// Records a pass and returns what to send: only diagnostics that differ
    /// from what the file was last sent.
    pub fn apply(&mut self, outcome: PassOutcome) -> Vec<(String, Vec<lsp::Diagnostic>)> {
        for (uri, provides) in outcome.provides {
            match provides {
                Some(provides) => self.provides.insert(uri, provides),
                None => self.provides.remove(&uri),
            };
        }
        self.pending.extend(outcome.deferred);
        let mut send = Vec::new();
        for (uri, found) in outcome.found {
            let before = self.published.get(&uri);
            let unchanged = match before {
                Some(before) => *before == found,
                None => found.is_empty(),
            };
            if unchanged {
                continue;
            }
            if found.is_empty() {
                self.published.remove(&uri);
            } else {
                self.published.insert(uri.clone(), found.clone());
            }
            send.push((uri, found));
        }
        send
    }
}

/// One pass's input, taken from [`ProjectState`].
pub struct Work {
    dirty: HashSet<String>,
    pending: BTreeSet<String>,
    full: bool,
}

/// What a pass found.
pub struct PassOutcome {
    /// New declaration topics per file; `None` for a file that is gone.
    provides: Vec<(String, Option<HashSet<String>>)>,
    /// Diagnostics per file checked (empty to clear it).
    found: Vec<(String, Vec<lsp::Diagnostic>)>,
    /// Over [`FILES_PER_PASS`]; checked by the next pass.
    deferred: Vec<String>,
    /// How many files were checked, for the log and the measurements.
    pub checked: usize,
}

impl PassOutcome {
    pub fn has_deferred(&self) -> bool {
        !self.deferred.is_empty()
    }
}

/// Runs one pass against the state as it is now. Read-only, so it runs under
/// the read lock; [`ProjectState::apply`] records it afterwards.
pub fn run_pass(state: &State, work: Work, limit: usize) -> PassOutcome {
    let project = &state.project;
    let eligible = |uri: &str| {
        !state.open_in_editor.contains(uri)
            && state.workspace.get(uri).is_some()
            && in_workspace(uri, &state.roots)
    };

    let mut provides = Vec::new();
    let mut changed_topics: HashSet<String> = HashSet::new();
    let mut candidates: BTreeSet<String> = work.pending;

    if work.full {
        for document in state.workspace.documents() {
            provides.push((document.uri().to_string(), Some(provided(document))));
            candidates.insert(document.uri().to_string());
        }
    }
    for uri in &work.dirty {
        let now = state.workspace.get(uri).map(provided);
        let before = project.provides.get(uri);
        let empty = HashSet::new();
        let (now_set, before_set) = (now.as_ref().unwrap_or(&empty), before.unwrap_or(&empty));
        for topic in now_set.symmetric_difference(before_set) {
            changed_topics.insert(topic_of(topic).to_string());
        }
        if !work.full {
            provides.push((uri.clone(), now));
        }
        // A closed file whose own text changed (on disk) is rechecked whatever
        // it declares: its calls may be what changed.
        candidates.insert(uri.clone());
    }

    if !changed_topics.is_empty() && !work.full {
        let changed_functions: HashSet<&str> = changed_topics
            .iter()
            .filter_map(|topic| topic.strip_prefix("f:"))
            .collect();
        for document in state.workspace.documents() {
            let calls_one = document.symbols().references.iter().any(|reference| {
                reference.kind == SymbolKind::Function
                    && changed_functions.contains(reference.name.as_str())
            });
            let declares_one = || {
                declarations_of(document)
                    .iter()
                    .any(|(key, _)| changed_topics.contains(&key.topic()))
            };
            if calls_one || declares_one() {
                candidates.insert(document.uri().to_string());
            }
        }
    }

    let mut found = Vec::new();
    let mut deferred = Vec::new();
    let mut checked = 0;
    // Built once for the pass, on the first file that needs checking.
    let mut shared: Option<(Declarations, super::CrossFile<'_>)> = None;
    for uri in candidates {
        if !eligible(&uri) {
            // Gone from the index, or never ours: clear whatever it was sent.
            // An open file is left alone; its own publish owns it.
            if project.published.contains_key(&uri) && !state.open_in_editor.contains(&uri) {
                found.push((uri, Vec::new()));
            }
            continue;
        }
        if checked == limit {
            deferred.push(uri);
            continue;
        }
        let (Some(document), Ok(url)) = (state.workspace.get(&uri), Url::parse(&uri)) else {
            continue;
        };
        let shared = shared.get_or_insert_with(|| {
            (
                Declarations::of(&state.workspace),
                super::CrossFile::new(&state.workspace, state.catalog.as_ref()),
            )
        });
        found.push((
            uri.clone(),
            closed_file_diagnostics(state, document, &url, shared),
        ));
        checked += 1;
    }

    PassOutcome {
        provides,
        found,
        deferred,
        checked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from("C:\\project")
        } else {
            PathBuf::from("/project")
        }
    }

    fn uri(name: &str) -> String {
        Url::from_file_path(root().join(name)).unwrap().to_string()
    }

    fn state(files: &[(&str, &str)]) -> State {
        let mut state = State {
            roots: vec![root()],
            ..State::default()
        };
        state.diagnostics.project_indexed = true;
        for (name, text) in files {
            state.workspace.open(uri(name), *text);
        }
        state
    }

    fn codes_in(found: &[lsp::Diagnostic]) -> Vec<String> {
        found
            .iter()
            .filter_map(|diagnostic| match &diagnostic.code {
                Some(lsp::NumberOrString::String(code)) => Some(code.clone()),
                _ => None,
            })
            .collect()
    }

    fn duplicate_codes(state: &State, name: &str) -> Vec<&'static str> {
        let declarations = Declarations::of(&state.workspace);
        duplicates(state.workspace.get(&uri(name)).unwrap(), &declarations)
            .into_iter()
            .map(|duplicate| duplicate.diagnostic.code)
            .collect()
    }

    #[test]
    fn a_function_is_duplicated_only_by_the_same_name_and_parameter_types() {
        let state = state(&[
            ("a.sk", "function heal(p: player, n: number = 1):\n\tstop\n"),
            (
                "b.sk",
                "function heal(target: player, amount: number):\n\tstop\n",
            ),
            ("c.sk", "function heal(p: player):\n\tstop\n"),
            (
                "d.sk",
                "local function heal(p: player, n: number):\n\tstop\n",
            ),
        ]);
        assert_eq!(duplicate_codes(&state, "a.sk"), vec!["duplicate-function"]);
        assert_eq!(duplicate_codes(&state, "b.sk"), vec!["duplicate-function"]);
        // An overload by type, and a local function, are no conflict.
        assert!(duplicate_codes(&state, "c.sk").is_empty());
        assert!(duplicate_codes(&state, "d.sk").is_empty());
    }

    #[test]
    fn commands_and_reflect_patterns_are_duplicated_across_files() {
        let state = state(&[
            ("a.sk", "command /Home:\n\ttrigger:\n\t\tstop\n"),
            ("b.sk", "command home:\n\ttrigger:\n\t\tstop\n"),
            (
                "c.sk",
                "effect greet %player%:\n\ttrigger:\n\t\tstop\n\
                 effect   greet %player%:\n\ttrigger:\n\t\tstop\n",
            ),
        ]);
        assert_eq!(duplicate_codes(&state, "a.sk"), vec!["duplicate-command"]);
        assert_eq!(
            duplicate_codes(&state, "c.sk"),
            vec!["duplicate-syntax", "duplicate-syntax"]
        );
    }

    #[test]
    fn a_duplicate_links_to_the_other_file() {
        let state = state(&[
            ("a.sk", "command /home:\n\ttrigger:\n\t\tstop\n"),
            ("b.sk", "\n\ncommand /home:\n\ttrigger:\n\t\tstop\n"),
        ]);
        let document = state.workspace.get(&uri("a.sk")).unwrap();
        let url = Url::parse(&uri("a.sk")).unwrap();
        let found = duplicate_diagnostics(&state, document, &url);
        let related = found[0].related_information.as_ref().unwrap();
        assert_eq!(related[0].location.uri.as_str(), uri("b.sk"));
        assert_eq!(related[0].location.range.start.line, 2);
        assert_eq!(found[0].severity, Some(lsp::DiagnosticSeverity::WARNING));
    }

    /// Runs passes until nothing is left, returning everything sent.
    fn settle(state: &mut State) -> HashMap<String, Vec<String>> {
        let mut sent = HashMap::new();
        while state.project.has_work() {
            let work = state.project.take_work();
            let outcome = run_pass(state, work, 2);
            for (uri, found) in state.project.apply(outcome) {
                sent.insert(uri, codes_in(&found));
            }
        }
        sent
    }

    #[test]
    fn breaking_a_function_reaches_its_closed_callers_and_fixing_it_clears_them() {
        let mut state = state(&[
            ("lib.sk", "function greet(p: player):\n\tstop\n"),
            ("a.sk", "on join:\n\tgreet(player)\n"),
            ("b.sk", "on quit:\n\tgreet(player)\n"),
            ("c.sk", "on death:\n\tstop\n"),
        ]);
        state.open_in_editor.insert(uri("lib.sk"));
        state.project.everything();
        assert!(
            settle(&mut state).is_empty(),
            "a clean project sends nothing"
        );

        // The open library loses its function.
        state.workspace.update(&uri("lib.sk"), "on load:\n\tstop\n");
        state.project.changed(&uri("lib.sk"));
        let sent = settle(&mut state);
        assert_eq!(
            sent.get(&uri("a.sk")),
            Some(&vec!["unknown-function".into()])
        );
        assert_eq!(
            sent.get(&uri("b.sk")),
            Some(&vec!["unknown-function".into()])
        );
        assert!(
            !sent.contains_key(&uri("c.sk")),
            "unrelated files are not touched"
        );
        assert!(
            !sent.contains_key(&uri("lib.sk")),
            "an open file is its own publish's"
        );

        // Now with two parameters: the callers' arity is wrong instead.
        state.workspace.update(
            &uri("lib.sk"),
            "function greet(p: player, n: number):\n\tstop\n",
        );
        state.project.changed(&uri("lib.sk"));
        let sent = settle(&mut state);
        assert_eq!(sent.get(&uri("a.sk")), Some(&vec!["function-arity".into()]));

        // Fixed: both are cleared.
        state
            .workspace
            .update(&uri("lib.sk"), "function greet(p: player):\n\tstop\n");
        state.project.changed(&uri("lib.sk"));
        let sent = settle(&mut state);
        assert_eq!(sent.get(&uri("a.sk")), Some(&Vec::new()));
        assert_eq!(sent.get(&uri("b.sk")), Some(&Vec::new()));
    }

    #[test]
    fn a_deleted_file_is_cleared_and_files_outside_the_workspace_are_never_sent() {
        let mut state = state(&[("a.sk", "on join:\n\tmissing()\n")]);
        let outside = Url::from_file_path(if cfg!(windows) {
            "C:\\elsewhere\\x.sk"
        } else {
            "/elsewhere/x.sk"
        })
        .unwrap()
        .to_string();
        state
            .workspace
            .open(outside.clone(), "on join:\n\tmissing()\n");
        state.project.everything();
        let sent = settle(&mut state);
        assert_eq!(
            sent.get(&uri("a.sk")),
            Some(&vec!["unknown-function".into()])
        );
        assert!(!sent.contains_key(&outside));

        state.workspace.close(&uri("a.sk"));
        state.project.changed(&uri("a.sk"));
        let sent = settle(&mut state);
        assert_eq!(sent.get(&uri("a.sk")), Some(&Vec::new()));
    }

    #[test]
    fn a_pass_checks_at_most_its_limit_and_defers_the_rest() {
        let files: Vec<(String, &str)> = (0..5)
            .map(|n| (format!("f{n}.sk"), "on join:\n\tmissing()\n"))
            .collect();
        let borrowed: Vec<(&str, &str)> = files.iter().map(|(n, t)| (n.as_str(), *t)).collect();
        let mut state = state(&borrowed);
        state.project.everything();
        let work = state.project.take_work();
        let outcome = run_pass(&state, work, 2);
        assert_eq!(outcome.checked, 2);
        assert!(outcome.has_deferred());
        state.project.apply(outcome);
        assert_eq!(settle(&mut state).len(), 3, "the rest follow");
    }

    #[test]
    fn opening_a_file_hands_it_back_to_its_own_publish() {
        let mut state = state(&[("a.sk", "on join:\n\tmissing()\n")]);
        state.project.everything();
        settle(&mut state);
        state.project.opened(&uri("a.sk"));
        state.open_in_editor.insert(uri("a.sk"));
        // Closing it again sends its project diagnostics afresh.
        state.open_in_editor.remove(&uri("a.sk"));
        state.project.closed(&uri("a.sk"));
        let sent = settle(&mut state);
        assert_eq!(
            sent.get(&uri("a.sk")),
            Some(&vec!["unknown-function".into()])
        );
    }

    /// The cost of a full pass on a synthetic 300-file project. Run with
    /// `cargo test --release -- --ignored --nocapture project_pass_cost`.
    #[test]
    #[ignore]
    fn project_pass_cost() {
        let mut files = Vec::new();
        for n in 0..300 {
            let mut text = String::new();
            for f in 0..5 {
                text.push_str(&format!(
                    "function f{n}_{f}(p: player, amount: number = 1) :: number:\n\
                     \tset {{_x}} to {{_amount}} * 2\n\treturn {{_x}}\n\n"
                ));
            }
            text.push_str(&format!("command /cmd{n}:\n\ttrigger:\n"));
            for call in 0..10 {
                let target = (n * 7 + call) % 300;
                text.push_str(&format!(
                    "\t\tset {{_r}} to f{target}_{}(player, 2)\n",
                    call % 5
                ));
            }
            // Every tenth file calls one hot function, the one edited below.
            if n % 10 == 0 {
                text.push_str("\t\tset {_hot} to f0_0(player)\n");
            }
            text.push_str("\non join:\n\tbroadcast \"hi\"\n\twait 1 second\n");
            files.push((format!("s{n}.sk"), text));
        }
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        let mut state = state(&borrowed);
        // The real database when present, for its function list.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../vendor/docs.json");
        let docs = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| skript_docs::Docs::parse(&text).ok())
            .unwrap_or_else(skript_docs::fallback_docs);
        state.catalog = Some(skript_docs::Catalog::build(docs));

        state.project.everything();
        let started = std::time::Instant::now();
        let mut passes = 0;
        while state.project.has_work() {
            let work = state.project.take_work();
            let outcome = run_pass(&state, work, FILES_PER_PASS);
            state.project.apply(outcome);
            passes += 1;
        }
        let full = started.elapsed();

        // One edit that changes a declaration ten files call.
        let edited = uri("s0.sk");
        state.open_in_editor.insert(edited.clone());
        let text = state
            .workspace
            .get(&edited)
            .unwrap()
            .text()
            .replace("f0_0(p: player,", "f0_0(");
        state.workspace.update(&edited, text);
        state.project.changed(&edited);
        let started = std::time::Instant::now();
        let work = state.project.take_work();
        let outcome = run_pass(&state, work, FILES_PER_PASS);
        let rechecked = outcome.checked;
        state.project.apply(outcome);
        let incremental = started.elapsed();

        // A keystroke that changes no declaration.
        state.project.changed(&edited);
        let started = std::time::Instant::now();
        let work = state.project.take_work();
        let outcome = run_pass(&state, work, FILES_PER_PASS);
        state.project.apply(outcome);
        let keystroke = started.elapsed();

        println!(
            "300 files: full {full:?} in {passes} passes; declaration edit {incremental:?} \
             ({rechecked} files); keystroke {keystroke:?}"
        );
    }
}
