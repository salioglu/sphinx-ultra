//! Sphinx's own read transforms that belong to no docutils family
//! (`sphinx/transforms/__init__.py`, `sphinx/transforms/i18n.py`) —
//! PreserveTranslatableMessages, MoveModuleTargets, HandleCodeBlocks,
//! AutoNumbering, ReorderConsecutiveTargetAndIndexNodes, SortIds,
//! DoctestTransform and FilterSystemMessages — and docutils' Transitions
//! (`docutils/transforms/misc.py`).

use super::references::{collect_paths, update_basic_atts};
use super::{for_each_node_mut, node_at, node_at_mut, NodePath, TransformCtx};
use crate::doctree::ids::make_id;
use crate::doctree::{kinds, messages, AttrValue, Node, RAWSOURCE};
use crate::env::numbers::std_numfig_title;
use crate::env::std_domain::is_none_sentinel;
use crate::utils::parse_py_str_tuple;

/// `PreserveTranslatableMessages` (`sphinx/transforms/i18n.py:103-111`,
/// priority 10, the first read transform): every `translatable` node —
/// in Sphinx 9.1 only the `toctree` (`sphinx/addnodes.py:58`) — records
/// the messages a translation would replace (`preserve_original_messages`,
/// `addnodes.py:61-70`): `rawentries`, set to `[]` if absent, gains every
/// truthy entry title in `entries` order (an explicit `Title <ref>`
/// entry's; a bare one's is `None`), and a truthy `caption` is copied to
/// `rawcaption`. Probed: Sphinx's parse leaves neither attribute, so they
/// are written here, not by the `toctree` directive.
///
/// `entries` holds each `(title, ref)` as its Python repr
/// ([`crate::env::toctree::ResolvedEntries::entries_attr`]), read back with
/// [`parse_py_str_tuple`]. An absent `caption` is the parser's `"True"`
/// rendering of `None` ([`is_none_sentinel`]), so `:caption: True` — a
/// truthy caption in Sphinx — reads as none here, the sentinel's known
/// limit.
pub(super) fn preserve_translatable_messages(ctx: &mut TransformCtx) {
    for_each_node_mut(&mut ctx.tree.root, |node| {
        if node.kind != kinds::TOCTREE {
            return;
        }
        let mut rawentries = match node.get("rawentries") {
            Some(AttrValue::List(titles)) => titles.clone(),
            _ => Vec::new(),
        };
        if let Some(AttrValue::List(entries)) = node.get("entries") {
            rawentries.extend(
                entries
                    .iter()
                    .filter_map(|entry| parse_py_str_tuple(entry)?.into_iter().next()?)
                    .filter(|title| !title.is_empty()),
            );
        }
        node.set("rawentries", AttrValue::List(rawentries));
        if let Some(AttrValue::Str(caption)) = node.get("caption") {
            if !caption.is_empty() && !is_none_sentinel(caption) {
                let caption = caption.clone();
                node.set("rawcaption", AttrValue::Str(caption));
            }
        }
    });
}

/// `HandleCodeBlocks` (`sphinx/transforms/__init__.py:178-197`, priority
/// 210): a `block_quote` whose children are all `doctest_block`s is
/// replaced by them (`:186-188`) — an indented `>>>` block becomes a plain
/// doctest block. `replace_self` hands the quote's ids, classes, names and
/// dupnames to the first of them (`update_basic_atts`, `docutils/
/// nodes.py:1120-1132`). `all()` of no children is true, so an empty block
/// quote is replaced by nothing.
///
/// Upstream walks the document's live iterator, checking each block quote
/// before it descends into it: an outer quote whose child is an inner
/// quote of doctest blocks is not unwrapped (its child is no doctest block
/// when it is checked), the inner one is. Rewriting each node's children
/// before the walk descends into them is the same order.
pub(super) fn handle_code_blocks(ctx: &mut TransformCtx) {
    for_each_node_mut(&mut ctx.tree.root, |node| {
        let mut index = 0;
        while index < node.children.len() {
            let child = &node.children[index];
            if child.kind == kinds::BLOCK_QUOTE
                && child
                    .children
                    .iter()
                    .all(|grandchild| grandchild.kind == kinds::DOCTEST_BLOCK)
            {
                let quote = &mut node.children[index];
                let mut doctests = std::mem::take(&mut quote.children);
                if let Some(first) = doctests.first_mut() {
                    update_basic_atts(first, quote);
                }
                let count = doctests.len();
                node.children.splice(index..=index, doctests);
                index += count;
            } else {
                index += 1;
            }
        }
    });
}

/// `AutoNumbering` (`sphinx/transforms/__init__.py:200-214`, priority 210,
/// ahead of PropagateTargets at 260): every node the standard domain
/// numbers — a `figure`, `table` or `container`, by exact class
/// (`is_enumerable_node`, `sphinx/domains/std/__init__.py:799-803,
/// 1363-1364`) — that has a numfig title (a `caption` or `title` child,
/// even an empty one: `get_numfig_title`, `:1366-1378`, is then `''`, not
/// `None`) and no ids yet gets an implicit target
/// (`document.note_implicit_target`): the next auto id, no name. Document
/// order, from the parse's continued registry ([`TransformCtx::ids`]), so
/// the number follows every auto id the parse handed out. A label written
/// above the node reaches it only later, after the auto id (probed: `..
/// _f:` + a captioned figure is `ids="id1 f"`), and numbering files the
/// node under `ids[0]`, the auto id (`collectors/toctree.py:334`).
pub(super) fn auto_numbering(ctx: &mut TransformCtx) {
    let unnumbered = collect_paths(&ctx.tree.root, |node| {
        matches!(node.kind, "figure" | kinds::TABLE | "container")
            && std_numfig_title(node).is_some()
            && node.attrs.ids.is_empty()
    });
    for path in unnumbered {
        let id = ctx.ids.allocate_auto_id();
        if let Some(node) = node_at_mut(&mut ctx.tree.root, &path) {
            node.attrs.ids.push(id);
        }
    }
}

/// `DoctestTransform` (`sphinx/transforms/__init__.py:327-334`, priority
/// 500): every `doctest_block` gets the `doctest` class — appended, as
/// upstream appends it, whatever classes the block already has.
pub(super) fn doctest_transform(ctx: &mut TransformCtx) {
    for_each_node_mut(&mut ctx.tree.root, |node| {
        if node.kind == kinds::DOCTEST_BLOCK {
            node.attrs.classes.push("doctest".to_string());
        }
    });
}

/// `Transition must be child of <document> or <section>.`
/// (`docutils/transforms/misc.py:105-106`).
const NOT_IN_A_SECTION: &str = "Transition must be child of <document> or <section>.";
/// `misc.py:107-111`.
const BEGINS_A_SECTION: &str = "Document or section may not begin with a transition.";
/// `misc.py:112-114`.
const FOLLOWS_A_TRANSITION: &str =
    "At least one body element must separate transitions; adjacent transitions are not allowed.";
/// `misc.py:133-135`.
const ENDS_THE_DOCUMENT: &str = "Document may not end with a transition.";

/// docutils' `Transitions` (`docutils/transforms/misc.py:64-143`, priority
/// 830): each `transition`, in document order, is checked where it stands
/// and, ending a section, moved up.
///
/// Misplaced — not a child of the document or a section (`:105-106`),
/// first in its parent or right after a title, subtitle, meta or
/// decoration (`:107-111`), or right after another transition
/// (`:112-114`) — it warns, `base_node=` the transition, so the reporter
/// prints the record at the transition's line when it is created
/// (`:116`). The message goes into the tree right after the transition
/// only if the parent validates with a paragraph in the transition's
/// place (`:117-126`; [`takes_a_body_element_at`]); FilterSystemMessages
/// (999) strips it again unless `keep_warnings`.
///
/// A document's or section's last child (counting its message) moves up
/// past every ancestor it also ends, to right after the first one with a
/// following sibling (`:127-143`); one that ends the document stays, and
/// `Document may not end with a transition.` is appended to its parent
/// unvalidated (`:133-137`).
///
/// Upstream iterates the live tree, so a transition it moved up is met
/// again in its new place — after the section it left, with something
/// following it: nothing to report, nowhere to move. Moving never changes
/// the transitions' document order (a moved one ended the subtree it
/// leaves, and lands right after it), so each one is found as the first
/// transition after the last one handled, and handled exactly once.
pub(super) fn transitions(ctx: &mut TransformCtx) {
    let mut after = None;
    while let Some(path) = next_transition(&ctx.tree.root, after.as_deref()) {
        after = Some(visit_transition(ctx, path));
    }
}

/// The first `transition` after the node at `after` in document order —
/// its descendants first — or after the root's start with no `after`.
/// One pass from there by an explicit stack of sibling cursors.
fn next_transition(root: &Node, after: Option<&[usize]>) -> Option<NodePath> {
    let after = after.unwrap_or(&[]);
    // Each frame: a node and the next index among its children; `path`
    // addresses the top frame's node.
    let mut frames: Vec<(&Node, usize)> = Vec::with_capacity(after.len() + 1);
    let mut node = root;
    for &index in after {
        frames.push((node, index + 1));
        node = node.children.get(index)?;
    }
    frames.push((node, 0));
    let mut path: NodePath = after.to_vec();
    while let Some(frame) = frames.last_mut() {
        let (node, index) = *frame;
        let Some(child) = node.children.get(index) else {
            frames.pop();
            path.pop();
            continue;
        };
        frame.1 += 1;
        path.push(index);
        if child.kind == kinds::TRANSITION {
            return Some(path);
        }
        frames.push((child, 0));
    }
    None
}

/// `Transitions.visit_transition` (`misc.py:98-143`) for the transition at
/// `path`; returns where it is afterwards.
fn visit_transition(ctx: &mut TransformCtx, path: NodePath) -> NodePath {
    let Some((&at, parent_path)) = path.split_last() else {
        return path;
    };
    let Some(parent) = node_at(&ctx.tree.root, parent_path) else {
        return path;
    };
    let structural = matches!(parent.kind, kinds::DOCUMENT | kinds::SECTION);
    let previous = at.checked_sub(1).map(|index| parent.children[index].kind);
    let text = if !structural {
        Some(NOT_IN_A_SECTION)
    } else if matches!(
        previous,
        None | Some(kinds::TITLE | kinds::SUBTITLE | "meta" | "decoration")
    ) {
        Some(BEGINS_A_SECTION)
    } else if previous == Some(kinds::TRANSITION) {
        Some(FOLLOWS_A_TRANSITION)
    } else {
        None
    };
    let span = parent.children[at].span;
    let mut index = at;
    if let Some(text) = text {
        let warning = ctx.message_at(messages::WARNING, text, (span.source, span.line));
        ctx.reporter.report(&warning);
        let parent = node_at_mut(&mut ctx.tree.root, parent_path).expect("the parent is there");
        if takes_a_body_element_at(parent, at) {
            parent.children.insert(at + 1, warning);
            index += 1;
        }
    }
    if !structural {
        return path;
    }
    let root = &ctx.tree.root;
    if index + 1 != node_at(root, parent_path).map_or(0, |parent| parent.children.len()) {
        return path;
    }
    // `sibling` climbs while it is its parent's last child; `ancestor` is
    // its path.
    let mut ancestor: &[usize] = parent_path;
    let index = loop {
        let Some((&index, up)) = ancestor.split_last() else {
            // `sibling` is the document: the transition ends it.
            let warning = ctx.message_at(
                messages::WARNING,
                ENDS_THE_DOCUMENT,
                (span.source, span.line),
            );
            ctx.reporter.report(&warning);
            let parent = node_at_mut(&mut ctx.tree.root, parent_path).expect("the parent is there");
            parent.children.push(warning);
            return path;
        };
        if index + 1 != node_at(root, up).map_or(0, |parent| parent.children.len()) {
            break index;
        }
        ancestor = up;
    };
    let (_, up) = ancestor.split_last().expect("the loop broke on a child");
    let up = up.to_vec();
    let parent = node_at_mut(&mut ctx.tree.root, parent_path).expect("the parent is there");
    let transition = parent.children.remove(at);
    let new_parent = node_at_mut(&mut ctx.tree.root, &up).expect("the ancestor is there");
    new_parent.children.insert(index + 1, transition);
    let mut moved = up;
    moved.push(index + 1);
    moved
}

/// Whether `parent` would validate (`Element.validate(recursive=False)`,
/// `docutils/nodes.py:1330-1356`) with a paragraph in place of its child
/// at `index` — Transitions' test for attaching its message there
/// (`misc.py:117-126`).
///
/// A `document` never does in a Sphinx build: `TranslationProgressTotaliser`
/// (25) has given it a `translation_progress` attribute, which is no valid
/// attribute of it (`validate_attributes`, `nodes.py:1239-1266`; probed:
/// no document-level transition message is attached). Neither does any
/// other parent but a `section`: a transition below anything else is
/// Sphinx's doing (`only`, an object description's content), and those
/// elements have no content model (`Element.content_model = ()`, `:555`;
/// probed with `only`) — the parser never builds docutils' own body
/// elements around one (a nested transition is a parse error).
fn takes_a_body_element_at(parent: &Node, index: usize) -> bool {
    parent.kind == kinds::SECTION && section_validates(parent, index)
}

/// `section.validate(recursive=False)` with a paragraph at `paragraph`:
/// its attributes, each a valid section attribute (`ids`, `classes`,
/// `names`, `dupnames`, `source`) with ids and classes `make_id`-clean
/// (`validate_identifier_list`, `nodes.py:3137-3171`; a module's
/// `module-a.b` id is not), then its children against the content model
/// (`:1677-1681`) — a title, an optional subtitle, body elements, topics,
/// sidebars and transitions, then sections and transitions, nothing else
/// — each transition among them also placed validly
/// (`transition.validate_position`, `:1614-1636`): neither first nor
/// right after a title, subtitle, meta or decoration, nor last, nor right
/// after a transition.
fn section_validates(section: &Node, paragraph: usize) -> bool {
    let attrs = &section.attrs;
    let clean = |ids: &[String]| ids.iter().all(|id| make_id(id) == *id);
    if !attrs.backrefs.is_empty()
        || attrs.extra.iter().any(|(key, _)| *key != RAWSOURCE)
        || !clean(&attrs.ids)
        || !clean(&attrs.classes)
    {
        return false;
    }
    let kinds: Vec<&str> = section
        .children
        .iter()
        .enumerate()
        .map(|(at, child)| {
            if at == paragraph {
                kinds::PARAGRAPH
            } else {
                child.kind
            }
        })
        .collect();
    let placed = |at: usize| {
        kinds[at] != kinds::TRANSITION
            || (at + 1 < kinds.len()
                && !matches!(
                    kinds[at - 1],
                    kinds::TITLE | kinds::SUBTITLE | "meta" | "decoration" | kinds::TRANSITION
                ))
    };
    if kinds.first() != Some(&kinds::TITLE) {
        return false;
    }
    let mut at = 1;
    if kinds.get(at) == Some(&kinds::SUBTITLE) {
        at += 1;
    }
    while at < kinds.len()
        && (is_body_element(kinds[at])
            || matches!(kinds[at], "topic" | "sidebar" | kinds::TRANSITION))
    {
        if !placed(at) {
            return false;
        }
        at += 1;
    }
    while at < kinds.len() && matches!(kinds[at], kinds::SECTION | kinds::TRANSITION) {
        if !placed(at) {
            return false;
        }
        at += 1;
    }
    at == kinds.len()
}

/// `isinstance(node, nodes.Body)` by tagname: every docutils and Sphinx
/// (`sphinx/addnodes.py`) element class deriving from it, as probed from
/// the installed docutils 0.22.4 / Sphinx 9.1.0. Not among them, so ending
/// a section's body model: Sphinx's `only`, `highlightlang`, `glossary`,
/// `hlist`, `tabular_col_spec`, `centered`, `acks`, `start_of_file`.
fn is_body_element(kind: &str) -> bool {
    matches!(
        kind,
        "admonition"
            | "attention"
            | "block_quote"
            | "bullet_list"
            | "caution"
            | "citation"
            | "comment"
            | "compact_paragraph"
            | "compound"
            | "container"
            | "danger"
            | "definition_list"
            | "desc"
            | "desc_content"
            | "doctest_block"
            | "download_reference"
            | "enumerated_list"
            | "error"
            | "field_list"
            | "figure"
            | "footnote"
            | "hint"
            | "image"
            | "important"
            | "index"
            | "line_block"
            | "literal_block"
            | "math_block"
            | "note"
            | "number_reference"
            | "option_list"
            | "paragraph"
            | "pending"
            | "productionlist"
            | "raw"
            | "reference"
            | "rubric"
            | "seealso"
            | "substitution_definition"
            | "system_message"
            | "table"
            | "target"
            | "tip"
            | "toctree"
            | "versionmodified"
            | "warning"
    )
}

/// `FilterSystemMessages` (`sphinx/transforms/__init__.py:337-347`,
/// priority 999, the last read transform): every `system_message` whose
/// `level` is below `2 if keep_warnings else 5` (`:343`) is removed from
/// its parent — with `keep_warnings` off (the default) every message,
/// SEVERE (4) included; with it on, INFO (1) and DEBUG (0) only.
/// `document.findall` visits every message, descendants of a kept one
/// included (`:344`), and removing one takes its subtree with it.
///
/// Nothing is printed: each message was written to the warning stream when
/// the parse (or a transform) created it, and this transform only logs the
/// removal at DEBUG (`:346`), which no build prints by default.
pub(super) fn filter_system_messages(ctx: &mut TransformCtx) {
    let filterlevel = if ctx.config.keep_warnings { 2 } else { 5 };
    remove_messages_below(&mut ctx.tree.root, filterlevel);
}

/// `MoveModuleTargets` (`sphinx/transforms/__init__.py:153-175`, priority
/// 210): a `py:module` target that is the first thing in a section — the
/// section's third child, after its title and the module's index node
/// (`:168-172`; `py:module` emits `[index, target]`) — is absorbed by the
/// section: its ids are prepended to the section's (`:174`) and it is
/// removed (`:175`). The index node stays. A target with no ids, one
/// without the `ismod` attribute, or one at any other position is left to
/// PropagateTargets (`:164-172`) — a `:no-index-entry:` module's target
/// sits at index 1, so it is propagated onto the next node instead.
///
/// Upstream walks a snapshot of every target (`:163`), so once a module
/// target leaves index 2 the target behind it moves up into that slot and,
/// if it is a module target too, is absorbed when its turn comes, its ids
/// going first. Draining the slot section by section is the same thing:
/// only the removal at index 2 moves a sibling into it.
pub(super) fn move_module_targets(ctx: &mut TransformCtx) {
    for_each_node_mut(&mut ctx.tree.root, |node| {
        if node.kind == kinds::SECTION {
            while node.children.get(2).is_some_and(is_module_target) {
                let target = node.children.remove(2);
                node.attrs.ids.splice(0..0, target.attrs.ids);
            }
        }
    });
}

/// `node['ids'] and 'ismod' in node` for a `target` (`:164-167`).
fn is_module_target(node: &Node) -> bool {
    node.kind == kinds::TARGET && !node.attrs.ids.is_empty() && node.get("ismod").is_some()
}

/// `ReorderConsecutiveTargetAndIndexNodes` (`sphinx/transforms/
/// __init__.py:446-515`, priority 220, before PropagateTargets so that the
/// index nodes between targets do not stop a label reaching its node): for
/// each target, the run of `target`/`index` siblings it starts
/// (`findall(descend=False, siblings=True)`, `:491-495`) is stably sorted
/// with every index node ahead of every target (`_sort_key`, `:508-515`)
/// when it holds two or more nodes (`:497-505`; the run is all one
/// parent's consecutive children by construction).
///
/// Upstream visits targets with the document's live iterator while it
/// re-slices the parent (same length, so the iterator keeps its place): a
/// target sorted further along is visited again — as the head of a run of
/// targets alone, a no-op — and an index node sorted back past the
/// iterator is not, having no target below it to miss. Visiting each
/// child slot of each parent left to right is therefore the same.
pub(super) fn reorder_consecutive_target_and_index_nodes(ctx: &mut TransformCtx) {
    for_each_node_mut(&mut ctx.tree.root, |node| {
        let children = &mut node.children;
        for start in 0..children.len() {
            if children[start].kind != kinds::TARGET {
                continue;
            }
            let end = start
                + children[start..]
                    .iter()
                    .take_while(|child| matches!(child.kind, kinds::TARGET | "index"))
                    .count();
            if end - start >= 2 {
                children[start..end].sort_by_key(|child| child.kind != "index");
            }
        }
    });
}

/// `SortIds` (`sphinx/transforms/__init__.py:217-225`, priority 261, right
/// after PropagateTargets): a section with more than one id whose first id
/// starts with `id` has that id moved to the end (`:224-225`). Meant for a
/// docutils auto id (`id1`), it matches any id with the prefix — a section
/// titled "Identity" given a label gets `ids="lbl identity"`, and its toc
/// anchor and HTML id become the label's. `names` are left alone.
pub(super) fn sort_ids(ctx: &mut TransformCtx) {
    for_each_node_mut(&mut ctx.tree.root, |node| {
        let ids = &mut node.attrs.ids;
        if node.kind == kinds::SECTION && ids.len() > 1 && ids[0].starts_with("id") {
            ids.rotate_left(1);
        }
    });
}

fn remove_messages_below(node: &mut Node, filterlevel: i64) {
    node.children.retain(|child| {
        !(child.kind == kinds::SYSTEM_MESSAGE
            && matches!(child.get("level"), Some(AttrValue::Int(level)) if *level < filterlevel))
    });
    for child in &mut node.children {
        remove_messages_below(child, filterlevel);
    }
}

#[cfg(test)]
mod tests {
    use crate::doctree::ids::IdRegistry;
    use crate::doctree::{kinds, messages, AttrValue, Doctree, Node, Span};
    use crate::rst::{ParseOptions, RegistryExport};
    use crate::transforms::{apply_read_transforms, parse_and_transform, TransformConfig};

    /// One message of every level the parser creates, in sphinx mode, in
    /// document order: WARNING (inline markup, after its paragraph), INFO
    /// then ERROR (an unknown directive: docutils' `No directive entry for
    /// "nosuchdirective"` lookup note, then `Unknown directive type`),
    /// SEVERE (a `raw` directive whose file does not exist), and INFO again
    /// (the second `Dup` section's duplicate implicit name, inside that
    /// section after its title).
    const EVERY_LEVEL: &str = "Dup\n===\n\nPara *bad.\n\n.. nosuchdirective::\n\n\
                               .. raw:: html\n   :file: nonexistent-raw-input.html\n\n\
                               Dup\n===\n\ny\n";

    fn opts() -> ParseOptions {
        ParseOptions {
            source_path: "<snippet>".to_string(),
            sphinx: true,
            ..Default::default()
        }
    }

    /// The `level` of every `system_message` in `node`, in document order.
    fn message_levels(node: &Node) -> Vec<i64> {
        let mut levels = Vec::new();
        if node.kind == kinds::SYSTEM_MESSAGE {
            if let Some(AttrValue::Int(level)) = node.get("level") {
                levels.push(*level);
            }
        }
        for child in &node.children {
            levels.extend(message_levels(child));
        }
        levels
    }

    fn levels_after(config: &TransformConfig) -> Vec<i64> {
        message_levels(&parse_and_transform(EVERY_LEVEL, &opts(), config).0.root)
    }

    fn keeping_warnings() -> TransformConfig {
        TransformConfig {
            keep_warnings: true,
            ..TransformConfig::default()
        }
    }

    #[test]
    fn the_snippet_raises_a_message_of_every_level() {
        let parsed = crate::rst::parse_rst(EVERY_LEVEL, &opts());
        assert_eq!(message_levels(&parsed.root), [2, 1, 3, 4, 1]);
    }

    /// `filterlevel = 2 if self.config.keep_warnings else 5`
    /// (`transforms/__init__.py:343`): with `keep_warnings` at its `False`
    /// default the filter level is one above SEVERE (4), so every message
    /// the parse left in the tree goes — SEVERE included. The printed
    /// records are untouched: they were written when the messages were
    /// created, and the transform only logs at DEBUG (`:346`).
    #[test]
    fn filter_system_messages_drops_everything_below_severe_by_default() {
        let (tree, records) =
            parse_and_transform(EVERY_LEVEL, &opts(), &TransformConfig::default());
        assert_eq!(message_levels(&tree.root), Vec::<i64>::new());
        let pformat = tree.root.pformat();
        assert!(
            pformat.contains("<problematic"),
            "the inline `problematic` stays behind its removed message: {pformat}"
        );
        let parse_records = crate::rst::parse_rst_full(EVERY_LEVEL, &opts())
            .registry
            .diagnostics;
        assert_eq!(records, parse_records, "the filter prints nothing");
        assert_eq!(records.len(), 3, "WARNING, ERROR and SEVERE printed");
    }

    /// `keep_warnings = True` lowers the filter level to 2: WARNING, ERROR
    /// and SEVERE stay in place, the INFO goes.
    #[test]
    fn keep_warnings_keeps_levels_two_and_up() {
        assert_eq!(levels_after(&keeping_warnings()), [2, 3, 4]);
    }

    /// Below level 2 nothing survives under either setting — including a
    /// DEBUG message, and an INFO nested inside a WARNING the filter keeps
    /// (`document.findall(nodes.system_message)` visits the descendants of
    /// every message too, `transforms/__init__.py:344`).
    #[test]
    fn info_messages_never_survive() {
        for config in [TransformConfig::default(), keeping_warnings()] {
            assert!(
                !levels_after(&config).contains(&1),
                "an INFO survived under keep_warnings={}",
                config.keep_warnings
            );
        }

        let mut warning = messages::system_message(messages::WARNING, "kept", 0, 3, "<snippet>");
        warning.children.push(messages::system_message(
            messages::INFO,
            "nested",
            0,
            3,
            "<snippet>",
        ));
        let mut debug = messages::system_message(messages::INFO, "debug", 0, 2, "<snippet>");
        debug.set("level", AttrValue::Int(0));
        debug.set("type", AttrValue::Str("DEBUG".to_string()));
        let mut quote = Node::elem(kinds::BLOCK_QUOTE, Span::ZERO);
        quote.children.push(debug);
        quote.children.push(warning);
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        root.children.push(quote);
        let mut tree = Doctree {
            root,
            sources: vec!["<snippet>".to_string()],
        };
        let mut registry = RegistryExport::default();
        apply_read_transforms(
            &mut tree,
            IdRegistry::new(),
            0,
            None,
            "index",
            &keeping_warnings(),
            &mut registry,
        );
        assert_eq!(message_levels(&tree.root), [2]);
        assert_eq!(tree.root.children[0].children.len(), 1);
        assert!(registry.diagnostics.is_empty());
    }

    /// The `(level, line, text)` of every record a read printed.
    fn printed(records: &[crate::rst::diagnostics::Diagnostic]) -> Vec<(u8, Option<u32>, &str)> {
        records
            .iter()
            .map(|d| (d.level, d.line, d.text.as_str()))
            .collect()
    }

    /// Transitions (830) attaches its warnings — so FilterSystemMessages
    /// (999) strips them again under the default `keep_warnings=False` —
    /// but the reporter printed each when Transitions created it
    /// (`docutils/utils/__init__.py:213-215`): one record, located at the
    /// transition (`base_node=node`, `misc.py:113,135-137`), either way.
    #[test]
    fn transition_warnings_print_whatever_keep_warnings_keeps() {
        let src = "Para.\n\n----\n";
        let expected = [(2, Some(3), "Document may not end with a transition.")];
        let (tree, records) = parse_and_transform(src, &opts(), &TransformConfig::default());
        assert_eq!(printed(&records), expected);
        let kinds: Vec<&str> = tree.root.children.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [kinds::PARAGRAPH, kinds::TRANSITION]);

        let (tree, records) = parse_and_transform(src, &opts(), &keeping_warnings());
        assert_eq!(printed(&records), expected);
        let kinds: Vec<&str> = tree.root.children.iter().map(|n| n.kind).collect();
        assert_eq!(
            kinds,
            [kinds::PARAGRAPH, kinds::TRANSITION, kinds::SYSTEM_MESSAGE]
        );
    }

    /// A transition ending a section moves up past every ancestor it also
    /// ends, to right after the first one that has a following sibling
    /// (`misc.py:124-143`) — here four levels, to the document.
    #[test]
    fn a_transition_ending_nested_sections_moves_past_them_all() {
        let src = "A\n=\n\nB\n-\n\nC\n~\n\nD\n+\n\nd\n\n----\n\nE\n=\n\ne\n";
        let (tree, records) = parse_and_transform(src, &opts(), &keeping_warnings());
        assert_eq!(records, []);
        let kinds: Vec<&str> = tree.root.children.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [kinds::SECTION, kinds::TRANSITION, kinds::SECTION]);
        let mut deepest = &tree.root.children[0];
        while let Some(section) = deepest.children.iter().find(|n| n.kind == kinds::SECTION) {
            deepest = section;
        }
        assert_eq!(
            deepest.children.last().map(|n| n.kind),
            Some(kinds::PARAGRAPH)
        );
    }

    /// Every transition is visited once, whatever moving it did — no input
    /// makes the pass loop. Each section here begins with a transition and
    /// ends with a second, which moves up after it; the last one cannot
    /// move and ends the document (probed with three sections: records at
    /// lines 4, 6, 11, 13, 18, 20, 20).
    #[test]
    fn transitions_are_each_visited_once() {
        let sections = 200;
        let src: String = (0..sections)
            .map(|i| format!("S{i}\n====\n\n----\n\n----\n\n"))
            .collect();
        let (tree, records) = parse_and_transform(&src, &opts(), &TransformConfig::default());
        assert_eq!(records.len(), 2 * sections + 1);
        assert_eq!(
            records.last().map(|d| d.text.as_str()),
            Some("Document may not end with a transition.")
        );
        assert_eq!(tree.root.children.len(), 2 * sections - 1);
    }

    /// `Transition must be child of <document> or <section>.`
    /// (`misc.py:105-106`): printed, and nothing else — no Sphinx parent of
    /// a transition other than a document or section validates with a
    /// paragraph in its place, so the message is not attached, and only a
    /// document's or section's transition ever moves (`:118-119`). (The
    /// parser never builds this tree — a nested transition is a parse-time
    /// error — so it is built by hand.)
    #[test]
    fn a_transition_outside_a_section_only_prints() {
        let at = |line| Span {
            source: 0,
            line,
            start: 0,
            end: 0,
        };
        let mut quote = Node::elem(kinds::BLOCK_QUOTE, at(1));
        quote.children.push(Node::elem(kinds::PARAGRAPH, at(1)));
        quote.children.push(Node::elem(kinds::TRANSITION, at(3)));
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        root.children.push(quote);
        root.children.push(Node::elem(kinds::PARAGRAPH, at(5)));
        let mut tree = Doctree {
            root,
            sources: vec!["<snippet>".to_string()],
        };
        let before = tree.root.clone();
        let mut registry = RegistryExport::default();
        apply_read_transforms(
            &mut tree,
            IdRegistry::new(),
            0,
            None,
            "index",
            &keeping_warnings(),
            &mut registry,
        );
        assert_eq!(
            printed(&registry.diagnostics),
            [(
                2,
                Some(3),
                "Transition must be child of <document> or <section>."
            )]
        );
        assert_eq!(tree.root, before);
    }

    /// HandleCodeBlocks (210) unwraps a block quote whose children are all
    /// doctest blocks (`transforms/__init__.py:186-188`) — vacuously, one
    /// with no children at all, which it replaces with nothing. (The parser
    /// never builds an empty block quote; built by hand.)
    #[test]
    fn an_empty_block_quote_is_unwrapped_to_nothing() {
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        root.children
            .push(Node::elem(kinds::BLOCK_QUOTE, Span::ZERO));
        root.children.push(Node::elem(kinds::PARAGRAPH, Span::ZERO));
        let mut tree = Doctree {
            root,
            sources: vec!["<snippet>".to_string()],
        };
        apply_read_transforms(
            &mut tree,
            IdRegistry::new(),
            0,
            None,
            "index",
            &keeping_warnings(),
            &mut RegistryExport::default(),
        );
        let kinds: Vec<&str> = tree.root.children.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [kinds::PARAGRAPH]);
    }

    /// `replace_self` hands the unwrapped quote's ids, classes, names and
    /// dupnames to its first doctest block (`update_basic_atts`,
    /// `docutils/nodes.py:1120-1132`), after the block's own and skipping
    /// ones it has; the later blocks get nothing. (The parser stamps a
    /// pending class inside the quote instead — its own divergence — so the
    /// quote is built by hand.)
    #[test]
    fn an_unwrapped_quote_hands_its_attributes_to_the_first_doctest_block() {
        let mut first = Node::elem(kinds::DOCTEST_BLOCK, Span::ZERO);
        first.attrs.classes.push("own".to_string());
        let mut quote = Node::elem(kinds::BLOCK_QUOTE, Span::ZERO);
        quote.attrs.classes = vec!["special".to_string(), "own".to_string()];
        quote.attrs.ids.push("q".to_string());
        quote.attrs.names.push("q".to_string());
        quote.children.push(first);
        quote
            .children
            .push(Node::elem(kinds::DOCTEST_BLOCK, Span::ZERO));
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        root.children.push(quote);
        let mut tree = Doctree {
            root,
            sources: vec!["<snippet>".to_string()],
        };
        apply_read_transforms(
            &mut tree,
            IdRegistry::new(),
            0,
            None,
            "index",
            &keeping_warnings(),
            &mut RegistryExport::default(),
        );
        assert_eq!(
            tree.root.pformat(),
            "<document>\n    \
             <doctest_block classes=\"own special doctest\" ids=\"q\" names=\"q\">\n    \
             <doctest_block classes=\"doctest\">\n"
        );
    }
}
