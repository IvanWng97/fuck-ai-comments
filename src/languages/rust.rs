use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    CallableSubtrees, LanguageSpec, OwnerCandidate, OwnerLocation, analyze, ancestors,
    canonical_syntax, document, function_name, node_text,
};
use super::walk::{WalkEvent, events};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

#[derive(Clone, Copy)]
struct Rust {
    crate_root: bool,
}

type RustContext = HashMap<usize, HashMap<String, Option<bool>>>;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Rust::for_path(path))
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, Rust::for_path(path))
}

impl Rust {
    fn for_path(path: &Path) -> Self {
        Self {
            crate_root: path.file_name().is_some_and(|name| name == "lib.rs"),
        }
    }
}

impl LanguageSpec for Rust {
    type Context = RustContext;

    fn label(self) -> &'static str {
        "Rust"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn build_context(self, root: Node<'_>, source: &str) -> Self::Context {
        public_type_context(root, source)
    }

    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        _function_depth: usize,
        _callable_subtrees: &CallableSubtrees,
    ) -> Option<OwnerCandidate> {
        if let Some(span) = rust_function_span(node, source) {
            return Some(OwnerCandidate::function(
                span,
                function_name(node, location, source),
                rust_function_namespace(node, source),
            ));
        }
        if !matches!(node.kind(), "const_item" | "static_item") {
            return None;
        }
        Some(OwnerCandidate::leaf(
            Span::from_node(node),
            node.child_by_field_name("name")
                .map_or("<unknown>", |name| node_text(name, source))
                .to_owned(),
        ))
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        context: &Self::Context,
    ) -> Option<CommentKind> {
        if !is_comment(node) {
            return None;
        }
        if is_public_rustdoc(node, source, self.crate_root, context) {
            return Some(CommentKind::PublicDocs);
        }
        if is_safety_proof(node, source) {
            return Some(CommentKind::SafetyProof);
        }
        Some(CommentKind::Narrative)
    }
}

fn rust_function_span(node: Node<'_>, source: &str) -> Option<Span> {
    if !matches!(node.kind(), "function_item" | "closure_expression") {
        return None;
    }
    let mut start = node;
    while let Some(attribute) = start
        .prev_named_sibling()
        .filter(|sibling| sibling.kind() == "attribute_item")
    {
        let gap = source.get(attribute.end_byte()..start.start_byte())?;
        if !gap.bytes().all(|byte| byte.is_ascii_whitespace()) {
            break;
        }
        start = attribute;
    }
    let mut span = Span::from_node(node);
    span.start_byte = start.start_byte();
    span.start_line = start.start_position().row + 1;
    Some(span)
}

fn rust_function_namespace(node: Node<'_>, source: &str) -> Vec<String> {
    let mut namespace: Vec<_> = ancestors(node)
        .filter_map(|ancestor| rust_namespace_segment(ancestor, source))
        .collect();
    namespace.reverse();
    namespace
}

fn rust_namespace_segment(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "impl_item" => {
            let target = node
                .child_by_field_name("type")
                .map(|target| canonical_syntax(target, source))?;
            let trait_name = node
                .child_by_field_name("trait")
                .map(|item| canonical_syntax(item, source));
            Some(trait_name.map_or_else(
                || format!("impl:{target}"),
                |trait_name| format!("impl:{trait_name} for {target}"),
            ))
        }
        "trait_item" => node
            .child_by_field_name("name")
            .map(|name| format!("trait:{}", canonical_syntax(name, source))),
        "mod_item" => node
            .child_by_field_name("name")
            .map(|name| format!("mod:{}", node_text(name, source))),
        _ => None,
    }
}

fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
}

fn is_public_rustdoc(
    node: Node<'_>,
    source: &str,
    crate_root: bool,
    context: &RustContext,
) -> bool {
    let text = node_text(node, source).trim_start();
    if text.starts_with("//!") || text.starts_with("/*!") {
        return is_public_inner_doc(node, source, crate_root);
    }
    let outer_doc = (text.starts_with("///") && !text.starts_with("////"))
        || (text.starts_with("/**") && !text.starts_with("/***"));
    if !outer_doc {
        return false;
    }
    rustdoc_target(node, source).is_some_and(|target| is_reachable_target(target, source, context))
}

fn is_public_inner_doc(node: Node<'_>, source: &str, crate_root: bool) -> bool {
    if node
        .parent()
        .is_some_and(|parent| parent.kind() == "source_file")
    {
        return crate_root;
    }
    if ancestors(node)
        .any(|ancestor| matches!(ancestor.kind(), "function_item" | "closure_expression"))
    {
        return false;
    }
    let Some(module) = ancestors(node).find(|ancestor| ancestor.kind() == "mod_item") else {
        return false;
    };
    has_bare_pub(module, source) && has_public_module_ancestry(module, source)
}

fn is_reachable_target(target: Node<'_>, source: &str, context: &RustContext) -> bool {
    if !has_public_module_ancestry(target, source) {
        return false;
    }
    if ancestors(target)
        .any(|ancestor| matches!(ancestor.kind(), "function_item" | "closure_expression"))
    {
        return false;
    }
    if let Some(implementation) = ancestors(target).find(|ancestor| ancestor.kind() == "impl_item")
    {
        return has_bare_pub(target, source)
            && local_impl_type_is_public(implementation, source, context);
    }
    if let Some(container) = ancestors(target).find(|ancestor| {
        matches!(
            ancestor.kind(),
            "trait_item" | "enum_item" | "struct_item" | "union_item"
        )
    }) {
        return has_bare_pub(container, source)
            && (matches!(container.kind(), "trait_item" | "enum_item")
                || has_bare_pub(target, source));
    }
    has_bare_pub(target, source)
}

fn has_public_module_ancestry(node: Node<'_>, source: &str) -> bool {
    ancestors(node)
        .filter(|ancestor| ancestor.kind() == "mod_item")
        .all(|module| has_bare_pub(module, source))
}

fn local_impl_type_is_public(
    implementation: Node<'_>,
    source: &str,
    context: &RustContext,
) -> bool {
    if implementation.child_by_field_name("trait").is_some() {
        return false;
    }
    let Some(target) = implementation
        .child_by_field_name("type")
        .filter(|target| target.kind() == "type_identifier")
        .map(|target| node_text(target, source))
    else {
        return false;
    };
    let Some(scope) = implementation
        .parent()
        .filter(|parent| matches!(parent.kind(), "source_file" | "declaration_list"))
    else {
        return false;
    };
    context
        .get(&scope.id())
        .and_then(|types| types.get(target))
        .is_some_and(|visibility| *visibility == Some(true))
}

fn public_type_context(root: Node<'_>, source: &str) -> RustContext {
    let mut context: RustContext = HashMap::new();
    for event in events(root) {
        let WalkEvent::Enter(declaration) = event else {
            continue;
        };
        if !matches!(
            declaration.kind(),
            "struct_item" | "enum_item" | "union_item"
        ) {
            continue;
        }
        let Some(scope) = declaration
            .parent()
            .filter(|parent| matches!(parent.kind(), "source_file" | "declaration_list"))
        else {
            continue;
        };
        let Some(name) = declaration.child_by_field_name("name") else {
            continue;
        };
        let public =
            has_bare_pub(declaration, source) && has_public_module_ancestry(declaration, source);
        let declarations = context.entry(scope.id()).or_default();
        declarations
            .entry(node_text(name, source).to_owned())
            .and_modify(|visibility| *visibility = None)
            .or_insert(Some(public));
    }
    context
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
        if (is_comment(next) && is_rustdoc_text(node_text(next, source)))
            || next.kind() == "attribute_item"
        {
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
    let Some(proof) = comment_body(node_text(first, source)).strip_prefix("SAFETY:") else {
        return false;
    };
    if proof.trim().is_empty() {
        return false;
    }

    let last = last_contiguous_comment(node);
    let Some(next) = last.next_named_sibling() else {
        return false;
    };
    if !is_adjacent(last, next, source) {
        return false;
    }
    is_unsafe_construct(attached_expression(next))
}

fn attached_expression(node: Node<'_>) -> Node<'_> {
    if node.kind() == "expression_statement" {
        return node.named_child(0).unwrap_or(node);
    }
    node
}

fn is_unsafe_construct(node: Node<'_>) -> bool {
    if node.kind() == "unsafe_block" {
        return true;
    }
    if !matches!(
        node.kind(),
        "foreign_mod_item"
            | "function_item"
            | "function_signature_item"
            | "impl_item"
            | "trait_item"
    ) {
        return false;
    }

    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        child.kind() == "unsafe"
            || (child.kind() == "function_modifiers" && has_unsafe_keyword(child))
    })
}

fn has_unsafe_keyword(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == "unsafe")
}

fn is_adjacent(left: Node<'_>, right: Node<'_>, source: &str) -> bool {
    source
        .get(left.end_byte()..right.start_byte())
        .is_some_and(|gap| {
            gap.bytes().all(|byte| byte.is_ascii_whitespace())
                && gap.bytes().filter(|byte| *byte == b'\n').count() <= 1
        })
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
