const MODES = new Set(["all", "worktree", "staged", "base"]);
const UNSAFE_EXECUTABLE_PATH = /["\p{Cc}\u2028\u2029]/u;

export function executableCommand(executable) {
  if (UNSAFE_EXECUTABLE_PATH.test(executable)) {
    throw new Error("cannot safely represent executable path");
  }
  return `"${executable}"`;
}

export function buildCheckArguments({ mode, path, base, head }) {
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

  result.push("--", path);
  return result;
}
