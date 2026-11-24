//! `env.metadata[docname]` — the document's bibliographic fields, read as
//! build metadata.
//!
//! Port of `MetadataCollector.process_doc`
//! (`SP/sphinx/environment/collectors/metadata.py:35-68`). docutils'
//! `DocInfo` transform (340, [`crate::transforms::misc::DocInfo`]) has
//! turned a leading field list into the document's `docinfo`; the collector
//! reads it into the metadata and pops it out of the doctree, so the
//! persisted doctree never holds it. Sphinx disables `DocTitle`
//! (`doctitle_xform = False`), so a document whose title comes first has
//! no docinfo.
//!
//! The metadata keeps a string per key, which is all its consumers read
//! (`orphan`, `nosearch`, `nocomments`, `tocdepth`, which they parse
//! themselves). Two of Sphinx's values are not strings and are left out:
//! the `authors` list, and `tocdepth` coerced to an int (kept as the
//! field's text, which the consumers parse as Sphinx coerces it).

use std::collections::BTreeMap;

use crate::doctree::{is_text_element, kinds, Doctree, Node};
use crate::env::BuildEnvironment;
use crate::transforms::misc::is_pre_bibliographic;

/// `MetadataCollector.process_doc` for the document `docname`: when the
/// first child of the document that is not `PreBibliographic`
/// ([`is_pre_bibliographic`]) is its `docinfo`, each child of it becomes
/// metadata — a `field`, its name's `astext()` to its body's; another
/// `TextElement` (`author`, `date`, …), its class name to its `astext()`;
/// `authors` is left out (see the module docs) — and the docinfo leaves the
/// doctree (`doctree.pop(index)`). The document's metadata is recorded
/// either way, empty without a docinfo.
pub fn process_doc(env: &mut BuildEnvironment, docname: &str, doctree: &mut Doctree) {
    let root = &mut doctree.root;
    let docinfo = root
        .children
        .iter()
        .position(|node| !is_pre_bibliographic(node.kind))
        .filter(|&index| root.children[index].kind == "docinfo")
        .map(|index| root.children.remove(index));
    let metadata = docinfo.as_ref().map(docinfo_metadata).unwrap_or_default();
    env.metadata.insert(docname.to_string(), metadata);
}

/// The metadata of a `docinfo` node, in the order the collector writes it
/// (a later key overwrites an earlier one, as in the dict).
fn docinfo_metadata(docinfo: &Node) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    for node in &docinfo.children {
        if node.kind == "authors" {
            continue;
        }
        if node.kind == kinds::FIELD {
            if let [name, body] = node.children.as_slice() {
                metadata.insert(name.astext_docutils(), body.astext_docutils());
            }
        } else if is_text_element(node.kind) {
            metadata.insert(node.kind.to_string(), node.astext_docutils());
        }
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::{read_standalone, ReadSettings};
    use crate::rst::ParseOptions;

    /// The metadata of `source` read as Sphinx reads it (stage R's DocInfo,
    /// stage M's collector), and whether a `docinfo` or a field list is
    /// left in the doctree.
    fn read(source: &str) -> (BTreeMap<String, String>, bool) {
        let parse = ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            ..ParseOptions::default()
        };
        let read = read_standalone(source, &parse, &ReadSettings::sphinx_defaults());
        let left = read
            .outcome
            .doctree()
            .root
            .children
            .iter()
            .any(|node| matches!(node.kind, "docinfo" | kinds::FIELD_LIST));
        (read.env.metadata["index"].clone(), left)
    }

    #[test]
    fn leading_field_list_becomes_metadata() {
        let (metadata, left) = read(":orphan:\n\nTitle\n=====\n\nBody.\n");
        assert_eq!(metadata.get("orphan").map(String::as_str), Some(""));
        assert!(!left);
    }

    #[test]
    fn field_bodies_are_captured() {
        let (metadata, _) = read(":tocdepth: 2\n:nocomments:\n\nBody.\n");
        assert_eq!(metadata.get("tocdepth").map(String::as_str), Some("2"));
        assert!(metadata.contains_key("nocomments"));
    }

    /// The bibliographic fields DocInfo knows become elements, which the
    /// collector keys by class name (Sphinx 9.1.0: `{'version': '1',
    /// 'revision': '2', 'status': 's', 'organization': 'o', 'contact': 'c',
    /// 'address': '1\nStreet', 'copyright': 'cc'}`); an RCS keyword is
    /// cleaned (`{'status': 'frontmatter.py', 'date': '2002-10-23'}`).
    #[test]
    fn bibliographic_elements_are_keyed_by_class() {
        let (metadata, _) = read(
            ":Version: 1\n:Revision: 2\n:Status: s\n:Organization: o\n:Contact: c\n\
             :Address: 1\n   Street\n:Copyright: cc\n\nBody.\n",
        );
        let expected: BTreeMap<String, String> = [
            ("version", "1"),
            ("revision", "2"),
            ("status", "s"),
            ("organization", "o"),
            ("contact", "c"),
            ("address", "1\nStreet"),
            ("copyright", "cc"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
        assert_eq!(metadata, expected);
        let (metadata, _) = read(
            ":Status: $RCSfile: frontmatter.py,v $\n:Date: $Date: 2002/10/23 21:38:00 $\n\n\
             Body.\n",
        );
        assert_eq!(
            metadata.get("status").map(String::as_str),
            Some("frontmatter.py")
        );
        assert_eq!(metadata.get("date").map(String::as_str), Some("2002-10-23"));
    }

    #[test]
    fn comments_and_targets_do_not_hide_the_field_list() {
        // docutils skips PreBibliographic nodes before looking for docinfo.
        let (metadata, left) = read(".. a comment\n\n:orphan:\n\nBody.\n");
        assert!(metadata.contains_key("orphan"), "{metadata:?}");
        assert!(!left);
    }

    /// `raw` is `PreBibliographic` too (`docutils/nodes.py:2600`).
    /// Verified against docutils 0.22.4: the same source yields
    /// `<document><docinfo><field classes="orphan">…` — the field list is
    /// still bibliographic with a `.. raw::` block ahead of it. So is
    /// Sphinx's `index`.
    #[test]
    fn a_leading_raw_block_or_index_does_not_hide_the_field_list() {
        let (metadata, _) = read(".. raw:: html\n\n   <hr>\n\n:orphan:\n\nBody.\n");
        assert_eq!(metadata.get("orphan").map(String::as_str), Some(""));
        let (metadata, _) = read(".. index:: x\n\n:orphan:\n\nBody.\n");
        assert_eq!(metadata.get("orphan").map(String::as_str), Some(""));
    }

    #[test]
    fn a_field_list_below_the_first_body_element_is_not_metadata() {
        let (metadata, left) = read("Body.\n\n:orphan:\n");
        assert!(metadata.is_empty(), "{metadata:?}");
        assert!(left);
    }

    #[test]
    fn a_document_without_fields_has_no_metadata() {
        assert!(read("Title\n=====\n").0.is_empty());
    }
}
