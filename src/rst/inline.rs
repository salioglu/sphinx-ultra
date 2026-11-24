//! The inline parser (M2 wave 2): a port of docutils `states.py Inliner`
//! for emphasis/strong/literal recognition, escape semantics, and the
//! problematic/unclosed machinery. References, roles, and footnote
//! references extend this module in later wave-2 tasks.
//!
//! Escape model (probe-verified): backslash escapes convert to a `\x00`
//! marker + char before recognition (`escape2null`); at Text emission the
//! markers strip (`\x00 ` and `\x00\n` remove BOTH chars, joining words;
//! bare `\x00` drops, keeping the escaped char). Inline literals restore
//! backslashes instead. Where docutils builds its Text from the
//! null-escaped text, the node keeps that data in its side-car
//! ([`Node::text_node_escaped`]; SmartQuotes reads it), while its `text` is
//! the stripped form. Behavior sources: the wave-2 probe notes
//! (2026-08-07-m2-wave2-probes.md) and the differential fixture — never
//! memory.

use crate::doctree::{kinds, messages, AttrValue, Node, Span};
use crate::transforms::state::{DocLoc, DocumentState};

use std::sync::Arc;

use super::punctuation;
use super::roles::sphinx::{XrefEnv, XrefRole};
use super::roles::{
    self, DispatchCtx, Resolution, RoleCall, RoleCrash, RoleFn, RoleOptions, RoleTable,
};
use crate::rst::LogLocation;

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

/// docutils `nodes.unescape`, which lives with the doctree IR as it does
/// in docutils (`nodes.py`): [`Node::text_node_escaped`] unescapes too.
pub use crate::doctree::unescape;

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
            // `\s` in a Python `str` pattern is `str.isspace`, which admits
            // the C0 separators `\x1c`-`\x1f` (see `crate::utils::py_isspace`).
            crate::utils::py_isspace(c)
                || punctuation::OPENERS.contains(&c)
                || punctuation::DELIMITERS.contains(&c)
        }
    }
}

fn is_end_suffix_ok(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            crate::utils::py_isspace(c)
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

pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// docutils `simplename`: `(?:(?!_)\w)+(?:[-._+:](?:(?!_)\w)+)*` — word-char
/// atoms (underscore excluded) joined by single `-._+:` separators. Returns
/// the char length of the match starting at `at`, or None.
pub(crate) fn match_simplename(chars: &[char], at: usize) -> Option<usize> {
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

/// The slice of sphinx's `env.ref_context` the inline roles read: the
/// values `OptionXRefRole`/`PyXRefRole` stamp on their pending_xrefs, plus
/// the key-*existence* flags `AnyXRefRole.process_link`'s blanket
/// `refnode.attributes.update(env.ref_context)` (`sphinx/roles.py`) needs —
/// a ref_context key can exist holding Python `None` (pformat's `"True"`
/// sentinel) or a list that the balanced directive nesting has drained back
/// to `[]` (pformat `""`) by the time the doctree is serialized, so a bare
/// `Option<&str>` cannot distinguish "absent" from those.
#[derive(Clone, Copy, Default)]
pub struct RefContext<'a> {
    /// `env.ref_context['std:program']` — the `.. program::` in scope, which
    /// `OptionXRefRole.process_link` stamps on every `:option:` reference
    /// (`domains/std/__init__.py:351-364`).
    pub program: Option<&'a str>,
    /// `env.ref_context['py:module']` / `['py:class']` — the enclosing
    /// module/class scope `PyXRefRole.process_link` stamps on every py
    /// pending_xref (`domains/python/__init__.py:568-569`).
    ///
    /// Like [`Self::py_class`], this `None` cannot by itself distinguish
    /// "the key is absent" from "the key exists holding `None`" — which is
    /// why [`Self::py_module_key`] exists. The distinction is reachable:
    /// `before_content` pushes `ref_context.get('py:module')`, which is
    /// `None` when a `:module:`-carrying directive has no enclosing module
    /// scope, and `after_content` then ASSIGNS that `None` back
    /// (`if modules:` is true for a one-element list holding `None`,
    /// `_object.py:498-503`) instead of popping the key. Research spec §8
    /// trap 14 says the same. The `:any:` role's blanket `ref_context`
    /// copy is where it shows, as the `"True"` sentinel.
    pub py_module: Option<&'a str>,
    pub py_class: Option<&'a str>,
    /// `'py:class' in env.ref_context`: true once any py object directive
    /// set a class prefix (`before_content`) or completed (`after_content`
    /// always assigns the key, `_object.py:482-503`) — the value may be
    /// `None` (`py_class` absent), which the `:any:` role still stamps.
    pub py_class_key: bool,
    /// `'py:module' in env.ref_context`: true while a module scope is set
    /// AND after a `:module:`-carrying directive has closed, where the key
    /// survives holding `None`. Cleared by `.. py:currentmodule:: None`
    /// (`ref_context.pop`).
    pub py_module_key: bool,
    /// `'py:classes' in env.ref_context`: created by `before_content` for
    /// nesting kinds and unconditionally by `after_content`'s `setdefault`.
    /// The *list* is what `AnyXRefRole` copies onto the node, and balanced
    /// nesting drains it to `[]` by end of parse — so it renders `""`.
    pub py_classes_key: bool,
    /// `'py:modules' in env.ref_context`: created when a `:module:` option
    /// pushed (`before_content`/`after_content`, `_object.py:477-480`).
    /// Same aliased-list story as `py_classes_key`: always drained.
    pub py_modules_key: bool,
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
    /// The document state: ids, names and serials are document-level, and
    /// its reporter builds and logs the messages, located at `lineno` of
    /// the span's source.
    state: &'a mut DocumentState,
    /// The role registries the parse looks roles up in: the document's
    /// ([`super::ParseOnlyState::roles`]), as it was when this text block
    /// started.
    table: Arc<RoleTable>,
    /// The configuration the Sphinx roles read (`manpages_url`).
    read: &'a super::ReadConfig,
    /// The mode and document state the lookup depends on; in Sphinx mode
    /// every role occurrence is also recorded.
    dispatch: DispatchCtx,
    docname: &'a str,
    /// The enclosing document's ref_context state (see [`RefContext`]).
    ctx: RefContext<'a>,
    /// The py-domain configuration the roles read; today only
    /// `add_function_parentheses`, in [`Self::emit_xref_node`].
    py: &'a crate::py::PySigConfig,
    roles: Vec<super::RoleRecord>,
    nodes: Vec<Node>,
    messages: Vec<Node>,
    /// Text accumulated since the last emitted inline node.
    pending: String,
}

impl<'a> Inliner<'a> {
    /// Where the inliner's messages are located: `lineno` of the text
    /// block's source.
    fn loc(&self) -> DocLoc {
        DocLoc::at(self.span.source, self.lineno)
    }

    /// `self.reporter.<level>(text, line=lineno)`: reported through the
    /// document state, which logs it ([`DocumentState::report`]).
    fn report(&mut self, level: u8, text: &str) -> Node {
        let loc = self.loc();
        self.state.report(level, text, loc, Vec::new())
    }

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
            Some(c) if crate::utils::py_isspace(*c) => false,
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
                    !crate::utils::py_isspace(prev)
                } else {
                    !crate::utils::py_isspace(prev) && prev != NULL
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

    /// A plain-text run's `nodes.Text(text)` (`states.py:1160`), built from
    /// the null-escaped run so it keeps docutils' data. `implicit_inline`
    /// makes no Text of an empty run only: a run of escaped whitespace
    /// (`\x00 `, `\x00\n`) unescapes to nothing, which pformat and the
    /// writers print as nothing, but its Text stays, and SmartQuotes reads
    /// it as whitespace before the quote after it.
    fn push_plain_text(&mut self, data: &str) {
        if !data.is_empty() {
            self.nodes.push(Node::text_node_escaped(data, self.span));
        }
    }

    /// docutils `implicit_inline`: standalone URIs and emails inside plain
    /// text runs become reference nodes.
    fn implicit_inline(&mut self, text: &str) {
        // Cheap pre-checks: without ':' no scheme URI can match; without
        // '@' no email can match (kills quadratic rescans over ordinary
        // hyphenated text).
        if !text.contains(':') && !text.contains('@') {
            self.push_plain_text(text);
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
                    self.push_plain_text(&before);
                    // `nodes.reference(unescape(text, True), text, ...)`
                    // (`states.py:1110-1111`): the Text keeps the markers,
                    // the rawsource has the backslashes back.
                    let mut r = Node::elem(kinds::REFERENCE, self.span);
                    r.set_rawsource(&unescape(&display, true));
                    r.set("refuri", AttrValue::Str(unescape(&refuri, false)));
                    r.children
                        .push(Node::text_node_escaped(&display, self.span));
                    self.nodes.push(r);
                    i += len;
                    emitted_upto = i;
                    continue;
                }
            }
            i += 1;
        }
        let rest: String = chars[emitted_upto..].iter().collect();
        self.push_plain_text(&rest);
    }

    /// `inline_obj` (`states.py:821-844`) and docutils' generic roles: the
    /// Text keeps the null-escaped data, except where `restore` puts the
    /// backslashes back (inline literals, `:math:`).
    fn emit_inline(&mut self, kind: &'static str, content: &str, restore: bool) {
        self.flush_text();
        let mut node = Node::elem(kind, self.span);
        let text = if restore {
            Node::text_node(unescape(content, true), self.span)
        } else {
            Node::text_node_escaped(content, self.span)
        };
        node.children.push(text);
        self.nodes.push(node);
    }

    /// Unclosed start-string: `problematic` holding only the start-string
    /// + a WARNING message; the rest of the text re-scans.
    fn emit_problematic(&mut self, start_str: &str, construct: &str) {
        self.flush_text();
        // `Inliner.problematic`: `set_id(msg)`, then `set_id(prb)`.
        let msg_id = self.state.ids.allocate(kinds::SYSTEM_MESSAGE, &[], None);
        let prob_id = self.state.ids.allocate(kinds::PROBLEMATIC, &[], None);
        let mut prob = Node::elem(kinds::PROBLEMATIC, self.span);
        prob.attrs.ids.push(prob_id.clone());
        prob.set("refid", AttrValue::Str(msg_id.clone()));
        prob.children
            .push(Node::text_node(start_str.to_string(), self.span));
        self.nodes.push(prob);
        let mut msg = self.report(
            messages::WARNING,
            &format!("Inline {construct} start-string without end-string."),
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
        // `nodes.reference(referencename + refend, ...)` (`states.py:1084`).
        let mut r = Node::elem(kinds::REFERENCE, self.span);
        r.set_rawsource(&format!("{name}{}", "_".repeat(underscores)));
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
            self.state.note_refname(&mut r);
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
                if !crate::utils::py_isspace(prev) && prev != NULL {
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
                let msgs = self.state.note_explicit_target(&mut t, self.loc());
                t.children.push(Node::text_node_escaped(&raw, self.span));
                self.nodes.push(t);
                self.messages.extend(msgs);
                end + 1
            }
            None => {
                // `'Inline %s start-string without end-string.' %
                // nodeclass.__name__` (states.py:839) — the class is
                // `nodes.target`, so the word is "target", not
                // "internal target" (probed against docutils 0.22.4).
                self.emit_problematic("_`", "target");
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
            // `rawsource = unescape(string[matchstart:textend], True)`
            // (`states.py:890`): the whole construct, backquotes and
            // underscores included.
            let construct: String = self.chars[i..end + 1 + underscores].iter().collect();
            self.phrase_reference(&raw, &unescape(&construct, true), underscores);
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
            // No role: the default role, docutils' `''` (`Inliner.interpreted`
            // with `role=''`).
            (None, None) => {
                let rawsource: String = self.chars[i..end + 1].iter().collect();
                self.apply_role("", &raw, &rawsource);
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
            let info = self.report(messages::INFO, &info_text);
            self.messages.push(info);
        }
        // `Inliner.problematic`: `set_id(msg)`, then `set_id(prb)`.
        let msg_id = self.state.ids.allocate(kinds::SYSTEM_MESSAGE, &[], None);
        let prob_id = self.state.ids.allocate(kinds::PROBLEMATIC, &[], None);
        let mut prob = Node::elem(kinds::PROBLEMATIC, self.span);
        prob.attrs.ids.push(prob_id.clone());
        prob.set("refid", AttrValue::Str(msg_id.clone()));
        prob.children
            .push(Node::text_node(unescape(rawsource, true), self.span));
        self.nodes.push(prob);
        let mut msg = self.report(level, text);
        msg.attrs.ids.push(msg_id);
        msg.attrs.backrefs.push(prob_id);
        self.messages.push(msg);
    }

    /// The nodes a role returned, after the text before them.
    fn push_nodes(&mut self, nodes: impl IntoIterator<Item = Node>) {
        self.flush_text();
        self.nodes.extend(nodes);
    }

    /// Where a role that crashes Sphinx is (decision 13): the line of the
    /// text block in its source.
    fn crash_location(&self) -> LogLocation {
        LogLocation::Node {
            source: self.span.source,
            line: self.lineno,
        }
    }

    /// Keeps the document's first crash (decision 13); the parse goes on,
    /// and the build fails with it.
    fn crash(&mut self, crash: RoleCrash) {
        self.state.parse.crash.get_or_insert(crash);
    }

    /// The `:external:`/`:external+inv:` roles, which
    /// `IntersphinxDispatcher` claims *before* any other role lookup
    /// (`ext/intersphinx/_resolve.py:350-366`) when `sphinx.ext.intersphinx`
    /// is loaded ([`roles::resolve_role`]); without it the name is an
    /// unknown domain's.
    ///
    /// `IntersphinxRole.run` (`_resolve.py:378-485`) reads the inventory,
    /// domain and role names from `orig_name`, the name the dispatcher was
    /// given as the document wrote it ([`RoleFn::External`]), not from the
    /// lowercased name `call` settled on: all three are case-sensitive
    /// (`:external+MyInv:` names the inventory `MyInv`, and
    /// `:external:Py:class:` a domain `Py` there is none of). It calls the
    /// domain's role with `self.text`, the text unescaped, so there an
    /// escaped `\<` opens an explicit target and an escaped `\!` disables
    /// the reference (both probed against Sphinx 9.1.0). `run` opens with
    /// `assert self.name == self.orig_name.lower()`, which crashes Sphinx
    /// with an `AssertionError` when the role is called by another name —
    /// through a custom role, or as a default role named with capitals;
    /// [`Self::call_role`] keeps that crash instead of calling this.
    ///
    /// A name that does not name a usable role produces no content at all in
    /// Sphinx, and a warning located at the role. Here the warning text is
    /// parked on a marker node instead: the parse layer knows the role
    /// grammar but not which inventories loaded, and Sphinx checks the
    /// inventory *first* — so deferring the whole report to resolution is
    /// what keeps the precedence between the two right.
    fn emit_external_xref(&mut self, orig_name: &str, call: &RoleCall<'_>) {
        match crate::intersphinx::external_role(orig_name) {
            crate::intersphinx::ExternalRole::Xref {
                inventory,
                domain,
                role,
            } => {
                // `role_func(f'{domain_name}:{role_name}', self.rawtext,
                // self.text, ...)` (`_resolve.py:465-473`): the domain's
                // role, called with its full name and the unescaped text.
                let xref_role = roles::DomainId::from_name(&domain)
                    .map_or(XrefRole::base(), |domain| {
                        XrefRole::of_domain(domain, &role)
                    });
                let name = format!("{domain}:{role}");
                let text = unescape(call.escaped, false);
                let call = RoleCall {
                    name: &name,
                    escaped: &text,
                    ..*call
                };
                let nodes = roles::sphinx::xref(
                    &call,
                    &xref_role,
                    &self.xref_env(),
                    self.state,
                    Some(inventory),
                );
                self.push_nodes(nodes);
            }
            crate::intersphinx::ExternalRole::Failed(diagnostic) => {
                let mut node = Node::elem("pending_xref", self.span);
                node.set("refdoc", AttrValue::Str(self.docname.to_string()));
                node.set("intersphinx", AttrValue::Int(1));
                node.set("intersphinx_role_error", AttrValue::Str(diagnostic.message));
                self.push_nodes([node]);
            }
        }
    }

    /// What an `XRefRole` reads of the document ([`XrefEnv`]).
    fn xref_env(&self) -> XrefEnv<'a> {
        XrefEnv {
            docname: self.docname,
            ref_context: self.ctx,
            add_function_parentheses: self.py.add_function_parentheses,
        }
    }

    /// The interpreted text `:given_name:` + `raw` (`Inliner.interpreted`,
    /// `SP/docutils/parsers/rst/states.py:996-1007`), `given_name` empty
    /// for the default role: look the role up ([`roles::resolve_role`])
    /// and run it ([`Self::call_role`]), or report docutils' `Unknown
    /// interpreted text role` ERROR, naming the role as written, over a
    /// `problematic` of the whole construct. `rawsource` is the
    /// construct, null-escaped.
    fn apply_role(&mut self, given_name: &str, raw: &str, rawsource: &str) {
        if self.dispatch.sphinx && !given_name.is_empty() {
            // Record every named role occurrence for M1's validation:
            // its role name, and the title and target `ReferenceRole`
            // would split, trimmed as M1's scanner trimmed them.
            let lower = given_name.to_lowercase();
            let parts = roles::reference::ReferenceParts::parse(raw);
            let (target, display) = if parts.explicit {
                (
                    parts.target.trim().to_string(),
                    Some(parts.title.trim().to_string()),
                )
            } else {
                (parts.target, None)
            };
            let last_segment = lower.rsplit(':').next().unwrap_or(&lower).to_string();
            self.roles.push(super::RoleRecord {
                source: self.span.source,
                name: last_segment,
                full_name: given_name.to_string(),
                target,
                display,
                line: self.lineno,
            });
        }
        let table = Arc::clone(&self.table);
        let (role, call_name) =
            match roles::resolve_role(&table, &self.dispatch, given_name, self.state) {
                Resolution::Found {
                    role,
                    call_name,
                    info,
                } => {
                    if let Some(info) = info {
                        let message = self.report(messages::INFO, &info);
                        self.messages.push(message);
                    }
                    (role, call_name)
                }
                Resolution::NotFound { info } => {
                    self.role_problematic(
                        rawsource,
                        info,
                        messages::ERROR,
                        &format!("Unknown interpreted text role \"{given_name}\"."),
                    );
                    return;
                }
            };
        self.call_role(&role, &call_name, raw, rawsource, &RoleOptions::default());
    }

    /// Runs `role`, called as `name` with `options`, over the interpreted
    /// text `raw`: a custom role calls its base with its options updated
    /// by the call's; a Sphinx role first settles its name, which for the
    /// default role is the document's (`SphinxRole.__call__`,
    /// `SP/sphinx/util/docutils.py:650-659`) and, with none, a crash
    /// (decision 13). A role's error is reported over a `problematic` of
    /// the raw text (`inliner.problematic(rawtext, rawtext, msg)`).
    fn call_role(
        &mut self,
        role: &RoleFn,
        name: &str,
        raw: &str,
        rawsource: &str,
        options: &RoleOptions,
    ) {
        let location = self.crash_location();
        let settled;
        let name = if role.is_sphinx_role() {
            let default = self
                .state
                .parse
                .default_role
                .clone()
                .filter(|default| !default.is_empty());
            match (name.is_empty(), default) {
                (false, _) => {
                    settled = name.to_lowercase();
                    settled.as_str()
                }
                (true, Some(default)) => {
                    settled = default;
                    settled.as_str()
                }
                (true, None) => {
                    self.crash(RoleCrash::sphinx_error(
                        "cannot determine default role!",
                        location,
                    ));
                    return;
                }
            }
        } else {
            name
        };
        let rawtext = unescape(rawsource, true);
        let call = RoleCall {
            name,
            rawtext: &rawtext,
            escaped: raw,
            options,
            span: self.span,
        };
        let error = |me: &mut Self, text: String| {
            me.role_problematic(rawsource, None, messages::ERROR, &text)
        };
        match role {
            RoleFn::Custom(custom) => {
                let options = custom.options.updated_with(options);
                self.call_role(&custom.base, name, raw, rawsource, &options);
            }
            RoleFn::Generic(kind) => self.push_nodes([roles::docutils::generic(kind, &call)]),
            RoleFn::GenericCustom => self.push_nodes([roles::docutils::generic("inline", &call)]),
            RoleFn::Math => self.push_nodes([roles::docutils::math(&call)]),
            RoleFn::Raw => match roles::docutils::raw(&call) {
                Ok(node) => self.push_nodes([node]),
                Err(text) => error(self, text),
            },
            RoleFn::PepReference => match roles::docutils::pep_reference(&call) {
                Ok(node) => self.push_nodes([node]),
                Err(text) => error(self, text),
            },
            RoleFn::RfcReference => {
                match roles::docutils::rfc_reference(&call, self.dispatch.sphinx) {
                    Ok(node) => self.push_nodes([node]),
                    Err(text) => error(self, text),
                }
            }
            RoleFn::DocutilsCode => self.push_nodes([roles::docutils::docutils_code(&call)]),
            RoleFn::Unimplemented => error(self, roles::docutils::unimplemented(&call)),
            RoleFn::SphinxCode => self.push_nodes([roles::sphinx::sphinx_code(&call)]),
            RoleFn::SphinxGeneric(registered) => {
                self.push_nodes([roles::sphinx::sphinx_generic(&call, registered)])
            }
            RoleFn::Kbd => self.push_nodes(roles::sphinx::kbd(&call)),
            RoleFn::GuiLabel | RoleFn::MenuSelection => {
                let menuselection = matches!(role, RoleFn::MenuSelection);
                match roles::sphinx::guilabel(&call, menuselection, location) {
                    Ok(node) => self.push_nodes([node]),
                    Err(crash) => self.crash(crash),
                }
            }
            RoleFn::EmphasizedLiteral => {
                self.push_nodes([roles::sphinx::emphasized_literal(&call)])
            }
            RoleFn::Abbr => self.push_nodes([roles::sphinx::abbr(&call)]),
            RoleFn::Manpage => {
                let url = self.read.manpages_url.as_deref();
                match roles::sphinx::manpage(&call, url, location) {
                    Ok(node) => self.push_nodes([node]),
                    Err(crash) => self.crash(crash),
                }
            }
            RoleFn::Index => match roles::sphinx::index_role(&call, self.state, location) {
                Ok(nodes) => self.push_nodes(nodes),
                Err(crash) => self.crash(crash),
            },
            RoleFn::Pep | RoleFn::Rfc | RoleFn::Cve | RoleFn::Cwe => {
                let run = match role {
                    RoleFn::Pep => roles::sphinx::pep,
                    RoleFn::Rfc => roles::sphinx::rfc,
                    RoleFn::Cve => roles::sphinx::cve,
                    _ => roles::sphinx::cwe,
                };
                match run(&call, self.state) {
                    Ok(nodes) => self.push_nodes(nodes),
                    Err(text) => error(self, text),
                }
            }
            RoleFn::Download => {
                let env = self.xref_env();
                let nodes = roles::sphinx::download(&call, &env, self.state);
                self.push_nodes(nodes);
            }
            RoleFn::Any | RoleFn::Eq => {
                let xref_role = match role {
                    RoleFn::Any => XrefRole::ANY,
                    _ => XrefRole::EQ,
                };
                let env = self.xref_env();
                let nodes = roles::sphinx::xref(&call, &xref_role, &env, self.state, None);
                self.push_nodes(nodes);
            }
            // The domain's adapter calls its role with the role's full
            // name, whatever name it was called by — the default role's
            // `''` and a custom role's included
            // (`SP/sphinx/domains/__init__.py:157-181`).
            RoleFn::Domain(domain, role_name) => {
                let full_name = format!("{}:{role_name}", domain.name());
                let call = RoleCall {
                    name: &full_name,
                    ..call
                };
                let env = self.xref_env();
                let xref_role = XrefRole::of_domain(*domain, role_name);
                let nodes = roles::sphinx::xref(&call, &xref_role, &env, self.state, None);
                self.push_nodes(nodes);
            }
            // `IntersphinxRole.run` opens with `assert self.name ==
            // self.orig_name.lower()` (`_resolve.py:379`): called by any
            // other name, through a custom role or as a default role whose
            // name has a capital, the role crashes Sphinx with an
            // `AssertionError` that has no message (decision 13).
            RoleFn::External(orig_name) => {
                if call.name == orig_name.to_lowercase() {
                    self.emit_external_xref(orig_name, &call);
                } else {
                    self.crash(RoleCrash::python("AssertionError", String::new(), location));
                }
            }
        }
    }

    /// Phrase reference body handling incl. embedded `<uri>`/`<alias_>`
    /// (`phrase_ref`, `states.py:913-986`). `rawsource` is the whole
    /// construct, the reference's docutils `rawsource`.
    fn phrase_reference(&mut self, raw: &str, rawsource: &str, underscores: usize) {
        self.flush_text();
        let ids = |s: &str| crate::doctree::ids::fully_normalize_name(s);
        let wsn = |s: &str| crate::doctree::ids::whitespace_normalize_name(s);

        // embedded link: unescaped `<...>` at the very end, preceded by
        // whitespace (or the whole content).
        let embedded = find_embedded_link(raw);
        let mut r = Node::elem(kinds::REFERENCE, self.span);
        r.set_rawsource(rawsource);
        match embedded {
            Some((text_part, link)) => {
                // `match.group(1)`: the link with the whitespace before it,
                // the rawsource of the target the link makes
                // (`states.py:929`, `:941`).
                let link_source = &raw[text_part.len()..];
                let is_alias = link.ends_with('_')
                    && !link.ends_with("\u{0}_")
                    && !link
                        .chars()
                        .rev()
                        .nth(1)
                        .map(|c| c == NULL)
                        .unwrap_or(false)
                    && !looks_like_uri(&link);
                // The reference's Text is docutils' `text = escaped[:match
                // .start(0)]` (`states.py:918`), markers kept; an empty text
                // falls back to the alias or URI, already unescaped — the
                // alias normalized, as `refname` is (`states.py:946-949`).
                let display_text;
                if is_alias {
                    let alias = ids(&unescape(&link[..link.len() - 1], false));
                    let display_data = if text_part.is_empty() {
                        alias.clone()
                    } else {
                        text_part.clone()
                    };
                    display_text = unescape(&display_data, false);
                    r.set("name", AttrValue::Str(wsn(&display_text)));
                    r.set("refname", AttrValue::Str(alias.clone()));
                    r.children
                        .push(Node::text_node_escaped(&display_data, self.span));
                    if underscores == 1 {
                        // `nodes.target(..., refname=alias)` is built with its
                        // reference, which binding its name compares. The
                        // target is noted as an indirect target BEFORE the
                        // reference's `refname` (`states.py:975-980`), so
                        // `refnames[alias]` lists the target first.
                        let mut t = Node::elem(kinds::TARGET, self.span);
                        t.set_rawsource(link_source);
                        t.attrs.names.push(ids(&display_text));
                        t.set("refname", AttrValue::Str(alias));
                        self.state.note_indirect_target(&mut t);
                        self.state.note_refname(&mut r);
                        let msgs = self.state.note_implicit_target(&mut t, self.loc());
                        self.nodes.push(r);
                        self.nodes.push(t);
                        self.messages.extend(msgs);
                    } else {
                        // `__` with an alias: `reference['refname'] = alias`
                        // and `note_refname(reference)` (`states.py:964-966`).
                        self.state.note_refname(&mut r);
                        self.nodes.push(r);
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
                    let display_data = if text_part.is_empty() {
                        uri.clone()
                    } else {
                        text_part.clone()
                    };
                    display_text = unescape(&display_data, false);
                    r.set("name", AttrValue::Str(wsn(&display_text)));
                    r.set("refuri", AttrValue::Str(uri.clone()));
                    r.children
                        .push(Node::text_node_escaped(&display_data, self.span));
                    self.nodes.push(r);
                    if underscores == 1 {
                        // `nodes.target(..., refuri=alias)`, as above.
                        let mut t = Node::elem(kinds::TARGET, self.span);
                        t.set_rawsource(link_source);
                        t.attrs.names.push(ids(&display_text));
                        t.set("refuri", AttrValue::Str(uri));
                        let msgs = self.state.note_implicit_target(&mut t, self.loc());
                        self.nodes.push(t);
                        self.messages.extend(msgs);
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
                    self.state.note_refname(&mut r);
                }
                // `text = escaped` (`states.py:951`).
                r.children.push(Node::text_node_escaped(raw, self.span));
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
        // `footnote_reference` (`states.py:1050-1080`): both node kinds
        // carry `'[%s]_' % label` as their rawsource, and each `note_*`
        // call runs `set_id` — twice on one node for `[#name]_`, whose
        // second call re-registers the id the first allocated.
        let set_refname = |node: &mut Node, label: &str| {
            node.set(
                "refname",
                AttrValue::Str(crate::doctree::ids::fully_normalize_name(label)),
            );
        };
        let node = if is_citation {
            let mut c = Node::elem(kinds::CITATION_REFERENCE, self.span);
            c.set_rawsource(&format!("[{label}]_"));
            set_refname(&mut c, &label);
            c.children.push(Node::text_node(label.clone(), self.span));
            self.state.note_citation_ref(&mut c);
            c
        } else {
            let mut f = Node::elem(kinds::FOOTNOTE_REFERENCE, self.span);
            f.set_rawsource(&format!("[{label}]_"));
            let refname = if label == "*" {
                f.set("auto", AttrValue::Str("*".to_string()));
                self.state.note_symbol_footnote_ref(&mut f);
                ""
            } else if let Some(rest) = label.strip_prefix('#') {
                f.set("auto", AttrValue::Int(1));
                self.state.note_autofootnote_ref(&mut f);
                rest
            } else {
                f.children.push(Node::text_node(label.clone(), self.span));
                &label
            };
            if !refname.is_empty() {
                set_refname(&mut f, refname);
                self.state.note_footnote_ref(&mut f);
            }
            f
        };
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
        // `inline_obj`'s `rawsource = unescape(string[matchstart:textend],
        // True)` (`states.py:834`): the end pattern `\|_{0,2}` puts the
        // underscores inside it.
        let construct: String = self.chars[i..after].iter().collect();
        let mut subref = Node::elem(kinds::SUBSTITUTION_REFERENCE, self.span);
        subref.set_rawsource(&unescape(&construct, true));
        subref.set(
            "refname",
            AttrValue::Str(crate::doctree::ids::whitespace_normalize_name(&text)),
        );
        subref
            .children
            .push(Node::text_node_escaped(&raw, self.span));
        if underscores == 0 {
            self.nodes.push(subref);
        } else {
            // `nodes.reference('|%s%s' % (subref_text, endstring), '')`
            // (`states.py:1035-1036`), noted under its `refname` unless
            // anonymous.
            let mut outer = Node::elem(kinds::REFERENCE, self.span);
            outer.set_rawsource(&format!("|{text}|{}", "_".repeat(underscores)));
            if underscores == 2 {
                outer.set("anonymous", AttrValue::Int(1));
            } else {
                outer.set(
                    "refname",
                    AttrValue::Str(crate::doctree::ids::fully_normalize_name(&text)),
                );
                self.state.note_refname(&mut outer);
            }
            outer.children.push(subref);
            self.nodes.push(outer);
        }
        after
    }
}

/// Remove whitespace per line from a URI (escaped whitespace survives as
/// literal after unescape). docutils' `''.join(unescape(part).split())` is
/// Python's `str.split()`, so `\x1f` goes too ([`crate::utils::py_isspace`]).
fn clean_uri(link: &str) -> String {
    let joined: String = link
        .split('\n')
        .map(|part| {
            part.chars()
                .filter(|c| !crate::utils::py_isspace(*c))
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
    // `%(non_whitespace_escape_before)s>` — the closing '>' must be
    // preceded by neither an escape null nor Python whitespace
    // (`(?<![\s\x00])`, states.py:780).
    if n >= 2 && (chars[n - 2] == NULL || crate::utils::py_isspace(chars[n - 2])) {
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
        // `(?:[ \n]+|^)` — LITERAL space or newline, not `\s`.
        let before = chars[open - 1];
        if !(before == ' ' || before == '\n') {
            return None;
        }
    }
    // `<%(non_whitespace_after)s` — `(?!\s)` after the open bracket.
    if chars
        .get(open + 1)
        .is_some_and(|c| crate::utils::py_isspace(*c))
    {
        return None;
    }
    let link: String = chars[open + 1..n - 1].iter().collect();
    if link.is_empty() {
        return None;
    }
    // `escaped[:match.start(0)]`, and the match starts at the leftmost of
    // the `[ \n]+` run.
    let text: String = chars[..open].iter().collect();
    Some((text.trim_end_matches([' ', '\n']).to_string(), link))
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
/// title, a term…) on its own, in docutils mode. `lineno` anchors any
/// generated messages, which stamp `source_path` when the state has no
/// source table yet (a state the block parser threads already has one).
pub fn parse_inline(
    text: &str,
    span: Span,
    lineno: u32,
    state: &mut DocumentState,
    source_path: &str,
) -> InlineResult {
    if state.parse.sources.is_empty() {
        state.parse.sources.push(std::sync::Arc::from(source_path));
    }
    // Docutils mode: no sphinx role reads the py or read configuration,
    // so the defaults are as good as any.
    let py = crate::py::PySigConfig::default();
    parse_inline_ext(
        text,
        span,
        lineno,
        state,
        DispatchCtx {
            sphinx: false,
            default_domain: None,
            intersphinx: false,
        },
        "index",
        RefContext::default(),
        &py,
        &super::ReadConfig::default(),
    )
}

/// [`parse_inline`] with the mode, role dispatch and context the block
/// parser passes; messages stamp the path of `span`'s source from the
/// state's source table. Roles are looked up in the document's role
/// table ([`super::ParseOnlyState::roles`]), the mode's built-in one when
/// the document has none of its own.
#[allow(clippy::too_many_arguments)]
pub(crate) fn parse_inline_ext(
    text: &str,
    span: Span,
    lineno: u32,
    state: &mut DocumentState,
    dispatch: DispatchCtx,
    docname: &str,
    ctx: RefContext<'_>,
    py: &crate::py::PySigConfig,
    read: &super::ReadConfig,
) -> InlineResult {
    let escaped = escape2null(text);
    let table = state
        .parse
        .roles
        .clone()
        .unwrap_or_else(|| RoleTable::builtin(dispatch.sphinx));
    let mut inliner = Inliner {
        chars: escaped.chars().collect(),
        span,
        lineno,
        state,
        table,
        read,
        dispatch,
        docname,
        ctx,
        py,
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
        let mut state = DocumentState::default();
        let r = parse_inline(text, Span::ZERO, 1, &mut state, "<snippet>");
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

    /// Sphinx-mode inline parse, for the roles that only exist there: the
    /// default domain is `py`, and `sphinx.ext.intersphinx` is loaded, so
    /// the `:external:` roles exist.
    fn sphinx_nodes(text: &str) -> Vec<Node> {
        sphinx_nodes_with(text, &crate::py::PySigConfig::default())
    }

    /// The same, under a chosen py-domain configuration.
    fn sphinx_nodes_with(text: &str, py: &crate::py::PySigConfig) -> Vec<Node> {
        sphinx_nodes_in(text, RefContext::default(), py)
    }

    /// The same, under a chosen ref_context.
    fn sphinx_nodes_in(text: &str, ctx: RefContext<'_>, py: &crate::py::PySigConfig) -> Vec<Node> {
        let mut state = DocumentState::default();
        let dispatch = DispatchCtx {
            sphinx: true,
            default_domain: Some(roles::DomainId::Py),
            intersphinx: true,
        };
        parse_inline_ext(
            text,
            Span::ZERO,
            1,
            &mut state,
            dispatch,
            "index",
            ctx,
            py,
            &crate::rst::ReadConfig::default(),
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

    /// `IntersphinxRole` reads its inventory, domain and role names from
    /// the name as written (`orig_name`, `_resolve.py:378-380`), not the
    /// lowercased one: an inventory keeps its capitals, and a capital in
    /// the domain or the role name names neither (probed against Sphinx
    /// 9.1.0: with an `intersphinx_mapping` key `MyInv`, the first
    /// resolves, and the other two warn in these words).
    #[test]
    fn an_external_role_reads_its_names_as_written() {
        let nodes = sphinx_nodes(":external+MyInv:py:class:`int`");
        assert_eq!(nodes.len(), 1);
        let xref = &nodes[0];
        assert_eq!(xref.kind, "pending_xref");
        assert_eq!(
            attr(xref, "inventory"),
            Some(&AttrValue::Str("MyInv".into()))
        );
        assert_eq!(attr(xref, "refdomain"), Some(&AttrValue::Str("py".into())));
        assert_eq!(attr(xref, "reftype"), Some(&AttrValue::Str("class".into())));
        for (text, expected) in [
            (
                ":external:Py:class:`int`",
                "domain for external cross-reference not found: 'Py'",
            ),
            (
                ":external:py:Class:`int`",
                "role for external cross-reference not found in domain 'py': 'Class'",
            ),
        ] {
            let nodes = sphinx_nodes(text);
            assert_eq!(nodes.len(), 1, "{text}");
            assert_eq!(
                attr(&nodes[0], "intersphinx_role_error"),
                Some(&AttrValue::Str(expected.to_string())),
                "{text}"
            );
        }
    }

    /// The domain's role gets `IntersphinxRole.text`, the text unescaped
    /// (`_resolve.py:465-473`), so there an escaped `\<` opens an explicit
    /// target and an escaped `\!` disables the reference, where the same
    /// role called directly keeps both literal (probed against Sphinx
    /// 9.1.0: `x \<int>` links `int` titled `x`, `\!int` is a plain
    /// `literal`).
    #[test]
    fn an_external_role_gives_the_domain_role_the_unescaped_text() {
        let nodes = sphinx_nodes(":external+myinv:py:class:`x \\<int>`");
        assert_eq!(nodes.len(), 1);
        let xref = &nodes[0];
        assert_eq!(xref.kind, "pending_xref");
        assert_eq!(attr(xref, "refexplicit"), Some(&AttrValue::Int(1)));
        assert_eq!(attr(xref, "reftarget"), Some(&AttrValue::Str("int".into())));
        assert_eq!(xref.astext(), "x");
        assert_eq!(
            attr(xref, "inventory"),
            Some(&AttrValue::Str("myinv".into()))
        );

        let direct = sphinx_nodes(":py:class:`x \\<int>`");
        assert_eq!(attr(&direct[0], "refexplicit"), Some(&AttrValue::Int(0)));
        assert_eq!(
            attr(&direct[0], "reftarget"),
            Some(&AttrValue::Str("x <int>".into()))
        );

        let nodes = sphinx_nodes(":external+myinv:py:class:`\\!int`");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, kinds::LITERAL);
        assert_eq!(nodes[0].astext(), "int");
        assert_eq!(attr(&nodes[0], "intersphinx"), None);
        let direct = sphinx_nodes(":py:class:`\\!int`");
        assert_eq!(direct[0].kind, "pending_xref");
        assert_eq!(
            attr(&direct[0], "reftarget"),
            Some(&AttrValue::Str("!int".into()))
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

    /// The interception is by name, so a name that merely starts with the
    /// same letters goes through the ordinary lookup, which has no
    /// `externally` role: docutils' unknown-role `problematic`, no
    /// intersphinx node.
    #[test]
    fn a_role_named_like_external_is_not_intercepted() {
        let nodes = sphinx_nodes(":externally:`x`");
        assert_eq!(nodes[0].kind, kinds::PROBLEMATIC);
        assert_eq!(attr(&nodes[0], "intersphinx"), None);
    }

    // --- XRefRole.update_title_and_target (`roles.py:87-98`) ---------------
    //
    // Expected shapes below are the probe-verified pformat of the research
    // spec §3.2 (probes P1-P7, full outputs in its appendix A.3) plus the
    // ordering probes Q1-Q11 recorded in the task-2 brief's "Probe
    // outcomes". `add_function_parentheses` is the ONLY config value these
    // read; everything else in the bundle stays at its default.

    fn with_parens(add_function_parentheses: bool) -> crate::py::PySigConfig {
        crate::py::PySigConfig {
            add_function_parentheses,
            ..crate::py::PySigConfig::default()
        }
    }

    /// `(literal text, reftarget)` of the single xref one snippet produces.
    fn xref_title_and_target(text: &str, add_function_parentheses: bool) -> (String, String) {
        let nodes = sphinx_nodes_with(text, &with_parens(add_function_parentheses));
        assert_eq!(nodes.len(), 1, "{text}");
        let xref = &nodes[0];
        let target = match attr(xref, "reftarget") {
            Some(AttrValue::Str(target)) => target.clone(),
            other => panic!("reftarget on {text}: {other:?}"),
        };
        (xref.astext(), target)
    }

    /// P1/P6: an implicit `:py:func:`/`:py:meth:` title gains `()` under the
    /// default config. P7: `:py:class:` has no `fix_parens` and never does.
    #[test]
    fn an_implicit_fix_parens_title_gains_parens_by_default() {
        assert_eq!(
            xref_title_and_target(":py:func:`mymod.myfunc`", true),
            ("mymod.myfunc()".to_string(), "mymod.myfunc".to_string()),
            "P1"
        );
        assert_eq!(
            xref_title_and_target(":py:meth:`Obj.method`", true),
            ("Obj.method()".to_string(), "Obj.method".to_string()),
            "P6"
        );
        assert_eq!(
            xref_title_and_target(":py:class:`mymod.MyClass`", true),
            ("mymod.MyClass".to_string(), "mymod.MyClass".to_string()),
            "P7: no fix_parens on the class role"
        );
    }

    /// P3: the parens the author wrote are not doubled — sphinx strips a
    /// trailing `()` before deciding whether to append one.
    #[test]
    fn an_already_parenthesized_implicit_title_is_not_doubled() {
        assert_eq!(
            xref_title_and_target(":py:func:`mymod.myfunc()`", true),
            ("mymod.myfunc()".to_string(), "mymod.myfunc".to_string()),
            "P3"
        );
    }

    /// P2/P4: with `add_function_parentheses = False` an implicit title
    /// loses its parens — including parens the author wrote themselves.
    #[test]
    fn add_function_parentheses_false_removes_the_parens_the_author_wrote() {
        assert_eq!(
            xref_title_and_target(":py:func:`mymod.myfunc`", false),
            ("mymod.myfunc".to_string(), "mymod.myfunc".to_string()),
            "P2"
        );
        assert_eq!(
            xref_title_and_target(":py:func:`mymod.myfunc()`", false),
            ("mymod.myfunc".to_string(), "mymod.myfunc".to_string()),
            "P4"
        );
        assert_eq!(
            xref_title_and_target(":py:meth:`Obj.method()`", false),
            ("Obj.method".to_string(), "Obj.method".to_string()),
            "Q7"
        );
    }

    /// P5/Q6: an explicit title is never touched, under either setting —
    /// the guard is `if not self.has_explicit_title` (`roles.py:88`).
    #[test]
    fn an_explicit_title_is_untouched_under_both_settings() {
        for add_parens in [true, false] {
            assert_eq!(
                xref_title_and_target(":py:func:`custom title <mymod.myfunc>`", add_parens),
                ("custom title".to_string(), "mymod.myfunc".to_string()),
                "P5, add_function_parentheses={add_parens}"
            );
            assert_eq!(
                xref_title_and_target(":py:func:`other() <mymod.myfunc>`", add_parens),
                ("other()".to_string(), "mymod.myfunc".to_string()),
                "Q6: an explicit title keeps parens it wrote itself"
            );
        }
    }

    /// The target strip sits OUTSIDE the explicit-title guard
    /// (`roles.py:97`), so it happens under both settings and for explicit
    /// titles too (Q4/Q5) — and it removes exactly one pair (Q10/Q11).
    #[test]
    fn the_target_always_loses_one_trailing_paren_pair() {
        for add_parens in [true, false] {
            assert_eq!(
                xref_title_and_target(":py:func:`custom title <mymod.myfunc()>`", add_parens).1,
                "mymod.myfunc",
                "Q4/Q5, add_function_parentheses={add_parens}"
            );
        }
        assert_eq!(
            xref_title_and_target(":py:func:`mymod.myfunc()()`", true),
            ("mymod.myfunc()()".to_string(), "mymod.myfunc()".to_string()),
            "Q10: one pair off the target, and a title already ending in \
             `()` gains nothing"
        );
        assert_eq!(
            xref_title_and_target(":py:func:`mymod.myfunc()()`", false),
            ("mymod.myfunc()".to_string(), "mymod.myfunc()".to_string()),
            "Q11: one pair off each"
        );
    }

    /// Q8/Q9: a py role without `fix_parens` keeps its parens in BOTH the
    /// title and the target — the strip is not a py-domain-wide rule.
    #[test]
    fn a_py_role_without_fix_parens_keeps_its_parens() {
        for (text, expected) in [
            (":py:obj:`mymod.thing()`", "mymod.thing()"),
            (":py:data:`mymod.thing()`", "mymod.thing()"),
        ] {
            assert_eq!(
                xref_title_and_target(text, true),
                (expected.to_string(), expected.to_string()),
                "{text}"
            );
        }
    }

    /// Q2/Q3: `update_title_and_target` runs BEFORE
    /// `PyXRefRole.process_link`, so the target loses its `()` before the
    /// `~` shortening splits it — and the shortening runs on the title,
    /// which by then carries the decided parens.
    #[test]
    fn the_tilde_shortening_sees_the_title_fix_parens_already_produced() {
        assert_eq!(
            xref_title_and_target(":py:func:`~mymod.myfunc`", true),
            ("myfunc()".to_string(), "mymod.myfunc".to_string()),
            "Q1"
        );
        assert_eq!(
            xref_title_and_target(":py:func:`~mymod.myfunc()`", true),
            ("myfunc()".to_string(), "mymod.myfunc".to_string()),
            "Q2"
        );
        assert_eq!(
            xref_title_and_target(":py:func:`~mymod.myfunc()`", false),
            ("myfunc".to_string(), "mymod.myfunc".to_string()),
            "Q3"
        );
    }

    // --- PyXRefRole.process_link (`domains/python/__init__.py:559-600`) ----
    //
    // Expected shapes pasted from this task's probe_t6 run ([PY §3.1]
    // probes roles_basic / roles_tilde_dot / role_deco and the
    // role_*_lstrip / role_dot_explicit_title cases).

    /// [PY §3.1 roles_basic]: every py xref carries the ref_context attrs
    /// (None → "True" sentinel) and NO refspecific attribute at all.
    #[test]
    fn a_py_xref_outside_any_scope_stamps_the_none_sentinels() {
        let nodes = sphinx_nodes(":py:func:`target`");
        let xref = &nodes[0];
        assert_eq!(attr(xref, "py:class"), Some(&AttrValue::Str("True".into())));
        assert_eq!(
            attr(xref, "py:module"),
            Some(&AttrValue::Str("True".into()))
        );
        assert_eq!(attr(xref, "refspecific"), None, "absent, not 0");
    }

    /// [PY §3.1 roles_tilde_dot]: a `.`-prefixed target strips the dot and
    /// stamps `refspecific="1"`; the implicit title lost the dot too.
    #[test]
    fn a_dot_prefixed_target_becomes_refspecific() {
        let nodes = sphinx_nodes(":py:meth:`.Cls.meth`");
        let xref = &nodes[0];
        assert_eq!(attr(xref, "refspecific"), Some(&AttrValue::Int(1)));
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str("Cls.meth".into()))
        );
        assert_eq!(xref.astext(), "Cls.meth()");

        // The dot branch sits OUTSIDE the implicit-title guard: an
        // explicit title keeps its text while the target still turns
        // refspecific (probe role_dot_explicit_title).
        let nodes = sphinx_nodes(":py:meth:`M <.Cls.meth>`");
        let xref = &nodes[0];
        assert_eq!(attr(xref, "refspecific"), Some(&AttrValue::Int(1)));
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str("Cls.meth".into()))
        );
        assert_eq!(xref.astext(), "M");
    }

    /// Probe role_title_lstrip_dots: `..pkg.f` — the implicit title loses
    /// EVERY leading dot, the target loses exactly ONE (the refspecific
    /// branch), leaving `reftarget=".pkg.f"`.
    #[test]
    fn title_lstrips_all_dots_while_the_target_loses_one() {
        let nodes = sphinx_nodes(":py:func:`..pkg.f`");
        let xref = &nodes[0];
        assert_eq!(xref.astext(), "pkg.f()");
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str(".pkg.f".into()))
        );
        assert_eq!(attr(xref, "refspecific"), Some(&AttrValue::Int(1)));
    }

    /// Probe role_target_lstrip_tilde: the target `lstrip`s every leading
    /// `~` while the title's shortening consumes one and keeps the last
    /// dotted component.
    #[test]
    fn the_target_lstrips_every_tilde() {
        let nodes = sphinx_nodes(":py:func:`~~pkg.f`");
        let xref = &nodes[0];
        assert_eq!(xref.astext(), "f()");
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str("pkg.f".into()))
        );
        assert_eq!(attr(xref, "refspecific"), None);
    }

    /// [PY §3.1 role_deco]: `_PyDecoXRefRole` prefixes `@` to the title —
    /// UNCONDITIONALLY, explicit titles included (probe role_deco_explicit)
    /// — while the target stays bare; the inner literal is `xref py py-deco`.
    #[test]
    fn the_deco_role_prefixes_an_at_sign() {
        let nodes = sphinx_nodes(":py:deco:`mydeco`");
        let xref = &nodes[0];
        assert_eq!(xref.astext(), "@mydeco");
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str("mydeco".into()))
        );
        let inner = &xref.children[0];
        assert_eq!(inner.kind, kinds::LITERAL);
        assert_eq!(
            inner.attrs.classes,
            vec!["xref".to_string(), "py".to_string(), "py-deco".to_string()]
        );

        let nodes = sphinx_nodes(":py:deco:`custom <target>`");
        let xref = &nodes[0];
        assert_eq!(xref.astext(), "@custom");
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str("target".into()))
        );
    }

    /// The ref_context stamping path: a parse carrying a module/class
    /// scope lands both on the xref instead of the sentinels.
    #[test]
    fn a_py_xref_inside_a_scope_stamps_the_ref_context() {
        let py = crate::py::PySigConfig::default();
        let nodes = sphinx_nodes_in(
            ":py:func:`target`",
            RefContext {
                py_module: Some("mymod"),
                py_class: Some("C"),
                ..RefContext::default()
            },
            &py,
        );
        let xref = &nodes[0];
        assert_eq!(attr(xref, "py:class"), Some(&AttrValue::Str("C".into())));
        assert_eq!(
            attr(xref, "py:module"),
            Some(&AttrValue::Str("mymod".into()))
        );
    }

    // --- AnyXRefRole (`sphinx/roles.py`), probe-verified 2026-09-02 --------

    /// `:any:` is registered domainless: `refdomain=""`, `reftype="any"`,
    /// `warn_dangling=True`, inner `literal classes="xref any"` — and with
    /// NO ref_context in scope, no context attribute at all (unlike
    /// `PyXRefRole`'s "True" sentinels).
    #[test]
    fn an_any_role_emits_a_domainless_pending_xref() {
        let nodes = sphinx_nodes(":any:`target`");
        let xref = &nodes[0];
        assert_eq!(xref.kind, "pending_xref");
        assert_eq!(attr(xref, "refdomain"), Some(&AttrValue::Str("".into())));
        assert_eq!(attr(xref, "reftype"), Some(&AttrValue::Str("any".into())));
        assert_eq!(attr(xref, "refwarn"), Some(&AttrValue::Int(1)));
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str("target".into()))
        );
        for absent in [
            "py:class",
            "py:classes",
            "py:module",
            "py:modules",
            "std:program",
        ] {
            assert_eq!(attr(xref, absent), None, "{absent} must not be stamped");
        }
        let inner = &xref.children[0];
        assert_eq!(inner.kind, kinds::LITERAL);
        assert_eq!(
            inner.attrs.classes,
            vec!["xref".to_string(), "any".to_string()]
        );
    }

    /// `AnyXRefRole.process_link` copies every EXISTING ref_context key:
    /// values for the strings, `""` for the (drained) list keys, and the
    /// `"True"` sentinel for a `py:class` key holding `None`.
    #[test]
    fn an_any_role_copies_the_ref_context_keys_that_exist() {
        let py = crate::py::PySigConfig::default();
        let nodes = sphinx_nodes_in(
            ":any:`target`",
            RefContext {
                program: Some("prog"),
                py_module: Some("m"),
                py_class: None,
                py_class_key: true,
                py_module_key: true,
                py_classes_key: true,
                py_modules_key: false,
            },
            &py,
        );
        let xref = &nodes[0];
        assert_eq!(attr(xref, "py:class"), Some(&AttrValue::Str("True".into())));
        assert_eq!(attr(xref, "py:classes"), Some(&AttrValue::Str("".into())));
        assert_eq!(attr(xref, "py:module"), Some(&AttrValue::Str("m".into())));
        assert_eq!(attr(xref, "py:modules"), None);
        assert_eq!(
            attr(xref, "std:program"),
            Some(&AttrValue::Str("prog".into()))
        );
    }

    /// The key-exists-holding-`None` shape `after_content` leaves behind
    /// after a `:module:`-carrying directive with no enclosing module
    /// scope: sphinx copies the key and docutils renders the `None` value
    /// as `"True"`, exactly like `py:class`.
    ///
    // oracle: sphinx 9.1.0 full build of `.. py:function:: f()` +
    // `:module: mymod` + body + ``After :any:`x`.`` -> the pending_xref
    // carries py:module="True" (scratchpad A/p8.py).
    #[test]
    fn an_any_role_stamps_a_present_but_none_py_module() {
        let py = crate::py::PySigConfig::default();
        let nodes = sphinx_nodes_in(
            ":any:`target`",
            RefContext {
                program: None,
                py_module: None,
                py_class: None,
                py_class_key: true,
                py_module_key: true,
                py_classes_key: true,
                py_modules_key: true,
            },
            &py,
        );
        let xref = &nodes[0];
        assert_eq!(
            attr(xref, "py:module"),
            Some(&AttrValue::Str("True".into()))
        );
        assert_eq!(attr(xref, "py:modules"), Some(&AttrValue::Str("".into())));
    }

    /// ... and an absent key still stamps nothing.
    #[test]
    fn an_any_role_omits_an_absent_py_module() {
        let py = crate::py::PySigConfig::default();
        let nodes = sphinx_nodes_in(
            ":any:`target`",
            RefContext {
                program: None,
                py_module: None,
                py_class: None,
                py_class_key: false,
                py_module_key: false,
                py_classes_key: false,
                py_modules_key: false,
            },
            &py,
        );
        assert_eq!(attr(&nodes[0], "py:module"), None);
    }

    /// The base `XRefRole.process_link` munging is the ONLY one `:any:`
    /// takes: whitespace runs collapse, case is kept, parens are kept, and
    /// an explicit title stays untouched.
    #[test]
    fn an_any_target_collapses_whitespace_and_nothing_else() {
        let nodes = sphinx_nodes(":any:`Foo()`");
        assert_eq!(
            attr(&nodes[0], "reftarget"),
            Some(&AttrValue::Str("Foo()".into()))
        );
        let nodes = sphinx_nodes(":any:`two\nlines`");
        assert_eq!(
            attr(&nodes[0], "reftarget"),
            Some(&AttrValue::Str("two lines".into()))
        );
        let nodes = sphinx_nodes(":any:`Click <target>`");
        let xref = &nodes[0];
        assert_eq!(attr(xref, "refexplicit"), Some(&AttrValue::Int(1)));
        assert_eq!(
            attr(xref, "reftarget"),
            Some(&AttrValue::Str("target".into()))
        );
        assert_eq!(xref.astext(), "Click");
    }
}
