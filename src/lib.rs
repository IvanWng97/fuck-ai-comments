//! Owner-aware comment policy shared by the CLI and editor integrations.

use std::collections::BTreeSet;
use std::path::Path;

use tree_sitter::{Node, Parser};

const FUNCTION_COMMENT_ABSOLUTE_MAX: usize = 8;
const FUNCTION_CODE_LINES_PER_COMMENT: usize = 4;
const LEAF_COMMENT_MAX_LINES: usize = 3;
const TEMPLATE_COMMENT_MAX_LINES: usize = 3;

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
        Some("rs") => analyze_tree(path, source, selection, TreeLanguage::Rust),
        Some("py" | "pyi") => analyze_tree(path, source, selection, TreeLanguage::Python),
        Some("js" | "mjs") => analyze_tree(path, source, selection, TreeLanguage::JavaScript),
        Some("jsx") => analyze_tree(path, source, selection, TreeLanguage::JavaScript),
        Some("ts") => analyze_tree(path, source, selection, TreeLanguage::TypeScript),
        Some("tsx") => analyze_tree(path, source, selection, TreeLanguage::Tsx),
        Some("toml") => analyze_toml(path, source, selection),
        Some("html" | "htm") => analyze_container(path, source, selection, ContainerLanguage::Html),
        Some("css") => analyze_container(path, source, selection, ContainerLanguage::Css),
        Some("astro") => analyze_container(path, source, selection, ContainerLanguage::Astro),
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
    /// TOML syntax or string boundaries were invalid.
    #[error("could not parse {path} as TOML: {detail}")]
    Toml {
        /// File that failed to parse.
        path: String,
        /// Parser or lexer error.
        detail: String,
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

#[derive(Debug)]
struct Leaf {
    span: Span,
    name: String,
    public: bool,
}

#[derive(Debug, Clone, Copy)]
enum TreeLanguage {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
}

#[derive(Debug, Clone, Copy)]
enum ContainerLanguage {
    Html,
    Css,
    Astro,
}

impl ContainerLanguage {
    fn label(self) -> &'static str {
        match self {
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Astro => "Astro",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Html => tree_sitter_html::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
            Self::Astro => tree_sitter_astro_next::LANGUAGE.into(),
        }
    }
}

impl TreeLanguage {
    fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript | Self::Tsx => "TypeScript",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn is_function(self, kind: &str) -> bool {
        match self {
            Self::Rust => kind == "function_item",
            Self::Python => kind == "function_definition",
            Self::JavaScript | Self::TypeScript | Self::Tsx => matches!(
                kind,
                "function_declaration"
                    | "function_expression"
                    | "arrow_function"
                    | "method_definition"
            ),
        }
    }

    fn is_comment(self, kind: &str) -> bool {
        match self {
            Self::Rust => matches!(kind, "line_comment" | "block_comment"),
            Self::Python | Self::JavaScript | Self::TypeScript | Self::Tsx => kind == "comment",
        }
    }

    fn directives(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["SAFETY:"],
            Self::Python => &["noqa", "type: ignore", "fmt:", "pragma:", "nosec"],
            Self::JavaScript | Self::TypeScript | Self::Tsx => {
                &["eslint", "@ts-", "istanbul ignore", "c8 ignore"]
            }
        }
    }

    fn leaf_stop_prefixes(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["//!"],
            Self::Python => &[
                "#!",
                "# -*-",
                "# coding",
                "# noqa",
                "# type:",
                "# fmt:",
                "# pragma:",
                "# nosec",
            ],
            Self::JavaScript | Self::TypeScript | Self::Tsx => {
                &["// eslint", "/* eslint", "// @ts-", "/* istanbul", "/* c8"]
            }
        }
    }
}

fn analyze_tree(
    path: &Path,
    source: &str,
    selection: &Selection,
    language: TreeLanguage,
) -> Result<Vec<Finding>, AnalysisError> {
    let mut parser = Parser::new();
    let grammar = language.grammar();
    parser
        .set_language(&grammar)
        .map_err(|error| AnalysisError::ParserInit {
            language: language.label(),
            detail: error.to_string(),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AnalysisError::Parse {
            path: path.display().to_string(),
            language: language.label(),
        })?;
    if tree.root_node().has_error() {
        return Err(AnalysisError::Parse {
            path: path.display().to_string(),
            language: language.label(),
        });
    }

    let mut functions = Vec::new();
    let mut comments = Vec::new();
    let mut leaves = Vec::new();
    collect_tree_nodes(
        tree.root_node(),
        source,
        language,
        0,
        &mut functions,
        &mut comments,
        &mut leaves,
    );
    let mut findings = function_findings(
        path,
        source,
        selection,
        &functions,
        &comments,
        language.directives(),
    );
    findings.extend(leaf_findings(
        path,
        source,
        selection,
        &leaves,
        &comments,
        language.label(),
        language.leaf_stop_prefixes(),
    ));
    Ok(findings)
}

#[derive(Debug)]
struct EmbeddedRegion {
    span: Span,
    language: TreeLanguage,
}

fn analyze_container(
    path: &Path,
    source: &str,
    selection: &Selection,
    language: ContainerLanguage,
) -> Result<Vec<Finding>, AnalysisError> {
    let mut parser = Parser::new();
    let grammar = language.grammar();
    parser
        .set_language(&grammar)
        .map_err(|error| AnalysisError::ParserInit {
            language: language.label(),
            detail: error.to_string(),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AnalysisError::Parse {
            path: path.display().to_string(),
            language: language.label(),
        })?;
    if tree.root_node().has_error() {
        return Err(AnalysisError::Parse {
            path: path.display().to_string(),
            language: language.label(),
        });
    }

    let mut comments = Vec::new();
    let mut regions = Vec::new();
    collect_container_nodes(
        tree.root_node(),
        source,
        language,
        &mut comments,
        &mut regions,
    );
    let mut findings = template_comment_findings(path, selection, &comments);
    for region in regions {
        findings.extend(analyze_embedded_region(path, source, selection, &region)?);
    }
    Ok(findings)
}

fn collect_container_nodes(
    node: Node<'_>,
    source: &str,
    language: ContainerLanguage,
    comments: &mut Vec<Comment>,
    regions: &mut Vec<EmbeddedRegion>,
) {
    if node.kind() == "comment" {
        comments.push(Comment {
            span: Span::from_node(node),
            text: node_text(node, source).to_owned(),
        });
    }
    if matches!(language, ContainerLanguage::Astro) && node.kind() == "frontmatter_js_block" {
        regions.push(EmbeddedRegion {
            span: Span::from_node(node),
            language: TreeLanguage::TypeScript,
        });
    }
    if !matches!(language, ContainerLanguage::Css)
        && node.kind() == "script_element"
        && let Some(raw) = first_descendant_with_kind(node, "raw_text")
    {
        let opening = source
            .get(node.start_byte()..raw.start_byte())
            .unwrap_or_default();
        if let Some(language) = script_language(opening) {
            regions.push(EmbeddedRegion {
                span: Span::from_node(raw),
                language,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_container_nodes(child, source, language, comments, regions);
    }
}

fn script_language(opening_tag: &str) -> Option<TreeLanguage> {
    let normalized = opening_tag.to_ascii_lowercase();
    if normalized.contains("lang=\"ts\"")
        || normalized.contains("lang='ts'")
        || normalized.contains("type=\"text/typescript\"")
        || normalized.contains("type='text/typescript'")
    {
        return Some(TreeLanguage::TypeScript);
    }
    let data_only = [
        "application/json",
        "application/ld+json",
        "importmap",
        "speculationrules",
    ];
    (!data_only.iter().any(|kind| normalized.contains(kind))).then_some(TreeLanguage::JavaScript)
}

fn template_comment_findings(
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

fn analyze_embedded_region(
    path: &Path,
    source: &str,
    selection: &Selection,
    region: &EmbeddedRegion,
) -> Result<Vec<Finding>, AnalysisError> {
    let Some(embedded) = source.get(region.span.start_byte..region.span.end_byte) else {
        return Err(AnalysisError::Parse {
            path: path.display().to_string(),
            language: region.language.label(),
        });
    };
    let line_offset = region.span.start_line - 1;
    let map_lines = |lines: &BTreeSet<usize>| {
        lines
            .iter()
            .filter(|line| region.span.start_line <= **line && **line <= region.span.end_line)
            .map(|line| line - line_offset)
            .collect()
    };
    let embedded_selection = Selection {
        changed: map_lines(&selection.changed),
        owners: map_lines(&selection.owners),
    };
    let mut findings = analyze_tree(path, embedded, &embedded_selection, region.language)?;
    for finding in &mut findings {
        finding.line += line_offset;
    }
    Ok(findings)
}

fn collect_tree_nodes(
    node: Node<'_>,
    source: &str,
    language: TreeLanguage,
    function_depth: usize,
    functions: &mut Vec<Function>,
    comments: &mut Vec<Comment>,
    leaves: &mut Vec<Leaf>,
) {
    let is_function = language.is_function(node.kind());
    if is_function {
        functions.push(Function {
            span: Span::from_node(node),
            name: function_name(node, source),
        });
    }
    if language.is_comment(node.kind()) {
        comments.push(Comment {
            span: Span::from_node(node),
            text: node_text(node, source).to_owned(),
        });
    }
    if let Some(leaf) = leaf_from_node(node, source, language, function_depth) {
        leaves.push(leaf);
    }

    let child_function_depth = function_depth + usize::from(is_function);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tree_nodes(
            child,
            source,
            language,
            child_function_depth,
            functions,
            comments,
            leaves,
        );
    }
}

fn function_name(node: Node<'_>, source: &str) -> String {
    node.child_by_field_name("name")
        .or_else(|| {
            node.parent()
                .and_then(|parent| parent.child_by_field_name("name"))
        })
        .map_or("<anonymous>", |name| node_text(name, source))
        .to_owned()
}

fn leaf_from_node(
    node: Node<'_>,
    source: &str,
    language: TreeLanguage,
    function_depth: usize,
) -> Option<Leaf> {
    match language {
        TreeLanguage::Rust if matches!(node.kind(), "const_item" | "static_item") => {
            let mut cursor = node.walk();
            let public = node
                .children(&mut cursor)
                .any(|child| child.kind() == "visibility_modifier");
            Some(Leaf {
                span: Span::from_node(node),
                name: node
                    .child_by_field_name("name")
                    .map_or("<unknown>", |name| node_text(name, source))
                    .to_owned(),
                public,
            })
        }
        TreeLanguage::Python if node.kind() == "assignment" && function_depth == 0 => {
            let name = node
                .child_by_field_name("left")
                .filter(|left| left.kind() == "identifier")
                .map(|left| node_text(left, source))?;
            let uppercase = name.bytes().any(|byte| byte.is_ascii_uppercase())
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
            uppercase.then(|| Leaf {
                span: Span::from_node(node),
                name: name.to_owned(),
                public: false,
            })
        }
        TreeLanguage::JavaScript | TreeLanguage::TypeScript | TreeLanguage::Tsx
            if node.kind() == "lexical_declaration"
                && node_text(node, source).trim_start().starts_with("const ") =>
        {
            let declarator = first_descendant_with_kind(node, "variable_declarator")?;
            let name = declarator
                .child_by_field_name("name")
                .map_or("<destructured>", |name| node_text(name, source));
            Some(Leaf {
                span: Span::from_node(node),
                name: name.to_owned(),
                public: false,
            })
        }
        _ => None,
    }
}

fn first_descendant_with_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find_map(|child| first_descendant_with_kind(child, kind))
}

fn node_text<'source>(node: Node<'_>, source: &'source str) -> &'source str {
    source.get(node.byte_range()).unwrap_or_default()
}

fn analyze_toml(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    toml::from_str::<toml::Table>(source).map_err(|error| AnalysisError::Toml {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;

    let mut findings = Vec::new();
    let mut pending = Vec::new();
    let mut mode = None;
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let (code, has_comment) =
            toml_line_parts(raw, &mut mode).map_err(|detail| AnalysisError::Toml {
                path: path.display().to_string(),
                detail: detail.to_owned(),
            })?;
        if has_comment && code.trim().is_empty() {
            pending.push(line_number);
            continue;
        }
        if code.trim().is_empty() {
            continue;
        }

        let owner = toml_key(code);
        let mut owned = pending.clone();
        if owner.is_some() && has_comment {
            owned.push(line_number);
        }
        let owner_touched = selection.owners.contains(&line_number);
        let comment_touched = owned.iter().any(|line| selection.changed.contains(line));
        if let Some(owner) = owner {
            if !owned.is_empty() && owner_touched && !comment_touched {
                findings.push(Finding {
                    path: path.display().to_string(),
                    line: owned[0],
                    rule: "comment-policy/comment-owner-changed",
                    message: format!(
                        "TOML key `{owner}` changed while its comment did not; edit or delete the comment to attest that it remains true"
                    ),
                });
            }
            if owned.len() > LEAF_COMMENT_MAX_LINES && (owner_touched || comment_touched) {
                findings.push(Finding {
                    path: path.display().to_string(),
                    line: owned[0],
                    rule: "comment-policy/leaf-comment-budget",
                    message: format!(
                        "{} comment lines own TOML key `{owner}`; allowance is {LEAF_COMMENT_MAX_LINES}",
                        owned.len()
                    ),
                });
            }
        }
        pending.clear();
    }
    if mode.is_some() {
        return Err(AnalysisError::Toml {
            path: path.display().to_string(),
            detail: "unterminated multiline string".to_owned(),
        });
    }
    Ok(findings)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TomlStringMode {
    Basic,
    Literal,
    MultilineBasic,
    MultilineLiteral,
}

fn toml_line_parts<'line>(
    line: &'line str,
    mode: &mut Option<TomlStringMode>,
) -> Result<(&'line str, bool), &'static str> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match *mode {
            Some(TomlStringMode::MultilineBasic) => {
                if bytes[index..].starts_with(b"\"\"\"") {
                    *mode = None;
                    index += 3;
                } else if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else {
                    index += 1;
                }
            }
            Some(TomlStringMode::MultilineLiteral) => {
                if bytes[index..].starts_with(b"'''") {
                    *mode = None;
                    index += 3;
                } else {
                    index += 1;
                }
            }
            Some(TomlStringMode::Basic) => {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else {
                    if bytes[index] == b'\"' {
                        *mode = None;
                    }
                    index += 1;
                }
            }
            Some(TomlStringMode::Literal) => {
                if bytes[index] == b'\'' {
                    *mode = None;
                }
                index += 1;
            }
            None => {
                if bytes[index] == b'#' {
                    return Ok((&line[..index], true));
                }
                if bytes[index..].starts_with(b"\"\"\"") {
                    *mode = Some(TomlStringMode::MultilineBasic);
                    index += 3;
                } else if bytes[index..].starts_with(b"'''") {
                    *mode = Some(TomlStringMode::MultilineLiteral);
                    index += 3;
                } else if bytes[index] == b'\"' {
                    *mode = Some(TomlStringMode::Basic);
                    index += 1;
                } else if bytes[index] == b'\'' {
                    *mode = Some(TomlStringMode::Literal);
                    index += 1;
                } else {
                    index += 1;
                }
            }
        }
    }
    if matches!(mode, Some(TomlStringMode::Basic | TomlStringMode::Literal)) {
        return Err("unterminated single-line string");
    }
    Ok((line, false))
}

fn toml_key(code: &str) -> Option<&str> {
    let stripped = code.trim();
    if stripped.is_empty() || stripped.starts_with('[') {
        return None;
    }

    let bytes = stripped.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        match quote {
            Some(b'\"') => {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'\"' {
                    quote = None;
                }
            }
            Some(b'\'') => {
                if byte == b'\'' {
                    quote = None;
                }
            }
            Some(_) => return None,
            None if matches!(byte, b'\"' | b'\'') => quote = Some(byte),
            None if byte == b'=' => return Some(stripped[..index].trim()),
            None => {}
        }
    }
    None
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

fn leaf_findings(
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

fn preceding_comment_lines(
    source: &str,
    comments: &[Comment],
    owner_start_line: usize,
    stop_prefixes: &[&str],
) -> BTreeSet<usize> {
    let physical: Vec<&str> = source.lines().collect();
    let mut by_line = std::collections::BTreeMap::new();
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
