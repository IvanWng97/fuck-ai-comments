//! Owner-aware comment policy shared by the CLI and editor integrations.

use std::path::Path;

mod languages;
mod model;
mod policy;

pub use model::{AnalysisError, Finding, Selection};

/// Analyze one supported source file.
///
/// # Errors
///
/// Returns an error when the file extension is unsupported or parsing fails.
pub fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    languages::analyze_file(path, source, selection)
}
