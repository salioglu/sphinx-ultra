//! Sphinx's own read transforms that belong to no docutils family
//! (`sphinx/transforms/__init__.py`). Today: MoveModuleTargets,
//! ReorderConsecutiveTargetAndIndexNodes, SortIds and FilterSystemMessages.

use super::TransformCtx;
use crate::doctree::{kinds, AttrValue, Node};

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
    let mut stack = vec![&mut ctx.tree.root];
    while let Some(node) = stack.pop() {
        if node.kind == kinds::SECTION {
            while node.children.get(2).is_some_and(is_module_target) {
                let target = node.children.remove(2);
                node.attrs.ids.splice(0..0, target.attrs.ids);
            }
        }
        stack.extend(node.children.iter_mut());
    }
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
    let mut stack = vec![&mut ctx.tree.root];
    while let Some(node) = stack.pop() {
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
        stack.extend(children.iter_mut());
    }
}

/// `SortIds` (`sphinx/transforms/__init__.py:217-225`, priority 261, right
/// after PropagateTargets): a section with more than one id whose first id
/// starts with `id` has that id moved to the end (`:224-225`). Meant for a
/// docutils auto id (`id1`), it matches any id with the prefix — a section
/// titled "Identity" given a label gets `ids="lbl identity"`, and its toc
/// anchor and HTML id become the label's. `names` are left alone.
pub(super) fn sort_ids(ctx: &mut TransformCtx) {
    let mut stack = vec![&mut ctx.tree.root];
    while let Some(node) = stack.pop() {
        let ids = &mut node.attrs.ids;
        if node.kind == kinds::SECTION && ids.len() > 1 && ids[0].starts_with("id") {
            ids.rotate_left(1);
        }
        stack.extend(node.children.iter_mut());
    }
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
    use crate::rst::ParseOptions;
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
        let mut records = Vec::new();
        apply_read_transforms(
            &mut tree,
            IdRegistry::new(),
            0,
            "index",
            &keeping_warnings(),
            &mut records,
        );
        assert_eq!(message_levels(&tree.root), [2]);
        assert_eq!(tree.root.children[0].children.len(), 1);
        assert!(records.is_empty());
    }
}
