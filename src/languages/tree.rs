use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{
    CodeToken, Comment, CommentKind, Function, Leaf, ParsedFile, Span, TreeInput, TreeOwner,
    TreeOwnership, TypeOwner, tree_document, tree_findings,
};

use super::walk::{WalkEvent, events};

pub(crate) const ANONYMOUS_FUNCTION_NAME: &str = "<anonymous>";

#[derive(Default)]
pub(crate) struct CallableSubtrees {
    nodes_with_callables: HashSet<usize>,
    callable_kind: Option<fn(&str) -> bool>,
}

struct CallableSubtreeFrame {
    node_id: usize,
    subtree_has_callable: bool,
}

impl CallableSubtrees {
    fn from_root(root: Node<'_>, callable_kind: Option<fn(&str) -> bool>) -> Self {
        let Some(callable_kind) = callable_kind else {
            return Self::default();
        };
        let mut nodes_with_callables = HashSet::new();
        let mut frames: Vec<CallableSubtreeFrame> = Vec::new();
        for event in events(root) {
            match event {
                WalkEvent::Enter(node) => {
                    record_callable_subtree_visit();
                    let is_callable = callable_kind(node.kind());
                    frames.push(CallableSubtreeFrame {
                        node_id: node.id(),
                        subtree_has_callable: is_callable,
                    });
                }
                WalkEvent::Leave(node) => {
                    let frame = frames.pop();
                    debug_assert_eq!(frame.as_ref().map(|frame| frame.node_id), Some(node.id()));
                    let Some(frame) = frame else {
                        continue;
                    };
                    if frame.subtree_has_callable {
                        nodes_with_callables.insert(frame.node_id);
                        if let Some(parent) = frames.last_mut() {
                            parent.subtree_has_callable = true;
                        }
                    }
                }
            }
        }
        debug_assert!(frames.is_empty());
        Self {
            nodes_with_callables,
            callable_kind: Some(callable_kind),
        }
    }

    pub(crate) fn contains_callable(&self, node: Node<'_>) -> bool {
        self.nodes_with_callables.contains(&node.id())
    }

    fn is_callable(&self, node: Node<'_>) -> bool {
        self.callable_kind
            .is_some_and(|callable_kind| callable_kind(node.kind()))
    }
}

#[cfg(test)]
thread_local! {
    static CALLABLE_SUBTREE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CALLABLE_CANDIDATE_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CALLABLE_FRONTIER_REGISTRATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_callable_subtree_visit() {
    CALLABLE_SUBTREE_VISITS.with(|visits| visits.set(visits.get() + 1));
}

#[cfg(not(test))]
fn record_callable_subtree_visit() {}

#[cfg(test)]
fn record_callable_candidate_check() {
    CALLABLE_CANDIDATE_CHECKS.with(|candidates| candidates.set(candidates.get() + 1));
}

#[cfg(not(test))]
fn record_callable_candidate_check() {}

#[cfg(test)]
fn record_callable_frontier_registration(count: usize) {
    CALLABLE_FRONTIER_REGISTRATIONS.with(|registrations| {
        registrations.set(registrations.get() + count);
    });
}

#[cfg(not(test))]
fn record_callable_frontier_registration(_count: usize) {}

#[cfg(test)]
pub(crate) fn reset_callable_frontier_counts() {
    CALLABLE_SUBTREE_VISITS.with(|visits| visits.set(0));
    CALLABLE_CANDIDATE_CHECKS.with(|candidates| candidates.set(0));
    CALLABLE_FRONTIER_REGISTRATIONS.with(|registrations| registrations.set(0));
}

#[cfg(test)]
pub(crate) fn callable_frontier_counts() -> (usize, usize, usize) {
    (
        CALLABLE_SUBTREE_VISITS.with(std::cell::Cell::get),
        CALLABLE_CANDIDATE_CHECKS.with(std::cell::Cell::get),
        CALLABLE_FRONTIER_REGISTRATIONS.with(std::cell::Cell::get),
    )
}

#[derive(Clone, Copy)]
pub(crate) enum DirectivePlacement {
    NextLine,
    NextNode,
    SameLine,
    PreviousLine,
    FilePreamble,
    FreeStanding,
}

#[derive(Default)]
pub(crate) struct AttachmentIndex {
    comments: HashMap<usize, CommentAttachment>,
}

#[derive(Clone, Copy)]
pub(crate) struct AttachmentSyntax {
    transparent_comment_wrapper: fn(&str) -> bool,
    preamble_trivia: fn(&str) -> bool,
}

impl AttachmentSyntax {
    pub(crate) const fn new(
        transparent_comment_wrapper: fn(&str) -> bool,
        preamble_trivia: fn(&str) -> bool,
    ) -> Self {
        Self {
            transparent_comment_wrapper,
            preamble_trivia,
        }
    }
}

impl Default for AttachmentSyntax {
    fn default() -> Self {
        Self::new(|_| false, |_| false)
    }
}

impl AttachmentIndex {
    pub(crate) fn from_root(root: Node<'_>, source: &str) -> Self {
        Self::with_syntax(root, source, AttachmentSyntax::default())
    }

    pub(crate) fn with_syntax(root: Node<'_>, source: &str, syntax: AttachmentSyntax) -> Self {
        let mut builder = AttachmentIndexBuilder::new(source, syntax);
        for event in events(root) {
            match event {
                WalkEvent::Enter(node) => builder.enter(node),
                WalkEvent::Leave(node) => builder.leave(node),
            }
        }
        Self {
            comments: builder.comments,
        }
    }

    pub(crate) fn is_attached(&self, node: Node<'_>, placement: DirectivePlacement) -> bool {
        if matches!(placement, DirectivePlacement::FreeStanding) {
            return true;
        }
        let Some(attachment) = self.comments.get(&node.id()) else {
            return false;
        };
        match placement {
            DirectivePlacement::NextLine => attachment.next_line,
            DirectivePlacement::NextNode => attachment.next_node,
            DirectivePlacement::SameLine => attachment.same_line,
            DirectivePlacement::PreviousLine => attachment.previous_line,
            DirectivePlacement::FilePreamble => attachment.file_preamble,
            DirectivePlacement::FreeStanding => true,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct CommentAttachment {
    starts_line: bool,
    end_row: usize,
    next_line: bool,
    next_node: bool,
    same_line: bool,
    previous_line: bool,
    file_preamble: bool,
}

struct AttachmentIndexBuilder<'source> {
    comments: HashMap<usize, CommentAttachment>,
    frames: Vec<AttachmentFrame>,
    source_position: SourcePosition<'source>,
    syntax: AttachmentSyntax,
}

// Tree-sitter's Node parent/sibling queries can retrace ancestry, so the frame stack derives every attachment fact in one forward cursor pass.
impl<'source> AttachmentIndexBuilder<'source> {
    fn new(source: &'source str, syntax: AttachmentSyntax) -> Self {
        Self {
            comments: HashMap::new(),
            frames: Vec::new(),
            source_position: SourcePosition::new(source),
            syntax,
        }
    }

    fn enter(&mut self, node: Node<'_>) {
        record_attachment_index_visit();
        let starts_line = self.source_position.starts_line_at(node.start_byte());
        let is_root = self.frames.is_empty();
        self.frames.push(AttachmentFrame::new(
            node,
            is_root,
            (self.syntax.transparent_comment_wrapper)(node.kind()),
            starts_line,
            (self.syntax.preamble_trivia)(node.kind()),
        ));
    }

    fn leave(&mut self, node: Node<'_>) {
        let frame = self.frames.pop();
        debug_assert_eq!(frame.as_ref().map(|frame| frame.node_id), Some(node.id()));
        let Some(frame) = frame else {
            return;
        };
        if let Some(parent) = self.frames.last_mut() {
            parent.record_child(
                node,
                frame.starts_line,
                frame.is_preamble_trivia,
                frame.transparent_comments,
                &mut self.comments,
            );
        }
    }
}

struct AttachmentFrame {
    node_id: usize,
    is_root: bool,
    is_transparent_comment_wrapper: bool,
    starts_line: bool,
    is_preamble_trivia: bool,
    preamble_open: bool,
    last_named_child: Option<NamedChild>,
    pending_next_node_comments: Vec<PendingNextNodeComment>,
    transparent_comments: Vec<usize>,
}

impl AttachmentFrame {
    fn new(
        node: Node<'_>,
        is_root: bool,
        is_transparent_comment_wrapper: bool,
        starts_line: bool,
        is_preamble_trivia: bool,
    ) -> Self {
        Self {
            node_id: node.id(),
            is_root,
            is_transparent_comment_wrapper,
            starts_line,
            is_preamble_trivia,
            preamble_open: true,
            last_named_child: None,
            pending_next_node_comments: Vec::new(),
            transparent_comments: Vec::new(),
        }
    }

    fn record_child(
        &mut self,
        node: Node<'_>,
        starts_line: bool,
        is_preamble_trivia: bool,
        transparent_comments: Vec<usize>,
        comments: &mut HashMap<usize, CommentAttachment>,
    ) {
        if !node.is_named() {
            return;
        }
        let child = NamedChild::new(node, starts_line, transparent_comments);
        if child.is_comment {
            let attachment = comments.entry(child.node_id).or_default();
            attachment.starts_line = starts_line;
            attachment.end_row = child.end_row;
            if let Some(previous) = self.last_named_child.as_ref() {
                attachment.same_line =
                    !previous.is_comment_boundary() && previous.end_row == child.start_row;
                attachment.previous_line = !previous.is_comment_boundary()
                    && previous.end_row.saturating_add(1) == child.start_row;
            }
            if self.is_root && self.preamble_open {
                attachment.file_preamble = true;
            }
        }
        if let Some(previous) = self.last_named_child.as_ref() {
            previous.record_immediate_next(&child, comments);
        }
        if child.is_comment_boundary() {
            for comment in child.comment_ids() {
                let attachment = comments.entry(comment).or_default();
                self.pending_next_node_comments
                    .push(PendingNextNodeComment {
                        node_id: comment,
                        starts_line: attachment.starts_line || child.starts_line,
                        end_row: attachment.end_row,
                    });
            }
        } else {
            for pending in self.pending_next_node_comments.drain(..) {
                comments.entry(pending.node_id).or_default().next_node =
                    pending.starts_line || pending.end_row == child.start_row;
            }
        }
        if self.is_transparent_comment_wrapper {
            self.transparent_comments.extend(child.comment_ids());
        }
        if self.is_root && !child.is_comment_boundary() && !is_preamble_trivia {
            self.preamble_open = false;
        }
        self.last_named_child = Some(child);
    }
}

struct PendingNextNodeComment {
    node_id: usize,
    starts_line: bool,
    end_row: usize,
}

struct NamedChild {
    node_id: usize,
    start_row: usize,
    end_row: usize,
    starts_line: bool,
    is_comment: bool,
    transparent_comments: Vec<usize>,
}

impl NamedChild {
    fn new(node: Node<'_>, starts_line: bool, transparent_comments: Vec<usize>) -> Self {
        Self {
            node_id: node.id(),
            start_row: node.start_position().row,
            end_row: node.end_position().row,
            starts_line,
            is_comment: is_comment_kind(node.kind()),
            transparent_comments,
        }
    }

    fn is_comment_boundary(&self) -> bool {
        self.is_comment || !self.transparent_comments.is_empty()
    }

    fn comment_ids(&self) -> impl Iterator<Item = usize> + '_ {
        self.is_comment
            .then_some(self.node_id)
            .into_iter()
            .chain(self.transparent_comments.iter().copied())
    }

    fn record_immediate_next(
        &self,
        next: &NamedChild,
        comments: &mut HashMap<usize, CommentAttachment>,
    ) {
        if next.is_comment_boundary() {
            return;
        }
        for comment in self.comment_ids() {
            let attachment = comments.entry(comment).or_default();
            attachment.next_line = next.start_row == attachment.end_row.saturating_add(1);
        }
    }
}

struct SourcePosition<'source> {
    source: &'source [u8],
    offset: usize,
    line_prefix_is_whitespace: bool,
}

impl<'source> SourcePosition<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source: source.as_bytes(),
            offset: 0,
            line_prefix_is_whitespace: true,
        }
    }

    fn starts_line_at(&mut self, offset: usize) -> bool {
        debug_assert!(self.offset <= offset);
        for byte in &self.source[self.offset..offset] {
            if *byte == b'\n' {
                self.line_prefix_is_whitespace = true;
            } else if !byte.is_ascii_whitespace() {
                self.line_prefix_is_whitespace = false;
            }
        }
        self.offset = offset;
        self.line_prefix_is_whitespace
    }
}

#[cfg(test)]
thread_local! {
    static ATTACHMENT_INDEX_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_attachment_index_visit() {
    ATTACHMENT_INDEX_VISITS.with(|visits| visits.set(visits.get() + 1));
}

#[cfg(not(test))]
fn record_attachment_index_visit() {}

#[cfg(test)]
pub(crate) fn reset_attachment_index_visits() {
    ATTACHMENT_INDEX_VISITS.with(|visits| visits.set(0));
}

#[cfg(test)]
pub(crate) fn attachment_index_visits() -> usize {
    ATTACHMENT_INDEX_VISITS.with(std::cell::Cell::get)
}

pub(crate) struct OwnerCandidate {
    data: OwnerData,
    suppressed_nodes: Vec<usize>,
    callable_frontier_roots: Vec<usize>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct OwnerLocation<'tree> {
    parent: Option<Node<'tree>>,
    leading_prefix: Option<Node<'tree>>,
}

impl<'tree> OwnerLocation<'tree> {
    pub(crate) fn parent(self) -> Option<Node<'tree>> {
        record_owner_parent_probe();
        self.parent
    }

    pub(crate) fn leading_prefix(self) -> Option<Node<'tree>> {
        self.leading_prefix
    }
}

enum OwnerData {
    Function(Function),
    Type(TypeOwner),
    Leaf(Leaf),
}

impl OwnerCandidate {
    pub(crate) fn function(span: Span, name: String, mut identity: Vec<String>) -> Self {
        identity.push(name.clone());
        Self {
            data: OwnerData::Function(Function {
                span,
                name,
                identity,
            }),
            suppressed_nodes: Vec::new(),
            callable_frontier_roots: Vec::new(),
        }
    }

    pub(crate) fn type_owner(span: Span, name: String, identity: Vec<String>) -> Self {
        Self {
            data: OwnerData::Type(TypeOwner {
                span,
                name,
                identity,
            }),
            suppressed_nodes: Vec::new(),
            callable_frontier_roots: Vec::new(),
        }
    }

    pub(crate) fn leaf(span: Span, name: String) -> Self {
        Self {
            data: OwnerData::Leaf(Leaf { span, name }),
            suppressed_nodes: Vec::new(),
            callable_frontier_roots: Vec::new(),
        }
    }

    pub(crate) fn suppressing(mut self, nodes: Vec<usize>) -> Self {
        self.suppressed_nodes = nodes;
        self
    }

    pub(crate) fn suppressing_callable_frontiers(mut self, mut roots: Vec<usize>) -> Self {
        roots.sort_unstable();
        roots.dedup();
        self.callable_frontier_roots = roots;
        self
    }
}

struct Facts {
    functions: Vec<Function>,
    types: Vec<TypeOwner>,
    comments: Vec<Comment>,
    leaves: Vec<Leaf>,
    syntax: Vec<SyntaxEvent>,
    ownership: TreeOwnership,
}

struct SyntaxEvent {
    span: Span,
    token: CodeToken,
    owner: Option<TreeOwner>,
}

impl Facts {
    fn input(&self) -> TreeInput<'_> {
        TreeInput {
            functions: &self.functions,
            types: &self.types,
            leaves: &self.leaves,
            comments: &self.comments,
            ownership: &self.ownership,
        }
    }
}

pub(crate) trait LanguageSpec: Copy {
    type Context: Default;

    fn label(self) -> &'static str;
    fn grammar(self) -> Language;
    fn build_context(self, _root: Node<'_>, _source: &str) -> Self::Context {
        Self::Context::default()
    }
    fn is_owner_prefix(self, _kind: &str) -> bool {
        false
    }
    fn callable_kind(self) -> Option<fn(&str) -> bool> {
        None
    }
    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        function_depth: usize,
        callable_subtrees: &CallableSubtrees,
    ) -> Option<OwnerCandidate>;
    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        context: &Self::Context,
    ) -> Option<CommentKind>;
}

pub(crate) fn analyze<S: LanguageSpec>(
    path: &Path,
    source: &str,
    selection: &Selection,
    spec: S,
) -> Result<Vec<Finding>, AnalysisError> {
    let facts = parse_facts(path, source, spec)?;
    Ok(tree_findings(
        path,
        source,
        selection,
        facts.input(),
        spec.label(),
    ))
}

pub(crate) fn document<S: LanguageSpec>(
    path: &Path,
    source: &str,
    spec: S,
) -> Result<ParsedFile, AnalysisError> {
    let Facts {
        functions,
        types,
        comments,
        leaves,
        syntax,
        ownership,
    } = parse_facts(path, source, spec)?;
    let code = assign_code(syntax, functions.len(), types.len(), leaves.len());
    Ok(tree_document(
        source,
        TreeInput {
            functions: &functions,
            types: &types,
            leaves: &leaves,
            comments: &comments,
            ownership: &ownership,
        },
        code,
    ))
}

fn parse_facts<S: LanguageSpec>(
    path: &Path,
    source: &str,
    spec: S,
) -> Result<Facts, AnalysisError> {
    let tree = parse(path, source, spec.label(), spec.grammar())?;
    let callable_subtrees = CallableSubtrees::from_root(tree.root_node(), spec.callable_kind());
    let context = spec.build_context(tree.root_node(), source);
    let mut collector = FactCollector::new(source);
    let mut traversal = Vec::new();
    for event in events(tree.root_node()) {
        match event {
            WalkEvent::Enter(node) => {
                let location = traversal
                    .last()
                    .map_or_else(OwnerLocation::default, |frame: &TraversalFrame<'_>| {
                        frame.location_for(node, source, spec)
                    });
                if let Some(frame) = traversal.last_mut() {
                    frame.record_child(node, source, spec);
                }
                collector.enter(node, location, spec, &context, &callable_subtrees);
                traversal.push(TraversalFrame::new(node));
            }
            WalkEvent::Leave(node) => {
                collector.leave(node, &callable_subtrees);
                let frame = traversal.pop();
                debug_assert_eq!(frame.map(|frame| frame.node.id()), Some(node.id()));
            }
        }
    }
    collector.finish()
}

struct TraversalFrame<'tree> {
    node: Node<'tree>,
    last_named_child: Option<Node<'tree>>,
    prefix_start: Option<Node<'tree>>,
}

impl<'tree> TraversalFrame<'tree> {
    fn new(node: Node<'tree>) -> Self {
        Self {
            node,
            last_named_child: None,
            prefix_start: None,
        }
    }

    fn location_for<S: LanguageSpec>(
        &self,
        node: Node<'tree>,
        source: &str,
        spec: S,
    ) -> OwnerLocation<'tree> {
        let leading_prefix = self
            .last_named_child
            .filter(|previous| spec.is_owner_prefix(previous.kind()))
            .filter(|previous| only_whitespace_between(*previous, node, source))
            .and(self.prefix_start);
        OwnerLocation {
            parent: Some(self.node),
            leading_prefix,
        }
    }

    fn record_child<S: LanguageSpec>(&mut self, node: Node<'tree>, source: &str, spec: S) {
        if !node.is_named() {
            return;
        }
        self.prefix_start = spec.is_owner_prefix(node.kind()).then(|| {
            self.last_named_child
                .filter(|previous| spec.is_owner_prefix(previous.kind()))
                .filter(|previous| only_whitespace_between(*previous, node, source))
                .and(self.prefix_start)
                .unwrap_or(node)
        });
        self.last_named_child = Some(node);
    }
}

fn only_whitespace_between(left: Node<'_>, right: Node<'_>, source: &str) -> bool {
    source
        .get(left.end_byte()..right.start_byte())
        .is_some_and(|gap| gap.bytes().all(|byte| byte.is_ascii_whitespace()))
}

#[derive(Default)]
struct FactCollector<'source> {
    source: &'source str,
    active: Vec<TreeOwner>,
    active_budgets: Vec<TreeOwner>,
    active_types: Vec<TreeOwner>,
    owner_nodes: Vec<OwnerFrame>,
    suppressed_owner_nodes: HashSet<usize>,
    callable_frontier_starts: HashMap<usize, usize>,
    active_callable_frontiers: Vec<ActiveCallableFrontier>,
    callable_frontiers_at_depth: Vec<usize>,
    callable_depth: usize,
    comment_node: Option<usize>,
    active_function_depth: usize,
    functions: Vec<Function>,
    function_parents: Vec<Option<TreeOwner>>,
    types: Vec<TypeOwner>,
    type_parents: Vec<Option<TreeOwner>>,
    comments: Vec<Comment>,
    comment_choices: Vec<CommentChoice>,
    leaves: Vec<Leaf>,
    leaf_parents: Vec<Option<TreeOwner>>,
    syntax: Vec<SyntaxEvent>,
    trailing_direct: Vec<Option<OwnerSpan>>,
    trailing_budget: Vec<Option<OwnerSpan>>,
    invariant_error: Option<String>,
}

struct OwnerFrame {
    node_id: usize,
    owner: TreeOwner,
    suppressed_nodes: Vec<usize>,
}

struct ActiveCallableFrontier {
    node_id: usize,
    callable_depth: usize,
    count: usize,
}

impl<'source> FactCollector<'source> {
    fn new(source: &'source str) -> Self {
        let line_slots = source.bytes().filter(|byte| *byte == b'\n').count() + 2;
        Self {
            source,
            trailing_direct: vec![None; line_slots],
            trailing_budget: vec![None; line_slots],
            ..Self::default()
        }
    }

    fn enter<S: LanguageSpec>(
        &mut self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        spec: S,
        context: &S::Context,
        callable_subtrees: &CallableSubtrees,
    ) {
        if self.comment_node.is_some() {
            return;
        }
        self.open_callable_frontiers(node);
        if let Some(kind) = spec.classify_comment(node, self.source, context) {
            self.push_comment(Comment {
                span: Span::from_comment_node(node, self.source),
                kind,
                text: node_text(node, self.source).to_owned(),
            });
            self.comment_node = Some(node.id());
            return;
        }

        let is_callable = callable_subtrees.is_callable(node);
        if is_callable {
            record_callable_candidate_check();
        }
        let callable_is_suppressed = is_callable && self.callable_frontier_is_active();
        let candidate = (!self.suppressed_owner_nodes.contains(&node.id())
            && !callable_is_suppressed)
            .then(|| {
                spec.owner(
                    node,
                    location,
                    self.source,
                    self.active_function_depth,
                    callable_subtrees,
                )
            })
            .flatten();
        if let Some(candidate) = candidate {
            let OwnerCandidate {
                data,
                suppressed_nodes,
                callable_frontier_roots,
            } = candidate;
            let owner = match data {
                OwnerData::Function(function) => {
                    let owner = TreeOwner::Function(self.functions.len());
                    self.functions.push(function);
                    self.active_function_depth += 1;
                    owner
                }
                OwnerData::Type(type_owner) => {
                    let owner = TreeOwner::Type(self.types.len());
                    self.types.push(type_owner);
                    owner
                }
                OwnerData::Leaf(leaf) => {
                    let owner = TreeOwner::Leaf(self.leaves.len());
                    self.leaves.push(leaf);
                    owner
                }
            };
            self.register_owner(owner, node.start_byte());
            self.suppressed_owner_nodes
                .extend(suppressed_nodes.iter().copied());
            self.register_callable_frontiers(node.id(), callable_frontier_roots);
            self.owner_nodes.push(OwnerFrame {
                node_id: node.id(),
                owner,
                suppressed_nodes,
            });
        }

        self.push_syntax(Span::from_node(node), CodeToken::enter(node.kind()));
        if node.child_count() == 0 {
            let text = node_text(node, self.source);
            if !text.trim().is_empty() {
                self.push_syntax(Span::from_node(node), CodeToken::atom(node.kind(), text));
            }
        }
        self.callable_depth += usize::from(is_callable);
    }

    fn leave(&mut self, node: Node<'_>, callable_subtrees: &CallableSubtrees) {
        if let Some(comment_node) = self.comment_node {
            if comment_node == node.id() {
                self.comment_node = None;
            }
            return;
        }

        self.push_syntax(Span::from_node(node), CodeToken::leave(node.kind()));
        if self
            .owner_nodes
            .last()
            .is_some_and(|frame| frame.node_id == node.id())
            && let Some(frame) = self.owner_nodes.pop()
        {
            let owner = frame.owner;
            for suppressed in frame.suppressed_nodes {
                self.suppressed_owner_nodes.remove(&suppressed);
            }
            let popped = self.active.pop();
            debug_assert_eq!(popped, Some(owner));
            if is_budget_owner(owner) {
                debug_assert_eq!(self.active_budgets.pop(), Some(owner));
            }
            if matches!(owner, TreeOwner::Type(_)) {
                debug_assert_eq!(self.active_types.pop(), Some(owner));
            }
            self.record_trailing(owner);
            self.active_function_depth -= usize::from(matches!(owner, TreeOwner::Function(_)));
        }
        if callable_subtrees.is_callable(node) {
            debug_assert!(self.callable_depth > 0);
            if self.callable_depth == 0 {
                self.invariant_error = Some("callable depth underflowed".to_owned());
            } else {
                self.callable_depth -= 1;
            }
        }
        self.close_callable_frontiers(node);
    }

    fn register_callable_frontiers(&mut self, owner_node_id: usize, roots: Vec<usize>) {
        record_callable_frontier_registration(roots.len());
        for root in roots {
            debug_assert_ne!(root, owner_node_id);
            *self.callable_frontier_starts.entry(root).or_default() += 1;
        }
    }

    fn open_callable_frontiers(&mut self, node: Node<'_>) {
        let Some(count) = self.callable_frontier_starts.remove(&node.id()) else {
            return;
        };
        if self.callable_frontiers_at_depth.len() <= self.callable_depth {
            self.callable_frontiers_at_depth
                .resize(self.callable_depth + 1, 0);
        }
        self.callable_frontiers_at_depth[self.callable_depth] += count;
        self.active_callable_frontiers.push(ActiveCallableFrontier {
            node_id: node.id(),
            callable_depth: self.callable_depth,
            count,
        });
    }

    fn callable_frontier_is_active(&self) -> bool {
        self.callable_frontiers_at_depth
            .get(self.callable_depth)
            .is_some_and(|count| *count > 0)
    }

    fn close_callable_frontiers(&mut self, node: Node<'_>) {
        if !self
            .active_callable_frontiers
            .last()
            .is_some_and(|frontier| frontier.node_id == node.id())
        {
            return;
        }
        let Some(frontier) = self.active_callable_frontiers.pop() else {
            return;
        };
        let count = &mut self.callable_frontiers_at_depth[frontier.callable_depth];
        debug_assert!(*count >= frontier.count);
        if *count < frontier.count {
            self.invariant_error = Some("callable frontier count underflowed".to_owned());
            return;
        }
        *count -= frontier.count;
        while self.callable_frontiers_at_depth.last() == Some(&0) {
            self.callable_frontiers_at_depth.pop();
        }
    }

    fn finish(mut self) -> Result<Facts, AnalysisError> {
        if self.callable_depth != 0
            || !self.callable_frontier_starts.is_empty()
            || !self.active_callable_frontiers.is_empty()
            || !self.callable_frontiers_at_depth.is_empty()
        {
            self.invariant_error = Some("callable frontier scopes did not close".to_owned());
        }
        if let Some(detail) = self.invariant_error {
            return Err(AnalysisError::Invariant(detail));
        }
        let mut choices = self.comment_choices;
        assign_leading(
            self.source,
            &self.comments,
            &owner_spans(&self.functions, &self.types, &self.leaves),
            &mut choices,
        );
        let mut ownership = TreeOwnership {
            function_budget: vec![Vec::new(); self.functions.len()],
            function_parents: self.function_parents,
            type_budget: vec![Vec::new(); self.types.len()],
            type_parents: self.type_parents,
            leaves: vec![Vec::new(); self.leaves.len()],
            leaf_parents: self.leaf_parents,
            file: Vec::new(),
            comment_owners: Vec::with_capacity(self.comments.len()),
        };
        materialize_comments(&self.comments, &choices, &mut ownership);
        Ok(Facts {
            functions: self.functions,
            types: self.types,
            comments: self.comments,
            leaves: self.leaves,
            syntax: self.syntax,
            ownership,
        })
    }

    fn register_owner(&mut self, owner: TreeOwner, node_start: usize) {
        let parent = self.active.last().copied().filter(|parent| {
            let parent = self.candidate(*parent);
            let child = self.candidate(owner);
            let valid = parent.start_byte <= child.start_byte
                && child.end_byte <= parent.end_byte
                && (parent.start_byte < child.start_byte || child.end_byte < parent.end_byte);
            if !valid {
                let relationship =
                    if parent.start_byte == child.start_byte && parent.end_byte == child.end_byte {
                        "equal"
                    } else {
                        "crossing"
                    };
                self.invariant_error = Some(format!(
                    "tree owners have {relationship} spans at bytes {}..{} and {}..{}",
                    parent.start_byte, parent.end_byte, child.start_byte, child.end_byte
                ));
            }
            valid
        });
        match owner {
            TreeOwner::Function(_) => self.function_parents.push(parent),
            TreeOwner::Type(_) => self.type_parents.push(parent),
            TreeOwner::Leaf(_) => self.leaf_parents.push(parent),
        }
        self.reassign_prefixed_comments(owner, node_start);
        self.reassign_prefixed_syntax(owner, node_start);
        self.active.push(owner);
        if is_budget_owner(owner) {
            self.active_budgets.push(owner);
        }
        if matches!(owner, TreeOwner::Type(_)) {
            self.active_types.push(owner);
        }
    }

    fn push_comment(&mut self, comment: Comment) {
        let mut choice = CommentChoice::default();
        match comment.kind {
            CommentKind::FileNarrative => {}
            CommentKind::TypeNarrative => {
                if let Some(owner) = self
                    .active_types
                    .last()
                    .copied()
                    .map(|owner| self.candidate(owner))
                {
                    choice.budget = Some(owner);
                    choice.direct = Some(owner);
                }
            }
            CommentKind::Narrative
            | CommentKind::ToolDirective
            | CommentKind::SafetyProof
            | CommentKind::PublicDocs => {
                choice.direct = self
                    .active
                    .last()
                    .copied()
                    .map(|owner| self.candidate(owner));
                choice.budget = self
                    .active_budgets
                    .last()
                    .copied()
                    .map(|owner| self.candidate(owner));
                if let Some(owner) = self
                    .trailing_direct
                    .get(comment.span.start_line)
                    .copied()
                    .flatten()
                {
                    choose(&mut choice.direct, owner);
                }
                if let Some(owner) = self
                    .trailing_budget
                    .get(comment.span.start_line)
                    .copied()
                    .flatten()
                {
                    choose(&mut choice.budget, owner);
                }
            }
        }
        self.comments.push(comment);
        self.comment_choices.push(choice);
    }

    fn record_trailing(&mut self, owner: TreeOwner) {
        let owner = self.candidate(owner);
        if let Some(slot) = self.trailing_direct.get_mut(owner.end_line) {
            choose(slot, owner);
        }
        if is_budget_owner(owner.owner)
            && let Some(slot) = self.trailing_budget.get_mut(owner.end_line)
        {
            choose(slot, owner);
        }
    }

    fn candidate(&self, owner: TreeOwner) -> OwnerSpan {
        OwnerSpan::new(
            owner,
            owner_span(owner, &self.functions, &self.types, &self.leaves),
        )
    }

    fn reassign_prefixed_syntax(&mut self, owner: TreeOwner, node_start: usize) {
        let span = owner_span(owner, &self.functions, &self.types, &self.leaves);
        if span.start_byte >= node_start {
            return;
        }
        for event in self.syntax.iter_mut().rev() {
            if event.span.end_byte <= span.start_byte {
                break;
            }
            if span.contains(&event.span) {
                event.owner = Some(owner);
            }
        }
    }

    fn reassign_prefixed_comments(&mut self, owner: TreeOwner, node_start: usize) {
        let candidate = self.candidate(owner);
        if candidate.start_byte >= node_start {
            return;
        }
        for (comment, choice) in self.comments.iter().zip(&mut self.comment_choices).rev() {
            if comment.span.end_byte <= candidate.start_byte {
                break;
            }
            if comment.span.end_byte <= node_start
                && candidate.start_byte <= comment.span.start_byte
                && is_regular(comment.kind)
            {
                choice.direct = Some(candidate);
                if is_budget_owner(owner) {
                    choice.budget = Some(candidate);
                }
            }
        }
    }

    fn push_syntax(&mut self, span: Span, token: CodeToken) {
        self.syntax.push(SyntaxEvent {
            span,
            token,
            owner: self.active.last().copied(),
        });
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct OwnerSpan {
    owner: TreeOwner,
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
}

impl OwnerSpan {
    fn new(owner: TreeOwner, span: &Span) -> Self {
        Self {
            owner,
            start_byte: span.start_byte,
            end_byte: span.end_byte,
            start_line: span.start_line,
            end_line: span.end_line,
        }
    }

    fn key(self) -> (usize, u8) {
        let priority = match self.owner {
            TreeOwner::Leaf(_) => 0,
            TreeOwner::Function(_) => 1,
            TreeOwner::Type(_) => 2,
        };
        (self.end_byte - self.start_byte, priority)
    }
}

#[derive(Clone, Copy, Default)]
struct CommentChoice {
    budget: Option<OwnerSpan>,
    direct: Option<OwnerSpan>,
}

fn owner_spans(functions: &[Function], types: &[TypeOwner], leaves: &[Leaf]) -> Vec<OwnerSpan> {
    functions
        .iter()
        .enumerate()
        .map(|(index, function)| OwnerSpan::new(TreeOwner::Function(index), &function.span))
        .chain(
            types
                .iter()
                .enumerate()
                .map(|(index, owner)| OwnerSpan::new(TreeOwner::Type(index), &owner.span)),
        )
        .chain(
            leaves
                .iter()
                .enumerate()
                .map(|(index, leaf)| OwnerSpan::new(TreeOwner::Leaf(index), &leaf.span)),
        )
        .collect()
}

fn assign_leading(
    source: &str,
    comments: &[Comment],
    owners: &[OwnerSpan],
    choices: &mut [CommentChoice],
) {
    let line_starts: Vec<_> = std::iter::once(0)
        .chain(
            source
                .bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index + 1),
        )
        .collect();
    let mut comment_by_line = vec![None; line_starts.len() + 1];
    for (index, comment) in comments.iter().enumerate() {
        let line_start = line_starts[comment.span.start_line - 1];
        if source.as_bytes()[line_start..comment.span.start_byte]
            .iter()
            .all(u8::is_ascii_whitespace)
        {
            for line in comment.span.lines() {
                comment_by_line[line] = Some(index);
            }
        }
    }

    let previous: Vec<_> = comments
        .iter()
        .map(|comment| {
            comment_by_line
                .get(comment.span.start_line.saturating_sub(1))
                .copied()
                .flatten()
        })
        .collect();
    let mut seeds = vec![CommentChoice::default(); comments.len()];
    for owner in owners {
        let Some(index) = comment_by_line
            .get(owner.start_line.saturating_sub(1))
            .copied()
            .flatten()
        else {
            continue;
        };
        if source
            .get(comments[index].span.end_byte..owner.start_byte)
            .is_some_and(|gap| gap.bytes().all(|byte| byte.is_ascii_whitespace()))
        {
            apply_regular(&mut seeds[index], *owner);
        }
    }
    for index in (0..comments.len()).rev() {
        let seed = seeds[index];
        if is_regular(comments[index].kind) {
            merge_choice(&mut choices[index], seed);
        }
        if let Some(previous) = previous[index] {
            merge_choice(&mut seeds[previous], seed);
        }
    }
}

fn merge_choice(target: &mut CommentChoice, source: CommentChoice) {
    if let Some(owner) = source.budget {
        choose(&mut target.budget, owner);
    }
    if let Some(owner) = source.direct {
        choose(&mut target.direct, owner);
    }
}

fn apply_regular(choice: &mut CommentChoice, owner: OwnerSpan) {
    if is_budget_owner(owner.owner) {
        choose(&mut choice.budget, owner);
    }
    choose(&mut choice.direct, owner);
}

fn choose(best: &mut Option<OwnerSpan>, candidate: OwnerSpan) {
    if best.is_none_or(|current| candidate.key() < current.key()) {
        *best = Some(candidate);
    }
}

fn is_budget_owner(owner: TreeOwner) -> bool {
    matches!(owner, TreeOwner::Function(_) | TreeOwner::Type(_))
}

fn is_regular(kind: CommentKind) -> bool {
    !matches!(
        kind,
        CommentKind::FileNarrative | CommentKind::TypeNarrative
    )
}

fn materialize_comments(
    comments: &[Comment],
    choices: &[CommentChoice],
    ownership: &mut TreeOwnership,
) {
    for (comment, choice) in comments.iter().zip(choices) {
        match choice.budget.map(|owner| owner.owner) {
            Some(TreeOwner::Function(index)) => {
                ownership.function_budget[index].push(comment.clone());
            }
            Some(TreeOwner::Type(index)) => ownership.type_budget[index].push(comment.clone()),
            Some(TreeOwner::Leaf(_)) | None => {}
        }
        let direct = choice.direct.map(|owner| owner.owner);
        match direct {
            Some(TreeOwner::Leaf(index)) => ownership.leaves[index].push(comment.clone()),
            Some(TreeOwner::Function(_) | TreeOwner::Type(_)) => {}
            None => ownership.file.push(comment.clone()),
        }
        ownership.comment_owners.push(direct);
    }
}

fn owner_span<'owner>(
    owner: TreeOwner,
    functions: &'owner [Function],
    types: &'owner [TypeOwner],
    leaves: &'owner [Leaf],
) -> &'owner Span {
    match owner {
        TreeOwner::Function(index) => &functions[index].span,
        TreeOwner::Type(index) => &types[index].span,
        TreeOwner::Leaf(index) => &leaves[index].span,
    }
}

pub(crate) fn parse(
    path: &Path,
    source: &str,
    label: &'static str,
    grammar: Language,
) -> Result<Tree, AnalysisError> {
    let mut parser = Parser::new();
    parser
        .set_language(&grammar)
        .map_err(|error| AnalysisError::ParserInit {
            language: label,
            detail: error.to_string(),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AnalysisError::Parse {
            path: path.display().to_string(),
            language: label,
        })?;
    if tree.root_node().has_error() {
        return Err(AnalysisError::Parse {
            path: path.display().to_string(),
            language: label,
        });
    }
    Ok(tree)
}

fn assign_code(
    syntax: impl IntoIterator<Item = SyntaxEvent>,
    function_count: usize,
    type_count: usize,
    leaf_count: usize,
) -> Vec<Vec<CodeToken>> {
    let mut code = vec![Vec::new(); 1 + function_count + type_count + leaf_count];
    for event in syntax {
        let owner = event.owner.map_or(0, |owner| match owner {
            TreeOwner::Function(index) => index + 1,
            TreeOwner::Type(index) => function_count + index + 1,
            TreeOwner::Leaf(index) => function_count + type_count + index + 1,
        });
        code[owner].push(event.token);
    }
    code
}

pub(crate) fn function_name(node: Node<'_>, location: OwnerLocation<'_>, source: &str) -> String {
    node.child_by_field_name("name")
        .or_else(|| {
            location
                .parent()
                .and_then(|parent| parent.child_by_field_name("name"))
        })
        .map_or(ANONYMOUS_FUNCTION_NAME, |name| node_text(name, source))
        .to_owned()
}

pub(crate) fn first_descendant_with_kind<'tree>(
    node: Node<'tree>,
    kind: &str,
) -> Option<Node<'tree>> {
    events(node).find_map(|event| {
        #[cfg(test)]
        FIRST_DESCENDANT_VISITS.with(|visits| visits.set(visits.get() + 1));
        match event {
            WalkEvent::Enter(candidate) if candidate.kind() == kind => Some(candidate),
            WalkEvent::Enter(_) | WalkEvent::Leave(_) => None,
        }
    })
}

#[cfg(test)]
thread_local! {
    static FIRST_DESCENDANT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_first_descendant_visits() {
    FIRST_DESCENDANT_VISITS.with(|visits| visits.set(0));
}

#[cfg(test)]
pub(crate) fn first_descendant_visits() -> usize {
    FIRST_DESCENDANT_VISITS.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    static OWNER_PARENT_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_owner_parent_probe() {
    OWNER_PARENT_PROBES.with(|probes| probes.set(probes.get() + 1));
}

#[cfg(not(test))]
fn record_owner_parent_probe() {}

#[cfg(test)]
pub(crate) fn reset_owner_parent_probes() {
    OWNER_PARENT_PROBES.with(|probes| probes.set(0));
}

#[cfg(test)]
pub(crate) fn owner_parent_probes() -> usize {
    OWNER_PARENT_PROBES.with(std::cell::Cell::get)
}

pub(crate) fn ancestors(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    std::iter::successors(node.parent(), |ancestor| ancestor.parent())
}

pub(crate) fn direct_named_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

pub(crate) fn starts_physical_line(node: Node<'_>, source: &str) -> bool {
    let line_start = source[..node.start_byte()]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    source.as_bytes()[line_start..node.start_byte()]
        .iter()
        .all(u8::is_ascii_whitespace)
}

pub(crate) fn canonical_syntax(node: Node<'_>, source: &str) -> String {
    let mut identity = String::new();
    let mut excluded_depth = 0_usize;
    for event in events(node) {
        match event {
            WalkEvent::Enter(_) if excluded_depth > 0 => excluded_depth += 1,
            WalkEvent::Enter(current) if is_comment_kind(current.kind()) => excluded_depth = 1,
            WalkEvent::Enter(current) if current.child_count() == 0 => {
                let text = node_text(current, source);
                if !text.trim().is_empty() {
                    push_length_prefixed(&mut identity, current.kind());
                    push_length_prefixed(&mut identity, text);
                }
            }
            WalkEvent::Enter(_) => {}
            WalkEvent::Leave(_) if excluded_depth > 0 => excluded_depth -= 1,
            WalkEvent::Leave(_) => {}
        }
    }
    identity
}

fn is_comment_kind(kind: &str) -> bool {
    matches!(
        kind,
        "comment" | "line_comment" | "block_comment" | "multiline_comment"
    )
}

fn push_length_prefixed(output: &mut String, value: &str) {
    output.push_str(&value.len().to_string());
    output.push(':');
    output.push_str(value);
}

pub(crate) fn node_text<'source>(node: Node<'_>, source: &'source str) -> &'source str {
    source.get(node.byte_range()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_same_start_owners_share_one_long_leading_chain() {
        const COUNT: usize = 4_096;
        const COMMENT: &str = "# Why.\n";

        let owner_start = COMMENT.len() * COUNT;
        let source = format!("{}{}", COMMENT.repeat(COUNT), "x".repeat(COUNT));
        let comments: Vec<_> = (0..COUNT)
            .map(|index| Comment {
                span: Span {
                    start_byte: index * COMMENT.len(),
                    end_byte: (index + 1) * COMMENT.len() - 1,
                    start_line: index + 1,
                    end_line: index + 1,
                },
                kind: CommentKind::Narrative,
                text: COMMENT.trim_end().to_owned(),
            })
            .collect();
        let functions: Vec<_> = (0..COUNT)
            .map(|index| Function {
                span: Span {
                    start_byte: owner_start,
                    end_byte: source.len() - index,
                    start_line: COUNT + 1,
                    end_line: COUNT + 1,
                },
                name: format!("f{index}"),
                identity: vec![format!("f{index}")],
            })
            .collect();

        let mut choices = vec![CommentChoice::default(); COUNT];
        assign_leading(
            &source,
            &comments,
            &owner_spans(&functions, &[], &[]),
            &mut choices,
        );

        assert!(
            choices
                .iter()
                .all(|choice| choice.direct.is_some_and(|owner| {
                    owner.owner == TreeOwner::Function(COUNT - 1) && choice.budget == choice.direct
                }))
        );
    }
}
