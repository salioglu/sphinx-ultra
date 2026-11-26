//! Differential test: the document state our read leaves behind vs Sphinx
//! 9.1.0's, for the committed read-state corpus
//! (`tests/fixtures/read_state_differential.json`; research report "read
//! document state" §11).
//!
//! Regenerate the fixture (manual, never in CI), with the locked core
//! oracle profile:
//!     PYTHONNOUSERSITE=1 uv run --locked --python 3.12 \
//!         --project tools/oracle_profiles/core python tools/gen_read_state_fixture.py
//!
//! Each case is one `index.rst` snippet read standalone, and five keys of
//! the read are compared separately:
//!
//! - `pseudo_xml`: the post-read doctree ([`read_standalone`]);
//! - `warnings`: the read's warning stream, one entry per line;
//! - `id_trace`: every id the read allocated, `[phase, kind, id, attached]`
//!   (the parse's [`IdAllocator`](sphinx_ultra::transforms::state::IdAllocator)
//!   trace, with `attached` filled in by the read). The fixture's whole
//!   trace is compared: ours holds the parse's events and those of the
//!   transforms ported so far;
//! - `registries_post_parse`: docutils' node registries and name table as
//!   the parse leaves them
//!   ([`registries_dump`](sphinx_ultra::transforms::state::DocumentState::registries_dump)
//!   of [`parse_rst_full`]'s state);
//! - `final_position`: where docutils locates a message with neither a node
//!   nor a line, `[source, line]`.
//!
//! The mismatching `(case, key)` pairs must equal [`KNOWN_READ_STATE_GAPS`]
//! exactly; [`read_state_census`] prints the rows.

mod support {
    pub mod read_gaps;
}

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use sphinx_ultra::builder::BuilderKind;
use sphinx_ultra::config::BuildConfig;
use sphinx_ultra::read::{read_standalone, ReadSettings};
use sphinx_ultra::rst::{parse_rst_full, ParseOptions};
use sphinx_ultra::transforms::state::SourceRef;
use support::read_gaps::{self, Census, GapFix};

#[derive(serde::Deserialize)]
struct Fixture {
    docutils_version: String,
    sphinx_version: String,
    settings: FixtureSettings,
    cases: Vec<Case>,
}

/// The read settings the generator recorded in the fixture header: the ones
/// the read consumes.
#[derive(serde::Deserialize)]
struct FixtureSettings {
    keep_warnings: bool,
    smartquotes: bool,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
    pseudo_xml: Value,
    warnings: Value,
    id_trace: Value,
    registries_post_parse: Value,
    final_position: Value,
}

impl Case {
    /// The fixture's value of `key`.
    fn expected(&self, key: &str) -> &Value {
        match key {
            "pseudo_xml" => &self.pseudo_xml,
            "warnings" => &self.warnings,
            "id_trace" => &self.id_trace,
            "registries_post_parse" => &self.registries_post_parse,
            "final_position" => &self.final_position,
            other => panic!("no key {other:?}"),
        }
    }
}

/// The compared keys, in the fixture's order of interest.
const KEYS: [&str; 5] = [
    "pseudo_xml",
    "warnings",
    "id_trace",
    "registries_post_parse",
    "final_position",
];

fn load_fixture() -> Fixture {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/read_state_differential.json"
    ));
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert_eq!(fixture.docutils_version, "0.22.4");
    assert_eq!(fixture.sphinx_version, "9.1.0");
    assert!(
        fixture.cases.len() >= 80,
        "fixture truncated? only {} cases",
        fixture.cases.len()
    );
    fixture
}

/// The read settings the fixture's header gives through a [`BuildConfig`]
/// (M1's validation off, decision 7) for the dummy builder the oracle
/// builds with, with the id trace on.
fn read_settings_of(settings: &FixtureSettings) -> ReadSettings {
    ReadSettings {
        trace_ids: true,
        ..ReadSettings::from_config(
            &BuildConfig {
                validate_directives: false,
                keep_warnings: settings.keep_warnings,
                smartquotes: settings.smartquotes,
                ..BuildConfig::default()
            },
            BuilderKind::Dummy,
        )
    }
}

/// Our value of every key for `rst`, read as the oracle harness reads a
/// snippet (`<SRCDIR>/index.rst`, docname `index`, no project).
fn ours(rst: &str, settings: &ReadSettings) -> BTreeMap<&'static str, Value> {
    let parse = ParseOptions {
        source_path: "<SRCDIR>/index.rst".to_string(),
        sphinx: true,
        docname: "index".to_string(),
        ..ParseOptions::default()
    };
    let read = read_standalone(rst, &parse, settings);
    let doctree = read.outcome.doctree();
    let state = read.outcome.state();

    // Sphinx's stream is each record's text and a line break.
    let stream: String = read
        .diagnostics
        .iter()
        .map(|diagnostic| format!("{}\n", diagnostic.render(true)))
        .collect();
    let warnings: Vec<&str> = match stream.strip_suffix('\n') {
        Some(text) => text.split('\n').collect(),
        None => Vec::new(),
    };
    let trace: Vec<Value> = state
        .ids
        .trace()
        .iter()
        .map(|event| json!([event.phase, event.kind, event.id, event.attached]))
        .collect();
    let position = state.final_position;
    let source = position.source.map(|SourceRef::Table(id)| {
        doctree
            .sources
            .get(usize::from(id))
            .cloned()
            .unwrap_or_default()
    });
    let parsed = parse_rst_full(rst, &parse);

    BTreeMap::from([
        ("pseudo_xml", Value::String(doctree.root.pformat())),
        ("warnings", json!(warnings)),
        ("id_trace", Value::Array(trace)),
        (
            "registries_post_parse",
            parsed.state.registries_dump(&parsed.doctree.root),
        ),
        ("final_position", json!([source, position.line])),
    ])
}

/// Every mismatching `(case, key)`, with the fixture's value and ours.
fn mismatches(fixture: &Fixture) -> BTreeMap<(String, &'static str), (Value, Value)> {
    let settings = read_settings_of(&fixture.settings);
    let mut found = BTreeMap::new();
    for case in &fixture.cases {
        let rst = case.rst.clone();
        let settings = settings.clone();
        let ours = match std::panic::catch_unwind(move || ours(&rst, &settings)) {
            Ok(ours) => ours,
            Err(_) => KEYS
                .iter()
                .map(|key| (*key, Value::String("PANICKED".to_string())))
                .collect(),
        };
        for key in KEYS {
            let expected = case.expected(key);
            if &ours[key] != expected {
                found.insert(
                    (case.name.clone(), key),
                    (expected.clone(), ours[key].clone()),
                );
            }
        }
    }
    found
}

/// The `(case, key)` pairs whose read state is not Sphinx's yet, each with
/// the fix families its divergence shows and the latest task that fixes
/// one of them. Checked strictly: a listed pair that matches fails the
/// test, and so does an unlisted pair that differs. Rows are printed by
/// [`read_state_census`], one per line as the census prints them, so a task
/// deletes or re-attributes a row by editing one line and a grep for an
/// owner shows whole rows.
#[rustfmt::skip]
const KNOWN_READ_STATE_GAPS: &[(&str, &str, &str)] = &[];

/// Holds every key of every read-state case to the oracle, strictly, against
/// [`KNOWN_READ_STATE_GAPS`].
#[test]
fn read_state_differential() {
    let fixture = load_fixture();
    let names: BTreeSet<&str> = fixture
        .cases
        .iter()
        .map(|case| case.name.as_str())
        .collect();
    assert_eq!(names.len(), fixture.cases.len(), "a case name repeats");

    let found = mismatches(&fixture);
    let known: BTreeSet<(&str, &str)> = KNOWN_READ_STATE_GAPS
        .iter()
        .map(|(case, key, _)| (*case, *key))
        .collect();
    assert_eq!(
        known.len(),
        KNOWN_READ_STATE_GAPS.len(),
        "KNOWN_READ_STATE_GAPS lists a (case, key) twice"
    );
    for (case, key) in &known {
        assert!(names.contains(case), "no case {case:?} in the fixture");
        assert!(KEYS.contains(key), "no key {key:?}");
    }
    let observed: BTreeSet<(&str, &str)> = found
        .keys()
        .map(|(case, key)| (case.as_str(), *key))
        .collect();
    let regressions: Vec<String> = observed
        .difference(&known)
        .map(|(case, key)| {
            let (expected, actual) = &found[&(case.to_string(), *key)];
            format!(
                "    ({case:?}, {key:?})\n        expected: {expected}\n        actual:   {actual}"
            )
        })
        .collect();
    let fixed: Vec<&(&str, &str)> = known.difference(&observed).collect();
    assert!(
        regressions.is_empty() && fixed.is_empty(),
        "the read-state mismatch set differs from KNOWN_READ_STATE_GAPS: {} regressions, {} \
         fixed (delete their rows: {fixed:?})\n{}",
        regressions.len(),
        fixed.len(),
        regressions.join("\n")
    );
}

/// Prints the current read-state mismatch set as `KNOWN_READ_STATE_GAPS`
/// rows. A row's reason lists the fix families its divergence shows and the
/// latest task that fixes one of them ([`support::read_gaps`]); a
/// divergence no family explains gets a `?` family and a comment above its
/// row, and fails the census once every row is printed:
///
/// `cargo test --locked --test read_state_differential read_state_census -- --ignored --nocapture`
#[test]
#[ignore]
fn read_state_census() {
    let fixture = load_fixture();
    let found = mismatches(&fixture);
    let mut table = String::new();
    let mut unexplained_rows = 0;
    for ((case, key), (expected, actual)) in &found {
        // The case's tree divergence: what an id whose node left the tree,
        // or a record about copied content, depends on.
        let tree = found
            .get(&(case.clone(), "pseudo_xml"))
            .map(|(expected, actual)| {
                read_gaps::tree_census(
                    expected.as_str().unwrap_or_default(),
                    actual.as_str().unwrap_or_default(),
                )
            });
        let census = match *key {
            "pseudo_xml" => tree.unwrap_or_default(),
            "warnings" => warning_census(expected, actual),
            "id_trace" => trace_census(expected, actual, tree.as_ref()),
            "registries_post_parse" => registries_census(expected, actual),
            _ => Census {
                unexplained: vec![format!("{key}: expected {expected}, actual {actual}")],
                ..Census::default()
            },
        };
        if !census.unexplained.is_empty() {
            unexplained_rows += 1;
        }
        for note in &census.unexplained {
            for line in note.lines() {
                table.push_str(&format!("    // ? {line}\n"));
            }
        }
        table.push_str(&format!(
            "    ({case:?}, {key:?}, {:?}),\n",
            census.reason()
        ));
    }
    println!("    // {} rows\n{table}", found.len());
    assert_eq!(
        unexplained_rows, 0,
        "rows with a `?` family need a family the census does not know"
    );
}

/// The records of a warning stream given as its lines: a record starts at
/// a located line or a bare `WARNING:`/`ERROR:`/`CRITICAL:` one, and the
/// lines after it are its continuation.
fn records(lines: &Value) -> Vec<String> {
    let mut records: Vec<String> = Vec::new();
    for line in lines.as_array().into_iter().flatten() {
        let line = line.as_str().unwrap_or_default();
        let starts_record = line.starts_with("<SRCDIR>")
            || line.starts_with("internal padding after ")
            || ["WARNING: ", "ERROR: ", "CRITICAL: "]
                .iter()
                .any(|prefix| line.starts_with(prefix));
        match records.last_mut() {
            Some(record) if !starts_record => {
                record.push('\n');
                record.push_str(line);
            }
            _ => records.push(line.to_string()),
        }
    }
    records
}

/// Records only Sphinx prints, by message, with the transform or collector
/// that raises them [TW §6.3]. Substitutions' (220), Footnotes' (620) and
/// the unreferenced-footnote warning (622) print since T11, and the image
/// collector's (880) since T13a.
const SPHINX_ONLY_RECORDS: &[(&str, GapFix)] = &[
    ("Unknown target name: ", read_gaps::DANGLING_REFS),
    ("Anonymous hyperlink mismatch: ", read_gaps::DANGLING_REFS),
    ("Indirect hyperlink target ", read_gaps::INDIRECT_TARGETS),
];

/// The families of a `warnings` divergence, record by record.
fn warning_census(expected: &Value, actual: &Value) -> Census {
    let expected = records(expected);
    let actual = records(actual);
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    let actual: Vec<&str> = actual.iter().map(String::as_str).collect();
    let mut census = Census::default();
    for hunk in read_gaps::line_hunks(&expected, &actual) {
        for record in hunk.removed {
            match SPHINX_ONLY_RECORDS
                .iter()
                .find(|(message, _)| record.contains(message))
            {
                Some((_, fix)) => census.add(*fix),
                None => census
                    .unexplained
                    .push(format!("a record only Sphinx prints: {record:?}")),
            }
        }
        for record in hunk.added {
            census
                .unexplained
                .push(format!("a record only we print: {record:?}"));
        }
    }
    census
}

/// Substitutions' (220) and Footnotes' (620) families, ported in T11,
/// AutoNumbering's (210), ported in T12, and the pending transforms' that
/// allocate ids, TargetNotes (540) and Contents (720), ported in T18b.
/// Like T10's (`dangling refs`, `indirect targets`), they stay for the ids
/// those transforms allocate, which later tasks' rows can shift.
const SUBSTITUTIONS: GapFix = read_gaps::gap_fix("substitutions", "5a-T11");
const FOOTNOTES: GapFix = read_gaps::gap_fix("footnotes", "5a-T11");
const AUTONUMBER_IDS: GapFix = read_gaps::gap_fix("autonumber ids", "5a-T12");
const PENDING: GapFix = read_gaps::gap_fix("pending", "5a-T18b");

/// The transforms that allocate ids in the corpus, by `python_name`, with
/// the task that ports each [DS §5.2].
const ALLOCATING_TRANSFORMS: &[(&str, GapFix)] = &[
    (
        "docutils.transforms.references.Substitutions",
        SUBSTITUTIONS,
    ),
    (
        "docutils.transforms.references.AnonymousHyperlinks",
        read_gaps::DANGLING_REFS,
    ),
    (
        "docutils.transforms.references.IndirectHyperlinks",
        read_gaps::INDIRECT_TARGETS,
    ),
    (
        "sphinx.transforms.references.SphinxDanglingReferences",
        read_gaps::DANGLING_REFS,
    ),
    ("docutils.transforms.references.Footnotes", FOOTNOTES),
    ("docutils.transforms.references.TargetNotes", PENDING),
    ("docutils.transforms.parts.Contents", PENDING),
    ("sphinx.transforms.AutoNumbering", AUTONUMBER_IDS),
];

/// The families of an `id_trace` divergence. Each transform-phase event is
/// owned by the task that ports its transform. Parse events must match in
/// order; one whose node left the tree (`attached` differs) is explained by
/// the case's tree divergence, `tree`.
fn trace_census(expected: &Value, actual: &Value, tree: Option<&Census>) -> Census {
    let events = |trace: &Value| -> Vec<(String, String, String, bool)> {
        trace
            .as_array()
            .into_iter()
            .flatten()
            .map(|event| {
                (
                    event[0].as_str().unwrap_or_default().to_string(),
                    event[1].as_str().unwrap_or_default().to_string(),
                    event[2].as_str().unwrap_or_default().to_string(),
                    event[3].as_bool().unwrap_or_default(),
                )
            })
            .collect()
    };
    let expected = events(expected);
    let actual = events(actual);
    let mut census = Census::default();
    for (phase, kind, id, _) in expected.iter().filter(|event| event.0 != "parse") {
        match ALLOCATING_TRANSFORMS.iter().find(|(name, _)| name == phase) {
            Some((_, fix)) => census.add(*fix),
            None => census
                .unexplained
                .push(format!("{phase} allocates {id} for a {kind}")),
        }
    }
    let parse_events = |events: &[(String, String, String, bool)]| -> Vec<(String, String, bool)> {
        events
            .iter()
            .filter(|event| event.0 == "parse")
            .map(|(_, kind, id, attached)| (kind.clone(), id.clone(), *attached))
            .collect()
    };
    let without_attachment = |events: &[(String, String, bool)]| -> Vec<(String, String)> {
        events
            .iter()
            .map(|(kind, id, _)| (kind.clone(), id.clone()))
            .collect()
    };
    let expected_events = parse_events(&expected);
    let actual_events = parse_events(&actual);
    let expected_parse = without_attachment(&expected_events);
    let actual_parse = without_attachment(&actual_events);
    if expected_parse != actual_parse {
        census.unexplained.push(format!(
            "parse events differ: expected {expected_parse:?}, actual {actual_parse:?}"
        ));
    } else {
        let flipped = expected_events
            .iter()
            .zip(&actual_events)
            .any(|(expected, actual)| expected.2 != actual.2);
        if flipped {
            match tree {
                Some(tree) => {
                    for fix in &tree.fixes {
                        census.add(*fix);
                    }
                }
                None => census
                    .unexplained
                    .push("a parse event's node left a tree that matches".to_string()),
            }
        }
    }
    census
}

/// The families of a `registries_post_parse` divergence. The parse
/// registers what docutils' does since the structure directives' parse
/// halves (T18a), so any divergence is unexplained.
fn registries_census(expected: &Value, actual: &Value) -> Census {
    let mut census = Census::default();
    let keys: BTreeSet<&String> = expected
        .as_object()
        .into_iter()
        .chain(actual.as_object())
        .flat_map(|registries| registries.keys())
        .collect();
    for key in keys {
        if expected[key] != actual[key] {
            census.unexplained.push(format!(
                "{key}: expected {}, actual {}",
                expected[key], actual[key]
            ));
        }
    }
    census
}
