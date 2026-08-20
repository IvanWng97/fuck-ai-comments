use serde::Deserialize;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default)]
pub(crate) struct PolicyConfig {
    narrative: Option<StaticPolicy>,
    rustdoc: Option<StaticPolicy>,
    safety_proof: Option<StaticPolicy>,
    tool_directive: Option<StaticPolicy>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum StaticPolicy {
    Relative,
    OwnerCapped,
    Capped(usize),
    Unlimited,
}

impl PolicyConfig {
    pub(crate) fn parse(source: &str) -> Result<Self, PolicyConfigError> {
        let file: ConfigFile = toml_edit::de::from_str(source)
            .map_err(|error| PolicyConfigError(format!("invalid TOML: {error}")))?;
        if file.schema_version != SCHEMA_VERSION {
            return Err(PolicyConfigError(format!(
                "unsupported schema-version {}; expected {SCHEMA_VERSION}",
                file.schema_version
            )));
        }

        Ok(Self {
            narrative: file
                .comments
                .narrative
                .map(|policy| policy.resolve("comments.narrative"))
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
        })
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
    comments: CommentPolicies,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CommentPolicies {
    narrative: Option<CommentPolicy>,
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
