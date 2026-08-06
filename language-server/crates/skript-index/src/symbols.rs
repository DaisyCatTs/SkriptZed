//! Extracts declarations and references from a parsed Skript document.
//!
//! This is the half of the language server that does not need `docs.json` at
//! all: functions, commands, options and variables are declared *by the script*,
//! so go-to-definition, find-references, rename and the outline work offline and
//! on the very first keystroke.

use tree_sitter::{Node, Tree};

use crate::text::Range;

/// What kind of thing a symbol is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Event,
    Command,
    /// A command entry such as `permission:` or `trigger:`.
    Entry,
    Function,
    /// `local function` — visible only inside the declaring script.
    LocalFunction,
    Option,
    /// A `{global}` or `{-ephemeral}` variable.
    GlobalVariable,
    /// A `{_local}` variable, scoped to one trigger.
    LocalVariable,
    Alias,
    Section,
    Structure,
}

impl SymbolKind {
    pub fn is_function(self) -> bool {
        matches!(self, SymbolKind::Function | SymbolKind::LocalFunction)
    }

    pub fn is_variable(self) -> bool {
        matches!(self, SymbolKind::GlobalVariable | SymbolKind::LocalVariable)
    }
}

/// A declaration, with the nesting the outline panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    /// Extra text shown after the name, e.g. a function's signature.
    pub detail: String,
    /// The whole construct, used for folding and for `class.around`.
    pub range: Range,
    /// Just the name, used as the go-to-definition target.
    pub selection_range: Range,
    pub children: Vec<Symbol>,
}

/// A use of a symbol somewhere in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub kind: SymbolKind,
    pub name: String,
    pub range: Range,
}

/// Everything extracted from one document.
#[derive(Debug, Clone, Default)]
pub struct FileSymbols {
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
}

impl FileSymbols {
    /// Flattens the symbol tree.
    pub fn flat(&self) -> Vec<&Symbol> {
        fn walk<'a>(symbols: &'a [Symbol], out: &mut Vec<&'a Symbol>) {
            for symbol in symbols {
                out.push(symbol);
                walk(&symbol.children, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.symbols, &mut out);
        out
    }

    /// The declaration whose name the cursor is on, if any.
    pub fn declaration_at(&self, position: crate::text::Position) -> Option<&Symbol> {
        self.flat()
            .into_iter()
            .find(|symbol| symbol.selection_range.touches(position))
    }

    /// The reference the cursor is on, if any.
    pub fn reference_at(&self, position: crate::text::Position) -> Option<&Reference> {
        self.references
            .iter()
            .find(|reference| reference.range.touches(position))
    }
}

/// Walks the tree and collects declarations and references.
pub fn extract(tree: &Tree, source: &str) -> FileSymbols {
    let mut out = FileSymbols::default();
    let root = tree.root_node();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if let Some(symbol) = structure_symbol(child, source, &mut out) {
            out.symbols.push(symbol);
        }
    }

    collect_references(root, source, &mut out);
    out
}

fn text_of(node: Node<'_>, source: &str) -> String {
    source[node.byte_range()].to_string()
}

fn structure_symbol(node: Node<'_>, source: &str, out: &mut FileSymbols) -> Option<Symbol> {
    match node.kind() {
        "event" => {
            let name = field_text(node, "name", source).unwrap_or_else(|| "event".into());
            Some(Symbol {
                kind: SymbolKind::Event,
                name: first_line(&name),
                detail: String::new(),
                range: node.into(),
                selection_range: node
                    .child_by_field_name("name")
                    .map(Range::from)
                    .unwrap_or(node.into()),
                children: body_sections(node, source),
            })
        }

        "command" => {
            let name_node = node.child_by_field_name("name")?;
            let raw = text_of(name_node, source);
            Some(Symbol {
                kind: SymbolKind::Command,
                // `/home` and `home` name the same command; store it without
                // the slash so references resolve either way.
                name: raw.trim_start_matches('/').to_string(),
                detail: command_arguments(node, source),
                range: node.into(),
                selection_range: name_node.into(),
                children: body_sections(node, source),
            })
        }

        "function" => {
            let name_node = node.child_by_field_name("name")?;
            let is_local = has_local_keyword(node, source);
            Some(Symbol {
                kind: if is_local {
                    SymbolKind::LocalFunction
                } else {
                    SymbolKind::Function
                },
                name: text_of(name_node, source),
                detail: function_signature(node, source),
                range: node.into(),
                selection_range: name_node.into(),
                children: Vec::new(),
            })
        }

        "options" => Some(Symbol {
            kind: SymbolKind::Structure,
            name: "options".into(),
            detail: String::new(),
            range: node.into(),
            selection_range: node.child(0).map(Range::from).unwrap_or(node.into()),
            children: option_entries(node, source, out),
        }),

        "variables" | "aliases" => {
            let kind_name = node.kind().to_string();
            Some(Symbol {
                kind: SymbolKind::Structure,
                name: kind_name,
                detail: String::new(),
                range: node.into(),
                selection_range: node.child(0).map(Range::from).unwrap_or(node.into()),
                children: assignment_entries(node, source),
            })
        }

        "using" | "auto_reload" | "import" => Some(Symbol {
            kind: SymbolKind::Structure,
            name: text_of(node, source).trim().to_string(),
            detail: String::new(),
            range: node.into(),
            selection_range: node.into(),
            children: Vec::new(),
        }),

        _ => None,
    }
}

fn has_local_keyword(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    // Bound rather than returned directly: the iterator borrows `cursor`, which
    // would not outlive the function's return expression.
    let found = node
        .children(&mut cursor)
        .any(|child| child.kind() == "keyword" && text_of(child, source) == "local");
    found
}

fn field_text(node: Node<'_>, field: &str, source: &str) -> Option<String> {
    // `_content` is inlined, so a field may repeat across several children;
    // join them so `on damage of player` reads as one name.
    let mut cursor = node.walk();
    let parts: Vec<String> = node
        .children_by_field_name(field, &mut cursor)
        .map(|child| text_of(child, source))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn function_signature(node: Node<'_>, source: &str) -> String {
    let params = node
        .child_by_field_name("parameters")
        .map(|params| text_of(params, source))
        .unwrap_or_default();
    let returns = node
        .child_by_field_name("return_type")
        .map(|ty| format!(" :: {}", text_of(ty, source).trim()))
        .unwrap_or_default();
    format!("({params}){returns}")
}

fn command_arguments(node: Node<'_>, source: &str) -> String {
    let mut cursor = node.walk();
    let args: Vec<String> = node
        .children_by_field_name("argument", &mut cursor)
        .map(|arg| text_of(arg, source))
        .collect();
    args.join(" ")
}

/// Nested sections inside a body, so the outline mirrors the file.
fn body_sections(node: Node<'_>, source: &str) -> Vec<Symbol> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "entry_section" | "entry" => {
                if let Some(key) = child.child_by_field_name("key") {
                    out.push(Symbol {
                        kind: SymbolKind::Entry,
                        name: text_of(key, source),
                        detail: String::new(),
                        range: child.into(),
                        selection_range: key.into(),
                        children: body_sections(child, source),
                    });
                }
            }
            "section" => {
                let header = field_text(child, "header", source).unwrap_or_default();
                out.push(Symbol {
                    kind: SymbolKind::Section,
                    name: first_line(&header),
                    detail: String::new(),
                    range: child.into(),
                    selection_range: child.into(),
                    children: body_sections(child, source),
                });
            }
            _ => {}
        }
    }
    out
}

fn option_entries(node: Node<'_>, source: &str, out: &mut FileSymbols) -> Vec<Symbol> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };

    let mut symbols = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if !matches!(child.kind(), "entry" | "entry_section") {
            continue;
        }
        let Some(key) = child.child_by_field_name("key") else {
            continue;
        };
        symbols.push(Symbol {
            kind: SymbolKind::Option,
            name: text_of(key, source),
            detail: child
                .child_by_field_name("value")
                .map(|value| text_of(value, source))
                .unwrap_or_default(),
            range: child.into(),
            selection_range: key.into(),
            children: Vec::new(),
        });
    }
    let _ = out;
    symbols
}

fn assignment_entries(node: Node<'_>, source: &str) -> Vec<Symbol> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };

    let mut symbols = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "assignment" {
            continue;
        }
        let Some(target) = child.child_by_field_name("target") else {
            continue;
        };
        let raw = text_of(target, source);
        let (kind, name) = if target.kind() == "variable" {
            (variable_kind(&raw), normalise_variable(&raw))
        } else {
            (SymbolKind::Alias, raw.trim().to_string())
        };
        symbols.push(Symbol {
            kind,
            name,
            detail: child
                .child_by_field_name("value")
                .map(|value| text_of(value, source).trim().to_string())
                .unwrap_or_default(),
            range: child.into(),
            selection_range: target.into(),
            children: Vec::new(),
        });
    }
    symbols
}

/// `{_x}` is trigger-local; `{x}` and `{-x}` are global (the `-` only affects
/// whether the value is persisted, not its visibility).
fn variable_kind(raw: &str) -> SymbolKind {
    if raw.starts_with("{_") {
        SymbolKind::LocalVariable
    } else {
        SymbolKind::GlobalVariable
    }
}

/// Strips the braces and the scope sigil, so `{_count}` and `{count}` both
/// index under `count` while keeping their kinds distinct.
pub fn normalise_variable(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim_start_matches(['_', '-'])
        .trim()
        .to_string()
}

/// Walks the whole tree for uses of functions, options and variables.
fn collect_references(node: Node<'_>, source: &str, out: &mut FileSymbols) {
    let mut stack = vec![node];

    while let Some(current) = stack.pop() {
        // The cursor must be created from the node being walked, not from the
        // root, or it outlives the borrow it was made from.
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            stack.push(child);
        }
        drop(cursor);

        match current.kind() {
            "function_call" => {
                if let Some(name) = current.child_by_field_name("name") {
                    out.references.push(Reference {
                        kind: SymbolKind::Function,
                        name: text_of(name, source),
                        range: name.into(),
                    });
                }
            }
            "option_ref" => {
                if let Some(name) = current.child_by_field_name("name") {
                    out.references.push(Reference {
                        kind: SymbolKind::Option,
                        name: text_of(name, source).trim().to_string(),
                        range: name.into(),
                    });
                }
            }
            "variable" => {
                let raw = text_of(current, source);
                // Only index variables whose name is static. An interpolated
                // name like `{home::%uuid of player%}` names a different
                // variable per player, so renaming it as one symbol would be
                // wrong.
                if current
                    .child_by_field_name("name")
                    .is_some_and(|name| name.child_count() == 1)
                {
                    out.references.push(Reference {
                        kind: variable_kind(&raw),
                        name: normalise_variable(&raw),
                        range: current.into(),
                    });
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn symbols(source: &str) -> FileSymbols {
        Document::new("file:///test.sk", source).symbols().clone()
    }

    #[test]
    fn finds_functions_with_their_signatures() {
        let found =
            symbols("function give_apple(name: text, amount: number) :: item:\n\treturn 1\n");
        assert_eq!(found.symbols.len(), 1);
        let function = &found.symbols[0];
        assert_eq!(function.kind, SymbolKind::Function);
        assert_eq!(function.name, "give_apple");
        assert!(function.detail.contains("name: text"));
        assert!(function.detail.contains(":: item"));
    }

    #[test]
    fn distinguishes_local_functions() {
        let found = symbols("local function helper():\n\treturn 1\n");
        assert_eq!(found.symbols[0].kind, SymbolKind::LocalFunction);
    }

    #[test]
    fn finds_commands_without_the_slash() {
        let found = symbols("command /home <text>:\n\ttrigger:\n\t\tstop\n");
        let command = &found.symbols[0];
        assert_eq!(command.kind, SymbolKind::Command);
        assert_eq!(command.name, "home");
        assert!(command.detail.contains("<text>"));
        // The `trigger:` entry shows up as a child, so the outline nests.
        assert!(command.children.iter().any(|child| child.name == "trigger"));
    }

    #[test]
    fn finds_options_and_their_values() {
        let found = symbols("options:\n\tprefix: &6[Server]\n");
        let option = &found.symbols[0].children[0];
        assert_eq!(option.kind, SymbolKind::Option);
        assert_eq!(option.name, "prefix");
    }

    #[test]
    fn finds_default_variables_and_aliases() {
        let found = symbols("variables:\n\t{score} = 0\n\naliases:\n\tores = iron ore\n");
        let variable = &found.symbols[0].children[0];
        assert_eq!(variable.kind, SymbolKind::GlobalVariable);
        assert_eq!(variable.name, "score");

        let alias = &found.symbols[1].children[0];
        assert_eq!(alias.kind, SymbolKind::Alias);
        assert_eq!(alias.name, "ores");
    }

    #[test]
    fn records_function_calls_options_and_variables_as_references() {
        let found = symbols(
            "on join:\n\tset {_x} to give_apple(\"a\", 1)\n\tsend \"{@prefix}\"\n\tadd 1 to {score}\n",
        );

        assert!(found
            .references
            .iter()
            .any(|r| r.kind == SymbolKind::Function && r.name == "give_apple"));
        assert!(found
            .references
            .iter()
            .any(|r| r.kind == SymbolKind::LocalVariable && r.name == "x"));
        assert!(found
            .references
            .iter()
            .any(|r| r.kind == SymbolKind::GlobalVariable && r.name == "score"));
    }

    #[test]
    fn skips_interpolated_variable_names() {
        // `{home::%uuid of player%}` names a different variable per player, so
        // it must not be treated as one renameable symbol.
        let found = symbols("on join:\n\tset {home::%uuid of player%} to 1\n");
        assert!(found.references.iter().all(|r| !r.name.contains("home")));
    }

    #[test]
    fn events_keep_their_full_name() {
        let found = symbols("on damage of player:\n\tstop\n");
        assert_eq!(found.symbols[0].kind, SymbolKind::Event);
        assert_eq!(found.symbols[0].name, "damage of player");
    }

    #[test]
    fn nested_sections_appear_in_the_outline() {
        let found = symbols("on join:\n\tif {_x} is set:\n\t\tstop\n");
        let event = &found.symbols[0];
        assert_eq!(event.children.len(), 1);
        assert!(event.children[0].name.starts_with("if"));
    }
}
