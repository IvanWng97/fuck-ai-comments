# fuck-ai-comments

Required, owner-aware comment linting for repositories that do not want a
10-line function buried under 10 lines of narration.

`fuck-ai-comments` does not try to guess whether a human or an AI wrote a
comment. It enforces objective properties that make low-value comments harder
to add and stale comments harder to keep:

- comments have small budgets relative to the code they own;
- every surviving comment must be re-attested when its owner changes; and
- moving an unchanged comment to a different owner is a failure.

Every finding is an error. There are no advisory rules and no inline disable
comments.

## Rules

| Rule | Required policy |
| --- | --- |
| Function budget | At most `min(8, max(1, code_lines / 4))` narrative comment lines |
| Type budget | Classes use the same relative narrative budget as functions |
| Comment block | Three or more consecutive narrative-only lines fail |
| Leaf budget | Constants, statics, and equivalent leaves get at most 3 narrative lines |
| File budget | At most `min(8, max(2, code_lines / 16))` file-level narrative lines |
| Template budget | HTML, CSS, and Astro template owners get at most 3 narrative lines |
| Absolute owner cap | Non-public comments, including directives and safety proofs, cannot exceed 8 lines on function/type/file owners or 3 on leaf/template/TOML owners |
| Stale comment | An unchanged comment fails when its owning code or semantic role changes |
| Reparented comment | An unchanged comment fails when it moves to another owner |

Rust public API docs do not consume a length budget. Structurally valid safety
proofs and tool directives do not consume the relative narrative budget, but
they do count toward the absolute owner cap. All three remain subject to drift
checks. Python function and class docstrings consume their owner budget; module
docstrings consume the file budget.

## Supported languages

- Rust: `.rs`
- Python: `.py`, `.pyi`, `.pyw`
- TOML: `.toml`
- JavaScript: `.js`, `.cjs`, `.mjs`, `.jsx`
- TypeScript: `.ts`, `.cts`, `.mts`, `.tsx`
- Web: `.html`, `.htm`, `.css`, `.astro`

HTML and Astro use their native grammar and then dispatch embedded scripts and
styles to the JavaScript, TypeScript, or CSS adapter. Data scripts and explicit
unsupported preprocessors remain opaque.

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

Supported source files are limited to 16 MiB. Git blob batches are limited to
128 MiB. Exceeding either limit fails closed instead of risking an unbounded CI
allocation.

## GitHub Action

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

The Action ref follows the package major (`0.x` uses `@v0`). After a versioned
release succeeds, release automation advances that moving major ref to the
released commit; consumers do not need an exact Action commit SHA.

## Install

Until the first tagged release is published, install directly from the public
repository:

```console
cargo install --git https://github.com/IvanWng97/fuck-ai-comments --locked
```

Tagged releases provide native archives for x86-64 Linux, x86-64 Windows,
x86-64 macOS, and Apple Silicon macOS. Each release includes `sha256.sum`, a
dist manifest, and GitHub build attestations.

```console
gh attestation verify fuck-ai-comments-*.tar.xz \
  --repo IvanWng97/fuck-ai-comments
```

## Architecture

The implementation deliberately delegates mechanical work:

- `tree-sitter` and official language grammars parse source;
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

## Development

```console
cargo fmt --all -- --check
cargo test --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
npm ci --ignore-scripts
npm test
npm run lint
npm run bundle:check
npm run release:check
```

The Rust MSRV is 1.88.0. The Action is bundled. `release.yml` is generated from
the dist configuration, then narrowed to job-level permissions by
`npm run release:generate`; `release:check` regenerates it in a temporary copy
so cargo-dist template drift remains a hard failure. The pinned cargo-dist
template cannot scope its built-in permissions or disable secret inheritance
for custom jobs, so its `allow-dirty = ["ci"]` escape hatch is intentional; the
tested transform changes only those two privileges.

## License

[MIT](LICENSE)
