use std::path::Path;

use tree_sitter::Node;

use super::tree::{LanguageSpec, analyze, first_descendant_with_kind, node_text};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, Leaf, Span};

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

    fn classify_comment(self, node: Node<'_>, source: &str) -> Option<CommentKind> {
        classify_comment(node, source)
    }

    fn leaf(self, node: Node<'_>, source: &str, _function_depth: usize) -> Option<Leaf> {
        leaf_from_node(node, source)
    }
}

pub(crate) fn classify_comment(node: Node<'_>, source: &str) -> Option<CommentKind> {
    if node.kind() != "comment" {
        return None;
    }
    let kind = if node.start_position().row == node.end_position().row
        && is_tool_directive(node_text(node, source))
    {
        CommentKind::ToolDirective
    } else {
        CommentKind::Narrative
    };
    Some(kind)
}

fn is_tool_directive(comment: &str) -> bool {
    let body = comment
        .trim()
        .strip_prefix("//")
        .or_else(|| comment.trim().strip_prefix("/*"))
        .unwrap_or(comment)
        .trim_end_matches("*/")
        .trim();
    [
        "eslint-disable",
        "eslint-enable",
        "eslint-disable-next-line",
        "eslint-disable-line",
        "eslint-env",
        "@ts-check",
        "@ts-nocheck",
        "@ts-ignore",
        "@ts-expect-error",
        "istanbul ignore",
        "c8 ignore",
    ]
    .iter()
    .any(|directive| {
        body == *directive
            || body
                .strip_prefix(directive)
                .is_some_and(|suffix| suffix.starts_with([' ', ':']))
    })
}

pub(crate) fn is_function_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "function_expression"
            | "generator_function_declaration"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
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
    })
}
