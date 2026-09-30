//! docutils' reference transforms (`docutils/transforms/references.py`),
//! which Sphinx's read phase inherits from the standalone reader. Today:
//! PropagateTargets.

use super::{node_at, node_at_mut, NodePath, TransformCtx};
use crate::doctree::{kinds, AttrValue, Node};

/// `PropagateTargets` (`docutils/transforms/references.py:17-95`, priority
/// 260): every block-level target without a reference of its own — a
/// `.. _label:`, an anonymous `.. __:`, the target of an `index` or
/// `py:module` directive — hands its `ids` and `names` on to the node after
/// it, *appended* to that node's own (`:60-61`), and keeps only
/// `refid = <its first id>` (`:91-95`). Targets are taken in document order
/// (`findall(nodes.target)`, `:43`), so a run of targets collapses onto the
/// last one first and then onto the node behind the run: `.. _a:` `.. _b:`
/// para gives `<target refid="a"><target refid="b"><paragraph ids="b a">`.
///
/// Skipped (`:45-48`): a target inside a `TextElement` — an inline target,
/// the one an `:index:` or `:envvar:` role puts beside its text (in a
/// paragraph, a line, a title…), and any target in a `versionmodified`
/// body, whose content sits directly in that TextElement (probed) — and one
/// that already carries `refid`, `refuri` or `refname`.
///
/// The receiving node is `target.next_node(ascend=True)` (`:50`) — the next
/// sibling, else the nearest ancestor's next sibling — stepping over
/// `system_message`s whole (`next_node(ascend=True, descend=False)`,
/// `:52-53`): they are still in the tree at 260, FilterSystemMessages only
/// runs at 999. Nothing is propagated when there is no such node, or when it
/// is `Invisible` (comment, substitution definition, pending, index) or
/// `Targetable` (footnote, citation) other than a target (`:56-59`).
///
/// A target that is a `figure`'s child followed by the figure's `caption`
/// is removed instead of keeping a `refid` (`:85-88`). The removals are
/// made after the walk: the walk only ever looks forward from a target, so
/// a removed target is never in the way of a later one, and the caption
/// docutils' iterator skips after the removal holds no target the walk
/// would propagate (its targets have a TextElement parent).
///
/// `document.ids` (`:67-69`) and `document.refids` (`note_refid`, `:95`)
/// are the walk-built [`super::DocumentLists`], which see the moved ids
/// and the new `refid`s. Not ported: the `expect_referenced_by_*`
/// bookkeeping (`:62-80`), which only marks targets `referenced` for
/// DanglingReferences' "Hyperlink target is not referenced" INFO — a
/// message Sphinx's SphinxDanglingReferences never lets through
/// (`sphinx/transforms/references.py:21-30`).
pub(super) fn propagate_targets(ctx: &mut TransformCtx) {
    let root = &mut ctx.tree.root;
    let mut removed: Vec<NodePath> = Vec::new();
    for path in collect_targets(root) {
        let Some((target, parent)) = node_at(root, &path).zip(parent_of(root, &path)) else {
            continue;
        };
        if is_text_element(parent.kind)
            || ["refid", "refuri", "refname"]
                .into_iter()
                .any(|key| target.get(key).is_some())
        {
            continue;
        }
        // `assert len(target) == 0` (`:49`) — upstream aborts on a
        // block-level target with children; none reaches here, and one that
        // did is left as it is. `target['ids'][0]` (`:91`) raises on a target
        // without ids, which no directive produces either.
        if !target.children.is_empty() || target.attrs.ids.is_empty() {
            continue;
        }
        let mut next = following(root, &path);
        while let Some(message) = next
            .as_ref()
            .filter(|next| node_at(root, next).map(|n| n.kind) == Some(kinds::SYSTEM_MESSAGE))
        {
            next = following(root, message);
        }
        let Some(next) = next else { continue };
        let Some(receiver) = node_at(root, &next) else {
            continue;
        };
        // A text node cannot carry ids; upstream would fail on it.
        if receiver.kind == kinds::TEXT
            || ((is_invisible(receiver.kind) || is_targetable(receiver.kind))
                && receiver.kind != kinds::TARGET)
        {
            continue;
        }
        let to_caption = parent.kind == "figure" && receiver.kind == "caption";

        let target = node_at_mut(root, &path).expect("the target was just read");
        let ids = std::mem::take(&mut target.attrs.ids);
        let names = std::mem::take(&mut target.attrs.names);
        if to_caption {
            removed.push(path);
        } else {
            target.set("refid", AttrValue::Str(ids[0].clone()));
        }
        let receiver = node_at_mut(root, &next).expect("the receiver was just read");
        receiver.attrs.ids.extend(ids);
        receiver.attrs.names.extend(names);
    }

    // Last first, so each path still names its target.
    for path in removed.iter().rev() {
        if let Some((index, parent)) = path.split_last() {
            if let Some(parent) = node_at_mut(root, parent) {
                parent.children.remove(*index);
            }
        }
    }
}

/// Every `target` below `root`, in document order (pre-order, by an
/// explicit stack rather than recursion).
fn collect_targets(root: &Node) -> Vec<NodePath> {
    let mut targets = Vec::new();
    let mut stack: Vec<(&Node, NodePath)> = vec![(root, Vec::new())];
    while let Some((node, path)) = stack.pop() {
        for (index, child) in node.children.iter().enumerate().rev() {
            let mut child_path = path.clone();
            child_path.push(index);
            stack.push((child, child_path));
        }
        if node.kind == kinds::TARGET {
            targets.push(path);
        }
    }
    targets
}

fn parent_of<'n>(root: &'n Node, path: &[usize]) -> Option<&'n Node> {
    let (_, parent) = path.split_last()?;
    node_at(root, parent)
}

/// `next_node(ascend=True, descend=False)` (`docutils/nodes.py:264-367`):
/// the node's next sibling, else its parent's, and so on up — the node
/// after `path` in document order once its own subtree is skipped. For a
/// target, which has no children, this is `next_node(ascend=True)` too.
fn following(root: &Node, path: &[usize]) -> Option<NodePath> {
    let mut path = path.to_vec();
    while let Some(index) = path.pop() {
        if index + 1 < node_at(root, &path)?.children.len() {
            path.push(index + 1);
            return Some(path);
        }
    }
    None
}

/// `isinstance(node, nodes.TextElement)` by tagname: every docutils and
/// Sphinx (`sphinx/addnodes.py`) element class deriving from it, as probed
/// from the installed docutils 0.22.4 / Sphinx 9.1.0 (research
/// `2026-09-30-m2-wave5-transforms.md` Appendix A).
fn is_text_element(kind: &str) -> bool {
    matches!(
        kind,
        "abbreviation"
            | "acronym"
            | "address"
            | "attribution"
            | "author"
            | "caption"
            | "centered"
            | "citation_reference"
            | "classifier"
            | "comment"
            | "compact_paragraph"
            | "contact"
            | "copyright"
            | "date"
            | "desc_addname"
            | "desc_annotation"
            | "desc_inline"
            | "desc_name"
            | "desc_optional"
            | "desc_parameter"
            | "desc_parameterlist"
            | "desc_returns"
            | "desc_sig_keyword"
            | "desc_sig_keyword_type"
            | "desc_sig_literal_char"
            | "desc_sig_literal_number"
            | "desc_sig_literal_string"
            | "desc_sig_name"
            | "desc_sig_operator"
            | "desc_sig_punctuation"
            | "desc_sig_space"
            | "desc_signature"
            | "desc_signature_line"
            | "desc_type"
            | "desc_type_parameter"
            | "desc_type_parameter_list"
            | "doctest_block"
            | "download_reference"
            | "emphasis"
            | "field_name"
            | "footnote_reference"
            | "generated"
            | "index"
            | "inline"
            | "label"
            | "line"
            | "literal"
            | "literal_block"
            | "literal_emphasis"
            | "literal_strong"
            | "manpage"
            | "math"
            | "math_block"
            | "number_reference"
            | "option_argument"
            | "option_string"
            | "organization"
            | "paragraph"
            | "pending_xref_condition"
            | "problematic"
            | "production"
            | "raw"
            | "reference"
            | "revision"
            | "rubric"
            | "status"
            | "strong"
            | "subscript"
            | "substitution_definition"
            | "substitution_reference"
            | "subtitle"
            | "superscript"
            | "target"
            | "term"
            | "title"
            | "title_reference"
            | "version"
            | "versionmodified"
    )
}

/// `isinstance(node, nodes.Invisible)`: docutils' comment,
/// substitution_definition, pending and target, and Sphinx's `index`
/// (probed as for [`is_text_element`]).
fn is_invisible(kind: &str) -> bool {
    matches!(
        kind,
        kinds::COMMENT | "substitution_definition" | "pending" | kinds::TARGET | "index"
    )
}

/// `isinstance(node, nodes.Targetable)`: footnote, citation and target.
fn is_targetable(kind: &str) -> bool {
    matches!(kind, kinds::FOOTNOTE | kinds::CITATION | kinds::TARGET)
}

#[cfg(test)]
mod tests {
    use crate::doctree::ids::IdRegistry;
    use crate::doctree::{kinds, Doctree, Node, Span};
    use crate::transforms::{apply_read_transforms, TransformConfig};

    fn elem(kind: &'static str, children: Vec<Node>) -> Node {
        let mut node = Node::elem(kind, Span::ZERO);
        node.children = children;
        node
    }

    /// The one structural change PropagateTargets makes (`references.py:
    /// 85-88`): a target that is a `figure`'s child and whose next node is
    /// the figure's `caption` hands its ids and names over and is removed,
    /// instead of staying behind with a `refid`. No directive puts a target
    /// there (a figure holds its image, then caption and legend), so the
    /// tree is built by hand.
    #[test]
    fn a_target_before_a_figure_caption_is_removed() {
        let mut target = Node::elem(kinds::TARGET, Span::ZERO);
        target.attrs.ids = vec!["t".to_string()];
        target.attrs.names = vec!["t".to_string()];
        let caption = elem("caption", vec![Node::text_node("Cap", Span::ZERO)]);
        let figure = elem(
            "figure",
            vec![elem(kinds::IMAGE, Vec::new()), target, caption],
        );
        let mut tree = Doctree {
            root: elem(kinds::DOCUMENT, vec![figure]),
            sources: vec!["<snippet>".to_string()],
        };
        apply_read_transforms(
            &mut tree,
            IdRegistry::new(),
            0,
            "index",
            &TransformConfig::default(),
            &mut Vec::new(),
        );
        let figure = &tree.root.children[0];
        let kinds: Vec<&str> = figure.children.iter().map(|child| child.kind).collect();
        assert_eq!(kinds, [kinds::IMAGE, "caption"], "the target is gone");
        assert_eq!(figure.children[1].attrs.ids, ["t"]);
        assert_eq!(figure.children[1].attrs.names, ["t"]);
    }
}
