//! Rust bindings for the Skript tree-sitter grammar.
//!
//! ```
//! let mut parser = tree_sitter::Parser::new();
//! parser
//!     .set_language(&tree_sitter_skript::LANGUAGE.into())
//!     .expect("loading the Skript grammar");
//! let tree = parser.parse("on join:\n\tsend \"hi\"\n", None).unwrap();
//! assert!(!tree.root_node().has_error());
//! ```

use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_skript() -> *const ();
}

/// The tree-sitter [`Language`][tree_sitter::Language] for Skript.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_skript) };

/// The grammar's node types, as JSON.
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");

#[cfg(test)]
mod tests {
    #[test]
    fn the_grammar_loads_and_parses() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("loading the Skript grammar");

        let tree = parser
            .parse("on join:\n\tsend \"hi\" to player\n", None)
            .unwrap();
        assert!(!tree.root_node().has_error());
        assert_eq!(tree.root_node().kind(), "source_file");
    }

    #[test]
    fn the_external_scanner_produces_nested_blocks() {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&super::LANGUAGE.into()).unwrap();

        let source = "on join:\n\tif {_x} is set:\n\t\tsend \"deep\"\n";
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error());
        // event -> block -> section -> block
        assert_eq!(tree.root_node().child(0).unwrap().kind(), "event");
    }
}
