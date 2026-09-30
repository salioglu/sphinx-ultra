# M2 wave 5 research — part 4: transforms, collectors, post-transforms, toctree resolution

Research key: `transforms`. Upstream pinned: Sphinx 9.1.0 / docutils 0.22.4 (installed at
`SPHINX=/root/.cache/uv/archive-v0/b4dBDAdEzskuqge1iT52j/lib/python3.12/site-packages/sphinx`,
`DOCUTILS=…/site-packages/docutils`). Every "probe" below was run with the pinned
`PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4'`
toolchain against real `SphinxTestApp` HTML builds (probe scripts are in
`scratchpad/probe-transforms/`: `harness.py`, `p_*.py`; they are throwaway, not committed).
Crate paths are relative to `/home/user/sphinx-ultra`.

---------------------------------------------------------------------------------------------

## 0. Executive summary (read this first)

1. **The crate runs no transform pass at all.** `src/rst/mod.rs:5-9` says so explicitly
   ("Transforms (doctitle promotion, target propagation, transition hoisting, message
   filtering) are explicitly NOT applied here"). What the crate *does* have is (a) a handful of
   transform *effects baked into the parser* because the read-phase oracle required them
   (ClassAttribute, GlossarySorter, RemoveTranslatableInline shapes, the post-propagation
   target shape of `.. math:: :label:`, a parse-time AutoNumbering id for captioned
   `literalinclude`), (b) *read-only replays* of `PropagateTargets` for label lookup and figure
   numbering (`src/env/std_domain.rs:485-700` `DocumentIds`/`PropagatedIds`), and (c) the
   write-time `ReferencesResolver` (std/py/any/intersphinx) + `PropagateDescDomain`
   (`src/env/resolve.rs:777-848`).
2. **The biggest HTML-visible gap is not a Sphinx transform but the docutils *reference*
   transforms** that Sphinx inherits from `standalone.Reader`: `Substitutions`,
   `PropagateTargets`, `AnonymousHyperlinks`, `IndirectHyperlinks`, `Footnotes`,
   `ExternalTargets`, `InternalTargets`, `DanglingReferences` (as
   `SphinxDanglingReferences`), `DocInfo`, `Transitions`. None exist in the crate
   (`grep -r` for their names in `src/` finds nothing but comments). Without them:
   `|subst|` stays a `substitution_reference`, `` `name`_ `` stays `refname=` (no href),
   anonymous links have no target, auto-numbered footnotes have no number/label/backrefs,
   `:orphan:`/`:tocdepth:` field lists render as a visible field list, and every block-level
   `.. _label:` renders in the wrong place. **An HTML writer on today's tree cannot produce a
   correct link for any `` `x`_ `` reference.**
3. Sphinx runs the whole read-transform list **per document, before pickling** (i.e. before
   the env collectors and domain `process_doc`). Exact applied order (probed from
   `document.transformer.applied`, §1.2) — 42 transforms, of which ~20 have observable effects
   for an HTML build. The persisted doctree is the post-transform tree.
4. The write side is: `get_and_resolve_doctree` = post-transforms (ReferencesResolver 10,
   OnlyNodeTransform 50, ImageDownloader 100 / DataURIExtractor 150 [no-ops for html],
   SigElementFallback 200 [no-op for html], PropagateDescDomain 200,
   HighlightLanguageTransform 400, TrimDoctestFlagsTransform 401) → `doctree-resolved` →
   `_resolve_toctree` for every `toctree` node → then (HTML only) `post_process_images` →
   translate. Crate has ReferencesResolver (partial: no `math`/`citation` domains, no
   `replace_self` attribute transfer) and PropagateDescDomain; **everything else is missing**.
5. **SmartQuotes is default-ON in Sphinx and not applied by the crate at all.** Both crate
   oracles pin `smartquotes=False` (`tools/gen_env_fixture.py:139`,
   `tools/gen_sphinx_fixture.py:40-41`), and `tests/env_differential.rs:887-935` lists it in
   `KNOWN_INERT_CONF` ("no smart-quote transform exists"). Every default Sphinx project emits
   “ ” ‘ ’ – — … in body text, titles, `<title>`, TOC entries, `:ref:` texts, glossary terms,
   captions and field names. It is *language-dependent* (de „…“, fr « … » with U+00A0, ja
   excluded) — §5. **Prerequisite:** the crate stores Text nodes *unescaped*
   (`src/rst/inline.rs:405-470`, `unescape()` at `:42`) and so has lost the information
   SmartQuotes needs to leave `\"`, `\'`, `\--`, `\...` straight. Recommendation (§12):
   HTML oracle keeps `smartquotes=False` as the base conf for structural families and adds a
   dedicated default-conf SmartQuotes family; implement the transform in wave 5 (it is a
   drop-in blocker: a default project diverges on nearly every page otherwise).
6. **Toctree resolution** (`_resolve_toctree` & friends, `adapters/toctree.py:119-575`) is fully
   specified in §8 with probed output shapes; it closes 23 `KNOWN_RESOLVED_GAPS` rows
   (`TOCTREE_RESOLUTION`) and the `toctree_circular` warning gap. **A real `sphinx-build -b html`
   crashes with `RecursionError` on a mutual toctree cycle** (probed, both alabaster and basic)
   — that project can never be an HTML oracle case.
7. Theme matters for warning *multiplicity*: with the default theme (alabaster) every page's
   sidebar calls `toctree()` → `global_toctree_for_doc` → `_toctree_entry` again, so a
   write-time toctree warning is emitted once per page (probed: 6 copies of
   `toc.no_title` for a 3-document project under alabaster, 1 under `basic`). Use
   `html_theme='basic'` in the v1 HTML oracle.
8. Other concrete, probed quirks the implementers must reproduce: `SortIds` moves a label to the
   front of a section whose own id starts with `id` (a section titled **"Identity"** with a
   `.. _lbl:` gets `<section id="lbl">` and toc anchor `#lbl`); `AutoNumbering` runs *before*
   `PropagateTargets`, so a `.. _f:`-labelled figure gets `ids="id1 f"` and HTML
   `id="id1"`; `MoveModuleTargets` (not PropagateTargets) is what gives `py_basic/a`'s section
   `ids="module-zmod a"`; references inside a non-matching `.. only::` are still resolved and
   still warn (ReferencesResolver 10 < OnlyNodeTransform 50); an image inside a substitution
   definition warns `image file not readable` **twice**; `math :name: e2` gives
   `ids="e2 equation-e2"` and with `numfig=True` `:eq:` renders `()`.

---------------------------------------------------------------------------------------------

## 1. The Sphinx 9.1 pipeline and where each transform runs

### 1.1 Phases (per document)

READ (`Builder.read` → `docnames = sorted(added | changed)`, `builders/__init__.py:512`; one
document at a time):

1. `_parse_str_to_doctree` (`util/docutils.py:847-908`): builds a `LoggingReporter`, installs a
   `SphinxTransformer`, adds, **in this order**, `_READER_TRANSFORMS`
   (`util/docutils.py:80-84` = `standalone.Reader().get_transforms()` minus
   `DanglingReferences`), the registry transforms (`app.registry.get_transforms()`), and the
   parser transforms (`RSTParser.get_transforms()` = docutils rst parser transforms minus
   docutils `SmartQuotes`, `parsers.py:66-74`). Then `parser.parse(...)`, then
   `transformer.apply_transforms()` (`util/docutils.py:906`).
2. Transform order = sort by the string `'%03d-%03d' % (priority, serial)` where `serial` is the
   insertion counter (`docutils/transforms/__init__.py:141-150`, sort+pop at `:177-193`). So
   equal priorities run FIFO in insertion order: reader transforms first, then registry
   transforms in extension-setup order, then parser transforms.
3. At priority 850 `SphinxDomains` runs every domain's `process_doc`
   (`transforms/references.py:33-41`) — std labels, index entries, citations are recorded
   **after** all reference transforms and SmartQuotes.
4. At priority 880 `DoctreeReadEvent` emits `doctree-read` (`transforms/__init__.py:418-424`),
   which runs the six env collectors (probed listener list, all priority 500, in this order):
   `DependenciesCollector`, `ImageCollector`, `DownloadFileCollector`, `MetadataCollector`,
   `TitleCollector`, `TocTreeCollector`.
5. At 999 `FilterSystemMessages` then `RemoveTranslatableInline`. The doctree is pickled.

BETWEEN: `env-get-updated` → `TocTreeCollector.get_updated_docs` =
`assign_section_numbers` + `assign_figure_numbers` (`collectors/toctree.py:194-378`);
`env.check_consistency()` (`environment/__init__.py:797-823`: orphan warnings, then
`_check_toc_parents`, then `domains._check_consistency()` → e.g. `Citation [%s] is not
referenced.`).

WRITE (`Builder.write`, `builders/__init__.py:705-747`): `env.toctree_includes` is re-sorted by
docname (`:739`), `prepare_writing` (HTML: `collect_relations()` → **RecursionError on a mutual
toctree cycle**), then `_write_serial(sorted(docnames))` inside `logging.pending_warnings()`
(buffered, order preserved). Per document, `_write_docname` (`builders/__init__.py:877-890`):

1. `env.get_and_resolve_doctree(docname, builder, tags=...)` (`environment/__init__.py:668-717`):
   `get_doctree` (unpickle, fresh `LoggingReporter`) → `apply_post_transforms`
   (`:759-776`; post-transforms sorted by priority, then `doctree-resolved` event) → for every
   `addnodes.toctree` in the tree: `_resolve_toctree(env, docname, builder, node, prune=True,
   includehidden=False, tags)`; `None` → `toctreenode.parent.replace(toctreenode, [])`
   (the `compound.toctree-wrapper` stays, empty), else `replace_self(result)`.
2. `builder.write_doc_serialized(docname, doctree)` — HTML: `post_process_images`
   (`builders/__init__.py:213-…`, html override `builders/html/__init__.py:961-987`) which
   rewrites `image['uri']` to the chosen candidate, then indexes for search.
3. `builder.write_doc(docname, doctree)` — HTML: translate + `get_doc_context` (+
   `document_toc`, `render_partial` of titles) + `handle_page` (templates; sidebars may call
   `toctree()` → `global_toctree_for_doc` → `_resolve_toctree` **again**, with its warnings).

### 1.2 Exact read-transform order (probed: `document.transformer.applied`, html builder, `extensions=[]`)

| prio-serial | transform | effect for an HTML build | crate status |
|---|---|---|---|
| 010-018 | `sphinx.transforms.ApplySourceWorkaround` (`transforms/__init__.py:237-245`, `util/nodes.py:127-190`) | source/line/rawsource patches for classifier/image/title/term/topic/literal_block — **location-only** (affects later warning line numbers, never output) | n/a (spans are set by the parser; verify image/title line = parent line where docutils leaves it unset) |
| 010-019 | `ExtraTranslatableNodes` (`:269-285`) | no-op unless `gettext_additional_targets` | not needed |
| 010-034 | `i18n.PreserveTranslatableMessages` (`i18n.py:103-111`) | toctree gets `rawentries` (list of explicit titles) and `rawcaption` | crate always emits `rawentries=""` (`src/rst/block.rs:6037`), never `rawcaption`; pformat-only (no HTML effect) |
| 020-035 | `i18n.Locale` | no-op without catalogs | not needed |
| 025-036 | `i18n.TranslationProgressTotaliser` | `document[translation_progress]` | stripped by both oracles' harnesses; no HTML effect |
| 100-033 | `compact_bullet_list.RefOnlyBulletListTransform` (`:54-84`) | no-op: `html_compact_lists` defaults True (probed) | only needed if `html_compact_lists=False` |
| 210-020 | `DefaultSubstitutions` (`:111-137`) | `|version|`, `|release|`, `|today|`, `|translation progress|` when not defined in the doc | **missing** |
| 210-021 | `MoveModuleTargets` (`:153-175`) | py module target at section index 2 → ids prepended to section, target removed | **missing** |
| 210-022 | `HandleCodeBlocks` (`:178-197`) | `block_quote` whose children are all `doctest_block` → unwrapped | **missing** |
| 210-025 | `AutoNumbering` (`:200-214`) | captioned/titled figure/table/container with `ids==[]` → implicit `idN` | **missing** (parse-time approximation for literalinclude only, `src/rst/block.rs:4234-4283`) |
| 210-026 | `AutoIndexUpgrader` (`:248-266`) | 4-tuple index entries → warning; never happens in core | not needed |
| 220-004 | docutils `references.Substitutions` (`references.py:642-764`) | substitution references replaced | **missing** |
| 220-032 | `ReorderConsecutiveTargetAndIndexNodes` (`:446-515`) | index nodes moved before targets in a consecutive target/index run | **missing** |
| 260-005 | docutils `references.PropagateTargets` (`:17-95`) | block target ids/names → next node; target gets `refid` | **replayed read-only** (`src/env/std_domain.rs:485-700`), not applied to the tree |
| 261-023 | `SortIds` (`:217-225`) | section ids: leading `id…` id moved last | **missing** |
| 320-006 | docutils `DocTitle` | **disabled** (`doctitle_xform=False`, `environment/__init__.py:69`) | correctly absent — do NOT implement |
| 340-008 | docutils `frontmatter.DocInfo` (`frontmatter.py:266-548`) | leading field list → `docinfo` (+ dedication/abstract topics) | **missing** (metadata read directly off the field list, `src/env/metadata.rs`) |
| 350-007 | docutils `SectionSubTitle` | disabled (`sectsubtitle_xform=False`) | correctly absent |
| 440-009 | docutils `AnonymousHyperlinks` (`references.py:98-160`) | anonymous refs ↔ targets | **missing** |
| 460-010 | docutils `IndirectHyperlinks` (`:163-338`) | indirect targets/refs resolved | **missing** |
| 500-024 | `DoctestTransform` (`:327-334`) | `doctest_block['classes'] += ['doctest']` | **missing** |
| 500-031 | `GlossarySorter` (`:427-443`) | sorts `:sorted:` glossaries | done at parse (`src/rst/block.rs:4706`) |
| 619-016 | `citation.CitationDefinitionTransform` (`domains/citation.py:133-148`) | `citation[docname]`, `note_citation` (duplicate warning), `label[support_smartquotes]=False` | **missing** |
| 619-017 | `citation.CitationReferenceTransform` (`:150-177`) | `citation_reference` → `pending_xref(refdomain='citation', reftype='ref', refwarn=True, support_smartquotes=False, ids=…, classes=…)` + `inline('[X]')` | **missing** |
| 620-011 | docutils `references.Footnotes` (`:416-635`) | numbering, labels, backrefs, symbol footnotes, manual footnote/citation linking | **missing** |
| 622-028 | `UnreferencedFootnotesDetector` (`:288-324`) | `Footnote [..] is not referenced.` warnings | **missing** |
| 640-012 | docutils `ExternalTargets` (`:340-373`) | `refname` → `refuri` | **missing** |
| 660-013 | docutils `InternalTargets` (`:376-411`) | `refname` → `refid` | **missing** |
| 700-015 | `latex.transforms.FootnoteDocnameUpdater` (`builders/latex/transforms.py:34-43`) | `docname` attr on every `footnote`/`footnote_reference` (registered for **all** builders) | **missing** (pformat-visible; not HTML-visible) |
| 740-003 | docutils `StripComments` | no-op (`strip_comments` unset) | not needed |
| 750-029 | `SphinxSmartQuotes` (`:361-415`) | typographic quotes/dashes/ellipses | **missing** (§5) |
| 820-001 | docutils `Decorations` | no-op (no generator/datestamp/source_link) | not needed |
| 830-014 | docutils `misc.Transitions` (`misc.py:64-143`) | transition validation/relocation + warnings | **missing** |
| 835-042 | docutils `Validate` | no-op (`validate` unset) | not needed |
| 840-002 | docutils `ExposeInternals` | no-op | not needed |
| 850-039 | `SphinxDanglingReferences` (`transforms/references.py:18-30` over docutils `:878-990`) | unresolved `refname` refs → `problematic` + ERROR; INFO suppressed | **missing** |
| 850-040 | `SphinxDomains` | domains' `process_doc` | ✓ (std/index/py in `src/builder.rs` merge phase) — citation/math missing |
| 880-030 | `DoctreeReadEvent` | collectors (§6) | partial |
| 880-041 | `versioning.UIDTransform` | no-op for html | not needed |
| 950-037 | `i18n.AddTranslationClasses` | no-op by default | not needed |
| 999-027 | `FilterSystemMessages` (`:337-347`) | removes every `system_message` with `level < (2 if keep_warnings else 5)` | **missing** — crate never strips (this is why `keep_warnings` is in `KNOWN_INERT_CONF`) |
| 999-038 | `i18n.RemoveTranslatableInline` (`i18n.py:692-706`) | unwraps `inline[translatable]` (docfields, changeset) | effect baked into the parser's docfield/versionmodified output |

Post-transforms (probed sorted order; latex/linkcheck/c/cpp ones are format/builder-gated no-ops
for html):

| prio-serial | post-transform | html effect | crate |
|---|---|---|---|
| 010-013 | `ReferencesResolver` (`post_transforms/__init__.py:62-309`) | resolve `pending_xref` | ✓ std/py/any/intersphinx (`src/env/resolve.rs`); ✗ `math` (`:eq:`), ✗ `citation`; ✗ `replace_self` attribute transfer (§7.1) |
| 050-014 | `OnlyNodeTransform` (`:324-332`, `util/nodes.py:719-742`) | `only` → children or empty `comment` | **missing** (+ no Tags evaluator) |
| 100-019 | `images.ImageDownloader` (`images.py:50-…`) | no-op: html `supported_remote_images=True` (`builders/html/__init__.py:126`) | not needed |
| 150-020 | `images.DataURIExtractor` (`images.py:129-160`) | no-op: html `supported_data_uri_images=True` (`:127`). **Active for the dummy builder** (base class default False) | only matters for the env (dummy) oracle with `data:` images |
| 200-015 | `SigElementFallbackTransform` (`:335-379`) | no-op (HTML5Translator is a `SphinxTranslator`; dummy has no translator) | not needed |
| 200-016 | `PropagateDescDomain` (`:382-390`) | `desc_signature['classes'] += [domain]` | ✓ `src/env/resolve.rs:831-848` |
| 400-017 | `code.HighlightLanguageTransform` (`code.py:30-86`) | stamp `language`/`force`/`linenos`, remove `highlightlang` | **missing** (`KNOWN_HIGHLIGHT_STAMP_GAPS`) |
| 401-018 | `code.TrimDoctestFlagsTransform` (`code.py:89-132`) | strip `# doctest:` flags / `<BLANKLINE>` | **missing** |

---------------------------------------------------------------------------------------------

## 2. What the crate has today (architecture map)

* **Parse** — `src/rst/mod.rs:386-396` `parse_rst_full` → `BlockParser::parse_document_full`.
  Output = docutils *parse-layer* tree (pinned by `tests/doctree_differential.rs`, 735 cases,
  `tools/gen_doctree_fixture.py:14` "PARSE-LAYER pseudo-XML (no transforms)") plus, in sphinx
  mode, sphinx directive/role shapes pinned against the *post-read-transform* sphinx oracle
  (`tests/sphinx_doctree_differential.rs`, 489 cases) whose corpus policy
  (`tools/gen_sphinx_fixture.py:71-80`) **excludes every case where the transforms change the
  tree** ("INFO stripping, PropagateTargets/IndirectHyperlinks/ExternalTargets/
  AnonymousHyperlinks target rewrites, Footnotes+FootnoteDocnameUpdater, DoctestTransform
  classes, doc-start docinfo consumption, image `candidates`, Transitions edge warnings…").
  That is exactly the list of transforms wave 5 must add.
* Transform effects **baked into the parser** (keep them; they are what Sphinx's tree shows):
  - ClassAttribute: `apply_pending_classes` `src/rst/block.rs:952-966` (sphinx mode stamps the
    next non-invisible sibling; docutils mode emits a `pending`).
  - GlossarySorter: `src/rst/block.rs:4706-4735`.
  - RemoveTranslatableInline / changeset: versionmodified paragraph with `translatable="0"`
    (`src/rst/block.rs:5780`), docfield bodies without the translatable inline.
  - Math: `run_sphinx_math` `src/rst/block.rs:4408-4449` emits the **post-PropagateTargets**
    shape directly (`<target refid="equation-X">` + `math_block ids="equation-X"`). Gaps:
    `:name:` is folded into the label but never `add_name`d (Sphinx: `ids="e2 equation-e2"
    names="e2"`), an empty `:label:` (or `math_number_all`) is not auto-labelled
    (Sphinx: `label="a:0"`, `ids="equation-a-0"`, number = next serial), and no
    `note_equation` registry (no `duplicate label of equation` warning, no `:eq:` resolution).
  - AutoNumbering approximation for `literalinclude` captions
    (`literalinclude_container`, `src/rst/block.rs:4234-4283`, allocates the auto id **at parse
    time**, i.e. in document order rather than after the parse). `code-block :caption:`,
    `figure`, `table`/`list-table`/`csv-table` titles get nothing.
* **Merge** — `src/builder.rs:917-1115` (per document in sorted docname order): titles
  (`env_toctree::document_title`), metadata (`src/env/metadata.rs`), dependencies
  (`src/env/dependencies.rs`), included, `build_toc` + `note_toctree`, parse-warning replay,
  index domain, std (+py) domain; then the doctree is persisted
  (`store_doctree`, bincode, `DOCTREE_FORMAT_VERSION`). This is the analogue of Sphinx's
  850–880 block — **the read-transform pass has to run before it** (§11).
* **Resolve** — `src/builder.rs:1193-1245`: numbering, `check_consistency`, `xref_phase`
  (`:1294-1368`, over a clone of each doctree in sorted order; `relative_uri` hard-wired to
  `''` at `:1316` = the dummy builder's answer), genindex, py-modindex, env save.
* **Write** — placeholder: `read_one_file` renders `<html><body>{escaped source}</body></html>`
  (`src/builder.rs:874-880`).

---------------------------------------------------------------------------------------------

## 3. docutils reference transforms (read phase) — spec

All of these need the docutils *document bookkeeping* that the parser builds:
`document.ids` (id→node), `nameids`/`nametypes` (name→id / explicit?), `refnames`
(name→[referencing nodes]), `refids`, `indirect_targets`, `autofootnotes`, `autofootnote_refs`,
`symbol_footnotes`, `symbol_footnote_refs`, `footnotes`, `footnote_refs`, `citations`,
`citation_refs`, `substitution_defs`, `substitution_names`, `anonymous` refs/targets (found by
walk), `id_counter`, `autofootnote_start` (1), `symbol_footnote_start` (0). The crate exports
only `nameids` (`RegistryExport.nameids`, `src/rst/mod.rs:253-256`) and keeps the id counter
private to the parse registry (`src/doctree/ids.rs:197-305`). **All of these lists are in
document (parse) order and can be rebuilt by a pre-order walk**; the id registry (used ids +
counter) must be *continued*, not rebuilt from zero, because transforms call `set_id` for
messages and `problematic` nodes and AutoNumbering/Footnotes call it for nodes (probed id
sequence: AnonymousHyperlinks' msg `id8`, its problematics `id9`,`id10`; IndirectHyperlinks
`id11`…`id15`; Footnotes `id16`…`id19` — strictly in transform priority order).

`replace_self` semantics (`docutils/nodes.py:1110-1135`) matter everywhere below: when a node
is replaced by an Element (or a list whose first item is an Element), the replaced node's
`ids`, `names`, `classes`, `dupnames` are **appended** to the new first node
(`update_basic_atts` `:850-869`, `append_attr_list` skips duplicates). Probed example: a
"too many autonumbered footnote references" `problematic` ends up `ids="id17 id2"` (own id,
then the replaced footnote_reference's).

### 3.1 DefaultSubstitutions (210) + Substitutions (220)

* DefaultSubstitutions (`transforms/__init__.py:111-137`): for each `substitution_reference`
  whose `refname` ∈ {`version`,`release`,`today`,`translation progress`} **and** not defined in
  this document (`document.substitution_defs`), replace by `Text(config.version)` /
  `Text(config.release)` / `Text(config.today or format_date(config.today_fmt or '%b %d, %Y',
  language))` / progress text. `format_date` (`util/i18n.py:263-…`) honours
  `SOURCE_DATE_EPOCH`, else now (UTC), and formats through babel (`%b` → `MMM` etc.,
  `util/i18n.py:177-217`) — **the HTML oracle must set `today=` (or `SOURCE_DATE_EPOCH`)**.
  Probed: `|today|` → `TODAY` with `today='TODAY'`; `|version|`→`1.0`; `|release|`→`1.0.1`.
* Substitutions (`references.py:642-764`): for every `substitution_reference` (list snapshot
  plus nested ones appended while iterating): look up `refname` exactly, else case-folded via
  `substitution_names` (probed: `|Name|` resolves `|name|`). Missing →
  `reporter.error('Undefined substitution referenced: "%s".' % refname, base_node=ref)`
  (loose message, printed `index.rst:4: ERROR: Undefined substitution referenced: "undef".
  [docutils]`), `problematic(rawsource, rawsource, refid=msgid)` with its own id replaces the
  ref (probed tree: `<problematic ids="id2" refid="id1">|undef|`; HTML `<a href="#id1"><span
  class="problematic" id="id2">|undef|</span></a>` — the `#id1` target never exists because
  the message is loose). `ltrim`/`rtrim`/`trim` options strip adjacent Text. Otherwise replaced
  by a deepcopy of the definition's **children** (the `substitution_definition` node stays in
  the tree — it is `Invisible`, the HTML translator skips it). Referential nodes in the copy
  with `refname` are `note_refname`d (so a `|link|_`-style substitution reference gets
  resolved by the later reference transforms). Circular definitions: the definition is replaced
  by an ERROR `Circular substitution definition detected:` + `literal_block(rawsource)`
  (attached); references by `Circular substitution definition referenced: "%s".`.
  Line-length guard: `Substitution definition "%s" exceeds the line-length-limit.` (no base
  node → loose-message location quirk, §9.3).

### 3.2 MoveModuleTargets (210), ReorderConsecutiveTargetAndIndexNodes (220), PropagateTargets (260), SortIds (261)

* **MoveModuleTargets** (`transforms/__init__.py:153-175`): for every `target` with non-empty
  `ids` that has an `ismod` attribute, whose parent is exactly a `section`, and which sits at
  `parent.index(target) == 2` (title, index node, target): `section['ids'][0:0] = target['ids']`
  and remove the target. `py:module` emits `[index, target(ismod)]` (Sphinx
  `domains/python/__init__.py:532-534`; crate `src/rst/block.rs:5680-5698` — same order), so a
  `py:module` that is the first thing in a section is absorbed. Probed:
  `<section ids="module-mymod mod-section" names="mod\ section">` (index node kept, target
  gone), HTML `<section id="module-mymod"><span id="mod-section"></span><h2>…<a
  class="headerlink" href="#module-mymod"…>`. With `:no-index-entry:` there is no index node,
  the target is at index 1, and MoveModuleTargets does **not** apply (PropagateTargets does).
  **The env corpus's `py_basic/a` is this case** (`<section ids="module-zmod a" names="a">`),
  not a PropagateTargets case as the `PROPAGATE_MODULE_TARGETS` reason text in
  `tests/env_differential.rs:600-606` suggests.
* **ReorderConsecutiveTargetAndIndexNodes** (`:446-515`): for each target (document order),
  collect the consecutive run of `target`/`index` siblings starting at it
  (`findall(descend=False, siblings=True)`); if ≥2 and contiguous in one parent, stable-sort the
  run with key index→0, target→1. Must run before PropagateTargets. Probed:
  `.. _c:` + `.. index:: single: foo` + para → `<index><target refid="c"><target
  refid="index-0"><paragraph ids="index-0 c" names="c">`; `.. index:: bar` + `.. _d:` + para
  → `<index><target refid="index-1"><target refid="d"><paragraph ids="d index-1" names="d">`.
* **PropagateTargets** (`references.py:17-95`): for every `target` (document order) whose
  parent is not a `TextElement` and that has no `refid`/`refuri`/`refname`:
  `next_node = target.next_node(ascend=True)`, skipping `system_message`s with
  `next_node(ascend=True, descend=False)`; skip if `None` or (`Invisible` or `Targetable`)
  and not `target`. Then `next_node['ids'].extend(target['ids'])`,
  `next_node['names'].extend(target['names'])`, update `document.ids`; if the target's parent
  is a `figure` and next is a `caption`, the target is **removed**; otherwise
  `target['refid'] = target['ids'][0]`, ids/names cleared, `note_refid`. Probed shapes:
  `.. _a:` `.. _b:` para → `<target refid="a"><target refid="b"><paragraph ids="b a" names="b
  a">` (chained: a→target b, then b→para); `.. _e:` + note → `<note ids="e" names="e">`;
  HTML `<p id="b"><span id="a"></span>…`.
  - `Invisible` = comment, substitution_definition, pending, target, **`addnodes.index`**,
    system_message? (no — system_message is skipped separately), `raw` is NOT invisible.
    `Targetable` = target, footnote, citation. The crate's read-only replay
    (`next_propagation_target`, `src/env/std_domain.rs:674-690`) blocks
    `comment|substitution_definition|pending|footnote|citation|TEXT` — **missing `index`**
    (harmless today only because Reorder isn't applied either; once Reorder runs the case
    disappears, but a target followed by an index node *not* in a contiguous run — e.g. in a
    different parent — would diverge). The in-tree implementation should use a proper
    `is_invisible`/`is_targetable` predicate (table in Appendix A).
* **SortIds** (`:217-225`): for every `section` with `len(ids) > 1` and
  `ids[0].startswith('id')`: `ids = ids[1:] + [ids[0]]`. Runs after PropagateTargets, so it
  sees label ids appended after the section's own id. **Probed quirk:** `.. _lbl:` +
  section "Identity" → `ids="lbl identity"`, `<section id="lbl"><span id="identity">`,
  headerlink `#lbl`, toc anchor `#lbl` (TocTreeCollector reads `ids[0]` at 880); a
  non-ASCII title (`日本`, auto id `id1`) + `.. _lbl2:` → `ids="lbl2 id1"`.
  (`names` are not reordered: `names="identity lbl"`.)

### 3.3 AnonymousHyperlinks (440), IndirectHyperlinks (460), ExternalTargets (640), InternalTargets (660), SphinxDanglingReferences (850)

Probed on one paragraph (`smartquotes=False`):

```
Named `ext`_, anonymous `anon`__, indirect `ind`_, internal `sec`_, `inline <https://x.org>`_,
alias `al <ext_>`_, unknown `nope`_, dup `d`_.
.. _ext: https://example.com
__ https://anon.example
.. _ind: ext_
.. _d: https://1
.. _d: https://2
```
→ `<reference name="ext" refuri="https://example.com">`, `<reference anonymous="1"
name="anon" refuri="https://anon.example">`, `<reference name="ind"
refuri="https://example.com">` (indirect target also rewritten to
`<target ids="ind" names="ind" refuri="https://example.com">`, `refname` dropped),
`<reference name="sec" refid="sec">`, alias `al` → `refuri="https://example.com"`,
`nope` → `<problematic ids="id4" refid="id3">`, `d` → `<problematic ids="id6" refid="id5">`.
Warnings, in order: `index.rst:10: WARNING: Duplicate explicit target name: "d". [docutils]`
(parse time), `index.rst:4: ERROR: Unknown target name: "nope". [docutils]`,
`index.rst:4: ERROR: Duplicate target name, cannot be used as a unique reference: "d".
[docutils]` (both from SphinxDanglingReferences at 850, reference walk order). Note:
**`name` attributes stay on references; `refname` is deleted when resolved.**

* AnonymousHyperlinks (`references.py:98-160`): pair anonymous references and anonymous
  targets in document order; count mismatch → one loose ERROR `Anonymous hyperlink mismatch: %s
  references but %s targets.\nSee "backrefs" attribute for IDs.` (no base node) and every
  anonymous reference becomes a `problematic` (probed multi-line warning, located at the
  *end-of-parse* position, §9.3). Otherwise each ref gets the target's `refuri`, or — following
  a propagated target (no ids) through `document.ids[target['refid']]` — `refid = ids[0]`.
* IndirectHyperlinks (`:163-338`): resolve every `document.indirect_targets` entry
  (targets with `refname`, e.g. `.. _ind: ext_`) recursively (`multiply_indirect` marks cycles),
  migrate `refuri` or `refid` back, then rewrite references to the indirect target's names/ids.
  Errors (loose, `base_node=target`): `Indirect hyperlink target %s refers to target "%s", %s.`
  with `%s` = `"ind" (id="ind")`, explanation ∈ {`which does not exist`, `which is a
  duplicate, and cannot be used as a unique reference`, `forming a circular reference`}; every
  reference to it becomes `problematic` (probed: `index.rst:11: ERROR: Indirect hyperlink target
  "ind" (id="ind") refers to target "missing", which does not exist. [docutils]`; circular case
  also turns the *other target itself* into a `problematic ids="id14 c2" names="c2"` —
  exotic, pin with an oracle case rather than reason about it).
* ExternalTargets (`:340-373`): for each target with `refuri`, each of its names: every
  unresolved reference in `refnames[name]` loses `refname`, gains `refuri`.
* InternalTargets (`:376-411`): for each target with neither `refuri` nor `refid`: references
  to its names get `refid = nameids[name]` (only if the name maps to an id — a duplicate name
  maps to `None` and the ref is left for DanglingReferences).
* SphinxDanglingReferences (`transforms/references.py:18-30`): docutils `DanglingReferences`
  with `reporter.report_level` raised to ≥ WARNING during `apply` (suppresses the INFO
  "Hyperlink target … is not referenced." noise). The visitor (`references.py:922-990`)
  handles `reference`, `footnote_reference`, `citation_reference` that are unresolved and still
  carry `refname`: if `nameids` has it → `refid`; else ERROR `Duplicate target name, cannot be
  used as a unique reference: "%s".` (name present but id None) or `Unknown target name:
  "%s".` — the latter with a hint paragraph child when the name contains `<`/`>`
  (`Did you want to embed a URI or alias?` + `\nOpening bracket missing.` /
  `\nThe embedded reference must be preceded by whitespace.` / `\nClosing bracket missing.` /
  `\nThe embedded reference must be the last text before the end string.` /
  `\nWhitespace around the embedded reference is not allowed.`). The `problematic` takes the
  reference's own first id if it has one (`prbid = node['ids'][0]`), else a new id.
  Messages are **loose** (never in the tree): with `keep_warnings=True` they still don't
  render; the `problematic` link dangles.

### 3.4 Footnotes (620) + citations (619) + FootnoteDocnameUpdater (700) + UnreferencedFootnotesDetector (622)

* CitationDefinitionTransform (619, `domains/citation.py:133-148`): for each `citation`:
  `node['docname'] = docname`, `domain.note_citation(node)` (`:70-82`: duplicate label →
  `logger.warning('duplicate citation %s, other instance in %s', label,
  env.doc2path(other_docname), location=node, type='ref', subtype='citation')` — **absolute
  path** of the earlier doc; then the newer registration **overwrites**), `label
  ['support_smartquotes'] = False`. Registry: `citations[label] = (docname, ids[0], line)`.
* CitationReferenceTransform (619, `:150-177`): each `citation_reference` →
  `pending_xref(target, refdomain='citation', reftype='ref', reftarget=target, refwarn=True,
  support_smartquotes=False, ids=node['ids'], classes=node.get('classes', []))` containing
  `inline(target, '[%s]' % target)`; `note_citation_reference` records the using docname.
  Resolution at write time (`resolve_xref` `:99-113`): `make_refnode(builder, fromdoc,
  docname, labelid, contnode)` — same doc → `refid`, else `refuri=<rel>#<labelid>`; the
  pending_xref's `ids` are transferred to the result by `replace_self` (probed:
  `<reference ids="id1" internal="1" refuri="a.html#cit"><inline>[CIT]`; unresolved →
  fallback `<inline ids="id2">[Missing]` + `WARNING: citation not found: Missing [ref.ref]`
  (dangling_warnings `'ref': 'citation not found: %(target)s'`).
  Consistency (`check_consistency` `:88-97`): `Citation [%s] is not referenced.`
  `location=(docname, lineno)` type ref.citation — emitted after the read phase (probed
  `a.rst:6: WARNING: Citation [Unref] is not referenced. [ref.citation]`).
* Footnotes (620, `references.py:416-635`):
  1. `number_footnotes`: each auto footnote (`[#]`, `[#label]`) in document order takes the
     next integer label **not already a name in `nameids`** (probed: manual `[1]`, `[2]`
     present → first auto footnote is `3`); inserts `label` as first child; labelled ones fix
     up their `footnote_refs` (text, `refid`, backref); unlabelled get `names=[label]` and
     `note_explicit_target`.
  2. `number_footnote_references`: `[#]_` refs take the unlabelled auto labels in order;
     overflow → ERROR `Too many autonumbered footnote references: only %d corresponding
     footnote%s available.` (`s` only if n>1), remaining refs → `problematic`.
  3. `symbolize_footnotes`: `[*]` footnotes get `* † ‡ § ¶ # ♠ ♥ ♦ ♣` repeated
     (`**` on the 11th …), `set_id`; refs paired in order; overflow → `Too many symbol
     footnote references: only %s corresponding footnotes available.`.
  4. `resolve_footnotes_and_citations`: manual footnotes/citations link to their refs by name
     (`refid`, backrefs). (In Sphinx citation refs are already `pending_xref` at this point.)
  Probed tree: `<footnote_reference auto="1" docname="index" ids="id1" refid="id6">3` …
  `<footnote auto="1" backrefs="id2 id5" docname="index" ids="lab" names="lab"><label>4`.
* FootnoteDocnameUpdater (700): `docname` on every `footnote` and `footnote_reference`
  (pformat-only).
* UnreferencedFootnotesDetector (622, `transforms/__init__.py:288-324`): Sphinx *logger*
  warnings, `type='ref', subtype='footnote'`, `location=node`:
  `Footnote [%s] is not referenced.` (manual, `names[0]`), `Footnote [*] is not referenced.`,
  `Footnote [#] is not referenced.` (auto — note an unlabelled auto footnote got a numeric
  name in step 1, so it *does* warn). Probed `index.rst:10: WARNING: Footnote [2] is not
  referenced. [ref.footnote]`, `index.rst:11: WARNING: Footnote [#] is not referenced.
  [ref.footnote]`.

### 3.5 DocInfo (340) and MetadataCollector

* DocInfo (`frontmatter.py:266-548`): if the first non-`PreBibliographic` child of the
  **document** is a `field_list`, it is replaced by a `docinfo` node (+ optional
  `dedication`/`abstract` topics inserted after `Titular`/decoration/meta). Bibliographic field
  names (case-insensitive, `fully_normalize_name`): author, authors, organization, address,
  contact, version, revision, status, date, copyright → dedicated nodes; dedication/abstract →
  topics; anything else (`orphan`, `tocdepth`, `nocomments`, custom) → the `field` itself with
  `classes=[make_id(name)]`. Warnings appended into the field body: `Cannot extract empty
  bibliographic field "%s".`, `Bibliographic field "%s"\nmust contain a single <paragraph>, not
  %s.`, `There can only be one "%s" field.`, the long `Cannot extract "%s" from bibliographic
  field:` text. RCS keyword cleanup (`$Date: …$` etc.).
* MetadataCollector.process_doc (`collectors/metadata.py:35-68`, at 880): reads the `docinfo`
  (authors → `md['authors'] = [...]` list; field → `md[field_name.astext()] =
  field_body.astext()`; other TextElement → `md[node.__class__.__name__] = astext()`),
  coerces `tocdepth` with `int()` (ValueError → 0), then **`doctree.pop(index)` — the docinfo
  is removed from the tree** (probed: `:author: Me` `:orphan:` `:tocdepth: 1` `:custom: value`
  + title → resolved tree has only the section; `metadata = {'author': 'Me', 'orphan': '',
  'tocdepth': 1, 'custom': 'value'}`). Dedication/abstract topics are *not* popped.
* **Important correction to `src/env/metadata.rs:21-33`:** that "Known gap — a field list
  below a promoted document title is not seen" is **not a gap**: Sphinx sets
  `doctitle_xform=False`, so a field list after the title stays in the section and is *not*
  metadata (probed `docinfo_after_title`: tree keeps the `field_list`, HTML renders `<dl
  class="field-list simple">`, `metadata={}`, `:orphan:` ignored). Current crate behaviour is
  already correct there; the only work is the DocInfo node + pop + biblio key mapping.
* Closes `KNOWN_RESOLVED_GAPS` `("orphan_doc", "orphan", …)`
  (`tests/env_differential.rs:647-652`).

### 3.6 Transitions (830)

`misc.py:64-143`: for each `transition`: parent not document/section → `Transition must be
child of <document> or <section>.`; first child or after title/subtitle/meta/decoration →
`Document or section may not begin with a transition.`; after another transition → `At least one
body element must separate transitions; adjacent transitions are not allowed.` (warning
inserted after the transition only if the parent's content model allows a body element); if
the transition is the **last** child of a section it is **moved up** after the nearest
ancestor that is not last; at the very end of the document it stays and `Document may not end
with a transition.` is appended (probed: `index.rst:15: WARNING: Document may not end with a
transition. [docutils]`, tree keeps `<transition>` as last child of the nested section, HTML
`<hr class="docutils" />`). Warnings are **attached** (then stripped by FilterSystemMessages
unless `keep_warnings`).

---------------------------------------------------------------------------------------------

## 4. Other Sphinx read transforms — spec

* **AutoNumbering** (210, `transforms/__init__.py:200-214`): for every Element (document
  order) with `domain.is_enumerable_node(node)` (class ∈ {`figure`, `table`, `container`} plus
  extension-registered; `domains/std/__init__.py:799-803,1363-1364`) and
  `get_numfig_title(node) is not None` (first `caption` or `title` child's `clean_astext` —
  an empty caption gives `''`, still not None; `:1366-1378`) and `node['ids'] == []` →
  `document.note_implicit_target(node)` (auto id `idN`, no name). Runs **before**
  PropagateTargets: probed `.. _f:` + figure → `<figure ids="id1 f" names="f">`, HTML
  `<figure class="align-default" id="id1"><span id="f"></span>…<a class="headerlink"
  href="#id1" title="Link to this image">`. Unlabelled code-block/figure/table/list-table →
  `ids="id1".."id4"`. Consequence for numbering: `toc_fignumbers` is keyed by `ids[0]`
  (`collectors/toctree.py:329-345`, `figure_id = fignode['ids'][0]` at `:334`), so a target-labelled figure is numbered under `id1`, not
  `f` — the crate's `PropagatedIds::effective_ids` (`src/env/std_domain.rs:620-627`) yields
  `['f']` today. Any generic `.. container::` with a title/caption child also qualifies
  (class match on `container`).
* **HandleCodeBlocks** (210, `:178-197`): `block_quote` whose children are *all*
  `doctest_block` → `replace_self(children)`. Probed: an indented `>>> quoted` block becomes a
  top-level `<doctest_block classes="doctest">`.
* **DoctestTransform** (500, `:327-334`): `doctest_block['classes'].append('doctest')`. HTML:
  `<div class="doctest highlight-default notranslate">`.
* **FilterSystemMessages** (999, `:337-347`): `filterlevel = 2 if keep_warnings else 5`;
  every `system_message` with `level < filterlevel` is removed (logged at DEBUG). **Default
  (`keep_warnings=False`) removes all of them** — HTML never shows system messages unless
  `keep_warnings=True`, and then only WARNING+ (INFO removed). Loose messages are never in the
  tree anyway. Crate: never strips (the reason for `keep_warnings` in `KNOWN_INERT_CONF`,
  `tests/env_differential.rs:889-898`).
* **SphinxContentsFilter** (`:350-358`): the TOC-title filter (`pending_xref` unwrapped,
  `image` skipped) — ✓ ported as `filter_title_children` (`src/env/toctree.rs:620-650`).
* **PreserveTranslatableMessages** (10): toctree `rawentries` = list of explicit entry titles
  (`addnodes.py:58-100`), `rawcaption` = caption; pformat-visible (`rawentries="Ext\ title
  Custom"`, `rawcaption="Main "Cap""` in the probed local toc), not HTML-visible.
* **AutoIndexUpgrader**, **ExtraTranslatableNodes**, **UIDTransform**, **Locale**,
  **AddTranslationClasses**, **RefOnlyBulletList** (with default `html_compact_lists=True`),
  **StripComments**, **Decorations**, **ExposeInternals**, **Validate**, **DocTitle**,
  **SectionSubTitle** — no-ops for a default HTML build (see §1.2 table).

---------------------------------------------------------------------------------------------

## 5. SmartQuotes — full spec, crate status, oracle decision

### 5.1 When it runs and whether it is on

* `SphinxSmartQuotes` (750, `transforms/__init__.py:361-415`) replaces docutils' own
  `SmartQuotes` (855), which `RSTParser.get_transforms` removes (`parsers.py:66-74`).
* `is_available()` (`:382-401`) — all must hold:
  1. `document.settings.smart_quotes` is not False — `env._update_settings` does
     `settings.setdefault('smart_quotes', True)` (`environment/__init__.py:381-382`);
  2. `config.smartquotes` (default **True**, `config.py:289`);
  3. builder name ∉ `smartquotes_excludes['builders']` (default `['man', 'text']`);
  4. `config.language` ∉ `smartquotes_excludes['languages']` (default
     `['ja', 'zh_CN', 'zh_TW']`, exact string match);
  5. some tag of `normalize_language_tag(settings.language_code)` (`= config.language`, default
     `'en'`; `language=None` in conf.py becomes `'en'`, `config.py:573-581`) is a key of
     `smartchars.quotes`.
* `smartquotes_action` (default `'qDe'`, `config.py:290`) replaces the class attribute.
* It runs **before** SphinxDomains (850) and the collectors (880), so educated text flows into
  `env.titles`/`longtitles` (→ `<title>`, relbar, `:doc:` link text), `env.tocs` (→ local TOC,
  toctrees, sidebar), std label `sectname` (→ `:ref:` text; probed `('index', 'tgt', 'Sub
  “section”')`), glossary term nodes, captions, metadata values. Section `ids`/`names`, index
  entry attributes, and target names are computed at parse time from raw text and are **not**
  affected (probed `names=""quoted"\ title's\ --\ here"`, index entry `'"term"'`).
* Not educated because they are attributes turned into Text at *write* time: toctree
  `:caption:` and explicit entry titles `Title <doc>` (probed `"Cap" -- x` and `"Explicit" --
  t` stay straight in the HTML with smartquotes on).
* HTML `render_partial` runs docutils' own `SmartQuotes` via `_PARSER_TRANSFORMS`
  (`builders/html/__init__.py:94,415-418`) but the builder's `_settings` have docutils'
  default `smart_quotes=False` (`:155-160`), so it is a no-op (probed: with `smartquotes=False`
  the `<title>` stays `&#34;Quoted&#34; Title&#39;s -- here` — note Jinja's `&#34;`/`&#39;`
  escaping there vs the body's `&quot;`).

### 5.2 Algorithm (docutils `universal.SmartQuotes.apply`, `universal.py:280-340`, with Sphinx's `get_tokens`)

For each `TextElement` in document order (`findall(nodes.TextElement)`):
* skip if it is a `FixedTextElement` or `Special` (`nodes_to_skip`, `:248`) — literal_block,
  doctest_block, math_block, comment, raw, target, index, substitution_definition,
  desc_name/addname/annotation/parameterlist/…, address, manpage, production;
* skip if its **parent** is a `TextElement` (nested inline elements belong to the enclosing
  "block" unit);
* `txtnodes` = all descendant `Text` nodes except those whose parent is `option_string`;
* `lang = node.get_language_code(document_language)` — first class `language-xx` on the node or
  an ancestor (`docutils/nodes.py:773-786`), else `config.language`; `normalize_language_tag`
  (`docutils/utils/__init__.py:741-765`: `'de_AT-1901'` → `['de-at-1901','de-at','de-1901',
  'de']`); the first tag present in `smartchars.quotes` wins, else a loose reporter WARNING
  `No smart quotes defined for language "%s".` (once per lang per document) and ASCII quotes;
* tokens (Sphinx override `:403-415`): for each Text, `is_smartquotable(txtnode)`
  (`util/nodes.py:697-716`) walks **all** ancestors: if any is `FixedTextElement`, `literal`,
  `math`, `image`, `raw`, `problematic`, or `not_smartquotable` subclass (desc_signature and
  all `desc_sig_*`, `desc_name`, `desc_addname`, `desc_inline`, `literal_emphasis`,
  `literal_strong`), or has attribute `support_smartquotes` is False (py `default_value`
  inline, citation `label`, citation `pending_xref`) → token `('literal', astext())`; else
  `('plain', re.sub(r'(?<=\x00)([-\\\'".`])', r'\\\1', str(txtnode)))` — i.e. the
  **null-escaped** text with a backslash inserted after each `\x00` before an active char;
* `educate_tokens(tokens, attr=smartquotes_action, language=lang)` (`smartquotes.py:565-675`),
  and every Text is replaced by `Text(newtext)` (newtext keeps the `\x00` markers; astext()
  drops them).

`educate_tokens` per plain token (literal tokens only update the context char):
`last_char = text[-1:]`; `processEscapes` (`\\ \" \' \. \- \`` → `&#92; &#34; &#39; &#46;
&#45; &#96;`, `:850-880`); dashes (`'D'` = old school: `---`→`—` U+2014 then `--`→`–` U+2013,
`:781-791`; `'d'` = `--`→em, `---`→en; `'i'` inverted); ellipses (`...` and `. . .` → `…`
U+2026, `:813-825`); backticks if `b`/`B`; quotes: `educateQuotes(context + text, lang)[1:]`
with `context = prev_last_char` with `"`/`'` replaced by `;` (`:652-655`); restore escapes;
`prev_token_last_char = last_char`. Initial context is `' '`.

`educateQuotes` (`:678-735`) with `smart = smartchars(lang)` (open/close primary/secondary;
`'en'` = `“ ” ‘ ’`, `'de'` = `„ “ ‚ ‘`, `'fr'` = `('« ', ' »', '“', '”')` with U+00A0,
`'ja'` = `「」『』`, full table `smartquotes.py:413-497`, apostrophe always `’`):
START_SINGLE/START_DOUBLE (`^'`/`^"` followed by punct at non-word-break → closing),
ADJACENT_1/2 (`"'`/`'"` before a word char), OPEN_SINGLE/OPEN_DOUBLE after `[([{]` or a dash
followed by optional punct and a space → closing, DECADE (`'` before `\d{2}s`, English only →
apostrophe), OPENING_SECONDARY (after whitespace/ZWSP/ZWNJ, bracket or dash, before word/punct),
APOSTROPHE `(?<=(\w|\d))'(?=\w)` (only if csquote ≠ apostrophe), CLOSING_SECONDARY `(?<!\s)'`,
remaining `'` → opening secondary, OPENING_PRIMARY, CLOSING_PRIMARY, remaining `"` → opening
primary. Regexes verbatim at `smartquotes.py:508-555`. The `\w`/`\s`/`\d` classes are Python
`re` Unicode classes.

### 5.3 Probed outputs (default conf, `language='en'`)

```
"Quoted" Title's -- here                → “Quoted” Title’s – here
He said "hello" and 'bye'. It's the '80s -- or 1990--2000 --- maybe... ok. . . done.
  → He said “hello” and ‘bye’. It’s the ’80s – or 1990–2000 — maybe… ok… done.
Escaped \"quote\" and \-- and \... and \'x\'.   → Escaped "quote" and -- and ... and 'x'.
'Start' and "*emph*" and "``lit``" end. ``code``'s apostrophe.
  → ‘Start’ and “<em>emph</em>” and “<code>lit</code>” end. <code>code</code>’s apostrophe.
He said "she said 'hi'" -- ok. x--y and 5'10" and rock 'n' roll. "Hello," she said. 'Twas.
  → He said “she said ‘hi’” – ok. x–y and 5’10” and rock ‘n’ roll. “Hello,” she said. ‘Twas.
See https://example.com/a--b/it's...    → link text https://example.com/a–b/it’s… (refuri unchanged!)
.. rst-class:: language-de  "Deutsch" und 'einfach'.  → „Deutsch“ und ‚einfach‘.  (<p lang="de">)
```
Unchanged: inline literal, `:code:`, `:option:` literal, `:samp:`, `:kbd:`, math, literal
blocks, py signatures incl. `default_value` (`'x'`), citation labels, substitution
*definitions* (Special). Educated: `:menuselection:` (`“A” ‣ “B”`), `:guilabel:`, `:ref:`
explicit titles, desc_content, glossary terms and definitions, field names and bodies, line
blocks, rubric, bullet items, admonition bodies, figure/code-block captions, versionmodified
text, URL display text. `language='de'` → `„Top“ – Title`; `'fr'` → `« Top » – Title` (U+00A0
inside the guillemets; secondary `“hi”`); `'ja'` → **nothing** educated (excluded language —
even a `language-de` paragraph stays ASCII); `smartquotes_action='qe'` → quotes/ellipses but
`--` stays.

### 5.4 Crate status and prerequisite

* Not implemented anywhere (`grep -ri smartquote src/` only finds the
  `support_smartquotes="0"` stamp in `src/py/arglist.rs:608-612`, which is already correct).
* **Null-escape information is lost at parse.** docutils `Text` stores the *null-escaped*
  string (`str(txt)`) and unescapes only in `astext()`/`pformat()`
  (`docutils/nodes.py:405-465`); the crate's inliner calls `unescape(text, false)` before
  creating every Text node (`src/rst/inline.rs:415-470`, `unescape` at `:42-60`). SmartQuotes
  needs to know which `-\'".`` ` characters were backslash-escaped. Options:
  (a) store the escaped form (`\u{0}`-prefixed) in `Node.text` for inliner-produced Text and
  make `astext()`/`pformat()`/HTML text output unescape — docutils-faithful but touches every
  reader of `node.text` (and must not leak `\0` into ids/names/attributes, which docutils
  computes from unescaped text); (b) add an optional side-channel on Text nodes (e.g.
  `escaped: Option<Vec<u32>>`, char indices of characters that followed a backslash, after
  `unescape`'s `\x00 `/`\x00\n` removal) populated only when non-empty — minimal blast
  radius. Either way bump `DOCTREE_FORMAT_VERSION` (and the env version if anything cached
  changes). Recommendation: (b).
* Needs a node-class table (Appendix A) for TextElement / FixedTextElement / Special /
  not_smartquotable membership; the crate's generic `Node` has only `kind`.
* Config plumbing: `smartquotes`, `smartquotes_action`, `smartquotes_excludes`, `language`
  (exists in `src/python_config.rs:46,334` but not consumed) into the read-transform pass.

### 5.5 Oracle decision (recommendation)

* Today: `tools/gen_env_fixture.py:139` `BASE_CONFOVERRIDES = {"smartquotes": False}` (every
  env project), `tools/gen_sphinx_fixture.py:40-44` pins `smartquotes=False` and "never
  overridden per-case", and `tests/env_differential.rs:903` lists `smartquotes` as inert with
  an "every value is inert" soundness arm (`:918-920`).
* The HTML oracle must NOT silently inherit `smartquotes=False` everywhere: Sphinx's default
  is True, so an oracle that never runs it would bless output nobody gets. Recommended split:
  1. **Structural families** (toctree, targets, footnotes, desc, tables, …) keep
     `smartquotes=False` so writer failures are not masked by typography and so their doctrees
     stay comparable to the env/doctree fixtures.
  2. **A dedicated `smartquotes_*` family at default conf** (smartquotes on) covering §5.3's
     matrix: escapes, every literal-exclusion class, titles → `<title>`/toc/relbar/`:ref:`/
     `:doc:` texts, glossary, captions, field lists, URL text, `language` ∈ {en, de, fr, ja},
     `smartquotes_action` variants, `smartquotes_excludes` override, `language-xx` classes.
  3. When the transform lands: remove `smartquotes` from `KNOWN_INERT_CONF`, map it in
     `config_of` (`tests/env_differential.rs:417-451`) as a real `-D smartquotes=0`, and add
     at least one env project with `conf: {"smartquotes": True}` (the generator's
     `{**BASE_CONFOVERRIDES, **entry.conf}` lets a project override the base) so the
     educated `tocs`/`titles`/std `sectname`s are pinned at the env layer too.
* Gate it: the crate's `smartquotes` default must be True (Sphinx's), so a build without
  conf.py educates.

---------------------------------------------------------------------------------------------

## 6. Environment collectors (doctree-read, 880)

| collector | upstream | crate | gap / work |
|---|---|---|---|
| Dependencies | `collectors/dependencies.py:37-50` (docutils `record_dependencies`, cwd-relative → srcdir-relative) | `src/env/dependencies.rs` (include records + image walk) | ✓ (images need candidates, below) |
| Image | `collectors/asset.py:48-134` | **missing** (`src/env/dependencies.rs:24-35` lists the omissions) | see below; closes `IMAGE_CANDIDATES` rows + 3 `KNOWN_WARNING_GAPS` |
| DownloadFile | `asset.py:152-174` | **missing** (no `:download:` role in the crate at all — `src/rst/inline.rs` has no `download` arm) | role + collector: `download_reference` gets `refuri` for `://` targets, else `filename = dlfiles.add_file(docname, rel)` = `<md5(posix rel path)>/<basename>`; `download file not readable: %s` (absolute filename, type download.not_readable) |
| Metadata | `metadata.py:35-68` | `src/env/metadata.rs` (reads field list, keeps it) | DocInfo + pop + biblio keys + `tocdepth` int (§3.5) |
| Title | `title.py:38-59` | `src/env/toctree.rs:944` `document_title` | ✓; longtitle from `doctree['title']` only via `.. title::` (not supported); must read the *post-SmartQuotes* tree |
| TocTree | `collectors/toctree.py:64-192` | `src/env/toctree.rs:424-615` `build_toc` | ✓ algorithm; must run on the *post-transform* tree (anchors use `ids[0]` after PropagateTargets/SortIds/MoveModuleTargets; titles after SmartQuotes) |

**ImageCollector.process_doc** (probed, `p_img.py`) — for every `image` (document order,
**including images inside `substitution_definition`s and their copies**):
* `candidates = {}` stored on `node['candidates']` (pformat `candidates="{'*': 'pic.png'}"` =
  Python dict repr, insertion order);
* `data:` uri or uri containing `://` → `{'?': uri}` and continue (no warning, no
  dependency);
* uri ending in `.*` → `uri = relfn2path(uri, docname)[0]` (srcdir-relative), glob the
  language-specific name then the plain one, `mimetype = guess_mimetype(file)` or
  `'image/x-' + ext`, keep the shortest path per mimetype (probed `pic.*` →
  `{'image/svg+xml': 'pic.svg', 'image/png': 'pic.png'}`); `sub/*.png` is **not** a glob (only
  a `.*` suffix triggers it) → literal path, warns;
* else `imguri = search_image_for_language(uri)` (`figure_language_filename` default
  `'{root}.{language}{ext}'`, used only if that file exists — with `language='en'` a
  `pic.en.png` beside `pic.png` wins), `node['uri'] = relfn2path(imguri, docname)[0]`,
  `candidates['*'] = uri`, and `node['original_uri'] = <authored uri>` if it changed (probed in
  `doc/x.rst`: `../pic.png` → `uri="pic.png" original_uri="../pic.png"`; `/pic.png` →
  `uri="pic.png" original_uri="/pic.png"`; `pic.png` → `uri="doc/pic.png"
  original_uri="pic.png"`);
* for each candidate path: `note_dependency`; if not `os.access(srcdir/path, R_OK)` →
  `logger.warning('image file not readable: %s', path, location=node, type='image',
  subtype='not_readable')` (srcdir-relative path; line = the directive line: probed image 6,
  image-in-list 10, figure 12, substitution image 18 — **printed twice** for a substitution
  image used once), else `env.images.add_file(docname, path)` (`util/_files.py:14-61`
  `FilenameUniqDict`: unique basename, collisions → `stem + str(i) + suffix`, i from 1).
* HTML later (`post_process_images`, `builders/__init__.py:213-…`): `'?'` untouched;
  `'*'` used; else first of `supported_image_types` (`['image/svg+xml', 'image/png',
  'image/gif', 'image/jpeg']`) present, else `a suitable image for %s builder not found: %s
  (%s)` warning; `node['uri']` rewritten to the candidate, and translated to
  `_images/<unique name>` only if the file is in `builder.images` (unreadable images keep their
  raw uri: probed `<img alt="missing1.png" src="missing1.png" />`).

---------------------------------------------------------------------------------------------

## 7. Post-transforms (write time) — spec and crate gaps

### 7.1 ReferencesResolver (10)

Crate port is thorough for std/py/any/intersphinx (`src/env/resolve.rs:891-1136`, pending
condition handling `XrefChildren` `:1460-1545`). Remaining gaps:

1. **`math` domain** — `refdomain` outside `{"", "std", "py"}` is short-circuited into the
   "cross-domain reference(s) not validated" count (`src/env/resolve.rs:1018-1028`), so
   `:eq:` never resolves (and `docs/IMPLEMENTATION_STATUS.md`'s list of such domains — c/cpp/
   js/rst — omits `math` and `citation`). Spec (`domains/math.py:69-127`,
   `directives/patches.py:137-200`): registry `objects[label] = (docname, serial+1)` filled
   **at parse** by the math directive (`note_equation`; later registration of the same label
   **overwrites** and warns `duplicate label of equation %s, other instance in %s` with the
   *docname*, `type='ref', subtype='equation'`, location = the math_block); resolution:
   `node_id = make_id('equation-' + target)`; `eqno = str(number)` unless `math_numfig and
   numfig`, then `'.'.join(toc_fignumbers[docname]['displaymath'].get(node_id, ()))` with the
   last `.` replaced by `math_numsep`; title `Text((math_eqref_format or '({number})').format(
   number=eqno))` (bad format → `Invalid math_eqref_format: %r` and `(%d)`);
   `make_refnode(...)`. Dangling: `equation not found: %(target)s` (`warn_dangling=True`,
   `[ref.eq]`). Probed: `a.rst` resolves its own `:eq:`e1`` to `b.html#equation-e1` because
   `b` redefined `e1` later in read order; with `numfig=True` `:eq:`e2`` (defined via
   `:name: e2`, `ids="e2 equation-e2"`) renders `()` because `toc_fignumbers` is keyed by
   `ids[0]` = `e2` — faithful bug. `:numref:`e1`` → `undefined label: 'e1' [ref.numref]`.
2. **`citation` domain** — see §3.4; needs the read-side transforms first.
3. **`replace_self` attribute transfer** — `node.replace_self(new_nodes)` moves the
   `pending_xref`'s `ids/names/classes/dupnames` onto the first replacement node
   (`docutils/nodes.py:1110-1135`); the crate builds replacement vectors without it
   (`resolve_children` `src/env/resolve.rs:850-889`). Visible for citation references
   (`<reference ids="id1" …>`, `<inline ids="id2">[Missing]`) and any role that puts classes
   on a pending_xref.
4. Traversal order: Sphinx walks `findall(pending_xref)` pre-order; the crate resolves
   children before parents (`resolve_children` recurses first). Only differs for nested
   pending_xrefs (warning order) — note, don't chase.

### 7.2 OnlyNodeTransform (50) + Tags

* `process_only_nodes` (`util/nodes.py:719-729`): for every `only`: keep → `replace_self(
  node.children or nodes.comment())`, drop → `replace_self(nodes.comment())` (an empty
  `<comment xml:space="preserve">` — kept so ids have somewhere to go; the HTML translator
  skips comments, so ids moved onto a dropped `only`'s comment vanish from the HTML).
  Because `replace_self` transfers attributes, a target propagated onto an `only` node lands on
  its first child (probed: `.. _lbl:` + `.. only:: <bad expr>` → `<paragraph ids="lbl"
  names="lbl">`).
* **Runs after ReferencesResolver**: references inside a dropped `only` are resolved and **do
  warn** (probed `index.rst:6: WARNING: undefined label: 'nope-in-latex' [ref.ref]` in an HTML
  build). Toctrees inside a dropped `only` are gone before toctree resolution, but they were
  still noted at read time (`toctree_includes`, relations: probed `a` stays in the
  prev/next chain).
* Evaluation `_only_node_keep_children` (`:732-742`): `tags.eval_condition(node['expr'])`; any
  exception → `logger.warning('exception while evaluating only directive expression: %s',
  err, location=node)` and **keep**. Builder tags (`builders/__init__.py:120-124`): `html`,
  `html` (name), `format_html`, `builder_html` (+ `-t` tags, `tags.add` in conf.py).
* `Tags.eval_condition` (`util/tags.py:15-102`): jinja2 parser restricted to names, `and`,
  `or`, `not`, parentheses and `x if c else y`; `true/True/false/False/none/None` parse as
  Const but then `_eval_node` raises. Probed exact error strings (the warning text is
  `str(err)`, including jinja's `\n  line 1` suffix):
  `'html and'` → `unexpected token 'end of template'\n  line 1`;
  `'html && latex'` → `unexpected char '&' at 5\n  line 1`;
  `'(html'` → `unexpected end of template, expected ')'.\n  line 1`;
  `'True'` → `invalid node, check parsing`;
  `'html latex'`, `'html,'`, `'html-x'`, `'html == latex'` → `chunk after expression`;
  `'1'` → `unexpected token 'integer'\n  line 1`; `"'html'"` → `unexpected token 'string'\n
  line 1`. Results are cached per condition (exceptions are not).
* `only` nodes also appear inside `env.tocs` (collector wraps them, `collectors/toctree.py:
  105-110`) and are evaluated again by `_toctree_copy_seq` (§8.4) — a bad expression can warn
  again there.
* Crate: `run_only` emits `only[expr]` + children (`src/rst/block.rs:5937-5943`); no
  evaluator exists anywhere (`grep eval_condition src/` → nothing). Port a tiny tokenizer +
  recursive-descent parser for the grammar above with jinja-compatible messages for the common
  errors (the list above is what the oracle should pin).

### 7.3 HighlightLanguageTransform (400) and TrimDoctestFlagsTransform (401)

* `HighlightLanguageVisitor` (`code.py:50-86`): a stack initialised with
  `(config.highlight_language, False, sys.maxsize)` at `document` (and `start_of_file`,
  singlehtml only); `highlightlang` sets the top to `(lang, force, linenothreshold)`; each
  `literal_block` without `language` gets `language`, `force`; without `linenos` gets
  `linenos = astext().count('\n') >= threshold - 1`. Then all `highlightlang` nodes are
  removed. Applies to `::` blocks, parsed-literal and literalinclude/code-block blocks that
  lack the attrs (doctest_block is not a literal_block). Probed (after `.. highlight:: python
  :linenothreshold: 3`): a 2-line block `linenos="0"`, a 3-line block `linenos="1"`;
  `::` before any highlight → `force="0" language="default" linenos="0"`; `.. highlight:: c
  :force:` → `force="1" language="c"`; with `highlight_language='rst'` the first block is
  `language="rst"`. pformat renders the Python bools as `"0"`/`"1"`.
* Parse-time sibling: `CodeBlock.run` uses `self.env.current_document.highlight_language or
  self.config.highlight_language` when no argument (`directives/code.py:158-166`); the crate
  hard-codes `"default"` instead of `config.highlight_language`
  (`src/rst/block.rs:5831-5837`) — fix together.
* TrimDoctestFlagsTransform (`code.py:89-132`): for literal_blocks with
  `rawsource == astext()` (i.e. not parsed-literal) and `language` in `{pycon, pycon3}`, or in
  `{py, python, py3, python3, default}` with rawsource starting `>>>`, or `guess` + pygments
  guessing PythonConsoleLexer; and for every `doctest_block`: if `node.get('trim_flags',
  config.trim_doctest_flags)` (default True): `source = blankline_re.sub('', rawsource)`
  (`^\s*<BLANKLINE>` multiline), `doctestopt_re.sub('', …)` (`[ \t]*#\s*doctest:.+$`
  multiline) (`ext/doctest.py:41-42`), `node.rawsource = source`, children =
  `[Text(source)]`. Probed: `>>> f()  # doctest: +SKIP\n<BLANKLINE>` → `>>> f()`
  (HTML one line); with `trim_doctest_flags=False` untouched. **Crate gap:** `Node` has no
  `rawsource`; parsed-literal detection must be recorded at parse (e.g. a marker for
  parsed-literal blocks, or compare against a stored raw text).
* Closes `KNOWN_HIGHLIGHT_STAMP_GAPS` (6 docs, `tests/env_differential.rs:746-753`) and the
  `numfig_on/a` `linenos` note; after it lands delete `drop_unstamped_highlight_attrs`
  (`:773-805`).

### 7.4 PropagateDescDomain (200) — done; SigElementFallback (200), ImageDownloader (100), DataURIExtractor (150) — no-ops for html

(Only the dummy-builder env oracle sees DataURIExtractor act on `data:` images; the corpus has
none.)

---------------------------------------------------------------------------------------------

## 8. Toctree resolution (write time) — `sphinx/environment/adapters/toctree.py`

### 8.1 Entry points

* `get_and_resolve_doctree` (`environment/__init__.py:700-715`): after post-transforms, for
  each `toctree` node (`findall`, document order): `_resolve_toctree(env, docname, builder,
  node, prune=True, includehidden=False, tags)`; `None` → the toctree node is removed (its
  `compound.toctree-wrapper` stays, empty: probed HTML `<div class="toctree-wrapper
  compound">\n</div>` for a `:hidden:` toctree), else `replace_self(result)`.
* `document_toc(env, docname, tags)` (`:50-67`) — the page's local TOC (`toc` context,
  `builders/html/__init__.py:625-626`): `_toctree_copy(env.tocs[docname], 2,
  metadata.get('tocdepth', 0), False, tags)` then every reference's `refuri = anchorname or
  '#'`. Missing doc → empty `paragraph`. It keeps copied `toctree` nodes (the translator skips
  them: `visit_toctree` raises SkipNode, `visit_bullet_list` skips a single-toctree list,
  `writers/html5.py:468-472,805-808`). `display_toc = toc_num_entries[docname] > 1`.
* `global_toctree_for_doc(env, docname, builder, tags, collapse, includehidden=True,
  maxdepth=0, titles_only=False)` (`:70-116`) — the `toctree()` template function
  (`builders/html/__init__.py:1022-1032,1121`, which defaults `collapse=True`,
  `includehidden=False` and drops `maxdepth=''`): resolves **every toctree node of the root
  doc's pickled (unresolved, pre-post-transform) doctree** — so toctrees inside `.. only::`
  in the root document are included regardless of tags — with `prune=True`, and concatenates
  the children of all non-None results into the first result.

### 8.2 `_resolve_toctree` (`:119-220`)

1. `hidden` and not `includehidden` → `None`.
2. `toctree_ancestors = _get_toctree_ancestors(env.toctree_includes, docname)` (`:562-575`:
   child→parent map where the **last** parent in (sorted) `toctree_includes` order wins; walk
   up from `docname`, excluding the root) — crate ✓ `toctree_ancestors`
   (`src/env/toctree.rs:792-812`).
3. `included = Matcher(include_patterns)`, `excluded = Matcher(exclude_patterns)`.
4. `maxdepth = maxdepth or toctree.get('maxdepth', -1)`; `titlesonly` / `includehidden` from
   the node OR the call.
5. `tocentries = _entries_from_toctree(..., toctree, parents=[])`; empty → `None`.
6. `newnode = compact_paragraph('', '')`; caption (`toctree.attributes.get('caption')`; the
   crate's "True" None-sentinel must count as absent) → `title(caption, '', Text(caption))`
   with `rawsource = rawcaption`, line/source of the toctree; append entries;
   `newnode['toctree'] = True`.
7. `_toctree_add_classes(newnode, 1, docname)` (below).
8. `newnode = _toctree_copy(newnode, 1, maxdepth if prune else 0, collapse, tags)`.
9. If `newnode[-1]` is an Element with no children → `None` ("no titles found").
10. For every `reference` whose `refuri` does not match `url_re` (`(?P<schema>.+)://.*`,
    `util/__init__.py:18`): `refuri = builder.get_relative_uri(docname, refuri) +
    anchorname`. (Dummy builder: always `''` — which is why the env fixture shows
    `refuri=""`/`refuri="#sub"`.)

### 8.3 `_entries_from_toctree` (`:223-306`) and `_toctree_entry` (`:309-383`)

For each `(title, ref)` in `toctree['entries']`:
* `url_re.match(ref)` → `_toctree_url_entry`: `bullet_list > list_item > compact_paragraph >
  reference(internal=False, refuri=ref, anchorname='', Text(title or ref))`;
* `ref == 'self'` → `_toctree_self_entry`: `reference(internal=True, refuri=toctree['parent'],
  anchorname='', Text(title or clean_astext(env.titles[parent])))`;
* `ref in StandardDomain._virtual_doc_names` → `_toctree_generated_entry`:
  `reference('', title or sectionname, internal=True, refuri=docname, anchorname='')` with
  `genindex→('genindex', 'Index')`, `modindex→('py-modindex', 'Module Index')`,
  `search→('search', 'Search Page')` (translated per `language`). The crate's
  `VIRTUAL_DOC_NAMES` (`src/env/toctree.rs:54`) carries names only — add the mapping;
* else: if `ref in parents` → `logger.warning('circular toctree references detected, ignoring:
  %s <- %s', ref, ' <- '.join(parents), location=ref, type='toc', subtype='circular')` and
  skip the entry (location is the **docname** → printed `<project>/b.rst: WARNING: circular
  toctree references detected, ignoring: b <- a <- b [toc.circular]`, no line);
  otherwise `_toctree_standard_entry(title, ref, env.metadata[ref].get('tocdepth', 0),
  env.tocs[ref], ancestors, prune, collapse, tags)`:
  - `ref in ancestors and (not prune or maxdepth <= 0)` → `toc.deepcopy()` (no
    only-evaluation, no pruning) else `_toctree_copy(toc, 2, maxdepth, collapse, tags)` —
    `maxdepth` here is the *target document's* `:tocdepth:`, not the toctree's;
  - explicit title replaces the text of references with `refuri == ref` and no anchorname,
    **only if the copied toc has exactly one top-level child** (a doc with two top-level
    sections ignores the explicit title);
* empty toc → `toctree contains reference to document %r that doesn't have a title: no link
  will be generated` (`location=toctreenode`, `toc.no_title`; probed `index.rst:4`);
* `KeyError` (no `env.metadata`/`env.tocs` entry) → `excluded(doc2path(ref, False))` →
  `toctree contains reference to excluded document %r` / `not included(...)` → `…non-included
  document %r` / else `…non-existing document %r` (`toc.excluded` / `toc.not_included` /
  `toc.not_readable`), and the entry is skipped.
* `titles_only`: for each top-level child with >1 children: if it contains toctree nodes,
  replace `top_level[1][:]` with them; else `pop(1)`.
* sub-toctrees: for each `toctree` in the entry's toc (list snapshot), skip hidden unless
  `includehidden`; otherwise resolve recursively with `parents=[refdoc, *parents]`,
  `subtree=True`, inserting the resulting list items after the toctree node in its parent and
  removing the toctree node. A circular sub-toctree leaves an **empty `<bullet_list>`** behind
  (fixture `toctree_circular` resolved pformat).
* top level (`subtree=False`) returns `[bullet_list(*entries)]`.

### 8.4 `_toctree_add_classes` (`:455-482`) and `_toctree_copy` (`:485-559`)

* add_classes(node, depth, docname): for children: `compact_paragraph`/`list_item` →
  `classes += ['toctree-l%d' % (depth-1)]` and recurse with the same depth; `bullet_list` →
  recurse with `depth+1`; `reference` with `refuri == docname` (pre-URI-rewrite docname!): if
  no anchorname, add `current` to the reference and **every ancestor** up to the root
  (`while branchnode: … = branchnode.parent`); then if `subnode.parent.parent` already
  `iscurrent` → `return` (exits the whole call for this node's remaining children), else set
  `iscurrent = True` on the reference and every ancestor. Root call `(newnode, 1)` →
  top-level items are `toctree-l1`. Probed (page `sub/b` global toc):
  `<list_item classes="toctree-l1 current" iscurrent="1">` / `<compact_paragraph
  classes="toctree-l1 current" iscurrent="1">` / `<reference anchorname="" classes="current"
  internal="1" iscurrent="1" refuri="" secnumber="2">`; the in-page anchor entry `#b1` gets
  `iscurrent` but not `current`; a sibling compact_paragraph of an ancestor list_item does
  **not** get `current` (only ancestors do). Attribute rendering: `iscurrent="1"`,
  `toctree="1"`, `secnumber="2 1"` (tuple, space-joined), `secnumber="True"` for `None`.
* copy(node, depth, maxdepth, collapse, tags): `depth = max(depth-1, 1)`, then
  `_toctree_copy_seq(..., initial_call=True)`:
  - `compact_paragraph`/`list_item` → shallow copy + children with `is_current =
    'iscurrent' in node`;
  - `bullet_list` → keep if `depth <= 1 or ((depth <= maxdepth or maxdepth <= 0) and (not
    collapse or is_current or 'iscurrent' in node))`; dropped unless kept or initial call;
    children copied at `depth+1`;
  - `toctree` → `node.copy()` (kept for later resolution);
  - `only` → children spliced in if `_only_node_keep_children` (may warn), else dropped;
  - `reference`/`title` → copy with deep-copied children;
  - anything else → `ValueError('Unexpected node type …')` (can't happen for collector
    output).
* Probed: `collapse=True` on page `sub/c` keeps only the current branch's sub-lists;
  `maxdepth=1` drops all sub-lists; `includehidden=True, titles_only=True` appends the hidden
  toctree's entries as a second `bullet_list`; `.. only:: latex` sections vanish from the
  copy but keep their secnumbers (`toc_secnumbers['sub/c']` has `#c3-latex: (2, 1, 3)`).

### 8.5 Output example (probed, `html_theme='basic'`, index.rst with `:caption: Main "Cap"
:maxdepth: 2 :numbered:` over `a, sub/b, self, https://example.com, Ext title
<https://example.org>, genindex, Custom <a>` + a `:hidden:` toctree)

```
<compound classes="toctree-wrapper">
    <compact_paragraph classes="current" iscurrent="1" toctree="1">
        <title>
            Main "Cap"
        <bullet_list classes="current" iscurrent="1">
            <list_item classes="toctree-l1">
                <compact_paragraph classes="toctree-l1">
                    <reference anchorname="" internal="1" refuri="a.html" secnumber="1">
                        A
                <bullet_list>
                    <list_item classes="toctree-l2">
                        <compact_paragraph classes="toctree-l2">
                            <reference anchorname="#a1" internal="1" refuri="a.html#a1" secnumber="1 1">
            ...
            <list_item classes="toctree-l1 current" iscurrent="1">       ← the `self` entry
                <compact_paragraph classes="toctree-l1 current" iscurrent="1">
                    <reference anchorname="" classes="current" internal="1" iscurrent="1" refuri="">
                        Index
            <list_item classes="toctree-l1"> … <reference anchorname="" internal="0" refuri="https://example.com">
            <list_item classes="toctree-l1"> … <reference anchorname="" internal="1" refuri="genindex.html"> Index
<compound classes="toctree-wrapper">                                   ← hidden toctree: emptied
```
HTML: `<li class="toctree-l1 current"><a class="current reference internal" href="#">Index</a>`
(`refuri ""` renders `href="#"`), `<a class="reference internal" href="a.html">1. A</a>`
(secnumber rendered by the writer). Cross-directory: from `sub/b`, `../a.html`, `c.html`,
`../index.html`. Warning in the same build: none from resolution; read-time
`duplicated entry found in toctree: a [toc.duplicate_entry]` and `a is already assigned
section numbers (nested numbered toctree?) [toc.secnum]` (`Custom <a>` repeats `a`).

### 8.6 Warning multiplicity and the circular crash

* Every call path into `_toctree_entry` re-emits its warnings: `get_and_resolve_doctree` once
  per document containing the toctree, plus `global_toctree_for_doc` **once per rendered page**
  for toctrees in the root doc when the theme's sidebar calls `toctree()`. Probed:
  `toc.no_title` ×6 under alabaster (5 pages incl. genindex/search + 1 resolve), ×1 under
  `basic` (its default sidebars `localtoc, relations, sourcelink, searchbox` don't call
  `toctree()`). The oracle should use `basic` and the crate must reproduce per-render
  re-emission when it renders `globaltoc.html`.
* `toctree_circular` (a→b→a): dummy builds produce 3 warnings (fixture:
  `b.rst: … b <- a <- b`, `a.rst: … a <- b <- a` ×2); **HTML builds crash** in
  `prepare_writing` → `collect_relations` → `_traverse_toctree` (`environment/__init__.py:
  914-940`, no visited-set) with `RecursionError` (probed, both themes). The crate must not
  crash; emit the circular warnings from resolution and record the no-crash behaviour as a
  documented better-than-Sphinx divergence (the env fixture already records `relations: null`
  for this project, `tools/gen_env_fixture.py:1672-1694`).

---------------------------------------------------------------------------------------------

## 9. Diagnostics produced by transforms (reporter vs logger)

### 9.1 Channels

* **Reporter** messages (docutils `reporter.warning/error/severe`): printed through Sphinx's
  `WarningStream` (`util/docutils.py:385-393`) as `<source>:<line>: <LEVEL>: <text>
  [docutils]` where LEVEL is `WARNING` (2), `ERROR` (3), `CRITICAL` (4 = SEVERE,
  `util/logging.py:30-41`); multi-line texts keep their newlines, the ` [docutils]` suffix
  goes after the last line (fixture `inc_basic`). Printed only if `level >= report_level`
  (2); INFO/DEBUG never print. `-W` counts them.
* **Logger** warnings (`logger.warning(..., type=, subtype=)`): `<location>: WARNING: <text>
  [type.subtype]`.

| transform | channel | attached to tree? | text(s) |
|---|---|---|---|
| Substitutions | reporter ERROR | no (loose) except circular-definition replacement | §3.1 |
| AnonymousHyperlinks | reporter ERROR | no; **no base_node** (§9.3) | `Anonymous hyperlink mismatch: %s references but %s targets.\nSee "backrefs" attribute for IDs.` |
| IndirectHyperlinks | reporter ERROR | no | §3.3 |
| Footnotes | reporter ERROR | no | §3.4 |
| UnreferencedFootnotesDetector | logger `ref.footnote` | — | §3.4 |
| CitationDefinitionTransform | logger `ref.citation` | — | `duplicate citation %s, other instance in %s` |
| SphinxSmartQuotes | reporter WARNING | no | `No smart quotes defined for language "%s".` |
| Transitions | reporter WARNING | **yes** (stripped by FilterSystemMessages unless keep_warnings) | §3.6 |
| SphinxDanglingReferences | reporter ERROR | no | §3.3 |
| DocInfo | reporter WARNING | yes, inside field bodies (popped with the docinfo) | §3.5 |
| ImageCollector | logger `image.not_readable` | — | `image file not readable: %s` |
| DownloadFileCollector | logger `download.not_readable` | — | `download file not readable: %s` |
| OnlyNodeTransform / toctree copy | logger (no type) | — | `exception while evaluating only directive expression: %s` |
| toctree resolution | logger `toc.circular`/`toc.no_title`/`toc.excluded`/`toc.not_included`/`toc.not_readable` | — | §8.3 |
| math domain | logger `ref.equation` (read), `ref.eq` (resolve) | — | §7.1 |
| citation domain | logger `ref.citation` (consistency), `ref.ref` (resolve) | — | §3.4 |

### 9.2 Per-document ordering

Read phase, for each document in sorted docname order: parse-time messages (reporter and
logger interleaved in source order) → transform messages in transform-priority order (so e.g.
Substitutions errors, then AnonymousHyperlinks, IndirectHyperlinks, citations (619),
Footnotes (620), UnreferencedFootnotes (622), SmartQuotes (750), Transitions (830),
DanglingReferences (850, reference walk order), SphinxDomains (850: index/std duplicate
labels…), then collectors at 880 (image/download not readable). Then after all reads:
`check_consistency` (orphans, `_check_toc_parents` info, `Citation [...] is not
referenced.`). Write phase, per document in sorted order: ReferencesResolver warnings (document
order), OnlyNodeTransform evaluation warnings, toctree-resolution warnings, then page-render
warnings (sidebar toctree). The crate's merge phase currently replays parse toctree warnings,
then parse log warnings, then index, then std (`src/builder.rs:1039-1082`) — transform
diagnostics must be recorded by the transform pass and replayed **between** the parse warnings
and the domain hooks; image warnings **after** the std domain hook.

### 9.3 Loose messages without `base_node` — location quirk (open item)

A reporter message raised in a transform *without* `base_node` takes its location from the
reporter's `get_source_and_line()`, still bound to the finished state machine. Probed with the
anonymous-mismatch error: `'T\n=\n\nA `x`__.\n'` → `index.rst:5` (one past the last line);
with three trailing blank lines → `index.rst:8`; without a trailing newline → `index.rst:5`;
when the document ends inside a directive's nested content (`.. note:: x\n\n   y\n`) →
`index.rst:: ERROR: …` (empty line number → the location string ends with `:`, Sphinx
prints `path::`). Affects AnonymousHyperlinks mismatch and the Substitutions line-length
error only. Recommend: reproduce the common "number of lines + 1" case, pin it and the
nested-content case with oracle cases, and treat any remaining mismatch as a ledgered
divergence.

---------------------------------------------------------------------------------------------

## 10. Gap list — each gap, where, and the work to close it

(K = closes a `tests/env_differential.rs` exemption; H = HTML-visible; W = warning-stream)

| # | Gap | Crate site | Work | Closes |
|---|---|---|---|---|
| T1 | Read-transform pass does not exist | `src/rst/mod.rs:386-396`, `src/builder.rs:829-900` | New module (e.g. `src/transforms/`) invoked in sphinx mode right after parse, before `store_doctree`/merge; carries the parse id registry forward (extend the parser to hand back `ids` set + `id_counter` + nametypes, or run the pass inside `parse_document_full` before the registry is dropped); rebuilds the docutils lists by pre-order walk; returns recorded diagnostics (level, text, source, line, children) for ordered replay. Keep `parse_rst` transform-free (the 735-case docutils fixture is pre-transform). Bump `DOCTREE_FORMAT_VERSION`. | enabler for all below |
| T2 | Substitutions + DefaultSubstitutions | none | §3.1; config `version`/`release`/`today`/`today_fmt`/`language` | H, W |
| T3 | Reorder + PropagateTargets + SortIds + MoveModuleTargets in-tree | replay in `src/env/std_domain.rs:485-700`, numbering in `src/env/numbers.rs:353-357` | §3.2; then the replays become identity (keep or delete, but `DocumentIds` must still see post-propagation ids); fix blocked-kinds predicate (add `index`) | K: `PROPAGATE_TARGETS` (labels_dups/a,b; index_entries/a; py_any/b), `PROPAGATE_MODULE_TARGETS` (py_basic/a, py_dup/b, py_toc/mod, py_toc_parents/mod, py_modindex/index, py_modindex_prefix/index); H; env toc anchors (`#lbl` for "Identity"-type sections) |
| T4 | Anonymous/Indirect/External/Internal hyperlinks + SphinxDanglingReferences | none | §3.3 | H, W |
| T5 | Footnotes + FootnoteDocnameUpdater + UnreferencedFootnotesDetector | none | §3.4 | H, W |
| T6 | Citation domain (both transforms, registry, consistency, resolve) | `src/env/resolve.rs:1018-1028` short-circuits non-std/py | §3.4, §7.1 | H, W |
| T7 | DocInfo + metadata pop + biblio keys + `tocdepth` int | `src/env/metadata.rs` | §3.5 (and correct its "known gap" note) | K: `("orphan_doc","orphan")`; H |
| T8 | Transitions | none | §3.6 | W (H only via relocation) |
| T9 | AutoNumbering | approximation at `src/rst/block.rs:4234-4283` | real pass at 210 after parse; delete the parse-time stamp (ids then allocate after all parse-time auto ids, like Sphinx) | H (figure/table/code-block ids), env `toc_fignumbers` keys |
| T10 | HandleCodeBlocks + DoctestTransform | none | §4 | H (`doctest` class, unwrapped blockquote) |
| T11 | SmartQuotes | none; Text unescaped at `src/rst/inline.rs:415-470` | §5 (escape side-channel, class table, config) | K: `KNOWN_INERT_CONF["smartquotes"]`; H everywhere |
| T12 | FilterSystemMessages | none | strip `level < (2 if keep_warnings else 5)` at end of the read pass | K: `KNOWN_INERT_CONF["keep_warnings"]` becomes a real key |
| T13 | ImageCollector | `src/env/dependencies.rs:24-35` omissions | §6 (`candidates`, `uri`/`original_uri` rewrite, globbing, i18n name, warning, `env.images` uniq dict); HTML side: `post_process_images` + `_images/` copying | K: `IMAGE_CANDIDATES` (toctree_numbered_depth2/a, numfig_off_numref/a, numfig_on/a, inc_deps/a) + `KNOWN_WARNING_GAPS` (toctree_numbered_depth2, numfig_on, numfig_off_numref) |
| T14 | `:download:` role + DownloadFileCollector | none | role (`download_reference`) + §6 | H, W |
| T15 | Math domain registry + `:eq:` + auto labels + `:name:` | `src/rst/block.rs:4408-4449`, `src/env/resolve.rs:1018` | §7.1 (`note_equation` in merge order, `math_number_all`, empty `:label:`, `add_name` for `:name:`, `math_numfig`, `math_eqref_format`, `math_numsep`) | H, W |
| T16 | OnlyNodeTransform + Tags evaluator | `run_only` `src/rst/block.rs:5937-5943` | §7.2; builder tags `html`, `format_html`, `builder_html` | H, W |
| T17 | HighlightLanguageTransform + TrimDoctestFlags | `highlightlang` emitted at `src/rst/block.rs:5918-5925`; code-block default at `:5831-5837` | §7.3 (+ config `highlight_language`, `trim_doctest_flags`; parsed-literal marker) | K: `KNOWN_HIGHLIGHT_STAMP_GAPS` (6 docs) + numfig_on/a linenos |
| T18 | `_resolve_toctree` family + `document_toc` + `global_toctree_for_doc` | `src/env/toctree.rs:30-35` ("NOT here") | §8, parameterised by a builder URI function (dummy `''` for the env oracle, html relative `.html`) and Tags | K: all 23 `TOCTREE_RESOLUTION` rows + `KNOWN_WARNING_GAPS["toctree_circular"]` |
| T19 | `replace_self` attribute transfer in ReferencesResolver | `src/env/resolve.rs:850-889` | append pending_xref ids/names/classes/dupnames to the first replacement Element | H (citations) |
| T20 | PreserveTranslatableMessages attrs | `src/rst/block.rs:6037` | `rawentries` = explicit titles list, `rawcaption` | pformat parity only |
| T21 | `highlight_language` config at parse | `src/rst/block.rs:5831-5837` | use config instead of literal `"default"` | H |

Exemption bookkeeping when these land: every table in `tests/env_differential.rs` is strict
(a listed document that stops diverging fails), and `exemption_arithmetic_matches_the_documented
_numbers` recomputes the counts quoted in `docs/IMPLEMENTATION_STATUS.md:40` and `ROADMAP.md:57`
("35/84 … 43 skipped … 6 stamp") — update both docs with the new figures in the same change.
The `KNOWN_RESOLVED_GAPS` reason text for `PROPAGATE_MODULE_TARGETS` should mention
MoveModuleTargets. The `docs/IMPLEMENTATION_STATUS.md:155-166` "Transforms not yet run" list is
incomplete: it names PropagateTargets, AutoNumbering, HighlightLanguageTransform but not the
docutils reference transforms, DocInfo, Transitions, SmartQuotes, FilterSystemMessages,
OnlyNodeTransform, the citation/math domains, or toctree resolution.

---------------------------------------------------------------------------------------------

## 11. Suggested architecture and order of work

1. **Transform context** (`TransformCtx`): the doctree, the continued id registry
   (`ids: HashSet`, counter per prefix, `nameids`/`nametypes`), document lists rebuilt by walk,
   config slice (smartquotes*, language, keep_warnings, version/release/today/today_fmt,
   highlight_language, trim_doctest_flags), docname, srcdir (images), and a diagnostics sink
   that records `{channel: reporter|logger, level, type/subtype, text, source, line}` in
   emission order.
2. Implement the read list in the exact probed order (§1.2), skipping the proven no-ops.
   Suggested landing order (each independently oracle-checkable): T12 FilterSystemMessages →
   T3 targets family → T4 hyperlinks → T2 substitutions → T5/T6 footnotes+citations → T7
   DocInfo → T8 Transitions → T9/T10 AutoNumbering/code blocks → T11 SmartQuotes.
3. Collectors (T13/T14) in the merge phase in Sphinx order, after the domain hooks.
4. Write side: a `Builder`-like trait with `name`, `format`, `get_target_uri`,
   `get_relative_uri`, `supported_image_types`, `supported_data_uri_images`,
   `supported_remote_images`, `tags`; the env oracle uses a Dummy impl (`''` URIs; and
   DataURIExtractor semantics), the HTML build uses an Html impl. Resolve per document:
   ReferencesResolver (T19) → OnlyNodeTransform (T16) → PropagateDescDomain →
   HighlightLanguage/TrimDoctest (T17) → toctree resolution (T18) → (html)
   `post_process_images` → translate.
5. Oracles:
   * Extend `tools/gen_sphinx_fixture.py` with a *post-transform* family (or a new fixture)
     for exactly the previously excluded constructs (targets/footnotes/substitutions/docinfo/
     transitions/doctest/anonymous & indirect links/citations), compared against the crate's
     parse+transform pipeline; the docutils fixture stays pre-transform.
   * The HTML oracle harness should capture, per document, both the resolved doctree pformat
     (patch `write_doc_serialized` for the pre-`post_process_images` tree and `write_doc` for
     the post one) and the HTML — a tree-level diff localises writer vs transform bugs.

---------------------------------------------------------------------------------------------

## 12. HTML-oracle configuration decisions (recommendations)

| knob | recommendation | why |
|---|---|---|
| `smartquotes` | base `False` for structural families + a default-conf SmartQuotes family | §5.5 |
| `html_theme` | `'basic'` | matches the wave-5 "basic-theme templates"; avoids per-page sidebar re-emission of toctree warnings (§8.6) |
| `keep_warnings` | default `False` for most; one project `True` (pins `system-message` rendering and level-2 filtering) | §4 FilterSystemMessages |
| `today` / `SOURCE_DATE_EPOCH` | set `today='…'` (or the env var) | `|today|` and templates' `last_updated` are otherwise nondeterministic |
| circular toctrees | exclude | Sphinx HTML crashes (§8.6) |
| `language` | `en` base; `de`/`fr`/`ja` only inside the SmartQuotes family | chrome strings (`Link to this heading` → `Lien vers cette rubrique`) are translated by `language` too |
| images | ship real files; include one missing image and one `pic.*` glob | ImageCollector + `post_process_images` |
| `numfig` | both off and on | `:eq:`/`numref`/caption numbers |

---------------------------------------------------------------------------------------------

## Appendix A — node-class membership needed by the transforms (probed from docutils 0.22.4 / sphinx 9.1.0)

`T`=TextElement, `F`=FixedTextElement, `S`=Special, `I`=Invisible, `Tg`=Targetable,
`NSQ`=not_smartquotable (class attr `support_smartquotes=False`).

* **T+F** (SmartQuotes skips as units; `is_smartquotable` false inside): `literal_block`,
  `doctest_block`, `math_block`, `address`, `comment` (+S+I), `raw` (+S), `desc_annotation`,
  `desc_parameterlist`, `desc_parameter`, `desc_optional`, `desc_returns`, `desc_type`,
  `desc_type_parameter`, `desc_type_parameter_list`, `desc_signature_line`, `manpage`,
  `production`, `desc_name` (+NSQ), `desc_addname` (+NSQ).
* **T+S (not F)**: `target` (+I+Tg), `substitution_definition` (+I), `index` (sphinx, +I).
* **S only**: `pending` (+I), `system_message`.
* **NSQ (T, not F)**: `desc_signature`, `desc_inline`, every `desc_sig_*`
  (`desc_sig_space/name/operator/punctuation/keyword/keyword_type/literal_number/
  literal_string/literal_char`), `literal_emphasis`, `literal_strong`.
* **Plain T** (SmartQuotes units when their parent is not T): `paragraph`,
  `compact_paragraph`, `title`, `subtitle`, `rubric`, `caption`, `label`, `term`,
  `classifier`, `field_name`, `attribution`, `line`, `option_argument`, `option_string`
  (its own Text excluded), `author`, `contact`, `organization`, `version`, `revision`,
  `status`, `date`, `copyright`, `centered`, `versionmodified`, `reference`,
  `download_reference`, `number_reference`, and the inline T's (`emphasis`, `strong`,
  `literal`, `inline`, `abbreviation`, `acronym`, `subscript`, `superscript`,
  `title_reference`, `problematic`, `math`, `generated`, `footnote_reference`,
  `citation_reference`, `substitution_reference`, `pending_xref_condition`).
* **Not T** (containers): `pending_xref` (Inline Element), `image`, `desc`, `desc_content`,
  `section`, `topic`, `sidebar`, admonitions, lists, tables, `figure`, `container`, `compound`,
  `definition`, `field_body`, `footnote`/`citation` (+Tg), `toctree`, `only`, `glossary`,
  `hlist`, `highlightlang`, `docinfo`, `transition`.
* **Invisible** (PropagateTargets won't donate into them): `comment`, `substitution_definition`,
  `pending`, `target`*, `index`. **Targetable**: `target`*, `footnote`, `citation`.
  (*`target` is exempted from the block.)
* `is_smartquotable` extra parents: `literal`, `math`, `image`, `raw`, `problematic`
  (`util/nodes.py:697-705`).

## Appendix B — probe index (scratchpad/probe-transforms/)

`list_transforms.py` (registry lists, listeners), `p_order.py` (applied order),
`p_smart.py`/`p_smart2.py`/`p_smart3.py` (SmartQuotes), `p_classes.py` (Appendix A),
`p_refs.py` (substitutions, hyperlinks, footnotes, transitions, docinfo, doctest, targets,
autonumber, sortids, modtargets, only), `p_sortids.py`, `p_circ.py` (circular crash, warning
multiplicity), `p_toctree.py` (resolution, local/global toc), `p_hl.py` (highlight/trim),
`p_math.py`, `p_cite.py`, `p_only.py`, `p_tags.py`, `p_img.py`, `p_err.py`, `p_loose.py`.
Harness: `harness.py` (`build(files, conf, builder, extra)` → warnings with the srcdir replaced
by `<src>`, resolved pformat captured in `write_doc`, all output HTML).
