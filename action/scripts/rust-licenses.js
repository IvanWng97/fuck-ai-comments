import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const cargoAboutVersion = "cargo-about 0.9.1";
const noticePath = "THIRD_PARTY_LICENSES";
const unresolvedSource = "UNRESOLVED-LICENSE-SOURCE";

async function runCommand({ command, args, cwd }) {
  const result = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(result.stderr || result.stdout || `${command} failed`);
  }
  return result.stdout;
}

export async function verifyRustLicenses({
  mode,
  root = repoRoot,
  runCargoAbout = runCommand,
  cargoAbout = process.env.CARGO_ABOUT_BIN ?? "cargo-about",
}) {
  if (mode !== "check" && mode !== "write") {
    throw new Error("license mode must be check or write");
  }

  const version = await runCargoAbout({
    command: cargoAbout,
    args: ["--version"],
    cwd: root,
  });
  if (version.trim() !== cargoAboutVersion) {
    throw new Error(`expected ${cargoAboutVersion}, got ${version.trim()}`);
  }

  const temporaryRoot = await mkdtemp(
    join(tmpdir(), "fuck-ai-comments-licenses-"),
  );
  const auditPath = join(temporaryRoot, "licenses.json");
  const generatedPath = join(temporaryRoot, noticePath);
  try {
    await runCargoAbout({
      command: cargoAbout,
      args: [
        "generate",
        "--locked",
        "--fail",
        "--config",
        "about.toml",
        "--format",
        "json",
        "--output-file",
        auditPath,
      ],
      cwd: root,
    });
    const audit = JSON.parse(await readFile(auditPath, "utf8"));
    if (
      !Array.isArray(audit.licenses) ||
      audit.licenses.some((license) => !license.source_path)
    ) {
      throw new Error("a dependency license is missing an audited source");
    }

    await runCargoAbout({
      command: cargoAbout,
      args: [
        "generate",
        "--locked",
        "--fail",
        "--config",
        "about.toml",
        "--output-file",
        generatedPath,
        "about.hbs",
      ],
      cwd: root,
    });
    const generatedText = await readFile(generatedPath, "utf8");
    const generated = Buffer.from(`${generatedText.trimEnd()}\n`);
    if (generated.includes(unresolvedSource)) {
      throw new Error("a dependency license is missing an audited source");
    }
    const committedPath = join(root, noticePath);
    if (mode === "write") {
      await writeFile(committedPath, generated);
      return;
    }

    const committed = await readFile(committedPath);
    if (!committed.equals(generated)) {
      throw new Error(
        "THIRD_PARTY_LICENSES is stale; run npm run rust-licenses:generate",
      );
    }
  } finally {
    await rm(temporaryRoot, { force: true, recursive: true });
  }
}

async function main() {
  const mode = process.argv[2];
  if (mode !== "--check" && mode !== "--write") {
    throw new Error("usage: rust-licenses.js --check|--write");
  }
  await verifyRustLicenses({ mode: mode.slice(2) });
}

const entry = process.argv[1];
if (entry && import.meta.url === pathToFileURL(entry).href) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : error}\n`);
    process.exitCode = 1;
  });
}
