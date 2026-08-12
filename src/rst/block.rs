//! The block-level recursive-descent parser (M2 wave 1).
//!
//! Model: docutils' `RSTStateMachine` re-expressed as recursive descent over
//! dedented line views. Every nested construct materializes a `Vec<LineRef>`
//! dedented to its own base column (docutils `get_indented` does the same),
//! so all productions parse "at column 0". Line numbers and byte spans ride
//! along on each `LineRef`.
//!
//! Dispatch order matches docutils `Body.initial_transitions`: bullet,
//! enumerator, doctest, line_block, explicit markup, anonymous target,
//! adornment line, text (underline-title / definition list / paragraph).
//! Behavior sources: probe notes (2026-08-07-m2-wave1-probes.md) and the
//! committed differential fixture — never memory.

use crate::doctree::ids::{self, IdRegistry};
use crate::doctree::{kinds, messages, AttrValue, Node, Span};

use super::lines::Lines;

const ADORNMENT_CHARS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
const BULLET_CHARS: [char; 6] = ['*', '+', '-', '\u{2022}', '\u{2023}', '\u{2043}'];

/// A pending block-quote segment: accumulated body lines plus an optional
/// (attribution node, marker lineno) that closed it.
type QuoteSegment<'a> = (Vec<LineRef<'a>>, Option<(Node, u32)>);

#[derive(Copy, Clone, Debug)]
struct LineRef<'a> {
    text: &'a str,
    lineno: u32,
    src_start: u32,
    src_end: u32,
    /// Cached leading-space count. Computed once at view construction and
    /// derived arithmetically on dedent — re-scanning per nesting level made
    /// deep nesting O(depth^3) (measured: 800-level nest took ~0.5s).
    indent: u32,
}

impl<'a> LineRef<'a> {
    fn new(text: &'a str, lineno: u32, src_start: u32, src_end: u32) -> LineRef<'a> {
        let indent = (text.len() - text.trim_start_matches(' ').len()) as u32;
        LineRef {
            text,
            lineno,
            src_start,
            src_end,
            indent,
        }
    }

    fn is_blank(&self) -> bool {
        self.text.is_empty()
    }

    fn indent(&self) -> usize {
        self.indent as usize
    }

    /// Dedent by `n` columns (leading columns are spaces by construction;
    /// marker lines are re-wrapped with [`LineRef::new`] instead).
    fn dedented(&self, n: usize) -> LineRef<'a> {
        let n = n.min(self.indent());
        LineRef {
            text: &self.text[n..],
            indent: self.indent - n as u32,
            ..*self
        }
    }
}

/// Slice a marker line after `n_chars` characters (char-aware: unicode
/// bullets are multi-byte).
fn rest_after(text: &str, n_chars: usize) -> &str {
    match text.char_indices().nth(n_chars) {
        Some((i, _)) => &text[i..],
        None => "",
    }
}

fn adornment_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if ADORNMENT_CHARS.contains(first) && chars.all(|c| c == first) {
        Some(first)
    } else {
        None
    }
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// docutils `column_width`: east-asian wide/fullwidth chars count 2.
fn column_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

/// Definition-list term split on the docutils classifier delimiter
/// `' +: +'` (one-or-more spaces, colon, one-or-more spaces).
fn split_classifiers(term: &str) -> Vec<String> {
    lazy_static::lazy_static! {
        static ref CLASSIFIER_RE: regex::Regex = regex::Regex::new(" +: +").unwrap();
    }
    CLASSIFIER_RE.split(term).map(str::to_string).collect()
}

struct SectionStart {
    title: String,
    style: (char, bool),
    /// Raw title + underline lines, for error literals.
    raw_lines: String,
    /// Extra messages inserted right after `<title>` (short-underline
    /// warning); the duplicate-name INFO is added by the caller.
    messages: Vec<Node>,
    title_lineno: u32,
    underline_lineno: u32,
    span: Span,
}

/// Nested-container recursion cap. Real documents nest ~10 deep; docutils
/// itself dies with RecursionError near Python's limit (~1000). We stay
/// total: content beyond this depth is dropped with an ERROR message.
const MAX_NEST_DEPTH: usize = 200;

pub(crate) struct BlockParser<'a> {
    top: Vec<LineRef<'a>>,
    pub(crate) source_path: &'a str,
    source_len: usize,
    pub(crate) registry: IdRegistry,
    styles: Vec<(char, bool)>,
    depth: usize,
    /// +1 inside table-cell nested parses: docutils' state-machine-derived
    /// line numbers (the unindent/unexpected-indentation family) run one
    /// high there (probe-verified); content-anchored messages stay absolute.
    line_bias: u32,
}

impl<'a> BlockParser<'a> {
    pub(crate) fn new(lines: &'a Lines, source_path: &'a str, source_len: usize) -> Self {
        let top = lines
            .iter()
            .enumerate()
            .map(|(i, l)| LineRef::new(&l.text, (i + 1) as u32, l.src_start, l.src_end))
            .collect();
        BlockParser {
            top,
            source_path,
            source_len,
            registry: IdRegistry::new(),
            styles: Vec::new(),
            depth: 0,
            line_bias: 0,
        }
    }

    fn span_of(&self, lines: &[LineRef<'_>], first: usize, last: usize) -> Span {
        let start = lines.get(first).map(|l| l.src_start).unwrap_or(0);
        let end = lines
            .get(last.min(lines.len().saturating_sub(1)))
            .map(|l| l.src_end)
            .unwrap_or(start);
        Span {
            source: 0,
            start,
            end,
        }
    }

    fn msg(&self, level: u8, text: &str, lineno: u32) -> Node {
        messages::system_message(level, text, lineno, self.source_path)
    }

    /// For state-machine-position-derived messages (see `line_bias`).
    fn msg_sm(&self, level: u8, text: &str, lineno: u32) -> Node {
        messages::system_message(level, text, lineno + self.line_bias, self.source_path)
    }

    /// Probe-verified: an explicit-markup element (comment/target) followed
    /// by an ADJACENT non-blank column-0 line that is not itself explicit
    /// markup warns. Consecutive `..`/`__ ` items chain without warning.
    fn warn_explicit_markup_end(&self, lines: &[LineRef<'a>], pos: usize, out: &mut Vec<Node>) {
        if let Some(l) = lines.get(pos) {
            let explicit_ish =
                l.text == ".." || l.text.starts_with(".. ") || l.text.starts_with("__ ");
            if !l.is_blank() && l.indent() == 0 && !explicit_ish {
                out.push(self.msg(
                    messages::WARNING,
                    "Explicit markup ends without a blank line; unexpected unindent.",
                    l.lineno,
                ));
            }
        }
    }

    // ------------------------------------------------------------------
    // document level (the only level where titles match)
    // ------------------------------------------------------------------

    pub(crate) fn parse_document(mut self) -> Node {
        let mut root = Node::elem(
            kinds::DOCUMENT,
            Span {
                source: 0,
                start: 0,
                end: self.source_len as u32,
            },
        );
        root.set("source", AttrValue::Str(self.source_path.to_string()));

        // Open sections, deepest last; nodes attach on close.
        let mut stack: Vec<Node> = Vec::new();
        let lines = std::mem::take(&mut self.top);
        let mut pos = 0usize;
        while pos < lines.len() {
            if lines[pos].is_blank() {
                pos += 1;
                continue;
            }
            let mut out = Vec::new();
            let section = self.parse_element(&lines, &mut pos, true, &mut out);
            for node in out {
                Self::container(&mut root, &mut stack).children.push(node);
            }
            if let Some(start) = section {
                self.open_section(start, &mut root, &mut stack);
            }
        }
        while !stack.is_empty() {
            Self::close_section(&mut root, &mut stack);
        }

        let fixups = self.registry.take_fixups();
        ids::apply_dupname_fixups(&mut root, &fixups);
        root
    }

    fn container<'r>(root: &'r mut Node, stack: &'r mut [Node]) -> &'r mut Node {
        match stack.last_mut() {
            Some(top) => top,
            None => root,
        }
    }

    fn close_section(root: &mut Node, stack: &mut Vec<Node>) {
        if let Some(mut done) = stack.pop() {
            if let Some(last) = done.children.last() {
                done.span.end = done.span.end.max(last.span.end);
            }
            Self::container(root, stack).children.push(done);
        }
    }

    fn open_section(&mut self, start: SectionStart, root: &mut Node, stack: &mut Vec<Node>) {
        let known = self.styles.iter().position(|s| *s == start.style);
        let level = match known {
            Some(i) => i + 1,
            None => self.styles.len() + 1,
        };
        if level > stack.len() + 1 {
            // Skipped level: ERROR, section dropped, content continues here.
            let text = format!(
                "Inconsistent title style: skip from level {} to {}.",
                stack.len(),
                level
            );
            let mut msg = self.msg(messages::ERROR, &text, start.title_lineno);
            msg = messages::with_literal(msg, &start.raw_lines);
            let established: Vec<String> = self
                .styles
                .iter()
                .map(|(c, over)| {
                    if *over {
                        format!("{c}/{c}")
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            msg = messages::with_paragraph(
                msg,
                &format!("Established title styles: {}", established.join(" ")),
            );
            Self::container(root, stack).children.push(msg);
            return;
        }
        if known.is_none() {
            self.styles.push(start.style);
        }
        while stack.len() >= level {
            Self::close_section(root, stack);
        }

        let inline = super::inline::parse_inline(
            &start.title,
            start.span,
            start.title_lineno,
            &mut self.registry,
            self.source_path,
        );
        let mut title = Node::elem(kinds::TITLE, start.span);
        title.children = inline.nodes;
        // Section name from the title's TEXT content (markup stripped).
        let mut section = Node::elem(kinds::SECTION, start.span);
        section
            .attrs
            .names
            .push(ids::fully_normalize_name(&title.astext()));
        let dup_info =
            self.registry
                .set_id_implicit(&mut section, start.underline_lineno, self.source_path);
        section.children.push(title);
        for m in start.messages {
            section.children.push(m);
        }
        for m in inline.messages {
            section.children.push(m);
        }
        if let Some(info) = dup_info {
            section.children.push(info);
        }
        stack.push(section);
    }

    // ------------------------------------------------------------------
    // element dispatch
    // ------------------------------------------------------------------

    fn parse_elements(&mut self, lines: &[LineRef<'a>]) -> Vec<Node> {
        if self.depth >= MAX_NEST_DEPTH {
            // sphinx-ultra-specific totality guard (docutils crashes here).
            let lineno = lines.first().map(|l| l.lineno).unwrap_or(1);
            return vec![self.msg(
                messages::ERROR,
                "Maximum nesting depth exceeded; deeper content skipped.",
                lineno,
            )];
        }
        self.depth += 1;
        let out = self.parse_elements_inner(lines);
        self.depth -= 1;
        out
    }

    fn parse_elements_inner(&mut self, lines: &[LineRef<'a>]) -> Vec<Node> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < lines.len() {
            if lines[pos].is_blank() {
                pos += 1;
                continue;
            }
            let section = self.parse_element(lines, &mut pos, false, &mut out);
            debug_assert!(section.is_none(), "titles never match in nested contexts");
        }
        out
    }

    /// Parse one element starting at `lines[*pos]` (non-blank). Returns a
    /// pending section start when `match_titles` and a title was found.
    fn parse_element(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        match_titles: bool,
        out: &mut Vec<Node>,
    ) -> Option<SectionStart> {
        let line = lines[*pos];
        if line.indent() > 0 {
            self.parse_block_quote(lines, pos, out);
            return None;
        }
        let text = line.text;

        if let Some(bullet) = Self::bullet_marker(text) {
            self.parse_bullet_list(lines, pos, bullet, out);
            return None;
        }
        if let Some(e) = parse_enumerator(text) {
            if self.try_enumerated_list(lines, pos, &e, out) {
                return None;
            }
            // invalid list start: fall through to the text path
        }
        if field_marker(text).is_some() {
            self.parse_field_list(lines, pos, out);
            return None;
        }
        if option_group_marker(text).is_some() && self.option_item_viable(lines, *pos) {
            self.parse_option_list(lines, pos, out);
            return None;
        }
        if text.starts_with(">>> ") || text == ">>>" {
            self.parse_doctest(lines, pos, out);
            return None;
        }
        if text == "|" || text.starts_with("| ") {
            self.parse_line_block(lines, pos, out);
            return None;
        }
        if is_grid_table_top(text) {
            self.parse_grid_table(lines, pos, out);
            return None;
        }
        if is_simple_table_top(text) {
            self.parse_simple_table(lines, pos, out);
            return None;
        }
        if text == ".." || text.starts_with(".. ") {
            self.parse_explicit(lines, pos, out);
            return None;
        }
        if let Some(rest) = text.strip_prefix("__ ") {
            self.parse_anonymous_shortcut(lines, pos, rest, out);
            return None;
        }
        if text == "__" {
            // Bare `__`: anonymous internal target (fixture-verified).
            self.parse_anonymous_shortcut(lines, pos, "", out);
            return None;
        }
        if let Some(c) = adornment_char(text) {
            return self.handle_adornment(lines, pos, c, match_titles, out);
        }
        self.handle_text(lines, pos, match_titles, out)
    }

    fn bullet_marker(text: &str) -> Option<char> {
        let mut chars = text.chars();
        let first = chars.next()?;
        if !BULLET_CHARS.contains(&first) {
            return None;
        }
        match chars.next() {
            None => Some(first),
            Some(' ') => Some(first),
            Some(_) => None,
        }
    }

    // ------------------------------------------------------------------
    // adornment lines ("line" state)
    // ------------------------------------------------------------------

    fn handle_adornment(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        ch: char,
        match_titles: bool,
        out: &mut Vec<Node>,
    ) -> Option<SectionStart> {
        let line = lines[*pos];
        let len = char_len(line.text);
        let next = lines.get(*pos + 1).copied();
        let next_is_text = next.map(|n| !n.is_blank()).unwrap_or(false);

        if !match_titles {
            if len >= 4 {
                let msg = messages::with_literal(
                    self.msg(
                        messages::ERROR,
                        "Unexpected section title or transition.",
                        line.lineno,
                    ),
                    line.text,
                );
                out.push(msg);
                *pos += 1;
            } else {
                // Fixture-verified: short adornments in nested contexts get
                // an INFO, then reprocess through the text state.
                out.push(self.msg(
                    messages::INFO,
                    "Unexpected possible title overline or transition.\nTreating it as ordinary text because it's so short.",
                    line.lineno,
                ));
                return self.handle_text(lines, pos, match_titles, out);
            }
            return None;
        }

        if !next_is_text {
            if len >= 4 {
                out.push(Node::elem(
                    kinds::TRANSITION,
                    self.span_of(lines, *pos, *pos),
                ));
                *pos += 1;
            } else {
                self.parse_paragraph_like(lines, pos, out);
            }
            return None;
        }

        // Overline candidacy: adornment, then a second line.
        let title_line = next.unwrap();
        if len < 4 {
            // Short overline: INFO, then reprocess through the text state
            // ("--\n--" becomes a section titled "--"; "---\n    x" becomes
            // a definition list).
            out.push(self.msg(
                messages::INFO,
                "Possible incomplete section title.\nTreating the overline as ordinary text because it's so short.",
                line.lineno,
            ));
            return self.handle_text(lines, pos, match_titles, out);
        }
        if adornment_char(title_line.text).is_some() {
            let msg = messages::with_literal(
                self.msg(
                    messages::ERROR,
                    "Invalid section title or transition marker.",
                    line.lineno,
                ),
                &format!("{}\n{}", line.text, title_line.text),
            );
            out.push(msg);
            *pos += 2;
            return None;
        }
        let under = lines.get(*pos + 2).copied();
        // Fixture-verified message split: at EOF the title is "incomplete";
        // with a blank or text third line the underline is "missing".
        let missing_underline = match under {
            None => Some(("Incomplete section title.", 2usize, false)),
            Some(u) if u.is_blank() => Some((
                "Missing matching underline for section title overline.",
                2,
                false,
            )),
            Some(u) if adornment_char(u.text).is_none() => Some((
                "Missing matching underline for section title overline.",
                3,
                true,
            )),
            _ => None,
        };
        if let Some((text, consume, third_in_literal)) = missing_underline {
            let literal = if third_in_literal {
                format!(
                    "{}\n{}\n{}",
                    line.text,
                    title_line.text,
                    lines[*pos + 2].text
                )
            } else {
                format!("{}\n{}", line.text, title_line.text)
            };
            let msg =
                messages::with_literal(self.msg(messages::ERROR, text, line.lineno), &literal);
            out.push(msg);
            *pos += consume;
            return None;
        }
        let under = under.unwrap();
        if adornment_char(under.text) != Some(ch) || char_len(under.text) != len {
            // Different char or different length: both are a mismatch.
            let msg = messages::with_literal(
                self.msg(
                    messages::ERROR,
                    "Title overline & underline mismatch.",
                    line.lineno,
                ),
                &format!("{}\n{}\n{}", line.text, title_line.text, under.text),
            );
            out.push(msg);
            *pos += 3;
            return None;
        }
        // Title column width (leading spaces included) wider than the
        // adornment: section is still created, WARNING inside.
        let mut msgs = Vec::new();
        if column_width(title_line.text) > len {
            msgs.push(messages::with_literal(
                self.msg(messages::WARNING, "Title overline too short.", line.lineno),
                &format!("{}\n{}\n{}", line.text, title_line.text, under.text),
            ));
        }
        let span = self.span_of(lines, *pos, *pos + 2);
        let raw = format!("{}\n{}\n{}", line.text, title_line.text, under.text);
        let title_lineno = title_line.lineno;
        let underline_lineno = under.lineno;
        let title_text = title_line.text.trim().to_string();
        *pos += 3;
        Some(SectionStart {
            title: title_text,
            style: (ch, true),
            raw_lines: raw,
            messages: msgs,
            title_lineno,
            underline_lineno,
            span,
        })
    }

    // ------------------------------------------------------------------
    // text state: underline titles, definition lists, paragraphs
    // ------------------------------------------------------------------

    fn handle_text(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        match_titles: bool,
        out: &mut Vec<Node>,
    ) -> Option<SectionStart> {
        let line = lines[*pos];
        let next = lines.get(*pos + 1).copied();

        if let Some(next) = next {
            if !next.is_blank() && next.indent() == 0 {
                if let Some(ch) = adornment_char(next.text) {
                    let title_len = column_width(line.text);
                    let ul_len = char_len(next.text);
                    if ul_len >= title_len || ul_len >= 4 {
                        if !match_titles {
                            let msg = messages::with_literal(
                                self.msg(messages::ERROR, "Unexpected section title.", next.lineno),
                                &format!("{}\n{}", line.text, next.text),
                            );
                            out.push(msg);
                            *pos += 2;
                            return None;
                        }
                        let mut msgs = Vec::new();
                        if ul_len < title_len {
                            msgs.push(messages::with_literal(
                                self.msg(
                                    messages::WARNING,
                                    "Title underline too short.",
                                    next.lineno,
                                ),
                                &format!("{}\n{}", line.text, next.text),
                            ));
                        }
                        let span = self.span_of(lines, *pos, *pos + 1);
                        let raw = format!("{}\n{}", line.text, next.text);
                        let title_lineno = line.lineno;
                        let underline_lineno = next.lineno;
                        let title = line.text.trim().to_string();
                        *pos += 2;
                        return Some(SectionStart {
                            title,
                            style: (ch, false),
                            raw_lines: raw,
                            messages: msgs,
                            title_lineno,
                            underline_lineno,
                            span,
                        });
                    }
                    if match_titles {
                        // Demoted: INFO, then the lines parse as a paragraph.
                        out.push(self.msg(
                            messages::INFO,
                            "Possible title underline, too short for the title.\nTreating it as ordinary text because it's so short.",
                            next.lineno,
                        ));
                    }
                    // fall through to paragraph (absorbs the underline line)
                }
            }
            if !next.is_blank() && next.indent() > 0 {
                // Single line + immediately indented block: definition list.
                self.parse_definition_list(lines, pos, out);
                return None;
            }
        }
        self.parse_paragraph_like(lines, pos, out);
        None
    }

    /// Paragraph: maximal run of adjacent column-0 non-blank lines, with
    /// docutils `::` literal-block chaining and the multi-line + indent
    /// "Unexpected indentation." recovery.
    fn parse_paragraph_like(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        out: &mut Vec<Node>,
    ) {
        let start = *pos;
        let mut end = *pos;
        while end < lines.len() && !lines[end].is_blank() && lines[end].indent() == 0 {
            end += 1;
        }
        let run: Vec<&str> = lines[start..end].iter().map(|l| l.text).collect();
        let joined = run.join("\n");
        let (text, expect_literal) = strip_literal_colons(&joined);
        let span = self.span_of(lines, start, end.saturating_sub(1));
        if !text.is_empty() {
            let result = super::inline::parse_inline(
                &text,
                span,
                lines[start].lineno,
                &mut self.registry,
                self.source_path,
            );
            let mut para = Node::elem(kinds::PARAGRAPH, span);
            para.children = result.nodes;
            out.push(para);
            out.extend(result.messages);
        }
        *pos = end;

        // Multi-line paragraph directly followed by an indented line.
        if end < lines.len()
            && !lines[end].is_blank()
            && lines[end].indent() > 0
            && end - start >= 2
        {
            out.push(self.msg_sm(
                messages::ERROR,
                "Unexpected indentation.",
                lines[end].lineno,
            ));
            // With a `::` trigger the indented block is STILL the literal
            // (fixture-verified); otherwise it becomes a block quote via the
            // ordinary element loop.
            if expect_literal {
                self.parse_literal_block(lines, pos, out);
            }
            return;
        }

        if expect_literal {
            self.parse_literal_block(lines, pos, out);
        }
    }

    fn parse_literal_block(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        let mut p = *pos;
        while p < lines.len() && lines[p].is_blank() {
            p += 1;
        }
        if p >= lines.len() {
            // Probe-verified: at EOF the warning still fires, anchored to
            // the line after the last one.
            let after_end = lines.last().map(|l| l.lineno + 1).unwrap_or(1);
            out.push(self.msg(
                messages::WARNING,
                "Literal block expected; none found.",
                after_end,
            ));
            *pos = p;
            return;
        }
        let first = lines[p];
        if first.indent() > 0 {
            // Indented literal block.
            let (block, consumed, _indent, terminator) = indented_block(lines, p);
            let text: Vec<&str> = block.iter().map(|l| l.text).collect();
            let span = self.span_of(lines, p, p + consumed - 1);
            let mut lb = Node::elem(kinds::LITERAL_BLOCK, span);
            lb.set("xml:space", AttrValue::Str("preserve".to_string()));
            lb.children.push(Node::text_node(text.join("\n"), span));
            out.push(lb);
            *pos = p + consumed;
            if let Some(term) = terminator {
                out.push(self.msg_sm(
                    messages::WARNING,
                    "Literal block ends without a blank line; unexpected unindent.",
                    term,
                ));
            }
            return;
        }
        let quote_char = first
            .text
            .chars()
            .next()
            .filter(|c| ADORNMENT_CHARS.contains(*c));
        if let Some(qc) = quote_char {
            // Quoted literal block: consistent same-char-prefixed run.
            let mut endq = p;
            while endq < lines.len()
                && !lines[endq].is_blank()
                && lines[endq].indent() == 0
                && lines[endq].text.starts_with(qc)
            {
                endq += 1;
            }
            let text: Vec<&str> = lines[p..endq].iter().map(|l| l.text).collect();
            let span = self.span_of(lines, p, endq - 1);
            let mut lb = Node::elem(kinds::LITERAL_BLOCK, span);
            lb.set("xml:space", AttrValue::Str("preserve".to_string()));
            lb.children.push(Node::text_node(text.join("\n"), span));
            out.push(lb);
            if endq < lines.len() && !lines[endq].is_blank() {
                let text = if lines[endq].indent() > 0 {
                    "Unexpected indentation."
                } else {
                    "Inconsistent literal block quoting."
                };
                out.push(self.msg(messages::ERROR, text, lines[endq].lineno));
            }
            *pos = endq;
            return;
        }
        out.push(self.msg(
            messages::WARNING,
            "Literal block expected; none found.",
            first.lineno,
        ));
        *pos = p;
    }

    // ------------------------------------------------------------------
    // lists
    // ------------------------------------------------------------------

    fn parse_bullet_list(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        bullet: char,
        out: &mut Vec<Node>,
    ) {
        let start = *pos;
        let mut list = Node::elem(kinds::BULLET_LIST, Span::ZERO);
        list.set("bullet", AttrValue::Str(bullet.to_string()));
        let mut warn_line: Option<u32> = None;
        loop {
            let item = self.parse_list_item(lines, pos, 1);
            list.children.push(item);
            let mut p = *pos;
            let mut saw_blank = false;
            while p < lines.len() && lines[p].is_blank() {
                p += 1;
                saw_blank = true;
            }
            if p >= lines.len() {
                *pos = p;
                break;
            }
            let line = lines[p];
            if line.indent() == 0 && Self::bullet_marker(line.text) == Some(bullet) {
                *pos = p;
                continue;
            }
            if !saw_blank {
                warn_line = Some(line.lineno);
            }
            *pos = p;
            break;
        }
        list.span = self.span_of(lines, start, pos.saturating_sub(1));
        out.push(list);
        if let Some(l) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Bullet list ends without a blank line; unexpected unindent.",
                l,
            ));
        }
    }

    /// Parse one list item whose marker occupies `marker_chars` characters on
    /// the current line. Content indent per docutils: marker + following
    /// spaces, or the next line's indent when the marker stands alone.
    /// Leaves `*pos` just past the item's content (trailing blank lines are
    /// left for the caller).
    fn parse_list_item(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        marker_chars: usize,
    ) -> Node {
        let marker_line = lines[*pos];
        let after = rest_after(marker_line.text, marker_chars);
        let spaces = after.len() - after.trim_start_matches(' ').len();
        let rest = &after[spaces..];
        let start = *pos;

        let mut body: Vec<LineRef<'a>> = Vec::new();
        let content_indent;
        if rest.is_empty() {
            // Fixture-verified: a bare marker's body may follow after blank
            // lines; the first indented line sets the content indent.
            let mut probe = start + 1;
            while probe < lines.len() && lines[probe].is_blank() {
                probe += 1;
            }
            match lines.get(probe) {
                Some(n) if !n.is_blank() && n.indent() > 0 => content_indent = n.indent(),
                _ => {
                    *pos = start + 1;
                    return Node::elem(kinds::LIST_ITEM, self.span_of(lines, start, start));
                }
            }
        } else {
            content_indent = marker_chars + spaces;
            body.push(LineRef::new(
                rest,
                marker_line.lineno,
                marker_line.src_start,
                marker_line.src_end,
            ));
        }

        let mut last_content = start;
        let mut pending_blanks: Vec<LineRef<'a>> = Vec::new();
        let mut scan = start + 1;
        while scan < lines.len() {
            let l = lines[scan];
            if l.is_blank() {
                pending_blanks.push(l);
                scan += 1;
                continue;
            }
            if l.indent() >= content_indent {
                body.append(&mut pending_blanks);
                body.push(l.dedented(content_indent));
                last_content = scan;
                scan += 1;
            } else {
                break;
            }
        }
        *pos = last_content + 1;

        let children = self.parse_elements(&body);
        let mut item = Node::elem(kinds::LIST_ITEM, self.span_of(lines, start, last_content));
        item.children = children;
        item
    }

    fn try_enumerated_list(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        first: &Enumerator,
        out: &mut Vec<Node>,
    ) -> bool {
        let mut candidates = initial_candidates(&first.literal, first.auto);
        if candidates.is_empty() {
            return false;
        }
        if !self.enum_item_valid(lines, *pos, first, &candidates, first.auto) {
            return false;
        }
        let start = *pos;
        let mut warn_line: Option<u32> = None;
        let mut items: Vec<Node> = Vec::new();
        let mut current = first.clone();
        // Fixture-verified: once an item is auto (#), explicit successors
        // invalidate; bare successors ("2." with no text) never continue.
        let mut auto_mode = first.auto;
        loop {
            let item = self.parse_list_item(lines, pos, current.marker_chars);
            items.push(item);
            let mut p = *pos;
            let mut saw_blank = false;
            while p < lines.len() && lines[p].is_blank() {
                p += 1;
                saw_blank = true;
            }
            if p >= lines.len() {
                *pos = p;
                break;
            }
            let line = lines[p];
            let mut accepted = false;
            if line.indent() == 0 {
                if let Some(e) = parse_enumerator(line.text) {
                    if e.prefix == first.prefix
                        && e.suffix == first.suffix
                        && !e.rest_empty
                        && !(auto_mode && !e.auto)
                    {
                        let narrowed = advance_candidates(&candidates, &e);
                        if !narrowed.is_empty()
                            && self.enum_item_valid(lines, p, &e, &narrowed, auto_mode || e.auto)
                        {
                            candidates = narrowed;
                            auto_mode |= e.auto;
                            current = e;
                            *pos = p;
                            accepted = true;
                        }
                    }
                }
            }
            if !accepted {
                if !saw_blank {
                    warn_line = Some(line.lineno);
                }
                *pos = p;
                break;
            }
        }

        let chosen = &candidates[0];
        let mut list = Node::elem(kinds::ENUMERATED_LIST, Span::ZERO);
        list.set("enumtype", AttrValue::Str(chosen.seq.to_string()));
        list.set("prefix", AttrValue::Str(first.prefix.to_string()));
        if chosen.initial != 1 {
            list.set("start", AttrValue::Int(chosen.initial as i64));
        }
        list.set("suffix", AttrValue::Str(first.suffix.to_string()));
        list.children = items;
        list.span = self.span_of(lines, start, pos.saturating_sub(1));
        let first_lineno = lines[start].lineno;
        out.push(list);
        if let Some(l) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Enumerated list ends without a blank line; unexpected unindent.",
                l,
            ));
        }
        if chosen.initial != 1 {
            out.push(self.msg(
                messages::INFO,
                &format!(
                    "Enumerated list start value not ordinal-1: \"{}\" (ordinal {})",
                    first.literal, chosen.initial
                ),
                first_lineno,
            ));
        }
        true
    }

    /// docutils validates an enumerated item by its OWN next line: blank,
    /// EOF, indented continuation, or a valid successor enumerator.
    fn enum_item_valid(
        &self,
        lines: &[LineRef<'a>],
        at: usize,
        item: &Enumerator,
        candidates: &[EnumCandidate],
        auto_context: bool,
    ) -> bool {
        let next = match lines.get(at + 1) {
            None => return true,
            Some(n) => n,
        };
        if next.is_blank() || next.indent() > 0 {
            return true;
        }
        match parse_enumerator(next.text) {
            Some(e)
                if e.prefix == item.prefix
                    && e.suffix == item.suffix
                    && !e.rest_empty
                    && !(auto_context && !e.auto) =>
            {
                !advance_candidates(candidates, &e).is_empty()
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------------
    // definition lists
    // ------------------------------------------------------------------

    fn parse_definition_list(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        out: &mut Vec<Node>,
    ) {
        let start = *pos;
        let mut dl = Node::elem(kinds::DEFINITION_LIST, Span::ZERO);
        let mut warn_line: Option<u32> = None;
        loop {
            let term_line = lines[*pos];
            let (block, consumed, _indent, terminator) = indented_block(lines, *pos + 1);
            let item_last = *pos + consumed;
            let mut item = Node::elem(
                kinds::DEFINITION_LIST_ITEM,
                self.span_of(lines, *pos, item_last),
            );
            let term_span = self.span_of(lines, *pos, *pos);
            let mut parts = split_classifiers(term_line.text).into_iter();
            let term_text = parts.next().unwrap_or_default();
            let mut term_msgs = Vec::new();
            let inline = super::inline::parse_inline(
                &term_text,
                term_span,
                term_line.lineno,
                &mut self.registry,
                self.source_path,
            );
            let mut term = Node::elem(kinds::TERM, term_span);
            term.children = inline.nodes;
            term_msgs.extend(inline.messages);
            item.children.push(term);
            for classifier in parts {
                let inline = super::inline::parse_inline(
                    &classifier,
                    term_span,
                    term_line.lineno,
                    &mut self.registry,
                    self.source_path,
                );
                let mut c = Node::elem(kinds::CLASSIFIER, term_span);
                c.children = inline.nodes;
                term_msgs.extend(inline.messages);
                item.children.push(c);
            }
            let mut definition =
                Node::elem(kinds::DEFINITION, self.span_of(lines, *pos + 1, item_last));
            // Fixture-verified: term/classifier inline messages land INSIDE
            // the definition, before its content.
            definition.children.append(&mut term_msgs);
            if term_line.text.ends_with("::") {
                // Probe-verified: docutils flags a term ending in `::`.
                definition.children.push(self.msg(
                    messages::INFO,
                    "Blank line missing before literal block (after the \"::\")? Interpreted as a definition list item.",
                    term_line.lineno + 1,
                ));
            }
            definition.children.extend(self.parse_elements(&block));
            item.children.push(definition);
            dl.children.push(item);
            *pos += 1 + consumed;

            // Another term? (column-0 text line + immediately indented body)
            let mut p = *pos;
            while p < lines.len() && lines[p].is_blank() {
                p += 1;
            }
            let continues = p < lines.len() && {
                let l = lines[p];
                let nxt = lines.get(p + 1);
                l.indent() == 0
                    && !l.is_blank()
                    && Self::bullet_marker(l.text).is_none()
                    && parse_enumerator(l.text).is_none()
                    && adornment_char(l.text).is_none()
                    && field_marker(l.text).is_none()
                    && option_group_marker(l.text).is_none()
                    && !l.text.starts_with(".. ")
                    && l.text != ".."
                    && !l.text.starts_with("| ")
                    && !l.text.starts_with(">>> ")
                    && !l.text.starts_with("__ ")
                    && nxt
                        .map(|n| !n.is_blank() && n.indent() > 0)
                        .unwrap_or(false)
            };
            if continues {
                *pos = p;
                continue;
            }
            if let Some(t) = terminator {
                warn_line = Some(t);
            }
            break;
        }
        dl.span = self.span_of(lines, start, pos.saturating_sub(1));
        out.push(dl);
        if let Some(l) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Definition list ends without a blank line; unexpected unindent.",
                l,
            ));
        }
    }

    // ------------------------------------------------------------------
    // block quotes
    // ------------------------------------------------------------------

    fn parse_block_quote(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let (block, consumed, _indent, terminator) = indented_block(lines, *pos);
        *pos = start + consumed;
        let span = self.span_of(lines, start, start + consumed - 1);

        // Split into blank-separated chunks; attribution chunks close quotes.
        let mut quotes: Vec<QuoteSegment<'a>> = Vec::new();
        let mut acc: Vec<LineRef<'a>> = Vec::new();
        let mut i = 0usize;
        while i < block.len() {
            if block[i].is_blank() {
                acc.push(block[i]);
                i += 1;
                continue;
            }
            let chunk_start = i;
            while i < block.len() && !block[i].is_blank() {
                i += 1;
            }
            let chunk = &block[chunk_start..i];
            // Probe-verified: an attribution needs preceding quote body —
            // a quote whose only content is "-- x" is a plain paragraph.
            let has_body = acc.iter().any(|l| !l.is_blank());
            match attribution_from_chunk(chunk, span) {
                Some(attr) if has_body => quotes.push((std::mem::take(&mut acc), Some(attr))),
                _ => acc.extend_from_slice(chunk),
            }
        }
        if !acc.iter().all(|l| l.is_blank()) || quotes.is_empty() {
            quotes.push((acc, None));
        }
        for (body, attribution) in quotes {
            let mut quote = Node::elem(kinds::BLOCK_QUOTE, span);
            quote.children = self.parse_elements(&body);
            let mut attr_messages = Vec::new();
            if let Some((raw_attr, lineno)) = attribution {
                let raw = raw_attr.astext();
                let inline = super::inline::parse_inline(
                    &raw,
                    raw_attr.span,
                    lineno,
                    &mut self.registry,
                    self.source_path,
                );
                let mut a = Node::elem(kinds::ATTRIBUTION, raw_attr.span);
                a.children = inline.nodes;
                attr_messages = inline.messages;
                quote.children.push(a);
            }
            if quote.children.is_empty() {
                continue;
            }
            out.push(quote);
            out.append(&mut attr_messages);
        }
        if let Some(t) = terminator {
            out.push(self.msg_sm(
                messages::WARNING,
                "Block quote ends without a blank line; unexpected unindent.",
                t,
            ));
        }
    }

    // ------------------------------------------------------------------
    // doctest + line blocks
    // ------------------------------------------------------------------

    fn parse_doctest(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        // Fixture-verified: a doctest block runs to the next BLANK line,
        // absorbing indented continuation/output lines verbatim.
        let start = *pos;
        let mut end = *pos;
        while end < lines.len() && !lines[end].is_blank() {
            end += 1;
        }
        let text: Vec<&str> = lines[start..end].iter().map(|l| l.text).collect();
        let span = self.span_of(lines, start, end - 1);
        let mut dt = Node::elem(kinds::DOCTEST_BLOCK, span);
        dt.set("xml:space", AttrValue::Str("preserve".to_string()));
        dt.children.push(Node::text_node(text.join("\n"), span));
        out.push(dt);
        *pos = end;
    }

    fn parse_line_block(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        // (depth, text): depth None on bare `|` lines inherits the previous
        // line's depth (fixture-verified). Continuations dedent by the FIRST
        // continuation line's indent, preserving deeper relative indents.
        let mut items: Vec<(Option<usize>, String)> = Vec::new();
        let mut cont_dedent: Option<usize> = None;
        let mut p = *pos;
        while p < lines.len() && !lines[p].is_blank() {
            let l = lines[p];
            if l.indent() == 0 && (l.text == "|" || l.text.starts_with("| ")) {
                cont_dedent = None;
                if l.text == "|" {
                    items.push((None, String::new()));
                } else {
                    let content = &l.text[2..];
                    let depth = content.len() - content.trim_start_matches(' ').len();
                    items.push((Some(depth), content[depth..].to_string()));
                }
                p += 1;
            } else if l.indent() > 0 && !items.is_empty() {
                let dedent = *cont_dedent.get_or_insert(l.indent());
                let dedent = dedent.min(l.indent());
                if let Some(last) = items.last_mut() {
                    if !last.1.is_empty() {
                        last.1.push('\n');
                    }
                    last.1.push_str(&l.text[dedent..]);
                }
                p += 1;
            } else {
                break;
            }
        }
        // Resolve inherited depths and inline-parse each line's text.
        let span = self.span_of(lines, start, p - 1);
        let first_lineno = lines[start].lineno;
        let mut resolved: Vec<(usize, Vec<Node>)> = Vec::with_capacity(items.len());
        let mut lb_messages: Vec<Node> = Vec::new();
        let mut prev_depth = 0usize;
        for (depth, text) in items {
            let d = depth.unwrap_or(prev_depth);
            prev_depth = d;
            if text.is_empty() {
                resolved.push((d, Vec::new()));
            } else {
                let inline = super::inline::parse_inline(
                    &text,
                    span,
                    first_lineno,
                    &mut self.registry,
                    self.source_path,
                );
                lb_messages.extend(inline.messages);
                resolved.push((d, inline.nodes));
            }
        }
        out.push(build_line_block(&mut resolved, span, 0));
        out.append(&mut lb_messages);
        // Fixture-verified: warning anchored to the LAST line-block line.
        if p < lines.len() && !lines[p].is_blank() {
            out.push(self.msg_sm(
                messages::WARNING,
                "Line block ends without a blank line.",
                lines[p - 1].lineno,
            ));
        }
        *pos = p;
    }

    // ------------------------------------------------------------------
    // explicit markup: comments + targets
    // ------------------------------------------------------------------

    fn parse_explicit(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        let line = lines[*pos];
        // docutils consumes ALL whitespace after `..` (fixture-verified for
        // multi-space forms).
        let rest = if line.text == ".." {
            ""
        } else {
            line.text[2..].trim_start()
        };

        if rest.starts_with('[') {
            if let Some(next_pos) = self.try_footnote_def(lines, pos, rest, out) {
                *pos = next_pos;
                self.warn_explicit_markup_end(lines, *pos, out);
                return;
            }
        }
        if rest.starts_with('_') {
            // Target attempt: the marker (name + link) may span ADJACENT
            // indented continuation lines; parse the joined form.
            let start = *pos;
            let lineno = line.lineno;
            let mut consumed = 0usize;
            while lines
                .get(start + 1 + consumed)
                .map(|l| !l.is_blank() && l.indent() > 0)
                .unwrap_or(false)
            {
                consumed += 1;
            }
            let cont: Vec<&str> = lines[start + 1..start + 1 + consumed]
                .iter()
                .map(|l| l.text.trim())
                .collect();
            let joined = if cont.is_empty() {
                rest.to_string()
            } else {
                format!("{}\n{}", rest, cont.join("\n"))
            };
            *pos = start + 1 + consumed;
            let span = self.span_of(lines, start, start + consumed);
            match parse_target_marker(&joined) {
                Some(marker) => {
                    let mut target = Node::elem(kinds::TARGET, span);
                    let mut internal = false;
                    let mut refuri_val: Option<String> = None;
                    if marker.anonymous {
                        target.set("anonymous", AttrValue::Int(1));
                    } else {
                        target
                            .attrs
                            .names
                            .push(ids::fully_normalize_name(&marker.name));
                    }
                    if marker.link.is_empty() {
                        internal = true;
                    } else if let Some(refname) = reference_name_from_link(&marker.link) {
                        target.set("refname", AttrValue::Str(refname));
                    } else {
                        let uri: String = marker
                            .link
                            .chars()
                            .filter(|c| !c.is_whitespace() && *c != '\\')
                            .collect();
                        refuri_val = Some(uri.clone());
                        target.set("refuri", AttrValue::Str(uri));
                    }
                    let msg = if marker.anonymous {
                        self.registry.set_id_anonymous(&mut target);
                        None
                    } else {
                        self.registry.set_id_explicit(
                            &mut target,
                            lineno,
                            self.source_path,
                            internal,
                            refuri_val.as_deref(),
                        )
                    };
                    if let Some(m) = msg {
                        out.push(m);
                    }
                    out.push(target);
                }
                None => {
                    // Malformed target: comment + WARNING (fixture-verified).
                    let mut text_lines: Vec<String> = vec![rest.to_string()];
                    text_lines.extend(cont.iter().map(|s| s.to_string()));
                    let mut comment = Node::elem(kinds::COMMENT, span);
                    comment.set("xml:space", AttrValue::Str("preserve".to_string()));
                    comment
                        .children
                        .push(Node::text_node(text_lines.join("\n"), span));
                    out.push(comment);
                    out.push(self.msg(messages::WARNING, "malformed hyperlink target.", lineno));
                }
            }
            self.warn_explicit_markup_end(lines, *pos, out);
            return;
        }

        if let Some((name, first_rest)) = directive_marker(rest) {
            self.parse_directive(lines, pos, &name, first_rest, out);
            self.warn_explicit_markup_end(lines, *pos, out);
            return;
        }

        // Comment. Probe-verified continuation rules: a comment with first-
        // line text absorbs the following indented block THROUGH internal
        // blank lines; a bare `..` takes a body only when the indented block
        // is ADJACENT (`..` + blank + indent leaves an empty comment and a
        // block quote).
        let start = *pos;
        let adjacent_body = lines
            .get(start + 1)
            .map(|l| !l.is_blank() && l.indent() > 0)
            .unwrap_or(false);
        let consume_block = !rest.is_empty() || adjacent_body;
        let (block, consumed) = if consume_block {
            let (block, consumed, _indent, _terminator) = indented_block(lines, start + 1);
            (block, consumed)
        } else {
            (Vec::new(), 0)
        };
        *pos = start + 1 + consumed;
        let span = self.span_of(lines, start, start + consumed);
        let mut text_lines: Vec<String> = Vec::new();
        if !rest.is_empty() {
            text_lines.push(rest.to_string());
        }
        let mut body: &[LineRef<'a>] = &block;
        if rest.is_empty() {
            while body.first().map(|l| l.is_blank()).unwrap_or(false) {
                body = &body[1..];
            }
        }
        for l in body {
            text_lines.push(l.text.to_string());
        }
        let mut comment = Node::elem(kinds::COMMENT, span);
        comment.set("xml:space", AttrValue::Str("preserve".to_string()));
        if !text_lines.is_empty() {
            comment
                .children
                .push(Node::text_node(text_lines.join("\n"), span));
        }
        out.push(comment);
        self.warn_explicit_markup_end(lines, *pos, out);
    }

    /// `.. [label]` footnote and citation definitions. Returns the new
    /// position past the construct, or None when `rest` is not a valid
    /// footnote/citation marker (falls through to comment).
    fn try_footnote_def(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        rest: &str,
        out: &mut Vec<Node>,
    ) -> Option<usize> {
        let chars: Vec<char> = rest.chars().collect();
        let mut j = 1usize; // past '['
        let label_start = j;
        match chars.get(j) {
            Some('#') => {
                j += 1;
                if let Some(len) = match_simplename_chars(&chars, j) {
                    j += len;
                }
            }
            Some('*') => j += 1,
            _ => j += match_simplename_chars(&chars, j)?,
        }
        if chars.get(j) != Some(&']') {
            return None;
        }
        let after = j + 1;
        if !(chars.len() == after || chars.get(after) == Some(&' ')) {
            return None;
        }
        let label: String = chars[label_start..j].iter().collect();
        let start = *pos;
        let lineno = lines[start].lineno;

        // Body: first-line remainder + following indented block (blanks
        // between marker and block allowed; docutils get_first_known_indented).
        // docutils' footnote pattern consumes ALL whitespace after `]`.
        let mut rest_from = after;
        while chars.get(rest_from) == Some(&' ') {
            rest_from += 1;
        }
        let first_rest: String = if rest_from > after {
            chars
                .get(rest_from..)
                .map(|c| c.iter().collect())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let (block, consumed, _indent, _term) = indented_block(lines, start + 1);
        let mut body: Vec<LineRef<'a>> = Vec::new();
        if !first_rest.trim().is_empty() {
            // remainder starts at a virtual column; treat as its own line
            body.push(LineRef::new(
                rest_after(
                    lines[start].text,
                    lines[start].text.chars().count() - first_rest.chars().count(),
                ),
                lineno,
                lines[start].src_start,
                lines[start].src_end,
            ));
        }
        for l in &block {
            body.push(*l);
        }

        let is_citation =
            !label.starts_with('#') && label != "*" && !label.chars().all(|c| c.is_ascii_digit());
        let kind = if is_citation {
            kinds::CITATION
        } else {
            kinds::FOOTNOTE
        };
        let span = self.span_of(lines, start, start + consumed);
        let mut node = Node::elem(kind, span);
        let mut has_label_child = false;
        if is_citation {
            node.attrs.names.push(ids::fully_normalize_name(&label));
            has_label_child = true;
        } else if label == "*" {
            node.set("auto", AttrValue::Str("*".to_string()));
        } else if let Some(rest_label) = label.strip_prefix('#') {
            node.set("auto", AttrValue::Int(1));
            if !rest_label.is_empty() {
                node.attrs.names.push(ids::fully_normalize_name(rest_label));
            }
        } else {
            node.attrs.names.push(ids::fully_normalize_name(&label));
            has_label_child = true;
        }
        let msg = if node.attrs.names.is_empty() {
            self.registry.set_id_anonymous(&mut node);
            None
        } else {
            self.registry
                .set_id_explicit(&mut node, lineno, self.source_path, true, None)
        };
        if has_label_child {
            let mut lab = Node::elem(kinds::LABEL, span);
            lab.children.push(Node::text_node(label.clone(), span));
            node.children.push(lab);
        }
        if let Some(m) = msg {
            node.children.push(m);
        }
        let content = self.parse_elements(&body);
        if content.is_empty() {
            let text = if is_citation {
                "Citation content expected."
            } else {
                "Footnote content expected."
            };
            node.children
                .push(self.msg(messages::WARNING, text, lineno));
        } else {
            node.children.extend(content);
        }
        out.push(node);
        Some(start + 1 + consumed)
    }

    /// Field lists: `:name: value` markers (probe-verified regex port).
    fn parse_field_list(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let mut fl = Node::elem(kinds::FIELD_LIST, Span::ZERO);
        let mut warn_line: Option<u32> = None;
        loop {
            let line = lines[*pos];
            let (name_raw, body_start) = field_marker(line.text).expect("checked by caller");
            let lineno = line.lineno;
            let field_span = self.span_of(lines, *pos, *pos);
            // body: marker-line remainder + any-indent continuation block
            let first_rest = line.text[body_start..].trim_start();
            let (block, consumed, _i, terminator) = indented_block(lines, *pos + 1);
            let mut body_lines: Vec<LineRef<'a>> = Vec::new();
            if !first_rest.is_empty() {
                let offset = line.text.len() - first_rest.len();
                body_lines.push(LineRef::new(
                    &line.text[offset..],
                    lineno,
                    line.src_start,
                    line.src_end,
                ));
            }
            body_lines.extend(block.iter().copied());
            *pos += 1 + consumed;

            let name_inline = super::inline::parse_inline(
                &name_raw,
                field_span,
                lineno,
                &mut self.registry,
                self.source_path,
            );
            let mut field = Node::elem(kinds::FIELD, field_span);
            let mut fname = Node::elem(kinds::FIELD_NAME, field_span);
            fname.children = name_inline.nodes;
            field.children.push(fname);
            let mut fbody = Node::elem(kinds::FIELD_BODY, field_span);
            fbody.children.extend(name_inline.messages);
            fbody.children.extend(self.parse_elements(&body_lines));
            field.children.push(fbody);
            fl.children.push(field);

            // continue on the next field marker (blanks allowed between)
            let mut p = *pos;
            while p < lines.len() && lines[p].is_blank() {
                p += 1;
            }
            let continues =
                p < lines.len() && lines[p].indent() == 0 && field_marker(lines[p].text).is_some();
            if continues {
                *pos = p;
                continue;
            }
            let _ = terminator;
            // Adjacency: any non-blank line directly after the field body
            // (indented-block terminator OR a col-0 line) warns.
            if let Some(l) = lines.get(*pos) {
                if !l.is_blank() {
                    warn_line = Some(l.lineno);
                }
            }
            break;
        }
        fl.span = self.span_of(lines, start, pos.saturating_sub(1));
        out.push(fl);
        if let Some(l) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Field list ends without a blank line; unexpected unindent.",
                l,
            ));
        }
    }

    /// An option marker line is only a list item when it has a two-space
    /// description or an indented following line (else: paragraph).
    fn option_item_viable(&self, lines: &[LineRef<'a>], at: usize) -> bool {
        let (_, desc) = match option_group_marker(lines[at].text) {
            Some(r) => r,
            None => return false,
        };
        if !desc.is_empty() {
            return true;
        }
        lines
            .get(at + 1)
            .map(|l| !l.is_blank() && l.indent() > 0)
            .unwrap_or(false)
    }

    fn parse_option_list(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let mut ol = Node::elem(kinds::OPTION_LIST, Span::ZERO);
        let mut warn_line: Option<u32> = None;
        loop {
            let line = lines[*pos];
            let (specs, desc) = option_group_marker(line.text).expect("checked by caller");
            let span = self.span_of(lines, *pos, *pos);
            let (block, consumed, _i, terminator) = indented_block(lines, *pos + 1);
            let mut body_lines: Vec<LineRef<'a>> = Vec::new();
            if !desc.is_empty() {
                let offset = line.text.len() - desc.len();
                body_lines.push(LineRef::new(
                    &line.text[offset..],
                    line.lineno,
                    line.src_start,
                    line.src_end,
                ));
            }
            body_lines.extend(block.iter().copied());
            *pos += 1 + consumed;

            let mut item = Node::elem(kinds::OPTION_LIST_ITEM, span);
            let mut group = Node::elem(kinds::OPTION_GROUP, span);
            for (opt_string, arg) in specs {
                let mut opt = Node::elem(kinds::OPTION, span);
                let mut os = Node::elem(kinds::OPTION_STRING, span);
                os.children.push(Node::text_node(opt_string, span));
                opt.children.push(os);
                if let Some((delim, argtext)) = arg {
                    let mut oa = Node::elem(kinds::OPTION_ARGUMENT, span);
                    oa.set("delimiter", AttrValue::Str(delim));
                    oa.children.push(Node::text_node(argtext, span));
                    opt.children.push(oa);
                }
                group.children.push(opt);
            }
            item.children.push(group);
            let mut description = Node::elem(kinds::DESCRIPTION, span);
            description.children = self.parse_elements(&body_lines);
            item.children.push(description);
            ol.children.push(item);

            let mut p = *pos;
            while p < lines.len() && lines[p].is_blank() {
                p += 1;
            }
            let continues = p < lines.len()
                && lines[p].indent() == 0
                && option_group_marker(lines[p].text).is_some()
                && self.option_item_viable(lines, p);
            if continues {
                *pos = p;
                continue;
            }
            let _ = terminator;
            if let Some(l) = lines.get(*pos) {
                if !l.is_blank() {
                    warn_line = Some(l.lineno);
                }
            }
            break;
        }
        ol.span = self.span_of(lines, start, pos.saturating_sub(1));
        out.push(ol);
        if let Some(l) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Option list ends without a blank line; unexpected unindent.",
                l,
            ));
        }
    }

    // ------------------------------------------------------------------
    // grid tables (docutils tableparser.GridTableParser port)
    // ------------------------------------------------------------------

    fn parse_grid_table(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        // isolate: consume until blank line
        let mut end = *pos;
        while end < lines.len() && !lines[end].is_blank() {
            end += 1;
        }
        let mut block: Vec<LineRef<'a>> = lines[start..end].to_vec();
        *pos = end;
        // docutils left-edge check: trim at the first line not starting
        // with '+' or '|'; the remainder re-parses and a blank-line
        // warning fires. The trim index feeds the stale-line quirk of the
        // bottom-corrupt error.
        let mut trailing_warning = None;
        let mut stale_i = block.len() - 1;
        for (i, l) in block.iter().enumerate().skip(1) {
            let t = l.text.trim_end();
            if !(t.starts_with('+') || t.starts_with('|')) {
                stale_i = i;
                trailing_warning = Some(self.msg(
                    messages::WARNING,
                    "Blank line required after table.",
                    l.lineno,
                ));
                block.truncate(i);
                *pos = start + i;
                break;
            }
        }
        // docutils trims a non-border tail back to the LAST valid border
        // (the remainder re-parses, with a blank-line-required warning),
        // BEFORE any alignment checks.
        if !is_grid_table_top(block[block.len() - 1].text.trim_end()) {
            let mut found = None;
            for i in (2..block.len() - 1).rev() {
                if is_grid_table_top(block[i].text.trim_end()) {
                    found = Some(i);
                    break;
                }
            }
            if let Some(i) = found {
                let next_lineno = block[i + 1].lineno;
                block.truncate(i + 1);
                *pos = start + i + 1;
                if trailing_warning.is_none() {
                    trailing_warning = Some(self.msg(
                        messages::WARNING,
                        "Blank line required after table.",
                        next_lineno,
                    ));
                }
            }
        }
        let raw_block: Vec<String> = block.iter().map(|l| l.text.to_string()).collect();
        let malformed = |detail: &str, lineno: u32| -> Node {
            messages::with_literal(
                messages::system_message(
                    messages::ERROR,
                    &format!("Malformed table.\n{detail}"),
                    lineno,
                    self.source_path,
                ),
                raw_block.join("\n").trim_end(),
            )
        };
        // right-border alignment (DISPLAY columns: east-asian wide = 2)
        let width = column_width(block[0].text.trim_end());
        for l in block.iter().skip(1) {
            let t = l.text.trim_end();
            if column_width(t) != width || !(t.ends_with('+') || t.ends_with('|')) {
                out.push(malformed("Right border not aligned or missing.", l.lineno));
                if let Some(w) = trailing_warning {
                    out.push(w);
                }
                return;
            }
        }
        // bottom border must be a grid border (line anchor reproduces
        // docutils' stale-index quirk: the last line the edge scans reached)
        if !is_grid_table_top(block[block.len() - 1].text.trim_end()) {
            let lineno = lines[(start + stale_i).min(lines.len() - 1)].lineno;
            out.push(malformed("Bottom border missing or corrupt.", lineno));
            if let Some(w) = trailing_warning {
                out.push(w);
            }
            return;
        }

        // grid as DISPLAY-column matrix (wide chars followed by a filler;
        // head/body sep '=' converted to '-')
        let mut grid: Vec<Vec<char>> = block
            .iter()
            .map(|l| {
                let mut row = Vec::new();
                for c in l.text.trim_end().chars() {
                    row.push(c);
                    if unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) == 2 {
                        row.push('\u{fffd}');
                    }
                }
                row
            })
            .collect();
        let mut head_sep: Option<usize> = None;
        for (i, row) in grid.iter_mut().enumerate() {
            let s: String = row.iter().collect();
            if is_grid_head_sep(&s) {
                if let Some(first) = head_sep {
                    out.push(malformed(
                        &format!(
                            "Multiple head/body row separators (table lines {} and {}); only one allowed.",
                            first + 1,
                            i + 1
                        ),
                        block[0].lineno,
                    ));
                    return;
                }
                head_sep = Some(i);
                for c in row.iter_mut() {
                    if *c == '=' {
                        *c = '-';
                    }
                }
            }
        }
        let nrows = grid.len();
        let at = |r: usize, c: usize| -> char {
            *grid.get(r).and_then(|row| row.get(c)).unwrap_or(&' ')
        };

        // trace cells from top-left corners
        let mut cells: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut colseps: Vec<usize> = vec![0];
        let mut rowseps: Vec<usize> = vec![0];
        let mut corners: Vec<(usize, usize)> = vec![(0, 0)];
        let mut done_to: Vec<(usize, usize)> = Vec::new(); // (left, bottom) per traced cell
        while let Some((top, left)) = corners.pop() {
            if cells
                .iter()
                .any(|(t, l, b, r)| *t <= top && top < *b && *l <= left && left < *r)
            {
                continue;
            }
            if at(top, left) != '+' {
                continue;
            }
            if let Some((bottom, right, mut cseps, mut rseps)) = trace_cell(&grid, top, left) {
                cells.push((top, left, bottom, right));
                colseps.append(&mut cseps);
                rowseps.append(&mut rseps);
                corners.push((top, right));
                corners.push((bottom, left));
                done_to.push((left, bottom));
                corners.sort();
                corners.dedup();
            }
        }
        colseps.sort_unstable();
        colseps.dedup();
        rowseps.sort_unstable();
        rowseps.dedup();

        // completeness: every column spanned to the bottom
        let bottom_row = nrows - 1;
        if rowseps.last() != Some(&bottom_row) && !cells.is_empty() {
            out.push(malformed(
                "Malformed table; parse incomplete.",
                block[0].lineno,
            ));
            return;
        }
        let _ = done_to;

        // structure
        let ncols = colseps.len().saturating_sub(1);
        let colwidths: Vec<usize> = colseps.windows(2).map(|w| w[1] - w[0] - 1).collect();
        let row_of = |o: usize| rowseps.iter().position(|r| *r == o);
        let col_of = |o: usize| colseps.iter().position(|c| *c == o);
        let nrows_struct = rowseps.len().saturating_sub(1);
        // rows[r][c] = Option<entry>
        let mut entries: Vec<Vec<Option<Node>>> = vec![];
        for _ in 0..nrows_struct {
            entries.push((0..ncols).map(|_| None).collect());
        }
        let mut covered: Vec<Vec<bool>> = vec![vec![false; ncols]; nrows_struct];
        let mut cell_list = cells.clone();
        cell_list.sort();
        for (top, left, bottom, right) in cell_list {
            let (Some(rn), Some(cn), Some(rb), Some(cr)) =
                (row_of(top), col_of(left), row_of(bottom), col_of(right))
            else {
                continue;
            };
            if covered[rn][cn] {
                continue;
            }
            let morerows = rb - rn - 1;
            let morecols = cr - cn - 1;
            for row in covered.iter_mut().take(rb).skip(rn) {
                for cell in row.iter_mut().take(cr).skip(cn) {
                    *cell = true;
                }
            }
            let span = self.span_of(lines, start + top, start + bottom);
            let mut entry = Node::elem(kinds::ENTRY, span);
            if morecols > 0 {
                entry.set("morecols", AttrValue::Int(morecols as i64));
            }
            if morerows > 0 {
                entry.set("morerows", AttrValue::Int(morerows as i64));
            }
            // cell block: rows top+1..bottom, cols left+1..right
            let mut cell_lines: Vec<LineRef<'a>> = Vec::new();
            for l in block.iter().take(bottom).skip(top + 1) {
                let text = display_slice(l.text, left + 1, right);
                cell_lines.push(LineRef::new(text, l.lineno, l.src_start, l.src_end));
            }
            let base = cell_lines
                .iter()
                .filter(|l| !l.text.trim().is_empty())
                .map(|l| l.indent())
                .min()
                .unwrap_or(0);
            let dedented: Vec<LineRef<'a>> = cell_lines
                .iter()
                .map(|l| {
                    if l.text.trim().is_empty() {
                        LineRef::new("", l.lineno, l.src_start, l.src_end)
                    } else {
                        let mut d = l.dedented(base);
                        // strip trailing whitespace inside the cell view
                        d.text = d.text.trim_end();
                        d
                    }
                })
                .collect();
            if dedented.iter().any(|l| !l.text.is_empty()) {
                self.line_bias += 1;
                entry.children = self.parse_elements(&dedented);
                self.line_bias -= 1;
            }
            entries[rn][cn] = Some(entry);
        }

        let table_span = self.span_of(lines, start, end.saturating_sub(1));
        let mut table = Node::elem(kinds::TABLE, table_span);
        let mut tgroup = Node::elem(kinds::TGROUP, table_span);
        tgroup.set("cols", AttrValue::Int(ncols as i64));
        for w in &colwidths {
            let mut cs = Node::elem(kinds::COLSPEC, table_span);
            cs.set("colwidth", AttrValue::Int(*w as i64));
            tgroup.children.push(cs);
        }
        let head_rows = head_sep.and_then(row_of).unwrap_or(0);
        let build_rows = |range: std::ops::Range<usize>, entries: &mut Vec<Vec<Option<Node>>>| {
            let mut rows = Vec::new();
            for r in range {
                let mut row = Node::elem(kinds::ROW, table_span);
                for slot in entries[r].iter_mut() {
                    if let Some(e) = slot.take() {
                        row.children.push(e);
                    }
                }
                rows.push(row);
            }
            rows
        };
        if head_sep.is_some() && head_rows > 0 {
            let mut thead = Node::elem(kinds::THEAD, table_span);
            thead.children = build_rows(0..head_rows, &mut entries);
            tgroup.children.push(thead);
        } else if head_sep.is_some() {
            let mut thead = Node::elem(kinds::THEAD, table_span);
            thead.children = build_rows(0..0, &mut entries);
            let _ = &mut thead;
            tgroup.children.push(thead);
        }
        let mut tbody = Node::elem(kinds::TBODY, table_span);
        tbody.children = build_rows(head_rows..nrows_struct, &mut entries);
        tgroup.children.push(tbody);
        table.children.push(tgroup);
        out.push(table);
        if let Some(w) = trailing_warning {
            out.push(w);
        }
    }

    fn parse_simple_table(&mut self, lines: &[LineRef<'a>], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let toplen = char_len(lines[start].text.trim_end());
        // isolate: find border candidates (=-runs line, same stripped length)
        let mut found = 0usize;
        let mut found_at = None;
        let mut end = None;
        let mut i = start + 1;
        while i < lines.len() {
            let t = lines[i].text.trim_end();
            if is_simple_table_border(t) {
                if char_len(t) != toplen {
                    let raw: Vec<String> = lines[start..=i]
                        .iter()
                        .map(|l| l.text.to_string())
                        .collect();
                    out.push(messages::with_literal(
                        self.msg(
                            messages::ERROR,
                            "Malformed table.\nBottom border or header rule does not match top border.",
                            lines[i].lineno,
                        ),
                        raw.join("\n").trim_end(),
                    ));
                    *pos = i + 1;
                    return;
                }
                found += 1;
                found_at = Some(i);
                if found == 2
                    || i + 1 >= lines.len()
                    || lines.get(i + 1).map(|l| l.is_blank()).unwrap_or(true)
                {
                    end = Some(i);
                    break;
                }
            }
            i += 1;
        }
        let Some(end) = end else {
            // no bottom border
            let (block_end, extra) = match found_at {
                Some(f) => (f, " or no blank line after table bottom"),
                None => (i.saturating_sub(1).max(start), ""),
            };
            let raw: Vec<String> = lines[start..=block_end.min(lines.len() - 1)]
                .iter()
                .map(|l| l.text.to_string())
                .collect();
            out.push(messages::with_literal(
                self.msg(
                    messages::ERROR,
                    &format!("Malformed table.\nNo bottom table border found{extra}."),
                    lines[start].lineno,
                ),
                raw.join("\n").trim_end(),
            ));
            *pos = block_end + 1;
            if !extra.is_empty() {
                if let Some(l) = lines.get(*pos).filter(|l| !l.is_blank()) {
                    out.push(self.msg(
                        messages::WARNING,
                        "Blank line required after table.",
                        l.lineno,
                    ));
                }
            }
            return;
        };
        *pos = end + 1;
        let blank_after_ok = lines.get(*pos).map(|l| l.is_blank()).unwrap_or(true);

        let block: Vec<LineRef<'a>> = lines[start..=end].to_vec();
        let raw_block: Vec<String> = block.iter().map(|l| l.text.to_string()).collect();
        let malformed = |detail: &str, lineno: u32| -> Node {
            messages::with_literal(
                messages::system_message(
                    messages::ERROR,
                    &format!("Malformed table.\n{detail}"),
                    lineno,
                    self.source_path,
                ),
                raw_block.join("\n").trim_end(),
            )
        };

        // columns from the top border '=' runs
        let top_chars: Vec<char> = block[0].text.trim_end().chars().collect();
        let mut columns: Vec<(usize, usize)> = Vec::new();
        let mut run_start = None;
        for (ci, c) in top_chars.iter().enumerate() {
            if *c == '=' {
                if run_start.is_none() {
                    run_start = Some(ci);
                }
            } else if let Some(s) = run_start.take() {
                columns.push((s, ci));
            }
        }
        if let Some(s) = run_start {
            columns.push((s, top_chars.len()));
        }
        let border_end = columns.last().map(|(_, e)| *e).unwrap_or(0);

        // interior head/body sep: full-'='-runs line converted to span line
        let mut head_sep_row: Option<usize> = None; // index into block
        let mut work: Vec<String> = block
            .iter()
            .map(|l| l.text.trim_end().to_string())
            .collect();
        let n = work.len();
        for (bi, w) in work.iter_mut().enumerate() {
            if bi > 0 && bi < n - 1 && is_simple_table_border(w) {
                head_sep_row = Some(bi);
                *w = w.replace('=', "-");
            }
        }
        let bottom = work.len() - 1;
        work[0] = work[0].replace('=', "-");
        work[bottom] = work[bottom].replace('=', "-");

        // rows: (start_line_idx, end_line_idx_exclusive, colspec)
        struct RawRow {
            start: usize,
            end: usize,
            cols: Vec<(usize, usize)>,
        }
        let parse_span_cols =
            |line: &str, table_line: usize| -> Result<Vec<(usize, usize)>, Box<Node>> {
                let chars: Vec<char> = line.chars().collect();
                let mut cols = Vec::new();
                let mut rs = None;
                for (ci, c) in chars.iter().enumerate() {
                    if *c == '-' {
                        if rs.is_none() {
                            rs = Some(ci);
                        }
                    } else if let Some(s) = rs.take() {
                        cols.push((s, ci));
                    }
                }
                if let Some(s) = rs {
                    cols.push((s, chars.len()));
                }
                if cols.last().map(|(_, e)| *e) != Some(border_end) {
                    return Err(Box::new(malformed(
                        &format!("Column span incomplete in table line {}.", table_line + 1),
                        block[0].lineno,
                    )));
                }
                Ok(cols)
            };

        let is_span_line = |s: &str| {
            let t = s.trim_end();
            !t.is_empty() && t.starts_with('-') && t.chars().all(|c| matches!(c, '-' | ' '))
        };
        let first_col = columns.first().copied().unwrap_or((0, 0));
        let mut rows: Vec<RawRow> = Vec::new();
        let mut open: Option<usize> = None;
        #[allow(clippy::needless_range_loop)]
        for bi in 1..work.len() {
            let line = &work[bi];
            let at_bottom = bi == bottom;
            if is_span_line(line) || at_bottom {
                let span_cols = match parse_span_cols(&work[bi], bi) {
                    Ok(c) => c,
                    Err(m) => {
                        out.push(*m);
                        return;
                    }
                };
                if let Some(s) = open.take() {
                    rows.push(RawRow {
                        start: s,
                        end: bi,
                        cols: span_cols,
                    });
                } else if !at_bottom || rows.is_empty() {
                    // span line with no open row: empty row
                    rows.push(RawRow {
                        start: bi,
                        end: bi,
                        cols: span_cols,
                    });
                }
                continue;
            }
            let fc_text = display_slice(line, first_col.0, first_col.1);
            if !fc_text.trim().is_empty() {
                if let Some(s) = open.take() {
                    rows.push(RawRow {
                        start: s,
                        end: bi,
                        cols: columns.clone(),
                    });
                }
                open = Some(bi);
            } else if open.is_none() {
                // blank first column with no open row: dropped silently
            }
        }
        if let Some(s) = open {
            rows.push(RawRow {
                start: s,
                end: bottom,
                cols: columns.clone(),
            });
        }

        // margin check + last-column extension, per ROW using the row's own
        // colspec (span rows have merged columns — docutils check_columns).
        let mut last_col_end = border_end;
        for row in &rows {
            for bi in row.start..row.end.min(bottom) {
                let line = &work[bi];
                for w2 in row.cols.windows(2) {
                    let (_, e1) = w2[0];
                    let (s2, _) = w2[1];
                    if !display_slice(line, e1, s2).trim().is_empty() {
                        out.push(malformed(
                            &format!("Text in column margin in table line {}.", bi + 1),
                            block[bi].lineno,
                        ));
                        return;
                    }
                }
                let row_border_end = row.cols.last().map(|(_, e)| *e).unwrap_or(border_end);
                let tail = display_slice(line, row_border_end, column_width(line));
                if !tail.trim().is_empty() {
                    let last_start = row.cols.last().map(|(s, _)| *s).unwrap_or(0);
                    let extent = last_start
                        + column_width(
                            display_slice(line, last_start, column_width(line)).trim_end(),
                        );
                    last_col_end = last_col_end.max(extent);
                }
            }
        }

        // map span cols -> column indices for morecols; validate alignment
        let col_starts: Vec<usize> = columns.iter().map(|(s, _)| *s).collect();
        let col_ends: Vec<usize> = columns.iter().map(|(_, e)| *e).collect();
        let mut built_rows: Vec<(usize, Node)> = Vec::new(); // (start_line, row)
        for row in &rows {
            let mut r = Node::elem(kinds::ROW, self.span_of(lines, start, end));
            for (ci, (cs, ce)) in row.cols.iter().enumerate() {
                let ce_eff = if ci == row.cols.len() - 1 {
                    last_col_end.max(*ce)
                } else {
                    *ce
                };
                let Some(ci_start) = col_starts.iter().position(|s| s == cs) else {
                    out.push(malformed(
                        &format!(
                            "Column span alignment problem in table line {}.",
                            row.start + 2
                        ),
                        block[0].lineno,
                    ));
                    return;
                };
                let span_end_col = if ci == row.cols.len() - 1 {
                    columns.len() - 1
                } else {
                    match col_ends.iter().position(|e| e == ce) {
                        Some(p) => p,
                        None => {
                            out.push(malformed(
                                &format!(
                                    "Column span alignment problem in table line {}.",
                                    row.start + 2
                                ),
                                block[0].lineno,
                            ));
                            return;
                        }
                    }
                };
                let morecols = span_end_col - ci_start;
                let mut entry = Node::elem(kinds::ENTRY, self.span_of(lines, start, end));
                if morecols > 0 {
                    entry.set("morecols", AttrValue::Int(morecols as i64));
                }
                // cell block
                let mut cell_lines: Vec<LineRef<'a>> = Vec::new();
                #[allow(clippy::needless_range_loop)]
                for bi in row.start..row.end.min(bottom) {
                    let l = &block[bi];
                    let orig = display_slice(l.text, *cs, ce_eff.min(column_width(l.text)));
                    let use_text = orig.trim_end();
                    cell_lines.push(LineRef::new(use_text, l.lineno, l.src_start, l.src_end));
                }
                let base = cell_lines
                    .iter()
                    .filter(|l| !l.text.trim().is_empty())
                    .map(|l| l.indent())
                    .min()
                    .unwrap_or(0);
                let dedented: Vec<LineRef<'a>> = cell_lines
                    .iter()
                    .map(|l| {
                        if l.text.trim().is_empty() {
                            LineRef::new("", l.lineno, l.src_start, l.src_end)
                        } else {
                            l.dedented(base)
                        }
                    })
                    .collect();
                if dedented.iter().any(|l| !l.text.is_empty()) {
                    self.line_bias += 1;
                    entry.children = self.parse_elements(&dedented);
                    self.line_bias -= 1;
                }
                r.children.push(entry);
            }
            built_rows.push((row.start, r));
        }

        // widened last column affects colwidths
        let mut colwidths: Vec<usize> = columns.iter().map(|(s, e)| e - s).collect();
        if let (Some(last), Some((s, _))) = (colwidths.last_mut(), columns.last()) {
            *last = (*last).max(last_col_end.saturating_sub(*s));
        }

        let table_span = self.span_of(lines, start, end);
        let mut table = Node::elem(kinds::TABLE, table_span);
        let mut tgroup = Node::elem(kinds::TGROUP, table_span);
        tgroup.set("cols", AttrValue::Int(columns.len() as i64));
        for w in &colwidths {
            let mut cs = Node::elem(kinds::COLSPEC, table_span);
            cs.set("colwidth", AttrValue::Int(*w as i64));
            tgroup.children.push(cs);
        }
        if let Some(sep) = head_sep_row {
            let mut thead = Node::elem(kinds::THEAD, table_span);
            let mut tbody_rows = Vec::new();
            for (rs, r) in built_rows {
                if rs < sep {
                    thead.children.push(r);
                } else {
                    tbody_rows.push(r);
                }
            }
            tgroup.children.push(thead);
            let mut tbody = Node::elem(kinds::TBODY, table_span);
            tbody.children = tbody_rows;
            tgroup.children.push(tbody);
        } else {
            let mut tbody = Node::elem(kinds::TBODY, table_span);
            tbody.children = built_rows.into_iter().map(|(_, r)| r).collect();
            tgroup.children.push(tbody);
        }
        table.children.push(tgroup);
        out.push(table);

        if !blank_after_ok {
            if let Some(l) = lines.get(*pos) {
                out.push(self.msg(
                    messages::WARNING,
                    "Blank line required after table.",
                    l.lineno,
                ));
            }
        }
    }

    /// `.. name:: …` directives: the docutils machinery (probe-verified;
    /// see 2026-08-13-m2-wave3-probes.md). Wave-3 registry: admonitions +
    /// generic admonition; more directives arrive in later tasks.
    fn parse_directive(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        name: &str,
        first_rest: &str,
        out: &mut Vec<Node>,
    ) {
        let start = *pos;
        let lineno = lines[start].lineno;
        let (block, consumed, _indent, _term) = indented_block(lines, start + 1);
        *pos = start + 1 + consumed;
        let span = self.span_of(lines, start, start + consumed);
        // Full raw source (original indentation preserved, trailing blanks
        // trimmed) — reproduced in EVERY directive error literal.
        let mut raw_lines: Vec<&str> = vec![lines[start].text];
        for l in &lines[start + 1..start + 1 + consumed] {
            raw_lines.push(l.text);
        }
        while raw_lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            raw_lines.pop();
        }
        let rawsource = raw_lines.join("\n");

        let lower = name.to_lowercase();
        let Some(spec) = directive_spec(&lower) else {
            // Unknown: INFO (language-resolution narrative) + ERROR.
            out.push(self.msg(
                messages::INFO,
                &format!(
                    "No directive entry for \"{name}\" in module \"docutils.parsers.rst.languages.en\".\nTrying \"{name}\" as canonical directive name."
                ),
                lineno,
            ));
            out.push(messages::with_literal(
                self.msg(
                    messages::ERROR,
                    &format!("Unknown directive type \"{name}\"."),
                    lineno,
                ),
                &rawsource,
            ));
            return;
        };

        let dir_error = |me: &Self, detail: &str| -> Node {
            messages::with_literal(
                me.msg(
                    messages::ERROR,
                    &format!("Error in \"{lower}\" directive:\n{detail}."),
                    lineno,
                ),
                &rawsource,
            )
        };

        // Split the block: arg_block = first_rest + lines up to the first
        // blank; content = everything after the first blank line.
        let mut arg_lines: Vec<LineRef<'a>> = Vec::new();
        if !first_rest.trim().is_empty() {
            let offset = lines[start].text.len() - first_rest.len();
            arg_lines.push(LineRef::new(
                lines[start].text[offset..].trim_start(),
                lineno,
                lines[start].src_start,
                lines[start].src_end,
            ));
        }
        let mut content: Vec<LineRef<'a>> = Vec::new();
        let mut in_content = arg_lines.is_empty() && first_rest.trim().is_empty() && false;
        let mut seen_blank = false;
        for l in &block {
            if seen_blank {
                content.push(*l);
            } else if l.is_blank() {
                seen_blank = true;
            } else if !in_content {
                arg_lines.push(*l);
            }
        }
        let _ = in_content;
        // Trim leading blanks of content.
        while content.first().map(|l| l.is_blank()).unwrap_or(false) {
            content.remove(0);
        }

        // Options: the arg block splits at the FIRST field-marker line.
        let mut argument_lines: Vec<LineRef<'a>> = Vec::new();
        let mut option_lines: Vec<LineRef<'a>> = Vec::new();
        let mut in_options = false;
        for l in &arg_lines {
            if !in_options && field_marker(l.text).is_some() {
                in_options = true;
            }
            if in_options {
                option_lines.push(*l);
            } else {
                argument_lines.push(*l);
            }
        }

        // Parse options.
        let mut classes: Vec<String> = Vec::new();
        let mut node_name: Option<String> = None;
        let mut i = 0usize;
        while i < option_lines.len() {
            let l = option_lines[i];
            let Some((opt_name, body_start)) = field_marker(l.text) else {
                out.push(dir_error(self, "invalid option block"));
                return;
            };
            // option value: rest of line + deeper-indented continuations
            let mut value = l.text[body_start..].trim().to_string();
            let mut j = i + 1;
            while j < option_lines.len()
                && option_lines[j].indent() > 0
                && field_marker(option_lines[j].text).is_none()
            {
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(option_lines[j].text.trim());
                j += 1;
            }
            if j < option_lines.len()
                && option_lines[j].indent() == 0
                && field_marker(option_lines[j].text).is_none()
            {
                out.push(dir_error(self, "invalid option block"));
                return;
            }
            match opt_name.to_lowercase().as_str() {
                "class" if spec.allow_class => {
                    if value.is_empty() {
                        out.push(dir_error(
                            self,
                            &format!(
                                "invalid option value: (option: \"class\"; value: None)\nargument required but none supplied"
                            ),
                        ));
                        return;
                    }
                    for word in value.split_whitespace() {
                        classes.push(ids::make_id(word));
                    }
                }
                "name" if spec.allow_class => {
                    node_name = Some(value.clone());
                }
                other => {
                    out.push(dir_error(
                        self,
                        &format!("unknown option: \"{other}\""),
                    ));
                    return;
                }
            }
            i = j;
        }

        // Arguments / leftover argument lines become content for
        // zero-argument directives (probe X6).
        let mut effective_content = content;
        match spec.kind {
            DirectiveKind::Admonition(kind) => {
                if !argument_lines.is_empty() {
                    let mut pre = argument_lines.clone();
                    if !effective_content.is_empty() {
                        pre.push(LineRef::new(
                            "",
                            lineno,
                            lines[start].src_start,
                            lines[start].src_end,
                        ));
                    }
                    pre.extend(effective_content.iter().copied());
                    effective_content = pre;
                }
                if effective_content.iter().all(|l| l.is_blank()) {
                    out.push(messages::with_literal(
                        self.msg(
                            messages::ERROR,
                            &format!(
                                "Content block expected for the \"{lower}\" directive; none found."
                            ),
                            lineno,
                        ),
                        &rawsource,
                    ));
                    return;
                }
                let mut node = Node::elem(kind, span);
                node.attrs.classes = classes;
                if let Some(n) = node_name {
                    node.attrs.names.push(ids::fully_normalize_name(&n));
                    let msg = self.registry.set_id_explicit(
                        &mut node,
                        lineno,
                        self.source_path,
                        true,
                        None,
                    );
                    if let Some(m) = msg {
                        out.push(m);
                    }
                }
                self.line_bias += 1;
                node.children.extend(self.parse_elements(&effective_content));
                self.line_bias -= 1;
                out.push(node);
            }
            DirectiveKind::GenericAdmonition => {
                if argument_lines.is_empty() {
                    out.push(dir_error(self, "1 argument(s) required, 0 supplied"));
                    return;
                }
                let title_text = argument_lines
                    .iter()
                    .map(|l| l.text.trim())
                    .collect::<Vec<_>>()
                    .join("\n");
                if effective_content.iter().all(|l| l.is_blank()) {
                    out.push(messages::with_literal(
                        self.msg(
                            messages::ERROR,
                            &format!(
                                "Content block expected for the \"{lower}\" directive; none found."
                            ),
                            lineno,
                        ),
                        &rawsource,
                    ));
                    return;
                }
                let mut node = Node::elem("admonition", span);
                if classes.is_empty() {
                    node.attrs
                        .classes
                        .push(format!("admonition-{}", ids::make_id(&title_text)));
                } else {
                    node.attrs.classes = classes;
                }
                if let Some(n) = node_name {
                    node.attrs.names.push(ids::fully_normalize_name(&n));
                    let msg = self.registry.set_id_explicit(
                        &mut node,
                        lineno,
                        self.source_path,
                        true,
                        None,
                    );
                    if let Some(m) = msg {
                        out.push(m);
                    }
                }
                let inline = super::inline::parse_inline(
                    &title_text,
                    span,
                    lineno,
                    &mut self.registry,
                    self.source_path,
                );
                let mut title = Node::elem(kinds::TITLE, span);
                title.children = inline.nodes;
                node.children.push(title);
                for m in inline.messages {
                    node.children.push(m);
                }
                self.line_bias += 1;
                node.children.extend(self.parse_elements(&effective_content));
                self.line_bias -= 1;
                out.push(node);
            }
        }
    }

    fn parse_anonymous_shortcut(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        rest: &str,
        out: &mut Vec<Node>,
    ) {
        let start = *pos;
        let mut consumed = 0usize;
        while lines
            .get(start + 1 + consumed)
            .map(|l| !l.is_blank() && l.indent() > 0)
            .unwrap_or(false)
        {
            consumed += 1;
        }
        let span = self.span_of(lines, start, start + consumed);
        let mut link = rest.trim().to_string();
        for l in &lines[start + 1..start + 1 + consumed] {
            if !link.is_empty() {
                link.push('\n');
            }
            link.push_str(l.text.trim());
        }
        *pos = start + 1 + consumed;
        let mut target = Node::elem(kinds::TARGET, span);
        target.set("anonymous", AttrValue::Int(1));
        if !link.is_empty() {
            if let Some(refname) = reference_name_from_link(&link) {
                target.set("refname", AttrValue::Str(refname));
            } else {
                let uri: String = link
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != '\\')
                    .collect();
                target.set("refuri", AttrValue::Str(uri));
            }
        }
        self.registry.set_id_anonymous(&mut target);
        out.push(target);
        self.warn_explicit_markup_end(lines, *pos, out);
    }
}

// ----------------------------------------------------------------------
// free helpers
// ----------------------------------------------------------------------

/// Field marker: `:name:` where the name may not start with `:`/space,
/// may not end with a space, and interior `:` is allowed unless followed
/// by space, backtick, or EOL. The marker must close with `:` + space/EOL.
/// Returns (raw name, byte index just past the closing colon).
fn field_marker(text: &str) -> Option<(String, usize)> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if first != ':' {
        return None;
    }
    let mut name = String::new();
    let mut prev_char: Option<char> = None;
    let mut it = chars.peekable();
    // reject :: and ": "
    match it.peek() {
        Some((_, ':')) | Some((_, ' ')) | None => return None,
        _ => {}
    }
    while let Some((i, c)) = it.next() {
        match c {
            '\\' => {
                name.push(c);
                if let Some((_, esc)) = it.next() {
                    name.push(esc);
                    prev_char = Some(esc);
                }
            }
            ':' => {
                let next = it.peek().map(|(_, c)| *c);
                match next {
                    None | Some(' ') => {
                        // closing colon; name may not end with a space
                        if prev_char == Some(' ') || name.is_empty() {
                            return None;
                        }
                        return Some((name, i + 1));
                    }
                    Some('`') => return None,
                    _ => {
                        name.push(':');
                        prev_char = Some(':');
                    }
                }
            }
            _ => {
                name.push(c);
                prev_char = Some(c);
            }
        }
    }
    None
}

/// Option-group marker: synonyms split on `, ` (not inside `<>`), each a
/// short (`-x`/`+x` with optional attached/spaced arg) or long
/// (`--name`/`/name` with `=`/space arg) option. Returns the specs plus
/// the description remainder (after 2+ spaces), or None when any synonym
/// is malformed.
#[allow(clippy::type_complexity)]
fn option_group_marker(text: &str) -> Option<(Vec<(String, Option<(String, String)>)>, &str)> {
    // split marker from description at the first run of 2+ spaces
    // OUTSIDE angle brackets
    let mut in_angle = false;
    let mut marker_end = text.len();
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    let mut k = 0;
    while k < bytes.len() {
        let (i, c) = bytes[k];
        match c {
            '<' => in_angle = true,
            '>' => in_angle = false,
            ' ' if !in_angle && bytes.get(k + 1).map(|(_, c)| *c == ' ').unwrap_or(false) => {
                marker_end = i;
                break;
            }
            _ => {}
        }
        k += 1;
    }
    let marker = &text[..marker_end];
    let desc = text[marker_end..].trim_start();

    // split synonyms on ', ' outside <>
    let mut specs = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_angle = false;
    let mchars: Vec<char> = marker.chars().collect();
    let mut idx = 0;
    while idx < mchars.len() {
        let c = mchars[idx];
        match c {
            '<' => {
                in_angle = true;
                cur.push(c);
            }
            '>' => {
                in_angle = false;
                cur.push(c);
            }
            ',' if !in_angle && mchars.get(idx + 1) == Some(&' ') => {
                parts.push(std::mem::take(&mut cur));
                idx += 1; // skip the space
            }
            _ => cur.push(c),
        }
        idx += 1;
    }
    parts.push(cur);

    for part in &parts {
        specs.push(parse_one_option(part)?);
    }
    Some((specs, desc))
}

/// One option synonym -> (option_string, Some((delimiter, argument))).
fn parse_one_option(part: &str) -> Option<(String, Option<(String, String)>)> {
    let optarg_ok = |s: &str| -> bool {
        if let Some(inner) = s.strip_prefix('<') {
            return inner.ends_with('>') && !inner[..inner.len() - 1].contains(['<', '>']);
        }
        let mut cs = s.chars();
        matches!(cs.next(), Some(c) if c.is_ascii_alphabetic())
            && cs.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    };
    if let Some(rest) = part.strip_prefix("--").or_else(|| part.strip_prefix('/')) {
        let prefix = if part.starts_with("--") { "--" } else { "/" };
        // optname [ =|space optarg ]
        let name_end = rest.find([' ', '=']).unwrap_or(rest.len());
        let (name, tail) = rest.split_at(name_end);
        let mut nc = name.chars();
        let name_ok = matches!(nc.next(), Some(c) if c.is_ascii_alphanumeric())
            && nc.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
        if !name_ok {
            return None;
        }
        if tail.is_empty() {
            return Some((format!("{prefix}{name}"), None));
        }
        let delim = &tail[..1];
        let arg = &tail[1..];
        if !optarg_ok(arg) {
            return None;
        }
        return Some((
            format!("{prefix}{name}"),
            Some((delim.to_string(), arg.to_string())),
        ));
    }
    let rest = part.strip_prefix('-').or_else(|| part.strip_prefix('+'))?;
    let prefix = &part[..1];
    let mut rc = rest.chars();
    let letter = rc.next().filter(|c| c.is_ascii_alphanumeric())?;
    let tail: String = rc.collect();
    if tail.is_empty() {
        return Some((format!("{prefix}{letter}"), None));
    }
    if let Some(arg) = tail.strip_prefix(' ') {
        if !optarg_ok(arg) {
            return None;
        }
        return Some((
            format!("{prefix}{letter}"),
            Some((" ".to_string(), arg.to_string())),
        ));
    }
    if !optarg_ok(&tail) {
        return None;
    }
    Some((format!("{prefix}{letter}"), Some((String::new(), tail))))
}

fn is_grid_table_top(text: &str) -> bool {
    // \+-[-+]+-\+ *$  (minimum "+-x-+": 5 chars)
    let t = text.trim_end();
    let chars: Vec<char> = t.chars().collect();
    chars.len() >= 5
        && chars[0] == '+'
        && chars[chars.len() - 1] == '+'
        && chars[1] == '-'
        && chars[chars.len() - 2] == '-'
        && chars[1..chars.len() - 1]
            .iter()
            .all(|c| matches!(c, '-' | '+'))
}

fn is_grid_head_sep(text: &str) -> bool {
    // \+=[=+]+=\+ *$  (minimum 5 chars)
    let t = text.trim_end();
    let chars: Vec<char> = t.chars().collect();
    chars.len() >= 5
        && chars[0] == '+'
        && chars[chars.len() - 1] == '+'
        && chars[1] == '='
        && chars[chars.len() - 2] == '='
        && chars[1..chars.len() - 1]
            .iter()
            .all(|c| matches!(c, '=' | '+'))
}

/// `=+[ =]*$` — a candidate simple-table border (incl. solid runs).
fn is_simple_table_border(text: &str) -> bool {
    let t = text.trim_end();
    !t.is_empty() && t.starts_with('=') && t.chars().all(|c| matches!(c, '=' | ' '))
}

fn is_simple_table_top(text: &str) -> bool {
    // =+( +=+)+ *$  (two or more '=' runs)
    let t = text.trim_end();
    if t.is_empty() {
        return false;
    }
    let mut runs = 0;
    let mut in_run = false;
    for c in t.chars() {
        match c {
            '=' => {
                if !in_run {
                    runs += 1;
                    in_run = true;
                }
            }
            ' ' => in_run = false,
            _ => return false,
        }
    }
    runs >= 2
}

/// Byte offsets per DISPLAY column (east-asian wide chars occupy two
/// columns; the second maps to the char's end so mid-char boundaries
/// exclude it — matching docutils' double-width padding behavior).
fn display_byte_index(text: &str) -> Vec<usize> {
    let mut index = Vec::with_capacity(text.len() + 1);
    for (b, c) in text.char_indices() {
        index.push(b);
        if unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) == 2 {
            index.push(b + c.len_utf8());
        }
    }
    index.push(text.len());
    index
}

/// Slice by DISPLAY column range (byte-safe; mid-wide-char boundaries
/// clamp to char edges).
fn display_slice(text: &str, from: usize, to: usize) -> &str {
    let index = display_byte_index(text);
    let n = index.len() - 1;
    let start = index[from.min(n)];
    let end = index[to.min(n)];
    if start >= end {
        ""
    } else {
        &text[start..end]
    }
}

/// Trace one grid cell from its top-left '+': returns (bottom, right,
/// column separators seen, row separators seen).
#[allow(clippy::type_complexity)]
fn trace_cell(
    grid: &[Vec<char>],
    top: usize,
    left: usize,
) -> Option<(usize, usize, Vec<usize>, Vec<usize>)> {
    let at =
        |r: usize, c: usize| -> Option<char> { grid.get(r).and_then(|row| row.get(c)).copied() };
    let width = grid.get(top).map(|r| r.len()).unwrap_or(0);
    // scan right along the top border
    let mut c = left + 1;
    let mut top_corners = Vec::new();
    loop {
        match at(top, c) {
            Some('+') => top_corners.push(c),
            Some('-') => {}
            _ => break,
        }
        c += 1;
        if c > width + 1 {
            break;
        }
    }
    for &right in &top_corners {
        // scan down the right edge
        let mut r = top + 1;
        let mut right_corners = Vec::new();
        loop {
            match at(r, right) {
                Some('+') => right_corners.push(r),
                Some('|') => {}
                _ => break,
            }
            r += 1;
            if r > grid.len() {
                break;
            }
        }
        for &bottom in &right_corners {
            // scan left along the bottom, then up the left edge
            let mut ok = true;
            let mut cseps = vec![left, right];
            for cc in left + 1..right {
                match at(bottom, cc) {
                    Some('+') => cseps.push(cc),
                    Some('-') => {}
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            let mut rseps = vec![top, bottom];
            for rr in top + 1..bottom {
                match at(rr, left) {
                    Some('+') => rseps.push(rr),
                    Some('|') => {}
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            return Some((bottom, right, cseps, rseps));
        }
    }
    None
}

/// Directive marker on the text after `.. `: `name[ ]?::` then space+rest
/// or EOL (probe-verified: at most ONE space before `::`; dangling
/// separators or `:` alone fall through to comment).
fn directive_marker(rest: &str) -> Option<(String, &str)> {
    let chars: Vec<char> = rest.chars().collect();
    let name_len = match_simplename_chars(&chars, 0)?;
    let mut j = name_len;
    if chars.get(j) == Some(&' ') {
        j += 1;
    }
    if chars.get(j) != Some(&':') || chars.get(j + 1) != Some(&':') {
        return None;
    }
    let after = j + 2;
    match chars.get(after) {
        None => {}
        Some(' ') => {}
        _ => return None,
    }
    let name: String = chars[..name_len].iter().collect();
    // byte offset of the remainder after ":: "
    let byte_after: usize = rest
        .char_indices()
        .nth(after + 1)
        .map(|(b, _)| b)
        .unwrap_or(rest.len());
    Some((name, &rest[byte_after..]))
}

enum DirectiveKind {
    /// note/warning/... : content-only, node kind = tagname.
    Admonition(&'static str),
    /// `.. admonition:: Title` with required title argument.
    GenericAdmonition,
}

struct DirectiveSpec {
    kind: DirectiveKind,
    /// :class:/:name: options accepted (all wave-3 task-3 directives).
    allow_class: bool,
}

fn directive_spec(lower: &str) -> Option<DirectiveSpec> {
    let adm = |k: &'static str| {
        Some(DirectiveSpec {
            kind: DirectiveKind::Admonition(k),
            allow_class: true,
        })
    };
    match lower {
        "note" => adm("note"),
        "warning" => adm("warning"),
        "tip" => adm("tip"),
        "hint" => adm("hint"),
        "important" => adm("important"),
        "caution" => adm("caution"),
        "danger" => adm("danger"),
        "error" => adm("error"),
        "attention" => adm("attention"),
        "admonition" => Some(DirectiveSpec {
            kind: DirectiveKind::GenericAdmonition,
            allow_class: true,
        }),
        _ => None,
    }
}

/// docutils `simplename` over a char slice (see rst::inline for the
/// pattern description).
fn match_simplename_chars(chars: &[char], at: usize) -> Option<usize> {
    let n = chars.len();
    let mut i = at;
    let atom = |c: char| (c.is_alphanumeric() || c == '_') && c != '_';
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

/// Consume an indented block starting at `start`: lines while blank or
/// indented, up to the LAST indented line (trailing blanks are neither
/// consumed nor included; callers see them).
/// Returns (dedented block, consumed line count, base indent, adjacency
/// terminator line number when the block ends at an adjacent non-blank
/// column-0 line).
fn indented_block<'a>(
    lines: &[LineRef<'a>],
    start: usize,
) -> (Vec<LineRef<'a>>, usize, usize, Option<u32>) {
    let mut end = start;
    let mut last_content = None;
    while end < lines.len() {
        let l = lines[end];
        if l.is_blank() {
            end += 1;
            continue;
        }
        if l.indent() > 0 {
            last_content = Some(end);
            end += 1;
        } else {
            break;
        }
    }
    let last_content = match last_content {
        Some(l) => l,
        None => return (Vec::new(), 0, 0, None),
    };
    let block_end = last_content + 1;
    let base = lines[start..block_end]
        .iter()
        .filter(|l| !l.is_blank())
        .map(|l| l.indent())
        .min()
        .unwrap_or(0);
    let block: Vec<LineRef<'a>> = lines[start..block_end]
        .iter()
        .map(|l| if l.is_blank() { *l } else { l.dedented(base) })
        .collect();
    let terminator = lines
        .get(block_end)
        .filter(|l| !l.is_blank())
        .map(|l| l.lineno);
    (block, block_end - start, base, terminator)
}

fn strip_literal_colons(text: &str) -> (String, bool) {
    if text == "::" {
        return (String::new(), true);
    }
    if let Some(head) = text.strip_suffix("::") {
        if head.is_empty() {
            return (String::new(), true);
        }
        let last = head.chars().last().unwrap();
        if last == ' ' || last == '\n' {
            return (head.trim_end().to_string(), true);
        }
        return (text[..text.len() - 1].to_string(), true);
    }
    (text.to_string(), false)
}

fn attribution_from_chunk(chunk: &[LineRef<'_>], span: Span) -> Option<(Node, u32)> {
    let first = chunk.first()?;
    if first.indent() != 0 {
        return None;
    }
    // Fixture-verified marker rules: `--`/`---` (not followed by another
    // hyphen) or an em dash, then ZERO or more spaces (all consumed), then
    // non-space text.
    let after = match first.text.strip_prefix('\u{2014}') {
        Some(r) => r,
        None => {
            // `---` then `--`; a further hyphen means an adornment, not a
            // marker. The `---` arm runs first, so the `--` arm's remainder
            // can only start with `-` for exactly `---x`-shaped input.
            let r = first
                .text
                .strip_prefix("---")
                .or_else(|| first.text.strip_prefix("--"))?;
            if r.starts_with('-') {
                return None;
            }
            r
        }
    };
    let rest = after.trim_start_matches(' ');
    if rest.is_empty() {
        return None;
    }
    // Continuation lines must share ONE uniform indent (else the chunk is
    // not an attribution at all) and dedent by exactly that indent.
    let mut text = rest.to_string();
    if chunk.len() > 1 {
        let indent = chunk[1].indent();
        for l in &chunk[1..] {
            if l.indent() != indent {
                return None;
            }
        }
        for l in &chunk[1..] {
            text.push('\n');
            text.push_str(&l.text[indent..]);
        }
    }
    let mut attribution = Node::elem(kinds::ATTRIBUTION, span);
    attribution.children.push(Node::text_node(text, span));
    Some((attribution, first.lineno))
}

fn build_line_block(items: &mut [(usize, Vec<Node>)], span: Span, guard: usize) -> Node {
    let mut lb = Node::elem(kinds::LINE_BLOCK, span);
    // Totality guard mirroring MAX_NEST_DEPTH: absurd nesting flattens
    // instead of overflowing the stack (docutils crashes here).
    if guard >= MAX_NEST_DEPTH {
        for (_, children) in items.iter_mut() {
            let mut line = Node::elem(kinds::LINE, span);
            line.children = std::mem::take(children);
            lb.children.push(line);
        }
        return lb;
    }
    let base = items.iter().map(|(d, _)| *d).min().unwrap_or(0);
    let mut i = 0usize;
    while i < items.len() {
        if items[i].0 <= base {
            let mut line = Node::elem(kinds::LINE, span);
            line.children = std::mem::take(&mut items[i].1);
            lb.children.push(line);
            i += 1;
        } else {
            let run_start = i;
            while i < items.len() && items[i].0 > base {
                i += 1;
            }
            lb.children
                .push(build_line_block(&mut items[run_start..i], span, guard + 1));
        }
    }
    lb
}

// ----------------------------------------------------------------------
// enumerators
// ----------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Enumerator {
    literal: String,
    prefix: &'static str,
    suffix: &'static str,
    auto: bool,
    /// Marker followed by end-of-line with no text (fixture-verified: valid
    /// for a lone first item, never for a successor).
    rest_empty: bool,
    /// Characters the marker occupies (prefix + literal + suffix).
    marker_chars: usize,
}

/// One possible (sequence, ordinal) interpretation of a list so far.
/// `initial` is the first item's ordinal under this sequence; `current` the
/// most recent item's. Priority order = docutils resolution order.
#[derive(Debug, Clone)]
struct EnumCandidate {
    seq: &'static str,
    initial: u64,
    current: u64,
}

fn roman_value(text: &str, lower: bool) -> Option<u64> {
    let t: String = if lower {
        text.to_string()
    } else {
        text.to_lowercase()
    };
    if t.is_empty() {
        return None;
    }
    // canonical: m{0,4}(cm|cd|d?c{0,3})(xc|xl|l?x{0,3})(ix|iv|v?i{0,3})
    let mut rest = t.as_str();
    let mut value = 0u64;
    let mut m_count = 0;
    while rest.starts_with('m') && m_count < 4 {
        value += 1000;
        rest = &rest[1..];
        m_count += 1;
    }
    for (nine, four, five, one, unit) in [
        ("cm", "cd", 'd', 'c', 100u64),
        ("xc", "xl", 'l', 'x', 10u64),
        ("ix", "iv", 'v', 'i', 1u64),
    ] {
        if let Some(r) = rest.strip_prefix(nine) {
            value += 9 * unit;
            rest = r;
            continue;
        }
        if let Some(r) = rest.strip_prefix(four) {
            value += 4 * unit;
            rest = r;
            continue;
        }
        if rest.starts_with(five) {
            value += 5 * unit;
            rest = &rest[1..];
        }
        let mut ones = 0;
        while rest.starts_with(one) && ones < 3 {
            value += unit;
            rest = &rest[1..];
            ones += 1;
        }
    }
    if rest.is_empty() && value > 0 {
        Some(value)
    } else {
        None
    }
}

/// Ordinal of `body` interpreted in a KNOWN sequence.
fn ordinal_in_sequence(body: &str, seq: &str) -> Option<u64> {
    match seq {
        "arabic" => body.parse::<u64>().ok(),
        "loweralpha" => {
            let mut chars = body.chars();
            let c = chars.next()?;
            (chars.next().is_none() && c.is_ascii_lowercase())
                .then(|| (c as u64) - ('a' as u64) + 1)
        }
        "upperalpha" => {
            let mut chars = body.chars();
            let c = chars.next()?;
            (chars.next().is_none() && c.is_ascii_uppercase())
                .then(|| (c as u64) - ('A' as u64) + 1)
        }
        "lowerroman" => roman_value(body, true),
        "upperroman" => roman_value(body, false),
        _ => None,
    }
}

/// Candidate interpretations of a FIRST enumerator, in docutils resolution
/// priority (probe-verified: `i`/`I` prefer roman; all other single letters
/// prefer alpha; multi-char roman must be canonically valid).
fn initial_candidates(body: &str, auto: bool) -> Vec<EnumCandidate> {
    let mk = |seq: &'static str, n: u64| EnumCandidate {
        seq,
        initial: n,
        current: n,
    };
    if auto {
        return vec![mk("arabic", 1)];
    }
    if body.chars().all(|c| c.is_ascii_digit()) && !body.is_empty() {
        return body
            .parse::<u64>()
            .ok()
            .filter(|v| *v <= i64::MAX as u64)
            .map(|v| vec![mk("arabic", v)])
            .unwrap_or_default();
    }
    let chars: Vec<char> = body.chars().collect();
    if chars.len() == 1 {
        // Probe-verified: single-letter firsts have NO ambiguity in docutils
        // 0.22.4 — 'i'/'I' are roman(1) ONLY ("i. x\nj. y" is a paragraph),
        // every other letter is alpha ONLY ("v. five\nvi. six" is a
        // paragraph). Successors reinterpret via ordinal_in_sequence, which
        // is how "h. i. j." stays alpha.
        let c = chars[0];
        return match c {
            'i' => vec![mk("lowerroman", 1)],
            'I' => vec![mk("upperroman", 1)],
            _ if c.is_ascii_lowercase() => {
                vec![mk("loweralpha", (c as u64) - ('a' as u64) + 1)]
            }
            _ if c.is_ascii_uppercase() => {
                vec![mk("upperalpha", (c as u64) - ('A' as u64) + 1)]
            }
            _ => Vec::new(),
        };
    }
    if chars.iter().all(|c| "ivxlcdm".contains(*c)) {
        if let Some(v) = roman_value(body, true) {
            return vec![mk("lowerroman", v)];
        }
    }
    if chars.iter().all(|c| "IVXLCDM".contains(*c)) {
        if let Some(v) = roman_value(body, false) {
            return vec![mk("upperroman", v)];
        }
    }
    Vec::new()
}

/// Narrow candidates by the next item's enumerator; ordinals advance.
fn advance_candidates(candidates: &[EnumCandidate], next: &Enumerator) -> Vec<EnumCandidate> {
    candidates
        .iter()
        .filter_map(|c| {
            let expected = c.current + 1;
            let ok = next.auto || ordinal_in_sequence(&next.literal, c.seq) == Some(expected);
            ok.then_some(EnumCandidate {
                seq: c.seq,
                initial: c.initial,
                current: expected,
            })
        })
        .collect()
}

fn parse_enumerator(text: &str) -> Option<Enumerator> {
    let (prefix, after_prefix): (&'static str, &str) = match text.strip_prefix('(') {
        Some(r) => ("(", r),
        None => ("", text),
    };
    let body_end = after_prefix
        .char_indices()
        .find(|(_, c)| !c.is_ascii_alphanumeric() && *c != '#')
        .map(|(i, _)| i)?;
    if body_end == 0 {
        return None;
    }
    let body = &after_prefix[..body_end];
    let after_body = &after_prefix[body_end..];
    let (suffix, rest): (&'static str, &str) = if prefix == "(" {
        (")", after_body.strip_prefix(')')?)
    } else if let Some(r) = after_body.strip_prefix('.') {
        (".", r)
    } else {
        (")", after_body.strip_prefix(')')?)
    };
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let auto = body == "#";
    if initial_candidates(body, auto).is_empty() {
        return None;
    }
    Some(Enumerator {
        literal: body.to_string(),
        prefix,
        suffix,
        auto,
        rest_empty: rest.trim().is_empty(),
        marker_chars: prefix.len() + body.len() + 1,
    })
}

// ----------------------------------------------------------------------
// targets
// ----------------------------------------------------------------------

struct TargetMarker {
    name: String,
    anonymous: bool,
    link: String,
}

/// Parse `_name: link`, ``_`name`: link``, `__: link` forms from the
/// (possibly multi-line, newline-joined) text after `..`. Returns None for
/// MALFORMED targets (the caller emits a comment + "malformed hyperlink
/// target." warning): missing colon, colon not followed by space/EOL,
/// empty or unclosed backtick phrase, empty plain name, bare `__`.
fn parse_target_marker(rest: &str) -> Option<TargetMarker> {
    let after = rest.strip_prefix('_')?;
    if let Some(a) = after.strip_prefix('_') {
        // `.. __:` / `.. __: uri` anonymous form; bare `.. __` is malformed.
        let link = a.strip_prefix(':')?;
        if !(link.is_empty() || link.starts_with(' ') || link.starts_with('\n')) {
            return None;
        }
        return Some(TargetMarker {
            name: String::new(),
            anonymous: true,
            link: link.trim().to_string(),
        });
    }
    if let Some(quoted) = after.strip_prefix('`') {
        let close = quoted.find('`')?;
        let name = &quoted[..close];
        if name.is_empty() {
            return None;
        }
        let link = quoted[close + 1..].strip_prefix(':')?;
        if !(link.is_empty() || link.starts_with(' ') || link.starts_with('\n')) {
            return None;
        }
        return Some(TargetMarker {
            name: name.to_string(),
            anonymous: false,
            link: link.trim().to_string(),
        });
    }
    // Plain name: scan to the first unescaped ':', which must be followed by
    // space, newline, or end of input.
    let mut name = String::new();
    let mut chars = after.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                if let Some((_, esc)) = chars.next() {
                    name.push(esc);
                }
            }
            ':' => {
                if name.is_empty() {
                    return None;
                }
                let link = &after[i + 1..];
                if !(link.is_empty() || link.starts_with(' ') || link.starts_with('\n')) {
                    return None;
                }
                return Some(TargetMarker {
                    name,
                    anonymous: false,
                    link: link.trim().to_string(),
                });
            }
            _ => name.push(c),
        }
    }
    None
}

/// `name_` or `` `phrase`_ `` → normalized reference name (indirect
/// target). The check runs on the whitespace-joined link block; an escaped
/// trailing underscore (`uri\_`) is NOT a reference (fixture-verified).
fn reference_name_from_link(link: &str) -> Option<String> {
    let joined = ids::whitespace_normalize_name(link);
    let body = joined.strip_suffix('_')?;
    if body.ends_with('\\') {
        return None;
    }
    if let Some(phrase) = body.strip_prefix('`').and_then(|b| b.strip_suffix('`')) {
        if phrase.is_empty() {
            return None;
        }
        return Some(ids::fully_normalize_name(phrase));
    }
    if !body.is_empty()
        && !body.ends_with('_')
        && !body.contains(char::is_whitespace)
        && !body.contains('`')
        && !body.contains('\\')
    {
        return Some(ids::fully_normalize_name(body));
    }
    None
}

// ----------------------------------------------------------------------
// tests (plan tasks 7-12; expectations probe-verified against docutils
// 0.22.4 parse-layer output — see 2026-08-07-m2-wave1-probes.md)
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::rst::{parse_rst, ParseOptions};

    fn pf(src: &str) -> String {
        parse_rst(
            src,
            &ParseOptions {
                source_path: "<snippet>".into(),
            },
        )
        .root
        .pformat()
    }

    // ----- task 7: document + paragraphs -----

    #[test]
    fn empty_document() {
        assert_eq!(pf(""), "<document source=\"<snippet>\">\n");
        assert_eq!(pf("   \n\n  \n"), "<document source=\"<snippet>\">\n");
    }

    #[test]
    fn single_paragraph() {
        assert_eq!(
            pf("Just some text."),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Just some text.\n"
        );
    }

    #[test]
    fn multiline_paragraph_keeps_internal_newlines() {
        assert_eq!(
            pf("line one\nline two"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        line one\n        line two\n"
        );
    }

    #[test]
    fn blank_lines_separate_paragraphs() {
        assert_eq!(
            pf("para one\n\n\npara two"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        para one\n    <paragraph>\n        para two\n"
        );
    }

    #[test]
    fn paragraph_spans_cover_source_bytes() {
        let src = "para one\n\npara two";
        let tree = parse_rst(
            src,
            &ParseOptions {
                source_path: "<snippet>".into(),
            },
        );
        let second = &tree.root.children[1];
        let text = &src[second.span.start as usize..second.span.end as usize];
        assert_eq!(text, "para two");
    }

    // ----- task 8: sections + transitions -----

    #[test]
    fn nested_sections_no_promotion() {
        assert_eq!(
            pf("Title\n=====\n\nPara under title.\n\nSub\n---\n\nPara under sub."),
            "<document source=\"<snippet>\">\n    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        <paragraph>\n            Para under title.\n        <section ids=\"sub\" names=\"sub\">\n            <title>\n                Sub\n            <paragraph>\n                Para under sub.\n"
        );
    }

    #[test]
    fn overline_and_underline_is_a_distinct_style() {
        let out = pf("=====\nOver\n=====\n\nUnder\n=====");
        assert!(out.contains("    <section ids=\"over\" names=\"over\">\n"));
        assert!(out.contains("        <section ids=\"under\" names=\"under\">\n"));
    }

    #[test]
    fn underline_too_short_warns_but_sections() {
        assert_eq!(
            pf("Long Section Title\n======\n"),
            "<document source=\"<snippet>\">\n    <section ids=\"long-section-title\" names=\"long\\ section\\ title\">\n        <title>\n            Long Section Title\n        <system_message level=\"2\" line=\"2\" source=\"<snippet>\" type=\"WARNING\">\n            <paragraph>\n                Title underline too short.\n            <literal_block xml:space=\"preserve\">\n                Long Section Title\n                ======\n"
        );
    }

    #[test]
    fn short_underline_demotes_to_paragraph() {
        let out = pf("Title\n===");
        assert!(out.contains("<system_message level=\"1\" line=\"2\" source=\"<snippet>\" type=\"INFO\">\n        <paragraph>\n            Possible title underline, too short for the title.\n            Treating it as ordinary text because it's so short.\n"));
        assert!(out.contains("<paragraph>\n        Title\n        ===\n"));
        assert!(!out.contains("<section"));
    }

    #[test]
    fn inconsistent_style_skip_is_error_and_drops_section() {
        let out = pf("A\n-\n\nB\n=\n\nC\n-\n\nD\n~\n\nbody\n");
        assert!(out.contains("Inconsistent title style: skip from level 1 to 3.\n"));
        assert!(out.contains("Established title styles: - =\n"));
        assert!(!out.contains("names=\"d\""));
        // D's body attaches inside C, after the error message.
        assert!(out.contains("        <paragraph>\n            body\n"));
    }

    #[test]
    fn duplicate_titles_dupname_both_sections() {
        assert_eq!(
            pf("Duplicate\n=========\n\nx\n\nDuplicate\n=========\n\ny\n"),
            "<document source=\"<snippet>\">\n    <section dupnames=\"duplicate\" ids=\"duplicate\">\n        <title>\n            Duplicate\n        <paragraph>\n            x\n    <section dupnames=\"duplicate\" ids=\"id1\">\n        <title>\n            Duplicate\n        <system_message backrefs=\"id1\" level=\"1\" line=\"7\" source=\"<snippet>\" type=\"INFO\">\n            <paragraph>\n                Duplicate implicit target name: \"duplicate\".\n        <paragraph>\n            y\n"
        );
    }

    #[test]
    fn transitions_parse_clean_everywhere_at_parse_layer() {
        assert_eq!(
            pf("Para.\n\n----\n\nMore."),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Para.\n    <transition>\n    <paragraph>\n        More.\n"
        );
        assert_eq!(
            pf("----\n\npara"),
            "<document source=\"<snippet>\">\n    <transition>\n    <paragraph>\n        para\n"
        );
        assert_eq!(
            pf("para\n\n----"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        para\n    <transition>\n"
        );
        assert_eq!(
            pf("para\n\n----\n\n----\n\nend"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        para\n    <transition>\n    <transition>\n    <paragraph>\n        end\n"
        );
        assert_eq!(
            pf("Head\n====\n\n----\n\npara"),
            "<document source=\"<snippet>\">\n    <section ids=\"head\" names=\"head\">\n        <title>\n            Head\n        <transition>\n        <paragraph>\n            para\n"
        );
        assert_eq!(
            pf("before\n\n---\n\nafter"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        before\n    <paragraph>\n        ---\n    <paragraph>\n        after\n"
        );
    }

    #[test]
    fn single_line_plus_underline_is_title_even_unblanked() {
        assert_eq!(
            pf("para\n----\nafter\n"),
            "<document source=\"<snippet>\">\n    <section ids=\"para\" names=\"para\">\n        <title>\n            para\n        <paragraph>\n            after\n"
        );
    }

    #[test]
    fn multiline_paragraph_absorbs_adornment() {
        assert_eq!(
            pf("line1\nline2\n----\nafter\n"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        line1\n        line2\n        ----\n        after\n"
        );
    }

    // ----- task 9: lists -----

    #[test]
    fn bullet_nesting_and_multi_paragraph_items() {
        assert_eq!(
            pf("- outer one\n\n  * inner a\n\n- first para of item\n\n  second para of item"),
            "<document source=\"<snippet>\">\n    <bullet_list bullet=\"-\">\n        <list_item>\n            <paragraph>\n                outer one\n            <bullet_list bullet=\"*\">\n                <list_item>\n                    <paragraph>\n                        inner a\n        <list_item>\n            <paragraph>\n                first para of item\n            <paragraph>\n                second para of item\n"
        );
    }

    #[test]
    fn tight_and_loose_lists_identical() {
        let tight = pf("- one\n- two");
        let loose = pf("- one\n\n- two");
        assert_eq!(tight, loose);
        assert!(tight.contains("<list_item>\n            <paragraph>\n                one\n"));
    }

    #[test]
    fn enumerated_formats() {
        assert!(pf("a. x\nb. y")
            .contains("<enumerated_list enumtype=\"loweralpha\" prefix=\"\" suffix=\".\">\n"));
        assert!(pf("(1) x\n(2) y")
            .contains("<enumerated_list enumtype=\"arabic\" prefix=\"(\" suffix=\")\">\n"));
        assert!(pf("A) x\nB) y")
            .contains("<enumerated_list enumtype=\"upperalpha\" prefix=\"\" suffix=\")\">\n"));
        assert!(pf("#. x\n#. y")
            .contains("<enumerated_list enumtype=\"arabic\" prefix=\"\" suffix=\".\">\n"));
    }

    #[test]
    fn enumerated_start_and_info_message() {
        let out = pf("3. three\n4. four");
        assert!(out.contains(
            "<enumerated_list enumtype=\"arabic\" prefix=\"\" start=\"3\" suffix=\".\">\n"
        ));
        assert!(out.contains("    <system_message level=\"1\" line=\"1\" source=\"<snippet>\" type=\"INFO\">\n        <paragraph>\n            Enumerated list start value not ordinal-1: \"3\" (ordinal 3)\n"));
    }

    #[test]
    fn non_consecutive_without_blank_aborts_to_paragraph() {
        assert_eq!(
            pf("1. one\n3. three"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        1. one\n        3. three\n"
        );
    }

    #[test]
    fn broken_sequence_mid_list_ends_it_with_warning() {
        assert_eq!(
            pf("1. one\n2. two\n5. five\n"),
            "<document source=\"<snippet>\">\n    <enumerated_list enumtype=\"arabic\" prefix=\"\" suffix=\".\">\n        <list_item>\n            <paragraph>\n                one\n    <system_message level=\"2\" line=\"2\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            Enumerated list ends without a blank line; unexpected unindent.\n    <paragraph>\n        2. two\n        5. five\n"
        );
    }

    #[test]
    fn single_letter_ambiguity() {
        assert!(pf("A. Einstein was smart.").contains("enumtype=\"upperalpha\""));
        assert!(pf("i. single").contains("enumtype=\"lowerroman\""));
        let v = pf("v. five");
        assert!(v.contains("enumtype=\"loweralpha\"") && v.contains("start=\"22\""));
        let c = pf("c. see");
        assert!(c.contains("enumtype=\"loweralpha\"") && c.contains("start=\"3\""));
        let ii = pf("ii. two\niii. three");
        assert!(ii.contains("enumtype=\"lowerroman\"") && ii.contains("start=\"2\""));
    }

    #[test]
    fn bullet_list_end_without_blank_warns() {
        assert_eq!(
            pf("- item\nplain\n"),
            "<document source=\"<snippet>\">\n    <bullet_list bullet=\"-\">\n        <list_item>\n            <paragraph>\n                item\n    <system_message level=\"2\" line=\"2\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            Bullet list ends without a blank line; unexpected unindent.\n    <paragraph>\n        plain\n"
        );
    }

    #[test]
    fn bullet_marker_alone_takes_indented_body() {
        assert_eq!(
            pf("-\n  body from next line\n"),
            "<document source=\"<snippet>\">\n    <bullet_list bullet=\"-\">\n        <list_item>\n            <paragraph>\n                body from next line\n"
        );
    }

    // ----- task 10: definition lists + block quotes -----

    #[test]
    fn definition_list_with_classifiers() {
        assert_eq!(
            pf("term2 : classifier one : classifier two\n    Definition2."),
            "<document source=\"<snippet>\">\n    <definition_list>\n        <definition_list_item>\n            <term>\n                term2\n            <classifier>\n                classifier one\n            <classifier>\n                classifier two\n            <definition>\n                <paragraph>\n                    Definition2.\n"
        );
    }

    #[test]
    fn no_space_colon_stays_in_term() {
        let out = pf("term:not a classifier\n    Definition.");
        assert!(out.contains("<term>\n                term:not a classifier\n"));
        assert!(!out.contains("<classifier>"));
    }

    #[test]
    fn consecutive_items_merge() {
        let out = pf("term1\n    Def1.\n\nterm2\n    Def2.");
        assert_eq!(out.matches("<definition_list>\n").count(), 1);
        assert_eq!(out.matches("<definition_list_item>\n").count(), 2);
    }

    #[test]
    fn definition_list_end_without_blank_warns() {
        let out = pf("term\n    def\nplain\n");
        assert!(out.contains("Definition list ends without a blank line; unexpected unindent.\n"));
        assert!(out.contains("<system_message level=\"2\" line=\"3\""));
    }

    #[test]
    fn block_quote_with_attribution() {
        assert_eq!(
            pf("Para.\n\n    No matter where you go, there you are.\n\n    -- Buckaroo Banzai"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Para.\n    <block_quote>\n        <paragraph>\n            No matter where you go, there you are.\n        <attribution>\n            Buckaroo Banzai\n"
        );
    }

    #[test]
    fn attribution_splits_sibling_quotes() {
        let out = pf("Para.\n\n    First quote.\n\n    -- First Author\n\n    Second quote.\n\n    -- Second Author");
        assert_eq!(out.matches("<block_quote>\n").count(), 2);
        assert!(out.contains("First Author") && out.contains("Second Author"));
    }

    #[test]
    fn multiline_attribution_joins_with_newline() {
        let out = pf("Para.\n\n    Quote.\n\n    -- Author Name,\n       Book Title, 1999\n");
        assert!(
            out.contains("<attribution>\n            Author Name,\n            Book Title, 1999\n")
        );
    }

    #[test]
    fn unexpected_indentation_after_multiline_paragraph() {
        assert_eq!(
            pf("line one\nline two\n    Indented without blank line.\n"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        line one\n        line two\n    <system_message level=\"3\" line=\"3\" source=\"<snippet>\" type=\"ERROR\">\n        <paragraph>\n            Unexpected indentation.\n    <block_quote>\n        <paragraph>\n            Indented without blank line.\n"
        );
    }

    #[test]
    fn partial_dedent_nests_inside_quote_with_warning() {
        assert_eq!(
            pf("Para.\n\n    quoted\n  dedented-oddly\n"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Para.\n    <block_quote>\n        <block_quote>\n            <paragraph>\n                quoted\n        <system_message level=\"2\" line=\"4\" source=\"<snippet>\" type=\"WARNING\">\n            <paragraph>\n                Block quote ends without a blank line; unexpected unindent.\n        <paragraph>\n            dedented-oddly\n"
        );
    }

    // ----- task 11: literal, doctest, line blocks -----

    #[test]
    fn literal_block_expanded_colon() {
        assert_eq!(
            pf("Paragraph introducing::\n\n    literal line one\n    literal line two"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Paragraph introducing:\n    <literal_block xml:space=\"preserve\">\n        literal line one\n        literal line two\n"
        );
    }

    #[test]
    fn colon_math_variants() {
        assert!(pf("Paragraph ends with ::\n\n    literal here")
            .contains("<paragraph>\n        Paragraph ends with\n"));
        assert!(pf("text:::\n\n    x").contains("<paragraph>\n        text::\n"));
        assert_eq!(
            pf("::\n\n    literal"),
            "<document source=\"<snippet>\">\n    <literal_block xml:space=\"preserve\">\n        literal\n"
        );
    }

    #[test]
    fn quoted_literal_block_keeps_quotes() {
        assert_eq!(
            pf("Next is a quoted literal::\n\n> quoted line one\n> quoted line two"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Next is a quoted literal:\n    <literal_block xml:space=\"preserve\">\n        > quoted line one\n        > quoted line two\n"
        );
    }

    #[test]
    fn inconsistent_quoted_literal_errors() {
        assert_eq!(
            pf("intro::\n\n> line one\n$ different\n"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        intro:\n    <literal_block xml:space=\"preserve\">\n        > line one\n    <system_message level=\"3\" line=\"4\" source=\"<snippet>\" type=\"ERROR\">\n        <paragraph>\n            Inconsistent literal block quoting.\n    <paragraph>\n        $ different\n"
        );
    }

    #[test]
    fn missing_literal_block_warns() {
        assert_eq!(
            pf("Intro::\n\nNot indented.\n"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Intro:\n    <system_message level=\"2\" line=\"3\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            Literal block expected; none found.\n    <paragraph>\n        Not indented.\n"
        );
    }

    #[test]
    fn literal_block_end_without_blank_warns() {
        let out = pf("para::\n\n    lit\nback\n");
        assert!(out.contains("Literal block ends without a blank line; unexpected unindent.\n"));
        assert!(out.contains("<system_message level=\"2\" line=\"4\""));
    }

    #[test]
    fn doctest_block() {
        assert_eq!(
            pf(">>> print(\"hello\")\nhello\n>>> 1 + 1\n2"),
            "<document source=\"<snippet>\">\n    <doctest_block xml:space=\"preserve\">\n        >>> print(\"hello\")\n        hello\n        >>> 1 + 1\n        2\n"
        );
    }

    #[test]
    fn line_block_nesting_and_empty_line() {
        assert_eq!(
            pf("| top one\n| top two\n|     nested one\n| back\n|\n| after empty"),
            "<document source=\"<snippet>\">\n    <line_block>\n        <line>\n            top one\n        <line>\n            top two\n        <line_block>\n            <line>\n                nested one\n        <line>\n            back\n        <line>\n        <line>\n            after empty\n"
        );
    }

    #[test]
    fn line_block_continuation_joins_line() {
        assert_eq!(
            pf("| A very long line\n  continued here\n| second\n"),
            "<document source=\"<snippet>\">\n    <line_block>\n        <line>\n            A very long line\n            continued here\n        <line>\n            second\n"
        );
    }

    // ----- task 12: comments + targets -----

    #[test]
    fn comment_forms() {
        assert_eq!(
            pf(".. This is a comment\n   that continues on\n   multiple lines."),
            "<document source=\"<snippet>\">\n    <comment xml:space=\"preserve\">\n        This is a comment\n        that continues on\n        multiple lines.\n"
        );
        // Probe-verified: `..` + blank + indented block leaves an EMPTY
        // comment; the block becomes an ordinary block quote.
        assert_eq!(
            pf("..\n\n   Indented block attached\n   to an empty comment start."),
            "<document source=\"<snippet>\">\n    <comment xml:space=\"preserve\">\n    <block_quote>\n        <paragraph>\n            Indented block attached\n            to an empty comment start.\n"
        );
        // Adjacent block IS the body.
        assert_eq!(
            pf("..\n   block line one\n   block line two"),
            "<document source=\"<snippet>\">\n    <comment xml:space=\"preserve\">\n        block line one\n        block line two\n"
        );
        assert_eq!(
            pf(".."),
            "<document source=\"<snippet>\">\n    <comment xml:space=\"preserve\">\n"
        );
    }

    #[test]
    fn comment_ragged_continuation_dedents_by_min() {
        assert_eq!(
            pf(".. first\n      deep\n   shallow\n"),
            "<document source=\"<snippet>\">\n    <comment xml:space=\"preserve\">\n        first\n           deep\n        shallow\n"
        );
    }

    #[test]
    fn comment_vs_target_dispatch() {
        let out = pf(".. _target: http://example.com\n\n.. just a comment::  with weird colons");
        assert!(out
            .contains("<target ids=\"target\" names=\"target\" refuri=\"http://example.com\">\n"));
        assert!(out.contains(
            "<comment xml:space=\"preserve\">\n        just a comment::  with weird colons\n"
        ));
    }

    #[test]
    fn target_forms_keep_ids_and_names_at_parse_layer() {
        let out = pf(".. _para-target:\n\nSome paragraph here.");
        assert!(out.contains("<target ids=\"para-target\" names=\"para-target\">\n    <paragraph>\n        Some paragraph here.\n"));

        let out = pf(".. _docutils: https://docutils.sourceforge.io/\n.. _indirect: docutils_");
        assert!(out.contains(
            "<target ids=\"docutils\" names=\"docutils\" refuri=\"https://docutils.sourceforge.io/\">\n"
        ));
        assert!(out.contains("<target ids=\"indirect\" names=\"indirect\" refname=\"docutils\">\n"));
    }

    #[test]
    fn multiline_refuri_concatenates() {
        assert_eq!(
            pf(".. _long: https://example.com/\n   path/here\n"),
            "<document source=\"<snippet>\">\n    <target ids=\"long\" names=\"long\" refuri=\"https://example.com/path/here\">\n"
        );
    }

    #[test]
    fn uri_with_spaces_strips_whitespace() {
        assert_eq!(
            pf(".. _a: B  Target_\n"),
            "<document source=\"<snippet>\">\n    <target ids=\"a\" names=\"a\" refuri=\"BTarget_\">\n"
        );
    }

    #[test]
    fn backtick_and_escaped_names() {
        assert_eq!(
            pf(".. _`name with: colon`: https://x/\n"),
            "<document source=\"<snippet>\">\n    <target ids=\"name-with-colon\" names=\"name\\ with:\\ colon\" refuri=\"https://x/\">\n"
        );
        assert_eq!(
            pf(".. _a\\: b: https://y/\n"),
            "<document source=\"<snippet>\">\n    <target ids=\"a-b\" names=\"a:\\ b\" refuri=\"https://y/\">\n"
        );
    }

    #[test]
    fn anonymous_targets_both_spellings() {
        assert_eq!(
            pf(".. __: https://example.com/1\n\n__ https://example.com/2"),
            "<document source=\"<snippet>\">\n    <target anonymous=\"1\" ids=\"id1\" refuri=\"https://example.com/1\">\n    <target anonymous=\"1\" ids=\"id2\" refuri=\"https://example.com/2\">\n"
        );
    }

    #[test]
    fn chained_targets_each_keep_own_ids() {
        let out = pf(".. _target1:\n.. _target2:\n\nSection Title\n=============");
        assert!(out.contains("<target ids=\"target1\" names=\"target1\">\n"));
        assert!(out.contains("<target ids=\"target2\" names=\"target2\">\n"));
        assert!(out.contains("<section ids=\"section-title\" names=\"section\\ title\">\n"));
    }

    #[test]
    fn duplicate_explicit_targets_warn_between() {
        assert_eq!(
            pf(".. _dup: https://1/\n\n.. _dup: https://2/\n"),
            "<document source=\"<snippet>\">\n    <target dupnames=\"dup\" ids=\"dup\" refuri=\"https://1/\">\n    <system_message level=\"2\" line=\"3\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            Duplicate explicit target name: \"dup\".\n    <target dupnames=\"dup\" ids=\"id1\" refuri=\"https://2/\">\n"
        );
    }

    // ----- nested-context errors -----

    #[test]
    fn nested_transition_and_title_are_errors() {
        assert_eq!(
            pf("Para.\n\n    ----\n\n    quoted\n"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Para.\n    <block_quote>\n        <system_message level=\"3\" line=\"3\" source=\"<snippet>\" type=\"ERROR\">\n            <paragraph>\n                Unexpected section title or transition.\n            <literal_block xml:space=\"preserve\">\n                ----\n        <paragraph>\n            quoted\n"
        );
        assert_eq!(
            pf("Para.\n\n    Fake\n    ====\n"),
            "<document source=\"<snippet>\">\n    <paragraph>\n        Para.\n    <block_quote>\n        <system_message level=\"3\" line=\"4\" source=\"<snippet>\" type=\"ERROR\">\n            <paragraph>\n                Unexpected section title.\n            <literal_block xml:space=\"preserve\">\n                Fake\n                ====\n"
        );
    }
}
