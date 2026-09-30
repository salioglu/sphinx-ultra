# M2 wave 5, sub-project 1 — the reporter channel and the read transforms

**Status:** design approved section by section in brainstorming (2026-09-30);
this document awaits the partner's review before planning.
**Parity targets:** Sphinx 9.1.0, docutils 0.22.4 (the repo's pinned oracles).
**Research:** `docs/superpowers/research/2026-09-30-m2-wave5-transforms.md`
(primary: §1.2 transform order, §3–§5 specs, §9 diagnostics, §10 gap table
T1–T21), `…-reporter-oracle.md` (primary: the channel), `…-doctree.md`,
`…-pipeline.md`, `…-env.md` (exemption map).

## 1. Where this sits

M2 wave 5 (ROADMAP §4, "HTML writer v1") is split into six sub-projects, each
with its own spec → plan → implementation cycle, in this order:

1. **Reporter channel + read transforms** — this document.
2. Builders, URIs and resolution (post-transforms, toctree resolution,
   collectors, math/citation resolution, `objects.inv` inputs).
3. Pygments-parity highlighter.
4. Theme/template engine.
5. HTML5 translator.
6. Page integration (StandaloneHTMLBuilder/dirhtml/dummy, finish tasks) —
   turns the committed page oracle (`tests/html_differential.rs`) on.

Sub-project 1 makes the *read side* produce what Sphinx's read phase produces:
the same persisted doctree and the same diagnostics, in the same order.

## 2. Goals and decisions

- **G1 — Reporter channel.** docutils `system_message`s of level ≥ 2 are
  printed the way Sphinx prints them: `{source}:{line}: {WARNING|ERROR|
  CRITICAL}: {text} [docutils]` at creation-time order, interleaved with the
  other read-phase records. This ends the known limitation that a broken
  `include` drops its content silently and `-W` stays green.
- **G2 — Read transforms.** The transforms Sphinx 9.1 applies in its read
  phase run on every document, in Sphinx's order, so the persisted doctree
  equals Sphinx's recorded doctree.
- **G3 — SmartQuotes**, as part of G2 (priority 750), at Sphinx's default
  (`smartquotes=True`).

Decisions taken with the partner:

- **D1 — Validators:** once docutils messages print, every M1
  directive/role validator check that reports something docutils or Sphinx
  already reports — or that has no Sphinx counterpart and fires on markup
  `sphinx-build` accepts — is removed. `validate_directives` stays on.
- **D2 — Exit policy:** reporter WARNING/ERROR/CRITICAL records are
  *warnings*: exit 0 without `-W`, exit 1 with `-W`, exactly like
  `sphinx-build`. The crate's own read failures (an unreadable source file,
  `BuildErrorReport`) keep exiting 1.
- **D3 — SmartQuotes is in this sub-project.**
- **D4 — Architecture A:** a separate transform pass after parse, in the
  parallel read phase, continuing the parser's id registry; the parse layer
  stays transform-free.

## 3. Scope

**In:**

- The reporter channel (G1) and a single ordered diagnostics stream per
  document.
- The read transforms, in this order (priority as probed from
  `document.transformer.applied`; research transforms.md §1.2):

  | Priority | Transform | Effect |
  |---|---|---|
  | 210 | DefaultSubstitutions | `\|version\|`, `\|release\|`, `\|today\|` when the document does not define them |
  | 210 | MoveModuleTargets | a leading `py:module` target's ids move onto its section |
  | 210 | HandleCodeBlocks | a block quote holding only doctest blocks is unwrapped |
  | 210 | AutoNumbering | captioned figure/table/code container with no id gets `idN` (replaces the parse-time approximation in `src/rst/block.rs`) |
  | 220 | Substitutions (docutils) | substitution references replaced, with docutils' errors |
  | 220 | ReorderConsecutiveTargetAndIndexNodes | index nodes moved ahead of a target run |
  | 260 | PropagateTargets (docutils) | applied **in the tree** (today only replayed read-only) |
  | 261 | SortIds | a section's leading `idN` id moved last |
  | 340 | DocInfo (docutils) + Sphinx's metadata removal | leading field list → docinfo → metadata, node removed |
  | 440 | AnonymousHyperlinks | anonymous refs ↔ anonymous targets |
  | 460 | IndirectHyperlinks | indirect targets and references resolved |
  | 500 | DoctestTransform | `doctest` class on doctest blocks |
  | 619 | citation definition/reference transforms | `citation[docname]`, duplicate-citation warning, `citation_reference` → `pending_xref` (resolution is sub-project 2) |
  | 620 | Footnotes (docutils) | numbering, labels, backrefs, symbol footnotes |
  | 622 | UnreferencedFootnotesDetector | `Footnote [..] is not referenced.` |
  | 640 | ExternalTargets | `refname` → `refuri` |
  | 660 | InternalTargets | `refname` → `refid` |
  | 700 | FootnoteDocnameUpdater | `docname` on footnotes and footnote references |
  | 750 | SphinxSmartQuotes | quotes, dashes, ellipses per language |
  | 830 | Transitions (docutils) | transition validation and relocation |
  | 850 | SphinxDanglingReferences | unresolved `refname` → `problematic` + ERROR |
  | 999 | FilterSystemMessages | drop in-tree messages below level 5, or below 2 with `keep_warnings` |

  Transforms proven no-ops for an HTML build (StripComments, Decorations,
  Validate, ExposeInternals, UIDTransform, i18n without catalogs,
  AutoIndexUpgrader, RefOnlyBulletList with `html_compact_lists=True`) are
  listed in the transform table but not implemented.
- Toctree `rawentries` (the explicit entry titles) and `rawcaption` at parse
  time (i18n PreserveTranslatableMessages; pformat parity only).
- Config keys, each readable from `conf.py`, YAML/JSON and `-D`, with
  Sphinx 9.1's defaults: `smartquotes` (`True`), `smartquotes_action`
  (`'qDe'`), `smartquotes_excludes` (`{'languages': ['ja', 'zh_CN',
  'zh_TW'], 'builders': ['man', 'text']}`), `keep_warnings` (`False`),
  `version` (`''`), `release` (`''`), `today` (`''`), `today_fmt` (`None`,
  meaning `'%b %d, %Y'`), `highlight_language` (`'default'`, used as the
  parse-time default where the parser hard-codes `"default"` today).
- The validator audit (D1).

**Out (sub-project 2 or later):** post-transforms (OnlyNodeTransform,
HighlightLanguageTransform, TrimDoctestFlagsTransform, ReferencesResolver
changes), toctree resolution, the image and download collectors, the math
domain, citation *resolution*, builder URIs, anything `html_*`.

## 4. Architecture

- **`src/transforms/`** (new): `mod.rs` (the transform table and
  `apply_read_transforms`), `references.rs` (substitutions, target
  propagation and sorting, the hyperlink family, dangling references),
  `footnotes.rs` (footnotes, citation transforms, docname updater,
  unreferenced detector), `frontmatter.rs` (DocInfo and metadata removal),
  `misc.rs` (Transitions, AutoNumbering, HandleCodeBlocks, Doctest,
  FilterSystemMessages, MoveModuleTargets, Reorder, SortIds),
  `smartquotes.rs`. Each ports its upstream function line by line and cites
  `file.py:line`.
- **Entry point:** `apply_read_transforms(doctree: &mut Doctree, ctx:
  &mut TransformCtx) ` runs in the read phase immediately after
  `parse_rst_full`, before the doctree is persisted and before the merge
  phase's domain hooks. Sphinx mode only.
- **`TransformCtx`** carries the doctree's continued id registry (the
  parser's used-id set, per-prefix counters, name ids and name types, handed
  back in `ParseOutput`), the docutils document lists rebuilt by one walk
  (name ids/types, referenced names, anonymous targets and refs, auto- and
  symbol-footnotes, footnote and citation references, substitution
  definitions), the config slice from §3, the docname, and the diagnostics
  sink.
- **Parse layer purity:** everything `tests/doctree_differential.rs` calls
  stays transform-free; `tests/sphinx_doctree_differential.rs` (whose oracle
  is Sphinx's post-transform read phase) calls parse + transforms.

## 5. Diagnostics stream

- One ordered list per document of records `{seq, channel, level,
  type/subtype, text, source, line: Option<u32>}`, `channel ∈ {reporter,
  logger}`, replacing today's separate toctree-warning list, parse-log
  warnings and the never-printed in-tree messages.
- The parser appends a reporter record **when docutils would write it** —
  at message creation, including messages whose node is later discarded —
  and appends its logger records (toctree warnings, py/std registration
  warnings, literalinclude reader warnings) in creation order. docutils'
  quirk is reproduced: a directive error's printed text lacks the literal
  block appended after creation, while the tree keeps it.
- Transforms append after the parse records.
- **Merge phase** (serial, docname order, documents read this build only):
  print the document's stream; then the index and std `process_doc` hooks
  (Sphinx's SphinxDomains, which follows SphinxDanglingReferences); then
  the existing collection (toc building, labels, index entries) over the
  post-transform tree; then persist the doctree. Numbering stays where it
  is, in the resolve phase, and now also reads post-transform ids. A
  document served from cache prints nothing — `sphinx-build` does not
  re-read it either.
- **Rendering:** `{source}:{line}: {LEVEL}: {text}[ [type.subtype]]`;
  reporter records carry type `docutils`; level 4 (SEVERE) renders
  `CRITICAL`; a record without a line renders `{source}:: {LEVEL}: …`;
  multi-line bodies print verbatim to stderr and the `-w` file. Level 1
  (INFO) never prints. Every printed record counts toward the warning total
  (D2).
- Resolution-phase warnings keep their current position after all
  read-phase output.
- Included-file locations keep the documented srcdir-relative spelling
  (IMPLEMENTATION_STATUS "Provenance path spelling", plan §Scope-8); the
  message bytes match Sphinx.

## 6. SmartQuotes and escapes

- docutils keeps backslash-escape information inside its text (`\x00`
  markers); the crate's inline parser drops it. Text nodes gain a side field
  holding the byte offsets of escaped characters, written only by the inline
  parser, read only by SmartQuotes, ignored by `pformat` and `astext`.
- The per-language quote tables are generated from docutils 0.22.4 by a
  tool script (`tools/gen_smartquotes_tables.py`, same pattern as
  `tools/gen_punctuation_tables.py`).
- `smartquotes_excludes` (languages and builders) is honored; a language
  without a table gets docutils' `No smart quotes defined for language
  "xx".` warning; non-smartquotable nodes follow docutils' and Sphinx's rules
  (FixedTextElement, Special, `support_smartquotes=False`, literal and raw
  content, the Sphinx node list).

## 7. Error handling and edge cases

- **Totality:** no transform panics on any parser output
  (`tests/rst_proptest.rs` gains parse + transforms sweeps).
- **Upstream error paths ported with their exact texts:** circular
  substitution definitions, undefined or duplicate substitution references,
  circular or unknown indirect targets, anonymous hyperlink mismatch, too
  many autonumbered footnote references, unreferenced footnotes, misplaced
  transitions, dangling references (INFO suppressed, ERROR printed with a
  `problematic` node).
- **Location of a message with no source node** (anonymous mismatch,
  substitution line-length): the end-of-parse position — `lines + 1` in a
  simple document, a line-less `{source}::` location when the document ends
  inside nested directive content. The simple case is ported; the nested
  case is ported if the parser can know it, otherwise ledgered with its
  probe.
- **Parser message audit:** each message-construction site in `src/rst`
  either corresponds to a docutils creation (and therefore prints) or is
  changed not to create a message docutils does not; messages docutils
  creates inside a nested parse that is later discarded are reproduced
  where the parser sees them, otherwise ledgered with the input.
- **Formats:** `DOCTREE_FORMAT_VERSION` 2 → 3 (post-transform meaning plus
  the escape field); `ENV_VERSION` bumps only if the environment's contents
  change. The first build after upgrading is a cold build.
- **`keep_warnings=True`** keeps level ≥ 2 messages in the tree for the
  writer (sub-project 5).

## 8. Testing and oracles

TDD per task. Every oracle keeps the repo's strict, self-cleaning exemption
discipline (a listed case that stops diverging fails) and extend-only
fixtures (existing cases regenerate byte-identically).

1. **Sphinx read oracle** (`tools/gen_sphinx_fixture.py`): its test runs
   parse + transforms; the transform families its header lists as EXCLUDED
   return as cases (targets, anonymous and indirect links, footnotes,
   citations, substitutions, docinfo, transitions, doctest blocks, INFO
   stripping); every case records its warnings stream (reporter + transform
   records, exact order and text); a SmartQuotes family runs with
   `smartquotes=True` over en/de/fr/ja plus escape and exclusion cases.
2. **docutils parse oracle** (`tools/gen_doctree_fixture.py`): trees
   unchanged; each case records the reporter stream as docutils writes it
   at creation, pinning membership and order (including the directive-error
   literal quirk).
3. **Environment oracle** (`tools/gen_env_fixture.py`): a new
   `warning_records` key (whole records; the existing line-split `warnings`
   key hides boundaries); new projects for reporter/logger interleaving,
   include failures, `keep_warnings=True` and `smartquotes=True`.
4. **End to end** (`tests/e2e_cli.rs`): a missing include prints
   `CRITICAL … [docutils]`, exits 0, and exits 1 under `-W`; the `-w` file
   carries the same records as stderr; validator texts follow D1.
5. **Totality:** proptest sweeps over parse + transforms, green at
   `PROPTEST_CASES=2048`.

## 9. Acceptance

- `cargo test --locked` green; zero divergence on every extended fixture.
- `tests/env_differential.rs` exemptions closed by this sub-project are
  removed — expected: the PropagateTargets entries (`labels_dups/a`,
  `labels_dups/b`, `index_entries/a`, `py_any/b`), the MoveModuleTargets
  entries (`py_basic/a`, `py_dup/b`, `py_toc/mod`, `py_toc_parents/mod`,
  `py_modindex/index`, `py_modindex_prefix/index`), `orphan_doc/orphan`,
  `KNOWN_WARNING_GAPS["inc_basic"]`, and `KNOWN_INERT_CONF`'s
  `smartquotes` and `keep_warnings`; a listed document that still diverges
  for a sub-project-2 reason (e.g. toctree resolution) moves to that
  reason's table instead. The exemption-arithmetic test and the figures in
  ROADMAP §2 and IMPLEMENTATION_STATUS change in the same commit.
- `cargo fmt --all`, `cargo clippy --all-targets --locked -- -D warnings`,
  `RUSTUP_TOOLCHAIN=1.85 cargo check --locked --all-targets` clean.
- ROADMAP §2, `docs/IMPLEMENTATION_STATUS.md` and `CHANGELOG.md` updated
  (ROADMAP §12), including the D1 validator removals and the D2 exit policy
  as user-visible changes.

## 10. Prior art

A stopped, pre-spec implementer branch (`worktree-agent-a4f3ddbcbdf34254d`,
commit `58c850a`, "the docutils reporter channel") exists locally. It was
written without this spec: implementers may read it for reference; nothing
in it is accepted without passing this sub-project's reviews.
