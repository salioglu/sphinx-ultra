//! `from_bincode` must not leak unbounded process memory when decoding a
//! blob that didn't come from this crate's own encoder (a corrupted file,
//! or a stale/foreign blob landing in the doctree cache). The interner
//! behind `Node::kind`/`Attrs::extra` keys (src/doctree/intern.rs) caps
//! itself at a generous constant and errors instead of leaking further once
//! that cap is exceeded; this proves it through the public API, without
//! needing access to the (crate-internal) interner itself.

use sphinx_ultra::doctree::{from_bincode, to_bincode, Doctree, Node, Span};

#[test]
fn from_bincode_errors_instead_of_leaking_unbounded_on_many_distinct_kinds() {
    // src/doctree/intern.rs's MAX_INTERNED is 4096. A doctree this crate
    // itself would ever produce uses at most a few hundred distinct kinds
    // (kinds.rs) and attribute keys combined, so generating well over 4096
    // distinct *fake* kinds here guarantees the cap trips regardless of
    // whatever else this process has already interned.
    let mut root = Node::elem("document", Span::ZERO);
    for i in 0..5000 {
        // Test-local leak, unrelated to the crate's own interner: encoding
        // via `Serialize` just needs valid `&'static str` bytes, not a
        // pointer that came from `intern`.
        let kind: &'static str = Box::leak(format!("fake-kind-{i}").into_boxed_str());
        root.children.push(Node::elem(kind, Span::ZERO));
    }
    let tree = Doctree {
        root,
        sources: vec!["<test>".to_string()],
    };
    let bytes = to_bincode(&tree);

    let result = from_bincode(&bytes);

    assert!(
        result.is_err(),
        "decoding 5000 distinct node kinds should trip the interner cap \
         and return an error, not leak unbounded process memory"
    );
}
