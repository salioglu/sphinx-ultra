# Sphinx Ultra Roadmap

**This is the canonical roadmap.** It supersedes the planning content in
[VALIDATION_FEATURES_PLAN.md](VALIDATION_FEATURES_PLAN.md) (now archived) and the
scattered status/priority sections that previously lived in the README and
[docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md). Statuses here were
established by a full code audit (call-graph traced from `src/main.rs`, behavior
verified by running the binary and differentially compared against Sphinx 9.1.0) in
August 2026.

## 1. Mission

**sphinx-ultra 1.0 is a production-grade, drop-in replacement for `sphinx-build -b html`,
with sphinx-needs built in as a first-class feature, at 10–100× the speed.**

Scope principles:

- **No feature exclusions.** Every Sphinx feature and every sphinx-needs feature is in
  scope. Nothing is "deliberately excluded" anymore — the previous validation-only
  scoping (which excluded search, theming, and templating) is retired. Features are
  *phased*, never *excluded*.
- **Compatibility is verified, not claimed.** Every "Sphinx-compatible" statement must be
  backed by a differential test against real Sphinx output or source. (This roadmap
  exists partly because the previous "100% Sphinx-compatible patterns" claim was
  falsified by differential testing — see §10.)
- **Production-ready workflows.** CI users must be able to trust exit codes, releases
  must be reproducible, and the binary must install and run on every advertised
  platform with no undocumented dependencies.
- **Parity targets:** Sphinx **9.1.x** (Python ≥3.12, docutils ≥0.21) and sphinx-needs
  **8.3.x**, tracked as upstream moves.

## 2. Where we actually are (verified baseline, 2026-08)

The 0.3.0 codebase is an architectural skeleton with excellent bones and placeholder
organs. The single most important fact: **the shipped build path emits the raw
RST/Markdown source, HTML-escaped, inside `<html><body>` — it does not yet render
documentation.** A large share of the codebase (the Sphinx-mirroring `HTMLBuilder`,
minijinja `TemplateEngine`, `SearchIndex`, `objects.inv` inventory, and all three
validation systems) is **dead code from the binary's execution path**: real modules
with real tests that `sphinx-ultra build` never calls.

| Subsystem | Status | Reality check |
|---|---|---|
| File discovery & patterns | **Working, verified** | `**` → `.*` and class emission now match Sphinx 9.1 exactly (2026-08); zero divergence across a committed 881-case differential suite generated against `sphinx.util.matching`. Discovery suffix-filters after matching like `Project.discover`. |
| Parallel orchestration | **Working** | Rayon pool, `-j`; per-file failures collected as error reports, build continues (fixed 2026-08). |
| Incremental cache | **Working** | Fixed 2026-08: hits write output, `--clean --incremental` safe, size/expiry knobs plumbed, config-change invalidation, warm-rebuild deadlock fixed, eviction honestly named. |
| RST parser | **Prototype (replacement landed, not wired)** | The binary still runs the line-scanner. M2 wave 1 (2026-08) landed the replacement as library code: `src/doctree/` (docutils-mirror IR, spans, byte-parity pseudo-XML, exact `make_id`/name normalization) + `src/rst/` (recursive-descent **block** parser: sections, transitions, bullet/enumerated/definition lists, block quotes + attribution, literal/doctest/line blocks, comments, hyperlink targets, docutils-exact system_messages) — zero divergence across a committed 175-case differential fixture generated against docutils 0.22.4 parse-layer output (`tools/gen_doctree_fixture.py`). Wave 2 (2026-08) completed the block+inline grammar as library code: full docutils Inliner (emphasis/strong/literal, every reference form, built-in roles, footnote/citation/substitution refs, standalone URIs, escape semantics), footnote/citation definitions, field + option lists, grid + simple tables — zero divergence across a committed 426-case parse-layer differential fixture. Remaining for the parser: directive machinery + substitution definitions (wave 3, where the new parser also replaces the line-scanner in the build). |
| Markdown parser | **Prototype** | pulldown-cmark events discarded except text; no headings/code/lists → `.md` titles and TOCs are always empty. |
| HTML rendering | **Placeholder** | Escaped raw source in `<html><body>`. `HTMLBuilder` (800 lines) + `TemplateEngine` (minijinja) exist but are never invoked. |
| Themes | **None** | `html_theme` is parsed and then ignored; no theme loading, no templates rendered, static shims copied but unreferenced. |
| Search | **Dead code** | In-memory index exists (non-Sphinx format); no `searchindex.js` emitted; no `searchtools.js` exists. |
| objects.inv / intersphinx | **Dead + broken** | Writer exists (unused); reader corrupts real inventories (lossy UTF-8 over zlib bytes); no resolution anywhere. |
| Extensions | **Stub** | Loading any extension prints one line and stores an inert record. Zero behavioral effect. (The never-used pyo3 dependency was removed 2026-08.) |
| Validation systems | **Two of three live** | Directive/role validation runs in every build (default on, false-positive heuristics fixed/demoted, Unknown silent); domain cross-ref validation runs under `-n`/nitpicky (2026-08). Constraint engine stays library-only until sphinx-needs items exist (M4) — no `ContentItem` producer yet; its always-success placeholder trait impls were deleted so future wiring can't silently no-op. |
| Build-path validation | **Working** | Toctree missing-ref + orphan checks with real line numbers and Sphinx docname resolution (relative/absolute/glob/`Title <doc>`); directive/role and nitpicky cross-ref passes wired 2026-08. All findings flow through the `-W`/`-w` pipeline. |
| conf.py support | **Working (declarative subset)** | Multi-line lists/dicts/tuples, string concat, triple quotes parsed (2026-08); every dropped construct warns with its line. Dynamic values await the M5 sidecar. |
| YAML/JSON config | **Working** | Serde defaults added 2026-08; partial configs load, both shipped YAML examples verified by tests. `--config` also accepts conf.py now. |
| CLI | **sphinx-build compatible** | Native `build/clean/stats` plus a full `sphinx-build` argument mode (2026-08): positional dirs, `-b html`, `-M html/clean`, `-D`/`-A`, `-d`, `-n`, `-q`, `-E`, `-a`, `-T`, `-t`, `-c`, `-j auto`, `-W`/`--keep-going`/`-w`, repeatable `-v`; pre-set `RUST_LOG` respected. Exit codes: 1 on errors/`-W`, 2 on usage/unsupported-builder/config errors (measured against sphinx-build 9.1.0). |
| CI/release | **Working** | Fixed 2026-08: publish gated on validation+build, Cargo.lock committed + `--locked` everywhere, MSRV (1.85) job, SHA-256 checksums + install.sh verification, aarch64-linux artifact built, vacuous `integration_test.rs` replaced by a real E2E suite. |
| Dependencies | **Pruned 2026-08** | 16 zero-call-site deps removed (incl. pyo3/pythonize, which carried two RUSTSEC advisories and linked libpython into every build). minijinja/flate2/base64 remain — used by the built-not-wired stack. |

Full evidence (file:line per finding) lives in [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md).

## 3. Architecture blueprint (the spine everything hangs on)

The critical path is a **real doctree pipeline**. Themes, search, extensions,
sphinx-needs, and every output format hang off it. The existing dead-code stack
(`HTMLBuilder`, `TemplateEngine`, `SearchIndex`, `InventoryFile`, `BuildEnvironment`,
domain/directive validation) is the right skeleton — the work is to make it real and
wire it in, not to write a third stack.

```
sources (.rst/.md) ──► Parsers (docutils-fidelity RST, MyST Markdown)
                          │  emit: typed doctree IR + source spans
                          ▼
                   Read phase (parallel)
                          │  per-doc: doctree, titles, TOC, labels, targets,
                          │  domain objects, needs, index entries, dependencies
                          ▼
                   BuildEnvironment (serialized cache, bincode)
                          │  merge → global toctree, relations (prev/next/parents),
                          │  numfig numbering, cross-ref resolution (domains +
                          │  intersphinx), transforms & post-transforms, events
                          ▼
                   Write phase (parallel)
                          │  doctree → HTML via theme templates (minijinja)
                          ▼
        outputs: pages, _static/_sources/_images/_downloads, genindex,
        domain indices, searchindex.js, objects.inv, .buildinfo, sitemap …
```

Load-bearing decisions:

1. **Typed doctree IR with docutils-equivalent node semantics** and source spans on
   every node. This is what `add_node`/transforms/i18n/needs all operate on.
2. **Two-phase build (read → resolve → write)** with an event bus mirroring Sphinx's
   events (`source-read`, `doctree-read`, `env-updated`, `doctree-resolved`,
   `html-page-context`, `build-finished`, …). Native extensions subscribe to the same
   lifecycle Python extensions expect — this is what makes the ecosystem portable.
3. **Extension ABI, native-first with an optional Python bridge.** Native Rust trait
   (`add_directive/add_role/add_transform/add_css_file/connect(event)`, plus an
   `html-collect-pages` equivalent so extensions can contribute non-doctree output
   pages — viewcode's `_modules/` needs it) for
   everything that doesn't need Python; a **sidecar CPython process in the project's
   venv** (JSON-RPC) for autodoc-class extensions. pyo3-in-process is *not* the plan:
   the sidecar isolates crashes, uses the project's venv, and keeps the core binary
   Python-free.
4. **`pyo3` stays out of the default build** *(removed 2026-08 — it linked libpython
   into every build with zero call sites, broke musl, and made Python an
   undocumented build dependency)*. The Python bridge arrives as an explicitly
   separate mechanism (sidecar process in M5), not a link-time dependency.
5. **conf.py strategy, two tiers.** Tier 1 (native): a real Python *parser* (not
   executor) handling the declarative 95% — multi-line lists, dicts, tuples, string
   concat, f-strings with literal parts — with **warnings for every dropped
   construct** (silent dropping is banned). Tier 2 (bridge): when the sidecar is
   available, execute conf.py in the venv exactly like Sphinx and serialize the
   namespace. `sphinx-ultra.toml/yaml` remains the native config path.
6. **Theme engine = minijinja + vendored theme packs.** Parse `theme.conf`/`theme.toml`,
   resolve inheritance chains across packages (`book → pydata → basic`), implement the
   Sphinx template contract (`pathto`, `toctree()`, `hasdoc`, sidebars, `css_tag`/
   `js_tag`, `theme_*` flattening, `html_context` passthrough), add `tobool`/`toint`
   filters, a `{% trans %}` → `gettext()` preprocessing pass, and the `_t` static-file
   templating pipeline. Vendor built assets from released wheels with a consolidated
   THIRD-PARTY-NOTICES file (all five target themes are MIT/BSD — verified viable).
7. **Byte-format compatibility at the seams**: `searchindex.js` (`Search.setIndex`
   schema + Snowball-equivalent stemming), `objects.inv` v2 (4-line header +
   zlib-compressed payload, emit **and** consume), needs.json (versioned schema with
   `field_type` annotations), anchor/permalink slugs (docutils id normalization).
   These formats are what let sphinx-ultra output interoperate with the ecosystem
   (themes' search UIs, intersphinx consumers, ubCode/needs tooling, deep links).

## 4. Milestones

Each milestone has acceptance criteria; a milestone is done when its criteria pass in
CI, not when its code merges. Versions are indicative; semver discipline starts at 1.0.

### M1 — v0.4 "Honest, solid core" (foundation repair)

The goal is that everything that *exists* works, everything that's *claimed* is true,
and CI can be trusted.

- **Correctness fixes** (all verified findings from the 2026-08 audit):
  - ✅ *(done 2026-08)* Relative `--source` crash; `[!seq]` classes (`[^/seq]`, Sphinx
    semantics); leading `^` in classes now literal (Sphinx semantics); Sphinx-parity
    directory pruning.
  - ✅ *(done 2026-08)* Pattern parity: `**` → Sphinx's `.*` semantics (no
    directory-boundary special case); class emission byte-identical to
    `_translate_pattern`; discovery suffix-filters after matching like
    `Project.discover`. Verified by a committed 881-case differential suite
    generated against `sphinx.util.matching` 9.1.0 — zero divergence.
  - ✅ *(done 2026-08)* Incremental cache: cache rendered output, never skip
    writing on hit, fix `--clean --incremental`, plumb
    `max_cache_size_mb`/`cache_expiration_hours`, honest eviction naming,
    config-change invalidation. (Also fixed in passing: warm-cache rebuilds
    deadlocked on a DashMap guard held across `alter` — caught by the E2E
    suite the moment `--incremental` got its first end-to-end test.)
  - ✅ *(done 2026-08)* Error pipeline: per-file failures become
    `BuildErrorReport`s (build continues), **non-zero exit code on errors**
    (sphinx-build parity: 1/2), real line numbers in toctree warnings, fix
    toctree false positives (captions, `Title <doc>`, `:glob:`,
    document-relative resolution).
  - ✅ *(done 2026-08)* Parser crash fixes: tab-indent panic, hyphenated directive
    names, `=`-underline title levels (docutils order-of-first-use).
  - ✅ *(done 2026-08)* Fix the two latent correctness defects in library code:
    reference-parser target/display inversion; constraint-engine `'static`
    transmute (templates now owned by the minijinja environment).
- **Repo & release hygiene:**
  - ✅ *(done 2026-08)* Commit `Cargo.lock`; `--locked` in CI/release. Delete
    `Cargo.toml.new`, `Cargo.lock.template`, `.packagename` after merging the useful
    metadata (`rust-version`, `keywords`, `categories`, `exclude`) into `Cargo.toml`.
  - ✅ *(done 2026-08)* Remove `pyo3`/`pythonize` (two RUSTSEC advisories, unblocks
    musl, drops the undocumented Python build dependency) and prune the other 14
    unused dependencies (syntect returns when actually wired, in M2/M3).
  - ✅ *(done 2026-08)* Gate `publish-crate` on `needs: [validate-version,
    build-release]`; add checksums (emitted per artifact, verified by install.sh);
    build the advertised `aarch64-unknown-linux-gnu` artifact.
  - ✅ *(done 2026-08)* MSRV: `rust-version = "1.85"` + MSRV CI job. Vacuous
    `integration_test.rs` step deleted, replaced by the E2E harness. Dependabot
    bumps applied in-repo (actions/cache 5, actions/checkout 6, criterion 0.7).
  - ✅ *(done 2026-08)* Fix `dev.sh serve` / `build.sh` references to the nonexistent
    `serve` command; backfill CHANGELOG entries for 0.2.0/0.3.0; update SECURITY.md
    (supported versions; delete claims about nonexistent subsystems).
- **Config loading that works:** ✅ *(done 2026-08)* `#[serde(default)]` across
  `BuildConfig` (partial YAML/JSON loads — verified on the repo's own examples);
  ✅ *(done 2026-08)* `--config conf.py` routed to the conf.py parser;
  ✅ *(done 2026-08)* conf.py parser upgraded to multi-line lists/dicts **with
  warnings on anything dropped**.
- ✅ *(done 2026-08)* **CLI foundation:** `sphinx-build`-compatible argument mode
  (positional `SOURCEDIR OUTPUTDIR`, `-b html` gate, `-M` make-mode — what
  quickstart Makefiles invoke — `-D key=value`, `-A key=value`, `-d doctreedir`,
  `-n`, `-q`, `-E`, `-a`, `-T`, `-t`, `-c`, `-j auto`, `--keep-going`, repeatable
  `-v`), global `--verbose`/`--config` (usable before or after the native
  subcommand), pre-set `RUST_LOG` respected. Parity measured against real sphinx-build
  9.1.0 (exit codes incl. `-W`'s collect-then-exit-1, `-M` output layout,
  message shapes). Companion executables: `sphinx-apidoc`/the 8.2+ `apidoc`
  extension ride M5 (autodoc bridge); `sphinx-autogen` is subsumed by M5
  autosummary stub generation; `sphinx-quickstart`-style scaffolding
  (`sphinx-ultra init`) lands in M7.
- ✅ *(done 2026-08)* **Wire the existing validation systems into the build**:
  directive/role validation on by default (`validate_directives`; Unknown stays
  silent; the `.. note:: inline` false-positive class fixed at parser+validator
  level, over-aggressive heuristics demoted) and `-n`/nitpicky cross-reference
  validation via the domain registry (`unknown document:`/`undefined label:`
  with line numbers; python-domain refs reported once as unvalidatable until
  the M5 sidecar). All findings flow through the standard warning pipeline so
  `-W`/`-w` see them. The constraint engine deliberately stays library-only
  until sphinx-needs items exist (M4) — nothing can produce `ContentItem`s yet.
- ✅ *(done 2026-08)* **End-to-end test harness** (the piece whose absence let the
  relative-path crash ship): `tests/e2e_cli.rs` runs the actual binary against
  fixture projects, asserting exit codes, warnings, and output tree. The
  commented-out `integration_test.rs` is deleted. (Grows with every subsequent
  M1 item.)

**Acceptance:** every command in README/QUICK_START works as written; both shipped
YAML examples load; `cargo test` includes E2E; pattern behavior matches Sphinx 9.1 in
a generated differential suite; releases are reproducible (`--locked`) and the publish
job cannot outrun validation; building requires no Python.

### M2 — v0.5 "A real doctree" (the parser milestone)

- **RST parser, docutils fidelity** (recursive descent, spans on every node):
  sections by adornment order (over/underline), transitions, paragraphs; **inline
  parser** (emphasis/strong/literal, roles incl. domain-qualified `:py:func:`,
  `~`/`.`/`!` modifiers, targets, references incl. anonymous/phrase, substitutions,
  footnote/citation refs, standalone links, escaping rules); bullet/enumerated/
  definition/field/option lists; grid + simple tables; literal blocks (incl. quoted,
  with the introducing paragraph kept), doctest and line blocks, block quotes with
  attribution; footnotes/citations with backlinks; hyperlink targets (internal,
  external, indirect, anonymous); substitution definitions incl. `|release|` etc.;
  comments; directive machinery with real option specs (typed conversion, content
  re-parsing, nesting) replacing the regex scanner.
- **Sphinx directives/roles on the new parser:** full `toctree` semantics (maxdepth/
  numbered/glob/hidden/includehidden/caption/titlesonly/reversed, `Title <doc>`,
  external URLs, `self`), `code-block`/`sourcecode`/`code` and `literalinclude`
  (all options; `:pyobject:` deferred to bridge), the `highlight` directive
  (`:linenothreshold:`/`:force:`) with `highlight_language`'s `default`
  fallback-guessing semantics and `pygments_style`, `include` (with `include-read`
  dependency tracking), admonitions incl. `:collapsible:` (8.2+) + `seealso` +
  `versionadded/changed/deprecated/removed` (+ dashed 9.x aliases), `only`/
  `ifconfig` tag expressions, `glossary`+`term`, `index` directive/role + genindex
  data, `math` + numbering + `eq`, `rst-class`, `hlist`, `rubric`, external-link
  roles (`pep`/`rfc`/`cve`/`cwe`), file-wide metadata (`:orphan:`, `:tocdepth:`,
  `:nosearch:`, `:nocomments:`), object-description anatomy incl. the no-index
  option family (`:no-index:`/`:no-index-entry:`/`:no-contents-entry:`/
  `:no-typesetting:` plus the legacy `:noindex:` spelling), `rst_prolog`/
  `rst_epilog` injection and `default_role`/`primary_domain`, `image`/`figure`
  with copying and `_images/` collision handling, `csv-table`/`list-table`/`table`.
- **Markdown (MyST core)** on markdown-it-compatible tokens: CommonMark + GFM tables,
  ```` ```{directive} ```` fences, `{role}` syntax, `(name)=` targets, front-matter,
  colon fences, and the common extension set (deflist, dollarmath, tasklist,
  substitution, attrs); shares the directive/role registries with RST.
- **Environment & resolution:** `BuildEnvironment` becomes real and serialized
  (bincode): global toctree graph → relations (parents/prev/next), numfig numbering,
  std domain (labels, `ref`/`numref`/`doc`/`term`/`option`/`envvar` roles plus the
  std directives — `program`/`option` with program-scoping and unscoped-fallback
  lookup, `envvar`, `confval` (7.4+, `:type:`/`:default:`), `describe`/`object`,
  `default-domain` — and duplicate-label warnings), py domain object registration
  from `py:*` directives with the object-signature config family
  (`maximum_signature_line_length` + per-domain variants, `toc_object_entries(_show_parents)`,
  `add_function_parentheses`), genindex + domain indices data, dependency-driven
  incremental invalidation (includes, images, literalinclude files, templates,
  config `rebuild` classes).
- **objects.inv:** byte-correct reader (binary-safe zlib) + writer; **intersphinx**
  resolution with cache, `intersphinx_mapping`, `:external:` roles,
  disabled-reftypes, and the shared HTTP config group (`tls_verify`/`tls_cacerts`/
  `user_agent`, also used by linkcheck in M3).
- **HTML writer v1:** doctree → semantic HTML through the (revived) `HTMLBuilder` +
  `TemplateEngine` with the `basic` theme lineage: real pages with title, headerlinks/
  permalinks (docutils-compatible slugs), local TOC, relbar prev/next, genindex,
  `_sources` copies, `.buildinfo` (Sphinx's config+tags-hash format, byte-verified
  in the differential harness), `objects.inv`, canonical
  URLs. Syntax highlighting via syntect with Pygments-compatible classes.
- Builders: `html`, `dirhtml`, `dummy`.

**Acceptance:** a golden-corpus differential harness (§10) builds a set of real-world
Sphinx projects with both `sphinx-build` and `sphinx-ultra` and diffs normalized
doctree/HTML output; corpus includes at least one nontrivial OSS project's docs
building with zero missing constructs; toctree/numfig/xref behavior matches Sphinx on
the corpus; anchors byte-match Sphinx for the corpus pages.

### M3 — v0.6 "Themes & search"

- **Theme engine** per §3.6: theme.conf/theme.toml, cross-package inheritance,
  options merging with unknown-option warnings, structured option values, user
  `templates_path` overrides with `!`-prefix parent lookup, `_t`/`.jinja` static
  templating, per-theme Pygments styles incl. dark variants, and loading of vendored
  theme `locale/` catalogs backing `_()`/`gettext` (sphinx-rtd-theme's chrome is
  translated; without this, non-English builds render English chrome).
- **Theme wave 1:** `basic` + **alabaster** (bootstraps the machinery) →
  **sphinx-rtd-theme** (pure Jinja, biggest installed base; exact `wy-*` DOM) →
  **furo** (small native shim: nav-tree checkbox injection, CSS-variable emission,
  source-link synthesis).
- **Search, byte-compatible:** `searchindex.js` in `Search.setIndex` schema
  (docnames/docurls/filenames/titles/terms/titleterms/alltitles/indexentries/objects/
  objtypes/objnames/envversion). Stemming must match the JS side exactly or search
  breaks: **classic Porter for English** (Sphinx's shipped JS `PorterStemmer`, not
  rust-stemmers' Porter2) and Snowball for da/de/es/fi/fr/hu/it/nl/no/pt/ro/ru/sv/tr,
  with generated stemmer-parity tests against Sphinx (same mechanism as the pattern
  tests). CJK segmentation (ja MeCab/TinySegmenter-class, zh jieba-class) is deferred
  to M7. Ship working `searchtools.js`/`language_data.js`/`sphinx_highlight.js`/
  `documentation_options.js`, render `search.html`/`genindex.html`/domain indices
  from real templates.
- **Asset pipeline:** `html_static_path`/`html_extra_path`, `html_css_files`/
  `html_js_files` with priorities and attribute tuples, `html_logo`/`html_favicon`,
  real (not shim) core JS, `html_baseurl`, OpenSearch emission.
- Builders: `singlehtml`, `linkcheck` (full `linkcheck_*` config), `text`.
- **`sphinx-ultra serve`**: watch (notify crate, debounced/coalesced, output-dir +
  user `--ignore` rules) + in-process incremental rebuild + websocket live-reload
  **injected into pages as served, never written into the build output** (per-page
  reload when the dirty set is small) + structured error overlay + sphinx-autobuild
  ergonomics parity (`--port` incl. 0, `--host`, `--open-browser`, `--pre-build`).
  Post-M5: also watch bridged-extension inputs (autodoc'd Python modules) with a
  kept-warm sidecar. This is the sphinx-autobuild replacement and the headline DX
  win (millisecond no-op rebuilds; see
  [docs/research/extensions.md](docs/research/extensions.md)).

**Acceptance:** the five wave-1/2 themes render the differential corpus with correct
navigation (collapse/current classes), working client-side search (query parity spot
checks vs Sphinx-built sites), theme options honored; a visual-regression suite
(screenshot diff) covers key pages per theme; `serve` sub-100ms incremental rebuild
on a 1k-file corpus.

### M4 — v0.7 "Extensions wave 1 (native)"

Native implementations on the extension ABI (each with per-extension conformance
tests against upstream output). Ordered by leverage:

1. `sphinx.ext.mathjax` (S) — node visitors + conditional asset injection
2. **sphinx-copybutton** (S) — first third-party: assets + config-templated JS
3. `sphinx.ext.extlinks` (S) — config-defined roles + hardcoded-link detection
4. `sphinx.ext.autosectionlabel` (S) — resolver pass + dup-label semantics
5. `sphinx.ext.todo` (S) — *the collector-pattern proving ground* (custom node, env
   storage, `doctree-resolved` aggregation, parallel merge)
6. `sphinx.ext.ifconfig`, `sphinx.ext.duration`, `sphinx.ext.githubpages` (S each)
7. **sphinx-sitemap** (S) + **sphinxext-opengraph** (S/M) + **sphinx-notfound-page**
   (S) + **sphinx-favicon** (S) — the RTD SEO plumbing set
8. `sphinx.ext.graphviz` (M) — `dot` subprocess, content-hash caching, svg/png+maps
9. **sphinxcontrib-mermaid** (S/M) — raw client-side mode first, `mmdc` later
10. **sphinx-design** (L) — grids/cards/dropdowns/tabs/badges/icons; ships the
    sphinx-tabs compat shim
11. `sphinx.ext.napoleon` (M) — pure text transform, Google+NumPy grammars (runs
    against docstrings from the bridge later; useful immediately for `py:*` content)
12. **myst-parser extended surface** — remaining MyST extensions to full parity
13. **sphinx-togglebutton**, **sphinx-last-updated-by-git** (S each)
14. `sphinx.ext.viewcode` HTML side (`_modules` pages) behind a static source-path
    provider (full fidelity arrives with the bridge)
15. **pydata-sphinx-theme** + **sphinx-book-theme** (theme wave 2; the `generate_*`
    helper family implemented natively against our toctree model)

**Acceptance:** a curated corpus of real projects using these extensions builds with
output parity; the "classic stack minus autodoc" (napoleon+intersphinx+myst+
copybutton+design) is green end-to-end.

### M5 — v0.8 "Python bridge (autodoc et al.)"

- **Sidecar protocol:** `sphinx-ultra-bridge` — a small Python package installed in
  the project's venv; JSON-RPC over stdio; warm process reused across rebuilds;
  crash-isolated; parallel-safe.
- **`sphinx.ext.autodoc`** (XL): documenter RPC (module → members/signatures/
  docstrings/source locations), all `auto*` directives, `autodoc_*` config,
  `autodoc-process-docstring`/`-signature`/`-skip-member` event parity (napoleon and
  typehints hook these — ours run natively on the returned docstrings); generated-rST
  re-parsing with source mapping; `autodoc_mock_imports`.
- **`sphinx.ext.autosummary`** (L): stub generation via minijinja from sidecar member
  trees; derived-artifact tracking in incremental builds.
- **sphinx-autodoc-typehints** (M) + core `autodoc_typehints=description` parity.
- **`sphinx.ext.viewcode`** full fidelity; **`sphinx.ext.linkcode`**;
  `sphinx.ext.inheritance_diagram` (bridge introspection + native graphviz).
- **nbsphinx/myst-nb native subset** (M): parse `.ipynb` JSON, render pre-executed
  outputs natively (covers the execute-in-CI workflow); kernel execution via sidecar
  later (XL tail).
- **conf.py tier-2**: execute conf.py in the venv via the sidecar; `setup(app)`
  inline-extension shim for the events/APIs we expose.
- Optional static-analysis fallback (griffe-style, no imports) as an opt-in fast
  path, clearly labeled not-bug-compatible.

**Acceptance:** a real Python library's docs (autodoc+napoleon+typehints+viewcode+
intersphinx) build with API pages at parity with Sphinx on the differential harness;
the sidecar survives module import errors without killing the build; a warm rebuild
after a docstring change is <500ms on the corpus project.

### M6 — v0.9 "sphinx-needs, first-class"

Target: sphinx-needs **8.3** semantics (verified inventory:
[docs/research/sphinx-needs.md](docs/research/sphinx-needs.md)), with
legacy 7.x spellings accepted-and-warned. Order of attack:

1. **Core need objects:** `needs_types`-driven directive synthesis, all need options,
   hash-identical ID generation (projects diff needs.json — byte fidelity matters),
   parts (`:np:`), nested needs, `needs_fields` typed fields (+ legacy
   `needs_extra_options`/`needs_global_options` translation), templates/
   `jinja_content` via minijinja, `list2need`.
2. **Filter engine:** Python-expression evaluator with CPython semantics (the
   load-bearing subsystem — reuse/replace the existing `ExpressionEvaluator`, which
   handles only `==`/`!=`/`in`/`and`/`or`/`not` today), the 8.1 fast-path optimizer,
   filter caching, `c.this_doc()`, `var.*` variants, filter-code/`filter_func` gated
   behind the sidecar.
3. **Presentation:** `needtable` (+DataTables assets), `needlist`, `needflow`
   (PlantUML **and** Graphviz engines), `needpie`/`needbar` (native chart renderer —
   plotters or SVG generation), `needgantt`/`needsequence`, `needuml`/`needarch`
   (Jinja-templated PlantUML with `flow/ref/uml/import` context), `needextract`,
   `needreport`, `if` directive.
4. **Data flow:** `needextend`, `needimport` (file/URL), `needs_external_needs`,
   **needs.json import/export at full fidelity** (versions, `needs_schema`,
   `field_type`, reproducible mode, per-id files, `needs` builder + `needs_build_json`),
   `needumls`/`:save:` export, permalinks. **Incremental-build correctness is a
   MUST here:** needs data persists in the serialized `BuildEnvironment`, changes to
   any need invalidate dependent filters/tables/flows, and `-E` re-fetches remote
   `needimport`/external sources (normal incremental builds reuse them).
5. **Links:** `needs_links` config, all link-type fields + backlinks, conditional
   links `ID[expr]`, dead-link handling, `needs_string_links`, `:need:`/
   `:need_outgoing:`/`:need_incoming:`/`:need_count:` roles with Jinja role
   templates (8.3), the 8.2 variant system (`needs_variant_data(_file)`, `var.*`
   filters, the `:variant:` role, per-field `parse_variants` `[variant]:value`
   option parsing, legacy `needs_variants` accept-and-warn).
6. **Dynamic functions:** `[[...]]`/`:ndf:` with the built-in set (`copy`,
   `check_linked_values`, `calc_sum`, `links_from_content`, …); custom Python
   functions via sidecar.
7. **Constraints & schema validation:** wire the (fixed) constraint engine to real
   needs with `needs_constraints` parsed from config; the 6.0+ JSON-schema system
   (`needs_schema_definitions`, local/network/network_back, severities,
   `schema_violations.json`) on a Rust JSON-schema engine — the one place we should
   handily beat upstream's jsonschema-rs numbers.
8. **Layouts/styles:** grid renderer, `<<meta()>>` layout-function mini-language,
   built-in layouts/styles, three CSS themes, Feather icons; `needs_warnings`.
9. **Services** (github, custom via sidecar) — last; open-needs is gone upstream.
10. **API surface:** a Rust equivalent of `sphinx_needs.api`
    (`add_need_type`/`add_field`/`add_dynamic_function`/`add_warning`, runtime
    `add_need`/`add_external_need`/`del_need`/`get_needs_view`) plus a sidecar shim
    mirroring it for Python extensions — third-party extensions calling
    `sphinx_needs.api` are the main ecosystem-compat risk.

**Pipeline ordering contract** (semantics-bearing, per upstream): collect →
`needextend` → dynamic functions → backlink computation → constraints/schema/
warnings → render. Getting this order wrong silently changes needextend/dynamic-
function/backlink results.

**Acceptance:** the sphinx-needs official demo/docs project builds with matching
needs.json (normalized diff), matching filter results, and rendered needs/tables/
flows; a corpus case exercises the needextend + dynamic-functions + backlinks
interaction; an incremental rebuild after editing one need correctly refreshes
dependent tables/flows; ubCode consumes our needs.json without complaint; schema
validation throughput ≥ upstream jsonschema-rs baseline.

### M7 — v1.0 "Production"

- **i18n:** `gettext` builder (`.pot` emission), `.po/.mo` consumption with
  doctree-level replacement, `locale_dirs`, Sphinx UI-string catalogs, sphinx-intl
  interop, `figure_language_filename`, translation-progress classes.
- Builders: `latex` (+`latexpdf`), `man`, `epub`, `texinfo`, `gettext`, `xml`/
  `pseudoxml`, `json` — `latex`/`man` to real-world quality, the rest to conformance.
- c/cpp/js/rst domains (cpp is compiler-frontend-sized; scope: parse-and-link parity
  for common declarations, documented limitations beyond).
- `doctest`/`coverage` builders via sidecar; `imgmath` + `sphinx.ext.imgconverter`
  (ImageMagick fallback, pairs with per-builder image candidates);
  `sphinxcontrib-plantuml` parity (shared with needs); **breathe-alternative**:
  native Doxygen-XML reader targeting breathe's directives (strategic C++
  audience); CJK search segmentation (ja/zh); legacy built-in themes (classic,
  nature, sphinxdoc, …) vendored to conformance (not pixel) quality;
  `sphinx-ultra init` project scaffolding (sphinx-quickstart equivalent).
- **Multi-version builds** as a first-class CLI feature (`sphinx-ultra multiversion`)
  rather than extension compat (the salvaged idea from the discarded `--all-projects`
  patch, redesigned: proper flag passthrough, error propagation, non-zero exits,
  isolated outputs).
- **Performance targets, enforced in CI:** 10k-file corpus full build < 10 s;
  incremental single-file rebuild < 100 ms; peak RSS < 1 GB on the 10k corpus;
  benchmark regression gate (criterion + tracked baselines).
- **Stability guarantees:** semver policy, MSRV policy (N-2), extension-ABI stability
  statement, deprecation policy, fuzzing (parser + patterns) in CI, `cargo audit`/
  `cargo deny` gates, signed releases + SBOM, PyPI wheels (`pip install sphinx-ultra`
  via maturin) alongside crates.io/binaries/Homebrew.
- **Dogfood:** this repo's documentation site is built and published by sphinx-ultra
  (closes [#16](https://github.com/salioglu/sphinx-ultra/issues/16)), using a wave-1
  theme, search, needs pages, and the differential harness as a public compat report.

**Acceptance ("1.0 gate"):** ≥ 25-project public compatibility corpus (incl. CPython
devguide-class, an RTD-theme project, a furo project, a pydata project, a needs
project) building at parity with published diff reports; zero known
correctness-class bugs older than one release; all production workflows (§9) green
for three consecutive releases.

### Post-1.0

`htmlhelp`/`qthelp`/`applehelp`/`devhelp` shells, `sphinx-gallery` (bridge),
`sphinxcontrib-spelling`-equivalent native lint, MyST-NB kernel execution,
sphinx-multiversion extension shim, ablog (works if directive/domain API parity
holds — harness-tracked), WASM plugin ABI for sandboxed third-party extensions,
incremental search-index updates, KaTeX/typst server-side math option, a shibuya
theme pack, and sphinx-immaterial (needs a bespoke strategy beyond vendored
templates — deep Python coupling; explicitly out of scope for the standard
JinjaTheme path).

## 5. Theme support matrix

Verified popularity (pypistats/GitHub, 2026-08); strategies from the theme research
(all licenses verified vendoring-compatible; assets vendored from released wheels,
never git trees):

| # | Theme | DL/mo | Strategy | Native shim required | Milestone |
|---|---|---:|---|---|---|
| 1 | **alabaster** (default) | 29.8M¹ | Vendor + minijinja | `_t` static templating; Pygments style table | M3 |
| 2 | **sphinx-rtd-theme** | 14.5M | Vendor + minijinja (pure Jinja) | none (needs exact toctree HTML, `toint`/`tobool`, and vendored jQuery from sphinxcontrib-jquery — a separate package from the theme wheel) | M3 |
| 3 | **furo** | 1.1M² | Vendor (furo + basic-ng) + shim | nav-tree checkbox rewrite; CSS-variable emission; source links (~300 lines) | M3 |
| 4 | **pydata-sphinx-theme** | 4.7M | Vendor + largest shim | `generate_header_nav_html`/`generate_toctree_html`/`generate_toc_html`, edit-URL provider, version switcher | M4 |
| 5 | **sphinx-book-theme** | 1.2M | Delta on pydata | launch/repo button URL synthesis | M4 |
| 6 | shibuya (stretch) | 0.2M | Vendor + minijinja | none expected | post-1.0 |

¹ inflated (hard dep of Sphinx) — still mandatory as the default theme.
² furo's mindshare (default for pip/attrs/urllib3-class projects) far exceeds its raw downloads.

Cross-cutting theme contract (built once, in M3): theme resolution/inheritance,
template context + `toctree()` callable with exact `toctree-l1 current` DOM, Jinja
dialect additions, byte-compatible search stack, wheel-based asset vendoring with
THIRD-PARTY-NOTICES. Unvendored third-party themes load best-effort through the same
path with a "Python hooks won't run" warning.

## 6. Extension support matrix

The 16-extension support target (verified downloads 2026-08; NATIVE = pure Rust,
BRIDGE = Python sidecar, HYBRID = split):

| # | Extension | DL/mo | Strategy | Size | Milestone |
|---|---|---:|---|---|---|
| 1 | sphinx.ext.**autodoc** | built-in | BRIDGE | XL | M5 |
| 2 | sphinx.ext.**autosummary** | built-in | BRIDGE | L | M5 |
| 3 | sphinx.ext.**napoleon** | built-in | NATIVE (text transform) | M | M4 |
| 4 | **sphinx-autodoc-typehints** | 9.1M | BRIDGE (thin, on autodoc) | M | M5 |
| 5 | sphinx.ext.**intersphinx** | built-in | NATIVE | M | M2 |
| 6 | sphinx.ext.**viewcode** | built-in | HYBRID | M | M4/M5 |
| 7 | **myst-parser** | 8.1M | NATIVE (second parser front-end) | XL | M2 (core) / M4 (full) |
| 8 | **sphinx-copybutton** | 7.6M | NATIVE | S | M4 |
| 9 | **sphinx-design** | 10.1M | NATIVE (+sphinx-tabs shim) | L | M4 |
| 10 | sphinx.ext.**mathjax** | built-in | NATIVE | S | M4 |
| 11 | **sphinxcontrib-mermaid** | 6.4M | NATIVE (raw mode; `mmdc` later) | S–M | M4 |
| 12 | sphinx.ext.**graphviz** | built-in | NATIVE (subprocess) | M | M4 |
| 13 | sphinx.ext.**extlinks** | built-in | NATIVE | S | M4 |
| 14 | sphinx.ext.**autosectionlabel** | built-in | NATIVE | S | M4 |
| 15 | sphinx.ext.**todo** | built-in | NATIVE (collector prototype) | S | M4 |
| 16 | **nbsphinx / myst-nb** | 1.9M/0.9M | HYBRID (native pre-executed; kernel via bridge) | XL | M5 |

Plus **sphinx-autobuild** (5.7M) — replaced by first-class `sphinx-ultra serve` (M3),
and **sphinx-needs** — first-class core (M6), not an extension.

Runners-up tracked in-roadmap: sitemap/opengraph/notfound-page/favicon/togglebutton/
last-updated-by-git/githubpages/ifconfig/duration (all NATIVE S, folded into M4);
plantuml (M, with needs); doctest/linkcode/inheritance_diagram (bridge, M5/M7);
breathe → native Doxygen-XML reader (M7); spelling (native lint, post-1.0);
sphinx-gallery (bridge, post-1.0); sphinx-hoverxref (skip — archived upstream,
absorbed by RTD Addons); sphinx-tabs (compat shim over sphinx-design).

## 7. Sphinx feature coverage commitments

The full inventory (verified against Sphinx 9.1 in August 2026) lives in
[docs/research/sphinx-features.md](docs/research/sphinx-features.md) (companions:
[themes](docs/research/themes.md), [extensions](docs/research/extensions.md),
[sphinx-needs](docs/research/sphinx-needs.md)) and will be encoded into the
differential harness; tier roll-up:

- **Tier 0 — required for the drop-in claim** (M1–M3): full docutils RST; toctree
  semantics; std+py domains; all cross-reference and semantic roles; conf.py strategy;
  html/dirhtml; theming contract incl. `basic` lineage; searchindex.js + searchtools;
  objects.inv emit+consume; intersphinx; nitpicky + `suppress_warnings`; genindex +
  py-modindex; permalink/anchor-slug byte compatibility; numfig; incremental
  semantics (`rebuild` classes, `-E`, `-a`); event bus + `add_*` extension API;
  `_sources`/`_static`/`_images`/`_downloads` layout; `.buildinfo`; exit codes.
- **Tier 1 — most real projects** (M4–M7): MyST full surface; autodoc/napoleon/
  autosummary/typehints; linkcheck; gettext/i18n; latex + man; viewcode/extlinks/
  todo/graphviz/autosectionlabel; c/cpp/js domains; dark-mode Pygments; sitemap/404/
  OGP; parallel read+write; `-W --keep-going`; `html_sidebars`/`html_additional_pages`;
  smartquotes; `source-read`/`include-read` hooks; image candidates by builder.
- **Tier 2 — long tail** (M7+): epub/texinfo/text/xml/changes; help builders;
  doctest/coverage; productionlist/rst domain; imgmath; `template_bridge`;
  serialized-HTML builders.

Anything not listed is still in scope (no exclusions) — unlisted items default to
Tier 2 and get a tier assignment when the differential harness first encounters them.

## 8. sphinx-needs coverage commitments

Parity target sphinx-needs 8.3 (see M6 for sequencing). MUST-level items:
need-directive family with typed `needs_fields`, hash-identical IDs, parts/nesting;
the Python-semantics filter engine with 8.1 fast paths; needtable/needlist/needflow
(both engines)/needpie/needuml; needextend/needimport/external needs; needs.json
byte-fidelity both directions; full link model incl. backlinks and conditional links;
dynamic functions; constraints + 6.0 schema validation with `schema_violations.json`;
layout grid system + styles + CSS themes; `needs_warnings`; `needs_from_toml`
(ubproject.toml ecosystem). Python callables in conf.py (custom dynamic functions,
callable `needs_warnings` entries, services, `filter_func`, and the
`sphinx_needs.api` shim) ride the sidecar; this is the only sphinx-needs area whose
fidelity is contingent on the bridge — and it is scoped, not excluded. As with §7,
any sphinx-needs item not named here defaults to a scheduled (never excluded) slot
and gets an explicit milestone when the M6 harness first encounters it.

## 9. Production-readiness workstream (continuous, starts in M1)

| Area | Commitment |
|---|---|
| CI | 3-OS matrix + MSRV job + `--locked`; fmt/clippy `-D warnings`; cargo-audit + cargo-deny; coverage gate; benchmark regression gate; E2E suite; differential-corpus job (nightly); fuzzing (parser, patterns) |
| Releases | tag→version validation → build (5+ targets incl. aarch64-linux and musl, unblocked by the pyo3 removal) → checksums/SBOM → gated crates.io publish; reproducible with committed Cargo.lock; CHANGELOG enforced by release script |
| Packaging | crates.io (clean metadata, exclude list), GitHub binaries, PyPI wheels via maturin (post-M5), Homebrew tap; install.sh target list generated from the release matrix; checksum-verified installs |
| Runtime deps | Core binary: zero (no Python, static musl build); bridge features degrade gracefully with actionable messages |
| Exit codes & CI trust | Errors → non-zero always; `-W` semantics; `--keep-going`; structured warning categories with `suppress_warnings` |
| Docs honesty | Every capability claim in README/docs backed by a test; performance numbers only from the benchmark suite with corpus + hardware stated; IMPLEMENTATION_STATUS regenerated from the audit, updated per merge |
| Governance | semver from 1.0; MSRV N-2; deprecation windows ≥ 2 minors; SECURITY.md kept truthful; issue templates already in place |

## 10. Testing strategy

1. **Unit tests** per module (exists; extend to the new parser node-by-node).
2. **E2E CLI tests**: run the binary on fixture projects; assert exit codes, warning
   text, output tree (new in M1 — this is the class of test whose absence shipped the
   relative-path crash and the unloadable YAML examples).
3. **Differential harness** (the backbone, M2+): build a corpus with real
   `sphinx-build` (pinned 9.1.x, in Docker) and sphinx-ultra; compare normalized
   outputs (doctree JSON, HTML DOM with volatile bits stripped, searchindex terms,
   objects.inv entries, needs.json). Publish per-project compat reports; every
   compatibility claim in docs links to a green harness run.
4. **Generated parity tests**: pattern matching and anchor slugs generated
   directly against `sphinx.util.matching`/docutils id rules (catches the `**`-class
   divergences mechanically).
5. **Visual regression** per theme (M3+): headless screenshots of corpus pages.
6. **Property tests** (proptest, already a dev-dep): parser never panics on arbitrary
   input (the tab-indent panic class); pattern translate/match round-trips.
7. **Performance benchmarks** with regression gates; the current bench suite is
   rewritten once the parser is real (today's numbers measure paragraph-skimming).
8. **Fuzzing** (cargo-fuzz) for the RST/MyST parsers and objects.inv reader.

## 11. Risks

| Risk | Mitigation |
|---|---|
| RST parser fidelity is a long tail (docutils has 20 years of edge cases) | Differential harness from day one; corpus-driven prioritization; property/fuzz testing; treat docutils source as the spec |
| conf.py is arbitrary Python | Two-tier strategy (native parser with loud warnings + sidecar execution); corpus telemetry on which constructs actually appear |
| cpp domain is compiler-frontend-sized | Scoped parity (common declarations), documented limits, breathe-XML alternative for serious C++ users |
| Python-bridge complexity (venv discovery, version skew) | Sidecar protocol versioned; bridge package published to PyPI; graceful degradation with actionable errors |
| Upstream moves (Sphinx 10, needs 9) | Track in the harness against pinned + latest; parity target updated per minor |
| Theme asset drift (wheels update) | Vendored packs pinned + hash-locked; CI job diffs against newest wheel to flag drift |
| Scope weight vs. maintainer bandwidth | Milestones are strictly ordered; each is shippable alone; matrices define "done" so partial credit is visible; CONTRIBUTING points newcomers at S-sized matrix rows |

## 12. How this roadmap is maintained

- This file is the single source of truth for scope and sequencing. PRs that change
  feature status must update §2 (and IMPLEMENTATION_STATUS) in the same change.
- Statuses use: **done / in progress / planned-M*n* / post-1.0**. Nothing is ever
  marked "excluded".
- The milestone acceptance criteria are the release gates; version numbers may shift,
  criteria may not weaken without a documented decision here.
