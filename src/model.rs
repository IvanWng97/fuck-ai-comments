use std::collections::BTreeSet;
use std::path::Path;

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
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, serde::Serialize)]
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
    Member,
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
