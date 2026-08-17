use std::collections::BTreeSet;
use std::path::Path;

use tree_sitter::{Language, Node};

use super::{javascript, tree, typescript};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{Comment, Span, template_findings};

pub(crate) trait ContainerSpec: Copy {
    fn label(self) -> &'static str;
    fn grammar(self) -> Language;

    fn is_comment(self, node: Node<'_>) -> bool {
        node.kind() == "comment"
    }

    fn embedded_region(self, _node: Node<'_>, _source: &str) -> Option<EmbeddedRegion> {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum EmbeddedLanguage {
    JavaScript,
    TypeScript,
}

#[derive(Debug)]
pub(crate) struct EmbeddedRegion {
    pub(crate) span: Span,
    pub(crate) language: EmbeddedLanguage,
}

pub(crate) fn analyze<S: ContainerSpec>(
    path: &Path,
    source: &str,
    selection: &Selection,
    spec: S,
) -> Result<Vec<Finding>, AnalysisError> {
    let syntax = tree::parse(path, source, spec.label(), spec.grammar())?;
    let mut comments = Vec::new();
    let mut regions = Vec::new();
    collect_nodes(
        syntax.root_node(),
        source,
        spec,
        &mut comments,
        &mut regions,
    );
    let mut findings = template_findings(path, selection, &comments);
    for region in regions {
        findings.extend(analyze_embedded_region(path, source, selection, &region)?);
    }
    Ok(findings)
}

pub(crate) fn script_region(node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
    if node.kind() != "script_element" {
        return None;
    }
    let raw = tree::first_descendant_with_kind(node, "raw_text")?;
    let opening = source.get(node.start_byte()..raw.start_byte())?;
    script_language(opening).map(|language| EmbeddedRegion {
        span: Span::from_node(raw),
        language,
    })
}

fn collect_nodes<S: ContainerSpec>(
    node: Node<'_>,
    source: &str,
    spec: S,
    comments: &mut Vec<Comment>,
    regions: &mut Vec<EmbeddedRegion>,
) {
    if spec.is_comment(node) {
        comments.push(Comment {
            span: Span::from_node(node),
            text: tree::node_text(node, source).to_owned(),
        });
    }
    if let Some(region) = spec.embedded_region(node, source) {
        regions.push(region);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_nodes(child, source, spec, comments, regions);
    }
}

fn script_language(opening_tag: &str) -> Option<EmbeddedLanguage> {
    let normalized = opening_tag.to_ascii_lowercase();
    if normalized.contains("lang=\"ts\"")
        || normalized.contains("lang='ts'")
        || normalized.contains("type=\"text/typescript\"")
        || normalized.contains("type='text/typescript'")
    {
        return Some(EmbeddedLanguage::TypeScript);
    }
    let data_only = [
        "application/json",
        "application/ld+json",
        "importmap",
        "speculationrules",
    ];
    (!data_only.iter().any(|kind| normalized.contains(kind)))
        .then_some(EmbeddedLanguage::JavaScript)
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
            language: embedded_label(region.language),
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
    let mut findings = match region.language {
        EmbeddedLanguage::JavaScript => {
            javascript::analyze_file(path, embedded, &embedded_selection)?
        }
        EmbeddedLanguage::TypeScript => {
            typescript::analyze_file(path, embedded, &embedded_selection)?
        }
    };
    for finding in &mut findings {
        finding.line += line_offset;
    }
    Ok(findings)
}

fn embedded_label(language: EmbeddedLanguage) -> &'static str {
    match language {
        EmbeddedLanguage::JavaScript => "JavaScript",
        EmbeddedLanguage::TypeScript => "TypeScript",
    }
}
