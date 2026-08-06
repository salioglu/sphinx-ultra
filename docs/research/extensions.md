# Sphinx Extension Support Research for sphinx-ultra (mid-2026)

Scope: the ~15 extensions (beyond sphinx-needs) a Rust Sphinx reimplementation should support, with interface analysis and Rust implementation strategy. Popularity verified 2026-08-06 via pypistats.org (last-month downloads) and the GitHub API (stars, activity, archived status). Sphinx core itself: **~89.0M downloads/month**, 7,957 stars — built-in `sphinx.ext.*` extensions ship inside that number and have no separate stats.

## Verified popularity data (third-party candidates, ranked)

| Package | DL/month (Jul–Aug 2026) | GitHub ★ | Status note |
|---|---:|---:|---|
| sphinx-design | 10.13M | 229 | active; official successor to sphinx-panels/tabs |
| sphinx-autodoc-typehints | 9.13M | 587 | active (tox-dev) |
| myst-parser | 8.11M | 883 | active (executablebooks) |
| sphinx-copybutton | 7.56M | 270 | active |
| sphinxcontrib-mermaid | 6.37M | 407 | active |
| sphinx-autobuild | 5.72M | 608 | active; moved to sphinx-doc org; ws-based rewrite |
| sphinxcontrib-spelling | 3.37M | 89 | active; CI-only niche |
| sphinx-tabs | 1.92M | 273 | maintained but superseded by sphinx-design |
| nbsphinx | 1.88M | 472 | active |
| breathe | 1.47M | 813 | active-ish (last push Jan 2026) |
| myst-nb | 0.94M | 239 | active |
| sphinx-notfound-page | 0.82M | 71 | active (readthedocs) |
| sphinxext-opengraph | 0.72M | 91 | active; adopted into sphinx-doc org |
| sphinx-sitemap | 0.66M | 63 | active |
| sphinx-gallery | 0.66M | 455 | active |
| sphinxcontrib-plantuml | 0.59M | 132 | active |
| sphinx-last-updated-by-git | 0.56M | 60 | active |
| sphinx-togglebutton | 0.48M | 47 | low activity |
| sphinx-favicon | 0.20M | 24 | low activity |
| sphinx-multiversion | 0.17M | 185 | semi-active |
| sphinx-hoverxref | 0.09M | 105 | **ARCHIVED Apr 2025** — replaced by Read the Docs Addons "Link previews"; drop from consideration |

Built-in popularity (no per-package stats; ranked from ecosystem usage — RTD config scrapes, default `conf.py` templates, `sphinx-quickstart` defaults): autodoc, napoleon, intersphinx, viewcode, mathjax, autosummary are near-universal in Python-project docs; todo, extlinks, autosectionlabel, graphviz, doctest common; ifconfig, coverage, duration, githubpages, linkcode, imgmath, inheritance_diagram long-tail.

---

## Selected 16 (the support target)

Legend: **NATIVE** = reimplement in Rust; **BRIDGE** = requires executing Python (pyo3 embed or sidecar RPC process); **HYBRID** = split. Difficulty S/M/L/XL.

### 1. sphinx.ext.autodoc — BRIDGE, XL

**What/why.** Generates API docs from live Python objects' docstrings. The single most load-bearing extension in the ecosystem; the reason most Python projects use Sphinx at all.

**Interface.** Directives: `automodule`, `autoclass`, `autoexception`, `autofunction`, `autodecorator`, `autodata`, `automethod`, `autoattribute`, `autoproperty` (produce nested `py` domain directives — autodoc emits generated rST that is re-parsed). Events it *emits* (other extensions hook these — napoleon, typehints depend on them): `autodoc-process-docstring`, `autodoc-process-signature`, `autodoc-before-process-signature`, `autodoc-process-bases`, `autodoc-skip-member`. Config: `autodoc_default_options`, `autodoc_member_order`, `autoclass_content`, `autodoc_class_signature`, `autodoc_typehints` (`signature`/`description`/`none`/`both`), `autodoc_typehints_format`, `autodoc_mock_imports`, `autodoc_inherit_docstrings`, `autodoc_preserve_defaults`. **Builder-phase: imports the user's Python code at read time** — needs the project's venv, `sys.path` manipulation, triggers arbitrary import side effects; uses `ModuleAnalyzer` (tokenizes source for attribute docs/comments `#:`).

**Rust strategy.** BRIDGE, XL. Two viable shapes: (a) sidecar CPython process in the project's venv exposing a "documenter RPC" (module path in → JSON of members, signatures, docstrings, source locations out), Rust renders the `py` domain output; (b) pyo3-embedded interpreter running real autodoc against a shim `sphinx` API. Sidecar is safer (venv isolation, crashes don't kill the build, parallelizable). A third option to note in the roadmap: static-analysis fallback à la griffe/sphinx-autoapi (no imports, pure parsing — could even be Rust-native via a Python parser crate) — attractive but not bug-compatible; offer it as an opt-in fast path, not the compat path. The docstring content still flows through the normal rST parser, so the Rust side must support re-parsing generated rST fragments with correct source mapping.

**Table stakes: YES** — #1 of the classic stack.

### 2. sphinx.ext.autosummary — BRIDGE, L

**What/why.** Companion to autodoc: `.. autosummary::` builds link tables of APIs and (with `:toctree:`) **generates stub `.rst` pages before the read phase** (`autosummary_generate=True`, `:recursive:` for whole package trees). Backbone of numpydoc-style API reference sites (NumPy, pandas, scikit-learn).

**Interface.** Directive `autosummary` (options `:toctree:`, `:recursive:`, `:nosignatures:`, `:template:`); role/shorthand `:obj:` resolution of short names; `builder-inited` event runs the stub generator (imports every listed object to introspect members); Jinja templates (`autosummary/module.rst`, `class.rst`, customizable via `templates_path`); config `autosummary_generate`, `autosummary_generate_overwrite`, `autosummary_ignore_module_all`, `autosummary_imported_members`. Phase interaction is nasty: it mutates the source tree (writes files) before reading, so incremental builds must treat generated stubs as derived artifacts.

**Rust strategy.** BRIDGE (rides the same sidecar as autodoc for introspection), L. Stub generation itself is templating — Rust (minijinja) can render once the sidecar returns the member tree. Signature-table extraction reuses autodoc RPC.

**Table stakes:** yes for API-reference-heavy projects (the whole scientific-Python stack); borderline otherwise.

### 3. sphinx.ext.napoleon — NATIVE, M

**What/why.** Parses Google/NumPy-style docstrings into rST field lists. Effectively mandatory because nobody writes raw `:param x:` fields anymore.

**Interface.** No directives/roles/nodes. Hooks `autodoc-process-docstring` (transforms the docstring line list in place) and `autodoc-skip-member`. Config: `napoleon_google_docstring`, `napoleon_numpy_docstring`, `napoleon_use_param`, `napoleon_use_rtype`, `napoleon_use_ivar`, `napoleon_attr_annotations`, `napoleon_preprocess_types`, `napoleon_custom_sections`, ~20 knobs total.

**Rust strategy.** NATIVE, M. It is a pure line-oriented text transform (`GoogleDocstring`/`NumpyDocstring` classes) with zero Python-runtime dependency — port to Rust and run it on docstrings returned by the autodoc sidecar *before* rST parsing. M not S because of two grammars, section aliasing, and config-conditional output. This is the poster child for HYBRID economics: keep introspection in Python, do text transforms in Rust.

**Table stakes: YES.**

### 4. sphinx-autodoc-typehints — BRIDGE, M (9.13M DL/mo)

**What/why.** Moves PEP 484 annotations out of signatures into `:type:`/`:rtype:` description fields. Still outranks most of the ecosystem in downloads even though core `autodoc_typehints="description"` covers ~80% of it.

**Interface.** Hooks `autodoc-process-signature` (strips annotations) and `autodoc-process-docstring` (injects type fields); calls `typing.get_type_hints()` at build time — **must execute in the project interpreter** to resolve forward refs/`from __future__ import annotations`. Config: `always_document_param_types`, `typehints_fully_qualified`, `typehints_document_rtype`, `typehints_use_signature(_return)`, `typehints_defaults`, `typehints_formatter` (a Python callable — bridge-only feature).

**Rust strategy.** BRIDGE, M — a thin layer on the autodoc sidecar: have the sidecar resolve hints and return structured types; formatting/injection can be Rust. Cheapest path: ship core `autodoc_typehints="description"` parity first and offer this as compat mode.

**Table stakes:** near — very common in typed-Python projects.

### 5. sphinx.ext.intersphinx — NATIVE, M

**What/why.** Cross-project linking via `objects.inv` inventories. The connective tissue of the whole ecosystem; virtually every serious project maps `python`, `numpy`, etc.

**Interface.** Config `intersphinx_mapping`, `intersphinx_cache_limit`, `intersphinx_timeout`, `intersphinx_disabled_reftypes`; loads inventories at `builder-inited` (HTTP fetch + local cache); hooks `missing-reference` (fires after local resolution fails) to resolve `pending_xref` nodes; provides `:external:domain:role:` explicit roles. **Dual obligation:** sphinx-ultra must also *emit* a spec-compatible `objects.inv` (zlib-compressed v2 format) from its own builds or the rest of the ecosystem can't link back.

**Rust strategy.** NATIVE, M. Inventory format is trivial (header + zlib lines); the work is the resolution precedence rules, caching, and a correct `missing-reference`-equivalent hook point in the Rust resolver so *other* extensions (incl. bridged ones) can participate.

**Table stakes: YES.**

### 6. sphinx.ext.viewcode — HYBRID, M

**What/why.** `[source]` links from API docs to highlighted `_modules/` source pages.

**Interface.** Hooks `doctree-read` (walks `py` domain signatures, records module→fullname tags via `ModuleAnalyzer`, which locates module files through the import system and parses source — needs `viewcode_follow_imported_members`), `env-merge-info`/`env-purge-doc` (parallel-build bookkeeping), `html-collect-pages` (emits the extra highlighted pages at write phase — a builder API sphinx-ultra must expose: extensions adding non-doctree output pages), `missing-reference` for epub. Config: `viewcode_follow_imported_members`, `viewcode_line_numbers`, `viewcode_enable_epub`.

**Rust strategy.** HYBRID, M. Sidecar returns file path + line ranges per documented object (it already has them for autodoc); Rust renders the source pages with its own highlighter and wires the back-and-forth links. Requires the "extra pages" hook in the HTML builder.

**Table stakes: YES** (part of the classic stack).

### 7. myst-parser — NATIVE, XL (8.11M DL/mo)

**What/why.** Markdown (CommonMark + MyST extensions) as a first-class Sphinx source language. The default choice for new projects and the entire Jupyter Book ecosystem; a Sphinx reimplementation without Markdown is dead on arrival in 2026.

**Interface.** Registers a source parser for `.md` via `source_suffix`; produces docutils AST directly (no rST intermediate). Syntax surface: full CommonMark; ` ```{directive} ` fenced and `:::` colon-fence directives (arbitrary Sphinx directives — including bridged ones — must be invocable from Markdown); `{role}`content`` roles; front-matter metadata; targets `(name)=`; config `myst_enable_extensions` (`colon_fence`, `deflist`, `dollarmath`, `amsmath`, `linkify`, `substitution`, `tasklist`, `attrs_inline`, `attrs_block`, `fieldlist`, `html_image`, `replacements`, `smartquotes`, `strikethrough`), `myst_heading_anchors`, `myst_substitutions` (Jinja), `myst_url_schemes`, `myst_all_links_external`, `myst_number_code_blocks`, `myst_footnote_transition`.

**Rust strategy.** NATIVE, XL. Build on a markdown-it-compatible Rust parser (`markdown-it.rs` exists and mirrors the token/plugin architecture myst uses in Python) and map tokens → the same internal doctree the rST parser produces. The hard part is directive/role dispatch parity and source-location fidelity for diagnostics. This is arguably sphinx-ultra's second parser front-end, not "an extension."

**Table stakes: YES.**

### 8. sphinx-copybutton — NATIVE, S (7.56M DL/mo)

**What/why.** Copy-to-clipboard button on code blocks. Ubiquitous because it's free UX.

**Interface.** No directives/nodes. `builder-inited`/setup: `app.add_js_file` (`copybutton.js` + bundled `clipboard.min.js`), `app.add_css_file`; passes config into JS via a generated fragment. Config: `copybutton_prompt_text`, `copybutton_prompt_is_regexp`, `copybutton_only_copy_prompt_lines`, `copybutton_remove_prompts`, `copybutton_copy_empty_lines`, `copybutton_selector`, `copybutton_exclude` (default `.linenos, .gp`), `copybutton_image_svg`.

**Rust strategy.** NATIVE, S. Vendor the JS/CSS, template the config into a JS snippet, register assets. Requires only a generic "extension adds static assets + script config" mechanism. Ideal first native extension.

**Table stakes: YES.**

### 9. sphinx-design — NATIVE, L (10.13M DL/mo — highest third-party)

**What/why.** Responsive web components: grids, cards, dropdowns, tabs, badges, buttons, icons. The official successor to sphinx-panels *and* sphinx-tabs; pydata-sphinx-theme/furo ecosystems assume it. Highest-downloaded third-party extension.

**Interface.** Directives: `grid`, `grid-item`, `grid-item-card`, `card`, `card-carousel`, `dropdown`, `tab-set`, `tab-item`, `article-info`, `button-link`, `button-ref`; roles: `bdg-*` (badge variants), `octicon`, `material-regular` etc. icon roles. Output is plain container nodes with Bootstrap-ish CSS classes + one compiled CSS file and a small JS file for synchronized tabs (`sd_tab_set` sync keys). No build-time computation, no events beyond asset registration; `button-ref` participates in normal xref resolution.

**Rust strategy.** NATIVE, L. Mechanically simple (nested-directive parsing → classed containers) but wide: dozens of directives × many options × responsive class grammar (`:columns: 12 6 4 4`) + shipping/versioning the CSS. Doing this natively also gives sphinx-tabs users a migration target.

**Table stakes:** rapidly becoming yes for modern themes; not part of the minimal classic stack.

### 10. sphinx.ext.mathjax — NATIVE, S

**What/why.** Math rendering. `math` role/directive parsing is docutils/Sphinx core; this extension just renders via MathJax in HTML. On by default in most themes/projects with any math.

**Interface.** Registers HTML visitors for `math`/`math_block` nodes; injects MathJax `<script>` (CDN by default) only on pages containing math (`html-page-context` interaction); config `mathjax_path`, `mathjax3_config`/`mathjax_options`, `mathjax_inline`/`mathjax_display` delimiters. Runner-up sibling `imgmath` shells out to LaTeX — skip initially.

**Rust strategy.** NATIVE, S. Node visitor + conditional asset injection. Optional differentiator later: build-time KaTeX/typst server-side rendering for zero-JS pages.

**Table stakes: YES** for any scientific project.

### 11. sphinxcontrib-mermaid — NATIVE (core), S–M (6.37M DL/mo)

**What/why.** Mermaid diagrams — the de facto diagram syntax of 2026 (GitHub/GitLab native support drove adoption).

**Interface.** Directive `mermaid` (inline code or `:file:`), plus `autoclasstree` (imports Python classes to draw inheritance — bridge-only, rarely used). Config: `mermaid_output_format` (`raw` = client-side render; `png`/`svg` = shell out to `mmdc` CLI at build time), `mermaid_version`, `mermaid_init_js`, `mermaid_params`, `mermaid_use_local`, `mermaid_d3_zoom`. Raw mode = emit `<pre class="mermaid">` + inject mermaid.js + init script.

**Rust strategy.** NATIVE, S for raw client-side mode (vendor mermaid.js for offline/CSP builds); M to add the `mmdc` subprocess path for LaTeX/PDF output; mark `autoclasstree` unsupported or route via the autodoc sidecar.

**Table stakes:** no, but extremely popular; cheap win.

### 12. sphinx.ext.graphviz — NATIVE, M

**What/why.** `graphviz`/`graph`/`digraph` directives rendering DOT at build time. Common in architecture-heavy docs; also the substrate for `inheritance_diagram`.

**Interface.** Directives with options (`:layout:`, `:caption:`, `:align:`, `:class:`); config `graphviz_dot`, `graphviz_dot_args`, `graphviz_output_format` (`png` w/ image maps for links, or `svg`). **Build-time subprocess** (`dot`), content-hash caching of outputs into `_images/`.

**Rust strategy.** NATIVE, M — subprocess orchestration + hashing + cache is standard; alternatively link a pure-Rust graphviz layout crate later. `inheritance_diagram` (runner-up) layers class introspection on top → needs the bridge.

**Table stakes:** no, but expected of a "complete" Sphinx.

### 13. sphinx.ext.extlinks — NATIVE, S

**What/why.** Config-defined URL-shortening roles (`:issue:`123``). Tiny, everywhere.

**Interface.** Config `extlinks = {name: (url_pattern_with_%s, caption_pattern)}`; registers one role per entry at setup; `extlinks_detect_hardcoded_links` warns on literal URLs that match a pattern (a post-transform over reference nodes).

**Rust strategy.** NATIVE, S. Requires runtime role registration from config — a generic capability sphinx-needs also relies on.

**Table stakes:** effectively yes (near-zero cost).

### 14. sphinx.ext.autosectionlabel — NATIVE, S

**What/why.** Makes every section title referenceable via `:ref:` without manual labels. Extremely common in prose-heavy docs.

**Interface.** Hooks `doctree-read`; registers each section title as a `std` domain label; config `autosectionlabel_prefix_document` (dedupe by `doc:Title`), `autosectionlabel_maxdepth`. Known duplicate-label warning behavior must be replicated.

**Rust strategy.** NATIVE, S — a resolver-phase pass over the doctree writing into the label table.

**Table stakes:** yes-adjacent; trivial.

### 15. sphinx.ext.todo — NATIVE, S

**What/why.** `todo`/`todolist` directives. Moderately used directly, but **architecturally important**: it is the canonical "collector" extension (custom node + env storage + `doctree-resolved` aggregation + parallel merge) — the same machinery sphinx-needs, `sphinx.ext.ifconfig`, and dozens of third-party extensions use.

**Interface.** Nodes `todo_node`, `todolist`; events `doctree-resolved` (expand todolist, back-link to origins), `env-purge-doc`, `env-merge-info`; config `todo_include_todos`, `todo_emit_warnings`, `todo_link_only`.

**Rust strategy.** NATIVE, S — implement it as the proving ground for the generic collect/aggregate API.

**Table stakes:** no; strategic.

### 16. nbsphinx (with myst-nb as the alternative) — HYBRID, XL (1.88M / 0.94M DL/mo)

**What/why.** Jupyter notebooks as doc pages, optionally executed at build time. Core of scientific/ML documentation.

**Interface (nbsphinx).** Source parser for `.ipynb`; **executes notebooks during read via jupyter kernels** (`nbsphinx_execute` = `auto`/`always`/`never`, `nbsphinx_allow_errors`, `nbsphinx_timeout`, `nbsphinx_kernel_name`); converts Markdown cells **via pandoc** (external binary!); Jinja-templated `nbsphinx_prolog`/`nbsphinx_epilog`; injects cell CSS; thumbnail/gallery support; special `nbsphinx-toctree` cell metadata. myst-nb instead parses md cells with myst-parser and adds `jupyter-cache`, `{code-cell}` directives in `.md` notebooks, and glue roles.

**Rust strategy.** HYBRID, XL overall, but with a valuable native subset: **`.ipynb` is JSON — parsing cells and rendering *pre-executed* outputs (text, images, HTML) is fully native (M)**, which covers the very common `execute=never`/pre-run-in-CI workflow. Execution mode requires a Jupyter sidecar (bridge). Prefer the myst-nb model (md cells through the native MyST parser → no pandoc dependency).

**Table stakes:** yes within scientific Python; no elsewhere.

---

## Runners-up (support later, or via bridge/compat shim)

| Extension | DL/mo | Verdict for sphinx-ultra |
|---|---:|---|
| **breathe** (Doxygen/C++) | 1.47M | Strongest runner-up; XL. Better plan: native Doxygen-XML reader (it's just XML → C++ domain nodes — no Python runtime needed, NATIVE/XL) targeting the same directives (`doxygenclass` etc.). Big strategic audience (C++/embedded) for a fast builder; slipping maintenance of breathe/upstream is an opening. |
| **sphinx-tabs** | 1.92M | Superseded by sphinx-design; ship a thin native compat shim (S) mapping `tabs/tab` → design tab-set rendering. |
| **sphinxcontrib-spelling** | 3.37M | CI-only spell-check builder (PyEnchant). Native alternative: integrate a Rust spellchecker as a lint mode (M). Not a rendering concern. |
| **sphinx-sitemap** | 0.66M | NATIVE S — `html-page-context`/build-finished pass writing `sitemap.xml` from `html_baseurl` + page list. Easy early win. |
| **sphinxext-opengraph** | 0.72M | NATIVE S/M — per-page meta tags via `html-page-context` (first-paragraph/description extraction, `og:image`, optional social-card image generation = M). Easy win. |
| **sphinx-notfound-page** | 0.82M | NATIVE S — extra 404 page with absolutized URLs. |
| **sphinx-togglebutton** | 0.48M | NATIVE S — assets + `toggle` directive/class. |
| **sphinx-favicon** | 0.20M | NATIVE S — `<link rel>` injection from config. |
| **sphinx-last-updated-by-git** | 0.56M | NATIVE S/M — per-file `git log` timestamps feeding `html_last_updated_fmt`; note it's a dependency of sphinx-sitemap 2.9. |
| **sphinxcontrib-plantuml** | 0.59M | NATIVE M — subprocess to plantuml.jar, same shape as graphviz. |
| **sphinx-gallery** | 0.66M | BRIDGE XL — *executes example scripts*, scrapes matplotlib figures; defer; myst-nb path covers much of the need. |
| **sphinx-multiversion** | 0.17M | Orchestrates builds across git refs; better solved as a first-class sphinx-ultra CLI feature than as extension compat. |
| **sphinx-hoverxref** | 0.09M | **Skip — archived Apr 2025**, functionality absorbed by RTD Addons "Link previews". |
| Built-ins: **doctest** (BRIDGE M — runs Python snippets), **linkcode** (BRIDGE S — user callback in conf.py), **inheritance_diagram** (BRIDGE M — imports classes, layers on graphviz), **coverage**, **duration** (NATIVE S — build-timing report), **githubpages** (NATIVE S — emit `.nojekyll` + CNAME), **ifconfig** (NATIVE S), **imgmath** (subprocess LaTeX, M) | — | All feasible later; none gate adoption. `duration`/`githubpages`/`ifconfig` are near-free natively. |

## What the ecosystem considers table-stakes

For a typical Python project's docs, the assumed stack is: **autodoc + napoleon + intersphinx + viewcode + autosummary + mathjax (built-ins) and myst-parser + sphinx-copybutton (third-party)** — with sphinx-autodoc-typehints (or `autodoc_typehints="description"` parity) and sphinx-design close behind, and sphinx-autobuild assumed as the dev loop. RTD-deployed projects additionally assume sitemap/opengraph/notfound-page-level SEO plumbing. A reimplementation that renders rST beautifully but lacks autodoc+napoleon+intersphinx will not be adopted by Python projects; one that lacks myst-parser will not be adopted by new projects of any language.

## Dev-server story (sphinx-autobuild equivalent — 5.72M DL/mo)

Current sphinx-autobuild (post-2024 rewrite, now under sphinx-doc org) = `watchfiles` + Starlette/uvicorn static server + websocket-injected hot reload, rebuilds via subprocess `sphinx-build`. What a built-in `sphinx-ultra serve` needs from the builder:

1. **In-process incremental build API** — no CLI re-exec; a callable "rebuild(changed_paths) → BuildReport" with millisecond no-op builds. This is the headline advantage Rust can deliver (Vite-like DX vs. sphinx-autobuild's multi-second loops).
2. **Accurate dependency graph** — per-document dependencies (includes, images, templates, `conf.py`, extension-registered `note_dependency` files, autodoc'd Python modules via the sidecar) so a change dirties the minimal doc set; plus knowledge of global invalidators (toctree/labels/search index) to decide partial vs. full re-resolve.
3. **File watching** with debounce/coalescing (`notify` crate), ignore rules for the output dir and user `--ignore` globs, and watching of bridged-extension inputs (e.g., watched Python packages triggering autodoc refresh).
4. **Serve-time reload injection** — websocket client script injected into HTML *as served*, never written into the build output; reload on build-finished, with per-page reload when the dirty set is small.
5. **Diagnostics surface** — structured warnings/errors from the builder streamed to the browser as an overlay and to the terminal; build must not leave a corrupted output tree on failure.
6. **Ergonomics parity**: port selection/`--port 0`, `--open-browser`, `--pre-build` hooks, `--host`, graceful venv/kernel sidecar reuse across rebuilds (don't restart the Python bridge per rebuild — keep it warm).

## Cross-cutting architectural implications

- The 16 selections force exactly four platform capabilities, in priority order: (1) **asset/registration API** (add_js/css, html-page-context, extra pages) — unlocks copybutton, mathjax, mermaid, design, sitemap, opengraph, favicon, togglebutton; (2) **event + collector + transform pipeline** with parallel merge semantics — todo, autosectionlabel, extlinks, intersphinx hook points, sphinx-needs; (3) **Python sidecar RPC** in the project venv — autodoc, autosummary, typehints, viewcode, doctest, linkcode, notebook execution; (4) **second parser front-end** (MyST) sharing the doctree/directive registry.
- Difficulty totals: NATIVE items are mostly S/M with two L (design) and one XL (myst-parser); the bridge is one XL platform investment (autodoc sidecar) that then makes four more extensions M-or-less.
- Emit-side compatibility matters as much as consume-side: `objects.inv`, `searchindex.js`-equivalent, and stable anchor/permalink generation are what let sphinx-ultra output interoperate with the surviving Python ecosystem.

Sources: [pypistats.org](https://pypistats.org) per-package pages (linked per row, e.g. [sphinx-design](https://pypistats.org/packages/sphinx-design), [myst-parser](https://pypistats.org/packages/myst-parser), [sphinx-autobuild](https://pypistats.org/packages/sphinx-autobuild)), GitHub API repo metadata (stars/pushed_at/archived, 2026-08-06), [sphinx-hoverxref README (deprecation)](https://github.com/readthedocs/sphinx-hoverxref/blob/main/README.rst), [sphinx_design PyPI (panels/tabs succession)](https://pypi.org/project/sphinx_design/), [sphinx-autobuild NEWS (2024 rewrite)](https://github.com/sphinx-doc/sphinx-autobuild/blob/main/NEWS.rst), [sphinx-favicon docs](https://sphinx-favicon.readthedocs.io/).