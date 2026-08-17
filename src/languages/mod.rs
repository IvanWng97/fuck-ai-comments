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
mod walk;

use std::path::Path;

use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::ParsedFile;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Adapter {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Toml,
    Html,
    Css,
    Astro,
}

impl Adapter {
    fn for_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("rs") => Some(Self::Rust),
            Some("py" | "pyi" | "pyw") => Some(Self::Python),
            Some("js" | "cjs" | "mjs" | "jsx") => Some(Self::JavaScript),
            Some("ts" | "cts" | "mts") => Some(Self::TypeScript),
            Some("tsx") => Some(Self::Tsx),
            Some("toml") => Some(Self::Toml),
            Some("html" | "htm") => Some(Self::Html),
            Some("css") => Some(Self::Css),
            Some("astro") => Some(Self::Astro),
            _ => None,
        }
    }
}

pub(crate) fn supports_path(path: &Path) -> bool {
    Adapter::for_path(path).is_some()
}

pub(crate) fn same_adapter(before: &Path, after: &Path) -> Result<bool, AnalysisError> {
    let after_adapter = Adapter::for_path(after)
        .ok_or_else(|| AnalysisError::Unsupported(after.display().to_string()))?;
    Ok(Adapter::for_path(before) == Some(after_adapter))
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    match Adapter::for_path(path) {
        Some(Adapter::Rust) => rust::parse_file(path, source),
        Some(Adapter::Python) => python::parse_file(path, source),
        Some(Adapter::JavaScript) => javascript::parse_file(path, source),
        Some(Adapter::TypeScript) => typescript::parse_file(path, source),
        Some(Adapter::Tsx) => typescript::parse_tsx_file(path, source),
        Some(Adapter::Toml) => toml::parse_file(path, source),
        Some(Adapter::Html) => html::parse_file(path, source),
        Some(Adapter::Css) => css::parse_file(path, source),
        Some(Adapter::Astro) => astro::parse_file(path, source),
        None => Err(AnalysisError::Unsupported(path.display().to_string())),
    }
}

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    match Adapter::for_path(path) {
        Some(Adapter::Rust) => rust::analyze_file(path, source, selection),
        Some(Adapter::Python) => python::analyze_file(path, source, selection),
        Some(Adapter::JavaScript) => javascript::analyze_file(path, source, selection),
        Some(Adapter::TypeScript) => typescript::analyze_file(path, source, selection),
        Some(Adapter::Tsx) => typescript::analyze_tsx_file(path, source, selection),
        Some(Adapter::Toml) => toml::analyze_file(path, source, selection),
        Some(Adapter::Html) => html::analyze_file(path, source, selection),
        Some(Adapter::Css) => css::analyze_file(path, source, selection),
        Some(Adapter::Astro) => astro::analyze_file(path, source, selection),
        None => Err(AnalysisError::Unsupported(path.display().to_string())),
    }
}
