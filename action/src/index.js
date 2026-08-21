import path from "node:path";

import * as core from "@actions/core";
import * as exec from "@actions/exec";

import {
  buildCheckArgumentsFromInputs,
  executableCommand,
} from "./arguments.js";
import { platformSpec } from "./platform.js";
import { installCli } from "./release.js";
import { processCliResult } from "./report.js";

async function run() {
  const specification = platformSpec(process.platform, process.arch);
  const executable = await installCli(
    core.getInput("version", { required: true }),
    specification,
  );
  core.addPath(path.dirname(executable));

  const arguments_ = buildCheckArgumentsFromInputs(core.getInput);
  const result = await exec.getExecOutput(
    executableCommand(executable),
    arguments_,
    {
      ignoreReturnCode: true,
      silent: true,
    },
  );
  processCliResult(result, core);
}

run().catch((error) => {
  core.setFailed(error instanceof Error ? error.message : String(error));
});
