//! Differential test: our RST parser and read-transform pass vs the SPHINX
//! ORACLE — the pseudo-XML a real `sphinx-build` 9.1.0 read phase (dummy
//! builder, `extensions = []`, smartquotes off but for the `sq` family,
//! keep_warnings on) produces for
//! the committed fixture corpus. The oracle's tree is the one Sphinx's read
//! transforms leave, so ours is too: every case goes through
//! [`parse_and_transform`] under the fixture's pinned
//! [`fixture_transform_config`], against the oracle's one-document project,
//! and the tree (but for the image `candidates` of [`IMAGE_CANDIDATES`]),
//! the printed records (parse-time and transform-time) and the
//! `env.metadata` the read collected (a case's `metadata`, absent meaning
//! `{}`) are compared.
//!
//! Regenerate the fixture (manual, never in CI):
//!     PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
//!         --with 'docutils==0.22.4' \
//!         python tools/gen_sphinx_fixture.py
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
//! `ParseOptions.py` ([`sphinx_ultra::py::PySigConfig`]) or, since M2 wave 5,
//! onto `ParseOptions.highlight_language` or the read transforms'
//! [`TransformConfig`] (the default substitutions' `version`/`release`/
//! `today`/`today_fmt`, and the `sq` family's `smartquotes`,
//! `smartquotes_action`, `smartquotes_excludes` and `language`, which turn
//! SmartQuotes on over the base's `smartquotes=False`); an unmapped key is a hard
//! error so a future generator-side conf addition fails HERE instead of
//! silently parsing under defaults (serde ignores unknown struct fields, so
//! without the explicit map a conf case would quietly lose its config).
//!
//! The generator pins `SOURCE_DATE_EPOCH` for its whole run and records it in
//! the fixture header (`settings.source_date_epoch`); every case is
//! transformed with that instant as its build date
//! ([`BuildDate::Epoch`]), so `|today|` formats the date Sphinx formatted.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use sphinx_ultra::config::{ExcludeList, SmartquotesExcludes};
use sphinx_ultra::error::BuildWarning;
use sphinx_ultra::py::PySigConfig;
use sphinx_ultra::rst::ParseOptions;
use sphinx_ultra::transforms::{
    parse_and_transform, parse_and_transform_full, BuildDate, TransformConfig,
};

#[derive(serde::Deserialize)]
struct Fixture {
    docutils_version: String,
    sphinx_version: String,
    settings: Settings,
    cases: Vec<Case>,
}

/// The fixture header's `settings` this harness reads.
#[derive(serde::Deserialize)]
struct Settings {
    /// The `SOURCE_DATE_EPOCH` the generator ran under.
    source_date_epoch: i64,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
    pseudo_xml: String,
    /// The records the read phase printed for the snippet, in print order
    /// (see the generator's docstring).
    warnings: Vec<String>,
    #[serde(default)]
    conf: BTreeMap<String, serde_json::Value>,
    /// The `env.metadata` Sphinx's MetadataCollector read off the
    /// document's docinfo (see the generator's docstring, "PER-CASE
    /// METADATA"); absent when it read nothing.
    #[serde(default)]
    metadata: BTreeMap<String, serde_json::Value>,
}

/// Map a fixture case's `conf` dict onto the [`ParseOptions`] the parse
/// layer consumes — its [`PySigConfig`] and its `highlight_language`, every
/// other field at its default for the caller to fill — and the
/// [`TransformConfig`] the read transforms consume, the latter starting from
/// `transforms` (the fixture's base). Errors on any key (or value shape) it
/// does not understand.
fn configs_from_conf(
    conf: &BTreeMap<String, serde_json::Value>,
    transforms: TransformConfig,
) -> Result<(ParseOptions, TransformConfig), String> {
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

    fn opt_string(key: &str, value: &Value) -> Result<Option<String>, String> {
        match value {
            Value::Null => Ok(None),
            other => string(key, other).map(Some),
        }
    }
    /// A `smartquotes_excludes` dict: each half a list of strings, an
    /// absent half `[]` (`.get(..., [])`, `transforms/__init__.py:383-384`).
    fn excludes(key: &str, value: &Value) -> Result<SmartquotesExcludes, String> {
        let map = value
            .as_object()
            .ok_or_else(|| format!("conf key {key}: expected a dict, got {value}"))?;
        let half = |name: &str| -> Result<ExcludeList, String> {
            match map.get(name) {
                None => Ok(ExcludeList::default()),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| string(key, item))
                    .collect::<Result<_, _>>()
                    .map(ExcludeList::Names),
                Some(other) => Err(format!(
                    "conf key {key}.{name}: expected a list, got {other}"
                )),
            }
        };
        if let Some(other) = map
            .keys()
            .find(|k| !["languages", "builders"].contains(&k.as_str()))
        {
            return Err(format!("conf key {key}: unmapped entry {other:?}"));
        }
        Ok(SmartquotesExcludes {
            languages: half("languages")?,
            builders: half("builders")?,
        })
    }

    let mut opts = ParseOptions::default();
    let py = &mut opts.py;
    let mut transforms = transforms;
    for (key, value) in conf {
        match key.as_str() {
            // CodeBlock's default language (`sphinx/directives/code.py:
            // 157-166`), a parse-time read.
            "highlight_language" => opts.highlight_language = string(key, value)?,
            // The default substitutions (DefaultSubstitutions, priority 210).
            "version" => transforms.version = string(key, value)?,
            "release" => transforms.release = string(key, value)?,
            "today" => transforms.today = string(key, value)?,
            "today_fmt" => transforms.today_fmt = opt_string(key, value)?,
            // SphinxSmartQuotes (750): the `sq` family's knobs.
            "smartquotes" => transforms.smartquotes = boolean(key, value)?,
            "smartquotes_action" => transforms.smartquotes_action = string(key, value)?,
            "smartquotes_excludes" => transforms.smartquotes_excludes = excludes(key, value)?,
            "language" => transforms.language = string(key, value)?,
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
                    "unmapped conf key {other:?}: teach configs_from_conf about it \
                     (and the parse layer or the transforms, if it is neither a \
                     ParseOptions nor a TransformConfig knob)"
                ));
            }
        }
    }
    Ok((opts, transforms))
}

/// The read-transform configuration every fixture case was generated under:
/// the generator's fixed `CONFOVERRIDES` (`keep_warnings=True`,
/// `smartquotes=False`), its `dummy` builder and its pinned
/// `SOURCE_DATE_EPOCH`, every other key at Sphinx's default. A case's own
/// `conf` never touches `keep_warnings`, and only the `sq` family's
/// `smartquotes` (the generator asserts both).
fn fixture_transform_config(settings: &Settings) -> TransformConfig {
    TransformConfig {
        keep_warnings: true,
        smartquotes: false,
        builder: "dummy".to_string(),
        build_date: BuildDate::Epoch(settings.source_date_epoch),
        ..TransformConfig::default()
    }
}

#[test]
fn an_unmapped_conf_key_fails() {
    let mut conf = BTreeMap::new();
    conf.insert(
        "python_no_such_setting".to_string(),
        serde_json::Value::Bool(true),
    );
    let err = configs_from_conf(&conf, TransformConfig::default()).unwrap_err();
    assert!(
        err.contains("unmapped conf key \"python_no_such_setting\""),
        "unexpected error text: {err}"
    );

    // A mapped key with the wrong value shape fails too.
    for (key, value) in [
        ("add_function_parentheses", serde_json::json!("yes")),
        ("version", serde_json::json!(1.2)),
        ("today_fmt", serde_json::json!(false)),
    ] {
        let conf = BTreeMap::from([(key.to_string(), value)]);
        assert!(
            configs_from_conf(&conf, TransformConfig::default()).is_err(),
            "{key} accepted a wrong-shaped value"
        );
    }
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
    let (opts, transforms) = configs_from_conf(&conf, TransformConfig::default()).unwrap();
    assert_eq!(transforms, TransformConfig::default());
    assert_eq!(opts.highlight_language, "default");
    assert_eq!(
        opts.py,
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

/// The default substitutions' keys land on the transform configuration,
/// over the base the caller hands in, and leave the py knobs alone.
#[test]
fn a_mapped_conf_translates_onto_the_transform_config() {
    let conf: BTreeMap<String, serde_json::Value> = serde_json::from_str(
        r#"{"version": "1.2", "release": "1.2.3", "today": "Sept 30", "today_fmt": "%Y"}"#,
    )
    .unwrap();
    let base = TransformConfig {
        keep_warnings: true,
        ..TransformConfig::default()
    };
    let (opts, transforms) = configs_from_conf(&conf, base.clone()).unwrap();
    assert_eq!(opts.py, PySigConfig::default());
    assert_eq!(
        transforms,
        TransformConfig {
            version: "1.2".to_string(),
            release: "1.2.3".to_string(),
            today: "Sept 30".to_string(),
            today_fmt: Some("%Y".to_string()),
            ..base
        }
    );
}

/// The `sq` family's SmartQuotes knobs land on the transform
/// configuration over the base; a `smartquotes_excludes` half it leaves
/// out is `[]`, and an entry other than the two halves fails.
#[test]
fn a_mapped_smartquotes_conf_translates_onto_the_transform_config() {
    let conf: BTreeMap<String, serde_json::Value> = serde_json::from_str(
        r#"{"smartquotes": true, "smartquotes_action": "q", "language": "de",
            "smartquotes_excludes": {"builders": ["dummy"]}}"#,
    )
    .unwrap();
    let base = TransformConfig {
        smartquotes: false,
        ..TransformConfig::default()
    };
    let (opts, transforms) = configs_from_conf(&conf, base.clone()).unwrap();
    assert_eq!(opts.py, PySigConfig::default());
    assert_eq!(
        transforms,
        TransformConfig {
            smartquotes: true,
            smartquotes_action: "q".to_string(),
            language: "de".to_string(),
            smartquotes_excludes: SmartquotesExcludes {
                languages: ExcludeList::default(),
                builders: ExcludeList::Names(vec!["dummy".to_string()]),
            },
            ..base
        }
    );
    for bad in [
        serde_json::json!({"languages": "de"}),
        serde_json::json!({"other": []}),
        serde_json::json!(["de"]),
    ] {
        let conf = BTreeMap::from([("smartquotes_excludes".to_string(), bad.clone())]);
        assert!(
            configs_from_conf(&conf, TransformConfig::default()).is_err(),
            "{bad}"
        );
    }
}

/// `highlight_language` lands on the parse options — CodeBlock reads it
/// while the directive runs (`sphinx/directives/code.py:157-166`) — and
/// leaves the rest alone.
#[test]
fn a_mapped_highlight_language_translates_onto_the_parse_options() {
    let conf = BTreeMap::from([(
        "highlight_language".to_string(),
        serde_json::json!("python"),
    )]);
    let (opts, transforms) = configs_from_conf(&conf, TransformConfig::default()).unwrap();
    assert_eq!(opts.highlight_language, "python");
    assert_eq!(opts.py, PySigConfig::default());
    assert_eq!(transforms, TransformConfig::default());
    let wrong = BTreeMap::from([("highlight_language".to_string(), serde_json::json!(1))]);
    assert!(configs_from_conf(&wrong, TransformConfig::default()).is_err());
}

/// Cases whose oracle tree carries the one attribute the read cannot make
/// yet: `candidates`, which `ImageCollector.process_doc`
/// (`sphinx/environment/collectors/asset.py:48-88`, a `doctree-read`
/// listener at 880) stamps on every `image` — `{'?': uri}` for a remote URI
/// (`:63-64`), with no warning. Image collection is sub-project 2's. A
/// listed case is compared with that attribute dropped from the oracle's
/// `image` lines ([`without_image_candidates`]) — every other attribute,
/// node and character still compared. Strict: a listed case whose oracle
/// carries no `candidates` fails, and one whose tree we stamp ourselves
/// mismatches the stripped oracle.
const IMAGE_CANDIDATES: &[&str] = &["tx_misc.figure_autonumbered_id"];

/// `pseudo_xml` with the ` candidates="…"` attribute dropped from every
/// `image` open tag, or `None` when no `image` line carries one.
fn without_image_candidates(pseudo_xml: &str) -> Option<String> {
    const NEEDLE: &str = " candidates=\"";
    let mut dropped = false;
    let mut out = String::with_capacity(pseudo_xml.len());
    for line in pseudo_xml.lines() {
        let mut line = line.to_string();
        if line.trim_start().starts_with("<image ") {
            if let Some(start) = line.find(NEEDLE) {
                let value = start + NEEDLE.len();
                if let Some(end) = line[value..].find('"') {
                    line.replace_range(start..value + end + 1, "");
                    dropped = true;
                }
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    dropped.then_some(out)
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

    let found_docs = Arc::new(BTreeSet::from(["index".to_string()]));
    let mut unused: BTreeSet<&str> = IMAGE_CANDIDATES.iter().copied().collect();
    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let (opts, transforms) =
            match configs_from_conf(&case.conf, fixture_transform_config(&fixture.settings)) {
                Ok(configs) => configs,
                Err(err) => {
                    mismatches.push(format!("[{}] CONF ERROR: {err}", case.name));
                    continue;
                }
            };
        let mut expected = case.pseudo_xml.clone();
        if IMAGE_CANDIDATES.contains(&case.name.as_str()) {
            if let Some(stripped) = without_image_candidates(&expected) {
                expected = stripped;
                unused.remove(case.name.as_str());
            }
        }
        let rst = case.rst.clone();
        let found_docs = Arc::clone(&found_docs);
        let ours = std::panic::catch_unwind(move || {
            parse_and_transform(
                &rst,
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx: true,
                    docname: "index".into(),
                    found_docs: Some(found_docs),
                    ..opts
                },
                &transforms,
            )
            .0
            .root
            .pformat()
        });
        match ours {
            Err(_) => mismatches.push(format!("[{}] PANICKED on:\n{}", case.name, case.rst)),
            Ok(got) if got != expected => mismatches.push(format!(
                "[{}] MISMATCH\n--- rst ---\n{}\n--- sphinx 9.1.0 ---\n{}\n--- ours ---\n{}",
                case.name, case.rst, expected, got
            )),
            Ok(_) => {}
        }
    }
    assert!(
        unused.is_empty(),
        "IMAGE_CANDIDATES entries whose oracle stamps no `candidates`: {unused:#?}"
    );
    assert!(
        mismatches.is_empty(),
        "{} divergence(s) from the sphinx 9.1.0 oracle:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}

/// Records the oracle prints that the read cannot make, because Sphinx
/// computes them against the build environment rather than from the
/// document alone. `PythonDomain.note_object`'s duplicate-description
/// warning compares the registration with every object the environment
/// holds — other documents' included — so this crate replays the parse's
/// registration records in the merge phase (`env::py_domain::
/// collect_registrations`), and the environment oracle
/// (`tests/env_differential.rs`, projects `py_dup` and — for its place
/// among the parse's other records, by the registration's `seq` —
/// `reporter_interleave`) is its venue; likewise the citation domain's
/// duplicate warning, from a read transform (project `citations`).
/// `(case, record, reason)`; strict: each record must be in the case's
/// oracle output and absent from ours.
const MERGE_TIME_RECORDS: &[(&str, &str, &str)] = &[
    (
        "py.duplicate_functions",
        "<snippet>:3: WARNING: duplicate object description of dup, other instance in index, \
         use :no-index: for one of them",
        "duplicate object descriptions are an environment replay (merge phase)",
    ),
    (
        "py.duplicate_modules",
        "<snippet>:3: WARNING: duplicate object description of dupmod, other instance in \
         index, use :no-index: for one of them",
        "duplicate object descriptions are an environment replay (merge phase)",
    ),
    // `CitationDomain.note_citation` (`sphinx/domains/citation.py:70-82`),
    // called from CitationDefinitionTransform (619), compares the citation
    // with every one the environment holds; the read pass records the call
    // (`RegistryExport::citations`, with the `seq` it spent) and the merge
    // phase replays it (`env::citation_domain`). Venue for the place among
    // the transforms' records and across documents: the environment oracle's
    // `citations` project.
    (
        "tx_footnotes.citation_duplicate",
        "<snippet>:4: WARNING: duplicate citation CIT, other instance in <snippet> \
         [ref.citation]",
        "duplicate citations are an environment replay (merge phase)",
    ),
];

/// The printed form of one parse diagnostic: its source's path — with the
/// `doc2path` suffix a tuple `location=` gets (`Diagnostic::
/// rendered_path`) — through the one renderer every sink uses.
fn printed(d: &sphinx_ultra::rst::diagnostics::Diagnostic, sources: &[String]) -> String {
    let path = d.rendered_path(&sources[d.source as usize]);
    BuildWarning::from_diagnostic(d, PathBuf::from(path)).render()
}

/// The read prints what Sphinx's read phase prints for each snippet, in
/// the same order: reporter records at their creation, interleaved with the
/// directives' and domains' logger records, then the read transforms'
/// records — the whole stream [`parse_and_transform`] returns, so a
/// transform-time record the oracle prints is compared like any other
/// (`tx_filter.info_message_stripped` guards the transform side: the INFO
/// FilterSystemMessages strips never prints). Parsed, like the tree
/// comparison above, against the oracle's project, whose only document is
/// `index` (the toctree resolves against it).
#[test]
fn every_case_warns_what_sphinx_prints() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sphinx_doctree_differential.json"
    ));
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert!(fixture.cases.len() >= 400);
    let found_docs = Arc::new(BTreeSet::from(["index".to_string()]));

    let mut unused: BTreeSet<(&str, &str)> = MERGE_TIME_RECORDS
        .iter()
        .map(|(case, record, _)| (*case, *record))
        .collect();
    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let (opts, transforms) =
            match configs_from_conf(&case.conf, fixture_transform_config(&fixture.settings)) {
                Ok(configs) => configs,
                Err(err) => {
                    mismatches.push(format!("[{}] CONF ERROR: {err}", case.name));
                    continue;
                }
            };
        let mut expected = case.warnings.clone();
        for (name, record, _) in MERGE_TIME_RECORDS {
            if *name == case.name {
                let before = expected.len();
                expected.retain(|w| w != record);
                if expected.len() < before {
                    unused.remove(&(*name, *record));
                }
            }
        }
        let rst = case.rst.clone();
        let found_docs = Arc::clone(&found_docs);
        let ours = std::panic::catch_unwind(move || {
            let (tree, records) = parse_and_transform(
                &rst,
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx: true,
                    docname: "index".into(),
                    found_docs: Some(found_docs),
                    ..opts
                },
                &transforms,
            );
            records
                .iter()
                .map(|d| printed(d, &tree.sources))
                .collect::<Vec<_>>()
        });
        match ours {
            Err(_) => mismatches.push(format!("[{}] PANICKED on:\n{}", case.name, case.rst)),
            Ok(got) if got != expected => mismatches.push(format!(
                "[{}] WARNINGS MISMATCH\n--- rst ---\n{}\n--- sphinx 9.1.0 ---\n{:#?}\n--- ours ---\n{:#?}",
                case.name, case.rst, expected, got
            )),
            Ok(_) => {}
        }
    }
    assert!(
        unused.is_empty(),
        "MERGE_TIME_RECORDS entries the oracle no longer prints: {unused:#?}"
    );
    assert!(
        mismatches.is_empty(),
        "{} warning divergence(s) from the sphinx 9.1.0 oracle:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}

/// The read collects the `env.metadata` Sphinx's MetadataCollector
/// (`sphinx/environment/collectors/metadata.py:35-68`, `doctree-read` at
/// priority 880) reads off a document's docinfo — the values DocInfo (340)
/// left there, `tocdepth` coerced with Python's `int()` (0 on failure),
/// `authors` a list — for every case, an empty map where the oracle
/// recorded none. The collection rides the read's registry
/// ([`sphinx_ultra::rst::RegistryExport::metadata`]), which the merge phase
/// stores as `env.metadata[docname]`.
#[test]
fn every_case_collects_the_metadata_sphinx_collects() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sphinx_doctree_differential.json"
    ));
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    // Anti-truncation floor for the recorded key: a generator that stopped
    // recording it would leave every case comparing two empty maps.
    let recorded = fixture
        .cases
        .iter()
        .filter(|case| !case.metadata.is_empty())
        .count();
    assert!(recorded >= 20, "only {recorded} cases record metadata");

    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let (opts, transforms) =
            match configs_from_conf(&case.conf, fixture_transform_config(&fixture.settings)) {
                Ok(configs) => configs,
                Err(err) => {
                    mismatches.push(format!("[{}] CONF ERROR: {err}", case.name));
                    continue;
                }
            };
        let rst = case.rst.clone();
        let ours = std::panic::catch_unwind(move || {
            parse_and_transform_full(
                &rst,
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx: true,
                    docname: "index".into(),
                    ..opts
                },
                &transforms,
            )
            .registry
            .metadata
            .iter()
            .map(|(name, value)| (name.clone(), value.to_json()))
            .collect::<BTreeMap<String, serde_json::Value>>()
        });
        match ours {
            Err(_) => mismatches.push(format!("[{}] PANICKED on:\n{}", case.name, case.rst)),
            Ok(got) if got != case.metadata => mismatches.push(format!(
                "[{}] METADATA MISMATCH\n--- rst ---\n{}\n--- sphinx 9.1.0 ---\n{:#?}\n--- ours ---\n{:#?}",
                case.name, case.rst, case.metadata, got
            )),
            Ok(_) => {}
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} metadata divergence(s) from the sphinx 9.1.0 oracle:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}
