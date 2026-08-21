# Changelog

All notable changes to `fuck-ai-comments` are documented here. The project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/IvanWng97/fuck-ai-comments/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/IvanWng97/fuck-ai-comments/releases/tag/v0.1.0
