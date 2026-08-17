use std::path::Path;

use tree_sitter::Node;

use super::tree::{LanguageSpec, analyze, node_text};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{Leaf, Span};

#[derive(Clone, Copy)]
struct Rust;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Rust)
}

impl LanguageSpec for Rust {
    fn label(self) -> &'static str {
        "Rust"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn is_function(self, kind: &str) -> bool {
        kind == "function_item"
    }

    fn is_comment(self, node: Node<'_>) -> bool {
        matches!(node.kind(), "line_comment" | "block_comment")
    }

    fn directives(self) -> &'static [&'static str] {
        &["SAFETY:"]
    }

    fn leaf_stop_prefixes(self) -> &'static [&'static str] {
        &["//!"]
    }

    fn leaf(self, node: Node<'_>, source: &str, _function_depth: usize) -> Option<Leaf> {
        if !matches!(node.kind(), "const_item" | "static_item") {
            return None;
        }
        let mut cursor = node.walk();
        let public = node
            .children(&mut cursor)
            .any(|child| child.kind() == "visibility_modifier");
        Some(Leaf {
            span: Span::from_node(node),
            name: node
                .child_by_field_name("name")
                .map_or("<unknown>", |name| node_text(name, source))
                .to_owned(),
            public,
        })
    }
}
