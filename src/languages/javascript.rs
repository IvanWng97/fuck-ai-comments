use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    ANONYMOUS_FUNCTION_NAME, AttachmentIndex, AttachmentSyntax, CallableSubtrees,
    DirectivePlacement, LanguageSpec, OwnerCandidate, OwnerLocation, analyze, document,
    first_descendant_with_kind, function_name as default_function_name, has_direct_child,
    node_text,
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
        AttachmentIndex::with_syntax(root, source, attachment_syntax())
    }

    fn is_owner_prefix(self, kind: &str) -> bool {
        is_owner_prefix(kind)
    }

    fn callable_kind(self) -> Option<fn(&str) -> bool> {
        Some(is_function_kind)
    }

    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        _function_depth: usize,
        callable_subtrees: &CallableSubtrees,
    ) -> Option<OwnerCandidate> {
        owner_from_node(node, location, source, callable_subtrees)
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

pub(crate) fn attachment_syntax() -> AttachmentSyntax {
    AttachmentSyntax::new(
        |kind| kind == "jsx_expression",
        |kind| kind == "hash_bang_line",
    )
}

pub(crate) fn is_owner_prefix(kind: &str) -> bool {
    kind == "decorator"
}

pub(crate) fn owner_from_node(
    node: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
    callable_subtrees: &CallableSubtrees,
) -> Option<OwnerCandidate> {
    if let Some(binding) = CallableBinding::from_owner(node, callable_subtrees) {
        let name = binding.name(source);
        let span = prefixed_span(node, location);
        let callable_frontier_roots = binding.callable_frontier_roots;
        if default_export_value(node).is_some() {
            return Some(
                OwnerCandidate::function(span, name, Vec::new())
                    .suppressing_callable_frontiers(callable_frontier_roots),
            );
        }
        return Some(
            OwnerCandidate::leaf(span, name)
                .suppressing_callable_frontiers(callable_frontier_roots),
        );
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
    let kind = if tool_directive(node_text(node, source))
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
    if typescript_suppression_directive(comment) {
        return Some(DirectivePlacement::NextLine);
    }
    if comment.contains(['\r', '\n']) {
        return None;
    }
    if typescript_preamble_directive(comment) {
        return Some(DirectivePlacement::FilePreamble);
    }
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
    if eslint_env_directive(body) || style == CommentStyle::Block && body == "istanbul ignore file"
    {
        return Some(DirectivePlacement::FilePreamble);
    }
    if body == "prettier-ignore" {
        return Some(DirectivePlacement::NextNode);
    }
    if style == CommentStyle::Block
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

fn typescript_line_comment_body(comment: &str) -> Option<&str> {
    let body = comment.strip_prefix("//")?;
    Some(body.strip_prefix('/').unwrap_or(body).trim_start())
}

fn typescript_suppression_directive(comment: &str) -> bool {
    let candidate = if let Some(body) = typescript_line_comment_body(comment) {
        body
    } else {
        let Some(body) = comment
            .strip_prefix("/*")
            .and_then(|comment| comment.strip_suffix("*/"))
        else {
            return false;
        };
        let candidate_start = body.rfind(['\r', '\n']).map_or(0, |index| index + 1);
        let (preceding_rows, candidate) = body.split_at(candidate_start);
        if !preceding_rows
            .chars()
            .all(|character| character.is_whitespace() || character == '*')
        {
            return false;
        }
        candidate
            .trim_start_matches(char::is_whitespace)
            .trim_start_matches(['/', '*'])
            .trim_start()
    };
    ["@ts-ignore", "@ts-expect-error"]
        .iter()
        .any(|directive| candidate.starts_with(directive))
}

fn typescript_preamble_directive(comment: &str) -> bool {
    let Some(body) = typescript_line_comment_body(comment) else {
        return false;
    };
    ["@ts-check", "@ts-nocheck"].iter().any(|directive| {
        body.strip_prefix(directive).is_some_and(|suffix| {
            suffix.is_empty() || suffix.starts_with(':') || suffix.starts_with(char::is_whitespace)
        })
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

impl<'tree> CallableBinding<'tree> {
    fn from_owner(owner: Node<'tree>, callable_subtrees: &CallableSubtrees) -> Option<Self> {
        let mut callable_frontier_roots = Vec::new();
        if is_callable_binding_owner(owner.kind()) {
            for_each_binding(owner, |binding| {
                if let Some(value) = binding.child_by_field_name("value")
                    && callable_subtrees.contains_callable(value)
                {
                    callable_frontier_roots.push(value.id());
                }
            });
        } else if let Some(value) = default_export_value(owner)
            && callable_subtrees.contains_callable(value)
        {
            callable_frontier_roots.push(value.id());
        } else if let Some(right) = assignment_right(owner)
            && callable_subtrees.contains_callable(right)
        {
            callable_frontier_roots.push(right.id());
        }
        (!callable_frontier_roots.is_empty()).then_some(Self {
            owner,
            callable_frontier_roots,
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
    callable_frontier_roots: Vec<usize>,
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
    if owner.kind() != "export_statement" || !has_direct_child(owner, "default") {
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
    if let Some(prefix) = location.leading_prefix() {
        span.start_byte = prefix.start_byte();
        span.start_line = prefix.start_position().row + 1;
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
    use tree_sitter::Parser;

    #[test]
    fn unbound_nested_callable_names_do_not_scan_descendants() {
        let depth = 64;
        let source = format!("{}1;\n", "() => ".repeat(depth));
        crate::languages::tree::reset_first_descendant_visits();

        parse_file(Path::new("nested.js"), &source).expect("valid nested JavaScript callables");

        assert_eq!(crate::languages::tree::first_descendant_visits(), 0);
    }

    fn chained_assignment_frontier_counts(depth: usize, width: usize) -> (usize, usize, usize) {
        let assignments = (0..depth)
            .map(|index| format!("x{index} = "))
            .collect::<String>();
        let callables = (0..width)
            .map(|index| format!("() => {index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("const root = {assignments}[{callables}];\n");
        crate::languages::tree::reset_callable_frontier_counts();
        parse_file(Path::new("frontier.js"), &source).expect("valid JavaScript");
        crate::languages::tree::callable_frontier_counts()
    }

    fn grouped_binding_frontier_counts(size: usize) -> (usize, usize, usize) {
        let bindings = (0..size)
            .map(|index| format!("f{index} = () => {index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("const {bindings};\n");
        crate::languages::tree::reset_callable_frontier_counts();
        parse_file(Path::new("frontier.js"), &source).expect("valid JavaScript");
        crate::languages::tree::callable_frontier_counts()
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

    fn syntax_node_count(language: tree_sitter::Language, source: &str) -> usize {
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("JavaScript-family grammar should load");
        parser
            .parse(source, None)
            .expect("parser should return a tree")
            .root_node()
            .descendant_count()
    }

    #[test]
    fn chained_assignment_frontier_work_is_linear_when_deep() {
        let shallow = chained_assignment_frontier_counts(64, 1);
        let medium = chained_assignment_frontier_counts(128, 1);
        let deep = chained_assignment_frontier_counts(256, 1);

        assert_eq!(deep.0 - medium.0, 2 * (medium.0 - shallow.0));
        assert_eq!(
            [
                (shallow.1, shallow.2),
                (medium.1, medium.2),
                (deep.1, deep.2)
            ],
            [(1, 65), (1, 129), (1, 257)],
        );
    }

    #[test]
    fn chained_assignment_frontier_work_is_linear_when_deep_and_wide() {
        let shallow = chained_assignment_frontier_counts(64, 64);
        let medium = chained_assignment_frontier_counts(128, 128);
        let deep = chained_assignment_frontier_counts(256, 256);

        assert_eq!(deep.0 - medium.0, 2 * (medium.0 - shallow.0));
        assert_eq!(
            [
                (shallow.1, shallow.2),
                (medium.1, medium.2),
                (deep.1, deep.2)
            ],
            [(64, 65), (128, 129), (256, 257)],
        );
    }

    #[test]
    fn grouped_binding_frontier_work_is_linear() {
        let shallow = grouped_binding_frontier_counts(64);
        let medium = grouped_binding_frontier_counts(128);
        let deep = grouped_binding_frontier_counts(256);

        assert_eq!(deep.0 - medium.0, 2 * (medium.0 - shallow.0));
        assert_eq!(
            [
                (shallow.1, shallow.2),
                (medium.1, medium.2),
                (deep.1, deep.2)
            ],
            [(64, 64), (128, 128), (256, 256)],
        );
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

    #[test]
    fn attachment_index_visits_each_javascript_family_node_once() {
        let cases = [
            (
                Path::new("render.js"),
                "// prettier-ignore\n// context\nrender();\n",
                tree_sitter_javascript::LANGUAGE.into(),
            ),
            (
                Path::new("render.ts"),
                "// prettier-ignore\n// context\nrender<string>();\n",
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            ),
            (
                Path::new("Card.tsx"),
                "const Card = () => <main>{/* prettier-ignore */}{/* context */}<span /></main>;\n",
                tree_sitter_typescript::LANGUAGE_TSX.into(),
            ),
        ];

        for (path, source, language) in cases {
            let expected = syntax_node_count(language, source);
            crate::languages::tree::reset_attachment_index_visits();

            crate::analyze_all(crate::SourceFile { path, text: source })
                .expect("valid JavaScript-family source");

            assert_eq!(
                crate::languages::tree::attachment_index_visits(),
                expected,
                "{path:?} must use one forward attachment visit per syntax node"
            );
        }
    }
}
