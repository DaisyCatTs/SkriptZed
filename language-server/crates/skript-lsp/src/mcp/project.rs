//! `project_symbols`, and the project index `classify` resolves calls with.
//! Scripts are read fresh on every call, so an answer never describes files
//! as they were when the server started.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use skript_index::{Document, FileSymbols, Symbol, SymbolKind};

use super::tools::{
    clip, fit, limit_arg, string_arg, DEFAULT_SYMBOL_LIMIT, MAX_MEMBERS, MAX_SYMBOL_LIMIT, SHORT,
};

pub(super) fn symbol_kind_label(symbol: &Symbol) -> &'static str {
    match symbol.kind {
        SymbolKind::Function => "function",
        SymbolKind::LocalFunction => "local function",
        SymbolKind::Command => "command",
        SymbolKind::Option => "option",
        SymbolKind::Class if symbol.detail == "struct" => "struct",
        SymbolKind::Class => "class",
        SymbolKind::Field => "field",
        SymbolKind::Method => "method",
        _ => "symbol",
    }
}

/// Global functions declared in the project, lowercased name to `file:line`.
pub(super) fn index_functions(project: Option<&Path>) -> HashMap<String, String> {
    let mut functions = HashMap::new();
    for script in read_project(project) {
        for symbol in &script.symbols.symbols {
            if symbol.kind == SymbolKind::Function {
                functions
                    .entry(symbol.name.to_ascii_lowercase())
                    .or_insert_with(|| {
                        format!("{}:{}", script.path, symbol.selection_range.start.line + 1)
                    });
            }
        }
    }
    functions
}

/// One script of the project, parsed.
struct Script {
    /// Relative to the project root, with forward slashes.
    path: String,
    symbols: FileSymbols,
}

/// Every script under the project, read fresh on each call so an answer never
/// describes files as they were when the server started.
fn read_project(project: Option<&Path>) -> Vec<Script> {
    let Some(root) = project else {
        return Vec::new();
    };
    let mut scripts: Vec<Script> = skript_index::discover_scripts(root)
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let document = Document::new(path.to_string_lossy(), text);
            Some(Script {
                path: relative(root, &path),
                symbols: document.symbols().clone(),
            })
        })
        .collect();
    scripts.sort_by(|a, b| a.path.cmp(&b.path));
    scripts
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(PathBuf::from)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn project_symbols(
    arguments: &Map<String, Value>,
    project: Option<&Path>,
) -> Result<String, String> {
    let Some(root) = project else {
        return Err("No project folder is open, so there are no scripts to read.".into());
    };
    let kind = string_arg(arguments, "kind")?
        .unwrap_or("all")
        .to_ascii_lowercase();
    if !["all", "function", "command", "option", "class", "struct"].contains(&kind.as_str()) {
        return Err(format!(
            "unknown kind `{kind}`; expected all, function, command, option, class or struct"
        ));
    }
    let query = string_arg(arguments, "query")?.map(str::to_ascii_lowercase);
    let limit = limit_arg(arguments, DEFAULT_SYMBOL_LIMIT, MAX_SYMBOL_LIMIT)?;

    let scripts = read_project(Some(root));
    let mut found = Vec::new();
    let mut matched = 0;
    for script in &scripts {
        for symbol in top_level(&script.symbols) {
            let label = symbol_kind_label(symbol);
            let wanted_kind =
                kind == "all" || label == kind || (kind == "function" && label == "local function");
            let wanted_name = query
                .as_deref()
                .is_none_or(|query| symbol.name.to_ascii_lowercase().contains(query));
            if !wanted_kind || !wanted_name {
                continue;
            }
            matched += 1;
            if found.len() < limit {
                found.push(symbol_json(symbol, &script.path));
            }
        }
    }

    let mut head = Map::new();
    head.insert("project".into(), json!(root.to_string_lossy()));
    head.insert("scripts_read".into(), json!(scripts.len()));
    if matched > found.len() {
        head.insert(
            "limited".into(),
            json!(format!(
                "{matched} symbols match; showing the first {}",
                found.len()
            )),
        );
    }
    Ok(fit(head, "symbols", found))
}

/// Functions, commands, classes and structs, plus each option inside an
/// `options:` block, which the index nests under the structure.
fn top_level(symbols: &FileSymbols) -> Vec<&Symbol> {
    let mut out = Vec::new();
    for symbol in &symbols.symbols {
        match symbol.kind {
            SymbolKind::Function
            | SymbolKind::LocalFunction
            | SymbolKind::Command
            | SymbolKind::Class => out.push(symbol),
            SymbolKind::Structure => out.extend(
                symbol
                    .children
                    .iter()
                    .filter(|child| child.kind == SymbolKind::Option),
            ),
            _ => {}
        }
    }
    out
}

fn symbol_json(symbol: &Symbol, path: &str) -> Value {
    let mut out = Map::new();
    out.insert("kind".into(), json!(symbol_kind_label(symbol)));
    out.insert("name".into(), json!(symbol.name));
    let detail = clip(&symbol.detail, SHORT);
    match symbol.kind {
        SymbolKind::Function | SymbolKind::LocalFunction => {
            out.insert(
                "signature".into(),
                json!(format!("{}{}", symbol.name, detail)),
            );
        }
        SymbolKind::Command if !detail.is_empty() => {
            out.insert("arguments".into(), json!(detail));
        }
        SymbolKind::Option if !detail.is_empty() => {
            out.insert("value".into(), json!(detail));
        }
        _ => {}
    }
    out.insert("file".into(), json!(path));
    out.insert("line".into(), json!(symbol.selection_range.start.line + 1));

    if symbol.kind == SymbolKind::Class {
        let members: Vec<Value> = symbol
            .children
            .iter()
            .filter(|member| matches!(member.kind, SymbolKind::Field | SymbolKind::Method))
            .take(MAX_MEMBERS)
            .map(|member| {
                let detail = clip(&member.detail, SHORT);
                let shape = match member.kind {
                    SymbolKind::Method => format!("{}{detail}", member.name),
                    _ if detail.is_empty() => member.name.clone(),
                    _ => format!("{}: {detail}", member.name),
                };
                json!({
                    "kind": symbol_kind_label(member),
                    "name": member.name,
                    "shape": shape,
                    "line": member.selection_range.start.line + 1,
                })
            })
            .collect();
        if symbol.children.len() > MAX_MEMBERS {
            out.insert(
                "more_members".into(),
                json!(symbol.children.len() - MAX_MEMBERS),
            );
        }
        out.insert("members".into(), Value::Array(members));
    }
    Value::Object(out)
}
