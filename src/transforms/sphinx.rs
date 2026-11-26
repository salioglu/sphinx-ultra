//! Sphinx's own read transforms (`SP/sphinx/transforms/`), ported one row
//! of the read-transform table at a time ([`super::READ_TRANSFORMS`]).
//!
//! A transform that walks the tree walks it the way its Python loop does:
//! over `list(document.findall(...))`, a snapshot taken before the loop
//! changes anything ([`TxTree::findall`]), or over the generator itself, a
//! live walk that reads each child list as it is when it gets there
//! ([`TxTree::next_node`] from the node it is at).

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use unicode_normalization::UnicodeNormalization;

use super::state::NodeView;
use super::tree::{Findall, TxId, TxNode, TxTree};
use super::{Transform, TransformCx};
use crate::diagnostics::Category;
use crate::doctree::{is_text_element, kinds, AttrValue};
use crate::read::BuildDate;
use crate::rst::{LogLocation, SphinxCrash};
use crate::utils::py_repr_str;

/// 010-018 `ApplySourceWorkaround` (`SP/sphinx/transforms/__init__.py:237-245`,
/// `SP/sphinx/util/nodes.py:127-216`), its rawsource half: every
/// `TextElement`, `image` and `topic` of the tree whose `rawsource` is
/// empty takes its `astext()`, a `term`'s rawsource first losing each of
/// its item's classifiers (`\s*:\s*<classifier>`, the last one first).
///
/// What docutils' parse records is the rawsource that stays: the
/// Referential nodes', a literal block's, a target's. Where docutils
/// records a node's markup source and the parse here records none (a
/// paragraph, a title, inline markup, a term, an image's directive block),
/// the node takes its text instead; no transform or writer reads those.
/// The one the transforms read is a reference's with none of its own, an
/// image `:target:`'s, which takes the image's `alt`: the text
/// `DanglingReferences` (850) gives its `problematic`.
///
/// The location half is no pass: [`super::location`] applies it whenever
/// it locates a node, to the location docutils' parse leaves the node
/// (`location::apply_source_workaround`): a classifier takes its item's
/// source; an image, title or topic without a source its parent's source
/// and line (the titles of topics, sidebars and sections are the ones
/// docutils leaves without); a literal block without a source (a
/// `parsed-literal` in a container without `document`) its nearest
/// located ancestor's source; and a rubric, line, image or field name
/// still without one its nearest located ancestor's source and line 0.
/// The parse records the two inputs only it knows
/// ([`super::state::DocumentState::loc_overrides`]): which rubrics and
/// parsed-literal blocks docutils leaves unstamped, and where 010 puts a
/// classifier docutils makes with an empty rawsource, the line before its
/// item's. It also records where docutils stamps each line of a
/// `line-block` directive, which is not 010's doing.
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

/// 010-034 `PreserveTranslatableMessages` (`SP/sphinx/transforms/i18n.py:103-111`):
/// every `translatable` node of the tree keeps its original messages
/// (`preserve_original_messages`) before `Locale` (020) could translate
/// them, on every read, message catalogs or not. Sphinx's one
/// `translatable` class is `toctree` (`SP/sphinx/addnodes.py:58-70`): its
/// entries' titles, those that are not empty, go into `rawentries`, a list
/// it starts when the node has none (`setdefault`), and its caption, when
/// it has one, into `rawcaption`. Nothing later in the read changes them.
///
/// The parse stores a toctree without a caption the way pformat prints
/// docutils' `None`, as `caption="True"`, so the caption is read as the
/// resolve pass reads it ([`crate::env::toctree::caption_of`]): one that
/// reads `True` is taken for none and gets no `rawcaption`.
pub struct PreserveTranslatableMessages;

impl Transform for PreserveTranslatableMessages {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let tree = &mut *cx.tree;
        for toctree in tree.findall(tree.root(), |node| node.kind == kinds::TOCTREE) {
            let node = tree.node_mut(toctree);
            let mut rawentries = match node.get("rawentries") {
                Some(AttrValue::List(rawentries)) => rawentries.clone(),
                _ => Vec::new(),
            };
            rawentries.extend(
                crate::env::toctree::entries_of(node.get("entries"))
                    .into_iter()
                    .filter_map(|(title, _)| title.filter(|title| !title.is_empty())),
            );
            node.set("rawentries", AttrValue::List(rawentries));
            if let Some(caption) = crate::env::toctree::caption_of(node.get("caption")).cloned() {
                node.set("rawcaption", AttrValue::Str(caption));
            }
        }
    }
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
/// makes no node, as docutils does. Removing one with a basic attribute
/// (the class of an epigraph or a `rst-class` whose content makes no node)
/// crashes Sphinx 9.1.0: `replace_self([])` asserts that no basic
/// attribute is lost (`SP/docutils/nodes.py:1110-1132`), so the read
/// fails there (decision 13).
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
            if !doctest_only {
                next = tree.next_node(quote, is_quote, false, true, true, true);
                continue;
            }
            if blocks.is_empty() {
                if let Some(message) = losing_basic_attribute(tree.node(quote)) {
                    cx.crash(
                        SphinxCrash::python("AssertionError", message, LogLocation::None),
                        quote,
                    );
                    return;
                }
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

/// The assertion `replace_self` fails replacing `node` by nothing
/// (`SP/docutils/nodes.py:1124-1130`): `Losing "<att>" attribute:
/// <value>` for the first of docutils' basic attributes — `ids`,
/// `classes`, `names`, `dupnames` — the node holds a value of, the list
/// as Python prints it; `None` when it holds none.
fn losing_basic_attribute(node: &TxNode) -> Option<String> {
    let attrs = &node.attrs;
    [
        ("ids", &attrs.ids),
        ("classes", &attrs.classes),
        ("names", &attrs.names),
        ("dupnames", &attrs.dupnames),
    ]
    .into_iter()
    .find(|(_, values)| !values.is_empty())
    .map(|(name, values)| {
        let values: Vec<String> = values.iter().map(|value| py_repr_str(value)).collect();
        format!("Losing \"{name}\" attribute: [{}]", values.join(", "))
    })
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

/// 210-025 `AutoNumbering` (`SP/sphinx/transforms/__init__.py:200-214`):
/// every node the std domain numbers — a figure, a table or a container
/// (`enumerable_nodes`, matched by exact class,
/// `SP/sphinx/domains/std/__init__.py:1363-1366`), so a code block's
/// caption wrapper — that has a caption or a title among its children
/// (`get_numfig_title` is not `None`) and no ids takes an implicit target:
/// `document.note_implicit_target(node)`, an id from the shared counter
/// ([`TransformCx::set_id`]) and its names bound to it, of which a node
/// without ids has none. As a transform it numbers once the whole parse
/// is over, so its ids follow every id the parse allocated ([DS §5.6]),
/// and before PropagateTargets (260) hands a label above the node its
/// ids. The loop is a live walk over every element, which allocates ids
/// and moves no node.
pub struct AutoNumbering;

impl Transform for AutoNumbering {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let root = cx.tree.root();
        let enumerable = cx.tree.findall(root, |node| {
            matches!(node.kind, "figure" | kinds::TABLE | "container") && node.attrs.ids.is_empty()
        });
        for node in enumerable {
            let titled = cx
                .tree
                .node(node)
                .children
                .iter()
                .any(|&child| matches!(cx.tree.node(child).kind, "caption" | kinds::TITLE));
            if !titled {
                continue;
            }
            let id = cx.set_id(node);
            let numbered = cx.tree.node(node);
            let view = NodeView {
                kind: numbered.kind,
                names: &numbered.attrs.names,
                refuri: numbered.str_attr("refuri"),
                refname: numbered.str_attr("refname"),
            };
            let duplicates = cx.state.names.bind(view, &id, false);
            debug_assert!(
                duplicates.is_empty(),
                "a node without ids has no names to duplicate"
            );
        }
    }
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

/// 210-020 `DefaultSubstitutions` (`SP/sphinx/transforms/__init__.py:111-150`):
/// each reference to one of the four default substitutions — `|today|`,
/// `|version|`, `|release|` and `|translation progress|` — that no
/// definition of that exact name overrides (`_DEFAULT_SUBSTITUTIONS -
/// set(self.document.substitution_defs)`, names as defined, case kept)
/// is replaced by a Text ([`default_substitution`]). It runs before
/// Substitutions (220), so the references in a definition are replaced
/// in the definition, whose copies then carry the text.
///
/// The loop is a live walk; each reference becomes one Text, so it skips
/// nothing.
pub struct DefaultSubstitutions;

/// `_DEFAULT_SUBSTITUTIONS` (`SP/sphinx/transforms/__init__.py:47-52`).
const DEFAULT_SUBSTITUTIONS: [&str; 4] = ["version", "release", "today", "translation progress"];

impl Transform for DefaultSubstitutions {
    fn apply(&self, cx: &mut TransformCx<'_>) {
        let registries = &cx.state.registries;
        let to_handle: Vec<&str> = DEFAULT_SUBSTITUTIONS
            .into_iter()
            .filter(|name| !registries.has_substitution_def(name))
            .collect();
        if to_handle.is_empty() {
            return;
        }
        let is_reference = |node: &TxNode| node.kind == kinds::SUBSTITUTION_REFERENCE;
        let mut walk = Findall::new(cx.tree.root());
        while let Some(reference) = walk.next(cx.tree, is_reference) {
            let name = cx.tree.node(reference).str_attr("refname");
            let Some(name) = name.filter(|name| to_handle.contains(name)) else {
                continue;
            };
            let name = name.to_string();
            let Some(text) = default_substitution(cx, &name, reference) else {
                return;
            };
            let node = cx.tree.new_node(kinds::TEXT);
            cx.tree.node_mut(node).text = Some(text);
            cx.tree.replace_self(reference, vec![node]);
        }
    }
}

/// `DefaultSubstitutions._handle_default_substitution`: the text of the
/// default substitution `name`, which `reference` names. `|today|` is the
/// `today` setting when it is set, otherwise the build date as
/// `format_date` formats it with `today_fmt`, or with `en`'s
/// `'%b %d, %Y'` when that is unset or empty ([`format_date`]);
/// `|version|` and `|release|` are the settings, empty by default;
/// `|translation progress|` is calculated from the stamp 025 left on the
/// document ([`translation_progress`]).
///
/// `None` once `format_date` has crashed Sphinx 9.1.0 (decision 13),
/// where the reference is: on a build date in a year Python's `datetime`
/// does not hold ([`BuildDate::OutOfRange`]), which it converts first; or
/// on a token babel rejects, `%U` or `%W`, for which `babel_format_date`
/// (`SP/sphinx/util/i18n.py:225-253`) takes the `ValueError` for an
/// unknown locale, warns `Invalid Babel locale` and formats again in
/// `en`, which raises it again.
fn default_substitution(cx: &mut TransformCx<'_>, name: &str, reference: TxId) -> Option<String> {
    let settings = cx.settings;
    Some(match name {
        "translation progress" => {
            let root = cx.tree.node(cx.tree.root());
            translation_progress(root.str_attr("translation_progress"))
        }
        "today" if !settings.today.is_empty() => settings.today.clone(),
        "today" => {
            let format = settings
                .today_fmt
                .as_deref()
                .filter(|format| !format.is_empty())
                .unwrap_or("%b %d, %Y");
            let date = match settings.build_date {
                BuildDate::Date(date) => date,
                BuildDate::OutOfRange(year) => {
                    let message = format!("year {year} is out of range");
                    cx.crash(
                        SphinxCrash::python("ValueError", message, LogLocation::None),
                        reference,
                    );
                    return None;
                }
            };
            match format_date(format, date) {
                Ok(today) => today,
                Err(BabelRejects) => {
                    // `if not locale: locale = 'en'`.
                    let locale = Some(settings.language.as_str())
                        .filter(|language| !language.is_empty())
                        .unwrap_or("en");
                    cx.warn_unlocated(
                        format!("Invalid Babel locale: {}.", py_repr_str(locale)),
                        Category::from_parts(Some("i18n"), Some("babel")),
                    );
                    let message = "Invalid length for field: 'WW'".to_string();
                    cx.crash(
                        SphinxCrash::python("ValueError", message, LogLocation::None),
                        reference,
                    );
                    return None;
                }
            }
        }
        "version" => settings.version.clone(),
        _ => settings.release.clone(),
    })
}

/// `_calculate_translation_progress` (`SP/sphinx/transforms/__init__.py:140-150`)
/// over the document's `translation_progress`, the repr of the dict 025
/// stamps (`{'total': T, 'translated': N}`): the share translated, as
/// Python's `f'{translated / total:.2%}'`, or `no translated elements!`
/// when nothing is translatable, as in every build here (020 is inert).
fn translation_progress(stamp: Option<&str>) -> String {
    let Some(stamp) = stamp else {
        return "could not calculate translation progress!".to_string();
    };
    let count = |key: &str| -> Option<i64> {
        let rest = stamp.split(&format!("'{key}': ")).nth(1)?;
        let digits = rest
            .split(|c: char| !c.is_ascii_digit() && c != '-')
            .next()?;
        digits.parse().ok()
    };
    match (count("total"), count("translated")) {
        (Some(total), Some(translated)) if total > 0 => {
            format!("{:.2}%", translated as f64 / total as f64 * 100.0)
        }
        _ => "no translated elements!".to_string(),
    }
}

/// babel rejecting a pattern `format_date` maps a strftime token to:
/// `ValueError: Invalid length for field: 'WW'` for `%U` and `%W`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BabelRejects;

/// Sphinx's `format_date(format, date=date, language='en')`
/// (`SP/sphinx/util/i18n.py:177-313`): `format` split on the strftime
/// tokens of `date_format_mappings`, in that table's order, each token
/// replaced by `date` formatted with the babel pattern it maps to, under
/// babel 2.18.0's `en` data, and every other piece kept as it is.
///
/// Only `en` is ported; another `language` formats as `en` (a documented
/// divergence, T19a). Two tokens map to the pattern `WW`, which babel
/// rejects, in every language: [`BabelRejects`] for the first of them.
pub(crate) fn format_date(format: &str, date: DateTime<Utc>) -> Result<String, BabelRejects> {
    let mut formatted = String::new();
    let mut rest = format;
    while let Some(first) = rest.chars().next() {
        let token = DATE_TOKENS
            .iter()
            .find(|(token, _)| rest.starts_with(*token));
        match token {
            Some(&(token, field)) => {
                formatted.push_str(&babel_en(field, date).ok_or(BabelRejects)?);
                rest = &rest[token.len()..];
            }
            None => {
                formatted.push(first);
                rest = &rest[first.len_utf8()..];
            }
        }
    }
    Ok(formatted)
}

/// What one strftime token of `date_format_mappings` formats in `en`.
#[derive(Debug, Clone, Copy)]
enum DateField {
    WeekdayShort,
    WeekdayLong,
    MonthShort,
    MonthLong,
    /// babel's `medium` datetime, date and time.
    MediumDateTime,
    MediumDate,
    MediumTime,
    /// A number, zero-padded to the width.
    Day(usize),
    Hour24(usize),
    Hour12(usize),
    DayOfYear(usize),
    Month(usize),
    Minute(usize),
    Second(usize),
    Period,
    /// `e`, the local day of the week.
    LocalWeekday,
    /// `W`, the week of the month.
    WeekOfMonth,
    /// `WW`, which babel rejects.
    Rejected,
    /// `YY`, the week-based year's last two digits.
    WeekYear,
    Year,
    ZoneName,
    ZoneOffset,
    Percent,
}

/// `date_format_mappings` (`SP/sphinx/util/i18n.py:177-219`), in its order,
/// which is the order the split's alternation tries them in.
const DATE_TOKENS: [(&str, DateField); 31] = [
    ("%a", DateField::WeekdayShort),
    ("%A", DateField::WeekdayLong),
    ("%b", DateField::MonthShort),
    ("%B", DateField::MonthLong),
    ("%c", DateField::MediumDateTime),
    ("%-d", DateField::Day(1)),
    ("%d", DateField::Day(2)),
    ("%-H", DateField::Hour24(1)),
    ("%H", DateField::Hour24(2)),
    ("%-I", DateField::Hour12(1)),
    ("%I", DateField::Hour12(2)),
    ("%-j", DateField::DayOfYear(1)),
    ("%j", DateField::DayOfYear(3)),
    ("%-m", DateField::Month(1)),
    ("%m", DateField::Month(2)),
    ("%-M", DateField::Minute(1)),
    ("%M", DateField::Minute(2)),
    ("%p", DateField::Period),
    ("%-S", DateField::Second(1)),
    ("%S", DateField::Second(2)),
    ("%U", DateField::Rejected),
    ("%w", DateField::LocalWeekday),
    ("%-W", DateField::WeekOfMonth),
    ("%W", DateField::Rejected),
    ("%x", DateField::MediumDate),
    ("%X", DateField::MediumTime),
    ("%y", DateField::WeekYear),
    ("%Y", DateField::Year),
    ("%Z", DateField::ZoneName),
    ("%z", DateField::ZoneOffset),
    ("%%", DateField::Percent),
];

/// babel's `en` weekday names, Monday first (`get_day_names`, format
/// context).
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// babel's `en` month names (`get_month_names`, format context).
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `field` of `date` as babel's `DateTimeFormat` formats it under `en`
/// (`SP/babel/dates.py`): names from `WEEKDAYS` and `MONTHS`, whose
/// abbreviations are their first three letters, numbers zero-padded to
/// the pattern's width (`'%0*d'`), the medium formats `MMM d, y`,
/// `h:mm:ss a` and `{1}, {0}` with a narrow no-break space before the
/// period, and the UTC zone's `UTC` and `+0000`. `None` for a pattern
/// babel rejects.
fn babel_en(field: DateField, date: DateTime<Utc>) -> Option<String> {
    let weekday = date.weekday().num_days_from_monday() as usize;
    let month = date.month0() as usize;
    let hour12 = match date.hour() % 12 {
        0 => 12,
        hour => hour,
    };
    let medium_date = format!("{} {}, {}", &MONTHS[month][..3], date.day(), date.year());
    let medium_time = format!(
        "{hour12}:{:02}:{:02}\u{202f}{}",
        date.minute(),
        date.second(),
        period(date)
    );
    Some(match field {
        DateField::WeekdayShort => WEEKDAYS[weekday][..3].to_string(),
        DateField::WeekdayLong => WEEKDAYS[weekday].to_string(),
        DateField::MonthShort => MONTHS[month][..3].to_string(),
        DateField::MonthLong => MONTHS[month].to_string(),
        DateField::MediumDateTime => format!("{medium_date}, {medium_time}"),
        DateField::MediumDate => medium_date,
        DateField::MediumTime => medium_time,
        DateField::Day(width) => format!("{:0width$}", date.day()),
        DateField::Hour24(width) => format!("{:0width$}", date.hour()),
        DateField::Hour12(width) => format!("{hour12:0width$}"),
        DateField::DayOfYear(width) => format!("{:0width$}", date.ordinal()),
        DateField::Month(width) => format!("{:0width$}", date.month()),
        DateField::Minute(width) => format!("{:0width$}", date.minute()),
        DateField::Second(width) => format!("{:0width$}", date.second()),
        DateField::Period => period(date).to_string(),
        // `value % 7 + 1` with `en`'s first week day, Monday.
        DateField::LocalWeekday => (weekday + 1).to_string(),
        DateField::WeekOfMonth => week_number(date.day(), weekday).to_string(),
        DateField::Rejected => return None,
        DateField::WeekYear => {
            let year = week_based_year(date);
            let digits = format!("{year:02}");
            digits[digits.len() - 2..].to_string()
        }
        DateField::Year => format!("{:04}", date.year()),
        DateField::ZoneName => "UTC".to_string(),
        DateField::ZoneOffset => "+0000".to_string(),
        DateField::Percent => "%".to_string(),
    })
}

/// babel's `a`: `en`'s `AM` or `PM`.
fn period(date: DateTime<Utc>) -> &'static str {
    if date.hour() >= 12 {
        "PM"
    } else {
        "AM"
    }
}

/// babel's `DateTimeFormat.get_week_number(day_of_period, day_of_week)`
/// under `en`, whose weeks start on Monday (`first_week_day = 0`) and
/// whose first week needs one day (`min_week_days = 1`): the week of the
/// period, a month or a year, holding its day `day_of_period` (1-based),
/// a `day_of_week` (Monday 0).
fn week_number(day_of_period: u32, day_of_week: usize) -> i64 {
    let day_of_period = i64::from(day_of_period);
    let first_day = (day_of_week as i64 - day_of_period + 1).rem_euclid(7);
    // `7 - first_day >= min_week_days` always holds with one day.
    (day_of_period + first_day - 1).div_euclid(7) + 1
}

/// babel's `format_year` for `Y` (`SP/babel/dates.py:1517-1528`): the
/// calendar year, less one for a day of early January in the previous
/// year's last week, plus one for a day of late December in the next
/// year's first week (`get_week_of_year`).
fn week_based_year(date: DateTime<Utc>) -> i32 {
    let weekday = date.weekday().num_days_from_monday() as usize;
    let year = date.year();
    // `get_week_of_year`.
    let mut week = week_number(date.ordinal(), weekday);
    if week == 0 {
        let last = NaiveDate::from_ymd_opt(year - 1, 12, 31);
        if let Some(last) = last {
            week = week_number(
                last.ordinal(),
                last.weekday().num_days_from_monday() as usize,
            );
        }
    } else if week > 52 {
        let next = NaiveDate::from_ymd_opt(year + 1, 1, 1);
        if let Some(next) = next {
            let next_weekday = next.weekday().num_days_from_monday() as i64;
            if week_number(1, next_weekday as usize) == 1
                && 32 - next_weekday.rem_euclid(7) <= i64::from(date.day())
            {
                week = 1;
            }
        }
    }
    if date.month() == 1 && date.day() < 7 && week >= 52 {
        year - 1
    } else if date.month() == 12 && date.day() > 25 && week <= 2 {
        year + 1
    } else {
        year
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
    use crate::transforms::test_support::{
        differences, harness_settings, index_rst, read_snippet, read_snippet_with, SphinxRead, DOC,
    };
    use crate::transforms::tree::TxTree;
    use crate::transforms::{read_transforms_with, NoOp, TransformId, TransformKey};
    use std::path::Path;

    /// The doctree stage R leaves for `rst`, each row run by `transform`.
    fn stage_r_with(
        rst: &str,
        transform: &dyn Fn(TransformId) -> &'static dyn Transform,
    ) -> Doctree {
        let parsed = crate::rst::parse_rst_full(rst, &index_rst());
        let settings = ReadSettings::sphinx_defaults();
        read_transforms_with(parsed, &settings, "index", Path::new("<SRCDIR>"), transform)
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
            srcdir: Path::new("<SRCDIR>"),
        });
        tree.into_doctree()
    }

    /// `PreserveTranslatableMessages` (010-034) keeps a toctree's entry
    /// titles as `rawentries` and its caption as `rawcaption`; with
    /// neither, `rawentries` is empty and there is no `rawcaption`. The
    /// node the directive makes has neither attribute: the parse leaves
    /// them to the read.
    //
    // oracle: the env fixture's `smartquotes_resolved` project (Sphinx
    //   9.1.0): `tocs_pformat["index"]`'s toctree, exactly as the first
    //   row's transformed tag; `TocTree.run`
    //   (`SP/sphinx/directives/other.py:64-87`) sets neither attribute.
    #[test]
    fn preserve_translatable_messages_keeps_a_toctrees_titles_and_caption() {
        fn toctree_tag(node: &Node) -> Option<String> {
            if node.kind == kinds::TOCTREE {
                return Some(crate::doctree::pformat::starttag(node));
            }
            node.children.iter().find_map(toctree_tag)
        }
        let found: std::collections::BTreeSet<String> =
            ["index", "a", "b"].map(String::from).into();
        let options = ParseOptions {
            found_docs: Some(std::sync::Arc::new(found)),
            ..index_rst()
        };
        let rows: [(&str, &str, &str); 2] = [
            (
                ".. toctree::\n   :caption: \"Contents\" -- here\n\n   a\n   \
                 \"Explicit\" title -- b <b>\n",
                "<toctree caption=\"\"Contents\" -- here\" entries=\"(None,\\ 'a') \
                 ('\"Explicit\"\\ title\\ --\\ b',\\ 'b')\" glob=\"0\" hidden=\"0\" \
                 includefiles=\"a b\" includehidden=\"0\" maxdepth=\"-1\" numbered=\"0\" \
                 parent=\"index\" titlesonly=\"0\">",
                "<toctree caption=\"\"Contents\" -- here\" entries=\"(None,\\ 'a') \
                 ('\"Explicit\"\\ title\\ --\\ b',\\ 'b')\" glob=\"0\" hidden=\"0\" \
                 includefiles=\"a b\" includehidden=\"0\" maxdepth=\"-1\" numbered=\"0\" \
                 parent=\"index\" rawcaption=\"\"Contents\" -- here\" \
                 rawentries=\"\"Explicit\"\\ title\\ --\\ b\" titlesonly=\"0\">",
            ),
            (
                ".. toctree::\n\n   a\n   b\n",
                "<toctree caption=\"True\" entries=\"(None,\\ 'a') (None,\\ 'b')\" glob=\"0\" \
                 hidden=\"0\" includefiles=\"a b\" includehidden=\"0\" maxdepth=\"-1\" \
                 numbered=\"0\" parent=\"index\" titlesonly=\"0\">",
                "<toctree caption=\"True\" entries=\"(None,\\ 'a') (None,\\ 'b')\" glob=\"0\" \
                 hidden=\"0\" includefiles=\"a b\" includehidden=\"0\" maxdepth=\"-1\" \
                 numbered=\"0\" parent=\"index\" rawentries=\"\" titlesonly=\"0\">",
            ),
        ];
        for (rst, parsed, preserved) in rows {
            let doctree = crate::rst::parse_rst(rst, &options);
            assert_eq!(
                toctree_tag(&doctree.root).as_deref(),
                Some(parsed),
                "{rst:?}"
            );
            let doctree = apply_row(doctree, TransformId::PreserveTranslatableMessages);
            assert_eq!(
                toctree_tag(&doctree.root).as_deref(),
                Some(preserved),
                "{rst:?}"
            );
        }
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
    /// An empty quote with a class crashes Sphinx where the walk reaches
    /// it, and only there: one the walk skips stays.
    #[test]
    fn handle_code_blocks_unwraps_doctest_only_block_quotes() {
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
        // with no node asserts that no basic attribute is lost, naming the
        // first of `ids`, `classes`, `names` and `dupnames` that holds a
        // value (decision 13). The read stops there, where the quote is. An
        // empty epigraph the walk skips, after an empty quote it removed,
        // does not crash it (probed: exit 0).
        let crash = |rst: &str| {
            let parsed = crate::rst::parse_rst_full(rst, &index_rst());
            let settings = ReadSettings::sphinx_defaults();
            let read = read_transforms_with(
                parsed,
                &settings,
                "index",
                Path::new("<SRCDIR>"),
                &TransformId::transform,
            );
            read.state()
                .crash
                .as_ref()
                .map(|crash| (crash.to_string(), crash.rendered_location.clone()))
        };
        assert_eq!(
            crash("Para.\n\n.. epigraph::\n\n   .. default-domain:: py\n"),
            Some((
                "AssertionError: Losing \"classes\" attribute: ['epigraph']".to_string(),
                Some("<SRCDIR>/index.rst:5".to_string())
            ))
        );
        let mut quote = TxTree::from_parse(
            Doctree {
                root: element(kinds::BLOCK_QUOTE, Vec::new()),
                sources: Vec::new(),
            },
            &mut DocumentState::default(),
        );
        let root = quote.root();
        quote.node_mut(root).attrs.dupnames.push("d".to_string());
        quote.node_mut(root).attrs.names.push("it's".to_string());
        assert_eq!(
            losing_basic_attribute(quote.node(root)).as_deref(),
            Some("Losing \"names\" attribute: [\"it's\"]")
        );
        assert_eq!(
            crash(
                "Para.\n\n   .. default-domain:: py\n\n.. epigraph::\n\n   \
                 .. default-domain:: py\n\nText.\n"
            ),
            None
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
        read_snippet(rst, false).body
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

    /// `DefaultSubstitutions` (210-020, `SP/sphinx/transforms/__init__.py:111-150`)
    /// replaces each reference to `|today|`, `|version|`, `|release|` or
    /// `|translation progress|` that no definition of that exact name
    /// overrides by a Text: `today` when it is set, otherwise the build
    /// date formatted with `today_fmt` or `'%b %d, %Y'`; `version` and
    /// `release` as configured, empty by default (an empty Text prints
    /// nothing); and, from 025's stamp, `no translated elements!`. It runs
    /// before Substitutions (220), so a definition's `|today|` is replaced
    /// before the definition is copied.
    //
    // oracle (Sphinx 9.1.0): each input read through
    //   tools/gen_read_state_fixture.py's harness, with `today`, `version`
    //   and `release`, or `today_fmt`, set in the configuration and
    //   `SOURCE_DATE_EPOCH=1700000000` (2023-11-14T22:13:20Z).
    #[test]
    fn default_substitutions_fill_today_version_release() {
        let dated =
            |today: &str, today_fmt: Option<&str>, version: &str, release: &str| ReadSettings {
                today: today.to_string(),
                today_fmt: today_fmt.map(str::to_string),
                version: version.to_string(),
                release: release.to_string(),
                build_date: BuildDate::Date(
                    chrono::DateTime::from_timestamp(1_700_000_000, 0)
                        .expect("a representable date"),
                ),
                ..harness_settings(true)
            };
        let rows: [(&str, ReadSettings, &str); 4] = [
            (
                "Built |today|, version |version|, release |release|.\n",
                dated("April 1, 2026", Some("%Y"), "1.2", "1.2.3"),
                "    <paragraph>\n        Built \n        April 1, 2026\n        , version \n        \
                 1.2\n        , release \n        1.2.3\n        .\n",
            ),
            (
                "Built |today|.\n",
                dated("", Some("%B %d, %Y (%A)"), "", ""),
                "    <paragraph>\n        Built \n        November 14, 2023 (Tuesday)\n        .\n",
            ),
            (
                "Built |today| and |translation progress|; |version| is mine.\n\n\
                 .. |version| replace:: mine\n.. |d| replace:: on |today|\n\nCopy |d|.\n",
                dated("", None, "1.2", ""),
                "    <paragraph>\n        Built \n        Nov 14, 2023\n         and \n        \
                 no translated elements!\n        ; \n        mine\n         is mine.\n    \
                 <substitution_definition names=\"version\">\n        mine\n    \
                 <substitution_definition names=\"d\">\n        on \n        Nov 14, 2023\n    \
                 <paragraph>\n        Copy \n        on \n        Nov 14, 2023\n        .\n",
            ),
            (
                "Use |x| and |version| and |release|.\n\n.. |x| replace:: v |version| e\n",
                dated("", None, "", ""),
                "    <paragraph>\n        Use \n        v \n         e\n         and \n         \
                 and \n        .\n    <substitution_definition names=\"x\">\n        v \n         \
                 e\n",
            ),
        ];
        for (rst, settings, body) in rows {
            let read = read_snippet_with(rst, &settings);
            assert_eq!(read.body, body, "{rst:?}");
            assert!(read.warnings.is_empty(), "{rst:?}: {:#?}", read.warnings);
        }
        // Without 025's stamp the progress cannot be calculated.
        assert_eq!(
            translation_progress(None),
            "could not calculate translation progress!"
        );
        assert_eq!(
            translation_progress(Some("{'total': 0, 'translated': 0}")),
            "no translated elements!"
        );
        assert_eq!(
            translation_progress(Some("{'total': 8, 'translated': 1}")),
            "12.50%"
        );
    }

    /// The `en` port of Sphinx's `format_date` (`SP/sphinx/util/i18n.py:177-313`):
    /// each strftime token maps to a babel pattern, formatted with babel's
    /// `en` data (weeks start on Monday, the first week has one day), and
    /// everything else stays as it is.
    //
    // oracle (Sphinx 9.1.0, babel 2.18.0):
    //   `format_date(fmt, date=d, language='en')` for each `d` and `fmt`.
    #[test]
    fn format_date_ports_babel_en() {
        let at = |y, m, d, h, min, s| {
            chrono::NaiveDate::from_ymd_opt(y, m, d)
                .and_then(|date| date.and_hms_opt(h, min, s))
                .expect("a valid date")
                .and_utc()
        };
        let sunday = at(2026, 1, 4, 15, 7, 9);
        // `%U` and `%W` map to `WW`, which babel rejects, wherever they
        // stand in the format.
        for format in ["%U", "%W", "%Y week %W", "%W %U"] {
            assert_eq!(
                super::format_date(format, sunday),
                Err(BabelRejects),
                "{format:?}"
            );
        }
        let format_date = |format: &str, date| {
            super::format_date(format, date).expect("babel formats every other token")
        };
        for (format, formatted) in [
            ("%a", "Sun"),
            ("%A", "Sunday"),
            ("%b", "Jan"),
            ("%B", "January"),
            ("%c", "Jan 4, 2026, 3:07:09\u{202f}PM"),
            ("%-d", "4"),
            ("%d", "04"),
            ("%-H", "15"),
            ("%H", "15"),
            ("%-I", "3"),
            ("%I", "03"),
            ("%-j", "4"),
            ("%j", "004"),
            ("%-m", "1"),
            ("%m", "01"),
            ("%-M", "7"),
            ("%M", "07"),
            ("%p", "PM"),
            ("%-S", "9"),
            ("%S", "09"),
            ("%w", "7"),
            ("%-W", "1"),
            ("%x", "Jan 4, 2026"),
            ("%X", "3:07:09\u{202f}PM"),
            ("%y", "26"),
            ("%Y", "2026"),
            ("%Z", "UTC"),
            ("%z", "+0000"),
            ("%%", "%"),
            ("%e", "%e"),
            ("%b %d, %Y", "Jan 04, 2026"),
            ("%%a", "%a"),
            ("%-", "%-"),
            ("%", "%"),
            ("x%", "x%"),
            ("%B %-d, %Y", "January 4, 2026"),
            ("Built %A", "Built Sunday"),
        ] {
            assert_eq!(format_date(format, sunday), formatted, "{format:?}");
        }
        // `%w`: Monday 1 to Sunday 7, across a week.
        let weekdays: Vec<String> = (4..=11)
            .map(|day| format_date("%a %w", at(2026, 1, day, 0, 0, 0)))
            .collect();
        assert_eq!(
            weekdays,
            ["Sun 7", "Mon 1", "Tue 2", "Wed 3", "Thu 4", "Fri 5", "Sat 6", "Sun 7"]
        );
        // `%-W`, the week of the month, weeks starting on Monday.
        let weeks: Vec<String> = (1..=31)
            .map(|day| format_date("%-W", at(2026, 3, day, 0, 0, 0)))
            .collect();
        let expected = "1 2 2 2 2 2 2 2 3 3 3 3 3 3 3 4 4 4 4 4 4 4 5 5 5 5 5 5 5 6 6";
        assert_eq!(weeks.join(" "), expected);
        // `%y` is the week-based year's last two digits; `%-j`, `%j` the
        // day of the year.
        for ((y, m, d), formatted) in [
            ((2020, 12, 26), "20 2020 361 361 Sat"),
            ((2020, 12, 31), "21 2020 366 366 Thu"),
            ((2021, 1, 1), "21 2021 1 001 Fri"),
            ((2021, 1, 4), "21 2021 4 004 Mon"),
            ((2024, 12, 30), "25 2024 365 365 Mon"),
            ((2016, 1, 1), "16 2016 1 001 Fri"),
            ((2026, 12, 31), "27 2026 365 365 Thu"),
            ((2032, 12, 26), "32 2032 361 361 Sun"),
            ((2032, 12, 31), "33 2032 366 366 Fri"),
            ((2033, 1, 1), "33 2033 1 001 Sat"),
        ] {
            assert_eq!(
                format_date("%y %Y %-j %j %a", at(y, m, d, 0, 0, 0)),
                formatted,
                "{y}-{m}-{d}"
            );
        }
        // The 12-hour clock and the period, around midnight and noon.
        for (hour, formatted) in [
            (
                0,
                "Nov 23, 2026, 12:05:06\u{202f}AM|Nov 23, 2026|12:05:06\u{202f}AM|AM|12|12|00|0",
            ),
            (
                1,
                "Nov 23, 2026, 1:05:06\u{202f}AM|Nov 23, 2026|1:05:06\u{202f}AM|AM|01|1|01|1",
            ),
            (
                12,
                "Nov 23, 2026, 12:05:06\u{202f}PM|Nov 23, 2026|12:05:06\u{202f}PM|PM|12|12|12|12",
            ),
            (
                13,
                "Nov 23, 2026, 1:05:06\u{202f}PM|Nov 23, 2026|1:05:06\u{202f}PM|PM|01|1|13|13",
            ),
            (
                23,
                "Nov 23, 2026, 11:05:06\u{202f}PM|Nov 23, 2026|11:05:06\u{202f}PM|PM|11|11|23|23",
            ),
        ] {
            assert_eq!(
                format_date("%c|%x|%X|%p|%I|%-I|%H|%-H", at(2026, 11, 23, hour, 5, 6)),
                formatted,
                "{hour}h"
            );
        }
        let months: Vec<String> = (1..=12)
            .map(|month| format_date("%b %B", at(2026, month, 1, 0, 0, 0)))
            .collect();
        assert_eq!(
            months,
            [
                "Jan January",
                "Feb February",
                "Mar March",
                "Apr April",
                "May May",
                "Jun June",
                "Jul July",
                "Aug August",
                "Sep September",
                "Oct October",
                "Nov November",
                "Dec December"
            ]
        );
        // Year 5: `%Y` pads to four digits, the medium formats' `y` does not.
        assert_eq!(
            format_date("%Y-%m-%d|%c", at(5, 1, 1, 0, 0, 0)),
            "0005-01-01|Jan 1, 5, 12:00:00\u{202f}AM"
        );
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
            let read =
                read_transforms_with(parsed, &settings, "index", Path::new("<SRCDIR>"), transform);
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
                srcdir: Path::new("<SRCDIR>"),
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

    /// `AutoNumbering` (210-025, `SP/sphinx/transforms/__init__.py:200-214`)
    /// gives every figure, table and code-block container that has a
    /// caption or a title and no ids an implicit id from the shared
    /// counter, once the parse is over: `docutils-0534`'s table and an
    /// unlabelled captioned `code-block` take `id1`, and a captioned
    /// `code-block` under a label takes `id1` before PropagateTargets
    /// (260) adds the label's `mylabel`. The ids the parse allocated come
    /// first (the table after a problematic takes `id3`), and a node a
    /// `:name:` gave ids takes none. Each row is a Sphinx 9.1.0 read of the
    /// input as `index.rst` (the read-state harness, `keep_warnings =
    /// True`), the figures' images stamped and warned about by the image
    /// collector (880-030): `docutils-0490`, and `docutils-0492` without a
    /// caption, which takes no id.
    #[test]
    fn autonumbering_ids_captioned_enumerables() {
        let rows = [
            SphinxRead {
                // docutils-0534.
                rst: ".. table:: Table Title\n   :widths: 30 70\n   :align: center\n\n   \
                      =====  =====\n   A      B\n   =====  =====\n   1      2\n   =====  =====\n",
                body: "    <table align=\"center\" classes=\"colwidths-given\" ids=\"id1\">\n        \
                       <title>\n            Table Title\n        <tgroup cols=\"2\">\n            \
                       <colspec colwidth=\"30\">\n            <colspec colwidth=\"70\">\n            \
                       <thead>\n                <row>\n                    \
                       <entry>\n                        <paragraph>\n                            \
                       A\n                    <entry>\n                        \
                       <paragraph>\n                            B\n            <tbody>\n                \
                       <row>\n                    <entry>\n                        \
                       <paragraph>\n                            1\n                    \
                       <entry>\n                        <paragraph>\n                            2\n",
                warnings: &[],
                trace: &[("AutoNumbering", "table", "id1", true)],
            },
            SphinxRead {
                rst: ".. code-block:: python\n   :caption: cap\n\n   pass\n",
                body: "    <container classes=\"literal-block-wrapper\" ids=\"id1\" \
                       literal_block=\"1\">\n        <caption>\n            cap\n        <literal_block \
                       force=\"0\" highlight_args=\"{}\" language=\"python\" \
                       xml:space=\"preserve\">\n            pass\n",
                warnings: &[],
                trace: &[("AutoNumbering", "container", "id1", true)],
            },
            SphinxRead {
                rst: ".. _mylabel:\n\n.. code-block:: python\n   :caption: cap\n\n   pass\n",
                body: "    <target refid=\"mylabel\">\n    <container classes=\"literal-block-wrapper\" \
                       ids=\"id1 mylabel\" literal_block=\"1\" names=\"mylabel\">\n        \
                       <caption>\n            cap\n        <literal_block force=\"0\" \
                       highlight_args=\"{}\" language=\"python\" xml:space=\"preserve\">\n            \
                       pass\n",
                warnings: &[],
                trace: &[
                    ("parse", "target", "mylabel", true),
                    ("AutoNumbering", "container", "id1", true),
                ],
            },
            SphinxRead {
                rst: ".. table:: Cap\n\n   =  =\n   a  b\n   =  =\n\nText *x.\n",
                body: "    <table ids=\"id3\">\n        <title>\n            Cap\n        <tgroup \
                       cols=\"2\">\n            <colspec colwidth=\"1\">\n            <colspec \
                       colwidth=\"1\">\n            <tbody>\n                <row>\n                    \
                       <entry>\n                        <paragraph>\n                            \
                       a\n                    <entry>\n                        \
                       <paragraph>\n                            b\n    <paragraph>\n        \
                       Text \n        <problematic ids=\"id2\" refid=\"id1\">\n            *\n        \
                       x.\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"2\" line=\"7\" \
                       source=\"<SRCDIR>/index.rst\" type=\"WARNING\">\n        <paragraph>\n            \
                       Inline emphasis start-string without end-string.\n",
                warnings: &["<SRCDIR>/index.rst:7: WARNING: Inline emphasis start-string without \
                             end-string. [docutils]"],
                trace: &[
                    ("parse", "system_message", "id1", true),
                    ("parse", "problematic", "id2", true),
                    ("AutoNumbering", "table", "id3", true),
                ],
            },
            SphinxRead {
                rst: ".. table:: Cap\n   :name: tab\n\n   =  =\n   a  b\n   =  =\n",
                body: "    <table ids=\"tab\" names=\"tab\">\n        <title>\n            Cap\n        \
                       <tgroup cols=\"2\">\n            <colspec colwidth=\"1\">\n            <colspec \
                       colwidth=\"1\">\n            <tbody>\n                <row>\n                    \
                       <entry>\n                        <paragraph>\n                            \
                       a\n                    <entry>\n                        \
                       <paragraph>\n                            b\n",
                warnings: &[],
                trace: &[("parse", "table", "tab", true)],
            },
            SphinxRead {
                // docutils-0490.
                rst: ".. figure:: pic.png\n\n   Caption text.\n",
                body: "    <figure ids=\"id1\">\n        <image candidates=\"{'*': 'pic.png'}\" \
                       uri=\"pic.png\">\n        <caption>\n            Caption text.\n",
                warnings: &["<SRCDIR>/index.rst:1: WARNING: image file not readable: pic.png \
                             [image.not_readable]"],
                trace: &[("AutoNumbering", "figure", "id1", true)],
            },
            SphinxRead {
                // docutils-0492.
                rst: ".. figure:: pic.png\n",
                body: "    <figure>\n        <image candidates=\"{'*': 'pic.png'}\" \
                       uri=\"pic.png\">\n",
                warnings: &["<SRCDIR>/index.rst:1: WARNING: image file not readable: pic.png \
                             [image.not_readable]"],
                trace: &[],
            },
        ];
        let mismatches = differences(&rows);
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
    }

    /// A `literalinclude`'s caption container is numbered by AutoNumbering
    /// (210-025) like any captioned code block: after every id the parse
    /// allocated, the sentinel's `id5`/`id6` included, so it takes `id7`
    /// (Sphinx 9.1.0, the read-state harness reading the input as
    /// `index.rst` beside a `code.py` holding `pass`).
    #[test]
    fn a_literalinclude_caption_is_numbered_after_the_parse() {
        let tmp = tempfile::tempdir().expect("a temporary source directory");
        std::fs::write(tmp.path().join("code.py"), "pass\n").expect("code.py writes");
        let rst = "Title\n=====\n\n.. literalinclude:: code.py\n   :caption: Cap :nod:bogus:`x` \
                   *open\n\n.. literalinclude:: code.py\n   :caption: ----------\n\nSentinel \
                   :nod:sentinel:`z`.\n";
        let parse = ParseOptions {
            source_path: tmp.path().join("index.rst").display().to_string(),
            srcdir: Some(tmp.path().to_path_buf()),
            ..index_rst()
        };
        let read = crate::read::read_standalone(rst, &parse, &harness_settings(true));
        let trace: Vec<(&str, &str, &str, bool)> = read
            .outcome
            .state()
            .ids
            .trace()
            .iter()
            .map(|event| {
                let phase = event.phase.rsplit('.').next().unwrap_or_default();
                (phase, event.kind, event.id.as_str(), event.attached)
            })
            .collect();
        assert_eq!(
            trace,
            [
                ("parse", "section", "title", true),
                ("parse", "system_message", "id1", false),
                ("parse", "problematic", "id2", true),
                ("parse", "system_message", "id3", false),
                ("parse", "problematic", "id4", true),
                ("parse", "system_message", "id5", true),
                ("parse", "problematic", "id6", true),
                ("AutoNumbering", "container", "id7", true),
            ]
        );
        let pformat = read.outcome.doctree().root.pformat();
        assert!(
            pformat.contains(
                "        <container classes=\"literal-block-wrapper\" ids=\"id7\" \
                 literal_block=\"1\">\n"
            ),
            "{pformat}"
        );
    }
}
