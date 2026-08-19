import { spawnSync } from "node:child_process";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, relative, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const releasePath = ".github/workflows/release.yml";
const configPath = "dist-workspace.toml";
const dirtyCiSetting = 'allow-dirty = ["ci"]\n';
const copyExcludes = new Set([".git", "node_modules", "target"]);

function replaceOnce(source, before, after, label) {
  const first = source.indexOf(before);
  if (first === -1 || source.indexOf(before, first + before.length) !== -1) {
    throw new Error(`cargo-dist output must contain exactly one ${label}`);
  }
  return source.slice(0, first) + after + source.slice(first + before.length);
}

function transformJob(source, jobName, nextJobName, transform) {
  const startMarker = `\n  ${jobName}:\n`;
  const start = source.indexOf(startMarker);
  if (start === -1) {
    throw new Error(`cargo-dist output is missing the ${jobName} job`);
  }

  const end = nextJobName
    ? source.indexOf(`\n  ${nextJobName}:\n`, start + startMarker.length)
    : source.length;
  if (end === -1) {
    throw new Error(`cargo-dist output is missing the ${nextJobName} job`);
  }

  return (
    source.slice(0, start) +
    transform(source.slice(start, end)) +
    source.slice(end)
  );
}

export function hardenReleaseWorkflow(generated) {
  let hardened = generated;

  hardened = replaceOnce(
    hardened,
    "      tag-flag: ${{ inputs.tag && inputs.tag != 'dry-run' && format('--tag={0}', inputs.tag) || '' }}\n",
    "",
    "composite tag flag output",
  );
  hardened = replaceOnce(
    hardened,
    "      - id: plan\n        run: |\n          dist ${{ (inputs.tag && inputs.tag != 'dry-run' && format('host --steps=create --tag={0}', inputs.tag)) || 'plan' }} --output-format=json > plan-dist-manifest.json\n",
    "      - id: plan\n        shell: bash\n        env:\n          RELEASE_TAG: ${{ (inputs.tag != 'dry-run' && inputs.tag) || '' }}\n        run: |\n          if [[ -n \"$RELEASE_TAG\" ]]; then\n            dist host --steps=create --tag \"$RELEASE_TAG\" --output-format=json > plan-dist-manifest.json\n          else\n            dist plan --output-format=json > plan-dist-manifest.json\n          fi\n",
    "plan tag command",
  );
  hardened = replaceOnce(
    hardened,
    "        type: string\n\njobs:\n",
    "        type: string\n\nconcurrency:\n  group: ${{ github.workflow }}-${{ inputs.tag || github.ref }}\n  cancel-in-progress: false\n\njobs:\n",
    "per-release concurrency",
  );
  hardened = replaceOnce(
    hardened,
    "# If you push multiple tags at once, separate instances of this workflow will\n# spin up, creating an independent announcement for each one. However, GitHub\n# will hard limit this to 3 tags per commit, as it will assume more tags is a\n# mistake.\n#\n",
    "",
    "tag-push-only guidance",
  );
  hardened = replaceOnce(
    hardened,
    "      - name: Build artifacts\n        run: |\n          # Actually do builds and make zips and whatnot\n          dist build ${{ needs.plan.outputs.tag-flag }} --print=linkage --output-format=json ${{ matrix.dist_args }} > dist-manifest.json\n",
    '      - name: Build artifacts\n        shell: bash\n        env:\n          DIST_ARGS: ${{ matrix.dist_args }}\n          RELEASE_TAG: ${{ needs.plan.outputs.tag }}\n        run: |\n          # Actually do builds and make zips and whatnot\n          tag_args=()\n          if [[ -n "$RELEASE_TAG" ]]; then\n            tag_args=(--tag "$RELEASE_TAG")\n          fi\n          dist_args=()\n          if [[ -n "$DIST_ARGS" ]]; then\n            read -r -a dist_args <<<"$DIST_ARGS"\n          fi\n          dist build "${tag_args[@]}" --print=linkage --output-format=json "${dist_args[@]}" > dist-manifest.json\n',
    "local build tag command",
  );
  hardened = replaceOnce(
    hardened,
    "    container: ${{ matrix.container && matrix.container.image || null }}\n",
    "    container: ${{ matrix.container && matrix.container.image || null }} # zizmor: ignore[unpinned-images] cargo-dist selects this image from an authorized plan.\n",
    "cargo-dist container",
  );
  hardened = replaceOnce(
    hardened,
    "      - name: Install dist\n        run: ${{ matrix.install_dist.run }}\n",
    "      - name: Install dist\n        run: ${{ matrix.install_dist.run }} # zizmor: ignore[template-injection] cargo-dist supplies this command from an authorized plan.\n",
    "cargo-dist installer command",
  );
  hardened = replaceOnce(
    hardened,
    "      - name: Install dependencies\n        run: |\n          ${{ matrix.packages_install }}\n",
    "      - name: Install dependencies\n        run: | # zizmor: ignore[template-injection] cargo-dist supplies this command from an authorized plan.\n          ${{ matrix.packages_install }}\n",
    "cargo-dist dependency command",
  );
  hardened = replaceOnce(
    hardened,
    '            echo "$HOME/.cargo/bin" >> $GITHUB_PATH\n',
    '            echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"\n',
    "GitHub path output",
  );
  hardened = replaceOnce(
    hardened,
    '          echo "paths<<EOF" >> "$GITHUB_OUTPUT"\n          dist print-upload-files-from-manifest --manifest dist-manifest.json >> "$GITHUB_OUTPUT"\n          echo "EOF" >> "$GITHUB_OUTPUT"\n',
    '          {\n            echo "paths<<EOF"\n            dist print-upload-files-from-manifest --manifest dist-manifest.json\n            echo "EOF"\n          } >> "$GITHUB_OUTPUT"\n',
    "local artifact output",
  );
  hardened = transformJob(
    hardened,
    "build-local-artifacts",
    "build-global-artifacts",
    (job) =>
      replaceOnce(
        job,
        "    needs:\n      - plan\n    if: ${{ fromJson(needs.plan.outputs.val).ci.github.artifacts_matrix.include != null && (needs.plan.outputs.publishing == 'true' || fromJson(needs.plan.outputs.val).ci.github.pr_run_mode == 'upload') || inputs.tag == 'dry-run' }}\n",
        "    needs:\n      - plan\n      - custom-authorize-release\n    if: ${{ always() && fromJson(needs.plan.outputs.val).ci.github.artifacts_matrix.include != null && ((needs.plan.outputs.publishing == 'true' && needs.custom-authorize-release.result == 'success') || fromJson(needs.plan.outputs.val).ci.github.pr_run_mode == 'upload' || inputs.tag == 'dry-run') }}\n",
        "local build authorization dependency",
      ),
  );
  hardened = replaceOnce(
    hardened,
    '      - id: cargo-dist\n        shell: bash\n        run: |\n          dist build ${{ needs.plan.outputs.tag-flag }} --output-format=json "--artifacts=global" > dist-manifest.json\n',
    '      - id: cargo-dist\n        shell: bash\n        env:\n          RELEASE_TAG: ${{ needs.plan.outputs.tag }}\n        run: |\n          tag_args=()\n          if [[ -n "$RELEASE_TAG" ]]; then\n            tag_args=(--tag "$RELEASE_TAG")\n          fi\n          dist build "${tag_args[@]}" --output-format=json "--artifacts=global" > dist-manifest.json\n',
    "global build tag command",
  );
  hardened = replaceOnce(
    hardened,
    '          echo "paths<<EOF" >> "$GITHUB_OUTPUT"\n          jq --raw-output ".upload_files[]" dist-manifest.json >> "$GITHUB_OUTPUT"\n          echo "EOF" >> "$GITHUB_OUTPUT"\n',
    '          {\n            echo "paths<<EOF"\n            jq --raw-output ".upload_files[]" dist-manifest.json\n            echo "EOF"\n          } >> "$GITHUB_OUTPUT"\n',
    "global artifact output",
  );
  hardened = replaceOnce(
    hardened,
    "      - id: host\n        shell: bash\n        run: |\n          dist host ${{ needs.plan.outputs.tag-flag }} --steps=upload --steps=release --output-format=json > dist-manifest.json\n",
    '      - id: host\n        shell: bash\n        env:\n          RELEASE_TAG: ${{ needs.plan.outputs.tag }}\n        run: |\n          dist host --tag "$RELEASE_TAG" --steps=upload --steps=release --output-format=json > dist-manifest.json\n',
    "host tag command",
  );
  hardened = replaceOnce(
    hardened,
    "      - name: Create GitHub Release\n        env:\n",
    '      - name: Create GitHub Release\n        shell: bash\n        env:\n          RELEASE_TAG: "${{ needs.plan.outputs.tag }}"\n',
    "GitHub release environment",
  );
  hardened = replaceOnce(
    hardened,
    '          echo "$ANNOUNCEMENT_BODY" > $RUNNER_TEMP/notes.txt\n\n          gh release create "${{ needs.plan.outputs.tag }}" --target "$RELEASE_COMMIT" $PRERELEASE_FLAG --title "$ANNOUNCEMENT_TITLE" --notes-file "$RUNNER_TEMP/notes.txt" artifacts/*\n',
    '          set -eo pipefail\n\n          tag_count=$(gh api \\\n            --method GET \\\n            "repos/${GITHUB_REPOSITORY}/git/matching-refs/tags/${RELEASE_TAG}" |\n            jq -er --arg ref "refs/tags/${RELEASE_TAG}" \'\n              if type == "array" then\n                map(select(.ref == $ref)) | length\n              else\n                error("matching refs response must be an array")\n              end\n            \')\n          if [[ $tag_count != 0 ]]; then\n            echo "release tag already exists: ${RELEASE_TAG}" >&2\n            exit 1\n          fi\n\n          release_count=$(gh api \\\n            --paginate \\\n            --slurp \\\n            --method GET \\\n            "repos/${GITHUB_REPOSITORY}/releases" \\\n            -F per_page=100 |\n            jq -er --arg tag "$RELEASE_TAG" \'\n              if type == "array" and all(.[]; type == "array") then\n                [.[][] | select(.tag_name == $tag)] | length\n              else\n                error("releases response must contain arrays of releases")\n              end\n            \')\n          if [[ $release_count != 0 ]]; then\n            echo "release already exists: ${RELEASE_TAG}" >&2\n            exit 1\n          fi\n\n          echo "$ANNOUNCEMENT_BODY" > "$RUNNER_TEMP/notes.txt"\n\n          prerelease_args=()\n          if [[ $PRERELEASE_FLAG == "--prerelease" ]]; then\n            prerelease_args=(--prerelease)\n          fi\n          gh release create "$RELEASE_TAG" --target "$RELEASE_COMMIT" "${prerelease_args[@]}" --title "$ANNOUNCEMENT_TITLE" --notes-file "$RUNNER_TEMP/notes.txt" artifacts/*\n',
    "GitHub release tag argument",
  );
  hardened = replaceOnce(
    hardened,
    'permissions:\n  "contents": "write"\n',
    'permissions:\n  "contents": "read"\n',
    "workflow contents permission",
  );

  hardened = transformJob(hardened, "host", "announce", (host) => {
    let next = replaceOnce(
      host,
      "    steps:\n",
      '    permissions:\n      "contents": "write"\n    steps:\n',
      "host steps block",
    );
    next = replaceOnce(
      next,
      "(needs.custom-authorize-release.result == 'skipped' || needs.custom-authorize-release.result == 'success')",
      "needs.custom-authorize-release.result == 'success'",
      "host authorization result",
    );
    return next;
  });
  hardened = transformJob(
    hardened,
    "custom-authorize-release",
    "host",
    (job) => {
      let next = replaceOnce(
        job,
        "    secrets: inherit\n",
        "",
        "inherited secrets line",
      );
      next = replaceOnce(
        next,
        "    needs:\n      - plan\n      - build-local-artifacts\n",
        "    needs:\n      - plan\n    if: ${{ needs.plan.outputs.publishing == 'true' }}\n",
        "authorization dependencies",
      );
      next = replaceOnce(
        next,
        "    with:\n      plan: ${{ needs.plan.outputs.val }}\n",
        "    with:\n      plan: ${{ needs.plan.outputs.val }}\n      requested_tag: ${{ needs.plan.outputs.tag }}\n",
        "authorization requested tag input",
      );
      return next;
    },
  );
  hardened = transformJob(
    hardened,
    "custom-action-e2e",
    "custom-update-major-tag",
    (job) =>
      replaceOnce(job, "    secrets: inherit\n", "", "inherited secrets line"),
  );
  hardened = transformJob(
    hardened,
    "custom-update-major-tag",
    undefined,
    (job) => {
      let next = replaceOnce(
        job,
        "    secrets: inherit\n",
        "",
        "inherited secrets line",
      );
      next = replaceOnce(
        next,
        "    needs:\n      - plan\n      - announce\n",
        "    needs:\n      - plan\n      - custom-action-e2e\n",
        "compatibility tag E2E dependency",
      );
      return next;
    },
  );

  if (hardened.includes("secrets: inherit")) {
    throw new Error(
      "cargo-dist output contains another inherited secrets grant",
    );
  }
  return hardened;
}

async function copyWorkingTree(destination) {
  await cp(repoRoot, destination, {
    recursive: true,
    filter(source) {
      const path = relative(repoRoot, source);
      const topLevel = path.split(sep)[0];
      return path === "" || !copyExcludes.has(topLevel);
    },
  });
}

async function generateHardenedWorkflow() {
  const temporaryRoot = await mkdtemp(join(tmpdir(), "fuck-ai-comments-dist-"));
  const temporaryRepo = join(temporaryRoot, "repo");
  try {
    await copyWorkingTree(temporaryRepo);
    const copiedConfig = join(temporaryRepo, configPath);
    const config = await readFile(copiedConfig, "utf8");
    await writeFile(
      copiedConfig,
      replaceOnce(config, dirtyCiSetting, "", "allow-dirty CI setting"),
    );

    const dist = process.env.CARGO_DIST_BIN ?? "dist";
    const result = spawnSync(dist, ["generate"], {
      cwd: temporaryRepo,
      encoding: "utf8",
    });
    if (result.error) {
      throw result.error;
    }
    if (result.status !== 0) {
      throw new Error(
        result.stderr || result.stdout || "cargo-dist generate failed",
      );
    }

    const generated = await readFile(join(temporaryRepo, releasePath), "utf8");
    return hardenReleaseWorkflow(generated);
  } finally {
    await rm(temporaryRoot, { force: true, recursive: true });
  }
}

async function main() {
  const mode = process.argv[2];
  if (mode !== "--check" && mode !== "--write") {
    throw new Error("usage: harden-release-workflow.js --check|--write");
  }

  const expected = await generateHardenedWorkflow();
  const committedPath = join(repoRoot, releasePath);
  if (mode === "--write") {
    await writeFile(committedPath, expected);
    return;
  }

  const committed = await readFile(committedPath, "utf8");
  if (committed !== expected) {
    throw new Error(
      "release.yml differs from hardened cargo-dist output; run npm run release:generate",
    );
  }
}

const entry = process.argv[1];
if (entry && import.meta.url === pathToFileURL(entry).href) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : error}\n`);
    process.exitCode = 1;
  });
}
