use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Deserialize;

const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Default)]
pub(crate) struct PolicyConfig {
    narrative: Option<StaticPolicy>,
    documentation: Option<StaticPolicy>,
    public_documentation: Option<StaticPolicy>,
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
            documentation: file
                .comments
                .documentation
                .map(|policy| policy.resolve("comments.documentation"))
                .transpose()?,
            public_documentation: file
                .comments
                .public_documentation
                .map(|policy| policy.resolve("comments.public-documentation"))
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
        // Git never descends into an ignored parent, so query each hierarchy
        // level before honoring a child negation from a flat Git path list.
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
    pub(crate) fn narrative(&self) -> StaticPolicy {
        self.narrative.unwrap_or(StaticPolicy::Relative)
    }

    pub(crate) fn documentation(&self) -> StaticPolicy {
        self.documentation.unwrap_or(StaticPolicy::Relative)
    }

    pub(crate) fn public_documentation(&self) -> StaticPolicy {
        self.public_documentation.unwrap_or(StaticPolicy::Unlimited)
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
    documentation: Option<CommentPolicy>,
    public_documentation: Option<CommentPolicy>,
    safety_proof: Option<CommentPolicy>,
    tool_directive: Option<CommentPolicy>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CommentPolicy {
    mode: PolicyMode,
    max_lines: Option<usize>,
}

impl CommentPolicy {
    fn resolve(self, path: &str) -> Result<StaticPolicy, PolicyConfigError> {
        match (self.mode, self.max_lines) {
            (PolicyMode::Relative, None) => Ok(StaticPolicy::Relative),
            (PolicyMode::OwnerCapped, None) => Ok(StaticPolicy::OwnerCapped),
            (PolicyMode::Unlimited, None) => Ok(StaticPolicy::Unlimited),
            (PolicyMode::Capped, Some(max_lines)) => Ok(StaticPolicy::Capped(max_lines)),
            (PolicyMode::Capped, None) => Err(PolicyConfigError(format!(
                "{path}.max-lines is required when mode = \"capped\""
            ))),
            (PolicyMode::Relative | PolicyMode::OwnerCapped | PolicyMode::Unlimited, Some(_)) => {
                Err(PolicyConfigError(format!(
                    "{path}.max-lines is only valid when mode = \"capped\""
                )))
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PolicyMode {
    Relative,
    OwnerCapped,
    Capped,
    Unlimited,
}

/// A repository policy configuration was malformed or unsupported.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PolicyConfigError(String);
