# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Implementation reality per subsystem lives in
[docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md); the plan to move
everything forward is [ROADMAP.md](ROADMAP.md).

## [Unreleased]

### Added

- End-to-end CLI test harness: the binary now runs against fixture projects in CI,
  asserting exit codes, warnings, and output trees (replaces the fully
  commented-out `integration_test.rs`)
- MSRV declared (`rust-version = "1.85"`) and verified by a dedicated CI job
- Release artifacts now ship SHA-256 checksums, and `install.sh` verifies them
- `aarch64-unknown-linux-gnu` release artifact (previously advertised by
  `install.sh` but never built)
- `--config` now accepts a `conf.py` path (previously YAML/JSON only)
- Crate metadata for crates.io: `keywords`, `categories`, `documentation`,
  `exclude`

### Fixed

- **Builds with errors now exit 1** (sphinx-build parity); per-file failures
  are reported as errors while the rest of the build continues (previously the
  first failing file aborted the whole build, and error exits were 0)
- Toctree warnings carry the entry's real line number (previously hardcoded
  to 10) and follow Sphinx resolution semantics: document-relative and
  `/`-absolute targets, `Title <target>` entries, external URLs, `self`, and
  `:glob:` patterns (dead globs get Sphinx's "didn't match any documents"
  warning) — eliminating the caption/`Title <doc>`/glob/relative-path false
  positives
- RST parser crash class: hyphenated and domain directive names
  (`code-block`, `py:function`) are recognized, tab-indented directive content
  no longer hits a byte-slicing panic path, and section levels follow
  docutils' order-of-first-use rule (so `=`-underlined titles are no longer
  "Untitled")
- Reference parser: `` :doc:`Title <target>` `` now resolves the
  angle-bracket target (target and display text were inverted)
- Constraint engine: removed a memory-unsound `'static` transmute in the
  template cache; compiled templates are now owned by the minijinja
  environment
- Partial YAML/JSON configs now load: all `BuildConfig` fields have serde
  defaults (previously every field was required, and both YAML examples shipped
  in this repo failed to load)
- `install.sh` no longer corrupts captured values with log output (logs now go
  to stderr) and fails cleanly on download errors (`curl -f`)
- Source paths canonicalized so relative `--source` values (including the
  default `.`) no longer crash the build *(2026-08)*
- Sphinx-parity pattern semantics: `[!…]` character classes, literal leading
  `^`, and directory pruning *(2026-08)*

### Changed

- `Cargo.lock` is committed; CI and releases build with `--locked`
  (reproducible builds)
- crates.io publishing is gated on version validation and release builds
  succeeding
- Removed `pyo3`/`pythonize` (zero call sites, two RUSTSEC advisories, linked
  libpython into every build) and 14 other unused dependencies *(2026-08)*;
  Python interop returns as a sidecar process (ROADMAP M5)
- Removed references to the not-yet-implemented `serve` command from dev
  scripts (planned for ROADMAP M3)
- Deleted scaffold leftovers: `Cargo.toml.new`, `Cargo.lock.template`,
  `.packagename`

## [0.3.0] - 2025-10-13

### Added

- Sphinx-style `include_patterns`/`exclude_patterns` file discovery with a
  pattern-translation engine and compatibility test suite
- Directive & role validation system (library): validators for common RST
  directives and roles with severity levels *(library-only in this release;
  not yet invoked by `sphinx-ultra build` — wiring is ROADMAP M1)*

### Fixed

- Granular GitHub token permissions in workflows (code-scanning alert)

## [0.2.1] - 2025-10-13

### Added

- Domain system & cross-reference validation (library): pluggable domain
  architecture with Python (`:func:`, `:class:`, …) and RST (`:doc:`, `:ref:`,
  `:numref:`) domains, reference parser, fuzzy suggestions for broken
  references *(library-only in this release; not yet invoked by
  `sphinx-ultra build`)*

## [0.2.0] - 2025-10-13

### Added

- Constraint validation system inspired by sphinx-needs (library): expression
  evaluator (`==`, `!=`, `in`, `and`, `or`, `not`), severity-based failure
  actions, template-based messages *(library-only in this release)*
- musl release targets and release-script publishing instructions

### Changed

- Dependency updates (dependabot: production dependencies, actions/cache 4,
  action-gh-release 2)

## [0.1.0] - 2025-09-07

### Added

- Initial project setup: parallel build pipeline (rayon), incremental cache
  with blake3 change detection, RST/Markdown line-scanning parsers, CLI
  (`build`/`clean`/`stats`) with `-W`/`-w` warning handling, toctree
  missing-reference and orphan checks, configuration auto-detection
  (conf.py subset → YAML → JSON → defaults)
