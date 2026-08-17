export const SUPPORTED_TARGETS = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-pc-windows-msvc",
  "x86_64-unknown-linux-gnu",
];

const PLATFORMS = new Map([
  ["darwin-arm64", ["aarch64-apple-darwin", "fuck-ai-comments"]],
  ["darwin-x64", ["x86_64-apple-darwin", "fuck-ai-comments"]],
  ["linux-x64", ["x86_64-unknown-linux-gnu", "fuck-ai-comments"]],
  ["win32-x64", ["x86_64-pc-windows-msvc", "fuck-ai-comments.exe"]],
]);

export function platformSpec(platform, architecture) {
  const resolved = PLATFORMS.get(`${platform}-${architecture}`);
  if (!resolved) {
    throw new Error(`unsupported runner: ${platform}/${architecture}`);
  }
  return { target: resolved[0], binaryName: resolved[1] };
}
