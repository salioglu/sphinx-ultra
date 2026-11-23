//! Sphinx's own read transforms (`SP/sphinx/transforms/`), ported one row
//! of the read-transform table at a time ([`super::READ_TRANSFORMS`]).
//!
//! A transform that walks the tree walks it the way its Python loop does:
//! over `list(document.findall(...))`, a snapshot taken before the loop
//! changes anything ([`TxTree::findall`]), or over the generator itself, a
//! live walk that reads each child list as it is when it gets there
//! ([`TxTree::next_node`] from the node it is at).

use unicode_normalization::UnicodeNormalization;

use super::tree::{Findall, TxId, TxNode, TxTree};
use super::{Transform, TransformCx};
use crate::doctree::{is_text_element, kinds, AttrValue};

/// 010-018 `ApplySourceWorkaround` (`SP/sphinx/transforms/__init__.py:237-245`,
/// `SP/sphinx/util/nodes.py:127-216`), its rawsource half: every
/// `TextElement`, `image` and `topic` of the tree whose `rawsource` is
/// empty takes its `astext()`, a `term`'s rawsource first losing each of
/// its item's classifiers (`\s*:\s*<classifier>`, the last one first). The
/// location half needs no pass: [`super::location::get_source_line`] reads
/// a node's location as the workaround leaves it.
///
/// What docutils' parse records is the rawsource that stays: the
/// Referential nodes', a literal block's, a target's. Where docutils
/// records a node's markup source and the parse here records none (a
/// paragraph, a title, inline markup, a term, an image's directive block),
/// the node takes its text instead; no transform or writer reads those.
/// The one the transforms read is a reference's with none of its own, an
/// image `:target:`'s, which takes the image's `alt`: the text
/// `DanglingReferences` (850) gives its `problematic`.
pub struct ApplySourceWorkaround;

impl Transform for ApplySourceWorkaround {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let tree = &mut *cx.tree;
        let worked = |node: &TxNode| {
            is_text_element(node.kind) || matches!(node.kind, kinds::IMAGE | "topic")
        };
        for node in tree.findall(tree.root(), worked) {
            if tree.node(node).kind == kinds::TERM {
                strip_classifiers(tree, node);
            }
            if tree.node(node).rawsource().is_none() {
                let text = tree.astext(node);
                tree.node_mut(node).set_rawsource(&text);
            }
        }
    }
}

/// `apply_source_workaround`'s term branch (`SP/sphinx/util/nodes.py:165-174`):
/// for each classifier of the term's item, the last one first,
/// `re.sub(r'\s*:\s*%s' % re.escape(classifier.astext()), '', rawsource)`,
/// Python's `\s` being Unicode whitespace and the four separators
/// `\x1c`-`\x1f`. The parse records no rawsource on a term (docutils'
/// is its whole line, classifiers included), so for now a term has none
/// to strip and takes its text, which holds no classifier.
fn strip_classifiers(tree: &mut TxTree, term: TxId) {
    let Some(mut rawsource) = tree.node(term).rawsource().map(str::to_string) else {
        return;
    };
    let Some(item) = tree.parent(term) else {
        return;
    };
    let classifiers = tree.findall(item, |node| node.kind == kinds::CLASSIFIER);
    for classifier in classifiers.into_iter().rev() {
        let pattern = format!(
            r"[\s\x1c-\x1f]*:[\s\x1c-\x1f]*{}",
            regex::escape(&tree.astext(classifier))
        );
        let pattern = regex::Regex::new(&pattern).expect("an escaped classifier compiles");
        rawsource = pattern.replace_all(&rawsource, "").into_owned();
    }
    tree.node_mut(term).set_rawsource(&rawsource);
}

/// 210-021 `MoveModuleTargets` (`SP/sphinx/transforms/__init__.py:153-175`):
/// a `py:module` target (`ismod`) that stands third in a section, after
/// the title and the module's index node, gives the section its ids ahead
/// of the section's own and leaves the tree. docutils' `document.ids` is
/// not updated: the ids still name the target ([DS §5.6]). The loop works
/// through a snapshot of the targets (`list(self.document.findall(...))`).
pub struct MoveModuleTargets;

impl Transform for MoveModuleTargets {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let tree = &mut *cx.tree;
        let targets = tree.findall(tree.root(), |node| node.kind == kinds::TARGET);
        for target in targets {
            let node = tree.node(target);
            if node.attrs.ids.is_empty() || node.get("ismod").is_none() {
                continue;
            }
            let Some(section) = tree.parent(target) else {
                continue;
            };
            let at = tree
                .node(section)
                .children
                .iter()
                .position(|&child| child == target);
            if tree.node(section).kind != kinds::SECTION || at != Some(2) {
                continue;
            }
            let ids = tree.node(target).attrs.ids.clone();
            tree.node_mut(section).attrs.ids.splice(0..0, ids);
            tree.remove(section, target);
        }
    }
}

/// 220-032 `ReorderConsecutiveTargetAndIndexNodes`
/// (`SP/sphinx/transforms/__init__.py:446-515`): from each target, the run
/// of consecutive sibling targets and `index` nodes it starts is sorted
/// stably, index nodes first, so that the targets stand together before
/// the node `PropagateTargets` (260) labels with them. The loop is a live
/// walk ([`Findall`]): after a sort it goes on from the run's place, where
/// a target it visits again starts a run of targets, which stays as it is.
pub struct ReorderConsecutiveTargetAndIndexNodes;

impl Transform for ReorderConsecutiveTargetAndIndexNodes {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let tree = &mut *cx.tree;
        let mut walk = Findall::new(tree.root());
        while let Some(target) = walk.next(tree, |node| node.kind == kinds::TARGET) {
            reorder_index_target_nodes(tree, target);
        }
    }
}

/// `_reorder_index_target_nodes` (`SP/sphinx/transforms/__init__.py:480-506`)
/// from the target at `start`: `start` and its following siblings while
/// they are targets or index nodes, sorted by `_sort_key` (index 0,
/// target 1) when there are two or more.
fn reorder_index_target_nodes(tree: &mut TxTree, start: TxId) {
    let Some(parent) = tree.parent(start) else {
        return;
    };
    let children = &tree.node(parent).children;
    let Some(first) = children.iter().position(|&child| child == start) else {
        return;
    };
    let mut run: Vec<TxId> = children[first..]
        .iter()
        .copied()
        .take_while(|&node| matches!(tree.node(node).kind, kinds::TARGET | "index"))
        .collect();
    if run.len() < 2 {
        return;
    }
    let len = run.len();
    run.sort_by_key(|&node| u8::from(tree.node(node).kind != "index"));
    tree.node_mut(parent)
        .children
        .splice(first..first + len, run);
}

/// 261-023 `SortIds` (`SP/sphinx/transforms/__init__.py:217-234`): a
/// section with more than one id whose first starts with `id` — any id,
/// not only an automatic one ([DS §5.6]) — takes it last. The loop is a
/// live walk that moves no node.
pub struct SortIds;

impl Transform for SortIds {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let tree = &mut *cx.tree;
        let mut walk = Findall::new(tree.root());
        while let Some(section) = walk.next(tree, |node| node.kind == kinds::SECTION) {
            let ids = &mut tree.node_mut(section).attrs.ids;
            if ids.len() > 1 && ids[0].starts_with("id") {
                ids.rotate_left(1);
            }
        }
    }
}

/// 025 `TranslationProgressTotaliser` (`SP/sphinx/transforms/i18n.py:631-651`):
/// stamps the document's `translation_progress`, the number of elements
/// carrying a `translated` attribute and how many of them are translated.
/// Only `Locale` (020) sets that attribute, and only from message catalogs,
/// which a read here never has (020 is inert), so every document is
/// stamped `{'total': 0, 'translated': 0}`, the dict's Python repr as
/// pformat prints it.
pub struct TranslationProgressTotaliser;

impl Transform for TranslationProgressTotaliser {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        cx.tree.node_mut(root).set(
            "translation_progress",
            AttrValue::Str("{'total': 0, 'translated': 0}".to_string()),
        );
    }
}

/// 210-022 `HandleCodeBlocks` (`SP/sphinx/transforms/__init__.py:178-197`):
/// replaces every block quote whose children are all doctest blocks by
/// those blocks (`node.replace_self(node.children)`), so that a doctest
/// indented under a paragraph stands on its own rather than in a quote.
/// The first block takes the quote's ids, classes and names
/// ([`TxTree::replace_self`]). An empty quote qualifies too (`all()` of
/// nothing is true) and is removed: the parse keeps a quote whose content
/// makes no node, as docutils does. One with a basic attribute stays,
/// since removing it crashes Sphinx (decision 13).
///
/// The loop is `for node in self.document.findall(nodes.block_quote)`, a
/// live walk, which goes on from where the quote was, as the generator
/// does over the parent's child list by index ([`next_after`]): after the
/// last block that replaced it (doctest blocks hold only text), and after
/// an empty quote it skips the node that took the quote's place, subtree
/// and all, as a Python list iterator skips the element after a removed
/// one.
pub struct HandleCodeBlocks;

impl Transform for HandleCodeBlocks {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let tree = &mut *cx.tree;
        let is_quote = |node: &TxNode| node.kind == kinds::BLOCK_QUOTE;
        let mut next = tree.next_node(tree.root(), is_quote, false, true, false, false);
        while let Some(quote) = next {
            let blocks = tree.node(quote).children.clone();
            let doctest_only = blocks
                .iter()
                .all(|&block| tree.node(block).kind == kinds::DOCTEST_BLOCK);
            // Replacing an empty quote that has a basic attribute (the
            // class of an epigraph whose content makes no node) crashes
            // Sphinx 9.1.0: `replace_self([])` asserts that no basic
            // attribute is lost (`AssertionError: Losing "classes"
            // attribute`). Inputs that crash Sphinx are not modelled
            // (decision 13: the build exits 2, T17a), so the quote stays
            // and the walk goes on as past any quote it keeps.
            let attrs = &tree.node(quote).attrs;
            let crashes_sphinx = blocks.is_empty()
                && !(attrs.ids.is_empty()
                    && attrs.classes.is_empty()
                    && attrs.names.is_empty()
                    && attrs.dupnames.is_empty());
            if !doctest_only || crashes_sphinx {
                next = tree.next_node(quote, is_quote, false, true, true, true);
                continue;
            }
            let parent = tree
                .parent(quote)
                .expect("a block quote under the root has a parent");
            let index = tree
                .node(parent)
                .children
                .iter()
                .position(|&child| child == quote)
                .expect("a node the walk reaches is among its parent's children");
            tree.replace_self(quote, blocks.clone());
            let walked = match blocks.last() {
                Some(&last) => last,
                None => match tree.node(parent).children.get(index) {
                    Some(&skipped) => skipped,
                    None => parent,
                },
            };
            next = next_after(tree, walked, is_quote);
        }
    }
}

/// The first node `pred` accepts after the subtree at `id` in pre-order,
/// read from the tree as it is now: where a live `findall` walk goes on
/// once it is done with that subtree, `id`'s following siblings and then
/// its ancestors' ([`TxTree::next_node`] from the subtree's last node).
fn next_after(tree: &TxTree, id: TxId, pred: impl Fn(&TxNode) -> bool) -> Option<TxId> {
    let mut last = id;
    while let Some(&child) = tree.node(last).children.last() {
        last = child;
    }
    tree.next_node(last, pred, false, true, true, true)
}

/// 500-024 `DoctestTransform` (`SP/sphinx/transforms/__init__.py:327-334`):
/// appends the class `doctest` to every doctest block in the tree, even to
/// one that has it already (`node['classes'].append('doctest')`). The loop
/// is a live walk that moves no node.
pub struct DoctestTransform;

impl Transform for DoctestTransform {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let tree = &mut *cx.tree;
        let is_doctest = |node: &TxNode| node.kind == kinds::DOCTEST_BLOCK;
        let mut next = tree.next_node(tree.root(), is_doctest, false, true, false, false);
        while let Some(block) = next {
            tree.node_mut(block)
                .attrs
                .classes
                .push("doctest".to_string());
            next = tree.next_node(block, is_doctest, false, true, true, true);
        }
    }
}

/// 500-031 `GlossarySorter` (`SP/sphinx/transforms/__init__.py:427-443`):
/// sorts the definition list of every glossary whose `sorted` is true (the
/// directive's `:sorted:`) by `unicodedata.normalize('NFD',
/// item[0].astext().lower())`: each item's first term, its `astext()`
/// lowercased (Python's `str.lower`, as Rust's `to_lowercase`, final sigma
/// included) and then NFD-normalized. `sorted` is stable and compares code
/// points, as Rust's string order does. Sphinx sorts in the read rather
/// than in the directive, after the i18n transforms, so Substitutions
/// (220) and AnonymousHyperlinks (440) walk the glossary in source order.
///
/// The loop is a live walk: a glossary is sorted before the walk descends
/// into it, so a glossary in one of its definitions is reached in sorted
/// order.
pub struct GlossarySorter;

impl Transform for GlossarySorter {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let tree = &mut *cx.tree;
        let is_glossary = |node: &TxNode| node.kind == "glossary";
        let mut next = tree.next_node(tree.root(), is_glossary, false, true, false, false);
        while let Some(glossary) = next {
            let node = tree.node(glossary);
            let sorted = node.get("sorted").is_some_and(AttrValue::is_truthy);
            // `glossary[0]`, the definition list the directive made.
            let list = node.children.first().copied();
            if let (true, Some(list)) = (sorted, list) {
                let mut items = tree.node(list).children.clone();
                items.sort_by_cached_key(|&item| glossary_sort_key(tree, item));
                tree.node_mut(list).children = items;
            }
            next = tree.next_node(glossary, is_glossary, false, true, true, true);
        }
    }
}

/// `GlossarySorter`'s key for a definition list item: the NFD form of its
/// first child's, the first term's, `astext()` lowercased. The `index`
/// node `make_glossary_term` appends to the term adds nothing to it.
fn glossary_sort_key(tree: &TxTree, item: TxId) -> String {
    let text = tree
        .node(item)
        .children
        .first()
        .map(|&term| tree.astext(term))
        .unwrap_or_default();
    text.to_lowercase().nfd().collect()
}

/// 999-027 `FilterSystemMessages` (`SP/sphinx/transforms/__init__.py:337-347`,
/// [TW A11]): removes every `system_message` of the document whose level is
/// below `filterlevel = 2 if keep_warnings else 5` from its parent, working
/// through a snapshot (`list(self.document.findall(...))`), so under the
/// default `keep_warnings = False` none stays. The removed message's text
/// went to the warning stream when it was raised; Sphinx only logs it again
/// at debug level here.
pub struct FilterSystemMessages;

impl Transform for FilterSystemMessages {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let filter_level = if cx.settings.keep_warnings { 2 } else { 5 };
        let root = cx.tree.root();
        let messages = cx
            .tree
            .findall(root, |node| node.kind == kinds::SYSTEM_MESSAGE);
        for message in messages {
            let kept = matches!(
                cx.tree.node(message).get("level"),
                Some(AttrValue::Int(level)) if *level >= filter_level
            );
            if kept {
                continue;
            }
            // `node.parent.remove(node)`: docutils leaves the removed
            // node's `parent` link as it was.
            if let Some(parent) = cx.tree.parent(message) {
                cx.tree
                    .node_mut(parent)
                    .children
                    .retain(|&child| child != message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::messages::{self, ERROR, INFO, SEVERE, WARNING};
    use crate::doctree::{Doctree, Node, Span};
    use crate::read::ReadSettings;
    use crate::rst::ParseOptions;
    use crate::transforms::state::DocumentState;
    use crate::transforms::tree::TxTree;
    use crate::transforms::{read_transforms_with, NoOp, TransformId, TransformKey};

    /// How Sphinx parses `<SRCDIR>/index.rst`.
    fn index_rst() -> ParseOptions {
        ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            ..ParseOptions::default()
        }
    }

    /// The doctree stage R leaves for `rst`, each row run by `transform`.
    fn stage_r_with(
        rst: &str,
        transform: &dyn Fn(TransformId) -> &'static dyn Transform,
    ) -> Doctree {
        let parsed = crate::rst::parse_rst_full(rst, &index_rst());
        let settings = ReadSettings::sphinx_defaults();
        read_transforms_with(parsed, &settings, "index", transform)
            .doctree()
            .clone()
    }

    /// The pformat of the doctree stage R leaves for `rst`.
    fn stage_r(rst: &str) -> String {
        stage_r_with(rst, &TransformId::transform).root.pformat()
    }

    /// `doctree` with the transform of row `id` applied to it alone, at
    /// the row's key.
    fn apply_row(doctree: Doctree, id: TransformId) -> Doctree {
        let (key, _) = *crate::transforms::READ_TRANSFORMS
            .iter()
            .find(|(_, row)| *row == id)
            .expect("a row of the table");
        let mut state = DocumentState::default();
        let mut tree = TxTree::from_parse(doctree, &mut state);
        id.transform().apply(&mut TransformCx {
            tree: &mut tree,
            state: &mut state,
            settings: &ReadSettings::sphinx_defaults(),
            key,
            docname: "index",
        });
        tree.into_doctree()
    }

    /// `HandleCodeBlocks` (210-022, `SP/sphinx/transforms/__init__.py:178-197`)
    /// replaces every block quote whose children are all doctest blocks by
    /// those blocks, and leaves one with anything else in it.
    /// `docutils-0147` is the oracle's own post-read doctree; the other
    /// rows are Sphinx 9.1.0 reads of each input as `index.rst` (the
    /// read-state harness, `keep_warnings = True`). The first block takes
    /// the quote's classes (`replace_self`'s `update_basic_atts`), before
    /// `DoctestTransform` (500) appends its own. A doctest-only quote
    /// nested in a quote that also holds a doctest block is unwrapped
    /// after the outer quote was judged, so the outer one stays.
    ///
    /// The walk is docutils' live `findall`, a generator over the tree's
    /// child lists by index, and the hand-built trees pin where it goes on
    /// after a replacement. Replaced by its blocks, a quote's place is
    /// walked on from after them. An empty quote (`all()` of nothing is
    /// true) is removed, and the walk, a Python list iterator, skips the
    /// node that took its place, subtree and all: a doctest-only quote
    /// right after an empty one stays. An empty last child ends its
    /// parent's walk. (docutils 0.22.4: each tree built with
    /// `docutils.nodes` and `HandleCodeBlocks(document).apply()`.) The
    /// two `.. default-domain::` rows reach the same walk through the read:
    /// the parse keeps the quote the directive leaves empty, and Sphinx
    /// 9.1.0 skips a section after it, subtree included, or a paragraph.
    /// An empty quote with a class, which crashes Sphinx, stays.
    #[test]
    fn handle_code_blocks_unwraps_doctest_only_block_quotes() {
        const DOC: &str = "<document source=\"<SRCDIR>/index.rst\" \
                           translation_progress=\"{'total': 0, 'translated': 0}\">\n";
        let rows: [(&str, &str); 7] = [
            (
                // docutils-0147.
                "Para.\n\n    >>> quoted_code()\n    output\n",
                "    <paragraph>\n        Para.\n    \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n        \
                 >>> quoted_code()\n        output\n",
            ),
            (
                "Para.\n\n    >>> a\n\n    >>> b\n",
                "    <paragraph>\n        Para.\n    \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n        >>> a\n    \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n        >>> b\n",
            ),
            (
                "Para.\n\n    >>> a\n\n    text\n",
                "    <paragraph>\n        Para.\n    <block_quote>\n        \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n            >>> a\n        \
                 <paragraph>\n            text\n",
            ),
            (
                "Para.\n\n    >>> outer\n\n        >>> inner\n",
                "    <paragraph>\n        Para.\n    <block_quote>\n        \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n            >>> outer\n        \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n            >>> inner\n",
            ),
            (
                ".. epigraph::\n\n   >>> 1\n",
                "    <doctest_block classes=\"epigraph doctest\" xml:space=\"preserve\">\n        \
                 >>> 1\n",
            ),
            (
                // The parse keeps the quote `.. default-domain::` leaves
                // empty; 210 removes it and skips the section that took
                // its place, so the quote in the section stays.
                "Para.\n\n   .. default-domain:: py\n\nTitle\n=====\n\nText.\n\n   >>> 1\n",
                "    <paragraph>\n        Para.\n    <section ids=\"title\" names=\"title\">\n        \
                 <title>\n            Title\n        <paragraph>\n            Text.\n        \
                 <block_quote>\n            \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n                >>> 1\n",
            ),
            (
                // Only the paragraph after the empty quote is skipped.
                "Para.\n\n   .. default-domain:: py\n\nText.\n\n   >>> 1\n",
                "    <paragraph>\n        Para.\n    <paragraph>\n        Text.\n    \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n        >>> 1\n",
            ),
        ];
        for (rst, body) in rows {
            assert_eq!(stage_r(rst), format!("{DOC}{body}"), "{rst:?}");
        }
        // An empty quote with a basic attribute, the class of an epigraph
        // whose content makes no node, crashes Sphinx 9.1.0: `replace_self`
        // with no node asserts that no basic attribute is lost
        // (`AssertionError: Losing "classes" attribute: ['epigraph']`).
        // Inputs that crash Sphinx are not modelled (decision 13: the build
        // exits 2, T17a), so 210 leaves the quote where it is.
        assert_eq!(
            stage_r("Para.\n\n.. epigraph::\n\n   .. default-domain:: py\n"),
            format!(
                "{DOC}    <paragraph>\n        Para.\n    <block_quote classes=\"epigraph\">\n"
            )
        );

        fn element(kind: &'static str, children: Vec<Node>) -> Node {
            let mut node = Node::elem(kind, Span::ZERO);
            node.children = children;
            node
        }
        fn text(kind: &'static str, text: &str) -> Node {
            element(kind, vec![Node::text_node(text, Span::ZERO)])
        }
        fn doctest(code: &str) -> Node {
            let mut node = text(kinds::DOCTEST_BLOCK, code);
            node.set("xml:space", AttrValue::Str("preserve".to_string()));
            node
        }
        let quote = |children| element(kinds::BLOCK_QUOTE, children);
        let handled = |children: Vec<Node>| {
            let mut root = element(kinds::DOCUMENT, children);
            root.set("source", AttrValue::Str("<test>".to_string()));
            let doctree = Doctree {
                root,
                sources: vec!["<test>".to_string()],
            };
            apply_row(doctree, TransformId::HandleCodeBlocks)
                .root
                .pformat()
        };
        const TEST: &str = "<document source=\"<test>\">\n";
        let block = |code: &str, depth: usize| {
            let indent = "    ".repeat(depth);
            format!("{indent}<doctest_block xml:space=\"preserve\">\n{indent}    {code}\n")
        };
        assert_eq!(
            handled(vec![
                text(kinds::PARAGRAPH, "Para."),
                quote(Vec::new()),
                quote(vec![doctest(">>> 1")]),
            ]),
            format!(
                "{TEST}    <paragraph>\n        Para.\n    <block_quote>\n{}",
                block(">>> 1", 2)
            )
        );
        assert_eq!(
            handled(vec![
                text(kinds::PARAGRAPH, "Para."),
                quote(Vec::new()),
                text(kinds::PARAGRAPH, "Two."),
                quote(vec![doctest(">>> 1")]),
            ]),
            format!(
                "{TEST}    <paragraph>\n        Para.\n    <paragraph>\n        Two.\n{}",
                block(">>> 1", 1)
            )
        );
        assert_eq!(
            handled(vec![
                quote(Vec::new()),
                quote(vec![
                    quote(vec![doctest(">>> 1")]),
                    text(kinds::PARAGRAPH, "x"),
                ]),
            ]),
            format!(
                "{TEST}    <block_quote>\n        <block_quote>\n{}        <paragraph>\n            x\n",
                block(">>> 1", 3)
            )
        );
        assert_eq!(
            handled(vec![
                element(
                    kinds::SECTION,
                    vec![text(kinds::TITLE, "T"), quote(Vec::new())]
                ),
                quote(vec![doctest(">>> 1")]),
            ]),
            format!(
                "{TEST}    <section>\n        <title>\n            T\n{}",
                block(">>> 1", 1)
            )
        );
        assert_eq!(
            handled(vec![
                quote(vec![doctest(">>> 1"), doctest(">>> 2")]),
                quote(vec![doctest(">>> 3")]),
            ]),
            format!(
                "{TEST}{}{}{}",
                block(">>> 1", 1),
                block(">>> 2", 1),
                block(">>> 3", 1)
            )
        );
    }

    /// `DoctestTransform` (500-024, `SP/sphinx/transforms/__init__.py:327-334`)
    /// appends the class `doctest` to every doctest block in the tree, one
    /// in a list item included, and appends it again to a block that has it
    /// already (`.. rst-class:: doctest`). `docutils-0099` and
    /// `docutils-0102` are the oracle's own post-read doctrees; the last row
    /// is a Sphinx 9.1.0 read of the input as `index.rst`.
    #[test]
    fn doctest_blocks_get_the_doctest_class() {
        const DOC: &str = "<document source=\"<SRCDIR>/index.rst\" \
                           translation_progress=\"{'total': 0, 'translated': 0}\">\n";
        let rows: [(&str, &str); 3] = [
            (
                // docutils-0099.
                ">>> print(\"hello\")\nhello\n>>> 1 + 1\n2\n",
                "    <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n        \
                 >>> print(\"hello\")\n        hello\n        >>> 1 + 1\n        2\n",
            ),
            (
                // docutils-0102.
                "- item\n\n  >>> code()\n  result\n",
                "    <bullet_list bullet=\"-\">\n        <list_item>\n            \
                 <paragraph>\n                item\n            \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n                \
                 >>> code()\n                result\n",
            ),
            (
                ".. rst-class:: doctest\n\n>>> x\n",
                "    <doctest_block classes=\"doctest doctest\" xml:space=\"preserve\">\n        \
                 >>> x\n",
            ),
        ];
        for (rst, body) in rows {
            assert_eq!(stage_r(rst), format!("{DOC}{body}"), "{rst:?}");
        }
    }

    /// `GlossarySorter` (500-031, `SP/sphinx/transforms/__init__.py:427-443`)
    /// sorts a `:sorted:` glossary in the read, not in the parse: the parse
    /// leaves the terms in source order, stage R sorts them, and stage R
    /// with its 500-031 row run as a no-op does not. The key is the first
    /// term's `astext()`, lowercased (Python's `str.lower`, final sigma
    /// included: `ΣΑΣ` keys as `σας`, before `σαςa`), then NFD-normalized
    /// (`éclair` sorts as `e` + U+0301). A glossary without `:sorted:`
    /// keeps its order. Oracle: `sx_directives.glossary_sorted_reorders_terms`
    /// and `glossary_unsorted_keeps_source_order` of the Sphinx snippet
    /// fixture, and a Sphinx 9.1.0 read of the Greek glossary as
    /// `index.rst`.
    #[test]
    fn glossary_sorter_runs_at_500() {
        fn terms(node: &Node, out: &mut Vec<String>) {
            if node.kind == kinds::TERM {
                out.push(node.astext_docutils());
            }
            for child in &node.children {
                terms(child, out);
            }
        }
        let terms_of = |doctree: &Doctree| {
            let mut out = Vec::new();
            terms(&doctree.root, &mut out);
            out
        };
        let without_500 = |id: TransformId| -> &'static dyn Transform {
            match id {
                TransformId::GlossarySorter => &NoOp,
                _ => id.transform(),
            }
        };
        let rows: [(&str, [&str; 4], [&str; 4]); 2] = [
            (
                ".. glossary::\n   :sorted:\n\n   zeta\n      Z def.\n\n   Alpha\n      A def.\n\n   \
                 \u{e9}clair\n      E def.\n\n   beta\n      B def.\n",
                ["zeta", "Alpha", "\u{e9}clair", "beta"],
                ["Alpha", "beta", "\u{e9}clair", "zeta"],
            ),
            (
                ".. glossary::\n\n   zeta\n      Z def.\n\n   Alpha\n      A def.\n\n   \
                 \u{e9}clair\n      E def.\n\n   beta\n      B def.\n",
                ["zeta", "Alpha", "\u{e9}clair", "beta"],
                ["zeta", "Alpha", "\u{e9}clair", "beta"],
            ),
        ];
        for (rst, source_order, read_order) in rows {
            let parsed = crate::rst::parse_rst_full(rst, &index_rst());
            assert_eq!(
                terms_of(&parsed.doctree),
                source_order,
                "the parse: {rst:?}"
            );
            assert_eq!(
                terms_of(&stage_r_with(rst, &TransformId::transform)),
                read_order,
                "stage R: {rst:?}"
            );
            assert_eq!(
                terms_of(&stage_r_with(rst, &without_500)),
                source_order,
                "stage R without 500-031: {rst:?}"
            );
        }
        let greek = ".. glossary::\n   :sorted:\n\n   \u{3c3}\u{3b1}\u{3c2}a\n      t.\n\n   \
                     \u{3a3}\u{391}\u{3a3}\n      s.\n";
        assert_eq!(
            terms_of(&stage_r_with(greek, &TransformId::transform)),
            ["\u{3a3}\u{391}\u{3a3}", "\u{3c3}\u{3b1}\u{3c2}a"]
        );
    }

    /// The post-read pformat of `rst` (stage R and stage M, which removes
    /// the reporter's messages), less the document line every read of
    /// `<SRCDIR>/index.rst` opens with.
    fn read_body(rst: &str) -> String {
        const DOC: &str = "<document source=\"<SRCDIR>/index.rst\" \
                           translation_progress=\"{'total': 0, 'translated': 0}\">\n";
        let read =
            crate::read::read_standalone(rst, &index_rst(), &ReadSettings::sphinx_defaults());
        let pformat = read.outcome.doctree().root.pformat();
        pformat
            .strip_prefix(DOC)
            .unwrap_or_else(|| panic!("{rst:?}: unexpected document line:\n{pformat}"))
            .to_string()
    }

    /// `SortIds` (261-023, `SP/sphinx/transforms/__init__.py:217-234`) moves
    /// a section's first id to the end when the section has several and
    /// the first starts with `id` — any id, not only an automatic one
    /// ([DS §5.6]): `Identity` behind the `mylabel` the label propagated
    /// (`state.sortids_startswith`), and the automatic `id1` of a section
    /// whose title's id a label took first behind that label
    /// (`inventory_projects/std_objects_and_docs/b`). A first id that does
    /// not start with `id`, or a lone one, stays.
    //
    // oracle (Sphinx 9.1.0, docutils 0.22.4): each input read as
    //   `index.rst` through tools/gen_read_state_fixture.py's harness.
    #[test]
    fn sort_ids_moves_any_id_prefixed_first_id() {
        let rows: [(&str, &str); 4] = [
            (
                // state.sortids_startswith.
                ".. _mylabel:\n\nIdentity\n========\n\nText.\n",
                "    <target refid=\"mylabel\">\n    \
                 <section ids=\"mylabel identity\" names=\"identity mylabel\">\n        \
                 <title>\n            Identity\n        <paragraph>\n            Text.\n",
            ),
            (
                // inventory_projects/std_objects_and_docs/b.rst.
                "B\n=\n\nSee :term:`a term` and :doc:`a`.\n\n.. _example:\n\nexample\n-------\n\n\
                 Section whose title equals its id (dispname == fullname -> '-').\n",
                "    <section ids=\"b\" names=\"b\">\n        <title>\n            B\n        \
                 <paragraph>\n            See \n            <pending_xref refdoc=\"index\" \
                 refdomain=\"std\" refexplicit=\"0\" reftarget=\"a term\" reftype=\"term\" \
                 refwarn=\"1\">\n                <inline classes=\"xref std std-term\">\n                    \
                 a term\n             and \n            <pending_xref refdoc=\"index\" \
                 refdomain=\"std\" refexplicit=\"0\" reftarget=\"a\" reftype=\"doc\" \
                 refwarn=\"1\">\n                <inline classes=\"xref std std-doc\">\n                    \
                 a\n            .\n        <target refid=\"example\">\n        \
                 <section dupnames=\"example\" ids=\"example id1\" names=\"example\">\n            \
                 <title>\n                example\n            <paragraph>\n                \
                 Section whose title equals its id (dispname == fullname -> '-').\n",
            ),
            (
                ".. _target1:\n.. _target2:\n\nSection Title\n=============\n",
                "    <target refid=\"target1\">\n    <target refid=\"target2\">\n    \
                 <section ids=\"section-title target2 target1\" \
                 names=\"section\\ title target2 target1\">\n        <title>\n            \
                 Section Title\n",
            ),
            (
                "Identity\n========\n\nText.\n",
                "    <section ids=\"identity\" names=\"identity\">\n        <title>\n            \
                 Identity\n        <paragraph>\n            Text.\n",
            ),
        ];
        for (rst, body) in rows {
            assert_eq!(read_body(rst), body, "{rst:?}");
        }
    }

    /// `MoveModuleTargets` (210-021, `SP/sphinx/transforms/__init__.py:153-175`)
    /// gives a `py:module` target's ids to the section it opens — the
    /// target that stands third in the section, after the title and the
    /// module's index node — ahead of the section's own, and takes the
    /// target out (`py_toc/mod`, [TW A4]). A module target anywhere else
    /// stays, and `PropagateTargets` (260) moves its id to the next node.
    /// That includes one an empty block quote stands before at 210-021
    /// (the quote a `.. default-domain::` indented under the title leaves,
    /// which `HandleCodeBlocks`, 210-022, removes only afterwards).
    //
    // oracle (Sphinx 9.1.0): as `sort_ids_moves_any_id_prefixed_first_id`.
    #[test]
    fn move_module_targets_into_section() {
        // py_toc/mod.rst, down to its first description.
        let module = "Mod\n===\n\n.. py:module:: pkg.mymod\n\n.. py:class:: MyClass\n\n   \
                      Class body.\n\n   .. py:method:: my_method(arg)\n\n      Method body.\n\n\
                      .. py:function:: my_func(x)\n\n   Function body.\n";
        let body = read_body(module);
        assert_eq!(
            body.split("        <desc ").next(),
            Some(
                "    <section ids=\"module-pkg.mymod mod\" names=\"mod\">\n        <title>\n            \
                 Mod\n        <index entries=\"('pair',\\ 'module;\\ pkg.mymod',\\ \
                 'module-pkg.mymod',\\ '',\\ None)\">\n        <index entries=\"('single',\\ \
                 'MyClass\\ (class\\ in\\ pkg.mymod)',\\ 'pkg.mymod.MyClass',\\ '',\\ None)\">\n"
            )
        );
        assert!(!body.contains("<target"), "{body}");

        let index = "        <index entries=\"('pair',\\ 'module;\\ foo',\\ 'module-foo',\\ '',\\ \
                     None)\">\n";
        let rows: [(&str, String); 4] = [
            (
                "Title\n=====\n\n.. py:module:: foo\n\nText.\n",
                format!(
                    "    <section ids=\"module-foo title\" names=\"title\">\n        <title>\n            \
                     Title\n{index}        <paragraph>\n            Text.\n"
                ),
            ),
            (
                "Title\n=====\n\n   .. default-domain:: py\n\n.. py:module:: foo\n\nText.\n",
                format!(
                    "    <section ids=\"title\" names=\"title\">\n        <title>\n            \
                     Title\n{index}        <target ismod=\"1\" refid=\"module-foo\">\n        \
                     <paragraph ids=\"module-foo\">\n            Text.\n"
                ),
            ),
            (
                "Title\n=====\n\nIntro.\n\n.. py:module:: foo\n\nText.\n",
                format!(
                    "    <section ids=\"title\" names=\"title\">\n        <title>\n            \
                     Title\n        <paragraph>\n            Intro.\n{index}        \
                     <target ismod=\"1\" refid=\"module-foo\">\n        \
                     <paragraph ids=\"module-foo\">\n            Text.\n"
                ),
            ),
            (
                ".. py:module:: foo\n\nText.\n",
                format!(
                    "{}    <target ismod=\"1\" refid=\"module-foo\">\n    \
                     <paragraph ids=\"module-foo\">\n        Text.\n",
                    &index[4..]
                ),
            ),
        ];
        for (rst, body) in rows {
            assert_eq!(read_body(rst), body, "{rst:?}");
        }
    }

    /// The rawsource half of `ApplySourceWorkaround` (010-018,
    /// `SP/sphinx/util/nodes.py:127-216`): every `TextElement`, `image`
    /// and `topic` of the tree whose `rawsource` is empty takes its
    /// `astext()`, a `term`'s first dropping each classifier it is
    /// followed by. A rawsource the parse recorded stays (the reference's
    /// own, the literal block's); an image `:target:`'s reference, which
    /// has none, takes the image's `alt`. Sphinx 9.1.0 gives the values
    /// below for these nodes; on the others the parse records no rawsource
    /// where docutils records the markup source, and the fill gives their
    /// text instead (nothing reads a paragraph's rawsource).
    //
    // oracle (Sphinx 9.1.0): the input read as `index.rst` through
    //   tools/gen_read_state_fixture.py's harness, printing `rawsource` of
    //   every TextElement, image and topic of the post-read doctree.
    #[test]
    fn apply_source_workaround_fills_empty_rawsources() {
        let doctree = stage_r_with(
            "Title\n=====\n\nSee `x <http://u>`_ here.\n\nterm : cls\n   def\n\n\
             .. image:: x.png\n   :target: http://t\n   :alt: Alt\n\n::\n\n   code\n",
            &TransformId::transform,
        );
        fn collect<'a>(node: &'a Node, kind: &str, out: &mut Vec<&'a Node>) {
            if node.kind == kind {
                out.push(node);
            }
            for child in &node.children {
                collect(child, kind, out);
            }
        }
        let rawsources = |kind: &str| {
            let mut nodes = Vec::new();
            collect(&doctree.root, kind, &mut nodes);
            nodes
                .iter()
                .map(|node| node.rawsource().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(rawsources(kinds::TITLE), ["Title"]);
        assert_eq!(rawsources(kinds::REFERENCE), ["`x <http://u>`_", "Alt"]);
        assert_eq!(rawsources(kinds::TERM), ["term"]);
        assert_eq!(rawsources(kinds::CLASSIFIER), ["cls"]);
        assert_eq!(rawsources(kinds::LITERAL_BLOCK), ["code"]);
        // Without the 010 row the image's reference keeps no rawsource.
        let without_010 = |id: TransformId| -> &'static dyn Transform {
            match id {
                TransformId::ApplySourceWorkaround => &NoOp,
                _ => id.transform(),
            }
        };
        let doctree = stage_r_with(
            ".. image:: x.png\n   :target: http://t\n   :alt: Alt\n",
            &without_010,
        );
        assert_eq!(doctree.root.children[0].rawsource(), None);
    }

    /// `TranslationProgressTotaliser` (025) stamps the document's
    /// translation progress, `{'total': 0, 'translated': 0}` in a build
    /// with no message catalogs: the stamp T6's read made itself, which the
    /// table's 025 row now makes. Stage R with the read's own table stamps
    /// the document; the same stage with the 025 row run as a no-op does
    /// not.
    #[test]
    fn translation_progress_is_stamped_by_025() {
        let parse = ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            ..ParseOptions::default()
        };
        let stage_r = |transform: &dyn Fn(TransformId) -> &'static dyn Transform| {
            let parsed = crate::rst::parse_rst_full("Text.\n", &parse);
            let settings = ReadSettings::sphinx_defaults();
            let read = read_transforms_with(parsed, &settings, "index", transform);
            read.doctree().root.pformat()
        };
        assert_eq!(
            stage_r(&TransformId::transform),
            "<document source=\"<SRCDIR>/index.rst\" \
             translation_progress=\"{'total': 0, 'translated': 0}\">\n    \
             <paragraph>\n        Text.\n"
        );
        let without_025 = |id: TransformId| -> &'static dyn Transform {
            match id {
                TransformId::TranslationProgressTotaliser => &NoOp,
                _ => id.transform(),
            }
        };
        assert_eq!(
            stage_r(&without_025),
            "<document source=\"<SRCDIR>/index.rst\">\n    <paragraph>\n        Text.\n"
        );
    }

    /// `FilterSystemMessages` (999) removes every `system_message` below
    /// `filterlevel = 2 if keep_warnings else 5`
    /// (`SP/sphinx/transforms/__init__.py:337-347`, [TW A11]): under
    /// `keep_warnings = False` every level goes, SEVERE included; under
    /// `True` only INFO does. One message of each level, at the top of the
    /// document and inside a section; the rest of the tree stays.
    #[test]
    fn filter_system_messages_honours_keep_warnings() {
        let filtered = |keep_warnings: bool| -> Node {
            let message = |level: u8| {
                messages::system_message(
                    level,
                    &format!("level {level}"),
                    "<SRCDIR>/index.rst",
                    Some(1),
                    Vec::new(),
                )
            };
            let mut section = Node::elem(kinds::SECTION, Span::ZERO);
            section.children = vec![message(WARNING), message(INFO)];
            let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
            root.children = vec![message(SEVERE), message(INFO), section, message(ERROR)];
            let doctree = Doctree {
                root,
                sources: vec!["<SRCDIR>/index.rst".to_string()],
            };
            let mut state = DocumentState::default();
            let mut tree = TxTree::from_parse(doctree, &mut state);
            let settings = ReadSettings {
                keep_warnings,
                ..ReadSettings::sphinx_defaults()
            };
            FilterSystemMessages.apply(&mut TransformCx {
                tree: &mut tree,
                state: &mut state,
                settings: &settings,
                key: TransformKey {
                    priority: 999,
                    serial: 27,
                },
                docname: "index",
            });
            tree.into_doctree().root
        };
        fn levels(node: &Node, found: &mut Vec<i64>) {
            if node.kind == kinds::SYSTEM_MESSAGE {
                if let Some(AttrValue::Int(level)) = node.get("level") {
                    found.push(*level);
                }
            }
            for child in &node.children {
                levels(child, found);
            }
        }

        assert_eq!(filtered(false).pformat(), "<document>\n    <section>\n");
        let kept = filtered(true);
        let mut found = Vec::new();
        levels(&kept, &mut found);
        assert_eq!(found, [4, 2, 3]);
        assert_eq!(
            kept.children
                .iter()
                .map(|child| child.kind)
                .collect::<Vec<_>>(),
            [kinds::SYSTEM_MESSAGE, kinds::SECTION, kinds::SYSTEM_MESSAGE]
        );
    }
}
