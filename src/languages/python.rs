use std::path::Path;

use tree_sitter::Node;

use super::tree::{LanguageSpec, analyze, node_text};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{Leaf, Span};

#[derive(Clone, Copy)]
struct Python;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Python)
}

impl LanguageSpec for Python {
    fn label(self) -> &'static str {
        "Python"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn is_function(self, kind: &str) -> bool {
        kind == "function_definition"
    }

    fn is_comment(self, node: Node<'_>) -> bool {
        node.kind() == "comment" || is_function_docstring(node)
    }

    fn directives(self) -> &'static [&'static str] {
        &["noqa", "type: ignore", "fmt:", "pragma:", "nosec"]
    }

    fn leaf_stop_prefixes(self) -> &'static [&'static str] {
        &[
            "#!",
            "# -*-",
            "# coding",
            "# noqa",
            "# type:",
            "# fmt:",
            "# pragma:",
            "# nosec",
        ]
    }

    fn leaf(self, node: Node<'_>, source: &str, function_depth: usize) -> Option<Leaf> {
        if node.kind() != "assignment" || function_depth != 0 {
            return None;
        }
        let name = node
            .child_by_field_name("left")
            .filter(|left| left.kind() == "identifier")
            .map(|left| node_text(left, source))?;
        let uppercase = name.bytes().any(|byte| byte.is_ascii_uppercase())
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        uppercase.then(|| Leaf {
            span: Span::from_node(node),
            name: name.to_owned(),
            public: false,
        })
    }
}

fn is_function_docstring(node: Node<'_>) -> bool {
    if node.kind() != "string" {
        return false;
    }
    let Some(statement) = node
        .parent()
        .filter(|parent| parent.kind() == "expression_statement")
    else {
        return false;
    };
    let Some(body) = statement.parent().filter(|parent| parent.kind() == "block") else {
        return false;
    };
    if body
        .parent()
        .is_none_or(|parent| parent.kind() != "function_definition")
    {
        return false;
    }
    body.named_child(0)
        .is_some_and(|first_statement| first_statement.id() == statement.id())
}
