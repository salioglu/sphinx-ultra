//! The inline parser (M2 wave 2): a port of docutils `states.py Inliner`
//! for emphasis/strong/literal recognition, escape semantics, and the
//! problematic/unclosed machinery. References, roles, and footnote
//! references extend this module in later wave-2 tasks.
//!
//! Escape model (probe-verified): backslash escapes convert to a `\x00`
//! marker + char before recognition (`escape2null`); at Text emission the
//! markers strip (`\x00 ` and `\x00\n` remove BOTH chars, joining words;
//! bare `\x00` drops, keeping the escaped char). Inline literals restore
//! backslashes instead. Behavior sources: the wave-2 probe notes
//! (2026-08-07-m2-wave2-probes.md) and the differential fixture — never
//! memory.

use crate::doctree::ids::IdRegistry;
use crate::doctree::{kinds, messages, AttrValue, Node, Span};

use super::punctuation;

const NULL: char = '\u{0}';

/// docutils `utils.escape2null`: every `\` + char becomes `\x00` + char; a
/// trailing lone backslash becomes a bare `\x00`.
pub fn escape2null(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(NULL);
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// docutils `nodes.unescape`: drop `\x00 ` and `\x00\n` (both chars),
/// then bare `\x00` markers. With `restore_backslashes` every marker
/// becomes a literal `\` instead (inline literals).
pub fn unescape(text: &str, restore_backslashes: bool) -> String {
    if restore_backslashes {
        return text.replace(NULL, "\\");
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == NULL {
            match chars.peek() {
                Some(' ') | Some('\n') => {
                    chars.next();
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Result of inline-parsing one text block: the paragraph/title children,
/// plus system_messages the caller attaches AFTER the enclosing element.
pub struct InlineResult {
    pub nodes: Vec<Node>,
    pub messages: Vec<Node>,
}

fn is_start_prefix_ok(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || punctuation::OPENERS.contains(&c)
                || punctuation::DELIMITERS.contains(&c)
        }
    }
}

fn is_end_suffix_ok(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || c == NULL
                || punctuation::CLOSING_DELIMITERS.contains(&c)
                || punctuation::DELIMITERS.contains(&c)
                || punctuation::CLOSERS.contains(&c)
        }
    }
}

/// docutils `punctuation_chars.match_chars(c1, c2)`: positional pairing on
/// the RAW opener/closer strings plus the extra quote pairs.
fn match_chars(c1: char, c2: char) -> bool {
    match punctuation::OPENERS_RAW.iter().position(|c| *c == c1) {
        None => false,
        Some(i) => {
            if punctuation::CLOSERS_RAW.get(i) == Some(&c2) {
                return true;
            }
            punctuation::QUOTE_PAIRS
                .iter()
                .any(|(k, v)| *k == c1 && v.contains(c2))
        }
    }
}

struct Inliner<'a> {
    /// escape2null'd text as a char vector (positions are char indices).
    chars: Vec<char>,
    span: Span,
    lineno: u32,
    source_path: &'a str,
    registry: &'a mut IdRegistry,
    nodes: Vec<Node>,
    messages: Vec<Node>,
    /// Text accumulated since the last emitted inline node.
    pending: String,
}

impl<'a> Inliner<'a> {
    /// docutils `quoted_start`: the start-string is suppressed silently
    /// when enclosed in a matching delimiter pair, or when it sits at the
    /// very end of the text.
    fn quoted_start(&self, start: usize, len: usize) -> bool {
        let post = match self.chars.get(start + len) {
            None => return true, // start-string at end of text: silent skip
            Some(c) => *c,
        };
        if start == 0 {
            return false;
        }
        let pre = self.chars[start - 1];
        match_chars(pre, post)
    }

    fn start_ok(&self, at: usize, len: usize) -> bool {
        let prev = at.checked_sub(1).map(|i| self.chars[i]);
        if !is_start_prefix_ok(prev) {
            return false;
        }
        // non_whitespace_after: the char following the start-string.
        match self.chars.get(at + len) {
            Some(c) if c.is_whitespace() => false,
            _ => !self.quoted_start(at, len),
        }
    }

    /// Find the end-string: the FIRST position satisfying both the
    /// lookbehind and lookahead, searching from `from`, with non-empty
    /// content (docutils `endmatch.start(1) >= 1`). Lookbehind: emphasis/
    /// strong forbid whitespace AND `\x00` before the end-string; literals
    /// forbid only whitespace (`allow_null_before`).
    fn find_end(&self, from: usize, end_str: &[char], allow_null_before: bool) -> Option<usize> {
        let n = self.chars.len();
        let len = end_str.len();
        // content non-empty: end-string can start at from+1 at the earliest
        let mut i = from + 1;
        while i + len <= n {
            if self.chars[i..i + len] == *end_str {
                let prev = self.chars[i - 1];
                let ok_behind = if allow_null_before {
                    !prev.is_whitespace()
                } else {
                    !prev.is_whitespace() && prev != NULL
                };
                let ok_ahead = is_end_suffix_ok(self.chars.get(i + len).copied());
                if ok_behind && ok_ahead {
                    return Some(i);
                }
            }
            i += 1;
        }
        None
    }

    fn flush_text(&mut self) {
        if !self.pending.is_empty() {
            let text = unescape(&self.pending, false);
            if !text.is_empty() {
                self.nodes.push(Node::text_node(text, self.span));
            }
            self.pending.clear();
        }
    }

    fn emit_inline(&mut self, kind: &'static str, content: &str, restore: bool) {
        self.flush_text();
        let mut node = Node::elem(kind, self.span);
        node.children
            .push(Node::text_node(unescape(content, restore), self.span));
        self.nodes.push(node);
    }

    /// Unclosed start-string: `problematic` holding only the start-string
    /// + a WARNING message; the rest of the text re-scans.
    fn emit_problematic(&mut self, start_str: &str, construct: &str) {
        self.flush_text();
        let msg_id = self.registry.allocate_auto_id();
        let prob_id = self.registry.allocate_auto_id();
        let mut prob = Node::elem(kinds::PROBLEMATIC, self.span);
        prob.attrs.ids.push(prob_id.clone());
        prob.set("refid", AttrValue::Str(msg_id.clone()));
        prob.children
            .push(Node::text_node(start_str.to_string(), self.span));
        self.nodes.push(prob);
        let mut msg = messages::system_message(
            messages::WARNING,
            &format!("Inline {construct} start-string without end-string."),
            self.lineno,
            self.source_path,
        );
        msg.attrs.ids.push(msg_id);
        msg.attrs.backrefs.push(prob_id);
        self.messages.push(msg);
    }

    fn run(&mut self) {
        let n = self.chars.len();
        let mut i = 0usize;
        while i < n {
            let c = self.chars[i];
            let (start_len, kind, end_str, construct, restore): (
                usize,
                &'static str,
                &[char],
                &str,
                bool,
            ) = if c == '*' && self.chars.get(i + 1) == Some(&'*') {
                (2, kinds::STRONG, &['*', '*'], "strong", false)
            } else if c == '*' {
                (1, kinds::EMPHASIS, &['*'], "emphasis", false)
            } else if c == '`' && self.chars.get(i + 1) == Some(&'`') {
                (2, kinds::LITERAL, &['`', '`'], "literal", true)
            } else {
                self.pending.push(c);
                i += 1;
                continue;
            };

            if !self.start_ok(i, start_len) {
                self.pending.push(c);
                i += 1;
                continue;
            }
            let content_from = i + start_len;
            // Literal lookbehind allows \x00 (restore==true implies literal).
            match self.find_end(content_from, end_str, restore) {
                Some(end) => {
                    let content: String = self.chars[content_from..end].iter().collect();
                    self.emit_inline(kind, &content, restore);
                    i = end + end_str.len();
                }
                None => {
                    let start_str: String = self.chars[i..i + start_len].iter().collect();
                    self.emit_problematic(&start_str, construct);
                    i += start_len;
                }
            }
        }
        self.flush_text();
    }
}

/// Inline-parse one logical text block (a paragraph's joined lines, a
/// title, a term…). `lineno` anchors any generated messages.
pub fn parse_inline(
    text: &str,
    span: Span,
    lineno: u32,
    registry: &mut IdRegistry,
    source_path: &str,
) -> InlineResult {
    let escaped = escape2null(text);
    let mut inliner = Inliner {
        chars: escaped.chars().collect(),
        span,
        lineno,
        source_path,
        registry,
        nodes: Vec::new(),
        messages: Vec::new(),
        pending: String::new(),
    };
    inliner.run();
    InlineResult {
        nodes: inliner.nodes,
        messages: inliner.messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape2null_and_unescape_roundtrip() {
        assert_eq!(escape2null("a\\*b"), "a\u{0}*b");
        assert_eq!(unescape("a\u{0}*b", false), "a*b");
        assert_eq!(unescape("one\u{0} two", false), "onetwo"); // escaped space joins
        assert_eq!(unescape("a\u{0}\\b", true), "a\\\\b"); // restore: marker -> backslash
    }

    fn pi(text: &str) -> (Vec<Node>, Vec<Node>) {
        let mut reg = IdRegistry::new();
        let r = parse_inline(text, Span::ZERO, 1, &mut reg, "<snippet>");
        (r.nodes, r.messages)
    }

    #[test]
    fn plain_text_stays_single_node() {
        let (nodes, msgs) = pi("just text");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].text.as_deref(), Some("just text"));
        assert!(msgs.is_empty());
    }

    #[test]
    fn simple_emphasis_strong_literal() {
        let (nodes, _) = pi("before *emph* after");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].text.as_deref(), Some("before "));
        assert_eq!(nodes[1].kind, kinds::EMPHASIS);
        assert_eq!(nodes[1].astext(), "emph");
        assert_eq!(nodes[2].text.as_deref(), Some(" after"));

        let (nodes, _) = pi("*a* **b** ``c``");
        assert_eq!(nodes.len(), 5); // em, " ", strong, " ", literal
        assert_eq!(nodes[2].kind, kinds::STRONG);
        assert_eq!(nodes[4].kind, kinds::LITERAL);
    }

    #[test]
    fn word_chars_block_recognition() {
        let (nodes, msgs) = pi("a*b*c");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].text.as_deref(), Some("a*b*c"));
        assert!(msgs.is_empty());
    }

    #[test]
    fn quoted_start_suppresses_silently() {
        let (nodes, msgs) = pi("\"*\"");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].text.as_deref(), Some("\"*\""));
        assert!(msgs.is_empty());
        let (nodes, msgs) = pi("word *");
        assert_eq!(nodes[0].text.as_deref(), Some("word *"));
        assert!(msgs.is_empty());
    }

    #[test]
    fn unclosed_produces_problematic_and_message() {
        let (nodes, msgs) = pi("*oops");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].kind, kinds::PROBLEMATIC);
        assert_eq!(nodes[0].astext(), "*");
        assert_eq!(nodes[0].attrs.ids, vec!["id2"]);
        assert_eq!(nodes[0].get("refid"), Some(&AttrValue::Str("id1".into())));
        assert_eq!(nodes[1].text.as_deref(), Some("oops"));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].attrs.ids, vec!["id1"]);
        assert_eq!(msgs[0].attrs.backrefs, vec!["id2"]);
        assert_eq!(
            msgs[0].children[0].astext(),
            "Inline emphasis start-string without end-string."
        );
    }

    #[test]
    fn no_nesting_and_first_end_wins() {
        let (nodes, _) = pi("**a *b* c**");
        assert_eq!(nodes[0].kind, kinds::STRONG);
        assert_eq!(nodes[0].astext(), "a *b* c");
        let (nodes, _) = pi("*word *word*");
        assert_eq!(nodes[0].kind, kinds::EMPHASIS);
        assert_eq!(nodes[0].astext(), "word *word");
        let (nodes, _) = pi("***x***");
        assert_eq!(nodes[0].kind, kinds::STRONG);
        assert_eq!(nodes[0].astext(), "*x*");
    }

    #[test]
    fn literal_restores_backslashes() {
        let (nodes, _) = pi("``a\\*b``");
        assert_eq!(nodes[0].kind, kinds::LITERAL);
        assert_eq!(nodes[0].astext(), "a\\*b");
    }

    #[test]
    fn escapes_disappear_in_plain_text() {
        let (nodes, _) = pi("\\*not markup\\*");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].text.as_deref(), Some("*not markup*"));
    }
}
