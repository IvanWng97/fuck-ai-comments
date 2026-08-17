import assert from "node:assert/strict";
import test from "node:test";

import { argStringToArray } from "../../node_modules/@actions/exec/lib/toolrunner.js";

import { buildCheckArguments, executableCommand } from "../src/arguments.js";

test("quotes a full executable path for the actions exec parser", () => {
  const executable = String.raw`C:\runner tools\fuck-ai-comments.exe`;

  assert.deepEqual(argStringToArray(executableCommand(executable)), [
    executable,
  ]);
});

test("rejects an executable path containing a quote", () => {
  assert.throws(
    () => executableCommand('/runner tools/"quoted"/fuck-ai-comments'),
    /cannot safely represent executable path/u,
  );
});

test("rejects an executable path containing a control character", () => {
  assert.throws(
    () => executableCommand("/runner tools\n/fuck-ai-comments"),
    /cannot safely represent executable path/u,
  );
});

test("constructs each explicit mode as an argv array", () => {
  assert.deepEqual(
    buildCheckArguments({
      mode: "all",
      path: "source tree",
      base: "",
      head: "",
    }),
    ["check", "--all", "--", "source tree"],
  );
  assert.deepEqual(
    buildCheckArguments({
      mode: "staged",
      path: ".",
      base: "",
      head: "",
    }),
    ["check", "--staged", "--", "."],
  );
  assert.deepEqual(
    buildCheckArguments({
      mode: "worktree",
      path: ".",
      base: "",
      head: "",
    }),
    ["check", "--", "."],
  );
  assert.deepEqual(
    buildCheckArguments({
      mode: "base",
      path: ".",
      base: "base sha",
      head: "head sha",
    }),
    ["check", "--base", "base sha", "--head", "head sha", "--", "."],
  );
});

test("rejects ambiguous mode inputs", () => {
  assert.throws(
    () =>
      buildCheckArguments({
        mode: "base",
        path: ".",
        base: "",
        head: "",
      }),
    /base is required/u,
  );
  assert.throws(
    () =>
      buildCheckArguments({
        mode: "all",
        path: ".",
        base: "main",
        head: "",
      }),
    /only valid/u,
  );
});
