mod astro;
mod container;
mod css;
mod html;
mod javascript;
mod kotlin;
mod objective_c;
mod python;
mod rust;
mod swift;
mod toml;
mod tree;
mod typescript;
mod walk;

use std::path::Path;

use crate::model::{AnalysisError, Finding, OwnerKind, Selection};
use crate::policy::ParsedFile;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Adapter {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Kotlin,
    ObjectiveC,
    Swift,
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
            Some("kt" | "kts") => Some(Self::Kotlin),
            Some("m") => Some(Self::ObjectiveC),
            Some("swift") => Some(Self::Swift),
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
        Some(Adapter::Kotlin) => kotlin::parse_file(path, source),
        Some(Adapter::ObjectiveC) => objective_c::parse_file(path, source),
        Some(Adapter::Swift) => swift::parse_file(path, source),
        Some(Adapter::Toml) => toml::parse_file(path, source),
        Some(Adapter::Html) => html::parse_file(path, source),
        Some(Adapter::Css) => css::parse_file(path, source),
        Some(Adapter::Astro) => astro::parse_file(path, source),
        None => Err(AnalysisError::Unsupported(path.display().to_string())),
    }
}

pub(crate) fn parse_validated_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    let document = parse_file(path, source)?;
    let display_path = path.to_string_lossy();
    let Some(file) = document.owners.first() else {
        return Err(AnalysisError::Invariant(format!(
            "{display_path} has no implicit file owner"
        )));
    };
    if file.kind != OwnerKind::File || file.parent.is_some() {
        return Err(AnalysisError::Invariant(format!(
            "{display_path} has an invalid implicit file owner"
        )));
    }
    if document.owners.iter().skip(1).any(|owner| {
        owner
            .parent
            .is_none_or(|parent| parent >= document.owners.len())
    }) {
        return Err(AnalysisError::Invariant(format!(
            "{display_path} contains an owner with no valid parent"
        )));
    }
    if document
        .comments
        .iter()
        .any(|comment| comment.owner >= document.owners.len())
    {
        return Err(AnalysisError::Invariant(format!(
            "{display_path} contains a comment with no valid owner"
        )));
    }
    Ok(document)
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
        Some(Adapter::Kotlin) => kotlin::analyze_file(path, source, selection),
        Some(Adapter::ObjectiveC) => objective_c::analyze_file(path, source, selection),
        Some(Adapter::Swift) => swift::analyze_file(path, source, selection),
        Some(Adapter::Toml) => toml::analyze_file(path, source, selection),
        Some(Adapter::Html) => html::analyze_file(path, source, selection),
        Some(Adapter::Css) => css::analyze_file(path, source, selection),
        Some(Adapter::Astro) => astro::analyze_file(path, source, selection),
        None => Err(AnalysisError::Unsupported(path.display().to_string())),
    }
}
