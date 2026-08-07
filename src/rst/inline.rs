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

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// docutils `simplename`: `(?:(?!_)\w)+(?:[-._+:](?:(?!_)\w)+)*` — word-char
/// atoms (underscore excluded) joined by single `-._+:` separators. Returns
/// the char length of the match starting at `at`, or None.
fn match_simplename(chars: &[char], at: usize) -> Option<usize> {
    let n = chars.len();
    let mut i = at;
    let atom = |c: char| is_word_char(c) && c != '_';
    if i >= n || !atom(chars[i]) {
        return None;
    }
    while i < n && atom(chars[i]) {
        i += 1;
    }
    loop {
        if i < n && matches!(chars[i], '-' | '.' | '_' | '+' | ':') {
            let sep_end = i + 1;
            if sep_end < n && atom(chars[sep_end]) {
                i = sep_end + 1;
                while i < n && atom(chars[i]) {
                    i += 1;
                }
                continue;
            }
        }
        break;
    }
    Some(i - at)
}

fn is_urilast(c: char) -> bool {
    matches!(c, '_' | '~' | '*' | '/' | '=' | '+') || c.is_ascii_alphanumeric()
}

fn is_uric(c: char) -> bool {
    matches!(
        c,
        '-' | '_'
            | '.'
            | '!'
            | '~'
            | '*'
            | '\''
            | '('
            | ')'
            | '['
            | ']'
            | ';'
            | '/'
            | ':'
            | '@'
            | '&'
            | '='
            | '+'
            | '$'
            | ','
            | '%'
            | '\u{0}'
    ) || c.is_ascii_alphanumeric()
}

fn is_emailc(c: char) -> bool {
    matches!(
        c,
        '-' | '_'
            | '!'
            | '~'
            | '*'
            | '\''
            | '{'
            | '|'
            | '}'
            | '/'
            | '#'
            | '?'
            | '^'
            | '`'
            | '&'
            | '='
            | '+'
            | '$'
            | '%'
            | '\u{0}'
    ) || c.is_ascii_alphanumeric()
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
            let pending = std::mem::take(&mut self.pending);
            self.implicit_inline(&pending);
        }
    }

    /// docutils `implicit_inline`: standalone URIs and emails inside plain
    /// text runs become reference nodes.
    fn implicit_inline(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut emitted_upto = 0usize;
        let mut i = 0usize;
        while i < n {
            let prev = i.checked_sub(1).map(|p| chars[p]);
            let at_start = is_start_prefix_ok(prev);
            if at_start {
                if let Some((len, refuri, display)) = match_standalone_uri(&chars, i) {
                    let before: String = chars[emitted_upto..i].iter().collect();
                    let before = unescape(&before, false);
                    if !before.is_empty() {
                        self.nodes.push(Node::text_node(before, self.span));
                    }
                    let mut r = Node::elem(kinds::REFERENCE, self.span);
                    r.set("refuri", AttrValue::Str(unescape(&refuri, false)));
                    r.children
                        .push(Node::text_node(unescape(&display, false), self.span));
                    self.nodes.push(r);
                    i += len;
                    emitted_upto = i;
                    continue;
                }
            }
            i += 1;
        }
        let rest: String = chars[emitted_upto..].iter().collect();
        let rest = unescape(&rest, false);
        if !rest.is_empty() {
            self.nodes.push(Node::text_node(rest, self.span));
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
            // simple span constructs: strong / emphasis / literal
            let simple: Option<(usize, &'static str, &[char], &str, bool)> =
                if c == '*' && self.chars.get(i + 1) == Some(&'*') {
                    Some((2, kinds::STRONG, &['*', '*'], "strong", false))
                } else if c == '*' {
                    Some((1, kinds::EMPHASIS, &['*'], "emphasis", false))
                } else if c == '`' && self.chars.get(i + 1) == Some(&'`') {
                    Some((2, kinds::LITERAL, &['`', '`'], "literal", true))
                } else {
                    None
                };
            if let Some((start_len, kind, end_str, construct, restore)) = simple {
                if !self.start_ok(i, start_len) {
                    self.pending.push(c);
                    i += 1;
                    continue;
                }
                let content_from = i + start_len;
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
                continue;
            }
            if c == '_' && self.chars.get(i + 1) == Some(&'`') && self.start_ok(i, 2) {
                i = self.inline_internal_target(i);
                continue;
            }
            if c == '`' && self.start_ok(i, 1) {
                i = self.backtick_construct(i);
                continue;
            }
            if c == '[' && self.start_ok(i, 1) {
                if let Some(next_i) = self.try_footnote_or_citation(i) {
                    i = next_i;
                    continue;
                }
                self.pending.push(c);
                i += 1;
                continue;
            }
            if c == '|' && self.start_ok(i, 1) {
                i = self.substitution_ref(i);
                continue;
            }
            if is_word_char(c) && c != '_' {
                let prev = i.checked_sub(1).map(|p| self.chars[p]);
                if is_start_prefix_ok(prev) {
                    if let Some(next_i) = self.try_word_reference(i) {
                        i = next_i;
                        continue;
                    }
                }
                // consume the whole word so inner chars never re-dispatch
                let mut j = i;
                while j < n && is_word_char(self.chars[j]) {
                    self.pending.push(self.chars[j]);
                    j += 1;
                }
                i = j;
                continue;
            }
            self.pending.push(c);
            i += 1;
        }
        self.flush_text();
    }

    /// `word_` / `word__` references.
    fn try_word_reference(&mut self, i: usize) -> Option<usize> {
        let len = match_simplename(&self.chars, i)?;
        let mut end = i + len;
        let mut underscores = 0usize;
        while underscores < 2 && self.chars.get(end) == Some(&'_') {
            end += 1;
            underscores += 1;
        }
        if underscores == 0 || !is_end_suffix_ok(self.chars.get(end).copied()) {
            return None;
        }
        // the name itself must not end with the separator swallowing the
        // ref underscores: match_simplename never consumes trailing '_'
        let name: String = self.chars[i..i + len].iter().collect();
        if name.contains(NULL) {
            return None;
        }
        self.flush_text();
        let mut r = Node::elem(kinds::REFERENCE, self.span);
        if underscores == 2 {
            r.set("anonymous", AttrValue::Int(1));
            r.set(
                "name",
                AttrValue::Str(crate::doctree::ids::whitespace_normalize_name(&name)),
            );
        } else {
            r.set(
                "name",
                AttrValue::Str(crate::doctree::ids::whitespace_normalize_name(&name)),
            );
            r.set(
                "refname",
                AttrValue::Str(crate::doctree::ids::fully_normalize_name(&name)),
            );
        }
        r.children.push(Node::text_node(name, self.span));
        self.nodes.push(r);
        Some(end)
    }

    /// Find an end char that may carry 0..=max_underscores trailing
    /// underscores BEFORE the end-suffix check (phrase refs, substitution
    /// refs: the underscores are part of the end-string pattern). Returns
    /// (end_index, underscore_count).
    fn find_end_with_underscores(
        &self,
        from: usize,
        end_char: char,
        max_underscores: usize,
    ) -> Option<(usize, usize)> {
        let n = self.chars.len();
        let mut i = from + 1;
        while i < n {
            if self.chars[i] == end_char {
                let prev = self.chars[i - 1];
                if !prev.is_whitespace() && prev != NULL {
                    let mut after = i + 1;
                    let mut u = 0usize;
                    while u < max_underscores && self.chars.get(after) == Some(&'_') {
                        after += 1;
                        u += 1;
                    }
                    if is_end_suffix_ok(self.chars.get(after).copied()) {
                        return Some((i, u));
                    }
                }
            }
            i += 1;
        }
        None
    }

    /// `_`marked text`` inline internal targets.
    fn inline_internal_target(&mut self, i: usize) -> usize {
        let content_from = i + 2;
        match self.find_end(content_from, &['`'], false) {
            Some(end) => {
                let raw: String = self.chars[content_from..end].iter().collect();
                self.flush_text();
                let mut t = Node::elem(kinds::TARGET, self.span);
                t.attrs
                    .names
                    .push(crate::doctree::ids::fully_normalize_name(&unescape(
                        &raw, false,
                    )));
                let msg = self.registry.set_id_explicit(
                    &mut t,
                    self.lineno,
                    self.source_path,
                    true,
                    None,
                );
                t.children
                    .push(Node::text_node(unescape(&raw, false), self.span));
                self.nodes.push(t);
                if let Some(m) = msg {
                    self.messages.push(m);
                }
                end + 1
            }
            None => {
                self.emit_problematic("_`", "internal target");
                i + 2
            }
        }
    }

    /// Backtick constructs: phrase references (trailing `_`/`__`) or
    /// interpreted text (default role: title_reference).
    fn backtick_construct(&mut self, i: usize) -> usize {
        let content_from = i + 1;
        let (end, underscores) = match self.find_end_with_underscores(content_from, '`', 2) {
            Some(r) => r,
            None => {
                self.emit_problematic("`", "interpreted text or phrase reference");
                return i + 1;
            }
        };
        let raw: String = self.chars[content_from..end].iter().collect();
        if underscores > 0 {
            self.phrase_reference(&raw, underscores);
            return end + 1 + underscores;
        }
        // interpreted text, default role
        self.emit_inline(kinds::TITLE_REFERENCE, &raw, false);
        end + 1
    }

    /// Phrase reference body handling incl. embedded `<uri>`/`<alias_>`.
    fn phrase_reference(&mut self, raw: &str, underscores: usize) {
        self.flush_text();
        let ids = |s: &str| crate::doctree::ids::fully_normalize_name(s);
        let wsn = |s: &str| crate::doctree::ids::whitespace_normalize_name(s);

        // embedded link: unescaped `<...>` at the very end, preceded by
        // whitespace (or the whole content).
        let embedded = find_embedded_link(raw);
        let mut r = Node::elem(kinds::REFERENCE, self.span);
        match embedded {
            Some((text_part, link)) => {
                let is_alias = link.ends_with('_')
                    && !link.ends_with("\u{0}_")
                    && !link
                        .chars()
                        .rev()
                        .nth(1)
                        .map(|c| c == NULL)
                        .unwrap_or(false)
                    && !looks_like_uri(&link);
                let display_text;
                if is_alias {
                    let alias = ids(&unescape(&link[..link.len() - 1], false));
                    display_text = if text_part.is_empty() {
                        unescape(&link[..link.len() - 1], false)
                    } else {
                        unescape(&text_part, false)
                    };
                    r.set("name", AttrValue::Str(wsn(&display_text)));
                    r.set("refname", AttrValue::Str(alias.clone()));
                    r.children
                        .push(Node::text_node(display_text.clone(), self.span));
                    self.nodes.push(r);
                    if underscores == 1 {
                        let mut t = Node::elem(kinds::TARGET, self.span);
                        t.attrs.names.push(ids(&display_text));
                        let msg =
                            self.registry
                                .set_id_implicit(&mut t, self.lineno, self.source_path);
                        t.set("refname", AttrValue::Str(alias));
                        self.nodes.push(t);
                        if let Some(m) = msg {
                            self.messages.push(m);
                        }
                    }
                } else {
                    // URI: strip escaped trailing underscore marker, remove
                    // whitespace line-by-line (escaped spaces survive).
                    let mut uri_src = link.clone();
                    if uri_src.ends_with('_')
                        && uri_src
                            .chars()
                            .rev()
                            .nth(1)
                            .map(|c| c == NULL)
                            .unwrap_or(false)
                    {
                        // `alias\_` -> literal underscore URI
                        uri_src = format!("{}_", &uri_src[..uri_src.len() - 2]);
                    }
                    let uri = clean_uri(&uri_src);
                    let uri = if looks_like_email(&uri) && !uri.starts_with("mailto:") {
                        format!("mailto:{uri}")
                    } else {
                        uri
                    };
                    display_text = if text_part.is_empty() {
                        uri.clone()
                    } else {
                        unescape(&text_part, false)
                    };
                    r.set("name", AttrValue::Str(wsn(&display_text)));
                    r.set("refuri", AttrValue::Str(uri.clone()));
                    r.children
                        .push(Node::text_node(display_text.clone(), self.span));
                    self.nodes.push(r);
                    if underscores == 1 {
                        let mut t = Node::elem(kinds::TARGET, self.span);
                        t.attrs.names.push(ids(&display_text));
                        let msg =
                            self.registry
                                .set_id_implicit(&mut t, self.lineno, self.source_path);
                        t.set("refuri", AttrValue::Str(uri));
                        self.nodes.push(t);
                        if let Some(m) = msg {
                            self.messages.push(m);
                        }
                    }
                }
            }
            None => {
                let text = unescape(raw, false);
                r.set("name", AttrValue::Str(wsn(&text)));
                if underscores == 2 {
                    r.set("anonymous", AttrValue::Int(1));
                } else {
                    r.set("refname", AttrValue::Str(ids(&text)));
                }
                r.children.push(Node::text_node(text, self.span));
                self.nodes.push(r);
            }
        }
    }

    /// `[label]_` footnote and citation references.
    fn try_footnote_or_citation(&mut self, i: usize) -> Option<usize> {
        let n = self.chars.len();
        let mut j = i + 1;
        // label: '#simplename' | '#' | '*' | digits | simplename
        let label_start = j;
        match self.chars.get(j) {
            Some(&'#') => {
                j += 1;
                if let Some(len) = match_simplename(&self.chars, j) {
                    j += len;
                }
            }
            Some(&'*') => j += 1,
            _ => j += match_simplename(&self.chars, j)?,
        }
        if self.chars.get(j) != Some(&']') || self.chars.get(j + 1) != Some(&'_') {
            return None;
        }
        let after = j + 2;
        if !is_end_suffix_ok(self.chars.get(after).copied()) {
            return None;
        }
        let _ = n;
        let label: String = self.chars[label_start..j].iter().collect();
        self.flush_text();
        let is_citation =
            !label.starts_with('#') && label != "*" && !label.chars().all(|c| c.is_ascii_digit());
        let id = self.registry.allocate_auto_id();
        let mut node = if is_citation {
            let mut c = Node::elem(kinds::CITATION_REFERENCE, self.span);
            c.set(
                "refname",
                AttrValue::Str(crate::doctree::ids::fully_normalize_name(&label)),
            );
            c.children.push(Node::text_node(label.clone(), self.span));
            c
        } else {
            let mut f = Node::elem(kinds::FOOTNOTE_REFERENCE, self.span);
            if label == "*" {
                f.set("auto", AttrValue::Str("*".to_string()));
            } else if let Some(rest) = label.strip_prefix('#') {
                f.set("auto", AttrValue::Int(1));
                if !rest.is_empty() {
                    f.set(
                        "refname",
                        AttrValue::Str(crate::doctree::ids::fully_normalize_name(rest)),
                    );
                }
            } else {
                f.set(
                    "refname",
                    AttrValue::Str(crate::doctree::ids::fully_normalize_name(&label)),
                );
                f.children.push(Node::text_node(label.clone(), self.span));
            }
            f
        };
        node.attrs.ids.push(id);
        self.nodes.push(node);
        Some(after)
    }

    /// `|sub|`, `|sub|_`, `|sub|__` substitution references.
    fn substitution_ref(&mut self, i: usize) -> usize {
        let content_from = i + 1;
        let (end, underscores) = match self.find_end_with_underscores(content_from, '|', 2) {
            Some(r) => r,
            None => {
                self.emit_problematic("|", "substitution_reference");
                return i + 1;
            }
        };
        let after = end + 1 + underscores;
        let raw: String = self.chars[content_from..end].iter().collect();
        let text = unescape(&raw, false);
        self.flush_text();
        let mut subref = Node::elem(kinds::SUBSTITUTION_REFERENCE, self.span);
        subref.set(
            "refname",
            AttrValue::Str(crate::doctree::ids::whitespace_normalize_name(&text)),
        );
        subref
            .children
            .push(Node::text_node(text.clone(), self.span));
        if underscores == 0 {
            self.nodes.push(subref);
        } else {
            let mut outer = Node::elem(kinds::REFERENCE, self.span);
            if underscores == 2 {
                outer.set("anonymous", AttrValue::Int(1));
            } else {
                outer.set(
                    "refname",
                    AttrValue::Str(crate::doctree::ids::fully_normalize_name(&text)),
                );
            }
            outer.children.push(subref);
            self.nodes.push(outer);
        }
        after
    }
}

/// Remove whitespace per line from a URI (escaped whitespace survives as
/// literal after unescape).
fn clean_uri(link: &str) -> String {
    let joined: String = link
        .split('\n')
        .map(|part| {
            part.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        })
        .collect();
    unescape(&joined, false)
}

fn looks_like_uri(link: &str) -> bool {
    let mut chars = link.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return looks_like_email(link),
    }
    let scheme: String = link
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
        .collect();
    link[scheme.len()..].starts_with(':')
        && punctuation::URI_SCHEMES.contains(&scheme.to_lowercase().as_str())
        || looks_like_email(link)
}

fn looks_like_email(s: &str) -> bool {
    let s = s.trim_end_matches('_');
    let Some(at) = s.find('@') else { return false };
    let (local, domain) = (&s[..at], &s[at + 1..]);
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && local.chars().all(|c| is_emailc(c) || c == '.')
        && domain.chars().all(|c| is_emailc(c) || c == '.')
}

/// Embedded link inside a phrase: unescaped `<...>` at the very END,
/// preceded by unescaped whitespace (or being the whole content).
/// Returns (text_part_trimmed, link_content).
fn find_embedded_link(raw: &str) -> Option<(String, String)> {
    if !raw.ends_with('>') {
        return None;
    }
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    // find matching unescaped '<' scanning backward
    let mut open = None;
    for k in (0..n - 1).rev() {
        if chars[k] == '<' && (k == 0 || chars[k - 1] != NULL) {
            open = Some(k);
            break;
        }
    }
    let open = open?;
    if open > 0 {
        // must be preceded by whitespace
        let before = chars[open - 1];
        if !(before.is_whitespace() || (before == '\n')) {
            return None;
        }
    }
    let link: String = chars[open + 1..n - 1].iter().collect();
    if link.is_empty() {
        return None;
    }
    let text: String = chars[..open].iter().collect();
    Some((text.trim_end().to_string(), link))
}

/// Standalone URI or email starting at `at`. Returns (consumed_len,
/// refuri, display_text).
fn match_standalone_uri(chars: &[char], at: usize) -> Option<(usize, String, String)> {
    // absolute URI: scheme ':' hierarchical [?query] [#fragment]
    if chars[at].is_ascii_alphabetic() {
        let mut j = at;
        while j < chars.len()
            && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '.' | '+' | '-'))
        {
            j += 1;
        }
        if j < chars.len() && chars[j] == ':' {
            let scheme: String = chars[at..j].iter().collect();
            if punctuation::URI_SCHEMES.contains(&scheme.to_lowercase().as_str()) {
                let mut k = j + 1;
                let seg_end = |k: usize, stop: &[char]| -> usize {
                    let mut e = k;
                    while e < chars.len() && is_uric(chars[e]) && !stop.contains(&chars[e]) {
                        e += 1;
                    }
                    // segment must end on a urilast char: trim back
                    let mut last = e;
                    while last > k && !is_urilast(chars[last - 1]) {
                        last -= 1;
                    }
                    last
                };
                let h_end = seg_end(k, &['?', '#']);
                if h_end == k {
                    return None;
                }
                k = h_end;
                if chars.get(k) == Some(&'?') {
                    let q_end = seg_end(k + 1, &['#']);
                    if q_end > k + 1 {
                        k = q_end;
                    }
                }
                if chars.get(k) == Some(&'#') {
                    let f_end = seg_end(k + 1, &[]);
                    if f_end > k + 1 {
                        k = f_end;
                    }
                }
                if !is_end_suffix_ok(chars.get(k).copied()) && chars.get(k) != Some(&'>') {
                    return None;
                }
                let uri: String = chars[at..k].iter().collect();
                return Some((k - at, uri.clone(), uri));
            }
        }
    }
    // bare email ('@' is a literal separator in the docutils pattern, not
    // an emailc member — consume it here, validate via looks_like_email)
    {
        let mut j = at;
        while j < chars.len() && (is_emailc(chars[j]) || chars[j] == '.' || chars[j] == '@') {
            j += 1;
        }
        // trim to urilast
        while j > at && !is_urilast(chars[j - 1]) {
            j -= 1;
        }
        let cand: String = chars[at..j].iter().collect();
        if cand.contains('@')
            && looks_like_email(&cand)
            && (is_end_suffix_ok(chars.get(j).copied()) || chars.get(j) == Some(&'>'))
        {
            let refuri = if cand.starts_with("mailto:") {
                cand.clone()
            } else {
                format!("mailto:{cand}")
            };
            return Some((j - at, refuri, cand));
        }
    }
    None
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
