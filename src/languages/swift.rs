use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    ANONYMOUS_FUNCTION_NAME, AttachmentIndex, DirectivePlacement, LanguageSpec, OwnerCandidate,
    OwnerLocation, analyze, canonical_syntax, direct_named_child, document, node_text,
    outermost_node_ids_matching,
};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

#[derive(Clone, Copy)]
struct Swift;

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
        AttachmentIndex::from_root(root, source)
    }

    fn owner(
        self,
        node: Node<'_>,
        _location: OwnerLocation<'_>,
        source: &str,
        _function_depth: usize,
    ) -> Option<OwnerCandidate> {
        owner_from_node(node, source)
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        context: &Self::Context,
    ) -> Option<CommentKind> {
        match node.kind() {
            "comment"
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

fn owner_from_node(node: Node<'_>, source: &str) -> Option<OwnerCandidate> {
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
    if let Some((name, suppressed)) = callable_binding(node, source) {
        return Some(OwnerCandidate::leaf(Span::from_node(node), name).suppressing(suppressed));
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
    let body = body.trim();
    let separator = body.find(char::is_whitespace)?;
    let (command, rules) = body.split_at(separator);
    let placement = match command {
        "swiftlint:disable" | "swiftlint:enable" => DirectivePlacement::Region,
        "swiftlint:disable:next" | "swiftlint:enable:next" => DirectivePlacement::NextLine,
        "swiftlint:disable:this" | "swiftlint:enable:this" => DirectivePlacement::SameLine,
        "swiftlint:disable:previous" | "swiftlint:enable:previous" => {
            DirectivePlacement::PreviousLine
        }
        _ => return None,
    };
    valid_swiftlint_rule_list(rules).then_some(placement)
}

fn valid_swiftlint_rule_list(rules: &str) -> bool {
    let mut rules = rules.split_ascii_whitespace();
    let Some(first) = rules.next() else {
        return false;
    };
    valid_swiftlint_rule(first) && rules.all(valid_swiftlint_rule)
}

fn valid_swiftlint_rule(rule: &str) -> bool {
    rule.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && rule
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn swiftformat_directive(comment: &str) -> Option<DirectivePlacement> {
    let body = comment.strip_prefix("//")?;
    if body.starts_with('/') {
        return None;
    }
    let body = body.trim();
    let separator = body.find(char::is_whitespace)?;
    let (command, arguments) = body.split_at(separator);
    let (placement, valid_arguments) = match command {
        "swiftformat:disable" | "swiftformat:enable" => (
            DirectivePlacement::Region,
            valid_swiftformat_rule_list(arguments),
        ),
        "swiftformat:disable:next" | "swiftformat:enable:next" => (
            DirectivePlacement::NextLine,
            valid_swiftformat_rule_list(arguments),
        ),
        "swiftformat:disable:this" | "swiftformat:enable:this" => (
            DirectivePlacement::SameLine,
            valid_swiftformat_rule_list(arguments),
        ),
        "swiftformat:disable:previous" | "swiftformat:enable:previous" => (
            DirectivePlacement::PreviousLine,
            valid_swiftformat_rule_list(arguments),
        ),
        "swiftformat:options" => (
            DirectivePlacement::Region,
            valid_swiftformat_options(arguments),
        ),
        "swiftformat:options:next" => (
            DirectivePlacement::NextLine,
            valid_swiftformat_options(arguments),
        ),
        "swiftformat:options:this" => (
            DirectivePlacement::SameLine,
            valid_swiftformat_options(arguments),
        ),
        "swiftformat:options:previous" => (
            DirectivePlacement::PreviousLine,
            valid_swiftformat_options(arguments),
        ),
        _ => return None,
    };
    valid_arguments.then_some(placement)
}

fn valid_swiftformat_rule_list(rules: &str) -> bool {
    let mut rules = rules.split_ascii_whitespace();
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

fn valid_swiftformat_options(arguments: &str) -> bool {
    let mut arguments = arguments.split_ascii_whitespace();
    let mut found = false;
    while let Some(option) = arguments.next() {
        let Some(name) = option.strip_prefix("--") else {
            return false;
        };
        if !valid_swiftformat_option_name(name) {
            return false;
        }
        let Some(value) = arguments.next() else {
            return false;
        };
        if value.starts_with("--") || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return false;
        }
        found = true;
    }
    found
}

fn valid_swiftformat_option_name(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
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

fn callable_binding(node: Node<'_>, source: &str) -> Option<(String, Vec<usize>)> {
    if node.kind() == "property_declaration" && !property_has_body(node) {
        let name = grouped_property_name(node, source)?;
        let mut cursor = node.walk();
        let mut suppressed = Vec::new();
        for value in node.children_by_field_name("value", &mut cursor) {
            suppressed.extend(outermost_node_ids_matching(value, is_swift_callable));
        }
        if !suppressed.is_empty() {
            return Some((name, suppressed));
        }
    }
    let target = assignment_target(node)?;
    let mut cursor = node.walk();
    let mut suppressed = Vec::new();
    for result in node.children_by_field_name("result", &mut cursor) {
        suppressed.extend(outermost_node_ids_matching(result, is_swift_callable));
    }
    (!suppressed.is_empty()).then(|| (node_text(target, source).trim().to_owned(), suppressed))
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

fn has_direct_child(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| child.kind() == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested_directive_attachment_visits(depth: usize) -> usize {
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!(
                "class C{index} {{\n// swiftlint:disable:next force_cast\n"
            ));
        }
        source.push_str("let value = 1\n");
        source.push_str(&"}\n".repeat(depth));
        crate::languages::tree::reset_attachment_index_visits();
        crate::analyze_all(crate::SourceFile {
            path: Path::new("Deep.swift"),
            text: &source,
        })
        .expect("valid nested Swift");
        crate::languages::tree::attachment_index_visits()
    }

    #[test]
    fn nested_bindings_scan_each_outer_closure_once() {
        let depth = 64;
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!("let c{index} = {{\n"));
        }
        source.push_str("1\n");
        source.push_str(&"}\n".repeat(depth));
        crate::languages::tree::reset_outermost_scan_visits();
        parse_file(Path::new("Nested.swift"), &source).expect("valid Swift");
        assert_eq!(crate::languages::tree::outermost_scan_visits(), depth);
    }

    #[test]
    fn nested_directive_attachment_index_visits_are_linear() {
        let shallow = nested_directive_attachment_visits(16);
        let medium = nested_directive_attachment_visits(32);
        let deep = nested_directive_attachment_visits(64);
        assert_eq!(deep - medium, 2 * (medium - shallow));
    }
}
