import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import test from "node:test";

import { parse as parseToml } from "smol-toml";
import { parse as parseYaml } from "yaml";

const workflowPath = ".github/workflows/update-major-tag.yml";
const authorizationWorkflowPath = ".github/workflows/authorize-release.yml";
const actionE2eWorkflowPath = ".github/workflows/action-e2e.yml";
const localCommandPath = `.${delimiter}${process.env.PATH ?? ""}`;

test("cargo-dist dispatches releases from main without a tag-push trigger", async () => {
  const config = parseToml(await readFile("dist-workspace.toml", "utf8"));
  const source = await readFile(".github/workflows/release.yml", "utf8");
  const release = parseYaml(source);

  assert.equal(config.dist["dispatch-releases"], true);
  assert.deepEqual(release.on, {
    pull_request: null,
    workflow_dispatch: {
      inputs: {
        tag: {
          default: "dry-run",
          description: "Release Tag",
          required: true,
          type: "string",
        },
      },
    },
  });
  assert.deepEqual(release.concurrency, {
    "cancel-in-progress": false,
    group: "${{ github.workflow }}-${{ inputs.tag || github.ref }}",
  });
  assert.doesNotMatch(source, /^\s*push:\s*$/gmu);
  assert.doesNotMatch(source, /push(?:ing)?[^\n]*tags?/iu);
});

test("release runbook requires trusted main and an all-tag ruleset", async () => {
  const cargo = parseToml(await readFile("Cargo.toml", "utf8"));
  const readme = await readFile("README.md", "utf8");

  assert.match(
    readme,
    new RegExp(
      `gh workflow run release\\.yml --ref main -f tag=v${cargo.package.version.replaceAll(".", "\\.")}`,
      "u",
    ),
  );
  assert.match(readme, /all tag\s+refs\s+\(`~ALL`\)/u);
  assert.match(
    readme,
    /requires a successful\s+`release-authorization`\s+deployment/u,
  );
  assert.match(readme, /restricts deletion/u);
  assert.match(readme, /blocks force pushes and non-fast-forward\s+updates/u);
  assert.match(
    readme,
    /`release-authorization` environment only permits\s+protected\s+branches/u,
  );
  assert.match(readme, /no owner or administrator bypass/u);
  assert.doesNotMatch(readme, /GitHub Actions App ID `15368`/u);
  assert.match(readme, /protect `main`/u);
  assert.doesNotMatch(readme, /push(?:ing)? (?:a |the )?tag/u);
});

async function runAuthorization({
  plan,
  requestedTag = "v0.1.0-rc.1",
  eventName = "workflow_dispatch",
  eventRef = "refs/heads/main",
  releaseCommit = "1111111111111111111111111111111111111111",
  defaultHead = releaseCommit,
  tagRefs = [],
  releases = [],
  workflowRuns = [
    {
      conclusion: "success",
      event: "push",
      head_sha: releaseCommit,
      status: "completed",
    },
  ],
}) {
  const workflow = parseYaml(await readFile(authorizationWorkflowPath, "utf8"));
  const job = Object.values(workflow.jobs)[0];
  const script = job.steps.map((step) => step.run ?? "").join("\n");
  const temporary = await mkdtemp(join(tmpdir(), "authorize-release-"));
  try {
    const fakeGh = join(temporary, "gh");
    await writeFile(
      fakeGh,
      `#!/bin/sh
case "$*" in
  *git/ref/heads/*) printf '%s\\n' "$FAKE_DEFAULT_HEAD" ;;
  *actions/workflows/ci.yml/runs*) printf '[%s]\\n' "$FAKE_WORKFLOW_RUNS" ;;
  *git/matching-refs/tags/*) printf '%s\\n' "$FAKE_TAG_REFS" ;;
  *repos/*/releases*) printf '[%s]\\n' "$FAKE_RELEASES" ;;
  *) printf 'unexpected gh invocation: %s\\n' "$*" >&2; exit 64 ;;
esac
`,
    );
    await chmod(fakeGh, 0o755);
    return spawnSync("bash", ["-c", script], {
      cwd: temporary,
      encoding: "utf8",
      env: {
        ...process.env,
        DEFAULT_BRANCH: "main",
        EVENT_NAME: eventName,
        EVENT_REF: eventRef,
        FAKE_DEFAULT_HEAD: defaultHead,
        FAKE_RELEASES: JSON.stringify(releases),
        FAKE_TAG_REFS: JSON.stringify(tagRefs),
        FAKE_WORKFLOW_RUNS: JSON.stringify({ workflow_runs: workflowRuns }),
        GH_TOKEN: "test-token",
        GITHUB_REPOSITORY: "owner/repository",
        PATH: localCommandPath,
        PLAN: JSON.stringify(plan),
        RELEASE_COMMIT: releaseCommit,
        REQUESTED_TAG: requestedTag,
      },
    });
  } finally {
    await rm(temporary, { force: true, recursive: true });
  }
}

async function runMajorTagUpdate({
  currentCommit,
  releaseCommit,
  allowFastForward = false,
}) {
  const workflow = parseYaml(await readFile(workflowPath, "utf8"));
  const job = Object.values(workflow.jobs)[0];
  const script = job.steps.map((step) => step.run ?? "").join("\n");
  const temporary = await mkdtemp(join(tmpdir(), "update-major-tag-"));
  try {
    const state = join(temporary, "ref-state");
    const log = join(temporary, "gh-log");
    if (currentCommit) {
      await writeFile(state, `${currentCommit}\n`);
    }
    const fakeGh = join(temporary, "gh");
    await writeFile(
      fakeGh,
      `#!/bin/sh
printf '%s\\n' "$*" >> "$FAKE_GH_LOG"
case "$*" in
  *git/ref/tags/*)
    test -f "$FAKE_REF_STATE" || exit 1
    cat "$FAKE_REF_STATE"
    ;;
  *--method\\ PATCH*)
    next=
    force=
    for argument in "$@"; do
      case "$argument" in
        sha=*) next=\${argument#sha=} ;;
        force=*) force=\${argument#force=} ;;
      esac
    done
    test "$force" = false || exit 65
    test "$FAKE_ALLOW_FAST_FORWARD" = 1 || exit 66
    printf '%s\\n' "$next" > "$FAKE_REF_STATE"
    ;;
  *--method\\ POST*)
    next=
    for argument in "$@"; do
      case "$argument" in
        sha=*) next=\${argument#sha=} ;;
      esac
    done
    test ! -f "$FAKE_REF_STATE" || exit 67
    printf '%s\\n' "$next" > "$FAKE_REF_STATE"
    ;;
  *) printf 'unexpected gh invocation: %s\\n' "$*" >&2; exit 64 ;;
esac
`,
    );
    await chmod(fakeGh, 0o755);
    const result = spawnSync("bash", ["-c", script], {
      cwd: temporary,
      encoding: "utf8",
      env: {
        ...process.env,
        FAKE_ALLOW_FAST_FORWARD: allowFastForward ? "1" : "0",
        FAKE_GH_LOG: "gh-log",
        FAKE_REF_STATE: "ref-state",
        GH_TOKEN: "test-token",
        GITHUB_REPOSITORY: "owner/repository",
        PATH: localCommandPath,
        PLAN: JSON.stringify({
          announcement_is_prerelease: false,
          releases: [{ app_version: "0.1.0" }],
        }),
        RELEASE_COMMIT: releaseCommit,
      },
    });
    let nextCommit;
    try {
      nextCommit = (await readFile(state, "utf8")).trim();
    } catch (error) {
      if (error.code !== "ENOENT") {
        throw error;
      }
    }
    return {
      log: await readFile(log, "utf8"),
      nextCommit,
      result,
    };
  } finally {
    await rm(temporary, { force: true, recursive: true });
  }
}

test("release tags remain argv data at every shell sink", async () => {
  const release = parseYaml(
    await readFile(".github/workflows/release.yml", "utf8"),
  );
  const plan = release.jobs.plan;
  assert.equal(plan.outputs["tag-flag"], undefined);

  const tagConsumers = [
    plan.steps.find((step) => step.id === "plan"),
    release.jobs["build-local-artifacts"].steps.find(
      (step) => step.name === "Build artifacts",
    ),
    release.jobs["build-global-artifacts"].steps.find(
      (step) => step.id === "cargo-dist",
    ),
    release.jobs.host.steps.find((step) => step.id === "host"),
    release.jobs.host.steps.find(
      (step) => step.name === "Create GitHub Release",
    ),
  ];
  for (const step of tagConsumers) {
    assert.equal(step.shell, "bash");
    assert.match(step.env.RELEASE_TAG, /^\$\{\{ .+ \}\}$/u);
    assert.match(step.run, /"\$RELEASE_TAG"/u);
    assert.doesNotMatch(step.run, /\$\{\{[^}]*tag/u);
  }

  const temporary = await mkdtemp(join(tmpdir(), "release-tag-argv-"));
  try {
    const capture = join(temporary, "argv");
    const fakeDist = join(temporary, "dist");
    await writeFile(
      fakeDist,
      '#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE"\nprintf \'%s\\n\' \'{"releases":[]}\'\n',
    );
    await chmod(fakeDist, 0o755);

    const maliciousTag = "v0.1.0$(touch${IFS}PWNED)";
    const result = spawnSync("bash", ["-c", tagConsumers[0].run], {
      cwd: temporary,
      encoding: "utf8",
      env: {
        ...process.env,
        CAPTURE: "argv",
        GITHUB_OUTPUT: "output",
        PATH: localCommandPath,
        RELEASE_TAG: maliciousTag,
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual((await readFile(capture, "utf8")).trimEnd().split("\n"), [
      "host",
      "--steps=create",
      "--tag",
      maliciousTag,
      "--output-format=json",
    ]);
    await assert.rejects(readFile(join(temporary, "PWNED")), /ENOENT/u);
  } finally {
    await rm(temporary, { force: true, recursive: true });
  }
});

test("cargo-dist owns the post-release compatibility tag job", async () => {
  const config = parseToml(await readFile("dist-workspace.toml", "utf8"));
  const release = parseYaml(
    await readFile(".github/workflows/release.yml", "utf8"),
  );

  assert.deepEqual(config.dist["post-announce-jobs"], [
    "./action-e2e",
    "./update-major-tag",
  ]);
  assert.deepEqual(
    config.dist["github-custom-job-permissions"]["update-major-tag"],
    { contents: "write" },
  );

  const generatedJob = release.jobs["custom-update-major-tag"];
  assert.deepEqual(generatedJob.needs, ["plan", "custom-action-e2e"]);
  assert.equal(generatedJob.uses, "./.github/workflows/update-major-tag.yml");
  assert.equal(generatedJob.with.plan, "${{ needs.plan.outputs.val }}");
  assert.deepEqual(generatedJob.permissions, { contents: "write" });
  assert.equal("secrets" in generatedJob, false);
});

test("cargo-dist gates the compatibility tag on released Action E2E", async () => {
  const config = parseToml(await readFile("dist-workspace.toml", "utf8"));
  const readme = await readFile("README.md", "utf8");
  const release = parseYaml(
    await readFile(".github/workflows/release.yml", "utf8"),
  );
  assert.deepEqual(config.dist["github-custom-job-permissions"]["action-e2e"], {
    contents: "read",
  });

  const generatedJob = release.jobs["custom-action-e2e"];
  assert.deepEqual(generatedJob.needs, ["plan", "announce"]);
  assert.equal(generatedJob.uses, "./.github/workflows/action-e2e.yml");
  assert.equal(generatedJob.with.plan, "${{ needs.plan.outputs.val }}");
  assert.deepEqual(generatedJob.permissions, { contents: "read" });
  assert.equal("secrets" in generatedJob, false);

  const source = await readFile(actionE2eWorkflowPath, "utf8");
  const workflow = parseYaml(source);
  assert.deepEqual(workflow.permissions, {});
  assert.deepEqual(workflow.on.workflow_call.inputs.plan, {
    required: true,
    type: "string",
  });
  assert.deepEqual(Object.keys(workflow.jobs), ["action-e2e"]);

  const job = workflow.jobs["action-e2e"];
  assert.deepEqual(job.strategy, {
    "fail-fast": false,
    matrix: {
      os: ["macos-15", "macos-15-intel", "ubuntu-24.04", "windows-2025"],
    },
  });
  assert.equal(job["runs-on"], "${{ matrix.os }}");
  assert.deepEqual(job.permissions, { contents: "read" });
  assert.deepEqual(job.env, {
    E2E_ROOT: "${{ github.workspace }}/fuck-ai-comments-action-e2e",
  });

  const checkout = job.steps[0];
  assert.equal(
    checkout.uses,
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
  );
  assert.deepEqual(checkout.with, {
    "persist-credentials": false,
    ref: "refs/tags/${{ fromJSON(inputs.plan).announcement_tag }}",
  });

  const fixtures = job.steps[1];
  assert.equal(fixtures.name, "Prepare policy fixtures");
  assert.equal(fixtures.id, "fixtures");
  assert.equal(fixtures.shell, "bash");
  assert.match(fixtures.run, /git init --quiet "\$E2E_ROOT\/attestation"/u);
  assert.match(fixtures.run, /base=%s\\n.+GITHUB_OUTPUT/su);
  assert.doesNotMatch(fixtures.run, /cargo|npm|node|curl|wget|gh release/iu);

  const version = "${{ fromJSON(inputs.plan).releases[0].app_version }}";
  assert.deepEqual(job.steps.slice(2, 6), [
    {
      name: "Accept clean source",
      id: "clean",
      uses: "./",
      with: {
        version,
        mode: "all",
        profile: "full",
        path: "${{ env.E2E_ROOT }}/clean",
      },
    },
    {
      name: "Reject static violation",
      id: "static-violation",
      "continue-on-error": true,
      uses: "./",
      with: {
        version,
        mode: "all",
        profile: "full",
        path: "${{ env.E2E_ROOT }}/static-violation",
      },
    },
    {
      name: "Reject stale owner change",
      id: "stale-attestation",
      "continue-on-error": true,
      uses: "./",
      with: {
        version,
        mode: "base",
        profile: "attestation",
        base: "${{ steps.fixtures.outputs.base }}",
        path: "${{ env.E2E_ROOT }}/attestation",
      },
    },
    {
      name: "Reject parse error",
      id: "parse-error",
      "continue-on-error": true,
      uses: "./",
      with: {
        version,
        mode: "all",
        profile: "full",
        path: "${{ env.E2E_ROOT }}/parse-error",
      },
    },
  ]);
  assert.deepEqual(job.steps[6], {
    name: "Assert packaged Action outcomes",
    if: "${{ always() }}",
    shell: "bash",
    env: {
      CLEAN_OUTCOME: "${{ steps.clean.outcome }}",
      STATIC_VIOLATION_OUTCOME: "${{ steps.static-violation.outcome }}",
      STALE_ATTESTATION_OUTCOME: "${{ steps.stale-attestation.outcome }}",
      PARSE_ERROR_OUTCOME: "${{ steps.parse-error.outcome }}",
    },
    run: 'test "$CLEAN_OUTCOME" = success\ntest "$STATIC_VIOLATION_OUTCOME" = failure\ntest "$STALE_ATTESTATION_OUTCOME" = failure\ntest "$PARSE_ERROR_OUTCOME" = failure\n',
  });
  assert.equal(job.steps.length, 7);
  assert.match(
    readme,
    /packaged Action must then pass on x86-64 Linux and Windows, plus x86-64\s+and Apple Silicon macOS/u,
  );
  assert.doesNotMatch(
    source,
    /pull_request|secrets\.|contents:\s*write|gh release|dist host/u,
  );
});

test("stable and prerelease artifacts run E2E before stable v0 advances", async () => {
  const e2e = parseYaml(await readFile(actionE2eWorkflowPath, "utf8"));
  const updateMajor = parseYaml(await readFile(workflowPath, "utf8"));

  assert.equal("if" in e2e.jobs["action-e2e"], false);
  assert.equal(
    updateMajor.jobs["update-major-tag"].if,
    "${{ !fromJSON(inputs.plan).announcement_is_prerelease }}",
  );
});

test("cargo-dist gates hosting on the native release authorization job", async () => {
  const config = parseToml(await readFile("dist-workspace.toml", "utf8"));
  const release = parseYaml(
    await readFile(".github/workflows/release.yml", "utf8"),
  );

  assert.deepEqual(config.dist["global-artifacts-jobs"], [
    "./authorize-release",
  ]);
  assert.deepEqual(config.dist["github-custom-job-permissions"], {
    "action-e2e": { contents: "read" },
    "authorize-release": { actions: "read", contents: "read" },
    "update-major-tag": { contents: "write" },
  });

  const generatedJob = release.jobs["custom-authorize-release"];
  assert.deepEqual(generatedJob.needs, ["plan"]);
  assert.equal(
    generatedJob.if,
    "${{ needs.plan.outputs.publishing == 'true' }}",
  );
  assert.equal(generatedJob.uses, "./.github/workflows/authorize-release.yml");
  assert.equal(generatedJob.with.plan, "${{ needs.plan.outputs.val }}");
  assert.equal(
    generatedJob.with.requested_tag,
    "${{ needs.plan.outputs.tag }}",
  );
  assert.deepEqual(generatedJob.permissions, {
    actions: "read",
    contents: "read",
  });
  assert.equal("secrets" in generatedJob, false);
  assert.deepEqual(release.jobs["build-local-artifacts"].needs, [
    "plan",
    "custom-authorize-release",
  ]);
  assert.match(
    release.jobs["build-local-artifacts"].if,
    /publishing == 'true'.+custom-authorize-release\.result == 'success'/u,
  );
  assert.match(
    release.jobs["build-local-artifacts"].if,
    /inputs\.tag == 'dry-run'/u,
  );
  assert.ok(release.jobs.host.needs.includes("custom-authorize-release"));
  assert.equal(
    release.jobs.host.if.includes(
      "needs.custom-authorize-release.result == 'success'",
    ),
    true,
  );
  assert.doesNotMatch(
    release.jobs.host.if,
    /custom-authorize-release\.result == 'skipped'/u,
  );
  assert.equal(config.dist["pr-run-mode"], "plan");
  assert.match(
    release.jobs["build-local-artifacts"].if,
    /publishing == 'true'.+pr_run_mode == 'upload'/u,
  );
  assert.match(release.jobs.host.if, /publishing == 'true'/u);
});

test("release authorization accepts one unused canonical prerelease on green main", async () => {
  const plan = {
    announcement_tag: "v0.1.0-rc.1",
    releases: [{ app_version: "0.1.0-rc.1" }],
  };
  const result = await runAuthorization({ plan });
  assert.equal(result.status, 0, result.stderr);

  const workflow = parseYaml(await readFile(authorizationWorkflowPath, "utf8"));
  assert.deepEqual(workflow.permissions, {});
  assert.deepEqual(workflow.on.workflow_call.inputs, {
    plan: { required: true, type: "string" },
    requested_tag: { required: true, type: "string" },
  });
  const job = workflow.jobs["authorize-release"];
  assert.equal(job.environment, "release-authorization");
  assert.deepEqual(job.permissions, { actions: "read", contents: "read" });
  assert.equal(job.env.EVENT_NAME, "${{ github.event_name }}");
  assert.equal(job.env.EVENT_REF, "${{ github.ref }}");
  assert.equal(job.env.REQUESTED_TAG, "${{ inputs.requested_tag }}");
  assert.equal(job.env.RELEASE_COMMIT, "${{ github.sha }}");
  assert.equal(
    job.env.DEFAULT_BRANCH,
    "${{ github.event.repository.default_branch }}",
  );
  assert.equal(
    job.steps.some((step) => step.uses),
    false,
  );

  const script = job.steps.map((step) => step.run ?? "").join("\n");
  assert.match(script, /expected_ref="refs\/heads\/main"/u);
  assert.match(script, /expected_tag="v\$\{app_version\}"/u);
  assert.match(script, /actions\/workflows\/ci\.yml\/runs/u);
  assert.match(script, /git\/matching-refs\/tags/u);
  assert.match(script, /repos\/\$\{GITHUB_REPOSITORY\}\/releases/u);
  assert.doesNotMatch(script, /cargo\s+test|npm\s+test|semver|=~/u);
});

test("release authorization rejects untrusted, used, stale, or untested requests", async () => {
  const validPlan = {
    announcement_tag: "v0.1.0-rc.1",
    releases: [{ app_version: "0.1.0-rc.1" }],
  };
  const cases = [
    {
      name: "multiple plan releases",
      options: {
        plan: {
          ...validPlan,
          releases: [
            { app_version: "0.1.0-rc.1" },
            { app_version: "0.2.0-rc.1" },
          ],
        },
      },
    },
    {
      name: "not a workflow dispatch",
      options: { eventName: "push", plan: validPlan },
    },
    {
      name: "not dispatched from main",
      options: { eventRef: "refs/heads/release", plan: validPlan },
    },
    {
      name: "requested tag mismatch",
      options: { requestedTag: "v0.1.0-rc.2", plan: validPlan },
    },
    {
      name: "announcement tag mismatch",
      options: {
        plan: { ...validPlan, announcement_tag: "release-0.1.0-rc.1" },
      },
    },
    {
      name: "commit is not default branch head",
      options: {
        defaultHead: "2222222222222222222222222222222222222222",
        plan: validPlan,
      },
    },
    {
      name: "tag already exists",
      options: {
        plan: validPlan,
        tagRefs: [
          {
            object: { sha: "1111111111111111111111111111111111111111" },
            ref: "refs/tags/v0.1.0-rc.1",
          },
        ],
      },
    },
    {
      name: "release already exists",
      options: {
        plan: validPlan,
        releases: [{ tag_name: "v0.1.0-rc.1" }],
      },
    },
    {
      name: "CI push did not succeed",
      options: {
        plan: validPlan,
        workflowRuns: [
          {
            conclusion: "failure",
            event: "push",
            head_sha: "1111111111111111111111111111111111111111",
            status: "completed",
          },
        ],
      },
    },
    {
      name: "CI run belongs to another commit",
      options: {
        plan: validPlan,
        workflowRuns: [
          {
            conclusion: "success",
            event: "push",
            head_sha: "2222222222222222222222222222222222222222",
            status: "completed",
          },
        ],
      },
    },
    {
      name: "CI run was not triggered by a push",
      options: {
        plan: validPlan,
        workflowRuns: [
          {
            conclusion: "success",
            event: "pull_request",
            head_sha: "1111111111111111111111111111111111111111",
            status: "completed",
          },
        ],
      },
    },
    {
      name: "CI run is not complete",
      options: {
        plan: validPlan,
        workflowRuns: [
          {
            conclusion: "success",
            event: "push",
            head_sha: "1111111111111111111111111111111111111111",
            status: "in_progress",
          },
        ],
      },
    },
  ];

  for (const { name, options } of cases) {
    const result = await runAuthorization(options);
    assert.notEqual(result.status, 0, `${name} unexpectedly passed`);
  }
});

test("release authorization accepts any matching successful CI push run", async () => {
  const plan = {
    announcement_tag: "v0.1.0-rc.1",
    releases: [{ app_version: "0.1.0-rc.1" }],
  };
  const successful = {
    conclusion: "success",
    event: "push",
    head_sha: "1111111111111111111111111111111111111111",
    status: "completed",
  };
  for (const workflowRuns of [
    [{ ...successful, conclusion: "failure" }, successful],
    [successful, { ...successful }],
  ]) {
    const result = await runAuthorization({ plan, workflowRuns });
    assert.equal(result.status, 0, result.stderr);
  }
});

test("GitHub release creation atomically creates an unused tag at the release commit", async () => {
  const release = parseYaml(
    await readFile(".github/workflows/release.yml", "utf8"),
  );
  const step = release.jobs.host.steps.find(
    (candidate) => candidate.name === "Create GitHub Release",
  );
  assert.match(step.run, /git\/matching-refs\/tags/u);
  assert.match(step.run, /repos\/\$\{GITHUB_REPOSITORY\}\/releases/u);
  assert.match(
    step.run,
    /gh release create "\$RELEASE_TAG" --target "\$RELEASE_COMMIT"/u,
  );
  assert.doesNotMatch(step.run, /--verify-tag|git push|git tag/u);

  const temporary = await mkdtemp(join(tmpdir(), "create-release-"));
  try {
    const fakeGh = join(temporary, "gh");
    await writeFile(
      fakeGh,
      `#!/bin/sh
case "$*" in
  *git/matching-refs/tags/*) printf '[]\\n' ;;
  *repos/*/releases*) printf '[[]]\\n' ;;
  release\\ create*) printf '%s\\n' "$@" > "$FAKE_RELEASE_ARGV" ;;
  *) printf 'unexpected gh invocation: %s\\n' "$*" >&2; exit 64 ;;
esac
`,
    );
    await chmod(fakeGh, 0o755);

    const maliciousTag = "v0.1.0$(touch${IFS}PWNED)";
    const releaseCommit = "1111111111111111111111111111111111111111";
    const result = spawnSync("bash", ["-c", step.run], {
      cwd: temporary,
      encoding: "utf8",
      env: {
        ...process.env,
        ANNOUNCEMENT_BODY: "notes",
        ANNOUNCEMENT_TITLE: "title",
        FAKE_RELEASE_ARGV: "release-argv",
        GITHUB_REPOSITORY: "owner/repository",
        PATH: localCommandPath,
        PRERELEASE_FLAG: "",
        RELEASE_COMMIT: releaseCommit,
        RELEASE_TAG: maliciousTag,
        RUNNER_TEMP: ".",
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(
      (await readFile(join(temporary, "release-argv"), "utf8"))
        .trimEnd()
        .split("\n")
        .slice(0, 5),
      ["release", "create", maliciousTag, "--target", releaseCommit],
    );
    await assert.rejects(readFile(join(temporary, "PWNED")), /ENOENT/u);
  } finally {
    await rm(temporary, { force: true, recursive: true });
  }
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
  assert.ok(job.steps.every((step) => !step.uses));

  const script = job.steps.map((step) => step.run ?? "").join("\n");
  assert.match(script, /releases\[0\]\.app_version/u);
  assert.match(script, /refs\/tags\/v\$\{major\}/u);
  assert.match(script, /RELEASE_COMMIT/u);
  assert.doesNotMatch(script, /refs\/tags\/v\d/u);
});

test("compatibility tag creation and reruns are idempotent", async () => {
  const releaseCommit = "1111111111111111111111111111111111111111";
  const created = await runMajorTagUpdate({ releaseCommit });
  assert.equal(created.result.status, 0, created.result.stderr);
  assert.equal(created.nextCommit, releaseCommit);
  assert.match(created.log, /--method POST/u);

  const rerun = await runMajorTagUpdate({
    currentCommit: releaseCommit,
    releaseCommit,
  });
  assert.equal(rerun.result.status, 0, rerun.result.stderr);
  assert.equal(rerun.nextCommit, releaseCommit);
  assert.doesNotMatch(rerun.log, /--method (?:PATCH|POST)/u);
});

test("a newer stable release fast-forwards the compatibility tag", async () => {
  const olderCommit = "1111111111111111111111111111111111111111";
  const newerCommit = "2222222222222222222222222222222222222222";
  const update = await runMajorTagUpdate({
    allowFastForward: true,
    currentCommit: olderCommit,
    releaseCommit: newerCommit,
  });

  assert.equal(update.result.status, 0, update.result.stderr);
  assert.equal(update.nextCommit, newerCommit);
  assert.match(update.log, /--method PATCH/u);
  assert.match(update.log, /force=false/u);
});

test("an out-of-order v0 release cannot rewind the compatibility tag", async () => {
  const newerCommit = "2222222222222222222222222222222222222222";
  const olderCommit = "1111111111111111111111111111111111111111";
  const update = await runMajorTagUpdate({
    currentCommit: newerCommit,
    releaseCommit: olderCommit,
  });

  assert.notEqual(update.result.status, 0);
  assert.equal(update.nextCommit, newerCommit);
  assert.match(update.log, /--method PATCH/u);
  assert.match(update.log, /force=false/u);
  assert.doesNotMatch(update.log, /force=true/u);
});
