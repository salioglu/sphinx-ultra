//! Typed doctree IR with docutils-equivalent node semantics (M2 wave 1).
//!
//! Design (recorded in docs/superpowers/plans/2026-08-07-m2-wave-map.md):
//! docutils-mirror generic node — node identity and attributes are data, not
//! Rust types, so the pseudo-XML parity serializer is a direct dump and
//! docutils transforms/writers port line-by-line. One `Node` struct covers
//! every element type; `kind` holds the docutils tagname from [`kinds`].
//!
//! Source spans are structural: every node carries a [`Span`] with real
//! `(source, line)` provenance plus the byte range of its text in the
//! parser's processed source (docutils itself only keeps `(source, line)`).
//! Spans are line-granular; the inline parser stamps its nodes with the
//! enclosing text block's span.

pub mod ids;
mod intern;
pub mod kinds;
pub mod messages;
pub mod pformat;

pub(crate) use intern::intern;

use serde::{Deserialize, Deserializer, Serialize};

/// Source provenance of a node: which source it came from, the 1-based
/// line docutils would report for it, and the byte range of its text.
///
/// `source` indexes the per-doctree source table ([`Doctree::sources`];
/// 0 is the document itself, included files push further entries), and
/// `start..end` is a byte range into the parser's *processed* text of that
/// source (tab-expanded, trailing whitespace stripped — the text the
/// parser actually consumed).
///
/// `line` is 1-based within `source`; 0 means unknown. It carries the
/// docutils reporting convention the env layer's warnings need: every node
/// is stamped with the first line of its span, except a `section`, which
/// is stamped one past that (docutils creates a section only once the
/// state machine has consumed the title's underline).
///
/// `Default` is [`Span::ZERO`]: source 0, line 0 (unknown), empty range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub source: u16,
    pub line: u32,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const ZERO: Span = Span {
        source: 0,
        line: 0,
        start: 0,
        end: 0,
    };
}

/// Attribute value. docutils attribute dicts hold ints and strings for
/// everything wave 1 emits; docutils' five *universal* list attributes
/// (`ids`, `names`, ...) live in [`Attrs`]' typed fields instead.
///
/// [`AttrValue::List`] covers the element-specific list-valued attributes
/// docutils renders through the same `serial_escape`-and-join path as the
/// universal ones (`toctree[entries]`, `toctree[includefiles]`) — storing
/// them as a list rather than a pre-joined string keeps the escaping in
/// `pformat` (one implementation, not one per producer) and keeps the items
/// readable by consumers such as `env::toctree::note_toctree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttrValue {
    Int(i64),
    Str(String),
    List(Vec<String>),
}

/// docutils' universal list attributes (`basic_attributes` + `backrefs`) as
/// typed fields, plus an open, name-sorted list for everything else
/// (`refuri`, `enumtype`, `level`, …). `pformat` merges both sets and prints
/// all pairs in one alphabetical sequence, exactly like docutils `attlist()`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attrs {
    pub ids: Vec<String>,
    pub names: Vec<String>,
    pub dupnames: Vec<String>,
    pub classes: Vec<String>,
    pub backrefs: Vec<String>,
    /// Kept sorted by key; use [`Node::set`] to maintain the invariant.
    #[serde(
        serialize_with = "intern::serialize_extra",
        deserialize_with = "intern::deserialize_extra"
    )]
    pub extra: Vec<(&'static str, AttrValue)>,
}

/// The [`Attrs::extra`] key under which a node keeps docutils'
/// `Node.rawsource` — the source text it was parsed from — where a read
/// transform reads it back: a `substitution_reference` (the reference as
/// written) and a `substitution_definition` (its explicit-markup block),
/// from which Substitutions builds its `problematic` and a circular
/// definition's literal (`docutils/transforms/references.py:702-704,
/// 735-738`). docutils keeps `rawsource` beside `Element.attributes`, not
/// in them, so `attlist()` never prints it, and neither does
/// [`Node::pformat`].
pub const RAWSOURCE: &str = "rawsource";

/// One doctree node. Element nodes have `text == None`; text leaves have
/// `kind == kinds::TEXT`, `Some(text)`, and no children or attributes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    #[serde(serialize_with = "intern::serialize_str")]
    pub kind: &'static str,
    pub span: Span,
    pub text: Option<String>,
    pub attrs: Attrs,
    pub children: Vec<Node>,
}

/// Owned mirror of [`Node`] whose only job is to let `#[derive(Deserialize)]`
/// do the field-by-field work. Deriving `Deserialize` directly on `Node`
/// hits a serde-derive limitation: a field whose type literally names the
/// `'static` lifetime (`kind: &'static str`) makes the derived impl require
/// `'de: 'static` instead of the unconstrained `impl<'de> Deserialize<'de>`
/// every other caller (including bincode decoding from a borrowed `&[u8]`)
/// needs. Deserializing into this all-owned shadow and interning `kind`
/// afterward sidesteps it.
#[derive(Deserialize)]
struct NodeShadow {
    kind: String,
    span: Span,
    text: Option<String>,
    attrs: Attrs,
    children: Vec<Node>,
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let shadow = NodeShadow::deserialize(deserializer)?;
        let kind = intern(&shadow.kind).map_err(serde::de::Error::custom)?;
        Ok(Node {
            kind,
            span: shadow.span,
            text: shadow.text,
            attrs: shadow.attrs,
            children: shadow.children,
        })
    }
}

impl Node {
    pub fn elem(kind: &'static str, span: Span) -> Node {
        Node {
            kind,
            span,
            text: None,
            attrs: Attrs::default(),
            children: Vec::new(),
        }
    }

    pub fn text_node(s: impl Into<String>, span: Span) -> Node {
        Node {
            kind: kinds::TEXT,
            span,
            text: Some(s.into()),
            attrs: Attrs::default(),
            children: Vec::new(),
        }
    }

    /// docutils `Element.copy()`: same kind, span and attributes, but **no
    /// children** (docutils copies `rawsource` and attributes only;
    /// `deepcopy` is the one that takes the subtree).
    pub fn shallow_copy(&self) -> Node {
        Node {
            kind: self.kind,
            span: self.span,
            text: self.text.clone(),
            attrs: self.attrs.clone(),
            children: Vec::new(),
        }
    }

    /// Set a scalar attribute, keeping `attrs.extra` sorted by key and
    /// overwriting any existing value for the same key.
    pub fn set(&mut self, key: &'static str, value: AttrValue) {
        match self.attrs.extra.binary_search_by(|(k, _)| k.cmp(&key)) {
            Ok(i) => self.attrs.extra[i].1 = value,
            Err(i) => self.attrs.extra.insert(i, (key, value)),
        }
    }

    pub fn get(&self, key: &'static str) -> Option<&AttrValue> {
        self.attrs
            .extra
            .binary_search_by(|(k, _)| k.cmp(&key))
            .ok()
            .map(|i| &self.attrs.extra[i].1)
    }

    /// Concatenated text of all text descendants.
    ///
    /// Wave-1 simplification of docutils `Node.astext()`: children join with
    /// `""` (docutils joins with a per-element `child_text_separator`, which
    /// only matters for elements wave 1 never calls `astext` on — revisit in
    /// wave 2 when inline nodes need `" "` and table cells need `"\n\n"`).
    pub fn astext(&self) -> String {
        match &self.text {
            Some(t) => t.clone(),
            None => self.children.iter().map(Node::astext).collect(),
        }
    }

    /// Byte-parity pseudo-XML rendering (docutils `document.pformat()`).
    pub fn pformat(&self) -> String {
        pformat::pformat(self)
    }
}

/// One parsed document. `root.kind == kinds::DOCUMENT`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Doctree {
    pub root: Node,
    /// The source table `Span.source` indexes: one path per source the
    /// parse consumed. Entry 0 is the document's own path; sub-parses over
    /// lifted text (table cells) and, later, included files push further
    /// entries. `#[serde(default = "default_sources")]` is the standard
    /// hedge for a field added after data in this shape might already
    /// exist — but note it only rescues *self-describing* formats (e.g.
    /// JSON) from a missing field: bincode's wire format has no
    /// field-presence framing, so a bincode blob that predates this field
    /// would still fail to decode (`UnexpectedEnd`) rather than fall back
    /// to the default. No such blob existed before the field did, so that
    /// gap isn't live today.
    #[serde(default = "default_sources")]
    pub sources: Vec<String>,
}

impl Doctree {
    /// The `(source path, line)` a warning about a node should report —
    /// docutils' `(node.source, node.line)`. Line 0 means unknown; a
    /// `source` the table doesn't know (a foreign or hand-built span)
    /// falls back to the document's own path.
    pub fn source_and_line(&self, span: Span) -> (&str, u32) {
        let path = self
            .sources
            .get(span.source as usize)
            .or_else(|| self.sources.first())
            .map(String::as_str)
            .unwrap_or("<document>");
        (path, span.line)
    }
}

fn default_sources() -> Vec<String> {
    vec!["<document>".to_string()]
}

/// Encode a doctree to its bincode wire format (config: `standard()`).
/// Infallible in practice: every field either derives `Serialize` or goes
/// through the interner's plain string encoding, neither of which can fail.
pub fn to_bincode(doctree: &Doctree) -> Vec<u8> {
    bincode::serde::encode_to_vec(doctree, bincode::config::standard())
        .expect("Doctree encoding is infallible")
}

/// Decode a doctree previously written by [`to_bincode`]. Also the entry
/// point for bytes that *weren't* — a corrupted file, or a stale/foreign
/// blob from a version-skewed cache — which can fail for the usual decode
/// reasons and additionally once decoding would intern more than
/// `intern`'s bounded table allows (see `src/doctree/intern.rs`).
pub fn from_bincode(bytes: &[u8]) -> anyhow::Result<Doctree> {
    let (doctree, _consumed): (Doctree, usize) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard())?;
    Ok(doctree)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `#[serde(default = "default_sources")]` on `Doctree::sources` only
    /// rescues a missing field for *self-describing* formats (see the
    /// field's doc comment) — bincode's positional wire format has no way
    /// to signal "field absent, use the default" for a trailing struct
    /// field, so this property can only be demonstrated through JSON here.
    #[test]
    fn doctree_deserialize_defaults_sources_when_field_absent_in_json() {
        let json = r#"{"root":{"kind":"document","span":{"source":0,"line":0,"start":0,"end":0},"text":null,"attrs":{"ids":[],"names":[],"dupnames":[],"classes":[],"backrefs":[],"extra":{}},"children":[]}}"#;

        let restored: Doctree = serde_json::from_str(json).expect("json without sources decodes");

        assert_eq!(restored.sources, vec!["<document>".to_string()]);
        assert_eq!(restored.root.kind, kinds::DOCUMENT);
    }

    #[test]
    fn elem_constructs_with_kind_and_span() {
        let n = Node::elem(
            kinds::PARAGRAPH,
            Span {
                source: 0,
                line: 1,
                start: 0,
                end: 10,
            },
        );
        assert_eq!(n.kind, "paragraph");
        assert!(n.text.is_none());
        assert!(n.children.is_empty());
    }

    #[test]
    fn text_node_holds_text() {
        let t = Node::text_node(
            "hello",
            Span {
                source: 0,
                line: 1,
                start: 0,
                end: 5,
            },
        );
        assert_eq!(t.kind, kinds::TEXT);
        assert_eq!(t.text.as_deref(), Some("hello"));
    }

    #[test]
    fn set_keeps_extra_sorted_and_get_finds() {
        let mut n = Node::elem(kinds::TARGET, Span::ZERO);
        n.set("refuri", AttrValue::Str("https://x/".into()));
        n.set("anonymous", AttrValue::Int(1));
        assert_eq!(n.attrs.extra[0].0, "anonymous");
        assert_eq!(n.get("refuri"), Some(&AttrValue::Str("https://x/".into())));
        n.set("refuri", AttrValue::Str("https://y/".into()));
        assert_eq!(n.attrs.extra.len(), 2);
        assert_eq!(n.get("refuri"), Some(&AttrValue::Str("https://y/".into())));
    }

    #[test]
    fn source_and_line_reads_the_table_and_falls_back_to_entry_0() {
        let tree = Doctree {
            root: Node::elem(kinds::DOCUMENT, Span::ZERO),
            sources: vec!["a.rst".to_string(), "b.rst".to_string()],
        };
        let span = |source, line| Span {
            source,
            line,
            start: 0,
            end: 0,
        };
        assert_eq!(tree.source_and_line(span(0, 3)), ("a.rst", 3));
        assert_eq!(tree.source_and_line(span(1, 7)), ("b.rst", 7));
        assert_eq!(
            tree.source_and_line(span(9, 2)),
            ("a.rst", 2),
            "an unknown source id falls back to the document's own path"
        );
    }

    #[test]
    fn astext_joins_text_descendants() {
        let mut p = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        p.children
            .push(Node::text_node("line one\nline two", Span::ZERO));
        assert_eq!(p.astext(), "line one\nline two");
    }
}
