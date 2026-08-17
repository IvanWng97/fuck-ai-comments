use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{
    CodeToken, Comment, CommentKind, Function, Leaf, ParsedFile, Span, TreeInput, TreeOwner,
    TreeOwnership, TypeOwner, tree_document, tree_findings,
};

use super::walk::{WalkEvent, events};

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
    fn function_span(self, node: Node<'_>, source: &str) -> Option<Span>;
    fn function_namespace(self, _node: Node<'_>, _source: &str) -> Vec<String> {
        Vec::new()
    }
    fn type_owner(self, _node: Node<'_>, _source: &str) -> Option<TypeOwner> {
        None
    }
    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        context: &Self::Context,
    ) -> Option<CommentKind>;
    fn leaf(self, node: Node<'_>, source: &str, function_depth: usize) -> Option<Leaf>;
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
    for event in events(tree.root_node()) {
        match event {
            WalkEvent::Enter(node) => collector.enter(node, spec, &context),
            WalkEvent::Leave(node) => collector.leave(node),
        }
    }
    collector.finish()
}

#[derive(Default)]
struct FactCollector<'source> {
    source: &'source str,
    active: Vec<TreeOwner>,
    active_budgets: Vec<TreeOwner>,
    active_types: Vec<TreeOwner>,
    owner_nodes: Vec<(usize, TreeOwner)>,
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

    fn enter<S: LanguageSpec>(&mut self, node: Node<'_>, spec: S, context: &S::Context) {
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

        let function = spec.function_span(node, self.source).map(|span| {
            let name = function_name(node, self.source);
            let mut identity = spec.function_namespace(node, self.source);
            identity.push(name.clone());
            Function {
                span,
                name,
                identity,
            }
        });
        let type_owner = spec.type_owner(node, self.source);
        let leaf = spec.leaf(node, self.source, self.active_function_depth);
        if usize::from(type_owner.is_some())
            + usize::from(function.is_some())
            + usize::from(leaf.is_some())
            > 1
        {
            self.invariant_error = Some(format!(
                "tree node at bytes {}..{} produced multiple owners",
                node.start_byte(),
                node.end_byte()
            ));
        }

        let mut entered = None;
        if let Some(type_owner) = type_owner {
            let owner = TreeOwner::Type(self.types.len());
            self.types.push(type_owner);
            self.register_owner(owner, node.start_byte());
            entered = Some(owner);
        } else if let Some(function) = function {
            let owner = TreeOwner::Function(self.functions.len());
            self.functions.push(function);
            self.register_owner(owner, node.start_byte());
            self.active_function_depth += 1;
            entered = Some(owner);
        } else if let Some(leaf) = leaf {
            let owner = TreeOwner::Leaf(self.leaves.len());
            self.leaves.push(leaf);
            self.register_owner(owner, node.start_byte());
            entered = Some(owner);
        }
        if let Some(owner) = entered {
            self.owner_nodes.push((node.id(), owner));
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
        let Some((owner_node, _)) = self.owner_nodes.last() else {
            return;
        };
        if *owner_node != node.id() {
            return;
        }
        let Some((_, owner)) = self.owner_nodes.pop() else {
            return;
        };
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

fn function_name(node: Node<'_>, source: &str) -> String {
    node.child_by_field_name("name")
        .or_else(|| {
            first_descendant_with_kind(node, "function_definition")
                .and_then(|function| function.child_by_field_name("name"))
        })
        .or_else(|| {
            node.parent()
                .and_then(|parent| parent.child_by_field_name("name"))
        })
        .map_or("<anonymous>", |name| node_text(name, source))
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
