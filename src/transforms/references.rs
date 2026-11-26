//! docutils' substitution and hyperlink-target transforms
//! (`SP/docutils/transforms/references.py`) and Sphinx's
//! `SphinxDanglingReferences` (`SP/sphinx/transforms/references.py:18-30`),
//! ported one row of the read-transform table at a time
//! ([`super::READ_TRANSFORMS`]).
//!
//! Substitutions (220) first puts a copy of each substitution's
//! definition in place of every reference to it. The target transforms
//! then resolve the document's references in four passes over its targets —
//! internal targets propagated to the node they label (260), anonymous
//! references paired with anonymous targets (440), indirect targets
//! followed to what they refer to (460), then the references to external
//! (640) and internal (660) targets — and report every reference still
//! dangling (850). In between, a `.. target-notes::` turns the external
//! targets referred to into footnotes (540). A reference, substitution reference or target they
//! cannot resolve becomes a `problematic` holding its rawsource, which
//! points at the message; the message itself is written to the stream and
//! never placed in the tree, though its id is taken for good
//! ([DS §5.2-5.5]). The one message placed in the tree is the one about a
//! circular substitution definition, which takes the definition's place.
//!
//! The registries they read are the parse's, by node identity, so a node
//! the parse detached still resolves, errs and is replaced
//! ([`super::state::Registries`]). `document.ids` is [`TxTree::by_id`],
//! which knows no node for an id the parse registered on a node it then
//! dropped (a `:name:`d image in a rejected table); docutils' map still
//! holds that node, so a name bound to such an id is a target like any
//! other, an element holding the id. What else the dropped node carried is
//! gone here: an external target dropped that way would give an indirect
//! target its `refuri`, where here it gives its id.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use super::state::NodeView;
use super::tree::{Findall, TxId, TxNode, TxTree};
use super::{Transform, TransformCx};
use crate::doctree::messages::{self, ERROR, INFO};
use crate::doctree::pending::PyValue;
use crate::doctree::{is_text_element, kinds, unescape, AttrValue};
use crate::rst::{LogLocation, SphinxCrash};
use crate::utils::{py_isspace, py_repr_str};

/// `isinstance(node, nodes.Invisible)`: docutils' `comment`,
/// `substitution_definition`, `target` and `pending`, and Sphinx's `index`
/// (`SP/sphinx/addnodes.py:428`).
fn is_invisible(kind: &str) -> bool {
    matches!(
        kind,
        kinds::COMMENT | "substitution_definition" | kinds::TARGET | "pending" | "index"
    )
}

/// `isinstance(node, nodes.Targetable)`: `footnote`, `citation`, `target`.
fn is_targetable(kind: &str) -> bool {
    matches!(kind, kinds::FOOTNOTE | kinds::CITATION | kinds::TARGET)
}

fn is_target(node: &TxNode) -> bool {
    node.kind == kinds::TARGET
}

/// `isinstance(node, nodes.reference)`: docutils' `reference` and its
/// Sphinx subclasses `number_reference` and `download_reference`
/// (`SP/sphinx/addnodes.py:551-559`).
pub(super) fn is_reference(kind: &str) -> bool {
    matches!(
        kind,
        kinds::REFERENCE | kinds::DOWNLOAD_REFERENCE | "number_reference"
    )
}

/// Whether the node has the attribute `key` (`node.hasattr(key)`).
fn has(node: &TxNode, key: &'static str) -> bool {
    node.get(key).is_some()
}

/// `document.refnames.get(name, [])`: the nodes the parse noted under the
/// refname `name`, in registration order, wherever they are now.
fn refnames(cx: &TransformCx<'_>, name: &str) -> Vec<TxId> {
    cx.state
        .registries
        .refnames
        .get(name)
        .unwrap_or_default()
        .iter()
        .filter_map(|&uid| cx.tree.by_uid(uid))
        .collect()
}

/// `document.refids.get(id, [])`.
fn refids(tree: &TxTree, id: &str) -> Vec<TxId> {
    tree.refids()
        .get(id)
        .map(<[TxId]>::to_vec)
        .unwrap_or_default()
}

/// `nodes.problematic(rawsource, rawsource, refid=msgid)`: a problematic
/// holding the text `rawsource` (none when it is empty, as
/// `TextElement` makes no Text of `''`), pointing at the message `msgid`.
pub(super) fn problematic(tree: &mut TxTree, rawsource: &str, msgid: &str) -> TxId {
    let prb = tree.new_node(kinds::PROBLEMATIC);
    let node = tree.node_mut(prb);
    node.set("refid", AttrValue::Str(msgid.to_string()));
    node.set_rawsource(rawsource);
    if !rawsource.is_empty() {
        let text = tree.new_node(kinds::TEXT);
        tree.node_mut(text).text = Some(rawsource.to_string());
        tree.append(prb, text);
    }
    prb
}

/// The allocating problematic shape of Substitutions, AnonymousHyperlinks,
/// IndirectHyperlinks and Footnotes ([DS §5.5]): the problematic for
/// `reference` takes an id of its own (`prbid = set_id(prb)`), the message
/// `message` a backref to it, and it replaces the reference, whose ids it
/// takes after its own.
pub(super) fn replace_with_problematic(
    cx: &mut TransformCx<'_>,
    reference: TxId,
    message: TxId,
    msgid: &str,
) {
    let rawsource = cx
        .tree
        .node(reference)
        .rawsource()
        .unwrap_or_default()
        .to_string();
    let prb = problematic(cx.tree, &rawsource, msgid);
    let prbid = cx.set_id(prb);
    cx.tree.node_mut(message).attrs.backrefs.push(prbid);
    cx.tree.replace_self(reference, vec![prb]);
}

/// 220-004 `Substitutions` (`SP/docutils/transforms/references.py:642-765`):
/// each substitution reference of the tree, in a snapshot taken before the
/// loop, and then each reference a copy brings in, is replaced by a deep
/// copy of its definition's children ([`substitute`]). Nothing registers
/// the copies but the transform itself: each of their top-level
/// references that has a `refname` is noted under it (`note_refname`),
/// after the definition's own, which the parse noted
/// ([`TxTree::deepcopy`]).
pub struct Substitutions;

/// docutils' `line_length_limit` setting (`SP/docutils/frontend.py`, default
/// 10000), which Sphinx leaves as it is: the longest definition text a
/// reference may copy.
const LINE_LENGTH_LIMIT: usize = 10_000;

/// What one Substitutions run keeps across its references.
#[derive(Default)]
struct SubstitutionRun {
    /// `subreflist`: the references to substitute, the tree's and then
    /// those the copies bring in, in the order they are found.
    references: Vec<TxId>,
    /// `nested`: per substitution, the definitions whose copies referred
    /// to it. A definition among its own is one that refers back to
    /// itself, directly or through another.
    nested: HashMap<String, Vec<String>>,
    /// `nested_ref['ref-origin']`: the reference whose copy brought each
    /// nested reference in, which a circular one is reported at.
    origin: HashMap<TxId, TxId>,
}

impl Transform for Substitutions {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        let mut run = SubstitutionRun {
            references: cx
                .tree
                .findall(root, |node| node.kind == kinds::SUBSTITUTION_REFERENCE),
            ..SubstitutionRun::default()
        };
        let mut next = 0;
        while let Some(&reference) = run.references.get(next) {
            next += 1;
            substitute(cx, reference, &mut run);
            if cx.crashed() {
                return;
            }
        }
    }
}

/// One turn of Substitutions' loop, for `reference`.
///
/// The definition is the one the reference names (`document.substitution_defs`,
/// the last of that name), or the one its name lowercased normalizes to
/// (`substitution_names`). An undefined name, or a definition whose text
/// passes [`LINE_LENGTH_LIMIT`] (reported with no node, at the parse's
/// final position), is an ERROR, and the reference becomes a problematic
/// ([`replace_with_problematic`]). Otherwise `ltrim` and `rtrim` strip the
/// Text before and after the reference ([`strip_text`]), the definition is
/// deep-copied, and each reference in the copy is noted for the loop
/// ([`SubstitutionRun`]), until one names a definition whose copies it was
/// found in, a circular definition ([`circular_definition`]). The copy's
/// children then take the reference's place, and its top-level references
/// with a `refname` are noted (`note_refname`).
///
/// A reference in the copy that names no definition crashes Sphinx 9.1.0
/// (decision 13): `normed[nested_ref['refname'].lower()]` raises
/// `KeyError` while the copy is made, at `reference`.
fn substitute(cx: &mut TransformCx<'_>, reference: TxId, run: &mut SubstitutionRun) {
    let refname = cx
        .tree
        .node(reference)
        .str_attr("refname")
        .unwrap_or_default()
        .to_string();
    let registries = &cx.state.registries;
    let key = if registries.has_substitution_def(&refname) {
        Some(refname.clone())
    } else {
        registries
            .substitution_names
            .get(&refname.to_lowercase())
            .cloned()
    };
    let definition = key.as_deref().and_then(|key| substitution_def(cx, key));
    let message = match (&key, definition) {
        (Some(key), Some(definition)) => {
            let length = cx.tree.astext(definition).chars().count();
            (length > LINE_LENGTH_LIMIT).then(|| {
                let text =
                    format!("Substitution definition \"{key}\" exceeds the line-length-limit.");
                cx.system_message(ERROR, &text, None, Vec::new())
            })
        }
        _ => {
            let text = format!("Undefined substitution referenced: \"{refname}\".");
            Some(cx.system_message(ERROR, &text, Some(reference), Vec::new()))
        }
    };
    let (Some(key), Some(definition), None) = (key, definition, message) else {
        if let Some(message) = message {
            let msgid = cx.set_id(message);
            replace_with_problematic(cx, reference, message, &msgid);
        }
        return;
    };

    if let Some(parent) = cx.tree.parent(reference) {
        let siblings = &cx.tree.node(parent).children;
        if let Some(index) = siblings.iter().position(|&child| child == reference) {
            let has = |attribute| cx.tree.node(definition).get(attribute).is_some();
            let (ltrim, rtrim, trim) = (has("ltrim"), has("rtrim"), has("trim"));
            let before = index.checked_sub(1).map(|at| siblings[at]);
            let after = siblings.get(index + 1).copied();
            if ltrim || trim {
                if let Some(text) = before.filter(|&node| cx.tree.node(node).kind == kinds::TEXT) {
                    strip_text(cx.tree, text, |data| data.trim_end_matches(py_isspace));
                }
            }
            if rtrim || trim {
                if let Some(text) = after.filter(|&node| cx.tree.node(node).kind == kinds::TEXT) {
                    strip_text(cx.tree, text, |data| data.trim_start_matches(py_isspace));
                }
            }
        }
    }

    let copy = cx.tree.deepcopy(definition, cx.state);
    let is_reference = |node: &TxNode| node.kind == kinds::SUBSTITUTION_REFERENCE;
    for nested in cx.tree.findall(copy, is_reference) {
        let normed = cx
            .tree
            .node(nested)
            .str_attr("refname")
            .unwrap_or_default()
            .to_lowercase();
        let Some(nested_name) = cx.state.registries.substitution_names.get(&normed).cloned() else {
            let crash = SphinxCrash::python("KeyError", py_repr_str(&normed), LogLocation::None);
            cx.crash(crash, reference);
            return;
        };
        // `nested_name in nested.setdefault(nested_name, [])`: a copy of
        // the definition `nested_name` itself referred to it before.
        let referrers = run.nested.entry(nested_name.clone()).or_default();
        if referrers.contains(&nested_name) {
            circular_definition(cx, reference, &refname, run);
            return;
        }
        referrers.push(key.clone());
        run.origin.insert(nested, reference);
        run.references.push(nested);
    }
    let children = cx.tree.node(copy).children.clone();
    cx.tree.replace_self(reference, children.clone());
    for child in children {
        let node = cx.tree.node(child);
        let referential = matches!(
            node.kind,
            kinds::REFERENCE
                | kinds::FOOTNOTE_REFERENCE
                | kinds::CITATION_REFERENCE
                | "download_reference"
                | "number_reference"
        );
        if let Some(refname) = node.str_attr("refname").filter(|_| referential) {
            let refname = refname.to_string();
            let uid = cx.tree.assign_uid(child, cx.state);
            cx.state.registries.refnames.push(&refname, uid);
        }
    }
}

/// `document.substitution_defs[key]`: the last definition registered
/// under the name `key`, wherever it is now.
fn substitution_def(cx: &TransformCx<'_>, key: &str) -> Option<TxId> {
    let (_, uid) = cx
        .state
        .registries
        .substitution_defs
        .iter()
        .rev()
        .find(|(name, _)| name == key)?;
    cx.tree.by_uid(*uid)
}

/// The `except CircularSubstitutionDefinitionError` branch of
/// Substitutions: a reference that stands in a substitution definition
/// reports the definition (`Circular substitution definition detected:`,
/// with its block, at its location), and the message takes its place and
/// its names, with no id; any other reports the reference the chain of
/// copies started from (`Circular substitution definition referenced`)
/// and becomes a problematic.
///
/// A definition with no place to give the message crashes Sphinx 9.1.0 in
/// `replace_self`, once the message is logged (decision 13): one a message
/// took the place of already is looked up among its former parent's
/// children (`ValueError`, naming it as docutils' `repr` does,
/// [`element_repr`]); a copy a circular reference stopped before it was
/// placed, whose nested references the loop still reaches, has no parent
/// (`AttributeError`).
fn circular_definition(
    cx: &mut TransformCx<'_>,
    reference: TxId,
    refname: &str,
    run: &SubstitutionRun,
) {
    let parent = cx.tree.parent(reference);
    if let Some(definition) =
        parent.filter(|&parent| cx.tree.node(parent).kind == "substitution_definition")
    {
        let rawsource = cx
            .tree
            .node(definition)
            .rawsource()
            .unwrap_or_default()
            .to_string();
        let message = cx.system_message(
            ERROR,
            "Circular substitution definition detected:",
            Some(definition),
            vec![messages::literal_block(&rawsource)],
        );
        let crash = match cx.tree.parent(definition) {
            Some(parent) if cx.tree.node(parent).children.contains(&definition) => None,
            Some(_) => Some(SphinxCrash::python(
                "ValueError",
                format!("{} is not in list", element_repr(cx.tree, definition)),
                LogLocation::None,
            )),
            None if cx.tree.detached().contains(&definition) => None,
            None => Some(SphinxCrash::python(
                "AttributeError",
                "'NoneType' object has no attribute 'replace'".to_string(),
                LogLocation::None,
            )),
        };
        match crash {
            Some(crash) => cx.crash(crash, definition),
            None => cx.tree.replace_self(definition, vec![message]),
        }
        return;
    }
    let mut origin = reference;
    while let Some(&from) = run.origin.get(&origin) {
        origin = from;
    }
    let text = format!("Circular substitution definition referenced: \"{refname}\".");
    let message = cx.system_message(ERROR, &text, Some(origin), Vec::new());
    let msgid = cx.set_id(message);
    replace_with_problematic(cx, reference, message, &msgid);
}

/// docutils' `repr()` of the element at `node` (`Element.__repr__`,
/// `SP/docutils/nodes.py:630-641`): its tag, its names when it has any,
/// and its children's short reprs ([`short_repr`]), cut to 56 characters
/// and ` ...` once they pass 60.
fn element_repr(tree: &TxTree, node: TxId) -> String {
    let element = tree.node(node);
    let mut data = String::new();
    for &child in &element.children {
        data.push_str(&short_repr(tree, child));
        if data.chars().count() > 60 {
            data = data.chars().take(56).collect::<String>() + " ...";
            break;
        }
    }
    match element.attrs.names.as_slice() {
        [] => format!("<{}: {data}>", element.kind),
        names => format!("<{} \"{}\": {data}>", element.kind, names.join("; ")),
    }
}

/// docutils' `shortrepr()` of the node at `node` (`nodes.py:431-435`,
/// `:643-647`): an element's tag and its names, or `<#text: …>` with the
/// `repr()` of a Text's data (`str(Text)`, null-escaped), cut to 14
/// characters and ` ...` when it passes 18.
fn short_repr(tree: &TxTree, node: TxId) -> String {
    let node = tree.node(node);
    if node.kind == kinds::TEXT {
        let data = node
            .ext
            .as_deref()
            .and_then(|ext| ext.raw.as_deref())
            .or(node.text.as_deref())
            .unwrap_or_default();
        let data = if data.chars().count() > 18 {
            data.chars().take(14).collect::<String>() + " ..."
        } else {
            data.to_string()
        };
        return format!("<#text: {}>", py_repr_str(&data));
    }
    match node.attrs.names.as_slice() {
        [] => format!("<{}...>", node.kind),
        names => format!("<{} \"{}\"...>", node.kind, names.join("; ")),
    }
}

/// `parent[index] = parent[index].rstrip()` (or `.lstrip()`) for the Text
/// at `id`: docutils strips the Text's data, the inliner's null-escaped
/// form, of Python's whitespace (`strip`), so an escaped space stays
/// unless it ends the text, and the text is the data unescaped.
fn strip_text(tree: &mut TxTree, id: TxId, strip: fn(&str) -> &str) {
    let node = tree.node_mut(id);
    match node.ext.as_deref_mut().and_then(|ext| ext.raw.as_mut()) {
        Some(raw) => {
            let stripped = strip(raw).to_string();
            node.text = Some(unescape(&stripped, false));
            if let Some(ext) = node.ext.as_deref_mut() {
                ext.raw = stripped.contains('\0').then(|| stripped.into());
            }
        }
        None => {
            let stripped = node.text.as_deref().map(|text| strip(text).to_string());
            node.text = stripped;
        }
    }
}

/// 260-005 `PropagateTargets` (`SP/docutils/transforms/references.py:17-95`):
/// a block-level target that refers to nothing (`.. _label:`) gives its
/// ids and names to the node after it, `next_node(ascend=True)`, skipping
/// `system_message`s subtree and all, but not to an `Invisible` or
/// `Targetable` node other than a target, so a run of labels collapses onto
/// the node after the last one. `document.ids` follows the ids, and the
/// target keeps its first former id as its `refid` and is noted in
/// `document.refids`; one in a figure whose caption took its ids is taken
/// out instead.
///
/// The loop is `for target in self.document.findall(nodes.target)`, a
/// live walk ([`Findall`]): a target the walk has not reached yet sees the
/// ids an earlier one gave it.
pub struct PropagateTargets;

impl Transform for PropagateTargets {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let mut walk = Findall::new(cx.tree.root());
        while let Some(target) = walk.next(cx.tree, is_target) {
            propagate_target(cx, target);
        }
    }
}

fn propagate_target(cx: &mut TransformCx<'_>, target: TxId) {
    let tree = &mut *cx.tree;
    let parent = tree.parent(target);
    let inline = parent.is_some_and(|parent| is_text_element(tree.node(parent).kind));
    let node = tree.node(target);
    if inline
        || ["refid", "refuri", "refname"]
            .iter()
            .any(|&key| has(node, key))
    {
        return;
    }
    let any = |_: &TxNode| true;
    let mut next = tree.next_node(target, any, false, true, true, true);
    while let Some(message) = next.filter(|&node| tree.node(node).kind == kinds::SYSTEM_MESSAGE) {
        next = tree.next_node(message, any, false, false, true, true);
    }
    let Some(next) = next else {
        return;
    };
    let kind = tree.node(next).kind;
    if (is_invisible(kind) || is_targetable(kind)) && kind != kinds::TARGET {
        return;
    }
    let ids = tree.node(target).attrs.ids.clone();
    let names = tree.node(target).attrs.names.clone();
    let receiver = &mut tree.node_mut(next).attrs;
    receiver.ids.extend(ids.iter().cloned());
    receiver.names.extend(names.iter().cloned());
    tree.expect_referenced_by(next, target, &ids, &names);
    for id in &ids {
        tree.note_id(id, next);
    }
    if let Some(parent) = parent {
        if tree.node(parent).kind == "figure" && tree.node(next).kind == "caption" {
            tree.remove(parent, target);
            return;
        }
    }
    let Some(first) = ids.first() else {
        // docutils' `target['ids'][0]` raises: a target without ids is
        // never made.
        return;
    };
    let node = tree.node_mut(target);
    node.set("refid", AttrValue::Str(first.clone()));
    node.attrs.ids.clear();
    node.attrs.names.clear();
    cx.note_refid(target);
}

/// 440-009 `AnonymousHyperlinks` (`references.py:98-160`): the anonymous
/// references and anonymous targets of the tree pair up in order. When
/// their counts differ, the mismatch is reported with no node — at the
/// parse's final position ([DS §6.2]) — and every anonymous reference
/// becomes a `problematic` ([`replace_with_problematic`]). Otherwise each
/// reference not yet pointing anywhere takes its target's `refuri`, or
/// the first id of the node its target labels (through the targets
/// PropagateTargets emptied), noted in `document.refids`.
pub struct AnonymousHyperlinks;

impl Transform for AnonymousHyperlinks {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        let anonymous = |kind: &'static str| {
            move |node: &TxNode| {
                node.kind == kind && node.get("anonymous").is_some_and(AttrValue::is_truthy)
            }
        };
        let references = cx.tree.findall(root, anonymous(kinds::REFERENCE));
        let targets = cx.tree.findall(root, anonymous(kinds::TARGET));
        if references.len() != targets.len() {
            let text = format!(
                "Anonymous hyperlink mismatch: {} references but {} targets.\n\
                 See \"backrefs\" attribute for IDs.",
                references.len(),
                targets.len()
            );
            let message = cx.system_message(ERROR, &text, None, Vec::new());
            let msgid = cx.set_id(message);
            for reference in references {
                replace_with_problematic(cx, reference, message, &msgid);
            }
            return;
        }
        for (reference, target) in references.into_iter().zip(targets) {
            let node = cx.tree.node(reference);
            if has(node, "refid") || has(node, "refuri") {
                continue;
            }
            cx.tree.set_referenced(target);
            let mut seen = HashSet::new();
            let mut target = target;
            while seen.insert(target) {
                let node = cx.tree.node(target);
                if let Some(refuri) = node.str_attr("refuri") {
                    let refuri = AttrValue::Str(refuri.to_string());
                    cx.tree.node_mut(reference).set("refuri", refuri);
                    cx.tree.set_resolved(reference);
                    break;
                }
                if let Some(first) = node.attrs.ids.first() {
                    let refid = AttrValue::Str(first.clone());
                    cx.tree.node_mut(reference).set("refid", refid);
                    cx.note_refid(reference);
                    break;
                }
                // A propagated target: on to the node that holds its ids.
                match node
                    .str_attr("refid")
                    .and_then(|refid| cx.tree.by_id(refid))
                {
                    Some(labelled) => target = labelled,
                    None => break,
                }
            }
        }
    }
}

/// 460-010 `IndirectHyperlinks` (`references.py:163-337`): each indirect
/// target the parse noted, in registration order, is resolved — through
/// the targets it refers to, recursively — to a `refuri` or a `refid`
/// ([`resolve_indirect_target`]), and every reference to its names and
/// ids then takes the same ([`resolve_indirect_references`]). A target
/// whose name resolves to nothing, to a name two targets claim, or back to
/// itself is reported ([`indirect_target_error`]).
pub struct IndirectHyperlinks;

impl Transform for IndirectHyperlinks {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let targets: Vec<TxId> = cx
            .state
            .registries
            .indirect_targets
            .iter()
            .filter_map(|&uid| cx.tree.by_uid(uid))
            .collect();
        // docutils' `target.multiply_indirect`: the targets whose
        // resolution is on the way, which a cycle leads back to.
        let mut on_the_way = HashSet::new();
        for target in targets {
            if !cx.tree.resolved(target) {
                resolve_indirect_target(cx, target, &mut on_the_way);
            }
            resolve_indirect_references(cx, target);
        }
    }
}

/// `IndirectHyperlinks.resolve_indirect_target` (`references.py:223-264`).
fn resolve_indirect_target(cx: &mut TransformCx<'_>, target: TxId, on_the_way: &mut HashSet<TxId>) {
    let node = cx.tree.node(target);
    let refname = node.str_attr("refname").map(str::to_string);
    let reftarget_id = match &refname {
        Some(refname) => {
            let bound = cx
                .state
                .names
                .get(refname)
                .and_then(|entry| entry.id.clone());
            match bound.filter(|id| !id.is_empty()) {
                Some(id) => id,
                None => {
                    nonexistent_indirect_target(cx, target);
                    return;
                }
            }
        }
        // docutils' `target['refid']` raises when there is none.
        None => match node.str_attr("refid") {
            Some(refid) => refid.to_string(),
            None => return,
        },
    };
    // `reftarget = document.ids[reftarget_id]`: no node here for an id the
    // parse registered on a node it dropped, an element holding the id.
    let reftarget = cx.tree.by_id(&reftarget_id);
    if let Some(reftarget) = reftarget {
        cx.tree
            .note_referenced_by(reftarget, None, Some(&reftarget_id));
        let node = cx.tree.node(reftarget);
        if node.kind == kinds::TARGET && !cx.tree.resolved(reftarget) && has(node, "refname") {
            if on_the_way.contains(&target) {
                indirect_target_error(cx, target, "forming a circular reference");
                return;
            }
            on_the_way.insert(target);
            resolve_indirect_target(cx, reftarget, on_the_way);
            on_the_way.remove(&target);
        }
    }
    let (refuri, refid, has_ids) = match reftarget {
        Some(reftarget) => {
            let node = cx.tree.node(reftarget);
            (
                node.str_attr("refuri").map(str::to_string),
                node.str_attr("refid").map(str::to_string),
                !node.attrs.ids.is_empty(),
            )
        }
        None => (None, None, true),
    };
    if let Some(refuri) = refuri {
        let node = cx.tree.node_mut(target);
        node.set("refuri", AttrValue::Str(refuri));
        node.delattr("refid");
    } else if let Some(refid) = refid {
        cx.tree.node_mut(target).set("refid", AttrValue::Str(refid));
        cx.note_refid(target);
    } else if has_ids {
        cx.tree
            .node_mut(target)
            .set("refid", AttrValue::Str(reftarget_id));
        cx.note_refid(target);
    } else {
        nonexistent_indirect_target(cx, target);
        return;
    }
    if refname.is_some() {
        cx.tree.node_mut(target).delattr("refname");
    }
    cx.tree.set_resolved(target);
}

/// `IndirectHyperlinks.nonexistent_indirect_target` (`references.py:266-271`).
fn nonexistent_indirect_target(cx: &mut TransformCx<'_>, target: TxId) {
    let Some(refname) = cx.tree.node(target).str_attr("refname") else {
        // docutils' `target['refname']` raises.
        return;
    };
    let explanation = if cx.state.names.has_name(refname) {
        "which is a duplicate, and cannot be used as a unique reference"
    } else {
        "which does not exist"
    };
    indirect_target_error(cx, target, explanation);
}

/// `IndirectHyperlinks.indirect_target_error` (`references.py:276-298`):
/// the error, at the target's location, names the target by its first
/// name and first id. Every node that refers to one of its names (in the
/// parse's `refnames`) or ids (in `document.refids`), once each, in that
/// order, becomes a `problematic` ([`replace_with_problematic`]), and the
/// target counts as resolved.
fn indirect_target_error(cx: &mut TransformCx<'_>, target: TxId, explanation: &str) {
    let node = cx.tree.node(target);
    let names = node.attrs.names.clone();
    let ids = node.attrs.ids.clone();
    let refname = node.str_attr("refname").unwrap_or_default().to_string();
    let mut naming = String::new();
    let mut reflist = Vec::new();
    if let Some(name) = names.first() {
        naming = format!("\"{name}\" ");
    }
    for name in &names {
        reflist.extend(refnames(cx, name));
    }
    for id in &ids {
        reflist.extend(refids(cx.tree, id));
    }
    if let Some(id) = ids.first() {
        naming.push_str(&format!("(id=\"{id}\")"));
    }
    let text = format!(
        "Indirect hyperlink target {naming} refers to target \"{refname}\", {explanation}."
    );
    let message = cx.system_message(ERROR, &text, Some(target), Vec::new());
    let msgid = cx.set_id(message);
    // `utils.uniq(reflist)`.
    let mut seen = HashSet::new();
    for reference in reflist {
        if seen.insert(reference) {
            replace_with_problematic(cx, reference, message, &msgid);
        }
    }
    cx.tree.set_resolved(target);
}

/// `IndirectHyperlinks.resolve_indirect_references` (`references.py:300-337`):
/// every unresolved node that refers to one of the target's names, then
/// to one of its ids, takes the target's `refid` (noted in
/// `document.refids`) or `refuri` in place of its `refname` or `refid`; a
/// target among them passes it on in turn.
fn resolve_indirect_references(cx: &mut TransformCx<'_>, target: TxId) {
    let node = cx.tree.node(target);
    let (attname, attval) = if let Some(refid) = node.str_attr("refid") {
        ("refid", refid.to_string())
    } else if let Some(refuri) = node.str_attr("refuri") {
        ("refuri", refuri.to_string())
    } else {
        return;
    };
    let names = node.attrs.names.clone();
    let ids = node.attrs.ids.clone();
    let by_name = names
        .iter()
        .map(|name| (refnames(cx, name), "refname", name));
    let by_name: Vec<_> = by_name.collect();
    for (reflist, key, name) in by_name {
        if !reflist.is_empty() {
            cx.tree.note_referenced_by(target, Some(name), None);
        }
        pass_on(cx, &reflist, key, attname, &attval);
    }
    for id in &ids {
        let reflist = refids(cx.tree, id);
        if !reflist.is_empty() {
            cx.tree.note_referenced_by(target, None, Some(id));
        }
        pass_on(cx, &reflist, "refid", attname, &attval);
    }
}

/// One `for ref in reflist` loop of [`resolve_indirect_references`]: each
/// unresolved node drops `key` and takes `attname`.
fn pass_on(
    cx: &mut TransformCx<'_>,
    reflist: &[TxId],
    key: &'static str,
    attname: &'static str,
    attval: &str,
) {
    for &reference in reflist {
        if cx.tree.resolved(reference) {
            continue;
        }
        let node = cx.tree.node_mut(reference);
        node.delattr(key);
        node.set(attname, AttrValue::Str(attval.to_string()));
        if attname == "refid" {
            cx.note_refid(reference);
        }
        cx.tree.set_resolved(reference);
        if cx.tree.node(reference).kind == kinds::TARGET {
            resolve_indirect_references(cx, reference);
        }
    }
}

/// 540 `TargetNotes` (`references.py:767-847`), the transform a
/// `.. target-notes::` defers to its `pending` node, after every static
/// transform of its priority ([`super::schedule`]): each external target
/// some node refers to by name gets a footnote holding its `refuri`
/// ([`make_target_footnote`]), and so, after them, does each anonymous
/// reference to a `refuri`; a footnote reference to it follows each
/// referring node that is not itself a target. Footnotes with one `refuri`
/// are one footnote. The footnotes take the pending node's place, and
/// Footnotes (620) numbers them and their references after the parse's
/// auto-numbered ones, through the registries this transform adds them
/// to.
///
/// docutils' two loops are live walks (`findall`) over a tree the
/// transform changes, and a footnote reference inserted ahead of the
/// walk's position in a list it is walking makes it meet the target it is
/// on again, and so again: Sphinx 9.1.0 never finishes reading a document
/// where a reference to an external target comes before the target in the
/// same paragraph (`See Python_ and `Python <https://python.org/>`_.`) or
/// the same body (an image whose `:target:` names a target after it),
/// probed. Every insertion is a footnote reference or a space after a
/// node, never a target or a reference, so where Sphinx's read finishes
/// the walks meet each node once, in document order: the walks here are
/// snapshots, which also finish where Sphinx's never does, with each
/// target noted once.
pub struct TargetNotes;

impl Transform for TargetNotes {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        // The schedule runs this row for a `note_pending` record only, and
        // the arena keeps every identity it ever held.
        let pending = cx
            .pending_record()
            .and_then(|record| cx.tree.by_uid(record.node));
        debug_assert!(pending.is_some(), "TargetNotes without its pending node");
        let Some(pending) = pending else {
            return;
        };
        // `startnode.details.get('class', [])`.
        let classes = cx
            .tree
            .node(pending)
            .ext
            .as_deref()
            .and_then(|ext| ext.pending.as_deref())
            .and_then(|pending| {
                pending.details.iter().find_map(|(key, value)| match value {
                    PyValue::StrList(classes) if key == "class" => Some(classes.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        let root = cx.tree.root();
        let mut notes: HashMap<String, TxId> = HashMap::new();
        let mut nodelist = Vec::new();
        for target in cx.tree.findall(root, is_target) {
            let node = cx.tree.node(target);
            let Some(refuri) = node.str_attr("refuri").map(str::to_string) else {
                continue;
            };
            let names = node.attrs.names.clone();
            let refs: Vec<TxId> = names.iter().flat_map(|name| refnames(cx, name)).collect();
            if refs.is_empty() {
                continue;
            }
            let footnote = make_target_footnote(cx, &refuri, &refs, &notes, &classes);
            if let Entry::Vacant(slot) = notes.entry(refuri) {
                slot.insert(footnote);
                nodelist.push(footnote);
            }
        }
        // The anonymous references, which AnonymousHyperlinks (440) gave
        // their target's `refuri`.
        for reference in cx.tree.findall(root, |node| is_reference(node.kind)) {
            let node = cx.tree.node(reference);
            if !node.get("anonymous").is_some_and(AttrValue::is_truthy) {
                continue;
            }
            let Some(refuri) = node.str_attr("refuri").map(str::to_string) else {
                continue;
            };
            let footnote = make_target_footnote(cx, &refuri, &[reference], &notes, &classes);
            if let Entry::Vacant(slot) = notes.entry(refuri) {
                slot.insert(footnote);
                nodelist.push(footnote);
            }
        }
        cx.tree.replace_self(pending, nodelist);
    }
}

/// `TargetNotes.make_target_footnote` (`references.py:808-847`): the
/// footnote of `refuri`, the one `notes` holds or a new one, and a
/// footnote reference to it after each of `refs` that is not a target.
///
/// A new footnote takes the next `idN` (`set_id`, before it has a name),
/// then the name `TARGET_NOTE: idN` and `auto = 1`, and holds a paragraph
/// with a reference to the `refuri`; it is noted as an auto-numbered
/// footnote (`note_autofootnote`) and its name bound as an explicit
/// target's ([`super::state::NameTable::note_explicit_name`]). Its
/// identity is minted before its `set_id` (the T5b ruling): docutils'
/// `note_autofootnote` and `note_explicit_target` `set_id` it again, which
/// is a no-op for the node holding the id, as the allocator knows it by
/// that identity.
///
/// Each footnote reference refers to the footnote by its name, `auto = 1`,
/// with the directive's `:class:` values; it takes the next `idN`
/// (`note_autofootnote_ref`) and is noted under the name as a footnote
/// reference and a refname (`note_footnote_ref`). It goes in right after
/// the node referring to the target, preceded by a space
/// (`trim_footnote_reference_space` is off under Sphinx): a Text node, or
/// an `inline` holding it with the `:class:` values. A referring node with
/// no parent, a root of the detached forest (one docutils has in the
/// anonymous element a directive parsed it into), keeps its footnote
/// reference out of every tree, where docutils' is out of the document's.
fn make_target_footnote(
    cx: &mut TransformCx<'_>,
    refuri: &str,
    refs: &[TxId],
    notes: &HashMap<String, TxId>,
    classes: &[String],
) -> TxId {
    let (footnote, footnote_name) = match notes.get(refuri) {
        Some(&footnote) => {
            let names = &cx.tree.node(footnote).attrs.names;
            debug_assert_eq!(names.len(), 1, "a target note has one name");
            (footnote, names.first().cloned().unwrap_or_default())
        }
        None => {
            let footnote = cx.tree.new_node(kinds::FOOTNOTE);
            let uid = cx.tree.assign_uid(footnote, cx.state);
            let footnote_id = cx.set_id(footnote);
            let footnote_name = format!("TARGET_NOTE: {footnote_id}");
            let node = cx.tree.node_mut(footnote);
            node.set("auto", AttrValue::Int(1));
            node.attrs.names = vec![footnote_name.clone()];
            let paragraph = cx.tree.new_node(kinds::PARAGRAPH);
            let reference = cx.tree.new_node(kinds::REFERENCE);
            cx.tree
                .node_mut(reference)
                .set("refuri", AttrValue::Str(refuri.to_string()));
            if !refuri.is_empty() {
                let text = new_text(cx.tree, refuri);
                cx.tree.append(reference, text);
            }
            cx.tree.append(paragraph, reference);
            cx.tree.append(footnote, paragraph);
            cx.state.registries.autofootnotes.push(uid);
            let node = cx.tree.node(footnote);
            let view = NodeView {
                kind: node.kind,
                names: &node.attrs.names,
                refuri: node.str_attr("refuri"),
                refname: node.str_attr("refname"),
            };
            cx.state.names.note_explicit_name(view, &footnote_id);
            (footnote, footnote_name)
        }
    };
    for &reference in refs {
        if cx.tree.node(reference).kind == kinds::TARGET {
            continue;
        }
        let refnode = cx.tree.new_node(kinds::FOOTNOTE_REFERENCE);
        let node = cx.tree.node_mut(refnode);
        node.set("refname", AttrValue::Str(footnote_name.clone()));
        node.set("auto", AttrValue::Int(1));
        node.attrs.classes.extend(classes.iter().cloned());
        let uid = cx.tree.assign_uid(refnode, cx.state);
        cx.set_id(refnode);
        let registries = &mut cx.state.registries;
        registries.autofootnote_refs.push(uid);
        registries.footnote_refs.push(&footnote_name, uid);
        registries.refnames.push(&footnote_name, uid);
        let space = new_text(cx.tree, " ");
        let space = if classes.is_empty() {
            space
        } else {
            let inline = cx.tree.new_node("inline");
            cx.tree.node_mut(inline).attrs.classes = classes.to_vec();
            cx.tree.append(inline, space);
            inline
        };
        let Some(parent) = cx.tree.parent(reference) else {
            continue;
        };
        let index = cx
            .tree
            .node(parent)
            .children
            .iter()
            .position(|&child| child == reference);
        // docutils' `ref.parent.index(ref)` raises for a node out of its
        // parent, which no referring node is by 540: the transforms that
        // replace one (Footnotes, 620; the citation transforms, 619) run
        // later.
        debug_assert!(index.is_some(), "a referring node out of its parent");
        if let Some(index) = index {
            cx.tree.insert(parent, index + 1, space);
            cx.tree.insert(parent, index + 2, refnode);
        }
    }
    footnote
}

/// `nodes.Text(text)`, in no parent.
fn new_text(tree: &mut TxTree, text: &str) -> TxId {
    let node = tree.new_node(kinds::TEXT);
    tree.node_mut(node).text = Some(text.to_string());
    node
}

/// 640-012 `ExternalTargets` (`references.py:340-373`): every unresolved
/// node that refers to a name of a target with a `refuri` takes it in
/// place of its `refname`. The loop is a live walk that moves no node.
pub struct ExternalTargets;

impl Transform for ExternalTargets {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let mut walk = Findall::new(cx.tree.root());
        while let Some(target) = walk.next(cx.tree, is_target) {
            let node = cx.tree.node(target);
            let Some(refuri) = node.str_attr("refuri").map(str::to_string) else {
                continue;
            };
            for name in node.attrs.names.clone() {
                let reflist = refnames(cx, &name);
                if !reflist.is_empty() {
                    cx.tree.note_referenced_by(target, Some(&name), None);
                }
                for reference in reflist {
                    if cx.tree.resolved(reference) {
                        continue;
                    }
                    let node = cx.tree.node_mut(reference);
                    node.delattr("refname");
                    node.set("refuri", AttrValue::Str(refuri.clone()));
                    cx.tree.set_resolved(reference);
                }
            }
        }
    }
}

/// 660-013 `InternalTargets` (`references.py:376-414`): every unresolved
/// node that refers to a name of a target with neither `refuri` nor
/// `refid` takes the id the name is bound to (`document.nameids`) in place
/// of its `refname`. The loop is a live walk that moves no node.
pub struct InternalTargets;

impl Transform for InternalTargets {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let mut walk = Findall::new(cx.tree.root());
        while let Some(target) = walk.next(cx.tree, is_target) {
            let node = cx.tree.node(target);
            if has(node, "refuri") || has(node, "refid") {
                continue;
            }
            for name in node.attrs.names.clone() {
                let refid = cx.state.names.get(&name).and_then(|entry| entry.id.clone());
                let reflist = refnames(cx, &name);
                if !reflist.is_empty() {
                    cx.tree.note_referenced_by(target, Some(&name), None);
                }
                for reference in reflist {
                    if cx.tree.resolved(reference) {
                        continue;
                    }
                    if let Some(refid) = refid.as_ref().filter(|refid| !refid.is_empty()) {
                        let node = cx.tree.node_mut(reference);
                        node.delattr("refname");
                        node.set("refid", AttrValue::Str(refid.clone()));
                    }
                    cx.tree.set_resolved(reference);
                }
            }
        }
    }
}

/// 850-039 `SphinxDanglingReferences` (`SP/sphinx/transforms/references.py:18-30`,
/// docutils' `DanglingReferences`, `references.py:878-990`): walks the
/// tree (`document.walk`, which visits a node and then a copy of its
/// child list) and resolves each unresolved `reference`,
/// `footnote_reference` and `citation_reference` with a `refname` through
/// `document.nameids` ([`visit_reference`]). Then every target nothing
/// referenced, but an anonymous one, is reported as an INFO, which Sphinx
/// keeps out of the stream by raising the reporter's level for the
/// duration (the stream prints no INFO here at any time,
/// [`super::reporter::render_records`]). In Sphinx's read no citation
/// reference is left in the tree by then: `CitationReferenceTransform`
/// (619-017) has replaced each by a `pending_xref`.
pub struct SphinxDanglingReferences;

impl Transform for SphinxDanglingReferences {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        let mut walk = vec![root];
        while let Some(node) = walk.pop() {
            if matches!(
                cx.tree.node(node).kind,
                kinds::REFERENCE | kinds::FOOTNOTE_REFERENCE | kinds::CITATION_REFERENCE
            ) {
                visit_reference(cx, node);
            }
            walk.extend(cx.tree.node(node).children.iter().rev());
        }
        for target in cx.tree.findall(root, is_target) {
            if cx.tree.referenced(target) {
                continue;
            }
            let node = cx.tree.node(target);
            if node.get("anonymous").is_some_and(AttrValue::is_truthy) {
                continue;
            }
            let naming = node
                .attrs
                .names
                .first()
                .or(node.attrs.ids.first())
                .map(String::as_str)
                .or(node.str_attr("refid"));
            // docutils' `target['refid']` raises for a target with none.
            let Some(naming) = naming else {
                continue;
            };
            let text = format!("Hyperlink target \"{naming}\" is not referenced.");
            cx.system_message(INFO, &text, Some(target), Vec::new());
        }
    }
}

/// `DanglingReferencesVisitor.visit_reference` (`references.py:937-988`):
/// a name bound to an id resolves the reference to it; anything else
/// replaces the reference by a `problematic`, after an error at the
/// reference's location, with a hint for what looks like a broken
/// embedded link. The problematic reuses the reference's first id when it
/// has one (`prbid = node['ids'][0]`) and takes a new one otherwise
/// ([DS §5.5]).
fn visit_reference(cx: &mut TransformCx<'_>, node: TxId) {
    if cx.tree.resolved(node) {
        return;
    }
    let Some(refname) = cx.tree.node(node).str_attr("refname").map(str::to_string) else {
        return;
    };
    if let Some(id) = cx
        .state
        .names
        .get(&refname)
        .and_then(|entry| entry.id.clone())
    {
        let reference = cx.tree.node_mut(node);
        reference.delattr("refname");
        reference.set("refid", AttrValue::Str(id.clone()));
        // `document.ids[id]`: no node here for one the parse dropped.
        if let Some(holder) = cx.tree.by_id(&id) {
            cx.tree.note_referenced_by(holder, None, Some(&id));
        }
        cx.tree.set_resolved(node);
        return;
    }
    let (text, details) = if cx.state.names.has_name(&refname) {
        (
            format!("Duplicate target name, cannot be used as a unique reference: \"{refname}\"."),
            Vec::new(),
        )
    } else {
        let details = embedding_hint(&refname)
            .map(|hint| vec![messages::paragraph(&hint)])
            .unwrap_or_default();
        (format!("Unknown target name: \"{refname}\"."), details)
    };
    let message = cx.system_message(ERROR, &text, Some(node), details);
    let msgid = cx.set_id(message);
    let rawsource = cx
        .tree
        .node(node)
        .rawsource()
        .unwrap_or_default()
        .to_string();
    let prb = problematic(cx.tree, &rawsource, &msgid);
    let prbid = match cx.tree.node(node).attrs.ids.first() {
        Some(id) => id.clone(),
        None => cx.set_id(prb),
    };
    cx.tree.node_mut(message).attrs.backrefs.push(prbid);
    cx.tree.replace_self(node, vec![prb]);
}

/// The hint `DanglingReferences` adds to an unknown name holding `<` or
/// `>`, a botched embedded link (`references.py:960-976`).
fn embedding_hint(refname: &str) -> Option<String> {
    if !(refname.contains('<') || refname.contains('>')) {
        return None;
    }
    let mut hint = "Did you want to embed a URI or alias?".to_string();
    if !refname.contains('<') {
        hint.push_str("\nOpening bracket missing.");
    } else if !refname.contains(" <") {
        hint.push_str("\nThe embedded reference must be preceded by whitespace.");
    }
    if !refname.contains('>') {
        hint.push_str("\nClosing bracket missing.");
    } else if !refname.ends_with('>') {
        hint.push_str("\nThe embedded reference must be the last text before the end string.");
    }
    if refname.contains("< ") || refname.contains(" >") {
        hint.push_str("\nWhitespace around the embedded reference is not allowed.");
    }
    Some(hint)
}

#[cfg(test)]
mod tests {
    use crate::doctree::messages::{ERROR, INFO, WARNING};
    use crate::transforms::test_support::{differences, error, event, read_snippet, SphinxRead};

    /// `PropagateTargets` (260-005, `SP/docutils/transforms/references.py:17-95`)
    /// moves the ids and names of a block-level target that refers to
    /// nothing to the node after it, and leaves the target with `refid`,
    /// its first former id. A target is a node it may move them to, so a
    /// chain of labels collapses onto the section after the last one, the
    /// last label's ids first: `docutils-0121`, and CPython's
    /// `.. _booleans:` / `.. _and:` / `.. _or:` / `.. _not:` shape, whose
    /// every label then resolves (`DanglingReferences`, 850, through
    /// `nameids`). A `system_message` after the target is skipped, subtree
    /// and all. A comment or footnote (`Invisible`, `Targetable`) stops it,
    /// and so would an `index`, but `ReorderConsecutiveTargetAndIndexNodes`
    /// (220) moves the index nodes of a run of targets first.
    //
    // oracle (Sphinx 9.1.0, docutils 0.22.4): each input read as
    //   `index.rst` through tools/gen_read_state_fixture.py's harness
    //   (`keep_warnings = True`); `docutils-0121`'s tree is also the HTML
    //   oracle's post-read doctree.
    #[test]
    fn propagate_targets_chain() {
        let rows: [(&str, &str, &[&str]); 5] = [
            (
                // docutils-0121.
                ".. _target1:\n.. _target2:\n\nSection Title\n=============\n",
                "    <target refid=\"target1\">\n    <target refid=\"target2\">\n    \
                 <section ids=\"section-title target2 target1\" \
                 names=\"section\\ title target2 target1\">\n        <title>\n            \
                 Section Title\n",
                &[],
            ),
            (
                ".. _target1:\n.. _target2:\n.. _target3:\n.. _target4:\n\nSection Title\n\
                 =============\n\nSee target1_ and target3_.\n",
                "    <target refid=\"target1\">\n    <target refid=\"target2\">\n    \
                 <target refid=\"target3\">\n    <target refid=\"target4\">\n    \
                 <section ids=\"section-title target4 target3 target2 target1\" \
                 names=\"section\\ title target4 target3 target2 target1\">\n        \
                 <title>\n            Section Title\n        <paragraph>\n            See \n            \
                 <reference name=\"target1\" refid=\"target1\">\n                target1\n             \
                 and \n            <reference name=\"target3\" refid=\"target3\">\n                \
                 target3\n            .\n",
                &[],
            ),
            (
                ".. _x:\n\n.. nosuchdirective::\n\nPara.\n",
                "    <target refid=\"x\">\n    <system_message level=\"3\" line=\"3\" \
                 source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            \
                 Unknown directive type \"nosuchdirective\".\n        \
                 <literal_block xml:space=\"preserve\">\n            .. nosuchdirective::\n    \
                 <paragraph ids=\"x\" names=\"x\">\n        Para.\n",
                &[
                    "<SRCDIR>/index.rst:3: ERROR: Unknown directive type \"nosuchdirective\".",
                    "",
                    ".. nosuchdirective:: [docutils]",
                ],
            ),
            (
                ".. _x:\n\n.. index:: y\n\nPara.\n",
                "    <index entries=\"('single',\\ 'y',\\ 'index-0',\\ '',\\ None)\" inline=\"0\">\n    \
                 <target refid=\"x\">\n    <target refid=\"index-0\">\n    \
                 <paragraph ids=\"index-0 x\" names=\"x\">\n        Para.\n",
                &[],
            ),
            (
                ".. _x:\n\n.. a comment\n\nPara.\n\n.. _y:\n\n.. [1] note\n",
                "    <target ids=\"x\" names=\"x\">\n    <comment xml:space=\"preserve\">\n        \
                 a comment\n    <paragraph>\n        Para.\n    <target ids=\"y\" names=\"y\">\n    \
                 <footnote docname=\"index\" ids=\"id1\" names=\"1\">\n        <label>\n            \
                 1\n        <paragraph>\n            note\n",
                &["<SRCDIR>/index.rst:9: WARNING: Footnote [1] is not referenced. [ref.footnote]"],
            ),
        ];
        let mut mismatches = Vec::new();
        for (rst, body, warnings) in rows {
            let read = read_snippet(rst, true);
            if read.body != body || read.warnings != warnings {
                mismatches.push(format!(
                    "{rst:?}\n--- sphinx 9.1.0 ---\n{body}{warnings:#?}\n--- ours ---\n{}{:#?}",
                    read.body, read.warnings
                ));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// `AnonymousHyperlinks` (440-009, `references.py:98-160`): when the
    /// anonymous references and targets do not pair up, the transform
    /// reports the mismatch with no node, so docutils locates the message
    /// at the parse's final state-machine position ([DS §6.2]): none after
    /// a target (`docutils-0120`, which prints `index.rst::`), the line
    /// after the last paragraph (`docutils-0167`). Every anonymous
    /// reference becomes a `problematic` holding its rawsource, each with
    /// an id of its own after the message's; the message itself stays out
    /// of the tree.
    //
    // oracle (Sphinx 9.1.0): as `propagate_targets_chain`; the
    //   `docutils-0120` and `docutils-0167` streams are also the HTML
    //   oracle's.
    #[test]
    fn anonymous_mismatch_is_line_less() {
        let read = read_snippet(
            ".. __: https://example.com/1\n\n__ https://example.com/2\n",
            true,
        );
        assert_eq!(
            read.body,
            "    <target anonymous=\"1\" ids=\"id1\" refuri=\"https://example.com/1\">\n    \
             <target anonymous=\"1\" ids=\"id2\" refuri=\"https://example.com/2\">\n"
        );
        assert_eq!(
            read.warnings,
            error(
                "",
                "Anonymous hyperlink mismatch: 0 references but 2 targets.\n\
                 See \"backrefs\" attribute for IDs."
            )
        );
        assert_eq!(
            read.trace,
            [
                event("parse", "target", "id1", true),
                event("parse", "target", "id2", true),
                event("AnonymousHyperlinks", "system_message", "id3", false),
            ]
        );

        // docutils-0167: the final position is the line after the last
        // paragraph.
        let read = read_snippet("__ https://x/\npara after\n", true);
        assert_eq!(
            read.warnings[1..],
            error(
                "3",
                "Anonymous hyperlink mismatch: 0 references but 1 targets.\n\
                 See \"backrefs\" attribute for IDs."
            )
        );

        let read = read_snippet("See `a`__ and `b`__.\n\n__ http://a\n", true);
        assert_eq!(
            read.body,
            "    <paragraph>\n        See \n        <problematic ids=\"id3\" refid=\"id2\">\n            \
             `a`__\n         and \n        <problematic ids=\"id4\" refid=\"id2\">\n            \
             `b`__\n        .\n    <target anonymous=\"1\" ids=\"id1\" refuri=\"http://a\">\n"
        );
        assert_eq!(
            read.warnings,
            error(
                "",
                "Anonymous hyperlink mismatch: 2 references but 1 targets.\n\
                 See \"backrefs\" attribute for IDs."
            )
        );
        assert_eq!(
            read.trace,
            [
                event("parse", "target", "id1", true),
                event("AnonymousHyperlinks", "system_message", "id2", false),
                event("AnonymousHyperlinks", "problematic", "id3", true),
                event("AnonymousHyperlinks", "problematic", "id4", true),
            ]
        );
    }

    /// `IndirectHyperlinks` (460-010, `references.py:163-337`) reports an
    /// indirect target whose name is not a target's at the target's own
    /// location (`docutils-0122`), with the message out of the tree.
    //
    // oracle (Sphinx 9.1.0): as `propagate_targets_chain`.
    #[test]
    fn indirect_target_to_missing_name() {
        let read = read_snippet(".. _a: `phrase ref`_\n", true);
        assert_eq!(
            read.body,
            "    <target ids=\"a\" names=\"a\" refname=\"phrase ref\">\n"
        );
        assert_eq!(
            read.warnings,
            error(
                "1",
                "Indirect hyperlink target \"a\" (id=\"a\") refers to target \"phrase ref\", \
                 which does not exist."
            )
        );
        assert_eq!(
            read.trace,
            [
                event("parse", "target", "a", true),
                event("IndirectHyperlinks", "system_message", "id1", false),
            ]
        );
    }

    /// The rest of `IndirectHyperlinks`: a chain of indirect targets takes
    /// the last one's `refuri`, and so do the references to every link of
    /// it; one to an internal target takes its `refid`. A name two targets
    /// claim is reported as a duplicate, and the reference to the indirect
    /// target becomes a `problematic`. A cycle is reported once, for the
    /// target the walk started from, whose recursion then goes on: the
    /// other target of the cycle, and every reference to the reported
    /// target's name, become `problematic`s holding their rawsource (the
    /// explicit target's block text), and the reported target still takes
    /// a `refid` on the way out. A target the propagated ids of an
    /// internal target land on is the target its names refer to: the
    /// reference through both is reported as a cycle.
    //
    // oracle (Sphinx 9.1.0): as `propagate_targets_chain`.
    #[test]
    fn indirect_targets_resolve_chains_and_report_errors() {
        let rows: [(&str, &str, &[&str]); 5] = [
            (
                ".. _a: b_\n.. _b: c_\n.. _c: http://z\n\nSee a_, b_, c_.\n",
                "    <target ids=\"a\" names=\"a\" refuri=\"http://z\">\n    \
                 <target ids=\"b\" names=\"b\" refuri=\"http://z\">\n    \
                 <target ids=\"c\" names=\"c\" refuri=\"http://z\">\n    <paragraph>\n        See \n        \
                 <reference name=\"a\" refuri=\"http://z\">\n            a\n        , \n        \
                 <reference name=\"b\" refuri=\"http://z\">\n            b\n        , \n        \
                 <reference name=\"c\" refuri=\"http://z\">\n            c\n        .\n",
                &[],
            ),
            (
                ".. _a: b_\n.. _b:\n\nPara.\n\nSee a_ and b_.\n",
                "    <target ids=\"a\" names=\"a\" refid=\"b\">\n    <target refid=\"b\">\n    \
                 <paragraph ids=\"b\" names=\"b\">\n        Para.\n    <paragraph>\n        See \n        \
                 <reference name=\"a\" refid=\"b\">\n            a\n         and \n        \
                 <reference name=\"b\" refid=\"b\">\n            b\n        .\n",
                &[],
            ),
            (
                ".. _x:\n.. _x:\n\nPara.\n\n.. _a: x_\n\nSee a_.\n",
                "    <target dupnames=\"x\" refid=\"x\">\n    <system_message backrefs=\"id1\" \
                 level=\"2\" line=\"2\" source=\"<SRCDIR>/index.rst\" type=\"WARNING\">\n        \
                 <paragraph>\n            Duplicate explicit target name: \"x\".\n    \
                 <target dupnames=\"x\" refid=\"id1\">\n    <paragraph ids=\"id1 x\">\n        \
                 Para.\n    <target ids=\"a\" names=\"a\" refname=\"x\">\n    <paragraph>\n        \
                 See \n        <problematic ids=\"id3\" refid=\"id2\">\n            a_\n        .\n",
                &[
                    "<SRCDIR>/index.rst:2: WARNING: Duplicate explicit target name: \"x\". \
                     [docutils]",
                    "<SRCDIR>/index.rst:6: ERROR: Indirect hyperlink target \"a\" (id=\"a\") \
                     refers to target \"x\", which is a duplicate, and cannot be used as a \
                     unique reference. [docutils]",
                ],
            ),
            (
                ".. _a: b_\n.. _b: a_\n\nSee a_ and b_.\n",
                "    <target ids=\"a\" names=\"a\" refid=\"a\">\n    \
                 <problematic ids=\"id2 b\" names=\"b\" refid=\"id1\">\n        .. _b: a_\n    \
                 <paragraph>\n        See \n        <problematic ids=\"id3\" refid=\"id1\">\n            \
                 a_\n         and \n        <reference name=\"b\" refid=\"a\">\n            b\n        .\n",
                &[
                    "<SRCDIR>/index.rst:1: ERROR: Indirect hyperlink target \"a\" (id=\"a\") \
                     refers to target \"b\", forming a circular reference. [docutils]",
                ],
            ),
            (
                "__ missing_\n\nSee `anon`__ and t_.\n\n.. _t:\n.. _u: t_\n\nPara.\n",
                "    <target anonymous=\"1\" ids=\"id1\" refname=\"missing\">\n    <paragraph>\n        \
                 See \n        <problematic ids=\"id3\" refid=\"id2\">\n            `anon`__\n         \
                 and \n        <problematic ids=\"id5\" refid=\"id4\">\n            t_\n        .\n    \
                 <problematic ids=\"id7\" refid=\"id4\">\n        .. _t:\n    \
                 <problematic ids=\"id6 u t\" names=\"u t\" refid=\"id4\">\n        .. _u: t_\n    \
                 <paragraph>\n        Para.\n",
                &[
                    "<SRCDIR>/index.rst:1: ERROR: Indirect hyperlink target (id=\"id1\") refers \
                     to target \"missing\", which does not exist. [docutils]",
                    "<SRCDIR>/index.rst:6: ERROR: Indirect hyperlink target \"u\" (id=\"u\") \
                     refers to target \"t\", forming a circular reference. [docutils]",
                ],
            ),
        ];
        let mut mismatches = Vec::new();
        for (rst, body, warnings) in rows {
            let read = read_snippet(rst, true);
            if read.body != body || read.warnings != warnings {
                mismatches.push(format!(
                    "{rst:?}\n--- sphinx 9.1.0 ---\n{body}{warnings:#?}\n--- ours ---\n{}{:#?}",
                    read.body, read.warnings
                ));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// docutils stamps an explicit or anonymous hyperlink target with
    /// `abs_line_number()` (`SP/docutils/parsers/rst/states.py:2057`,
    /// `:2123`), its line in the top-level input with every inclusion's
    /// lines and padding spliced in, and `setup_child` gives a target in a
    /// container that has `document` the current source: `(file, absolute
    /// line)`, which 460's errors print after or inside an include. A
    /// target in a container without `document` (a list item) keeps no
    /// source, and the reporter reads its absolute line back against the
    /// input, its true line; one in a grid-table cell counts one high
    /// there ([`crate::rst::block`]'s `line_bias`), so it reads back the
    /// line below. Each row is a Sphinx 9.1.0 build (`-w`) of the
    /// document beside its included files.
    #[test]
    fn hyperlink_targets_take_docutils_absolute_input_line() {
        let missing = |naming: &str, refname: &str| {
            format!(
                "ERROR: Indirect hyperlink target {naming} refers to target \"{refname}\", \
                 which does not exist. [docutils]"
            )
        };
        /// The document, its included files and the expected records.
        type Row<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<String>);
        let rows: Vec<Row> = vec![
            (
                "Title\n=====\n\n.. include:: part.rst\n\n.. _x: y_\n",
                vec![("part.rst", "Part.\n")],
                vec![format!(
                    "/index.rst:11: {}",
                    missing("\"x\" (id=\"x\")", "y")
                )],
            ),
            (
                "Title\n=====\n\n.. include:: part.inc\n\nSee a_.\n\n.. _a: b_\n.. _b: a_\n",
                vec![("part.inc", "One.\n\nThree.\n")],
                vec![
                    "/index.rst:15: ERROR: Indirect hyperlink target \"a\" (id=\"a\") refers \
                      to target \"b\", forming a circular reference. [docutils]"
                        .to_string(),
                ],
            ),
            (
                ".. include:: part.inc\n\nSee `x`__.\n\n__ missing_\n",
                vec![("part.inc", "One.\n\nTwo.\n")],
                vec![format!(
                    "/index.rst:12: {}",
                    missing("(id=\"id1\")", "missing")
                )],
            ),
            (
                ".. include:: links.rst\n\nSee a_ b_.\n\n.. _a: missing_\n.. _b: missing2_\n",
                vec![(
                    "links.rst",
                    ".. _python: https://python.org\n.. |proj| replace:: Project\n",
                )],
                vec![
                    format!("/index.rst:11: {}", missing("\"a\" (id=\"a\")", "missing")),
                    format!("/index.rst:12: {}", missing("\"b\" (id=\"b\")", "missing2")),
                ],
            ),
            (
                ".. include:: links.rst\n\nText.\n\n.. include:: links.rst\n\nSee a_.\n\n\
                 .. _a: missing_\n",
                vec![("links.rst", ".. _python2: https://python.org\n")],
                vec![format!(
                    "/index.rst:19: {}",
                    missing("\"a\" (id=\"a\")", "missing")
                )],
            ),
            (
                ".. include:: links.rst\n\nTitle\n=====\n\nSee ind_.\n\n.. _ind: dup_\n\
                 .. _dup: http://a\n.. _dup: http://b\n",
                vec![(
                    "links.rst",
                    ".. _python: https://python.org\n.. |proj| replace:: Project\n",
                )],
                vec![
                    "/index.rst:10: WARNING: Duplicate explicit target name: \"dup\". [docutils]"
                        .to_string(),
                    "/index.rst:14: ERROR: Indirect hyperlink target \"ind\" (id=\"ind\") refers \
                     to target \"dup\", which is a duplicate, and cannot be used as a unique \
                     reference. [docutils]"
                        .to_string(),
                ],
            ),
            (
                "Title\n=====\n\n.. include:: inc.rst\n\nAfter.\n",
                vec![("inc.rst", "Para ind_.\n\n.. _ind: missing_\n")],
                vec![format!(
                    "inc.rst:9: {}",
                    missing("\"ind\" (id=\"ind\")", "missing")
                )],
            ),
            (
                ".. include:: part.inc\n\n- .. _x: missing_\n\n  See x_.\n",
                vec![("part.inc", "Part.\n")],
                vec![format!(
                    "/index.rst:3: {}",
                    missing("\"x\" (id=\"x\")", "missing")
                )],
            ),
            (
                "Title\n=====\n\n+------------------+\n| .. _x: missing_  |\n\
                 |                  |\n| text x_          |\n+------------------+\n",
                vec![],
                vec![format!(
                    "/index.rst:6: {}",
                    missing("\"x\" (id=\"x\")", "missing")
                )],
            ),
        ];
        let mut mismatches = Vec::new();
        for (index, files, expected) in rows {
            let tmp = tempfile::tempdir().expect("a temporary source directory");
            for (name, text) in &files {
                std::fs::write(tmp.path().join(name), text).expect("an included file writes");
            }
            let parse = crate::rst::ParseOptions {
                source_path: tmp.path().join("index.rst").display().to_string(),
                srcdir: Some(tmp.path().to_path_buf()),
                ..crate::transforms::test_support::index_rst()
            };
            let read = crate::read::read_standalone(
                index,
                &parse,
                &crate::transforms::test_support::harness_settings(false),
            );
            let root = tmp.path().display().to_string();
            let records: Vec<String> = read
                .diagnostics
                .iter()
                .map(|record| record.render(true).replace(&root, "").replace('\\', "/"))
                .collect();
            if records != expected {
                mismatches.push(format!("{index:?}\n{records:#?}\n{expected:#?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// Sphinx parses each glossary definition as a `StringList` of its own,
    /// numbered from its first line's offset in its file
    /// (`definition.items[0][1]`, `SP/sphinx/domains/std/__init__.py:540-545`),
    /// into a container that has `document` (`nested_parse_to_nodes`,
    /// `SP/sphinx/util/parsing.py:62-71`). The machine that stamps a
    /// target there counts the definition's lines by their line in their
    /// own file, not by their place in the top-level input, so a target in
    /// a definition prints its own line: after an include, in an included
    /// file, in a note around the glossary and in a table cell alike. A
    /// target in a container without `document` there (a note, a list
    /// item) keeps no source, and the reporter reads that count back
    /// against the top-level input, where an include before the glossary
    /// has moved the lines: the line it finds there is what Sphinx prints.
    /// Each row is a Sphinx 9.1.0 build (`-w`).
    #[test]
    fn glossary_definition_targets_count_their_own_file_s_lines() {
        let missing = |location: &str, naming: &str| {
            format!(
                "{location}: ERROR: Indirect hyperlink target {naming} refers to target \
                 \"missing\", which does not exist. [docutils]"
            )
        };
        let part = [("part.inc", "One.\n\nTwo.\n")];
        let after_include =
            |glossary: &str| format!("Title\n=====\n\n.. include:: part.inc\n\n{glossary}");
        let x = "\"x\" (id=\"x\")";
        /// The document, the files beside it and the expected records.
        type Row<'a> = (String, &'a [(&'a str, &'a str)], Vec<String>);
        let rows: Vec<Row> = vec![
            (
                after_include(".. glossary::\n\n   term\n      See x_.\n\n      .. _x: missing_\n"),
                &part,
                vec![missing("<SRCDIR>/index.rst:11", x)],
            ),
            (
                after_include(".. glossary::\n\n   term\n      See `y`__.\n\n      __ missing_\n"),
                &part,
                vec![missing("<SRCDIR>/index.rst:11", "(id=\"id1\")")],
            ),
            (
                "Title\n=====\n\n.. include:: gl.inc\n".to_string(),
                &[(
                    "gl.inc",
                    "Intro.\n\n.. glossary::\n\n   term\n      See x_.\n\n      .. _x: missing_\n",
                )],
                vec![missing("gl.inc:8", x)],
            ),
            (
                after_include(
                    ".. glossary::\n\n   one\n      First.\n\n   two\n      See x_.\n\n      \
                     .. _x: missing_\n",
                ),
                &part,
                vec![missing("<SRCDIR>/index.rst:14", x)],
            ),
            (
                after_include(
                    ".. note::\n\n   .. glossary::\n\n      term\n         See x_.\n\n         \
                     .. _x: missing_\n",
                ),
                &part,
                vec![missing("<SRCDIR>/index.rst:13", x)],
            ),
            (
                "Title\n=====\n\n+--------------------------+\n| .. glossary::            |\n\
                 |                          |\n|    term                  |\n\
                 |       See x_.            |\n|                          |\n\
                 |       .. _x: missing_    |\n+--------------------------+\n"
                    .to_string(),
                &[],
                vec![missing("<SRCDIR>/index.rst:10", x)],
            ),
            (
                after_include(
                    ".. glossary::\n\n   term\n      See x_.\n\n      .. note::\n\n         \
                     .. _x: missing_\n",
                ),
                &part,
                vec![missing("<SRCDIR>/index.rst:6", x)],
            ),
            (
                after_include(".. glossary::\n\n   term\n      See x_.\n\n      - .. _x: missing_\n"),
                &part,
                vec![missing("part.inc:5", x)],
            ),
            (
                "Title\n=====\n\n.. glossary::\n\n   term\n      See x_.\n\n      .. _x: missing_\n"
                    .to_string(),
                &[],
                vec![missing("<SRCDIR>/index.rst:9", x)],
            ),
        ];
        let mut mismatches = Vec::new();
        for (index, files, expected) in rows {
            let records = crate::transforms::test_support::read_beside(&index, files);
            if records != expected {
                mismatches.push(format!("{index:?}\n{records:#?}\n{expected:#?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// The std domain's `duplicate label` warning is `location=node`
    /// (`SP/sphinx/domains/std/__init__.py:959-965`): Sphinx's
    /// `get_node_location` over docutils' location for the label's node,
    /// as the transforms locate it, not the node's span. A rubric stands
    /// where its block ended (line 7), or has no line after an option
    /// block or in a container without `document`; a bare target in a note
    /// keeps no source (`<unknown>:line`); a bare target after an include
    /// stands at its line in the top-level input, the included lines and
    /// padding spliced in ([`hyperlink_targets_take_docutils_absolute_input_line`]);
    /// one in a glossary definition after an include, at its line in its
    /// file ([`glossary_definition_targets_count_their_own_file_s_lines`]).
    /// Probed on Sphinx 9.1.0 (`-w`), `a.rst` labelling `dup` first.
    #[test]
    fn duplicate_labels_are_located_as_get_node_location_locates_their_node() {
        let warning = |location: &str| {
            format!("{location}: WARNING: duplicate label dup, other instance in <SRCDIR>/a.rst")
        };
        let part = [("part.inc", "One.\n\nTwo.\n")];
        /// `z.rst`, the files beside it and where the warning is located.
        type Row<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
        let rows: [Row; 7] = [
            (
                "Z\n=\n\n.. _dup:\n\n.. rubric:: R\n\nText.\n",
                &[],
                "<SRCDIR>/z.rst:7",
            ),
            (
                "Z\n=\n\n.. _dup:\n\n.. rubric:: R\n   :class: x\n\nText.\n",
                &[],
                "<SRCDIR>/z.rst:",
            ),
            (
                "Z\n=\n\nPara.\n\n   .. _dup:\n\n   .. rubric:: R\n\n   Text.\n",
                &[],
                "<SRCDIR>/z.rst:",
            ),
            (
                "Z\n=\n\n+-------------------+\n| .. _dup:          |\n|                   |\n\
                 | .. rubric:: R     |\n|                   |\n| Text.             |\n\
                 +-------------------+\n",
                &[],
                "<SRCDIR>/z.rst:",
            ),
            (
                "Z\n=\n\n.. note::\n\n   Text.\n\n   .. _dup:\n",
                &[],
                "<unknown>:8",
            ),
            (
                "Z\n=\n\n.. include:: part.inc\n\n.. _dup:\n",
                &[("part.inc", "Part.\n")],
                "<SRCDIR>/z.rst:11",
            ),
            (
                "Z\n=\n\n.. include:: part.inc\n\n.. glossary::\n\n   term\n      Text.\n\n      \
                 .. _dup:\n",
                &part,
                "<SRCDIR>/z.rst:11",
            ),
        ];
        let mut mismatches = Vec::new();
        for (z, files, location) in rows {
            let project = crate::transforms::test_support::merge_project_beside(
                &[("a", ".. _dup:\n\nA\n=\n"), ("z", z)],
                files,
            );
            let expected = [warning(location)];
            if project.records != expected {
                mismatches.push(format!("{z:?}\n{:#?}\n{expected:#?}", project.records));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// A directive's node that docutils returns without a location takes
    /// what `setup_child` stamps on it, the machine's position as it is
    /// appended: after the content's nested parse, the last line's source
    /// and no line (`StringList.info` just past the end), or neither after
    /// a list, which takes the parent's (the section's line); a toctree's
    /// wrapper takes the position its option block left; a code block's
    /// caption container the `<generated text>` its caption was parsed
    /// from; and an epigraph's quote is stamped at its first content line.
    /// The `duplicate label` warning about each prints that. Probed on
    /// Sphinx 9.1.0 (`-w`), `a.rst` labelling `dup` first.
    #[test]
    fn nodes_directives_return_unlocated_take_the_machine_position() {
        let head = "Z\n=\n\n.. _dup:\n\n";
        let generated = format!("<SRCDIR>{}<generated text>:", std::path::MAIN_SEPARATOR);
        let rows = [
            // The caption's '<generated text>' source is made absolute
            // against the project the way os.path.abspath joins it, with
            // the platform's separator.
            (
                ".. code-block:: python\n   :caption: C\n\n   x = 1\n",
                generated.as_str(),
            ),
            (".. toctree::\n   :caption: C\n\n   a\n", "<SRCDIR>/z.rst:"),
            (
                ".. table:: T\n\n   ===  ===\n   a    b\n   ===  ===\n",
                "<SRCDIR>/z.rst:",
            ),
            (".. list-table:: L\n\n   * - x\n", "<SRCDIR>/z.rst:2"),
            (".. csv-table::\n\n   a, b\n", "<SRCDIR>/z.rst:"),
            (".. py:function:: fn()\n\n   Doc.\n", "<SRCDIR>/z.rst:"),
            (".. hlist::\n\n   * a\n   * b\n", "<SRCDIR>/z.rst:2"),
            (".. epigraph::\n\n   Quote.\n", "<SRCDIR>/z.rst:8"),
        ];
        let mut mismatches = Vec::new();
        for (directive, location) in rows {
            let z = format!("{head}{directive}");
            let project = crate::transforms::test_support::merge_project(&[
                ("a", ".. _dup:\n\nA\n=\n"),
                ("z", &z),
            ]);
            let expected = [format!(
                "{location}: WARNING: duplicate label dup, other instance in <SRCDIR>/a.rst"
            )];
            if project.records != expected {
                mismatches.push(format!("{z:?}\n{:#?}\n{expected:#?}", project.records));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// docutils parses each csv-table cell as `StringList(cell.splitlines(),
    /// source=source)` (`SP/docutils/parsers/rst/directives/tables.py:398-399`):
    /// what its nested machine stamps (a paragraph, a footnote, a citation,
    /// an image) stands at the csv source's line `1 +` its index in the
    /// cell, header cells too, so every record located through it prints
    /// line 1 of the document, or of the `:file:` it was read from; the
    /// parse's own records keep the document's lines. Probed on Sphinx
    /// 9.1.0 (`-w`; the consistency check's `Citation [CIT] is not
    /// referenced.` prints line 1 too).
    #[test]
    fn csv_cell_content_is_located_in_the_cell() {
        let at = |source: &str, level: &str, text: &str, category: &str| {
            format!("{source}:1: {level}: {text} [{category}]")
        };
        let doc = "<SRCDIR>/index.rst";
        let unknown = |name: &str| format!("Unknown target name: \"{name}\".");
        /// The document, the files beside it and the expected records.
        type Row<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<String>);
        let rows: Vec<Row> = vec![
            (
                "Para.\n\n.. csv-table::\n   :header: h1, h2\n\n   a, b\n   c, d\n   e, unknown_\n",
                vec![],
                vec![at(doc, "ERROR", &unknown("unknown"), "docutils")],
            ),
            (
                "Para.\n\n.. csv-table::\n   :header: h1, h2\n\n   a, b\n   c, d\n   e, |nope|\n",
                vec![],
                vec![at(doc, "ERROR", "Undefined substitution referenced: \"nope\".", "docutils")],
            ),
            (
                "Para.\n\n.. csv-table::\n\n   a, b\n   e, [#]_ [#]_\n\n.. [#] f\n",
                vec![],
                vec![at(
                    doc,
                    "ERROR",
                    "Too many autonumbered footnote references: only 1 corresponding footnote \
                     available.",
                    "docutils",
                )],
            ),
            (
                ".. csv-table::\n   :header: h1, unknown_\n\n   a, b\n",
                vec![],
                vec![at(doc, "ERROR", &unknown("unknown"), "docutils")],
            ),
            (
                "Para.\n\nMore.\n\n.. csv-table::\n\n   a, b\n   c, \".. [1] note\"\n",
                vec![],
                vec![at(doc, "WARNING", "Footnote [1] is not referenced.", "ref.footnote")],
            ),
            (
                "Para.\n\nMore.\n\n.. csv-table::\n\n   a, b\n   c, \".. image:: nothere.png\"\n",
                vec![],
                vec![at(doc, "WARNING", "image file not readable: nothere.png", "image.not_readable")],
            ),
            (
                "Title\n=====\n\nIntro.\n\n.. csv-table:: Links\n   :header: Name, Link\n\n   \
                 Python, python_\n   Rust, rust_\n\n.. _python: https://python.org\n",
                vec![],
                vec![at(doc, "ERROR", &unknown("rust"), "docutils")],
            ),
            (
                "Para.\n\n.. csv-table::\n   :file: data.csv\n",
                vec![("data.csv", "a, b\nc, unknown_\n")],
                vec![at("data.csv", "ERROR", &unknown("unknown"), "docutils")],
            ),
            // The parse's own records keep the document's lines.
            (
                "Para.\n\n.. csv-table::\n   :header: h1, h2\n\n   a, b\n   c, d\n   e, *unclosed\n",
                vec![],
                vec!["<SRCDIR>/index.rst:6: WARNING: Inline emphasis start-string without \
                      end-string. [docutils]"
                    .to_string()],
            ),
        ];
        let mut mismatches = Vec::new();
        for (index, files, expected) in rows {
            let tmp = tempfile::tempdir().expect("a temporary source directory");
            for (name, text) in &files {
                std::fs::write(tmp.path().join(name), text).expect("a file writes");
            }
            let parse = crate::rst::ParseOptions {
                source_path: tmp.path().join("index.rst").display().to_string(),
                srcdir: Some(tmp.path().to_path_buf()),
                ..crate::transforms::test_support::index_rst()
            };
            let read = crate::read::read_standalone(
                index,
                &parse,
                &crate::transforms::test_support::harness_settings(false),
            );
            let root = format!("{}{}", tmp.path().display(), std::path::MAIN_SEPARATOR);
            let records: Vec<String> = read
                .diagnostics
                .iter()
                .map(|record| record.render(true).replace(&root, "<SRCDIR>/"))
                .collect();
            if records != expected {
                mismatches.push(format!("{index:?}\n{records:#?}\n{expected:#?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// Sphinx's `make_glossary_term` stamps a term with its content item's
    /// 0-based offset (`term.line = lineno`,
    /// `SP/sphinx/domains/std/__init__.py:387-388`, `:518-521`), and the
    /// term's index node with the same: every transform record located
    /// through a term prints the line before the term's own (220, 620,
    /// 850). Probed on Sphinx 9.1.0 (`-w`).
    #[test]
    fn records_located_through_a_glossary_term_take_its_zero_based_offset() {
        let line = |n: u32, text: &str| format!("<SRCDIR>/index.rst:{n}: ERROR: {text} [docutils]");
        let nope = "Undefined substitution referenced: \"nope\".";
        let unknown = "Unknown target name: \"unknown\".";
        let rows: [(&str, Vec<String>); 6] = [
            (
                ".. glossary::\n\n   term |nope|\n      def\n",
                vec![line(2, nope)],
            ),
            (
                ".. glossary::\n\n   term unknown_\n      def\n",
                vec![line(2, unknown)],
            ),
            (
                ".. glossary::\n\n   term [#]_ [#]_\n      def\n\n.. [#] f\n",
                vec![line(
                    2,
                    "Too many autonumbered footnote references: only 1 corresponding \
                     footnote available.",
                )],
            ),
            (
                ".. glossary::\n\n   one\n   two |nope|\n      def\n",
                vec![line(3, nope)],
            ),
            (
                "Para.\n\nMore.\n\n.. glossary::\n\n   term |nope|\n      def\n\n   other \
                 unknown_\n      d\n",
                vec![line(6, nope), line(9, unknown)],
            ),
            (
                "Title\n=====\n\nIntro.\n\n.. glossary::\n\n   API\n      Application \
                 programming interface.\n\n   REST |nope|\n      Representational state \
                 transfer.\n",
                vec![line(10, nope)],
            ),
        ];
        let mut mismatches = Vec::new();
        for (rst, expected) in rows {
            let read = read_snippet(rst, false);
            if read.warnings != expected {
                mismatches.push(format!("{rst:?}\n{:#?}\n{expected:#?}", read.warnings));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// `SphinxDanglingReferences` (850-039) replaces a reference no target
    /// name resolves by a `problematic` holding the reference's rawsource
    /// (`docutils-0212`: `word_`), reported at the reference's location,
    /// the message itself out of the tree. A reference with no rawsource of
    /// its own, an image's `:target:`, has the one
    /// `ApplySourceWorkaround` (010) gave it, its `astext()`: the image's
    /// `alt`, or nothing. docutils then reports every target nothing
    /// referenced as an INFO (`Hyperlink target "unused" is not
    /// referenced.`), which Sphinx keeps out of the stream: the reporter
    /// logs it and the stream never prints it.
    //
    // oracle (Sphinx 9.1.0): as `propagate_targets_chain`; the INFO
    //   records (and the parse's WARNING) from a `Reporter.system_message`
    //   wrapper over the same reads, which prints every level.
    #[test]
    fn dangling_reference_becomes_problematic() {
        let read = read_snippet("See word_ here.\n", true);
        assert_eq!(
            read.body,
            "    <paragraph>\n        See \n        <problematic ids=\"id2\" refid=\"id1\">\n            \
             word_\n         here.\n"
        );
        assert_eq!(read.warnings, error("1", "Unknown target name: \"word\"."));
        assert_eq!(
            read.trace,
            [
                event("SphinxDanglingReferences", "system_message", "id1", false),
                event("SphinxDanglingReferences", "problematic", "id2", true),
            ]
        );

        let read = read_snippet("See word_ here.\n\n.. _unused:\n\nPara.\n", true);
        assert_eq!(read.warnings, error("1", "Unknown target name: \"word\"."));
        assert_eq!(
            read.reported,
            [
                (ERROR, "Unknown target name: \"word\".".to_string()),
                (
                    INFO,
                    "Hyperlink target \"unused\" is not referenced.".to_string()
                ),
            ]
        );
        // Nothing references the two `x` targets or the embedded URI's
        // `y`, yet only `unused` is reported: `dupname()` marks a node
        // referenced (`nodes.py:3036-3041`) and so does the parse an
        // embedded URI's target (`states.py:942`).
        let read = read_snippet(
            ".. _x: http://1\n.. _x: http://2\n\nPara `y <http://u>`_.\n\n.. _unused:\n\nText.\n",
            true,
        );
        assert_eq!(
            read.reported,
            [
                (
                    WARNING,
                    "Duplicate explicit target name: \"x\".".to_string()
                ),
                (
                    INFO,
                    "Hyperlink target \"unused\" is not referenced.".to_string()
                ),
            ]
        );

        for (rst, text) in [
            (
                ".. image:: x.png\n   :target: missing_\n   :alt: Alt text\n",
                "\n        Alt text",
            ),
            (".. image:: x.png\n   :target: missing_\n", ""),
        ] {
            let read = read_snippet(rst, true);
            assert_eq!(
                read.body,
                format!("    <problematic ids=\"id2\" refid=\"id1\">{text}\n"),
                "{rst:?}"
            );
            assert_eq!(
                read.warnings,
                error("", "Unknown target name: \"missing\"."),
                "{rst:?}"
            );
        }
    }

    /// The ids a loose message takes are gone for good ([DS §5.3],
    /// `docutils-0364`): `DanglingReferences` gives the message `id2`, the
    /// next free one, and the `problematic` reuses the reference's `id1`
    /// (`prbid = node['ids'][0]`), which the replaced reference still
    /// names in `document.ids`, so neither `id1` nor `id2` names a node in
    /// the tree.
    //
    // oracle (Sphinx 9.1.0): as `propagate_targets_chain`; the HTML oracle
    //   renders the problematic as `<a href="#id2">`.
    #[test]
    fn loose_messages_consume_ids() {
        let read = read_snippet("Title [1]_ Here\n===============\n\nbody\n", true);
        assert_eq!(
            read.body,
            "    <section ids=\"title-1-here\" names=\"title\\ 1\\ here\">\n        <title>\n            \
             Title \n            <problematic ids=\"id1\" refid=\"id2\">\n                [1]_\n             \
             Here\n        <paragraph>\n            body\n"
        );
        assert_eq!(read.warnings, error("2", "Unknown target name: \"1\"."));
        assert_eq!(
            read.trace,
            [
                event("parse", "footnote_reference", "id1", false),
                event("parse", "section", "title-1-here", true),
                event("SphinxDanglingReferences", "system_message", "id2", false),
            ]
        );
    }

    /// `state.alias_target_before_reference` ([DS §11]): an embedded alias
    /// registers its target before its reference, so `refnames['alias']`
    /// lists the target first. `IndirectHyperlinks` (460) reports the
    /// target (`alias` names nothing) and turns `text_`, the reference to
    /// the target's own name, into a `problematic`; `DanglingReferences`
    /// (850) then reports the two references to `alias`, in the tree's
    /// order.
    //
    // oracle: the read-state fixture's case of that name.
    #[test]
    fn alias_target_before_reference() {
        let read = read_snippet("See `text <alias_>`_ and alias_ and text_.\n", true);
        assert_eq!(
            read.body,
            "    <paragraph>\n        See \n        <problematic ids=\"id4\" refid=\"id3\">\n            \
             `text <alias_>`_\n        <target ids=\"text\" names=\"text\" refname=\"alias\">\n         \
             and \n        <problematic ids=\"id6\" refid=\"id5\">\n            alias_\n         and \n        \
             <problematic ids=\"id2\" refid=\"id1\">\n            text_\n        .\n"
        );
        let mut warnings = error(
            "1",
            "Indirect hyperlink target \"text\" (id=\"text\") refers to target \"alias\", \
             which does not exist.",
        );
        warnings.extend(error("1", "Unknown target name: \"alias\"."));
        warnings.extend(error("1", "Unknown target name: \"alias\"."));
        assert_eq!(read.warnings, warnings);
        assert_eq!(
            read.trace,
            [
                event("parse", "target", "text", true),
                event("IndirectHyperlinks", "system_message", "id1", false),
                event("IndirectHyperlinks", "problematic", "id2", true),
                event("SphinxDanglingReferences", "system_message", "id3", false),
                event("SphinxDanglingReferences", "problematic", "id4", true),
                event("SphinxDanglingReferences", "system_message", "id5", false),
                event("SphinxDanglingReferences", "problematic", "id6", true),
            ]
        );
    }

    /// A transform's message is written to the stream when it is made and
    /// never placed in the tree, even under `keep_warnings = True`, which
    /// keeps the parse's own messages: the inline-markup WARNING stays in
    /// the tree (with its id), the 850 ERROR prints but has no place, and
    /// the stream holds them in creation order, the parse's first.
    //
    // oracle (Sphinx 9.1.0): as `propagate_targets_chain`.
    #[test]
    fn loose_messages_print_but_stay_out_of_the_tree() {
        let read = read_snippet("See word_ here.\n\n*x\n", true);
        assert_eq!(
            read.body,
            "    <paragraph>\n        See \n        <problematic ids=\"id4\" refid=\"id3\">\n            \
             word_\n         here.\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            \
             *\n        x\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"2\" line=\"3\" \
             source=\"<SRCDIR>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            \
             Inline emphasis start-string without end-string.\n"
        );
        let mut warnings = vec![
            "<SRCDIR>/index.rst:3: WARNING: Inline emphasis start-string without end-string. \
             [docutils]"
                .to_string(),
        ];
        warnings.extend(error("1", "Unknown target name: \"word\"."));
        assert_eq!(read.warnings, warnings);
        assert_eq!(
            read.trace,
            [
                event("parse", "system_message", "id1", true),
                event("parse", "problematic", "id2", true),
                event("SphinxDanglingReferences", "system_message", "id3", false),
                event("SphinxDanglingReferences", "problematic", "id4", true),
            ]
        );
        // Without `keep_warnings` the parse's message goes too (999), and
        // the stream is the same.
        let read = read_snippet("See word_ here.\n\n*x\n", false);
        assert!(!read.body.contains("<system_message"), "{}", read.body);
        assert_eq!(read.warnings, warnings);
    }

    /// An id whose node the parse dropped (a `:name:`d image in a rejected
    /// `table`'s content) stays in docutils' `document.ids`, so it is a
    /// target like any other: `DanglingReferences` (850) resolves `pic_`
    /// to it and `IndirectHyperlinks` (460) points `alias` at it, with no
    /// message.
    //
    // oracle (Sphinx 9.1.0): as `propagate_targets_chain`.
    #[test]
    fn dropped_node_ids_resolve_as_detached_nodes() {
        let read = read_snippet(
            ".. table:: Cap\n\n   .. image:: x.png\n      :name: pic\n\nSee pic_.\n\n\
             .. _alias: pic_\n\nAnd alias_.\n",
            true,
        );
        let body = read
            .body
            .split_once("\n    <paragraph>\n")
            .map(|(_, rest)| rest)
            .expect("a paragraph after the rejected table");
        assert_eq!(
            body,
            "        See \n        <reference name=\"pic\" refid=\"pic\">\n            pic\n        .\n    \
             <target ids=\"alias\" names=\"alias\" refid=\"pic\">\n    <paragraph>\n        And \n        \
             <reference name=\"alias\" refid=\"pic\">\n            alias\n        .\n"
        );
        assert_eq!(read.warnings.len(), 6, "{:#?}", read.warnings);
        assert!(read.warnings[0].contains("Error parsing content block for the \"table\""));
    }

    /// An embedded alias with no text takes the normalized alias as its
    /// text and name (`phrase_ref`, `states.py:946-949`), so
    /// `` `<Foo Bar_>`_ `` is `foo bar`, and resolves through its target
    /// (460) like any other: to the explicit target that took the name.
    //
    // oracle (Sphinx 9.1.0): as `propagate_targets_chain`.
    #[test]
    fn embedded_alias_without_text_resolves_by_its_normalized_name() {
        let read = read_snippet("`<Foo Bar_>`_\n\n.. _foo bar: http://x\n", true);
        assert_eq!(
            read.body,
            "    <paragraph>\n        <reference name=\"foo bar\" refuri=\"http://x\">\n            \
             foo bar\n        <target dupnames=\"foo\\ bar\" ids=\"foo-bar\" refuri=\"http://x\">\n    \
             <target ids=\"id1\" names=\"foo\\ bar\" refuri=\"http://x\">\n"
        );
        assert!(read.warnings.is_empty(), "{:#?}", read.warnings);
    }
    /// `Substitutions` (220-004, `SP/docutils/transforms/references.py:642-765`)
    /// replaces each substitution reference of the tree by a deep copy of
    /// its definition's children (`docutils-0568`), finding the definition
    /// by its name, or by the name lowercased among the normalized names,
    /// where the last definition of a name wins. An undefined one is
    /// reported at the reference and becomes a problematic holding its
    /// rawsource (`docutils-0242` = `state.loose_ids_chain`: the three
    /// ERRORs at 220 precede 440's and 850's, and of the ten ids its
    /// messages and problematics take, three stay; `docutils-0243`).
    /// References in a copy are substituted in turn, the definitions' own
    /// included, however many references copy the same definition; a
    /// definition that refers to itself, directly or through another, is
    /// reported where it stands (`detected`, the message taking
    /// the definition's place and names) or at the reference the chain
    /// started from (`referenced`). `ltrim` and `rtrim` strip the
    /// whitespace before and after the reference. A role's `pending_xref`
    /// stays in its definition (`Body.substitution_def` keeps `Inline`
    /// nodes), so the copy has it too. A copied reference keeps
    /// its location rule: an image `:target:`'s is located at the paragraph
    /// the copy stands in, the definition's at the definition. 220 walks the
    /// tree before `GlossarySorter` (500) sorts it
    /// (`state.glossary_sorted_anonymous_refs`).
    #[test]
    fn substitutions_replace_and_report_undefined() {
        let rows = [
        SphinxRead {
            rst: ".. |sub| replace:: replacement text\n\nUse |sub| here.\n",
            body: "    <substitution_definition names=\"sub\">\n        replacement text\n    \
                <paragraph>\n        Use \n        replacement text\n         here.\n",
            warnings: &[],
            trace: &[],
        },
        SphinxRead {
            rst: "A |sub| B |sub2|_ C |sub3|__ D.\n",
            body: "    <paragraph>\n        A \n        <problematic ids=\"id2\" \
                refid=\"id1\">\n            |sub|\n         B \n        <problematic ids=\"id10\" \
                refid=\"id9\">\n            |sub2|_\n         C \n        <problematic ids=\"id8\" \
                refid=\"id7\">\n            |sub3|__\n         D.\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Undefined substitution referenced: \"sub\". \
                    [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Undefined substitution referenced: \"sub2\". \
                    [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Undefined substitution referenced: \"sub3\". \
                    [docutils]",
                "<SRCDIR>/index.rst:2: ERROR: Anonymous hyperlink mismatch: 1 references but 0 \
                    targets.",
                "See \"backrefs\" attribute for IDs. [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"sub2\". [docutils]",
            ],
            trace: &[
                ("Substitutions", "system_message", "id1", false),
                ("Substitutions", "problematic", "id2", true),
                ("Substitutions", "system_message", "id3", false),
                ("Substitutions", "problematic", "id4", false),
                ("Substitutions", "system_message", "id5", false),
                ("Substitutions", "problematic", "id6", false),
                ("AnonymousHyperlinks", "system_message", "id7", false),
                ("AnonymousHyperlinks", "problematic", "id8", true),
                ("SphinxDanglingReferences", "system_message", "id9", false),
                ("SphinxDanglingReferences", "problematic", "id10", true),
            ],
        },
        SphinxRead {
            rst: "A |Sub Name|_ B.\n",
            body: "    <paragraph>\n        A \n        <problematic ids=\"id4\" \
                refid=\"id3\">\n            |Sub Name|_\n         B.\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Undefined substitution referenced: \"Sub Name\". \
                    [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"sub name\". [docutils]",
            ],
            trace: &[
                ("Substitutions", "system_message", "id1", false),
                ("Substitutions", "problematic", "id2", false),
                ("SphinxDanglingReferences", "system_message", "id3", false),
                ("SphinxDanglingReferences", "problematic", "id4", true),
            ],
        },
        SphinxRead {
            rst: "Use |a| here.\n\n.. |a| replace:: x |b| y\n.. |b| replace:: *B* ref_\n",
            body: "    <paragraph>\n        Use \n        x \n        <emphasis>\n            \
                B\n         \n        <problematic ids=\"id2\" refid=\"id1\">\n            \
                ref_\n         y\n         here.\n    <substitution_definition \
                names=\"a\">\n        x \n        <emphasis>\n            B\n         \n        \
                <problematic ids=\"id4\" refid=\"id3\">\n            ref_\n         y\n    \
                <substitution_definition names=\"b\">\n        <emphasis>\n            \
                B\n         \n        <problematic ids=\"id6\" refid=\"id5\">\n            ref_\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Unknown target name: \"ref\". [docutils]",
                "<SRCDIR>/index.rst:3: ERROR: Unknown target name: \"ref\". [docutils]",
                "<SRCDIR>/index.rst:4: ERROR: Unknown target name: \"ref\". [docutils]",
            ],
            trace: &[
                ("SphinxDanglingReferences", "system_message", "id1", false),
                ("SphinxDanglingReferences", "problematic", "id2", true),
                ("SphinxDanglingReferences", "system_message", "id3", false),
                ("SphinxDanglingReferences", "problematic", "id4", true),
                ("SphinxDanglingReferences", "system_message", "id5", false),
                ("SphinxDanglingReferences", "problematic", "id6", true),
            ],
        },
        SphinxRead {
            rst: "Use |a| here.\n\n.. |a| replace:: x |a| y\n",
            body: "    <paragraph>\n        Use \n        x \n        <problematic ids=\"id2\" \
                refid=\"id1\">\n            |a|\n         y\n         here.\n    <system_message \
                level=\"3\" line=\"3\" names=\"a\" source=\"<SRCDIR>/index.rst\" \
                type=\"ERROR\">\n        <paragraph>\n            Circular substitution definition \
                detected:\n        <literal_block xml:space=\"preserve\">\n            .. |a| \
                replace:: x |a| y\n",
            warnings: &[
                "<SRCDIR>/index.rst:3: ERROR: Circular substitution definition detected:",
                "",
                ".. |a| replace:: x |a| y [docutils]",
                "<SRCDIR>/index.rst:1: ERROR: Circular substitution definition referenced: \"a\". \
                    [docutils]",
            ],
            trace: &[
                ("Substitutions", "system_message", "id1", false),
                ("Substitutions", "problematic", "id2", true),
            ],
        },
        SphinxRead {
            rst: "Use |a| here.\n\n.. |a| replace:: x |b| y\n.. |b| replace:: z |a| w\n",
            body: "    <paragraph>\n        Use \n        x \n        <problematic ids=\"id2\" \
                refid=\"id1\">\n            |b|\n         y\n         here.\n    <system_message \
                level=\"3\" line=\"3\" names=\"a\" source=\"<SRCDIR>/index.rst\" \
                type=\"ERROR\">\n        <paragraph>\n            Circular substitution definition \
                detected:\n        <literal_block xml:space=\"preserve\">\n            .. |a| \
                replace:: x |b| y\n    <system_message level=\"3\" line=\"4\" names=\"b\" source=\"\
                <SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Circular \
                substitution definition detected:\n        <literal_block \
                xml:space=\"preserve\">\n            .. |b| replace:: z |a| w\n",
            warnings: &[
                "<SRCDIR>/index.rst:1: ERROR: Circular substitution definition referenced: \"b\". \
                    [docutils]",
                "<SRCDIR>/index.rst:3: ERROR: Circular substitution definition detected:",
                "",
                ".. |a| replace:: x |b| y [docutils]",
                "<SRCDIR>/index.rst:4: ERROR: Circular substitution definition detected:",
                "",
                ".. |b| replace:: z |a| w [docutils]",
            ],
            trace: &[
                ("Substitutions", "system_message", "id1", false),
                ("Substitutions", "problematic", "id2", true),
            ],
        },
        SphinxRead {
            rst: "A |tm| B |tm2|.\n\n.. |tm| unicode:: U+2122\n   :ltrim:\n.. |tm2| unicode:: \
                U+2122\n   :trim:\n",
            body: "    <paragraph>\n        A\n        ™\n         B\n        ™\n        \
                .\n    <substitution_definition ltrim=\"1\" names=\"tm\">\n        ™\n    \
                <substitution_definition ltrim=\"1\" names=\"tm2\" rtrim=\"1\">\n        ™\n",
            warnings: &[],
            trace: &[],
        },
        // The trims strip the Text's null-escaped data: an escaped space
        // ending the text before goes, and an escaped no-break space
        // opening the text after stays, its escape standing first.
        SphinxRead {
            rst: "A\\ |t| B\n\n.. |t| unicode:: U+2014\n   :trim:\n",
            body: "    <paragraph>\n        A\n        —\n        B\n    \
                <substitution_definition ltrim=\"1\" names=\"t\" rtrim=\"1\">\n        —\n",
            warnings: &[],
            trace: &[],
        },
        SphinxRead {
            rst: "A |t|\\\u{a0}B\n\n.. |t| unicode:: U+2014\n   :trim:\n",
            body: "    <paragraph>\n        A\n        —\n        \u{a0}B\n    \
                <substitution_definition ltrim=\"1\" names=\"t\" rtrim=\"1\">\n        —\n",
            warnings: &[],
            trace: &[],
        },
        SphinxRead {
            rst: "A |Sub  NAME| B.\n\n.. |sub name| replace:: x\n.. |Sub Name| replace:: y\n",
            body: "    <paragraph>\n        A \n        y\n         B.\n    \
                <substitution_definition names=\"sub\\ name\">\n        x\n    \
                <substitution_definition names=\"Sub\\ Name\">\n        y\n",
            warnings: &[],
            trace: &[],
        },
        SphinxRead {
            rst: "Title\n=====\n\nPara |i| x.\n\n.. |i| image:: p.png\n   :target: nope_\n",
            body: "    <section ids=\"title\" names=\"title\">\n        <title>\n            \
                Title\n        <paragraph>\n            Para \n            <problematic \
                ids=\"id2\" refid=\"id1\">\n                i\n             x.\n        \
                <substitution_definition names=\"i\">\n            <problematic ids=\"id4\" \
                refid=\"id3\">\n                i\n",
            warnings: &[
                "<SRCDIR>/index.rst:4: ERROR: Unknown target name: \"nope\". [docutils]",
                "<SRCDIR>/index.rst:6: ERROR: Unknown target name: \"nope\". [docutils]",
            ],
            trace: &[
                ("parse", "section", "title", true),
                ("SphinxDanglingReferences", "system_message", "id1", false),
                ("SphinxDanglingReferences", "problematic", "id2", true),
                ("SphinxDanglingReferences", "system_message", "id3", false),
                ("SphinxDanglingReferences", "problematic", "id4", true),
            ],
        },
        SphinxRead {
            rst: ".. glossary::\n   :sorted:\n\n   zeta\n      Last |zsub| and `x`__.\n\n   \
                alpha\n      First |asub| and `y`__.\n\n__ https://x.example/\n__ \
                https://y.example/\n",
            body: "    <glossary sorted=\"1\">\n        <definition_list \
                classes=\"glossary\">\n            <definition_list_item>\n                <term \
                ids=\"term-alpha\">\n                    alpha\n                    <index \
                entries=\"('single',\\ 'alpha',\\ 'term-alpha',\\ 'main',\\ \
                None)\">\n                <definition>\n                    \
                <paragraph>\n                        First \n                        <problematic \
                ids=\"id6\" refid=\"id5\">\n                            \
                |asub|\n                         and \n                        <reference \
                anonymous=\"1\" name=\"y\" \
                refuri=\"https://y.example/\">\n                            \
                y\n                        .\n            <definition_list_item>\n                \
                <term ids=\"term-zeta\">\n                    zeta\n                    <index \
                entries=\"('single',\\ 'zeta',\\ 'term-zeta',\\ 'main',\\ \
                None)\">\n                <definition>\n                    \
                <paragraph>\n                        Last \n                        <problematic \
                ids=\"id4\" refid=\"id3\">\n                            \
                |zsub|\n                         and \n                        <reference \
                anonymous=\"1\" name=\"x\" \
                refuri=\"https://x.example/\">\n                            \
                x\n                        .\n    <target anonymous=\"1\" ids=\"id1\" \
                refuri=\"https://x.example/\">\n    <target anonymous=\"1\" ids=\"id2\" \
                refuri=\"https://y.example/\">\n",
            warnings: &[
                "<SRCDIR>/index.rst:5: ERROR: Undefined substitution referenced: \"zsub\". \
                    [docutils]",
                "<SRCDIR>/index.rst:8: ERROR: Undefined substitution referenced: \"asub\". \
                    [docutils]",
            ],
            trace: &[
                ("parse", "target", "id1", true),
                ("parse", "target", "id2", true),
                ("Substitutions", "system_message", "id3", false),
                ("Substitutions", "problematic", "id4", true),
                ("Substitutions", "system_message", "id5", false),
                ("Substitutions", "problematic", "id6", true),
            ],
        },
        SphinxRead {
            rst: "Use |y| here.\n\n.. |y| replace:: :ref:`T <nolabel>` and x\n",
            body: "    <paragraph>\n        Use \n        <pending_xref refdoc=\"index\" \
                refdomain=\"std\" refexplicit=\"1\" reftarget=\"nolabel\" reftype=\"ref\" \
                refwarn=\"1\">\n            <inline classes=\"xref std std-ref\">\n                \
                T\n         and x\n         here.\n    <substitution_definition \
                names=\"y\">\n        <pending_xref refdoc=\"index\" refdomain=\"std\" \
                refexplicit=\"1\" reftarget=\"nolabel\" reftype=\"ref\" \
                refwarn=\"1\">\n            <inline classes=\"xref std std-ref\">\n                \
                T\n         and x\n",
            warnings: &[],
            trace: &[],
        },
        SphinxRead {
            rst: "One |a| and |a|.\n\n.. |a| replace:: x |b|\n.. |b| replace:: B\n",
            body: "    <paragraph>\n        One \n        x \n        B\n         \
                and \n        x \n        B\n        .\n    <substitution_definition \
                names=\"a\">\n        x \n        B\n    <substitution_definition \
                names=\"b\">\n        B\n",
            warnings: &[],
            trace: &[],
        },
        SphinxRead {
            rst: "One |a| x.\n\nTwo |a| y.\n\n.. |a| replace:: |b|_\n.. |b| replace:: B text\n\
                .. _b: http://x\n",
            body: "    <paragraph>\n        One \n        <reference \
                refuri=\"http://x\">\n            B text\n         x.\n    <paragraph>\n        \
                Two \n        <reference refuri=\"http://x\">\n            B text\n         \
                y.\n    <substitution_definition names=\"a\">\n        <reference \
                refuri=\"http://x\">\n            B text\n    <substitution_definition \
                names=\"b\">\n        B text\n    <target ids=\"b\" names=\"b\" \
                refuri=\"http://x\">\n",
            warnings: &[],
            trace: &[
                ("parse", "target", "b", true),
            ],
        },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// Three inputs on which Sphinx 9.1.0 crashes in Substitutions (220,
    /// decision 13), each where docutils raises, after the records it
    /// logged before: a definition that refers to an undefined
    /// substitution (`normed[...]` raises `KeyError` while the referring
    /// reference's copy is made, at that reference); a definition reported
    /// circular a second time after the first message took its place
    /// (`replace_self` raises `ValueError`, naming the definition by
    /// docutils' `repr`, its children's short reprs cut once they pass 60
    /// characters, a Text's once it passes 18); and a copy a circular
    /// reference stopped before it was placed, reached through a reference
    /// it holds (`replace_self` on a node with no parent raises
    /// `AttributeError`), at the definition it copies.
    //
    // oracle (Sphinx 9.1.0): each input built as the project's index.
    #[test]
    fn substitutions_crash_where_sphinx_does() {
        let crash = |rst: &str| {
            let read = crate::read::read_standalone(
                rst,
                &crate::transforms::test_support::index_rst(),
                &crate::transforms::test_support::harness_settings(true),
            );
            let records: Vec<String> = read
                .diagnostics
                .iter()
                .map(|record| record.render(true))
                .collect();
            let crash = read.outcome.state().crash.clone().expect("Sphinx crashes");
            (records, crash.to_string(), crash.rendered_location)
        };
        let at = |line: u32| Some(format!("<SRCDIR>/index.rst:{line}"));
        assert_eq!(
            crash("Use |a| here.\n\n.. |a| replace:: x |Nope| y\n"),
            (Vec::new(), "KeyError: 'nope'".to_string(), at(1))
        );
        let detected = |line: u32, definition: &str| {
            format!(
                "<SRCDIR>/index.rst:{line}: ERROR: Circular substitution definition detected:\n\n\
                 {definition} [docutils]"
            )
        };
        let referenced = |name: &str, line: u32| {
            format!(
                "<SRCDIR>/index.rst:{line}: ERROR: Circular substitution definition referenced: \
                 \"{name}\". [docutils]"
            )
        };
        let b = ".. |b| replace:: |a| |b|";
        assert_eq!(
            crash(
                "P3 |b| q.\n\nP0 |b| q.\n\n.. |b| replace:: |a| |b|\n.. |a| replace:: |b| |b| t1\n"
            ),
            (
                vec![referenced("b", 3), detected(5, b), detected(5, b)],
                "ValueError: <substitution_definition \"b\": <substitution_reference...><#text: \
                 ' '><substitution_ref ...> is not in list"
                    .to_string(),
                at(5)
            )
        );
        let c = ".. |c| replace:: a long text of many words |c| |c|";
        assert_eq!(
            crash(&format!("P0 |c| q.\n\n{c}\n")),
            (
                vec![referenced("c", 1), detected(3, c), detected(3, c)],
                "ValueError: <substitution_definition \"c\": <#text: 'a long text of ...'>\
                 <substitution_reference...> ...> is not in list"
                    .to_string(),
                at(3)
            )
        );
        let a = ".. |a| replace:: |b| |a|";
        assert_eq!(
            crash("P0 |a| q.\n\nP1 |a| q.\n\n.. |a| replace:: |b| |a|\n.. |b| replace:: |b|\n"),
            (
                vec![
                    referenced("a", 3),
                    detected(5, a),
                    detected(6, ".. |b| replace:: |b|"),
                    referenced("b", 1),
                    referenced("a", 1),
                    detected(5, a),
                ],
                "AttributeError: 'NoneType' object has no attribute 'replace'".to_string(),
                at(5)
            )
        );
    }

    /// A substitution's copy notes each top-level reference that has a
    /// `refname` (`note_refname`), after the definition's own, which the
    /// parse noted, so `refnames['indirect']` lists the definition's
    /// reference first and then the copies in the order 220 made them
    /// (`state.subst_copy_refnames_order`, [DS §4.3 (f)]): when
    /// `IndirectHyperlinks` (460) turns them into problematics, the
    /// definition's takes `id2`, then the two copies `id3` and `id4`.
    #[test]
    fn subst_copies_allocate_ids_in_docutils_order() {
        let rows = [SphinxRead {
            rst: "One |s| two |s|.\n\n.. _indirect: missing_\n\n.. |s| replace:: indirect_\n",
            body: "    <paragraph>\n        One \n        <problematic ids=\"id3\" \
                refid=\"id1\">\n            indirect_\n         two \n        <problematic \
                ids=\"id4\" refid=\"id1\">\n            indirect_\n        .\n    <target \
                ids=\"indirect\" names=\"indirect\" refname=\"missing\">\n    \
                <substitution_definition names=\"s\">\n        <problematic ids=\"id2\" \
                refid=\"id1\">\n            indirect_\n",
            warnings: &[
                "<SRCDIR>/index.rst:3: ERROR: Indirect hyperlink target \"indirect\" \
                    (id=\"indirect\") refers to target \"missing\", which does not exist. \
                    [docutils]",
            ],
            trace: &[
                ("parse", "target", "indirect", true),
                ("IndirectHyperlinks", "system_message", "id1", false),
                ("IndirectHyperlinks", "problematic", "id2", true),
                ("IndirectHyperlinks", "problematic", "id3", true),
                ("IndirectHyperlinks", "problematic", "id4", true),
            ],
        }];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// `TargetNotes` (540, `SP/docutils/transforms/references.py:767-847`)
    /// gives each external target some node refers to by name a footnote,
    /// in document order, then each anonymous reference to a `refuri`;
    /// one `refuri` is one footnote, whose name is `TARGET_NOTE: idN`, its
    /// own id taken before its references' (`target_notes_basic`: the
    /// embedded URI and the unreferenced `alias` get none). A space and a
    /// `footnote_reference` follow each referring node that is not a
    /// target, with `:class:` on both, the space then in an `inline`. The
    /// footnotes take the pending node's place and Footnotes (620) numbers
    /// them after the parse's auto-numbered ones. A footnote only a target
    /// refers to has no reference, and 622 reports it
    /// (`tn_only_indirect`); a second directive makes footnotes of its
    /// own, whose references go in first (`tn_two`); the reference in a
    /// substitution definition and its copy each get one (`tn_subst`).
    //
    // oracle (Sphinx 9.1.0, docutils 0.22.4): each input read as
    //   `index.rst` through tools/gen_read_state_fixture.py's harness
    //   (`keep_warnings = True`; scratchpad/t18b/probes, rows named after
    //   their case in fixture_cases.json and edge_cases.json).
    #[test]
    fn target_notes_footnotes() {
        let rows = [
            // target_notes_basic
            SphinxRead {
                rst: "Title\n=====\n\nSee Python_ and Docutils_ and Python_ again, and an \
                         anonymous__ link and\nan `embedded <https://embedded.example/>`_ \
                         one; unused_ is not referenced.\n\n.. _Python: \
                         https://www.python.org/\n.. _Docutils: \
                         https://docutils.sourceforge.io/\n.. _unused: \
                         https://unused.example/\n.. __: https://anon.example/\n.. _alias: \
                         Python_\n\nNotes\n-----\n\n.. target-notes::\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <paragraph>\n            See \
                          \n            <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id3\" refid=\"id2\">\n                \
                          1\n             and \n            <reference name=\"Docutils\" \
                          refuri=\"https://docutils.sourceforge.io/\">\n                \
                          Docutils\n             \n            <footnote_reference \
                          auto=\"1\" docname=\"index\" ids=\"id6\" \
                          refid=\"id5\">\n                2\n             and \n            \
                          <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id4\" refid=\"id2\">\n                \
                          1\n             again, and an \n            <reference \
                          anonymous=\"1\" name=\"anonymous\" \
                          refuri=\"https://anon.example/\">\n                \
                          anonymous\n             \n            <footnote_reference \
                          auto=\"1\" docname=\"index\" ids=\"id10\" \
                          refid=\"id9\">\n                4\n             link \
                          and\n            an \n            <reference name=\"embedded\" \
                          refuri=\"https://embedded.example/\">\n                \
                          embedded\n            <target ids=\"embedded\" names=\"embedded\" \
                          refuri=\"https://embedded.example/\">\n             one; \
                          \n            <reference name=\"unused\" \
                          refuri=\"https://unused.example/\">\n                \
                          unused\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id8\" refid=\"id7\">\n                \
                          3\n             is not referenced.\n        <target \
                          ids=\"python\" names=\"python\" \
                          refuri=\"https://www.python.org/\">\n        <target \
                          ids=\"docutils\" names=\"docutils\" \
                          refuri=\"https://docutils.sourceforge.io/\">\n        <target \
                          ids=\"unused\" names=\"unused\" \
                          refuri=\"https://unused.example/\">\n        <target \
                          anonymous=\"1\" ids=\"id1\" \
                          refuri=\"https://anon.example/\">\n        <target ids=\"alias\" \
                          names=\"alias\" refuri=\"https://www.python.org/\">\n        \
                          <section ids=\"notes\" names=\"notes\">\n            \
                          <title>\n                Notes\n            <footnote auto=\"1\" \
                          backrefs=\"id3 id4\" docname=\"index\" ids=\"id2\" \
                          names=\"TARGET_NOTE:\\ id2\">\n                \
                          <label>\n                    1\n                \
                          <paragraph>\n                    <reference \
                          refuri=\"https://www.python.org/\">\n                        \
                          https://www.python.org/\n            <footnote auto=\"1\" \
                          backrefs=\"id6\" docname=\"index\" ids=\"id5\" \
                          names=\"TARGET_NOTE:\\ id5\">\n                \
                          <label>\n                    2\n                \
                          <paragraph>\n                    <reference \
                          refuri=\"https://docutils.sourceforge.io/\">\n                        \
                          https://docutils.sourceforge.io/\n            <footnote \
                          auto=\"1\" backrefs=\"id8\" docname=\"index\" ids=\"id7\" \
                          names=\"TARGET_NOTE:\\ id7\">\n                \
                          <label>\n                    3\n                \
                          <paragraph>\n                    <reference \
                          refuri=\"https://unused.example/\">\n                        \
                          https://unused.example/\n            <footnote auto=\"1\" \
                          backrefs=\"id10\" docname=\"index\" ids=\"id9\" \
                          names=\"TARGET_NOTE:\\ id9\">\n                \
                          <label>\n                    4\n                \
                          <paragraph>\n                    <reference \
                          refuri=\"https://anon.example/\">\n                        \
                          https://anon.example/\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "target", "embedded", true),
                    ("parse", "target", "python", true),
                    ("parse", "target", "docutils", true),
                    ("parse", "target", "unused", true),
                    ("parse", "target", "id1", true),
                    ("parse", "target", "alias", true),
                    ("parse", "section", "notes", true),
                    ("TargetNotes", "footnote", "id2", true),
                    ("TargetNotes", "footnote_reference", "id3", true),
                    ("TargetNotes", "footnote_reference", "id4", true),
                    ("TargetNotes", "footnote", "id5", true),
                    ("TargetNotes", "footnote_reference", "id6", true),
                    ("TargetNotes", "footnote", "id7", true),
                    ("TargetNotes", "footnote_reference", "id8", true),
                    ("TargetNotes", "footnote", "id9", true),
                    ("TargetNotes", "footnote_reference", "id10", true),
                ],
            },
            // target_notes_class
            SphinxRead {
                rst: "Title\n=====\n\nSee Python_ here.\n\n.. _Python: \
                         https://www.python.org/\n\n.. target-notes::\n   :class: custom\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <paragraph>\n            See \
                          \n            <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n            <inline classes=\"custom\">\n                 \
                          \n            <footnote_reference auto=\"1\" classes=\"custom\" \
                          docname=\"index\" ids=\"id2\" refid=\"id1\">\n                \
                          1\n             here.\n        <target ids=\"python\" \
                          names=\"python\" refuri=\"https://www.python.org/\">\n        \
                          <footnote auto=\"1\" backrefs=\"id2\" docname=\"index\" \
                          ids=\"id1\" names=\"TARGET_NOTE:\\ id1\">\n            \
                          <label>\n                1\n            \
                          <paragraph>\n                <reference \
                          refuri=\"https://www.python.org/\">\n                    \
                          https://www.python.org/\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "target", "python", true),
                    ("TargetNotes", "footnote", "id1", true),
                    ("TargetNotes", "footnote_reference", "id2", true),
                ],
            },
            // target_notes_dup_uri
            SphinxRead {
                rst: "Title\n=====\n\nSee one_ and two_ and `three \
                         <https://same.example/>`__.\n\n.. _one: https://same.example/\n.. \
                         _two: https://same.example/\n\n.. target-notes::\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <paragraph>\n            See \
                          \n            <reference name=\"one\" \
                          refuri=\"https://same.example/\">\n                \
                          one\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id2\" refid=\"id1\">\n                \
                          1\n             and \n            <reference name=\"two\" \
                          refuri=\"https://same.example/\">\n                \
                          two\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id3\" refid=\"id1\">\n                \
                          1\n             and \n            <reference name=\"three\" \
                          refuri=\"https://same.example/\">\n                \
                          three\n            .\n        <target ids=\"one\" names=\"one\" \
                          refuri=\"https://same.example/\">\n        <target ids=\"two\" \
                          names=\"two\" refuri=\"https://same.example/\">\n        \
                          <footnote auto=\"1\" backrefs=\"id2 id3\" docname=\"index\" \
                          ids=\"id1\" names=\"TARGET_NOTE:\\ id1\">\n            \
                          <label>\n                1\n            \
                          <paragraph>\n                <reference \
                          refuri=\"https://same.example/\">\n                    \
                          https://same.example/\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "target", "one", true),
                    ("parse", "target", "two", true),
                    ("TargetNotes", "footnote", "id1", true),
                    ("TargetNotes", "footnote_reference", "id2", true),
                    ("TargetNotes", "footnote_reference", "id3", true),
                ],
            },
            // target_notes_with_footnotes
            SphinxRead {
                rst: "Title\n=====\n\nManual [#a]_ and Python_.\n\n.. [#a] Auto \
                         footnote.\n\n.. _Python: https://www.python.org/\n\n.. \
                         target-notes::\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <paragraph>\n            \
                          Manual \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id1\" refid=\"a\">\n                \
                          1\n             and \n            <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id3\" refid=\"id2\">\n                \
                          2\n            .\n        <footnote auto=\"1\" backrefs=\"id1\" \
                          docname=\"index\" ids=\"a\" names=\"a\">\n            \
                          <label>\n                1\n            \
                          <paragraph>\n                Auto footnote.\n        <target \
                          ids=\"python\" names=\"python\" \
                          refuri=\"https://www.python.org/\">\n        <footnote auto=\"1\" \
                          backrefs=\"id3\" docname=\"index\" ids=\"id2\" \
                          names=\"TARGET_NOTE:\\ id2\">\n            \
                          <label>\n                2\n            \
                          <paragraph>\n                <reference \
                          refuri=\"https://www.python.org/\">\n                    \
                          https://www.python.org/\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "footnote_reference", "id1", true),
                    ("parse", "footnote", "a", true),
                    ("parse", "target", "python", true),
                    ("TargetNotes", "footnote", "id2", true),
                    ("TargetNotes", "footnote_reference", "id3", true),
                ],
            },
            // tn_only_indirect
            SphinxRead {
                rst: "Title\n=====\n\n.. _Python: https://www.python.org/\n.. _alias: \
                         Python_\n\n.. target-notes::\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <target ids=\"python\" \
                          names=\"python\" refuri=\"https://www.python.org/\">\n        \
                          <target ids=\"alias\" names=\"alias\" \
                          refuri=\"https://www.python.org/\">\n        <footnote auto=\"1\" \
                          docname=\"index\" ids=\"id1\" names=\"TARGET_NOTE:\\ \
                          id1\">\n            <label>\n                1\n            \
                          <paragraph>\n                <reference \
                          refuri=\"https://www.python.org/\">\n                    \
                          https://www.python.org/\n",
                warnings: &[
                    "<SRCDIR>/index.rst:2: WARNING: Footnote [#] is not referenced. \
                        [ref.footnote]",
                ],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "target", "python", true),
                    ("parse", "target", "alias", true),
                    ("TargetNotes", "footnote", "id1", true),
                ],
            },
            // tn_two
            SphinxRead {
                rst: "Title\n=====\n\nSee Python_.\n\n.. _Python: \
                         https://www.python.org/\n\n.. target-notes::\n\n.. target-notes::\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <paragraph>\n            See \
                          \n            <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id4\" refid=\"id3\">\n                \
                          2\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id2\" refid=\"id1\">\n                \
                          1\n            .\n        <target ids=\"python\" names=\"python\" \
                          refuri=\"https://www.python.org/\">\n        <footnote auto=\"1\" \
                          backrefs=\"id2\" docname=\"index\" ids=\"id1\" \
                          names=\"TARGET_NOTE:\\ id1\">\n            \
                          <label>\n                1\n            \
                          <paragraph>\n                <reference \
                          refuri=\"https://www.python.org/\">\n                    \
                          https://www.python.org/\n        <footnote auto=\"1\" \
                          backrefs=\"id4\" docname=\"index\" ids=\"id3\" \
                          names=\"TARGET_NOTE:\\ id3\">\n            \
                          <label>\n                2\n            \
                          <paragraph>\n                <reference \
                          refuri=\"https://www.python.org/\">\n                    \
                          https://www.python.org/\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "target", "python", true),
                    ("TargetNotes", "footnote", "id1", true),
                    ("TargetNotes", "footnote_reference", "id2", true),
                    ("TargetNotes", "footnote", "id3", true),
                    ("TargetNotes", "footnote_reference", "id4", true),
                ],
            },
            // tn_subst
            SphinxRead {
                rst: "Title\n=====\n\nSee |s|.\n\n.. |s| replace:: Python_\n\n.. _Python: \
                         https://www.python.org/\n\n.. target-notes::\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <paragraph>\n            See \
                          \n            <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id3\" refid=\"id1\">\n                \
                          1\n            .\n        <substitution_definition \
                          names=\"s\">\n            <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id2\" refid=\"id1\">\n                \
                          1\n        <target ids=\"python\" names=\"python\" \
                          refuri=\"https://www.python.org/\">\n        <footnote auto=\"1\" \
                          backrefs=\"id2 id3\" docname=\"index\" ids=\"id1\" \
                          names=\"TARGET_NOTE:\\ id1\">\n            \
                          <label>\n                1\n            \
                          <paragraph>\n                <reference \
                          refuri=\"https://www.python.org/\">\n                    \
                          https://www.python.org/\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "target", "python", true),
                    ("TargetNotes", "footnote", "id1", true),
                    ("TargetNotes", "footnote_reference", "id2", true),
                    ("TargetNotes", "footnote_reference", "id3", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }
}
