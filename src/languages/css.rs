use std::path::Path;

use super::container::{ContainerSpec, analyze, parse_file as parse};
use crate::config::PolicyConfig;
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::ParsedFile;

#[derive(Clone, Copy)]
struct Css;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
    policy: &PolicyConfig,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Css, policy)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    parse(path, source, Css)
}

impl ContainerSpec for Css {
    fn label(self) -> &'static str {
        "CSS"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_css::LANGUAGE.into()
    }
}
