use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{Comment, Function, Leaf, Span, function_findings, leaf_findings};

pub(crate) trait LanguageSpec: Copy {
    fn label(self) -> &'static str;
    fn grammar(self) -> Language;
    fn is_function(self, kind: &str) -> bool;
    fn is_comment(self, node: Node<'_>) -> bool;
    fn directives(self) -> &'static [&'static str];
    fn leaf_stop_prefixes(self) -> &'static [&'static str];
    fn leaf(self, node: Node<'_>, source: &str, function_depth: usize) -> Option<Leaf>;
}

pub(crate) fn analyze<S: LanguageSpec>(
    path: &Path,
    source: &str,
    selection: &Selection,
    spec: S,
) -> Result<Vec<Finding>, AnalysisError> {
    let tree = parse(path, source, spec.label(), spec.grammar())?;
    let mut functions = Vec::new();
    let mut comments = Vec::new();
    let mut leaves = Vec::new();
    collect_nodes(
        tree.root_node(),
        source,
        spec,
        0,
        &mut functions,
        &mut comments,
        &mut leaves,
    );
    let mut findings = function_findings(
        path,
        source,
        selection,
        &functions,
        &comments,
        spec.directives(),
    );
    findings.extend(leaf_findings(
        path,
        source,
        selection,
        &leaves,
        &comments,
        spec.label(),
        spec.leaf_stop_prefixes(),
    ));
    Ok(findings)
}

pub(crate) fn parse(
    path: &Path,
    source: &str,
    label: &'static str,
    grammar: Language,
) -> Result<Tree, AnalysisError> {
    let mut parser = Parser::new();
    parser
        .set_language(&grammar)
        .map_err(|error| AnalysisError::ParserInit {
            language: label,
            detail: error.to_string(),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AnalysisError::Parse {
            path: path.display().to_string(),
            language: label,
        })?;
    if tree.root_node().has_error() {
        return Err(AnalysisError::Parse {
            path: path.display().to_string(),
            language: label,
        });
    }
    Ok(tree)
}

fn collect_nodes<S: LanguageSpec>(
    node: Node<'_>,
    source: &str,
    spec: S,
    function_depth: usize,
    functions: &mut Vec<Function>,
    comments: &mut Vec<Comment>,
    leaves: &mut Vec<Leaf>,
) {
    let is_function = spec.is_function(node.kind());
    if is_function {
        functions.push(Function {
            span: Span::from_node(node),
            name: function_name(node, source),
        });
    }
    if spec.is_comment(node) {
        comments.push(Comment {
            span: Span::from_node(node),
            text: node_text(node, source).to_owned(),
        });
    }
    if let Some(leaf) = spec.leaf(node, source, function_depth) {
        leaves.push(leaf);
    }

    let child_function_depth = function_depth + usize::from(is_function);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_nodes(
            child,
            source,
            spec,
            child_function_depth,
            functions,
            comments,
            leaves,
        );
    }
}

fn function_name(node: Node<'_>, source: &str) -> String {
    node.child_by_field_name("name")
        .or_else(|| {
            node.parent()
                .and_then(|parent| parent.child_by_field_name("name"))
        })
        .map_or("<anonymous>", |name| node_text(name, source))
        .to_owned()
}

pub(crate) fn first_descendant_with_kind<'tree>(
    node: Node<'tree>,
    kind: &str,
) -> Option<Node<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find_map(|child| first_descendant_with_kind(child, kind))
}

pub(crate) fn node_text<'source>(node: Node<'_>, source: &'source str) -> &'source str {
    source.get(node.byte_range()).unwrap_or_default()
}
