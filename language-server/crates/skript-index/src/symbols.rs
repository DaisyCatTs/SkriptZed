//! Extracts declarations and references from a parsed Skript document.
//!
//! This is the half of the language server that does not need `docs.json` at
//! all: functions, commands, options and variables are declared *by the script*,
//! so go-to-definition, find-references, rename and the outline work offline and
//! on the very first keystroke.

use tree_sitter::{Node, Tree};

use crate::text::Range;

pub mod doc;

pub use doc::{DocComment, DocParam};

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
    /// A ClassySK `class Name:`.
    Class,
    /// A ClassySK field — `public kills: integer`.
    Field,
    /// A ClassySK method — `public reset():`.
    Method,
    /// A skript-reflect custom effect, expression, condition or event; see
    /// [`crate::syntax`].
    Syntax,
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
    /// The whole reference, including a variable's braces. Used for hit-testing
    /// so that clicking anywhere on `{score::*}` finds it.
    pub range: Range,
    /// Just the name, excluding `{`, the `_`/`-` scope sigil and `}`.
    ///
    /// Renames edit this and nothing else. Rebuilding the surrounding syntax
    /// from a prefix instead used to drop the `::*` off a list variable,
    /// silently turning it into a scalar.
    pub name_range: Range,
    /// For a trigger-local `{_x}`, the structure that owns it.
    ///
    /// Skript scopes `{_x}` to the running trigger, not the file. Treating it
    /// as file-wide made rename destructive in a way nothing warned about:
    /// renaming `{_i}` to `{_player}` in one trigger rewrote every *other*
    /// trigger's `{_i}` too, and if a sibling already used `{_player}` for
    /// something else, two unrelated variables were silently merged into one.
    ///
    /// `None` for anything genuinely file- or project-wide.
    pub scope: Option<Range>,
}

/// Everything extracted from one document.
#[derive(Debug, Clone, Default)]
pub struct FileSymbols {
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    /// The skript-reflect syntax this file defines, in source order.
    pub syntax: Vec<crate::syntax::SyntaxDefinition>,
    /// Doc comments, keyed by the documented declaration's
    /// `selection_range`; see [`FileSymbols::doc_for`].
    ///
    /// Kept beside the tree rather than on [`Symbol`], so the symbol tree
    /// stays what the outline renders and only documentable kinds pay for it.
    pub docs: Vec<(Range, DocComment)>,
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

    /// The doc comment of the declaration whose name is `selection_range`:
    /// a function, ClassySK method, command or skript-reflect definition.
    pub fn doc_for(&self, selection_range: Range) -> Option<&DocComment> {
        self.docs
            .iter()
            .find(|(range, _)| *range == selection_range)
            .map(|(_, doc)| doc)
    }

    /// The reference the cursor is on, if any.
    pub fn reference_at(&self, position: crate::text::Position) -> Option<&Reference> {
        self.references
            .iter()
            .find(|reference| reference.range.touches(position))
    }

    /// The top-level declaration whose range encloses `range`, among those
    /// whose kind `wanted` accepts.
    pub fn enclosing(&self, range: Range, wanted: impl Fn(SymbolKind) -> bool) -> Option<&Symbol> {
        self.symbols
            .iter()
            .find(|symbol| wanted(symbol.kind) && symbol.range.encloses(range))
    }

    /// As [`FileSymbols::enclosing`], but by whole lines: the top-level
    /// declaration whose lines include `line`, columns ignored.
    pub fn enclosing_line(
        &self,
        line: u32,
        wanted: impl Fn(SymbolKind) -> bool,
    ) -> Option<&Symbol> {
        self.symbols
            .iter()
            .find(|symbol| wanted(symbol.kind) && symbol.range.spans_line(line))
    }
}

/// Walks the tree and collects declarations and references.
pub fn extract(tree: &Tree, source: &str) -> FileSymbols {
    let mut out = FileSymbols::default();
    let root = tree.root_node();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if child.kind() == "event" {
            if let Some((symbol, definition)) = crate::syntax::definition(child, source) {
                out.symbols.push(symbol);
                out.syntax.push(definition);
                continue;
            }
        }
        if let Some(symbol) = structure_symbol(child, source) {
            out.symbols.push(symbol);
        }
    }

    collect_references(root, source, &mut out);
    out.docs = collect_docs(&out.symbols, source);
    out
}

/// The doc comments above every documentable declaration.
///
/// Read from the text rather than the tree: comment lines are their own
/// top-level nodes (comments are not `extras`), and inside a class they would
/// be siblings of the method, so the line above is the simpler question.
fn collect_docs(symbols: &[Symbol], source: &str) -> Vec<(Range, DocComment)> {
    let documented = |symbol: &&Symbol| {
        matches!(
            symbol.kind,
            SymbolKind::Function
                | SymbolKind::LocalFunction
                | SymbolKind::Command
                | SymbolKind::Syntax
                | SymbolKind::Method
        )
    };
    let methods = symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Class)
        .flat_map(|class| &class.children);
    let lines: Vec<&str> = source.lines().collect();
    symbols
        .iter()
        .chain(methods)
        .filter(documented)
        .filter_map(|symbol| {
            doc::above(&lines, symbol.range.start.line).map(|found| (symbol.selection_range, found))
        })
        .collect()
}

fn text_of(node: Node<'_>, source: &str) -> String {
    source[node.byte_range()].to_string()
}

fn structure_symbol(node: Node<'_>, source: &str) -> Option<Symbol> {
    match node.kind() {
        "event" => {
            let name = field_source(node, "name", source).unwrap_or_else(|| "event".into());
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
                children: parameters(node, source),
            })
        }

        "options" => Some(Symbol {
            kind: SymbolKind::Structure,
            name: "options".into(),
            detail: String::new(),
            range: node.into(),
            selection_range: node.child(0).map(Range::from).unwrap_or(node.into()),
            children: option_entries(node, source),
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

        // The header line only: an `import:` block's node also spans its
        // whole body, and the outline showed every imported class, newlines
        // and all, as one name.
        "using" | "auto_reload" | "import" => Some(Symbol {
            kind: SymbolKind::Structure,
            name: first_line(&text_of(node, source)),
            detail: String::new(),
            range: node.into(),
            selection_range: node.into(),
            children: Vec::new(),
        }),

        // oopsk's `struct Message:` — a class in all but name, whose fields are
        // `name: type [= default]` entries.
        "struct" => {
            let name_node = node.child_by_field_name("name")?;
            Some(Symbol {
                kind: SymbolKind::Class,
                name: text_of(name_node, source),
                detail: "struct".into(),
                range: node.into(),
                selection_range: name_node.into(),
                children: struct_fields(node, source),
            })
        }

        "class" => {
            let name_node = node.child_by_field_name("name")?;
            Some(Symbol {
                kind: SymbolKind::Class,
                name: text_of(name_node, source),
                detail: String::new(),
                range: node.into(),
                selection_range: name_node.into(),
                children: class_members(node, source),
            })
        }

        _ => None,
    }
}

/// An oopsk struct's fields. `const timestamp: date = now` is the field
/// `timestamp`; `converts to:` is not a field and is left out.
fn struct_fields(node: Node<'_>, source: &str) -> Vec<Symbol> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "entry" {
            continue;
        }
        let Some(key) = child.child_by_field_name("key") else {
            continue;
        };
        let raw = text_of(key, source);
        let name = raw
            .trim()
            .strip_prefix("constant ")
            .or_else(|| raw.trim().strip_prefix("const "))
            .unwrap_or(raw.trim())
            .trim()
            .to_string();
        out.push(Symbol {
            kind: SymbolKind::Field,
            name,
            detail: field_text(child, "value", source).unwrap_or_default(),
            range: child.into(),
            selection_range: key.into(),
            children: Vec::new(),
        });
    }
    out
}

/// A ClassySK class's fields and methods, in source order.
///
/// A method mirrors a function: its signature is the detail and its parameters
/// are its children, so `{_amount}` inside `deposit(amount: number)` resolves to
/// the parameter exactly as it would in a function.
fn class_members(node: Node<'_>, source: &str) -> Vec<Symbol> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        let kind = match child.kind() {
            "class_field" => SymbolKind::Field,
            "method" => SymbolKind::Method,
            _ => continue,
        };
        let Some(name) = child.child_by_field_name("name") else {
            continue;
        };
        let (detail, children) = if kind == SymbolKind::Method {
            (function_signature(child, source), parameters(child, source))
        } else {
            let ty = field_text(child, "type", source).unwrap_or_default();
            (ty.trim().to_string(), Vec::new())
        };
        out.push(Symbol {
            kind,
            name: text_of(name, source),
            detail,
            range: child.into(),
            selection_range: name.into(),
            children,
        });
    }
    out
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

/// The source text spanned by every child carrying `field`, as written.
///
/// Unlike [`field_text`], which joins the children with single spaces, this
/// keeps the original spacing: joining tokens rendered `chance of 50%` as
/// `chance of 50 %` and `%player%` as `% player %`.
pub(crate) fn field_source(node: Node<'_>, field: &str, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let mut children = node.children_by_field_name(field, &mut cursor);
    let first = children.next()?;
    let last = children.last().unwrap_or(first);
    Some(source[first.start_byte()..last.end_byte()].to_string())
}

/// The span covering every child carrying `field`.
///
/// `_content` is inlined by the grammar, so a header arrives as a run of
/// siblings rather than one node; taking only the first would clip the range to
/// its opening word.
pub(crate) fn field_range(node: Node<'_>, field: &str) -> Option<Range> {
    let mut cursor = node.walk();
    let mut children = node.children_by_field_name(field, &mut cursor);
    let first: Range = children.next()?.into();
    let last = children.last().map(Range::from).unwrap_or(first);
    Some(Range::new(first.start, last.end))
}

pub(crate) fn first_line(text: &str) -> String {
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

/// A function's parameters, as local variables scoped to its body.
///
/// They are `{_name}` inside the body, so they are the same kind as any other
/// trigger-local. Without them, renaming `{_name}` rewrote every use in the body
/// and left the signature untouched — producing a function that still parses,
/// still runs, and silently does nothing. Go-to-definition on a parameter also
/// had nowhere to go.
///
/// The grammar already names the fields, so there is nothing to parse here.
fn parameters(node: Node<'_>, source: &str) -> Vec<Symbol> {
    let Some(list) = node.child_by_field_name("parameters") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut cursor = list.walk();
    for child in list.children(&mut cursor) {
        if child.kind() != "parameter" {
            continue;
        }
        let Some(name) = child.child_by_field_name("name") else {
            continue;
        };
        out.push(Symbol {
            kind: SymbolKind::LocalVariable,
            name: text_of(name, source),
            detail: field_text(child, "type", source)
                .map(|ty| ty.trim().to_string())
                .unwrap_or_default(),
            range: child.into(),
            selection_range: name.into(),
            children: Vec::new(),
        });
    }
    out
}

/// The pattern of a `subcommand` header: everything between the keyword and
/// the colon, as written.
fn subcommand_pattern(node: Node<'_>, source: &str) -> String {
    let start = node
        .child(0)
        .map_or(node.start_byte(), |keyword| keyword.end_byte());
    let mut cursor = node.walk();
    // A trailing `# comment` or the body, whichever comes first, ends it.
    let end = node
        .children(&mut cursor)
        .find(|child| matches!(child.kind(), "comment" | "entry_body"))
        .map_or(node.end_byte(), |child| child.start_byte());
    source[start..end]
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_string()
}

/// Nested sections inside a body, so the outline mirrors the file.
pub(crate) fn body_sections(node: Node<'_>, source: &str) -> Vec<Symbol> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            // `subcommand <number>:` (Skript 2.17) — named by its pattern, the
            // way a command is, and holding entries the way a command does.
            "subcommand" => {
                let keyword = child.child(0).map(Range::from).unwrap_or(child.into());
                let pattern = subcommand_pattern(child, source);
                out.push(Symbol {
                    kind: SymbolKind::Entry,
                    name: format!("subcommand {pattern}").trim_end().to_string(),
                    detail: String::new(),
                    range: child.into(),
                    selection_range: keyword,
                    children: body_sections(child, source),
                });
            }
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
                let header = field_source(child, "header", source).unwrap_or_default();
                let mut children = body_sections(child, source);
                if takes_entries(&header) {
                    children.extend(entry_statements(child, source));
                    children.sort_by_key(|symbol| symbol.range.start);
                }
                out.push(Symbol {
                    kind: SymbolKind::Section,
                    name: first_line(&header),
                    detail: String::new(),
                    range: child.into(),
                    // Only the header, never the body. A selection range that
                    // spanned the whole section made `declaration_at` match
                    // every nested line, and because the server checks
                    // declarations before the catalog, hovering anything inside
                    // an `if` showed the `if` instead of the line under the
                    // cursor.
                    selection_range: field_range(child, "header").unwrap_or_else(|| child.into()),
                    children,
                });
            }
            _ => {}
        }
    }
    out
}

/// Whether a section with this header can hold `key: value` entries.
///
/// Addon sections take entries the way a command does: SkBee's
/// `register shaped recipe:` holds `result:`, `shape:` and `ingredients:`.
/// The grammar cannot tell those from statements, and a control-flow section
/// never holds them, so only those are ruled out here.
fn takes_entries(header: &str) -> bool {
    const CONTROL_FLOW: &[&str] = &[
        "if ",
        "else",
        "loop ",
        "while ",
        "do while",
        "switch ",
        "case ",
        "default",
        "chance of ",
    ];
    let lower = first_line(header).to_ascii_lowercase();
    !CONTROL_FLOW
        .iter()
        .any(|keyword| lower.starts_with(keyword) || lower == keyword.trim_end())
}

/// The statements in a section's body shaped like an entry, `result: diamond`.
///
/// Recorded as `Entry` symbols, which is what keeps them out of the
/// "unknown syntax" check and gives them an outline row. The key must be plain
/// words: `send "a: b"` and `{_x}.put(1: 2)` have a colon too, but never
/// behind nothing but letters and spaces.
fn entry_statements(section: Node<'_>, source: &str) -> Vec<Symbol> {
    let Some(body) = section.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "statement" {
            continue;
        }
        let text = &source[child.byte_range()];
        let Some(key) = entry_key(text) else {
            continue;
        };
        let start = child.start_position();
        let selection = Range::on_line(
            start.row as u32,
            start.column as u32,
            (start.column + key.len()) as u32,
        );
        out.push(Symbol {
            kind: SymbolKind::Entry,
            name: key.to_string(),
            detail: String::new(),
            range: child.into(),
            selection_range: selection,
            children: Vec::new(),
        });
    }
    out
}

/// The key of an entry-shaped line: plain words, then a colon that ends the
/// line or is followed by a space.
pub fn entry_key(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let (key, rest) = line.split_once(':')?;
    let plain = key.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && !key.ends_with(' ')
        && key.len() <= 48
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '_' | '-'));
    (plain && (rest.is_empty() || rest.starts_with(char::is_whitespace))).then_some(key)
}

fn option_entries(node: Node<'_>, source: &str) -> Vec<Symbol> {
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
            // The whole value: it arrives as a run of `value` children, and
            // the first alone cut `&6[Shop]` down to `&6`.
            detail: field_source(child, "value", source)
                .map(|value| value.trim().to_string())
                .unwrap_or_default(),
            range: child.into(),
            selection_range: key.into(),
            children: Vec::new(),
        });
    }
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
        // Select the name inside the braces, so a rename rewrites `score` and
        // leaves `{`, the scope sigil and `}` alone. Selecting the whole node
        // turned `{score} = 0` into `total = 0`.
        let selection = target
            .child_by_field_name("name")
            .map(Range::from)
            .unwrap_or_else(|| target.into());
        symbols.push(Symbol {
            kind,
            name,
            detail: child
                .child_by_field_name("value")
                .map(|value| text_of(value, source).trim().to_string())
                .unwrap_or_default(),
            range: child.into(),
            selection_range: selection,
            children: Vec::new(),
        });
    }
    symbols
}

/// Whether a variable name contains a `%…%` slot, which makes it dynamic.
fn has_interpolation(name: Node<'_>) -> bool {
    let mut cursor = name.walk();
    let found = name
        .children(&mut cursor)
        .any(|child| child.kind() == "interpolation");
    found
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

/// The range of the top-level structure containing `node`.
///
/// A trigger scope is exactly "the event, command or function this line is in",
/// and the tree already says so — walk up until the parent is the file itself.
///
/// The one exception is a ClassySK method. Each method runs as its own trigger,
/// so its `{_x}` is not its neighbour's: scoping to the whole class would make
/// renaming `{_i}` in one method rewrite every other method's `{_i}`.
fn enclosing_structure(node: Node<'_>) -> Option<Range> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "source_file" || current.kind() == "method" {
            return Some(current.into());
        }
        current = parent;
    }
    None
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
                        name_range: name.into(),
                        // Functions are project-wide.
                        scope: None,
                    });
                }
            }
            // `self::balance`, `{_w}::deposit(5)`. The receiver's class is a
            // runtime value, so these resolve by name alone — which is why
            // members can be navigated to but never renamed.
            "member_access" => {
                if let Some(member) = current.child_by_field_name("member") {
                    let (kind, name) = if member.kind() == "method_call" {
                        (SymbolKind::Method, member.child_by_field_name("name"))
                    } else {
                        (SymbolKind::Field, Some(member))
                    };
                    if let Some(name) = name {
                        out.references.push(Reference {
                            kind,
                            name: text_of(name, source),
                            range: name.into(),
                            name_range: name.into(),
                            // Classes are global in ClassySK.
                            scope: None,
                        });
                    }
                }
            }
            "option_ref" => {
                if let Some(name) = current.child_by_field_name("name") {
                    out.references.push(Reference {
                        kind: SymbolKind::Option,
                        name: text_of(name, source).trim().to_string(),
                        range: name.into(),
                        name_range: name.into(),
                        // Options are file-wide.
                        scope: None,
                    });
                }
            }
            "variable" => {
                let raw = text_of(current, source);
                // Only index variables whose name is static. An interpolated
                // name like `{home::%uuid of player%}` names a different
                // variable per player, so renaming it as one symbol would be
                // wrong.
                //
                // The test is for an `interpolation` child specifically, not
                // for a single child: `{score::*}` has three (text, `::`,
                // text) and is perfectly static, and list variables are far too
                // common in Skript to leave unindexed.
                if let Some(name_node) = current
                    .child_by_field_name("name")
                    .filter(|name| !has_interpolation(*name))
                {
                    let kind = variable_kind(&raw);
                    out.references.push(Reference {
                        kind,
                        name: normalise_variable(&raw),
                        range: current.into(),
                        name_range: name_node.into(),
                        scope: (kind == SymbolKind::LocalVariable)
                            .then(|| enclosing_structure(current))
                            .flatten(),
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

    /// Functions, methods, commands and custom syntax are all documented by
    /// the comment directly above them, and nothing else is.
    #[test]
    fn attaches_doc_comments_to_documentable_declarations() {
        let found = symbols(concat!(
            "# Gives a kit.\n",
            "# @param p who\n",
            "function kit(p: player):\n",
            "\tstop\n",
            "# A join handler.\n",
            "on join:\n",
            "\tstop\n",
            "# Goes home.\n",
            "command /home:\n",
            "\ttrigger:\n",
            "\t\tstop\n",
            "class Wallet:\n",
            "\t# Empties it.\n",
            "\tpublic reset():\n",
            "\t\tstop\n",
            "# Says hi.\n",
            "effect greet %player%:\n",
            "\ttrigger:\n",
            "\t\tstop\n",
        ));
        let described = |name: &str| {
            let symbol = found
                .flat()
                .into_iter()
                .find(|symbol| symbol.name == name)
                .unwrap_or_else(|| panic!("no symbol {name}"));
            found
                .doc_for(symbol.selection_range)
                .map(|doc| doc.description.clone())
        };
        assert_eq!(described("kit").as_deref(), Some("Gives a kit."));
        assert_eq!(described("home").as_deref(), Some("Goes home."));
        assert_eq!(described("reset").as_deref(), Some("Empties it."));
        assert_eq!(described("greet %player%").as_deref(), Some("Says hi."));
        assert_eq!(described("join"), None);
        assert_eq!(found.docs.len(), 4);
    }

    const WALLET: &str = concat!(
        "class Wallet:\n",
        "\tprivate balance: number = 0\n",
        "\tpublic deposit(amount: number, note?: text) :: boolean:\n",
        "\t\tset {_i} to {_amount}\n",
        "\t\tadd {_amount} to self::balance\n",
        "\tpublic reset():\n",
        "\t\tset {_i} to 0\n",
        "\t\tself::deposit(0)\n",
    );

    #[test]
    fn finds_a_classysk_class_with_its_fields_and_methods() {
        let found = symbols(WALLET);
        let class = &found.symbols[0];
        assert_eq!(class.kind, SymbolKind::Class);
        assert_eq!(class.name, "Wallet");

        let members: Vec<(SymbolKind, &str, &str)> = class
            .children
            .iter()
            .map(|m| (m.kind, m.name.as_str(), m.detail.as_str()))
            .collect();
        assert_eq!(
            members,
            vec![
                (SymbolKind::Field, "balance", "number"),
                (
                    SymbolKind::Method,
                    "deposit",
                    "(amount: number, note?: text) :: boolean"
                ),
                (SymbolKind::Method, "reset", "()"),
            ]
        );
        // A method's parameters are its children, as a function's are.
        let deposit = &class.children[1];
        let params: Vec<&str> = deposit.children.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(params, vec!["amount", "note"]);
    }

    #[test]
    fn finds_an_oopsk_struct_and_its_fields() {
        let found = symbols(concat!(
            "struct Message:\n",
            "\tcontents: string\n",
            "\tconst timestamp: date = now\n",
            "\tconverts to:\n",
            "\t\tstring via this->contents\n",
        ));
        let class = &found.symbols[0];
        assert_eq!(
            (class.kind, class.name.as_str()),
            (SymbolKind::Class, "Message")
        );
        let fields: Vec<(&str, &str)> = class
            .children
            .iter()
            .map(|f| (f.name.as_str(), f.detail.as_str()))
            .collect();
        // `const` is a modifier, not part of the name; `converts to:` is no field.
        assert_eq!(
            fields,
            vec![("contents", "string"), ("timestamp", "date = now")]
        );
    }

    #[test]
    fn member_access_is_a_member_reference_not_a_function_call() {
        let found = symbols(WALLET);
        let members: Vec<(SymbolKind, &str)> = found
            .references
            .iter()
            .filter(|r| !r.kind.is_variable())
            .map(|r| (r.kind, r.name.as_str()))
            .collect();
        assert!(members.contains(&(SymbolKind::Field, "balance")));
        assert!(members.contains(&(SymbolKind::Method, "deposit")));
        assert!(
            !members
                .iter()
                .any(|(kind, _)| *kind == SymbolKind::Function),
            "{members:?}"
        );
    }

    /// Each ClassySK method runs as its own trigger, so `{_i}` in `deposit`
    /// and `{_i}` in `reset` are different variables.
    #[test]
    fn a_local_variable_is_scoped_to_its_method_not_its_class() {
        let found = symbols(WALLET);
        let scopes: Vec<Option<Range>> = found
            .references
            .iter()
            .filter(|r| r.kind == SymbolKind::LocalVariable && r.name == "i")
            .map(|r| r.scope)
            .collect();
        assert_eq!(scopes.len(), 2);
        assert!(scopes.iter().all(Option::is_some));
        assert_ne!(scopes[0], scopes[1]);
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

    /// Skript 2.17 subcommands, both directly in the command and inside a
    /// `subcommands:` group, are named by their pattern.
    #[test]
    fn finds_subcommands_by_their_pattern() {
        let found = symbols(concat!(
            "command /bank <number>:\n",
            "\tsubcommand <number>: # pay\n",
            "\t\ttrigger:\n",
            "\t\t\tstop\n",
            "\tsubcommands:\n",
            "\t\tsubcommand [help]:\n",
            "\t\t\ttrigger:\n",
            "\t\t\t\tstop\n",
        ));
        let command = &found.symbols[0];
        assert_eq!(command.children[0].name, "subcommand <number>");
        assert_eq!(command.children[0].children[0].name, "trigger");
        let group = &command.children[1];
        assert_eq!(group.name, "subcommands");
        assert_eq!(group.children[0].name, "subcommand [help]");
    }

    #[test]
    fn finds_options_and_their_values() {
        let found = symbols("options:\n\tprefix: &6[Server] of the year\n");
        let option = &found.symbols[0].children[0];
        assert_eq!(option.kind, SymbolKind::Option);
        assert_eq!(option.name, "prefix");
        assert_eq!(option.detail, "&6[Server] of the year");
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

    /// Names come from the source as written, one line, trimmed.
    #[test]
    fn outline_names_keep_their_original_spacing() {
        let found = symbols(concat!(
            "import:\n",
            "\torg.bukkit.Bukkit\n",
            "\torg.bukkit.Material\n",
            "on join:\n",
            "\tchance of 50%:\n",
            "\t\tsend \"%player%\"\n",
            "\tif {_x} is \"%player%\":\n",
            "\t\tstop\n",
            "function f(who: player , n: number):\n",
            "\tstop\n",
        ));
        assert_eq!(found.symbols[0].name, "import:");
        let event = &found.symbols[1];
        assert_eq!(event.children[0].name, "chance of 50%");
        assert_eq!(event.children[1].name, "if {_x} is \"%player%\"");
        let function = &found.symbols[2];
        assert_eq!(function.children[0].detail, "player");
    }

    #[test]
    fn nested_sections_appear_in_the_outline() {
        let found = symbols("on join:\n\tif {_x} is set:\n\t\tstop\n");
        let event = &found.symbols[0];
        assert_eq!(event.children.len(), 1);
        assert!(event.children[0].name.starts_with("if"));
    }
}

#[cfg(test)]
mod enclosing_tests {
    use super::*;
    use crate::Document;

    #[test]
    fn finds_the_top_level_declaration_around_a_range() {
        let document = Document::new(
            "file:///t.sk",
            "class Wallet:
	public reset():
		stop

on join:
	stop
",
        );
        let symbols = document.symbols();
        let inside = Range::on_line(2, 2, 6);

        let class = symbols.enclosing(inside, |kind| kind == SymbolKind::Class);
        assert_eq!(class.map(|symbol| symbol.name.as_str()), Some("Wallet"));
        // The filter applies: no event encloses the method body.
        assert!(symbols
            .enclosing(inside, |kind| kind == SymbolKind::Event)
            .is_none());
        // Outside every declaration.
        assert!(symbols
            .enclosing(Range::on_line(3, 0, 0), |_| true)
            .is_none());
    }

    #[test]
    fn enclosing_line_ignores_columns() {
        let document = Document::new(
            "file:///t.sk",
            "on join:
	stop
",
        );
        let symbols = document.symbols();
        let found = symbols
            .enclosing_line(1, |_| true)
            .map(|symbol| symbol.kind);
        assert_eq!(found, Some(SymbolKind::Event));
        assert!(symbols.enclosing_line(5, |_| true).is_none());
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;
    use crate::{Document, Position};

    #[test]
    fn a_sections_selection_range_covers_only_its_header() {
        // Regression: the selection range used to span the whole section body,
        // so `declaration_at` matched every nested line. The LSP checks
        // declarations before the catalog, which made hover inside any section
        // show the enclosing `if` instead of the effect on the line.
        let document = Document::new(
            "file:///t.sk",
            "on join:\n\tif {_x} is set:\n\t\tsend \"a\"\n",
        );
        let symbols = document.symbols();

        // The cursor is on `send "a"`, two levels in.
        let inside_body = Position::new(2, 4);
        assert!(
            symbols.declaration_at(inside_body).is_none(),
            "a position inside a section body must not resolve to the section header"
        );

        // The header itself still resolves.
        let on_header = Position::new(1, 2);
        let found = symbols
            .declaration_at(on_header)
            .expect("header should resolve");
        assert_eq!(found.kind, SymbolKind::Section);
    }
}

#[cfg(test)]
mod parameter_symbols {
    use super::*;

    fn symbols(source: &str) -> FileSymbols {
        crate::Document::new("file:///test.sk", source)
            .symbols()
            .clone()
    }

    /// Renaming a parameter must be able to reach the signature.
    ///
    /// Before parameters were symbols, `{_who}` in the body was a reference with
    /// no declaration, so a rename rewrote the body and left `who: player` in
    /// the signature — a function that still parses, still runs, and silently
    /// does nothing. That is a language server damaging correct code, which is
    /// worse than a missing feature.
    #[test]
    fn a_function_declares_its_parameters() {
        let file = symbols(
            "function greet(who: player, amount: number = 1) :: text:\n\treturn \"hi %{_who}%\"\n",
        );

        let function = file
            .symbols
            .iter()
            .find(|symbol| symbol.kind.is_function())
            .expect("the function is a symbol");

        let names: Vec<&str> = function
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(names, vec!["who", "amount"]);

        for child in &function.children {
            assert_eq!(child.kind, SymbolKind::LocalVariable);
        }

        // The type comes along, so hover and signature help can use it without
        // scraping the rendered signature string.
        assert_eq!(function.children[0].detail.trim(), "player");
        assert_eq!(function.children[1].detail.trim(), "number");
    }

    #[test]
    fn the_selection_range_covers_only_the_name() {
        // A rename edits `selection_range`. If it covered `who: player`, the
        // rename would eat the type.
        let source = "function f(who: player):\n\treturn\n";
        let file = symbols(source);
        let parameter = &file
            .symbols
            .iter()
            .find(|s| s.kind.is_function())
            .unwrap()
            .children[0];

        let line = source.lines().next().unwrap();
        let start = parameter.selection_range.start.character as usize;
        let end = parameter.selection_range.end.character as usize;
        assert_eq!(&line[start..end], "who");
    }

    #[test]
    fn a_function_without_parameters_has_no_children() {
        let file = symbols("function tick():\n\treturn\n");
        let function = file.symbols.iter().find(|s| s.kind.is_function()).unwrap();
        assert!(function.children.is_empty());
    }
}

#[cfg(test)]
mod section_entries {
    use super::*;

    fn symbols(source: &str) -> FileSymbols {
        crate::Document::new("file:///t.sk", source)
            .symbols()
            .clone()
    }

    /// SkBee's recipe section holds entries, not statements; marking them is
    /// what keeps `result: diamond sword` from reading as unknown syntax.
    #[test]
    fn an_addon_sections_entries_are_entries() {
        let found = symbols(concat!(
            "on load:\n",
            "\tregister shaped recipe:\n",
            "\t\tid: \"custom:sword\"\n",
            "\t\tresult: diamond sword\n",
            "\t\tingredients:\n",
            "\t\t\tset ingredient of \"a\" to diamond\n",
            "\t\tsend \"a: b\"\n",
        ));
        let section = &found.symbols[0].children[0];
        let names: Vec<(SymbolKind, &str)> = section
            .children
            .iter()
            .map(|child| (child.kind, child.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                (SymbolKind::Entry, "id"),
                (SymbolKind::Entry, "result"),
                (SymbolKind::Section, "ingredients"),
            ]
        );
        assert_eq!(section.children[1].selection_range, Range::on_line(3, 2, 8));
    }

    #[test]
    fn control_flow_never_holds_entries() {
        let found = symbols("on load:\n\tif 1 is 1:\n\t\tresult: x\n");
        assert!(found.symbols[0].children[0].children.is_empty());
    }

    #[test]
    fn only_plain_words_make_a_key() {
        assert_eq!(entry_key("\tresult: diamond"), Some("result"));
        assert_eq!(
            entry_key("generate structures:"),
            Some("generate structures")
        );
        assert_eq!(entry_key("send \"a: b\""), None);
        assert_eq!(entry_key("{_x}.put(1: 2)"), None);
        assert_eq!(entry_key("time:12"), None);
        assert_eq!(entry_key("set {_x} to 1"), None);
    }
}
