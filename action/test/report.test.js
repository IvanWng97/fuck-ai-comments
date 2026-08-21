import assert from "node:assert/strict";
import test from "node:test";

import { processCliResult } from "../src/report.js";

function jsonReport({ filesScanned = 1, findings = [] } = {}) {
  return JSON.stringify({ schemaVersion: 1, filesScanned, findings });
}

function finding(index) {
  return {
    path: `src/file-${index}.rs`,
    line: index + 1,
    rule: `comment-policy/rule-${index}`,
    message: `Finding ${index}`,
  };
}

function recordingCore() {
  const calls = {
    errors: [],
    failures: [],
    groups: [],
    infos: [],
  };
  return {
    calls,
    core: {
      endGroup() {
        calls.groups.push("end");
      },
      error(message, properties) {
        calls.errors.push({ message, properties });
      },
      info(message) {
        calls.infos.push(message);
      },
      setFailed(message) {
        calls.failures.push(message);
      },
      startGroup(message) {
        calls.groups.push(message);
      },
    },
  };
}

test("publishes a clean versioned CLI report", () => {
  const { calls, core } = recordingCore();

  processCliResult(
    {
      exitCode: 0,
      stdout: jsonReport({ filesScanned: 2 }),
      stderr: "",
    },
    core,
  );

  assert.deepEqual(calls.infos, ["clean: 2 files scanned"]);
  assert.deepEqual(calls.errors, []);
  assert.deepEqual(calls.failures, []);
});

test("annotates nine findings, logs every finding, and fails once", () => {
  const { calls, core } = recordingCore();
  const findings = Array.from({ length: 12 }, (_, index) => finding(index));

  processCliResult(
    {
      exitCode: 1,
      stdout: jsonReport({ filesScanned: 3, findings }),
      stderr: "",
    },
    core,
  );

  assert.equal(calls.errors.length, 9);
  assert.deepEqual(calls.errors[0], {
    message: "Finding 0",
    properties: {
      file: "src/file-0.rs",
      startLine: 1,
      endLine: 1,
      title: "comment-policy/rule-0",
    },
  });
  assert.equal(
    calls.infos.filter((message) => message.includes("comment-policy/rule-"))
      .length,
    12,
  );
  assert.ok(calls.infos.includes("Annotated 9 of 12 violations"));
  assert.deepEqual(calls.groups, ["12 comment-policy violations", "end"]);
  assert.deepEqual(calls.failures, ["12 violations in 3 files"]);
});

test("rejects malformed or incompatible reports", () => {
  const malformedReports = [
    "not JSON",
    JSON.stringify({ schemaVersion: 2, filesScanned: 0, findings: [] }),
    JSON.stringify({ schemaVersion: 1, filesScanned: -1, findings: [] }),
    JSON.stringify({ schemaVersion: 1, filesScanned: 1, findings: [{}] }),
    JSON.stringify({
      schemaVersion: 1,
      filesScanned: 1,
      findings: [],
      unexpected: true,
    }),
  ];

  for (const stdout of malformedReports) {
    const { core } = recordingCore();
    assert.throws(
      () => processCliResult({ exitCode: 0, stdout, stderr: "" }, core),
      /invalid JSON report|unsupported JSON report|invalid report/u,
    );
  }
});

test("rejects a CLI exit code that contradicts its report", () => {
  const { core } = recordingCore();

  assert.throws(
    () =>
      processCliResult(
        {
          exitCode: 0,
          stdout: jsonReport({ findings: [finding(0)] }),
          stderr: "",
        },
        core,
      ),
    /exit code 0.*1 finding/u,
  );
});

test("surfaces trusted-analysis failures from stderr", () => {
  const { core } = recordingCore();

  assert.throws(
    () =>
      processCliResult(
        {
          exitCode: 2,
          stdout: "",
          stderr: "error: could not parse source.rs as Rust\n",
        },
        core,
      ),
    /could not parse source\.rs as Rust/u,
  );
});

test("logs hostile finding text as one inert JSON line", () => {
  const { calls, core } = recordingCore();
  const hostile = {
    path: "src/safe.rs\n::error::injected",
    line: 1,
    rule: "comment-policy/test",
    message: "visible\u202ehidden",
  };

  processCliResult(
    {
      exitCode: 1,
      stdout: jsonReport({ findings: [hostile] }),
      stderr: "",
    },
    core,
  );

  const logLine = calls.infos.find((message) => message.includes("injected"));
  assert.ok(logLine);
  assert.equal(logLine.includes("\n"), false);
  assert.equal(logLine.includes("\u202e"), false);
  assert.ok(logLine.includes("\\n::error::injected"));
  assert.ok(logLine.includes("\\u202e"));
});
