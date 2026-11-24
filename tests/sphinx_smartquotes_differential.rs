//! Differential test: our read with smart quotes on vs the SPHINX ORACLE's,
//! for the snippet corpus tests/sphinx_doctree_differential.rs reads with
//! them off. `tools/gen_smartquotes_fixture.py --variant` reads every case
//! of that corpus through the snippet harness of
//! tools/gen_sphinx_fixture.py twice, with `smartquotes = False` and
//! `True`, and records the cases whose tree or warnings change
//! (tests/fixtures/sphinx_smartquotes_differential.json).
//!
//! Regenerate the fixture (manual, never in CI), with the locked core
//! oracle profile, after tests/fixtures/sphinx_doctree_differential.json:
//!     PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
//!         --project tools/oracle_profiles/core \
//!         python tools/gen_smartquotes_fixture.py --variant
//!
//! Each case is read twice through the read path
//! tests/sphinx_doctree_differential.rs reads its corpus with
//! ([`read_standalone`]: the dummy builder, M1's directive and role
//! validation off, `keep_warnings` on): with smart quotes off, whose tree
//! must be the fixture's `pseudo_xml_off` (the precondition: a failure
//! there is another gap, not a smart-quotes one), and with
//! [`SmartQuotesSettings::default`] in the read settings, so that the
//! transform runs at 750 in stage R over the escape data the parse keeps:
//! its tree must be `pseudo_xml`, the warning lines it prints that the
//! first read did not must be `smartquotes_warnings`, and every line the
//! first read printed it must print too. The transform is never applied to
//! a post-read tree, which keeps no escape data.
//!
//! Every other case of the base corpus is one Sphinx's smart quotes leave
//! as they are: read the same two ways, both trees must be the base
//! fixture's `pseudo_xml`, and the two reads must print the same warning
//! lines. The base consumer reads that corpus with no skips and no
//! known-gap table, so neither is applied here either.
//!
//! Clones the tests/sphinx_doctree_differential.rs shape: committed JSON,
//! version assertions, a floor against silent truncation, every mismatch
//! collected before asserting, and panics reported as named mismatches. A
//! case's `conf` maps onto `ParseOptions.py` exactly as there, an unmapped
//! key an error.

use std::collections::{BTreeMap, BTreeSet};

use sphinx_ultra::builder::BuilderKind;
use sphinx_ultra::config::BuildConfig;
use sphinx_ultra::py::PySigConfig;
use sphinx_ultra::read::{read_standalone, ReadSettings};
use sphinx_ultra::rst::ParseOptions;
use sphinx_ultra::transforms::smartquotes::SmartQuotesSettings;

#[derive(serde::Deserialize)]
struct Fixture {
    docutils_version: String,
    sphinx_version: String,
    base_fixture: String,
    settings: FixtureSettings,
    cases: Vec<Case>,
}

/// The settings the generator read the corpus with the second time, as it
/// recorded them from the oracle app.
#[derive(serde::Deserialize)]
struct FixtureSettings {
    smartquotes: bool,
    smartquotes_action: String,
    language: String,
    keep_warnings: bool,
    builder: String,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
    #[serde(default)]
    conf: BTreeMap<String, serde_json::Value>,
    pseudo_xml_off: String,
    pseudo_xml: String,
    smartquotes_warnings: String,
}

/// The base corpus (tests/fixtures/sphinx_doctree_differential.json), which
/// the variant names as its `base_fixture`: what this test reads of it.
#[derive(serde::Deserialize)]
struct BaseFixture {
    docutils_version: String,
    sphinx_version: String,
    settings: BaseSettings,
    cases: Vec<BaseCase>,
}

/// The base corpus' read settings, as its generator recorded them.
#[derive(serde::Deserialize)]
struct BaseSettings {
    smartquotes: bool,
    keep_warnings: bool,
}

#[derive(serde::Deserialize)]
struct BaseCase {
    name: String,
    rst: String,
    #[serde(default)]
    conf: BTreeMap<String, serde_json::Value>,
    pseudo_xml: String,
}

/// Map a fixture case's `conf` dict onto the [`PySigConfig`] the parse layer
/// consumes, as tests/sphinx_doctree_differential.rs maps it. Errors on any
/// key (or value shape) it does not understand.
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
}

/// The two read settings of a case: the base corpus's (smart quotes off)
/// and the variant's ([`SmartQuotesSettings::default`]), both those of a
/// dummy build configured as the fixture's header says, with M1's
/// validation off (decision 7). The header's settings are Sphinx's
/// defaults, which is what the default smart-quotes settings are.
fn read_settings_of(settings: &FixtureSettings) -> (ReadSettings, ReadSettings) {
    assert!(
        settings.smartquotes,
        "the variant reads with smart quotes on"
    );
    assert_eq!(settings.builder, "dummy");
    let config = BuildConfig {
        validate_directives: false,
        keep_warnings: settings.keep_warnings,
        language: Some(settings.language.clone()),
        smartquotes_action: settings.smartquotes_action.clone(),
        ..BuildConfig::default()
    };
    let on = ReadSettings::from_config(&config, BuilderKind::Dummy);
    assert_eq!(on.smartquotes, Some(SmartQuotesSettings::default()));
    let off = ReadSettings::from_config(
        &BuildConfig {
            smartquotes: false,
            ..config
        },
        BuilderKind::Dummy,
    );
    assert_eq!(off.smartquotes, None);
    let on = ReadSettings {
        smartquotes: Some(SmartQuotesSettings::default()),
        ..off.clone()
    };
    (off, on)
}

/// The pformat of our post-read doctree for `rst` under `settings`, and the
/// lines of the warning stream the read printed.
fn read(rst: &str, parse: &ParseOptions, settings: &ReadSettings) -> (String, Vec<String>) {
    let read = read_standalone(rst, parse, settings);
    let lines = read
        .diagnostics
        .iter()
        .flat_map(|record| {
            record
                .render(true)
                .split('\n')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    (read.outcome.doctree().root.pformat(), lines)
}

/// How the warning lines `on` differ from `off`, each line of `off`
/// matching one of `on`: the lines of `on` that `off` does not hold, in
/// `on`'s order (the warnings a read adds), and the lines of `off` that
/// `on` does not hold (the warnings it removes), each line ending with a
/// line break.
fn line_changes(on: &[String], off: &[String]) -> (String, String) {
    let mut budget: BTreeMap<&str, usize> = BTreeMap::new();
    for line in off {
        *budget.entry(line.as_str()).or_default() += 1;
    }
    let added = on
        .iter()
        .filter(|line| match budget.get_mut(line.as_str()) {
            Some(left) if *left > 0 => {
                *left -= 1;
                false
            }
            _ => true,
        })
        .map(|line| format!("{line}\n"))
        .collect();
    let removed = budget
        .into_iter()
        .flat_map(|(line, left)| std::iter::repeat_n(format!("{line}\n"), left))
        .collect();
    (added, removed)
}

/// The parse options of a case read as the snippet harness reads it, under
/// the `PySigConfig` its `conf` maps onto.
fn parse_options(py: PySigConfig) -> ParseOptions {
    ParseOptions {
        source_path: "<snippet>".into(),
        sphinx: true,
        docname: "index".into(),
        py,
        ..Default::default()
    }
}

/// Our two reads of `rst`, under `off` and then `on`: each one's tree, and
/// how the warning lines of the second differ from the first's
/// ([`line_changes`]). `None` when a read panicked.
fn read_both(
    rst: &str,
    parse: ParseOptions,
    off: &ReadSettings,
    on: &ReadSettings,
) -> Option<(String, String, (String, String))> {
    let (rst, off, on) = (rst.to_string(), off.clone(), on.clone());
    std::panic::catch_unwind(move || {
        let (off_tree, off_lines) = read(&rst, &parse, &off);
        let (on_tree, on_lines) = read(&rst, &parse, &on);
        (off_tree, on_tree, line_changes(&on_lines, &off_lines))
    })
    .ok()
}

#[test]
fn smartquotes_variant_differential() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sphinx_smartquotes_differential.json"
    ));
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert_eq!(fixture.docutils_version, "0.22.4");
    assert_eq!(fixture.sphinx_version, "9.1.0");
    assert_eq!(
        fixture.base_fixture,
        "tests/fixtures/sphinx_doctree_differential.json"
    );
    // Anti-truncation floor, as the generator's: 63 cases when the fixture
    // was created (wave 5a T16b).
    assert!(
        fixture.cases.len() >= 55,
        "fixture truncated? only {} cases",
        fixture.cases.len()
    );

    let (off, on) = read_settings_of(&fixture.settings);
    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let py = match py_config_from_conf(&case.conf) {
            Ok(py) => py,
            Err(err) => {
                mismatches.push(format!("[{}] CONF ERROR: {err}", case.name));
                continue;
            }
        };
        let Some((off_tree, on_tree, (added, removed))) =
            read_both(&case.rst, parse_options(py), &off, &on)
        else {
            mismatches.push(format!("[{}] PANICKED on:\n{}", case.name, case.rst));
            continue;
        };
        if off_tree != case.pseudo_xml_off {
            mismatches.push(format!(
                "[{}] PRECONDITION (smart quotes off)\n--- rst ---\n{}\n--- sphinx 9.1.0 ---\n{}\n\
                 --- ours ---\n{}",
                case.name, case.rst, case.pseudo_xml_off, off_tree
            ));
        } else if on_tree != case.pseudo_xml
            || added != case.smartquotes_warnings
            || !removed.is_empty()
        {
            mismatches.push(format!(
                "[{}] MISMATCH (smart quotes on)\n--- rst ---\n{}\n--- sphinx 9.1.0 ---\n{}{}\n\
                 --- ours ---\n{}{}--- warnings removed ---\n{}",
                case.name,
                case.rst,
                case.pseudo_xml,
                case.smartquotes_warnings,
                on_tree,
                added,
                removed
            ));
        }
    }

    // The base corpus' other cases, which smart quotes leave as they are.
    let base_raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sphinx_doctree_differential.json"
    ));
    let base: BaseFixture = serde_json::from_str(base_raw).expect("base fixture parses");
    assert_eq!(
        (base.docutils_version.as_str(), base.sphinx_version.as_str()),
        (
            fixture.docutils_version.as_str(),
            fixture.sphinx_version.as_str()
        )
    );
    assert!(
        !base.settings.smartquotes,
        "the base corpus is read with smart quotes off"
    );
    assert_eq!(base.settings.keep_warnings, fixture.settings.keep_warnings);
    let variant: BTreeSet<&str> = fixture
        .cases
        .iter()
        .map(|case| case.name.as_str())
        .collect();
    let mut unchanged = 0;
    for case in &base.cases {
        if variant.contains(case.name.as_str()) {
            continue;
        }
        unchanged += 1;
        let py = match py_config_from_conf(&case.conf) {
            Ok(py) => py,
            Err(err) => {
                mismatches.push(format!("[{}] CONF ERROR: {err}", case.name));
                continue;
            }
        };
        let Some((off_tree, on_tree, (added, removed))) =
            read_both(&case.rst, parse_options(py), &off, &on)
        else {
            mismatches.push(format!("[{}] PANICKED on:\n{}", case.name, case.rst));
            continue;
        };
        if off_tree != case.pseudo_xml {
            mismatches.push(format!(
                "[{}] PRECONDITION (smart quotes off)\n--- rst ---\n{}\n--- sphinx 9.1.0 ---\n{}\n\
                 --- ours ---\n{}",
                case.name, case.rst, case.pseudo_xml, off_tree
            ));
        } else if on_tree != case.pseudo_xml || !added.is_empty() || !removed.is_empty() {
            mismatches.push(format!(
                "[{}] UNCHANGED BY SPHINX (smart quotes on)\n--- rst ---\n{}\n\
                 --- sphinx 9.1.0 ---\n{}\n--- ours ---\n{}--- warnings added ---\n{}\
                 --- warnings removed ---\n{}",
                case.name, case.rst, case.pseudo_xml, on_tree, added, removed
            ));
        }
    }
    // Every base case is read one way or the other: 460 of 523 when this
    // check was added (wave 5a T16b, fix round 1).
    assert_eq!(unchanged + fixture.cases.len(), base.cases.len());
    assert!(
        mismatches.is_empty(),
        "{} divergence(s) from the sphinx 9.1.0 oracle:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}
