//! Serde/bincode round-trip test for the doctree IR.
//!
//! Parses every case in both differential fixtures (docutils-mode and
//! sphinx-mode), round-trips the resulting `Doctree` through
//! `doctree::to_bincode`/`from_bincode`, and asserts the restored tree's
//! `pformat()` output is unchanged. Clones the collect-all-mismatches shape
//! from tests/doctree_differential.rs and tests/sphinx_doctree_differential.rs.
//!
//! The escape offsets beside the text nodes (`Node::escapes`) round-trip
//! too, appear only where a backslash was, and make a tree written in the
//! shape before them undecodable — the cache miss the unchanged
//! `DOCTREE_FORMAT_VERSION` relies on.

use sphinx_ultra::doctree::{from_bincode, to_bincode, Attrs, Doctree, Node, Span};
use sphinx_ultra::rst::{parse_rst, ParseOptions};
use sphinx_ultra::transforms::{parse_and_transform, TransformConfig};

#[derive(serde::Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rst: String,
}

const DOCUTILS_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/doctree_differential.json"
));
const SPHINX_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/sphinx_doctree_differential.json"
));

fn cases(raw: &str) -> Vec<Case> {
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert!(!fixture.cases.is_empty(), "fixture has no cases");
    fixture.cases
}

fn opts(sphinx: bool) -> ParseOptions {
    ParseOptions {
        source_path: "<snippet>".into(),
        sphinx,
        docname: "index".into(),
        exclude_patterns: Vec::new(),
        py: Default::default(),
        srcdir: None,
        found_docs: None,
        ..Default::default()
    }
}

/// Every node of `node`'s subtree, preorder.
fn all_nodes(node: &Node) -> Vec<&Node> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        out.push(node);
        stack.extend(node.children.iter().rev());
    }
    out
}

fn round_trip_fixture(raw: &str, sphinx: bool, source: &str) {
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert!(!fixture.cases.is_empty(), "fixture has no cases");

    let mut mismatches = Vec::new();
    for case in &fixture.cases {
        let tree = parse_rst(&case.rst, &opts(sphinx));
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
    round_trip_fixture(DOCUTILS_FIXTURE, false, "doctree_differential");
}

#[test]
fn sphinx_doctree_differential_round_trips_through_bincode() {
    round_trip_fixture(SPHINX_FIXTURE, true, "sphinx_doctree_differential");
}

/// The escape offsets beside each text node come back from bincode as
/// they went in, in both modes and through the read transforms.
#[test]
fn escapes_roundtrip_through_bincode() {
    let source = "T\\*i\\ tle\n=========\n\n\\\"a\\\" *b\\--c* `d\\...`_ |e\\'| x\\\ny\n\n\
                  .. _d...: https://example.org/\n.. |e'| replace:: f\\-g\n\n\
                  term\\ : cls \\: x\n   def\n";
    for sphinx in [false, true] {
        let trees = [
            parse_rst(source, &opts(sphinx)),
            parse_and_transform(source, &opts(sphinx), &TransformConfig::default()).0,
        ];
        for tree in trees {
            let escaped = all_nodes(&tree.root)
                .into_iter()
                .filter(|node| !node.escapes.is_empty())
                .count();
            assert!(
                escaped >= 5,
                "sphinx={sphinx}: only {escaped} escaped text nodes"
            );
            let restored = from_bincode(&to_bincode(&tree)).expect("decodes");
            assert_eq!(restored, tree, "sphinx={sphinx}");
        }
    }
}

/// Only the inline parser writes escapes: a document without a backslash
/// has none anywhere, and an element never has any.
#[test]
fn an_escape_free_document_has_no_escape_offsets() {
    for (raw, sphinx) in [(DOCUTILS_FIXTURE, false), (SPHINX_FIXTURE, true)] {
        for case in cases(raw) {
            let tree = parse_rst(&case.rst, &opts(sphinx));
            let escape_free = !case.rst.contains('\\');
            for node in all_nodes(&tree.root) {
                assert!(
                    node.escapes.is_empty() || (!escape_free && node.text.is_some()),
                    "{}: {} carries {:?}",
                    case.name,
                    node.kind,
                    node.escapes
                );
            }
        }
    }
}

/// The node shape before `escapes`, field for field.
#[derive(serde::Serialize)]
struct ShapeWithoutEscapes<'n> {
    kind: &'n str,
    span: Span,
    text: &'n Option<String>,
    attrs: &'n Attrs,
    children: Vec<ShapeWithoutEscapes<'n>>,
}

fn without_escapes(node: &Node) -> ShapeWithoutEscapes<'_> {
    ShapeWithoutEscapes {
        kind: node.kind,
        span: node.span,
        text: &node.text,
        attrs: &node.attrs,
        children: node.children.iter().map(without_escapes).collect(),
    }
}

/// A doctree a build wrote before `escapes` existed carries the same
/// version word (`DOCTREE_FORMAT_VERSION` stays 3), so it must fail to
/// decode — a cache miss — and never come back as some other tree. Every
/// fixture case, written in the old shape, fails.
#[test]
fn a_doctree_in_the_shape_before_escapes_fails_to_decode() {
    #[derive(serde::Serialize)]
    struct DoctreeWithoutEscapes<'n> {
        root: ShapeWithoutEscapes<'n>,
        sources: &'n Vec<String>,
    }
    for (raw, sphinx) in [(DOCUTILS_FIXTURE, false), (SPHINX_FIXTURE, true)] {
        for case in cases(raw) {
            let tree: Doctree =
                parse_and_transform(&case.rst, &opts(sphinx), &TransformConfig::default()).0;
            let old = DoctreeWithoutEscapes {
                root: without_escapes(&tree.root),
                sources: &tree.sources,
            };
            let bytes = bincode::serde::encode_to_vec(&old, bincode::config::standard()).unwrap();
            assert!(from_bincode(&bytes).is_err(), "{} decoded", case.name);
        }
    }
}
