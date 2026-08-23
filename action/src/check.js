export const MATCHER_OWNER = "fuck-ai-comments";

// Matches one CLI text finding: `path:line: rule: message`.
export const FINDING_PATTERN = String.raw`^(.+?):(\d+): (comment-policy/[a-z-]+): (.+)$`;

export const PROBLEM_MATCHER = {
  problemMatcher: [
    {
      owner: MATCHER_OWNER,
      pattern: [
        {
          regexp: FINDING_PATTERN,
          file: 1,
          line: 2,
          code: 3,
          message: 4,
        },
      ],
    },
  ],
};

export function failureMessage({ exitCode, stdout, stderr }) {
  if (exitCode === 0) {
    return null;
  }
  if (exitCode === 1) {
    return lastLine(stdout) ?? "fuck-ai-comments reported violations";
  }
  throw new Error(cliError(exitCode, stdout, stderr));
}

export function sarifDocument({ exitCode, stdout, stderr }) {
  if (exitCode !== 0 && exitCode !== 1) {
    throw new Error(cliError(exitCode, stdout, stderr));
  }
  try {
    JSON.parse(stdout);
  } catch (error) {
    throw new Error(
      `invalid SARIF report from fuck-ai-comments: ${error instanceof Error ? error.message : String(error)}`,
      { cause: error },
    );
  }
  return stdout;
}

function cliError(exitCode, stdout, stderr) {
  const detail = stderr.trim() || stdout.trim();
  const suffix = detail ? `: ${detail}` : "";
  return `fuck-ai-comments exited with code ${exitCode}${suffix}`;
}

function lastLine(text) {
  const lines = text
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter(Boolean);
  return lines.at(-1) ?? null;
}
