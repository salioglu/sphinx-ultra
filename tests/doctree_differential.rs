//! Differential test: our RST block parser vs docutils 0.22.4 parse-layer
//! pseudo-XML, over the committed fixture corpus.
//!
//! Regenerate the fixture (manual, never in CI):
//!     PYTHONNOUSERSITE=1 uv run --python 3.12 --with docutils==0.22.4 \
//!         python tools/gen_doctree_fixture.py
//!
//! Clones the tests/pattern_differential.rs shape: committed JSON, floor
//! guard against silent truncation, collect ALL mismatches before
//! asserting, and panics surface as named mismatches, not test aborts.
//!
//! Two comparisons per case: the tree (`pseudo_xml`) and the reporter
//! stream (`stream`) — the level >= 2 messages docutils wrote when it
//! created them, in creation order, which is what Sphinx prints.

use sphinx_ultra::rst::diagnostics::{Diagnostic, DiagnosticChannel};
use sphinx_ultra::rst::{parse_rst, parse_rst_full, ParseOptions};

#[derive(serde::Deserialize)]
struct Fixture {
    docutils_version: String,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
    pseudo_xml: String,
    /// `{line}: ({TYPE}/{N}) {message}` per reporter write, in write order
    /// (see the generator's docstring).
    stream: Vec<String>,
}

fn load_fixture() -> Fixture {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/doctree_differential.json"
    ));
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert_eq!(fixture.docutils_version, "0.22.4");
    assert!(
        fixture.cases.len() >= 200,
        "fixture truncated? only {} cases",
        fixture.cases.len()
    );
    fixture
}

fn docutils_options() -> ParseOptions {
    ParseOptions {
        source_path: "<snippet>".into(),
        sphinx: false,
        docname: "index".into(),
        exclude_patterns: Vec::new(),
        py: Default::default(),
        srcdir: None,
        found_docs: None,
        ..Default::default()
    }
}

/// A recorded diagnostic in the fixture's `stream` form. The fixture holds
/// reporter writes only, so a logger record — which a docutils-mode parse
/// must never make — renders with a marker that can match nothing there.
fn stream_record(d: &Diagnostic) -> String {
    let kind = match d.level {
        2 => "WARNING",
        3 => "ERROR",
        4 => "SEVERE",
        _ => "UNPRINTABLE",
    };
    let line = d.line.map(|line| line.to_string()).unwrap_or_default();
    let marker = match d.channel {
        DiagnosticChannel::Reporter => "",
        DiagnosticChannel::Logger => "[logger] ",
    };
    format!("{marker}{line}: ({kind}/{}) {}", d.level, d.text)
}

#[test]
fn matches_docutils_parser_pformat() {
    let fixture = load_fixture();

    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let rst = case.rst.clone();
        let ours =
            std::panic::catch_unwind(move || parse_rst(&rst, &docutils_options()).root.pformat());
        match ours {
            Err(_) => mismatches.push(format!("[{}] PANICKED on:\n{}", case.name, case.rst)),
            Ok(got) if got != case.pseudo_xml => mismatches.push(format!(
                "[{}] MISMATCH\n--- rst ---\n{}\n--- docutils ---\n{}\n--- ours ---\n{}",
                case.name, case.rst, case.pseudo_xml, got
            )),
            Ok(_) => {}
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} divergence(s) from docutils 0.22.4:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}

/// The parser records every level >= 2 message when it creates it — the
/// moment docutils' Reporter writes it to the warning stream
/// (`docutils/utils/__init__.py:213-215`) — so the recorded diagnostics
/// equal docutils' stream: same messages, same order, same text at
/// creation. That text is where the stream and the tree part ways: a
/// directive's `DirectiveError` message gets its literal block only after
/// it was written (`docutils/parsers/rst/states.py:2286-2290`), and a
/// message created in a nested parse whose nodes are discarded is written
/// but never attached.
#[test]
fn every_case_streams_what_docutils_writes() {
    let fixture = load_fixture();
    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let rst = case.rst.clone();
        let ours = std::panic::catch_unwind(move || {
            parse_rst_full(&rst, &docutils_options())
                .registry
                .diagnostics
                .iter()
                .map(stream_record)
                .collect::<Vec<_>>()
        });
        match ours {
            Err(_) => mismatches.push(format!("[{}] PANICKED on:\n{}", case.name, case.rst)),
            Ok(got) if got != case.stream => mismatches.push(format!(
                "[{}] STREAM MISMATCH\n--- rst ---\n{}\n--- docutils ---\n{:#?}\n--- ours ---\n{:#?}",
                case.name, case.rst, case.stream, got
            )),
            Ok(_) => {}
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} stream divergence(s) from docutils 0.22.4:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}
