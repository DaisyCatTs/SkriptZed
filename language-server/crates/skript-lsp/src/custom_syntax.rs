//! The project's own skript-reflect syntax, fed into the catalog.
//!
//! `effect greet %player%:` defines a line of the language. Once it is in the
//! catalog, `greet player` classifies, colours, completes and hovers exactly
//! like a core effect, and go-to-definition can find its way back here.
//!
//! The definitions change as the user types, so this has to be cheap on every
//! keystroke. [`sync`] walks the top-level definitions the index already
//! extracted (no re-parsing), hashes what matters, and only touches the
//! catalog when that hash moves. Installing a new set rebuilds a small side
//! index, never the main one; see `Catalog::set_workspace_entries`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use skript_docs::model::AddonRef;
use skript_docs::{Catalog, Category, Entry, EntryId, Reference};
use skript_index::{Document, SyntaxDefinition, SyntaxKind, Workspace};

use crate::server::{Backend, State};

/// How the catalog attributes the project's syntax, where an addon's name
/// would otherwise go.
pub const PROVIDER: &str = "this project";

/// Brings the catalog's copy of the project's syntax up to date.
///
/// Returns whether anything changed, in which case every open document's
/// diagnostics and colours may be stale.
pub fn sync(state: &mut State) -> bool {
    let Some(catalog) = state.catalog.as_mut() else {
        return false;
    };
    let definitions = all_definitions(&state.workspace);
    let tag = tag_of(&definitions);
    if catalog.workspace_tag() == Some(tag) {
        return false;
    }
    // A fresh catalog has no tag; installing an empty set into it changes
    // nothing anybody can see.
    let changed = !definitions.is_empty() || catalog.workspace_entries().next().is_some();
    let entries = definitions.iter().map(|(document, definition)| {
        (
            category_of(definition.kind),
            entry_for(document.uri(), definition),
        )
    });
    catalog.set_workspace_entries(entries.collect::<Vec<_>>(), tag);
    changed
}

/// Every definition in the workspace, ordered by file and then by position.
fn all_definitions(workspace: &Workspace) -> Vec<(&Document, &SyntaxDefinition)> {
    let mut found: Vec<(&Document, &SyntaxDefinition)> = workspace
        .documents()
        .flat_map(|document| {
            document
                .symbols()
                .syntax
                .iter()
                .map(move |definition| (document, definition))
        })
        .collect();
    // Stable, so each file keeps its own source order.
    found.sort_by(|a, b| a.0.uri().cmp(b.0.uri()));
    found
}

/// A hash of everything an entry is built from.
///
/// Positions are left out on purpose: typing a line above a definition moves
/// it without changing what it means, and must not rebuild anything.
fn tag_of(definitions: &[(&Document, &SyntaxDefinition)]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (document, definition) in definitions {
        document.uri().hash(&mut hasher);
        definition.kind.hash(&mut hasher);
        definition.declared.hash(&mut hasher);
        definition.patterns.hash(&mut hasher);
        definition.local.hash(&mut hasher);
        definition.property_of.hash(&mut hasher);
        definition.event_id.hash(&mut hasher);
        definition.event_values.hash(&mut hasher);
        definition.return_type.hash(&mut hasher);
        definition.comment.hash(&mut hasher);
    }
    hasher.finish()
}

fn category_of(kind: SyntaxKind) -> Category {
    match kind {
        SyntaxKind::Effect => Category::Effect,
        SyntaxKind::Expression => Category::Expression,
        SyntaxKind::Condition => Category::Condition,
        SyntaxKind::Event => Category::Event,
    }
}

/// The catalog entry describing one definition.
///
/// `id` carries [`SyntaxDefinition::key`], which is how a classified line
/// finds its way back to the definition; [`card`] hides it from the hover.
fn entry_for(uri: &str, definition: &SyntaxDefinition) -> Entry {
    let file = uri.rsplit('/').next().unwrap_or(uri);
    let mut description = Vec::new();
    if !definition.comment.is_empty() {
        description.push(definition.comment.join(" "));
    }
    let scope = if definition.local {
        " Local: only usable in that script."
    } else {
        ""
    };
    description.push(format!(
        "{} defined with skript-reflect in `{file}`.{scope}",
        capitalise(&definition.describe())
    ));
    Entry {
        id: definition.key(),
        name: definition.name(),
        description,
        patterns: definition.patterns.clone(),
        returns: definition.return_type.as_ref().map(|returns| Reference {
            id: returns.clone(),
            name: returns.clone(),
        }),
        event_values: definition
            .event_values
            .iter()
            .map(|value| Reference {
                id: value.clone(),
                name: value.clone(),
            })
            .collect(),
        addon: Some(AddonRef {
            name: PROVIDER.into(),
            url: String::new(),
            since_version: None,
        }),
        ..Default::default()
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The hover card for one of the project's entries.
///
/// The usual card ends with the entry's id, which here is an internal key
/// (`effect::greet %player%`) that means nothing to the reader.
pub fn card(category: Category, entry: &Entry) -> String {
    let shown = Entry {
        id: String::new(),
        ..entry.clone()
    };
    skript_docs::hover::render(category, &shown)
}

/// Where the project syntax `id` is defined.
///
/// A `local` definition counts only in its own file, so when the file asking
/// defines it locally, that one wins outright.
pub fn definitions_of<'w>(
    workspace: &'w Workspace,
    catalog: &Catalog,
    id: EntryId,
    from_uri: &str,
) -> Vec<(&'w Document, &'w SyntaxDefinition)> {
    if !catalog.is_workspace(id) {
        return Vec::new();
    }
    let Some(key) = catalog.entry(id).map(|entry| entry.id.as_str()) else {
        return Vec::new();
    };
    let found: Vec<(&Document, &SyntaxDefinition)> = all_definitions(workspace)
        .into_iter()
        .filter(|(document, definition)| {
            definition.key() == key && (!definition.local || document.uri() == from_uri)
        })
        .collect();
    let own: Vec<_> = found
        .iter()
        .copied()
        .filter(|(document, _)| document.uri() == from_uri)
        .collect();
    if own.is_empty() {
        found
    } else {
        own
    }
}

impl Backend {
    /// Updates the project's syntax in the catalog, then tells the client its
    /// colours may be stale. Returns whether anything changed.
    pub async fn sync_custom_syntax(&self) -> bool {
        let changed = sync(&mut *self.state.write().await);
        if changed {
            // Another open file's uses of an edited pattern were coloured
            // against the old one. Spawned, because it is a request to the
            // client and nothing here needs to wait for its answer; a client
            // without refresh support answers with an error, which is fine.
            let client = self.client.clone();
            tokio::spawn(async move {
                let _ = client.semantic_tokens_refresh().await;
            });
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = concat!(
        "# Says hello.\n",
        "effect [the] greet %player%:\n",
        "\ttrigger:\n",
        "\t\tsend \"hi\" to expr-1\n",
        "\n",
        "on join:\n",
        "\tgreet player\n",
    );

    fn state(source: &str) -> State {
        let mut state = State {
            catalog: Some(Catalog::build(skript_docs::fallback_docs())),
            ..Default::default()
        };
        state.workspace.open("file:///dir/custom.sk", source);
        state
    }

    #[test]
    fn a_definition_reaches_the_catalog() {
        let mut state = state(SOURCE);
        assert!(sync(&mut state));
        let catalog = state.catalog.as_ref().unwrap();
        let (id, _) = catalog
            .classify_line("greet player", skript_docs::LineRole::Statement)
            .expect("the custom effect classifies");
        let entry = catalog.entry(id).unwrap();
        assert_eq!(entry.name, "[the] greet %player%");
        assert_eq!(entry.provider(), PROVIDER);
        assert!(entry.description[0].contains("Says hello."));
        assert!(
            entry.description[1].contains("`custom.sk`"),
            "{:?}",
            entry.description
        );

        let card = card(id.category, entry);
        assert!(!card.contains("effect::"), "{card}");

        let found = definitions_of(&state.workspace, catalog, id, "file:///dir/other.sk");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.selection_range.start.line, 1);
    }

    /// Moving a definition without changing it must not rebuild anything:
    /// that is the per-keystroke path.
    #[test]
    fn an_unchanged_set_is_left_alone() {
        let mut state = state(SOURCE);
        assert!(sync(&mut state));
        assert!(!sync(&mut state));
        state
            .workspace
            .update("file:///dir/custom.sk", format!("\n\n{SOURCE}"));
        assert!(!sync(&mut state), "a moved definition rebuilt the overlay");

        state.workspace.update(
            "file:///dir/custom.sk",
            SOURCE.replace("greet %player%", "wave at %player%"),
        );
        assert!(sync(&mut state));
        let catalog = state.catalog.as_ref().unwrap();
        assert!(catalog
            .classify_line("greet player", skript_docs::LineRole::Statement)
            .is_none());
        assert!(catalog
            .classify_line("wave at player", skript_docs::LineRole::Statement)
            .is_some());
    }

    #[test]
    fn a_project_without_custom_syntax_reports_no_change() {
        let mut state = state("on join:\n\tstop\n");
        assert!(!sync(&mut state));
    }

    /// A local definition elsewhere is not visible; the asking file's own wins.
    #[test]
    fn a_local_definition_stays_in_its_file() {
        let mut state = state("local effect ping:\n\ttrigger:\n\t\tstop\n");
        state.workspace.open(
            "file:///dir/b.sk",
            "local effect ping:\n\ttrigger:\n\t\tstop\n",
        );
        sync(&mut state);
        let catalog = state.catalog.as_ref().unwrap();
        let (id, _) = catalog
            .classify_line("ping", skript_docs::LineRole::Statement)
            .unwrap();
        let found = definitions_of(&state.workspace, catalog, id, "file:///dir/b.sk");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.uri(), "file:///dir/b.sk");
        assert!(definitions_of(&state.workspace, catalog, id, "file:///dir/c.sk").is_empty());
    }

    /// Fifty definitions must not make a keystroke noticeably slower. Run with
    /// `--nocapture` to see the numbers.
    #[test]
    fn syncing_fifty_definitions_is_cheap() {
        let source: String = (0..50)
            .map(|n| format!("effect custom action {n} on %player%:\n\ttrigger:\n\t\tstop\n\n"))
            .collect();
        let mut state = state(&source);
        let started = std::time::Instant::now();
        assert!(sync(&mut state));
        let install = started.elapsed();

        let started = std::time::Instant::now();
        for _ in 0..100 {
            assert!(!sync(&mut state));
        }
        let unchanged = started.elapsed() / 100;
        println!("install {install:?}, unchanged sync {unchanged:?}");
        // Generous bounds for a debug build on a slow CI machine.
        assert!(install.as_millis() < 250, "install took {install:?}");
        assert!(
            unchanged.as_millis() < 5,
            "an unchanged sync took {unchanged:?}"
        );
    }
}
