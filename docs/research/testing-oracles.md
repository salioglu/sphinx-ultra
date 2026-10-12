# Testing Oracles Research

## Finding

There is no single exhaustive upstream golden corpus for this Sphinx-Ultra worktree. The repository has complementary corpora, and document-shaped inputs must be promoted through real Sphinx HTML builds wherever possible.

Verified local evidence:

| Existing source | Evidence |
| --- | --- |
| tests/fixtures/doctree_differential.json and tools/gen_doctree_fixture.py | 735 Docutils differential cases |
| tests/fixtures/sphinx_doctree_differential.json and tools/gen_sphinx_fixture.py | 489 Sphinx read-phase snippet cases |
| tests/fixtures/env_differential.json and tools/gen_env_fixture.py | 29 environment projects containing 84 documents |
| tests/fixtures/basic, basic_missing_ref, deps_image, intersphinx, literalinclude, toctree_forms, toctree_glob | 7 checked-in HTML-ish fixture projects |
| SPHINX_PROJECTS in tools/gen_inventory_fixture.py | 4 Sphinx-built inventory projects; handcrafted .inv files remain parser fixtures |
| tests/fixtures/pattern_differential.json and tools/gen_pattern_fixture.py | 881 parser-only pattern cases, outside the HTML oracle because they are not documents |
| C:\Users\johnm\Documents\repos\sphinx-needs\packages\sphinx-needs\tests\doc_test | 142 local sibling doc_test projects with conf.py |

The sibling checkout was verified at sphinx-needs 8.5.0, commit 58bcb59d861da95f2aca79f343e8bae6ec5c1250, with subtree tree 958172a89defcec69704f6b9d61e482e7c4e8409. Its importable package is packages/sphinx-needs/src/sphinx_needs and its documentation projects are packages/sphinx-needs/tests/doc_test. The checkout has unrelated root-level dirt, but packages/sphinx-needs is clean; generation must reject dirt inside that subtree.

## Reference profiles

- core is exactly Sphinx 9.1.0 with Docutils 0.22.4 and its own committed uv.lock.
- local_needs is exactly Sphinx 9.1.0 with Docutils 0.21.2 and its own committed uv.lock. Its locked dependencies include sphinx-needs 8.5.0 and pytest. The local source is selected by --needs-root or SPHINX_NEEDS_ROOT, prepended to the child PYTHONPATH, and verified inside the child against the import path, version, commit, subtree tree, and clean package-subtree status.
- Lock creation may resolve packages through the package index. Reference generation uses uv run --locked and a child socket guard; no build requires a network service.

## Corpus policy

Each in-scope source case appears exactly once in index.json. The ledger has only built, build-error, reference-crash, excluded-network, and excluded-plantuml statuses.

A local-needs case is one project directory, not one record per originating test invocation. Its origin contains all statically discovered pytest node IDs that reference that project and a variants_not_captured flag when the originating scope uses confoverrides or a non-HTML builder. The scanner does not import tests or extract assertion data. The project is built from its own conf.py with builder html, and Ultra runs against the resulting case when the reference status is runnable.

The 735 Docutils and 489 Sphinx snippets become one-document projects using extensions=[], master_doc='index', exclude_patterns=['_build'], smartquotes=False, and keep_warnings=True. Environment projects and the seven existing HTML projects are rebuilt with the real HTML builder. Inventory cases come from the four Sphinx-built projects in tools/gen_inventory_fixture.py.

The 881 parser-only pattern cases are explicitly outside this document oracle. They are not silently counted as missing; the source-set contract excludes them because they cannot create an HTML project.

## Comparison, storage, and diagnostics

Only CRLF-to-LF text normalization is permitted, plus replacement of the absolute source root with <SRCDIR> in the warnings stream. searchindex.js is compared as parsed JSON with object key order ignored and array order preserved. objects.inv is compared by its exact header and canonical decoded records. .buildinfo and all other opaque bytes are exact.

Reference generation is child-process only. The child installs guards for socket.socket.connect, socket.create_connection, and socket.getaddrinfo before calling sphinx.cmd.build.main. Static _static and _images bytes are content-addressed in blobs by SHA-256 while every logical output path remains in the ledger. NOTICE.md has one licensing row per source set and one row for Sphinx and alabaster theme assets; licensing is not stored per file.

The default Rust suite compares Ultra-to-Ultra for deterministic process and comparator checks. The ignored exhaustive suite invokes CARGO_BIN_EXE_sphinx-ultra with cache directories outside output directories, bounded output pipes, timeouts, deterministic case ordering, complete source-set coverage, and a report containing every mismatch. Its assertion output is capped at 64 KiB and points to the complete Markdown and JSON reports.

## Acceptance checks

1. Source discovery enforces at least 735 Docutils cases, at least 489 Sphinx cases, exactly 29 environment projects, at least 84 environment documents, exactly 7 HTML projects, exactly 4 inventory projects, and exactly 142 local-needs projects.
2. Discovered source keys equal ledger keys with no duplicates or unreferenced files.
3. Profile records contain versions, lock SHA-256, and local-needs commit and subtree provenance.
4. Generation is atomic, deterministic across different absolute roots, and rejects absolute-root bytes outside warnings.
5. The default cargo test suite remains green.
6. One ignored exhaustive differential test runs every built and build-error case, aggregates all mismatch categories, and exits nonzero while Ultra is incomplete.
