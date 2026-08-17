use std::path::Path;

use tree_sitter::Node;

use super::container::{ContainerSpec, EmbeddedRegion, analyze, script_region};
use crate::model::{AnalysisError, Finding, Selection};

#[derive(Clone, Copy)]
struct Html;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Html)
}

impl ContainerSpec for Html {
    fn label(self) -> &'static str {
        "HTML"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_html::LANGUAGE.into()
    }

    fn embedded_region(self, node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
        script_region(node, source)
    }
}
