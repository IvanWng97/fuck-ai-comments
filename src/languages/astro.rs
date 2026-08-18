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
    Strict,
    Recovering,
}

const FRONTMATTER_FENCE: &str = "---";

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    retry_with_frontmatter_recovery(|spec| analyze(path, source, selection, spec))
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    retry_with_frontmatter_recovery(|spec| parse(path, source, spec))
}

fn retry_with_frontmatter_recovery<T>(
    mut operation: impl FnMut(Astro) -> Result<T, AnalysisError>,
) -> Result<T, AnalysisError> {
    match operation(Astro::Strict) {
        Err(AnalysisError::Parse { .. }) => operation(Astro::Recovering),
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
        if matches!(self, Self::Strict) {
            let tree = tree::parse(path, source, self.label(), self.grammar())?;
            if frontmatter_has_ambiguous_fence(source, &tree) {
                return Err(astro_parse_error(path));
            }
            return Ok(tree);
        }
        let tree = tree::parse_recovering(path, source, self.label(), self.grammar())?;
        // Astro misreads regex backticks, so TypeScript lexical nodes establish the fence.
        match mask_frontmatter(path, source, &tree)? {
            Some(masked) => tree::parse(path, &masked, self.label(), self.grammar()),
            None => tree::reject_errors(path, self.label(), tree),
        }
    }

    fn embedded_region(self, node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
        if node.kind() == "frontmatter_js_block" {
            return Some(EmbeddedRegion {
                span: Span::from_node(node),
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
                .split_inclusive('\n')
                .filter(|line| is_frontmatter_fence(line))
                .nth(2)
                .is_some()
        })
}

fn mask_frontmatter(
    path: &Path,
    source: &str,
    tree: &Tree,
) -> Result<Option<String>, AnalysisError> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let Some(frontmatter) = root
        .named_children(&mut cursor)
        .find(|node| node.kind() == "frontmatter")
    else {
        return Ok(None);
    };
    let opening_start = frontmatter.start_byte();
    let opening_len = source[opening_start..]
        .find('\n')
        .map(|offset| offset + 1)
        .ok_or_else(|| astro_parse_error(path))?;
    let body_start = opening_start
        .checked_add(opening_len)
        .ok_or_else(|| astro_parse_error(path))?;
    if !is_frontmatter_fence(&source[opening_start..body_start]) {
        return Err(astro_parse_error(path));
    }

    let body_and_markup = &source[body_start..];
    let typescript = tree::parse_recovering(
        path,
        body_and_markup,
        "TypeScript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
    )?;
    let literal_intervals = outermost_typescript_literal_intervals(typescript.root_node());
    let mut literal_index = 0;
    let mut line_start = body_start;
    for line in body_and_markup.split_inclusive('\n') {
        if let Some(fence_offset) = frontmatter_fence_offset(line) {
            let relative_offset = line_start
                .checked_sub(body_start)
                .and_then(|offset| offset.checked_add(fence_offset))
                .ok_or_else(|| astro_parse_error(path))?;
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
                .is_some_and(|interval| {
                    interval.start <= relative_offset && relative_end <= interval.end
                })
            {
                line_start += line.len();
                continue;
            }
            let body = &source[body_start..line_start];
            tree::parse(
                path,
                body,
                "TypeScript",
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            )?;
            return Ok(Some(mask_frontmatter_body(
                source,
                body_start,
                line_start,
                fence_offset,
            )?));
        }
        line_start += line.len();
    }
    Err(astro_parse_error(path))
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

fn is_frontmatter_fence(line: &str) -> bool {
    frontmatter_fence_offset(line).is_some()
}

fn frontmatter_fence_offset(line: &str) -> Option<usize> {
    let content = line_content(line);
    let candidate = content.trim_start_matches([' ', '\t']);
    candidate
        .starts_with(FRONTMATTER_FENCE)
        .then_some(content.len() - candidate.len())
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

fn line_content(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn mask_frontmatter_body(
    source: &str,
    body_start: usize,
    fence_line_start: usize,
    fence_offset: usize,
) -> Result<String, AnalysisError> {
    let fence_end = fence_line_start
        .checked_add(fence_offset)
        .and_then(|offset| offset.checked_add(FRONTMATTER_FENCE.len()))
        .ok_or_else(|| AnalysisError::Invariant("Astro fence offset overflowed".to_owned()))?;
    // Byte length and newlines stay fixed because later nodes index the original source.
    let mut masked = String::with_capacity(source.len());
    masked.push_str(&source[..body_start]);
    for byte in &source.as_bytes()[body_start..fence_line_start] {
        masked.push(match byte {
            b'\r' => '\r',
            b'\n' => '\n',
            _ => ' ',
        });
    }
    masked.push_str(FRONTMATTER_FENCE);
    masked.extend(std::iter::repeat_n(' ', fence_offset));
    masked.push_str(&source[fence_end..]);
    Ok(masked)
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
