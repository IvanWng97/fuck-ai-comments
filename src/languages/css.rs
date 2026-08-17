use std::path::Path;

use super::container::{ContainerSpec, analyze};
use crate::model::{AnalysisError, Finding, Selection};

#[derive(Clone, Copy)]
struct Css;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Css)
}

impl ContainerSpec for Css {
    fn label(self) -> &'static str {
        "CSS"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_css::LANGUAGE.into()
    }
}
