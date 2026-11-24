mod support {
    pub mod diagnostics;
    pub mod html_oracle;
}

use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use support::diagnostics::{run_bounded, run_bounded_with_timeout, ExitStatusKind, ProcessOutput};
use support::html_oracle::{
    apply_retention, aux_component_of, bounded_assertion_message, build_report, case_key,
    compare_file, compare_trees, compare_warnings, compare_warnings_with_roots, diagnose_html,
    diagnose_inventory, diagnose_needs_json, diagnose_searchindex, diagnose_warnings,
    group_first_divergences, load_fixture_suite, materialize_case_expected,
    materialize_case_inputs, parse_inventory, parse_keep, read_record, resolve_ultra_binary,
    status_name, walk_tree, warning_diagnostics_with_roots, write_logical_file, AuxComponent,
    CaseRecord, CaseResult, CaseStatus, Diagnostic, IndexDocument, InventoryRecord, Keep, Policy,
    ReportInputs,
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
    // The committed sidecar mirrors the ledger's `warnings` field for this
    // build-error case; the generator already reduced Sphinx's report and
    // normalized the source directory to <SRCDIR>.
    let expected = String::from_utf8_lossy(include_bytes!(
        "fixtures/html_oracle/core/warnings/html_projects/literalinclude.txt"
    ))
    .into_owned();
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
fn parse_inventory_accepts_every_sphinx_v2_record_shape() {
    // The regex rejects this line, which the docutils-0407 and sphinx_read-0172 references hold;
    // Sphinx's loader skips it.
    let empty_name = " std:label -1 index.html#id1$ -";
    let bytes = inventory_bytes(&format!(
        "my label std:label -1 index.html#my-label My label\n\
         index std:doc -1  Welcome\n\
         f py:function 1 api.html#$ -\n\
         {empty_name}\n",
    ));
    let (_, records) = parse_inventory(&bytes).expect("no record line makes an inventory invalid");
    let got: Vec<_> = records
        .iter()
        .map(|r| {
            (
                r.name.as_str(),
                r.domain_role.as_str(),
                r.priority,
                r.uri.as_str(),
                r.display_name.as_str(),
                r.raw.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("", "", 0, "", "", Some(empty_name)), // opaque raw record
            ("f", "py:function", 1, "api.html#$", "-", None),
            ("index", "std:doc", -1, "", "Welcome", None),
            (
                "my label",
                "std:label",
                -1,
                "index.html#my-label",
                "My label",
                None
            ),
        ]
    ); // parse_inventory sorts records
    let with = |line: &str| inventory_bytes(&format!("f py:function 1 api.html#$ -\n{line}\n"));
    assert!(compare_file("objects.inv", &with(empty_name), &with(empty_name), None).is_empty());
    let changed = compare_file(
        "objects.inv",
        &with(empty_name),
        &with(" std:label -1 index.html#id2$ -"),
        None,
    );
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].category, "objects-inventory-value");
}

#[test]
fn inventory_lines_that_do_not_rejoin_compare_byte_for_byte() {
    // The sphinx_read-0351 reference holds `nbsp`. Sphinx's regex reads the name `term` from it,
    // but its fields do not re-join to the line, so it stays a raw record and a writer that
    // drops the no-break space is caught.
    let nbsp = "term\u{a0} std:term -1 index.html#term-term -";
    let canonical = "term std:term -1 index.html#term-term -";
    let with = |line: &str| inventory_bytes(&format!("{line}\n"));
    let diagnostics = compare_file("objects.inv", &with(nbsp), &with(canonical), None);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].category, "objects-inventory-value");
    let (_, records) =
        parse_inventory(&with(nbsp)).expect("no record line makes an inventory invalid");
    assert_eq!(records[0].raw.as_deref(), Some(nbsp));
    // The exhaustive report localizes it: the raw line is missing, the parsed record extra.
    let tree = |line: &str| BTreeMap::from([(String::from("objects.inv"), with(line))]);
    let mut localized = compare_trees(&tree(nbsp), &tree(canonical), None, None, None)
        .into_iter()
        .map(|diagnostic| diagnostic.category)
        .collect::<Vec<_>>();
    localized.sort();
    assert_eq!(
        localized,
        ["inventory-extra-record", "inventory-missing-record"]
    );
    // The other field-boundary and priority spellings the regex alone absorbs.
    let index = "index std:doc -1 index.html T";
    let function = "f py:function 1 api.html#$ -";
    for (expected, actual) in [
        ("term\u{1f} std:term -1 index.html#term-term -", canonical), // sphinx_read-0352
        ("index std:doc -1 index.html \u{a0}T", index),               // docutils-0731
        ("f\tpy:function 1 api.html#$ -", function),
        ("f py:function 001 api.html#$ -", function),
        ("f py:function 1 api.html#$ - ", function),
    ] {
        let diagnostics = compare_file("objects.inv", &with(expected), &with(actual), None);
        assert_eq!(diagnostics.len(), 1, "{expected:?} vs {actual:?}");
        assert_eq!(diagnostics[0].category, "objects-inventory-value");
    }
}

/// The payload lines `parse_inventory` reads, and which of them stay raw,
/// for the shapes no reference holds (`SP/sphinx/util/inventory.py:113-128`):
/// `str.splitlines()` breaks at every Unicode line boundary and at a CRLF
/// pair once, a final break opens no line and a blank line inside is one
/// (raw `""`); the loader's `\s` and `rstrip()` take U+001F for whitespace,
/// so the fields it reads from a line holding one do not re-join to the
/// line; a role without a `:` and a priority past an `i32` stay raw.
#[test]
fn parse_inventory_splits_and_strips_lines_as_the_loader_does() {
    let records = |payload: &str| -> Vec<(String, Option<String>)> {
        let (_, records) = parse_inventory(&inventory_bytes(payload))
            .expect("no record line makes an inventory invalid");
        records
            .into_iter()
            .map(|record| (record.name, record.raw))
            .collect()
    };
    let parsed = |names: &str| -> Vec<(String, Option<String>)> {
        names.chars().map(|name| (name.to_string(), None)).collect()
    };
    let raw = |line: &str| (String::new(), Some(line.to_string()));

    let separators = [
        "\r", "\x0b", "\x0c", "\x1c", "\x1d", "\x1e", "\u{85}", "\u{2028}", "\u{2029}", "\r\n",
        "\n",
    ];
    let payload = ('a'..='k')
        .zip(separators)
        .map(|(name, separator)| format!("{name} py:function 1 {name}.html -{separator}"))
        .collect::<String>();
    assert_eq!(records(&payload), parsed("abcdefghijk"));

    let line = |name: &str| format!("{name} py:function 1 api.html -\n");
    assert_eq!(
        records(&format!("{}\n{}", line("a"), line("b"))),
        [vec![raw("")], parsed("ab")].concat()
    );

    for (shape, payload) in [
        ("U+001F the strip drops", "f py:function 1 api.html -\u{1f}"),
        ("U+001F between fields", "f\u{1f}py:function 1 api.html -"),
        (
            "U+001F ending the name",
            "term\u{1f} std:term -1 index.html#term-term -",
        ),
        ("a role without a colon", "f function 1 api.html -"),
        ("a priority past i32", "f py:function 2147483648 api.html -"),
        (
            "a priority below i32",
            "f py:function -2147483649 api.html -",
        ),
    ] {
        assert_eq!(records(payload), [raw(payload)], "{shape}");
    }
    assert_eq!(
        records("f py:function -2147483648 api.html -\ng py:function 2147483647 api.html -"),
        parsed("fg"),
        "the priorities an i32 holds"
    );
}

#[test]
fn comparator_reports_missing_and_unexpected_files() {
    let expected = BTreeMap::from([(String::from("index.html"), b"same".to_vec())]);
    let actual = BTreeMap::from([
        (String::from("index.html"), b"same".to_vec()),
        (String::from("extra.css"), b"extra".to_vec()),
    ]);
    let diagnostics = compare_trees(&expected, &actual, None, None, None);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].category, "unexpected-file");
    assert_eq!(diagnostics[0].logical_path, "extra.css");

    let actual = BTreeMap::new();
    let diagnostics = compare_trees(&expected, &actual, None, None, None);
    assert_eq!(diagnostics[0].category, "missing-file");
}

#[test]
fn comparator_localizes_tree_mismatches_and_status_class() {
    let expected = BTreeMap::from([(
        String::from("index.html"),
        b"<html><div class=\"body\" role=\"main\">\nA\n</div><footer>ok</footer></html>".to_vec(),
    )]);
    let actual = BTreeMap::from([(
        String::from("index.html"),
        b"<html><div class=\"body\" role=\"main\">\nB\n</div><footer>ok</footer></html>".to_vec(),
    )]);
    let diagnostics = compare_trees(
        &expected,
        &actual,
        Some(CaseStatus::Built),
        Some(CaseStatus::BuildError),
        None,
    );
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.category.as_str())
            .collect::<Vec<_>>(),
        ["status", "html-body"]
    );
    assert!(compare_trees(
        &expected,
        &expected,
        Some(CaseStatus::Built),
        Some(CaseStatus::Built),
        None
    )
    .is_empty());
}

#[test]
fn comparator_parses_real_searchindex_wrapper_with_and_without_semicolon() {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/html_oracle/core/refs/html_projects/basic/searchindex.js");
    let bytes = fs::read(&reference).expect("committed searchindex reference");
    let value = support::html_oracle::parse_json_value(&bytes, true)
        .expect("Sphinx's `Search.setIndex(...)` without a semicolon parses");
    assert!(value.get("docnames").is_some());
    let mut with_semicolon = bytes.clone();
    with_semicolon.extend_from_slice(b";\n");
    assert_eq!(
        support::html_oracle::parse_json_value(&with_semicolon, true).unwrap(),
        value
    );
    assert!(compare_file("searchindex.js", &bytes, &with_semicolon, None).is_empty());
    assert!(support::html_oracle::parse_json_value(b"Search.setIndex({}", true).is_err());
}

#[test]
fn comparator_maps_ultra_source_path_terms_onto_the_srcdir_token() {
    // The reference indexed <SRCDIR> for document 0; Ultra indexed the stems
    // of its own absolute input directory for the same document.
    let expected = json!({
        "terms": {"example": 0, "srcdir": 0, "titl": [0, 1], "work": 1},
        "titleterms": {"titl": [0, 1]},
    });
    let actual_text = json!({
        "terms": {
            "example": 0,
            "home": 0,
            "runner": 0,
            "work": [0, 1],
            "html_oracl": 0,
            "run": 0,
            "input": 0,
            "titl": [0, 1]
        },
        "titleterms": {"titl": [0, 1]},
    });
    let source_root = Path::new("/home/runner/work/html_oracle/runs/case/input");
    let actual = format!("Search.setIndex({actual_text})");
    let expected_bytes = format!("Search.setIndex({expected})");
    assert!(compare_file(
        "searchindex.js",
        expected_bytes.as_bytes(),
        actual.as_bytes(),
        Some(source_root),
    )
    .is_empty());
    // Without the source root nothing is rewritten.
    assert!(!compare_file(
        "searchindex.js",
        expected_bytes.as_bytes(),
        actual.as_bytes(),
        None,
    )
    .is_empty());
    // A document the reference did not mark keeps its terms ("work" in
    // document 1), and a real extra term still shows.
    let mut actual_value = actual_text.clone();
    actual_value["terms"]["bogus"] = json!(0);
    let actual = format!("Search.setIndex({actual_value})");
    let diagnostics = compare_file(
        "searchindex.js",
        expected_bytes.as_bytes(),
        actual.as_bytes(),
        Some(source_root),
    );
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].actual.contains("bogus"));
    assert!(diagnostics[0].actual.contains("\"work\": 1"));
    assert!(diagnostics[0].actual.contains("\"srcdir\": 0"));
    assert!(!diagnostics[0].actual.contains("runner"));
}

#[test]
fn comparator_zeroes_schema_report_timings_on_both_sides() {
    let expected = b"{\n  \"validation_summary\": \"Schema validation completed with 0 warning(s) in 0.000 seconds. Validated 0 needs/s.\",\n  \"validated_needs_per_second\": 0,\n  \"validated_needs_count\": 2,\n  \"validation_warnings\": {}\n}";
    let actual = b"{\r\n  \"validation_summary\": \"Schema validation completed with 0 warning(s) in 0.013 seconds. Validated 153.8 needs/s.\",\r\n  \"validated_needs_per_second\": 153.8,\r\n  \"validated_needs_count\": 2,\r\n  \"validation_warnings\": {}\r\n}";
    assert_eq!(
        support::html_oracle::policy_for_path("schema_violations.json"),
        Policy::NeedsSchemaReport
    );
    assert_eq!(
        support::html_oracle::policy_for_path("nested/schema_violations.json"),
        Policy::NeedsSchemaReport
    );
    assert!(compare_file("schema_violations.json", expected, actual, None).is_empty());
    let mut changed = actual.to_vec();
    changed.extend_from_slice(b"\n");
    let diagnostics = compare_file("schema_violations.json", expected, &changed, None);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].category, "text-value");
    // Other JSON files keep their timings.
    assert!(!compare_file("report.json", expected, actual, None).is_empty());
    assert_eq!(
        support::html_oracle::zero_schema_timings(actual),
        support::html_oracle::zero_schema_timings(&support::html_oracle::zero_schema_timings(
            actual
        ))
    );
}

#[test]
fn comparator_folds_lone_carriage_returns_like_the_generator() {
    assert!(compare_file("index.html", b"a\rb\r\nc\n", b"a\nb\nc\n", None).is_empty());
    assert!(compare_warnings(b"w\r", b"w\n", None, None).is_empty());
}

#[test]
fn committed_ledgers_load_and_validate() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/html_oracle");
    let suite = load_fixture_suite(&fixture_root)
        .unwrap_or_else(|error| panic!("committed HTML oracle ledgers must load: {error}"));
    for (profile, document) in &suite.profiles {
        let record = &document.profiles[profile];
        assert_eq!(record.platform, "linux");
        assert_eq!(record.sphinx, "9.1.0");
        assert!(record
            .determinism_shims
            .contains(&"uuid.uuid4=counter".to_string()));
        assert!(!document.cases.is_empty(), "{profile} has no cases");
        assert!(
            document.cases.iter().any(|case| case.status.is_runnable()),
            "{profile} has no runnable case"
        );
    }
    let core = &suite.profiles["core"].profiles["core"];
    assert_eq!(core.docutils, "0.22.4");
    assert_eq!(core.needs_version, None);
    let local_needs = &suite.profiles["local_needs"].profiles["local_needs"];
    assert_eq!(local_needs.docutils, "0.21.2");
    assert_eq!(local_needs.needs_version.as_deref(), Some("8.5.0"));
    assert_eq!(
        local_needs.needs_commit.as_deref(),
        Some("58bcb59d861da95f2aca79f343e8bae6ec5c1250")
    );
    assert_eq!(
        local_needs.needs_tree.as_deref(),
        Some("958172a89defcec69704f6b9d61e482e7c4e8409")
    );
    assert!(local_needs
        .determinism_shims
        .contains(&"needs_reproducible_json=1".to_string()));
    assert!(suite.profiles["local_needs"]
        .cases
        .iter()
        .any(|case| !case.origin.pytest_node_ids.is_empty()));
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
        raw: None,
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
    let (report, markdown) = build_report(&ReportInputs {
        total_cases: 2,
        results: &results,
        ultra_bin: Some("C:/ultra/sphinx-ultra.exe"),
        ..Default::default()
    });
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
    let (report, _) = build_report(&ReportInputs {
        total_cases: 3,
        results: &results,
        ..Default::default()
    });
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
    let (report, markdown) = build_report(&ReportInputs {
        total_cases: 5,
        excluded_cases: 1,
        reference_crash_cases: 1,
        results: &results,
        ..Default::default()
    });
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
    let (_report, markdown) = build_report(&ReportInputs {
        total_cases: 1,
        results: &results,
        reference_platforms: &platforms,
        ..Default::default()
    });
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
    let (report, markdown) = build_report(&ReportInputs {
        total_cases: 2,
        reference_crash_cases: 1,
        results: &[build_error],
        reference_cases: &[reference_crash],
        ..Default::default()
    });
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
    let (report, markdown) = build_report(&ReportInputs {
        total_cases: 30,
        results: &results,
        ..Default::default()
    });
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
    if let Some(filter) = &filter {
        // A filter that selects only excluded or reference-crash cases would
        // otherwise produce an empty, green run.
        assert!(
            !jobs.is_empty(),
            "HTML_ORACLE_FILTER matched no runnable ledger case: {filter}"
        );
    }
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

    let (report, markdown) = build_report(&ReportInputs {
        total_cases,
        excluded_cases,
        reference_crash_cases,
        results: &results,
        reference_platforms: &reference_platforms,
        reference_cases: &reference_cases,
        ultra_bin: Some(&ultra_bin),
    });
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

/// The Ultra executable, as an absolute path: cases run with the input
/// directory as the working directory, so a relative override must be
/// resolved against the test's own working directory first.
fn selected_ultra_binary() -> String {
    let selected = resolve_ultra_binary(
        std::env::var("HTML_ORACLE_ULTRA_BIN").ok().as_deref(),
        env!("CARGO_BIN_EXE_sphinx-ultra"),
    );
    let path = Path::new(&selected);
    if path.is_absolute() || path.components().count() == 1 {
        return selected;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
        .unwrap_or(selected)
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

    // Like the reference build, Ultra runs from the source directory so
    // cwd-relative paths in its output are source-relative.
    let mut command = Command::new(selected_ultra_binary());
    command
        .current_dir(&input_dir)
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
        compare_trees(
            &expected_tree,
            &walk_tree(&actual_dir)?,
            Some(case.status),
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
        .current_dir(input_dir)
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
            compare_trees(
                &expected_tree,
                &actual_tree,
                Some(expected_status),
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

/// The output files `html_oracle_aux_gate` compares besides the warning
/// stream; the pages and the rest of the output tree are the HTML writer's
/// and builder's (5b, 5c).
const AUX_OUTPUT_FILES: [&str; 2] = ["objects.inv", "searchindex.js"];

/// Runs one case for the aux gate under `run_root` and returns every
/// diagnostic of its warning stream, exit class, `objects.inv` and
/// `searchindex.js`.
///
/// Leaner than `run_html_case`: only the case's inputs are materialized, the
/// expected files are read straight from the ledger's storage, nothing else
/// of either output tree is read, no `result.json` is written, and the case
/// directory is deleted whatever the outcome. It always runs the binary
/// Cargo built for this test, never `HTML_ORACLE_ULTRA_BIN`.
fn run_aux_case(
    case: &CaseRecord,
    profile_root: &Path,
    run_root: &Path,
) -> Result<Vec<Diagnostic>, String> {
    let run_dir = run_root
        .join(&case.profile)
        .join(&case.source_set)
        .join(&case.case_id);
    if run_dir.exists() {
        fs::remove_dir_all(&run_dir)
            .map_err(|error| format!("remove stale run {}: {error}", run_dir.display()))?;
    }
    let outcome = compare_aux_outputs(case, profile_root, &run_dir);
    let removed = fs::remove_dir_all(&run_dir)
        .map_err(|error| format!("remove {}: {error}", run_dir.display()));
    let diagnostics = outcome?;
    removed?;
    Ok(diagnostics)
}

fn compare_aux_outputs(
    case: &CaseRecord,
    profile_root: &Path,
    run_dir: &Path,
) -> Result<Vec<Diagnostic>, String> {
    let input_dir = run_dir.join("input");
    let output_dir = run_dir.join("output");
    let cache_dir = run_dir.join("cache");
    let warnings_path = run_dir.join("warnings.txt");
    fs::create_dir_all(&input_dir)
        .map_err(|error| format!("create {}: {error}", input_dir.display()))?;
    materialize_case_inputs(case, profile_root, &input_dir)?;

    // Invoked exactly as `run_html_case` invokes it, from the source
    // directory, so the two harnesses normalize the same output.
    let mut command = Command::new(env!("CARGO_BIN_EXE_sphinx-ultra"));
    command
        .current_dir(&input_dir)
        .arg(&input_dir)
        .arg(&output_dir)
        .args(["-b", "html", "-d"])
        .arg(&cache_dir)
        .arg("-q")
        .arg("-w")
        .arg(&warnings_path);
    let process = run_bounded(command);
    let mut diagnostics = match process_status(&process) {
        Some(actual_status) => {
            let expected = case
                .files
                .iter()
                .filter(|record| AUX_OUTPUT_FILES.contains(&record.logical_path.as_str()))
                .map(|record| {
                    Ok((
                        record.logical_path.clone(),
                        read_record(record, profile_root)?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            let mut actual = BTreeMap::new();
            for logical_path in AUX_OUTPUT_FILES {
                if let Some(bytes) = read_if_present(&output_dir.join(logical_path))? {
                    actual.insert(logical_path.to_string(), bytes);
                }
            }
            // A file on one side only is that component's missing-file or
            // unexpected-file; the status class follows `compare_trees`' rule.
            compare_trees(
                &expected,
                &actual,
                Some(case.status),
                Some(actual_status),
                Some(&input_dir),
            )
        }
        None => vec![process_diagnostic(&process)],
    };
    // An absent warning file is an empty stream, as in `run_html_case`.
    let actual_warnings = read_if_present(&warnings_path)?.unwrap_or_default();
    let roots = Some(support::html_oracle::WarningRoots {
        source_root: Some(&input_dir),
        output_root: Some(&output_dir),
        doctree_root: Some(&cache_dir),
        case_root: Some(run_dir),
    });
    let expected_warnings = case.warnings.as_bytes();
    let mut warning_diagnostics =
        warning_diagnostics_with_roots(expected_warnings, &actual_warnings, None, roots);
    if warning_diagnostics.is_empty() {
        // The line diagnostics cannot see a stream that differs only in its
        // final line break; the whole-stream comparison can.
        warning_diagnostics =
            compare_warnings_with_roots(expected_warnings, &actual_warnings, None, roots);
    }
    diagnostics.extend(warning_diagnostics);
    Ok(diagnostics)
}

fn read_if_present(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

/// Runs every runnable core case through `run_aux_case` on scoped worker
/// threads and returns each case with its aux-component diagnostics, in
/// ledger order. `run_dir_name` names the run root under
/// `target/html-oracle`; the gate and the census each pass their own, so a
/// test binary that runs both never has one delete the other's case
/// directories.
fn run_aux_cases(run_dir_name: &str) -> Vec<(CaseRecord, Vec<Diagnostic>)> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let profile_root = manifest_dir.join("tests/fixtures/html_oracle/core");
    let ledger = IndexDocument::load(&profile_root.join("index.json"))
        .unwrap_or_else(|error| panic!("cannot load the core HTML oracle ledger: {error}"));
    let cases = ledger
        .cases
        .into_iter()
        .filter(|case| case.status.is_runnable())
        .collect::<Vec<_>>();
    let run_root = manifest_dir.join("target/html-oracle").join(run_dir_name);
    fs::create_dir_all(&run_root)
        .unwrap_or_else(|error| panic!("create {}: {error}", run_root.display()));
    // The binary prints its canonicalized source directory, so the roots the
    // warnings and `searchindex.js` are normalized with must be canonical too.
    let run_root = run_root
        .canonicalize()
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", run_root.display()));
    let workers = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(cases.len().max(1));
    let next_case = AtomicUsize::new(0);
    let mut finished = std::thread::scope(|scope| {
        let handles = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut finished = Vec::new();
                    while let Some(case) = cases.get(next_case.fetch_add(1, Ordering::Relaxed)) {
                        let diagnostics = run_aux_case(case, &profile_root, &run_root)
                            .unwrap_or_else(|error| panic!("{}: {error}", case_key(case)));
                        finished.push((
                            case_key(case),
                            diagnostics
                                .into_iter()
                                .filter(|diagnostic| aux_component_of(diagnostic).is_some())
                                .collect::<Vec<_>>(),
                        ));
                    }
                    finished
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("aux worker should not panic"))
            .collect::<BTreeMap<_, _>>()
    });
    cases
        .into_iter()
        .map(|case| {
            let diagnostics = finished
                .remove(&case_key(&case))
                .expect("every case was run");
            (case, diagnostics)
        })
        .collect()
}

/// Holds the warning stream, exit class, `objects.inv` and `searchindex.js`
/// of every runnable core case to the oracle, strictly: the set of
/// mismatching `(case_key, component)` pairs must equal `KNOWN_AUX_GAPS`.
/// A task that fixes a row deletes it; a new mismatch fails the gate.
#[test]
#[cfg_attr(
    windows,
    ignore = "oracle references are POSIX-spelled; Windows paths use backslashes"
)]
fn html_oracle_aux_gate() {
    let mut observed = BTreeMap::new();
    for (case, diagnostics) in run_aux_cases("aux-gate-runs") {
        for diagnostic in diagnostics {
            let component = aux_component_of(&diagnostic).expect("only aux diagnostics are kept");
            observed
                .entry((case_key(&case), component))
                .or_insert(diagnostic);
        }
    }
    let diff = aux_gate_diff(&observed.keys().cloned().collect(), KNOWN_AUX_GAPS);
    assert!(
        diff.is_clean(),
        "{}",
        aux_gate_failure(&diff, &observed, KNOWN_AUX_GAPS)
    );
}

/// The gate's failure message: three bounded lists, each regression with its
/// first diagnostic, each fixed or duplicate pair with its rows.
fn aux_gate_failure(
    diff: &AuxGateDiff,
    first_diagnostics: &BTreeMap<(String, AuxComponent), Diagnostic>,
    table: &[(&str, AuxComponent, &str)],
) -> String {
    let table_rows = |(case, component): &(String, AuxComponent)| {
        table
            .iter()
            .filter(|(row_case, row_component, _)| row_case == case && row_component == component)
            .map(|(case, component, reason)| {
                format!("    ({case:?}, AuxComponent::{component:?}, {reason:?}),")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut message = format!(
        "the aux mismatch set differs from KNOWN_AUX_GAPS: {} regressions, {} fixed rows, \
         {} duplicate rows",
        diff.regressions.len(),
        diff.fixed.len(),
        diff.duplicate_rows.len()
    );
    push_bounded_list(
        &mut message,
        "duplicate rows (list each case and component once)",
        diff.duplicate_rows.iter().map(table_rows),
    );
    push_bounded_list(
        &mut message,
        "regressions (not in KNOWN_AUX_GAPS)",
        diff.regressions.iter().map(|key| {
            let (case, component) = key;
            let diagnostic = &first_diagnostics[key];
            let (line, expected, actual) =
                first_differing_line(&diagnostic.expected, &diagnostic.actual);
            format!(
                "    {case} {component:?}: {} ({})\n        expected line {line}: {}\n        \
                 actual line {line}:   {}",
                diagnostic.category,
                diagnostic.detail,
                cap_line(expected),
                cap_line(actual)
            )
        }),
    );
    push_bounded_list(
        &mut message,
        "fixed (delete these rows)",
        diff.fixed.iter().map(table_rows),
    );
    message
}

fn push_bounded_list(
    message: &mut String,
    title: &str,
    items: impl ExactSizeIterator<Item = String>,
) {
    const MAX_ITEMS: usize = 60;
    let count = items.len();
    if count == 0 {
        return;
    }
    message.push_str(&format!("\n{title}: {count}"));
    for item in items.take(MAX_ITEMS) {
        message.push('\n');
        message.push_str(&item);
    }
    if count > MAX_ITEMS {
        message.push_str(&format!("\n    ... and {} more", count - MAX_ITEMS));
    }
}

/// The first line, numbered from 1, where two texts differ, and that line of
/// each (empty past a text's end).
fn first_differing_line<'a>(expected: &'a str, actual: &'a str) -> (usize, &'a str, &'a str) {
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut number = 1;
    loop {
        match (expected_lines.next(), actual_lines.next()) {
            (Some(left), Some(right)) if left == right => number += 1,
            (None, None) => return (number, "", ""),
            (left, right) => return (number, left.unwrap_or_default(), right.unwrap_or_default()),
        }
    }
}

fn cap_line(line: &str) -> String {
    const MAX_CHARS: usize = 240;
    if line.chars().count() <= MAX_CHARS {
        format!("{line:?}")
    } else {
        format!("{:?}...", line.chars().take(MAX_CHARS).collect::<String>())
    }
}

/// Prints the current aux mismatch set as `KNOWN_AUX_GAPS` rows, in table
/// order. A row's reason lists the fix families its diagnostics show and the
/// latest task that fixes one of them. A mismatch no family explains gets a
/// `?` family and a comment above its row, and fails the census once every
/// row is printed:
///
/// `cargo test --locked --test html_differential html_oracle_aux_census -- --ignored --nocapture`
#[test]
#[ignore]
fn html_oracle_aux_census() {
    let mut counts = BTreeMap::<AuxComponent, usize>::new();
    let mut unexplained_rows = 0;
    let mut table = String::new();
    for (case, diagnostics) in run_aux_cases("aux-census-runs") {
        let mut by_component = BTreeMap::<AuxComponent, Vec<Diagnostic>>::new();
        for diagnostic in diagnostics {
            let component = aux_component_of(&diagnostic).expect("only aux diagnostics are kept");
            by_component.entry(component).or_default().push(diagnostic);
        }
        if by_component.is_empty() {
            continue;
        }
        for (component, diagnostics) in by_component {
            let census = match component {
                AuxComponent::Warnings => warning_census(&case, &diagnostics),
                AuxComponent::ExitClass => {
                    GapCensus::unattributed(diagnostics.iter().map(|diagnostic| {
                        format!("{}: {}", diagnostic.category, diagnostic.detail)
                    }))
                }
                AuxComponent::ObjectsInv => inventory_census(&diagnostics),
                AuxComponent::SearchIndex => searchindex_census(&diagnostics),
            };
            *counts.entry(component).or_default() += 1;
            if !census.unexplained.is_empty() {
                unexplained_rows += 1;
            }
            for note in &census.unexplained {
                for line in note.lines() {
                    table.push_str(&format!("    // ? {line}\n"));
                }
            }
            table.push_str(&format!(
                "    ({:?}, AuxComponent::{component:?}, {:?}),\n",
                case_key(&case),
                census.reason()
            ));
        }
    }
    println!("    // Rows per component: {counts:?}\n{table}");
    assert_eq!(
        unexplained_rows, 0,
        "rows with a `?` family need a family the census does not know"
    );
}

/// The owners of gap rows in the order their tasks land: a row is owned by
/// the latest task that fixes one of its families.
const GAP_OWNERS: &[&str] = &[
    "5a-T3",
    "5a-T4",
    "5a-T5a",
    "5a-T5b",
    "5a-T5c",
    "5a-T6",
    "5a-T7",
    "5a-T8",
    "5a-T9a",
    "5a-T9b",
    "5a-T9c",
    "5a-T10",
    "5a-T11",
    "5a-T12",
    "5a-T13a",
    "5a-T13b",
    "5a-T14",
    "5a-T15",
    "5a-T16a",
    "5a-T16b",
    "5a-T17a",
    "5a-T17b",
    "5a-T17c",
    "5a-T18a",
    "5a-T18b",
    "5a-T18c",
    "5b (oracle regen)",
    "5b",
    "5c",
];

/// One fix family of a gap row and the task that fixes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct GapFix {
    family: &'static str,
    owner: &'static str,
}

const fn gap_fix(family: &'static str, owner: &'static str) -> GapFix {
    GapFix { family, owner }
}

/// What the census learned about one `(case, component)` mismatch.
#[derive(Debug, Default)]
struct GapCensus {
    fixes: BTreeSet<GapFix>,
    /// Differences no family explains, printed above the row.
    unexplained: Vec<String>,
}

impl GapCensus {
    /// A census that attributes these differences to no family.
    fn unattributed(notes: impl IntoIterator<Item = String>) -> Self {
        Self {
            fixes: BTreeSet::new(),
            unexplained: notes.into_iter().collect(),
        }
    }

    /// `"<families>: <owner>"`, the families in `F1`…`F12`, `INV:`, `SI:`
    /// order, a `?` family for anything unexplained.
    fn reason(&self) -> String {
        let mut families = self.fixes.iter().map(|fix| fix.family).collect::<Vec<_>>();
        if !self.unexplained.is_empty() {
            families.push("?");
        }
        families.sort_by_key(|family| family_sort_key(family));
        families.dedup();
        let owner = self
            .fixes
            .iter()
            .map(|fix| fix.owner)
            .max_by_key(|owner| {
                GAP_OWNERS
                    .iter()
                    .position(|known| known == owner)
                    .unwrap_or_else(|| panic!("unknown gap owner {owner:?}"))
            })
            .unwrap_or("?");
        format!("{}: {owner}", families.join("+"))
    }
}

/// Sorts `F2` before `F10` and `F8` before `F8b`, and the `F` families
/// before `INV:` and `SI:` causes.
fn family_sort_key(family: &str) -> (bool, u32, &str) {
    let numbered = family
        .strip_prefix('F')
        .map(|rest| {
            rest.split_at(
                rest.find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(rest.len()),
            )
        })
        .and_then(|(number, suffix)| Some((number.parse().ok()?, suffix)));
    match numbered {
        Some((number, suffix)) => (false, number, suffix),
        None => (true, 0, family),
    }
}

/// `[docutils]` records that only a transform not ported yet, or the HTML
/// writer, creates, by message, with that family. Every other `[docutils]`
/// record is one the parser creates, which the reporter channel prints since
/// T8 (F1) [TW §6.3, §7.4], or one a ported transform creates (`Transitions`'
/// since T9c, F7; the hyperlink-target transforms' since T10, F4's
/// `Unknown target name`, `Anonymous hyperlink mismatch` and `Indirect
/// hyperlink target`; Substitutions' and Footnotes' since T11, F4's
/// `Undefined substitution referenced` and `Too many … footnote
/// references`): a record one side alone prints is unexplained.
const REPORTER_MESSAGE_FIXES: &[(&str, GapFix)] = &[("Cannot scale image!", gap_fix("F8", "5b"))];

/// Logger records Ultra does not print, by message [TW §6.3, §7.3]. The
/// unreferenced-footnote warning (`[ref.footnote]`, F6) is printed since
/// T11, the image collector's `image file not readable` (F3) since T13a,
/// and the citation and math domains' `Citation [X] is not referenced.`
/// and `citation not found` (F6) and `equation not found` (F10) since
/// T13b.
const MISSING_LOGGER_FIXES: &[(&str, GapFix)] = &[
    ("Configuration error!", gap_fix("F9", "5a-T3")),
    ("Could not obtain image size. ", gap_fix("F8", "5b")),
];

/// The messages of the M1 directive and role validators
/// (`src/directives/validation/`), which Sphinx never prints (F2).
const M1_VALIDATION_MESSAGES: &[&str] = &[
    r"Unknown option '.*'( for \S+ directive)?",
    r"\S+ option should not have a value",
    r"\S+ is not a valid length: '.*'",
    r"Invalid alignment: .*",
    r"Unusual image extension: .*",
    r"Scale must be a number",
    r"(Image|Include file|Literalinclude file) path cannot be empty",
    r"Unmatched braces in math (content|expression)",
    r"\S+ directive (requires .*|has no content|is empty)",
    r"\S+( \S+)? role (requires .*|should .*)",
    r"Abbreviations typically contain uppercase letters",
    r"Command contains potentially dangerous characters",
    r"Document reference should not include file extension",
    r"Download target has no file extension",
    r"File path contains invalid characters",
    r"Unusual file type for download: .*",
    r"Use &amp; for literal ampersand in GUI labels",
];

/// The families of a `Warnings` mismatch, record by record: each record only
/// one side prints is matched against the message families of [TW §6.3,
/// §7.3]. A node-located record printed with its included file's relative
/// path (`part.rst:6` for `<SRCDIR>/part.rst:6`, F8b) is fixed since T14, so
/// one now is unexplained.
fn warning_census(case: &CaseRecord, diagnostics: &[Diagnostic]) -> GapCensus {
    use regex::Regex;
    use std::sync::LazyLock;
    static M1: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!("^({})$", M1_VALIDATION_MESSAGES.join("|"))).expect("valid pattern")
    });

    let (expected_only, actual_only) = match warning_record_diff(case, diagnostics) {
        Ok(records) => records,
        Err(note) => return GapCensus::unattributed([note]),
    };
    let mut census = GapCensus::default();
    for record in &expected_only {
        let message = warning_record_message(record).unwrap_or(record);
        let fixes = if let Some((_, fix)) = REPORTER_MESSAGE_FIXES
            .iter()
            .find(|(pattern, _)| record.ends_with(" [docutils]") && record.contains(pattern))
        {
            vec![*fix]
        } else if let Some((_, fix)) = MISSING_LOGGER_FIXES
            .iter()
            .find(|(pattern, _)| record.contains(pattern))
        {
            vec![*fix]
        } else {
            census.unexplained.push(format!("expected only: {message}"));
            continue;
        };
        census.fixes.extend(fixes);
    }
    for record in &actual_only {
        let message = warning_record_message(record).unwrap_or(record);
        if M1.is_match(message) {
            census.fixes.insert(gap_fix("F2", "5a-T3"));
        } else {
            census.unexplained.push(format!("actual only: {record}"));
        }
    }
    census
}

/// The warning records only one side prints, `(expected_only, actual_only)`.
///
/// The actual stream is rebuilt from the case's expected stream and its
/// positional `warning-*-line` diagnostics; it splits into records at every
/// line that opens one (`[<location>: ]WARNING|ERROR|CRITICAL: …`), and the
/// lines before the first such line (a fatal report) form one record.
fn warning_record_diff(
    case: &CaseRecord,
    diagnostics: &[Diagnostic],
) -> Result<(Vec<String>, Vec<String>), String> {
    let normalized = case.warnings.replace("\r\n", "\n").replace('\r', "\n");
    let expected = normalized.lines().collect::<Vec<_>>();
    let mut at_line = BTreeMap::new();
    for diagnostic in diagnostics {
        let line = diagnostic
            .first_expected_line
            .filter(|_| diagnostic.category.starts_with("warning-"))
            .ok_or_else(|| format!("{} is not a line diagnostic", diagnostic.category))?;
        if diagnostic.category != "warning-extra-line"
            && expected.get(line - 1) != Some(&diagnostic.expected.as_str())
        {
            return Err(format!("expected line {line} is not the ledger's"));
        }
        at_line.insert(line, diagnostic);
    }
    let lines_of = |category: &str| {
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == category)
            .count()
    };
    let actual_len =
        expected.len() + lines_of("warning-extra-line") - lines_of("warning-missing-line");
    let actual = (1..=actual_len)
        .map(|line| match at_line.get(&line) {
            Some(diagnostic) => Ok(diagnostic.actual.as_str()),
            None => expected
                .get(line - 1)
                .copied()
                .ok_or_else(|| format!("actual line {line} cannot be rebuilt")),
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut balance = BTreeMap::<String, isize>::new();
    for record in warning_records(&expected) {
        *balance.entry(record).or_default() += 1;
    }
    for record in warning_records(&actual) {
        *balance.entry(record).or_default() -= 1;
    }
    let mut expected_only = Vec::new();
    let mut actual_only = Vec::new();
    for (record, count) in balance {
        let side = if count > 0 {
            &mut expected_only
        } else {
            &mut actual_only
        };
        side.extend(std::iter::repeat_n(record, count.unsigned_abs()));
    }
    if expected_only.is_empty() && actual_only.is_empty() {
        return Err("both streams hold the same records in another order".to_string());
    }
    Ok((expected_only, actual_only))
}

/// Splits a warning stream into records, each a line that opens one and its
/// continuation lines; lines before the first record (a fatal report) form
/// one record.
fn warning_records(lines: &[&str]) -> Vec<String> {
    let mut records = Vec::<Vec<&str>>::new();
    for &line in lines {
        match records.last_mut() {
            Some(record) if warning_record_message(line).is_none() => record.push(line),
            _ => records.push(vec![line]),
        }
    }
    records
        .into_iter()
        .map(|record| record.join("\n"))
        .collect()
}

/// The message of a line that opens a warning record,
/// `[<location>: ]WARNING|ERROR|CRITICAL: <message>`.
fn warning_record_message(line: &str) -> Option<&str> {
    use regex::Regex;
    use std::sync::LazyLock;
    static RECORD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:\S+?(?::\d*)?: )?(?:WARNING|ERROR|CRITICAL): (.*)").expect("valid pattern")
    });
    RECORD
        .captures(line)
        .and_then(|captures| captures.get(1))
        .map(|message| message.as_str())
}

/// The causes of an `objects.inv` mismatch [BASE §2.8], told by each record
/// only one side holds or holds differently. The two the census knew, a
/// title's dangling reference kept as its raw source (`INV:word_`) and a
/// label PropagateTargets or IndirectHyperlinks changes
/// (`INV:target-transforms`), are fixed since T10: every mismatch is now
/// unexplained.
fn inventory_census(diagnostics: &[Diagnostic]) -> GapCensus {
    let mut census = GapCensus::default();
    census
        .unexplained
        .extend(diagnostics.iter().map(|diagnostic| {
            format!(
                "{}: expected {:?}, actual {:?}",
                diagnostic.category, diagnostic.expected, diagnostic.actual
            )
        }));
    census
}

/// The causes of a `searchindex.js` mismatch [TW A18, BASE §2.9]. The ones
/// the census knew are fixed: a section anchor the label PropagateTargets
/// moves onto it (`SI:propagate-targets`) and a reference the 440, 460 or
/// 850 transforms keep as its raw source (`SI:word_`) since T10, a
/// substitution reference indexed by its name (`SI:substitutions`) and a
/// footnote reference kept as its raw source (`SI:footnote-refs`) since
/// T11: every mismatch is now unexplained.
fn searchindex_census(diagnostics: &[Diagnostic]) -> GapCensus {
    GapCensus::unattributed(diagnostics.iter().map(|diagnostic| {
        format!(
            "{}: {}: expected {:?}, actual {:?}",
            diagnostic.category, diagnostic.detail, diagnostic.expected, diagnostic.actual
        )
    }))
}

/// The verdict of `html_oracle_aux_gate`: the observed `(case_key, component)`
/// mismatches against the rows of `KNOWN_AUX_GAPS`.
#[derive(Debug, Default, PartialEq, Eq)]
struct AuxGateDiff {
    /// Observed mismatches that no row lists.
    regressions: Vec<(String, AuxComponent)>,
    /// Rows whose mismatch is gone; the task that fixed them deletes them.
    fixed: Vec<(String, AuxComponent)>,
    /// `(case_key, component)` pairs that more than one row lists.
    duplicate_rows: Vec<(String, AuxComponent)>,
}

impl AuxGateDiff {
    fn is_clean(&self) -> bool {
        self.regressions.is_empty() && self.fixed.is_empty() && self.duplicate_rows.is_empty()
    }
}

/// Compares the observed mismatch set with a gap table, each list sorted.
fn aux_gate_diff(
    observed: &BTreeSet<(String, AuxComponent)>,
    table: &[(&str, AuxComponent, &str)],
) -> AuxGateDiff {
    let mut listed = BTreeSet::new();
    let mut duplicate_rows = BTreeSet::new();
    for (case, component, _reason) in table {
        let key = (case.to_string(), *component);
        if !listed.insert(key.clone()) {
            duplicate_rows.insert(key);
        }
    }
    AuxGateDiff {
        regressions: observed.difference(&listed).cloned().collect(),
        fixed: listed.difference(observed).cloned().collect(),
        duplicate_rows: duplicate_rows.into_iter().collect(),
    }
}

fn aux_diagnostic(category: &str, logical_path: &str) -> Diagnostic {
    Diagnostic {
        category: category.to_string(),
        logical_path: logical_path.to_string(),
        first_expected_line: None,
        expected: String::new(),
        actual: String::new(),
        detail: "synthetic".to_string(),
    }
}

#[test]
fn aux_component_of_maps_harness_categories() {
    // A case that hangs, fails to spawn or loses its pipes has no exit class
    // to compare, so it must never pass silently.
    for category in ["timeout", "status", "spawn", "io", "process"] {
        assert_eq!(
            aux_component_of(&aux_diagnostic(category, "")),
            Some(AuxComponent::ExitClass),
            "{category}"
        );
    }
    for (category, logical_path, component) in [
        (
            "warning-missing-line",
            "warnings",
            Some(AuxComponent::Warnings),
        ),
        (
            "warning-changed-line",
            "warnings",
            Some(AuxComponent::Warnings),
        ),
        (
            "objects-inventory-value",
            "objects.inv",
            Some(AuxComponent::ObjectsInv),
        ),
        (
            "inventory-missing-record",
            "objects.inv",
            Some(AuxComponent::ObjectsInv),
        ),
        (
            "missing-file",
            "objects.inv",
            Some(AuxComponent::ObjectsInv),
        ),
        (
            "searchindex-changed-key",
            "searchindex.js",
            Some(AuxComponent::SearchIndex),
        ),
        (
            "unexpected-file",
            "searchindex.js",
            Some(AuxComponent::SearchIndex),
        ),
        ("html-body", "index.html", None),
        ("missing-file", "genindex.html", None),
        ("missing-file", "search.html", None),
        ("text-value", "_static/doctools.js", None),
        ("missing-file", "_sources/index.rst.txt", None),
    ] {
        assert_eq!(
            aux_component_of(&aux_diagnostic(category, logical_path)),
            component,
            "{category} {logical_path}"
        );
    }
}

#[test]
fn aux_gate_diff_reports_regressions_and_fixed_rows() {
    use AuxComponent::{ObjectsInv, SearchIndex, Warnings};
    let key = |case: &str, component| (case.to_string(), component);
    let table: &[(&str, AuxComponent, &str)] = &[
        ("core/s/still-failing", Warnings, "F1: 5a-T8"),
        ("core/s/fixed", SearchIndex, "SI:word_: 5a-T10"),
        ("core/s/still-failing", SearchIndex, "SI:word_: 5a-T10"),
    ];
    let observed = BTreeSet::from([
        key("core/s/still-failing", Warnings),
        key("core/s/still-failing", SearchIndex),
        key("core/s/new", ObjectsInv),
    ]);
    let diff = aux_gate_diff(&observed, table);
    assert_eq!(diff.regressions, [key("core/s/new", ObjectsInv)]);
    assert_eq!(diff.fixed, [key("core/s/fixed", SearchIndex)]);
    assert!(diff.duplicate_rows.is_empty());
    assert!(!diff.is_clean());

    // A table listing exactly the observed set, each pair once, is clean.
    let exact: &[(&str, AuxComponent, &str)] = &[
        ("core/s/new", ObjectsInv, "INV:x: 5a-T10"),
        ("core/s/still-failing", Warnings, "F1: 5a-T8"),
        ("core/s/still-failing", SearchIndex, "SI:word_: 5a-T10"),
    ];
    assert!(aux_gate_diff(&observed, exact).is_clean());

    // A case and component listed twice is an error, whatever the reasons say,
    // even when the set comparison alone would pass.
    let duplicated: &[(&str, AuxComponent, &str)] = &[
        ("core/s/new", ObjectsInv, "INV:x: 5a-T10"),
        ("core/s/still-failing", Warnings, "F1: 5a-T8"),
        ("core/s/still-failing", SearchIndex, "SI:word_: 5a-T10"),
        ("core/s/still-failing", Warnings, "F1+F3: 5a-T13a"),
    ];
    let diff = aux_gate_diff(&observed, duplicated);
    assert!(diff.regressions.is_empty());
    assert!(diff.fixed.is_empty());
    assert_eq!(diff.duplicate_rows, [key("core/s/still-failing", Warnings)]);
    assert!(!diff.is_clean());
}

/// Every `(case_key, component)` pair whose output differs from the oracle
/// today, checked strictly by `html_oracle_aux_gate`; `html_oracle_aux_census`
/// prints the current set as rows. A row's reason is `"<families>: <owner>"`:
/// every fix family its mismatch holds (`F1`…`F12` for the warning stream,
/// `SI:<cause>` for `searchindex.js`, `INV:<cause>` for `objects.inv`) and
/// the latest task that fixes one of them. Rows stay one per line, as the
/// census prints them, so a task deletes or re-attributes a row by editing
/// one line and a grep for an owner shows whole rows.
#[rustfmt::skip]
const KNOWN_AUX_GAPS: &[(&str, AuxComponent, &str)] = &[
    ("core/docutils_snippets/docutils-0447", AuxComponent::Warnings, "F8: 5b"),
    ("core/docutils_snippets/docutils-0449", AuxComponent::Warnings, "F8: 5b"),
    ("core/docutils_snippets/docutils-0450", AuxComponent::Warnings, "F8: 5b"),
];
