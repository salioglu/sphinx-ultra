//! Sphinx's own read transforms that belong to no docutils family
//! (`sphinx/transforms/__init__.py`). Today: FilterSystemMessages.

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
