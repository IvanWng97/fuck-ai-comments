import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { parse as parseToml } from "smol-toml";

import { platformSpec, SUPPORTED_TARGETS } from "../src/platform.js";
import {
  cacheArchitecture,
  exactVersion,
  extractedBinary,
  releaseAssetUrl,
  selectArchive,
} from "../src/release.js";

test("maps supported runners to dist targets", () => {
  assert.deepEqual(platformSpec("darwin", "arm64"), {
    target: "aarch64-apple-darwin",
    binaryName: "fuck-ai-comments",
  });
  assert.deepEqual(platformSpec("darwin", "x64"), {
    target: "x86_64-apple-darwin",
    binaryName: "fuck-ai-comments",
  });
  assert.deepEqual(platformSpec("linux", "x64"), {
    target: "x86_64-unknown-linux-gnu",
    binaryName: "fuck-ai-comments",
  });
  assert.deepEqual(platformSpec("win32", "x64"), {
    target: "x86_64-pc-windows-msvc",
    binaryName: "fuck-ai-comments.exe",
  });
  assert.throws(() => platformSpec("linux", "arm64"), /unsupported runner/u);
});

test("keeps action targets aligned with generated dist configuration", async () => {
  const configuration = parseToml(
    await readFile("dist-workspace.toml", "utf8"),
  );

  assert.deepEqual(configuration.dist.targets.toSorted(), SUPPORTED_TARGETS);
  assert.deepEqual(configuration.dist.installers, []);
  assert.equal(configuration.dist.checksum, "sha256");
  assert.equal(configuration.dist["source-tarball"], false);
  assert.equal(configuration.dist["github-attestations"], true);
});

test("selects archives from a real dist 0.32 manifest fixture", async () => {
  const manifest = JSON.parse(
    await readFile("action/test/fixtures/dist-manifest-0.32.0.json", "utf8"),
  );
  assert.equal(manifest.dist_version, "0.32.0");
  for (const target of SUPPORTED_TARGETS) {
    const selected = selectArchive(manifest, target);
    assert.match(selected, new RegExp(target, "u"));
  }
  assert.throws(
    () => selectArchive({ artifacts: {} }, "x86_64-unknown-linux-gnu"),
    /no unique archive/u,
  );
});

test("resolves cargo-dist archive layouts", () => {
  assert.equal(
    extractedBinary(
      "/tmp/extracted",
      "fuck-ai-comments-x86_64-unknown-linux-gnu.tar.xz",
      "fuck-ai-comments",
    ),
    "/tmp/extracted/fuck-ai-comments-x86_64-unknown-linux-gnu/fuck-ai-comments",
  );
  assert.equal(
    extractedBinary(
      "/tmp/extracted",
      "fuck-ai-comments-x86_64-pc-windows-msvc.zip",
      "fuck-ai-comments.exe",
    ),
    "/tmp/extracted/fuck-ai-comments.exe",
  );
});

test("uses the root-level Windows layout from cargo-dist 0.32", async () => {
  const fixture = JSON.parse(
    await readFile(
      "action/test/fixtures/cargo-dist-0.32.0-windows-archive.json",
      "utf8",
    ),
  );

  assert.match(fixture.sha256, /^[a-f\d]{64}$/u);
  assert.ok(fixture.entries.includes(fixture.binary));
  assert.ok(fixture.entries.every((entry) => !entry.includes("/")));
  assert.equal(
    extractedBinary("/tmp/extracted", fixture.archive, fixture.binary),
    "/tmp/extracted/dist.exe",
  );
});

test("rejects release archive traversal on either path syntax", () => {
  const artifact = (name) => ({
    artifacts: {
      archive: {
        kind: "executable-zip",
        name,
        target_triples: ["x86_64-pc-windows-msvc"],
      },
    },
  });

  assert.throws(
    () =>
      selectArchive(
        artifact("../fuck-ai-comments.zip"),
        "x86_64-pc-windows-msvc",
      ),
    /unsafe archive name/u,
  );
  assert.throws(
    () =>
      selectArchive(
        artifact("..\\fuck-ai-comments.zip"),
        "x86_64-pc-windows-msvc",
      ),
    /unsafe archive name/u,
  );
});

test("requires an exact version and never constructs a latest URL", () => {
  assert.equal(exactVersion("v0.1.0"), "0.1.0");
  assert.throws(() => exactVersion("latest"), /exact SemVer/u);
  assert.throws(() => exactVersion("^0.1.0"), /exact SemVer/u);
  assert.equal(
    releaseAssetUrl("0.1.0", "sha256.sum"),
    "https://github.com/IvanWng97/fuck-ai-comments/releases/download/v0.1.0/sha256.sum",
  );
  assert.equal(
    cacheArchitecture("x86_64-unknown-linux-gnu", "abc123"),
    "x86_64-unknown-linux-gnu-abc123",
  );
});
