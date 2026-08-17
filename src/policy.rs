use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tree_sitter::Node;

use crate::model::{Finding, Selection};

const FUNCTION_COMMENT_ABSOLUTE_MAX: usize = 8;
const FUNCTION_CODE_LINES_PER_COMMENT: usize = 4;
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
    pub(crate) text: String,
}

#[derive(Debug)]
pub(crate) struct Leaf {
    pub(crate) span: Span,
    pub(crate) name: String,
    pub(crate) public: bool,
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

pub(crate) fn function_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    functions: &[Function],
    comments: &[Comment],
    directives: &[&str],
) -> Vec<Finding> {
    let mut owned = vec![Vec::new(); functions.len()];
    for comment in comments {
        let owner = functions
            .iter()
            .enumerate()
            .filter(|(_, function)| function.span.contains(&comment.span))
            .min_by_key(|(_, function)| function.span.byte_len())
            .map(|(index, _)| index);
        if let Some(index) = owner {
            owned[index].push(comment.clone());
        }
    }

    let mut findings = Vec::new();
    for (function, mut owner_comments) in functions.iter().zip(owned) {
        if owner_comments.is_empty()
            || !function
                .span
                .lines()
                .any(|line| selection.owners.contains(&line))
        {
            continue;
        }
        owner_comments.sort_by_key(|comment| comment.span.start_byte);
        let all_comment_lines = comment_lines(&owner_comments);
        let code_touched = function
            .span
            .lines()
            .any(|line| selection.owners.contains(&line) && !all_comment_lines.contains(&line));
        let comment_touched = all_comment_lines
            .iter()
            .any(|line| selection.changed.contains(line));
        if code_touched && !comment_touched {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&all_comment_lines),
                rule: "comment-policy/comment-owner-changed",
                message: format!(
                    "function `{}` changed while its comment did not; edit or delete the comment to attest that it remains true",
                    function.name
                ),
            });
        }

        let blocks = comment_blocks(&owner_comments);
        let narrative: Vec<Comment> = blocks
            .iter()
            .filter(|block| !is_directive_block(block, directives))
            .flatten()
            .cloned()
            .collect();
        let narrative_lines = comment_lines(&narrative);
        if narrative_lines.is_empty() {
            continue;
        }
        for block in blocks
            .iter()
            .filter(|block| !is_directive_block(block, directives))
        {
            let lines = comment_lines(block);
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

        let code_lines = code_line_count(source, &function.span, &owner_comments);
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

pub(crate) fn leaf_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    leaves: &[Leaf],
    comments: &[Comment],
    language: &str,
    stop_prefixes: &[&str],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for leaf in leaves {
        let mut owned =
            preceding_comment_lines(source, comments, leaf.span.start_line, stop_prefixes);
        owned.extend(
            comments
                .iter()
                .filter(|comment| {
                    comment.span.start_line == leaf.span.end_line
                        && comment.span.start_byte >= leaf.span.end_byte
                })
                .flat_map(|comment| comment.span.lines()),
        );
        let owner_touched = leaf
            .span
            .lines()
            .any(|line| selection.owners.contains(&line));
        let comment_touched = owned.iter().any(|line| selection.changed.contains(line));
        if !owner_touched && !comment_touched {
            continue;
        }
        if !owned.is_empty() && owner_touched && !comment_touched {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&owned),
                rule: "comment-policy/comment-owner-changed",
                message: format!(
                    "{language} leaf `{}` changed while its comment did not; edit or delete the comment to attest that it remains true",
                    leaf.name
                ),
            });
        }
        if !leaf.public && owned.len() > LEAF_COMMENT_MAX_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&owned),
                rule: "comment-policy/leaf-comment-budget",
                message: format!(
                    "{} comment lines own {language} leaf `{}`; allowance is {LEAF_COMMENT_MAX_LINES}",
                    owned.len(),
                    leaf.name
                ),
            });
        }
    }
    findings
}

fn comment_blocks(comments: &[Comment]) -> Vec<Vec<Comment>> {
    let mut blocks: Vec<Vec<Comment>> = Vec::new();
    for comment in comments {
        if let Some(block) = blocks.last_mut()
            && block
                .last()
                .is_some_and(|previous| comment.span.start_line == previous.span.end_line + 1)
        {
            block.push(comment.clone());
        } else {
            blocks.push(vec![comment.clone()]);
        }
    }
    blocks
}

fn comment_lines(comments: &[Comment]) -> BTreeSet<usize> {
    comments
        .iter()
        .flat_map(|comment| comment.span.lines())
        .collect()
}

fn normalized_comment(comment: &str) -> &str {
    comment
        .trim_start_matches([' ', '\t', '/', '*', '!', '#', '-'])
        .trim()
}

fn is_directive_block(block: &[Comment], directives: &[&str]) -> bool {
    block.first().is_some_and(|comment| {
        let normalized = normalized_comment(&comment.text);
        directives
            .iter()
            .any(|directive| normalized.starts_with(directive))
    })
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

fn preceding_comment_lines(
    source: &str,
    comments: &[Comment],
    owner_start_line: usize,
    stop_prefixes: &[&str],
) -> BTreeSet<usize> {
    let physical: Vec<&str> = source.lines().collect();
    let mut by_line = BTreeMap::new();
    for comment in comments {
        for line in comment.span.lines() {
            by_line.insert(line, comment);
        }
    }

    let mut lines = BTreeSet::new();
    let mut cursor = owner_start_line.saturating_sub(1);
    while cursor > 0 {
        if physical
            .get(cursor - 1)
            .is_some_and(|line| line.trim().is_empty())
        {
            cursor -= 1;
            continue;
        }
        let Some(comment) = by_line.get(&cursor) else {
            break;
        };
        if stop_prefixes
            .iter()
            .any(|prefix| comment.text.trim_start().starts_with(prefix))
        {
            break;
        }
        lines.extend(comment.span.lines());
        cursor = comment.span.start_line.saturating_sub(1);
    }
    lines
}
