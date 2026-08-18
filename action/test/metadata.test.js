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
    ".github/workflows/codspeed.yml",
    ".github/workflows/coverage.yml",
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

test("Codecov uploads one explicit Rust coverage report", async () => {
  const workflowContents = await readFile(
    ".github/workflows/coverage.yml",
    "utf8",
  );
  const workflow = parseYaml(workflowContents);
  const config = parseYaml(await readFile("codecov.yml", "utf8"));
  const cargo = parseToml(await readFile("Cargo.toml", "utf8"));

  assert.deepEqual(workflow.on, {
    pull_request: null,
    push: { branches: ["main"] },
    workflow_dispatch: null,
  });
  assert.deepEqual(workflow.permissions, { contents: "read" });
  assert.deepEqual(Object.keys(workflow.jobs), ["coverage"]);
  assert.doesNotMatch(
    workflowContents,
    /pull_request_target|CODECOV_TOKEN|secrets\./u,
  );

  const job = workflow.jobs.coverage;
  assert.equal(job["runs-on"], "ubuntu-24.04");
  assert.equal(job["timeout-minutes"], 30);
  assert.equal(job["continue-on-error"], undefined);
  assert.deepEqual(job.permissions, {
    contents: "read",
    "id-token": "write",
  });
  assert.deepEqual(job.steps[0], {
    uses: "actions/checkout@v7",
    with: { "persist-credentials": false },
  });
  assert.deepEqual(
    job.steps.find((step) => step.name === "Install cargo-llvm-cov"),
    {
      name: "Install cargo-llvm-cov",
      run: "cargo install cargo-llvm-cov --version 0.9.0 --locked",
    },
  );
  assert.deepEqual(
    job.steps.find((step) => step.name === "Validate Codecov configuration"),
    {
      name: "Validate Codecov configuration",
      run: "curl --fail-with-body --silent --show-error --proto '=https' --tlsv1.2 --data-binary @codecov.yml https://codecov.io/validate",
    },
  );
  const stepNames = job.steps.map((step) => step.name).filter(Boolean);
  assert.ok(
    stepNames.indexOf("Validate Codecov configuration") <
      stepNames.indexOf("Generate coverage"),
  );
  assert.ok(
    stepNames.indexOf("Generate coverage") <
      stepNames.indexOf("Upload coverage"),
  );
  const coverageCommand = job.steps.find(
    (step) => step.name === "Generate coverage",
  ).run;
  assert.equal(
    coverageCommand,
    "cargo llvm-cov --workspace --all-features --locked --lcov --output-path lcov.info",
  );
  const jobCommands = job.steps.map((step) => step.run ?? "").join("\n");
  assert.doesNotMatch(
    jobCommands,
    /--all-targets|--benches|--bench(?:\s|=)|cargo\s+bench|cargo\s+codspeed/u,
  );
  assert.deepEqual(
    job.steps.find((step) => step.name === "Upload coverage"),
    {
      name: "Upload coverage",
      uses: "codecov/codecov-action@v7",
      with: {
        disable_search: true,
        fail_ci_if_error: true,
        files: "lcov.info",
        use_oidc: true,
        version: "v11.3.1",
      },
    },
  );
  assert.deepEqual(config.codecov, {
    require_ci_to_pass: false,
    strict_yaml_branch: "main",
    notify: { wait_for_ci: false },
  });
  assert.equal(config.coverage.status.project.default.informational, true);
  assert.equal(config.coverage.status.patch.default.informational, true);
  assert.equal(config.comment.layout, "diff, files");
  assert.deepEqual(config.ignore, ["benches/**", "tests/**"]);
  assert.ok(cargo.package.exclude.includes("codecov.yml"));
});

test("CodSpeed runs the 10K-LOC analysis benchmarks", async () => {
  const workflowContents = await readFile(
    ".github/workflows/codspeed.yml",
    "utf8",
  );
  const workflow = parseYaml(workflowContents);
  const ci = parseYaml(await readFile(".github/workflows/ci.yml", "utf8"));
  const cargo = parseToml(await readFile("Cargo.toml", "utf8"));

  assert.deepEqual(workflow.on, {
    pull_request: null,
    push: { branches: ["main"] },
    workflow_dispatch: null,
  });
  assert.deepEqual(workflow.permissions, { contents: "read" });
  assert.deepEqual(Object.keys(workflow.jobs), ["benchmarks"]);
  assert.doesNotMatch(
    workflowContents,
    /pull_request_target|CODSPEED_TOKEN|secrets\./u,
  );
  assert.deepEqual(cargo["dev-dependencies"].divan, {
    package: "codspeed-divan-compat",
    version: "=5.0.1",
  });
  assert.deepEqual(cargo.bench, [{ name: "analysis", harness: false }]);
  assert.equal(
    ci.jobs.rust.steps.find((step) => step.name === "Test").run,
    "cargo test --lib --bins --tests --all-features --locked",
  );
  assert.equal(
    ci.jobs.msrv.steps.find((step) => step.name === "Test MSRV").run,
    "cargo +1.88.0 test --lib --bins --tests --all-features --locked",
  );
  assert.equal(ci.jobs["ci-gate"].needs.includes("benchmarks"), false);

  const job = workflow.jobs.benchmarks;
  assert.equal(job["runs-on"], "ubuntu-24.04");
  assert.equal(job["timeout-minutes"], 30);
  assert.equal(job["continue-on-error"], undefined);
  assert.deepEqual(job.permissions, {
    contents: "read",
    "id-token": "write",
  });
  assert.deepEqual(job.steps[0], {
    uses: "actions/checkout@v7",
    with: { "persist-credentials": false },
  });
  assert.deepEqual(
    job.steps.find((step) => step.name === "Install cargo-codspeed"),
    {
      name: "Install cargo-codspeed",
      run: "cargo install cargo-codspeed --version 5.0.1 --locked",
    },
  );
  assert.deepEqual(
    job.steps.find((step) => step.name === "Build benchmarks"),
    {
      name: "Build benchmarks",
      run: "cargo codspeed build --locked -m simulation -m memory --bench analysis",
    },
  );
  assert.deepEqual(
    job.steps.find((step) => step.name === "Run benchmarks"),
    {
      name: "Run benchmarks",
      uses: "CodSpeedHQ/action@v5",
      with: {
        mode: "simulation,memory",
        run: "cargo codspeed run --bench analysis",
      },
    },
  );
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
