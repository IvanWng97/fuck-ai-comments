use std::path::Path;

use tree_sitter::Node;

use super::javascript::{
    attachment_syntax, classify_comment as classify_javascript_comment, is_function_kind,
    is_owner_prefix, owner_from_node, prefixed_span,
};
use super::tree::{
    AttachmentIndex, CallableSubtrees, DirectivePlacement, LanguageSpec, OwnerCandidate,
    OwnerLocation, analyze, canonical_syntax, direct_named_child, document, node_text,
};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile};

#[derive(Clone, Copy)]
enum TypeScript {
    Standard,
    Tsx,
}

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, TypeScript::Standard)
}

pub(crate) fn analyze_tsx_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, TypeScript::Tsx)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, TypeScript::Standard)
}

pub(crate) fn parse_tsx_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, TypeScript::Tsx)
}

impl LanguageSpec for TypeScript {
    type Context = AttachmentIndex;

    fn label(self) -> &'static str {
        "TypeScript"
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Standard => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn build_context(self, root: Node<'_>, source: &str) -> Self::Context {
        AttachmentIndex::with_syntax(root, source, attachment_syntax())
    }

    fn is_owner_prefix(self, kind: &str) -> bool {
        is_owner_prefix(kind)
    }

    fn callable_kind(self) -> Option<fn(&str) -> bool> {
        Some(is_function_kind)
    }

    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        _function_depth: usize,
        callable_subtrees: &CallableSubtrees,
    ) -> Option<OwnerCandidate> {
        typescript_owner(node, location, source)
            .or_else(|| owner_from_node(node, location, source, callable_subtrees))
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        context: &Self::Context,
    ) -> Option<CommentKind> {
        if node.kind() == "comment"
            && context.is_attached(node, DirectivePlacement::FilePreamble)
            && valid_triple_slash_directive(node_text(node, source))
        {
            Some(CommentKind::ToolDirective)
        } else {
            classify_javascript_comment(node, source, context)
        }
    }
}

fn typescript_owner(
    node: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
) -> Option<OwnerCandidate> {
    if let Some(owner) = ambient_owner(node, location, source) {
        return Some(owner);
    }
    if let Some(prefix) = typescript_type_prefix(node.kind()) {
        let name = node
            .child_by_field_name("name")
            .map(|name| node_text(name, source).trim().to_owned())?;
        return Some(OwnerCandidate::type_owner(
            prefixed_span(node, location),
            name.clone(),
            vec![format!("{prefix}:{name}")],
        ));
    }
    if !matches!(
        node.kind(),
        "function_signature" | "method_signature" | "abstract_method_signature"
    ) {
        return None;
    }
    let name = node
        .child_by_field_name("name")
        .map(|name| node_text(name, source).trim().to_owned())?;
    let outer = location
        .parent()
        .filter(|parent| parent.kind() == "ambient_declaration")
        .unwrap_or(node);
    Some(OwnerCandidate::function(
        if outer.id() == node.id() {
            prefixed_span(node, location)
        } else {
            crate::policy::Span::from_node(outer)
        },
        name,
        vec![format!("signature:{}", canonical_syntax(node, source))],
    ))
}

fn ambient_owner(
    node: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
) -> Option<OwnerCandidate> {
    if node.kind() != "ambient_declaration" {
        return None;
    }
    let declaration = ambient_declaration_child(node)?;
    let owner = match declaration.kind() {
        "lexical_declaration" => ambient_const_owner(node, declaration, location, source),
        "function_signature" => ambient_function_owner(node, declaration, location, source),
        _ => ambient_type_owner(node, declaration, location, source),
    }?;
    Some(owner.suppressing(vec![declaration.id()]))
}

fn ambient_declaration_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|child| {
        matches!(child.kind(), "lexical_declaration" | "function_signature")
            || typescript_type_prefix(child.kind()).is_some()
    })
}

fn ambient_function_owner(
    ambient: Node<'_>,
    declaration: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
) -> Option<OwnerCandidate> {
    let name = declaration
        .child_by_field_name("name")
        .map(|name| node_text(name, source).trim().to_owned())?;
    Some(OwnerCandidate::function(
        prefixed_span(ambient, location),
        name,
        Vec::new(),
    ))
}

fn ambient_type_owner(
    ambient: Node<'_>,
    declaration: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
) -> Option<OwnerCandidate> {
    let prefix = typescript_type_prefix(declaration.kind())?;
    let name = declaration
        .child_by_field_name("name")
        .map(|name| node_text(name, source).trim().to_owned())?;
    Some(OwnerCandidate::type_owner(
        prefixed_span(ambient, location),
        name.clone(),
        vec![format!("{prefix}:{name}")],
    ))
}

fn ambient_const_owner(
    ambient: Node<'_>,
    declaration: Node<'_>,
    location: OwnerLocation<'_>,
    source: &str,
) -> Option<OwnerCandidate> {
    if declaration
        .child_by_field_name("kind")
        .is_none_or(|kind| kind.kind() != "const")
    {
        return None;
    }
    let declarator = direct_named_child(declaration, "variable_declarator")?;
    let name = declarator
        .child_by_field_name("name")
        .map_or("<destructured>", |name| node_text(name, source))
        .to_owned();
    Some(OwnerCandidate::leaf(prefixed_span(ambient, location), name))
}

fn typescript_type_prefix(kind: &str) -> Option<&'static str> {
    match kind {
        "interface_declaration" => Some("interface"),
        "type_alias_declaration" => Some("type"),
        "enum_declaration" => Some("enum"),
        "class_declaration" | "abstract_class_declaration" => Some("class"),
        "internal_module" | "module" => Some("namespace"),
        _ => None,
    }
}

fn valid_triple_slash_directive(comment: &str) -> bool {
    let Some(body) = comment.trim().strip_prefix("///") else {
        return false;
    };
    if body.starts_with('/') {
        return false;
    }
    let body = body.trim();
    let Some(tag) = body
        .strip_prefix('<')
        .and_then(|body| body.strip_suffix("/>"))
    else {
        return false;
    };
    let tag = tag.trim();
    let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
    let name = &tag[..name_end];
    let Some(attributes) = parse_attributes(&tag[name_end..]) else {
        return false;
    };
    match name {
        "reference" => valid_reference_attributes(&attributes),
        "amd-module" => attributes.len() == 1 && attributes[0].0 == "name",
        "amd-dependency" => {
            attributes.iter().any(|(name, _)| *name == "path")
                && attributes
                    .iter()
                    .all(|(name, _)| matches!(*name, "path" | "name"))
        }
        _ => false,
    }
}

fn parse_attributes(mut input: &str) -> Option<Vec<(&str, &str)>> {
    let mut attributes = Vec::new();
    loop {
        input = input.trim_start();
        if input.is_empty() {
            return Some(attributes);
        }
        let name_end = input
            .find(|character: char| character.is_whitespace() || character == '=')
            .unwrap_or(input.len());
        let name = &input[..name_end];
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
            || attributes.iter().any(|(existing, _)| *existing == name)
        {
            return None;
        }
        input = input[name_end..].trim_start();
        input = input.strip_prefix('=')?.trim_start();
        let quote = *input.as_bytes().first()?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        input = &input[1..];
        let value_end = input.find(char::from(quote))?;
        let value = &input[..value_end];
        if value.is_empty() || value.contains(['<', '>']) {
            return None;
        }
        input = &input[value_end + 1..];
        if !input.is_empty() && !input.starts_with(char::is_whitespace) {
            return None;
        }
        attributes.push((name, value));
    }
}

fn valid_reference_attributes(attributes: &[(&str, &str)]) -> bool {
    if attributes == [("no-default-lib", "true")] {
        return true;
    }
    let mut primaries = attributes
        .iter()
        .filter(|(name, _)| matches!(*name, "path" | "types" | "lib"));
    let Some((primary, _)) = primaries.next() else {
        return false;
    };
    primaries.next().is_none()
        && attributes.iter().all(|(name, value)| match *name {
            "path" | "types" | "lib" => !value.is_empty(),
            "preserve" => *value == "true",
            "resolution-mode" => *primary == "types" && matches!(*value, "import" | "require"),
            _ => false,
        })
}
