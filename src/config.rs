use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Deserialize;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default)]
pub(crate) struct PolicyConfig {
    narrative: Option<StaticPolicy>,
    docstring: Option<StaticPolicy>,
    rustdoc: Option<StaticPolicy>,
    safety_proof: Option<StaticPolicy>,
    tool_directive: Option<StaticPolicy>,
}

/// Parsed repository configuration shared by analyzers and repository scanners.
///
/// Exclusion patterns use gitignore syntax and match repository-relative paths.
#[derive(Debug, Clone)]
pub struct RepositoryConfig {
    policy: PolicyConfig,
    exclusions: Gitignore,
}

impl Default for RepositoryConfig {
    fn default() -> Self {
        Self {
            policy: PolicyConfig::default(),
            exclusions: Gitignore::empty(),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum StaticPolicy {
    Relative,
    OwnerCapped,
    Capped(usize),
    Unlimited,
}

impl RepositoryConfig {
    /// Parse a strict, versioned repository configuration from TOML.
    ///
    /// # Errors
    ///
    /// Returns an error when the TOML, schema version, comment policy, or
    /// gitignore-style exclusion pattern is invalid.
    pub fn from_toml(source: &str) -> Result<Self, PolicyConfigError> {
        let file: ConfigFile = toml_edit::de::from_str(source)
            .map_err(|error| PolicyConfigError(format!("invalid TOML: {error}")))?;
        if file.schema_version != SCHEMA_VERSION {
            return Err(PolicyConfigError(format!(
                "unsupported schema-version {}; expected {SCHEMA_VERSION}",
                file.schema_version
            )));
        }

        let policy = PolicyConfig {
            narrative: file
                .comments
                .narrative
                .map(|policy| policy.resolve("comments.narrative"))
                .transpose()?,
            docstring: file
                .comments
                .docstring
                .map(|policy| policy.resolve("comments.docstring"))
                .transpose()?,
            rustdoc: file
                .comments
                .rustdoc
                .map(|policy| policy.resolve("comments.rustdoc"))
                .transpose()?,
            safety_proof: file
                .comments
                .safety_proof
                .map(|policy| policy.resolve("comments.safety-proof"))
                .transpose()?,
            tool_directive: file
                .comments
                .tool_directive
                .map(|policy| policy.resolve("comments.tool-directive"))
                .transpose()?,
        };
        let mut exclusions = GitignoreBuilder::new("");
        for (index, pattern) in file.exclude.iter().enumerate() {
            exclusions.add_line(None, pattern).map_err(|error| {
                PolicyConfigError(format!(
                    "invalid exclude pattern {} ({pattern:?}): {error}",
                    index + 1
                ))
            })?;
        }
        let exclusions = exclusions.build().map_err(|error| {
            PolicyConfigError(format!("could not compile exclude patterns: {error}"))
        })?;

        Ok(Self { policy, exclusions })
    }

    /// Return whether a repository-relative path is excluded by configuration.
    ///
    /// Absolute paths and paths containing parent traversal never match, so an
    /// invalid caller path cannot broaden an exclusion boundary.
    #[must_use]
    pub fn excludes_path(&self, path: &Path, is_directory: bool) -> bool {
        if path.as_os_str().is_empty()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return false;
        }
        let path = if path
            .components()
            .any(|component| matches!(component, Component::CurDir))
        {
            let mut normalized = PathBuf::new();
            for component in path.components() {
                if let Component::Normal(component) = component {
                    normalized.push(component);
                }
            }
            Cow::Owned(normalized)
        } else {
            Cow::Borrowed(path)
        };
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return false;
        }
        // Git does not descend into an ignored directory, so a child negation
        // cannot re-include a path through an ignored parent. Query the
        // library matcher at each hierarchy level to preserve that rule for
        // the flat path lists used by Git change modes.
        let ignored_parent = path
            .ancestors()
            .skip(1)
            .take_while(|parent| !parent.as_os_str().is_empty())
            .any(|parent| self.exclusions.matched(parent, true).is_ignore());
        ignored_parent || self.exclusions.matched(path, is_directory).is_ignore()
    }

    pub(crate) fn policy(&self) -> &PolicyConfig {
        &self.policy
    }
}

impl PolicyConfig {
    pub(crate) fn docstring(&self) -> StaticPolicy {
        self.docstring.unwrap_or(StaticPolicy::Relative)
    }

    pub(crate) fn narrative(&self) -> StaticPolicy {
        self.narrative.unwrap_or(StaticPolicy::Relative)
    }

    pub(crate) fn rustdoc(&self, public: bool) -> StaticPolicy {
        self.rustdoc.unwrap_or(if public {
            StaticPolicy::Unlimited
        } else {
            StaticPolicy::Relative
        })
    }

    pub(crate) fn safety_proof(&self) -> StaticPolicy {
        self.safety_proof.unwrap_or(StaticPolicy::OwnerCapped)
    }

    pub(crate) fn tool_directive(&self) -> StaticPolicy {
        self.tool_directive.unwrap_or(StaticPolicy::OwnerCapped)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ConfigFile {
    schema_version: u32,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    comments: CommentPolicies,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CommentPolicies {
    narrative: Option<CommentPolicy>,
    docstring: Option<CommentPolicy>,
    rustdoc: Option<CommentPolicy>,
    safety_proof: Option<CommentPolicy>,
    tool_directive: Option<CommentPolicy>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CommentPolicy {
    policy: PolicyMode,
    max_lines: Option<usize>,
}

impl CommentPolicy {
    fn resolve(self, path: &str) -> Result<StaticPolicy, PolicyConfigError> {
        match (self.policy, self.max_lines) {
            (PolicyMode::Relative, None) => Ok(StaticPolicy::Relative),
            (PolicyMode::Unlimited, None) => Ok(StaticPolicy::Unlimited),
            (PolicyMode::Capped, Some(max_lines)) if max_lines > 0 => {
                Ok(StaticPolicy::Capped(max_lines))
            }
            (PolicyMode::Capped, Some(_)) => Err(PolicyConfigError(format!(
                "{path}.max-lines must be greater than zero"
            ))),
            (PolicyMode::Capped, None) => Err(PolicyConfigError(format!(
                "{path}.max-lines is required when policy = \"capped\""
            ))),
            (PolicyMode::Relative | PolicyMode::Unlimited, Some(_)) => Err(PolicyConfigError(
                format!("{path}.max-lines is only valid when policy = \"capped\""),
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PolicyMode {
    Relative,
    Capped,
    Unlimited,
}

/// A repository policy configuration was malformed or unsupported.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PolicyConfigError(String);
