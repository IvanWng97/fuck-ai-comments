use std::path::Path;

use tree_sitter::Node;

use super::tree::{LanguageSpec, analyze, first_descendant_with_kind, node_text};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, Leaf, Span};

#[derive(Clone, Copy)]
struct Rust;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Rust)
}

impl LanguageSpec for Rust {
    fn label(self) -> &'static str {
        "Rust"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn is_function(self, kind: &str) -> bool {
        matches!(kind, "function_item" | "closure_expression")
    }

    fn classify_comment(self, node: Node<'_>, source: &str) -> Option<CommentKind> {
        if !is_comment(node) {
            return None;
        }
        if is_public_rustdoc(node, source) {
            return Some(CommentKind::PublicDocs);
        }
        if is_safety_proof(node, source) {
            return Some(CommentKind::SafetyProof);
        }
        Some(CommentKind::Narrative)
    }

    fn leaf(self, node: Node<'_>, source: &str, _function_depth: usize) -> Option<Leaf> {
        if !matches!(node.kind(), "const_item" | "static_item") {
            return None;
        }
        Some(Leaf {
            span: Span::from_node(node),
            name: node
                .child_by_field_name("name")
                .map_or("<unknown>", |name| node_text(name, source))
                .to_owned(),
        })
    }
}

fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
}

fn is_public_rustdoc(node: Node<'_>, source: &str) -> bool {
    let text = node_text(node, source).trim_start();
    if text.starts_with("//!") || text.starts_with("/*!") {
        return true;
    }
    let outer_doc = (text.starts_with("///") && !text.starts_with("////"))
        || (text.starts_with("/**") && !text.starts_with("/***"));
    if !outer_doc {
        return false;
    }
    rustdoc_target(node, source).is_some_and(|target| {
        has_bare_pub(target, source)
            || ancestors(target)
                .find(|ancestor| ancestor.kind() == "trait_item")
                .is_some_and(|trait_item| has_bare_pub(trait_item, source))
    })
}

fn rustdoc_target<'tree>(mut node: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    loop {
        let next = node.next_named_sibling()?;
        let gap = source.get(node.end_byte()..next.start_byte())?;
        if !gap.bytes().all(|byte| byte.is_ascii_whitespace())
            || gap.bytes().filter(|byte| *byte == b'\n').count() > 1
        {
            return None;
        }
        if is_comment(next) && is_rustdoc_text(node_text(next, source)) {
            node = next;
            continue;
        }
        return Some(next);
    }
}

fn is_rustdoc_text(comment: &str) -> bool {
    let trimmed = comment.trim_start();
    (trimmed.starts_with("///") && !trimmed.starts_with("////"))
        || trimmed.starts_with("//!")
        || (trimmed.starts_with("/**") && !trimmed.starts_with("/***"))
        || trimmed.starts_with("/*!")
}

fn has_bare_pub(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        child.kind() == "visibility_modifier" && node_text(child, source).trim() == "pub"
    })
}

fn is_safety_proof(node: Node<'_>, source: &str) -> bool {
    let first = first_contiguous_comment(node);
    if !comment_body(node_text(first, source)).starts_with("SAFETY:") {
        return false;
    }

    let last = last_contiguous_comment(node);
    let attached_to_unsafe = last.next_named_sibling().is_some_and(|next| {
        next.kind() == "unsafe_block"
            || first_descendant_with_kind(next, "unsafe_block").is_some()
            || node_text(next, source).trim_start().starts_with("unsafe ")
    });
    attached_to_unsafe || ancestors(node).any(|ancestor| ancestor.kind() == "unsafe_block")
}

fn first_contiguous_comment(mut node: Node<'_>) -> Node<'_> {
    while let Some(previous) = node.prev_named_sibling().filter(|previous| {
        is_comment(*previous) && previous.end_position().row + 1 == node.start_position().row
    }) {
        node = previous;
    }
    node
}

fn last_contiguous_comment(mut node: Node<'_>) -> Node<'_> {
    while let Some(next) = node.next_named_sibling().filter(|next| {
        is_comment(*next) && node.end_position().row + 1 == next.start_position().row
    }) {
        node = next;
    }
    node
}

fn comment_body(comment: &str) -> &str {
    comment
        .trim_start()
        .trim_start_matches(['/', '*', '!', ' '])
}

fn ancestors(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    std::iter::successors(node.parent(), |ancestor| ancestor.parent())
}
