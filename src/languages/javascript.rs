use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    ANONYMOUS_FUNCTION_NAME, AttachmentIndex, DirectivePlacement, LanguageSpec, OwnerCandidate,
    OwnerLocation, analyze, document, first_descendant_with_kind,
    function_name as default_function_name, node_text, outermost_node_ids_matching,
};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

// A module can have only one default export, so it supplies a stable identity without a binding.
const DEFAULT_EXPORT_NAME: &str = "default";

#[derive(Clone, Copy)]
struct JavaScript;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, JavaScript)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, JavaScript)
}

impl LanguageSpec for JavaScript {
    type Context = AttachmentIndex;

    fn label(self) -> &'static str {
        "JavaScript"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_javascript::LANGUAGE.into()
    }

    fn build_context(self, root: Node<'_>, source: &str) -> Self::Context {
        AttachmentIndex::from_root(root, source)
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
        source: &str,
        context: &Self::Context,
    ) -> Option<CommentKind> {
        classify_comment(node, source, context)
    }
}

pub(crate) fn owner_from_node(
    node: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
) -> Option<OwnerCandidate> {
    if let Some(binding) = CallableBinding::from_owner(node) {
        let name = binding.name(source);
        let span = prefixed_span(node, location);
        let suppressed = binding.suppressed_nodes;
        if default_export_value(node).is_some() {
            return Some(OwnerCandidate::function(span, name, Vec::new()).suppressing(suppressed));
        }
        return Some(OwnerCandidate::leaf(span, name).suppressing(suppressed));
    }
    if let Some(candidate) = class_owner(node, location, source) {
        return Some(candidate);
    }
    if is_function_kind(node.kind()) {
        let name = function_name(node, location, source);
        return Some(OwnerCandidate::function(
            prefixed_span(node, location),
            name,
            Vec::new(),
        ));
    }
    leaf_owner(node, location, source)
}

pub(crate) fn function_name(node: Node<'_>, location: OwnerLocation<'_>, source: &str) -> String {
    let name = default_function_name(node, location, source);
    if name == ANONYMOUS_FUNCTION_NAME && export_prefix(node, location).is_some() {
        DEFAULT_EXPORT_NAME.to_owned()
    } else {
        name
    }
}

pub(crate) fn classify_comment(
    node: Node<'_>,
    source: &str,
    attachments: &AttachmentIndex,
) -> Option<CommentKind> {
    if node.kind() != "comment" {
        return None;
    }
    let kind = if node.start_position().row == node.end_position().row
        && tool_directive(node_text(node, source))
            .is_some_and(|placement| attachments.is_attached(node, placement))
    {
        CommentKind::ToolDirective
    } else {
        CommentKind::Narrative
    };
    Some(kind)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CommentStyle {
    Line,
    Block,
}

fn tool_directive(comment: &str) -> Option<DirectivePlacement> {
    let comment = comment.trim();
    let (style, body) = if let Some(body) = comment.strip_prefix("//") {
        (CommentStyle::Line, body.trim())
    } else {
        let body = comment.strip_prefix("/*")?.strip_suffix("*/")?;
        (CommentStyle::Block, body.trim())
    };

    if eslint_rule_directive(body, "eslint-disable-next-line") {
        return Some(DirectivePlacement::NextLine);
    }
    if eslint_rule_directive(body, "eslint-disable-line") {
        return Some(DirectivePlacement::SameLine);
    }
    if eslint_rule_directive(body, "eslint-disable")
        || eslint_rule_directive(body, "eslint-enable")
        || style == CommentStyle::Block && c8_region_directive(body)
    {
        return Some(DirectivePlacement::FreeStanding);
    }
    if eslint_env_directive(body)
        || style == CommentStyle::Line && matches!(body, "@ts-check" | "@ts-nocheck")
        || style == CommentStyle::Block && body == "istanbul ignore file"
    {
        return Some(DirectivePlacement::FilePreamble);
    }
    if body == "prettier-ignore" {
        return Some(DirectivePlacement::NextNode);
    }
    if style == CommentStyle::Line && typescript_line_directive(body)
        || style == CommentStyle::Block
            && matches!(
                body,
                "istanbul ignore next"
                    | "istanbul ignore if"
                    | "istanbul ignore else"
                    | "c8 ignore next"
            )
        || style == CommentStyle::Block && numbered_directive(body, "istanbul ignore next")
        || style == CommentStyle::Block && numbered_directive(body, "c8 ignore next")
    {
        return Some(DirectivePlacement::NextLine);
    }
    None
}

fn eslint_rule_directive(body: &str, directive: &str) -> bool {
    if body == directive {
        return true;
    }
    body.strip_prefix(directive).is_some_and(|suffix| {
        suffix.starts_with(char::is_whitespace) && valid_eslint_rule_list(suffix.trim())
    })
}

fn valid_eslint_rule_list(value: &str) -> bool {
    let (rules, valid_description) = value
        .split_once(" -- ")
        .map_or((value, true), |(rules, description)| {
            (rules, !description.trim().is_empty())
        });
    valid_description
        && !rules.is_empty()
        && rules.split(',').all(|rule| {
            let rule = rule.trim();
            !rule.is_empty()
                && rule.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || matches!(character, '@' | '_' | '-' | '/' | '.')
                })
        })
}

fn eslint_env_directive(body: &str) -> bool {
    let Some(environments) = body.strip_prefix("eslint-env") else {
        return false;
    };
    if !environments.starts_with(char::is_whitespace) {
        return false;
    }
    environments.trim().split(',').all(|environment| {
        let mut parts = environment.trim().split(':');
        let Some(name) = parts.next() else {
            return false;
        };
        !name.is_empty()
            && name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
            && parts
                .next()
                .is_none_or(|enabled| matches!(enabled.trim(), "true" | "false"))
            && parts.next().is_none()
    })
}

fn typescript_line_directive(body: &str) -> bool {
    ["@ts-ignore", "@ts-expect-error"].iter().any(|directive| {
        body == *directive
            || body
                .strip_prefix(directive)
                .and_then(|suffix| suffix.strip_prefix(':'))
                .is_some_and(|description| !description.trim().is_empty())
    })
}

fn numbered_directive(body: &str, directive: &str) -> bool {
    body.strip_prefix(directive)
        .and_then(|suffix| suffix.strip_prefix(' '))
        .is_some_and(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()))
}

fn c8_region_directive(body: &str) -> bool {
    matches!(body, "c8 ignore start" | "c8 ignore stop")
}

fn is_function_kind(kind: &str) -> bool {
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

impl<'tree> CallableBinding<'tree> {
    fn from_owner(owner: Node<'tree>) -> Option<Self> {
        let mut suppressed_nodes = Vec::new();
        if is_callable_binding_owner(owner.kind()) {
            for_each_binding(owner, |binding| {
                if let Some(value) = binding.child_by_field_name("value") {
                    suppressed_nodes.extend(outermost_node_ids_matching(value, is_function_kind));
                }
            });
        } else if let Some(value) = default_export_value(owner) {
            suppressed_nodes.extend(outermost_node_ids_matching(value, is_function_kind));
        } else if let Some(right) = assignment_right(owner) {
            suppressed_nodes.extend(outermost_node_ids_matching(right, is_function_kind));
        }
        (!suppressed_nodes.is_empty()).then_some(Self {
            owner,
            suppressed_nodes,
        })
    }

    fn name(&self, source: &str) -> String {
        if default_export_value(self.owner).is_some() {
            return DEFAULT_EXPORT_NAME.to_owned();
        }
        if let Some(left) = assignment_left(self.owner) {
            return node_text(left, source).to_owned();
        }
        let mut name = String::new();
        for_each_binding(self.owner, |binding| {
            if let Some(binding_name) = binding.child_by_field_name("name") {
                if !name.is_empty() {
                    name.push_str(", ");
                }
                name.push_str(node_text(binding_name, source));
            }
        });
        name
    }
}

#[derive(Clone)]
struct CallableBinding<'tree> {
    owner: Node<'tree>,
    suppressed_nodes: Vec<usize>,
}

fn is_callable_binding_owner(kind: &str) -> bool {
    matches!(
        kind,
        "lexical_declaration"
            | "variable_declaration"
            | "field_definition"
            | "public_field_definition"
    )
}

fn default_export_value(owner: Node<'_>) -> Option<Node<'_>> {
    if owner.kind() != "export_statement" {
        return None;
    }
    let mut cursor = owner.walk();
    if !owner
        .children(&mut cursor)
        .any(|child| child.kind() == "default")
    {
        return None;
    }
    owner.child_by_field_name("value")
}

fn assignment_left(owner: Node<'_>) -> Option<Node<'_>> {
    (owner.kind() == "assignment_expression")
        .then(|| owner.child_by_field_name("left"))
        .flatten()
}

fn assignment_right(owner: Node<'_>) -> Option<Node<'_>> {
    (owner.kind() == "assignment_expression")
        .then(|| owner.child_by_field_name("right"))
        .flatten()
}

fn for_each_binding<'tree>(owner: Node<'tree>, mut visit: impl FnMut(Node<'tree>)) {
    match owner.kind() {
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = owner.walk();
            for binding in owner
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "variable_declarator")
            {
                visit(binding);
            }
        }
        "field_definition" | "public_field_definition" => visit(owner),
        _ => {}
    }
}

pub(crate) fn prefixed_span(node: Node<'_>, location: OwnerLocation<'_>) -> Span {
    if let Some(export) = export_prefix(node, location) {
        return Span::from_node(export);
    }
    let mut span = Span::from_node(node);
    if let Some(decorator) = location.leading_decorator() {
        span.start_byte = decorator.start_byte();
        span.start_line = decorator.start_position().row + 1;
    }
    span
}

fn export_prefix<'tree>(node: Node<'tree>, location: OwnerLocation<'tree>) -> Option<Node<'tree>> {
    let parent = location.parent()?;
    if parent.kind() != "export_statement" {
        return None;
    }
    parent
        .child_by_field_name("value")
        .or_else(|| parent.child_by_field_name("declaration"))
        .is_some_and(|value| value.id() == node.id())
        .then_some(parent)
}

fn leaf_owner(node: Node<'_>, location: OwnerLocation<'_>, source: &str) -> Option<OwnerCandidate> {
    if node.kind() != "lexical_declaration"
        || node
            .child_by_field_name("kind")
            .is_none_or(|kind| kind.kind() != "const")
    {
        return None;
    }
    let declarator = first_descendant_with_kind(node, "variable_declarator")?;
    let name = declarator
        .child_by_field_name("name")
        .map_or("<destructured>", |name| node_text(name, source));
    Some(OwnerCandidate::leaf(
        prefixed_span(node, location),
        name.to_owned(),
    ))
}

fn class_owner(
    node: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
) -> Option<OwnerCandidate> {
    if !matches!(
        node.kind(),
        "class" | "class_declaration" | "abstract_class_declaration"
    ) {
        return None;
    }
    let name = node
        .child_by_field_name("name")
        .map_or("<anonymous>", |name| node_text(name, source))
        .to_owned();
    Some(OwnerCandidate::type_owner(
        prefixed_span(node, location),
        name.clone(),
        vec![format!("class:{name}")],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration_scan_count(binding_count: usize) -> usize {
        let declarations = (0..binding_count)
            .map(|index| format!("C{index} = () => {index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("const {declarations};\n");
        crate::languages::tree::reset_outermost_scan_visits();
        parse_file(Path::new("components.js"), &source).expect("valid JavaScript");
        crate::languages::tree::outermost_scan_visits()
    }

    fn wrapper_scan_count(depth: usize) -> usize {
        let source = format!(
            "const handler = {}() => 1{};\n",
            "wrap(".repeat(depth),
            ")".repeat(depth)
        );
        crate::languages::tree::reset_outermost_scan_visits();
        parse_file(Path::new("components.js"), &source).expect("valid JavaScript");
        crate::languages::tree::outermost_scan_visits()
    }

    fn nested_binding_scan_count(depth: usize) -> usize {
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!("const C{index} = () => {{\n"));
        }
        source.push_str("return 1;\n");
        source.push_str(&"};\n".repeat(depth));
        crate::languages::tree::reset_outermost_scan_visits();
        parse_file(Path::new("components.js"), &source).expect("valid JavaScript");
        crate::languages::tree::outermost_scan_visits()
    }

    fn public_nested_typescript_parent_probes(depth: usize) -> usize {
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!("const C{index} = () => {{\n"));
        }
        source.push_str("return 1;\n");
        source.push_str(&"};\n".repeat(depth));
        crate::languages::tree::reset_owner_parent_probes();
        crate::analyze_all(crate::SourceFile {
            path: Path::new("Deep.tsx"),
            text: &source,
        })
        .expect("valid nested TSX");
        crate::languages::tree::owner_parent_probes()
    }

    fn public_preamble_attachment_visits(comment: &str, count: usize) -> usize {
        let source = format!("{}const value = 1;\n", format!("{comment}\n").repeat(count));
        crate::languages::tree::reset_attachment_index_visits();
        crate::analyze_all(crate::SourceFile {
            path: Path::new("preamble.ts"),
            text: &source,
        })
        .expect("valid TypeScript preamble");
        crate::languages::tree::attachment_index_visits()
    }

    fn nested_directive_attachment_visits(depth: usize) -> usize {
        let mut source = String::new();
        for index in 0..depth {
            source.push_str(&format!(
                "function f{index}() {{\n// eslint-disable-next-line no-console\n"
            ));
        }
        source.push_str("return 1;\n");
        source.push_str(&"}\n".repeat(depth));
        crate::languages::tree::reset_attachment_index_visits();
        crate::analyze_all(crate::SourceFile {
            path: Path::new("deep.js"),
            text: &source,
        })
        .expect("valid nested JavaScript");
        crate::languages::tree::attachment_index_visits()
    }

    #[test]
    fn binding_owner_scans_do_not_scale_with_contained_callables() {
        assert_eq!(declaration_scan_count(64), 64);
    }

    #[test]
    fn wrapper_scan_visits_grow_linearly_with_wrapper_nodes() {
        let shallow = wrapper_scan_count(8);
        let medium = wrapper_scan_count(16);
        let deep = wrapper_scan_count(32);
        assert_eq!(deep - medium, 2 * (medium - shallow));
    }

    #[test]
    fn nested_bindings_scan_each_outer_callable_once() {
        assert_eq!(nested_binding_scan_count(64), 64);
    }

    #[test]
    fn public_nested_typescript_owner_parent_probes_are_linear() {
        for depth in [50, 100, 200, 400] {
            assert_eq!(public_nested_typescript_parent_probes(depth), depth);
        }
    }

    #[test]
    fn javascript_and_typescript_preambles_are_indexed_linearly() {
        for comment in ["// @ts-check", "/// <reference types=\"node\" />"] {
            let shallow = public_preamble_attachment_visits(comment, 100);
            let medium = public_preamble_attachment_visits(comment, 200);
            let deep = public_preamble_attachment_visits(comment, 400);
            assert_eq!(deep - medium, 2 * (medium - shallow));
        }
    }

    #[test]
    fn nested_directive_attachment_index_visits_are_linear() {
        let shallow = nested_directive_attachment_visits(16);
        let medium = nested_directive_attachment_visits(32);
        let deep = nested_directive_attachment_visits(64);
        assert_eq!(deep - medium, 2 * (medium - shallow));
    }
}
