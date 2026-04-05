//! Tree-sitter grammar for the flutmax DSL.

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_flutmax() -> *const ();
}

/// Returns the Tree-sitter language definition for flutmax.
///
/// # Example
///
/// ```rust
/// let mut parser = tree_sitter::Parser::new();
/// parser
///     .set_language(&tree_sitter_flutmax::LANGUAGE.into())
///     .expect("Failed to load flutmax grammar");
///
/// let source = r#"wire osc = cycle~(440);"#;
/// let tree = parser.parse(source, None).unwrap();
/// let root = tree.root_node();
///
/// assert_eq!(root.kind(), "source_file");
/// assert_eq!(root.child_count(), 1);
/// ```
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_flutmax) };
