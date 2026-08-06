//! A formatter for Skript.
//!
//! Skript is whitespace-delimited and otherwise free-form English, so there is
//! no sensible way to re-flow a line — an "improvement" to `send "hello" to
//! player` would be a guess about an addon's syntax. This formatter therefore
//! does only what is unambiguously safe and unambiguously useful:
//!
//! * re-indents every line to its **structural** depth, taken from the parse
//!   tree rather than from the whitespace already there, so a file indented
//!   inconsistently comes out correct rather than consistently wrong;
//! * trims trailing whitespace;
//! * collapses runs of blank lines;
//! * ends the file with exactly one newline.
//!
//! What it never does:
//!
//! * touch anything *within* a line — strings, comments and `%…%` are returned
//!   byte-for-byte;
//! * re-indent the interior of a `###` block comment, whose contents are prose;
//! * format a file that does not parse. Skript's indentation *is* its syntax,
//!   so re-indenting a file we have misunderstood could silently move code into
//!   the wrong block. A broken file is left exactly as it is.

use skript_index::Document;
use tree_sitter::Node;

/// Formatting preferences.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Indent with a tab rather than spaces. Skript's own scripts, its
    /// `config.sk` and every community style guide use tabs.
    pub hard_tabs: bool,
    /// Spaces per level when `hard_tabs` is false.
    pub tab_size: usize,
    /// The most consecutive blank lines to keep.
    pub max_blank_lines: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            hard_tabs: true,
            tab_size: 4,
            max_blank_lines: 1,
        }
    }
}

impl Options {
    fn indent(&self, depth: usize) -> String {
        if self.hard_tabs {
            "\t".repeat(depth)
        } else {
            " ".repeat(depth * self.tab_size)
        }
    }
}

/// Formats `document`, returning `None` when there is nothing to change or the
/// file cannot be formatted safely.
pub fn format(document: &Document, options: Options) -> Option<String> {
    // Indentation carries meaning in Skript. If the parse is broken we do not
    // actually know which block a line belongs to, and guessing could move code
    // between branches of an `if`.
    if document.has_errors() {
        return None;
    }

    let text = document.text();
    let line_count = text.lines().count();
    if line_count == 0 {
        return None;
    }

    let mut depths = vec![0usize; line_count];
    let mut verbatim = vec![false; line_count];
    measure(document.tree().root_node(), 0, &mut depths, &mut verbatim);

    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;

    for (number, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            blank_run += 1;
            if blank_run <= options.max_blank_lines {
                out.push('\n');
            }
            continue;
        }
        blank_run = 0;

        if verbatim[number] {
            // Inside a block comment: the text is prose, and its leading
            // whitespace may well be deliberate.
            out.push_str(raw.trim_end());
        } else {
            out.push_str(&options.indent(depths[number]));
            out.push_str(raw.trim());
        }
        out.push('\n');
    }

    // Exactly one trailing newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }

    (out != text).then_some(out)
}

/// Records, for every line, how deeply nested it is and whether it is inside a
/// block comment.
fn measure(node: Node<'_>, depth: usize, depths: &mut [usize], verbatim: &mut [bool]) {
    if node.kind() == "block_comment" {
        // The delimiter line keeps the depth it was found at; the interior is
        // left untouched.
        let start = node.start_position().row;
        let end = node.end_position().row;
        if let Some(slot) = depths.get_mut(start) {
            *slot = depth;
        }
        for row in (start + 1)..=end {
            if let Some(slot) = verbatim.get_mut(row) {
                *slot = true;
            }
        }
        return;
    }

    // These are the nodes that introduce a level of indentation.
    let opens_a_level = matches!(
        node.kind(),
        "block" | "entry_body" | "assignment_body" | "import_body"
    );
    let inner = if opens_a_level { depth + 1 } else { depth };

    if opens_a_level {
        // A body node *starts* on its header's row, because INDENT is a
        // zero-width token emitted right after the section colon. Indenting
        // from there would push `on join:` in by a level, so the first row that
        // actually belongs to the body is its first child's.
        let first_row = node
            .named_child(0)
            .map(|child| child.start_position().row)
            .unwrap_or(node.start_position().row + 1);

        for row in first_row..=node.end_position().row {
            if let Some(slot) = depths.get_mut(row) {
                *slot = inner;
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        measure(child, inner, depths, verbatim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatted(source: &str) -> String {
        let document = Document::new("file:///t.sk", source);
        format(&document, Options::default()).unwrap_or_else(|| source.to_string())
    }

    #[test]
    fn normalises_spaces_to_tabs() {
        let out = formatted("on join:\n    send \"hi\"\n");
        assert_eq!(out, "on join:\n\tsend \"hi\"\n");
    }

    #[test]
    fn fixes_an_inconsistent_indent_from_the_parse_tree() {
        // The body is indented six spaces at one level; structurally it is one
        // level deep, so it comes out as one tab.
        let out = formatted("on join:\n      send \"hi\"\n");
        assert_eq!(out, "on join:\n\tsend \"hi\"\n");
    }

    #[test]
    fn indents_nested_blocks_by_depth() {
        let out = formatted("on join:\n\tif {_x} is set:\n\t\t\t\tsend \"deep\"\n");
        assert_eq!(out, "on join:\n\tif {_x} is set:\n\t\tsend \"deep\"\n");
    }

    #[test]
    fn indents_command_entries_and_their_trigger() {
        let out = formatted("command /a:\n  permission: x\n  trigger:\n     stop\n");
        assert_eq!(out, "command /a:\n\tpermission: x\n\ttrigger:\n\t\tstop\n");
    }

    #[test]
    fn can_use_spaces_instead() {
        let options = Options {
            hard_tabs: false,
            tab_size: 2,
            ..Options::default()
        };
        let document = Document::new("file:///t.sk", "on join:\n\tsend \"hi\"\n");
        assert_eq!(
            format(&document, options).unwrap(),
            "on join:\n  send \"hi\"\n"
        );
    }

    #[test]
    fn trims_trailing_whitespace() {
        let out = formatted("on join:   \n\tsend \"hi\"\t\n");
        assert_eq!(out, "on join:\n\tsend \"hi\"\n");
    }

    #[test]
    fn collapses_runs_of_blank_lines() {
        let out = formatted("on join:\n\tstop\n\n\n\n\non quit:\n\tstop\n");
        assert_eq!(out, "on join:\n\tstop\n\non quit:\n\tstop\n");
    }

    #[test]
    fn leaves_string_and_comment_contents_alone() {
        let source = "on join:\n\tsend \"  spaced   out  #1  \" # a  comment\n";
        assert_eq!(formatted(source), source);
    }

    #[test]
    fn does_not_reindent_inside_a_block_comment() {
        let source = "###\n   deliberately indented prose\n\tand a tab\n###\non join:\n\tstop\n";
        assert_eq!(formatted(source), source);
    }

    #[test]
    fn refuses_to_format_a_file_that_does_not_parse() {
        // Re-indenting a file we have misparsed could move code between the
        // branches of an `if`, so nothing is safer than doing nothing.
        let document = Document::new("file:///t.sk", "on join:\n\tsend \"unterminated\n");
        if document.has_errors() {
            assert!(format(&document, Options::default()).is_none());
        }
    }

    #[test]
    fn is_idempotent() {
        let messy = "command /a:\n  permission: x\n\n\n\n  trigger:\n     if {_x} is set:\n            stop   \n";
        let once = formatted(messy);
        let twice = formatted(&once);
        assert_eq!(once, twice, "formatting must reach a fixed point");
    }

    #[test]
    fn returns_nothing_when_the_file_is_already_formatted() {
        let document = Document::new("file:///t.sk", "on join:\n\tstop\n");
        assert!(format(&document, Options::default()).is_none());
    }

    #[test]
    fn preserves_every_non_whitespace_byte() {
        let source =
            "command /a:\n  permission: skript.a\n  trigger:\n     send \"x %player% y\"\n";
        let out = formatted(source);
        let strip = |text: &str| {
            text.chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
        };
        assert_eq!(strip(&out), strip(source));
    }
}
