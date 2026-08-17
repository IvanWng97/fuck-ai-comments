use std::path::Path;

use tree_sitter::Node;

use super::javascript::{classify_comment, is_function_kind, leaf_from_node};
use super::tree::{LanguageSpec, analyze};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, Leaf};

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

impl LanguageSpec for TypeScript {
    fn label(self) -> &'static str {
        "TypeScript"
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Standard => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn is_function(self, kind: &str) -> bool {
        is_function_kind(kind)
    }

    fn classify_comment(self, node: Node<'_>, source: &str) -> Option<CommentKind> {
        classify_comment(node, source)
    }

    fn leaf(self, node: Node<'_>, source: &str, _function_depth: usize) -> Option<Leaf> {
        leaf_from_node(node, source)
    }
}
