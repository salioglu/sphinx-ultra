//! docutils `pending` nodes ([SD §7.1]): a placeholder a directive leaves
//! in the tree for a transform that can only run once the whole document
//! is parsed. docutils keeps the transform and its `details` on the node
//! object, outside the attributes, and `pending.pformat` prints them after
//! the element's children as an `.. internal attributes:` block
//! (`nodes.py:2567-2589`). The node carries a [`Pending`] in its side-car
//! ([`super::NodeExt::pending`]) and pformat prints [`Pending::internals`].

use serde::{Deserialize, Serialize};

/// The transforms a `pending` node can defer under Sphinx 9.1.0's
/// directive registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PendingTransform {
    /// `class` / `rst-class` without content: applies the classes to the
    /// next element.
    ClassAttribute,
    /// `target-notes`.
    TargetNotes,
    /// `sectnum` / `section-numbering`.
    SectNum,
    /// `contents`.
    Contents,
}

impl PendingTransform {
    /// The transform class's `default_priority`, which orders it in the
    /// read stage (a pending transform runs after the registered ones of
    /// the same priority).
    pub const fn priority(self) -> u16 {
        match self {
            PendingTransform::ClassAttribute => 210,
            PendingTransform::TargetNotes => 540,
            PendingTransform::SectNum => 710,
            PendingTransform::Contents => 720,
        }
    }

    /// The transform class's dotted path, `'%s.%s' % (__module__,
    /// __name__)`, as pformat prints it after `.transform:`.
    pub const fn python_path(self) -> &'static str {
        match self {
            PendingTransform::ClassAttribute => "docutils.transforms.misc.ClassAttribute",
            PendingTransform::TargetNotes => "docutils.transforms.references.TargetNotes",
            PendingTransform::SectNum => "docutils.transforms.parts.SectNum",
            PendingTransform::Contents => "docutils.transforms.parts.Contents",
        }
    }
}

/// A Python value held in a pending node's `details`, as pformat prints it
/// with `%r`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PyValue {
    None,
    Int(i64),
    Str(String),
    StrList(Vec<String>),
}

impl PyValue {
    /// Python `repr()` of the value.
    fn repr(&self) -> String {
        match self {
            PyValue::None => "None".to_string(),
            PyValue::Int(n) => n.to_string(),
            PyValue::Str(s) => crate::utils::py_repr_str(s),
            PyValue::StrList(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(|item| crate::utils::py_repr_str(item))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// What a `pending` node defers: docutils' `pending.transform` and
/// `pending.details`, and the node's place in the document's
/// `note_pending` order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub transform: PendingTransform,
    /// 0-based `document.note_pending` order within the document: the
    /// order in which pending transforms of equal priority run.
    pub order: u32,
    /// Sorted by key, as `sorted(self.details.items())` prints them.
    pub details: Vec<(String, PyValue)>,
}

impl Pending {
    /// `nodes.pending(transform, details)` noted `order`-th: the details
    /// sorted by key, the order pformat prints them in
    /// (`sorted(self.details.items())`, `nodes.py:2567-2589`), and in which
    /// they are stored and compared. A Python dict holds each key once.
    pub fn new(
        transform: PendingTransform,
        order: u32,
        mut details: Vec<(String, PyValue)>,
    ) -> Pending {
        details.sort_by(|(a, _), (b, _)| a.cmp(b));
        debug_assert!(
            details.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "a pending node's details repeat a key"
        );
        Pending {
            transform,
            order,
            details,
        }
    }

    /// The `.. internal attributes:` lines of docutils' `pending.pformat`
    /// (`nodes.py:2567-2589`), without their indent: pformat prefixes each
    /// with `'    ' + indent * level`, the indent of the node's children.
    pub fn internals(&self) -> Vec<String> {
        let mut lines = vec![
            ".. internal attributes:".to_string(),
            format!("     .transform: {}", self.transform.python_path()),
            "     .details:".to_string(),
        ];
        lines.extend(
            self.details
                .iter()
                .map(|(key, value)| format!("       {key}: {}", value.repr())),
        );
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::{kinds, Doctree, Node, Span};

    fn parse_docutils(src: &str) -> Doctree {
        crate::rst::parse_rst(
            src,
            &crate::rst::ParseOptions {
                source_path: "<snippet>".into(),
                ..Default::default()
            },
        )
    }

    fn pending_of(node: &Node) -> &Pending {
        node.ext
            .as_deref()
            .and_then(|ext| ext.pending.as_deref())
            .expect("a pending node carries its Pending")
    }

    /// A docutils-mode `.. class::` without content is a `pending` node
    /// whose transform and details live in the side-car. Its internals are
    /// the block the parse-layer fixture pins for every such case
    /// (`dir_media.class_pending`, `dir_media.class_two_values`, …). The
    /// old stand-in printed the same block from a `#text` child, so only
    /// the side-car tells the two apart.
    //
    // oracle (docutils 0.22.4, parse layer): parse('.. class:: x\n')[0] is a
    //   pending with no children, transform misc.ClassAttribute, details
    //   {'class': ['x'], 'directive': 'class'}; pformat as asserted below.
    #[test]
    fn pending_internals_match_docutils() {
        let tree = parse_docutils(".. class:: x\n");
        let pending = &tree.root.children[0];
        assert_eq!(pending.kind, "pending");
        assert!(
            pending.children.is_empty(),
            "the internals live in the side-car, not in a #text child"
        );
        let expected = Pending::new(
            PendingTransform::ClassAttribute,
            0,
            vec![
                ("class".to_string(), PyValue::StrList(vec!["x".to_string()])),
                ("directive".to_string(), PyValue::Str("class".to_string())),
            ],
        );
        let ext = pending
            .ext
            .as_deref()
            .expect("a pending node carries a side-car");
        assert_eq!(ext.pending, Some(Box::new(expected)));
        assert_eq!(
            pending_of(pending).internals(),
            [
                ".. internal attributes:",
                "     .transform: docutils.transforms.misc.ClassAttribute",
                "     .details:",
                "       class: ['x']",
                "       directive: 'class'",
            ]
        );
        assert_eq!(
            tree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <pending>\n",
                "        .. internal attributes:\n",
                "             .transform: docutils.transforms.misc.ClassAttribute\n",
                "             .details:\n",
                "               class: ['x']\n",
                "               directive: 'class'\n",
            )
        );
    }

    /// `order` is the document's `note_pending` order.
    //
    // oracle (docutils 0.22.4): `.. class:: a\n\n.. class:: b\n\nPara.\n`
    //   schedules ('210-001', class ['a']) then ('210-002', class ['b']).
    #[test]
    fn pending_order_follows_note_pending() {
        let tree = parse_docutils(".. class:: a\n\n.. class:: b\n\nPara.\n");
        let orders: Vec<(u32, &PyValue)> = tree
            .root
            .children
            .iter()
            .filter(|node| node.kind == "pending")
            .map(|node| {
                let pending = pending_of(node);
                (pending.order, &pending.details[0].1)
            })
            .collect();
        assert_eq!(
            orders,
            [
                (0, &PyValue::StrList(vec!["a".to_string()])),
                (1, &PyValue::StrList(vec!["b".to_string()])),
            ]
        );
    }

    /// Details print `%r`-style, one per line in key order, and an empty
    /// `details` prints only `.details:`; a nested pending's internals sit
    /// at its children's indent.
    //
    // oracle (docutils 0.22.4): pending(parts.Contents, {'local': None,
    //   'depth': 2, 'backlinks': 'top', 'class': ['a', "it's"], 'empty': []})
    //   and section(pending(parts.Contents, {})) pformat as asserted below.
    #[test]
    fn pending_details_print_python_reprs() {
        let mut node = Node::elem("pending", Span::ZERO);
        node.ext = Some(Box::new(crate::doctree::NodeExt {
            pending: Some(Box::new(Pending::new(
                PendingTransform::Contents,
                0,
                vec![
                    ("backlinks".to_string(), PyValue::Str("top".to_string())),
                    (
                        "class".to_string(),
                        PyValue::StrList(vec!["a".to_string(), "it's".to_string()]),
                    ),
                    ("depth".to_string(), PyValue::Int(2)),
                    ("empty".to_string(), PyValue::StrList(Vec::new())),
                    ("local".to_string(), PyValue::None),
                ],
            ))),
            ..Default::default()
        }));
        assert_eq!(
            node.pformat(),
            concat!(
                "<pending>\n",
                "    .. internal attributes:\n",
                "         .transform: docutils.transforms.parts.Contents\n",
                "         .details:\n",
                "           backlinks: 'top'\n",
                "           class: ['a', \"it's\"]\n",
                "           depth: 2\n",
                "           empty: []\n",
                "           local: None\n",
            )
        );

        if let Some(pending) = node.ext.as_mut().and_then(|ext| ext.pending.as_mut()) {
            pending.details.clear();
        }
        let mut section = Node::elem(kinds::SECTION, Span::ZERO);
        section.children.push(node);
        assert_eq!(
            section.pformat(),
            concat!(
                "<section>\n",
                "    <pending>\n",
                "        .. internal attributes:\n",
                "             .transform: docutils.transforms.parts.Contents\n",
                "             .details:\n",
            )
        );
    }

    /// [`Pending::new`] keeps the details sorted by key, as pformat prints
    /// them (`sorted(self.details.items())`), whatever order a directive
    /// gives them in: they are printed and compared in stored order.
    //
    // oracle (docutils 0.22.4): `.. contents::` with `:local:`, `:depth: 2`
    //   and `:backlinks: top` in that order prints `backlinks`, `depth`,
    //   `local` (the parse-layer fixture's `dir_parts` rows).
    #[test]
    fn pending_new_sorts_its_details() {
        let pending = Pending::new(
            PendingTransform::Contents,
            3,
            vec![
                ("local".to_string(), PyValue::None),
                ("depth".to_string(), PyValue::Int(2)),
                ("backlinks".to_string(), PyValue::Str("top".to_string())),
            ],
        );
        assert_eq!(pending.transform, PendingTransform::Contents);
        assert_eq!(pending.order, 3);
        let keys: Vec<&str> = pending
            .details
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(keys, ["backlinks", "depth", "local"]);
    }

    // oracle (docutils 0.22.4): misc.ClassAttribute 210,
    //   references.TargetNotes 540, parts.SectNum 710, parts.Contents 720.
    #[test]
    fn pending_transforms_name_their_docutils_classes() {
        let rows = [
            (
                PendingTransform::ClassAttribute,
                210,
                "docutils.transforms.misc.ClassAttribute",
            ),
            (
                PendingTransform::TargetNotes,
                540,
                "docutils.transforms.references.TargetNotes",
            ),
            (
                PendingTransform::SectNum,
                710,
                "docutils.transforms.parts.SectNum",
            ),
            (
                PendingTransform::Contents,
                720,
                "docutils.transforms.parts.Contents",
            ),
        ];
        for (transform, priority, path) in rows {
            assert_eq!(transform.priority(), priority);
            assert_eq!(transform.python_path(), path);
        }
    }
}
