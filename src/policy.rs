use std::collections::BTreeSet;
use std::path::Path;

use tree_sitter::Node;

use crate::model::{Finding, OwnerKind, Selection};

const FUNCTION_COMMENT_ABSOLUTE_MAX: usize = 8;
const FUNCTION_CODE_LINES_PER_COMMENT: usize = 4;
const COMMENT_BLOCK_MIN_LINES: usize = 3;
const FILE_COMMENT_ABSOLUTE_MAX: usize = 8;
const FILE_CODE_LINES_PER_COMMENT: usize = 16;
pub(crate) const LEAF_COMMENT_MAX_LINES: usize = 3;
const TEMPLATE_COMMENT_MAX_LINES: usize = 3;
const OWNER_COMMENT_CAP_RULE: &str = "comment-policy/owner-comment-cap";

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Span {
    pub(crate) start_byte: usize,
    pub(crate) end_byte: usize,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
}

impl Span {
    pub(crate) fn from_node(node: Node<'_>) -> Self {
        Self {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
        }
    }

    pub(crate) fn from_comment_node(node: Node<'_>, source: &str) -> Self {
        let mut span = Self::from_node(node);
        while span.end_byte > span.start_byte
            && source.as_bytes()[span.end_byte - 1].is_ascii_whitespace()
            && matches!(source.as_bytes()[span.end_byte - 1], b'\n' | b'\r')
        {
            span.end_byte -= 1;
        }
        if span.end_byte < node.end_byte() && span.end_line > span.start_line {
            span.end_line -= 1;
        }
        span
    }

    pub(crate) fn contains(&self, other: &Self) -> bool {
        self.start_byte <= other.start_byte && other.end_byte <= self.end_byte
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = usize> {
        self.start_line..=self.end_line
    }
}

#[derive(Debug)]
pub(crate) struct Function {
    pub(crate) span: Span,
    pub(crate) name: String,
    pub(crate) identity: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct TypeOwner {
    pub(crate) span: Span,
    pub(crate) name: String,
    pub(crate) identity: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Comment {
    pub(crate) span: Span,
    pub(crate) kind: CommentKind,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum CommentKind {
    Narrative,
    FileNarrative,
    TypeNarrative,
    ToolDirective,
    SafetyProof,
    PublicDocs,
}

impl CommentKind {
    fn is_narrative(self) -> bool {
        matches!(
            self,
            Self::Narrative | Self::FileNarrative | Self::TypeNarrative
        )
    }
}

#[derive(Debug)]
pub(crate) struct Leaf {
    pub(crate) span: Span,
    pub(crate) name: String,
}

pub(crate) struct TreeOwnership {
    pub(crate) function_budget: Vec<Vec<Comment>>,
    pub(crate) function_parents: Vec<Option<TreeOwner>>,
    pub(crate) type_budget: Vec<Vec<Comment>>,
    pub(crate) type_parents: Vec<Option<TreeOwner>>,
    pub(crate) leaves: Vec<Vec<Comment>>,
    pub(crate) leaf_parents: Vec<Option<TreeOwner>>,
    pub(crate) file: Vec<Comment>,
    pub(crate) comment_owners: Vec<Option<TreeOwner>>,
}

#[derive(Clone, Copy)]
pub(crate) struct TreeInput<'input> {
    pub(crate) functions: &'input [Function],
    pub(crate) types: &'input [TypeOwner],
    pub(crate) leaves: &'input [Leaf],
    pub(crate) comments: &'input [Comment],
    pub(crate) ownership: &'input TreeOwnership,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct CodeToken {
    event: CodeEvent,
    pub(crate) kind: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum CodeEvent {
    Enter,
    Atom,
    Leave,
}

impl CodeToken {
    pub(crate) fn enter(kind: &str) -> Self {
        Self {
            event: CodeEvent::Enter,
            kind: kind.to_owned(),
            text: String::new(),
        }
    }

    pub(crate) fn atom(kind: &str, text: &str) -> Self {
        Self {
            event: CodeEvent::Atom,
            kind: kind.to_owned(),
            text: text.to_owned(),
        }
    }

    pub(crate) fn leave(kind: &str) -> Self {
        Self {
            event: CodeEvent::Leave,
            kind: kind.to_owned(),
            text: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OwnerSnapshot {
    pub(crate) kind: OwnerKind,
    pub(crate) name: String,
    pub(crate) identity: Vec<String>,
    pub(crate) span: Span,
    pub(crate) parent: Option<usize>,
    pub(crate) code: Vec<CodeToken>,
}

#[derive(Debug, Clone)]
pub(crate) struct CommentSnapshot {
    pub(crate) kind: CommentKind,
    pub(crate) text: String,
    pub(crate) span: Span,
    pub(crate) owner: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedFile {
    pub(crate) owners: Vec<OwnerSnapshot>,
    pub(crate) comments: Vec<CommentSnapshot>,
}

pub(crate) fn tree_document(
    source: &str,
    input: TreeInput<'_>,
    code: Vec<Vec<CodeToken>>,
) -> ParsedFile {
    let file_span = Span {
        start_byte: 0,
        end_byte: source.len(),
        start_line: 1,
        end_line: source.lines().count().max(1),
    };
    let mut owners =
        Vec::with_capacity(1 + input.functions.len() + input.types.len() + input.leaves.len());
    owners.push(OwnerSnapshot {
        kind: OwnerKind::File,
        name: "<file>".to_owned(),
        identity: vec!["file".to_owned()],
        span: file_span,
        parent: None,
        code: code.first().cloned().unwrap_or_default(),
    });
    owners.extend(input.functions.iter().enumerate().map(|(index, function)| {
        OwnerSnapshot {
            kind: OwnerKind::Function,
            name: function.name.clone(),
            identity: function.identity.clone(),
            span: function.span.clone(),
            parent: input.ownership.function_parents[index]
                .map(|parent| {
                    owner_snapshot_index(parent, input.functions.len(), input.types.len())
                })
                .or(Some(0)),
            code: code.get(index + 1).cloned().unwrap_or_default(),
        }
    }));
    let type_offset = 1 + input.functions.len();
    owners.extend(input.types.iter().enumerate().map(|(index, type_owner)| {
        OwnerSnapshot {
            kind: OwnerKind::Type,
            name: type_owner.name.clone(),
            identity: type_owner.identity.clone(),
            span: type_owner.span.clone(),
            parent: input.ownership.type_parents[index]
                .map(|parent| {
                    owner_snapshot_index(parent, input.functions.len(), input.types.len())
                })
                .or(Some(0)),
            code: code.get(type_offset + index).cloned().unwrap_or_default(),
        }
    }));
    let leaf_offset = type_offset + input.types.len();
    owners.extend(input.leaves.iter().enumerate().map(|(index, leaf)| {
        OwnerSnapshot {
            kind: OwnerKind::Leaf,
            name: leaf.name.clone(),
            identity: vec![leaf.name.clone()],
            span: leaf.span.clone(),
            parent: input.ownership.leaf_parents[index]
                .map(|parent| {
                    owner_snapshot_index(parent, input.functions.len(), input.types.len())
                })
                .or(Some(0)),
            code: code.get(leaf_offset + index).cloned().unwrap_or_default(),
        }
    }));
    let comments = input
        .comments
        .iter()
        .zip(&input.ownership.comment_owners)
        .map(|(comment, owner)| CommentSnapshot {
            kind: comment.kind,
            text: comment.text.clone(),
            span: comment.span.clone(),
            owner: owner.map_or(0, |owner| {
                owner_snapshot_index(owner, input.functions.len(), input.types.len())
            }),
        })
        .collect();
    ParsedFile { owners, comments }
}

fn owner_snapshot_index(owner: TreeOwner, function_count: usize, type_count: usize) -> usize {
    match owner {
        TreeOwner::Function(index) => index + 1,
        TreeOwner::Type(index) => function_count + index + 1,
        TreeOwner::Leaf(index) => function_count + type_count + index + 1,
    }
}

pub(crate) fn tree_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    input: TreeInput<'_>,
    language: &str,
) -> Vec<Finding> {
    let line_starts = LineStarts::new(source);
    let mut findings = function_findings(
        path,
        source,
        &line_starts,
        selection,
        input.functions,
        &input.ownership.function_budget,
    );
    findings.extend(type_findings(
        path,
        source,
        &line_starts,
        selection,
        input.types,
        &input.ownership.type_budget,
    ));
    findings.extend(leaf_findings(
        path,
        selection,
        input.leaves,
        &input.ownership.leaves,
        language,
    ));
    findings.extend(file_findings_with_lines(
        path,
        source,
        &line_starts,
        selection,
        &input.ownership.file,
        input.comments,
    ));
    findings
}

pub(crate) fn template_findings(
    path: &Path,
    selection: &Selection,
    comments: &[Comment],
    owner: &Span,
) -> Vec<Finding> {
    let template_selected =
        selection.selects_owner(OwnerKind::Template, owner.start_byte, owner.end_byte);
    if comments.is_empty() || !template_selected {
        return Vec::new();
    }
    let mut findings = owner_comment_cap_finding(path, OwnerKind::Template, "template", comments)
        .into_iter()
        .collect::<Vec<_>>();
    let narrative: Vec<Comment> = comments
        .iter()
        .filter(|comment| comment.kind.is_narrative())
        .cloned()
        .collect();
    let lines = comment_lines(&narrative);
    if lines.len() <= TEMPLATE_COMMENT_MAX_LINES {
        return findings;
    }
    findings.push(Finding {
        path: path.display().to_string(),
        line: first_line(&lines),
        rule: "comment-policy/template-comment-budget",
        message: format!(
            "template owns {} comment lines; allowance is {TEMPLATE_COMMENT_MAX_LINES}",
            lines.len()
        ),
    });
    findings
}

fn function_findings(
    path: &Path,
    source: &str,
    line_starts: &LineStarts,
    selection: &Selection,
    functions: &[Function],
    budget: &[Vec<Comment>],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (function, budget_comments) in functions.iter().zip(budget) {
        findings.extend(scoped_owner_findings(
            path,
            source,
            line_starts,
            selection,
            ScopedOwner {
                kind: OwnerKind::Function,
                kind_label: "function",
                budget_rule: "comment-policy/function-comment-budget",
                name: &function.name,
                span: &function.span,
            },
            budget_comments,
        ));
    }
    findings
}

fn type_findings(
    path: &Path,
    source: &str,
    line_starts: &LineStarts,
    selection: &Selection,
    types: &[TypeOwner],
    budget: &[Vec<Comment>],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (type_owner, budget_comments) in types.iter().zip(budget) {
        findings.extend(scoped_owner_findings(
            path,
            source,
            line_starts,
            selection,
            ScopedOwner {
                kind: OwnerKind::Type,
                kind_label: "type",
                budget_rule: "comment-policy/type-comment-budget",
                name: &type_owner.name,
                span: &type_owner.span,
            },
            budget_comments,
        ));
    }
    findings
}

struct ScopedOwner<'owner> {
    kind: OwnerKind,
    kind_label: &'static str,
    budget_rule: &'static str,
    name: &'owner str,
    span: &'owner Span,
}

fn scoped_owner_findings(
    path: &Path,
    source: &str,
    line_starts: &LineStarts,
    selection: &Selection,
    owner: ScopedOwner<'_>,
    budget_comments: &[Comment],
) -> Vec<Finding> {
    if budget_comments.is_empty()
        || !selection.selects_owner(owner.kind, owner.span.start_byte, owner.span.end_byte)
    {
        return Vec::new();
    }

    let mut budget_comments = budget_comments.to_vec();
    budget_comments.sort_by_key(|comment| comment.span.start_byte);
    let owner_label = format!("{} `{}`", owner.kind_label, owner.name);
    let mut findings = owner_comment_cap_finding(path, owner.kind, &owner_label, &budget_comments)
        .into_iter()
        .collect::<Vec<_>>();
    let narrative: Vec<Comment> = budget_comments
        .iter()
        .filter(|comment| comment.kind.is_narrative())
        .cloned()
        .collect();
    let narrative_lines = comment_lines(&narrative);
    if narrative_lines.is_empty() {
        return findings;
    }
    for block in comment_blocks(source, line_starts, &narrative) {
        let lines = comment_lines(&block);
        if lines.len() >= COMMENT_BLOCK_MIN_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&lines),
                rule: "comment-policy/comment-block-budget",
                message: format!(
                    "{COMMENT_BLOCK_MIN_LINES}+-comment run inside {owner_label}; split the code or keep the local rationale below {COMMENT_BLOCK_MIN_LINES} lines"
                ),
            });
        }
    }

    let code_lines = code_line_count(source, owner.span, &budget_comments);
    let allowance = FUNCTION_COMMENT_ABSOLUTE_MAX
        .min(1_usize.max(code_lines / FUNCTION_CODE_LINES_PER_COMMENT));
    if narrative_lines.len() > allowance {
        findings.push(Finding {
            path: path.display().to_string(),
            line: first_line(&narrative_lines),
            rule: owner.budget_rule,
            message: format!(
                "{owner_label} owns {} comment lines for {code_lines} code lines; allowance is {allowance}",
                narrative_lines.len()
            ),
        });
    }
    findings
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TreeOwner {
    Function(usize),
    Type(usize),
    Leaf(usize),
}

fn leaf_findings(
    path: &Path,
    selection: &Selection,
    leaves: &[Leaf],
    owned: &[Vec<Comment>],
    language: &str,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (leaf, owned_comments) in leaves.iter().zip(owned) {
        let narrative: Vec<Comment> = owned_comments
            .iter()
            .filter(|comment| comment.kind.is_narrative())
            .cloned()
            .collect();
        let narrative_lines = comment_lines(&narrative);
        let owner_touched =
            selection.selects_owner(OwnerKind::Leaf, leaf.span.start_byte, leaf.span.end_byte);
        if !owner_touched {
            continue;
        }
        findings.extend(owner_comment_cap_finding(
            path,
            OwnerKind::Leaf,
            &format!("{language} leaf `{}`", leaf.name),
            owned_comments,
        ));
        if narrative_lines.len() > LEAF_COMMENT_MAX_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&narrative_lines),
                rule: "comment-policy/leaf-comment-budget",
                message: format!(
                    "{} comment lines own {language} leaf `{}`; allowance is {LEAF_COMMENT_MAX_LINES}",
                    narrative_lines.len(),
                    leaf.name
                ),
            });
        }
    }
    findings
}

pub(crate) fn file_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    comments: &[Comment],
    all_comments: &[Comment],
) -> Vec<Finding> {
    let line_starts = LineStarts::new(source);
    file_findings_with_lines(
        path,
        source,
        &line_starts,
        selection,
        comments,
        all_comments,
    )
}

fn file_findings_with_lines(
    path: &Path,
    source: &str,
    line_starts: &LineStarts,
    selection: &Selection,
    comments: &[Comment],
    all_comments: &[Comment],
) -> Vec<Finding> {
    let file_span = Span {
        start_byte: 0,
        end_byte: source.len(),
        start_line: 1,
        end_line: source.lines().count().max(1),
    };
    if comments.is_empty()
        || !selection.selects_owner(OwnerKind::File, file_span.start_byte, file_span.end_byte)
    {
        return Vec::new();
    }
    let mut findings = owner_comment_cap_finding(path, OwnerKind::File, "file scope", comments)
        .into_iter()
        .collect::<Vec<_>>();
    let narrative: Vec<Comment> = comments
        .iter()
        .filter(|comment| comment.kind.is_narrative())
        .cloned()
        .collect();
    let narrative_lines = comment_lines(&narrative);
    if narrative_lines.is_empty() {
        return findings;
    }

    for block in comment_blocks(source, line_starts, &narrative) {
        let lines = comment_lines(&block);
        if lines.len() >= COMMENT_BLOCK_MIN_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&lines),
                rule: "comment-policy/comment-block-budget",
                message: format!(
                    "{COMMENT_BLOCK_MIN_LINES}+-comment run at file scope; keep rationale beside its owner or below {COMMENT_BLOCK_MIN_LINES} lines"
                ),
            });
        }
    }

    let code_lines = code_line_count(source, &file_span, all_comments);
    let allowance =
        FILE_COMMENT_ABSOLUTE_MAX.min(2_usize.max(code_lines / FILE_CODE_LINES_PER_COMMENT));
    if narrative_lines.len() > allowance {
        findings.push(Finding {
            path: path.display().to_string(),
            line: first_line(&narrative_lines),
            rule: "comment-policy/file-comment-budget",
            message: format!(
                "file scope owns {} comment lines for {code_lines} code lines; allowance is {allowance}",
                narrative_lines.len()
            ),
        });
    }
    findings
}

pub(crate) fn owner_comment_cap_finding(
    path: &Path,
    owner_kind: OwnerKind,
    owner: &str,
    comments: &[Comment],
) -> Option<Finding> {
    let lines: BTreeSet<usize> = comments
        .iter()
        .filter(|comment| comment.kind != CommentKind::PublicDocs)
        .flat_map(|comment| comment.span.lines())
        .collect();
    let narrative_lines: BTreeSet<usize> = comments
        .iter()
        .filter(|comment| comment.kind.is_narrative())
        .flat_map(|comment| comment.span.lines())
        .collect();
    let allowance = match owner_kind {
        OwnerKind::Function | OwnerKind::Type => FUNCTION_COMMENT_ABSOLUTE_MAX,
        OwnerKind::File => FILE_COMMENT_ABSOLUTE_MAX,
        OwnerKind::Leaf | OwnerKind::TomlKey => LEAF_COMMENT_MAX_LINES,
        OwnerKind::Template => TEMPLATE_COMMENT_MAX_LINES,
    };
    (lines.len() > allowance && narrative_lines.len() <= allowance).then(|| Finding {
        path: path.display().to_string(),
        line: first_line(&lines),
        rule: OWNER_COMMENT_CAP_RULE,
        message: format!(
            "{owner} owns {} non-public comment lines; absolute allowance is {allowance}",
            lines.len()
        ),
    })
}

struct LineStarts(Vec<usize>);

impl LineStarts {
    fn new(source: &str) -> Self {
        Self(
            std::iter::once(0)
                .chain(
                    source
                        .bytes()
                        .enumerate()
                        .filter(|(_, byte)| *byte == b'\n')
                        .map(|(index, _)| index + 1),
                )
                .collect(),
        )
    }

    fn for_line(&self, one_based_line: usize) -> usize {
        self.0
            .get(one_based_line.saturating_sub(1))
            .copied()
            .unwrap_or_default()
    }
}

fn comment_blocks(
    source: &str,
    line_starts: &LineStarts,
    comments: &[Comment],
) -> Vec<Vec<Comment>> {
    let mut blocks: Vec<Vec<Comment>> = Vec::new();
    for comment in comments {
        if let Some(block) = blocks.last_mut()
            && block.last().is_some_and(|previous| {
                comment.span.start_line == previous.span.end_line + 1
                    && starts_physical_line(source, line_starts, previous)
                    && starts_physical_line(source, line_starts, comment)
            })
        {
            block.push(comment.clone());
        } else {
            blocks.push(vec![comment.clone()]);
        }
    }
    blocks
}

fn starts_physical_line(source: &str, line_starts: &LineStarts, comment: &Comment) -> bool {
    let line_start = line_starts.for_line(comment.span.start_line);
    source.as_bytes()[line_start..comment.span.start_byte]
        .iter()
        .all(u8::is_ascii_whitespace)
}

fn comment_lines(comments: &[Comment]) -> BTreeSet<usize> {
    comments
        .iter()
        .flat_map(|comment| comment.span.lines())
        .collect()
}

fn code_line_count(source: &str, owner: &Span, comments: &[Comment]) -> usize {
    let mut comments = comments.iter().peekable();
    while comments
        .peek()
        .is_some_and(|comment| comment.span.end_byte <= owner.start_byte)
    {
        comments.next();
    }
    let mut current = comments.next();
    let mut line_has_code = false;
    let mut code_lines = 0;
    for (offset, byte) in source.as_bytes()[owner.start_byte..owner.end_byte]
        .iter()
        .copied()
        .enumerate()
    {
        let absolute = owner.start_byte + offset;
        while current.is_some_and(|comment| comment.span.end_byte <= absolute) {
            current = comments.next();
        }
        if byte == b'\n' {
            code_lines += usize::from(line_has_code);
            line_has_code = false;
        } else if current.is_none_or(|comment| {
            absolute < comment.span.start_byte || comment.span.end_byte <= absolute
        }) && !byte.is_ascii_whitespace()
        {
            line_has_code = true;
        }
    }
    code_lines + usize::from(line_has_code)
}

fn first_line(lines: &BTreeSet<usize>) -> usize {
    lines.first().copied().unwrap_or(1)
}
