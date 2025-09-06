use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// Custom serialization for PathBuf to handle cross-platform compatibility
fn serialize_pathbuf<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&path.to_string_lossy())
}

fn deserialize_pathbuf<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(PathBuf::from(s))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Source file path
    #[serde(
        serialize_with = "serialize_pathbuf",
        deserialize_with = "deserialize_pathbuf"
    )]
    pub source_path: PathBuf,

    /// Output file path
    #[serde(
        serialize_with = "serialize_pathbuf",
        deserialize_with = "deserialize_pathbuf"
    )]
    pub output_path: PathBuf,

    /// Document title
    pub title: String,

    /// Document content (parsed)
    pub content: DocumentContent,

    /// Document metadata
    pub metadata: DocumentMetadata,

    /// Rendered HTML content
    pub html: String,

    /// Source file modification time
    pub source_mtime: DateTime<Utc>,

    /// Build time
    pub build_time: DateTime<Utc>,

    /// Cross-references found in this document
    pub cross_refs: Vec<CrossReference>,

    /// Table of contents
    pub toc: Vec<TocEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocumentContent {
    RestructuredText(RstContent),
    Markdown(MarkdownContent),
    PlainText(String),
}

impl std::fmt::Display for DocumentContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DocumentContent::RestructuredText(rst) => write!(f, "{}", rst.raw),
            DocumentContent::Markdown(md) => write!(f, "{}", md.raw),
            DocumentContent::PlainText(text) => write!(f, "{}", text),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RstContent {
    /// Raw RST content
    pub raw: String,

    /// Parsed AST
    pub ast: Vec<RstNode>,

    /// Directives found in the document
    pub directives: Vec<RstDirective>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkdownContent {
    /// Raw Markdown content
    pub raw: String,

    /// Parsed AST
    pub ast: Vec<MarkdownNode>,

    /// Front matter
    pub front_matter: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentMetadata {
    /// Document author(s)
    pub authors: Vec<String>,

    /// Document creation date
    pub created: Option<DateTime<Utc>>,

    /// Document last modified date
    pub modified: Option<DateTime<Utc>>,

    /// Document tags
    pub tags: Vec<String>,

    /// Document category
    pub category: Option<String>,

    /// Custom metadata fields
    pub custom: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossReference {
    /// Reference type (doc, ref, func, class, etc.)
    pub ref_type: String,

    /// Reference target
    pub target: String,

    /// Reference text
    pub text: Option<String>,

    /// Line number where reference appears
    pub line_number: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocEntry {
    /// Entry title
    pub title: String,

    /// Entry level (1-6)
    pub level: usize,

    /// Anchor ID
    pub anchor: String,

    /// Line number
    pub line_number: usize,

    /// Child entries
    pub children: Vec<TocEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RstNode {
    Title {
        text: String,
        level: usize,
        line: usize,
    },
    Paragraph {
        content: String,
        line: usize,
    },
    CodeBlock {
        language: Option<String>,
        content: String,
        line: usize,
    },
    List {
        items: Vec<String>,
        ordered: bool,
        line: usize,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        line: usize,
    },
    Directive {
        name: String,
        args: Vec<String>,
        options: HashMap<String, String>,
        content: String,
        line: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarkdownNode {
    Heading {
        text: String,
        level: usize,
        line: usize,
    },
    Paragraph {
        content: String,
        line: usize,
    },
    CodeBlock {
        language: Option<String>,
        content: String,
        line: usize,
    },
    List {
        items: Vec<String>,
        ordered: bool,
        line: usize,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        line: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RstDirective {
    /// Directive name (e.g., "code-block", "toctree", "autoclass")
    pub name: String,

    /// Directive arguments
    pub args: Vec<String>,

    /// Directive options
    pub options: HashMap<String, String>,

    /// Directive content
    pub content: String,

    /// Line number where directive starts
    pub line: usize,
}

impl Document {
    pub fn new(source_path: PathBuf, output_path: PathBuf) -> Self {
        Self {
            source_path,
            output_path,
            title: String::new(),
            content: DocumentContent::PlainText(String::new()),
            metadata: DocumentMetadata::default(),
            html: String::new(),
            source_mtime: Utc::now(),
            build_time: Utc::now(),
            cross_refs: Vec::new(),
            toc: Vec::new(),
        }
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn add_cross_ref(&mut self, cross_ref: CrossReference) {
        self.cross_refs.push(cross_ref);
    }

    pub fn add_toc_entry(&mut self, entry: TocEntry) {
        self.toc.push(entry);
    }

    pub fn set_html(&mut self, html: String) {
        self.html = html;
        self.build_time = Utc::now();
    }
}

impl TocEntry {
    pub fn new(title: String, level: usize, anchor: String, line_number: usize) -> Self {
        Self {
            title,
            level,
            anchor,
            line_number,
            children: Vec::new(),
        }
    }

    pub fn add_child(&mut self, child: TocEntry) {
        self.children.push(child);
    }
}
