//! Pseudo-XML emitter, byte-identical to docutils `document.pformat()`.
//!
//! Rules (probed against docutils 0.22.4, see the wave-1 probe notes):
//! - Element line: 4-space indent per depth, `<kind attrs>`, no closing tags.
//! - Attributes: the five list attributes (backrefs/classes/dupnames/ids/
//!   names, empty ones suppressed) merge with scalar attributes into ONE
//!   alphabetically-sorted sequence. Scalars always print.
//! - NO XML escaping anywhere. List-attribute items go through docutils
//!   `serial_escape` (`\` -> `\\`, then ` ` -> `\ `) and join with a space.
//! - Text nodes: each line of the text on its own line at child indent.
//! - Every emitted line ends with `\n`.

use super::{kinds, AttrValue, Node};

fn serial_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace(' ', "\\ ")
}

fn push_list_attr(out: &mut Vec<(&str, String)>, name: &'static str, values: &[String]) {
    if !values.is_empty() {
        let joined = values
            .iter()
            .map(|v| serial_escape(v))
            .collect::<Vec<_>>()
            .join(" ");
        out.push((name, joined));
    }
}

fn write_node(node: &Node, depth: usize, out: &mut String) {
    let indent = "    ".repeat(depth);
    if node.kind == kinds::TEXT {
        if let Some(text) = &node.text {
            // Python str.splitlines() semantics: a trailing newline does
            // NOT produce a final empty line (interior empties are kept).
            let mut lines: Vec<&str> = text.split('\n').collect();
            if lines.len() > 1 && lines.last() == Some(&"") {
                lines.pop();
            }
            for line in lines {
                out.push_str(&indent);
                out.push_str(line);
                out.push('\n');
            }
        }
        return;
    }

    let mut attrs: Vec<(&str, String)> = Vec::new();
    push_list_attr(&mut attrs, "backrefs", &node.attrs.backrefs);
    push_list_attr(&mut attrs, "classes", &node.attrs.classes);
    push_list_attr(&mut attrs, "dupnames", &node.attrs.dupnames);
    push_list_attr(&mut attrs, "ids", &node.attrs.ids);
    push_list_attr(&mut attrs, "names", &node.attrs.names);
    for (key, value) in &node.attrs.extra {
        let rendered = match value {
            AttrValue::Int(i) => i.to_string(),
            AttrValue::Str(s) => s.clone(),
        };
        attrs.push((key, rendered));
    }
    attrs.sort_by(|a, b| a.0.cmp(b.0));

    out.push_str(&indent);
    out.push('<');
    out.push_str(node.kind);
    for (name, value) in &attrs {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        out.push_str(value);
        out.push('"');
    }
    out.push_str(">\n");

    for child in &node.children {
        write_node(child, depth + 1, out);
    }
}

pub fn pformat(node: &Node) -> String {
    let mut out = String::new();
    write_node(node, 0, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use crate::doctree::{kinds, AttrValue, Node, Span};

    #[test]
    fn pformat_section_with_attrs() {
        let mut doc = Node::elem(kinds::DOCUMENT, Span::ZERO);
        doc.set("source", AttrValue::Str("<snippet>".into()));
        let mut sec = Node::elem(kinds::SECTION, Span::ZERO);
        sec.attrs.ids.push("title".into());
        sec.attrs.names.push("title".into());
        let mut title = Node::elem(kinds::TITLE, Span::ZERO);
        title.children.push(Node::text_node("Title", Span::ZERO));
        sec.children.push(title);
        doc.children.push(sec);
        assert_eq!(
            doc.pformat(),
            "<document source=\"<snippet>\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n"
        );
    }

    #[test]
    fn pformat_serial_escapes_spaces_in_list_values() {
        let mut sec = Node::elem(kinds::SECTION, Span::ZERO);
        sec.attrs.ids.push("my-section-title".into());
        sec.attrs.dupnames.push("my section title!".into());
        assert_eq!(
            sec.pformat(),
            "<section dupnames=\"my\\ section\\ title!\" ids=\"my-section-title\">\n"
        );
    }

    #[test]
    fn pformat_serial_escapes_backslashes_before_spaces() {
        // Probe B: `.. _a\\b:` -> names="a\\\\b" (value `a\b` -> `a\\b`)
        let mut t = Node::elem(kinds::TARGET, Span::ZERO);
        t.attrs.names.push("a\\b".into());
        assert_eq!(t.pformat(), "<target names=\"a\\\\b\">\n");
    }

    #[test]
    fn pformat_does_not_xml_escape() {
        // Probe B: quotes, angle brackets, ampersands print raw.
        let mut t = Node::elem(kinds::TARGET, Span::ZERO);
        t.attrs.names.push("a \"quote\" <b> & c".into());
        t.set("refuri", AttrValue::Str("https://x/?q=1&r=2".into()));
        assert_eq!(
            t.pformat(),
            "<target names=\"a\\ \"quote\"\\ <b>\\ &\\ c\" refuri=\"https://x/?q=1&r=2\">\n"
        );
        let mut p = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        p.children
            .push(Node::text_node("x < y & z > w", Span::ZERO));
        assert_eq!(p.pformat(), "<paragraph>\n    x < y & z > w\n");
    }

    #[test]
    fn pformat_system_message_scalar_attrs_sorted() {
        let mut m = Node::elem(kinds::SYSTEM_MESSAGE, Span::ZERO);
        m.set("level", AttrValue::Int(2));
        m.set("line", AttrValue::Int(3));
        m.set("source", AttrValue::Str("<snippet>".into()));
        m.set("type", AttrValue::Str("WARNING".into()));
        let mut p = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        p.children
            .push(Node::text_node("Title underline too short.", Span::ZERO));
        m.children.push(p);
        assert_eq!(
            m.pformat(),
            "<system_message level=\"2\" line=\"3\" source=\"<snippet>\" type=\"WARNING\">\n    <paragraph>\n        Title underline too short.\n"
        );
    }

    #[test]
    fn pformat_list_and_scalar_attrs_interleave_alphabetically() {
        // backrefs (list) sorts before level/line/... (scalars): one sequence.
        let mut m = Node::elem(kinds::SYSTEM_MESSAGE, Span::ZERO);
        m.attrs.backrefs.push("id1".into());
        m.set("level", AttrValue::Int(1));
        m.set("line", AttrValue::Int(7));
        m.set("source", AttrValue::Str("<snippet>".into()));
        m.set("type", AttrValue::Str("INFO".into()));
        assert_eq!(
            m.pformat(),
            "<system_message backrefs=\"id1\" level=\"1\" line=\"7\" source=\"<snippet>\" type=\"INFO\">\n"
        );
    }

    #[test]
    fn pformat_multiline_text_indents_each_line() {
        let mut p = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        p.children.push(Node::text_node("Title\n===", Span::ZERO));
        assert_eq!(p.pformat(), "<paragraph>\n    Title\n    ===\n");
    }

    #[test]
    fn pformat_xml_space_preserve() {
        let mut lb = Node::elem(kinds::LITERAL_BLOCK, Span::ZERO);
        lb.set("xml:space", AttrValue::Str("preserve".into()));
        lb.children.push(Node::text_node("code here", Span::ZERO));
        assert_eq!(
            lb.pformat(),
            "<literal_block xml:space=\"preserve\">\n    code here\n"
        );
    }

    #[test]
    fn pformat_empty_element_prints_bare_tag() {
        // Probe: empty list_item / comment / transition print as a bare tag line.
        let li = Node::elem(kinds::LIST_ITEM, Span::ZERO);
        assert_eq!(li.pformat(), "<list_item>\n");
        let t = Node::elem(kinds::TRANSITION, Span::ZERO);
        assert_eq!(t.pformat(), "<transition>\n");
    }
}
