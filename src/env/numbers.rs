//! Section and figure numbering: `TocTreeCollector.assign_section_numbers`
//! and `assign_figure_numbers` (`sphinx/environment/collectors/toctree.py:197-378`),
//! the two halves of `get_updated_docs` the resolve phase runs once every
//! document's toc is in the environment.
//!
//! - [`assign_section_numbers`] walks `env.tocs` from every `:numbered:`
//!   toctree, filling `env.toc_secnumbers[docname][anchorname]` and stamping
//!   a `secnumber` attribute onto the very `reference` nodes
//!   [`super::toctree::build_toc`] created (plus the document's title node,
//!   so rellinks can show its number).
//! - [`assign_figure_numbers`] walks the *doctrees* in document order,
//!   following toctrees depth-first, and numbers every enumerable node
//!   (figure/table/captioned code-block) into
//!   `env.toc_fignumbers[docname][figtype][id]`.
//!
//! Both return the docnames whose numbers differ from the previous
//! environment — Sphinx's `env-get-updated` contribution, which widens the
//! write set on an incremental build.
//!
//! NOT here: the `numref` role's use of these tables (`std` domain
//! resolution) and `numfig_format` rendering. This module only assigns the
//! numbers.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::doctree::{kinds, AttrValue, Doctree, Node};
use crate::env::toctree::VIRTUAL_DOC_NAMES;
use crate::env::BuildEnvironment;

/// How the numbering passes reach a document's doctree.
///
/// Sphinx calls `env.get_doctree(docname)`, which unpickles
/// `doctreedir/<docname>.doctree` on every call. This crate already holds
/// most doctrees in memory (the read phase's results, including the ones a
/// warm cache hit loaded from disk), so the loader hands back a [`Cow`]:
/// borrowed for a doctree the caller already has, owned for one it had to
/// read. Returning `None` — Sphinx would raise — simply skips the document.
pub type DoctreeLoader<'a> = dyn Fn(&str) -> Option<Cow<'a, Doctree>> + 'a;

/// One diagnostic from [`assign_section_numbers`].
///
/// Sphinx logs it with `location=toctreenode`, which renders as the
/// containing document's `source:line`. A [`Node`] in this crate carries
/// byte offsets, not line numbers, so the location is reported as the
/// containing docname plus the ordinal of the `toctree` node within that
/// document (document order) — enough for the builder to look the line up
/// in the document's parse records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberingWarning {
    /// The document containing the `toctree` node the warning is about.
    pub docname: String,
    /// That node's 0-based ordinal among the document's `toctree` nodes, in
    /// document order.
    pub toctree_index: usize,
    /// The message, formatted exactly as Sphinx formats it.
    pub message: String,
    /// Sphinx's `type.subtype` category; see
    /// [`crate::error::BuildWarning::category`].
    pub category: Option<String>,
}

/// What [`assign_section_numbers`] produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionNumbering {
    /// Docnames whose `toc_secnumbers` differ from the previous
    /// environment's (Sphinx's `rewrite_needed`), in assignment order.
    pub changed: Vec<String>,
    pub warnings: Vec<NumberingWarning>,
}

/// Assign a section number to every heading under a `:numbered:` toctree.
///
/// Exact port of `assign_section_numbers` (`collectors/toctree.py:197-283`).
/// `env.toc_secnumbers` is rebuilt from scratch (the old table is only kept
/// to diff against), every reachable document is numbered exactly once, and
/// a document a second numbered toctree tries to claim produces warning #20
/// instead.
///
/// The walk mutates `env.tocs` in place: each entry's `reference` node gets
/// the `secnumber` attribute Sphinx stamps there, which is what makes the
/// rendered toc show `1.2. Title`.
pub fn assign_section_numbers<'d>(
    env: &mut BuildEnvironment,
    load_doctree: &DoctreeLoader<'d>,
) -> SectionNumbering {
    let old = std::mem::take(&mut env.toc_secnumbers);
    let mut walker = SecnumWalker {
        env,
        load_doctree,
        old,
        assigned: BTreeSet::new(),
        numstack: vec![0],
        out: SectionNumbering::default(),
    };

    // Sphinx iterates `env.numbered_toctrees`, a Python set; this iterates
    // the same membership in docname order, which is deterministic (the
    // numbers themselves are per-toctree, so the order only decides which
    // of two competing numbered toctrees wins — and that case is exactly
    // the one warning #20 reports).
    let containers: Vec<String> = walker.env.numbered_toctrees.iter().cloned().collect();
    for docname in containers {
        walker.assigned.insert(docname.clone());
        let Some(doctree) = (walker.load_doctree)(&docname) else {
            continue;
        };
        for (index, toctree) in toctree_nodes(&doctree.root).into_iter().enumerate() {
            let depth = numbered_depth(toctree);
            if depth != 0 {
                // Every numbered toctree restarts the numbering.
                walker.numstack = vec![0];
                walker.walk_toctree(&docname, index, toctree, depth);
            }
        }
    }

    walker.out
}

struct SecnumWalker<'e, 'l, 'd> {
    env: &'e mut BuildEnvironment,
    load_doctree: &'l DoctreeLoader<'d>,
    /// `env.toc_secnumbers` as it was before this pass, to diff against.
    old: BTreeMap<String, BTreeMap<String, Vec<u32>>>,
    assigned: BTreeSet<String>,
    numstack: Vec<u32>,
    out: SectionNumbering,
}

impl SecnumWalker<'_, '_, '_> {
    /// `_walk_toctree` (`collectors/toctree.py:247-281`).
    ///
    /// `location_*` identify the `toctree` node being walked, for warning
    /// #20's `location=toctreenode`.
    fn walk_toctree(
        &mut self,
        location_docname: &str,
        location_index: usize,
        toctree: &Node,
        depth: i64,
    ) {
        if depth == 0 {
            return;
        }
        // Sphinx iterates `toctree['entries']` and skips URL entries and
        // `'self'`; `includefiles` is exactly that subset of `entries`, in
        // the same order (`directives/other.py:88-176` appends to both
        // together and reverses both together).
        for reference in toctree_includefiles(toctree).to_vec() {
            if self.assigned.contains(&reference) {
                self.out.warnings.push(NumberingWarning {
                    docname: location_docname.to_string(),
                    toctree_index: location_index,
                    message: format!(
                        "{reference} is already assigned section numbers \
                         (nested numbered toctree?)"
                    ),
                    category: Some("toc.secnum".to_string()),
                });
                continue;
            }
            if !self.env.tocs.contains_key(&reference) {
                continue;
            }
            self.assigned.insert(reference.clone());

            // The walk writes into the toc's own `reference` nodes and into
            // the document's title node, so both are lifted out of the
            // environment for the duration and put back afterwards.
            let mut toc = self
                .env
                .tocs
                .remove(&reference)
                .expect("presence checked above");
            let mut titlenode = self.env.titles.remove(&reference);
            let mut secnums: BTreeMap<String, Vec<u32>> = BTreeMap::new();
            let mut toctree_index = 0;
            self.walk_toc(
                &reference,
                &mut toctree_index,
                &mut toc.children,
                &mut secnums,
                depth,
                titlenode.as_mut(),
            );
            self.env.tocs.insert(reference.clone(), toc);
            if let Some(title) = titlenode {
                self.env.titles.insert(reference.clone(), title);
            }

            if self.old.get(&reference) != Some(&secnums) {
                self.out.changed.push(reference.clone());
            }
            // Sphinx installs `secnums` in `env.toc_secnumbers` before the
            // walk and mutates it in place; nothing reads the table during
            // the walk, so installing the finished map here is equivalent.
            self.env.toc_secnumbers.insert(reference, secnums);
        }
    }

    /// `_walk_toc` (`collectors/toctree.py:206-245`).
    ///
    /// `titlenode` is the document's own title, which the first numbered
    /// entry claims (that is the document itself); every branch that hands
    /// it down also drops its own copy, which `Option::take` mirrors
    /// exactly.
    fn walk_toc(
        &mut self,
        docname: &str,
        toctree_index: &mut usize,
        children: &mut [Node],
        secnums: &mut BTreeMap<String, Vec<u32>>,
        depth: i64,
        mut titlenode: Option<&mut Node>,
    ) {
        for subnode in children.iter_mut() {
            match subnode.kind {
                kinds::BULLET_LIST => {
                    self.numstack.push(0);
                    self.walk_toc(
                        docname,
                        toctree_index,
                        &mut subnode.children,
                        secnums,
                        depth - 1,
                        titlenode.take(),
                    );
                    self.numstack.pop();
                }
                // `only` entries are numbered even though the tag filter may
                // exclude them later: Sphinx accepts the resulting gaps
                // rather than guessing (`collectors/toctree.py:223-228`).
                kinds::LIST_ITEM | kinds::ONLY => {
                    self.walk_toc(
                        docname,
                        toctree_index,
                        &mut subnode.children,
                        secnums,
                        depth,
                        titlenode.take(),
                    );
                }
                kinds::COMPACT_PARAGRAPH => {
                    // Object-description entries (`desc_signature`) opt out.
                    if subnode.get("skip_section_number").is_some() {
                        continue;
                    }
                    if let Some(last) = self.numstack.last_mut() {
                        *last += 1;
                    }
                    let number = if depth > 0 {
                        Some(self.numstack.clone())
                    } else {
                        None
                    };
                    let Some(reference) = subnode.children.first_mut() else {
                        continue;
                    };
                    let anchorname = match reference.get("anchorname") {
                        Some(AttrValue::Str(anchor)) => anchor.clone(),
                        _ => String::new(),
                    };
                    secnums.insert(anchorname, number.clone().unwrap_or_default());
                    reference.set("secnumber", secnumber_attr(number.as_deref()));
                    if let Some(title) = titlenode.take() {
                        title.set("secnumber", secnumber_attr(number.as_deref()));
                    }
                }
                kinds::TOCTREE => {
                    let index = *toctree_index;
                    *toctree_index += 1;
                    self.walk_toctree(docname, index, subnode, depth);
                }
                _ => {}
            }
        }
    }
}

/// docutils renders a `None` attribute value as `name="True"` (its
/// "boolean attribute" convention, `nodes.py:663-664`), which is how a
/// section past the toctree's `:numbered:` depth prints — Sphinx sets
/// `reference['secnumber'] = None` there. A real number is a Python list,
/// which renders space-joined.
fn secnumber_attr(number: Option<&[u32]>) -> AttrValue {
    match number {
        Some(number) => AttrValue::List(number.iter().map(u32::to_string).collect()),
        None => AttrValue::Str("True".to_string()),
    }
}

/// Assign a figure number to every enumerable node reachable from the root
/// document.
///
/// Exact port of `assign_figure_numbers` (`collectors/toctree.py:285-378`).
/// Note the ordering Sphinx relies on: `env.toc_fignumbers` is cleared
/// *unconditionally*, but only refilled when `numfig` is on — so turning
/// `numfig` off drops every figure number (and, quirk included, reports
/// nothing as changed).
///
/// Must run after [`assign_section_numbers`]: figure numbers are scoped by
/// the section numbers that pass assigns.
pub fn assign_figure_numbers<'d>(
    env: &mut BuildEnvironment,
    numfig: bool,
    numfig_secnum_depth: u32,
    load_doctree: &DoctreeLoader<'d>,
) -> Vec<String> {
    let old = std::mem::take(&mut env.toc_fignumbers);
    if !numfig {
        return Vec::new();
    }

    let root_doc = env.root_doc.clone();
    let mut walker = FignumWalker {
        env,
        load_doctree,
        secnum_depth: numfig_secnum_depth as usize,
        assigned: BTreeSet::new(),
        counters: BTreeMap::new(),
    };
    walker.walk_doc(&root_doc, &[]);

    env.toc_fignumbers
        .iter()
        .filter(|(docname, fignums)| old.get(*docname) != Some(*fignums))
        .map(|(docname, _)| docname.clone())
        .collect()
}

struct FignumWalker<'e, 'l, 'd> {
    env: &'e mut BuildEnvironment,
    load_doctree: &'l DoctreeLoader<'d>,
    secnum_depth: usize,
    assigned: BTreeSet<String>,
    /// figtype -> (truncated section number -> count so far).
    counters: BTreeMap<String, BTreeMap<Vec<u32>, u32>>,
}

impl FignumWalker<'_, '_, '_> {
    /// `_walk_doc` (`collectors/toctree.py:366-370`).
    fn walk_doc(&mut self, docname: &str, secnum: &[u32]) {
        if !self.assigned.insert(docname.to_string()) {
            return;
        }
        // Sphinx's `env.get_doctree` raises for a document with no doctree;
        // here a document we cannot load is simply not numbered.
        let Some(doctree) = (self.load_doctree)(docname) else {
            return;
        };
        self.walk_doctree(docname, &doctree.root.children, secnum);
    }

    /// `_walk_doctree` (`collectors/toctree.py:338-364`).
    fn walk_doctree(&mut self, docname: &str, children: &[Node], secnum: &[u32]) {
        for subnode in children {
            // docutils Text nodes are not Elements: Sphinx's isinstance
            // chain skips them entirely.
            if subnode.kind == kinds::TEXT {
                continue;
            }
            if subnode.kind == kinds::SECTION {
                let next = self.section_number(docname, subnode);
                let inherited = if next.is_empty() { secnum } else { &next };
                self.walk_doctree(docname, &subnode.children, inherited);
            } else if subnode.kind == kinds::TOCTREE {
                // Document order, depth first: this is what makes figure
                // numbers follow the reading order of the whole project.
                // `includefiles` is `entries` minus URL and `self` entries,
                // which is exactly what Sphinx skips here.
                for subdocname in toctree_includefiles(subnode).to_vec() {
                    if VIRTUAL_DOC_NAMES.contains(&subdocname.as_str()) {
                        continue;
                    }
                    self.walk_doc(&subdocname, secnum);
                }
            } else {
                if let Some(figtype) = figtype_of(subnode) {
                    if let Some(figure_id) = subnode.attrs.ids.first() {
                        let figure_id = figure_id.clone();
                        self.register_fignumber(docname, secnum, figtype, &figure_id);
                    }
                }
                self.walk_doctree(docname, &subnode.children, secnum);
            }
        }
    }

    /// `get_section_number` (`collectors/toctree.py:310-318`): a section's
    /// own number if the toc has one, else the document's own.
    fn section_number(&self, docname: &str, section: &Node) -> Vec<u32> {
        let anchorname = format!(
            "#{}",
            section.attrs.ids.first().map(String::as_str).unwrap_or("")
        );
        let Some(secnumbers) = self.env.toc_secnumbers.get(docname) else {
            return Vec::new();
        };
        secnumbers
            .get(&anchorname)
            .or_else(|| secnumbers.get(""))
            .cloned()
            .unwrap_or_default()
    }

    /// `register_fignumber` + `get_next_fignumber`
    /// (`collectors/toctree.py:320-336`).
    fn register_fignumber(
        &mut self,
        docname: &str,
        secnum: &[u32],
        figtype: &str,
        figure_id: &str,
    ) {
        let truncated = &secnum[..secnum.len().min(self.secnum_depth)];
        let counter = self
            .counters
            .entry(figtype.to_string())
            .or_default()
            .entry(truncated.to_vec())
            .or_insert(0);
        *counter += 1;

        let mut number = truncated.to_vec();
        number.push(*counter);
        self.env
            .toc_fignumbers
            .entry(docname.to_string())
            .or_default()
            .entry(figtype.to_string())
            .or_default()
            .insert(figure_id.to_string(), number);
    }
}

/// `get_figtype` (`collectors/toctree.py:296-308`) over the two domains
/// that register enumerable nodes (`Domain.enumerable_nodes` is empty for
/// every other one, `domains/__init__.py:99`).
///
/// Sphinx asks every domain in `env.domains.sorted()` — **alphabetical**,
/// so `math` is asked before `std` — and takes the first answer, with one
/// twist: a `StandardDomain` whose `get_numfig_title` comes back empty is
/// `continue`d past *before* its figtype is considered, which is how an
/// uncaptioned figure/table/container ends up unnumbered. Only the std
/// domain gets that treatment, so a labelled equation is numbered with no
/// caption of any kind.
fn figtype_of(node: &Node) -> Option<&'static str> {
    if let Some(figtype) = math_enumerable_node_type(node) {
        return Some(figtype);
    }
    let figtype = std_enumerable_node_type(node);
    if clean_astext(std_numfig_title(node)?).is_empty() {
        return None;
    }
    figtype
}

/// `MathDomain.enumerable_nodes` (`domains/math.py:58-60`): a `math_block`
/// is a `displaymath`, which is where `:eq:` picks an equation's number up
/// (`domains/math.py:115-121`) when `numfig` and `math_numfig` are both on.
/// Note there is no `math_numfig` gate *here*: Sphinx numbers display math
/// whenever `numfig` is on, and only consults `math_numfig` when rendering.
fn math_enumerable_node_type(node: &Node) -> Option<&'static str> {
    (node.kind == "math_block").then_some("displaymath")
}

/// `StandardDomain.get_enumerable_node_type` (`domains/std/__init__.py:1380-1393`)
/// over `enumerable_nodes` (`:799-803`).
fn std_enumerable_node_type(node: &Node) -> Option<&'static str> {
    match node.kind {
        kinds::SECTION => Some("section"),
        "figure" => Some("figure"),
        kinds::TABLE => Some("table"),
        // A `container` carrying the `literal_block` flag *and* a
        // `literal_block` child is a captioned code block; every other
        // container falls through to the plain `enumerable_nodes` lookup,
        // which also says `code-block` (and is then filtered out by the
        // caption check, since a bare container has no caption).
        "container" => Some("code-block"),
        _ => None,
    }
}

/// `StandardDomain.get_numfig_title` (`domains/std/__init__.py:1366-1378`):
/// the caption (or title) of an enumerable node, or `None` when the node
/// isn't enumerable at all. Note `section` is deliberately absent from
/// `enumerable_nodes`, so a section has no numfig title.
pub(crate) fn std_numfig_title(node: &Node) -> Option<&Node> {
    if !matches!(node.kind, "figure" | "container" | kinds::TABLE) {
        return None;
    }
    node.children
        .iter()
        .find(|child| child.kind == "caption" || child.kind == kinds::TITLE)
}

/// `sphinx.util.nodes.clean_astext`: the node's text with `raw` subtrees
/// dropped and image alt text blanked (an `image` contributes no text here
/// either way).
pub(crate) fn clean_astext(node: &Node) -> String {
    fn walk(node: &Node, out: &mut String) {
        if node.kind == "raw" {
            return;
        }
        match &node.text {
            Some(text) => out.push_str(text),
            None => {
                for child in &node.children {
                    walk(child, out);
                }
            }
        }
    }
    let mut out = String::new();
    walk(node, &mut out);
    out
}

/// `toctreenode.get('numbered', 0)` — the `:numbered:` depth.
fn numbered_depth(toctree: &Node) -> i64 {
    match toctree.get("numbered") {
        Some(AttrValue::Int(depth)) => *depth,
        _ => 0,
    }
}

/// `toctree['includefiles']`: the docname subset of `toctree['entries']`,
/// in entry order.
fn toctree_includefiles(toctree: &Node) -> &[String] {
    match toctree.get("includefiles") {
        Some(AttrValue::List(files)) => files,
        _ => &[],
    }
}

/// Every `toctree` node in a subtree, in document (pre-order) order —
/// docutils `findall(addnodes.toctree)`.
fn toctree_nodes(node: &Node) -> Vec<&Node> {
    fn walk<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if node.kind == kinds::TOCTREE {
            out.push(node);
        }
        for child in &node.children {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::Span;
    use crate::env::toctree::{build_toc, document_title, note_toctree, toctree_copies};
    use crate::rst::{parse_rst, ParseOptions};

    /// Build an environment the way the real merge phase does, from a
    /// `docname -> rst` corpus, and hand back the doctrees for the loader.
    fn read(sources: &[(&str, &str)]) -> (BuildEnvironment, BTreeMap<String, Doctree>) {
        let found: BTreeSet<String> = sources.iter().map(|(name, _)| name.to_string()).collect();
        let found = std::sync::Arc::new(found);

        let mut env = BuildEnvironment {
            root_doc: "index".to_string(),
            ..Default::default()
        };
        let mut doctrees = BTreeMap::new();
        for (docname, body) in sources {
            let doctree = parse_rst(
                body,
                &ParseOptions {
                    source_path: format!("{docname}.rst"),
                    sphinx: true,
                    docname: (*docname).to_string(),
                    exclude_patterns: Vec::new(),
                    found_docs: Some(std::sync::Arc::clone(&found)),
                },
            );
            env.all_docs.insert((*docname).to_string(), 0);
            env.titles
                .insert((*docname).to_string(), document_title(&doctree));
            let (toc, entries) = build_toc(&doctree, docname);
            for toctree in toctree_copies(&toc) {
                note_toctree(&mut env, docname, toctree);
            }
            env.tocs.insert((*docname).to_string(), toc);
            env.toc_num_entries.insert((*docname).to_string(), entries);
            doctrees.insert((*docname).to_string(), doctree);
        }
        (env, doctrees)
    }

    fn loader<'a>(
        doctrees: &'a BTreeMap<String, Doctree>,
    ) -> impl Fn(&str) -> Option<Cow<'a, Doctree>> + 'a {
        move |docname: &str| doctrees.get(docname).map(Cow::Borrowed)
    }

    fn secnums(env: &BuildEnvironment, docname: &str) -> Vec<(String, Vec<u32>)> {
        env.toc_secnumbers
            .get(docname)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    #[test]
    fn numbered_toctree_numbers_every_reachable_document() {
        let (mut env, doctrees) = read(&[
            (
                "index",
                "Index\n=====\n\n.. toctree::\n   :numbered:\n\n   a\n   b\n",
            ),
            ("a", "A\n=\n\nSub\n---\n\nText.\n"),
            ("b", "B\n=\n\nLeaf.\n"),
        ]);
        let load = loader(&doctrees);
        let out = assign_section_numbers(&mut env, &load);

        assert_eq!(
            secnums(&env, "a"),
            vec![(String::new(), vec![1]), ("#sub".to_string(), vec![1, 1]),]
        );
        assert_eq!(secnums(&env, "b"), vec![(String::new(), vec![2])]);
        // The container itself is never numbered.
        assert!(!env.toc_secnumbers.contains_key("index"));
        assert_eq!(out.changed, vec!["a".to_string(), "b".to_string()]);
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn secnumbers_are_stamped_onto_the_toc_references_and_the_title() {
        let (mut env, doctrees) = read(&[
            (
                "index",
                "Index\n=====\n\n.. toctree::\n   :numbered:\n\n   a\n",
            ),
            ("a", "A\n=\n\nSub\n---\n\nText.\n"),
        ]);
        let load = loader(&doctrees);
        assign_section_numbers(&mut env, &load);

        let toc = env.tocs["a"].pformat();
        assert!(
            toc.contains(r#"anchorname="" internal="1" refuri="a" secnumber="1""#),
            "{toc}"
        );
        assert!(
            toc.contains(r##"anchorname="#sub" internal="1" refuri="a" secnumber="1 1""##),
            "{toc}"
        );
        // The document title carries the document's own number, which is
        // what next/prev/parent rellinks render.
        assert_eq!(
            env.titles["a"].get("secnumber"),
            Some(&AttrValue::List(vec!["1".to_string()]))
        );
    }

    #[test]
    fn entries_below_the_numbered_depth_get_no_number() {
        let (mut env, doctrees) = read(&[
            (
                "index",
                "Index\n=====\n\n.. toctree::\n   :numbered: 1\n\n   a\n",
            ),
            ("a", "A\n=\n\nSub\n---\n\nText.\n"),
        ]);
        let load = loader(&doctrees);
        assign_section_numbers(&mut env, &load);

        assert_eq!(
            secnums(&env, "a"),
            vec![(String::new(), vec![1]), ("#sub".to_string(), vec![])]
        );
        // docutils prints a None attribute value as `="True"`.
        assert!(
            env.tocs["a"]
                .pformat()
                .contains(r##"anchorname="#sub" internal="1" refuri="a" secnumber="True""##),
            "{}",
            env.tocs["a"].pformat()
        );
    }

    #[test]
    fn a_document_two_numbered_toctrees_claim_warns_once() {
        let (mut env, doctrees) = read(&[
            (
                "index",
                "Index\n=====\n\n.. toctree::\n   :numbered:\n\n   a\n\n.. toctree::\n   :numbered:\n\n   a\n",
            ),
            ("a", "A\n=\n\nText.\n"),
        ]);
        let load = loader(&doctrees);
        let out = assign_section_numbers(&mut env, &load);

        assert_eq!(
            out.warnings,
            vec![NumberingWarning {
                docname: "index".to_string(),
                toctree_index: 1,
                message: "a is already assigned section numbers (nested numbered toctree?)"
                    .to_string(),
                category: Some("toc.secnum".to_string()),
            }]
        );
        // The first toctree still numbered it.
        assert_eq!(secnums(&env, "a"), vec![(String::new(), vec![1])]);
    }

    #[test]
    fn a_document_that_is_not_numbered_keeps_no_secnumbers() {
        let (mut env, doctrees) = read(&[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n"),
            ("a", "A\n=\n\nText.\n"),
        ]);
        let load = loader(&doctrees);
        let out = assign_section_numbers(&mut env, &load);

        assert!(env.toc_secnumbers.is_empty());
        assert!(out.changed.is_empty());
        assert!(!env.tocs["a"].pformat().contains("secnumber="));
    }

    #[test]
    fn unchanged_numbers_are_not_reported_as_changed() {
        let (mut env, doctrees) = read(&[
            (
                "index",
                "Index\n=====\n\n.. toctree::\n   :numbered:\n\n   a\n",
            ),
            ("a", "A\n=\n\nText.\n"),
        ]);
        let load = loader(&doctrees);
        assert_eq!(assign_section_numbers(&mut env, &load).changed, ["a"]);
        // A second pass over the same environment finds the same numbers.
        assert!(assign_section_numbers(&mut env, &load).changed.is_empty());
    }

    #[test]
    fn figure_numbers_are_scoped_by_the_secnum_depth() {
        let sources = [
            (
                "index",
                "Index\n=====\n\n.. toctree::\n   :numbered:\n\n   a\n   b\n",
            ),
            (
                "a",
                "A\n=\n\n.. figure:: one.png\n   :name: fig-one\n\n   One.\n",
            ),
            (
                "b",
                "B\n=\n\n.. figure:: two.png\n   :name: fig-two\n\n   Two.\n",
            ),
        ];
        // depth 1: figures are numbered per top-level document number.
        let (mut env, doctrees) = read(&sources);
        let load = loader(&doctrees);
        assign_section_numbers(&mut env, &load);
        assign_figure_numbers(&mut env, true, 1, &load);
        assert_eq!(env.toc_fignumbers["a"]["figure"]["fig-one"], vec![1, 1]);
        assert_eq!(env.toc_fignumbers["b"]["figure"]["fig-two"], vec![2, 1]);

        // depth 0: one global counter.
        let (mut env, doctrees) = read(&sources);
        let load = loader(&doctrees);
        assign_section_numbers(&mut env, &load);
        assign_figure_numbers(&mut env, true, 0, &load);
        assert_eq!(env.toc_fignumbers["a"]["figure"]["fig-one"], vec![1]);
        assert_eq!(env.toc_fignumbers["b"]["figure"]["fig-two"], vec![2]);
    }

    #[test]
    fn uncaptioned_enumerables_are_skipped() {
        let (mut env, doctrees) = read(&[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n"),
            (
                "a",
                "A\n=\n\n.. figure:: none.png\n   :name: fig-bare\n\n.. figure:: cap.png\n   :name: fig-cap\n\n   Caption.\n",
            ),
        ]);
        let load = loader(&doctrees);
        assign_figure_numbers(&mut env, true, 1, &load);

        let figures = &env.toc_fignumbers["a"]["figure"];
        assert!(!figures.contains_key("fig-bare"), "{figures:?}");
        assert_eq!(figures["fig-cap"], vec![1]);
    }

    #[test]
    fn numfig_off_clears_every_figure_number() {
        let (mut env, doctrees) = read(&[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n"),
            (
                "a",
                "A\n=\n\n.. figure:: cap.png\n   :name: fig-cap\n\n   Caption.\n",
            ),
        ]);
        let load = loader(&doctrees);
        assign_figure_numbers(&mut env, true, 1, &load);
        assert!(!env.toc_fignumbers.is_empty());

        // Sphinx resets the table before checking `numfig`, so turning the
        // option off drops the numbers assigned under it.
        assert!(assign_figure_numbers(&mut env, false, 1, &load).is_empty());
        assert!(env.toc_fignumbers.is_empty());
    }

    #[test]
    fn a_document_reached_twice_is_numbered_once() {
        let (mut env, doctrees) = read(&[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n   b\n"),
            (
                "a",
                "A\n=\n\n.. figure:: cap.png\n   :name: fig-a\n\n   Caption.\n\n.. toctree::\n\n   b\n",
            ),
            (
                "b",
                "B\n=\n\n.. figure:: cap.png\n   :name: fig-b\n\n   Caption.\n",
            ),
        ]);
        let load = loader(&doctrees);
        assign_figure_numbers(&mut env, true, 1, &load);

        // `b` is reached from `a` first (depth-first), then skipped when
        // `index`'s own toctree gets to it: one number, not two.
        assert_eq!(env.toc_fignumbers["a"]["figure"]["fig-a"], vec![1]);
        assert_eq!(env.toc_fignumbers["b"]["figure"]["fig-b"], vec![2]);
    }

    #[test]
    fn tables_and_captioned_code_blocks_get_their_own_counters() {
        let (mut env, doctrees) = read(&[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n"),
            (
                "a",
                "A\n=\n\n.. list-table:: The Table\n   :name: tab-a\n\n   * - x\n\n.. code-block:: python\n   :name: code-a\n   :caption: The Listing\n\n   x = 1\n",
            ),
        ]);
        let load = loader(&doctrees);
        assign_figure_numbers(&mut env, true, 1, &load);

        assert_eq!(env.toc_fignumbers["a"]["table"]["tab-a"], vec![1]);
        assert_eq!(env.toc_fignumbers["a"]["code-block"]["code-a"], vec![1]);
    }

    /// The `math` domain also registers an enumerable node, and sorts
    /// *before* `std`: a labelled equation is numbered as `displaymath`
    /// with no caption requirement, while an unlabelled one has no ids and
    /// is therefore never registered.
    #[test]
    fn labelled_display_math_is_numbered_as_displaymath() {
        let (mut env, doctrees) = read(&[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n"),
            (
                "a",
                "A\n=\n\n.. math::\n   :label: eq-one\n\n   x = 1\n\n.. math::\n\n   y = 2\n",
            ),
        ]);
        let load = loader(&doctrees);
        assign_figure_numbers(&mut env, true, 1, &load);

        let equations = &env.toc_fignumbers["a"]["displaymath"];
        assert_eq!(equations["equation-eq-one"], vec![1]);
        assert_eq!(equations.len(), 1, "{equations:?}");
    }

    #[test]
    fn a_missing_doctree_is_skipped_rather_than_panicking() {
        let mut env = BuildEnvironment {
            root_doc: "index".to_string(),
            ..Default::default()
        };
        env.numbered_toctrees.insert("index".to_string());
        let empty: BTreeMap<String, Doctree> = BTreeMap::new();
        let load = loader(&empty);

        assert!(assign_section_numbers(&mut env, &load).changed.is_empty());
        assert!(assign_figure_numbers(&mut env, true, 1, &load).is_empty());
    }

    #[test]
    fn secnumber_attribute_renders_the_way_docutils_renders_it() {
        let mut node = Node::elem(kinds::REFERENCE, Span::ZERO);
        node.set("secnumber", secnumber_attr(Some(&[1, 2, 3])));
        assert_eq!(node.pformat(), "<reference secnumber=\"1 2 3\">\n");

        let mut node = Node::elem(kinds::REFERENCE, Span::ZERO);
        node.set("secnumber", secnumber_attr(None));
        assert_eq!(node.pformat(), "<reference secnumber=\"True\">\n");
    }
}
