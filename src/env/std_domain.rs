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
/// registrations Sphinx performs from directives at parse time (glossary
/// terms via `make_glossary_term`, `option`/`envvar`/`confval` objects via
/// `ObjectDescription.add_target_and_index`) — which this port harvests
/// from the finished doctree instead, since our parse layer has no domain
/// callbacks to run.
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
    collect_labels(env, doc, &ids, doc2path, warnings);
    collect_glossary_terms(env, doc, warnings);
    collect_descriptions(env, doc, warnings);
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
    // `next(node.findall(addnodes.toctree), None)` with a caption.
    let toctree = find_first(node, kinds::TOCTREE)?;
    match toctree.get("caption") {
        Some(AttrValue::Str(caption)) if !caption.is_empty() => Some(caption.clone()),
        _ => None,
    }
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
            warnings.push(duplicate_object_warning(doc, term, "term", &text, &other));
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

/// Object descriptions: Sphinx registers these while the directive runs
/// (`ObjectDescription.add_target_and_index` → `note_object` /
/// `add_program_option`, `domains/std/__init__.py:226-330`), so this replays
/// the registration from the finished `desc` anatomy instead.
///
/// Nothing produces `desc` nodes yet — the std directives land in the next
/// task, which is when the `std_objects` fixture project stops being
/// exempted. The walk is here so that landing them is a parser change only.
fn collect_descriptions(
    env: &mut BuildEnvironment,
    doc: &DocumentSource<'_>,
    warnings: &mut Vec<BuildWarning>,
) {
    let mut descs: Vec<&Node> = Vec::new();
    collect_desc_nodes(&doc.doctree.root, &mut descs);
    for desc in descs {
        // `describe`/`object` (GenericObject's base) deliberately register
        // nothing: `ObjectDescription.add_target_and_index` is a no-op
        // unless a domain overrides it.
        let Some(AttrValue::Str(domain)) = desc.get("domain") else {
            continue;
        };
        if domain != "std" {
            continue;
        }
        let Some(AttrValue::Str(objtype)) = desc.get("objtype") else {
            continue;
        };
        for signature in desc.children.iter().filter(|c| c.kind == "desc_signature") {
            let Some(node_id) = signature.attrs.ids.first() else {
                continue;
            };
            if objtype == "option" {
                // `Cmdoption.add_target_and_index` (`:290-315`) registers
                // every `allnames` spelling of the option against the
                // program in scope.
                let program = match desc.get("std:program") {
                    Some(AttrValue::Str(program)) => Some(program.as_str()),
                    _ => None,
                };
                for name in signature_names(signature) {
                    env.std
                        .add_program_option(program, &name, doc.docname, node_id);
                }
                continue;
            }
            let name = match signature.get("fullname") {
                Some(AttrValue::Str(name)) => name.clone(),
                _ => desc_name(signature),
            };
            if let Some(other) = env.std.note_object(objtype, &name, doc.docname, node_id) {
                warnings.push(duplicate_object_warning(
                    doc, signature, objtype, &name, &other,
                ));
            }
        }
    }
}

fn collect_desc_nodes<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
    if node.kind == "desc" {
        out.push(node);
        return;
    }
    for child in &node.children {
        collect_desc_nodes(child, out);
    }
}

/// `desc_signature['allnames']`: every spelling an `option` directive
/// declared (`--foo`, `-f`, ...).
fn signature_names(signature: &Node) -> Vec<String> {
    match signature.get("allnames") {
        Some(AttrValue::List(names)) => names.clone(),
        Some(AttrValue::Str(name)) => vec![name.clone()],
        _ => vec![desc_name(signature)],
    }
}

fn desc_name(signature: &Node) -> String {
    signature
        .children
        .iter()
        .find(|child| child.kind == "desc_name")
        .map(clean_astext)
        .unwrap_or_default()
}

/// Warning [ENV §8 #2]: `duplicate %s description of %s, other instance in %s`.
fn duplicate_object_warning(
    doc: &DocumentSource<'_>,
    node: &Node,
    objtype: &str,
    name: &str,
    other: &str,
) -> BuildWarning {
    BuildWarning::new(
        doc.path.to_path_buf(),
        Some(node_line(node, doc.text)),
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
pub(crate) struct DocumentIds<'a> {
    map: HashMap<&'a str, (usize, &'a Node)>,
}

impl<'a> DocumentIds<'a> {
    pub(crate) fn of(doctree: &'a Doctree) -> Self {
        // Document (pre-order) order, with each node's parent kind, exactly
        // the sequence `Node.next_node(ascend=True)` walks.
        let mut flat: Vec<(&Node, &'static str)> = Vec::new();
        flatten(&doctree.root, kinds::DOCUMENT, &mut flat);

        let mut map: HashMap<&str, (usize, &Node)> = HashMap::new();
        for (order, (node, _)) in flat.iter().enumerate() {
            for id in &node.attrs.ids {
                map.insert(id.as_str(), (order, *node));
            }
        }

        // PropagateTargets, in document order so that chained targets
        // collapse onto the same final node.
        for (index, (node, parent)) in flat.iter().enumerate() {
            if !is_propagating_target(node, parent) {
                continue;
            }
            let Some((order, target)) = next_propagation_target(&flat, index) else {
                continue;
            };
            for id in &node.attrs.ids {
                map.insert(id.as_str(), (order, target));
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

fn flatten<'a>(node: &'a Node, parent: &'static str, out: &mut Vec<(&'a Node, &'static str)>) {
    out.push((node, parent));
    for child in &node.children {
        flatten(child, node.kind, out);
    }
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
fn next_propagation_target<'a>(
    flat: &[(&'a Node, &'static str)],
    index: usize,
) -> Option<(usize, &'a Node)> {
    let mut next = index + 1;
    while flat
        .get(next)
        .is_some_and(|(node, _)| node.kind == kinds::SYSTEM_MESSAGE)
    {
        next += 1;
    }
    let (node, _) = flat.get(next)?;
    let blocked = matches!(
        node.kind,
        kinds::COMMENT
            | "substitution_definition"
            | "pending"
            | kinds::FOOTNOTE
            | kinds::CITATION
            | kinds::TEXT
    );
    (!blocked).then_some((next, *node))
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
            Some(&("a".to_string(), "term-environment".to_string()))
        );
        assert_eq!(
            env.std.terms.get("environment"),
            Some(&("a".to_string(), "term-environment".to_string())),
            "`terms` is keyed by the lowercased term, `objects` by the term as written"
        );
        assert!(env.std.terms.contains_key("template engine"));
        // A glossary term is not a label.
        assert_eq!(env.std.labels.len(), PRESEEDED_LABELS.len());
    }

    #[test]
    fn a_term_defined_twice_warns_naming_the_other_document() {
        let glossary = "A\n=\n\n.. glossary::\n\n   environment\n      A thing.\n";
        let (env, warnings) = read(&[("a", glossary), ("b", glossary)]);

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(
            warnings[0].render(),
            "/src/b.rst:4: WARNING: duplicate term description of environment, \
             other instance in a",
            "an object duplicate names the other *docname*, not its path — \
             and offers no `:no-index:` hint, unlike the py domain's"
        );
        assert_eq!(
            env.std.terms["environment"],
            ("b".to_string(), "term-environment".to_string())
        );
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
