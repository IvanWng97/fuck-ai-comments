use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;

use tree_sitter::{Node, Tree};

use super::container::{
    ContainerSpec, EmbeddedLanguage, EmbeddedRegion, analyze, astro_script_region,
    astro_style_region, parse_file as parse,
};
use super::tree;
use super::walk::{WalkEvent, events};
use crate::config::PolicyConfig;
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{ParsedFile, Span};

#[derive(Clone, Copy)]
enum Astro {
    Strict,
    Recovering(RecoveredFrontmatter),
}

#[derive(Clone, Copy)]
struct RecoveredFrontmatter {
    body_start: usize,
    fence_start: usize,
    start_line: usize,
    end_line: usize,
}

impl RecoveredFrontmatter {
    fn new(source: &str, body_start: usize, fence_start: usize) -> Self {
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

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
    policy: &PolicyConfig,
) -> Result<Vec<Finding>, AnalysisError> {
    retry_with_frontmatter_recovery(path, source, |spec| {
        analyze(path, source, selection, spec, policy)
    })
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    retry_with_frontmatter_recovery(path, source, |spec| parse(path, source, spec))
}

fn retry_with_frontmatter_recovery<T>(
    path: &Path,
    source: &str,
    mut operation: impl FnMut(Astro) -> Result<T, AnalysisError>,
) -> Result<T, AnalysisError> {
    match operation(Astro::Strict) {
        Err(error @ AnalysisError::Parse { .. }) => match recover_frontmatter(path, source)? {
            Some(frontmatter) => operation(Astro::Recovering(frontmatter)),
            None => Err(error),
        },
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

    fn outer_source<'source>(
        self,
        source: &'source str,
    ) -> Result<Cow<'source, str>, AnalysisError> {
        match self {
            Self::Strict => Ok(Cow::Borrowed(source)),
            Self::Recovering(frontmatter) => mask_frontmatter(source, frontmatter).map(Cow::Owned),
        }
    }

    fn validate_outer(self, path: &Path, source: &str, tree: &Tree) -> Result<(), AnalysisError> {
        if matches!(self, Self::Strict) && frontmatter_has_ambiguous_fence(source, tree) {
            Err(astro_parse_error(path))
        } else {
            Ok(())
        }
    }

    fn embedded_region(self, node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
        if node.kind() == "frontmatter_js_block" {
            let outer_span = Span::from_node(node);
            return Some(EmbeddedRegion {
                span: match self {
                    Self::Strict => outer_span.clone(),
                    Self::Recovering(frontmatter) => frontmatter.span(),
                },
                outer_span,
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
) -> Result<Option<RecoveredFrontmatter>, AnalysisError> {
    let opening_start = {
        let tree = tree::parse_recovering(
            path,
            source,
            "Astro",
            tree_sitter_astro_next::LANGUAGE.into(),
        )?;
        #[cfg(test)]
        let _tree_guard = RecoveryTreeGuard::new();
        frontmatter_opening_start(&tree)
    };
    let Some(opening_start) = opening_start else {
        return Ok(None);
    };
    let body_start = opening_start
        .checked_add(FRONTMATTER_FENCE.len())
        .ok_or_else(|| astro_parse_error(path))?;
    if source.get(opening_start..body_start) != Some(FRONTMATTER_FENCE) {
        return Err(astro_parse_error(path));
    }

    // Astro misreads regex backticks, so TypeScript lexical nodes establish the fence.
    let body_and_markup = &source[body_start..];
    let literal_intervals = {
        let typescript = tree::parse_recovering(
            path,
            body_and_markup,
            "TypeScript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        )?;
        #[cfg(test)]
        let _tree_guard = RecoveryTreeGuard::new();
        outermost_typescript_literal_intervals(typescript.root_node())
    };
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
        {
            let _body_tree = tree::parse(
                path,
                body,
                "TypeScript",
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            )?;
            #[cfg(test)]
            let _tree_guard = RecoveryTreeGuard::new();
        }
        return Ok(Some(RecoveredFrontmatter::new(
            source,
            body_start,
            fence_start,
        )));
    }
    Err(astro_parse_error(path))
}

fn frontmatter_opening_start(tree: &Tree) -> Option<usize> {
    events(tree.root_node()).find_map(|event| {
        let WalkEvent::Enter(node) = event else {
            return None;
        };
        (node.kind() == FRONTMATTER_FENCE).then_some(node.start_byte())
    })
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
    static LIVE_RECOVERY_TREES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PEAK_RECOVERY_TREES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
struct RecoveryTreeGuard;

#[cfg(test)]
impl RecoveryTreeGuard {
    fn new() -> Self {
        let live = LIVE_RECOVERY_TREES.with(|trees| {
            let live = trees.get() + 1;
            trees.set(live);
            live
        });
        PEAK_RECOVERY_TREES.with(|peak| peak.set(peak.get().max(live)));
        Self
    }
}

#[cfg(test)]
impl Drop for RecoveryTreeGuard {
    fn drop(&mut self) {
        LIVE_RECOVERY_TREES.with(|trees| trees.set(trees.get() - 1));
    }
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

#[cfg(test)]
fn reset_recovery_tree_peak() {
    LIVE_RECOVERY_TREES.with(|trees| trees.set(0));
    PEAK_RECOVERY_TREES.with(|peak| peak.set(0));
}

#[cfg(test)]
fn peak_recovery_trees() -> usize {
    PEAK_RECOVERY_TREES.with(std::cell::Cell::get)
}

fn mask_frontmatter(
    source: &str,
    frontmatter: RecoveredFrontmatter,
) -> Result<String, AnalysisError> {
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
    masked.push_str(&source[..frontmatter.body_start]);
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        literal_tree_visits, peak_recovery_trees, reset_literal_tree_visits,
        reset_recovery_tree_peak,
    };
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

    #[test]
    fn recovery_keeps_only_one_cst_alive() {
        let source = nested_template_with_fake_fences(64, 64);
        reset_recovery_tree_peak();

        analyze_all(SourceFile {
            path: Path::new("deep.astro"),
            text: &source,
        })
        .expect("valid nested templates should recover");

        assert_eq!(peak_recovery_trees(), 1);
    }
}
