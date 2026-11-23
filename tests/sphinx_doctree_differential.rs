//! Differential tests: our read vs the SPHINX ORACLE — the pseudo-XML a real
//! `sphinx-build` 9.1.0 read phase (dummy builder, `extensions = []`)
//! produces, for two committed corpora:
//!
//! - `sphinx_doctree_differential.json`, one-snippet reads under the
//!   fixture's own settings (smartquotes off, keep_warnings on);
//! - `sphinx_docutils_doctree_differential.json`, the post-read doctrees of
//!   the HTML oracle's docutils snippets, each read under the oracle's own
//!   `conf.py` (keep_warnings off) — compared strictly against
//!   [`KNOWN_POST_READ_GAPS`].
//!
//! Regenerate the fixtures (manual, never in CI), with the locked core
//! oracle profile:
//!     PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
//!         --project tools/oracle_profiles/core python tools/gen_sphinx_fixture.py
//! and the same with `--corpus html-oracle-docutils` for the second.
//!
//! Both read through [`read_standalone`] with the [`ReadSettings`] the
//! fixture's recorded settings give through a [`BuildConfig`] (M1's
//! directive and role validation off, wave 5a decision 7), never Sphinx's
//! bare defaults, and compare its post-read doctree as it is.
//!
//! Clones the tests/doctree_differential.rs shape: committed JSON, version
//! assertions (BOTH sphinx and docutils are recorded), floor guard against
//! silent truncation, collect ALL mismatches before asserting, and panics
//! surface as named mismatches, not test aborts. The fixture's source paths
//! are normalized to the "<snippet>" token; ParseOptions.source_path below
//! must use the same token.
//!
//! Per-case config (wave-4.5 task 8): a case may carry a `conf` dict — the
//! confoverrides the generator applied for that case. Every key maps onto
//! `ParseOptions.py` ([`sphinx_ultra::py::PySigConfig`]); an unmapped key is
//! a hard error so a future generator-side conf addition fails HERE instead
//! of silently parsing under defaults (serde ignores unknown struct fields,
//! so without the explicit map a conf case would quietly lose its config).

mod support {
    pub mod read_gaps;
}

use std::collections::{BTreeMap, BTreeSet};

use sphinx_ultra::config::BuildConfig;
use sphinx_ultra::py::PySigConfig;
use sphinx_ultra::read::{read_standalone, ReadSettings};
use sphinx_ultra::rst::ParseOptions;

#[derive(serde::Deserialize)]
struct Fixture {
    docutils_version: String,
    sphinx_version: String,
    settings: FixtureSettings,
    cases: Vec<Case>,
}

/// The read settings the generator recorded in the fixture header (and
/// asserted against the oracle app): the ones the read consumes.
#[derive(serde::Deserialize)]
struct FixtureSettings {
    keep_warnings: bool,
    smartquotes: bool,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
    pseudo_xml: String,
    #[serde(default)]
    conf: BTreeMap<String, serde_json::Value>,
}

/// Map a fixture case's `conf` dict onto the [`PySigConfig`] the parse layer
/// consumes. Errors on any key (or value shape) it does not understand.
fn py_config_from_conf(conf: &BTreeMap<String, serde_json::Value>) -> Result<PySigConfig, String> {
    use serde_json::Value;

    fn opt_i64(key: &str, value: &Value) -> Result<Option<i64>, String> {
        match value {
            Value::Null => Ok(None),
            Value::Number(n) => n
                .as_i64()
                .map(Some)
                .ok_or_else(|| format!("conf key {key}: non-integer number {n}")),
            other => Err(format!("conf key {key}: expected integer, got {other}")),
        }
    }
    fn boolean(key: &str, value: &Value) -> Result<bool, String> {
        value
            .as_bool()
            .ok_or_else(|| format!("conf key {key}: expected bool, got {value}"))
    }
    fn string(key: &str, value: &Value) -> Result<String, String> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("conf key {key}: expected string, got {value}"))
    }

    let mut py = PySigConfig::default();
    for (key, value) in conf {
        match key.as_str() {
            "maximum_signature_line_length" => {
                py.maximum_signature_line_length = opt_i64(key, value)?;
            }
            "python_maximum_signature_line_length" => {
                py.python_maximum_signature_line_length = opt_i64(key, value)?;
            }
            "python_trailing_comma_in_multi_line_signatures" => {
                py.python_trailing_comma_in_multi_line_signatures = boolean(key, value)?;
            }
            "python_display_short_literal_types" => {
                py.python_display_short_literal_types = boolean(key, value)?;
            }
            "python_use_unqualified_type_names" => {
                py.python_use_unqualified_type_names = boolean(key, value)?;
            }
            "toc_object_entries" => py.toc_object_entries = boolean(key, value)?,
            "toc_object_entries_show_parents" => {
                py.toc_object_entries_show_parents = string(key, value)?;
            }
            "add_function_parentheses" => py.add_function_parentheses = boolean(key, value)?,
            "add_module_names" => py.add_module_names = boolean(key, value)?,
            "strip_signature_backslash" => py.strip_signature_backslash = boolean(key, value)?,
            other => {
                return Err(format!(
                    "unmapped conf key {other:?}: teach py_config_from_conf about it \
                     (and the parse layer, if it is not a PySigConfig knob)"
                ));
            }
        }
    }
    Ok(py)
}

#[test]
fn an_unmapped_conf_key_fails() {
    let mut conf = BTreeMap::new();
    conf.insert(
        "python_no_such_setting".to_string(),
        serde_json::Value::Bool(true),
    );
    let err = py_config_from_conf(&conf).unwrap_err();
    assert!(
        err.contains("unmapped conf key \"python_no_such_setting\""),
        "unexpected error text: {err}"
    );

    // A mapped key with the wrong value shape fails too.
    let mut conf = BTreeMap::new();
    conf.insert(
        "add_function_parentheses".to_string(),
        serde_json::Value::String("yes".to_string()),
    );
    assert!(py_config_from_conf(&conf).is_err());
}

#[test]
fn a_mapped_conf_translates_onto_py_sig_config() {
    let conf: BTreeMap<String, serde_json::Value> = serde_json::from_str(
        r#"{
            "maximum_signature_line_length": 8,
            "python_maximum_signature_line_length": null,
            "python_trailing_comma_in_multi_line_signatures": false,
            "python_use_unqualified_type_names": true,
            "add_function_parentheses": false
        }"#,
    )
    .unwrap();
    let py = py_config_from_conf(&conf).unwrap();
    assert_eq!(
        py,
        PySigConfig {
            maximum_signature_line_length: Some(8),
            python_maximum_signature_line_length: None,
            python_trailing_comma_in_multi_line_signatures: false,
            python_use_unqualified_type_names: true,
            add_function_parentheses: false,
            ..PySigConfig::default()
        }
    );
}

/// The read settings of a build whose configuration sets `keep_warnings`
/// as `settings` records it: a [`BuildConfig`] with M1's validation off
/// (decision 7), projected the way a build projects it.
fn read_settings_of(settings: &FixtureSettings) -> ReadSettings {
    assert!(
        !settings.smartquotes,
        "no smart-quote transform exists yet, so only `smartquotes = False` \
         reads like the oracle"
    );
    ReadSettings::from_config(&BuildConfig {
        validate_directives: false,
        keep_warnings: settings.keep_warnings,
        ..BuildConfig::default()
    })
}

/// The pformat of our post-read doctree for `rst`, read standalone as the
/// oracle harness reads it.
fn read_pformat(rst: &str, parse: &ParseOptions, settings: &ReadSettings) -> String {
    read_standalone(rst, parse, settings)
        .outcome
        .doctree()
        .root
        .pformat()
}

#[test]
fn matches_sphinx_oracle_pformat() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sphinx_doctree_differential.json"
    ));
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert_eq!(fixture.docutils_version, "0.22.4");
    assert_eq!(fixture.sphinx_version, "9.1.0");
    // Consumer-side anti-truncation floor, raised with the generator's in
    // wave-4.5 task 16 (both were set against a much smaller corpus and had
    // gone slack: 300 here against 426 committed cases). The generator
    // carries the matching global floor plus per-family ones.
    assert!(
        fixture.cases.len() >= 400,
        "fixture truncated? only {} cases",
        fixture.cases.len()
    );

    let settings = read_settings_of(&fixture.settings);
    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let py = match py_config_from_conf(&case.conf) {
            Ok(py) => py,
            Err(err) => {
                mismatches.push(format!("[{}] CONF ERROR: {err}", case.name));
                continue;
            }
        };
        let rst = case.rst.clone();
        let settings = settings.clone();
        let ours = std::panic::catch_unwind(move || {
            let parse = ParseOptions {
                source_path: "<snippet>".into(),
                sphinx: true,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                py,
                found_docs: None,
                srcdir: None,
                ..Default::default()
            };
            read_pformat(&rst, &parse, &settings)
        });
        match ours {
            Err(_) => mismatches.push(format!("[{}] PANICKED on:\n{}", case.name, case.rst)),
            Ok(got) if got != case.pseudo_xml => mismatches.push(format!(
                "[{}] MISMATCH\n--- rst ---\n{}\n--- sphinx 9.1.0 ---\n{}\n--- ours ---\n{}",
                case.name, case.rst, case.pseudo_xml, got
            )),
            Ok(_) => {}
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} divergence(s) from the sphinx 9.1.0 oracle:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// The HTML oracle's docutils snippets, post-read
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct DocutilsFixture {
    docutils_version: String,
    sphinx_version: String,
    cases: Vec<DocutilsCase>,
}

/// One HTML-oracle docutils snippet: its input, the `conf.py` the oracle
/// read it with and Sphinx's post-read doctree. (The fixture's `resolved`,
/// the dummy builder's resolved doctree, is compared from T14.)
#[derive(serde::Deserialize)]
struct DocutilsCase {
    name: String,
    rst: String,
    conf: String,
    post_read: String,
}

fn docutils_fixture() -> DocutilsFixture {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sphinx_docutils_doctree_differential.json"
    ));
    let fixture: DocutilsFixture = serde_json::from_str(raw).expect("fixture parses");
    assert_eq!(fixture.docutils_version, "0.22.4");
    assert_eq!(fixture.sphinx_version, "9.1.0");
    fixture
}

/// The docutils snippets of the HTML oracle's core ledger that its harness
/// runs: status `built` or `build-error`, never a `reference-crash`
/// (`CaseStatus::is_runnable`, `tests/support/html_oracle.rs`; wave 5a
/// decision 13).
fn runnable_docutils_snippets() -> BTreeSet<String> {
    #[derive(serde::Deserialize)]
    struct Ledger {
        cases: Vec<LedgerCase>,
    }
    #[derive(serde::Deserialize)]
    struct LedgerCase {
        case_id: String,
        source_set: String,
        status: String,
    }
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/html_oracle/core/index.json"
    );
    let raw = std::fs::read_to_string(path).expect("the core ledger reads");
    let ledger: Ledger = serde_json::from_str(&raw).expect("the core ledger parses");
    ledger
        .cases
        .into_iter()
        .filter(|case| {
            case.source_set == "docutils_snippets"
                && matches!(case.status.as_str(), "built" | "build-error")
        })
        .map(|case| case.case_id)
        .collect()
}

/// The build configuration the oracle's `conf.py` gives, read the way the
/// CLI reads it, with M1's validation off (decision 7).
fn conf_py_config(conf: &str) -> BuildConfig {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("conf.py");
    std::fs::write(&path, conf).expect("conf.py writes");
    let config = BuildConfig::from_conf_py(&path).expect("the oracle conf.py loads");
    assert_eq!(
        config.conf_values.get("smartquotes"),
        Some(&serde_json::Value::Bool(false)),
        "no smart-quote transform exists yet, so only `smartquotes = False` reads \
         like the oracle"
    );
    BuildConfig {
        validate_directives: false,
        ..config
    }
}

/// Every docutils snippet whose post-read doctree differs from the
/// oracle's, by case: `(expected, actual)` pformats, a panic reported as
/// the actual text.
fn post_read_mismatches(fixture: &DocutilsFixture) -> BTreeMap<String, (String, String)> {
    let mut configs: BTreeMap<&str, BuildConfig> = BTreeMap::new();
    let mut mismatches = BTreeMap::new();
    for case in &fixture.cases {
        let config = configs
            .entry(case.conf.as_str())
            .or_insert_with(|| conf_py_config(&case.conf));
        let settings = ReadSettings::from_config(config);
        let parse = ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            exclude_patterns: config.exclude_patterns.clone(),
            py: PySigConfig::from(&*config),
            source_encoding: config.source_encoding.clone(),
            ..ParseOptions::default()
        };
        let rst = case.rst.clone();
        let actual = std::panic::catch_unwind(move || read_pformat(&rst, &parse, &settings))
            .unwrap_or_else(|_| "PANICKED\n".to_string());
        if actual != case.post_read {
            mismatches.insert(case.name.clone(), (case.post_read.clone(), actual));
        }
    }
    mismatches
}

/// The docutils snippets whose post-read doctree is not Sphinx's yet, each
/// with the fix families its divergence shows ([BASE §3.8]) and the latest
/// task that fixes one of them. Checked strictly: a listed case that
/// matches fails the test, and so does an unlisted case that differs. Rows
/// are printed by [`docutils_snippets_post_read_census`], one per line as
/// the census prints them, so a task deletes or re-attributes a row by
/// editing one line and a grep for an owner shows whole rows.
#[rustfmt::skip]
const KNOWN_POST_READ_GAPS: &[(&str, &str)] = &[
    // Since T10, the census's `dangling refs` in 0236, 0237, 0242 and 0243
    // is an id shift: AnonymousHyperlinks (440) and DanglingReferences (850)
    // make each problematic as Sphinx does, but in Sphinx's read Footnotes
    // (620) and Substitutions (220) report first and take ids for their
    // messages. The census prints `dangling refs: 5a-T10` for 0237 and
    // 0243, which show nothing else; they are T11's.
    ("docutils-0236", "dangling refs+footnotes: 5a-T11"),
    ("docutils-0237", "footnotes: 5a-T11"),
    ("docutils-0240", "citations: 5a-T11"),
    ("docutils-0241", "citations: 5a-T11"),
    ("docutils-0242", "dangling refs+substitutions: 5a-T11"),
    ("docutils-0243", "substitutions: 5a-T11"),
    ("docutils-0280", "footnotes: 5a-T11"),
    ("docutils-0281", "footnotes: 5a-T11"),
    ("docutils-0282", "footnotes: 5a-T11"),
    ("docutils-0283", "footnotes: 5a-T11"),
    ("docutils-0284", "citations: 5a-T11"),
    ("docutils-0285", "footnotes: 5a-T11"),
    ("docutils-0286", "citations: 5a-T11"),
    ("docutils-0287", "footnotes: 5a-T11"),
    ("docutils-0288", "citations: 5a-T11"),
    ("docutils-0289", "footnotes: 5a-T11"),
    ("docutils-0290", "footnotes: 5a-T11"),
    ("docutils-0291", "footnotes: 5a-T11"),
    ("docutils-0292", "citations+footnotes: 5a-T11"),
    ("docutils-0293", "footnotes: 5a-T11"),
    ("docutils-0294", "footnotes: 5a-T11"),
    ("docutils-0295", "footnotes: 5a-T11"),
    ("docutils-0297", "docinfo: 5a-T12"),
    ("docutils-0300", "docinfo: 5a-T12"),
    ("docutils-0301", "docinfo: 5a-T12"),
    ("docutils-0303", "docinfo: 5a-T12"),
    ("docutils-0304", "docinfo: 5a-T12"),
    ("docutils-0305", "docinfo: 5a-T12"),
    ("docutils-0306", "docinfo: 5a-T12"),
    ("docutils-0307", "docinfo: 5a-T12"),
    ("docutils-0308", "docinfo: 5a-T12"),
    ("docutils-0309", "docinfo: 5a-T12"),
    ("docutils-0310", "docinfo: 5a-T12"),
    ("docutils-0311", "docinfo: 5a-T12"),
    ("docutils-0359", "footnotes: 5a-T11"),
    ("docutils-0360", "footnotes: 5a-T11"),
    ("docutils-0362", "docinfo: 5a-T12"),
    ("docutils-0367", "substitutions: 5a-T11"),
    ("docutils-0368", "footnotes: 5a-T11"),
    ("docutils-0371", "docinfo: 5a-T12"),
    ("docutils-0372", "footnotes: 5a-T11"),
    ("docutils-0375", "footnotes: 5a-T11"),
    ("docutils-0425", "candidates: 5a-T13a"),
    ("docutils-0426", "candidates: 5a-T13a"),
    ("docutils-0427", "candidates: 5a-T13a"),
    ("docutils-0428", "candidates: 5a-T13a"),
    ("docutils-0434", "candidates: 5a-T13a"),
    ("docutils-0435", "candidates: 5a-T13a"),
    ("docutils-0437", "candidates: 5a-T13a"),
    ("docutils-0438", "candidates: 5a-T13a"),
    ("docutils-0442", "candidates: 5a-T13a"),
    ("docutils-0443", "candidates: 5a-T13a"),
    ("docutils-0446", "candidates: 5a-T13a"),
    ("docutils-0447", "candidates: 5a-T13a"),
    ("docutils-0448", "candidates: 5a-T13a"),
    ("docutils-0449", "candidates: 5a-T13a"),
    ("docutils-0450", "candidates: 5a-T13a"),
    ("docutils-0451", "candidates: 5a-T13a"),
    ("docutils-0489", "autonumber ids+candidates: 5a-T13a"),
    ("docutils-0490", "autonumber ids+candidates: 5a-T13a"),
    ("docutils-0491", "candidates: 5a-T13a"),
    ("docutils-0492", "candidates: 5a-T13a"),
    ("docutils-0493", "autonumber ids+candidates: 5a-T13a"),
    ("docutils-0494", "autonumber ids+candidates: 5a-T13a"),
    ("docutils-0496", "candidates: 5a-T13a"),
    ("docutils-0497", "candidates: 5a-T13a"),
    ("docutils-0498", "candidates: 5a-T13a"),
    ("docutils-0499", "candidates: 5a-T13a"),
    ("docutils-0501", "candidates: 5a-T13a"),
    ("docutils-0502", "autonumber ids+candidates: 5a-T13a"),
    ("docutils-0534", "autonumber ids: 5a-T12"),
    ("docutils-0543", "autonumber ids: 5a-T12"),
    ("docutils-0548", "autonumber ids: 5a-T12"),
    ("docutils-0559", "autonumber ids: 5a-T12"),
    ("docutils-0568", "substitutions: 5a-T11"),
    ("docutils-0590", "candidates: 5a-T13a"),
    ("docutils-0591", "candidates: 5a-T13a"),
    ("docutils-0593", "candidates: 5a-T13a"),
    ("docutils-0611", "substitutions: 5a-T11"),
    ("docutils-0615", "footnotes: 5a-T11"),
    ("docutils-0616", "footnotes: 5a-T11"),
    ("docutils-0617", "citations: 5a-T11"),
    ("docutils-0685", "candidates: 5a-T13a"),
    ("docutils-0707", "candidates: 5a-T13a"),
    ("docutils-0708", "candidates: 5a-T13a"),
    ("docutils-0712", "substitutions: 5a-T11"),
    ("docutils-0716", "candidates: 5a-T13a"),
    ("docutils-0725", "docinfo: 5a-T12"),
    ("docutils-0726", "docinfo: 5a-T12"),
];

/// Holds Sphinx's post-read doctree of every runnable HTML-oracle docutils
/// snippet, read under the oracle's `conf.py`, to [`KNOWN_POST_READ_GAPS`].
#[test]
fn docutils_snippets_post_read_differential() {
    let fixture = docutils_fixture();
    let names: BTreeSet<String> = fixture.cases.iter().map(|case| case.name.clone()).collect();
    assert_eq!(names.len(), fixture.cases.len(), "a case name repeats");
    assert_eq!(
        names,
        runnable_docutils_snippets(),
        "the fixture's cases are not the ledger's runnable docutils snippets"
    );

    let mismatches = post_read_mismatches(&fixture);
    let known: BTreeSet<&str> = KNOWN_POST_READ_GAPS.iter().map(|(case, _)| *case).collect();
    assert_eq!(
        known.len(),
        KNOWN_POST_READ_GAPS.len(),
        "KNOWN_POST_READ_GAPS lists a case twice"
    );
    let observed: BTreeSet<&str> = mismatches.keys().map(String::as_str).collect();
    let regressions: Vec<String> = observed
        .difference(&known)
        .map(|case| {
            let (expected, actual) = &mismatches[*case];
            let (line, expected, actual) = first_differing_line(expected, actual);
            format!(
                "    {case}: line {line}\n        expected: {expected:?}\n        actual:   {actual:?}"
            )
        })
        .collect();
    let fixed: Vec<&str> = known.difference(&observed).copied().collect();
    assert!(
        regressions.is_empty() && fixed.is_empty(),
        "the post-read mismatch set differs from KNOWN_POST_READ_GAPS: {} regressions, {} fixed \
         (delete their rows: {fixed:?})\n{}",
        regressions.len(),
        fixed.len(),
        regressions.join("\n")
    );
}

/// The first line, numbered from 1, where two texts differ, and that line
/// of each (empty past a text's end).
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

/// Prints the current post-read mismatch set as `KNOWN_POST_READ_GAPS`
/// rows. A row's reason lists the fix families its divergence shows and the
/// latest task that fixes one of them ([`support::read_gaps`]); a
/// divergence no family explains gets a `?` family and a comment above its
/// row, and fails the census once every row is printed:
///
/// `cargo test --locked --test sphinx_doctree_differential docutils_snippets_post_read_census -- --ignored --nocapture`
#[test]
#[ignore]
fn docutils_snippets_post_read_census() {
    let fixture = docutils_fixture();
    let mut table = String::new();
    let mut unexplained_rows = 0;
    let mismatches = post_read_mismatches(&fixture);
    for (case, (expected, actual)) in &mismatches {
        let census = support::read_gaps::tree_census(expected, actual);
        if !census.unexplained.is_empty() {
            unexplained_rows += 1;
        }
        for note in &census.unexplained {
            for line in note.lines() {
                table.push_str(&format!("    // ? {line}\n"));
            }
        }
        table.push_str(&format!("    ({case:?}, {:?}),\n", census.reason()));
    }
    println!("    // {} rows\n{table}", mismatches.len());
    assert_eq!(
        unexplained_rows, 0,
        "rows with a `?` family need a family the census does not know"
    );
}
