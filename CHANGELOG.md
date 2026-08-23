# Changelog

All notable changes to `fuck-ai-comments` are documented here. The project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.0] - 2026-08-23

### Added

- `check --format sarif` writes a SARIF 2.1.0 report with the full rule
  registry, so findings upload to GitHub code scanning through
  `github/codeql-action/upload-sarif` and pull requests surface only newly
  introduced alerts.
- The `sarif-file` Action input writes that report into the workspace and
  exposes its path as the `sarif-file` output.
- `fuck_ai_comments::rules` publishes stable rule identifiers with the
  metadata shipped in machine-readable reports.

### Changed

- The Action no longer parses the JSON report or emits its own annotations; it
  streams the text report under a GitHub problem matcher, so annotations follow
  the platform's per-step limits and the complete finding list stays in the
  step log.

### Fixed

- Rust struct, union, and tuple fields and enum variants are now member
  owners with independent comment budgets, so per-field and per-variant
  documentation no longer aggregates onto the enclosing type (#41).

## [0.2.0] - 2026-08-20

### Changed

- Replaced configuration schema v1 with the breaking, language-independent v2
  taxonomy: `narrative`, `documentation`, `public-documentation`,
  `safety-proof`, and `tool-directive`.
- Split comment classification into independent semantic-role and attachment
  dimensions so language adapters no longer create combined variants such as
  file docstrings or file rustdoc.

### Added

- Structural documentation recognition for attached JSDoc/TSDoc, KDoc, Swift
  documentation comments, and Objective-C Doxygen comments.
- Explicit `owner-capped` configuration and zero-line caps for repositories
  that want to forbid a semantic comment category completely.

## [0.1.3] - 2026-08-20

### Added

- Independent `comments.docstring` policy for Python function, class, and
  module docstrings.
- Gitignore-style top-level `exclude` patterns shared by all scan modes and
  loaded from the same source authority as the rest of the configuration.
- Versioned `--format json` CLI reports and source-line GitHub Action
  annotations with complete folded logs.
- A concise README quick start and this changelog.

### Fixed

- Rust structs, enums, unions, traits, type aliases, and implementation blocks
  now own their documentation budgets instead of aggregating at file scope
  ([#35](https://github.com/IvanWng97/fuck-ai-comments/issues/35)).
- Narrowed `check --all PATH` scans now retain repository-root configuration
  discovery and reject redirected Git authority
  ([#36](https://github.com/IvanWng97/fuck-ai-comments/issues/36)).

## [0.1.2] - 2026-08-20

### Added

- Strict, versioned repository policy configuration for narrative, rustdoc,
  safety-proof, and tool-directive comments.

### Fixed

- Internal Rust documentation is classified as rustdoc independently of item
  visibility ([#32](https://github.com/IvanWng97/fuck-ai-comments/issues/32)).

## [0.1.1] - 2026-08-19

### Added

- Cargo-authoritative custom Rust library-root discovery.
- Trusted crates.io publishing and the moving `v0` GitHub Action compatibility
  tag.

## [0.1.0] - 2026-08-19

- Initial owner-aware comment budgets, stale/reparented comment attestation,
  fail-closed Git modes, multi-language analyzers, native release archives, and
  the GitHub Action.

[Unreleased]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/IvanWng97/fuck-ai-comments/releases/tag/v0.1.0
