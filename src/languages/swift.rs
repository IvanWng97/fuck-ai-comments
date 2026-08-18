use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    ANONYMOUS_FUNCTION_NAME, AttachmentIndex, AttachmentSyntax, CallableSubtrees,
    DirectivePlacement, LanguageSpec, OwnerCandidate, OwnerLocation, analyze, canonical_syntax,
    direct_named_child, document, has_direct_child, node_text,
};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

#[derive(Clone, Copy)]
struct Swift;

// SwiftLint reserves only the exact whitespace-delimited hyphen; bare hyphens remain rule text.
const SWIFTLINT_TRAILING_COMMENT_DELIMITER: &str = " - ";

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Swift)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, Swift)
}

impl LanguageSpec for Swift {
    type Context = AttachmentIndex;

    fn label(self) -> &'static str {
        "Swift"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_swift::LANGUAGE.into()
    }

    fn build_context(self, root: Node<'_>, source: &str) -> Self::Context {
        AttachmentIndex::with_syntax(
            root,
            source,
            AttachmentSyntax::default().with_physical_lines(),
        )
    }

    fn callable_kind(self) -> Option<fn(&str) -> bool> {
        Some(is_swift_callable)
    }

    fn owner(
        self,
        node: Node<'_>,
        _location: OwnerLocation<'_>,
        source: &str,
        _function_depth: usize,
        callable_subtrees: &CallableSubtrees,
    ) -> Option<OwnerCandidate> {
        owner_from_node(node, source, callable_subtrees)
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        context: &Self::Context,
    ) -> Option<CommentKind> {
        match node.kind() {
            "comment" | "multiline_comment"
                if swift_directive(node_text(node, source))
                    .is_some_and(|placement| context.is_attached(node, placement)) =>
            {
                Some(CommentKind::ToolDirective)
            }
            "comment" | "multiline_comment" => Some(CommentKind::Narrative),
            _ => None,
        }
    }
}

fn owner_from_node(
    node: Node<'_>,
    source: &str,
    callable_subtrees: &CallableSubtrees,
) -> Option<OwnerCandidate> {
    if let Some(segment) = type_segment(node, source) {
        let name = node
            .child_by_field_name("name")
            .map(|name| node_text(name, source).trim().to_owned())?;
        return Some(OwnerCandidate::type_owner(
            Span::from_node(node),
            name,
            vec![segment],
        ));
    }
    if property_has_body(node) {
        return Some(OwnerCandidate::function(
            Span::from_node(node),
            grouped_property_name(node, source)?,
            Vec::new(),
        ));
    }
    if let Some((name, roots)) = callable_binding(node, source, callable_subtrees) {
        return Some(
            OwnerCandidate::leaf(Span::from_node(node), name).suppressing_callable_frontiers(roots),
        );
    }
    let function_name = match node.kind() {
        "function_declaration" if node.child_by_field_name("body").is_some() => node
            .child_by_field_name("name")
            .map(|name| node_text(name, source).to_owned()),
        "init_declaration" if node.child_by_field_name("body").is_some() => Some("init".to_owned()),
        "deinit_declaration" if node.child_by_field_name("body").is_some() => {
            Some("deinit".to_owned())
        }
        "subscript_declaration"
            if direct_named_child(node, "computed_property")
                .is_some_and(computed_property_has_body) =>
        {
            Some("subscript".to_owned())
        }
        "lambda_literal" => Some(ANONYMOUS_FUNCTION_NAME.to_owned()),
        _ => None,
    };
    if let Some(name) = function_name {
        let identity = function_signature(node, source)
            .map(|signature| vec![format!("signature:{signature}")])
            .unwrap_or_default();
        return Some(OwnerCandidate::function(
            Span::from_node(node),
            name,
            identity,
        ));
    }
    let name = immutable_property_name(node, source)?;
    Some(OwnerCandidate::leaf(Span::from_node(node), name))
}

fn swift_directive(comment: &str) -> Option<DirectivePlacement> {
    swiftlint_directive(comment).or_else(|| swiftformat_directive(comment))
}

fn swiftlint_directive(comment: &str) -> Option<DirectivePlacement> {
    let body = comment.strip_prefix("//")?;
    if body.starts_with('/') {
        return None;
    }
    let body = body.trim_start();
    let separator = body.find(' ')?;
    let (command, rules) = body.split_at(separator);
    let rules = rules
        .split_once(SWIFTLINT_TRAILING_COMMENT_DELIMITER)
        .map_or(rules, |(rules, _)| rules);
    let placement = match command {
        "swiftlint:disable" | "swiftlint:enable" => DirectivePlacement::FreeStanding,
        "swiftlint:disable:next" | "swiftlint:enable:next" => {
            DirectivePlacement::PhysicalNextLine(0)
        }
        "swiftlint:disable:this" | "swiftlint:enable:this" => {
            DirectivePlacement::PhysicalSameLine(0)
        }
        "swiftlint:disable:previous" | "swiftlint:enable:previous" => {
            DirectivePlacement::PhysicalPreviousLine(0)
        }
        _ => return None,
    };
    valid_swiftlint_rule_list(rules).then_some(placement)
}

fn valid_swiftlint_rule_list(rules: &str) -> bool {
    rules.split_whitespace().next().is_some()
}

fn swiftformat_directive(comment: &str) -> Option<DirectivePlacement> {
    if let Some(body) = comment.strip_prefix("//") {
        let body = body.trim();
        let body = body.trim_start_matches(is_swiftformat_decoration);
        return (!body.contains(['\r', '\n']))
            .then(|| swiftformat_command(body))
            .flatten();
    }

    let body = comment.strip_prefix("/*")?.strip_suffix("*/")?;
    swiftformat_block_directive(body)
}

fn swiftformat_block_directive(body: &str) -> Option<DirectivePlacement> {
    let mut candidate = None;
    for (row_index, row) in body.split('\n').enumerate() {
        let row = row.trim_matches(is_swiftformat_decoration);
        if row.is_empty() {
            continue;
        }
        let placement = swiftformat_command(row)?;
        if candidate.replace((placement, row_index)).is_some() {
            return None;
        }
    }

    let (placement, candidate_row) = candidate?;
    Some(placement.with_physical_marker_row(candidate_row))
}

fn is_swiftformat_decoration(character: char) -> bool {
    character.is_whitespace() || matches!(character, '/' | '*')
}

fn swiftformat_command(body: &str) -> Option<DirectivePlacement> {
    let separator = body.find(' ')?;
    let (command, arguments) = body.split_at(separator);
    let (placement, valid_arguments) = match command {
        "swiftformat:disable" | "swiftformat:enable" => (
            DirectivePlacement::FreeStanding,
            valid_swiftformat_rule_list(arguments),
        ),
        "swiftformat:disable:next" | "swiftformat:enable:next" => (
            DirectivePlacement::PhysicalNextLine(0),
            valid_swiftformat_rule_list(arguments),
        ),
        "swiftformat:disable:this" | "swiftformat:enable:this" => (
            DirectivePlacement::PhysicalSameLine(0),
            valid_swiftformat_rule_list(arguments),
        ),
        "swiftformat:disable:previous" | "swiftformat:enable:previous" => (
            DirectivePlacement::PhysicalPreviousLine(0),
            valid_swiftformat_rule_list(arguments),
        ),
        _ => return None,
    };
    valid_arguments.then_some(placement)
}

fn valid_swiftformat_rule_list(rules: &str) -> bool {
    let mut rules = rules
        .split(|character: char| character == ',' || character.is_ascii_whitespace())
        .filter(|rule| !rule.is_empty());
    let Some(first) = rules.next() else {
        return false;
    };
    valid_swiftformat_rule(first) && rules.all(valid_swiftformat_rule)
}

fn valid_swiftformat_rule(rule: &str) -> bool {
    rule.as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && rule
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn property_has_body(node: Node<'_>) -> bool {
    if node.kind() != "property_declaration" {
        return false;
    }
    node.child_by_field_name("computed_value")
        .is_some_and(computed_property_has_body)
        || direct_named_child(node, "willset_didset_block").is_some()
}

fn computed_property_has_body(node: Node<'_>) -> bool {
    if node.kind() != "computed_property" {
        return false;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| match child.kind() {
            "statements" => true,
            "computed_getter" | "computed_setter" | "computed_modify" => {
                has_direct_child(child, "{")
            }
            _ => false,
        })
}

fn assignment_target(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind() == "assignment")
        .then(|| node.child_by_field_name("target"))
        .flatten()
}

fn grouped_property_name(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let patterns: Vec<_> = node.children_by_field_name("name", &mut cursor).collect();
    let names = patterns
        .into_iter()
        .map(|pattern| pattern.child_by_field_name("bound_identifier"))
        .collect::<Option<Vec<_>>>()?;
    let mut names: Vec<_> = names
        .into_iter()
        .map(|name| node_text(name, source))
        .collect();
    names.sort_unstable();
    (!names.is_empty()).then(|| names.join(","))
}

fn callable_binding(
    node: Node<'_>,
    source: &str,
    callable_subtrees: &CallableSubtrees,
) -> Option<(String, Vec<usize>)> {
    if node.kind() == "property_declaration" && !property_has_body(node) {
        let name = grouped_property_name(node, source)?;
        let mut cursor = node.walk();
        let mut roots = Vec::new();
        for value in node.children_by_field_name("value", &mut cursor) {
            if callable_subtrees.contains_callable(value) {
                roots.push(value.id());
            }
        }
        if !roots.is_empty() {
            return Some((name, roots));
        }
    }
    let target = assignment_target(node)?;
    let mut cursor = node.walk();
    let mut roots = Vec::new();
    for result in node.children_by_field_name("result", &mut cursor) {
        if callable_subtrees.contains_callable(result) {
            roots.push(result.id());
        }
    }
    (!roots.is_empty()).then(|| (node_text(target, source).trim().to_owned(), roots))
}

fn is_swift_callable(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "init_declaration"
            | "deinit_declaration"
            | "subscript_declaration"
            | "lambda_literal"
            | "computed_property"
            | "computed_getter"
            | "computed_setter"
            | "computed_modify"
            | "willset_didset_block"
    )
}

fn function_signature(node: Node<'_>, source: &str) -> Option<String> {
    let body = node
        .child_by_field_name("body")
        .or_else(|| direct_named_child(node, "computed_property"))?;
    let mut cursor = node.walk();
    let signature: String = node
        .children(&mut cursor)
        .filter(|child| child.id() != body.id())
        .map(|child| canonical_syntax(child, source))
        .collect();
    (!signature.is_empty()).then_some(signature)
}

fn immutable_property_name(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "property_declaration" || property_has_body(node) {
        return None;
    }
    let binding = direct_named_child(node, "value_binding_pattern")?;
    let mutability = binding.child_by_field_name("mutability")?;
    if node_text(mutability, source) != "let" {
        return None;
    }
    let pattern = node.child_by_field_name("name")?;
    pattern
        .child_by_field_name("bound_identifier")
        .map(|name| node_text(name, source).to_owned())
}

fn type_segment(node: Node<'_>, source: &str) -> Option<String> {
    let kind = match node.kind() {
        "class_declaration" => node
            .child_by_field_name("declaration_kind")
            .map(|kind| node_text(kind, source))?,
        "protocol_declaration" => "protocol",
        _ => return None,
    };
    let name = node.child_by_field_name("name")?;
    Some(format!("{kind}:{}", canonical_name(name, source)))
}

fn canonical_name(node: Node<'_>, source: &str) -> String {
    node_text(node, source)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested_directive_source(depth: usize) -> String {
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!(
                "class C{index} {{\n// swiftlint:disable:next force_cast\n"
            ));
        }
        source.push_str("let value = 1\n");
        source.push_str(&"}\n".repeat(depth));
        source
    }

    fn nested_directive_attachment_visits(depth: usize) -> usize {
        let source = nested_directive_source(depth);
        crate::languages::tree::reset_attachment_index_visits();
        crate::analyze_all(crate::SourceFile {
            path: Path::new("Deep.swift"),
            text: &source,
        })
        .expect("valid nested Swift");
        crate::languages::tree::attachment_index_visits()
    }

    fn nested_directive_physical_operations(depth: usize) -> usize {
        let source = nested_directive_source(depth);
        crate::languages::tree::reset_physical_attachment_operations();
        parse_file(Path::new("Deep.swift"), &source).expect("valid nested Swift");
        crate::languages::tree::physical_attachment_operations()
    }

    fn alternating_directive_physical_operations(count: usize) -> usize {
        let mut source = String::from("func work() {\n");
        for index in 0..count {
            source.push_str(&format!(
                "// swiftlint:disable:next custom-rule\ntarget{index}()\n"
            ));
        }
        source.push_str("}\n");
        crate::languages::tree::reset_physical_attachment_operations();
        parse_file(Path::new("Alternating.swift"), &source).expect("valid alternating Swift");
        crate::languages::tree::physical_attachment_operations()
    }

    #[test]
    fn nested_bindings_register_each_callable_frontier_once() {
        let depth = 64;
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!("let c{index} = {{\n"));
        }
        source.push_str("1\n");
        source.push_str(&"}\n".repeat(depth));
        crate::languages::tree::reset_callable_frontier_counts();
        parse_file(Path::new("Nested.swift"), &source).expect("valid Swift");
        let (_, candidates, registrations) = crate::languages::tree::callable_frontier_counts();
        assert_eq!((candidates, registrations), (depth, depth));
    }

    #[test]
    fn nested_directive_attachment_index_visits_are_linear() {
        let shallow = nested_directive_attachment_visits(16);
        let medium = nested_directive_attachment_visits(32);
        let deep = nested_directive_attachment_visits(64);
        assert_eq!(deep - medium, 2 * (medium - shallow));
    }

    #[test]
    fn alternating_directive_physical_operations_are_linear() {
        let shallow = alternating_directive_physical_operations(16);
        let medium = alternating_directive_physical_operations(32);
        let deep = alternating_directive_physical_operations(64);
        assert_eq!(deep - medium, 2 * (medium - shallow));
    }

    #[test]
    fn nested_directive_physical_operations_are_linear() {
        let shallow = nested_directive_physical_operations(16);
        let medium = nested_directive_physical_operations(32);
        let deep = nested_directive_physical_operations(64);
        assert_eq!(deep - medium, 2 * (medium - shallow));
    }
}
