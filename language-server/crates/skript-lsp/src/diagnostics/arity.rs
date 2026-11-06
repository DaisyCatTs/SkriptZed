//! Function calls with more arguments than the function takes, or fewer than
//! it needs. Skript refuses to load either.
//!
//! Counting is only done where the count is certain:
//!
//! * a function with exactly one parameter is skipped, because Skript folds
//!   `f(1, 2)` into one list argument when that parameter is plural, and the
//!   index does not know plurality;
//! * an argument holding a bare `and` or `or` is skipped. Skript parses the
//!   whole argument text as one expression list, so `f(a, b and c)` is three
//!   arguments, unless some pattern claims the `and` (`x between 1 and 5`);
//! * an unclosed call, an empty argument and an `{@option}` (which can expand
//!   to anything, commas included) all skip the call.
//!
//! A name with several declarations (a duplicate, or a local function beside
//! a global one) is accepted when any of them fits.

use std::collections::HashMap;

use skript_docs::{Catalog, Category};
use skript_index::{Document, Range, Reference, SymbolKind, Workspace};

use super::{Diagnostic, Severity};
use crate::calls;

/// How many arguments one declaration accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Arity {
    /// Parameters without a default. Named arguments let a call skip a
    /// default in the middle, so this is the loosest lower bound there is.
    required: usize,
    total: usize,
}

impl Arity {
    fn accepts(self, count: usize) -> bool {
        (self.required..=self.total).contains(&count)
    }

    fn describe(self) -> String {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        match (self.required, self.total) {
            (_, 0) => "no arguments".into(),
            (low, high) if low == high => format!("{high} argument{}", plural(high)),
            (low, high) => format!("{low} to {high} arguments"),
        }
    }
}

/// The arity of a parameter list, or `None` when it is not clear enough to
/// judge a call by.
///
/// Catalog signatures are free-form prose more than syntax: `fromBase(string
/// value: objects, …)` has a space in a name. Anything that is not
/// `name: type [= default]` gives up rather than guessing.
fn arity_of(parameters: &[String]) -> Option<Arity> {
    let mut required = 0;
    for parameter in parameters {
        let (name, ty) = parameter.split_once(':')?;
        // ClassySK's `name?: type` is optional; Skript never writes it.
        let name = name.trim();
        let (name, marked_optional) = match name.strip_suffix('?') {
            Some(name) => (name, true),
            None => (name, false),
        };
        let is_name = !name.is_empty()
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        if !is_name || ty.trim().is_empty() {
            return None;
        }
        // A type cannot contain `=`, so any `=` starts a default. The catalog
        // spells some optional parameters `world: optional world`.
        let optional = marked_optional
            || ty.contains('=')
            || ty
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("optional ");
        if !optional {
            required += 1;
        }
    }
    Some(Arity {
        required,
        total: parameters.len(),
    })
}

/// Every declaration any file can call, by name: the catalog's functions and
/// every script's global ones. `None` marks a name with at least one
/// declaration too unclear to count against.
///
/// Built once and shared by every file checked against the same project, so
/// rechecking hundreds of closed files does not rebuild it per file.
pub(super) struct Shared(HashMap<String, Option<Vec<Arity>>>);

fn record(found: &mut HashMap<String, Option<Vec<Arity>>>, name: &str, arity: Option<Arity>) {
    let slot = found
        .entry(name.to_string())
        .or_insert_with(|| Some(Vec::new()));
    match (slot.as_mut(), arity) {
        (Some(list), Some(arity)) => list.push(arity),
        _ => *slot = None,
    }
}

impl Shared {
    pub(super) fn new(workspace: &Workspace, catalog: Option<&Catalog>) -> Self {
        let mut found = HashMap::new();
        if let Some(catalog) = catalog {
            for entry in catalog.docs().entries(Category::Function) {
                let arity = match entry.patterns.as_slice() {
                    [pattern] => arity_of(&calls::signature_parameters(pattern)),
                    _ => None,
                };
                record(&mut found, &entry.name, arity);
            }
        }
        for other in workspace.documents() {
            for symbol in other.symbols().flat() {
                if symbol.kind == SymbolKind::Function {
                    record(&mut found, &symbol.name, symbol_arity(other, symbol));
                }
            }
        }
        Self(found)
    }

    /// What a call to `name` from `document` can mean, its own local
    /// functions included; `None` when undeclared or unclear.
    fn lookup(&self, document: &Document, name: &str) -> Option<Vec<Arity>> {
        let mut local = HashMap::new();
        for symbol in document.symbols().flat() {
            if symbol.kind == SymbolKind::LocalFunction && symbol.name == name {
                record(&mut local, name, symbol_arity(document, symbol));
            }
        }
        match (self.0.get(name), local.remove(name)) {
            (None, None) | (Some(None), _) | (_, Some(None)) => None,
            (global, local) => {
                let mut list = global.cloned().flatten().unwrap_or_default();
                list.extend(local.flatten().unwrap_or_default());
                Some(list)
            }
        }
    }
}

/// A script function's arity. `function f:` with no parentheses, or a
/// signature spliced in from an option, says nothing reliable about its
/// parameters.
fn symbol_arity(document: &Document, symbol: &skript_index::Symbol) -> Option<Arity> {
    let line = document.line(symbol.selection_range.start.line);
    let after = line
        .get(symbol.selection_range.end.character as usize..)
        .unwrap_or("");
    (after.trim_start().starts_with('(') && !symbol.detail.contains("{@"))
        .then(|| arity_of(&calls::signature_parameters(&symbol.detail)))
        .flatten()
}

/// Whether `text` holds the word `and` or `or` outside strings, variable
/// names and nested parentheses.
fn has_list_word(text: &str) -> bool {
    let (mut in_string, mut braces, mut parens) = (false, 0u32, 0u32);
    let mut word = String::new();
    let mut found = false;
    let mut end_word = |word: &mut String| {
        found |= word.eq_ignore_ascii_case("and") || word.eq_ignore_ascii_case("or");
        word.clear();
    };
    for ch in text.chars() {
        match ch {
            '"' if braces == 0 => in_string = !in_string,
            _ if in_string => {}
            '{' => braces += 1,
            '}' => braces = braces.saturating_sub(1),
            _ if braces > 0 => {}
            '(' => parens += 1,
            ')' => parens = parens.saturating_sub(1),
            _ if parens == 0 && ch.is_alphanumeric() => {
                word.push(ch);
                continue;
            }
            _ => {}
        }
        end_word(&mut word);
    }
    end_word(&mut word);
    found
}

/// The call's argument count, when it can be known for certain.
fn argument_count(line: &str, open: usize) -> Option<(usize, bool)> {
    let found = calls::arguments(line, open);
    if !found.closed {
        return None;
    }
    let texts: Vec<&str> = found
        .spans
        .iter()
        .map(|&(start, end)| line[start..end].trim())
        .collect();
    if texts == [""] {
        return Some((0, false));
    }
    if texts
        .iter()
        .any(|text| text.is_empty() || text.contains("{@") || has_list_word(text))
    {
        return None;
    }
    let spreads_a_list = texts.iter().any(|text| text.contains("::*"));
    Some((texts.len(), spreads_a_list))
}

/// Whether a call is a skript-reflect method (`{_list}.add(1)`) rather than a
/// Skript function that happens to share the name.
fn preceded_by_dot(line: &str, reference: &Reference) -> bool {
    line[..(reference.range.start.character as usize).min(line.len())]
        .trim_end()
        .ends_with('.')
}

pub(super) fn check(
    document: &Document,
    workspace: &Workspace,
    catalog: Option<&Catalog>,
    out: &mut Vec<Diagnostic>,
) {
    // Built lazily: most files call no function at all.
    let mut shared: Option<Shared> = None;
    let mut lookup = |name: &str| {
        shared
            .get_or_insert_with(|| Shared::new(workspace, catalog))
            .lookup(document, name)
    };
    check_calls(document, &mut lookup, out);
}

/// As [`check`], against declarations built once for many files.
pub(super) fn check_shared(document: &Document, shared: &Shared, out: &mut Vec<Diagnostic>) {
    check_calls(document, &mut |name| shared.lookup(document, name), out);
}

fn check_calls(
    document: &Document,
    lookup: &mut dyn FnMut(&str) -> Option<Vec<Arity>>,
    out: &mut Vec<Diagnostic>,
) {
    for reference in &document.symbols().references {
        if reference.kind != SymbolKind::Function || super::preceded_by_new(document, reference) {
            continue;
        }
        let line = document.line(reference.range.start.line);
        let open = reference.range.end.character as usize;
        if line.as_bytes().get(open) != Some(&b'(') || preceded_by_dot(line, reference) {
            continue;
        }
        let Some((count, spreads_a_list)) = argument_count(line, open) else {
            continue;
        };
        let Some(arities) = lookup(reference.name.as_str()) else {
            continue;
        };
        if arities.is_empty()
            || arities
                .iter()
                .any(|arity| arity.total == 1 || arity.accepts(count))
        {
            continue;
        }
        let too_few = arities.iter().all(|arity| count < arity.required);
        // Skript does not spread a list over several parameters, but whether
        // `{_args::*}` was meant to is a guess this check need not make.
        if too_few && spreads_a_list {
            continue;
        }
        out.push(arity_diagnostic(reference, &arities, count));
    }
}

fn arity_diagnostic(reference: &Reference, arities: &[Arity], count: usize) -> Diagnostic {
    let passes = format!(
        "this call passes {count} argument{}",
        if count == 1 { "" } else { "s" }
    );
    let message = match arities {
        [arity] => format!(
            "`{}` takes {}, but {passes}.",
            reference.name,
            arity.describe()
        ),
        _ => format!(
            "No declaration of `{}` takes {count} argument{}.",
            reference.name,
            if count == 1 { "" } else { "s" }
        ),
    };
    Diagnostic {
        range: Range::new(reference.range.start, reference.range.end),
        severity: Severity::Error,
        message,
        code: "function-arity",
        related: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arity_codes(source: &str) -> Vec<(u32, String)> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let mut out = Vec::new();
        check(document, &workspace, None, &mut out);
        out.sort_by_key(|d| d.range.start);
        out.into_iter()
            .map(|d| (d.range.start.line, d.message))
            .collect()
    }

    const TWO: &str = "function two(a: text, b: number = 5):\n\tstop\n\n";

    #[test]
    fn too_many_and_too_few_arguments_are_errors() {
        let found = arity_codes(&format!(
            "{TWO}on join:\n\tset {{_x}} to two(\"a\", 1, 2)\n\tset {{_y}} to two()\n"
        ));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].0, 4);
        assert!(found[0].1.contains("1 to 2 arguments"), "{found:?}");
        assert!(found[0].1.contains("passes 3"), "{found:?}");
        assert!(found[1].1.contains("passes 0"), "{found:?}");
    }

    #[test]
    fn a_default_makes_its_argument_optional() {
        let found = arity_codes(&format!(
            "{TWO}on join:\n\tset {{_x}} to two(\"a\")\n\tset {{_x}} to two(\"a\", 2)\n"
        ));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn named_arguments_are_counted() {
        let found = arity_codes(&format!(
            "{TWO}on join:\n\tset {{_x}} to two(b: 1, a: \"x\")\n"
        ));
        assert!(found.is_empty(), "{found:?}");
        let found = arity_codes(&format!(
            "{TWO}on join:\n\tset {{_x}} to two(a: \"x\", b: 1, c: 2)\n"
        ));
        assert_eq!(found.len(), 1, "{found:?}");
    }

    /// Skript folds `one(1, 2)` into a single list argument when the
    /// parameter is plural, and the index cannot tell.
    #[test]
    fn a_single_parameter_function_is_never_judged() {
        let found = arity_codes("function one(xs: numbers):\n\tstop\non join:\n\tone(1, 2, 3)\n");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_zero_parameter_function_rejects_any_argument() {
        let found = arity_codes("function none():\n\tstop\non join:\n\tnone()\n\tnone(1, 2)\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].1.contains("takes no arguments"), "{found:?}");
    }

    #[test]
    fn uncertain_counts_are_skipped() {
        // Each of these would be reported if its commas were trusted.
        let source = format!(
            "{TWO}function three(a: text, b: text, c: text):\n\tstop\noptions:\n\targs: 1, 2\non join:\n\tthree(1, 2 and 3)\n\tthree({{@args}}, 3)\n\ttwo(1, 2, 3\n\ttwo(1, , 2)\n\tthree({{_a::*}})\n"
        );
        let found = arity_codes(&source);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn nesting_strings_and_braces_do_not_split_arguments() {
        let found = arity_codes(&format!(
            "{TWO}on join:\n\ttwo(\"a, b and c\", f(1, 2))\n\ttwo({{_a, b}}, (1, 2))\n"
        ));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_reflect_method_or_constructor_is_not_the_function() {
        let found = arity_codes(&format!(
            "{TWO}on join:\n\t{{_list}}.two(1, 2, 3)\n\tset {{_x}} to new two(1, 2, 3)\n"
        ));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn any_fitting_declaration_accepts_the_call() {
        let source = "function f(a: text, b: text):\n\tstop\nlocal function f(a: text, b: text, c: text):\n\tstop\non join:\n\tf(1, 2, 3)\n";
        assert!(arity_codes(source).is_empty());
    }

    #[test]
    fn catalog_signatures_are_read_when_they_are_clear() {
        let arity = |pattern: &str| arity_of(&calls::signature_parameters(pattern));
        assert_eq!(
            arity("log(n: number, base: number = [[integer:10]])"),
            Some(Arity {
                required: 1,
                total: 2
            })
        );
        assert_eq!(
            arity("location(x: number, y: number, z: number, world: optional world)"),
            Some(Arity {
                required: 3,
                total: 4
            })
        );
        assert_eq!(
            arity("fromBase(string value: objects, base: integer)"),
            None
        );
    }

    #[test]
    fn has_list_word_ignores_strings_braces_and_parens() {
        assert!(has_list_word("1 and 2"));
        assert!(has_list_word("a OR b"));
        assert!(!has_list_word("\"a and b\""));
        assert!(!has_list_word("{_and}"));
        assert!(!has_list_word("f(1 and 2)"));
        assert!(!has_list_word("brand"));
    }
}
