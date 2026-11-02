//! Which sections enclose a line, read from indentation.
//!
//! The checks that need this only ever ask about files that parse cleanly,
//! where indentation and the tree agree. Reading it from the text keeps this
//! crate off a direct `tree-sitter` dependency.

use std::collections::HashSet;

use crate::lines;

/// The file split into lines, with the `###` interiors marked.
pub(super) struct Lines<'t> {
    pub lines: Vec<&'t str>,
    prose: HashSet<usize>,
}

impl<'t> Lines<'t> {
    pub fn new(text: &'t str) -> Self {
        Self {
            lines: text.lines().collect(),
            prose: lines::block_comment_lines(text),
        }
    }

    /// Line `number` without its comment, or `None` for block-comment prose
    /// and for lines with no code at all.
    pub fn code(&self, number: usize) -> Option<&'t str> {
        if self.prose.contains(&number) {
            return None;
        }
        let line = self.lines.get(number)?;
        if lines::is_block_comment_delimiter(line) {
            return None;
        }
        let code = lines::code_part(line);
        (!code.trim().is_empty()).then_some(code)
    }

    /// The headers of the sections around `number`, innermost first, each
    /// trimmed and without its colon. The last one is the top-level
    /// structure's.
    pub fn enclosing_headers(&self, number: usize) -> Vec<&'t str> {
        let Some(code) = self.code(number) else {
            return Vec::new();
        };
        let mut depth = skript_index::indent_width(code);
        let mut out = Vec::new();
        for above in (0..number).rev() {
            if depth == 0 {
                break;
            }
            let Some(code) = self.code(above) else {
                continue;
            };
            let indent = skript_index::indent_width(code);
            if indent < depth {
                out.push(code.trim().trim_end_matches(':').trim_end());
                depth = indent;
            }
        }
        out
    }
}

/// Whether a section header is core control flow, which runs its body in the
/// same event as the line above it.
///
/// Anything else may be an addon section that runs its body later, in another
/// event, or with values of its own: a GUI slot's click handler, a
/// skript-reflect section with arguments.
pub(super) fn is_control_flow(header: &str) -> bool {
    let lower = header.to_ascii_lowercase();
    lower == "else"
        || ["if ", "else if ", "loop ", "while ", "do while ", "for "]
            .iter()
            .any(|keyword| lower.starts_with(keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_come_innermost_first_and_skip_comments() {
        let text = "on join:\n\tif x:\n\t# note\n\n\t\tloop 3 times:\n\t\t\tstop # done\n";
        let lines = Lines::new(text);
        assert_eq!(
            lines.enclosing_headers(5),
            vec!["loop 3 times", "if x", "on join"]
        );
        assert!(lines.enclosing_headers(0).is_empty());
    }

    #[test]
    fn block_comment_prose_is_not_a_header() {
        let text = "on join:\n###\nnot a header:\n###\n\tstop\n";
        assert_eq!(Lines::new(text).enclosing_headers(4), vec!["on join"]);
    }

    #[test]
    fn only_core_control_flow_counts() {
        assert!(is_control_flow("if player is op"));
        assert!(is_control_flow("else"));
        assert!(is_control_flow("Else if x"));
        assert!(is_control_flow("for each {_x} in {_list::*}"));
        assert!(!is_control_flow("format gui slot 1 of player with stone"));
        assert!(!is_control_flow("elsewhere"));
    }
}
