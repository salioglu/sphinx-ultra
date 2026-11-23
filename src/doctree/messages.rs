//! `system_message` node construction with docutils-exact shape.
//!
//! The parser never builds a message on its own: it reports it through
//! [`crate::transforms::state::DocumentState::report`], which builds the
//! node with [`system_message`] and logs it with its creation-time text.

use super::{kinds, AttrValue, Node, Span};

pub const INFO: u8 = 1;
pub const WARNING: u8 = 2;
pub const ERROR: u8 = 3;
pub const SEVERE: u8 = 4;

/// The reporter's name for `level`, a message's `type`.
pub(crate) fn type_name(level: u8) -> &'static str {
    match level {
        1 => "INFO",
        2 => "WARNING",
        3 => "ERROR",
        _ => "SEVERE",
    }
}

/// `nodes.system_message(text, *children, level=, type=, source=, line=)`:
/// `<system_message level line source type>`, a paragraph holding `text`,
/// then `children`. A message without a line has no `line` attribute.
///
/// `line` is the absolute 1-based line of the triggering source line
/// (the underline for title problems, the indented line for indent errors).
pub fn system_message(
    level: u8,
    text: &str,
    source: &str,
    line: Option<u32>,
    children: Vec<Node>,
) -> Node {
    let mut msg = Node::elem(kinds::SYSTEM_MESSAGE, Span::ZERO);
    msg.set("level", AttrValue::Int(i64::from(level)));
    if let Some(line) = line {
        msg.set("line", AttrValue::Int(i64::from(line)));
    }
    msg.set("source", AttrValue::Str(source.to_string()));
    msg.set("type", AttrValue::Str(type_name(level).to_string()));
    msg.children.push(paragraph(text));
    msg.children.extend(children);
    msg
}

/// `nodes.literal_block(raw, raw)` as a message child: the offending source
/// block (docutils reproduces e.g. the title + underline inside the
/// message).
pub fn literal_block(raw: &str) -> Node {
    let mut lb = Node::elem(kinds::LITERAL_BLOCK, Span::ZERO);
    lb.set("xml:space", AttrValue::Str("preserve".to_string()));
    lb.children.push(Node::text_node(raw, Span::ZERO));
    lb
}

/// `nodes.paragraph('', text)` as a message child (docutils' "Established
/// title styles: …").
pub fn paragraph(text: &str) -> Node {
    let mut para = Node::elem(kinds::PARAGRAPH, Span::ZERO);
    para.children.push(Node::text_node(text, Span::ZERO));
    para
}

/// Append the offending source block to a message that already exists:
/// `run_directive` adds it to a `DirectiveError`'s message after the
/// reporter created it (`SP/docutils/parsers/rst/states.py:2287-2291`), so
/// the text the reporter logged does not hold it.
pub fn with_literal(mut msg: Node, raw: &str) -> Node {
    msg.children.push(literal_block(raw));
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_message_shape() {
        let m = system_message(
            WARNING,
            "Title underline too short.",
            "<snippet>",
            Some(3),
            Vec::new(),
        );
        assert_eq!(
            m.pformat(),
            "<system_message level=\"2\" line=\"3\" source=\"<snippet>\" type=\"WARNING\">\n    <paragraph>\n        Title underline too short.\n"
        );
        // No line: no `line` attribute (docutils' line-less messages).
        let m = system_message(ERROR, "Anonymous.", "<snippet>", None, Vec::new());
        assert_eq!(
            m.pformat(),
            "<system_message level=\"3\" source=\"<snippet>\" type=\"ERROR\">\n    <paragraph>\n        Anonymous.\n"
        );
    }

    #[test]
    fn children_follow_the_text_paragraph() {
        let at_creation = system_message(
            WARNING,
            "Title underline too short.",
            "<snippet>",
            Some(2),
            vec![literal_block("Long Section Title\n======")],
        );
        let appended = with_literal(
            system_message(
                WARNING,
                "Title underline too short.",
                "<snippet>",
                Some(2),
                Vec::new(),
            ),
            "Long Section Title\n======",
        );
        let expected = "<system_message level=\"2\" line=\"2\" source=\"<snippet>\" type=\"WARNING\">\n    <paragraph>\n        Title underline too short.\n    <literal_block xml:space=\"preserve\">\n        Long Section Title\n        ======\n";
        assert_eq!(at_creation.pformat(), expected);
        assert_eq!(appended.pformat(), expected);
    }
}
