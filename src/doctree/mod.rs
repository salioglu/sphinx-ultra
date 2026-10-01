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

use std::borrow::Cow;

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
/// 735-738`); a hyperlink, footnote or citation reference (as written)
/// and a hyperlink target (its explicit-markup block, or an embedded
/// alias's `<…>`), from which the hyperlink transforms build theirs
/// (`:149-150,293-294,983`). docutils keeps `rawsource` beside
/// `Element.attributes`, not in them, so `attlist()` never prints it, and
/// neither does [`Node::pformat`].
pub const RAWSOURCE: &str = "rawsource";

/// [`Node::escapes`] tag: an escaped space docutils' `unescape` removed.
pub const ESCAPED_SPACE: u32 = 1 << 31;

/// [`Node::escapes`] tag: an escaped newline docutils' `unescape` removed.
pub const ESCAPED_NEWLINE: u32 = 1 << 30;

/// The byte-offset bits of a [`Node::escapes`] entry: a text node keeps no
/// escape past its first GiB.
const ESCAPE_OFFSET: u32 = ESCAPED_NEWLINE - 1;

/// One doctree node. Element nodes have `text == None`; text leaves have
/// `kind == kinds::TEXT`, `Some(text)`, and no children or attributes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    /// Where docutils' text holds the backslash escapes this `text` has
    /// lost; empty on every element and on every text node without one
    /// (an empty `Vec` does not allocate).
    ///
    /// docutils' Inliner parses `escape2null(text)` — each `\` + char a
    /// `\x00` + char (`docutils/parsers/rst/states.py:750`,
    /// `docutils/utils/__init__.py:657-668`) — and its `Text` keeps the
    /// nulls: `str(node)` holds them, only `astext()` drops them, with an
    /// escaped space or newline (`Text.astext`, `nodes.unescape`,
    /// `docutils/nodes.py:440-441,2925-2939`). This tree's `text` is that
    /// `astext()`; each entry is one `\x00` of `str(node)`, in order:
    ///
    /// - a byte offset `n` of `text`: a `\x00` before `text[n..]`, whose
    ///   first character was escaped (at `n == text.len()`, a trailing lone
    ///   backslash's bare `\x00`);
    /// - `n | ESCAPED_SPACE`, `n | ESCAPED_NEWLINE`: an escaped space or
    ///   newline `unescape` removed — `\x00 `, `\x00\n` — at offset `n`.
    ///
    /// Together they are all of `str(node)` ([`Node::null_escaped`]), which
    /// is what docutils reads where an escape matters: SmartQuotes educates
    /// `str(node)` — a `\x00` before a quote, dash or dot keeps it plain,
    /// and an escaped space is still whitespace to the quote rules
    /// (`docutils/transforms/universal.py:267-278`,
    /// `sphinx/transforms/__init__.py:403-415`) — a term splits off its
    /// classifiers in it (`states.py:3011`), and an `authors` field its
    /// names (`docutils/transforms/frontmatter.py:516-526`).
    ///
    /// The inline parser writes it ([`Node::text_from_null_escaped`]), as do
    /// the transforms that rebuild a text node from its `str()`; `pformat`
    /// and `astext` ignore it, as docutils prints `astext()`.
    ///
    /// It is the first field on the wire: a tree in the shape before it —
    /// what a doctree file or `env.bin` of the same format version written
    /// before it holds — starts with its root's kind length and bytes, which
    /// decode as offsets out of order and fail at once: `document`,
    /// `title` and `bullet_list`, the roots persisted, each spell bytes
    /// that drop somewhere (see `DOCTREE_FORMAT_VERSION`).
    pub escapes: Vec<u32>,
    #[serde(serialize_with = "intern::serialize_str")]
    pub kind: &'static str,
    pub span: Span,
    pub text: Option<String>,
    pub attrs: Attrs,
    pub children: Vec<Node>,
}

/// One [`Node::escapes`] entry: its byte offset and the whitespace that
/// followed its `\x00` in docutils' string when `unescape` removed one;
/// `None` for an entry with both tags.
fn escape_entry(entry: u32) -> Option<(usize, Option<char>)> {
    let offset = (entry & ESCAPE_OFFSET) as usize;
    match entry & !ESCAPE_OFFSET {
        0 => Some((offset, None)),
        ESCAPED_SPACE => Some((offset, Some(' '))),
        ESCAPED_NEWLINE => Some((offset, Some('\n'))),
        _ => None,
    }
}

/// Every entry well-tagged and the offsets non-decreasing.
fn escapes_in_order(escapes: &[u32]) -> bool {
    let mut previous = 0;
    for &entry in escapes {
        match escape_entry(entry) {
            Some((offset, _)) if offset >= previous => previous = offset,
            _ => return false,
        }
    }
    true
}

/// Whether `escapes` can sit beside `text`: in order, and every offset on
/// a character boundary of it (its end included).
fn escapes_fit(text: &str, escapes: &[u32]) -> bool {
    escapes_in_order(escapes)
        && escapes
            .iter()
            .all(|&entry| text.is_char_boundary((entry & ESCAPE_OFFSET) as usize))
}

/// `escapes` decodes first ([`Node::escapes`]): an entry out of order or
/// badly tagged fails right here, before the rest of a node written in
/// another shape is misread.
fn deserialize_escapes<'de, D>(deserializer: D) -> Result<Vec<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let escapes = Vec::<u32>::deserialize(deserializer)?;
    if !escapes_in_order(&escapes) {
        return Err(serde::de::Error::custom(
            "escape offsets out of order: a node of another shape",
        ));
    }
    Ok(escapes)
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
    #[serde(deserialize_with = "deserialize_escapes")]
    escapes: Vec<u32>,
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
        let fits = match &shadow.text {
            Some(text) => escapes_fit(text, &shadow.escapes),
            None => shadow.escapes.is_empty(),
        };
        if !fits {
            return Err(serde::de::Error::custom(
                "escape offsets that do not fit the node's text",
            ));
        }
        let kind = intern(&shadow.kind).map_err(serde::de::Error::custom)?;
        Ok(Node {
            escapes: shadow.escapes,
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
            escapes: Vec::new(),
            kind,
            span,
            text: None,
            attrs: Attrs::default(),
            children: Vec::new(),
        }
    }

    pub fn text_node(s: impl Into<String>, span: Span) -> Node {
        Node {
            escapes: Vec::new(),
            kind: kinds::TEXT,
            span,
            text: Some(s.into()),
            attrs: Attrs::default(),
            children: Vec::new(),
        }
    }

    /// A text node holding `text` — docutils' `astext()` — with the
    /// [`Node::escapes`] of its `str(node)` beside it.
    pub fn text_node_escaped(text: impl Into<String>, escapes: Vec<u32>, span: Span) -> Node {
        let text = text.into();
        debug_assert!(escapes_fit(&text, &escapes), "{escapes:?} beside {text:?}");
        Node {
            escapes,
            ..Node::text_node(text, span)
        }
    }

    /// docutils' `nodes.Text(data)` for a null-escaped `data` — the
    /// Inliner's text, or a `str(node)` a transform rewrote: the text is
    /// `unescape(data)` (`docutils/nodes.py:2925-2939` — an escaped space
    /// or newline goes with its `\x00`, any other `\x00` alone) and every
    /// `\x00` an entry of [`Node::escapes`].
    pub fn text_from_null_escaped(data: &str, span: Span) -> Node {
        if !data.contains('\0') {
            return Node::text_node(data, span);
        }
        let mut text = String::with_capacity(data.len());
        let mut escapes = Vec::new();
        let mut chars = data.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\0' {
                text.push(c);
                continue;
            }
            let tag = match chars.peek() {
                Some(' ') => ESCAPED_SPACE,
                Some('\n') => ESCAPED_NEWLINE,
                _ => 0,
            };
            if tag != 0 {
                chars.next();
            }
            if let Some(offset) = u32::try_from(text.len())
                .ok()
                .filter(|offset| *offset <= ESCAPE_OFFSET)
            {
                escapes.push(offset | tag);
            }
        }
        Node::text_node_escaped(text, escapes, span)
    }

    /// docutils' `str(node)` of a text node — [`Node::text`] with the
    /// `\x00`s of [`Node::escapes`] back in place; `None` for an element.
    pub fn null_escaped(&self) -> Option<Cow<'_, str>> {
        let text = self.text.as_deref()?;
        if self.escapes.is_empty() {
            return Some(Cow::Borrowed(text));
        }
        let mut out = String::with_capacity(text.len() + 2 * self.escapes.len());
        let mut copied = 0;
        for (offset, removed) in self.escapes.iter().filter_map(|&e| escape_entry(e)) {
            // Constructed and decoded nodes keep `escapes_fit`; an entry a
            // hand edit put out of step is skipped, not a panic.
            let Some(piece) = text.get(copied..offset) else {
                continue;
            };
            out.push_str(piece);
            out.push('\0');
            out.extend(removed);
            copied = offset;
        }
        out.push_str(&text[copied..]);
        Some(Cow::Owned(out))
    }

    /// docutils `Element.copy()`: same kind, span and attributes, but **no
    /// children** (docutils copies `rawsource` and attributes only;
    /// `deepcopy` is the one that takes the subtree).
    pub fn shallow_copy(&self) -> Node {
        Node {
            escapes: self.escapes.clone(),
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

    /// Remove a scalar attribute (`del node[key]`), returning its value.
    pub fn remove(&mut self, key: &str) -> Option<AttrValue> {
        let index = self
            .attrs
            .extra
            .binary_search_by(|(k, _)| (*k).cmp(key))
            .ok()?;
        Some(self.attrs.extra.remove(index).1)
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

/// Test support: a tree in the wire shape [`Node`] had before
/// [`Node::escapes`] — what a doctree file or `env.bin` written by an
/// earlier build of this branch holds.
#[cfg(test)]
pub(crate) mod shape_before_escapes {
    use super::{Attrs, Doctree, Node, Span};
    use serde::Serialize;

    #[derive(Serialize)]
    pub(crate) struct OldNode<'n> {
        kind: &'n str,
        span: Span,
        text: &'n Option<String>,
        attrs: &'n Attrs,
        children: Vec<OldNode<'n>>,
    }

    #[derive(Serialize)]
    pub(crate) struct OldDoctree<'n> {
        root: OldNode<'n>,
        sources: &'n [String],
    }

    pub(crate) fn node(node: &Node) -> OldNode<'_> {
        OldNode {
            kind: node.kind,
            span: node.span,
            text: &node.text,
            attrs: &node.attrs,
            children: node.children.iter().map(self::node).collect(),
        }
    }

    /// `doctree` encoded as [`super::to_bincode`] encoded it then.
    pub(crate) fn to_bincode(doctree: &Doctree) -> Vec<u8> {
        let old = OldDoctree {
            root: node(&doctree.root),
            sources: &doctree.sources,
        };
        bincode::serde::encode_to_vec(old, bincode::config::standard()).unwrap()
    }
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
        let json = r#"{"root":{"escapes":[],"kind":"document","span":{"source":0,"line":0,"start":0,"end":0},"text":null,"attrs":{"ids":[],"names":[],"dupnames":[],"classes":[],"backrefs":[],"extra":{}},"children":[]}}"#;

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

    fn escape2null(raw: &str) -> String {
        crate::rst::inline::escape2null(raw)
    }

    /// `Node::text_node_escaped` keeps the text docutils' `astext()` gives
    /// and, beside it, where the `\x00` markers of its `str(node)` were.
    #[test]
    fn text_node_escaped_holds_the_text_and_its_escapes() {
        let t = Node::text_node_escaped("\"a\"", vec![0, 2], Span::ZERO);
        assert_eq!(t.kind, kinds::TEXT);
        assert_eq!(t.text.as_deref(), Some("\"a\""));
        assert_eq!(t.escapes, [0, 2]);
        assert_eq!(t.astext(), "\"a\"");
        assert_eq!(t.null_escaped().as_deref(), Some("\u{0}\"a\u{0}\""));
    }

    /// docutils' `str(text)` survives the split into text and escapes:
    /// every escaped character behind its `\x00`, every escaped space or
    /// newline `unescape` removes (`docutils/nodes.py:2937-2938`) back as
    /// `\x00 `/`\x00\n`, a trailing lone backslash as a bare `\x00`
    /// (`docutils/utils/__init__.py:657-668`).
    #[test]
    fn a_null_escaped_string_round_trips_through_text_and_escapes() {
        for raw in [
            "plain",
            "\\\"a\\\"",
            "a\\--b",
            "x\\ \"y\"",
            "line1\\\nline2",
            "foo\\",
            "a\\\\b",
            "é\\'ü\\ \\x",
            "\\ \\\n\\",
            "",
        ] {
            let null_escaped = escape2null(raw);
            let node = Node::text_from_null_escaped(&null_escaped, Span::ZERO);
            assert_eq!(
                node.text.as_deref(),
                Some(crate::rst::inline::unescape(&null_escaped, false).as_str()),
                "{raw:?}"
            );
            assert_eq!(
                node.null_escaped().as_deref(),
                Some(null_escaped.as_str()),
                "{raw:?}"
            );
        }
    }

    /// An escaped character is its plain byte offset; an escaped space or
    /// newline, which the text no longer holds, is the offset where it
    /// was, tagged; a trailing lone backslash is the offset one past the
    /// text.
    #[test]
    fn escaped_whitespace_and_a_trailing_backslash_are_tagged_offsets() {
        let node = |raw: &str| Node::text_from_null_escaped(&escape2null(raw), Span::ZERO);
        assert_eq!(node("é\\'").escapes, [2]);
        assert_eq!(node("x\\ y").escapes, [1 | ESCAPED_SPACE]);
        assert_eq!(node("x\\\ny").escapes, [1 | ESCAPED_NEWLINE]);
        assert_eq!(node("foo\\").escapes, [3]);
        assert_eq!(node("a\\ \\\"b").escapes, [1 | ESCAPED_SPACE, 1]);
    }

    /// No escape, no heap: `escapes` is on every node.
    #[test]
    fn an_escape_free_node_allocates_no_escapes() {
        assert_eq!(
            Node::elem(kinds::PARAGRAPH, Span::ZERO).escapes.capacity(),
            0
        );
        assert_eq!(Node::text_node("x", Span::ZERO).escapes.capacity(), 0);
        assert_eq!(
            Node::text_from_null_escaped("plain", Span::ZERO)
                .escapes
                .capacity(),
            0
        );
    }

    fn in_a_document(node: Node) -> Doctree {
        let mut root = Node::elem(kinds::DOCUMENT, Span::ZERO);
        let mut paragraph = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        paragraph.children.push(node);
        root.children.push(paragraph);
        Doctree {
            root,
            sources: vec!["<test>".to_string()],
        }
    }

    /// A decoded tree upholds what the constructors do, so that a reader
    /// can slice `text` at any escape offset: the offsets sit on character
    /// boundaries within the text, in order, at most one whitespace tag
    /// each, and only on a text node.
    #[test]
    fn decoding_rejects_escapes_that_do_not_fit_the_text() {
        let text = |text: &str, escapes: Vec<u32>| {
            let mut node = Node::text_node(text, Span::ZERO);
            node.escapes = escapes;
            node
        };
        let mut element = Node::elem(kinds::EMPHASIS, Span::ZERO);
        element.escapes = vec![0];
        for (what, node) in [
            ("past the end", text("ab", vec![3])),
            ("inside a character", text("é", vec![1])),
            ("out of order", text("abc", vec![2, 1])),
            (
                "both whitespace tags",
                text("ab", vec![1 | ESCAPED_SPACE | ESCAPED_NEWLINE]),
            ),
            ("on an element", element),
        ] {
            assert!(
                from_bincode(&to_bincode(&in_a_document(node))).is_err(),
                "{what}"
            );
        }
        let fits = text("é\"", vec![0, 2, 3 | ESCAPED_SPACE]);
        let tree = in_a_document(fits);
        assert_eq!(from_bincode(&to_bincode(&tree)).unwrap(), tree);
    }

    /// A node written before `escapes` existed — the root of a doctree file,
    /// or of a `titles`/`longtitles`/`tocs` entry in `env.bin` — fails to
    /// decode at its first bytes: `escapes` is the first field, so the old
    /// kind string's length and bytes are read as escape offsets, which
    /// for each of these kinds drop somewhere.
    #[test]
    fn a_node_in_the_shape_before_escapes_fails_to_decode() {
        let mut list = Node::elem(kinds::BULLET_LIST, Span::ZERO);
        list.children.push(Node::elem(kinds::LIST_ITEM, Span::ZERO));
        let mut title = Node::elem(kinds::TITLE, Span::ZERO);
        title.children.push(Node::text_node("T", Span::ZERO));
        let document = in_a_document(Node::text_node("x", Span::ZERO)).root;
        for node in [document, title, list] {
            let bytes = bincode::serde::encode_to_vec(
                shape_before_escapes::node(&node),
                bincode::config::standard(),
            )
            .unwrap();
            let decoded: Result<(Node, usize), _> =
                bincode::serde::decode_from_slice(&bytes, bincode::config::standard());
            assert!(decoded.is_err(), "{}", node.kind);
        }
    }
}
