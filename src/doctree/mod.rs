//! Typed doctree IR with docutils-equivalent node semantics (M2 wave 1).
//!
//! Design (recorded in docs/superpowers/plans/2026-08-07-m2-wave-map.md):
//! docutils-mirror generic node — node identity and attributes are data, not
//! Rust types, so the pseudo-XML parity serializer is a direct dump and
//! docutils transforms/writers port line-by-line. One `Node` struct covers
//! every element type; `kind` holds the docutils tagname from [`kinds`].
//!
//! Source spans are structural: every node carries a byte-offset [`Span`]
//! into the original source (docutils itself only keeps `(source, line)`).
//! Spans are line-granular in wave 1; the wave-2 inline parser refines them.

pub mod ids;
pub mod kinds;
pub mod messages;
pub mod pformat;

/// Byte-offset range into a source file. `source` indexes a per-doctree
/// source table; wave 1 always uses source 0 (`include` arrives in wave 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub source: u16,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const ZERO: Span = Span {
        source: 0,
        start: 0,
        end: 0,
    };
}

/// Scalar attribute value. docutils attribute dicts hold ints and strings for
/// everything wave 1 emits; list-valued attributes live in [`Attrs`]' typed
/// fields instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrValue {
    Int(i64),
    Str(String),
}

/// docutils' universal list attributes (`basic_attributes` + `backrefs`) as
/// typed fields, plus an open, name-sorted list for everything else
/// (`refuri`, `enumtype`, `level`, …). `pformat` merges both sets and prints
/// all pairs in one alphabetical sequence, exactly like docutils `attlist()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attrs {
    pub ids: Vec<String>,
    pub names: Vec<String>,
    pub dupnames: Vec<String>,
    pub classes: Vec<String>,
    pub backrefs: Vec<String>,
    /// Kept sorted by key; use [`Node::set`] to maintain the invariant.
    pub extra: Vec<(&'static str, AttrValue)>,
}

/// One doctree node. Element nodes have `text == None`; text leaves have
/// `kind == kinds::TEXT`, `Some(text)`, and no children or attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: &'static str,
    pub span: Span,
    pub text: Option<String>,
    pub attrs: Attrs,
    pub children: Vec<Node>,
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
#[derive(Debug, Clone, PartialEq)]
pub struct Doctree {
    pub root: Node,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elem_constructs_with_kind_and_span() {
        let n = Node::elem(
            kinds::PARAGRAPH,
            Span {
                source: 0,
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
    fn astext_joins_text_descendants() {
        let mut p = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        p.children
            .push(Node::text_node("line one\nline two", Span::ZERO));
        assert_eq!(p.astext(), "line one\nline two");
    }
}
