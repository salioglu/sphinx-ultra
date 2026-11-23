//! Typed doctree IR with docutils-equivalent node semantics (M2 wave 1).
//!
//! Design (recorded in docs/plans/2026-08-07-m2-wave-map.md):
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
//!
//! What docutils keeps on its node objects beside the printed attributes —
//! `rawsource`, a `pending` node's transform, a Text's null-escaped data,
//! and the object's identity — lives in one optional, boxed side-car,
//! [`NodeExt`] (wave 5a, decision 10).

pub mod ids;
mod intern;
pub mod kinds;
pub mod messages;
pub mod pending;
pub mod pformat;

pub(crate) use intern::intern;

use std::borrow::Cow;
use std::num::NonZeroU32;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

/// A node's identity within one document's read stage — what Python
/// gets from object identity. The document state's registries key the
/// nodes docutils registers by it. Minted through [`Uid::new`] from a
/// per-document counter, so it never repeats within a document and
/// orders as minted; never persisted ([`NodeExt::uid`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Uid(NonZeroU32);

impl Uid {
    pub const fn new(n: NonZeroU32) -> Uid {
        Uid(n)
    }
}

/// A node's side-car: what docutils keeps on its node objects beside the
/// attributes pformat prints. Boxed and optional, so a node without one
/// costs a single pointer.
///
/// Two halves:
/// - tree data, persisted with the doctree and compared by [`Node`]'s
///   `PartialEq`: [`rawsource`](Self::rawsource) and
///   [`pending`](Self::pending);
/// - read-stage data, neither persisted nor compared:
///   [`uid`](Self::uid) and [`raw`](Self::raw).
///
/// pformat prints none of it, except that a `pending` node prints its
/// [`pending::Pending::internals`], as docutils' `pending.pformat` does.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeExt {
    /// The node's identity during the read stage: set on nodes docutils
    /// registers and on nodes that carry a location override.
    #[serde(skip)]
    pub uid: Option<Uid>,
    /// docutils `node.rawsource`. The parser records it on `literal_block`
    /// and `doctest_block` (Sphinx's HTML writer highlights a literal block
    /// only when its `rawsource` equals its `astext()`); the field also
    /// serves the other kinds whose `rawsource` a later stage reads:
    /// Referential inline nodes, `pending` and `substitution_definition`.
    /// `None` is docutils' default `''` ([`Node::set_rawsource`]).
    pub rawsource: Option<Box<str>>,
    /// The data of a `#text` leaf docutils builds from null-escaped text:
    /// `str(Text)`, with `\0` before each backslash-escaped character,
    /// where the node's `text` is its unescaped `astext()`. Set only when
    /// the data holds a `\0` ([`Node::text_node_escaped`]); read by
    /// SmartQuotes and cleared at the end of the read transforms.
    #[serde(skip)]
    pub raw: Option<Box<str>>,
    /// The transform a `pending` node defers and its details; set on
    /// `kind == "pending"` only.
    pub pending: Option<Box<pending::Pending>>,
}

impl NodeExt {
    /// Whether any field the doctree persists is set.
    fn has_tree_data(&self) -> bool {
        self.rawsource.is_some() || self.pending.is_some()
    }
}

/// One doctree node. Element nodes have `text == None`; text leaves have
/// `kind == kinds::TEXT`, `Some(text)`, and no children or attributes.
///
/// `PartialEq` is written by hand: see its impl.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    #[serde(serialize_with = "intern::serialize_str")]
    pub kind: &'static str,
    pub span: Span,
    pub text: Option<String>,
    pub attrs: Attrs,
    pub children: Vec<Node>,
    /// The side-car ([`NodeExt`]). One that holds only read-stage data is
    /// written as `None`, so the persisted bytes never depend on the read
    /// stage.
    #[serde(serialize_with = "serialize_ext")]
    pub ext: Option<Box<NodeExt>>,
}

fn serialize_ext<S: Serializer>(
    ext: &Option<Box<NodeExt>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match ext.as_deref().filter(|ext| ext.has_tree_data()) {
        Some(ext) => serializer.serialize_some(ext),
        None => serializer.serialize_none(),
    }
}

/// Equality over everything a doctree persists: kind, span, text,
/// attributes, children and the side-car's tree data (`rawsource`,
/// `pending`). The side-car's read-stage data (`uid`, `raw`) never takes
/// part, and a missing side-car equals an empty one, so a tree read back
/// from the cache equals the tree its parse produced.
impl PartialEq for Node {
    fn eq(&self, other: &Node) -> bool {
        self.kind == other.kind
            && self.span == other.span
            && self.text == other.text
            && self.attrs == other.attrs
            && self.rawsource() == other.rawsource()
            && self.pending() == other.pending()
            && self.children == other.children
    }
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
    ext: Option<Box<NodeExt>>,
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
            ext: shadow.ext,
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
            ext: None,
        }
    }

    pub fn text_node(s: impl Into<String>, span: Span) -> Node {
        Node {
            kind: kinds::TEXT,
            span,
            text: Some(s.into()),
            attrs: Attrs::default(),
            children: Vec::new(),
            ext: None,
        }
    }

    /// docutils `nodes.Text(data)` over null-escaped `data` (the inliner's
    /// `escape2null` form): `text` is `unescape(data)`, the `astext()` every
    /// consumer reads, and the side-car keeps `data` itself as
    /// [`NodeExt::raw`] when it holds a `\0` marker.
    pub fn text_node_escaped(data: &str, span: Span) -> Node {
        let mut node = Node::text_node(unescape(data, false), span);
        if data.contains('\0') {
            node.ext_mut().raw = Some(data.into());
        }
        node
    }

    /// docutils `Element.copy()`: same kind, span and attributes, but **no
    /// children** (`deepcopy` is the one that takes the subtree). Like
    /// docutils' `copy()`, it keeps `rawsource`, a `pending` node's
    /// transform and details, and a Text's data; the copy is a new object,
    /// so it has no `uid`.
    pub fn shallow_copy(&self) -> Node {
        let ext = self.ext.as_deref().and_then(|ext| {
            let copy = NodeExt {
                uid: None,
                ..ext.clone()
            };
            (copy.has_tree_data() || copy.raw.is_some()).then(|| Box::new(copy))
        });
        Node {
            kind: self.kind,
            span: self.span,
            text: self.text.clone(),
            attrs: self.attrs.clone(),
            children: Vec::new(),
            ext,
        }
    }

    fn ext_mut(&mut self) -> &mut NodeExt {
        self.ext.get_or_insert_with(Box::default)
    }

    /// The node's read-stage identity ([`NodeExt::uid`]).
    pub fn uid(&self) -> Option<Uid> {
        self.ext.as_deref().and_then(|ext| ext.uid)
    }

    pub fn set_uid(&mut self, uid: Uid) {
        self.ext_mut().uid = Some(uid);
    }

    /// docutils `node.rawsource` ([`NodeExt::rawsource`]); `None` is `''`.
    pub fn rawsource(&self) -> Option<&str> {
        self.ext.as_deref().and_then(|ext| ext.rawsource.as_deref())
    }

    /// Record docutils' `rawsource`. The empty string is docutils' default
    /// and is stored as `None`, so equal values compare equal however they
    /// were set.
    pub fn set_rawsource(&mut self, s: &str) {
        if s.is_empty() {
            if let Some(ext) = self.ext.as_deref_mut() {
                ext.rawsource = None;
            }
        } else {
            self.ext_mut().rawsource = Some(s.into());
        }
    }

    /// The pending transform of a `pending` node ([`NodeExt::pending`]).
    fn pending(&self) -> Option<&pending::Pending> {
        self.ext.as_deref().and_then(|ext| ext.pending.as_deref())
    }

    /// docutils `str(Text)`: the null-escaped data when the side-car keeps
    /// it ([`NodeExt::raw`]), else the text itself (no escape marker in it).
    /// Empty for an element.
    pub fn text_data(&self) -> Cow<'_, str> {
        match self.ext.as_deref().and_then(|ext| ext.raw.as_deref()) {
            Some(raw) => Cow::Borrowed(raw),
            None => Cow::Borrowed(self.text.as_deref().unwrap_or_default()),
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

    /// docutils `node.astext()` as it runs under Sphinx 9.1.0. A Text gives
    /// its unescaped data. An element joins its children's `astext()` with
    /// its class's `child_text_separator`: `""` for every `TextElement`
    /// ([`TEXT_ELEMENT_KINDS`]) and for `option` and Sphinx's
    /// `pending_xref`; `", "` for `option_group` and Sphinx's parameter
    /// lists; `"  "` for `option_list_item`; `" "` for a multi-line
    /// `desc_signature`; `"\n\n"` for every other `Element`. The class
    /// overrides apply on top: `image` gives its `alt`, `option_argument`
    /// prefixes its `delimiter` (default `" "`), `desc_returns` prefixes
    /// `" -> "`, and `desc_parameterlist` / `desc_optional` /
    /// `desc_type_parameter_list` wrap in `()` / `[]` / `[]`.
    ///
    /// A `system_message` gives docutils' own override,
    /// `'{source}:{line}: ({type}/{level}) {body}'` (`nodes.py:2516-2519`),
    /// its `line` empty when it has none; the body alone is
    /// [`Node::element_astext`].
    ///
    /// Unlike [`Node::astext`], which joins every child with `""`.
    pub fn astext_docutils(&self) -> String {
        let mut text = String::new();
        push_astext(self, &mut text);
        text
    }

    /// docutils `Element.astext()`: the children's `astext()` joined with
    /// the class's `child_text_separator`, with no class override on top.
    /// For a `system_message` that is its body, without the location
    /// prefix: the text Sphinx's warning stream prints for it.
    pub fn element_astext(&self) -> String {
        let mut text = String::new();
        push_children_astext(self, child_text_separator(self), &mut text);
        text
    }

    /// Byte-parity pseudo-XML rendering (docutils `document.pformat()`).
    pub fn pformat(&self) -> String {
        pformat::pformat(self)
    }
}

/// What docutils' `astext()` reads of a node: its kind, its text, its
/// scalar attributes and its children. [`push_astext`] is written over it
/// once, for the owned tree ([`Node::astext_docutils`]) and for the
/// transforms' arena (`TxTree::astext`), which reads its nodes in place.
pub(crate) trait AstextView<'a>: Copy {
    fn kind(self) -> &'static str;
    fn text(self) -> Option<&'a str>;
    fn attr(self, key: &'static str) -> Option<&'a AttrValue>;
    fn children(self) -> impl Iterator<Item = Self>;
}

impl<'a> AstextView<'a> for &'a Node {
    fn kind(self) -> &'static str {
        self.kind
    }

    fn text(self) -> Option<&'a str> {
        self.text.as_deref()
    }

    fn attr(self, key: &'static str) -> Option<&'a AttrValue> {
        self.get(key)
    }

    fn children(self) -> impl Iterator<Item = Self> {
        self.children.iter()
    }
}

/// [`Node::astext_docutils`] of `node`, appended to `out`.
pub(crate) fn push_astext<'a>(node: impl AstextView<'a>, out: &mut String) {
    match node.kind() {
        kinds::TEXT => out.push_str(node.text().unwrap_or_default()),
        kinds::IMAGE => {
            if let Some(AttrValue::Str(alt)) = node.attr("alt") {
                out.push_str(alt);
            }
        }
        kinds::OPTION_ARGUMENT => {
            let delimiter = match node.attr("delimiter") {
                Some(AttrValue::Str(delimiter)) => delimiter.as_str(),
                _ => " ",
            };
            out.push_str(delimiter);
            push_children_astext(node, "", out);
        }
        "desc_returns" => {
            out.push_str(" -> ");
            push_children_astext(node, "", out);
        }
        "desc_parameterlist" => {
            out.push('(');
            push_children_astext(node, ", ", out);
            out.push(')');
        }
        "desc_optional" | "desc_type_parameter_list" => {
            out.push('[');
            push_children_astext(node, ", ", out);
            out.push(']');
        }
        kinds::SYSTEM_MESSAGE => {
            let attr = |key: &'static str| match node.attr(key) {
                Some(AttrValue::Str(value)) => value.clone(),
                Some(AttrValue::Int(value)) => value.to_string(),
                _ => String::new(),
            };
            out.push_str(&format!(
                "{}:{}: ({}/{}) ",
                attr("source"),
                attr("line"),
                attr("type"),
                attr("level"),
            ));
            push_children_astext(node, child_text_separator(node), out);
        }
        _ => push_children_astext(node, child_text_separator(node), out),
    }
}

/// The children's `astext()` joined with `separator`, appended to `out`.
fn push_children_astext<'a>(node: impl AstextView<'a>, separator: &str, out: &mut String) {
    for (at, child) in node.children().enumerate() {
        if at > 0 {
            out.push_str(separator);
        }
        push_astext(child, out);
    }
}

/// docutils / Sphinx `child_text_separator` of the node's class.
fn child_text_separator<'a>(node: impl AstextView<'a>) -> &'static str {
    match node.kind() {
        kinds::OPTION | kinds::PENDING_XREF => "",
        kinds::OPTION_GROUP => ", ",
        kinds::OPTION_LIST_ITEM => "  ",
        // A property in `addnodes.desc_signature`: `' '` when
        // `self.get('is_multiline')` is truthy.
        "desc_signature" if node.attr("is_multiline").is_some_and(AttrValue::is_truthy) => " ",
        kind if is_text_element(kind) => "",
        _ => "\n\n",
    }
}

impl AttrValue {
    /// Python truthiness of the attribute value.
    pub(crate) fn is_truthy(&self) -> bool {
        match self {
            AttrValue::Int(n) => *n != 0,
            AttrValue::Str(s) => !s.is_empty(),
            AttrValue::List(items) => !items.is_empty(),
        }
    }
}

/// Every element class of docutils 0.22.4 (`nodes.py`) and Sphinx 9.1.0
/// (`addnodes.py`) that subclasses `nodes.TextElement`, by tagname, sorted
/// for [`is_text_element`]. Abstract bases (`desc_sig_element`) and the
/// `desc_classname` alias of `desc_addname` have no tagname of their own.
//
// oracle: every class `c` of `docutils.nodes` and `sphinx.addnodes` with
//   `issubclass(c, nodes.TextElement)`, by name (scratchpad t4probe/te.py).
const TEXT_ELEMENT_KINDS: &[&str] = &[
    "abbreviation",
    "acronym",
    "address",
    "attribution",
    "author",
    "caption",
    "centered",
    "citation_reference",
    "classifier",
    "comment",
    "compact_paragraph",
    "contact",
    "copyright",
    "date",
    "desc_addname",
    "desc_annotation",
    "desc_inline",
    "desc_name",
    "desc_optional",
    "desc_parameter",
    "desc_parameterlist",
    "desc_returns",
    "desc_sig_keyword",
    "desc_sig_keyword_type",
    "desc_sig_literal_char",
    "desc_sig_literal_number",
    "desc_sig_literal_string",
    "desc_sig_name",
    "desc_sig_operator",
    "desc_sig_punctuation",
    "desc_sig_space",
    "desc_signature",
    "desc_signature_line",
    "desc_type",
    "desc_type_parameter",
    "desc_type_parameter_list",
    "doctest_block",
    "download_reference",
    "emphasis",
    "field_name",
    "footnote_reference",
    "generated",
    "index",
    "inline",
    "label",
    "line",
    "literal",
    "literal_block",
    "literal_emphasis",
    "literal_strong",
    "manpage",
    "math",
    "math_block",
    "number_reference",
    "option_argument",
    "option_string",
    "organization",
    "paragraph",
    "pending_xref_condition",
    "problematic",
    "production",
    "raw",
    "reference",
    "revision",
    "rubric",
    "status",
    "strong",
    "subscript",
    "substitution_definition",
    "substitution_reference",
    "subtitle",
    "superscript",
    "target",
    "term",
    "title",
    "title_reference",
    "version",
    "versionmodified",
];

/// Whether `kind` names a `nodes.TextElement` class ([`TEXT_ELEMENT_KINDS`]).
pub(crate) fn is_text_element(kind: &str) -> bool {
    TEXT_ELEMENT_KINDS.binary_search(&kind).is_ok()
}

/// docutils `nodes.unescape` (`nodes.py:2925-2939`) over null-escaped text
/// (the inliner's `escape2null` form): drop `\x00 ` and `\x00\n` (both
/// chars, so an escaped space or newline vanishes), then bare `\x00`
/// markers, keeping the escaped char. With `restore_backslashes` every
/// marker becomes a literal `\` instead (inline literals).
pub fn unescape(text: &str, restore_backslashes: bool) -> String {
    if restore_backslashes {
        return text.replace('\0', "\\");
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\0' {
            if let Some(' ' | '\n') = chars.peek() {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
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

    fn parse(src: &str, sphinx: bool) -> Doctree {
        crate::rst::parse_rst(
            src,
            &crate::rst::ParseOptions {
                source_path: "<snippet>".into(),
                sphinx,
                ..Default::default()
            },
        )
    }

    /// The first node of `kind` in document order.
    fn first_of<'a>(node: &'a Node, kind: &str) -> Option<&'a Node> {
        if node.kind == kind {
            return Some(node);
        }
        node.children.iter().find_map(|child| first_of(child, kind))
    }

    /// Every `#text` leaf, in document order.
    fn text_leaves(node: &Node) -> Vec<&Node> {
        if node.kind == kinds::TEXT {
            return vec![node];
        }
        node.children.iter().flat_map(text_leaves).collect()
    }

    fn uid(n: u32) -> Uid {
        Uid::new(std::num::NonZeroU32::new(n).expect("a non-zero uid"))
    }

    /// docutils keeps a literal block's `rawsource` beside its text:
    /// `literal_block(data, data)` for a `::` block, the markup-bearing
    /// content for `parsed-literal` (`directives/body.py` `ParsedLiteral`),
    /// where it differs from `astext()` — which is how Sphinx's HTML writer
    /// tells a parsed literal from code to highlight. pformat never prints
    /// it.
    //
    // oracle (docutils 0.22.4, parse layer): `.. parsed-literal::\n\n   *x*\n`
    //   -> literal_block rawsource '*x*', astext() 'x'; `::\n\n  code\n` ->
    //   literal_block rawsource 'code'; both pformats as asserted below.
    #[test]
    fn rawsource_is_recorded_but_never_printed() {
        let tree = parse(".. parsed-literal::\n\n   *x*\n", true);
        let block = first_of(&tree.root, kinds::LITERAL_BLOCK).expect("a literal_block");
        assert_eq!(block.rawsource(), Some("*x*"));
        assert_eq!(block.astext_docutils(), "x");
        assert_eq!(
            tree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <literal_block xml:space=\"preserve\">\n",
                "        <emphasis>\n",
                "            x\n",
            )
        );

        let tree = parse("::\n\n  code\n", true);
        let block = first_of(&tree.root, kinds::LITERAL_BLOCK).expect("a literal_block");
        assert_eq!(block.rawsource(), Some("code"));
        assert_eq!(
            tree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <literal_block xml:space=\"preserve\">\n",
                "        code\n",
            )
        );
    }

    // oracle (docutils 0.22.4, parse layer): the section of
    //   `Title\n=====\n\nPara one.\n\nPara two.\n` -> astext()
    //   'Title\n\nPara one.\n\nPara two.'; the paragraph `a *b* c` -> 'a b c';
    //   image(alt='pic').astext() -> 'pic', image(uri='x').astext() -> ''.
    #[test]
    fn astext_docutils_matches_docutils() {
        let tree = parse("Title\n=====\n\nPara one.\n\nPara two.\n", false);
        let section = &tree.root.children[0];
        assert_eq!(section.kind, kinds::SECTION);
        assert_eq!(section.astext_docutils(), "Title\n\nPara one.\n\nPara two.");
        assert_eq!(
            tree.root.astext_docutils(),
            "Title\n\nPara one.\n\nPara two."
        );

        let tree = parse("a *b* c\n", false);
        assert_eq!(tree.root.children[0].astext_docutils(), "a b c");

        let mut image = Node::elem(kinds::IMAGE, Span::ZERO);
        assert_eq!(image.astext_docutils(), "");
        image.set("alt", AttrValue::Str("pic".into()));
        assert_eq!(image.astext_docutils(), "pic");
    }

    #[test]
    fn text_element_kinds_are_sorted_for_binary_search() {
        assert!(TEXT_ELEMENT_KINDS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(is_text_element(kinds::PARAGRAPH));
        assert!(is_text_element("desc_sig_space"));
        assert!(!is_text_element(kinds::SECTION));
        assert!(!is_text_element(kinds::PENDING_XREF));
    }

    /// The class-level `child_text_separator`s and `astext` overrides of
    /// docutils' option lists and of Sphinx's `pending_xref` and signature
    /// nodes — everything [`Node::astext_docutils`] dispatches on besides
    /// the `TextElement`/`Element` split. A `system_message` overrides
    /// `astext()` with its `source:line: (TYPE/level) ` prefix, which
    /// Sphinx's warning stream strips again: [`Node::element_astext`] is
    /// the body it prints.
    //
    // oracle (docutils 0.22.4 / sphinx 9.1.0, scratchpad t4probe):
    //   option_list_item '-a, --all=FILE  description here' (sep '  '),
    //   option_group '-a, --all=FILE' (', '), option '--all=FILE' (''),
    //   option_argument '=FILE' / ' X' (delimiter + text);
    //   desc_parameterlist(a, b) '(a, b)', desc_optional(a, b) '[a, b]',
    //   desc_type_parameter_list(T, U) '[T, U]', desc_returns('int') ' -> int',
    //   desc_signature(l1, l2) 'l1l2' and 'l1 l2' with is_multiline,
    //   pending_xref(literal a, literal b) 'ab';
    //   system_message('Boom here.', literal_block('x', 'x'), line=3,
    //   source='<snippet>').astext() '<snippet>:3: (WARNING/2) Boom here.\n\nx',
    //   Element.astext of it 'Boom here.\n\nx'; without a line
    //   '<snippet>:: (ERROR/3) Boom.'.
    #[test]
    fn astext_docutils_follows_class_overrides() {
        let tree = parse("-a, --all=FILE  description here\n-b X  other\n", false);
        let item = first_of(&tree.root, kinds::OPTION_LIST_ITEM).expect("an option_list_item");
        assert_eq!(item.astext_docutils(), "-a, --all=FILE  description here");
        let group = &item.children[0];
        assert_eq!(group.kind, kinds::OPTION_GROUP);
        assert_eq!(group.astext_docutils(), "-a, --all=FILE");
        assert_eq!(group.children[1].astext_docutils(), "--all=FILE");
        let list = first_of(&tree.root, kinds::OPTION_LIST).expect("an option_list");
        let second = &list.children[1].children[0].children[0];
        assert_eq!(second.kind, kinds::OPTION);
        assert_eq!(second.children[1].astext_docutils(), " X");

        let text_elem = |kind: &'static str, text: &str| {
            let mut node = Node::elem(kind, Span::ZERO);
            node.children.push(Node::text_node(text, Span::ZERO));
            node
        };
        let elem = |kind: &'static str, children: Vec<Node>| {
            let mut node = Node::elem(kind, Span::ZERO);
            node.children = children;
            node
        };
        let params = || {
            vec![
                text_elem("desc_parameter", "a"),
                text_elem("desc_parameter", "b"),
            ]
        };
        assert_eq!(
            elem("desc_parameterlist", params()).astext_docutils(),
            "(a, b)"
        );
        assert_eq!(elem("desc_optional", params()).astext_docutils(), "[a, b]");
        let type_params = elem(
            "desc_type_parameter_list",
            vec![
                text_elem("desc_type_parameter", "T"),
                text_elem("desc_type_parameter", "U"),
            ],
        );
        assert_eq!(type_params.astext_docutils(), "[T, U]");
        assert_eq!(
            text_elem("desc_returns", "int").astext_docutils(),
            " -> int"
        );
        let mut signature = elem(
            "desc_signature",
            vec![
                text_elem("desc_signature_line", "l1"),
                text_elem("desc_signature_line", "l2"),
            ],
        );
        assert_eq!(signature.astext_docutils(), "l1l2");
        signature.set("is_multiline", AttrValue::Int(1));
        assert_eq!(signature.astext_docutils(), "l1 l2");
        let xref = elem(
            kinds::PENDING_XREF,
            vec![
                text_elem(kinds::LITERAL, "a"),
                text_elem(kinds::LITERAL, "b"),
            ],
        );
        assert_eq!(xref.astext_docutils(), "ab");

        let message = messages::system_message(
            messages::WARNING,
            "Boom here.",
            "<snippet>",
            Some(3),
            vec![messages::literal_block("x")],
        );
        assert_eq!(
            message.astext_docutils(),
            "<snippet>:3: (WARNING/2) Boom here.\n\nx"
        );
        assert_eq!(message.element_astext(), "Boom here.\n\nx");
        let line_less =
            messages::system_message(messages::ERROR, "Boom.", "<snippet>", None, Vec::new());
        assert_eq!(line_less.astext_docutils(), "<snippet>:: (ERROR/3) Boom.");
    }

    /// [SQ §7.9 item 3]: every Text the inliner builds from null-escaped
    /// text keeps docutils' data (`\0` + char per backslash escape) in the
    /// side-car's `raw`, while `text` and pformat stay the unescaped
    /// `astext()`. The input is [SQ] S9's `backslash_escapes` case.
    //
    // oracle (docutils 0.22.4, parse layer): str() of the four Text nodes
    //   and the pformat, exactly as asserted below (smartquotes off).
    #[test]
    fn escaped_text_keeps_docutils_data() {
        let source = r#"\"escaped double\" and \'escaped single\' and it\'s.

Escaped dashes \-\- and \-- and ellipsis \.\.\. and \... done.

Backslash then quote \\"x" and escaped space x\ "y" and '\a'.

Escaped letter "\a" and \`backtick\` done.
"#;
        let data = [
            "\u{0}\"escaped double\u{0}\" and \u{0}'escaped single\u{0}' and it\u{0}'s.",
            "Escaped dashes \u{0}-\u{0}- and \u{0}-- and ellipsis \u{0}.\u{0}.\u{0}. and \u{0}... done.",
            "Backslash then quote \u{0}\\\"x\" and escaped space x\u{0} \"y\" and '\u{0}a'.",
            "Escaped letter \"\u{0}a\" and \u{0}`backtick\u{0}` done.",
        ];
        let pformat = concat!(
            "<document source=\"<snippet>\">\n",
            "    <paragraph>\n",
            "        \"escaped double\" and 'escaped single' and it's.\n",
            "    <paragraph>\n",
            "        Escaped dashes -- and -- and ellipsis ... and ... done.\n",
            "    <paragraph>\n",
            "        Backslash then quote \\\"x\" and escaped space x\"y\" and 'a'.\n",
            "    <paragraph>\n",
            "        Escaped letter \"a\" and `backtick` done.\n",
        );
        for sphinx in [false, true] {
            let tree = parse(source, sphinx);
            let leaves = text_leaves(&tree.root);
            let raws: Vec<Option<&str>> = leaves
                .iter()
                .map(|leaf| leaf.ext.as_deref().and_then(|ext| ext.raw.as_deref()))
                .collect();
            assert_eq!(raws, data.map(Some), "sphinx mode: {sphinx}");
            let datas: Vec<String> = leaves
                .iter()
                .map(|leaf| leaf.text_data().into_owned())
                .collect();
            assert_eq!(datas, data, "sphinx mode: {sphinx}");
            assert_eq!(tree.root.pformat(), pformat, "sphinx mode: {sphinx}");
        }

        // A Text without escape markers has nothing to keep.
        let plain = Node::text_node_escaped("no markers", Span::ZERO);
        assert!(plain.ext.is_none());
        assert_eq!(plain.text_data(), "no markers");
        let escaped = Node::text_node_escaped("a\u{0}*b\u{0} c", Span::ZERO);
        assert_eq!(escaped.text.as_deref(), Some("a*bc"));
        assert_eq!(escaped.text_data(), "a\u{0}*b\u{0} c");
    }

    /// Decision 10: `uid` and `raw` are read-stage metadata, so a side-car
    /// holding only them — or nothing — compares equal to no side-car,
    /// at any depth; `rawsource` and `pending` are tree data and count.
    #[test]
    fn node_ext_is_ignored_by_equality_except_rawsource_and_pending() {
        let plain = Node::text_node("a*b", Span::ZERO);

        let mut with_uid = plain.clone();
        with_uid.set_uid(uid(7));
        assert_eq!(with_uid.uid(), Some(uid(7)));
        assert_eq!(with_uid, plain);

        let with_raw = Node::text_node_escaped("a\u{0}*b", Span::ZERO);
        assert!(with_raw.ext.is_some());
        assert_eq!(with_raw, plain);

        let mut with_empty = plain.clone();
        with_empty.ext = Some(Box::default());
        assert_eq!(with_empty, plain);

        let mut parent = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        parent.children.push(plain.clone());
        let mut stamped = parent.clone();
        stamped.set_uid(uid(1));
        stamped.children[0] = with_uid.clone();
        assert_eq!(stamped, parent);

        let block = Node::elem(kinds::LITERAL_BLOCK, Span::ZERO);
        let mut with_rawsource = block.clone();
        with_rawsource.set_rawsource("code");
        assert_ne!(with_rawsource, block);
        with_rawsource.set_rawsource("");
        assert_eq!(with_rawsource.rawsource(), None, "'' is docutils' default");
        assert_eq!(with_rawsource, block);

        let bare = Node::elem("pending", Span::ZERO);
        let mut with_pending = bare.clone();
        with_pending.ext = Some(Box::new(NodeExt {
            pending: Some(Box::new(pending::Pending {
                transform: pending::PendingTransform::ClassAttribute,
                order: 0,
                details: Vec::new(),
            })),
            ..NodeExt::default()
        }));
        assert_ne!(with_pending, bare);
        let mut other_order = with_pending.clone();
        if let Some(pending) = other_order
            .ext
            .as_mut()
            .and_then(|ext| ext.pending.as_mut())
        {
            pending.order = 1;
        }
        assert_ne!(other_order, with_pending);
    }
}
