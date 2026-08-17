//! Owner-aware comment policy shared by the CLI and editor integrations.

use std::collections::BTreeSet;
use std::path::Path;

use tree_sitter::{Node, Parser};

const FUNCTION_COMMENT_ABSOLUTE_MAX: usize = 8;
const FUNCTION_CODE_LINES_PER_COMMENT: usize = 4;

/// One required policy violation.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct Finding {
    /// Repository-relative file path.
    pub path: String,
    /// One-based source line.
    pub line: usize,
    /// Stable machine-readable rule identifier.
    pub rule: &'static str,
    /// Human-readable remediation.
    pub message: String,
}

/// Changed lines and deletion anchors used to select affected owners.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Added or edited lines in the new source.
    pub changed: BTreeSet<usize>,
    /// New lines plus anchors for deletion-only hunks.
    pub owners: BTreeSet<usize>,
}

impl Selection {
    /// Select every physical line in `source`.
    #[must_use]
    pub fn all(source: &str) -> Self {
        let lines: BTreeSet<_> = (1..=source.lines().count()).collect();
        Self {
            changed: lines.clone(),
            owners: lines,
        }
    }
}

/// Analyze one supported source file.
///
/// # Errors
///
/// Returns an error when the file extension is unsupported or parsing fails.
pub fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("rs") => analyze_rust(path, source, selection),
        _ => Err(AnalysisError::Unsupported(path.display().to_string())),
    }
}

/// A source file could not be analyzed without guessing.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    /// The file extension has no registered language adapter.
    #[error("unsupported source file: {0}")]
    Unsupported(String),
    /// Tree-sitter rejected the configured grammar.
    #[error("could not initialize the {language} parser: {detail}")]
    ParserInit {
        /// Language adapter being initialized.
        language: &'static str,
        /// Parser error returned by Tree-sitter.
        detail: String,
    },
    /// The source contains syntax errors, so owner relationships are unreliable.
    #[error("could not parse {path} as {language}")]
    Parse {
        /// File that failed to parse.
        path: String,
        /// Language selected from the file extension.
        language: &'static str,
    },
}

#[derive(Debug, Clone)]
struct Span {
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
}

impl Span {
    fn from_node(node: Node<'_>) -> Self {
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

    fn lines(&self) -> impl Iterator<Item = usize> {
        self.start_line..=self.end_line
    }
}

#[derive(Debug)]
struct Function {
    span: Span,
    name: String,
}

#[derive(Debug, Clone)]
struct Comment {
    span: Span,
    text: String,
}

fn analyze_rust(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|error| AnalysisError::ParserInit {
            language: "Rust",
            detail: error.to_string(),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AnalysisError::Parse {
            path: path.display().to_string(),
            language: "Rust",
        })?;
    if tree.root_node().has_error() {
        return Err(AnalysisError::Parse {
            path: path.display().to_string(),
            language: "Rust",
        });
    }

    let mut functions = Vec::new();
    let mut comments = Vec::new();
    collect_rust_nodes(tree.root_node(), source, &mut functions, &mut comments);
    Ok(function_findings(
        path,
        source,
        selection,
        &functions,
        &comments,
        &["SAFETY:"],
    ))
}

fn collect_rust_nodes(
    node: Node<'_>,
    source: &str,
    functions: &mut Vec<Function>,
    comments: &mut Vec<Comment>,
) {
    match node.kind() {
        "function_item" => {
            let name = node
                .child_by_field_name("name")
                .and_then(|name| name.utf8_text(source.as_bytes()).ok())
                .unwrap_or("<anonymous>")
                .to_owned();
            functions.push(Function {
                span: Span::from_node(node),
                name,
            });
        }
        "line_comment" | "block_comment" => {
            let text = node
                .utf8_text(source.as_bytes())
                .unwrap_or_default()
                .to_owned();
            comments.push(Comment {
                span: Span::from_node(node),
                text,
            });
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_rust_nodes(child, source, functions, comments);
    }
}

fn function_findings(
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
