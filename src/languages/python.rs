use std::path::Path;

use tree_sitter::Node;

use super::tree::{LanguageSpec, analyze, node_text};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, Leaf, Span};

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

    fn classify_comment(self, node: Node<'_>, source: &str) -> Option<CommentKind> {
        if is_function_docstring(node) {
            return Some(CommentKind::Narrative);
        }
        (node.kind() == "comment").then(|| {
            if is_tool_directive(node_text(node, source)) {
                CommentKind::ToolDirective
            } else {
                CommentKind::Narrative
            }
        })
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
        })
    }
}

fn is_tool_directive(comment: &str) -> bool {
    let body = comment
        .trim_start()
        .strip_prefix('#')
        .unwrap_or(comment)
        .trim();
    body == "noqa"
        || body.starts_with("noqa:")
        || body == "type: ignore"
        || (body.starts_with("type: ignore[") && body.ends_with(']'))
        || matches!(body, "fmt: off" | "fmt: on" | "fmt: skip")
        || body == "pragma: no cover"
        || body == "nosec"
        || body.starts_with("nosec ")
        || body.starts_with("nosec:")
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
