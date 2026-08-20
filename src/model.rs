use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// Repository-level facts used to classify source files during analysis.
///
/// The default context performs no repository discovery and recognizes only
/// structural paths ending in `src/lib.rs` as Rust library roots. Use
/// [`Self::from_rust_library_roots`] when an external authority such as Cargo
/// has supplied the complete set of library target roots for a repository.
#[derive(Debug, Clone, Default)]
pub struct AnalysisContext {
    rust_targets: RustTargets,
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
        })
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

/// Repository facts supplied to [`AnalysisContext`] were malformed or unsafe.
#[derive(Debug, thiserror::Error)]
#[error("invalid analysis context: {0}")]
pub struct AnalysisContextError(String);

/// Selects which comment-policy guarantees an analysis entry point enforces.
#[non_exhaustive]
#[derive(clap::ValueEnum, Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum AnalysisProfile {
    /// Enforce static comment budgets and change attestations.
    #[default]
    #[value(name = "full")]
    Full,
    /// Validate source structure and enforce only change attestations.
    #[value(name = "attestation")]
    Attestation,
}

impl AnalysisProfile {
    pub(crate) fn runs_static_policy(self) -> bool {
        match self {
            Self::Full => true,
            Self::Attestation => false,
        }
    }
}

/// One source snapshot supplied to an analysis entry point.
#[derive(Debug, Clone, Copy)]
pub struct SourceFile<'source> {
    /// Path used to select the language adapter and report findings.
    pub path: &'source Path,
    /// UTF-8 source text at this revision.
    pub text: &'source str,
}

/// One required policy violation.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct Finding {
    /// Repository-relative file path.
    pub path: String,
    /// One-based source line.
    pub line: usize,
    /// Stable machine-readable rule identifier.
    pub rule: &'static str,
    /// Human-readable remediation.
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Selection {
    pub(crate) owners: BTreeSet<OwnerSelection>,
    all: bool,
}

impl Selection {
    pub(crate) fn all() -> Self {
        Self {
            owners: BTreeSet::new(),
            all: true,
        }
    }

    pub(crate) fn select_owner(&mut self, kind: OwnerKind, start_byte: usize, end_byte: usize) {
        self.owners.insert(OwnerSelection {
            kind,
            start_byte,
            end_byte,
        });
    }

    pub(crate) fn selects_owner(
        &self,
        kind: OwnerKind,
        start_byte: usize,
        end_byte: usize,
    ) -> bool {
        self.all
            || self.owners.contains(&OwnerSelection {
                kind,
                start_byte,
                end_byte,
            })
    }

    pub(crate) fn project_into(&self, start_byte: usize, end_byte: usize) -> Self {
        let owners = self
            .owners
            .iter()
            .filter(|owner| start_byte <= owner.start_byte && owner.end_byte <= end_byte)
            .map(|owner| OwnerSelection {
                kind: owner.kind,
                start_byte: owner.start_byte - start_byte,
                end_byte: owner.end_byte - start_byte,
            })
            .collect();
        Self {
            owners,
            all: self.all,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum OwnerKind {
    File,
    Function,
    Type,
    Leaf,
    Template,
    TomlKey,
}

#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct OwnerSelection {
    pub(crate) kind: OwnerKind,
    pub(crate) start_byte: usize,
    pub(crate) end_byte: usize,
}

/// A source file could not be analyzed without guessing.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    /// The file extension has no registered language adapter.
    #[error("unsupported source file: {0}")]
    Unsupported(String),
    /// Tree-sitter rejected the configured grammar.
    #[error("could not initialize the {language} parser: {detail}")]
    ParserInit {
        /// Language adapter being initialized.
        language: &'static str,
        /// Parser error returned by Tree-sitter.
        detail: String,
    },
    /// The source contains syntax errors, so owner relationships are unreliable.
    #[error("could not parse {path} as {language}")]
    Parse {
        /// File that failed to parse.
        path: String,
        /// Language selected from the file extension.
        language: &'static str,
    },
    /// TOML syntax or string boundaries were invalid.
    #[error("could not parse {path} as TOML: {detail}")]
    Toml {
        /// File that failed to parse.
        path: String,
        /// Parser or lexer error.
        detail: String,
    },
    /// Old/new owners or comments could not be paired uniquely.
    #[error("ambiguous change: {0}")]
    AmbiguousChange(String),
    /// A change snapshot exceeded the diff engine's token capacity.
    #[error("{snapshot} snapshot has {tokens} diff tokens; supported limit is {maximum} tokens")]
    DiffCapacity {
        /// Revision side whose token sequence exceeded the limit.
        snapshot: &'static str,
        /// Number of tokens in the rejected snapshot.
        tokens: usize,
        /// Largest token sequence accepted by the diff engine.
        maximum: usize,
    },
    /// A language adapter violated the normalized ownership contract.
    #[error("invalid analysis model: {0}")]
    Invariant(String),
}
