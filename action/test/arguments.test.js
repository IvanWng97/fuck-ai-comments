import assert from "node:assert/strict";
import test from "node:test";

import { argStringToArray } from "../../node_modules/@actions/exec/lib/toolrunner.js";

import {
  buildCheckArguments,
  buildCheckArgumentsFromInputs,
  executableCommand,
} from "../src/arguments.js";

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
      profile: "full",
      path: "source tree",
      base: "",
      head: "",
    }),
    [
      "check",
      "--all",
      "--profile",
      "full",
      "--format",
      "json",
      "--",
      "source tree",
    ],
  );
  assert.deepEqual(
    buildCheckArguments({
      mode: "staged",
      profile: "attestation",
      path: ".",
      base: "",
      head: "",
    }),
    [
      "check",
      "--staged",
      "--profile",
      "attestation",
      "--format",
      "json",
      "--",
      ".",
    ],
  );
  assert.deepEqual(
    buildCheckArguments({
      mode: "worktree",
      profile: "full",
      path: ".",
      base: "",
      head: "",
    }),
    ["check", "--profile", "full", "--format", "json", "--", "."],
  );
  assert.deepEqual(
    buildCheckArguments({
      mode: "base",
      profile: "attestation",
      path: ".",
      base: "base sha",
      head: "head sha",
    }),
    [
      "check",
      "--base",
      "base sha",
      "--head",
      "head sha",
      "--profile",
      "attestation",
      "--format",
      "json",
      "--",
      ".",
    ],
  );
});

test("passes profile validation to the Rust CLI", () => {
  assert.deepEqual(
    buildCheckArguments({
      mode: "all",
      profile: "attestation",
      path: ".",
      base: "",
      head: "",
    }),
    [
      "check",
      "--all",
      "--profile",
      "attestation",
      "--format",
      "json",
      "--",
      ".",
    ],
  );
  assert.deepEqual(
    buildCheckArguments({
      mode: "worktree",
      profile: "future-profile",
      path: ".",
      base: "",
      head: "",
    }),
    ["check", "--profile", "future-profile", "--format", "json", "--", "."],
  );
});

test("reads the Action profile input into the Rust CLI argv", () => {
  const inputs = new Map([
    ["mode", "worktree"],
    ["profile", "attestation"],
    ["path", "source tree"],
    ["base", ""],
    ["head", ""],
    ["config", "policy/custom.toml"],
  ]);

  assert.deepEqual(
    buildCheckArgumentsFromInputs((name) => inputs.get(name) ?? ""),
    [
      "check",
      "--profile",
      "attestation",
      "--config",
      "policy/custom.toml",
      "--format",
      "json",
      "--",
      "source tree",
    ],
  );
});

test("rejects ambiguous mode inputs", () => {
  assert.throws(
    () =>
      buildCheckArguments({
        mode: "base",
        profile: "full",
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
        profile: "full",
        path: ".",
        base: "main",
        head: "",
      }),
    /only valid/u,
  );
});
