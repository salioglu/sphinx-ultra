//! Sphinx's citation transforms' tree halves (619), docutils' `Footnotes`
//! (620), Sphinx's `UnreferencedFootnotesDetector` (622) and
//! `FootnoteDocnameUpdater` (700), ported one row of the read-transform
//! table at a time ([`super::READ_TRANSFORMS`]).
//!
//! Footnotes and citations are linked to their references through the
//! parse's registries ([`super::state::Registries`]), which hold the nodes
//! by identity in registration order: a reference the parse discarded with
//! a rejected substitution definition or table is still numbered and still
//! backlinked, and the citation references 619 takes out of the tree still
//! give their citation its backrefs ([DS §4.3-4.4]).

use super::references::replace_with_problematic;
use super::state::{CitationNote, NodeView};
use super::tree::{TxId, TxNode, TxTree};
use super::{Transform, TransformCx};
use crate::diagnostics::Category;
use crate::doctree::messages::ERROR;
use crate::doctree::{kinds, AttrValue, Uid};

/// 619-016 `CitationDefinitionTransform`
/// (`SP/sphinx/domains/citation.py:133-148`), its tree half: every citation
/// of the tree takes the document's name as its `docname`, and its label,
/// its first child, is not smartquoted (`support_smartquotes="0"`). What
/// the transform registers with the citation domain, each citation's
/// label, ids and node in tree order, is recorded
/// ([`super::state::DocumentState::citation_notes`]) for the domain's
/// stage-M half (T13b). The loop moves no node.
pub struct CitationDefinitionTransform;

impl Transform for CitationDefinitionTransform {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        for citation in cx.tree.findall(root, |node| node.kind == kinds::CITATION) {
            let docname = AttrValue::Str(cx.docname.to_string());
            cx.tree.node_mut(citation).set("docname", docname);
            let label = cx.tree.node(citation).children.first().copied();
            let node = cx.tree.assign_uid(citation, cx.state);
            cx.state.citation_notes.push(CitationNote {
                label: label.map(|label| cx.tree.astext(label)).unwrap_or_default(),
                ids: cx.tree.node(citation).attrs.ids.clone(),
                node,
            });
            if let Some(label) = label {
                cx.tree
                    .node_mut(label)
                    .set("support_smartquotes", AttrValue::Int(0));
            }
        }
    }
}

/// 619-017 `CitationReferenceTransform` (`SP/sphinx/domains/citation.py:151-175`),
/// its tree half: every citation reference of the tree is replaced by a
/// `pending_xref` the write phase resolves through the citation domain,
/// to the target its text names, holding `[target]` in an `inline`. The
/// `pending_xref` keeps the reference's ids and classes, and its location
/// (`copy_source_info`; here, its span). The reference leaves the tree but
/// stays in `citation_refs`, where Footnotes (620) finds it. Each target
/// is recorded, in tree order
/// ([`super::state::DocumentState::citation_reference_targets`]), for the
/// domain's `note_citation_reference` in stage M (T13b). Each reference
/// becomes one node, so the live loop skips nothing.
pub struct CitationReferenceTransform;

impl Transform for CitationReferenceTransform {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        let is_reference = |node: &TxNode| node.kind == kinds::CITATION_REFERENCE;
        for reference in cx.tree.findall(root, is_reference) {
            let target = cx.tree.astext(reference);
            let node = cx.tree.node(reference);
            let (ids, classes, span) = (
                node.attrs.ids.clone(),
                node.attrs.classes.clone(),
                node.span,
            );
            let xref = cx.tree.new_node(kinds::PENDING_XREF);
            let inline = cx.tree.new_node("inline");
            let text = cx.tree.new_node(kinds::TEXT);
            cx.tree.node_mut(text).text = Some(format!("[{target}]"));
            cx.tree.append(inline, text);
            cx.tree.node_mut(inline).set_rawsource(&target);
            let node = cx.tree.node_mut(xref);
            node.span = span;
            node.attrs.ids = ids;
            node.attrs.classes = classes;
            node.set_rawsource(&target);
            for (key, value) in [
                ("refdomain", AttrValue::Str("citation".to_string())),
                ("reftype", AttrValue::Str("ref".to_string())),
                ("reftarget", AttrValue::Str(target.clone())),
                ("refwarn", AttrValue::Int(1)),
                ("support_smartquotes", AttrValue::Int(0)),
            ] {
                node.set(key, value);
            }
            cx.tree.append(xref, inline);
            cx.tree.replace_self(reference, vec![xref]);
            cx.state.citation_reference_targets.push(target);
        }
    }
}

/// 620-011 `Footnotes` (`SP/docutils/transforms/references.py:416-636`):
/// numbers the auto-numbered footnotes and their references
/// ([`number_footnotes`], [`number_footnote_references`]), labels the
/// symbol footnotes and their references ([`symbolize_footnotes`]), and
/// links the manually numbered footnotes and the citations to theirs
/// ([`resolve_footnotes_and_citations`]), each from its registry.
///
/// The document's `autofootnote_start` and `symbol_footnote_start` start
/// at 1 and 0, and only this run moves them.
pub struct Footnotes;

/// `Footnotes.symbols`: the labels of the symbol footnotes, repeated once
/// more each time round.
const SYMBOLS: [&str; 10] = [
    "*", "\u{2020}", "\u{2021}", "\u{a7}", "\u{b6}", "#", "\u{2660}", "\u{2665}", "\u{2666}",
    "\u{2663}",
];

impl Transform for Footnotes {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let labels = number_footnotes(cx, 1);
        number_footnote_references(cx, &labels);
        symbolize_footnotes(cx);
        resolve_footnotes_and_citations(cx);
    }
}

/// The nodes a registry holds, in its order: every registered node is in
/// the arena, in the tree, out of it or in the detached forest.
fn registered(tree: &TxTree, uids: &[Uid]) -> Vec<TxId> {
    uids.iter().filter_map(|&uid| tree.by_uid(uid)).collect()
}

/// Whether the node has the attribute `key` (`node.hasattr(key)`).
fn has(tree: &TxTree, id: TxId, key: &'static str) -> bool {
    tree.node(id).get(key).is_some()
}

/// `node += nodes.Text(text)`.
fn append_text(tree: &mut TxTree, node: TxId, text: &str) {
    let child = tree.new_node(kinds::TEXT);
    tree.node_mut(child).text = Some(text.to_string());
    tree.append(node, child);
}

/// `footnote.insert(0, nodes.label('', text))`.
fn insert_label(tree: &mut TxTree, footnote: TxId, text: &str) {
    let label = tree.new_node(kinds::LABEL);
    append_text(tree, label, text);
    tree.insert(footnote, 0, label);
}

/// The reference at `reference` points at the note at `note`: it takes
/// the note's first id as its `refid`, and the note its first id as a
/// backref (`ref['refid'] = note['ids'][0]`, `note.add_backref(ref['ids'][0])`;
/// docutils asserts both have exactly one).
fn link(cx: &mut TransformCx<'_>, reference: TxId, note: TxId) {
    if let Some(id) = cx.tree.node(note).attrs.ids.first().cloned() {
        cx.tree.node_mut(reference).set("refid", AttrValue::Str(id));
    }
    if let Some(id) = cx.tree.node(reference).attrs.ids.first().cloned() {
        cx.tree.node_mut(note).attrs.backrefs.push(id);
    }
}

/// `Footnotes.number_footnotes` (`references.py:494-521`): each
/// auto-numbered footnote, in registration order, takes the first number
/// from `startnum` on that no name holds (`label not in nameids`, so a
/// manual `[1]` or a target named `1` keeps it) as its `label`; each
/// reference to its name (`[#name]_`) takes the number as its text and
/// the footnote's id, and is resolved; a footnote with neither a name nor
/// a duplicate one takes the number as its name, bound as an explicit
/// target, and the number joins the labels the anonymous references take
/// ([`number_footnote_references`]), which this returns.
///
/// The number is a name no node holds, so binding it never meets a
/// duplicate: NameTable's bind-time snapshot of an earlier binding
/// ([`super::state::NameEntry::refuri`]) is never consulted here.
fn number_footnotes(cx: &mut TransformCx<'_>, mut startnum: u32) -> Vec<String> {
    let mut labels = Vec::new();
    let footnotes = registered(cx.tree, &cx.state.registries.autofootnotes);
    for footnote in footnotes {
        let label = loop {
            let label = startnum.to_string();
            startnum += 1;
            if !cx.state.names.has_name(&label) {
                break label;
            }
        };
        insert_label(cx.tree, footnote, &label);
        for name in cx.tree.node(footnote).attrs.names.clone() {
            let references = cx
                .state
                .registries
                .footnote_refs
                .get(&name)
                .map(|uids| registered(cx.tree, uids))
                .unwrap_or_default();
            for reference in references {
                append_text(cx.tree, reference, &label);
                cx.tree.node_mut(reference).delattr("refname");
                link(cx, reference, footnote);
                cx.note_refid(reference);
                cx.tree.set_resolved(reference);
            }
        }
        let node = cx.tree.node(footnote);
        if node.attrs.names.is_empty() && node.attrs.dupnames.is_empty() {
            cx.tree.node_mut(footnote).attrs.names.push(label.clone());
            // `note_explicit_target(footnote, footnote)`: `set_id` keeps
            // the footnote's id, and `set_name_id_map` binds the number.
            let node = cx.tree.node(footnote);
            if let Some(id) = node.attrs.ids.last() {
                let view = NodeView {
                    kind: node.kind,
                    names: &node.attrs.names,
                    refuri: node.str_attr("refuri"),
                    refname: node.str_attr("refname"),
                };
                let duplicates = cx.state.names.bind(view, id, true);
                debug_assert!(duplicates.is_empty(), "a footnote number is a new name");
            }
            labels.push(label);
        }
    }
    labels
}

/// `Footnotes.number_footnote_references` (`references.py:523-559`): each
/// auto-numbered reference, in registration order, not resolved and
/// without a `refid`, takes the next of `labels` as its text and the id
/// the label is bound to, and the footnote holding it a backref; a
/// `[#name]_` whose footnote is missing is numbered too, and keeps its
/// `refname`. When the labels run out, an ERROR is reported at the first
/// reference left over, and every reference from the registry's position
/// of that label on, but a resolved one or one with a `refname`, becomes
/// a problematic ([`replace_with_problematic`]).
fn number_footnote_references(cx: &mut TransformCx<'_>, labels: &[String]) {
    let references = cx.state.registries.autofootnote_refs.clone();
    let mut i = 0;
    for &uid in &references {
        let Some(reference) = cx.tree.by_uid(uid) else {
            continue;
        };
        if cx.tree.resolved(reference) || has(cx.tree, reference, "refid") {
            continue;
        }
        let Some(label) = labels.get(i) else {
            let n = labels.len();
            let s = if n > 1 { "s" } else { "" };
            let text = format!(
                "Too many autonumbered footnote references: only {n} corresponding footnote{s} \
                 available."
            );
            let message = cx.system_message(ERROR, &text, Some(reference), Vec::new());
            let msgid = cx.set_id(message);
            for other in registered(cx.tree, &references[i..]) {
                if cx.tree.resolved(other) || has(cx.tree, other, "refname") {
                    continue;
                }
                replace_with_problematic(cx, other, message, &msgid);
            }
            break;
        };
        append_text(cx.tree, reference, label);
        let id = cx.state.names.get(label).and_then(|entry| entry.id.clone());
        if let Some(id) = id {
            cx.tree
                .node_mut(reference)
                .set("refid", AttrValue::Str(id.clone()));
            cx.note_refid(reference);
            let backref = cx.tree.node(reference).attrs.ids.first().cloned();
            if let (Some(footnote), Some(backref)) = (cx.tree.by_id(&id), backref) {
                cx.tree.node_mut(footnote).attrs.backrefs.push(backref);
            }
        }
        cx.tree.set_resolved(reference);
        i += 1;
    }
}

/// `Footnotes.symbolize_footnotes` (`references.py:561-599`): each symbol
/// footnote, in registration order, takes the next symbol as its `label`,
/// `*` to `♣` and then each doubled, tripled, …; each symbol reference
/// takes the label of the footnote at its own position as its text and
/// that footnote's id, and the footnote a backref. When the footnotes run
/// out, an ERROR is reported at the first reference left over, and every
/// reference from it on, but a resolved one or one with a `refid`,
/// becomes a problematic. A symbol reference is never marked resolved.
fn symbolize_footnotes(cx: &mut TransformCx<'_>) {
    let footnotes = registered(cx.tree, &cx.state.registries.symbol_footnotes);
    let mut labels = Vec::with_capacity(footnotes.len());
    for (start, &footnote) in footnotes.iter().enumerate() {
        let label = SYMBOLS[start % SYMBOLS.len()].repeat(start / SYMBOLS.len() + 1);
        insert_label(cx.tree, footnote, &label);
        labels.push(label);
    }
    let references = cx.state.registries.symbol_footnote_refs.clone();
    for (i, &uid) in references.iter().enumerate() {
        let Some(reference) = cx.tree.by_uid(uid) else {
            continue;
        };
        let (Some(label), Some(&footnote)) = (labels.get(i), footnotes.get(i)) else {
            let text = format!(
                "Too many symbol footnote references: only {} corresponding footnotes \
                 available.",
                labels.len()
            );
            let message = cx.system_message(ERROR, &text, Some(reference), Vec::new());
            let msgid = cx.set_id(message);
            for other in registered(cx.tree, &references[i..]) {
                if cx.tree.resolved(other) || has(cx.tree, other, "refid") {
                    continue;
                }
                replace_with_problematic(cx, other, message, &msgid);
            }
            break;
        };
        append_text(cx.tree, reference, label);
        link(cx, reference, footnote);
        cx.note_refid(reference);
    }
}

/// `Footnotes.resolve_footnotes_and_citations` (`references.py:601-622`):
/// each manually numbered footnote, then each citation, in registration
/// order, takes every reference registered under one of its names
/// ([`resolve_references`]). The citation references 619 replaced are
/// among them.
fn resolve_footnotes_and_citations(cx: &mut TransformCx<'_>) {
    let footnotes = registered(cx.tree, &cx.state.registries.footnotes);
    for footnote in footnotes {
        for label in cx.tree.node(footnote).attrs.names.clone() {
            let references = cx.state.registries.footnote_refs.get(&label);
            if let Some(references) = references.map(|uids| registered(cx.tree, uids)) {
                resolve_references(cx, footnote, &references);
            }
        }
    }
    let citations = registered(cx.tree, &cx.state.registries.citations);
    for citation in citations {
        for label in cx.tree.node(citation).attrs.names.clone() {
            let references = cx.state.registries.citation_refs.get(&label);
            if let Some(references) = references.map(|uids| registered(cx.tree, uids)) {
                resolve_references(cx, citation, &references);
            }
        }
    }
}

/// `Footnotes.resolve_references` (`references.py:624-636`): each
/// reference not resolved yet drops its `refname`, takes the note's id as
/// its `refid` and gives the note a backref, and is resolved. No refid is
/// noted.
fn resolve_references(cx: &mut TransformCx<'_>, note: TxId, references: &[TxId]) {
    for &reference in references {
        if cx.tree.resolved(reference) {
            continue;
        }
        cx.tree.node_mut(reference).delattr("refname");
        link(cx, reference, note);
        cx.tree.set_resolved(reference);
    }
}

/// 622-028 `UnreferencedFootnotesDetector`
/// (`SP/sphinx/transforms/__init__.py:288-324`): warns about every footnote
/// no reference reached (no backrefs), registry by registry — the manually
/// numbered ones, the symbol ones, then the auto-numbered ones — at the
/// footnote ([`TransformCx::warn`], `[ref.footnote]`). A numbered footnote
/// whose name a duplicate moved to `dupnames` is left to docutils'
/// duplicate warning.
pub struct UnreferencedFootnotesDetector;

impl Transform for UnreferencedFootnotesDetector {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let registries = &cx.state.registries;
        let lists = [
            (registries.footnotes.clone(), None),
            (registries.symbol_footnotes.clone(), Some("*")),
            (registries.autofootnotes.clone(), Some("#")),
        ];
        for (uids, label) in lists {
            for footnote in registered(cx.tree, &uids) {
                let node = cx.tree.node(footnote);
                let named = label == Some("*") || !node.attrs.names.is_empty();
                if !node.attrs.backrefs.is_empty() || !named {
                    continue;
                }
                let label = label
                    .unwrap_or_else(|| node.attrs.names[0].as_str())
                    .to_string();
                cx.warn(
                    footnote,
                    format!("Footnote [{label}] is not referenced."),
                    Category::from_parts(Some("ref"), Some("footnote")),
                );
            }
        }
    }
}

/// 700-015 `FootnoteDocnameUpdater` (`SP/sphinx/builders/latex/transforms.py:34-43`):
/// every footnote and footnote reference of the tree takes the document's
/// name as its `docname`. The loop moves no node.
pub struct FootnoteDocnameUpdater;

impl Transform for FootnoteDocnameUpdater {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        let footnotes = cx.tree.findall(root, |node| {
            matches!(node.kind, kinds::FOOTNOTE | kinds::FOOTNOTE_REFERENCE)
        });
        for node in footnotes {
            let docname = AttrValue::Str(cx.docname.to_string());
            cx.tree.node_mut(node).set("docname", docname);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::transforms::test_support::{differences, SphinxRead};

    // oracle (Sphinx 9.1.0, docutils 0.22.4) for every row of this module:
    //   the input read as `index.rst` through tools/gen_read_state_fixture.py's
    //   harness (`keep_warnings = True`): the post-read doctree, the warning
    //   stream and the id trace. Rows named after a corpus case are that
    //   case's input; `state.*` rows are the read-state fixture's.

    const DOCUTILS_0236: SphinxRead = SphinxRead {
        rst: "Refs [1]_ [#]_ [#label]_ [*]_ end.\n",
        body: "    <paragraph>\n        Refs \n        <problematic ids=\"id1\" \
            refid=\"id9\">\n            [1]_\n         \n        <problematic ids=\"id6 id2\" \
            refid=\"id5\">\n            [#]_\n         \n        <problematic ids=\"id3\" \
            refid=\"id10\">\n            [#label]_\n         \n        <problematic ids=\"id8 \
            id4\" refid=\"id7\">\n            [*]_\n         end.\n",
        warnings: &[
            "<SRCDIR>/index.rst:1: ERROR: Too many autonumbered footnote references: only 0 \
                corresponding footnote available. [docutils]",
            "<SRCDIR>/index.rst:1: ERROR: Too many symbol footnote references: only 0 \
                corresponding footnotes available. [docutils]",
            "<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"1\". [docutils]",
            "<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"label\". [docutils]",
        ],
        trace: &[
            ("parse", "footnote_reference", "id1", false),
            ("parse", "footnote_reference", "id2", false),
            ("parse", "footnote_reference", "id3", false),
            ("parse", "footnote_reference", "id4", false),
            ("Footnotes", "system_message", "id5", false),
            ("Footnotes", "problematic", "id6", true),
            ("Footnotes", "system_message", "id7", false),
            ("Footnotes", "problematic", "id8", true),
            ("SphinxDanglingReferences", "system_message", "id9", false),
            ("SphinxDanglingReferences", "system_message", "id10", false),
        ],
    };

    const DOCUTILS_0575: SphinxRead = SphinxRead {
        rst: ".. |f| replace:: note [#]_\n",
        body: "    <system_message level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" \
            type=\"ERROR\">\n        <paragraph>\n            References to auto-numbered and \
            auto-symbol footnotes are not supported in a substitution definition.\n        \
            <literal_block xml:space=\"preserve\">\n            .. |f| replace:: note [#]_\n",
        warnings: &[
            "<SRCDIR>/index.rst:1: ERROR: References to auto-numbered and auto-symbol \
                footnotes are not supported in a substitution definition.",
            "",
            ".. |f| replace:: note [#]_ [docutils]",
            "<SRCDIR>/index.rst:1: ERROR: Too many autonumbered footnote references: only 0 \
                corresponding footnote available. [docutils]",
        ],
        trace: &[
            ("parse", "footnote_reference", "id1", false),
            ("Footnotes", "system_message", "id2", false),
            ("Footnotes", "problematic", "id3", false),
        ],
    };

    /// `Footnotes` (620-011, `SP/docutils/transforms/references.py:416-636`)
    /// numbers the auto-numbered footnotes in registration order with the
    /// first number no name holds yet (`label not in nameids`: a manual
    /// `[1]` or a target named `1` takes `1`, one IndirectHyperlinks, 460,
    /// resolved included, so binding the number never meets a duplicate
    /// name), gives each a `label` and the
    /// number as its name, and numbers each anonymous `[#]_` after them, a
    /// `[#name]_` taking its footnote's; symbol footnotes take `*`, `†`, …,
    /// doubled past the tenth; a manual footnote and its references resolve
    /// by name. Each reference takes the footnote's id as `refid` and its
    /// text, and the footnote the reference's id as a backref
    /// (`state.footnote_ref_kinds`). A `[#name]_` whose footnote is missing
    /// is still numbered in the anonymous loop, keeping its `refname`, and
    /// 850 then leaves it alone. Without footnotes every reference fails
    /// (`docutils-0236`): 620 reports the anonymous and symbol ones and 850
    /// the named ones. `FootnoteDocnameUpdater` (700) stamps `docname` on
    /// every footnote and reference left in the tree.
    #[test]
    fn footnotes_number_label_and_backref() {
        let rows = [
        SphinxRead {
            rst: "See [#n]_, [*]_ and [1]_.\n\n.. [#n] named auto\n.. [*] symbol\n.. [1] \
                manual\n",
            body: "    <paragraph>\n        See \n        <footnote_reference auto=\"1\" \
                docname=\"index\" ids=\"id1\" refid=\"n\">\n            2\n        , \n        \
                <footnote_reference auto=\"*\" docname=\"index\" ids=\"id2\" \
                refid=\"id4\">\n            *\n         and \n        <footnote_reference \
                docname=\"index\" ids=\"id3\" refid=\"id5\">\n            1\n        .\n    \
                <footnote auto=\"1\" backrefs=\"id1\" docname=\"index\" ids=\"n\" \
                names=\"n\">\n        <label>\n            2\n        <paragraph>\n            \
                named auto\n    <footnote auto=\"*\" backrefs=\"id2\" docname=\"index\" \
                ids=\"id4\">\n        <label>\n            *\n        <paragraph>\n            \
                symbol\n    <footnote backrefs=\"id3\" docname=\"index\" ids=\"id5\" \
                names=\"1\">\n        <label>\n            1\n        <paragraph>\n            \
                manual\n",
            warnings: &[],
            trace: &[
                ("parse", "footnote_reference", "id1", true),
                ("parse", "footnote_reference", "id2", true),
                ("parse", "footnote_reference", "id3", true),
                ("parse", "footnote", "n", true),
                ("parse", "footnote", "id4", true),
                ("parse", "footnote", "id5", true),
            ],
        },
        SphinxRead {
            rst: "See [#missing]_ and [#]_.\n\n.. [#] one\n",
            body: "    <paragraph>\n        See \n        <footnote_reference auto=\"1\" \
                docname=\"index\" ids=\"id1\" refid=\"id3\" refname=\"missing\">\n            \
                1\n         and \n        <problematic ids=\"id5 id2\" refid=\"id4\">\n            \
                [#]_\n        .\n    <footnote auto=\"1\" backrefs=\"id1\" docname=\"index\" \
                ids=\"id3\" names=\"1\">\n        <label>\n            1\n        \
                <paragraph>\n            one\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Too many autonumbered footnote references: only 1 \
                    corresponding footnote available. [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", true),
                ("parse", "footnote_reference", "id2", false),
                ("parse", "footnote", "id3", true),
                ("Footnotes", "system_message", "id4", false),
                ("Footnotes", "problematic", "id5", true),
            ],
        },
        SphinxRead {
            rst: "[#]_ text 1_\n\n.. _1: http://x\n\n.. [#] auto\n",
            body: "    <paragraph>\n        <footnote_reference auto=\"1\" docname=\"index\" \
                ids=\"id1\" refid=\"id3\">\n            2\n         text \n        <reference \
                name=\"1\" refuri=\"http://x\">\n            1\n    <target ids=\"id2\" \
                names=\"1\" refuri=\"http://x\">\n    <footnote auto=\"1\" backrefs=\"id1\" \
                docname=\"index\" ids=\"id3\" names=\"2\">\n        <label>\n            \
                2\n        <paragraph>\n            auto\n",
            warnings: &[],
            trace: &[
                ("parse", "footnote_reference", "id1", true),
                ("parse", "target", "id2", true),
                ("parse", "footnote", "id3", true),
            ],
        },
        SphinxRead {
            rst: "S0 [*]_ S1 [*]_ S2 [*]_ S3 [*]_ S4 [*]_ S5 [*]_ S6 [*]_ S7 [*]_ S8 [*]_ S9 \
                [*]_ S10 [*]_ S11 [*]_ \n\n.. [*] s0\n.. [*] s1\n.. [*] s2\n.. [*] s3\n.. [*] s4\n\
                .. [*] s5\n.. [*] s6\n.. [*] s7\n.. [*] s8\n.. [*] s9\n.. [*] s10\n.. [*] s11\n",
            body: "    <paragraph>\n        S0 \n        <footnote_reference auto=\"*\" \
                docname=\"index\" ids=\"id1\" refid=\"id13\">\n            *\n         \
                S1 \n        <footnote_reference auto=\"*\" docname=\"index\" ids=\"id2\" \
                refid=\"id14\">\n            †\n         S2 \n        <footnote_reference \
                auto=\"*\" docname=\"index\" ids=\"id3\" refid=\"id15\">\n            ‡\n         \
                S3 \n        <footnote_reference auto=\"*\" docname=\"index\" ids=\"id4\" \
                refid=\"id16\">\n            §\n         S4 \n        <footnote_reference \
                auto=\"*\" docname=\"index\" ids=\"id5\" refid=\"id17\">\n            ¶\n         \
                S5 \n        <footnote_reference auto=\"*\" docname=\"index\" ids=\"id6\" \
                refid=\"id18\">\n            #\n         S6 \n        <footnote_reference \
                auto=\"*\" docname=\"index\" ids=\"id7\" refid=\"id19\">\n            ♠\n         \
                S7 \n        <footnote_reference auto=\"*\" docname=\"index\" ids=\"id8\" \
                refid=\"id20\">\n            ♥\n         S8 \n        <footnote_reference \
                auto=\"*\" docname=\"index\" ids=\"id9\" refid=\"id21\">\n            ♦\n         \
                S9 \n        <footnote_reference auto=\"*\" docname=\"index\" ids=\"id10\" \
                refid=\"id22\">\n            ♣\n         S10 \n        <footnote_reference \
                auto=\"*\" docname=\"index\" ids=\"id11\" refid=\"id23\">\n            \
                **\n         S11 \n        <footnote_reference auto=\"*\" docname=\"index\" \
                ids=\"id12\" refid=\"id24\">\n            ††\n    <footnote auto=\"*\" \
                backrefs=\"id1\" docname=\"index\" ids=\"id13\">\n        <label>\n            \
                *\n        <paragraph>\n            s0\n    <footnote auto=\"*\" backrefs=\"id2\" \
                docname=\"index\" ids=\"id14\">\n        <label>\n            †\n        \
                <paragraph>\n            s1\n    <footnote auto=\"*\" backrefs=\"id3\" \
                docname=\"index\" ids=\"id15\">\n        <label>\n            ‡\n        \
                <paragraph>\n            s2\n    <footnote auto=\"*\" backrefs=\"id4\" \
                docname=\"index\" ids=\"id16\">\n        <label>\n            §\n        \
                <paragraph>\n            s3\n    <footnote auto=\"*\" backrefs=\"id5\" \
                docname=\"index\" ids=\"id17\">\n        <label>\n            ¶\n        \
                <paragraph>\n            s4\n    <footnote auto=\"*\" backrefs=\"id6\" \
                docname=\"index\" ids=\"id18\">\n        <label>\n            #\n        \
                <paragraph>\n            s5\n    <footnote auto=\"*\" backrefs=\"id7\" \
                docname=\"index\" ids=\"id19\">\n        <label>\n            ♠\n        \
                <paragraph>\n            s6\n    <footnote auto=\"*\" backrefs=\"id8\" \
                docname=\"index\" ids=\"id20\">\n        <label>\n            ♥\n        \
                <paragraph>\n            s7\n    <footnote auto=\"*\" backrefs=\"id9\" \
                docname=\"index\" ids=\"id21\">\n        <label>\n            ♦\n        \
                <paragraph>\n            s8\n    <footnote auto=\"*\" backrefs=\"id10\" \
                docname=\"index\" ids=\"id22\">\n        <label>\n            ♣\n        \
                <paragraph>\n            s9\n    <footnote auto=\"*\" backrefs=\"id11\" \
                docname=\"index\" ids=\"id23\">\n        <label>\n            **\n        \
                <paragraph>\n            s10\n    <footnote auto=\"*\" backrefs=\"id12\" \
                docname=\"index\" ids=\"id24\">\n        <label>\n            ††\n        \
                <paragraph>\n            s11\n",
            warnings: &[],
            trace: &[
                ("parse", "footnote_reference", "id1", true),
                ("parse", "footnote_reference", "id2", true),
                ("parse", "footnote_reference", "id3", true),
                ("parse", "footnote_reference", "id4", true),
                ("parse", "footnote_reference", "id5", true),
                ("parse", "footnote_reference", "id6", true),
                ("parse", "footnote_reference", "id7", true),
                ("parse", "footnote_reference", "id8", true),
                ("parse", "footnote_reference", "id9", true),
                ("parse", "footnote_reference", "id10", true),
                ("parse", "footnote_reference", "id11", true),
                ("parse", "footnote_reference", "id12", true),
                ("parse", "footnote", "id13", true),
                ("parse", "footnote", "id14", true),
                ("parse", "footnote", "id15", true),
                ("parse", "footnote", "id16", true),
                ("parse", "footnote", "id17", true),
                ("parse", "footnote", "id18", true),
                ("parse", "footnote", "id19", true),
                ("parse", "footnote", "id20", true),
                ("parse", "footnote", "id21", true),
                ("parse", "footnote", "id22", true),
                ("parse", "footnote", "id23", true),
                ("parse", "footnote", "id24", true),
            ],
        },
        SphinxRead {
            rst: "[#]_ and 1_\n\n.. _1: y_\n.. _y: http://x\n\n.. [#] auto\n",
            body: "    <paragraph>\n        <footnote_reference auto=\"1\" docname=\"index\" \
                ids=\"id1\" refid=\"id3\">\n            2\n         and \n        <reference \
                name=\"1\" refuri=\"http://x\">\n            1\n    <target ids=\"id2\" \
                names=\"1\" refuri=\"http://x\">\n    <target ids=\"y\" names=\"y\" \
                refuri=\"http://x\">\n    <footnote auto=\"1\" backrefs=\"id1\" docname=\"index\" \
                ids=\"id3\" names=\"2\">\n        <label>\n            2\n        \
                <paragraph>\n            auto\n",
            warnings: &[],
            trace: &[
                ("parse", "footnote_reference", "id1", true),
                ("parse", "target", "id2", true),
                ("parse", "target", "y", true),
                ("parse", "footnote", "id3", true),
            ],
        },
            DOCUTILS_0236,
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// The two problematic id shapes ([DS §5.5], `state.problematic_id_shapes`
    /// = `docutils-0236`): a problematic 220, 440, 460 or 620 makes takes an
    /// id of its own and then the ids of the node it replaces
    /// (`ids="<new> <old>"`: the `[#]_` and `[*]_` 620 replaces, a `[1]_`
    /// 460 replaces), while 850's reuses the reference's first id
    /// (`ids="<old>"`: the `[1]_` and `[#label]_` it replaces). A
    /// substitution or anonymous reference has no ids of its own, so 220's
    /// and 440's take only theirs. Each message takes the id before its
    /// problematic, and stays out of the tree.
    #[test]
    fn problematic_id_shapes() {
        let rows = [
            DOCUTILS_0236,
        SphinxRead {
            rst: "See [1]_.\n\n.. _1: missing_\n",
            body: "    <paragraph>\n        See \n        <problematic ids=\"id4 id1\" \
                refid=\"id3\">\n            [1]_\n        .\n    <target ids=\"id2\" names=\"1\" \
                refname=\"missing\">\n",
            warnings: &[
                "<SRCDIR>/index.rst:3: ERROR: Indirect hyperlink target \"1\" (id=\"id2\") refers \
                    to target \"missing\", which does not exist. [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", false),
                ("parse", "target", "id2", true),
                ("IndirectHyperlinks", "system_message", "id3", false),
                ("IndirectHyperlinks", "problematic", "id4", true),
            ],
        },
        SphinxRead {
            rst: "A |nope| B.\n",
            body: "    <paragraph>\n        A \n        <problematic ids=\"id2\" \
                refid=\"id1\">\n            |nope|\n         B.\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Undefined substitution referenced: \"nope\". \
                    [docutils]",
            ],
            trace: &[
                ("Substitutions", "system_message", "id1", false),
                ("Substitutions", "problematic", "id2", true),
            ],
        },
        SphinxRead {
            rst: "See `x`__ and [#]_.\n",
            body: "    <paragraph>\n        See \n        <problematic ids=\"id3\" \
                refid=\"id2\">\n            `x`__\n         and \n        <problematic ids=\"id5 \
                id1\" refid=\"id4\">\n            [#]_\n        .\n",
            warnings: &[
                "<SRCDIR>/index.rst:2: ERROR: Anonymous hyperlink mismatch: 1 references but 0 \
                    targets.",
                "See \"backrefs\" attribute for IDs. [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Too many autonumbered footnote references: only 0 \
                    corresponding footnote available. [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", false),
                ("AnonymousHyperlinks", "system_message", "id2", false),
                ("AnonymousHyperlinks", "problematic", "id3", true),
                ("Footnotes", "system_message", "id4", false),
                ("Footnotes", "problematic", "id5", true),
            ],
        },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// A footnote reference the parse registered inside a substitution
    /// definition it then rejected stays registered (Review Focus 5,
    /// `state.rejected_subst_autofootnote`, `docutils-0575`): 620 numbers it
    /// first, so the footnote's backref is its id and the reference in the
    /// text is the one too many, reported at its own line; with no footnote
    /// it is the one too many, reported at the rejected definition's line,
    /// and the problematic that replaces it, like its message, is loose.
    #[test]
    fn detached_footnote_refs_are_numbered() {
        let rows = [
        SphinxRead {
            rst: ".. |f| replace:: note [#]_\n\nText [#]_.\n\n.. [#] the footnote\n",
            body: "    <system_message level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" \
                type=\"ERROR\">\n        <paragraph>\n            References to auto-numbered and \
                auto-symbol footnotes are not supported in a substitution definition.\n        \
                <literal_block xml:space=\"preserve\">\n            .. |f| replace:: note \
                [#]_\n    <paragraph>\n        Text \n        <problematic ids=\"id5 id2\" \
                refid=\"id4\">\n            [#]_\n        .\n    <footnote auto=\"1\" \
                backrefs=\"id1\" docname=\"index\" ids=\"id3\" names=\"1\">\n        \
                <label>\n            1\n        <paragraph>\n            the footnote\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: References to auto-numbered and auto-symbol \
                    footnotes are not supported in a substitution definition.",
                "",
                ".. |f| replace:: note [#]_ [docutils]",
                "<SRCDIR>/index.rst:3: ERROR: Too many autonumbered footnote references: only 1 \
                    corresponding footnote available. [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", false),
                ("parse", "footnote_reference", "id2", false),
                ("parse", "footnote", "id3", true),
                ("Footnotes", "system_message", "id4", false),
                ("Footnotes", "problematic", "id5", true),
            ],
        },
            DOCUTILS_0575,
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// 620 numbers references in registration order, not tree order
    /// ([DS §4.3 (a)-(c)]): the doc fields of `state.docfields_footnote_order`
    /// regroup `:param b:` before `:returns:`, so the text reads `1 / 3 /
    /// 2`; `GlossarySorter` (500) sorts `zeta`'s reference after `alpha`'s
    /// (`state.glossary_sorted_footnote_order`), and it keeps number 1. In
    /// `state.docinfo_abstract_footnote_order` the abstract's reference
    /// keeps number 1 and the author's 2: DocInfo (340) puts the docinfo,
    /// which holds the author's, ahead of the abstract's topic, and the
    /// metadata collector (880) pops the docinfo, reference and all.
    #[test]
    fn footnote_numbering_follows_registration() {
        let rows = [
            SphinxRead {
                rst: ".. py:function:: f(a, b)\n\n   :param a: first [#]_\n   :returns: result \
                [#]_\n   :param b: second [#]_\n\n.. [#] one\n.. [#] two\n.. [#] three\n",
                body: "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ \
                '',\\ None)\">\n    <desc classes=\"py function\" desctype=\"function\" \
                domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" \
                no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" \
                objtype=\"function\">\n        <desc_signature _toc_name=\"f()\" \
                _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" \
                ids=\"f\" module=\"True\">\n            <desc_name classes=\"sig-name descname\" \
                xml:space=\"preserve\">\n                f\n            <desc_parameterlist \
                multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" \
                xml:space=\"preserve\">\n                <desc_parameter \
                xml:space=\"preserve\">\n                    <desc_sig_name \
                classes=\"n\">\n                        a\n                <desc_parameter \
                xml:space=\"preserve\">\n                    <desc_sig_name \
                classes=\"n\">\n                        b\n        <desc_content>\n            \
                <field_list>\n                <field>\n                    \
                <field_name>\n                        Parameters\n                    \
                <field_body>\n                        <bullet_list>\n                            \
                <list_item>\n                                \
                <paragraph>\n                                    \
                <literal_strong>\n                                        \
                a\n                                     -- \n                                    \
                first \n                                    <footnote_reference auto=\"1\" \
                docname=\"index\" ids=\"id1\" \
                refid=\"id4\">\n                                        \
                1\n                            <list_item>\n                                \
                <paragraph>\n                                    \
                <literal_strong>\n                                        \
                b\n                                     -- \n                                    \
                second \n                                    <footnote_reference auto=\"1\" \
                docname=\"index\" ids=\"id3\" \
                refid=\"id6\">\n                                        3\n                \
                <field>\n                    <field_name>\n                        \
                Returns\n                    <field_body>\n                        \
                <paragraph>\n                            result \n                            \
                <footnote_reference auto=\"1\" docname=\"index\" ids=\"id2\" \
                refid=\"id5\">\n                                2\n    <footnote auto=\"1\" \
                backrefs=\"id1\" docname=\"index\" ids=\"id4\" names=\"1\">\n        \
                <label>\n            1\n        <paragraph>\n            one\n    <footnote \
                auto=\"1\" backrefs=\"id2\" docname=\"index\" ids=\"id5\" names=\"2\">\n        \
                <label>\n            2\n        <paragraph>\n            two\n    <footnote \
                auto=\"1\" backrefs=\"id3\" docname=\"index\" ids=\"id6\" names=\"3\">\n        \
                <label>\n            3\n        <paragraph>\n            three\n",
                warnings: &[],
                trace: &[
                    ("parse", "footnote_reference", "id1", true),
                    ("parse", "footnote_reference", "id2", true),
                    ("parse", "footnote_reference", "id3", true),
                    ("parse", "footnote", "id4", true),
                    ("parse", "footnote", "id5", true),
                    ("parse", "footnote", "id6", true),
                ],
            },
            SphinxRead {
                rst: ".. glossary::\n   :sorted:\n\n   zeta\n      Last letter [#]_.\n\n   \
                alpha\n      First letter [#]_.\n\n.. [#] z note\n.. [#] a note\n",
                body: "    <glossary sorted=\"1\">\n        <definition_list \
                classes=\"glossary\">\n            <definition_list_item>\n                <term \
                ids=\"term-alpha\">\n                    alpha\n                    <index \
                entries=\"('single',\\ 'alpha',\\ 'term-alpha',\\ 'main',\\ \
                None)\">\n                <definition>\n                    \
                <paragraph>\n                        First letter \n                        \
                <footnote_reference auto=\"1\" docname=\"index\" ids=\"id2\" \
                refid=\"id4\">\n                            2\n                        \
                .\n            <definition_list_item>\n                <term \
                ids=\"term-zeta\">\n                    zeta\n                    <index \
                entries=\"('single',\\ 'zeta',\\ 'term-zeta',\\ 'main',\\ \
                None)\">\n                <definition>\n                    \
                <paragraph>\n                        Last letter \n                        \
                <footnote_reference auto=\"1\" docname=\"index\" ids=\"id1\" \
                refid=\"id3\">\n                            1\n                        .\n    \
                <footnote auto=\"1\" backrefs=\"id1\" docname=\"index\" ids=\"id3\" \
                names=\"1\">\n        <label>\n            1\n        <paragraph>\n            z \
                note\n    <footnote auto=\"1\" backrefs=\"id2\" docname=\"index\" ids=\"id4\" \
                names=\"2\">\n        <label>\n            2\n        <paragraph>\n            a \
                note\n",
                warnings: &[],
                trace: &[
                    ("parse", "footnote_reference", "id1", true),
                    ("parse", "footnote_reference", "id2", true),
                    ("parse", "footnote", "id3", true),
                    ("parse", "footnote", "id4", true),
                ],
            },
            SphinxRead {
                rst: ":Abstract: abstract text [#]_\n:Author: someone [#]_\n\nBody.\n\n\
                .. [#] abstract note\n.. [#] author note\n",
                body: "    <topic classes=\"abstract\">\n        <title>\n            Abstract\n        \
                <paragraph>\n            abstract text \n            <footnote_reference \
                auto=\"1\" docname=\"index\" ids=\"id1\" refid=\"id3\">\n                1\n    \
                <paragraph>\n        Body.\n    <footnote auto=\"1\" backrefs=\"id1\" \
                docname=\"index\" ids=\"id3\" names=\"1\">\n        <label>\n            1\n        \
                <paragraph>\n            abstract note\n    <footnote auto=\"1\" backrefs=\"id2\" \
                docname=\"index\" ids=\"id4\" names=\"2\">\n        <label>\n            2\n        \
                <paragraph>\n            author note\n",
                warnings: &[],
                trace: &[
                    ("parse", "footnote_reference", "id1", true),
                    ("parse", "footnote_reference", "id2", false),
                    ("parse", "footnote", "id3", true),
                    ("parse", "footnote", "id4", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// The references a rejected `table` or `list-table` parsed stay
    /// registered (`state.rejected_table_refs`, `state.rejected_list_table_refs`,
    /// [DS §4.3 (d)]): they take the footnotes first, so each footnote's
    /// backref names an id no element in the tree carries, and the
    /// reference in the text is the one too many.
    #[test]
    fn rejected_table_refs_keep_backrefs() {
        let rows = [
        SphinxRead {
            rst: ".. table:: Cap [#]_\n\n   not a table [#]_\n\nText [#]_.\n\n.. [#] a\n.. [#] \
                b\n",
            body: "    <system_message level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" \
                type=\"ERROR\">\n        <paragraph>\n            Error parsing content block for \
                the \"table\" directive: exactly one table expected.\n        <literal_block \
                xml:space=\"preserve\">\n            .. table:: Cap \
                [#]_\n            \n               not a table [#]_\n    <paragraph>\n        \
                Text \n        <problematic ids=\"id7 id3\" refid=\"id6\">\n            \
                [#]_\n        .\n    <footnote auto=\"1\" backrefs=\"id1\" docname=\"index\" \
                ids=\"id4\" names=\"1\">\n        <label>\n            1\n        \
                <paragraph>\n            a\n    <footnote auto=\"1\" backrefs=\"id2\" \
                docname=\"index\" ids=\"id5\" names=\"2\">\n        <label>\n            \
                2\n        <paragraph>\n            b\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Error parsing content block for the \"table\" \
                    directive: exactly one table expected.",
                "",
                ".. table:: Cap [#]_",
                "",
                "   not a table [#]_ [docutils]",
                "<SRCDIR>/index.rst:5: ERROR: Too many autonumbered footnote references: only 2 \
                    corresponding footnotes available. [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", false),
                ("parse", "footnote_reference", "id2", false),
                ("parse", "footnote_reference", "id3", false),
                ("parse", "footnote", "id4", true),
                ("parse", "footnote", "id5", true),
                ("Footnotes", "system_message", "id6", false),
                ("Footnotes", "problematic", "id7", true),
            ],
        },
        SphinxRead {
            rst: ".. list-table::\n\n   not a list [#]_\n\nText [#]_.\n\n.. [#] a\n",
            body: "    <system_message level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" \
                type=\"ERROR\">\n        <paragraph>\n            Error parsing content block for \
                the \"list-table\" directive: exactly one bullet list expected.\n        \
                <literal_block xml:space=\"preserve\">\n            .. \
                list-table::\n            \n               not a list [#]_\n    \
                <paragraph>\n        Text \n        <problematic ids=\"id5 id2\" \
                refid=\"id4\">\n            [#]_\n        .\n    <footnote auto=\"1\" \
                backrefs=\"id1\" docname=\"index\" ids=\"id3\" names=\"1\">\n        \
                <label>\n            1\n        <paragraph>\n            a\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Error parsing content block for the \"list-table\" \
                    directive: exactly one bullet list expected.",
                "",
                ".. list-table::",
                "",
                "   not a list [#]_ [docutils]",
                "<SRCDIR>/index.rst:5: ERROR: Too many autonumbered footnote references: only 1 \
                    corresponding footnote available. [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", false),
                ("parse", "footnote_reference", "id2", false),
                ("parse", "footnote", "id3", true),
                ("Footnotes", "system_message", "id4", false),
                ("Footnotes", "problematic", "id5", true),
            ],
        },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// The `Too many autonumbered/symbol footnote references` family
    /// ([TW §6.3]: 4 records in `docutils-0236`, `-0237` and `-0575`). The
    /// auto-numbered message says `footnote` for 0 or 1 and `footnotes`
    /// for more; the symbol message always says `footnotes`. Every
    /// reference from the first one too many on becomes a problematic
    /// pointing at the one message, but a `[#name]_`, which stays for 850
    /// (`docutils-0237`), and the symbol loop's references, which 620 never
    /// marks resolved.
    #[test]
    fn too_many_footnote_references() {
        let rows = [
            DOCUTILS_0236,
        SphinxRead {
            rst: "Ref [#Label]_ end.\n",
            body: "    <paragraph>\n        Ref \n        <problematic ids=\"id1\" \
                refid=\"id3\">\n            [#Label]_\n         end.\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Too many autonumbered footnote references: only 0 \
                    corresponding footnote available. [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"label\". [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", false),
                ("Footnotes", "system_message", "id2", false),
                ("SphinxDanglingReferences", "system_message", "id3", false),
            ],
        },
            DOCUTILS_0575,
        SphinxRead {
            rst: "A [#]_ B [#]_ C [#]_ D [#x]_.\n\n.. [#] a\n.. [#] b\n",
            body: "    <paragraph>\n        A \n        <footnote_reference auto=\"1\" \
                docname=\"index\" ids=\"id1\" refid=\"id5\">\n            1\n         B \n        \
                <footnote_reference auto=\"1\" docname=\"index\" ids=\"id2\" \
                refid=\"id6\">\n            2\n         C \n        <problematic ids=\"id8 id3\" \
                refid=\"id7\">\n            [#]_\n         D \n        <problematic ids=\"id4\" \
                refid=\"id9\">\n            [#x]_\n        .\n    <footnote auto=\"1\" \
                backrefs=\"id1\" docname=\"index\" ids=\"id5\" names=\"1\">\n        \
                <label>\n            1\n        <paragraph>\n            a\n    <footnote \
                auto=\"1\" backrefs=\"id2\" docname=\"index\" ids=\"id6\" names=\"2\">\n        \
                <label>\n            2\n        <paragraph>\n            b\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Too many autonumbered footnote references: only 2 \
                    corresponding footnotes available. [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"x\". [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", true),
                ("parse", "footnote_reference", "id2", true),
                ("parse", "footnote_reference", "id3", false),
                ("parse", "footnote_reference", "id4", false),
                ("parse", "footnote", "id5", true),
                ("parse", "footnote", "id6", true),
                ("Footnotes", "system_message", "id7", false),
                ("Footnotes", "problematic", "id8", true),
                ("SphinxDanglingReferences", "system_message", "id9", false),
            ],
        },
        SphinxRead {
            rst: "S [*]_ T [*]_ U [*]_.\n\n.. [*] s\n",
            body: "    <paragraph>\n        S \n        <footnote_reference auto=\"*\" \
                docname=\"index\" ids=\"id1\" refid=\"id4\">\n            *\n         T \n        \
                <problematic ids=\"id6 id2\" refid=\"id5\">\n            [*]_\n         \
                U \n        <problematic ids=\"id7 id3\" refid=\"id5\">\n            [*]_\n        \
                .\n    <footnote auto=\"*\" backrefs=\"id1\" docname=\"index\" \
                ids=\"id4\">\n        <label>\n            *\n        <paragraph>\n            s\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Too many symbol footnote references: only 1 \
                    corresponding footnotes available. [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", true),
                ("parse", "footnote_reference", "id2", false),
                ("parse", "footnote_reference", "id3", false),
                ("parse", "footnote", "id4", true),
                ("Footnotes", "system_message", "id5", false),
                ("Footnotes", "problematic", "id6", true),
                ("Footnotes", "problematic", "id7", true),
            ],
        },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// `UnreferencedFootnotesDetector` (622-028,
    /// `SP/sphinx/transforms/__init__.py:288-324`) warns about every
    /// footnote without a backref, list by list: the manual footnotes, the
    /// symbol ones, then the auto-numbered ones, each in registration order
    /// (`state.unreferenced_order`), at the footnote's own location, as a
    /// `[ref.footnote]` logger record after the reporter records of the
    /// transforms before it (`state.final_position.footnote`). A footnote
    /// whose name a duplicate moved to `dupnames` is left to docutils'
    /// duplicate warning.
    #[test]
    fn unreferenced_footnotes_warn_in_order() {
        let rows = [
        SphinxRead {
            rst: ".. [#] auto one\n.. [*] sym\n.. [1] manual\n.. [#b] auto labeled\n",
            body: "    <footnote auto=\"1\" docname=\"index\" ids=\"id1\" \
                names=\"2\">\n        <label>\n            2\n        <paragraph>\n            \
                auto one\n    <footnote auto=\"*\" docname=\"index\" ids=\"id2\">\n        \
                <label>\n            *\n        <paragraph>\n            sym\n    <footnote \
                docname=\"index\" ids=\"id3\" names=\"1\">\n        <label>\n            \
                1\n        <paragraph>\n            manual\n    <footnote auto=\"1\" \
                docname=\"index\" ids=\"b\" names=\"b\">\n        <label>\n            3\n        \
                <paragraph>\n            auto labeled\n",
            warnings: &[
                "<SRCDIR>/index.rst:3: WARNING: Footnote [1] is not referenced. [ref.footnote]",
                "<SRCDIR>/index.rst:2: WARNING: Footnote [*] is not referenced. [ref.footnote]",
                "<SRCDIR>/index.rst:1: WARNING: Footnote [#] is not referenced. [ref.footnote]",
                "<SRCDIR>/index.rst:4: WARNING: Footnote [#] is not referenced. [ref.footnote]",
            ],
            trace: &[
                ("parse", "footnote", "id1", true),
                ("parse", "footnote", "id2", true),
                ("parse", "footnote", "id3", true),
                ("parse", "footnote", "b", true),
            ],
        },
        SphinxRead {
            rst: "One [#]_.\n\n.. [#] a\n.. [#] b\n",
            body: "    <paragraph>\n        One \n        <footnote_reference auto=\"1\" \
                docname=\"index\" ids=\"id1\" refid=\"id2\">\n            1\n        .\n    \
                <footnote auto=\"1\" backrefs=\"id1\" docname=\"index\" ids=\"id2\" \
                names=\"1\">\n        <label>\n            1\n        <paragraph>\n            \
                a\n    <footnote auto=\"1\" docname=\"index\" ids=\"id3\" names=\"2\">\n        \
                <label>\n            2\n        <paragraph>\n            b\n",
            warnings: &[
                "<SRCDIR>/index.rst:4: WARNING: Footnote [#] is not referenced. [ref.footnote]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", true),
                ("parse", "footnote", "id2", true),
                ("parse", "footnote", "id3", true),
            ],
        },
        SphinxRead {
            rst: "Para `x`__.\n\n.. [1] fn\n\n",
            body: "    <paragraph>\n        Para \n        <problematic ids=\"id3\" \
                refid=\"id2\">\n            `x`__\n        .\n    <footnote docname=\"index\" \
                ids=\"id1\" names=\"1\">\n        <label>\n            1\n        \
                <paragraph>\n            fn\n",
            warnings: &[
                "<SRCDIR>/index.rst:: ERROR: Anonymous hyperlink mismatch: 1 references but 0 \
                    targets.",
                "See \"backrefs\" attribute for IDs. [docutils]",
                "<SRCDIR>/index.rst:3: WARNING: Footnote [1] is not referenced. [ref.footnote]",
            ],
            trace: &[
                ("parse", "footnote", "id1", true),
                ("AnonymousHyperlinks", "system_message", "id2", false),
                ("AnonymousHyperlinks", "problematic", "id3", true),
            ],
        },
        SphinxRead {
            rst: "See [1]_.\n\n.. [1] a\n.. [1] b\n.. [#d] c\n.. [#d] d\n",
            body: "    <paragraph>\n        See \n        <problematic ids=\"id1\" \
                refid=\"id5\">\n            [1]_\n        .\n    <footnote docname=\"index\" \
                dupnames=\"1\" ids=\"id2\">\n        <label>\n            1\n        \
                <paragraph>\n            a\n    <footnote docname=\"index\" dupnames=\"1\" \
                ids=\"id3\">\n        <label>\n            1\n        <system_message \
                backrefs=\"id3\" level=\"2\" line=\"4\" source=\"<SRCDIR>/index.rst\" \
                type=\"WARNING\">\n            <paragraph>\n                Duplicate explicit \
                target name: \"1\".\n        <paragraph>\n            b\n    <footnote auto=\"1\" \
                docname=\"index\" dupnames=\"d\" ids=\"d\">\n        <label>\n            \
                2\n        <paragraph>\n            c\n    <footnote auto=\"1\" docname=\"index\" \
                dupnames=\"d\" ids=\"id4\">\n        <label>\n            3\n        \
                <system_message backrefs=\"id4\" level=\"2\" line=\"6\" source=\"\
                <SRCDIR>/index.rst\" type=\"WARNING\">\n            <paragraph>\n                \
                Duplicate explicit target name: \"d\".\n        <paragraph>\n            d\n",
            warnings: &[
                "<SRCDIR>/index.rst:4: WARNING: Duplicate explicit target name: \"1\". [docutils]",
                "<SRCDIR>/index.rst:6: WARNING: Duplicate explicit target name: \"d\". [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Duplicate target name, cannot be used as a unique \
                    reference: \"1\". [docutils]",
            ],
            trace: &[
                ("parse", "footnote_reference", "id1", false),
                ("parse", "footnote", "id2", true),
                ("parse", "footnote", "id3", true),
                ("parse", "footnote", "d", true),
                ("parse", "footnote", "id4", true),
                ("SphinxDanglingReferences", "system_message", "id5", false),
            ],
        },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// `CitationReferenceTransform` (619-017,
    /// `SP/sphinx/domains/citation.py:155-175`) replaces every citation
    /// reference in the tree by a `pending_xref` into the citation domain
    /// that keeps its ids, and `CitationDefinitionTransform` (619-016) stamps
    /// each citation's `docname` and marks its label not smartquoted. The
    /// citation's backrefs come from the replaced references, which
    /// `citation_refs` still holds ([DS §4.3 (g)], `state.citation_backrefs`).
    /// What the citation domain registers in stage M (T13b) is recorded in
    /// tree order: each citation's label, ids and node, and each
    /// reference's target.
    #[test]
    fn citation_references_become_pending_xref() {
        let rows = [
            SphinxRead {
                rst: "Cite [CIT]_ and [CIT]_.\n\n.. [CIT] The citation.\n",
                body: "    <paragraph>\n        Cite \n        <pending_xref ids=\"id1\" \
                refdomain=\"citation\" reftarget=\"CIT\" reftype=\"ref\" refwarn=\"1\" \
                support_smartquotes=\"0\">\n            <inline>\n                [CIT]\n         \
                and \n        <pending_xref ids=\"id2\" refdomain=\"citation\" reftarget=\"CIT\" \
                reftype=\"ref\" refwarn=\"1\" support_smartquotes=\"0\">\n            \
                <inline>\n                [CIT]\n        .\n    <citation backrefs=\"id1 id2\" \
                docname=\"index\" ids=\"cit\" names=\"cit\">\n        <label \
                support_smartquotes=\"0\">\n            CIT\n        <paragraph>\n            The \
                citation.\n",
                warnings: &[],
                trace: &[
                    ("parse", "citation_reference", "id1", false),
                    ("parse", "citation_reference", "id2", false),
                    ("parse", "citation", "cit", true),
                ],
            },
            SphinxRead {
                rst: "Cite [cite2020]_ end.\n",
                body: "    <paragraph>\n        Cite \n        <pending_xref ids=\"id1\" \
                refdomain=\"citation\" reftarget=\"cite2020\" reftype=\"ref\" refwarn=\"1\" \
                support_smartquotes=\"0\">\n            <inline>\n                \
                [cite2020]\n         end.\n",
                warnings: &[],
                trace: &[("parse", "citation_reference", "id1", false)],
            },
            SphinxRead {
                rst: ".. [CIT2020] A citation body.\n",
                body:
                    "    <citation docname=\"index\" ids=\"cit2020\" names=\"cit2020\">\n        \
                <label support_smartquotes=\"0\">\n            CIT2020\n        \
                <paragraph>\n            A citation body.\n",
                warnings: &[],
                trace: &[("parse", "citation", "cit2020", true)],
            },
            SphinxRead {
                rst: "+-------------+\n| See [C1]_.  |\n+-------------+\n\n.. [C1] c\n",
                body: "    <table>\n        <tgroup cols=\"1\">\n            <colspec \
                colwidth=\"13\">\n            <tbody>\n                <row>\n                    \
                <entry>\n                        <paragraph>\n                            \
                See \n                            <pending_xref ids=\"id1\" refdomain=\"citation\" \
                reftarget=\"C1\" reftype=\"ref\" refwarn=\"1\" \
                support_smartquotes=\"0\">\n                                \
                <inline>\n                                    [C1]\n                            \
                .\n    <citation backrefs=\"id1\" docname=\"index\" ids=\"c1\" \
                names=\"c1\">\n        <label support_smartquotes=\"0\">\n            C1\n        \
                <paragraph>\n            c\n",
                warnings: &[],
                trace: &[
                    ("parse", "citation_reference", "id1", false),
                    ("parse", "citation", "c1", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));

        let read = crate::read::read_standalone(
            "Cite [CIT]_ and [CIT]_.\n\n.. [CIT] The citation.\n\n.. [Other] Two.\n",
            &crate::transforms::test_support::index_rst(),
            &crate::read::ReadSettings::sphinx_defaults(),
        );
        let state = read.outcome.state();
        let notes: Vec<(&str, &[String])> = state
            .citation_notes
            .iter()
            .map(|note| (note.label.as_str(), note.ids.as_slice()))
            .collect();
        assert_eq!(
            notes,
            [
                ("CIT", &["cit".to_string()][..]),
                ("Other", &["other".to_string()][..])
            ]
        );
        assert_eq!(state.citation_reference_targets, ["CIT", "CIT"]);
    }
}
