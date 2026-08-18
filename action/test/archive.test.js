import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import * as toolCache from "@actions/tool-cache";

test("every native archive carries project and dependency licenses", async () => {
  const manifest = JSON.parse(
    await readFile("action/test/fixtures/dist-manifest-0.32.0.json", "utf8"),
  );
  const archives = Object.values(manifest.artifacts).filter(
    (artifact) => artifact.kind === "executable-zip",
  );

  assert.equal(archives.length, 4);
  for (const archive of archives) {
    const assets = new Set(archive.assets.map((asset) => asset.name));
    for (const required of [
      "LICENSE",
      "README.md",
      "THIRD_PARTY_LICENSES",
      "fuck-ai-comments",
    ]) {
      assert.ok(assets.has(required), `${archive.name}: missing ${required}`);
    }
  }
});

test(
  "tool-cache preserves the executable in cargo-dist tar layouts",
  { skip: process.platform === "win32" },
  async () => {
    const directory = await mkdtemp(path.join(tmpdir(), "fuck-ai-comments-"));
    const source = path.join(directory, "source");
    const archiveRoot = "fuck-ai-comments-x86_64-unknown-linux-gnu";
    const root = path.join(source, archiveRoot);
    const binary = path.join(root, "fuck-ai-comments");
    const archive = path.join(directory, `${archiveRoot}.tar.xz`);
    await mkdir(root, { recursive: true });
    await writeFile(binary, "#!/bin/sh\nexit 0\n");
    await chmod(binary, 0o755);
    execFileSync("tar", ["-cJf", archive, "-C", source, archiveRoot]);

    const extracted = await toolCache.extractTar(
      archive,
      path.join(directory, "extracted"),
      "xJ",
    );
    const metadata = await stat(
      path.join(extracted, archiveRoot, "fuck-ai-comments"),
    );

    assert.notEqual(metadata.mode & 0o111, 0);
  },
);
