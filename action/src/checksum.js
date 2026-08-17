import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";

export function expectedChecksum(contents, artifactName) {
  const matches = [];
  for (const line of contents.split(/\r?\n/u)) {
    const match = /^([a-f\d]{64}) [ *](.+)$/iu.exec(line);
    if (match?.[2] === artifactName) {
      matches.push(match[1].toLowerCase());
    }
  }
  if (matches.length !== 1) {
    throw new Error(
      `sha256.sum must contain exactly one checksum for ${artifactName}`,
    );
  }
  return matches[0];
}

export async function sha256(file) {
  const digest = createHash("sha256");
  for await (const chunk of createReadStream(file)) {
    digest.update(chunk);
  }
  return digest.digest("hex");
}

export async function verifyChecksum(archive, checksumFile, artifactName) {
  const contents = await readFile(checksumFile, "utf8");
  const expected = expectedChecksum(contents, artifactName);
  const actual = await sha256(archive);
  if (actual !== expected) {
    throw new Error(`SHA-256 verification failed for ${artifactName}`);
  }
}
