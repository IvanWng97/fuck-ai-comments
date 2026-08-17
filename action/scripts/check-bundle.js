import { execFileSync } from "node:child_process";

const npm = process.platform === "win32" ? "npm.cmd" : "npm";
execFileSync(npm, ["run", "bundle"], { stdio: "inherit" });
execFileSync("git", ["diff", "--exit-code", "--", "dist"], {
  stdio: "inherit",
});
const untracked = execFileSync(
  "git",
  ["status", "--porcelain=v1", "--untracked-files=all", "--", "dist"],
  { encoding: "utf8" },
);
if (untracked.trim()) {
  process.stderr.write(untracked);
  process.exitCode = 1;
}
