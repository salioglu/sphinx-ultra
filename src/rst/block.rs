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
    /// Innermost container node kind during nested content parses (docutils
    /// `state_machine.node`); None at document/section level. Directives
    /// like topic/sidebar validate their direct parent against this.
    nested_node_kind: Option<&'static str>,
    /// Sphinx mode (see [`super::ParseOptions::sphinx`]).
    pub(crate) sphinx: bool,
    /// The docname stamped on pending_xref nodes (sphinx `refdoc`).
    pub(crate) docname: String,
    /// Every discovered docname (sphinx `env.found_docs`), for toctree entry
    /// resolution; `None` outside a build (see
    /// [`super::ParseOptions::found_docs`]).
    pub(crate) found_docs: Option<std::sync::Arc<std::collections::BTreeSet<String>>>,
    /// `exclude_patterns` (see [`super::ParseOptions::exclude_patterns`]).
    pub(crate) exclude_patterns: Vec<String>,
    /// `.. highlight::` state consumed by later code-blocks in the same
    /// document (sphinx env.temp_data\['highlight_language'\]).
    highlight_language: Option<String>,
    /// `.. program::` state consumed by later `.. option::` directives in
    /// the same document (sphinx `env.ref_context['std:program']`).
    program: Option<String>,
    /// Sphinx-mode class/rst-class pending classes (the ClassAttribute
    /// transform effect applied inline).
    pending_classes: Option<Vec<String>>,
    /// Per-document equation counter (math domain numbering).
    equation_serial: u32,
    /// Validation-feed records collected during the parse.
    directive_records: Vec<super::DirectiveRecord>,
    role_records: Vec<super::RoleRecord>,
    toctree_records: Vec<super::ToctreeRecord>,
    /// `.. option::` registrations, in document order — the one piece of
    /// `Cmdoption.add_target_and_index` the finished `desc` anatomy cannot
    /// carry, because the program in scope comes from `env.ref_context` and
    /// is stamped on no node (see [`super::ProgramOptionRecord`]).
    program_option_records: Vec<super::ProgramOptionRecord>,
    /// Set while running a substitution-embedded directive (docutils
    /// SubstitutionDef state): replace/unicode/date require it, image
    /// flips its align validation, unicode's trim flags land here.
    substitution_ctx: Option<SubstCtx>,
    /// Substitution names seen (whitespace-normalized, case-preserving) —
    /// docutils document.substitution_defs.
    substitution_names_seen: Vec<String>,
    /// Names defined more than once: earlier nodes get names -> dupnames
    /// in a post-parse walk (docutils mutates the old node in place).
    substitution_dupnames: Vec<String>,
}

#[derive(Debug, Default)]
struct SubstCtx {
    ltrim: bool,
    rtrim: bool,
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
            nested_node_kind: None,
            sphinx: false,
            docname: "index".to_string(),
            found_docs: None,
            exclude_patterns: Vec::new(),
            highlight_language: None,
            program: None,
            pending_classes: None,
            equation_serial: 0,
            directive_records: Vec::new(),
            role_records: Vec::new(),
            toctree_records: Vec::new(),
            program_option_records: Vec::new(),
            substitution_ctx: None,
            substitution_names_seen: Vec::new(),
            substitution_dupnames: Vec::new(),
        }
    }

    /// parse_document plus the flat build-pipeline records.
    pub(crate) fn parse_document_full(mut self) -> super::ParseOutput {
        let root = self.parse_document_impl();
        // Harvest the id/name registry before it drops with `self`: wave 4's
        // std-domain label harvest needs name -> (id, explicit) data that
        // otherwise dies with the BlockParser.
        let registry = super::RegistryExport {
            nameids: self.registry.nameids_snapshot(),
            index_serial: self.registry.index_serial(),
            program_options: std::mem::take(&mut self.program_option_records),
        };
        super::ParseOutput {
            doctree: crate::doctree::Doctree {
                root,
                sources: vec![self.source_path.to_string()],
            },
            directive_records: std::mem::take(&mut self.directive_records),
            role_records: std::mem::take(&mut self.role_records),
            toctrees: std::mem::take(&mut self.toctree_records),
            registry,
        }
    }

    /// Validation-feed record with the M1 validation-scanner's semantics
    /// (spec-INdependent, so registered and unknown directives record the
    /// same way): whitespace-split args, marker-line text routed to
    /// content for the admonition name set, raw string options.
    fn capture_directive_record(
        &mut self,
        name: &str,
        first_line: &LineRef<'a>,
        block: &[LineRef<'a>],
        lineno: u32,
    ) {
        const INLINE_ADMONITIONS: &[&str] = &[
            "note",
            "warning",
            "tip",
            "hint",
            "important",
            "caution",
            "danger",
            "error",
            "attention",
            "seealso",
        ];
        let mut options: Vec<(String, String)> = Vec::new();
        let mut content_lines: Vec<String> = Vec::new();
        let marker_text = first_line.text.trim();
        let lower = name.to_lowercase();
        let is_admonition = INLINE_ADMONITIONS.contains(&lower.as_str());
        if is_admonition && !marker_text.is_empty() {
            content_lines.push(marker_text.to_string());
        }
        // Leading option lines; everything after is content.
        let mut in_options = true;
        for l in block {
            if l.is_blank() {
                if !in_options {
                    content_lines.push(String::new());
                }
                continue;
            }
            if in_options {
                if let Some((oname, body_start)) = field_marker(l.text.trim_start()) {
                    let base = l.text.len() - l.text.trim_start().len();
                    let val = l.text[base + body_start..].trim().to_string();
                    options.push((oname, val));
                    continue;
                }
                in_options = false;
            }
            content_lines.push(l.text.to_string());
        }
        while content_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            content_lines.pop();
        }
        let arguments: Vec<String> = if is_admonition {
            Vec::new()
        } else {
            marker_text.split_whitespace().map(str::to_string).collect()
        };
        self.directive_records.push(super::DirectiveRecord {
            name: name.to_string(),
            arguments,
            options,
            content: content_lines.join("\n"),
            line: lineno,
        });
    }

    /// Inline parse through the parser's own registry/mode; collects role
    /// records emitted by the inliner.
    fn inline(&mut self, text: &str, span: Span, lineno: u32) -> super::inline::InlineResult {
        let mut result = super::inline::parse_inline_ext(
            text,
            span,
            lineno,
            &mut self.registry,
            self.source_path,
            self.sphinx,
            &self.docname,
        );
        self.role_records.append(&mut result.roles);
        result
    }

    /// parse_elements with the containing node kind recorded (docutils
    /// nested_parse: `state_machine.node` = the container element).
    fn parse_nested(&mut self, lines: &[LineRef<'a>], kind: &'static str) -> Vec<Node> {
        let saved = self.nested_node_kind.replace(kind);
        let nodes = self.parse_elements(lines);
        self.nested_node_kind = saved;
        nodes
    }

    /// Nested parse over OWNED text (csv-table cells and, later,
    /// rst_prolog/include): a sub-parser over a locally built `Lines`,
    /// sharing this parser's id registry, with linenos offset so absolute
    /// line numbers keep working.
    fn parse_detached(&mut self, text: &str, first_lineno: u32, kind: &'static str) -> Vec<Node> {
        let lines = Lines::new(text);
        let mut sub = BlockParser::new(&lines, self.source_path, text.len());
        for l in &mut sub.top {
            l.lineno += first_lineno.saturating_sub(1);
        }
        sub.registry = std::mem::replace(&mut self.registry, IdRegistry::new());
        sub.nested_node_kind = Some(kind);
        sub.line_bias = self.line_bias;
        sub.depth = self.depth;
        // Mode + records must flow through the detached parse (review
        // finding: csv cells previously parsed in docutils mode and their
        // directive/role records were dropped).
        sub.sphinx = self.sphinx;
        sub.docname = self.docname.clone();
        sub.found_docs = self.found_docs.clone();
        sub.exclude_patterns = self.exclude_patterns.clone();
        sub.highlight_language = self.highlight_language.clone();
        sub.program = self.program.clone();
        let top = std::mem::take(&mut sub.top);
        let nodes = sub.parse_elements(&top);
        self.registry = sub.registry;
        self.directive_records.append(&mut sub.directive_records);
        self.role_records.append(&mut sub.role_records);
        self.toctree_records.append(&mut sub.toctree_records);
        self.program_option_records
            .append(&mut sub.program_option_records);
        nodes
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

    fn parse_document_impl(&mut self) -> Node {
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
            self.apply_pending_classes(&mut out, 0);
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
        // Duplicate substitution definitions: docutils dupname()s the OLD
        // node in place; we re-walk since the tree is owned (all but the
        // LAST same-name definition lose the name).
        for name in std::mem::take(&mut self.substitution_dupnames) {
            let total = count_subst_defs(&root, &name);
            if total > 1 {
                let mut remaining = total - 1;
                dupname_subst_defs(&mut root, &name, &mut remaining);
            }
        }
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

        let inline = self.inline(&start.title, start.span, start.title_lineno);
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
            let before = out.len();
            let section = self.parse_element(lines, &mut pos, false, &mut out);
            debug_assert!(section.is_none(), "titles never match in nested contexts");
            self.apply_pending_classes(&mut out, before);
        }
        out
    }

    /// Sphinx mode runs the ClassAttribute transform effect inline: a
    /// class/rst-class directive without content stamps the next
    /// non-invisible sibling element (the pending node itself vanishes).
    fn apply_pending_classes(&mut self, out: &mut [Node], from: usize) {
        if self.pending_classes.is_none() {
            return;
        }
        for node in out[from..].iter_mut() {
            if matches!(
                node.kind,
                kinds::COMMENT | kinds::TARGET | kinds::SYSTEM_MESSAGE | "substitution_definition"
            ) {
                continue;
            }
            if let Some(classes) = self.pending_classes.take() {
                node.attrs.classes.extend(classes);
            }
            break;
        }
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
            let result = self.inline(&text, span, lines[start].lineno);
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

        let children = self.parse_nested(&body, "list_item");
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
            let inline = self.inline(&term_text, term_span, term_line.lineno);
            let mut term = Node::elem(kinds::TERM, term_span);
            term.children = inline.nodes;
            term_msgs.extend(inline.messages);
            item.children.push(term);
            for classifier in parts {
                let inline = self.inline(&classifier, term_span, term_line.lineno);
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
            definition
                .children
                .extend(self.parse_nested(&block, "definition"));
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
        out.extend(self.block_quote_elements(&block, span));
        if let Some(t) = terminator {
            out.push(self.msg_sm(
                messages::WARNING,
                "Block quote ends without a blank line; unexpected unindent.",
                t,
            ));
        }
    }

    /// docutils `Body.block_quote()`: build block_quote element(s) plus
    /// interleaved attribution messages from an already-extracted block.
    /// Shared by indented block quotes and the epigraph/highlights/
    /// pull-quote directives.
    fn block_quote_elements(&mut self, block: &[LineRef<'a>], span: Span) -> Vec<Node> {
        let mut out: Vec<Node> = Vec::new();
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
            quote.children = self.parse_nested(&body, "block_quote");
            let mut attr_messages = Vec::new();
            if let Some((raw_attr, lineno)) = attribution {
                let raw = raw_attr.astext();
                let inline = self.inline(&raw, raw_attr.span, lineno);
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
        out
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
                let inline = self.inline(&text, span, first_lineno);
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
        // docutils explicit_construct(): a construct whose parse raises
        // MarkupError queues a WARNING and falls through to the comment
        // path, which re-absorbs the whole block (through internal blanks).
        let mut construct_error: Option<Node> = None;
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
                    // Malformed target: queue the WARNING and fall through
                    // to the comment path below (fixture-verified: the
                    // comment re-absorbs the block through blank lines).
                    *pos = start;
                    construct_error =
                        Some(self.msg(messages::WARNING, "malformed hyperlink target.", lineno));
                }
            }
            if construct_error.is_none() {
                self.warn_explicit_markup_end(lines, *pos, out);
                return;
            }
        }

        // Substitution definitions dispatch BEFORE directives
        // (states.py:2441-2483 construct order). The construct pattern
        // requires a non-space char after `|` (`(?![ ])`) — `.. | x` is a
        // plain comment, not a malformed substitution (review finding 19).
        if construct_error.is_none()
            && rest.starts_with('|')
            && !matches!(rest[1..].chars().next(), None | Some(' '))
            && self.parse_substitution_def(lines, pos, rest, out, &mut construct_error)
        {
            return;
        }

        if construct_error.is_none() {
            if let Some((name, first_rest)) = directive_marker(rest) {
                self.parse_directive(lines, pos, &name, first_rest, out);
                self.warn_explicit_markup_end(lines, *pos, out);
                return;
            }
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
        if let Some(err) = construct_error {
            out.push(err);
        }
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
        let content = self.parse_nested(&body, if is_citation { "citation" } else { "footnote" });
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

            let name_inline = self.inline(&name_raw, field_span, lineno);
            let mut field = Node::elem(kinds::FIELD, field_span);
            let mut fname = Node::elem(kinds::FIELD_NAME, field_span);
            fname.children = name_inline.nodes;
            field.children.push(fname);
            let mut fbody = Node::elem(kinds::FIELD_BODY, field_span);
            fbody.children.extend(name_inline.messages);
            fbody
                .children
                .extend(self.parse_nested(&body_lines, "field_body"));
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
            description.children = self.parse_nested(&body_lines, "description");
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
                entry.children = self.parse_nested(&dedented, "entry");
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
                    entry.children = self.parse_nested(&dedented, "entry");
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
        // Full raw source (original indentation preserved) — reproduced in
        // EVERY directive error literal. Fixture-verified: docutils'
        // block_text spans the marker through ALL trailing blank lines
        // (the final newline then disappears in line-splitting, so exactly
        // one trailing blank renders in the literal).
        let mut raw_end = start + 1 + consumed;
        while lines.get(raw_end).map(|l| l.is_blank()).unwrap_or(false) {
            raw_end += 1;
        }
        let mut raw_lines: Vec<&str> = vec![lines[start].text];
        for l in &lines[start + 1..raw_end] {
            raw_lines.push(l.text);
        }
        let rawsource = raw_lines.join("\n");

        let first_line = {
            let t = first_rest.trim_start_matches(' ');
            let offset = lines[start].text.len() - t.len();
            LineRef::new(
                &lines[start].text[offset..],
                lineno,
                lines[start].src_start,
                lines[start].src_end,
            )
        };
        self.run_directive_core(
            name,
            first_line,
            &block,
            &rawsource,
            lineno,
            span,
            Vec::new(),
            out,
        );
    }

    /// The name-lookup + parse_directive_block + run dispatch shared by
    /// body-level directives and substitution-embedded ones.
    #[allow(clippy::too_many_arguments)]
    fn run_directive_core(
        &mut self,
        name: &str,
        first_line: LineRef<'a>,
        block: &[LineRef<'a>],
        rawsource: &str,
        lineno: u32,
        span: Span,
        presets: Vec<(String, OptVal)>,
        out: &mut Vec<Node>,
    ) {
        self.capture_directive_record(name, &first_line, block, lineno);
        let lower = name.to_lowercase();
        let Some(spec) = directive_spec_mode(&lower, self.sphinx) else {
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
                rawsource,
            ));
            return;
        };

        // MarkupError wrapper (states.py:2274-2281): uses the directive
        // name AS WRITTEN (`.. NOTE::` errors say "NOTE").
        let dir_error = |me: &Self, detail: &str| -> Node {
            messages::with_literal(
                me.msg(
                    messages::ERROR,
                    &format!("Error in \"{name}\" directive:\n{detail}."),
                    lineno,
                ),
                rawsource,
            )
        };

        // ---- parse_directive_block (states.py:2301-2345), exact order ----
        // `indented` mirrors get_first_known_indented(match.end(),
        // strip_top=0): the marker-line remainder after `::` and ALL
        // following spaces, then the (already dedented) indented block.
        let mut indented: Vec<LineRef<'a>> = vec![first_line];
        indented.extend(block.iter().copied());
        // Exactly ONE leading blank line is trimmed, then all trailing.
        if indented.first().map(|l| l.is_blank()).unwrap_or(false) {
            indented.remove(0);
        }
        while indented.last().map(|l| l.is_blank()).unwrap_or(false) {
            indented.pop();
        }

        // Split arg block vs content at the first blank line — only when
        // the directive declares arguments or options.
        let declares_specs = spec.required_arguments > 0
            || spec.optional_arguments > 0
            || !spec.option_spec.is_empty();
        let mut arg_block: Vec<LineRef<'a>>;
        let mut content: Vec<LineRef<'a>>;
        let blank_idx;
        if !indented.is_empty() && declares_specs {
            blank_idx = indented
                .iter()
                .position(|l| l.is_blank())
                .unwrap_or(indented.len());
            arg_block = indented[..blank_idx].to_vec();
            content = indented
                .get(blank_idx + 1..)
                .map(|s| s.to_vec())
                .unwrap_or_default();
        } else {
            blank_idx = 0;
            arg_block = Vec::new();
            content = indented.clone();
        }

        // Options before arguments (parse_directive_options,
        // states.py:2347-2363): the arg block splits at the FIRST
        // field-marker line. Presets (the substitution alt=) seed the
        // dict and are overridden by parsed options.
        let mut options: Vec<(String, OptVal)> = presets;
        if !spec.option_spec.is_empty() {
            if let Some(k) = arg_block
                .iter()
                .position(|l| field_marker(l.text).is_some())
            {
                let opt_block = arg_block.split_off(k);
                match parse_extension_options(&opt_block, spec.option_spec) {
                    Ok(opts) => {
                        for (k2, v) in opts {
                            match options.iter_mut().find(|(n, _)| *n == k2) {
                                Some(slot) => slot.1 = v,
                                None => options.push((k2, v)),
                            }
                        }
                    }
                    Err(detail) => {
                        out.push(dir_error(self, &detail));
                        return;
                    }
                }
            }
        }

        // Leftover argument lines become content for argument-less
        // directives (probe X6), re-joined with the blank separator and
        // everything after it (states.py:2330-2334).
        if !arg_block.is_empty() && spec.required_arguments == 0 && spec.optional_arguments == 0 {
            let mut rejoined = arg_block.clone();
            rejoined.extend(indented[blank_idx.min(indented.len())..].iter().copied());
            content = rejoined;
            arg_block.clear();
        }
        while content.first().map(|l| l.is_blank()).unwrap_or(false) {
            content.remove(0);
        }

        // Arguments (parse_directive_arguments, states.py:2365-2380).
        let mut arguments: Vec<String> = Vec::new();
        if spec.required_arguments + spec.optional_arguments > 0 {
            let arg_text = arg_block
                .iter()
                .map(|l| l.text)
                .collect::<Vec<_>>()
                .join("\n");
            match parse_directive_arguments(&arg_text, &spec) {
                Ok(a) => arguments = a,
                Err(detail) => {
                    out.push(dir_error(self, &detail));
                    return;
                }
            }
        }

        // The content-permission check runs LAST (states.py:2343-2344).
        if !content.is_empty() && !spec.has_content {
            out.push(dir_error(self, "no content permitted"));
            return;
        }

        let input = DirectiveInput {
            name,
            arguments,
            options,
            content,
            span,
            lineno,
            rawsource,
        };
        match spec.kind {
            DirectiveKind::Admonition(kind) => self.run_admonition(kind, input, out),
            DirectiveKind::GenericAdmonition => self.run_generic_admonition(input, out),
            DirectiveKind::Image => self.run_image(input, out),
            DirectiveKind::PseudoSection(kind) => self.run_pseudo_section(kind, input, out),
            DirectiveKind::Rubric => self.run_rubric(input, out),
            DirectiveKind::QuoteClass(class) => self.run_quote_class(class, input, out),
            DirectiveKind::Compound => self.run_compound(input, out),
            DirectiveKind::Container => self.run_container(input, out),
            DirectiveKind::ParsedLiteral => self.run_parsed_literal(input, out),
            DirectiveKind::Figure => self.run_figure(input, out),
            DirectiveKind::Code => self.run_code(input, out),
            DirectiveKind::MathBlock => self.run_math(input, out),
            DirectiveKind::Raw => self.run_raw(input, out),
            DirectiveKind::LineBlockDir => self.run_line_block(input, out),
            DirectiveKind::ClassDir => self.run_class(input, out),
            DirectiveKind::RstTable => self.run_rst_table(input, out),
            DirectiveKind::CsvTable => self.run_csv_table(input, out),
            DirectiveKind::ListTable => self.run_list_table(input, out),
            DirectiveKind::Replace => self.run_replace(input, out),
            DirectiveKind::UnicodeDir => self.run_unicode(input, out),
            DirectiveKind::DateDir => self.run_date(input, out),
            DirectiveKind::Toctree => self.run_toctree(input, out),
            DirectiveKind::VersionChange(info) => self.run_version_change(info, input, out),
            DirectiveKind::SeeAlso => self.run_seealso(input, out),
            DirectiveKind::SphinxCodeBlock => self.run_sphinx_code_block(input, out),
            DirectiveKind::Highlight => self.run_highlight(input, out),
            DirectiveKind::Only => self.run_only(input, out),
            DirectiveKind::SphinxMath => self.run_sphinx_math(input, out),
            DirectiveKind::IndexDir => self.run_index(input, out),
            DirectiveKind::HList => self.run_hlist(input, out),
            DirectiveKind::Glossary => self.run_glossary(input, out),
            DirectiveKind::ObjectDesc(kind) => self.run_object_description(kind, input, out),
            DirectiveKind::ProgramDir => self.run_program(input),
            // `DefaultDomain.run` sets `env.current_document.default_domain`
            // and returns []. This crate implements no domain whose
            // directives/roles the default would route to (the std domain is
            // always consulted last anyway), so the state has nothing to
            // steer — the node-level effect, an empty return, is all of it.
            DirectiveKind::DefaultDomainDir => {}
        }
    }

    /// `.. program::` (`domains/std/__init__.py:333-348`): pure
    /// `env.ref_context` state, no nodes. The literal argument `None` pops
    /// the scope rather than naming a program called "None".
    fn run_program(&mut self, input: DirectiveInput<'a, '_>) {
        let program = ws_collapse(input.arguments[0].trim(), "-");
        if program == "None" {
            self.program = None;
        } else {
            self.program = Some(program);
        }
    }

    /// sphinx math (patches.py MathDirective + math-domain numbering).
    /// Absent label/number are Python None -> pformat "True".
    fn run_sphinx_math(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let mut latex = input
            .content
            .iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(arg) = input.arguments.first() {
            latex = if latex.is_empty() {
                format!("{arg}\n\n")
            } else {
                format!("{arg}\n\n{latex}")
            };
        }
        let label =
            match opt_get(&input.options, "label").or_else(|| opt_get(&input.options, "name")) {
                Some(OptVal::Str(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            };
        let nowrap = opt_get(&input.options, "nowrap").is_some()
            || opt_get(&input.options, "no-wrap").is_some();
        let mut node = Node::elem("math_block", input.span);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        node.set("docname", AttrValue::Str(self.docname.clone()));
        node.set("no-wrap", AttrValue::Int(i64::from(nowrap)));
        node.set("nowrap", AttrValue::Int(i64::from(nowrap)));
        node.set("xml:space", AttrValue::Str("preserve".to_string()));
        node.children.push(Node::text_node(latex, input.span));
        match label {
            Some(label) => {
                self.equation_serial += 1;
                let id = ids::make_id(&format!("equation-{label}"));
                node.attrs.ids.push(id.clone());
                node.set("label", AttrValue::Str(label));
                node.set("number", AttrValue::Int(i64::from(self.equation_serial)));
                let mut target = Node::elem(kinds::TARGET, input.span);
                target.set("refid", AttrValue::Str(id));
                out.push(target);
                out.push(node);
            }
            None => {
                node.set("label", AttrValue::Str("True".to_string()));
                node.set("number", AttrValue::Str("True".to_string()));
                out.push(node);
            }
        }
    }

    /// sphinx index directive (sphinx/domains/index.py IndexDirective).
    fn run_index(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let target_id = format!("index-{}", self.registry.new_index_serialno());
        let mut entries: Vec<String> = Vec::new();
        for line in input.arguments[0].split('\n') {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            entries.extend(process_index_entry(line, &target_id));
        }
        let mut index = Node::elem("index", input.span);
        index.set("entries", AttrValue::Str(entries.join(" ")));
        index.set("inline", AttrValue::Int(0));
        let mut target = Node::elem(kinds::TARGET, input.span);
        match opt_get(&input.options, "name") {
            Some(OptVal::Str(n)) => {
                target.attrs.names.push(ids::fully_normalize_name(n));
            }
            _ => target.attrs.ids.push(target_id),
        }
        out.push(index);
        out.push(target);
    }

    /// sphinx hlist (other.py HList): content must be exactly one bullet
    /// list; distributed into ncolumns hlistcol children.
    fn run_hlist(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let ncolumns = match opt_get(&input.options, "columns") {
            Some(OptVal::Int(n)) if *n > 0 => *n as usize,
            _ => 2,
        };
        let children = self.parse_nested(&input.content, "element");
        let one_list = children.len() == 1 && children[0].kind == kinds::BULLET_LIST;
        if !one_list {
            // logger.warning('.. hlist content is not a list') goes to the
            // log stream, not the tree.
            return;
        }
        let list = children.into_iter().next().expect("length checked");
        let items = list.children;
        let npercol = items.len() / ncolumns;
        let nmore = items.len() % ncolumns;
        let mut hlist = Node::elem("hlist", input.span);
        hlist.set("ncolumns", AttrValue::Str(ncolumns.to_string()));
        let mut it = items.into_iter();
        for col in 0..ncolumns {
            let take = npercol + usize::from(col < nmore);
            let mut bl = Node::elem(kinds::BULLET_LIST, input.span);
            for _ in 0..take {
                match it.next() {
                    Some(item) => bl.children.push(item),
                    None => break,
                }
            }
            let mut colnode = Node::elem("hlistcol", input.span);
            colnode.children.push(bl);
            hlist.children.push(colnode);
        }
        out.push(hlist);
    }

    /// sphinx glossary (std domain): term lines + indented definitions;
    /// each term gets a term-<id> target and an embedded index entry.
    fn run_glossary(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let mut glossary = Node::elem("glossary", input.span);
        glossary.set(
            "sorted",
            AttrValue::Int(i64::from(opt_get(&input.options, "sorted").is_some())),
        );
        let mut dl = Node::elem(kinds::DEFINITION_LIST, input.span);
        dl.attrs.classes.push("glossary".to_string());
        // Entry split: unindented term line(s) followed by an indented
        // definition block (misformat warnings go to the log, not the
        // tree; the corpus pins well-formed input).
        let mut i = 0usize;
        let content = &input.content;
        while i < content.len() {
            if content[i].is_blank() {
                i += 1;
                continue;
            }
            if content[i].indent() > 0 {
                // Stray indented line without a term: log-warned, skipped.
                i += 1;
                continue;
            }
            let mut term_lines: Vec<LineRef<'a>> = Vec::new();
            while i < content.len() && !content[i].is_blank() && content[i].indent() == 0 {
                term_lines.push(content[i]);
                i += 1;
            }
            let mut def_lines: Vec<LineRef<'a>> = Vec::new();
            while i < content.len() && (content[i].is_blank() || content[i].indent() > 0) {
                if content[i].is_blank()
                    && content
                        .get(i + 1)
                        .map(|l| !l.is_blank() && l.indent() == 0)
                        .unwrap_or(true)
                {
                    i += 1;
                    break;
                }
                def_lines.push(content[i]);
                i += 1;
            }
            while def_lines.last().map(|l| l.is_blank()).unwrap_or(false) {
                def_lines.pop();
            }
            let mut item = Node::elem(kinds::DEFINITION_LIST_ITEM, input.span);
            let mut term_messages: Vec<Node> = Vec::new();
            for tl in &term_lines {
                let raw_term = tl.text.trim();
                // split_term_classifiers: ' +: +' — first classifier is
                // the index key.
                let mut parts = raw_term.splitn(2, " : ");
                let term_text = parts.next().unwrap_or(raw_term).trim_end().to_string();
                let index_key = parts
                    .next()
                    .map(|c| c.split(" : ").next().unwrap_or(c).trim().to_string());
                // Sphinx's `make_glossary_term` stamps the term node with
                // the *term line's* own source info, not the directive's
                // (`domains/std/__init__.py:386-388`), and the index node it
                // appends inherits it. Everything that reports a term's
                // location — the duplicate-object warning, above all — reads
                // that, so each term carries its own span here.
                let term_span = Span {
                    source: input.span.source,
                    start: tl.src_start,
                    end: tl.src_end,
                };
                let inline = self.inline(&term_text, term_span, tl.lineno);
                let mut term = Node::elem(kinds::TERM, term_span);
                term.children = inline.nodes;
                term_messages.extend(inline.messages);
                let base = ids::make_id(&format!("term-{term_text}"));
                let node_id = if base == "term" || base.is_empty() {
                    let id = format!("term-{}", self.registry.new_index_serialno());
                    id
                } else {
                    base
                };
                term.attrs.ids.push(node_id.clone());
                let mut index = Node::elem("index", term_span);
                index.set(
                    "entries",
                    AttrValue::Str(index_entry_tuple(
                        "single",
                        &term_text,
                        &node_id,
                        "main",
                        index_key.as_deref(),
                    )),
                );
                term.children.push(index);
                item.children.push(term);
            }
            item.children.extend(term_messages);
            let dedented = dedent_by_min(&def_lines);
            let mut definition = Node::elem(kinds::DEFINITION, input.span);
            definition.children = self.parse_nested(&dedented, "definition");
            item.children.push(definition);
            dl.children.push(item);
        }
        glossary.children.push(dl);
        out.push(glossary);
    }

    /// sphinx `ObjectDescription.run` (`directives/__init__.py:183-314`):
    /// the `index` + `desc` anatomy every object-describing directive
    /// shares, with each subclass's `handle_signature` /
    /// `add_target_and_index` / `transform_content` inlined by
    /// [`ObjectDescKind`].
    fn run_object_description(
        &mut self,
        kind: ObjectDescKind,
        input: DirectiveInput<'a, '_>,
        out: &mut Vec<Node>,
    ) {
        let Some(argument) = input.arguments.first() else {
            return;
        };
        // `self.name` is the directive name as written for the bare
        // docutils registration, but `'{domain}:{name}'` for a domain
        // directive (`Domain.directive`'s adapter, `domains/__init__.py`),
        // which is why `describe` reports `domain=""` and `option` reports
        // `domain="std"` / `objtype="option"`.
        let (domain, objtype) = match kind {
            ObjectDescKind::Describe => ("", input.name.to_string()),
            _ => ("std", input.name.to_lowercase()),
        };
        let span = input.span;

        // Deprecated-alias merge (`:226-241`): the old spelling feeds the
        // new one, and BOTH attributes end up carrying the merged value.
        let has = |name: &'static str| opt_get(&input.options, name).is_some();
        let no_index = has("no-index") || has("noindex");
        let no_index_entry = has("no-index-entry") || has("noindexentry");
        let no_contents_entry = has("no-contents-entry") || has("nocontentsentry");
        let no_typesetting = has("no-typesetting");

        let mut desc = Node::elem("desc", span);
        desc.set("domain", AttrValue::Str(domain.to_string()));
        desc.set("objtype", AttrValue::Str(objtype.clone()));
        // 'desctype' is sphinx's backwards-compatible alias of 'objtype'.
        desc.set("desctype", AttrValue::Str(objtype.clone()));
        desc.set("no-index", AttrValue::Int(i64::from(no_index)));
        desc.set("noindex", AttrValue::Int(i64::from(no_index)));
        desc.set("no-index-entry", AttrValue::Int(i64::from(no_index_entry)));
        desc.set("noindexentry", AttrValue::Int(i64::from(no_index_entry)));
        desc.set(
            "no-contents-entry",
            AttrValue::Int(i64::from(no_contents_entry)),
        );
        desc.set(
            "nocontentsentry",
            AttrValue::Int(i64::from(no_contents_entry)),
        );
        desc.set("no-typesetting", AttrValue::Int(i64::from(no_typesetting)));
        if !domain.is_empty() {
            desc.attrs.classes.push(domain.to_string());
        }
        desc.attrs.classes.push(objtype.clone());

        let mut index_entries: Vec<String> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for sig in object_signatures(argument) {
            let mut signode = Node::elem("desc_signature", span);
            signode
                .attrs
                .classes
                .extend(["sig".to_string(), "sig-object".to_string()]);
            let name = self.handle_object_signature(kind, &sig, &mut signode);
            // `_toc_parts`/`_toc_name` are assigned in a `finally` (`:264-272`),
            // so the ValueError path carries them too. Only
            // `ConfigurationValue` overrides the two empty defaults.
            let (toc_parts, toc_name) = match (kind, &name) {
                (ObjectDescKind::Confval, Some(n)) => {
                    (format!("({},)", py_repr(Some(n))), n.clone())
                }
                _ => ("()".to_string(), String::new()),
            };
            signode.set("_toc_parts", AttrValue::Str(toc_parts));
            signode.set("_toc_name", AttrValue::Str(toc_name));
            // "only add target and index entry if this is the first
            // description of the object with this name in this desc block".
            if let Some(name) = name {
                if !names.contains(&name) {
                    names.push(name.clone());
                    if !no_index {
                        self.object_target_and_index(
                            kind,
                            &objtype,
                            &name,
                            &mut signode,
                            &mut index_entries,
                        );
                    }
                }
            }
            desc.children.push(signode);
        }

        let mut content = Node::elem("desc_content", span);
        content.children = self.parse_nested(&input.content, "desc_content");
        if kind == ObjectDescKind::Confval {
            self.confval_transform_content(&input, &mut content);
        }
        desc.children.push(content);

        let mut index = Node::elem("index", span);
        index.set("entries", AttrValue::Str(index_entries.join(" ")));
        out.push(index);

        if no_typesetting {
            // `:299-313`: the description is replaced by a bare target
            // carrying every id it and its children had — and dropped
            // entirely when there are none (docutils rejects an id-less
            // target).
            let mut ids = Vec::new();
            collect_element_ids(&desc, &mut ids);
            if !ids.is_empty() {
                let mut target = Node::elem(kinds::TARGET, span);
                target.attrs.ids = ids;
                out.push(target);
            }
            return;
        }
        out.push(desc);
    }

    /// The per-subclass `handle_signature`. Returns the object name, or
    /// `None` for the ValueError path — where `run` clears the signature
    /// node and drops the whole signature into one `desc_name` (`:259-263`),
    /// which each arm does itself.
    fn handle_object_signature(
        &mut self,
        kind: ObjectDescKind,
        sig: &str,
        signode: &mut Node,
    ) -> Option<String> {
        let span = signode.span;
        match kind {
            // The base `handle_signature` raises unconditionally (`:100-111`).
            ObjectDescKind::Describe => {
                signode.children.clear();
                signode.children.push(desc_name_node(sig, span));
                None
            }
            // `GenericObject.handle_signature` (`domains/std:56-64`).
            ObjectDescKind::EnvVar => {
                signode.children.clear();
                signode.children.push(desc_name_node(sig, span));
                Some(ws_collapse(sig, " "))
            }
            // `ConfigurationValue.handle_signature` (`domains/std:126-131`).
            ObjectDescKind::Confval => {
                signode.children.clear();
                signode.children.push(desc_name_node(sig, span));
                let name = ws_collapse(sig, " ");
                signode.set("fullname", AttrValue::Str(name.clone()));
                Some(name)
            }
            ObjectDescKind::Cmdoption => self.handle_option_signature(sig, signode),
        }
    }

    /// `Cmdoption.handle_signature` (`domains/std/__init__.py:229-290`) with
    /// `option_emphasise_placeholders` at its default False, which is the
    /// plain `desc_name` + `desc_addname` pair per spelling.
    fn handle_option_signature(&mut self, sig: &str, signode: &mut Node) -> Option<String> {
        let span = signode.span;
        let mut firstname: Option<String> = None;
        let mut allnames: Vec<String> = Vec::new();
        for potential in sig.split(", ") {
            let Some((optname, args)) = option_desc_match(potential.trim()) else {
                // `logger.warning('Malformed option description %r, should
                // look like "opt", "-opt args", ...')` goes to the warning
                // stream, which the parse layer has no sink for; the tree
                // effect is that this spelling contributes nothing.
                continue;
            };
            // "optional value surrounded by brackets (ex. foo[=bar])".
            let (optname, args) = match (optname.strip_suffix('['), args.strip_suffix(']')) {
                (Some(trimmed), Some(_)) => (trimmed.to_string(), format!("[{args}")),
                _ => (optname, args),
            };
            if firstname.is_some() {
                signode.children.push(desc_addname_node(", ", span));
            }
            signode.children.push(desc_name_node(&optname, span));
            signode.children.push(desc_addname_node(&args, span));
            firstname.get_or_insert_with(|| optname.clone());
            allnames.push(optname);
        }
        let firstname = match firstname {
            Some(name) => name,
            None => {
                signode.children.clear();
                signode.children.push(desc_name_node(sig, span));
                return None;
            }
        };
        signode.set("allnames", AttrValue::List(allnames));
        Some(firstname)
    }

    /// The per-subclass `add_target_and_index`: node ids through sphinx's
    /// `make_id`, the index entries, and (for options) the program-scoped
    /// registration the env layer replays.
    fn object_target_and_index(
        &mut self,
        kind: ObjectDescKind,
        objtype: &str,
        name: &str,
        signode: &mut Node,
        entries: &mut Vec<String>,
    ) {
        match kind {
            // `ObjectDescription.add_target_and_index` is `pass` (`:113-120`)
            // — no id, no index entry, no std object. (Unreachable in
            // practice: `Describe`'s handle_signature never returns a name.)
            ObjectDescKind::Describe => {}
            // `GenericObject.add_target_and_index` (`domains/std:66-84`).
            // `EnvVar.indextemplate` has no ':' separator, so the whole
            // template is a 'single' entry value.
            ObjectDescKind::EnvVar => {
                let node_id = self.note_object_id(objtype, name, signode);
                entries.push(index_entry_tuple(
                    "single",
                    &format!("environment variable; {name}"),
                    &node_id,
                    "",
                    None,
                ));
            }
            // `ConfigurationValue.add_target_and_index` (`domains/std:142-151`).
            ObjectDescKind::Confval => {
                let node_id = self.note_object_id(objtype, name, signode);
                entries.push(index_entry_tuple(
                    "pair",
                    &format!("{name}; configuration value"),
                    &node_id,
                    "",
                    None,
                ));
            }
            // `Cmdoption.add_target_and_index` (`domains/std:292-330`).
            ObjectDescKind::Cmdoption => {
                let program = self.program.clone();
                let allnames = match signode.get("allnames") {
                    Some(AttrValue::List(names)) => names.clone(),
                    _ => Vec::new(),
                };
                for optname in &allnames {
                    let mut prefix = String::from("cmdoption");
                    if let Some(program) = &program {
                        prefix.push('-');
                        prefix.push_str(program);
                    }
                    if !optname.starts_with(['-', '/']) {
                        prefix.push_str("-arg");
                    }
                    let node_id = self.registry.sphinx_make_id(&prefix, optname);
                    signode.attrs.ids.push(node_id);
                }
                // `note_explicit_target` runs once, AFTER every id is
                // chosen, so the ids of one signature never see each other
                // in `document.ids`.
                for node_id in signode.attrs.ids.clone() {
                    self.registry.note_explicit_id(&node_id);
                }
                // Every spelling registers against `signode['ids'][0]`.
                let first_id = signode.attrs.ids.first().cloned().unwrap_or_default();
                for optname in &allnames {
                    self.program_option_records
                        .push(super::ProgramOptionRecord {
                            program: program.clone(),
                            name: optname.clone(),
                            node_id: first_id.clone(),
                        });
                }
                let descr = match &program {
                    Some(program) => format!("{program} command line option"),
                    None => "command line option".to_string(),
                };
                for optname in &allnames {
                    entries.push(index_entry_tuple(
                        "pair",
                        &format!("{descr}; {optname}"),
                        &first_id,
                        "",
                        None,
                    ));
                }
            }
        }
    }

    /// The `make_id` + `note_explicit_target` pair the single-id
    /// `add_target_and_index` implementations share.
    fn note_object_id(&mut self, prefix: &str, name: &str, signode: &mut Node) -> String {
        let node_id = self.registry.sphinx_make_id(prefix, name);
        signode.attrs.ids.push(node_id.clone());
        self.registry.note_explicit_id(&node_id);
        node_id
    }

    /// `ConfigurationValue.transform_content` (`domains/std:153-185`):
    /// `:type:` and `:default:` render as a field list prepended to the
    /// description content, each field followed by its own inline messages.
    fn confval_transform_content(&mut self, input: &DirectiveInput<'a, '_>, content: &mut Node) {
        let mut field_list = Node::elem(kinds::FIELD_LIST, input.span);
        for (option, label) in [("type", "Type"), ("default", "Default")] {
            let Some(OptVal::Str(value)) = opt_get(&input.options, option) else {
                continue;
            };
            let parsed = self.inline(&value.clone(), input.span, input.lineno);
            let mut field_name = Node::elem(kinds::FIELD_NAME, input.span);
            field_name.children.push(Node::text_node(label, input.span));
            let mut field_body = Node::elem(kinds::FIELD_BODY, input.span);
            field_body.children = parsed.nodes;
            let mut field = Node::elem(kinds::FIELD, input.span);
            field.children.push(field_name);
            field.children.push(field_body);
            field_list.children.push(field);
            field_list.children.extend(parsed.messages);
        }
        if !field_list.children.is_empty() {
            content.children.insert(0, field_list);
        }
    }

    /// versionadded family (sphinx/domains/changeset.py VersionChange):
    /// a versionmodified node holding ONE translatable="0" paragraph whose
    /// lead-in inline ends with '.' (no text) or ': ' (text follows as
    /// siblings in the same paragraph).
    fn run_version_change(
        &mut self,
        info: &'static (&'static str, &'static str, &'static str),
        input: DirectiveInput<'a, '_>,
        out: &mut Vec<Node>,
    ) {
        let (type_name, label, lead_fmt) = *info;
        let version = &input.arguments[0];
        // Inline messages from the explanation must anchor on the text's
        // own line, not the directive marker (review finding 41).
        let mut text_lineno = input.lineno;
        let text: Option<String> = input
            .arguments
            .get(1)
            .cloned()
            .or_else(|| {
                if input.content.is_empty() {
                    None
                } else {
                    text_lineno = input.content[0].lineno;
                    Some(
                        input
                            .content
                            .iter()
                            .map(|l| l.text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }
            })
            .filter(|t| !t.is_empty());
        let mut node = Node::elem("versionmodified", input.span);
        node.set("type", AttrValue::Str(type_name.to_string()));
        node.set("version", AttrValue::Str(version.clone()));
        let lead_base = lead_fmt.replace("{}", version);
        let lead = match &text {
            Some(_) => format!("{lead_base}: "),
            None => format!("{lead_base}."),
        };
        let mut para = Node::elem(kinds::PARAGRAPH, input.span);
        para.set("translatable", AttrValue::Int(0));
        let mut inner = Node::elem("inline", input.span);
        inner
            .attrs
            .classes
            .extend(["versionmodified".to_string(), label.to_string()]);
        inner.children.push(Node::text_node(lead, input.span));
        para.children.push(inner);
        let mut messages = Vec::new();
        if let Some(t) = text {
            let inline = self.inline(&t, input.span, text_lineno);
            para.children.extend(inline.nodes);
            messages = inline.messages;
        }
        node.children.push(para);
        out.push(node);
        out.extend(messages);
    }

    /// seealso (sphinx/directives/other.py): admonition-shaped custom
    /// node with no attributes.
    fn run_seealso(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let mut node = Node::elem("seealso", input.span);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        let content = self.parse_nested(&input.content, "seealso");
        node.children.extend(content);
        out.push(node);
    }

    /// sphinx code-block (sphinx/directives/code.py CodeBlock): language
    /// falls back to the `.. highlight::` state then the 'default'
    /// sentinel; :caption: wraps in a literal-block-wrapper container
    /// that takes the ids/names.
    fn run_sphinx_code_block(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let language = input
            .arguments
            .first()
            .cloned()
            .or_else(|| self.highlight_language.clone())
            .unwrap_or_else(|| "default".to_string());
        // sphinx util.parselinenos + CodeBlock.run: an invalid spec
        // REPLACES the whole block with a WARNING system_message;
        // out-of-range lines are filtered (review findings 31/43/45/46).
        let nlines = input.content.len() as i64;
        let mut hl_lines: Vec<i64> = Vec::new();
        if let Some(OptVal::Str(spec)) = opt_get(&input.options, "emphasize-lines") {
            match parse_linenos(spec, nlines) {
                Ok(lines_list) => hl_lines = lines_list,
                Err(msg) => {
                    out.push(self.msg(messages::WARNING, &msg, input.lineno));
                    return;
                }
            }
        }
        let highlight_args = if hl_lines.is_empty() {
            "{}".to_string()
        } else {
            format!(
                "{{'hl_lines': [{}]}}",
                hl_lines
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let mut lb = Node::elem(kinds::LITERAL_BLOCK, input.span);
        lb.set(
            "force",
            AttrValue::Int(i64::from(opt_get(&input.options, "force").is_some())),
        );
        lb.set("highlight_args", AttrValue::Str(highlight_args));
        lb.set("language", AttrValue::Str(language));
        if opt_get(&input.options, "linenos").is_some() {
            lb.set("linenos", AttrValue::Int(1));
        }
        lb.set("xml:space", AttrValue::Str("preserve".to_string()));
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            lb.attrs.classes.extend(classes.iter().cloned());
        }
        let code = input
            .content
            .iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n");
        lb.children.push(Node::text_node(code, input.span));
        match opt_get(&input.options, "caption") {
            Some(OptVal::Str(caption_text)) => {
                let mut container = Node::elem("container", input.span);
                container
                    .attrs
                    .classes
                    .push("literal-block-wrapper".to_string());
                container.set("literal_block", AttrValue::Int(1));
                self.directive_add_name(&mut container, &input.options, input.lineno, out);
                let inline = self.inline(&caption_text.clone(), input.span, input.lineno);
                let mut caption = Node::elem("caption", input.span);
                caption.children = inline.nodes;
                container.children.push(caption);
                container.children.push(lb);
                out.push(container);
                out.extend(inline.messages);
            }
            _ => {
                self.directive_add_name(&mut lb, &input.options, input.lineno, out);
                out.push(lb);
            }
        }
    }

    /// sphinx highlight: emits a highlightlang node AND sets the state
    /// later code-blocks read (env.temp_data parity).
    fn run_highlight(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let lang = input.arguments[0].clone();
        self.highlight_language = Some(lang.clone());
        let mut node = Node::elem("highlightlang", input.span);
        node.set(
            "force",
            AttrValue::Int(i64::from(opt_get(&input.options, "force").is_some())),
        );
        node.set("lang", AttrValue::Str(lang));
        let threshold = match opt_get(&input.options, "linenothreshold") {
            Some(OptVal::Int(n)) => *n,
            _ => i64::MAX,
        };
        node.set("linenothreshold", AttrValue::Int(threshold));
        out.push(node);
    }

    /// sphinx only: expr stored verbatim; evaluation is a later build
    /// phase.
    fn run_only(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let mut node = Node::elem("only", input.span);
        node.set("expr", AttrValue::Str(input.arguments[0].clone()));
        let content = self.parse_nested(&input.content, "only");
        node.children.extend(content);
        out.push(node);
    }

    /// sphinx toctree: entries recorded (as authored, with per-entry
    /// lines) for the build pipeline; the node is a best-effort
    /// `compound.toctree-wrapper > toctree` (probe shape; exact attr
    /// parity lands with the sphinx-fixture toctree cases).
    fn run_toctree(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let glob = matches!(opt_get(&input.options, "glob"), Some(OptVal::Null));
        let mut entries: Vec<super::ToctreeEntryRecord> = Vec::new();
        let mut raw_entries: Vec<String> = Vec::new();
        for l in &input.content {
            if l.is_blank() {
                continue;
            }
            let t = l.text.trim();
            raw_entries.push(t.to_string());
            // sphinx explicit_title_re `^(.+?)\s*<(.*?)>$`: the TITLE part
            // must be nonempty — a bare `<foo>` entry is a literal target
            // named '<foo>' (review finding 40).
            let (title, target) = match crate::env::toctree::split_explicit_title(t) {
                Some((title, target)) => (Some(title.to_string()), target.to_string()),
                None => (None, t.to_string()),
            };
            entries.push(super::ToctreeEntryRecord {
                title,
                target,
                line: l.lineno,
            });
        }
        // Full sphinx attr set (oracle-pinned). entries/includefiles are
        // resolved against the environment's document set the way
        // `TocTree.parse_content` does — including its warnings, which ride
        // the record to the builder; a parse with no environment
        // (`found_docs: None`) resolves nothing and leaves both empty.
        let resolved = match &self.found_docs {
            Some(found) => {
                crate::env::toctree::resolve_entries(&crate::env::toctree::ToctreeContent {
                    content: &raw_entries,
                    docname: &self.docname,
                    glob,
                    reversed: opt_get(&input.options, "reversed").is_some(),
                    line: input.lineno,
                    found_docs: found,
                    source_suffixes: SOURCE_SUFFIXES,
                    exclude_patterns: &self.exclude_patterns,
                })
            }
            None => crate::env::toctree::ResolvedEntries::default(),
        };
        self.toctree_records.push(super::ToctreeRecord {
            glob,
            entries: entries.clone(),
            line: input.lineno,
            warnings: resolved.warnings.clone(),
        });
        let mut toctree = Node::elem("toctree", input.span);
        match opt_get(&input.options, "caption") {
            // pformat renders a Python None attr value as "True".
            Some(OptVal::Str(c)) => toctree.set("caption", AttrValue::Str(c.clone())),
            _ => toctree.set("caption", AttrValue::Str("True".to_string())),
        }
        toctree.set("entries", resolved.entries_attr());
        toctree.set("glob", AttrValue::Int(i64::from(glob)));
        toctree.set(
            "hidden",
            AttrValue::Int(i64::from(opt_get(&input.options, "hidden").is_some())),
        );
        toctree.set("includefiles", resolved.includefiles_attr());
        toctree.set(
            "includehidden",
            AttrValue::Int(i64::from(
                opt_get(&input.options, "includehidden").is_some(),
            )),
        );
        let maxdepth = match opt_get(&input.options, "maxdepth") {
            Some(OptVal::Int(d)) => *d,
            _ => -1,
        };
        toctree.set("maxdepth", AttrValue::Int(maxdepth));
        // sphinx `int_or_nothing` (directives/other.py:36): a bare
        // `:numbered:` is depth 999, not 999_999.
        let numbered = match opt_get(&input.options, "numbered") {
            Some(OptVal::Str(s)) if s.is_empty() => 999,
            Some(OptVal::Str(s)) => py_int(s).unwrap_or(0),
            _ => 0,
        };
        toctree.set("numbered", AttrValue::Int(numbered));
        toctree.set("parent", AttrValue::Str(self.docname.clone()));
        toctree.set("rawentries", AttrValue::Str(String::new()));
        toctree.set(
            "titlesonly",
            AttrValue::Int(i64::from(opt_get(&input.options, "titlesonly").is_some())),
        );
        let mut compound = Node::elem("compound", input.span);
        compound.attrs.classes.push("toctree-wrapper".to_string());
        compound.children.push(toctree);
        out.push(compound);
    }

    fn substitution_context_error(
        &self,
        input: &DirectiveInput<'a, '_>,
        out: &mut Vec<Node>,
    ) -> bool {
        if self.substitution_ctx.is_some() {
            return false;
        }
        out.push(self.directive_run_error(
            &format!(
                "Invalid context: the \"{}\" directive can only be used within a substitution definition.",
                input.name
            ),
            input.lineno,
            input.rawsource,
        ));
        true
    }

    /// replace (misc.py:357-387).
    fn run_replace(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if self.substitution_context_error(&input, out) {
            return;
        }
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        // The nested parse runs OUTSIDE the SubstitutionDef state: an
        // embedded `.. date::` inside replace content must context-error
        // exactly like at body level (review finding 22).
        let saved_ctx = self.substitution_ctx.take();
        let children = self.parse_nested(&input.content, "substitution_definition");
        self.substitution_ctx = saved_ctx;
        let mut msgs: Vec<Node> = Vec::new();
        let mut paragraphs: Vec<Node> = Vec::new();
        let mut others = false;
        for c in children {
            if c.kind == kinds::SYSTEM_MESSAGE {
                let mut m = c;
                m.attrs.backrefs.clear();
                msgs.push(m);
            } else if c.kind == kinds::PARAGRAPH {
                paragraphs.push(c);
            } else {
                others = true;
            }
        }
        if paragraphs.len() == 1 && !others {
            out.extend(msgs);
            out.extend(paragraphs.remove(0).children);
        } else {
            // reporter.error without a literal child (misc.py:378-383).
            out.push(self.msg(
                messages::ERROR,
                &format!(
                    "Error in \"{}\" directive: may contain a single paragraph only.",
                    input.name
                ),
                input.lineno,
            ));
        }
    }

    /// unicode (misc.py:390-431).
    fn run_unicode(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if self.substitution_context_error(&input, out) {
            return;
        }
        let trim = opt_get(&input.options, "trim").is_some();
        let ltrim = opt_get(&input.options, "ltrim").is_some();
        let rtrim = opt_get(&input.options, "rtrim").is_some();
        if let Some(ctx) = self.substitution_ctx.as_mut() {
            ctx.ltrim |= trim || ltrim;
            ctx.rtrim |= trim || rtrim;
        }
        let arg = &input.arguments[0];
        let codes_text = &arg[..unicode_comment_cut(arg)];
        for code in codes_text.split_whitespace() {
            match unicode_code(code) {
                Ok(s) => out.push(Node::text_node(s, input.span)),
                Err(detail) => {
                    out.push(self.directive_run_error(
                        &format!("Invalid character code: {code}\nValueError: {detail}"),
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
            }
        }
    }

    /// date (misc.py:639-666): strftime at PARSE time (deliberately
    /// non-deterministic output; the fixture corpus avoids success cases).
    fn run_date(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if self.substitution_context_error(&input, out) {
            return;
        }
        let format = if input.content.is_empty() {
            "%Y-%m-%d".to_string()
        } else {
            input
                .content
                .iter()
                .map(|l| l.text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        out.push(Node::text_node(strftime_now(&format), input.span));
    }

    /// Table.make_title (tables.py:46-57).
    fn table_make_title(&mut self, input: &DirectiveInput<'a, '_>) -> (Option<Node>, Vec<Node>) {
        match input.arguments.first() {
            Some(text) => {
                let inline = self.inline(text, input.span, input.lineno);
                let mut title = Node::elem(kinds::TITLE, input.span);
                title.children = inline.nodes;
                (Some(title), inline.messages)
            }
            None => (None, Vec::new()),
        }
    }

    /// Shared tail of the three table directives: user classes, width,
    /// align, the colwidths marker class, :name:, then the title at
    /// index 0 (tables.py:141-171).
    #[allow(clippy::too_many_arguments)]
    fn finish_table(
        &mut self,
        mut table: Node,
        input: &DirectiveInput<'a, '_>,
        title: Option<Node>,
        title_messages: Vec<Node>,
        out: &mut Vec<Node>,
    ) {
        if let Some(OptVal::Str(w)) = opt_get(&input.options, "width") {
            table.set("width", AttrValue::Str(w.clone()));
        }
        if let Some(OptVal::Str(a)) = opt_get(&input.options, "align") {
            table.set("align", AttrValue::Str(a.clone()));
        }
        self.directive_add_name(&mut table, &input.options, input.lineno, out);
        if let Some(t) = title {
            table.children.insert(0, t);
        }
        out.push(table);
        out.extend(title_messages);
    }

    /// table (tables.py RSTTable:127-172).
    fn run_rst_table(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            // RSTTable's missing-content diagnostic is a WARNING, unlike
            // the assert_has_content ERROR family (tables.py:135-139).
            out.push(self.directive_run_message(
                messages::WARNING,
                &format!(
                    "Content block expected for the \"{}\" directive; none found.",
                    input.name
                ),
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let (title, title_messages) = self.table_make_title(&input);
        let children = self.parse_nested(&input.content, "element");
        if children.len() != 1 || children[0].kind != kinds::TABLE {
            out.push(self.directive_run_error(
                &format!(
                    "Error parsing content block for the \"{}\" directive: exactly one table expected.",
                    input.name
                ),
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let mut table = children.into_iter().next().expect("length checked");
        // User classes precede the colwidths marker class here (RSTTable
        // run order); csv/list get theirs appended AFTER the build-time
        // marker instead — both orders fixture-pinned.
        if let Some(OptVal::StrList(cls)) = opt_get(&input.options, "class") {
            table.attrs.classes.extend(cls.iter().cloned());
        }
        match opt_get(&input.options, "widths") {
            Some(OptVal::Str(kw)) if kw == "auto" => {
                table.attrs.classes.push("colwidths-auto".to_string());
            }
            Some(OptVal::Str(_)) => {
                // 'grid': keep the syntax-derived colwidths.
                table.attrs.classes.push("colwidths-given".to_string());
            }
            Some(OptVal::IntList(list)) => {
                let n_cols = table
                    .children
                    .first()
                    .map(|tg| {
                        tg.children
                            .iter()
                            .filter(|c| c.kind == kinds::COLSPEC)
                            .count()
                    })
                    .unwrap_or(0);
                if list.len() != n_cols {
                    out.push(self.directive_run_error(
                        &format!(
                            "\"{}\" widths do not match the number of columns in table ({}).",
                            input.name, n_cols
                        ),
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
                if let Some(tg) = table.children.first_mut() {
                    let mut i = 0usize;
                    for c in &mut tg.children {
                        if c.kind == kinds::COLSPEC {
                            c.set("colwidth", AttrValue::Int(list[i]));
                            i += 1;
                        }
                    }
                }
                table.attrs.classes.push("colwidths-given".to_string());
            }
            _ => {}
        }
        self.finish_table(table, &input, title, title_messages, out);
    }

    /// csv-table (tables.py CSVTable:175-403).
    fn run_csv_table(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let has_file = opt_get(&input.options, "file").is_some();
        let has_url = opt_get(&input.options, "url").is_some();
        // get_csv_data (tables.py:321-388).
        let csv_text: String;
        if !input.content.is_empty() {
            if has_file || has_url {
                out.push(self.directive_run_error(
                    &format!(
                        "\"{}\" directive may not both specify an external file and have content.",
                        input.name
                    ),
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
            csv_text = input
                .content
                .iter()
                .map(|l| l.text)
                .collect::<Vec<_>>()
                .join("\n");
        } else if has_file {
            if has_url {
                out.push(self.directive_run_error(
                    &format!(
                        "The \"file\" and \"url\" options may not be simultaneously specified for the \"{}\" directive.",
                        input.name
                    ),
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
            let Some(OptVal::Str(path)) = opt_get(&input.options, "file") else {
                unreachable!("file option is Path-converted");
            };
            let base = std::path::Path::new(self.source_path)
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            match std::fs::read_to_string(base.join(path)) {
                Ok(t) => csv_text = t,
                Err(_) => {
                    // Unlike raw's io.error_string (InputError: prefix),
                    // the csv path formats the bare OSError.
                    out.push(self.directive_run_message(
                        messages::SEVERE,
                        &format!(
                            "Problems with \"{}\" directive path:\n[Errno 2] No such file or directory: {}.",
                            input.name,
                            py_repr(Some(path))
                        ),
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
            }
        } else {
            out.push(self.directive_run_message(
                messages::WARNING,
                &format!(
                    "The \"{}\" directive requires content; none supplied.",
                    input.name
                ),
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let (title, title_messages) = self.table_make_title(&input);
        // Dialect (tables.py DocutilsDialect:198-220).
        let delim = match opt_get(&input.options, "delim") {
            Some(OptVal::Str(s)) => s.chars().next().unwrap_or(','),
            _ => ',',
        };
        let quote = match opt_get(&input.options, "quote") {
            Some(OptVal::Str(s)) => s.chars().next().unwrap_or('"'),
            _ => '"',
        };
        let escape = match opt_get(&input.options, "escape") {
            Some(OptVal::Str(s)) => s.chars().next(),
            _ => None,
        };
        let skipinitialspace = opt_get(&input.options, "keepspace").is_none();
        let doublequote = escape.is_none();
        let header_rows = match opt_get(&input.options, "header-rows") {
            Some(OptVal::Int(n)) => *n as usize,
            _ => 0,
        };
        let stub_columns = match opt_get(&input.options, "stub-columns") {
            Some(OptVal::Int(n)) => *n as usize,
            _ => 0,
        };
        let header_option_rows: Vec<Vec<String>> = match opt_get(&input.options, "header") {
            Some(OptVal::Str(h)) => {
                parse_csv_text(h, delim, quote, escape, doublequote, skipinitialspace)
            }
            _ => Vec::new(),
        };
        let rows = parse_csv_text(
            &csv_text,
            delim,
            quote,
            escape,
            doublequote,
            skipinitialspace,
        );
        let max_header_cols = header_option_rows.iter().map(Vec::len).max().unwrap_or(0);
        let max_cols = rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(max_header_cols);
        let row_lens: Vec<usize> = rows.iter().map(Vec::len).collect();
        if let Err(msg) = Self::check_table_dimensions(
            input.name,
            rows.len(),
            &row_lens,
            header_rows,
            stub_columns,
        ) {
            out.push(self.directive_run_error(&msg, input.lineno, input.rawsource));
            return;
        }
        // Column widths (tables.py:101-118).
        let widths_opt = opt_get(&input.options, "widths").cloned();
        let col_widths: Vec<i64> = match &widths_opt {
            Some(OptVal::IntList(list)) => {
                if list.len() != max_cols {
                    out.push(self.directive_run_error(
                        &format!(
                            "\"{}\" widths do not match the number of columns in table ({}).",
                            input.name, max_cols
                        ),
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
                list.clone()
            }
            _ => {
                if max_cols == 0 {
                    out.push(self.directive_run_error(
                        "No table data detected in CSV file.",
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
                vec![(100 / max_cols) as i64; max_cols]
            }
        };
        // Cells -> entry nodes; short rows extend with empty cells.
        let mut make_row = |cells: &[String]| -> Vec<Node> {
            let mut entries = Vec::with_capacity(max_cols);
            for i in 0..max_cols {
                let mut entry = Node::elem(kinds::ENTRY, input.span);
                if let Some(cell) = cells.get(i) {
                    if !cell.is_empty() {
                        entry.children = self.parse_detached(cell, input.lineno, "entry");
                    }
                }
                entries.push(entry);
            }
            entries
        };
        let mut head: Vec<Vec<Node>> = Vec::new();
        let mut body: Vec<Vec<Node>> = Vec::new();
        for cells in &header_option_rows {
            head.push(make_row(cells));
        }
        for (i, cells) in rows.iter().enumerate() {
            if i < header_rows {
                head.push(make_row(cells));
            } else {
                body.push(make_row(cells));
            }
        }
        let mut table = Self::build_directive_table(
            &col_widths,
            stub_columns,
            widths_opt.as_ref(),
            head,
            body,
            input.span,
        );
        if let Some(OptVal::StrList(cls)) = opt_get(&input.options, "class") {
            table.attrs.classes.extend(cls.iter().cloned());
        }
        self.finish_table(table, &input, title, title_messages, out);
    }

    /// list-table (tables.py ListTable:406-523).
    fn run_list_table(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_run_error(
                &format!(
                    "The \"{}\" directive is empty; content required.",
                    input.name
                ),
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let (title, title_messages) = self.table_make_title(&input);
        let children = self.parse_nested(&input.content, "element");
        let content_error = |me: &Self, detail: &str| -> Node {
            me.directive_run_error(
                &format!(
                    "Error parsing content block for the \"{}\" directive: {detail}",
                    input.name
                ),
                input.lineno,
                input.rawsource,
            )
        };
        if children.len() != 1 || children[0].kind != kinds::BULLET_LIST {
            out.push(content_error(self, "exactly one bullet list expected."));
            return;
        }
        let outer = children.into_iter().next().expect("length checked");
        let mut table_data: Vec<Vec<Vec<Node>>> = Vec::new();
        let mut first_len: Option<usize> = None;
        for (i, item) in outer.children.into_iter().enumerate() {
            let one_inner_list =
                item.children.len() == 1 && item.children[0].kind == kinds::BULLET_LIST;
            if !one_inner_list {
                out.push(content_error(
                    self,
                    &format!(
                        "two-level bullet list expected, but row {} does not contain a second-level bullet list.",
                        i + 1
                    ),
                ));
                return;
            }
            let inner = item.children.into_iter().next().expect("length checked");
            let row: Vec<Vec<Node>> = inner.children.into_iter().map(|it| it.children).collect();
            if let Some(f) = first_len {
                if row.len() != f {
                    out.push(content_error(
                        self,
                        &format!(
                            "uniform two-level bullet list expected, but row {} does not contain the same number of items as row 1 ({} vs {}).",
                            i + 1,
                            row.len(),
                            f
                        ),
                    ));
                    return;
                }
            } else {
                first_len = Some(row.len());
            }
            table_data.push(row);
        }
        let header_rows = match opt_get(&input.options, "header-rows") {
            Some(OptVal::Int(n)) => *n as usize,
            _ => 0,
        };
        let stub_columns = match opt_get(&input.options, "stub-columns") {
            Some(OptVal::Int(n)) => *n as usize,
            _ => 0,
        };
        let row_lens: Vec<usize> = table_data.iter().map(Vec::len).collect();
        if let Err(msg) = Self::check_table_dimensions(
            input.name,
            table_data.len(),
            &row_lens,
            header_rows,
            stub_columns,
        ) {
            out.push(self.directive_run_error(&msg, input.lineno, input.rawsource));
            return;
        }
        let n_cols = first_len.unwrap_or(0);
        let widths_opt = opt_get(&input.options, "widths").cloned();
        let col_widths: Vec<i64> = match &widths_opt {
            Some(OptVal::IntList(list)) => {
                if list.len() != n_cols {
                    out.push(self.directive_run_error(
                        &format!(
                            "\"{}\" widths do not match the number of columns in table ({}).",
                            input.name, n_cols
                        ),
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
                list.clone()
            }
            _ => {
                if n_cols == 0 {
                    out.push(content_error(self, "exactly one bullet list expected."));
                    return;
                }
                vec![(100 / n_cols) as i64; n_cols]
            }
        };
        let mut all_rows: Vec<Vec<Node>> = Vec::new();
        for row in table_data {
            let entries: Vec<Node> = row
                .into_iter()
                .map(|cell_children| {
                    let mut entry = Node::elem(kinds::ENTRY, input.span);
                    entry.children = cell_children;
                    entry
                })
                .collect();
            all_rows.push(entries);
        }
        let body = all_rows.split_off(header_rows.min(all_rows.len()));
        let head = all_rows;
        let mut table = Self::build_directive_table(
            &col_widths,
            stub_columns,
            widths_opt.as_ref(),
            head,
            body,
            input.span,
        );
        if let Some(OptVal::StrList(cls)) = opt_get(&input.options, "class") {
            table.attrs.classes.extend(cls.iter().cloned());
        }
        self.finish_table(table, &input, title, title_messages, out);
    }

    /// check_table_dimensions (tables.py:59-91). Err = the message text.
    fn check_table_dimensions(
        name: &str,
        rows: usize,
        row_lens: &[usize],
        header_rows: usize,
        stub_columns: usize,
    ) -> Result<(), String> {
        if rows < header_rows {
            return Err(format!(
                "{header_rows} header row(s) specified but only {rows} row(s) of data supplied (\"{name}\" directive)."
            ));
        }
        if rows == header_rows && header_rows > 0 {
            return Err(format!(
                "Insufficient data supplied ({rows} row(s)); no data remaining for table body, required by \"{name}\" directive."
            ));
        }
        for len in row_lens {
            if *len < stub_columns {
                return Err(format!(
                    "{stub_columns} stub column(s) specified but only {len} columns(s) of data supplied (\"{name}\" directive)."
                ));
            }
            if *len == stub_columns && stub_columns > 0 {
                return Err(format!(
                    "Insufficient data supplied ({len} columns(s)); no data remaining for table body, required by \"{name}\" directive."
                ));
            }
        }
        Ok(())
    }

    /// build_table (states.py:1911-1953) for the csv/list table paths.
    fn build_directive_table(
        col_widths: &[i64],
        stub_columns: usize,
        widths_opt: Option<&OptVal>,
        head: Vec<Vec<Node>>,
        body: Vec<Vec<Node>>,
        span: Span,
    ) -> Node {
        let mut table = Node::elem(kinds::TABLE, span);
        match widths_opt {
            Some(OptVal::Str(kw)) if kw == "auto" => {
                table.attrs.classes.push("colwidths-auto".to_string());
            }
            Some(OptVal::IntList(_)) => {
                table.attrs.classes.push("colwidths-given".to_string());
            }
            _ => {}
        }
        let mut tgroup = Node::elem(kinds::TGROUP, span);
        tgroup.set("cols", AttrValue::Int(col_widths.len() as i64));
        for (i, w) in col_widths.iter().enumerate() {
            let mut colspec = Node::elem(kinds::COLSPEC, span);
            colspec.set("colwidth", AttrValue::Int(*w));
            if i < stub_columns {
                colspec.set("stub", AttrValue::Int(1));
            }
            tgroup.children.push(colspec);
        }
        let build_rows = |rows: Vec<Vec<Node>>| -> Vec<Node> {
            rows.into_iter()
                .map(|entries| {
                    let mut row = Node::elem(kinds::ROW, span);
                    row.children = entries;
                    row
                })
                .collect()
        };
        if !head.is_empty() {
            let mut thead = Node::elem(kinds::THEAD, span);
            thead.children = build_rows(head);
            tgroup.children.push(thead);
        }
        let mut tbody = Node::elem(kinds::TBODY, span);
        tbody.children = build_rows(body);
        tgroup.children.push(tbody);
        table.children.push(tgroup);
        table
    }

    /// topic + sidebar (body.py BasePseudoSection:21-96).
    fn run_pseudo_section(
        &mut self,
        kind: &'static str,
        input: DirectiveInput<'a, '_>,
        out: &mut Vec<Node>,
    ) {
        // Sidebar's own pre-checks run before the shared context check
        // (body.py:88-96).
        if kind == "sidebar" {
            if self.nested_node_kind == Some("sidebar") {
                out.push(self.directive_run_error(
                    &format!(
                        "The \"{}\" directive may not be used within a sidebar element.",
                        input.name
                    ),
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
            if opt_get(&input.options, "subtitle").is_some() && input.arguments.is_empty() {
                out.push(self.directive_run_error(
                    "The \"subtitle\" option may not be used without a title.",
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
        }
        // BasePseudoSection context check: allowed parents are the document
        // root, sections, and sidebars (body.py:33-40).
        if let Some(parent) = self.nested_node_kind {
            if parent != "sidebar" {
                out.push(self.directive_run_error(
                    &format!(
                        "The \"{}\" directive may not be used within topics or body elements.",
                        input.name
                    ),
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
        }
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let mut node = Node::elem(kind, input.span);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        let mut title_messages: Vec<Node> = Vec::new();
        if let Some(title_text) = input.arguments.first() {
            let inline = self.inline(title_text, input.span, input.lineno);
            let mut title = Node::elem(kinds::TITLE, input.span);
            title.children = inline.nodes;
            node.children.push(title);
            title_messages.extend(inline.messages);
            if let Some(OptVal::Str(subtitle_text)) = opt_get(&input.options, "subtitle") {
                let sub_inline = self.inline(subtitle_text, input.span, input.lineno);
                let mut subtitle = Node::elem(kinds::SUBTITLE, input.span);
                subtitle.children = sub_inline.nodes;
                node.children.push(subtitle);
                title_messages.extend(sub_inline.messages);
            }
        }
        node.children.append(&mut title_messages);
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        let content = self.parse_nested(&input.content, kind);
        node.children.extend(content);
        out.push(node);
    }

    /// rubric (body.py:240-254): inline children, no paragraph wrapper,
    /// inline messages as siblings after the node.
    fn run_rubric(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let inline = self.inline(&input.arguments[0], input.span, input.lineno);
        let mut node = Node::elem("rubric", input.span);
        node.children = inline.nodes;
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        out.push(node);
        out.extend(inline.messages);
    }

    /// epigraph / highlights / pull-quote (body.py:257-283): standard
    /// block-quote elements, each block_quote stamped with the class.
    fn run_quote_class(
        &mut self,
        class: &'static str,
        input: DirectiveInput<'a, '_>,
        out: &mut Vec<Node>,
    ) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let mut elements = self.block_quote_elements(&input.content, input.span);
        for el in &mut elements {
            if el.kind == kinds::BLOCK_QUOTE {
                el.attrs.classes.push(class.to_string());
            }
        }
        out.extend(elements);
    }

    /// compound (body.py:286-301).
    fn run_compound(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let mut node = Node::elem("compound", input.span);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        let content = self.parse_nested(&input.content, "compound");
        node.children.extend(content);
        out.push(node);
    }

    /// container (body.py:304-329): classes come from the ARGUMENT.
    fn run_container(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let mut classes: Vec<String> = Vec::new();
        if let Some(arg) = input.arguments.first() {
            match convert_option(Conv::ClassOption, Some(arg)) {
                Ok(OptVal::StrList(list)) => classes = list,
                _ => {
                    out.push(self.directive_run_error(
                        &format!(
                            "Invalid class attribute value for \"{}\" directive: \"{}\".",
                            input.name, arg
                        ),
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
            }
        }
        let mut node = Node::elem("container", input.span);
        node.attrs.classes.extend(classes);
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        let content = self.parse_nested(&input.content, "container");
        node.children.extend(content);
        out.push(node);
    }

    /// parsed-literal (body.py:132-146): full inline parse inside a
    /// whitespace-preserving literal_block; messages follow the node.
    fn run_parsed_literal(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let text = input
            .content
            .iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n");
        let inline = self.inline(&text, input.span, input.lineno);
        let mut node = Node::elem(kinds::LITERAL_BLOCK, input.span);
        node.set("xml:space", AttrValue::Str("preserve".to_string()));
        node.children = inline.nodes;
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        out.push(node);
        out.extend(inline.messages);
    }

    /// DirectiveError-style message (raised by a directive's own run()):
    /// message text VERBATIM — no 'Error in "X" directive:' prefix — plus
    /// the raw block as a literal_block child (states.py:2287-2291).
    fn directive_run_message(&self, level: u8, text: &str, lineno: u32, rawsource: &str) -> Node {
        messages::with_literal(self.msg(level, text, lineno), rawsource)
    }

    fn directive_run_error(&self, text: &str, lineno: u32, rawsource: &str) -> Node {
        self.directive_run_message(messages::ERROR, text, lineno, rawsource)
    }

    /// assert_has_content() (rst/__init__.py:370-377).
    fn directive_content_error(&self, name: &str, lineno: u32, rawsource: &str) -> Node {
        self.directive_run_error(
            &format!("Content block expected for the \"{name}\" directive; none found."),
            lineno,
            rawsource,
        )
    }

    /// add_name() (rst/__init__.py:379-389): the :name: option registers an
    /// explicit target on the node.
    fn directive_add_name(
        &mut self,
        node: &mut Node,
        options: &[(String, OptVal)],
        lineno: u32,
        out: &mut Vec<Node>,
    ) {
        if let Some(OptVal::Str(n)) = opt_get(options, "name") {
            node.attrs.names.push(ids::fully_normalize_name(n));
            let msg = self
                .registry
                .set_id_explicit(node, lineno, self.source_path, true, None);
            if let Some(m) = msg {
                out.push(m);
            }
        }
    }

    fn run_admonition(
        &mut self,
        kind: &'static str,
        input: DirectiveInput<'a, '_>,
        out: &mut Vec<Node>,
    ) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let mut node = Node::elem(kind, input.span);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        let content = self.parse_nested(&input.content, kind);
        node.children.extend(content);
        out.push(node);
    }

    fn run_generic_admonition(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let title_text = input.arguments[0].clone();
        let mut node = Node::elem("admonition", input.span);
        match opt_get(&input.options, "class") {
            Some(OptVal::StrList(classes)) => {
                node.attrs.classes.extend(classes.iter().cloned());
            }
            _ => {
                // Auto class from the title, only without :class:
                // (admonitions.py:44-46).
                node.attrs
                    .classes
                    .push(format!("admonition-{}", ids::make_id(&title_text)));
            }
        }
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        let inline = self.inline(&title_text, input.span, input.lineno);
        let mut title = Node::elem(kinds::TITLE, input.span);
        title.children = inline.nodes;
        node.children.push(title);
        for m in inline.messages {
            node.children.push(m);
        }
        let content = self.parse_nested(&input.content, "admonition");
        node.children.extend(content);
        out.push(node);
    }

    fn run_image(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        match self.build_image(&input, out) {
            Ok(node) => out.push(node),
            Err(msg) => out.push(*msg),
        }
    }

    /// images.py Image.run(): builds the image node (possibly wrapped in a
    /// reference); Err carries the system_message. Shared with figure.
    fn build_image(
        &mut self,
        input: &DirectiveInput<'a, '_>,
        out: &mut Vec<Node>,
    ) -> Result<Node, Box<Node>> {
        // Two-stage :align: validation (images.py:53-63): the converter
        // accepted all six values; at body level only horizontal ones are
        // legal, inside a substitution definition only vertical ones. The
        // DirectiveError text CONTAINS its own 'Error in …' lead — the
        // machinery adds no prefix. Two spaces before 'Valid' are
        // docutils-verbatim.
        if let Some(OptVal::Str(align)) = opt_get(&input.options, "align") {
            let in_subst = self.substitution_ctx.is_some();
            let bad = if in_subst {
                matches!(align.as_str(), "left" | "center" | "right")
            } else {
                matches!(align.as_str(), "top" | "middle" | "bottom")
            };
            if bad {
                let (ctx_txt, valid) = if in_subst {
                    (
                        " within a substitution definition",
                        "\"top\", \"middle\", \"bottom\"",
                    )
                } else {
                    ("", "\"left\", \"center\", \"right\"")
                };
                return Err(Box::new(self.directive_run_error(
                    &format!(
                        "Error in \"{}\" directive: \"{}\" is not a valid value for the \"align\" option{}.  Valid values for \"align\" are: {}.",
                        input.name, align, ctx_txt, valid
                    ),
                    input.lineno,
                    input.rawsource,
                )));
            }
        }
        let uri = uri_from_argument(&input.arguments[0]);
        // :target: wraps the image in a reference (images.py:74-93).
        let mut reference: Option<Node> = None;
        if let Some(OptVal::Str(target)) = opt_get(&input.options, "target") {
            let mut node = Node::elem(kinds::REFERENCE, input.span);
            match parse_image_target(target) {
                ImageTarget::Refname { name, refname } => {
                    node.set("name", AttrValue::Str(name));
                    node.set("refname", AttrValue::Str(refname));
                }
                ImageTarget::Refuri(refuri) => {
                    node.set("refuri", AttrValue::Str(refuri));
                }
            }
            reference = Some(node);
        }
        let mut image = Node::elem("image", input.span);
        for (name, val) in &input.options {
            match (name.as_str(), val) {
                ("alt", OptVal::Str(v)) => image.set("alt", AttrValue::Str(v.clone())),
                ("height", OptVal::Str(v)) => image.set("height", AttrValue::Str(v.clone())),
                ("width", OptVal::Str(v)) => image.set("width", AttrValue::Str(v.clone())),
                ("align", OptVal::Str(v)) => image.set("align", AttrValue::Str(v.clone())),
                ("loading", OptVal::Str(v)) => image.set("loading", AttrValue::Str(v.clone())),
                ("scale", OptVal::Int(v)) => image.set("scale", AttrValue::Int(*v)),
                // Arbitrary-precision values carry the exact digit string.
                ("scale", OptVal::Str(v)) => image.set("scale", AttrValue::Str(v.clone())),
                ("class", OptVal::StrList(v)) => {
                    image.attrs.classes.extend(v.iter().cloned());
                }
                // `name`/`target` are consumed by add_name / the
                // reference wrapper.
                _ => {}
            }
        }
        image.set("uri", AttrValue::Str(uri));
        self.directive_add_name(&mut image, &input.options, input.lineno, out);
        Ok(match reference {
            Some(mut r) => {
                r.children.push(image);
                r
            }
            None => image,
        })
    }

    /// figure (images.py:110-186), plus sphinx's override (patches.py:33-56)
    /// which moves `:name:` from the inner image onto the figure itself.
    fn run_figure(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        // sphinx pops `name` before delegating to docutils, so the image
        // never sees it, and re-applies it to the figure node afterwards —
        // but only on the success path (a figure returned *with* an error
        // node, or an error alone, keeps no name at all).
        let name_on_figure = self.sphinx;
        let image_input = DirectiveInput {
            name: input.name,
            arguments: input.arguments.clone(),
            options: input
                .options
                .iter()
                .filter(|(n, _)| {
                    !matches!(n.as_str(), "figwidth" | "figclass" | "align")
                        && !(name_on_figure && n == "name")
                })
                .cloned()
                .collect(),
            content: Vec::new(),
            span: input.span,
            lineno: input.lineno,
            rawsource: input.rawsource,
        };
        let image_node = match self.build_image(&image_input, out) {
            Ok(n) => n,
            Err(msg) => {
                // Inner image error short-circuits: no <figure> at all.
                out.push(*msg);
                return;
            }
        };
        let mut figure = Node::elem("figure", input.span);
        match opt_get(&input.options, "figwidth") {
            // ':figwidth: image' needs PIL, which the oracle environment
            // lacks: silent no-op (images.py:150-159).
            Some(OptVal::Str(w)) if w == "image" => {}
            Some(OptVal::Str(w)) => figure.set("width", AttrValue::Str(w.clone())),
            _ => {}
        }
        if let Some(OptVal::StrList(cls)) = opt_get(&input.options, "figclass") {
            figure.attrs.classes.extend(cls.iter().cloned());
        }
        if let Some(OptVal::Str(a)) = opt_get(&input.options, "align") {
            figure.set("align", AttrValue::Str(a.clone()));
        }
        figure.children.push(image_node);
        if !input.content.is_empty() {
            let children = self.parse_nested(&input.content, "figure");
            let mut caption_done = false;
            let mut legend_children: Vec<Node> = Vec::new();
            for child in children {
                if caption_done {
                    legend_children.push(child);
                    continue;
                }
                if child.kind == kinds::TARGET {
                    figure.children.push(child);
                } else if child.kind == kinds::PARAGRAPH {
                    let mut caption = Node::elem("caption", input.span);
                    caption.children = child.children;
                    figure.children.push(caption);
                    caption_done = true;
                } else if child.kind == kinds::COMMENT && child.children.is_empty() {
                    caption_done = true;
                } else {
                    // Unlike other directives, the figure node is emitted
                    // BEFORE the error (images.py:176-181).
                    out.push(figure);
                    out.push(self.directive_run_error(
                        "Figure caption must be a paragraph or empty comment.",
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
            }
            if !legend_children.is_empty() {
                let mut legend = Node::elem("legend", input.span);
                legend.children = legend_children;
                figure.children.push(legend);
            }
        }
        if name_on_figure {
            // After the nested parse, exactly where sphinx calls it — the
            // caption's own targets are registered first.
            self.directive_add_name(&mut figure, &input.options, input.lineno, out);
        }
        out.push(figure);
    }

    /// code (body.py:149-211). The parity oracle runs docutils WITHOUT
    /// Pygments: a language argument fails the whole directive with a
    /// WARNING; language-less code emits a plain classes="code" literal.
    fn run_code(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        if !input.arguments.is_empty() {
            out.push(self.directive_run_message(
                messages::WARNING,
                "Cannot analyze code. Pygments package not found.",
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let number_lines = match opt_get(&input.options, "number-lines") {
            Some(OptVal::Str(v)) => {
                let raw = if v.is_empty() { "1" } else { v.as_str() };
                match py_int(raw) {
                    Some(n) => Some(n),
                    None => {
                        out.push(self.directive_run_error(
                            ":number-lines: with non-integer start value",
                            input.lineno,
                            input.rawsource,
                        ));
                        return;
                    }
                }
            }
            _ => None,
        };
        let code_lines: Vec<&str> = input.content.iter().map(|l| l.text).collect();
        let mut node = Node::elem(kinds::LITERAL_BLOCK, input.span);
        node.attrs.classes.push("code".to_string());
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        node.set("xml:space", AttrValue::Str("preserve".to_string()));
        match number_lines {
            Some(start) => {
                // NumberLines (docutils/utils/code_analyzer.py): a padded
                // 'ln' inline before every line.
                let endline = start.saturating_add(input.content.len() as i64);
                let width = endline.to_string().len();
                for (i, line) in code_lines.iter().enumerate() {
                    let lineno = start.saturating_add(i as i64);
                    let mut ln = Node::elem("inline", input.span);
                    ln.attrs.classes.push("ln".to_string());
                    ln.children
                        .push(Node::text_node(format!("{lineno:>width$} "), input.span));
                    node.children.push(ln);
                    let text = if i + 1 == code_lines.len() {
                        (*line).to_string()
                    } else {
                        format!("{line}\n")
                    };
                    node.children.push(Node::text_node(text, input.span));
                }
            }
            None => {
                node.children
                    .push(Node::text_node(code_lines.join("\n"), input.span));
            }
        }
        self.directive_add_name(&mut node, &input.options, input.lineno, out);
        out.push(node);
    }

    /// math (body.py:214-237): blank-line-separated blocks become sibling
    /// math_block nodes; :name: only lands on the first (options.pop).
    fn run_math(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let joined = input
            .content
            .iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n");
        let mut named = false;
        for block in joined.split("\n\n") {
            if block.is_empty() {
                continue;
            }
            let mut node = Node::elem("math_block", input.span);
            node.set("xml:space", AttrValue::Str("preserve".to_string()));
            if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
                node.attrs.classes.extend(classes.iter().cloned());
            }
            node.children.push(Node::text_node(block, input.span));
            if !named {
                self.directive_add_name(&mut node, &input.options, input.lineno, out);
                named = true;
            }
            out.push(node);
        }
    }

    /// raw (misc.py:270-354).
    fn run_raw(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let has_file = opt_get(&input.options, "file").is_some();
        let has_url = opt_get(&input.options, "url").is_some();
        let text: String;
        let mut source_attr: Option<String> = None;
        if !input.content.is_empty() {
            if has_file || has_url {
                out.push(self.directive_run_error(
                    &format!(
                        "\"{}\" directive may not both specify an external file and have content.",
                        input.name
                    ),
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
            text = input
                .content
                .iter()
                .map(|l| l.text)
                .collect::<Vec<_>>()
                .join("\n");
        } else if has_file {
            if has_url {
                out.push(self.directive_run_error(
                    &format!(
                        "The \"file\" and \"url\" options may not be simultaneously specified for the \"{}\" directive.",
                        input.name
                    ),
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
            let Some(OptVal::Str(path)) = opt_get(&input.options, "file") else {
                unreachable!("file option is Path-converted");
            };
            let base = std::path::Path::new(self.source_path)
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let full = base.join(path);
            match std::fs::read_to_string(&full) {
                Ok(t) => {
                    // docutils strips ONE trailing newline via rstrip
                    // hazard; keep verbatim minus trailing newline.
                    text = t.trim_end_matches('\n').to_string();
                    source_attr = Some(path.clone());
                }
                Err(_) => {
                    out.push(self.directive_run_message(
                        messages::SEVERE,
                        &format!(
                            "Problems with \"{}\" directive path:\nInputError: [Errno 2] No such file or directory: {}.",
                            input.name,
                            py_repr(Some(path))
                        ),
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
            }
        } else if has_url {
            // URL fetching is out of parse-layer scope; the corpus only
            // pins the mutual-exclusivity errors above.
            out.push(self.directive_run_message(
                messages::SEVERE,
                &format!(
                    "Problems with \"{}\" directive URL: fetching is not supported.",
                    input.name
                ),
                input.lineno,
                input.rawsource,
            ));
            return;
        } else {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let format = input.arguments[0]
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut node = Node::elem("raw", input.span);
        node.set("format", AttrValue::Str(format));
        node.set("xml:space", AttrValue::Str("preserve".to_string()));
        if let Some(src) = source_attr {
            node.set("source", AttrValue::Str(src));
        }
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        node.children.push(Node::text_node(text, input.span));
        out.push(node);
    }

    /// line-block directive (body.py:99-129): same tree as `|` syntax.
    fn run_line_block(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(input.name, input.lineno, input.rawsource));
            return;
        }
        let mut resolved: Vec<(usize, Vec<Node>)> = Vec::with_capacity(input.content.len());
        let mut lb_messages: Vec<Node> = Vec::new();
        let mut prev_depth = 0usize;
        for l in &input.content {
            if l.is_blank() {
                resolved.push((prev_depth, Vec::new()));
                continue;
            }
            let depth = l.indent();
            prev_depth = depth;
            let inline = self.inline(l.text.trim(), input.span, l.lineno);
            lb_messages.extend(inline.messages);
            resolved.push((depth, inline.nodes));
        }
        let mut block = build_line_block(&mut resolved, input.span, 0);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            block.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(&mut block, &input.options, input.lineno, out);
        out.push(block);
        out.append(&mut lb_messages);
    }

    /// class (misc.py:434-469): with content, classes apply directly to
    /// every top-level child; without, a pending node is emitted for the
    /// ClassAttribute transform.
    fn run_class(&mut self, input: DirectiveInput<'a, '_>, out: &mut Vec<Node>) {
        let class_values = match convert_option(Conv::ClassOption, Some(&input.arguments[0])) {
            Ok(OptVal::StrList(list)) => list,
            _ => {
                out.push(self.directive_run_error(
                    &format!(
                        "Invalid class attribute value for \"{}\" directive: \"{}\".",
                        input.name, input.arguments[0]
                    ),
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
        };
        if !input.content.is_empty() {
            let mut children = self.parse_nested(&input.content, "element");
            for child in &mut children {
                child.attrs.classes.extend(class_values.iter().cloned());
            }
            out.extend(children);
        } else if self.sphinx {
            // Sphinx's read phase runs ClassAttribute; the pending node
            // never survives into the doctree — stamp the next sibling.
            self.pending_classes = Some(class_values);
        } else {
            let mut pending = Node::elem("pending", input.span);
            let details = format!(
                ".. internal attributes:\n     .transform: docutils.transforms.misc.ClassAttribute\n     .details:\n       class: [{}]\n       directive: {}",
                class_values
                    .iter()
                    .map(|c| py_repr(Some(c)))
                    .collect::<Vec<_>>()
                    .join(", "),
                py_repr(Some(input.name)),
            );
            pending.children.push(Node::text_node(details, input.span));
            out.push(pending);
        }
    }

    /// `.. |name| directive::` substitution definitions
    /// (states.py:2140-2217 + the SubstitutionDef state 2806-2829).
    /// Returns true when consumed; false = malformed marker — the caller
    /// falls through to the comment path with `construct_error` set.
    fn parse_substitution_def(
        &mut self,
        lines: &[LineRef<'a>],
        pos: &mut usize,
        rest: &'a str,
        out: &mut Vec<Node>,
        construct_error: &mut Option<Node>,
    ) -> bool {
        let start = *pos;
        let lineno = lines[start].lineno;
        let (block, consumed, _indent, _term) = indented_block(lines, start + 1);
        let span = self.span_of(lines, start, start + consumed);
        // blocktext for message literals: the raw marker line + raw block
        // (trailing blanks already trimmed by indented_block).
        let mut bt: Vec<&str> = vec![lines[start].text];
        for l in &lines[start + 1..start + 1 + consumed] {
            bt.push(l.text);
        }
        let blocktext = bt.join("\n");

        // Marker scan: `|name|` possibly joined across adjacent block lines
        // (states.py:2151-2160). Failure at end-of-block = MarkupError.
        let mut acc: String = rest.to_string();
        let mut used = 0usize;
        let marker = loop {
            if let Some(m) = match_substitution_marker(&acc) {
                break m;
            }
            if used >= block.len() || block[used].is_blank() {
                *construct_error = Some(self.msg(
                    messages::WARNING,
                    "malformed substitution definition.",
                    lineno,
                ));
                return false;
            }
            acc.push(' ');
            acc.push_str(block[used].text.trim());
            used += 1;
        };
        *pos = start + 1 + consumed;
        // Remainder after the marker lives on ONE physical line: `rest`
        // when the marker was single-line, else the last joined block line.
        let (rem_slice, rem_lineno): (&'a str, u32) = if used == 0 {
            (&rest[marker.remainder_start..], lineno)
        } else {
            let last = block[used - 1];
            let trimmed = last.text.trim();
            let seg_start = acc.len() - trimmed.len();
            let within = marker.remainder_start.saturating_sub(seg_start);
            let base = last.text.len() - last.text.trim_start().len();
            (&last.text[base + within..], last.lineno)
        };
        let mut content_block: Vec<LineRef<'a>> = block[used..].to_vec();

        let subname_ws = ids::whitespace_normalize_name(&marker.name);
        // Missing contents (states.py:2168-2176).
        if rem_slice.trim().is_empty() && content_block.iter().all(|l| l.is_blank()) {
            out.push(messages::with_literal(
                self.msg(
                    messages::WARNING,
                    &format!(
                        "Substitution definition \"{}\" missing contents.",
                        marker.name
                    ),
                    lineno,
                ),
                &blocktext,
            ));
            self.warn_explicit_markup_end(lines, *pos, out);
            return true;
        }

        let mut subst = Node::elem("substitution_definition", span);
        subst.attrs.names.push(subname_ws.clone());

        // Locate the embedded-directive line: the marker remainder, else
        // the first non-blank content line (hanging-indent form).
        // raw_content_from tracks the BLOCK index where the directive's
        // continuation lines begin, for rawsource reconstruction with
        // original indentation (docutils strip_indent=False).
        let mut raw_content_from = used;
        let (dline, dlineno) = if !rem_slice.trim().is_empty() {
            (
                LineRef::new(
                    rem_slice.trim_start_matches(' '),
                    rem_lineno,
                    lines[start].src_start,
                    lines[start].src_end,
                ),
                rem_lineno,
            )
        } else {
            while content_block.first().map(|l| l.is_blank()).unwrap_or(false) {
                content_block.remove(0);
                raw_content_from += 1;
            }
            let first = content_block.remove(0);
            raw_content_from += 1;
            let dedent = first.indent();
            (first.dedented(dedent), first.lineno)
        };
        // Embedded directive marker: simplename + `::` + (space|EOL) — NO
        // optional space before `::` (SubstitutionDef state pattern).
        let mut produced: Vec<Node> = Vec::new();
        if let Some((dname, dfirst_rest)) = match_embedded_directive(dline.text) {
            let dblock = dedent_by_min(&content_block);
            // rawsource with ORIGINAL indentation (the nested state
            // machine's lines are strip_indent=False; fixture-verified).
            let embedded_raw = {
                let mut v: Vec<&str> = vec![dline.text];
                for l in &lines[start + 1 + raw_content_from..start + 1 + consumed] {
                    v.push(l.text);
                }
                v.join("\n")
            };
            let dfirst = {
                let t = dfirst_rest.trim_start_matches(' ');
                let offset = dline.text.len() - t.len();
                LineRef::new(
                    &dline.text[offset..],
                    dlineno,
                    dline.src_start,
                    dline.src_end,
                )
            };
            self.substitution_ctx = Some(SubstCtx::default());
            let saved_kind = self.nested_node_kind.replace("substitution_definition");
            self.run_directive_core(
                &dname,
                dfirst,
                &dblock,
                &embedded_raw,
                dlineno,
                span,
                vec![("alt".to_string(), OptVal::Str(subname_ws.clone()))],
                &mut produced,
            );
            self.nested_node_kind = saved_kind;
            let ctx = self.substitution_ctx.take().unwrap_or_default();
            if ctx.ltrim {
                subst.set("ltrim", AttrValue::Int(1));
            }
            if ctx.rtrim {
                subst.set("rtrim", AttrValue::Int(1));
            }
        }
        // Hoist non-inline children to the parent, in document order
        // (states.py:2184-2191); inline/Text stay in the definition.
        for n in produced {
            if n.kind == kinds::TEXT || is_inline_kind(n.kind) {
                subst.children.push(n);
            } else {
                out.push(n);
            }
        }
        // Problematic content check (states.py:2194-2201).
        if tree_any(&subst, &|n| n.kind == kinds::PROBLEMATIC) {
            let mut msg = self.msg(
                messages::ERROR,
                "Problematic content in substitution definition",
                lineno,
            );
            let mut lb = Node::elem(kinds::LITERAL_BLOCK, Span::ZERO);
            lb.set("xml:space", AttrValue::Str("preserve".to_string()));
            lb.children.push(Node::text_node(&blocktext, Span::ZERO));
            msg.children.push(lb);
            let mut bq = Node::elem(kinds::BLOCK_QUOTE, Span::ZERO);
            let mut para = Node::elem(kinds::PARAGRAPH, Span::ZERO);
            para.children = std::mem::take(&mut subst.children);
            bq.children.push(para);
            msg.children.push(bq);
            out.push(msg);
            self.warn_explicit_markup_end(lines, *pos, out);
            return true;
        }
        // Disallowed content (states.py:2219-2227).
        if let Some(phrase) = find_disallowed_in_substitution(&subst) {
            out.push(messages::with_literal(
                self.msg(
                    messages::ERROR,
                    &format!("{phrase} are not supported in a substitution definition."),
                    lineno,
                ),
                &blocktext,
            ));
            self.warn_explicit_markup_end(lines, *pos, out);
            return true;
        }
        // Empty or invalid (states.py:2203-2210).
        if subst.children.is_empty() {
            out.push(messages::with_literal(
                self.msg(
                    messages::WARNING,
                    &format!(
                        "Substitution definition \"{}\" empty or invalid.",
                        marker.name
                    ),
                    lineno,
                ),
                &blocktext,
            ));
            self.warn_explicit_markup_end(lines, *pos, out);
            return true;
        }
        // note_substitution_def (nodes.py:2056-2073): duplicate names are
        // case-sensitively compared; the error precedes the new node and
        // the OLD node loses its name (post-parse walk).
        if self.substitution_names_seen.contains(&subname_ws) {
            out.push(self.msg(
                messages::ERROR,
                &format!("Duplicate substitution definition name: \"{subname_ws}\"."),
                lineno,
            ));
            if !self.substitution_dupnames.contains(&subname_ws) {
                self.substitution_dupnames.push(subname_ws.clone());
            }
        } else {
            self.substitution_names_seen.push(subname_ws);
        }
        out.push(subst);
        self.warn_explicit_markup_end(lines, *pos, out);
        true
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

#[derive(Clone, Copy)]
enum DirectiveKind {
    /// note/warning/... : content-only, node kind = tagname.
    Admonition(&'static str),
    /// `.. admonition:: Title` with required title argument.
    GenericAdmonition,
    /// `.. image:: uri` (images.py Image).
    Image,
    /// topic / sidebar (body.py BasePseudoSection).
    PseudoSection(&'static str),
    Rubric,
    /// epigraph / highlights / pull-quote: block_quote + class.
    QuoteClass(&'static str),
    Compound,
    Container,
    ParsedLiteral,
    Figure,
    Code,
    MathBlock,
    Raw,
    LineBlockDir,
    ClassDir,
    RstTable,
    CsvTable,
    ListTable,
    Replace,
    UnicodeDir,
    DateDir,
    /// sphinx toctree (sphinx/directives/other.py TocTree).
    Toctree,
    /// versionadded/versionchanged/deprecated/versionremoved:
    /// (type name, label class, lead-in format).
    VersionChange(&'static (&'static str, &'static str, &'static str)),
    SeeAlso,
    /// sphinx code-block/sourcecode (sphinx/directives/code.py).
    SphinxCodeBlock,
    Highlight,
    Only,
    SphinxMath,
    IndexDir,
    HList,
    Glossary,
    /// `.. describe::`/`.. object::`, `.. envvar::`, `.. confval::`,
    /// `.. option::`/`.. cmdoption::` — sphinx `ObjectDescription.run`
    /// (`directives/__init__.py:183-314`) with a per-directive
    /// `handle_signature`/`add_target_and_index`.
    ObjectDesc(ObjectDescKind),
    /// `.. program::` (`domains/std/__init__.py:333-348`).
    ProgramDir,
    /// `.. default-domain::` (`directives/__init__.py:353-366`).
    DefaultDomainDir,
}

/// Which `ObjectDescription` subclass a `desc`-producing directive is.
#[derive(Clone, Copy, PartialEq)]
enum ObjectDescKind {
    /// The bare `ObjectDescription`, registered with docutils under
    /// `describe`/`object` (`directives/__init__.py:375-377`): its
    /// `handle_signature` always raises and its `add_target_and_index` is a
    /// no-op, so it emits desc anatomy with no ids, no index entries and no
    /// std-domain registration.
    Describe,
    /// `GenericObject` (`domains/std/__init__.py:50-88`) — `envvar` is the
    /// only one this crate registers.
    EnvVar,
    /// `ConfigurationValue` (`domains/std/__init__.py:115-185`).
    Confval,
    /// `Cmdoption` (`domains/std/__init__.py:226-330`).
    Cmdoption,
}

/// Sphinx-mode registry: overlays/extends the docutils-native table.
fn directive_spec_mode(lower: &str, sphinx: bool) -> Option<DirectiveSpec> {
    if sphinx {
        if let Some(s) = sphinx_directive_spec(lower) {
            return Some(s);
        }
    }
    directive_spec(lower)
}

/// Suffixes a toctree entry may spell out and still name a document
/// (sphinx `config.source_suffix`, whose default is `{'.rst': ...}`). These
/// are the extensions `SphinxBuilder::is_source_file` discovers.
const SOURCE_SUFFIXES: &[&str] = &[".rst", ".md", ".txt"];

const TOCTREE_OPTS: &[(&str, Conv)] = &[
    ("maxdepth", Conv::PyIntAny),
    ("name", Conv::Unchanged),
    ("class", Conv::ClassOption),
    ("caption", Conv::UnchangedRequired),
    ("glob", Conv::Flag),
    ("hidden", Conv::Flag),
    ("includehidden", Conv::Flag),
    ("numbered", Conv::Unchanged),
    ("titlesonly", Conv::Flag),
    ("reversed", Conv::Flag),
];

const VERSIONADDED: (&str, &str, &str) = ("versionadded", "added", "Added in version {}");
const VERSIONCHANGED: (&str, &str, &str) = ("versionchanged", "changed", "Changed in version {}");
const DEPRECATED: (&str, &str, &str) = ("deprecated", "deprecated", "Deprecated since version {}");
const VERSIONREMOVED: (&str, &str, &str) = ("versionremoved", "removed", "Removed in version {}");

const CODE_BLOCK_OPTS: &[(&str, Conv)] = &[
    ("force", Conv::Flag),
    ("linenos", Conv::Flag),
    ("dedent", Conv::PyIntAny),
    ("lineno-start", Conv::PyIntAny),
    ("emphasize-lines", Conv::UnchangedRequired),
    ("caption", Conv::UnchangedRequired),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
];

const HIGHLIGHT_OPTS: &[(&str, Conv)] =
    &[("linenothreshold", Conv::PyIntAny), ("force", Conv::Flag)];

/// sphinx.util.parselinenos: 1-based spec ('1,3-5', open ends '-4'/'4-')
/// against `nlines` total lines; invalid or reversed specs raise. Range
/// materialization is clamped to nlines so a huge upper bound cannot
/// blow memory (values past nlines are filtered anyway).
fn parse_linenos(spec: &str, nlines: i64) -> Result<Vec<i64>, String> {
    let invalid = || format!("invalid line number spec: {}", py_repr(Some(spec)));
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once('-') {
            let (a, b) = (a.trim(), b.trim());
            let start = if a.is_empty() {
                1
            } else {
                py_int(a).ok_or_else(invalid)?
            };
            let end = if b.is_empty() {
                nlines
            } else {
                py_int(b).ok_or_else(invalid)?
            };
            if start > end {
                return Err(invalid());
            }
            let clamped_end = end.min(nlines);
            let mut n = start.max(1);
            while n <= clamped_end {
                out.push(n);
                n += 1;
            }
        } else if !part.is_empty() {
            let n = py_int(part).ok_or_else(invalid)?;
            if n >= 1 && n <= nlines {
                out.push(n);
            }
        } else {
            return Err(invalid());
        }
    }
    Ok(out)
}

fn sphinx_directive_spec(lower: &str) -> Option<DirectiveSpec> {
    let version_change = |info: &'static (&'static str, &'static str, &'static str)| {
        Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 1,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: &[],
            kind: DirectiveKind::VersionChange(info),
        })
    };
    match lower {
        "toctree" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: TOCTREE_OPTS,
            kind: DirectiveKind::Toctree,
        }),
        "versionadded" => version_change(&VERSIONADDED),
        "versionchanged" => version_change(&VERSIONCHANGED),
        "deprecated" => version_change(&DEPRECATED),
        "versionremoved" => version_change(&VERSIONREMOVED),
        "seealso" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::SeeAlso,
        }),
        "code-block" | "sourcecode" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 1,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: CODE_BLOCK_OPTS,
            kind: DirectiveKind::SphinxCodeBlock,
        }),
        "highlight" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: false,
            option_spec: HIGHLIGHT_OPTS,
            kind: DirectiveKind::Highlight,
        }),
        "only" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: &[],
            kind: DirectiveKind::Only,
        }),
        "math" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 1,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: SPHINX_MATH_OPTS,
            kind: DirectiveKind::SphinxMath,
        }),
        "index" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: false,
            option_spec: NAME_ONLY_OPTS,
            kind: DirectiveKind::IndexDir,
        }),
        "hlist" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: HLIST_OPTS,
            kind: DirectiveKind::HList,
        }),
        "glossary" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: GLOSSARY_OPTS,
            kind: DirectiveKind::Glossary,
        }),
        "describe" | "object" => Some(object_desc_spec(ObjectDescKind::Describe)),
        "envvar" => Some(object_desc_spec(ObjectDescKind::EnvVar)),
        "confval" => Some(object_desc_spec(ObjectDescKind::Confval)),
        "option" | "cmdoption" => Some(object_desc_spec(ObjectDescKind::Cmdoption)),
        "program" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: false,
            option_spec: &[],
            kind: DirectiveKind::ProgramDir,
        }),
        "default-domain" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: false,
            option_spec: &[],
            kind: DirectiveKind::DefaultDomainDir,
        }),
        _ => None,
    }
}

/// `ObjectDescription`'s class-level directive shape
/// (`directives/__init__.py:51-63`): one whitespace-joined argument
/// (multiple signatures arrive as its embedded newlines) and content.
fn object_desc_spec(kind: ObjectDescKind) -> DirectiveSpec {
    DirectiveSpec {
        required_arguments: 1,
        optional_arguments: 0,
        final_argument_whitespace: true,
        has_content: true,
        // `ConfigurationValue` REPLACES the inherited option_spec: it adds
        // `:type:`/`:default:` and drops the three deprecated aliases
        // (`domains/std/__init__.py:117-124`).
        option_spec: match kind {
            ObjectDescKind::Confval => CONFVAL_OPTS,
            _ => OBJECT_DESCRIPTION_OPTS,
        },
        kind: DirectiveKind::ObjectDesc(kind),
    }
}

/// sphinx `ws_re.sub(repl, s)` (`util/__init__.py`, `ws_re = re.compile(r'\s+')`).
fn ws_collapse(s: &str, repl: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_ws {
                out.push_str(repl);
                in_ws = true;
            }
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// `ObjectDescription.get_signatures` (`directives/__init__.py:88-98`) with
/// `strip_signature_backslash` at its default False: backslash-newline pairs
/// vanish (`nl_escape_re`), then one stripped signature per line.
fn object_signatures(argument: &str) -> Vec<String> {
    argument
        .replace("\\\n", "")
        .split('\n')
        .map(|line| line.trim().to_string())
        .collect()
}

/// `option_desc_re = r'((?:/|--|-|\+)?[^\s=]+)(=?\s*.*)'` matched with
/// `re.match` (anchored at the start only). The optional prefix backtracks:
/// `--` is tried before `-`, and both before the empty alternative, so a
/// bare `--` matches as prefix `-` + name `-`.
fn option_desc_match(s: &str) -> Option<(String, String)> {
    let mut prefixes: Vec<usize> = Vec::new();
    if s.starts_with('/') {
        prefixes.push(1);
    }
    if s.starts_with("--") {
        prefixes.push(2);
    }
    if s.starts_with('-') {
        prefixes.push(1);
    }
    if s.starts_with('+') {
        prefixes.push(1);
    }
    prefixes.push(0);
    for prefix in prefixes {
        // `[^\s=]+`, greedy and at least one character long.
        let taken: usize = s[prefix..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '=')
            .map(char::len_utf8)
            .sum();
        if taken > 0 {
            return Some((
                s[..prefix + taken].to_string(),
                s[prefix + taken..].to_string(),
            ));
        }
    }
    None
}

/// `addnodes.desc_name` — `_DescClassesInjector` stamps the two classes and
/// `FixedTextElement` the `xml:space` (`sphinx/addnodes.py`).
fn desc_name_node(text: &str, span: Span) -> Node {
    sig_text_node("desc_name", ["sig-name", "descname"], text, span)
}

/// `addnodes.desc_addname`.
fn desc_addname_node(text: &str, span: Span) -> Node {
    sig_text_node("desc_addname", ["sig-prename", "descclassname"], text, span)
}

fn sig_text_node(kind: &'static str, classes: [&str; 2], text: &str, span: Span) -> Node {
    let mut node = Node::elem(kind, span);
    node.attrs
        .classes
        .extend(classes.iter().map(|c| c.to_string()));
    node.set("xml:space", AttrValue::Str("preserve".to_string()));
    // `TextElement(rawsource, text)` adds no child for an empty text — the
    // `desc_addname` of an argument-less option is an empty element.
    if !text.is_empty() {
        node.children.push(Node::text_node(text, span));
    }
    node
}

/// `[node_id for el in node.findall(nodes.Element) for node_id in el['ids']]`
/// — `findall` yields the node itself first, then its descendants in
/// document order, and skips Text nodes (they are not Elements).
fn collect_element_ids(node: &Node, out: &mut Vec<String>) {
    if node.kind == kinds::TEXT {
        return;
    }
    out.extend(node.attrs.ids.iter().cloned());
    for child in &node.children {
        collect_element_ids(child, out);
    }
}

/// `ObjectDescription.option_spec` (`directives/__init__.py:55-63`).
const OBJECT_DESCRIPTION_OPTS: &[(&str, Conv)] = &[
    ("no-index", Conv::Flag),
    ("no-index-entry", Conv::Flag),
    ("no-contents-entry", Conv::Flag),
    ("no-typesetting", Conv::Flag),
    ("noindex", Conv::Flag),
    ("noindexentry", Conv::Flag),
    ("nocontentsentry", Conv::Flag),
];

/// `ConfigurationValue.option_spec` (`domains/std/__init__.py:117-124`).
const CONFVAL_OPTS: &[(&str, Conv)] = &[
    ("no-index", Conv::Flag),
    ("no-index-entry", Conv::Flag),
    ("no-contents-entry", Conv::Flag),
    ("no-typesetting", Conv::Flag),
    ("type", Conv::UnchangedRequired),
    ("default", Conv::UnchangedRequired),
];

/// One serialized 5-tuple for the index `entries` attr: docutils pformat
/// renders list items via serial_escape (spaces backslash-escaped inside
/// each item, items space-joined).
pub(crate) fn index_entry_tuple(
    entrytype: &str,
    value: &str,
    target_id: &str,
    main: &str,
    key: Option<&str>,
) -> String {
    let key_repr = match key {
        Some(k) => py_repr(Some(k)),
        None => "None".to_string(),
    };
    let tuple = format!(
        "({}, {}, {}, {}, {})",
        py_repr(Some(entrytype)),
        py_repr(Some(value)),
        py_repr(Some(target_id)),
        py_repr(Some(main)),
        key_repr
    );
    tuple.replace(' ', "\\ ")
}

/// process_index_entry (sphinx/util/nodes.py:431-482): returns serialized
/// 5-tuples. Legacy types raise in sphinx; here they fall through to the
/// single form (hardening note — the oracle corpus avoids them).
fn process_index_entry(entry: &str, target_id: &str) -> Vec<String> {
    const TYPES: &[&str] = &["single", "pair", "double", "triple", "see", "seealso"];
    let (main, entry) = match entry.strip_prefix('!') {
        Some(rest) => ("main", rest),
        None => ("", entry),
    };
    for t in TYPES {
        if let Some(value) = entry.strip_prefix(&format!("{t}:")) {
            let value = value.trim();
            let ty = if *t == "double" { "pair" } else { t };
            return vec![index_entry_tuple(ty, value, target_id, main, None)];
        }
    }
    // Comma shorthand with per-item '!'.
    if entry.contains(',') {
        return entry
            .split(',')
            .map(|part| {
                let part = part.trim();
                let (m, p) = match part.strip_prefix('!') {
                    Some(rest) => ("main", rest),
                    None => (main, part),
                };
                index_entry_tuple("single", p, target_id, m, None)
            })
            .collect();
    }
    vec![index_entry_tuple("single", entry, target_id, main, None)]
}

const SPHINX_MATH_OPTS: &[(&str, Conv)] = &[
    ("label", Conv::Unchanged),
    ("name", Conv::Unchanged),
    ("class", Conv::ClassOption),
    ("no-wrap", Conv::Flag),
    ("nowrap", Conv::Flag),
];

const HLIST_OPTS: &[(&str, Conv)] = &[("columns", Conv::PyIntAny)];

const GLOSSARY_OPTS: &[(&str, Conv)] = &[("sorted", Conv::Flag)];

const UNICODE_OPTS: &[(&str, Conv)] = &[
    ("trim", Conv::Flag),
    ("ltrim", Conv::Flag),
    ("rtrim", Conv::Flag),
];

/// `( |\n|^)\.\. ` comment split for the unicode directive (misc.py:399):
/// returns the byte index where the argument text is cut.
fn unicode_comment_cut(text: &str) -> usize {
    if text.starts_with(".. ") {
        return 0;
    }
    let bytes = text.as_bytes();
    for i in 0..text.len() {
        if (bytes[i] == b' ' || bytes[i] == b'\n') && text[i + 1..].starts_with(".. ") {
            return i;
        }
    }
    text.len()
}

/// Minimal strftime over the current LOCAL-approximated (UTC) time:
/// %Y/%m/%d/%H/%M/%S/%% expand, other bytes pass through.
fn strftime_now(format: &str) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let (h, mi, s) = ((secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&year.to_string()),
            Some('m') => out.push_str(&format!("{m:02}")),
            Some('d') => out.push_str(&format!("{d:02}")),
            Some('H') => out.push_str(&format!("{h:02}")),
            Some('M') => out.push_str(&format!("{mi:02}")),
            Some('S') => out.push_str(&format!("{s:02}")),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

const TABLE_OPTS: &[(&str, Conv)] = &[
    ("align", Conv::Choice(H_ALIGN_VALUES)),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
    ("width", Conv::LengthOrPercentageOrUnitless("")),
    ("widths", Conv::WidthsAutoGrid),
];

const CSV_TABLE_OPTS: &[(&str, Conv)] = &[
    ("header-rows", Conv::NonnegativeInt),
    ("stub-columns", Conv::NonnegativeInt),
    ("header", Conv::Unchanged),
    ("width", Conv::LengthOrPercentageOrUnitless("")),
    ("widths", Conv::WidthsAuto),
    ("file", Conv::Path),
    ("url", Conv::Uri),
    ("encoding", Conv::Encoding),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
    ("align", Conv::Choice(H_ALIGN_VALUES)),
    ("delim", Conv::SingleCharOrWhitespaceOrUnicode),
    ("keepspace", Conv::Flag),
    ("quote", Conv::SingleCharOrUnicode),
    ("escape", Conv::SingleCharOrUnicode),
];

const LIST_TABLE_OPTS: &[(&str, Conv)] = &[
    ("header-rows", Conv::NonnegativeInt),
    ("stub-columns", Conv::NonnegativeInt),
    ("width", Conv::LengthOrPercentageOrUnitless("")),
    ("widths", Conv::WidthsAuto),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
    ("align", Conv::Choice(H_ALIGN_VALUES)),
];

/// Python csv.reader over the option-configured dialect
/// (tables.py DocutilsDialect): doublequote unless an escapechar is set,
/// skipinitialspace unless :keepspace:, quoted cells may span lines.
fn parse_csv_text(
    text: &str,
    delim: char,
    quote: char,
    escape: Option<char>,
    doublequote: bool,
    skipinitialspace: bool,
) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_quotes = false;
    let mut cell_started = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if Some(c) == escape {
                if let Some(n) = chars.next() {
                    cell.push(n);
                }
            } else if c == quote {
                if doublequote && chars.peek() == Some(&quote) {
                    chars.next();
                    cell.push(quote);
                } else {
                    in_quotes = false;
                }
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            c if c == quote && !cell_started => {
                in_quotes = true;
                cell_started = true;
            }
            c if c == delim => {
                row.push(std::mem::take(&mut cell));
                cell_started = false;
                if skipinitialspace {
                    while chars.peek() == Some(&' ') {
                        chars.next();
                    }
                }
            }
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                cell_started = false;
            }
            c => {
                cell.push(c);
                cell_started = true;
            }
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// The docutils Directive class contract (rst/__init__.py:305-318).
#[derive(Clone, Copy)]
struct DirectiveSpec {
    required_arguments: usize,
    optional_arguments: usize,
    final_argument_whitespace: bool,
    has_content: bool,
    option_spec: &'static [(&'static str, Conv)],
    kind: DirectiveKind,
}

/// Option converters (directives/__init__.py:156-481). Each mirrors one
/// docutils conversion function, including its exact error text.
#[derive(Clone, Copy)]
enum Conv {
    Flag,
    Unchanged,
    UnchangedRequired,
    NonnegativeInt,
    Percentage,
    LengthOrUnitless,
    /// The &str is the docutils `default` unit suffix appended to unitless
    /// values ("" for image width, "px" for figwidth).
    LengthOrPercentageOrUnitless(&'static str),
    ClassOption,
    Choice(&'static [&'static str]),
    Path,
    Uri,
    /// codecs.lookup validation is approximated as accept-any (hardening
    /// note: exotic names docutils rejects are accepted here).
    Encoding,
    /// figure :figwidth:: the literal 'image' keyword or a length.
    Figwidth,
    /// Plain Python int() — negatives allowed (sphinx maxdepth).
    PyIntAny,
    SingleCharOrUnicode,
    SingleCharOrWhitespaceOrUnicode,
    /// value_or(('auto', 'grid'), positive_int_list) — the table :widths:.
    WidthsAutoGrid,
    /// value_or(('auto',), positive_int_list) — csv/list-table :widths:.
    WidthsAuto,
    /// positive_int as used by the widths list elements.
    PositiveIntForList,
}

/// Converted option values (Python-typed in docutils: None/str/int/list).
#[derive(Clone, Debug, PartialEq)]
enum OptVal {
    /// flag options convert to Python None.
    Null,
    Str(String),
    Int(i64),
    IntList(Vec<i64>),
    StrList(Vec<String>),
}

/// The arguments/options/content/etc. handed to a directive's run().
struct DirectiveInput<'a, 'r> {
    /// The directive name AS WRITTEN (docutils self.name; error messages
    /// reproduce the original case).
    name: &'r str,
    arguments: Vec<String>,
    options: Vec<(String, OptVal)>,
    content: Vec<LineRef<'a>>,
    span: Span,
    lineno: u32,
    rawsource: &'r str,
}

fn opt_get<'o>(options: &'o [(String, OptVal)], name: &str) -> Option<&'o OptVal> {
    options.iter().find(|(n, _)| n == name).map(|(_, v)| v)
}

const ADMONITION_OPTS: &[(&str, Conv)] = &[("class", Conv::ClassOption), ("name", Conv::Unchanged)];

const SIDEBAR_OPTS: &[(&str, Conv)] = &[
    ("subtitle", Conv::UnchangedRequired),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
];

const NAME_ONLY_OPTS: &[(&str, Conv)] = &[("name", Conv::Unchanged)];

const IMAGE_ALIGN_VALUES: &[&str] = &["top", "middle", "bottom", "left", "center", "right"];
const IMAGE_LOADING_VALUES: &[&str] = &["embed", "link", "lazy"];
const IMAGE_OPTS: &[(&str, Conv)] = &[
    ("alt", Conv::Unchanged),
    ("height", Conv::LengthOrUnitless),
    ("width", Conv::LengthOrPercentageOrUnitless("")),
    ("scale", Conv::Percentage),
    ("align", Conv::Choice(IMAGE_ALIGN_VALUES)),
    ("target", Conv::UnchangedRequired),
    ("loading", Conv::Choice(IMAGE_LOADING_VALUES)),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
];

const H_ALIGN_VALUES: &[&str] = &["left", "center", "right"];
const FIGURE_OPTS: &[(&str, Conv)] = &[
    ("alt", Conv::Unchanged),
    ("height", Conv::LengthOrUnitless),
    ("width", Conv::LengthOrPercentageOrUnitless("")),
    ("scale", Conv::Percentage),
    ("align", Conv::Choice(H_ALIGN_VALUES)),
    ("target", Conv::UnchangedRequired),
    ("loading", Conv::Choice(IMAGE_LOADING_VALUES)),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
    ("figwidth", Conv::Figwidth),
    ("figclass", Conv::ClassOption),
];

const CODE_OPTS: &[(&str, Conv)] = &[
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
    ("number-lines", Conv::Unchanged),
];

const RAW_OPTS: &[(&str, Conv)] = &[
    ("file", Conv::Path),
    ("url", Conv::Uri),
    ("encoding", Conv::Encoding),
    ("class", Conv::ClassOption),
];

fn directive_spec(lower: &str) -> Option<DirectiveSpec> {
    let adm = |k: &'static str| {
        Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::Admonition(k),
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
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::GenericAdmonition,
        }),
        "image" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: false,
            option_spec: IMAGE_OPTS,
            kind: DirectiveKind::Image,
        }),
        "topic" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::PseudoSection("topic"),
        }),
        "sidebar" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 1,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: SIDEBAR_OPTS,
            kind: DirectiveKind::PseudoSection("sidebar"),
        }),
        "rubric" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: false,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::Rubric,
        }),
        "epigraph" => Some(quote_class_spec("epigraph")),
        "highlights" => Some(quote_class_spec("highlights")),
        "pull-quote" => Some(quote_class_spec("pull-quote")),
        "compound" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::Compound,
        }),
        "container" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 1,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: NAME_ONLY_OPTS,
            kind: DirectiveKind::Container,
        }),
        "parsed-literal" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::ParsedLiteral,
        }),
        "figure" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: FIGURE_OPTS,
            kind: DirectiveKind::Figure,
        }),
        "code" | "code-block" | "sourcecode" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 1,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: CODE_OPTS,
            kind: DirectiveKind::Code,
        }),
        "math" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::MathBlock,
        }),
        "raw" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: RAW_OPTS,
            kind: DirectiveKind::Raw,
        }),
        "line-block" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: ADMONITION_OPTS,
            kind: DirectiveKind::LineBlockDir,
        }),
        // en-alias table entries whose canonical directive is implemented
        // (languages/en.py: code-block/sourcecode -> code, rst-class ->
        // class, section-numbering -> sectnum [unimplemented]).
        "class" | "rst-class" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: &[],
            kind: DirectiveKind::ClassDir,
        }),
        "table" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 1,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: TABLE_OPTS,
            kind: DirectiveKind::RstTable,
        }),
        "csv-table" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 1,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: CSV_TABLE_OPTS,
            kind: DirectiveKind::CsvTable,
        }),
        "list-table" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 1,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: LIST_TABLE_OPTS,
            kind: DirectiveKind::ListTable,
        }),
        "replace" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: &[],
            kind: DirectiveKind::Replace,
        }),
        "unicode" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: false,
            option_spec: UNICODE_OPTS,
            kind: DirectiveKind::UnicodeDir,
        }),
        "date" => Some(DirectiveSpec {
            required_arguments: 0,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: &[],
            kind: DirectiveKind::DateDir,
        }),
        _ => None,
    }
}

/// epigraph/highlights/pull-quote: content-only, NO options at all
/// (body.py:257-283 — option_spec is not declared).
fn quote_class_spec(class: &'static str) -> DirectiveSpec {
    DirectiveSpec {
        required_arguments: 0,
        optional_arguments: 0,
        final_argument_whitespace: false,
        has_content: true,
        option_spec: &[],
        kind: DirectiveKind::QuoteClass(class),
    }
}

/// parse_directive_arguments (states.py:2365-2380).
fn parse_directive_arguments(arg_text: &str, spec: &DirectiveSpec) -> Result<Vec<String>, String> {
    let required = spec.required_arguments;
    let optional = spec.optional_arguments;
    let words: Vec<&str> = arg_text.split_whitespace().collect();
    if words.len() < required {
        return Err(format!(
            "{} argument(s) required, {} supplied",
            required,
            words.len()
        ));
    }
    if words.len() > required + optional {
        if spec.final_argument_whitespace {
            return Ok(py_split_max(arg_text, required + optional - 1));
        }
        return Err(format!(
            "maximum {} argument(s) allowed, {} supplied",
            required + optional,
            words.len()
        ));
    }
    Ok(words.iter().map(|w| w.to_string()).collect())
}

/// Python `str.split(None, maxsplit)`: whitespace runs separate the first
/// `maxsplit` tokens; the remainder keeps internal whitespace verbatim.
fn py_split_max(text: &str, maxsplit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    for _ in 0..maxsplit {
        if rest.is_empty() {
            return out;
        }
        match rest.find(char::is_whitespace) {
            Some(i) => {
                out.push(rest[..i].to_string());
                rest = rest[i..].trim_start();
            }
            None => {
                out.push(rest.to_string());
                return out;
            }
        }
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

/// parse_extension_options + extract_options + assemble_option_dict
/// (states.py:2382-2413, utils.py:274-369). Errors return the MarkupError
/// detail string; the caller adds the 'Error in "X" directive:' wrapper.
fn parse_extension_options(
    opt_block: &[LineRef<'_>],
    option_spec: &'static [(&'static str, Conv)],
) -> Result<Vec<(String, OptVal)>, String> {
    // Pass 1 (extract_options): collect (lowercased name, body) fields.
    // A multi-word field name errors during this pass, in field order.
    let mut fields: Vec<(String, Option<String>)> = Vec::new();
    let mut i = 0usize;
    while i < opt_block.len() {
        let l = opt_block[i];
        let marker = if l.indent() == 0 {
            field_marker(l.text)
        } else {
            None
        };
        let Some((raw_name, body_start)) = marker else {
            return Err("invalid option block".to_string());
        };
        let mut body_lines: Vec<&str> = Vec::new();
        let first = l.text[body_start..].trim_start_matches(' ');
        if !first.is_empty() {
            body_lines.push(first);
        }
        // Continuation lines (any deeper indent) join the field body,
        // dedented by their common indent, '\n'-separated.
        let mut j = i + 1;
        while j < opt_block.len() && opt_block[j].indent() > 0 {
            j += 1;
        }
        let conts = &opt_block[i + 1..j];
        let min_indent = conts.iter().map(|c| c.indent()).min().unwrap_or(0);
        for c in conts {
            body_lines.push(&c.text[min_indent.min(c.indent())..]);
        }
        if raw_name.split_whitespace().count() != 1 {
            return Err(
                "invalid option data: extension option field name may not contain multiple words"
                    .to_string(),
            );
        }
        let body = if body_lines.is_empty() {
            None
        } else {
            Some(body_lines.join("\n"))
        };
        fields.push((raw_name.to_lowercase(), body));
        i = j;
    }
    // Pass 2 (assemble_option_dict): unknown, then duplicate, then convert.
    let mut out: Vec<(String, OptVal)> = Vec::new();
    for (name, body) in &fields {
        let Some((_, conv)) = option_spec.iter().find(|(n, _)| n == name) else {
            return Err(format!("unknown option: \"{name}\""));
        };
        if out.iter().any(|(n, _)| n == name) {
            return Err(format!("invalid option data: duplicate option \"{name}\""));
        }
        match convert_option(*conv, body.as_deref()) {
            Ok(v) => out.push((name.clone(), v)),
            Err(detail) => {
                return Err(format!(
                    "invalid option value: (option: \"{}\"; value: {})\n{}",
                    name,
                    py_repr(body.as_deref()),
                    detail
                ));
            }
        }
    }
    Ok(out)
}

fn convert_option(conv: Conv, value: Option<&str>) -> Result<OptVal, String> {
    match conv {
        Conv::Flag => match value {
            Some(v) if !v.trim().is_empty() => {
                Err(format!("no argument is allowed; \"{v}\" supplied"))
            }
            _ => Ok(OptVal::Null),
        },
        Conv::PyIntAny => {
            let Some(v) = value else {
                return Err(
                    "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'"
                        .to_string(),
                );
            };
            match py_int_canonical(v) {
                Some((neg, digits)) => Ok(int_optval(neg, &digits)),
                None => Err(format!(
                    "invalid literal for int() with base 10: {}",
                    py_repr(Some(v))
                )),
            }
        }
        Conv::NonnegativeInt => {
            let Some(v) = value else {
                return Err(
                    "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'"
                        .to_string(),
                );
            };
            nonnegative_int(v)
        }
        Conv::SingleCharOrUnicode | Conv::SingleCharOrWhitespaceOrUnicode => {
            let Some(v) = value else {
                return Err("argument required but none supplied".to_string());
            };
            if matches!(conv, Conv::SingleCharOrWhitespaceOrUnicode) {
                if v == "tab" {
                    return Ok(OptVal::Str("\t".to_string()));
                }
                if v == "space" {
                    return Ok(OptVal::Str(" ".to_string()));
                }
            }
            let decoded = unicode_code(v)?;
            if decoded.chars().count() != 1 {
                return Err(format!(
                    "{} invalid; must be a single character or a Unicode code",
                    py_repr(Some(&decoded))
                ));
            }
            Ok(OptVal::Str(decoded))
        }
        Conv::WidthsAutoGrid | Conv::WidthsAuto => {
            let Some(v) = value else {
                return Err("argument required but none supplied".to_string());
            };
            let keywords: &[&str] = if matches!(conv, Conv::WidthsAutoGrid) {
                &["auto", "grid"]
            } else {
                &["auto"]
            };
            if keywords.contains(&v) {
                return Ok(OptVal::Str(v.to_string()));
            }
            let parts: Vec<&str> = if v.contains(',') {
                v.split(',').collect()
            } else {
                v.split_whitespace().collect()
            };
            let mut list = Vec::new();
            for p in parts {
                match convert_option(Conv::PositiveIntForList, Some(p.trim()))? {
                    OptVal::Int(n) => list.push(n),
                    _ => unreachable!(),
                }
            }
            Ok(OptVal::IntList(list))
        }
        Conv::PositiveIntForList => {
            let Some(v) = value else {
                return Err("argument required but none supplied".to_string());
            };
            match py_int(v) {
                Some(n) if n >= 1 => Ok(OptVal::Int(n)),
                Some(_) => Err("negative or zero value; must be positive".to_string()),
                None => Err(format!(
                    "invalid literal for int() with base 10: {}",
                    py_repr(Some(v))
                )),
            }
        }
        Conv::Unchanged => Ok(OptVal::Str(value.unwrap_or("").to_string())),
        Conv::UnchangedRequired => match value {
            None => Err("argument required but none supplied".to_string()),
            Some(v) => Ok(OptVal::Str(v.to_string())),
        },
        Conv::Percentage => {
            // percentage(): rstrip(' %'), then nonnegative_int; None slips
            // through to int(None)'s TypeError (directives/__init__.py:235).
            let Some(v) = value else {
                return Err(
                    "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'"
                        .to_string(),
                );
            };
            nonnegative_int(v.trim_end_matches([' ', '%']))
        }
        Conv::LengthOrUnitless => {
            let Some(v) = value else {
                return Err("expected string or bytes-like object, got 'NoneType'".to_string());
            };
            let mut units: Vec<&str> = CSS3_LENGTH_UNITS.to_vec();
            units.push("");
            get_measure(v, &units).map(OptVal::Str)
        }
        Conv::LengthOrPercentageOrUnitless(default) => {
            let Some(v) = value else {
                return Err("expected string or bytes-like object, got 'NoneType'".to_string());
            };
            let mut units: Vec<&str> = CSS3_LENGTH_UNITS.to_vec();
            units.push("%");
            match get_measure(v, &units) {
                Ok(m) => Ok(OptVal::Str(m)),
                Err(first_error) => match get_measure(v, &[""]) {
                    Ok(m) => Ok(OptVal::Str(format!("{m}{default}"))),
                    Err(_) => Err(first_error),
                },
            }
        }
        Conv::Path => {
            let Some(v) = value else {
                return Err("argument required but none supplied".to_string());
            };
            Ok(OptVal::Str(
                v.lines().map(str::trim).collect::<Vec<_>>().join(""),
            ))
        }
        Conv::Uri => {
            let Some(v) = value else {
                return Err("argument required but none supplied".to_string());
            };
            Ok(OptVal::Str(uri_from_argument(v)))
        }
        Conv::Encoding => {
            let Some(v) = value else {
                return Err("argument required but none supplied".to_string());
            };
            Ok(OptVal::Str(v.to_string()))
        }
        Conv::Figwidth => {
            let Some(v) = value else {
                return Err("expected string or bytes-like object, got 'NoneType'".to_string());
            };
            if v.eq_ignore_ascii_case("image") {
                return Ok(OptVal::Str("image".to_string()));
            }
            convert_option(Conv::LengthOrPercentageOrUnitless("px"), Some(v))
        }
        Conv::ClassOption => {
            let Some(v) = value else {
                return Err("argument required but none supplied".to_string());
            };
            let mut names = Vec::new();
            for word in v.split_whitespace() {
                let id = ids::make_id(word);
                if id.is_empty() {
                    return Err(format!("cannot make \"{word}\" into a class name"));
                }
                names.push(id);
            }
            Ok(OptVal::StrList(names))
        }
        Conv::Choice(values) => {
            let Some(v) = value else {
                return Err(format!(
                    "must supply an argument; choose from {}",
                    format_choice_values(values)
                ));
            };
            let lowered = v.trim().to_lowercase();
            if values.contains(&lowered.as_str()) {
                Ok(OptVal::Str(lowered))
            } else {
                Err(format!(
                    "\"{v}\" unknown; choose from {}",
                    format_choice_values(values)
                ))
            }
        }
    }
}

/// format_values (directives/__init__.py:448-450).
fn format_choice_values(values: &[&str]) -> String {
    let init = values[..values.len() - 1]
        .iter()
        .map(|v| format!("\"{v}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}, or \"{}\"", init, values[values.len() - 1])
}

/// unicode_code (directives/__init__.py:330-352): decimal, hex forms
/// (0x/x/\x/U+/\u/&#x...;), or the text itself when neither matches.
fn unicode_code(code: &str) -> Result<String, String> {
    // Python gates on str.isdigit() (Nd digits AND digit-typed No chars
    // like '²'), then int() — which only accepts the Nd ones.
    if !code.is_empty() && code.chars().all(super::digits::is_python_digit) {
        let Some((false, digits)) = py_int_canonical(code) else {
            return Err(format!(
                "invalid literal for int() with base 10: {}",
                py_repr(Some(code))
            ));
        };
        let n: u32 = digits
            .parse()
            .map_err(|_| format!("code too large ({code})"))?;
        return char::from_u32(n)
            .map(|c| c.to_string())
            .ok_or_else(|| "chr() arg not in range(0x110000)".to_string());
    }
    let lower = code.to_lowercase();
    let hex = ["0x", "x", "\\x", "u+", "u", "\\u"]
        .iter()
        .find_map(|p| lower.strip_prefix(p))
        .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|h| h.to_string())
        .or_else(|| {
            lower
                .strip_prefix("&#x")
                .and_then(|h| h.strip_suffix(';'))
                .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(|h| h.to_string())
        });
    match hex {
        Some(h) => {
            let n = u32::from_str_radix(&h, 16).map_err(|_| format!("code too large ({h})"))?;
            char::from_u32(n)
                .map(|c| c.to_string())
                .ok_or_else(|| "chr() arg not in range(0x110000)".to_string())
        }
        None => Ok(code.to_string()),
    }
}

/// The converted int as an OptVal: i64 when it fits, else the canonical
/// decimal string (Python ints are arbitrary precision; pformat renders
/// both identically).
fn int_optval(neg: bool, digits: &str) -> OptVal {
    let display = py_int_display(neg, digits);
    match display.parse::<i64>() {
        Ok(n) => OptVal::Int(n),
        Err(_) => OptVal::Str(display),
    }
}

/// nonnegative_int (directives/__init__.py:224-231), with Python's own
/// int() error text for bad literals.
fn nonnegative_int(s: &str) -> Result<OptVal, String> {
    match py_int_canonical(s) {
        Some((true, _)) => Err("negative value; must be positive or zero".to_string()),
        Some((false, digits)) => Ok(int_optval(false, &digits)),
        None => Err(format!(
            "invalid literal for int() with base 10: {}",
            py_repr(Some(s))
        )),
    }
}

/// Python int(str), canonicalized: arbitrary precision (the value is the
/// canonical ASCII decimal string), Unicode Nd digits accepted with their
/// decimal values, single underscores allowed BETWEEN digits, surrounding
/// whitespace ignored. Returns (negative, digits-without-sign, canonical
/// leading-zero-stripped ASCII string WITH sign).
fn py_int_canonical(s: &str) -> Option<(bool, String)> {
    let t = s.trim();
    let (neg, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if body.is_empty() {
        return None;
    }
    let mut digits = String::with_capacity(body.len());
    let mut prev_underscore = true; // leading underscore rejected
    for c in body.chars() {
        if c == '_' {
            if prev_underscore {
                return None;
            }
            prev_underscore = true;
            continue;
        }
        let d = super::digits::decimal_digit_value(c)?;
        digits.push(char::from(b'0' + d as u8));
        prev_underscore = false;
    }
    if prev_underscore {
        // trailing underscore (or all-underscores)
        return None;
    }
    let stripped = digits.trim_start_matches('0');
    let canonical = if stripped.is_empty() { "0" } else { stripped };
    Some((neg && canonical != "0", canonical.to_string()))
}

fn py_int_display(neg: bool, digits: &str) -> String {
    if neg {
        format!("-{digits}")
    } else {
        digits.to_string()
    }
}

/// Python int(str) for numeric consumers; None when invalid OR outside
/// i64 (attr-facing paths must use [`py_int_canonical`] to keep exact
/// digits for values Python would carry at arbitrary precision).
fn py_int(s: &str) -> Option<i64> {
    let (neg, digits) = py_int_canonical(s)?;
    py_int_display(neg, &digits).parse::<i64>().ok()
}

/// Python repr() for option-value error messages (strings and None).
fn py_repr(value: Option<&str>) -> String {
    match value {
        None => "None".to_string(),
        Some(s) => {
            let quote = if s.contains('\'') && !s.contains('"') {
                '"'
            } else {
                '\''
            };
            let mut out = String::new();
            out.push(quote);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c);
                    }
                    c => out.push(c),
                }
            }
            out.push(quote);
            out
        }
    }
}

/// CSS3_LENGTH_UNITS (directives/__init__.py:247-248).
const CSS3_LENGTH_UNITS: &[&str] = &[
    "em", "ex", "ch", "rem", "vw", "vh", "vmin", "vmax", "cm", "mm", "Q", "in", "pt", "pc", "px",
];

/// get_measure (directives/__init__.py:260-274) over nodes.parse_measure
/// (nodes.py:3084-3107). Returns the normalized `{value}{unit}` string.
fn get_measure(argument: &str, units: &[&str]) -> Result<String, String> {
    let no_valid = || format!("\"{argument}\" is no valid measure.");
    // fullmatch: (-?[0-9.]+) *([a-zA-Zµ]*|%?)
    let s = argument;
    let digits_start = if s.starts_with('-') { 1 } else { 0 };
    let mut j = digits_start;
    let bytes = s.as_bytes();
    while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
        j += 1;
    }
    if j == digits_start {
        return Err(no_valid());
    }
    let number = &s[..j];
    let mut k = j;
    while k < bytes.len() && bytes[k] == b' ' {
        k += 1;
    }
    let unit = &s[k..];
    let unit_ok = unit == "%" || unit.chars().all(|c| c.is_ascii_alphabetic() || c == 'µ');
    if !unit_ok {
        return Err(no_valid());
    }
    // Python: int() first (arbitrary precision — exact digits preserved),
    // float() second; negative or unlisted unit is the units-list error.
    let (negative, norm) = if let Some((neg, digits)) = py_int_canonical(number) {
        (neg, py_int_display(neg, &digits))
    } else if let Ok(f) = number.parse::<f64>() {
        (f < 0.0, py_float_str(f))
    } else {
        return Err(no_valid());
    };
    if negative || !units.contains(&unit) {
        return Err(format!(
            "not a positive number or measure of one of the following units:\n{}",
            units
                .iter()
                .filter(|u| !u.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(format!("{norm}{unit}"))
}

/// Python float repr for simple decimals (1.0 -> "1.0", 1.5 -> "1.5").
fn py_float_str(f: f64) -> String {
    if f == f.trunc() && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

/// directives.uri (directives/__init__.py:209-221): unescaped whitespace is
/// removed; backslash-escaped whitespace separates space-joined parts.
fn uri_from_argument(argument: &str) -> String {
    let escaped = super::inline::escape2null(argument);
    let mut parts: Vec<&str> = Vec::new();
    for chunk in escaped.split("\x00 ") {
        parts.extend(chunk.split("\x00\n"));
    }
    parts
        .iter()
        .map(|p| {
            super::inline::unescape(p, false)
                .split_whitespace()
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// states.py parse_target (2095-2113) for the image :target: option: a
/// block whose last line ends in `_` may be an indirect reference;
/// otherwise it is a refuri with all whitespace removed.
enum ImageTarget {
    Refname { name: String, refname: String },
    Refuri(String),
}

fn parse_image_target(target: &str) -> ImageTarget {
    let lines: Vec<&str> = target.lines().collect();
    let ends_underscore = lines
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().ends_with('_'))
        .unwrap_or(false);
    if ends_underscore {
        let joined = lines.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
        if let Some(data) = reference_data_from_link(&joined) {
            return ImageTarget::Refname {
                name: ids::whitespace_normalize_name(&data),
                refname: ids::fully_normalize_name(&data),
            };
        }
    }
    ImageTarget::Refuri(target.split_whitespace().collect::<String>())
}

/// `|name|` marker in a (possibly line-joined) substitution-def head:
/// `\|(?![ ])(?P<name>.+?)(?<![\s\x00])\|([ ]+|$)` (states.py:1992-2001).
struct SubstMarker {
    name: String,
    /// Byte index where the remainder after the marker + separator spaces
    /// begins (== input length when the marker ends the line).
    remainder_start: usize,
}

fn match_substitution_marker(acc: &str) -> Option<SubstMarker> {
    let cs: Vec<(usize, char)> = acc.char_indices().collect();
    if cs.len() < 3 || cs[0].1 != '|' || cs[1].1 == ' ' {
        return None;
    }
    for k in 2..cs.len() {
        if cs[k].1 != '|' || cs[k - 1].1.is_whitespace() {
            continue;
        }
        let name = acc[cs[1].0..cs[k].0].to_string();
        let after = &acc[cs[k].0 + 1..];
        if after.is_empty() {
            return Some(SubstMarker {
                name,
                remainder_start: acc.len(),
            });
        }
        if after.starts_with(' ') {
            let spaces = after.len() - after.trim_start_matches(' ').len();
            return Some(SubstMarker {
                name,
                remainder_start: cs[k].0 + 1 + spaces,
            });
        }
        // Closing pipe not followed by space/EOL: the non-greedy regex
        // tries a later close.
    }
    None
}

/// SubstitutionDef embedded-directive marker: `(simplename)::( +|$)` —
/// unlike the body-level form, NO space is allowed before `::`.
fn match_embedded_directive(text: &str) -> Option<(String, &str)> {
    let chars: Vec<char> = text.chars().collect();
    let name_len = match_simplename_chars(&chars, 0)?;
    if chars.get(name_len) != Some(&':') || chars.get(name_len + 1) != Some(&':') {
        return None;
    }
    let after = name_len + 2;
    match chars.get(after) {
        None => {}
        Some(' ') => {}
        _ => return None,
    }
    let name: String = chars[..name_len].iter().collect();
    let byte_after = text
        .char_indices()
        .nth(after + 1)
        .map(|(b, _)| b)
        .unwrap_or(text.len());
    Some((name, &text[byte_after..]))
}

fn dedent_by_min<'a>(block: &[LineRef<'a>]) -> Vec<LineRef<'a>> {
    let min = block
        .iter()
        .filter(|l| !l.is_blank())
        .map(|l| l.indent())
        .min()
        .unwrap_or(0);
    block.iter().map(|l| l.dedented(min)).collect()
}

/// docutils nodes.Inline membership for the kinds this parser emits
/// (image/target/raw are genuinely Inline in docutils' class hierarchy).
fn is_inline_kind(kind: &str) -> bool {
    matches!(
        kind,
        "emphasis"
            | "strong"
            | "literal"
            | "reference"
            | "title_reference"
            | "abbreviation"
            | "acronym"
            | "subscript"
            | "superscript"
            | "math"
            | "image"
            | "problematic"
            | "inline"
            | "substitution_reference"
            | "footnote_reference"
            | "citation_reference"
            | "target"
            | "raw"
    )
}

fn tree_any(node: &Node, pred: &dyn Fn(&Node) -> bool) -> bool {
    node.children.iter().any(|c| pred(c) || tree_any(c, pred))
}

fn has_extra_attr(node: &Node, key: &str) -> bool {
    node.attrs.extra.iter().any(|(k, _)| *k == key)
}

/// disallowed_inside_substitution_definitions (states.py:2219-2227),
/// first hit in document order wins.
fn find_disallowed_in_substitution(node: &Node) -> Option<&'static str> {
    for c in &node.children {
        let hit = if c.kind == kinds::REFERENCE && has_extra_attr(c, "anonymous") {
            Some("Anonymous references")
        } else if c.kind == kinds::FOOTNOTE_REFERENCE && has_extra_attr(c, "auto") {
            Some("References to auto-numbered and auto-symbol footnotes")
        } else if !c.attrs.names.is_empty() || !c.attrs.ids.is_empty() {
            Some("Targets (names and identifiers)")
        } else {
            None
        };
        if hit.is_some() {
            return hit;
        }
        if let Some(h) = find_disallowed_in_substitution(c) {
            return Some(h);
        }
    }
    None
}

fn count_subst_defs(node: &Node, name: &str) -> usize {
    let mut c = usize::from(
        node.kind == "substitution_definition" && node.attrs.names.iter().any(|n| n == name),
    );
    for ch in &node.children {
        c += count_subst_defs(ch, name);
    }
    c
}

fn dupname_subst_defs(node: &mut Node, name: &str, remaining: &mut usize) {
    if *remaining == 0 {
        return;
    }
    if node.kind == "substitution_definition" && node.attrs.names.iter().any(|n| n == name) {
        node.attrs.names.retain(|n| n != name);
        node.attrs.dupnames.push(name.to_string());
        *remaining -= 1;
        return;
    }
    for ch in &mut node.children {
        dupname_subst_defs(ch, name, remaining);
        if *remaining == 0 {
            return;
        }
    }
}

/// Like [`reference_name_from_link`] but returns the reference TEXT
/// (simple name or phrase) before normalization — docutils is_reference().
fn reference_data_from_link(link: &str) -> Option<String> {
    let joined = ids::whitespace_normalize_name(link);
    let body = joined.strip_suffix('_')?;
    if body.ends_with('\\') {
        return None;
    }
    if let Some(phrase) = body.strip_prefix('`').and_then(|b| b.strip_suffix('`')) {
        if phrase.is_empty() {
            return None;
        }
        return Some(phrase.to_string());
    }
    if !body.is_empty()
        && !body.ends_with('_')
        && !body.contains(char::is_whitespace)
        && !body.contains('`')
        && !body.contains('\\')
    {
        return Some(body.to_string());
    }
    None
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
                sphinx: false,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                found_docs: None,
            },
        )
        .root
        .pformat()
    }

    /// Same, with sphinx's directive set and node overrides enabled.
    fn pf_sphinx(src: &str) -> String {
        parse_rst(
            src,
            &ParseOptions {
                source_path: "<snippet>".into(),
                sphinx: true,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                found_docs: None,
            },
        )
        .root
        .pformat()
    }

    /// docutils registers a figure's `:name:` on the *image*
    /// (`Image.run` -> `add_name`); sphinx pops the option first and applies
    /// it to the figure instead (`directives/patches.py:33-56`). The
    /// difference is load-bearing: `numfig` keys figure numbers off
    /// `figure['ids'][0]`, and `:ref:`/`:numref:` resolve to that node.
    ///
    /// The sphinx half was re-verified against the 9.1.0 oracle in wave-4
    /// task 9 (`.. figure:: pic.png` + `:name: myfig` →
    /// `<figure ids="myfig" names="myfig">` over `<image ...>`), but the
    /// case cannot join `tests/fixtures/sphinx_doctree_differential.json`:
    /// a figure needs an `image`, and `ImageCollector.process_doc` stamps
    /// every image with `candidates="{'*': 'pic.png'}"`, one of that
    /// corpus's enumerated excluded divergences. This assertion is the
    /// standing pin until image collection lands.
    #[test]
    fn a_figure_name_lands_on_the_image_in_docutils_and_the_figure_in_sphinx() {
        let src = ".. figure:: pic.png\n   :name: fig one\n\n   Caption.\n";

        let docutils = pf(src);
        assert!(
            docutils.contains(r#"<image ids="fig-one" names="fig\ one" uri="pic.png">"#),
            "{docutils}"
        );
        assert!(docutils.contains("<figure>"), "{docutils}");

        let sphinx = pf_sphinx(src);
        assert!(
            sphinx.contains(r#"<figure ids="fig-one" names="fig\ one">"#),
            "{sphinx}"
        );
        assert!(sphinx.contains(r#"<image uri="pic.png">"#), "{sphinx}");
    }

    /// sphinx returns early — without re-applying the popped `:name:` —
    /// when the figure came back with an error node, so neither node ends
    /// up named.
    #[test]
    fn a_figure_whose_caption_is_malformed_keeps_no_name() {
        let sphinx = pf_sphinx(".. figure:: pic.png\n   :name: fig-bad\n\n   - not a caption\n");
        // (the raw source is echoed inside the error's literal_block, so
        // this checks the attributes, not the text)
        assert!(!sphinx.contains(r#"ids="fig-bad""#), "{sphinx}");
        assert!(sphinx.contains("<figure>\n"), "{sphinx}");
        assert!(sphinx.contains("<system_message"), "{sphinx}");
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
                sphinx: false,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                found_docs: None,
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
