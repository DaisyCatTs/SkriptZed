//! Minimal position types.
//!
//! Deliberately not `lsp_types`: the index is about Skript, not about the
//! protocol, and keeping the protocol out of it means the indexing logic can be
//! tested without constructing LSP values. `skript-lsp` converts at its edge.
//!
//! Positions are **byte** offsets within a line, matching what tree-sitter
//! reports as a column. `skript-lsp` re-encodes them to UTF-16 for the wire.

/// A zero-based line/character position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// A half-open span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    /// The byte columns `start..end` of a single line.
    pub fn on_line(line: u32, start: u32, end: u32) -> Self {
        Self::new(Position::new(line, start), Position::new(line, end))
    }

    pub fn contains(&self, position: Position) -> bool {
        position >= self.start && position < self.end
    }

    /// True when `position` sits inside the range or exactly at its end, which
    /// is what "the cursor is on this symbol" means when the caret is just past
    /// the last character.
    pub fn touches(&self, position: Position) -> bool {
        position >= self.start && position <= self.end
    }

    /// True when `inner` lies entirely within this range, ends included.
    pub fn encloses(&self, inner: Range) -> bool {
        self.start <= inner.start && inner.end <= self.end
    }

    /// True when `line` is one of the lines this range touches, whatever the
    /// columns.
    pub fn spans_line(&self, line: u32) -> bool {
        self.start.line <= line && line <= self.end.line
    }
}

impl From<tree_sitter::Node<'_>> for Range {
    fn from(node: tree_sitter::Node<'_>) -> Self {
        let start = node.start_position();
        let end = node.end_position();
        Range {
            start: Position::new(start.row as u32, start.column as u32),
            end: Position::new(end.row as u32, end.column as u32),
        }
    }
}

/// The byte length of a line's leading whitespace.
pub fn indent_width(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_is_half_open_and_touches_is_closed() {
        let range = Range::new(Position::new(1, 2), Position::new(1, 6));
        assert!(range.contains(Position::new(1, 2)));
        assert!(range.contains(Position::new(1, 5)));
        assert!(!range.contains(Position::new(1, 6)));
        assert!(range.touches(Position::new(1, 6)));
        assert!(!range.touches(Position::new(1, 7)));
    }

    #[test]
    fn encloses_includes_both_ends() {
        let outer = Range::new(Position::new(1, 0), Position::new(4, 3));
        assert!(outer.encloses(outer));
        assert!(outer.encloses(Range::on_line(2, 5, 9)));
        assert!(!outer.encloses(Range::on_line(4, 1, 4)));
        assert!(!outer.encloses(Range::on_line(0, 0, 1)));
    }

    #[test]
    fn spans_line_ignores_columns() {
        let range = Range::new(Position::new(1, 7), Position::new(3, 0));
        assert!(!range.spans_line(0));
        assert!(range.spans_line(1));
        assert!(range.spans_line(3));
        assert!(!range.spans_line(4));
    }

    #[test]
    fn indent_width_counts_bytes_of_leading_whitespace() {
        assert_eq!(indent_width("\t\tsend"), 2);
        assert_eq!(indent_width("    x"), 4);
        assert_eq!(indent_width("x"), 0);
        assert_eq!(indent_width("   "), 3);
    }
}
