use std::path::Path;

use tree_sitter::Node;

use super::container::{ContainerSpec, EmbeddedLanguage, EmbeddedRegion, analyze, script_region};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::Span;

#[derive(Clone, Copy)]
struct Astro;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Astro)
}

impl ContainerSpec for Astro {
    fn label(self) -> &'static str {
        "Astro"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_astro_next::LANGUAGE.into()
    }

    fn embedded_region(self, node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
        if node.kind() == "frontmatter_js_block" {
            return Some(EmbeddedRegion {
                span: Span::from_node(node),
                language: EmbeddedLanguage::TypeScript,
            });
        }
        script_region(node, source)
    }
}
