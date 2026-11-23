//! docutils' hyperlink-target transforms (`SP/docutils/transforms/references.py`)
//! and Sphinx's `SphinxDanglingReferences`
//! (`SP/sphinx/transforms/references.py:18-30`), ported one row of the
//! read-transform table at a time ([`super::READ_TRANSFORMS`]).
//!
//! They resolve a document's references in four passes over its targets —
//! internal targets propagated to the node they label (260), anonymous
//! references paired with anonymous targets (440), indirect targets
//! followed to what they refer to (460), then the references to external
//! (640) and internal (660) targets — and report every reference still
//! dangling (850). A reference or target they cannot resolve becomes a
//! `problematic` holding its rawsource, which points at the message; the
//! message itself is written to the stream and never placed in the tree,
//! though its id is taken for good ([DS §5.2-5.5]).
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

use std::collections::HashSet;

use super::tree::{Findall, TxId, TxNode, TxTree};
use super::{Transform, TransformCx};
use crate::doctree::messages::{self, ERROR, INFO};
use crate::doctree::{is_text_element, kinds, AttrValue};

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
fn problematic(tree: &mut TxTree, rawsource: &str, msgid: &str) -> TxId {
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

/// The allocating problematic shape of AnonymousHyperlinks and
/// IndirectHyperlinks ([DS §5.5]): the problematic for `reference` takes
/// an id of its own (`prbid = set_id(prb)`), the message `message` a
/// backref to it, and it replaces the reference, whose ids it takes after
/// its own.
fn replace_with_problematic(cx: &mut TransformCx<'_>, reference: TxId, message: TxId, msgid: &str) {
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
/// child list) and resolves each unresolved `reference` and
/// `footnote_reference` with a `refname` through `document.nameids`
/// ([`visit_reference`]). Then every target nothing referenced, but an
/// anonymous one, is reported as an INFO, which Sphinx keeps out of the
/// stream by raising the reporter's level for the duration (the stream
/// prints no INFO here at any time, [`super::reporter::render_records`]).
///
/// docutils visits a `citation_reference` the same way, but in Sphinx's
/// read none is left in the tree by then: `CitationReferenceTransform`
/// (619-017) has replaced every one by a `pending_xref`, which the write
/// phase resolves. Until that row is ported (T11), the walk leaves the
/// citation references alone rather than report what Sphinx never does.
pub struct SphinxDanglingReferences;

impl Transform for SphinxDanglingReferences {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        let mut walk = vec![root];
        while let Some(node) = walk.pop() {
            if matches!(
                cx.tree.node(node).kind,
                kinds::REFERENCE | kinds::FOOTNOTE_REFERENCE
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
    use crate::config::BuildConfig;
    use crate::doctree::messages::{ERROR, INFO, WARNING};
    use crate::read::{read_standalone, ReadSettings};
    use crate::rst::ParseOptions;
    use crate::transforms::reporter::LogRecord;

    /// The document line every post-read pformat of `<SRCDIR>/index.rst`
    /// opens with.
    const DOC: &str = "<document source=\"<SRCDIR>/index.rst\" \
                       translation_progress=\"{'total': 0, 'translated': 0}\">\n";

    /// One read of a snippet as the read-state harness reads it.
    struct Read {
        /// The post-read pformat, less [`DOC`].
        body: String,
        /// The warning stream, one entry per line.
        warnings: Vec<String>,
        /// The id trace: `[phase, kind, id, attached]`, the phase's Python
        /// name shortened to its class.
        trace: Vec<(String, &'static str, String, bool)>,
        /// The reporter's records of every level, `(level, text)`.
        reported: Vec<(u8, String)>,
    }

    /// Our read of `rst` as `<SRCDIR>/index.rst`, the way the read-state
    /// fixture reads a snippet (`tools/gen_read_state_fixture.py`: M1's
    /// validation off, smart quotes off, the id trace on), with
    /// `keep_warnings` as given.
    fn read_snippet_with(rst: &str, keep_warnings: bool) -> Read {
        let settings = ReadSettings {
            trace_ids: true,
            ..ReadSettings::from_config(&BuildConfig {
                validate_directives: false,
                keep_warnings,
                ..BuildConfig::default()
            })
        };
        let parse = ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            ..ParseOptions::default()
        };
        let read = read_standalone(rst, &parse, &settings);
        let pformat = read.outcome.doctree().root.pformat();
        let body = pformat
            .strip_prefix(DOC)
            .unwrap_or_else(|| panic!("{rst:?}: unexpected document line:\n{pformat}"))
            .to_string();
        let warnings = read
            .diagnostics
            .iter()
            .flat_map(|diagnostic| {
                diagnostic
                    .render(true)
                    .split('\n')
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
        let state = read.outcome.state();
        let trace = state
            .ids
            .trace()
            .iter()
            .map(|event| {
                let phase = event.phase.rsplit('.').next().unwrap_or_default();
                (
                    phase.to_string(),
                    event.kind,
                    event.id.clone(),
                    event.attached,
                )
            })
            .collect();
        let reported = state
            .log
            .iter()
            .filter_map(|record| match record {
                LogRecord::Reporter { level, text, .. } => Some((*level, text.clone())),
                LogRecord::Logger { .. } => None,
            })
            .collect();
        Read {
            body,
            warnings,
            trace,
            reported,
        }
    }

    /// [`read_snippet_with`] under the read-state fixture's `keep_warnings =
    /// True`.
    fn read_snippet(rst: &str) -> Read {
        read_snippet_with(rst, true)
    }

    fn event(
        phase: &str,
        kind: &'static str,
        id: &str,
        attached: bool,
    ) -> (String, &'static str, String, bool) {
        (phase.to_string(), kind, id.to_string(), attached)
    }

    /// `<SRCDIR>/index.rst:<line>: ERROR: <text> [docutils]`, as the
    /// stream's lines.
    fn error(line: &str, text: &str) -> Vec<String> {
        format!("<SRCDIR>/index.rst:{line}: ERROR: {text} [docutils]")
            .split('\n')
            .map(str::to_string)
            .collect()
    }

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
                 a comment\n    <paragraph>\n        Para.\n    <target ids=\"y\" names=\"y\">\n",
                &[],
            ),
        ];
        let mut mismatches = Vec::new();
        for (rst, body, warnings) in rows {
            let read = read_snippet(rst);
            // The footnote's own shape is the footnote transforms' (T11).
            let ours = match read.body.find("    <footnote ") {
                Some(at) => &read.body[..at],
                None => &read.body,
            };
            let stream: Vec<&str> = read
                .warnings
                .iter()
                .map(String::as_str)
                .filter(|line| !line.ends_with("[ref.footnote]"))
                .collect();
            if ours != body || stream != warnings {
                mismatches.push(format!(
                    "{rst:?}\n--- sphinx 9.1.0 ---\n{body}{warnings:#?}\n--- ours ---\n{ours}{stream:#?}"
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
        let read = read_snippet(".. __: https://example.com/1\n\n__ https://example.com/2\n");
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
        let read = read_snippet("__ https://x/\npara after\n");
        assert_eq!(
            read.warnings[1..],
            error(
                "3",
                "Anonymous hyperlink mismatch: 0 references but 1 targets.\n\
                 See \"backrefs\" attribute for IDs."
            )
        );

        let read = read_snippet("See `a`__ and `b`__.\n\n__ http://a\n");
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
        let read = read_snippet(".. _a: `phrase ref`_\n");
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
            let read = read_snippet(rst);
            if read.body != body || read.warnings != warnings {
                mismatches.push(format!(
                    "{rst:?}\n--- sphinx 9.1.0 ---\n{body}{warnings:#?}\n--- ours ---\n{}{:#?}",
                    read.body, read.warnings
                ));
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
        let read = read_snippet("See word_ here.\n");
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

        let read = read_snippet("See word_ here.\n\n.. _unused:\n\nPara.\n");
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
            let read = read_snippet(rst);
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
        let read = read_snippet("Title [1]_ Here\n===============\n\nbody\n");
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
        let read = read_snippet("See `text <alias_>`_ and alias_ and text_.\n");
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
        let read = read_snippet("See word_ here.\n\n*x\n");
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
        let read = read_snippet_with("See word_ here.\n\n*x\n", false);
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
        let read = read_snippet("`<Foo Bar_>`_\n\n.. _foo bar: http://x\n");
        assert_eq!(
            read.body,
            "    <paragraph>\n        <reference name=\"foo bar\" refuri=\"http://x\">\n            \
             foo bar\n        <target dupnames=\"foo\\ bar\" ids=\"foo-bar\" refuri=\"http://x\">\n    \
             <target ids=\"id1\" names=\"foo\\ bar\" refuri=\"http://x\">\n"
        );
        assert!(read.warnings.is_empty(), "{:#?}", read.warnings);
    }
}
