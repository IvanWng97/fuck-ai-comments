import path from "node:path";

import * as core from "@actions/core";
import * as exec from "@actions/exec";

import { buildCheckArguments, executableCommand } from "./arguments.js";
import { platformSpec } from "./platform.js";
import { installCli } from "./release.js";

async function run() {
  const specification = platformSpec(process.platform, process.arch);
  const executable = await installCli(
    core.getInput("version", { required: true }),
    specification,
  );
  core.addPath(path.dirname(executable));

  const arguments_ = buildCheckArguments({
    mode: core.getInput("mode", { required: true }),
    profile: core.getInput("profile", { required: true }),
    path: core.getInput("path", { required: true }),
    base: core.getInput("base"),
    head: core.getInput("head"),
  });
  await exec.exec(executableCommand(executable), arguments_);
}

run().catch((error) => {
  core.setFailed(error instanceof Error ? error.message : String(error));
});
