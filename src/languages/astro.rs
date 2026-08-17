use std::path::Path;

use tree_sitter::{Node, Tree};

use super::container::{
    ContainerSpec, EmbeddedLanguage, EmbeddedRegion, analyze, astro_script_region,
    astro_style_region, parse_file as parse,
};
use super::tree;
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{ParsedFile, Span};

#[derive(Clone, Copy)]
struct Astro;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Astro)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    parse(path, source, Astro)
}

impl ContainerSpec for Astro {
    fn label(self) -> &'static str {
        "Astro"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_astro_next::LANGUAGE.into()
    }

    fn parse_outer(self, path: &Path, source: &str) -> Result<Tree, AnalysisError> {
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
    let mut line_start = body_start;
    for line in body_and_markup.split_inclusive('\n') {
        if let Some(fence_offset) = frontmatter_fence_offset(line) {
            let relative_offset = line_start
                .checked_sub(body_start)
                .and_then(|offset| offset.checked_add(fence_offset))
                .ok_or_else(|| astro_parse_error(path))?;
            if is_typescript_literal(typescript.root_node(), relative_offset) {
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
        .starts_with("---")
        .then_some(content.len() - candidate.len())
}

fn is_typescript_literal(root: Node<'_>, offset: usize) -> bool {
    let end = offset.saturating_add(3);
    let mut current = root.descendant_for_byte_range(offset, end);
    while let Some(node) = current {
        if matches!(
            node.kind(),
            "comment" | "regex" | "string" | "template_string"
        ) {
            return true;
        }
        current = node.parent();
    }
    false
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
        .and_then(|offset| offset.checked_add(3))
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
    masked.push_str("---");
    masked.extend(std::iter::repeat_n(' ', fence_offset));
    masked.push_str(&source[fence_end..]);
    Ok(masked)
}
