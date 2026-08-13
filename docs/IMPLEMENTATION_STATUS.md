# Implementation Status

**Audit-verified status as of 2026-08-13** (v0.4.0 + M2 waves 1–3).
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
| File discovery w/ include/exclude patterns | ✅ (differentially verified) | `src/builder.rs` `discover_source_files`, `src/matching.rs`. `**` now translates to `.*` exactly like Sphinx 9.1 (wave 4); character-class emission is byte-identical to `sphinx.util.matching._translate_pattern` (incl. backslash doubling, `[]a]`/`[!]a]` edge cases). Verified by a committed 881-case differential fixture generated against sphinx 9.1.0 (`tools/gen_pattern_fixture.py`, `tests/pattern_differential.rs`) — zero divergence. Discovery keeps `include_patterns=['**']` and suffix-filters after matching, like Sphinx's `Project.discover`. Earlier 2026-08 fixes: `[!…]` → `[^/…]`, literal leading `^`, directory pruning. |
| Parallel orchestration | ✅ | rayon pool sized by `-j`/config. Per-file failures become `BuildErrorReport`s and the build continues (2026-08). |
| Incremental cache | ✅ | Fixed 2026-08: warm-cache rebuilds no longer deadlock (DashMap guard held across `alter` — found by the new E2E suite); hits write the rendered page; `--clean --incremental` produces a full tree (clean clears the cache); `max_cache_size_mb`/`cache_expiration_hours` plumbed; config changes invalidate via blake3 fingerprint; eviction honestly named least-accessed (LFU-style). |
| Dependency graph | 🔴 | `build_dependency_graph` returns empty vecs (TODO) and its result is ignored. No include/toctree-driven invalidation. |
| RST parsing | ✅ (docutils-fidelity, wired) | **M2 wave 3 (2026-08-13): the binary runs the new parser** — `Parser::parse` → `src/rst/parse_rst_full` (sphinx mode); the M1 line-scanner is deleted. `src/doctree/` generic-node IR with byte-parity `pformat`; `src/rst/` block + inline grammar (waves 1–2) plus docutils-exact directive machinery (typed option converters with docutils-verbatim error texts, options-before-arguments evaluation order, rawsource literals, as-written names), the docutils built-in directive set (admonitions/topic/sidebar/rubric/quote-family/compound/container/parsed-literal/image/figure/code/math/raw/line-block/class/table/csv-table/list-table) and substitution definitions (replace/unicode/date, embedded directives, duplicate dupname semantics). Zero divergence on a committed 653-case fixture vs docutils 0.22.4 parse layer (`tests/doctree_differential.rs`). Sphinx-mode set (toctree, versionmodified family, seealso, code-block/sourcecode + highlight state, only, rst-class, math + equation targets, index, hlist, glossary, xref pending_xref anatomy, pep/rfc/cve/cwe) verified against a real sphinx-build 9.1.0 read-phase oracle: 277 cases, zero divergence (`tests/sphinx_doctree_differential.rs`). Document now derives title/toc (docutils `make_id` anchors)/labels/toctree entries/directive+role records from the doctree; the three builder raw-source re-scanners are gone. Deferrals recorded in wave notes: literalinclude/include, object descriptions, ifconfig, meta, rst_prolog/epilog/default_role. |
| Markdown parsing | ❌ | Only `Event::Text` survives pulldown-cmark; headings/code/lists/tables discarded; `.md` titles/TOCs always empty; front matter TODO. |
| HTML rendering | 🔴 | `builder.rs` "Simple document rendering (placeholder)": output is `<html><body>{escaped raw source}</body></html>`. `DocumentContent::Display` returns the raw source. No AST rendering, layout, navigation, or asset links. |
| Toctree validation (missing refs, orphans) | ✅ | Real per-entry line numbers; Sphinx docname resolution (document-relative, `/`-absolute, `.`/`..`); `Title <doc>`, captions, URLs, `self` handled; `:glob:` patterns expand with Sphinx's dead-pattern warning; orphan check is exact membership (2026-08). |
| Directive/role validation in the build | ✅ | Wired wave 4: runs on every build (`validate_directives`, default on; `-D validate_directives=0` disables). Findings surface as warnings with file:line through the standard `-W`/`-w` pipeline. Unknown directives/roles stay silent (10+10 validators cover a fraction of Sphinx). False-positive heuristics fixed/demoted: `.. note:: inline` is content not arguments; bare `code-block`, spaces/uppercase in `:ref:` labels, relative `:doc:` paths, kbd/menuselection styles all accepted. |
| Nitpicky cross-reference validation (`-n`) | ✅ (opt-in) | Wired wave 4: `:doc:`/`:ref:` resolve against built documents, explicit `.. _label:` targets, and section anchors via the domain registry; broken refs warn `unknown document:`/`undefined label:` with line numbers. Python-domain refs counted, reported once as unvalidatable (no object inventory until the M5 sidecar). The wave-3 flip sources labels from real doctree target nodes (no more literal-block false registrations). |
| Warning pipeline (`-W`, `-w`) | ✅ | Toctree, directive/role, and nitpicky warnings all flow through it; `-W` exits 1 with sphinx-build 9.1's exact behavior (collect-all; keep-going is the default since Sphinx 8.1). |
| Error pipeline | ✅ | Per-file failures are collected as `BuildErrorReport`s while the build continues; **builds with errors exit 1** (sphinx-build parity), `-W`+warnings exits 1, usage errors exit 2 via clap (2026-08). |
| Static asset copying | 🟡 | Copies 5 handwritten shim files (incl. a 61-line fake jquery.js) + project `_static`/`_templates`; generated pages reference none of them; `html_static_path` ignored by the live path. |
| Search index / genindex / objects.inv emission | 🔴 | `generate_search_index`/`generate_indices` are TODO no-ops. No searchindex.js, genindex.html, or objects.inv in output (verified empirically). |
| Extension loading | 🔴 | Loading any extension fabricates a stub record and prints one line. Zero behavioral effect. (The never-used pyo3 dependency was removed 2026-08; Python interop arrives as a sidecar in ROADMAP M5.) |
| Build stats | 🟡 | `files_skipped` hardcoded 0; cache hits only counted under `--incremental` (default-on in sphinx-build mode). |
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
| `domains/` (Python + RST domain validation) | ✅ wired (wave 4) | RST domain drives the `-n` nitpicky pass (see pipeline table). Python domain still has no registration source until M5. Reference scanner is backtick-only and domain-qualified-aware now. Remaining: external-ref detection is a hardcoded stdlib prefix whitelist (inert while python refs are skipped); duplicate labels silently overwrite. |
| `directives/validation/` (10+10 validators) | ✅ wired (wave 4) | Runs on every build (see pipeline table). The `.. note:: inline text` false-positive class is fixed at the parser+validator level. |
| `validation/` (constraint engine) | 🧩 | Deliberately **not** wired in M1: nothing can produce `ContentItem`s until sphinx-needs item extraction exists (M4/M5) — wiring it now would validate an empty set. The always-success placeholder trait impls were deleted (wave 4) so future wiring can't silently no-op through the trait-method collision. Remaining: expression evaluator supports only `==`/`!=`/`in list`/`and`/`or`/`not`; no way to declare constraints in any config file. |
| `directives.rs` (HTML processor registry) | 🧩 | ~40 processors registered, 28 are stubs emitting HTML comments; `process_directive` has zero call sites (the never-used registry field was removed from `Parser` in wave 4); name-collides with the validation `DirectiveRegistry`. |
| `roles.rs` | ✅ deleted (wave 4) | Was never declared in any module tree — 291 lines the compiler never saw. Role rendering arrives with the real pipeline in M2/M3. |

## Configuration

| Feature | Status | Evidence / gaps |
|---|---|---|
| conf.py parsing | ✅ (declarative subset) | Rewritten 2026-08: logical-statement scanner + Python literal parser handles multi-line lists/dicts/tuples, nesting, string concatenation, triple-quoted strings, comments. **Every dropped construct warns** with `conf.py:line`. Dynamic values (env vars, calls) still require the M5 sidecar. Half of `ConfPyConfig` (latex_*/epub_*/source_suffix/nitpick_*…) is declared but never populated (M2+ consumers). |
| YAML/JSON config | ✅ | Serde defaults across all config structs (2026-08): partial configs load; both shipped YAML examples verified by unit + E2E tests. |
| Config auto-detection order | ✅ | conf.py → yaml → yml → json → default. |
| `--config` flag | ✅ | Routes `conf.py`/`.py` to the Python config parser (2026-08); YAML/JSON as before. |
| Config knobs actually consumed | 🟡 | Consumed now: `max_cache_size_mb`, `cache_expiration_hours` (wave 3); `nitpicky`, `validate_directives`, `doctree_dir`, `fail_on_warning`, `include/exclude_patterns`, `parallel_jobs` (wave 4). Still decorative until their consumers land (M2/M3): `html_theme`, `theme.*`, `output.syntax_highlighting`/`highlight_theme`/`minify_html`/`search_index`, `optimization.*`, `html_static_path`, `html_context`, `tags`. |
| `-D key=value` overrides | ✅ | Wave 4: typed coercion against the field's existing type, dotted paths for nested sections and map settings (`html_context.name=value`), duplicated-pair sync (`html_theme`, `templates_path`, `html_static_path`), unknown keys warn with sphinx-build's message and count toward `-W`/the `-w` file. Known gap: conf.py *parser* warnings still bypass the `-W` totals (config-diagnostics channel is M2). |

## CLI vs sphinx-build

| Capability | Status |
|---|---|
| `build --source/--output`, `-j`, `--clean`, `--incremental`, `-W`, `-w` | ✅ (relative `--source` crash fixed 2026-08) |
| Positional `SOURCEDIR OUTPUTDIR`, `-b html`, `-M html/clean`, `-D`, `-A`, `-n`, `-q`, `-E`, `-a`, `-c`, `-t`, `-T`, `--keep-going`, `-j auto`, repeatable `-v` | ✅ (wave 4) — sphinx-build compatible argument mode; parity measured against real sphinx-build 9.1.0 (exit codes, `-M` output layout, message shapes). Non-html builders and make-mode targets exit 2 with an honest message. Trailing FILENAMES accepted with a not-supported-yet warning. A source dir literally named `build`/`clean`/`stats` needs `./`-prefixing (documented). |
| Non-zero exit on build errors | ✅ exit 1 on build errors and `-W`+warnings (all warnings collected first, sphinx 9.1 behavior), 2 on usage/config/unsupported-builder errors. Deliberately **stricter** than sphinx-build on logged errors: real sphinx-build exits 0 on ERROR diagnostics without `-W`; unreadable sources silently passing CI is the exact M1 trust problem, so we exit 1. sphinx-build mode also refuses an output dir that equals/contains the source dir (exit 1) and requires a config (exit 2), like sphinx-build. |
| `RUST_LOG` | ✅ pre-set `RUST_LOG` wins over `-v`/`-q` defaults (wave 4; was clobbered at startup) |
| `serve` (advertised by dev.sh/build.sh) | ⬜ does not exist (ROADMAP M3) |

## Infrastructure & release

| Area | Status | Notes |
|---|---|---|
| CI (fmt, clippy -D warnings, tests, audit, coverage, 3-OS) | ✅ | MSRV job added (1.85, `--locked`); vacuous `integration_test.rs` step removed (2026-08). |
| E2E tests of the binary | ✅ | `tests/e2e_cli.rs` (2026-08): runs the real binary against fixture projects; asserts exit codes, warning text, output tree, `--config` routing. |
| Cargo.lock | ✅ | Committed (2026-08); CI/release/publish all run `--locked`. |
| Release workflow | ✅ | `publish-crate` gated on `needs: [validate-version, build-release]`; SHA-256 checksums published per artifact and verified by install.sh; artifacts named `os-arch` (e.g. `linux-aarch64`, built on `ubuntu-24.04-arm`); musl built with `cross` (2026-08). |
| pyo3/pythonize | ✅ removed (2026-08) | Had zero call sites while linking libpython into every build (two RUSTSEC advisories, broken musl target, undocumented Python build dependency). Python interop returns as a venv **sidecar process** in ROADMAP M5 — not as a link-time dependency. |
| Unused dependencies | ✅ pruned (2026-08) | Removed: pyo3, pythonize, syntect, cssparser, minifier, tar, bincode, crossbeam, lru, config, glob, walkdir, indexmap, toml, ini, handlebars. syntect returns when highlighting is actually wired (M2/M3). |
| Repo hygiene | ✅ | Scaffold leftovers deleted; metadata (`rust-version = "1.85"`, keywords, categories, exclude) merged into `Cargo.toml`; CHANGELOG backfilled (0.2.0/0.2.1/0.3.0); SECURITY.md describes the real attack surface (2026-08). |

## Testing status

| Suite | Status |
|---|---|
| Unit tests (lib + bin) | ✅ 111 passing |
| Pattern compatibility tests | ✅ 10 passing — assertions now encode Sphinx 9.1 semantics (wave 4) |
| Pattern differential suite | ✅ 881 generated cases vs `sphinx.util.matching` 9.1.0, zero divergence; regenerate with `uv run --python 3.12 --with 'sphinx>=9.1,<9.2' python tools/gen_pattern_fixture.py` |
| `tests/e2e_cli.rs` | ✅ 36 passing — incl. 19 sphinx-build-mode and validation-wiring tests (wave 4) |
| Benchmarks | 🟡 exercise the placeholder pipeline (numbers measure escaped-text copying); cache benchmark is `black_box(42)` |

## Historical note

Previous versions of this document (and the README/VALIDATION_FEATURES_PLAN) marked
the domain system, directive/role validation, and constraint engine as "Fully
Implemented ✅". That was true of the *library code and its unit tests* but not of
the product: none of the three systems has ever been invoked by `sphinx-ultra build`.
This document now tracks binary-reachable behavior only; wiring those systems into
the build is ROADMAP M1.
