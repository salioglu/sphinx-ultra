# Implementation Status

**Audit-verified status as of 2026-08-06** (v0.3.0 + the 2026-08 pattern/path fixes).
Method: every status below was established by tracing call graphs from the binary's
entry point (`src/main.rs` → `SphinxBuilder::build`), running the built binary
against fixture projects, and — for compatibility claims — differential comparison
against real Sphinx 9.1.0. Statuses describe **what `sphinx-ultra build` actually
executes**, not what modules exist.

Status legend:

- ✅ **working** — implemented and exercised by the binary's execution path
- 🟡 **partial** — some paths work; documented gaps
- 🧩 **built-not-wired** — real, tested library code with **zero call sites** in the
  build path (runs only from `examples/` or unit tests)
- 🔴 **stub** — placeholder that does nothing useful
- ❌ **broken** — exists but incorrect (verified)
- ⬜ **missing** — not implemented

The plan to move everything to ✅ is [ROADMAP.md](../ROADMAP.md).

## Core build pipeline

| Feature | Status | Evidence / gaps |
|---|---|---|
| File discovery w/ include/exclude patterns | ✅ (one divergence) | `src/builder.rs` `discover_source_files`, `src/matching.rs`. One verified remaining divergence vs Sphinx 9.1: `**` translates to `(?:[^/]+/)*` where Sphinx uses `.*` (so `**/index.rst` matches `index.rst` here but not in Sphinx) — differential parity suite is ROADMAP M1. Fixed 2026-08: `[!…]` → `[^/…]` (negated classes never match `/`), leading `^` now literal, Sphinx-parity directory pruning. |
| Parallel orchestration | ✅ | rayon pool sized by `-j`/config. Gap: one file error aborts the whole build (`builder.rs` collect); per-file error collection is M1. |
| Incremental cache | ❌ | Sound staleness check (blake3), but a cache hit returns **before writing output**, and `--clean --incremental` yields an output dir with no HTML for cached docs (cache loaded pre-clean). `max_cache_size_mb`/`cache_expiration_hours` config never plumbed (hardcoded 500MB/24h); eviction is access-count (not LRU as documented). |
| Dependency graph | 🔴 | `build_dependency_graph` returns empty vecs (TODO) and its result is ignored. No include/toctree-driven invalidation. |
| RST parsing | ❌ | Line-scanner (`src/parser.rs`): directive regex `\w+` misses hyphenated names — **`code-block` is not recognized**; options require exactly-3-space indent; content dedent `&line[3..]` can panic on short tab-indented lines; hardcoded underline→level map breaks `=`-titled docs (title becomes "Untitled"); no inline markup, lists, tables, footnotes, substitutions, comments, targets, transitions; the `::` literal-block branch drops the introducing paragraph. |
| Markdown parsing | ❌ | Only `Event::Text` survives pulldown-cmark; headings/code/lists/tables discarded; `.md` titles/TOCs always empty; front matter TODO. |
| HTML rendering | 🔴 | `builder.rs` "Simple document rendering (placeholder)": output is `<html><body>{escaped raw source}</body></html>`. `DocumentContent::Display` returns the raw source. No AST rendering, layout, navigation, or asset links. |
| Toctree validation (missing refs, orphans) | 🟡 | The only validation that runs. False positives on captions, `Title <doc>` entries, `:glob:`, and subdirectory-relative refs; warning line numbers hardcoded to `10`; orphan check uses a path-prefix heuristic. |
| Warning pipeline (`-W`, `-w`) | 🟡 | Works; only two warning types are ever emitted. |
| Error pipeline | 🔴 | `BuildErrorReport` plumbing exists end-to-end but nothing ever pushes an error; **builds with errors exit 0** (only `-W`+warnings exits 1). |
| Static asset copying | 🟡 | Copies 5 handwritten shim files (incl. a 61-line fake jquery.js) + project `_static`/`_templates`; generated pages reference none of them; `html_static_path` ignored by the live path. |
| Search index / genindex / objects.inv emission | 🔴 | `generate_search_index`/`generate_indices` are TODO no-ops. No searchindex.js, genindex.html, or objects.inv in output (verified empirically). |
| Extension loading | 🔴 | Loading any extension fabricates a stub record and prints one line. Zero behavioral effect. (The never-used pyo3 dependency was removed 2026-08; Python interop arrives as a sidecar in ROADMAP M5.) |
| Build stats | 🟡 | `files_skipped` hardcoded 0; `errors` always 0; cache hits only counted under `--incremental`. |
| `clean` / `stats` commands | ✅ | `stats` cross-ref count is naive substring counting. |

## Built-but-not-wired stack (the "second codebase")

These are real modules with passing tests, exported from `lib.rs`, with **no call
sites in the binary** — they run only from `examples/` and unit tests:

| Module | Status | Notes |
|---|---|---|
| `html_builder.rs` (Sphinx `StandaloneHTMLBuilder` mirror, 800 lines) | 🧩 | Internally placeholder-grade even if wired: doc titles TODO, empty local TOC, empty search dump, `.buildinfo` in wrong format. |
| `template.rs` (minijinja engine + templates/) | 🧩 | User `templates_path` loading commented out ("lifetime issues"); `toctree()` returns an empty div; `pathto` ignores page depth; genindex/search templates use Python-only constructs (unregistered `_()`, `count.append(count.pop()+1)`) that fail at render time. |
| `search.rs` (in-memory index) | 🧩 | Output format is not Sphinx's `Search.setIndex` schema; 3-rule stemmer; title weighting inert. |
| `inventory.rs` (objects.inv) | 🧩 / ❌ | Writer plausible; **reader corrupts real inventories** (lossy UTF-8 conversion + line-splitting over binary zlib bytes). |
| `environment.rs` (BuildEnvironment) | 🧩 | Never constructed in the binary; `collect_relations` returns empty TODO. |
| `domains/` (Python + RST domain validation) | 🧩 / ❌ | **Reference parser inverts target/display-text for `` :doc:`Title <target>` `` (its own test locks in the wrong order)**; external-ref detection is a hardcoded stdlib prefix whitelist; duplicate labels silently overwrite. |
| `directives/validation/` (10+10 validators) | 🧩 | Heuristic rules that false-positive on valid Sphinx (e.g. `.. note:: inline text` triggers both an arguments warning and a missing-content error). |
| `validation/` (constraint engine) | 🧩 / ❌ | Expression evaluator supports only `==`/`!=`/`in list`/`and`/`or`/`not`; **memory-unsound `'static` transmute in the template cache** (use-after-free hazard); trait impls are placeholders; no way to declare constraints in any config file. |
| `directives.rs` (HTML processor registry) | 🧩 | ~40 processors registered, 28 are stubs emitting HTML comments; `process_directive` has zero call sites; name-collides with the validation `DirectiveRegistry`. |
| `roles.rs` | ⬜ | **Not declared in any module tree — never compiled** (orphaned source file). Ironically its `text <target>` parsing is correct, unlike the compiled domains parser. |

## Configuration

| Feature | Status | Evidence / gaps |
|---|---|---|
| conf.py parsing | 🟡 | Line-scanner for single-line assignments (self-described stub). Multi-line lists (the normal style for `extensions`/`exclude_patterns`) silently dropped — verified: a multi-line `exclude_patterns` leaves excluded files in the build. Dicts never parse (so `html_theme_options` is always empty). No warnings for dropped config. Half of `ConfPyConfig` (latex_*/epub_*/source_suffix/nitpick_*…) is declared but never populated. |
| YAML/JSON config | ❌ | No serde defaults → every field required. **Both YAML files shipped in this repo fail to load** ("missing field"), incl. `examples/basic/sphinx-ultra.yaml`. |
| Config auto-detection order | ✅ | conf.py → yaml → yml → json → default. |
| `--config` flag | 🟡 | Cannot point at a conf.py (YAML/JSON only) — inconsistent with auto-detect. |
| Config knobs actually consumed | ❌ | `html_theme`, `theme.*`, `output.syntax_highlighting`/`highlight_theme`/`minify_html`/`search_index`, `optimization.*`, `max_cache_size_mb`, `cache_expiration_hours`, `html_static_path` are parsed and then **never read by any consumer** — configuration is largely decorative today. |

## CLI vs sphinx-build

| Capability | Status |
|---|---|
| `build --source/--output`, `-j`, `--clean`, `--incremental`, `-W`, `-w` | ✅ (relative `--source` crash fixed 2026-08) |
| Positional `SOURCEDIR OUTPUTDIR`, `-b`, `-M`, `-D`, `-A`, `-n`, `-q`, `-E`, `-a`, `-c`, `-t`, `-T`, `--keep-going`, `-j auto` | ⬜ (ROADMAP M1) |
| Non-zero exit on build errors | ❌ (errors exit 0 today; M1) |
| `--verbose` position | 🟡 global flag only before the subcommand; `RUST_LOG` is overwritten at startup |
| `serve` (advertised by dev.sh/build.sh) | ⬜ does not exist (ROADMAP M3) |

## Infrastructure & release

| Area | Status | Notes |
|---|---|---|
| CI (fmt, clippy -D warnings, tests, audit, coverage, 3-OS) | ✅ | No MSRV job; `--all-features` is vacuous; `integration_test.rs` is 100% commented out yet runs as a green CI step. |
| E2E tests of the binary | ⬜ | Absent — which is how the relative-path crash and unloadable YAML examples shipped. ROADMAP M1. |
| Cargo.lock | ❌ | Gitignored for a binary crate → non-reproducible CI/releases; cache keys hash nothing. |
| Release workflow | 🟡 | Solid tag/version validation, but `publish-crate` has no `needs:` gate (can publish before validation/builds); no checksums; no aarch64-linux artifact despite install.sh advertising one. |
| pyo3/pythonize | ✅ removed (2026-08) | Had zero call sites while linking libpython into every build (two RUSTSEC advisories, broken musl target, undocumented Python build dependency). Python interop returns as a venv **sidecar process** in ROADMAP M5 — not as a link-time dependency. |
| Unused dependencies | ✅ pruned (2026-08) | Removed: pyo3, pythonize, syntect, cssparser, minifier, tar, bincode, crossbeam, lru, config, glob, walkdir, indexmap, toml, ini, handlebars. syntect returns when highlighting is actually wired (M2/M3). |
| Repo hygiene | 🟡 | `Cargo.toml.new` / `Cargo.lock.template` / `.packagename` are scaffold leftovers that ship inside the crates.io package; the useful metadata (`rust-version`, keywords, categories, exclude) lives only in the dead `Cargo.toml.new`. CHANGELOG has no 0.2.0/0.3.0 entries. SECURITY.md describes subsystems that do not exist. |

## Testing status

| Suite | Status |
|---|---|
| Unit tests (lib) | ✅ 74 passing |
| Pattern compatibility tests | ✅ 10 passing — but several assert the **divergent** (non-Sphinx) `**` semantics while labeled "from Sphinx documentation"; differential regeneration is M1 |
| `tests/integration_test.rs` | ❌ 0 tests — entire file commented out ("disabled to avoid compilation errors") |
| E2E CLI tests | ⬜ none |
| Benchmarks | 🟡 exercise the placeholder pipeline (numbers measure escaped-text copying); cache benchmark is `black_box(42)` |

## Historical note

Previous versions of this document (and the README/VALIDATION_FEATURES_PLAN) marked
the domain system, directive/role validation, and constraint engine as "Fully
Implemented ✅". That was true of the *library code and its unit tests* but not of
the product: none of the three systems has ever been invoked by `sphinx-ultra build`.
This document now tracks binary-reachable behavior only; wiring those systems into
the build is ROADMAP M1.
