import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { verifyRustLicenses } from "../scripts/rust-licenses.js";

test("committed notices contain parser attributions without local paths", async () => {
  const notices = await readFile("THIRD_PARTY_LICENSES", "utf8");

  for (const expected of [
    "tree-sitter-swift 0.7.3",
    "Copyright (c) 2021 alex-pinkus",
    "tree-sitter-objc 3.0.2",
    "Copyright (c) 2023 Amaan Qureshi",
    "tree-sitter-kotlin-ng 1.1.0",
    "Copyright (c) 2024 Amaan Qureshi",
  ]) {
    assert.ok(notices.includes(expected), `missing ${expected}`);
  }
  assert.doesNotMatch(notices, /fuck-ai-comments 0\.1\.0/u);
  assert.doesNotMatch(notices, /(?:\/Users\/|[A-Z]:\\Users\\)/u);
  assert.doesNotMatch(notices, /&(?:amp|gt|lt|quot);/u);
  assert.doesNotMatch(notices, /UNRESOLVED-LICENSE-SOURCE/u);
});

async function licenseFixture(testContext, notice) {
  const root = await mkdtemp(path.join(tmpdir(), "fuck-ai-comments-licenses-"));
  testContext.after(() => rm(root, { force: true, recursive: true }));
  await writeFile(path.join(root, "about.toml"), "accepted = []\n");
  await writeFile(path.join(root, "about.hbs"), "{{text}}\n");
  await writeFile(path.join(root, "THIRD_PARTY_LICENSES"), notice);
  return root;
}

function fakeCargoAbout(generated, sourcePath = "LICENSE") {
  return async ({ args }) => {
    if (args[0] === "--version") {
      return "cargo-about 0.9.1\n";
    }
    if (args.includes("json")) {
      assert.deepEqual(args.slice(0, 7), [
        "generate",
        "--locked",
        "--fail",
        "--config",
        "about.toml",
        "--format",
        "json",
      ]);
      const outputIndex = args.indexOf("--output-file") + 1;
      await writeFile(
        args[outputIndex],
        JSON.stringify({ licenses: [{ source_path: sourcePath }] }),
      );
      return "";
    }
    assert.deepEqual(args.slice(0, 6), [
      "generate",
      "--locked",
      "--fail",
      "--config",
      "about.toml",
      "--output-file",
    ]);
    assert.equal(args.at(-1), "about.hbs");
    await writeFile(args[6], generated);
    return "";
  };
}

test("license verification rejects a stale committed notice", async (t) => {
  const root = await licenseFixture(t, "stale notices\n");

  await assert.rejects(
    verifyRustLicenses({
      mode: "check",
      root,
      runCargoAbout: fakeCargoAbout("generated notices\n"),
    }),
    /THIRD_PARTY_LICENSES is stale/u,
  );
});

test("license verification rejects synthesized license text", async (t) => {
  const generated = "generated notices\n";
  const root = await licenseFixture(t, generated);

  await assert.rejects(
    verifyRustLicenses({
      mode: "check",
      root,
      runCargoAbout: fakeCargoAbout(generated, null),
    }),
    /missing an audited source/u,
  );
});

test("license generation cannot overwrite notices after a later source failure", async (t) => {
  const generated = "UNRESOLVED-LICENSE-SOURCE\n";
  const root = await licenseFixture(t, generated);

  await assert.rejects(
    verifyRustLicenses({
      mode: "write",
      root,
      runCargoAbout: fakeCargoAbout(generated),
    }),
    /missing an audited source/u,
  );
});

test("license generation writes one final newline", async (t) => {
  const root = await licenseFixture(t, "old notices\n");

  await verifyRustLicenses({
    mode: "write",
    root,
    runCargoAbout: fakeCargoAbout("generated notices\n\n"),
  });

  assert.equal(
    await readFile(path.join(root, "THIRD_PARTY_LICENSES"), "utf8"),
    "generated notices\n",
  );
});
