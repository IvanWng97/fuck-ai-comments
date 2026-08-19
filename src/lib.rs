//! Owner-aware comment policy shared by the CLI and editor integrations.

#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

use std::path::Path;

mod change;
mod identity;
mod languages;
mod model;
mod policy;

pub use model::{AnalysisError, Finding, SourceFile};

/// Return whether a path has a registered language adapter.
#[must_use]
pub fn supports_path(path: &Path) -> bool {
    languages::supports_path(path)
}

/// Analyze every owner in one source snapshot.
///
/// # Errors
///
/// Returns an error when the file extension is unsupported or parsing fails.
pub fn analyze_all(file: SourceFile<'_>) -> Result<Vec<Finding>, AnalysisError> {
    languages::analyze_file(file.path, file.text, &model::Selection::all())
}

/// Analyze only owners changed between two source snapshots.
///
/// # Errors
///
/// Returns an error when either snapshot cannot be parsed or ownership cannot
/// be paired without guessing.
pub fn analyze_change(
    before: SourceFile<'_>,
    after: SourceFile<'_>,
) -> Result<Vec<Finding>, AnalysisError> {
    change::analyze(before, after)
}
