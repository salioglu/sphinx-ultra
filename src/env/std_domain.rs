//! The `std` domain: cross-reference labels, glossary terms, program
//! options and generic objects — Sphinx's `StandardDomain`
//! (`domains/std/__init__.py`).
//!
//! This module owns the *collection* half (Sphinx's `process_doc`,
//! `note_object`, `_note_term`, `add_program_option`); the *resolution*
//! half lives in [`crate::env::resolve`].
//!
//! See `docs/superpowers/plans/2026-08-31-m2-wave4-research-spec-sphinx-env-toctree-domains.md`
//! §4 for the attribute-by-attribute mapping this port is drawn from.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::doctree::{kinds, AttrValue, Doctree, Node};
use crate::env::numbers::{clean_astext, std_numfig_title};
use crate::env::BuildEnvironment;
use crate::error::{BuildWarning, WarningType};
use crate::rst::RegistryExport;

/// Standard-domain (`std`) registries: cross-reference labels, generic
/// objects (`:option:`, `:envvar:`, ...), program options, and glossary
/// terms. Field shapes mirror Sphinx's `StandardDomain.data` exactly
/// (`domains/std/__init__.py:768-781`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StdDomainData {
    /// labelname -> (docname, labelid, sectionname).
    pub labels: BTreeMap<String, (String, String, String)>,
    /// labelname -> (docname, labelid).
    pub anonlabels: BTreeMap<String, (String, String)>,
    /// (objtype, name) -> (docname, labelid).
    pub objects: BTreeMap<(String, String), (String, String)>,
    /// (program, optname) -> (docname, labelid).
    pub progoptions: BTreeMap<(Option<String>, String), (String, String)>,
    /// lowercased term -> (docname, labelid).
    pub terms: BTreeMap<String, (String, String)>,
}

/// The three virtual pages Sphinx's `initial_data` preseeds
/// (`domains/std/__init__.py:768-781`), plus `py-modindex`, which the
/// *python* domain adds on top through `Domain.setup`
/// (`domains/__init__.py:136-142`: every domain index gets a hyperlink
/// target named `{domain}-{index}`). All four are part of the oracle
/// contract — a `:ref:` to any of them resolves in an otherwise empty
/// project.
const PRESEEDED_LABELS: &[(&str, &str, &str)] = &[
    ("genindex", "genindex", "Index"),
    ("modindex", "py-modindex", "Module Index"),
    ("py-modindex", "py-modindex", "Python Module Index"),
    ("search", "search", "Search Page"),
];

impl Default for StdDomainData {
    fn default() -> Self {
        let mut labels = BTreeMap::new();
        let mut anonlabels = BTreeMap::new();
        for (name, docname, title) in PRESEEDED_LABELS {
            labels.insert(
                (*name).to_string(),
                ((*docname).to_string(), String::new(), (*title).to_string()),
            );
            anonlabels.insert((*name).to_string(), ((*docname).to_string(), String::new()));
        }
        Self {
            labels,
            anonlabels,
            objects: BTreeMap::new(),
            progoptions: BTreeMap::new(),
            terms: BTreeMap::new(),
        }
    }
}

impl StdDomainData {
    /// `StandardDomain.note_object` (`:848-864`). Returns the docname of a
    /// previous description of the same object, which the caller reports as
    /// warning [ENV §8 #2] — note Sphinx names the *docname* here, not
    /// `doc2path`, and offers no `:no-index:` hint (unlike the py domain).
    pub fn note_object(
        &mut self,
        objtype: &str,
        name: &str,
        docname: &str,
        labelid: &str,
    ) -> Option<String> {
        let key = (objtype.to_string(), name.to_string());
        let previous = self.objects.get(&key).map(|(doc, _)| doc.clone());
        self.objects
            .insert(key, (docname.to_string(), labelid.to_string()));
        previous
    }

    /// `StandardDomain._note_term` (`:871-878`): a glossary term is an
    /// object *and* a lowercased entry in `terms`, which is what makes
    /// `:term:` resolution case-insensitive.
    pub fn note_term(&mut self, term: &str, docname: &str, labelid: &str) -> Option<String> {
        let previous = self.note_object("term", term, docname, labelid);
        self.terms.insert(
            term.to_lowercase(),
            (docname.to_string(), labelid.to_string()),
        );
        previous
    }

    /// `StandardDomain.add_program_option` (`:995-1000`) — **first entry
    /// wins**, unlike every other registry here.
    pub fn add_program_option(
        &mut self,
        program: Option<&str>,
        name: &str,
        docname: &str,
        labelid: &str,
    ) {
        self.progoptions
            .entry((program.map(str::to_string), name.to_string()))
            .or_insert_with(|| (docname.to_string(), labelid.to_string()));
    }
}

/// One document's parse output, as [`process_doc`] consumes it.
pub struct DocumentSource<'a> {
    pub docname: &'a str,
    pub doctree: &'a Doctree,
    /// docutils `document.nameids`/`nametypes`, harvested at the end of the
    /// parse.
    pub registry: &'a RegistryExport,
    /// The document's rST source: docutils node lines are derived from it
    /// (a [`crate::doctree::Span`] is a byte range, not a line).
    pub text: &'a str,
    pub path: &'a Path,
}

/// `StandardDomain.process_doc` (`domains/std/__init__.py:937-993`) plus the
/// registrations Sphinx performs from directives at parse time, which our
/// parse layer has no domain callbacks to run: glossary terms
/// (`make_glossary_term`) are replayed from the finished doctree, and the
/// `option`/`envvar`/`confval` registrations
/// (`ObjectDescription.add_target_and_index`) from the records the parse
/// layer kept — see [`RegistryExport::program_options`] for why the doctree
/// cannot carry those.
///
/// `doc2path` renders another document's source path for the duplicate-label
/// warning [ENV §8 #1], which names the *path*, not the docname.
pub fn process_doc(
    env: &mut BuildEnvironment,
    doc: &DocumentSource<'_>,
    doc2path: &dyn Fn(&str) -> PathBuf,
    warnings: &mut Vec<BuildWarning>,
) {
    let ids = DocumentIds::of(doc.doctree);
    // Order matters, and it is Sphinx's. Glossary terms and object
    // descriptions register *during the parse* (`make_glossary_term` ->
    // `_note_term`, `ObjectDescription.add_target_and_index` ->
    // `note_object`), while `StandardDomain.process_doc`'s label pass runs
    // only once the parse has finished. So Sphinx's duplicate-term and
    // duplicate-object warnings always precede the same document's
    // duplicate-label warnings, and come out interleaved with each other in
    // document order. This crate has no domain callbacks in the parse, so
    // both registration passes run here: collected together, put back into
    // source order, and emitted ahead of the label pass.
    //
    // Still not Sphinx: these warnings interleave with the document's
    // *parse* warnings there, where the builder emits the whole parse
    // stream before calling this. That is the cross-category ordering the
    // ledger defers to a later wave.
    let mut parse_time = Vec::new();
    collect_glossary_terms(env, doc, &mut parse_time);
    collect_descriptions(env, doc, &mut parse_time);
    parse_time.sort_by_key(|warning| warning.line);
    warnings.append(&mut parse_time);
    collect_labels(env, doc, &ids, doc2path, warnings);
}

/// The label half of `process_doc` (`:938-993`).
fn collect_labels(
    env: &mut BuildEnvironment,
    doc: &DocumentSource<'_>,
    ids: &DocumentIds<'_>,
    doc2path: &dyn Fn(&str) -> PathBuf,
    warnings: &mut Vec<BuildWarning>,
) {
    // Sphinx iterates `document.nametypes` — a dict, so in the order names
    // were registered, which is document order. Our registry is a hash map,
    // so the order is recovered from where each name's node sits in the
    // document instead (same sequence, and deterministic either way).
    let mut named: Vec<(&str, &str, usize, &Node)> = Vec::new();
    for (name, labelid, explicit) in &doc.registry.nameids {
        // `if not explicit: continue` / `if labelid is None: continue`.
        if !explicit {
            continue;
        }
        let Some(labelid) = labelid else { continue };
        // Sphinx indexes `document.ids[labelid]` unconditionally; a name
        // whose id names no node in the tree can only mean our parse layer
        // registered an id it never stamped onto a node, so skip rather
        // than crash.
        let Some((order, node)) = ids.get(labelid) else {
            continue;
        };
        named.push((name, labelid, order, node));
    }
    named.sort_by_key(|(name, _, order, _)| (*order, *name));

    for (name, labelid, _, node) in named {
        // "ignore footnote labels, labels automatically generated from a
        // link and object descriptions" (`:951-958`).
        if node.kind == kinds::FOOTNOTE
            || node.get("refuri").is_some()
            || node.kind.starts_with("desc_")
        {
            continue;
        }
        if let Some((other, _, _)) = env.std.labels.get(name) {
            warnings.push(
                BuildWarning::new(
                    doc.path.to_path_buf(),
                    Some(node_line(node, doc.text)),
                    format!(
                        "duplicate label {name}, other instance in {}",
                        doc2path(other).display()
                    ),
                    WarningType::DuplicateLabel,
                )
                // `logger.warning(...)` with no `type=`/`subtype=`: this one
                // carries no `[type.subtype]` suffix.
                .with_category(None),
            );
        }
        env.std.anonlabels.insert(
            name.to_string(),
            (doc.docname.to_string(), labelid.to_string()),
        );

        let Some(sectname) = section_name(node) else {
            // "anonymous-only labels": an anonlabel, but nothing `:ref:`
            // can title itself from.
            continue;
        };
        env.std.labels.insert(
            name.to_string(),
            (doc.docname.to_string(), labelid.to_string(), sectname),
        );
    }
}

/// The `sectname` ladder of `process_doc` (`:967-992`). `None` is Sphinx's
/// `continue` — the label stays anonymous-only.
fn section_name(node: &Node) -> Option<String> {
    if node.kind == kinds::SECTION {
        // `title = node[0]` — Sphinx indexes blindly; a section always has
        // its title first.
        return Some(clean_astext(node.children.first()?));
    }
    if node.kind == "rubric" {
        return Some(clean_astext(node));
    }
    if is_enumerable_node(node) {
        let title = numfig_title(node).unwrap_or_default();
        // "if not sectname: continue" — an uncaptioned figure/table/code
        // block is not titled by its label.
        return (!title.is_empty()).then_some(title);
    }

    let mut node = node;
    if matches!(node.kind, kinds::DEFINITION_LIST | kinds::FIELD_LIST) && !node.children.is_empty()
    {
        node = &node.children[0];
    }
    if matches!(node.kind, kinds::FIELD | kinds::DEFINITION_LIST_ITEM) {
        node = node.children.first()?;
    }
    if matches!(node.kind, kinds::TERM | kinds::FIELD_NAME) {
        return Some(clean_astext(node));
    }
    // `next(node.findall(addnodes.toctree), None)` with a caption —
    // `if toctree and toctree.get('caption')`, so a captionless toctree
    // leaves the label anonymous-only.
    let toctree = find_first(node, kinds::TOCTREE)?;
    match toctree.get("caption") {
        Some(AttrValue::Str(caption)) if !caption.is_empty() && !is_none_sentinel(caption) => {
            Some(caption.clone())
        }
        _ => None,
    }
}

/// docutils renders a `None` attribute value as the string `True`
/// (`nodes.Element.starttag`: a value of `None` prints as `name="True"`),
/// and our parse layer stores that rendering directly — `toctree[caption]`,
/// `math_block[label]`/`[number]`, `pending_xref[py:class]`/`[py:module]`
/// and `pending_xref[std:program]`. A consumer testing such an attribute for
/// Python truthiness has to treat the sentinel as absent, or a captionless
/// toctree ends up named "True" and every `:option:` written outside a
/// `.. program::` looks scoped to a program called "True".
///
/// The other attributes read across this crate are not affected: `refuri`,
/// `refid` and `refname` are presence-tested on `target` nodes, which never
/// carry the sentinel (our parser sets them only to real values), and the
/// `desc` attributes are string-valued by construction. Any *new* read of a
/// possibly-`None` attribute belongs behind this test.
///
/// The encoding cannot tell a missing value from the literal string
/// `"True"`, so `:caption: True` is read as no caption. Fixing that means
/// giving the parse layer an optional attribute value rather than the
/// rendered sentinel — a doctree-wide change, not one this module can make.
pub(crate) fn is_none_sentinel(value: &str) -> bool {
    value == "True"
}

/// `StandardDomain.is_enumerable_node` over `enumerable_nodes` (`:798-803`).
fn is_enumerable_node(node: &Node) -> bool {
    matches!(node.kind, "figure" | kinds::TABLE | "container")
}

/// `StandardDomain.get_numfig_title` (`:1366-1378`).
fn numfig_title(node: &Node) -> Option<String> {
    is_enumerable_node(node)
        .then(|| std_numfig_title(node).map(clean_astext))
        .flatten()
}

/// Glossary terms: `make_glossary_term` (`domains/std/__init__.py:375-407`)
/// calls `_note_term(term.astext(), node_id)` while the directive runs. Our
/// parse layer emits the finished `glossary`/`definition_list` anatomy
/// without calling back into a domain, so the registration is replayed from
/// the tree: every `term` carrying an id inside a `definition_list` classed
/// `glossary`.
fn collect_glossary_terms(
    env: &mut BuildEnvironment,
    doc: &DocumentSource<'_>,
    warnings: &mut Vec<BuildWarning>,
) {
    let mut terms: Vec<&Node> = Vec::new();
    collect_glossary_term_nodes(&doc.doctree.root, &mut terms);
    for term in terms {
        let Some(node_id) = term.attrs.ids.first() else {
            continue;
        };
        // `termtext = term.astext()` is taken before the index node is
        // appended; an `index` node contributes no text either way.
        let text = term.astext();
        if let Some(other) = env.std.note_term(&text, doc.docname, node_id) {
            warnings.push(duplicate_object_warning(
                doc,
                glossary_term_line(term, doc.text),
                "term",
                &text,
                &other,
            ));
        }
    }
}

fn collect_glossary_term_nodes<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
    if node.kind == kinds::DEFINITION_LIST
        && node.attrs.classes.iter().any(|class| class == "glossary")
    {
        for item in &node.children {
            for child in &item.children {
                if child.kind == kinds::TERM {
                    out.push(child);
                }
            }
        }
        return;
    }
    for child in &node.children {
        collect_glossary_term_nodes(child, out);
    }
}

/// Object descriptions: Sphinx registers these from inside the directive
/// (`ObjectDescription.add_target_and_index` → `note_object` /
/// `add_program_option`, `domains/std/__init__.py:226-330`), against state
/// the finished doctree does not carry — see
/// [`RegistryExport::program_options`] — so the parse layer records the
/// calls and this replays them.
///
/// `describe`/`object` produce no records at all: the base
/// `add_target_and_index` is a no-op, so they contribute neither an object
/// nor an id.
fn collect_descriptions(
    env: &mut BuildEnvironment,
    doc: &DocumentSource<'_>,
    warnings: &mut Vec<BuildWarning>,
) {
    for record in &doc.registry.program_options {
        env.std.add_program_option(
            record.program.as_deref(),
            &record.name,
            doc.docname,
            &record.node_id,
        );
    }
    for record in &doc.registry.std_objects {
        if let Some(other) =
            env.std
                .note_object(&record.objtype, &record.name, doc.docname, &record.node_id)
        {
            warnings.push(duplicate_object_warning(
                doc,
                record.line as usize,
                &record.objtype,
                &record.name,
                &other,
            ));
        }
    }
}

/// The line Sphinx reports for a glossary term, which is one *less* than
/// the term's own: `make_glossary_term` is handed the linenos of
/// `self.content.items`, and a directive's content items carry docutils'
/// **0-based** line offsets (`content_offset` comes from
/// `abs_line_offset()`), while everything else in a warning location is
/// 1-based. Verified against sphinx 9.1.0: a term on source line 8 reports
/// `b.rst:7`, one on line 11 reports `b.rst:10`.
fn glossary_term_line(term: &Node, text: &str) -> usize {
    node_line(term, text).saturating_sub(1)
}

/// Warning [ENV §8 #2]: `duplicate %s description of %s, other instance in %s`.
fn duplicate_object_warning(
    doc: &DocumentSource<'_>,
    line: usize,
    objtype: &str,
    name: &str,
    other: &str,
) -> BuildWarning {
    BuildWarning::new(
        doc.path.to_path_buf(),
        Some(line),
        format!("duplicate {objtype} description of {name}, other instance in {other}"),
        WarningType::DuplicateLabel,
    )
    .with_category(None)
}

/// docutils `document.ids` *after* the `PropagateTargets` transform
/// (`docutils/transforms/references.py:17-95`), which is the map
/// `process_doc` indexes.
///
/// Our parse layer does not run that transform — a `.. _label:` before a
/// section stays its own `target` node instead of donating its id and name
/// to the section — so the propagation is replayed here, read-only, over
/// the tree we do produce. Everything Sphinx's `document.ids` would point at
/// is therefore reachable by id; only the *serialized* doctree still shows
/// the unpropagated shape (which is why the oracle's `resolved_pformat` for
/// such documents is still exempted).
///
/// Reachability *by id* is not the whole of the transform, though: a
/// consumer that reads `node['ids']` straight off the tree still sees the
/// unpropagated list. [`PropagatedIds`] replays it in that direction, and
/// is what the numbering passes and `get_fignumber` use.
pub(crate) struct DocumentIds<'a> {
    map: HashMap<&'a str, (usize, &'a Node)>,
}

impl<'a> DocumentIds<'a> {
    pub(crate) fn of(doctree: &'a Doctree) -> Self {
        // Document (pre-order) order, exactly the sequence
        // `Node.next_node(ascend=True)` walks.
        let mut flat: Vec<FlatNode<'a>> = Vec::new();
        flatten(&doctree.root, kinds::DOCUMENT, &mut flat);

        let mut map: HashMap<&str, (usize, &Node)> = HashMap::new();
        for (order, entry) in flat.iter().enumerate() {
            for id in &entry.node.attrs.ids {
                map.insert(id.as_str(), (order, entry.node));
            }
        }

        // PropagateTargets, in document order so that chained targets
        // collapse onto the same final node.
        for donation in propagations(&flat) {
            for id in donation.ids {
                map.insert(id.as_str(), (donation.order, donation.receiver));
            }
        }
        Self { map }
    }

    fn get(&self, id: &str) -> Option<(usize, &'a Node)> {
        self.map.get(id).copied()
    }

    /// The node an id names, after propagation.
    pub(crate) fn node(&self, id: &str) -> Option<&'a Node> {
        self.map.get(id).map(|(_, node)| *node)
    }
}

/// One id donation `PropagateTargets` would make: the node that receives
/// the ids, where it sits in the pre-order walk, and the donor target's ids.
struct Donation<'a> {
    order: usize,
    receiver: &'a Node,
    ids: &'a [String],
}

/// Every donation docutils' `PropagateTargets` (`references.py:17-95`)
/// would make over `flat`, in document order — so that chained targets
/// collapse onto the same final node.
fn propagations<'a>(flat: &[FlatNode<'a>]) -> Vec<Donation<'a>> {
    let mut donations = Vec::new();
    for (index, entry) in flat.iter().enumerate() {
        if !is_propagating_target(entry.node, entry.parent) {
            continue;
        }
        let Some((order, receiver)) = next_propagation_target(flat, index) else {
            continue;
        };
        donations.push(Donation {
            order,
            receiver,
            ids: entry.node.attrs.ids.as_slice(),
        });
    }
    donations
}

/// `node['ids']` as docutils leaves it once `PropagateTargets` has run
/// (`references.py:71-72` *extends* the receiving node's list), keyed by
/// node identity inside one doctree.
///
/// [`DocumentIds`] replays the same transform the other way round — id to
/// node — which is what a lookup by label needs. The numbering passes walk
/// the tree instead, and both halves of a figure number key off the node's
/// own id list: `register_fignumber` files the number under
/// `fignode['ids'][0]` (`collectors/toctree.py:320-336`) and
/// `get_fignumber` reads it back with `target_node['ids'][0]`
/// (`domains/std/__init__.py:1395-1422`). Without this, a `.. _label:`
/// written above a figure/table/code-block — the classic docutils spelling,
/// as opposed to the `:name:` option — leaves the enumerable node with an
/// empty `ids`, so it is never numbered and every `:numref:` to it fails.
///
/// Still missing, and out of scope here: Sphinx's `AutoNumbering` transform
/// (`transforms/__init__.py:200-214`), which hands an *implicit* id to a
/// captioned enumerable node carrying no label at all. Such a node is still
/// skipped by the numbering walk, where Sphinx numbers it.
pub(crate) struct PropagatedIds {
    /// Receiving node's address -> the ids donated to it, in donation
    /// order. Addresses are stable for as long as the doctree the map was
    /// built from is borrowed, which is the only window a `PropagatedIds`
    /// is used in.
    donations: HashMap<usize, Vec<String>>,
}

impl PropagatedIds {
    pub(crate) fn of(doctree: &Doctree) -> Self {
        let mut flat: Vec<FlatNode<'_>> = Vec::new();
        flatten(&doctree.root, kinds::DOCUMENT, &mut flat);

        let mut donations: HashMap<usize, Vec<String>> = HashMap::new();
        for donation in propagations(&flat) {
            donations
                .entry(node_identity(donation.receiver))
                .or_default()
                .extend(donation.ids.iter().cloned());
        }
        Self { donations }
    }

    /// The node's own ids first, then every id donated to it — the exact
    /// list `next_node['ids'].extend(target['ids'])` leaves behind.
    pub(crate) fn effective_ids(&self, node: &Node) -> Vec<String> {
        let mut ids = node.attrs.ids.clone();
        if let Some(donated) = self.donations.get(&node_identity(node)) {
            ids.extend(donated.iter().cloned());
        }
        ids
    }
}

fn node_identity(node: &Node) -> usize {
    std::ptr::from_ref(node) as usize
}

/// One node in the pre-order walk, with what it hangs off and where its
/// own subtree ends — the index a `descend=False` step jumps to.
struct FlatNode<'a> {
    node: &'a Node,
    parent: &'static str,
    subtree_end: usize,
}

fn flatten<'a>(node: &'a Node, parent: &'static str, out: &mut Vec<FlatNode<'a>>) {
    let index = out.len();
    out.push(FlatNode {
        node,
        parent,
        // Patched below, once the subtree is laid out.
        subtree_end: 0,
    });
    for child in &node.children {
        flatten(child, node.kind, out);
    }
    out[index].subtree_end = out.len();
}

/// "Only block-level targets without reference (like `.. _target:`)"
/// (`references.py:44-49`). `TextElement` parents mean an inline target;
/// `refid`/`refuri`/`refname` mean the target already points somewhere.
fn is_propagating_target(node: &Node, parent: &'static str) -> bool {
    node.kind == kinds::TARGET
        && !matches!(
            parent,
            kinds::PARAGRAPH | kinds::TITLE | kinds::TERM | kinds::FIELD_NAME | "caption"
        )
        // `assert len(target) == 0` — docutils only ever propagates a
        // childless target, so an inline one (which carries its own text)
        // is excluded whatever its parent element happens to be.
        && node.children.is_empty()
        && node.get("refid").is_none()
        && node.get("refuri").is_none()
        && node.get("refname").is_none()
}

/// The node a target donates its ids to: the next node in document order,
/// skipping `system_message`s, and never an `Invisible`/`Targetable` other
/// than a `target` (`references.py:50-59`).
///
/// The skip is `next_node(ascend=True, descend=False)` — the message's
/// *sibling*, so its whole subtree is jumped over, not its first child. A
/// `system_message` always has children (the problem text), so descending
/// into one would hand the target's ids to a `paragraph` inside a warning
/// and leave the section behind it unlabelled.
fn next_propagation_target<'a>(flat: &[FlatNode<'a>], index: usize) -> Option<(usize, &'a Node)> {
    let mut next = index + 1;
    while let Some(entry) = flat.get(next) {
        if entry.node.kind != kinds::SYSTEM_MESSAGE {
            break;
        }
        next = entry.subtree_end;
    }
    let node = flat.get(next)?.node;
    let blocked = matches!(
        node.kind,
        kinds::COMMENT
            | "substitution_definition"
            | "pending"
            | kinds::FOOTNOTE
            | kinds::CITATION
            | kinds::TEXT
    );
    (!blocked).then_some((next, node))
}

/// The first descendant of `node` with the given kind, in document order.
fn find_first<'a>(node: &'a Node, kind: &str) -> Option<&'a Node> {
    if node.kind == kind {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_first(child, kind))
}

/// The 1-based source line docutils would report for `node` — what
/// `logger.warning(..., location=node)` renders after the colon.
///
/// A node's line is the first line of its source span, with one exception:
/// docutils creates a `section` only once the state machine has consumed
/// the title's *underline*, so a section's line is one past its title
/// (verified against docutils 0.22.4 for both the underline and
/// overline+underline forms).
pub(crate) fn node_line(node: &Node, text: &str) -> usize {
    let line = line_of(text, node.span.start);
    if node.kind == kinds::SECTION {
        line + 1
    } else {
        line
    }
}

/// The 1-based line containing byte `offset`.
pub(crate) fn line_of(text: &str, offset: u32) -> usize {
    let end = (offset as usize).min(text.len());
    1 + text[..end].bytes().filter(|byte| *byte == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst::{parse_rst_full, ParseOptions};

    fn parse(source: &str, docname: &str) -> crate::rst::ParseOutput {
        parse_rst_full(
            source,
            &ParseOptions {
                source_path: format!("<{docname}>"),
                sphinx: true,
                docname: docname.to_string(),
                found_docs: None,
                exclude_patterns: Vec::new(),
            },
        )
    }

    /// Fold one source into a fresh environment and return the warnings.
    fn read(sources: &[(&str, &str)]) -> (BuildEnvironment, Vec<BuildWarning>) {
        let mut env = BuildEnvironment::default();
        let mut warnings = Vec::new();
        let doc2path = |docname: &str| PathBuf::from(format!("/src/{docname}.rst"));
        for (docname, source) in sources {
            let parsed = parse(source, docname);
            let path = PathBuf::from(format!("/src/{docname}.rst"));
            process_doc(
                &mut env,
                &DocumentSource {
                    docname,
                    doctree: &parsed.doctree,
                    registry: &parsed.registry,
                    text: source,
                    path: &path,
                },
                &doc2path,
                &mut warnings,
            );
        }
        (env, warnings)
    }

    /// Sphinx registers glossary terms and object descriptions during the
    /// parse and runs the label pass afterwards, so a document carrying all
    /// three kinds of duplicate reports them in source order with the label
    /// last. Verified against sphinx 9.1.0 on this exact pair of documents.
    #[test]
    fn parse_time_diagnostics_precede_label_diagnostics() {
        let document = ".. envvar:: MYVAR\n\n\
                        .. glossary::\n\n   \
                        alpha\n      \
                        The first.\n\n\
                        .. _dup:\n\n\
                        Sec\n---\n\nx\n";
        let (_, warnings) = read(&[("a", document), ("b", document)]);

        assert_eq!(
            warnings
                .iter()
                .map(|warning| (warning.line, warning.message.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    Some(1),
                    "duplicate envvar description of MYVAR, other instance in a"
                ),
                (
                    Some(4),
                    "duplicate term description of alpha, other instance in a"
                ),
                (
                    Some(11),
                    "duplicate label dup, other instance in /src/a.rst"
                ),
            ],
            "{warnings:?}"
        );
    }

    /// `envvar`/`confval` register std objects from the `desc` anatomy;
    /// `option` registers program options instead (and never an object);
    /// `describe`/`object` register nothing at all, because the base
    /// `ObjectDescription.add_target_and_index` is a no-op. Both tables
    /// below are the `env.domaindata['std']` a sphinx 9.1.0 dummy build
    /// produces for this exact source.
    #[test]
    fn object_descriptions_register_per_directive() {
        let (env, warnings) = read(&[(
            "a",
            ".. envvar:: HOME_A\n\n\
             .. confval:: my_setting\n\n\
             .. program:: myprog\n\n\
             .. option:: --verbose, -v\n\n\
             .. program:: None\n\n\
             .. option:: --global-opt\n\n\
             .. describe:: widget\n\n\
             .. object:: thing\n",
        )]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            env.std.objects,
            [
                (
                    ("confval".to_string(), "my_setting".to_string()),
                    ("a".to_string(), "confval-my_setting".to_string())
                ),
                (
                    ("envvar".to_string(), "HOME_A".to_string()),
                    ("a".to_string(), "envvar-HOME_A".to_string())
                ),
            ]
            .into_iter()
            .collect(),
            "`option` contributes no object, and neither `describe` nor \
             `object` contributes anything"
        );
        assert_eq!(
            env.std.progoptions,
            [
                (
                    (None, "--global-opt".to_string()),
                    ("a".to_string(), "cmdoption-global-opt".to_string())
                ),
                // Both spellings of one signature register against its FIRST id.
                (
                    (Some("myprog".to_string()), "--verbose".to_string()),
                    ("a".to_string(), "cmdoption-myprog-verbose".to_string())
                ),
                (
                    (Some("myprog".to_string()), "-v".to_string()),
                    ("a".to_string(), "cmdoption-myprog-verbose".to_string())
                ),
            ]
            .into_iter()
            .collect()
        );
    }

    /// `:no-typesetting:` registers the object and then throws the whole
    /// `desc` node away, leaving only an `index` node and a bare target
    /// (`ObjectDescription.run:299-313`) — so nothing about the object is
    /// left in the doctree to harvest. Verified against a sphinx 9.1.0
    /// build of this source, which registers all three.
    #[test]
    fn no_typesetting_registers_the_object_it_refuses_to_render() {
        let (env, warnings) = read(&[(
            "a",
            ".. confval:: hidden_setting\n   :no-typesetting:\n\n\
             .. envvar:: HIDDEN\n   :no-typesetting:\n\n\
             .. option:: --hidden\n   :no-typesetting:\n",
        )]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            env.std.objects,
            [
                (
                    ("confval".to_string(), "hidden_setting".to_string()),
                    ("a".to_string(), "confval-hidden_setting".to_string())
                ),
                (
                    ("envvar".to_string(), "HIDDEN".to_string()),
                    ("a".to_string(), "envvar-HIDDEN".to_string())
                ),
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(
            env.std.progoptions,
            [(
                (None, "--hidden".to_string()),
                ("a".to_string(), "cmdoption-hidden".to_string())
            )]
            .into_iter()
            .collect()
        );
    }

    /// `note_object`'s duplicate warning [ENV §8 #2], which the description
    /// walk raises with the signature's own line — byte-checked against a
    /// sphinx 9.1.0 build of the same two documents.
    #[test]
    fn a_duplicate_object_description_warns_with_the_sphinx_text() {
        let (_, warnings) = read(&[
            ("a", ".. envvar:: HOME\n"),
            ("b", "B\n=\n\n.. envvar:: HOME\n"),
        ]);
        assert_eq!(
            warnings.iter().map(|w| w.render()).collect::<Vec<_>>(),
            vec![
                "/src/b.rst:4: WARNING: duplicate envvar description of HOME, \
                 other instance in a"
            ]
        );
    }

    #[test]
    fn the_four_virtual_labels_are_preseeded() {
        let std = StdDomainData::default();
        assert_eq!(
            std.labels.get("genindex"),
            Some(&("genindex".to_string(), String::new(), "Index".to_string()))
        );
        assert_eq!(
            std.labels.get("modindex"),
            Some(&(
                "py-modindex".to_string(),
                String::new(),
                "Module Index".to_string()
            ))
        );
        assert_eq!(
            std.labels.get("py-modindex"),
            Some(&(
                "py-modindex".to_string(),
                String::new(),
                "Python Module Index".to_string()
            ))
        );
        assert_eq!(
            std.anonlabels.get("search"),
            Some(&("search".to_string(), String::new()))
        );
        assert_eq!(std.anonlabels.len(), 4);
    }

    #[test]
    fn a_label_before_a_section_is_titled_by_that_section() {
        let (env, warnings) = read(&[(
            "a",
            "A\n=\n\n.. _dup-label:\n\nSection One\n-----------\n\nText.\n",
        )]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            env.std.labels.get("dup-label"),
            Some(&(
                "a".to_string(),
                "dup-label".to_string(),
                "Section One".to_string()
            )),
            "the target donates its id to the following section, whose \
             title becomes the label's section name"
        );
        assert_eq!(
            env.std.anonlabels.get("dup-label"),
            Some(&("a".to_string(), "dup-label".to_string()))
        );
        // Implicit section names are never labels: only explicit targets.
        assert!(!env.std.labels.contains_key("section one"));
    }

    #[test]
    fn a_duplicate_label_warns_at_the_second_definition() {
        let (env, warnings) = read(&[
            (
                "a",
                "A\n=\n\n.. _dup-label:\n\nSection One\n-----------\n\nText.\n",
            ),
            (
                "b",
                "B\n=\n\n.. _dup-label:\n\nSection Two\n-----------\n\nText.\n",
            ),
        ]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(
            warnings[0].render(),
            "/src/b.rst:7: WARNING: duplicate label dup-label, other instance in /src/a.rst",
            "the location is the *section* the target propagated onto, whose \
             docutils line is its title underline"
        );
        // Last definition wins.
        assert_eq!(
            env.std.labels["dup-label"],
            (
                "b".to_string(),
                "dup-label".to_string(),
                "Section Two".to_string()
            )
        );
    }

    #[test]
    fn a_label_on_a_captioned_figure_is_titled_by_its_caption() {
        let (env, _) = read(&[(
            "a",
            "A\n=\n\n.. figure:: pic.png\n   :name: fig-a\n\n   The Caption\n",
        )]);
        assert_eq!(
            env.std.labels.get("fig-a"),
            Some(&(
                "a".to_string(),
                "fig-a".to_string(),
                "The Caption".to_string()
            ))
        );
    }

    #[test]
    fn a_label_on_an_uncaptioned_enumerable_stays_anonymous_only() {
        let (env, _) = read(
            &["a"]
                .iter()
                .map(|d| (*d, "A\n=\n\n.. figure:: pic.png\n   :name: fig-a\n"))
                .collect::<Vec<_>>(),
        );
        assert!(
            !env.std.labels.contains_key("fig-a"),
            "an uncaptioned figure has no numfig title, so `continue`"
        );
        assert!(env.std.anonlabels.contains_key("fig-a"));
    }

    /// The node id keeps the term's case: `make_glossary_term` goes through
    /// sphinx's own `_make_id` fork, not docutils' lowercasing `make_id`
    /// (the corpus case `sx_directives.glossary_case_and_underscores` pins
    /// `term-HTTP_Method` against the oracle).
    #[test]
    fn glossary_terms_register_as_objects_and_lowercased_terms() {
        let (env, warnings) = read(&[(
            "a",
            "A\n=\n\n.. glossary::\n\n   Environment\n      A thing.\n\n   template engine\n      Another.\n",
        )]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            env.std
                .objects
                .get(&("term".to_string(), "Environment".to_string())),
            Some(&("a".to_string(), "term-Environment".to_string()))
        );
        assert_eq!(
            env.std.terms.get("environment"),
            Some(&("a".to_string(), "term-Environment".to_string())),
            "`terms` is keyed by the lowercased term, `objects` by the term as written"
        );
        assert!(env.std.terms.contains_key("template engine"));
        // A glossary term is not a label.
        assert_eq!(env.std.labels.len(), PRESEEDED_LABELS.len());
    }

    /// The location is pinned to what sphinx 9.1.0 actually prints for this
    /// exact source, checked by building it with `sphinx -b dummy`:
    ///
    /// ```text
    /// b.rst:5: WARNING: duplicate term description of environment, other instance in a
    /// ```
    ///
    /// The term is on line 6 — see [`glossary_term_line`] for why Sphinx
    /// says 5 (the same run reports 7 and 10 for terms on lines 8 and 11).
    #[test]
    fn a_term_defined_twice_warns_naming_the_other_document() {
        let glossary = "A\n=\n\n.. glossary::\n\n   environment\n      A thing.\n";
        let (env, warnings) = read(&[("a", glossary), ("b", glossary)]);

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(
            warnings[0].render(),
            "/src/b.rst:5: WARNING: duplicate term description of environment, \
             other instance in a",
            "an object duplicate names the other *docname*, not its path — \
             and offers no `:no-index:` hint, unlike the py domain's"
        );
        assert_eq!(
            env.std.terms["environment"],
            ("b".to_string(), "term-environment".to_string())
        );
    }

    /// A `system_message` between the target and the section it labels —
    /// an unknown directive, say — must be stepped *over*, not into:
    /// docutils skips it with `next_node(ascend=True, descend=False)`, so
    /// the ids land on the section, not on a paragraph inside the warning.
    #[test]
    fn a_system_message_between_a_target_and_its_section_is_stepped_over() {
        let (env, _) = read(&[(
            "a",
            "A\n=\n\n.. _lbl:\n\n.. nosuchdirective::\n\n   body\n\nSection\n-------\n\nText.\n",
        )]);

        assert_eq!(
            env.std.labels.get("lbl"),
            Some(&("a".to_string(), "lbl".to_string(), "Section".to_string())),
            "the label belongs to the section behind the message"
        );
    }

    /// A captionless `toctree` stores docutils' `None` rendering (`"True"`)
    /// in its caption attribute; Sphinx tests the real value's truthiness,
    /// so the label stays anonymous-only rather than being named "True".
    #[test]
    fn a_label_on_a_captionless_toctree_is_not_named_by_the_none_sentinel() {
        let (env, _) = read(&[("a", "A\n=\n\n.. _lbl:\n\n.. toctree::\n\n   other\n")]);

        assert!(
            !env.std.labels.contains_key("lbl"),
            "got {:?}",
            env.std.labels.get("lbl")
        );
        assert!(env.std.anonlabels.contains_key("lbl"));

        // A real caption still names it.
        let (env, _) = read(&[(
            "a",
            "A\n=\n\n.. _lbl:\n\n.. toctree::\n   :caption: Real Caption\n\n   other\n",
        )]);
        assert_eq!(env.std.labels["lbl"].2, "Real Caption".to_string());
    }

    #[test]
    fn a_label_pointing_at_a_link_target_is_skipped() {
        // `.. _elsewhere: https://example.com/` — an external target, which
        // Sphinx skips ("labels automatically generated from a link").
        let (env, _) = read(&[("a", "A\n=\n\n.. _elsewhere: https://example.com/\n")]);
        assert!(!env.std.labels.contains_key("elsewhere"));
        assert!(!env.std.anonlabels.contains_key("elsewhere"));
    }

    #[test]
    fn first_program_option_entry_wins() {
        let mut std = StdDomainData::default();
        std.add_program_option(Some("prog"), "--opt", "a", "cmdoption-prog-opt");
        std.add_program_option(Some("prog"), "--opt", "b", "other-id");
        assert_eq!(
            std.progoptions[&(Some("prog".to_string()), "--opt".to_string())],
            ("a".to_string(), "cmdoption-prog-opt".to_string())
        );
    }

    #[test]
    fn note_object_reports_the_previous_docname() {
        let mut std = StdDomainData::default();
        assert_eq!(std.note_object("envvar", "PATH", "a", "envvar-PATH"), None);
        assert_eq!(
            std.note_object("envvar", "PATH", "b", "envvar-PATH"),
            Some("a".to_string())
        );
        assert_eq!(
            std.objects[&("envvar".to_string(), "PATH".to_string())],
            ("b".to_string(), "envvar-PATH".to_string()),
            "the later description still wins, exactly like Sphinx"
        );
    }

    #[test]
    fn node_lines_follow_docutils_conventions() {
        let source = "Top\n===\n\nUnder\n-----\n\nBody.\n";
        let parsed = parse(source, "a");
        let top = &parsed.doctree.root.children[0];
        assert_eq!(node_line(top, source), 2, "section line = its underline");
        let under = top
            .children
            .iter()
            .find(|c| c.kind == kinds::SECTION)
            .unwrap();
        assert_eq!(node_line(under, source), 5);
        let body = under
            .children
            .iter()
            .find(|c| c.kind == kinds::PARAGRAPH)
            .unwrap();
        assert_eq!(node_line(body, source), 7, "other nodes: their first line");
    }
}
