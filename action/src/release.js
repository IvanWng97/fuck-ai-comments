import { access, chmod, readFile } from "node:fs/promises";
import path from "node:path";

import * as toolCache from "@actions/tool-cache";
import semver from "semver";

import { expectedChecksum, verifyChecksum } from "./checksum.js";

const OWNER = "IvanWng97";
const REPOSITORY = "fuck-ai-comments";
const TOOL_CACHE_NAME = "fuck-ai-comments";

export function exactVersion(input) {
  const version = semver.valid(input);
  if (!version) {
    throw new Error(`version must be exact SemVer, got: ${input}`);
  }
  return version;
}

export function selectArchive(manifest, target) {
  const artifacts = Object.values(manifest.artifacts ?? {});
  const matches = artifacts.filter(
    (artifact) =>
      artifact.kind === "executable-zip" &&
      artifact.target_triples?.includes(target),
  );
  if (matches.length !== 1) {
    throw new Error(`release manifest has no unique archive for ${target}`);
  }

  const name = matches[0].name;
  if (
    typeof name !== "string" ||
    path.posix.basename(name) !== name ||
    path.win32.basename(name) !== name
  ) {
    throw new Error(
      `release manifest contains an unsafe archive name for ${target}`,
    );
  }
  if (!name.endsWith(".tar.xz") && !name.endsWith(".zip")) {
    throw new Error(`unsupported archive format: ${name}`);
  }
  return name;
}

export function releaseAssetUrl(version, asset) {
  return `https://github.com/${OWNER}/${REPOSITORY}/releases/download/v${encodeURIComponent(version)}/${encodeURIComponent(asset)}`;
}

export function cacheArchitecture(target, digest) {
  return `${target}-${digest}`;
}

export function extractedBinary(extractedDirectory, archiveName, binaryName) {
  if (archiveName.endsWith(".zip")) {
    return path.join(extractedDirectory, binaryName);
  }
  const root = archiveName.slice(0, -".tar.xz".length);
  return path.join(extractedDirectory, root, binaryName);
}

export async function installCli(versionInput, { target, binaryName }) {
  const version = exactVersion(versionInput);
  const [manifestPath, checksumPath] = await Promise.all([
    toolCache.downloadTool(releaseAssetUrl(version, "dist-manifest.json")),
    toolCache.downloadTool(releaseAssetUrl(version, "sha256.sum")),
  ]);
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const archiveName = selectArchive(manifest, target);
  const digest = expectedChecksum(
    await readFile(checksumPath, "utf8"),
    archiveName,
  );
  const cachedArchitecture = cacheArchitecture(target, digest);
  const cached = toolCache.find(TOOL_CACHE_NAME, version, cachedArchitecture);
  if (cached) {
    const binary = path.join(cached, binaryName);
    await ensureExecutable(binary);
    return binary;
  }

  const archivePath = await toolCache.downloadTool(
    releaseAssetUrl(version, archiveName),
  );
  await verifyChecksum(archivePath, checksumPath, archiveName);

  const extracted = archiveName.endsWith(".zip")
    ? await toolCache.extractZip(archivePath)
    : await toolCache.extractTar(archivePath, undefined, "xJ");
  const binary = extractedBinary(extracted, archiveName, binaryName);
  await ensureExecutable(binary);
  const cacheDirectory = await toolCache.cacheFile(
    binary,
    binaryName,
    TOOL_CACHE_NAME,
    version,
    cachedArchitecture,
  );
  return path.join(cacheDirectory, binaryName);
}

async function ensureExecutable(binary) {
  await access(binary);
  if (process.platform !== "win32") {
    await chmod(binary, 0o755);
  }
}
