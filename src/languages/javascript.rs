use std::path::Path;

use tree_sitter::Node;

use super::tree::{LanguageSpec, analyze, first_descendant_with_kind, node_text};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{Leaf, Span};

pub(crate) const DIRECTIVES: &[&str] = &["eslint", "@ts-", "istanbul ignore", "c8 ignore"];
pub(crate) const LEAF_STOP_PREFIXES: &[&str] =
    &["// eslint", "/* eslint", "// @ts-", "/* istanbul", "/* c8"];

#[derive(Clone, Copy)]
struct JavaScript;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, JavaScript)
}

impl LanguageSpec for JavaScript {
    fn label(self) -> &'static str {
        "JavaScript"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_javascript::LANGUAGE.into()
    }

    fn is_function(self, kind: &str) -> bool {
        is_function_kind(kind)
    }

    fn is_comment(self, node: Node<'_>) -> bool {
        node.kind() == "comment"
    }

    fn directives(self) -> &'static [&'static str] {
        DIRECTIVES
    }

    fn leaf_stop_prefixes(self) -> &'static [&'static str] {
        LEAF_STOP_PREFIXES
    }

    fn leaf(self, node: Node<'_>, source: &str, _function_depth: usize) -> Option<Leaf> {
        leaf_from_node(node, source)
    }
}

pub(crate) fn is_function_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration" | "function_expression" | "arrow_function" | "method_definition"
    )
}

pub(crate) fn leaf_from_node(node: Node<'_>, source: &str) -> Option<Leaf> {
    if node.kind() != "lexical_declaration"
        || !node_text(node, source).trim_start().starts_with("const ")
    {
        return None;
    }
    let declarator = first_descendant_with_kind(node, "variable_declarator")?;
    let name = declarator
        .child_by_field_name("name")
        .map_or("<destructured>", |name| node_text(name, source));
    Some(Leaf {
        span: Span::from_node(node),
        name: name.to_owned(),
        public: false,
    })
}
