use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tree_sitter::Node;

use crate::model::{Finding, Selection};

const FUNCTION_COMMENT_ABSOLUTE_MAX: usize = 8;
const FUNCTION_CODE_LINES_PER_COMMENT: usize = 4;
const FILE_COMMENT_ABSOLUTE_MAX: usize = 8;
const FILE_CODE_LINES_PER_COMMENT: usize = 16;
const LEAF_COMMENT_MAX_LINES: usize = 3;
const TEMPLATE_COMMENT_MAX_LINES: usize = 3;

#[derive(Debug, Clone)]
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

    fn contains(&self, other: &Self) -> bool {
        self.start_byte <= other.start_byte && other.end_byte <= self.end_byte
    }

    fn byte_len(&self) -> usize {
        self.end_byte - self.start_byte
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = usize> {
        self.start_line..=self.end_line
    }
}

#[derive(Debug)]
pub(crate) struct Function {
    pub(crate) span: Span,
    pub(crate) name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Comment {
    pub(crate) span: Span,
    pub(crate) kind: CommentKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CommentKind {
    Narrative,
    ToolDirective,
    SafetyProof,
    PublicDocs,
}

#[derive(Debug)]
pub(crate) struct Leaf {
    pub(crate) span: Span,
    pub(crate) name: String,
}

struct TreeOwnership {
    functions: Vec<Vec<Comment>>,
    function_budget: Vec<Vec<Comment>>,
    leaves: Vec<Vec<Comment>>,
    file: Vec<Comment>,
}

pub(crate) fn tree_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    functions: &[Function],
    leaves: &[Leaf],
    comments: &[Comment],
    language: &str,
) -> Vec<Finding> {
    let ownership = assign_tree_comments(source, functions, leaves, comments);
    let mut findings = function_findings(
        path,
        source,
        selection,
        functions,
        &ownership.functions,
        &ownership.function_budget,
    );
    findings.extend(leaf_findings(
        path,
        selection,
        leaves,
        &ownership.leaves,
        language,
    ));
    findings.extend(file_findings(
        path,
        source,
        selection,
        &ownership.file,
        comments,
    ));
    findings
}

pub(crate) fn template_findings(
    path: &Path,
    selection: &Selection,
    comments: &[Comment],
) -> Vec<Finding> {
    comments
        .iter()
        .filter(|comment| {
            comment.span.lines().count() > TEMPLATE_COMMENT_MAX_LINES
                && comment
                    .span
                    .lines()
                    .any(|line| selection.owners.contains(&line))
        })
        .map(|comment| Finding {
            path: path.display().to_string(),
            line: comment.span.start_line,
            rule: "comment-policy/template-comment-budget",
            message: format!(
                "template comment spans {} lines; allowance is {TEMPLATE_COMMENT_MAX_LINES}",
                comment.span.lines().count()
            ),
        })
        .collect()
}

fn function_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    functions: &[Function],
    owned: &[Vec<Comment>],
    budget: &[Vec<Comment>],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for ((function, comments), budget_comments) in functions.iter().zip(owned).zip(budget) {
        let mut owner_comments = comments.clone();
        let mut budget_comments = budget_comments.clone();
        budget_comments.sort_by_key(|comment| comment.span.start_byte);
        let owner_selected = function
            .span
            .lines()
            .any(|line| selection.owners.contains(&line))
            || budget_comments.iter().any(|comment| {
                comment
                    .span
                    .lines()
                    .any(|line| selection.owners.contains(&line))
            });
        if budget_comments.is_empty() || !owner_selected {
            continue;
        }
        owner_comments.sort_by_key(|comment| comment.span.start_byte);
        let budget_comment_lines = comment_lines(&budget_comments);
        let code_touched = function
            .span
            .lines()
            .any(|line| selection.owners.contains(&line) && !budget_comment_lines.contains(&line));
        if code_touched {
            for comment in &owner_comments {
                let comment_touched = comment
                    .span
                    .lines()
                    .any(|line| selection.changed.contains(&line));
                if !comment_touched {
                    findings.push(Finding {
                        path: path.display().to_string(),
                        line: comment.span.start_line,
                        rule: "comment-policy/comment-owner-changed",
                        message: format!(
                            "function `{}` changed while this comment did not; edit or delete it to attest that it remains true",
                            function.name
                        ),
                    });
                }
            }
        }

        let narrative: Vec<Comment> = budget_comments
            .iter()
            .filter(|comment| comment.kind == CommentKind::Narrative)
            .cloned()
            .collect();
        let narrative_lines = comment_lines(&narrative);
        if narrative_lines.is_empty() {
            continue;
        }
        for block in comment_blocks(source, &narrative) {
            let lines = comment_lines(&block);
            if lines.len() >= 3 {
                findings.push(Finding {
                    path: path.display().to_string(),
                    line: first_line(&lines),
                    rule: "comment-policy/comment-block-budget",
                    message: format!(
                        "3+-comment run inside function `{}`; split the code or keep the local rationale to at most 2 lines",
                        function.name
                    ),
                });
            }
        }

        let code_lines = code_line_count(source, &function.span, &budget_comments);
        let allowance = FUNCTION_COMMENT_ABSOLUTE_MAX
            .min(1_usize.max(code_lines / FUNCTION_CODE_LINES_PER_COMMENT));
        if narrative_lines.len() > allowance {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&narrative_lines),
                rule: "comment-policy/function-comment-budget",
                message: format!(
                    "function `{}` owns {} comment lines for {code_lines} code lines; allowance is {allowance}",
                    function.name,
                    narrative_lines.len()
                ),
            });
        }
    }
    findings
}

#[derive(Clone, Copy)]
enum TreeOwner {
    Function(usize),
    Leaf(usize),
}

fn assign_tree_comments(
    source: &str,
    functions: &[Function],
    leaves: &[Leaf],
    comments: &[Comment],
) -> TreeOwnership {
    let function_leading: Vec<BTreeSet<usize>> = functions
        .iter()
        .map(|function| preceding_comment_indexes(source, comments, &function.span))
        .collect();
    let leaf_leading: Vec<BTreeSet<usize>> = leaves
        .iter()
        .map(|leaf| preceding_comment_indexes(source, comments, &leaf.span))
        .collect();
    let mut ownership = TreeOwnership {
        functions: vec![Vec::new(); functions.len()],
        function_budget: vec![Vec::new(); functions.len()],
        leaves: vec![Vec::new(); leaves.len()],
        file: Vec::new(),
    };

    for (comment_index, comment) in comments.iter().enumerate() {
        let function_owner = functions
            .iter()
            .enumerate()
            .filter(|(index, function)| {
                let trailing = comment.span.start_line == function.span.end_line
                    && comment.span.start_byte >= function.span.end_byte;
                function.span.contains(&comment.span)
                    || function_leading[*index].contains(&comment_index)
                    || trailing
            })
            .min_by_key(|(_, function)| function.span.byte_len())
            .map(|(index, _)| index);
        if let Some(index) = function_owner {
            ownership.function_budget[index].push(comment.clone());
        }

        let mut best: Option<((usize, u8), TreeOwner)> = None;
        for (index, function) in functions.iter().enumerate() {
            let trailing = comment.span.start_line == function.span.end_line
                && comment.span.start_byte >= function.span.end_byte;
            if function.span.contains(&comment.span)
                || function_leading[index].contains(&comment_index)
                || trailing
            {
                choose_owner(
                    &mut best,
                    (function.span.byte_len(), 1),
                    TreeOwner::Function(index),
                );
            }
        }
        for (index, leaf) in leaves.iter().enumerate() {
            let trailing = comment.span.start_line == leaf.span.end_line
                && comment.span.start_byte >= leaf.span.end_byte;
            if leaf.span.contains(&comment.span)
                || leaf_leading[index].contains(&comment_index)
                || trailing
            {
                choose_owner(&mut best, (leaf.span.byte_len(), 0), TreeOwner::Leaf(index));
            }
        }

        match best.map(|(_, owner)| owner) {
            Some(TreeOwner::Function(index)) => {
                ownership.functions[index].push(comment.clone());
            }
            Some(TreeOwner::Leaf(index)) => ownership.leaves[index].push(comment.clone()),
            None => ownership.file.push(comment.clone()),
        }
    }
    ownership
}

fn choose_owner(best: &mut Option<((usize, u8), TreeOwner)>, key: (usize, u8), owner: TreeOwner) {
    if best.as_ref().is_none_or(|(current, _)| key < *current) {
        *best = Some((key, owner));
    }
}

fn preceding_comment_indexes(source: &str, comments: &[Comment], owner: &Span) -> BTreeSet<usize> {
    let physical: Vec<&str> = source.lines().collect();
    let mut by_line = BTreeMap::new();
    for (index, comment) in comments.iter().enumerate() {
        for line in comment.span.lines() {
            by_line.insert(line, index);
        }
    }

    let mut indexes = BTreeSet::new();
    let mut cursor = owner.start_line.saturating_sub(1);
    while cursor > 0 {
        if physical
            .get(cursor - 1)
            .is_some_and(|line| line.trim().is_empty())
        {
            break;
        }
        let Some(index) = by_line.get(&cursor).copied() else {
            break;
        };
        let comment = &comments[index];
        indexes.insert(index);
        cursor = comment.span.start_line.saturating_sub(1);
    }
    if let Some(nearest) = indexes
        .iter()
        .map(|index| &comments[*index])
        .max_by_key(|comment| comment.span.end_byte)
    {
        let separated_by_code = source
            .get(nearest.span.end_byte..owner.start_byte)
            .is_none_or(|gap| !gap.bytes().all(|byte| byte.is_ascii_whitespace()));
        if separated_by_code {
            indexes.clear();
        }
    }
    indexes
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
        let owned = comment_lines(owned_comments);
        let narrative: Vec<Comment> = owned_comments
            .iter()
            .filter(|comment| comment.kind == CommentKind::Narrative)
            .cloned()
            .collect();
        let narrative_lines = comment_lines(&narrative);
        let owner_touched = leaf
            .span
            .lines()
            .any(|line| selection.owners.contains(&line));
        let any_comment_touched = owned.iter().any(|line| selection.changed.contains(line));
        if !owner_touched && !any_comment_touched {
            continue;
        }
        if owner_touched {
            for comment in owned_comments {
                let comment_touched = comment
                    .span
                    .lines()
                    .any(|line| selection.changed.contains(&line));
                if !comment_touched {
                    findings.push(Finding {
                        path: path.display().to_string(),
                        line: comment.span.start_line,
                        rule: "comment-policy/comment-owner-changed",
                        message: format!(
                            "{language} leaf `{}` changed while this comment did not; edit or delete it to attest that it remains true",
                            leaf.name
                        ),
                    });
                }
            }
        }
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

fn file_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    comments: &[Comment],
    all_comments: &[Comment],
) -> Vec<Finding> {
    if comments.is_empty() || selection.owners.is_empty() {
        return Vec::new();
    }
    let narrative: Vec<Comment> = comments
        .iter()
        .filter(|comment| comment.kind == CommentKind::Narrative)
        .cloned()
        .collect();
    let narrative_lines = comment_lines(&narrative);
    if narrative_lines.is_empty() {
        return Vec::new();
    }

    let mut findings = Vec::new();
    for block in comment_blocks(source, &narrative) {
        let lines = comment_lines(&block);
        if lines.len() >= 3 {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&lines),
                rule: "comment-policy/comment-block-budget",
                message: "3+-comment run at file scope; keep rationale beside its owner or at most 2 lines"
                    .to_owned(),
            });
        }
    }

    let file_span = Span {
        start_byte: 0,
        end_byte: source.len(),
        start_line: 1,
        end_line: source.lines().count().max(1),
    };
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

fn comment_blocks(source: &str, comments: &[Comment]) -> Vec<Vec<Comment>> {
    let mut blocks: Vec<Vec<Comment>> = Vec::new();
    for comment in comments {
        if let Some(block) = blocks.last_mut()
            && block.last().is_some_and(|previous| {
                comment.span.start_line == previous.span.end_line + 1
                    && starts_physical_line(source, previous)
                    && starts_physical_line(source, comment)
            })
        {
            block.push(comment.clone());
        } else {
            blocks.push(vec![comment.clone()]);
        }
    }
    blocks
}

fn starts_physical_line(source: &str, comment: &Comment) -> bool {
    let line_start = source[..comment.span.start_byte]
        .rfind('\n')
        .map_or(0, |index| index + 1);
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

fn code_line_count(source: &str, function: &Span, comments: &[Comment]) -> usize {
    let mut uncommented = source.as_bytes().to_vec();
    for comment in comments {
        uncommented[comment.span.start_byte..comment.span.end_byte].fill(b' ');
    }
    uncommented[function.start_byte..function.end_byte]
        .split(|byte| *byte == b'\n')
        .filter(|line| line.iter().any(|byte| !byte.is_ascii_whitespace()))
        .count()
}

fn first_line(lines: &BTreeSet<usize>) -> usize {
    lines.first().copied().unwrap_or(1)
}
