use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    ANONYMOUS_FUNCTION_NAME, AttachmentIndex, AttachmentSyntax, CallableSubtrees, LanguageSpec,
    OwnerCandidate, OwnerLocation, analyze_with_policy, canonical_syntax, direct_named_child,
    document, has_direct_child, node_text,
};
use crate::config::PolicyConfig;
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentAttachmentScope, CommentClassification, ParsedFile, Span};

#[derive(Clone, Copy)]
struct Kotlin;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
    policy: &PolicyConfig,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze_with_policy(path, source, selection, Kotlin, policy)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, Kotlin)
}

impl LanguageSpec for Kotlin {
    type Context = AttachmentIndex;

    fn label(self) -> &'static str {
        "Kotlin"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_kotlin_ng::LANGUAGE.into()
    }

    fn build_context(self, root: Node<'_>, source: &str) -> Result<Self::Context, AnalysisError> {
        Ok(AttachmentIndex::with_syntax(
            root,
            source,
            AttachmentSyntax::default().with_documentation_comments(is_kotlin_documentation),
        ))
    }

    fn callable_kind(self) -> Option<fn(&str) -> bool> {
        Some(is_kotlin_callable)
    }

    fn owner(
        self,
        node: Node<'_>,
        _location: OwnerLocation<'_>,
        source: &str,
        _context: &Self::Context,
        _function_depth: usize,
        callable_subtrees: &CallableSubtrees,
    ) -> Result<Option<OwnerCandidate>, AnalysisError> {
        Ok(owner_from_node(node, source, callable_subtrees))
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        _source: &str,
        context: &Self::Context,
    ) -> Option<CommentClassification> {
        if !matches!(node.kind(), "line_comment" | "block_comment") {
            return None;
        }
        Some(
            if context.is_leading_documentation(node, is_documentable_declaration) {
                CommentClassification::documentation(CommentAttachmentScope::Inferred)
            } else {
                CommentClassification::narrative()
            },
        )
    }
}

fn is_kotlin_documentation(comment: &str) -> bool {
    let comment = comment.trim();
    comment.starts_with("/**") && comment != "/**/"
}

fn is_documentable_declaration(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "object_declaration"
            | "companion_object"
            | "enum_entry"
            | "function_declaration"
            | "property_declaration"
            | "secondary_constructor"
            | "type_alias"
    )
}

fn owner_from_node(
    node: Node<'_>,
    source: &str,
    callable_subtrees: &CallableSubtrees,
) -> Option<OwnerCandidate> {
    if let Some(segment) = type_segment(node, source) {
        return Some(OwnerCandidate::type_owner(
            Span::from_node(node),
            type_name(node, source),
            vec![segment],
        ));
    }
    if let Some((name, roots)) = callable_binding(node, source, callable_subtrees) {
        return Some(
            OwnerCandidate::leaf(Span::from_node(node), name).suppressing_callable_frontiers(roots),
        );
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
            "function_declaration" => node
                .child_by_field_name("name")
                .map_or(ANONYMOUS_FUNCTION_NAME, |name| node_text(name, source))
                .to_owned(),
            "anonymous_function" | "lambda_literal" => ANONYMOUS_FUNCTION_NAME.to_owned(),
            _ => return None,
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

fn callable_binding(
    node: Node<'_>,
    source: &str,
    callable_subtrees: &CallableSubtrees,
) -> Option<(String, Vec<usize>)> {
    if let Some(name) = single_property_name(node)
        && body_property_name(node).is_none()
        && let Some(initializer) = property_initializer(node)
        && callable_subtrees.contains_callable(initializer)
    {
        return Some((node_text(name, source).to_owned(), vec![initializer.id()]));
    }
    let left = assignment_left(node)?;
    let right = assignment_right(node)?;
    callable_subtrees
        .contains_callable(right)
        .then(|| (node_text(left, source).trim().to_owned(), vec![right.id()]))
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
    fn unbound_nested_lambda_names_do_not_scan_descendants() {
        let depth = 64;
        let source = format!("{}1{}", "{ ".repeat(depth), " }".repeat(depth));
        crate::languages::tree::reset_first_descendant_visits();

        parse_file(Path::new("Nested.kts"), &source).expect("valid nested Kotlin lambdas");

        assert_eq!(crate::languages::tree::first_descendant_visits(), 0);
    }

    #[test]
    fn nested_bindings_register_each_callable_frontier_once() {
        let depth = 64;
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!("val c{index} = {{\n"));
        }
        source.push_str("1\n");
        source.push_str(&"}\n".repeat(depth));
        crate::languages::tree::reset_callable_frontier_counts();
        parse_file(Path::new("Nested.kt"), &source).expect("valid Kotlin");
        let (_, candidates, registrations) = crate::languages::tree::callable_frontier_counts();
        assert_eq!((candidates, registrations), (depth, depth));
    }
}
