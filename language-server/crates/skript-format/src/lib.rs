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
//! * separates a multi-line structure from the structure below it by one
//!   blank line when nothing does yet;
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
    /// Separate a structure from the multi-line structure above it by one
    /// blank line when nothing separates them.
    ///
    /// Only ever adds a line, and only where there was none: deliberate extra
    /// spacing is `max_blank_lines`'s business, and a comment directly above a
    /// structure stays attached to it. Single-line structures such as a run of
    /// `using …` lines are left packed together.
    pub blank_line_between_structures: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            hard_tabs: true,
            tab_size: 4,
            max_blank_lines: 1,
            blank_line_between_structures: true,
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

/// What one input line becomes.
struct Rendered {
    /// `None` when the line is dropped: a blank line beyond
    /// `max_blank_lines`, or one at the end of the file.
    text: Option<String>,
    /// A blank line is inserted above it.
    blank_before: bool,
}

/// Formats `document`, returning `None` when there is nothing to change or the
/// file cannot be formatted safely.
pub fn format(document: &Document, options: Options) -> Option<String> {
    let rendered = render(document, options)?;
    let mut out = String::with_capacity(document.text().len());
    join(&rendered, &mut out);
    (out != document.text()).then_some(out)
}

/// A replacement for whole lines of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineEdit {
    /// The first line replaced.
    pub start_line: usize,
    /// The last line replaced, inclusive.
    pub end_line: usize,
    /// The new text for those lines, each ending in a newline.
    pub text: String,
}

/// Formats the lines `first..=last`, widened to cover every top-level
/// structure they touch.
///
/// Skript infers one indent unit per top-level structure. Re-indenting only
/// the selected half of one with tabs, while the other half keeps its spaces,
/// would turn a working structure into one Skript rejects; widening to the
/// whole structure is what keeps it consistent.
///
/// `None` when nothing in that span changes or the file cannot be formatted.
pub fn format_lines(
    document: &Document,
    options: Options,
    first: usize,
    last: usize,
) -> Option<LineEdit> {
    let rendered = render(document, options)?;
    let line_count = rendered.len();
    let mut start = first.min(last);
    if start >= line_count {
        return None;
    }
    let mut end = first.max(last).min(line_count - 1);
    for (row_start, row_end) in structures(document) {
        if row_start <= end && row_end >= start {
            start = start.min(row_start);
            end = end.max(row_end);
        }
    }
    let end = end.min(line_count - 1);

    let mut text = String::new();
    join(&rendered[start..=end], &mut text);
    let original: String = document
        .text()
        .lines()
        .skip(start)
        .take(end - start + 1)
        .flat_map(|line| [line, "\n"])
        .collect();
    (text != original).then_some(LineEdit {
        start_line: start,
        end_line: end,
        text,
    })
}

fn join(rendered: &[Rendered], out: &mut String) {
    for line in rendered {
        if line.blank_before {
            out.push('\n');
        }
        if let Some(text) = &line.text {
            out.push_str(text);
            out.push('\n');
        }
    }
}

/// Every line's formatted form, or `None` when the file cannot be formatted
/// safely.
fn render(document: &Document, options: Options) -> Option<Vec<Rendered>> {
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
    let separate = if options.blank_line_between_structures {
        needs_separation(document, line_count)
    } else {
        vec![false; line_count]
    };

    let last_code = text
        .lines()
        .enumerate()
        .filter(|(_, raw)| !raw.trim().is_empty())
        .map(|(number, _)| number)
        .last();
    let mut out = Vec::with_capacity(line_count);
    let mut blank_run = 0usize;

    for (number, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            blank_run += 1;
            // Exactly one trailing newline: blank lines at the end go.
            let before_end = last_code.is_some_and(|last| number < last);
            let kept = blank_run <= options.max_blank_lines && before_end;
            out.push(Rendered {
                text: kept.then(String::new),
                blank_before: false,
            });
            continue;
        }
        blank_run = 0;

        let line = if verbatim[number] {
            // Inside a block comment: the text is prose, and its leading
            // whitespace may well be deliberate.
            raw.trim_end().to_string()
        } else {
            format!("{}{}", options.indent(depths[number]), raw.trim())
        };
        out.push(Rendered {
            text: Some(line),
            blank_before: separate[number],
        });
    }
    Some(out)
}

/// The rows each top-level structure spans, comments excluded.
fn structures(document: &Document) -> Vec<(usize, usize)> {
    let root = document.tree().root_node();
    let mut cursor = root.walk();
    let found = root
        .named_children(&mut cursor)
        .filter(|node| !matches!(node.kind(), "comment" | "block_comment"))
        .map(|node| {
            let start = node.start_position().row;
            let end = node.end_position();
            // A node that ends at column 0 ended with the line before.
            let end_row = if end.column == 0 && end.row > start {
                end.row - 1
            } else {
                end.row
            };
            (start, end_row)
        })
        .collect();
    found
}

/// For each line, whether it opens a structure that sits directly under a
/// multi-line structure with nothing between them.
fn needs_separation(document: &Document, line_count: usize) -> Vec<bool> {
    let lines: Vec<&str> = document.text().lines().collect();
    let mut out = vec![false; line_count];
    for pair in structures(document).windows(2) {
        let ((above_start, above_end), (start, _)) = (pair[0], pair[1]);
        let Some(previous) = start.checked_sub(1).and_then(|row| lines.get(row)) else {
            continue;
        };
        // A comment directly above a structure belongs to it, and a blank
        // line is already a separation.
        let touching = start == above_end + 1
            && !previous.trim().is_empty()
            && !previous.trim_start().starts_with('#');
        if touching && above_end > above_start {
            if let Some(slot) = out.get_mut(start) {
                *slot = true;
            }
        }
    }
    out
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
        "block" | "entry_body" | "assignment_body" | "import_body" | "class_body"
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

    /// A ClassySK class body is its own node kind. Before it was listed as
    /// opening a level, formatting flattened every field and method to column
    /// 0 — which ClassySK then rejects, so the format action broke the script.
    #[test]
    fn keeps_a_classysk_class_body_indented() {
        let out = formatted(concat!(
            "class Wallet:\n",
            "  private balance: number = 0\n",
            "  public deposit(amount: number):\n",
            "      add {_amount} to self::balance\n",
        ));
        assert_eq!(
            out,
            concat!(
                "class Wallet:\n",
                "\tprivate balance: number = 0\n",
                "\tpublic deposit(amount: number):\n",
                "\t\tadd {_amount} to self::balance\n",
            )
        );
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

    #[test]
    fn separates_structures_that_touch() {
        let out = formatted("on join:\n\tstop\non quit:\n\tstop\nfunction f():\n\tstop\n");
        assert_eq!(
            out,
            "on join:\n\tstop\n\non quit:\n\tstop\n\nfunction f():\n\tstop\n"
        );
    }

    #[test]
    fn the_separation_rule_only_ever_adds_one_line() {
        // Two blank lines are deliberate; `max_blank_lines` decides, not this.
        let options = Options {
            max_blank_lines: 2,
            ..Options::default()
        };
        let source = "on join:\n\tstop\n\n\non quit:\n\tstop\n";
        assert!(format(&Document::new("file:///t.sk", source), options).is_none());
    }

    #[test]
    fn a_comment_above_a_structure_stays_attached() {
        let source = "on join:\n\tstop\n# Says goodbye.\non quit:\n\tstop\n";
        assert_eq!(formatted(source), source);
        let block = "on join:\n\tstop\n###\nprose\n###\non quit:\n\tstop\n";
        assert_eq!(formatted(block), block);
    }

    /// A run of `using` lines is one block, not five structures to space out.
    #[test]
    fn single_line_structures_stay_packed() {
        let source = "using for loops\nusing error catching\non join:\n\tstop\n";
        assert_eq!(formatted(source), source);
    }

    #[test]
    fn the_separation_rule_can_be_turned_off() {
        let options = Options {
            blank_line_between_structures: false,
            ..Options::default()
        };
        let source = "on join:\n\tstop\non quit:\n\tstop\n";
        assert!(format(&Document::new("file:///t.sk", source), options).is_none());
    }

    #[test]
    fn separation_is_idempotent() {
        let once = formatted("options:\n\ta: 1\non join:\n\tstop\non quit:\n\tstop\n");
        assert_eq!(formatted(&once), once);
    }

    const TWO_TRIGGERS: &str = "on join:\n    send \"a\"\n    send \"b\"\n\non quit:\n  stop   \n";

    #[test]
    fn a_range_is_widened_to_its_whole_structure() {
        let document = Document::new("file:///t.sk", TWO_TRIGGERS);
        // Only line 2 was selected; reindenting it alone would leave a tab
        // beside spaces inside one structure, which Skript rejects.
        let edit = format_lines(&document, Options::default(), 2, 2).expect("an edit");
        assert_eq!((edit.start_line, edit.end_line), (0, 2));
        assert_eq!(edit.text, "on join:\n\tsend \"a\"\n\tsend \"b\"\n");
    }

    #[test]
    fn a_range_leaves_other_structures_alone() {
        let document = Document::new("file:///t.sk", TWO_TRIGGERS);
        let edit = format_lines(&document, Options::default(), 5, 5).expect("an edit");
        assert_eq!((edit.start_line, edit.end_line), (4, 5));
        assert_eq!(edit.text, "on quit:\n\tstop\n");
    }

    #[test]
    fn a_range_that_is_already_formatted_changes_nothing() {
        let document = Document::new("file:///t.sk", "on join:\n\tstop\n\non quit:\n  stop\n");
        assert!(format_lines(&document, Options::default(), 0, 1).is_none());
        assert!(format_lines(&document, Options::default(), 40, 50).is_none());
    }

    #[test]
    fn a_range_refuses_a_file_that_does_not_parse() {
        let document = Document::new("file:///t.sk", "on join:\n\tsend \"unterminated\n");
        if document.has_errors() {
            assert!(format_lines(&document, Options::default(), 0, 1).is_none());
        }
    }
}
