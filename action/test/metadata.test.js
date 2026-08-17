import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { parse as parseToml } from "smol-toml";
import { parse as parseYaml } from "yaml";

test("action metadata uses Node 24 and a committed bundle", async () => {
  const metadata = parseYaml(await readFile("action.yml", "utf8"));
  const cargo = parseToml(await readFile("Cargo.toml", "utf8"));

  assert.equal(metadata.runs.using, "node24");
  assert.equal(metadata.runs.main, "dist/index.js");
  assert.equal(metadata.runs.steps, undefined);
  assert.equal(metadata.inputs.version.default, cargo.package.version);
  assert.equal(metadata.inputs.mode.default, "base");
  assert.equal(metadata.inputs.path.default, ".");
  assert.deepEqual(Object.keys(metadata.inputs).toSorted(), [
    "base",
    "head",
    "mode",
    "path",
    "version",
  ]);
});

test("workflow actions use official major tags", async () => {
  for (const workflow of [
    ".github/workflows/ci.yml",
    ".github/workflows/release.yml",
  ]) {
    const contents = await readFile(workflow, "utf8");
    const references = [...contents.matchAll(/uses:\s+[^@\s]+@([^\s]+)/gu)];
    assert.ok(references.length > 0, `${workflow} has no action references`);
    for (const [, reference] of references) {
      assert.match(reference, /^v\d+$/u, `${workflow}: ${reference}`);
    }
  }
});

test("README uses the Action major for the current package version", async () => {
  const cargo = parseToml(await readFile("Cargo.toml", "utf8"));
  const readme = await readFile("README.md", "utf8");
  const major = cargo.package.version.split(".")[0];
  const references = [
    ...readme.matchAll(/IvanWng97\/fuck-ai-comments@(v\d+)/gu),
  ].map((match) => match[1]);

  assert.ok(references.length > 0, "README has no Action example");
  assert.deepEqual(new Set(references), new Set([`v${major}`]));
});

test("ordinary CI has read-only repository permissions", async () => {
  const workflow = parseYaml(
    await readFile(".github/workflows/ci.yml", "utf8"),
  );

  assert.deepEqual(workflow.permissions, { contents: "read" });
});

test("ordinary CI exposes one stable aggregate gate", async () => {
  const workflow = parseYaml(
    await readFile(".github/workflows/ci.yml", "utf8"),
  );

  assert.deepEqual(workflow.jobs["ci-gate"].needs.toSorted(), [
    "action",
    "msrv",
    "rust",
  ]);
  assert.match(
    workflow.jobs["ci-gate"].steps[0].run,
    /RUST_RESULT.*MSRV_RESULT.*ACTION_RESULT/su,
  );
});
