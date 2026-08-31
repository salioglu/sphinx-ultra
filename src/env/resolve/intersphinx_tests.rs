//! Where intersphinx meets the reference resolver: the hook sits exactly
//! where Sphinx's `missing-reference` event does — after the local domain
//! has failed, before the dangling-reference warning
//! (`transforms/post_transforms/__init__.py:112-158`) — and `:external:`
//! nodes are resolved ahead of all of it, as `IntersphinxRoleResolver`
//! (priority `ReferencesResolver.default_priority - 1`) does.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::*;
use crate::doctree::{kinds, AttrValue, Doctree, Node, Span};
use crate::intersphinx::{Intersphinx, IntersphinxData};
use crate::inventory::InventoryFile;

fn inventory_bytes(project: &str, version: &str, entries: &[&str]) -> Vec<u8> {
    use std::io::Write;
    let mut out = format!(
        "# Sphinx inventory version 2\n\
         # Project: {project}\n\
         # Version: {version}\n\
         # The remainder of this file is compressed using zlib.\n"
    )
    .into_bytes();
    let mut body = String::new();
    for entry in entries {
        body.push_str(entry);
        body.push('\n');
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(9));
    encoder.write_all(body.as_bytes()).unwrap();
    out.extend_from_slice(&encoder.finish().unwrap());
    out
}

fn loaded_intersphinx() -> Intersphinx {
    let bytes = inventory_bytes(
        "Other",
        "2.0",
        &[
            "some-label std:label -1 page.html#$ A Section",
            "somedoc std:doc -1 somedoc.html Some Doc",
            "other.thing py:function 1 api.html#$ -",
        ],
    );
    let inventory = InventoryFile::loads(&bytes, "https://other.example/").unwrap();
    let mut data = IntersphinxData {
        main: inventory.clone(),
        ..Default::default()
    };
    data.named.insert("other".to_string(), inventory);
    Intersphinx {
        data,
        disabled_reftypes: BTreeSet::from(["std:doc".to_string()]),
        resolve_self: String::new(),
    }
}

/// One document holding a single `pending_xref` with the given attributes.
fn document(attrs: &[(&'static str, AttrValue)], contnode_text: &str) -> Doctree {
    let mut xref = Node::elem(kinds::PENDING_XREF, Span::ZERO);
    for (key, value) in attrs {
        xref.set(key, value.clone());
    }
    if !contnode_text.is_empty() {
        let mut inner = Node::elem("inline", Span::ZERO);
        inner.attrs.classes = vec!["xref".to_string()];
        inner
            .children
            .push(Node::text_node(contnode_text, Span::ZERO));
        xref.children.push(inner);
    }
    let mut paragraph = Node::elem(kinds::PARAGRAPH, Span::ZERO);
    paragraph.children.push(xref);
    let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
    root.children.push(paragraph);
    Doctree {
        root,
        sources: vec!["index.rst".to_string()],
    }
}

fn xref_attrs(domain: &str, reftype: &str, target: &str) -> Vec<(&'static str, AttrValue)> {
    vec![
        ("refdoc", AttrValue::Str("index".to_string())),
        ("refdomain", AttrValue::Str(domain.to_string())),
        ("reftype", AttrValue::Str(reftype.to_string())),
        ("reftarget", AttrValue::Str(target.to_string())),
        ("refexplicit", AttrValue::Int(0)),
        ("refwarn", AttrValue::Int(1)),
    ]
}

struct Resolved {
    doctree: Doctree,
    warnings: Vec<String>,
}

impl Resolved {
    /// The node that replaced the `pending_xref`, if any survived.
    fn node(&self) -> Option<&Node> {
        self.doctree.root.children.first()?.children.first()
    }
}

fn resolve(isx: &Intersphinx, mut doctree: Doctree) -> Resolved {
    let env = {
        let mut env = BuildEnvironment::default();
        env.std.labels.insert(
            "local-label".to_string(),
            (
                "index".to_string(),
                "local-label".to_string(),
                "Local".to_string(),
            ),
        );
        env.all_docs.insert("index".to_string(), 0);
        env
    };
    let formats = BTreeMap::new();
    let resolver = Resolver {
        env: &env,
        numfig: true,
        numfig_format: &formats,
        doctree: &|_| None,
        relative_uri: &|_, _| String::new(),
        intersphinx: isx,
    };
    let nitpick = NitpickConfig {
        nitpicky: false,
        ignore: &[],
        ignore_regex: &[],
    };
    let resolution = resolve_document(
        &resolver,
        &nitpick,
        "index",
        &mut doctree,
        "",
        Path::new("index.rst"),
    );
    Resolved {
        doctree,
        warnings: resolution
            .warnings
            .iter()
            .map(|warning| warning.render())
            .collect(),
    }
}

#[test]
fn without_a_mapping_the_hook_changes_nothing() {
    // The environment oracle builds projects with no intersphinx at all;
    // adding the hook must leave every one of their dangling warnings
    // exactly as it was.
    let inert = Intersphinx::default();
    let resolved = resolve(
        &inert,
        document(&xref_attrs("std", "ref", "nowhere"), "nowhere"),
    );
    assert_eq!(
        resolved.warnings,
        vec!["index.rst:1: WARNING: undefined label: 'nowhere' [ref.ref]"]
    );
    assert_eq!(
        resolved.node().map(|node| node.kind),
        Some("inline"),
        "the content node stays in place"
    );
}

#[test]
fn a_reference_the_local_domain_missed_resolves_through_an_inventory() {
    let resolved = resolve(
        &loaded_intersphinx(),
        document(&xref_attrs("std", "ref", "some-label"), "some-label"),
    );

    assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    let node = resolved.node().expect("a reference node");
    assert_eq!(node.kind, kinds::REFERENCE);
    assert_eq!(
        node.get("refuri"),
        Some(&AttrValue::Str(
            "https://other.example/page.html#some-label".to_string()
        ))
    );
    assert_eq!(
        node.get("reftitle"),
        Some(&AttrValue::Str("(in Other v2.0)".to_string()))
    );
    assert_eq!(
        node.get("internal"),
        Some(&AttrValue::Int(0)),
        "an intersphinx reference is external, unlike every local one"
    );
    assert_eq!(
        node.astext(),
        "A Section",
        "the inventory's display name replaces the written text"
    );
}

#[test]
fn the_local_domain_still_wins_over_an_inventory() {
    let resolved = resolve(
        &loaded_intersphinx(),
        document(&xref_attrs("std", "ref", "local-label"), "local-label"),
    );
    let node = resolved.node().expect("a reference node");
    assert_eq!(
        node.get("internal"),
        Some(&AttrValue::Int(1)),
        "resolution order is domain first, intersphinx second"
    );
}

#[test]
fn a_reference_into_a_domain_this_build_cannot_resolve_still_tries_intersphinx() {
    let resolved = resolve(
        &loaded_intersphinx(),
        document(&xref_attrs("py", "func", "other.thing"), "other.thing()"),
    );
    let node = resolved.node().expect("a reference node");
    assert_eq!(
        node.get("refuri"),
        Some(&AttrValue::Str(
            "https://other.example/api.html#other.thing".to_string()
        ))
    );
    assert!(resolved.warnings.is_empty());
}

#[test]
fn a_self_referential_prefix_falls_back_to_the_local_domain() {
    let mut isx = loaded_intersphinx();
    isx.resolve_self = "mine".to_string();

    // `mine:local-label` names this project, so the prefix is stripped and
    // the local label resolves.
    let resolved = resolve(
        &isx,
        document(&xref_attrs("std", "ref", "mine:local-label"), "Local"),
    );
    let node = resolved.node().expect("a reference node");
    assert_eq!(node.get("internal"), Some(&AttrValue::Int(1)));
    assert!(resolved.warnings.is_empty());

    // The same prefix on a target that does not exist locally reports the
    // *written* target, not the stripped one.
    let resolved = resolve(
        &isx,
        document(&xref_attrs("std", "ref", "mine:nope"), "nope"),
    );
    assert_eq!(
        resolved.warnings,
        vec!["index.rst:1: WARNING: undefined label: 'mine:nope' [ref.ref]"]
    );
}

// -- the `:external:` role's own resolution path --

fn external_attrs(
    domain: &str,
    reftype: &str,
    target: &str,
    inventory: Option<&str>,
) -> Vec<(&'static str, AttrValue)> {
    let mut attrs = xref_attrs(domain, reftype, target);
    attrs.push(("intersphinx", AttrValue::Int(1)));
    if let Some(inventory) = inventory {
        attrs.push(("inventory", AttrValue::Str(inventory.to_string())));
    }
    attrs
}

#[test]
fn an_external_reference_bypasses_the_disabled_reftypes() {
    // `std:doc` is disabled by default, but `:external:std:doc:` is not a
    // bare reference.
    let resolved = resolve(
        &loaded_intersphinx(),
        document(&external_attrs("std", "doc", "somedoc", None), "somedoc"),
    );
    assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    assert_eq!(
        resolved.node().and_then(|node| node.get("refuri")),
        Some(&AttrValue::Str(
            "https://other.example/somedoc.html".to_string()
        ))
    );
}

#[test]
fn an_unresolved_external_reference_warns_with_sphinxs_exact_text() {
    let resolved = resolve(
        &loaded_intersphinx(),
        document(&external_attrs("std", "ref", "whatever", None), "whatever"),
    );
    assert_eq!(
        resolved.warnings,
        vec![
            "index.rst:1: WARNING: external std:ref reference target not found: whatever [ref.ref]"
        ]
    );
    assert_eq!(
        resolved.node().map(|node| node.kind),
        Some("inline"),
        "the content node replaces the failed reference"
    );
}

#[test]
fn an_external_reference_to_an_unknown_inventory_warns_and_leaves_nothing() {
    let resolved = resolve(
        &loaded_intersphinx(),
        document(
            &external_attrs("std", "ref", "some-label", Some("nope")),
            "some-label",
        ),
    );
    assert_eq!(
        resolved.warnings,
        vec![
            "index.rst:1: WARNING: inventory for external cross-reference not found: 'nope' \
             [intersphinx.external]"
        ]
    );
    assert!(
        resolved.node().is_none(),
        "Sphinx's role returns no nodes at all for this failure"
    );
}

#[test]
fn a_role_name_the_parser_could_not_use_warns_at_resolution_and_leaves_nothing() {
    let mut attrs = vec![
        ("refdoc", AttrValue::Str("index".to_string())),
        ("intersphinx", AttrValue::Int(1)),
    ];
    attrs.push((
        "intersphinx_role_error",
        AttrValue::Str("invalid external cross-reference suffix: 'a:b:c'".to_string()),
    ));
    let resolved = resolve(&loaded_intersphinx(), document(&attrs, ""));

    assert_eq!(
        resolved.warnings,
        vec![
            "index.rst:1: WARNING: invalid external cross-reference suffix: 'a:b:c' \
             [intersphinx.external]"
        ]
    );
    assert!(resolved.node().is_none());
}

#[test]
fn an_unknown_inventory_is_reported_before_a_bad_role_name() {
    // Sphinx checks the inventory first (`_resolve.py:385-390`), so a role
    // that is wrong in both ways reports the inventory.
    let mut attrs = external_attrs("std", "ref", "x", Some("nope"));
    attrs.push((
        "intersphinx_role_error",
        AttrValue::Str("invalid external cross-reference suffix: 'a:b:c'".to_string()),
    ));
    let resolved = resolve(&loaded_intersphinx(), document(&attrs, "x"));

    assert_eq!(resolved.warnings.len(), 1);
    assert!(
        resolved.warnings[0].contains("inventory for external cross-reference not found"),
        "{:?}",
        resolved.warnings
    );
}

#[test]
fn an_external_reference_into_a_self_referential_inventory_resolves_locally() {
    let mut isx = loaded_intersphinx();
    isx.resolve_self = "mine".to_string();
    let resolved = resolve(
        &isx,
        document(
            &external_attrs("std", "ref", "local-label", Some("mine")),
            "Local",
        ),
    );
    let node = resolved.node().expect("a reference node");
    assert_eq!(
        node.get("internal"),
        Some(&AttrValue::Int(1)),
        "a self-referential inventory sends the node through local resolution"
    );
    assert!(resolved.warnings.is_empty());
}
