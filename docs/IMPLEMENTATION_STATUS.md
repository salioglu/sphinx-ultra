# Implementation Status

**Audit-verified status as of 2026-10-01** (v0.4.1 + M2 waves 1–4.5 + M2 wave 5 sub-project 1: the reporter channel and the read transforms).
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
| Parallel orchestration | ✅ | rayon pool sized by `-j`/config. Per-file failures become `BuildErrorReport`s and the build continues (2026-08). Since M2 wave 5 every thread that parses or walks the doctrees — the read pool's, and the thread the binary runs the build on — has a 64 MiB stack (`rst::PARSE_STACK_SIZE`, address space committed only as touched), and the public parse/transform entry points move their work onto such a thread whatever thread calls them, so the parser's 200-level nesting guard, never the stack, ends deep nesting. Should the system refuse threads that size (a strict overcommit policy, an address-space limit), the pool, the build thread and the entry points fall back to the default stack and run as before. |
| Incremental cache | ✅ | Fixed 2026-08: warm-cache rebuilds no longer deadlock (DashMap guard held across `alter` — found by the new E2E suite); hits write the rendered page; `--clean --incremental` produces a full tree (clean clears the cache); `max_cache_size_mb`/`cache_expiration_hours` plumbed; config changes invalidate via blake3 fingerprint; eviction honestly named least-accessed (LFU-style). M2 wave 4: staleness is now Sphinx's env-level computation, not mtime alone (below). |
| Dependency graph / outdated computation | ✅ (M2 wave 4) | `build_dependency_graph`'s empty-vec TODO is gone. `BuildEnvironment::get_outdated_files` (`src/env/mod.rs`) is a port of Sphinx's: added ∪ changed ∪ removed documents, where "changed" consults `env.dependencies[docname]` — a dependency that is missing or newer than the document's read time makes the document outdated. `src/env/dependencies.rs` ports `note_dependency` and `relfn2path`. **M2 wave 4.5 ended the images-only limitation**: `include` and `literalinclude` call `note_dependency` on every member file they read, and `env.included` records the inclusion graph, so touching an included fragment re-reads the documents that include it. `docutils.conf` and gettext catalogs are still unmodelled. One deliberate non-dependency: a docutils **standard include** (`.. include:: <isonum.txt>`) records nothing, because sphinx's `Include.run` bypasses the path rewrite entirely for `<…>` targets (`other.py:410-412`) — the files are vendored, so there is nothing on disk to watch. That is unit-tested; the env oracle cannot reach it. Fixture `tests/fixtures/deps_image/` + `tests/e2e_cli.rs` cover the touch-an-image-and-rebuild path. Config-class (`rebuild='env'`) narrowing is deliberately not done: the whole-config `.config-fingerprint` wipes the cache on *any* config change, a strict superset that cannot under-rebuild. |
| RST parsing | ✅ (docutils-fidelity, wired) | **M2 wave 3 (2026-08-13): the binary runs the new parser** — `Parser::parse` → `src/rst/parse_rst_full` (sphinx mode); the M1 line-scanner is deleted. `src/doctree/` generic-node IR with byte-parity `pformat`; `src/rst/` block + inline grammar (waves 1–2) plus docutils-exact directive machinery (typed option converters with docutils-verbatim error texts, options-before-arguments evaluation order, rawsource literals, as-written names), the docutils built-in directive set (admonitions/topic/sidebar/rubric/quote-family/compound/container/parsed-literal/image/figure/code/math/raw/line-block/class/table/csv-table/list-table) and substitution definitions (replace/unicode/date, embedded directives, duplicate dupname semantics). Zero divergence on a committed 761-case fixture vs docutils 0.22.4 parse layer (`tests/doctree_differential.rs`), which since M2 wave 5 also pins each case's reporter stream — the messages docutils writes, in creation order. Sphinx-mode set (toctree, versionmodified family, seealso, code-block/sourcecode + highlight state, only, rst-class, math + equation targets, index, hlist, glossary, xref pending_xref anatomy, pep/rfc/cve/cwe) verified against a real sphinx-build 9.1.0 read-phase oracle (`tests/sphinx_doctree_differential.rs`). Document now derives title/toc (docutils `make_id` anchors)/labels/toctree entries/directive+role records from the doctree; the three builder raw-source re-scanners are gone. **M2 wave 4** added generic object-description anatomy (`desc`/`desc_signature`/`desc_name`/`desc_addname`/`desc_annotation`/`desc_content`, the `:no-index:`/`:no-index-entry:`/`:no-contents-entry:`/`:no-typesetting:` family, the `PropagateDescDomain` transform) and the std-domain directives on top of it — `program`, `option` (incl. `[=value]` and comma-separated multi-name forms), `envvar`, `confval` with `:type:`/`:default:`, `describe`/`object`, `default-domain`; plus glossary terms taking their ids from Sphinx's `make_id` (not docutils') and index entries following `process_index_entry` onto a list-valued attribute. **M2 wave 4.5** added the py domain's fourteen directives on that anatomy, the `include`/`literalinclude` family (see their own rows below), and a faithful port of `Glossary.run`'s line state machine — which fixed the entry split (terms separated by a blank line share one `definition_list_item`; a `.. ` comment does not split a multi-term entry) and made its three misformat diagnostics appear in the doctree (printed since M2 wave 5's reporter channel). The sphinx oracle — since M2 wave 5 a comparison of parse **plus read transforms** and of the printed record stream — stands at **697 cases, zero divergence**. Remaining deferrals: ifconfig, meta, rst_prolog/epilog/default_role (all three wanted the per-line provenance layer wave 4.5 built, so they are now unblocked). Known gap recorded in-tree (`tools/gen_sphinx_fixture.py` header): `ObjectDescription`'s `allow_section_headings=True` is not modelled — this crate's nested parse is `match_titles=False` throughout, so a section title (or a `topic`/`sidebar`) inside a description body is rejected with `Unexpected section title.` where Sphinx accepts it. Threading a real `match_titles` through the section machinery is its own change; the two probe cases are held out of the corpus rather than committed knowingly-red. |
| Read transforms | ✅ (M2 wave 5, sub-project 1) | `src/transforms/` runs Sphinx 9.1's read transforms on every sphinx-mode document right after the parse — inside the parallel read phase, before the doctree is persisted and before the merge phase's domain hooks — in the order probed from `document.transformer.applied` (`READ_TRANSFORMS`, `src/transforms/mod.rs`): PreserveTranslatableMessages (010: toctree `rawentries`/`rawcaption`), DefaultSubstitutions (`\|version\|`, `\|release\|`, `\|today\|`, `\|translation progress\|`), MoveModuleTargets, HandleCodeBlocks and AutoNumbering (210), Substitutions and ReorderConsecutiveTargetAndIndexNodes (220), PropagateTargets (260), SortIds (261), DocInfo (340), AnonymousHyperlinks (440), IndirectHyperlinks (460), DoctestTransform (500), the citation definition and reference transforms (619 — a citation reference becomes an unresolved `pending_xref`; resolving it is sub-project 2), Footnotes (620), UnreferencedFootnotesDetector (622), ExternalTargets (640), InternalTargets (660), FootnoteDocnameUpdater (700), SphinxSmartQuotes (750), Transitions (830), SphinxDanglingReferences (850), MetadataCollector's read and removal of the docinfo (880 — in the pass, because it must precede FilterSystemMessages) and FilterSystemMessages (999). Each ports its upstream function and cites `file.py:line`, error texts and locations included. The persisted doctree is the post-transform tree (`DOCTREE_FORMAT_VERSION` 3), so labels, numbering, the env and a warm build all read Sphinx's ids; the docutils parse layer (`parse_rst`) stays transform-free for the docutils oracle. SmartQuotes runs at Sphinx's defaults (`smartquotes=True`, `smartquotes_action='qDe'`), honours `smartquotes_excludes` (languages, and builders — the builder's name now joins the cache fingerprint), `language` and `language-xx` classes, prints `No smart quotes defined for language "xx".`, and leaves backslash-escaped quotes, dashes and dots plain through the escape offsets text nodes now carry (`Node::escapes`). Transforms proven no-ops for an HTML build (StripComments, Decorations, Validate, ExposeInternals, UIDTransform, i18n without catalogs, AutoIndexUpgrader, RefOnlyBulletList under `html_compact_lists=True`) are named in the table, not ported. Evidence: the sphinx oracle's transform families (`tx_filter` 1, `tx_targets` 16, `tx_subst` 25, `tx_links` 53, `tx_footnotes` 23, `tx_docinfo` 34, `tx_misc` 26 cases) and its SmartQuotes family (`sq`, 20 cases, `smartquotes=True`), zero divergence; the env oracle's `keep_warnings_*`, `citations` and `smartquotes_default` projects; and the totality sweep (`tests/rst_proptest.rs`). |
| Docutils diagnostics (the reporter channel) | ✅ (M2 wave 5, sub-project 1) | The parser records every docutils `system_message` of level ≥ 2 the moment it creates it — with the text it has then, so a directive error prints without the literal block the tree's copy later gains, and a message whose node is later discarded still prints — into one ordered per-document stream (`src/rst/diagnostics.rs`) that the directives' and domains' logger records share; the transforms append theirs. The merge phase prints each document read this build, in docname order: its stream in creation order, then the index and std `process_doc` warnings. A document served from cache prints nothing — `sphinx-build` does not re-read it either. Rendering is Sphinx's: `{source}:{line}: {WARNING\|ERROR\|CRITICAL}: {text} [docutils]` (docutils' SEVERE is `CRITICAL`; a record without a line prints `{source}::`; a multi-line text prints verbatim on stderr and in the `-w` file), and INFO never prints. **Exit policy (decision D2):** every printed record counts as a warning — exit 0 without `-W`, exit 1 with it — exactly as `sphinx-build` does; none is a `BuildErrorReport`. This closed wave 4.5's known limitation that a broken `include` dropped its content silently with `-W` green. Evidence: both parse oracles pin the stream (the docutils fixture's per-case `stream`, the sphinx fixture's per-case `warnings`), the env oracle compares whole records (`warning_records`), and `tests/e2e_cli.rs` pins the CLI (`a_missing_include_prints_critical_and_exits_zero`, `dash_w_fails_a_build_whose_only_record_is_critical`, `w_file_and_stderr_carry_the_same_reporter_records`, `a_cached_document_prints_no_read_diagnostics`, `an_include_error_inside_an_included_file_names_that_file`). |
| Python domain (directives, registration, resolution) | ✅ (M2 wave 4.5) | All fourteen `py:*` directives (`module`, `currentmodule`, `function`, `class`, `exception`, `method`, `classmethod`, `staticmethod`, `attribute`, `property`, `data`, `decorator`, `decoratormethod`, `type`) on wave 4's object-description anatomy, with a real signature grammar: `src/py/expr.rs` is a Python expression parser plus an `ast.unparse` port (CPython 3.12 `_Unparser`'s precedence table and paren placement) for annotations (parameter defaults go through a port of `sphinx.pycode.ast.unparse` instead, which keeps a literal's source text — `0x10` stays `0x10` — a split `src/py/arglist.rs`'s header calls a trap), `src/py/arglist.rs` ports `_parse_arglist` / `_parse_type_list` / `pseudo_parse_arglist` (PEP 695 type-parameter lists included), and `src/py/annotations.rs` ports `_parse_annotation` / `type_to_xref` / `parse_reftarget`. Registration into `domaindata['py']` (`src/env/py_domain.rs`): insertion-ordered objects and modules, aliased entries, `duplicate object description of %s, other instance in %s, use :no-index: for one of them`, `more than one target found for cross-reference %r: %s`, and the `any`-role variant. Resolution (`src/env/resolve.rs`) covers `:py:func:`/`:py:class:`/`:py:meth:`/`:py:mod:`/`:py:attr:`/`:py:data:`/`:py:exc:`/`:py:obj:`/`:py:const:`/`:py:deco:` with sphinx's `refspecific` search order, the `builtin_resolver` fallback at priority 900, and `:any:` as the domainless role it actually is. Evidence: the sphinx doctree oracle's `py`/`pysig`/`pyconf` families (122 cases) and the env oracle's py projects, both zero divergence. |
| Object-signature config family | ✅ (M2 wave 4.5) | Nine keys, plumbed from `conf.py`/YAML/`-D` into the parser as `PySigConfig` (`src/py/mod.rs`): `maximum_signature_line_length`, `python_maximum_signature_line_length`, `python_trailing_comma_in_multi_line_signatures`, `python_display_short_literal_types`, `python_use_unqualified_type_names`, `toc_object_entries`, `toc_object_entries_show_parents`, `add_function_parentheses`, `add_module_names` (plus `strip_signature_backslash`). Pinned by 33 `pyconf` oracle cases carrying per-case `confoverrides`, and by `toc_object_entries_match_the_probe_for_all_five_config_variants` in the env suite. One deliberate divergence: an out-of-enum `toc_object_entries_show_parents` is accepted with a warning whose candidate list is in registration order, because sphinx's own list comes out of a hash-ordered set and is therefore not byte-reproducible. Panel fix round B added the `check_confval_types` check for the two `int \| None` keys (`maximum_signature_line_length`, `python_maximum_signature_line_length`): a `-D …=20` — a *string* under Sphinx, because `convert_overrides` never coerces a `None`-default key — or a mistyped `conf.py` literal warns ``The config value `…' has type `str'; expected `NoneType' or `int'.`` byte-exactly, in Sphinx's registration order, and leaves the key unset; Sphinx warns identically and then crashes on the first signature. |
| `include` / `literalinclude` | ✅ (M2 wave 4.5) | Built on a new per-line provenance layer (`SourceTable` + `SpliceRequest`, `src/rst/block.rs` + `src/rst/lines.rs`): every line carries the source it came from, so a warning inside an included file points into that file. `include` covers the whole docutils option set (`:literal:`, `:code:`, `:number-lines:`, `:encoding:`, `:tab-width:`, `:start-line:`/`:end-line:`/`:start-after:`/`:end-before:`, `:class:`/`:name:`), the sphinx path rewrite (a leading `/` is srcdir-relative), circular-inclusion detection with sphinx's multi-line chain message, and the 35 vendored docutils standard include files (byte-identical to the pinned wheel). `literalinclude` ports sphinx's reader filter chain in order — `:lines:`, `:start-after:`/`:end-before:`/`:start-at:`/`:end-at:`, `:pyobject:`, `:prepend:`/`:append:`, `:dedent:`, `:diff:` (a difflib port), `:emphasize-lines:`, `:lineno-match:`/`:linenos:`/`:lineno-start:`, `:tab-width:`, `:encoding:`, `:caption:`/`:name:`/`:class:`/`:language:`/`:force:` — with every reader error funnelled into the single reporter warning sphinx emits. `:pyobject:` is a port of `sphinx.pycode`'s `DefinitionFinder` tokenizer, checked differentially against the real one over 1200 real modules (24,903 tags, 0 mismatches). `source_encoding` (default `utf-8-sig`) is a real key and the default `:encoding:` of both directives (panel fix round B) — and only that: this crate's own `.rst` sources are still decoded as UTF-8, where Sphinx passes the key to docutils as `settings.input_encoding` for the document read too. The diagnostics both directives raise — a missing or unreadable file, the refused `:parser:`, a circular inclusion — are docutils reporter messages and **print since M2 wave 5** (`index.rst:9: CRITICAL: Problems with "include" directive path: … [docutils]`, counted by `-W`), located in the included file when they arise there. |
| Markdown parsing | ❌ | Only `Event::Text` survives pulldown-cmark; headings/code/lists/tables discarded; `.md` titles/TOCs always empty; front matter TODO. |
| HTML rendering | 🔴 | `builder.rs` "Simple document rendering (placeholder)": output is `<html><body>{escaped raw source}</body></html>`. `DocumentContent::Display` returns the raw source. No AST rendering, layout, navigation, or asset links. |
| BuildEnvironment (read → merge → resolve → write) | ✅ (M2 wave 4) | `src/env/` replaces the never-constructed `src/environment.rs`. The build is now four phases over a real environment: a parallel read producing per-document doctrees (persisted as bincode under the `-d` dir, behind a `SUDT`+version header so an older format is an honest miss, `src/builder.rs`), a merge into a serialized `BuildEnvironment` (bincode, `ENV_VERSION`-stamped, `env.save`/`load`; M2 wave 5 moved `DOCTREE_FORMAT_VERSION` 2 → 3 — the persisted doctree is the post-transform tree — and `ENV_VERSION` 3 → 4 — the citation domain and typed metadata values — so the first build after upgrading is a cold one), a resolve pass per document over a *copy* of its doctree in docname order (mirroring Sphinx's `get_and_resolve_doctree` and therefore its warning order), and the write phase. Modules: `toctree.rs` (graph, `tocs`, `toctree_includes`, `files_to_rebuild`, relations, consistency warnings), `numbers.rs` (`toc_secnumbers`/`toc_fignumbers`), `std_domain.rs`, `genindex.rs`, `metadata.rs`, `dependencies.rs`, `resolve.rs`. |
| Environment differential oracle | ✅ (M2 wave 4) | `tools/gen_env_fixture.py` builds 36 projects (96 documents) with a real `SphinxTestApp` + `app.build()` on sphinx 9.1.0 and records the post-build environment; `tests/env_differential.rs` (66 tests) replays each project through this crate and compares every key: `tocs`, `toc_num_entries`, `toctree_includes`, `files_to_rebuild`, `relations`, `toc_secnumbers`, `toc_fignumbers`, the std registries, index entries, genindex, the full warning stream (since M2 wave 5 as whole records, `warning_records`, so a multi-line docutils message is one record), and each document's resolved-doctree pseudo-XML — **zero divergence**. Each corpus project is built exactly **once, cold** (`build_project` makes a fresh tempdir, never calls `enable_incremental`, and every corpus-wide assertion reads that single build); warm-equals-cold is a separate claim, asserted by hand-written tests in the same file over their own two- and three-document projects. Wave 4.5 added five compare keys (the py registries, `py_modindex`, `included`, and the file dependencies) and the py-domain and file-inclusion projects. M2 wave 5 sub-project 1 added seven projects (`reporter_interleave`, `inc_missing`, `inc_nested_error`, `keep_warnings_true`, `keep_warnings_false`, `citations`, `smartquotes_default`) and closed the PropagateTargets and MoveModuleTargets exemptions (ten documents), the `orphan` docinfo one, `inc_basic`'s reporter-output warning gap, and both `KNOWN_INERT_CONF` keys (`keep_warnings`, `smartquotes`). Strict, self-cleaning exemption tables (`KNOWN_WARNING_GAPS`, `KNOWN_RESOLVED_GAPS`, `KNOWN_HIGHLIGHT_STAMP_GAPS`; `KNOWN_TOC_GAPS`, `KNOWN_STD_GAPS` and `KNOWN_INERT_CONF` are empty) name what is not yet compared and why: 32/36 projects' warning streams match byte-for-byte (3 differ only by `image file not readable`, 1 by the write-phase circular-toctree warning), and 52/96 resolved doctrees match byte-for-byte: 36 are skipped wholesale by `KNOWN_RESOLVED_GAPS` (31 an unresolved `toctree` node — sub-project 2's `_resolve_toctree` — 4 an image without `candidates`, 1 a citation reference awaiting the citation domain's resolution) and 8 are compared in full except for the highlight stamp. Those figures are computed from the tables and the fixture by `exemption_arithmetic_matches_the_documented_numbers`, which first proves each table names a fixture document exactly once and the two document tables disjoint, so this sentence can no longer drift from the code (panel fix round B — the previous hand count was wrong in both directions). `KNOWN_HIGHLIGHT_STAMP_GAPS` is narrower than the others by construction: it still compares the entire tree and forgives nothing but the *presence* of a `HighlightLanguageTransform` attribute (`language`/`force`/`linenos` on a `literal_block`) that we do not emit at all — and never a `linenos="1"`, which in this corpus is directive-set by construction (the transform only ever stamps `"0"`; `inc_basic/b`'s `:lineno-match:` carries one). The `inc_highlight` project, in neither table, compares `language`/`force`/`linenos` set by `:language:`/`:linenos:`/`:lineno-start:`/`:force:` at full strength. Listing a project that has *stopped* diverging fails the test, so exemptions cannot outlive their cause; wave 4.5 also added the one-directional soundness rule that an exempted project's warnings must stay a SUBSET of the oracle's, so an exemption for a MISSING warning can never quietly cover an INVENTED one. |
| Toctree graph, relations, consistency warnings | ✅ (M2 wave 4) | Sphinx docname resolution (document-relative, `/`-absolute, `.`/`..`), `Title <doc>`, captions, URLs, `self`, `:glob:` with the dead-pattern warning, `:numbered:`/`:maxdepth:`/`:titlesonly:`/`:hidden:`/`:includehidden:`/`:reversed:`. The graph feeds `relations` (parents/prev/next, incl. Sphinx's quirk that a first child's `prev` is its parent) and the consistency warnings: nonexisting vs excluded entries, self-reference, circular toctrees, multiple parents (an *information* notice, not a warning), and `document isn't included in any toctree`. **Behavior change vs M1:** a toctree warning is now located at the `.. toctree::` directive line, as Sphinx locates it, not at the offending entry's line, and carries Sphinx's category suffix (`[toc.not_readable]`). Both changes are pinned by the env oracle and by `tests/e2e_cli.rs`. |
| Directive/role validation in the build | 🟡 inert (M2 wave 5, decision D1) | The pass still runs on every build (`validate_directives`, default on; `-D validate_directives=0` skips it) but reports nothing. Once docutils' own diagnostics printed, wave 5's audit (decision D1) removed every built-in check: each either repeated a report docutils or Sphinx already prints (a missing argument or content block, an unknown option, a flag given a value, an invalid image length or alignment, an empty `:doc:`/`:ref:`/`:download:` target) or had no Sphinx counterpart and fired on markup `sphinx-build` accepts (`Unusual image extension`, an empty `toctree` or `math`, unbalanced `math` braces, and the role heuristics for `doc`/`download`/`math`/`abbr`/`command`/`file`/`guilabel`). Commit `f29bcb9` lists every removed check, and each validator's `*_is_silent_*` tests pin the markup that drew it. Every built-in validator now returns `Valid`, `Unknown` (no validator registered) stays silent, counted in a debug log line, and the registries, traits, statistics and the pass remain as the extension point — whether to keep or delete them is the partner's call. History: wired in M1 wave 4; audited against the parser's `option_spec` tables in M2 wave 4.5, which fixed six drifts and removed four fabricated diagnostics; emptied by D1. |
| std domain + cross-reference resolution | ✅ (M2 wave 4) | `src/env/std_domain.rs` collects labels (explicit targets, section/figure/table/code-block anchors with their titles), glossary terms, `option`s with program scoping and unscoped fallback, `envvar`s, and `confval`s; `src/env/resolve.rs` is a port of `ReferencesResolver` + `StandardDomain.resolve_xref` for `:ref:`, `:numref:`, `:doc:`, `:term:`, `:option:`, `:envvar:`, `:keyword:`, `:token:`. Warnings are Sphinx's own texts and categories — `duplicate label …, other instance in …`, `undefined label:`, `unknown document:`, `term not in glossary:`, `unknown option:`, `numfig is disabled. :numref: is ignored.`, `no number is assigned for …`, `the link has no caption: …`. **Behavior change:** these follow Sphinx's `warn_dangling` flags, which are set on seven std reftypes — `ref`, `numref`, `doc`, `term`, `keyword`, `option`, `confval` (`domains/std/__init__.py:748-766`) — regardless of `-n`, so a broken reference of any of those seven now warns in a default build; `-n`/`nitpicky` widens the warning to the remaining reftypes. `nitpick_ignore`/`nitpick_ignore_regex` are honored. **M2 wave 4.5 re-scoped the "skipping python-domain references" notice**: its python-domain population is gone (py references now resolve and warn), and the notice survives as `N cross-domain reference(s) not validated (domain not implemented until M5)`, printed for `:c:`/`:cpp:`/`:js:`/`:rst:` references, which remain unvalidated until those domains land (see *Diagnostics* under the known divergences). The old wording said "python-domain" but always counted every non-std domain. |
| Section & figure numbering (`numfig`) | ✅ (M2 wave 4) | `src/env/numbers.rs`: section numbers from `:numbered:` toctrees respecting `numfig_secnum_depth`, then figure/table/code-block/`displaymath` numbering scoped by them, in Sphinx's order and with its alphabetical-domain `get_figtype` dispatch. `:numref:` renders through `numfig_format` (`{name}`/`{number}` new style and `%s` old style). Pinned by the env oracle's `toc_secnumbers`/`toc_fignumbers` keys and by the corpus's numfig projects. |
| Warning pipeline (`-W`, `-w`) | ✅ | Docutils reporter records (M2 wave 5: `WARNING`/`ERROR`/`CRITICAL`, category `docutils`), toctree, environment and resolution warnings all flow through it; `-W` exits 1 with sphinx-build 9.1's exact behavior (collect-all; keep-going is the default since Sphinx 8.1). M2 wave 4 added Sphinx's warning **categories**: a warning logged with a `type` renders a ` [type.subtype]` suffix (`show_warning_types`, on by default since Sphinx 8.3); a `subtype`-only warning prints bare, like Sphinx's. |
| Error pipeline | ✅ | Per-file failures (an unreadable source) are collected as `BuildErrorReport`s while the build continues; **builds with such errors exit 1**, `-W`+warnings exits 1, usage errors exit 2 via clap (2026-08). A docutils `ERROR`/`CRITICAL` record is not one of them: since M2 wave 5 (decision D2) it is a warning, as in `sphinx-build` — exit 0 without `-W`. |
| Static asset copying | 🟡 | Copies 5 handwritten shim files (incl. a 61-line fake jquery.js) + project `_static`/`_templates`; generated pages reference none of them; `html_static_path` ignored by the live path. |
| genindex data | ✅ (M2 wave 4) | `src/env/genindex.rs` ports `IndexDomain.process_doc` (5-tuple entries, `split_index_msg` validation with Sphinx's `invalid {type} index entry {value!r}` warning and node removal) and `IndexEntries.create_index` (single/pair/triple/see/seealso, `!main` promotion, Symbols and `_` grouping, insertion-ordered sub-entries, the dropped-entry notice). Compared against the oracle's `genindex` key for every corpus project. M2 wave 4.5 added the **py-modindex** data (`PythonModuleIndex.generate`: first-letter grouping after `modindex_common_prefix` stripping, the collapse flag, and the synopsis/platform/deprecated columns), compared against the oracle's `py_modindex` key. Neither has a renderer until the wave-5 HTML writer. |
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
| `directives/validation/` (10+10 validators) | ✅ wired, inert (M2 wave 5) | Runs on every build and, since decision D1 removed every built-in check, reports nothing (see pipeline table). |
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
| Config knobs actually consumed | 🟡 | Consumed now: `max_cache_size_mb`, `cache_expiration_hours` (M1 wave 3); `nitpicky`, `validate_directives`, `doctree_dir`, `fail_on_warning`, `include/exclude_patterns`, `parallel_jobs` (M1 wave 4); `root_doc`/`master_doc`, `numfig`, `numfig_format`, `numfig_secnum_depth`, `nitpick_ignore`, `nitpick_ignore_regex`, `intersphinx_mapping`, `intersphinx_disabled_reftypes`, `intersphinx_resolve_self`, `intersphinx_cache_limit`, `intersphinx_timeout`, `tls_verify`, `tls_cacerts`, `user_agent` (M2 wave 4 — each with the same conf.py/YAML/`-D` plumbing and, for the intersphinx ones, Sphinx's own `ConfigError` texts on malformed input); `smartquotes`, `smartquotes_action`, `smartquotes_excludes`, `keep_warnings`, `language` (SmartQuotes' quote set), `version`, `release`, `today`, `today_fmt` (the default substitutions) and `highlight_language` (a sphinx-mode `code-block` without a language) (M2 wave 5, at Sphinx 9.1's defaults — `version`/`release` now default to unset, read as `''`, where earlier builds defaulted them to `1.0.0`). Still decorative until their consumers land (M2 wave 5/M3): `html_theme`, `theme.*`, `output.syntax_highlighting`/`highlight_theme`/`minify_html`/`search_index`, `optimization.*`, `html_static_path`, `html_context`, `tags`. |
| `-D key=value` overrides | ✅ | Wave 4: typed coercion against the field's existing type, dotted paths for nested sections and map settings (`html_context.name=value`), duplicated-pair sync (`html_theme`, `templates_path`, `html_static_path`), unknown keys warn with sphinx-build's message and count toward `-W`/the `-w` file. Known gap: conf.py *parser* warnings still bypass the `-W` totals (config-diagnostics channel is M2). |

## CLI vs sphinx-build

| Capability | Status |
|---|---|
| `build --source/--output`, `-j`, `--clean`, `--incremental`, `-W`, `-w` | ✅ (relative `--source` crash fixed 2026-08) |
| Positional `SOURCEDIR OUTPUTDIR`, `-b html`, `-M html/clean`, `-D`, `-A`, `-n`, `-q`, `-E`, `-a`, `-c`, `-t`, `-T`, `--keep-going`, `-j auto`, repeatable `-v` | ✅ (wave 4) — sphinx-build compatible argument mode; parity measured against real sphinx-build 9.1.0 (exit codes, `-M` output layout, message shapes). Non-html builders and make-mode targets exit 2 with an honest message. Trailing FILENAMES accepted with a not-supported-yet warning. A source dir literally named `build`/`clean`/`stats` needs `./`-prefixing (documented). |
| Non-zero exit on build errors | ✅ exit 1 on build errors and `-W`+warnings (all warnings collected first, sphinx 9.1 behavior), 2 on usage/config/unsupported-builder errors. A docutils `ERROR`/`CRITICAL` diagnostic exits 0 without `-W`, as in sphinx-build (M2 wave 5, decision D2). Deliberately **stricter** than sphinx-build on the crate's own read failures: an unreadable source silently passing CI is the exact M1 trust problem, so that exits 1. sphinx-build mode also refuses an output dir that equals/contains the source dir (exit 1) and requires a config (exit 2), like sphinx-build. |
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

**1227 tests passing, 7 ignored** (as of M2 wave 5 sub-project 1,
`CARGO_INCREMENTAL=0 cargo test --locked` on 2026-10-01). That is what
`cargo test` reports across its thirteen targets — 1033 lib + 8 bin + 186
integration; the 7 ignored are `tests/html_differential.rs`'s page-oracle
tests, off until the HTML writer lands; **0 of them are doc-tests** (the run
lists `Doc-tests sphinx_ultra … running 0 tests` separately). A raw `#[test]`
grep over `src/` and `tests/` returns 1232; with
`tests/inventory_roundtrip.rs`'s two `#[tokio::test]`s that is 1234 — cargo's
1227 passing plus the 7 ignored.
Every generator below is pinned to sphinx 9.1.0 / docutils 0.22.4 and asserts
those versions at runtime; all five reproduce their committed output
byte-identically. `PYTHONNOUSERSITE=1` is on every *live* regen command — the
five generators, the four `tests/*.rs` headers, the rows below and the two
emitted Rust headers — as of panel fix round E; the historical plan documents
under `docs/superpowers/plans/` still quote the un-flagged forms they were
written with. The two *table* generators that emit Rust source
(`tools/gen_digit_tables.py`, `tools/gen_punctuation_tables.py`, and the
headers they write into `src/rst/digits.rs` and `src/rst/punctuation.rs`) were
the last holdouts; round D's note scoped them out rather than fixing them. Both
also need a `cargo fmt --all` after a regen, which their docstrings now say.
M2 wave 5 added a third, `tools/gen_smartquotes_tables.py` (docutils 0.22.4's
SmartQuotes tables, written into `src/transforms/smartquotes_tables.rs`), with
the flag and the `cargo fmt --all` step in its docstring and header from the
start.

| Suite | Status |
|---|---|
| Unit tests (lib + bin) | ✅ 1041 passing (1033 lib + 8 bin) |
| Pattern compatibility tests | ✅ 10 passing — assertions encode Sphinx 9.1 semantics (M1 wave 4) |
| Pattern differential suite | ✅ 881 generated cases vs `sphinx.util.matching` 9.1.0, zero divergence; regenerate with `PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx>=9.1,<9.2' python tools/gen_pattern_fixture.py` |
| Doctree differential suite (docutils parse layer) | ✅ 761 generated cases vs docutils 0.22.4, zero divergence — the tree and, since M2 wave 5, the reporter stream each case writes (2 tests); regenerate with `PYTHONNOUSERSITE=1 uv run --python 3.12 --with docutils==0.22.4 python tools/gen_doctree_fixture.py` (the flag is not optional — `uv run` keeps user site-packages on `sys.path`, and a user-site Pygments there silently re-records every `code:: python` case as tokenized output) |
| Sphinx doctree differential suite (real read phase) | ✅ 697 generated cases vs a `sphinx-build` 9.1.0 read phase, zero divergence — since M2 wave 5 compared after the read transforms, with each case's printed records and collected metadata (8 tests); regenerate with `PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python tools/gen_sphinx_fixture.py` |
| Environment differential suite | ✅ 66 tests over 36 projects / 96 documents vs a real `SphinxTestApp` build, zero divergence on every compared key (exemption tables above). The corpus comparison is over one **cold** build per project; warm-equals-cold is asserted by hand-written tests over their own two- and three-document projects. Same `uv` invocation with `tools/gen_env_fixture.py` |
| HTML differential harness (`tests/html_differential.rs`) | ✅ 9 passing, 7 ignored — the page-level oracle a real `sphinx-build -b html`/`-b dirhtml` 9.1.0 wrote for the committed corpus (`tests/fixtures/html_differential_*.json`); the fixture, normalization and helper checks pass, and the seven per-key output comparisons are `#[ignore]`d until the HTML builder is wired |
| Inventory round-trip suite | ✅ 5 tests (3 `#[test]` + 2 `#[tokio::test]`, so a bare `#[test]` grep undercounts it) over 12 committed `.inv` files (4 sphinx-written, 3 handcrafted-valid, 5 handcrafted-malformed), expectations taken from Sphinx's own `InventoryFile.loads`; same `uv` invocation with `tools/gen_inventory_fixture.py` |
| Doctree serde / interner-cap suites | ✅ 6 passing (5 + 1) — bincode round-trip, the escape offsets' encoding, and the interner's bound |
| Property tests (`tests/rst_proptest.rs`) | ✅ 18 passing — the parser never panics on arbitrary, multiline, multibyte or deeply nested input, and (wave 4.5) on arbitrary py and std object signatures, arbitrary annotations through `parse_annotation`, and arbitrary `include`/`literalinclude` option blocks and file arguments against a real scratch srcdir (the file-argument sweep draws control characters, newlines, absolute and `..` paths since panel fix round B, and pins totality only — sphinx reads whatever path `relfn2path` yields, so "never reads outside the project" is not a property either side has). Round 1 widened the std sweep's body generator, whose fixed ASCII lines at three fixed indents could not reach the `glossary` dedent branch that broke totality: it now draws an arbitrary indent over text carrying 2-, 3- and 4-byte characters. **M2 wave 5** made the property the Sphinx read's: every generator's input also runs through `parse_and_transform` under a drawn `TransformConfig` (`keep_warnings`, SmartQuotes on/off, arbitrary actions and languages, excluded builders, arbitrary `today_fmt` and build date), and two generators are new — transform-shaped documents whose substitution, target, footnote and citation names collide (cycles and case variants included), and `transforms_survive_the_deep_nesting_sweep`: a quarter of its cases nest 210–260 levels — every level but the innermost one of the 14 containers that nest in each other, the innermost also possibly `topic`, `sidebar` or `versionadded`, which end a chain — so they pass the parser's 200-level guard, which the test asserts each of them prints, and the transforms then run over those deepest trees; the other cases nest 1–39 levels (`the_deep_nesting_documents_reach_the_guard` shows each of the 14 reaches the guard on its own). Every case runs under a 60 s per-case timeout (proptest's forked `timeout`), so a transform that never ends — or whose work outgrows the document — fails the sweep instead of hanging it. That is how a 2,048-case run (task 15, review round 3) found a self-doubling substitution definition (`.. \|a\| replace:: \|a\|_ x` beside `.. \|A\|`) expanding to 8,192 levels, which every later transform walked in time quadratic in that depth (60.2 s): the depth limit above now ends it, `a_self_wrapping_substitution_stops_at_the_depth_limit` pins the minimal input, and DanglingReferences no longer walks up from, or scans the siblings of, each reference. Failing seeds persist beside the test file (`tests/rst_proptest.proptest-regressions`, gitignored — a sweep's find is kept as a named test, since a seed replays only while the strategies stay as they were); `RST_PROPTEST_CASE_TIMES=1 … -- --nocapture` prints each deep case's time. Green at `PROPTEST_CASES=2048` (18/18, 217.3 s); the deep sweep alone at 2,048 cases with `PROPTEST_RNG_SEED=20261001` is green too, its slowest case 6.1 s (260 levels; mean 263 ms over its 515 cases of 210 levels or more) |
| `tests/e2e_cli.rs` | ✅ 61 passing — the real binary against fixture projects: exit codes, warning text, output trees, `--config` routing, sphinx-build mode, incremental/dependency rebuilds, and (M2 wave 5) 260-level directive nesting reaching the parser's guard instead of the stack limit, under a 512 KiB main thread too |
| Benchmarks | ❌ `benches/builder_benchmark.rs` panics at line 69 (`No such file or directory`) — it hands the parser a `test.rst` path that does not exist, so `cargo test --all-targets` and `cargo bench` fail. Pre-existing and outside plain `cargo test`, which is why no wave caught it. The rest exercise the placeholder write path (numbers measure escaped-text copying) and the cache benchmark is `black_box(42)`. Rewrite is scheduled with M2 wave 5. |

## Known divergences from Sphinx 9.1.0 (M2 wave 5, sub-project 1)

Every entry was probed against the pinned toolchain and is recorded, with its
input, in the sub-project's ledger. Two kinds: **deliberate divergences** —
where `sphinx-build` crashes, hangs or fetches over the network, this crate
prints a record and carries on — and **parked divergences**, left for the
sub-project that owns them.

**Transforms not yet run.** Rewritten from the research gap table
(`docs/superpowers/research/2026-09-30-m2-wave5-transforms.md` §10): the read
transforms of its rows T1–T12, T20 and T21 all run now (the *Read transforms*
row above). What remains is write-time work, sub-project 2 and later:

- `HighlightLanguageTransform` and `TrimDoctestFlagsTransform` (post-transforms
  400/401, T17): a `literal_block` carries no stamped `language`/`force`/
  `linenos`. Exempted narrowly by `KNOWN_HIGHLIGHT_STAMP_GAPS` (8 documents;
  see the env-oracle row).
- Toctree resolution (`_resolve_toctree` and its family, T18): a `toctree`
  node stays unresolved — 31 of `KNOWN_RESOLVED_GAPS`' 36 documents, and the
  `toctree_circular` project's warning gap.
- `ImageCollector` and `DownloadFileCollector` (T13, T14): no
  `image[candidates]` (4 documents) and no `image file not readable` warning
  (3 projects' warning gaps).
- Citation resolution (the write half of T6): the `pending_xref` the citation
  reference transform makes stays unresolved (1 document). The read half —
  `citation[docname]`, `duplicate citation …`, `Citation [..] is not
  referenced.` — runs.
- The math domain (T15): equation registration, `:eq:`, `:name:` on `math`.
- `OnlyNodeTransform` and the tags evaluator (T16), and `ReferencesResolver`
  handing a `pending_xref`'s ids, names and classes to its replacement (T19).
- Transforms proven no-ops for an HTML build (StripComments, Decorations,
  Validate, ExposeInternals, UIDTransform, i18n without catalogs,
  AutoIndexUpgrader, RefOnlyBulletList under `html_compact_lists=True`) are
  named in `READ_TRANSFORMS` and not ported.

**Deliberate: crate-only messages.** Each has no Sphinx counterpart and
replaces a crash or a network fetch; dropping it would drop content silently,
so it prints, counts as a warning and fails `-W` (the nesting guard's ERROR is
one too; it is listed with the crash fallbacks below). Input — `sphinx-build`
— this crate:

- **`.. include::` with `:parser:`** — runs the named parser — `CRITICAL:
  Problem with "include" directive:` / `parser mode is not supported by
  sphinx-ultra (planned with MyST, M2 wave 6)`, nothing included.
- **`include`'s `:tab-width:` beyond a C `int`** — an uncaught
  `OverflowError` aborts the build (`literalinclude` warns, as this crate
  does) — `CRITICAL: Problem with "include" directive:` / `Python int too
  large to convert to C int`.
- **`.. raw::` with `:url:`** — docutils fetches the URL — `CRITICAL:
  Problems with "raw" directive URL: fetching is not supported.`, nothing
  fetched.
- **A `:glob:` toctree pattern this crate cannot compile** — Python's
  `fnmatch` cannot fail — `toctree glob pattern '…' is not usable: …`
  (`ToctreeWarningKind::PatternError`) instead of an empty match.

**Deliberate: crash- and hang-versus-continue.** `sphinx-build` aborts or
never finishes; this crate prints what Sphinx prints (or would) and carries
on. Input — `sphinx-build` — this crate:

- **Content nested more than 200 levels** — dies with `RecursionError` well
  before 200 (probed: from 98 nested `note`s, 82 nested `py:function`s;
  docutils alone from 110 notes, 165 bullet lists or block quotes) — the
  parser's guard (`MAX_NEST_DEPTH`, `src/rst/block.rs`) fires at 200 levels:
  `ERROR: Maximum nesting depth exceeded; deeper content skipped.
  [docutils]`, the deeper content dropped, the build carried on. Every thread
  that parses has the stack to get there (`rst::PARSE_STACK_SIZE`, 64 MiB,
  unless the system refuses a thread that size);
  pinned end to end on 260 nested `note`s, admonitions and `py:function`s
  (`nesting_past_the_guard_prints_its_error_instead_of_aborting`, and under
  a 512 KiB main thread).
- **`today_fmt` with `%U` or `%W`** — logs `Invalid Babel locale: 'en'.` and
  aborts (`ValueError: Invalid length for field: 'WW'`) — the token is kept
  as written.
- **A `SOURCE_DATE_EPOCH` Python cannot read** (not a float, an infinity,
  NaN, or a year outside 1–9999), in a document that uses `|today|` with
  `today` unset — aborts
  (`ValueError`/`OverflowError`) — the build date is the current time.
- **A substitution definition naming one that does not exist, used before
  it** (docutils' `KeyError`, `references.py:726`) — aborts — `ERROR:
  Undefined substitution referenced: "<name>".` for the nested reference, a
  `problematic` in its place, everything else expanded.
- **A second "Circular substitution definition detected" for a definition
  already replaced, or for a discarded copy** (`parent.index(old)` raises,
  `nodes.py:1101-1103`) — aborts, having printed its records — the same
  records, and the expansion stops there; references it had not reached stay.
- **A substitution cycle through names that differ only in case**
  (`.. |A| replace:: |b|`, `.. |b| replace:: |A|`, `.. |a| replace:: z`, then
  `|A|`) — never finishes — a backstop (a BLAKE3 digest of the expansion
  state, which repeats only where docutils would loop for ever) ends it with
  the ordinary circular-substitution errors.
- **A substitution definition 1,000 levels deep** — only expansion grows
  one so deep: a definition wrapping a reference to itself under a name
  another definition folds onto (`.. |a| replace:: |a|_ x` beside
  `.. |A| replace:: y`) escapes the circularity test and doubles at each
  expansion of its own reference — dies with `RecursionError` measuring it
  for the line-length check (`len(subdef.astext())`, `references.py:696`;
  probed: builds when the doubling stops at 512 levels, dies at 1,024; 13
  frames lie below Substitutions, so from 987) — a reference to a
  definition that deep is not expanded (`MAX_DEFINITION_DEPTH`,
  `src/transforms/references.rs`): `ERROR: Substitution definition "a"
  exceeds the maximum nesting depth. [docutils]` at the reference, a
  `problematic` in its place, the rest expanded. Without the limit the
  doubling ran on to 8,192 levels with two characters a level (with none,
  until memory ran out), and every later transform walked each copy in
  time quadratic in that depth: the deep proptest sweep's 60 s timeout
  (its 220-level case: a 43 s release build before, 0.7 s after).
- **A bibliographic field whose body holds only error messages**
  (`:version:` + an unknown directive) — aborts (`IndexError`,
  `frontmatter.py:463`) — `Bibliographic field "version"` / `must contain a
  single <paragraph>, not [].` and the build continues.
- **Chains deeper than CPython's recursion limit** — indirect hyperlink
  targets naming each other, or substitutions whose links wrap the next in a
  reference (`|x|_`) — `RecursionError` — explicit stacks resolve them (the
  recursion limit is not modelled, but for a substitution definition too
  deep to measure, above), printing whatever records the document earns.
- **IndirectHyperlinks' own `ValueError`/`KeyError`** — a `problematic`
  docutils would put in place of a node already out of its parent; an id no
  node in the tree carries — aborts — the replacement is skipped; the target
  is left unresolved.

Shared with Sphinx, not a divergence: substitution definitions whose
expansions double from one to the next run up to the line-length limit only
after work exponential in their number (docutils is slower still). No
expansion budget is imposed — any cap on work would change the output of a
document docutils finishes; the depth limit above fires only where docutils
has died — so a crafted document can make a build very slow, as it can
`sphinx-build`.

**Parked divergences (known limitations).** Each is left for the backlog
named. Most are pre-existing parse-layer gaps the new oracles exposed; the
SmartQuotes warning's line, the English-only `|today|` and bibliographic
names, the Transitions shortcut and the warm-build citation warning are
limits of this sub-project's own transforms.

- *Parse layer (the parser-gap backlog).*
  - Messages inside grid and simple table cells are numbered one line low
    (`+-----+` / `| *a  |` / `+-----+`: docutils line 3, here 2).
  - A Sphinx-mode unknown role (`` :foo:`x` ``) becomes a silent
    `pending_xref`; Sphinx prints `ERROR: Unknown interpreted text role
    "foo".` with a `problematic` — so `-W` passes here where it fails under
    Sphinx.
  - `centered` is an unknown directive; `versionadded`/`versionchanged`/
    `deprecated` join their content into one paragraph where Sphinx parses
    it as body elements.
  - Sphinx-mode `:code:` restores a backslash Sphinx drops
    (`` :code:`e\*f` `` reads `e*f` in Sphinx, `e\*f` here) and lacks Sphinx
    9.1's `language=""`; an escaped literal-block marker (`b\::`) makes a
    literal block here, a paragraph `b::` + block quote under docutils;
    `` :emphasis:`\ ` `` is an `emphasis` there, a `problematic` with a
    spurious start-string warning here; `:samp:` with braces
    (`` :samp:`a{b}c` ``) is one Text here, `a` + `emphasis` + `c` there.
  - `.. rst-class::` before a section title stamps the next paragraph here,
    the section in Sphinx — so a `language-xx` class there governs different
    SmartQuotes units.
  - An inline-markup start-string right after a Unicode `Pd`/`Po` character
    (`–**x**`) is not recognized here, as docutils recognizes it — about 6%
    of random fuzz paragraphs diverge before SmartQuotes runs.
  - docutils-only settings — `smart_quotes: alt…` and `smartquote-locales`,
    set from `docutils.conf` — are not modelled.
  - docutils-mode edges: the inline-target duplicate INFO's placement; the
    error lines of references in a field name or a later line-block line;
    the order inside docutils' bookkeeping lists.
- *Locations of rare records.*
  - An undefined-substitution error inside an attribution, a field name or a
    glossary term carries the wrong line (the inline span's line
    bookkeeping).
  - A duplicate label set by a `rubric`'s `:name:` prints `x.rst::` under
    Sphinx (docutils gives the rubric no line) and the rubric's line here.
  - `No smart quotes defined for language "xx".` follows the parse layer's
    line, which differs from docutils' for directive-made rubrics and figure
    captions (`:class: language-zz` on a rubric: Sphinx `::`, here `:6`;
    `:figclass:`: Sphinx `:4`, here `:1`); its text, level, count and `-W`
    effect are right.
  - A reference inside the line-less paragraph of `Problematic content in
    substitution definition` (``.. |x| replace:: `a`_ *b`` + `Text |x|.`)
    prints `a.rst:4:` where Sphinx prints `a.rst::` (it printed `:0:` before
    this sub-project — wrong both times).
  - A document ending in `.. include::` and trailing blank lines, or in an
    include inside a block quote, puts the end-of-parse location (anonymous
    hyperlink mismatch, substitution line-length) on the main file's last
    line instead of the included file's padding.
  - A reporter record raised inside an included file names it relative to
    the srcdir where docutils names it relative to the cwd — the same text
    when `sphinx-build` runs from the srcdir (the *Provenance path spelling*
    entry below).
- *Documents that already print an error.* Footnote references the parser
  dropped (in a refused substitution definition) or replaced after a failed
  indirect target stay numbered and linked in Sphinx, whose lists keep them;
  here they do not.
- *Transitions.* Handling each transition once misses docutils' second-visit
  `Transition must be child of <document> or <section>.` for a transition
  moved into a non-structural parent — unreachable while nested parses
  reject section titles (the `match_titles` gap in the RST-parsing row).
- *Warm builds.* `check_consistency` runs on every build where Sphinx runs
  it only when documents were re-read, so a no-change rebuild repeats orphan
  warnings and `Citation [..] is not referenced.` (sub-project 6).
- *The math domain (sub-project 2).* Duplicate `math` `:label:`s print
  neither Sphinx's `duplicate label of equation …` nor its `Duplicate ID`
  error.
- *i18n (ROADMAP M7).* `|today|` is formatted in English for every
  `language` (Sphinx formats with Babel's locale data — `de` gives
  `Feb. 13, 2009`); only docutils' English bibliographic field names are
  recognized.
- *Performance.* `ReorderConsecutiveTargetAndIndexNodes` takes time
  quadratic in the length of a run of adjacent targets.

**A note on the design.** The sub-project's spec
(`docs/superpowers/specs/2026-09-30-m2-wave5-sp1-reporter-transforms-design.md`
§6) says the escape offsets beside a text node are "written only by the
inline parser". As built — and as ruled during the work — they are written
wherever docutils builds a Text from `str(node)`: the inline parser, the block
parser's definition-list term split, and the transforms that rebuild a text
node (`:trim:`, the RCS-keyword cleanup, the `authors` split, SmartQuotes).
The approved spec is left as it is; the code follows this reading.

## Known divergences from Sphinx 9.1.0 (M2 wave 4.5)

Every one of these was found by probing the real toolchain, is documented at its
code site, and is deliberate. They are listed here so nobody has to rediscover
them. Divergences from earlier waves are recorded in the tables above.

**The file-inserting directives.**

- `:parser:` is refused, not silently ignored: `.. include:: f.md` with
  `:parser: myst` produces a SEVERE reading `parser mode is not supported by
  sphinx-ultra (planned with MyST, M2 wave 6)`. Sphinx would run the named
  parser.
- `:code:` **with a language argument** goes through this crate's Pygments-less
  `code` machinery (wave 3), which emits docutils' `Cannot analyze code. Pygments
  package not found.` where the sphinx oracle — which ships Pygments — tokenizes.
  The language-less form is at parity, and both probe-verified forms are the only
  ones the oracle corpus uses.
- **Provenance path spelling.** docutils' `adapt_path` keeps the cwd-relative
  spelling of an *included* file only in the node `source` attribute; what a
  user sees from Sphinx is **absolute** — an in-include warning location is
  cwd-absolutized for a node location (`sphinx.util.logging.get_node_location`)
  and srcdir-joined for a `(source, line)` tuple (`env.doc2path`). This crate
  spells included-content provenance srcdir-relative throughout — node `source`
  attributes, circular-inclusion chains, SEVERE texts and in-include warning
  locations alike — so on the CLI a user sees a bare relative path
  (`part.rst:5: WARNING: …`) where `sphinx-build` prints the absolute one;
  the message bytes are identical.
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
  names="t">` there, `<target ids="t" names="t">` + `<paragraph>` at the parse
  layer, which stays transform-free for the docutils oracle by design. **Closed
  for the build in M2 wave 5:** the read transforms apply it in the tree (the
  sphinx oracle's `tx_targets` family pins it), and the env oracle's "unapplied
  `PropagateTargets`" exemptions are gone.
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
- Equation references are parsed but not resolved: `:eq:` now carries Sphinx's
  `refdomain="math"`, which puts it in the "domain not implemented" count
  beside `c:`/`cpp:`/`js:` rather than through the std resolver. The math
  domain is wave-5 work.

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

- **References into a domain this build does not implement are counted, not
  warned about.** Every `refdomain` outside `{"", "std", "py"}` — `:c:`,
  `:cpp:`, `:js:`, `:rst:` — short-circuits in `src/env/resolve.rs` and is
  summed into the one-line `N cross-domain reference(s) not validated (domain
  not implemented until M5)` notice; sphinx resolves them and warns under `-n`.
  Closes with M5.
- **A `:doc:` target that looks like a URL is exempt from `unknown document:`.**
  An M1 heuristic kept deliberately (`src/env/resolve.rs`, pinned by the CLI
  e2e suite): `:doc:`https://example.com/page`` is treated as somebody
  linking out. Sphinx has no such carve-out and warns `unknown document:
  'https://example.com/page' [ref.doc]`.
- The three **glossary misformat warnings** are reported ONE LINE LOW, because
  sphinx reports them at a 0-based `content.items` offset that the reporter then
  renders as a 1-based line. Reproduced deliberately — the oracle compares bytes.

## Historical note

Previous versions of this document (and the README/VALIDATION_FEATURES_PLAN) marked
the domain system, directive/role validation, and constraint engine as "Fully
Implemented ✅". That was true of the *library code and its unit tests* but not of
the product: none of the three systems had ever been invoked by `sphinx-ultra build`.
This document tracks binary-reachable behavior only. M1 wired directive/role
validation (and M2 wave 5 removed every built-in check, decision D1); M2 wave 4
replaced the domain system outright with an oracle-pinned std domain and deleted
the original (`docs/DOMAIN_SYSTEM.md`, which documented only that API, went with
it); the constraint engine is still library-only, waiting on a `ContentItem`
producer in M4.
