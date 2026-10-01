//! `env.metadata[docname]` — what a document's `docinfo` says, read as
//! build metadata.
//!
//! Port of `MetadataCollector.process_doc`
//! (`sphinx/environment/collectors/metadata.py:35-68`). docutils' DocInfo
//! transform (`docutils/transforms/frontmatter.py:266-548`, priority 340;
//! [`crate::transforms::frontmatter`]) turns the document's leading field
//! list into a `docinfo` node; the collector, which Sphinx runs from the
//! `doctree-read` event (priority 880, after every other read transform but
//! FilterSystemMessages), reads that node into this map and pops it from the
//! doctree. The pop and the place it happens are the read pass's
//! ([`crate::transforms::frontmatter`]'s `metadata_collector`); this module
//! is what the collector makes of the node ([`metadata_from_docinfo`]) and
//! the shape it is kept in ([`MetadataValue`]).
//!
//! The collection runs inside the read pass rather than in the merge phase
//! because it has to see the tree before FilterSystemMessages (999) does:
//! probed with `keep_warnings=False`, an empty `:author:` collects
//! `'<path>:1: (WARNING/2) Cannot extract empty bibliographic field
//! "author".'` — the text of the `system_message` DocInfo appended to the
//! field body, which the filter then removes with the rest of the docinfo
//! already gone. The read pass hands the result to the merge phase through
//! [`crate::rst::RegistryExport::metadata`].
//!
//! **Not a gap: a field list after the document's title is no metadata.**
//! Sphinx sets `doctitle_xform=False` (`sphinx/environment/__init__.py:69`),
//! so docutils' DocTitle never promotes a lone section's title: the section
//! stays the document's first child, the field list below its title stays
//! inside it, and DocInfo — which looks only at the document's own children
//! — never sees it (probed, oracle case `tx_docinfo.not_leading_after_title`:
//! the `field_list` stays in the tree, `metadata` is `{}`, and a
//! title-then-`:orphan:` document still gets `document isn't included in
//! any toctree`). Doctitle promotion must not be added.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::doctree::{kinds, Node};
use crate::transforms::frontmatter::{astext, is_bibliographic_text_element};

/// One `env.metadata[docname]` value. Sphinx's are Python objects of three
/// types (`collectors/metadata.py:47-66`): the text of a field or of a
/// bibliographic element (`str`), the `tocdepth` field coerced with `int()`
/// (`int`), and the author names of an `authors` element (`list[str]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataValue {
    Str(String),
    /// Python's `int` is unbounded; a `tocdepth` beyond `i64` saturates —
    /// which every consumer, comparing it with a depth, reads alike.
    Int(i64),
    List(Vec<String>),
}

impl MetadataValue {
    /// The value as Python's `json.dumps` writes it: a string, a number, a
    /// list of strings.
    pub fn to_json(&self) -> JsonValue {
        match self {
            MetadataValue::Str(text) => JsonValue::from(text.as_str()),
            MetadataValue::Int(number) => JsonValue::from(*number),
            MetadataValue::List(items) => JsonValue::from(items.clone()),
        }
    }
}

/// One document's metadata: name to value. Sphinx's dict keeps insertion
/// order; this map sorts by name — the same pairs (a repeated name keeps
/// its last value either way), which is all its consumers read.
pub type Metadata = BTreeMap<String, MetadataValue>;

/// What `MetadataCollector.process_doc` reads off a `docinfo` node
/// (`collectors/metadata.py:44-66`), child by child: an `authors` element
/// as the list of its authors' texts under `authors`; a (non-bibliographic,
/// or malformed bibliographic) `field` as its body's text under its name's
/// text, as written; any other — bibliographic — `TextElement` as its text
/// under its class name (`author`, `version`, ...). Then `tocdepth` is
/// coerced with Python's `int()`, 0 when that raises (`:60-66`).
///
/// Every text is docutils' `astext()` ([`astext`]) — a malformed field's
/// body holds the `system_message` DocInfo appended to it, and that is
/// collected too.
pub fn metadata_from_docinfo(docinfo: &Node) -> Metadata {
    let mut metadata = Metadata::new();
    for node in &docinfo.children {
        if node.kind == "authors" {
            let authors = node.children.iter().map(astext).collect();
            metadata.insert("authors".to_string(), MetadataValue::List(authors));
        } else if node.kind == kinds::FIELD {
            // `assert len(node) == 2`: a field is its name and its body.
            if let [name, body] = node.children.as_slice() {
                metadata.insert(astext(name), MetadataValue::Str(astext(body)));
            }
        } else if is_bibliographic_text_element(node.kind) {
            metadata.insert(node.kind.to_string(), MetadataValue::Str(astext(node)));
        }
    }
    if let Some(MetadataValue::Str(value)) = metadata.get("tocdepth") {
        let depth = python_int(value).unwrap_or(0);
        metadata.insert("tocdepth".to_string(), MetadataValue::Int(depth));
    }
    metadata
}

/// Python's `int(text)` for a `str` (CPython `PyLong_FromUnicodeObject`):
/// every non-ASCII character becomes a space if it is whitespace and its
/// ASCII digit if it is a Unicode decimal digit (anything else fails;
/// `_PyUnicode_TransformDecimalAndSpaceToASCII`), and the result is read as
/// an optionally signed base-10 numeral between ASCII whitespace, single
/// underscores allowed between digits (`PyLong_FromString`). ASCII
/// characters are taken as they are — so `\x1f`, which `str.isspace()`
/// calls whitespace, is no whitespace here. `None` where Python raises
/// `ValueError`; a value beyond `i64` saturates.
fn python_int(text: &str) -> Option<i64> {
    let mut ascii = Vec::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() && c != '\x7f' {
            ascii.push(c as u8);
        } else if crate::utils::py_isspace(c) {
            ascii.push(b' ');
        } else {
            ascii.push(b'0' + crate::utils::py_decimal(c)?);
        }
    }
    let is_space = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r');
    let start = ascii.iter().position(|b| !is_space(b))?;
    let end = ascii.iter().rposition(|b| !is_space(b))? + 1;
    let numeral = &ascii[start..end];
    match numeral.split_first() {
        Some((b'-', digits)) => digits_value(digits, true),
        Some((b'+', digits)) => digits_value(digits, false),
        _ => digits_value(numeral, false),
    }
}

/// The value of a run of ASCII digits with single underscores between
/// them (none leading or trailing), negated if `negative`, saturating.
fn digits_value(digits: &[u8], negative: bool) -> Option<i64> {
    if digits.first() == Some(&b'_') || digits.last() == Some(&b'_') {
        return None;
    }
    let mut value: i64 = 0;
    let mut any = false;
    let mut previous_underscore = false;
    for &b in digits {
        if b == b'_' {
            if previous_underscore {
                return None;
            }
            previous_underscore = true;
            continue;
        }
        previous_underscore = false;
        let digit = i64::from(b.checked_sub(b'0').filter(|d| *d <= 9)?);
        value = value.saturating_mul(10);
        value = if negative {
            value.saturating_sub(digit)
        } else {
            value.saturating_add(digit)
        };
        any = true;
    }
    any.then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst::ParseOptions;
    use crate::transforms::parse_full_and_transform;

    fn metadata(source: &str) -> Metadata {
        let opts = ParseOptions {
            source_path: "<snippet>".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            ..Default::default()
        };
        parse_full_and_transform(source, &opts).registry.metadata
    }

    fn text(value: &str) -> MetadataValue {
        MetadataValue::Str(value.to_string())
    }

    #[test]
    fn leading_field_list_becomes_metadata() {
        let metadata = metadata(":orphan:\n\nTitle\n=====\n\nBody.\n");
        assert_eq!(metadata, Metadata::from([("orphan".to_string(), text(""))]));
    }

    /// `tocdepth` is the one value Sphinx coerces: `int(value)`, and 0 when
    /// that raises (`collectors/metadata.py:60-66`).
    #[test]
    fn tocdepth_is_an_int_and_zero_when_it_is_not_one() {
        let depth = |source: &str| metadata(source).get("tocdepth").cloned();
        assert_eq!(depth(":tocdepth: 2\n"), Some(MetadataValue::Int(2)));
        assert_eq!(depth(":tocdepth: abc\n"), Some(MetadataValue::Int(0)));
        assert_eq!(depth(":tocdepth:\n"), Some(MetadataValue::Int(0)));
        // Two paragraphs: `'2\n\n3'` is no numeral.
        assert_eq!(depth(":tocdepth: 2\n\n   3\n"), Some(MetadataValue::Int(0)));
        // The name is matched as written: `TocDepth` stays a string.
        assert_eq!(metadata(":TocDepth: 2\n").get("TocDepth"), Some(&text("2")));
    }

    #[test]
    fn field_bodies_are_captured() {
        let metadata = metadata(":tocdepth: 2\n:nocomments:\n\nBody.\n");
        assert_eq!(metadata.get("tocdepth"), Some(&MetadataValue::Int(2)));
        assert_eq!(metadata.get("nocomments"), Some(&text("")));
    }

    /// Bibliographic elements are keyed by class name, whatever the case of
    /// the field name; `authors` is a list; the dedication and abstract are
    /// topics, not metadata.
    #[test]
    fn bibliographic_fields_are_keyed_by_class_name() {
        let metadata =
            metadata(":Author: Me\n:authors: A; B\n:Version: 1.0\n:abstract: Sum.\n\nBody.\n");
        assert_eq!(
            metadata,
            Metadata::from([
                ("author".to_string(), text("Me")),
                (
                    "authors".to_string(),
                    MetadataValue::List(vec!["A".to_string(), "B".to_string()])
                ),
                ("version".to_string(), text("1.0")),
            ])
        );
    }

    /// The body's text is docutils' `astext()`: block children joined by a
    /// blank line (`Element.child_text_separator`).
    #[test]
    fn a_field_body_reads_as_docutils_astext() {
        let metadata = metadata(":custom: a *b*\n\n   - c\n   - d\n\nBody.\n");
        assert_eq!(metadata.get("custom"), Some(&text("a b\n\nc\n\nd")));
    }

    #[test]
    fn comments_and_targets_do_not_hide_the_field_list() {
        // docutils skips PreBibliographic nodes before looking for docinfo.
        let metadata = metadata(".. a comment\n\n:orphan:\n\nBody.\n");
        assert!(metadata.contains_key("orphan"), "{metadata:?}");
    }

    /// `raw` is `PreBibliographic` too (`docutils/nodes.py:2600`).
    /// Verified against docutils 0.22.4: the same source yields
    /// `<document><docinfo><field classes="orphan">…` — the field list is
    /// still bibliographic with a `.. raw::` block ahead of it.
    #[test]
    fn a_leading_raw_block_does_not_hide_the_field_list() {
        let metadata = metadata(".. raw:: html\n\n   <hr>\n\n:orphan:\n\nBody.\n");
        assert_eq!(metadata.get("orphan"), Some(&text("")));
    }

    #[test]
    fn a_field_list_below_the_first_body_element_is_not_metadata() {
        let metadata = metadata("Body.\n\n:orphan:\n");
        assert!(metadata.is_empty(), "{metadata:?}");
    }

    /// The correction to the old "known gap" note: with Sphinx's
    /// `doctitle_xform=False` a field list after the title is inside the
    /// section, never the document's leading field list.
    #[test]
    fn a_field_list_after_the_title_is_not_metadata() {
        let metadata = metadata("Title\n=====\n\n:orphan:\n");
        assert!(metadata.is_empty(), "{metadata:?}");
    }

    #[test]
    fn a_document_without_fields_has_no_metadata() {
        assert!(metadata("Title\n=====\n").is_empty());
    }

    /// Python `int()` on a `str` (probed against CPython 3.12): ASCII
    /// whitespace and Unicode whitespace around an optional sign and
    /// digits, Unicode decimal digits, single underscores between digits.
    #[test]
    fn python_int_reads_what_cpython_reads() {
        for (text, expected) in [
            ("2", Some(2)),
            (" +1_0 ", Some(10)),
            ("\u{a0}+1_0", Some(10)),
            ("\u{85}1\u{85}", Some(1)),
            (" -0 ", Some(0)),
            ("-2", Some(-2)),
            ("007", Some(7)),
            ("\u{663}", Some(3)),
            ("\u{ff11}\u{ff12}", Some(12)),
            ("1__0", None),
            ("_1", None),
            ("1_", None),
            ("0x1", None),
            ("- 1", None),
            ("1\x1f", None),
            ("\x1f1", None),
            ("1\x7f", None),
            ("", None),
            ("+", None),
            ("abc", None),
            ("\u{b2}", None),
            ("123456789012345678901234567890", Some(i64::MAX)),
            ("-123456789012345678901234567890", Some(i64::MIN)),
        ] {
            assert_eq!(python_int(text), expected, "int({text:?})");
        }
    }

    #[test]
    fn values_serialize_like_python_json() {
        assert_eq!(text("x").to_json(), serde_json::json!("x"));
        assert_eq!(MetadataValue::Int(3).to_json(), serde_json::json!(3));
        assert_eq!(
            MetadataValue::List(vec!["A".to_string()]).to_json(),
            serde_json::json!(["A"])
        );
    }
}
