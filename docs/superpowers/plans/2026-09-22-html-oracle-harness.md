# HTML Oracle Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

## Goal

Create one comprehensive first PR that discovers the complete in-scope corpus, builds committed references with real Sphinx HTML builds, and runs the actual sphinx-ultra CLI against every runnable case. The default cargo test suite stays green. One ignored exhaustive test runs the whole runnable set, aggregates every mismatch, writes target/html-oracle/report.md and target/html-oracle/report.json, and exits nonzero while Ultra is incomplete.

This is one first PR because the acceptance contract is end-to-end: committed references, deterministic generation, a runnable comparator, and a complete known-red report. It is divided into bounded commits so each schema, discovery, runner, storage, comparator, process, and report task is independently reviewable.

No production or test implementation is written until this plan is executed.
The implementation PR changes only tools/, tests/, tests/fixtures/html_oracle/,
and NOTICE.md; nothing under src/ changes.

## Architecture

Reference generation is Python and has three layers:

1. Discovery reads the existing JSON fixtures and project directories, materializes each document-shaped input as an HTML project, and records provenance.
2. A child runner installs deterministic UUID and network shims before importing Sphinx, then calls sphinx.cmd.build.main(argv) with the pinned Sphinx environment and Sphinx -q -w warning-file options.
3. The generator captures output, warnings, status, hashes, and static assets into an atomic fixture tree.

Rust tests have three layers:

1. tests/support/html_oracle.rs loads and validates index.json, reconstructs logical trees, applies the fixed path policy, and compares outputs.
2. tests/support/diagnostics.rs executes child processes with bounded pipes and deadlines.
3. tests/support/html_oracle.rs also produces reporting-only diagnostics: retained side-by-side trees, HTML body/chrome localization, keyed needs.json differences, structured searchindex.js and objects.inv differences, warning line diffs, and first-divergence groups.
4. tests/html_differential.rs invokes env!("CARGO_BIN_EXE_sphinx-ultra") for every runnable ledger case, then writes the aggregate report.

Required implementation paths:

~~~text
tools/gen_html_oracle.py
tools/html_oracle_runner.py
tools/html_oracle_cases.toml
tools/oracle_profiles/core/pyproject.toml
tools/oracle_profiles/core/uv.lock
tools/oracle_profiles/local_needs/pyproject.toml
tools/oracle_profiles/local_needs/uv.lock
tools/test_gen_html_oracle.py
tests/html_differential.rs
tests/support/html_oracle.rs
tests/support/diagnostics.rs
tests/fixtures/html_oracle/core/index.json
tests/fixtures/html_oracle/core/inputs
tests/fixtures/html_oracle/core/refs
tests/fixtures/html_oracle/core/blobs
tests/fixtures/html_oracle/local_needs/index.json
tests/fixtures/html_oracle/local_needs/inputs
tests/fixtures/html_oracle/local_needs/refs
tests/fixtures/html_oracle/local_needs/blobs
tests/fixtures/html_oracle/NOTICE.md
~~~

Each profile owns tests/fixtures/html_oracle/profile/index.json, inputs, refs, and blobs. Each case directory is tests/fixtures/html_oracle/profile/inputs/source_set/case_id and tests/fixtures/html_oracle/profile/refs/source_set/case_id. Sanitize case IDs to A-Za-z0-9_.-, limit the sanitized portion to 48 characters, and append a hyphen plus the first eight hex characters of sha256 of the unsanitized identifier on truncation or collision.

### Corpus and scope

The verified source counts are:

| Source set | Profile | Source | Contract |
| --- | --- | --- | --- |
| docutils_snippets | core | tests/fixtures/doctree_differential.json | at least 735 one-document projects |
| sphinx_read_snippets | core | tests/fixtures/sphinx_doctree_differential.json | at least 489 one-document projects |
| environment_projects | core | tests/fixtures/env_differential.json | exactly 29 projects and at least 84 documents |
| html_projects | core | seven named directories in tests/fixtures | exactly 7 projects |
| inventory_projects | core | SPHINX_PROJECTS in tools/gen_inventory_fixture.py | exactly 4 Sphinx-built projects |
| sphinx_needs_doc_tests | local_needs | packages/sphinx-needs/tests/doc_test | exactly 142 directories containing conf.py |

The 881 records in tests/fixtures/pattern_differential.json are parser-only pattern cases, not documents, and are outside the HTML oracle. They are not materialized or ledgered. The handcrafted .inv files in tests/fixtures/inventories remain parser fixtures; only the four Sphinx-built projects from SPHINX_PROJECTS become cases.

The seven existing HTML-ish directories are basic, basic_missing_ref, deps_image, intersphinx, literalinclude, toctree_forms, and toctree_glob. They are read-only inputs. The Docutils, Sphinx read-phase, and environment JSON fixtures are also read-only.

For every discovered source key, the generator emits exactly one CaseRecord in
exactly one selected-profile index. Discovery keys and ledger keys must be
equal after deterministic sorting; duplicates, omissions, and extras are
generation errors.

Each Docutils or Sphinx snippet becomes an index.rst plus the base conf.py
convention from tools/gen_sphinx_fixture.py:

~~~python
project = "html-oracle"
extensions = []
master_doc = "index"
exclude_patterns = ["_build"]
~~~

Apply `smartquotes=False` and `keep_warnings=True` as the same explicit
configuration overrides used by that generator. Environment projects use the
same effective overrides, while inventory projects follow the
`BASE_CONFOVERRIDES` and `CONF_PY_TEMPLATE` conventions in
tools/gen_inventory_fixture.py.

Environment projects are reconstructed with all documents and built with the real html builder. Existing HTML projects keep their own conf.py. Inventory projects use the real html builder and the conventions in tools/gen_inventory_fixture.py.

Local-needs discovery is restricted to needs-root/packages/sphinx-needs/tests/doc_test. The checked sibling checkout has source at packages/sphinx-needs/src/sphinx_needs and exactly 142 direct doc_test directories with conf.py. One case is one project. The ledger records every statically found pytest node ID referencing that project and sets variants_not_captured when a matching test scope contains confoverrides or a non-html builder. The scanner reads AST and source paths only; it does not import tests or extract assertions.

### Status model

Use exactly these statuses:

| Status | Reference handling | Ultra handling |
| --- | --- | --- |
| built | exit code 0; capture complete tree | run and compare complete tree |
| build-error | nonzero exit without a Python traceback; capture warnings and partial tree | run and require build-error class, warnings, and files |
| reference-crash | traceback; capture record and do not run Ultra | not scheduled |
| excluded-network | static remote fetch requirement | not scheduled |
| excluded-plantuml | static conf.py load of sphinxcontrib.plantuml | not scheduled |

Excluded records have null exit_code, empty warnings, a required excluded_reason, and no files. A traceback is the exact marker Traceback (most recent call last): in combined child output. A nonzero result without that marker is build-error. A build-error is the supported cannot-generate reference case and is still run through Ultra.

Local-needs cases are built with their own conf.py, committed, and sent to
Ultra whenever their reference status is built or build-error. The plan does
not emulate native sphinx-needs directives; differences caused by Ultra's
current handling remain known-red results.

The runner installs uuid.uuid4 = uuid.UUID(int=n, version=4), with n starting at 1 for each child process, before importing Sphinx. The exact shim list is recorded in each profile record as uuid.uuid4=counter. Every local-needs build also passes -D needs_reproducible_json=1 and records needs_reproducible_json=1 in its profile shim list. This makes repeated reference builds comparable; Ultra ID differences remain visible mismatches.

### Fixed per-path comparison policy

TOML has no normalization settings. Both Python and Rust implement this exact table:

| Path or stream | Policy |
| --- | --- |
| warnings stream | UTF-8 with replacement, CRLF to LF, replace the absolute source root with <SRCDIR>, exact text comparison |
| searchindex.js | CRLF to LF, remove Search.setIndex( and the final );, parse JSON, compare values with object-key order ignored and array order preserved |
| needs.json | Parse JSON, compare values with object-key order ignored and array order preserved |
| objects.inv | exact four-line header, zlib-decode records, parse name/domain-role/priority/URI/display name, sort by all five fields, compare header plus canonical record list |
| *.buildinfo | exact bytes |
| *.html, _sources/**, *.css, *.js except searchindex.js, *.json except needs.json, *.xml, *.txt | CRLF to LF, then exact bytes |
| every other path, including images | exact bytes |

There is no HTML DOM rewrite or field-specific normalizer. Source-root replacement is permitted only in warnings. The generator fails if an absolute source, build, or cache root occurs in captured reference bytes outside warnings.
Normalization is applied only at the named field or logical-path boundary;
stored bytes and SHA-256 values remain the captured bytes.

### Storage and determinism

HTML, searchindex.js, objects.inv, .buildinfo, _sources, and other non-static outputs are stored under the profile refs directory. Every logical path beginning _static/ or _images/ is stored under that profile's blobs directory using its content SHA-256 as the filename. The ledger retains every logical path, so deduplicating bytes never removes logical output.

For sorted relative path and content digest pairs, use:

~~~python
def canonical_hash(entries: list[tuple[str, str]]) -> str:
    payload = "".join(
        f"{path}\0{digest}\n"
        for path, digest in sorted(entries)
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()
~~~

Use this formula for input_sha256 and tree_sha256. Generation uses a process pool controlled by -j N, default os.cpu_count(), but collects and serializes results by profile, source_set, case_id, then logical path.

Each profile owns a subtree. Generate one selected profile into its sibling staging directory (`core.staging` or `local_needs.staging`), validate that complete profile index, inputs, refs, and blobs, rename the old selected profile directory to `core.old` or `local_needs.old`, rename staging to the selected profile directory, and remove the matching `.old` directory only after the new tree is visible. A failure before the second rename preserves that profile and never touches the other profile. Run one failure-injection test.

## Tech Stack

Python 3.12 stdlib, Sphinx 9.1.0, Docutils 0.22.4 for core, Docutils 0.21.2 for local_needs, sphinx-needs 8.5.0 loaded by explicit PYTHONPATH, and uv locked projects. Lock creation may resolve packages through the package index; generation uses uv run --locked and the child socket guard.

Rust uses serde, serde_json, sha2, tempfile, std::process, std::thread, std::sync, and std::time. Add sha2 as a direct Cargo dependency for exact SHA-256. Do not add a runtime network service or OS sandbox.

## Full index.json schema

The generator writes sorted UTF-8 JSON at tests/fixtures/html_oracle/profile/index.json. Each profile index is complete for that profile; Rust merges the two indexes in memory for exhaustive execution. These Python types are the complete writer schema:

~~~python
from typing import Literal, TypedDict

CaseStatus = Literal[
    "built",
    "build-error",
    "reference-crash",
    "excluded-network",
    "excluded-plantuml",
]
FileStorage = Literal["input", "ref", "blob"]

class ProfileRecord(TypedDict):
    sphinx: str
    docutils: str
    needs_version: str | None
    needs_commit: str | None
    needs_tree: str | None
    lock_path: str
    lock_sha256: str
    determinism_shims: list[str]

class OriginRecord(TypedDict):
    source_set: str
    origin_path: str
    pytest_node_ids: list[str]
    variants_not_captured: bool

class FileRecord(TypedDict):
    logical_path: str
    storage: FileStorage
    storage_path: str
    sha256: str
    size: int

class CaseRecord(TypedDict):
    profile: str
    source_set: str
    case_id: str
    status: CaseStatus
    exit_code: int | None
    warnings: str
    excluded_reason: str | None
    origin: OriginRecord
    input_files: list[FileRecord]
    input_sha256: str
    tree_sha256: str
    files: list[FileRecord]
    needs_json: FileRecord | None
    needs_status: CaseStatus | None
    needs_exit_code: int | None
    needs_warnings: str | None

class IndexDocument(TypedDict):
    schema_version: int
    generator: str
    profiles: dict[str, ProfileRecord]
    cases: list[CaseRecord]
~~~

The complete JSON shape is:

~~~json
{
  "schema_version": 1,
  "generator": "html-oracle/1",
  "profiles": {
    "core": {
      "sphinx": "9.1.0",
      "docutils": "0.22.4",
      "needs_version": null,
      "needs_commit": null,
      "needs_tree": null,
      "lock_path": "tools/oracle_profiles/core/uv.lock",
      "lock_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "determinism_shims": ["uuid.uuid4=counter"]
    },
    "local_needs": {
      "sphinx": "9.1.0",
      "docutils": "0.21.2",
      "needs_version": "8.5.0",
      "needs_commit": "58bcb59d861da95f2aca79f343e8bae6ec5c1250",
      "needs_tree": "958172a89defcec69704f6b9d61e482e7c4e8409",
      "lock_path": "tools/oracle_profiles/local_needs/uv.lock",
      "lock_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "determinism_shims": ["uuid.uuid4=counter", "needs_reproducible_json=1"]
    }
  },
  "cases": [
    {
      "profile": "core",
      "source_set": "docutils_snippets",
      "case_id": "docutils-0001",
      "status": "built",
      "exit_code": 0,
      "warnings": "",
      "excluded_reason": null,
      "origin": {
        "source_set": "docutils_snippets",
        "origin_path": "tests/fixtures/doctree_differential.json[0]",
        "pytest_node_ids": [],
        "variants_not_captured": false
      },
      "input_files": [
        {
          "logical_path": "index.rst",
          "storage": "input",
          "storage_path": "inputs/docutils_snippets/docutils-0001/index.rst",
          "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
          "size": 128
        }
      ],
      "input_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "tree_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "files": [
        {
          "logical_path": "index.html",
          "storage": "ref",
          "storage_path": "refs/docutils_snippets/docutils-0001/index.html",
          "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
          "size": 1032
        }
      ],
      "needs_json": null,
      "needs_status": null,
      "needs_exit_code": null,
      "needs_warnings": null
    }
  ]
}
~~~

The Rust serde model is:

~~~rust
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexDocument {
    pub schema_version: u32,
    pub generator: String,
    pub profiles: BTreeMap<String, ProfileRecord>,
    pub cases: Vec<CaseRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRecord {
    pub sphinx: String,
    pub docutils: String,
    pub needs_version: Option<String>,
    pub needs_commit: Option<String>,
    pub needs_tree: Option<String>,
    pub lock_path: String,
    pub lock_sha256: String,
    pub determinism_shims: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginRecord {
    pub source_set: String,
    pub origin_path: String,
    pub pytest_node_ids: Vec<String>,
    pub variants_not_captured: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStorage {
    Input,
    Ref,
    Blob,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    pub logical_path: String,
    pub storage: FileStorage,
    pub storage_path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseStatus {
    Built,
    BuildError,
    ReferenceCrash,
    ExcludedNetwork,
    ExcludedPlantuml,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseRecord {
    pub profile: String,
    pub source_set: String,
    pub case_id: String,
    pub status: CaseStatus,
    pub exit_code: Option<i32>,
    pub warnings: String,
    pub excluded_reason: Option<String>,
    pub origin: OriginRecord,
    pub input_files: Vec<FileRecord>,
    pub input_sha256: String,
    pub tree_sha256: String,
    pub files: Vec<FileRecord>,
    pub needs_json: Option<FileRecord>,
    pub needs_status: Option<CaseStatus>,
    pub needs_exit_code: Option<i32>,
    pub needs_warnings: Option<String>,
}
~~~

Validation rejects unknown fields, missing fields, unknown statuses, absolute paths, drive prefixes, parent components, symlinks, wrong hashes, wrong sizes, duplicate case keys, unsorted case keys, unknown profiles, and status-specific nullability errors. Every input file is represented by input_files and every output file by files. Core cases require all four needs fields to be null. Local-needs cases with HTML status built or build-error require the second-build fields to be populated consistently. Every profile file reference must resolve below its selected profile directory, either tests/fixtures/html_oracle/core or tests/fixtures/html_oracle/local_needs.

## TDD implementation tasks

### Task 1: Profiles, configuration, and schema

- [ ] Write red tests in tools/test_gen_html_oracle.py for unknown status, missing input_files, missing input_sha256, path escape, hash mismatch, duplicate key, null exit_code for built, and files on an excluded case.
- [ ] Create tools/oracle_profiles/core/pyproject.toml with Sphinx 9.1.0, Docutils 0.22.4, and pytest 8 through 9.
- [ ] Create tools/oracle_profiles/local_needs/pyproject.toml with Sphinx 9.1.0, Docutils 0.21.2, sphinx-needs 8.5.0, and pytest 8 through 9. Do not add tool.uv.sources.
- [ ] Create tools/html_oracle_cases.toml with the six source sets and exact counts above. There is no pattern source set.
- [ ] Create both locks with uv lock --project tools/oracle_profiles/core and uv lock --project tools/oracle_profiles/local_needs. All later commands use uv run --locked.
- [ ] Implement the Python schema and validator in tools/gen_html_oracle.py using the complete types above.
- [ ] Implement the Rust serde model in tests/support/html_oracle.rs and a default schema test in tests/html_differential.rs.
- [ ] Run uv run --locked --project tools/oracle_profiles/core python -m pytest tools/test_gen_html_oracle.py -q; cargo fmt --all -- --check; cargo test --test html_differential schema. Expected green result: all schema tests pass.
- [ ] Commit test: define HTML oracle schema and profiles.

### Task 2: Discovery, materialization, provenance, and licensing

- [ ] Write red tests for 735 Docutils cases, 489 Sphinx cases, 29 projects, 84 documents, 7 HTML projects, 4 inventory projects, 142 needs projects, duplicate detection, ledger/discovery set equality, and needs-root escape.
- [ ] Implement core discovery in tools/gen_html_oracle.py. Preserve bytes, reject symlinks, create one-document snippet projects, rebuild environment projects as HTML, copy the seven named projects, and select only SPHINX_PROJECTS.
- [ ] Implement direct-child needs discovery at packages/sphinx-needs/tests/doc_test. Scan packages/sphinx-needs/tests/**/*.py with ast.parse. Emit repository-relative node IDs from file, class, and function scopes. Detect confoverrides and non-html builder values only to set variants_not_captured. Do not import tests or inspect assertions.
- [ ] Implement static AST inspection. Mark excluded-network for a remote URL that a build would fetch. A remote intersphinx target with local inventory remains allowed. Mark excluded-plantuml when the AST finds a sphinxcontrib.plantuml entry in extensions, an import sphinxcontrib.plantuml statement, or an app.setup_extension call whose argument names sphinxcontrib.plantuml. Add plantuml_from_app_extension to the regression fixture set and assert it is excluded-plantuml. If both match, excluded-network wins.
- [ ] Create NOTICE.md with exactly one licensing row per source set and one row for Sphinx and alabaster theme assets. Use BSD-2-Clause for Docutils and Sphinx-derived sets, the repository license for checked-in fixture projects, and MIT for sphinx-needs. State that the 881 pattern records are outside this HTML corpus and that licensing is not recorded per file.
- [ ] Run uv run --locked --project tools/oracle_profiles/core python -m pytest tools/test_gen_html_oracle.py -q -k discovery. Expected green result: source floors, exact project counts, and the 142 count pass.
- [ ] Commit feat: discover complete HTML oracle corpus.

### Task 3: Child runner, provenance, and network denial

- [ ] Write red tests for a conf.py socket attempt, core versions, wrong needs import location, wrong version, wrong commit, wrong tree, dirty package subtree, and harmless dirt outside that subtree.
- [ ] Create tools/html_oracle_runner.py. Parse --profile, --sourcedir, --outputdir, --doctree-dir, --builder, --warnings-file, and --needs-root.
- [ ] Install this guard before importing Sphinx:

~~~python
import socket

def reject_network(*args, **kwargs):
    raise RuntimeError("network disabled by html oracle")

socket.socket.connect = reject_network
socket.create_connection = reject_network
socket.getaddrinfo = reject_network
~~~

- [ ] Before importing Sphinx, install the UUID shim exactly as follows:

~~~python
import itertools
import uuid

uuid_counter = itertools.count(1)
uuid.uuid4 = lambda: uuid.UUID(int=next(uuid_counter), version=4)
~~~

Record uuid.uuid4=counter in the profile record. For local_needs also append needs_reproducible_json=1 and add -D needs_reproducible_json=1 to the Sphinx argv.
- [ ] Call sphinx.cmd.build.main with argv ["-q", "-w", str(warnings_file), "-b", builder, "-d", str(doctree_dir), str(sourcedir), str(outputdir)]. The parent captures combined stdout and stderr only for traceback and status classification, sets PYTHONNOUSERSITE=1, and reads warnings only from warnings_file.
- [ ] For local_needs, prepend needs-root/packages/sphinx-needs/src to PYTHONPATH and verify inside the child that sphinx_needs.__file__ is inside that path, __version__ is 8.5.0, git HEAD is 58bcb59d861da95f2aca79f343e8bae6ec5c1250, git HEAD:packages/sphinx-needs is 958172a89defcec69704f6b9d61e482e7c4e8409, and git status --porcelain -- packages/sphinx-needs is empty. There is no bypass flag.
- [ ] Read warnings_file into the normalized warnings field using the fixed warnings policy. Keep combined stdout and stderr out of the ledger. Classify the traceback marker in combined output as reference-crash and other nonzero output as build-error.
- [ ] Run uv run --locked --project tools/oracle_profiles/core python -m pytest tools/test_gen_html_oracle.py -q -k runner. Expected green result: child network access is rejected and all provenance checks are enforced.
- [ ] Commit feat: add guarded Sphinx oracle runner.

### Task 4: Complete trees, hashes, blobs, and atomic replacement

- [ ] Write red tests for symlink rejection, parent path rejection, repeated static bytes producing one blob, complete logical paths, hash changes, absolute-root leaks, and failure injection.
- [ ] Walk each profile's input and output trees with sorted relative paths and check is_symlink before is_file. Record every input file in input_files and every captured output in files. Capture partial output for build-error and reference-crash. Store non-static output under the profile refs directory and _static and _images bytes under the profile blobs directory by content SHA-256.
- [ ] Write warnings to profile/refs/source_set/case_id/warnings.txt and validate them against the warnings field. Keep warnings outside the logical files list.
- [ ] Add reverse artifact validation: after generation, walk every file under a profile's inputs, refs, and blobs and require a matching input_files or files record with the same storage path, size, and SHA-256. Permit only the per-case warnings.txt files outside those records; the profile index.json and shared NOTICE.md are outside these walks. Fail on an unreferenced stale artifact.
- [ ] Compute input_sha256 and tree_sha256 with canonical_hash. The empty entry list is used for excluded cases.
- [ ] Stage one complete selected profile in tests/fixtures/html_oracle/core.staging or tests/fixtures/html_oracle/local_needs.staging according to `--profile`. Validate every selected-profile ledger hash, file, path, source-set key, lock digest, and root-leak rule. Rename the selected `core` or `local_needs` directory to its matching `.old` directory, rename staging to the selected profile name, and remove the `.old` directory only after the final rename. Make HTML_ORACLE_INJECT_FAILURE_AFTER=case-count fail before rename and preserve the old selected profile.
- [ ] Run uv run --locked --project tools/oracle_profiles/core python -m pytest tools/test_gen_html_oracle.py -q -k "storage or hash or atomic". Expected green result: no symlink escapes, blobs deduplicate, logical trees remain complete, and failure is recoverable.
- [ ] Commit feat: store deterministic oracle trees atomically.

### Task 5: Fixed normalizers and comparator

- [ ] Write red Rust tests for CRLF text equality, exact .buildinfo bytes, warnings-only source-root replacement, searchindex key ordering, needs.json key ordering, searchindex array ordering, malformed wrappers, canonical objects.inv records, opaque bytes, missing files, and unexpected files.
- [ ] Add Policy values Warnings, SearchIndex, NeedsJson, ObjectsInventory, TextCrlf, and ExactBytes to tests/support/html_oracle.rs. Dispatch searchindex.js and needs.json before objects.inv and general text extensions.
- [ ] Implement searchindex.js parsing of Search.setIndex( JSON ); with JSON object key order ignored, array order preserved, and scalar types exact.
- [ ] Implement needs.json parsing with the same canonical JSON value comparison as searchindex.js, with object-key order ignored, array order preserved, and scalar types exact. Read the needs_json FileRecord from the local-needs profile index and return needs-json-value or invalid-needs-json.
- [ ] Implement objects.inv parsing of four exact header lines plus zlib records into five fields sorted by the complete tuple. Use invalid-objects-inventory and objects-inventory-value categories.
- [ ] Reconstruct blob files, compare the union of logical paths, compare normalized warnings, and compare status class. built requires success; build-error requires build-error plus exact warnings and files; reference-crash and excluded records are not scheduled.
- [ ] Return missing-file, unexpected-file, bytes-value, text-value, searchindex-value, objects-inventory-value, invalid-searchindex, invalid-objects-inventory, status, warning, spawn, io, and timeout categories.
- [ ] Run cargo fmt --all and cargo test --test html_differential normalizer comparator schema. Expected green result: every table policy test passes.
- [ ] Commit feat: compare HTML oracle trees by fixed policy.

### Task 6: Reporting-only side-by-side diagnostics

Files:

- Modify: tests/support/html_oracle.rs
- Modify: tests/html_differential.rs

- [ ] Write red synthetic Rust tests in tests/html_differential.rs for `html-body`, `html-chrome`, `html-both`, and `html-unstructured`, keyed needs.json differences, searchindex top-level key differences, objects.inv missing/extra/changed records, warning line differences, and first-divergence grouping. Use these exact HTML inputs for the four localization tests:

~~~rust
let expected = "<html><div class=\"body\" role=\"main\">\nA\n</div><footer>ok</footer></html>";
let body_changed = "<html><div class=\"body\" role=\"main\">\nB\n</div><footer>ok</footer></html>";
let chrome_changed = "<html><div class=\"body\" role=\"main\">\nA\n</div><footer>changed</footer></html>";
let both_changed = "<html><div class=\"body\" role=\"main\">\nB\n</div><footer>changed</footer></html>";
let unstructured = "<html><main>A</main></html>";
assert_eq!(diagnose_html(expected, body_changed).category, "html-body");
assert_eq!(diagnose_html(expected, chrome_changed).category, "html-chrome");
assert_eq!(diagnose_html(expected, both_changed).category, "html-both");
assert_eq!(diagnose_html(expected, unstructured).category, "html-unstructured");
~~~

Use this needs.json pair and assert `missing-need`, `extra-need`, and `need-field` records are keyed by `versions[0].needs["N-1"]`, while a changed top-level key produces `needs-top-level` and an unexpected shape produces `needs-json-path`:

~~~json
{"versions":[{"version":"1","needs":{"N-1":{"title":"One","status":"open"},"N-2":{"title":"Two"}}}]}
{"versions":[{"version":"1","needs":{"N-1":{"title":"Changed","status":"open"},"N-3":{"title":"Three"}}}],"extra":true}
~~~

Assert the searchindex fixture reports `searchindex-missing-key`, `searchindex-extra-key`, and `searchindex-changed-key`, the inventory fixture reports `inventory-missing-record`, `inventory-extra-record`, and `inventory-changed-record`, normalized warnings report `warning-missing-line`, `warning-extra-line`, and `warning-changed-line`, and three synthetic failures at expected line 7 produce one first-divergence group with count 3. Add retention assertions that unset HTML_ORACLE_KEEP resolves to `failed`, `all` retains a passing run, and a complete case key is the rerun filter.
- [ ] Run `cargo test --test html_differential diagnostic_synthetic -- --nocapture`. Expected red result before implementation: the diagnostic functions and report grouping are missing; expected green result after implementation: all synthetic category and grouping assertions pass without invoking Sphinx-Ultra.
- [ ] Add the reporting interfaces to tests/support/html_oracle.rs without changing comparison decisions:

~~~rust
pub struct Diagnostic {
    pub category: String,
    pub logical_path: String,
    pub first_expected_line: Option<usize>,
    pub expected: String,
    pub actual: String,
    pub detail: String,
}

pub struct InventoryRecord {
    pub name: String,
    pub domain_role: String,
    pub priority: i32,
    pub uri: String,
    pub display_name: String,
}

pub struct FirstDivergenceGroup {
    pub expected_line: usize,
    pub count: usize,
    pub sample_files: Vec<String>,
}

pub fn diagnose_html(expected: &str, actual: &str) -> Diagnostic;
pub fn diagnose_needs_json(expected: &serde_json::Value, actual: &serde_json::Value) -> Vec<Diagnostic>;
pub fn diagnose_searchindex(expected: &serde_json::Value, actual: &serde_json::Value) -> Vec<Diagnostic>;
pub fn diagnose_inventory(expected: &[InventoryRecord], actual: &[InventoryRecord]) -> Vec<Diagnostic>;
pub fn diagnose_warnings(expected: &str, actual: &str) -> Vec<Diagnostic>;
pub fn group_first_divergences(diagnostics: &[Diagnostic]) -> Vec<FirstDivergenceGroup>;
~~~

Implement `diagnose_html` as a report-only wrapper around the fixed comparator, dispatched only after a strict `*.html` comparison has found a mismatch. Normalize CRLF first, locate the exact `<div class="body" role="main">` marker in each side, and find its matching `</div>` with a simple tag-depth scan that increments on non-self-closing `<div` tags and decrements on `</div>` tags. If either marker is absent, return `html-unstructured` with a whole-file unified diff. Otherwise compare body and the concatenated prefix/suffix chrome: body-only differences are `html-body`, chrome-only differences are `html-chrome`, and differences in both are `html-both`. Record the first differing full-file line and emit a three-line-context unified diff for the body before the chrome, capped at 64 KiB. Never rewrite the compared bytes or turn a diagnostic into a pass.
- [ ] Implement structured diagnostics in tests/support/html_oracle.rs. For needs.json, require `versions` to be an array and each element's `needs` to be an object; key records by `versions[index].needs[id]`, emit missing/extra/per-field records, report top-level and version-key changes, and use a JSON-path diff under `needs-json-path` when either shape is unexpected. For searchindex.js, report missing, extra, and changed top-level keys. For objects.inv, compare canonical five-field records and report missing, extra, and changed records. For warnings, compare normalized lines and include missing, extra, and changed line counts. Sort all diagnostics by logical path, category, first line, and detail; sort first-divergence groups by descending count then ascending expected line and retain the first 25.
- [ ] Implement retained run artifacts in tests/support/html_oracle.rs and tests/html_differential.rs with this exact layout and result shape:

~~~text
target/html-oracle/runs/<profile>/<source_set>/<case_id>/
  input/
  expected/
    warnings.txt
    needs/needs.json
  actual/
  actual-warnings.txt
  actual-needs/
  result.json
~~~

`input/` is populated from the case's materialized input files; `expected/` is reconstructed from profile refs and blobs and includes the reference warnings file plus local-needs needs/needs.json; `actual/` is Ultra's HTML output; `actual-warnings.txt` is Ultra's `-w` file; `actual-needs/` is Ultra's `-b needs` output when the case has a needs artifact; and `result.json` contains the case key, HTML status, needs status, pass/fail, sorted diagnostics, and the exact rerun filter `profile/source_set/case_id`. Recreate the complete run directory before each case and never place cache or doctree files inside `actual/`. A retained case is directly inspectable with `diff -r target/html-oracle/runs/profile/source_set/case_id/expected target/html-oracle/runs/profile/source_set/case_id/actual`.
- [ ] Parse `HTML_ORACLE_KEEP` as exactly `failed` or `all`, defaulting to `failed`. With `failed`, delete a passing case directory after writing its result; with `all`, retain every side-by-side directory. Parse `HTML_ORACLE_FILTER` as a substring over `profile/source_set/case_id`; the report's rerun command uses the complete case key so it selects one case. Reject any other keep value or a filter with no matching ledger case.
- [ ] Run `cargo test --test html_differential diagnostic_synthetic -- --nocapture` again. Expected green result: body/chrome classification, keyed needs diagnostics, searchindex/inventory/warning diagnostics, first-divergence groups, run layout, and keep/filter policy tests pass; existing strict comparator tests remain unchanged.
- [ ] Commit `test: add HTML oracle diagnostics`.

### Task 7: Actual CLI execution and bounded process handling

- [ ] Write red tests that re-enter std::env::current_exe through an ignored helper selected by HTML_ORACLE_DIAGNOSTICS_HELPER. Test a timeout, more than 512 KiB on both streams, and a read or kill error.
- [ ] Define ExitStatusKind with Success, BuildError(i32), Timeout, SpawnError(String), and IoError(String) in tests/support/diagnostics.rs.
- [ ] Spawn with piped stdout and stderr, drain both pipes to EOF on reader threads, retain at most 256 KiB per stream plus [output truncated], poll every 20 milliseconds to a 60-second deadline, kill and drain on timeout, and map wait/read/kill failures to IoError.
- [ ] Before every Ultra spawn, delete target/html-oracle/runs/profile/source_set/case_id and recreate it with separate output, cache, and warnings paths. This fresh-run rule prevents stale output from making a missing-file comparison pass.
- [ ] In tests/html_differential.rs invoke env!("CARGO_BIN_EXE_sphinx-ultra") with positional input and output paths, -b html, -d cache path, -q, and -w warnings path. The output, cache, and warnings paths are siblings under the fresh target/html-oracle/runs/profile/source_set/case_id directory. Do not use -M.
- [ ] Read the Ultra warnings file as the only source of the compared warnings field. Use combined stdout and stderr only for status, traceback, and bounded diagnostics; never store that combined stream as warnings.
- [ ] Verify against src/main.rs: positional SOURCEDIR OUTPUTDIR, -b/--builder, -c/--conf-dir, -d/--doctree-dir, -D, -A, -t/--tag, -n/--nitpicky, -q/--quiet, -w/--warning-file, -E/--fresh-env, -a/--write-all, and -T/--show-traceback.
- [ ] Run cargo test --test html_differential diagnostics smoke repeat_run. Expected green result: CARGO_BIN_EXE_sphinx-ultra executes, cache and warnings are outside output, a second fresh run has identical output and warnings, and no pipe deadlock occurs.
- [ ] Commit test: run Ultra with bounded diagnostics.

### Task 8: Two-root determinism and generator ordering

- [ ] Write red tests that generate one small complete core source set below two different absolute roots and compare core/index.json, inputs, refs, blobs, order, and root-leak behavior. The local-needs HTML and needs.json two-root regression is covered in Task 10 with the runner shim.
- [ ] Write the Rust repeat-run test that deletes and recreates one case run directory, invokes Ultra twice with -q -w, and asserts identical output-tree bytes and warning-file bytes.
- [ ] Implement sorted source-set, origin, case, file, and JSON-key ordering; UTF-8 JSON with indent 2 and final LF; process-pool -j N with canonical result collection; distinct absolute source, output, cache, and warnings paths per child. The repeat-build helper always starts with a deleted run directory.
- [ ] Implement the focused core command: uv run --locked --project tools/oracle_profiles/core python tools/gen_html_oracle.py --config tools/html_oracle_cases.toml --out tests/fixtures/html_oracle --profile core -j 2.
- [ ] Implement the focused local-needs command: uv run --locked --project tools/oracle_profiles/local_needs python tools/gen_html_oracle.py --config tools/html_oracle_cases.toml --out tests/fixtures/html_oracle --profile local_needs --needs-root C:\Users\johnm\Documents\repos\sphinx-needs -j 4.
- [ ] Ensure a focused profile run stages, validates, and replaces only its selected profile subtree; it cannot remove or rewrite an unselected profile. Run the two focused commands when producing the complete corpus.
- [ ] Run uv run --locked --project tools/oracle_profiles/core python -m pytest tools/test_gen_html_oracle.py -q -k deterministic. Expected green result: different absolute roots produce identical trees.
- [ ] Commit test: prove two-root oracle determinism.

### Task 9: Exhaustive differential run and reports

- [ ] Write red report tests for ordering by profile, source_set, case_id, logical_path, category; per-case pass/fail; counts per profile/source_set and category; excluded and reference-crash counts; side-by-side run retention; `HTML_ORACLE_KEEP=failed|all`; exact per-case `HTML_ORACLE_FILTER` reruns; the `html-body`, `html-chrome`, `html-both`, `html-unstructured`, needs, searchindex, inventory, and warning categories; the top-25 first-divergence table; and a 64 KiB assertion cap.
- [ ] Load both profile/index.json files and merge their cases in memory. Apply HTML_ORACLE_FILTER as a substring over profile/source_set/case_id. Schedule built and build-error. Do not schedule reference-crash, excluded-network, or excluded-plantuml. A filter with no matches is an error.
- [ ] Use available_parallelism as a bounded thread pool. Each worker recreates target/html-oracle/runs/profile/source_set/case_id, writes `input/`, reconstructs `expected/` from refs and blobs, runs Ultra into `actual/` with cache/doctree outside `actual/`, captures `actual-warnings.txt`, creates `actual-needs/` for every run and runs `-b needs` there when the ledger has local-needs fields, writes `result.json`, and applies HTML_ORACLE_KEEP after comparison. Each owned result includes its exact literal rerun filter such as `core/docutils_snippets/docutils-0001`; the main thread sorts all results before reporting.
- [ ] Write target/html-oracle/report.json with `total_cases`, `scheduled_cases`, `passed_cases`, `failed_cases`, `excluded_cases`, `reference_crash_cases`, `counts_by_source_set`, `counts_by_category`, `summary_by_profile_source_set`, `categories`, `first_divergences`, and `cases`. `summary_by_profile_source_set` has `profile`, `source_set`, `scheduled`, `passed`, and `failed`; `first_divergences` has `expected_line`, `count`, and `sample_files` for the top 25 groups; each case embeds the same diagnostics and rerun filter as result.json.
- [ ] Write report.md with these sections in order: summary table with one row per profile × source_set, category count table, most-common first-divergence table grouped by expected HTML line and capped at 25 rows, and per-case sections. Each failed case includes the run directory, all structured diagnostics, capped diffs, and a command containing its literal rerun key, for example `HTML_ORACLE_FILTER=core/docutils_snippets/docutils-0001 HTML_ORACLE_KEEP=all cargo test --test html_differential html_oracle_exhaustive -- --ignored --nocapture`.
- [ ] Cap each body diff, chrome diff, JSON-path diff, inventory diff, warning diff, and combined stdout plus assertion text at 64 KiB. The assertion points to both report files and states that the files contain every result; diagnostics never change strict comparison pass/fail decisions.
- [ ] Mark the test #[ignore] as html_oracle_exhaustive. Run cargo test --test html_differential html_oracle_exhaustive -- --ignored --nocapture. Expected result while Ultra is incomplete: every runnable case is attempted, all failures are aggregated, and the command exits nonzero.
- [ ] Run `HTML_ORACLE_FILTER=core/docutils_snippets/docutils-0001 HTML_ORACLE_KEEP=all cargo test --test html_differential html_oracle_exhaustive -- --ignored --nocapture` for a single-case diagnostic rerun, then run cargo test --test html_differential report and cargo test --test html_differential -- --list. Expected green result: report, retention, filter, and layout tests pass and exhaustive is ignored by default.
- [ ] Commit test: add exhaustive HTML oracle report.

### Task 10: Add the local-needs needs.json oracle

Files:

- Modify: tools/gen_html_oracle.py
- Modify: tools/html_oracle_runner.py
- Modify: tests/support/html_oracle.rs
- Modify: tests/html_differential.rs
- Test: tools/test_gen_html_oracle.py and tests/html_differential.rs

- [ ] Write red Python and Rust tests for four new case fields. Core cases must have needs_json, needs_status, needs_exit_code, and needs_warnings all null. A local-needs built case must record a needs status and a needs.json FileRecord at profile-relative storage path refs/sphinx_needs_doc_tests/doc_df_links_from_content/needs/needs.json when the second build writes the file. A local-needs build-error may retain a partial needs.json record, but its status, exit code, and warnings must be consistent.
- [ ] Use the existing local-needs project packages/sphinx-needs/tests/doc_test/doc_df_links_from_content as the regression project because its conf.py sets needs_build_json = True and its document creates need HTML. Build it twice below different absolute roots and assert that both the HTML output and needs.json are byte-identical after the fixed policies.
- [ ] After the HTML reference build has status built or build-error, run a second child reference build with a separate source/output/cache/warnings run directory and argv ["-q", "-w", warnings_file, "-b", "needs", "-D", "needs_reproducible_json=1", "-d", doctree_dir, sourcedir, needs_output]. Apply the UUID shim before Sphinx import and record needs_reproducible_json=1 in the local-needs determinism_shims list.
- [ ] Capture the second build status using the same traceback rule. Read only its -w warnings file into needs_warnings. If needs.json exists, store it at repository path tests/fixtures/html_oracle/local_needs/refs/sphinx_needs_doc_tests/case_id/needs/needs.json and set needs_json to a ref FileRecord. Set needs_status and needs_exit_code for the second build. Do not run this second build for core cases, excluded cases, or HTML reference-crash cases.
- [ ] Add the needs.json policy row to both generator and Rust comparator dispatch: parse JSON, ignore object-key order, preserve array order, and compare scalar types exactly. Keep needs.json emitted by the HTML build inside files and apply the same policy there.
- [ ] Extend the exhaustive worker. For every local-needs case whose HTML status is built or build-error, create the expected `needs/needs.json` side-by-side artifact, create `actual-needs/`, and invoke Ultra with -b needs, -q, -w warnings, and -d cache. If Ultra rejects the needs builder, add a needs-builder mismatch and do not merge it into the HTML mismatch list. If the builder runs, compare needs status, needs warnings, and needs.json with keyed `missing-need`, `extra-need`, `need-field`, `needs-top-level`, and `needs-json-path` diagnostics plus separate needs-json-value, invalid-needs-json, needs-warning, and needs-status categories. The HTML invocation and its mismatches remain independently reported.
- [ ] Do not translate or pre-filter native sphinx-needs directives: native Ultra processing of those directives is not assumed. A rejected needs builder is reported as needs-builder, and any other Ultra inability is reported as a known-red needs or HTML mismatch rather than being presented as a successful native needs build.
- [ ] Add report counts and Markdown sections for needs-builder and all other needs categories. The default suite remains green because only the ignored exhaustive test runs reference comparisons.
- [ ] Run uv run --locked --project tools/oracle_profiles/core python -m pytest tools/test_gen_html_oracle.py -q -k needs_json; cargo test --test html_differential needs_json. Expected red result before implementation: missing schema fields, missing artifact, and missing comparator policy. Expected green result after implementation: the regression project produces stable needs.json and all schema/status tests pass.
- [ ] Commit feat: add needs JSON oracle.

### Task 11: Generate artifacts, verify integrity, and hand off

- [ ] Run uv run --locked --project tools/oracle_profiles/core python -m pytest tools/test_gen_html_oracle.py -q; cargo fmt --all; cargo clippy --all-targets --all-features -- -D warnings; cargo test. Expected green result: all default checks pass before generation.
- [ ] Generate core with uv run --locked --project tools/oracle_profiles/core python tools/gen_html_oracle.py --config tools/html_oracle_cases.toml --out tests/fixtures/html_oracle --profile core -j 4.
- [ ] Generate local_needs with uv run --locked --project tools/oracle_profiles/local_needs python tools/gen_html_oracle.py --config tools/html_oracle_cases.toml --out tests/fixtures/html_oracle --profile local_needs --needs-root C:\Users\johnm\Documents\repos\sphinx-needs -j 4.
- [ ] Implement and run the separate verification commands `uv run --locked --project tools/oracle_profiles/core python tools/gen_html_oracle.py --config tools/html_oracle_cases.toml --out tests/fixtures/html_oracle --verify --profile core` and `uv run --locked --project tools/oracle_profiles/local_needs python tools/gen_html_oracle.py --config tools/html_oracle_cases.toml --out tests/fixtures/html_oracle --verify --profile local_needs --needs-root C:\Users\johnm\Documents\repos\sphinx-needs`. A focused verification validates only its profile subtree.
- [ ] Verify every file record, size, hash, blob name, logical path, lock digest, source-set count, source/ledger key equality, hash formula, sorted serialization, symlink rule, path rule, root-leak rule, and final fixture size. Print total size and warn if it exceeds 150 MB.
- [ ] Run cargo test --test html_differential html_oracle_exhaustive -- --ignored --nocapture. Expected result: known-red nonzero with complete report.md and report.json.
- [ ] Run cargo fmt --all -- --check; cargo clippy --all-targets --all-features -- -D warnings; cargo test; uv run --locked --project tools/oracle_profiles/core python -m pytest tools/test_gen_html_oracle.py -q; git diff --check.
- [ ] Review that only the planned generator, profiles, fixtures, Rust support, tests, and NOTICE.md changed. Commit feat: commit complete HTML oracle corpus.

## Final review checklist

- [ ] Read-only verification confirms src/main.rs still supports positional source/output, -b html, and -d cache paths exactly as used; no file under src/ is modified.
- [ ] tools/gen_sphinx_fixture.py still has extensions=[], master_doc='index', exclude_patterns=['_build'], smartquotes=False, and keep_warnings=True.
- [ ] tools/gen_inventory_fixture.py still has four SPHINX_PROJECTS entries.
- [ ] tests/fixtures still has exactly the seven named HTML projects.
- [ ] The sibling checkout still has packages/sphinx-needs/src/sphinx_needs and packages/sphinx-needs/tests/doc_test with exactly 142 conf.py directories.
- [ ] The local-needs commit and subtree tree match 58bcb59d861da95f2aca79f343e8bae6ec5c1250 and 958172a89defcec69704f6b9d61e482e7c4e8409.
- [ ] core/index.json and local_needs/index.json contain only the five statuses above, and no focused regeneration changes the other profile subtree.
- [ ] The comparison code implements only the fixed table.
- [ ] Static blobs are deduplicated while all logical paths are present.
- [ ] The ignored exhaustive test uses CARGO_BIN_EXE_sphinx-ultra, external cache paths, all built and build-error cases, bounded diagnostics, deterministic ordering, complete side-by-side run directories, HTML_ORACLE_KEEP, exact per-case HTML_ORACLE_FILTER reruns, structured needs/searchindex/objects.inv/warnings diagnostics, first-divergence grouping, and complete reports.
- [ ] The report contains the profile × source_set summary, category counts, top-25 first-divergence groups, per-case diagnostics, and a rerun filter for every failure; diagnostics never relax strict comparisons.
- [ ] Synthetic diagnostics tests for HTML body/chrome, needs.json keyed diffs, inventory records, and first-divergence grouping pass in the default suite.
- [ ] Nothing under src/ changes; the implementation scope is limited to tools/, tests/, tests/fixtures/html_oracle/, and NOTICE.md.
- [ ] No implementation code is written outside the planned paths.
