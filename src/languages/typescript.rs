use std::path::Path;

use tree_sitter::Node;

use super::javascript::{classify_comment, function_namespace, is_function_kind, leaf_from_node};
use super::tree::{LanguageSpec, analyze, document};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, Leaf, ParsedFile};

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
    type Context = ();

    fn label(self) -> &'static str {
        "TypeScript"
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Standard => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn function_span(self, node: Node<'_>, _source: &str) -> Option<crate::policy::Span> {
        is_function_kind(node.kind()).then(|| crate::policy::Span::from_node(node))
    }

    fn function_namespace(self, node: Node<'_>, source: &str) -> Vec<String> {
        function_namespace(node, source)
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        source: &str,
        _context: &Self::Context,
    ) -> Option<CommentKind> {
        classify_comment(node, source)
    }

    fn leaf(self, node: Node<'_>, source: &str, _function_depth: usize) -> Option<Leaf> {
        leaf_from_node(node, source)
    }
}
