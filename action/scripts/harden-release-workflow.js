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
  let hardened = replaceOnce(
    generated,
    'permissions:\n  "contents": "write"\n',
    'permissions:\n  "contents": "read"\n',
    "workflow contents permission",
  );

  hardened = transformJob(hardened, "host", "announce", (host) =>
    replaceOnce(
      host,
      "    steps:\n",
      '    permissions:\n      "contents": "write"\n    steps:\n',
      "host steps block",
    ),
  );
  hardened = transformJob(
    hardened,
    "custom-update-major-tag",
    undefined,
    (job) =>
      replaceOnce(job, "    secrets: inherit\n", "", "inherited secrets line"),
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
