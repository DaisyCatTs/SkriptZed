//! `{_x}` read in a trigger that never sets it, and `{_x}` set but never read.
//!
//! Skript scopes a local to one trigger, so both are visible from the trigger
//! alone. Neither is an error to Skript, and both are reported only when no
//! reading of the trigger could make them wrong. Any doubt counts the other
//! way:
//!
//! * a write is anything that *might* write: every variable on a line that
//!   starts with a change verb (`set`, `add`, `loop`, `for`, …), on an addon
//!   section's header, right after `in`/`into`/`to`/`as`/`from`/`with` (`…
//!   and store it in {_x}`), or at the start of a line (skript-reflect's
//!   `{_x}.field`);
//! * a read is anything that is not certainly a write: only `set {_x} to`,
//!   `add … to {_x}`, `remove … from {_x}` and `delete {_x}` are;
//! * a list (`{_x::*}`, `{_x::1}`) is skipped wholesale, and so is a trigger
//!   with a dynamic name (`{_%name%}`), which could be any variable;
//! * a variable whose text appears in the trigger more often than the parse
//!   tree saw it (`evaluate "set {_x} to 1"`) is skipped;
//! * only events, commands, functions and ClassySK methods are checked: a
//!   skript-reflect `expression …:` hands locals from `parse:` to `get:`.

use std::collections::HashSet;

use skript_docs::{Catalog, Category, LineRole};
use skript_index::{Document, Range, Reference, Symbol, SymbolKind};

use super::layout::{is_control_flow, Lines};
use super::{Diagnostic, Severity};

pub(super) const NEVER_SET: &str = "local-never-set";
pub(super) const NEVER_READ: &str = "local-never-read";

/// Words that start a statement which may write any variable on its line.
const WRITE_VERBS: &[&str] = &[
    "set", "add", "remove", "delete", "clear", "reset", "increase", "decrease", "subtract",
    "multiply", "divide", "push", "erase", "loop", "for", "make", "create", "copy", "parse",
    "change", "store", "save", "load", "split", "sort", "shuffle", "reverse",
];

/// Words after which a variable is usually a destination.
const DESTINATION_WORDS: &[&str] = &["in", "into", "to", "as", "from", "with"];

/// A command's `trigger:` and friends are entries, not sections; code under
/// them runs in the command's own event.
fn is_command_entry(header: &str) -> bool {
    let lower = header.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "trigger" | "suggestions" | "tab completions" | "tab complete" | "subcommands"
    ) || lower.starts_with("subcommand ")
}

/// One use of a local, with the text around it.
struct Occurrence<'t> {
    reference: &'t Reference,
    /// The variable as written, braces included.
    text: &'t str,
    /// The line's code, comment and indentation removed.
    code: &'t str,
    before: &'t str,
    after: &'t str,
    /// Every section between the trigger and this line is control flow.
    plain: bool,
}

impl Occurrence<'_> {
    fn first_word(&self) -> String {
        self.code
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    }

    fn may_write(&self) -> bool {
        let first = self.first_word();
        let is_header = self.code.trim_end().ends_with(':');
        let previous = self
            .before
            .split_whitespace()
            .next_back()
            .unwrap_or("")
            .to_ascii_lowercase();
        WRITE_VERBS.contains(&first.as_str())
            || (is_header && !matches!(first.as_str(), "if" | "else" | "while"))
            || DESTINATION_WORDS.contains(&previous.as_str())
            || self.before.trim().is_empty()
    }

    /// `set {_x} to …`, and nothing else that could read the old value.
    fn is_set_target(&self) -> bool {
        self.before.trim().eq_ignore_ascii_case("set")
            && self
                .after
                .trim_start()
                .get(..3)
                .is_some_and(|word| word.eq_ignore_ascii_case("to "))
    }

    fn certainly_writes(&self) -> bool {
        if self.is_set_target() {
            return true;
        }
        if !self.after.trim().is_empty() {
            return false;
        }
        let before = self.before.trim().to_ascii_lowercase();
        matches!(before.as_str(), "delete" | "clear" | "reset")
            || (before.starts_with("add ") && before.ends_with(" to"))
            || (before.starts_with("remove ") && before.ends_with(" from"))
    }
}

/// Whether a top-level structure runs as a trigger whose locals start empty.
fn is_trigger(symbol: &Symbol, header: &str, catalog: Option<&Catalog>) -> bool {
    match symbol.kind {
        SymbolKind::Command | SymbolKind::Function | SymbolKind::LocalFunction => true,
        SymbolKind::Method => true,
        SymbolKind::Event => {
            let lower = header.to_ascii_lowercase();
            lower.starts_with("on ")
                || lower.starts_with("every ")
                || catalog
                    .and_then(|catalog| catalog.classify_line(header, LineRole::from_indent(0)))
                    .is_some_and(|(id, _)| id.category == Category::Event)
        }
        _ => false,
    }
}

/// The names a trigger starts with: a function's or method's parameters, a
/// command's named arguments (`<name: text>`) on its header or any of its
/// subcommands. Unnamed arguments are not stored as variables.
fn declared_names(owner: &Symbol, lines: &Lines<'_>) -> HashSet<String> {
    if owner.kind != SymbolKind::Command {
        return owner
            .children
            .iter()
            .filter(|child| child.kind == SymbolKind::LocalVariable)
            .map(|child| child.name.trim().to_ascii_lowercase())
            .collect();
    }
    let mut names = HashSet::new();
    for number in owner.range.start.line..=owner.range.end.line {
        let Some(code) = lines.code(number as usize) else {
            continue;
        };
        let lower = code.trim_start().to_ascii_lowercase();
        if !(lower.starts_with("command ") || lower.starts_with("subcommand ")) {
            continue;
        }
        let mut rest = code;
        while let Some(open) = rest.find('<') {
            let Some(close) = rest[open..].find('>') else {
                break;
            };
            let spec = &rest[open + 1..open + close];
            if let Some((name, _)) = spec.split_once(':').filter(|(name, _)| {
                !name.trim().is_empty()
                    && name
                        .trim()
                        .chars()
                        .all(|ch| ch.is_alphanumeric() || ch == '_')
            }) {
                names.insert(name.trim().to_ascii_lowercase());
            }
            rest = &rest[open + close + 1..];
        }
    }
    names
}

/// What the dynamic locals in a trigger's text could stand for.
enum Dynamic {
    /// None at all.
    Nothing,
    /// Only list elements, `{_homes::%player%}`: these roots are unsafe.
    Roots(HashSet<String>),
    /// `{_%name%}`, which could be any variable.
    Anything,
}

fn dynamic_locals(scope_lines: &[&str]) -> Dynamic {
    let mut roots = HashSet::new();
    for code in scope_lines {
        let mut rest = *code;
        while let Some(at) = rest.find("{_") {
            let body = &rest[at + 2..];
            let mut depth = 1u32;
            let mut end = body.len();
            for (index, ch) in body.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    end = index;
                    break;
                }
            }
            let name = &body[..end];
            if let Some(cut) = name.find(['%', '{']) {
                match name[..cut].split_once("::") {
                    Some((root, _)) => {
                        roots.insert(root.trim().to_ascii_lowercase());
                    }
                    None => return Dynamic::Anything,
                }
            }
            rest = &body[end.min(body.len())..];
        }
    }
    if roots.is_empty() {
        Dynamic::Nothing
    } else {
        Dynamic::Roots(roots)
    }
}

pub(super) fn check(document: &Document, catalog: Option<&Catalog>, out: &mut Vec<Diagnostic>) {
    // A broken parse may have put a line in the wrong trigger.
    if document.has_errors() {
        return;
    }
    let lines = Lines::new(document.text());
    let symbols = document.symbols();
    let flat = symbols.flat();
    // An option's value can be code that uses a local: `{@log}` expanding to
    // `send "%{_msg}%"` reads `{_msg}` where no `{_msg}` is written.
    let options_use_locals = flat.iter().any(|symbol| {
        symbol.kind == SymbolKind::Structure
            && symbol.name == "options"
            && (symbol.range.start.line..=symbol.range.end.line).any(|number| {
                lines
                    .code(number as usize)
                    .is_some_and(|c| c.contains("{_"))
            })
    });

    let mut scopes: Vec<(Range, Vec<&Reference>)> = Vec::new();
    for reference in &symbols.references {
        let Some(scope) = reference
            .scope
            .filter(|_| reference.kind == SymbolKind::LocalVariable)
        else {
            continue;
        };
        match scopes.iter_mut().find(|(range, _)| *range == scope) {
            Some((_, list)) => list.push(reference),
            None => scopes.push((scope, vec![reference])),
        }
    }

    for (scope, references) in scopes {
        let Some(owner) = flat.iter().find(|symbol| symbol.range == scope) else {
            continue;
        };
        let header = lines
            .code(scope.start.line as usize)
            .map(|code| code.trim().trim_end_matches(':').trim_end())
            .unwrap_or_default();
        if !is_trigger(owner, header, catalog) {
            continue;
        }
        check_trigger(owner, &references, &lines, options_use_locals, out);
    }
}

fn check_trigger(
    owner: &Symbol,
    references: &[&Reference],
    lines: &Lines<'_>,
    options_use_locals: bool,
    out: &mut Vec<Diagnostic>,
) {
    let scope_lines: Vec<&str> = (owner.range.start.line..=owner.range.end.line)
        .filter_map(|number| lines.code(number as usize))
        .collect();
    if options_use_locals && scope_lines.iter().any(|code| code.contains("{@")) {
        return;
    }
    let mut list_roots = match dynamic_locals(&scope_lines) {
        Dynamic::Anything => return,
        Dynamic::Nothing => HashSet::new(),
        Dynamic::Roots(roots) => roots,
    };
    let root_of = |name: &str| {
        name.split("::")
            .next()
            .unwrap_or(name)
            .trim()
            .to_ascii_lowercase()
    };
    list_roots.extend(
        references
            .iter()
            .filter(|reference| reference.name.contains("::"))
            .map(|reference| root_of(&reference.name)),
    );
    let declared = declared_names(owner, lines);
    let lowered: Vec<String> = scope_lines
        .iter()
        .map(|code| code.to_ascii_lowercase())
        .collect();

    let mut names: Vec<String> = Vec::new();
    for reference in references {
        let name = reference.name.trim().to_ascii_lowercase();
        if !names.contains(&name) {
            names.push(name);
        }
    }

    for name in names {
        if list_roots.contains(&root_of(&name)) || name.contains(['{', '}', '@', '%']) {
            continue;
        }
        // In document order: the index collects references depth-first from
        // the end, and "never set" is reported on the first use.
        let mut group: Vec<Occurrence<'_>> = references
            .iter()
            .filter(|reference| reference.name.trim().eq_ignore_ascii_case(&name))
            .filter_map(|reference| occurrence(reference, lines))
            .collect();
        group.sort_by_key(|occurrence| occurrence.reference.range.start);
        // Text the tree did not see as a variable may still use it.
        let needle = format!("{{_{name}}}");
        let textual: usize = lowered
            .iter()
            .map(|code| code.matches(&needle).count())
            .sum();
        if group.is_empty() || textual != group.len() || !group.iter().all(|o| o.plain) {
            continue;
        }

        if !declared.contains(&name) && !group.iter().any(Occurrence::may_write) {
            let first = &group[0];
            out.push(Diagnostic {
                range: first.reference.range,
                severity: Severity::Warning,
                message: format!(
                    "`{}` is never set in this trigger, so it is always empty here.",
                    first.text
                ),
                code: NEVER_SET,
                related: Vec::new(),
            });
        }

        let dead = !declared.contains(&name)
            && group.iter().all(Occurrence::certainly_writes)
            && group.iter().any(Occurrence::is_set_target);
        if dead {
            for occurrence in &group {
                out.push(Diagnostic {
                    range: occurrence.reference.range,
                    severity: Severity::Hint,
                    message: format!(
                        "`{}` is set but never read in this trigger.",
                        occurrence.text
                    ),
                    code: NEVER_READ,
                    related: Vec::new(),
                });
            }
        }
    }
}

fn occurrence<'t>(reference: &'t Reference, lines: &Lines<'t>) -> Option<Occurrence<'t>> {
    let number = reference.range.start.line;
    if reference.range.end.line != number {
        return None;
    }
    let code = lines.code(number as usize)?;
    let start = reference.range.start.character as usize;
    let end = reference.range.end.character as usize;
    let headers = lines.enclosing_headers(number as usize);
    let plain = headers.split_last().is_some_and(|(_, inner)| {
        inner
            .iter()
            .all(|h| is_control_flow(h) || is_command_entry(h))
    });
    Some(Occurrence {
        reference,
        text: code.get(start..end)?,
        code: code.trim_start(),
        before: code.get(..start)?,
        after: code.get(end..)?,
        plain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_index::Workspace;

    fn found(source: &str) -> Vec<(&'static str, u32, String)> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let document = workspace.get("file:///t.sk").unwrap();
        let mut out = Vec::new();
        check(document, None, &mut out);
        out.sort_by_key(|d| d.range.start);
        out.into_iter()
            .map(|d| (d.code, d.range.start.line, d.message))
            .collect()
    }

    fn codes(source: &str) -> Vec<&'static str> {
        found(source).into_iter().map(|(code, _, _)| code).collect()
    }

    #[test]
    fn a_local_read_but_never_set_is_a_warning() {
        let out = found("on join:\n\tif player is op:\n\t\tsend \"%{_x}%\" to player\n");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!((out[0].0, out[0].1), (NEVER_SET, 2));
        assert!(out[0].2.contains("`{_x}`"), "{out:?}");
    }

    #[test]
    fn never_set_is_reported_on_the_first_use() {
        let out = found("on join:\n\tsend {_x}\n\tsend {_x}\n\tsend {_x}\n");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].1, 1);
    }

    #[test]
    fn a_local_set_but_never_read_is_a_hint_on_every_write() {
        let out = found("on join:\n\tset {_x} to 1\n\tset {_x} to 2\n\tsend \"hi\"\n");
        assert_eq!(
            out.iter().map(|(c, l, _)| (*c, *l)).collect::<Vec<_>>(),
            vec![(NEVER_READ, 1), (NEVER_READ, 2)]
        );
    }

    #[test]
    fn set_then_read_is_fine_and_case_does_not_matter() {
        assert!(codes("on join:\n\tset {_X} to 1\n\tsend \"%{_x}%\"\n").is_empty());
        assert!(codes("on join:\n\tset {_x} to {_x} + 1\n").is_empty());
        assert!(codes("on join:\n\tadd 1 to {_n}\n\tsend \"%{_n}%\"\n").is_empty());
    }

    #[test]
    fn parameters_and_named_command_arguments_are_set() {
        assert!(codes("function f(p: player):\n\tsend \"hi\" to {_p}\n").is_empty());
        assert!(codes(
            "command /home:\n\tsubcommand set <name: text>:\n\t\ttrigger:\n\t\t\tsend {_name}\n"
        )
        .is_empty());
        // An unnamed argument is never stored as a variable.
        assert_eq!(
            codes("command /a <text>:\n\ttrigger:\n\t\tif {_text} is set:\n\t\t\tstop\n"),
            vec![NEVER_SET]
        );
    }

    #[test]
    fn a_classysk_method_parameter_is_set() {
        let source =
            "class W:\n\tpublic deposit(amount: number):\n\t\tadd {_amount} to self::balance\n";
        assert!(codes(source).is_empty());
    }

    #[test]
    fn each_trigger_is_its_own_scope() {
        let out = codes("on join:\n\tset {_x} to 1\non quit:\n\tif {_x} is 1:\n\t\tstop\n");
        assert_eq!(out, vec![NEVER_READ, NEVER_SET]);
    }

    #[test]
    fn anything_that_might_write_counts_as_a_write() {
        for line in [
            "retrieve the data and store it in {_x}",
            "loop {_x} times:\n\t\tstop",
            "for each {_x} in all players:\n\t\tstop",
            "{_x}.add(1)",
            "split \"a b\" at \" \" into {_x}",
        ] {
            let source = format!("on join:\n\t{line}\n\tif {{_x}} is set:\n\t\tstop\n");
            assert!(!codes(&source).contains(&NEVER_SET), "{line:?}");
        }
    }

    #[test]
    fn anything_that_might_read_counts_as_a_read() {
        for line in [
            "set {_x}'s name to \"a\"",
            "send {_x}",
            "{_x}.add(1)",
            "set {_y} to {_x}",
        ] {
            let source = format!("on join:\n\tset {{_x}} to 1\n\t{line}\n");
            let about_x = found(&source)
                .into_iter()
                .any(|(code, _, message)| code == NEVER_READ && message.contains("{_x}"));
            assert!(!about_x, "{line:?}");
        }
    }

    #[test]
    fn lists_and_dynamic_names_are_skipped() {
        assert!(codes("on join:\n\tset {_x::1} to 1\n\tsend {_x}\n").is_empty());
        assert!(codes("on join:\n\tset {_x::*} to 1 and 2\n").is_empty());
        assert!(codes("on join:\n\tset {_%player%} to 1\n\tsend {_y}\n").is_empty());
        // A dynamic list element only taints its own root.
        assert_eq!(
            codes(
                "on join:\n\tset {_h::%player%} to 1\n\tsend {_h}\n\tif {_y} is set:\n\t\tstop\n"
            ),
            vec![NEVER_SET]
        );
    }

    #[test]
    fn text_the_tree_did_not_see_hides_the_variable() {
        assert!(codes("on join:\n\tevaluate \"set {_x} to 1\"\n\tsend {_x}\n").is_empty());
    }

    #[test]
    fn an_option_that_uses_locals_disables_the_check_where_options_are_used() {
        let source = "options:\n\tlog: send \"%{_m}%\"\non join:\n\tset {_m} to \"a\"\n\t{@log}\n";
        assert!(codes(source).is_empty());
    }

    /// A GUI click handler or a skript-reflect section can run its body with
    /// values of its own.
    #[test]
    fn a_use_inside_an_addon_section_is_never_judged() {
        let source = "on join:\n\tformat slot 1 of player with stone:\n\t\tsend {_x}\n";
        assert!(codes(source).is_empty());
    }

    /// skript-reflect hands locals from `parse:` to `get:`.
    #[test]
    fn only_real_triggers_are_checked() {
        let source = "expression my thing:\n\tparse:\n\t\tset {_x} to 1\n\tget:\n\t\treturn {_y}\n";
        assert!(codes(source).is_empty(), "{:?}", codes(source));
    }

    #[test]
    fn a_comment_mentioning_the_variable_does_not_hide_it() {
        let out = codes("on join:\n\tset {_x} to 1 # {_x} is unused\n");
        assert_eq!(out, vec![NEVER_READ]);
    }
}
