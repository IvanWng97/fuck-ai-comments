use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    CallableSubtrees, LanguageSpec, OwnerCandidate, OwnerLocation, analyze, document,
    first_descendant_with_kind, function_name, node_text, starts_physical_line,
};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

const BANDIT_NUMERIC_TEST_ID_DIGITS: usize = 3;

#[derive(Clone, Copy)]
struct Python;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Python)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, Python)
}

impl LanguageSpec for Python {
    type Context = ();

    fn label(self) -> &'static str {
        "Python"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        function_depth: usize,
        _callable_subtrees: &CallableSubtrees,
    ) -> Option<OwnerCandidate> {
        if let Some(class) = class_owner_node(node, location) {
            let name = class
                .child_by_field_name("name")
                .map(|name| node_text(name, source).to_owned())?;
            return Some(OwnerCandidate::type_owner(
                Span::from_node(node),
                name.clone(),
                vec![format!("class:{name}")],
            ));
        }
        if function_owner_node(node, location).is_some() {
            return Some(OwnerCandidate::function(
                Span::from_node(node),
                function_name(node, location, source),
                Vec::new(),
            ));
        }
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
        uppercase.then(|| OwnerCandidate::leaf(Span::from_node(node), name.to_owned()))
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        _context: &Self::Context,
    ) -> Option<CommentKind> {
        if let Some(scope) = docstring_scope(node, source) {
            return Some(match scope {
                DocstringScope::Function => CommentKind::Narrative,
                DocstringScope::Type => CommentKind::TypeNarrative,
                DocstringScope::File => CommentKind::FileNarrative,
            });
        }
        (node.kind() == "comment").then(|| {
            if is_tool_directive(node, source) {
                CommentKind::ToolDirective
            } else {
                CommentKind::Narrative
            }
        })
    }
}

fn class_owner_node<'tree>(
    node: Node<'tree>,
    location: OwnerLocation<'tree>,
) -> Option<Node<'tree>> {
    match node.kind() {
        "decorated_definition" => decorated_body(node, "class_definition"),
        "class_definition"
            if location
                .parent()
                .is_none_or(|parent| parent.kind() != "decorated_definition") =>
        {
            Some(node)
        }
        _ => None,
    }
}

fn function_owner_node<'tree>(
    node: Node<'tree>,
    location: OwnerLocation<'tree>,
) -> Option<Node<'tree>> {
    match node.kind() {
        "decorated_definition" => decorated_body(node, "function_definition"),
        "function_definition"
            if location
                .parent()
                .is_none_or(|parent| parent.kind() != "decorated_definition") =>
        {
            Some(node)
        }
        _ => None,
    }
}

fn decorated_body<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn is_tool_directive(node: Node<'_>, source: &str) -> bool {
    let body = comment_body(node_text(node, source));
    match body {
        "fmt: off" | "fmt: on" => is_standalone_format_marker(node, source),
        "fmt: skip" => is_statement_trailing(node, source),
        _ => is_statement_trailing(node, source) && is_line_suppression(body),
    }
}

fn comment_body(comment: &str) -> &str {
    comment
        .trim_start()
        .strip_prefix('#')
        .unwrap_or(comment)
        .trim()
}

fn is_line_suppression(body: &str) -> bool {
    let directive = body.split_once(" #").map_or(body, |(head, _)| head).trim();
    is_noqa(directive)
        || is_type_ignore(directive)
        || directive.eq_ignore_ascii_case("pragma: no cover")
        || is_nosec(directive)
}

fn is_noqa(directive: &str) -> bool {
    if directive.eq_ignore_ascii_case("noqa") {
        return true;
    }
    strip_ascii_case_prefix(directive, "noqa:").is_some_and(|codes| valid_list(codes, is_lint_code))
}

fn is_type_ignore(directive: &str) -> bool {
    if directive == "type: ignore" {
        return true;
    }
    directive
        .strip_prefix("type: ignore[")
        .and_then(|codes| codes.strip_suffix(']'))
        .is_some_and(|codes| valid_list(codes, is_mypy_code))
}

fn is_nosec(directive: &str) -> bool {
    if directive == "nosec" {
        return true;
    }
    directive
        .strip_prefix("nosec ")
        .is_some_and(|tests| valid_list(tests, is_bandit_test))
}

fn valid_list(input: &str, valid_item: fn(&str) -> bool) -> bool {
    let mut items = input
        .split(|character: char| character == ',' || character.is_ascii_whitespace())
        .filter(|item| !item.is_empty());
    let Some(first) = items.next() else {
        return false;
    };
    valid_item(first) && items.all(valid_item)
}

fn is_lint_code(code: &str) -> bool {
    let letters = code.bytes().take_while(u8::is_ascii_uppercase).count();
    letters > 0 && letters < code.len() && code.as_bytes()[letters..].iter().all(u8::is_ascii_digit)
}

fn is_mypy_code(code: &str) -> bool {
    code.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && code
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_bandit_test(test: &str) -> bool {
    let is_id = test.strip_prefix('B').is_some_and(|digits| {
        digits.len() == BANDIT_NUMERIC_TEST_ID_DIGITS
            && digits.bytes().all(|byte| byte.is_ascii_digit())
    });
    let is_name = test.contains('_')
        && test
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    is_id || is_name
}

fn strip_ascii_case_prefix<'text>(text: &'text str, prefix: &str) -> Option<&'text str> {
    text.get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))?;
    text.get(prefix.len()..)
}

fn is_statement_trailing(node: Node<'_>, source: &str) -> bool {
    let line_start = source[..node.start_byte()]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    if source.as_bytes()[line_start..node.start_byte()]
        .iter()
        .all(u8::is_ascii_whitespace)
    {
        return false;
    }

    let row = node.start_position().row;
    let mut current = node;
    loop {
        if current.prev_named_sibling().is_some_and(|previous| {
            previous.end_position().row == row && controls_line_directive(previous.kind())
        }) {
            return true;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        if parent.start_position().row == row
            && parent.start_byte() < node.start_byte()
            && controls_line_directive(parent.kind())
        {
            return true;
        }
        if matches!(parent.kind(), "module" | "block") {
            return false;
        }
        current = parent;
    }
}

fn controls_line_directive(kind: &str) -> bool {
    kind.ends_with("_statement")
        || kind.ends_with("_definition")
        || matches!(
            kind,
            "decorator"
                | "elif_clause"
                | "else_clause"
                | "except_clause"
                | "finally_clause"
                | "case_clause"
        )
}

fn is_standalone_format_marker(node: Node<'_>, source: &str) -> bool {
    if !starts_physical_line(node, source) {
        return false;
    }
    node.parent().is_some_and(|parent| {
        matches!(parent.kind(), "module" | "block")
            || (controls_line_directive(parent.kind()) && has_direct_block(parent))
    })
}

fn has_direct_block(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == "block")
}

#[derive(Clone, Copy)]
enum DocstringScope {
    Function,
    Type,
    File,
}

fn docstring_scope(node: Node<'_>, source: &str) -> Option<DocstringScope> {
    if !is_static_string_expression(node, source) {
        return None;
    }
    let statement = node
        .parent()
        .filter(|parent| parent.kind() == "expression_statement")?;
    let container = statement.parent()?;
    let scope = match container.kind() {
        "module" => DocstringScope::File,
        "block" => match container.parent().map(|parent| parent.kind()) {
            Some("function_definition") => DocstringScope::Function,
            Some("class_definition") => DocstringScope::Type,
            _ => return None,
        },
        _ => return None,
    };
    container
        .named_child(0)
        .is_some_and(|first_statement| first_statement.id() == statement.id())
        .then_some(scope)
}

fn is_static_string_expression(mut node: Node<'_>, source: &str) -> bool {
    while node.kind() == "parenthesized_expression" {
        let Some(child) = node.named_child(0) else {
            return false;
        };
        if node.named_child_count() != 1 {
            return false;
        }
        node = child;
    }

    match node.kind() {
        "string" => is_static_text_string(node, source),
        "concatenated_string" => {
            let mut cursor = node.walk();
            let mut children = node.named_children(&mut cursor);
            let mut saw_string = false;
            children.all(|child| {
                saw_string = true;
                is_static_text_string(child, source)
            }) && saw_string
        }
        _ => false,
    }
}

fn is_static_text_string(node: Node<'_>, source: &str) -> bool {
    if node.kind() != "string" || first_descendant_with_kind(node, "interpolation").is_some() {
        return false;
    }
    let text = node_text(node, source);
    let prefix = text
        .find(['\'', '"'])
        .and_then(|quote| text.get(..quote))
        .unwrap_or(text);
    !prefix
        .bytes()
        .any(|byte| matches!(byte.to_ascii_lowercase(), b'b' | b'f'))
}
