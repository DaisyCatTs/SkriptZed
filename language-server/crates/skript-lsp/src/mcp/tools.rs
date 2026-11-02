//! The four tools, their schemas, and the caps that keep an answer from
//! flooding the model's context.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};
use skript_docs::{Catalog, Category, Entry, EntryId, LineRole};
use skript_index::{Document, FileSymbols, Symbol, SymbolKind};

use super::project::{index_functions, project_symbols, symbol_kind_label};
use super::{search, Knowledge};
use crate::{entries, lines};

/// The most text one tool answer may carry. Roughly 6,000 tokens: enough for
/// a full hover card or a page of results, small enough that a careless query
/// cannot crowd out the conversation.
pub const MAX_TEXT: usize = 24_000;
pub const DEFAULT_SEARCH_LIMIT: usize = 10;
pub const MAX_SEARCH_LIMIT: usize = 25;
pub const MAX_PATTERNS_SHOWN: usize = 4;
pub const MAX_CLASSIFY_LINES: usize = 200;
/// A Skript line longer than this is not hand-written, and matching it would
/// cost more than it could tell anyone.
pub const MAX_LINE_CHARS: usize = 2_000;
pub const DEFAULT_SYMBOL_LIMIT: usize = 100;
pub const MAX_SYMBOL_LIMIT: usize = 300;
pub const MAX_MEMBERS: usize = 50;
/// Short text fields (descriptions, patterns) are cut to this many chars.
pub(super) const SHORT: usize = 200;

/// What a tool can see.
pub struct Context<'a> {
    pub knowledge: Option<&'a Knowledge>,
    pub project: Option<&'a Path>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    SearchSyntax,
    GetDocs,
    Classify,
    ProjectSymbols,
}

impl Tool {
    const ALL: [Tool; 4] = [
        Tool::SearchSyntax,
        Tool::GetDocs,
        Tool::Classify,
        Tool::ProjectSymbols,
    ];

    fn name(self) -> &'static str {
        match self {
            Tool::SearchSyntax => "search_syntax",
            Tool::GetDocs => "get_docs",
            Tool::Classify => "classify",
            Tool::ProjectSymbols => "project_symbols",
        }
    }

    pub fn from_name(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.name() == name)
    }

    /// Project symbols come from the scripts alone, so they need not wait
    /// for the syntax database.
    pub fn needs_catalog(self) -> bool {
        self != Tool::ProjectSymbols
    }

    pub fn run(
        self,
        arguments: &Map<String, Value>,
        context: &Context<'_>,
    ) -> Result<String, String> {
        match (self, context.knowledge) {
            (Tool::SearchSyntax, Some(knowledge)) => search_syntax(arguments, knowledge),
            (Tool::GetDocs, Some(knowledge)) => get_docs(arguments, knowledge),
            (Tool::Classify, Some(knowledge)) => classify(arguments, knowledge, context.project),
            (Tool::ProjectSymbols, _) => project_symbols(arguments, context.project),
            (_, None) => Err("The Skript syntax database is not loaded.".into()),
        }
    }

    fn definition(self) -> Value {
        let categories: Vec<&str> = Category::ALL.iter().map(|c| c.label()).collect();
        let (title, description, schema) = match self {
            Tool::SearchSyntax => (
                "Search Skript syntax",
                "Find Skript syntax (effects, conditions, expressions, events, sections, \
                 structures, types, functions) by meaning or by words of its pattern. Covers core \
                 Skript plus the addons detected in the project. Returns name, category, \
                 patterns, addon, since and a one-line description for each match.",
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Words to look for, e.g. \"send action bar\"." },
                        "category": { "type": "string", "enum": categories, "description": "Only this kind of syntax." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SEARCH_LIMIT, "description": "At most this many results (default 10)." }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            ),
            Tool::GetDocs => (
                "Get Skript docs",
                "The full documentation of one syntax entry, as markdown: description, syntax \
                 patterns, event values, examples, version and requirements, and which addon \
                 provides it. Look an entry up by its name or id as search_syntax returned it.",
                json!({
                    "type": "object",
                    "properties": {
                        "name_or_id": { "type": "string", "description": "An entry's name (\"Action Bar\") or id." },
                        "category": { "type": "string", "enum": categories, "description": "Disambiguates entries that share a name." }
                    },
                    "required": ["name_or_id"],
                    "additionalProperties": false
                }),
            ),
            Tool::Classify => (
                "Classify Skript lines",
                "What each line of Skript resolves to (category, entry name, addon), using the \
                 same classification the editor uses for highlighting and hover. Indentation \
                 matters: an unindented line is a structure or event header. Lines matching no \
                 known syntax are reported as unknown.",
                json!({
                    "type": "object",
                    "properties": {
                        "lines": {
                            "type": "array",
                            "items": { "type": "string" },
                            "maxItems": MAX_CLASSIFY_LINES,
                            "description": "Lines of a script, with their indentation, in order."
                        }
                    },
                    "required": ["lines"],
                    "additionalProperties": false
                }),
            ),
            Tool::ProjectSymbols => (
                "List project symbols",
                "What the open project's scripts declare: functions with their signatures, \
                 commands, options, ClassySK classes with their fields and methods, and oopsk \
                 structs with their fields, each with its file and line.",
                json!({
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["all", "function", "command", "option", "class", "struct"], "description": "Only this kind of symbol (default all)." },
                        "query": { "type": "string", "description": "Only symbols whose name contains this." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SYMBOL_LIMIT, "description": "At most this many symbols (default 100)." }
                    },
                    "additionalProperties": false
                }),
            ),
        };
        json!({
            "name": self.name(),
            "title": title,
            "description": description,
            "inputSchema": schema,
            "annotations": {
                "title": title,
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        })
    }
}

/// The `tools/list` result.
pub fn list() -> Value {
    let tools: Vec<Value> = Tool::ALL.into_iter().map(Tool::definition).collect();
    json!({ "tools": tools })
}

// ---------------------------------------------------------------- arguments

pub(super) fn string_arg<'a>(
    arguments: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn required_string<'a>(arguments: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    string_arg(arguments, key)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| format!("`{key}` is required and must be a non-empty string"))
}

/// A limit clamped into range: an over-large request is capped rather than
/// refused, so the model still gets an answer.
pub(super) fn limit_arg(
    arguments: &Map<String, Value>,
    default: usize,
    max: usize,
) -> Result<usize, String> {
    match arguments.get("limit") {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|&limit| limit >= 1)
            .map(|limit| (limit as usize).min(max))
            .ok_or_else(|| "`limit` must be a positive integer".to_string()),
    }
}

fn category_arg(arguments: &Map<String, Value>) -> Result<Option<Category>, String> {
    let Some(label) = string_arg(arguments, "category")? else {
        return Ok(None);
    };
    Category::from_label(&label.to_ascii_lowercase())
        .map(Some)
        .ok_or_else(|| {
            let labels: Vec<&str> = Category::ALL.iter().map(|c| c.label()).collect();
            format!(
                "unknown category `{label}`; expected one of {}",
                labels.join(", ")
            )
        })
}

// ------------------------------------------------------------------- output

/// Cuts `text` to `max` chars on a char boundary, marking the cut.
pub(super) fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// Serialises `head` plus as many of `items` as fit in [`MAX_TEXT`] under
/// `key`, setting `truncated` when some had to be left out. Cutting the JSON
/// text itself would hand the model something unparseable.
pub(super) fn fit(mut head: Map<String, Value>, key: &str, items: Vec<Value>) -> String {
    let base = Value::Object(head.clone()).to_string().len() + key.len() + 40;
    let mut used = base;
    let total = items.len();
    let mut kept = Vec::new();
    for item in items {
        let size = item.to_string().len() + 1;
        if used + size > MAX_TEXT {
            break;
        }
        used += size;
        kept.push(item);
    }
    if kept.len() < total {
        head.insert("truncated".into(), json!(true));
        head.insert("omitted".into(), json!(total - kept.len()));
    }
    head.insert(key.into(), Value::Array(kept));
    Value::Object(head).to_string()
}

/// The common head of a catalog answer, warning when the real database is
/// missing so "nothing found" is not taken at face value.
fn catalog_head(knowledge: &Knowledge) -> Map<String, Value> {
    let mut head = Map::new();
    if let Some(warning) = &knowledge.degraded {
        head.insert(
            "warning".into(),
            json!(format!("Only a tiny built-in catalog is loaded: {warning}")),
        );
    }
    head
}

fn addon_label(entry: &Entry) -> &str {
    entry.provider()
}

// ------------------------------------------------------------ search_syntax

fn search_syntax(arguments: &Map<String, Value>, knowledge: &Knowledge) -> Result<String, String> {
    let query = required_string(arguments, "query")?;
    let category = category_arg(arguments)?;
    let limit = limit_arg(arguments, DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT)?;
    let catalog = &knowledge.catalog;

    let results: Vec<Value> = search::search(catalog, query, category, limit)
        .into_iter()
        .filter_map(|id| Some(summary(id, catalog.entry(id)?, catalog)))
        .collect();

    let mut head = catalog_head(knowledge);
    head.insert("query".into(), json!(query));
    if results.is_empty() {
        head.insert(
            "hint".into(),
            json!("No match. Try other words, or drop the category filter."),
        );
    }
    Ok(fit(head, "results", results))
}

/// One search result.
fn summary(id: EntryId, entry: &Entry, catalog: &Catalog) -> Value {
    let mut out = Map::new();
    out.insert("name".into(), json!(display_name(entry)));
    out.insert("category".into(), json!(id.category.label()));
    if !entry.id.is_empty() {
        out.insert("id".into(), json!(entry.id));
    }
    out.insert("addon".into(), json!(addon_label(entry)));
    if let Some(since) = entry.since.first() {
        out.insert("since".into(), json!(since));
    }
    let patterns: Vec<String> = entry
        .patterns
        .iter()
        .take(MAX_PATTERNS_SHOWN)
        .map(|pattern| clip(pattern, SHORT))
        .collect();
    out.insert("patterns".into(), json!(patterns));
    if entry.patterns.len() > MAX_PATTERNS_SHOWN {
        out.insert(
            "more_patterns".into(),
            json!(entry.patterns.len() - MAX_PATTERNS_SHOWN),
        );
    }
    let description = skript_docs::hover::summary(entry);
    if !description.is_empty() {
        out.insert("description".into(), json!(clip(&description, SHORT)));
    }
    if entry.is_deprecated() {
        out.insert("deprecated".into(), json!(true));
    }
    if let Some(note) = catalog.availability(id) {
        out.insert("note".into(), json!(note));
    }
    Value::Object(out)
}

fn display_name(entry: &Entry) -> &str {
    if entry.name.is_empty() {
        &entry.id
    } else {
        &entry.name
    }
}

// ----------------------------------------------------------------- get_docs

fn get_docs(arguments: &Map<String, Value>, knowledge: &Knowledge) -> Result<String, String> {
    let wanted = required_string(arguments, "name_or_id")?.trim();
    let category = category_arg(arguments)?;
    let catalog = &knowledge.catalog;

    let matches = find_entries(catalog, wanted, category);
    let Some(&first) = matches.first() else {
        let closest: Vec<String> = search::search(catalog, wanted, category, 5)
            .into_iter()
            .filter_map(|id| {
                let entry = catalog.entry(id)?;
                Some(format!("{} ({})", display_name(entry), id.category.label()))
            })
            .collect();
        let mut message = format!("No entry is named or has the id `{wanted}`.");
        if !closest.is_empty() {
            message.push_str(&format!(" Closest: {}.", closest.join(", ")));
        }
        return Err(message);
    };

    let hover = catalog
        .hover(first)
        .ok_or_else(|| "That entry has no documentation.".to_string())?;
    let mut text = String::new();
    if let Some(warning) = &knowledge.degraded {
        text.push_str(&format!(
            "> Only a tiny built-in catalog is loaded: {warning}\n\n"
        ));
    }
    let notes = provenance_notes(knowledge, first);
    if !notes.is_empty() {
        text.push_str(&notes.join(" · "));
        text.push_str("\n\n");
    }
    text.push_str(&hover);

    if matches.len() > 1 {
        let others: Vec<String> = matches[1..]
            .iter()
            .take(10)
            .filter_map(|&id| {
                let entry = catalog.entry(id)?;
                Some(format!(
                    "{} `{}` ({})",
                    id.category.label(),
                    if entry.id.is_empty() {
                        display_name(entry)
                    } else {
                        &entry.id
                    },
                    addon_label(entry)
                ))
            })
            .collect();
        text.push_str(&format!(
            "\n\n---\n\nOther entries match `{wanted}`: {}. Pass `category` or an id to pick one.",
            others.join(", ")
        ));
    }

    if text.chars().count() > MAX_TEXT {
        text = format!("{}\n\n[truncated]", clip(&text, MAX_TEXT));
    }
    Ok(text)
}

/// Entries whose id or name is `wanted`, ignoring case. Id matches first, as
/// ids are unique and names are not; then core Skript before addons.
fn find_entries(catalog: &Catalog, wanted: &str, category: Option<Category>) -> Vec<EntryId> {
    let categories: &[Category] = match &category {
        Some(category) => std::slice::from_ref(category),
        None => Category::ALL,
    };
    let mut found: Vec<(u8, EntryId)> = Vec::new();
    for &category in categories {
        for (index, entry) in catalog.docs().entries(category).iter().enumerate() {
            let by_id = !entry.id.is_empty() && entry.id.eq_ignore_ascii_case(wanted);
            let by_name = !entry.name.is_empty() && entry.name.eq_ignore_ascii_case(wanted);
            if by_id || by_name {
                let rank = u8::from(!by_id) * 2 + u8::from(entry.addon.is_some());
                found.push((rank, EntryId { category, index }));
            }
        }
    }
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, id)| id).collect()
}

/// Availability and provenance, worded as the editor's hover words them.
fn provenance_notes(knowledge: &Knowledge, id: EntryId) -> Vec<String> {
    let catalog = &knowledge.catalog;
    let mut notes = Vec::new();
    if let Some(note) = catalog.availability(id) {
        notes.push(format!("**Note:** {note}"));
    }
    let Some(addon) = catalog.entry(id).and_then(|entry| entry.addon.as_ref()) else {
        return notes;
    };
    let installed = knowledge
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
    let state = if knowledge.detection.is_known() && !installed {
        ", not installed"
    } else {
        ""
    };
    notes.push(format!("from {label}{version}{state}"));
    notes
}

// ----------------------------------------------------------------- classify

fn classify(
    arguments: &Map<String, Value>,
    knowledge: &Knowledge,
    project: Option<&Path>,
) -> Result<String, String> {
    let lines: Vec<&str> = match arguments.get("lines") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .ok_or("every item of `lines` must be a string")
            })
            .collect::<Result<_, _>>()?,
        _ => return Err("`lines` is required and must be an array of strings".into()),
    };
    let skipped = lines.len().saturating_sub(MAX_CLASSIFY_LINES);
    // One string holding several lines is split, so the indentation that
    // decides each line's role is still seen.
    let text = lines[..lines.len() - skipped].join("\n");
    let document = Document::new("mcp:snippet.sk", text.as_str());

    let mut lookup = Lookup {
        knowledge,
        document: &document,
        block_comments: lines::block_comment_lines(&text),
        project,
        project_functions: None,
    };
    let mut results = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let Some(mut result) = lookup.classify(number as u32, raw) else {
            continue;
        };
        result.insert("line".into(), json!(number + 1));
        result.insert("text".into(), json!(clip(raw.trim(), SHORT)));
        results.push(Value::Object(result));
    }

    let mut head = catalog_head(knowledge);
    if skipped > 0 {
        head.insert(
            "skipped_lines".into(),
            json!(format!(
                "{skipped} line(s) past the first {MAX_CLASSIFY_LINES} were not classified"
            )),
        );
    }
    Ok(fit(head, "lines", results))
}

/// What [`classify`] needs to place each line of one snippet.
struct Lookup<'a> {
    knowledge: &'a Knowledge,
    document: &'a Document,
    block_comments: HashSet<usize>,
    project: Option<&'a Path>,
    /// Read off disk only when a line calls a function the catalog does not
    /// know, which most snippets never do.
    project_functions: Option<HashMap<String, String>>,
}

impl Lookup<'_> {
    /// What one line is. `None` for a blank line, which is not worth a result.
    fn classify(&mut self, number: u32, raw: &str) -> Option<Map<String, Value>> {
        let (knowledge, document) = (self.knowledge, self.document);
        let mut out = Map::new();
        let block_comment = lines::is_block_comment_delimiter(raw)
            || self.block_comments.contains(&(number as usize));
        let code = lines::code_part(raw).trim();
        if code.is_empty() || block_comment {
            if raw.trim().is_empty() {
                return None;
            }
            out.insert("kind".into(), json!("comment"));
            return Some(out);
        }
        if raw.chars().count() > MAX_LINE_CHARS {
            out.insert("kind".into(), json!("unknown"));
            out.insert("note".into(), json!("line too long to classify"));
            return Some(out);
        }
        let symbols = document.symbols();

        // Declarations the grammar owns, checked first as the editor's colouring
        // does: their catalog patterns end in a regex that claims the whole line.
        if let Some(symbol) = declaration_on(symbols, number) {
            out.insert("kind".into(), json!("declaration"));
            out.insert("declares".into(), json!(symbol_kind_label(symbol)));
            out.insert("name".into(), json!(symbol.name));
            if !symbol.detail.is_empty() {
                out.insert("detail".into(), json!(clip(&symbol.detail, SHORT)));
            }
            return Some(out);
        }

        let catalog = &knowledge.catalog;
        let role = LineRole::from_indent(skript_index::indent_width(raw));
        let pattern_code = code.trim_end_matches(':').trim_end();
        // The same filter as semantic tokens: a match that explains none of the
        // line's own words is a catch-all, not an answer.
        let classified = catalog
            .classify_line(pattern_code, role)
            .filter(|(_, matched)| matched.explains_text(pattern_code));
        if let Some((id, _)) = classified {
            if let Some(entry) = catalog.entry(id) {
                out.insert("kind".into(), json!("syntax"));
                out.insert("category".into(), json!(id.category.label()));
                out.insert("name".into(), json!(display_name(entry)));
                if !entry.id.is_empty() {
                    out.insert("id".into(), json!(entry.id));
                }
                out.insert("addon".into(), json!(addon_label(entry)));
                if entry.is_deprecated() {
                    out.insert("deprecated".into(), json!(true));
                }
                if let Some(note) = catalog.availability(id) {
                    out.insert("note".into(), json!(note));
                }
                return Some(out);
            }
        }

        // A bare call's effect is registered internally and is in no published
        // pattern, so the parse tree answers instead.
        if let Some(call) = symbols.references.iter().find(|reference| {
            matches!(reference.kind, SymbolKind::Function | SymbolKind::Method)
                && reference.name_range.start.line == number
        }) {
            out.insert("kind".into(), json!("function call"));
            out.insert("name".into(), json!(call.name));
            let builtin = catalog
                .docs()
                .entries(Category::Function)
                .iter()
                .any(|entry| entry.name.eq_ignore_ascii_case(&call.name));
            if builtin {
                out.insert("declared_in".into(), json!("Skript (built-in function)"));
            } else if call.kind == SymbolKind::Function {
                let project = self.project;
                let functions = self
                    .project_functions
                    .get_or_insert_with(|| index_functions(project));
                if let Some(place) = functions.get(&call.name.to_ascii_lowercase()) {
                    out.insert("declared_in".into(), json!(place));
                }
            }
            return Some(out);
        }

        if let Some(entry) = entry_on(symbols, number) {
            out.insert("kind".into(), json!("structure entry"));
            out.insert("name".into(), json!(entry.name));
            if let Some(curated) = entries::lookup(entries::COMMAND, &entry.name) {
                out.insert("summary".into(), json!(curated.summary));
            }
            return Some(out);
        }

        out.insert("kind".into(), json!("unknown"));
        let from_addon = knowledge.uninstalled.as_ref().and_then(|rest| {
            let (id, _) = rest.classify_line(pattern_code, role)?;
            let addon = rest.entry(id)?.addon.as_ref()?;
            Some(addon.name.clone())
        });
        if let Some(addon) = from_addon {
            out.insert(
                "note".into(),
                json!(format!(
                    "matches `{addon}` syntax, and `{addon}` is not installed"
                )),
            );
        }
        Some(out)
    }
}

/// A function, command or ClassySK/oopsk declaration made on `line`.
fn declaration_on(symbols: &FileSymbols, line: u32) -> Option<&Symbol> {
    symbols.flat().into_iter().find(|symbol| match symbol.kind {
        SymbolKind::Class | SymbolKind::Field | SymbolKind::Method => {
            symbol.range.start.line == line
        }
        SymbolKind::Function | SymbolKind::LocalFunction | SymbolKind::Command => {
            symbol.selection_range.start.line == line
        }
        _ => false,
    })
}

/// A command entry, option, alias or variable default keyed on `line`.
fn entry_on(symbols: &FileSymbols, line: u32) -> Option<&Symbol> {
    symbols.flat().into_iter().find(|symbol| {
        matches!(
            symbol.kind,
            SymbolKind::Entry
                | SymbolKind::Option
                | SymbolKind::Alias
                | SymbolKind::GlobalVariable
                | SymbolKind::LocalVariable
        ) && symbol.selection_range.start.line == line
    })
}
