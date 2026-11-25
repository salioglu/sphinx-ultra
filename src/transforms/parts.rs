//! docutils' document-parts transforms (`SP/docutils/transforms/parts.py`),
//! each the transform a directive defers to its `pending` node, after
//! every static transform of its priority ([`super::schedule`]):
//! `SectNum` (710) numbers the document's sections for a `.. sectnum::`,
//! and `Contents` (720) builds a `.. contents::` table of them. SectNum
//! runs first, so a table lists the numbers too.
//!
//! The settings both read are docutils' defaults, which Sphinx 9.1.0 keeps
//! (`SP/sphinx/environment/__init__.py:59-75`): `sectnum_xform` on, so
//! SectNum numbers the tree itself, `toc_backlinks = 'entry'`, and no
//! `use_latex_toc`, so Contents builds its table.

use std::fmt;

use super::references::is_reference;
use super::state::DocumentState;
use super::tree::{TxId, TxNode, TxTree};
use super::{Transform, TransformCx};
use crate::doctree::pending::PyValue;
use crate::doctree::{kinds, AttrValue};
use crate::rst::{LogLocation, SphinxCrash};

/// The pending node of the row that applies, docutils' `self.startnode`:
/// the node of the `note_pending` record the schedule ran the row for.
/// The arena keeps every identity it ever held, so it is found wherever
/// it is, in the tree or in the detached forest.
fn startnode(cx: &TransformCx<'_>) -> Option<TxId> {
    let pending = cx
        .pending_record()
        .and_then(|record| cx.tree.by_uid(record.node));
    debug_assert!(pending.is_some(), "a pending transform without its node");
    pending
}

/// `pending.details.get(key)`: the value the directive stored under `key`.
fn detail<'a>(pending: &'a TxNode, key: &str) -> Option<&'a PyValue> {
    pending
        .ext
        .as_deref()
        .and_then(|ext| ext.pending.as_deref())
        .and_then(|pending| pending.details.iter().find(|(known, _)| known == key))
        .map(|(_, value)| value)
}

/// `level < depth` for a `depth` detail, `sys.maxsize` when the directive
/// has none. The directive keeps an `int` beyond `i64` as its decimal
/// string (`int_optval`): one is above every level when positive and below
/// every level when negative, as Python compares them.
fn within_depth(level: u32, depth: Option<&PyValue>) -> bool {
    match depth {
        None => true,
        Some(PyValue::Int(depth)) => i64::from(level) < *depth,
        Some(PyValue::Str(digits)) => !digits.starts_with('-'),
        // Both directives convert `depth` to an int.
        Some(_) => {
            debug_assert!(false, "a depth detail that is no int");
            true
        }
    }
}

/// 710 `SectNum` (`parts.py:18-67`): the pending node leaves its parent,
/// and every section of the document, wherever the directive stood, gets
/// its number: its title takes a `generated` node, classed `sectnum`,
/// holding `<prefix><numbers joined by '.'><suffix>` and three U+00A0, as
/// its first child, and `auto = 1`. The top level counts from `:start:`
/// (1 by default), every level below from 1, and the numbering goes down
/// to `:depth:` levels (all by default); the top level is always numbered,
/// whatever the depth. Sphinx's `doctitle_xform` is off, so the page's
/// title is a section like any other: number `1`, the rest under it.
pub struct SectNum;

impl Transform for SectNum {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let Some(pending) = startnode(cx) else {
            return;
        };
        let node = cx.tree.node(pending);
        let text_detail = |key| match detail(node, key) {
            Some(PyValue::Str(text)) => text.clone(),
            _ => String::new(),
        };
        let numbering = Numbering {
            maxdepth: detail(node, "depth").cloned(),
            start: detail(node, "start").map_or_else(SectionNumber::one, SectionNumber::of),
            prefix: text_detail("prefix"),
            suffix: text_detail("suffix"),
        };
        // A pending node with no parent is a root of the detached forest,
        // one docutils has in the anonymous element a directive parsed it
        // into: it leaves that element, out of every tree.
        if let Some(parent) = cx.tree.parent(pending) {
            cx.tree.remove(parent, pending);
        }
        let root = cx.tree.root();
        number_sections(cx.tree, &numbering, root, &[], 0);
    }
}

/// What a SectNum pending node's details ask for.
struct Numbering {
    /// `details.get('depth')`: `None` numbers every level.
    maxdepth: Option<PyValue>,
    /// `details.get('start', 1)`.
    start: SectionNumber,
    /// `details.get('prefix', '')` and `details.get('suffix', '')`.
    prefix: String,
    suffix: String,
}

/// `SectNum.update_section_numbers(node, prefix, depth)`: numbers the
/// section children of the node at `node`, which stands at level `depth`,
/// under the numbers `prefix` of its own section, and below them while the
/// depth allows.
fn number_sections(
    tree: &mut TxTree,
    numbering: &Numbering,
    node: TxId,
    prefix: &[String],
    depth: u32,
) {
    let depth = depth + 1;
    let mut sectnum = if prefix.is_empty() {
        numbering.start.clone()
    } else {
        SectionNumber::one()
    };
    for child in tree.node(node).children.clone() {
        if tree.node(child).kind != kinds::SECTION {
            continue;
        }
        let mut numbers = prefix.to_vec();
        numbers.push(sectnum.to_string());
        // `title = child[0]`: a section opens with its title.
        if let Some(&title) = tree.node(child).children.first() {
            let text = format!(
                "{}{}{}\u{a0}\u{a0}\u{a0}",
                numbering.prefix,
                numbers.join("."),
                numbering.suffix
            );
            let generated = tree.new_node("generated");
            tree.node_mut(generated)
                .attrs
                .classes
                .push("sectnum".to_string());
            let text = new_text(tree, &text);
            tree.append(generated, text);
            tree.insert(title, 0, generated);
            tree.node_mut(title).set("auto", AttrValue::Int(1));
        }
        if within_depth(depth, numbering.maxdepth.as_ref()) {
            number_sections(tree, numbering, child, &numbers, depth);
        }
        sectnum.increment();
    }
}

/// A section number as SectNum counts it: a Python `int`, whatever its
/// size, from `:start:` on, one more for each section (`sectnum += 1`),
/// printed with `str()`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionNumber {
    negative: bool,
    /// The magnitude's decimal digits, most significant first, without
    /// leading zeros: `"0"` for zero, which is never negative.
    digits: String,
}

impl SectionNumber {
    fn one() -> SectionNumber {
        SectionNumber {
            negative: false,
            digits: "1".to_string(),
        }
    }

    /// The `int` a `start` detail holds: an `i64`, or, past its range, the
    /// decimal string the directive kept (`int_optval`).
    fn of(value: &PyValue) -> SectionNumber {
        let number = match value {
            PyValue::Int(number) => SectionNumber::parse(&number.to_string()),
            PyValue::Str(digits) => SectionNumber::parse(digits),
            _ => None,
        };
        // The directive converts `start` to an int.
        debug_assert!(number.is_some(), "a start detail that is no int");
        number.unwrap_or_else(SectionNumber::one)
    }

    /// The number a decimal literal spells (`-` and ASCII digits).
    fn parse(text: &str) -> Option<SectionNumber> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(digits) => (true, digits),
            None => (false, text),
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let digits = digits.trim_start_matches('0');
        let digits = if digits.is_empty() { "0" } else { digits };
        Some(SectionNumber {
            negative: negative && digits != "0",
            digits: digits.to_string(),
        })
    }

    /// `+= 1`.
    fn increment(&mut self) {
        let mut digits = std::mem::take(&mut self.digits).into_bytes();
        if self.negative {
            // -(m - 1): the magnitude, at least 1, goes down by one.
            for digit in digits.iter_mut().rev() {
                if *digit == b'0' {
                    *digit = b'9';
                } else {
                    *digit -= 1;
                    break;
                }
            }
            let zeros = digits.iter().take_while(|&&digit| digit == b'0').count();
            digits.drain(..zeros.min(digits.len() - 1));
            self.negative = digits != b"0";
        } else {
            let carried = digits.iter_mut().rev().all(|digit| {
                if *digit == b'9' {
                    *digit = b'0';
                    true
                } else {
                    *digit += 1;
                    false
                }
            });
            if carried {
                digits.insert(0, b'1');
            }
        }
        self.digits = String::from_utf8(digits).expect("ASCII digits");
    }
}

impl fmt::Display for SectionNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str(&self.digits)
    }
}

/// `nodes.Text(text)`, in no parent.
fn new_text(tree: &mut TxTree, text: &str) -> TxId {
    let node = tree.new_node(kinds::TEXT);
    tree.node_mut(node).text = Some(text.to_string());
    node
}

/// 720 `Contents` (`parts.py:70-151`): the pending node in a contents
/// topic becomes the table: a `bullet_list` with one `list_item` per
/// section of the start node, each holding a `paragraph` with a
/// `reference` to the section (`refid`, its first id) whose children are a
/// [`contents_filter_copy`] of its title, then the list of its own
/// sections, down to `:depth:` levels. The start node is the document, or
/// with `:local:` the section the topic stands in (through a `sidebar` or
/// any other container, up to the document). Each reference takes the next
/// `idN` as it is made, in pre-order, after every id the parse and the
/// transforms before took (TargetNotes' at 540 among them). A list is
/// classed `auto-toc` when its last section's title has SectNum's `auto`.
///
/// Backlinks (`:backlinks:`, docutils' `toc_backlinks` default `entry`):
/// each listed title with no `reference` in it (a `pending_xref` or a
/// footnote reference does not count) takes as its `refid` the id of its
/// entry, or with `top` the topic's first id; with `none` it takes none.
/// A later table overwrites an earlier one's.
///
/// With no section to list, the topic leaves its parent; its id, which the
/// directive registered, stays taken. A topic the parse discarded (a
/// contents directive in `replace` content) is still built: its titles'
/// refids are in the tree, its table is not. With `:local:`, the walk up
/// from such a topic finds no section or document, and Sphinx 9.1.0
/// crashes (decision 13): `AttributeError` at `parts.py:100`.
pub struct Contents;

impl Transform for Contents {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let Some(pending) = startnode(cx) else {
            return;
        };
        // The directive puts the pending node in its topic.
        let topic = cx.tree.parent(pending);
        debug_assert!(topic.is_some(), "a contents pending node out of its topic");
        let Some(topic) = topic else {
            return;
        };
        let node = cx.tree.node(pending);
        let local = detail(node, "local").is_some();
        let backlinks = match detail(node, "backlinks") {
            Some(PyValue::Str(choice)) if choice == "top" => Backlinks::Top,
            Some(PyValue::None) => Backlinks::None,
            // `entry`, the directive's choice or `toc_backlinks`.
            _ => Backlinks::Entry,
        };
        let depth = detail(node, "depth").cloned();
        let start = if local {
            // `startnode.parent.parent`, then up to a section or the
            // document; a walk that runs out of parents raises.
            let mut start = cx.tree.parent(topic);
            while let Some(node) = start {
                if matches!(cx.tree.node(node).kind, kinds::SECTION | kinds::DOCUMENT) {
                    break;
                }
                start = cx.tree.parent(node);
            }
            let Some(start) = start else {
                let crash = SphinxCrash::python(
                    "AttributeError",
                    "'NoneType' object has no attribute 'parent'".to_string(),
                    LogLocation::None,
                );
                cx.crash(crash, pending);
                return;
            };
            start
        } else {
            cx.tree.root()
        };
        // `self.startnode.parent['ids'][0]`: the directive registered the
        // topic, which therefore has an id.
        let toc_id = cx.tree.node(topic).attrs.ids.first().cloned();
        debug_assert!(toc_id.is_some(), "a contents topic without an id");
        let toc = Toc {
            backlinks,
            toc_id: toc_id.unwrap_or_default(),
            depth,
        };
        match build_contents(cx, &toc, start, 0) {
            Some(contents) => cx.tree.replace_self(pending, vec![contents]),
            None => {
                if let Some(parent) = cx.tree.parent(topic) {
                    cx.tree.remove(parent, topic);
                }
            }
        }
    }
}

/// What a contents table's titles link back to (`:backlinks:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backlinks {
    Entry,
    Top,
    None,
}

/// What one contents table is built with.
struct Toc {
    backlinks: Backlinks,
    /// The topic's first id, the `top` backlinks' target.
    toc_id: String,
    /// `details.get('depth', sys.maxsize)`.
    depth: Option<PyValue>,
}

/// `Contents.build_contents(node, level)`: the list of the sections of the
/// node at `node`, which stands at `level`, or `None` when it has none.
fn build_contents(cx: &mut TransformCx<'_>, toc: &Toc, node: TxId, level: u32) -> Option<TxId> {
    let level = level + 1;
    let sections: Vec<TxId> = cx
        .tree
        .node(node)
        .children
        .iter()
        .copied()
        .filter(|&child| cx.tree.node(child).kind == kinds::SECTION)
        .collect();
    let mut entries = Vec::new();
    let mut auto = false;
    for section in sections {
        // `title = section[0]`: a section opens with its title.
        let Some(&title) = cx.tree.node(section).children.first() else {
            continue;
        };
        auto = cx
            .tree
            .node(title)
            .get("auto")
            .is_some_and(AttrValue::is_truthy);
        let entrytext = contents_filter_copy(cx.tree, cx.state, title);
        let reference = cx.tree.new_node(kinds::REFERENCE);
        // `section['ids'][0]`: a section is registered as it is parsed.
        let refid = cx.tree.node(section).attrs.ids.first().cloned();
        debug_assert!(refid.is_some(), "a section without an id");
        cx.tree
            .node_mut(reference)
            .set("refid", AttrValue::Str(refid.unwrap_or_default()));
        for child in entrytext {
            cx.tree.append(reference, child);
        }
        let ref_id = cx.set_id(reference);
        let entry = cx.tree.new_node(kinds::PARAGRAPH);
        cx.tree.append(entry, reference);
        let item = cx.tree.new_node(kinds::LIST_ITEM);
        cx.tree.append(item, entry);
        let linked = cx
            .tree
            .next_node(
                title,
                |node| is_reference(node.kind),
                false,
                true,
                false,
                false,
            )
            .is_some();
        let backlink = match toc.backlinks {
            _ if linked => None,
            Backlinks::Entry => Some(ref_id),
            Backlinks::Top => Some(toc.toc_id.clone()),
            Backlinks::None => None,
        };
        if let Some(backlink) = backlink {
            cx.tree
                .node_mut(title)
                .set("refid", AttrValue::Str(backlink));
        }
        if within_depth(level, toc.depth.as_ref()) {
            if let Some(subsects) = build_contents(cx, toc, section, level) {
                cx.tree.append(item, subsects);
            }
        }
        entries.push(item);
    }
    if entries.is_empty() {
        return None;
    }
    let contents = cx.tree.new_node(kinds::BULLET_LIST);
    for item in entries {
        cx.tree.append(contents, item);
    }
    if auto {
        cx.tree
            .node_mut(contents)
            .attrs
            .classes
            .push("auto-toc".to_string());
    }
    Some(contents)
}

/// docutils' `ContentsFilter` (`parts.py:154-175`), a `TreeCopyVisitor`,
/// over the title at `title`: copies of its children, in no parent, for a
/// contents entry. It is docutils' filter, not Sphinx's toc filter
/// ([`crate::env::toctree`]), which drops images:
///
/// - a `footnote_reference` or a `citation_reference` is left out;
/// - an `image` gives a Text node of its `alt`, or nothing without one;
/// - a `problematic`, a `reference` or a `target` gives its children's
///   copies in its place (a `reference`'s subclasses, `download_reference`
///   and `number_reference`, are copied: the visitor dispatches on a
///   node's own class name);
/// - any other node is copied ([`TxTree::copy`]) with its attributes and
///   its location, and its children filtered into it: a `pending_xref`
///   resolves (and warns) once more, an `index` adds its entries again, a
///   `download_reference` is collected again, SectNum's `generated` shows
///   the number. Adjacent Text nodes stay apart.
pub fn contents_filter_copy(
    tree: &mut TxTree,
    state: &mut DocumentState,
    title: TxId,
) -> Vec<TxId> {
    let mut copies = Vec::new();
    for child in tree.node(title).children.clone() {
        filter_into(tree, state, child, &mut copies);
    }
    copies
}

/// One node's visit in [`contents_filter_copy`]: what it gives, appended
/// to `into`.
fn filter_into(tree: &mut TxTree, state: &mut DocumentState, node: TxId, into: &mut Vec<TxId>) {
    match tree.node(node).kind {
        kinds::CITATION_REFERENCE | kinds::FOOTNOTE_REFERENCE => {}
        kinds::IMAGE => {
            if let Some(AttrValue::Str(alt)) = tree.node(node).get("alt") {
                let alt = alt.clone();
                into.push(new_text(tree, &alt));
            }
        }
        kinds::PROBLEMATIC | kinds::REFERENCE | kinds::TARGET => {
            for child in tree.node(node).children.clone() {
                filter_into(tree, state, child, into);
            }
        }
        _ => {
            let copy = tree.copy(node, state);
            let mut children = Vec::new();
            for child in tree.node(node).children.clone() {
                filter_into(tree, state, child, &mut children);
            }
            for child in children {
                tree.append(copy, child);
            }
            into.push(copy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transforms::test_support::{differences, merge_project, resolved, SphinxRead};

    // oracle (Sphinx 9.1.0, docutils 0.22.4, CPython 3.12.2) for every
    //   SphinxRead row of this module: the input read as `index.rst` through
    //   tools/gen_read_state_fixture.py's harness (`keep_warnings = True`):
    //   the post-read doctree, the warning stream and the id trace
    //   (scratchpad/t18b/probes: snippet_probe.py over fixture_cases.json,
    //   edge_cases.json and more_cases.json; rows named after their case).

    /// Contents (720) builds its table from the sections of the document,
    /// each entry a `reference` to the section's first id, nested by
    /// level, and links each listed title back to its entry
    /// (`toc_backlinks = 'entry'`, docutils' default, which Sphinx keeps)
    /// or with `:backlinks: top` to the topic. A later table overwrites an
    /// earlier one's backlinks, and the second topic is nameless where a
    /// section holds the name (`contents_name_clash`). A table with nothing
    /// to list takes its topic out of the tree, but the id the directive
    /// registered for it stays taken (`contents_removed_topic_label`: the
    /// later label gets `id1`). The entries take their ids as the table is
    /// built, after every id the parse took and after TargetNotes' (540).
    #[test]
    fn contents_topic_structure_and_backlinks() {
        let rows = [
            // contents_basic
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n\nSection A\n---------\n\nText \
                         A.\n\nSub A1\n~~~~~~\n\nText A1.\n\nSection B\n---------\n\nText \
                         B.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id1\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id2\" \
                          refid=\"section-a\">\n                                    Section \
                          A\n                            \
                          <bullet_list>\n                                \
                          <list_item>\n                                    \
                          <paragraph>\n                                        <reference \
                          ids=\"id3\" \
                          refid=\"sub-a1\">\n                                            \
                          Sub A1\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id4\" \
                          refid=\"section-b\">\n                                    Section \
                          B\n        <section ids=\"section-a\" names=\"section\\ \
                          a\">\n            <title refid=\"id2\">\n                Section \
                          A\n            <paragraph>\n                Text A.\n            \
                          <section ids=\"sub-a1\" names=\"sub\\ a1\">\n                \
                          <title refid=\"id3\">\n                    Sub \
                          A1\n                <paragraph>\n                    Text \
                          A1.\n        <section ids=\"section-b\" names=\"section\\ \
                          b\">\n            <title refid=\"id4\">\n                Section \
                          B\n            <paragraph>\n                Text B.\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "section-a", true),
                    ("parse", "section", "sub-a1", true),
                    ("parse", "section", "section-b", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                    ("Contents", "reference", "id3", true),
                    ("Contents", "reference", "id4", true),
                ],
            },
            // contents_backlinks_top
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n   :backlinks: top\n\nSection \
                         A\n---------\n\nText A.\n\nSub A1\n~~~~~~\n\nText A1.\n\nSection \
                         B\n---------\n\nText B.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"contents\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id2\" \
                          refid=\"section-a\">\n                                    Section \
                          A\n                            \
                          <bullet_list>\n                                \
                          <list_item>\n                                    \
                          <paragraph>\n                                        <reference \
                          ids=\"id3\" \
                          refid=\"sub-a1\">\n                                            \
                          Sub A1\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id4\" \
                          refid=\"section-b\">\n                                    Section \
                          B\n        <section ids=\"section-a\" names=\"section\\ \
                          a\">\n            <title refid=\"contents\">\n                \
                          Section A\n            <paragraph>\n                Text \
                          A.\n            <section ids=\"sub-a1\" names=\"sub\\ \
                          a1\">\n                <title \
                          refid=\"contents\">\n                    Sub A1\n                \
                          <paragraph>\n                    Text A1.\n        <section \
                          ids=\"section-b\" names=\"section\\ b\">\n            <title \
                          refid=\"contents\">\n                Section B\n            \
                          <paragraph>\n                Text B.\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "section-a", true),
                    ("parse", "section", "sub-a1", true),
                    ("parse", "section", "section-b", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                    ("Contents", "reference", "id3", true),
                    ("Contents", "reference", "id4", true),
                ],
            },
            // contents_name_clash
            SphinxRead {
                rst: "Contents\n========\n\n.. contents::\n\n.. \
                         contents::\n\nSec\n---\n\nx\n",
                body: "    <section ids=\"contents\" names=\"contents\">\n        <title \
                          refid=\"id5\">\n            Contents\n        <topic \
                          classes=\"contents\" ids=\"id1\">\n            \
                          <title>\n                Contents\n            \
                          <bullet_list>\n                <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id3\" \
                          refid=\"contents\">\n                            \
                          Contents\n                    \
                          <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id4\" refid=\"sec\">\n                                    \
                          Sec\n        <topic classes=\"contents\" \
                          ids=\"id2\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id5\" \
                          refid=\"contents\">\n                            \
                          Contents\n                    \
                          <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id6\" refid=\"sec\">\n                                    \
                          Sec\n        <section ids=\"sec\" names=\"sec\">\n            \
                          <title refid=\"id6\">\n                Sec\n            \
                          <paragraph>\n                x\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "contents", true),
                    ("parse", "topic", "id1", true),
                    ("parse", "topic", "id2", true),
                    ("parse", "section", "sec", true),
                    ("Contents", "reference", "id3", true),
                    ("Contents", "reference", "id4", true),
                    ("Contents", "reference", "id5", true),
                    ("Contents", "reference", "id6", true),
                ],
            },
            // contents_removed_topic_label
            SphinxRead {
                rst: "Para.\n\n.. contents::\n\n.. _contents:\n\nPara two, see contents_.\n",
                body: "    <paragraph>\n        Para.\n    <target refid=\"id1\">\n    \
                          <paragraph ids=\"id1\" names=\"contents\">\n        Para two, see \
                          \n        <reference name=\"contents\" \
                          refid=\"id1\">\n            contents\n        .\n",
                warnings: &[],
                trace: &[
                    ("parse", "topic", "contents", false),
                    ("parse", "target", "id1", true),
                ],
            },
            // contents_after_target_notes
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n\nSee Python_.\n\n.. _Python: \
                         https://www.python.org/\n\n.. target-notes::\n\nSec\n---\n\nx\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id3\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id3\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id4\" refid=\"sec\">\n                                    \
                          Sec\n        <paragraph>\n            See \n            \
                          <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id2\" refid=\"id1\">\n                \
                          1\n            .\n        <target ids=\"python\" names=\"python\" \
                          refuri=\"https://www.python.org/\">\n        <footnote auto=\"1\" \
                          backrefs=\"id2\" docname=\"index\" ids=\"id1\" \
                          names=\"TARGET_NOTE:\\ id1\">\n            \
                          <label>\n                1\n            \
                          <paragraph>\n                <reference \
                          refuri=\"https://www.python.org/\">\n                    \
                          https://www.python.org/\n        <section ids=\"sec\" \
                          names=\"sec\">\n            <title \
                          refid=\"id4\">\n                Sec\n            \
                          <paragraph>\n                x\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "target", "python", true),
                    ("parse", "section", "sec", true),
                    ("TargetNotes", "footnote", "id1", true),
                    ("TargetNotes", "footnote_reference", "id2", true),
                    ("Contents", "reference", "id3", true),
                    ("Contents", "reference", "id4", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// Contents' entry text is docutils' `ContentsFilter` copy of the title
    /// ([`contents_filter_copy`]), not Sphinx's toc filter: a
    /// `pending_xref`, an `index` node and a `download_reference` are
    /// copied, an image gives its `alt`, a `target`, a `reference` and a
    /// `problematic` give their children, and a footnote reference is left
    /// out. A title with a `reference` in it (or a subclass of one: the
    /// `download_reference`) gets no backlink. The copies are read again
    /// by what reads the tree after 720: the download collector warns
    /// twice about the file, and the write phase resolves each
    /// `pending_xref` again, so an unresolved `:ref:` in a listed title
    /// warns twice, identically, and an `:index:` in one adds its entry
    /// twice.
    //
    // oracle: the rows as above; the warnings and the index entries of a
    //   dummy build of each two-section project (structure-directives
    //   research report §3.1, `contents_unresolved_xref_title` and
    //   `contents_index_role_title`).
    #[test]
    fn contents_filter_copies_xrefs_and_index() {
        let rows = [
            // contents_xref_titles
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n\n.. _lab:\n\nSee \
                         :ref:`lab`\n--------------\n\nThe :py:func:`len` \
                         function\n---------------------------\n\nAn ``:abbr:`` :abbr:`LIFO \
                         (last-in, first-out)` \
                         title\n-----------------------------------------------------\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id1\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id2\" \
                          refid=\"see-lab\">\n                                    See \
                          \n                                    <pending_xref \
                          refdoc=\"index\" refdomain=\"std\" refexplicit=\"0\" \
                          reftarget=\"lab\" reftype=\"ref\" \
                          refwarn=\"1\">\n                                        <inline \
                          classes=\"xref std \
                          std-ref\">\n                                            \
                          lab\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id3\" \
                          refid=\"the-len-function\">\n                                    \
                          The \n                                    <pending_xref \
                          py:class=\"True\" py:module=\"True\" refdoc=\"index\" \
                          refdomain=\"py\" refexplicit=\"0\" reftarget=\"len\" \
                          reftype=\"func\" \
                          refwarn=\"0\">\n                                        <literal \
                          classes=\"xref py \
                          py-func\">\n                                            \
                          len()\n                                     \
                          function\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id4\" \
                          refid=\"an-abbr-lifo-title\">\n                                    \
                          An \n                                    \
                          <literal>\n                                        \
                          :abbr:\n                                     \
                          \n                                    <abbreviation \
                          explanation=\"last-in, \
                          first-out\">\n                                        \
                          LIFO\n                                     title\n        <target \
                          refid=\"lab\">\n        <section ids=\"see-lab lab\" \
                          names=\"see\\ lab lab\">\n            <title \
                          refid=\"id2\">\n                See \n                \
                          <pending_xref refdoc=\"index\" refdomain=\"std\" \
                          refexplicit=\"0\" reftarget=\"lab\" reftype=\"ref\" \
                          refwarn=\"1\">\n                    <inline classes=\"xref std \
                          std-ref\">\n                        lab\n        <section \
                          ids=\"the-len-function\" names=\"the\\ len()\\ \
                          function\">\n            <title refid=\"id3\">\n                \
                          The \n                <pending_xref py:class=\"True\" \
                          py:module=\"True\" refdoc=\"index\" refdomain=\"py\" \
                          refexplicit=\"0\" reftarget=\"len\" reftype=\"func\" \
                          refwarn=\"0\">\n                    <literal classes=\"xref py \
                          py-func\">\n                        len()\n                 \
                          function\n        <section ids=\"an-abbr-lifo-title\" \
                          names=\"an\\ :abbr:\\ lifo\\ title\">\n            <title \
                          refid=\"id4\">\n                An \n                \
                          <literal>\n                    :abbr:\n                 \
                          \n                <abbreviation explanation=\"last-in, \
                          first-out\">\n                    LIFO\n                 title\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "target", "lab", true),
                    ("parse", "section", "see-lab", true),
                    ("parse", "section", "the-len-function", true),
                    ("parse", "section", "an-abbr-lifo-title", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                    ("Contents", "reference", "id3", true),
                    ("Contents", "reference", "id4", true),
                ],
            },
            // contents_index_role_title
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n\nThe :index:`spam` \
                         section\n-------------------------\n\nx\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id1\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id2\" \
                          refid=\"the-spam-section\">\n                                    \
                          The \n                                    <index \
                          entries=\"('single',\\ 'spam',\\ 'index-0',\\ '',\\ \
                          None)\">\n                                    \
                          spam\n                                     section\n        \
                          <section ids=\"the-spam-section\" names=\"the\\ spam\\ \
                          section\">\n            <title refid=\"id2\">\n                \
                          The \n                <index entries=\"('single',\\ 'spam',\\ \
                          'index-0',\\ '',\\ None)\">\n                <target \
                          ids=\"index-0\">\n                spam\n                 \
                          section\n            <paragraph>\n                x\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "the-spam-section", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                ],
            },
            // co_image_titles
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n\nWith |a| \
                         alt\n------------\n\nWithout |b| alt\n---------------\n\n.. |a| \
                         image:: a.png\n   :alt: ALT\n.. |b| image:: b.png\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id1\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id2\" \
                          refid=\"with-a-alt\">\n                                    With \
                          \n                                    \
                          ALT\n                                     \
                          alt\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id3\" \
                          refid=\"without-b-alt\">\n                                    \
                          Without \n                                    \
                          b\n                                     alt\n        <section \
                          ids=\"with-a-alt\" names=\"with\\ a\\ alt\">\n            <title \
                          refid=\"id2\">\n                With \n                <image \
                          alt=\"ALT\" candidates=\"{'*': 'a.png'}\" \
                          uri=\"a.png\">\n                 alt\n        <section \
                          ids=\"without-b-alt\" names=\"without\\ b\\ alt\">\n            \
                          <title refid=\"id3\">\n                Without \n                \
                          <image alt=\"b\" candidates=\"{'*': 'b.png'}\" \
                          uri=\"b.png\">\n                 alt\n            \
                          <substitution_definition names=\"a\">\n                <image \
                          alt=\"ALT\" candidates=\"{'*': 'a.png'}\" \
                          uri=\"a.png\">\n            <substitution_definition \
                          names=\"b\">\n                <image alt=\"b\" candidates=\"{'*': \
                          'b.png'}\" uri=\"b.png\">\n",
                warnings: &[
                    "<SRCDIR>/index.rst:12: WARNING: image file not readable: a.png \
                        [image.not_readable]",
                    "<SRCDIR>/index.rst:14: WARNING: image file not readable: b.png \
                        [image.not_readable]",
                    "<SRCDIR>/index.rst:12: WARNING: image file not readable: a.png \
                        [image.not_readable]",
                    "<SRCDIR>/index.rst:14: WARNING: image file not readable: b.png \
                        [image.not_readable]",
                ],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "with-a-alt", true),
                    ("parse", "section", "without-b-alt", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                    ("Contents", "reference", "id3", true),
                ],
            },
            // co_inline_target_title
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n\nA _`inline` target \
                         [#]_\n-----------------------\n\n.. [#] n\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id3\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id3\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id4\" \
                          refid=\"a-inline-target\">\n                                    A \
                          \n                                    \
                          inline\n                                     target \n        \
                          <section ids=\"a-inline-target\" names=\"a\\ inline\\ \
                          target\">\n            <title refid=\"id4\">\n                A \
                          \n                <target ids=\"inline\" \
                          names=\"inline\">\n                    inline\n                 \
                          target \n                <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id1\" refid=\"id2\">\n                    \
                          1\n            <footnote auto=\"1\" backrefs=\"id1\" \
                          docname=\"index\" ids=\"id2\" names=\"1\">\n                \
                          <label>\n                    1\n                \
                          <paragraph>\n                    n\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "target", "inline", true),
                    ("parse", "footnote_reference", "id1", true),
                    ("parse", "section", "a-inline-target", true),
                    ("parse", "footnote", "id2", true),
                    ("Contents", "reference", "id3", true),
                    ("Contents", "reference", "id4", true),
                ],
            },
            // co_download_title
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n\nGet \
                         :download:`f.txt`\n---------------------\n\nx\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id1\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id2\" \
                          refid=\"get-f-txt\">\n                                    Get \
                          \n                                    <download_reference \
                          refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" \
                          reftarget=\"f.txt\" reftype=\"download\" \
                          refwarn=\"0\">\n                                        <literal \
                          classes=\"xref \
                          download\">\n                                            \
                          f.txt\n        <section ids=\"get-f-txt\" names=\"get\\ \
                          f.txt\">\n            <title>\n                Get \
                          \n                <download_reference refdoc=\"index\" \
                          refdomain=\"\" refexplicit=\"0\" reftarget=\"f.txt\" \
                          reftype=\"download\" refwarn=\"0\">\n                    <literal \
                          classes=\"xref download\">\n                        \
                          f.txt\n            <paragraph>\n                x\n",
                warnings: &[
                    "<SRCDIR>/index.rst:6: WARNING: download file not readable: \
                        <SRCDIR>/f.txt [download.not_readable]",
                    "<SRCDIR>/index.rst:6: WARNING: download file not readable: \
                        <SRCDIR>/f.txt [download.not_readable]",
                ],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "get-f-txt", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                ],
            },
            // co_problematic_title
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n\nBad `ref`_ \
                         here\n---------------\n\nx\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id1\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id2\" \
                          refid=\"bad-ref-here\">\n                                    Bad \
                          \n                                    \
                          ref\n                                     here\n        <section \
                          ids=\"bad-ref-here\" names=\"bad\\ ref\\ here\">\n            \
                          <title>\n                Bad \n                <problematic \
                          ids=\"id4\" refid=\"id3\">\n                    \
                          `ref`_\n                 here\n            \
                          <paragraph>\n                x\n",
                warnings: &[
                    "<SRCDIR>/index.rst:7: ERROR: Unknown target name: \"ref\". [docutils]",
                ],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "bad-ref-here", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                    ("SphinxDanglingReferences", "system_message", "id3", false),
                    ("SphinxDanglingReferences", "problematic", "id4", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));

        let project = merge_project(&[(
            "index",
            "Title\n=====\n\n.. contents::\n\nAbout :ref:`missing-label`\n\
             --------------------------\n\nx\n",
        )]);
        let warning = "<SRCDIR>/index.rst:6: WARNING: undefined label: 'missing-label' [ref.ref]";
        assert_eq!(resolved(&project, "index").warnings, [warning, warning]);

        let project = merge_project(&[(
            "index",
            "Title\n=====\n\n.. contents::\n\nThe :index:`spam` section\n\
             -------------------------\n\nx\n",
        )]);
        let entries: Vec<(&str, &str)> = project.env.index_entries["index"]
            .iter()
            .map(|entry| (entry.value.as_str(), entry.target_id.as_str()))
            .collect();
        assert_eq!(entries, [("spam", "index-0"), ("spam", "index-0")]);
    }

    /// A contents directive in Sphinx's `.. only::` is valid (`only` is a
    /// plain `Element`), and with `:local:` its table lists the sections
    /// of the section it stands in, which Contents finds walking up
    /// through the `only`, as it walks up through a `sidebar`.
    #[test]
    fn contents_inside_only_is_valid() {
        let rows = [
            // contents_inside_only
            SphinxRead {
                rst: "Title\n=====\n\n.. only:: html\n\n   .. contents::\n      \
                         :local:\n\n.. only:: latex\n\n   .. topic:: T\n\n      \
                         body\n\nSec\n---\n\nx\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <only \
                          expr=\"html\">\n            <topic classes=\"contents local\" \
                          ids=\"contents\" names=\"contents\">\n                \
                          <bullet_list>\n                    \
                          <list_item>\n                        \
                          <paragraph>\n                            <reference ids=\"id1\" \
                          refid=\"sec\">\n                                Sec\n        \
                          <only expr=\"latex\">\n            <topic>\n                \
                          <title>\n                    T\n                \
                          <paragraph>\n                    body\n        <section \
                          ids=\"sec\" names=\"sec\">\n            <title \
                          refid=\"id1\">\n                Sec\n            \
                          <paragraph>\n                x\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "sec", true),
                    ("Contents", "reference", "id1", true),
                ],
            },
            // co_sidebar_nested
            SphinxRead {
                rst: "Title\n=====\n\nSec\n---\n\n.. sidebar:: S\n\n   .. contents::\n      \
                         :local:\n\nSub\n~~~\n\nx\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <section ids=\"sec\" \
                          names=\"sec\">\n            <title>\n                \
                          Sec\n            <sidebar>\n                \
                          <title>\n                    S\n                <topic \
                          classes=\"contents local\" ids=\"contents\" \
                          names=\"contents\">\n                    \
                          <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id1\" refid=\"sub\">\n                                    \
                          Sub\n            <section ids=\"sub\" \
                          names=\"sub\">\n                <title \
                          refid=\"id1\">\n                    Sub\n                \
                          <paragraph>\n                    x\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "section", "sec", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "sub", true),
                    ("Contents", "reference", "id1", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// SectNum (710) puts `<generated classes="sectnum">` holding the
    /// section's number and three U+00A0 first in each title of the
    /// document, wherever the directive stands, and stamps the title
    /// `auto="1"`: `:start:`, `:prefix:`, `:suffix:` and `:depth:` shape
    /// it, two directives stack (the later one's first), and Contents,
    /// after it, copies the numbers into its entries and classes each list
    /// whose last section is numbered `auto-toc`. The numbers are part of
    /// the title for whatever reads the tree after the read: the
    /// environment's titles (whence the HTML `<title>`), longtitles and
    /// tocs.
    //
    // oracle: the rows as above; `env.titles`, `env.longtitles` and
    //   `env.tocs` of a dummy build of `sectnum_basic` (structure-directives
    //   research report §3.2, its runs/sectnum_basic/record.json).
    #[test]
    fn sectnum_generated_nodes() {
        let rows = [
            // sectnum_basic
            SphinxRead {
                rst: "Title\n=====\n\n.. sectnum::\n\nSection A\n---------\n\nText \
                         A.\n\nSub A1\n~~~~~~\n\nText A1.\n\nSection B\n---------\n\nText \
                         B.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          1\u{a0}\u{a0}\u{a0}\n            Title\n        <section \
                          ids=\"section-a\" names=\"section\\ a\">\n            <title \
                          auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.1\u{a0}\u{a0}\u{a0}\n                Section A\n            \
                          <paragraph>\n                Text A.\n            <section \
                          ids=\"sub-a1\" names=\"sub\\ a1\">\n                <title \
                          auto=\"1\">\n                    <generated \
                          classes=\"sectnum\">\n                        \
                          1.1.1\u{a0}\u{a0}\u{a0}\n                    Sub \
                          A1\n                <paragraph>\n                    Text \
                          A1.\n        <section ids=\"section-b\" names=\"section\\ \
                          b\">\n            <title auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.2\u{a0}\u{a0}\u{a0}\n                Section B\n            \
                          <paragraph>\n                Text B.\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "section", "section-a", true),
                    ("parse", "section", "sub-a1", true),
                    ("parse", "section", "section-b", true),
                ],
            },
            // sectnum_options
            SphinxRead {
                rst: "Title\n=====\n\n.. sectnum::\n   :depth: 2\n   :prefix: A.\n   \
                         :suffix: )\n   :start: 3\n\nSection A\n---------\n\nText A.\n\nSub \
                         A1\n~~~~~~\n\nText A1.\n\nSection B\n---------\n\nText B.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          A.3)\u{a0}\u{a0}\u{a0}\n            Title\n        <section \
                          ids=\"section-a\" names=\"section\\ a\">\n            <title \
                          auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          A.3.1)\u{a0}\u{a0}\u{a0}\n                Section A\n            \
                          <paragraph>\n                Text A.\n            <section \
                          ids=\"sub-a1\" names=\"sub\\ a1\">\n                \
                          <title>\n                    Sub A1\n                \
                          <paragraph>\n                    Text A1.\n        <section \
                          ids=\"section-b\" names=\"section\\ b\">\n            <title \
                          auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          A.3.2)\u{a0}\u{a0}\u{a0}\n                Section B\n            \
                          <paragraph>\n                Text B.\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "section", "section-a", true),
                    ("parse", "section", "sub-a1", true),
                    ("parse", "section", "section-b", true),
                ],
            },
            // sectnum_two
            SphinxRead {
                rst: "Title\n=====\n\n.. sectnum::\n\n.. sectnum::\n   :prefix: \
                         X\n\nSection A\n---------\n\nText A.\n\nSub A1\n~~~~~~\n\nText \
                         A1.\n\nSection B\n---------\n\nText B.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          X1\u{a0}\u{a0}\u{a0}\n            <generated \
                          classes=\"sectnum\">\n                \
                          1\u{a0}\u{a0}\u{a0}\n            Title\n        <section \
                          ids=\"section-a\" names=\"section\\ a\">\n            <title \
                          auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          X1.1\u{a0}\u{a0}\u{a0}\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.1\u{a0}\u{a0}\u{a0}\n                Section A\n            \
                          <paragraph>\n                Text A.\n            <section \
                          ids=\"sub-a1\" names=\"sub\\ a1\">\n                <title \
                          auto=\"1\">\n                    <generated \
                          classes=\"sectnum\">\n                        \
                          X1.1.1\u{a0}\u{a0}\u{a0}\n                    <generated \
                          classes=\"sectnum\">\n                        \
                          1.1.1\u{a0}\u{a0}\u{a0}\n                    Sub \
                          A1\n                <paragraph>\n                    Text \
                          A1.\n        <section ids=\"section-b\" names=\"section\\ \
                          b\">\n            <title auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          X1.2\u{a0}\u{a0}\u{a0}\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.2\u{a0}\u{a0}\u{a0}\n                Section B\n            \
                          <paragraph>\n                Text B.\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "section", "section-a", true),
                    ("parse", "section", "sub-a1", true),
                    ("parse", "section", "section-b", true),
                ],
            },
            // sectnum_contents_depth
            SphinxRead {
                rst: "Title\n=====\n\n.. sectnum::\n   :depth: 2\n\n.. contents::\n   \
                         :local:\n   :depth: 1\n\nA\n-\n\nA1\n~~\n\nB\n-\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          1\u{a0}\u{a0}\u{a0}\n            Title\n        <topic \
                          classes=\"contents local\" ids=\"contents\" \
                          names=\"contents\">\n            <bullet_list \
                          classes=\"auto-toc\">\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"a\">\n                            <generated \
                          classes=\"sectnum\">\n                                \
                          1.1\u{a0}\u{a0}\u{a0}\n                            \
                          A\n                <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id2\" \
                          refid=\"b\">\n                            <generated \
                          classes=\"sectnum\">\n                                \
                          1.2\u{a0}\u{a0}\u{a0}\n                            B\n        \
                          <section ids=\"a\" names=\"a\">\n            <title auto=\"1\" \
                          refid=\"id1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.1\u{a0}\u{a0}\u{a0}\n                A\n            <section \
                          ids=\"a1\" names=\"a1\">\n                \
                          <title>\n                    A1\n        <section ids=\"b\" \
                          names=\"b\">\n            <title auto=\"1\" \
                          refid=\"id2\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.2\u{a0}\u{a0}\u{a0}\n                B\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "a", true),
                    ("parse", "section", "a1", true),
                    ("parse", "section", "b", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));

        let project = merge_project(&[(
            "index",
            "Title\n=====\n\n.. sectnum::\n\nSection A\n---------\n\nText A.\n\nSub A1\n\
             ~~~~~~\n\nText A1.\n\nSection B\n---------\n\nText B.\n",
        )]);
        let env = &project.env;
        let title = "<title>\n    <generated classes=\"sectnum\">\n        \
                     1\u{a0}\u{a0}\u{a0}\n    Title\n";
        assert_eq!(env.titles["index"].pformat(), title);
        assert_eq!(env.longtitles["index"].pformat(), title);
        let entry = |indent: &str, anchor: &str, number: &str, text: &str| {
            format!(
                "{indent}<compact_paragraph>\n{indent}    <reference anchorname=\"{anchor}\" \
                 internal=\"1\" refuri=\"index\">\n{indent}        <generated \
                 classes=\"sectnum\">\n{indent}            {number}\u{a0}\u{a0}\u{a0}\n\
                 {indent}        {text}\n"
            )
        };
        assert_eq!(
            env.tocs["index"].pformat(),
            [
                "<bullet_list>\n    <list_item>\n".to_string(),
                entry("        ", "", "1", "Title"),
                "        <bullet_list>\n            <list_item>\n".to_string(),
                entry("                ", "#section-a", "1.1", "Section A"),
                "                <bullet_list>\n                    <list_item>\n".to_string(),
                entry("                        ", "#sub-a1", "1.1.1", "Sub A1"),
                "            <list_item>\n".to_string(),
                entry("                ", "#section-b", "1.2", "Section B"),
            ]
            .concat()
        );
    }

    /// The `depth` and `start` details keep an `int` beyond `i64` as its
    /// decimal string (`int_optval`), which SectNum and Contents read as
    /// Python reads the `int`: a huge `:depth:` numbers or lists every
    /// level, a huge negative one only the top level, and a huge
    /// `:start:` counts on from itself.
    #[test]
    fn sectnum_and_contents_read_ints_beyond_i64() {
        let rows = [
            // sn_depth_huge
            SphinxRead {
                rst: "Title\n=====\n\n.. sectnum::\n   :depth: \
                         99999999999999999999\n\nA\n-\n\nA1\n~~\n\nB\n-\n\nC\n-\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          1\u{a0}\u{a0}\u{a0}\n            Title\n        <section \
                          ids=\"a\" names=\"a\">\n            <title \
                          auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.1\u{a0}\u{a0}\u{a0}\n                A\n            <section \
                          ids=\"a1\" names=\"a1\">\n                <title \
                          auto=\"1\">\n                    <generated \
                          classes=\"sectnum\">\n                        \
                          1.1.1\u{a0}\u{a0}\u{a0}\n                    A1\n        <section \
                          ids=\"b\" names=\"b\">\n            <title \
                          auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.2\u{a0}\u{a0}\u{a0}\n                B\n        <section \
                          ids=\"c\" names=\"c\">\n            <title \
                          auto=\"1\">\n                <generated \
                          classes=\"sectnum\">\n                    \
                          1.3\u{a0}\u{a0}\u{a0}\n                C\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "section", "a", true),
                    ("parse", "section", "a1", true),
                    ("parse", "section", "b", true),
                    ("parse", "section", "c", true),
                ],
            },
            // sn_depth_huge_neg
            SphinxRead {
                rst: "Title\n=====\n\n.. sectnum::\n   :depth: \
                         -99999999999999999999\n\nA\n-\n\nA1\n~~\n\nB\n-\n\nC\n-\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          1\u{a0}\u{a0}\u{a0}\n            Title\n        <section \
                          ids=\"a\" names=\"a\">\n            <title>\n                \
                          A\n            <section ids=\"a1\" names=\"a1\">\n                \
                          <title>\n                    A1\n        <section ids=\"b\" \
                          names=\"b\">\n            <title>\n                B\n        \
                          <section ids=\"c\" names=\"c\">\n            \
                          <title>\n                C\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "section", "a", true),
                    ("parse", "section", "a1", true),
                    ("parse", "section", "b", true),
                    ("parse", "section", "c", true),
                ],
            },
            // sn_start_huge
            SphinxRead {
                rst: "A\n=\n\nB\n=\n\nC\n=\n\n.. sectnum::\n   :start: \
                         99999999999999999999\n",
                body: "    <section ids=\"a\" names=\"a\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          99999999999999999999\u{a0}\u{a0}\u{a0}\n            A\n    \
                          <section ids=\"b\" names=\"b\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          100000000000000000000\u{a0}\u{a0}\u{a0}\n            B\n    \
                          <section ids=\"c\" names=\"c\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          100000000000000000001\u{a0}\u{a0}\u{a0}\n            C\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "a", true),
                    ("parse", "section", "b", true),
                    ("parse", "section", "c", true),
                ],
            },
            // co_depth_huge
            SphinxRead {
                rst: "Title\n=====\n\n.. contents::\n   :depth: \
                         99999999999999999999\n\nA\n-\n\nA1\n~~\n\nB\n-\n\nC\n-\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id1\">\n            Title\n        <topic \
                          classes=\"contents\" ids=\"contents\" \
                          names=\"contents\">\n            <title>\n                \
                          Contents\n            <bullet_list>\n                \
                          <list_item>\n                    \
                          <paragraph>\n                        <reference ids=\"id1\" \
                          refid=\"title\">\n                            \
                          Title\n                    <bullet_list>\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id2\" refid=\"a\">\n                                    \
                          A\n                            \
                          <bullet_list>\n                                \
                          <list_item>\n                                    \
                          <paragraph>\n                                        <reference \
                          ids=\"id3\" \
                          refid=\"a1\">\n                                            \
                          A1\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id4\" refid=\"b\">\n                                    \
                          B\n                        \
                          <list_item>\n                            \
                          <paragraph>\n                                <reference \
                          ids=\"id5\" refid=\"c\">\n                                    \
                          C\n        <section ids=\"a\" names=\"a\">\n            <title \
                          refid=\"id2\">\n                A\n            <section \
                          ids=\"a1\" names=\"a1\">\n                <title \
                          refid=\"id3\">\n                    A1\n        <section \
                          ids=\"b\" names=\"b\">\n            <title \
                          refid=\"id4\">\n                B\n        <section ids=\"c\" \
                          names=\"c\">\n            <title refid=\"id5\">\n                \
                          C\n",
                warnings: &[],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", true),
                    ("parse", "section", "a", true),
                    ("parse", "section", "a1", true),
                    ("parse", "section", "b", true),
                    ("parse", "section", "c", true),
                    ("Contents", "reference", "id1", true),
                    ("Contents", "reference", "id2", true),
                    ("Contents", "reference", "id3", true),
                    ("Contents", "reference", "id4", true),
                    ("Contents", "reference", "id5", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// A pending node the parse discarded with the content it stood in
    /// (here a `replace` directive's, which may hold a single paragraph
    /// only) is still noted, so its transform still runs. SectNum numbers
    /// the document; Contents builds a table no tree holds, but the
    /// titles it lists link back to it (with `:backlinks: top`, to its
    /// topic's id); TargetNotes' footnote stays out of the tree while its
    /// references go in, numbered and linked to it by Footnotes (620).
    #[test]
    fn discarded_pending_nodes_still_transform() {
        let rows = [
            // replace_contents
            SphinxRead {
                rst: "Title\n=====\n\n.. |x| replace::\n\n   .. \
                         contents::\n\nSec\n---\n\nBody.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"id1\">\n            Title\n        <system_message \
                          level=\"3\" line=\"4\" source=\"<SRCDIR>/index.rst\" \
                          type=\"ERROR\">\n            <paragraph>\n                Error \
                          in \"replace\" directive: may contain a single paragraph \
                          only.\n        <system_message level=\"2\" line=\"4\" \
                          source=\"<SRCDIR>/index.rst\" type=\"WARNING\">\n            \
                          <paragraph>\n                Substitution definition \"x\" empty \
                          or invalid.\n            <literal_block \
                          xml:space=\"preserve\">\n                .. |x| \
                          replace::\n                \n                   .. \
                          contents::\n        <section ids=\"sec\" \
                          names=\"sec\">\n            <title \
                          refid=\"id2\">\n                Sec\n            \
                          <paragraph>\n                Body.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:4: ERROR: Error in \"replace\" directive: may \
                        contain a single paragraph only. [docutils]",
                    "<SRCDIR>/index.rst:4: WARNING: Substitution definition \"x\" empty or \
                        invalid.",
                    "",
                    ".. |x| replace::",
                    "",
                    "   .. contents:: [docutils]",
                ],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", false),
                    ("parse", "section", "sec", true),
                    ("Contents", "reference", "id1", false),
                    ("Contents", "reference", "id2", false),
                ],
            },
            // replace_contents_top
            SphinxRead {
                rst: "Title\n=====\n\n.. |x| replace::\n\n   .. contents::\n      \
                         :backlinks: top\n\nSec\n---\n\nBody.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          refid=\"contents\">\n            Title\n        <system_message \
                          level=\"3\" line=\"4\" source=\"<SRCDIR>/index.rst\" \
                          type=\"ERROR\">\n            <paragraph>\n                Error \
                          in \"replace\" directive: may contain a single paragraph \
                          only.\n        <system_message level=\"2\" line=\"4\" \
                          source=\"<SRCDIR>/index.rst\" type=\"WARNING\">\n            \
                          <paragraph>\n                Substitution definition \"x\" empty \
                          or invalid.\n            <literal_block \
                          xml:space=\"preserve\">\n                .. |x| \
                          replace::\n                \n                   .. \
                          contents::\n                      :backlinks: top\n        \
                          <section ids=\"sec\" names=\"sec\">\n            <title \
                          refid=\"contents\">\n                Sec\n            \
                          <paragraph>\n                Body.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:4: ERROR: Error in \"replace\" directive: may \
                        contain a single paragraph only. [docutils]",
                    "<SRCDIR>/index.rst:4: WARNING: Substitution definition \"x\" empty or \
                        invalid.",
                    "",
                    ".. |x| replace::",
                    "",
                    "   .. contents::",
                    "      :backlinks: top [docutils]",
                ],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "topic", "contents", false),
                    ("parse", "section", "sec", true),
                    ("Contents", "reference", "id1", false),
                    ("Contents", "reference", "id2", false),
                ],
            },
            // replace_sectnum
            SphinxRead {
                rst: "Title\n=====\n\n.. |x| replace::\n\n   .. \
                         sectnum::\n\nSec\n---\n\nBody.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        <title \
                          auto=\"1\">\n            <generated \
                          classes=\"sectnum\">\n                \
                          1\u{a0}\u{a0}\u{a0}\n            Title\n        <system_message \
                          level=\"3\" line=\"4\" source=\"<SRCDIR>/index.rst\" \
                          type=\"ERROR\">\n            <paragraph>\n                Error \
                          in \"replace\" directive: may contain a single paragraph \
                          only.\n        <system_message level=\"2\" line=\"4\" \
                          source=\"<SRCDIR>/index.rst\" type=\"WARNING\">\n            \
                          <paragraph>\n                Substitution definition \"x\" empty \
                          or invalid.\n            <literal_block \
                          xml:space=\"preserve\">\n                .. |x| \
                          replace::\n                \n                   .. \
                          sectnum::\n        <section ids=\"sec\" \
                          names=\"sec\">\n            <title auto=\"1\">\n                \
                          <generated classes=\"sectnum\">\n                    \
                          1.1\u{a0}\u{a0}\u{a0}\n                Sec\n            \
                          <paragraph>\n                Body.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:4: ERROR: Error in \"replace\" directive: may \
                        contain a single paragraph only. [docutils]",
                    "<SRCDIR>/index.rst:4: WARNING: Substitution definition \"x\" empty or \
                        invalid.",
                    "",
                    ".. |x| replace::",
                    "",
                    "   .. sectnum:: [docutils]",
                ],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "section", "sec", true),
                ],
            },
            // replace_target_notes
            SphinxRead {
                rst: "Title\n=====\n\nSee Python_.\n\n.. _Python: \
                         https://www.python.org/\n\n.. |x| replace::\n\n   .. \
                         target-notes::\n\nSec\n---\n\nBody.\n",
                body: "    <section ids=\"title\" names=\"title\">\n        \
                          <title>\n            Title\n        <paragraph>\n            See \
                          \n            <reference name=\"Python\" \
                          refuri=\"https://www.python.org/\">\n                \
                          Python\n             \n            <footnote_reference auto=\"1\" \
                          docname=\"index\" ids=\"id2\" refid=\"id1\">\n                \
                          1\n            .\n        <target ids=\"python\" names=\"python\" \
                          refuri=\"https://www.python.org/\">\n        <system_message \
                          level=\"3\" line=\"8\" source=\"<SRCDIR>/index.rst\" \
                          type=\"ERROR\">\n            <paragraph>\n                Error \
                          in \"replace\" directive: may contain a single paragraph \
                          only.\n        <system_message level=\"2\" line=\"8\" \
                          source=\"<SRCDIR>/index.rst\" type=\"WARNING\">\n            \
                          <paragraph>\n                Substitution definition \"x\" empty \
                          or invalid.\n            <literal_block \
                          xml:space=\"preserve\">\n                .. |x| \
                          replace::\n                \n                   .. \
                          target-notes::\n        <section ids=\"sec\" \
                          names=\"sec\">\n            <title>\n                \
                          Sec\n            <paragraph>\n                Body.\n",
                warnings: &[
                    "<SRCDIR>/index.rst:8: ERROR: Error in \"replace\" directive: may \
                        contain a single paragraph only. [docutils]",
                    "<SRCDIR>/index.rst:8: WARNING: Substitution definition \"x\" empty or \
                        invalid.",
                    "",
                    ".. |x| replace::",
                    "",
                    "   .. target-notes:: [docutils]",
                ],
                trace: &[
                    ("parse", "section", "title", true),
                    ("parse", "target", "python", true),
                    ("parse", "section", "sec", true),
                    ("TargetNotes", "footnote", "id1", false),
                    ("TargetNotes", "footnote_reference", "id2", true),
                ],
            },
        ];
        let differences = differences(&rows);
        assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    }

    /// With `:local:`, Contents walks up from its topic to a section or
    /// the document, and from a topic the parse discarded the walk runs
    /// out of parents: Sphinx 9.1.0 crashes there (decision 13), with
    /// `AttributeError: 'NoneType' object has no attribute 'parent'` at
    /// `docutils/transforms/parts.py:100`, after the parse's records.
    //
    // oracle: Sphinx 9.1.0, the snippet through the read-state harness:
    //   the two parse records, then the exception
    //   (scratchpad/t18b/probes, edge_cases.json `replace_contents_local`);
    //   `sphinx-build` exits 2 (scratchpad/rereview-t18a-r3/e2e/projC).
    #[test]
    fn contents_local_in_discarded_content_crashes() {
        let read = crate::read::read_standalone(
            "Title\n=====\n\n.. |x| replace::\n\n   .. contents::\n      :local:\n\nSec\n---\n\n\
             Body.\n",
            &crate::transforms::test_support::index_rst(),
            &crate::transforms::test_support::harness_settings(true),
        );
        let records: Vec<String> = read
            .diagnostics
            .iter()
            .map(|record| record.render(true))
            .collect();
        assert_eq!(
            records,
            [
                "<SRCDIR>/index.rst:4: ERROR: Error in \"replace\" directive: may contain a \
                 single paragraph only. [docutils]",
                "<SRCDIR>/index.rst:4: WARNING: Substitution definition \"x\" empty or \
                 invalid.\n\n.. |x| replace::\n\n   .. contents::\n      :local: [docutils]",
            ]
        );
        let crash = read.outcome.state().crash.clone().expect("Sphinx crashes");
        assert_eq!(
            (crash.to_string(), crash.rendered_location),
            (
                "AttributeError: 'NoneType' object has no attribute 'parent'".to_string(),
                Some("<SRCDIR>/index.rst:6".to_string())
            )
        );
    }

    /// SectNum counts with Python's `int`, whatever its size: `str()` of
    /// `:start:`, then one more per section, through zero and past the
    /// `i64` range either way.
    //
    // oracle (Sphinx 9.1.0, docutils 0.22.4, CPython 3.12.2): three
    //   top-level sections numbered from `:start:` (scratchpad/t18b/probes,
    //   edge_cases.json `sn_start_*`).
    #[test]
    fn section_numbers_count_as_python_ints() {
        let rows: [(PyValue, [&str; 3]); 8] = [
            (PyValue::Int(1), ["1", "2", "3"]),
            (PyValue::Int(-1), ["-1", "0", "1"]),
            (
                PyValue::Int(i64::MAX),
                [
                    "9223372036854775807",
                    "9223372036854775808",
                    "9223372036854775809",
                ],
            ),
            (
                PyValue::Int(i64::MIN),
                [
                    "-9223372036854775808",
                    "-9223372036854775807",
                    "-9223372036854775806",
                ],
            ),
            (
                PyValue::Str("-9223372036854775810".to_string()),
                [
                    "-9223372036854775810",
                    "-9223372036854775809",
                    "-9223372036854775808",
                ],
            ),
            (
                PyValue::Str("99999999999999999999".to_string()),
                [
                    "99999999999999999999",
                    "100000000000000000000",
                    "100000000000000000001",
                ],
            ),
            (
                PyValue::Str("-99999999999999999999".to_string()),
                [
                    "-99999999999999999999",
                    "-99999999999999999998",
                    "-99999999999999999997",
                ],
            ),
            (
                PyValue::Str("-100000000000000000000".to_string()),
                [
                    "-100000000000000000000",
                    "-99999999999999999999",
                    "-99999999999999999998",
                ],
            ),
        ];
        for (start, expected) in rows {
            let mut number = SectionNumber::of(&start);
            let mut counted = Vec::new();
            for _ in 0..3 {
                counted.push(number.to_string());
                number.increment();
            }
            assert_eq!(counted, expected, "start {start:?}");
        }
    }
}
