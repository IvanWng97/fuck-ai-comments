const REPORT_SCHEMA_VERSION = 1;
// GitHub Actions permits 10 error annotations per step. core.setFailed emits
// the final error, so nine slots remain for source-level findings.
const MAX_FINDING_ANNOTATIONS = 9;
const BIDI_OR_LINE_SEPARATOR =
  /[\u061c\u200e\u200f\u2028\u2029\u202a-\u202e\u2066-\u2069]/gu;

export function processCliResult({ exitCode, stdout, stderr }, core) {
  if (exitCode !== 0 && exitCode !== 1) {
    const detail = stderr.trim() || stdout.trim();
    const suffix = detail ? `: ${detail}` : "";
    throw new Error(`fuck-ai-comments exited with code ${exitCode}${suffix}`);
  }

  const report = parseReport(stdout);
  const expectedExitCode = report.findings.length === 0 ? 0 : 1;
  if (exitCode !== expectedExitCode) {
    throw new Error(
      `fuck-ai-comments exit code ${exitCode} contradicts its JSON report with ${report.findings.length} ${noun(report.findings.length, "finding", "findings")}`,
    );
  }

  publishReport(report, core);
}

function parseReport(stdout) {
  let report;
  try {
    report = JSON.parse(stdout);
  } catch (error) {
    throw new Error(
      `invalid JSON report from fuck-ai-comments: ${error instanceof Error ? error.message : String(error)}`,
      { cause: error },
    );
  }

  requireRecord(report, "report");
  requireExactKeys(
    report,
    ["schemaVersion", "filesScanned", "findings"],
    "report",
  );
  if (report.schemaVersion !== REPORT_SCHEMA_VERSION) {
    throw new Error(
      `unsupported JSON report schema ${String(report.schemaVersion)}; expected ${REPORT_SCHEMA_VERSION}`,
    );
  }
  requireNonnegativeInteger(report.filesScanned, "report.filesScanned");
  if (!Array.isArray(report.findings)) {
    throw new Error("invalid report.findings: expected an array");
  }
  for (const [index, finding] of report.findings.entries()) {
    validateFinding(finding, index);
  }
  if (report.findings.length > 0 && report.filesScanned === 0) {
    throw new Error(
      "invalid report: findings require at least one scanned file",
    );
  }
  return report;
}

function validateFinding(finding, index) {
  const label = `report.findings[${index}]`;
  requireRecord(finding, label);
  requireExactKeys(finding, ["path", "line", "rule", "message"], label);
  requireNonemptyString(finding.path, `${label}.path`);
  requirePositiveInteger(finding.line, `${label}.line`);
  requireNonemptyString(finding.rule, `${label}.rule`);
  requireNonemptyString(finding.message, `${label}.message`);
}

function publishReport(report, core) {
  const count = report.findings.length;
  if (count === 0) {
    core.info(
      `clean: ${report.filesScanned} ${noun(report.filesScanned, "file", "files")} scanned`,
    );
    return;
  }

  core.startGroup(
    `${count} comment-policy ${noun(count, "violation", "violations")}`,
  );
  try {
    for (const finding of report.findings) {
      core.info(safeJsonLine(finding));
    }
  } finally {
    core.endGroup();
  }

  const annotated = Math.min(count, MAX_FINDING_ANNOTATIONS);
  for (const finding of report.findings.slice(0, annotated)) {
    core.error(finding.message, {
      file: finding.path,
      startLine: finding.line,
      endLine: finding.line,
      title: finding.rule,
    });
  }
  if (annotated < count) {
    core.info(`Annotated ${annotated} of ${count} violations`);
  }
  core.setFailed(
    `${count} ${noun(count, "violation", "violations")} in ${report.filesScanned} ${noun(report.filesScanned, "file", "files")}`,
  );
}

function safeJsonLine(value) {
  return JSON.stringify(value).replace(
    BIDI_OR_LINE_SEPARATOR,
    (character) =>
      `\\u${character.codePointAt(0).toString(16).padStart(4, "0")}`,
  );
}

function requireRecord(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`invalid ${label}: expected an object`);
  }
}

function requireExactKeys(value, expected, label) {
  const actual = Object.keys(value).sort();
  const sortedExpected = [...expected].sort();
  if (
    actual.length !== sortedExpected.length ||
    actual.some((key, index) => key !== sortedExpected[index])
  ) {
    throw new Error(`invalid ${label}: unexpected or missing fields`);
  }
}

function requireNonnegativeInteger(value, label) {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`invalid ${label}: expected a nonnegative integer`);
  }
}

function requirePositiveInteger(value, label) {
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new Error(`invalid ${label}: expected a positive integer`);
  }
}

function requireNonemptyString(value, label) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`invalid ${label}: expected a nonempty string`);
  }
}

function noun(count, singular, plural) {
  return count === 1 ? singular : plural;
}
