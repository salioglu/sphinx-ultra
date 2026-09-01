# Implementation Status

**Audit-verified status as of 2026-08-31** (v0.4.1 + M2 waves 1–4).
Method: every status below was established by tracing call graphs from the binary's
entry point (`src/main.rs` → `SphinxBuilder::build`), running the built binary
against fixture projects, and — for compatibility claims — differential comparison
against real Sphinx 9.1.0. Statuses describe **what `sphinx-ultra build` actually
executes**, not what modules exist.

Note on the word "wave": rows dated 2026-08 and marked "(wave *n*)" without a
milestone refer to **M1** waves; M2 waves are always written out as "M2 wave *n*".

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
| Incremental cache | ✅ | Fixed 2026-08: warm-cache rebuilds no longer deadlock (DashMap guard held across `alter` — found by the new E2E suite); hits write the rendered page; `--clean --incremental` produces a full tree (clean clears the cache); `max_cache_size_mb`/`cache_expiration_hours` plumbed; config changes invalidate via blake3 fingerprint; eviction honestly named least-accessed (LFU-style). M2 wave 4: staleness is now Sphinx's env-level computation, not mtime alone (below). |
| Dependency graph / outdated computation | ✅ (M2 wave 4) | `build_dependency_graph`'s empty-vec TODO is gone. `BuildEnvironment::get_outdated_files` (`src/env/mod.rs`) is a port of Sphinx's: added ∪ changed ∪ removed documents, where "changed" consults `env.dependencies[docname]` — a dependency that is missing or newer than the document's read time makes the document outdated. `src/env/dependencies.rs` ports `note_dependency` and `relfn2path`. **Population today is image `uri`s only**: `include`/`literalinclude` are the other file-dependency sources and are wave 4.5; `docutils.conf` and gettext catalogs are unmodelled. Fixture `tests/fixtures/deps_image/` + `tests/e2e_cli.rs` cover the touch-an-image-and-rebuild path. Config-class (`rebuild='env'`) narrowing is deliberately not done: the whole-config `.config-fingerprint` wipes the cache on *any* config change, a strict superset that cannot under-rebuild. |
| RST parsing | ✅ (docutils-fidelity, wired) | **M2 wave 3 (2026-08-13): the binary runs the new parser** — `Parser::parse` → `src/rst/parse_rst_full` (sphinx mode); the M1 line-scanner is deleted. `src/doctree/` generic-node IR with byte-parity `pformat`; `src/rst/` block + inline grammar (waves 1–2) plus docutils-exact directive machinery (typed option converters with docutils-verbatim error texts, options-before-arguments evaluation order, rawsource literals, as-written names), the docutils built-in directive set (admonitions/topic/sidebar/rubric/quote-family/compound/container/parsed-literal/image/figure/code/math/raw/line-block/class/table/csv-table/list-table) and substitution definitions (replace/unicode/date, embedded directives, duplicate dupname semantics). Zero divergence on a committed 663-case fixture vs docutils 0.22.4 parse layer (`tests/doctree_differential.rs`). Sphinx-mode set (toctree, versionmodified family, seealso, code-block/sourcecode + highlight state, only, rst-class, math + equation targets, index, hlist, glossary, xref pending_xref anatomy, pep/rfc/cve/cwe) verified against a real sphinx-build 9.1.0 read-phase oracle (`tests/sphinx_doctree_differential.rs`). Document now derives title/toc (docutils `make_id` anchors)/labels/toctree entries/directive+role records from the doctree; the three builder raw-source re-scanners are gone. **M2 wave 4** added generic object-description anatomy (`desc`/`desc_signature`/`desc_name`/`desc_addname`/`desc_annotation`/`desc_content`, the `:no-index:`/`:no-index-entry:`/`:no-contents-entry:`/`:no-typesetting:` family, the `PropagateDescDomain` transform) and the std-domain directives on top of it — `program`, `option` (incl. `[=value]` and comma-separated multi-name forms), `envvar`, `confval` with `:type:`/`:default:`, `describe`/`object`, `default-domain`; plus glossary terms taking their ids from Sphinx's `make_id` (not docutils') and index entries following `process_index_entry` onto a list-valued attribute. The sphinx oracle now stands at **316 cases, zero divergence**. Remaining deferrals: literalinclude/include and py-domain object descriptions (both M2 wave 4.5), ifconfig, meta, rst_prolog/epilog/default_role. Known gap recorded in-tree (`tools/gen_sphinx_fixture.py` header): `ObjectDescription`'s `allow_section_headings=True` is not modelled — this crate's nested parse is `match_titles=False` throughout, so a section title (or a `topic`/`sidebar`) inside a description body is rejected with `Unexpected section title.` where Sphinx accepts it. Threading a real `match_titles` through the section machinery is its own change; the two probe cases are held out of the corpus rather than committed knowingly-red. |
| Markdown parsing | ❌ | Only `Event::Text` survives pulldown-cmark; headings/code/lists/tables discarded; `.md` titles/TOCs always empty; front matter TODO. |
| HTML rendering | 🔴 | `builder.rs` "Simple document rendering (placeholder)": output is `<html><body>{escaped raw source}</body></html>`. `DocumentContent::Display` returns the raw source. No AST rendering, layout, navigation, or asset links. |
| BuildEnvironment (read → merge → resolve → write) | ✅ (M2 wave 4) | `src/env/` replaces the never-constructed `src/environment.rs`. The build is now four phases over a real environment: a parallel read producing per-document doctrees (persisted as bincode under the `-d` dir, behind a `SUDT`+version header so an older format is an honest miss, `src/builder.rs`), a merge into a serialized `BuildEnvironment` (bincode, `ENV_VERSION`-stamped, `env.save`/`load`), a resolve pass per document over a *copy* of its doctree in docname order (mirroring Sphinx's `get_and_resolve_doctree` and therefore its warning order), and the write phase. Modules: `toctree.rs` (graph, `tocs`, `toctree_includes`, `files_to_rebuild`, relations, consistency warnings), `numbers.rs` (`toc_secnumbers`/`toc_fignumbers`), `std_domain.rs`, `genindex.rs`, `metadata.rs`, `dependencies.rs`, `resolve.rs`. |
| Environment differential oracle | ✅ (M2 wave 4) | `tools/gen_env_fixture.py` builds 15 multi-document projects (47 documents) with a real `SphinxTestApp` + `app.build()` on sphinx 9.1.0 and records the post-build environment; `tests/env_differential.rs` (22 tests) replays each project through this crate and compares every key: `tocs`, `toc_num_entries`, `toctree_includes`, `files_to_rebuild`, `relations`, `toc_secnumbers`, `toc_fignumbers`, the std registries, index entries, genindex, the full warning stream, and each document's resolved-doctree pseudo-XML — **zero divergence**. Each corpus project is built exactly **once, cold** (`build_project` makes a fresh tempdir, never calls `enable_incremental`, and every corpus-wide assertion reads that single build); warm-equals-cold is a separate claim, asserted by six hand-written tests in the same file over their own two- and three-document projects. Three strict, self-cleaning exemption tables (`KNOWN_WARNING_GAPS`, `KNOWN_RESOLVED_GAPS`, `KNOWN_INERT_CONF`; `KNOWN_TOC_GAPS` and `KNOWN_STD_GAPS` are empty) name what is not yet compared and why: 11/15 projects' warning streams match byte-for-byte (3 differ only by `image file not readable`, 1 by the write-phase circular-toctree warning), and 20/47 resolved doctrees match byte-for-byte (the other 27 carry an unresolved `toctree` node — wave 5's `_resolve_toctree` — an image without `candidates`, or an unapplied `PropagateTargets`). Listing a project that has *stopped* diverging fails the test, so exemptions cannot outlive their cause. |
| Toctree graph, relations, consistency warnings | ✅ (M2 wave 4) | Sphinx docname resolution (document-relative, `/`-absolute, `.`/`..`), `Title <doc>`, captions, URLs, `self`, `:glob:` with the dead-pattern warning, `:numbered:`/`:maxdepth:`/`:titlesonly:`/`:hidden:`/`:includehidden:`/`:reversed:`. The graph feeds `relations` (parents/prev/next, incl. Sphinx's quirk that a first child's `prev` is its parent) and the consistency warnings: nonexisting vs excluded entries, self-reference, circular toctrees, multiple parents (an *information* notice, not a warning), and `document isn't included in any toctree`. **Behavior change vs M1:** a toctree warning is now located at the `.. toctree::` directive line, as Sphinx locates it, not at the offending entry's line, and carries Sphinx's category suffix (`[toc.not_readable]`). Both changes are pinned by the env oracle and by `tests/e2e_cli.rs`. |
| Directive/role validation in the build | ✅ | Wired wave 4: runs on every build (`validate_directives`, default on; `-D validate_directives=0` disables). Findings surface as warnings with file:line through the standard `-W`/`-w` pipeline. Unknown directives/roles stay silent (10+10 validators cover a fraction of Sphinx). False-positive heuristics fixed/demoted: `.. note:: inline` is content not arguments; bare `code-block`, spaces/uppercase in `:ref:` labels, relative `:doc:` paths, kbd/menuselection styles all accepted. |
| std domain + cross-reference resolution | ✅ (M2 wave 4) | `src/env/std_domain.rs` collects labels (explicit targets, section/figure/table/code-block anchors with their titles), glossary terms, `option`s with program scoping and unscoped fallback, `envvar`s, and `confval`s; `src/env/resolve.rs` is a port of `ReferencesResolver` + `StandardDomain.resolve_xref` for `:ref:`, `:numref:`, `:doc:`, `:term:`, `:option:`, `:envvar:`, `:keyword:`, `:token:`. Warnings are Sphinx's own texts and categories — `duplicate label …, other instance in …`, `undefined label:`, `unknown document:`, `term not in glossary:`, `unknown option:`, `numfig is disabled. :numref: is ignored.`, `no number is assigned for …`, `the link has no caption: …`. **Behavior change:** these follow Sphinx's `warn_dangling` flags, which are set on seven std reftypes — `ref`, `numref`, `doc`, `term`, `keyword`, `option`, `confval` (`domains/std/__init__.py:748-766`) — regardless of `-n`, so a broken reference of any of those seven now warns in a default build; `-n`/`nitpicky` widens the warning to the remaining reftypes. `nitpick_ignore`/`nitpick_ignore_regex` are honored. Python-domain refs are still counted and reported once as unvalidatable (M2 wave 4.5 registers them). |
| Section & figure numbering (`numfig`) | ✅ (M2 wave 4) | `src/env/numbers.rs`: section numbers from `:numbered:` toctrees respecting `numfig_secnum_depth`, then figure/table/code-block/`displaymath` numbering scoped by them, in Sphinx's order and with its alphabetical-domain `get_figtype` dispatch. `:numref:` renders through `numfig_format` (`{name}`/`{number}` new style and `%s` old style). Pinned by the env oracle's `toc_secnumbers`/`toc_fignumbers` keys and by the corpus's numfig projects. |
| Warning pipeline (`-W`, `-w`) | ✅ | Toctree, directive/role, environment and resolution warnings all flow through it; `-W` exits 1 with sphinx-build 9.1's exact behavior (collect-all; keep-going is the default since Sphinx 8.1). M2 wave 4 added Sphinx's warning **categories**: a warning logged with a `type` renders a ` [type.subtype]` suffix (`show_warning_types`, on by default since Sphinx 8.3); a `subtype`-only warning prints bare, like Sphinx's. |
| Error pipeline | ✅ | Per-file failures are collected as `BuildErrorReport`s while the build continues; **builds with errors exit 1** (sphinx-build parity), `-W`+warnings exits 1, usage errors exit 2 via clap (2026-08). |
| Static asset copying | 🟡 | Copies 5 handwritten shim files (incl. a 61-line fake jquery.js) + project `_static`/`_templates`; generated pages reference none of them; `html_static_path` ignored by the live path. |
| genindex data | ✅ (M2 wave 4) | `src/env/genindex.rs` ports `IndexDomain.process_doc` (5-tuple entries, `split_index_msg` validation with Sphinx's `invalid {type} index entry {value!r}` warning and node removal) and `IndexEntries.create_index` (single/pair/triple/see/seealso, `!main` promotion, Symbols and `_` grouping, insertion-ordered sub-entries, the dropped-entry notice). Compared against the oracle's `genindex` key for every corpus project. |
| Index/search **file** emission | 🔴 | Nothing reaches the output tree: `generate_indices`/`generate_search_index` (`src/builder.rs`) are still TODO no-ops, so there is no `genindex.html` (the data above exists but has no renderer), no `searchindex.js`, and no `objects.inv` (the writer is real and tested but has no production call site). All three land with the M2 wave-5 HTML writer. |
| Extension loading | 🔴 | Loading any extension fabricates a stub record and prints one line. Zero behavioral effect. (The never-used pyo3 dependency was removed 2026-08; Python interop arrives as a sidecar in ROADMAP M5.) |
| Build stats | 🟡 | `files_skipped` hardcoded 0; cache hits only counted under `--incremental` (default-on in sphinx-build mode). |
| `clean` / `stats` commands | ✅ | `stats` cross-ref count is naive substring counting. |

## Built-but-not-wired stack (the "second codebase")

These are real modules with passing tests, exported from `lib.rs`, with **no call
sites in the binary** — they run only from `examples/` and unit tests. M2 wave 4
shrank this list: what remains is the **write side**, which the wave-5 HTML writer
revives.

| Module | Status | Notes |
|---|---|---|
| `html_builder.rs` (Sphinx `StandaloneHTMLBuilder` mirror, 800 lines) | 🧩 | Internally placeholder-grade even if wired: doc titles TODO, empty local TOC, empty search dump, `.buildinfo` in wrong format. Its dead `dump_inventory` call site was removed in M2 wave 4; the writer it called is now decoupled. **Kept deliberately** — deleting it would throw away the wave-5 starting point. |
| `template.rs` (minijinja engine + templates/) | 🧩 | User `templates_path` loading commented out ("lifetime issues"); `toctree()` returns an empty div; `pathto` ignores page depth; genindex/search templates use Python-only constructs (unregistered `_()`, `count.append(count.pop()+1)`) that fail at render time. **Kept deliberately** (wave-5 boundary). |
| `search.rs` (in-memory index) | 🧩 | Untouched by M2 wave 4. Output format is not Sphinx's `Search.setIndex` schema; 3-rule stemmer; title weighting inert. **Kept deliberately** (wave-5 boundary) — it is the only search code in the tree, and M3 rewrites it rather than starting over. |
| `inventory.rs` reader | ✅ wired (M2 wave 4) | Rewritten binary-safe (the old reader lossily UTF-8-converted the zlib payload and then split it on `str::lines`, corrupting real inventories); v1 + v2, `$`/`-` expansion, Sphinx's own `ValueError` texts. Live: intersphinx is the consumer. |
| `inventory.rs` writer (`InventoryFile::dump`) | 🧩 | Real and bytewise-verified against inventories a real `sphinx-build` wrote (`tests/inventory_roundtrip.rs`), but **no production call site** — nothing writes an `objects.inv` into a build output until the wave-5 HTML writer's finish task. |
| `environment.rs` (BuildEnvironment) | ✅ deleted (M2 wave 4) | 500 lines that were never constructed in the binary, with a `collect_relations` that returned an empty TODO. Replaced by `src/env/`, which the build actually runs (see the pipeline table). |
| `domains/` (Python + RST domain validation) | ✅ deleted (M2 wave 4) | The M1 heuristic layer: a regex reference scanner, a `DomainRegistry` of hand-registered names, fuzzy suggestions. Its live surface was replaced by the std domain (`src/env/std_domain.rs`) and Sphinx's resolution pass (`src/env/resolve.rs`), both oracle-pinned; the module then had zero call sites and went, along with `docs/DOMAIN_SYSTEM.md`, which documented only it. |
| `directives/validation/` (10+10 validators) | ✅ wired (wave 4) | Runs on every build (see pipeline table). The `.. note:: inline text` false-positive class is fixed at the parser+validator level. |
| `validation/` (constraint engine) | 🧩 | Deliberately **not** wired in M1: nothing can produce `ContentItem`s until sphinx-needs item extraction exists (M4/M5) — wiring it now would validate an empty set. The always-success placeholder trait impls were deleted (wave 4) so future wiring can't silently no-op through the trait-method collision. Remaining: expression evaluator supports only `==`/`!=`/`in list`/`and`/`or`/`not`; no way to declare constraints in any config file. **Kept deliberately**: unlike `domains/`, nothing has replaced it — it is waiting for a producer, not for a rewrite. |
| `directives.rs` (HTML processor registry) | 🧩 | ~40 processors registered, 28 are stubs emitting HTML comments; `process_directive` has zero call sites (the never-used registry field was removed from `Parser` in wave 4); name-collides with the validation `DirectiveRegistry`. |
| `roles.rs` | ✅ deleted (M1 wave 4) | Was never declared in any module tree — 291 lines the compiler never saw. Role rendering arrives with the real pipeline in M2/M3. |

**Deletion policy** (why `domains/` and `environment.rs` went and `search.rs`,
`template.rs`, `html_builder.rs` and `validation/` stayed): a module is deleted when
something else has taken over its job and it has zero call sites. `domains/` and
`environment.rs` were both superseded by `src/env/`. The rest have no replacement —
they are the starting points for M2 wave 5 and M3, and deleting them would trade a
known-imperfect implementation for a blank file.

## Configuration

| Feature | Status | Evidence / gaps |
|---|---|---|
| conf.py parsing | ✅ (declarative subset) | Rewritten 2026-08: logical-statement scanner + Python literal parser handles multi-line lists/dicts/tuples, nesting, string concatenation, triple-quoted strings, comments. **Every dropped construct warns** with `conf.py:line`. Dynamic values (env vars, calls) still require the M5 sidecar. Half of `ConfPyConfig` (latex_*/epub_*/source_suffix/nitpick_*…) is declared but never populated (M2+ consumers). |
| YAML/JSON config | ✅ | Serde defaults across all config structs (2026-08): partial configs load; both shipped YAML examples verified by unit + E2E tests. |
| Config auto-detection order | ✅ | conf.py → yaml → yml → json → default. |
| `--config` flag | ✅ | Routes `conf.py`/`.py` to the Python config parser (2026-08); YAML/JSON as before. |
| Config knobs actually consumed | 🟡 | Consumed now: `max_cache_size_mb`, `cache_expiration_hours` (M1 wave 3); `nitpicky`, `validate_directives`, `doctree_dir`, `fail_on_warning`, `include/exclude_patterns`, `parallel_jobs` (M1 wave 4); `root_doc`/`master_doc`, `numfig`, `numfig_format`, `numfig_secnum_depth`, `nitpick_ignore`, `nitpick_ignore_regex`, `intersphinx_mapping`, `intersphinx_disabled_reftypes`, `intersphinx_resolve_self`, `intersphinx_cache_limit`, `intersphinx_timeout`, `tls_verify`, `tls_cacerts`, `user_agent` (M2 wave 4 — each with the same conf.py/YAML/`-D` plumbing and, for the intersphinx ones, Sphinx's own `ConfigError` texts on malformed input). Still decorative until their consumers land (M2 wave 5/M3): `html_theme`, `theme.*`, `output.syntax_highlighting`/`highlight_theme`/`minify_html`/`search_index`, `optimization.*`, `html_static_path`, `html_context`, `tags`. |
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
| CI (fmt, clippy -D warnings, tests, audit, coverage, 3-OS) | 🟡 | The fmt/clippy/test/audit/coverage legs are real; vacuous `integration_test.rs` step removed (2026-08). **The `MSRV (1.85)` job and the `beta` matrix leg are vacuous** (found by the M2 wave-4 sweep; the fix is PR #54, which is repo-wide and lands ahead of this branch): `dtolnay/rust-toolchain` sets the toolchain with `rustup default`, which rustup ranks *below* the repo's `rust-toolchain.toml` (`channel = "stable"`), so both jobs compile with stable and have never tested what they name. Reproduce: in a checkout, `rustup show active-toolchain` prints `stable … (overridden by '…/rust-toolchain.toml')`, while `RUSTUP_TOOLCHAIN=1.85 rustup show active-toolchain` prints `1.85 … (overridden by environment variable RUSTUP_TOOLCHAIN)`. #54 sets `RUSTUP_TOOLCHAIN` in those two jobs; until it merges, the MSRV is verified by hand: `RUSTUP_TOOLCHAIN=1.85 cargo check --locked --all-targets`, green as of M2 wave 4. |
| E2E tests of the binary | ✅ | `tests/e2e_cli.rs` (2026-08): runs the real binary against fixture projects; asserts exit codes, warning text, output tree, `--config` routing. |
| Cargo.lock | ✅ | Committed (2026-08); CI/release/publish all run `--locked`. |
| Release workflow | ✅ | `publish-crate` gated on `needs: [validate-version, build-release]`; SHA-256 checksums published per artifact and verified by install.sh; artifacts named `os-arch` (e.g. `linux-aarch64`, built on `ubuntu-24.04-arm`); musl built with `cross` (2026-08). |
| pyo3/pythonize | ✅ removed (2026-08) | Had zero call sites while linking libpython into every build (two RUSTSEC advisories, broken musl target, undocumented Python build dependency). Python interop returns as a venv **sidecar process** in ROADMAP M5 — not as a link-time dependency. |
| Unused dependencies | ✅ pruned (2026-08) | Removed: pyo3, pythonize, syntect, cssparser, minifier, tar, bincode, crossbeam, lru, config, glob, walkdir, indexmap, toml, ini, handlebars. syntect returns when highlighting is actually wired (M2 wave 5/M3). M2 wave 4 re-added **bincode** (this time with call sites: environment + doctree persistence) and added **ureq** with rustls (intersphinx inventory fetching). `resolver = "3"` is set for that second one: edition 2021's default resolver would happily pick a `ureq` whose own `rust-version` exceeds ours, breaking `cargo install` on the MSRV we advertise — for other people, silently. |
| Repo hygiene | ✅ | Scaffold leftovers deleted; metadata (`rust-version = "1.85"`, keywords, categories, exclude) merged into `Cargo.toml`; CHANGELOG backfilled (0.2.0/0.2.1/0.3.0); SECURITY.md describes the real attack surface, including the outbound HTTPS the intersphinx work added (updated M2 wave 4). |

## Testing status

**529 tests, all passing** (as of M2 wave 4). Every generator below is pinned to
sphinx 9.1.0 / docutils 0.22.4 and asserts those versions at runtime; all five
reproduce their committed output byte-identically.

| Suite | Status |
|---|---|
| Unit tests (lib + bin) | ✅ 429 passing (422 lib + 7 bin) |
| Pattern compatibility tests | ✅ 10 passing — assertions encode Sphinx 9.1 semantics (M1 wave 4) |
| Pattern differential suite | ✅ 881 generated cases vs `sphinx.util.matching` 9.1.0, zero divergence; regenerate with `uv run --python 3.12 --with 'sphinx>=9.1,<9.2' python tools/gen_pattern_fixture.py` |
| Doctree differential suite (docutils parse layer) | ✅ 663 generated cases vs docutils 0.22.4, zero divergence; regenerate with `uv run --python 3.12 --with docutils==0.22.4 python tools/gen_doctree_fixture.py` |
| Sphinx doctree differential suite (real read phase) | ✅ 316 generated cases vs a `sphinx-build` 9.1.0 read phase, zero divergence; regenerate with `uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python tools/gen_sphinx_fixture.py` |
| Environment differential suite | ✅ 22 tests over 15 projects / 47 documents vs a real `SphinxTestApp` build, zero divergence on every compared key (exemption tables above). The corpus comparison is over one **cold** build per project; warm-equals-cold is asserted by six of those tests over their own two- and three-document projects. Same `uv` invocation with `tools/gen_env_fixture.py` |
| Inventory round-trip suite | ✅ 5 tests over 12 committed `.inv` files (4 sphinx-written, 3 handcrafted-valid, 5 handcrafted-malformed), expectations taken from Sphinx's own `InventoryFile.loads`; same `uv` invocation with `tools/gen_inventory_fixture.py` |
| Doctree serde / interner-cap suites | ✅ 3 passing — bincode round-trip and the interner's bound |
| Property tests (`tests/rst_proptest.rs`) | ✅ 7 passing — the parser never panics on arbitrary, multiline, multibyte, or deeply nested input |
| `tests/e2e_cli.rs` | ✅ 50 passing — the real binary against fixture projects: exit codes, warning text, output trees, `--config` routing, sphinx-build mode, incremental/dependency rebuilds |
| Benchmarks | 🟡 exercise the placeholder write path (numbers measure escaped-text copying); cache benchmark is `black_box(42)`. Rewrite is scheduled with M2 wave 5. |

## Historical note

Previous versions of this document (and the README/VALIDATION_FEATURES_PLAN) marked
the domain system, directive/role validation, and constraint engine as "Fully
Implemented ✅". That was true of the *library code and its unit tests* but not of
the product: none of the three systems had ever been invoked by `sphinx-ultra build`.
This document tracks binary-reachable behavior only. M1 wired directive/role
validation; M2 wave 4 replaced the domain system outright with an oracle-pinned std
domain and deleted the original (`docs/DOMAIN_SYSTEM.md`, which documented only that
API, went with it); the constraint engine is still library-only, waiting on a
`ContentItem` producer in M4.
