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
//! 5. [`resolve_toctree`] — `_resolve_toctree` (`adapters/toctree.py:119`):
//!    the write-phase expansion of a `toctree` node into the entry tree
//!    Sphinx writes in its place, which [`resolve_body_toctrees`] does for
//!    every toctree of a document the resolve pass writes
//!    (`get_and_resolve_doctree`).
//!
//! [`document_title`] is the neighbouring `TitleCollector.process_doc`
//! (`collectors/title.py:27`), which shares this module's
//! `SphinxContentsFilter` port.
//!
//! NOT here (later wave-4 tasks): `assign_section_numbers` /
//! `assign_figure_numbers` (they write `toc_secnumbers`/`toc_fignumbers` and
//! stamp `secnumber` onto the references this module builds).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::diagnostics::{Category, Diagnostic, DiagnosticSink, Location, Severity};
use crate::doctree::{kinds, AttrValue, Doctree, Node, Span};
use crate::env::numbers::clean_astext;
use crate::env::resolve::{take_basic_atts, update_basic_atts};
use crate::env::BuildEnvironment;
use crate::matching;
use crate::rst::LogLocation;
use crate::transforms::tags::Tags;
use crate::utils::py_repr_str;

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
    /// Source-table index of the `.. toctree::` marker's line (the node's
    /// `source`): every diagnostic below is stamped with it.
    pub source: u16,
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
    /// Source-table index of the `.. toctree::` directive's line: a
    /// toctree written inside an included file warns against THAT file
    /// (`location=toctree` is the node, whose `source` is the included
    /// file's). Deliberately not `#[serde(default)]` — the warning rides
    /// the document cache, and a pre-provenance record decoding with
    /// source 0 would name the includer again (cache-shape rule,
    /// [`crate::rst::RegistryExport::program_options`]).
    pub source: u16,
    /// 1-based line of the `.. toctree::` directive (Sphinx's
    /// `location=toctree`), within `source`.
    pub line: u32,
    /// The message, formatted exactly as Sphinx formats it.
    pub message: String,
    /// Sphinx's `type.subtype` category, or `None` where Sphinx logs the
    /// warning without a `type` (see [`crate::error::BuildWarning::category`]).
    pub category: Option<String>,
    pub kind: ToctreeWarningKind,
    /// The parse's record sequence number: [`resolve_entries`] leaves it 0
    /// and the parser stamps it as the directive logs the warning
    /// ([`crate::transforms::state::DocumentState::draw_seq`]). Not
    /// `#[serde(default)]` (cache-shape rule).
    pub seq: u32,
}

impl ToctreeWarning {
    /// The record Sphinx logs, `location=toctree`: a `WARNING` with the
    /// warning's category, located at the directive's node
    /// ([`LogLocation::node_warning`]).
    pub fn diagnostic(&self, sources: &[String], srcdir: &Path) -> Diagnostic {
        let category = self
            .category
            .as_deref()
            .map_or(Category::None, Category::parse);
        LogLocation::node_warning(
            sources,
            srcdir,
            self.source,
            self.line,
            self.message.clone(),
            category,
        )
    }
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
        source,
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
            source,
            line,
            message,
            category: category.map(str::to_string),
            kind,
            seq: 0,
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

/// Sphinx's `Matcher` (`util/matching.py:69-85`): does any pattern match?
/// Like `Matcher`, a pattern starting `**/` also matches without that
/// prefix, so `**/x.rst` matches a top-level `x.rst`. A pattern that fails
/// to compile matches nothing — the caller that cares (glob expansion)
/// checks compilation separately.
fn matches_any(path: &str, patterns: &[String]) -> bool {
    let expanded = patterns
        .iter()
        .filter_map(|pattern| pattern.strip_prefix("**/"));
    patterns
        .iter()
        .map(String::as_str)
        .chain(expanded)
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

// ---------------------------------------------------------------------------
// 2. build_toc (sphinx/environment/collectors/toctree.py process_doc)
// ---------------------------------------------------------------------------

/// The document's local table of contents (`env.tocs[docname]`) and its
/// entry count (`env.toc_num_entries[docname]`).
///
/// Exact port of `TocTreeCollector.process_doc`'s nested `build_toc`:
/// sections (title filtered through [`filter_title_children`], anchor `''`
/// for the first entry and `'#' + ids[0]` afterwards), `only` wrappers, and
/// — inside any other element — `toctree` copies plus the object signatures
/// of `desc` nodes, which contribute
/// `compact_paragraph[skip_section_number] > reference > literal` entries.
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
            // `toctree` nodes to copy into the toc and `desc` nodes whose
            // signatures become object entries. (sphinx `continue`s on
            // nested sections, which only skips *processing* them — findall
            // still descends.)
            collect_body_entries(node, docname, num_entries, &mut entries, &[]);
        }
    }

    if entries.is_empty() {
        return None;
    }
    let mut list = Node::elem(kinds::BULLET_LIST, Span::ZERO);
    list.children = entries;
    Some(list)
}

/// The `elif isinstance(sectionnode, nodes.Element)` arm of `build_toc`
/// (`collectors/toctree.py:112-181`), in document order.
///
/// Sphinx keeps a `memo_parents` map from each `desc` to the `list_item` its
/// last signature produced, and attaches a nested description's entries to
/// the nearest ancestor `desc` that has one. That "nearest memo'd ancestor"
/// is exactly what recursion already tracks, so it rides along as `path`:
/// the chain of indices from `entries` down to the list_item currently
/// standing in for `memo_parents`. `toctree` copies ignore it — sphinx
/// appends those to the section-level `entries` wherever it finds them.
fn collect_body_entries(
    node: &Node,
    docname: &str,
    num_entries: &mut u32,
    entries: &mut Vec<Node>,
    path: &[usize],
) {
    if node.kind == kinds::TEXT {
        return;
    }
    if node.kind == kinds::TOCTREE {
        entries.push(node.shallow_copy());
        return;
    }
    if node.kind == "desc" {
        // "Save the latest desc_signature as the one we put sub entries in."
        let mut child_path: Option<Vec<usize>> = None;
        for signature in node.children.iter().filter(|c| c.kind == "desc_signature") {
            let Some(entry) = object_toc_entry(node, signature, docname, num_entries) else {
                continue;
            };
            let target = resolve_attach_point(entries, path);
            target.push(entry);
            let mut deeper = path.to_vec();
            deeper.push(target.len() - 1);
            child_path = Some(deeper);
        }
        let child_path = child_path.unwrap_or_else(|| path.to_vec());
        for child in &node.children {
            collect_body_entries(child, docname, num_entries, entries, &child_path);
        }
        return;
    }
    for child in &node.children {
        collect_body_entries(child, docname, num_entries, entries, path);
    }
}

/// Walk `path` (a chain of list_item indices) down to the entry list a
/// nested object entry belongs in, creating the `bullet_list` each step
/// needs — sphinx's `if isinstance(root_entry[-1], bullet_list): ... else:
/// root_entry.append(bullet_list(...))` (`:167-175`). An empty path is the
/// section level itself.
fn resolve_attach_point<'a>(entries: &'a mut Vec<Node>, path: &[usize]) -> &'a mut Vec<Node> {
    let mut current = entries;
    for &index in path {
        let item = &mut current[index];
        if item.children.last().map(|c| c.kind) != Some(kinds::BULLET_LIST) {
            item.children
                .push(Node::elem(kinds::BULLET_LIST, Span::ZERO));
        }
        current = &mut item
            .children
            .last_mut()
            .expect("a bullet_list was just ensured")
            .children;
    }
    current
}

/// One object signature's toc entry (`collectors/toctree.py:129-157`), or
/// `None` for the three skips: no `_toc_name` (the domain opted out of toc
/// entries for this objtype), `:no-contents-entry:` on the description, and
/// no ids (`:no-index:`).
fn object_toc_entry(
    desc: &Node,
    signature: &Node,
    docname: &str,
    num_entries: &mut u32,
) -> Option<Node> {
    let toc_name = match signature.get("_toc_name") {
        Some(AttrValue::Str(name)) if !name.is_empty() => name.clone(),
        _ => return None,
    };
    if matches!(desc.get("no-contents-entry"), Some(AttrValue::Int(1))) {
        return None;
    }
    if signature.attrs.ids.is_empty() {
        return None;
    }
    let anchorname = make_anchor_name(&signature.attrs.ids, num_entries);

    let mut literal = Node::elem(kinds::LITERAL, Span::ZERO);
    literal.children.push(Node::text_node(toc_name, Span::ZERO));
    let mut reference = Node::elem(kinds::REFERENCE, Span::ZERO);
    reference.set("anchorname", AttrValue::Str(anchorname));
    reference.set("internal", AttrValue::Int(1));
    reference.set("refuri", AttrValue::Str(docname.to_string()));
    reference.children.push(literal);
    let mut para = Node::elem(kinds::COMPACT_PARAGRAPH, Span::ZERO);
    para.set("skip_section_number", AttrValue::Int(1));
    para.children.push(reference);
    let mut item = Node::elem(kinds::LIST_ITEM, Span::ZERO);
    item.children.push(para);
    Some(item)
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
/// The per-domain `check_consistency` hooks Sphinx runs after these, of
/// which only the citation domain's warns, are the builder's next step
/// ([`crate::env::citation::check_consistency`]); the
/// `env-check-consistency` event, which no extension here listens to, is
/// not ported.
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
/// title's contents, filtered exactly like a toc entry's, or, in a
/// document without a section, the document's `title` attribute (the
/// `title` directive's) or `<no title>`.
///
/// Port of `TitleCollector.process_doc` (`collectors/title.py:38-59`);
/// [`document_longtitle`] is its other half.
pub fn document_title(doctree: &Doctree) -> Node {
    let mut title = Node::elem(kinds::TITLE, Span::ZERO);
    match first_section(&doctree.root) {
        Some(section) => {
            if let Some(first_child) = section.children.first() {
                title.children = filter_title_children(first_child);
            }
        }
        // sphinx: `doctree.get('title', '<no title>')`.
        None => {
            let text = title_attribute(doctree).unwrap_or("<no title>");
            title.children.push(Node::text_node(text, Span::ZERO));
        }
    }
    title
}

/// `env.longtitles[docname]` when it is not the document's title
/// ([`document_title`]): a fresh `title` node holding the document's
/// `title` attribute as text, which the `title` directive sets
/// (`collectors/title.py:45-47`). `None` without the attribute.
pub fn document_longtitle(doctree: &Doctree) -> Option<Node> {
    let text = title_attribute(doctree)?;
    let mut title = Node::elem(kinds::TITLE, Span::ZERO);
    title.children.push(Node::text_node(text, Span::ZERO));
    Some(title)
}

/// The document's `title` attribute (`'title' in doctree`).
fn title_attribute(doctree: &Doctree) -> Option<&str> {
    match doctree.root.get("title") {
        Some(AttrValue::Str(text)) => Some(text),
        _ => None,
    }
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

// ---------------------------------------------------------------------------
// 5. _resolve_toctree (sphinx/environment/adapters/toctree.py:119-559)
// ---------------------------------------------------------------------------

/// The docname and section name each virtual toctree target resolves to:
/// `StandardDomain._virtual_doc_names` (`domains/std/__init__.py:784-788`),
/// values included this time.
const VIRTUAL_DOCS: [(&str, &str, &str); 3] = [
    ("genindex", "genindex", "Index"),
    ("modindex", "py-modindex", "Module Index"),
    ("search", "search", "Search Page"),
];

/// The `(title, ref)` pairs of a `toctree` node's `entries` attribute, read
/// back from the Python tuple reprs [`ResolvedEntries::entries_attr`] stores
/// (`(None, 'intro')`, `('Custom', 'api')`).
pub(crate) fn toctree_entries(toctree: &Node) -> Vec<(Option<String>, String)> {
    entries_of(toctree.get("entries"))
}

/// [`toctree_entries`] for the value `entries` of a toctree's `entries`
/// attribute, wherever the node lives (a read transform's arena too).
pub(crate) fn entries_of(entries: Option<&AttrValue>) -> Vec<(Option<String>, String)> {
    let Some(AttrValue::List(items)) = entries else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| parse_entry_repr(item))
        .collect()
}

/// One `(None, 'ref')` / `('title', 'ref')` tuple repr.
fn parse_entry_repr(item: &str) -> Option<(Option<String>, String)> {
    let inner = item.strip_prefix('(')?.strip_suffix(')')?;
    let (title, rest) = match inner.strip_prefix("None") {
        Some(rest) => (None, rest),
        None => {
            let (title, rest) = parse_py_str(inner)?;
            (Some(title), rest)
        }
    };
    let (target, rest) = parse_py_str(rest.strip_prefix(", ")?)?;
    rest.is_empty().then_some((title, target))
}

/// Inverse of [`py_repr_str`]: the CPython string literal `s` starts with,
/// decoded, plus whatever follows it.
fn parse_py_str(s: &str) -> Option<(String, &str)> {
    let mut chars = s.char_indices();
    let (_, quote) = chars.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut out = String::new();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => {
                let (_, escaped) = chars.next()?;
                let hex_digits = match escaped {
                    'n' => {
                        out.push('\n');
                        continue;
                    }
                    'r' => {
                        out.push('\r');
                        continue;
                    }
                    't' => {
                        out.push('\t');
                        continue;
                    }
                    'x' => 2,
                    'u' => 4,
                    'U' => 8,
                    other => {
                        out.push(other);
                        continue;
                    }
                };
                let mut code = 0u32;
                for _ in 0..hex_digits {
                    let (_, digit) = chars.next()?;
                    code = code.checked_mul(16)? + digit.to_digit(16)?;
                }
                out.push(char::from_u32(code)?);
            }
            c if c == quote => return Some((out, &s[at + c.len_utf8()..])),
            c => out.push(c),
        }
    }
    None
}

fn attr_flag(node: &Node, key: &'static str) -> bool {
    matches!(node.get(key), Some(AttrValue::Int(n)) if *n != 0)
}

/// `toctree.attributes.get('caption')` when it is truthy
/// ([`caption_of`]).
fn toctree_caption(toctree: &Node) -> Option<&String> {
    caption_of(toctree.get("caption"))
}

/// A toctree's caption, read from the value `caption` of its `caption`
/// attribute, when it is truthy. The parser stores an absent caption the
/// way docutils prints a `None` attribute, as the string `True`
/// (`Element.starttag`), so a caption literally reading `True` is
/// indistinguishable from none and treated as none.
pub(crate) fn caption_of(caption: Option<&AttrValue>) -> Option<&String> {
    match caption {
        Some(AttrValue::Str(caption)) if !caption.is_empty() && caption != "True" => Some(caption),
        _ => None,
    }
}

/// `_resolve_toctree`'s keyword arguments (`adapters/toctree.py:119-131`).
///
/// A document's own toctrees resolve with [`Self::BODY`]; the HTML
/// builder's sidebar passes its own ([HB §9.9]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToctreeResolveOptions {
    /// Cut the tree at `maxdepth`; an unpruned tree keeps every level.
    pub prune: bool,
    /// The depth to cut at. `0` takes the node's own `:maxdepth:`
    /// (`maxdepth or toctree.get('maxdepth', -1)`), which is `-1`, every
    /// level, when the directive gave none.
    pub maxdepth: i64,
    /// Keep only each entry's title and its sub-toctrees; the node's
    /// `:titlesonly:` turns it on as well.
    pub titles_only: bool,
    /// Drop every sub-list off the way to the document being written.
    pub collapse: bool,
    /// Resolve hidden toctrees too, the node itself included; the node's
    /// `:includehidden:` turns it on for the toctrees below it.
    pub includehidden: bool,
}

impl ToctreeResolveOptions {
    /// What `get_and_resolve_doctree` passes for a document's own toctrees
    /// (`SP/sphinx/environment/__init__.py:701-713`, [HB §9.9]): cut at the
    /// node's own `:maxdepth:`, nothing collapsed, hidden toctrees left out.
    pub const BODY: ToctreeResolveOptions = ToctreeResolveOptions {
        prune: true,
        maxdepth: 0,
        titles_only: false,
        collapse: false,
        includehidden: false,
    };
}

/// What the resolution's warnings need, beyond Sphinx's arguments, to
/// print where Sphinx prints them and to word a missing entry's: Sphinx's
/// nodes carry their source paths, and its `env` knows `srcdir`,
/// `doc2path` and the configuration. A node here carries an index into its
/// document's source table ([`Span::source`]), and [`BuildEnvironment`]
/// keeps none of the rest.
pub struct ToctreeLocations<'a> {
    /// `env.srcdir`, against which a node's source and
    /// `doc2path(docname, False)` are taken.
    pub source_dir: &'a Path,
    /// `env.doc2path(docname)`: a document's source file, or the path a
    /// document the project does not have would have.
    pub doc2path: &'a dyn Fn(&str) -> PathBuf,
    /// A document's source table ([`Doctree::sources`]), which the spans
    /// of the `toctree` nodes read from it index. A toctree another
    /// document's toc holds is read from the document its `parent` names.
    pub sources: &'a dyn Fn(&str) -> Option<&'a [String]>,
    /// `include_patterns`, which together with `exclude_patterns` words the
    /// warning about an entry naming a document the environment does not
    /// have.
    pub include_patterns: &'a [String],
    /// `exclude_patterns`.
    pub exclude_patterns: &'a [String],
}

impl ToctreeLocations<'_> {
    /// `location=toctreenode`: `get_node_location` of the toctree, its
    /// source made absolute and its line
    /// (`SP/sphinx/util/logging.py:554-562`, [`LogLocation::Node`]). A
    /// document whose source table is not at hand is taken to hold the
    /// toctree in its own source.
    fn toctree(&self, toctree: &Node) -> Location {
        let parent = str_attr(toctree, "parent").unwrap_or_default();
        let (source, line) = (toctree.span.source, toctree.span.line);
        match (self.sources)(parent) {
            Some(sources) => LogLocation::render_node(sources, self.source_dir, source, line),
            None => {
                let own = [(self.doc2path)(parent).display().to_string()];
                LogLocation::render_node(&own, self.source_dir, source, line)
            }
        }
    }

    /// `location=docname`: `doc2path(docname)`, with no line — or, a
    /// string holding a `:` not being a docname to Sphinx's translator,
    /// the string itself (`SP/sphinx/util/logging.py:519-520`).
    fn document(&self, docname: &str) -> Location {
        if docname.contains(':') {
            return Location::Verbatim(docname.to_string());
        }
        Location::Path {
            path: (self.doc2path)(docname).display().to_string(),
            line: None,
        }
    }

    /// `str(env.doc2path(docname, False))`, spelled with `/` as `Matcher`
    /// canonicalizes it (`canon_path`).
    fn relative_path(&self, docname: &str) -> String {
        let path = (self.doc2path)(docname);
        let relative = path.strip_prefix(self.source_dir).unwrap_or(&path);
        let spelled = relative.to_string_lossy();
        if std::path::MAIN_SEPARATOR == '/' {
            spelled.into_owned()
        } else {
            spelled.replace(std::path::MAIN_SEPARATOR, "/")
        }
    }
}

/// The toctree half of `get_and_resolve_doctree`
/// (`SP/sphinx/environment/__init__.py:701-713`), which the resolve pass
/// runs over a document after its post-transforms: every `toctree` node of
/// `docname`'s doctree is replaced by its resolution with the body's
/// arguments ([`ToctreeResolveOptions::BODY`], [`resolve_toctree`]), which
/// takes the toctree's ids, classes, names and dupnames (docutils'
/// `replace_self`), or removed when that is nothing. `relative_uri` is the
/// builder's `get_relative_uri(from, to)`, the tocs' `only` nodes are
/// evaluated against `tags`, and the warnings go to `sink` in the order
/// Sphinx logs them.
///
/// Sphinx replaces the nodes during a live `findall`, and this walk keeps
/// its two consequences: a resolution put in a toctree's place is not
/// walked again, so a hidden toctree it holds stays as the bare node; and
/// the node after a removed toctree is skipped (no directive puts two
/// toctrees side by side: each has a `compound` of its own).
pub fn resolve_body_toctrees(
    env: &BuildEnvironment,
    docname: &str,
    doctree: &mut Doctree,
    relative_uri: &dyn Fn(&str, &str) -> String,
    tags: &Tags,
    locations: &ToctreeLocations<'_>,
    sink: &mut DiagnosticSink,
) {
    let body = BodyToctrees {
        env,
        docname,
        relative_uri,
        tags,
        locations,
    };
    body.resolve_below(&mut doctree.root, sink);
}

/// What [`resolve_body_toctrees`] resolves every toctree with.
struct BodyToctrees<'a, 'l> {
    env: &'a BuildEnvironment,
    docname: &'a str,
    relative_uri: &'a dyn Fn(&str, &str) -> String,
    tags: &'a Tags,
    locations: &'a ToctreeLocations<'l>,
}

impl BodyToctrees<'_, '_> {
    /// The toctrees below `node`, in document order.
    fn resolve_below(&self, node: &mut Node, sink: &mut DiagnosticSink) {
        let mut index = 0;
        while index < node.children.len() {
            if node.children[index].kind != kinds::TOCTREE {
                self.resolve_below(&mut node.children[index], sink);
                index += 1;
                continue;
            }
            let resolved = resolve_toctree(
                self.env,
                self.docname,
                &node.children[index],
                &ToctreeResolveOptions::BODY,
                self.relative_uri,
                self.tags,
                self.locations,
                sink,
            );
            match resolved {
                Some(mut resolved) => {
                    let basic = take_basic_atts(&mut node.children[index]);
                    update_basic_atts(&mut resolved, basic);
                    node.children[index] = resolved;
                }
                None => {
                    node.children.remove(index);
                }
            }
            // The walk's list iterator moves on to the next index either way.
            index += 1;
        }
    }
}

/// Resolve a `toctree` node into the entry tree Sphinx writes in its
/// place, as the document `docname` is written: `_resolve_toctree`
/// (`SP/sphinx/environment/adapters/toctree.py:119-220`).
///
/// `None` is Sphinx's "renders to nothing": a hidden toctree (unless
/// `opts` includes hidden ones), or one whose entries give no title.
/// Otherwise a `compact_paragraph` (`toctree=True`) holding the caption as
/// a `title` and one `bullet_list` of the entries (`_entries_from_toctree`),
/// each item and its paragraph classed `toctree-l<depth>` and the branch
/// leading to `docname` marked `current` and `iscurrent`
/// (`_toctree_add_classes`) before the tree is cut to the depth `opts` asks
/// for (`_toctree_copy`); last, every reference that is not a URL points at
/// `relative_uri(docname, refuri)` plus its anchor.
///
/// The warnings go to `sink`, located through `locations`, as Sphinx logs
/// them: a circular entry (`toc.circular`), an entry whose document has
/// no title (`toc.no_title`), an entry naming a document the environment
/// does not have (`toc.excluded`, `toc.not_included`, `toc.not_readable`),
/// and a toc's `only` expression that does not evaluate.
///
/// `_toctree_copy_seq` raises `ValueError` on a node a toc cannot hold;
/// a toc [`build_toc`] makes holds none, and such a node is copied whole.
#[allow(clippy::too_many_arguments)]
pub fn resolve_toctree(
    env: &BuildEnvironment,
    docname: &str,
    toctree: &Node,
    opts: &ToctreeResolveOptions,
    relative_uri: &dyn Fn(&str, &str) -> String,
    tags: &Tags,
    locations: &ToctreeLocations<'_>,
    sink: &mut DiagnosticSink,
) -> Option<Node> {
    if attr_flag(toctree, "hidden") && !opts.includehidden {
        return None;
    }
    let ancestors = toctree_ancestors(&env.toctree_includes, docname);
    let maxdepth = match opts.maxdepth {
        0 => match toctree.get("maxdepth") {
            Some(AttrValue::Int(depth)) => *depth,
            _ => -1,
        },
        depth => depth,
    };
    let walk = ToctreeWalk {
        env,
        prune: opts.prune,
        titles_only: opts.titles_only || attr_flag(toctree, "titlesonly"),
        collapse: opts.collapse,
        includehidden: opts.includehidden || attr_flag(toctree, "includehidden"),
        tags,
        ancestors: &ancestors,
        locations,
    };
    let entries = walk.entries_from_toctree(toctree, &[], false, sink);
    if entries.is_empty() {
        return None;
    }

    let mut newnode = Node::elem(kinds::COMPACT_PARAGRAPH, Span::ZERO);
    if let Some(caption) = toctree_caption(toctree) {
        // `title(caption, '', Text(caption))`, at the toctree's source and
        // line.
        let mut title = Node::elem(kinds::TITLE, toctree.span);
        title
            .children
            .push(Node::text_node(caption.clone(), Span::ZERO));
        newnode.children.push(title);
    }
    newnode.children.extend(entries);
    newnode.set("toctree", AttrValue::Int(1));

    // The classes first, then the cut, so that marking and pruning cannot
    // interact (sphinx-doc/sphinx#1046).
    add_classes(&mut newnode, docname);
    let copier = Copier {
        maxdepth: if opts.prune { maxdepth } else { 0 },
        collapse: opts.collapse,
        tags,
    };
    let mut newnode = copier.copy(&newnode, 1, sink);

    // `isinstance(newnode[-1], nodes.Element) and len(newnode[-1]) == 0`:
    // no titles found.
    if newnode
        .children
        .last()
        .is_some_and(|last| last.kind != kinds::TEXT && last.children.is_empty())
    {
        return None;
    }
    set_relative_uris(&mut newnode, docname, relative_uri);
    Some(newnode)
}

/// The target paths of a resolved toctree (`adapters/toctree.py:213-218`):
/// a reference whose `refuri` is not a URL (`url_re`) points at
/// `get_relative_uri(docname, refuri)` plus its `anchorname`.
fn set_relative_uris(node: &mut Node, docname: &str, relative_uri: &dyn Fn(&str, &str) -> String) {
    if node.kind == kinds::REFERENCE {
        let uri = match str_attr(node, "refuri") {
            Some(refuri) if !is_url(refuri) => {
                let anchorname = str_attr(node, "anchorname").unwrap_or_default();
                Some(format!("{}{anchorname}", relative_uri(docname, refuri)))
            }
            _ => None,
        };
        if let Some(uri) = uri {
            node.set("refuri", AttrValue::Str(uri));
        }
    }
    for child in &mut node.children {
        set_relative_uris(child, docname, relative_uri);
    }
}

/// The per-resolution context `_entries_from_toctree` threads through its
/// recursion.
struct ToctreeWalk<'a, 'l> {
    env: &'a BuildEnvironment,
    prune: bool,
    titles_only: bool,
    collapse: bool,
    includehidden: bool,
    tags: &'a Tags,
    /// `_get_toctree_ancestors(env.toctree_includes, docname)`.
    ancestors: &'a [String],
    locations: &'a ToctreeLocations<'l>,
}

impl ToctreeWalk<'_, '_> {
    /// `_entries_from_toctree` (`adapters/toctree.py:223-306`): the toc of
    /// every entry of `toctreenode` ([`Self::toctree_entry`]), cut to its
    /// titles under `:titlesonly:`, its sub-toctrees resolved in place;
    /// wrapped in one `bullet_list` unless `subtree`.
    fn entries_from_toctree(
        &self,
        toctreenode: &Node,
        parents: &[String],
        subtree: bool,
        sink: &mut DiagnosticSink,
    ) -> Vec<Node> {
        let mut entries = Vec::new();
        for (title, reference) in toctree_entries(toctreenode) {
            let Some((mut toc, refdoc)) =
                self.toctree_entry(title.as_deref(), &reference, toctreenode, parents, sink)
            else {
                continue;
            };

            // `:titlesonly:` keeps each top-level title and the toctrees
            // under it, and nothing else.
            if self.titles_only {
                for top_level in &mut toc.children {
                    if top_level.children.len() > 1 {
                        let subtrees = find_toctrees(top_level);
                        if subtrees.is_empty() {
                            top_level.children.remove(1);
                        } else {
                            top_level.children[1].children = subtrees;
                        }
                    }
                }
            }

            let mut sub_parents = Vec::with_capacity(parents.len() + 1);
            sub_parents.push(refdoc);
            sub_parents.extend_from_slice(parents);
            self.expand_sub_toctrees(&mut toc, &sub_parents, sink);

            entries.extend(toc.children);
        }

        if subtree {
            entries
        } else {
            let mut list = Node::elem(kinds::BULLET_LIST, Span::ZERO);
            list.children = entries;
            vec![list]
        }
    }

    /// `_toctree_entry` (`adapters/toctree.py:309-383`): the toc of one
    /// entry and the document it names (`''` for a URL, `self` or a
    /// generated page), or `None` where Sphinx raises `LookupError` — a
    /// circular entry, or one naming a document the environment does not
    /// have — and skips the entry.
    fn toctree_entry(
        &self,
        title: Option<&str>,
        reference: &str,
        toctreenode: &Node,
        parents: &[String],
        sink: &mut DiagnosticSink,
    ) -> Option<(Node, String)> {
        let (toc, refdoc) = if is_url(reference) {
            // `_toctree_url_entry`: only a missing title falls back to the
            // URL (`if title is None`).
            let text = title.unwrap_or(reference);
            (entry_toc(text, false, reference), String::new())
        } else if reference == "self" {
            // `_toctree_self_entry`: the toctree's own document, titled by
            // its title unless the entry has one (a `KeyError` without).
            let parent = str_attr(toctreenode, "parent").unwrap_or_default();
            let title = title.filter(|title| !title.is_empty());
            let text = match (title, self.env.titles.get(parent)) {
                (Some(title), _) => title.to_string(),
                (None, Some(own_title)) => clean_astext(own_title),
                (None, None) => return self.missing_entry(reference, toctreenode, sink),
            };
            (entry_toc(&text, true, parent), String::new())
        } else if let Some((_, docname, sectionname)) =
            VIRTUAL_DOCS.iter().find(|(name, _, _)| *name == reference)
        {
            // `_toctree_generated_entry`: `if not title`.
            let text = title
                .filter(|title| !title.is_empty())
                .unwrap_or(sectionname);
            (entry_toc(text, true, docname), String::new())
        } else {
            if parents.iter().any(|parent| parent == reference) {
                push_warning(
                    sink,
                    self.locations.document(reference),
                    format!(
                        "circular toctree references detected, ignoring: {reference} <- {}",
                        parents.join(" <- ")
                    ),
                    Category::from_parts(Some("toc"), Some("circular")),
                );
                return None;
            }
            // `env.metadata` is a `defaultdict`, so only `env.tocs[ref]` can
            // raise the `KeyError`.
            let Some(toc) = self.env.tocs.get(reference) else {
                return self.missing_entry(reference, toctreenode, sink);
            };
            let tocdepth = self
                .env
                .metadata
                .get(reference)
                .and_then(|metadata| metadata.get("tocdepth"))
                .and_then(|depth| depth.trim().parse::<i64>().ok())
                .unwrap_or(0);
            (
                self.standard_entry(title, reference, tocdepth, toc, sink),
                reference.to_string(),
            )
        };

        if toc.children.is_empty() {
            // An empty toc: no title will show up in the toctree.
            push_warning(
                sink,
                self.locations.toctree(toctreenode),
                format!(
                    "toctree contains reference to document {} that doesn't have a \
                     title: no link will be generated",
                    py_repr_str(reference)
                ),
                Category::from_parts(Some("toc"), Some("no_title")),
            );
        }
        Some((toc, refdoc))
    }

    /// The `except KeyError` of `_toctree_entry`: an entry naming a
    /// document the environment does not have (one removed after the
    /// toctree's document was read) is skipped, with a warning worded by
    /// where `doc2path(ref, False)` falls.
    fn missing_entry(
        &self,
        reference: &str,
        toctreenode: &Node,
        sink: &mut DiagnosticSink,
    ) -> Option<(Node, String)> {
        let path = self.locations.relative_path(reference);
        let (message, subtype) = if matches_any(&path, self.locations.exclude_patterns) {
            (
                "toctree contains reference to excluded document",
                "excluded",
            )
        } else if !matches_any(&path, self.locations.include_patterns) {
            (
                "toctree contains reference to non-included document",
                "not_included",
            )
        } else {
            (
                "toctree contains reference to non-existing document",
                "not_readable",
            )
        };
        push_warning(
            sink,
            self.locations.toctree(toctreenode),
            format!("{message} {}", py_repr_str(reference)),
            Category::from_parts(Some("toc"), Some(subtype)),
        );
        None
    }

    /// `_toctree_standard_entry` (`adapters/toctree.py:431-452`): the toc
    /// of the document `reference`, copied whole when the document is an
    /// ancestor of the one being written and no `:tocdepth:` cuts it (its
    /// `only` nodes kept, unevaluated), cut to its `:tocdepth:` otherwise;
    /// an explicit title replaces the text of the entry's own reference
    /// when the toc has a single top item.
    fn standard_entry(
        &self,
        title: Option<&str>,
        reference: &str,
        tocdepth: i64,
        toc: &Node,
        sink: &mut DiagnosticSink,
    ) -> Node {
        let ancestor = self.ancestors.iter().any(|ancestor| ancestor == reference);
        let mut toc = if ancestor && (!self.prune || tocdepth <= 0) {
            toc.clone()
        } else {
            let copier = Copier {
                maxdepth: tocdepth,
                collapse: self.collapse,
                tags: self.tags,
            };
            copier.copy(toc, 2, sink)
        };
        if let Some(title) = title.filter(|title| !title.is_empty()) {
            if toc.children.len() == 1 {
                replace_entry_title(&mut toc.children[0], reference, title);
            }
        }
        toc
    }

    /// The `for sub_toc_node in list(toc.findall(addnodes.toctree))` loop
    /// of `_entries_from_toctree`: every sub-toctree a toc holds is replaced
    /// by its entries, in document order, unless it is hidden and hidden
    /// toctrees are not included — then it stays as the bare node (which
    /// renders, and indexes, as nothing).
    fn expand_sub_toctrees(&self, node: &mut Node, parents: &[String], sink: &mut DiagnosticSink) {
        let mut index = 0;
        while index < node.children.len() {
            if node.children[index].kind != kinds::TOCTREE {
                self.expand_sub_toctrees(&mut node.children[index], parents, sink);
                index += 1;
                continue;
            }
            if attr_flag(&node.children[index], "hidden") && !self.includehidden {
                index += 1;
                continue;
            }
            let sub_toc_node = node.children.remove(index);
            let expanded = self.entries_from_toctree(&sub_toc_node, parents, true, sink);
            let inserted = expanded.len();
            node.children.splice(index..index, expanded);
            index += inserted;
        }
    }
}

/// `bullet_list > list_item > compact_paragraph > reference > Text`: the
/// toc of a URL (`internal=False`), `self` or generated entry
/// (`internal=True`), `adapters/toctree.py:386-428`.
fn entry_toc(text: &str, internal: bool, refuri: &str) -> Node {
    let mut reference = Node::elem(kinds::REFERENCE, Span::ZERO);
    reference.set("anchorname", AttrValue::Str(String::new()));
    reference.set("internal", AttrValue::Int(i64::from(internal)));
    reference.set("refuri", AttrValue::Str(refuri.to_string()));
    reference.children.push(Node::text_node(text, Span::ZERO));
    let mut para = Node::elem(kinds::COMPACT_PARAGRAPH, Span::ZERO);
    para.children.push(reference);
    let mut item = Node::elem(kinds::LIST_ITEM, Span::ZERO);
    item.children.push(para);
    let mut toc = Node::elem(kinds::BULLET_LIST, Span::ZERO);
    toc.children.push(item);
    toc
}

/// An explicit `Title <ref>` replaces the text of every reference to the
/// entry's document itself (`refuri == ref`, no anchor).
fn replace_entry_title(node: &mut Node, reference: &str, title: &str) {
    let is_entry_reference = node.kind == kinds::REFERENCE
        && str_attr(node, "refuri") == Some(reference)
        && str_attr(node, "anchorname") == Some("");
    if is_entry_reference {
        node.children = vec![Node::text_node(title, Span::ZERO)];
        return;
    }
    for child in &mut node.children {
        replace_entry_title(child, reference, title);
    }
}

/// `findall(addnodes.toctree)` below `node`, in document order, as the
/// childless copies the toc holds anyway.
fn find_toctrees(node: &Node) -> Vec<Node> {
    let mut out = Vec::new();
    fn walk(node: &Node, out: &mut Vec<Node>) {
        for child in &node.children {
            if child.kind == kinds::TOCTREE {
                out.push(child.shallow_copy());
            }
            walk(child, out);
        }
    }
    walk(node, &mut out);
    out
}

/// `_toctree_add_classes(newnode, 1, docname)` (`adapters/toctree.py:455-482`):
/// every `list_item` and `compact_paragraph` gets the `toctree-l<N>` class
/// of its depth, and a reference to `docname` marks its branch: with no
/// anchor, it and every node above it get the `current` class; then, unless
/// its item is already `iscurrent`, it and every node above it are.
fn add_classes(newnode: &mut Node, docname: &str) {
    add_classes_below(newnode, &mut Vec::new(), 1, docname);
}

/// `_toctree_add_classes(node, depth, docname)` for the node at `path`
/// below `root`: marking a branch reaches every node above a reference,
/// which the path names.
fn add_classes_below(root: &mut Node, path: &mut Vec<usize>, depth: i64, docname: &str) {
    for index in 0..node_at(root, path).children.len() {
        path.push(index);
        let marked = add_classes_at(root, path, depth, docname);
        path.pop();
        if marked == Marked::Already {
            return;
        }
    }
}

/// Whether a reference's item was already `iscurrent`, which stops the
/// marking of its paragraph (`return` in `_toctree_add_classes`).
#[derive(PartialEq, Eq)]
enum Marked {
    Already,
    Not,
}

/// One child of `_toctree_add_classes`'s loop, at `path` below `root`.
fn add_classes_at(root: &mut Node, path: &mut Vec<usize>, depth: i64, docname: &str) -> Marked {
    let node = node_at(root, path);
    match node.kind {
        kinds::COMPACT_PARAGRAPH | kinds::LIST_ITEM => {
            let class = format!("toctree-l{}", depth - 1);
            node_at_mut(root, path).attrs.classes.push(class);
            add_classes_below(root, path, depth, docname);
        }
        kinds::BULLET_LIST => add_classes_below(root, path, depth + 1, docname),
        kinds::REFERENCE if str_attr(node, "refuri") == Some(docname) => {
            if str_attr(node, "anchorname").unwrap_or_default().is_empty() {
                for len in 0..=path.len() {
                    node_at_mut(root, &path[..len])
                        .attrs
                        .classes
                        .push("current".to_string());
                }
            }
            let item_is_current = path
                .len()
                .checked_sub(2)
                .is_some_and(|len| attr_flag(node_at(root, &path[..len]), "iscurrent"));
            if item_is_current {
                return Marked::Already;
            }
            for len in 0..=path.len() {
                node_at_mut(root, &path[..len]).set("iscurrent", AttrValue::Int(1));
            }
        }
        _ => {}
    }
    Marked::Not
}

/// A string attribute of `node`.
fn str_attr<'n>(node: &'n Node, key: &'static str) -> Option<&'n str> {
    match node.get(key) {
        Some(AttrValue::Str(value)) => Some(value),
        _ => None,
    }
}

/// The node at `path` (child indices) below `root`.
fn node_at<'n>(root: &'n Node, path: &[usize]) -> &'n Node {
    path.iter().fold(root, |node, &index| &node.children[index])
}

/// [`node_at`], mutably.
fn node_at_mut<'n>(root: &'n mut Node, path: &[usize]) -> &'n mut Node {
    path.iter()
        .fold(root, |node, &index| &mut node.children[index])
}

/// `_toctree_copy`'s arguments besides the node and its depth: the depth
/// to cut at (`<= 0` keeps every level), whether to drop the sub-lists off
/// the way to the document being written, and the tags a toc's `only`
/// nodes are evaluated against.
struct Copier<'a> {
    maxdepth: i64,
    collapse: bool,
    tags: &'a Tags,
}

impl Copier<'_> {
    /// `_toctree_copy(node, depth, ...)` (`adapters/toctree.py:485-493`):
    /// `node` cut and copied, its depth counted from `depth - 1` (and at
    /// least 1).
    fn copy(&self, node: &Node, depth: i64, sink: &mut DiagnosticSink) -> Node {
        let depth = (depth - 1).max(1);
        let mut copied = self.copy_seq(node, depth, true, false, sink);
        debug_assert_eq!(copied.len(), 1);
        copied.pop().unwrap_or_else(|| node.shallow_copy())
    }

    /// `_toctree_copy_seq` (`adapters/toctree.py:496-559`): a paragraph or
    /// an item is copied with its children; a list is too when it is the
    /// top level, or within the depth and (collapsing) on the way to the
    /// current document, and dropped otherwise, unless it is the node the
    /// copy starts at; a toctree is copied for the caller to resolve; an
    /// `only` node gives way to its children when the tags satisfy it, and
    /// to nothing otherwise; anything else (a reference, a caption) is
    /// copied whole.
    fn copy_seq(
        &self,
        node: &Node,
        depth: i64,
        initial_call: bool,
        is_current: bool,
        sink: &mut DiagnosticSink,
    ) -> Vec<Node> {
        // `'iscurrent' in node`.
        let node_is_current = node.get("iscurrent").is_some();
        match node.kind {
            kinds::COMPACT_PARAGRAPH | kinds::LIST_ITEM => {
                let mut copy = node.shallow_copy();
                for child in &node.children {
                    let copied = self.copy_seq(child, depth, false, node_is_current, sink);
                    copy.children.extend(copied);
                }
                vec![copy]
            }
            kinds::BULLET_LIST => {
                let keep = depth <= 1
                    || ((depth <= self.maxdepth || self.maxdepth <= 0)
                        && (!self.collapse || is_current || node_is_current));
                if !keep && !initial_call {
                    return Vec::new();
                }
                let mut copy = node.shallow_copy();
                for child in &node.children {
                    let copied = self.copy_seq(child, depth + 1, false, node_is_current, sink);
                    copy.children.extend(copied);
                }
                vec![copy]
            }
            kinds::TOCTREE => vec![node.shallow_copy()],
            kinds::ONLY => {
                if !self.keep_only_children(node, sink) {
                    return Vec::new();
                }
                let mut copied = Vec::new();
                for child in &node.children {
                    copied.extend(self.copy_seq(child, depth, false, node_is_current, sink));
                }
                copied
            }
            _ => vec![node.clone()],
        }
    }

    /// `_only_node_keep_children` (`SP/sphinx/util/nodes.py:732-742`):
    /// whether the tags satisfy the `only` node's expression; one that does
    /// not evaluate keeps the children, with a warning at `location=node`.
    /// A toc's `only` node is built without a source or a line
    /// (`TocTreeCollector.process_doc`), as is every node above it, so the
    /// warning has no location (probed on Sphinx 9.1.0).
    fn keep_only_children(&self, only: &Node, sink: &mut DiagnosticSink) -> bool {
        let evaluated = match only.get("expr") {
            Some(AttrValue::Str(expr)) => self.tags.eval_condition(expr),
            // `node['expr']` raising `KeyError('expr')`.
            _ => Err("'expr'".to_string()),
        };
        evaluated.unwrap_or_else(|error| {
            push_warning(
                sink,
                Location::None,
                format!("exception while evaluating only directive expression: {error}"),
                Category::None,
            );
            true
        })
    }
}

/// A warning the resolution logs, after every record `sink` already holds.
fn push_warning(
    sink: &mut DiagnosticSink,
    location: Location,
    message: String,
    category: Category,
) {
    sink.push_next(Diagnostic {
        location,
        severity: Severity::Warning,
        message,
        category,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::BuilderKind;
    use crate::rst;
    use crate::transforms::test_support::{merge_project, Project, DOC};

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
                py: Default::default(),
                srcdir: None,
                found_docs: Some(std::sync::Arc::new(found.clone())),
                ..Default::default()
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

    /// `build_toc`'s `memo_parents` bubbling (`collectors/toctree.py:159-181`),
    /// pinned against a sphinx 9.1.0 `env.tocs['a']` for this exact source:
    /// nested descriptions nest, a multi-signature description collects its
    /// children under its LAST signature, and a description that produced no
    /// entry of its own (`describe` has no `_toc_name`) is walked past, so
    /// its children land at the section level.
    #[test]
    fn object_entries_nest_under_the_nearest_description_that_has_one() {
        let doctree = parse(
            concat!(
                "A\n=\n\n",
                ".. confval:: outer\n\n   Outer body.\n\n",
                "   .. confval:: inner\n\n      Inner body.\n\n",
                "      .. confval:: deepest\n\n         Deep body.\n\n",
                ".. confval:: sibling\n\n",
                ".. confval:: multi_a\n             multi_b\n\n",
                "   .. confval:: under_multi\n\n",
                ".. describe:: nothing\n\n   .. confval:: under_describe\n",
            ),
            "a",
            &docs(&["a"]),
        );
        let (toc, n) = build_toc(&doctree, "a");
        assert_eq!(n, 9);
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
                "                <compact_paragraph skip_section_number=\"1\">\n",
                "                    <reference anchorname=\"#confval-outer\" internal=\"1\" refuri=\"a\">\n",
                "                        <literal>\n",
                "                            outer\n",
                "                <bullet_list>\n",
                "                    <list_item>\n",
                "                        <compact_paragraph skip_section_number=\"1\">\n",
                "                            <reference anchorname=\"#confval-inner\" internal=\"1\" refuri=\"a\">\n",
                "                                <literal>\n",
                "                                    inner\n",
                "                        <bullet_list>\n",
                "                            <list_item>\n",
                "                                <compact_paragraph skip_section_number=\"1\">\n",
                "                                    <reference anchorname=\"#confval-deepest\" internal=\"1\" refuri=\"a\">\n",
                "                                        <literal>\n",
                "                                            deepest\n",
                "            <list_item>\n",
                "                <compact_paragraph skip_section_number=\"1\">\n",
                "                    <reference anchorname=\"#confval-sibling\" internal=\"1\" refuri=\"a\">\n",
                "                        <literal>\n",
                "                            sibling\n",
                "            <list_item>\n",
                "                <compact_paragraph skip_section_number=\"1\">\n",
                "                    <reference anchorname=\"#confval-multi_a\" internal=\"1\" refuri=\"a\">\n",
                "                        <literal>\n",
                "                            multi_a\n",
                "            <list_item>\n",
                "                <compact_paragraph skip_section_number=\"1\">\n",
                "                    <reference anchorname=\"#confval-multi_b\" internal=\"1\" refuri=\"a\">\n",
                "                        <literal>\n",
                "                            multi_b\n",
                "                <bullet_list>\n",
                "                    <list_item>\n",
                "                        <compact_paragraph skip_section_number=\"1\">\n",
                "                            <reference anchorname=\"#confval-under_multi\" internal=\"1\" refuri=\"a\">\n",
                "                                <literal>\n",
                "                                    under_multi\n",
                "            <list_item>\n",
                "                <compact_paragraph skip_section_number=\"1\">\n",
                "                    <reference anchorname=\"#confval-under_describe\" internal=\"1\" refuri=\"a\">\n",
                "                        <literal>\n",
                "                            under_describe\n",
            )
        );
    }

    /// The three `build_toc` skips: no `_toc_name` (only `confval` sets one
    /// in the std domain), `:no-contents-entry:`, and an id-less signature
    /// (`:no-index:`).
    #[test]
    fn object_entries_skip_unnamed_opted_out_and_id_less_signatures() {
        let doctree = parse(
            concat!(
                "A\n=\n\n",
                ".. envvar:: HOME\n\n",
                ".. confval:: hidden\n   :no-contents-entry:\n\n",
                ".. confval:: unindexed\n   :no-index:\n\n",
                ".. confval:: kept\n",
            ),
            "a",
            &docs(&["a"]),
        );
        let (toc, n) = build_toc(&doctree, "a");
        // The section plus exactly one object entry.
        assert_eq!(n, 2);
        let out = toc.pformat();
        assert!(out.contains("#confval-kept"), "{out}");
        assert!(!out.contains("HOME"), "{out}");
        assert!(!out.contains("hidden"), "{out}");
        assert!(!out.contains("unindexed"), "{out}");
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
            source: 0,
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
    /// first source suffix — by Sphinx's `Matcher`, which also tries a
    /// `**/` pattern without its prefix: `**/drafts.rst` covers a top-level
    /// `drafts` (probed on Sphinx 9.1.0: `[toc.excluded]`).
    #[test]
    fn excluded_targets_get_their_own_message() {
        let found = docs(&["index"]);
        for (pattern, target) in [("drafts/*", "drafts/wip"), ("**/drafts.rst", "drafts")] {
            let excluded = vec![pattern.to_string()];
            let entries = lines(&[target]);
            let resolved = resolve_entries(&ToctreeContent {
                exclude_patterns: &excluded,
                ..content(&entries, "index", &found)
            });
            assert_eq!(
                resolved.warnings[0].message,
                format!("toctree contains reference to excluded document '{target}'"),
                "{pattern}"
            );
            assert_eq!(
                resolved.warnings[0].category.as_deref(),
                Some("toc.excluded"),
                "{pattern}"
            );
        }
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
                source: 0,
                line: 1,
                message: "toctree glob pattern 'missing*' didn't match any documents".to_string(),
                category: None,
                kind: ToctreeWarningKind::EmptyGlob,
                seq: 0,
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

    /// `.. title::` sets the document's `title` attribute, the last one
    /// winning, and Sphinx's TitleCollector (`collectors/title.py:38-59`)
    /// makes it the document's longtitle, which only the HTML `<title>` and
    /// the search index's titles show. `env.titles`, which toctrees and the
    /// `:doc:` role show, keeps the first section title, and takes the
    /// attribute only in a document without a section, whose toc stays
    /// empty, so that a toctree naming it still warns that it has no title.
    //
    // oracle: Sphinx 9.1.0, a dummy build of these four documents
    //   (scratchpad/t18a/probes/sbuild.py, project `title_toctree`): the
    //   post-read document lines, `env.titles`, `env.longtitles`,
    //   `env.tocs['nosec']`, the resolved `index` and its warning.
    #[test]
    fn title_sets_longtitle_only() {
        let project = project(&[
            (
                "index",
                "Index\n=====\n\n.. toctree::\n\n   page\n   sub\n   nosec\n",
            ),
            ("nosec", ".. title:: Only Title\n\nJust a paragraph.\n"),
            (
                "page",
                ".. title:: Custom <Page> & Title\n\nReal Title\n==========\n\nBody.\n",
            ),
            (
                "sub",
                ".. title:: Sub Long Title\n\nSub\n===\n\n.. title:: Last One Wins\n\nBody.\n",
            ),
        ]);
        let env = &project.env;
        let titles = |docname: &str| {
            (
                env.titles[docname].pformat(),
                env.longtitles[docname].pformat(),
            )
        };
        let title = |text: &str| format!("<title>\n    {text}\n");
        assert_eq!(titles("index"), (title("Index"), title("Index")));
        assert_eq!(
            titles("page"),
            (title("Real Title"), title("Custom <Page> & Title"))
        );
        assert_eq!(titles("sub"), (title("Sub"), title("Last One Wins")));
        assert_eq!(titles("nosec"), (title("Only Title"), title("Only Title")));
        assert_eq!(env.tocs["nosec"].pformat(), "<bullet_list>\n");

        let document_line = |docname: &str| {
            let (_, doctree) = project
                .doctrees
                .iter()
                .find(|(name, _)| name == docname)
                .expect("a document of the project");
            doctree
                .root
                .pformat()
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let document = |docname: &str, title: &str| {
            format!(
                "<document source=\"<SRCDIR>/{docname}.rst\" title=\"{title}\" \
                 translation_progress=\"{{'total': 0, 'translated': 0}}\">"
            )
        };
        assert_eq!(
            document_line("page"),
            document("page", "Custom <Page> & Title")
        );
        assert_eq!(document_line("sub"), document("sub", "Last One Wins"));
        assert_eq!(document_line("nosec"), document("nosec", "Only Title"));

        let (index, warnings) = resolve_body(&project, "index");
        assert_eq!(
            warnings,
            [
                "<SRCDIR>/index.rst:4: WARNING: toctree contains reference to document 'nosec' \
                 that doesn't have a title: no link will be generated [toc.no_title]"
            ]
        );
        assert_eq!(
            index,
            concat!(
                "    <section ids=\"index\" names=\"index\">\n",
                "        <title>\n",
                "            Index\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph toctree=\"1\">\n",
                "                <bullet_list>\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                Real Title\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                Sub\n",
            )
        );
    }

    #[test]
    fn toctree_entries_read_back_what_entries_attr_wrote() {
        let entries = ResolvedEntries {
            entries: vec![
                (None, "intro".to_string()),
                (Some("Custom <x>".to_string()), "api/it's".to_string()),
                (
                    Some("say \"hi\"".to_string()),
                    "https://example.org/a?b=c".to_string(),
                ),
                (Some("tab\there".to_string()), "self".to_string()),
            ],
            ..Default::default()
        };
        let mut toctree = Node::elem(kinds::TOCTREE, Span::ZERO);
        toctree.set("entries", entries.entries_attr());
        assert_eq!(toctree_entries(&toctree), entries.entries);
    }

    /// The project `documents` (`(docname, source)` pairs) read and merged
    /// the way the build reads one ([`merge_project`]), in sorted docname
    /// order.
    fn project(documents: &[(&str, &str)]) -> Project {
        let mut sorted = documents.to_vec();
        sorted.sort_by_key(|(docname, _)| *docname);
        merge_project(&sorted)
    }

    /// [`resolve_body_with`] under Sphinx's default `include_patterns`
    /// (`['**']`) and `exclude_patterns` (`[]`).
    fn resolve_body(project: &Project, docname: &str) -> (String, Vec<String>) {
        resolve_body_with(project, docname, &["**"], &[])
    }

    /// `docname`'s body toctrees resolved as the dummy builder resolves
    /// them ([`resolve_body_toctrees`]: its tags and `''` URIs), under
    /// `include_patterns` and `exclude_patterns`: the resolved doctree's
    /// pformat less its document line, and the warning stream's lines.
    fn resolve_body_with(
        project: &Project,
        docname: &str,
        include_patterns: &[&str],
        exclude_patterns: &[&str],
    ) -> (String, Vec<String>) {
        let include_patterns: Vec<String> =
            include_patterns.iter().map(|p| p.to_string()).collect();
        let exclude_patterns: Vec<String> =
            exclude_patterns.iter().map(|p| p.to_string()).collect();
        let doc2path = |docname: &str| PathBuf::from(format!("<SRCDIR>/{docname}.rst"));
        let sources = |docname: &str| {
            project
                .doctrees
                .iter()
                .find(|(name, _)| name == docname)
                .map(|(_, doctree)| doctree.sources.as_slice())
        };
        let locations = ToctreeLocations {
            source_dir: Path::new("<SRCDIR>"),
            doc2path: &doc2path,
            sources: &sources,
            include_patterns: &include_patterns,
            exclude_patterns: &exclude_patterns,
        };
        let tags = Tags::new(BuilderKind::Dummy.tags().iter().map(|tag| tag.to_string()));
        let (_, doctree) = project
            .doctrees
            .iter()
            .find(|(name, _)| name == docname)
            .expect("a document of the project");
        let mut doctree = doctree.clone();
        let mut sink = DiagnosticSink::default();
        resolve_body_toctrees(
            &project.env,
            docname,
            &mut doctree,
            &|_, _| String::new(),
            &tags,
            &locations,
            &mut sink,
        );
        let pformat = doctree.root.pformat();
        let document = DOC.replace("index.rst", &format!("{docname}.rst"));
        let body = pformat
            .strip_prefix(&document)
            .unwrap_or_else(|| panic!("unexpected document line:\n{pformat}"))
            .to_string();
        let warnings = sink
            .into_ordered()
            .iter()
            .flat_map(|diagnostic| {
                diagnostic
                    .render(true)
                    .split('\n')
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
        (body, warnings)
    }

    /// A document's own toctrees resolve to Sphinx's entry tree
    /// (`get_and_resolve_doctree`'s arguments): the caption becomes a
    /// `title`, every item and its paragraph get the `toctree-l<N>` class
    /// of their depth, and the branch leading to the document being written
    /// is marked `current` and `iscurrent` before the tree is pruned to
    /// `:maxdepth:` — `x`'s entry for `y` stays `current` although `x`'s own
    /// entry below it is pruned. A URL entry is external
    /// (`internal="0"`); every other `refuri` is the dummy builder's `''`
    /// plus the entry's anchor. `:titlesonly:` keeps each entry's title and
    /// its sub-toctrees. Every value probed on Sphinx 9.1.0.
    #[test]
    fn body_toctree_entries_and_classes() {
        let project = project(&[
            (
                "index",
                "Root\n====\n\n.. toctree::\n   :caption: Contents\n   :maxdepth: 2\n\n   \
                 a\n   https://example.org\n   Explicit <c>\n   self\n   genindex\n",
            ),
            (
                "a",
                "A\n=\n\nA Sec\n-----\n\nA Sub\n~~~~~\n\n.. toctree::\n\n   b\n",
            ),
            ("b", "B\n=\n\nB Sec\n-----\n"),
            ("c", "C\n=\n\nC Sec\n-----\n"),
            ("t", "T\n=\n\n.. toctree::\n   :titlesonly:\n\n   a\n"),
            ("x", "X\n=\n\n.. toctree::\n   :maxdepth: 1\n\n   y\n"),
            ("y", "Y\n=\n\n.. toctree::\n\n   x\n"),
        ]);

        let (index, warnings) = resolve_body(&project, "index");
        assert_eq!(
            index,
            concat!(
                "    <section ids=\"root\" names=\"root\">\n",
                "        <title>\n",
                "            Root\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph classes=\"current\" iscurrent=\"1\" toctree=\"1\">\n",
                "                <title>\n",
                "                    Contents\n",
                "                <bullet_list classes=\"current\" iscurrent=\"1\">\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                A\n",
                "                        <bullet_list>\n",
                "                            <list_item classes=\"toctree-l2\">\n",
                "                                <compact_paragraph classes=\"toctree-l2\">\n",
                "                                    <reference anchorname=\"#a-sec\" internal=\"1\" refuri=\"#a-sec\">\n",
                "                                        A Sec\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"0\" refuri=\"https://example.org\">\n",
                "                                https://example.org\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                Explicit\n",
                "                        <bullet_list>\n",
                "                            <list_item classes=\"toctree-l2\">\n",
                "                                <compact_paragraph classes=\"toctree-l2\">\n",
                "                                    <reference anchorname=\"#c-sec\" internal=\"1\" refuri=\"#c-sec\">\n",
                "                                        C Sec\n",
                "                    <list_item classes=\"toctree-l1 current\" iscurrent=\"1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1 current\" iscurrent=\"1\">\n",
                "                            <reference anchorname=\"\" classes=\"current\" internal=\"1\" iscurrent=\"1\" refuri=\"\">\n",
                "                                Root\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                Index\n",
            )
        );
        assert_eq!(warnings, Vec::<String>::new());

        let (t, warnings) = resolve_body(&project, "t");
        assert_eq!(
            t,
            concat!(
                "    <section ids=\"t\" names=\"t\">\n",
                "        <title>\n",
                "            T\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph toctree=\"1\">\n",
                "                <bullet_list>\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                A\n",
                "                        <bullet_list>\n",
                "                            <list_item classes=\"toctree-l2\">\n",
                "                                <compact_paragraph classes=\"toctree-l2\">\n",
                "                                    <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                        B\n",
            )
        );
        assert_eq!(warnings, Vec::<String>::new());

        let (x, warnings) = resolve_body(&project, "x");
        assert_eq!(
            x,
            concat!(
                "    <section ids=\"x\" names=\"x\">\n",
                "        <title>\n",
                "            X\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph classes=\"current\" iscurrent=\"1\" toctree=\"1\">\n",
                "                <bullet_list classes=\"current\" iscurrent=\"1\">\n",
                "                    <list_item classes=\"toctree-l1 current\" iscurrent=\"1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                Y\n",
            )
        );
        assert_eq!(
            warnings,
            [
                "<SRCDIR>/y.rst: WARNING: circular toctree references detected, \
                 ignoring: y <- x <- y [toc.circular]"
            ]
        );
    }

    /// A hidden toctree resolves to nothing (`_resolve_toctree` returns
    /// `None` unless asked to include hidden ones) and leaves its document;
    /// a hidden toctree inside another document's toc stays in the
    /// resolved tree as the bare `toctree` node, unless the toctree being
    /// resolved is `:includehidden:`. Probed on Sphinx 9.1.0.
    #[test]
    fn hidden_toctree_resolves_to_nothing() {
        let project = project(&[
            (
                "index",
                "Root\n====\n\n.. toctree::\n\n   a\n\n.. toctree::\n   :hidden:\n\n   b\n\n\
                 .. toctree::\n   :includehidden:\n\n   c\n",
            ),
            ("a", "A\n=\n\n.. toctree::\n   :hidden:\n\n   a2\n"),
            ("a2", "A2\n==\n"),
            ("b", "B\n=\n"),
            ("c", "C\n=\n\n.. toctree::\n   :hidden:\n\n   c2\n"),
            ("c2", "C2\n==\n"),
        ]);

        let (a, warnings) = resolve_body(&project, "a");
        assert_eq!(
            a,
            concat!(
                "    <section ids=\"a\" names=\"a\">\n",
                "        <title>\n",
                "            A\n",
                "        <compound classes=\"toctree-wrapper\">\n",
            )
        );
        assert_eq!(warnings, Vec::<String>::new());

        let (index, warnings) = resolve_body(&project, "index");
        assert_eq!(
            index,
            concat!(
                "    <section ids=\"root\" names=\"root\">\n",
                "        <title>\n",
                "            Root\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph toctree=\"1\">\n",
                "                <bullet_list>\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                A\n",
                "                        <bullet_list>\n",
                "                            <toctree caption=\"True\" entries=\"(None,\\ 'a2')\" glob=\"0\" hidden=\"1\" includefiles=\"a2\" includehidden=\"0\" maxdepth=\"-1\" numbered=\"0\" parent=\"a\" rawentries=\"\" titlesonly=\"0\">\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph toctree=\"1\">\n",
                "                <bullet_list>\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                C\n",
                "                        <bullet_list>\n",
                "                            <list_item classes=\"toctree-l2\">\n",
                "                                <compact_paragraph classes=\"toctree-l2\">\n",
                "                                    <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                        C2\n",
            )
        );
        assert_eq!(warnings, Vec::<String>::new());

        // The same hidden toctree, resolved on its own: nothing, unless the
        // call includes hidden toctrees.
        let (_, doctree) = &project.doctrees[project
            .doctrees
            .iter()
            .position(|(name, _)| name == "index")
            .expect("index is read")];
        let hidden = find_toctrees(&doctree.root).swap_remove(1);
        assert!(attr_flag(&hidden, "hidden"));
        let doc2path = |docname: &str| PathBuf::from(format!("<SRCDIR>/{docname}.rst"));
        let include_patterns = ["**".to_string()];
        let locations = ToctreeLocations {
            source_dir: Path::new("<SRCDIR>"),
            doc2path: &doc2path,
            sources: &|_| None,
            include_patterns: &include_patterns,
            exclude_patterns: &[],
        };
        let tags = Tags::new(BuilderKind::Dummy.tags().iter().map(|tag| tag.to_string()));
        let resolve = |opts: &ToctreeResolveOptions| {
            let mut sink = DiagnosticSink::default();
            let resolved = resolve_toctree(
                &project.env,
                "index",
                &hidden,
                opts,
                &|_, _| String::new(),
                &tags,
                &locations,
                &mut sink,
            );
            assert!(sink.into_ordered().is_empty());
            resolved
        };
        assert!(resolve(&ToctreeResolveOptions::BODY).is_none());
        let included = resolve(&ToctreeResolveOptions {
            includehidden: true,
            ..ToctreeResolveOptions::BODY
        })
        .expect("an included hidden toctree resolves");
        assert_eq!(included.astext(), "B");
    }

    /// An entry already among the documents whose toctrees led to it is a
    /// cycle: Sphinx skips it and warns `circular toctree references
    /// detected, ignoring: <ref> <- <parents>` (`toc.circular`) at
    /// `location=ref`, the document's path with no line — the oracle's
    /// `toctree_circular` project, every document's resolution probed on
    /// Sphinx 9.1.0.
    #[test]
    fn circular_toctree_warns() {
        let project = project(&[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n"),
            ("a", "A\n=\n\n.. toctree::\n\n   b\n"),
            ("b", "B\n=\n\n.. toctree::\n\n   a\n"),
        ]);
        let circular = |chain: &str, at: &str| {
            format!(
                "<SRCDIR>/{at}.rst: WARNING: circular toctree references detected, \
                 ignoring: {chain} [toc.circular]"
            )
        };

        let (a, warnings) = resolve_body(&project, "a");
        assert_eq!(warnings, [circular("b <- a <- b", "b")]);
        assert_eq!(
            a,
            concat!(
                "    <section ids=\"a\" names=\"a\">\n",
                "        <title>\n",
                "            A\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph classes=\"current\" iscurrent=\"1\" toctree=\"1\">\n",
                "                <bullet_list classes=\"current\" iscurrent=\"1\">\n",
                "                    <list_item classes=\"toctree-l1 current\" iscurrent=\"1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                B\n",
                "                        <bullet_list classes=\"current\" iscurrent=\"1\">\n",
                "                            <list_item classes=\"toctree-l2 current\" iscurrent=\"1\">\n",
                "                                <compact_paragraph classes=\"toctree-l2 current\" iscurrent=\"1\">\n",
                "                                    <reference anchorname=\"\" classes=\"current\" internal=\"1\" iscurrent=\"1\" refuri=\"\">\n",
                "                                        A\n",
                "                                <bullet_list>\n",
            )
        );

        let (_, warnings) = resolve_body(&project, "b");
        assert_eq!(warnings, [circular("a <- b <- a", "a")]);
        let (index, warnings) = resolve_body(&project, "index");
        assert_eq!(warnings, [circular("a <- b <- a", "a")]);
        assert_eq!(
            index,
            concat!(
                "    <section ids=\"index\" names=\"index\">\n",
                "        <title>\n",
                "            Index\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph toctree=\"1\">\n",
                "                <bullet_list>\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                A\n",
                "                        <bullet_list>\n",
                "                            <list_item classes=\"toctree-l2\">\n",
                "                                <compact_paragraph classes=\"toctree-l2\">\n",
                "                                    <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                        B\n",
                "                                <bullet_list>\n",
            )
        );
    }

    /// An entry whose document has no title contributes nothing, and
    /// Sphinx warns `toctree contains reference to document %r that
    /// doesn't have a title: no link will be generated` (`toc.no_title`)
    /// at `location=toctreenode`: the toctree being resolved, which for a
    /// sub-toctree is the one in the other document (`a.rst:4` when `index`
    /// is resolved). A toctree left with no entry renders to nothing.
    /// Probed on Sphinx 9.1.0.
    #[test]
    fn toctree_entry_without_title_warns() {
        let project = project(&[
            (
                "index",
                "Index\n=====\n\n.. toctree::\n   :caption: Top\n\n   a\n   notitle\n",
            ),
            ("a", "A\n=\n\n.. toctree::\n\n   notitle\n"),
            ("notitle", "No title here.\n"),
        ]);
        let no_title = |at: &str| {
            format!(
                "<SRCDIR>/{at}.rst:4: WARNING: toctree contains reference to document \
                 'notitle' that doesn't have a title: no link will be generated [toc.no_title]"
            )
        };

        let (a, warnings) = resolve_body(&project, "a");
        assert_eq!(warnings, [no_title("a")]);
        assert_eq!(
            a,
            concat!(
                "    <section ids=\"a\" names=\"a\">\n",
                "        <title>\n",
                "            A\n",
                "        <compound classes=\"toctree-wrapper\">\n",
            )
        );

        let (index, warnings) = resolve_body(&project, "index");
        assert_eq!(warnings, [no_title("a"), no_title("index")]);
        assert_eq!(
            index,
            concat!(
                "    <section ids=\"index\" names=\"index\">\n",
                "        <title>\n",
                "            Index\n",
                "        <compound classes=\"toctree-wrapper\">\n",
                "            <compact_paragraph toctree=\"1\">\n",
                "                <title>\n",
                "                    Top\n",
                "                <bullet_list>\n",
                "                    <list_item classes=\"toctree-l1\">\n",
                "                        <compact_paragraph classes=\"toctree-l1\">\n",
                "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
                "                                A\n",
                "                        <bullet_list>\n",
            )
        );
    }

    /// An entry naming a document the environment no longer has (one
    /// removed after the toctree's document was read, which Sphinx does not
    /// re-read) is skipped with Sphinx's `KeyError` warning at
    /// `location=toctreenode`, worded by where `doc2path(ref, False)` falls:
    /// `excluded` when `exclude_patterns` matches it (a `**/` pattern also
    /// matching at the top level, as Sphinx's `Matcher` expands it),
    /// `non-included` when `include_patterns` does not, `non-existing`
    /// otherwise. Probed on Sphinx 9.1.0 (an incremental build after
    /// deleting `gone.rst`, and `_resolve_toctree` under each pair of
    /// patterns).
    #[test]
    fn toctree_entry_excluded_or_missing_warns() {
        let mut project = project(&[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n   gone\n"),
            ("a", "A\n=\n"),
            ("gone", "Gone\n====\n"),
        ]);
        project.env.clear_doc("gone");
        let expected = concat!(
            "    <section ids=\"index\" names=\"index\">\n",
            "        <title>\n",
            "            Index\n",
            "        <compound classes=\"toctree-wrapper\">\n",
            "            <compact_paragraph toctree=\"1\">\n",
            "                <bullet_list>\n",
            "                    <list_item classes=\"toctree-l1\">\n",
            "                        <compact_paragraph classes=\"toctree-l1\">\n",
            "                            <reference anchorname=\"\" internal=\"1\" refuri=\"\">\n",
            "                                A\n",
        );
        for (include, exclude, warning) in [
            (
                &["**"][..],
                &[][..],
                "toctree contains reference to non-existing document 'gone' [toc.not_readable]",
            ),
            (
                &["**"],
                &["gone.rst"],
                "toctree contains reference to excluded document 'gone' [toc.excluded]",
            ),
            (
                &["**"],
                &["**/gone.rst"],
                "toctree contains reference to excluded document 'gone' [toc.excluded]",
            ),
            (
                &["index.rst", "a.rst"],
                &[],
                "toctree contains reference to non-included document 'gone' [toc.not_included]",
            ),
        ] {
            let (index, warnings) = resolve_body_with(&project, "index", include, exclude);
            assert_eq!(index, expected, "include {include:?}, exclude {exclude:?}");
            assert_eq!(
                warnings,
                [format!("<SRCDIR>/index.rst:4: WARNING: {warning}")],
                "include {include:?}, exclude {exclude:?}"
            );
        }
    }
}
