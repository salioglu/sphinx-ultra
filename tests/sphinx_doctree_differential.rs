//! Differential test: our RST parser vs the SPHINX ORACLE — the pseudo-XML a
//! real `sphinx-build` 9.1.0 read phase (dummy builder, `extensions = []`,
//! smartquotes off, keep_warnings on) produces for the committed fixture corpus.
//!
//! Regenerate the fixture (manual, never in CI):
//!     uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
//!         python tools/gen_sphinx_fixture.py
//!
//! Clones the tests/doctree_differential.rs shape: committed JSON, version
//! assertions (BOTH sphinx and docutils are recorded), floor guard against
//! silent truncation, collect ALL mismatches before asserting, and panics
//! surface as named mismatches, not test aborts. The fixture's source paths
//! are normalized to the "<snippet>" token; ParseOptions.source_path below
//! must use the same token.

use sphinx_ultra::rst::{parse_rst, ParseOptions};

#[derive(serde::Deserialize)]
struct Fixture {
    docutils_version: String,
    sphinx_version: String,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
    pseudo_xml: String,
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
    assert!(
        fixture.cases.len() >= 40,
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
                    sphinx: true,
                    docname: "index".into(),
                },
            )
            .root
            .pformat()
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
