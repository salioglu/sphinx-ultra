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
    /// Role occurrences (sphinx mode only) for the build pipeline.
    pub roles: Vec<super::RoleRecord>,
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
    /// Earliest position from which an end-string search is known to fail,
    /// per (end char, length) — validity is position-local, so one failed
    /// full scan means all later scans fail too (kills the O(n^2)
    /// unclosed-markup pathology).
    failed_end_search: std::collections::HashMap<(char, usize), usize>,
    span: Span,
    lineno: u32,
    source_path: &'a str,
    registry: &'a mut IdRegistry,
    /// Sphinx mode: unknown-in-docutils roles become pending_xref nodes
    /// (no messages) and every role occurrence is recorded.
    sphinx: bool,
    docname: &'a str,
    /// `env.ref_context['std:program']` — the `.. program::` in scope, which
    /// `OptionXRefRole.process_link` stamps on every `:option:` reference
    /// (`domains/std/__init__.py:351-364`).
    program: Option<&'a str>,
    roles: Vec<super::RoleRecord>,
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
    fn find_end(
        &mut self,
        from: usize,
        end_str: &[char],
        allow_null_before: bool,
    ) -> Option<usize> {
        let key = (end_str[0], end_str.len());
        if let Some(fail_from) = self.failed_end_search.get(&key) {
            if from >= *fail_from {
                return None;
            }
        }
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
        self.failed_end_search.insert(key, from);
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
        // Cheap pre-checks: without ':' no scheme URI can match; without
        // '@' no email can match (kills quadratic rescans over ordinary
        // hyphenated text).
        if !text.contains(':') && !text.contains('@') {
            let out = unescape(text, false);
            if !out.is_empty() {
                self.nodes.push(Node::text_node(out, self.span));
            }
            return;
        }
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

    /// Backtick constructs: phrase references (trailing `_`/`__`),
    /// role-prefixed/suffixed interpreted text, or the default role.
    fn backtick_construct(&mut self, i: usize) -> usize {
        // Detect a `:name:` role prefix ending exactly at `i`.
        let prefix_role = self.role_prefix_before(i);
        let content_from = i + 1;
        let (end, underscores) = match self.find_end_with_underscores(content_from, '`', 2) {
            Some(r) => r,
            None => {
                self.emit_problematic("`", "interpreted text or phrase reference");
                return i + 1;
            }
        };
        let raw: String = self.chars[content_from..end].iter().collect();
        if underscores > 0 && prefix_role.is_none() {
            self.phrase_reference(&raw, underscores);
            return end + 1 + underscores;
        }
        // Suffix role `text`:name: (only when no trailing underscores).
        let suffix_role = if underscores == 0 {
            self.role_suffix_after(end)
        } else {
            None
        };
        match (prefix_role, suffix_role) {
            (Some((pstart, pname)), Some((send, sname))) => {
                let _ = (pname, sname);
                // Both prefix and suffix: WARNING + problematic over the span.
                self.trim_pending(i - pstart);
                let rawsource: String = self.chars[pstart..send].iter().collect();
                self.role_problematic(
                    &rawsource,
                    None,
                    messages::WARNING,
                    "Multiple roles in interpreted text (both prefix and suffix present; only one allowed).",
                );
                send
            }
            (Some((pstart, name)), None) => {
                self.trim_pending(i - pstart);
                let rawsource: String = self.chars[pstart..end + 1].iter().collect();
                self.apply_role(&name, &raw, &rawsource);
                end + 1
            }
            (None, Some((send, name))) => {
                let rawsource: String = self.chars[i..send].iter().collect();
                self.apply_role(&name, &raw, &rawsource);
                send
            }
            (None, None) => {
                self.emit_inline(kinds::TITLE_REFERENCE, &raw, false);
                end + 1
            }
        }
    }

    /// `:name:` immediately before position `i` with a valid construct
    /// prefix before it. The name may be COLON-JOINED (`:py:func:` — ':'
    /// is a simplename separator), so scan back through the whole
    /// word/separator run to the OUTERMOST candidate colon (docutils'
    /// leftmost-match semantics). Returns (construct_start, role_name).
    fn role_prefix_before(&self, i: usize) -> Option<(usize, String)> {
        if i < 3 || self.chars[i - 1] != ':' {
            return None;
        }
        // Scan backward over word chars and simplename separators to find
        // the earliest possible opening colon.
        let mut j = i - 1;
        while j > 0 {
            let c = self.chars[j - 1];
            if is_word_char(c) || matches!(c, '-' | '.' | '+' | ':') {
                j -= 1;
            } else {
                break;
            }
        }
        // Try opening colons from the OUTERMOST inward.
        let mut k = j;
        while k + 1 < i - 1 {
            if self.chars[k] == ':' {
                let name: String = self.chars[k + 1..i - 1].iter().collect();
                let name_chars: Vec<char> = name.chars().collect();
                if !name_chars.is_empty()
                    && match_simplename(&name_chars, 0) == Some(name_chars.len())
                {
                    let prev = k.checked_sub(1).map(|p| self.chars[p]);
                    if is_start_prefix_ok(prev) {
                        return Some((k, name));
                    }
                }
            }
            k += 1;
        }
        None
    }

    /// `:name:` immediately after the closing backtick at `end`. Returns
    /// (construct_end_exclusive, role_name).
    fn role_suffix_after(&self, end: usize) -> Option<(usize, String)> {
        let mut j = end + 1;
        if self.chars.get(j) != Some(&':') {
            return None;
        }
        j += 1;
        let len = match_simplename(&self.chars, j)?;
        j += len;
        if self.chars.get(j) != Some(&':') {
            return None;
        }
        j += 1;
        if !is_end_suffix_ok(self.chars.get(j).copied()) {
            return None;
        }
        let name: String = self.chars[end + 2..j - 1].iter().collect();
        Some((j, name))
    }

    /// Remove the last `n` chars from pending (a role prefix that turned
    /// out to be part of the construct).
    fn trim_pending(&mut self, n: usize) {
        for _ in 0..n {
            self.pending.pop();
        }
    }

    /// problematic + message pair for role errors. `info` precedes the
    /// main message without ids/backrefs.
    fn role_problematic(&mut self, rawsource: &str, info: Option<String>, level: u8, text: &str) {
        self.flush_text();
        if let Some(info_text) = info {
            self.messages.push(messages::system_message(
                messages::INFO,
                &info_text,
                self.lineno,
                self.source_path,
            ));
        }
        let msg_id = self.registry.allocate_auto_id();
        let prob_id = self.registry.allocate_auto_id();
        let mut prob = Node::elem(kinds::PROBLEMATIC, self.span);
        prob.attrs.ids.push(prob_id.clone());
        prob.set("refid", AttrValue::Str(msg_id.clone()));
        prob.children
            .push(Node::text_node(unescape(rawsource, true), self.span));
        self.nodes.push(prob);
        let mut msg = messages::system_message(level, text, self.lineno, self.source_path);
        msg.attrs.ids.push(msg_id);
        msg.attrs.backrefs.push(prob_id);
        self.messages.push(msg);
    }

    /// sphinx PEP/RFC/CVE/CWE roles (sphinx/roles.py): an index entry +
    /// anonymous index-N target + external reference wrapping a strong.
    fn emit_sphinx_extlink(&mut self, kind: &str, raw: &str) {
        let text = unescape(raw, false);
        let (target, title, explicit) = match (text.rfind('<'), text.ends_with('>')) {
            (Some(lt), true) => (
                text[lt + 1..text.len() - 1].trim().to_string(),
                text[..lt].trim_end().to_string(),
                true,
            ),
            _ => (text.clone(), text.clone(), false),
        };
        let (numpart, anchor) = match target.split_once('#') {
            Some((a, b)) => (a.to_string(), Some(b.to_string())),
            None => (target.clone(), None),
        };
        let (kind_key, index_text, refuri, default_title) = match kind {
            "pep" => {
                let Ok(num) = numpart.parse::<u32>() else {
                    self.role_problematic(
                        raw,
                        None,
                        messages::ERROR,
                        &format!("invalid PEP number {target}"),
                    );
                    return;
                };
                let mut uri = format!("https://peps.python.org/pep-{num:04}/");
                if let Some(a) = &anchor {
                    uri.push('#');
                    uri.push_str(a);
                }
                (
                    "pep",
                    format!("Python Enhancement Proposals; PEP {target}"),
                    uri,
                    format!("PEP {title}"),
                )
            }
            "rfc" => {
                let Ok(num) = numpart.parse::<u32>() else {
                    self.role_problematic(
                        raw,
                        None,
                        messages::ERROR,
                        &format!("invalid RFC number {target}"),
                    );
                    return;
                };
                let formatted = match &anchor {
                    Some(a) => {
                        let mut f = None;
                        for prefix in ["appendix", "page", "section"] {
                            let cap = {
                                let mut c = prefix.chars();
                                let f0 = c.next().expect("nonempty").to_uppercase();
                                format!("{f0}{}", c.as_str())
                            };
                            if a == prefix {
                                f = Some(format!("RFC {numpart} {cap}"));
                                break;
                            }
                            if let Some(rest) = a.strip_prefix(&format!("{prefix}-")) {
                                f = Some(format!("RFC {numpart} {cap} {rest}"));
                                break;
                            }
                        }
                        f.unwrap_or_else(|| format!("RFC {target}"))
                    }
                    None => format!("RFC {numpart}"),
                };
                let mut uri = format!("https://datatracker.ietf.org/doc/html/rfc{num}.html");
                if let Some(a) = &anchor {
                    uri.push('#');
                    uri.push_str(a);
                }
                (
                    "rfc",
                    format!("RFC; {formatted}"),
                    uri,
                    if explicit { title.clone() } else { formatted },
                )
            }
            "cve" => {
                let mut uri = format!("https://www.cve.org/CVERecord?id=CVE-{numpart}");
                if let Some(a) = &anchor {
                    uri.push('#');
                    uri.push_str(a);
                }
                (
                    "cve",
                    format!("Common Vulnerabilities and Exposures; CVE {target}"),
                    uri,
                    format!("CVE {title}"),
                )
            }
            _ => {
                let Ok(num) = numpart.parse::<u32>() else {
                    self.role_problematic(
                        raw,
                        None,
                        messages::ERROR,
                        &format!("invalid CWE number {target}"),
                    );
                    return;
                };
                let mut uri = format!("https://cwe.mitre.org/data/definitions/{num}.html");
                if let Some(a) = &anchor {
                    uri.push('#');
                    uri.push_str(a);
                }
                (
                    "cwe",
                    format!("Common Weakness Enumeration; CWE {target}"),
                    uri,
                    format!("CWE {title}"),
                )
            }
        };
        let serial = self.registry.new_index_serialno();
        let target_id = format!("index-{serial}");
        self.flush_text();
        let mut index = Node::elem("index", self.span);
        index.set(
            "entries",
            AttrValue::List(vec![super::block::index_entry_tuple(
                "single",
                &index_text,
                &target_id,
                "",
                None,
            )]),
        );
        self.nodes.push(index);
        let mut tnode = Node::elem(kinds::TARGET, self.span);
        tnode.attrs.ids.push(target_id.clone());
        // `self.inliner.document.note_explicit_target(target)` (`roles.py`):
        // the id joins `document.ids`, so a later `make_id` cannot reuse it.
        self.registry.note_explicit_id(&target_id);
        self.nodes.push(tnode);
        let mut reference = Node::elem(kinds::REFERENCE, self.span);
        reference.attrs.classes.push(kind_key.to_string());
        reference.set("internal", AttrValue::Int(0));
        reference.set("refuri", AttrValue::Str(refuri));
        let mut strong = Node::elem(kinds::STRONG, self.span);
        strong.children.push(Node::text_node(
            if explicit { title } else { default_title },
            self.span,
        ));
        reference.children.push(strong);
        self.nodes.push(reference);
    }

    /// sphinx.roles.XRefRole anatomy (probe-verified via :doc: in the
    /// wave-3 probes): pending_xref with refdoc/refdomain/refexplicit/
    /// reftarget/reftype/refwarn attrs and an `inline classes="xref
    /// {domain} {domain}-{type}"` child. Role→domain mapping covers the
    /// std and py domains; explicit `domain:role` names pass through.
    fn emit_sphinx_xref(&mut self, lower: &str, raw: &str) {
        let (domain, reftype) = match lower.rsplit_once(':') {
            Some((d, t)) => (d.to_string(), t.to_string()),
            None => {
                let d = match lower {
                    "doc" | "ref" | "term" | "option" | "envvar" | "numref" | "keyword"
                    | "token" | "program" | "confval" => "std",
                    "func" | "class" | "meth" | "mod" | "attr" | "data" | "exc" | "obj"
                    | "const" | "deco" => "py",
                    _ => "std",
                };
                (d.to_string(), lower.to_string())
            }
        };
        self.emit_xref_node(&domain, &reftype, raw, None);
    }

    /// The `:external:`/`:external+inv:` roles, which
    /// `IntersphinxDispatcher` claims *before* docutils' own role lookup
    /// (`ext/intersphinx/_resolve.py:350-366`) — and therefore before the
    /// generic `domain:role` split above, which would otherwise read
    /// `external:py:func` as domain `external:py`.
    ///
    /// The name is the one the author wrote, not the lowercased one: the
    /// inventory name inside it is case-sensitive
    /// (`IntersphinxRole.orig_name`).
    ///
    /// A name that does not name a usable role produces no content at all in
    /// Sphinx, and a warning located at the role. Here the warning text is
    /// parked on a marker node instead: the parse layer knows the role
    /// grammar but not which inventories loaded, and Sphinx checks the
    /// inventory *first* — so deferring the whole report to resolution is
    /// what keeps the precedence between the two right.
    fn emit_external_xref(&mut self, given_name: &str, raw: &str) {
        match crate::intersphinx::external_role(given_name) {
            crate::intersphinx::ExternalRole::Xref {
                inventory,
                domain,
                role,
            } => self.emit_xref_node(&domain, &role, raw, Some(inventory)),
            crate::intersphinx::ExternalRole::Failed(diagnostic) => {
                self.flush_text();
                let mut node = Node::elem("pending_xref", self.span);
                node.set("refdoc", AttrValue::Str(self.docname.to_string()));
                node.set("intersphinx", AttrValue::Int(1));
                node.set("intersphinx_role_error", AttrValue::Str(diagnostic.message));
                self.nodes.push(node);
            }
        }
    }

    /// The body of [`Self::emit_sphinx_xref`], with the domain and role
    /// already decided. `external` is `Some(inventory)` for a node the
    /// `:external:` role produced — `Some(None)` when that role named no
    /// inventory.
    fn emit_xref_node(
        &mut self,
        domain: &str,
        reftype: &str,
        raw: &str,
        external: Option<Option<String>>,
    ) {
        let text = unescape(raw, false);
        let (domain, reftype) = (domain.to_string(), reftype.to_string());
        // `Title <target>` explicit form.
        let (target, display, explicit) = match (text.rfind('<'), text.ends_with('>')) {
            (Some(lt), true) => (
                text[lt + 1..text.len() - 1].trim().to_string(),
                text[..lt].trim_end().to_string(),
                true,
            ),
            _ => (text.clone(), text.clone(), false),
        };
        // `XRefRole.lowercase` (`roles.py:122-124`): the target — never the
        // title — is lowercased, for `:ref:` *and* `:numref:`
        // (`domains/std/__init__.py:752-760`, both `lowercase=True`).
        // `XRefRole.process_link` then collapses whitespace runs in the
        // target (`roles.py:165`, `ws_re.sub(' ', target)`), which is what
        // `fully_normalize_name` does on top of lowercasing — bar the
        // leading/trailing strip, and docutils cannot produce an
        // interpreted-text target with either (a space after the opening
        // backtick is "start-string without end-string", verified against
        // docutils 0.22.4). py targets drop a leading `~` from the target
        // while the title keeps only the last dotted segment.
        let (target, display) = match (domain.as_str(), reftype.as_str()) {
            ("std", "ref" | "numref") => {
                (crate::doctree::ids::fully_normalize_name(&target), display)
            }
            ("py", _) if target.starts_with('~') && !explicit => {
                let full = target[1..].to_string();
                let short = full.rsplit('.').next().unwrap_or(&full).to_string();
                (full, short)
            }
            _ => (target, display),
        };
        let mut node = Node::elem("pending_xref", self.span);
        let py = domain == "py";
        if py {
            // Context attrs (current class/module) are None outside a py
            // scope; pformat renders None as "True".
            node.set("py:class", AttrValue::Str("True".to_string()));
            node.set("py:module", AttrValue::Str("True".to_string()));
        }
        node.set("refdoc", AttrValue::Str(self.docname.to_string()));
        node.set("refdomain", AttrValue::Str(domain.clone()));
        // `node['intersphinx'] = True; node['inventory'] = inv_name`
        // (`_resolve.py:474-483`) — the stamp that sends this node through
        // `IntersphinxRoleResolver` instead of ordinary domain resolution.
        if let Some(inventory) = external {
            node.set("intersphinx", AttrValue::Int(1));
            if let Some(inventory) = inventory {
                node.set("inventory", AttrValue::Str(inventory));
            }
        }
        node.set("refexplicit", AttrValue::Int(i64::from(explicit)));
        node.set("reftarget", AttrValue::Str(target));
        node.set("reftype", AttrValue::Str(reftype.clone()));
        // `XRefRole.warn_dangling` (`roles.py:134`), which is what makes a
        // role report a dangling reference outside nitpicky mode. Only these
        // seven std roles carry it (`domains/std/__init__.py:748-766`); no py
        // role does.
        //
        // The list is exhaustive on purpose. Everything else that lands here
        // is a role this crate has no implementation for — including
        // Sphinx's own non-xref roles (`:kbd:`, `:file:`, `:guilabel:`,
        // `:command:`, `:abbr:`, `:program:`, ..., `roles.py:28-36` and
        // `:608-626`), which produce plain inline nodes in real Sphinx and
        // resolve nothing. Defaulting those to `warn_dangling` made every
        // document that used one warn `'kbd' reference target not found`.
        let warn_dangling = matches!(
            (domain.as_str(), reftype.as_str()),
            (
                "std",
                "ref" | "numref" | "doc" | "term" | "keyword" | "option" | "confval"
            )
        );
        node.set("refwarn", AttrValue::Int(i64::from(warn_dangling)));
        // `OptionXRefRole.process_link` (`domains/std/__init__.py:351-364`).
        // Outside a `.. program::` scope the value is Python None, which
        // pformat renders as the "True" sentinel.
        if domain == "std" && reftype == "option" {
            node.set(
                "std:program",
                AttrValue::Str(self.program.unwrap_or("True").to_string()),
            );
        }
        // py xrefs wrap in a literal (code-styled); callables display
        // with parens.
        let mut display = display;
        if py && matches!(reftype.as_str(), "func" | "meth") && !explicit {
            display.push_str("()");
        }
        // `XRefRole.innernodeclass` (`roles.py:67`): `literal` unless the
        // role overrides it, which in the std domain only `ref`, `term` and
        // `doc` do (`domains/std/__init__.py:752-765`).
        let inline_inner = matches!(
            (domain.as_str(), reftype.as_str()),
            ("std", "ref" | "term" | "doc")
        );
        let mut inner = Node::elem(
            if inline_inner {
                "inline"
            } else {
                kinds::LITERAL
            },
            self.span,
        );
        inner.attrs.classes = vec![
            "xref".to_string(),
            domain.clone(),
            format!("{domain}-{reftype}"),
        ];
        inner.children.push(Node::text_node(display, self.span));
        node.children.push(inner);
        self.flush_text();
        // `EnvVarXRefRole.result_nodes` (`domains/std/__init__.py:91-112`):
        // an `:envvar:` reference also *indexes* the variable, under its bare
        // name and under the same 'environment variable; %s' heading the
        // `.. envvar::` directive uses, both anchored on a fresh
        // `index-N` target placed just before the reference.
        // `EnvVarXRefRole.result_nodes` is gated on `is_ref`
        // (`domains/std/__init__.py:99-102`), which `XRefRole.run` clears for
        // a `!`-prefixed role text: `ReferenceRole.run` sets
        // `self.disabled = text.startswith('!')` and `create_non_xref_node`
        // then calls `result_nodes(..., is_ref=False)`, which returns the bare
        // node — no index entries, no target, and no `index` serial consumed.
        // (The rest of `disabled` — emitting the literal alone, without a
        // pending_xref, and with the `!` stripped — is not modelled yet; this
        // guard keeps the un-modelled half from also corrupting document-wide
        // `index-N` numbering.)
        if domain == "std" && reftype == "envvar" && !text.starts_with('!') {
            let varname = match node.get("reftarget") {
                Some(AttrValue::Str(target)) => target.clone(),
                _ => String::new(),
            };
            let target_id = format!("index-{}", self.registry.new_index_serialno());
            let mut index = Node::elem("index", self.span);
            index.set(
                "entries",
                AttrValue::List(vec![
                    super::block::index_entry_tuple("single", &varname, &target_id, "", None),
                    super::block::index_entry_tuple(
                        "single",
                        &format!("environment variable; {varname}"),
                        &target_id,
                        "",
                        None,
                    ),
                ]),
            );
            self.nodes.push(index);
            let mut tnode = Node::elem(kinds::TARGET, self.span);
            tnode.attrs.ids.push(target_id.clone());
            self.registry.note_explicit_id(&target_id);
            self.nodes.push(tnode);
        }
        self.nodes.push(node);
    }

    /// Apply a built-in role by (raw, unlowercased) name.
    fn apply_role(&mut self, given_name: &str, raw: &str, rawsource: &str) {
        let lower = given_name.to_lowercase();
        if self.sphinx {
            // Record EVERY role occurrence for the build pipeline
            // (validation + nitpicky), M1-scanner display-split semantics.
            let text = unescape(raw, false);
            let (target, display) = match (text.rfind('<'), text.ends_with('>')) {
                (Some(lt), true) => (
                    text[lt + 1..text.len() - 1].trim().to_string(),
                    Some(text[..lt].trim().to_string()),
                ),
                _ => (text.clone(), None),
            };
            let last_segment = lower.rsplit(':').next().unwrap_or(&lower).to_string();
            self.roles.push(super::RoleRecord {
                name: last_segment,
                full_name: given_name.to_string(),
                target,
                display,
                line: self.lineno,
            });
        }
        // en language aliases -> canonical names
        let canonical = match lower.as_str() {
            "abbreviation" | "ab" => "abbreviation",
            "acronym" | "ac" => "acronym",
            "code" => "code",
            "emphasis" => "emphasis",
            "literal" => "literal",
            "math" => "math",
            "pep-reference" | "pep" => "pep-reference",
            "rfc-reference" | "rfc" => "rfc-reference",
            "strong" => "strong",
            "subscript" | "sub" => "subscript",
            "superscript" | "sup" => "superscript",
            "title-reference" | "title" | "t" => "title-reference",
            "raw" => "raw",
            "index" | "i" => "index",
            "named-reference" => "named-reference",
            "anonymous-reference" => "anonymous-reference",
            "footnote-reference" => "footnote-reference",
            "citation-reference" => "citation-reference",
            "substitution-reference" => "substitution-reference",
            "target" => "target",
            "uri-reference" | "uri" | "url" => "uri-reference",
            _ if self.sphinx && crate::intersphinx::is_external_role(given_name) => {
                self.emit_external_xref(given_name, raw);
                return;
            }
            _ if self.sphinx && matches!(lower.as_str(), "cve" | "cwe") => {
                self.emit_sphinx_extlink(&lower, raw);
                return;
            }
            _ if self.sphinx => {
                // Sphinx mode: non-docutils roles resolve through the
                // domain registries; at parse layer they become
                // pending_xref nodes and NEVER message. (Genuinely unknown
                // roles would error in real Sphinx — hardening note.)
                self.emit_sphinx_xref(&lower, raw);
                return;
            }
            _ => {
                // Not in the language module: INFO + canonical lookup, which
                // also fails for anything we do not know -> ERROR.
                let info = format!(
                    "No role entry for \"{given_name}\" in module \"docutils.parsers.rst.languages.en\".\nTrying \"{given_name}\" as canonical role name."
                );
                if lower == "restructuredtext-unimplemented-role" {
                    self.role_problematic(
                        rawsource,
                        Some(info),
                        messages::ERROR,
                        &format!("Interpreted text role \"{given_name}\" not implemented."),
                    );
                } else {
                    self.role_problematic(
                        rawsource,
                        Some(info),
                        messages::ERROR,
                        &format!("Unknown interpreted text role \"{given_name}\"."),
                    );
                }
                return;
            }
        };
        match canonical {
            "emphasis" => self.emit_inline(kinds::EMPHASIS, raw, false),
            "strong" => self.emit_inline(kinds::STRONG, raw, false),
            "literal" => self.emit_inline(kinds::LITERAL, raw, false),
            "subscript" => self.emit_inline(kinds::SUBSCRIPT, raw, false),
            "superscript" => self.emit_inline(kinds::SUPERSCRIPT, raw, false),
            "title-reference" => self.emit_inline(kinds::TITLE_REFERENCE, raw, false),
            "abbreviation" => self.emit_inline(kinds::ABBREVIATION, raw, false),
            "acronym" => self.emit_inline(kinds::ACRONYM, raw, false),
            "math" => self.emit_inline(kinds::MATH, raw, true),
            "code" => {
                self.flush_text();
                let mut node = Node::elem(kinds::LITERAL, self.span);
                node.attrs.classes.push("code".to_string());
                node.children
                    .push(Node::text_node(unescape(raw, true), self.span));
                self.nodes.push(node);
            }
            "pep-reference" => {
                if self.sphinx {
                    // Sphinx overrides pep with an index-emitting external.
                    self.emit_sphinx_extlink("pep", raw);
                    return;
                }
                let text = unescape(raw, false);
                match text.parse::<u32>().ok().filter(|n| *n <= 9999) {
                    Some(n) => {
                        self.flush_text();
                        let mut r = Node::elem(kinds::REFERENCE, self.span);
                        r.set(
                            "refuri",
                            AttrValue::Str(format!("https://peps.python.org/pep-{n:04}")),
                        );
                        r.children
                            .push(Node::text_node(format!("PEP {text}"), self.span));
                        self.nodes.push(r);
                    }
                    None => self.role_problematic(
                        rawsource,
                        None,
                        messages::ERROR,
                        &format!(
                            "PEP number must be a number from 0 to 9999; \"{text}\" is invalid."
                        ),
                    ),
                }
            }
            "rfc-reference" => {
                if self.sphinx {
                    self.emit_sphinx_extlink("rfc", raw);
                    return;
                }
                let text = unescape(raw, false);
                let (numpart, fragment) = match text.split_once('#') {
                    Some((a, b)) => (a.to_string(), Some(b.to_string())),
                    None => (text.clone(), None),
                };
                match numpart.parse::<u32>().ok().filter(|n| *n >= 1) {
                    Some(n) => {
                        self.flush_text();
                        let mut r = Node::elem(kinds::REFERENCE, self.span);
                        let mut uri = format!("https://tools.ietf.org/html/rfc{n}.html");
                        if let Some(f) = fragment {
                            uri = format!("{uri}#{f}");
                        }
                        r.set("refuri", AttrValue::Str(uri));
                        r.children
                            .push(Node::text_node(format!("RFC {n}"), self.span));
                        self.nodes.push(r);
                    }
                    None => self.role_problematic(
                        rawsource,
                        None,
                        messages::ERROR,
                        &format!(
                            "RFC number must be a number greater than or equal to 1; \"{numpart}\" is invalid."
                        ),
                    ),
                }
            }
            "raw" => self.role_problematic(
                rawsource,
                None,
                messages::ERROR,
                "No format (Writer name) is associated with this role: \"raw\".\nThe \"raw\" role cannot be used directly.\nInstead, use the \"role\" directive to create a new role with an associated format.",
            ),
            other => self.role_problematic(
                rawsource,
                None,
                messages::ERROR,
                &format!("Interpreted text role \"{other}\" not implemented."),
            ),
        }
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
    // The closing '>' must be unescaped.
    if n >= 2 && chars[n - 2] == NULL {
        return None;
    }
    // find matching unescaped '<' scanning backward
    let mut open = None;
    for k in (0..n - 1).rev() {
        if chars[k] == '<' && (k == 0 || chars[k - 1] != NULL) {
            open = Some(k);
            break;
        }
    }
    let open = open?;
    // docutils: link content is ([^<>]|\x00[<>])+ — any unescaped '<' or
    // '>' inside invalidates the whole embedded link.
    let mut m = open + 1;
    while m < n - 1 {
        let c = chars[m];
        if c == NULL {
            m += 2;
            continue;
        }
        if c == '<' || c == '>' {
            return None;
        }
        m += 1;
    }
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
    parse_inline_ext(
        text,
        span,
        lineno,
        registry,
        source_path,
        false,
        "index",
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn parse_inline_ext(
    text: &str,
    span: Span,
    lineno: u32,
    registry: &mut IdRegistry,
    source_path: &str,
    sphinx: bool,
    docname: &str,
    program: Option<&str>,
) -> InlineResult {
    let escaped = escape2null(text);
    let mut inliner = Inliner {
        chars: escaped.chars().collect(),
        span,
        lineno,
        source_path,
        registry,
        sphinx,
        docname,
        program,
        roles: Vec::new(),
        nodes: Vec::new(),
        messages: Vec::new(),
        pending: String::new(),
        failed_end_search: std::collections::HashMap::new(),
    };
    inliner.run();
    InlineResult {
        nodes: inliner.nodes,
        messages: inliner.messages,
        roles: inliner.roles,
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

    /// Sphinx-mode inline parse, for the roles that only exist there.
    fn sphinx_nodes(text: &str) -> Vec<Node> {
        let mut reg = IdRegistry::new();
        parse_inline_ext(
            text,
            Span::ZERO,
            1,
            &mut reg,
            "<snippet>",
            true,
            "index",
            None,
        )
        .nodes
    }

    fn attr<'a>(node: &'a Node, key: &'static str) -> Option<&'a AttrValue> {
        node.get(key)
    }

    /// `:external:py:func:` must not be split by the generic
    /// `rsplit_once(':')` domain rule, which would read it as domain
    /// `external:py` and role `func`.
    #[test]
    fn an_external_role_is_intercepted_before_the_generic_domain_split() {
        let nodes = sphinx_nodes(":external:py:func:`os.path.join`");
        assert_eq!(nodes.len(), 1);
        let xref = &nodes[0];
        assert_eq!(xref.kind, "pending_xref");
        assert_eq!(attr(xref, "refdomain"), Some(&AttrValue::Str("py".into())));
        assert_eq!(attr(xref, "reftype"), Some(&AttrValue::Str("func".into())));
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str("os.path.join".into()))
        );
        assert_eq!(
            attr(xref, "intersphinx"),
            Some(&AttrValue::Int(1)),
            "the node must be stamped so the resolver treats it as external"
        );
        assert_eq!(
            attr(xref, "inventory"),
            None,
            "`external:` names no inventory"
        );
    }

    #[test]
    fn an_external_role_can_name_its_inventory_and_omit_the_domain() {
        let nodes = sphinx_nodes(":external+other:ref:`some-label`");
        let xref = &nodes[0];
        assert_eq!(xref.kind, "pending_xref");
        assert_eq!(attr(xref, "refdomain"), Some(&AttrValue::Str("std".into())));
        assert_eq!(attr(xref, "reftype"), Some(&AttrValue::Str("ref".into())));
        assert_eq!(
            attr(xref, "inventory"),
            Some(&AttrValue::Str("other".into())),
            "the inventory name keeps its case"
        );
    }

    /// Sphinx's role emits no nodes at all for a name it cannot use; the
    /// warning is carried on a marker the resolver reports and drops, since
    /// only the resolver knows where the document was and what loaded.
    #[test]
    fn a_malformed_external_role_carries_its_warning_to_resolution() {
        for (text, expected) in [
            (
                ":external:a:b:c:`x`",
                "invalid external cross-reference suffix: 'a:b:c'",
            ),
            (
                ":external:py:function:`x`",
                "role for external cross-reference not found in domain 'py': 'function' \
                 (perhaps you meant one of: 'func', 'obj')",
            ),
        ] {
            let nodes = sphinx_nodes(text);
            assert_eq!(nodes.len(), 1, "{text}");
            assert_eq!(
                attr(&nodes[0], "intersphinx_role_error"),
                Some(&AttrValue::Str(expected.to_string())),
                "{text}"
            );
            assert!(
                nodes[0].children.is_empty(),
                "a failed role contributes no content: {text}"
            );
        }
    }

    /// The interception is by name, so an ordinary role whose name merely
    /// starts with the same letters is untouched.
    #[test]
    fn a_role_named_like_external_is_still_an_ordinary_xref() {
        let nodes = sphinx_nodes(":externally:`x`");
        assert_eq!(
            attr(&nodes[0], "reftype"),
            Some(&AttrValue::Str("externally".into()))
        );
        assert_eq!(attr(&nodes[0], "intersphinx"), None);
    }
}
