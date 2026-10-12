# HTML Output Artifacts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the live HTML build emit Sphinx 9.1.0-compatible `searchindex.js`, `objects.inv`, and `.buildinfo` artifacts.

**Architecture:** Keep the existing `SphinxBuilder` pipeline as the production entry point and add small serialization/collection helpers beside the existing search and inventory modules. The search writer freezes document, title, index, and supported domain-object data into Sphinx's compact JSON schema; the inventory writer reuses `InventoryFile::dump` with deterministic `std` and `py` object lists; the buildinfo writer emits Sphinx's four-line text framing and implements `stable_hash` for Ultra's modeled HTML configuration values, with unsupported values documented as gaps.

**Tech Stack:** Rust, Tokio filesystem APIs, serde_json, flate2, existing `BuildEnvironment` domain registries, Sphinx 9.1.0 reference outputs.

---

### Task 1: Search-index contract

**Files:**
- Modify: `tests/e2e_cli.rs`
- Modify: `src/search.rs`
- Modify: `src/builder.rs`
- Modify: `src/html_builder.rs`

- [x] **Step 1: Write the failing e2e assertion**

Add an e2e test for `tests/fixtures/basic` that reads `searchindex.js`, asserts the exact `Search.setIndex(` prefix and `)` suffix, parses the inner JSON, and checks the Sphinx 9.1 key set plus the reference values for `docnames`, `filenames`, `titles`, `alltitles`, `terms`, `titleterms`, and empty object/index tables. The test must also assert that the value is not plain JSON.

- [x] **Step 2: Run only that test and verify the expected failure**

Run `cargo test --test e2e_cli build_emits_sphinx_search_index -- --exact --nocapture`. It should fail because the current build leaves `searchindex.js` absent; if it fails for another reason, correct the test setup before implementing.

- [x] **Step 3: Implement the Sphinx freeze shape**

Extend the search serialization so the emitted object contains, in Sphinx's schema, `alltitles`, `docnames`, `envversion`, `filenames`, `indexentries`, `objects`, `objnames`, `objtypes`, `terms`, `titles`, and `titleterms`. Use Sphinx 9.1's compact JSON separators and `Search.setIndex(<json>)`; collapse single-document term postings to an integer and multi-document postings to sorted integer arrays. Use the supported document titles, title anchors, index records, `std`/`py` search objects, and the Sphinx 9.1 env-version map already represented by Ultra's supported domains. Preserve deterministic document and map ordering.

- [x] **Step 4: Wire the live builder and direct HTMLBuilder writer**

Pass the build's processed documents/doctrees and environment data into the search freeze helper from `SphinxBuilder::generate_search_index`, write `searchindex.js` atomically or directly after successful serialization, and update `HTMLBuilder::dump_search_index` to use the same wrapper/schema instead of its placeholder plain JSON.

- [x] **Step 5: Run the focused test and commit**

Run `cargo test --test e2e_cli build_emits_sphinx_search_index -- --exact --nocapture` and `cargo test search:: --lib`. Commit as `feat: emit sphinx search index`.

### Task 2: Object inventory emission

**Files:**
- Modify: `tests/e2e_cli.rs`
- Modify: `src/builder.rs`
- Modify: `src/inventory.rs` only if the integration exposes a missing writer contract
- Modify: `docs/IMPLEMENTATION_STATUS.md`

- [x] **Step 1: Write the failing inventory assertion**

Add an e2e test for the `basic` fixture that reads `objects.inv`, asserts the four exact Sphinx header lines, decompresses the zlib tail with `flate2`, and compares the sorted payload records to Sphinx 9.1's basic reference records: the document record plus `genindex`, `modindex`, `py-modindex`, and `search` labels. Also assert that `InventoryFile::loads` can parse the emitted bytes.

- [x] **Step 2: Run only that test and verify the expected failure**

Run `cargo test --test e2e_cli build_emits_sphinx_object_inventory -- --exact --nocapture`. It should fail because the live build currently does not create `objects.inv`.

- [x] **Step 3: Collect supported std/py domain objects**

Add a builder helper that converts `BuildEnvironment::all_docs`, titles, `std.labels`, `std.objects`, `std.progoptions`, `std.terms`, `py.modules`, and non-aliased `py.objects` into `InvObject` vectors. Match Sphinx priorities, anchors, display names, virtual labels, document records, and `std`/`py` domain ordering. Use `InventoryFile::dump` with `get_target_uri(docname)` so its existing Sphinx-compatible sorting, `$` compaction, headers, and zlib payload framing remain authoritative.

- [x] **Step 4: Wire inventory dumping into the HTML build**

Call the collection/dump helper after the environment has been resolved and before the build returns, writing `<outdir>/objects.inv` for every HTML build. Sphinx 9.1 omits the standard domain's `term` records and keeps aliased Python objects at priority `-1`; the existing collector/writer matches both details. Update the status note to describe the live call sites and any intentionally unsupported domains.

- [x] **Step 5: Run focused inventory tests and commit**

Run `cargo test --test e2e_cli build_emits_sphinx_object_inventory -- --exact --nocapture`, `cargo test --test inventory_roundtrip`, and the relevant env unit tests. Commit as `feat: emit sphinx object inventory`.

### Task 3: Sphinx buildinfo framing

**Files:**
- Modify: `tests/e2e_cli.rs`
- Modify: `src/builder.rs`
- Modify: `src/html_builder.rs`
- Modify: `docs/IMPLEMENTATION_STATUS.md`

- [x] **Step 1: Write the failing buildinfo assertion**

Add an e2e test for the `basic` fixture that compares `.buildinfo`'s first two lines exactly with Sphinx 9.1, requires `config: ` and `tags: ` lines containing lowercase hexadecimal hashes of the Sphinx field width, and requires the final blank line. It must reject the current JSON object.

- [x] **Step 2: Run only that test and verify the expected failure**

Run `cargo test --test e2e_cli build_emits_sphinx_buildinfo -- --exact --nocapture`. It should fail because the current file is JSON.

- [x] **Step 3: Implement text output with an explicit hash gap**

Emit the exact Sphinx 9.1 four-line layout and trailing newline. Implement the Python `stable_hash` algorithm for the HTML configuration values modeled by Ultra, including the built-in HTML tags and user tags. Add code comments for HTML-category settings that Ultra cannot represent; those custom values remain the documented config-hash gap.

- [x] **Step 4: Wire both production paths and commit**

Call the buildinfo writer from the live `SphinxBuilder` HTML build and update `HTMLBuilder::write_build_info` to the same text format. Update the status note to describe the exact layout, modeled-value parity, and deliberate config-hash gap.

- [x] **Step 5: Run focused test and commit**

Run `cargo test --test e2e_cli build_emits_sphinx_buildinfo -- --exact --nocapture` and the related HTML builder tests. Commit as `feat: emit sphinx build info`.

### Task 4: Final verification

**Files:**
- No new production files; update the plan checkboxes and status note if needed.

- [x] **Step 1: Run formatting**

Run `cargo fmt --all -- --check`.

- [x] **Step 2: Run linting**

Run `cargo clippy --all-targets --all-features -- -D warnings`.

- [x] **Step 3: Run the complete test suite**

Run `cargo test` and record the result, including any pre-existing benchmark/all-target limitation if it is outside this command.

- [x] **Step 4: Review commits and working tree**

Run `git log --oneline main..HEAD`, `git diff main..HEAD --check`, and `git status --short`; confirm there is one implementation commit per artifact, no push/merge occurred, and the final report calls out the config hash gap plus any oracle command blocked by the sandbox.
