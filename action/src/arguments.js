const MODES = new Set(["all", "worktree", "staged", "base"]);
const UNSAFE_EXECUTABLE_PATH = /["\p{Cc}\u2028\u2029]/u;

export function executableCommand(executable) {
  if (UNSAFE_EXECUTABLE_PATH.test(executable)) {
    throw new Error("cannot safely represent executable path");
  }
  return `"${executable}"`;
}

export function buildCheckArgumentsFromInputs(getInput) {
  return buildCheckArguments({
    mode: getInput("mode", { required: true }),
    profile: getInput("profile", { required: true }),
    path: getInput("path", { required: true }),
    base: getInput("base"),
    head: getInput("head"),
    config: getInput("config"),
  });
}

export function buildCheckArguments({
  mode,
  profile,
  path,
  base,
  head,
  config,
}) {
  if (!MODES.has(mode)) {
    throw new Error(`unsupported check mode: ${mode}`);
  }
  if (mode === "base" && !base) {
    throw new Error("base is required when mode is base");
  }
  if (mode !== "base" && (base || head)) {
    throw new Error("base and head are only valid when mode is base");
  }

  const result = ["check"];
  if (mode === "all") {
    result.push("--all");
  } else if (mode === "staged") {
    result.push("--staged");
  } else if (mode === "base") {
    result.push("--base", base);
    if (head) {
      result.push("--head", head);
    }
  }

  result.push("--profile", profile);
  if (config) {
    result.push("--config", config);
  }
  result.push("--", path);
  return result;
}
