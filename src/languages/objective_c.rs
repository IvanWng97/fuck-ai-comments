use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    ANONYMOUS_FUNCTION_NAME, CallableSubtrees, LanguageSpec, OwnerCandidate, OwnerLocation,
    analyze_with_policy, direct_named_child, document, first_descendant_with_kind, node_text,
};
use super::walk::{WalkEvent, events};
use crate::config::PolicyConfig;
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

#[derive(Clone, Copy)]
struct ObjectiveC;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
    policy: &PolicyConfig,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze_with_policy(path, source, selection, ObjectiveC, policy)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, ObjectiveC)
}

impl LanguageSpec for ObjectiveC {
    type Context = ();

    fn label(self) -> &'static str {
        "Objective-C"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_objc::LANGUAGE.into()
    }

    fn callable_kind(self) -> Option<fn(&str) -> bool> {
        Some(is_objective_c_callable)
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
        source: &str,
        _context: &Self::Context,
    ) -> Option<CommentKind> {
        (node.kind() == "comment").then(|| {
            if clang_format_directive(node_text(node, source)) {
                CommentKind::ToolDirective
            } else {
                CommentKind::Narrative
            }
        })
    }
}

fn owner_from_node(
    node: Node<'_>,
    source: &str,
    callable_subtrees: &CallableSubtrees,
) -> Option<OwnerCandidate> {
    if let Some(segment) = type_segment(node, source) {
        return Some(OwnerCandidate::type_owner(
            Span::from_node(node),
            type_name(node, source)?,
            vec![segment],
        ));
    }
    if let Some((name, roots)) = callable_binding(node, source, callable_subtrees) {
        return Some(
            OwnerCandidate::leaf(Span::from_node(node), name).suppressing_callable_frontiers(roots),
        );
    }
    let name = match node.kind() {
        "method_definition" => method_identity(node, source),
        "function_definition" => declarator_name(node, source).map(str::to_owned),
        "block_literal" => Some(ANONYMOUS_FUNCTION_NAME.to_owned()),
        _ => None,
    };
    if let Some(name) = name {
        return Some(OwnerCandidate::function(
            Span::from_node(node),
            name,
            Vec::new(),
        ));
    }
    let declarator = initialized_const_declarator(node, source)?;
    Some(OwnerCandidate::leaf(
        Span::from_node(node),
        declarator_name(declarator, source)?.to_owned(),
    ))
}

fn clang_format_directive(comment: &str) -> bool {
    let comment = comment.trim_end_matches(['\r', '\n']);
    if matches!(comment, "/* clang-format off */" | "/* clang-format on */") {
        return true;
    }
    ["// clang-format off", "// clang-format on"]
        .into_iter()
        .any(|directive| {
            comment == directive
                || comment
                    .strip_prefix(directive)
                    .is_some_and(|suffix| suffix.starts_with(':'))
        })
}

fn assignment_left(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind() == "assignment_expression")
        .then(|| node.child_by_field_name("left"))
        .flatten()
}

fn assignment_right(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind() == "assignment_expression")
        .then(|| node.child_by_field_name("right"))
        .flatten()
}

fn grouped_declaration_name(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let declarators: Vec<_> = node
        .children_by_field_name("declarator", &mut cursor)
        .collect();
    let mut names: Vec<_> = declarators
        .into_iter()
        .map(|declarator| declarator_name(declarator, source))
        .collect::<Option<_>>()?;
    names.sort_unstable();
    (!names.is_empty()).then(|| names.join(","))
}

fn callable_binding(
    node: Node<'_>,
    source: &str,
    callable_subtrees: &CallableSubtrees,
) -> Option<(String, Vec<usize>)> {
    if node.kind() == "declaration" {
        let mut cursor = node.walk();
        let mut roots = Vec::new();
        for declarator in node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "init_declarator")
        {
            if let Some(value) = declarator.child_by_field_name("value")
                && callable_subtrees.contains_callable(value)
            {
                roots.push(value.id());
            }
        }
        if !roots.is_empty() {
            return Some((grouped_declaration_name(node, source)?, roots));
        }
    }
    let left = assignment_left(node)?;
    let right = assignment_right(node)?;
    callable_subtrees
        .contains_callable(right)
        .then(|| (node_text(left, source).trim().to_owned(), vec![right.id()]))
}

fn is_objective_c_callable(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition" | "method_definition" | "block_literal"
    )
}

fn method_identity(node: Node<'_>, source: &str) -> Option<String> {
    let kind = node_text(node, source)
        .trim_start()
        .bytes()
        .next()
        .filter(|byte| matches!(byte, b'+' | b'-'))? as char;
    method_selector(node, source).map(|selector| format!("{kind}{selector}"))
}

fn method_selector(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let mut identifiers = node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "identifier")
        .map(|child| node_text(child, source));
    let first = identifiers.next()?;
    let mut selector = first.to_owned();
    for identifier in identifiers {
        selector.push(':');
        selector.push_str(identifier);
    }
    if direct_named_child(node, "method_parameter").is_some() {
        selector.push(':');
    }
    Some(selector)
}

fn declarator_name<'source>(node: Node<'_>, source: &'source str) -> Option<&'source str> {
    let declarator = node.child_by_field_name("declarator").unwrap_or(node);
    first_descendant_with_kind(declarator, "identifier").map(|name| node_text(name, source))
}

fn type_name(node: Node<'_>, source: &str) -> Option<String> {
    let class_name = direct_identifier(node).map(|name| node_text(name, source))?;
    let category = node
        .child_by_field_name("category")
        .map(|name| node_text(name, source));
    Some(category.map_or_else(
        || class_name.to_owned(),
        |category| format!("{class_name}({category})"),
    ))
}

fn type_segment(node: Node<'_>, source: &str) -> Option<String> {
    let prefix = match node.kind() {
        "class_interface" => "interface",
        "class_implementation" => "implementation",
        "protocol_declaration" => "protocol",
        _ => return None,
    };
    type_name(node, source).map(|name| format!("{prefix}:{name}"))
}

fn direct_identifier(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == "identifier")
}

fn initialized_const_declarator<'tree>(node: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    if node.kind() != "declaration" {
        return None;
    }
    let declaration_is_const = direct_has_const_qualifier(node, source);
    events(node).find_map(|event| match event {
        WalkEvent::Enter(candidate)
            if candidate.kind() == "init_declarator"
                && (declaration_is_const || declarator_is_const(candidate, source)) =>
        {
            Some(candidate)
        }
        WalkEvent::Enter(_) | WalkEvent::Leave(_) => None,
    })
}

fn declarator_is_const(node: Node<'_>, source: &str) -> bool {
    let mut current = node.child_by_field_name("declarator");
    while let Some(declarator) = current {
        if direct_has_const_qualifier(declarator, source) {
            return true;
        }
        current = declarator.child_by_field_name("declarator");
    }
    false
}

fn direct_has_const_qualifier(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == "type_qualifier" && node_text(child, source).trim() == "const")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_bindings_register_each_callable_frontier_once() {
        let depth = 32;
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!("void (^c{index})(void) = ^{{\n"));
        }
        source.push_str("return;\n");
        source.push_str(&"};\n".repeat(depth));
        crate::languages::tree::reset_callable_frontier_counts();
        parse_file(Path::new("Nested.m"), &source).expect("valid Objective-C");
        let (_, candidates, registrations) = crate::languages::tree::callable_frontier_counts();
        assert_eq!((candidates, registrations), (depth, depth));
    }
}
