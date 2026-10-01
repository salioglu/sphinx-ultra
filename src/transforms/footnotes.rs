//! Footnotes and the read side of citations: Sphinx's citation transforms
//! (CitationDefinitionTransform and CitationReferenceTransform, priority
//! 619, `sphinx/domains/citation.py:133-177`), docutils' Footnotes (620,
//! `docutils/transforms/references.py:416-635`), Sphinx's
//! UnreferencedFootnotesDetector (622, `sphinx/transforms/__init__.py:
//! 288-324`) and FootnoteDocnameUpdater (700, `sphinx/builders/latex/
//! transforms.py:34-43`, which `setup` registers for every builder, `:646`).
//!
//! The citation transforms run first: by the time Footnotes links notes to
//! their references, every `citation_reference` has become a `pending_xref`
//! — but docutils' `document.citation_refs` still holds the replaced nodes,
//! and Footnotes still back-links the citations to them
//! ([`TransformCtx::replaced_citation_refs`]). Resolving a `pending_xref`
//! against the citations of the whole project is the write phase's
//! (`CitationDomain.resolve_xref`, `:99-113`) — not here.
//!
//! Footnotes marks what it resolves `resolved`, a Python attribute outside
//! the tree: within the transform a set of paths stands in for it, and the
//! transforms after it see what it leaves — no `refname` on what it
//! resolved, except the one case [`super::references`] recognises by its
//! `refname` beside a `refid` (a labelled reference numbered from an
//! unlabelled footnote).

use std::collections::{BTreeMap, HashSet};

use crate::doctree::{kinds, messages, AttrValue, Node};
use crate::rst::CitationRecord;

use super::references::{collect_paths, location_at, problematic_for, replace_at, str_value};
use super::{for_each_node_mut, node_at, node_at_mut, NodePath, TransformCtx};

/// `CitationDefinitionTransform` (`sphinx/domains/citation.py:133-148`,
/// priority 619, ahead of its reference twin): every `citation`, in
/// document order (`findall`), gets `docname`, is registered with the
/// citation domain (`note_citation`, `:70-82`), and has its first child —
/// its `label` — marked `support_smartquotes = False`.
///
/// The registration is the domain's: `citations[label] = (docname,
/// ids[0], line)`, warning `duplicate citation %s, other instance in %s`
/// (`type='ref', subtype='citation'`, at the citation) when the label is
/// already registered, by this document or any other. Whether it is
/// depends on the environment, so the call is recorded
/// ([`CitationRecord`], with the `seq` it spends here — the warning
/// prints among this pass's records, after Substitutions' and the
/// hyperlink transforms', before Footnotes') and the merge phase replays it
/// ([`crate::env::citation_domain`]). A citation without an id (upstream's
/// `node['ids'][0]` raises) registers nothing; the parse gives every one
/// an id.
pub(super) fn citation_definitions(ctx: &mut TransformCtx) {
    let root = &mut ctx.tree.root;
    for path in collect_paths(root, |node| node.kind == kinds::CITATION) {
        let Some(citation) = node_at_mut(root, &path) else {
            continue;
        };
        citation.set("docname", AttrValue::Str(ctx.docname.to_string()));
        if let Some(node_id) = citation.attrs.ids.first() {
            ctx.citations.push(CitationRecord {
                label: citation
                    .children
                    .first()
                    .map(Node::astext)
                    .unwrap_or_default(),
                node_id: node_id.clone(),
                source: citation.span.source,
                line: citation.span.line,
                seq: ctx.reporter.next_seq(),
            });
        }
        if let Some(label) = citation.children.first_mut() {
            label.set("support_smartquotes", AttrValue::Int(0));
        }
    }
}

/// `CitationReferenceTransform` (`sphinx/domains/citation.py:150-177`,
/// priority 619): every `citation_reference` (`[CIT]_`), in document
/// order, is replaced by `pending_xref(refdomain='citation', reftype='ref',
/// reftarget=<its text>, refwarn=True, support_smartquotes=False)` with its
/// `ids` and `classes`, holding `inline('[<text>]')` — the reference
/// CitationDomain resolves when the document is written. `reftarget` is
/// the label as written: `[cit]_` targets `cit`, which the domain does not
/// match with `.. [CIT]` (its registries key the label as written), though
/// Footnotes back-links the two through the normalized name both carry.
///
/// `note_citation_reference` (`:84-86`), the domain's record of which
/// documents reference a label, is read off the finished doctree's
/// `pending_xref`s by the merge phase ([`crate::env::citation_domain`]):
/// every one this transform makes stays in the tree, and the record keeps
/// no order.
///
/// The replaced references stay in docutils' `document.citation_refs`,
/// where Footnotes (620) back-links each citation to the ones still
/// carrying their `refname` (unresolved: IndirectHyperlinks, 460, takes it
/// away from those it resolves) — [`TransformCtx::replaced_citation_refs`].
pub(super) fn citation_references(ctx: &mut TransformCtx) {
    let root = &mut ctx.tree.root;
    for path in collect_paths(root, |node| node.kind == kinds::CITATION_REFERENCE) {
        let Some(reference) = node_at(root, &path) else {
            continue;
        };
        let target = reference.astext();
        if let (Some(refname), Some(id)) =
            (str_value(reference, "refname"), reference.attrs.ids.first())
        {
            ctx.replaced_citation_refs
                .entry(refname.to_string())
                .or_default()
                .push(id.clone());
        }
        let span = reference.span;
        let mut xref = Node::elem(kinds::PENDING_XREF, span);
        xref.attrs.ids = reference.attrs.ids.clone();
        xref.attrs.classes = reference.attrs.classes.clone();
        xref.set("refdomain", AttrValue::Str("citation".to_string()));
        xref.set("reftype", AttrValue::Str("ref".to_string()));
        xref.set("reftarget", AttrValue::Str(target.clone()));
        xref.set("refwarn", AttrValue::Int(1));
        xref.set("support_smartquotes", AttrValue::Int(0));
        let mut inline = Node::elem("inline", span);
        inline
            .children
            .push(Node::text_node(format!("[{target}]"), span));
        xref.children.push(inline);
        replace_at(root, &path, xref);
    }
}

/// `Footnotes.symbols` (`docutils/transforms/references.py:482-497`): the
/// symbol footnotes' labels, each repeated once more on every pass.
const SYMBOLS: [&str; 10] = [
    "*", "\u{2020}", "\u{2021}", "\u{a7}", "\u{b6}", "#", "\u{2660}", "\u{2665}", "\u{2666}",
    "\u{2663}",
];

/// `Footnotes` (`docutils/transforms/references.py:416-635`, priority 620):
/// numbers the auto-numbered footnotes, labels the symbol ones, and links
/// every footnote and citation to its references and back, over
/// `document`'s lists — the auto-numbered (`[#]`, `[#label]`), symbol
/// (`[*]`) and manual footnotes and their references, each in document
/// order, and the citations with the references 619 replaced. In
/// `apply`'s order (`:499-505`):
///
/// 1. `number_footnotes` (`:507-534`): each auto-numbered footnote takes
///    the next number, from 1, that is not already a name
///    (`document.nameids`: a manual `[1]` makes the first auto one `2`) as
///    its label. Every reference to one of its names (a labelled
///    `[#label]_`, or a manual `[N]_` naming a labelled `[#N]`) takes the
///    number as its text, loses its `refname` and points at the footnote,
///    which links back. An unnamed footnote is named by its number
///    (`note_explicit_target` — registered in the continued id registry,
///    where SphinxDanglingReferences finds a manual `[N]_` naming it) and
///    the number joins the list unlabelled references draw from; one whose
///    name was duplicated away (`dupnames`) does neither.
/// 2. `number_footnote_references` (`:536-569`): each auto-numbered
///    reference not resolved (nor pointing anywhere) takes the next of
///    those numbers, its footnote linking back. A labelled reference that
///    no footnote carries the label of is among them: it keeps its
///    `refname` (`tx_footnotes.unmatched_label_takes_a_number`). When the
///    numbers run out: `Too many autonumbered footnote references: only %d
///    corresponding footnote%s available.` (ERROR, at that reference, `s`
///    only for more than one), the message spends an id, and every
///    reference from the position the count of numbers handed out gives
///    on that is neither resolved nor carries a `refname` becomes a
///    `problematic` pointing at it, with the next id.
/// 3. `symbolize_footnotes` (`:571-606`): the symbol footnotes take `*`,
///    `†`, `‡`, `§`, `¶`, `#`, `♠`, `♥`, `♦`, `♣`, then each doubled, and so
///    on; the symbol references take them in order. When they run out:
///    `Too many symbol footnote references: only %s corresponding
///    footnotes available.` and the same `problematic`s.
/// 4. `resolve_footnotes_and_citations` (`:608-635`): each manual footnote
///    and each citation is linked with the unresolved references to each
///    of its names.
///
/// The labels are inserted last (in reverse document order): every step
/// before addresses nodes by the paths the transform found them at, and a
/// label inserted into a footnote would move the references inside it.
/// The other changes keep the tree's shape — a `problematic` replaces its
/// reference in place, a reference's text goes after its last child.
///
/// Not ported: a reference an earlier transform or the parse took out of
/// the tree stays in docutils' lists and is numbered or linked there (a
/// footnote reference in a refused substitution definition, `References
/// to auto-numbered and auto-symbol footnotes are not supported in a
/// substitution definition.`, still takes a number and a backref); the
/// walk-built lists hold only what is in the tree.
pub(super) fn footnotes(ctx: &mut TransformCtx) {
    let lists = ctx.lists();
    if lists.autofootnotes.is_empty()
        && lists.autofootnote_refs.is_empty()
        && lists.symbol_footnotes.is_empty()
        && lists.symbol_footnote_refs.is_empty()
        && lists.footnotes.is_empty()
        && lists.citations.is_empty()
    {
        return;
    }
    let mut pass = FootnotePass {
        autofootnotes: lists.autofootnotes.clone(),
        autofootnote_refs: lists.autofootnote_refs.clone(),
        symbol_footnotes: lists.symbol_footnotes.clone(),
        symbol_footnote_refs: lists.symbol_footnote_refs.clone(),
        footnotes: lists.footnotes.clone(),
        footnote_refs: lists.footnote_refs.clone(),
        citations: lists.citations.clone(),
        resolved: HashSet::new(),
        labels: Vec::new(),
    };
    let autofootnote_labels = pass.number_footnotes(ctx);
    pass.number_footnote_references(ctx, &autofootnote_labels);
    pass.symbolize_footnotes(ctx);
    pass.resolve_footnotes_and_citations(ctx);
    pass.insert_labels(ctx);
}

/// Footnotes' state: `document`'s lists as the transform found them, the
/// references it has resolved (`ref.resolved`), and the labels it inserts
/// once everything else is done.
struct FootnotePass {
    autofootnotes: Vec<NodePath>,
    autofootnote_refs: Vec<NodePath>,
    symbol_footnotes: Vec<NodePath>,
    symbol_footnote_refs: Vec<NodePath>,
    footnotes: Vec<NodePath>,
    footnote_refs: BTreeMap<String, Vec<NodePath>>,
    citations: Vec<NodePath>,
    resolved: HashSet<NodePath>,
    labels: Vec<(NodePath, String)>,
}

/// An auto-numbered footnote's number, handed to an unlabelled reference:
/// `(number, the footnote, its id)` — the `nameids[label]` and
/// `ids[id]` lookups upstream makes (`references.py:562-563`).
type AutoLabel = (String, NodePath, String);

impl FootnotePass {
    /// `ref.resolved`: resolved by this transform, or before it — an
    /// earlier transform that resolves a reference (IndirectHyperlinks,
    /// 460) gives it a `refuri` or `refid` (and takes its `refname`).
    fn is_resolved(&self, root: &Node, path: &NodePath) -> bool {
        self.resolved.contains(path)
            || node_at(root, path)
                .is_some_and(|node| node.get("refid").is_some() || node.get("refuri").is_some())
    }

    /// `number_footnotes` (`references.py:507-534`); returns
    /// `autofootnote_labels`.
    fn number_footnotes(&mut self, ctx: &mut TransformCtx) -> Vec<AutoLabel> {
        let mut autofootnote_labels = Vec::new();
        // `document.autofootnote_start` (`nodes.py:1783`): one document,
        // one Footnotes run.
        let mut startnum: u64 = 1;
        for path in &self.autofootnotes {
            // Ends: the names are finite.
            let label = loop {
                let label = startnum.to_string();
                startnum += 1;
                if ctx.ids.name_id(&label).is_none() {
                    break label;
                }
            };
            self.labels.push((path.clone(), label.clone()));
            let root = &mut ctx.tree.root;
            let Some(footnote) = node_at(root, path) else {
                continue;
            };
            // `footnote['ids'][0]`: upstream asserts exactly one id.
            let Some(footnote_id) = footnote.attrs.ids.first().cloned() else {
                continue;
            };
            let names = footnote.attrs.names.clone();
            let unnamed = names.is_empty() && footnote.attrs.dupnames.is_empty();
            let mut backrefs = Vec::new();
            for name in &names {
                for reference in self.footnote_refs.get(name).into_iter().flatten() {
                    if let Some(id) = number_reference(root, reference, &label, &footnote_id) {
                        backrefs.push(id);
                    }
                    if let Some(node) = node_at_mut(root, reference) {
                        node.remove("refname");
                    }
                    self.resolved.insert(reference.clone());
                }
            }
            let Some(footnote) = node_at_mut(root, path) else {
                continue;
            };
            footnote.attrs.backrefs.extend(backrefs);
            if unnamed {
                footnote.attrs.names.push(label.clone());
                ctx.ids.note_explicit_name(&label, &footnote_id);
                autofootnote_labels.push((label, path.clone(), footnote_id));
            }
        }
        autofootnote_labels
    }

    /// `number_footnote_references` (`references.py:536-569`).
    fn number_footnote_references(&mut self, ctx: &mut TransformCtx, labels: &[AutoLabel]) {
        let mut i = 0;
        for path in &self.autofootnote_refs {
            if self.is_resolved(&ctx.tree.root, path) {
                continue;
            }
            let Some((label, footnote, footnote_id)) = labels.get(i) else {
                let n = labels.len();
                let s = if n > 1 { "s" } else { "" };
                let text = format!(
                    "Too many autonumbered footnote references: only {n} corresponding \
                     footnote{s} available."
                );
                let references = &self.autofootnote_refs;
                self.overflow(ctx, references, path, &text, i, |node| {
                    node.get("refname").is_some()
                });
                break;
            };
            let root = &mut ctx.tree.root;
            if let Some(id) = number_reference(root, path, label, footnote_id) {
                if let Some(footnote) = node_at_mut(root, footnote) {
                    footnote.attrs.backrefs.push(id);
                }
            }
            self.resolved.insert(path.clone());
            i += 1;
        }
    }

    /// `symbolize_footnotes` (`references.py:571-606`). Each symbol
    /// footnote already has its id (`note_symbol_footnote`, `nodes.py:
    /// 2031-2033`), so `set_id` makes none.
    fn symbolize_footnotes(&mut self, ctx: &mut TransformCtx) {
        let mut labels = Vec::with_capacity(self.symbol_footnotes.len());
        for (start, path) in self.symbol_footnotes.iter().enumerate() {
            let (reps, index) = (start / SYMBOLS.len(), start % SYMBOLS.len());
            let label = SYMBOLS[index].repeat(reps + 1);
            labels.push(label.clone());
            self.labels.push((path.clone(), label));
        }
        for (i, path) in self.symbol_footnote_refs.iter().enumerate() {
            let Some(label) = labels.get(i) else {
                let text = format!(
                    "Too many symbol footnote references: only {} corresponding footnotes \
                     available.",
                    labels.len()
                );
                let references = &self.symbol_footnote_refs;
                self.overflow(ctx, references, path, &text, i, |node| {
                    node.get("refid").is_some()
                });
                break;
            };
            let footnote = &self.symbol_footnotes[i];
            let root = &mut ctx.tree.root;
            let Some(footnote_id) = node_at(root, footnote).and_then(|n| n.attrs.ids.first())
            else {
                continue;
            };
            let footnote_id = footnote_id.clone();
            if let Some(id) = number_reference(root, path, label, &footnote_id) {
                if let Some(footnote) = node_at_mut(root, footnote) {
                    footnote.attrs.backrefs.push(id);
                }
            }
        }
    }

    /// The overflow branch of `number_footnote_references` and
    /// `symbolize_footnotes` (`references.py:544-560,586-600`): the ERROR
    /// at the reference that found no label (`base_node=ref`), its id, and a
    /// `problematic` pointing at it — with the next id, and the reference's
    /// own ids after (`replace_self`) — for every reference of
    /// `references` from position `from` on (upstream's index, the count of
    /// labels handed out) that is not resolved and is not `skipped`.
    fn overflow(
        &self,
        ctx: &mut TransformCtx,
        references: &[NodePath],
        at: &NodePath,
        text: &str,
        from: usize,
        skipped: impl Fn(&Node) -> bool,
    ) {
        let (source, line) = location_at(&ctx.tree.root, at);
        let message = ctx.message(messages::ERROR, text, source, Some(line));
        ctx.reporter.report(&message);
        let message_id = ctx.ids.allocate_auto_id();
        for path in references.iter().skip(from) {
            let root = &ctx.tree.root;
            let Some(reference) = node_at(root, path) else {
                continue;
            };
            if self.is_resolved(root, path) || skipped(reference) {
                continue;
            }
            let mut problematic = problematic_for(reference, message_id.clone());
            problematic.attrs.ids.push(ctx.ids.allocate_auto_id());
            replace_at(&mut ctx.tree.root, path, problematic);
        }
    }

    /// `resolve_footnotes_and_citations` (`references.py:608-622`) and
    /// `resolve_references` (`:624-635`): a manual footnote's references,
    /// by each of its names, not yet resolved, lose their `refname` and
    /// point at it, which links back; a citation links back to the
    /// unresolved references 619 replaced (which no longer are in the tree
    /// to point anywhere).
    fn resolve_footnotes_and_citations(&mut self, ctx: &mut TransformCtx) {
        let root = &mut ctx.tree.root;
        for path in &self.footnotes {
            let Some(footnote) = node_at(root, path) else {
                continue;
            };
            let Some(footnote_id) = footnote.attrs.ids.first().cloned() else {
                continue;
            };
            let mut backrefs = Vec::new();
            for name in footnote.attrs.names.clone() {
                for reference in self.footnote_refs.get(&name).into_iter().flatten() {
                    if self.is_resolved(root, reference) {
                        continue;
                    }
                    let Some(node) = node_at_mut(root, reference) else {
                        continue;
                    };
                    node.remove("refname");
                    node.set("refid", AttrValue::Str(footnote_id.clone()));
                    backrefs.extend(node.attrs.ids.first().cloned());
                    self.resolved.insert(reference.clone());
                }
            }
            if let Some(footnote) = node_at_mut(root, path) {
                footnote.attrs.backrefs.extend(backrefs);
            }
        }
        for path in &self.citations {
            let Some(citation) = node_at_mut(root, path) else {
                continue;
            };
            let mut backrefs = Vec::new();
            for name in &citation.attrs.names {
                if let Some(ids) = ctx.replaced_citation_refs.get(name) {
                    backrefs.extend(ids.iter().cloned());
                }
            }
            citation.attrs.backrefs.extend(backrefs);
        }
    }

    /// `footnote.insert(0, nodes.label('', label))` (`references.py:520,
    /// 579`) for every footnote numbered or symbolized, last in document
    /// order first: an insertion moves only the nodes inside its footnote,
    /// and every footnote still to be labelled comes before it in the
    /// document or encloses it.
    fn insert_labels(&mut self, ctx: &mut TransformCtx) {
        self.labels.sort_by(|(a, _), (b, _)| b.cmp(a));
        for (path, text) in self.labels.drain(..) {
            if let Some(footnote) = node_at_mut(&mut ctx.tree.root, &path) {
                let span = footnote.span;
                let mut label = Node::elem(kinds::LABEL, span);
                label.children.push(Node::text_node(text, span));
                footnote.children.insert(0, label);
            }
        }
    }
}

/// A reference numbered or symbolized (`references.py:523-529,561-568,585,
/// 602-604`): `ref += Text(label)` and `refid` set to the footnote's id
/// (`number_footnotes` drops its `refname` too, the other two leave it).
/// Returns the reference's first id, for the footnote's backrefs.
fn number_reference(
    root: &mut Node,
    path: &NodePath,
    label: &str,
    footnote_id: &str,
) -> Option<String> {
    let reference = node_at_mut(root, path)?;
    let span = reference.span;
    reference.children.push(Node::text_node(label, span));
    reference.set("refid", AttrValue::Str(footnote_id.to_string()));
    reference.attrs.ids.first().cloned()
}

/// `UnreferencedFootnotesDetector` (`sphinx/transforms/__init__.py:
/// 288-324`, priority 622): Sphinx logs (`type='ref', subtype='footnote'`,
/// at the footnote) every footnote without a backref — the manual ones
/// first, named (`Footnote [%s] is not referenced.` with the first name;
/// one whose name was duplicated away is skipped), then the symbol ones
/// (`Footnote [*] is not referenced.`), then the auto-numbered ones that
/// have a name (`Footnote [#] is not referenced.`, labelled or numbered),
/// each kind in document order.
pub(super) fn unreferenced_footnotes(ctx: &mut TransformCtx) {
    let (tree, lists) = ctx.tree_and_lists();
    let mut unreferenced: Vec<(String, (u16, u32))> = Vec::new();
    let footnotes = |paths: &[NodePath]| {
        paths
            .iter()
            .filter_map(|path| node_at(&tree.root, path))
            .filter(|node| node.attrs.backrefs.is_empty())
            .collect::<Vec<_>>()
    };
    let at = |node: &Node| (node.span.source, node.span.line);
    for node in footnotes(&lists.footnotes) {
        if let Some(name) = node.attrs.names.first() {
            unreferenced.push((format!("Footnote [{name}] is not referenced."), at(node)));
        }
    }
    for node in footnotes(&lists.symbol_footnotes) {
        unreferenced.push(("Footnote [*] is not referenced.".to_string(), at(node)));
    }
    for node in footnotes(&lists.autofootnotes) {
        if !node.attrs.names.is_empty() {
            unreferenced.push(("Footnote [#] is not referenced.".to_string(), at(node)));
        }
    }
    for (text, (source, line)) in unreferenced {
        ctx.reporter.log(
            messages::WARNING,
            Some("ref.footnote".to_string()),
            text,
            source,
            Some(line),
            false,
        );
    }
}

/// `FootnoteDocnameUpdater` (`sphinx/builders/latex/transforms.py:34-43`,
/// priority 700): every `footnote` and `footnote_reference` gets the
/// document's `docname`.
pub(super) fn footnote_docnames(ctx: &mut TransformCtx) {
    let docname = ctx.docname;
    for_each_node_mut(&mut ctx.tree.root, |node| {
        if matches!(node.kind, kinds::FOOTNOTE | kinds::FOOTNOTE_REFERENCE) {
            node.set("docname", AttrValue::Str(docname.to_string()));
        }
    });
}
