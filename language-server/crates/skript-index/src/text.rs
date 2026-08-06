//! Minimal position types.
//!
//! Deliberately not `lsp_types`: the index is about Skript, not about the
//! protocol, and keeping the protocol out of it means the indexing logic can be
//! tested without constructing LSP values. `skript-lsp` converts at its edge.
//!
//! Positions are UTF-8 character offsets within a line, matching what
//! tree-sitter reports. `skript-lsp` re-encodes to UTF-16 for the wire.

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

    pub fn contains(&self, position: Position) -> bool {
        position >= self.start && position < self.end
    }

    /// True when `position` sits inside the range or exactly at its end, which
    /// is what "the cursor is on this symbol" means when the caret is just past
    /// the last character.
    pub fn touches(&self, position: Position) -> bool {
        position >= self.start && position <= self.end
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
}
