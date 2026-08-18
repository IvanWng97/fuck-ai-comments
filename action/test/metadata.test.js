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

test("ordinary CI verifies Rust license notices and package contents", async () => {
  const workflow = parseYaml(
    await readFile(".github/workflows/ci.yml", "utf8"),
  );
  const manifest = JSON.parse(await readFile("package.json", "utf8"));
  const actionSteps = workflow.jobs.action.steps;
  const rustSteps = workflow.jobs.rust.steps;

  assert.equal(
    manifest.scripts["rust-licenses:install"],
    "cargo install cargo-about --version 0.9.1 --features cli --locked",
  );
  assert.equal(
    manifest.scripts["rust-licenses:check"],
    "node action/scripts/rust-licenses.js --check",
  );
  assert.deepEqual(
    actionSteps.find((step) => step.name === "Install cargo-about"),
    {
      name: "Install cargo-about",
      if: "runner.os == 'Linux'",
      run: "npm run rust-licenses:install",
    },
  );
  assert.deepEqual(
    actionSteps.find((step) => step.name === "Verify Rust license notices"),
    {
      name: "Verify Rust license notices",
      if: "runner.os == 'Linux'",
      run: "npm run rust-licenses:check",
    },
  );
  assert.deepEqual(
    rustSteps.find((step) => step.name === "Verify distributable package"),
    {
      name: "Verify distributable package",
      if: "runner.os == 'Linux'",
      run: "cargo package --list --locked | grep -Fx THIRD_PARTY_LICENSES\ncargo package --locked\n",
    },
  );
});
