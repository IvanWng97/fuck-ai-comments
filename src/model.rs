use std::collections::BTreeSet;

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

/// Changed lines and deletion anchors used to select affected owners.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Added or edited lines in the new source.
    pub changed: BTreeSet<usize>,
    /// New lines plus anchors for deletion-only hunks.
    pub owners: BTreeSet<usize>,
}

impl Selection {
    /// Select every physical line in `source`.
    #[must_use]
    pub fn all(source: &str) -> Self {
        let lines: BTreeSet<_> = (1..=source.lines().count()).collect();
        Self {
            changed: lines.clone(),
            owners: lines,
        }
    }
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
}
