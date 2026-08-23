import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import * as core from "@actions/core";
import * as exec from "@actions/exec";

import {
  buildCheckArgumentsFromInputs,
  executableCommand,
} from "./arguments.js";
import {
  MATCHER_OWNER,
  PROBLEM_MATCHER,
  failureMessage,
  sarifDocument,
} from "./check.js";
import { platformSpec } from "./platform.js";
import { installCli } from "./release.js";

async function run() {
  const specification = platformSpec(process.platform, process.arch);
  const executable = await installCli(
    core.getInput("version", { required: true }),
    specification,
  );
  core.addPath(path.dirname(executable));
  const command = executableCommand(executable);

  const matcher = await writeProblemMatcher();
  core.info(`::add-matcher::${matcher}`);
  try {
    const text = await exec.getExecOutput(
      command,
      buildCheckArgumentsFromInputs(core.getInput, "text"),
      { ignoreReturnCode: true },
    );
    const failure = failureMessage(text);

    const sarifFile = core.getInput("sarif-file");
    if (sarifFile) {
      const sarif = await exec.getExecOutput(
        command,
        buildCheckArgumentsFromInputs(core.getInput, "sarif"),
        { ignoreReturnCode: true, silent: true },
      );
      const target = path.resolve(
        process.env.GITHUB_WORKSPACE ?? process.cwd(),
        sarifFile,
      );
      await writeFile(target, sarifDocument(sarif));
      core.setOutput("sarif-file", target);
    }

    if (failure) {
      core.setFailed(failure);
    }
  } finally {
    core.info(`::remove-matcher owner=${MATCHER_OWNER}::`);
  }
}

async function writeProblemMatcher() {
  const directory = await mkdtemp(
    path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "fuck-ai-comments-"),
  );
  const file = path.join(directory, "problem-matcher.json");
  await writeFile(file, JSON.stringify(PROBLEM_MATCHER));
  return file;
}

run().catch((error) => {
  core.setFailed(error instanceof Error ? error.message : String(error));
});
