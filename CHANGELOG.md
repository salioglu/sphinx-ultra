# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Implementation reality per subsystem lives in
[docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md); the plan to move
everything forward is [ROADMAP.md](ROADMAP.md).

## [Unreleased]

### Added

- **M2 wave 4: the build has a real environment, and it warns like Sphinx.**
  The pipeline is now read → merge → resolve → write over a serialized
  `BuildEnvironment`, and the diagnostics that come out of it are
  Sphinx's own — same texts, same locations, same `[category]` suffixes.
  What that means in practice, per subsystem:
  - **toctree**: the global graph, relations (parents/prev/next) and the
    consistency checks — nonexisting vs excluded entries, self-referencing
    toctrees, circular toctrees, a document reached from several toctrees
    (an *information* notice, not a warning, so it does not fail `-W`), and
    `document isn't included in any toctree`.
  - **numbering**: `numfig`, `numfig_secnum_depth` and `numfig_format` are
    honored; `:numref:` resolves to real numbers, with Sphinx's
    `numfig is disabled. :numref: is ignored.` and `no number is assigned
    for …` warnings.
  - **std domain**: labels, glossary terms, `option`s (with `program`
    scoping and unscoped fallback), `envvar`s and `confval`s are collected
    and resolved for `:ref:`/`:numref:`/`:doc:`/`:term:`/`:option:`/
    `:envvar:`, with `duplicate label`, `undefined label:`,
    `unknown document:`, `term not in glossary:` and `unknown option:`
    warnings. `nitpick_ignore` and `nitpick_ignore_regex` are honored.
  - **std directives**: `program`, `option` (incl. `[=value]` and
    comma-separated names), `envvar`, `confval` (`:type:`/`:default:`),
    `describe`/`object` and `default-domain`, on a generic
    object-description anatomy with the `:no-index:` option family.
  - **general index**: `index` directives and roles are collected and
    assembled into the grouped, sorted structure `genindex.html` renders —
    single/pair/triple/see/seealso, `!main`, Symbols grouping. **No
    `genindex.html` is written yet**; the page needs the HTML writer.
  - **objects.inv**: a byte-correct reader and writer, verified against
    inventories a real `sphinx-build` produced. **Nothing writes an
    `objects.inv` into your output yet** — the writer is waiting on the
    HTML writer's finish task. The reader is live, because:
  - **intersphinx**: `intersphinx_mapping` (named and unnamed), inventory
    loading with the on-disk cache, `intersphinx_disabled_reftypes`, the
    `:external:`/`:external+inv:` roles, and the shared HTTP settings
    (`tls_verify`, `tls_cacerts`, `user_agent`, `intersphinx_timeout`).
    Cross-project references resolve.
  - **incremental builds**: a document is now rebuilt when a file it
    depends on changes, not only when its own source does. Today that
    means images; `include`/`literalinclude` follow in wave 4.5.
  Evidence: an environment-layer differential oracle builds 15
  multi-document projects (47 documents) with a real `sphinx-build` 9.1.0
  and compares the toctree graph, relations, numbering, std registries,
  index data, the whole warning stream and every document's resolved
  doctree — zero divergence on every compared key, cold and warm. The
  exemptions that remain are listed in `tests/env_differential.rs` and
  summarized in [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md).
- New configuration knobs, readable from `conf.py`, YAML/JSON and `-D`:
  `numfig`, `numfig_format`, `numfig_secnum_depth`, `nitpick_ignore`,
  `nitpick_ignore_regex`, `intersphinx_mapping`,
  `intersphinx_disabled_reftypes`, `intersphinx_resolve_self`,
  `intersphinx_cache_limit`, `intersphinx_timeout`, `tls_verify`,
  `tls_cacerts`, `user_agent`. Malformed `intersphinx_mapping` entries
  fail with Sphinx's own `ConfigError` messages.

### Added (continued: earlier M2 waves and M1 follow-ups)

- **M2 wave 3: the docutils-fidelity parser is now THE parser.**
  `Parser::parse` runs `src/rst/` (sphinx mode) and derives the whole
  `Document` from the doctree — title, toc with docutils `make_id`
  anchors, explicit-target labels, toctree entries with real per-entry
  lines, and directive/role records that feed the validation and
  nitpicky passes. The M1 line-scanner and the three raw-source
  re-scanners in the builder are gone; the 39-test e2e warning/exit-code
  surface is byte-preserved.
- M2 wave 3 directive machinery (docutils-exact): argument/option/content
  extraction with typed option converters and docutils-verbatim error
  texts, content re-parsing/nesting, unknown-directive shapes, and the
  full docutils built-in set — admonitions (incl. generic), topic,
  sidebar, rubric, epigraph/highlights/pull-quote, compound, container,
  parsed-literal, image, figure, code, math, raw, line-block, class
  (pending node), table/csv-table/list-table — plus substitution
  definitions (`replace::`/`unicode::`/`date::` and embedded directives,
  duplicate dupname semantics). Docutils differential fixture: 653 cases,
  zero divergence.
- M2 wave 3 Sphinx set against a second, real-Sphinx oracle
  (`tools/gen_sphinx_fixture.py`, 277 cases at zero divergence vs a
  sphinx-build 9.1.0 read phase): toctree, versionadded/versionchanged/
  deprecated/versionremoved, seealso, code-block/sourcecode + highlight,
  only, rst-class, math (labels + equation targets), index directive,
  hlist, glossary, xref roles (`:doc:`/`:ref:`/py-domain pending_xref
  anatomy), and pep/rfc/cve/cwe index-emitting external links.
  Deliberate deferrals (literalinclude/include, object descriptions,
  ifconfig, meta, rst_prolog/epilog/default_role) are recorded in the
  wave notes.
- M2 wave 2 (library-only): the docutils inline parser — emphasis/strong/
  literal, all reference forms (named/phrase/anonymous/embedded with inline
  targets), built-in interpreted-text roles (incl. PEP/RFC references),
  footnote/citation/substitution references, standalone URIs and emails,
  docutils escape semantics — plus footnote and citation definitions, field
  lists, full option lists, and grid + simple tables with docutils-exact
  error recovery. The differential fixture now covers 426 cases at zero
  divergence against docutils 0.22.4.
- M2 wave 1 (library-only, not yet wired into the build): typed doctree IR
  with docutils-equivalent node semantics and source spans (`src/doctree/`),
  and a docutils-fidelity recursive-descent RST **block** parser
  (`src/rst/`) covering sections, transitions, bullet/enumerated/definition
  lists, block quotes with attribution, literal/doctest/line blocks,
  comments, and hyperlink targets — byte-identical pseudo-XML against
  docutils 0.22.4 across a committed 175-case differential fixture
  (`tests/doctree_differential.rs`), plus a proptest totality suite.
  The binary's behavior is unchanged; the new parser replaces the
  line-scanner in M2 wave 3.

### Changed

- **Broken `:doc:` and `:ref:` references now warn without `-n`.**
  Sphinx sets `warn_dangling` on exactly those two reftypes, so it reports
  `unknown document: '…'` and `undefined label: '…'` in a default build and
  reserves `-n`/`nitpicky` for the rest. Previous releases only reported
  them under `-n`. **This can turn a passing `-W` build into a failing
  one** for a project that has broken `:doc:`/`:ref:` targets and was
  relying on the old, quieter default.
- **Toctree warnings moved to the `.. toctree::` directive line** and now
  carry Sphinx's warning category. Where a missing entry previously
  reported at the entry's own line and bare, it now reports at the
  directive's line with a ` [toc.not_readable]` suffix — matching
  `sphinx-build`, whose toctree warnings are logged against the directive
  node. Warning *categories* (`show_warning_types`, on by default since
  Sphinx 8.3) are now emitted generally, so other warnings gain a
  ` [type.subtype]` suffix too. **Anything that greps or diffs build
  output will see different lines**, and a `-w` warning file is not
  byte-comparable with one from 0.4.x.

### Removed

- The M1 domain system (`sphinx_ultra::domains`, and with it the crate-root
  re-exports `CrossReference`, `DomainObject`, `DomainRegistry`,
  `DomainValidator` and `ReferenceType`). It was a regex reference scanner
  with fuzzy suggestions; the std domain and the real resolution pass
  replaced its whole live surface, after which it had no call sites.
  Library consumers that imported those names have no drop-in replacement
  yet — the new API is `sphinx_ultra::env`. (`document::CrossReference` is
  a different type and still exists.)

### Internal

- Persisted doctrees now carry a magic + format-version header. bincode has
  no self-description, so a doctree written by an older build used to
  decode *successfully* into a plausible-but-wrong tree; a mismatched
  version is now an honest cache miss. Practical effect when upgrading:
  the first build after this change re-reads every document once, then
  caches normally.

### Fixed

- **The `objects.inv` reader corrupted real inventories.** It converted the
  zlib-compressed payload to a `String` lossily and then split it with
  `str::lines`, so any inventory whose compressed bytes happened to contain
  a bare `\r`/`\n` or a non-UTF-8 sequence — which content-rich inventories
  routinely do — lost or mangled entries. The reader is now binary-safe end
  to end, handles v1 and v2, expands `$` anchors and `-` display names, and
  reproduces Sphinx's own `ValueError` texts for malformed files. This was
  unreachable from `sphinx-ultra build` before now (nothing consumed an
  inventory), so it bit only direct users of the `sphinx_ultra::inventory`
  API — but intersphinx consumes it as of this release, so it had to be
  right first.
- `install.sh` no longer prefixes archive names with the tag's `v`
  (`sphinx-ultra-v0.4.0-...` 404'd; assets are named `sphinx-ultra-0.4.0-...`
  — broken for every release since checksums were introduced)

## [0.4.0] - 2026-08-07

### Added

- **sphinx-build compatible argument mode**: `sphinx-ultra SOURCEDIR OUTPUTDIR`
  with `-b html`, `-M html`/`-M clean` make-mode (output under `OUTPUTDIR/html`),
  `-D key=value` / `-A name=value` overrides, `-d doctreedir`, `-n`, `-q`, `-E`,
  `-a`, `-T`, `-t tag`, `-c confdir`, `-j N|auto`, `-W`/`--keep-going`/`-w`,
  repeatable `-v`. Parity (exit codes, output layout, message shapes) measured
  against real sphinx-build 9.1.0; incremental by default like sphinx-build
- **Directive/role validation runs in every build** (`validate_directives`
  config knob, default on): findings surface as warnings with file:line through
  the standard `-W`/`-w` pipeline; unknown directives/roles stay silent
- **Nitpicky cross-reference validation** (`-n` / `nitpicky`): `:doc:`/`:ref:`
  resolve against built documents, explicit `.. _label:` targets, and section
  anchors; broken refs warn `unknown document:` / `undefined label:` with line
  numbers
- Generated pattern differential suite: 881 committed cases verified against
  `sphinx.util.matching` 9.1.0 (`tools/gen_pattern_fixture.py` regenerates)
- `-D` overrides work on every config field with typed coercion, dotted paths
  for nested sections, and sphinx-build's warn-and-ignore for unknown keys

- End-to-end CLI test harness: the binary now runs against fixture projects in CI,
  asserting exit codes, warnings, and output trees (replaces the fully
  commented-out `integration_test.rs`)
- MSRV declared (`rust-version = "1.85"`) and verified by a dedicated CI job
- Release artifacts now ship SHA-256 checksums, and `install.sh` verifies them
- `linux-aarch64` release artifact (previously advertised by
  `install.sh` but never built)
- `--config` now accepts a `conf.py` path (previously YAML/JSON only)
- Crate metadata for crates.io: `keywords`, `categories`, `documentation`,
  `exclude`

### Changed

- **`**` glob semantics now match Sphinx 9.1 exactly** (breaking for patterns
  relying on the old gitignore-style behavior): `**` translates to `.*` with no
  directory-boundary special case, so `**/index.rst` no longer matches a
  top-level `index.rst` and `foo/**/bar` requires at least one intermediate
  component — exactly like `sphinx-build`. Character-class emission (incl.
  backslash doubling) is byte-identical to Sphinx's `_translate_pattern`
- A pre-set `RUST_LOG` is respected (it was previously overwritten on every
  run); `-v`/`-q` only set the default filter
- Deleted the orphaned `src/roles.rs` (never part of the module tree), the
  `Parser`'s never-called directive-processor registry, and the constraint
  engine's always-success placeholder trait impls (they shadowed the real
  `validate_constraint` under auto-ref and would have made future wiring
  silently validate nothing)

### Fixed

- **Validation false positives on valid Sphinx**: `.. note:: inline text` is
  content, not "arguments" (was both an arguments warning and a
  missing-content error); bare `.. code-block::`, spaces/uppercase in `:ref:`
  labels, relative `:doc:` paths, image lengths without units, and arbitrary
  kbd/menuselection styles are all accepted now
- **Incremental cache overhauled**: warm-cache rebuilds no longer deadlock
  (every second `--incremental` run previously hung forever); cache hits
  write the rendered page to the output tree; `--clean --incremental`
  produces a complete build; `max_cache_size_mb`/`cache_expiration_hours`
  are honored (previously hardcoded); any config change invalidates the
  cache; eviction renamed to match its actual least-accessed policy
- **conf.py parsing rewritten** for the declarative subset: multi-line
  lists/dicts/tuples, nested literals, adjacent string concatenation, and
  triple-quoted strings now parse (multi-line `extensions`/
  `exclude_patterns` — the normal style — previously dropped silently);
  every construct the parser cannot handle now warns with its
  `conf.py:line`
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

- Release artifacts renamed from Rust target triples to `os-arch`
  (`linux-x86_64`, `linux-x86_64-musl`, `linux-aarch64`, `macos-x86_64`,
  `macos-aarch64`, `windows-x86_64`); `install.sh` detects the new names
- The musl artifact is built with `cross` (container-pinned musl
  toolchain) after host `musl-gcc` linking broke twice from runner-image
  drift
- `scripts/release.sh` now syncs `Cargo.lock` with the bumped version, and
  the release workflow fails fast on a stale lockfile (the v0.4.0 first
  cut failed every `--locked` build this way)
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
