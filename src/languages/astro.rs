use std::ops::Range;
use std::path::Path;

use tree_sitter::{Node, Tree};

use super::container::{
    ContainerSpec, EmbeddedLanguage, EmbeddedRegion, analyze, astro_script_region,
    astro_style_region, parse_file as parse,
};
use super::tree;
use super::walk::{WalkEvent, events};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{ParsedFile, Span};

#[derive(Clone, Copy)]
enum Astro {
    Strict { opening_start: Option<usize> },
    Recovering(RecoveredFrontmatter),
}

#[derive(Clone, Copy)]
struct RecoveredFrontmatter {
    body_start: usize,
    fence_start: usize,
    start_line: usize,
    end_line: usize,
    mask_opening: bool,
}

impl RecoveredFrontmatter {
    fn new(source: &str, body_start: usize, fence_start: usize, mask_opening: bool) -> Self {
        let start_line = source.as_bytes()[..body_start]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count()
            + 1;
        let end_line = start_line
            + source.as_bytes()[body_start..fence_start]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count();
        Self {
            body_start,
            fence_start,
            start_line,
            end_line,
            mask_opening,
        }
    }

    fn span(self) -> Span {
        Span {
            start_byte: self.body_start,
            end_byte: self.fence_start,
            start_line: self.start_line,
            end_line: self.end_line,
        }
    }
}

const FRONTMATTER_FENCE: &str = "---";
const HTML_COMMENT_OPEN: &[u8] = b"<!--";
const HTML_COMMENT_CLOSE_DASHES: usize = 2;
const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    retry_with_frontmatter_recovery(path, source, |spec| analyze(path, source, selection, spec))
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    retry_with_frontmatter_recovery(path, source, |spec| parse(path, source, spec))
}

fn retry_with_frontmatter_recovery<T>(
    path: &Path,
    source: &str,
    mut operation: impl FnMut(Astro) -> Result<T, AnalysisError>,
) -> Result<T, AnalysisError> {
    let opening_start = frontmatter_opening_start(source);
    match operation(Astro::Strict { opening_start }) {
        Err(error @ AnalysisError::Parse { .. }) => {
            let Some(opening_start) = opening_start else {
                return Err(error);
            };
            let frontmatter = recover_frontmatter(path, source, opening_start)?;
            operation(Astro::Recovering(frontmatter))
        }
        result => result,
    }
}

impl ContainerSpec for Astro {
    fn label(self) -> &'static str {
        "Astro"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_astro_next::LANGUAGE.into()
    }

    fn parse_outer(self, path: &Path, source: &str) -> Result<Tree, AnalysisError> {
        match self {
            Self::Strict { opening_start } => {
                let tree = tree::parse(path, source, self.label(), self.grammar())?;
                if parsed_frontmatter_opening_start(&tree) != opening_start
                    || frontmatter_has_ambiguous_fence(source, &tree)
                {
                    return Err(astro_parse_error(path));
                }
                Ok(tree)
            }
            Self::Recovering(frontmatter) => {
                let masked = mask_frontmatter(source, frontmatter)?;
                tree::parse(path, &masked, self.label(), self.grammar())
            }
        }
    }

    fn embedded_region(self, node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
        if node.kind() == "frontmatter_js_block" {
            return Some(EmbeddedRegion {
                span: match self {
                    Self::Strict { .. } => Span::from_node(node),
                    Self::Recovering(frontmatter) => frontmatter.span(),
                },
                language: EmbeddedLanguage::TypeScript,
                owner_name: "<frontmatter>",
            });
        }
        astro_script_region(node, source).or_else(|| astro_style_region(node, source))
    }
}

fn frontmatter_has_ambiguous_fence(source: &str, tree: &Tree) -> bool {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let Some(frontmatter) = root
        .named_children(&mut cursor)
        .find(|node| node.kind() == "frontmatter")
    else {
        return false;
    };
    source
        .get(frontmatter.byte_range())
        .is_some_and(|frontmatter| {
            frontmatter
                .match_indices(FRONTMATTER_FENCE)
                .nth(2)
                .is_some()
        })
}

fn recover_frontmatter(
    path: &Path,
    source: &str,
    opening_start: usize,
) -> Result<RecoveredFrontmatter, AnalysisError> {
    let tree = tree::parse_recovering(
        path,
        source,
        "Astro",
        tree_sitter_astro_next::LANGUAGE.into(),
    )?;
    let mask_opening = parsed_frontmatter_opening_start(&tree) != Some(opening_start)
        || prefix_has_bang_closed_comment(source, opening_start);
    let body_start = opening_start
        .checked_add(FRONTMATTER_FENCE.len())
        .ok_or_else(|| astro_parse_error(path))?;
    if source.get(opening_start..body_start) != Some(FRONTMATTER_FENCE) {
        return Err(astro_parse_error(path));
    }

    // Astro misreads regex backticks, so TypeScript lexical nodes establish the fence.
    let body_and_markup = &source[body_start..];
    let typescript = tree::parse_recovering(
        path,
        body_and_markup,
        "TypeScript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
    )?;
    let literal_intervals = outermost_typescript_literal_intervals(typescript.root_node());
    let mut literal_index = 0;
    for (relative_offset, _) in body_and_markup.match_indices(FRONTMATTER_FENCE) {
        let relative_end = relative_offset
            .checked_add(FRONTMATTER_FENCE.len())
            .ok_or_else(|| astro_parse_error(path))?;
        while literal_intervals
            .get(literal_index)
            .is_some_and(|interval| interval.end <= relative_offset)
        {
            literal_index += 1;
        }
        if literal_intervals
            .get(literal_index)
            .is_some_and(|interval| interval.start < relative_end && relative_offset < interval.end)
        {
            continue;
        }
        let fence_start = body_start
            .checked_add(relative_offset)
            .ok_or_else(|| astro_parse_error(path))?;
        let body = &source[body_start..fence_start];
        tree::parse(
            path,
            body,
            "TypeScript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        )?;
        return Ok(RecoveredFrontmatter::new(
            source,
            body_start,
            fence_start,
            mask_opening,
        ));
    }
    Err(astro_parse_error(path))
}

fn parsed_frontmatter_opening_start(tree: &Tree) -> Option<usize> {
    let root = tree.root_node();
    let mut root_cursor = root.walk();
    let frontmatter = root
        .named_children(&mut root_cursor)
        .find(|node| node.kind() == "frontmatter")?;
    let mut frontmatter_cursor = frontmatter.walk();
    frontmatter
        .children(&mut frontmatter_cursor)
        .find(|node| node.kind() == FRONTMATTER_FENCE)
        .map(|node| node.start_byte())
}

fn frontmatter_opening_start(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut offset = 0;
    let mut segment_has_text = false;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.starts_with(FRONTMATTER_FENCE.as_bytes()) {
            return Some(offset);
        }
        if offset == 0 && remaining.starts_with(UTF8_BOM) {
            offset += UTF8_BOM.len();
            continue;
        }
        match bytes[offset] {
            b'<' if is_markup_declaration(remaining) => {
                if segment_has_text {
                    return None;
                }
                offset = markup_declaration_end(bytes, offset)?;
                segment_has_text = false;
            }
            b'<' if starts_element(remaining) => return None,
            b'{' | b'}' | b'\'' | b'"' | b'`' | b'/' => return None,
            byte if byte.is_ascii_whitespace() => offset += 1,
            _ => {
                segment_has_text = true;
                offset += 1;
            }
        }
    }
    None
}

fn is_markup_declaration(source: &[u8]) -> bool {
    source.starts_with(HTML_COMMENT_OPEN)
        || source
            .get(1)
            .is_some_and(|byte| matches!(byte, b'!' | b'?'))
}

fn starts_element(source: &[u8]) -> bool {
    source
        .get(1)
        .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'/' | b'>'))
}

fn markup_declaration_end(source: &[u8], start: usize) -> Option<usize> {
    let remaining = &source[start..];
    if remaining.starts_with(HTML_COMMENT_OPEN) {
        return html_comment_end(source, start + HTML_COMMENT_OPEN.len());
    }
    let content_start = start + 2;
    find_bytes(&source[content_start..], b">").map(|relative| content_start + relative + 1)
}

fn html_comment_end(source: &[u8], start: usize) -> Option<usize> {
    let mut offset = start;
    let mut dash_count: usize = 0;
    while offset < source.len() {
        match source[offset] {
            b'-' => dash_count = dash_count.saturating_add(1),
            b'>' if dash_count >= HTML_COMMENT_CLOSE_DASHES => return Some(offset + 1),
            b'!' if dash_count >= HTML_COMMENT_CLOSE_DASHES
                && source.get(offset + 1) == Some(&b'>') =>
            {
                return Some(offset + 2);
            }
            _ => dash_count = 0,
        }
        offset += 1;
    }
    None
}

fn prefix_has_bang_closed_comment(source: &str, end: usize) -> bool {
    let bytes = &source.as_bytes()[..end];
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset..].starts_with(HTML_COMMENT_OPEN) {
            let Some(comment_end) = html_comment_end(bytes, offset + HTML_COMMENT_OPEN.len())
            else {
                return false;
            };
            if bytes.get(comment_end - 2) == Some(&b'!') {
                return true;
            }
            offset = comment_end;
        } else {
            offset += 1;
        }
    }
    false
}

fn find_bytes(source: &[u8], needle: &[u8]) -> Option<usize> {
    source
        .windows(needle.len())
        .position(|window| window == needle)
}

fn outermost_typescript_literal_intervals(root: Node<'_>) -> Vec<Range<usize>> {
    let mut intervals = Vec::new();
    for event in events(root) {
        let WalkEvent::Enter(node) = event else {
            continue;
        };
        #[cfg(test)]
        record_literal_tree_visit();
        if !matches!(
            node.kind(),
            "comment" | "regex" | "string" | "template_string"
        ) {
            continue;
        }
        let interval = node.byte_range();
        if intervals
            .last()
            .is_none_or(|outer: &Range<usize>| outer.end <= interval.start)
        {
            intervals.push(interval);
        }
    }
    intervals
}

fn astro_parse_error(path: &Path) -> AnalysisError {
    AnalysisError::Parse {
        path: path.display().to_string(),
        language: "Astro",
    }
}

#[cfg(test)]
thread_local! {
    static LITERAL_TREE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_literal_tree_visit() {
    LITERAL_TREE_VISITS.with(|visits| visits.set(visits.get() + 1));
}

#[cfg(test)]
fn reset_literal_tree_visits() {
    LITERAL_TREE_VISITS.with(|visits| visits.set(0));
}

#[cfg(test)]
fn literal_tree_visits() -> usize {
    LITERAL_TREE_VISITS.with(std::cell::Cell::get)
}

fn mask_frontmatter(
    source: &str,
    frontmatter: RecoveredFrontmatter,
) -> Result<String, AnalysisError> {
    let opening_start = frontmatter
        .body_start
        .checked_sub(FRONTMATTER_FENCE.len())
        .ok_or_else(|| AnalysisError::Invariant("Astro fence offset underflowed".to_owned()))?;
    let fence_end = frontmatter
        .fence_start
        .checked_add(FRONTMATTER_FENCE.len())
        .ok_or_else(|| AnalysisError::Invariant("Astro fence offset overflowed".to_owned()))?;
    let line_start = source[..frontmatter.fence_start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let normalized_fence_start = line_start.max(frontmatter.body_start);
    // Byte length and newlines stay fixed because later nodes index the original source.
    let mut masked = String::with_capacity(source.len());
    if frontmatter.mask_opening {
        push_masked_opening_prefix(&mut masked, &source[..opening_start]);
        masked.push_str(FRONTMATTER_FENCE);
    } else {
        masked.push_str(&source[..frontmatter.body_start]);
    }
    for byte in &source.as_bytes()[frontmatter.body_start..normalized_fence_start] {
        masked.push(match byte {
            b'\r' => '\r',
            b'\n' => '\n',
            _ => ' ',
        });
    }
    masked.push_str(FRONTMATTER_FENCE);
    masked.extend(std::iter::repeat_n(
        ' ',
        frontmatter.fence_start - normalized_fence_start,
    ));
    masked.push_str(&source[fence_end..]);
    Ok(masked)
}

fn push_masked_opening_prefix(masked: &mut String, prefix: &str) {
    let bytes = prefix.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset..].starts_with(HTML_COMMENT_OPEN)
            && let Some(end) = markup_declaration_end(bytes, offset)
        {
            if bytes.get(end - 2) == Some(&b'!') {
                masked.push_str(&prefix[offset..end - 2]);
                masked.push_str("->");
            } else {
                masked.push_str(&prefix[offset..end]);
            }
            offset = end;
            continue;
        }
        masked.push(match bytes[offset] {
            b'\r' => '\r',
            b'\n' => '\n',
            _ => ' ',
        });
        offset += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{literal_tree_visits, reset_literal_tree_visits};
    use crate::{SourceFile, analyze_all, analyze_change};

    fn nested_template_with_fake_fences(depth: usize, fence_count: usize) -> String {
        let mut source = String::new();
        source.push_str("---\nconst trigger = /`/g;\nconst art = ");
        for _ in 0..depth {
            source.push_str("`x${");
        }
        source.push('`');
        source.push('\n');
        for _ in 0..fence_count {
            source.push_str("---\n");
        }
        source.push('`');
        for _ in 0..depth {
            source.push_str("}`");
        }
        source.push_str(";\n---\n<main>{art}</main>\n");
        source
    }

    fn recovery_literal_tree_visits(depth: usize, fence_count: usize) -> usize {
        let source = nested_template_with_fake_fences(depth, fence_count);
        reset_literal_tree_visits();
        analyze_all(SourceFile {
            path: Path::new("deep.astro"),
            text: &source,
        })
        .expect("valid nested templates should recover");
        literal_tree_visits()
    }

    #[test]
    fn valid_frontmatter_analysis_skips_the_recovery_cst_walk() {
        let source = "---\nconst value = 1;\n---\n<main>{value}</main>\n";
        reset_literal_tree_visits();

        analyze_all(SourceFile {
            path: Path::new("Page.astro"),
            text: source,
        })
        .expect("valid frontmatter should use the normal Astro parse");

        assert_eq!(literal_tree_visits(), 0);
    }

    #[test]
    fn valid_frontmatter_change_skips_the_recovery_cst_walk() {
        let before = "---\nconst value = 1;\n---\n<main>{value}</main>\n";
        let after = "---\nconst value = 2;\n---\n<main>{value}</main>\n";
        reset_literal_tree_visits();

        analyze_change(
            SourceFile {
                path: Path::new("Page.astro"),
                text: before,
            },
            SourceFile {
                path: Path::new("Page.astro"),
                text: after,
            },
        )
        .expect("valid frontmatter snapshots should use the normal Astro parse");

        assert_eq!(literal_tree_visits(), 0);
    }

    #[test]
    fn fake_fences_do_not_repeat_a_deep_literal_tree_walk() {
        let shallow_one = recovery_literal_tree_visits(4, 1);
        let shallow_many = recovery_literal_tree_visits(4, 64);
        let deep_one = recovery_literal_tree_visits(64, 1);
        let deep_many = recovery_literal_tree_visits(64, 64);

        assert!(
            shallow_one > 0 && deep_one > 0,
            "the work counter must observe both CST walks"
        );
        assert_eq!(
            [shallow_many, deep_many],
            [shallow_one, deep_one],
            "literal CST work must not multiply with fake-fence count at either depth"
        );
    }
}
