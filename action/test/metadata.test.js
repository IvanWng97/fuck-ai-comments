import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

import { parse as parseToml } from "smol-toml";
import { parse as parseYaml } from "yaml";

const ACTION_PINS = {
  "CodSpeedHQ/action": {
    sha: "4296e51e7041e24dadb86d1d6e8b9320d223dbe8",
    version: "v5",
  },
  "actions/attest": {
    sha: "1e69f48acb82d1966a394da916b4c1698aa569d6",
    version: "v4",
  },
  "actions/checkout": {
    sha: "3d3c42e5aac5ba805825da76410c181273ba90b1",
    version: "v7",
  },
  "actions/download-artifact": {
    sha: "3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
    version: "v8",
  },
  "actions/setup-node": {
    sha: "820762786026740c76f36085b0efc47a31fe5020",
    version: "v7",
  },
  "actions/upload-artifact": {
    sha: "043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
    version: "v7",
  },
  "codecov/codecov-action": {
    sha: "fb8b3582c8e4def4969c97caa2f19720cb33a72f",
    version: "v7",
  },
};

function pinnedAction(name) {
  return `${name}@${ACTION_PINS[name].sha}`;
}

async function workflowPaths(directory) {
  const paths = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      paths.push(...(await workflowPaths(path)));
    } else if (/\.ya?ml$/u.test(entry.name)) {
      paths.push(path);
    }
  }
  return paths;
}

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

test("workflow actions use reviewed commits with version comments", async () => {
  const workflowDirectory = ".github/workflows";
  const workflows = await workflowPaths(workflowDirectory);
  const seen = new Set();
  for (const workflow of workflows) {
    const contents = await readFile(workflow, "utf8");
    const references = [
      ...contents.matchAll(
        /^\s*(?:-\s*)?uses:\s+([^\s#]+)(?:\s+#\s+(v\d+))?\s*$/gmu,
      ),
    ];
    for (const [, reference, version] of references) {
      if (reference.startsWith("./")) {
        continue;
      }
      const match = /^([^@]+)@([a-f\d]{40})$/u.exec(reference);
      assert.ok(match, `${workflow}: ${reference} is not commit-pinned`);
      const [, action, sha] = match;
      assert.deepEqual(
        { sha, version },
        ACTION_PINS[action],
        `${workflow}: unexpected ${action} pin`,
      );
      seen.add(action);
    }
  }
  assert.deepEqual(seen, new Set(Object.keys(ACTION_PINS)));

  const config = parseToml(await readFile("dist-workspace.toml", "utf8"));
  assert.deepEqual(config.dist["github-action-commits"], {
    "actions/attest": `${ACTION_PINS["actions/attest"].sha} # v4`,
    "actions/checkout": `${ACTION_PINS["actions/checkout"].sha} # v7`,
    "actions/download-artifact": `${ACTION_PINS["actions/download-artifact"].sha} # v8`,
    "actions/upload-artifact": `${ACTION_PINS["actions/upload-artifact"].sha} # v7`,
  });
});

test("cargo-dist installer version follows the dist configuration", async () => {
  const config = parseToml(await readFile("dist-workspace.toml", "utf8"));
  const version = config.dist["cargo-dist-version"];
  const officialInstaller = `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/axodotdev/cargo-dist/releases/download/v${version}/cargo-dist-installer.sh | sh`;
  for (const workflowPath of [
    ".github/workflows/ci.yml",
    ".github/workflows/release.yml",
  ]) {
    const workflow = parseYaml(await readFile(workflowPath, "utf8"));
    const steps = Object.values(workflow.jobs).flatMap(
      (job) => job.steps ?? [],
    );
    const installers = steps.filter(
      (step) =>
        step.name === "Install dist" &&
        step.run.includes("cargo-dist/releases/download"),
    );
    assert.ok(installers.length > 0, `${workflowPath} has no dist installer`);
    for (const installer of installers) {
      assert.equal(installer.run, officialInstaller);
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
    uses: pinnedAction("actions/checkout"),
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
      uses: pinnedAction("codecov/codecov-action"),
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
    uses: pinnedAction("actions/checkout"),
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
      uses: pinnedAction("CodSpeedHQ/action"),
      with: {
        mode: "simulation,memory",
        run: "cargo codspeed run --bench analysis",
      },
    },
  );
});

test("required dogfood exercises the optimized release profile", async () => {
  const ci = parseYaml(await readFile(".github/workflows/ci.yml", "utf8"));
  const dogfood = ci.jobs.rust.steps.find(
    (step) => step.name === "Dogfood required comment policy",
  );

  assert.deepEqual(dogfood, {
    name: "Dogfood required comment policy",
    if: "runner.os == 'Linux'",
    run: "cargo run --release --locked -- check --all .",
  });
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
