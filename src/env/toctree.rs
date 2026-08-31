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
//!
//! [`document_title`] is the neighbouring `TitleCollector.process_doc`
//! (`collectors/title.py:27`), which shares this module's
//! `SphinxContentsFilter` port.
//!
//! NOT here (later wave-4 tasks): `assign_section_numbers` /
//! `assign_figure_numbers` (they write `toc_secnumbers`/`toc_fignumbers` and
//! stamp `secnumber` onto the references this module builds), and the
//! `addnodes.desc` branch of `build_toc` — no `desc` nodes exist in the
//! doctree until the object-description directives land.

use std::collections::BTreeSet;

use crate::doctree::{kinds, AttrValue, Doctree, Node, Span};
use crate::env::BuildEnvironment;
use crate::matching;

/// Sphinx's `StandardDomain._virtual_doc_names` (`domains/std/__init__.py:784`):
/// docnames that resolve even though no source file produces them.
const VIRTUAL_DOC_NAMES: [&str; 3] = ["genindex", "py-modindex", "search"];

// ---------------------------------------------------------------------------
// 1. Entry resolution (sphinx/directives/other.py TocTree.parse_content)
// ---------------------------------------------------------------------------

/// `toctree['entries']` (title, ref) pairs plus `toctree['includefiles']`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedEntries {
    /// `(title, ref)`; `ref` is a resolved docname, an external URL, or the
    /// literal `self`.
    pub entries: Vec<(Option<String>, String)>,
    /// The subset of `entries` that are real documents, in the same order.
    pub includefiles: Vec<String>,
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
/// Port of `TocTree.parse_content` (`sphinx/directives/other.py:88-179`),
/// minus its four `logger.warning` calls: toctree diagnostics are still
/// produced by the M1-parity pass in `builder::validate_documents`, and
/// emitting them from here too would double every pinned message. Warning
/// parity (including the `excluded` / `duplicated entry` cases this port
/// silently drops) is a later wave-4 task.
///
/// `content` are the directive's content lines with blank ones already
/// removed; `docname` is the containing document; `found_docs` is every
/// docname the project discovered.
pub fn resolve_entries(
    content: &[String],
    docname: &str,
    glob: bool,
    reversed: bool,
    found_docs: &BTreeSet<String>,
    source_suffixes: &[&str],
) -> ResolvedEntries {
    // `all_docnames` is consumed as entries claim documents (so a glob never
    // re-lists what an earlier entry named); `frozen` keeps the full set for
    // the existence check. The current document is not a candidate: a
    // toctree entry naming its own document is "nonexisting" to Sphinx.
    let mut all: BTreeSet<&str> = found_docs.iter().map(String::as_str).collect();
    all.extend(VIRTUAL_DOC_NAMES);
    all.remove(docname);
    let frozen = all.clone();

    let mut out = ResolvedEntries::default();
    for entry in content {
        if entry.is_empty() {
            continue;
        }
        let explicit = split_explicit_title(entry);
        let url_match = is_url(entry);

        if glob && has_glob_metachars(entry) && explicit.is_none() && !url_match {
            let pattern = docname_join(docname, entry);
            let matched: Vec<String> = all
                .iter()
                .filter(|d| !VIRTUAL_DOC_NAMES.contains(d))
                .filter(|d| matching::pattern_match(d, &pattern).unwrap_or(false))
                .map(|d| (*d).to_string())
                .collect(); // BTreeSet iteration order == sorted(), as sphinx does
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
            // sphinx: `toctree contains reference to {excluded,nonexisting}
            // document %r` + note_reread(). See the fn doc comment.
            continue;
        }
        // sphinx warns `duplicated entry found in toctree: %s` when the
        // document was already claimed, but appends either way.
        all.remove(resolved.as_str());
        out.entries.push((title, resolved.clone()));
        out.includefiles.push(resolved);
    }

    if reversed {
        out.entries.reverse();
        out.includefiles.reverse();
    }
    out
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

/// Sphinx `url_re` (`(?P<schema>.+)://.*`, anchored with `.match`): any
/// `://` preceded by at least one character.
fn is_url(entry: &str) -> bool {
    entry.find("://").is_some_and(|i| i >= 1)
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

    #[test]
    fn entries_resolve_relative_absolute_and_self_targets() {
        let found = docs(&["index", "sub/b", "sub/c", "a"]);
        let resolved = resolve_entries(
            &[
                "c".to_string(),
                "/a".to_string(),
                "self".to_string(),
                "https://example.invalid/x".to_string(),
                "missing".to_string(),
                "sub/b".to_string(),
            ],
            "sub/b",
            false,
            false,
            &found,
            &[".rst"],
        );
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
    }

    #[test]
    fn glob_entries_expand_sorted_and_skip_virtual_docs() {
        let found = docs(&["index", "pages/a", "pages/b"]);
        let resolved = resolve_entries(
            &["pages/*".to_string()],
            "index",
            true,
            false,
            &found,
            &[".rst"],
        );
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
    }

    #[test]
    fn explicit_titles_and_suffixes() {
        let found = docs(&["index", "other"]);
        let resolved = resolve_entries(
            &["Linked <other.rst>".to_string(), "<foo>".to_string()],
            "index",
            false,
            false,
            &found,
            &[".rst"],
        );
        assert_eq!(
            resolved.entries,
            vec![(Some("Linked".to_string()), "other".to_string())],
            "`<foo>` is a literal (missing) target, not an empty title"
        );
        assert_eq!(
            resolved.entries_attr(),
            AttrValue::List(vec!["('Linked', 'other')".to_string()])
        );
    }

    #[test]
    fn reversed_flips_both_lists() {
        let found = docs(&["index", "a", "b"]);
        let resolved = resolve_entries(
            &["a".to_string(), "b".to_string()],
            "index",
            false,
            true,
            &found,
            &[".rst"],
        );
        assert_eq!(
            resolved.includefiles,
            vec!["b".to_string(), "a".to_string()]
        );
    }

    #[test]
    fn virtual_docnames_are_valid_entries() {
        let found = docs(&["index"]);
        let resolved = resolve_entries(
            &["genindex".to_string()],
            "index",
            false,
            false,
            &found,
            &[".rst"],
        );
        assert_eq!(resolved.includefiles, vec!["genindex".to_string()]);
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
