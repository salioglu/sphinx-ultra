mod support {
    pub mod diagnostics;
    pub mod html_oracle;
}

use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use support::diagnostics::{run_bounded, run_bounded_with_timeout, ExitStatusKind, ProcessOutput};
use support::html_oracle::{
    apply_retention, bounded_assertion_message,
    build_report_with_platforms_reference_cases_and_bin, case_key, compare_file, compare_trees,
    compare_warnings, compare_warnings_with_roots, diagnose_html, diagnose_inventory,
    diagnose_needs_json, diagnose_searchindex, diagnose_warnings, group_first_divergences,
    load_fixture_suite, materialize_case_expected, materialize_case_inputs,
    mismatch_diagnostics_with_source_root, parse_keep, read_record, resolve_ultra_binary,
    status_name, walk_tree, warning_diagnostics_with_roots, write_logical_file, CaseRecord,
    CaseResult, CaseStatus, IndexDocument, InventoryRecord, Keep, Policy,
};

fn minimal_case() -> serde_json::Value {
    json!({
        "profile": "core",
        "source_set": "synthetic",
        "case_id": "case-1",
        "status": "built",
        "exception_type": null,
        "exit_code": 0,
        "warnings": "",
        "excluded_reason": null,
        "origin": {
            "source_set": "synthetic",
            "origin_path": "synthetic.json[0]",
            "pytest_node_ids": [],
            "variants_not_captured": false
        },
        "input_files": [],
        "input_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "tree_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "files": [],
        "needs_json": null,
        "needs_status": null,
        "needs_exit_code": null,
        "needs_warnings": null
    })
}

fn minimal_index(case_record: serde_json::Value) -> serde_json::Value {
    json!({
        "schema_version": 1,
        "generator": "html-oracle/1",
        "profiles": {
            "core": {
                "sphinx": "9.1.0",
                "docutils": "0.22.4",
                "platform": "linux",
                "needs_version": null,
                "needs_commit": null,
                "needs_tree": null,
                "lock_path": "tools/oracle_profiles/core/uv.lock",
                "lock_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "determinism_shims": ["uuid.uuid4=counter"]
            }
        },
        "cases": [case_record]
    })
}

fn minimal_local_index(case_record: serde_json::Value) -> serde_json::Value {
    json!({
        "schema_version": 1,
        "generator": "html-oracle/1",
        "profiles": {
            "local_needs": {
                "sphinx": "9.1.0",
                "docutils": "0.21.2",
                "platform": "linux",
                "needs_version": "8.5.0",
                "needs_commit": "58bcb59d861da95f2aca79f343e8bae6ec5c1250",
                "needs_tree": "958172a89defcec69704f6b9d61e482e7c4e8409",
                "lock_path": "tools/oracle_profiles/local_needs/uv.lock",
                "lock_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "determinism_shims": ["uuid.uuid4=counter", "needs_reproducible_json=1"]
            }
        },
        "cases": [case_record]
    })
}

#[test]
fn schema_deserializes_complete_index_document() {
    let document = IndexDocument::from_value(minimal_index(minimal_case()))
        .expect("complete schema should deserialize and validate");
    assert_eq!(document.schema_version, 1);
    assert_eq!(document.cases.len(), 1);
    assert_eq!(document.profiles["core"].platform, "linux");
}

#[test]
fn schema_rejects_missing_profile_platform() {
    let mut index = minimal_index(minimal_case());
    index["profiles"]["core"]
        .as_object_mut()
        .unwrap()
        .remove("platform");
    assert!(IndexDocument::from_value(index).is_err());
}

#[test]
fn schema_keeps_exception_type_null_for_built_case() {
    let document = IndexDocument::from_value(minimal_index(minimal_case())).unwrap();
    assert!(document.cases[0].exception_type.is_none());
}

#[test]
fn schema_rejects_exception_type_for_built_case() {
    let mut case = minimal_case();
    case["exception_type"] = json!("TypeError");
    assert!(IndexDocument::from_value(minimal_index(case)).is_err());
}

#[test]
fn schema_allows_nullable_build_error_exception_type() {
    let mut case = minimal_case();
    case["status"] = json!("build-error");
    case["exit_code"] = json!(1);
    assert!(IndexDocument::from_value(minimal_index(case)).is_ok());
}

#[test]
fn schema_accepts_typed_build_error_and_reference_crash() {
    let mut build_error = minimal_case();
    build_error["status"] = json!("build-error");
    build_error["exception_type"] = json!("sphinx.errors.ConfigError");
    build_error["exit_code"] = json!(1);
    assert!(IndexDocument::from_value(minimal_index(build_error)).is_ok());

    let mut reference_crash = minimal_case();
    reference_crash["status"] = json!("reference-crash");
    reference_crash["exception_type"] = json!("TypeError");
    reference_crash["exit_code"] = json!(1);
    assert!(IndexDocument::from_value(minimal_index(reference_crash)).is_ok());
}

#[test]
fn schema_rejects_reference_crash_without_exception_type() {
    let mut case = minimal_case();
    case["status"] = json!("reference-crash");
    case["exit_code"] = json!(1);
    assert!(IndexDocument::from_value(minimal_index(case)).is_err());
}

#[test]
fn schema_rejects_unknown_status() {
    let mut case = minimal_case();
    case["status"] = json!("not-a-status");
    assert!(IndexDocument::from_value(minimal_index(case)).is_err());
}

#[test]
fn schema_rejects_missing_input_files() {
    let mut case = minimal_case();
    case.as_object_mut().unwrap().remove("input_files");
    assert!(IndexDocument::from_value(minimal_index(case)).is_err());
}

#[test]
fn schema_rejects_missing_input_hash() {
    let mut case = minimal_case();
    case.as_object_mut().unwrap().remove("input_sha256");
    assert!(IndexDocument::from_value(minimal_index(case)).is_err());
}

#[test]
fn schema_rejects_path_escape() {
    let mut case = minimal_case();
    case["input_files"] = json!([{
        "logical_path": "index.rst",
        "storage": "input",
        "storage_path": "inputs/../outside/index.rst",
        "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "size": 0
    }]);
    assert!(IndexDocument::from_value(minimal_index(case)).is_err());
}

#[test]
fn schema_rejects_hash_mismatch() {
    let mut case = minimal_case();
    case["input_files"] = json!([{
        "logical_path": "index.rst",
        "storage": "input",
        "storage_path": "inputs/synthetic/case-1/index.rst",
        "sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "size": 5
    }]);
    let root = tempfile::tempdir().unwrap();
    let profile_root = root.path().join("core");
    std::fs::create_dir_all(profile_root.join("inputs/synthetic/case-1")).unwrap();
    std::fs::write(
        profile_root.join("inputs/synthetic/case-1/index.rst"),
        b"hello",
    )
    .unwrap();
    let index_path = profile_root.join("index.json");
    std::fs::write(
        &index_path,
        serde_json::to_vec(&minimal_index(case)).unwrap(),
    )
    .unwrap();
    assert!(IndexDocument::load(&index_path).is_err());
}

#[test]
fn schema_rejects_duplicate_case_key() {
    let first = minimal_case();
    let second = minimal_case();
    let mut index = minimal_index(first);
    index["cases"] = json!([second, minimal_case()]);
    assert!(IndexDocument::from_value(index).is_err());
}

#[test]
fn schema_rejects_null_exit_code_for_built_case() {
    let mut case = minimal_case();
    case["exit_code"] = serde_json::Value::Null;
    assert!(IndexDocument::from_value(minimal_index(case)).is_err());
}

#[test]
fn schema_rejects_files_on_excluded_case() {
    let mut case = minimal_case();
    case["status"] = json!("excluded-network");
    case["exit_code"] = serde_json::Value::Null;
    case["excluded_reason"] = json!("synthetic exclusion");
    case["files"] = json!([{
        "logical_path": "index.html",
        "storage": "ref",
        "storage_path": "refs/synthetic/case-1/index.html",
        "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "size": 0
    }]);
    assert!(IndexDocument::from_value(minimal_index(case)).is_err());
}

#[test]
fn schema_keeps_needs_fields_null_for_core_cases() {
    let document = IndexDocument::from_value(minimal_index(minimal_case())).unwrap();
    let case = &document.cases[0];
    assert!(case.needs_json.is_none());
    assert!(case.needs_status.is_none());
    assert!(case.needs_exit_code.is_none());
    assert!(case.needs_warnings.is_none());
}

#[test]
fn schema_accepts_consistent_local_needs_second_build_fields() {
    let mut case = minimal_case();
    case["profile"] = json!("local_needs");
    case["needs_json"] = json!({
        "logical_path": "needs.json",
        "storage": "ref",
        "storage_path": "refs/sphinx_needs_doc_tests/case-1/needs/needs.json",
        "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "size": 2
    });
    case["needs_status"] = json!("built");
    case["needs_exit_code"] = json!(0);
    case["needs_warnings"] = json!("");
    let document = IndexDocument::from_value(minimal_local_index(case))
        .expect("local-needs second build fields should be accepted");
    assert!(document.cases[0].needs_json.is_some());
}

#[test]
fn schema_rejects_inconsistent_local_needs_second_build_fields() {
    let mut case = minimal_case();
    case["profile"] = json!("local_needs");
    case["needs_exit_code"] = json!(0);
    case["needs_warnings"] = json!("");
    assert!(IndexDocument::from_value(minimal_local_index(case)).is_err());
}

#[test]
fn needs_builder_failure_is_reported_as_its_own_category() {
    let diagnostic = support::html_oracle::needs_builder_diagnostic(
        CaseStatus::Built,
        CaseStatus::BuildError,
        true,
    )
    .expect("built-to-empty-build-error should be a builder mismatch");
    assert_eq!(diagnostic.category, "needs-builder");
    assert_eq!(diagnostic.logical_path, "needs.json");
}

#[test]
fn comparator_normalizes_crlf_text_but_not_buildinfo() {
    assert!(compare_file("index.html", b"a\r\nb\r\n", b"a\nb\n", None).is_empty());
    assert_eq!(
        compare_file("environment.buildinfo", b"a\r\nb", b"a\nb", None)[0].category,
        "bytes-value"
    );
    assert_eq!(
        support::html_oracle::policy_for_path("index.html"),
        Policy::TextCrlf
    );
    assert_eq!(
        support::html_oracle::policy_for_path("environment.buildinfo"),
        Policy::ExactBytes
    );
}

#[test]
fn comparator_replaces_source_root_only_for_warnings() {
    let expected = b"/oracle/source/index.rst:1: WARNING: issue\r\n";
    let actual = b"C:\\run\\source/index.rst:1: WARNING: issue\n";
    assert!(compare_warnings(
        expected,
        actual,
        Some(Path::new("/oracle/source")),
        Some(Path::new("C:\\run\\source")),
    )
    .is_empty());
    assert_eq!(
        compare_file("index.html", b"/oracle/source", b"C:\\run\\source", None)[0].category,
        "text-value"
    );
}

#[test]
fn comparator_replaces_source_root_in_text_and_searchindex_only() {
    let source_root = Path::new("C:/run/source");
    let text_expected = b"path=<SRCDIR>/index.rst\nliteral=C:/run/source-code\n";
    let text_actual = b"path=C:/run/source/index.rst\nliteral=C:/run/source-code\n";
    assert!(compare_file("index.html", text_expected, text_actual, Some(source_root)).is_empty());

    let search_expected = br#"Search.setIndex({"filenames":["<SRCDIR>/index"]});"#;
    let search_actual = br#"Search.setIndex({"filenames":["C:/run/source/index"]});"#;
    assert!(compare_file(
        "searchindex.js",
        search_expected,
        search_actual,
        Some(source_root)
    )
    .is_empty());

    let windows_root = Path::new(r"C:\run\source");
    let windows_search_expected = br#"Search.setIndex({"filenames":["<SRCDIR>\\index"]});"#;
    let windows_search_actual = br#"Search.setIndex({"filenames":["C:\\run\\source\\index"]});"#;
    assert!(compare_file(
        "searchindex.js",
        windows_search_expected,
        windows_search_actual,
        Some(windows_root)
    )
    .is_empty());

    let windows_text_expected = br#"path=<SRCDIR>\\index.rst"#;
    let windows_text_actual = br#"path=C:\\run\\source\\index.rst"#;
    assert!(compare_file(
        "index.html",
        windows_text_expected,
        windows_text_actual,
        Some(windows_root)
    )
    .is_empty());
}

#[test]
fn comparator_normalizes_all_runtime_tokens_in_warnings() {
    let expected = b"<SRCDIR>/index.rst <OUTDIR>/index.html <DOCTREEDIR>/index.doctree <CASEDIR>\\warnings.txt <SPHINX_ERR_LOG>\n";
    let actual = br#"C:\run\case\source/index.rst C:\run\case\output/index.html C:\run\case\doctree/index.doctree C:\run\case\warnings.txt C:\tmp\sphinx-err-abc.log
"#;
    let roots = support::html_oracle::WarningRoots {
        source_root: Some(Path::new(r"C:\run\case\source")),
        output_root: Some(Path::new(r"C:\run\case\output")),
        doctree_root: Some(Path::new(r"C:\run\case\doctree")),
        case_root: Some(Path::new(r"C:\run\case")),
    };
    let diagnostics = compare_warnings_with_roots(expected, actual, None, Some(roots));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn comparator_reduces_real_sphinx_error_report() {
    let expected = String::from_utf8_lossy(include_bytes!(
        "fixtures/html_oracle/core/refs/html_projects/literalinclude/warnings.txt"
    ))
    .replace(
        "/home/runner/work/sphinx-ultra/sphinx-ultra/target/html-oracle/generator-work/case-2usubfn1/source",
        "<SRCDIR>",
    );
    let actual = br#"
Configuration error!

Versions
========

* Platform:         windows; (Windows-11)
* Python version:   3.12.15 (CPython)

Last Messages
=============

None.

Loaded Extensions
=================

None.

Traceback
=========

      File "C:\run\site-packages\sphinx\config.py", line 349, in read
        raise ConfigError(
    sphinx.errors.ConfigError: config directory doesn't contain a conf.py file (C:\run\case\source)


The full traceback has been saved in:
/tmp/sphinx-err-actual.log

To report this error to the developers, please open an issue.
"#;
    let roots = support::html_oracle::WarningRoots {
        source_root: Some(Path::new(r"C:\run\case\source")),
        output_root: Some(Path::new(r"C:\run\case\output")),
        doctree_root: Some(Path::new(r"C:\run\case\doctree")),
        case_root: Some(Path::new(r"C:\run\case")),
    };
    let diagnostics = compare_warnings_with_roots(expected.as_bytes(), actual, None, Some(roots));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn comparator_reduces_real_crash_reports_on_ultra_side() {
    let fixtures: &[(&[u8], &str)] = &[
        (
            include_bytes!("fixtures/sphinx_error_reports/docutils-0500.txt").as_slice(),
            "ValueError: list.remove(x): x not in list",
        ),
        (
            include_bytes!("fixtures/sphinx_error_reports/numfig_on.txt").as_slice(),
            "TypeError: not all arguments converted during string formatting",
        ),
        (
            include_bytes!("fixtures/sphinx_error_reports/doc_service_github.txt").as_slice(),
            "RuntimeError: network disabled by html oracle",
        ),
    ];
    for (expected, exception_line) in fixtures {
        let actual = String::from_utf8_lossy(expected)
            .replace("Linux-6.17.0-1022-azure-x86_64-with-glibc2.39", "Windows-11")
            .replace(
                "/home/runner/work/sphinx-ultra/sphinx-ultra/tools/oracle_profiles/core/.venv/lib/python3.12/site-packages/",
                r"C:\run\.venv\Lib\site-packages\\",
            )
            .replace(
                "/home/runner/work/sphinx-ultra/sphinx-ultra/tools/html_oracle_runner.py",
                r"C:\run\html_oracle_runner.py",
            );
        assert!(String::from_utf8_lossy(expected).contains(exception_line));
        assert!(actual.contains(exception_line));
        let diagnostics = compare_warnings_with_roots(expected, actual.as_bytes(), None, None);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}

#[test]
fn comparator_parses_searchindex_and_needs_json_with_key_order_ignored() {
    let search_expected = br#"Search.setIndex({"docnames":["a"],"titles":["A"]});"#;
    let search_actual = br#"Search.setIndex({"titles":["A"],"docnames":["a"]});"#;
    assert!(compare_file("searchindex.js", search_expected, search_actual, None).is_empty());

    let needs_expected = br#"{"versions":[{"needs":{"N-1":{"title":"One","status":"open"}}}]}"#;
    let needs_actual = br#"{"versions":[{"needs":{"N-1":{"status":"open","title":"One"}}}]}"#;
    assert!(compare_file("needs.json", needs_expected, needs_actual, None).is_empty());
}

#[test]
fn comparator_preserves_array_order_and_rejects_malformed_wrappers() {
    let expected = br#"Search.setIndex({"docnames":["a","b"]});"#;
    let reordered = br#"Search.setIndex({"docnames":["b","a"]});"#;
    assert_eq!(
        compare_file("searchindex.js", expected, reordered, None)[0].category,
        "searchindex-value"
    );
    let malformed = br#"{"docnames":[]}"#;
    assert_eq!(
        compare_file("searchindex.js", expected, malformed, None)[0].category,
        "invalid-searchindex"
    );
}

fn inventory_bytes(records: &str) -> Vec<u8> {
    let header = b"# Sphinx inventory version 2\n# Project: synthetic\n# Version: 1\n# The remainder of this file is compressed using zlib.\n";
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(records.as_bytes()).unwrap();
    let mut bytes = header.to_vec();
    bytes.extend(encoder.finish().unwrap());
    bytes
}

#[test]
fn comparator_canonicalizes_inventory_records_and_keeps_opaque_bytes_exact() {
    let first = inventory_bytes("alpha py:function 1 a.html -\nbeta py:function 1 b.html Beta\n");
    let second = inventory_bytes("beta py:function 1 b.html Beta\nalpha py:function 1 a.html -\n");
    assert!(compare_file("objects.inv", &first, &second, None).is_empty());
    assert_eq!(
        compare_file("image.png", b"\x00\x01", b"\x00\x02", None)[0].category,
        "bytes-value"
    );
}

#[test]
fn comparator_reports_missing_and_unexpected_files() {
    let expected = BTreeMap::from([(String::from("index.html"), b"same".to_vec())]);
    let actual = BTreeMap::from([
        (String::from("index.html"), b"same".to_vec()),
        (String::from("extra.css"), b"extra".to_vec()),
    ]);
    let diagnostics = compare_trees(&expected, &actual, None, None, None, None);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].category, "unexpected-file");
    assert_eq!(diagnostics[0].logical_path, "extra.css");

    let actual = BTreeMap::new();
    let diagnostics = compare_trees(&expected, &actual, None, None, None, None);
    assert_eq!(diagnostics[0].category, "missing-file");
}

#[test]
fn diagnostic_synthetic_html_localization_classifies_body_and_chrome() {
    let expected = "<html><div class=\"body\" role=\"main\">\nA\n</div><footer>ok</footer></html>";
    let body_changed =
        "<html><div class=\"body\" role=\"main\">\nB\n</div><footer>ok</footer></html>";
    let chrome_changed =
        "<html><div class=\"body\" role=\"main\">\nA\n</div><footer>changed</footer></html>";
    let both_changed =
        "<html><div class=\"body\" role=\"main\">\nB\n</div><footer>changed</footer></html>";
    let unstructured = "<html><main>A</main></html>";
    assert_eq!(diagnose_html(expected, body_changed).category, "html-body");
    assert_eq!(
        diagnose_html(expected, chrome_changed).category,
        "html-chrome"
    );
    assert_eq!(diagnose_html(expected, both_changed).category, "html-both");
    assert_eq!(
        diagnose_html(expected, unstructured).category,
        "html-body-fallback"
    );
}

#[test]
fn diagnostic_synthetic_html_falls_back_to_actual_main_region() {
    let expected = "<html><div class=\"body\" role=\"main\">\nA\n</div><footer>ok</footer></html>";
    let actual = "<html><header>different</header><main>\nB\n</main><footer>ok</footer></html>";
    let diagnostic = diagnose_html(expected, actual);
    assert_eq!(diagnostic.category, "html-body-fallback");
    assert!(diagnostic.detail.contains("-A"));
    assert!(diagnostic.detail.contains("+B"));
}

#[test]
fn diagnostic_html_fallback_prefers_role_main_before_main_and_body() {
    let expected = "<html><div class=\"body\" role=\"main\">\nA\n</div></html>";
    let actual =
        "<html><aside role=\"main\">\nROLE\n</aside><main>\nMAIN\n</main><body>BODY</body></html>";
    let diagnostic = diagnose_html(expected, actual);
    assert_eq!(diagnostic.category, "html-body-fallback");
    assert!(diagnostic.detail.contains("+ROLE"));
    assert!(!diagnostic.detail.contains("+MAIN"));
}

#[test]
fn diagnostic_synthetic_needs_json_is_keyed_by_need_id() {
    let expected = json!({
        "versions": [{
            "version": "1",
            "needs": {
                "N-1": {"title": "One", "status": "open"},
                "N-2": {"title": "Two"}
            }
        }]
    });
    let actual = json!({
        "versions": [{
            "version": "1",
            "needs": {
                "N-1": {"title": "Changed", "status": "open"},
                "N-3": {"title": "Three"}
            }
        }],
        "extra": true
    });
    let diagnostics = diagnose_needs_json(&expected, &actual);
    assert!(diagnostics.iter().any(|item| {
        item.category == "missing-need" && item.logical_path.contains("needs[\"N-2\"]")
    }));
    assert!(diagnostics.iter().any(|item| {
        item.category == "extra-need" && item.logical_path.contains("needs[\"N-3\"]")
    }));
    assert!(diagnostics.iter().any(|item| {
        item.category == "need-field"
            && item.logical_path == "versions[0].needs[\"N-1\"]"
            && item.detail.contains("title")
    }));
    assert!(diagnostics
        .iter()
        .any(|item| item.category == "needs-top-level"));
    assert_eq!(
        diagnose_needs_json(&json!({"versions": "bad"}), &actual)[0].category,
        "needs-json-path"
    );
}

#[test]
fn diagnostic_synthetic_structured_files_report_key_and_record_differences() {
    let expected_search = json!({"docnames": ["a"], "missing": false, "titles": ["A"]});
    let actual_search = json!({"docnames": ["a"], "extra": true, "titles": ["B"]});
    let search = diagnose_searchindex(&expected_search, &actual_search);
    assert!(search
        .iter()
        .any(|item| item.category == "searchindex-missing-key"));
    assert!(search
        .iter()
        .any(|item| item.category == "searchindex-extra-key"));
    assert!(search
        .iter()
        .any(|item| item.category == "searchindex-changed-key"));

    let record = |name: &str, uri: &str, display_name: &str| InventoryRecord {
        name: name.to_string(),
        domain_role: "py:function".to_string(),
        priority: 1,
        uri: uri.to_string(),
        display_name: display_name.to_string(),
    };
    let inventory = diagnose_inventory(
        &[
            record("missing", "missing.html", "-"),
            record("changed", "old.html", "Old"),
        ],
        &[
            record("extra", "extra.html", "-"),
            record("changed", "new.html", "New"),
        ],
    );
    assert!(inventory
        .iter()
        .any(|item| item.category == "inventory-missing-record"));
    assert!(inventory
        .iter()
        .any(|item| item.category == "inventory-extra-record"));
    assert!(inventory
        .iter()
        .any(|item| item.category == "inventory-changed-record"));
}

#[test]
fn diagnostic_synthetic_warnings_and_first_divergence_are_grouped() {
    let warnings = diagnose_warnings("one\r\ntwo\nthree\n", "one\ntwo-changed\n");
    assert!(warnings
        .iter()
        .any(|item| item.category == "warning-changed-line"));
    assert!(warnings
        .iter()
        .any(|item| item.category == "warning-missing-line"));
    let diagnostics = (0..3)
        .map(|index| support::html_oracle::Diagnostic {
            category: "html-body".to_string(),
            logical_path: format!("page-{index}.html"),
            first_expected_line: Some(7),
            expected: "same expected\nrest".to_string(),
            actual: "same actual\nrest".to_string(),
            detail: String::new(),
        })
        .collect::<Vec<_>>();
    let mut diagnostics = diagnostics;
    diagnostics.push(support::html_oracle::Diagnostic {
        category: "html-body".to_string(),
        logical_path: "page-0.html".to_string(),
        first_expected_line: Some(7),
        expected: "same expected\nrest".to_string(),
        actual: "same actual\nrest".to_string(),
        detail: String::new(),
    });
    let groups = group_first_divergences(&diagnostics);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].expected_text, "same expected");
    assert_eq!(groups[0].actual_text, "same actual");
    assert_eq!(groups[0].expected_line, Some(7));
    assert_eq!(groups[0].count, 4);
    assert_eq!(groups[0].sample_files.len(), 3);
}

#[test]
fn diagnostic_first_divergence_groups_by_content_not_line_number() {
    let diagnostics = vec![
        support::html_oracle::Diagnostic {
            category: "html-body".to_string(),
            logical_path: "one.html".to_string(),
            first_expected_line: Some(1),
            expected: "one\nrest".to_string(),
            actual: "uno\nrest".to_string(),
            detail: String::new(),
        },
        support::html_oracle::Diagnostic {
            category: "html-body".to_string(),
            logical_path: "two.html".to_string(),
            first_expected_line: Some(9),
            expected: "one\nother".to_string(),
            actual: "uno\nother".to_string(),
            detail: String::new(),
        },
        support::html_oracle::Diagnostic {
            category: "html-body".to_string(),
            logical_path: "three.html".to_string(),
            first_expected_line: Some(9),
            expected: "two\nrest".to_string(),
            actual: "dos\nrest".to_string(),
            detail: String::new(),
        },
    ];
    let groups = group_first_divergences(&diagnostics);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].count, 2);
    assert_eq!(groups[0].expected_text, "one");
    assert_eq!(groups[0].actual_text, "uno");
}

#[test]
fn diagnostic_synthetic_keep_and_filter_policy_defaults_and_rejects_invalid_values() {
    assert_eq!(parse_keep(None).unwrap().as_str(), "failed");
    assert_eq!(parse_keep(Some("all")).unwrap().as_str(), "all");
    assert!(parse_keep(Some("passing")).is_err());
    assert_eq!(
        support::html_oracle::rerun_filter("core/synthetic/case-1", Some("synthetic")),
        Some("core/synthetic/case-1".to_string())
    );
    assert_eq!(
        support::html_oracle::rerun_filter("core/synthetic/case-1", Some("other")),
        None
    );
    assert_eq!(
        resolve_ultra_binary(Some("C:/fix/sphinx-ultra.exe"), "built-in"),
        "C:/fix/sphinx-ultra.exe"
    );
    assert_eq!(resolve_ultra_binary(Some(""), "built-in"), "built-in");
}

fn diagnostics_helper_command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg("diagnostics_helper")
        .arg("--ignored")
        .arg("--nocapture")
        .env("HTML_ORACLE_DIAGNOSTICS_HELPER", mode);
    command
}

#[test]
#[ignore]
fn diagnostics_helper() {
    match std::env::var("HTML_ORACLE_DIAGNOSTICS_HELPER").as_deref() {
        Ok("sleep") => std::thread::sleep(Duration::from_secs(5)),
        Ok("flood") => {
            let block = vec![b'x'; 8192];
            let mut stdout = std::io::stdout().lock();
            let mut stderr = std::io::stderr().lock();
            for _ in 0..128 {
                stdout.write_all(&block).unwrap();
                stderr.write_all(&block).unwrap();
            }
        }
        _ => {}
    }
}

#[test]
fn diagnostics_timeout_is_reported_without_hanging() {
    let result = run_bounded_with_timeout(
        diagnostics_helper_command("sleep"),
        Duration::from_millis(100),
    );
    assert!(matches!(result.status, ExitStatusKind::Timeout));
}

#[test]
fn diagnostics_caps_both_output_streams_after_draining_them() {
    let result =
        run_bounded_with_timeout(diagnostics_helper_command("flood"), Duration::from_secs(5));
    assert!(matches!(result.status, ExitStatusKind::Success));
    assert!(result.stdout.len() <= 256 * 1024 + 32);
    assert!(result.stderr.len() <= 256 * 1024 + 32);
    assert!(result.stdout.contains("[output truncated]"));
    assert!(result.stderr.contains("[output truncated]"));
}

#[test]
fn diagnostics_reports_spawn_errors() {
    let command = Command::new("html-oracle-command-that-does-not-exist");
    let result = run_bounded_with_timeout(command, Duration::from_millis(100));
    assert!(matches!(result.status, ExitStatusKind::SpawnError(_)));
}

fn report_case(
    profile: &str,
    source_set: &str,
    case_id: &str,
    passed: bool,
    category: Option<&str>,
    line: Option<usize>,
) -> CaseResult {
    CaseResult {
        profile: profile.to_string(),
        source_set: source_set.to_string(),
        case_id: case_id.to_string(),
        html_status: "built".to_string(),
        exception_type: None,
        needs_status: None,
        passed,
        run_dir: format!("target/html-oracle/runs/{profile}/{source_set}/{case_id}"),
        rerun_filter: format!("{profile}/{source_set}/{case_id}"),
        common_mismatch_count: 0,
        diagnostics: category
            .map(|category| {
                vec![support::html_oracle::Diagnostic {
                    category: category.to_string(),
                    logical_path: format!("{case_id}.html"),
                    first_expected_line: line,
                    expected: "expected".to_string(),
                    actual: "actual".to_string(),
                    detail: "synthetic".to_string(),
                }]
            })
            .unwrap_or_default(),
    }
}

fn report_case_with_diagnostic(
    profile: &str,
    source_set: &str,
    case_id: &str,
    logical_path: &str,
    category: &str,
    expected: &str,
    actual: &str,
) -> CaseResult {
    let mut result = report_case(profile, source_set, case_id, false, None, None);
    result.diagnostics = vec![support::html_oracle::Diagnostic {
        category: category.to_string(),
        logical_path: logical_path.to_string(),
        first_expected_line: Some(1),
        expected: expected.to_string(),
        actual: actual.to_string(),
        detail: "synthetic".to_string(),
    }];
    result
}

#[test]
fn report_compacts_diagnostics_and_aggregates_common_mismatches() {
    let expected = "expected text\n".repeat(100_000);
    let actual = "actual text\n".repeat(100_000);
    let results = vec![
        report_case_with_diagnostic(
            "core",
            "set-a",
            "case-1",
            "_static/alabaster.css",
            "text-value",
            &expected,
            &actual,
        ),
        report_case_with_diagnostic(
            "core",
            "set-b",
            "case-2",
            "_static/alabaster.css",
            "text-value",
            &expected,
            &actual,
        ),
    ];
    let (report, markdown) = build_report_with_platforms_reference_cases_and_bin(
        2,
        0,
        0,
        &results,
        &BTreeMap::new(),
        &[],
        Some("C:/ultra/sphinx-ultra.exe"),
    );
    let serialized = serde_json::to_vec(&report).unwrap();
    assert!(serialized.len() < 1_000_000);
    let common = &report["common_mismatches"][0];
    assert_eq!(common["count"], 2);
    assert_eq!(common["source_set_count"], 2);
    assert!(report["cases"][0]["diagnostics"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(report["cases"][0]["common_mismatch_count"], 1);
    assert!(common["diff"].as_str().unwrap().len() <= 64 * 1024);
    assert!(common["expected_sha256"].as_str().unwrap().len() == 64);
    assert!(common["actual_sha256"].as_str().unwrap().len() == 64);
    assert_eq!(common["expected_size"], expected.len());
    assert_eq!(common["actual_size"], actual.len());
    assert!(markdown.contains("## Common mismatches"));
    assert!(markdown.contains("+1 common mismatches (see Common mismatches)"));
    assert!(markdown.contains("Ultra binary: `C:/ultra/sphinx-ultra.exe`"));
}

#[test]
fn report_counts_failing_cases_by_file_kind() {
    let results = vec![
        report_case_with_diagnostic(
            "core",
            "synthetic",
            "html",
            "index.html",
            "html-body",
            "a",
            "b",
        ),
        report_case_with_diagnostic(
            "core",
            "synthetic",
            "static",
            "_static/app.css",
            "text-value",
            "a",
            "b",
        ),
        report_case_with_diagnostic(
            "core",
            "synthetic",
            "needs",
            "needs.json",
            "needs-top-level",
            "a",
            "b",
        ),
    ];
    let (report, _) = support::html_oracle::build_report(3, 0, 0, &results);
    assert_eq!(report["failing_cases_by_file_kind"]["html"], 1);
    assert_eq!(report["failing_cases_by_file_kind"]["static"], 1);
    assert_eq!(report["failing_cases_by_file_kind"]["needs.json"], 1);
    assert_eq!(report["failing_cases_by_file_kind"]["searchindex"], 0);
}

#[test]
fn report_synthetic_data_is_sorted_and_contains_summary_counts() {
    let results = vec![
        report_case(
            "local_needs",
            "z-set",
            "z-case",
            false,
            Some("html-body"),
            Some(7),
        ),
        report_case("core", "a-set", "a-case", true, None, None),
        report_case("core", "a-set", "b-case", false, Some("warning"), Some(2)),
    ];
    let (report, markdown) = support::html_oracle::build_report(5, 1, 1, &results);
    assert_eq!(report["total_cases"], 5);
    assert_eq!(report["scheduled_cases"], 3);
    assert_eq!(report["passed_cases"], 1);
    assert_eq!(report["failed_cases"], 2);
    assert_eq!(report["excluded_cases"], 1);
    assert_eq!(report["reference_crash_cases"], 1);
    assert_eq!(report["cases"][0]["rerun_filter"], "core/a-set/a-case");
    assert_eq!(report["cases"][1]["rerun_filter"], "core/a-set/b-case");
    assert_eq!(report["counts_by_category"]["html-body"], 1);
    assert_eq!(report["counts_by_category"]["warning"], 1);
    assert!(report["cases"][1]["diagnostics"][0]["expected"].is_null());
    assert_eq!(
        report["cases"][1]["diagnostics"][0]["expected_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(markdown.contains("## Summary"));
    assert!(markdown.contains("## Category counts"));
    assert!(markdown.contains("## Most common first-divergence"));
    assert!(markdown.contains("## core/a-set/a-case"));
}

#[test]
fn report_prints_reference_platform_and_host_difference_note() {
    let results = vec![report_case("core", "synthetic", "case", true, None, None)];
    let platforms = BTreeMap::from([(String::from("core"), String::from("reference-test"))]);
    let (_report, markdown) =
        support::html_oracle::build_report_with_platforms(1, 0, 0, &results, &platforms);
    assert!(markdown.contains("| core | reference-test |"));
    assert!(markdown.contains("Reference platform: `reference-test`"));
    assert!(markdown.contains("Separator and path differences are expected when"));
}

#[test]
fn report_places_exception_type_next_to_build_error_and_reference_crash_status() {
    let mut build_error = report_case("core", "synthetic", "build-error", false, None, None);
    build_error.html_status = "build-error".to_string();
    build_error.exception_type = Some("sphinx.errors.ConfigError".to_string());
    let reference_crash = support::html_oracle::ReferenceCase {
        profile: "core".to_string(),
        source_set: "synthetic".to_string(),
        case_id: "reference-crash".to_string(),
        status: "reference-crash".to_string(),
        exception_type: Some("TypeError".to_string()),
    };
    let (report, markdown) = support::html_oracle::build_report_with_platforms_and_reference_cases(
        2,
        0,
        1,
        &[build_error],
        &BTreeMap::new(),
        &[reference_crash],
    );
    assert_eq!(report["cases"][0]["html_status"], "build-error");
    assert_eq!(
        report["cases"][0]["exception_type"],
        "sphinx.errors.ConfigError"
    );
    assert_eq!(report["reference_cases"][0]["status"], "reference-crash");
    assert_eq!(report["reference_cases"][0]["exception_type"], "TypeError");
    assert!(markdown.contains("status: build-error (exception_type: `sphinx.errors.ConfigError`)"));
    assert!(markdown.contains("| reference-crash | TypeError |"));
}

#[test]
fn report_synthetic_first_divergences_are_capped_at_twenty_five() {
    let results = (0..30)
        .map(|index| {
            report_case_with_diagnostic(
                "core",
                "synthetic",
                &format!("case-{index:02}"),
                &format!("page-{index:02}.html"),
                "html-body",
                &format!("expected-{index}"),
                &format!("actual-{index}"),
            )
        })
        .collect::<Vec<_>>();
    let (report, markdown) = support::html_oracle::build_report(30, 0, 0, &results);
    assert_eq!(report["first_divergences"].as_array().unwrap().len(), 25);
    assert_eq!(report["first_divergences"][0]["expected_line"], 1);
    assert!(markdown.contains("| 1 | expected-0 | actual-0 | 1 |"));
}

#[test]
fn report_assertion_text_is_capped_and_points_to_both_files() {
    let message = support::html_oracle::bounded_assertion_message(
        Path::new("target/html-oracle/report.md"),
        Path::new("target/html-oracle/report.json"),
        &"x".repeat(100 * 1024),
    );
    assert!(message.len() <= 64 * 1024 + 128);
    assert!(message.contains("target/html-oracle/report.md"));
    assert!(message.contains("target/html-oracle/report.json"));
    assert!(message.contains("[output truncated]"));
}

#[test]
fn report_retention_deletes_only_passing_failed_runs() {
    let root = tempfile::tempdir().unwrap();
    let passing = root.path().join("passing");
    let failing = root.path().join("failing");
    std::fs::create_dir_all(&passing).unwrap();
    std::fs::create_dir_all(&failing).unwrap();
    support::html_oracle::apply_retention(&passing, true, Keep::Failed).unwrap();
    support::html_oracle::apply_retention(&failing, false, Keep::Failed).unwrap();
    assert!(!passing.exists());
    assert!(failing.exists());

    let all = root.path().join("all");
    std::fs::create_dir_all(&all).unwrap();
    support::html_oracle::apply_retention(&all, true, Keep::All).unwrap();
    assert!(all.exists());
}

#[test]
fn missing_fixture_corpus_has_a_clear_error() {
    let root = tempfile::tempdir().unwrap();
    let error = support::html_oracle::load_fixture_suite(root.path())
        .expect_err("missing fixture corpus should be reported");
    assert!(error.contains("core/index.json"));
    assert!(error.contains("local_needs/index.json"));
}

#[test]
#[ignore]
/// Runs all committed HTML oracle cases.
///
/// Set `HTML_ORACLE_ULTRA_BIN` to an alternate Ultra executable for fix branches.
fn html_oracle_exhaustive() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/html_oracle");
    let suite = load_fixture_suite(&fixture_root)
        .unwrap_or_else(|error| panic!("cannot run HTML oracle exhaustive test: {error}"));
    let keep = parse_keep(std::env::var("HTML_ORACLE_KEEP").ok().as_deref())
        .unwrap_or_else(|error| panic!("{error}"));
    let filter = std::env::var("HTML_ORACLE_FILTER").ok();
    let ultra_bin = selected_ultra_binary();
    let all_cases = suite
        .profiles
        .values()
        .flat_map(|document| document.cases.iter().cloned())
        .collect::<Vec<_>>();
    let reference_cases = all_cases
        .iter()
        .filter(|case| case.status == CaseStatus::ReferenceCrash)
        .map(|case| support::html_oracle::ReferenceCase {
            profile: case.profile.clone(),
            source_set: case.source_set.clone(),
            case_id: case.case_id.clone(),
            status: status_name(case.status).to_string(),
            exception_type: case.exception_type.clone(),
        })
        .collect::<Vec<_>>();
    let reference_platforms = suite
        .profiles
        .iter()
        .map(|(profile, document)| {
            let platform = document
                .profiles
                .get(profile)
                .expect("fixture profile should be present")
                .platform
                .clone();
            (profile.clone(), platform)
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(filter) = &filter {
        if !all_cases.iter().any(|case| case_key(case).contains(filter)) {
            panic!("HTML_ORACLE_FILTER matched no ledger case: {filter}");
        }
    }
    let total_cases = all_cases.len();
    let excluded_cases = all_cases
        .iter()
        .filter(|case| case.status.is_excluded())
        .count();
    let reference_crash_cases = all_cases
        .iter()
        .filter(|case| case.status == CaseStatus::ReferenceCrash)
        .count();
    let jobs = all_cases
        .into_iter()
        .filter(|case| case.status.is_runnable())
        .filter(|case| {
            filter
                .as_deref()
                .is_none_or(|filter| case_key(case).contains(filter))
        })
        .collect::<Vec<_>>();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/html-oracle/runs");
    fs::create_dir_all(&run_root).expect("create HTML oracle run root");
    let worker_count = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1)
        .min(jobs.len().max(1));
    let queue = Arc::new(Mutex::new(VecDeque::from(jobs)));
    let (sender, receiver) = mpsc::channel();
    let mut workers = Vec::new();
    for _ in 0..worker_count {
        let queue = Arc::clone(&queue);
        let sender = sender.clone();
        let run_root = run_root.clone();
        let fixture_root = fixture_root.clone();
        workers.push(std::thread::spawn(move || loop {
            let case = queue.lock().unwrap().pop_front();
            let Some(case) = case else {
                break;
            };
            let profile_root = fixture_root.join(&case.profile);
            let result = run_html_case(&case, &profile_root, &run_root, keep);
            sender
                .send(result)
                .expect("report receiver should remain open");
        }));
    }
    drop(sender);
    let mut results = Vec::new();
    for result in receiver {
        results.push(result.unwrap_or_else(|error| panic!("HTML oracle case failed: {error}")));
    }
    for worker in workers {
        worker.join().expect("HTML oracle worker should not panic");
    }

    let (report, markdown) = build_report_with_platforms_reference_cases_and_bin(
        total_cases,
        excluded_cases,
        reference_crash_cases,
        &results,
        &reference_platforms,
        &reference_cases,
        Some(&ultra_bin),
    );
    let report_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/html-oracle");
    let report_json = report_root.join("report.json");
    let report_markdown = report_root.join("report.md");
    fs::write(
        &report_json,
        serde_json::to_vec_pretty(&report).expect("report JSON serializes"),
    )
    .expect("write report.json");
    fs::write(&report_markdown, markdown).expect("write report.md");
    if report["failed_cases"].as_u64().unwrap_or_default() != 0 {
        let details = serde_json::to_string_pretty(&report).unwrap_or_default();
        panic!(
            "{}",
            bounded_assertion_message(&report_markdown, &report_json, &details)
        );
    }
}

fn selected_ultra_binary() -> String {
    resolve_ultra_binary(
        std::env::var("HTML_ORACLE_ULTRA_BIN").ok().as_deref(),
        env!("CARGO_BIN_EXE_sphinx-ultra"),
    )
}

fn run_html_case(
    case: &CaseRecord,
    profile_root: &Path,
    run_root: &Path,
    keep: Keep,
) -> Result<CaseResult, String> {
    let run_dir = run_root
        .join(&case.profile)
        .join(&case.source_set)
        .join(&case.case_id);
    if run_dir.exists() {
        fs::remove_dir_all(&run_dir)
            .map_err(|error| format!("remove stale run {}: {error}", run_dir.display()))?;
    }
    let input_dir = run_dir.join("input");
    let expected_dir = run_dir.join("expected");
    let actual_dir = run_dir.join("actual");
    let cache_dir = run_dir.join("cache");
    let warnings_path = run_dir.join("actual-warnings.txt");
    let actual_needs_dir = run_dir.join("actual-needs");
    for directory in [
        &input_dir,
        &expected_dir,
        &actual_dir,
        &cache_dir,
        &actual_needs_dir,
    ] {
        fs::create_dir_all(directory)
            .map_err(|error| format!("create {}: {error}", directory.display()))?;
    }
    materialize_case_inputs(case, profile_root, &input_dir)?;
    let expected_tree = materialize_case_expected(case, profile_root, &expected_dir)?;
    fs::write(expected_dir.join("warnings.txt"), case.warnings.as_bytes())
        .map_err(|error| format!("write expected warnings: {error}"))?;

    let mut command = Command::new(selected_ultra_binary());
    command
        .arg(&input_dir)
        .arg(&actual_dir)
        .args(["-b", "html", "-d"])
        .arg(&cache_dir)
        .arg("-q")
        .args(["-w"])
        .arg(&warnings_path);
    let process = run_bounded(command);
    let actual_status = process_status(&process);
    let mut diagnostics = if actual_status.is_some() {
        compare_output_trees(
            &expected_tree,
            &walk_tree(&actual_dir)?,
            case.status,
            actual_status,
            Some(&input_dir),
        )
    } else {
        vec![process_diagnostic(&process)]
    };
    let actual_warnings = fs::read(&warnings_path).unwrap_or_default();
    diagnostics.extend(warning_diagnostics_with_roots(
        case.warnings.as_bytes(),
        &actual_warnings,
        None,
        Some(support::html_oracle::WarningRoots {
            source_root: Some(&input_dir),
            output_root: Some(&actual_dir),
            doctree_root: Some(&cache_dir),
            case_root: Some(&run_dir),
        }),
    ));
    let (needs_status, needs_diagnostics) = if case.needs_status.is_some() {
        run_needs_case(
            case,
            profile_root,
            &input_dir,
            &expected_dir,
            &actual_needs_dir,
            &run_dir,
        )?
    } else {
        (None, Vec::new())
    };
    diagnostics.extend(needs_diagnostics);
    diagnostics.sort_by(|left, right| {
        left.logical_path
            .cmp(&right.logical_path)
            .then_with(|| left.category.cmp(&right.category))
            .then_with(|| left.first_expected_line.cmp(&right.first_expected_line))
            .then_with(|| left.detail.cmp(&right.detail))
    });
    let passed = diagnostics.is_empty();
    let result = CaseResult {
        profile: case.profile.clone(),
        source_set: case.source_set.clone(),
        case_id: case.case_id.clone(),
        html_status: actual_status
            .map(status_name)
            .unwrap_or("io-error")
            .to_string(),
        exception_type: case.exception_type.clone(),
        needs_status: needs_status.map(status_name).map(str::to_string),
        passed,
        run_dir: run_dir.to_string_lossy().into_owned(),
        rerun_filter: case_key(case),
        common_mismatch_count: 0,
        diagnostics,
    };
    fs::write(
        run_dir.join("result.json"),
        serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write result.json: {error}"))?;
    apply_retention(&run_dir, passed, keep)?;
    Ok(result)
}

fn run_needs_case(
    case: &CaseRecord,
    profile_root: &Path,
    input_dir: &Path,
    expected_dir: &Path,
    actual_needs_dir: &Path,
    run_dir: &Path,
) -> Result<(Option<CaseStatus>, Vec<support::html_oracle::Diagnostic>), String> {
    let expected_status = case
        .needs_status
        .ok_or_else(|| format!("{} has no needs_status", case_key(case)))?;
    let mut expected_tree = BTreeMap::new();
    if let Some(record) = &case.needs_json {
        let bytes = read_record(record, profile_root)?;
        expected_tree.insert("needs.json".to_string(), bytes.clone());
        write_logical_file(&expected_dir.join("needs"), "needs.json", &bytes)?;
    }
    let needs_cache = run_dir.join("needs-cache");
    let needs_warnings = run_dir.join("actual-needs-warnings.txt");
    let mut command = Command::new(selected_ultra_binary());
    command
        .arg(input_dir)
        .arg(actual_needs_dir)
        .args(["-b", "needs", "-D", "needs_reproducible_json=1", "-d"])
        .arg(&needs_cache)
        .arg("-q")
        .args(["-w"])
        .arg(&needs_warnings);
    let process = run_bounded(command);
    let actual_status = process_status(&process);
    let mut diagnostics = if let Some(actual_status) = actual_status {
        let actual_tree = walk_tree(actual_needs_dir)?;
        if let Some(diagnostic) = support::html_oracle::needs_builder_diagnostic(
            expected_status,
            actual_status,
            actual_tree.is_empty(),
        ) {
            vec![diagnostic]
        } else {
            compare_output_trees(
                &expected_tree,
                &actual_tree,
                expected_status,
                Some(actual_status),
                Some(input_dir),
            )
        }
    } else {
        vec![process_diagnostic(&process)]
    };
    let actual_warnings = fs::read(&needs_warnings).unwrap_or_default();
    diagnostics.extend(warning_diagnostics_with_roots(
        case.needs_warnings
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
        &actual_warnings,
        None,
        Some(support::html_oracle::WarningRoots {
            source_root: Some(input_dir),
            output_root: Some(actual_needs_dir),
            doctree_root: Some(&needs_cache),
            case_root: Some(run_dir),
        }),
    ));
    Ok((actual_status, diagnostics))
}

fn process_status(process: &ProcessOutput) -> Option<CaseStatus> {
    match process.status {
        ExitStatusKind::Success => Some(CaseStatus::Built),
        ExitStatusKind::BuildError(_) => Some(CaseStatus::BuildError),
        ExitStatusKind::Timeout | ExitStatusKind::SpawnError(_) | ExitStatusKind::IoError(_) => {
            None
        }
    }
}

fn process_diagnostic(process: &ProcessOutput) -> support::html_oracle::Diagnostic {
    let (category, detail) = match &process.status {
        ExitStatusKind::Timeout => ("timeout", "Ultra process exceeded the deadline".to_string()),
        ExitStatusKind::SpawnError(error) => ("spawn", error.clone()),
        ExitStatusKind::IoError(error) => ("io", error.clone()),
        ExitStatusKind::Success | ExitStatusKind::BuildError(_) => {
            ("process", "unexpected process status".to_string())
        }
    };
    support::html_oracle::Diagnostic {
        category: category.to_string(),
        logical_path: String::new(),
        first_expected_line: None,
        expected: String::new(),
        actual: format!("{}\n{}", process.stdout, process.stderr),
        detail,
    }
}

fn compare_output_trees(
    expected: &BTreeMap<String, Vec<u8>>,
    actual: &BTreeMap<String, Vec<u8>>,
    expected_status: CaseStatus,
    actual_status: Option<CaseStatus>,
    source_root: Option<&Path>,
) -> Vec<support::html_oracle::Diagnostic> {
    let mut paths = expected
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    paths.extend(actual.keys().cloned());
    let mut diagnostics = Vec::new();
    for path in paths {
        match (expected.get(&path), actual.get(&path)) {
            (Some(expected), Some(actual)) => {
                diagnostics.extend(mismatch_diagnostics_with_source_root(
                    &path,
                    expected,
                    actual,
                    source_root,
                ));
            }
            (Some(expected), None) => diagnostics.push(support::html_oracle::Diagnostic {
                category: "missing-file".to_string(),
                logical_path: path,
                first_expected_line: Some(1),
                expected: String::from_utf8_lossy(expected).into_owned(),
                actual: String::new(),
                detail: "file exists only in expected output".to_string(),
            }),
            (None, Some(actual)) => diagnostics.push(support::html_oracle::Diagnostic {
                category: "unexpected-file".to_string(),
                logical_path: path,
                first_expected_line: None,
                expected: String::new(),
                actual: String::from_utf8_lossy(actual).into_owned(),
                detail: "file exists only in actual output".to_string(),
            }),
            (None, None) => unreachable!(),
        }
    }
    if let Some(actual_status) = actual_status {
        let expected_error = expected_status == CaseStatus::BuildError;
        let actual_error = actual_status == CaseStatus::BuildError;
        if expected_error != actual_error {
            diagnostics.push(support::html_oracle::Diagnostic {
                category: "status".to_string(),
                logical_path: String::new(),
                first_expected_line: None,
                expected: status_name(expected_status).to_string(),
                actual: status_name(actual_status).to_string(),
                detail: "build status class differs".to_string(),
            });
        }
    }
    diagnostics
}
