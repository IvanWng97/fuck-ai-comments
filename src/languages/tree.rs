use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{
    CodeToken, Comment, CommentKind, Function, Leaf, ParsedFile, Span, TreeInput, TreeOwner,
    TreeOwnership, TypeOwner, tree_document, tree_findings,
};

use super::walk::{WalkEvent, events, outermost_matching_nodes};

pub(crate) const ANONYMOUS_FUNCTION_NAME: &str = "<anonymous>";

#[derive(Clone, Copy)]
pub(crate) enum DirectivePlacement {
    Region,
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

impl AttachmentIndex {
    pub(crate) fn from_root(root: Node<'_>, source: &str) -> Self {
        let mut builder = AttachmentIndexBuilder::new(source);
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
            DirectivePlacement::Region => attachment.starts_line,
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
}

// Tree-sitter's Node parent/sibling queries can retrace ancestry, so the frame stack derives every attachment fact in one forward cursor pass.
impl<'source> AttachmentIndexBuilder<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            comments: HashMap::new(),
            frames: Vec::new(),
            source_position: SourcePosition::new(source),
        }
    }

    fn enter(&mut self, node: Node<'_>) {
        record_attachment_index_visit();
        let starts_line = self.source_position.starts_line_at(node.start_byte());
        let is_root = self.frames.is_empty();
        if let Some(parent) = self.frames.last_mut() {
            parent.record_child(node, starts_line, &mut self.comments);
        }
        self.frames.push(AttachmentFrame::new(node, is_root));
    }

    fn leave(&mut self, node: Node<'_>) {
        let frame = self.frames.pop();
        debug_assert_eq!(frame.as_ref().map(|frame| frame.node_id), Some(node.id()));
        let Some(frame) = frame else {
            return;
        };
        if !frame.jsx_comments.is_empty()
            && let Some(parent) = self.frames.last_mut()
        {
            parent.record_jsx_comments(node.id(), frame.jsx_comments);
        }
    }
}

struct AttachmentFrame {
    node_id: usize,
    is_root: bool,
    is_jsx_expression: bool,
    preamble_open: bool,
    last_named_child: Option<NamedChild>,
    jsx_comments: Vec<usize>,
}

impl AttachmentFrame {
    fn new(node: Node<'_>, is_root: bool) -> Self {
        Self {
            node_id: node.id(),
            is_root,
            is_jsx_expression: node.kind() == "jsx_expression",
            preamble_open: true,
            last_named_child: None,
            jsx_comments: Vec::new(),
        }
    }

    fn record_child(
        &mut self,
        node: Node<'_>,
        starts_line: bool,
        comments: &mut HashMap<usize, CommentAttachment>,
    ) {
        if !node.is_named() {
            return;
        }
        let child = NamedChild::new(node);
        if let Some(previous) = self.last_named_child.as_ref() {
            previous.record_next(&child, comments);
        }
        if child.is_comment {
            let attachment = comments.entry(child.node_id).or_default();
            attachment.starts_line = starts_line;
            if let Some(previous) = self.last_named_child.as_ref() {
                attachment.same_line = !previous.is_comment && previous.end_row == child.start_row;
                attachment.previous_line =
                    !previous.is_comment && previous.end_row.saturating_add(1) == child.start_row;
            }
            if self.is_root && self.preamble_open {
                attachment.file_preamble = true;
            }
            if self.is_jsx_expression {
                self.jsx_comments.push(child.node_id);
            }
        }
        if self.is_root && !child.is_comment && node.kind() != "hash_bang_line" {
            self.preamble_open = false;
        }
        self.last_named_child = Some(child);
    }

    fn record_jsx_comments(&mut self, node_id: usize, jsx_comments: Vec<usize>) {
        if let Some(child) = self
            .last_named_child
            .as_mut()
            .filter(|child| child.node_id == node_id)
        {
            child.jsx_comments = jsx_comments;
        }
    }
}

struct NamedChild {
    node_id: usize,
    start_row: usize,
    end_row: usize,
    is_comment: bool,
    jsx_comments: Vec<usize>,
}

impl NamedChild {
    fn new(node: Node<'_>) -> Self {
        Self {
            node_id: node.id(),
            start_row: node.start_position().row,
            end_row: node.end_position().row,
            is_comment: is_comment_kind(node.kind()),
            jsx_comments: Vec::new(),
        }
    }

    fn record_next(&self, next: &NamedChild, comments: &mut HashMap<usize, CommentAttachment>) {
        if self.is_comment {
            let attachment = comments.entry(self.node_id).or_default();
            attachment.next_node = !next.is_comment;
            attachment.next_line =
                !next.is_comment && next.start_row == self.end_row.saturating_add(1);
        }
        for comment in &self.jsx_comments {
            comments.entry(*comment).or_default().next_node = !next.is_comment;
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
}

#[derive(Clone, Copy, Default)]
pub(crate) struct OwnerLocation<'tree> {
    parent: Option<Node<'tree>>,
    leading_decorator: Option<Node<'tree>>,
}

impl<'tree> OwnerLocation<'tree> {
    pub(crate) fn parent(self) -> Option<Node<'tree>> {
        record_owner_parent_probe();
        self.parent
    }

    pub(crate) fn leading_decorator(self) -> Option<Node<'tree>> {
        self.leading_decorator
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
        }
    }

    pub(crate) fn leaf(span: Span, name: String) -> Self {
        Self {
            data: OwnerData::Leaf(Leaf { span, name }),
            suppressed_nodes: Vec::new(),
        }
    }

    pub(crate) fn suppressing(mut self, nodes: Vec<usize>) -> Self {
        self.suppressed_nodes = nodes;
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
    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        function_depth: usize,
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
    let context = spec.build_context(tree.root_node(), source);
    let mut collector = FactCollector::new(source);
    let mut traversal = Vec::new();
    for event in events(tree.root_node()) {
        match event {
            WalkEvent::Enter(node) => {
                let location = traversal
                    .last()
                    .map_or_else(OwnerLocation::default, |frame: &TraversalFrame<'_>| {
                        frame.location_for(node, source)
                    });
                if let Some(frame) = traversal.last_mut() {
                    frame.record_child(node, source);
                }
                collector.enter(node, location, spec, &context);
                traversal.push(TraversalFrame::new(node));
            }
            WalkEvent::Leave(node) => {
                collector.leave(node);
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
    decorator_start: Option<Node<'tree>>,
}

impl<'tree> TraversalFrame<'tree> {
    fn new(node: Node<'tree>) -> Self {
        Self {
            node,
            last_named_child: None,
            decorator_start: None,
        }
    }

    fn location_for(&self, node: Node<'tree>, source: &str) -> OwnerLocation<'tree> {
        let leading_decorator = self
            .last_named_child
            .filter(|previous| previous.kind() == "decorator")
            .filter(|previous| only_whitespace_between(*previous, node, source))
            .and(self.decorator_start);
        OwnerLocation {
            parent: Some(self.node),
            leading_decorator,
        }
    }

    fn record_child(&mut self, node: Node<'tree>, source: &str) {
        if !node.is_named() {
            return;
        }
        self.decorator_start = (node.kind() == "decorator").then(|| {
            self.last_named_child
                .filter(|previous| previous.kind() == "decorator")
                .filter(|previous| only_whitespace_between(*previous, node, source))
                .and(self.decorator_start)
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
    ) {
        if self.comment_node.is_some() {
            return;
        }
        if let Some(kind) = spec.classify_comment(node, self.source, context) {
            self.push_comment(Comment {
                span: Span::from_comment_node(node, self.source),
                kind,
                text: node_text(node, self.source).to_owned(),
            });
            self.comment_node = Some(node.id());
            return;
        }

        let candidate = (!self.suppressed_owner_nodes.contains(&node.id()))
            .then(|| spec.owner(node, location, self.source, self.active_function_depth))
            .flatten();
        if let Some(candidate) = candidate {
            let OwnerCandidate {
                data,
                suppressed_nodes,
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
    }

    fn leave(&mut self, node: Node<'_>) {
        if let Some(comment_node) = self.comment_node {
            if comment_node == node.id() {
                self.comment_node = None;
            }
            return;
        }

        self.push_syntax(Span::from_node(node), CodeToken::leave(node.kind()));
        let Some(frame) = self.owner_nodes.last() else {
            return;
        };
        if frame.node_id != node.id() {
            return;
        }
        let Some(frame) = self.owner_nodes.pop() else {
            return;
        };
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

    fn finish(self) -> Result<Facts, AnalysisError> {
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
            first_descendant_with_kind(node, "function_definition")
                .and_then(|function| function.child_by_field_name("name"))
        })
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
    events(node).find_map(|event| match event {
        WalkEvent::Enter(candidate) if candidate.kind() == kind => Some(candidate),
        WalkEvent::Enter(_) | WalkEvent::Leave(_) => None,
    })
}

pub(crate) fn outermost_node_ids_matching(
    node: Node<'_>,
    matches: impl FnMut(&str) -> bool,
) -> Vec<usize> {
    outermost_matching_nodes(node, matches)
        .into_iter()
        .map(|candidate| candidate.id())
        .collect()
}

#[cfg(test)]
pub(crate) fn reset_outermost_scan_visits() {
    super::walk::reset_outermost_visits();
}

#[cfg(test)]
pub(crate) fn outermost_scan_visits() -> usize {
    super::walk::outermost_visits()
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
