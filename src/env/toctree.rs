//! Sphinx's toctree bookkeeping: the read-phase half of
//! `sphinx/environment/collectors/toctree.py` plus the two pieces of
//! `sphinx/directives/other.py` and `sphinx/environment/adapters/toctree.py`
//! that feed it.
//!
//! Three ports live here, in the order the build runs them:
//!
//! 1. [`resolve_entries`] — `TocTree.parse_content` (`directives/other.py:88`):
//!    turns the directive's raw content lines into the `entries`/`includefiles`
//!    attributes of the `toctree` node, resolving relative docnames, expanding
//!    `:glob:` patterns and dropping targets that aren't real documents. This
//!    is environment-dependent (it needs the full docname set), which is why
//!    the parser takes a `found_docs` set: Sphinx resolves at parse time too,
//!    from `env.found_docs`.
//! 2. [`build_toc`] — `TocTreeCollector.process_doc` (`collectors/toctree.py:64`):
//!    the document's local table of contents as a doctree-shaped
//!    `bullet_list`, with every `toctree` node copied into it.
//! 3. [`note_toctree`] — `adapters/toctree.py:32`: the toctree graph
//!    (`toctree_includes`, `files_to_rebuild`, `glob_toctrees`,
//!    `numbered_toctrees`).
//! 4. [`collect_relations`] / [`toctree_ancestors`] / [`check_consistency`]
//!    — the whole-project reads of that graph
//!    (`environment/__init__.py:778-823`, `adapters/toctree.py:562`), which
//!    only make sense once every document has been noted.
//!
//! [`document_title`] is the neighbouring `TitleCollector.process_doc`
//! (`collectors/title.py:27`), which shares this module's
//! `SphinxContentsFilter` port.
//!
//! NOT here (later wave-4 tasks): `assign_section_numbers` /
//! `assign_figure_numbers` (they write `toc_secnumbers`/`toc_fignumbers` and
//! stamp `secnumber` onto the references this module builds), the
//! `addnodes.desc` branch of `build_toc` — no `desc` nodes exist in the
//! doctree until the object-description directives land — and
//! `_resolve_toctree`, the write-phase renderer that turns a `toctree` node
//! into the rendered navigation tree (and emits the `circular toctree
//! references detected` diagnostic).

use std::collections::{BTreeMap, BTreeSet};

use crate::doctree::{kinds, AttrValue, Doctree, Node, Span};
use crate::env::BuildEnvironment;
use crate::matching;

/// Sphinx's `StandardDomain._virtual_doc_names` (`domains/std/__init__.py:784-788`):
/// docnames that resolve even though no source file produces them.
///
/// Careful: `_virtual_doc_names` is a **dict**, and every consumer that
/// treats it as a name set takes `frozenset(...)` of it — i.e. its *keys*
/// (`directives/other.py:91`, `collectors/toctree.py:287`,
/// `adapters/toctree.py:330`). The middle entry is therefore `modindex`,
/// the label authors write in a toctree; `py-modindex` is that key's
/// *value*, the docname the page is finally written to, and is not itself
/// a virtual name.
pub(crate) const VIRTUAL_DOC_NAMES: [&str; 3] = ["genindex", "modindex", "search"];

// ---------------------------------------------------------------------------
// 1. Entry resolution (sphinx/directives/other.py TocTree.parse_content)
// ---------------------------------------------------------------------------

/// Everything one `toctree` directive needs resolved against the project's
/// document set — the inputs of `TocTree.parse_content`.
#[derive(Debug, Clone, Copy)]
pub struct ToctreeContent<'a> {
    /// The directive's content lines, with blank ones already removed.
    pub content: &'a [String],
    /// The containing document.
    pub docname: &'a str,
    pub glob: bool,
    pub reversed: bool,
    /// 1-based line of the `.. toctree::` marker. Sphinx logs every
    /// diagnostic below with `location=toctree`, i.e. the directive node's
    /// source info — *not* the offending entry's own line.
    pub line: u32,
    /// Every docname the project discovered (sphinx `env.found_docs`).
    pub found_docs: &'a BTreeSet<String>,
    /// `source_suffix`, in configuration order; the first is what
    /// `doc2path` appends for a document that does not exist.
    pub source_suffixes: &'a [&'a str],
    /// `exclude_patterns`, which decide whether a missing entry is reported
    /// as excluded or as nonexisting.
    pub exclude_patterns: &'a [String],
}

/// What kind of toctree diagnostic this is, kept alongside the Sphinx
/// message so the builder can map it onto its own coarse
/// [`crate::error::WarningType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToctreeWarningKind {
    /// An entry naming a document that is excluded or absent.
    MissingDocument,
    /// A `:glob:` pattern that matched nothing.
    EmptyGlob,
    /// A document already claimed by an earlier entry.
    DuplicateEntry,
    /// Not a Sphinx diagnostic: a `:glob:` pattern this crate could not
    /// compile. Python's `fnmatch` cannot fail, so Sphinx has no equivalent
    /// — but silently treating the pattern as matching nothing would hide a
    /// real bug behind an `empty_glob` warning.
    PatternError,
}

/// One diagnostic produced while resolving a toctree's entries.
///
/// Carried on the parse record (and therefore through the document cache)
/// rather than logged on the spot: resolution happens inside the parser,
/// which has no warning sink, and a cache hit that skipped the parse must
/// still reproduce the build's warnings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToctreeWarning {
    /// 1-based line of the `.. toctree::` directive (Sphinx's
    /// `location=toctree`).
    pub line: u32,
    /// The message, formatted exactly as Sphinx formats it.
    pub message: String,
    /// Sphinx's `type.subtype` category, or `None` where Sphinx logs the
    /// warning without a `type` (see [`crate::error::BuildWarning::category`]).
    pub category: Option<String>,
    pub kind: ToctreeWarningKind,
}

/// `toctree['entries']` (title, ref) pairs plus `toctree['includefiles']`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedEntries {
    /// `(title, ref)`; `ref` is a resolved docname, an external URL, or the
    /// literal `self`.
    pub entries: Vec<(Option<String>, String)>,
    /// The subset of `entries` that are real documents, in the same order.
    pub includefiles: Vec<String>,
    /// Diagnostics for the entries that did not resolve. The single source
    /// of truth for toctree warnings: nothing downstream re-resolves.
    pub warnings: Vec<ToctreeWarning>,
}

impl ResolvedEntries {
    /// The `entries` attribute as docutils renders it: one Python tuple
    /// repr per item (`(None, 'intro')`), which `pformat` then
    /// `serial_escape`s and joins.
    pub fn entries_attr(&self) -> AttrValue {
        AttrValue::List(
            self.entries
                .iter()
                .map(|(title, target)| {
                    let title = match title {
                        Some(t) => py_repr_str(t),
                        None => "None".to_string(),
                    };
                    format!("({title}, {})", py_repr_str(target))
                })
                .collect(),
        )
    }

    pub fn includefiles_attr(&self) -> AttrValue {
        AttrValue::List(self.includefiles.clone())
    }
}

/// Resolve a toctree directive's content lines against the document set.
///
/// Exact port of `TocTree.parse_content` (`sphinx/directives/other.py:88-179`),
/// **including all four of its `logger.warning` calls** — they come back on
/// [`ResolvedEntries::warnings`] instead of going to a logger, because this
/// runs inside the parser. This is the project's only toctree resolver: the
/// build's user-visible toctree diagnostics are exactly these warnings, so
/// what warns and what resolves can never disagree.
///
/// The one deliberate addition is [`ToctreeWarningKind::PatternError`],
/// which has no Sphinx counterpart (see its doc comment).
///
/// Not ported: `env.note_reread()` on a missing entry (nothing consumes the
/// re-read set yet).
pub fn resolve_entries(input: &ToctreeContent<'_>) -> ResolvedEntries {
    let &ToctreeContent {
        content,
        docname,
        glob,
        reversed,
        line,
        found_docs,
        source_suffixes,
        exclude_patterns,
    } = input;

    // `all_docnames` is consumed as entries claim documents (so a glob never
    // re-lists what an earlier entry named); `frozen` keeps the full set for
    // the existence check. The current document is not a candidate: a
    // toctree entry naming its own document is "nonexisting" to Sphinx.
    let mut all: BTreeSet<&str> = found_docs.iter().map(String::as_str).collect();
    all.extend(VIRTUAL_DOC_NAMES);
    all.remove(docname);
    let frozen = all.clone();

    let mut out = ResolvedEntries::default();
    let mut warnings: Vec<ToctreeWarning> = Vec::new();
    // Takes its sink as an argument rather than capturing it, so it holds no
    // borrow across the loop body that also mutates `out`.
    let warn = |sink: &mut Vec<ToctreeWarning>,
                message: String,
                category: Option<&str>,
                kind: ToctreeWarningKind| {
        sink.push(ToctreeWarning {
            line,
            message,
            category: category.map(str::to_string),
            kind,
        });
    };

    for entry in content {
        if entry.is_empty() {
            continue;
        }
        let explicit = split_explicit_title(entry);
        let url_match = is_url(entry);

        if glob && has_glob_metachars(entry) && explicit.is_none() && !url_match {
            let pattern = docname_join(docname, entry);
            // BTreeSet iteration order == sorted(), as sphinx does.
            let mut matched: Vec<String> = Vec::new();
            let mut pattern_error = None;
            for candidate in all.iter().filter(|d| !VIRTUAL_DOC_NAMES.contains(d)) {
                match matching::pattern_match(candidate, &pattern) {
                    Ok(true) => matched.push((*candidate).to_string()),
                    Ok(false) => {}
                    Err(e) => {
                        pattern_error.get_or_insert_with(|| e.to_string());
                    }
                }
            }
            if let Some(error) = pattern_error {
                warn(
                    &mut warnings,
                    format!(
                        "toctree glob pattern {} is not usable: {error}",
                        py_repr_str(entry)
                    ),
                    None,
                    ToctreeWarningKind::PatternError,
                );
            } else if matched.is_empty() {
                warn(
                    &mut warnings,
                    format!(
                        "toctree glob pattern {} didn't match any documents",
                        py_repr_str(entry)
                    ),
                    // sphinx passes `subtype='empty_glob'` but no `type`, so
                    // no `[...]` suffix is appended.
                    None,
                    ToctreeWarningKind::EmptyGlob,
                );
            }
            for name in matched {
                all.remove(name.as_str());
                out.entries.push((None, name.clone()));
                out.includefiles.push(name);
            }
            continue;
        }

        let (title, reference) = match explicit {
            Some((title, target)) => (Some(title.to_string()), target),
            None => (None, entry.as_str()),
        };
        let mut resolved = reference;
        for suffix in source_suffixes {
            if let Some(stripped) = resolved.strip_suffix(suffix) {
                resolved = stripped;
                break;
            }
        }
        let resolved = docname_join(docname, resolved);

        if url_match || reference == "self" {
            out.entries.push((title, reference.to_string()));
            continue;
        }
        if !frozen.contains(resolved.as_str()) {
            // sphinx matches `exclude_patterns` against `doc2path(ref,
            // base=False)` — the source-relative path, which for a document
            // that does not exist is the docname plus the *first* configured
            // source suffix.
            let path = format!(
                "{resolved}{}",
                source_suffixes.first().copied().unwrap_or_default()
            );
            let (message, category) = if matches_any(&path, exclude_patterns) {
                (
                    "toctree contains reference to excluded document",
                    "toc.excluded",
                )
            } else {
                (
                    "toctree contains reference to nonexisting document",
                    "toc.not_readable",
                )
            };
            warn(
                &mut warnings,
                format!("{message} {}", py_repr_str(&resolved)),
                Some(category),
                ToctreeWarningKind::MissingDocument,
            );
            continue;
        }
        // sphinx warns when the document was already claimed, but appends
        // the entry either way.
        if !all.remove(resolved.as_str()) {
            warn(
                &mut warnings,
                format!("duplicated entry found in toctree: {resolved}"),
                Some("toc.duplicate_entry"),
                ToctreeWarningKind::DuplicateEntry,
            );
        }
        out.entries.push((title, resolved.clone()));
        out.includefiles.push(resolved);
    }

    // `:reversed:` flips the two entry lists only; diagnostics keep the
    // order they were produced in.
    if reversed {
        out.entries.reverse();
        out.includefiles.reverse();
    }
    out.warnings = warnings;
    out
}

/// Sphinx's `Matcher` (`util/matching.py`): does any pattern match?
/// A pattern that fails to compile matches nothing — the caller that cares
/// (glob expansion) checks compilation separately.
fn matches_any(path: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| matching::pattern_match(path, pattern).unwrap_or(false))
}

/// Split `Some Title <target>` into its two halves, as Sphinx's
/// `explicit_title_re` (`^(.+?)\s*<(.*?)>$`, `util/nodes.py`) does: the
/// **first** `<` that leaves a non-empty title wins, and the line must end
/// with `>`. A bare `<foo>` is therefore a literal target named `<foo>`,
/// not an empty-titled reference.
pub fn split_explicit_title(entry: &str) -> Option<(&str, &str)> {
    let entry = entry.strip_suffix('>')?;
    let open = entry.find('<')?;
    if open == 0 {
        return None;
    }
    let title = entry[..open].trim_end();
    if title.is_empty() {
        return None;
    }
    Some((title, &entry[open + 1..]))
}

/// Sphinx `url_re` (`(?P<schema>.+)://.*`, anchored with `.match`): *some*
/// `://` preceded by at least one character — the regex backtracks, so a
/// leading `://` does not rule out a later one satisfying the schema part.
fn is_url(entry: &str) -> bool {
    entry.match_indices("://").any(|(at, _)| at >= 1)
}

/// Sphinx `glob_re` (`.*[*?\[].*`).
fn has_glob_metachars(entry: &str) -> bool {
    entry.contains(['*', '?', '['])
}

/// Sphinx `docname_join` (`util/__init__.py`):
/// `posixpath.normpath(posixpath.join('/' + basedocname, '..', docname))[1:]`
/// — a leading `/` makes the target source-root-relative, anything else is
/// relative to the referencing document's directory, and `.`/`..` segments
/// are normalized.
pub fn docname_join(base_docname: &str, docname: &str) -> String {
    let (base, target) = match docname.strip_prefix('/') {
        Some(stripped) => ("", stripped),
        None => (
            base_docname.rsplit_once('/').map(|(d, _)| d).unwrap_or(""),
            docname,
        ),
    };

    let mut segments: Vec<&str> = Vec::new();
    for seg in base.split('/').chain(target.split('/')) {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

/// `repr()` of a Python `str`: single quotes unless that would need
/// escaping and double quotes wouldn't.
fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

// ---------------------------------------------------------------------------
// 2. build_toc (sphinx/environment/collectors/toctree.py process_doc)
// ---------------------------------------------------------------------------

/// The document's local table of contents (`env.tocs[docname]`) and its
/// entry count (`env.toc_num_entries[docname]`).
///
/// Exact port of `TocTreeCollector.process_doc`'s nested `build_toc`, for
/// the node kinds that exist in the doctree today: sections (title filtered
/// through [`filter_title_children`], anchor `''` for the first entry and
/// `'#' + ids[0]` afterwards), `only` wrappers, and `toctree` nodes copied
/// out of any other element. The `addnodes.desc` branch — object signatures
/// contributing `compact_paragraph[skip_section_number] > reference >
/// literal` entries — is deliberately absent: no directive produces `desc`
/// nodes yet.
///
/// An empty document yields an empty `bullet_list` and 0 entries.
pub fn build_toc(doctree: &Doctree, docname: &str) -> (Node, u32) {
    let mut num_entries = 0u32;
    let toc = build_toc_level(&doctree.root.children, docname, &mut num_entries)
        .unwrap_or_else(|| Node::elem(kinds::BULLET_LIST, Span::ZERO));
    (toc, num_entries)
}

fn build_toc_level(nodes: &[Node], docname: &str, num_entries: &mut u32) -> Option<Node> {
    let mut entries: Vec<Node> = Vec::new();

    for node in nodes {
        // docutils Text nodes aren't Elements; sphinx's isinstance chain
        // skips them entirely.
        if node.kind == kinds::TEXT {
            continue;
        }

        if node.kind == kinds::SECTION {
            // sphinx: `title = sectionnode[0]` — the section's first child.
            let title_children = node
                .children
                .first()
                .map(filter_title_children)
                .unwrap_or_default();
            let anchorname = make_anchor_name(&node.attrs.ids, num_entries);

            let mut reference = Node::elem(kinds::REFERENCE, Span::ZERO);
            reference.set("anchorname", AttrValue::Str(anchorname));
            reference.set("internal", AttrValue::Int(1));
            reference.set("refuri", AttrValue::Str(docname.to_string()));
            reference.children = title_children;

            let mut para = Node::elem(kinds::COMPACT_PARAGRAPH, Span::ZERO);
            para.children.push(reference);
            let mut item = Node::elem(kinds::LIST_ITEM, Span::ZERO);
            item.children.push(para);
            if let Some(sub) = build_toc_level(&node.children, docname, num_entries) {
                item.children.push(sub);
            }
            entries.push(item);
        } else if node.kind == kinds::ONLY {
            // Deferred tag filtering: the entries stay in the toc wrapped in
            // a fresh `only` node carrying the same expression.
            let mut only = Node::elem(kinds::ONLY, Span::ZERO);
            if let Some(expr) = node.get("expr") {
                only.set("expr", expr.clone());
            }
            if let Some(sub) = build_toc_level(&node.children, docname, num_entries) {
                only.children = sub.children;
                entries.push(only);
            }
        } else {
            // Any other element: `findall()` over the whole subtree for
            // toctree nodes to copy into the toc. (sphinx `continue`s on
            // nested sections, which only skips *processing* them — findall
            // still descends, and nothing below a section is a toctree that
            // this walk would otherwise miss.)
            collect_toctree_copies(node, &mut entries);
        }
    }

    if entries.is_empty() {
        return None;
    }
    let mut list = Node::elem(kinds::BULLET_LIST, Span::ZERO);
    list.children = entries;
    Some(list)
}

fn collect_toctree_copies(node: &Node, entries: &mut Vec<Node>) {
    // docutils findall() yields the node itself first, then descendants.
    if node.kind == kinds::TOCTREE {
        entries.push(node.shallow_copy());
    }
    for child in &node.children {
        collect_toctree_copies(child, entries);
    }
}

/// `_make_anchor_name` (`collectors/toctree.py:381`): the very first entry
/// of a document gets the empty anchor (it *is* the page), everything after
/// gets `'#' + ids[0]`.
fn make_anchor_name(ids: &[String], num_entries: &mut u32) -> String {
    let anchor = if *num_entries == 0 {
        String::new()
    } else {
        // sphinx indexes ids[0] unconditionally (an id-less section past the
        // first would raise there); an id-less section yields a bare "#".
        format!("#{}", ids.first().map(String::as_str).unwrap_or(""))
    };
    *num_entries += 1;
    anchor
}

/// `SphinxContentsFilter` (`sphinx/transforms/__init__.py:350`) over
/// docutils' `ContentsFilter`/`TreeCopyVisitor`
/// (`docutils/transforms/parts.py:154`): a copy of the title's children with
/// reference-ish wrappers unwrapped (children kept, wrapper dropped) and
/// footnote/citation references and images dropped whole.
fn filter_title_children(title: &Node) -> Vec<Node> {
    let mut out = Vec::new();
    filter_into(&title.children, &mut out);
    out
}

fn filter_into(children: &[Node], out: &mut Vec<Node>) {
    for child in children {
        match child.kind {
            // SkipNode: the node and its children are dropped. (docutils'
            // base filter would keep an image's `alt` text; sphinx's
            // override drops images outright.)
            kinds::FOOTNOTE_REFERENCE | kinds::CITATION_REFERENCE | kinds::IMAGE => {}
            // SkipDeparture on a visit that never copied the node: the
            // wrapper vanishes, its children land in the enclosing parent.
            kinds::REFERENCE | kinds::TARGET | kinds::PROBLEMATIC | kinds::PENDING_XREF => {
                filter_into(&child.children, out);
            }
            kinds::TEXT => out.push(child.clone()),
            _ => {
                let mut copy = child.shallow_copy();
                filter_into(&child.children, &mut copy.children);
                out.push(copy);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. note_toctree (sphinx/environment/adapters/toctree.py)
// ---------------------------------------------------------------------------

/// Record one `toctree` node's file relations in the environment.
///
/// Port of `note_toctree` (`adapters/toctree.py:32-47`). Note the
/// `setdefault` in `env.toctree_includes.setdefault(docname, []).extend(...)`:
/// the key is created even when the toctree includes nothing.
pub fn note_toctree(env: &mut BuildEnvironment, docname: &str, toctree: &Node) {
    if matches!(toctree.get("glob"), Some(AttrValue::Int(n)) if *n != 0) {
        env.glob_toctrees.insert(docname.to_string());
    }
    if matches!(toctree.get("numbered"), Some(AttrValue::Int(n)) if *n != 0) {
        env.numbered_toctrees.insert(docname.to_string());
    }

    let include_files: &[String] = match toctree.get("includefiles") {
        Some(AttrValue::List(files)) => files,
        _ => &[],
    };
    for include_file in include_files {
        env.files_to_rebuild
            .entry(include_file.clone())
            .or_default()
            .insert(docname.to_string());
    }
    env.toctree_includes
        .entry(docname.to_string())
        .or_default()
        .extend(include_files.iter().cloned());
}

/// Every `toctree` node inside a built toc, in the order
/// [`build_toc`] copied them in — which is the order Sphinx calls
/// `note_toctree` in, since it notes each node at the moment it copies it.
pub fn toctree_copies(toc: &Node) -> Vec<&Node> {
    let mut out = Vec::new();
    fn walk<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if node.kind == kinds::TOCTREE {
            out.push(node);
        }
        for child in &node.children {
            walk(child, out);
        }
    }
    walk(toc, &mut out);
    out
}

// ---------------------------------------------------------------------------
// 4. Whole-project reads of the toctree graph
// ---------------------------------------------------------------------------

/// `[parent, prev, next]` — one document's place in the global document
/// order, as `env.collect_relations()` records it.
pub type Relation = (Option<String>, Option<String>, Option<String>);

/// Every document's `[parent, prev, next]`, from a pre-order walk of the
/// toctree graph rooted at `env.root_doc`.
///
/// Port of `BuildEnvironment.collect_relations`
/// (`environment/__init__.py:778-795`) over [`traverse_toctree`]. `prev`/
/// `next` are the flattened document order, so the first child of a
/// document has that document as its `prev` — Sphinx's chain is linear, not
/// sibling-scoped.
pub fn collect_relations(env: &BuildEnvironment) -> BTreeMap<String, Relation> {
    let order = traverse_toctree(&env.toctree_includes, &env.root_doc);
    let mut relations = BTreeMap::new();

    let mut prev: Option<String> = None;
    for (index, (parent, docname)) in order.iter().enumerate() {
        let next = order.get(index + 1).map(|(_, doc)| doc.clone());
        relations.insert(docname.clone(), (parent.clone(), prev.take(), next));
        prev = Some(docname.clone());
    }
    relations
}

/// Pre-order depth-first walk of `toctree_includes` from `root`, yielding
/// `(parent, docname)` once per document — the first visit wins.
///
/// Port of `_traverse_toctree` (`environment/__init__.py:914-939`) with one
/// deliberate difference: **descent is guarded by the visited set, not just
/// yielding**. Sphinx recurses into every child unconditionally and only
/// filters the *yields*, so a mutual `A -> B -> A` cycle recurses without
/// bound and raises `RecursionError` — a real, oracle-verified sphinx 9.1.0
/// crash (`tests/fixtures/env_differential.json` records `relations: null`
/// for the `toctree_circular` project because of it). For an acyclic graph
/// the two are equivalent: re-descending into an already-visited document
/// can only re-yield documents that were already yielded, and those are
/// filtered out anyway.
///
/// The walk is iterative for the same reason it is guarded: an explicit
/// stack cannot overflow on a deep document tree.
///
/// Sphinx logs `self referenced toctree found. Ignored.` (`toc.circular`)
/// when a document's toctree includes the document itself, and drops that
/// subtree. The drop is ported; the warning is not, because the branch is
/// unreachable in this pipeline — [`resolve_entries`] removes the current
/// document from its own candidate set, so a self-entry never reaches
/// `toctree_includes`; it is reported at parse time as
/// `toctree contains reference to nonexisting document` instead (which is
/// exactly what the oracle records for the `toctree_self_ref` project).
pub fn traverse_toctree(
    toctree_includes: &BTreeMap<String, Vec<String>>,
    root: &str,
) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<(Option<String>, String)> = vec![(None, root.to_string())];

    while let Some((parent, docname)) = stack.pop() {
        if parent.as_deref() == Some(docname.as_str()) {
            continue;
        }
        if !visited.insert(docname.clone()) {
            continue;
        }
        if let Some(children) = toctree_includes.get(&docname) {
            // Reversed, so the explicit stack pops them left-to-right.
            for child in children.iter().rev() {
                stack.push((Some(docname.clone()), child.clone()));
            }
        }
        out.push((parent, docname));
    }
    out
}

/// The chain of toctree parents above `docname`, nearest first, starting
/// with `docname` itself.
///
/// Port of `_get_toctree_ancestors` (`adapters/toctree.py:562-575`). A
/// document with no toctree parent has no ancestors at all — not even
/// itself — and the `d not in ancestors` guard stops the walk on a cycle
/// (which is why this function, unlike `_traverse_toctree`, survives the
/// circular corpus project).
///
/// When a document has several toctree parents the last one wins, matching
/// Sphinx's `parent |= dict.fromkeys(children, p)` over `toctree_includes`.
/// Sphinx iterates that dict in read order and this map iterates in docname
/// order; both resolve to the same "largest parent docname" for the sorted
/// read order every build of this crate performs.
pub fn toctree_ancestors(
    toctree_includes: &BTreeMap<String, Vec<String>>,
    docname: &str,
) -> Vec<String> {
    let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
    for (container, children) in toctree_includes {
        for child in children {
            parent.insert(child.as_str(), container.as_str());
        }
    }

    let mut ancestors: Vec<String> = Vec::new();
    let mut current = docname;
    while let Some(next) = parent.get(current) {
        if ancestors.iter().any(|seen| seen == current) {
            break;
        }
        ancestors.push(current.to_string());
        current = next;
    }
    ancestors
}

/// Whether a [`ConsistencyMessage`] is a warning (counts toward `-W`) or an
/// informational note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsistencyLevel {
    Warning,
    Info,
}

/// One diagnostic from [`check_consistency`], located at a document rather
/// than at a source line (Sphinx passes `location=docname`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsistencyMessage {
    pub docname: String,
    pub message: String,
    /// Sphinx's `type.subtype` category; see
    /// [`crate::error::BuildWarning::category`].
    pub category: Option<String>,
    pub level: ConsistencyLevel,
}

/// Post-read consistency checks over the finished toctree graph.
///
/// Port of `BuildEnvironment.check_consistency`
/// (`environment/__init__.py:797-823`) and the `_check_toc_parents`
/// (`:942-960`) it calls: every document that no toctree reaches gets
/// `document isn't included in any toctree`, and every document reachable
/// from more than one gets an *informational* note (Sphinx uses
/// `logger.info` there, so it must not count toward `-W`).
///
/// Not ported: the `env-check-consistency` event and the per-domain
/// `check_consistency` hooks, neither of which exists yet.
pub fn check_consistency(env: &BuildEnvironment) -> Vec<ConsistencyMessage> {
    let mut messages = Vec::new();

    let included: BTreeSet<&str> = env
        .included
        .values()
        .flatten()
        .map(String::as_str)
        .collect();

    // `all_docs` is a BTreeMap, so this is sphinx's `sorted(self.all_docs)`.
    for docname in env.all_docs.keys() {
        // Reachable from some toctree, the root itself, textually included
        // by another document, or explicitly marked `:orphan:`.
        if env.files_to_rebuild.contains_key(docname)
            || *docname == env.root_doc
            || included.contains(docname.as_str())
            || env
                .metadata
                .get(docname)
                .is_some_and(|meta| meta.contains_key("orphan"))
        {
            continue;
        }
        messages.push(ConsistencyMessage {
            docname: docname.clone(),
            message: "document isn't included in any toctree".to_string(),
            category: Some("toc.not_included".to_string()),
            level: ConsistencyLevel::Warning,
        });
    }

    // The parent list is interpolated verbatim, so its order is visible.
    // Sphinx builds it from `toctree_includes` in read (insertion) order and
    // this map iterates in docname order; both are the same order for the
    // sorted read every build of this crate performs.
    let mut toc_parents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (container, children) in &env.toctree_includes {
        for child in children {
            toc_parents
                .entry(child.as_str())
                .or_default()
                .push(container.as_str());
        }
    }
    for (docname, parents) in toc_parents {
        if parents.len() <= 1 {
            continue;
        }
        // sphinx interpolates the parent list with `%s`, i.e. Python's
        // `str(list)` — a repr of each element inside brackets.
        let list = parents
            .iter()
            .map(|parent| py_repr_str(parent))
            .collect::<Vec<_>>()
            .join(", ");
        let selected = parents.iter().max().expect("len > 1");
        messages.push(ConsistencyMessage {
            docname: docname.to_string(),
            message: format!(
                "document is referenced in multiple toctrees: [{list}], \
                 selecting: {selected} <- {docname}"
            ),
            category: Some("toc.multiple_toc_parents".to_string()),
            level: ConsistencyLevel::Info,
        });
    }

    messages
}

// ---------------------------------------------------------------------------
// TitleCollector (sphinx/environment/collectors/title.py)
// ---------------------------------------------------------------------------

/// `env.titles[docname]`: a fresh `title` node holding the first section
/// title's contents, filtered exactly like a toc entry's.
///
/// Port of `TitleCollector.process_doc` (`collectors/title.py:27`). Its
/// `longtitles` differ only when the document carries a `title` attribute
/// (set by the `title` directive / `<meta>`), which nothing produces yet —
/// so the caller stores this same node under both keys.
pub fn document_title(doctree: &Doctree) -> Node {
    let mut title = Node::elem(kinds::TITLE, Span::ZERO);
    match first_section(&doctree.root) {
        Some(section) => {
            if let Some(first_child) = section.children.first() {
                title.children = filter_title_children(first_child);
            }
        }
        // sphinx: `doctree.get('title', '<no title>')`.
        None => title
            .children
            .push(Node::text_node("<no title>", Span::ZERO)),
    }
    title
}

/// First `section` in document order (docutils `findall(nodes.section)`).
fn first_section(node: &Node) -> Option<&Node> {
    for child in &node.children {
        if child.kind == kinds::SECTION {
            return Some(child);
        }
        if let Some(found) = first_section(child) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst;

    fn docs(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    fn parse(source: &str, docname: &str, found: &BTreeSet<String>) -> Doctree {
        rst::parse_rst(
            source,
            &rst::ParseOptions {
                source_path: "<snippet>".to_string(),
                sphinx: true,
                docname: docname.to_string(),
                exclude_patterns: Vec::new(),
                found_docs: Some(std::sync::Arc::new(found.clone())),
            },
        )
    }

    #[test]
    fn empty_document_yields_empty_bullet_list() {
        let doctree = parse("", "index", &docs(&[]));
        let (toc, n) = build_toc(&doctree, "index");
        assert_eq!(toc.pformat(), "<bullet_list>\n");
        assert_eq!(n, 0);
    }

    #[test]
    fn first_entry_has_empty_anchor_and_later_entries_use_ids() {
        let doctree = parse("A\n=\n\nSub\n---\n\nText.\n", "a", &docs(&["a"]));
        let (toc, n) = build_toc(&doctree, "a");
        assert_eq!(n, 2);
        assert_eq!(
            toc.pformat(),
            concat!(
                "<bullet_list>\n",
                "    <list_item>\n",
                "        <compact_paragraph>\n",
                "            <reference anchorname=\"\" internal=\"1\" refuri=\"a\">\n",
                "                A\n",
                "        <bullet_list>\n",
                "            <list_item>\n",
                "                <compact_paragraph>\n",
                "                    <reference anchorname=\"#sub\" internal=\"1\" refuri=\"a\">\n",
                "                        Sub\n",
            )
        );
    }

    #[test]
    fn title_inline_markup_survives_but_references_are_unwrapped() {
        let doctree = parse(
            "A `link <https://x/>`_ and *em* [#f]_\n=====================================\n\n.. [#f] note\n",
            "a",
            &docs(&["a"]),
        );
        let (toc, _) = build_toc(&doctree, "a");
        // reference wrapper dropped (its text kept), footnote_reference
        // dropped whole, emphasis kept.
        assert!(toc.pformat().contains("<emphasis>\n"), "{}", toc.pformat());
        assert!(!toc.pformat().contains("<reference anchorname=\"\" internal=\"1\" refuri=\"a\">\n                A\n                <reference"));
        assert!(
            !toc.pformat().contains("footnote_reference"),
            "{}",
            toc.pformat()
        );
        assert!(toc.pformat().contains("link"), "{}", toc.pformat());
    }

    /// An `only` node's toc entries are re-wrapped in a fresh `only`
    /// carrying the same expression, so the tag filtering can happen at
    /// render time. (A section inside `.. only::` would be the other half of
    /// this branch, but the parser rejects section titles in nested content,
    /// so a toctree is what reaches the collector today.)
    #[test]
    fn only_directive_wraps_its_entries() {
        let doctree = parse(
            ".. only:: html\n\n   .. toctree::\n\n      a\n",
            "index",
            &docs(&["index", "a"]),
        );
        let (toc, n) = build_toc(&doctree, "index");
        assert_eq!(n, 0, "a copied toctree is not a numbered entry");
        assert!(
            toc.pformat()
                .starts_with("<bullet_list>\n    <only expr=\"html\">\n        <toctree "),
            "{}",
            toc.pformat()
        );

        let mut env = BuildEnvironment::default();
        for node in toctree_copies(&toc) {
            note_toctree(&mut env, "index", node);
        }
        assert_eq!(
            env.toctree_includes.get("index"),
            Some(&vec!["a".to_string()]),
            "a toctree inside `only` still contributes to the graph"
        );
    }

    #[test]
    fn toctree_node_is_copied_into_the_toc_and_noted() {
        let found = docs(&["index", "a", "b"]);
        let doctree = parse(
            "Index\n=====\n\n.. toctree::\n\n   a\n   b\n",
            "index",
            &found,
        );
        let (toc, n) = build_toc(&doctree, "index");
        assert_eq!(n, 1, "the copied toctree is not an entry");

        let mut env = BuildEnvironment::default();
        for node in toctree_copies(&toc) {
            note_toctree(&mut env, "index", node);
        }
        assert_eq!(
            env.toctree_includes.get("index"),
            Some(&vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            env.files_to_rebuild.get("a"),
            Some(&BTreeSet::from(["index".to_string()]))
        );
        assert!(env.glob_toctrees.is_empty());
        assert!(env.numbered_toctrees.is_empty());
    }

    #[test]
    fn note_toctree_creates_the_includes_key_even_when_empty() {
        // sphinx `setdefault(docname, []).extend([])`.
        let mut env = BuildEnvironment::default();
        let mut toctree = Node::elem(kinds::TOCTREE, Span::ZERO);
        toctree.set("includefiles", AttrValue::List(vec![]));
        note_toctree(&mut env, "index", &toctree);
        assert_eq!(env.toctree_includes.get("index"), Some(&Vec::new()));
        assert!(env.files_to_rebuild.is_empty());
    }

    #[test]
    fn glob_and_numbered_flags_reach_the_environment() {
        let mut env = BuildEnvironment::default();
        let mut toctree = Node::elem(kinds::TOCTREE, Span::ZERO);
        toctree.set("glob", AttrValue::Int(1));
        toctree.set("numbered", AttrValue::Int(999));
        toctree.set("includefiles", AttrValue::List(vec!["a".into()]));
        note_toctree(&mut env, "index", &toctree);
        assert!(env.glob_toctrees.contains("index"));
        assert!(env.numbered_toctrees.contains("index"));
    }

    /// Every `resolve_entries` input a test needs, defaulted.
    fn content<'a>(
        lines: &'a [String],
        docname: &'a str,
        found: &'a BTreeSet<String>,
    ) -> ToctreeContent<'a> {
        ToctreeContent {
            content: lines,
            docname,
            glob: false,
            reversed: false,
            line: 1,
            found_docs: found,
            source_suffixes: &[".rst"],
            exclude_patterns: &[],
        }
    }

    fn lines(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn entries_resolve_relative_absolute_and_self_targets() {
        let found = docs(&["index", "sub/b", "sub/c", "a"]);
        let entries = lines(&[
            "c",
            "/a",
            "self",
            "https://example.invalid/x",
            "missing",
            "sub/b",
        ]);
        let resolved = resolve_entries(&content(&entries, "sub/b", &found));
        assert_eq!(
            resolved.entries,
            vec![
                (None, "sub/c".to_string()),
                (None, "a".to_string()),
                (None, "self".to_string()),
                (None, "https://example.invalid/x".to_string()),
            ],
            "missing docs drop; `sub/b` is the current document, which is not \
             a candidate for its own toctree"
        );
        assert_eq!(
            resolved.includefiles,
            vec!["sub/c".to_string(), "a".to_string()]
        );
        assert_eq!(
            resolved
                .warnings
                .iter()
                .map(|w| w.message.as_str())
                .collect::<Vec<_>>(),
            vec![
                "toctree contains reference to nonexisting document 'sub/missing'",
                "toctree contains reference to nonexisting document 'sub/sub/b'",
            ],
            "sphinx reports the *joined* docname, so a document-relative miss \
             names the directory it was resolved against"
        );
    }

    /// The oracle's `toctree_self_ref` project: a toctree entry naming its
    /// own document is not a `self referenced toctree` — parse_content
    /// removes the current document from the candidate set first, so it
    /// comes out as an ordinary missing-document warning, and never reaches
    /// `toctree_includes`.
    #[test]
    fn an_entry_naming_its_own_document_is_reported_as_nonexisting() {
        let found = docs(&["index", "a"]);
        let entries = lines(&["index", "a"]);
        let resolved = resolve_entries(&content(&entries, "index", &found));
        assert_eq!(resolved.includefiles, vec!["a".to_string()]);
        assert_eq!(
            resolved
                .warnings
                .iter()
                .map(|w| (w.message.as_str(), w.category.as_deref()))
                .collect::<Vec<_>>(),
            vec![(
                "toctree contains reference to nonexisting document 'index'",
                Some("toc.not_readable")
            )]
        );
    }

    /// Every diagnostic is located at the `.. toctree::` marker, not at the
    /// offending entry: sphinx passes `location=toctree`, the directive node.
    #[test]
    fn diagnostics_are_located_at_the_directive() {
        let found = docs(&["index"]);
        let entries = lines(&["missing"]);
        let resolved = resolve_entries(&ToctreeContent {
            line: 12,
            ..content(&entries, "index", &found)
        });
        assert_eq!(resolved.warnings.len(), 1);
        assert_eq!(resolved.warnings[0].line, 12);
        assert_eq!(
            resolved.warnings[0].category.as_deref(),
            Some("toc.not_readable")
        );
    }

    /// A missing target that `exclude_patterns` covers is reported as
    /// *excluded* rather than *nonexisting* (`directives/other.py:150-153`),
    /// matched against `doc2path(ref, base=False)` — the docname plus the
    /// first source suffix.
    #[test]
    fn excluded_targets_get_their_own_message() {
        let found = docs(&["index"]);
        let excluded = vec!["drafts/*".to_string()];
        let entries = lines(&["drafts/wip"]);
        let resolved = resolve_entries(&ToctreeContent {
            exclude_patterns: &excluded,
            ..content(&entries, "index", &found)
        });
        assert_eq!(
            resolved.warnings[0].message,
            "toctree contains reference to excluded document 'drafts/wip'"
        );
        assert_eq!(
            resolved.warnings[0].category.as_deref(),
            Some("toc.excluded")
        );
    }

    #[test]
    fn a_document_claimed_twice_warns_but_is_still_listed() {
        let found = docs(&["index", "a"]);
        let entries = lines(&["a", "a"]);
        let resolved = resolve_entries(&content(&entries, "index", &found));
        assert_eq!(
            resolved.includefiles,
            vec!["a".to_string(), "a".to_string()],
            "sphinx appends the duplicate either way"
        );
        assert_eq!(
            resolved.warnings[0].message,
            "duplicated entry found in toctree: a"
        );
        assert_eq!(
            resolved.warnings[0].category.as_deref(),
            Some("toc.duplicate_entry")
        );
    }

    /// `_virtual_doc_names` is a dict, and `parse_content` unions
    /// `frozenset(...)` of it into the candidate set — so the virtual names
    /// are its **keys**. `modindex` is the one an author writes in a
    /// toctree; `py-modindex` is that key's value (the docname the module
    /// index is finally written to) and is not itself a virtual name.
    #[test]
    fn the_virtual_docnames_are_the_dict_keys_not_its_values() {
        let found = docs(&["index"]);
        let entries = lines(&["genindex", "modindex", "search"]);
        let resolved = resolve_entries(&content(&entries, "index", &found));
        assert_eq!(
            resolved.includefiles,
            vec![
                "genindex".to_string(),
                "modindex".to_string(),
                "search".to_string()
            ]
        );
        assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);

        let entries = lines(&["py-modindex"]);
        let resolved = resolve_entries(&content(&entries, "index", &found));
        assert!(resolved.includefiles.is_empty());
        assert_eq!(
            resolved
                .warnings
                .iter()
                .map(|w| w.message.as_str())
                .collect::<Vec<_>>(),
            vec!["toctree contains reference to nonexisting document 'py-modindex'"]
        );
    }

    #[test]
    fn glob_entries_expand_sorted_and_skip_virtual_docs() {
        let found = docs(&["index", "pages/a", "pages/b"]);
        let entries = lines(&["pages/*"]);
        let resolved = resolve_entries(&ToctreeContent {
            glob: true,
            ..content(&entries, "index", &found)
        });
        assert_eq!(
            resolved.includefiles,
            vec!["pages/a".to_string(), "pages/b".to_string()]
        );
        assert_eq!(
            resolved.entries_attr(),
            AttrValue::List(vec![
                "(None, 'pages/a')".to_string(),
                "(None, 'pages/b')".to_string()
            ])
        );
        assert!(resolved.warnings.is_empty());
    }

    /// A dead glob warns with the entry as authored (sphinx `%r` of `entry`,
    /// not of the joined pattern), and — because sphinx passes no `type` —
    /// carries no `[type.subtype]` category.
    #[test]
    fn a_glob_that_matches_nothing_warns() {
        let found = docs(&["index", "pages/a"]);
        let entries = lines(&["missing*"]);
        let resolved = resolve_entries(&ToctreeContent {
            glob: true,
            ..content(&entries, "index", &found)
        });
        assert!(resolved.includefiles.is_empty());
        assert_eq!(
            resolved.warnings,
            vec![ToctreeWarning {
                line: 1,
                message: "toctree glob pattern 'missing*' didn't match any documents".to_string(),
                category: None,
                kind: ToctreeWarningKind::EmptyGlob,
            }]
        );
    }

    /// An uncompilable pattern must not masquerade as "matched nothing":
    /// that would hide the bug behind a plausible Sphinx warning.
    #[test]
    fn an_uncompilable_glob_pattern_is_reported_as_such() {
        let found = docs(&["index", "a"]);
        // `[z-a]` is a character class with a reversed range: our pattern
        // translation hands it to `regex`, which refuses it.
        let entries = lines(&["[z-a]*"]);
        let resolved = resolve_entries(&ToctreeContent {
            glob: true,
            ..content(&entries, "index", &found)
        });
        assert_eq!(resolved.warnings.len(), 1, "{:?}", resolved.warnings);
        assert_eq!(resolved.warnings[0].kind, ToctreeWarningKind::PatternError);
        assert!(
            resolved.warnings[0]
                .message
                .starts_with("toctree glob pattern '[z-a]*' is not usable:"),
            "{}",
            resolved.warnings[0].message
        );
    }

    #[test]
    fn explicit_titles_and_suffixes() {
        let found = docs(&["index", "other"]);
        let entries = lines(&["Linked <other.rst>", "<foo>"]);
        let resolved = resolve_entries(&content(&entries, "index", &found));
        assert_eq!(
            resolved.entries,
            vec![(Some("Linked".to_string()), "other".to_string())],
            "`<foo>` is a literal (missing) target, not an empty title"
        );
        assert_eq!(
            resolved.entries_attr(),
            AttrValue::List(vec!["('Linked', 'other')".to_string()])
        );
        assert_eq!(
            resolved.warnings[0].message,
            "toctree contains reference to nonexisting document '<foo>'"
        );
    }

    #[test]
    fn reversed_flips_both_lists() {
        let found = docs(&["index", "a", "b"]);
        let entries = lines(&["a", "b"]);
        let resolved = resolve_entries(&ToctreeContent {
            reversed: true,
            ..content(&entries, "index", &found)
        });
        assert_eq!(
            resolved.includefiles,
            vec!["b".to_string(), "a".to_string()]
        );
    }

    /// `url_re` is `(?P<schema>.+)://.*` matched (not fullmatched) from the
    /// start, and `.+` backtracks: only a `://` at offset 0 with no other
    /// occurrence fails to be a URL.
    #[test]
    fn url_detection_matches_the_backtracking_regex() {
        assert!(is_url("https://example.invalid/x"));
        assert!(is_url("a://b"));
        assert!(!is_url("://leading"));
        assert!(
            is_url("://a://b"),
            "the second `://` has a non-empty schema before it"
        );
        assert!(!is_url("plain/docname"));
    }

    #[test]
    fn virtual_docnames_are_valid_entries() {
        let found = docs(&["index"]);
        let entries = lines(&["genindex"]);
        let resolved = resolve_entries(&content(&entries, "index", &found));
        assert_eq!(resolved.includefiles, vec!["genindex".to_string()]);
        assert!(resolved.warnings.is_empty());
    }

    // -----------------------------------------------------------------
    // Whole-project reads of the graph
    // -----------------------------------------------------------------

    /// An environment holding nothing but a toctree graph and a document
    /// set, which is all the graph reads look at.
    fn graph(root: &str, includes: &[(&str, &[&str])]) -> BuildEnvironment {
        let mut env = BuildEnvironment {
            root_doc: root.to_string(),
            ..Default::default()
        };
        for (container, children) in includes {
            let children: Vec<String> = children.iter().map(|c| (*c).to_string()).collect();
            for child in &children {
                env.files_to_rebuild
                    .entry(child.clone())
                    .or_default()
                    .insert((*container).to_string());
            }
            env.toctree_includes
                .insert((*container).to_string(), children);
        }
        for docname in env
            .toctree_includes
            .keys()
            .cloned()
            .chain(env.files_to_rebuild.keys().cloned())
            .collect::<Vec<_>>()
        {
            env.all_docs.insert(docname, 0);
        }
        env.all_docs.insert(root.to_string(), 0);
        env
    }

    fn relation(env: &BuildEnvironment, docname: &str) -> (String, String, String) {
        let show = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".to_string());
        let (parent, prev, next) = collect_relations(env)[docname].clone();
        (show(&parent), show(&prev), show(&next))
    }

    /// The oracle's `toctree_nested` shape: `index -> [a, b]`, `a -> [a1,
    /// a2]`. `prev`/`next` chain the flattened pre-order, so `a1`'s `prev`
    /// is its own parent `a`, not a sibling.
    #[test]
    fn relations_chain_the_preorder_walk() {
        let env = graph("index", &[("index", &["a", "b"]), ("a", &["a1", "a2"])]);
        assert_eq!(
            relation(&env, "index"),
            ("-".into(), "-".into(), "a".into())
        );
        assert_eq!(
            relation(&env, "a"),
            ("index".into(), "index".into(), "a1".into())
        );
        assert_eq!(relation(&env, "a1"), ("a".into(), "a".into(), "a2".into()));
        assert_eq!(relation(&env, "a2"), ("a".into(), "a1".into(), "b".into()));
        assert_eq!(
            relation(&env, "b"),
            ("index".into(), "a2".into(), "-".into())
        );
    }

    /// The oracle's `toctree_multi_parent` shape: `c` is reachable from
    /// both `a` and `b`, and the *first* visit — depth-first through `a` —
    /// is the one that sets its parent.
    #[test]
    fn a_document_with_two_parents_keeps_the_first_visit() {
        let env = graph(
            "index",
            &[("index", &["a", "b"]), ("a", &["c"]), ("b", &["c"])],
        );
        assert_eq!(relation(&env, "c"), ("a".into(), "a".into(), "b".into()));
        assert_eq!(
            relation(&env, "b"),
            ("index".into(), "c".into(), "-".into())
        );
    }

    #[test]
    fn a_project_without_toctrees_relates_only_its_root() {
        let env = graph("index", &[]);
        assert_eq!(
            relation(&env, "index"),
            ("-".into(), "-".into(), "-".into())
        );
        assert_eq!(collect_relations(&env).len(), 1);
    }

    /// Sphinx 9.1.0 raises `RecursionError` here (its `traversed` set filters
    /// yields but never guards descent). The port must terminate and answer.
    #[test]
    fn a_mutual_cycle_terminates_instead_of_recursing_forever() {
        let env = graph("index", &[("index", &["a"]), ("a", &["b"]), ("b", &["a"])]);
        let relations = collect_relations(&env);
        assert_eq!(
            relations.keys().collect::<Vec<_>>(),
            vec!["a", "b", "index"],
            "every document is still reached exactly once"
        );
        assert_eq!(relation(&env, "b"), ("a".into(), "a".into(), "-".into()));
    }

    /// A document whose own toctree lists it (only reachable from a stale
    /// or hand-built environment — [`resolve_entries`] never produces it):
    /// sphinx drops that subtree, and so does this.
    #[test]
    fn a_self_parenting_toctree_drops_its_subtree() {
        let env = graph("index", &[("index", &["a"]), ("a", &["a"])]);
        let relations = collect_relations(&env);
        assert_eq!(relations.keys().collect::<Vec<_>>(), vec!["a", "index"]);
    }

    #[test]
    fn ancestors_walk_up_to_the_root_and_stop_on_cycles() {
        let includes = graph("index", &[("index", &["a"]), ("a", &["b"])]).toctree_includes;
        assert_eq!(toctree_ancestors(&includes, "b"), vec!["b", "a"]);
        assert_eq!(
            toctree_ancestors(&includes, "index"),
            Vec::<String>::new(),
            "a document with no toctree parent has no ancestors, not even itself"
        );

        let cyclic = graph("index", &[("a", &["b"]), ("b", &["a"])]).toctree_includes;
        assert_eq!(toctree_ancestors(&cyclic, "a"), vec!["a", "b"]);
    }

    #[test]
    fn documents_no_toctree_reaches_are_reported_as_orphans() {
        let mut env = graph("index", &[("index", &["a"])]);
        env.all_docs.insert("stray".to_string(), 0);
        env.all_docs.insert("textually_included".to_string(), 0);
        env.all_docs.insert("marked".to_string(), 0);
        env.included.insert(
            "a".to_string(),
            BTreeSet::from(["textually_included".to_string()]),
        );
        env.metadata.insert(
            "marked".to_string(),
            BTreeMap::from([("orphan".to_string(), String::new())]),
        );

        let messages = check_consistency(&env);
        assert_eq!(
            messages
                .iter()
                .map(|m| (m.docname.as_str(), m.level))
                .collect::<Vec<_>>(),
            vec![("stray", ConsistencyLevel::Warning)],
            "the root, toctree'd, textually included and `:orphan:` \
             documents are all exempt; {messages:?}"
        );
        assert_eq!(
            messages[0].message,
            "document isn't included in any toctree"
        );
        assert_eq!(messages[0].category.as_deref(), Some("toc.not_included"));
    }

    /// Several toctree parents is an *info*, never a warning: sphinx uses
    /// `logger.info`, so it must not be able to fail a `-W` build.
    #[test]
    fn several_toctree_parents_is_informational() {
        let env = graph(
            "index",
            &[("index", &["a", "b"]), ("a", &["c"]), ("b", &["c"])],
        );
        let messages = check_consistency(&env);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].level, ConsistencyLevel::Info);
        assert_eq!(messages[0].docname, "c");
        assert_eq!(
            messages[0].message,
            "document is referenced in multiple toctrees: ['a', 'b'], selecting: b <- c"
        );
    }

    #[test]
    fn py_repr_quoting_matches_python() {
        assert_eq!(py_repr_str("a"), "'a'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str("say \"hi\""), "'say \"hi\"'");
        assert_eq!(py_repr_str("both ' and \""), "'both \\' and \"'");
        assert_eq!(py_repr_str("a\\b"), "'a\\\\b'");
        assert_eq!(py_repr_str("a\nb"), "'a\\nb'");
    }

    #[test]
    fn docname_join_normalizes() {
        assert_eq!(docname_join("sub/b", "c"), "sub/c");
        assert_eq!(docname_join("sub/b", "/a"), "a");
        assert_eq!(docname_join("sub/b", "../a"), "a");
        assert_eq!(docname_join("index", "a"), "a");
    }

    #[test]
    fn document_title_filters_like_a_toc_entry() {
        let doctree = parse("A *b* c\n=======\n\nText.\n", "a", &docs(&["a"]));
        assert_eq!(
            document_title(&doctree).pformat(),
            "<title>\n    A \n    <emphasis>\n        b\n     c\n"
        );
    }

    #[test]
    fn document_title_without_a_section_says_no_title() {
        let doctree = parse("Just a paragraph.\n", "a", &docs(&["a"]));
        assert_eq!(
            document_title(&doctree).pformat(),
            "<title>\n    <no title>\n"
        );
    }
}
