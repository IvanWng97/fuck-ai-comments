import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { parse as parseToml } from "smol-toml";
import { parse as parseYaml } from "yaml";

import { hardenReleaseWorkflow } from "../scripts/harden-release-workflow.js";

const workflowPath = ".github/workflows/update-major-tag.yml";

test("cargo-dist owns the post-release compatibility tag job", async () => {
  const config = parseToml(await readFile("dist-workspace.toml", "utf8"));
  const release = parseYaml(
    await readFile(".github/workflows/release.yml", "utf8"),
  );

  assert.deepEqual(config.dist["post-announce-jobs"], ["./update-major-tag"]);
  assert.deepEqual(config.dist["github-custom-job-permissions"], {
    "update-major-tag": { contents: "write" },
  });

  const generatedJob = release.jobs["custom-update-major-tag"];
  assert.deepEqual(generatedJob.needs, ["plan", "announce"]);
  assert.equal(generatedJob.uses, "./.github/workflows/update-major-tag.yml");
  assert.equal(generatedJob.with.plan, "${{ needs.plan.outputs.val }}");
  assert.deepEqual(generatedJob.permissions, { contents: "write" });
  assert.equal("secrets" in generatedJob, false);
});

test("release jobs only receive the repository access they need", async () => {
  const release = parseYaml(
    await readFile(".github/workflows/release.yml", "utf8"),
  );

  assert.deepEqual(release.permissions, { contents: "read" });
  assert.deepEqual(release.jobs.host.permissions, { contents: "write" });

  const writers = Object.entries(release.jobs)
    .filter(([, job]) => job.permissions?.contents === "write")
    .map(([name]) => name);
  assert.deepEqual(writers, ["host", "custom-update-major-tag"]);
});

test("hardening is a narrow transform over cargo-dist output", () => {
  const generated = `name: Release
permissions:
  "contents": "write"
jobs:
  plan:
    steps: []
  host:
    outputs:
      val: output
    steps:
      - run: true
  announce:
    steps: []
  custom-update-major-tag:
    uses: ./.github/workflows/update-major-tag.yml
    secrets: inherit
    permissions:
      "contents": "write"
`;

  assert.equal(
    hardenReleaseWorkflow(generated),
    `name: Release
permissions:
  "contents": "read"
jobs:
  plan:
    steps: []
  host:
    outputs:
      val: output
    permissions:
      "contents": "write"
    steps:
      - run: true
  announce:
    steps: []
  custom-update-major-tag:
    uses: ./.github/workflows/update-major-tag.yml
    permissions:
      "contents": "write"
`,
  );
});

test("compatibility tag workflow derives one major alias after stable releases", async () => {
  const source = await readFile(workflowPath, "utf8");
  const workflow = parseYaml(source);

  assert.deepEqual(workflow.permissions, {});
  assert.deepEqual(workflow.on.workflow_call.inputs.plan, {
    required: true,
    type: "string",
  });

  const jobs = Object.values(workflow.jobs);
  assert.equal(jobs.length, 1);
  const job = jobs[0];
  assert.equal(
    job.if,
    "${{ !fromJSON(inputs.plan).announcement_is_prerelease }}",
  );
  assert.deepEqual(job.permissions, { contents: "write" });
  assert.equal(job.env.GH_TOKEN, "${{ github.token }}");
  assert.equal(job.env.PLAN, "${{ inputs.plan }}");
  assert.equal(job.env.RELEASE_COMMIT, "${{ github.sha }}");
  assert.equal(
    job.steps.some((step) => step.uses),
    false,
  );

  const script = job.steps.map((step) => step.run ?? "").join("\n");
  assert.match(script, /releases\[0\]\.app_version/u);
  assert.match(script, /refs\/tags\/v\$\{major\}/u);
  assert.match(script, /RELEASE_COMMIT/u);
  assert.doesNotMatch(script, /refs\/tags\/v\d/u);
});
