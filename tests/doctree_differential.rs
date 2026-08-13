//! Differential test: our RST block parser vs docutils 0.22.4 parse-layer
//! pseudo-XML, over the committed fixture corpus.
//!
//! Regenerate the fixture (manual, never in CI):
//!     uv run --python 3.12 --with docutils==0.22.4 python tools/gen_doctree_fixture.py
//!
//! Clones the tests/pattern_differential.rs shape: committed JSON, floor
//! guard against silent truncation, collect ALL mismatches before
//! asserting, and panics surface as named mismatches, not test aborts.

use sphinx_ultra::rst::{parse_rst, ParseOptions};

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
}

#[test]
fn matches_docutils_parser_pformat() {
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

    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let rst = case.rst.clone();
        let ours = std::panic::catch_unwind(move || {
            parse_rst(
                &rst,
                &ParseOptions {
                    source_path: "<snippet>".into(),
                    sphinx: false,
                },
            )
            .root
            .pformat()
        });
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
