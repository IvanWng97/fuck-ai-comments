import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { expectedChecksum, sha256, verifyChecksum } from "../src/checksum.js";

test("verifies an archive against the unified checksum manifest", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "fuck-ai-comments-"));
  const archive = path.join(directory, "tool.tar.xz");
  const checksums = path.join(directory, "sha256.sum");
  await writeFile(archive, "verified bytes");
  const digest = await sha256(archive);
  await writeFile(checksums, `${digest}  tool.tar.xz\n`);

  await verifyChecksum(archive, checksums, "tool.tar.xz");
});

test("fails closed for tampering, missing entries, and duplicates", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "fuck-ai-comments-"));
  const archive = path.join(directory, "tool.zip");
  const checksums = path.join(directory, "sha256.sum");
  await writeFile(archive, "original");
  const digest = await sha256(archive);
  await writeFile(checksums, `${digest} *tool.zip\n`);
  await writeFile(archive, "tampered");

  await assert.rejects(
    verifyChecksum(archive, checksums, "tool.zip"),
    /verification failed/u,
  );
  assert.throws(() => expectedChecksum("", "tool.zip"), /exactly one/u);
  assert.throws(
    () =>
      expectedChecksum(
        `${digest}  tool.zip\n${digest} *tool.zip\n`,
        "tool.zip",
      ),
    /exactly one/u,
  );
});
