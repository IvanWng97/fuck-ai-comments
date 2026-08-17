mod astro;
mod container;
mod css;
mod html;
mod javascript;
mod python;
mod rust;
mod toml;
mod tree;
mod typescript;

use std::path::Path;

use crate::model::{AnalysisError, Finding, Selection};

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("rs") => rust::analyze_file(path, source, selection),
        Some("py" | "pyi") => python::analyze_file(path, source, selection),
        Some("js" | "mjs" | "jsx") => javascript::analyze_file(path, source, selection),
        Some("ts") => typescript::analyze_file(path, source, selection),
        Some("tsx") => typescript::analyze_tsx_file(path, source, selection),
        Some("toml") => toml::analyze_file(path, source, selection),
        Some("html" | "htm") => html::analyze_file(path, source, selection),
        Some("css") => css::analyze_file(path, source, selection),
        Some("astro") => astro::analyze_file(path, source, selection),
        _ => Err(AnalysisError::Unsupported(path.display().to_string())),
    }
}
