//! Serde/bincode round-trip test for the doctree IR.
//!
//! Parses every case in both differential fixtures (docutils-mode and
//! sphinx-mode), round-trips the resulting `Doctree` through
//! `doctree::to_bincode`/`from_bincode`, and asserts the restored tree's
//! `pformat()` output is unchanged. Clones the collect-all-mismatches shape
//! from tests/doctree_differential.rs and tests/sphinx_doctree_differential.rs.

use sphinx_ultra::doctree::{from_bincode, to_bincode};
use sphinx_ultra::rst::{parse_rst, ParseOptions};

#[derive(serde::Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
}

fn round_trip_fixture(raw: &str, sphinx: bool, source: &str) {
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert!(!fixture.cases.is_empty(), "fixture has no cases");

    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let tree = parse_rst(
            &case.rst,
            &ParseOptions {
                source_path: "<snippet>".into(),
                sphinx,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                found_docs: None,
            },
        );
        let original_pformat = tree.root.pformat();
        let bytes = to_bincode(&tree);
        match from_bincode(&bytes) {
            Err(e) => mismatches.push(format!("[{source}/{}] from_bincode failed: {e}", case.name)),
            Ok(restored) => {
                let restored_pformat = restored.root.pformat();
                if restored_pformat != original_pformat {
                    mismatches.push(format!(
                        "[{source}/{}] pformat MISMATCH after round-trip\n--- before ---\n{}\n--- after ---\n{}",
                        case.name, original_pformat, restored_pformat
                    ));
                }
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} round-trip divergence(s) in {source}:\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}

#[test]
fn doctree_differential_round_trips_through_bincode() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/doctree_differential.json"
    ));
    round_trip_fixture(raw, false, "doctree_differential");
}

#[test]
fn sphinx_doctree_differential_round_trips_through_bincode() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sphinx_doctree_differential.json"
    ));
    round_trip_fixture(raw, true, "sphinx_doctree_differential");
}
