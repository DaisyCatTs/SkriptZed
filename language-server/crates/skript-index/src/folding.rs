//! Folding ranges.
//!
//! Zed does **not** read a `folds.scm` — it never has. Folding comes from
//! indentation, from `brackets.scm`, and from the language server's
//! `textDocument/foldingRange`, which Zed consumes complete with the
//! `collapsedText` shown on the folded line.
//!
//! That makes this module the only way to deliver the folding the brief asked
//! for — events, commands, functions, loops, if/else, while, sections, options
//! and block comments — with a placeholder that says what was folded away.

use tree_sitter::{Node, Tree};

/// A foldable region. Lines are zero-based; the end line is inclusive, matching
/// LSP's `FoldingRange`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: FoldKind,
    /// Shown in place of the folded text.
    pub collapsed_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Region,
    Comment,
}

/// Collects every foldable region in the document.
pub fn ranges(tree: &Tree, source: &str) -> Vec<Fold> {
    let mut out = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        for child in node.children(&mut cursor) {
            stack.push(child);
        }

        match node.kind() {
            // Block comments fold as comments so the editor can collapse them
            // with the "fold all comments" action.
            "block_comment" => push(&mut out, node, FoldKind::Comment, "###  …  ###".into()),

            // Every structure and nested section folds on its body, so the
            // header line stays visible.
            "event" | "command" | "function" | "options" | "variables" | "aliases" | "import"
            | "section" | "entry_section" => {
                let Some(body) = node.child_by_field_name("body") else {
                    continue;
                };
                let placeholder = summarise(node, body, source);
                // Fold from the header line, not the body's first line, so the
                // fold arrow sits on `on join:` where a reader expects it.
                let start = node.start_position().row as u32;
                let end = body.end_position().row as u32;
                if end > start {
                    out.push(Fold {
                        start_line: start,
                        end_line: end,
                        kind: FoldKind::Region,
                        collapsed_text: placeholder,
                    });
                }
            }
            _ => {}
        }
    }

    out.sort_by_key(|fold| (fold.start_line, fold.end_line));
    out.dedup_by_key(|fold| (fold.start_line, fold.end_line));
    out
}

fn push(out: &mut Vec<Fold>, node: Node<'_>, kind: FoldKind, collapsed_text: String) {
    let start = node.start_position().row as u32;
    let end = node.end_position().row as u32;
    if end > start {
        out.push(Fold {
            start_line: start,
            end_line: end,
            kind,
            collapsed_text,
        });
    }
}

/// Builds the text shown on a folded line: how much was hidden, so folding a
/// long trigger still tells you something.
fn summarise(node: Node<'_>, body: Node<'_>, source: &str) -> String {
    let lines = body
        .end_position()
        .row
        .saturating_sub(body.start_position().row)
        + 1;
    let noun: String = match node.kind() {
        "entry_section" => entry_key(node, source).unwrap_or_else(|| "section".into()),
        kind @ ("event" | "command" | "function" | "options" | "variables" | "aliases"
        | "import") => kind.to_string(),
        _ => "section".to_string(),
    };
    format!(" … {lines} lines of {noun} ")
}

fn entry_key(node: Node<'_>, source: &str) -> Option<String> {
    let key = node.child_by_field_name("key")?;
    Some(source[key.byte_range()].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn folds(source: &str) -> Vec<Fold> {
        Document::new("file:///t.sk", source).folding_ranges()
    }

    #[test]
    fn folds_an_event_from_its_header_line() {
        let found = folds("on join:\n\tsend \"a\"\n\tsend \"b\"\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].start_line, 0);
        assert_eq!(found[0].end_line, 2);
        assert!(found[0].collapsed_text.contains("event"));
    }

    #[test]
    fn folds_nested_sections_separately() {
        let found = folds("on join:\n\tif {_x} is set:\n\t\tstop\n");
        // The event and the `if` each fold.
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].start_line, 0);
        assert_eq!(found[1].start_line, 1);
    }

    #[test]
    fn folds_command_entries() {
        let found = folds("command /home:\n\ttrigger:\n\t\tstop\n\t\tstop\n");
        assert!(found
            .iter()
            .any(|fold| fold.collapsed_text.contains("trigger")));
    }

    #[test]
    fn folds_block_comments_as_comments() {
        let found = folds("###\nhidden\nlines\n###\non join:\n\tstop\n");
        assert!(found.iter().any(|fold| fold.kind == FoldKind::Comment));
    }

    #[test]
    fn does_not_fold_a_single_line_construct() {
        // Nothing to hide, so no fold arrow.
        let found = folds("on join:\n\tstop\n");
        assert_eq!(found.len(), 1);
        let found = folds("using script reflection\n");
        assert!(found.is_empty());
    }

    #[test]
    fn folds_options_and_variables_blocks() {
        let found = folds("options:\n\ta: 1\n\tb: 2\n\nvariables:\n\t{x} = 1\n\t{y} = 2\n");
        assert!(found.iter().any(|f| f.collapsed_text.contains("options")));
        assert!(found.iter().any(|f| f.collapsed_text.contains("variables")));
    }
}
