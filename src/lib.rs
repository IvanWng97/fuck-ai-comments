//! Owner-aware comment policy shared by the CLI and editor integrations.

#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

use std::path::Path;

mod change;
mod identity;
mod languages;
mod model;
mod policy;

pub use model::{AnalysisError, AnalysisProfile, Finding, SourceFile};

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
    analyze_all_with_profile(file, AnalysisProfile::Full)
}

/// Analyze one source snapshot under the selected profile.
///
/// [`AnalysisProfile::Attestation`] validates the language and normalized
/// ownership model, then returns no findings because a snapshot has no change
/// to attest.
///
/// # Errors
///
/// Returns an error when the file extension is unsupported, parsing fails, or
/// the normalized ownership model is invalid.
pub fn analyze_all_with_profile(
    file: SourceFile<'_>,
    profile: AnalysisProfile,
) -> Result<Vec<Finding>, AnalysisError> {
    if profile.runs_static_policy() {
        languages::analyze_file(file.path, file.text, &model::Selection::all())
    } else {
        languages::parse_validated_file(file.path, file.text)?;
        Ok(Vec::new())
    }
}

/// Analyze only owners changed between two source snapshots.
///
/// # Errors
///
/// Returns an error when either snapshot cannot be parsed, exceeds the diff
/// engine's token capacity, or ownership cannot be paired without guessing.
pub fn analyze_change(
    before: SourceFile<'_>,
    after: SourceFile<'_>,
) -> Result<Vec<Finding>, AnalysisError> {
    change::analyze(before, after)
}

/// Analyze a source change under the selected profile.
///
/// [`AnalysisProfile::Attestation`] validates and pairs both normalized source
/// documents but emits only unchanged meaningful comments whose owner changed
/// or whose ownership moved.
///
/// # Errors
///
/// Returns an error when either snapshot cannot be parsed or exceeds the diff
/// engine's token capacity, when the snapshots use different language
/// adapters, or when ownership cannot be paired without guessing.
pub fn analyze_change_with_profile(
    before: SourceFile<'_>,
    after: SourceFile<'_>,
    profile: AnalysisProfile,
) -> Result<Vec<Finding>, AnalysisError> {
    change::analyze_with_profile(before, after, profile)
}
