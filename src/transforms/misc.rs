//! docutils' miscellaneous transforms (`SP/docutils/transforms/misc.py`).

use super::tree::{TxId, TxNode, TxTree};
use super::{Transform, TransformCx};
use crate::doctree::ids::make_id;
use crate::doctree::kinds;
use crate::doctree::messages::WARNING;

/// `Transitions`' four messages (`SP/docutils/transforms/misc.py:99-107`,
/// `:135`).
const NOT_STRUCTURAL: &str = "Transition must be child of <document> or <section>.";
const BEGINS: &str = "Document or section may not begin with a transition.";
const ADJACENT: &str = "At least one body element must separate transitions; \
                        adjacent transitions are not allowed.";
const ENDS_DOCUMENT: &str = "Document may not end with a transition.";

/// 830-014 `Transitions` (`SP/docutils/transforms/misc.py:64-143`): checks
/// where each transition stands and moves one that ends a section up the
/// tree.
///
/// A transition must be a child of the document or of a section, may not
/// begin one (be its first child, or follow a title, subtitle, meta or
/// decoration) and may not follow another transition. A misplaced one is
/// reported at its own location ([`TransformCx::report`]); the message
/// joins the tree right after it only when the parent still validates with
/// a paragraph in the transition's place ([`validates_with_paragraph_at`]),
/// and the transition's index then counts the message. A transition left
/// last in a section moves behind the first ancestor that is not a last
/// child; one left last in the document stays, and the message
/// `Document may not end with a transition.` is appended to its parent.
///
/// The loop is `for node in self.document.findall(nodes.transition)`, a
/// live walk ([`TxTree::next_node`] from each transition, which this
/// transform moves but never replaces). The generator walks each child
/// list by index, so the next node it yields after a move is the moved
/// transition itself, at its new place right behind the ancestor it was
/// moved behind, and the walk here visits it there again too. Under the
/// document or a section that visit changes nothing: the transition
/// follows that ancestor, a section or a node holding one, so it neither
/// begins its parent nor follows a transition, and it is not a last child.
/// Under any other parent, an `only` or a `desc_content` holding sections,
/// it reports that the transition is not the child of a document or
/// section.
pub struct Transitions;

impl Transform for Transitions {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let is_transition = |node: &TxNode| node.kind == kinds::TRANSITION;
        let root = cx.tree.root();
        let mut next = cx
            .tree
            .next_node(root, is_transition, false, true, false, false);
        while let Some(transition) = next {
            next = if visit_transition(cx, transition) {
                // The generator's second visit, which never moves the
                // transition again: it is not a last child now.
                Some(transition)
            } else {
                cx.tree
                    .next_node(transition, is_transition, false, true, true, true)
            };
        }
    }
}

/// `Transitions.visit_transition` (`SP/docutils/transforms/misc.py:94-143`)
/// for the transition at `node`: whether it moved the transition.
fn visit_transition(cx: &mut TransformCx<'_>, node: TxId) -> bool {
    let parent = cx
        .tree
        .parent(node)
        .expect("a transition under the root has a parent");
    let mut index = child_index(cx.tree, parent, node);
    let previous = index
        .checked_sub(1)
        .map(|at| cx.tree.node(cx.tree.node(parent).children[at]).kind);
    let structural = matches!(cx.tree.node(parent).kind, kinds::DOCUMENT | kinds::SECTION);
    let message = if !structural {
        Some(NOT_STRUCTURAL)
    } else if previous
        .is_none_or(|kind| matches!(kind, kinds::TITLE | kinds::SUBTITLE | "meta" | "decoration"))
    {
        Some(BEGINS)
    } else if previous == Some(kinds::TRANSITION) {
        Some(ADJACENT)
    } else {
        None
    };
    if let Some(text) = message {
        let warning = cx.report(WARNING, text, node);
        if validates_with_paragraph_at(cx.tree, parent, index) {
            cx.tree.insert(parent, index + 1, warning);
            index += 1;
        }
    }
    if !structural {
        return false;
    }
    if index + 1 != cx.tree.node(parent).children.len() {
        // No need to move the node.
        return false;
    }
    // Climb from the transition while the node behind which it is to be
    // moved, docutils' `sibling`, is the last child of its parent,
    // `above` (`index` may count the message inserted after the
    // transition, as in docutils).
    let mut above = parent;
    while index + 1 == cx.tree.node(above).children.len() {
        let sibling = above;
        let Some(grand) = cx.tree.parent(sibling) else {
            // The transition ends the document: it stays, and the warning
            // goes behind it.
            let warning = cx.report(WARNING, ENDS_DOCUMENT, node);
            cx.tree.append(parent, warning);
            return false;
        };
        index = child_index(cx.tree, grand, sibling);
        above = grand;
    }
    cx.tree.remove(parent, node);
    cx.tree.insert(above, index + 1, node);
    true
}

/// `parent.index(child)`.
fn child_index(tree: &TxTree, parent: TxId, child: TxId) -> usize {
    tree.node(parent)
        .children
        .iter()
        .position(|&node| node == child)
        .expect("a node the walk reaches is among its parent's children")
}

/// `node.parent.validate(recursive=False)` as `visit_transition` runs it,
/// with a paragraph in place of the transition at `index`
/// (`SP/docutils/transforms/misc.py:110-119`; `Element.validate`,
/// `SP/docutils/nodes.py:1330-1356`): the element's attributes
/// ([`attributes_valid`]) and its children against its content model
/// ([`content_valid`]). Under Sphinx the document never validates:
/// `TranslationProgressTotaliser` (025) gave it `translation_progress`,
/// which docutils' `document` does not allow.
fn validates_with_paragraph_at(tree: &TxTree, parent: TxId, index: usize) -> bool {
    let element = tree.node(parent);
    let children: Vec<&str> = element
        .children
        .iter()
        .enumerate()
        .map(|(at, &child)| {
            if at == index {
                kinds::PARAGRAPH
            } else {
                tree.node(child).kind
            }
        })
        .collect();
    attributes_valid(element) && content_valid(element.kind, &children)
}

/// `Element.validate_attributes` (`SP/docutils/nodes.py:1239-1265`) for a
/// parent of a transition: every attribute it has is one its class allows,
/// the four list attributes and `source` for any element, `title` too for
/// the document, and ids and classes are valid identifiers
/// (`validate_identifier`, `:3137-3149`: `value == make_id(value)`). Names
/// and dupnames take any value, and the validators' normalizations change
/// nothing a parse made. `backrefs` belongs to the `BackLinkable` classes
/// alone, none of which holds a transition.
fn attributes_valid(element: &TxNode) -> bool {
    let attrs = &element.attrs;
    let identifiers = attrs
        .ids
        .iter()
        .chain(&attrs.classes)
        .all(|value| *value == make_id(value));
    let known = attrs
        .extra
        .iter()
        .all(|(key, _)| *key == "source" || (element.kind == kinds::DOCUMENT && *key == "title"));
    identifiers && attrs.backrefs.is_empty() && known
}

/// How many children of a content model's category may stand at its place
/// (docutils' quantifiers).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quantifier {
    /// `'.'`: exactly one.
    One,
    /// `'?'`: one at most.
    Optional,
    /// `'*'`: any number.
    Any,
}

/// The element classes a content model names (`SP/docutils/nodes.py`).
#[derive(Clone, Copy)]
enum Category {
    Title,
    Subtitle,
    Meta,
    Decoration,
    Docinfo,
    Transition,
    Section,
    Topic,
    Sidebar,
    Body,
}

impl Category {
    /// `isinstance(child, category)` for a child of `kind`.
    fn holds(self, kind: &str) -> bool {
        match self {
            Category::Title => kind == kinds::TITLE,
            Category::Subtitle => kind == kinds::SUBTITLE,
            Category::Meta => kind == "meta",
            Category::Decoration => kind == "decoration",
            Category::Docinfo => kind == "docinfo",
            Category::Transition => kind == kinds::TRANSITION,
            Category::Section => kind == kinds::SECTION,
            Category::Topic => kind == "topic",
            Category::Sidebar => kind == "sidebar",
            Category::Body => is_body(kind),
        }
    }
}

type ContentModel = &'static [(&'static [Category], Quantifier)];

/// `document.content_model` (`SP/docutils/nodes.py:1697-1705`).
const DOCUMENT_MODEL: ContentModel = &[
    (&[Category::Title], Quantifier::Optional),
    (&[Category::Subtitle], Quantifier::Optional),
    (&[Category::Meta], Quantifier::Any),
    (&[Category::Decoration], Quantifier::Optional),
    (&[Category::Docinfo], Quantifier::Optional),
    (&[Category::Transition], Quantifier::Optional),
    (
        &[
            Category::Body,
            Category::Topic,
            Category::Sidebar,
            Category::Transition,
        ],
        Quantifier::Any,
    ),
    (&[Category::Section, Category::Transition], Quantifier::Any),
];

/// `section.content_model` (`SP/docutils/nodes.py:1677-1681`).
const SECTION_MODEL: ContentModel = &[
    (&[Category::Title], Quantifier::One),
    (&[Category::Subtitle], Quantifier::Optional),
    (
        &[
            Category::Body,
            Category::Topic,
            Category::Sidebar,
            Category::Transition,
        ],
        Quantifier::Any,
    ),
    (&[Category::Section, Category::Transition], Quantifier::Any),
];

/// `Element.validate_content` (`SP/docutils/nodes.py:1267-1310`) and the
/// leftover check of `Element.validate` (`:1343-1352`) for an element of
/// `kind` whose children are of `children` kinds: the children match the
/// model's parts in order, each match passing its own position check
/// ([`position_valid`]), and none is left over. The parse puts a
/// transition under the document, a section, Sphinx's `only` or a
/// `desc_content` (the content [`crate::rst`] parses with section titles);
/// the last two keep `Element`'s empty model, so any child is left over.
fn content_valid(kind: &str, children: &[&str]) -> bool {
    let model = match kind {
        kinds::DOCUMENT => DOCUMENT_MODEL,
        kinds::SECTION => SECTION_MODEL,
        _ => &[],
    };
    let fits = |categories: &[Category], at: usize| {
        children
            .get(at)
            .is_some_and(|kind| categories.iter().any(|category| category.holds(kind)))
    };
    // The child the walk is at; `children.len()` once there is none.
    let mut at = 0;
    for &(categories, quantifier) in model {
        if !fits(categories, at) {
            if quantifier == Quantifier::One {
                return false;
            }
            continue;
        }
        if !position_valid(children, at) {
            return false;
        }
        at += 1;
        if quantifier == Quantifier::Any {
            while fits(categories, at) {
                if !position_valid(children, at) {
                    return false;
                }
                at += 1;
            }
        }
    }
    at == children.len()
}

/// `validate_position` of the child at `at`: a transition may not begin
/// its parent (come first, or after a title, subtitle, meta or
/// decoration), end it, or follow another transition
/// (`SP/docutils/nodes.py:1614-1636`); a subtitle may not come first
/// (`:1570-1575`). Any other child has no constraint.
fn position_valid(children: &[&str], at: usize) -> bool {
    match children[at] {
        kinds::TRANSITION => {
            let predecessor = at.checked_sub(1).map(|before| children[before]);
            let begins = predecessor.is_none_or(|kind| {
                matches!(kind, kinds::TITLE | kinds::SUBTITLE | "meta" | "decoration")
            });
            let ends = at + 1 == children.len();
            !(begins || ends || predecessor == Some(kinds::TRANSITION))
        }
        kinds::SUBTITLE => at != 0,
        _ => true,
    }
}

/// Whether `kind` names a docutils body element, `isinstance(node,
/// nodes.Body)`: the element classes of the `General`, `Sequential`,
/// `Admonition` and `Special` categories, `Invisible` ones included
/// (`SP/docutils/nodes.py:1398-1449` and the classes from `:2165`), and
/// Sphinx's node classes derived from them (`SP/sphinx/addnodes.py`). A
/// `glossary`, `only`, `highlightlang`, `hlist` or `centered` is not one.
fn is_body(kind: &str) -> bool {
    matches!(
        kind,
        // General.
        "paragraph"
            | "rubric"
            | "compound"
            | "container"
            | "block_quote"
            | "reference"
            | "literal_block"
            | "doctest_block"
            | "math_block"
            | "line_block"
            | "footnote"
            | "citation"
            | "image"
            | "figure"
            | "table"
            // Sequential.
            | "bullet_list"
            | "enumerated_list"
            | "definition_list"
            | "field_list"
            | "option_list"
            // Admonition.
            | "attention"
            | "caution"
            | "danger"
            | "error"
            | "hint"
            | "important"
            | "note"
            | "tip"
            | "warning"
            | "admonition"
            // Special, and Invisible.
            | "system_message"
            | "raw"
            | "comment"
            | "substitution_definition"
            | "target"
            | "pending"
            // Sphinx: General, Admonition, Invisible, and subclasses of
            // `paragraph` and `reference`.
            | "toctree"
            | "desc_content"
            | "desc"
            | "versionmodified"
            | "seealso"
            | "productionlist"
            | "index"
            | "compact_paragraph"
            | "number_reference"
            | "download_reference"
    )
}

#[cfg(test)]
mod tests {
    use crate::config::BuildConfig;
    use crate::read::{read_standalone, ReadSettings};
    use crate::rst::ParseOptions;
    use crate::transforms::reporter::LogRecord;

    /// `Transitions` (830-014, `SP/docutils/transforms/misc.py:64-143`):
    /// the four transition cases of [TW §6.3] (`docutils-0030` to `-0033`),
    /// then the shapes behind them. A misplaced transition is reported
    /// against its own location; the message joins the tree right after it
    /// only when its parent validates with a paragraph in its place
    /// (`validate(recursive=False)`). The document never does under Sphinx:
    /// 025 stamped `translation_progress`, which docutils' `document` does
    /// not allow. A section does when its content model holds: an `index`
    /// is a body element (`Invisible`), a `glossary` is not, and another
    /// misplaced transition fails its own position check. A transition
    /// that ends a section moves up behind the first ancestor that is not
    /// a last child; one that ends the document stays, followed by the
    /// warning. When the inserted message is the section's last child, the
    /// transition still moves (`index += 1` points at the message) and the
    /// message stays behind. One under a non-structural parent (`only`) is
    /// reported and left there, and so is one that ends a section under an
    /// `only` or in a `desc_content` and moves into it: the generator
    /// visits a moved transition again at its new place.
    ///
    /// Each row is a Sphinx 9.1.0 read of the input as `index.rst` (the
    /// read-state harness, `keep_warnings = True`): the post-read doctree
    /// and the warning stream; the four corpus cases' warnings are also
    /// their aux-gate references.
    #[test]
    fn transitions_match_docutils() {
        const DOC: &str = "<document source=\"<SRCDIR>/index.rst\" \
                           translation_progress=\"{'total': 0, 'translated': 0}\">\n";
        const BEGIN: &str = "Document or section may not begin with a transition.";
        const END: &str = "Document may not end with a transition.";
        const ADJACENT: &str = "At least one body element must separate transitions; \
                                adjacent transitions are not allowed.";
        const CHILD: &str = "Transition must be child of <document> or <section>.";
        let message = |indent: usize, line: u32, text: &str| {
            let pad = "    ".repeat(indent);
            format!(
                "{pad}<system_message level=\"2\" line=\"{line}\" source=\"<SRCDIR>/index.rst\" \
                 type=\"WARNING\">\n{pad}    <paragraph>\n{pad}        {text}\n"
            )
        };
        let record = |line: u32, text: &str| {
            format!("<SRCDIR>/index.rst:{line}: WARNING: {text} [docutils]")
        };
        let rows: Vec<(&str, String, Vec<String>)> = vec![
            (
                // docutils-0030.
                "----\n\npara\n",
                "    <transition>\n    <paragraph>\n        para\n".to_string(),
                vec![record(1, BEGIN)],
            ),
            (
                // docutils-0031.
                "para\n\n----\n",
                format!(
                    "    <paragraph>\n        para\n    <transition>\n{}",
                    message(1, 3, END)
                ),
                vec![record(3, END)],
            ),
            (
                // docutils-0032.
                "para\n\n----\n\n----\n\nend\n",
                "    <paragraph>\n        para\n    <transition>\n    <transition>\n    \
                 <paragraph>\n        end\n"
                    .to_string(),
                vec![record(5, ADJACENT)],
            ),
            (
                // docutils-0033.
                "Head\n====\n\n----\n\npara\n",
                format!(
                    "    <section ids=\"head\" names=\"head\">\n        <title>\n            \
                     Head\n        <transition>\n{}        <paragraph>\n            para\n",
                    message(2, 4, BEGIN)
                ),
                vec![record(4, BEGIN)],
            ),
            (
                "A\n=\n\n----\n\nB\n=\n\npara\n",
                format!(
                    "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n{}    \
                     <transition>\n    <section ids=\"b\" names=\"b\">\n        <title>\n            \
                     B\n        <paragraph>\n            para\n",
                    message(2, 4, BEGIN)
                ),
                vec![record(4, BEGIN)],
            ),
            (
                "A\n=\n\npara\n\n----\n",
                format!(
                    "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                     <paragraph>\n            para\n        <transition>\n{}",
                    message(2, 6, END)
                ),
                vec![record(6, END)],
            ),
            (
                "A\n=\n\n----\n\npara\n\n.. glossary::\n\n   t\n      d\n",
                "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                 <transition>\n        <paragraph>\n            para\n        \
                 <glossary sorted=\"0\">\n            <definition_list classes=\"glossary\">\n                \
                 <definition_list_item>\n                    <term ids=\"term-t\">\n                        \
                 t\n                        \
                 <index entries=\"('single',\\ 't',\\ 'term-t',\\ 'main',\\ None)\">\n                    \
                 <definition>\n                        <paragraph>\n                            d\n"
                    .to_string(),
                vec![record(4, BEGIN)],
            ),
            (
                "A\n=\n\n----\n\npara\n\n.. index:: x\n",
                format!(
                    "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                     <transition>\n{}        <paragraph>\n            para\n        \
                     <index entries=\"('single',\\ 'x',\\ 'index-0',\\ '',\\ None)\" inline=\"0\">\n        \
                     <target ids=\"index-0\">\n",
                    message(2, 4, BEGIN)
                ),
                vec![record(4, BEGIN)],
            ),
            (
                "A\n=\n\n----\n\npara\n\n----\n\n----\n\nmore\n",
                "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                 <transition>\n        <paragraph>\n            para\n        <transition>\n        \
                 <transition>\n        <paragraph>\n            more\n"
                    .to_string(),
                vec![record(4, BEGIN), record(10, ADJACENT)],
            ),
            (
                "A\n=\n\nB\n-\n\npara\n\n----\n\nC\n=\n\nz\n",
                "    <section ids=\"a\" names=\"a\">\n        <title>\n            A\n        \
                 <section ids=\"b\" names=\"b\">\n            <title>\n                B\n            \
                 <paragraph>\n                para\n    <transition>\n    \
                 <section ids=\"c\" names=\"c\">\n        <title>\n            C\n        \
                 <paragraph>\n            z\n"
                    .to_string(),
                Vec::new(),
            ),
            (
                ".. only:: html\n\n   para\n\n   ----\n\n   more\n",
                "    <only expr=\"html\">\n        <paragraph>\n            para\n        \
                 <transition>\n        <paragraph>\n            more\n"
                    .to_string(),
                vec![record(5, CHILD)],
            ),
            (
                // A transition that ends a section under `only` moves into
                // the `only`, and the generator, visiting it again there,
                // reports it.
                ".. only:: html\n\n   A\n   =\n\n   para\n\n   ----\n\n   B\n   =\n\n   text\n",
                "    <only expr=\"html\">\n        <section ids=\"a\" names=\"a\">\n            \
                 <title>\n                A\n            <paragraph>\n                para\n        \
                 <transition>\n        <section ids=\"b\" names=\"b\">\n            <title>\n                \
                 B\n            <paragraph>\n                text\n"
                    .to_string(),
                vec![record(8, CHILD)],
            ),
            (
                // The same in a `desc_content`.
                ".. py:function:: f()\n\n   A\n   =\n\n   para\n\n   ----\n\n   B\n   =\n\n   text\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n    \
                 <desc classes=\"py function\" desctype=\"function\" domain=\"py\" \
                 no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" \
                 nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n        \
                 <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" \
                 classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n            \
                 <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n                \
                 f\n            <desc_parameterlist xml:space=\"preserve\">\n        \
                 <desc_content>\n            <section ids=\"a\" names=\"a\">\n                \
                 <title>\n                    A\n                <paragraph>\n                    \
                 para\n            <transition>\n            <section ids=\"b\" names=\"b\">\n                \
                 <title>\n                    B\n                <paragraph>\n                    \
                 text\n"
                    .to_string(),
                vec![record(8, CHILD)],
            ),
        ];
        let settings = ReadSettings::from_config(&BuildConfig {
            validate_directives: false,
            keep_warnings: true,
            ..BuildConfig::default()
        });
        let parse = ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            ..ParseOptions::default()
        };
        let mut mismatches = Vec::new();
        for (rst, body, warnings) in rows {
            let read = read_standalone(rst, &parse, &settings);
            let tree = read.outcome.doctree().root.pformat();
            let stream: Vec<String> = read
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.render(true))
                .collect();
            if tree != format!("{DOC}{body}") || stream != warnings {
                mismatches.push(format!(
                    "{rst:?}\n--- sphinx 9.1.0 ---\n{DOC}{body}{warnings:#?}\n--- ours ---\n\
                     {tree}{stream:#?}"
                ));
            }
            // Each message that prints is the transform's record: keyed by
            // 830-014, after the parse's records. (The INFOs
            // `SphinxDanglingReferences`, 850-039, raises about targets
            // nothing references never print.)
            for record in &read.outcome.state().log {
                if let LogRecord::Reporter { key, level, .. } = record {
                    if *level >= crate::doctree::messages::WARNING {
                        assert_eq!((key.priority, key.serial), (830, 14), "{rst:?}: {record:?}");
                    }
                }
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }
}
