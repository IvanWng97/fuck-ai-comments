use std::borrow::Cow;
use std::path::Path;

use tree_sitter::{Language, Node};

use super::walk::{WalkEvent, events};
use super::{css, javascript, tree, typescript};
use crate::model::{AnalysisError, Finding, OwnerKind, Selection};
use crate::policy::{
    CodeToken, Comment, CommentKind, CommentSnapshot, OwnerSnapshot, ParsedFile, Span,
    template_findings,
};

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
    Css,
    JavaScript,
    TypeScript,
}

#[derive(Debug)]
pub(crate) struct EmbeddedRegion {
    pub(crate) span: Span,
    pub(crate) language: EmbeddedLanguage,
    pub(crate) owner_name: &'static str,
}

struct Facts {
    syntax: Vec<CodeToken>,
    comments: Vec<Comment>,
    regions: Vec<EmbeddedRegion>,
}

pub(crate) fn analyze<S: ContainerSpec>(
    path: &Path,
    source: &str,
    selection: &Selection,
    spec: S,
) -> Result<Vec<Finding>, AnalysisError> {
    let facts = parse_facts(path, source, spec)?;
    let owner = file_span(source);
    let mut findings = template_findings(path, selection, &facts.comments, &owner);
    for region in facts.regions {
        findings.extend(analyze_embedded_region(path, source, selection, &region)?);
    }
    Ok(findings)
}

pub(crate) fn parse_file<S: ContainerSpec>(
    path: &Path,
    source: &str,
    spec: S,
) -> Result<ParsedFile, AnalysisError> {
    let Facts {
        syntax,
        comments,
        regions,
    } = parse_facts(path, source, spec)?;
    let file_span = file_span(source);
    let mut document = ParsedFile {
        owners: vec![
            OwnerSnapshot {
                kind: OwnerKind::File,
                name: "<file>".to_owned(),
                identity: vec!["file".to_owned()],
                span: file_span.clone(),
                parent: None,
                code: Vec::new(),
            },
            OwnerSnapshot {
                kind: OwnerKind::Template,
                name: "<template>".to_owned(),
                identity: vec!["template".to_owned()],
                span: file_span,
                parent: Some(0),
                code: syntax,
            },
        ],
        comments: comments
            .into_iter()
            .map(|comment| CommentSnapshot {
                kind: comment.kind,
                text: comment.text,
                span: comment.span,
                owner: 1,
            })
            .collect(),
    };
    for region in regions {
        let embedded = parse_embedded_file(path, source, &region)?;
        merge_embedded_file(&mut document, embedded, &region, 1)?;
    }
    Ok(document)
}

fn file_span(source: &str) -> Span {
    Span {
        start_byte: 0,
        end_byte: source.len(),
        start_line: 1,
        end_line: source.lines().count().max(1),
    }
}

fn parse_facts<S: ContainerSpec>(
    path: &Path,
    source: &str,
    spec: S,
) -> Result<Facts, AnalysisError> {
    let tree = tree::parse(path, source, spec.label(), spec.grammar())?;
    let mut syntax = Vec::new();
    let mut comments = Vec::new();
    let mut regions = Vec::new();
    let mut excluded_depth = 0;
    for event in events(tree.root_node()) {
        match event {
            WalkEvent::Enter(node) => {
                let node_span = Span::from_node(node);
                let is_comment = spec.is_comment(node);
                if is_comment {
                    comments.push(Comment {
                        span: Span::from_comment_node(node, source),
                        kind: CommentKind::Narrative,
                        text: tree::node_text(node, source).to_owned(),
                    });
                }
                if let Some(region) = spec.embedded_region(node, source) {
                    regions.push(region);
                }
                let starts_exclusion = is_comment
                    || regions
                        .last()
                        .is_some_and(|region| region.span.contains(&node_span));
                if excluded_depth > 0 || starts_exclusion {
                    excluded_depth += 1;
                    continue;
                }
                syntax.push(CodeToken::enter(node.kind()));
                if node.child_count() == 0 {
                    let text = tree::node_text(node, source);
                    if !text.trim().is_empty() {
                        syntax.push(CodeToken::atom(node.kind(), text));
                    }
                }
            }
            WalkEvent::Leave(node) => {
                if excluded_depth > 0 {
                    excluded_depth -= 1;
                } else {
                    syntax.push(CodeToken::leave(node.kind()));
                }
            }
        }
    }
    Ok(Facts {
        syntax,
        comments,
        regions,
    })
}

pub(crate) fn script_region(node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
    if node.kind() != "script_element" {
        return None;
    }
    let start_tag = direct_named_child_with_kind(node, "start_tag")?;
    let raw = tree::first_descendant_with_kind(node, "raw_text")?;
    html_script_language(start_tag, source).map(|language| EmbeddedRegion {
        span: Span::from_node(raw),
        language,
        owner_name: "<script>",
    })
}

pub(crate) fn astro_script_region(node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
    if node.kind() != "script_element" {
        return None;
    }
    let start_tag = direct_named_child_with_kind(node, "start_tag")?;
    let raw = tree::first_descendant_with_kind(node, "raw_text")?;
    let language = if has_only_src_attributes(start_tag, source) {
        Some(EmbeddedLanguage::TypeScript)
    } else {
        browser_script_language(start_tag, source)
    }?;
    Some(EmbeddedRegion {
        span: Span::from_node(raw),
        language,
        owner_name: "<script>",
    })
}

pub(crate) fn style_region(node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
    if node.kind() != "style_element" {
        return None;
    }
    let start_tag = direct_named_child_with_kind(node, "start_tag")?;
    let raw = tree::first_descendant_with_kind(node, "raw_text")?;
    style_is_css(start_tag, source).then(|| EmbeddedRegion {
        span: Span::from_node(raw),
        language: EmbeddedLanguage::Css,
        owner_name: "<style>",
    })
}

pub(crate) fn astro_style_region(node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
    if node.kind() != "style_element" {
        return None;
    }
    let start_tag = direct_named_child_with_kind(node, "start_tag")?;
    let raw = tree::first_descendant_with_kind(node, "raw_text")?;
    astro_style_is_css(start_tag, source).then(|| EmbeddedRegion {
        span: Span::from_node(raw),
        language: EmbeddedLanguage::Css,
        owner_name: "<style>",
    })
}

fn html_script_language(start_tag: Node<'_>, source: &str) -> Option<EmbeddedLanguage> {
    let type_language = browser_script_language(start_tag, source)?;
    match attribute_value(start_tag, source, "lang") {
        AttributeValue::Missing | AttributeValue::Valueless => Some(type_language),
        AttributeValue::Literal(value) if is_typescript_language(&value) => {
            Some(EmbeddedLanguage::TypeScript)
        }
        AttributeValue::Literal(_) => Some(type_language),
        AttributeValue::Dynamic => None,
    }
}

fn browser_script_language(start_tag: Node<'_>, source: &str) -> Option<EmbeddedLanguage> {
    match attribute_value(start_tag, source, "type") {
        AttributeValue::Missing | AttributeValue::Valueless => Some(EmbeddedLanguage::JavaScript),
        AttributeValue::Literal(value) => classify_script_type(&value),
        AttributeValue::Dynamic => Some(EmbeddedLanguage::JavaScript),
    }
}

fn classify_script_type(value: &str) -> Option<EmbeddedLanguage> {
    let essence = value.trim();
    if [
        "application/typescript",
        "application/x-typescript",
        "text/typescript",
    ]
    .iter()
    .any(|candidate| essence.eq_ignore_ascii_case(candidate))
    {
        return Some(EmbeddedLanguage::TypeScript);
    }
    let javascript = [
        "",
        "module",
        "application/ecmascript",
        "application/javascript",
        "application/x-ecmascript",
        "application/x-javascript",
        "text/ecmascript",
        "text/javascript",
        "text/javascript1.0",
        "text/javascript1.1",
        "text/javascript1.2",
        "text/javascript1.3",
        "text/javascript1.4",
        "text/javascript1.5",
        "text/jscript",
        "text/livescript",
        "text/x-ecmascript",
        "text/x-javascript",
    ];
    javascript
        .iter()
        .any(|candidate| essence.eq_ignore_ascii_case(candidate))
        .then_some(EmbeddedLanguage::JavaScript)
}

fn is_typescript_language(value: &str) -> bool {
    value.eq_ignore_ascii_case("ts") || value.eq_ignore_ascii_case("typescript")
}

fn style_is_css(start_tag: Node<'_>, source: &str) -> bool {
    match attribute_value(start_tag, source, "type") {
        AttributeValue::Missing | AttributeValue::Valueless => true,
        AttributeValue::Literal(value) => {
            value.is_empty() || value.eq_ignore_ascii_case("text/css")
        }
        AttributeValue::Dynamic => true,
    }
}

fn astro_style_is_css(start_tag: Node<'_>, source: &str) -> bool {
    if !style_is_css(start_tag, source) {
        return false;
    }
    match attribute_value(start_tag, source, "lang") {
        AttributeValue::Missing | AttributeValue::Valueless => true,
        AttributeValue::Literal(value) => value.is_empty() || value.eq_ignore_ascii_case("css"),
        AttributeValue::Dynamic => true,
    }
}

enum AttributeValue<'source> {
    Missing,
    Valueless,
    Literal(Cow<'source, str>),
    Dynamic,
}

fn attribute_value<'source>(
    start_tag: Node<'_>,
    source: &'source str,
    wanted_name: &str,
) -> AttributeValue<'source> {
    let mut cursor = start_tag.walk();
    let attribute = start_tag
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "attribute")
        .find_map(|attribute| {
            let name = direct_named_child_with_kind(attribute, "attribute_name")?;
            tree::node_text(name, source)
                .eq_ignore_ascii_case(wanted_name)
                .then_some(attribute)
        });
    let Some(attribute) = attribute else {
        return AttributeValue::Missing;
    };
    let mut cursor = attribute.walk();
    for child in attribute.named_children(&mut cursor) {
        match child.kind() {
            "attribute_value" | "quoted_attribute_value" => {
                let value = trim_attribute_value(tree::node_text(child, source));
                return AttributeValue::Literal(html_escape::decode_html_entities(value));
            }
            "attribute_backtick_string" | "attribute_interpolation" => {
                return AttributeValue::Dynamic;
            }
            _ => {}
        }
    }
    AttributeValue::Valueless
}

fn has_only_src_attributes(start_tag: Node<'_>, source: &str) -> bool {
    let mut cursor = start_tag.walk();
    start_tag
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "attribute")
        .all(|attribute| {
            direct_named_child_with_kind(attribute, "attribute_name")
                .is_some_and(|name| tree::node_text(name, source).eq_ignore_ascii_case("src"))
        })
}

fn direct_named_child_with_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn trim_attribute_value(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn parse_embedded_file(
    path: &Path,
    source: &str,
    region: &EmbeddedRegion,
) -> Result<ParsedFile, AnalysisError> {
    let embedded = embedded_source(path, source, region)?;
    match region.language {
        EmbeddedLanguage::Css => css::parse_file(path, embedded),
        EmbeddedLanguage::JavaScript => javascript::parse_file(path, embedded),
        EmbeddedLanguage::TypeScript => typescript::parse_file(path, embedded),
    }
}

fn merge_embedded_file(
    destination: &mut ParsedFile,
    embedded: ParsedFile,
    region: &EmbeddedRegion,
    parent: usize,
) -> Result<(), AnalysisError> {
    let owner_offset = destination.owners.len();
    for (index, mut owner) in embedded.owners.into_iter().enumerate() {
        shift_span(&mut owner.span, region)?;
        if index == 0 {
            owner.name = region.owner_name.to_owned();
            owner.parent = Some(parent);
        } else {
            let embedded_parent = owner.parent.ok_or_else(|| {
                AnalysisError::Invariant(format!(
                    "{} embedded owner `{}` has no parent",
                    embedded_label(region.language),
                    owner.name
                ))
            })?;
            owner.parent = Some(checked_owner_offset(owner_offset, embedded_parent)?);
        }
        destination.owners.push(owner);
    }
    for mut comment in embedded.comments {
        shift_span(&mut comment.span, region)?;
        comment.owner = checked_owner_offset(owner_offset, comment.owner)?;
        destination.comments.push(comment);
    }
    Ok(())
}

fn checked_owner_offset(offset: usize, owner: usize) -> Result<usize, AnalysisError> {
    offset.checked_add(owner).ok_or_else(|| {
        AnalysisError::Invariant("embedded owner index exceeded the platform limit".to_owned())
    })
}

fn shift_span(span: &mut Span, region: &EmbeddedRegion) -> Result<(), AnalysisError> {
    span.start_byte = span
        .start_byte
        .checked_add(region.span.start_byte)
        .ok_or_else(|| {
            AnalysisError::Invariant("embedded byte span exceeded the platform limit".to_owned())
        })?;
    span.end_byte = span
        .end_byte
        .checked_add(region.span.start_byte)
        .ok_or_else(|| {
            AnalysisError::Invariant("embedded byte span exceeded the platform limit".to_owned())
        })?;
    let line_offset = region.span.start_line.saturating_sub(1);
    span.start_line = span.start_line.checked_add(line_offset).ok_or_else(|| {
        AnalysisError::Invariant("embedded line span exceeded the platform limit".to_owned())
    })?;
    span.end_line = span.end_line.checked_add(line_offset).ok_or_else(|| {
        AnalysisError::Invariant("embedded line span exceeded the platform limit".to_owned())
    })?;
    Ok(())
}

fn analyze_embedded_region(
    path: &Path,
    source: &str,
    selection: &Selection,
    region: &EmbeddedRegion,
) -> Result<Vec<Finding>, AnalysisError> {
    let embedded = embedded_source(path, source, region)?;
    let line_offset = region.span.start_line - 1;
    let embedded_selection = selection.project_into(region.span.start_byte, region.span.end_byte);
    let mut findings = match region.language {
        EmbeddedLanguage::Css => css::analyze_file(path, embedded, &embedded_selection)?,
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

fn embedded_source<'source>(
    path: &Path,
    source: &'source str,
    region: &EmbeddedRegion,
) -> Result<&'source str, AnalysisError> {
    source
        .get(region.span.start_byte..region.span.end_byte)
        .ok_or_else(|| AnalysisError::Parse {
            path: path.display().to_string(),
            language: embedded_label(region.language),
        })
}

fn embedded_label(language: EmbeddedLanguage) -> &'static str {
    match language {
        EmbeddedLanguage::Css => "CSS",
        EmbeddedLanguage::JavaScript => "JavaScript",
        EmbeddedLanguage::TypeScript => "TypeScript",
    }
}
