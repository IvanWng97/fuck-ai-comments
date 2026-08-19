use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    CallableSubtrees, LanguageSpec, OwnerCandidate, OwnerLocation, analyze, canonical_syntax,
    document, function_name, has_direct_child, node_text,
};
use super::walk::{WalkEvent, events};
use crate::identity::{IdentityArena, IdentityId};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

#[derive(Clone, Copy)]
struct Rust {
    crate_root: bool,
}

#[derive(Default)]
struct RustContext {
    nodes: HashMap<usize, RustNodeContext>,
    function_namespaces: HashMap<usize, Option<IdentityId>>,
    namespaces: IdentityArena,
    containers: HashMap<usize, RustContainerKind>,
    public_local_impls: HashMap<usize, bool>,
    public_outer_docs: HashSet<usize>,
    safety_proof_comments: HashSet<usize>,
    invalid_scope: bool,
}

#[derive(Clone, Copy, Default)]
struct RustNodeContext {
    direct_parent_is_source_file: bool,
    within_callable: bool,
    has_module_ancestor: bool,
    public_module_ancestry: bool,
    nearest_impl: Option<usize>,
    nearest_container: Option<usize>,
    bare_public: bool,
}

#[derive(Clone, Copy)]
enum RustContainerKind {
    PublicMembers,
    ExplicitlyPublicMembers,
}

struct RustTypeDraft {
    node_id: usize,
    scope_id: usize,
    name: String,
}

struct RustImplDraft {
    node_id: usize,
    scope_id: Option<usize>,
    target: Option<String>,
}

struct RustContextFrame {
    node_id: usize,
    is_source_file: bool,
    is_declaration_list: bool,
    namespace_before: Option<IdentityId>,
    callable_pushed: bool,
    module_is_private: bool,
    implementation_pushed: bool,
    container_pushed: bool,
    pending_outer_rustdoc_end: Option<usize>,
    pending_outer_rustdocs: Vec<usize>,
    pending_safety_comments: Vec<usize>,
    pending_safety_end_row: Option<usize>,
    pending_safety_end_byte: usize,
    pending_safety_header: bool,
}

#[derive(Default)]
struct RustDirectChildFacts {
    outer_rustdocs: Vec<usize>,
    safety_proofs: Vec<usize>,
}

#[cfg(test)]
thread_local! {
    static RUST_CONTEXT_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RUST_CONTEXT_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RUSTDOC_TARGET_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RUST_SAFETY_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RUST_NAMESPACE_SEGMENT_STORAGE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_rust_context_work() {
    RUST_CONTEXT_WORK.with(|work| work.set(work.get() + 1));
}

#[cfg(not(test))]
fn record_rust_context_work() {}

#[cfg(test)]
fn reset_rust_context_counts() {
    RUST_CONTEXT_WORK.with(|work| work.set(0));
    RUST_CONTEXT_ENTRIES.with(|entries| entries.set(0));
    RUSTDOC_TARGET_PROBES.with(|probes| probes.set(0));
    RUST_SAFETY_PROBES.with(|probes| probes.set(0));
    RUST_NAMESPACE_SEGMENT_STORAGE.with(|segments| segments.set(0));
}

#[cfg(test)]
fn rust_context_work() -> usize {
    RUST_CONTEXT_WORK.with(std::cell::Cell::get)
}

#[cfg(test)]
fn record_rust_context_entries(entries: usize) {
    RUST_CONTEXT_ENTRIES.with(|count| count.set(entries));
}

#[cfg(not(test))]
fn record_rust_context_entries(_entries: usize) {}

#[cfg(test)]
fn rust_context_entries() -> usize {
    RUST_CONTEXT_ENTRIES.with(std::cell::Cell::get)
}

#[cfg(test)]
fn record_rustdoc_target_probe() {
    RUSTDOC_TARGET_PROBES.with(|probes| probes.set(probes.get() + 1));
}

#[cfg(not(test))]
fn record_rustdoc_target_probe() {}

#[cfg(test)]
fn rustdoc_target_probes() -> usize {
    RUSTDOC_TARGET_PROBES.with(std::cell::Cell::get)
}

#[cfg(test)]
fn record_rust_safety_probe() {
    RUST_SAFETY_PROBES.with(|probes| probes.set(probes.get() + 1));
}

#[cfg(not(test))]
fn record_rust_safety_probe() {}

#[cfg(test)]
fn rust_safety_probes() -> usize {
    RUST_SAFETY_PROBES.with(std::cell::Cell::get)
}

#[cfg(test)]
fn record_rust_namespace_segment_storage(segments: usize) {
    RUST_NAMESPACE_SEGMENT_STORAGE.with(|total| total.set(total.get() + segments));
}

#[cfg(not(test))]
fn record_rust_namespace_segment_storage(_segments: usize) {}

#[cfg(test)]
fn rust_namespace_segment_storage() -> usize {
    RUST_NAMESPACE_SEGMENT_STORAGE.with(std::cell::Cell::get)
}

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

    fn build_context(self, root: Node<'_>, source: &str) -> Result<Self::Context, AnalysisError> {
        rust_context(root, source)
    }

    fn into_identity_arena(self, context: Self::Context) -> IdentityArena {
        context.namespaces
    }

    fn is_owner_prefix(self, kind: &str) -> bool {
        kind == "attribute_item"
    }

    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        context: &Self::Context,
        _function_depth: usize,
        _callable_subtrees: &CallableSubtrees,
    ) -> Result<Option<OwnerCandidate>, AnalysisError> {
        if let Some(span) = rust_function_span(node, location) {
            let namespace = *context.function_namespaces.get(&node.id()).ok_or_else(|| {
                AnalysisError::Invariant("Rust callable has no namespace context entry".to_owned())
            })?;
            return Ok(Some(OwnerCandidate::function_with_identity_parent(
                span,
                function_name(node, location, source),
                namespace,
            )));
        }
        if !matches!(node.kind(), "const_item" | "static_item") {
            return Ok(None);
        }
        Ok(Some(OwnerCandidate::leaf(
            Span::from_node(node),
            node.child_by_field_name("name")
                .map_or("<unknown>", |name| node_text(name, source))
                .to_owned(),
        )))
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
        if context.safety_proof_comments.contains(&node.id()) {
            return Some(CommentKind::SafetyProof);
        }
        Some(CommentKind::Narrative)
    }
}

fn rust_function_span(node: Node<'_>, location: OwnerLocation<'_>) -> Option<Span> {
    if !matches!(node.kind(), "function_item" | "closure_expression") {
        return None;
    }
    let start = location.leading_prefix().unwrap_or(node);
    let mut span = Span::from_node(node);
    span.start_byte = start.start_byte();
    span.start_line = start.start_position().row + 1;
    Some(span)
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
        return is_public_inner_doc(node, crate_root, context);
    }
    let outer_doc = (text.starts_with("///") && !text.starts_with("////"))
        || (text.starts_with("/**") && !text.starts_with("/***"));
    if !outer_doc {
        return false;
    }
    context.public_outer_docs.contains(&node.id())
}

fn is_public_inner_doc(node: Node<'_>, crate_root: bool, context: &RustContext) -> bool {
    if context.invalid_scope {
        return false;
    }
    let Some(node_context) = context.nodes.get(&node.id()) else {
        return false;
    };
    if node_context.direct_parent_is_source_file {
        return crate_root;
    }
    !node_context.within_callable
        && node_context.has_module_ancestor
        && node_context.public_module_ancestry
}

fn is_reachable_target(target: usize, context: &RustContext) -> bool {
    if context.invalid_scope {
        return false;
    }
    let Some(target_context) = context.nodes.get(&target) else {
        return false;
    };
    if !target_context.public_module_ancestry || target_context.within_callable {
        return false;
    }
    if let Some(implementation) = target_context.nearest_impl {
        return target_context.bare_public
            && context
                .public_local_impls
                .get(&implementation)
                .copied()
                .unwrap_or(false);
    }
    if let Some(container) = target_context.nearest_container {
        let container_public = context
            .nodes
            .get(&container)
            .is_some_and(|container| container.bare_public);
        let members_are_public = context
            .containers
            .get(&container)
            .is_some_and(|kind| matches!(kind, RustContainerKind::PublicMembers));
        return container_public && (members_are_public || target_context.bare_public);
    }
    target_context.bare_public
}

fn rust_context(root: Node<'_>, source: &str) -> Result<RustContext, AnalysisError> {
    let mut context = RustContext::default();
    let mut frames: Vec<RustContextFrame> = Vec::new();
    let mut namespace = None;
    let mut callable_depth = 0_usize;
    let mut module_depth = 0_usize;
    let mut private_module_depth = 0_usize;
    let mut implementations = Vec::new();
    let mut containers = Vec::new();
    let mut type_drafts = Vec::new();
    let mut impl_drafts = Vec::new();
    let mut outer_doc_targets = Vec::new();

    for event in events(root) {
        record_rust_context_work();
        match event {
            WalkEvent::Enter(node) => {
                let child_facts = frames
                    .last_mut()
                    .map(|frame| frame.record_direct_child(node, source))
                    .unwrap_or_default();
                let outer_rustdocs = child_facts.outer_rustdocs;
                context
                    .safety_proof_comments
                    .extend(child_facts.safety_proofs);
                record_bare_public_visibility(
                    node,
                    source,
                    &mut frames,
                    &mut context,
                    &mut private_module_depth,
                );
                let parent = frames.last();
                let node_context = RustNodeContext {
                    direct_parent_is_source_file: parent.is_some_and(|frame| frame.is_source_file),
                    within_callable: callable_depth > 0,
                    has_module_ancestor: module_depth > 0,
                    public_module_ancestry: private_module_depth == 0,
                    nearest_impl: implementations.last().copied(),
                    nearest_container: containers.last().copied(),
                    bare_public: false,
                };
                if !outer_rustdocs.is_empty()
                    || is_inner_rustdoc(node, source)
                    || matches!(
                        node.kind(),
                        "struct_item" | "enum_item" | "union_item" | "trait_item"
                    )
                {
                    context.nodes.insert(node.id(), node_context);
                }
                outer_doc_targets.extend(
                    outer_rustdocs
                        .into_iter()
                        .map(|comment| (comment, node.id())),
                );

                if matches!(node.kind(), "function_item" | "closure_expression") {
                    context.function_namespaces.insert(node.id(), namespace);
                }
                if matches!(node.kind(), "struct_item" | "enum_item" | "union_item")
                    && let Some(scope) = parent.filter(|frame| frame.is_declaration_scope())
                    && let Some(name) = node.child_by_field_name("name")
                {
                    type_drafts.push(RustTypeDraft {
                        node_id: node.id(),
                        scope_id: scope.node_id,
                        name: node_text(name, source).to_owned(),
                    });
                }
                if node.kind() == "impl_item" {
                    let target = node
                        .child_by_field_name("trait")
                        .is_none()
                        .then(|| node.child_by_field_name("type"))
                        .flatten()
                        .filter(|target| target.kind() == "type_identifier")
                        .map(|target| node_text(target, source).to_owned());
                    impl_drafts.push(RustImplDraft {
                        node_id: node.id(),
                        scope_id: parent
                            .filter(|frame| frame.is_declaration_scope())
                            .map(|frame| frame.node_id),
                        target,
                    });
                }

                let namespace_before = namespace;
                if let Some(segment) = rust_namespace_segment(node, source) {
                    namespace = Some(context.namespaces.push(namespace, segment)?);
                }
                let callable_pushed = matches!(node.kind(), "function_item" | "closure_expression");
                callable_depth += usize::from(callable_pushed);
                let module_is_private = node.kind() == "mod_item";
                module_depth += usize::from(module_is_private);
                private_module_depth += usize::from(module_is_private);
                let implementation_pushed = node.kind() == "impl_item";
                if implementation_pushed {
                    implementations.push(node.id());
                }
                let container_pushed = rust_container_kind(node).is_some_and(|kind| {
                    context.containers.insert(node.id(), kind);
                    containers.push(node.id());
                    true
                });
                frames.push(RustContextFrame {
                    node_id: node.id(),
                    is_source_file: node.kind() == "source_file",
                    is_declaration_list: node.kind() == "declaration_list",
                    namespace_before,
                    callable_pushed,
                    module_is_private,
                    implementation_pushed,
                    container_pushed,
                    pending_outer_rustdoc_end: None,
                    pending_outer_rustdocs: Vec::new(),
                    pending_safety_comments: Vec::new(),
                    pending_safety_end_row: None,
                    pending_safety_end_byte: 0,
                    pending_safety_header: false,
                });
            }
            WalkEvent::Leave(node) => {
                let frame = frames.pop();
                debug_assert_eq!(frame.as_ref().map(|frame| frame.node_id), Some(node.id()));
                let Some(frame) = frame else {
                    continue;
                };
                namespace = frame.namespace_before;
                callable_depth -= usize::from(frame.callable_pushed);
                if node.kind() == "mod_item" {
                    module_depth -= 1;
                    private_module_depth -= usize::from(frame.module_is_private);
                }
                if frame.implementation_pushed {
                    pop_scope(&mut implementations, node.id(), "Rust implementation")?;
                }
                if frame.container_pushed {
                    pop_scope(&mut containers, node.id(), "Rust container")?;
                }
            }
        }
    }
    debug_assert!(frames.is_empty());

    let mut public_types: HashMap<usize, HashMap<String, Option<bool>>> = HashMap::new();
    for draft in type_drafts {
        let public = context.nodes.get(&draft.node_id).is_some_and(|node| {
            node.bare_public && node.public_module_ancestry && !node.within_callable
        });
        public_types
            .entry(draft.scope_id)
            .or_default()
            .entry(draft.name)
            .and_modify(|visibility| *visibility = None)
            .or_insert(Some(public));
    }
    for draft in impl_drafts {
        let public = draft
            .scope_id
            .zip(draft.target.as_deref())
            .and_then(|(scope, target)| public_types.get(&scope)?.get(target))
            .is_some_and(|visibility| *visibility == Some(true));
        context.public_local_impls.insert(draft.node_id, public);
    }
    for (comment, target) in outer_doc_targets {
        record_rustdoc_target_probe();
        if is_reachable_target(target, &context) {
            context.public_outer_docs.insert(comment);
        }
    }
    record_rust_context_entries(
        context.nodes.len()
            + context.function_namespaces.len()
            + context.namespaces.len()
            + context.containers.len()
            + context.public_local_impls.len()
            + context.public_outer_docs.len()
            + context.safety_proof_comments.len(),
    );
    record_rust_namespace_segment_storage(context.namespaces.len());
    Ok(context)
}

fn pop_scope(
    scopes: &mut Vec<usize>,
    expected: usize,
    label: &'static str,
) -> Result<(), AnalysisError> {
    let actual = scopes.pop();
    debug_assert_eq!(actual, Some(expected));
    if actual != Some(expected) {
        return Err(AnalysisError::Invariant(format!(
            "{label} scopes closed out of order"
        )));
    }
    Ok(())
}

fn record_bare_public_visibility(
    node: Node<'_>,
    source: &str,
    frames: &mut [RustContextFrame],
    context: &mut RustContext,
    private_module_depth: &mut usize,
) {
    if node.kind() != "visibility_modifier" || node_text(node, source).trim() != "pub" {
        return;
    }
    let Some(parent) = frames.last_mut() else {
        return;
    };
    if let Some(parent_context) = context.nodes.get_mut(&parent.node_id) {
        parent_context.bare_public = true;
    }
    if parent.module_is_private {
        parent.module_is_private = false;
        if let Some(depth) = private_module_depth.checked_sub(1) {
            *private_module_depth = depth;
        } else {
            debug_assert!(false, "private Rust module scope must be open");
            context.invalid_scope = true;
        }
    }
}

impl RustContextFrame {
    fn is_declaration_scope(&self) -> bool {
        self.is_source_file || self.is_declaration_list
    }

    fn record_direct_child(&mut self, node: Node<'_>, source: &str) -> RustDirectChildFacts {
        if !node.is_named() {
            return RustDirectChildFacts::default();
        }
        let safety_proofs = self.record_safety_child(node, source);
        let adjacent = self.pending_outer_rustdoc_end.is_some_and(|end| {
            source.get(end..node.start_byte()).is_some_and(|gap| {
                gap.bytes().all(|byte| byte.is_ascii_whitespace())
                    && gap.bytes().filter(|byte| *byte == b'\n').count() <= 1
            })
        });
        if is_outer_rustdoc(node, source) {
            if !adjacent {
                self.pending_outer_rustdocs.clear();
            }
            self.pending_outer_rustdocs.push(node.id());
            self.pending_outer_rustdoc_end = Some(node.end_byte());
            return RustDirectChildFacts {
                outer_rustdocs: Vec::new(),
                safety_proofs,
            };
        }
        if adjacent && (node.kind() == "attribute_item" || is_inner_rustdoc(node, source)) {
            self.pending_outer_rustdoc_end = Some(node.end_byte());
            return RustDirectChildFacts {
                outer_rustdocs: Vec::new(),
                safety_proofs,
            };
        }
        self.pending_outer_rustdoc_end = None;
        let outer_rustdocs = if adjacent {
            std::mem::take(&mut self.pending_outer_rustdocs)
        } else {
            self.pending_outer_rustdocs.clear();
            Vec::new()
        };
        RustDirectChildFacts {
            outer_rustdocs,
            safety_proofs,
        }
    }

    fn record_safety_child(&mut self, node: Node<'_>, source: &str) -> Vec<usize> {
        if is_comment(node) {
            record_rust_safety_probe();
            let continues_block = self
                .pending_safety_end_row
                .and_then(|row| row.checked_add(1))
                == Some(node.start_position().row);
            if !continues_block {
                self.pending_safety_comments.clear();
                self.pending_safety_header = comment_body(node_text(node, source))
                    .strip_prefix("SAFETY:")
                    .is_some_and(|proof| !proof.trim().is_empty());
            }
            self.pending_safety_comments.push(node.id());
            self.pending_safety_end_row = Some(node.end_position().row);
            self.pending_safety_end_byte = node.end_byte();
            return Vec::new();
        }

        let attached = self.pending_safety_header
            && source
                .get(self.pending_safety_end_byte..node.start_byte())
                .is_some_and(|gap| {
                    gap.bytes().all(|byte| byte.is_ascii_whitespace())
                        && gap.bytes().filter(|byte| *byte == b'\n').count() <= 1
                })
            && is_unsafe_construct(attached_expression(node));
        self.pending_safety_end_row = None;
        self.pending_safety_header = false;
        if attached {
            std::mem::take(&mut self.pending_safety_comments)
        } else {
            self.pending_safety_comments.clear();
            Vec::new()
        }
    }
}

fn is_outer_rustdoc(node: Node<'_>, source: &str) -> bool {
    if !is_comment(node) {
        return false;
    }
    let text = node_text(node, source).trim_start();
    (text.starts_with("///") && !text.starts_with("////"))
        || (text.starts_with("/**") && !text.starts_with("/***"))
}

fn is_inner_rustdoc(node: Node<'_>, source: &str) -> bool {
    if !is_comment(node) {
        return false;
    }
    let text = node_text(node, source).trim_start();
    text.starts_with("//!") || text.starts_with("/*!")
}

fn rust_container_kind(node: Node<'_>) -> Option<RustContainerKind> {
    match node.kind() {
        "trait_item" | "enum_item" => Some(RustContainerKind::PublicMembers),
        "struct_item" | "union_item" => Some(RustContainerKind::ExplicitlyPublicMembers),
        _ => None,
    }
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
    has_direct_child(node, "unsafe")
}

fn comment_body(comment: &str) -> &str {
    comment
        .trim_start()
        .trim_start_matches(['/', '*', '!', ' '])
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;

    use crate::identity::{identity_work, reset_identity_work};
    use crate::{SourceFile, analyze_all, analyze_change};

    use super::{
        reset_rust_context_counts, rust_context_entries, rust_context_work,
        rust_namespace_segment_storage, rust_safety_probes, rustdoc_target_probes,
    };

    #[test]
    fn rust_function_namespace_context_work_is_linear_in_nested_functions() {
        for depth in [16, 32, 64] {
            let source = nested_functions(depth);
            reset_rust_context_counts();

            let findings = analyze_all(SourceFile {
                path: Path::new("src/lib.rs"),
                text: &source,
            })
            .expect("generated nested Rust functions must parse");

            assert!(findings.is_empty(), "comment-free source stays clean");
            assert!(
                rust_context_work() <= 40 * depth,
                "Rust context work must stay linear at depth {depth}"
            );
        }
    }

    #[test]
    fn rust_public_module_namespaces_store_each_segment_once() {
        const DEPTH: usize = 4_096;
        let source = nested_public_modules(DEPTH);
        reset_rust_context_counts();

        let findings = analyze_all(SourceFile {
            path: Path::new("src/lib.rs"),
            text: &source,
        })
        .expect("generated nested public Rust modules must parse");

        assert!(findings.is_empty(), "comment-free source stays clean");
        let stored = rust_namespace_segment_storage();
        assert_eq!(stored, DEPTH, "each module segment must be stored once");
    }

    #[test]
    fn rust_change_identity_storage_is_linear() {
        for depth in [4_096, 2_048, 1_024] {
            let before = nested_public_modules(depth);
            let after = before.replacen("{}", "{ changed(); }", 1);
            reset_rust_context_counts();
            reset_identity_work();

            let findings = analyze_change(
                SourceFile {
                    path: Path::new("src/lib.rs"),
                    text: &before,
                },
                SourceFile {
                    path: Path::new("src/lib.rs"),
                    text: &after,
                },
            )
            .expect("nested public Rust modules must pair after one body change");

            assert!(findings.is_empty(), "comment-free changes stay clean");
            assert_eq!(
                rust_namespace_segment_storage(),
                3 * depth,
                "each analysis stores one namespace segment per module"
            );
            let (arena_nodes, canonical_visits, canonical_nodes, canonical_hash_probes) =
                identity_work();
            assert_eq!(arena_nodes, 5 * depth + 2);
            assert_eq!(canonical_visits, 4 * depth + 2);
            assert_eq!(canonical_nodes, 2 * depth + 1);
            assert_eq!(canonical_hash_probes, canonical_visits);
        }
    }

    #[test]
    fn rust_context_only_indexes_semantic_nodes_in_token_dense_functions() {
        let mut source = String::from("fn calculate() {\n");
        for statement in 0..1_024 {
            writeln!(source, "let value_{statement:04} = {statement};")
                .expect("writing to a String cannot fail");
        }
        source.push_str("}\n");
        reset_rust_context_counts();

        let findings = analyze_all(SourceFile {
            path: Path::new("src/lib.rs"),
            text: &source,
        })
        .expect("generated token-dense Rust function must parse");

        assert!(findings.is_empty(), "comment-free source stays clean");
        assert_eq!(
            rust_context_entries(),
            1,
            "only the function namespace belongs in Context"
        );
    }

    #[test]
    fn rustdoc_target_resolution_work_is_linear_in_contiguous_doc_blocks() {
        for comment_count in [64, 128, 256] {
            let mut source = "/// Public contract detail.\n".repeat(comment_count);
            source.push_str("pub fn operation() {}\n");
            reset_rust_context_counts();

            let findings = analyze_all(SourceFile {
                path: Path::new("src/lib.rs"),
                text: &source,
            })
            .expect("generated public Rust docs must parse");

            assert!(findings.is_empty(), "public Rust docs stay exempt");
            assert_eq!(
                rustdoc_target_probes(),
                comment_count,
                "each Rustdoc comment is resolved exactly once"
            );
        }
    }

    #[test]
    fn safety_proof_resolution_work_is_linear_in_contiguous_comment_blocks() {
        for comment_count in [64, 128, 256] {
            let mut source = String::from("fn operation(pointer: *const u8) {\n");
            source.push_str("// SAFETY: the caller keeps the pointer readable.\n");
            source.push_str(
                &"// This line records one supporting invariant.\n".repeat(comment_count - 1),
            );
            source.push_str("unsafe { core::ptr::read(pointer); }\n}\n");
            reset_rust_context_counts();

            let findings = analyze_all(SourceFile {
                path: Path::new("src/lib.rs"),
                text: &source,
            })
            .expect("generated Rust safety proof must parse");

            assert_eq!(
                findings
                    .iter()
                    .map(|finding| finding.rule)
                    .collect::<Vec<_>>(),
                ["comment-policy/owner-comment-cap"],
                "attached proof bypasses narrative budgets but not the absolute owner cap"
            );
            assert_eq!(
                rust_safety_probes(),
                comment_count,
                "each potential safety comment is inspected exactly once"
            );
        }
    }

    #[test]
    fn safety_proof_requires_an_adjacent_unsafe_target() {
        for target in [
            "\nunsafe { core::ptr::read(pointer); }",
            "safe_operation();",
        ] {
            let source = format!(
                concat!(
                    "fn operation(pointer: *const u8) {{\n",
                    "// SAFETY: the caller keeps the pointer readable.\n",
                    "// The allocation remains live.\n",
                    "// The pointer is aligned.\n",
                    "// Reading one byte stays in bounds.\n",
                    "{}\n",
                    "}}\n",
                ),
                target,
            );

            let findings = analyze_all(SourceFile {
                path: Path::new("src/lib.rs"),
                text: &source,
            })
            .expect("generated negative Rust safety case must parse");

            assert!(
                findings
                    .iter()
                    .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
                "detached or safe targets must not exempt narrative comments: {findings:#?}"
            );
        }
    }

    fn nested_functions(depth: usize) -> String {
        let mut source = String::new();
        for function in 0..depth {
            writeln!(source, "fn operation_{function:03}() {{")
                .expect("writing to a String cannot fail");
        }
        source.push_str("run();\n");
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source
    }

    fn nested_public_modules(depth: usize) -> String {
        let mut source = String::new();
        for module in 0..depth {
            writeln!(source, "pub mod namespace_{module:04} {{")
                .expect("writing to a String cannot fail");
            writeln!(source, "pub fn operation_{module:04}() {{}}")
                .expect("writing to a String cannot fail");
        }
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source
    }
}
