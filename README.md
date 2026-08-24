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

## Quick start

Install the current crates.io release, then establish a repository-wide static
baseline:

```console
cargo install fuck-ai-comments --locked
cd your-repository
fuck-ai-comments check --all
```

Commit an optional `fuck-ai-comments.toml` when a repository needs explicit
semantic-category caps or generated-source exclusions. For pull requests, use
the [`@v0` GitHub Action](#github-action) to enforce both the static baseline
and changed-owner attestation.

## Rules

| Rule                             | Required policy                                                                                                                                                                            |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Function budget                  | At most `min(8, max(1, code_lines / 4))` narrative comment lines                                                                                                                           |
| Type budget                      | Recognized type owners in Rust, Python, JavaScript/TypeScript, Kotlin, Objective-C, and Swift use the same relative narrative budget as functions                                          |
| Function/type/file comment block | Three or more consecutive narrative-only lines fail; leaf, member, template, and TOML owners allow at most 3 total                                                                         |
| Leaf budget                      | Constants, statics, and equivalent leaves get at most 3 narrative lines                                                                                                                    |
| Member budget                    | Rust struct, union, and tuple fields, enum variants, and const/static struct-literal fields are members with their own 3-line narrative budget; member docs never aggregate onto the owner |
| File budget                      | At most `min(8, max(2, code_lines / 16))` file-level narrative lines                                                                                                                       |
| Template budget                  | HTML, CSS, and Astro template owners get at most 3 narrative lines                                                                                                                         |
| Default absolute owner cap       | Comments using built-in relative or owner-capped policies cannot exceed 8 lines on function/type/file owners or 3 on leaf/member/template/TOML owners                                      |
| Stale comment                    | Unchanged meaningful normalized text fails when its owning code or semantic role changes                                                                                                   |
| Reparented comment               | Unchanged meaningful normalized text fails when it moves to another owner                                                                                                                  |

Function and type `code_lines` count physical rows assigned to that budget.
Nested functions and types use their own budgets, while leaf code stays in its
nearest function or type budget. File `code_lines` remain whole-file.
Rust structs, enums, unions, traits, type aliases, and implementation blocks are
type owners. Struct, union, and tuple fields and enum variants are member
owners: each budgets its own comments under the same semantic categories, while
its code rows still size the declaring type's relative budget. Member identities
are parent-qualified (`Report.width`, `Kind::First`). Field initializers of a
struct literal that is a `const` or `static` value, directly or through
single-value wrappers such as `Some(..)`, `Box::new(..)`, or `&`, are members
of that leaf (`FOUR.a`, `NESTED.inner.a`); rows inside array, tuple, or
multi-argument literals have no stable identity and stay with the leaf. Module
headers and documentation on module declarations remain at file scope.

Documentation is classified from syntax and attachment, not from a marker alone.
The adapters recognize Python docstrings; attached Rust docs; attached JSDoc and
TSDoc; KDoc; Swift `///` and `/** ... */` prefixes (including compiler-recognized
repeated delimiters); and leading Objective-C Doxygen comments or its same-line
trailing member forms. Detached documentation-looking comments remain narrative.
General documentation uses the relative budget by default.

Proven Rust public API docs do not consume a length budget. In one repository
discovery pass, the CLI asks each detected Cargo workspace once for authoritative
library target roots, including custom `[lib].path` and workspace members. Pure
library calls retain the conservative `src/lib.rs` default unless the caller
supplies an `AnalysisContext` with proven roots.
Structurally valid safety proofs and tool directives do not consume the relative
narrative budget, but by default they count toward the absolute owner cap. All three
remain subject to drift checks when they have meaningful normalized text. Python
function and class docstrings attach to their owner; module docstrings attach to
the file.

Normalization strips comment delimiters, collapses whitespace, ignores terminal
punctuation, and removes Unicode `Default_Ignorable_Code_Point` characters.
Comments left empty by that normalization retain their static classification
and budget treatment, but have no cross-revision identity and therefore cannot
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
make a baked-in catalog incorrect; by default directives still count toward the
absolute owner cap and still require stale-comment attestation.

## Configuration

Place an optional `fuck-ai-comments.toml` at the Git repository root. `--all`
uses that repository authority even when its final path narrows the scan to one
file or subdirectory. Outside a Git worktree, `--all` discovers the file at its
scan root. The schema is versioned and strict: unknown fields, unsupported
versions, and `max-lines` on any mode other than `capped` fail with exit code
`2`. Invalid exclusion patterns also fail closed.

```toml
schema-version = 2
exclude = ["generated/**", "vendor/**", "!vendor/maintained.py"]

[comments.documentation]
mode = "capped"
max-lines = 6

[comments.public-documentation]
mode = "unlimited"

[comments.safety-proof]
mode = "owner-capped"

[comments.tool-directive]
mode = "owner-capped"
```

The configurable semantic categories are `narrative`, `documentation`,
`public-documentation`, `safety-proof`, and `tool-directive`. They are
language-independent: `documentation`, for example, covers Python docstrings,
internal Rust docs, JSDoc/TSDoc, KDoc, Swift docs, and Objective-C docs. Each
category accepts one mode:

- `relative`: use the existing owner-relative and comment-block budgets;
- `owner-capped`: skip relative budgets but retain the built-in absolute cap
  for the owning function, type, file, leaf, member, template, or TOML key;
- `capped`: replace the relative and aggregate owner budgets for that semantic
  category with `max-lines` per owner; or
- `unlimited`: skip static length budgets for that semantic category.

`max-lines` is a nonnegative integer; `0` bans that category. Omitted entries
preserve the built-in modes: `narrative` and `documentation` are `relative`,
`public-documentation` is `unlimited`, and structural `safety-proof` and
`tool-directive` comments are `owner-capped`. A configured `capped` mode is
enforced independently, so it can raise or lower the built-in ceiling for one
category without making it unlimited.

Classification stays structural. Valid inner Rust docs and outer docs attached
to a Rust item are documentation; a detached `///` sequence remains narrative.
Rustdoc syntax outranks markers in its prose, so `/// SAFETY: ...` remains
documentation while a structurally attached non-doc `// SAFETY: ...` is a safety
proof. Safety proofs and tool directives must satisfy their existing marker,
syntax, and attachment rules. Configuration changes budgets only: it does not
create arbitrary prefix classifiers, and `unlimited` never disables stale-comment
or reparenting checks.

Top-level `exclude` entries use gitignore syntax, including `!` re-inclusions,
and apply to repository-relative source paths. Automatic discovery and every
Git mode match from the Git worktree root. Outside Git, `--all` matches from its
scan root; explicit `--all --config` also uses the scan root and does not invoke
Git discovery. Exclusions never hide the active config or Cargo metadata inputs
from authority checks.

Git modes load configuration from the same authority as the source: the
worktree, index for `--staged`, or requested head commit for `--base`. A
configuration-only change triggers a full static rescan under `--profile full`.
Use `--config PATH` to select a different file explicitly; an explicit path is
always read from the current filesystem, so full-profile Git checks
conservatively rescan all included source files. An explicit config inside the
scan scope is excluded from the source count. Configurations do not cascade or
merge.

## CLI

```console
# New baseline: scan every supported regular file, including hidden paths.
# .gitignore and .ignore still define intentional exclusions.
fuck-ai-comments check --all

# Scan one directory or file; absolute paths work too.
fuck-ai-comments check --all ./src

# Local default: HEAD, or an empty pre-first-commit baseline, versus current content.
fuck-ai-comments check

# Restrict changed-owner analysis to one directory.
fuck-ai-comments check ./src

# Pre-commit: HEAD, or an empty pre-first-commit baseline, versus the index only.
fuck-ai-comments check --staged

# CI or a committed branch range. The CLI compares merge-base(base, head) to head.
fuck-ai-comments check --base origin/main --head HEAD

# Change attestation only: validate and pair source, but skip static budgets.
fuck-ai-comments check --base origin/main --head HEAD --profile attestation

# Override automatic fuck-ai-comments.toml discovery.
fuck-ai-comments check --all --config policy/comments.toml

# Emit the stable, versioned report consumed by integrations.
fuck-ai-comments check --all --format json

# Emit SARIF 2.1.0 for GitHub code scanning or any SARIF consumer.
fuck-ai-comments check --all --format sarif > comment-policy.sarif
```

Pass a file or directory as the final argument to narrow the scope. Exit codes
are stable:

- `0`: clean;
- `1`: one or more required policy findings;
- `2`: the analysis could not be trusted, including parser, Git, UTF-8,
  ownership ambiguity, nonexistent scopes, non-regular supported paths, or
resource-limit errors.

`--format json` preserves those exit codes and writes one report containing
`schemaVersion`, `filesScanned`, and sorted `findings` (`path`, `line`, `rule`,
and `message`). `--format sarif` preserves them too and writes one SARIF 2.1.0
run whose driver publishes every rule from `fuck_ai_comments::rules` with
`shortDescription`, `fullDescription`, and `help`; each finding becomes an
`error`-level result with one physical location. Trusted-analysis failures
(exit `2`) remain diagnostics on standard error instead of pretending to be a
policy report.

Git is the authority for rename pairing; if supported additions and deletions
remain unpaired, the CLI cannot prove ancestry and exits with code `2`.
Before and after snapshots that select different language adapters likewise
exit with code `2`; the changed file is never reinterpreted as a new static
baseline.
When a Cargo manifest is detected, failure to resolve its workspace metadata
also exits with code `2`; the CLI never guesses custom Rust library roots from
filenames or a partial manifest parse. File-only ignore rules cannot hide a
manifest in a scanned directory, and Git modes seed discovery from the
authoritative tracked-manifest set.
Git modes reuse one Cargo role map only after proving that every compared
snapshot has unchanged Cargo target inputs (`Cargo.toml` and implicit
`src/lib.rs` roots) and that live metadata matches the
worktree, index, or requested head. A target-input change or an external
workspace whose committed state cannot be proven exits with code `2`.
The default `--profile full` preserves all policy checks. The `attestation`
profile is valid only for Git change modes and emits only
`comment-owner-changed` and `comment-reparented`; added files are still parsed
and validated, while `--all --profile attestation` is rejected.

Supported source files are limited to 16 MiB. Git blob batches are limited to
128 MiB. Exceeding either limit fails closed instead of risking an unbounded CI
allocation.

## GitHub Action

Use `IvanWng97/fuck-ai-comments@v0`; stable `0.x` releases advance that
compatibility tag only after packaged-Action E2E passes.

The default Action mode is the fail-closed `base` mode: callers must provide a
base revision, and the CLI compares the base/head merge base to the head. A bare
Action invocation fails instead of silently running a clean-worktree scan that
cannot detect stale comments.

For pull requests, make the static baseline and changed-owner attestation two
required steps in the same required job, and publish the static findings to
code scanning with GitHub's own `upload-sarif` step:

```yaml
name: comment-policy

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read
  security-events: write

jobs:
  comments:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
          persist-credentials: false
      - name: Enforce repository-wide comment budgets
        id: budgets
        uses: IvanWng97/fuck-ai-comments@v0
        with:
          mode: all
          sarif-file: comment-policy.sarif
      - name: Publish comment budgets to code scanning
        if: ${{ !cancelled() && steps.budgets.outputs.sarif-file }}
        uses: github/codeql-action/upload-sarif@v4
        with:
          sarif_file: ${{ steps.budgets.outputs.sarif-file }}
          category: comment-policy
      - name: Enforce changed-owner attestation
        if: github.event_name == 'pull_request'
        uses: IvanWng97/fuck-ai-comments@v0
        with:
          mode: base
          profile: attestation
          base: ${{ github.event.pull_request.base.sha }}
          head: ${{ github.event.pull_request.head.sha }}
```

Action inputs are `mode` (`all`, `worktree`, `staged`, or `base`), `profile`
(`full` or `attestation`), `path`, `base`, `head`, `config`, `sarif-file`, and
an exact CLI `version`; the `sarif-file` output is the absolute path of the
written report.
Inputs become an argv array; they are never concatenated into a shell command,
and the Rust CLI is the profile-validation authority. The Action streams the
CLI's text report into the step log under a GitHub problem matcher, so findings
become source-line annotations within the platform's per-step limit, the
complete list stays in the log, and the step fails once with the CLI summary.
With `sarif-file`, it also writes a SARIF 2.1.0 report; uploading it with
`github/codeql-action/upload-sarif` turns every finding into a code scanning
alert, and pull requests then fail only on alerts they introduce. Code scanning
needs `security-events: write`, is free on public repositories, and requires
GitHub Code Security on private ones; the `push` trigger keeps the default
branch analyzed so pull-request comparisons have a baseline, and each upload in
one run needs its own `category`. `mode: all` establishes or checks a static
baseline; it does not replace `mode: base`, because a single valid comment can
only be checked for drift by comparing revisions. Push and manual workflows
must likewise pass an explicit before/after range for drift checks, or
deliberately use `mode: all` when the job exists only to establish a new
baseline.

Stable `0.x` releases advance `@v0` only after packaged-Action E2E succeeds.
Consumers do not need an exact Action commit SHA.

## Release

After updating the package version, releases are manual workflow dispatches from
`main`; the workflow publishes the crate and creates the tag only after
authorization, artifact builds, and hosting succeed:

```console
version=$(cargo metadata --locked --no-deps --format-version 1 |
  jq -er '.packages[] | select(.name == "fuck-ai-comments") | .version')
gh workflow run release.yml --ref main -f "tag=v${version}"
```

Before an automated dispatch, protect `main` and create a `release-authorization`
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

crates.io requires a crate to exist before its Trusted Publisher can be
configured. Version `0.1.0` was bootstrapped from a trusted workstation with a
local owner token. For a new crate name, first run `cargo publish --dry-run
--locked`, publish once with `cargo publish --locked`, and never copy that token
into GitHub. Then configure a crates.io Trusted Publisher for repository owner
`IvanWng97`, repository `fuck-ai-comments`, workflow filename `release.yml`, and
environment `release-authorization`. Subsequent publish jobs use that protected
environment and an OIDC-issued short-lived token; they do not require a stored
crates.io API token.

Published releases provide native archives for x86-64 Linux, x86-64 Windows,
x86-64 macOS, and Apple Silicon macOS. Each release includes `sha256.sum`, a
dist manifest, and GitHub build attestations. Every native archive carries the
project `LICENSE`, `README.md`, and generated `THIRD_PARTY_LICENSES` notices.
The packaged Action must then pass on x86-64 Linux and Windows, plus x86-64
and Apple Silicon macOS, before a stable `0.x` release advances `v0`;
prereleases run the same checks without changing the compatibility tag.

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
- ICU4X supplies the Unicode `Default_Ignorable_Code_Point` property used by
  attestation normalization;
- `imara-diff` supplies fixed-Myers line and comment anchors through one
  interned-input and hunk pipeline;
- `ignore` supplies repository walking plus both file-based and configured
  gitignore semantics;
- `serde_json` supplies the versioned integration report encoding;
- Cargo metadata supplies workspace membership and Rust library target roots;
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
`analyze_all`; every `change_*_10k_loc_per_snapshot` case compares two
10,000-line snapshots through `analyze_change`. The newline-dense Rust change
case exercises sparse anchor retention, while the adversarial Rust case
exercises Myers on reordered unique lines. The suite covers every adapter
family, including separate
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
