//! The build-path parser front end.
//!
//! Since M2 wave 3, RST goes through the docutils-fidelity parser in
//! [`crate::rst`] (sphinx mode): `Parser::parse` builds the [`Document`]
//! the pipeline consumes — title, toc (docutils-normalized anchors),
//! toctree entries with per-entry lines, directive/role records for the
//! validation system, and explicit-target labels for nitpicky mode — all
//! derived from the doctree at parse time and serializable for the
//! incremental cache. The M1 line-scanner is gone. Markdown stays on the
//! wave-6 (MyST) TODO path.

use anyhow::Result;
use log::debug;
use pulldown_cmark::{Event, Parser as MarkdownParser, Tag};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::config::BuildConfig;
use crate::doctree::{kinds, AttrValue, Doctree, Node, Span};
use crate::document::{
    Document, DocumentContent, LabelRecord, MarkdownContent, MarkdownNode, RstContent, TocEntry,
};
use crate::read::{self, ReadOutcome, ReadSettings};
use crate::rst;
use crate::transforms::state::NameTable;
use crate::utils;

pub struct Parser {
    /// `exclude_patterns` from the build configuration, which the `toctree`
    /// directive consults to tell an excluded target from a nonexisting one
    /// (sphinx `TocTree.parse_content` reads `self.config.exclude_patterns`).
    exclude_patterns: Vec<String>,
    /// The object-signature / py-domain configuration the read phase
    /// consumes (see [`crate::rst::ParseOptions::py`]), projected out of the
    /// build configuration once instead of per document.
    py: crate::py::PySigConfig,
    /// The project source directory (see
    /// [`crate::rst::ParseOptions::srcdir`]): the build path sets it via
    /// [`Parser::with_srcdir`] so the `include` directive resolves paths
    /// sphinx-style; a bare `Parser::new` keeps standalone behavior.
    srcdir: Option<std::path::PathBuf>,
    /// `source_encoding` (see [`crate::rst::ParseOptions::source_encoding`]).
    source_encoding: String,
    /// The read configuration ([`ReadSettings`]), projected out of the build
    /// configuration once.
    read: ReadSettings,
    /// The build configuration the Sphinx-mode parse reads
    /// ([`crate::rst::ReadConfig`]), projected out of it once.
    read_config: rst::ReadConfig,
}

/// Everything one source file's read produces: the pipeline's [`Document`]
/// and the read it was derived from ([`ReadOutcome`]: the doctree and the
/// docutils document state beside it). The build's read phase keeps both —
/// the doctree is what the environment layer (tocs, titles, domains) reads,
/// and what gets persisted per document; the state is what the merge's
/// stage M reads beside it. The parse's toctree, directive and role records
/// and its registry export move on to the `Document`, which the incremental
/// cache persists, so `outcome` holds empty stand-ins for them (see
/// [`ReadOutcome`]).
pub struct ParsedFile {
    pub document: Document,
    pub outcome: ReadOutcome,
}

impl Parser {
    pub fn new(config: &BuildConfig) -> Result<Self> {
        Ok(Self {
            exclude_patterns: config.exclude_patterns.clone(),
            py: crate::py::PySigConfig::from(config),
            srcdir: None,
            source_encoding: config.source_encoding.clone(),
            read: ReadSettings::from_config(config),
            read_config: rst::ReadConfig::from(config),
        })
    }

    /// Attach the project source directory (sphinx `env.srcdir`), enabling
    /// the `include` directive's sphinx-mode path resolution and its
    /// `included`/`dependencies` recording.
    pub fn with_srcdir(mut self, srcdir: std::path::PathBuf) -> Self {
        self.srcdir = Some(srcdir);
        self
    }

    pub fn parse(&self, file_path: &Path, content: &str, docname: &str) -> Result<Document> {
        Ok(self.parse_full(file_path, content, docname, None)?.document)
    }

    /// Read one source file, keeping the read ([`ParsedFile::outcome`]).
    ///
    /// `found_docs` is the project's full docname set (sphinx
    /// `env.found_docs`), which the `toctree` directive resolves its entries
    /// against; `None` parses without an environment (see
    /// [`crate::rst::ParseOptions::found_docs`]).
    pub fn parse_full(
        &self,
        file_path: &Path,
        content: &str,
        docname: &str,
        found_docs: Option<Arc<BTreeSet<String>>>,
    ) -> Result<ParsedFile> {
        let output_path = self.get_output_path(file_path)?;
        let mut document = Document::new(file_path.to_path_buf(), output_path);

        // Set source modification time
        document.source_mtime = utils::get_file_mtime(file_path)?;

        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");

        let outcome = match extension {
            "rst" => self.parse_rst_into(content, file_path, docname, found_docs, &mut document),
            "md" => {
                document.content = self.parse_markdown(content)?;
                document.title = "Untitled".to_string();
                ReadOutcome::without_parse(empty_doctree())
            }
            _ => {
                document.content = DocumentContent::PlainText(content.to_string());
                document.title = "Untitled".to_string();
                ReadOutcome::without_parse(empty_doctree())
            }
        };

        debug!(
            "Parsed document: {} ({} chars)",
            file_path.display(),
            content.len()
        );

        Ok(ParsedFile { document, outcome })
    }

    /// Read one RST source ([`read::read_document`]) and build `document`
    /// from the read: title, toc and labels from its doctree; toctrees,
    /// directive and role records and the registry export moved out of its
    /// [`rst::ParseOutput`]. Returns the read, whose doctree callers (tests
    /// included) can inspect for docname-carrying attrs
    /// (`pending_xref[refdoc]`, toctree `parent`) that don't otherwise
    /// survive onto `Document`.
    fn parse_rst_into(
        &self,
        content: &str,
        file_path: &Path,
        docname: &str,
        found_docs: Option<Arc<BTreeSet<String>>>,
        document: &mut Document,
    ) -> ReadOutcome {
        let mut outcome = read::read_document(
            content,
            &rst::ParseOptions {
                source_path: file_path.display().to_string(),
                sphinx: true,
                docname: docname.to_string(),
                found_docs,
                exclude_patterns: self.exclude_patterns.clone(),
                py: self.py.clone(),
                srcdir: self.srcdir.clone(),
                source_encoding: self.source_encoding.clone(),
                trace_ids: self.read.trace_ids,
                read: self.read_config.clone(),
            },
            &self.read,
        );
        {
            let root = &outcome.doctree().root;

            // Title: the first section title (the M1 scanner's "Untitled"
            // fallback preserved).
            document.title = first_section_title(root).unwrap_or_else(|| "Untitled".to_string());

            // Flat TOC; the builder's stack walk nests by level. Anchors are
            // the sections' docutils ids (make_id) — a verified Sphinx-parity
            // improvement over the M1 lowercase/space-hyphen slugs. Line
            // numbers come from the spans' stamped provenance.
            let mut toc = Vec::new();
            collect_toc(root, 1, &mut toc);
            document.toc = toc;

            // Explicit targets for nitpicky label resolution.
            let mut labels = Vec::new();
            collect_labels(root, &outcome.state().names, &mut labels);
            document.labels = labels;
        }

        let output = &mut outcome.output;
        document.toctrees = std::mem::take(&mut output.toctrees);
        document.directive_records = std::mem::take(&mut output.directive_records);
        document.role_records = std::mem::take(&mut output.role_records);
        document.registry = std::mem::take(&mut output.registry);

        document.content = DocumentContent::RestructuredText(RstContent {
            raw: content.to_string(),
            ast: Vec::new(),
            directives: Vec::new(),
        });

        outcome
    }

    fn parse_markdown(&self, content: &str) -> Result<DocumentContent> {
        let mut nodes = Vec::new();
        let parser = MarkdownParser::new(content);
        let current_line = 1;

        for event in parser {
            match event {
                Event::Start(Tag::Heading { .. }) => {
                    // We'll handle this in the text event
                }
                Event::End(_) => {
                    // Handle end tags generically
                }
                Event::Start(Tag::Paragraph) => {
                    // Start of paragraph
                }
                Event::Start(Tag::CodeBlock(_)) => {
                    // Start of code block
                }
                Event::Text(text) => {
                    // Handle text content based on context
                    nodes.push(MarkdownNode::Paragraph {
                        content: text.to_string(),
                        line: current_line,
                    });
                }
                Event::Code(_code) => {
                    // Inline code
                }
                _ => {
                    // Handle other events as needed
                }
            }
        }

        Ok(DocumentContent::Markdown(MarkdownContent {
            raw: content.to_string(),
            ast: nodes,
            front_matter: None, // TODO: Parse YAML front matter
        }))
    }

    fn get_output_path(&self, source_path: &Path) -> Result<std::path::PathBuf> {
        let mut output_path = source_path.to_path_buf();
        output_path.set_extension("html");
        Ok(output_path)
    }
}

/// The doctree of a source file this crate has no real parser for yet
/// (Markdown, plain text): an empty `document`. It is deliberately empty
/// rather than a guess — the environment layer reading it will report no
/// sections, no toc entries and no toctrees, which is exactly what this
/// crate currently knows about such a file.
fn empty_doctree() -> Doctree {
    Doctree {
        root: Node::elem(kinds::DOCUMENT, Span::ZERO),
        sources: vec!["<document>".to_string()],
    }
}

fn first_section_title(root: &Node) -> Option<String> {
    for child in &root.children {
        if child.kind == kinds::SECTION {
            for c in &child.children {
                if c.kind == kinds::TITLE {
                    return Some(c.astext());
                }
            }
        }
    }
    None
}

fn collect_toc(node: &Node, level: usize, out: &mut Vec<TocEntry>) {
    for child in &node.children {
        if child.kind != kinds::SECTION {
            continue;
        }
        if let Some(title) = child.children.iter().find(|c| c.kind == kinds::TITLE) {
            out.push(TocEntry {
                title: title.astext(),
                level,
                anchor: child.attrs.ids.first().cloned().unwrap_or_default(),
                line_number: title.span.line as usize,
                children: Vec::new(),
            });
        }
        collect_toc(child, level + 1, out);
    }
}

/// The names of the targets under `root`, in document order, each at its
/// target's line. A target `PropagateTargets` (260) gave its ids and names
/// to the node after it keeps its first former id as its `refid`: its
/// names are the explicit ones `names` binds to that id.
fn collect_labels(root: &Node, names: &NameTable, out: &mut Vec<LabelRecord>) {
    let mut explicit: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, entry) in names.iter() {
        if let Some(id) = entry.id.as_deref().filter(|_| entry.explicit) {
            explicit.entry(id).or_default().push(name);
        }
    }
    collect_target_names(root, &explicit, out);
}

/// [`collect_labels`] under `node`, with `explicit` the explicit names
/// bound to each id.
fn collect_target_names(
    node: &Node,
    explicit: &HashMap<&str, Vec<&str>>,
    out: &mut Vec<LabelRecord>,
) {
    for child in &node.children {
        if child.kind == kinds::TARGET {
            let propagated = match child.get("refid") {
                Some(AttrValue::Str(refid))
                    if child.attrs.names.is_empty() && child.attrs.ids.is_empty() =>
                {
                    explicit.get(refid.as_str())
                }
                _ => None,
            };
            let target_names = match propagated {
                Some(names) => names.clone(),
                None => child.attrs.names.iter().map(String::as_str).collect(),
            };
            out.extend(target_names.into_iter().map(|name| LabelRecord {
                name: name.to_string(),
                line: child.span.line as usize,
            }));
        }
        collect_target_names(child, explicit, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BuildConfig;
    use crate::doctree::AttrValue;

    fn parse_doc(content: &str) -> Document {
        let parser = Parser::new(&BuildConfig::default()).unwrap();
        // Bypass mtime lookup by parsing through the internals.
        let mut document = Document::new("test.rst".into(), "test.html".into());
        parser.parse_rst_into(content, Path::new("test.rst"), "index", None, &mut document);
        document
    }

    /// First node of the given kind, depth-first.
    fn find_by_kind<'a>(node: &'a Node, kind: &str) -> Option<&'a Node> {
        if node.kind == kind {
            return Some(node);
        }
        node.children.iter().find_map(|c| find_by_kind(c, kind))
    }

    #[test]
    fn hyphenated_directive_is_recorded() {
        let doc = parse_doc(".. code-block:: python\n\n   x = 1\n");
        assert_eq!(doc.directive_records.len(), 1);
        assert_eq!(doc.directive_records[0].name, "code-block");
        assert_eq!(doc.directive_records[0].arguments, vec!["python"]);
        assert_eq!(doc.directive_records[0].content, "x = 1");
        assert_eq!(doc.directive_records[0].line, 1);
    }

    #[test]
    fn domain_directive_is_recorded() {
        let doc = parse_doc(".. py:function:: foo(x)\n\n   Does foo.\n");
        assert_eq!(doc.directive_records.len(), 1);
        assert_eq!(doc.directive_records[0].name, "py:function");
    }

    #[test]
    fn tab_indented_directive_content_does_not_panic() {
        let doc = parse_doc(".. note::\n\n\tshort\n");
        assert_eq!(doc.directive_records.len(), 1);
        assert_eq!(doc.directive_records[0].content, "short");
    }

    #[test]
    fn title_and_toc_from_sections() {
        let doc = parse_doc("Title\n=====\n\nBody.\n\nSub\n---\n\nMore.\n");
        assert_eq!(doc.title, "Title");
        assert_eq!(doc.toc.len(), 2);
        assert_eq!(doc.toc[0].level, 1);
        assert_eq!(doc.toc[0].anchor, "title");
        assert_eq!(doc.toc[1].level, 2);
        assert_eq!(doc.toc[1].title, "Sub");
        assert_eq!(doc.toc[1].line_number, 6);
    }

    #[test]
    fn toctree_entries_recorded_with_lines() {
        let doc = parse_doc(
            "Title\n=====\n\n.. toctree::\n   :maxdepth: 2\n   :glob:\n\n   installation\n   Linked <other>\n",
        );
        assert_eq!(doc.toctrees.len(), 1);
        let t = &doc.toctrees[0];
        assert!(t.glob);
        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.entries[0].target, "installation");
        assert_eq!(t.entries[0].line, 8);
        assert_eq!(t.entries[1].title.as_deref(), Some("Linked"));
        assert_eq!(t.entries[1].target, "other");
    }

    #[test]
    fn labels_and_roles_recorded() {
        let doc =
            parse_doc(".. _setup-label:\n\nSee :ref:`setup-label` and :doc:`installation`.\n");
        assert_eq!(doc.labels.len(), 1);
        assert_eq!(doc.labels[0].name, "setup-label");
        assert_eq!(doc.labels[0].line, 1);
        let names: Vec<&str> = doc.role_records.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["ref", "doc"]);
        assert_eq!(doc.role_records[0].target, "setup-label");
        assert_eq!(doc.role_records[0].line, 3);
    }

    /// Each target of a chain keeps its own label once `PropagateTargets`
    /// (260) has moved the chain's names to the section; an external
    /// target, which stays, keeps its name.
    #[test]
    fn labels_follow_propagated_targets() {
        let parser = Parser::new(&BuildConfig::default()).unwrap();
        let mut doc = Document::new("test.rst".into(), "test.html".into());
        let read = parser.parse_rst_into(
            ".. _a:\n.. _b:\n\nSection\n=======\n\n.. _ext: http://x\n\nText.\n",
            Path::new("test.rst"),
            "index",
            None,
            &mut doc,
        );
        let section = find_by_kind(&read.doctree().root, kinds::SECTION).expect("a section");
        assert_eq!(section.attrs.names, ["section", "b", "a"]);
        let labels: Vec<(&str, usize)> = doc
            .labels
            .iter()
            .map(|label| (label.name.as_str(), label.line))
            .collect();
        assert_eq!(labels, [("a", 1), ("b", 2), ("ext", 7)]);
    }

    /// Nested docs (e.g. `guide/install.rst`) must carry their real
    /// root-relative docname into `pending_xref[refdoc]` and the toctree
    /// `parent` attr — not the bare file stem (`install`).
    #[test]
    fn real_docname_threads_into_refdoc_and_toctree_parent() {
        let parser = Parser::new(&BuildConfig::default()).unwrap();
        let mut document = Document::new("guide/install.rst".into(), "guide/install.html".into());
        let outcome = parser.parse_rst_into(
            ".. toctree::\n\n   other\n\nSee :doc:`other`.\n",
            Path::new("guide/install.rst"),
            "guide/install",
            None,
            &mut document,
        );
        let doctree = outcome.doctree();

        let toctree = find_by_kind(&doctree.root, "toctree").expect("toctree node");
        assert_eq!(
            toctree.get("parent"),
            Some(&AttrValue::Str("guide/install".to_string()))
        );

        let xref = find_by_kind(&doctree.root, "pending_xref").expect("pending_xref node");
        assert_eq!(
            xref.get("refdoc"),
            Some(&AttrValue::Str("guide/install".to_string()))
        );
    }

    /// The id registry's nameids table (name -> (id, explicit)) must
    /// survive parse_document_full as `ParseOutput.registry` — wave 4's
    /// std-domain label harvest needs it after the registry itself drops.
    #[test]
    fn registry_export_carries_nameids_with_explicitness() {
        let output = rst::parse_rst_full(
            "Section\n=======\n\n.. _tgt:\n\nBody.\n",
            &rst::ParseOptions {
                source_path: "<snippet>".to_string(),
                sphinx: true,
                docname: "index".to_string(),
                exclude_patterns: Vec::new(),
                py: Default::default(),
                srcdir: None,
                found_docs: None,
                ..Default::default()
            },
        );

        let tgt = output
            .registry
            .nameids
            .iter()
            .find(|(name, _, _)| name == "tgt")
            .expect("tgt registered");
        assert_eq!(tgt, &("tgt".to_string(), Some("tgt".to_string()), true));

        let section = output
            .registry
            .nameids
            .iter()
            .find(|(name, _, _)| name == "section")
            .expect("section registered");
        assert_eq!(
            section,
            &("section".to_string(), Some("section".to_string()), false)
        );
    }

    /// A parsed file carries its read for the merge: the doctree, stamped
    /// as every Sphinx document is, and the docutils document state beside
    /// it (here the reporter's record of the short underline), while the
    /// records the pipeline reads off the `Document` move there.
    #[test]
    fn a_parsed_file_carries_its_read() {
        let parser = Parser::new(&BuildConfig::default()).unwrap();
        let mut document = Document::new("index.rst".into(), "index.html".into());
        let outcome = parser.parse_rst_into(
            "Title\n====\n\n.. toctree::\n\n   other\n",
            Path::new("index.rst"),
            "index",
            None,
            &mut document,
        );
        assert_eq!(
            outcome.doctree().root.get("translation_progress"),
            Some(&AttrValue::Str("{'total': 0, 'translated': 0}".to_string()))
        );
        assert_eq!(outcome.state().log.len(), 1);
        assert_eq!(document.toctrees.len(), 1);
        assert!(outcome.output.toctrees.is_empty());
    }
}
