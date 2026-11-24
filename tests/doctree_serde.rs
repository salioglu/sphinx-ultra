//! Serde/bincode round-trip test for the doctree IR.
//!
//! Parses every case in both differential fixtures (docutils-mode and
//! sphinx-mode), round-trips the resulting `Doctree` through
//! `doctree::to_bincode`/`from_bincode`, and asserts the restored tree's
//! `pformat()` output is unchanged and the restored tree equals the parsed
//! one (which covers the persisted half of each node's side-car). Clones
//! the collect-all-mismatches shape from tests/doctree_differential.rs and
//! tests/sphinx_doctree_differential.rs.

use std::num::NonZeroU32;

use sphinx_ultra::doctree::{from_bincode, to_bincode, Uid};
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
                py: Default::default(),
                srcdir: None,
                found_docs: None,
                ..Default::default()
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
                } else if restored != tree {
                    mismatches.push(format!(
                        "[{source}/{}] the restored doctree differs outside pformat \
                         (spans, sources or a side-car's rawsource/pending)",
                        case.name
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

/// A node's side-car persists its tree data — a literal block's
/// `rawsource`, a `pending` node's transform and details — and drops its
/// read-stage data: the `uid` stamped here through `Uid::new`, and the
/// docutils `raw` data of an escaped Text. Equality never looked at the
/// dropped half, so the restored tree still equals the original.
#[test]
fn side_car_round_trip_keeps_rawsource_and_pending_drops_uid_and_raw() {
    let mut tree = parse_rst(
        "::\n\n  code\n\n.. class:: x\n\nEscaped \\*star\\*.\n",
        &ParseOptions {
            source_path: "<snippet>".into(),
            ..Default::default()
        },
    );
    let uid = Uid::new(NonZeroU32::new(1).expect("a non-zero uid"));
    tree.root.children[0].set_uid(uid);

    let [literal, pending, paragraph] = &tree.root.children[..] else {
        panic!(
            "expected literal_block, pending, paragraph: {}",
            tree.root.pformat()
        );
    };
    assert_eq!(literal.uid(), Some(uid));
    assert_eq!(literal.rawsource(), Some("code"));
    let pending_data = pending.ext.as_deref().and_then(|ext| ext.pending.clone());
    assert!(
        pending_data.is_some(),
        "the pending node carries its Pending"
    );
    assert_eq!(
        paragraph.children[0].text_data(),
        "Escaped \u{0}*star\u{0}*."
    );

    let restored = from_bincode(&to_bincode(&tree)).expect("the round trip decodes");
    let [literal, pending, paragraph] = &restored.root.children[..] else {
        panic!("restored shape changed: {}", restored.root.pformat());
    };
    assert_eq!(literal.rawsource(), Some("code"), "rawsource is persisted");
    assert_eq!(
        pending.ext.as_deref().and_then(|ext| ext.pending.clone()),
        pending_data,
        "pending is persisted"
    );
    assert_eq!(literal.uid(), None, "uid is read-stage only");
    assert_eq!(
        paragraph.children[0].text_data(),
        "Escaped *star*.",
        "raw is read-stage only"
    );
    assert!(
        paragraph.children[0].ext.is_none(),
        "a side-car with nothing persisted is written as none"
    );
    assert_eq!(restored, tree, "equality ignores the dropped uid and raw");
    assert_eq!(restored.root.pformat(), tree.root.pformat());
}
