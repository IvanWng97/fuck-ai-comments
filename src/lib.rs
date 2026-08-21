//! Owner-aware comment policy shared by the CLI and editor integrations.

#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

mod change;
mod config;
mod identity;
mod languages;
mod model;
mod policy;

pub use config::{PolicyConfigError, RepositoryConfig};
pub use model::{AnalysisError, AnalysisProfile, Finding, SourceFile};

/// Repository-level facts used to classify source files during analysis.
///
/// The default context performs no repository discovery and recognizes only
/// structural paths ending in `src/lib.rs` as Rust library roots. Use
/// [`Self::from_rust_library_roots`] when an external authority such as Cargo
/// has supplied the complete set of library target roots for a repository.
#[derive(Debug, Clone, Default)]
pub struct AnalysisContext {
    rust_targets: RustTargets,
    policy: config::PolicyConfig,
}

#[derive(Debug, Clone, Default)]
enum RustTargets {
    #[default]
    ConventionalPaths,
    CargoLibraryRoots(BTreeSet<PathBuf>),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum RustFileRole {
    CrateRoot,
    ModuleOrUnknown,
}

/// Repository facts supplied to [`AnalysisContext`] were malformed or unsafe.
#[derive(Debug, thiserror::Error)]
#[error("invalid analysis context: {0}")]
pub struct AnalysisContextError(String);

impl AnalysisContext {
    /// Build a context from Cargo-proven library roots.
    ///
    /// The supplied set is authoritative, including when it is empty. Paths
    /// are normalized by removing `.` components. Relative and absolute paths
    /// are accepted so callers can use the same coordinates as [`SourceFile`],
    /// but paths containing `..` are rejected.
    ///
    /// # Errors
    ///
    /// Returns an error when a root is empty or contains `..`.
    pub fn from_rust_library_roots<I, P>(roots: I) -> Result<Self, AnalysisContextError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let roots = roots
            .into_iter()
            .map(|path| normalize_repository_path(path.as_ref()))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            rust_targets: RustTargets::CargoLibraryRoots(roots),
            policy: config::PolicyConfig::default(),
        })
    }

    /// Apply comment policies from a repository configuration TOML document.
    ///
    /// The complete document is validated, including exclusion patterns, but
    /// this analysis-only context does not walk files. Repository scanners can
    /// parse [`RepositoryConfig`] once and apply both its policies and path
    /// exclusions.
    ///
    /// # Errors
    ///
    /// Returns an error when the document is malformed, uses an unsupported
    /// schema version, or contains an invalid configuration declaration.
    pub fn with_policy_toml(mut self, source: &str) -> Result<Self, PolicyConfigError> {
        let config = RepositoryConfig::from_toml(source)?;
        self.policy = config.policy().clone();
        Ok(self)
    }

    /// Apply the comment policies from a parsed repository configuration.
    #[must_use]
    pub fn with_repository_config(mut self, config: &RepositoryConfig) -> Self {
        self.policy = config.policy().clone();
        self
    }

    pub(crate) fn policy(&self) -> &config::PolicyConfig {
        &self.policy
    }

    pub(crate) fn rust_file_role(&self, path: &Path) -> RustFileRole {
        let crate_root = match &self.rust_targets {
            RustTargets::ConventionalPaths => path.ends_with(Path::new("src/lib.rs")),
            RustTargets::CargoLibraryRoots(roots) => {
                normalize_repository_path(path).is_ok_and(|normalized| roots.contains(&normalized))
            }
        };
        if crate_root {
            RustFileRole::CrateRoot
        } else {
            RustFileRole::ModuleOrUnknown
        }
    }

    /// Analyze every owner in one source snapshot using this repository context.
    ///
    /// # Errors
    ///
    /// Returns an error when the file extension is unsupported or parsing fails.
    pub fn analyze_all(&self, file: SourceFile<'_>) -> Result<Vec<Finding>, AnalysisError> {
        self.analyze_all_with_profile(file, AnalysisProfile::Full)
    }

    /// Analyze one source snapshot under the selected profile and repository context.
    ///
    /// # Errors
    ///
    /// Returns an error when the file extension is unsupported, parsing fails,
    /// or the normalized ownership model is invalid.
    pub fn analyze_all_with_profile(
        &self,
        file: SourceFile<'_>,
        profile: AnalysisProfile,
    ) -> Result<Vec<Finding>, AnalysisError> {
        if profile.runs_static_policy() {
            languages::analyze_file_with_context(
                self,
                file.path,
                file.text,
                &model::Selection::all(),
            )
        } else {
            languages::parse_validated_file_with_context(self, file.path, file.text)?;
            Ok(Vec::new())
        }
    }

    /// Analyze only owners changed between two snapshots using this repository context.
    ///
    /// # Errors
    ///
    /// Returns an error when either snapshot cannot be parsed, exceeds the diff
    /// engine's token capacity, or ownership cannot be paired without guessing.
    pub fn analyze_change(
        &self,
        before: SourceFile<'_>,
        after: SourceFile<'_>,
    ) -> Result<Vec<Finding>, AnalysisError> {
        self.analyze_change_with_profile(before, after, AnalysisProfile::Full)
    }

    /// Analyze a source change under the selected profile and repository context.
    ///
    /// # Errors
    ///
    /// Returns an error when either snapshot cannot be parsed or exceeds the
    /// diff engine's token capacity, when the snapshots use different language
    /// adapters, or when ownership cannot be paired without guessing.
    pub fn analyze_change_with_profile(
        &self,
        before: SourceFile<'_>,
        after: SourceFile<'_>,
        profile: AnalysisProfile,
    ) -> Result<Vec<Finding>, AnalysisError> {
        change::analyze_with_context(self, before, after, profile)
    }
}

fn normalize_repository_path(path: &Path) -> Result<PathBuf, AnalysisContextError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => normalized.push(component),
            Component::RootDir | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
            Component::ParentDir => {
                return Err(AnalysisContextError(format!(
                    "Rust library root {} contains a parent traversal",
                    path.display()
                )));
            }
        }
    }
    if normalized.as_os_str().is_empty() || normalized.file_name().is_none() {
        return Err(AnalysisContextError(
            "Rust library root is empty".to_owned(),
        ));
    }
    Ok(normalized)
}

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
    AnalysisContext::default().analyze_all_with_profile(file, profile)
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
