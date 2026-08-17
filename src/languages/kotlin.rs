use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    LanguageSpec, OwnerCandidate, OwnerLocation, analyze, canonical_syntax, direct_named_child,
    document, function_name, node_text, outermost_node_ids_matching,
};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

#[derive(Clone, Copy)]
struct Kotlin;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Kotlin)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, Kotlin)
}

impl LanguageSpec for Kotlin {
    type Context = ();

    fn label(self) -> &'static str {
        "Kotlin"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_kotlin_ng::LANGUAGE.into()
    }

    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        _function_depth: usize,
    ) -> Option<OwnerCandidate> {
        owner_from_node(node, location, source)
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        _source: &str,
        _context: &Self::Context,
    ) -> Option<CommentKind> {
        matches!(node.kind(), "line_comment" | "block_comment").then_some(CommentKind::Narrative)
    }
}

fn owner_from_node(
    node: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
) -> Option<OwnerCandidate> {
    if let Some(segment) = type_segment(node, source) {
        return Some(OwnerCandidate::type_owner(
            Span::from_node(node),
            type_name(node, source),
            vec![segment],
        ));
    }
    if let Some((name, suppressed)) = callable_binding(node, source) {
        return Some(OwnerCandidate::leaf(Span::from_node(node), name).suppressing(suppressed));
    }
    if let Some(name) = body_property_name(node) {
        return Some(OwnerCandidate::function(
            Span::from_node(node),
            node_text(name, source).to_owned(),
            Vec::new(),
        ));
    }
    if matches!(
        node.kind(),
        "function_declaration"
            | "secondary_constructor"
            | "anonymous_initializer"
            | "anonymous_function"
            | "lambda_literal"
    ) {
        let name = match node.kind() {
            "secondary_constructor" => "constructor".to_owned(),
            "anonymous_initializer" => "init".to_owned(),
            _ => function_name(node, location, source),
        };
        return Some(OwnerCandidate::function(
            Span::from_node(node),
            name,
            callable_signature(node, source),
        ));
    }
    let name = single_val_name(node)?;
    Some(OwnerCandidate::leaf(
        Span::from_node(node),
        node_text(name, source).to_owned(),
    ))
}

fn type_segment(node: Node<'_>, source: &str) -> Option<String> {
    let kind = match node.kind() {
        "class_declaration" => "class",
        "object_declaration" => "object",
        "companion_object" => "companion",
        "enum_entry" => "enum-entry",
        _ => return None,
    };
    Some(format!("{kind}:{}", type_name(node, source)))
}

fn type_name(node: Node<'_>, source: &str) -> String {
    node.child_by_field_name("name")
        .or_else(|| direct_named_child(node, "identifier"))
        .map_or("Companion", |name| node_text(name, source))
        .to_owned()
}

fn callable_signature(node: Node<'_>, source: &str) -> Vec<String> {
    if !matches!(
        node.kind(),
        "function_declaration" | "secondary_constructor"
    ) {
        return Vec::new();
    }

    let mut signature = Vec::new();
    if node.kind() == "function_declaration" {
        let Some(name) = node.child_by_field_name("name") else {
            return signature;
        };
        let mut cursor = node.walk();
        let receiver = node
            .named_children(&mut cursor)
            .take_while(|child| child.id() != name.id())
            .filter(|child| !matches!(child.kind(), "modifiers" | "type_parameters"))
            .map(|child| canonical_syntax(child, source))
            .collect::<String>();
        if !receiver.is_empty() {
            signature.push(format!("receiver:{receiver}"));
        }
    }

    let Some(parameters) = direct_named_child(node, "function_value_parameters") else {
        return signature;
    };
    let mut cursor = parameters.walk();
    let mut modifiers = String::new();
    for child in parameters.named_children(&mut cursor) {
        if child.kind() == "parameter_modifiers" {
            modifiers = canonical_syntax(child, source);
            continue;
        }
        if child.kind() != "parameter" {
            continue;
        }
        if let Some(parameter_type) = parameter_type(child) {
            signature.push(format!(
                "parameter:{modifiers}{}",
                canonical_syntax(parameter_type, source)
            ));
        }
        modifiers.clear();
    }
    signature
}

fn parameter_type(parameter: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = parameter.walk();
    parameter
        .named_children(&mut cursor)
        .find(|child| child.kind() != "identifier")
}

fn has_direct_child(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| child.kind() == kind)
}

fn single_val_name(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() != "property_declaration" || !has_direct_child(node, "val") {
        return None;
    }
    single_property_name(node)
}

fn single_property_name(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() != "property_declaration" {
        return None;
    }
    let declaration = direct_named_child(node, "variable_declaration")?;
    direct_named_child(declaration, "identifier")
}

fn property_initializer(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() != "property_declaration" {
        return None;
    }
    if let Some(delegate) = direct_named_child(node, "property_delegate") {
        return Some(delegate);
    }
    let mut cursor = node.walk();
    let mut after_equals = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "=" {
            after_equals = true;
        } else if after_equals
            && child.is_named()
            && !matches!(child.kind(), "line_comment" | "block_comment")
        {
            return Some(child);
        }
    }
    None
}

fn assignment_left(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind() == "assignment")
        .then(|| node.child_by_field_name("left"))
        .flatten()
}

fn assignment_right(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind() == "assignment")
        .then(|| node.child_by_field_name("right"))
        .flatten()
}

fn body_property_name(node: Node<'_>) -> Option<Node<'_>> {
    let name = single_property_name(node)?;
    ["getter", "setter"]
        .into_iter()
        .any(|kind| direct_named_child(node, kind).is_some())
        .then_some(name)
}

fn callable_binding(node: Node<'_>, source: &str) -> Option<(String, Vec<usize>)> {
    if let Some(name) = single_property_name(node)
        && body_property_name(node).is_none()
        && let Some(initializer) = property_initializer(node)
    {
        let suppressed = outermost_node_ids_matching(initializer, is_kotlin_callable);
        if !suppressed.is_empty() {
            return Some((node_text(name, source).to_owned(), suppressed));
        }
    }
    let left = assignment_left(node)?;
    let right = assignment_right(node)?;
    let suppressed = outermost_node_ids_matching(right, is_kotlin_callable);
    (!suppressed.is_empty()).then(|| (node_text(left, source).trim().to_owned(), suppressed))
}

fn is_kotlin_callable(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "secondary_constructor"
            | "anonymous_initializer"
            | "anonymous_function"
            | "lambda_literal"
            | "getter"
            | "setter"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_bindings_scan_each_outer_callable_once() {
        let depth = 64;
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!("val c{index} = {{\n"));
        }
        source.push_str("1\n");
        source.push_str(&"}\n".repeat(depth));
        crate::languages::tree::reset_outermost_scan_visits();
        parse_file(Path::new("Nested.kt"), &source).expect("valid Kotlin");
        assert_eq!(crate::languages::tree::outermost_scan_visits(), depth);
    }
}
