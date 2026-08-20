use std::path::Path;

use tree_sitter::Node;

use super::javascript::{
    attachment_syntax, classify_comment as classify_javascript_comment, is_function_kind,
    is_owner_prefix, owner_from_node, prefixed_span,
};
use super::tree::{
    AttachmentIndex, CallableSubtrees, DirectivePlacement, LanguageSpec, OwnerCandidate,
    OwnerLocation, analyze_with_policy, canonical_syntax, direct_named_child, document, node_text,
};
use crate::config::PolicyConfig;
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
    policy: &PolicyConfig,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze_with_policy(path, source, selection, TypeScript::Standard, policy)
}

pub(crate) fn analyze_tsx_file(
    path: &Path,
    source: &str,
    selection: &Selection,
    policy: &PolicyConfig,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze_with_policy(path, source, selection, TypeScript::Tsx, policy)
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

    fn build_context(self, root: Node<'_>, source: &str) -> Result<Self::Context, AnalysisError> {
        Ok(AttachmentIndex::with_syntax(
            root,
            source,
            attachment_syntax(),
        ))
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
        _context: &Self::Context,
        _function_depth: usize,
        callable_subtrees: &CallableSubtrees,
    ) -> Result<Option<OwnerCandidate>, AnalysisError> {
        Ok(typescript_owner(node, location, source)
            .or_else(|| owner_from_node(node, location, source, callable_subtrees)))
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

const PATH_ATTRIBUTE: &str = "path";
const TYPES_ATTRIBUTE: &str = "types";
const LIB_ATTRIBUTE: &str = "lib";
const NO_DEFAULT_LIB_ATTRIBUTE: &str = "no-default-lib";
const PRESERVE_ATTRIBUTE: &str = "preserve";
const RESOLUTION_MODE_ATTRIBUTE: &str = "resolution-mode";
const NAME_ATTRIBUTE: &str = "name";
const TRUE_ATTRIBUTE_VALUE: &str = "true";
const REFERENCE_ATTRIBUTES: &[&str] = &[
    PATH_ATTRIBUTE,
    TYPES_ATTRIBUTE,
    LIB_ATTRIBUTE,
    NO_DEFAULT_LIB_ATTRIBUTE,
    PRESERVE_ATTRIBUTE,
    RESOLUTION_MODE_ATTRIBUTE,
];
const AMD_MODULE_ATTRIBUTES: &[&str] = &[NAME_ATTRIBUTE];
const AMD_DEPENDENCY_ATTRIBUTES: &[&str] = &[PATH_ATTRIBUTE, NAME_ATTRIBUTE];

#[derive(Clone, Copy)]
enum TripleSlashTag {
    Reference,
    AmdModule,
    AmdDependency,
}

impl TripleSlashTag {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "reference" => Some(Self::Reference),
            "amd-module" => Some(Self::AmdModule),
            "amd-dependency" => Some(Self::AmdDependency),
            _ => None,
        }
    }

    fn allowed_attributes(self) -> &'static [&'static str] {
        match self {
            Self::Reference => REFERENCE_ATTRIBUTES,
            Self::AmdModule => AMD_MODULE_ATTRIBUTES,
            Self::AmdDependency => AMD_DEPENDENCY_ATTRIBUTES,
        }
    }

    fn valid_attributes(self, attributes: &[(&str, &str)]) -> bool {
        match self {
            Self::Reference => valid_reference_attributes(attributes),
            Self::AmdModule => attributes.len() == 1 && attributes[0].0 == NAME_ATTRIBUTE,
            Self::AmdDependency => attributes.iter().any(|(name, _)| *name == PATH_ATTRIBUTE),
        }
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
    let Some(tag_kind) = TripleSlashTag::from_name(name) else {
        return false;
    };
    let Some(attributes) = parse_attributes(&tag[name_end..], tag_kind.allowed_attributes()) else {
        return false;
    };
    tag_kind.valid_attributes(&attributes)
}

#[cfg(test)]
thread_local! {
    static ATTRIBUTE_NAME_INSPECTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_attribute_name_inspection() {
    ATTRIBUTE_NAME_INSPECTIONS.with(|inspections| inspections.set(inspections.get() + 1));
}

#[cfg(not(test))]
fn record_attribute_name_inspection() {}

fn parse_attributes<'source>(
    mut input: &'source str,
    allowed_names: &[&str],
) -> Option<Vec<(&'source str, &'source str)>> {
    let mut attributes = Vec::new();
    let mut seen = vec![false; allowed_names.len()];
    loop {
        input = input.trim_start();
        if input.is_empty() {
            return Some(attributes);
        }
        let name_end = input
            .find(|character: char| character.is_whitespace() || character == '=')
            .unwrap_or(input.len());
        let name = &input[..name_end];
        let name_index = allowed_names.iter().position(|allowed| {
            record_attribute_name_inspection();
            *allowed == name
        })?;
        if std::mem::replace(&mut seen[name_index], true) {
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
    if attributes == [(NO_DEFAULT_LIB_ATTRIBUTE, TRUE_ATTRIBUTE_VALUE)] {
        return true;
    }
    let mut primaries = attributes
        .iter()
        .filter(|(name, _)| matches!(*name, PATH_ATTRIBUTE | TYPES_ATTRIBUTE | LIB_ATTRIBUTE));
    let Some((primary, _)) = primaries.next() else {
        return false;
    };
    primaries.next().is_none()
        && attributes.iter().all(|(name, value)| match *name {
            PATH_ATTRIBUTE | TYPES_ATTRIBUTE | LIB_ATTRIBUTE => !value.is_empty(),
            PRESERVE_ATTRIBUTE => *value == TRUE_ATTRIBUTE_VALUE,
            RESOLUTION_MODE_ATTRIBUTE => {
                *primary == TYPES_ATTRIBUTE && matches!(*value, "import" | "require")
            }
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const ATTRIBUTE_NAME_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

    fn unknown_attribute_name(index: usize) -> String {
        let radix = ATTRIBUTE_NAME_ALPHABET.len();
        let first = char::from(ATTRIBUTE_NAME_ALPHABET[(index / (radix * radix)) % radix]);
        let second = char::from(ATTRIBUTE_NAME_ALPHABET[(index / radix) % radix]);
        let third = char::from(ATTRIBUTE_NAME_ALPHABET[index % radix]);
        format!("x{first}{second}{third}")
    }

    fn malformed_reference_attribute_inspections(attribute_count: usize) -> usize {
        let mut directive = String::from("/// <reference");
        for index in 0..attribute_count {
            write!(directive, " {}=\"value\"", unknown_attribute_name(index)).unwrap();
        }
        directive.push_str(" />");

        ATTRIBUTE_NAME_INSPECTIONS.with(|inspections| inspections.set(0));
        assert!(!valid_triple_slash_directive(&directive));
        ATTRIBUTE_NAME_INSPECTIONS.with(std::cell::Cell::get)
    }

    #[test]
    fn unknown_triple_slash_attributes_have_bounded_name_inspections() {
        const ATTRIBUTE_COUNTS: [usize; 4] = [1_000, 2_000, 4_000, 8_000];

        let inspections = ATTRIBUTE_COUNTS.map(malformed_reference_attribute_inspections);

        assert_eq!(
            inspections,
            ATTRIBUTE_COUNTS.map(|_| REFERENCE_ATTRIBUTES.len()),
            "each unknown name should inspect only the finite reference attribute set"
        );
    }
}
