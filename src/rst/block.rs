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
    source_path: &'a str,
    source_len: usize,
    registry: IdRegistry,
    styles: Vec<(char, bool)>,
    depth: usize,
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
        if text.starts_with(">>> ") || text == ">>>" {
            self.parse_doctest(lines, pos, out);
            return None;
        }
        if text == "|" || text.starts_with("| ") {
            self.parse_line_block(lines, pos, out);
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
            out.push(self.msg(
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
                out.push(self.msg(
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
            out.push(self.msg(
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
            out.push(self.msg(
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
            out.push(self.msg(
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
            out.push(self.msg(
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
            out.push(self.msg(
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
        let first_rest: String = chars
            .get(after + 1..)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        let first_rest = if chars.get(after) == Some(&' ') {
            first_rest
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
