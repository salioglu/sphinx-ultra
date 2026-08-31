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
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use crate::config::BuildConfig;
use crate::doctree::{kinds, Doctree, Node, Span};
use crate::document::{
    Document, DocumentContent, LabelRecord, MarkdownContent, MarkdownNode, RstContent, TocEntry,
};
use crate::rst;
use crate::utils;

pub struct Parser {
    /// `exclude_patterns` from the build configuration, which the `toctree`
    /// directive consults to tell an excluded target from a nonexisting one
    /// (sphinx `TocTree.parse_content` reads `self.config.exclude_patterns`).
    exclude_patterns: Vec<String>,
}

/// Everything one source file's parse produces: the pipeline's [`Document`]
/// and the doctree it was derived from. The build's read phase keeps both —
/// the doctree is what the environment layer (tocs, titles, domains) reads,
/// and what gets persisted per document.
pub struct ParsedFile {
    pub document: Document,
    pub doctree: Doctree,
}

impl Parser {
    pub fn new(config: &BuildConfig) -> Result<Self> {
        Ok(Self {
            exclude_patterns: config.exclude_patterns.clone(),
        })
    }

    pub fn parse(&self, file_path: &Path, content: &str, docname: &str) -> Result<Document> {
        Ok(self.parse_full(file_path, content, docname, None)?.document)
    }

    /// Parse one source file, keeping the doctree.
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

        let doctree = match extension {
            "rst" => self.parse_rst_into(content, file_path, docname, found_docs, &mut document),
            "md" => {
                document.content = self.parse_markdown(content)?;
                document.title = "Untitled".to_string();
                empty_doctree()
            }
            _ => {
                document.content = DocumentContent::PlainText(content.to_string());
                document.title = "Untitled".to_string();
                empty_doctree()
            }
        };

        debug!(
            "Parsed document: {} ({} chars)",
            file_path.display(),
            content.len()
        );

        Ok(ParsedFile { document, doctree })
    }

    /// Returns the parsed doctree so callers (tests included) can inspect
    /// docname-carrying attrs (`pending_xref[refdoc]`, toctree `parent`)
    /// that don't otherwise survive onto `Document`.
    fn parse_rst_into(
        &self,
        content: &str,
        file_path: &Path,
        docname: &str,
        found_docs: Option<Arc<BTreeSet<String>>>,
        document: &mut Document,
    ) -> Doctree {
        let output = rst::parse_rst_full(
            content,
            &rst::ParseOptions {
                source_path: file_path.display().to_string(),
                sphinx: true,
                docname: docname.to_string(),
                found_docs,
                exclude_patterns: self.exclude_patterns.clone(),
            },
        );
        let line_starts = line_start_offsets(content);
        {
            let root = &output.doctree.root;

            // Title: the first section title (the M1 scanner's "Untitled"
            // fallback preserved).
            document.title = first_section_title(root).unwrap_or_else(|| "Untitled".to_string());

            // Flat TOC; the builder's stack walk nests by level. Anchors are
            // the sections' docutils ids (make_id) — a verified Sphinx-parity
            // improvement over the M1 lowercase/space-hyphen slugs.
            let mut toc = Vec::new();
            collect_toc(root, 1, &line_starts, &mut toc);
            document.toc = toc;

            // Explicit targets for nitpicky label resolution.
            let mut labels = Vec::new();
            collect_labels(root, &line_starts, &mut labels);
            document.labels = labels;
        }

        document.toctrees = output.toctrees;
        document.directive_records = output.directive_records;
        document.role_records = output.role_records;
        document.registry = output.registry;

        document.content = DocumentContent::RestructuredText(RstContent {
            raw: content.to_string(),
            ast: Vec::new(),
            directives: Vec::new(),
        });

        output.doctree
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

/// Byte offsets of each line start, for span-to-line mapping.
fn line_start_offsets(content: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            starts.push((i + 1) as u32);
        }
    }
    starts
}

/// 1-based line of a byte offset.
fn line_of_offset(line_starts: &[u32], offset: u32) -> usize {
    match line_starts.binary_search(&offset) {
        Ok(i) => i + 1,
        Err(i) => i,
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

fn collect_toc(node: &Node, level: usize, line_starts: &[u32], out: &mut Vec<TocEntry>) {
    for child in &node.children {
        if child.kind != kinds::SECTION {
            continue;
        }
        if let Some(title) = child.children.iter().find(|c| c.kind == kinds::TITLE) {
            out.push(TocEntry {
                title: title.astext(),
                level,
                anchor: child.attrs.ids.first().cloned().unwrap_or_default(),
                line_number: line_of_offset(line_starts, title.span.start),
                children: Vec::new(),
            });
        }
        collect_toc(child, level + 1, line_starts, out);
    }
}

fn collect_labels(node: &Node, line_starts: &[u32], out: &mut Vec<LabelRecord>) {
    for child in &node.children {
        if child.kind == kinds::TARGET && !child.attrs.names.is_empty() {
            for name in &child.attrs.names {
                out.push(LabelRecord {
                    name: name.clone(),
                    line: line_of_offset(line_starts, child.span.start),
                });
            }
        }
        collect_labels(child, line_starts, out);
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

    /// Nested docs (e.g. `guide/install.rst`) must carry their real
    /// root-relative docname into `pending_xref[refdoc]` and the toctree
    /// `parent` attr — not the bare file stem (`install`).
    #[test]
    fn real_docname_threads_into_refdoc_and_toctree_parent() {
        let parser = Parser::new(&BuildConfig::default()).unwrap();
        let mut document = Document::new("guide/install.rst".into(), "guide/install.html".into());
        let doctree = parser.parse_rst_into(
            ".. toctree::\n\n   other\n\nSee :doc:`other`.\n",
            Path::new("guide/install.rst"),
            "guide/install",
            None,
            &mut document,
        );

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
                found_docs: None,
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
}
