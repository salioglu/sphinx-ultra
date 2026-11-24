//! Differential tests: our read vs the SPHINX ORACLE — the pseudo-XML a real
//! `sphinx-build` 9.1.0 read phase (dummy builder, `extensions = []`)
//! produces, for two committed corpora:
//!
//! - `sphinx_doctree_differential.json`, one-snippet reads under the
//!   fixture's own settings (smartquotes off, keep_warnings on);
//! - `sphinx_docutils_doctree_differential.json`, the post-read doctrees of
//!   the HTML oracle's docutils snippets, each read under the oracle's own
//!   `conf.py` (keep_warnings off) — compared strictly against
//!   [`KNOWN_POST_READ_GAPS`] — and the dummy builder's resolved doctrees
//!   of the same reads ([`resolve_standalone`]), compared strictly against
//!   [`KNOWN_RESOLVED_STANDALONE_GAPS`].
//!
//! Regenerate the fixtures (manual, never in CI), with the locked core
//! oracle profile:
//!     PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
//!         --project tools/oracle_profiles/core python tools/gen_sphinx_fixture.py
//! and the same with `--corpus html-oracle-docutils` for the second.
//!
//! Both read through [`read_standalone`] with the [`ReadSettings`] the
//! fixture's recorded settings give through a [`BuildConfig`] (M1's
//! directive and role validation off, wave 5a decision 7) for the dummy
//! builder the oracle builds with, never Sphinx's bare defaults, and
//! compare its post-read doctree as it is. Both corpora read with smart
//! quotes off: the fixture's header says so, and so does the oracle's
//! `conf.py`; tests/sphinx_smartquotes_differential.rs reads the first
//! corpus with them on.
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
//! the parse options through tests/support/fixture_conf.rs, which the
//! smart-quotes variant consumer shares; an unmapped key is a hard error so
//! a future generator-side conf addition fails HERE instead of silently
//! parsing under defaults (serde ignores unknown struct fields, so without
//! the explicit map a conf case would quietly lose its config).

mod support {
    pub mod fixture_conf;
    pub mod read_gaps;
    pub mod text_diff;
}

use std::collections::{BTreeMap, BTreeSet};

use sphinx_ultra::builder::BuilderKind;
use sphinx_ultra::config::BuildConfig;
use sphinx_ultra::py::PySigConfig;
use sphinx_ultra::read::{read_standalone, resolve_standalone, ReadSettings};
use sphinx_ultra::rst::{ParseOptions, ReadConfig};

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

#[test]
fn an_unmapped_conf_key_fails() {
    let mut conf = BTreeMap::new();
    conf.insert(
        "python_no_such_setting".to_string(),
        serde_json::Value::Bool(true),
    );
    let err = support::fixture_conf::snippet_parse_options(&conf).unwrap_err();
    assert!(
        err.contains("unmapped conf key \"python_no_such_setting\""),
        "unexpected error text: {err}"
    );

    // A mapped key with the wrong value shape fails too, a non-integer line
    // length included.
    for (key, value) in [
        ("add_function_parentheses", serde_json::json!("yes")),
        ("maximum_signature_line_length", serde_json::json!(8.5)),
        ("default_role", serde_json::json!(1)),
    ] {
        let conf = BTreeMap::from([(key.to_string(), value)]);
        assert!(
            support::fixture_conf::snippet_parse_options(&conf).is_err(),
            "{key}"
        );
    }
}

#[test]
fn a_mapped_conf_translates_onto_the_parse_options() {
    let conf: BTreeMap<String, serde_json::Value> = serde_json::from_str(
        r#"{
            "maximum_signature_line_length": 8,
            "python_maximum_signature_line_length": null,
            "python_trailing_comma_in_multi_line_signatures": false,
            "python_use_unqualified_type_names": true,
            "add_function_parentheses": false,
            "default_role": "any",
            "primary_domain": null,
            "highlight_language": "python3",
            "manpages_url": "https://man.example/{page}"
        }"#,
    )
    .unwrap();
    let parse = support::fixture_conf::snippet_parse_options(&conf).unwrap();
    assert_eq!(
        parse.py,
        PySigConfig {
            maximum_signature_line_length: Some(8),
            python_maximum_signature_line_length: None,
            python_trailing_comma_in_multi_line_signatures: false,
            python_use_unqualified_type_names: true,
            add_function_parentheses: false,
            ..PySigConfig::default()
        }
    );
    assert_eq!(
        parse.read,
        ReadConfig {
            default_role: Some("any".to_string()),
            primary_domain: None,
            highlight_language: "python3".to_string(),
            manpages_url: Some("https://man.example/{page}".to_string()),
            ..ReadConfig::default()
        }
    );
    assert_eq!(
        (
            parse.source_path.as_str(),
            parse.sphinx,
            parse.docname.as_str()
        ),
        ("<snippet>", true, "index")
    );
}

/// The read settings of a build whose configuration sets `keep_warnings`
/// and `smartquotes` as `settings` records them: a [`BuildConfig`] with
/// M1's validation off (decision 7), projected the way a dummy build
/// projects it.
fn read_settings_of(settings: &FixtureSettings) -> ReadSettings {
    ReadSettings::from_config(
        &BuildConfig {
            validate_directives: false,
            keep_warnings: settings.keep_warnings,
            smartquotes: settings.smartquotes,
            ..BuildConfig::default()
        },
        BuilderKind::Dummy,
    )
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
        let parse = match support::fixture_conf::snippet_parse_options(&case.conf) {
            Ok(parse) => parse,
            Err(err) => {
                mismatches.push(format!("[{}] CONF ERROR: {err}", case.name));
                continue;
            }
        };
        let rst = case.rst.clone();
        let settings = settings.clone();
        let ours = std::panic::catch_unwind(move || read_pformat(&rst, &parse, &settings));
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
// The HTML oracle's docutils snippets, post-read and resolved
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct DocutilsFixture {
    docutils_version: String,
    sphinx_version: String,
    cases: Vec<DocutilsCase>,
}

/// One HTML-oracle docutils snippet: its input, the `conf.py` the oracle
/// read it with, Sphinx's post-read doctree and the dummy builder's
/// resolved doctree.
#[derive(serde::Deserialize)]
struct DocutilsCase {
    name: String,
    rst: String,
    conf: String,
    post_read: String,
    resolved: String,
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
/// CLI reads it, with M1's validation off (decision 7). The oracle turns
/// smart quotes off there (`smartquotes = False`), which the read honours.
fn conf_py_config(conf: &str) -> BuildConfig {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("conf.py");
    std::fs::write(&path, conf).expect("conf.py writes");
    let config = BuildConfig::from_conf_py(&path).expect("the oracle conf.py loads");
    assert!(
        !config.smartquotes,
        "the oracle's conf.py turns smart quotes off"
    );
    BuildConfig {
        validate_directives: false,
        ..config
    }
}

/// Every docutils snippet whose doctree, as `compare` gives it from the
/// snippet's standalone read under the oracle's `conf.py`, differs from
/// the tree `expected` takes from the fixture, by case: `(expected,
/// actual)` pformats, a panic reported as the actual text.
fn snippet_mismatches(
    fixture: &DocutilsFixture,
    expected: fn(&DocutilsCase) -> &str,
    compare: fn(&str, &ParseOptions, &ReadSettings) -> String,
) -> BTreeMap<String, (String, String)> {
    let mut configs: BTreeMap<&str, BuildConfig> = BTreeMap::new();
    let mut mismatches = BTreeMap::new();
    for case in &fixture.cases {
        let config = configs
            .entry(case.conf.as_str())
            .or_insert_with(|| conf_py_config(&case.conf));
        let settings = ReadSettings::from_config(config, BuilderKind::Dummy);
        let parse = ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            exclude_patterns: config.exclude_patterns.clone(),
            py: PySigConfig::from(&*config),
            source_encoding: config.source_encoding.clone(),
            read: ReadConfig::from(&*config),
            ..ParseOptions::default()
        };
        let rst = case.rst.clone();
        let actual = std::panic::catch_unwind(move || compare(&rst, &parse, &settings))
            .unwrap_or_else(|_| "PANICKED\n".to_string());
        let expected = expected(case);
        if actual != expected {
            mismatches.insert(case.name.clone(), (expected.to_string(), actual));
        }
    }
    mismatches
}

/// Every docutils snippet whose post-read doctree differs from the
/// oracle's ([`snippet_mismatches`]).
fn post_read_mismatches(fixture: &DocutilsFixture) -> BTreeMap<String, (String, String)> {
    snippet_mismatches(fixture, |case| &case.post_read, read_pformat)
}

/// The pformat of the doctree [`resolve_standalone`] gives for `rst`: the
/// dummy builder's write, as the oracle's `get_and_resolve_doctree`.
fn resolved_pformat(rst: &str, parse: &ParseOptions, settings: &ReadSettings) -> String {
    let read = read_standalone(rst, parse, settings);
    resolve_standalone(&read, settings, BuilderKind::Dummy)
        .0
        .root
        .pformat()
}

/// Every docutils snippet whose resolved doctree differs from the
/// oracle's ([`snippet_mismatches`]).
fn resolved_mismatches(fixture: &DocutilsFixture) -> BTreeMap<String, (String, String)> {
    snippet_mismatches(fixture, |case| &case.resolved, resolved_pformat)
}

/// The docutils snippets whose post-read doctree is not Sphinx's yet, each
/// with the fix families its divergence shows ([BASE §3.8]) and the latest
/// task that fixes one of them. Checked strictly: a listed case that
/// matches fails the test, and so does an unlisted case that differs. Rows
/// are printed by [`docutils_snippets_post_read_census`], one per line as
/// the census prints them, so a task deletes or re-attributes a row by
/// editing one line and a grep for an owner shows whole rows.
#[rustfmt::skip]
const KNOWN_POST_READ_GAPS: &[(&str, &str)] = &[];

/// The docutils snippets whose resolved doctree ([`resolve_standalone`],
/// the dummy builder's write) is not Sphinx's yet, as
/// [`KNOWN_POST_READ_GAPS`] lists the post-read ones: strictly, one row per
/// line as [`docutils_snippets_resolved_census`] prints it.
#[rustfmt::skip]
const KNOWN_RESOLVED_STANDALONE_GAPS: &[(&str, &str)] = &[];

/// Holds a mismatch set to its strict gap table: every listed case must
/// differ and every differing case must be listed.
fn assert_gaps(
    table: &str,
    gaps: &[(&str, &str)],
    mismatches: &BTreeMap<String, (String, String)>,
) {
    let known: BTreeSet<&str> = gaps.iter().map(|(case, _)| *case).collect();
    assert_eq!(known.len(), gaps.len(), "{table} lists a case twice");
    let observed: BTreeSet<&str> = mismatches.keys().map(String::as_str).collect();
    let regressions: Vec<String> = observed
        .difference(&known)
        .map(|case| {
            let (expected, actual) = &mismatches[*case];
            match support::text_diff::first_differing_line(expected, actual) {
                Some((line, expected, actual)) => format!(
                    "    {case}: line {line}\n        expected: {expected:?}\n        actual:   \
                     {actual:?}"
                ),
                None => format!("    {case}: only a final line break differs"),
            }
        })
        .collect();
    let fixed: Vec<&str> = known.difference(&observed).copied().collect();
    assert!(
        regressions.is_empty() && fixed.is_empty(),
        "the mismatch set differs from {table}: {} regressions, {} fixed (delete their rows: \
         {fixed:?})\n{}",
        regressions.len(),
        fixed.len(),
        regressions.join("\n")
    );
}

/// The fixture's case-name set is the ledger's runnable docutils snippets.
fn assert_fixture_cases(fixture: &DocutilsFixture) {
    let names: BTreeSet<String> = fixture.cases.iter().map(|case| case.name.clone()).collect();
    assert_eq!(names.len(), fixture.cases.len(), "a case name repeats");
    assert_eq!(
        names,
        runnable_docutils_snippets(),
        "the fixture's cases are not the ledger's runnable docutils snippets"
    );
}

/// Holds Sphinx's post-read doctree of every runnable HTML-oracle docutils
/// snippet, read under the oracle's `conf.py`, to [`KNOWN_POST_READ_GAPS`].
#[test]
fn docutils_snippets_post_read_differential() {
    let fixture = docutils_fixture();
    assert_fixture_cases(&fixture);
    assert_gaps(
        "KNOWN_POST_READ_GAPS",
        KNOWN_POST_READ_GAPS,
        &post_read_mismatches(&fixture),
    );
}

/// Holds the dummy builder's resolved doctree of every runnable HTML-oracle
/// docutils snippet ([`resolve_standalone`]) to
/// [`KNOWN_RESOLVED_STANDALONE_GAPS`].
#[test]
fn docutils_snippets_resolved_differential() {
    let fixture = docutils_fixture();
    assert_fixture_cases(&fixture);
    assert_gaps(
        "KNOWN_RESOLVED_STANDALONE_GAPS",
        KNOWN_RESOLVED_STANDALONE_GAPS,
        &resolved_mismatches(&fixture),
    );
}

/// Prints `mismatches` as gap-table rows. A row's reason lists the fix
/// families its divergence shows and the latest task that fixes one of
/// them ([`support::read_gaps`]); a divergence no family explains gets a
/// `?` family and a comment above its row, and fails the census once
/// every row is printed.
fn print_census(mismatches: &BTreeMap<String, (String, String)>) {
    let mut table = String::new();
    let mut unexplained_rows = 0;
    for (case, (expected, actual)) in mismatches {
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

/// Prints the current post-read mismatch set as `KNOWN_POST_READ_GAPS`
/// rows ([`print_census`]):
///
/// `cargo test --locked --test sphinx_doctree_differential docutils_snippets_post_read_census -- --ignored --nocapture`
#[test]
#[ignore]
fn docutils_snippets_post_read_census() {
    print_census(&post_read_mismatches(&docutils_fixture()));
}

/// Prints the current resolved mismatch set as
/// `KNOWN_RESOLVED_STANDALONE_GAPS` rows ([`print_census`]):
///
/// `cargo test --locked --test sphinx_doctree_differential docutils_snippets_resolved_census -- --ignored --nocapture`
#[test]
#[ignore]
fn docutils_snippets_resolved_census() {
    print_census(&resolved_mismatches(&docutils_fixture()));
}
