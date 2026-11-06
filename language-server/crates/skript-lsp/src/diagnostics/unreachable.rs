//! Statements after a `stop` or a `return` in the same block.
//!
//! Skript warns about these itself: `ScriptLoader.loadItems` prints
//! "Unreachable code. The previous statement stops further execution." on
//! the statement after any item with an execution intent. This mirrors the
//! plain cases only, where the intent is certain from the text:
//!
//! * `stop`, `exit`, `stop trigger` and `exit trigger` (`EffExit`), which
//!   parse anywhere;
//! * `return <value>` inside a function that declares a return type, nested
//!   only in core control flow. Anywhere else `return` may fail to parse, and
//!   a statement that fails to parse never sets the intent.
//!
//! `exit 2 sections`, `continue` and an `if`/`else` that stops in every
//! branch also stop execution, but whether they parse depends on the
//! sections around them, so they are left alone. So is `stop if x`, which is
//! `EffDoIf` and stops nothing when `x` is false.
//!
//! A script that mentions `suppress … unreachable code warnings` anywhere is
//! skipped whole: the effect can switch the warning off for the rest of the
//! file or for one section, and reading which is not worth the risk.

use skript_index::{Document, Position, Range, Symbol, SymbolKind};

use super::layout::{is_control_flow, Lines};
use super::{Diagnostic, Severity};

pub(super) const CODE: &str = "unreachable-code";

/// Skript's own message.
const MESSAGE: &str = "Unreachable code. The previous statement stops further execution.";

/// `(exit|stop) [trigger]`.
fn is_stop(lower: &str) -> bool {
    let mut words = lower.split_whitespace();
    matches!(
        (words.next(), words.next(), words.next()),
        (Some("stop" | "exit"), None | Some("trigger"), None)
    )
}

/// Whether a function header declares a return type: `:: type`, `-> type`
/// or `returns type` after the parameter list.
fn returns_a_value(header: &str) -> bool {
    let Some(close) = header.rfind(')') else {
        return false;
    };
    let rest = header[close + 1..].trim().to_ascii_lowercase();
    rest.starts_with("::") || rest.starts_with("->") || rest.starts_with("returns ")
}

fn stops(lower: &str, owner: &Symbol, headers: &[&str], function_returns: bool) -> bool {
    if is_stop(lower) {
        return true;
    }
    // `return false if x is false` is `EffDoIf`, which stops nothing when
    // the condition fails; Skript's own ExprArithmetic.sk ends a function
    // with three of them. Any ` if ` (a ternary too) rules the line out.
    let returns = lower
        .strip_prefix("return ")
        .is_some_and(|value| !value.trim().is_empty() && !lower.contains(" if "));
    returns
        && owner.kind.is_function()
        && function_returns
        && headers
            .split_last()
            .is_some_and(|(_, inner)| inner.iter().all(|header| is_control_flow(header)))
}

pub(super) fn check(document: &Document, out: &mut Vec<Diagnostic>) {
    // Indentation decides what "the same block" is; a broken parse may mean
    // the indentation is wrong too.
    if document.has_errors() {
        return;
    }
    let text = document.text();
    if text
        .to_ascii_lowercase()
        .contains("unreachable code warning")
    {
        return;
    }
    let lines = Lines::new(text);

    for owner in &document.symbols().symbols {
        if !matches!(
            owner.kind,
            SymbolKind::Event
                | SymbolKind::Command
                | SymbolKind::Function
                | SymbolKind::LocalFunction
        ) {
            continue;
        }
        let start = owner.range.start.line as usize;
        let end = (owner.range.end.line as usize).min(lines.lines.len().saturating_sub(1));
        let function_returns = owner.kind.is_function()
            && lines
                .code(start)
                .is_some_and(|header| returns_a_value(header.trim_end().trim_end_matches(':')));

        for number in start + 1..=end {
            let Some(code) = lines.code(number) else {
                continue;
            };
            let lower = code.trim().to_ascii_lowercase();
            let headers = lines.enclosing_headers(number);
            // A command's code lives under its `trigger:` entry; a bare line
            // beside the entries is not a statement.
            if owner.kind == SymbolKind::Command
                && !headers
                    .iter()
                    .any(|header| header.eq_ignore_ascii_case("trigger"))
            {
                continue;
            }
            if !stops(&lower, owner, &headers, function_returns) {
                continue;
            }
            let indent = skript_index::indent_width(code);
            if let Some(range) = dead_run(&lines, number, end, indent) {
                out.push(Diagnostic {
                    range,
                    severity: Severity::Warning,
                    message: MESSAGE.into(),
                    code: CODE,
                    related: Vec::new(),
                });
            }
        }
    }
}

/// The statements after line `number` in its block, through the last line
/// the block holds, when the next code line is a statement at the same
/// depth.
fn dead_run(lines: &Lines<'_>, number: usize, end: usize, indent: usize) -> Option<Range> {
    let mut code_lines = (number + 1..=end).filter_map(|at| lines.code(at).map(|code| (at, code)));
    let (first, first_code) = code_lines.next()?;
    if skript_index::indent_width(first_code) != indent {
        return None;
    }
    // `else` after a `stop` is a misplaced `else`, which Skript reports as
    // an error of its own.
    let lower = first_code.trim().to_ascii_lowercase();
    if lower == "else" || lower.starts_with("else ") || lower.starts_with("else:") {
        return None;
    }
    let (last, last_code) = std::iter::once((first, first_code))
        .chain(code_lines)
        .take_while(|(_, code)| skript_index::indent_width(code) >= indent)
        .last()?;
    Some(Range::new(
        Position::new(first as u32, indent as u32),
        Position::new(last as u32, last_code.trim_end().len() as u32),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_index::Workspace;

    fn found(source: &str) -> Vec<Range> {
        let mut workspace = Workspace::new();
        workspace.open("file:///t.sk", source);
        let mut out = Vec::new();
        check(workspace.get("file:///t.sk").unwrap(), &mut out);
        out.into_iter().map(|d| d.range).collect()
    }

    #[test]
    fn a_statement_after_stop_is_unreachable() {
        let out = found("on join:\n\tstop\n\tsend \"hi\"\n");
        assert_eq!(out, vec![Range::on_line(2, 1, 10)]);
    }

    #[test]
    fn the_range_covers_the_rest_of_the_block_and_no_more() {
        let source =
            "on join:\n\tif player is op:\n\t\texit\n\t\tsend \"a\"\n\t\tif x:\n\t\t\tsend \"b\"\n\tsend \"c\"\n";
        let out = found(source);
        assert_eq!(
            out,
            vec![Range::new(Position::new(3, 2), Position::new(5, 11))]
        );
    }

    #[test]
    fn stop_at_the_end_of_a_block_is_fine() {
        assert!(found("on join:\n\tif x:\n\t\tstop\n\tsend \"a\"\n").is_empty());
        assert!(found("on join:\n\tsend \"a\"\n\tstop\n").is_empty());
        assert!(found("on join:\n\tstop\n\t# trailing comment\n").is_empty());
    }

    #[test]
    fn only_the_plain_stop_effect_counts() {
        for line in [
            "stop if player is op",
            "stop 2 sections",
            "stop the loop",
            "continue",
            "send \"stop\"",
            "stop all sounds",
        ] {
            let source = format!("on join:\n\t{line}\n\tsend \"a\"\n");
            assert!(found(&source).is_empty(), "{line:?}");
        }
        assert_eq!(found("on join:\n\tStop Trigger\n\tsend \"a\"\n").len(), 1);
    }

    #[test]
    fn return_counts_in_a_function_that_returns_a_value() {
        assert_eq!(
            found("function f() :: number:\n\treturn 1\n\tsend \"a\"\n").len(),
            1
        );
        assert_eq!(
            found("function f() returns number:\n\tif x:\n\t\treturn 1\n\t\tsend \"a\"\n").len(),
            1
        );
        // A conditional return stops nothing when its condition fails.
        assert!(
            found("function f() :: boolean:\n\treturn false if {_x} is set\n\treturn true\n")
                .is_empty()
        );
        // No return type: Skript refuses the `return`, so it stops nothing.
        assert!(found("function f():\n\treturn 1\n\tsend \"a\"\n").is_empty());
        // Outside a function, `return` belongs to whatever handles it.
        assert!(found("on join:\n\treturn 1\n\tsend \"a\"\n").is_empty());
        // An addon section may be its own return handler.
        assert!(found(
            "function f() :: number:\n\trun section {_s}:\n\t\treturn 1\n\t\tsend \"a\"\n"
        )
        .is_empty());
    }

    #[test]
    fn a_command_counts_under_its_trigger() {
        assert_eq!(
            found("command /a:\n\ttrigger:\n\t\tstop\n\t\tsend \"a\"\n").len(),
            1
        );
    }

    #[test]
    fn a_misplaced_else_is_not_this_warning() {
        assert!(found("on join:\n\tif x:\n\t\tstop\n\tstop\n\telse:\n\t\tstop\n").is_empty());
    }

    #[test]
    fn suppressing_the_warning_silences_the_file() {
        let source =
            "on load:\n\tsuppress unreachable code warnings\non join:\n\tstop\n\tsend \"a\"\n";
        assert!(found(source).is_empty());
    }
}
