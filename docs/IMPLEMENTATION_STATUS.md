# Implementation Status

**Audit-verified status as of 2026-10-27** (v0.6.0: M2 waves 1–4.5, plus the Sphinx-format `searchindex.js` / `objects.inv` / `.buildinfo` writers and the HTML differential oracle that 0.6.0 added — PRs #61, #62, #63, #68, #69). Re-audited for 0.6.0 by tracing the binary's call graph and running it against the fixture projects.
**Updated 2026-11-24 for M2 wave 5a-1** (PR #83, read-side parity): docutils' and Sphinx's read transforms, the reporter channel with Sphinx's warning-record grammar, the citation and math domains, `only` evaluation and body toctree resolution in the resolve pass. The rows and the divergence list below say what changed; [the wave 5a-1 divergence list](#known-divergences-from-sphinx-910-m2-wave-5a-1) collects what is still open, by owner.
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

The plan to move everything to ✅ is [ROADMAP.md](ROADMAP.md).

## Core build pipeline

| Feature | Status | Evidence / gaps |
|---|---|---|
| File discovery w/ include/exclude patterns | ✅ (differentially verified) | `src/builder.rs` `discover_source_files`, `src/matching.rs`. `**` now translates to `.*` exactly like Sphinx 9.1 (wave 4); character-class emission is byte-identical to `sphinx.util.matching._translate_pattern` (incl. backslash doubling, `[]a]`/`[!]a]` edge cases). Verified by a committed 881-case differential fixture generated against sphinx 9.1.0 (`tools/gen_pattern_fixture.py`, `tests/pattern_differential.rs`) — zero divergence. Discovery keeps `include_patterns=['**']` and suffix-filters after matching, like Sphinx's `Project.discover`. Earlier 2026-08 fixes: `[!…]` → `[^/…]`, literal leading `^`, directory pruning. |
| Parallel orchestration | ✅ | rayon pool sized by `-j`/config. Per-file failures become `BuildErrorReport`s and the build continues (2026-08). |
| Incremental cache | ✅ | Fixed 2026-08: warm-cache rebuilds no longer deadlock (DashMap guard held across `alter` — found by the new E2E suite); hits write the rendered page; `--clean --incremental` produces a full tree (clean clears the cache); `max_cache_size_mb`/`cache_expiration_hours` plumbed; config changes invalidate via blake3 fingerprint; eviction honestly named least-accessed (LFU-style). M2 wave 4: staleness is now Sphinx's env-level computation, not mtime alone (below). **M2 wave 5a-1** bumped both persisted formats (`DOCTREE_FORMAT_VERSION` 2→5, `ENV_VERSION` 3→6) and the document-cache entry shape, so the first build after upgrading is a cold one. A document that is not re-read re-emits none of its read-phase records; one that is re-read emits all of them, in the cold build's order. Consistency and resolve-pass records are recomputed for every document on every build, and some warm builds re-read documents Sphinx keeps (decision 6, see the [divergence list](#known-divergences-from-sphinx-910-m2-wave-5a-1)). |
| Dependency graph / outdated computation | ✅ (M2 wave 4) | `build_dependency_graph`'s empty-vec TODO is gone. `BuildEnvironment::get_outdated_files` (`src/env/mod.rs`) is a port of Sphinx's: added ∪ changed ∪ removed documents, where "changed" consults `env.dependencies[docname]` — a dependency that is missing or newer than the document's read time makes the document outdated. `src/env/dependencies.rs` ports `note_dependency` and `relfn2path`. **M2 wave 4.5 ended the images-only limitation**: `include` and `literalinclude` call `note_dependency` on every member file they read, and `env.included` records the inclusion graph, so touching an included fragment re-reads the documents that include it. `docutils.conf` and gettext catalogs are still unmodelled. One deliberate non-dependency: a docutils **standard include** (`.. include:: <isonum.txt>`) records nothing, because sphinx's `Include.run` bypasses the path rewrite entirely for `<…>` targets (`other.py:410-412`) — the files are vendored, so there is nothing on disk to watch. That is unit-tested; the env oracle cannot reach it. Fixture `tests/fixtures/deps_image/` + `tests/e2e_cli.rs` cover the touch-an-image-and-rebuild path. Config-class (`rebuild='env'`) narrowing is deliberately not done: the whole-config `.config-fingerprint` wipes the cache on *any* config change, a strict superset that cannot under-rebuild. |
| RST parsing | ✅ (docutils-fidelity, wired) | **M2 wave 3 (2026-08-13): the binary runs the new parser** — `Parser::parse` → `src/rst/parse_rst_full` (sphinx mode); the M1 line-scanner is deleted. `src/doctree/` generic-node IR with byte-parity `pformat`; `src/rst/` block + inline grammar (waves 1–2) plus docutils-exact directive machinery (typed option converters with docutils-verbatim error texts, options-before-arguments evaluation order, rawsource literals, as-written names), the docutils built-in directive set (admonitions/topic/sidebar/rubric/quote-family/compound/container/parsed-literal/image/figure/code/math/raw/line-block/class/table/csv-table/list-table) and substitution definitions (replace/unicode/date, embedded directives, duplicate dupname semantics). Zero divergence on a committed 735-case fixture vs docutils 0.22.4 parse layer (`tests/doctree_differential.rs`). Sphinx-mode set (toctree, versionmodified family, seealso, code-block/sourcecode + highlight state, only, rst-class, math + equation targets, index, hlist, glossary, xref pending_xref anatomy, pep/rfc/cve/cwe) verified against a real sphinx-build 9.1.0 read-phase oracle (`tests/sphinx_doctree_differential.rs`). Document now derives title/toc (docutils `make_id` anchors)/labels/toctree entries/directive+role records from the doctree; the three builder raw-source re-scanners are gone. **M2 wave 4** added generic object-description anatomy (`desc`/`desc_signature`/`desc_name`/`desc_addname`/`desc_annotation`/`desc_content`, the `:no-index:`/`:no-index-entry:`/`:no-contents-entry:`/`:no-typesetting:` family, the `PropagateDescDomain` transform) and the std-domain directives on top of it — `program`, `option` (incl. `[=value]` and comma-separated multi-name forms), `envvar`, `confval` with `:type:`/`:default:`, `describe`/`object`, `default-domain`; plus glossary terms taking their ids from Sphinx's `make_id` (not docutils') and index entries following `process_index_entry` onto a list-valued attribute. **M2 wave 4.5** added the py domain's fourteen directives on that anatomy, the `include`/`literalinclude` family (see their own rows below), and a faithful port of `Glossary.run`'s line state machine — which fixed the entry split (terms separated by a blank line share one `definition_list_item`; a `.. ` comment does not split a multi-term entry) and made its three misformat diagnostics appear in the doctree (printed since M2 wave 5a-1). **M2 wave 5a-1** routed the bare object-description spellings through `primary_domain` (`.. function::` is `py:function`, `.. class::` the py domain's class), parses `only`, object-description and `py:module` content with titles allowed (`nested_parse_to_nodes(..., allow_section_headings=True)`), and makes the parse export its document state — ids, names, docutils' node registries, pending transforms and the reporter log with each message's creation-time text — for the read transforms (next row). The sphinx oracle now stands at **493 cases, zero divergence**, compared on the post-read doctree. Remaining deferrals, all in PR 5a-2: smart quotes, `.. role::`/`.. default-role::`, `rst_prolog`/`rst_epilog`, `contents`/`sectnum`/`meta` and the remaining structure directives. |
| Read transforms | ✅ (M2 wave 5a-1) | `src/transforms/` applies docutils' and Sphinx's read transforms in Sphinx's `(priority, serial)` order over an arena of the doctree (`TxTree`): the document-local ones in the parallel read (stage R: `PropagateTargets`, the anonymous, indirect, external and internal target transforms, substitutions, footnotes and citations, `AutoNumbering`, `ClassAttribute`, `DocInfo`, transitions, `DoctestTransform`, `GlossarySorter`, the dangling-reference check, …) and the environment-touching ones sequentially in sorted docname order (stage M: the domains, the collectors including images, `FilterSystemMessages` with `keep_warnings`). The persisted doctree is Sphinx's post-read tree. Evidence: the 493-case Sphinx read oracle; the 734 docutils snippets of the HTML oracle read under its own `conf.py`, post-read and dummy-resolved (`tests/sphinx_doctree_differential.rs`, both zero divergence, `KNOWN_POST_READ_GAPS` and `KNOWN_RESOLVED_STANDALONE_GAPS` empty); and a 99-case read-state oracle (`tests/read_state_differential.rs`, `tools/gen_read_state_fixture.py`) that pins every id `set_id` hands out with the transform that asked for it, the node registries after the parse, the final state-machine position and the warning stream — four rows left in `KNOWN_READ_STATE_GAPS`, one case's pending serials, owned by PR 5a-2's structure directives. The resolve pass adds `OnlyNodeTransform` (tags: `-t`, the builder's `html`/`format_html`/`builder_html`), `HighlightLanguageTransform`, `TrimDoctestFlags` and body toctree resolution with the dummy builder's `''` URIs. |
| Python domain (directives, registration, resolution) | ✅ (M2 wave 4.5) | All fourteen `py:*` directives (`module`, `currentmodule`, `function`, `class`, `exception`, `method`, `classmethod`, `staticmethod`, `attribute`, `property`, `data`, `decorator`, `decoratormethod`, `type`) on wave 4's object-description anatomy, with a real signature grammar: `src/py/expr.rs` is a Python expression parser plus an `ast.unparse` port (CPython 3.12 `_Unparser`'s precedence table and paren placement) for annotations (parameter defaults go through a port of `sphinx.pycode.ast.unparse` instead, which keeps a literal's source text — `0x10` stays `0x10` — a split `src/py/arglist.rs`'s header calls a trap), `src/py/arglist.rs` ports `_parse_arglist` / `_parse_type_list` / `pseudo_parse_arglist` (PEP 695 type-parameter lists included), and `src/py/annotations.rs` ports `_parse_annotation` / `type_to_xref` / `parse_reftarget`. Registration into `domaindata['py']` (`src/env/py_domain.rs`): insertion-ordered objects and modules, aliased entries, `duplicate object description of %s, other instance in %s, use :no-index: for one of them`, `more than one target found for cross-reference %r: %s`, and the `any`-role variant. Resolution (`src/env/resolve.rs`) covers `:py:func:`/`:py:class:`/`:py:meth:`/`:py:mod:`/`:py:attr:`/`:py:data:`/`:py:exc:`/`:py:obj:`/`:py:const:`/`:py:deco:` with sphinx's `refspecific` search order, the `builtin_resolver` fallback at priority 900, and `:any:` as the domainless role it actually is. Evidence: the sphinx doctree oracle's `py`/`pysig`/`pyconf` families (122 cases) and the env oracle's py projects, both zero divergence. The bare `.. function::`/`.. class::`/… spellings that Sphinx's default `primary_domain = 'py'` accepts reach the domain since M2 wave 5a-1 (`primary_domain` is a real key). |
| Object-signature config family | ✅ (M2 wave 4.5) | Nine keys, plumbed from `conf.py`/YAML/`-D` into the parser as `PySigConfig` (`src/py/mod.rs`): `maximum_signature_line_length`, `python_maximum_signature_line_length`, `python_trailing_comma_in_multi_line_signatures`, `python_display_short_literal_types`, `python_use_unqualified_type_names`, `toc_object_entries`, `toc_object_entries_show_parents`, `add_function_parentheses`, `add_module_names` (plus `strip_signature_backslash`). Pinned by 33 `pyconf` oracle cases carrying per-case `confoverrides`, and by `toc_object_entries_match_the_probe_for_all_five_config_variants` in the env suite. One deliberate divergence: an out-of-enum `toc_object_entries_show_parents` is accepted with a warning whose candidate list is in registration order, because sphinx's own list comes out of a hash-ordered set and is therefore not byte-reproducible. Panel fix round B added the `check_confval_types` check for the two `int \| None` keys (`maximum_signature_line_length`, `python_maximum_signature_line_length`): a `-D …=20` — a *string* under Sphinx, because `convert_overrides` never coerces a `None`-default key — or a mistyped `conf.py` literal warns ``The config value `…' has type `str'; expected `NoneType' or `int'.`` byte-exactly, in Sphinx's registration order, and leaves the key unset; Sphinx warns identically and then crashes on the first signature. |
| `include` / `literalinclude` | ✅ (M2 wave 4.5) | Built on a new per-line provenance layer (`SourceTable` + `SpliceRequest`, `src/rst/block.rs` + `src/rst/lines.rs`): every line carries the source it came from, so a warning inside an included file points into that file. `include` covers the whole docutils option set (`:literal:`, `:code:`, `:number-lines:`, `:encoding:`, `:tab-width:`, `:start-line:`/`:end-line:`/`:start-after:`/`:end-before:`, `:class:`/`:name:`), the sphinx path rewrite (a leading `/` is srcdir-relative), circular-inclusion detection with sphinx's multi-line chain message, and the 35 vendored docutils standard include files (byte-identical to the pinned wheel). `literalinclude` ports sphinx's reader filter chain in order — `:lines:`, `:start-after:`/`:end-before:`/`:start-at:`/`:end-at:`, `:pyobject:`, `:prepend:`/`:append:`, `:dedent:`, `:diff:` (a difflib port), `:emphasize-lines:`, `:lineno-match:`/`:linenos:`/`:lineno-start:`, `:tab-width:`, `:encoding:`, `:caption:`/`:name:`/`:class:`/`:language:`/`:force:` — with every reader error funnelled into the single reporter warning sphinx emits. `:pyobject:` is a port of `sphinx.pycode`'s `DefinitionFinder` tokenizer, checked differentially against the real one over 1200 real modules (24,903 tags, 0 mismatches). `source_encoding` (default `utf-8-sig`) is a real key and the default `:encoding:` of both directives (panel fix round B) — and only that: this crate's own `.rst` sources are still decoded as UTF-8, where Sphinx passes the key to docutils as `settings.input_encoding` for the document read too. The diagnostics both directives raise — a missing or unreadable file, the refused `:parser:`, a circular inclusion — are docutils reporter messages, and since M2 wave 5a-1 they are printed on stderr and in the `-w` file like every other reporter record (a SEVERE prints as `CRITICAL`), so `-W` fails on a broken include path as `sphinx-build -W` does. `csv-table` and `raw` `:file:` arguments are recorded as dependencies too. |
| Markdown parsing | ❌ | Only `Event::Text` survives pulldown-cmark; headings/code/lists/tables discarded; `.md` titles/TOCs always empty; front matter TODO. |
| HTML rendering | 🔴 | `builder.rs` "Simple document rendering (placeholder)": each page is `<html><body>{escaped raw source}</body></html>`, produced in the read phase so the cache can store it; the resolved doctree, numbering and xref results never reach the page. No layout, title, ids, links, toctree expansion or highlighting. **The gap is now measured**: the HTML differential oracle (0.6.0, `tests/html_differential.rs`, 1,264 core + 142 sphinx-needs reference builds by real Sphinx 9.1.0) is `#[ignore]`d and red by design — none of the 1,260 built core cases passes until the writer lands (M2 wave 5b; the one passing core case is the reference build error, compared on its exit class and warnings). Since M2 wave 5a-1 the non-page outputs are gated on every test run instead: `html_oracle_aux_gate` compares the warning stream, the exit class, `objects.inv` and `searchindex.js` of all 1,261 runnable core cases strictly, and all four match except the warning stream of three image-scale cases the writer owns (`KNOWN_AUX_GAPS`, 3 rows, 5b). 5b renders the pages with Sphinx 9.1.0's `basic` and alabaster 1.0.0 templates through minijinja and deletes the M1 `HTMLBuilder`/`TemplateEngine` (below). |
| BuildEnvironment (read → merge → resolve → write) | ✅ (M2 wave 4) | `src/env/` replaces the never-constructed `src/environment.rs`. The build is now four phases over a real environment: a parallel read producing per-document doctrees (persisted as bincode under the `-d` dir, behind a `SUDT`+version header so an older format is an honest miss, `src/builder.rs`), a merge into a serialized `BuildEnvironment` (bincode, `ENV_VERSION`-stamped, `env.save`/`load`), a resolve pass per document over a *copy* of its doctree in docname order (mirroring Sphinx's `get_and_resolve_doctree` and therefore its warning order), and the write phase. Modules: `toctree.rs` (graph, `tocs`, `toctree_includes`, `files_to_rebuild`, relations, consistency warnings), `numbers.rs` (`toc_secnumbers`/`toc_fignumbers`), `std_domain.rs`, `genindex.rs`, `metadata.rs`, `dependencies.rs`, `resolve.rs`. |
| Environment differential oracle | ✅ (M2 wave 4; byte-exact since M2 wave 5a-1) | **Since M2 wave 5a-1 every exemption table is empty**: all 31 projects' warning streams and all 89 documents' resolved doctrees match byte-for-byte (`DOCUMENTED_*` in `tests/env_differential.rs`; `KNOWN_HIGHLIGHT_STAMP_GAPS` was deleted with the transform it waited for). The history below describes the wave-4.5 state. `tools/gen_env_fixture.py` builds 29 projects (84 documents; 31 and 89 since wave 5a-1) with a real `SphinxTestApp` + `app.build()` on sphinx 9.1.0 and records the post-build environment; `tests/env_differential.rs` (61 tests) replays each project through this crate and compares every key: `tocs`, `toc_num_entries`, `toctree_includes`, `files_to_rebuild`, `relations`, `toc_secnumbers`, `toc_fignumbers`, the std registries, index entries, genindex, the full warning stream, and each document's resolved-doctree pseudo-XML — **zero divergence**. Each corpus project is built exactly **once, cold** (`build_project` makes a fresh tempdir, never calls `enable_incremental`, and every corpus-wide assertion reads that single build); warm-equals-cold is a separate claim, asserted by hand-written tests in the same file over their own two- and three-document projects. Wave 4.5 added five compare keys (the py registries, `py_modindex`, `included`, and the file dependencies) and the py-domain and file-inclusion projects. Strict, self-cleaning exemption tables (`KNOWN_WARNING_GAPS`, `KNOWN_RESOLVED_GAPS`, `KNOWN_HIGHLIGHT_STAMP_GAPS`, `KNOWN_INERT_CONF`; `KNOWN_TOC_GAPS` and `KNOWN_STD_GAPS` are empty) name what is not yet compared and why: 24/29 projects' warning streams match byte-for-byte (3 differ only by `image file not readable`, 1 by the write-phase circular-toctree warning, 1 — `inc_basic` — because its whole oracle warning set is docutils *reporter* output that this crate keeps in-tree rather than streaming to stderr), and 35/84 resolved doctrees match byte-for-byte: 43 are skipped wholesale by `KNOWN_RESOLVED_GAPS` (an unresolved `toctree` node — wave 5's `_resolve_toctree` — an image without `candidates`, an unapplied `PropagateTargets`, or the `orphan` docinfo field list Sphinx removes) and 6 are compared in full except for the highlight stamp. Those figures are computed from the tables and the fixture by `exemption_arithmetic_matches_the_documented_numbers`, which first proves each table names a fixture document exactly once and the two document tables disjoint, so this sentence can no longer drift from the code (panel fix round B — the previous hand count was wrong in both directions). `KNOWN_HIGHLIGHT_STAMP_GAPS` is narrower than the others by construction: it still compares the entire tree and forgives nothing but the *presence* of a `HighlightLanguageTransform` attribute (`language`/`force`/`linenos` on a `literal_block`) that we do not emit at all — and never a `linenos="1"`, which in this corpus is directive-set by construction (the transform only ever stamps `"0"`; `inc_basic/b`'s `:lineno-match:` carries one). The `inc_highlight` project, in neither table, compares `language`/`force`/`linenos` set by `:language:`/`:linenos:`/`:lineno-start:`/`:force:` at full strength. Listing a project that has *stopped* diverging fails the test, so exemptions cannot outlive their cause; wave 4.5 also added the one-directional soundness rule that an exempted project's warnings must stay a SUBSET of the oracle's, so an exemption for a MISSING warning can never quietly cover an INVENTED one. |
| Toctree graph, relations, consistency warnings | ✅ (M2 wave 4) | Sphinx docname resolution (document-relative, `/`-absolute, `.`/`..`), `Title <doc>`, captions, URLs, `self`, `:glob:` with the dead-pattern warning, `:numbered:`/`:maxdepth:`/`:titlesonly:`/`:hidden:`/`:includehidden:`/`:reversed:`. The graph feeds `relations` (parents/prev/next, incl. Sphinx's quirk that a first child's `prev` is its parent) and the consistency warnings: nonexisting vs excluded entries, self-reference, circular toctrees, multiple parents (an *information* notice, not a warning), and `document isn't included in any toctree`. **Behavior change vs M1:** a toctree warning is now located at the `.. toctree::` directive line, as Sphinx locates it, not at the offending entry's line, and carries Sphinx's category suffix (`[toc.not_readable]`). Both changes are pinned by the env oracle and by `tests/e2e_cli.rs`. **M2 wave 5a-1** ported Sphinx's `_resolve_toctree`/`_toctree_entry` for body toctrees (`src/env/toctree.rs`): the resolve pass replaces each `toctree` node as Sphinx's dummy builder does, with `circular toctree references detected`, the excluded/nonexisting entry warnings and `only` evaluation inside tocs, which closed the 2026-10-27 gaps (the circular-toctree warning and the self-reference wording) and emptied `KNOWN_WARNING_GAPS`. The sidebar's global toctree, `collapse` and real URIs are the writer's (5b/5c). |
| Directive/role validation in the build | ✅ (native `build` only since M2 wave 5a-1) | **M2 wave 5a-1 turned it off in sphinx-build mode** (`validate_directives` is forced false there), because the reporter channel now prints docutils' and Sphinx's own diagnostics — `Unknown directive type`, `Unknown interpreted text role`, the option errors — and the M1 validators would add warnings `sphinx-build` never prints. The native `build` subcommand keeps them. Wired wave 4: runs on every native build (`validate_directives`, default on; `-D validate_directives=0` disables). Findings surface as warnings with file:line through the standard `-W`/`-w` pipeline. In native mode unknown directives/roles stay silent (10+10 validators cover a fraction of Sphinx). False-positive heuristics fixed/demoted: `.. note:: inline` is content not arguments; bare `code-block`, spaces/uppercase in `:ref:` labels, relative `:doc:` paths, kbd/menuselection styles all accepted. **M2 wave 4.5** audited all ten validators against the parser's own probe-verified `option_spec` tables, in both directions, after the env oracle caught one drift in the wild (`Unknown option 'lines'` on a `literalinclude`). Six more were found and fixed. Four invented warnings on markup `sphinx-build` accepts: `code-block`'s `force` and `class`, `figure`'s `figwidth`/`figclass` and `figname` (which warned under the *image* directive's name), and `image`/`figure`'s `loading`. One was latent — `include`'s `parser`/`class`/`name` were missing from a list nothing on the build path reads (`IncludeValidator::validate` checks only that an argument is present and non-empty — the file-extension heuristic beside it was itself a fabricated diagnostic, deleted in panel fix round B; the sole `valid_options()` consumer is the default `get_suggestions`, reached from `examples/` alone) — and one, `figure`'s `figname`, was a hole in the PARSER's own table that the audit compares against, closed in review round 1 (docutils `images.py:125`). Plus `literalinclude`'s `start-line`/`end-line`, which it advertised although sphinx's `LiteralInclude` has neither. Each list is now one shared const, and two mechanical tests keep the lists and the `validate` match in step for every validator. Panel fix round B removed four *fabricated* diagnostics on top of that (`Unusual file extension for include:`, the `… must be a positive integer` arms for `lineno-start`/`tab-width`/`dedent`, `Code-block directive has no content`, and `toctree`'s `maxdepth` range check — `-1` is the documented "unlimited"), widened the drift audit to negative and zero values, and pinned a probe-clean Sphinx project end-to-end to earn no validation warning. |
| std domain + cross-reference resolution | ✅ (M2 wave 4) | `src/env/std_domain.rs` collects labels (explicit targets, section/figure/table/code-block anchors with their titles), glossary terms, `option`s with program scoping and unscoped fallback, `envvar`s, and `confval`s; `src/env/resolve.rs` is a port of `ReferencesResolver` + `StandardDomain.resolve_xref` for `:ref:`, `:numref:`, `:doc:`, `:term:`, `:option:`, `:envvar:`, `:keyword:`, `:token:`. Warnings are Sphinx's own texts and categories — `duplicate label …, other instance in …`, `undefined label:`, `unknown document:`, `term not in glossary:`, `unknown option:`, `numfig is disabled. :numref: is ignored.`, `no number is assigned for …`, `the link has no caption: …`. **Behavior change:** these follow Sphinx's `warn_dangling` flags, which are set on seven std reftypes — `ref`, `numref`, `doc`, `term`, `keyword`, `option`, `confval` (`domains/std/__init__.py:748-766`) — regardless of `-n`, so a broken reference of any of those seven now warns in a default build; `-n`/`nitpicky` widens the warning to the remaining reftypes. `nitpick_ignore`/`nitpick_ignore_regex` are honored. **M2 wave 4.5 re-scoped the "skipping python-domain references" notice**: its python-domain population is gone (py references now resolve and warn), and the notice survives as `N cross-domain reference(s) not validated (domain not implemented until M5)`, printed for `:c:`/`:cpp:`/`:js:`/`:rst:` references, which remain unvalidated until those domains land (see *Diagnostics* under the known divergences). The old wording said "python-domain" but always counted every non-std domain. **M2 wave 5a-1** added the **citation** and **math** domains (`src/env/citation.rs`, `src/env/math.rs`: duplicate citations and equations, `:eq:` resolution, `Citation [X] is not referenced.`) and removed the M1 carve-out that exempted a URL-shaped `:doc:` target from `unknown document:`, which Sphinx does not have. |
| Section & figure numbering (`numfig`) | ✅ (M2 wave 4) | `src/env/numbers.rs`: section numbers from `:numbered:` toctrees respecting `numfig_secnum_depth`, then figure/table/code-block/`displaymath` numbering scoped by them, in Sphinx's order and with its alphabetical-domain `get_figtype` dispatch. `:numref:` renders through `numfig_format` (`{name}`/`{number}` new style and `%s` old style). Pinned by the env oracle's `toc_secnumbers`/`toc_fignumbers` keys and by the corpus's numfig projects. |
| Warning pipeline (`-W`, `-w`) | ✅ | Toctree, directive/role, environment and resolution warnings all flow through it; `-W` exits 1 with sphinx-build 9.1's exact behavior (collect-all; keep-going is the default since Sphinx 8.1). M2 wave 4 added Sphinx's warning **categories**: a warning logged with a `type` renders a ` [type.subtype]` suffix (`show_warning_types`, on by default since Sphinx 8.3); a `subtype`-only warning prints bare, like Sphinx's. **M2 wave 5a-1** made it one diagnostics stream with Sphinx's record grammar (`src/diagnostics.rs`): docutils reporter records print as Sphinx's `WarningStream` logs them (`source:line: LEVEL: text [docutils]`, a SEVERE as `CRITICAL`), with the creation-time text and docutils' own location for the node; each document's records are ordered by `(priority, serial, seq)` and documents in sorted docname order, the same at `-j 1` and `-j N`. In sphinx-build mode stderr carries the bytes of the `-w` file (no log prefix, no progress lines without `-v`), plus two notices of this crate's own that are not records — not in `-w`, not counted by `-W`: the `conf.py` evaluator's dropped-construct warnings (still with a log prefix) and the one-line `docutils.conf` NOTICE; `suppress_warnings` drops a record before it is printed or counted, and `show_warning_types` gates the suffix. |
| Error pipeline | ✅ | Per-file failures are collected as `BuildErrorReport`s while the build continues; a source that cannot be read or decoded exits 1 (deliberately stricter than sphinx-build), `-W`+warnings exits 1, usage errors exit 2 via clap (2026-08). **Since M2 wave 5a-1 docutils reporter records never change the exit status**, as in Sphinx: an ERROR or CRITICAL record exits 0 without `-W` and 1 with it (it counts as a warning there). |
| Static asset copying | 🟡 | Copies 5 handwritten shim files (incl. a 61-line fake jquery.js) + project `_static`/`_templates`; generated pages reference none of them; `html_static_path` ignored by the live path. |
| genindex data | ✅ (M2 wave 4) | `src/env/genindex.rs` ports `IndexDomain.process_doc` (5-tuple entries, `split_index_msg` validation with Sphinx's `invalid {type} index entry {value!r}` warning and node removal) and `IndexEntries.create_index` (single/pair/triple/see/seealso, `!main` promotion, Symbols and `_` grouping, insertion-ordered sub-entries, the dropped-entry notice). Compared against the oracle's `genindex` key for every corpus project. M2 wave 4.5 added the **py-modindex** data (`PythonModuleIndex.generate`: first-letter grouping after `modindex_common_prefix` stripping, the collapse flag, and the synopsis/platform/deprecated columns), compared against the oracle's `py_modindex` key. Neither has a renderer until the wave-5 HTML writer. |
| `searchindex.js` | ✅ (0.6.0) | Every build writes a Sphinx 9.1-format `Search.setIndex(...)` payload: `SphinxIndexBuilder` (`src/search.rs`) is a port of `IndexBuilder`, fed from each *resolved* doctree with the toctree expanded as Sphinx expands it (`resolve_toctree` in `src/env/toctree.rs`: maxdepth, titlesonly, tocdepth, hidden, URL/`self` entries), every `title` node, `:nosearch:`, comments and raw blocks, `split_index_msg` index entries, the `get_objects()` rows, and `json.dumps(sort_keys=True, ensure_ascii=True)` byte layout. English terms go through a port of Snowball's English stemmer with Sphinx's stopword and "stemmer must not remove words" rules, pinned by a 25,000-word fixture generated from the locked oracle profile (`tests/fixtures/search_english.tsv`). Since M2 wave 5a-1 it is fed the post-read trees with `only` evaluated, and it matches `sphinx-build` for all 1,261 runnable core HTML-oracle cases (`html_oracle_aux_gate`). Still missing: `search.html`, `_static/searchtools.js`/`language_data.js` (so the index is not usable in a browser yet), non-English languages. PRs #61, #62, #68 (@johnml1135). |
| `objects.inv` in the output tree | ✅ (0.6.0) | `generate_indices` writes a v2 inventory through `InventoryFile::dump` from the one `get_objects()` port (`src/env/objects.rs`) the search index also reads: every document as `std:doc`, the virtual `genindex`/`modindex`/`py-modindex`/`search` labels, std objects (labels, terms, options, envvars, confvals) and py objects, with Sphinx's `searchprio` table, glossary terms at priority `-1`, and percent-encoded `docname.html#anchor` targets. Records match real `sphinx-build` for every HTML fixture; the anchors point into pages that have no ids yet. |
| `.buildinfo` | ✅ (0.6.0) | Sphinx's layout with `config:` and `tags:` hashes from a port of `sphinx.util._serialise.stable_hash` over every html-category key — as the `conf.py` literal (`BuildConfig::conf_values`) or Sphinx's default — and `builder_html`/`format_html`/`html` plus `-t` tags. Byte-identical to `sphinx-build` for every HTML fixture incl. `-t custom`; fifteen single-setting `conf.py` variants pinned by a unit test. Nothing reads it back; the incremental decision uses `.config-fingerprint` and `env.bin`. The 87 built sphinx-needs oracle cases still differ on the `config:` line, because the extension's config-time contributions are not modelled. |
| `genindex.html` / `py-modindex.html` / `search.html` | 🔴 | The data behind all three is computed at Sphinx parity (rows above and below); no page is rendered. They land with the HTML writer. |
| Extension loading | 🔴 | Loading any extension fabricates a stub record and logs one line (at debug level since M2 wave 5a-1, so sphinx-build mode's stdout stays Sphinx's). Zero behavioral effect: an extension's directives and roles print docutils' `Unknown directive type`/`Unknown interpreted text role` ERRORs. (The never-used pyo3 dependency was removed 2026-08; Python interop arrives as a sidecar in ROADMAP M5.) |
| Build stats | 🟡 | `files_skipped` hardcoded 0; cache hits only counted under `--incremental` (default-on in sphinx-build mode). |
| `clean` / `stats` commands | ✅ | `stats` cross-ref count is naive substring counting. |

## Built-but-not-wired stack (the "second codebase")

These are real modules with passing tests, exported from `lib.rs`, with **no call
sites in the binary** — they run only from `examples/` and unit tests. M2 wave 4
shrank this list: what remains is the **write side**. It is **not** revived: M2
wave 5b renders pages with Sphinx 9.1.0's `basic` and alabaster 1.0.0 templates,
vendored verbatim and rendered by minijinja through a compatibility layer, and
deletes the M1 `HTMLBuilder`, `TemplateEngine`, `templates/` and `static/`
(roadmap decision U4, recorded 2026-11-22).

| Module | Status | Notes |
|---|---|---|
| `html_builder.rs` (Sphinx `StandaloneHTMLBuilder` mirror, 800 lines) | 🟡 | The direct path now writes the Sphinx-shaped search index, `objects.inv`, and `.buildinfo`; its broader template/index pipeline remains placeholder-grade and is not the live CLI path. |
| `template.rs` (minijinja engine + templates/) | 🧩 | User `templates_path` loading commented out ("lifetime issues"); `toctree()` returns an empty div; `pathto` ignores page depth; genindex/search templates use Python-only constructs (unregistered `_()`, `count.append(count.pop()+1)`) that fail at render time. Deleted in M2 wave 5b together with `html_builder.rs` (U4). |
| `search.rs` (in-memory index) | ✅ wired | The live path is a port of Sphinx 9.1's `IndexBuilder`, fed from each resolved doctree with the toctree expanded as Sphinx expands it (`maxdepth`, `titlesonly`, `tocdepth`, hidden toctrees, URL/`self`/virtual entries): every title node, `:nosearch:`, comments and raw blocks, the Snowball English stemmer with Sphinx's "stemmer must not remove words" fallback, `split_index_msg` index entries, and `json.dumps(sort_keys=True)` byte layout. The direct `HTMLBuilder` path shares the term pipeline and the object tables. Still differing from Sphinx: `html_search_language` other than English, and domains outside `std`/`py` (`only` nodes are evaluated since M2 wave 5a-1). |
| `inventory.rs` reader | ✅ wired (M2 wave 4) | Rewritten binary-safe (the old reader lossily UTF-8-converted the zlib payload and then split it on `str::lines`, corrupting real inventories); v1 + v2, `$`/`-` expansion, Sphinx's own `ValueError` texts. Live: intersphinx is the consumer. |
| `inventory.rs` writer (`InventoryFile::dump`) | ✅ wired | Live `SphinxBuilder` and direct `HTMLBuilder` paths write `objects.inv` through the same Sphinx-compatible writer, from the one `get_objects()` port (`env/objects.rs`) the search index also reads. As in Sphinx 9.1, glossary `term` records are written at priority `-1` (unsearchable, still resolvable), aliased Python objects likewise, and docnames are percent-encoded in their URIs; unsupported domains remain out of scope. |
| `environment.rs` (BuildEnvironment) | ✅ deleted (M2 wave 4) | 500 lines that were never constructed in the binary, with a `collect_relations` that returned an empty TODO. Replaced by `src/env/`, which the build actually runs (see the pipeline table). |
| `domains/` (Python + RST domain validation) | ✅ deleted (M2 wave 4) | The M1 heuristic layer: a regex reference scanner, a `DomainRegistry` of hand-registered names, fuzzy suggestions. Its live surface was replaced by the std domain (`src/env/std_domain.rs`) and Sphinx's resolution pass (`src/env/resolve.rs`), both oracle-pinned; the module then had zero call sites and went, along with `docs/DOMAIN_SYSTEM.md`, which documented only it. |
| `directives/validation/` (10+10 validators) | ✅ wired (wave 4) | Runs on every native `build` (see pipeline table); off in sphinx-build mode since M2 wave 5a-1. The `.. note:: inline text` false-positive class is fixed at the parser+validator level. |
| `validation/` (constraint engine) | 🧩 | Deliberately **not** wired in M1: nothing can produce `ContentItem`s until sphinx-needs item extraction exists (M4/M5) — wiring it now would validate an empty set. The always-success placeholder trait impls were deleted (wave 4) so future wiring can't silently no-op through the trait-method collision. Remaining: expression evaluator supports only `==`/`!=`/`in list`/`and`/`or`/`not`; no way to declare constraints in any config file. **Kept deliberately**: unlike `domains/`, nothing has replaced it — it is waiting for a producer, not for a rewrite. |
| `directives.rs` (HTML processor registry) | 🧩 | ~40 processors registered, 28 are stubs emitting HTML comments; `process_directive` has zero call sites (the never-used registry field was removed from `Parser` in wave 4); name-collides with the validation `DirectiveRegistry`. |
| `roles.rs` | ✅ deleted (M1 wave 4) | Was never declared in any module tree — 291 lines the compiler never saw. Role rendering arrives with the real pipeline in M2/M3. |

**Deletion policy** (why `domains/` and `environment.rs` went and `search.rs`,
`template.rs`, `html_builder.rs` and `validation/` stayed): a module is deleted when
something else has taken over its job and it has zero call sites. `domains/` and
`environment.rs` were both superseded by `src/env/`. `template.rs` and
`html_builder.rs` go in M2 wave 5b, when the vendored Sphinx templates take over
their job (U4); `validation/` has no replacement yet and waits for its producer.

## Configuration

| Feature | Status | Evidence / gaps |
|---|---|---|
| conf.py parsing | ✅ (declarative subset) | Rewritten 2026-08: logical-statement scanner + Python literal parser handles multi-line lists/dicts/tuples, nesting, string concatenation, triple-quoted strings, comments. **Every dropped construct warns** with `conf.py:line` — on stderr, outside the warning stream: not in `-w`, not counted by `-W`. Dynamic values (env vars, calls) still require the M5 sidecar. Half of `ConfPyConfig` (latex_*/epub_*/source_suffix/nitpick_*…) is declared but never populated (M2+ consumers). |
| YAML/JSON config | ✅ | Serde defaults across all config structs (2026-08): partial configs load; both shipped YAML examples verified by unit + E2E tests. |
| Config auto-detection order | ✅ | conf.py → yaml → yml → json → default. |
| `--config` flag | ✅ | Routes `conf.py`/`.py` to the Python config parser (2026-08); YAML/JSON as before. |
| Config knobs actually consumed | 🟡 | Consumed now: `max_cache_size_mb`, `cache_expiration_hours` (M1 wave 3); `nitpicky`, `validate_directives`, `doctree_dir`, `fail_on_warning`, `include/exclude_patterns`, `parallel_jobs` (M1 wave 4); `root_doc`/`master_doc`, `numfig`, `numfig_format`, `numfig_secnum_depth`, `nitpick_ignore`, `nitpick_ignore_regex`, `intersphinx_mapping`, `intersphinx_disabled_reftypes`, `intersphinx_resolve_self`, `intersphinx_cache_limit`, `intersphinx_timeout`, `tls_verify`, `tls_cacerts`, `user_agent` (M2 wave 4 — each with the same conf.py/YAML/`-D` plumbing and, for the intersphinx ones, Sphinx's own `ConfigError` texts on malformed input); `suppress_warnings`, `show_warning_types`, `keep_warnings`, `primary_domain`, `highlight_language`, `trim_doctest_flags`, `today`, `today_fmt` and `language` (image variants, `\|today\|`) (M2 wave 5a-1). A `docutils.conf` beside `conf.py` is not read; its presence prints one NOTICE line on stderr. Still decorative until their consumers land (M2 wave 5/M3): `html_theme`, `theme.*`, `output.syntax_highlighting`/`highlight_theme`/`minify_html`/`search_index`, `optimization.*`, `html_static_path`, `html_context`, `tags`. |
| `-D key=value` overrides | ✅ | Wave 4: typed coercion against the field's existing type, dotted paths for nested sections and map settings (`html_context.name=value`), duplicated-pair sync (`html_theme`, `templates_path`, `html_static_path`), unknown keys warn with sphinx-build's message and count toward `-W`/the `-w` file. Known gap: conf.py *parser* warnings still bypass the `-W` totals (config-diagnostics channel is M2). |

## CLI vs sphinx-build

| Capability | Status |
|---|---|
| `build --source/--output`, `-j`, `--clean`, `--incremental`, `-W`, `-w` | ✅ (relative `--source` crash fixed 2026-08) |
| Positional `SOURCEDIR OUTPUTDIR`, `-b html`, `-M html/clean`, `-D`, `-A`, `-n`, `-q`, `-E`, `-a`, `-c`, `-t`, `-T`, `--keep-going`, `-j auto`, repeatable `-v` | ✅ (wave 4) — sphinx-build compatible argument mode; parity measured against real sphinx-build 9.1.0 (exit codes, `-M` output layout, message shapes). Non-html builders and make-mode targets exit 2 with an honest message. Trailing FILENAMES accepted with a not-supported-yet warning. A source dir literally named `build`/`clean`/`stats` needs `./`-prefixing (documented). |
| Non-zero exit on build errors | ✅ exit 1 on build errors and `-W`+warnings (all warnings collected first, sphinx 9.1 behavior), 2 on usage/config/unsupported-builder errors. Since M2 wave 5a-1 a docutils reporter record — ERROR or CRITICAL included — never changes the exit status, as in Sphinx: 0 without `-W`, 1 with it. Deliberately **stricter** than sphinx-build only for a source that cannot be read or decoded: unreadable sources silently passing CI is the exact M1 trust problem, so that exits 1. sphinx-build mode also refuses an output dir that equals/contains the source dir (exit 1) and requires a config (exit 2), like sphinx-build. |
| `RUST_LOG` | ✅ pre-set `RUST_LOG` wins over `-v`/`-q` defaults (wave 4; was clobbered at startup) |
| `serve` | ⬜ does not exist (ROADMAP M3) |

## Infrastructure & release

| Area | Status | Notes |
|---|---|---|
| CI (fmt, clippy -D warnings, tests, audit, coverage, 3-OS) | ✅ | The MSRV and beta legs test what they name since PR #54 (2026-09-01: `RUSTUP_TOOLCHAIN` outranks `rust-toolchain.toml`). 2026-10-27: every third-party action pinned to a commit SHA, `Swatinem/rust-cache`, `cargo deny check` (licenses/bans/sources/advisories, exceptions with reasons in `deny.toml`) beside `cargo audit` in the required `Security Audit` job, a dependency-review gate on pull requests, a weekly scheduled audit (`audit.yml`) and an OpenSSF Scorecard run (`scorecard.yml`). The four required status checks keep their names. |
| E2E tests of the binary | ✅ | `tests/e2e_cli.rs` (2026-08): runs the real binary against fixture projects; asserts exit codes, warning text, output tree, `--config` routing. |
| Cargo.lock | ✅ | Committed (2026-08); CI/release/publish all run `--locked`. |
| Release workflow | ✅ | release-please (`release-please.yml`, manifest mode, `release-type: rust`) maintains the release PR from Conventional Commits, bumps `Cargo.toml`/`Cargo.lock`, writes `CHANGELOG.md` and creates the tag and GitHub release; it then calls `release.yml` (`workflow_call`), which validates tag vs `Cargo.toml` and the lockfile, builds six platform archives with `--locked` (musl via `cross`, aarch64 on `ubuntu-24.04-arm`), attaches SHA-256 checksums and a build-provenance attestation to the release, and publishes the crate. Attaching the archives and publishing the crate wait for approval in the `release` environment; a release made by hand runs `release.yml` through `workflow_dispatch`. `scripts/install.sh` verifies the checksum. Process: `docs/RELEASING.md`. |
| pyo3/pythonize | ✅ removed (2026-08) | Had zero call sites while linking libpython into every build (two RUSTSEC advisories, broken musl target, undocumented Python build dependency). Python interop returns as a venv **sidecar process** in ROADMAP M5 — not as a link-time dependency. |
| Unused dependencies | ✅ pruned (2026-08) | Removed: pyo3, pythonize, syntect, cssparser, minifier, tar, bincode, crossbeam, lru, config, glob, walkdir, indexmap, toml, ini, handlebars. Highlighting does not bring syntect back: M2 wave 5b ports Pygments 2.21.0 instead (roadmap decision U3), because byte parity needs Pygments' own token stream. M2 wave 4 re-added **bincode** (this time with call sites: environment + doctree persistence) and added **ureq** with rustls (intersphinx inventory fetching). `resolver = "3"` is set for that second one: edition 2021's default resolver would happily pick a `ureq` whose own `rust-version` exceeds ours, breaking `cargo install` on the MSRV we advertise — for other people, silently. |
| Repo hygiene | ✅ | Scaffold leftovers deleted; metadata (`rust-version = "1.85"`, keywords, categories, exclude) merged into `Cargo.toml`; CHANGELOG backfilled (0.2.0/0.2.1/0.3.0); SECURITY.md describes the real attack surface, including the outbound HTTPS the intersphinx work added (updated M2 wave 4). |

## Testing status

**1,371 tests, all passing** (M2 wave 5a-1, 2026-11-24; v0.6.0 had 1,125): 1,111 lib
+ 7 bin + 253 integration across fourteen targets, plus 6 `#[ignore]`d (the exhaustive
HTML oracle run and its diagnostics helper, the aux-gate census, the post-read and
resolved censuses, the read-state census); **0 of them are doc-tests**.
Every generator below is pinned to sphinx 9.1.0 / docutils 0.22.4 (the HTML oracle's
`local_needs` profile to docutils 0.21.2 and sphinx-needs 8.5.0) and asserts those
versions at runtime; each reproduces its committed output byte-identically. `PYTHONNOUSERSITE=1` is on every *live* regen command — the
five generators, the four `tests/*.rs` headers, the rows below and the two
emitted Rust headers — as of panel fix round E; the historical plan documents
under `docs/plans/` still quote the un-flagged forms they were
written with. The two *table* generators that emit Rust source
(`tools/gen_digit_tables.py`, `tools/gen_punctuation_tables.py`, and the
headers they write into `src/rst/digits.rs` and `src/rst/punctuation.rs`) were
the last holdouts; round D's note scoped them out rather than fixing them. Both
also need a `cargo fmt --all` after a regen, which their docstrings now say.

| Suite | Status |
|---|---|
| Unit tests (lib + bin) | ✅ 1,118 passing (1,111 lib + 7 bin) |
| Pattern compatibility tests | ✅ 10 passing — assertions encode Sphinx 9.1 semantics (M1 wave 4) |
| Pattern differential suite | ✅ 881 generated cases vs `sphinx.util.matching` 9.1.0, zero divergence; regenerate with `PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx>=9.1,<9.2' python tools/gen_pattern_fixture.py` |
| Doctree differential suite (docutils parse layer) | ✅ 735 generated cases vs docutils 0.22.4, zero divergence; regenerate with `PYTHONNOUSERSITE=1 uv run --python 3.12 --with docutils==0.22.4 python tools/gen_doctree_fixture.py` (the flag is not optional — `uv run` keeps user site-packages on `sys.path`, and a user-site Pygments there silently re-records every `code:: python` case as tokenized output) |
| Sphinx doctree differential suite (real read phase) | ✅ 493 generated cases vs a `sphinx-build` 9.1.0 read phase, compared on the post-read doctree since M2 wave 5a-1, zero divergence; plus the HTML oracle's 734 docutils snippets read under its own `conf.py`, post-read and dummy-resolved, zero divergence (`KNOWN_POST_READ_GAPS` and `KNOWN_RESOLVED_STANDALONE_GAPS` empty); 5 tests + 2 ignored censuses. Regenerate with `PYTHONNOUSERSITE=1 uv run --locked --python 3.12 --project tools/oracle_profiles/core python tools/gen_sphinx_fixture.py` (`--corpus html-oracle-docutils` for the snippets) |
| Read-state differential suite | ✅ (M2 wave 5a-1) 99 cases: the ids `set_id` hands out with the transform that asked, the node registries after the parse, the final state-machine position, the warning stream and the post-read pseudo-XML; 4 rows left in `KNOWN_READ_STATE_GAPS` (one case's pending serials, PR 5a-2's T18a/T18b). `tools/gen_read_state_fixture.py`, same locked-profile command |
| Environment differential suite | ✅ 61 tests over 31 projects / 89 documents vs a real `SphinxTestApp` build, zero divergence on every compared key, every exemption table empty since M2 wave 5a-1: 89/89 resolved doctrees and 31/31 warning streams byte-for-byte. The corpus comparison is over one **cold** build per project; warm-equals-cold is asserted by hand-written tests over their own two- and three-document projects. Same `uv` invocation with `tools/gen_env_fixture.py` |
| Inventory round-trip suite | ✅ 5 tests (3 `#[test]` + 2 `#[tokio::test]`, so a bare `#[test]` grep undercounts it) over 12 committed `.inv` files (4 sphinx-written, 3 handcrafted-valid, 5 handcrafted-malformed), expectations taken from Sphinx's own `InventoryFile.loads`; same `uv` invocation with `tools/gen_inventory_fixture.py` |
| English search-term oracle | ✅ 3 lib tests over `tests/fixtures/search_english.tsv` (25,000 words: Snowball stem + the term Sphinx finally indexes, or `-` if dropped), generated from the locked `tools/oracle_profiles/core` profile (sphinx 9.1.0, snowballstemmer 3.1.1) by `tools/gen_search_english_fixture.py`; `rust-stemmers` was rejected because it disagrees with snowballstemmer on 39 of 123,962 words |
| HTML differential oracle (`tests/html_differential.rs` + `tests/support/html_oracle.rs`) | ✅ 62 passing + 3 ignored. Since M2 wave 5a-1 `html_oracle_aux_gate` runs every runnable core case through the binary on each test run and compares the warning stream, the exit class, `objects.inv` and `searchindex.js` strictly against `KNOWN_AUX_GAPS` (3 rows: the warning stream of three image-scale cases, owned by the 5b writer); `html_oracle_aux_census` (ignored) prints the current set. The other non-ignored tests validate both committed ledgers (`tests/fixtures/html_oracle/{core,local_needs}/index.json`: schema, hashes, pins, discovery, an Ultra-to-Ultra smoke run) and the comparators; `html_oracle_exhaustive` (`#[ignore]`, red by design) runs every runnable case through the real binary and writes `target/html-oracle/report.{md,json}`. Corpus: 1,264 core cases (735 docutils + 489 sphinx snippets as one-document projects, 29 env projects, 7 HTML projects, 4 inventory projects; 1,260 built, 3 reference crashes, 1 build error) and 142 sphinx-needs `doc_test` projects (87 built, 49 excluded for PlantUML, 3 for network, 2 build errors, 1 reference crash), generated on Linux by the manual `html-oracle.yml` workflow. See `docs/testing.md` |
| Doctree serde / interner-cap suites | ✅ 4 passing — bincode round-trips of both doctree fixtures and (M2 wave 5a-1) of the node side-car, which keeps `rawsource` and `pending` and drops the read-stage `uid` and an escaped Text's `raw` data; and the interner's bound |
| Property tests (`tests/rst_proptest.rs`) | ✅ 14 passing — the parser never panics on arbitrary, multiline, multibyte or deeply nested input, and (wave 4.5) on arbitrary py and std object signatures, arbitrary annotations through `parse_annotation`, and arbitrary `include`/`literalinclude` option blocks and file arguments against a real scratch srcdir (the file-argument sweep draws control characters, newlines, absolute and `..` paths since panel fix round B, and pins totality only — sphinx reads whatever path `relfn2path` yields, so "never reads outside the project" is not a property either side has). Round 1 widened the std sweep's body generator, whose fixed ASCII lines at three fixed indents could not reach the `glossary` dedent branch that broke totality: it now draws an arbitrary indent over text carrying 2-, 3- and 4-byte characters. Green at `PROPTEST_CASES=2048` (14/14, 15.9s) |
| `tests/e2e_cli.rs` | ✅ 89 passing — the real binary against fixture projects: exit codes, warning text, output trees, `--config` routing, sphinx-build mode, incremental/dependency rebuilds, and (0.6.0) the `searchindex.js`/`objects.inv`/`.buildinfo` artifacts incl. `-D`/`-t` effects on the hashes; (M2 wave 5a-1) stderr equal to `-w`, record order at `-j 1` and `-j 4`, `-W` counting reporter and transform records, `suppress_warnings`, a touched document re-emitting its read records, CRLF sources |
| Benchmarks | ❌ `benches/builder_benchmark.rs` panics at line 69 (`No such file or directory`) — it hands the parser a `test.rst` path that does not exist, so `cargo test --all-targets` and `cargo bench` fail. Pre-existing and outside plain `cargo test`, which is why no wave caught it. The rest exercise the placeholder write path (numbers measure escaped-text copying) and the cache benchmark is `black_box(42)`. The rewrite, with regression gates, is M2 wave 5c's (roadmap decision U7). |

## Known divergences from Sphinx 9.1.0 (M2 wave 5a-1)

Wave 5a-1 (PR #83) is the first half of the read-side parity wave; PR 5a-2
(smart quotes, Sphinx's generic roles and read-side config, the structure
directives) and the writer waves 5b and 5c follow. "Decision *n*" is a scope
decision of the wave's plan.

### What a user notices

- **No M1 validation warnings in sphinx-build mode** (decision 7). The
  directive/role validators run only under the native `build` subcommand;
  sphinx-build mode prints what Sphinx prints instead.
- **docutils' diagnostics are printed** (decision 4, 8): every reporter
  record — `Unknown directive type`, `Unknown interpreted text role`, a
  missing include, `Unknown target name`, `Undefined substitution referenced`,
  footnote and citation errors — on stderr and in the `-w` file, as bare
  Sphinx record lines (`path:line: LEVEL: text [docutils]`, a SEVERE as
  `CRITICAL`). stderr equals the `-w` file except for two notices of this
  crate's own: the `conf.py` evaluator's dropped-construct warnings and the
  `docutils.conf` notice. Progress lines need `-v`.
- **Reporter ERRORs no longer change the exit status** (decision 4): exit 0
  without `-W`, 1 with it (every record counts), 2 on a fatal error. An
  unreadable or undecodable source still exits 1.
- **`suppress_warnings` and `show_warning_types`** work as in Sphinx: a
  suppressed record is neither printed nor counted, and
  `show_warning_types = False` drops the `[type.subtype]` suffix.
- **A `docutils.conf` beside `conf.py` is not read**; its presence prints
  one `NOTICE:` line on stderr, outside the warning stream (decision 16).
- **New persisted formats**: `DOCTREE_FORMAT_VERSION` 2→5, `ENV_VERSION`
  3→6 and a new document-cache entry shape. The first build after upgrading
  re-reads everything.
- **The `:doc:` URL carve-out is gone**: a URL-shaped `:doc:` target warns
  `unknown document:` as in Sphinx (decision 17).
- **Warm builds re-emit consistency and resolve-pass records for every
  document** (decision 6, owner 5c). A warm `-W` build with a dangling
  reference fails where Sphinx's passes, because Sphinx's HTML builder writes
  only outdated documents and skips `check_consistency` when nothing was
  re-read (on CPython a no-change warm build prints 386 resolve-phase lines).
  Some warm builds also re-read documents Sphinx keeps, so their read-phase
  records print again: document-cache entries older than
  `cache_expiration_hours` (24 h), a `-t` tag or any config change that is not
  `rebuild='env'` (the whole-config fingerprint), `-a`, and the toctree
  containers of a deleted document — which also print the read-time
  `toctree contains reference to nonexisting document` wording where Sphinx
  prints the write-time `non-existing` one.
- **Reporter records of included files spell the path srcdir-relative**,
  which is docutils' cwd-relative spelling only when the build runs from the
  source directory; logger warnings print the absolute path, as Sphinx does.
- **`:external:` role diagnostics are reported at resolve time** (decision
  17, owner 5c), after the read records, and an included file's ones print
  its absolute path where Sphinx prints `doc2path(docname):lineno`.
- **`|today|` is formatted in English only** (`today_fmt` with `language`
  other than `en` still uses English names); `SOURCE_DATE_EPOCH` with `_`
  digit separators falls back to now.
- **`rst_prolog`/`rst_epilog` are not applied until PR 5a-2** (T17b), but
  references are now resolved: a reference to a target or substitution
  defined there prints `Unknown target name` / `Undefined substitution
  referenced` (an ERROR, with a `problematic` node) where Sphinx resolves it,
  so a `-W` build of such a project fails until 5a-2.

### Open divergences, by owner

**PR 5a-2** (each task brings its own fixture cases):

- T16a: the `versionadded`/`versionchanged`/`deprecated`/`versionremoved`
  directives inline-parse their argument or joined content instead of
  nested-parsing the content, so constructs in it can print records Sphinx
  does not (CPython: two `Inline emphasis` at `c-api/contextvars.rst:13`) or
  miss a target (`jython`). Smart quotes are not applied (`smartquotes` is
  inert).
- T17a: XRefRole's `!` prefix is ignored for std roles (CPython: 298
  `unknown keyword`, 83 `unknown option`, 2 `term not in glossary`);
  `.. role::` and `.. default-role::` are unknown directives; the inputs that
  crash Sphinx are not refused yet (decision 13) — an empty classed block
  quote reaching `HandleCodeBlocks` is kept where Sphinx crashes.
- T17b: `rst_prolog`/`rst_epilog` (above); the `.. hlist content is not a
  list` warning is missing; once the prolog is spliced, explicit targets'
  absolute lines must count its lines.
- T17c: `source_suffix` discovery and download files (`env.dlfiles`).
- T18a/T18b: the structure directives with pending transforms, `contents`,
  `sectnum` and `TargetNotes`; the four `KNOWN_READ_STATE_GAPS` rows (one
  case's pending serials).
- T18a, found while classifying CPython's records: inline markup next to a
  non-ASCII dash or ellipsis is rejected. An end-string followed by one
  (`*b*—but`) prints `Inline … start-string without end-string` where
  docutils accepts the markup. A start-string preceded by one (`x —*b* y`)
  is a silent divergence: it is not recognized, so the text stays plain,
  with no emphasis and no record, where docutils makes it emphasis. Both
  halves come from `tools/gen_punctuation_tables.py` not expanding the
  ranges in docutils' regex-class strings (`src/rst/punctuation.rs`'
  `DELIMITERS` holds `‐` and `‗` but not `‑`-`‖`). The fix is the
  generator, the regenerated table and a pin, with a docutils corpus case
  from T18a's regeneration, the only one that grows that corpus.
- T18c: `centered`, `productionlist`, `tabularcolumns`, `acks`,
  `sectionauthor` and the other Sphinx extras.

**Wave 5b** (HTML writer and alabaster):

- The three `KNOWN_AUX_GAPS` rows: image-scale warnings the writer prints
  (`docutils-0447`, `-0449`, `-0450`).
- A `system_message` with an explicit `line=None` pretty-prints
  `line="True"`; the writer must print `, line None`.
- `ApplySourceWorkaround` (010) gives paragraphs, titles, terms and images
  their `astext()` as `rawsource` where docutils records the markup source;
  no 5a transform reads it, but the highlighter does (and T18a's
  `parsed-literal`). The `rawsource` of quoted literal blocks, doctest blocks,
  `literalinclude`, `code-block` and `include :literal:` is not pinned yet.
- `include :code:` keeps the code as one text node: Pygments' token
  `inline`s wait for the Pygments port (U3).
- The date formatter behind `|today|` (`format_date`, a `babel` `en` port in
  `src/transforms/sphinx.rs`) is UTC-only and not a transform;
  `html_last_updated_fmt` needs local time, so it moves to a utility module.
- `resolve_toctree`'s `collapse=True`, `prune=False` and `titles_only` paths
  are ported but unexercised: the sidebar must pin them. Its 8-parameter
  signature with `ToctreeLocations` is the interface 5b and 5c reuse.
- The HTML oracle is regenerated on Linux first (U8): the 30 conf-variant
  `sphinx_read` references are stale, and the cases 5a appended wait for it.

**Wave 5c** (builders, multi-document chrome, performance, golden corpus):
decision 6's write set and re-read policy, and decision 17's read-phase
placement of `:external:` diagnostics (both above); `dirhtml`/`dummy`
builder cases, the sidebar toctree and real URIs.

**Later milestones**: the `c`, `cpp`, `js` and `rst` domains (M5; their 38
prefixed directives stay `Unknown directive type`); extensions such as
`sphinx.ext.doctest` (M4); `DocInfo` and `|today|` for languages other than
English, and `term`/`classifier` `rawsource` with `ApplySourceWorkaround`'s
i18n half (M7); a grouped doc-field body located through docfields'
`inline(translatable=True)` wrapper (`:param x: body bad_` prints line 3 in
Sphinx, 1 here).

**Accepted (documented, no owner)** — location and parse-layer residuals,
each found by probing Sphinx 9.1.0 and documented at its code site:

- Line arithmetic: directive-line messages inside grid and simple table
  cells print one line low; a duplicate `:name:` on `include :literal:` is
  located at the directive line (Sphinx: the included file's start line + 1);
  the line-block directive's line arithmetic after an earlier include;
  inline messages in a glossary term after an include, or in an included
  file, print the term's own file and the line before the term's (as with
  no include), where Sphinx reads the term's 0-based offset back through the
  combined top-level input (a term on `index.rst:8` after an include prints
  `part.inc:1` in Sphinx, `index.rst:7` here); a
  line-block directive or glossary inside a cell takes the cell offset once
  (docutils twice); INFO records in cells keep the true line (pure-docutils
  doctrees only); `Title underline too short.` is missing before `Unexpected
  section title.` in nested contexts; a rubric ending a grid-table cell
  (final position 8 in Sphinx, 0 here); an empty `line` (line 0 in Sphinx).
- Targets and includes: an include inside a directive's content reads a
  shifted position (Sphinx `part.inc:2`, here `index.rst:14`); a target after
  an include inside a disconnected list keeps the top-level index; a target
  after an include *inside* a glossary definition prints its line in its file
  (Sphinx counts the inserted lines); so does a bare target closing a file
  included inside a glossary definition (Sphinx's definition machine counts
  the included lines as its own, after the include's line and
  `insert_input`'s padding line: `part.inc:15`, here `part.inc:5`; code
  site: the guard in `resolve_absolute_lines`, `src/rst/block.rs`; probe
  `bare_target_from_include_in_definition`); a duplicate explicit target
  name in a glossary definition after an include prints the target's own
  line (Sphinx reads the definition's count back against the top-level
  input);
  `parsed-literal` line numbers after or through an include (a small
  follow-up with `note_absolute_line`); a bare target ending a csv-table cell
  with a duplicate label prints the document where Sphinx prints
  `<unknown>`; duplicate labels on a plain grid or simple table and an
  `invalid index entry` in a csv cell are located by their spans in stage M.
- Messages: duplicate inline-target messages are placed after the paragraph
  (docutils: before) and located at the text's line instead of the top-level
  machine position; a substitution definition reports `Problematic content`
  where docutils reports an earlier illegal node; a multi-line non-directive
  substitution body misses `Explicit markup ends without a blank line`; a
  substitution message's literal block drops the definition's trailing blank
  lines (`keep_warnings` only); `Duplicate ID` for an `equation-<label>` is
  reported only when an equation target held the id first; its holder's
  start tag is read off the finished tree (a later duplicate section shows
  `dupnames=` where Sphinx printed `names=`).
- Tables: a simple-table span line that skips a column is accepted (docutils:
  `Column span alignment problem`); `Column span incomplete` is located at
  the table's first line and found before earlier rows' checks; the margin
  check runs line-major (docutils: column-major); `csv-table` accepts
  malformed quoting docutils' strict dialect rejects.
- Other: a code-block caption is parsed into an unattached container (Sphinx:
  attached, source `<generated text>`); directive option field names are not
  inline-parsed (a role there loses `unknown role name`); an `only` inside a
  container docutils fills before placing (definition, field body, table
  entry) stays inside it where Sphinx appends it after; a dropped external
  target gives an indirect target its id rather than its refuri; an
  `rst-class`-hoisted rubric's location; a `parsed-literal` hoisted out of a
  substitution definition keeps its own source; `figure_language_filename`
  is fixed at its default, `.*` image patterns are taken in sorted order
  (Sphinx: directory order) and the glob branch's `OSError` text is rebuilt
  best-effort; `env.metadata` omits the authors list and keeps `tocdepth` as
  text; tags with non-ASCII identifier classes and `\N{}` escapes are
  approximated; on Windows an included file in a subdirectory prints mixed
  separators; `-D` on `suppress_warnings` and `show_warning_types` is coerced
  by this crate's typed `-D` rules, not Sphinx's `convert_overrides`;
  `suppress_warnings` entries `''`, `.` and `.*` do not drop
  `validate_intersphinx_mapping`'s type-less ERRORs; the closing line prints
  the output directory as given where Sphinx prints it relative (stdout
  only).

**Deferred past wave 5a** (decision 17): extracting doc fields, `include` and
the `SourceTable` out of `src/rst/block.rs` (about 22,600 lines).

## Known divergences from Sphinx 9.1.0 (M2 wave 4.5)

Every one of these was found by probing the real toolchain, is documented at its
code site, and is deliberate. They are listed here so nobody has to rediscover
them. Divergences from earlier waves are recorded in the tables above; the ones
M2 wave 5a-1 closed say so, and what wave 5a-1 left open is in the section
before this one.

**Transforms not yet run (shape divergences)** — closed by M2 wave 5a-1.
`PropagateTargets`, `AutoNumbering` and `HighlightLanguageTransform` are ported
and applied to the persisted tree (the read transforms row), and the
`KNOWN_RESOLVED_GAPS` and `KNOWN_HIGHLIGHT_STAMP_GAPS` entries they caused are
gone. The parse layer itself stays transform-free by design (decision 1), so
`tests/doctree_differential.rs` still compares docutils' parse-layer trees.

**The file-inserting directives.**

- `:parser:` is refused, not silently ignored: `.. include:: f.md` with
  `:parser: myst` produces a SEVERE reading `parser mode is not supported by
  sphinx-ultra (planned with MyST, M2 wave 6)`. Sphinx would run the named
  parser.
- `:code:` **with a language argument** keeps its `literal_block` (classes
  `code <language>`, the file as `source`) for every language Pygments 2.21.0
  has a lexer for, and reports docutils' `No Pygments lexer found` for any
  other, as a Sphinx build does. The block holds the code as one text node:
  Pygments' token `inline`s inside it wait for the Pygments port (5b).
- **Provenance path spelling.** docutils' `adapt_path` keeps the cwd-relative
  spelling of an *included* file only in the node `source` attribute; what a
  user sees from Sphinx is **absolute** — an in-include warning location is
  cwd-absolutized for a node location (`sphinx.util.logging.get_node_location`)
  and srcdir-joined for a `(source, line)` tuple (`env.doc2path`). This crate
  spells included-content provenance srcdir-relative throughout — node `source`
  attributes, circular-inclusion chains, SEVERE texts and in-include warning
  locations alike — so on the CLI a user sees a bare relative path
  (`part.rst:5: WARNING: …`) where `sphinx-build` prints the absolute one;
  the message bytes are identical. Since M2 wave 5a-1 a logger warning
  located at a node prints the node's source made absolute, as
  `get_node_location` does (`<srcdir>/part.rst:5`), so only docutils reporter
  records keep the relative spelling — docutils' own cwd-relative one when the
  build runs from the source directory, and a different one when it does not.
  The env oracle canonicalizes both sides (`canon_scope8`), which is sound
  because the same transformation runs on both and only prefix *presence* can be
  masked; any difference below the srcdir still diverges. This is plan §Scope-8.
- The **encoding table is narrower than `codecs.lookup`**: this crate accepts the
  codecs a documentation build realistically names, and rejects the rest with
  docutils' own error text rather than silently mis-decoding.
- **Touching an included `.txt` re-reads its own document.** A file that is both
  a source document and an include member is discovered as both; the resulting
  rebuild is a superset of sphinx's. Pre-existing discovery-breadth behaviour,
  not introduced by the include work.
- **Empty-file `:number-lines:`** now sizes its column from `string2lines` like
  docutils. The `:code:` sibling keeps docutils' own ragged `startline + 1`
  quirk on purpose — that one IS the upstream behaviour.
- **`env.included` for an include that resolves *outside* `srcdir`.** Sphinx's
  `path2doc` lets `relative_to` fail and records the absolute path itself,
  suffix stripped, as a pseudo-docname — `env.included['index'] ==
  {'/…/ext/part'}` for `.. include:: link/part.rst` where `link` leads out of
  the tree; this crate records nothing there (`utils::path2doc`). The
  dependency record is identical on both sides. Output-inert: the set's only
  reader in 9.1.0 is `check_consistency`'s membership test against the real
  docnames, which a `/`-rooted name can never pass, so warnings and build
  output agree (env-oracle probe s1, round C). It would surface only through
  the env oracle's `included` key on a corpus project that includes an
  out-of-tree `.rst` through a symlink — none does; if one ever does, emit
  the pseudo-docname or exempt the key.

**Hyperlink targets and Python whitespace (panel fix rounds D and E).**

Round D's two entries here were both wrong, and round E replaced them with
code. What they claimed, and what was actually true:

- **A malformed multi-line target's fallback comment.** Round D held out
  `.. _name` + an indented `: uri` as "malformed on both sides, the comment
  starts on a different line". It is not: at round D this crate produced a
  **valid** `<target ids="name" names="name" refuri="uri">` and **no warning
  at all**, where docutils yields a `<comment>` holding `: uri` plus
  `WARNING line 2: malformed hyperlink target.` (the behaviour round D
  described belongs to a *different* input, `.. _name` + `   uri`). Round E
  ports `Body.hyperlink_target` whole — the one-line-at-a-time join with the
  continuation's indentation kept, `explicit.patterns.target` run by hand with
  its real tail `(?<![\s\x00])[ ]?:([ ]+|$)`
  (`non_whitespace_escape_before = r'(?<![\s\x00])'`, states.py:780 — the old
  in-code comment cited `(?<![ \n\x00])`, which does not exist), and the
  malformed path's line attribution and comment slice — and pins 20
  `round_e` docutils cases over it, this one included. No divergence remains
  in this construct.
- **Python `\s` outside the name paths.** Round D said "two docutils-parity
  nano-edges remain" and that every other site was "not name munging". Round
  D's own verifier refuted that with seven divergent sites, two of them id
  munging. Round E converted them all, plus every other site a probe could
  make diverge — `class_option`, `directives.uri`, `positive_int_list` (which
  had been raising an ERROR on `:widths: 1\x1f2`), `raw`'s `format`, the
  `unicode` directive's codes, `parse_directive_arguments`, extension-option
  field names, sphinx's `option_desc_re`, docfields' `split`/`rsplit`,
  `string2lines`' rstrip, and the whole inline-markup start/end-string
  lookaround family including `embedded_link`. The table below is the honest
  successor to the "sweep complete" claim. Its scope is the grep
  `is_whitespace|split_whitespace|\.trim\(\)|\.trim_start\(\)|\.trim_end\(\)`
  over `src/rst/` and `src/doctree/`, minus `py_isspace` lines and comment
  lines — widened in round F from the `trim()`-only grep round E ran (which
  had silently left the `trim_start()`/`trim_end()` family out) and
  re-probed with LEADING-character shapes, which round E's trailing/interior
  probes had not covered; what that turned up is in the round-F note below.
  It names every one of the **52** sites that grep returns at the round-F
  commit, and `src/doctree/` has none (its only two hits are doc comments).

| Site | Classification | Basis |
|---|---|---|
| `src/rst/block.rs:451`, `:468`, `:469`, `:470` | benign | `directive_records` bookkeeping for the validation pipeline, not doctree output; the argument split it mirrors (`parse_directive_arguments`) is `py_split`. |
| `src/rst/block.rs:2320`, `:2783`, `:2790`, `:3160`, `:3167`, `:7720`, `:7748`, `:9896`, `:12173`, `:13187` | benign | Blank-or-not tests over a whole line, a line's tail, or a table-cell view — text that `string2lines` (rounds E/F) or the cell slice (`get_2D_block`'s rstrip, round F) has already rstripped with `py_isspace`. Rust's White_Space set is a strict subset of Python's `isspace`, so `trim()` cannot find anything that rstrip left, and a tail of pure Python whitespace is already gone. Probed: `Sec\x1f` + underline, `- a\x1f`, `.. a\x1f`, `.. [1]\x1ftext`, `-a\x1f  desc`. |
| `src/rst/block.rs:2570`, `:2590`, `:2593`, `:2625`, `:2629`, `:2631`, `:2642`, `:2657`, `:2859`, `:2866`, `:2877`, `:2908`, `:2940`, `:2945`, `:2964`, `:3009`, `:3095`, `:8118`, `:8132`, `:8146`, `:8152` | benign | `trim_end()` over whole table lines — borders, rows, the error literals' joined blocks — which carry no trailing whitespace at all after `string2lines`; the four border predicates (`:8118`-`:8152`) mirror docutils' ` *$` tails, which admit spaces only, and `:3095` measures a tail already found non-blank with `py_isspace`. Same subset argument. Probed (round F): a nested grid table in a cell with `\x1f` before the closing `\|`, `\| >>> 1\x1f\|`, a line block and a literal block in a cell — all SAME. |
| `src/rst/block.rs:4404` | benign | `ws_collapse` (sphinx `ws_re`) already splits on `py_isspace`; the `trim()` only strips the argument's ends, which the collapse would drop anyway. |
| `src/rst/block.rs:4464` | benign | The `.. index::` line loop. Sphinx does not strip the line, but `process_index_entry` (round F: `py_isspace` throughout) strips a superset of what this `trim()` removes, and an all-whitespace line yields no entry on either side. |
| `src/rst/block.rs:5642`, `:5713` | benign — probed | `py:module`/`py:currentmodule`'s `self.arguments[0].strip()`. `parse_directive_arguments` (`py_split`, round E) has already removed a one-word argument's outer Python whitespace, and a two-word one is the `maximum 1 argument(s) allowed` error for these `final_argument_whitespace=False` directives. Probed `.. py:module:: \x1fa b` and the `currentmodule` twin — SAME. |
| `src/rst/block.rs:7693`, `:7710`, `:7713` | benign — probed | Multi-line substitution markers: docutils `.strip()`s each continuation (`substitution_def`) and the crate's `trim()` plus `trim_start()` offset land on the same remainder because the block's own indentation is spaces. Probed `.. \|a` + `   \x1fb\| replace:: x` and the NBSP twin — SAME. |
| `src/rst/block.rs:7904`, `:7909` | benign — probed | The hyperlink-target remainder feeds the ported `parse_target`, which — like docutils' `''.join(unescape(line).split())` — removes ALL whitespace with `py_split`. Probed `\x1f` leading the link, leading a second-line link, leading and trailing a continuation — SAME. |
| `src/rst/block.rs:8594`, `:12255` | benign | Operands of `int()`: `parselinenos`' `a`/`b` halves (its outer `part.strip()` is `py_isspace` since round F) and `positive_int_list`'s items. `int()` strips exactly Unicode White_Space — Rust's set — so these trims reproduce it (see `:12478`). |
| `src/rst/block.rs:11040` | ledgered, documented in place | `literalinclude` `:dedent:`'s "non-whitespace stripped by dedent" check. docutils' is `str.isspace`-based; the dedent columns are spaces by construction in every probed shape, so no input reaches the difference. |
| `src/rst/block.rs:12478` | **benign, and `py_isspace` would be WRONG** | `py_int_canonical`. Probed over all 0x110000 codepoints against CPython 3.12: `int()` strips Unicode White_Space only and *rejects* `\x1c`-`\x1f`, which `str.isspace` admits — `int('\x1f2')` raises, and docutils reports it (`:widths: 1,\x1f2` → `invalid literal for int() with base 10: '\x1f2'`). Rust's `trim()` is the exactly-right predicate. |
| `src/rst/inline.rs:840`, `:841`, `:1415`, `:1416` | **ledgered — real divergence** | Sphinx's `explicit_title_re = r'^(.+?)\s*(?<!\x00)<(.*?)>$'`. Three probe-confirmed gaps in one construct: the crate takes the LAST `<` where the non-greedy `(.+?)` takes the FIRST (`` :pep:`a <b> <8>` `` → sphinx `invalid PEP number b> <8`, ours resolves PEP 8; `` :ref:`a <b> <c>` `` → sphinx `reftarget="b> <c"`, ours `"c"`); it trims the TARGET group (`:840`, `:1415`), which sphinx does not (`` :pep:`title < 8 >` `` → sphinx index entry `PEP  8 `); and the title's `trim_end()` (`:841`, `:1416`) is Rust's set where sphinx's is `\s`. Porting `explicit_title_re` properly is one wave-5 change, not three. |
| `src/utils.rs:189` (outside the grep's scope; the predecessor `src/rst/block.rs:12464` row) | converted | `py_repr_str`'s printability test: `is_control() \|\| is_whitespace()` is exactly Cc ∪ Zs ∪ Zl ∪ Zp (Cs is unrepresentable in a Rust `char`). Cf/Co/Cn stay unescaped — the honest gap, since matching them needs a generated Unicode general-category table this tree does not have. Round F made this the ONE implementation: `block.rs`'s `py_repr` (directive messages, index tuples) and every warning-stream `%r` (toctree, resolver, py domain, intersphinx, builder) call it; `src/env/toctree.rs` had carried a second copy that escaped only `< 0x20` and `0x7f`. |

Round E's probes also turned up four divergences it did NOT change, each
verified present at `94b08cd` (so none is a round-E regression) and each
recorded with its input in
`.superpowers/sdd/2026-09-01-m2-wave4.5-py-domain/panel-fix-E-report.md`:

- **`explicit_title_re`** — the table row above.
- **A malformed substitution definition's fallback comment**, the exact
  sibling of the hyperlink-target bug round E fixed. `substitution_def`
  (states.py:2140-2178) also raises its `MarkupError` with the state machine
  already on the block's last line, and that block is gathered with
  `until_blank=False`, so it runs THROUGH a trailing blank: `.. |ab replace::
  q` + a blank line + `para` yields an EMPTY `<comment>` and the warning at
  line 2, where this crate emits a comment holding line 1 and warns at line 1.
  No control character is needed to see it. Round E stopped short because
  `indented_block` deliberately trims trailing blanks and the fix needs
  docutils' un-trimmed extent — a change to shared machinery that wants its
  own round. The single-line/EOF forms agree and ARE pinned
  (`substitutions.marker_*_before_close_malformed`).
- **`PropagateTargets` is not run at the parse layer** (plan §Scope-3, stated
  in-code at `src/rst/block.rs`). Sphinx's recorded doctree moves a block
  target's `ids` onto the next body node and leaves `refid` behind:
  `.. _t:` + a blank + `para` gives `<target refid="t">` + `<paragraph ids="t"
  names="t">` there, `<target ids="t" names="t">` + `<paragraph>` here. Closed
  by M2 wave 5a-1: the read transforms apply it (260) to the persisted tree,
  and the Sphinx oracle compares the post-read tree; the parse layer stays
  transform-free on purpose.
- **A simple-table cell's nested line attribution** is one line short:
  `=== ===` / `a::  b` / `=== ===` warns `Literal block expected; none found.`
  at line 4 under docutils and line 3 here. Round F found the same cell also
  carries a message docutils suppresses when the line is a bare `::` — see
  the round-F note below.

**Python whitespace, round F — the leading-character sweep.** Round E's
verifier found the table above had been scoped to a `trim()`-only grep, which
left the `trim_start()`/`trim_end()` family out, and named three divergent
sites. Round F widened the grep and — because round E's "benign" evidence had
been trailing/interior shapes only — re-probed every remaining site with a
LEADING NBSP or `\x1f` as well, against the pinned oracles through the fixture
generators' own harnesses (`gen_doctree_fixture.parse_pformat`,
`gen_sphinx_fixture.probe`, `gen_env_fixture.build_project`). That refuted
round E's "benign" verdict at twelve of the table's line numbers beyond the
glossary — seven in the whole-line row (both title trims, the simple-table
first-column and column-margin tests, the toctree entry, the `line-block`
line, the signature line) and five in the field row (option synonyms,
`parselinenos`, `parse_line_num_spec`, the meta-field name, and
`process_index_entry`'s three trims counted once); the field row's own stated
probe, `:emphasize-lines: 1\x1f,2`, is one of the pins that is red at
`5a5dd24`. It also found three `trim_start()`/`trim_end()` sites the old grep
had never listed (the field body, the option description, the `::` tail) and
one non-trim `str.strip()` test (`is_enumerated_list_item`). All of it is
fixed in code and pinned — docutils 718→735, sphinx 472→489, +2 unit tests,
+2 env tests. Rebuilt at `5a5dd24` against the new pins, the pre-fix crate is
red on 17 docutils cases, 13 sphinx cases, both env tests and both unit pins
(the other four sphinx cases are guards that already passed); each pre-fix
output is in
`.superpowers/sdd/2026-09-01-m2-wave4.5-py-domain/panel-fix-F-report.md`:

- The three named sites: the definition-list term `rstrip()`s
  (`states.py:3015` — `term\xa0 : cls` → `term`); the sphinx glossary term and
  its first classifier are VERBATIM (`split_term_classifiers` — `term\xa0` and
  the index entry `'term\xa0'` keep the NBSP, and `term : \xa0cls` keeps it in
  the key), which also replaced a `splitn(" : ")` with the ` +: +` split
  (`term  :  cls` and `term : a : b` came out right by way of the old trims
  and stay pinned as guards); `string2lines_tw` rstrips with `py_isspace`, so
  a 10 000-character included line plus `\x1f` is not over the line-length
  limit.
- `py_repr_str` is one implementation (`src/utils.rs`) behind every
  warning-stream `%r`, pinned by a toctree entry `foo\xa0bar` that sphinx
  reports as `'foo\xa0bar'`.
- A leading NBSP is KEPT where docutils' ` +` / `  +` regex tails keep it: a
  field body's first line (`:a: \xa0b`) and an option-list description
  (`-a  \xa0desc`) — the crate had `trim_start()`ed both.
- Python `strip()`/`lstrip()`/`rstrip()` where the crate had Rust's set: the
  `::` tail (`abc\x1f ::` → `abc`); `process_index_entry`'s five trims
  (`!\x1fa`, `single: \x1fa`, `a,\x1fb`, `a, !\x1fb`, `pair: \x1fa; b`,
  `a\x1f, b`); an overlined title (`\x1fT` → `T`) — while an underline-only
  title keeps a leading NBSP, because `Text.underline` only `rstrip()`s; the
  `line-block` directive's per-line strip and `lstrip`-measured indent (a
  leading NBSP is a nested `line_block`); the simple-table column margin and
  first-column blank tests (`a  \x1fb` is a two-cell row, `\x1f   c` a
  continuation); option synonyms (`-x, \x1f-y` registers both);
  `parselinenos` (`:emphasize-lines: \x1f1` and `1\x1f,2` highlight);
  `parse_line_num_spec` (`:lines: \x1f1`); `get_signatures` (a second
  signature line opening with `\x1f`); `_filter_meta_fields`
  (`:\x1fmeta private:` is filtered); and the table-cell views, now
  `get_2D_block`'s own rstrip (no input reaches the old `trim_end()` — every
  cell consumer rstrips again — but it is docutils' predicate now).
- NO strip where sphinx has none: toctree entries. `TocTree.parse_content`
  reads `self.content` verbatim, so an entry indented deeper than the block
  (`   a` / `     b`) names the document `'  b'` — nonexisting — and leaves
  `b` an orphan; the crate had trimmed it and resolved `b`.
- `is_enumerated_list_item`'s `if not next_line[:1].strip()`: a next line
  opening with an NBSP or `\x1f` is "blank or indented", so `1. a` /
  `\xa0b` IS an enumerated list (which then ends without a blank line) —
  not a trim site, but the same `str.strip()` semantics.

Two divergence classes the sweep found are NOT whitespace-predicate sites and
stay ledgered, each probe-confirmed at `5a5dd24` and unchanged by this round:

- **The indentation measure.** docutils' `get_indented` measures a line's
  indent as `len(line) - len(line.lstrip())` — Python `lstrip`, so an NBSP or
  `\x1f` after the spaces IS indentation — while `LineRec::indent` counts
  ASCII spaces: `para` + blank + `   \xa0quoted` is a block quote holding
  `quoted` under docutils and `\xa0quoted` here (the `\x1f` twin likewise).
  Changing it means changing the block model in `src/rst/lines.rs`, which
  every construct's dedent goes through — a wave-5 item, not a one-liner.
- **A bare `::` inside a table cell.** `Body.line` short-circuits
  `match.string.strip() == '::'` to text with no message; this crate emits
  the `Unexpected possible title overline or transition` INFO inside grid and
  simple cells (a top-level `::` agrees), on top of the one-line-short
  attribution recorded above. Unrelated to whitespace — the `\x1f`-free
  control diverges identically.

**Duplicate explicit targets set from a directive option.** When `:name:` (or
`figure`'s `:figname:`) repeats a name an earlier explicit target already
claimed, docutils appends its `Duplicate explicit target name: "…"` message to
the node it was set on and then **detaches it again** if that node's content
model rejects a `system_message` child (`nodes.py:1983-1990` — the `Messages`
transform would re-place it, and this crate runs no transform pass at the parse
layer). So at the parse layer docutils shows no warning at all on `image`,
`note` and `figure`; this crate emits the warning beside the node. Pre-existing
(wave 1-3 machinery, identical for plain `:name:`), probed in wave 4.5's review
round 1, and the reason the `figname`-collision case is held out of the docutils
corpus with its reason recorded at the case list in
`tools/gen_doctree_fixture.py`. Everything else about `:figname:` is at
parity.

**Cross-reference roles.**

- The explicit `Title <target>` split takes the **last** `<` in the role text;
  Sphinx's `explicit_title_re` (`^(.+?)\s*(?<!\x00)<(.*?)>$`,
  `util/docutils.py`) takes the **first** one a non-empty title can precede, and
  its null-marker lookbehind makes an escaped `\<` ordinary text. Three shapes
  differ, each probed against sphinx 9.1.0 and silent on both sides:
  `:term:`a <b> <c>`` targets `c` here, `b> <c` there; `:term:`<foo>`` is an
  explicit reference to `foo` here, an implicit one to `<foo>` there;
  `:term:`a \<b>`` is explicit-to-`b` here, implicit-to-`a <b>` there (this
  crate unescapes before the split, so the marker is already gone). Wave-5
  backlog; the padding and whitespace halves of the same split are pinned by
  the oracle (`sx_roles.xref_explicit_target_padding_kept`).
- Equation references are resolved since M2 wave 5a-1, through the math
  domain (`src/env/math.rs`); `:eq:` left the "domain not implemented" count.

**`:pyobject:` and the `DefinitionFinder` port.**

- **`:tab-width:` (≠ 8) + `:pyobject:` + mixed indentation is the one combination
  that can change which TAGS are found**, not merely how they render: tab
  expansion at a width other than 8 can reorder indentation columns and so change
  the block structure the tokenizer sees. Kept out of the oracle corpora
  deliberately.
- A file that **tokenizes but does not parse** (`x = = 1`) yields tags here where
  sphinx's analyzer raises and warns. Better-than-sphinx, but a divergence: do
  not fixture an unparsable file.
- **PEP 701** same-quote f-string nesting is lexed pre-3.12 style. Python 3.12
  allows `f"{d["k"]}"`; this lexer ends the string at the inner quote. Signatures
  and module-level code that reach `:pyobject:` do not use it.

**The py signature grammar.**

- Expressions outside the supported subset take the `pseudo_parse_arglist`
  fallback silently. For comparisons, f-strings and `{**a}` dict unpacking
  sphinx *warns* (`NotImplementedError`/`ValueError` inside
  `signature_from_str`) where we stay silent; `lambda`, slices and `**kwargs`
  in a call are rendered rather than refused by sphinx, so there the difference
  is a different parameter list on our side with no warning on either — a
  `lambda` default in particular is comma-split by the fallback. What sphinx
  renders for the `**` case is not the source text, though: `_UnparseVisitor`
  formats every keyword as `f'{k.arg}={…}'` and a `**` keyword has `arg=None`,
  so `signature_from_str('(x=f(**kw))')` gives the default `f(None=kw)`
  (`sphinx/pycode/ast.py:127-131`, probed on 9.1.0; `(x=lambda a: a)` gives
  `lambda a: ...` and `(x=a[1:2])` gives `a[1:2]`). Complex defaults differ
  textually (`1 + 2j` vs `1+2j`).
- A return annotation ending in `)` — `f(x) -> (int, str)` — is a node-shape
  divergence: sphinx's `py_sig_re` swallows the parenthesised retann into the
  arglist and its def-wrapped grammar still yields params `[x]`, while this
  crate's arglist grammar rejects the text and pseudo-parses it (silently, no
  warning on either side). Documented T6 deviation 2; the case is held out of
  the corpus as `EXCLUDED["py.function_greedy_retann"]`
  (`tools/gen_sphinx_fixture.py`).
- Numeric source spelling is recovered in source order, which agrees with
  render order everywhere except one shape: `ast.Call` splits `args` from
  `keywords`, so a `*` unpack written AFTER a keyword argument is rendered
  before it and consumes the earlier token — `f(k=0x1, *a(0x2))` prints
  `f(*a(2), k=1)` where sphinx keeps `f(*a(0x2), k=0x1)`. The text order
  matches sphinx (`visit_Call` reorders identically); only a non-decimal
  spelling in such a fragment differs. Fixing it needs source spans on
  `PyConst` (panel fix round A residual, `src/py/arglist.rs` header).
- A multi-statement annotation (`int;`, `int\nstr` — `Module(body=[Expr,
  Expr])`, which sphinx's `functools.reduce` renders as the concatenation of
  both) falls back to a single whole-text cross-reference here. Conservative;
  recorded in `src/py/expr.rs`'s divergence list.
- `MAX_DEPTH` (200) is a whole-expression complexity budget — trailers and binop
  folds charge it too — so a flat ~200-operation chain errs into the fallback
  where CPython parses. Err-side and conservative.
- `repr()`'s printable test under-escapes the **unassigned (Cn)** code points; a
  full-codespace sweep against the pinned interpreter shows no over-escaping and
  no other under-escaping class.
- **`builtin_resolver`'s name sets are pinned to CPython 3.12**, the oracle
  toolchain (`WindowsError` is absent, for instance). They must be revisited on a
  toolchain bump; the provenance comment at `BUILTIN_CLASSES` carries the
  coupling.
- `sphinx` **crashes** (`AssertionError` at `sphinx/util/docfields.py:381`) on
  a `field_list` child that is not a two-child `field`; this crate passes such a
  child through. The trigger is **reachable**: `.. confval:: t` + `:type: *bad`
  (likewise `:default: *bad` and `:type: *a b`) — the unterminated emphasis
  drops a one-child `system_message` into the field list the directive
  generates — aborts a full `sphinx-build` 9.1.0, while the same field written
  in the directive *body*, `:type: int` and `:type: *bad*` build clean. (Task
  16's "does not reproduce" was read off the harness3 read-phase venue, which
  never runs `DocFieldTransformer`; three independent full-build reproductions
  in the panel round reversed it, and the in-code note at `doc_field_step1`
  carries the transcript.) The `len != 2` pass-through is therefore a live,
  deliberate better-than-sphinx divergence, unpinnable by an oracle — which is
  why `sx_std.confval_bad_type_markup` stays out of the sphinx corpus.

**Diagnostics.**

- **In-tree docutils `system_message`s are not surfaced on stderr.** Closed by
  M2 wave 5a-1: every reporter record is printed on stderr and in the `-w` file
  as Sphinx's `WarningStream` prints it, and counts toward `-W`; a broken
  `include` path fails `-W` as it does under `sphinx-build`.
- **References into a domain this build does not implement are counted, not
  warned about.** Every `refdomain` outside `{"", "std", "py"}` — `:c:`,
  `:cpp:`, `:js:`, `:rst:` — short-circuits in `src/env/resolve.rs` and is
  summed into the one-line `N cross-domain reference(s) not validated (domain
  not implemented until M5)` notice; sphinx resolves them and warns under `-n`.
  Closes with M5.
- **A `:doc:` target that looks like a URL is exempt from `unknown document:`.**
  Removed in M2 wave 5a-1 (decision 17): `:doc:`https://example.com/page``
  now warns `unknown document: 'https://example.com/page' [ref.doc]`, as
  Sphinx does.
- The three **glossary misformat warnings** are reported ONE LINE LOW, because
  sphinx reports them at a 0-based `content.items` offset that the reporter then
  renders as a 1-based line. Reproduced deliberately — the oracle compares bytes.

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
