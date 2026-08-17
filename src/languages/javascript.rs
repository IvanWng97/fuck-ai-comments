use std::path::Path;

use tree_sitter::Node;

use super::tree::{LanguageSpec, analyze, document, first_descendant_with_kind, node_text};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, Leaf, ParsedFile, Span};

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
    type Context = ();

    fn label(self) -> &'static str {
        "JavaScript"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_javascript::LANGUAGE.into()
    }

    fn function_span(self, node: Node<'_>, _source: &str) -> Option<Span> {
        is_function_kind(node.kind()).then(|| Span::from_node(node))
    }

    fn function_namespace(self, node: Node<'_>, source: &str) -> Vec<String> {
        function_namespace(node, source)
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        _context: &Self::Context,
    ) -> Option<CommentKind> {
        classify_comment(node, source)
    }

    fn leaf(self, node: Node<'_>, source: &str, _function_depth: usize) -> Option<Leaf> {
        leaf_from_node(node, source)
    }
}

pub(crate) fn function_namespace(node: Node<'_>, source: &str) -> Vec<String> {
    let mut namespace: Vec<_> = std::iter::successors(node.parent(), |ancestor| ancestor.parent())
        .filter(|ancestor| {
            matches!(
                ancestor.kind(),
                "class" | "class_declaration" | "abstract_class_declaration"
            )
        })
        .map(|class| {
            class.child_by_field_name("name").map_or_else(
                || "class:<anonymous>".to_owned(),
                |name| format!("class:{}", node_text(name, source)),
            )
        })
        .collect();
    namespace.reverse();
    namespace
}

pub(crate) fn classify_comment(node: Node<'_>, source: &str) -> Option<CommentKind> {
    if node.kind() != "comment" {
        return None;
    }
    let kind = if node.start_position().row == node.end_position().row
        && tool_directive(node_text(node, source))
            .is_some_and(|placement| directive_is_attached(node, placement))
    {
        CommentKind::ToolDirective
    } else {
        CommentKind::Narrative
    };
    Some(kind)
}

#[derive(Clone, Copy)]
enum DirectivePlacement {
    NextLine,
    SameLine,
    FilePreamble,
    FreeStanding,
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

fn directive_is_attached(node: Node<'_>, placement: DirectivePlacement) -> bool {
    match placement {
        DirectivePlacement::NextLine => node.next_named_sibling().is_some_and(|next| {
            next.kind() != "comment"
                && next.start_position().row == node.end_position().row.saturating_add(1)
        }),
        DirectivePlacement::SameLine => node.prev_named_sibling().is_some_and(|previous| {
            previous.kind() != "comment" && previous.end_position().row == node.start_position().row
        }),
        DirectivePlacement::FilePreamble => {
            node.parent()
                .is_some_and(|parent| parent.kind() == "program")
                && std::iter::successors(node.prev_named_sibling(), |sibling| {
                    sibling.prev_named_sibling()
                })
                .all(|sibling| matches!(sibling.kind(), "comment" | "hash_bang_line"))
        }
        DirectivePlacement::FreeStanding => true,
    }
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
    Some(Leaf {
        span: Span::from_node(node),
        name: name.to_owned(),
    })
}
