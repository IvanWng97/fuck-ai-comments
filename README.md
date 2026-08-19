# fuck-ai-comments

Required, owner-aware comment linting for repositories that do not want a
10-line function buried under 10 lines of narration.

`fuck-ai-comments` does not try to guess whether a human or an AI wrote a
comment. It enforces objective properties that make low-value comments harder
to add and stale comments harder to keep:

- comments have small budgets relative to the code they own;
- every surviving comment with meaningful normalized text must be re-attested
  when its owner changes; and
- moving that unchanged text to a different owner is a failure.

Every finding is an error. There are no advisory rules and no inline disable
comments.

## Rules

| Rule                             | Required policy                                                                                                                                   |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Function budget                  | At most `min(8, max(1, code_lines / 4))` narrative comment lines                                                                                  |
| Type budget                      | Recognized type owners in Python, JavaScript/TypeScript, Kotlin, Objective-C, and Swift use the same relative narrative budget as functions       |
| Function/type/file comment block | Three or more consecutive narrative-only lines fail; leaf, template, and TOML owners allow at most 3 total                                        |
| Leaf budget                      | Constants, statics, and equivalent leaves get at most 3 narrative lines                                                                           |
| File budget                      | At most `min(8, max(2, code_lines / 16))` file-level narrative lines                                                                              |
| Template budget                  | HTML, CSS, and Astro template owners get at most 3 narrative lines                                                                                |
| Absolute owner cap               | Non-public comments, including directives and safety proofs, cannot exceed 8 lines on function/type/file owners or 3 on leaf/template/TOML owners |
| Stale comment                    | Unchanged meaningful normalized text fails when its owning code or semantic role changes                                                          |
| Reparented comment               | Unchanged meaningful normalized text fails when it moves to another owner                                                                         |

Function and type `code_lines` count physical rows assigned to that budget.
Nested functions and types use their own budgets, while leaf code stays in its
nearest function or type budget. File `code_lines` remain whole-file.

Rust public API docs do not consume a length budget. Structurally valid safety
proofs and tool directives do not consume the relative narrative budget, but
they do count toward the absolute owner cap. All three remain subject to drift
checks when they have meaningful normalized text. Python function and class
docstrings consume their owner budget; module docstrings consume the file budget.

Normalized-empty separators retain their existing static classification and
budget treatment, but have no cross-revision identity and therefore cannot
become stale or reparented.

## Supported languages

- Rust: `.rs`
- Python: `.py`, `.pyi`, `.pyw`
- TOML: `.toml`
- JavaScript: `.js`, `.cjs`, `.mjs`, `.jsx`
- TypeScript and React TSX: `.ts`, `.cts`, `.mts`, `.tsx`
- Objective-C: `.m`
- Swift: `.swift`
- Kotlin: `.kt`, `.kts`
- Web: `.html`, `.htm`, `.css`, `.astro`

HTML and Astro use their native grammar and then dispatch embedded scripts and
styles to the JavaScript, TypeScript, or CSS adapter. Data scripts and explicit
unsupported preprocessors remain opaque.

React uses the TypeScript TSX grammar and the same callable-owner rules as
JavaScript; it is not a duplicate language adapter. Objective-C headers are not
registered because `.h` is ambiguous across C, C++, and Objective-C, and `.mm`
is not registered because the upstream Objective-C grammar does not implement
complete Objective-C++.

Tool-directive recognition validates source syntax and placement, not whether
an external linter configuration enables a rule identifier. Custom rule IDs
make a baked-in catalog incorrect; directives still count toward the absolute
owner cap and still require stale-comment attestation.

## CLI

```console
# New baseline: scan every supported regular file, including hidden paths.
# .gitignore and .ignore still define intentional exclusions.
fuck-ai-comments check --all

# Scan one directory or file; absolute paths work too.
fuck-ai-comments check --all ./src

# Local default: HEAD versus staged + unstaged + untracked content.
fuck-ai-comments check

# Restrict changed-owner analysis to one directory.
fuck-ai-comments check ./src

# Pre-commit: HEAD versus the index only.
fuck-ai-comments check --staged

# CI or a committed branch range. The CLI compares merge-base(base, head) to head.
fuck-ai-comments check --base origin/main --head HEAD
```

Pass a file or directory as the final argument to narrow the scope. Exit codes
are stable:

- `0`: clean;
- `1`: one or more required policy findings;
- `2`: the analysis could not be trusted, including parser, Git, UTF-8,
  ownership ambiguity, nonexistent scopes, non-regular supported paths, or
  resource-limit errors.

Git is the authority for rename pairing; if supported additions and deletions
remain unpaired, the CLI cannot prove ancestry and exits with code `2`.

Supported source files are limited to 16 MiB. Git blob batches are limited to
128 MiB. Exceeding either limit fails closed instead of risking an unbounded CI
allocation.

## GitHub Action

The Action becomes available when the first stable release creates the `v0`
compatibility tag.

The default Action mode is the fail-closed `base` mode: callers must provide a
base revision, and the CLI compares the base/head merge base to the head. A bare
Action invocation fails instead of silently running a clean-worktree scan that
cannot detect stale comments.

For pull requests, make the static baseline and changed-owner attestation two
required steps in the same required job:

```yaml
name: comment-policy

on:
  pull_request:

permissions:
  contents: read

jobs:
  comments:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
          persist-credentials: false
      - name: Enforce repository-wide comment budgets
        uses: IvanWng97/fuck-ai-comments@v0
        with:
          mode: all
      - name: Enforce changed-owner attestation
        uses: IvanWng97/fuck-ai-comments@v0
        with:
          mode: base
          base: ${{ github.event.pull_request.base.sha }}
          head: ${{ github.event.pull_request.head.sha }}
```

Action inputs are `mode` (`all`, `worktree`, `staged`, or `base`), `path`,
`base`, `head`, and an exact CLI `version`. Inputs become an argv array; they
are never concatenated into a shell command. `mode: all` establishes or checks
a static baseline; it does not replace `mode: base`, because a single valid
comment can only be checked for drift by comparing revisions. Push and manual
workflows must likewise pass an explicit before/after range for drift checks,
or deliberately use `mode: all` when the job exists only to establish a new
baseline.

The first stable `0.x` release creates `@v0` after packaged-Action E2E succeeds;
subsequent stable `0.x` releases advance it under the same gate. Consumers do
not need an exact Action commit SHA.

## Install

Until the first tagged release is published, install directly from the public
repository:

```console
cargo install --git https://github.com/IvanWng97/fuck-ai-comments --locked
```

Releases are manual workflow dispatches from `main`; the workflow creates the
tag only after authorization, artifact builds, and hosting succeed:

```console
gh workflow run release.yml --ref main -f tag=v0.1.0
```

Before the first dispatch, protect `main` and create a `release-authorization`
environment. The `release-authorization` environment only permits protected
branches and carries no secrets. Activate a repository tag ruleset for all tag
refs (`~ALL`). The ruleset requires a successful `release-authorization`
deployment. It restricts deletion and blocks force pushes and non-fast-forward
updates. Configure no owner or administrator bypass. This broad scope is required
because historical cargo-dist workflows accepted several noncanonical version-tag
shapes; the current workflow authorizes only `v<package-version>`. It also requires
that the dispatched commit is the current `main` HEAD and has a successful `ci.yml`
push run for that exact SHA. A `tag=dry-run` dispatch may build and test without
repository write permission.

Published releases provide native archives for x86-64 Linux, x86-64 Windows,
x86-64 macOS, and Apple Silicon macOS. Each release includes `sha256.sum`, a
dist manifest, and GitHub build attestations. Every native archive carries the
project `LICENSE`, `README.md`, and generated `THIRD_PARTY_LICENSES` notices.
The packaged Action must then pass on Linux, macOS, and Windows before a stable
`0.x` release advances `v0`; prereleases run the same checks without changing
the compatibility tag.

On macOS or Linux, download and verify the native archives:

```sh
gh release download --repo IvanWng97/fuck-ai-comments \
  --pattern 'fuck-ai-comments-*.tar.xz'
for artifact in fuck-ai-comments-*.tar.xz; do
  gh attestation verify "$artifact" --repo IvanWng97/fuck-ai-comments
done
```

On Windows PowerShell, download and verify the Windows archive:

```powershell
gh release download --repo IvanWng97/fuck-ai-comments `
  --pattern 'fuck-ai-comments-*.zip'
Get-ChildItem -File fuck-ai-comments-*.zip | ForEach-Object {
  gh attestation verify $_.FullName --repo IvanWng97/fuck-ai-comments
}
```

## Architecture

The implementation deliberately delegates mechanical work:

- `tree-sitter` and maintained upstream language grammars parse source;
- `toml_edit` plus the official `toml_parser` lexer preserve TOML structure and
  exact comment spans;
- `similar` supplies Myers diff anchors;
- `ignore` supplies repository walking and ignore semantics;
- Git plumbing supplies revisions, rename records, and blobs;
- `dist` supplies release planning and native archives; and
- the official GitHub Actions toolkit supplies download, extraction, cache,
  execution, and error reporting.

The project owns only its domain semantics: comment ownership, budgets,
per-comment attestation, and fail-closed pairing.

Language adapters do not repair or replace an upstream grammar. Valid syntax
that the selected grammar cannot parse exits with code `2`; compatibility
workarounds are accepted only when they preserve the upstream AST and exact
source coordinates. Parser and grammar versions are exact-pinned because their
AST shapes are part of the policy contract; dependency upgrades must pass the
adapter tests and corpus scan explicitly.

## Development

```console
cargo fmt --all -- --check
cargo test --lib --bins --tests --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
npm ci --ignore-scripts
npm test
npm run lint
npm run bundle:check
npm run rust-licenses:check
npm run release:check
```

The release-workflow commands require cargo-dist 0.32.0's `dist` executable on
`PATH`. The Rust license commands require the pinned cargo-about version;
install it with `npm run rust-licenses:install`, and regenerate the committed
notice with `npm run rust-licenses:generate` after dependency changes.

The Rust MSRV is 1.88.0. The Action is bundled.

The release pipeline is cargo-dist 0.32.0 output plus one tested hardening layer
covering safe tag arguments, pre-build authorization and DAG gates,
released-Action E2E ordering, full Action commit pins, least permissions, and no
inherited secrets. Run `npm run release:generate` to apply that layer;
`release:check` regenerates the workflow in a temporary copy so cargo-dist
template drift remains a hard failure. The pinned template cannot express all
of these constraints, so its `allow-dirty = ["ci"]` escape hatch is intentional.

### Performance and coverage

Run `cargo bench --locked --bench analysis` for local wall-clock baselines. Every
`static_*_10k_loc` case analyzes exactly 10,000 physical source lines through
`analyze_all`; the change case compares two 10,000-line snapshots through
`analyze_change`. The suite covers every adapter family, including separate
TypeScript and TSX grammars plus Astro fast and recovery paths. Its elapsed time
is therefore milliseconds per 10K LOC. CodSpeed runs the same deterministic
fixtures on pull requests with simulated CPU and heap-allocation measurements.
Local wall-clock results are machine-specific and are not a shared-runner gate;
the synthetic fixtures are stress workloads, not claims of representative
application throughput.

Codecov reports informational Rust line coverage from the ordinary test suite.
Generate the same LCOV report locally with:

```console
cargo llvm-cov --workspace --all-features --locked --lcov --output-path lcov.info
```

## License

[MIT](LICENSE)
