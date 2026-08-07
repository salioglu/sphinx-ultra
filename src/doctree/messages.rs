//! `system_message` node construction with docutils-exact shape.

use super::{kinds, AttrValue, Node, Span};

pub const INFO: u8 = 1;
pub const WARNING: u8 = 2;
pub const ERROR: u8 = 3;
pub const SEVERE: u8 = 4;

fn type_name(level: u8) -> &'static str {
    match level {
        1 => "INFO",
        2 => "WARNING",
        3 => "ERROR",
        _ => "SEVERE",
    }
}

/// `<system_message level line source type><paragraph>text`.
///
/// `line` is the absolute 1-based line of the triggering source line
/// (the underline for title problems, the indented line for indent errors).
pub fn system_message(level: u8, text: &str, line: u32, source: &str) -> Node {
    let mut msg = Node::elem(kinds::SYSTEM_MESSAGE, Span::ZERO);
    msg.set("level", AttrValue::Int(i64::from(level)));
    msg.set("line", AttrValue::Int(i64::from(line)));
    msg.set("source", AttrValue::Str(source.to_string()));
    msg.set("type", AttrValue::Str(type_name(level).to_string()));
    let mut para = Node::elem(kinds::PARAGRAPH, Span::ZERO);
    para.children.push(Node::text_node(text, Span::ZERO));
    msg.children.push(para);
    msg
}

/// Append the offending source block as `<literal_block xml:space="preserve">`
/// (docutils reproduces e.g. the title + underline inside the message).
pub fn with_literal(mut msg: Node, raw: &str) -> Node {
    let mut lb = Node::elem(kinds::LITERAL_BLOCK, Span::ZERO);
    lb.set("xml:space", AttrValue::Str("preserve".to_string()));
    lb.children.push(Node::text_node(raw, Span::ZERO));
    msg.children.push(lb);
    msg
}

/// Append a plain paragraph child (docutils' "Established title styles: …").
pub fn with_paragraph(mut msg: Node, text: &str) -> Node {
    let mut para = Node::elem(kinds::PARAGRAPH, Span::ZERO);
    para.children.push(Node::text_node(text, Span::ZERO));
    msg.children.push(para);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_message_shape() {
        let m = system_message(WARNING, "Title underline too short.", 3, "<snippet>");
        assert_eq!(
            m.pformat(),
            "<system_message level=\"2\" line=\"3\" source=\"<snippet>\" type=\"WARNING\">\n    <paragraph>\n        Title underline too short.\n"
        );
    }

    #[test]
    fn with_literal_appends_preserved_block() {
        let m = with_literal(
            system_message(WARNING, "Title underline too short.", 2, "<snippet>"),
            "Long Section Title\n======",
        );
        assert_eq!(
            m.pformat(),
            "<system_message level=\"2\" line=\"2\" source=\"<snippet>\" type=\"WARNING\">\n    <paragraph>\n        Title underline too short.\n    <literal_block xml:space=\"preserve\">\n        Long Section Title\n        ======\n"
        );
    }
}
