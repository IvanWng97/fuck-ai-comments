use std::path::Path;

use tree_sitter::Node;

use super::container::{
    ContainerSpec, EmbeddedRegion, analyze, parse_file as parse, script_region, style_region,
};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::ParsedFile;

#[derive(Clone, Copy)]
struct Html;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Html)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    parse(path, source, Html)
}

impl ContainerSpec for Html {
    fn label(self) -> &'static str {
        "HTML"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_html::LANGUAGE.into()
    }

    fn embedded_region(self, node: Node<'_>, source: &str) -> Option<EmbeddedRegion> {
        script_region(node, source).or_else(|| style_region(node, source))
    }
}
