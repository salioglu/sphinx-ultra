# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Implementation reality per subsystem lives in
[docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md); the plan to move
everything forward is [ROADMAP.md](ROADMAP.md).

## [Unreleased]

### Added

- **M2 wave 5, sub-project 1: docutils' diagnostics print, and Sphinx's read
  transforms run.** What a document's read phase prints and stores is now
  what `sphinx-build` 9.1.0's is, but for the divergences listed below.
  - **docutils diagnostics print.** Every message docutils' reporter raises
    at level 2 or above — a missing `include`, a directive error, an unknown
    target, an undefined substitution, malformed markup — is recorded when
    the parser creates it and printed in Sphinx's read order and format:
    `index.rst:14: CRITICAL: Problems with "include" directive path:` plus
    the reason and ` [docutils]` (`WARNING`, `ERROR`, or `CRITICAL` for
    docutils' SEVERE; a message without a line prints `index.rst::`). A
    document's records print in creation order, interleaved with the
    directives' and domains' own warnings; a multi-line message prints whole,
    on stderr and in the `-w` file alike. A document served from the cache
    prints nothing, as `sphinx-build` does not re-read it. This closes wave
    4.5's known limitation: a broken `include`/`literalinclude` path no
    longer drops its content silently.
  - **The read transforms run** on every document, in Sphinx's order, before
    it is stored: `|version|`, `|release|`, `|today|` and every other
    substitution, with docutils' circular, undefined and line-length errors;
    named, anonymous, indirect, external and internal hyperlinks, and
    `Unknown target name` errors for dangling ones; label targets moved onto
    the node they label, and a `py:module` target onto its section;
    auto-numbered, labelled, symbol and manual footnotes, with `Footnote [..]
    is not referenced.`; citations (`duplicate citation …`, `Citation [..]
    is not referenced.` — resolving a citation reference waits for
    sub-project 2); a leading field list (`:orphan:`, `:tocdepth:`,
    `:nocomments:`, bibliographic fields) read into the document's metadata
    and taken out of the tree; doctest blocks, auto-numbered figure, table
    and code-block ids, misplaced-transition warnings; and in-tree messages
    removed below `keep_warnings`' level.
  - **SmartQuotes, on by default as in Sphinx**: straight quotes become
    typographic ones, `--`/`---` dashes and `...` an ellipsis — in titles,
    paragraphs, label texts and the table of contents — per `language`,
    honouring `smartquotes_action`, `smartquotes_excludes` and
    `language-xx` classes; backslash-escaped characters stay plain and
    literals untouched; a language without quote data warns `No smart
    quotes defined for language "xx".` Set `smartquotes = False` to keep
    straight quotes.
  - **Deliberate divergences and known limitations** are listed in
    [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md): where
    `sphinx-build` crashes, hangs or fetches over the network (content
    nested past 200 levels, `include`'s `:parser:`, `raw`'s `:url:`, a
    `SOURCE_DATE_EPOCH` Python cannot read, a case-folded substitution
    cycle, …) this build prints a record and carries on. One gap remains in
    that guard: a chain of directives nested close to it (199 nested
    `.. py:function::`) can exhaust a read thread's stack and abort a
    release build — on a document `sphinx-build` already crashes on (from
    82 nested `py:function`s).
  Evidence: the read-phase doctree oracle at 697 cases (178 of them for the
  transforms, 20 for SmartQuotes), compared after the transforms and with
  each case's printed records; the docutils parse oracle at 761 cases with
  each case's message stream; the environment oracle at 36 projects / 96
  documents, warnings compared record by record — all at zero divergence
  against a real `sphinx-build` 9.1.0 — and a parse + transforms totality
  sweep green at `PROPTEST_CASES=2048`.
- New configuration keys for the read transforms, readable from `conf.py`,
  YAML/JSON and `-D`, at Sphinx 9.1's defaults: `smartquotes` (`True`),
  `smartquotes_action` (`'qDe'`), `smartquotes_excludes` (languages `ja`,
  `zh_CN`, `zh_TW`; builders `man`, `text`), `keep_warnings` (`False`),
  `today` (`''`), `today_fmt` (unset, meaning `'%b %d, %Y'`) and
  `highlight_language` (`'default'`; the language of a `code-block` that
  names none).

- **M2 wave 4.5: the Python domain, and files can include files.**
  `.. py:function::` and its thirteen siblings are no longer unknown
  directives, and `include`/`literalinclude` work — which also means a
  document is rebuilt when a file it *includes* changes.
  - **py directives**: `py:module`, `py:currentmodule`, `py:function`,
    `py:class`, `py:exception`, `py:method`, `py:classmethod`,
    `py:staticmethod`, `py:attribute`, `py:property`, `py:data`,
    `py:decorator`, `py:decoratormethod`, `py:type` — with real signature
    parsing. Annotations go through a port of CPython's `ast.unparse`;
    parameter defaults go through a port of `sphinx.pycode.ast.unparse`,
    which keeps a literal's source text — so `def f(x: int = 0x10)` still
    shows `0x10`, as it does under Sphinx; PEP 695 type-parameter lists,
    multi-line signatures, `:async:`/`:abstractmethod:`/`:final:` and the
    whole `:no-index:` option family are supported.
  - **doc fields**: `:param:`, `:type:`, `:raises:`, `:returns:`, `:rtype:`,
    `:var:` and their aliases render as Sphinx renders them — grouped
    Parameters/Raises lists, `:param int x:` type syntax, `:meta private:`
    filtering, and type cross-references. This pass runs for the *std*
    kinds too, so an `envvar`'s `:param x:` now renders as `Param x`.
  - **py cross-references**: `:py:func:`, `:py:class:`, `:py:meth:`,
    `:py:mod:`, `:py:attr:`, `:py:data:`, `:py:exc:`, `:py:obj:`,
    `:py:const:`, `:py:deco:` resolve against the registered objects, with
    Sphinx's search order, its `~`/`.`/`!` modifiers, and the builtin
    fallback (`:py:class:`int`` resolves). `:any:` resolves across domains.
  - **py-modindex data**: modules are grouped and sorted the way
    `PythonModuleIndex` groups them, honoring `modindex_common_prefix`. Like
    `genindex`, the page itself needs the HTML writer.
  - **`include`**: the whole docutils option set (`:literal:`, `:code:`,
    `:number-lines:`, `:encoding:`, `:tab-width:`,
    `:start-line:`/`:end-line:`/`:start-after:`/`:end-before:`,
    `:class:`/`:name:`), Sphinx's srcdir-relative `/path` rewrite, circular
    inclusion detection, and the docutils standard include files
    (`.. include:: <isonum.txt>`).
  - **`literalinclude`**: `:lines:`, `:start-after:`/`:end-before:`/
    `:start-at:`/`:end-at:`, `:pyobject:`, `:prepend:`/`:append:`,
    `:dedent:`, `:diff:`, `:emphasize-lines:`, `:linenos:`/`:lineno-match:`/
    `:lineno-start:`, `:tab-width:`, `:encoding:`, `:caption:`, `:name:`,
    `:class:`, `:language:`, `:force:`.
  - **incremental builds**: `include` and `literalinclude` record their
    member files as dependencies, so editing an included fragment rebuilds
    the documents that include it. Wave 4's dependency tracking is no
    longer images-only.
  - **glossary**: the three misformat diagnostics Sphinx raises
    (`glossary term must be preceded by empty line`, `glossary terms must
    not be separated by empty lines`, `glossary seems to be misformatted,
    check indentation`) are recorded in the doctree, and print since M2
    wave 5.
  - **`include`/`literalinclude` diagnostics** (a missing or unreadable
    file, the refused `:parser:`, a circular inclusion) are docutils
    *reporter* messages. Wave 4.5 recorded them in the doctree only — a
    broken include path dropped its content silently and `-W` stayed green
    — until M2 wave 5's reporter channel printed them (above).
  Evidence: the environment oracle grew to 29 projects / 84 documents and
  the read-phase doctree oracle to 489 cases, both at zero divergence
  against a real `sphinx-build` 9.1.0; `:pyobject:`'s tokenizer was checked
  against `sphinx.pycode`'s over 1200 real modules (24,903 definitions, no
  mismatches).
- New configuration knobs for object signatures, readable from `conf.py`,
  YAML/JSON and `-D` (all ten verified `-D`-settable):
  `maximum_signature_line_length`,
  `python_maximum_signature_line_length`,
  `python_trailing_comma_in_multi_line_signatures`,
  `python_display_short_literal_types`,
  `python_use_unqualified_type_names`, `toc_object_entries`,
  `toc_object_entries_show_parents`, `add_function_parentheses`,
  `add_module_names`, `strip_signature_backslash`. An out-of-range
  `toc_object_entries_show_parents` warns and is kept, like Sphinx.
- `source_encoding` is a real configuration key (`conf.py`, YAML/JSON and
  `-D`; default `utf-8-sig`, Sphinx's own) and is the default `:encoding:`
  of `include` and `literalinclude`. A non-UTF-8 value prints Sphinx's
  deprecation warning byte-for-byte (`Support for source encodings other
  than UTF-8 is deprecated and will be removed in Sphinx 10. …`); a codec
  this crate cannot decode earns one additional notice. The key governs the
  file-inserting directives **only**: this crate still decodes its own
  `.rst` sources as UTF-8, where Sphinx hands `source_encoding` to docutils
  as `settings.input_encoding` for the document read as well.
- `maximum_signature_line_length` and `python_maximum_signature_line_length`
  are type-checked the way Sphinx's `check_confval_types` checks them.
  `-D maximum_signature_line_length=20` — which Sphinx keeps as the
  *string* `'20'`, because a key whose default is `None` is never coerced —
  now warns ``The config value `maximum_signature_line_length' has type
  `str'; expected `NoneType' or `int'.`` (byte-exact, counts toward `-W`)
  and leaves the key unset, where earlier builds silently coerced it. A
  mistyped `conf.py` literal warns the same way with its Python type name.
  Sphinx itself goes on to crash on the first signature; this build does
  not.

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
  doctree — zero divergence on every compared key. Each corpus project is
  built **once, cold**; warm-equals-cold is asserted separately, by
  hand-written tests over their own small two- and three-document
  projects, not over the corpus. The exemptions that remain are listed in
  `tests/env_differential.rs` and summarized in
  [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md).
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

- **Breaking: the doctree and environment cache formats changed again (M2
  wave 5).** `DOCTREE_FORMAT_VERSION` went 2 → 3 (a stored doctree now
  means the post-transform tree) and `ENV_VERSION` 3 → 4 (the citation
  domain's registries; typed metadata values), and the builder's name
  joined the cache fingerprint. Old caches are an honest **miss**, so **the
  first build after upgrading is a full cold build**. No action is
  required.
- **docutils' diagnostics now print, and they are warnings (decision D2).**
  A docutils `WARNING`, `ERROR` or `CRITICAL` record exits 0 without `-W`
  and 1 with it, exactly as `sphinx-build` does; only this build's own read
  failures (an unreadable source) still exit 1 on their own. **This can
  turn a passing `-W` build into a failing one** for any project whose
  documents now earn read-phase records — docutils' (a broken include, an
  unknown target or substitution, a misplaced transition) or the read
  transforms' (an unreferenced footnote, a duplicate citation) — as it
  fails under `sphinx-build`. Build once without `-W` before upgrading a CI
  job that uses it.
- **The directive/role validators no longer report (decision D1).** With
  docutils' own messages printing, every validator check that repeated one
  (a missing argument or content block, an unknown option, a flag given a
  value, an invalid image width, height, scale or alignment, an empty
  `:doc:`/`:ref:`/`:download:` target) or fired on markup `sphinx-build`
  accepts (`Unusual image extension`, `Toctree directive is empty`, an
  empty or brace-unbalanced `math`, the house-style checks on `:doc:`,
  `:download:`, `:math:`, `:abbr:`, `:command:`, `:file:` and `:guilabel:`)
  is gone, and Sphinx's own message prints where Sphinx has one.
  `validate_directives` stays (default on) and reports nothing; anything
  that grepped for the old texts sees Sphinx's.
- **Text output changes with SmartQuotes on** (Sphinx's default): titles,
  paragraphs, label texts and the table of contents carry typographic
  quotes, dashes and ellipses where earlier builds kept the straight ones.
- `version` and `release` default to empty, Sphinx's `''`, instead of
  `1.0.0`.
- **Breaking: the doctree and environment cache formats both changed
  (M2 wave 4.5).** `DOCTREE_FORMAT_VERSION` went 1 → 2 and `ENV_VERSION`
  2 → 3, because both structures gained fields (per-line source provenance
  on doctrees; py-domain registries, the inclusion graph and file
  dependencies on the environment). Old blobs are an honest cache **miss**,
  not a mis-decode, so **the first build after upgrading is a full cold
  build**. No action is required; `-E` is not needed.
- **More warnings are new by default in this release (M2 wave 4.5).** The
  py domain now participates in cross-reference resolution, so a project
  with Python API docs gains diagnostics it never saw here before:
  `duplicate object description of …, other instance in …, use :no-index:
  for one of them`, `more than one target found for cross-reference …`,
  `more than one target found for 'any' cross-reference …`, and — under
  `-n`/`nitpicky` — dangling `:py:*:` references. `:any:` is now the
  domainless, `warn_dangling` role it is in Sphinx, so a broken `:any:`
  target warns `'any' reference target not found: … [ref.any]` **without
  `-n`** — that one reaches any project that uses `:any:`, whether or not
  it documents Python objects. The "skipping N python-domain references"
  notice lost its python-domain population (those references are now
  resolved and warned about); it survives, re-worded, as `N cross-domain
  reference(s) not validated (domain not implemented until M5)`, and still
  covers `:c:`, `:cpp:`, `:js:` and `:rst:` references, which stay
  unvalidated until those domains land.
  **This can turn a passing `-W` build into a failing one** for any project
  that documents Python objects or uses `:any:`. Build once without `-W`
  before upgrading a CI job that uses it.
- **Broken standard-domain references now warn without `-n`.**
  Sphinx sets `warn_dangling` on seven std reftypes — `:ref:`, `:numref:`,
  `:doc:`, `:term:`, `:keyword:`, `:option:` and `:confval:`
  (`domains/std/__init__.py:748-766`) — and that flag alone produces the
  warning, with no `-n` involved; `-n`/`nitpicky` only widens it to
  everything else. This release mirrors all seven, so `unknown document:
  '…'`, `undefined label: '…'`, `term not in glossary: '…'`,
  `unknown option: '…'` and their siblings now appear in a default build
  where previous releases reported them only under `-n`.
- **Several warnings are new by default in this release.** Besides the
  seven reftypes above, `duplicate label …, other instance in …`,
  `invalid <type> index entry …`, and the self-referencing and circular
  toctree warnings are all emitted now and were emitted under no flag at
  all in 0.4.x.

  **Together these can turn a passing `-W` build into a failing one**,
  and not only for projects with broken `:doc:`/`:ref:` targets: a project
  with a duplicate label, a malformed index entry, a circular toctree or a
  broken `:term:`/`:option:`/`:confval:`/`:numref:`/`:keyword:` reference
  will newly fail. Build once without `-W` before upgrading a CI job that
  uses it.
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

This release makes four source-breaking changes to the public library
surface. The binary's CLI is unaffected.

- The M1 domain system (`sphinx_ultra::domains`, and with it the crate-root
  re-exports `CrossReference`, `DomainObject`, `DomainRegistry`,
  `DomainValidator` and `ReferenceType`). It was a regex reference scanner
  with fuzzy suggestions; the std domain and the real resolution pass
  replaced its whole live surface, after which it had no call sites.
  Library consumers that imported those names have no drop-in replacement
  yet — the new API is `sphinx_ultra::env`. (`document::CrossReference` is
  a different type and still exists.)
- `sphinx_ultra::environment` is gone, and with it the public
  `BuildEnvironment::{new, add_document, doc2path, collect_relations,
  doc_needs_update, update_domain_object, get_all_objects}`, `Domain`,
  `ObjectType`, `DomainObject`, `DomainIndex`, `IndexEntry` and
  `create_standard_domains`. The module was never constructed by the
  binary; `sphinx_ultra::env` is the replacement, and it is a different
  design rather than a renamed one.
- **The crate-root `BuildEnvironment` re-export now names a different
  type.** `pub use environment::BuildEnvironment` became
  `pub use env::BuildEnvironment`, which shares no method name with the
  old type. `use sphinx_ultra::BuildEnvironment;` therefore keeps
  compiling while every call against it breaks — the same name-collision
  trap flagged for `CrossReference` above, and the one most likely to
  read as a mysterious error rather than a rename.
- `InventoryFile::dump`'s signature changed from
  `(filename, &BuildEnvironment, &HTMLBuilder)` to
  `(path, project, version, domains, get_target_uri)`, and the public
  field `Inventory.data` changed from `HashMap<..>` to `BTreeMap<..>`
  (the writer's output has to be deterministic).

### Internal

- Persisted doctrees now carry a magic + format-version header. bincode has
  no self-description, so a doctree written by an older build used to
  decode *successfully* into a plausible-but-wrong tree; a mismatched
  version is now an honest cache miss. Practical effect when upgrading:
  the first build after this change re-reads every document once, then
  caches normally.

### Fixed

- **Glossary terms are verbatim, definition-list terms are `rstrip()`ped,
  toctree entries are not trimmed (M2 wave 4.5, panel fix round F).**
  Sphinx's `split_term_classifiers` takes a glossary term and its first
  classifier exactly as written, so `term\xa0 : cls` keeps its NBSP in the
  `<term>`, the index entry (`'term\xa0'`) and the registered term, and
  `term : \xa0cls` keeps it in the index key; docutils' `Text.term` does the
  opposite for a plain definition list (`text = parts[0].rstrip()`, Python's
  whitespace set). `TocTree.parse_content` reads each entry line verbatim, so
  an entry indented deeper than its block (`   a` / `     b`) names the
  nonexisting document `'  b'` and leaves `b` an orphan, as Sphinx warns —
  this crate had trimmed it and resolved `b`. Warning-stream `%r` now escapes
  every non-printable character the way CPython's `repr` does
  (`'foo\xa0bar'`) from one `py_repr_str`; the `src/env/toctree.rs` copy had
  escaped only `< 0x20` and `0x7f`. The same round moved the remaining
  `trim_start()`/`trim_end()` sites to Python's `strip` semantics — a field
  body's or option description's leading NBSP is kept, and
  `process_index_entry`, `parselinenos`, `parse_line_num_spec`,
  `get_signatures`, `_filter_meta_fields`, the `::` tail, overlined titles,
  `line-block` lines, simple-table margins and option synonyms strip
  `\x1c`-`\x1f` like a space — and `include` in insert mode rstrips each line
  with Python's set before the line-length-limit check. Every change is
  pinned: docutils 718→735, sphinx 472→489, +2 env tests, +2 unit tests.
- **`.. _ name:` was parsed as a hyperlink target (M2 wave 4.5, panel fix
  round D).** docutils' target construct is `\.\.[ ]+_(?![ ]|$)`: a space or
  end-of-line right after the `_` makes the whole block a plain comment. This
  crate read `.. _ pad  lbl :` as a target whose stripped name collided with
  a real `.. _pad  lbl:` — a spurious `Duplicate explicit target name`
  message, and on its own a label Sphinx never has, so a `:ref:` to it
  resolved here and warned `undefined label` there. It is a comment now, as
  are a bare `.. _` (with or without an indented continuation) and
  `.. _\tx:` (tabs expand before the match); a backtick phrase that opens
  with a space or closes after one is `malformed hyperlink target.`, as
  docutils' target pattern says. The plain form keeps a space before its
  colon — probed, still a target.
- **Names, labels and URIs now split on Python's whitespace (M2 wave 4.5,
  panel fix round D).** Round C fixed the cross-reference targets; the same
  `\x1c`-`\x1f` gap was still in every docutils name normalizer
  (`fully_normalize_name`, `whitespace_normalize_name`, both `make_id`s), the
  target/anonymous/image/embedded URI cleanups, the indirect-reference check,
  the std domain's `ws_re` port for `envvar`/`confval`/`program`, and the
  `:option:` subcommand fold. `.. _a\x1fb:` is the label `a b` (both `:ref:`
  spellings reach it), `.. envvar:: FOO\x1fBAR` indexes `environment
  variable; FOO BAR`, `.. program:: git\x1fadd` scopes its options under
  `git-add`, and `:option:`git\x1fadd -x`` resolves — all as under Sphinx
  9.1.0. Pinned by 17 docutils cases, 5 sphinx cases and the `names_round_d`
  env-oracle project, compared at full strength.
- **A huge `:tab-width:` on an `include` reported the wrong error first (M2
  wave 4.5, panel fix round C).** The C-int range check that keeps an
  out-of-range `:tab-width:` from hanging the parser ran before the file
  was even opened, so `.. include:: missing.rst` with
  `:tab-width: 2147483648` reported the overflow where docutils reports the
  missing file. docutils reaches `expandtabs` only after the read and the
  clip succeed — behind `tab_width >= 0` in `:literal:`/`:code:` mode, and
  per line of `string2lines` in insert mode, which never expands an empty
  file at all. The check now fires exactly there; probed against docutils
  0.22.4 over missing/empty/normal files × the three modes × a huge and a
  hugely negative width, plus the `:start-after:` and clip-to-empty
  orderings.
- **An `include` through a symlink recorded the path it had not read (M2
  wave 4.5, panel fix round C).** The file *opened* followed the symlink,
  like Sphinx's `relfn2path` (which `.resolve()`s the joined path), but the
  two bookkeeping records — the included docname behind the "document isn't
  included in any toctree" check, and the dependency an incremental rebuild
  watches — still spelled the *lexical* path. `.. include:: link/../part.rst`
  beside a real `part.rst` therefore suppressed the orphan warning Sphinx
  prints for `part.rst`, and watched a file whose changes could not affect
  the build. Both records now follow the resolved path, spelled relative to
  the resolved source directory (`../ext/part.rst` for a file the link led
  out of the tree) — the same `env.dependencies` a real `sphinx-build` ends
  with, and the same `env.included` for every file *inside* the source
  directory. One knowing simplification remains for a `.rst` the link leads
  *outside* it: Sphinx's `path2doc` then records the absolute path itself as
  a pseudo-docname in `env.included`, this crate records nothing. The entry
  is output-inert (its only reader is the orphan check, which a `/`-rooted
  name can never satisfy) and is listed under the known divergences in
  `docs/IMPLEMENTATION_STATUS.md`.
- **Cross-reference targets now collapse Python's whitespace, and an
  explicit `Title <target>` keeps its padding (M2 wave 4.5, panel fix round
  C).** Sphinx's `ws_re` is Python's `\s`, which admits `\x1c`-`\x1f`;
  `:doc:`a\x1fb`` now reaches the resolver as `a b`, as it does under
  Sphinx. And the target between the brackets of an explicit title is taken
  verbatim — collapsed, never stripped — for every role, `:ref:` and
  `:numref:` included: they lowercase their target and nothing more, where
  this crate had been applying docutils' `fully_normalize_name`, which
  strips the ends as well.
- **`:eq:` is the math domain's role (M2 wave 4.5, panel fix round C).**
  Registered without a domain prefix like `:any:`, it produced a
  `pending_xref` with `refdomain=""`; `MathReferenceRole.result_nodes`
  stamps `refdomain="math"`, and the inner node's classes stay
  `xref eq`. Resolution of equation targets is a wave-5 domain, so an
  `:eq:` reference now joins the build's "domain not implemented" count
  instead of being run through the std resolver it never belonged to.
- **Directive validation invented four more warnings `sphinx-build` never
  emits (M2 wave 4.5, panel fix round B).** `.. include::` of a file whose
  extension is not `.rst`/`.txt`/`.md`/`.inc` — the docutils standard
  include files, `.. include:: <isonum.txt>`, among them — warned `Unusual
  file extension for include:`; `literalinclude` rejected `:lineno-start:`,
  `:tab-width:` and `:dedent:` values, and `code-block` `:lineno-start:`
  values, that Sphinx's option converters accept, with `… must be a
  positive integer` (`code-block` has no `:tab-width:` and accepted
  `:dedent:` all along); an empty
  `code-block` warned `Code-block directive has no content` (it is legal);
  and `toctree`'s `:maxdepth:` was range-checked although `-1` is the
  documented "unlimited". Each of those failed `-W` on a project Sphinx
  builds clean. All four are gone, the validator drift audit now sweeps
  negative and zero values, and a probe-clean Sphinx project is pinned
  end-to-end to earn no validation warning at all.
- **Warnings raised inside an included file now name that file (M2 wave
  4.5, panel fix round B).** Toctree, numbering and directive/role
  validation warnings for content that arrived through `.. include::` were
  reported against the *including* document at the included file's line
  number; they now carry the included file's path, as under Sphinx (modulo
  the relative-vs-absolute spelling recorded in the known divergences). Two
  cross-reference warnings were located wrongly as well: a dangling
  annotation reference in a Python signature (`def f(x: Missing)`) rendered
  `:0:` and named the wrong file — it now locates at the signature's own
  file and line, the included file's for a signature inside an include —
  and a dangling `:param Missing x:` doc-field reference located at the
  field list's own line where Sphinx walks up to the nearest ancestor that
  has a location (the enclosing section, an admonition, or no location at
  all directly under the document). Pinned byte-for-byte by the
  `py_locations` oracle project.
- **Directive validation invented `Unknown option '…'` warnings for options
  Sphinx accepts (M2 wave 4.5).** `literalinclude` warned about `:lines:`,
  `:emphasize-lines:` and `:lineno-match:`; `code-block` about `:force:` and
  `:class:`; `figure` about its own `:figwidth:`/`:figclass:` and about
  `:figname:` (all three naming the *image* directive in the message);
  `image` and `figure` about `:loading:`. Each of those failed `-W` on a
  project `sphinx-build` builds clean. `include`'s missing
  `:parser:`/`:class:`/`:name:` were fixed in the same sweep but never
  warned: that validator checks the argument and the file extension only,
  and nothing on the build path consults its option list — a latent trap
  rather than a live bug. Every validator's option list is now checked
  against the parser's own option spec, in both directions, by a test that
  covers all ten of them — which also removed `literalinclude`'s advertised
  `:start-line:`/`:end-line:`, options Sphinx's `literalinclude` does not
  have, and added `:figname:` to the parser's own `figure` table, where it
  was missing (docutils `images.py:125`).
- **A `glossary` comment split a multi-term entry (M2 wave 4.5).** A `.. `
  comment line between two terms produced two definition list items, the
  first with an empty `<definition>` — a shape docutils never emits. The
  entry split is now a faithful port of Sphinx's own line state machine, so
  terms on both sides of a comment share one entry, terms separated by a
  blank line share one entry (and warn), and a definition dedents by its
  first line rather than by the block minimum.
- **`:number-lines:` on an empty included file padded the line number to two
  columns** where docutils uses one.
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
- **`.. toctree::` with `:numbered: 2` no longer warns.** `:numbered:`
  takes an optional depth, and the directive validator had it filed as a
  valueless flag, so the documented spelling produced
  `numbered option should not have a value` on every build and failed `-W`.
- **Comment lines inside a `glossary` are no longer parsed as terms.** An
  unindented `.. ` line is a comment, as it is for Sphinx; previously each
  one became a glossary term with its own index entry, and a comment
  repeated in one glossary raised a spurious `duplicate term description`.
- **`-W` and `-n` no longer invalidate the build cache**, and a `conf.py`
  that sets two or more `html_context` keys no longer invalidates it on
  every run. Both were consequences of what the cache fingerprint covered.
- **Two source files that map to one document name are resolved
  deterministically**, keeping the one whose suffix comes first (previously
  both were built, to one output path, from one shared doctree). This is
  silent for a collision Sphinx's default `source_suffix` cannot see — a
  `page.rst` beside a `page.md` builds clean, as it does under Sphinx.
  Sphinx's `multiple files found for the document "…"` warning is reserved
  for a collision between two files Sphinx would both have read.
- **An `intersphinx_timeout` that is negative, NaN or absurdly large is
  ignored with a warning** rather than aborting the process.

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
