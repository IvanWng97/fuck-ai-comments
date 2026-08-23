import assert from "node:assert/strict";
import test from "node:test";

import {
  FINDING_PATTERN,
  MATCHER_OWNER,
  PROBLEM_MATCHER,
  failureMessage,
  sarifDocument,
} from "../src/check.js";

const pattern = new RegExp(FINDING_PATTERN, "u");

test("the problem matcher captures file, line, rule, and message", () => {
  const match = pattern.exec(
    "src/policy.rs:261: comment-policy/type-comment-budget: type `Member` owns 2 comment lines for 7 code lines; allowance is 1",
  );

  assert.deepEqual(match?.slice(1), [
    "src/policy.rs",
    "261",
    "comment-policy/type-comment-budget",
    "type `Member` owns 2 comment lines for 7 code lines; allowance is 1",
  ]);
});

test("the problem matcher handles Windows paths and colons in messages", () => {
  const relative = pattern.exec(
    String.raw`nested\z.rs:1: comment-policy/leaf-comment-budget: 4 comment lines own Rust leaf ` +
      "`LIMIT`; allowance is 3",
  );
  assert.equal(relative?.[1], String.raw`nested\z.rs`);
  assert.equal(relative?.[2], "1");

  const absolute = pattern.exec(
    String.raw`C:\work\a.rs:3: comment-policy/comment-reparented: unchanged comment moved from type ` +
      "`A` to type `B`: edit it",
  );
  assert.equal(absolute?.[1], String.raw`C:\work\a.rs`);
  assert.equal(absolute?.[3], "comment-policy/comment-reparented");
  assert.equal(
    absolute?.[4],
    "unchanged comment moved from type `A` to type `B`: edit it",
  );
});

test("the problem matcher ignores summary and error lines", () => {
  for (const line of [
    "clean: 1 file scanned",
    "2 violations in 2 files",
    "error: could not parse src/lib.rs as Rust",
    "src/lib.rs:x: comment-policy/file-comment-budget: not a line",
  ]) {
    assert.equal(pattern.exec(line), null, line);
  }
});

test("the problem matcher document registers one owner", () => {
  assert.deepEqual(PROBLEM_MATCHER, {
    problemMatcher: [
      {
        owner: MATCHER_OWNER,
        pattern: [
          { regexp: FINDING_PATTERN, file: 1, line: 2, code: 3, message: 4 },
        ],
      },
    ],
  });
});

test("a clean run has no failure message", () => {
  assert.equal(
    failureMessage({
      exitCode: 0,
      stdout: "clean: 3 files scanned\n",
      stderr: "",
    }),
    null,
  );
});

test("violations fail with the CLI summary line", () => {
  assert.equal(
    failureMessage({
      exitCode: 1,
      stdout:
        "a.rs:1: comment-policy/leaf-comment-budget: detail\n2 violations in 2 files\n",
      stderr: "",
    }),
    "2 violations in 2 files",
  );
  assert.equal(
    failureMessage({ exitCode: 1, stdout: "", stderr: "" }),
    "fuck-ai-comments reported violations",
  );
});

test("CLI errors surface their detail", () => {
  assert.throws(
    () =>
      failureMessage({
        exitCode: 2,
        stdout: "",
        stderr: "error: could not parse src/lib.rs as Rust\n",
      }),
    /fuck-ai-comments exited with code 2: error: could not parse src\/lib\.rs as Rust/u,
  );
  assert.throws(
    () => failureMessage({ exitCode: 127, stdout: "", stderr: "" }),
    /fuck-ai-comments exited with code 127$/u,
  );
});

test("SARIF output is passed through when it is well-formed JSON", () => {
  const document = '{"version":"2.1.0","runs":[]}\n';

  assert.equal(
    sarifDocument({ exitCode: 1, stdout: document, stderr: "" }),
    document,
  );
  assert.equal(
    sarifDocument({ exitCode: 0, stdout: document, stderr: "" }),
    document,
  );
});

test("SARIF output is rejected when malformed or after a CLI error", () => {
  assert.throws(
    () => sarifDocument({ exitCode: 0, stdout: "{", stderr: "" }),
    /invalid SARIF report from fuck-ai-comments/u,
  );
  assert.throws(
    () => sarifDocument({ exitCode: 2, stdout: "", stderr: "error: broken\n" }),
    /fuck-ai-comments exited with code 2: error: broken/u,
  );
});
