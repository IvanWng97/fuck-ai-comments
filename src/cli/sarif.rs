//! Minimal SARIF 2.1.0 emitter covering the subset GitHub code scanning consumes.

use std::io::{self, Write};

use fuck_ai_comments::{Finding, rules};
use serde::Serialize;

use super::check::Report;

const SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const VERSION: &str = "2.1.0";
const ERROR_LEVEL: &str = "error";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Sarif<'report> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: [Run<'report>; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Run<'report> {
    tool: Tool,
    results: Vec<SarifResult<'report>>,
}

#[derive(Serialize)]
struct Tool {
    driver: Driver,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Driver {
    name: &'static str,
    semantic_version: &'static str,
    information_uri: &'static str,
    rules: Vec<ReportingDescriptor>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportingDescriptor {
    id: &'static str,
    name: &'static str,
    short_description: Text<&'static str>,
    full_description: Text<&'static str>,
    help: Help,
    default_configuration: Configuration,
    properties: RuleProperties,
}

#[derive(Serialize)]
struct Text<T> {
    text: T,
}

#[derive(Serialize)]
struct Help {
    text: &'static str,
    markdown: &'static str,
}

#[derive(Serialize)]
struct Configuration {
    level: &'static str,
}

#[derive(Serialize)]
struct RuleProperties {
    #[serde(rename = "problem.severity")]
    problem_severity: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult<'report> {
    rule_id: &'report str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule_index: Option<usize>,
    level: &'static str,
    message: Text<&'report str>,
    locations: [Location; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    physical_location: PhysicalLocation,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalLocation {
    artifact_location: ArtifactLocation,
    region: Region,
}

#[derive(Serialize)]
struct ArtifactLocation {
    uri: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    start_line: usize,
}

pub(super) fn write_sarif_report(report: &Report, output: &mut impl Write) -> io::Result<()> {
    let sarif = Sarif {
        schema: SCHEMA,
        version: VERSION,
        runs: [Run {
            tool: Tool {
                driver: Driver {
                    name: env!("CARGO_PKG_NAME"),
                    semantic_version: env!("CARGO_PKG_VERSION"),
                    information_uri: env!("CARGO_PKG_REPOSITORY"),
                    rules: rules::ALL.iter().map(descriptor).collect(),
                },
            },
            results: report.findings.iter().map(result).collect(),
        }],
    };
    serde_json::to_writer(&mut *output, &sarif).map_err(|error| {
        let kind = error.io_error_kind().unwrap_or(io::ErrorKind::Other);
        io::Error::new(kind, error)
    })?;
    writeln!(output)
}

fn descriptor(rule: &rules::Rule) -> ReportingDescriptor {
    ReportingDescriptor {
        id: rule.id,
        name: rule.name,
        short_description: Text {
            text: rule.short_description,
        },
        full_description: Text {
            text: rule.full_description,
        },
        help: Help {
            text: rule.help,
            markdown: rule.help,
        },
        default_configuration: Configuration { level: ERROR_LEVEL },
        properties: RuleProperties {
            problem_severity: ERROR_LEVEL,
        },
    }
}

fn result(finding: &Finding) -> SarifResult<'_> {
    SarifResult {
        rule_id: finding.rule,
        rule_index: rules::ALL.iter().position(|rule| rule.id == finding.rule),
        level: ERROR_LEVEL,
        message: Text {
            text: &finding.message,
        },
        locations: [Location {
            physical_location: PhysicalLocation {
                artifact_location: ArtifactLocation {
                    uri: artifact_uri(&finding.path),
                },
                region: Region {
                    start_line: finding.line,
                },
            },
        }],
    }
}

fn artifact_uri(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::artifact_uri;

    #[test]
    fn artifact_uris_use_forward_slashes() {
        assert_eq!(artifact_uri("nested\\z.rs"), "nested/z.rs");
        assert_eq!(artifact_uri("nested/z.rs"), "nested/z.rs");
    }
}
