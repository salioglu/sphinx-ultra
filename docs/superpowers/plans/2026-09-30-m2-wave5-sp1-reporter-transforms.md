# M2 wave 5 · sub-project 1 — reporter channel and read transforms — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make sphinx-ultra's read phase produce Sphinx 9.1's persisted doctree and Sphinx's read-phase diagnostics — docutils reporter messages printed in order, and the ~22 observable read transforms, SmartQuotes included.

**Architecture:** The parser records every diagnostic at creation into one ordered per-document stream and hands its id registry back; a new `src/transforms/` pass runs Sphinx's read transforms in priority order right after parse (parallel read phase); the merge phase prints each re-read document's stream, then runs the domain hooks. The parse layer stays transform-free for the docutils oracle.

**Tech Stack:** Rust 2021 (MSRV 1.85), the crate's generic doctree IR (`src/doctree`), Python oracle generators under `tools/` run through the pinned `uv` command.

**Spec:** `docs/superpowers/specs/2026-09-30-m2-wave5-sp1-reporter-transforms-design.md`. Research the tasks cite: `docs/superpowers/research/2026-09-30-m2-wave5-{transforms,reporter-oracle,doctree,pipeline,env}.md` (`probes/` beside them).

## Global Constraints

- Parity targets: Sphinx **9.1.0**, docutils **0.22.4**. Every oracle regeneration uses exactly: `PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' python tools/<generator>.py` (`PYTHONNOUSERSITE=1` is mandatory).
- Fixtures are **extend-only**: a regeneration must reproduce every existing case/project byte-identically; only new cases/keys may appear.
- Exemption tables are **strict and self-cleaning**: a listed item that stops diverging fails its test; `exemption_arithmetic_matches_the_documented_numbers` in `tests/env_differential.rs` must pass.
- **Cache-shape rule:** a field added to a persisted shape (`RegistryExport`, `Document`, anything under `Doctree`) gets **no** `#[serde(default)]`; extend the `must_miss` decode tests in `src/rst/mod.rs` for every new `RegistryExport` field; bump `DOCTREE_FORMAT_VERSION` (`src/builder.rs`) from **2 to 3** exactly once (Task 6) and re-bump only if a later task changes the persisted meaning again (record why in the commit).
- Warning rendering: `{source}:{line}: {LEVEL}: {text}` + ` [{category}]` when a category exists; reporter records carry category `docutils`; level 2 → `WARNING`, 3 → `ERROR`, 4 → `CRITICAL`; no line → `{source}:: {LEVEL}: {text}`; level 1 never prints.
- Exit policy: every printed record counts toward the warning total; exit 0 without `-W`, exit 1 with `-W`; reporter records are never `BuildErrorReport`s.
- Config defaults (Sphinx 9.1 `config.py`): `smartquotes=True`, `smartquotes_action='qDe'`, `smartquotes_excludes={'languages': ['ja', 'zh_CN', 'zh_TW'], 'builders': ['man', 'text']}`, `keep_warnings=False`, `version=''`, `release=''`, `today=''`, `today_fmt=None` (meaning `'%b %d, %Y'`), `highlight_language='default'`, `language='en'`.
- Code style: match the surrounding code — dense doc comments citing upstream `file.py:line` for every ported behaviour; claims only from probes.
- TDD (superpowers:test-driven-development): no production code without a failing test watched first. **Do not read, adapt or copy the stopped pre-spec branches** (`worktree-agent-*`); implement fresh from tests.
- Every task ends with: `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` (full suite; report any failure by name), and — when the toolchain is present — `RUSTUP_TOOLCHAIN=1.85 cargo check --locked --all-targets`.
- Commit messages end with the two lines:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_013cB9UwssanjiwebAbyLCFQ`.
- Docs (ROADMAP §2, `docs/IMPLEMENTATION_STATUS.md`, `CHANGELOG.md`) are touched only in Task 15.

## Review Focus

1. **Warm incremental builds:** a document served from cache prints none of its read-phase records, and its loaded doctree is the post-transform one — pinned in Task 4 (`a_cached_document_prints_no_read_diagnostics`) and Task 6 (`a_warm_build_loads_the_transformed_doctree`).
2. **Diagnostics from included files:** a reporter message raised inside an `include`d file carries that file's source and line, and transforms working across spliced content keep it — pinned in Task 4 (`an_include_error_inside_an_included_file_names_that_file`).
3. **Python-whitespace and non-ASCII text through transforms:** NBSP/`\x1f` in target names, substitution names, footnote labels and SmartQuotes input — pinned by oracle cases in Tasks 7, 8, 10 and 14 (each task adds at least one `…_nbsp` case).
4. **Deep nesting:** transforms walk arbitrarily deep trees without stack overflow — pinned in Task 15 (`transforms_survive_the_deep_nesting_sweep`, the proptest deep-nesting generator through parse + transforms).
5. **`-W`/`-w` accounting with CRITICAL records:** the `-w` file carries every printed record and `-W` fails on a CRITICAL-only build — pinned in Task 4 (`w_file_and_stderr_carry_the_same_reporter_records`, `dash_w_fails_a_build_whose_only_record_is_critical`).

---

## File Structure

| Path | Responsibility | Tasks |
|---|---|---|
| `src/config.rs`, `src/python_config.rs` | new config keys and Sphinx defaults | 1 |
| `src/error.rs` | `WarningLevel`, level-aware `BuildWarning` rendering | 2 |
| `src/rst/diagnostics.rs` (new) | `Diagnostic`, `DiagnosticChannel`, the per-parse `Reporter` recorder | 2, 3 |
| `src/rst/mod.rs` | `RegistryExport.diagnostics`, `ParseOutput.ids`, removal of `log_warnings` | 3 |
| `src/rst/block.rs`, `src/rst/inline.rs`, `src/doctree/ids.rs` | record at creation; message audit; escape side field (13); rawentries/rawcaption and highlight default (12) | 3, 12, 13 |
| `src/builder.rs`, `src/main.rs` | merge-phase emission; transforms wired into read; format bump | 4, 6 |
| `src/directives/validation/**` | D1 audit | 5 |
| `src/transforms/mod.rs` (new) | `TransformConfig`, `TransformCtx`, transform table, `apply_read_transforms`, `parse_and_transform` | 6 |
| `src/transforms/misc.rs` (new) | FilterSystemMessages, MoveModuleTargets, Reorder, SortIds, AutoNumbering, HandleCodeBlocks, Doctest, Transitions | 6, 7, 11 |
| `src/transforms/references.rs` (new) | PropagateTargets, Substitutions, hyperlink family, dangling references | 7, 8, 9 |
| `src/transforms/footnotes.rs` (new) | footnotes, citations, docname updater, unreferenced detector | 10 |
| `src/transforms/frontmatter.rs` (new) | DocInfo + metadata removal | 11 |
| `src/transforms/smartquotes.rs` (new), `src/transforms/smartquotes_tables.rs` (generated) | SmartQuotes | 14 |
| `src/doctree/mod.rs` | `Node.escapes` side field | 13 |
| `tools/gen_doctree_fixture.py`, `tools/gen_sphinx_fixture.py`, `tools/gen_env_fixture.py`, `tools/gen_smartquotes_tables.py` (new) | oracle extensions | 3, 4, 6–14 |
| `tests/doctree_differential.rs`, `tests/sphinx_doctree_differential.rs`, `tests/env_differential.rs`, `tests/e2e_cli.rs`, `tests/rst_proptest.rs` | comparisons | 3–15 |

---

### Task 1: Configuration keys with Sphinx 9.1 defaults

**Files:**
- Modify: `src/config.rs` (fields, `Default`, `apply_override`), `src/python_config.rs` (conf.py extraction + `apply_to`)
- Test: unit tests in both files

**Interfaces:**
- Produces on `BuildConfig`: `smartquotes: bool`, `smartquotes_action: String`, `smartquotes_excludes: SmartquotesExcludes { languages: Vec<String>, builders: Vec<String> }`, `keep_warnings: bool`, `today: String`, `today_fmt: Option<String>`, `highlight_language: String`; `version`/`release` keep `Option<String>` but default to `None` (read as `''` by consumers).

- [ ] **Step 1: Write the failing tests** in `src/config.rs`:
  - `sphinx_defaults_for_the_read_transform_keys`: `BuildConfig::default()` has `smartquotes == true`, `smartquotes_action == "qDe"`, excludes languages `["ja","zh_CN","zh_TW"]` and builders `["man","text"]`, `keep_warnings == false`, `today == ""`, `today_fmt == None`, `highlight_language == "default"`, `version == None`, `release == None`.
  - `read_transform_keys_are_d_overridable`: `apply_override("smartquotes","0")` → `false`; `("keep_warnings","1")` → `true`; `("today","2026-01-01")`; `("today_fmt","%Y")` → `Some("%Y")`; `("highlight_language","none")`; `("smartquotes_action","q")`.
  - In `src/python_config.rs`: `read_transform_keys_are_read_from_conf_py` parsing `smartquotes = False\nsmartquotes_action = 'De'\nsmartquotes_excludes = {'languages': ['de'], 'builders': []}\nkeep_warnings = True\ntoday = 'X'\ntoday_fmt = '%d'\nhighlight_language = 'python'\nversion = '1.2'\nrelease = '1.2.3'\n` and asserting each field after `apply_to`.
- [ ] **Step 2: Run** `cargo test --locked --lib read_transform_keys sphinx_defaults_for_the_read_transform_keys` — expect FAIL (fields missing).
- [ ] **Step 3: Implement** the fields with `#[serde(default)]`-style config defaults (config structs, not persisted shapes), conf.py extraction, `-D` coercion; update any existing test that relied on the `"1.0.0"` version/release default.
- [ ] **Step 4: Run** the same filter — PASS; then the full-suite constraint commands.
- [ ] **Step 5: Commit** `feat(config): the read-transform keys at Sphinx 9.1's defaults`.

### Task 2: Diagnostic records and level-aware warning rendering

**Files:**
- Create: `src/rst/diagnostics.rs`
- Modify: `src/rst/mod.rs` (`pub mod diagnostics;`), `src/error.rs`
- Test: unit tests in both files

**Interfaces:**
- Produces: `pub enum DiagnosticChannel { Reporter, Logger }`; `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] pub struct Diagnostic { pub seq: u32, pub channel: DiagnosticChannel, pub level: u8, pub category: Option<String>, pub text: String, pub source: u16, pub line: Option<u32>, pub doc2path_location: bool }`; `pub enum WarningLevel { Warning, Error, Critical }` with `WarningLevel::from_docutils(level: u8) -> WarningLevel` (2→Warning, 3→Error, ≥4→Critical); `BuildWarning.level: WarningLevel` (constructors default `Warning`); `BuildWarning::from_diagnostic(d: &Diagnostic, source_path: PathBuf) -> BuildWarning` (reporter channel → category `Some("docutils")`).

- [ ] **Step 1: Write the failing tests** in `src/error.rs`:
  - `reporter_records_render_with_level_and_docutils_type`: a reporter `Diagnostic{level:4, line:Some(14), text:"Problems with \"include\" directive path:\nInputError: [Errno 2] No such file or directory: 'nothere.rst'."}` at `index.rst` renders exactly `index.rst:14: CRITICAL: Problems with "include" directive path:\nInputError: [Errno 2] No such file or directory: 'nothere.rst'. [docutils]`.
  - `level_three_renders_error`: level 3 → `index.rst:2: ERROR: … [docutils]`.
  - `a_record_without_a_line_renders_a_double_colon`: `line: None`, level 3, text `Anonymous hyperlink mismatch: 1 references but 0 targets.\nSee "backrefs" attribute for IDs.` → `index.rst:: ERROR: Anonymous hyperlink mismatch: 1 references but 0 targets.\nSee "backrefs" attribute for IDs. [docutils]`.
  - `logger_records_keep_their_category_or_none`: a logger record with `category: None` renders with no suffix; with `Some("toc.not_readable")` renders ` [toc.not_readable]`.
  - `existing_warnings_still_render_as_warning`: `BuildWarning::new(...)` output is unchanged (regression guard).
- [ ] **Step 2: Run** `cargo test --locked --lib error::` — FAIL.
- [ ] **Step 3: Implement** the types and `render` (the `-w` file and stderr both use `render`).
- [ ] **Step 4: Run** — PASS; full-suite constraint commands.
- [ ] **Step 5: Commit** `feat(diagnostics): the Diagnostic record and WARNING/ERROR/CRITICAL rendering`.

### Task 3: Record every parse-time diagnostic at creation, pinned by both parse oracles

**Files:**
- Modify: `src/rst/diagnostics.rs` (the recorder), `src/rst/block.rs` (every `msg`/`msg_sm`/`messages::system_message` site, `directive_run_message`, toctree and parse-log records, py/std registration records), `src/rst/inline.rs`, `src/doctree/ids.rs` (duplicate-name messages), `src/rst/mod.rs` (`RegistryExport.diagnostics` replaces `log_warnings`; `ToctreeRecord.warnings` folded in and removed; `ParseOutput.ids: IdRegistry`), the builder/env call sites that read the removed fields (compile only — printing changes are Task 4)
- Modify: `tools/gen_doctree_fixture.py` (per-case `stream`: the reporter's writes at creation, as Sphinx's `WarningStream` would print them), `tools/gen_sphinx_fixture.py` (per-case `warnings`: the ordered records a real build prints for the snippet)
- Test: `tests/doctree_differential.rs`, `tests/sphinx_doctree_differential.rs`, unit tests in `src/rst/diagnostics.rs` and `src/rst/mod.rs`

**Interfaces:**
- Consumes: `Diagnostic`, `DiagnosticChannel` (Task 2).
- Produces: `pub struct Reporter` with `next_seq(&self) -> u32`, `report(&self, msg: &Node)` (records a level ≥ 2 message using its text at call time), `log(&self, level: u8, category: Option<String>, text: String, source: u16, line: Option<u32>, doc2path_location: bool)`, `take(self) -> Vec<Diagnostic>`; `RegistryExport.diagnostics: Vec<Diagnostic>` (ordered by `seq`); `ParseOutput.ids: crate::doctree::ids::IdRegistry` (not serialized).

- [ ] **Step 1: Extend the generators** (extend-only) and regenerate both fixtures; confirm `git diff` shows only the added keys.
- [ ] **Step 2: Write the failing comparisons:** `tests/doctree_differential.rs::every_case_streams_what_docutils_writes` (compares `parse_rst_full(...).registry.diagnostics` rendered as `{line}: (LEVEL/N) text` against `stream`) and `tests/sphinx_doctree_differential.rs::every_case_warns_what_sphinx_prints`; unit test `a_directive_error_prints_without_its_literal` (`.. note::` + `   :bogus:` style `self.error()` path: the record's text excludes the literal block the tree message carries) and `records_are_numbered_in_creation_order`; extend `a_registry_written_before_the_std_records_existed_fails_to_decode` with `"diagnostics"` and drop `"log_warnings"`.
- [ ] **Step 3: Run** `cargo test --locked --test doctree_differential --test sphinx_doctree_differential` — FAIL on the new tests.
- [ ] **Step 4: Implement** the recorder; route every creation site through it; audit each site against docutils (a site docutils never creates is changed not to create; a message docutils creates in a discarded nested parse is recorded — ledger any you cannot reach, with the input, in the report); give toctree, parse-log and registration records their `seq` at creation.
- [ ] **Step 5: Run** both targets and `cargo test --locked --lib rst::` — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(reporter): record every parse-time diagnostic at creation`.

### Task 4: Print each re-read document's diagnostics in Sphinx's order

**Files:**
- Modify: `src/builder.rs` (`report_parse_warnings` → emit `document.registry.diagnostics` in `seq` order, then the index/std `process_doc` warnings), `src/main.rs` (level-aware stderr/`-w`/`-W` accounting)
- Modify: `tools/gen_env_fixture.py` (new per-project key `warning_records`: whole records; new projects `reporter_interleave` (reporter + toctree + py-duplicate records interleaved in one document), `inc_missing` (a missing include), `inc_nested_error` (an error raised inside an included file))
- Test: `tests/env_differential.rs`, `tests/e2e_cli.rs`

**Interfaces:**
- Consumes: `RegistryExport.diagnostics` (Task 3), `BuildWarning::from_diagnostic` (Task 2).

- [ ] **Step 1: Regenerate** the env fixture (extend-only).
- [ ] **Step 2: Write the failing tests:** `tests/env_differential.rs::warning_records_match_the_oracle` (whole records; remove `KNOWN_WARNING_GAPS["inc_basic"]`); `tests/e2e_cli.rs`: `a_missing_include_prints_critical_and_exits_zero`, `dash_w_fails_a_build_whose_only_record_is_critical` (exit 1), `w_file_and_stderr_carry_the_same_reporter_records`, `a_cached_document_prints_no_read_diagnostics` (warm `--incremental` rebuild prints none of the first build's reporter records), `an_include_error_inside_an_included_file_names_that_file`.
- [ ] **Step 3: Run** `cargo test --locked --test env_differential --test e2e_cli` — FAIL.
- [ ] **Step 4: Implement** the ordered emission (only for documents read this build) and main-side accounting.
- [ ] **Step 5: Run** — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(reporter): print docutils diagnostics in Sphinx's read order`.

### Task 5: Validator audit (decision D1)

**Files:**
- Modify: `src/directives/validation/builtin.rs`, `src/directives/validation/roles.rs`, `src/directives/validation.rs`, `src/builder.rs` (`validate_directives_and_roles` only if a whole validator goes)
- Test: unit tests beside each validator; `tests/e2e_cli.rs` pins that referenced removed texts

- [ ] **Step 1: Write the failing tests:** for each of the ten directive validators and ten role validators, a test `<validator>_is_silent_where_docutils_already_reports` feeding the markup docutils/Sphinx itself diagnoses (e.g. `.. note::` with no content) and asserting the validator emits nothing; and `<validator>_is_silent_on_markup_sphinx_accepts` for each check with no Sphinx counterpart that fires on accepted markup. Keep a check only when a test proves it reports something neither docutils nor Sphinx reports and never fires on accepted markup.
- [ ] **Step 2: Run** `cargo test --locked --lib directives::validation` — FAIL.
- [ ] **Step 3: Remove** the duplicating/fabricating checks (delete dead helpers they leave); update the e2e pins.
- [ ] **Step 4: Run** — PASS; full-suite constraint commands.
- [ ] **Step 5: Commit** `fix(validation): stop double-reporting what docutils now prints` — body lists every removed check (the audit table also goes in the task report).

### Task 6: The transform pass, FilterSystemMessages and the format bump

**Files:**
- Create: `src/transforms/mod.rs`, `src/transforms/misc.rs`
- Modify: `src/lib.rs` (`pub mod transforms;`), `src/parser.rs` (sphinx-mode `.rst` runs `apply_read_transforms` after `parse_rst_full`), `src/builder.rs` (`DOCTREE_FORMAT_VERSION` 2 → 3), `tests/sphinx_doctree_differential.rs` (parse + transforms), `tests/env_differential.rs` (`KNOWN_INERT_CONF` loses `keep_warnings`)
- Test: unit tests in `src/transforms/`, the two harnesses, `src/builder.rs` tests

**Interfaces:**
- Consumes: `ParseOutput.ids`, `Reporter`/`Diagnostic` (Task 3), config keys (Task 1).
- Produces: `pub struct TransformConfig { pub smartquotes: bool, pub smartquotes_action: String, pub smartquotes_excludes: SmartquotesExcludes, pub keep_warnings: bool, pub language: String, pub version: String, pub release: String, pub today: String, pub today_fmt: Option<String> }` with `impl Default` (Sphinx defaults) and `impl From<&BuildConfig>`; `pub fn apply_read_transforms(tree: &mut Doctree, ids: IdRegistry, docname: &str, config: &TransformConfig, diagnostics: &mut Vec<Diagnostic>)`; `pub fn parse_and_transform(source: &str, opts: &ParseOptions, config: &TransformConfig) -> (Doctree, Vec<Diagnostic>)`; the transform table `static READ_TRANSFORMS: &[(u16, &str, fn(&mut TransformCtx))]` in Sphinx order (no-ops listed with a `None`-bodied entry or omitted with a comment naming them).

- [ ] **Step 1: Write the failing tests:** `filter_system_messages_drops_everything_below_severe_by_default`, `keep_warnings_keeps_levels_two_and_up`, `info_messages_never_survive`; `a_warm_build_loads_the_transformed_doctree` (builder test: persisted blob decodes to the post-transform tree); `a_version_2_doctree_is_a_cache_miss`; switch `tests/sphinx_doctree_differential.rs` to `parse_and_transform` with the fixture's `keep_warnings=True`/`smartquotes=False`; add the sphinx-fixture case `tx_filter.info_message_stripped` (an INFO-emitting snippet, e.g. a duplicate implicit target name — previously excluded by the corpus policy) and the env projects `keep_warnings_true` and `keep_warnings_false` (the same document with a WARNING-level and an INFO-level message, built under each setting: the resolved doctree keeps only the WARNING under `True` and no `system_message` at all under `False`); remove `keep_warnings` from `KNOWN_INERT_CONF`.
- [ ] **Step 2: Run** `cargo test --locked --lib transforms:: builder::` and the two harnesses — FAIL.
- [ ] **Step 3: Implement** `TransformCtx` (the continued `IdRegistry`, the docutils document lists rebuilt by one walk, config, docname, diagnostics sink), the table, the read-phase call, FilterSystemMessages, the bump.
- [ ] **Step 4: Run** — PASS; full-suite constraint commands.
- [ ] **Step 5: Commit** `feat(transforms): the read-transform pass and FilterSystemMessages`.

### Task 7: Target transforms in the tree

**Files:**
- Modify: `src/transforms/misc.rs` (MoveModuleTargets 210, ReorderConsecutiveTargetAndIndexNodes 220, SortIds 261), `src/transforms/references.rs` (PropagateTargets 260), `src/env/std_domain.rs` + `src/env/numbers.rs` (the read-only PropagateTargets replays become identity or are deleted — labels and numbering must read the post-transform ids), `tools/gen_sphinx_fixture.py` (family `tx_targets`; re-admit `py.module_basic`, `py.duplicate_modules`), `tests/env_differential.rs`
- Research: transforms.md §3.2

- [ ] **Step 1: Add oracle cases** (extend-only): `tx_targets.block_target_then_paragraph` (`.. _t:\n\npara\n`), `…chained_targets`, `…target_then_section`, `…identity_section_label` (`.. _lbl:\n\nIdentity\n========\n` — SortIds), `…target_index_reorder` (`.. _t:\n.. index:: x\n\npara\n`), `…module_target_moves_to_section`, `…target_name_nbsp` (a name containing U+00A0); regenerate.
- [ ] **Step 2: Write the failing expectations:** the new cases compare in `tests/sphinx_doctree_differential.rs`; remove from `KNOWN_RESOLVED_GAPS` the PropagateTargets entries `labels_dups/a`, `labels_dups/b`, `index_entries/a`, `py_any/b` and the MoveModuleTargets entries `py_basic/a`, `py_dup/b`, `py_toc/mod`, `py_toc_parents/mod`, `py_modindex/index`, `py_modindex_prefix/index` (an entry still diverging for a sub-project-2 reason moves to that reason instead); update the arithmetic test's numbers.
- [ ] **Step 3: Run** `cargo test --locked --test sphinx_doctree_differential --test env_differential` — FAIL.
- [ ] **Step 4: Implement** the four transforms per research §3.2.
- [ ] **Step 5: Run** — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(transforms): target propagation, module targets, reorder and SortIds in the tree`.

### Task 8: Substitutions

**Files:**
- Modify: `src/transforms/references.rs` (DefaultSubstitutions 210 — `|version|`, `|release|`, `|today|`, `|translation progress|` with the exact text the Step 1 probe records; docutils Substitutions 220 with its errors), `tools/gen_sphinx_fixture.py` (family `tx_subst`, per-case conf overrides for `version`/`release`/`today`)
- Research: transforms.md §3.1

- [ ] **Step 1: Add oracle cases:** `tx_subst.replace`, `…unicode`, `…nested`, `…circular` (error text), `…undefined` (error text + `problematic`), `…case_insensitive_fallback`, `…default_version_release` (conf `version='1.2'`, `release='1.2.3'`), `…default_today_fixed` (conf `today='Sept 30'`), `…today_fmt` (conf `today_fmt='%Y'` with `SOURCE_DATE_EPOCH` pinned by the generator — document the pin in its header), `…doc_definition_wins`, `…name_nbsp`, `…translation_progress`, `…expansion_exceeds_line_length_limit` (docutils' substitution line-length error and its no-node location, research §9.3); regenerate.
- [ ] **Step 2: Failing comparisons** in `tests/sphinx_doctree_differential.rs` (records included).
- [ ] **Step 3: Run** — FAIL.
- [ ] **Step 4: Implement** per §3.1 (`today` without `today` config uses the build date through `today_fmt`; the date source is injectable for tests and honours `SOURCE_DATE_EPOCH` like Sphinx's `format_date`).
- [ ] **Step 5: Run** — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(transforms): substitutions and the default substitutions`.

### Task 9: Hyperlink transforms and dangling references

**Files:**
- Modify: `src/transforms/references.rs` (AnonymousHyperlinks 440, IndirectHyperlinks 460, ExternalTargets 640, InternalTargets 660, SphinxDanglingReferences 850), `tools/gen_sphinx_fixture.py` (family `tx_links`)
- Research: transforms.md §3.3, §9.3 (loose-message location)

- [ ] **Step 1: Add oracle cases:** `tx_links.named_external`, `…named_internal`, `…anonymous_pair`, `…anonymous_mismatch` (record with no line: `index.rst:: ERROR`… — use the probed location), `…indirect_chain`, `…indirect_circular`, `…indirect_unknown`, `…embedded_uri`, `…duplicate_target_reference`, `…dangling_reference` (ERROR + `problematic`; INFO for implicit names suppressed), `…name_nbsp`, `…anonymous_mismatch_in_nested_directive` (the end-inside-nested-content location; if not reproducible, ledger it in the report and exclude the case with its probe); regenerate.
- [ ] **Step 2: Failing comparisons.**
- [ ] **Step 3: Run** — FAIL.
- [ ] **Step 4: Implement** per §3.3/§9.3.
- [ ] **Step 5: Run** — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(transforms): anonymous, indirect, external and internal hyperlinks`.

### Task 10: Footnotes and citations (read side)

**Files:**
- Create: `src/transforms/footnotes.rs` (docutils Footnotes 620, citation transforms 619 — `citation[docname]`, the duplicate-citation warning with its Sphinx text and category, `citation_reference` → `pending_xref(refdomain='citation', reftype='ref', refwarn=True)` + `inline('[X]')`, UnreferencedFootnotesDetector 622, FootnoteDocnameUpdater 700), `tools/gen_sphinx_fixture.py` (family `tx_footnotes`)
- Research: transforms.md §3.4

- [ ] **Step 1: Add oracle cases:** `tx_footnotes.auto_numbered`, `…auto_named`, `…manual`, `…symbol`, `…mixed_order`, `…too_many_references` (error text), `…unreferenced` (warning text), `…citation_definition_and_reference`, `…citation_duplicate`, `…label_nbsp`; regenerate.
- [ ] **Step 2: Failing comparisons.**
- [ ] **Step 3: Run** — FAIL.
- [ ] **Step 4: Implement** per §3.4 (citation *resolution* stays sub-project 2; the `pending_xref` stays unresolved here).
- [ ] **Step 5: Run** — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(transforms): footnotes and the read side of citations`.

### Task 11: DocInfo and metadata removal

**Files:**
- Create: `src/transforms/frontmatter.rs` (docutils DocInfo 340 over the `doctitle_xform=False` tree; Sphinx's MetadataCollector removal of the docinfo node), Modify: `src/env/metadata.rs` (read metadata from the docinfo shape; `tocdepth` parsed as an int, 0 on failure; correct the wrong "known gap" note at `:21-32`), `tools/gen_sphinx_fixture.py` (family `tx_docinfo`), `tests/env_differential.rs` (remove `orphan_doc/orphan`)
- Research: transforms.md §3.5

- [ ] **Step 1: Add oracle cases:** `tx_docinfo.orphan`, `…tocdepth`, `…nocomments_nosearch`, `…bibliographic_fields` (author, version, …), `…not_leading` (a field list after a paragraph stays), `…tocdepth_not_an_int`; regenerate.
- [ ] **Step 2: Failing comparisons** (+ the env exemption removal).
- [ ] **Step 3: Run** — FAIL.
- [ ] **Step 4: Implement** per §3.5. If the environment's metadata shape changes (e.g. `tocdepth` stored as an int), bump `ENV_VERSION` (`src/env/mod.rs`) in this commit and add `a_version_3_environment_is_a_cold_start` beside the existing env-version test.
- [ ] **Step 5: Run** — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(transforms): DocInfo and Sphinx's metadata removal`.

### Task 12: Transitions, AutoNumbering, code blocks, doctest, and parse-time parity items

**Files:**
- Modify: `src/transforms/misc.rs` (Transitions 830 with its warnings; AutoNumbering 210 replacing the parse-time approximation in `src/rst/block.rs` for captioned literalinclude; HandleCodeBlocks 210; DoctestTransform 500), `src/rst/block.rs` (toctree `rawentries` = the explicit entry titles, `rawcaption`; the code-block/`::` default language from `highlight_language` instead of the literal `"default"`), `src/rst/mod.rs` (`ParseOptions.highlight_language: String`, default `"default"`), `tools/gen_sphinx_fixture.py` (family `tx_misc`)
- Research: transforms.md §3.6, §4, §1.2 rows 010-034 and 210-025

- [ ] **Step 1: Add oracle cases:** `tx_misc.transition_at_start`, `…transition_at_end`, `…consecutive_transitions`, `…transition_in_section`, `…figure_autonumbered_id`, `…table_autonumbered_id`, `…code_block_caption_autonumbered_id`, `…doctest_block_class`, `…blockquote_of_doctests_unwrapped`, `…toctree_rawentries_rawcaption`, `…highlight_language_default_from_config` (conf `highlight_language='python'`); regenerate.
- [ ] **Step 2: Failing comparisons.**
- [ ] **Step 3: Run** — FAIL.
- [ ] **Step 4: Implement.**
- [ ] **Step 5: Run** — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(transforms): transitions, auto-numbered ids, doctest blocks and toctree raw attributes`.

### Task 13: The escape side channel on text nodes

**Files:**
- Modify: `src/doctree/mod.rs` (`Node.escapes: Vec<u32>` — byte offsets, into `text`, of characters docutils null-escaped; empty for elements and unescaped text; the 21 `Node { … }` literals updated), `src/rst/inline.rs` (the escape2null/unescape path at Text emission fills it), `src/doctree/pformat.rs` (unchanged output — test only), `src/builder.rs` (`DOCTREE_FORMAT_VERSION` 3 → 4: the persisted shape changes)
- Test: unit tests in `src/rst/inline.rs`, `tests/doctree_serde.rs`

**Interfaces:**
- Produces: `Node::text_node_escaped(text: impl Into<String>, escapes: Vec<u32>, span: Span) -> Node`; `Node.escapes` read by Task 14.

- [ ] **Step 1: Write the failing tests:** `escaped_quote_offsets_are_recorded` (`\"a\"` → text `"a"`, escapes `[0]`), `escaped_dashes_and_ellipsis_offsets_are_recorded` (`a\--b`, `a\...`), `pformat_and_astext_ignore_escapes` (byte-identical output to a node without escapes), `escapes_roundtrip_through_bincode`, `an_escape_free_document_has_no_escape_offsets`.
- [ ] **Step 2: Run** `cargo test --locked --lib rst::inline doctree:: --test doctree_serde` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS; full-suite constraint commands (both parse oracles unchanged).
- [ ] **Step 5: Commit** `feat(doctree): keep docutils' escape information beside text nodes`.

### Task 14: SmartQuotes

**Files:**
- Create: `tools/gen_smartquotes_tables.py` (generates `src/transforms/smartquotes_tables.rs` from docutils 0.22.4's `smartquotes.smartchars` quote tables; header with the regen command and `cargo fmt --all`), `src/transforms/smartquotes.rs` (SphinxSmartQuotes 750: docutils `SmartQuotes.apply` + Sphinx's `get_tokens`, `smartquotes_action`, `smartquotes_excludes` languages/builders, `is_smartquotable`, FixedTextElement/Special/`support_smartquotes=False`/literal/raw exclusions, the `No smart quotes defined for language "xx".` warning), Modify: `tools/gen_sphinx_fixture.py` (family `sq`, run with `smartquotes=True`), `tools/gen_env_fixture.py` (project `smartquotes_default`: titles, a `:ref:` label text, a glossary term, a field name), `tests/env_differential.rs` (`KNOWN_INERT_CONF` loses `smartquotes`)
- Research: transforms.md §5

- [ ] **Step 1: Generate the tables**; add oracle cases `sq.en_quotes_dashes_ellipsis`, `sq.escaped_quote_stays_straight`, `sq.escaped_dashes`, `sq.literal_untouched`, `sq.title_and_rubric`, `sq.de_quotes`, `sq.fr_nbsp`, `sq.ja_excluded`, `sq.action_q_only` (conf `smartquotes_action='q'`), `sq.unknown_language` (the warning), `sq.nbsp_neighbours`; regenerate both fixtures.
- [ ] **Step 2: Failing comparisons** (+ the `KNOWN_INERT_CONF` removal).
- [ ] **Step 3: Run** — FAIL.
- [ ] **Step 4: Implement** per §5.
- [ ] **Step 5: Run** — PASS; full-suite constraint commands.
- [ ] **Step 6: Commit** `feat(transforms): SmartQuotes at Sphinx's defaults`.

### Task 15: Totality sweep, exemption arithmetic and docs

**Files:**
- Modify: `tests/rst_proptest.rs` (every existing generator also runs `parse_and_transform`; `transforms_survive_the_deep_nesting_sweep`), `tests/env_differential.rs` (arithmetic numbers final), `ROADMAP.md` §2, `docs/IMPLEMENTATION_STATUS.md` (pipeline rows, the "Transforms not yet run" list rewritten from the research gap table, the include/reporter known limitation removed, validator row per D1, test counts), `CHANGELOG.md` (`[Unreleased]`: the reporter channel, D1 removals and D2 exit policy as user-visible changes, the transforms, SmartQuotes, the new config keys, the cold first build after upgrading)

- [ ] **Step 1: Write the failing tests** (the new proptest bodies) and run `PROPTEST_CASES=2048 cargo test --locked --test rst_proptest` — FAIL until the sweep compiles and runs; fix any panic found by adding its minimal input as a named regression test first.
- [ ] **Step 2: Run** the whole constraint set; record the final `cargo test` counts per target.
- [ ] **Step 3: Update the three docs** with the recorded counts and exemption figures (every number from a command you ran).
- [ ] **Step 4: Commit** `docs: record M2 wave 5 sub-project 1 (reporter channel, read transforms, SmartQuotes)`.
