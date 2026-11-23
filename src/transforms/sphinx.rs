//! Sphinx's own read transforms (`SP/sphinx/transforms/`), ported one row
//! of the read-transform table at a time ([`super::READ_TRANSFORMS`]).

use super::{Transform, TransformCx};
use crate::doctree::{kinds, AttrValue};

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
