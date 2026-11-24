//! docutils' canonical role functions
//! (`SP/docutils/parsers/rst/roles.py:258-410`), as a docutils parse and
//! a Sphinx parse reach them ([RC §3.14]).

use crate::doctree::{kinds, unescape, AttrValue, Node};

use super::RoleCall;

/// `node_class(rawtext, text, **options)`: an element over `rawtext` with
/// the options' classes.
fn element(kind: &'static str, call: &RoleCall<'_>) -> Node {
    let mut node = Node::elem(kind, call.span);
    node.set_rawsource(call.rawtext);
    if let Some(classes) = &call.options.classes {
        node.attrs.classes = classes.clone();
    }
    node
}

/// docutils `GenericRole` (`roles.py:200-214`): the text, null escapes and
/// all, in a node of `kind`.
pub(crate) fn generic(kind: &'static str, call: &RoleCall<'_>) -> Node {
    let mut node = element(kind, call);
    node.children
        .push(Node::text_node_escaped(call.escaped, call.span));
    node
}

/// docutils `math_role` (`roles.py:381-388`): the text with its
/// backslashes restored.
pub(crate) fn math(call: &RoleCall<'_>) -> Node {
    let mut node = element(kinds::MATH, call);
    node.children
        .push(Node::text_node(unescape(call.escaped, true), call.span));
    node
}

/// docutils `raw_role` (`roles.py:322-341`): a `raw` node in the custom
/// role's format, its text with the backslashes restored; with no format,
/// the error docutils reports.
pub(crate) fn raw(call: &RoleCall<'_>) -> Result<Node, String> {
    let Some(format) = &call.options.format else {
        return Err(format!(
            "No format (Writer name) is associated with this role: \"{}\".\n\
             The \"raw\" role cannot be used directly.\n\
             Instead, use the \"role\" directive to create a new role with an \
             associated format.",
            call.name
        ));
    };
    let mut node = element("raw", call);
    node.set("format", AttrValue::Str(format.clone()));
    node.set("xml:space", AttrValue::Str("preserve".to_string()));
    node.children
        .push(Node::text_node(unescape(call.escaped, true), call.span));
    Ok(node)
}

/// docutils `pep_reference_role` (`roles.py:271-289`): a plain `reference`
/// to `pep_base_url` + `pep-%04d`, which Sphinx leaves at docutils'
/// `https://peps.python.org/`.
pub(crate) fn pep_reference(call: &RoleCall<'_>) -> Result<Node, String> {
    let text = unescape(call.escaped, false);
    match PyInt::parse(&text)
        .and_then(|n| n.value())
        .filter(|n| (0..=9999).contains(n))
    {
        Some(n) => {
            let mut node = element(kinds::REFERENCE, call);
            node.set(
                "refuri",
                AttrValue::Str(format!("https://peps.python.org/pep-{n:04}")),
            );
            node.children
                .push(Node::text_node(format!("PEP {text}"), call.span));
            Ok(node)
        }
        None => Err(format!(
            "PEP number must be a number from 0 to 9999; \"{text}\" is invalid."
        )),
    }
}

/// docutils `rfc_reference_role` (`roles.py:295-319`): a plain `reference`
/// to `rfc_base_url` + `rfc%d.html`, the part after `#` kept as the
/// fragment. Sphinx sets `rfc_base_url` to the IETF datatracker
/// (`SP/sphinx/environment/__init__.py:66`).
pub(crate) fn rfc_reference(call: &RoleCall<'_>, sphinx: bool) -> Result<Node, String> {
    let text = unescape(call.escaped, false);
    let (number, section) = match text.split_once('#') {
        Some((number, section)) => (number, Some(section)),
        None => (text.as_str(), None),
    };
    match PyInt::parse(number).filter(|n| !n.negative && n.digits != "0") {
        Some(n) => {
            let n = n.display();
            let mut node = element(kinds::REFERENCE, call);
            let base = if sphinx {
                "https://datatracker.ietf.org/doc/html/"
            } else {
                "https://tools.ietf.org/html/"
            };
            let mut uri = format!("{base}rfc{n}.html");
            if let Some(section) = section {
                uri.push('#');
                uri.push_str(section);
            }
            node.set("refuri", AttrValue::Str(uri));
            node.children
                .push(Node::text_node(format!("RFC {n}"), call.span));
            Ok(node)
        }
        // `'... "%s" is invalid.' % text`: the whole text, the section
        // included.
        None => Err(format!(
            "RFC number must be a number greater than or equal to 1; \"{text}\" is invalid."
        )),
    }
}

/// docutils `code_role` (`roles.py:344-378`): a `literal` with `code` and
/// the options' classes, then the language, over the text with its
/// backslashes restored. docutils tokenizes the text of a role with a
/// language through Pygments; that is not ported, and such a role keeps
/// its text whole.
pub(crate) fn docutils_code(call: &RoleCall<'_>) -> Node {
    let mut node = Node::elem(kinds::LITERAL, call.span);
    node.set_rawsource(call.rawtext);
    let mut classes = vec!["code".to_string()];
    classes.extend(call.options.classes.iter().flatten().cloned());
    if let Some(language) = call.options.language.as_ref().filter(|l| !l.is_empty()) {
        if !classes.contains(language) {
            classes.push(language.clone());
        }
    }
    node.attrs.classes = classes;
    node.children
        .push(Node::text_node(unescape(call.escaped, true), call.span));
    node
}

/// docutils `unimplemented_role` (`roles.py:391-397`): the error, naming
/// the role as the document wrote it.
pub(crate) fn unimplemented(call: &RoleCall<'_>) -> String {
    format!("Interpreted text role \"{}\" not implemented.", call.name)
}

/// A Python `int`, as `int(text)` makes one of a string: its sign and its
/// canonical decimal digits, at any precision (`crate::pyformat::int`:
/// surrounding whitespace, one sign, Unicode decimal digits and single
/// underscores between them, no more than 4,300 of them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PyInt {
    pub(crate) negative: bool,
    pub(crate) digits: String,
}

impl PyInt {
    /// `int(text)`, or `None` where Python raises `ValueError`.
    pub(crate) fn parse(text: &str) -> Option<PyInt> {
        let (negative, digits) = crate::pyformat::int(text).ok()?;
        Some(PyInt { negative, digits })
    }

    /// The value, when it fits an `i64` (every value a range check needs).
    pub(crate) fn value(&self) -> Option<i64> {
        let magnitude: i64 = self.digits.parse().ok()?;
        Some(if self.negative { -magnitude } else { magnitude })
    }

    /// `'%d' % n`.
    pub(crate) fn display(&self) -> String {
        if self.negative {
            format!("-{}", self.digits)
        } else {
            self.digits.clone()
        }
    }

    /// `'%0<width>d' % n`: zeros after the sign up to `width` characters.
    pub(crate) fn zero_padded(&self, width: usize) -> String {
        let sign = if self.negative { "-" } else { "" };
        let pad = width.saturating_sub(sign.len() + self.digits.len());
        format!("{sign}{}{}", "0".repeat(pad), self.digits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Python's `int()` and `%d` formatting over the strings the PEP and
    /// RFC roles parse.
    #[test]
    fn py_int_parses_and_formats_like_python() {
        for (text, display, padded) in [
            ("8", "8", "0008"),
            (" 8 ", "8", "0008"),
            ("008", "8", "0008"),
            ("+3", "3", "0003"),
            ("-1", "-1", "-001"),
            ("1_000", "1000", "1000"),
            ("12345", "12345", "12345"),
            (
                "99999999999999999999",
                "99999999999999999999",
                "99999999999999999999",
            ),
            ("\u{663}", "3", "0003"),
        ] {
            let n = PyInt::parse(text).unwrap_or_else(|| panic!("{text:?} parses"));
            assert_eq!(
                (n.display(), n.zero_padded(4)),
                (display.to_string(), padded.to_string())
            );
        }
        for text in ["1__0", "_1", "1_", "abc", "", "1.0", "2 #page-3"] {
            assert_eq!(PyInt::parse(text), None, "{text:?}");
        }
        assert_eq!(
            PyInt::parse("-0").map(|n| n.display()),
            Some("0".to_string())
        );
        assert_eq!(PyInt::parse("10000").and_then(|n| n.value()), Some(10000));
    }
}
