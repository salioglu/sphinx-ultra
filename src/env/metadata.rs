//! `env.metadata[docname]` — the document's bibliographic field list, read
//! as build metadata.
//!
//! Port of `MetadataCollector.process_doc`
//! (`sphinx/environment/collectors/metadata.py:34`). Sphinx reads a
//! `docinfo` node, which docutils' `DocInfo` transform
//! (`docutils/transforms/frontmatter.py`) builds from a leading field list;
//! this crate has no such transform yet, so the port reads that leading
//! `field_list` directly. The two agree on what a field contributes —
//! `field_name.astext()` -> `field_body.astext()` — which is all the
//! consumers need (`orphan`, `nocomments`, `tocdepth`).
//!
//! Deliberate omissions:
//!
//! * Sphinx pops the `docinfo` node out of the doctree; this leaves the
//!   `field_list` in place, because removing it is a doctree-shape change
//!   that belongs with the `DocInfo` transform itself.
//! * The bibliographic special cases (`authors`, and the `tocdepth`
//!   int-coercion) are not applied: nothing reads them yet.
//!
//! **Known gap — a field list below a promoted document title is not seen.**
//! docutils runs `DocTitle` before `DocInfo`, so in a single-section
//! document the section title is promoted to a document `title` and the
//! field list that follows it still becomes `docinfo`. This crate applies no
//! transforms at parse time (`crate::rst`, module header: "Transforms
//! (doctitle promotion, ...) are explicitly NOT applied here"), so such a
//! document keeps its `section` wrapper and the field list never sits at
//! document level for this port to find. Consequence: a document whose
//! `:orphan:` marker follows its title is not exempted from the
//! `document isn't included in any toctree` warning. `title`/`subtitle` are
//! already in [`is_pre_bibliographic`], so this closes by itself the moment
//! doctitle promotion lands — nothing here needs to change.

use std::collections::BTreeMap;

use crate::doctree::{kinds, Doctree, Node};

/// docutils' `PreBibliographic` node classes — what
/// `first_child_not_matching_class(nodes.PreBibliographic)` skips over
/// before deciding whether the document opens with bibliographic fields.
///
/// The complete set, enumerated from docutils 0.22.4 by walking
/// `nodes.PreBibliographic`'s element subclasses: the four `Invisible` ones
/// (`comment`, `substitution_definition`, `target`, `pending`,
/// `nodes.py:2477-2522`) plus `title`, `subtitle`, `meta`, `decoration`,
/// `system_message` and `raw` (`nodes.py:1561-2600`).
///
/// `raw` is the one of these a document can hit today — `.. raw:: html`
/// ahead of the field list. The rest are transform output this crate does
/// not produce yet (see the module header), but skipping them is correct
/// and free.
fn is_pre_bibliographic(node: &Node) -> bool {
    matches!(
        node.kind,
        kinds::COMMENT | kinds::TARGET | kinds::SYSTEM_MESSAGE | kinds::TITLE | kinds::SUBTITLE
    ) || node.kind == "substitution_definition"
        || node.kind == "pending"
        || node.kind == "raw"
        || node.kind == "meta"
        || node.kind == "decoration"
}

/// The document's metadata: field name -> field body text, empty when the
/// document does not open with a field list.
pub fn document_metadata(doctree: &Doctree) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();

    let Some(first) = doctree
        .root
        .children
        .iter()
        .find(|node| !is_pre_bibliographic(node))
    else {
        return metadata;
    };
    if first.kind != kinds::FIELD_LIST {
        return metadata;
    }

    for field in &first.children {
        if field.kind != kinds::FIELD {
            continue;
        }
        let name = field
            .children
            .iter()
            .find(|child| child.kind == kinds::FIELD_NAME);
        let body = field
            .children
            .iter()
            .find(|child| child.kind == kinds::FIELD_BODY);
        if let Some(name) = name {
            metadata.insert(name.astext(), body.map(Node::astext).unwrap_or_default());
        }
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst;

    fn parse(source: &str) -> Doctree {
        rst::parse_rst(source, &rst::ParseOptions::default())
    }

    #[test]
    fn leading_field_list_becomes_metadata() {
        let metadata = document_metadata(&parse(":orphan:\n\nTitle\n=====\n\nBody.\n"));
        assert_eq!(metadata.get("orphan").map(String::as_str), Some(""));
    }

    #[test]
    fn field_bodies_are_captured() {
        let metadata = document_metadata(&parse(":tocdepth: 2\n:nocomments:\n\nBody.\n"));
        assert_eq!(metadata.get("tocdepth").map(String::as_str), Some("2"));
        assert!(metadata.contains_key("nocomments"));
    }

    #[test]
    fn comments_and_targets_do_not_hide_the_field_list() {
        // docutils skips PreBibliographic nodes before looking for docinfo.
        let metadata = document_metadata(&parse(".. a comment\n\n:orphan:\n\nBody.\n"));
        assert!(metadata.contains_key("orphan"), "{metadata:?}");
    }

    /// `raw` is `PreBibliographic` too (`docutils/nodes.py:2600`), and it is
    /// the one member of that set a document can produce before any
    /// transform runs. Verified against docutils 0.22.4: the same source
    /// yields `<document><docinfo><field classes="orphan">…` — the field
    /// list is still bibliographic with a `.. raw::` block ahead of it.
    #[test]
    fn a_leading_raw_block_does_not_hide_the_field_list() {
        let metadata = document_metadata(&parse(".. raw:: html\n\n   <hr>\n\n:orphan:\n\nBody.\n"));
        assert_eq!(metadata.get("orphan").map(String::as_str), Some(""));
    }

    #[test]
    fn a_field_list_below_the_first_body_element_is_not_metadata() {
        let metadata = document_metadata(&parse("Body.\n\n:orphan:\n"));
        assert!(metadata.is_empty(), "{metadata:?}");
    }

    #[test]
    fn a_document_without_fields_has_no_metadata() {
        assert!(document_metadata(&parse("Title\n=====\n")).is_empty());
    }
}
