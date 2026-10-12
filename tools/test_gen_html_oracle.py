import copy
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest
import docutils

REPO_ROOT = Path(__file__).resolve().parents[1]

from tools.gen_html_oracle import (
    DiscoveryError,
    DiscoveredCase,
    SchemaError,
    assert_discovery_keys_equal,
    StorageError,
    _case_record_from_result,
    _format_root_leaks,
    _run_reference_case,
    atomic_swap_profile,
    canonical_hash,
    capture_output_tree,
    classify_project,
    discover_cases,
    ensure_unique_case_keys,
    generate_profile,
    profile_record,
    normalize_warnings,
    reduce_sphinx_error_report,
    _find_root_leaks,
    verify_profile_lock,
    store_output_files,
    validate_needs_metadata,
    validate_index_document,
    store_blob,
)


def _file_record(storage_path: str, content: bytes, *, logical_path: str = "index.rst"):
    return {
        "logical_path": logical_path,
        "storage": "input",
        "storage_path": storage_path,
        "sha256": hashlib.sha256(content).hexdigest(),
        "size": len(content),
    }


def _document(tmp_path: Path) -> dict:
    input_path = tmp_path / "inputs" / "docutils_snippets" / "case" / "index.rst"
    input_path.parent.mkdir(parents=True)
    content = b"Hello\n"
    input_path.write_bytes(content)
    input_record = _file_record(
        "inputs/docutils_snippets/case/index.rst", content
    )
    return {
        "schema_version": 1,
        "generator": "html-oracle/1",
        "profiles": {
            "core": {
                "sphinx": "9.1.0",
                "docutils": "0.22.4",
                "platform": "linux",
                "needs_version": None,
                "needs_commit": None,
                "needs_tree": None,
                "lock_path": "tools/oracle_profiles/core/uv.lock",
                "lock_sha256": "0" * 64,
                "determinism_shims": ["uuid.uuid4=counter"],
            }
        },
        "cases": [
            {
                "profile": "core",
                "source_set": "docutils_snippets",
                "case_id": "case",
                "status": "built",
                "exit_code": 0,
                "exception_type": None,
                "warnings": "",
                "excluded_reason": None,
                "origin": {
                    "source_set": "docutils_snippets",
                    "origin_path": "tests/fixtures/doctree_differential.json[0]",
                    "pytest_node_ids": [],
                    "variants_not_captured": False,
                },
                "input_files": [input_record],
                "input_sha256": "0" * 64,
                "tree_sha256": "0" * 64,
                "files": [],
                "needs_json": None,
                "needs_status": None,
                "needs_exit_code": None,
                "needs_warnings": None,
            }
        ],
    }


def test_rejects_unknown_status(tmp_path):
    document = _document(tmp_path)
    document["cases"][0]["status"] = "unknown"
    with pytest.raises(SchemaError, match="status"):
        validate_index_document(document, tmp_path)


def test_rejects_missing_input_files(tmp_path):
    document = _document(tmp_path)
    del document["cases"][0]["input_files"]
    with pytest.raises(SchemaError, match="input_files"):
        validate_index_document(document, tmp_path)


def test_rejects_missing_input_hash(tmp_path):
    document = _document(tmp_path)
    del document["cases"][0]["input_sha256"]
    with pytest.raises(SchemaError, match="input_sha256"):
        validate_index_document(document, tmp_path)


def test_rejects_storage_path_escape(tmp_path):
    document = _document(tmp_path)
    document["cases"][0]["input_files"][0]["storage_path"] = (
        "inputs/docutils_snippets/case/../../escape"
    )
    with pytest.raises(SchemaError, match="path"):
        validate_index_document(document, tmp_path)


def test_rejects_hash_mismatch(tmp_path):
    document = _document(tmp_path)
    document["cases"][0]["input_files"][0]["sha256"] = "1" * 64
    with pytest.raises(SchemaError, match="hash"):
        validate_index_document(document, tmp_path)


def test_normalize_warnings_replaces_all_runtime_roots_and_crash_log():
    case_root = Path(r"C:\oracle\case-123")
    source_root = case_root / "source"
    output_root = case_root / "html" / "output"
    doctree_root = case_root / "html" / "doctree"
    raw = (
        f"{source_root}\\index.rst:1: WARNING: source\n"
        f"{output_root}\\index.html: output\n"
        f"{doctree_root}\\index.doctree: doctree\n"
        f"{case_root}\\warnings.txt: case\n"
        "C:\\tmp\\sphinx-err-abc123.log: crash\n"
    ).encode()

    normalized = normalize_warnings(
        raw,
        source_root,
        output_root=output_root,
        doctree_root=doctree_root,
        case_root=case_root,
    )

    assert b"<SRCDIR>\\index.rst" in normalized.encode()
    assert b"<OUTDIR>\\index.html" in normalized.encode()
    assert b"<DOCTREEDIR>\\index.doctree" in normalized.encode()
    assert b"<CASEDIR>\\warnings.txt" in normalized.encode()
    assert b"<SPHINX_ERR_LOG>" in normalized.encode()
    assert str(case_root).encode() not in normalized.encode()


def test_verify_profile_lock_checks_recorded_digest(tmp_path):
    lock_path = tmp_path / "tools" / "oracle_profiles" / "core" / "uv.lock"
    lock_path.parent.mkdir(parents=True)
    lock_path.write_bytes(b"lock contents")
    record = {
        "lock_path": "tools/oracle_profiles/core/uv.lock",
        "lock_sha256": hashlib.sha256(b"lock contents").hexdigest(),
    }
    verify_profile_lock(tmp_path, record)
    lock_path.write_bytes(b"tampered")
    with pytest.raises(StorageError, match="lock_sha256"):
        verify_profile_lock(tmp_path, record)


def test_rejects_duplicate_case_key(tmp_path):
    document = _document(tmp_path)
    duplicate = copy.deepcopy(document["cases"][0])
    document["cases"].append(duplicate)
    with pytest.raises(SchemaError, match="duplicate"):
        validate_index_document(document, tmp_path)


def test_rejects_null_exit_code_for_built(tmp_path):
    document = _document(tmp_path)
    document["cases"][0]["exit_code"] = None
    with pytest.raises(SchemaError, match="exit_code"):
        validate_index_document(document, tmp_path)


def test_rejects_files_on_excluded_case(tmp_path):
    document = _document(tmp_path)
    case = document["cases"][0]
    case["status"] = "excluded-network"
    case["exit_code"] = None
    case["excluded_reason"] = "remote fetch"
    case["input_files"] = []
    case["input_sha256"] = "0" * 64
    case["files"] = [
        {
            "logical_path": "index.html",
            "storage": "ref",
            "storage_path": "refs/docutils_snippets/case/index.html",
            "sha256": "0" * 64,
            "size": 0,
        }
    ]
    with pytest.raises(SchemaError, match="excluded"):
        validate_index_document(document, tmp_path)


def test_discovers_the_complete_core_corpus():
    repo_root = Path(__file__).resolve().parents[1]
    cases = discover_cases(
        repo_root,
        repo_root / "tools" / "html_oracle_cases.toml",
        profile="core",
    )
    counts = {}
    for case in cases:
        counts[case.source_set] = counts.get(case.source_set, 0) + 1
    assert counts == {
        "docutils_snippets": 735,
        "environment_projects": 29,
        "html_projects": 7,
        "inventory_projects": 4,
        "sphinx_read_snippets": 489,
    }
    assert all(case.files for case in cases)


def test_snippet_projects_disable_rendered_system_messages():
    repo_root = Path(__file__).resolve().parents[1]
    cases = discover_cases(
        repo_root,
        repo_root / "tools" / "html_oracle_cases.toml",
        profile="core",
    )
    snippet_cases = [case for case in cases if case.source_set == "docutils_snippets"]
    assert snippet_cases
    assert all(b"keep_warnings = False" in case.files["conf.py"] for case in snippet_cases)
    assert all(b"keep_warnings = True" not in case.files["conf.py"] for case in snippet_cases)


def test_discovers_all_local_needs_projects():
    needs_root_value = os.environ.get("SPHINX_NEEDS_ROOT")
    if not needs_root_value:
        pytest.skip("SPHINX_NEEDS_ROOT is unset; local sphinx-needs checkout is required")
    repo_root = Path(__file__).resolve().parents[1]
    cases = discover_cases(
        repo_root,
        repo_root / "tools" / "html_oracle_cases.toml",
        profile="local_needs",
        needs_root=Path(needs_root_value),
    )
    assert len(cases) == 142
    assert {case.source_set for case in cases} == {"sphinx_needs_doc_tests"}
    assert all(case.files.get("conf.py") for case in cases)


def test_rejects_duplicate_discovery_keys():
    case = DiscoveredCase(
        profile="core",
        source_set="fixture",
        case_id="same",
        origin_path="fixture",
        files={"index.rst": b""},
    )
    with pytest.raises(DiscoveryError, match="duplicate"):
        ensure_unique_case_keys([case, case])


def test_rejects_discovery_ledger_key_mismatch():
    case = DiscoveredCase(
        profile="core",
        source_set="fixture",
        case_id="same",
        origin_path="fixture",
        files={"index.rst": b""},
    )
    with pytest.raises(DiscoveryError, match="mismatch"):
        assert_discovery_keys_equal([case], [])


def test_rejects_needs_root_without_pinned_subtree(tmp_path):
    repo_root = Path(__file__).resolve().parents[1]
    with pytest.raises(DiscoveryError, match="needs-root"):
        discover_cases(
            repo_root,
            repo_root / "tools" / "html_oracle_cases.toml",
            profile="local_needs",
            needs_root=tmp_path,
        )


def test_plantuml_app_extension_is_excluded_and_network_wins():
    files = {
        "conf.py": b"extensions = []\napp.setup_extension('sphinxcontrib.plantuml')\n",
        "index.rst": b"index\n=====\n",
    }
    assert classify_project(files) == ("excluded-plantuml", "conf.py loads sphinxcontrib.plantuml")
    both = {
        "conf.py": b"extensions = ['sphinxcontrib.plantuml']\nintersphinx_mapping = {'x': ('https://example.invalid', None)}\n",
        "index.rst": b"index\n=====\n",
    }
    assert classify_project(both)[0] == "excluded-network"


def test_intersphinx_remote_target_with_local_inventory_is_allowed():
    files = {
        "conf.py": (
            b"intersphinx_mapping = {'other': ('https://example.invalid/', 'local.inv')}\n"
        ),
        "index.rst": b"index\n=====\n",
        "local.inv": b"inventory",
    }
    assert classify_project(files) == (None, None)


def _run_runner(
    tmp_path: Path,
    *,
    conf: str | None,
    index: str = "Title\n=====\n",
    extra_args: list[str] | None = None,
):
    source = tmp_path / "src"
    output = tmp_path / "out"
    doctree = tmp_path / "doctree"
    warnings = tmp_path / "warnings.txt"
    exception_file = tmp_path / "exception.txt"
    source.mkdir()
    if conf is not None:
        (source / "conf.py").write_text(conf, encoding="utf-8")
    (source / "index.rst").write_text(index, encoding="utf-8")
    command = [
        sys.executable,
        str(Path(__file__).with_name("html_oracle_runner.py")),
        "--profile",
        "core",
        "--sourcedir",
        str(source),
        "--outputdir",
        str(output),
        "--doctree-dir",
        str(doctree),
        "--builder",
        "html",
        "--warnings-file",
        str(warnings),
        "--exception-file",
        str(exception_file),
    ]
    if extra_args:
        command.extend(extra_args)
    return subprocess.run(command, capture_output=True, text=True), output, warnings


def test_child_runner_denies_network_before_sphinx(tmp_path):
    result, output, _warnings = _run_runner(
        tmp_path,
        conf=(
            "project = 'network-test'\n"
            "extensions = []\n"
            "master_doc = 'index'\n"
            "import socket\n"
            "socket.create_connection(('example.invalid', 80))\n"
        ),
    )
    assert result.returncode != 0
    assert "network disabled by html oracle" in result.stderr
    assert not (output / "index.html").exists()


def test_child_runner_uses_pinned_core_versions(tmp_path):
    result, output, warnings = _run_runner(
        tmp_path,
        conf=(
            "project = 'version-test'\n"
            "extensions = []\n"
            "master_doc = 'index'\n"
        ),
    )
    assert result.returncode == 0, result.stderr
    assert (output / "index.html").is_file()
    assert warnings.read_text(encoding="utf-8") == ""


@pytest.mark.parametrize(
    ("conf_extra", "index", "expected"),
    [
        (
            "from sphinx.errors import ConfigError\nraise ConfigError('bad config')",
            "Title\n=====\n",
            "sphinx.errors.ConfigError",
        ),
        (
            "numfig = True\nnumfig_format = {'figure': 'Figure %s', 'table': 'Table {number}'}",
            ".. table:: Table\n   :name: table\n\n   =====  =====\n   A      B\n   =====  =====\n   a      b\n   =====  =====\n",
            "builtins.TypeError",
        ),
    ],
)
def test_child_runner_records_qualified_exception_type(tmp_path, conf_extra, index, expected):
    exception_file = tmp_path / "exception.txt"
    result, _output, _warnings = _run_runner(
        tmp_path,
        conf=(
            "project = 'exception-test'\n"
            "extensions = []\n"
            "master_doc = 'index'\n"
            f"{conf_extra}\n"
        ),
        index=index,
    )
    assert result.returncode == 2
    assert exception_file.read_text(encoding="utf-8") == expected + "\n"


def test_real_sphinx_error_report_reduction_keeps_prefix_header_and_exception(tmp_path):
    result, _output, warnings = _run_runner(tmp_path, conf=None)
    assert result.returncode == 2, result.stderr
    raw = warnings.read_bytes()
    assert b"Versions" in raw
    assert b"Loaded Extensions" in raw
    assert b"Traceback" in raw

    source = tmp_path / "src"
    normalized = normalize_warnings(
        raw,
        source,
        output_root=tmp_path / "out",
        doctree_root=tmp_path / "doctree",
        case_root=tmp_path,
    )
    reduced = reduce_sphinx_error_report(normalized)
    assert reduced.startswith("\nConfiguration error!\n")
    assert "sphinx.errors.ConfigError: config directory doesn't contain a conf.py file" in reduced
    assert "Versions" not in reduced
    assert "Loaded Extensions" not in reduced
    assert "Traceback" not in reduced
    assert _find_root_leaks(
        reduced.encode("utf-8"),
        [source, tmp_path / "out", tmp_path / "doctree", tmp_path],
    ) == []


@pytest.mark.parametrize(
    ("fixture_name", "prefix", "exception_line"),
    [
        (
            "docutils-0500.txt",
            "",
            "ValueError: list.remove(x): x not in list",
        ),
        (
            "numfig_on.txt",
            "<SRCDIR>/a.rst:4: WARNING: image file not readable",
            "TypeError: not all arguments converted during string formatting",
        ),
        (
            "doc_service_github.txt",
            '<SRCDIR>/index.rst:4: WARNING: "query" or "specific" missing',
            "RuntimeError: network disabled by html oracle",
        ),
    ],
)
def test_real_crash_reports_are_reduced_and_leak_free(
    fixture_name, prefix, exception_line
):
    fixture = REPO_ROOT / "tests" / "fixtures" / "sphinx_error_reports" / fixture_name
    raw = fixture.read_bytes()
    assert _find_root_leaks(raw, [REPO_ROOT])

    reduced = reduce_sphinx_error_report(raw.decode("utf-8"))
    assert reduced.startswith(prefix)
    assert exception_line in reduced
    assert "Versions" not in reduced
    assert "Last Messages" not in reduced
    assert "Loaded Extensions" not in reduced
    assert "Traceback" not in reduced
    assert _find_root_leaks(reduced.encode("utf-8"), [REPO_ROOT]) == []


def test_root_leak_scan_rejects_runtime_host_details(tmp_path):
    data = (
        f"{tmp_path} /home/runner /tmp/sphinx-err-abc.log "
        r"C:\work\.venv\Lib\site-packages\pkg Linux-6.17.0-1022-azure"
    ).encode("utf-8")
    assert _find_root_leaks(data, [tmp_path])


def test_case_record_stores_exception_type(tmp_path):
    case = DiscoveredCase(
        profile="core",
        source_set="docutils_snippets",
        case_id="case",
        origin_path="fixture",
        files={"index.rst": b"Title\n=====\n"},
    )
    record = _case_record_from_result(
        case,
        {
            "status": "reference-crash",
            "exit_code": 2,
            "exception_type": "builtins.TypeError",
            "warnings": "",
            "output_files": {},
            "root_leaks": [],
            "needs_status": None,
            "needs_exit_code": None,
            "needs_warnings": None,
            "needs_json_bytes": None,
        },
        tmp_path,
    )
    assert record["exception_type"] == "builtins.TypeError"


def test_validator_accepts_exception_type_for_reference_crash(tmp_path):
    document = _document(tmp_path)
    case = document["cases"][0]
    case["status"] = "reference-crash"
    case["exit_code"] = 2
    case["exception_type"] = "builtins.TypeError"
    case["input_sha256"] = canonical_hash(
        [(item["logical_path"], item["sha256"]) for item in case["input_files"]]
    )
    case["tree_sha256"] = canonical_hash([])
    validated = validate_index_document(document, tmp_path)
    assert validated["cases"][0]["exception_type"] == "builtins.TypeError"


def test_validator_rejects_exception_type_for_built_case(tmp_path):
    document = _document(tmp_path)
    document["cases"][0]["exception_type"] = "builtins.TypeError"
    with pytest.raises(SchemaError, match="exception_type"):
        validate_index_document(document, tmp_path)


def _fake_needs_root(tmp_path: Path) -> Path:
    root = tmp_path / "sphinx-needs"
    (root / "packages" / "sphinx-needs" / "src" / "sphinx_needs").mkdir(parents=True)
    (root / "packages" / "sphinx-needs" / "tests" / "doc_test").mkdir(parents=True)
    return root


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("module_file", Path("C:/wrong/sphinx_needs/__init__.py")),
        ("module_version", "0.0.0"),
        ("commit", "0" * 40),
        ("tree", "0" * 40),
        ("status", " M packages/sphinx-needs/src/sphinx_needs/__init__.py"),
    ],
)
def test_needs_provenance_rejects_wrong_observation(tmp_path, field, value):
    root = _fake_needs_root(tmp_path)
    observations = {
        "module_file": root / "packages/sphinx-needs/src/sphinx_needs/__init__.py",
        "module_version": "8.5.0",
        "commit": "58bcb59d861da95f2aca79f343e8bae6ec5c1250",
        "tree": "958172a89defcec69704f6b9d61e482e7c4e8409",
        "status": "",
    }
    observations[field] = value
    with pytest.raises(RuntimeError, match="needs provenance"):
        validate_needs_metadata(root, **observations)


def test_needs_provenance_ignores_dirt_outside_package_subtree(tmp_path):
    root = _fake_needs_root(tmp_path)
    validate_needs_metadata(
        root,
        module_file=root / "packages/sphinx-needs/src/sphinx_needs/__init__.py",
        module_version="8.5.0",
        commit="58bcb59d861da95f2aca79f343e8bae6ec5c1250",
        tree="958172a89defcec69704f6b9d61e482e7c4e8409",
        status="",
    )


def test_repeated_static_bytes_use_one_blob(tmp_path):
    blob_root = tmp_path / "blobs"
    first = store_blob(blob_root, b"same static bytes")
    second = store_blob(blob_root, b"same static bytes")
    assert first == second
    assert list(blob_root.iterdir()) == [blob_root / first]


def test_capture_preserves_all_logical_paths_and_changes_hashes(tmp_path):
    output = tmp_path / "output"
    output.joinpath("_static").mkdir(parents=True)
    output.joinpath("_images").mkdir()
    output.joinpath("_static/theme.css").write_bytes(b"same")
    output.joinpath("_images/logo.png").write_bytes(b"same")
    output.joinpath("index.html").write_bytes(b"one")
    records = capture_output_tree(output, tmp_path, "set", "case")
    assert [record["logical_path"] for record in records] == [
        "_images/logo.png",
        "_static/theme.css",
        "index.html",
    ]
    assert {record["storage"] for record in records if record["logical_path"].startswith("_")} == {"blob"}
    index_record = next(record for record in records if record["logical_path"] == "index.html")
    output.joinpath("index.html").write_bytes(b"two")
    changed = capture_output_tree(output, tmp_path, "set", "case")
    changed_record = next(record for record in changed if record["logical_path"] == "index.html")
    assert changed_record["sha256"] != index_record["sha256"]


def test_capture_rejects_symlink_before_file_check(tmp_path):
    output = tmp_path / "output"
    output.mkdir()
    target = tmp_path / "target.txt"
    target.write_bytes(b"target")
    try:
        (output / "link.txt").symlink_to(target)
    except (OSError, NotImplementedError):
        pytest.skip("symlink creation is unavailable")
    with pytest.raises(StorageError, match="symlink"):
        capture_output_tree(output, tmp_path, "set", "case")


def test_capture_rejects_absolute_root_leaks(tmp_path):
    output = tmp_path / "output"
    output.mkdir()
    (output / "index.html").write_text(str(tmp_path / "source"), encoding="utf-8")
    with pytest.raises(StorageError, match="root leak"):
        capture_output_tree(
            output,
            tmp_path,
            "set",
            "case",
            root_paths=[tmp_path / "source", tmp_path / "build", tmp_path / "cache"],
        )


def test_text_policy_replaces_source_root_and_crlf(tmp_path):
    source = tmp_path / "source"
    profile_root = tmp_path / "profile"
    source.mkdir()
    records = store_output_files(
        {
            "index.html": (
                f"before {source.as_posix()}/index.rst\r\n"
                f"after {source.as_posix()}-suffix\r\n"
            ).encode("utf-8")
        },
        profile_root,
        "set",
        "case",
        root_paths=[source],
        source_root=source,
    )
    stored = (profile_root / records[0]["storage_path"]).read_bytes()
    assert stored == (
        b"before <SRCDIR>/index.rst\n"
        + f"after {source.as_posix()}-suffix\n".encode("utf-8")
    )


def test_searchindex_policy_replaces_source_root_and_crlf(tmp_path):
    source = tmp_path / "source"
    profile_root = tmp_path / "profile"
    source.mkdir()
    records = store_output_files(
        {"searchindex.js": f"{source.as_posix()}\\index.html\r\n".encode("utf-8")},
        profile_root,
        "set",
        "case",
        root_paths=[source],
        source_root=source,
    )
    stored = (profile_root / records[0]["storage_path"]).read_bytes()
    assert stored == b"<SRCDIR>\\index.html\n"


def test_searchindex_policy_replaces_json_escaped_windows_root(tmp_path):
    source = tmp_path / "source"
    profile_root = tmp_path / "profile"
    source.mkdir()
    escaped_root = str(source.resolve()).replace("\\", "\\\\")
    records = store_output_files(
        {"searchindex.js": f"{escaped_root}\\\\index.html\r\n".encode("utf-8")},
        profile_root,
        "set",
        "case",
        root_paths=[source],
        source_root=source,
    )
    stored = (profile_root / records[0]["storage_path"]).read_bytes()
    assert stored == b"<SRCDIR>\\\\index.html\n"


def test_non_text_and_needs_json_source_root_leaks_are_still_rejected(tmp_path):
    source = tmp_path / "source"
    source.mkdir()
    for logical_path in ("image.png", "needs/needs.json"):
        with pytest.raises(StorageError, match="root leak"):
            store_output_files(
                {logical_path: str(source).encode("utf-8")},
                tmp_path / "profile",
                "set",
                "case",
                root_paths=[source],
            )


def test_root_leak_report_lists_all_case_files_in_sorted_order():
    message = _format_root_leaks(
        [
            ("core", "sphinx_read_snippets", "sphinx-read-0002", "index.html"),
            ("core", "docutils_snippets", "docutils-0001", "index.html"),
            ("core", "docutils_snippets", "docutils-0001", "searchindex.js"),
        ]
    )
    assert message == (
        "root leaks:\n"
        "core/docutils_snippets/docutils-0001: index.html\n"
        "core/docutils_snippets/docutils-0001: searchindex.js\n"
        "core/sphinx_read_snippets/sphinx-read-0002: index.html"
    )


def test_profile_record_includes_generator_platform():
    record = profile_record(REPO_ROOT, "core")
    assert record["platform"] == sys.platform


def test_profile_lock_digest_normalizes_crlf(tmp_path):
    lock_path = tmp_path / "tools" / "oracle_profiles" / "core" / "uv.lock"
    lock_path.parent.mkdir(parents=True)
    lock_path.write_bytes(b"version = 1\r\n\r\n[[package]]\r\n")
    profile = profile_record(tmp_path, "core")
    assert profile["lock_sha256"] == hashlib.sha256(
        b"version = 1\n\n[[package]]\n"
    ).hexdigest()
    verify_profile_lock(tmp_path, profile)


def test_rejects_profile_without_platform(tmp_path):
    document = _document(tmp_path)
    del document["profiles"]["core"]["platform"]
    with pytest.raises(SchemaError, match="platform"):
        validate_index_document(document, tmp_path)


def test_verify_warns_for_non_linux_reference_platform(tmp_path, capsys):
    config = tmp_path / "cases.toml"
    config.write_text(
        """[[source_sets]]
name = "html_projects"
profile = "core"
kind = "html_projects"
source = "tests/fixtures"
projects = ["basic"]
count = 1
""",
        encoding="utf-8",
    )
    output = tmp_path / "oracle"
    generate_profile(REPO_ROOT, config, output, profile="core", jobs=1)
    from tools.gen_html_oracle import verify_profile

    verify_profile(REPO_ROOT, config, output, profile="core")
    output_text = capsys.readouterr().out
    assert "WARNING" in output_text
    assert "platform" in output_text


def test_atomic_swap_failure_preserves_old_profile(tmp_path, monkeypatch):
    final = tmp_path / "core"
    staging = tmp_path / "core.staging"
    final.mkdir()
    (final / "sentinel").write_text("old", encoding="utf-8")
    staging.mkdir()
    (staging / "sentinel").write_text("new", encoding="utf-8")
    monkeypatch.setenv("HTML_ORACLE_INJECT_FAILURE_AFTER", "0")
    with pytest.raises(RuntimeError, match="injected"):
        atomic_swap_profile(staging, final, case_count=0)
    assert (final / "sentinel").read_text(encoding="utf-8") == "old"


def test_generate_profile_writes_valid_atomic_case_tree(tmp_path):
    config = tmp_path / "cases.toml"
    config.write_text(
        """[[source_sets]]
name = \"html_projects\"
profile = \"core\"
kind = \"html_projects\"
source = \"tests/fixtures\"
projects = [\"basic\"]
count = 1
""",
        encoding="utf-8",
    )
    output = tmp_path / "oracle"
    document = generate_profile(
        REPO_ROOT,
        config,
        output,
        profile="core",
        jobs=1,
    )
    assert len(document["cases"]) == 1
    assert document["cases"][0]["status"] == "built"
    assert (output / "core" / "index.json").is_file()
    assert not (output / "core.staging").exists()


def test_generate_profile_is_identical_under_two_output_roots(tmp_path):
    config = tmp_path / "cases.toml"
    config.write_text(
        """[[source_sets]]
name = \"html_projects\"
profile = \"core\"
kind = \"html_projects\"
source = \"tests/fixtures\"
projects = [\"basic\"]
count = 1
""",
        encoding="utf-8",
    )
    first_root = tmp_path / "first"
    second_root = tmp_path / "second"
    generate_profile(REPO_ROOT, config, first_root, profile="core", jobs=1)
    generate_profile(REPO_ROOT, config, second_root, profile="core", jobs=1)

    first_files = {
        path.relative_to(first_root / "core").as_posix(): path.read_bytes()
        for path in (first_root / "core").rglob("*")
        if path.is_file()
    }
    second_files = {
        path.relative_to(second_root / "core").as_posix(): path.read_bytes()
        for path in (second_root / "core").rglob("*")
        if path.is_file()
    }
    assert first_files == second_files


def test_local_needs_record_stores_second_build_json(tmp_path):
    case = DiscoveredCase(
        profile="local_needs",
        source_set="sphinx_needs_doc_tests",
        case_id="doc_basic",
        origin_path="packages/sphinx-needs/tests/doc_test/doc_basic",
        files={"conf.py": b"project = 'needs'\n", "index.rst": b"Needs\n=====\n"},
    )
    record = _case_record_from_result(
        case,
        {
            "status": "built",
            "exit_code": 0,
            "warnings": "",
            "output_files": {"index.html": b"<html />"},
            "needs_status": "built",
            "needs_exit_code": 0,
            "needs_warnings": "",
            "needs_json_bytes": b'{"needs": []}\n',
        },
        tmp_path,
    )
    assert record["needs_status"] == "built"
    assert record["needs_exit_code"] == 0
    assert record["needs_json"] is not None
    assert record["needs_json"]["storage_path"] == (
        "refs/sphinx_needs_doc_tests/doc_basic/needs/needs.json"
    )
    assert (tmp_path / record["needs_json"]["storage_path"]).read_bytes() == b'{"needs": []}\n'


def test_local_needs_built_case_requires_second_build_fields(tmp_path):
    document = _document(tmp_path)
    document["profiles"]["local_needs"] = copy.deepcopy(document["profiles"]["core"])
    document["cases"][0]["profile"] = "local_needs"
    document["cases"][0]["input_sha256"] = canonical_hash(
        [(item["logical_path"], item["sha256"]) for item in document["cases"][0]["input_files"]]
    )
    document["cases"][0]["tree_sha256"] = canonical_hash([])
    with pytest.raises(SchemaError, match="needs_status"):
        validate_index_document(document, tmp_path)


@pytest.mark.skipif(docutils.__version__ != "0.21.2", reason="requires the local-needs profile environment")
def test_local_needs_reference_case_captures_stable_needs_json():
    needs_root_value = os.environ.get("SPHINX_NEEDS_ROOT")
    if not needs_root_value:
        pytest.skip("SPHINX_NEEDS_ROOT is unset; local sphinx-needs checkout is required")
    needs_root = Path(needs_root_value)
    cases = discover_cases(
        REPO_ROOT,
        REPO_ROOT / "tools" / "html_oracle_cases.toml",
        profile="local_needs",
        needs_root=needs_root,
    )
    case = next(item for item in cases if item.case_id == "doc_basic")
    result = _run_reference_case(case, repo_root=REPO_ROOT, needs_root=needs_root)
    assert result["status"] == "built", result
    assert result["needs_status"] == "built", result
    assert result["needs_exit_code"] == 0
    assert result["needs_json_bytes"]
    repeat = _run_reference_case(case, repo_root=REPO_ROOT, needs_root=needs_root)
    assert result["needs_json_bytes"] == repeat["needs_json_bytes"]
