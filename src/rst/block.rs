//! The block-level recursive-descent parser (M2 wave 1).
//!
//! Model: docutils' `RSTStateMachine` re-expressed as recursive descent over
//! dedented line views. Every nested construct materializes a `Vec<LineRec>`
//! dedented to its own base column (docutils `get_indented` does the same),
//! so all productions parse "at column 0". Each `LineRec` carries `(source,
//! lineno)` provenance and a byte range into the parser-owned source-text
//! table — indices, not borrows, so a directive can splice an included
//! file's lines into the running stream (see [`SpliceRequest`]).
//!
//! Dispatch order matches docutils `Body.initial_transitions`: bullet,
//! enumerator, doctest, line_block, explicit markup, anonymous target,
//! adornment line, text (underline-title / definition list / paragraph).
//! Behavior sources: probe notes (2026-08-07-m2-wave1-probes.md) and the
//! committed differential fixture — never memory.

use crate::doctree::ids::{self, IdRegistry};
use crate::doctree::{kinds, messages, AttrValue, Node, Span};
use crate::utils::py_splitlines;
use unicode_normalization::UnicodeNormalization;

use std::sync::Arc;

use super::diagnostics::Reporter;
use super::lines::{LineRec, Lines};

const ADORNMENT_CHARS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
const BULLET_CHARS: [char; 6] = ['*', '+', '-', '\u{2022}', '\u{2023}', '\u{2043}'];

/// A pending block-quote segment: accumulated body lines plus an optional
/// (attribution node, marker lineno) that closed it.
type QuoteSegment = (Vec<LineRec>, Option<(Node, u32)>);

/// The parser-owned source table `LineRec::source` and `Span::source`
/// index: per source, its path (what messages stamp and
/// `Doctree::sources` publishes) and its *processed* text (what every
/// `LineRec` byte range slices).
///
/// Texts are `Arc<str>` so a caller that must hold line text across an
/// `&mut self` call can clone the handle ([`SourceTable::arc`]) and slice
/// a local instead of borrowing the parser — which is also what keeps a
/// mid-parse push (an included file arriving) free of self-reference.
#[derive(Debug, Default)]
pub(crate) struct SourceTable {
    paths: Vec<Arc<str>>,
    texts: Vec<Arc<str>>,
}

impl SourceTable {
    /// Append a source; returns its id, or `None` once the u16 id space is
    /// exhausted (a totality guard, like `MAX_NEST_DEPTH` — real documents
    /// never approach 65k sources).
    fn push(&mut self, path: Arc<str>, text: Arc<str>) -> Option<u16> {
        let id = u16::try_from(self.paths.len()).ok()?;
        self.paths.push(path);
        self.texts.push(text);
        Some(id)
    }

    fn path(&self, source: u16) -> &str {
        &self.paths[source as usize]
    }

    /// Shared handle on a path, for passing while `self` is mutably
    /// borrowed elsewhere.
    fn arc_path(&self, source: u16) -> Arc<str> {
        Arc::clone(&self.paths[source as usize])
    }

    fn text(&self, source: u16) -> &str {
        &self.texts[source as usize]
    }

    /// Shared handle on a source's processed text: slice a local clone
    /// instead of borrowing the parser when the text must stay usable
    /// across an `&mut self` call.
    fn arc(&self, source: u16) -> Arc<str> {
        Arc::clone(&self.texts[source as usize])
    }

    /// The current view of `rec` — valid until the next dedent/re-wrap of
    /// the record, unaffected by table growth.
    fn line_text(&self, rec: LineRec) -> &str {
        rec.slice(self.text(rec.source))
    }

    fn len(&self) -> usize {
        self.paths.len()
    }

    fn into_paths(self) -> Vec<String> {
        self.paths.iter().map(|p| p.to_string()).collect()
    }
}

/// What a directive hands back besides the nodes it pushed: lines of new
/// sources to insert into the running line stream right after the
/// directive (the `include` directive is the shipping producer). Plain
/// data — a directive builds one from its input alone, with no access to
/// parser internals.
///
/// Segments become consecutive source-table entries spliced contiguously
/// at the parse loop's cursor. `include` uses three, mirroring docutils'
/// `StateMachine.insert_input` layout (`statemachine.py:385-393`,
/// [INC PROBE 1]): a padding blank with synthetic source `internal
/// padding before <source>` (offset −1, here lineno 0), the included
/// lines plus the appended `''` and `.. end of inclusion from "<source>"`
/// marker pair (both carrying the included source with continuing
/// linenos), and a padding blank `internal padding after <source>`
/// (offset len, here lineno len+1).
#[derive(Debug)]
pub(crate) struct SpliceRequest {
    pub segments: Vec<SpliceSegment>,
}

/// One source's worth of spliced lines.
#[derive(Debug)]
pub(crate) struct SpliceSegment {
    /// Lines of the new source (re-processed — tab expansion at width 8,
    /// trailing-whitespace strip — on insertion; producers hand over
    /// already-processed text, for which that is a no-op).
    pub lines: Vec<String>,
    /// The path messages and spans attribute the lines to.
    pub source_path: String,
    /// Line number of the first line (subsequent lines count up from it).
    pub first_lineno: u32,
}

impl SpliceRequest {
    /// A single-segment request numbering its lines from 1.
    #[cfg(test)]
    fn single(lines: Vec<String>, source_path: String) -> SpliceRequest {
        SpliceRequest {
            segments: vec![SpliceSegment {
                lines,
                source_path,
                first_lineno: 1,
            }],
        }
    }
}

/// What running a directive produced beyond its nodes.
enum DirectiveOutcome {
    Done,
    Splice(SpliceRequest),
}

/// A glossary comment line: unindented and opening with `.. `
/// (`domains/std/__init__.py:452`, `line.startswith('.. ')` — the trailing
/// space is part of the test, so a bare `..` is still a term).
fn is_glossary_comment(line: &LineRec, line_text: &str) -> bool {
    line.indent() == 0 && line_text.starts_with(".. ")
}

/// Byte offset of the character `n_chars` into `text` (its length when
/// the text is shorter) — for re-wrapping a `LineRec` view past a marker
/// (char-aware: unicode bullets are multi-byte).
fn rest_after_offset(text: &str, n_chars: usize) -> usize {
    match text.char_indices().nth(n_chars) {
        Some((i, _)) => i,
        None => text.len(),
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

pub(crate) struct BlockParser {
    top: Vec<LineRec>,
    /// Source table (paths + processed texts). Entry 0 is the document;
    /// sub-parses over lifted text and spliced sources push.
    sources: SourceTable,
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
    /// Set inside a [`Self::parse_detached`] parse: the line its text
    /// starts at. docutils numbers a StringList built from lifted text
    /// (a caption, a csv cell) from offset 0, and the directive lookup's
    /// INFO takes its line from `document.current_line` — that item's own
    /// offset + 1 (`nodes.py:2091-2099`, `statemachine.py:497-503`) — so
    /// there the INFO line is relative to the text while every other
    /// message's stays absolute (`abs_line_number`).
    detached_first_lineno: Option<u32>,
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
    /// The py-domain configuration the read phase consumes (see
    /// [`super::ParseOptions::py`]).
    pub(crate) py: crate::py::PySigConfig,
    /// The project source directory (see [`super::ParseOptions::srcdir`]):
    /// `Some` switches the `include` directive to sphinx-mode path
    /// resolution and turns its `included`/`dependencies` recording on.
    pub(crate) srcdir: Option<std::path::PathBuf>,
    /// [`super::ParseOptions::source_encoding`]: the sphinx-mode default
    /// for `include`'s and `literalinclude`'s `:encoding:`.
    pub(crate) source_encoding: String,
    /// `.. highlight::` state consumed by later code-blocks in the same
    /// document (sphinx env.temp_data\['highlight_language'\]).
    highlight_language: Option<String>,
    /// [`super::ParseOptions::highlight_language`]: the configured
    /// `highlight_language` a code-block takes when no `.. highlight::` is
    /// in force (sphinx `self.config.highlight_language`).
    pub(crate) config_highlight_language: String,
    /// `.. program::` state consumed by later `.. option::` directives in
    /// the same document (sphinx `env.ref_context['std:program']`).
    program: Option<String>,
    /// sphinx `env.ref_context['py:module']` — set by `py:module`/
    /// `py:currentmodule` and pushed/popped by an object's `:module:`
    /// option (`domains/python/_object.py:477-480`, `498-503`).
    py_module: Option<String>,
    /// sphinx `env.ref_context['py:modules']` — the `:module:` option's
    /// push/pop stack (entries may be `None`: the module in scope when the
    /// option pushed).
    py_modules: Vec<Option<String>>,
    /// sphinx `env.ref_context['py:class']` — the innermost class scope
    /// (`_object.py:449-503`).
    py_class: Option<String>,
    /// sphinx `env.ref_context['py:classes']` — the `allow_nesting`
    /// (class/exception) nesting stack.
    py_classes: Vec<String>,
    /// Key-existence flags mirroring `'py:class' in env.ref_context` etc. —
    /// the `:any:` role copies every EXISTING ref_context key onto its
    /// pending_xref (`AnyXRefRole.process_link`), and a key can exist
    /// holding `None`/`[]`, which the value fields above cannot express
    /// (see [`super::inline::RefContext`]).
    py_class_key: bool,
    py_classes_key: bool,
    /// `'py:module' in env.ref_context`. Tracked separately from
    /// [`Self::py_module`] because the key can exist holding Python `None`
    /// — `after_content` ASSIGNS `modules.pop()`, and `before_content`
    /// pushed `ref_context.get('py:module')`, which is `None` when the
    /// `:module:`-carrying directive had no enclosing module scope
    /// (`_object.py:477-480` / `:498-503`; research spec §8 trap 14).
    py_module_key: bool,
    py_modules_key: bool,
    /// Sphinx-mode class/rst-class pending classes (the ClassAttribute
    /// transform effect applied inline).
    pending_classes: Option<Vec<String>>,
    /// Per-document equation counter (math domain numbering).
    equation_serial: u32,
    /// Validation-feed records collected during the parse.
    directive_records: Vec<super::DirectiveRecord>,
    role_records: Vec<super::RoleRecord>,
    toctree_records: Vec<super::ToctreeRecord>,
    /// The std-domain registrations the object-description directives made
    /// while running, in document order (see
    /// [`super::RegistryExport::program_options`] for why the finished
    /// doctree cannot carry them).
    program_option_records: Vec<super::ProgramOptionRecord>,
    std_object_records: Vec<super::ObjectRegistration>,
    /// The `glossary` directives' term registrations — see
    /// [`super::RegistryExport::glossary_terms`].
    glossary_term_records: Vec<super::GlossaryTermRecord>,
    /// The py-domain registrations (`PythonDomain.note_object` /
    /// `note_module` calls), in document order — see
    /// [`super::RegistryExport::py_objects`].
    py_object_records: Vec<super::PyObjectRecord>,
    py_module_records: Vec<super::PyModuleRecord>,
    /// The document's diagnostics stream: every message this parser
    /// creates is reported here as it is created, with the directives'
    /// logger warnings and the registrations' sequence numbers in between
    /// (see [`Reporter`]). A detached sub-parse borrows it, like the id
    /// registry.
    reporter: Reporter,
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
    /// A [`SpliceRequest`] a directive just returned, waiting for the
    /// enclosing block-parse loop to insert it at its cursor.
    pending_splice: Option<SpliceRequest>,
    /// docutils `document.include_log` (`nodes.py:1802-1803`): the open
    /// inclusions as `(source display path, clip options)` pairs. Seeded
    /// with the root document (and an empty clip) on first use
    /// (`misc.py:253-256`); an entry is pushed before each splice and
    /// popped when the comment path reaches the matching
    /// `.. end of inclusion from "..."` marker.
    include_log: Vec<(String, IncludeClip)>,
    /// Parse-recorded dependency paths (see
    /// [`super::RegistryExport::dependencies`]); sphinx mode only.
    dependency_records: Vec<String>,
    /// Parse-recorded included docnames (see
    /// [`super::RegistryExport::included`]); sphinx mode only.
    included_records: Vec<String>,
    /// Where the top-level input ended: the `(source, line)` one past its
    /// last line (see [`super::ParseOutput::end_of_input`]), set when
    /// [`Self::parse_document_impl`] has consumed it.
    end_of_input: Option<(u16, u32)>,
    /// Where the top-level element parsed last left docutils' top-level
    /// cursor ([`TopCursor`]), which [`Self::end_of_input`] reports.
    top_cursor: TopCursor,
}

/// Where docutils' top-level state machine leaves its cursor after the
/// last top-level element — what a message raised with no node is located
/// by once the parse is over (`Reporter.get_source_and_line`, bound to that
/// machine at `states.py:244-246`, reads `StringList.info(line_offset)`,
/// `statemachine.py:358-377,1299-1307`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum TopCursor {
    /// One past the last line ("just past the end"): the location is the
    /// last line's source, one line on. The run loop's last `next_line`
    /// stops there.
    #[default]
    JustPast,
    /// Further on: the element's nested list parse (`nested_list_parse`,
    /// `states.py:382-424`) ran to the end of the input — trailing blank
    /// lines included, which the explicit list and a line block read only
    /// through their last construct — and `goto_line` moved the cursor
    /// there, so the run loop's `next_line` steps beyond it and `info()`
    /// raises `IndexError` — no source, no line.
    Beyond,
    /// On the last line: a `::` paragraph that is the input's last line
    /// expects a literal block, and the empty quoted-literal parse steps
    /// back a line before handing its offset back (`QuotedLiteralBlock.
    /// eof`, `:3211-3225`; `Text.eof` ends the run there, `:2854-2857`).
    LastLine,
}

#[derive(Debug, Default)]
struct SubstCtx {
    ltrim: bool,
    rtrim: bool,
}

impl BlockParser {
    /// Parser over one document: the single-source form (entry 0 = the
    /// document, linenos `1..=n`).
    pub(crate) fn new(source: &str, source_path: &str) -> Self {
        let (text, top) = Lines::new(source).into_parts();
        let mut sources = SourceTable::default();
        sources
            .push(Arc::from(source_path), Arc::from(text))
            .expect("a fresh table accepts entry 0");
        BlockParser::from_parts(top, sources)
    }

    fn from_parts(top: Vec<LineRec>, sources: SourceTable) -> Self {
        BlockParser {
            top,
            sources,
            registry: IdRegistry::new(),
            styles: Vec::new(),
            depth: 0,
            line_bias: 0,
            nested_node_kind: None,
            detached_first_lineno: None,
            sphinx: false,
            docname: "index".to_string(),
            found_docs: None,
            exclude_patterns: Vec::new(),
            py: crate::py::PySigConfig::default(),
            srcdir: None,
            source_encoding: super::DEFAULT_SOURCE_ENCODING.to_string(),
            highlight_language: None,
            config_highlight_language: super::DEFAULT_HIGHLIGHT_LANGUAGE.to_string(),
            program: None,
            py_module: None,
            py_modules: Vec::new(),
            py_class: None,
            py_classes: Vec::new(),
            py_class_key: false,
            py_classes_key: false,
            py_module_key: false,
            py_modules_key: false,
            pending_classes: None,
            equation_serial: 0,
            directive_records: Vec::new(),
            role_records: Vec::new(),
            toctree_records: Vec::new(),
            program_option_records: Vec::new(),
            std_object_records: Vec::new(),
            glossary_term_records: Vec::new(),
            py_object_records: Vec::new(),
            py_module_records: Vec::new(),
            reporter: Reporter::default(),
            substitution_ctx: None,
            substitution_names_seen: Vec::new(),
            substitution_dupnames: Vec::new(),
            pending_splice: None,
            include_log: Vec::new(),
            dependency_records: Vec::new(),
            included_records: Vec::new(),
            end_of_input: None,
            top_cursor: TopCursor::JustPast,
        }
    }

    /// parse_document plus the flat build-pipeline records.
    pub(crate) fn parse_document_full(mut self) -> super::ParseOutput {
        let root = self.parse_document_impl();
        // Read before the stream is taken below: the transform pass
        // continues the document's numbering from here.
        let next_seq = self.reporter.peek_seq();
        // Harvest the id/name registry before it drops with `self`: wave 4's
        // std-domain label harvest needs name -> (id, explicit) data that
        // otherwise dies with the BlockParser.
        let registry = super::RegistryExport {
            nameids: self.registry.nameids_snapshot(),
            index_serial: self.registry.index_serial(),
            program_options: std::mem::take(&mut self.program_option_records),
            std_objects: std::mem::take(&mut self.std_object_records),
            glossary_terms: std::mem::take(&mut self.glossary_term_records),
            py_objects: std::mem::take(&mut self.py_object_records),
            py_modules: std::mem::take(&mut self.py_module_records),
            diagnostics: std::mem::take(&mut self.reporter).take(),
            dependencies: std::mem::take(&mut self.dependency_records),
            included: std::mem::take(&mut self.included_records),
            // Made by the read transforms (619), after the parse.
            citations: Vec::new(),
            // Collected by the read pass (MetadataCollector, 880).
            metadata: Default::default(),
        };
        super::ParseOutput {
            doctree: crate::doctree::Doctree {
                root,
                sources: self.sources.into_paths(),
            },
            directive_records: std::mem::take(&mut self.directive_records),
            role_records: std::mem::take(&mut self.role_records),
            toctrees: std::mem::take(&mut self.toctree_records),
            registry,
            ids: self.registry,
            next_seq,
            end_of_input: self.end_of_input,
        }
    }

    /// Validation-feed record with the M1 validation-scanner's semantics
    /// (spec-INdependent, so registered and unknown directives record the
    /// same way): whitespace-split args, marker-line text routed to
    /// content for the admonition name set, raw string options.
    fn capture_directive_record(
        &mut self,
        name: &str,
        first_line: &LineRec,
        block: &[LineRec],
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
        let marker_text = self.sources.line_text(*first_line).trim();
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
            let text = self.sources.line_text(*l);
            if in_options {
                if let Some((oname, body_start)) = field_marker(text.trim_start()) {
                    let base = text.len() - text.trim_start().len();
                    let val = text[base + body_start..].trim().to_string();
                    options.push((oname, val));
                    continue;
                }
                in_options = false;
            }
            content_lines.push(text.to_string());
        }
        while content_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            content_lines.pop();
        }
        let arguments: Vec<String> = if is_admonition {
            Vec::new()
        } else {
            crate::utils::py_split(marker_text)
                .map(str::to_string)
                .collect()
        };
        self.directive_records.push(super::DirectiveRecord {
            source: first_line.source,
            name: name.to_string(),
            arguments,
            options,
            content: content_lines.join("\n"),
            line: lineno,
        });
    }

    /// Inline parse through the parser's own registry/mode; collects role
    /// records emitted by the inliner. Messages the inliner raises stamp
    /// the span's own source path.
    ///
    /// The inline parse is atomic here, so its messages are reported as it
    /// returns — which is when they were created (see
    /// [`super::inline::InlineResult::messages`]).
    fn inline(&mut self, text: &str, span: Span, lineno: u32) -> super::inline::InlineResult {
        let source_path = self.sources.arc_path(span.source);
        let mut result = super::inline::parse_inline_ext(
            text,
            span,
            lineno,
            &mut self.registry,
            &source_path,
            self.sphinx,
            &self.docname,
            super::inline::RefContext {
                program: self.program.as_deref(),
                py_module: self.py_module.as_deref(),
                py_class: self.py_class.as_deref(),
                py_class_key: self.py_class_key,
                py_classes_key: self.py_classes_key,
                py_module_key: self.py_module_key,
                py_modules_key: self.py_modules_key,
            },
            &self.py,
        );
        self.role_records.append(&mut result.roles);
        for msg in &result.messages {
            self.reporter.report(msg);
        }
        result
    }

    /// parse_elements with the containing node kind recorded (docutils
    /// nested_parse: `state_machine.node` = the container element).
    fn parse_nested(&mut self, lines: &[LineRec], kind: &'static str) -> Vec<Node> {
        let saved = self.nested_node_kind.replace(kind);
        let nodes = self.parse_elements(lines);
        self.nested_node_kind = saved;
        nodes
    }

    /// Nested parse over OWNED text (csv-table cells and, later,
    /// rst_prolog): a sub-parser over a new source-table entry, sharing
    /// this parser's table and id registry, with linenos starting at
    /// `first_lineno` so absolute line numbers keep working. The entry's
    /// path copies `attribute_to`'s — the source the text was lifted from
    /// — so messages and spans keep attributing to it.
    fn parse_detached(
        &mut self,
        text: &str,
        first_lineno: u32,
        attribute_to: u16,
        kind: &'static str,
    ) -> Vec<Node> {
        let path = self.sources.arc_path(attribute_to);
        let Some((_id, top)) = self.push_source(path, text, first_lineno) else {
            // Source-id space exhausted (totality guard): drop the nested
            // content rather than mis-attribute it.
            return Vec::new();
        };
        let mut sub = BlockParser::from_parts(top, std::mem::take(&mut self.sources));
        sub.registry = std::mem::replace(&mut self.registry, IdRegistry::new());
        sub.nested_node_kind = Some(kind);
        sub.detached_first_lineno = Some(first_lineno);
        sub.line_bias = self.line_bias;
        sub.depth = self.depth;
        // Mode + records must flow through the detached parse (review
        // finding: csv cells previously parsed in docutils mode and their
        // directive/role records were dropped).
        sub.sphinx = self.sphinx;
        sub.docname = self.docname.clone();
        sub.found_docs = self.found_docs.clone();
        sub.exclude_patterns = self.exclude_patterns.clone();
        sub.py = self.py.clone();
        sub.srcdir = self.srcdir.clone();
        sub.highlight_language = self.highlight_language.clone();
        sub.config_highlight_language = self.config_highlight_language.clone();
        sub.program = self.program.clone();
        // The py ref_context flows in like `program` (state changes made
        // inside a detached parse stay local, matching the wave-4
        // convention); the record streams flow back out below.
        sub.py_module = self.py_module.clone();
        sub.py_modules = self.py_modules.clone();
        sub.py_class = self.py_class.clone();
        sub.py_classes = self.py_classes.clone();
        sub.py_class_key = self.py_class_key;
        sub.py_classes_key = self.py_classes_key;
        sub.py_module_key = self.py_module_key;
        sub.py_modules_key = self.py_modules_key;
        // The include log is document-level state shared with every nested
        // state machine in docutils (`misc.py:251-262` reads it through
        // `self.state.document`), so it transfers in and back out.
        sub.include_log = std::mem::take(&mut self.include_log);
        // One stream per document: the sub-parse's messages are this
        // document's, numbered in the same sequence.
        sub.reporter = std::mem::take(&mut self.reporter);
        let top = std::mem::take(&mut sub.top);
        let nodes = sub.parse_elements(&top);
        self.sources = sub.sources;
        self.registry = sub.registry;
        self.include_log = std::mem::take(&mut sub.include_log);
        self.reporter = std::mem::take(&mut sub.reporter);
        self.directive_records.append(&mut sub.directive_records);
        self.role_records.append(&mut sub.role_records);
        self.toctree_records.append(&mut sub.toctree_records);
        self.program_option_records
            .append(&mut sub.program_option_records);
        self.std_object_records.append(&mut sub.std_object_records);
        self.glossary_term_records
            .append(&mut sub.glossary_term_records);
        self.py_object_records.append(&mut sub.py_object_records);
        self.py_module_records.append(&mut sub.py_module_records);
        self.dependency_records.append(&mut sub.dependency_records);
        self.included_records.append(&mut sub.included_records);
        nodes
    }

    /// Process `text` into a new source-table entry named `path`; returns
    /// the entry's id and record stream (`None` when the id space is
    /// exhausted).
    fn push_source(
        &mut self,
        path: Arc<str>,
        text: &str,
        first_lineno: u32,
    ) -> Option<(u16, Vec<LineRec>)> {
        // The id is only known after the push, but the recs need it up
        // front — take it from the table length the push will use.
        let id = u16::try_from(self.sources.len()).ok()?;
        let (processed, recs) = Lines::for_source(text, id, first_lineno).into_parts();
        self.sources.push(path, Arc::from(processed))?;
        Some((id, recs))
    }

    /// Insert a [`SpliceRequest`]'s lines into `lines` at `at` (the
    /// block-parse loop's cursor, right past the directive that returned
    /// it): each segment's text becomes a new source-table entry and their
    /// records join the running stream contiguously. On id-space
    /// exhaustion the remaining segments are dropped (same totality guard
    /// as [`Self::push_source`]).
    fn apply_splice(&mut self, lines: &mut Vec<LineRec>, at: usize, request: SpliceRequest) {
        let mut recs: Vec<LineRec> = Vec::new();
        for segment in request.segments {
            let SpliceSegment {
                lines: raw_lines,
                source_path,
                first_lineno,
            } = segment;
            let text = raw_lines.join("\n");
            let Some((id, mut segment_recs)) =
                self.push_source(Arc::from(source_path), &text, first_lineno)
            else {
                break;
            };
            // `Lines` never yields a line after the final newline (and an
            // all-empty text yields none at all), but a segment's line
            // count is authoritative — a padding segment IS one blank
            // line. Restore trailing blanks as zero-width views.
            while segment_recs.len() < raw_lines.len() {
                let lineno = first_lineno + segment_recs.len() as u32;
                let end = self.sources.text(id).len() as u32;
                segment_recs.push(LineRec::new(id, lineno, end, end, ""));
            }
            recs.extend(segment_recs);
        }
        let at = at.min(lines.len());
        lines.splice(at..at, recs);
    }

    /// Re-wrap `rec` to the `from..to` byte sub-range of its current view
    /// (a marker consumed, a table-cell column carved): the leading-space
    /// cache is recomputed for the shrunk view.
    fn rewrap_range(&self, rec: LineRec, from: usize, to: usize) -> LineRec {
        let start = rec.start + from as u32;
        let end = rec.start + to as u32;
        debug_assert!(from <= to && end <= rec.end);
        LineRec::new(
            rec.source,
            rec.lineno,
            start,
            end,
            &self.sources.text(rec.source)[start as usize..end as usize],
        )
    }

    /// Re-wrap `rec` past the first `byte_off` bytes of its current view.
    fn rewrap_from(&self, rec: LineRec, byte_off: usize) -> LineRec {
        self.rewrap_range(rec, byte_off, (rec.end - rec.start) as usize)
    }

    /// A zero-width (blank) view at the start of `rec`'s line, keeping its
    /// provenance — the shape table cells use for their blank rows.
    fn blank_at(&self, rec: LineRec) -> LineRec {
        self.rewrap_range(rec, 0, 0)
    }

    /// The lines' current views joined with `\n`.
    fn join_lines(&self, lines: &[LineRec]) -> String {
        let mut joined = String::new();
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                joined.push('\n');
            }
            joined.push_str(self.sources.line_text(*l));
        }
        joined
    }

    fn span_of(&self, lines: &[LineRec], first: usize, last: usize) -> Span {
        let first_rec = lines.get(first);
        let start = first_rec.map(|l| l.start).unwrap_or(0);
        let end = lines
            .get(last.min(lines.len().saturating_sub(1)))
            .map(|l| l.end)
            .unwrap_or(start);
        Span {
            source: first_rec.map(|l| l.source).unwrap_or(0),
            line: first_rec.map(|l| l.lineno).unwrap_or(0),
            start,
            end,
        }
    }

    /// A `system_message` anchored at `lineno` of `source` — the message
    /// stamps that source's table path — CREATED: reported to the stream
    /// now, as docutils' `Reporter.system_message` writes it
    /// (`docutils/utils/__init__.py:213-215`). Only call this where
    /// docutils creates the message; a child docutils passes at creation
    /// goes in through [`Self::msg_literal`], and one it appends later
    /// (the `DirectiveError` literal) after this returns.
    fn msg(&self, level: u8, text: &str, source: u16, lineno: u32) -> Node {
        self.created(messages::system_message(
            level,
            text,
            source,
            lineno,
            self.sources.path(source),
        ))
    }

    /// For state-machine-position-derived messages (see `line_bias`);
    /// created like [`Self::msg`].
    fn msg_sm(&self, level: u8, text: &str, source: u16, lineno: u32) -> Node {
        self.msg(level, text, source, lineno + self.line_bias)
    }

    /// [`Self::msg`] with the `literal_block` docutils passes to the
    /// Reporter at creation (`nodes.literal_block(blocktext, blocktext)` as
    /// a `system_message` child), so the stream prints it too.
    fn msg_literal(&self, level: u8, text: &str, source: u16, lineno: u32, raw: &str) -> Node {
        self.created(messages::with_literal(
            messages::system_message(level, text, source, lineno, self.sources.path(source)),
            raw,
        ))
    }

    /// Report a message the moment it exists, then hand it on.
    fn created(&self, msg: Node) -> Node {
        self.reporter.report(&msg);
        msg
    }

    /// Probe-verified: an explicit-markup element (comment/target) followed
    /// by an ADJACENT non-blank column-0 line that is not itself explicit
    /// markup warns. Consecutive `..`/`__ ` items chain without warning.
    fn warn_explicit_markup_end(&self, lines: &[LineRec], pos: usize, out: &mut Vec<Node>) {
        if let Some(l) = lines.get(pos) {
            let text = self.sources.line_text(*l);
            let explicit_ish = text == ".." || text.starts_with(".. ") || text.starts_with("__ ");
            if !l.is_blank() && l.indent() == 0 && !explicit_ish {
                out.push(self.msg(
                    messages::WARNING,
                    "Explicit markup ends without a blank line; unexpected unindent.",
                    l.source,
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
                line: 1,
                start: 0,
                end: self.sources.text(0).len() as u32,
            },
        );
        root.set("source", AttrValue::Str(self.sources.path(0).to_string()));

        // Open sections, deepest last; nodes attach on close.
        let mut stack: Vec<Node> = Vec::new();
        let mut lines = std::mem::take(&mut self.top);
        let mut pos = 0usize;
        while pos < lines.len() {
            if lines[pos].is_blank() {
                pos += 1;
                continue;
            }
            let mut out = Vec::new();
            self.top_cursor = TopCursor::JustPast;
            let section = self.parse_element(&lines, &mut pos, true, &mut out);
            self.apply_pending_classes(&mut out, 0);
            for node in out {
                Self::container(&mut root, &mut stack).children.push(node);
            }
            if let Some(start) = section {
                self.open_section(start, &mut root, &mut stack);
            }
            // A directive just asked for new lines at the cursor (the
            // `include` directive's insert mode).
            if let Some(request) = self.pending_splice.take() {
                self.apply_splice(&mut lines, pos, request);
            }
        }
        while !stack.is_empty() {
            Self::close_section(&mut root, &mut stack);
        }
        // docutils' `StringList.info` "just past the end" (`statemachine.py:
        // 1299-1307`) through `get_source_and_line` (`:358-377`): the last
        // input line's source, one line on — spliced `include` lines count,
        // as they do in docutils' input list. Unless the last top-level
        // element left the cursor elsewhere ([`TopCursor`]).
        self.end_of_input = match self.top_cursor {
            TopCursor::JustPast => lines.last().map(|line| (line.source, line.lineno + 1)),
            TopCursor::Beyond => None,
            TopCursor::LastLine => lines.last().map(|line| (line.source, line.lineno)),
        };

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

    /// The element being parsed is one whose nested list parse, at the top
    /// level, runs on to the end of the input — or stops early, in which
    /// case the next top-level element resets the cursor
    /// ([`TopCursor::Beyond`]). Nested parses (depth above 0) move only a
    /// nested machine's cursor.
    fn note_cursor_beyond(&mut self) {
        if self.depth == 0 {
            self.top_cursor = TopCursor::Beyond;
        }
    }

    /// The explicit construct just parsed, ending before `pos`, reads no
    /// trailing blank line: a hyperlink or anonymous target
    /// (`until_blank=True`), an empty comment or the end-of-inclusion
    /// marker (nothing at all). The explicit list's nested parse stops at a
    /// blank line (`Explicit.blank` is `invalid_input`, `states.py:2803`),
    /// so when one follows, the top level eats it and its cursor ends just
    /// past the input ([`TopCursor::JustPast`]); at the very end of the
    /// input the list ran to it ([`TopCursor::Beyond`], set on dispatch).
    fn note_blank_left_to_top(&mut self, lines: &[LineRec], pos: usize) {
        if self.depth == 0 && pos < lines.len() {
            self.top_cursor = TopCursor::JustPast;
        }
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
            // Both children are passed at creation (`check_subsection`,
            // `states.py:451-459`), so the stream prints them. The title's
            // own messages (a short underline) were created — and printed —
            // already; docutils drops them from the tree here.
            let msg = self.created(messages::with_paragraph(
                messages::with_literal(
                    messages::system_message(
                        messages::ERROR,
                        &text,
                        start.span.source,
                        start.title_lineno,
                        self.sources.path(start.span.source),
                    ),
                    &start.raw_lines,
                ),
                &format!("Established title styles: {}", established.join(" ")),
            ));
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
        // The section's stamped line is one past its span's first line —
        // docutils creates the section only once the state machine has
        // consumed the underline, so it reports the underline line for the
        // plain form and the title line for the overline form.
        let mut section_span = start.span;
        section_span.line += 1;
        let mut section = Node::elem(kinds::SECTION, section_span);
        section
            .attrs
            .names
            .push(ids::fully_normalize_name(&title.astext()));
        let source_path = self.sources.arc_path(start.span.source);
        let dup_info = self.registry.set_id_implicit(
            &mut section,
            start.underline_lineno,
            start.span.source,
            &source_path,
        );
        let dup_info = dup_info.map(|msg| self.created(msg));
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

    fn parse_elements(&mut self, lines: &[LineRec]) -> Vec<Node> {
        if self.depth >= MAX_NEST_DEPTH {
            // sphinx-ultra-specific totality guard (docutils crashes here).
            let anchor = lines
                .first()
                .map(|l| (l.source, l.lineno))
                .unwrap_or((0, 1));
            return vec![self.msg(
                messages::ERROR,
                "Maximum nesting depth exceeded; deeper content skipped.",
                anchor.0,
                anchor.1,
            )];
        }
        self.depth += 1;
        let out = self.parse_elements_inner(lines);
        self.depth -= 1;
        out
    }

    fn parse_elements_inner(&mut self, lines: &[LineRec]) -> Vec<Node> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        // Once a directive splices new lines in, the loop continues over an
        // owned copy of the stream; until then the borrowed slice serves
        // (records are Copy, so the one-time copy is cheap and rare).
        let mut owned: Option<Vec<LineRec>> = None;
        loop {
            let cur: &[LineRec] = owned.as_deref().unwrap_or(lines);
            if pos >= cur.len() {
                break;
            }
            if cur[pos].is_blank() {
                pos += 1;
                continue;
            }
            let before = out.len();
            let section = self.parse_element(cur, &mut pos, false, &mut out);
            debug_assert!(section.is_none(), "titles never match in nested contexts");
            self.apply_pending_classes(&mut out, before);
            if let Some(request) = self.pending_splice.take() {
                let mut stream = owned.take().unwrap_or_else(|| lines.to_vec());
                self.apply_splice(&mut stream, pos, request);
                owned = Some(stream);
            }
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
        lines: &[LineRec],
        pos: &mut usize,
        match_titles: bool,
        out: &mut Vec<Node>,
    ) -> Option<SectionStart> {
        let line = lines[*pos];
        if line.indent() > 0 {
            self.parse_block_quote(lines, pos, out);
            return None;
        }
        // Local handle: the text must stay readable across the `&mut self`
        // dispatch calls below.
        let src = self.sources.arc(line.source);
        let text = line.slice(&src);

        // `bullet`, `enumerator`, `field_marker`, `option_marker`,
        // `explicit_markup`/`anonymous` and `Text.indent` (a definition
        // list) each parse the rest of their list with `nested_list_parse`
        // and `goto_line` past it (`states.py:1373-1387,1407-1439,
        // 1564-1576,1607-1634,2508-2527,2859-2874`). The explicit list stops
        // at a blank line the last construct did not read
        // ([`Self::note_blank_left_to_top`]).
        if let Some(bullet) = Self::bullet_marker(text) {
            self.note_cursor_beyond();
            self.parse_bullet_list(lines, pos, bullet, out);
            return None;
        }
        if let Some(e) = parse_enumerator(text) {
            if self.try_enumerated_list(lines, pos, &e, out) {
                self.note_cursor_beyond();
                return None;
            }
            // invalid list start: fall through to the text path
        }
        if field_marker(text).is_some() {
            self.note_cursor_beyond();
            self.parse_field_list(lines, pos, out);
            return None;
        }
        if option_group_marker(text).is_some() && self.option_item_viable(lines, *pos) {
            self.note_cursor_beyond();
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
            self.note_cursor_beyond();
            self.parse_explicit(lines, pos, out);
            return None;
        }
        if let Some(rest) = text.strip_prefix("__ ") {
            self.note_cursor_beyond();
            self.parse_anonymous_shortcut(lines, pos, rest, out);
            return None;
        }
        if text == "__" {
            // Bare `__`: anonymous internal target (fixture-verified).
            self.note_cursor_beyond();
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
        lines: &[LineRec],
        pos: &mut usize,
        ch: char,
        match_titles: bool,
        out: &mut Vec<Node>,
    ) -> Option<SectionStart> {
        let line = lines[*pos];
        let len = char_len(self.sources.line_text(line));
        let next = lines.get(*pos + 1).copied();
        let next_is_text = next.map(|n| !n.is_blank()).unwrap_or(false);

        if !match_titles {
            if len >= 4 {
                let msg = self.msg_literal(
                    messages::ERROR,
                    "Unexpected section title or transition.",
                    line.source,
                    line.lineno,
                    self.sources.line_text(line),
                );
                out.push(msg);
                *pos += 1;
            } else {
                // Fixture-verified: short adornments in nested contexts get
                // an INFO, then reprocess through the text state.
                out.push(self.msg(
                    messages::INFO,
                    "Unexpected possible title overline or transition.\nTreating it as ordinary text because it's so short.",
                    line.source,
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
                line.source,
                line.lineno,
            ));
            return self.handle_text(lines, pos, match_titles, out);
        }
        if adornment_char(self.sources.line_text(title_line)).is_some() {
            let literal = format!(
                "{}\n{}",
                self.sources.line_text(line),
                self.sources.line_text(title_line)
            );
            let msg = self.msg_literal(
                messages::ERROR,
                "Invalid section title or transition marker.",
                line.source,
                line.lineno,
                &literal,
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
            Some(u) if adornment_char(self.sources.line_text(u)).is_none() => Some((
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
                    self.sources.line_text(line),
                    self.sources.line_text(title_line),
                    self.sources.line_text(lines[*pos + 2])
                )
            } else {
                format!(
                    "{}\n{}",
                    self.sources.line_text(line),
                    self.sources.line_text(title_line)
                )
            };
            let msg = self.msg_literal(messages::ERROR, text, line.source, line.lineno, &literal);
            out.push(msg);
            *pos += consume;
            return None;
        }
        let under = under.unwrap();
        let under_text = self.sources.line_text(under);
        if adornment_char(under_text) != Some(ch) || char_len(under_text) != len {
            // Different char or different length: both are a mismatch.
            let literal = format!(
                "{}\n{}\n{}",
                self.sources.line_text(line),
                self.sources.line_text(title_line),
                self.sources.line_text(under)
            );
            let msg = self.msg_literal(
                messages::ERROR,
                "Title overline & underline mismatch.",
                line.source,
                line.lineno,
                &literal,
            );
            out.push(msg);
            *pos += 3;
            return None;
        }
        // Title column width (leading spaces included) wider than the
        // adornment: section is still created, WARNING inside.
        let mut msgs = Vec::new();
        let raw = format!(
            "{}\n{}\n{}",
            self.sources.line_text(line),
            self.sources.line_text(title_line),
            self.sources.line_text(under)
        );
        if column_width(self.sources.line_text(title_line)) > len {
            msgs.push(self.msg_literal(
                messages::WARNING,
                "Title overline too short.",
                line.source,
                line.lineno,
                &raw,
            ));
        }
        let span = self.span_of(lines, *pos, *pos + 2);
        let title_lineno = title_line.lineno;
        let underline_lineno = under.lineno;
        // `Line.text`: `title = title.rstrip()` then `section(title.lstrip(),
        // ...)` — Python's set at both ends (round F, `round_f` pin).
        let title_text = self
            .sources
            .line_text(title_line)
            .trim_matches(crate::utils::py_isspace)
            .to_string();
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
        lines: &[LineRec],
        pos: &mut usize,
        match_titles: bool,
        out: &mut Vec<Node>,
    ) -> Option<SectionStart> {
        let line = lines[*pos];
        let next = lines.get(*pos + 1).copied();

        if let Some(next) = next {
            if !next.is_blank() && next.indent() == 0 {
                if let Some(ch) = adornment_char(self.sources.line_text(next)) {
                    let title_len = column_width(self.sources.line_text(line));
                    let ul_len = char_len(self.sources.line_text(next));
                    if ul_len >= title_len || ul_len >= 4 {
                        let raw = format!(
                            "{}\n{}",
                            self.sources.line_text(line),
                            self.sources.line_text(next)
                        );
                        // `Text.underline` (`states.py:2879-2920`): the short-
                        // underline warning is created first — in a nested
                        // context too, where it precedes the unexpected-
                        // title error in the tree.
                        let mut msgs = Vec::new();
                        if ul_len < title_len {
                            msgs.push(self.msg_literal(
                                messages::WARNING,
                                "Title underline too short.",
                                next.source,
                                next.lineno,
                                &raw,
                            ));
                        }
                        if !match_titles {
                            out.append(&mut msgs);
                            out.push(self.msg_literal(
                                messages::ERROR,
                                "Unexpected section title.",
                                next.source,
                                next.lineno,
                                &raw,
                            ));
                            *pos += 2;
                            return None;
                        }
                        let span = self.span_of(lines, *pos, *pos + 1);
                        let title_lineno = line.lineno;
                        let underline_lineno = next.lineno;
                        // `Text.underline`: `title = context[0].rstrip()` —
                        // rstrip ONLY, so a leading NBSP stays in the title
                        // (round F, `round_f` pin; `trim()` had eaten it).
                        let title = self
                            .sources
                            .line_text(line)
                            .trim_end_matches(crate::utils::py_isspace)
                            .to_string();
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
                            next.source,
                            next.lineno,
                        ));
                    }
                    // fall through to paragraph (absorbs the underline line)
                }
            }
            if !next.is_blank() && next.indent() > 0 {
                // Single line + immediately indented block: definition list.
                self.note_cursor_beyond();
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
    fn parse_paragraph_like(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let mut end = *pos;
        while end < lines.len() && !lines[end].is_blank() && lines[end].indent() == 0 {
            end += 1;
        }
        // `Text.paragraph`: `data = '\n'.join(lines).rstrip()` — Python's
        // rstrip, BEFORE the `::` test. `string2lines` has already rstripped
        // the document's own lines, but synthesized ones (table cells) reach
        // here with their trailing whitespace intact.
        let joined = self.join_lines(&lines[start..end]);
        let joined = joined.trim_end_matches(crate::utils::py_isspace);
        let (text, expect_literal) = strip_literal_colons(joined);
        let span = self.span_of(lines, start, end.saturating_sub(1));
        // Multi-line paragraph directly followed by an indented line: the
        // error is created while the text block is gathered, BEFORE the
        // paragraph's inline parse (`Text.text`, `states.py:2922-2940`),
        // though the tree holds it after the paragraph and its messages.
        let unexpected_indentation = (end < lines.len()
            && !lines[end].is_blank()
            && lines[end].indent() > 0
            && end - start >= 2)
            .then(|| {
                self.msg_sm(
                    messages::ERROR,
                    "Unexpected indentation.",
                    lines[end].source,
                    lines[end].lineno,
                )
            });
        if !text.is_empty() {
            let result = self.inline(&text, span, lines[start].lineno);
            let mut para = Node::elem(kinds::PARAGRAPH, span);
            para.children = result.nodes;
            out.push(para);
            out.extend(result.messages);
        }
        *pos = end;

        if let Some(msg) = unexpected_indentation {
            out.push(msg);
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

    fn parse_literal_block(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let mut p = *pos;
        while p < lines.len() && lines[p].is_blank() {
            p += 1;
        }
        if p >= lines.len() {
            // Probe-verified: at EOF the warning still fires, anchored to
            // the line after the last one. With no line at all after the
            // `::` paragraph, docutils' cursor ends on the last line
            // ([`TopCursor::LastLine`]); after blank lines, one past them.
            if *pos >= lines.len() && self.depth == 0 {
                self.top_cursor = TopCursor::LastLine;
            }
            let anchor = lines
                .last()
                .map(|l| (l.source, l.lineno + 1))
                .unwrap_or((0, 1));
            out.push(self.msg(
                messages::WARNING,
                "Literal block expected; none found.",
                anchor.0,
                anchor.1,
            ));
            *pos = p;
            return;
        }
        let first = lines[p];
        if first.indent() > 0 {
            // Indented literal block.
            let (block, consumed, _indent, terminator) = indented_block(lines, p);
            let text = self.join_lines(&block);
            let span = self.span_of(lines, p, p + consumed - 1);
            let mut lb = Node::elem(kinds::LITERAL_BLOCK, span);
            lb.set("xml:space", AttrValue::Str("preserve".to_string()));
            lb.children.push(Node::text_node(text, span));
            out.push(lb);
            *pos = p + consumed;
            if let Some((term_source, term_lineno)) = terminator {
                out.push(self.msg_sm(
                    messages::WARNING,
                    "Literal block ends without a blank line; unexpected unindent.",
                    term_source,
                    term_lineno,
                ));
            }
            return;
        }
        let quote_char = self
            .sources
            .line_text(first)
            .chars()
            .next()
            .filter(|c| ADORNMENT_CHARS.contains(*c));
        if let Some(qc) = quote_char {
            // Quoted literal block: consistent same-char-prefixed run.
            let mut endq = p;
            while endq < lines.len()
                && !lines[endq].is_blank()
                && lines[endq].indent() == 0
                && self.sources.line_text(lines[endq]).starts_with(qc)
            {
                endq += 1;
            }
            // `quoted_literal_block` (`states.py:2961-2970`) parses on with
            // `nested_parse` and `goto_line`: run to the end of the input,
            // that leaves docutils' cursor past it; a blank line, an indent
            // or another quote character ends the nested parse first.
            if endq == lines.len() {
                self.note_cursor_beyond();
            }
            let text = self.join_lines(&lines[p..endq]);
            let span = self.span_of(lines, p, endq - 1);
            let mut lb = Node::elem(kinds::LITERAL_BLOCK, span);
            lb.set("xml:space", AttrValue::Str("preserve".to_string()));
            lb.children.push(Node::text_node(text, span));
            out.push(lb);
            if endq < lines.len() && !lines[endq].is_blank() {
                let text = if lines[endq].indent() > 0 {
                    "Unexpected indentation."
                } else {
                    "Inconsistent literal block quoting."
                };
                out.push(self.msg(
                    messages::ERROR,
                    text,
                    lines[endq].source,
                    lines[endq].lineno,
                ));
            }
            *pos = endq;
            return;
        }
        out.push(self.msg(
            messages::WARNING,
            "Literal block expected; none found.",
            first.source,
            first.lineno,
        ));
        *pos = p;
    }

    // ------------------------------------------------------------------
    // lists
    // ------------------------------------------------------------------

    fn parse_bullet_list(
        &mut self,
        lines: &[LineRec],
        pos: &mut usize,
        bullet: char,
        out: &mut Vec<Node>,
    ) {
        let start = *pos;
        let mut list = Node::elem(kinds::BULLET_LIST, Span::ZERO);
        list.set("bullet", AttrValue::Str(bullet.to_string()));
        let mut warn_line: Option<(u16, u32)> = None;
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
            if line.indent() == 0
                && Self::bullet_marker(self.sources.line_text(line)) == Some(bullet)
            {
                *pos = p;
                continue;
            }
            if !saw_blank {
                warn_line = Some((line.source, line.lineno));
            }
            *pos = p;
            break;
        }
        list.span = self.span_of(lines, start, pos.saturating_sub(1));
        out.push(list);
        if let Some((source, lineno)) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Bullet list ends without a blank line; unexpected unindent.",
                source,
                lineno,
            ));
        }
    }

    /// Parse one list item whose marker occupies `marker_chars` characters on
    /// the current line. Content indent per docutils: marker + following
    /// spaces, or the next line's indent when the marker stands alone.
    /// Leaves `*pos` just past the item's content (trailing blank lines are
    /// left for the caller).
    fn parse_list_item(&mut self, lines: &[LineRec], pos: &mut usize, marker_chars: usize) -> Node {
        let marker_line = lines[*pos];
        let marker_text = self.sources.line_text(marker_line);
        let after_off = rest_after_offset(marker_text, marker_chars);
        let after = &marker_text[after_off..];
        let spaces = after.len() - after.trim_start_matches(' ').len();
        let rest_off = after_off + spaces;
        let rest_is_empty = marker_text.len() == rest_off;
        let start = *pos;

        let mut body: Vec<LineRec> = Vec::new();
        let content_indent;
        if rest_is_empty {
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
            body.push(self.rewrap_from(marker_line, rest_off));
        }

        let mut last_content = start;
        let mut pending_blanks: Vec<LineRec> = Vec::new();
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
        lines: &[LineRec],
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
        let mut warn_line: Option<(u16, u32)> = None;
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
                if let Some(e) = parse_enumerator(self.sources.line_text(line)) {
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
                    warn_line = Some((line.source, line.lineno));
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
        let first_anchor = (lines[start].source, lines[start].lineno);
        out.push(list);
        if let Some((source, lineno)) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Enumerated list ends without a blank line; unexpected unindent.",
                source,
                lineno,
            ));
        }
        if chosen.initial != 1 {
            out.push(self.msg(
                messages::INFO,
                &format!(
                    "Enumerated list start value not ordinal-1: \"{}\" (ordinal {})",
                    first.literal, chosen.initial
                ),
                first_anchor.0,
                first_anchor.1,
            ));
        }
        true
    }

    /// docutils validates an enumerated item by its OWN next line: blank,
    /// EOF, indented continuation, or a valid successor enumerator.
    fn enum_item_valid(
        &self,
        lines: &[LineRec],
        at: usize,
        item: &Enumerator,
        candidates: &[EnumCandidate],
        auto_context: bool,
    ) -> bool {
        let next = match lines.get(at + 1) {
            None => return true,
            Some(n) => n,
        };
        // `if not next_line[:1].strip(): return True` — "blank or indented"
        // is Python's test on the FIRST character, so a line opening with an
        // NBSP or `\x1f` counts as indented (round F, `round_f` pins).
        let next_text = self.sources.line_text(*next);
        if next.is_blank()
            || next.indent() > 0
            || next_text
                .chars()
                .next()
                .is_some_and(crate::utils::py_isspace)
        {
            return true;
        }
        match parse_enumerator(next_text) {
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

    fn parse_definition_list(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let mut dl = Node::elem(kinds::DEFINITION_LIST, Span::ZERO);
        let mut warn_line: Option<(u16, u32)> = None;
        loop {
            let term_line = lines[*pos];
            let (block, consumed, _indent, terminator) = indented_block(lines, *pos + 1);
            let item_last = *pos + consumed;
            let mut item = Node::elem(
                kinds::DEFINITION_LIST_ITEM,
                self.span_of(lines, *pos, item_last),
            );
            let term_span = self.span_of(lines, *pos, *pos);
            let term_ends_in_colons = self.sources.line_text(term_line).ends_with("::");
            let mut parts = split_classifiers(self.sources.line_text(term_line)).into_iter();
            let has_classifiers = parts.len() > 1;
            let term_text = parts.next().unwrap_or_default();
            // `text = parts[0].rstrip()` (states.py:3015) — Python's
            // whitespace set, applied only on the classifier branch: a term
            // without one is a whole `string2lines` line, rstripped already.
            // Sphinx's glossary term is the opposite (kept verbatim; see
            // `run_glossary`). Probe-pinned, panel fix round F (`round_f`).
            let term_text = if has_classifiers {
                term_text
                    .trim_end_matches(crate::utils::py_isspace)
                    .to_string()
            } else {
                term_text
            };
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
            if term_ends_in_colons {
                // Probe-verified: docutils flags a term ending in `::`.
                definition.children.push(self.msg(
                    messages::INFO,
                    "Blank line missing before literal block (after the \"::\")? Interpreted as a definition list item.",
                    term_line.source,
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
                let text = self.sources.line_text(l);
                let nxt = lines.get(p + 1);
                l.indent() == 0
                    && !l.is_blank()
                    && Self::bullet_marker(text).is_none()
                    && parse_enumerator(text).is_none()
                    && adornment_char(text).is_none()
                    && field_marker(text).is_none()
                    && option_group_marker(text).is_none()
                    && !text.starts_with(".. ")
                    && text != ".."
                    && !text.starts_with("| ")
                    && !text.starts_with(">>> ")
                    && !text.starts_with("__ ")
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
        if let Some((source, lineno)) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Definition list ends without a blank line; unexpected unindent.",
                source,
                lineno,
            ));
        }
    }

    // ------------------------------------------------------------------
    // block quotes
    // ------------------------------------------------------------------

    fn parse_block_quote(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let (block, consumed, _indent, terminator) = indented_block(lines, *pos);
        *pos = start + consumed;
        let span = self.span_of(lines, start, start + consumed - 1);
        out.extend(self.block_quote_elements(&block, span));
        if let Some((source, lineno)) = terminator {
            out.push(self.msg_sm(
                messages::WARNING,
                "Block quote ends without a blank line; unexpected unindent.",
                source,
                lineno,
            ));
        }
    }

    /// docutils `Body.block_quote()`: build block_quote element(s) plus
    /// interleaved attribution messages from an already-extracted block.
    /// Shared by indented block quotes and the epigraph/highlights/
    /// pull-quote directives.
    fn block_quote_elements(&mut self, block: &[LineRec], span: Span) -> Vec<Node> {
        let mut out: Vec<Node> = Vec::new();
        // Split into blank-separated chunks; attribution chunks close quotes.
        let mut quotes: Vec<QuoteSegment> = Vec::new();
        let mut acc: Vec<LineRec> = Vec::new();
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
            match attribution_from_chunk(&self.sources, chunk, span) {
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

    fn parse_doctest(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        // Fixture-verified: a doctest block runs to the next BLANK line,
        // absorbing indented continuation/output lines verbatim.
        let start = *pos;
        let mut end = *pos;
        while end < lines.len() && !lines[end].is_blank() {
            end += 1;
        }
        let text = self.join_lines(&lines[start..end]);
        let span = self.span_of(lines, start, end - 1);
        let mut dt = Node::elem(kinds::DOCTEST_BLOCK, span);
        dt.set("xml:space", AttrValue::Str("preserve".to_string()));
        dt.children.push(Node::text_node(text, span));
        out.push(dt);
        *pos = end;
    }

    fn parse_line_block(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        // `line_block` (`states.py:1708-1725`) parses the lines after the
        // first with `nested_list_parse` and `goto_line` only when the
        // first line's block — the line and its indented continuation,
        // `get_first_known_indented(until_blank=True)` — is not
        // blank-finished: when a non-blank line directly follows it. That
        // nested parse stops at a blank line (`LineBlock.blank` is
        // `invalid_input`, `:2773`), so it reaches the end of the input
        // only when the block does (checked below).
        let mut first_end = start + 1;
        while first_end < lines.len()
            && !lines[first_end].is_blank()
            && lines[first_end].indent() > 0
        {
            first_end += 1;
        }
        let nested = first_end < lines.len() && !lines[first_end].is_blank();
        // (depth, text): depth None on bare `|` lines inherits the previous
        // line's depth (fixture-verified). Continuations dedent by the FIRST
        // continuation line's indent, preserving deeper relative indents.
        //
        // Each item keeps its `|` line's number: `line_block_line` inline-
        // parses every line at its own `lineno` (`states.py:1736-1748`).
        let mut items: Vec<(Option<usize>, String, u32)> = Vec::new();
        let mut cont_dedent: Option<usize> = None;
        let mut p = *pos;
        while p < lines.len() && !lines[p].is_blank() {
            let l = lines[p];
            let text = self.sources.line_text(l);
            if l.indent() == 0 && (text == "|" || text.starts_with("| ")) {
                cont_dedent = None;
                if text == "|" {
                    items.push((None, String::new(), l.lineno));
                } else {
                    let content = &text[2..];
                    let depth = content.len() - content.trim_start_matches(' ').len();
                    items.push((Some(depth), content[depth..].to_string(), l.lineno));
                }
                p += 1;
            } else if l.indent() > 0 && !items.is_empty() {
                let dedent = *cont_dedent.get_or_insert(l.indent());
                let dedent = dedent.min(l.indent());
                if let Some(last) = items.last_mut() {
                    if !last.1.is_empty() {
                        last.1.push('\n');
                    }
                    last.1.push_str(&text[dedent..]);
                }
                p += 1;
            } else {
                break;
            }
        }
        if nested && p == lines.len() {
            self.note_cursor_beyond();
        }
        // Resolve inherited depths and inline-parse each line's text.
        let span = self.span_of(lines, start, p - 1);
        let mut resolved: Vec<(usize, Vec<Node>)> = Vec::with_capacity(items.len());
        let mut lb_messages: Vec<Node> = Vec::new();
        let mut prev_depth = 0usize;
        for (depth, text, lineno) in items {
            let d = depth.unwrap_or(prev_depth);
            prev_depth = d;
            if text.is_empty() {
                resolved.push((d, Vec::new()));
            } else {
                let inline = self.inline(&text, span, lineno);
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
                lines[p - 1].source,
                lines[p - 1].lineno,
            ));
        }
        *pos = p;
    }

    // ------------------------------------------------------------------
    // explicit markup: comments + targets
    // ------------------------------------------------------------------

    fn parse_explicit(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let line = lines[*pos];
        // Local handle: `rest` must stay readable across the `&mut self`
        // construct dispatches below.
        let src = self.sources.arc(line.source);
        let line_text = line.slice(&src);
        // docutils consumes ALL whitespace after `..` (fixture-verified for
        // multi-space forms).
        // The explicit-markup transition is `\.\.( +|$)` (states.py,
        // `Body.patterns.explicit_markup`) and every construct pattern opens
        // `\.\.[ ]+` — LITERAL spaces, not Python's `\s`. `.. \xa0_x:` is
        // therefore a plain comment, not a target (fixture
        // round_e.explicit_nbsp_after_dots_is_comment).
        let rest = if line_text == ".." {
            ""
        } else {
            line_text[2..].trim_start_matches(' ')
        };
        // `match.end()` of that transition, in CHARACTERS (`..` plus the
        // literal spaces are ASCII): `Body.comment` slices the comment's
        // first line at this offset, and on the malformed-target path that
        // line is NOT the marker line.
        let dots_end = line_text.len() - rest.len();

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
        // Set only by the malformed-hyperlink-target path: the comment's
        // first line, which is then NOT the explicit-markup marker line.
        let mut comment_first: Option<String> = None;
        // The hyperlink-target construct is `\.\.[ ]+_(?![ ]|$)`
        // (states.py:2464-2469): the character after `_` on the FIRST line
        // must exist and must not be a space, or the whole block is a plain
        // comment — never a malformed target. `.. _ x:` and a bare `.. _`
        // (even with an indented continuation carrying `name: uri`) are
        // comments, and so is `.. _\tx:` once `expandtabs` has run
        // (fixture round_d.target_*_is_comment, docutils 0.22.4).
        if rest.starts_with('_') && !matches!(rest[1..].chars().next(), None | Some(' ')) {
            // `Body.hyperlink_target` (states.py:2055-2078): gather the block
            // with `get_first_known_indented(match.end(), until_blank=True,
            // strip_indent=False)` — block[0] is the first line PAST the `_`
            // and every continuation line keeps its FULL indentation — then
            // concatenate one line at a time (NO separator) until the target
            // pattern matches, or raise `malformed hyperlink target.`.
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
            let mut block: Vec<Vec<char>> = Vec::with_capacity(consumed + 1);
            block.push(escape2null_chars(&rest[1..]));
            for l in &lines[start + 1..start + 1 + consumed] {
                block.push(escape2null_chars(self.sources.line_text(*l)));
            }
            let mut escaped: Vec<char> = block[0].clone();
            let mut blockindex = 0usize;
            let hit = loop {
                if let Some(m) = match_target_pattern(&escaped) {
                    break Some(m);
                }
                blockindex += 1;
                match block.get(blockindex) {
                    Some(next) => escaped.extend_from_slice(next),
                    None => break None,
                }
            };
            match hit {
                Some((name, end)) => {
                    *pos = start + 1 + consumed;
                    let span = self.span_of(lines, start, start + consumed);
                    // `block[0] = (block[0] + ' ')[targetmatch.end()
                    // - len(escaped) - 1:].strip()` — a NEGATIVE slice, so it
                    // keeps the unmatched tail of the line the match ended on.
                    let keep = escaped.len() - end + 1;
                    let mut with_space = block[blockindex].clone();
                    with_space.push(' ');
                    let tail: String = with_space[with_space.len().saturating_sub(keep)..]
                        .iter()
                        .collect();
                    let mut link_block: Vec<Vec<char>> = vec![py_strip(&tail).chars().collect()];
                    for l in &block[blockindex + 1..] {
                        link_block.push(l.clone());
                    }
                    let mut target = Node::elem(kinds::TARGET, span);
                    // `nodes.target(block_text, ...)` (`make_target`,
                    // `states.py:2080-2090`): `match.string[:match.end()]`
                    // plus the block — the target's lines as they stand.
                    let blocktext = lines[start..=start + consumed]
                        .iter()
                        .map(|l| self.sources.line_text(*l))
                        .collect::<Vec<_>>()
                        .join("\n");
                    target.set(crate::doctree::RAWSOURCE, AttrValue::Str(blocktext));
                    let mut internal = false;
                    let mut refuri_val: Option<String> = None;
                    let anonymous = name.is_none();
                    match &name {
                        // `add_target`: `normalize_name(unescape(targetname))`.
                        Some(n) => target
                            .attrs
                            .names
                            .push(ids::fully_normalize_name(&unescape_nulls(n))),
                        None => target.set("anonymous", AttrValue::Int(1)),
                    }
                    match parse_target_block(&link_block) {
                        TargetRef::RefName(data) => {
                            target.set("refname", AttrValue::Str(ids::fully_normalize_name(&data)));
                        }
                        TargetRef::RefUri(uri) if uri.is_empty() => internal = true,
                        TargetRef::RefUri(uri) => {
                            refuri_val = Some(uri.clone());
                            target.set("refuri", AttrValue::Str(uri));
                        }
                    }
                    let msg = if anonymous {
                        self.registry.set_id_anonymous(&mut target);
                        None
                    } else {
                        let source_path = self.sources.arc_path(line.source);
                        self.registry
                            .set_id_explicit(
                                &mut target,
                                lineno,
                                line.source,
                                &source_path,
                                internal,
                                refuri_val.as_deref(),
                            )
                            .map(|msg| self.created(msg))
                    };
                    if let Some(m) = msg {
                        out.push(m);
                    }
                    out.push(target);
                }
                None => {
                    // Malformed. `explicit_construct` queues the WARNING at
                    // `abs_line_number()` — which `hyperlink_target` has
                    // already advanced to the LAST line of the block — and
                    // falls through to `Body.comment(match)`, so the comment
                    // starts on that same line, sliced at the TRANSITION
                    // match's end (`dots_end`), not at the marker line.
                    let last = start + consumed;
                    let last_line = lines[last];
                    comment_first =
                        Some(char_suffix(self.sources.line_text(last_line), dots_end).to_string());
                    *pos = last;
                    construct_error = Some(self.msg(
                        messages::WARNING,
                        "malformed hyperlink target.",
                        last_line.source,
                        last_line.lineno,
                    ));
                }
            }
            if construct_error.is_none() {
                // `until_blank=True` (`states.py:2058-2059`): a trailing
                // blank line is left to the top level.
                self.note_blank_left_to_top(lines, *pos);
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

        // Include marker (docutils Body.comment, states.py:2425-2433): a
        // comment line opening with `end of inclusion from "` whose next
        // line is blank pops the include log and emits NO node — that pop
        // is what makes two sequential includes of the same file legal.
        // The empty-log guard is a totality divergence: docutils pops
        // unguarded and dies with IndexError on a hand-written marker
        // (probe-verified); here such a line stays an ordinary comment.
        if construct_error.is_none()
            && rest.starts_with(INCLUDE_MARKER_PREFIX)
            && lines.get(*pos + 1).map(|l| l.is_blank()).unwrap_or(true)
            && !self.include_log.is_empty()
        {
            self.include_log.pop();
            *pos += 1;
            // Reads nothing past its own line (`states.py:2431-2433`).
            self.note_blank_left_to_top(lines, *pos);
            return;
        }

        // Comment. Probe-verified continuation rules: a comment with first-
        // line text absorbs the following indented block THROUGH internal
        // blank lines; a bare `..` takes a body only when the indented block
        // is ADJACENT (`..` + blank + indent leaves an empty comment and a
        // block quote).
        let start = *pos;
        // On the malformed-target path the comment opens on the block's LAST
        // line, whose text past `dots_end` replaces the marker remainder.
        let rest: &str = match comment_first {
            Some(ref s) => s.as_str(),
            None => rest,
        };
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
        if !consume_block {
            // The empty comment reads nothing (`states.py:2427-2430`); any
            // other reads its block, trailing blank lines included.
            self.note_blank_left_to_top(lines, *pos);
        }
        let span = self.span_of(lines, start, start + consumed);
        let mut text_lines: Vec<String> = Vec::new();
        if !rest.is_empty() {
            text_lines.push(rest.to_string());
        }
        let mut body: &[LineRec] = &block;
        if rest.is_empty() {
            while body.first().map(|l| l.is_blank()).unwrap_or(false) {
                body = &body[1..];
            }
        }
        for l in body {
            text_lines.push(self.sources.line_text(*l).to_string());
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
        lines: &[LineRec],
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
        let mut body: Vec<LineRec> = Vec::new();
        if !first_rest.trim().is_empty() {
            // remainder starts at a virtual column; treat as its own line
            let marker_text = self.sources.line_text(lines[start]);
            let off = rest_after_offset(
                marker_text,
                marker_text.chars().count() - first_rest.chars().count(),
            );
            body.push(self.rewrap_from(lines[start], off));
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
            let source = lines[start].source;
            let source_path = self.sources.arc_path(source);
            self.registry
                .set_id_explicit(&mut node, lineno, source, &source_path, true, None)
                .map(|msg| self.created(msg))
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
                .push(self.msg(messages::WARNING, text, lines[start].source, lineno));
        } else {
            node.children.extend(content);
        }
        out.push(node);
        Some(start + 1 + consumed)
    }

    /// Field lists: `:name: value` markers (probe-verified regex port).
    fn parse_field_list(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let mut fl = Node::elem(kinds::FIELD_LIST, Span::ZERO);
        let mut warn_line: Option<(u16, u32)> = None;
        loop {
            let line = lines[*pos];
            let line_text = self.sources.line_text(line);
            let (name_raw, body_start) = field_marker(line_text).expect("checked by caller");
            let lineno = line.lineno;
            let field_span = self.span_of(lines, *pos, *pos);
            // body: marker-line remainder + any-indent continuation block.
            // The remainder is `line[match.end():]` after `field_marker`'s
            // `( +|$)` (states.py:2960-2965): ASCII spaces only, nothing
            // lstripped, so an NBSP after the gap is body text (round F,
            // pinned in `round_f`; `trim_start()` had eaten it).
            let first_rest = line_text[body_start..].trim_start_matches(' ');
            let rest_offset = (!first_rest.is_empty()).then(|| line_text.len() - first_rest.len());
            let (block, consumed, _i, terminator) = indented_block(lines, *pos + 1);
            let mut body_lines: Vec<LineRec> = Vec::new();
            if let Some(offset) = rest_offset {
                body_lines.push(self.rewrap_from(line, offset));
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
            // docutils keeps the body's text as `field_body.rawsource`
            // (`'\n'.join(indented)`, `states.py:1592`), which DocInfo parses
            // again when a bibliographic field's one line came out as an
            // enumerated list — an author's initial, `:author: J. Doe`
            // (`frontmatter.py:446-456`; `crate::transforms::frontmatter`).
            // Kept for that shape only: a body led by an enumerated list,
            // of a single line.
            let mut content_lines = body_lines.iter().filter(|line| !line.is_blank());
            if let (Some(only), None) = (content_lines.next(), content_lines.next()) {
                if fbody
                    .children
                    .first()
                    .is_some_and(|first| first.kind == kinds::ENUMERATED_LIST)
                {
                    let rawsource = self.sources.line_text(*only).to_string();
                    fbody.set(crate::doctree::RAWSOURCE, AttrValue::Str(rawsource));
                }
            }
            field.children.push(fbody);
            fl.children.push(field);

            // continue on the next field marker (blanks allowed between)
            let mut p = *pos;
            while p < lines.len() && lines[p].is_blank() {
                p += 1;
            }
            let continues = p < lines.len()
                && lines[p].indent() == 0
                && field_marker(self.sources.line_text(lines[p])).is_some();
            if continues {
                *pos = p;
                continue;
            }
            let _ = terminator;
            // Adjacency: any non-blank line directly after the field body
            // (indented-block terminator OR a col-0 line) warns.
            if let Some(l) = lines.get(*pos) {
                if !l.is_blank() {
                    warn_line = Some((l.source, l.lineno));
                }
            }
            break;
        }
        fl.span = self.span_of(lines, start, pos.saturating_sub(1));
        out.push(fl);
        if let Some((source, lineno)) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Field list ends without a blank line; unexpected unindent.",
                source,
                lineno,
            ));
        }
    }

    /// An option marker line is only a list item when it has a two-space
    /// description or an indented following line (else: paragraph).
    fn option_item_viable(&self, lines: &[LineRec], at: usize) -> bool {
        let (_, desc) = match option_group_marker(self.sources.line_text(lines[at])) {
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

    fn parse_option_list(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let mut ol = Node::elem(kinds::OPTION_LIST, Span::ZERO);
        let mut warn_line: Option<(u16, u32)> = None;
        loop {
            let line = lines[*pos];
            let line_text = self.sources.line_text(line);
            let (specs, desc) = option_group_marker(line_text).expect("checked by caller");
            let desc_offset = (!desc.is_empty()).then(|| line_text.len() - desc.len());
            let span = self.span_of(lines, *pos, *pos);
            let (block, consumed, _i, terminator) = indented_block(lines, *pos + 1);
            let mut body_lines: Vec<LineRec> = Vec::new();
            if let Some(offset) = desc_offset {
                body_lines.push(self.rewrap_from(line, offset));
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
                && option_group_marker(self.sources.line_text(lines[p])).is_some()
                && self.option_item_viable(lines, p);
            if continues {
                *pos = p;
                continue;
            }
            let _ = terminator;
            if let Some(l) = lines.get(*pos) {
                if !l.is_blank() {
                    warn_line = Some((l.source, l.lineno));
                }
            }
            break;
        }
        ol.span = self.span_of(lines, start, pos.saturating_sub(1));
        out.push(ol);
        if let Some((source, lineno)) = warn_line {
            out.push(self.msg_sm(
                messages::WARNING,
                "Option list ends without a blank line; unexpected unindent.",
                source,
                lineno,
            ));
        }
    }

    // ------------------------------------------------------------------
    // grid tables (docutils tableparser.GridTableParser port)
    // ------------------------------------------------------------------

    fn parse_grid_table(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        // isolate: consume until blank line
        let mut end = *pos;
        while end < lines.len() && !lines[end].is_blank() {
            end += 1;
        }
        let mut block: Vec<LineRec> = lines[start..end].to_vec();
        *pos = end;
        // docutils left-edge check: trim at the first line not starting
        // with '+' or '|'; the remainder re-parses and a blank-line
        // warning fires. The trim index feeds the stale-line quirk of the
        // bottom-corrupt error.
        //
        // The warning is only CREATED once the table (and every message it
        // raises, its cells' included) exists: `table_top` builds the
        // table, then warns `if not blank_finish` (`states.py:1787-1797`).
        // Until then this holds its anchor.
        let mut trailing_warning: Option<(u16, u32)> = None;
        let mut stale_i = block.len() - 1;
        let mut edge_trim: Option<(usize, u16, u32)> = None;
        for (i, l) in block.iter().enumerate().skip(1) {
            let t = self.sources.line_text(*l).trim_end();
            if !(t.starts_with('+') || t.starts_with('|')) {
                stale_i = i;
                edge_trim = Some((i, l.source, l.lineno));
                break;
            }
        }
        if let Some((i, source, lineno)) = edge_trim {
            trailing_warning = Some((source, lineno));
            block.truncate(i);
            *pos = start + i;
        }
        // docutils trims a non-border tail back to the LAST valid border
        // (the remainder re-parses, with a blank-line-required warning),
        // BEFORE any alignment checks.
        if !is_grid_table_top(self.sources.line_text(block[block.len() - 1]).trim_end()) {
            let mut found = None;
            for i in (2..block.len() - 1).rev() {
                if is_grid_table_top(self.sources.line_text(block[i]).trim_end()) {
                    found = Some(i);
                    break;
                }
            }
            if let Some(i) = found {
                let next = block[i + 1];
                block.truncate(i + 1);
                *pos = start + i + 1;
                if trailing_warning.is_none() {
                    trailing_warning = Some((next.source, next.lineno));
                }
            }
        }
        let blank_line_required = |me: &Self| {
            trailing_warning.map(|(source, lineno)| {
                me.msg(
                    messages::WARNING,
                    "Blank line required after table.",
                    source,
                    lineno,
                )
            })
        };
        let raw_block: Vec<String> = block
            .iter()
            .map(|l| self.sources.line_text(*l).to_string())
            .collect();
        let table_source = block[0].source;
        // `malformed_table` passes the literal at creation
        // (`states.py:1900-1909`).
        let malformed = |me: &Self, detail: &str, lineno: u32| -> Node {
            me.msg_literal(
                messages::ERROR,
                &format!("Malformed table.\n{detail}"),
                table_source,
                lineno,
                raw_block.join("\n").trim_end(),
            )
        };
        // right-border alignment (DISPLAY columns: east-asian wide = 2)
        let width = column_width(raw_block[0].trim_end());
        for (l, raw) in block.iter().zip(&raw_block).skip(1) {
            let t = raw.trim_end();
            if column_width(t) != width || !(t.ends_with('+') || t.ends_with('|')) {
                out.push(malformed(
                    self,
                    "Right border not aligned or missing.",
                    l.lineno,
                ));
                out.extend(blank_line_required(self));
                return;
            }
        }
        // bottom border must be a grid border (line anchor reproduces
        // docutils' stale-index quirk: the last line the edge scans reached)
        if !is_grid_table_top(raw_block[raw_block.len() - 1].trim_end()) {
            let lineno = lines[(start + stale_i).min(lines.len() - 1)].lineno;
            out.push(malformed(self, "Bottom border missing or corrupt.", lineno));
            out.extend(blank_line_required(self));
            return;
        }

        // grid as DISPLAY-column matrix (wide chars followed by a filler;
        // head/body sep '=' converted to '-')
        let mut grid: Vec<Vec<char>> = raw_block
            .iter()
            .map(|l| {
                let mut row = Vec::new();
                for c in l.trim_end().chars() {
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
                        self,
                        &format!(
                            "Multiple head/body row separators (table lines {} and {}); only one allowed.",
                            first + 1,
                            i + 1
                        ),
                        // `TableMarkupError(..., offset=i)` located at
                        // `startline + offset` (`states.py:1900-1909`).
                        block[i].lineno,
                    ));
                    out.extend(blank_line_required(self));
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
                self,
                "Malformed table; parse incomplete.",
                block[0].lineno,
            ));
            out.extend(blank_line_required(self));
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
            let mut cell_lines: Vec<LineRec> = Vec::new();
            for l in block.iter().take(bottom).skip(top + 1) {
                let (s, e) = display_range(self.sources.line_text(*l), left + 1, right);
                cell_lines.push(self.rewrap_range(*l, s, e));
            }
            let base = cell_lines
                .iter()
                .filter(|l| !self.sources.line_text(**l).trim().is_empty())
                .map(|l| l.indent())
                .min()
                .unwrap_or(0);
            let dedented: Vec<LineRec> = cell_lines
                .iter()
                .map(|l| {
                    if self.sources.line_text(*l).trim().is_empty() {
                        self.blank_at(*l)
                    } else {
                        let d = l.dedented(base);
                        // `get_2D_block` rstrips the cell slice — Python's
                        // set (round F; every cell consumer rstrips again,
                        // so no input reaches the old `trim_end()`).
                        let trimmed = self
                            .sources
                            .line_text(d)
                            .trim_end_matches(crate::utils::py_isspace)
                            .len();
                        self.rewrap_range(d, 0, trimmed)
                    }
                })
                .collect();
            if dedented.iter().any(|l| !l.is_blank()) {
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
        out.extend(blank_line_required(self));
    }

    fn parse_simple_table(&mut self, lines: &[LineRec], pos: &mut usize, out: &mut Vec<Node>) {
        let start = *pos;
        let toplen = char_len(self.sources.line_text(lines[start]).trim_end());
        // isolate: find border candidates (=-runs line, same stripped length)
        let mut found = 0usize;
        let mut found_at = None;
        let mut end = None;
        let mut i = start + 1;
        while i < lines.len() {
            let t = self.sources.line_text(lines[i]).trim_end();
            if is_simple_table_border(t) {
                if char_len(t) != toplen {
                    let raw = self.join_lines(&lines[start..=i]);
                    out.push(self.msg_literal(
                        messages::ERROR,
                        "Malformed table.\nBottom border or header rule does not match top border.",
                        lines[i].source,
                        lines[i].lineno,
                        raw.trim_end(),
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
            let raw = self.join_lines(&lines[start..=block_end.min(lines.len() - 1)]);
            out.push(self.msg_literal(
                messages::ERROR,
                &format!("Malformed table.\nNo bottom table border found{extra}."),
                lines[start].source,
                lines[start].lineno,
                raw.trim_end(),
            ));
            *pos = block_end + 1;
            if !extra.is_empty() {
                if let Some(l) = lines.get(*pos).filter(|l| !l.is_blank()) {
                    out.push(self.msg(
                        messages::WARNING,
                        "Blank line required after table.",
                        l.source,
                        l.lineno,
                    ));
                }
            }
            return;
        };
        *pos = end + 1;
        let blank_after_ok = lines.get(*pos).map(|l| l.is_blank()).unwrap_or(true);

        let block: Vec<LineRec> = lines[start..=end].to_vec();
        let raw_block: Vec<String> = block
            .iter()
            .map(|l| self.sources.line_text(*l).to_string())
            .collect();
        let table_source = block[0].source;
        // `malformed_table` passes the literal at creation
        // (`states.py:1900-1909`).
        let malformed = |me: &Self, detail: &str, lineno: u32| -> Node {
            me.msg_literal(
                messages::ERROR,
                &format!("Malformed table.\n{detail}"),
                table_source,
                lineno,
                raw_block.join("\n").trim_end(),
            )
        };

        // columns from the top border '=' runs
        let top_chars: Vec<char> = raw_block[0].trim_end().chars().collect();
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
        let mut work: Vec<String> = raw_block.iter().map(|l| l.trim_end().to_string()).collect();
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
                    // `offset=offset`: the span line itself.
                    return Err(Box::new(malformed(
                        self,
                        &format!("Column span incomplete in table line {}.", table_line + 1),
                        block[table_line].lineno,
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
            // `line[:firstend].strip()` (tableparser.py) — Python's set: an
            // all-`\x1f` first column is a continuation (round F pin).
            if !fc_text.trim_matches(crate::utils::py_isspace).is_empty() {
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
                    // `check_columns`: `line[end:nextstart].strip()` —
                    // Python's set (round F pin: a `\x1f` in the margin).
                    if !display_slice(line, e1, s2)
                        .trim_matches(crate::utils::py_isspace)
                        .is_empty()
                    {
                        out.push(malformed(
                            self,
                            &format!("Text in column margin in table line {}.", bi + 1),
                            block[bi].lineno,
                        ));
                        return;
                    }
                }
                let row_border_end = row.cols.last().map(|(_, e)| *e).unwrap_or(border_end);
                let tail = display_slice(line, row_border_end, column_width(line));
                if !tail.trim_matches(crate::utils::py_isspace).is_empty() {
                    let last_start = row.cols.last().map(|(s, _)| *s).unwrap_or(0);
                    let extent = last_start
                        + column_width(
                            display_slice(line, last_start, column_width(line)).trim_end(),
                        );
                    last_col_end = last_col_end.max(extent);
                }
            }
        }

        // map span cols -> column indices for morecols; validate alignment.
        // Every row is validated before ANY cell is parsed: the table parser
        // raises its `TableMarkupError`s from `parser.parse(block)`, before
        // `build_table` nested-parses a single cell (`states.py:1799-1815`),
        // so a malformed table never creates its cells' messages.
        let col_starts: Vec<usize> = columns.iter().map(|(s, _)| *s).collect();
        let col_ends: Vec<usize> = columns.iter().map(|(_, e)| *e).collect();
        // Per row, per cell: (cs, ce_eff, morecols).
        let mut geometry: Vec<Vec<(usize, usize, usize)>> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut cells = Vec::with_capacity(row.cols.len());
            for (ci, (cs, ce)) in row.cols.iter().enumerate() {
                let ce_eff = if ci == row.cols.len() - 1 {
                    last_col_end.max(*ce)
                } else {
                    *ce
                };
                let span_end_col = if ci == row.cols.len() - 1 {
                    Some(columns.len() - 1)
                } else {
                    col_ends.iter().position(|e| e == ce)
                };
                let (Some(ci_start), Some(span_end_col)) =
                    (col_starts.iter().position(|s| s == cs), span_end_col)
                else {
                    // `init_row` raises with `offset=offset+1` — the line
                    // its message numbers (`tableparser.py`).
                    out.push(malformed(
                        self,
                        &format!(
                            "Column span alignment problem in table line {}.",
                            row.start + 2
                        ),
                        block[(row.start + 1).min(block.len() - 1)].lineno,
                    ));
                    return;
                };
                cells.push((*cs, ce_eff, span_end_col - ci_start));
            }
            geometry.push(cells);
        }
        let mut built_rows: Vec<(usize, Node)> = Vec::new(); // (start_line, row)
        for (row, cells) in rows.iter().zip(geometry) {
            let mut r = Node::elem(kinds::ROW, self.span_of(lines, start, end));
            for (cs, ce_eff, morecols) in cells {
                let mut entry = Node::elem(kinds::ENTRY, self.span_of(lines, start, end));
                if morecols > 0 {
                    entry.set("morecols", AttrValue::Int(morecols as i64));
                }
                // cell block
                let mut cell_lines: Vec<LineRec> = Vec::new();
                #[allow(clippy::needless_range_loop)]
                for bi in row.start..row.end.min(bottom) {
                    let l = block[bi];
                    let text = self.sources.line_text(l);
                    let (s, e) = display_range(text, cs, ce_eff.min(column_width(text)));
                    // `get_2D_block` rstrips the cell slice — Python's set
                    // (round F; see the grid-table cell view).
                    let e = s + text[s..e].trim_end_matches(crate::utils::py_isspace).len();
                    cell_lines.push(self.rewrap_range(l, s, e));
                }
                let base = cell_lines
                    .iter()
                    .filter(|l| !self.sources.line_text(**l).trim().is_empty())
                    .map(|l| l.indent())
                    .min()
                    .unwrap_or(0);
                let dedented: Vec<LineRec> = cell_lines
                    .iter()
                    .map(|l| {
                        if self.sources.line_text(*l).trim().is_empty() {
                            self.blank_at(*l)
                        } else {
                            l.dedented(base)
                        }
                    })
                    .collect();
                if dedented.iter().any(|l| !l.is_blank()) {
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
                    l.source,
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
        lines: &[LineRec],
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
        let mut rawsource = self.sources.line_text(lines[start]).to_string();
        for l in &lines[start + 1..raw_end] {
            rawsource.push('\n');
            rawsource.push_str(self.sources.line_text(*l));
        }

        let first_line = {
            let t = first_rest.trim_start_matches(' ');
            let offset = self.sources.line_text(lines[start]).len() - t.len();
            self.rewrap_from(lines[start], offset)
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
        first_line: LineRec,
        block: &[LineRec],
        rawsource: &str,
        lineno: u32,
        span: Span,
        presets: Vec<(String, OptVal)>,
        out: &mut Vec<Node>,
    ) {
        self.capture_directive_record(name, &first_line, block, lineno);
        let lower = name.to_lowercase();
        let Some(spec) = directive_spec_mode(&lower, self.sphinx) else {
            // Unknown: INFO (language-resolution narrative) + ERROR. The
            // INFO sits at `document.current_line` (see
            // `detached_first_lineno`).
            let info_line = match self.detached_first_lineno {
                Some(first) => lineno.saturating_sub(first) + 1,
                None => lineno,
            };
            out.push(self.msg(
                messages::INFO,
                &format!(
                    "No directive entry for \"{name}\" in module \"docutils.parsers.rst.languages.en\".\nTrying \"{name}\" as canonical directive name."
                ),
                span.source,
                info_line,
            ));
            out.push(self.msg_literal(
                messages::ERROR,
                &format!("Unknown directive type \"{name}\"."),
                span.source,
                lineno,
                rawsource,
            ));
            return;
        };

        // MarkupError wrapper (states.py:2274-2281): uses the directive
        // name AS WRITTEN (`.. NOTE::` errors say "NOTE").
        let dir_error = |me: &Self, detail: &str| -> Node {
            me.msg_literal(
                messages::ERROR,
                &format!("Error in \"{name}\" directive:\n{detail}."),
                span.source,
                lineno,
                rawsource,
            )
        };

        // ---- parse_directive_block (states.py:2301-2345), exact order ----
        // `indented` mirrors get_first_known_indented(match.end(),
        // strip_top=0): the marker-line remainder after `::` and ALL
        // following spaces, then the (already dedented) indented block.
        let mut indented: Vec<LineRec> = vec![first_line];
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
        let mut arg_block: Vec<LineRec>;
        let mut content: Vec<LineRec>;
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
                .position(|l| field_marker(self.sources.line_text(*l)).is_some())
            {
                let opt_block = arg_block.split_off(k);
                match parse_extension_options(&self.sources, &opt_block, spec.option_spec) {
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
        // `content_offset` (states.py:2320-2337): the content's own first
        // line when there is content (its offset is advanced past every
        // blank the trim above drops); otherwise one past the argument/
        // option block (`line_offset + i + 1` with the `for`/`else` making
        // `i` the block's length), or the line after the marker when
        // nothing at all follows it.
        let content_lineno = match (content.first(), indented.first()) {
            (Some(first), _) => first.lineno,
            (None, Some(first)) if declares_specs => first.lineno + blank_idx as u32 + 1,
            (None, Some(first)) => first.lineno,
            (None, None) => lineno + 1,
        };

        // Arguments (parse_directive_arguments, states.py:2365-2380).
        let mut arguments: Vec<String> = Vec::new();
        if spec.required_arguments + spec.optional_arguments > 0 {
            let arg_text = self.join_lines(&arg_block);
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
            content_lineno,
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
            DirectiveKind::ObjectDesc(kind) => {
                self.run_object_description(DescDispatch::Std(kind), input, out)
            }
            DirectiveKind::PyObjectDesc(py) => {
                self.run_object_description(DescDispatch::Py(py), input, out)
            }
            DirectiveKind::PyModule => self.run_py_module(input, out),
            DirectiveKind::PyCurrentModule => self.run_py_currentmodule(input),
            DirectiveKind::Include => {
                let outcome = self.run_include(input, out);
                self.finish_directive(outcome);
            }
            DirectiveKind::LiteralInclude => self.run_literalinclude(input, out),
            DirectiveKind::ProgramDir => self.run_program(input),
            // `DefaultDomain.run` sets `env.current_document.default_domain`
            // and returns []. This crate implements no domain whose
            // directives/roles the default would route to (the std domain is
            // always consulted last anyway), so the state has nothing to
            // steer — the node-level effect, an empty return, is all of it.
            DirectiveKind::DefaultDomainDir => {}
            #[cfg(test)]
            DirectiveKind::TestSplice => {
                let outcome = self.run_test_splice(input);
                self.finish_directive(outcome);
            }
        }
    }

    /// Bank a directive's [`DirectiveOutcome`] for the enclosing
    /// block-parse loop: a splice waits in `pending_splice` until the loop
    /// reaches its cursor. Splice-producing directive arms (`include`)
    /// route their return value through here.
    fn finish_directive(&mut self, outcome: DirectiveOutcome) {
        if let DirectiveOutcome::Splice(request) = outcome {
            self.pending_splice = Some(request);
        }
    }

    /// The test-only splice producer: content lines become a spliced
    /// source named by the argument. Built from the input alone — the
    /// shape T12's include takes: no parser internals needed to return a
    /// splice.
    #[cfg(test)]
    fn run_test_splice(&mut self, input: DirectiveInput<'_>) -> DirectiveOutcome {
        let lines = input
            .content
            .iter()
            .map(|l| self.sources.line_text(*l).to_string())
            .collect();
        DirectiveOutcome::Splice(SpliceRequest::single(lines, input.arguments[0].clone()))
    }

    // ------------------------------------------------------------------
    // include (docutils misc.py Include + the sphinx other.py override)
    // ------------------------------------------------------------------

    /// The `include` directive (`DU/parsers/rst/directives/misc.py:42-267`;
    /// sphinx-mode path rewrite per `SP/directives/other.py:371-416`).
    /// Insert mode returns a splice for the enclosing parse loop; the
    /// literal/code/parser modes and every error path push nodes and
    /// return [`DirectiveOutcome::Done`].
    fn run_include(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) -> DirectiveOutcome {
        // `settings.file_insertion_enabled` is not modeled (always true —
        // this crate has no docutils settings surface).
        let tab_width = opt_i64(&input.options, "tab-width").unwrap_or(8);
        // An out-of-C-int `tab_width` is checked where docutils actually
        // calls `expandtabs` — never before the read, so a missing file, a
        // decode failure or a bad clip reports ITS error alone, as docutils
        // does (see [`c_int_tabsize`] and [`Self::tab_width_overflow`]).
        // The circular-inclusion identity 4-tuple (`misc.py:85-88`).
        let clip: IncludeClip = (
            opt_i64(&input.options, "start-line"),
            opt_i64(&input.options, "end-line"),
            match opt_get(&input.options, "start-after") {
                Some(OptVal::Str(s)) => s.clone(),
                _ => String::new(),
            },
            match opt_get(&input.options, "end-before") {
                Some(OptVal::Str(s)) => s.clone(),
                _ => String::new(),
            },
        );
        // `directives.path` (`__init__.py:196-206`): join a multi-line
        // argument, stripping each line.
        let path_arg: String = input
            .arguments
            .first()
            .map(|a| a.lines().map(str::trim).collect())
            .unwrap_or_default();
        let target = self.resolve_include_target(&path_arg, input.span.source);
        // `env.note_included` runs BEFORE the file is opened
        // (`other.py:415` precedes `super().run()`), so even a missing
        // file records — but only when the path maps to a docname, and
        // never for a standard include (`other.py:410-412` bypasses the
        // rewrite entirely). It maps the RESOLVED path (`relfn2path`'s
        // `.resolve()`d `abs_fn`, `environment/__init__.py:475`) against
        // the resolved srcdir: through a symlink that can be a different
        // docname than the lexical spelling (`link/../c.rst` with `link ->
        // a/b` is `a/c`, not `c`), or no docname at all when the file lies
        // outside the tree — the §Scope-8 display spelling is not what
        // sphinx records here (panel round C, sweep [8]).
        if let IncludeTarget::File { io_path, .. } = &target {
            if self.sphinx {
                if let Some(srcdir) = &self.srcdir {
                    let resolved_srcdir = crate::utils::resolve_path(srcdir);
                    if let Some(docname) = crate::utils::path2doc(io_path, &resolved_srcdir) {
                        self.included_records.push(docname);
                    }
                }
            }
        }
        let Some(text) = self.include_read_file(&target, &input, &clip, out) else {
            return DirectiveOutcome::Done;
        };
        let display = target.display().to_string();
        // Mode precedence: literal wins over code wins over parser (the
        // `if` chain order, `misc.py:102-107`).
        if opt_get(&input.options, "literal").is_some() {
            self.include_as_literal(&text, &display, tab_width, &input, out);
            return DirectiveOutcome::Done;
        }
        if opt_get(&input.options, "code").is_some() {
            self.include_as_code(&text, &display, tab_width, &input, out);
            return DirectiveOutcome::Done;
        }
        if opt_get(&input.options, "parser").is_some() {
            // §Scope-decision: documented divergence — docutils re-parses
            // with the named parser; this crate does not ship one yet.
            out.push(self.directive_run_message(
                messages::SEVERE,
                "Problem with \"include\" directive:\nparser mode is not supported by \
                 sphinx-ultra (planned with MyST, M2 wave 6)",
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return DirectiveOutcome::Done;
        }
        self.include_insert(&text, display, tab_width, clip, &input, out)
    }

    /// Path resolution: the standard-include guard (`misc.py:90-92`), then
    /// sphinx's docname-relative `relfn2path` rewrite when a project is
    /// attached (§Scope-2a: EVERY include argument, nested ones too,
    /// resolves against the current *document*'s directory —
    /// `SP/directives/other.py:413-416` rewrites before docutils ever
    /// sees the path), else docutils' containing-file-relative branch
    /// (`adapt_path`, `misc.py:28-39`).
    fn resolve_include_target(&self, path_arg: &str, at_source: u16) -> IncludeTarget {
        if path_arg.len() >= 2 && path_arg.starts_with('<') && path_arg.ends_with('>') {
            return IncludeTarget::Standard(path_arg[1..path_arg.len() - 1].to_string());
        }
        if self.sphinx {
            if let Some(srcdir) = &self.srcdir {
                let rel = crate::utils::relfn2path_rel(path_arg, &self.docname);
                // The OPEN path resolves symlinks before interpreting
                // `..`, exactly as sphinx's `.resolve()` does — a lexical
                // collapse would read a different file through a
                // symlinked directory (see
                // [`crate::utils::relfn2path_io`]).
                let abs = crate::utils::relfn2path_io(path_arg, &self.docname, srcdir);
                // sphinx's `rel_fn` (`_relative_path(abs_fn, self.srcdir)`,
                // `environment/__init__.py:477`): the RESOLVED path spelled
                // relative to the resolved srcdir, walking up for a file
                // outside it — the spelling `note_dependency` records, so
                // `srcdir / record` is the file that was actually read.
                let record =
                    crate::utils::relative_path_walk_up(&abs, &crate::utils::resolve_path(srcdir));
                // §Scope-8: every path-bearing surface of included content
                // spells the srcdir-relative form (deliberate divergence
                // from sphinx's environment-dependent cwd-relative
                // spelling).
                return IncludeTarget::File {
                    io_path: abs,
                    display: rel,
                    record,
                };
            }
        }
        // docutils mode: relative to the directory of the file containing
        // the directive (which may itself be an included file).
        let source_path = self.sources.path(at_source);
        let base = containing_dir(source_path);
        let joined = if path_arg.starts_with('/') || base.is_empty() {
            path_arg.to_string()
        } else {
            format!("{base}/{path_arg}")
        };
        let display = if let Some(rest) = joined.strip_prefix('/') {
            format!("/{}", crate::utils::normalize_dot_segments(rest))
        } else {
            crate::utils::normalize_dot_segments(&joined)
        };
        IncludeTarget::File {
            io_path: std::path::PathBuf::from(&display),
            record: display.clone(),
            display,
        }
    }

    /// `read_file` (`misc.py:111-157`): open + decode + clip, with the
    /// probe-pinned SEVERE texts. `None` means an error node was pushed.
    fn include_read_file(
        &mut self,
        target: &IncludeTarget,
        input: &DirectiveInput<'_>,
        clip: &IncludeClip,
        out: &mut Vec<Node>,
    ) -> Option<String> {
        let severe = |me: &Self, text: &str| {
            me.directive_run_message(
                messages::SEVERE,
                text,
                input.span.source,
                input.lineno,
                input.rawsource,
            )
        };
        let mut text = match target {
            IncludeTarget::Standard(name) => match standard_include_text(name) {
                Some(text) => text.to_string(),
                None => {
                    // Divergence (documented): docutils spells the missing
                    // standard file cwd-relative into its installation
                    // directory; the `<name>` form is the only stable
                    // spelling this crate has.
                    out.push(severe(
                        self,
                        &format!(
                            "Problems with \"{}\" directive path:\nInputError: [Errno 2] No \
                             such file or directory: '{}'.",
                            input.name,
                            target.display()
                        ),
                    ));
                    return None;
                }
            },
            IncludeTarget::File {
                io_path,
                display,
                record,
            } => {
                let bytes = match std::fs::read(io_path) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        out.push(severe(
                            self,
                            &format!(
                                "Problems with \"{}\" directive path:\n{}.",
                                input.name,
                                py_input_error_text(&error, display)
                            ),
                        ));
                        return None;
                    }
                };
                // A successful open records the dependency BEFORE reading
                // (`misc.py:130`) — a decode failure below still records.
                self.record_include_dependency(record);
                // `encoding = self.options.get('encoding',
                // self.state.document.settings.input_encoding)`
                // (`misc.py:116`). The fallback differs by venue, both
                // probed: bare docutils leaves `input_encoding` at its
                // `'utf-8'` default and a BOM survives as U+FEFF, while
                // sphinx overwrites the setting with
                // `config.source_encoding`
                // (`environment/__init__.py:68` + `:375`) — the
                // `source_encoding` config key, whose default
                // [`SPHINX_DEFAULT_SOURCE_ENCODING`] strips it. A
                // configured name outside this crate's table falls back to
                // that default; `BuildConfig::validate` already warned.
                let encoding = match opt_get(&input.options, "encoding") {
                    Some(OptVal::Str(name)) => {
                        lookup_encoding(name).expect("the encoding converter validated the name")
                    }
                    _ if self.sphinx => lookup_encoding(&self.source_encoding)
                        .unwrap_or(SPHINX_DEFAULT_SOURCE_ENCODING),
                    _ => IncludeEncoding::Utf8,
                };
                match decode_include_bytes(&bytes, encoding) {
                    Ok(text) => text,
                    Err(error_text) => {
                        out.push(severe(
                            self,
                            &format!("Problem with \"{}\" directive:\n{error_text}", input.name),
                        ));
                        return None;
                    }
                }
            }
        };
        // Universal newlines (docutils reads in text mode).
        if text.contains('\r') {
            text = text.replace("\r\n", "\n").replace('\r', "\n");
        }
        // Clip: line slice, then start-after, then end-before, each on the
        // previous result (`misc.py:136-157`).
        let (startline, endline, starttext, endtext) = clip;
        if startline.map(|v| v != 0).unwrap_or(false) || endline.is_some() {
            let lines = py_splitlines(&text);
            let (from, to) = py_slice(lines.len(), *startline, *endline);
            text = lines[from..to].join("\n");
        }
        if !starttext.is_empty() {
            match text.find(starttext.as_str()) {
                Some(index) => text = text[index + starttext.len()..].to_string(),
                None => {
                    out.push(severe(
                        self,
                        &format!(
                            "Problem with \"start-after\" option of \"{}\" \
                             directive:\nText not found.",
                            input.name
                        ),
                    ));
                    return None;
                }
            }
        }
        if !endtext.is_empty() {
            match text.find(endtext.as_str()) {
                Some(index) => text.truncate(index),
                None => {
                    out.push(severe(
                        self,
                        &format!(
                            "Problem with \"end-before\" option of \"{}\" \
                             directive:\nText not found.",
                            input.name
                        ),
                    ));
                    return None;
                }
            }
        }
        Some(text)
    }

    /// The docutils-side dependency record
    /// (`settings.record_dependencies.add(path)`, `misc.py:130`, which
    /// sphinx's `DependenciesCollector` harvests into `env.dependencies`
    /// as `srcdir / _relative_path(path, srcdir)`): every successfully
    /// opened project file, non-doc files included, spelled as the
    /// RESOLVED path relative to the srcdir (`IncludeTarget::File::record`)
    /// — through a symlink that is the file actually read, not the lexical
    /// display path. Standard includes never reach here (§Scope-2b:
    /// recording their environment-specific path would outdate the
    /// document on every warm rebuild). Sphinx mode only — a standalone
    /// parse has no environment to replay into.
    fn record_include_dependency(&mut self, record: &str) {
        if self.sphinx && self.srcdir.is_some() {
            self.dependency_records.push(record.to_string());
        }
    }

    /// The `OverflowError` docutils lets escape from `str.expandtabs`
    /// (see [`c_int_tabsize`]) — sphinx aborts the whole build on it —
    /// as the SEVERE this parser reports instead. Raised exactly where
    /// docutils would have called `expandtabs`: after a successful read
    /// and clip, in `:literal:`/`:code:` mode only for a non-negative
    /// width (`misc.py:165-167`, `:193-194`), and in insert mode per line
    /// of `string2lines` — so never for an empty text (probed against
    /// docutils 0.22.4: a missing file, a decode failure or a bad
    /// `start-after` with a huge width report only their own error; an
    /// empty file in insert mode reports nothing; `:literal:` with a huge
    /// NEGATIVE width keeps its tabs silently).
    fn tab_width_overflow(&self, input: &DirectiveInput<'_>, text: &str) -> Node {
        self.directive_run_message(
            messages::SEVERE,
            &format!("Problem with \"{}\" directive:\n{text}", input.name),
            input.span.source,
            input.lineno,
            input.rawsource,
        )
    }

    /// `insert_into_input_lines` (`misc.py:236-267`): length check,
    /// circular check, marker suffix, splice.
    fn include_insert(
        &mut self,
        text: &str,
        display: String,
        tab_width: i64,
        clip: IncludeClip,
        input: &DirectiveInput<'_>,
        out: &mut Vec<Node>,
    ) -> DirectiveOutcome {
        let textlines = match string2lines_tw(text, tab_width) {
            Ok(lines) => lines,
            Err(overflow) => {
                out.push(self.tab_width_overflow(input, overflow));
                return DirectiveOutcome::Done;
            }
        };
        // Excessively long lines abort with a WARNING (`misc.py:245-250`);
        // the reported number restarts at the clip, like everything else.
        for (i, line) in textlines.iter().enumerate() {
            if line.chars().count() > LINE_LENGTH_LIMIT {
                let line_no = i as i64 + 1 + clip.0.unwrap_or(0);
                out.push(self.directive_run_message(
                    messages::WARNING,
                    &format!("\"{display}\": line {line_no} exceeds the line-length-limit."),
                    input.span.source,
                    input.lineno,
                    input.rawsource,
                ));
                return DirectiveOutcome::Done;
            }
        }
        // Circular inclusion (`misc.py:251-262`), keyed on
        // (source, clip options): the same file with different clipping is
        // legal, and sequential re-includes are legal because the marker
        // comment pops the log entry.
        if self.include_log.is_empty() {
            let root = self.include_display_of(input.span.source);
            self.include_log
                .push((root, (None, None, String::new(), String::new())));
        }
        if self
            .include_log
            .iter()
            .any(|(source, opts)| *source == display && *opts == clip)
        {
            let chain: Vec<&str> = std::iter::once(display.as_str())
                .chain(self.include_log.iter().rev().map(|(s, _)| s.as_str()))
                .collect();
            out.push(self.directive_run_message(
                messages::WARNING,
                &format!(
                    "circular inclusion in \"{}\" directive:\n{}",
                    input.name,
                    chain.join("\n> ")
                ),
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return DirectiveOutcome::Done;
        }
        self.include_log.push((display.clone(), clip));
        // Marker suffix for the comment-path pop (`misc.py:264`). The
        // blank line is load-bearing: without it an included file ending
        // in paragraph text would absorb the marker as a continuation
        // line.
        let mut lines = textlines;
        lines.push(String::new());
        lines.push(format!(".. end of inclusion from \"{display}\""));
        let after_lineno = lines.len() as u32 + 1;
        DirectiveOutcome::Splice(SpliceRequest {
            segments: vec![
                SpliceSegment {
                    lines: vec![String::new()],
                    source_path: format!("internal padding before {display}"),
                    first_lineno: 0,
                },
                SpliceSegment {
                    lines,
                    source_path: display.clone(),
                    first_lineno: 1,
                },
                SpliceSegment {
                    lines: vec![String::new()],
                    source_path: format!("internal padding after {display}"),
                    first_lineno: after_lineno,
                },
            ],
        })
    }

    /// The display spelling of `source`'s path — srcdir-relative when a
    /// project is attached (§Scope-8), the table path otherwise. Seeds the
    /// include log with the root document's spelling.
    fn include_display_of(&self, source: u16) -> String {
        let path = self.sources.path(source);
        if self.sphinx {
            if let Some(srcdir) = &self.srcdir {
                if let Ok(rel) = std::path::Path::new(path).strip_prefix(srcdir) {
                    return rel.to_string_lossy().replace('\\', "/");
                }
            }
        }
        path.to_string()
    }

    /// `:literal:` mode (`misc.py:159-185`).
    fn include_as_literal(
        &mut self,
        text: &str,
        display: &str,
        tab_width: i64,
        input: &DirectiveInput<'_>,
        out: &mut Vec<Node>,
    ) {
        // Tabs expand unless `tab_width` is negative (`misc.py:165-167`:
        // `if self.tab_width >= 0: text = text.expandtabs(self.tab_width)`),
        // so a negative width outside C-int range is never even looked at
        // — only a non-negative one raises. The `:number-lines:` column
        // width is sized further down from the EXPANDED text's
        // `splitlines()` count (`misc.py:174-176`).
        let text = if tab_width >= 0 {
            if let Err(overflow) = c_int_tabsize(tab_width) {
                out.push(self.tab_width_overflow(input, overflow));
                return;
            }
            py_expandtabs(text, tab_width)
        } else {
            text.to_string()
        };
        let mut node = Node::elem(kinds::LITERAL_BLOCK, input.span);
        node.set("source", AttrValue::Str(display.to_string()));
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        node.set("xml:space", AttrValue::Str("preserve".to_string()));
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        match opt_get(&input.options, "number-lines") {
            Some(value) => {
                // `firstline = options['number-lines'] or 1` — a bare flag
                // is Python None and an explicit 0 is falsy; both mean 1.
                let firstline = match value {
                    OptVal::Int(n) if *n != 0 => *n,
                    OptVal::Str(s) => saturating_i64(s),
                    _ => 1,
                };
                let text = text.strip_suffix('\n').unwrap_or(&text);
                // `lastline = firstline + len(text.splitlines())`
                // (`misc.py:176`) — measured on THIS text, i.e. after
                // `expandtabs` and after the single trailing newline is
                // removed, and with Python's full `splitlines()` boundary
                // set. The rendering below still walks `split('\n')`,
                // because that is what `NumberLines.__iter__` does; only
                // the column WIDTH comes from the splitlines count. The
                // two counts differ for a text ending in a blank line
                // (one fewer) and for the splitlines-only separators
                // (`\x1c`-`\x1e`, NEL, LS, PS), and an empty file counts
                // 0 — which is what keeps its column one digit wide.
                let content_len = py_splitlines(text).len();
                let code_lines: Vec<String> = text.split('\n').map(String::from).collect();
                push_number_lines(&mut node, &code_lines, firstline, content_len, input.span);
            }
            None => node.children.push(Node::text_node(text, input.span)),
        }
        out.push(node);
    }

    /// `:code:` mode (`misc.py:187-205`): delegate to the `code` directive
    /// machinery with the option value as the language argument. This
    /// crate's `code` is the Pygments-less docutils shape (wave 3), so a
    /// language argument fails with the pygments WARNING — a pinned
    /// divergence from the sphinx oracle, which ships pygments.
    fn include_as_code(
        &mut self,
        text: &str,
        display: &str,
        tab_width: i64,
        input: &DirectiveInput<'_>,
        out: &mut Vec<Node>,
    ) {
        // `misc.py:193-194`: the same negative-width gate as literal mode.
        let text = if tab_width >= 0 {
            if let Err(overflow) = c_int_tabsize(tab_width) {
                out.push(self.tab_width_overflow(input, overflow));
                return;
            }
            py_expandtabs(text, tab_width)
        } else {
            text.to_string()
        };
        let text = text.strip_suffix('\n').unwrap_or(&text);
        let language = match opt_get(&input.options, "code") {
            Some(OptVal::Str(s)) => s.clone(),
            _ => String::new(),
        };
        // `CodeBlock(self.name, [options.pop('code')], self.options,
        // [text.removesuffix('\n')], ...)` — an empty language behaves as
        // no argument (`body.py:159-162`: `language = ''` is falsy).
        let arguments = if language.is_empty() {
            Vec::new()
        } else {
            vec![language]
        };
        let sub_input = DirectiveInput {
            name: input.name,
            arguments,
            options: input.options.clone(),
            content: Vec::new(),
            span: input.span,
            lineno: input.lineno,
            content_lineno: input.content_lineno,
            rawsource: input.rawsource,
        };
        let code_lines: Vec<String> = text.split('\n').map(String::from).collect();
        // `content_len` = 1: docutils hands CodeBlock the whole file as a
        // SINGLE content element (`[text.removesuffix('\n')]`,
        // `misc.py:187-205`), so `body.py:194`'s `endline = startline +
        // len(self.content)` is `startline + 1` and the number column
        // comes out ragged. Faithful quirk — see [`push_number_lines`].
        self.run_code_with_lines(&sub_input, code_lines, Some(display), 1, out);
    }

    // ------------------------------------------------------------------
    // literalinclude glue (SP/directives/code.py LiteralInclude.run)
    // ------------------------------------------------------------------

    /// The `literalinclude` directive (`SP/directives/code.py:447-506`):
    /// resolve → `note_dependency` → reader chain → node anatomy. Every
    /// reader error funnels into ONE reporter warning at the directive
    /// line whose message is the error text (`code.py:505-506`); the
    /// reader's logger-channel warnings are logger records with the
    /// doc2path-doubled rendered location and never enter the tree
    /// ([INC §3.4]). `settings.file_insertion_enabled` is not modeled
    /// (always true — this crate has no docutils settings surface).
    fn run_literalinclude(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        let path_arg = input.arguments.first().cloned().unwrap_or_default();
        let (rel, filename, display_path) =
            self.literalinclude_resolve(&path_arg, input.span.source);
        // `env.note_dependency(rel_filename)` runs BEFORE the file is
        // read (`code.py:463-464`) — an unreadable file still records,
        // and so does an option-conflict error (the reader is constructed
        // after). The `:diff:` file is deliberately NOT recorded.
        if self.sphinx && self.srcdir.is_some() {
            self.dependency_records.push(rel);
        }
        let options = self.literalinclude_options(&input);
        let mut reader = match LiteralIncludeReader::new(
            filename.clone(),
            options.clone(),
            &self.source_encoding,
        ) {
            Ok(reader) => reader,
            Err(text) => {
                out.push(self.msg(messages::WARNING, &text, input.span.source, input.lineno));
                return;
            }
        };
        let result = reader.read();
        // Logger-channel warnings surface whether or not the read
        // succeeded (probed: `:lines: 99` warns out-of-range AND errs
        // no-lines-pulled — both reach the stream).
        for message in reader.take_warnings() {
            self.push_literalinclude_log_warning(message, &input);
        }
        let (text, lines) = match result {
            Ok(pair) => pair,
            Err(text) => {
                out.push(self.msg(messages::WARNING, &text, input.span.source, input.lineno));
                return;
            }
        };

        let mut lb = Node::elem(kinds::LITERAL_BLOCK, input.span);
        lb.set("force", AttrValue::Int(i64::from(options.force)));
        // `language`: 'udiff' in diff mode, else the option verbatim —
        // and absent entirely otherwise: NO highlight_language fallback
        // (`code.py:472-475`; contrast CodeBlock, which always sets it).
        if options.diff.is_some() {
            lb.set("language", AttrValue::Str("udiff".to_string()));
        } else if let Some(language) = &options.language {
            lb.set("language", AttrValue::Str(language.clone()));
        }
        // `linenos` is stamped only when one of the three numbering
        // options asks for it (probed: absent otherwise, `True` → "1").
        if options.linenos || options.lineno_start.is_some() || options.lineno_match {
            lb.set("linenos", AttrValue::Int(1));
        }
        lb.attrs.classes.extend(options.classes.iter().cloned());
        // `highlight_args`: `hl_lines` first when `:emphasize-lines:` is
        // given (1-based, filtered to the post-filter count — which is
        // also the out-of-range denominator), `linenostart`
        // UNCONDITIONAL (`code.py:483-494`).
        let mut hl_lines: Option<Vec<i64>> = None;
        if let Some(spec_text) = &options.emphasize_lines {
            let total = lines as i64;
            match parse_line_num_spec(spec_text, total) {
                Ok(spec) => {
                    if spec.any_out_of_range(total) {
                        self.push_literalinclude_log_warning(
                            format!(
                                "line number spec is out of range(1-{}): {}",
                                total,
                                py_repr(Some(spec_text))
                            ),
                            &input,
                        );
                    }
                    hl_lines = Some(spec.in_range_values(total).iter().map(|x| x + 1).collect());
                }
                Err(text) => {
                    // An invalid spec replaces the WHOLE block with the
                    // reporter warning (the run()-except funnel).
                    out.push(self.msg(messages::WARNING, &text, input.span.source, input.lineno));
                    return;
                }
            }
        }
        let linenostart = reader.lineno_start;
        let highlight_args = match &hl_lines {
            Some(values) => format!(
                "{{'hl_lines': [{}], 'linenostart': {linenostart}}}",
                values
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!("{{'linenostart': {linenostart}}}"),
        };
        lb.set("highlight_args", AttrValue::Str(highlight_args));
        // The `source` ATTRIBUTE is the included file's absolute path
        // (what pformat shows, `code.py:469`); the node's span keeps the
        // rst file + directive line (`set_source_info`).
        lb.set("source", AttrValue::Str(display_path.display().to_string()));
        lb.set("xml:space", AttrValue::Str("preserve".to_string()));
        if !text.is_empty() {
            lb.children.push(Node::text_node(text, input.span));
        }

        match &options.caption {
            Some(caption_option) => {
                // `caption = self.options.get('caption') or self.arguments[0]`
                // (`code.py:496-498`): the EMPTY `:caption:` falls back to
                // the include path as written.
                let caption_text = if caption_option.is_empty() {
                    path_arg.as_str()
                } else {
                    caption_option.as_str()
                };
                match self.container_wrapper(caption_text, lb, &input, out) {
                    Ok(container) => out.push(container),
                    Err(text) => out.push(self.msg(
                        messages::WARNING,
                        &text,
                        input.span.source,
                        input.lineno,
                    )),
                }
            }
            None => {
                self.directive_add_name(
                    &mut lb,
                    &input.options,
                    input.span.source,
                    input.lineno,
                    out,
                );
                out.push(lb);
            }
        }
    }

    /// `container_wrapper` (`code.py:78-96`): the caption parses as RST —
    /// a leading `system_message` raises the `Invalid caption` ValueError
    /// into the reporter funnel; otherwise the first node's children become
    /// the caption (everything after it is discarded, exactly as sphinx
    /// keeps only `parsed[0]`). An unnamed container gets its id from the
    /// read pass's AutoNumbering (210, [`crate::transforms`]).
    fn container_wrapper(
        &mut self,
        caption: &str,
        literal_node: Node,
        input: &DirectiveInput<'_>,
        out: &mut Vec<Node>,
    ) -> Result<Node, String> {
        // `directive.parse_text_to_nodes(caption,
        // offset=directive.content_offset)`: a throwaway parse whose nodes
        // past the first are dropped — but whose messages were created,
        // and printed, at the content offset's lines.
        let parsed =
            self.parse_detached(caption, input.content_lineno, input.span.source, "caption");
        let first = parsed.into_iter().next();
        if let Some(node) = &first {
            if node.kind == kinds::SYSTEM_MESSAGE {
                // `'Invalid caption: %s' % node.astext()` — the message
                // renders through system_message.astext()'s
                // `source:line: (TYPE/level)` prefix (probed).
                return Err(format!("Invalid caption: {}", system_message_astext(node)));
            }
        }
        let mut container = Node::elem("container", input.span);
        container
            .attrs
            .classes
            .push("literal-block-wrapper".to_string());
        container.set("literal_block", AttrValue::Int(1));
        let mut caption_node = Node::elem("caption", input.span);
        if let Some(node) = first {
            caption_node.children = node.children;
        }
        container.children.push(caption_node);
        container.children.push(literal_node);
        // `add_name` lands on the CONTAINER (`code.py:502`).
        self.directive_add_name(
            &mut container,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        Ok(container)
    }

    /// Path resolution for the literalinclude argument and its `:diff:`
    /// file: sphinx's docname-relative `env.relfn2path` when a project is
    /// attached (`code.py:454-456`, `:463`), else the containing-file
    /// fallback (a parse without an environment — the directive is
    /// sphinx-registered only, but the parser stays total). Returns
    /// `(rel_filename, io path, display path)`: the reader OPENS the
    /// resolved path ([`crate::utils::relfn2path_io`] — symlinks before
    /// `..`, like sphinx's `.resolve()`), `note_dependency` records that
    /// resolved path spelled relative to the resolved srcdir (sphinx's
    /// `rel_fn`, `_relative_path(abs_fn, srcdir)` — `../ext/x.txt` for a
    /// file the symlink led outside the tree), and the `literal_block`
    /// `source` attribute keeps the lexical spelling §Scope-8 fixes.
    fn literalinclude_resolve(
        &self,
        path_arg: &str,
        at_source: u16,
    ) -> (String, std::path::PathBuf, std::path::PathBuf) {
        if self.sphinx {
            if let Some(srcdir) = &self.srcdir {
                let io_path = crate::utils::relfn2path_io(path_arg, &self.docname, srcdir);
                let record = crate::utils::relative_path_walk_up(
                    &io_path,
                    &crate::utils::resolve_path(srcdir),
                );
                return (
                    record,
                    io_path,
                    crate::utils::relfn2path(path_arg, &self.docname, srcdir),
                );
            }
        }
        let source_path = self.sources.path(at_source);
        let base = containing_dir(source_path);
        let joined = if path_arg.starts_with('/') || base.is_empty() {
            path_arg.to_string()
        } else {
            format!("{base}/{path_arg}")
        };
        let display = if let Some(rest) = joined.strip_prefix('/') {
            format!("/{}", crate::utils::normalize_dot_segments(rest))
        } else {
            crate::utils::normalize_dot_segments(&joined)
        };
        let path = std::path::PathBuf::from(&display);
        (display, path.clone(), path)
    }

    /// The converted directive options, retyped for the reader.
    fn literalinclude_options(&self, input: &DirectiveInput<'_>) -> LiteralIncludeOptions {
        let get_str = |name: &str| match opt_get(&input.options, name) {
            Some(OptVal::Str(s)) => Some(s.clone()),
            _ => None,
        };
        LiteralIncludeOptions {
            dedent: match opt_get(&input.options, "dedent") {
                Some(OptVal::Null) => Some(None),
                Some(OptVal::Int(n)) => Some(Some(*n)),
                // A beyond-i64 value keeps canonical digits as a string;
                // saturation strips the same everything a Python cut of
                // that size would.
                Some(OptVal::Str(s)) => Some(Some(saturating_i64(s))),
                _ => None,
            },
            linenos: opt_get(&input.options, "linenos").is_some(),
            lineno_start: opt_i64(&input.options, "lineno-start"),
            lineno_match: opt_get(&input.options, "lineno-match").is_some(),
            tab_width: opt_i64(&input.options, "tab-width"),
            language: get_str("language"),
            force: opt_get(&input.options, "force").is_some(),
            encoding: get_str("encoding"),
            pyobject: get_str("pyobject"),
            lines: get_str("lines"),
            start_after: get_str("start-after"),
            end_before: get_str("end-before"),
            start_at: get_str("start-at"),
            end_at: get_str("end-at"),
            prepend: get_str("prepend"),
            append: get_str("append"),
            emphasize_lines: get_str("emphasize-lines"),
            caption: get_str("caption"),
            classes: match opt_get(&input.options, "class") {
                Some(OptVal::StrList(classes)) => classes.clone(),
                _ => Vec::new(),
            },
            // `run()` makes the diff file absolute via `env.relfn2path`
            // BEFORE the reader is built (`code.py:454-456`).
            diff: get_str("diff").map(|d| self.literalinclude_resolve(&d, input.span.source).1),
        }
    }

    /// One literalinclude logger-channel warning ([INC §3.4]): a logger
    /// record with the doc2path-doubled rendered location (see
    /// [`super::diagnostics::Diagnostic::doc2path_location`]), never the
    /// tree. The location is the directive's `(source, line)` tuple — under
    /// an include, the included file's own provenance.
    fn push_literalinclude_log_warning(&mut self, message: String, input: &DirectiveInput<'_>) {
        self.log_warning(message, input.span.source, input.lineno, true);
    }

    /// A directive's `logger.warning(text, location=...)` with no
    /// `type`/`subtype` — so no `[category]` suffix — recorded where Sphinx
    /// makes the call. `doc2path_location` marks a tuple `location=` whose
    /// path `doc2path` suffixes (see
    /// [`super::diagnostics::Diagnostic::doc2path_location`]).
    fn log_warning(&self, text: String, source: u16, line: u32, doc2path_location: bool) {
        self.reporter.log(
            messages::WARNING,
            None,
            text,
            source,
            Some(line),
            doc2path_location,
        );
    }

    /// `.. program::` (`domains/std/__init__.py:333-348`): pure
    /// `env.ref_context` state, no nodes. The literal argument `None` pops
    /// the scope rather than naming a program called "None".
    fn run_program(&mut self, input: DirectiveInput<'_>) {
        let Some(argument) = input.arguments.first() else {
            return;
        };
        let program = ws_collapse(argument.trim(), "-");
        if program == "None" {
            self.program = None;
        } else {
            self.program = Some(program);
        }
    }

    /// sphinx math (patches.py MathDirective + math-domain numbering).
    /// Absent label/number are Python None -> pformat "True".
    fn run_sphinx_math(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        let mut latex = self.join_lines(&input.content);
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
    fn run_index(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
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
        index.set("entries", AttrValue::List(entries));
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
    fn run_hlist(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
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
    ///
    /// The entry split is a line-by-line port of `Glossary.run`'s state
    /// machine (`domains/std/__init__.py:434-510`) — `in_definition`,
    /// `in_comment`, `was_empty`, `indent_len` — rather than a chunker,
    /// because three of its four states are observable:
    ///
    /// * a term line that follows a definition body without a blank line
    ///   still joins a NEW entry, and warns;
    /// * terms separated BY a blank line still join the SAME entry (one
    ///   `definition_list_item` with several `term` children), and warn;
    /// * an unindented `.. ` comment (`:452-455`, the trailing space is
    ///   part of the test so a bare `..` is a term) neither ends an entry
    ///   nor touches `was_empty` — it `continue`s before the flag is
    ///   cleared — so terms on both sides of a comment share one entry, and
    ///   the comment's indented continuation lines are swallowed
    ///   (`:493-494`, `elif in_comment: pass`).
    ///
    /// The three misformat warnings are `self.state.reporter.warning`
    /// calls, i.e. docutils reporter messages: they land in the tree as
    /// `system_message` nodes BEFORE the glossary node (`return [*messages,
    /// node]`, `:555`) and, in Sphinx, additionally on stderr through the
    /// docutils→logging bridge with a `[docutils]` type suffix. Only the
    /// in-tree half is produced here; CLI-surfacing in-tree system_messages
    /// is a pre-existing project-wide gap (wave-4.5 task 12 ruling), not a
    /// glossary-specific one.
    ///
    /// LINE NUMBER: the warnings are reported at `lineno` taken from
    /// `self.content.items`, whose offsets are 0-based, while the reporter
    /// renders them as 1-based line numbers — so Sphinx reports each of
    /// these three warnings ONE LINE LOW. Probe-pinned against 9.1.0 (a
    /// term on document line 5 reports `line="4"`), and reproduced here
    /// with `lineno - 1`, because the doctree oracle compares the bytes.
    fn run_glossary(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        /// `_('glossary term must be preceded by empty line')` (`:461-465`).
        const NEEDS_EMPTY_LINE: &str = "glossary term must be preceded by empty line";
        /// `_('glossary terms must not be separated by empty lines')`
        /// (`:470-476`).
        const NOT_SEPARATED: &str = "glossary terms must not be separated by empty lines";
        /// `_('glossary seems to be misformatted, check indentation')`
        /// (`:481-486` and `:497-503`).
        const MISFORMATTED: &str = "glossary seems to be misformatted, check indentation";

        let mut glossary = Node::elem("glossary", input.span);
        glossary.set(
            "sorted",
            AttrValue::Int(i64::from(opt_get(&input.options, "sorted").is_some())),
        );
        let mut dl = Node::elem(kinds::DEFINITION_LIST, input.span);
        dl.attrs.classes.push("glossary".to_string());

        // ---- `Glossary.run`'s first loop: collect entries + warnings ----
        let mut entries: Vec<(Vec<LineRec>, Vec<LineRec>)> = Vec::new();
        let mut msgs: Vec<Node> = Vec::new();
        let mut in_definition = true;
        let mut in_comment = false;
        let mut was_empty = true;
        let mut indent_len = 0usize;
        for &rec in input.content.iter() {
            // `if not line:` — every source line is right-stripped by
            // `string2lines`, so a whitespace-only line IS empty here.
            if rec.is_blank() {
                if in_definition {
                    if let Some(last) = entries.last_mut() {
                        last.1.push(rec);
                    }
                }
                was_empty = true;
                continue;
            }
            if rec.indent() == 0 {
                if is_glossary_comment(&rec, self.sources.line_text(rec)) {
                    in_comment = true;
                    // `continue` BEFORE `was_empty = False`: a comment is
                    // invisible to the blank-line bookkeeping.
                    continue;
                }
                in_comment = false;
                if in_definition {
                    if !was_empty {
                        msgs.push(self.glossary_msg(NEEDS_EMPTY_LINE, rec));
                    }
                    entries.push((vec![rec], Vec::new()));
                    in_definition = false;
                } else {
                    if was_empty {
                        msgs.push(self.glossary_msg(NOT_SEPARATED, rec));
                    }
                    match entries.last_mut() {
                        Some(last) => last.0.push(rec),
                        // Sphinx-unreachable (`entries` is non-empty
                        // whenever `in_definition` is false), kept because
                        // the port mirrors the branch structure.
                        None => msgs.push(self.glossary_msg(MISFORMATTED, rec)),
                    }
                }
            } else if !in_comment {
                if !in_definition {
                    // "first line of definition, determines indentation"
                    in_definition = true;
                    indent_len = rec.indent();
                }
                // `line[indent_len:]` (`:501`) — a raw slice, so a
                // continuation line indented LESS than the first one loses
                // non-whitespace characters (probe: `   shallow` under a
                // 6-column first line renders `llow`). Python slices by
                // CHARACTER and clamps past the end, so the offset goes
                // through `rest_after_offset`: a byte-count `min` would
                // both split a multi-byte char (panic) and cut the wrong
                // number of characters off a non-ASCII line.
                let off = rest_after_offset(self.sources.line_text(rec), indent_len);
                let line = self.rewrap_from(rec, off);
                match entries.last_mut() {
                    Some(last) => last.1.push(line),
                    None => msgs.push(self.glossary_msg(MISFORMATTED, rec)),
                }
            }
            was_empty = false;
        }

        // ---- `Glossary.run`'s second loop: entries -> definition list ----
        for (term_lines, def_lines) in &entries {
            let mut def_lines = def_lines.clone();
            while def_lines.last().map(|l| l.is_blank()).unwrap_or(false) {
                def_lines.pop();
            }
            let mut item = Node::elem(kinds::DEFINITION_LIST_ITEM, input.span);
            let mut term_messages: Vec<Node> = Vec::new();
            for tl in term_lines {
                // `split_term_classifiers` (`domains/std/__init__.py:366-372`):
                // `parts = _term_classifiers_re.split(line)` on ` +: +`,
                // then `term = parts[0]` and `first_classifier = parts[1]`
                // — both VERBATIM, nothing stripped. Unlike docutils'
                // `Text.term` (which rstrips the term), `term\xa0 : cls`
                // keeps its NBSP in the <term>, the index entry and the
                // registered term, and `term : \xa0cls` keeps it in the
                // index key. A term line is at content column 0 and
                // `string2lines`-rstripped, so there is nothing to trim.
                // Probe-pinned, panel fix round F (gen_sphinx_fixture.py).
                let mut parts = split_classifiers(self.sources.line_text(*tl)).into_iter();
                let term_text = parts.next().unwrap_or_default();
                let index_key = parts.next();
                // Sphinx's `make_glossary_term` stamps the term node with
                // the *term line's* own source info, not the directive's
                // (`domains/std/__init__.py:386-388`), and the index node it
                // appends inherits it. Everything that reports a term's
                // location — the duplicate-object warning, above all — reads
                // that, so each term carries its own span here.
                let term_span = Span {
                    source: tl.source,
                    line: tl.lineno,
                    start: tl.start,
                    end: tl.end,
                };
                // `self.parse_inline(term_, lineno=lineno)` hands the inliner
                // the content item's 0-BASED offset (the quirk
                // [`Self::glossary_msg`] documents), so a term's inline
                // messages sit one line up (probed: a term on line 4 warns
                // at line 3).
                let inline = self.inline(&term_text, term_span, tl.lineno.saturating_sub(1));
                let mut term = Node::elem(kinds::TERM, term_span);
                term.children = inline.nodes;
                term_messages.extend(inline.messages);
                // `termtext = term.astext()` (`domains/std:389`), taken from
                // the PARSED term and before the index node is appended: a
                // term written with markup registers, indexes and ids itself
                // under its rendered text, not its source text.
                let term_text = term.astext();
                // `make_glossary_term` (`domains/std:375-407`):
                // `make_id(env, document, 'term', termtext)` — sphinx's
                // case-preserving `_make_id` fork, with a `term`-keyed
                // serial fallback of its own, then `note_explicit_target`.
                let node_id = self.registry.sphinx_make_id("term", &term_text);
                self.registry.note_explicit_id(&node_id);
                term.attrs.ids.push(node_id.clone());
                // `std._note_term(termtext, node_id, location=term)`: the
                // registration — and its duplicate warning — happens here,
                // after this term's inline messages and before its
                // definition is parsed. `location=term` prints the line
                // `make_glossary_term` stamped on the term, the item's
                // 0-based offset (the quirk `glossary_msg` documents).
                self.glossary_term_records.push(super::GlossaryTermRecord {
                    term: term_text.clone(),
                    node_id: node_id.clone(),
                    source: tl.source,
                    line: tl.lineno.saturating_sub(1),
                    seq: self.reporter.next_seq(),
                });
                let mut index = Node::elem("index", term_span);
                index.set(
                    "entries",
                    AttrValue::List(vec![index_entry_tuple(
                        "single",
                        &term_text,
                        &node_id,
                        "main",
                        index_key.as_deref(),
                    )]),
                );
                term.children.push(index);
                item.children.push(term);
            }
            item.children.extend(term_messages);
            let mut definition = Node::elem(kinds::DEFINITION, input.span);
            definition.children = self.parse_nested(&def_lines, "definition");
            item.children.push(definition);
            dl.children.push(item);
        }
        // `GlossarySorter` (`transforms/__init__.py:426-442`), a read-phase
        // transform at priority 500, so the STORED doctree is already
        // sorted:
        //
        //     definition_list[:] = sorted(
        //         definition_list,
        //         key=lambda item: unicodedata.normalize(
        //             'NFD', cast('nodes.term', item)[0].astext().lower()))
        //
        // Running it here rather than as a transform is equivalent: the
        // list is complete, `sorted` is stable, and the ids/index entries
        // were already allocated in SOURCE order by the loop above — which
        // is what sphinx does too, since the directive runs long before the
        // transform. (Sphinx defers it only so i18n can substitute the
        // terms first; this crate has no i18n phase.)
        if opt_get(&input.options, "sorted").is_some() {
            dl.children.sort_by_key(Self::glossary_sort_key);
        }
        glossary.children.push(dl);
        // `return [*messages, node]` (`domains/std/__init__.py:555`).
        out.extend(msgs);
        out.push(glossary);
    }

    /// `GlossarySorter`'s key: the NFD-normalized, lowercased `astext()` of
    /// the item's FIRST `term` child. The `index` node `make_glossary_term`
    /// appends to that term contributes nothing to `astext()` (an empty
    /// `Element` under a `TextElement`'s empty child separator), so the key
    /// is the rendered term text. Python compares strings by code point,
    /// which is the same order as Rust's UTF-8 byte comparison.
    fn glossary_sort_key(item: &Node) -> String {
        let text = item
            .children
            .first()
            .map(|term| term.astext())
            .unwrap_or_default();
        text.to_lowercase().nfd().collect()
    }

    /// One of `Glossary.run`'s three misformat warnings, anchored the way
    /// Sphinx anchors them: at the content item's 0-based offset, which the
    /// reporter then renders as a 1-based line — see [`Self::run_glossary`].
    fn glossary_msg(&self, text: &str, rec: LineRec) -> Node {
        self.msg(
            messages::WARNING,
            text,
            rec.source,
            rec.lineno.saturating_sub(1),
        )
    }

    /// sphinx `ObjectDescription.run` (`directives/__init__.py:183-314`):
    /// the `index` + `desc` anatomy every object-describing directive
    /// shares, with each subclass's `handle_signature` /
    /// `add_target_and_index` / `transform_content` inlined by
    /// [`ObjectDescKind`].
    fn run_object_description(
        &mut self,
        kind: DescDispatch,
        input: DirectiveInput<'_>,
        out: &mut Vec<Node>,
    ) {
        let Some(argument) = input.arguments.first() else {
            return;
        };
        // `self.name` is the directive name as written for the bare
        // docutils registration, but `'{domain}:{name}'` for a domain
        // directive (`Domain.directive`'s adapter, `domains/__init__.py`),
        // which is why `describe` reports `domain=""` and `option` reports
        // `domain="std"` / `objtype="option"`. For the aliasing py
        // directives, `run()` rewrites `self.name` BEFORE the base run
        // partitions it, so the objtype is the ALIASED kind's (trap 13).
        let (domain, objtype) = match kind {
            DescDispatch::Std(ObjectDescKind::Describe) => ("", input.name.to_string()),
            DescDispatch::Std(_) => ("std", input.name.to_lowercase()),
            DescDispatch::Py(py) => ("py", py.kind.objtype().to_string()),
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
        // Names are the `(fullname, name_prefix)` tuples py's
        // `handle_signature` returns (`_object.py:397`); std kinds carry an
        // empty prefix. Dedup is on the whole tuple, exactly like the base
        // run's `if name not in self.names` (`directives/__init__.py:273`).
        let mut names: Vec<(String, String)> = Vec::new();
        for sig in object_signatures(argument, self.py.strip_signature_backslash) {
            let mut signode = Node::elem("desc_signature", span);
            signode
                .attrs
                .classes
                .extend(["sig".to_string(), "sig-object".to_string()]);
            let name = self.handle_object_signature(kind, &sig, &input, &mut signode);
            if let DescDispatch::Std(std_kind) = kind {
                // `_toc_parts`/`_toc_name` are assigned in a `finally`
                // (`:264-272`), so the ValueError path carries them too.
                // Only `ConfigurationValue` overrides the two empty
                // defaults; the py arm stamps its own inside
                // `handle_py_signature`.
                //
                // The `finally` gates the real values on
                // `if self.config.toc_object_entries:` and otherwise
                // assigns `()` / `''` — for EVERY object description, not
                // just the py ones, so the std arm needs the same gate.
                let (toc_parts, toc_name) = match (std_kind, &name) {
                    (ObjectDescKind::Confval, Some((n, _))) if self.py.toc_object_entries => {
                        (format!("({},)", py_repr(Some(n))), n.clone())
                    }
                    _ => ("()".to_string(), String::new()),
                };
                signode.set("_toc_parts", AttrValue::Str(toc_parts));
                signode.set("_toc_name", AttrValue::Str(toc_name));
            }
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
                            &input,
                            &mut signode,
                            &mut index_entries,
                        );
                    }
                }
            }
            desc.children.push(signode);
        }

        // py `before_content` (`_object.py:449-480`): the class/module
        // ref_context pushes the nested content parses under.
        if let DescDispatch::Py(py) = kind {
            self.py_before_content(py, &names, &input);
        }
        let mut content = Node::elem("desc_content", span);
        content.children = self.parse_nested(&input.content, "desc_content");
        if kind == DescDispatch::Std(ObjectDescKind::Confval) {
            self.confval_transform_content(&input, &mut content);
        }
        // Base-run tail order (`directives/__init__.py`): the
        // `object-description-transform` event fires FIRST (its only
        // handler, `filter_meta_fields`, guards `domain == 'py'` —
        // `domains/python/__init__.py:610-611` — so a std `:meta:` field
        // survives and renders renamed), then `DocFieldTransformer`
        // rewrites the doc fields UNCONDITIONALLY for every object
        // description (`directives/__init__.py:295`) — std kinds get an
        // empty typemap, so their fields all take the unknown
        // rename-and-pass-through branch — then `after_content` pops the
        // ref_context the py field xrefs just read.
        match kind {
            DescDispatch::Py(py) => {
                filter_meta_fields(&mut content);
                self.transform_doc_fields(&mut content, py_field_type_map);
                self.py_after_content(py, &input);
            }
            DescDispatch::Std(_) => {
                self.transform_doc_fields(&mut content, std_field_type_map);
            }
        }
        desc.children.push(content);

        let mut index = Node::elem("index", span);
        index.set("entries", AttrValue::List(index_entries));
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

    /// The per-subclass `handle_signature`. Returns the object name — a
    /// `(name, prefix)` tuple, prefix empty for std kinds — or `None` for
    /// the ValueError path, where `run` clears the signature node and drops
    /// the whole signature into one `desc_name` (`:259-263`), which each
    /// arm does itself.
    fn handle_object_signature(
        &mut self,
        kind: DescDispatch,
        sig: &str,
        input: &DirectiveInput<'_>,
        signode: &mut Node,
    ) -> Option<(String, String)> {
        let span = signode.span;
        let std_name = |name: String| Some((name, String::new()));
        match kind {
            // The base `handle_signature` raises unconditionally (`:100-111`).
            DescDispatch::Std(ObjectDescKind::Describe) => {
                signode.children.clear();
                signode.children.push(desc_name_node(sig, span));
                None
            }
            // `GenericObject.handle_signature` (`domains/std:56-64`).
            DescDispatch::Std(ObjectDescKind::EnvVar) => {
                signode.children.clear();
                signode.children.push(desc_name_node(sig, span));
                std_name(ws_collapse(sig, " "))
            }
            // `ConfigurationValue.handle_signature` (`domains/std:126-131`).
            DescDispatch::Std(ObjectDescKind::Confval) => {
                signode.children.clear();
                signode.children.push(desc_name_node(sig, span));
                let name = ws_collapse(sig, " ");
                signode.set("fullname", AttrValue::Str(name.clone()));
                std_name(name)
            }
            DescDispatch::Std(ObjectDescKind::Cmdoption) => self
                .handle_option_signature(sig, input.lineno, signode)
                .and_then(std_name),
            DescDispatch::Py(py) => self.handle_py_signature(py, sig, input, signode),
        }
    }

    /// `Cmdoption.handle_signature` (`domains/std/__init__.py:229-290`) with
    /// `option_emphasise_placeholders` at its default False, which is the
    /// plain `desc_name` + `desc_addname` pair per spelling.
    fn handle_option_signature(
        &mut self,
        sig: &str,
        lineno: u32,
        signode: &mut Node,
    ) -> Option<String> {
        let span = signode.span;
        let mut firstname: Option<String> = None;
        let mut allnames: Vec<String> = Vec::new();
        for potential in sig.split(", ") {
            // `potential_option.strip()` (`domains/std/__init__.py`) —
            // Python's set (round F pin: `-x, \x1f-y` registers both).
            let potential = potential.trim_matches(crate::utils::py_isspace);
            let Some((optname, args)) = option_desc_match(potential) else {
                // This diagnostic goes to the logger, not the tree
                // (`domains/std/__init__.py:237-245`), located on the
                // signature node — which carries the directive's own line.
                // The spelling contributes nothing either way.
                self.log_warning(
                    format!(
                        "Malformed option description {}, should look like \"opt\", \
                         \"-opt args\", \"--opt args\", \"/opt args\" or \"+opt args\"",
                        py_repr(Some(potential))
                    ),
                    span.source,
                    lineno,
                    false,
                );
                continue;
            };
            // "optional value surrounded by brackets (ex. foo[=bar])".
            // Sphinx tests `args[-1] == ']'` unguarded, so `.. option:: foo[`
            // raises IndexError out of the whole parse; leaving the
            // signature unchanged is the hardening deviation (a crash is not
            // a contract, and there is no tree to be byte-identical to).
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
    /// `make_id`, the index entries, and the domain registration records
    /// the env layer replays.
    fn object_target_and_index(
        &mut self,
        kind: DescDispatch,
        objtype: &str,
        name_cls: &(String, String),
        input: &DirectiveInput<'_>,
        signode: &mut Node,
        entries: &mut Vec<String>,
    ) {
        let name: &str = &name_cls.0;
        let line = input.lineno;
        match kind {
            DescDispatch::Py(py) => {
                self.py_target_and_index(py, objtype, name_cls, input, signode, entries)
            }
            // `ObjectDescription.add_target_and_index` is `pass` (`:113-120`)
            // — no id, no index entry, no std object. (Unreachable in
            // practice: `Describe`'s handle_signature never returns a name.)
            DescDispatch::Std(ObjectDescKind::Describe) => {}
            // `GenericObject.add_target_and_index` (`domains/std:66-84`).
            // `EnvVar.indextemplate` has no ':' separator, so the whole
            // template is a 'single' entry value.
            DescDispatch::Std(ObjectDescKind::EnvVar) => {
                let node_id = self.note_object_id(objtype, name, line, signode);
                entries.push(index_entry_tuple(
                    "single",
                    &format!("environment variable; {name}"),
                    &node_id,
                    "",
                    None,
                ));
            }
            // `ConfigurationValue.add_target_and_index` (`domains/std:142-151`).
            DescDispatch::Std(ObjectDescKind::Confval) => {
                let node_id = self.note_object_id(objtype, name, line, signode);
                entries.push(index_entry_tuple(
                    "pair",
                    &format!("{name}; configuration value"),
                    &node_id,
                    "",
                    None,
                ));
            }
            // `Cmdoption.add_target_and_index` (`domains/std:292-330`).
            DescDispatch::Std(ObjectDescKind::Cmdoption) => {
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
                            source: signode.span.source,
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

    /// The `make_id` + `note_explicit_target` + `note_object` trio the
    /// single-id `add_target_and_index` implementations share.
    fn note_object_id(
        &mut self,
        objtype: &str,
        name: &str,
        line: u32,
        signode: &mut Node,
    ) -> String {
        // Both callers pass `self.objtype` as the make_id prefix.
        let node_id = self.registry.sphinx_make_id(objtype, name);
        signode.attrs.ids.push(node_id.clone());
        self.registry.note_explicit_id(&node_id);
        self.std_object_records.push(super::ObjectRegistration {
            source: signode.span.source,
            objtype: objtype.to_string(),
            name: name.to_string(),
            node_id: node_id.clone(),
            line,
            // `note_object` warns about a duplicate right here, at parse
            // time; the merge phase replays it into this position.
            seq: self.reporter.next_seq(),
        });
        node_id
    }

    /// `PyObject.handle_signature` with every subclass override inlined
    /// (`domains/python/_object.py:248-397`, `__init__.py`, [PY §1.3]).
    /// Returns `(fullname, name_prefix)`, or `None` for the no-match
    /// ValueError path — silent, whole sig in one `desc_name`, empty toc
    /// attrs, no registration (trap 7).
    fn handle_py_signature(
        &mut self,
        py: PyDirective,
        sig: &str,
        input: &DirectiveInput<'_>,
        signode: &mut Node,
    ) -> Option<(String, String)> {
        let span = signode.span;
        let Some(m) = py_sig_match(sig) else {
            signode.children.clear();
            signode.children.push(desc_name_node(sig, span));
            signode.set("_toc_parts", AttrValue::Str("()".to_string()));
            signode.set("_toc_name", AttrValue::Str(String::new()));
            return None;
        };

        // Python-truthy option access: a present-but-empty value is falsy
        // everywhere handle_signature consults these.
        let opt_str = |name: &'static str| match opt_get(&input.options, name) {
            Some(OptVal::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let opt_truthy = |name: &'static str| opt_str(name).filter(|s| !s.is_empty());

        // `modname = self.options.get('module', ref_context['py:module'])`
        // (`_object.py:263`): option PRESENCE wins, even with an empty value.
        let modname: Option<String> = match opt_str("module") {
            Some(module) => Some(module),
            None => self.py_module.clone(),
        };
        let ref_class = self.py_class.clone().filter(|c| !c.is_empty());

        // Module/class resolution (`_object.py:262-285`).
        let mut prefix = m.prefix.clone().unwrap_or_default();
        let name = m.name.clone();
        let fullname: String;
        let classname_attr: String;
        let add_module: bool;
        match &ref_class {
            Some(classname) => {
                add_module = false;
                if !prefix.is_empty()
                    && (prefix == *classname || prefix.starts_with(&format!("{classname}.")))
                {
                    // Class name given again in the signature: stripped
                    // from display, kept in the fullname.
                    fullname = format!("{prefix}{name}");
                    prefix = prefix[classname.len()..]
                        .trim_start_matches('.')
                        .to_string();
                } else if !prefix.is_empty() {
                    // A DIFFERENT prefix inside a class nests under it:
                    // `D.meth` inside `C` → `C.D.meth` (probe
                    // method_other_prefix).
                    fullname = format!("{classname}.{prefix}{name}");
                } else {
                    fullname = format!("{classname}.{name}");
                }
                classname_attr = classname.clone();
            }
            None => {
                add_module = true;
                if !prefix.is_empty() {
                    // A dotted prefix at top level becomes the signature
                    // CLASS name, not a module (trap 12).
                    classname_attr = prefix.trim_end_matches('.').to_string();
                    fullname = format!("{prefix}{name}");
                } else {
                    classname_attr = String::new();
                    fullname = name.clone();
                }
            }
        }

        // Stamped on every successful signature (`_object.py:287-289`);
        // a None modname pformats as the `"True"` sentinel (trap 2).
        signode.set(
            "module",
            AttrValue::Str(modname.clone().unwrap_or_else(|| "True".to_string())),
        );
        signode.set("class", AttrValue::Str(classname_attr.clone()));
        signode.set("fullname", AttrValue::Str(fullname.clone()));

        let single_line = crate::py::arglist::SingleLineOpts {
            parameter_list: opt_get(&input.options, "single-line-parameter-list").is_some(),
            type_parameter_list: opt_get(&input.options, "single-line-type-parameter-list")
                .is_some(),
        };
        let (multi_line_params, multi_line_tp) =
            crate::py::arglist::multi_line_flags(sig, &m, single_line, &self.py);

        // Annotation xrefs read the RAW ref_context, not the option-
        // modified modname: the `:module:` option only touches
        // `env.ref_context` in before_content (`_annotations.py:62-66`).
        // They carry the signature's span: sphinx stamps the signode
        // (`set_source_info`) and locates annotation-xref warnings there
        // through the `get_source_line` ancestor walk.
        let ctx = crate::py::annotations::PyRefContext {
            module: self.py_module.clone(),
            class_: self.py_class.clone(),
            span,
        };

        // 1. Signature prefix keywords (`get_signature_prefix`).
        let prefix_nodes = py_signature_prefix(py, input);
        if !prefix_nodes.is_empty() {
            let mut anno = desc_annotation_node(span);
            anno.children = prefix_nodes;
            signode.children.push(anno);
        }

        // 2. Written prefix, else `{modname}.` under add_module_names
        // (`_object.py:326-330`).
        if !prefix.is_empty() {
            signode.children.push(desc_addname_node(&prefix, span));
        } else if let Some(modname) = modname.as_deref().filter(|s| !s.is_empty()) {
            if add_module && self.py.add_module_names {
                signode
                    .children
                    .push(desc_addname_node(&format!("{modname}."), span));
            }
        }

        // 3. Object name.
        signode.children.push(desc_name_node(&name, span));

        // 4. Type parameter list; any failure is a WARNING (`_object.py:
        // 342-345`), interpolating the exception text (probes
        // tp_list_warning / tp_list_tokerror).
        if let Some(tp_list) = m.tp_list.as_deref().filter(|t| !t.is_empty()) {
            match crate::py::arglist::parse_type_list(tp_list, multi_line_tp, &ctx, &self.py) {
                Ok(node) => signode.children.push(node),
                Err(err) => self.log_warning(
                    format!(
                        "could not parse tp_list ({}): {err}",
                        py_repr(Some(tp_list))
                    ),
                    span.source,
                    input.lineno,
                    false,
                ),
            }
        }

        // 5. Parameter list. An EMPTY written `()` is falsy and routes to
        // the needs_arglist branch, exactly like no parens at all (see
        // arglist_empty_still_carries_attrs in src/py/arglist.rs); the
        // bare paramlist carries NO multi_line attrs (trap 1).
        match m.arglist.as_deref().filter(|a| !a.is_empty()) {
            Some(arglist) => {
                match crate::py::arglist::parse_arglist(arglist, multi_line_params, &ctx, &self.py)
                {
                    Ok(node) => signode.children.push(node),
                    Err(crate::py::arglist::SigParseError::Syntax(_)) => {
                        // `logger.debug` — invisible (`_object.py:355-369`).
                        signode
                            .children
                            .push(crate::py::arglist::pseudo_parse_arglist(
                                arglist,
                                multi_line_params,
                                &ctx,
                                &self.py,
                            ));
                    }
                    Err(err) => {
                        // Duplicate parameter names: WARNING + pseudo
                        // fallback (`_object.py:370-381`, probe
                        // arglist_dup_warning).
                        self.log_warning(
                            format!(
                                "could not parse arglist ({}): {err}",
                                py_repr(Some(arglist))
                            ),
                            span.source,
                            input.lineno,
                            false,
                        );
                        signode
                            .children
                            .push(crate::py::arglist::pseudo_parse_arglist(
                                arglist,
                                multi_line_params,
                                &ctx,
                                &self.py,
                            ));
                    }
                }
            }
            None => {
                if py.needs_arglist() {
                    let mut params = Node::elem("desc_parameterlist", span);
                    params.set("xml:space", AttrValue::Str("preserve".to_string()));
                    signode.children.push(params);
                }
            }
        }

        // 6. Return annotation (`_object.py:387-389`).
        if let Some(retann) = m.retann.as_deref().filter(|r| !r.is_empty()) {
            let mut returns = Node::elem("desc_returns", span);
            returns.set("xml:space", AttrValue::Str("preserve".to_string()));
            returns.children = crate::py::annotations::parse_annotation(retann, &ctx, &self.py);
            signode.children.push(returns);
        }

        // 7. `:annotation:` option tail (`_object.py:391-395`).
        if let Some(anno) = opt_truthy("annotation") {
            let mut node = desc_annotation_node(span);
            node.children.push(crate::py::annotations::desc_sig_space());
            node.children.push(Node::text_node(anno, span));
            signode.children.push(node);
        }

        // Subclass tails run AFTER the base handle_signature returns:
        // `:type:`/`:value:` for data/attribute (the `:`/`=` here are
        // desc_sig_punctuation, unlike parameter defaults — trap 3),
        // `:type:` only for property, display-only `:canonical:` for
        // py:type.
        match py.kind {
            PyObjectKind::Data | PyObjectKind::Attribute => {
                if let Some(typ) = opt_truthy("type") {
                    let mut node = desc_annotation_node(span);
                    node.children
                        .push(crate::py::annotations::desc_sig_punctuation(":"));
                    node.children.push(crate::py::annotations::desc_sig_space());
                    node.children
                        .extend(crate::py::annotations::parse_annotation(
                            &typ, &ctx, &self.py,
                        ));
                    signode.children.push(node);
                }
                if let Some(value) = opt_truthy("value") {
                    let mut node = desc_annotation_node(span);
                    node.children.push(crate::py::annotations::desc_sig_space());
                    node.children
                        .push(crate::py::annotations::desc_sig_punctuation("="));
                    node.children.push(crate::py::annotations::desc_sig_space());
                    node.children.push(Node::text_node(value, span));
                    signode.children.push(node);
                }
            }
            PyObjectKind::Property => {
                if let Some(typ) = opt_truthy("type") {
                    let mut node = desc_annotation_node(span);
                    node.children
                        .push(crate::py::annotations::desc_sig_punctuation(":"));
                    node.children.push(crate::py::annotations::desc_sig_space());
                    node.children
                        .extend(crate::py::annotations::parse_annotation(
                            &typ, &ctx, &self.py,
                        ));
                    signode.children.push(node);
                }
            }
            PyObjectKind::TypeAlias => {
                if let Some(canonical) = opt_truthy("canonical") {
                    let mut node = desc_annotation_node(span);
                    node.children.push(crate::py::annotations::desc_sig_space());
                    node.children
                        .push(crate::py::annotations::desc_sig_punctuation("="));
                    node.children.push(crate::py::annotations::desc_sig_space());
                    node.children
                        .extend(crate::py::annotations::parse_annotation(
                            &canonical, &ctx, &self.py,
                        ));
                    signode.children.push(node);
                }
            }
            _ => {}
        }

        // Decorators insert the `@` addname FIRST, after everything else
        // ran (`__init__.py:124-127`, `313-316`).
        if py.decorator {
            signode.children.insert(0, desc_addname_node("@", span));
        }

        // `_toc_parts`/`_toc_name` — `_object_hierarchy_parts` +
        // `_toc_entry_name` (`_object.py:399-408`, `505-522`), gated on
        // `toc_object_entries` by the base run's finally (`:264-272`).
        if self.py.toc_object_entries {
            let mut parts: Vec<String> = Vec::new();
            if let Some(modname) = modname.as_deref().filter(|s| !s.is_empty()) {
                parts.push(modname.to_string());
            }
            parts.extend(fullname.split('.').map(str::to_string));
            let toc_name = py_toc_entry_name(&parts, &fullname, py.kind, &self.py);
            signode.set("_toc_parts", AttrValue::Str(py_tuple_repr(&parts)));
            signode.set("_toc_name", AttrValue::Str(toc_name));
        } else {
            signode.set("_toc_parts", AttrValue::Str("()".to_string()));
            signode.set("_toc_name", AttrValue::Str(String::new()));
        }

        Some((fullname, prefix))
    }

    /// `PyObject.add_target_and_index` + the PyFunction extension
    /// (`_object.py:415-447`, `__init__.py:95-109`, [PY §1.4/1.5]).
    fn py_target_and_index(
        &mut self,
        py: PyDirective,
        objtype: &str,
        name_cls: &(String, String),
        input: &DirectiveInput<'_>,
        signode: &mut Node,
        entries: &mut Vec<String>,
    ) {
        let opt_str = |name: &'static str| match opt_get(&input.options, name) {
            Some(OptVal::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let modname: Option<String> = match opt_str("module") {
            Some(module) => Some(module),
            None => self.py_module.clone(),
        };
        let modname = modname.filter(|m| !m.is_empty());
        let name = &name_cls.0;
        let fullname = match &modname {
            Some(modname) => format!("{modname}.{name}"),
            None => name.clone(),
        };
        // Empty prefix: the id IS the fullname, `id{n}` on collision
        // ([PY §1.5], the empty-prefix branch of `sphinx_make_id`).
        let node_id = self.registry.sphinx_make_id("", &fullname);
        signode.attrs.ids.push(node_id.clone());
        self.registry.note_explicit_id(&node_id);
        self.py_object_records.push(super::PyObjectRecord {
            fullname: fullname.clone(),
            objtype: objtype.to_string(),
            node_id: node_id.clone(),
            aliased: false,
            source: signode.span.source,
            lineno: input.lineno,
            seq: self.reporter.next_seq(),
        });
        // `:canonical:` registers an alias — except on py:type, where the
        // option is display-only (`_object.py:427-437`, §6).
        if py.kind != PyObjectKind::TypeAlias {
            if let Some(canonical) = opt_str("canonical").filter(|c| !c.is_empty()) {
                self.py_object_records.push(super::PyObjectRecord {
                    fullname: canonical,
                    objtype: objtype.to_string(),
                    node_id: node_id.clone(),
                    aliased: true,
                    source: signode.span.source,
                    lineno: input.lineno,
                    seq: self.reporter.next_seq(),
                });
            }
        }
        let has = |n: &'static str| opt_get(&input.options, n).is_some();
        if has("no-index-entry") || has("noindexentry") {
            return;
        }
        let index_text = py_index_text(
            py,
            input,
            modname.as_deref(),
            name,
            self.py.add_module_names,
        );
        if !index_text.is_empty() {
            entries.push(index_entry_tuple("single", &index_text, &node_id, "", None));
        }
        // PyFunction adds its entry in its own add_target_and_index
        // (`__init__.py:95-109`): module-less functions are a PAIR entry
        // (trap 10).
        if py.kind == PyObjectKind::Function {
            match &modname {
                Some(modname) => entries.push(index_entry_tuple(
                    "single",
                    &format!("{name}() (in module {modname})"),
                    &node_id,
                    "",
                    None,
                )),
                None => entries.push(index_entry_tuple(
                    "pair",
                    &format!("built-in function; {name}()"),
                    &node_id,
                    "",
                    None,
                )),
            }
        }
    }

    /// `PyObject.before_content` (`_object.py:449-480`): class scope from
    /// the LAST signature's name — the fullname for nesting kinds, the
    /// written prefix otherwise — plus the `:module:` option push.
    fn py_before_content(
        &mut self,
        py: PyDirective,
        names: &[(String, String)],
        input: &DirectiveInput<'_>,
    ) {
        let mut prefix: Option<String> = None;
        if let Some((fullname, name_prefix)) = names.last() {
            if py.allow_nesting() {
                prefix = Some(fullname.clone());
            } else if !name_prefix.is_empty() {
                prefix = Some(name_prefix.trim_matches('.').to_string());
            }
        }
        if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
            self.py_class = Some(prefix.clone());
            self.py_class_key = true;
            if py.allow_nesting() {
                self.py_classes.push(prefix);
                self.py_classes_key = true;
            }
        }
        if let Some(OptVal::Str(module)) = opt_get(&input.options, "module") {
            self.py_modules.push(self.py_module.take());
            self.py_module = Some(module.clone());
            self.py_module_key = true;
            self.py_modules_key = true;
        }
    }

    /// `PyObject.after_content` (`_object.py:482-503`): pop the nesting
    /// stack (nesting kinds only), always reassign `py:class` from the
    /// stack top, and undo the `:module:` push.
    fn py_after_content(&mut self, py: PyDirective, input: &DirectiveInput<'_>) {
        if py.allow_nesting() {
            self.py_classes.pop();
        }
        self.py_class = self.py_classes.last().cloned();
        // `after_content` `setdefault`s `py:classes` and assigns `py:class`
        // unconditionally, so both keys exist from here on.
        self.py_class_key = true;
        self.py_classes_key = true;
        if opt_get(&input.options, "module").is_some() {
            match self.py_modules.pop() {
                // `ref_context['py:module'] = modules.pop()`: the key
                // SURVIVES holding whatever `before_content` pushed —
                // `None` when nothing enclosed this directive. That is the
                // reachable branch under balanced nesting, and the one
                // `:any:` renders as the `"True"` sentinel.
                Some(previous) => {
                    self.py_module = previous;
                    self.py_module_key = true;
                }
                // `ref_context.pop('py:module')`: the key is removed.
                None => {
                    self.py_module = None;
                    self.py_module_key = false;
                }
            }
            self.py_modules_key = true;
        }
    }

    /// `DocFieldTransformer(self).transform_all(content_node)` — the base
    /// `run` applies it to EVERY object description AFTER the
    /// `object-description-transform` event and BEFORE `after_content`,
    /// so the ref_context the field xrefs read is still the object's own
    /// scope. Only immediate `field_list` children are transformed
    /// (`docfields.py:354-359`). The `map` is the directive's
    /// `get_field_type_map()`: the py table for py kinds, empty for the
    /// std kinds (none of them declare `doc_field_types`), whose fields
    /// therefore all take the unknown rename-and-pass-through branch.
    fn transform_doc_fields(&mut self, content: &mut Node, map: DocFieldTypeMap) {
        // The field builders stamp their nodes from `DocFieldEnv::span`,
        // not from here; the context's own span stays unstamped.
        let ctx = crate::py::annotations::PyRefContext {
            module: self.py_module.clone(),
            class_: self.py_class.clone(),
            span: Span::ZERO,
        };
        for child in &mut content.children {
            if child.kind == kinds::FIELD_LIST {
                transform_doc_field_list(child, map, &ctx, &self.py);
            }
        }
    }

    /// `PyModule.run` (`domains/python/__init__.py:492-536`, [PY §1.5]):
    /// node order `[index?, target, *content]`, always-set ref_context
    /// (trap 6), registration unless `:no-index:`.
    fn run_py_module(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        let Some(argument) = input.arguments.first() else {
            return;
        };
        let modname = argument.trim().to_string();
        let has = |n: &'static str| opt_get(&input.options, n).is_some();
        let no_index = has("no-index") || has("noindex");
        // ALWAYS sets the module scope, even under `:no-index:` (trap 6).
        self.py_module = Some(modname.clone());
        self.py_module_key = true;
        // Content parses BEFORE the module's own id is allocated
        // (`__init__.py:505-510`), so ids taken by content come first.
        // sphinx parses it with allow_section_headings=True; sections
        // inside nested content are not representable in this parser (a
        // pre-existing wave-4 limitation shared by every nested parse),
        // and the T8 corpus excludes section-bearing module content.
        let content = self.parse_nested(&input.content, "py_module");
        if !no_index {
            let node_id = self.registry.sphinx_make_id("module", &modname);
            self.registry.note_explicit_id(&node_id);
            let mut target = Node::elem(kinds::TARGET, input.span);
            target.attrs.ids.push(node_id.clone());
            target.set("ismod", AttrValue::Int(1));
            let opt = |name: &'static str| match opt_get(&input.options, name) {
                Some(OptVal::Str(s)) => s.clone(),
                _ => String::new(),
            };
            self.py_module_records.push(super::PyModuleRecord {
                name: modname.clone(),
                node_id: node_id.clone(),
                synopsis: opt("synopsis"),
                platform: opt("platform"),
                deprecated: has("deprecated"),
                source: input.span.source,
                lineno: input.lineno,
            });
            // `note_object(modname, 'module', node_id)` (`__init__.py:522`)
            // — modules also join the objects table.
            self.py_object_records.push(super::PyObjectRecord {
                fullname: modname.clone(),
                objtype: "module".to_string(),
                node_id: node_id.clone(),
                aliased: false,
                source: input.span.source,
                lineno: input.lineno,
                seq: self.reporter.next_seq(),
            });
            if !has("no-index-entry") {
                let mut index = Node::elem("index", input.span);
                index.set(
                    "entries",
                    AttrValue::List(vec![index_entry_tuple(
                        "pair",
                        &format!("module; {modname}"),
                        &node_id,
                        "",
                        None,
                    )]),
                );
                out.push(index);
            }
            // NOTE §Scope-3: this is the parse-layer shape — the target
            // keeps its ids. The read-transform pass ([`crate::transforms`])
            // moves them: onto the enclosing section when the module opens
            // it (MoveModuleTargets), else onto the next body node with the
            // target keeping a `refid` (PropagateTargets; trap 5).
            out.push(target);
        }
        out.extend(content);
    }

    /// `PyCurrentModule.run` (`__init__.py:550-556`): pure ref_context
    /// state, no nodes; the literal argument `None` pops the scope.
    fn run_py_currentmodule(&mut self, input: DirectiveInput<'_>) {
        let Some(argument) = input.arguments.first() else {
            return;
        };
        let modname = argument.trim();
        if modname == "None" {
            // `ref_context.pop('py:module', None)` — the key goes away.
            self.py_module = None;
            self.py_module_key = false;
        } else {
            self.py_module = Some(modname.to_string());
            self.py_module_key = true;
        }
    }

    /// `ConfigurationValue.transform_content` (`domains/std:153-185`):
    /// `:type:` and `:default:` render as a field list prepended to the
    /// description content, each field followed by its own inline messages.
    fn confval_transform_content(&mut self, input: &DirectiveInput<'_>, content: &mut Node) {
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
        input: DirectiveInput<'_>,
        out: &mut Vec<Node>,
    ) {
        let (type_name, label, lead_fmt) = *info;
        let version = &input.arguments[0];
        // Inline messages from the explanation must anchor on the text's
        // own line, not the directive marker (review finding 41) — and an
        // explanation given as the second argument parses at
        // `lineno=self.lineno + 1` (`domains/changeset.py:72-75`; probed:
        // `.. versionadded:: 1.0 *x` warns at line 2).
        let mut text_lineno = input.lineno + 1;
        let text: Option<String> = input
            .arguments
            .get(1)
            .cloned()
            .or_else(|| {
                if input.content.is_empty() {
                    None
                } else {
                    text_lineno = input.content[0].lineno;
                    Some(self.join_lines(&input.content))
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
    fn run_seealso(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let mut node = Node::elem("seealso", input.span);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        let content = self.parse_nested(&input.content, "seealso");
        node.children.extend(content);
        out.push(node);
    }

    /// sphinx code-block (sphinx/directives/code.py CodeBlock): language
    /// falls back to the `.. highlight::` state, then the configured
    /// `highlight_language` (`current_document.highlight_language or
    /// config.highlight_language`, `code.py:157-166`); :caption: wraps in a
    /// literal-block-wrapper container that takes the ids/names.
    fn run_sphinx_code_block(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        let language = input
            .arguments
            .first()
            .cloned()
            .or_else(|| self.highlight_language.clone())
            .unwrap_or_else(|| self.config_highlight_language.clone());
        // sphinx util.parselinenos + CodeBlock.run: an invalid spec
        // REPLACES the whole block with a WARNING system_message;
        // out-of-range lines are filtered (review findings 31/43/45/46).
        let nlines = input.content.len() as i64;
        let mut hl_lines: Vec<i64> = Vec::new();
        if let Some(OptVal::Str(spec)) = opt_get(&input.options, "emphasize-lines") {
            match parse_linenos(spec, nlines) {
                Ok(lines_list) => {
                    // `CodeBlock.run` warns through the logger when any
                    // member is past the content (`code.py:130-137`),
                    // located with a `(source, line)` tuple — which
                    // `doc2path` suffixes (probed: `<snippet>.rst:1:`).
                    if parse_line_num_spec(spec, nlines).is_ok_and(|s| s.any_out_of_range(nlines)) {
                        self.log_warning(
                            format!(
                                "line number spec is out of range(1-{nlines}): {}",
                                py_repr(Some(spec))
                            ),
                            input.span.source,
                            input.lineno,
                            true,
                        );
                    }
                    hl_lines = lines_list;
                }
                Err(msg) => {
                    out.push(self.msg(messages::WARNING, &msg, input.span.source, input.lineno));
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
        let code = self.join_lines(&input.content);
        lb.children.push(Node::text_node(code, input.span));
        match opt_get(&input.options, "caption") {
            // `container_wrapper` (`code.py:174-177`), shared with
            // literalinclude: the caption is a throwaway nested parse, and a
            // leading system_message becomes the `Invalid caption` warning
            // the directive returns instead.
            Some(OptVal::Str(caption_text)) => {
                let caption_text = caption_text.clone();
                match self.container_wrapper(&caption_text, lb, &input, out) {
                    Ok(container) => out.push(container),
                    Err(text) => out.push(self.msg(
                        messages::WARNING,
                        &text,
                        input.span.source,
                        input.lineno,
                    )),
                }
            }
            _ => {
                self.directive_add_name(
                    &mut lb,
                    &input.options,
                    input.span.source,
                    input.lineno,
                    out,
                );
                out.push(lb);
            }
        }
    }

    /// sphinx highlight: emits a highlightlang node AND sets the state
    /// later code-blocks read (env.temp_data parity).
    fn run_highlight(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
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
    fn run_only(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
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
    fn run_toctree(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        let glob = matches!(opt_get(&input.options, "glob"), Some(OptVal::Null));
        let mut entries: Vec<super::ToctreeEntryRecord> = Vec::new();
        let mut raw_entries: Vec<String> = Vec::new();
        for l in &input.content {
            if l.is_blank() {
                continue;
            }
            // `for entry in self.content` (`TocTree.parse_content`) — the
            // line VERBATIM, nothing stripped: an entry indented deeper than
            // the block keeps its extra spaces and names a document that does
            // not exist (round F, env-test pin; `trim()` had resolved it).
            let t = self.sources.line_text(*l);
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
        // `TocTree.parse_content` does — including its `logger.warning`s,
        // recorded here in the order it logs them; a parse with no
        // environment (`found_docs: None`) resolves nothing and leaves both
        // empty.
        let resolved = match &self.found_docs {
            Some(found) => {
                crate::env::toctree::resolve_entries(&crate::env::toctree::ToctreeContent {
                    content: &raw_entries,
                    docname: &self.docname,
                    glob,
                    reversed: opt_get(&input.options, "reversed").is_some(),
                    source: input.span.source,
                    line: input.lineno,
                    found_docs: found,
                    source_suffixes: SOURCE_SUFFIXES,
                    exclude_patterns: &self.exclude_patterns,
                })
            }
            None => crate::env::toctree::ResolvedEntries::default(),
        };
        for warning in &resolved.warnings {
            self.reporter.log(
                messages::WARNING,
                warning.category.clone(),
                warning.message.clone(),
                warning.source,
                Some(warning.line),
                false,
            );
        }
        self.toctree_records.push(super::ToctreeRecord {
            glob,
            entries: entries.clone(),
            source: input.span.source,
            line: input.lineno,
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
        // No `rawentries`/`rawcaption`: Sphinx's directive writes neither;
        // PreserveTranslatableMessages (010) does, in the read pass
        // (`crate::transforms`).
        toctree.set(
            "titlesonly",
            AttrValue::Int(i64::from(opt_get(&input.options, "titlesonly").is_some())),
        );
        let mut compound = Node::elem("compound", input.span);
        compound.attrs.classes.push("toctree-wrapper".to_string());
        compound.children.push(toctree);
        out.push(compound);
    }

    fn substitution_context_error(&self, input: &DirectiveInput<'_>, out: &mut Vec<Node>) -> bool {
        if self.substitution_ctx.is_some() {
            return false;
        }
        out.push(self.directive_run_error(
            &format!(
                "Invalid context: the \"{}\" directive can only be used within a substitution definition.",
                input.name
            ),
            input.span.source, input.lineno,
            input.rawsource,
        ));
        true
    }

    /// replace (misc.py:357-387).
    fn run_replace(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if self.substitution_context_error(&input, out) {
            return;
        }
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
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
                input.span.source,
                input.lineno,
            ));
        }
    }

    /// unicode (misc.py:390-431).
    fn run_unicode(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
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
        // `self.comment_pattern.split(...)[0].split()` (misc.py:422) —
        // Python's `str.split()`, so `0x41\x1f0x42` is TWO codes.
        for code in crate::utils::py_split(codes_text) {
            match unicode_code(code) {
                Ok(s) => out.push(Node::text_node(s, input.span)),
                Err(detail) => {
                    out.push(self.directive_run_error(
                        &format!("Invalid character code: {code}\nValueError: {detail}"),
                        input.span.source,
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
    fn run_date(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if self.substitution_context_error(&input, out) {
            return;
        }
        let format = if input.content.is_empty() {
            "%Y-%m-%d".to_string()
        } else {
            self.join_lines(&input.content)
        };
        out.push(Node::text_node(strftime_now(&format), input.span));
    }

    /// Table.make_title (tables.py:46-57).
    fn table_make_title(&mut self, input: &DirectiveInput<'_>) -> (Option<Node>, Vec<Node>) {
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
        input: &DirectiveInput<'_>,
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
        self.directive_add_name(
            &mut table,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        if let Some(t) = title {
            table.children.insert(0, t);
        }
        out.push(table);
        out.extend(title_messages);
    }

    /// table (tables.py RSTTable:127-172).
    fn run_rst_table(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            // RSTTable's missing-content diagnostic is a WARNING, unlike
            // the assert_has_content ERROR family (tables.py:135-139).
            out.push(self.directive_reported_message(
                messages::WARNING,
                &format!(
                    "Content block expected for the \"{}\" directive; none found.",
                    input.name
                ),
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let (title, title_messages) = self.table_make_title(&input);
        let children = self.parse_nested(&input.content, "element");
        if children.len() != 1 || children[0].kind != kinds::TABLE {
            out.push(self.directive_reported_error(
                &format!(
                    "Error parsing content block for the \"{}\" directive: exactly one table expected.",
                    input.name
                ),
                input.span.source, input.lineno,
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
                    out.push(self.directive_reported_error(
                        &format!(
                            "\"{}\" widths do not match the number of columns in table ({}).",
                            input.name, n_cols
                        ),
                        input.span.source,
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
    fn run_csv_table(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        // `CSVTable.run` makes the title before it reads any data
        // (`tables.py:286`): the title's inline messages are created —
        // and printed — even when a data error then discards the title.
        let (title, title_messages) = self.table_make_title(&input);
        let has_file = opt_get(&input.options, "file").is_some();
        let has_url = opt_get(&input.options, "url").is_some();
        // get_csv_data (tables.py:321-388).
        let csv_text: String;
        if !input.content.is_empty() {
            if has_file || has_url {
                out.push(self.directive_reported_error(
                    &format!(
                        "\"{}\" directive may not both specify an external file and have content.",
                        input.name
                    ),
                    input.span.source,
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
            csv_text = self.join_lines(&input.content);
        } else if has_file {
            if has_url {
                out.push(self.directive_reported_error(
                    &format!(
                        "The \"file\" and \"url\" options may not be simultaneously specified for the \"{}\" directive.",
                        input.name
                    ),
                    input.span.source, input.lineno,
                    input.rawsource,
                ));
                return;
            }
            let Some(OptVal::Str(path)) = opt_get(&input.options, "file") else {
                unreachable!("file option is Path-converted");
            };
            let base = std::path::Path::new(self.sources.path(0))
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            match std::fs::read_to_string(base.join(path)) {
                Ok(t) => csv_text = t,
                Err(_) => {
                    // Unlike raw's io.error_string (InputError: prefix),
                    // the csv path formats the bare OSError.
                    out.push(self.directive_reported_message(
                        messages::SEVERE,
                        &format!(
                            "Problems with \"{}\" directive path:\n[Errno 2] No such file or directory: {}.",
                            input.name,
                            py_repr(Some(path))
                        ),
                        input.span.source, input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
            }
        } else {
            out.push(self.directive_reported_message(
                messages::WARNING,
                &format!(
                    "The \"{}\" directive requires content; none supplied.",
                    input.name
                ),
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
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
            out.push(self.directive_reported_error(
                &msg,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        // Column widths (tables.py:101-118).
        let widths_opt = opt_get(&input.options, "widths").cloned();
        let col_widths: Vec<i64> = match &widths_opt {
            Some(OptVal::IntList(list)) => {
                if list.len() != max_cols {
                    out.push(self.directive_reported_error(
                        &format!(
                            "\"{}\" widths do not match the number of columns in table ({}).",
                            input.name, max_cols
                        ),
                        input.span.source,
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
                list.clone()
            }
            _ => {
                if max_cols == 0 {
                    out.push(self.directive_reported_error(
                        "No table data detected in CSV file.",
                        input.span.source,
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
                        entry.children =
                            // `build_table` nests every csv cell at the
                            // directive's `content_offset` (each cell's
                            // row offset is 0, `tables.py:398`, `states.py:1951`).
                            self.parse_detached(
                                cell,
                                input.content_lineno,
                                input.span.source,
                                "entry",
                            );
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
    fn run_list_table(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_reported_error(
                &format!(
                    "The \"{}\" directive is empty; content required.",
                    input.name
                ),
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let (title, title_messages) = self.table_make_title(&input);
        let children = self.parse_nested(&input.content, "element");
        let content_error = |me: &Self, detail: &str| -> Node {
            me.directive_reported_error(
                &format!(
                    "Error parsing content block for the \"{}\" directive: {detail}",
                    input.name
                ),
                input.span.source,
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
            out.push(self.directive_reported_error(
                &msg,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let n_cols = first_len.unwrap_or(0);
        let widths_opt = opt_get(&input.options, "widths").cloned();
        let col_widths: Vec<i64> = match &widths_opt {
            Some(OptVal::IntList(list)) => {
                if list.len() != n_cols {
                    out.push(self.directive_reported_error(
                        &format!(
                            "\"{}\" widths do not match the number of columns in table ({}).",
                            input.name, n_cols
                        ),
                        input.span.source,
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
        input: DirectiveInput<'_>,
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
                    input.span.source,
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
            if opt_get(&input.options, "subtitle").is_some() && input.arguments.is_empty() {
                out.push(self.directive_run_error(
                    "The \"subtitle\" option may not be used without a title.",
                    input.span.source,
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
                    input.span.source,
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
        }
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
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
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        let content = self.parse_nested(&input.content, kind);
        node.children.extend(content);
        out.push(node);
    }

    /// rubric (body.py:240-254): inline children, no paragraph wrapper,
    /// inline messages as siblings after the node.
    fn run_rubric(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        let inline = self.inline(&input.arguments[0], input.span, input.lineno);
        let mut node = Node::elem("rubric", input.span);
        node.children = inline.nodes;
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        out.push(node);
        out.extend(inline.messages);
    }

    /// epigraph / highlights / pull-quote (body.py:257-283): standard
    /// block-quote elements, each block_quote stamped with the class.
    fn run_quote_class(
        &mut self,
        class: &'static str,
        input: DirectiveInput<'_>,
        out: &mut Vec<Node>,
    ) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
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
    fn run_compound(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let mut node = Node::elem("compound", input.span);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        let content = self.parse_nested(&input.content, "compound");
        node.children.extend(content);
        out.push(node);
    }

    /// container (body.py:304-329): classes come from the ARGUMENT.
    fn run_container(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
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
                        input.span.source,
                        input.lineno,
                        input.rawsource,
                    ));
                    return;
                }
            }
        }
        let mut node = Node::elem("container", input.span);
        node.attrs.classes.extend(classes);
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        let content = self.parse_nested(&input.content, "container");
        node.children.extend(content);
        out.push(node);
    }

    /// parsed-literal (body.py:132-146): full inline parse inside a
    /// whitespace-preserving literal_block; messages follow the node.
    fn run_parsed_literal(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let text = self.join_lines(&input.content);
        let inline = self.inline(&text, input.span, input.lineno);
        let mut node = Node::elem(kinds::LITERAL_BLOCK, input.span);
        node.set("xml:space", AttrValue::Str("preserve".to_string()));
        node.children = inline.nodes;
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        out.push(node);
        out.extend(inline.messages);
    }

    /// DirectiveError-style message (raised by a directive's own run()):
    /// message text VERBATIM — no 'Error in "X" directive:' prefix — plus
    /// the raw block as a literal_block child (states.py:2287-2291).
    ///
    /// The literal is appended AFTER creation there — `msg_node =
    /// self.reporter.system_message(error.level, error.msg, line=lineno)`
    /// then `msg_node += nodes.literal_block(block_text, block_text)` —
    /// so the stream (and Sphinx's printed record) carries the paragraph
    /// only while the tree node has both: [`Self::msg`], then the child.
    fn directive_run_message(
        &self,
        level: u8,
        text: &str,
        source: u16,
        lineno: u32,
        rawsource: &str,
    ) -> Node {
        messages::with_literal(self.msg(level, text, source, lineno), rawsource)
    }

    fn directive_run_error(&self, text: &str, source: u16, lineno: u32, rawsource: &str) -> Node {
        self.directive_run_message(messages::ERROR, text, source, lineno, rawsource)
    }

    /// A message a directive creates itself — `self.reporter.error(text,
    /// nodes.literal_block(self.block_text, self.block_text),
    /// line=self.lineno)`, returned rather than raised (the table
    /// directives, `tables.py`; figure's caption check, `images.py:179-183`)
    /// — so the literal is there at creation and the stream prints it,
    /// unlike [`Self::directive_run_message`]'s.
    fn directive_reported_message(
        &self,
        level: u8,
        text: &str,
        source: u16,
        lineno: u32,
        rawsource: &str,
    ) -> Node {
        self.msg_literal(level, text, source, lineno, rawsource)
    }

    /// [`Self::directive_reported_message`] at ERROR level.
    fn directive_reported_error(
        &self,
        text: &str,
        source: u16,
        lineno: u32,
        rawsource: &str,
    ) -> Node {
        self.directive_reported_message(messages::ERROR, text, source, lineno, rawsource)
    }

    /// assert_has_content() (rst/__init__.py:370-377).
    fn directive_content_error(
        &self,
        name: &str,
        source: u16,
        lineno: u32,
        rawsource: &str,
    ) -> Node {
        self.directive_run_error(
            &format!("Content block expected for the \"{name}\" directive; none found."),
            source,
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
        source: u16,
        lineno: u32,
        out: &mut Vec<Node>,
    ) {
        if let Some(OptVal::Str(n)) = opt_get(options, "name") {
            self.note_explicit_name(node, n, source, lineno, out);
        }
    }

    /// `node['names'].append(fully_normalize_name(name))` followed by
    /// `document.note_explicit_target(node, node)` — the tail shared by
    /// `Directive.add_name` and `Figure`'s `figname` branch
    /// (`images.py:156-157`).
    fn note_explicit_name(
        &mut self,
        node: &mut Node,
        name: &str,
        source: u16,
        lineno: u32,
        out: &mut Vec<Node>,
    ) {
        node.attrs.names.push(ids::fully_normalize_name(name));
        let source_path = self.sources.arc_path(source);
        let msg = self
            .registry
            .set_id_explicit(node, lineno, source, &source_path, true, None);
        if let Some(m) = msg {
            out.push(self.created(m));
        }
    }

    fn run_admonition(
        &mut self,
        kind: &'static str,
        input: DirectiveInput<'_>,
        out: &mut Vec<Node>,
    ) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let mut node = Node::elem(kind, input.span);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        let content = self.parse_nested(&input.content, kind);
        node.children.extend(content);
        out.push(node);
    }

    fn run_generic_admonition(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
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
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
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

    fn run_image(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        match self.build_image(&input, out) {
            Ok(node) => out.push(node),
            Err(msg) => out.push(*msg),
        }
    }

    /// images.py Image.run(): builds the image node (possibly wrapped in a
    /// reference); Err carries the system_message. Shared with figure.
    fn build_image(
        &mut self,
        input: &DirectiveInput<'_>,
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
                    input.span.source, input.lineno,
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
        self.directive_add_name(
            &mut image,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
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
    fn run_figure(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
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
                    !matches!(n.as_str(), "figwidth" | "figclass" | "figname" | "align")
                        && !(name_on_figure && n == "name")
                })
                .cloned()
                .collect(),
            content: Vec::new(),
            span: input.span,
            lineno: input.lineno,
            content_lineno: input.content_lineno,
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
        // `if figname:` (`images.py:155-157`) — the figure's OWN explicit
        // target, registered BEFORE the caption/legend parse (unlike
        // sphinx's `:name:`, which lands after it). The truthiness test is
        // Python's, so a valueless `:figname:` is a no-op where a valueless
        // `:name:` would still push an empty name through `add_name`.
        if let Some(OptVal::Str(n)) = opt_get(&input.options, "figname") {
            if !n.is_empty() {
                let n = n.clone();
                self.note_explicit_name(&mut figure, &n, input.span.source, input.lineno, out);
            }
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
                    out.push(self.directive_reported_error(
                        "Figure caption must be a paragraph or empty comment.",
                        input.span.source,
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
            self.directive_add_name(
                &mut figure,
                &input.options,
                input.span.source,
                input.lineno,
                out,
            );
        }
        out.push(figure);
    }

    /// code (body.py:149-211). The parity oracle runs docutils WITHOUT
    /// Pygments: a language argument fails the whole directive with a
    /// WARNING; language-less code emits a plain classes="code" literal.
    fn run_code(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let code_lines: Vec<String> = input
            .content
            .iter()
            .map(|l| self.sources.line_text(*l).to_string())
            .collect();
        // The directive's own content is one list element per line, so
        // `len(self.content)` is the real line count here.
        let content_len = code_lines.len();
        self.run_code_with_lines(&input, code_lines, None, content_len, out);
    }

    /// The `code` node construction shared by the directive itself and the
    /// `include` directive's `:code:` mode (which passes its file text as
    /// the lines and its path as the `source` attribute —
    /// `CodeBlock.run`'s "if called from include" branch).
    ///
    /// `content_len` is docutils' `len(self.content)` — the number of
    /// content LIST ELEMENTS, which sizes the `number-lines` column (see
    /// [`push_number_lines`]; the include-called path passes 1).
    fn run_code_with_lines(
        &mut self,
        input: &DirectiveInput<'_>,
        code_lines: Vec<String>,
        source_attr: Option<&str>,
        content_len: usize,
        out: &mut Vec<Node>,
    ) {
        if !input.arguments.is_empty() {
            out.push(self.directive_run_message(
                messages::WARNING,
                "Cannot analyze code. Pygments package not found.",
                input.span.source,
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
                            input.span.source,
                            input.lineno,
                            input.rawsource,
                        ));
                        return;
                    }
                }
            }
            // The include directive's converter is flag-or-int
            // (`value_or((None,), int)`): a bare flag is Python None, and
            // `CodeBlock` numbers it from `int(None or 1)`.
            Some(OptVal::Null) => Some(1),
            Some(OptVal::Int(n)) => Some(*n),
            _ => None,
        };
        let mut node = Node::elem(kinds::LITERAL_BLOCK, input.span);
        node.attrs.classes.push("code".to_string());
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            node.attrs.classes.extend(classes.iter().cloned());
        }
        if let Some(source) = source_attr {
            node.set("source", AttrValue::Str(source.to_string()));
        }
        node.set("xml:space", AttrValue::Str("preserve".to_string()));
        match number_lines {
            Some(start) => {
                push_number_lines(&mut node, &code_lines, start, content_len, input.span)
            }
            None => {
                node.children
                    .push(Node::text_node(code_lines.join("\n"), input.span));
            }
        }
        self.directive_add_name(
            &mut node,
            &input.options,
            input.span.source,
            input.lineno,
            out,
        );
        out.push(node);
    }

    /// math (body.py:214-237): blank-line-separated blocks become sibling
    /// math_block nodes; :name: only lands on the first (options.pop).
    fn run_math(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        let joined = self.join_lines(&input.content);
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
                self.directive_add_name(
                    &mut node,
                    &input.options,
                    input.span.source,
                    input.lineno,
                    out,
                );
                named = true;
            }
            out.push(node);
        }
    }

    /// raw (misc.py:270-354).
    fn run_raw(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
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
                    input.span.source,
                    input.lineno,
                    input.rawsource,
                ));
                return;
            }
            text = self.join_lines(&input.content);
        } else if has_file {
            if has_url {
                out.push(self.directive_run_error(
                    &format!(
                        "The \"file\" and \"url\" options may not be simultaneously specified for the \"{}\" directive.",
                        input.name
                    ),
                    input.span.source, input.lineno,
                    input.rawsource,
                ));
                return;
            }
            let Some(OptVal::Str(path)) = opt_get(&input.options, "file") else {
                unreachable!("file option is Path-converted");
            };
            let base = std::path::Path::new(self.sources.path(0))
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
                        input.span.source, input.lineno,
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
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        } else {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        // `' '.join(self.arguments[0].lower().split())` (misc.py:296).
        let format = crate::utils::py_split(&input.arguments[0].to_lowercase())
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
    fn run_line_block(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        if input.content.is_empty() {
            out.push(self.directive_content_error(
                input.name,
                input.span.source,
                input.lineno,
                input.rawsource,
            ));
            return;
        }
        // `LineBlock.run` (`directives/body.py:110-130`) calls `add_name`
        // BEFORE the lines: the name takes its ids first, and a duplicate-
        // name message is created — and written — ahead of the lines'
        // messages. docutils then detaches it (`note_explicit_target(node,
        // node)` appends it to the line_block, whose content model admits
        // no system_message, and `msgnode.validate` pops it,
        // `nodes.py:1982-1989`), so it prints but never reaches the tree.
        let mut block_names = Node::elem(kinds::LINE_BLOCK, input.span);
        let mut detached: Vec<Node> = Vec::new();
        self.directive_add_name(
            &mut block_names,
            &input.options,
            input.span.source,
            input.lineno,
            &mut detached,
        );
        let mut resolved: Vec<(usize, Vec<Node>)> = Vec::with_capacity(input.content.len());
        let mut lb_messages: Vec<Node> = Vec::new();
        let mut prev_depth = 0usize;
        // Each content line — blank ones counted — inline-parses at
        // `self.lineno + self.content_offset`, the offset advancing per
        // line: docutils adds the 0-based content offset to the directive's
        // own line (probed: a directive at line 5 with content from line 8
        // warns at 12).
        let first_line = input.lineno + input.content_lineno.saturating_sub(1);
        for (i, l) in input.content.iter().enumerate() {
            if l.is_blank() {
                resolved.push((prev_depth, Vec::new()));
                continue;
            }
            // `LineBlock.run` (misc.py): `inline_text(line_text.strip())`
            // and `line.indent = len(line_text) - len(line_text.lstrip())`
            // — Python's set for both, so a leading NBSP or `\x1f` is
            // INDENT, not text (round F, `dir_body` pins).
            let raw = self.sources.line_text(*l);
            let depth = raw
                .chars()
                .take_while(|c| crate::utils::py_isspace(*c))
                .count();
            prev_depth = depth;
            let text = raw.trim_matches(crate::utils::py_isspace).to_string();
            let inline = self.inline(&text, input.span, first_line + i as u32);
            lb_messages.extend(inline.messages);
            resolved.push((depth, inline.nodes));
        }
        let mut block = build_line_block(&mut resolved, input.span, 0);
        if let Some(OptVal::StrList(classes)) = opt_get(&input.options, "class") {
            block.attrs.classes.extend(classes.iter().cloned());
        }
        block.attrs.ids = block_names.attrs.ids;
        block.attrs.names = block_names.attrs.names;
        block.attrs.dupnames = block_names.attrs.dupnames;
        out.push(block);
        out.append(&mut lb_messages);
    }

    /// class (misc.py:434-469): with content, classes apply directly to
    /// every top-level child; without, a pending node is emitted for the
    /// ClassAttribute transform.
    fn run_class(&mut self, input: DirectiveInput<'_>, out: &mut Vec<Node>) {
        let class_values = match convert_option(Conv::ClassOption, Some(&input.arguments[0])) {
            Ok(OptVal::StrList(list)) => list,
            _ => {
                out.push(self.directive_run_error(
                    &format!(
                        "Invalid class attribute value for \"{}\" directive: \"{}\".",
                        input.name, input.arguments[0]
                    ),
                    input.span.source,
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
        lines: &[LineRec],
        pos: &mut usize,
        rest: &str,
        out: &mut Vec<Node>,
        construct_error: &mut Option<Node>,
    ) -> bool {
        let start = *pos;
        let marker_rec = lines[start];
        let lineno = marker_rec.lineno;
        let msg_source = marker_rec.source;
        let (block, consumed, _indent, _term) = indented_block(lines, start + 1);
        let span = self.span_of(lines, start, start + consumed);
        // blocktext for message literals: the raw marker line + raw block
        // (trailing blanks already trimmed by indented_block).
        let mut blocktext = self.sources.line_text(marker_rec).to_string();
        for l in &lines[start + 1..start + 1 + consumed] {
            blocktext.push('\n');
            blocktext.push_str(self.sources.line_text(*l));
        }

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
                    msg_source,
                    lineno,
                ));
                return false;
            }
            acc.push(' ');
            acc.push_str(self.sources.line_text(block[used]).trim());
            used += 1;
        };
        *pos = start + 1 + consumed;
        // Remainder after the marker lives on ONE physical line: `rest`
        // when the marker was single-line, else the last joined block line.
        let (rem_rec, rem_lineno): (LineRec, u32) = if used == 0 {
            // `rest` is the marker line's text past `.. ` and its leading
            // spaces; recover its byte offset within the line to re-wrap.
            let rest_offset = self.sources.line_text(marker_rec).len() - rest.len();
            (
                self.rewrap_from(marker_rec, rest_offset + marker.remainder_start),
                lineno,
            )
        } else {
            let last = block[used - 1];
            let last_text = self.sources.line_text(last);
            let trimmed = last_text.trim();
            let seg_start = acc.len() - trimmed.len();
            let within = marker.remainder_start.saturating_sub(seg_start);
            let base = last_text.len() - last_text.trim_start().len();
            (self.rewrap_from(last, base + within), last.lineno)
        };
        let mut content_block: Vec<LineRec> = block[used..].to_vec();

        let subname_ws = ids::whitespace_normalize_name(&marker.name);
        // Missing contents (states.py:2168-2176).
        if self.sources.line_text(rem_rec).trim().is_empty()
            && content_block.iter().all(|l| l.is_blank())
        {
            out.push(self.msg_literal(
                messages::WARNING,
                &format!(
                    "Substitution definition \"{}\" missing contents.",
                    marker.name
                ),
                msg_source,
                lineno,
                &blocktext,
            ));
            self.warn_explicit_markup_end(lines, *pos, out);
            return true;
        }

        let mut subst = Node::elem("substitution_definition", span);
        subst.attrs.names.push(subname_ws.clone());
        // `nodes.substitution_definition(blocktext)` (`states.py:2169`).
        subst.set(crate::doctree::RAWSOURCE, AttrValue::Str(blocktext.clone()));

        // Locate the embedded-directive line: the marker remainder, else
        // the first non-blank content line (hanging-indent form).
        // raw_content_from tracks the BLOCK index where the directive's
        // continuation lines begin, for rawsource reconstruction with
        // original indentation (docutils strip_indent=False).
        let mut raw_content_from = used;
        let (dline, dlineno) = if !self.sources.line_text(rem_rec).trim().is_empty() {
            let rem_text = self.sources.line_text(rem_rec);
            let spaces = rem_text.len() - rem_text.trim_start_matches(' ').len();
            (self.rewrap_from(rem_rec, spaces), rem_lineno)
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
        let dline_src = self.sources.arc(dline.source);
        let dline_text = dline.slice(&dline_src);
        if let Some((dname, dfirst_rest)) = match_embedded_directive(dline_text) {
            let dblock = dedent_by_min(&content_block);
            // rawsource with ORIGINAL indentation (the nested state
            // machine's lines are strip_indent=False; fixture-verified).
            let embedded_raw = {
                let mut raw = dline_text.to_string();
                for l in &lines[start + 1 + raw_content_from..start + 1 + consumed] {
                    raw.push('\n');
                    raw.push_str(self.sources.line_text(*l));
                }
                raw
            };
            let dfirst = {
                let t = dfirst_rest.trim_start_matches(' ');
                let offset = dline_text.len() - t.len();
                self.rewrap_from(dline, offset)
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
            // The literal is passed at creation; the block quote of the
            // offending content is appended after (`msg.append(...)`), so
            // the stream prints the first two only.
            let mut msg = self.msg_literal(
                messages::ERROR,
                "Problematic content in substitution definition",
                msg_source,
                lineno,
                &blocktext,
            );
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
            out.push(self.msg_literal(
                messages::ERROR,
                &format!("{phrase} are not supported in a substitution definition."),
                msg_source,
                lineno,
                &blocktext,
            ));
            self.warn_explicit_markup_end(lines, *pos, out);
            return true;
        }
        // Empty or invalid (states.py:2203-2210).
        if subst.children.is_empty() {
            out.push(self.msg_literal(
                messages::WARNING,
                &format!(
                    "Substitution definition \"{}\" empty or invalid.",
                    marker.name
                ),
                msg_source,
                lineno,
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
                msg_source,
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
        lines: &[LineRec],
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
            link.push_str(self.sources.line_text(*l).trim());
        }
        *pos = start + 1 + consumed;
        // `until_blank=True` (`states.py:2532-2533`).
        self.note_blank_left_to_top(lines, *pos);
        let mut target = Node::elem(kinds::TARGET, span);
        target.set("anonymous", AttrValue::Int(1));
        // `blocktext = match.string[:match.end()] + '\n'.join(block)`
        // (`anonymous_target`, `states.py:2530-2538`): the `__ ` line, then
        // the continuation lines with their common indentation stripped.
        let continuation = &lines[start + 1..start + 1 + consumed];
        let dedent = continuation.iter().map(|l| l.indent()).min().unwrap_or(0);
        let mut blocktext = self.sources.line_text(lines[start]).to_string();
        for l in continuation {
            blocktext.push('\n');
            blocktext.push_str(&self.sources.line_text(*l)[dedent..]);
        }
        target.set(crate::doctree::RAWSOURCE, AttrValue::Str(blocktext));
        if !link.is_empty() {
            // `anonymous_target` (states.py:2530-2537) hands the escaped
            // block straight to `make_target(..., '')`.
            let block: Vec<Vec<char>> = crate::utils::py_splitlines(&link)
                .into_iter()
                .map(escape2null_chars)
                .collect();
            match parse_target_block(&block) {
                TargetRef::RefName(data) => {
                    target.set("refname", AttrValue::Str(ids::fully_normalize_name(&data)));
                }
                TargetRef::RefUri(uri) => {
                    target.set("refuri", AttrValue::Str(uri));
                }
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
    // `option_marker`'s `(  +| ?$)` (states.py:1250) eats spaces only and the
    // description is `line[match.end():]` (`:1641`), so an NBSP after the gap
    // is description text (round F, pinned in `round_f`).
    let desc = text[marker_end..].trim_start_matches(' ');

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
    let (start, end) = display_range(text, from, to);
    &text[start..end]
}

/// The byte range [`display_slice`] would take — for carving a `LineRec`
/// sub-view rather than a borrowed slice.
fn display_range(text: &str, from: usize, to: usize) -> (usize, usize) {
    let index = display_byte_index(text);
    let n = index.len() - 1;
    let start = index[from.min(n)];
    let end = index[to.min(n)];
    if start >= end {
        (start, start)
    } else {
        (start, end)
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
    /// The `py:*` object-description family (`sphinx/domains/python`),
    /// running through the same `ObjectDescription.run` anatomy with the
    /// py-domain `handle_signature`/`add_target_and_index`/`before_content`
    /// overrides ([PY §1.3-1.5, §7]).
    PyObjectDesc(PyDirective),
    /// `.. py:module::` — a plain `SphinxDirective`, NOT an
    /// ObjectDescription (`domains/python/__init__.py:473-536`).
    PyModule,
    /// `.. py:currentmodule::` — pure ref_context state, emits nothing
    /// (`__init__.py:539-556`).
    PyCurrentModule,
    /// `.. include::` (`DU/parsers/rst/directives/misc.py:42-267`; the
    /// sphinx override only rewrites the path and records env state).
    Include,
    /// `.. literalinclude::` (`SP/directives/code.py:413-506`): a
    /// SIBLING of include — it produces a literal_block node, never a
    /// splice.
    LiteralInclude,
    /// `.. program::` (`domains/std/__init__.py:333-348`).
    ProgramDir,
    /// `.. default-domain::` (`directives/__init__.py:353-366`).
    DefaultDomainDir,
    /// Test-only exercise of the [`SpliceRequest`] channel: content lines
    /// become a spliced source named by the argument. Kept alongside the
    /// real producer ([`Self::Include`], wave-4.5 task 12) because it
    /// drives the channel without touching the filesystem.
    #[cfg(test)]
    TestSplice,
}

/// Which Python object a `py:*` object-description directive describes.
/// Values are the sphinx directive classes AFTER name aliasing [PY §1.1]:
/// `py:classmethod`/`py:staticmethod`/`py:decoratormethod` are `Method`,
/// `py:decorator` is `Function` — their `run()` rewrites `self.name`
/// before the base run partitions it, so the desc's objtype is the
/// aliased kind's (trap 13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyObjectKind {
    Function,
    Data,
    Class,
    Exception,
    Method,
    Attribute,
    Property,
    TypeAlias,
}

impl PyObjectKind {
    /// The desc `objtype`/`desctype` string (also the second desc class).
    fn objtype(self) -> &'static str {
        match self {
            PyObjectKind::Function => "function",
            PyObjectKind::Data => "data",
            PyObjectKind::Class => "class",
            PyObjectKind::Exception => "exception",
            PyObjectKind::Method => "method",
            PyObjectKind::Attribute => "attribute",
            PyObjectKind::Property => "property",
            PyObjectKind::TypeAlias => "type",
        }
    }
}

/// One `py:*` object directive after spec-lookup-time aliasing: the
/// [`PyObjectKind`] plus what the aliasing directives' `run()` injects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PyDirective {
    kind: PyObjectKind,
    /// `py:classmethod`/`py:staticmethod` inject `options['classmethod']`
    /// / `options['staticmethod']` in `run()` (`__init__.py:287-303`)
    /// rather than accepting the flag as an option (their option_spec is
    /// the plain `PyObject` copy).
    injected: Option<&'static str>,
    /// `py:decorator`/`py:decoratormethod`: `needs_arglist()` forced off
    /// and a leading `desc_addname('@')` (`__init__.py:116-130`, `306-319`).
    decorator: bool,
}

impl PyDirective {
    /// `needs_arglist()`: True only for PyFunction and PyMethod
    /// (`__init__.py:92-93`, `230-231`); decorators override it back to
    /// False (`:129-130`, `:318-319`).
    fn needs_arglist(self) -> bool {
        matches!(self.kind, PyObjectKind::Function | PyObjectKind::Method) && !self.decorator
    }

    /// `allow_nesting`: PyClasslike only (`__init__.py:186`).
    fn allow_nesting(self) -> bool {
        matches!(self.kind, PyObjectKind::Class | PyObjectKind::Exception)
    }
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

/// The `run_object_description` dispatch: which family's overrides run on
/// top of the shared `ObjectDescription.run` anatomy.
#[derive(Clone, Copy, PartialEq)]
enum DescDispatch {
    Std(ObjectDescKind),
    Py(PyDirective),
}

/// The option names one directive's parse-time spec admits, in spec order.
///
/// Exists for the validator drift audit in
/// `src/directives/validation/builtin.rs`: the `*_OPTS` tables here are
/// this crate's probe-verified transcription of the real docutils/sphinx
/// `option_spec`s, so they are the right oracle for what a validator may
/// call an "Unknown option". Task 14 found the two lists had drifted and
/// the build warned about `:lines:` on a `literalinclude`, a diagnostic
/// Sphinx has no counterpart for.
#[cfg(test)]
pub(crate) fn directive_option_names(name: &str) -> Option<Vec<&'static str>> {
    directive_spec_mode(name, true).map(|spec| spec.option_spec.iter().map(|(n, _)| *n).collect())
}

/// Sphinx-mode registry: overlays/extends the docutils-native table.
fn directive_spec_mode(lower: &str, sphinx: bool) -> Option<DirectiveSpec> {
    #[cfg(test)]
    if lower == "sphinx-ultra-test-splice" {
        return Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: true,
            option_spec: &[],
            kind: DirectiveKind::TestSplice,
        });
    }
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

/// The literalinclude option spec (`SP/directives/code.py:423-445`).
/// `caption` is `unchanged` — the EMPTY value is meaningful; `lineno-start`
/// and `tab-width` are plain Python `int` (negatives allowed).
const LITERALINCLUDE_OPTS: &[(&str, Conv)] = &[
    ("dedent", Conv::OptionalInt),
    ("linenos", Conv::Flag),
    ("lineno-start", Conv::PyIntAny),
    ("lineno-match", Conv::Flag),
    ("tab-width", Conv::PyIntAny),
    ("language", Conv::UnchangedRequired),
    ("force", Conv::Flag),
    ("encoding", Conv::Encoding),
    ("pyobject", Conv::UnchangedRequired),
    ("lines", Conv::UnchangedRequired),
    ("start-after", Conv::UnchangedRequired),
    ("end-before", Conv::UnchangedRequired),
    ("start-at", Conv::UnchangedRequired),
    ("end-at", Conv::UnchangedRequired),
    ("prepend", Conv::UnchangedRequired),
    ("append", Conv::UnchangedRequired),
    ("emphasize-lines", Conv::UnchangedRequired),
    ("caption", Conv::Unchanged),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
    ("diff", Conv::UnchangedRequired),
];

/// sphinx.util.parselinenos: 1-based spec ('1,3-5', open ends '-4'/'4-')
/// against `nlines` total lines; invalid or reversed specs raise. Range
/// materialization is clamped to nlines so a huge upper bound cannot
/// blow memory (values past nlines are filtered anyway).
fn parse_linenos(spec: &str, nlines: i64) -> Result<Vec<i64>, String> {
    let invalid = || format!("invalid line number spec: {}", py_repr(Some(spec)));
    let mut out = Vec::new();
    for part in spec.split(',') {
        // `begend = part.strip().split('-')` — Python's set, BEFORE `int()`
        // gets to reject a `\x1f` (round F pins: `\x1f1`, `1\x1f,2`). The
        // inner `a`/`b` trims below stand in for `int()`'s own White_Space
        // strip, which is exactly Rust's set.
        let part = part.trim_matches(crate::utils::py_isspace);
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
        "literalinclude" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: false,
            option_spec: LITERALINCLUDE_OPTS,
            kind: DirectiveKind::LiteralInclude,
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
        // `PythonDomain.directives` (`domains/python/__init__.py:739-754`)
        // with the run()-time name aliasing resolved at spec-lookup time
        // [PY §1.1].
        "py:function" => Some(py_object_desc_spec(PyObjectKind::Function, None, false)),
        "py:data" => Some(py_object_desc_spec(PyObjectKind::Data, None, false)),
        "py:class" => Some(py_object_desc_spec(PyObjectKind::Class, None, false)),
        "py:exception" => Some(py_object_desc_spec(PyObjectKind::Exception, None, false)),
        "py:method" => Some(py_object_desc_spec(PyObjectKind::Method, None, false)),
        "py:classmethod" => Some(py_object_desc_spec(
            PyObjectKind::Method,
            Some("classmethod"),
            false,
        )),
        "py:staticmethod" => Some(py_object_desc_spec(
            PyObjectKind::Method,
            Some("staticmethod"),
            false,
        )),
        "py:attribute" => Some(py_object_desc_spec(PyObjectKind::Attribute, None, false)),
        "py:property" => Some(py_object_desc_spec(PyObjectKind::Property, None, false)),
        "py:type" => Some(py_object_desc_spec(PyObjectKind::TypeAlias, None, false)),
        "py:decorator" => Some(py_object_desc_spec(PyObjectKind::Function, None, true)),
        "py:decoratormethod" => Some(py_object_desc_spec(PyObjectKind::Method, None, true)),
        "py:module" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: true,
            option_spec: PY_MODULE_OPTS,
            kind: DirectiveKind::PyModule,
        }),
        "py:currentmodule" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: false,
            has_content: false,
            option_spec: &[],
            kind: DirectiveKind::PyCurrentModule,
        }),
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

/// `PyObject.option_spec` (`domains/python/_object.py:172-185`) plus each
/// subclass's additions ([PY §1.2]). The macro keeps the shared twelve in
/// one place.
macro_rules! py_object_opts {
    ($($extra:tt)*) => {
        &[
            ("no-index", Conv::Flag),
            ("no-index-entry", Conv::Flag),
            ("no-contents-entry", Conv::Flag),
            ("no-typesetting", Conv::Flag),
            ("noindex", Conv::Flag),
            ("noindexentry", Conv::Flag),
            ("nocontentsentry", Conv::Flag),
            ("single-line-parameter-list", Conv::Flag),
            ("single-line-type-parameter-list", Conv::Flag),
            ("module", Conv::Unchanged),
            ("canonical", Conv::Unchanged),
            ("annotation", Conv::Unchanged),
            $($extra)*
        ]
    };
}

const PY_OBJECT_OPTS: &[(&str, Conv)] = py_object_opts!();
const PY_FUNCTION_OPTS: &[(&str, Conv)] = py_object_opts!(("async", Conv::Flag),);
const PY_VARIABLE_OPTS: &[(&str, Conv)] =
    py_object_opts!(("type", Conv::Unchanged), ("value", Conv::Unchanged),);
const PY_CLASSLIKE_OPTS: &[(&str, Conv)] =
    py_object_opts!(("abstract", Conv::Flag), ("final", Conv::Flag),);
const PY_METHOD_OPTS: &[(&str, Conv)] = py_object_opts!(
    ("abstract", Conv::Flag),
    ("abstractmethod", Conv::Flag),
    ("async", Conv::Flag),
    ("classmethod", Conv::Flag),
    ("final", Conv::Flag),
    ("staticmethod", Conv::Flag),
);
const PY_PROPERTY_OPTS: &[(&str, Conv)] = py_object_opts!(
    ("abstract", Conv::Flag),
    ("abstractmethod", Conv::Flag),
    ("classmethod", Conv::Flag),
    ("type", Conv::Unchanged),
);

/// `PyModule.option_spec` (`__init__.py:480-490`): note **no
/// `noindexentry`** old spelling (probe `module_bad_option`: it is the
/// unknown-option error), and `no-typesetting` is accepted but unused by
/// `PyModule.run` (probe `module_no_typesetting`: inert). `platform`/
/// `synopsis` are identity lambdas in sphinx — `Conv::Unchanged` differs
/// only for a bare valueless option (`''` here vs Python `None`, probe
/// `module_synopsis_bare`), which the record keeps as `''`.
const PY_MODULE_OPTS: &[(&str, Conv)] = &[
    ("platform", Conv::Unchanged),
    ("synopsis", Conv::Unchanged),
    ("no-index", Conv::Flag),
    ("no-index-entry", Conv::Flag),
    ("no-contents-entry", Conv::Flag),
    ("no-typesetting", Conv::Flag),
    ("noindex", Conv::Flag),
    ("nocontentsentry", Conv::Flag),
    ("deprecated", Conv::Flag),
];

/// The `py:*` object-description directives share `ObjectDescription`'s
/// class-level shape ([`object_desc_spec`]); the option spec is the
/// subclass's — with `py:classmethod`/`py:staticmethod` RESET to the plain
/// `PyObject.option_spec.copy()` (`__init__.py:285`, `:297`): their flags
/// arrive via [`PyDirective::injected`], not as options.
fn py_object_desc_spec(
    kind: PyObjectKind,
    injected: Option<&'static str>,
    decorator: bool,
) -> DirectiveSpec {
    let option_spec: &'static [(&'static str, Conv)] = if injected.is_some() {
        PY_OBJECT_OPTS
    } else {
        match kind {
            PyObjectKind::Function => PY_FUNCTION_OPTS,
            PyObjectKind::Data | PyObjectKind::Attribute => PY_VARIABLE_OPTS,
            PyObjectKind::Class | PyObjectKind::Exception => PY_CLASSLIKE_OPTS,
            PyObjectKind::Method => PY_METHOD_OPTS,
            PyObjectKind::Property => PY_PROPERTY_OPTS,
            // PyTypeAlias re-declares `canonical`, which the base set
            // already carries with the same conversion (`__init__.py:436-439`).
            PyObjectKind::TypeAlias => PY_OBJECT_OPTS,
        }
    };
    DirectiveSpec {
        required_arguments: 1,
        optional_arguments: 0,
        final_argument_whitespace: true,
        has_content: true,
        option_spec,
        kind: DirectiveKind::PyObjectDesc(PyDirective {
            kind,
            injected,
            decorator,
        }),
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
/// Python's `\s` is `str.isspace` — `\x1c`-`\x1f` included, which Rust's
/// `char::is_whitespace` leaves out ([`crate::utils::py_isspace`]; probed:
/// `.. envvar:: FOO\x1fBAR` indexes `environment variable; FOO BAR`,
/// `.. program:: git\x1fadd` scopes its options under `git-add`).
fn ws_collapse(s: &str, repl: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if crate::utils::py_isspace(c) {
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

/// `ObjectDescription.get_signatures` (`directives/__init__.py:88-98`):
/// backslash-newline pairs vanish (`nl_escape_re`), then one stripped
/// signature per line — each put through `strip_backslash_re.sub(r'\1', …)`
/// when `strip_signature_backslash` is on (probe strip_backslash_on:
/// `f(a\_b)` documents parameter `a_b`).
fn object_signatures(argument: &str, strip_signature_backslash: bool) -> Vec<String> {
    argument
        .replace("\\\n", "")
        .split('\n')
        .map(|line| {
            // `get_signatures`: `line.strip()` per line — Python's set
            // (round F pin: a second signature opening with `\x1f`).
            let line = line.trim_matches(crate::utils::py_isspace);
            if strip_signature_backslash {
                strip_backslashes(line)
            } else {
                line.to_string()
            }
        })
        .collect()
}

/// `strip_backslash_re.sub(r'\1', line)` — `\\(.)`: every backslash
/// followed by a character is removed keeping the character; a lone
/// trailing backslash has no `.` to consume and survives.
fn strip_backslashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next) => out.push(next),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `py_sig_re` (`domains/python/_object.py:41-50`) as a [`PySigMatch`]:
/// groups (prefix, name, tp_list, arglist, retann) plus the byte spans of
/// groups 3/4 that the multi-line measurement subtracts. The regex crate's
/// leftmost-first captures match Python's backtracking on this pattern
/// (pinned by the py_sig_match tests, incl. the greedy-arglist edge).
fn py_sig_match(sig: &str) -> Option<crate::py::arglist::PySigMatch> {
    lazy_static::lazy_static! {
        static ref PY_SIG_RE: regex::Regex = regex::Regex::new(
            r"(?x)^ ([\w.]*\.)?               # class name(s)
                  (\w+) \s*                   # thing name
                  (?: \[ \s* (.*?) \s* \] )?  # optional: type parameters list
                  (?: \( \s* (.*) \s* \)      # optional: arguments
                   (?: \s* ->\s* (.*) )?      #           return annotation
                  )? $",
        )
        .expect("py_sig_re compiles");
    }
    let caps = PY_SIG_RE.captures(sig)?;
    let group = |i: usize| caps.get(i).map(|m| m.as_str().to_string());
    let span_of = |i: usize| caps.get(i).map(|m| (m.start(), m.end())).unwrap_or((0, 0));
    Some(crate::py::arglist::PySigMatch {
        prefix: group(1),
        name: group(2).unwrap_or_default(),
        tp_list: group(3),
        arglist: group(4),
        retann: group(5),
        tp_span: span_of(3),
        arg_span: span_of(4),
    })
}

/// `get_signature_prefix` per py kind (`__init__.py`, [PY §1.3]): the
/// keyword set in FIXED order, each keyword a `desc_sig_keyword` +
/// `desc_sig_space` pair. Note `staticmethod` prints keyword `static`.
fn py_signature_prefix(py: PyDirective, input: &DirectiveInput<'_>) -> Vec<Node> {
    let has = |n: &'static str| opt_get(&input.options, n).is_some() || py.injected == Some(n);
    let mut words: Vec<&str> = Vec::new();
    match py.kind {
        PyObjectKind::Function => {
            if has("async") {
                words.push("async");
            }
        }
        PyObjectKind::Class | PyObjectKind::Exception => {
            if has("final") {
                words.push("final");
            }
            if has("abstract") {
                words.push("abstract");
            }
            words.push(py.kind.objtype());
        }
        PyObjectKind::Method => {
            if has("final") {
                words.push("final");
            }
            if has("abstract") || has("abstractmethod") {
                words.push("abstractmethod");
            }
            if has("async") {
                words.push("async");
            }
            if has("classmethod") {
                words.push("classmethod");
            }
            if has("staticmethod") {
                words.push("static");
            }
        }
        PyObjectKind::Property => {
            if has("abstract") || has("abstractmethod") {
                words.push("abstract");
            }
            if has("classmethod") {
                words.push("class");
            }
            words.push("property");
        }
        PyObjectKind::TypeAlias => words.push("type"),
        PyObjectKind::Data | PyObjectKind::Attribute => {}
    }
    words
        .iter()
        .flat_map(|word| {
            [
                crate::py::annotations::desc_sig_keyword(word),
                crate::py::annotations::desc_sig_space(),
            ]
        })
        .collect()
}

/// `get_index_text` per py kind ([PY §1.4]); PyFunction returns `''` and
/// adds its entry in its own `add_target_and_index` instead.
fn py_index_text(
    py: PyDirective,
    input: &DirectiveInput<'_>,
    modname: Option<&str>,
    name: &str,
    add_module_names: bool,
) -> String {
    let has = |n: &'static str| opt_get(&input.options, n).is_some() || py.injected == Some(n);
    // `clsname, attrname = name.rsplit('.', 1)` with the add_module_names
    // qualification (`__init__.py:262-279` and friends).
    let split = |name: &str| -> Option<(String, String)> {
        let (cls, last) = name.rsplit_once('.')?;
        let cls = match modname {
            Some(modname) if add_module_names => format!("{modname}.{cls}"),
            _ => cls.to_string(),
        };
        Some((cls, last.to_string()))
    };
    match py.kind {
        PyObjectKind::Function => String::new(),
        PyObjectKind::Data => match modname {
            Some(modname) => format!("{name} (in module {modname})"),
            None => format!("{name} (built-in variable)"),
        },
        PyObjectKind::Class => match modname {
            Some(modname) => format!("{name} (class in {modname})"),
            None => format!("{name} (built-in class)"),
        },
        // Exception index entries are the bare name (trap 10).
        PyObjectKind::Exception => name.to_string(),
        PyObjectKind::Method => match split(name) {
            Some((cls, meth)) => {
                if has("classmethod") {
                    format!("{meth}() ({cls} class method)")
                } else if has("staticmethod") {
                    format!("{meth}() ({cls} static method)")
                } else {
                    format!("{meth}() ({cls} method)")
                }
            }
            None => match modname {
                Some(modname) => format!("{name}() (in module {modname})"),
                None => format!("{name}()"),
            },
        },
        PyObjectKind::Attribute => match split(name) {
            Some((cls, attr)) => format!("{attr} ({cls} attribute)"),
            None => match modname {
                Some(modname) => format!("{name} (in module {modname})"),
                None => name.to_string(),
            },
        },
        PyObjectKind::Property => match split(name) {
            Some((cls, attr)) => format!("{attr} ({cls} property)"),
            None => match modname {
                Some(modname) => format!("{name} (in module {modname})"),
                None => name.to_string(),
            },
        },
        PyObjectKind::TypeAlias => match split(name) {
            Some((cls, attr)) => format!("{attr} (type alias in {cls})"),
            None => match modname {
                Some(modname) => format!("{name} (in module {modname})"),
                None => name.to_string(),
            },
        },
    }
}

/// `PyObject._toc_entry_name` (`_object.py:505-522`): parens for the
/// callable objtypes iff `add_function_parentheses`, then the
/// `toc_object_entries_show_parents` shape (unknown values fall through to
/// `''`, matching the un-handled `return` path).
fn py_toc_entry_name(
    parts: &[String],
    fullname: &str,
    kind: PyObjectKind,
    cfg: &crate::py::PySigConfig,
) -> String {
    let Some(last) = parts.last() else {
        return String::new();
    };
    let callable = matches!(kind, PyObjectKind::Function | PyObjectKind::Method);
    let parens = if cfg.add_function_parentheses && callable {
        "()"
    } else {
        ""
    };
    match cfg.toc_object_entries_show_parents.as_str() {
        "domain" => format!("{fullname}{parens}"),
        "hide" => format!("{last}{parens}"),
        "all" => {
            let mut joined = parts[..parts.len() - 1].to_vec();
            joined.push(format!("{last}{parens}"));
            joined.join(".")
        }
        _ => String::new(),
    }
}

/// Python tuple-repr of a string sequence: `()`, `('a',)`, `('a', 'b')` —
/// the `_toc_parts` pformat shape.
fn py_tuple_repr(parts: &[String]) -> String {
    match parts {
        [] => "()".to_string(),
        [one] => format!("({},)", py_repr(Some(one))),
        _ => format!(
            "({})",
            parts
                .iter()
                .map(|part| py_repr(Some(part)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `addnodes.desc_annotation` — a `FixedTextElement` (xml:space preserve)
/// with no extra classes; also the shape of `desc_returns`.
fn desc_annotation_node(span: Span) -> Node {
    let mut node = Node::elem("desc_annotation", span);
    node.set("xml:space", AttrValue::Str("preserve".to_string()));
    node
}

// ====================================================================
// Doc-field transformation (M2 wave 4.5 task 7): the
// `sphinx.util.docfields.DocFieldTransformer` port, run as a
// desc_content post-pass for EVERY object-description kind
// [PY §1.6 "Doc fields"].
//
// The banner used to say "py object kinds only", which the task-7 review
// adjudicated as a plan defect: `ObjectDescription.run` applies the
// transformer unconditionally, so an `envvar`'s `:param x:` really does
// render as "Param x" and a `:meta:` field really is NOT filtered on the
// std side. The py/std split lives entirely in the TYPE MAP handed in
// ([`py_field_type_map`] vs the empty [`std_field_type_map`]), not in
// whether the pass runs.
// ====================================================================

/// One `PyObject.doc_field_types` entry. The five entries mirror
/// `_object.py:187-232` exactly (names/typenames/labels/roles verified
/// against the 9.1.0 source dump).
/// Sphinx keys these by `Field.name` (`parameter`/`variable`/
/// `exceptions`/`returnvalue`/`returntype`); the port keys the grouped
/// entries and the `types` map by [`PY_DOC_FIELDS`] index instead.
struct PyDocField {
    /// The rendered `field_name` label.
    label: &'static str,
    /// `GroupedField`: every occurrence collects into ONE field.
    is_grouped: bool,
    /// `TypedField`: `:type x:` companions and `:param type name:` syntax.
    is_typed: bool,
    /// Single-item groups collapse to a bare paragraph (no bullet_list).
    can_collapse: bool,
    /// Whether the field REQUIRES an argument (`Field.has_arg`); a
    /// mismatch in either direction demotes the field to unknown.
    has_arg: bool,
    /// Role for the field-argument xrefs (only raises' `exc`).
    rolename: &'static str,
    /// Role for typed fields' type xrefs (`class`).
    typerolename: &'static str,
    /// Role for a single-text BODY (only rtype's `class`).
    bodyrolename: &'static str,
    /// Whether the field class carries `PyXrefMixin` — `returnvalue` is a
    /// plain `docfields.Field`, everything else is a `Py*Field`. The mixin
    /// is what splits multi-type strings and stamps the py attrs.
    py_xref: bool,
}

/// Indices into [`PY_DOC_FIELDS`].
const PY_FIELD_PARAMETER: usize = 0;
const PY_FIELD_VARIABLE: usize = 1;
const PY_FIELD_EXCEPTIONS: usize = 2;
const PY_FIELD_RETURNVALUE: usize = 3;
const PY_FIELD_RETURNTYPE: usize = 4;

const PY_DOC_FIELDS: [PyDocField; 5] = [
    PyDocField {
        label: "Parameters",
        is_grouped: true,
        is_typed: true,
        can_collapse: true,
        has_arg: true,
        rolename: "",
        typerolename: "class",
        bodyrolename: "",
        py_xref: true,
    },
    PyDocField {
        label: "Variables",
        is_grouped: true,
        is_typed: true,
        can_collapse: true,
        has_arg: true,
        rolename: "",
        typerolename: "class",
        bodyrolename: "",
        py_xref: true,
    },
    PyDocField {
        label: "Raises",
        is_grouped: true,
        is_typed: false,
        can_collapse: true,
        has_arg: true,
        rolename: "exc",
        typerolename: "",
        bodyrolename: "",
        py_xref: true,
    },
    PyDocField {
        label: "Returns",
        is_grouped: false,
        is_typed: false,
        can_collapse: false,
        has_arg: false,
        rolename: "",
        typerolename: "",
        bodyrolename: "",
        py_xref: false,
    },
    PyDocField {
        label: "Return type",
        is_grouped: false,
        is_typed: false,
        can_collapse: false,
        has_arg: false,
        rolename: "",
        typerolename: "",
        bodyrolename: "class",
        py_xref: true,
    },
];

/// A directive's `get_field_type_map()` lookup: field-name ->
/// `(doc_field_types index, is_typefield)`.
type DocFieldTypeMap = fn(&str) -> Option<(usize, bool)>;

/// `get_field_type_map()` for the std object-description kinds
/// (Describe/EnvVar/Confval/Cmdoption): none of them declare
/// `doc_field_types`, so the map is empty and every field takes the
/// transformer's unknown branch — capitalized name, body passed through
/// (oracle probes envvar_param/describe_param/option_param/
/// confval_type_and_field/envvar_multi_fields).
fn std_field_type_map(_name: &str) -> Option<(usize, bool)> {
    None
}

/// `ObjectDescription.get_field_type_map()` for the py set: field-name ->
/// `(doc_field_types index, is_typefield)`.
fn py_field_type_map(name: &str) -> Option<(usize, bool)> {
    Some(match name {
        "param" | "parameter" | "arg" | "argument" | "keyword" | "kwarg" | "kwparam" => {
            (PY_FIELD_PARAMETER, false)
        }
        "paramtype" | "type" => (PY_FIELD_PARAMETER, true),
        "var" | "ivar" | "cvar" => (PY_FIELD_VARIABLE, false),
        "vartype" => (PY_FIELD_VARIABLE, true),
        "raises" | "raise" | "exception" | "except" => (PY_FIELD_EXCEPTIONS, false),
        "returns" | "return" => (PY_FIELD_RETURNVALUE, false),
        "rtype" => (PY_FIELD_RETURNTYPE, false),
        _ => return None,
    })
}

/// `filter_meta_fields` (`domains/python/__init__.py:603-617`), fired on
/// the `object-description-transform` event — which the base `run` emits
/// BEFORE `DocFieldTransformer.transform_all` (`directives/__init__.py`),
/// so `:meta:` fields vanish from the raw list and the transformer then
/// replaces the emptied list with an empty `<field_list>` that REMAINS
/// [PY §1.6 meta probe]. py domain only (the event handler checks).
fn filter_meta_fields(content: &mut Node) {
    for child in &mut content.children {
        if child.kind == kinds::FIELD_LIST {
            child.children.retain(|field| {
                if field.kind != kinds::FIELD {
                    return true;
                }
                let name = field.children.first().map(Node::astext).unwrap_or_default();
                // `field[0].astext().strip()` — Python's set (round F pin).
                let name = name.trim_matches(crate::utils::py_isspace);
                !(name == "meta" || name.starts_with("meta "))
            });
        }
    }
}

/// `_is_single_paragraph` (`docfields.py:34-42`): exactly one paragraph,
/// tolerating trailing system_messages.
fn is_single_field_paragraph(field_body: &Node) -> bool {
    if field_body.children.is_empty() {
        return false;
    }
    if field_body.children[1..]
        .iter()
        .any(|n| n.kind != kinds::SYSTEM_MESSAGE)
    {
        return false;
    }
    field_body.children[0].kind == kinds::PARAGRAPH
}

/// Python `str.split(None, maxsplit=1)` on a field name: leading
/// whitespace skipped, the remainder trimmed at its start. A missing or
/// all-whitespace remainder is the `ValueError` path — the ORIGINAL text
/// comes back whole with an empty argument (`docfields.py:384-389`).
fn split_field_name(text: &str) -> (String, String) {
    let trimmed = text.trim_start_matches(crate::utils::py_isspace);
    if let Some(i) = trimmed.find(crate::utils::py_isspace) {
        let rest = trimmed[i..].trim_start_matches(crate::utils::py_isspace);
        if !rest.is_empty() {
            return (trimmed[..i].to_string(), rest.to_string());
        }
    }
    (text.to_string(), String::new())
}

/// Python `fieldarg.rsplit(None, 1)` for the `:param type name:` syntax
/// (`docfields.py:448-455`): `None` is the single-token `ValueError` path.
fn rsplit_field_arg(arg: &str) -> Option<(String, String)> {
    let trimmed = arg.trim_end_matches(crate::utils::py_isspace);
    let (i, ws) = trimmed
        .char_indices()
        .rev()
        .find(|(_, c)| crate::utils::py_isspace(*c))?;
    let head = trimmed[..i].trim_end_matches(crate::utils::py_isspace);
    if head.is_empty() {
        return None;
    }
    Some((head.to_string(), trimmed[i + ws.len_utf8()..].to_string()))
}

/// Python `s[0:1].upper() + s[1:]` (unknown-field renaming).
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// docutils `nodes.Inline` membership for the kinds this parser emits,
/// plus `Text` — the filter typed-field bodies pass through
/// (`docfields.py:442`; block-level nodes would render invalid markup).
fn is_inline_or_text(node: &Node) -> bool {
    matches!(
        node.kind,
        kinds::TEXT
            | "abbreviation"
            | "acronym"
            | "citation_reference"
            | "emphasis"
            | "footnote_reference"
            | "generated"
            | "image"
            | "index"
            | "inline"
            | "literal"
            | "literal_emphasis"
            | "literal_strong"
            | "math"
            | "pending_xref"
            | "problematic"
            | "raw"
            | "reference"
            | "strong"
            | "subscript"
            | "substitution_reference"
            | "superscript"
            | "target"
            | "title_reference"
    )
}

/// `PyXrefMixin._delimiters_re` split with the captured delimiters kept
/// (Python `re.split` with a group) and empties dropped (`filter(None)`).
/// Returns `(piece, is_delimiter)`; a text piece can never start with a
/// delimiter match (the scan would have split there), so the flag is
/// exactly `self._delimiters_re.match(sub_target)` (`_object.py:589`).
fn split_type_delimiters(target: &str) -> Vec<(String, bool)> {
    lazy_static::lazy_static! {
        static ref DELIMITERS_RE: regex::Regex =
            regex::Regex::new(r"\s*[\[\](),](?:\s*o[rf]\s)?\s*|\s+o[rf]\s+|\s*\|\s*|\.\.\.")
                .unwrap();
    }
    let mut out = Vec::new();
    let mut last = 0;
    for m in DELIMITERS_RE.find_iter(target) {
        if m.start() > last {
            out.push((target[last..m.start()].to_string(), false));
        }
        out.push((target[m.start()..m.end()].to_string(), true));
        last = m.end();
    }
    if last < target.len() {
        out.push((target[last..].to_string(), false));
    }
    out
}

/// A `TextElement(rawsource, text)`: element node with one Text child
/// (none when the text is empty, like docutils).
fn doc_field_inline(kind: &'static str, text: &str, span: Span) -> Node {
    let mut node = Node::elem(kind, span);
    if !text.is_empty() {
        node.children.push(Node::text_node(text, span));
    }
    node
}

/// `PyXrefMixin.make_xref` (`_object.py:514-562`) over
/// `Field.make_xref` (`docfields.py:78-119`). The mixin always calls the
/// base with `inliner=None`, so a non-empty rolename ALWAYS yields a
/// `pending_xref` (never the role-run inline), which then gets
/// `refspecific=1`, the `py:module`/`py:class` ref_context attrs, the
/// `parse_reftarget` title rewrite, and — only when title == target — the
/// `python_use_unqualified_type_names` two-condition wrapping with the
/// innernode INSIDE each condition [SIG §4.2 item 2, probes F-U1/F-U2].
/// The directive/environment state every field builder reads: the
/// ref_context slice the xrefs stamp, the py signature config, and the
/// span new nodes carry.
///
/// That span is UNSTAMPED (line 0, the field_list's source): docutils
/// tracks no provenance for the nodes `DocFieldTransformer` builds — the
/// new `field_list`, `field`, `field_body`, `paragraph` and the xrefs in
/// them are all `nodes.X()` constructions with no `source`/`line` — and
/// neither do the `desc_content`/`desc` above them, so a resolution
/// warning on a doc-field xref locates through `get_source_line`'s
/// ancestor walk at the nearest node that IS stamped: the enclosing
/// section (its underline line), admonition, list item, ... — and for a
/// field written inside an `.. include::`, at the INCLUDER's section
/// (probe-pinned, `tests/env_differential.rs`). Stamping the field_list's
/// own line here would report a line sphinx never prints. The resolver's
/// walk (`src/env/resolve.rs`, `resolve_children`) reads a zero line as
/// "inherit from the nearest stamped ancestor".
struct DocFieldEnv<'a> {
    /// The directive's `get_field_type_map()` (py table or the empty std
    /// one).
    map: DocFieldTypeMap,
    ctx: &'a crate::py::annotations::PyRefContext,
    cfg: &'a crate::py::PySigConfig,
    span: Span,
}

fn py_make_doc_xref(
    rolename: &str,
    target: &str,
    innernode: &'static str,
    contnode: Option<Node>,
    env: &DocFieldEnv<'_>,
) -> Node {
    let span = env.span;
    if rolename.is_empty() {
        // `return contnode or innernode(target, target)` — no xref, no
        // mixin post-processing (the result is not a pending_xref).
        return contnode.unwrap_or_else(|| doc_field_inline(innernode, target, span));
    }
    let mut refnode = Node::elem("pending_xref", span);
    refnode.set("refdomain", AttrValue::Str("py".to_string()));
    // Python bools render as 0/1 in pformat (`Element.starttag`).
    refnode.set("refexplicit", AttrValue::Int(0));
    refnode.set("reftype", AttrValue::Str(rolename.to_string()));
    refnode.set("reftarget", AttrValue::Str(target.to_string()));
    refnode
        .children
        .push(contnode.unwrap_or_else(|| doc_field_inline(innernode, target, span)));
    // `PythonDomain.process_field_xref` is a no-op in 9.1.0.

    // PyXrefMixin post-processing (`_object.py:537-562`).
    refnode.set("refspecific", AttrValue::Int(1));
    // ref_context attrs are Python None outside a py scope; pformat
    // renders None as the "True" sentinel (same convention as
    // `crate::py::annotations::type_to_xref`).
    refnode.set(
        "py:module",
        AttrValue::Str(env.ctx.module.clone().unwrap_or_else(|| "True".to_string())),
    );
    refnode.set(
        "py:class",
        AttrValue::Str(env.ctx.class_.clone().unwrap_or_else(|| "True".to_string())),
    );
    let (reftype, reftarget, reftitle, _refspecific) =
        crate::py::annotations::parse_reftarget(target);
    if reftarget != reftitle {
        // `~pkg.Cls` / leading-`.` / `typing.` rewrite — takes precedence
        // over the unqualified-names branch (elif).
        refnode.set("reftype", AttrValue::Str(reftype));
        refnode.set("reftarget", AttrValue::Str(reftarget));
        refnode.children.clear();
        refnode
            .children
            .push(doc_field_inline(innernode, &reftitle, span));
    } else if env.cfg.python_use_unqualified_type_names {
        let children = std::mem::take(&mut refnode.children);
        // `shortname = target.rpartition('.')[-1]`.
        let shortname = target.rsplit('.').next().unwrap_or(target);
        let textnode = doc_field_inline(innernode, shortname, span);
        for (condition, nodes) in [("resolved", vec![textnode]), ("*", children)] {
            let mut cond = Node::elem("pending_xref_condition", span);
            cond.set("condition", AttrValue::Str(condition.to_string()));
            cond.children = nodes;
            refnode.children.push(cond);
        }
    }
    refnode
}

/// `make_xrefs`: `PyXrefMixin.make_xrefs` (`_object.py:568-608`) for the
/// `Py*Field` classes — delimiter split, sticky `Literal[...]`
/// suppression — or the plain single-xref `Field.make_xrefs`
/// (`docfields.py:121-136`) for `returnvalue`.
fn py_make_doc_xrefs(
    spec: &PyDocField,
    rolename: &str,
    target: &str,
    innernode: &'static str,
    contnode: Option<&Node>,
    env: &DocFieldEnv<'_>,
) -> Vec<Node> {
    if !spec.py_xref {
        return vec![py_make_doc_xref(
            rolename,
            target,
            innernode,
            contnode.cloned(),
            env,
        )];
    }
    let split_contnode = contnode.is_some_and(|c| c.astext() == target);
    let mut in_literal = false;
    let mut results = Vec::new();
    for (sub_target, is_delim) in split_type_delimiters(target) {
        let cont: Option<Node> = if split_contnode {
            Some(Node::text_node(sub_target.clone(), env.span))
        } else {
            contnode.cloned()
        };
        if in_literal || is_delim {
            results
                .push(cont.unwrap_or_else(|| doc_field_inline(innernode, &sub_target, env.span)));
        } else {
            results.push(py_make_doc_xref(
                rolename,
                &sub_target,
                innernode,
                cont,
                env,
            ));
        }
        if matches!(
            sub_target.as_str(),
            "Literal" | "typing.Literal" | "~typing.Literal"
        ) {
            in_literal = true;
        }
    }
    results
}

/// One collected entry: a pass-through original field, or a field type
/// with its `(fieldarg, content)` items (grouped types collect many).
enum DocFieldEntry {
    Pass(Node),
    Typed {
        ftype: usize,
        items: Vec<(String, Vec<Node>)>,
    },
}

/// `DocFieldTransformer.transform` for ONE `field_list` node, with the
/// directive's typemap. Sphinx builds a fresh `nodes.field_list` and calls
/// `node.replace_self(new_list)` (`docfields.py:510`); `replace_self`
/// then runs `new_list.update_basic_atts(node)`
/// (`DU/nodes.py:Element.replace_self`), which APPENDS the replaced node's
/// four basic attributes — `ids`, `names`, `classes`, `dupnames` — onto
/// the replacement. Everything else (a `field_list` carries nothing else
/// in practice) is dropped with the old node.
///
/// The port rebuilds the children in place, so the faithful move is to
/// KEEP those four and clear the rest. Reachable, and now for std kinds
/// too: `.. rst-class:: myclass` before a field list inside a
/// `py:function`/`confval` body puts `classes="myclass"` on the field
/// list, and an explicit target before it puts `ids`/`names` there
/// (probes `rst_class_before_field_list_in_py_desc`,
/// `name_target_before_field_list_in_py_desc`). The task-7 review's
/// original premise — that the attributes are dropped — was wrong.
fn transform_doc_field_list(
    node: &mut Node,
    map: DocFieldTypeMap,
    ctx: &crate::py::annotations::PyRefContext,
    cfg: &crate::py::PySigConfig,
) {
    use std::collections::HashMap;
    let env = DocFieldEnv {
        map,
        ctx,
        cfg,
        // Unstamped on purpose — see the struct's doc comment.
        span: Span {
            line: 0,
            ..node.span
        },
    };
    // Sphinx `replace_self`s the parsed field_list with a fresh
    // `nodes.field_list()`: the node that survives here is that new,
    // provenance-less list, not the stamped one the parser built.
    node.span = env.span;
    let fields = std::mem::take(&mut node.children);
    node.attrs = crate::doctree::Attrs {
        ids: std::mem::take(&mut node.attrs.ids),
        names: std::mem::take(&mut node.attrs.names),
        classes: std::mem::take(&mut node.attrs.classes),
        dupnames: std::mem::take(&mut node.attrs.dupnames),
        ..Default::default()
    };

    // Step 1: collect field types and content (`docfields.py:374-482`).
    let mut entries: Vec<DocFieldEntry> = Vec::new();
    let mut group_indices: HashMap<usize, usize> = HashMap::new();
    let mut types: HashMap<usize, HashMap<String, Vec<Node>>> = HashMap::new();
    for field in fields {
        doc_field_step1(field, &mut entries, &mut types, &mut group_indices, &env);
    }

    // Step 2: construct the new field list (`docfields.py:484-510`).
    for entry in entries {
        match entry {
            DocFieldEntry::Pass(field) => node.children.push(field),
            DocFieldEntry::Typed { ftype, items } => {
                let mut empty = HashMap::new();
                let fieldtypes = types.get_mut(&ftype).unwrap_or(&mut empty);
                node.children.push(make_doc_field(
                    &PY_DOC_FIELDS[ftype],
                    items,
                    fieldtypes,
                    &env,
                ));
            }
        }
    }
}

/// `DocFieldTransformer._transform_step_1` (`docfields.py:374-482`),
/// minus the translatable-inline wrapper: sphinx wraps grouped/plain
/// content in `nodes.inline(translatable=True)`, which the
/// `RemoveTranslatableInline(999)` transform splices away again on every
/// untranslated build — the harness3 probes never see it, so the port
/// skips the round-trip.
fn doc_field_step1(
    mut field: Node,
    entries: &mut Vec<DocFieldEntry>,
    types: &mut std::collections::HashMap<usize, std::collections::HashMap<String, Vec<Node>>>,
    group_indices: &mut std::collections::HashMap<usize, usize>,
    env: &DocFieldEnv<'_>,
) {
    // SANCTIONED DIVERGENCE. Sphinx has a bare `assert len(field) == 2`
    // here (`docfields.py:381`), so a `field_list` child that is not a
    // two-child `field` aborts the whole build with an AssertionError;
    // this guard passes such a child through untouched instead. Strictly
    // better than sphinx and unpinnable by the oracle (a crash has no
    // pformat to compare), so it stays a divergence by design.
    //
    // The trigger is REACHABLE, and task 7's repro is the one to use:
    // `.. confval:: t` + `:type: *bad` (likewise `:default: *bad`, and
    // `:type: *a b`). The unterminated emphasis makes docutils drop a
    // one-child `system_message` into the field_list the confval
    // directive generates, `len(field) == 1`, and the assert fires. A
    // task-16 re-probe that reported "builds clean" was reading the
    // harness3 read-phase venue, which never runs `DocFieldTransformer`;
    // re-probed here through a full `SphinxTestApp(buildername='dummy')`
    // + `app.build()`, all three inputs abort with
    // `AssertionError` at `sphinx/util/docfields.py:381`, while
    // `:type: int`, `:type: *bad*` and the same field written in the
    // directive BODY build clean. Hence the EXCLUDED entry
    // `sx_std.confval_bad_type_markup` in tools/gen_sphinx_fixture.py:
    // a crash has no pformat, so the case is unpinnable by the oracle.
    if field.children.len() != 2 {
        entries.push(DocFieldEntry::Pass(field));
        return;
    }
    let name_text = field.children[0].astext();
    let (fieldtype_name, mut fieldarg) = split_field_name(&name_text);
    let lookup = (env.map)(&fieldtype_name);

    // Sort out unknown fields (or an argument mismatching the spec):
    // capitalize the field name and pass the field through untouched —
    // except a lone typefield body, which still gets type-linked.
    let known = lookup.is_some_and(|(i, _)| PY_DOC_FIELDS[i].has_arg == !fieldarg.is_empty());

    // Collect the content, trying not to keep unnecessary paragraphs.
    // Only a known field, or the unknown branch's typefield sub-case,
    // ever reads it — and under the std kinds' EMPTY type map `lookup` is
    // always None, so cloning the body up front copied every std
    // description's field bodies for nothing.
    let single_para = is_single_field_paragraph(&field.children[1]);
    let content: Vec<Node> = if !(known || matches!(lookup, Some((_, true)))) {
        Vec::new()
    } else if single_para {
        field.children[1].children[0].children.clone()
    } else {
        field.children[1].children.clone()
    };

    if !known {
        let mut new_fieldname = capitalize_first(&fieldtype_name);
        if !fieldarg.is_empty() {
            new_fieldname.push(' ');
            new_fieldname.push_str(&fieldarg);
        }
        // `field_name[0] = nodes.Text(new_fieldname)` — only the FIRST
        // child is replaced.
        let name_span = field.children[0].span;
        if !field.children[0].children.is_empty() {
            field.children[0].children[0] = Node::text_node(new_fieldname, name_span);
        }
        if let Some((ftype, true)) = lookup {
            // "but if this has a type then we can at least link it"
            if content.len() == 1 && content[0].kind == kinds::TEXT {
                let spec = &PY_DOC_FIELDS[ftype];
                let target = content[0].astext();
                let xrefs = py_make_doc_xrefs(
                    spec,
                    spec.typerolename,
                    &target,
                    "emphasis",
                    Some(&content[0]),
                    env,
                );
                let body = &mut field.children[1];
                if single_para {
                    body.children[0].children = xrefs;
                } else {
                    let mut para = Node::elem(kinds::PARAGRAPH, env.span);
                    para.children = xrefs;
                    body.children = vec![para];
                }
            }
        }
        entries.push(DocFieldEntry::Pass(field));
        return;
    }
    let (ftype, is_typefield) = lookup.expect("known implies present");
    let spec = &PY_DOC_FIELDS[ftype];

    // A typefield puts its content into the types collection and emits
    // nothing itself; only inline nodes survive the trip.
    if is_typefield {
        let filtered: Vec<Node> = content.into_iter().filter(is_inline_or_text).collect();
        if !filtered.is_empty() {
            types.entry(ftype).or_default().insert(fieldarg, filtered);
        }
        return;
    }

    // Also support the `:param type name:` syntax.
    if spec.is_typed {
        if let Some((argtype, argname)) = rsplit_field_arg(&fieldarg) {
            types
                .entry(ftype)
                .or_default()
                .insert(argname.clone(), vec![Node::text_node(argtype, env.span)]);
            fieldarg = argname;
        }
    }

    if spec.is_grouped {
        if let Some(&i) = group_indices.get(&ftype) {
            if let DocFieldEntry::Typed { items, .. } = &mut entries[i] {
                items.push((fieldarg, content));
            }
        } else {
            group_indices.insert(ftype, entries.len());
            entries.push(DocFieldEntry::Typed {
                ftype,
                items: vec![(fieldarg, content)],
            });
        }
    } else {
        entries.push(DocFieldEntry::Typed {
            ftype,
            items: vec![(fieldarg, content)],
        });
    }
}

/// `make_field` dispatch on the field class: `TypedField.make_field`
/// (`docfields.py:286-339`), `GroupedField.make_field` (`:214-248`), or
/// `Field.make_field` (`:141-184`).
fn make_doc_field(
    spec: &PyDocField,
    items: Vec<(String, Vec<Node>)>,
    fieldtypes: &mut std::collections::HashMap<String, Vec<Node>>,
    env: &DocFieldEnv<'_>,
) -> Node {
    let span = env.span;
    let mut fieldname = Node::elem(kinds::FIELD_NAME, span);
    fieldname.children.push(Node::text_node(spec.label, span));

    let fieldbody_children: Vec<Node> = if spec.is_typed {
        // TypedField: `name ( <type xrefs> ) -- description`, with the
        // type popped from the `:type x:` map (pop guards a doubled
        // `:param x:` from inserting the same type nodes twice).
        let handle_item = |fieldarg: String,
                           content: Vec<Node>,
                           fieldtypes: &mut std::collections::HashMap<String, Vec<Node>>|
         -> Node {
            let mut par = Node::elem(kinds::PARAGRAPH, span);
            par.children.extend(py_make_doc_xrefs(
                spec,
                spec.rolename,
                &fieldarg,
                "literal_strong",
                None,
                env,
            ));
            if let Some(fieldtype) = fieldtypes.remove(&fieldarg) {
                par.children.push(Node::text_node(" (", span));
                if fieldtype.len() == 1 && fieldtype[0].kind == kinds::TEXT {
                    let typename = fieldtype[0].astext();
                    par.children.extend(py_make_doc_xrefs(
                        spec,
                        spec.typerolename,
                        &typename,
                        "literal_emphasis",
                        None,
                        env,
                    ));
                } else {
                    par.children.extend(fieldtype);
                }
                par.children.push(Node::text_node(")", span));
            }
            let has_content = content.iter().any(|c| !c.astext().trim().is_empty());
            if has_content {
                par.children.push(Node::text_node(" -- ", span));
                par.children.extend(content);
            }
            par
        };

        if items.len() == 1 && spec.can_collapse {
            let (fieldarg, content) = items.into_iter().next().expect("one item");
            vec![handle_item(fieldarg, content, fieldtypes)]
        } else {
            let mut listnode = Node::elem(kinds::BULLET_LIST, span);
            for (fieldarg, content) in items {
                let mut li = Node::elem(kinds::LIST_ITEM, span);
                li.children.push(handle_item(fieldarg, content, fieldtypes));
                listnode.children.push(li);
            }
            vec![listnode]
        }
    } else if spec.is_grouped {
        // GroupedField: `<arg xref> -- description` items (the ` -- ` is
        // unconditional here, unlike TypedField's).
        let mut list_items: Vec<Node> = Vec::new();
        for (fieldarg, content) in items {
            let mut par = Node::elem(kinds::PARAGRAPH, span);
            par.children.extend(py_make_doc_xrefs(
                spec,
                spec.rolename,
                &fieldarg,
                "literal_strong",
                None,
                env,
            ));
            par.children.push(Node::text_node(" -- ", span));
            par.children.extend(content);
            let mut li = Node::elem(kinds::LIST_ITEM, span);
            li.children.push(par);
            list_items.push(li);
        }
        if list_items.len() == 1 && spec.can_collapse {
            let mut li = list_items.pop().expect("one item");
            vec![li.children.pop().expect("item paragraph")]
        } else {
            let mut listnode = Node::elem(kinds::BULLET_LIST, span);
            listnode.children = list_items;
            vec![listnode]
        }
    } else {
        // Field: single entry; a single-Text body may get a body role
        // (rtype's `class`). Both py Field-type fields have
        // `has_arg=False`, so the fieldarg-in-name branch is unreachable.
        let (_fieldarg, mut content) = items.into_iter().next().expect("one item");
        let single_textish = content.len() == 1
            && (content[0].kind == kinds::TEXT
                || (content[0].kind == "inline"
                    && content[0].children.len() == 1
                    && content[0].children[0].kind == kinds::TEXT));
        if single_textish {
            let target = content[0].astext();
            let contnode = content[0].clone();
            content = py_make_doc_xrefs(
                spec,
                spec.bodyrolename,
                &target,
                "emphasis",
                Some(&contnode),
                env,
            );
        }
        let mut par = Node::elem(kinds::PARAGRAPH, span);
        par.children = content;
        vec![par]
    };

    let mut fieldbody = Node::elem(kinds::FIELD_BODY, span);
    fieldbody.children = fieldbody_children;
    let mut fieldnode = Node::elem(kinds::FIELD, span);
    fieldnode.children.push(fieldname);
    fieldnode.children.push(fieldbody);
    fieldnode
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
            .take_while(|c| !crate::utils::py_isspace(*c) && *c != '=')
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

/// One `index['entries']` 5-tuple, rendered the way `str(tuple)` renders it
/// in Python — the *unescaped* item docutils then puts through
/// `serial_escape` when it prints the list attribute, which is why the
/// `entries` attribute is an [`AttrValue::List`] rather than a pre-joined
/// string: only the list form doubles a backslash inside a value, as
/// docutils does.
///
/// [`crate::env::genindex::parse_index_entries`] is the exact inverse, and
/// is what lifts these back out of a doctree for the index domain.
pub(crate) fn index_entry_tuple(
    entrytype: &str,
    value: &str,
    target_id: &str,
    main: &str,
    key: Option<&str>,
) -> String {
    format!(
        "({}, {}, {}, {}, {})",
        py_repr(Some(entrytype)),
        py_repr(Some(value)),
        py_repr(Some(target_id)),
        py_repr(Some(main)),
        py_repr(key)
    )
}

/// `process_index_entry` (`sphinx/util/nodes.py:431-482`): one `.. index::`
/// line as serialized 5-tuples.
///
/// Two details the shape of this function turns on: the `!` main marker is
/// stripped *with the whitespace behind it* (`entry[1:].lstrip()`), and the
/// comma shorthand re-splits `oentry` — the line *before* that strip — so
/// each comma-separated value re-reads its own `!`.
///
/// The legacy `module:`/`keyword:`/... prefixes raise `ValueError` in
/// sphinx; here they fall through to the shorthand branch (hardening note —
/// the oracle corpus avoids them).
fn process_index_entry(entry: &str, target_id: &str) -> Vec<String> {
    use crate::utils::py_isspace;
    const TYPES: &[&str] = &["single", "pair", "double", "triple", "see", "seealso"];
    // Every strip here is Python's — `entry.strip()`, `entry[1:].lstrip()`,
    // `value.strip()` — so `\x1f` goes wherever a space would (round F,
    // pinned by the `index_*_us_*` sphinx cases; Rust's set had kept it).
    let oentry = entry.trim_matches(py_isspace);
    let stripped = match oentry.strip_prefix('!') {
        Some(rest) => rest.trim_start_matches(py_isspace),
        None => oentry,
    };
    let main = if oentry.starts_with('!') { "main" } else { "" };
    for t in TYPES {
        if let Some(value) = stripped.strip_prefix(&format!("{t}:")) {
            let value = value.trim_matches(py_isspace);
            let ty = if *t == "double" { "pair" } else { t };
            return vec![index_entry_tuple(ty, value, target_id, main, None)];
        }
    }
    // Shorthand notation for single entries: every comma-separated value of
    // the *original* line, each carrying its own `!` marker.
    oentry
        .split(',')
        .filter_map(|value| {
            let value = value.trim_matches(py_isspace);
            let (main, value) = match value.strip_prefix('!') {
                Some(rest) => ("main", rest.trim_start_matches(py_isspace)),
                None => ("", value),
            };
            if value.is_empty() {
                return None;
            }
            Some(index_entry_tuple("single", value, target_id, main, None))
        })
        .collect()
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
    /// codecs.lookup validation against the encodings this crate can
    /// decode (see [`lookup_encoding`]), with docutils' `unknown
    /// encoding` error text. Valid-but-undecodable Python codec names
    /// (utf-16, cp1252, ...) are rejected too — documented divergence.
    Encoding,
    /// `directives.value_or((None,), int)` — the include directive's
    /// `number-lines`: a bare flag is Python None, else `int(arg)`.
    FlagOrInt,
    /// figure :figwidth:: the literal 'image' keyword or a length.
    Figwidth,
    /// Plain Python int() — negatives allowed (sphinx maxdepth).
    PyIntAny,
    /// sphinx `optional_int` (`SP/directives/__init__.py:32-41`): a bare
    /// option is Python None, else a non-negative int — the
    /// literalinclude `:dedent:`.
    OptionalInt,
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
struct DirectiveInput<'r> {
    /// The directive name AS WRITTEN (docutils self.name; error messages
    /// reproduce the original case).
    name: &'r str,
    arguments: Vec<String>,
    options: Vec<(String, OptVal)>,
    content: Vec<LineRec>,
    span: Span,
    lineno: u32,
    /// docutils' `content_offset` as a 1-based line
    /// (`parse_directive_block`, `states.py:2301-2345`): where a nested
    /// parse at the directive's content offset numbers its first line —
    /// the first content line, or, with no content, the line the offset
    /// arithmetic lands on past the argument/option block. Sphinx parses a
    /// `:caption:` there (`container_wrapper`, `code.py:78-96`), so the
    /// messages that throwaway parse creates are located by it.
    content_lineno: u32,
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
    ("figname", Conv::Unchanged),
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

/// The include option spec (`misc.py:59-71`). `parser` is accepted
/// unvalidated (its docutils converter resolves a parser class; the mode
/// itself is the §Scope-decision SEVERE).
const INCLUDE_OPTS: &[(&str, Conv)] = &[
    ("literal", Conv::Flag),
    ("code", Conv::Unchanged),
    ("encoding", Conv::Encoding),
    ("parser", Conv::Unchanged),
    ("tab-width", Conv::PyIntAny),
    ("start-line", Conv::PyIntAny),
    ("end-line", Conv::PyIntAny),
    ("start-after", Conv::UnchangedRequired),
    ("end-before", Conv::UnchangedRequired),
    ("number-lines", Conv::FlagOrInt),
    ("class", Conv::ClassOption),
    ("name", Conv::Unchanged),
];

// ----------------------------------------------------------------------
// include helpers (docutils misc.py Include)
// ----------------------------------------------------------------------

/// The comment-line prefix that pops [`BlockParser::include_log`]
/// (`DU/parsers/rst/states.py:2425-2433`).
const INCLUDE_MARKER_PREFIX: &str = "end of inclusion from \"";

/// docutils `settings.line_length_limit` default
/// (`DU/parsers/__init__.py:76`).
const LINE_LENGTH_LIMIT: usize = 10_000;

/// The circular-inclusion identity's clip half: `(start-line, end-line,
/// start-after, end-before)` exactly as `misc.py:85-88` builds it.
type IncludeClip = (Option<i64>, Option<i64>, String, String);

/// A resolved include argument.
enum IncludeTarget {
    /// `<name>` — one of the vendored docutils standard include files.
    Standard(String),
    /// A project file: the filesystem path to open, the display spelling
    /// every path-bearing surface uses (§Scope-8: srcdir-relative in
    /// sphinx mode), and the bookkeeping spelling sphinx's `relfn2path`
    /// returns as `rel_fn` — the RESOLVED `io_path` relative to the
    /// resolved srcdir, walking up with `..` for a file outside it —
    /// which is what `note_dependency` records. The two differ through a
    /// symlinked directory (`link/../x.rst` displays as `x.rst`; it was
    /// read beside the link's TARGET). Docutils mode has no srcdir and
    /// records nothing, so there `record` is just the display spelling.
    File {
        io_path: std::path::PathBuf,
        display: String,
        record: String,
    },
}

impl IncludeTarget {
    fn display(&self) -> std::borrow::Cow<'_, str> {
        match self {
            // The `<name>` spelling is the only environment-independent
            // one this crate has for a standard include (docutils prints
            // its installation directory, cwd-relative).
            IncludeTarget::Standard(name) => std::borrow::Cow::Owned(format!("<{name}>")),
            IncludeTarget::File { display, .. } => std::borrow::Cow::Borrowed(display),
        }
    }
}

/// An int-converted option's value; canonical big-int strings (beyond
/// i64) saturate — every consumer clamps anyway.
fn opt_i64(options: &[(String, OptVal)], name: &str) -> Option<i64> {
    match opt_get(options, name) {
        Some(OptVal::Int(n)) => Some(*n),
        Some(OptVal::Str(s)) => Some(saturating_i64(s)),
        _ => None,
    }
}

fn saturating_i64(canonical: &str) -> i64 {
    canonical
        .parse::<i64>()
        .unwrap_or(if canonical.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        })
}

/// Python `str.expandtabs(tabsize)`: the column resets at `\n`/`\r` and
/// counts characters; `tabsize <= 0` removes tabs outright.
fn py_expandtabs(text: &str, tabsize: i64) -> String {
    // The caller must have run [`c_int_tabsize`] first: CPython refuses a
    // tabsize outside C-int range before it looks at the string, and an
    // unchecked i64 would push spaces here until the allocator gives up.
    debug_assert!(c_int_tabsize(tabsize).is_ok(), "unchecked tabsize");
    let mut out = String::with_capacity(text.len());
    let mut col: i64 = 0;
    for c in text.chars() {
        match c {
            '\t' => {
                if tabsize > 0 {
                    let pad = tabsize.saturating_sub(col % tabsize);
                    for _ in 0..pad {
                        out.push(' ');
                    }
                    col = col.saturating_add(pad);
                }
            }
            '\n' | '\r' => {
                out.push(c);
                col = 0;
            }
            _ => {
                out.push(c);
                col = col.saturating_add(1);
            }
        }
    }
    out
}

/// `str.expandtabs`'s `OverflowError` text.
const PY_INT_TOO_LARGE_FOR_C_INT: &str = "Python int too large to convert to C int";

/// CPython parses `str.expandtabs(tabsize)`'s argument with `i` — a C
/// `int` — so a value outside `[i32::MIN, i32::MAX]` raises
/// `OverflowError: Python int too large to convert to C int` BEFORE the
/// string is examined: even `'ab'.expandtabs(2**31)` raises, while
/// `2**31 - 1` and `-2**31` are accepted (probed, scratchpad A/tw.py).
///
/// `literalinclude` funnels the exception into one directive warning
/// (`code.py:505`, `except Exception`), matching sphinx byte-for-byte.
/// `include` has no such guard, so sphinx ABORTS the whole build on the
/// same input — a crash has no pformat, so our SEVERE there is an
/// unpinnable better-than-sphinx divergence. What IS pinnable is the
/// ordering: docutils reaches `expandtabs` only after the read succeeds,
/// so every earlier failure (and a text `expandtabs` never sees) must win
/// over this check — see `BlockParser::tab_width_overflow`.
fn c_int_tabsize(tabsize: i64) -> Result<i64, &'static str> {
    if tabsize < i64::from(i32::MIN) || tabsize > i64::from(i32::MAX) {
        return Err(PY_INT_TOO_LARGE_FOR_C_INT);
    }
    Ok(tabsize)
}

/// docutils `statemachine.string2lines(text, tab_width,
/// convert_whitespace=True)` (`DU/statemachine.py:1497-1516`): `\v`/`\f`
/// to spaces, splitlines, per-line `expandtabs(tab_width)` + `rstrip()` —
/// Python's `str.rstrip()`, i.e. [`crate::utils::py_isspace`], the same
/// conversion `src/rst/lines.rs` got in round E. The line-length-limit
/// check runs on this output, so a trailing `\x1f` the rstrip does not eat
/// would push a limit-length line over it (probe-pinned, round F).
///
/// `expandtabs` runs per LINE — `[s.expandtabs(tab_width).rstrip() for s
/// in astring.splitlines()]` — regardless of the width's sign, so an
/// out-of-C-int width is an error for any non-empty text and a no-op for
/// an empty one (zero lines, zero calls; probed: an empty file included
/// with `:tab-width: 2147483648` produces nothing at all).
fn string2lines_tw(text: &str, tab_width: i64) -> Result<Vec<String>, &'static str> {
    let converted = text.replace(['\x0b', '\x0c'], " ");
    let lines = py_splitlines(&converted);
    if !lines.is_empty() {
        c_int_tabsize(tab_width)?;
    }
    Ok(lines
        .into_iter()
        .map(|line| {
            let expanded = py_expandtabs(line, tab_width);
            expanded
                .trim_end_matches(crate::utils::py_isspace)
                .to_string()
        })
        .collect())
}

/// Python `sequence[start:end]` slice bounds: negatives count from the
/// end, everything clamps, `end < start` yields the empty slice.
fn py_slice(len: usize, start: Option<i64>, end: Option<i64>) -> (usize, usize) {
    let n = len as i64;
    let index = |v: i64| -> i64 {
        if v < 0 {
            (n + v).max(0)
        } else {
            v.min(n)
        }
    };
    let from = start.map(&index).unwrap_or(0);
    let to = end.map(&index).unwrap_or(n).max(from);
    (from as usize, to as usize)
}

/// `os.path.dirname(source)` (`misc.py:33`, the `adapt_path` base of both
/// file-inserting directives in docutils mode): everything before the last
/// separator, and the empty string when there is none.
///
/// The separators are the PLATFORM's ([`std::path::is_separator`]: `/`
/// everywhere, `\` too on Windows) because the document source path is an
/// OS path — `C:\docs\main.rst` holds no `/` at all, and splitting it on
/// `/` alone left the base empty, so every docutils-mode include resolved
/// against the process cwd instead of against the containing file.
fn containing_dir(source_path: &str) -> &str {
    match source_path.rfind(std::path::is_separator) {
        Some(cut) => &source_path[..cut],
        None => "",
    }
}

#[cfg(test)]
mod containing_dir_tests {
    use super::containing_dir;

    /// The POSIX rule holds everywhere; the `\` half only exists on
    /// Windows, where `os.path.dirname` splits on it too (a document
    /// source path there is `C:\docs\main.rst`, with no `/` in it at all).
    #[test]
    fn the_containing_directory_splits_on_the_platforms_separators() {
        assert_eq!(containing_dir("a/b/main.rst"), "a/b");
        assert_eq!(containing_dir("/abs/main.rst"), "/abs");
        assert_eq!(containing_dir("main.rst"), "");
        assert_eq!(containing_dir(""), "");
        assert_eq!(containing_dir("<string>"), "");
        #[cfg(windows)]
        {
            assert_eq!(containing_dir(r"C:\docs\main.rst"), r"C:\docs");
            assert_eq!(containing_dir(r"C:\docs\sub/main.rst"), r"C:\docs\sub");
        }
        #[cfg(not(windows))]
        {
            // A backslash is an ordinary filename character off Windows.
            assert_eq!(containing_dir(r"C:\docs\main.rst"), "");
        }
    }
}

/// The docutils `io.FileInput` open-failure spelling: `io.error_string`
/// renders `InputError: [Errno N] <strerror>: '<path>'` (`DU/io.py:72-75`
/// wraps the OSError as its `InputError` subclass). Probe-pinned for
/// errno 2 (missing), 13 (permission denied) and 21 (directory).
fn py_input_error_text(error: &std::io::Error, path: &str) -> String {
    match py_oserror_parts(error) {
        Some((errno, strerror)) => format!("InputError: [Errno {errno}] {strerror}: '{path}'"),
        // An error shape neither branch below recognizes: Rust's own
        // message, without the `[Errno N]` Python would only have if we
        // knew the number.
        None => format!("InputError: {error}: '{path}'"),
    }
}

/// `(OSError.errno, OSError.strerror)` as CPython would spell the pair.
///
/// Python reports a POSIX errno and `os.strerror`'s text on EVERY
/// platform: a missing file is `[Errno 2] No such file or directory` in
/// Windows Python exactly as in Linux Python, because the errno comes from
/// the CRT's own mapping of the Win32 status and the text from Python's
/// `strerror` table.
fn py_oserror_parts(error: &std::io::Error) -> Option<(i32, String)> {
    platform_errno(error).or_else(|| {
        python_errno_of_kind(error.kind()).map(|(errno, text)| (errno, text.to_string()))
    })
}

/// On Unix `raw_os_error()` IS the errno Python reports, and Rust renders
/// it through the very `strerror(3)` table `os.strerror` reads — so the
/// pair is byte-identical for every errno, including the ones no
/// [`std::io::ErrorKind`] names.
#[cfg(unix)]
fn platform_errno(error: &std::io::Error) -> Option<(i32, String)> {
    let errno = error.raw_os_error()?;
    // Rust renders a raw OS error as "<strerror> (os error N)"; Python's
    // message is the bare strerror.
    let rendered = std::io::Error::from_raw_os_error(errno).to_string();
    let suffix = format!(" (os error {errno})");
    let strerror = rendered.strip_suffix(suffix.as_str()).unwrap_or(&rendered);
    Some((errno, strerror.to_string()))
}

/// Off Unix, `raw_os_error()` is NOT an errno — on Windows it is the Win32
/// code (2 `ERROR_FILE_NOT_FOUND`, 3 `ERROR_PATH_NOT_FOUND`, 5
/// `ERROR_ACCESS_DENIED`), which Rust renders with the Win32 text ("The
/// system cannot find the file specified."). Python prints neither, so the
/// number and the text both have to come from [`python_errno_of_kind`].
#[cfg(not(unix))]
fn platform_errno(_error: &std::io::Error) -> Option<(i32, String)> {
    None
}

/// The `(errno, os.strerror(errno))` pairs whose NUMBER and TEXT are the
/// same in glibc, in macOS libc and in the MSVC CRT — the table Python
/// prints from on any platform this crate builds for. Probed on the pinned
/// toolchain: `os.strerror(2)` `'No such file or directory'`,
/// `os.strerror(13)` `'Permission denied'`, `os.strerror(20)` `'Not a
/// directory'`, `os.strerror(21)` `'Is a directory'`.
///
/// ENAMETOOLONG and ELOOP are deliberately absent: their numbers are
/// platform-specific (36/40 on Linux, 63/62 on macOS, 38/114 in the CRT),
/// so there is no portable pair to pin, and on Unix — the only place this
/// crate is probed against — [`platform_errno`] answers first anyway.
fn python_errno_of_kind(kind: std::io::ErrorKind) -> Option<(i32, &'static str)> {
    match kind {
        std::io::ErrorKind::NotFound => Some((2, "No such file or directory")),
        std::io::ErrorKind::PermissionDenied => Some((13, "Permission denied")),
        std::io::ErrorKind::NotADirectory => Some((20, "Not a directory")),
        std::io::ErrorKind::IsADirectory => Some((21, "Is a directory")),
        _ => None,
    }
}

/// The text encodings the include directive can actually decode. The
/// `encoding` option converter validates against this set, so a
/// valid-but-unsupported Python codec name (utf-16, cp1252, ...) earns
/// docutils' `unknown encoding` option error — a documented divergence
/// (docutils accepts every `codecs.lookup` name).
#[derive(Clone, Copy, PartialEq)]
enum IncludeEncoding {
    Utf8,
    Utf8Sig,
    Ascii,
    Latin1,
}

/// The default value of sphinx's `source_encoding` config key, which the
/// environment copies onto `settings.input_encoding`
/// (`environment/__init__.py:68` `'input_encoding': 'utf-8-sig'` and
/// `:375` `self.settings['input_encoding'] = config.source_encoding`).
/// Both file-inserting directives fall back to the CONFIGURED value
/// ([`BlockParser::source_encoding`], threaded from
/// `BuildConfig::source_encoding`): `include` through
/// `settings.input_encoding` (`misc.py:116`) and `literalinclude` through
/// `config.source_encoding` directly ([`LiteralIncludeReader::new`]).
/// This constant is what a configured name outside [`lookup_encoding`]'s
/// table degrades to (sphinx would raise `LookupError` at the first read;
/// `BuildConfig::validate` warns instead).
const SPHINX_DEFAULT_SOURCE_ENCODING: IncludeEncoding = IncludeEncoding::Utf8Sig;

/// Whether `name` is a codec this crate can decode — the `source_encoding`
/// config check in `BuildConfig::validate` asks before the parser has to
/// fall back.
pub(crate) fn is_supported_encoding(name: &str) -> bool {
    lookup_encoding(name).is_some()
}

/// `codecs.lookup` normalization + alias resolution for the supported
/// set. Python lowercases and collapses runs of punctuation to `_`.
fn lookup_encoding(name: &str) -> Option<IncludeEncoding> {
    let mut normalized = String::with_capacity(name.len());
    let mut pending_sep = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_sep && !normalized.is_empty() {
                normalized.push('_');
            }
            pending_sep = false;
            normalized.push(c.to_ascii_lowercase());
        } else {
            pending_sep = true;
        }
    }
    match normalized.as_str() {
        "utf_8" | "utf8" | "utf" | "u8" | "cp65001" => Some(IncludeEncoding::Utf8),
        "utf_8_sig" => Some(IncludeEncoding::Utf8Sig),
        "ascii" | "us_ascii" | "us" | "646" | "cp367" | "ibm367" | "ansi_x3_4_1968"
        | "ansi_x3_4_1986" | "iso646_us" | "iso_ir_6" | "csascii" => Some(IncludeEncoding::Ascii),
        "latin_1" | "latin1" | "latin" | "l1" | "iso_8859_1" | "iso8859_1" | "iso8859" | "8859"
        | "cp819" | "ibm819" | "iso_ir_100" | "csisolatin1" => Some(IncludeEncoding::Latin1),
        _ => None,
    }
}

/// Decode with Python's exact `UnicodeDecodeError` message on failure
/// (the SEVERE body text — probe-pinned, no trailing period).
fn decode_include_bytes(bytes: &[u8], encoding: IncludeEncoding) -> Result<String, String> {
    match encoding {
        IncludeEncoding::Utf8 => std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|e| py_utf8_error_text(bytes, e)),
        IncludeEncoding::Utf8Sig => {
            let stripped = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
            std::str::from_utf8(stripped)
                .map(str::to_string)
                .map_err(|e| py_utf8_error_text(stripped, e))
        }
        IncludeEncoding::Ascii => match bytes.iter().position(|&b| b >= 0x80) {
            None => Ok(std::str::from_utf8(bytes)
                .expect("pure ASCII is valid UTF-8")
                .to_string()),
            Some(i) => Err(format!(
                "UnicodeDecodeError: 'ascii' codec can't decode byte 0x{:02x} in position {}: \
                 ordinal not in range(128)",
                bytes[i], i
            )),
        },
        IncludeEncoding::Latin1 => Ok(bytes.iter().map(|&b| b as char).collect()),
    }
}

/// CPython's UTF-8 decode error message: reason and error range follow
/// the "maximal subpart" convention std's `Utf8Error` also uses.
fn py_utf8_error_text(bytes: &[u8], error: std::str::Utf8Error) -> String {
    let start = error.valid_up_to();
    let (len, reason) = match error.error_len() {
        Some(len) => {
            // 0xC2..=0xF4 are the legal multi-byte lead bytes; anything
            // else at the error position is an invalid start byte.
            let reason = if matches!(bytes[start], 0xC2..=0xF4) {
                "invalid continuation byte"
            } else {
                "invalid start byte"
            };
            (len, reason)
        }
        None => (bytes.len() - start, "unexpected end of data"),
    };
    if len == 1 {
        format!(
            "UnicodeDecodeError: 'utf-8' codec can't decode byte 0x{:02x} in position {}: {}",
            bytes[start], start, reason
        )
    } else {
        format!(
            "UnicodeDecodeError: 'utf-8' codec can't decode bytes in position {}-{}: {}",
            start,
            start + len - 1,
            reason
        )
    }
}

// ----------------------------------------------------------------------
// literalinclude (SP/directives/code.py LiteralInclude + reader)
// ----------------------------------------------------------------------

/// Python `str.splitlines(keepends=True)`: the full boundary set (incl.
/// `\v\f\x1c-\x1e\x85\u{2028}\u{2029}`), `\r\n` kept as one line ending.
fn py_splitlines_keepends(text: &str) -> Vec<String> {
    let is_boundary = |c: char| {
        matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        )
    };
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if is_boundary(c) {
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = chars.peek() {
                    chars.next();
                    end = j + 1;
                }
            }
            out.push(text[start..end].to_string());
            start = end;
        }
    }
    if start < text.len() {
        out.push(text[start..].to_string());
    }
    out
}

/// Python 3.12 `textwrap.dedent` (the probes' interpreter — 3.13 rewrote
/// it with `str.isspace` line-blanking; 3.12's regexes blank `[ \t]`-only
/// lines):
///
/// 1. `^[ \t]+$` (MULTILINE) segments — whitespace-only between `\n`
///    boundaries — are emptied BEFORE the margin is computed.
/// 2. The margin is the common `[ \t]` prefix of every segment with a
///    character beyond its prefix, reduced pairwise.
/// 3. `(?m)^margin` is stripped; a segment not starting with the margin
///    is left untouched (CPython's consistency assert is dead code).
fn py_textwrap_dedent(text: &str) -> String {
    let cleaned: Vec<&str> = text
        .split('\n')
        .map(|seg| {
            if !seg.is_empty() && seg.chars().all(|c| c == ' ' || c == '\t') {
                ""
            } else {
                seg
            }
        })
        .collect();
    let mut margin: Option<&str> = None;
    for seg in &cleaned {
        let prefix_end = seg.find(|c| c != ' ' && c != '\t').unwrap_or(seg.len());
        if prefix_end == seg.len() {
            continue; // nothing beyond the prefix — not a margin witness
        }
        let indent = &seg[..prefix_end];
        margin = Some(match margin {
            None => indent,
            Some(current) if indent.starts_with(current) => current,
            Some(current) if current.starts_with(indent) => indent,
            Some(current) => {
                // First disagreement truncates (one side is never a
                // prefix of the other here, so a mismatch exists).
                let common = current
                    .bytes()
                    .zip(indent.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                &current[..common]
            }
        });
    }
    let margin = margin.unwrap_or("");
    cleaned
        .iter()
        .map(|seg| seg.strip_prefix(margin).unwrap_or(seg))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One comma-part of a parsed `:lines:`/`:emphasize-lines:` spec.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LineSpecPart {
    /// One 0-based index. May be negative: `0` parses to −1 (Python's
    /// `int('0') - 1`), which the selection then wraps Python-style.
    Single(i64),
    /// The half-open 0-based range `start..end` — never empty (`A > B`
    /// raises at parse), `start` can be −1 (`0-N`), `end` can exceed the
    /// file (open `A-` ranges use `max(A, total)`).
    Range(i64, i64),
}

/// `sphinx.util._lines.parse_line_num_spec`'s result, kept as parts
/// instead of the materialized list Python builds — `1-999999999` would
/// otherwise allocate the whole range only for everything past the file
/// end to be dropped. Every consumer question (first member, contiguity,
/// out-of-range presence, in-range values in written order) is answered
/// from the parts with identical semantics.
struct LineSpec {
    parts: Vec<LineSpecPart>,
}

impl LineSpec {
    /// `linelist[0]` — the parse guarantees at least one part and no part
    /// is empty, so a parsed spec always has a first member.
    fn first(&self) -> i64 {
        match self.parts[0] {
            LineSpecPart::Single(n) => n,
            LineSpecPart::Range(a, _) => a,
        }
    }

    /// `all(first + i == n for i, n in enumerate(linelist))` — including
    /// out-of-range members, exactly like Python (`code.py:305-312`).
    fn is_contiguous(&self) -> bool {
        let mut expected = self.first();
        for part in &self.parts {
            match *part {
                LineSpecPart::Single(n) => {
                    if n != expected {
                        return false;
                    }
                    expected = n + 1;
                }
                LineSpecPart::Range(a, b) => {
                    if a != expected {
                        return false;
                    }
                    expected = b;
                }
            }
        }
        true
    }

    /// `any(i >= total for i in linelist)`.
    fn any_out_of_range(&self, total: i64) -> bool {
        self.parts.iter().any(|part| match *part {
            LineSpecPart::Single(n) => n >= total,
            LineSpecPart::Range(_, b) => b > total,
        })
    }

    /// The members `< total`, in written order with duplicates preserved
    /// (`[n for n in linelist if n < total]`) — negative members included,
    /// the way Python's filter keeps them.
    fn in_range_values(&self, total: i64) -> Vec<i64> {
        let mut out = Vec::new();
        for part in &self.parts {
            match *part {
                LineSpecPart::Single(n) => {
                    if n < total {
                        out.push(n);
                    }
                }
                LineSpecPart::Range(a, b) => {
                    let mut n = a;
                    while n < b.min(total) {
                        out.push(n);
                        n += 1;
                    }
                }
            }
        }
        out
    }
}

/// `parse_line_num_spec` (`SP/util/_lines.py:4-29`): comma-separated
/// parts, `N` → the 0-based `N-1`, `A-B` → `range(A-1, B)` with `A`
/// defaulting to 1 and `B` to `max(A, total)` (open ranges), `A > B` or a
/// bare `-` → `ValueError(f'invalid line number spec: {spec!r}')`.
fn parse_line_num_spec(spec: &str, total: i64) -> Result<LineSpec, String> {
    let invalid = || format!("invalid line number spec: {}", py_repr(Some(spec)));
    let mut parts = Vec::new();
    for part in spec.split(',') {
        // `part.strip().split('-')` — Python's set (round F, unit-test pin).
        let stripped = part.trim_matches(crate::utils::py_isspace);
        let begend: Vec<&str> = stripped.split('-').collect();
        if begend == [""; 2] {
            return Err(invalid());
        }
        match begend.len() {
            1 => {
                let n = py_int(begend[0]).ok_or_else(invalid)?;
                parts.push(LineSpecPart::Single(n - 1));
            }
            2 => {
                let start = if begend[0].is_empty() {
                    1
                } else {
                    py_int(begend[0]).ok_or_else(invalid)?
                };
                let end = if begend[1].is_empty() {
                    start.max(total)
                } else {
                    py_int(begend[1]).ok_or_else(invalid)?
                };
                if start > end {
                    return Err(invalid());
                }
                parts.push(LineSpecPart::Range(start - 1, end));
            }
            _ => return Err(invalid()),
        }
    }
    Ok(LineSpec { parts })
}

/// `dedent_lines` (`SP/directives/code.py:59-75`): `None` (a bare
/// `:dedent:`) is a full `textwrap.dedent` over the joined text; an
/// integer cuts `line[dedent:]` per line (character slice), preserving a
/// bare `'\n'` for lines that become empty, warning once when any cut
/// prefix held non-whitespace.
///
/// WHITESPACE-SET NANO-EDGE (shared with the other `is_whitespace` sites
/// in this file): sphinx tests `if any(s[:dedent].strip() ...)`, and
/// Python's `str.strip()` treats `\x1c`-`\x1f` (the FS/GS/RS/US
/// separators) as whitespace, while Rust's `char::is_whitespace` — the
/// Unicode `White_Space` property — does not. A cut prefix made only of
/// those four characters is silent in sphinx and warns here. `\x85` and
/// the Unicode spaces agree in both. Not modelled: reaching it needs a
/// separator character in a source file's leading indentation.
fn dedent_lines(
    lines: Vec<String>,
    dedent: Option<i64>,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let Some(dedent) = dedent else {
        return py_splitlines_keepends(&py_textwrap_dedent(&lines.concat()));
    };
    let dedent = dedent.max(0) as usize;
    if lines
        .iter()
        .any(|line| line.chars().take(dedent).any(|c| !c.is_whitespace()))
    {
        warnings.push("non-whitespace stripped by dedent".to_string());
    }
    lines
        .iter()
        .map(|line| {
            let new_line: String = line.chars().skip(dedent).collect();
            if line.ends_with('\n') && new_line.is_empty() {
                "\n".to_string()
            } else {
                new_line
            }
        })
        .collect()
}

/// difflib opcode tags.
#[derive(Clone, Copy, Debug, PartialEq)]
enum OpTag {
    Replace,
    Delete,
    Insert,
    Equal,
}

type Opcode = (OpTag, usize, usize, usize, usize);

/// `difflib.SequenceMatcher` over line sequences — exactly the subset
/// `unified_diff` constructs (`isjunk=None`, `autojunk=True`), opcode for
/// opcode against CPython 3.12's difflib. `show_diff` feeds it whole
/// files (`SP/directives/code.py:261-266`).
struct PySequenceMatcher<'a> {
    a: &'a [String],
    b: &'a [String],
    /// element → ascending indices in `b`; `__chain_b`'s autojunk purge
    /// removes popular elements (> n/100 + 1 occurrences once `b` has ≥
    /// 200 lines) so they cannot SEED a match — the extension loops in
    /// `find_longest_match` still grow a match across them, exactly like
    /// CPython (whose extension loops test `bjunk`, empty here, not
    /// `bpopular`).
    b2j: std::collections::HashMap<&'a str, Vec<usize>>,
}

impl<'a> PySequenceMatcher<'a> {
    fn new(a: &'a [String], b: &'a [String]) -> Self {
        let mut b2j: std::collections::HashMap<&'a str, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, elt) in b.iter().enumerate() {
            b2j.entry(elt.as_str()).or_default().push(i);
        }
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, indices| indices.len() <= ntest);
        }
        PySequenceMatcher { a, b, b2j }
    }

    /// `find_longest_match` with an empty junk set: the two pairs of
    /// extension while-loops collapse into one. Returns `(i, j, size)`.
    fn find_longest_match(
        &self,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let mut besti = alo;
        let mut bestj = blo;
        let mut bestsize = 0usize;
        let mut j2len: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for i in alo..ahi {
            let mut newj2len: std::collections::HashMap<usize, usize> =
                std::collections::HashMap::new();
            if let Some(indices) = self.b2j.get(self.a[i].as_str()) {
                for &j in indices {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = if j == 0 {
                        1
                    } else {
                        j2len.get(&(j - 1)).copied().unwrap_or(0) + 1
                    };
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        while besti > alo && bestj > blo && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && self.a[besti + bestsize] == self.b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// `get_matching_blocks`: queue-driven recursion then adjacent-block
    /// merging, terminated by the `(la, lb, 0)` sentinel.
    fn get_matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let la = self.a.len();
        let lb = self.b.len();
        let mut queue = vec![(0usize, la, 0usize, lb)];
        let mut blocks: Vec<(usize, usize, usize)> = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let mut non_adjacent: Vec<(usize, usize, usize)> = Vec::new();
        let (mut i1, mut j1, mut k1) = (0usize, 0usize, 0usize);
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    non_adjacent.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            non_adjacent.push((i1, j1, k1));
        }
        non_adjacent.push((la, lb, 0));
        non_adjacent
    }

    fn get_opcodes(&self) -> Vec<Opcode> {
        let mut i = 0usize;
        let mut j = 0usize;
        let mut answer: Vec<Opcode> = Vec::new();
        for (ai, bj, size) in self.get_matching_blocks() {
            let tag = if i < ai && j < bj {
                Some(OpTag::Replace)
            } else if i < ai {
                Some(OpTag::Delete)
            } else if j < bj {
                Some(OpTag::Insert)
            } else {
                None
            };
            if let Some(tag) = tag {
                answer.push((tag, i, ai, j, bj));
            }
            i = ai + size;
            j = bj + size;
            if size > 0 {
                answer.push((OpTag::Equal, ai, i, bj, j));
            }
        }
        answer
    }

    /// `get_grouped_opcodes(n)`: leading/trailing equal runs trimmed to
    /// `n` context lines, groups split at equal runs longer than `2n`.
    fn get_grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.get_opcodes();
        if codes.is_empty() {
            codes.push((OpTag::Equal, 0, 1, 0, 1));
        }
        if codes[0].0 == OpTag::Equal {
            let (tag, i1, i2, j1, j2) = codes[0];
            codes[0] = (
                tag,
                i1.max(i2.saturating_sub(n)),
                i2,
                j1.max(j2.saturating_sub(n)),
                j2,
            );
        }
        let last = codes.len() - 1;
        if codes[last].0 == OpTag::Equal {
            let (tag, i1, i2, j1, j2) = codes[last];
            codes[last] = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let nn = n + n;
        let mut groups: Vec<Vec<Opcode>> = Vec::new();
        let mut group: Vec<Opcode> = Vec::new();
        for (tag, i1, i2, j1, j2) in codes {
            if tag == OpTag::Equal && i2 - i1 > nn {
                group.push((tag, i1, (i1 + n).min(i2), j1, (j1 + n).min(j2)));
                groups.push(std::mem::take(&mut group));
                group.push((
                    tag,
                    i1.max(i2.saturating_sub(n)),
                    i2,
                    j1.max(j2.saturating_sub(n)),
                    j2,
                ));
            } else {
                group.push((tag, i1, i2, j1, j2));
            }
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == OpTag::Equal) {
            groups.push(group);
        }
        groups
    }
}

/// `difflib.unified_diff(a, b, fromfile, tofile)` — no timestamps, three
/// context lines, `\n` line terminator: the exact call `show_diff` makes.
fn py_unified_diff(a: &[String], b: &[String], fromfile: &str, tofile: &str) -> Vec<String> {
    /// `_format_range_unified`: 1-based start, `start,length` with the
    /// single-line shorthand and empty ranges starting one line early.
    fn format_range_unified(start: usize, stop: usize) -> String {
        let beginning = start + 1;
        let length = stop - start;
        if length == 1 {
            return beginning.to_string();
        }
        let beginning = if length == 0 {
            beginning - 1
        } else {
            beginning
        };
        format!("{beginning},{length}")
    }
    let matcher = PySequenceMatcher::new(a, b);
    let mut out = Vec::new();
    let mut started = false;
    for group in matcher.get_grouped_opcodes(3) {
        if !started {
            started = true;
            out.push(format!("--- {fromfile}\n"));
            out.push(format!("+++ {tofile}\n"));
        }
        let first = group[0];
        let last = group[group.len() - 1];
        let file1_range = format_range_unified(first.1, last.2);
        let file2_range = format_range_unified(first.3, last.4);
        out.push(format!("@@ -{file1_range} +{file2_range} @@\n"));
        for (tag, i1, i2, j1, j2) in group {
            match tag {
                OpTag::Equal => {
                    for line in &a[i1..i2] {
                        out.push(format!(" {line}"));
                    }
                }
                OpTag::Replace => {
                    for line in &a[i1..i2] {
                        out.push(format!("-{line}"));
                    }
                    for line in &b[j1..j2] {
                        out.push(format!("+{line}"));
                    }
                }
                OpTag::Delete => {
                    for line in &a[i1..i2] {
                        out.push(format!("-{line}"));
                    }
                }
                OpTag::Insert => {
                    for line in &b[j1..j2] {
                        out.push(format!("+{line}"));
                    }
                }
            }
        }
    }
    out
}

/// docutils `system_message.astext()` (`DU/nodes.py`): the location
/// prefix `'{source}:{line}: ({type}/{level}) '` plus the element text —
/// children joined with `'\n\n'` (`Element.child_text_separator`). The
/// `Invalid caption` message body renders through this (probed).
fn system_message_astext(node: &Node) -> String {
    let source = match node.get("source") {
        Some(AttrValue::Str(s)) => s.clone(),
        _ => String::new(),
    };
    let line = match node.get("line") {
        Some(AttrValue::Int(n)) => n.to_string(),
        _ => String::new(),
    };
    let msg_type = match node.get("type") {
        Some(AttrValue::Str(s)) => s.clone(),
        _ => String::new(),
    };
    let level = match node.get("level") {
        Some(AttrValue::Int(n)) => *n,
        _ => 0,
    };
    let body = node
        .children
        .iter()
        .map(Node::astext)
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{source}:{line}: ({msg_type}/{level}) {body}")
}

/// `LiteralIncludeReader.INVALID_OPTIONS_PAIR` (`code.py:189-203`), in
/// list order — the first present pair names the error.
const LITERALINCLUDE_INVALID_PAIRS: &[(&str, &str)] = &[
    ("lineno-match", "lineno-start"),
    ("lineno-match", "append"),
    ("lineno-match", "prepend"),
    ("start-after", "start-at"),
    ("end-before", "end-at"),
    ("diff", "pyobject"),
    ("diff", "lineno-start"),
    ("diff", "lineno-match"),
    ("diff", "lines"),
    ("diff", "start-after"),
    ("diff", "end-before"),
    ("diff", "start-at"),
    ("diff", "end-at"),
];

/// The literalinclude option bundle, typed the way the reader consumes
/// it. Built by `run_literalinclude` from the converted directive
/// options; unit tests construct it directly — the reader never touches
/// the parser.
#[derive(Clone, Default)]
struct LiteralIncludeOptions {
    /// `Some(None)` is a bare `:dedent:` (full `textwrap.dedent`); the
    /// converter (`optional_int`) rejects negatives.
    dedent: Option<Option<i64>>,
    linenos: bool,
    lineno_start: Option<i64>,
    lineno_match: bool,
    tab_width: Option<i64>,
    language: Option<String>,
    force: bool,
    /// Validated by the option converter. `None` means sphinx's
    /// the configured `source_encoding` (default `'utf-8-sig'`,
    /// probe-pinned in the default-encoding error text).
    encoding: Option<String>,
    pyobject: Option<String>,
    lines: Option<String>,
    start_after: Option<String>,
    end_before: Option<String>,
    start_at: Option<String>,
    end_at: Option<String>,
    prepend: Option<String>,
    append: Option<String>,
    emphasize_lines: Option<String>,
    /// `directives.unchanged` — the EMPTY caption is meaningful (it falls
    /// back to the include path as written). `:name:` has no field: the
    /// glue's `directive_add_name` reads it from the raw options.
    caption: Option<String>,
    classes: Vec<String>,
    /// Already absolute — `run()` rewrites the `:diff:` value through
    /// `env.relfn2path` before the reader sees it (`code.py:454-456`).
    diff: Option<std::path::PathBuf>,
}

impl LiteralIncludeOptions {
    /// `option in self.options` for the INVALID_OPTIONS_PAIR names.
    fn has(&self, name: &str) -> bool {
        match name {
            "lineno-match" => self.lineno_match,
            "lineno-start" => self.lineno_start.is_some(),
            "append" => self.append.is_some(),
            "prepend" => self.prepend.is_some(),
            "start-after" => self.start_after.is_some(),
            "start-at" => self.start_at.is_some(),
            "end-before" => self.end_before.is_some(),
            "end-at" => self.end_at.is_some(),
            "diff" => self.diff.is_some(),
            "pyobject" => self.pyobject.is_some(),
            "lines" => self.lines.is_some(),
            _ => false,
        }
    }
}

/// `LiteralIncludeReader` (`SP/directives/code.py:205-410`): the pure
/// read/filter half of the directive — no parser access, unit-testable
/// against a fixture file. Errors are the exact `ValueError`/`OSError`
/// texts `run()`'s broad `except` turns into ONE reporter warning;
/// logger-channel diagnostics accumulate in `warnings` for the caller to
/// route ([INC §3.4]).
struct LiteralIncludeReader {
    /// Absolute path (`run()` resolves via `env.relfn2path` first).
    filename: std::path::PathBuf,
    options: LiteralIncludeOptions,
    /// Resolved encoding name — the `%r` in the decode-error text.
    encoding: String,
    /// `options.get('lineno-start', 1)`, then adjusted by pyobject /
    /// start / lines filters under `lineno-match`; the node's
    /// unconditional `highlight_args['linenostart']`.
    lineno_start: i64,
    /// Logger-channel warning texts in emit order (out-of-range line
    /// specs, dedent stripping) — never part of an error return.
    warnings: Vec<String>,
}

impl LiteralIncludeReader {
    /// Construction runs `parse_options` — the INVALID_OPTIONS_PAIR
    /// check (`code.py:215-219`). `source_encoding` is the configured
    /// `config.source_encoding`, the `:encoding:` default (`code.py:210`).
    fn new(
        filename: std::path::PathBuf,
        options: LiteralIncludeOptions,
        source_encoding: &str,
    ) -> Result<Self, String> {
        for (option1, option2) in LITERALINCLUDE_INVALID_PAIRS {
            if options.has(option1) && options.has(option2) {
                return Err(format!(
                    "Cannot use both \"{option1}\" and \"{option2}\" options"
                ));
            }
        }
        let encoding = options
            .encoding
            .clone()
            .unwrap_or_else(|| source_encoding.to_string());
        let lineno_start = options.lineno_start.unwrap_or(1);
        Ok(LiteralIncludeReader {
            filename,
            options,
            encoding,
            lineno_start,
            warnings: Vec::new(),
        })
    }

    fn filename_str(&self) -> String {
        self.filename.display().to_string()
    }

    fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// `read` (`code.py:242-259`): diff mode short-circuits the chain;
    /// otherwise the filters run IN THIS ORDER, and the returned count is
    /// post-filter (prepend/append included) — the `emphasize-lines`
    /// denominator.
    fn read(&mut self) -> Result<(String, usize), String> {
        let lines = if self.options.diff.is_some() {
            self.show_diff()?
        } else {
            let mut lines = self.read_file(&self.filename.clone())?;
            lines = self.pyobject_filter(lines)?;
            lines = self.start_filter(lines)?;
            lines = self.end_filter(lines)?;
            lines = self.lines_filter(lines)?;
            lines = self.dedent_filter(lines);
            lines = self.prepend_filter(lines);
            lines = self.append_filter(lines);
            lines
        };
        Ok((lines.concat(), lines.len()))
    }

    /// `read_file` (`code.py:221-240`): open + decode + expandtabs +
    /// `splitlines(True)`, with both probe-pinned error texts.
    fn read_file(&self, filename: &std::path::Path) -> Result<Vec<String>, String> {
        // `filename` arrived already `.resolve()`d — see
        // [`Self::literalinclude_resolve`].
        let bytes = std::fs::read(filename).map_err(|_| {
            format!(
                "Include file '{}' not found or reading it failed",
                filename.display()
            )
        })?;
        // The converter validated the name; the default is always known.
        let encoding = lookup_encoding(&self.encoding).unwrap_or(IncludeEncoding::Utf8Sig);
        let mut text = decode_include_bytes(&bytes, encoding).map_err(|_| {
            format!(
                "Encoding {} used for reading included file '{}' seems to be wrong, \
                 try giving an :encoding: option",
                py_repr(Some(&self.encoding)),
                filename.display()
            )
        })?;
        // Universal newlines: sphinx opens in text mode (newline=None).
        if text.contains('\r') {
            text = text.replace("\r\n", "\n").replace('\r', "\n");
        }
        if let Some(tab_width) = self.options.tab_width {
            // `except Exception` in `LiteralInclude.run` turns the
            // OverflowError into one directive warning (`code.py:505`).
            let tab_width = c_int_tabsize(tab_width)?;
            text = py_expandtabs(&text, tab_width);
        }
        Ok(py_splitlines_keepends(&text))
    }

    /// `show_diff` (`code.py:261-266`): current file first, then the (already
    /// absolute) `:diff:` file — the read order decides which missing-file
    /// error fires when both are gone.
    fn show_diff(&self) -> Result<Vec<String>, String> {
        let new_lines = self.read_file(&self.filename.clone())?;
        let old_filename = self
            .options
            .diff
            .clone()
            .expect("show_diff runs only with the diff option present");
        let old_lines = self.read_file(&old_filename)?;
        Ok(py_unified_diff(
            &old_lines,
            &new_lines,
            &old_filename.display().to_string(),
            &self.filename_str(),
        ))
    }

    /// `pyobject_filter` (`code.py:268-289`): the FIRST chain slot —
    /// `lines[start-1:end]` over the 1-based inclusive tag, and
    /// `lineno-match` ASSIGNS `lineno_start = start`.
    ///
    /// Sphinx builds its tags from a SECOND read of the file
    /// (`ModuleAnalyzer.for_file`, `tokenize.open`); we hand
    /// [`crate::py::pycode::find_tags`] the text this reader already
    /// decoded, which costs nothing in line numbers (see that module's
    /// divergence notes). An analyzer failure carries sphinx's
    /// `parsing %r failed: ` prefix (`SP/pycode/__init__.py:158-160`)
    /// with our own detail in place of the CPython `SyntaxError` repr
    /// sphinx interpolates there.
    fn pyobject_filter(&mut self, lines: Vec<String>) -> Result<Vec<String>, String> {
        let Some(pyobject) = self.options.pyobject.clone() else {
            return Ok(lines);
        };
        let tags = crate::py::pycode::find_tags(&lines.concat()).map_err(|e| {
            format!(
                "parsing {} failed: {e}",
                py_repr(Some(&self.filename_str()))
            )
        })?;
        match tags.get(pyobject.as_str()) {
            Some(&(_, start, end)) => {
                let (from, to) = py_slice(
                    lines.len(),
                    Some(i64::from(start) - 1),
                    Some(i64::from(end)),
                );
                if self.options.lineno_match {
                    self.lineno_start = i64::from(start);
                }
                Ok(lines[from..to].to_vec())
            }
            None => Err(format!(
                "Object named {} not found in include file _StrPath({})",
                py_repr(Some(&pyobject)),
                py_repr(Some(&self.filename_str()))
            )),
        }
    }

    /// `start_filter` (`code.py:324-355`): substring match, first match
    /// wins; `start-at` keeps the matched line, `start-after` drops
    /// through it, each with its own `lineno-match` bias.
    fn start_filter(&mut self, lines: Vec<String>) -> Result<Vec<String>, String> {
        let (start, drop_match) = if let Some(s) = self.options.start_at.clone() {
            (s, false)
        } else if let Some(s) = self.options.start_after.clone() {
            (s, true)
        } else {
            return Ok(lines);
        };
        for (lineno, line) in lines.iter().enumerate() {
            if line.contains(start.as_str()) {
                return if drop_match {
                    if self.options.lineno_match {
                        self.lineno_start += lineno as i64 + 1;
                    }
                    Ok(lines[lineno + 1..].to_vec())
                } else {
                    if self.options.lineno_match {
                        self.lineno_start += lineno as i64;
                    }
                    Ok(lines[lineno..].to_vec())
                };
            }
        }
        Err(if drop_match {
            format!("start-after pattern not found: {start}")
        } else {
            format!("start-at pattern not found: {start}")
        })
    }

    /// `end_filter` (`code.py:357-384`): substring match, first match
    /// wins — except `end-before` IGNORES a match on the very first line
    /// of the current list and keeps scanning (`:375-376`); a first-line
    /// match with no later one therefore raises not-found.
    fn end_filter(&mut self, lines: Vec<String>) -> Result<Vec<String>, String> {
        let (end, keep_match) = if let Some(e) = self.options.end_at.clone() {
            (e, true)
        } else if let Some(e) = self.options.end_before.clone() {
            (e, false)
        } else {
            return Ok(lines);
        };
        for (lineno, line) in lines.iter().enumerate() {
            if line.contains(end.as_str()) {
                if keep_match {
                    return Ok(lines[..lineno + 1].to_vec());
                } else if lineno != 0 {
                    return Ok(lines[..lineno].to_vec());
                }
                // end-before ignores first line
            }
        }
        Err(if keep_match {
            format!("end-at pattern not found: {end}")
        } else {
            format!("end-before pattern not found: {end}")
        })
    }

    /// `lines_filter` (`code.py:291-322`): warn-then-drop out-of-range
    /// members, the lineno-match contiguity gate, Python's negative-index
    /// wrap on selection, and the `_StrPath`-flavored empty-result error
    /// (the class repr leaks into the byte-exact text — hardcoded
    /// wrapper, [INC §3.4]).
    fn lines_filter(&mut self, lines: Vec<String>) -> Result<Vec<String>, String> {
        let Some(linespec) = self.options.lines.clone() else {
            return Ok(lines);
        };
        let total = lines.len() as i64;
        let spec = parse_line_num_spec(&linespec, total)?;
        if spec.any_out_of_range(total) {
            self.warnings.push(format!(
                "line number spec is out of range(1-{}): {}",
                total,
                py_repr(Some(&linespec))
            ));
        }
        if self.options.lineno_match {
            if spec.is_contiguous() {
                self.lineno_start += spec.first();
            } else {
                return Err(
                    "Cannot use \"lineno-match\" with a disjoint set of \"lines\"".to_string(),
                );
            }
        }
        let mut selected = Vec::new();
        for n in spec.in_range_values(total) {
            // Python `lines[n]`: negatives wrap from the end; too far
            // negative is an IndexError whose str() reaches the funnel.
            let index = if n < 0 { n + total } else { n };
            if index < 0 {
                return Err("list index out of range".to_string());
            }
            selected.push(lines[index as usize].clone());
        }
        if selected.is_empty() {
            return Err(format!(
                "Line spec {}: no lines pulled from include file _StrPath({})",
                py_repr(Some(&linespec)),
                py_repr(Some(&self.filename_str()))
            ));
        }
        Ok(selected)
    }

    /// `dedent_filter` (`code.py:404-410`).
    fn dedent_filter(&mut self, lines: Vec<String>) -> Vec<String> {
        match self.options.dedent {
            None => lines,
            Some(dedent) => dedent_lines(lines, dedent, &mut self.warnings),
        }
    }

    /// `prepend_filter` (`code.py:386-393`).
    fn prepend_filter(&self, mut lines: Vec<String>) -> Vec<String> {
        if let Some(prepend) = &self.options.prepend {
            lines.insert(0, format!("{prepend}\n"));
        }
        lines
    }

    /// `append_filter` (`code.py:395-402`).
    fn append_filter(&self, mut lines: Vec<String>) -> Vec<String> {
        if let Some(append) = &self.options.append {
            lines.push(format!("{append}\n"));
        }
        lines
    }
}

/// NumberLines (docutils/utils/code_analyzer.py): a padded 'ln' inline
/// before every line. The number-column width comes from
/// `start + content_len` — docutils computes `endline = startline +
/// len(self.content)` (`body.py:194`), where `content_len` is the number
/// of CONTENT LIST ELEMENTS the caller received, not necessarily the
/// number of rendered lines: the include-called CodeBlock gets its whole
/// file as ONE element (`[text.removesuffix('\n')]`, `misc.py:187-205`),
/// so its column is sized for `start + 1` and comes out ragged
/// (probe-pinned: a 12-line `:code:` `:number-lines:` include renders
/// `1 `…`9 `, `10 ` with no padding). Faithful bug, reproduced.
fn push_number_lines(
    node: &mut Node,
    code_lines: &[String],
    start: i64,
    content_len: usize,
    span: Span,
) {
    let endline = start.saturating_add(content_len as i64);
    let width = endline.to_string().len();
    for (i, line) in code_lines.iter().enumerate() {
        let lineno = start.saturating_add(i as i64);
        let mut ln = Node::elem("inline", span);
        ln.attrs.classes.push("ln".to_string());
        ln.children
            .push(Node::text_node(format!("{lineno:>width$} "), span));
        node.children.push(ln);
        let text = if i + 1 == code_lines.len() {
            line.to_string()
        } else {
            format!("{line}\n")
        };
        node.children.push(Node::text_node(text, span));
    }
}

/// The docutils standard include files (`.. include:: <isonum.txt>`),
/// vendored byte-exact from the pinned docutils 0.22.4 wheel — provenance
/// in src/rst/include/README.md. docutils resolves the `<name>` form
/// against its own installation's `parsers/rst/include/` directory
/// (`misc.py:73,90-92`); this table is that directory.
fn standard_include_text(name: &str) -> Option<&'static str> {
    macro_rules! table {
        ($($file:literal),* $(,)?) => {
            match name {
                $($file => Some(include_str!(concat!("include/", $file))),)*
                _ => None,
            }
        };
    }
    table!(
        "README.rst",
        "html-roles.txt",
        "isoamsa.txt",
        "isoamsb.txt",
        "isoamsc.txt",
        "isoamsn.txt",
        "isoamso.txt",
        "isoamsr.txt",
        "isobox.txt",
        "isocyr1.txt",
        "isocyr2.txt",
        "isodia.txt",
        "isogrk1.txt",
        "isogrk2.txt",
        "isogrk3.txt",
        "isogrk4-wide.txt",
        "isogrk4.txt",
        "isolat1.txt",
        "isolat2.txt",
        "isomfrk-wide.txt",
        "isomfrk.txt",
        "isomopf-wide.txt",
        "isomopf.txt",
        "isomscr-wide.txt",
        "isomscr.txt",
        "isonum.txt",
        "isopub.txt",
        "isotech.txt",
        "mmlalias.txt",
        "mmlextra-wide.txt",
        "mmlextra.txt",
        "s5defs.txt",
        "xhtml1-lat1.txt",
        "xhtml1-special.txt",
        "xhtml1-symbol.txt",
    )
}

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
        "include" => Some(DirectiveSpec {
            required_arguments: 1,
            optional_arguments: 0,
            final_argument_whitespace: true,
            has_content: false,
            option_spec: INCLUDE_OPTS,
            kind: DirectiveKind::Include,
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
    let words: Vec<&str> = crate::utils::py_split(arg_text).collect();
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
    let mut rest = text.trim_start_matches(crate::utils::py_isspace);
    for _ in 0..maxsplit {
        if rest.is_empty() {
            return out;
        }
        match rest.find(crate::utils::py_isspace) {
            Some(i) => {
                out.push(rest[..i].to_string());
                rest = rest[i..].trim_start_matches(crate::utils::py_isspace);
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
    sources: &SourceTable,
    opt_block: &[LineRec],
    option_spec: &'static [(&'static str, Conv)],
) -> Result<Vec<(String, OptVal)>, String> {
    // Pass 1 (extract_options): collect (lowercased name, body) fields.
    // A multi-word field name errors during this pass, in field order.
    let mut fields: Vec<(String, Option<String>)> = Vec::new();
    let mut i = 0usize;
    while i < opt_block.len() {
        let l = opt_block[i];
        let l_text = sources.line_text(l);
        let marker = if l.indent() == 0 {
            field_marker(l_text)
        } else {
            None
        };
        let Some((raw_name, body_start)) = marker else {
            return Err("invalid option block".to_string());
        };
        let mut body_lines: Vec<&str> = Vec::new();
        let first = l_text[body_start..].trim_start_matches(' ');
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
            body_lines.push(&sources.line_text(*c)[min_indent.min(c.indent())..]);
        }
        if crate::utils::py_split(&raw_name).count() != 1 {
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
        Conv::OptionalInt => match value {
            None => Ok(OptVal::Null),
            Some(v) => match py_int_canonical(v) {
                // `int('-0')` is 0, which optional_int accepts —
                // py_int_canonical already reports it non-negative.
                Some((true, _)) => Err("negative value; must be positive or zero".to_string()),
                Some((false, digits)) => Ok(int_optval(false, &digits)),
                None => Err(format!(
                    "invalid literal for int() with base 10: {}",
                    py_repr(Some(v))
                )),
            },
        },
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
                crate::utils::py_split(v).collect()
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
            if lookup_encoding(v).is_none() {
                return Err(format!("unknown encoding: \"{v}\""));
            }
            Ok(OptVal::Str(v.to_string()))
        }
        Conv::FlagOrInt => match value {
            None => Ok(OptVal::Null),
            Some(v) => match py_int_canonical(v) {
                Some((neg, digits)) => Ok(int_optval(neg, &digits)),
                None => Err(format!(
                    "invalid literal for int() with base 10: {}",
                    py_repr(Some(v))
                )),
            },
        },
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
            // `argument.split()` (directives/__init__.py:316).
            for word in crate::utils::py_split(v) {
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
            // `choice`: `argument.lower().strip()` (directives/__init__.py).
            let lowered = v.trim_matches(crate::utils::py_isspace).to_lowercase();
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
    // `int()` strips Unicode White_Space ONLY — probed over all 0x110000
    // codepoints against CPython 3.12: it REJECTS the four C0 separators
    // `\x1c`-`\x1f` that `str.isspace` admits, so Rust's `trim()` is the
    // exactly-right predicate here and `py_isspace` would be wrong.
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

/// Python repr() for option-value error messages (strings and None); the
/// string form is [`crate::utils::py_repr_str`], shared with every
/// warning-stream `%r` site (panel fix round F: `src/env/toctree.rs` had
/// carried a second copy that escaped only `< 0x20` and `0x7f`).
pub(crate) fn py_repr(value: Option<&str>) -> String {
    match value {
        None => "None".to_string(),
        Some(s) => crate::utils::py_repr_str(s),
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
        .map(|p| crate::utils::py_split(&super::inline::unescape(p, false)).collect::<String>())
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
    // `states.escape2null(self.options['target']).splitlines()` then
    // `self.state.parse_target(block, ...)` (images.py) — the same
    // `parse_target` the `.. _name:` construct runs, so an escaped space in
    // the URI survives as a real one.
    let block: Vec<Vec<char>> = crate::utils::py_splitlines(target)
        .into_iter()
        .map(escape2null_chars)
        .collect();
    match parse_target_block(&block) {
        TargetRef::RefName(data) => ImageTarget::Refname {
            name: ids::whitespace_normalize_name(&data),
            refname: ids::fully_normalize_name(&data),
        },
        TargetRef::RefUri(uri) => ImageTarget::Refuri(uri),
    }
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
        // `(?<![\s\x00])\|` — Python `\s` before the closing marker.
        if cs[k].1 != '|' || crate::utils::py_isspace(cs[k - 1].1) {
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

fn dedent_by_min(block: &[LineRec]) -> Vec<LineRec> {
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

/// The suffix of `s` past `skip` CHARACTERS (Python slicing), or `""` when
/// the string is shorter.
fn char_suffix(s: &str, skip: usize) -> &str {
    match s.char_indices().nth(skip) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

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
fn indented_block(
    lines: &[LineRec],
    start: usize,
) -> (Vec<LineRec>, usize, usize, Option<(u16, u32)>) {
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
    let block: Vec<LineRec> = lines[start..block_end]
        .iter()
        .map(|l| if l.is_blank() { *l } else { l.dedented(base) })
        .collect();
    let terminator = lines
        .get(block_end)
        .filter(|l| !l.is_blank())
        .map(|l| (l.source, l.lineno));
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
            // `data[:-3].rstrip()` (states.py:2733-2736) — Python's set, so
            // `abc\x1f ::` yields `abc` (round F, pinned in `round_f`).
            return (
                head.trim_end_matches(crate::utils::py_isspace).to_string(),
                true,
            );
        }
        return (text[..text.len() - 1].to_string(), true);
    }
    (text.to_string(), false)
}

fn attribution_from_chunk(
    sources: &SourceTable,
    chunk: &[LineRec],
    span: Span,
) -> Option<(Node, u32)> {
    let first = chunk.first()?;
    if first.indent() != 0 {
        return None;
    }
    let first_text = sources.line_text(*first);
    // Fixture-verified marker rules: `--`/`---` (not followed by another
    // hyphen) or an em dash, then ZERO or more spaces (all consumed), then
    // non-space text.
    let after = match first_text.strip_prefix('\u{2014}') {
        Some(r) => r,
        None => {
            // `---` then `--`; a further hyphen means an adornment, not a
            // marker. The `---` arm runs first, so the `--` arm's remainder
            // can only start with `-` for exactly `---x`-shaped input.
            let r = first_text
                .strip_prefix("---")
                .or_else(|| first_text.strip_prefix("--"))?;
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
            text.push_str(&sources.line_text(*l)[indent..]);
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

/// docutils `escape2null` (utils/__init__.py:657-668): a backslash becomes
/// `\x00` and the character after it is kept verbatim (a trailing backslash
/// leaves a lone `\x00`). Char vector, because every index in the target
/// grammar below is a Python character index.
fn escape2null_chars(s: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            out.push('\u{0}');
            if let Some(n) = it.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// docutils `nodes.unescape(text)` (nodes.py:2925-2939) with
/// `restore_backslashes=False`: drop `\x00 ` and `\x00\n` WHOLE (an escaped
/// space disappears, taking the space with it), then drop bare `\x00`.
fn unescape_nulls(s: &str) -> String {
    let mut t = s.to_string();
    for sep in ["\u{0} ", "\u{0}\n", "\u{0}"] {
        t = t.split(sep).collect::<String>();
    }
    t
}

/// Python `str.strip()` — [`crate::utils::py_isspace`], not Rust's set.
fn py_strip(s: &str) -> &str {
    s.trim_matches(crate::utils::py_isspace)
}

/// `''.join(text.split())` — every Python-whitespace run removed.
fn py_split_concat(s: &str) -> String {
    s.split(crate::utils::py_isspace).collect()
}

/// docutils `split_escaped_whitespace` (utils/__init__.py:671-679).
fn split_escaped_whitespace(text: &str) -> Vec<String> {
    text.split("\u{0} ")
        .flat_map(|part| part.split("\u{0}\n"))
        .map(str::to_string)
        .collect()
}

/// The tail shared by both branches of the `target` pattern
/// (states.py:1972-1977), starting at char index `p`:
/// `(?<!(?<!\x00):)(?<![\s\x00])[ ]?:([ ]+|$)`. Returns the match end.
fn target_pattern_tail(e: &[char], p: usize) -> Option<usize> {
    // `(?<!(?<!\x00):)` — no UNESCAPED colon at the end of the name.
    if p >= 1 && e[p - 1] == ':' && !(p >= 2 && e[p - 2] == '\u{0}') {
        return None;
    }
    // `non_whitespace_escape_before` = `(?<![\s\x00])` (states.py:780): the
    // name may end neither in Python whitespace nor in an escape null.
    if p >= 1 && (crate::utils::py_isspace(e[p - 1]) || e[p - 1] == '\u{0}') {
        return None;
    }
    // `[ ]?` is greedy: try the one optional space first, then none.
    for skip in [1usize, 0usize] {
        if skip == 1 && e.get(p) != Some(&' ') {
            continue;
        }
        let colon = p + skip;
        if e.get(colon) != Some(&':') {
            continue;
        }
        let mut c = colon + 1;
        if c < e.len() && e[c] == ' ' {
            while c < e.len() && e[c] == ' ' {
                c += 1;
            }
            return Some(c);
        }
        if c == e.len() {
            return Some(c);
        }
    }
    None
}

/// `explicit.patterns.target` (states.py:1959-1978), run by hand because the
/// pattern needs variable-order backtracking and two lookbehinds:
///
/// ```text
/// ( _ | (?!_)(?P<quote>`?)(?![ `])(?P<name>.+?)(?<![\s\x00])(?P=quote) )
/// (?<!(?<!\x00):)(?<![\s\x00])[ ]?:([ ]+|$)
/// ```
///
/// Input is the escape2null'd text AFTER the construct's `_`. Returns the
/// `name` group (`None` = the anonymous `_` branch) and the char index the
/// match ends at. Alternation order — anonymous first, then the greedy
/// quote, then the non-greedy name shortest-first — is the engine's, and it
/// decides which name wins (`` _`a`b`: `` → ``a`b``).
fn match_target_pattern(e: &[char]) -> Option<(Option<String>, usize)> {
    if e.first() == Some(&'_') {
        // The anonymous branch consumes exactly one `_`; `(?!_)` then bars
        // the named branch, so a failed tail means no match at all.
        return target_pattern_tail(e, 1).map(|end| (None, end));
    }
    if e.is_empty() {
        return None;
    }
    let quote_lens: &[usize] = if e[0] == '`' { &[1, 0] } else { &[0] };
    for &q in quote_lens {
        // `(?![ `])`
        match e.get(q) {
            Some(&c) if c != ' ' && c != '`' => {}
            _ => continue,
        }
        // `(?P<name>.+?)` — at least one char, shortest first. `.` never
        // matches a newline, and the joined block never holds one.
        for n in q + 1..=e.len() {
            let prev = e[n - 1];
            if crate::utils::py_isspace(prev) || prev == '\u{0}' {
                continue; // `(?<![\s\x00])`
            }
            if q == 1 && e.get(n) != Some(&'`') {
                continue; // `(?P=quote)`
            }
            if let Some(end) = target_pattern_tail(e, n + q) {
                return Some((Some(e[q..n].iter().collect()), end));
            }
        }
    }
    None
}

/// What `Body.parse_target` returns (states.py:2095-2113).
enum TargetRef {
    RefName(String),
    RefUri(String),
}

/// docutils `Body.parse_target` over the escaped link block.
fn parse_target_block(block: &[Vec<char>]) -> TargetRef {
    let stripped: Vec<String> = block
        .iter()
        .map(|l| {
            let s: String = l.iter().collect();
            py_strip(&s).to_string()
        })
        .collect();
    if stripped.last().is_some_and(|l| l.ends_with('_')) {
        if let Some(data) = is_reference(&stripped.join(" ")) {
            // The RAW `data`; `make_target` normalizes, and the `image`
            // `:target:` caller needs both normalizations of it.
            return TargetRef::RefName(data);
        }
    }
    let joined: Vec<String> = block.iter().map(|l| l.iter().collect()).collect();
    let parts = split_escaped_whitespace(&joined.join(" "));
    let reference: Vec<String> = parts
        .iter()
        .map(|p| py_split_concat(&unescape_nulls(p)))
        .collect();
    TargetRef::RefUri(reference.join(" "))
}

/// docutils `Body.is_reference` (states.py:2115-2120) plus
/// `explicit.patterns.reference` (states.py:1980-1994):
/// `((?P<simple>simplename)_|`(?![ ])(?P<phrase>.+?)(?<![\s\x00])`_)$`
/// against the whitespace-normalized (still escaped) reference.
fn is_reference(reference: &str) -> Option<String> {
    let chars: Vec<char> = ids::whitespace_normalize_name(reference).chars().collect();
    let n = chars.len();
    if n < 2 || chars[n - 1] != '_' {
        return None;
    }
    // `simplename_$`: the `$` pins the name to exactly `chars[..n-1]`, and a
    // greedy scan that covers the whole prefix is the only parse that can.
    if match_simplename_chars(&chars[..n - 1], 0) == Some(n - 1) {
        return Some(unescape_nulls(&chars[..n - 1].iter().collect::<String>()));
    }
    // `` `phrase`_$ `` — `$` pins the phrase to `chars[1..n-2]` too.
    if n >= 4
        && chars[0] == '`'
        && chars[n - 2] == '`'
        && chars[1] != ' '
        && !crate::utils::py_isspace(chars[n - 3])
        && chars[n - 3] != '\u{0}'
    {
        return Some(unescape_nulls(&chars[1..n - 2].iter().collect::<String>()));
    }
    None
}

// ----------------------------------------------------------------------
// tests (plan tasks 7-12; expectations probe-verified against docutils
// 0.22.4 parse-layer output — see 2026-08-07-m2-wave1-probes.md)
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst::{parse_rst, ParseOptions};

    /// The `(source, lineno)` sequence of a line stream.
    fn stream_of(lines: &[LineRec]) -> Vec<(u16, u32)> {
        lines.iter().map(|l| (l.source, l.lineno)).collect()
    }

    /// sphinx's `ws_re` (`\s+`) is Python's `str.isspace`: the C0
    /// separator `\x1f` collapses like a space in the std-domain names
    /// (`.. envvar::`/`.. confval::` to `' '`, `.. program::` to `'-'`).
    /// Sphinx 9.1.0 bytes: oracle cases `sx_std.*_python_whitespace_name`
    /// (panel fix round D).
    #[test]
    fn ws_collapse_treats_python_whitespace_as_a_run() {
        assert_eq!(ws_collapse("FOO\x1fBAR", " "), "FOO BAR");
        assert_eq!(ws_collapse("git\x1fadd", "-"), "git-add");
        assert_eq!(ws_collapse("a \x1f\t b", "-"), "a-b");
        assert_eq!(ws_collapse("plain", " "), "plain");
    }

    /// A substitution definition keeps docutils' `rawsource`, the
    /// `blocktext` it is created from (`states.py:2146,2169`: the marker
    /// line and the indented block, indentation kept) — what a circular
    /// definition's `literal_block` holds (docutils 0.22.4, oracle case
    /// `tx_subst.circular`), here with a continuation line.
    #[test]
    fn a_substitution_definition_keeps_its_rawsource() {
        let tree = parse_rst(
            "Para.\n\n.. |a| replace::\n   x |a|\n\nAfter.\n",
            &ParseOptions::default(),
        );
        let definition = &tree.root.children[1];
        assert_eq!(definition.kind, "substitution_definition");
        assert_eq!(
            definition.get(crate::doctree::RAWSOURCE),
            Some(&AttrValue::Str(".. |a| replace::\n   x |a|".to_string()))
        );
    }

    /// A hyperlink target keeps docutils' `rawsource`, the `blocktext` it
    /// is created from (`hyperlink_target`, `states.py:2055-2078`: the
    /// marker line from its start, then the block's further lines with
    /// their indentation; `anonymous_target`, `:2530-2538`: the `__ ` line,
    /// the block dedented) — what IndirectHyperlinks prints in the
    /// `problematic` that replaces a target referring to a failed one
    /// (oracle `tx_links.indirect_circular`: `.. _b: a_`).
    #[test]
    fn a_hyperlink_target_keeps_its_rawsource() {
        let tree = parse_rst(
            ".. _a: b_\n.. _c:\n   https://x.example/\n.. __: d_\n\n__ e_\n\nPara.\n",
            &ParseOptions::default(),
        );
        let rawsources: Vec<Option<&AttrValue>> = tree
            .root
            .children
            .iter()
            .filter(|node| node.kind == kinds::TARGET)
            .map(|node| node.get(crate::doctree::RAWSOURCE))
            .collect();
        let raw = |text: &str| Some(AttrValue::Str(text.to_string()));
        assert_eq!(
            rawsources,
            [
                raw(".. _a: b_").as_ref(),
                raw(".. _c:\n   https://x.example/").as_ref(),
                raw(".. __: d_").as_ref(),
                raw("__ e_").as_ref(),
            ]
        );
    }

    /// Where docutils' reporter locates a message raised after the parse
    /// with no node (research §9.3): its `get_source_and_line()` is the
    /// finished top-level state machine's, whose cursor stops one past the
    /// last line ("just past the end", `statemachine.py:1299-1307`) —
    /// unless the last construct was one whose nested list parse ran to the
    /// end of the input and moved the cursor there with `goto_line`, after
    /// which the run loop's `next_line` steps beyond it and `info()` raises
    /// `IndexError`: no source, no line (`:369-377`). Those constructs are
    /// the explicit markup list (`states.py:2508-2519`), bullet, enumerated,
    /// field and option lists (`:1373-1387,1407-1439,1564-1576,1607-1634`),
    /// a line block whose first line is not blank-finished (`:1708-1725`),
    /// a definition list (`:2859-2874`) and a quoted literal block running
    /// to the end of the input (`:2961-2970`; a blank line after it ends
    /// its nested parse early, `:3205-3209`); a block quote, a table, a
    /// doctest block, a paragraph, a literal block and a section title
    /// leave the cursor at the end. The explicit markup list and a line
    /// block's nested parse stop at a blank line (`Explicit.blank`,
    /// `LineBlock.blank` are `invalid_input`, `:2773,2803`), so the last
    /// construct must take any trailing blank lines itself: a comment, a
    /// directive, a footnote, a citation and a substitution definition do
    /// (their blocks are read without `until_blank`), but a hyperlink target
    /// and an anonymous target (`until_blank=True`, `:2058-2059,2532-2533`),
    /// an empty comment and the end-of-inclusion marker (which read nothing,
    /// `:2427-2433`) and a line block's lines leave them to the top level,
    /// which ends one past them. One more: a `::` paragraph with no line
    /// at all after it finds no literal block, and the empty quoted parse
    /// steps back (`previous_line`, `:3211-3225`) onto the last line — the
    /// location is that line. Only the top level counts: a section's
    /// content is parsed by the top-level machine itself (`new_subsection`,
    /// `:494-508`), and a list inside a block quote or a table cell moves
    /// only a nested machine's cursor. Probed (oracle
    /// `tx_links.anonymous_mismatch_*`).
    #[test]
    fn the_end_of_input_has_no_line_after_a_nested_list_parse_ran_to_the_end() {
        let end = |source: &str| {
            crate::rst::parse_rst_full(source, &ParseOptions::default()).end_of_input
        };
        for source in [
            "Para.\n\n.. comment\n",
            "Para.\n\n.. _t: https://x/\n",
            "Para.\n\n.. note:: y\n",
            "Para.\n\n.. |s| replace:: s\n",
            "Para.\n\n__ https://x/\n",
            "Para.\n\n.. comment\n\n\n",
            "Title\n=====\n\n.. _t:\n",
            "Para.\n\n- a\n- b\n",
            "Para.\n\n- a\n\n",
            "Para.\n\n#. one\n",
            "Para.\n\n:f: v\n",
            "Para.\n\n-o  opt\n",
            "Para.\n\nterm\n   def\n",
            "Para.\n\n| one\n| two\n",
            "Para.\n\n::\n\n> quoted\n",
            "---\n    x\n",
            "Para.\n\n..\n",
            "Para.\n\n.. c\n\n",
            "Para.\n\n.. [1] f\n\n",
            "Para.\n\n.. |s| replace:: S\n\n",
            "Para.\n\n.. note:: y\n\n",
            "Para.\n\n..\n   indented\n\n",
            "Para.\n\n.. _t: https://x/\n.. c\n\n",
            "Para.\n\n| one\n| two\n   cont\n",
            "Para.\n\n- a\n\n\n",
            "Para.\n\nterm\n   def\n\n",
            "Para.\n\n:f: v\n\n",
            "Para.\n\n-o  opt\n\n",
            "Para.\n\n#. one\n\n",
        ] {
            assert_eq!(end(source), None, "{source:?}");
        }
        for (source, line) in [
            ("Para.\n", 2),
            ("Para.", 2),
            ("Para.\n\n\n\n", 5),
            ("Title\n=====\n", 3),
            ("Para.\n\n| one\n", 4),
            ("Para.\n\n| one\n\n", 5),
            ("Para.\n\n| one\n   cont\n", 5),
            ("Para.\n\n>>> 1\n1\n", 5),
            ("Para.\n\n+---+\n| a |\n+---+\n", 6),
            ("Para.\n\n===  ===\na    b\n===  ===\n", 6),
            ("Para::\n\n    code\n", 4),
            ("Para.\n\n    quote\n", 4),
            ("Para.\n\n    - a\n", 4),
            ("Para.\n\n+-----+\n| - a |\n+-----+\n", 6),
            ("- a\n\nPara.\n", 4),
            (".. note:: y\n\nPara.\n", 4),
            ("* a\n\n  .. c\n\nTitle\n=====\n\ntext\n", 9),
            ("Para.\n\n::\n\n> quoted\n\n", 7),
            ("Para.\n\n::\n\n> a\n< b\n", 7),
            ("Para.\n\n::\n\n> a\n    b\n", 7),
            ("Para::\n\n", 3),
            ("Para.\n\nB::\n\n\n", 6),
            ("Para.\n\nB::\n", 3),
            ("Para::\n", 1),
            ("Para.\n\n.. _t: https://x/\n\n", 5),
            ("Para.\n\n.. _t: https://x/\n\n\n", 6),
            ("Para.\n\n.. _t:\n\n", 5),
            ("Para.\n\n.. __: https://x/\n\n", 5),
            ("Para.\n\n__ https://x/\n\n", 5),
            ("Para.\n\n.. _t: https://x/\n   more\n\n", 6),
            ("Para.\n\n.. c\n.. _t: https://x/\n\n", 6),
            ("Para.\n\n..\n\n", 5),
            ("Para.\n\n| one\n| two\n\n", 6),
            ("Para.\n\n| one\n| two\n   cont\n\n", 7),
            ("Title\n=====\n\n.. _t: https://x/\n\n", 6),
        ] {
            assert_eq!(end(source), Some((0, line)), "{source:?}");
        }
    }

    /// Run `explicit.patterns.target` over the text after the construct's
    /// `_`, the way `hyperlink_target` does.
    fn target_of(after_underscore: &str) -> Option<(Option<String>, usize)> {
        match_target_pattern(&escape2null_chars(after_underscore))
    }

    /// docutils' hyperlink-target construct is `\.\.[ ]+_(?![ ]|$)`: a
    /// space or end-of-line after `_` makes the block a comment, never a
    /// malformed target; inside a backtick phrase a leading space, or a
    /// space/newline before the closing quote, IS malformed
    /// (`(?![ `])` … `(?<![\s\x00])(?P=quote)`, states.py:780 — Python's
    /// `\s`, so a NBSP or `\x1f` there is malformed too). docutils 0.22.4
    /// bytes: fixture families `round_d`/`round_e`.
    #[test]
    fn target_marker_rejects_quoted_names_padded_with_spaces() {
        assert!(target_of("` x`: https://x/").is_none());
        assert!(target_of("`x `: https://x/").is_none());
        assert!(target_of("`x\u{a0}`: https://x/").is_none());
        assert!(target_of("`x\u{1f}`: https://x/").is_none());
        let (name, _) = target_of("`x y`: https://x/").expect("well-formed");
        assert_eq!(name.as_deref(), Some("x y"));
        // Non-greedy `.+?`: the FIRST closing backtick that lets the tail
        // match wins, so an inner backtick can land inside the name.
        let (name, _) = target_of("`a`b`: https://x/").expect("well-formed");
        assert_eq!(name.as_deref(), Some("a`b"));
    }

    /// The tail is `(?<![\s\x00])[ ]?:([ ]+|$)`: at most ONE space before
    /// the colon, and the name may not end in Python whitespace.
    #[test]
    fn target_pattern_tail_allows_exactly_one_space_before_the_colon() {
        let (name, end) = target_of("pad  lbl :").expect("one space is fine");
        assert_eq!(name.as_deref(), Some("pad  lbl"));
        assert_eq!(end, 10);
        assert!(target_of("pad  lbl  :").is_none(), "two spaces: malformed");
        assert!(target_of("lbl   :").is_none());
        assert!(target_of("x\u{a0}:").is_none(), "NBSP is Python whitespace");
        assert!(target_of("x\u{1f}:").is_none(), "and so is \\x1f");
        // The anonymous branch takes the same tail.
        let (name, _) = target_of("_ :").expect("`.. __ :` is anonymous");
        assert!(name.is_none());
        assert!(target_of("_  :").is_none());
    }

    #[test]
    fn splicing_a_pushed_source_inserts_its_recs_at_the_cursor() {
        let mut p = BlockParser::new("one\ntwo\nthree", "<doc>");
        let mut stream = std::mem::take(&mut p.top);
        assert_eq!(stream_of(&stream), vec![(0, 1), (0, 2), (0, 3)]);

        p.apply_splice(
            &mut stream,
            1,
            SpliceRequest::single(
                vec!["alpha".to_string(), "beta".to_string()],
                "inc.rst".to_string(),
            ),
        );

        assert_eq!(
            stream_of(&stream),
            vec![(0, 1), (1, 1), (1, 2), (0, 2), (0, 3)],
            "the pushed source's lines join the stream at the cursor"
        );
        assert_eq!(p.sources.len(), 2, "source_texts gained the new entry");
        assert_eq!(p.sources.path(1), "inc.rst");
        assert_eq!(p.sources.text(1), "alpha\nbeta");
    }

    #[test]
    fn a_directive_returned_splice_parses_at_the_cursor_with_its_own_provenance() {
        // The cfg(test) splice directive returns a SpliceRequest built from
        // its input alone; the block-parse loop must insert its lines right
        // after the directive and keep parsing.
        let src =
            "before\n\n.. sphinx-ultra-test-splice:: inc.rst\n\n   alpha\n\n   beta\n\nafter\n";
        let tree = parse_rst(
            src,
            &ParseOptions {
                source_path: "<doc>".into(),
                sphinx: false,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                py: Default::default(),
                srcdir: None,
                found_docs: None,
                ..Default::default()
            },
        );
        assert_eq!(
            tree.sources,
            vec!["<doc>".to_string(), "inc.rst".to_string()]
        );
        let paras: Vec<(String, u16, u32)> = tree
            .root
            .children
            .iter()
            .map(|n| (n.astext(), n.span.source, n.span.line))
            .collect();
        assert_eq!(
            paras,
            vec![
                ("before".to_string(), 0, 1),
                ("alpha".to_string(), 1, 1),
                ("beta".to_string(), 1, 3),
                ("after".to_string(), 0, 9),
            ],
            "spliced paragraphs carry the pushed source's id and 1-based lines"
        );
    }

    #[test]
    fn a_title_underline_warning_stamps_the_recs_table_path_and_lineno() {
        // "====" is >= 4 chars but shorter than the title: the section forms
        // with a "Title underline too short." WARNING whose source/line come
        // from the underline REC — its table path and lineno — not from a
        // recount of the document text.
        let tree = parse_rst(
            "badly\n====\n",
            &ParseOptions {
                source_path: "<doc>".into(),
                sphinx: false,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                py: Default::default(),
                srcdir: None,
                found_docs: None,
                ..Default::default()
            },
        );
        let section = &tree.root.children[0];
        let msg = section
            .children
            .iter()
            .find(|n| n.kind == kinds::SYSTEM_MESSAGE)
            .expect("short underline warns");
        assert_eq!(
            msg.get("source"),
            Some(&AttrValue::Str(tree.sources[0].clone())),
            "message source = the rec's table path"
        );
        assert_eq!(
            msg.get("line"),
            Some(&AttrValue::Int(2)),
            "message line = the underline rec's lineno"
        );
    }

    fn pf(src: &str) -> String {
        parse_rst(
            src,
            &ParseOptions {
                source_path: "<snippet>".into(),
                sphinx: false,
                docname: "index".into(),
                exclude_patterns: Vec::new(),
                py: Default::default(),
                srcdir: None,
                found_docs: None,
                ..Default::default()
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
                py: Default::default(),
                srcdir: None,
                found_docs: None,
                ..Default::default()
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

    /// docutils' `figname` (`images.py:125`, `:155-157`) is the figure's
    /// OWN explicit target, applied BEFORE the caption/legend parse — which
    /// is why sphinx's `Figure.run` (`patches.py:33`) can keep re-applying
    /// the popped `:name:` afterwards without either knowing about the
    /// other. It is `directives.unchanged`, so `if figname:` makes a
    /// valueless `:figname:` a no-op. Expectations pasted from
    /// `probe_figname.py` / `probe_figname_sx.py` (fix round 1); the
    /// docutils-mode shapes are also pinned in the docutils fixture corpus,
    /// which the sphinx corpus cannot carry (`candidates=` on every image).
    #[test]
    fn figname_targets_the_figure_itself_in_both_modes() {
        let src = ".. figure:: pic.png\n   :figname: my fig\n\n   Caption.\n";
        for got in [pf(src), pf_sphinx(src)] {
            assert!(
                got.contains(r#"<figure ids="my-fig" names="my\ fig">"#),
                "{got}"
            );
            assert!(got.contains("<image uri=\"pic.png\">"), "{got}");
        }
        // ... and it does NOT reach the inner image, where `:name:` lands
        // in docutils mode: the two can be set independently.
        let both = pf(".. figure:: pic.png\n   :figname: my fig\n   :name: other\n\n   Caption.\n");
        assert_eq!(
            both,
            "<document source=\"<snippet>\">\n    <figure ids=\"my-fig\" names=\"my\\ fig\">\n        <image ids=\"other\" names=\"other\" uri=\"pic.png\">\n        <caption>\n            Caption.\n"
        );
        // `if figname:` — an empty value is falsy, so nothing is stamped
        // (`Directive.add_name` has no such guard for `:name:`).
        assert_eq!(
            pf(".. figure:: pic.png\n   :figname:\n\n   Caption.\n"),
            "<document source=\"<snippet>\">\n    <figure>\n        <image uri=\"pic.png\">\n        <caption>\n            Caption.\n"
        );
        // The whole point of the fix: `figname` is a KNOWN option, so it
        // no longer trips the unknown-option error.
        assert!(!pf(src).contains("system_message"), "{}", pf(src));
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

    /// `EnvVarXRefRole.result_nodes` runs only when `is_ref`, which
    /// `XRefRole` clears for a `!`-prefixed role text. The index entries and
    /// the `index-N` target are the visible half; the invisible half is the
    /// serial, which is document-wide — burning one on a suppressed
    /// reference would renumber every later `index-N` id in the file.
    #[test]
    fn a_suppressed_envvar_reference_consumes_no_index_serial() {
        let live = pf_sphinx("See :envvar:`HOME_A` here.\n\n.. index:: Something\n");
        assert!(live.contains(r#"<target ids="index-0">"#), "{live}");
        assert!(
            live.contains(
                r#"<index entries="('single',\ 'Something',\ 'index-1',\ '',\ None)" inline="0">"#
            ),
            "a live :envvar: takes index-0, so the directive gets index-1:\n{live}"
        );

        let suppressed = pf_sphinx("See :envvar:`!HOME_A` here.\n\n.. index:: Something\n");
        assert!(
            !suppressed.contains("environment variable;"),
            "a suppressed reference emits no index entries:\n{suppressed}"
        );
        assert!(
            suppressed.contains(
                r#"<index entries="('single',\ 'Something',\ 'index-0',\ '',\ None)" inline="0">"#
            ),
            "and no serial, so the directive still gets index-0:\n{suppressed}"
        );
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
                py: Default::default(),
                srcdir: None,
                found_docs: None,
                ..Default::default()
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

    // ------------------------------------------------------------------
    // glossary misformat warnings (M2 wave 4.5 task 16)
    //
    // Every expectation below is pasted from a Sphinx 9.1.0 harness3 probe
    // run (probe_glossary / probe_glossary2, conventions per
    // tools/gen_sphinx_fixture.py) — never written from memory. The shapes
    // the sphinx-doctree fixture can carry are ALSO committed there; these
    // unit tests additionally pin the shapes the fixture cannot (the
    // ones whose oracle output the corpus generator would have to grow new
    // SUPPORTED_KINDS for) and document the mechanism.
    // ------------------------------------------------------------------

    /// A term line pressed straight against the previous definition body
    /// warns AND still opens a new entry. Note `line="4"` for a term on
    /// document line 5: Sphinx reports these three warnings one line low
    /// (0-based `content.items` offset rendered as a 1-based line).
    #[test]
    fn a_glossary_term_without_a_preceding_blank_line_warns() {
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n      def A\n   term B\n      def B\n"),
            "<document source=\"<snippet>\">\n    <system_message level=\"2\" line=\"4\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            glossary term must be preceded by empty line\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        def A\n            <definition_list_item>\n                <term ids=\"term-term-B\">\n                    term B\n                    <index entries=\"('single',\\ 'term\\ B',\\ 'term-term-B',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        def B\n"
        );
    }

    /// Terms separated BY a blank line warn and still share ONE entry —
    /// `was_empty` does not reset `in_definition`, so the second term joins
    /// the first term's `definition_list_item`.
    #[test]
    fn glossary_terms_separated_by_a_blank_line_warn_and_share_one_entry() {
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n\n   term B\n      def AB\n"),
            "<document source=\"<snippet>\">\n    <system_message level=\"2\" line=\"4\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            glossary terms must not be separated by empty lines\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <term ids=\"term-term-B\">\n                    term B\n                    <index entries=\"('single',\\ 'term\\ B',\\ 'term-term-B',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        def AB\n"
        );
    }

    /// The warning fires once per offending term, not once per glossary.
    #[test]
    fn every_blank_separated_glossary_term_warns() {
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n\n   term B\n\n   term C\n      def\n"),
            "<document source=\"<snippet>\">\n    <system_message level=\"2\" line=\"4\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            glossary terms must not be separated by empty lines\n    <system_message level=\"2\" line=\"6\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            glossary terms must not be separated by empty lines\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <term ids=\"term-term-B\">\n                    term B\n                    <index entries=\"('single',\\ 'term\\ B',\\ 'term-term-B',\\ 'main',\\ None)\">\n                <term ids=\"term-term-C\">\n                    term C\n                    <index entries=\"('single',\\ 'term\\ C',\\ 'term-term-C',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        def\n"
        );
    }

    /// An indented line before any term — reachable only when the content
    /// is MIXED-indent, because the directive machinery strips the common
    /// indent first (a uniformly over-indented glossary body is simply a
    /// list of terms, probe `indented_start_no_entries`).
    #[test]
    fn an_indented_glossary_line_with_no_term_yet_warns_about_indentation() {
        assert_eq!(
            pf_sphinx(".. glossary::\n\n      stray indented line\n\n   term A\n      def A\n"),
            "<document source=\"<snippet>\">\n    <system_message level=\"2\" line=\"2\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            glossary seems to be misformatted, check indentation\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        def A\n"
        );
    }

    /// A comment between two terms keeps them in ONE entry and warns about
    /// nothing: `Glossary.run` `continue`s on a comment line before
    /// clearing `was_empty`. This is the wave-4 backlog minor ("glossary
    /// comment splits a multi-term entry, producing an empty <definition>
    /// shape docutils never emits") — the state-machine port closes it.
    #[test]
    fn a_comment_between_glossary_terms_does_not_split_the_entry() {
        let expected = "<document source=\"<snippet>\">\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <term ids=\"term-term-B\">\n                    term B\n                    <index entries=\"('single',\\ 'term\\ B',\\ 'term-term-B',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        shared def\n";
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n   .. a comment\n   term B\n      shared def\n"),
            expected
        );
        // ... and the comment's indented continuation lines go with it.
        assert_eq!(
            pf_sphinx(
                ".. glossary::\n\n   term A\n   .. comment\n      swallowed\n   term B\n      shared def\n"
            ),
            expected
        );
    }

    /// A comment AFTER a definition body does not suppress the
    /// missing-blank-line warning, because it never clears `was_empty`.
    #[test]
    fn a_comment_after_a_glossary_definition_still_warns_on_the_next_term() {
        assert_eq!(
            pf_sphinx(
                ".. glossary::\n\n   term A\n      def A\n   .. comment\n   term B\n      def B\n"
            ),
            "<document source=\"<snippet>\">\n    <system_message level=\"2\" line=\"5\" source=\"<snippet>\" type=\"WARNING\">\n        <paragraph>\n            glossary term must be preceded by empty line\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        def A\n            <definition_list_item>\n                <term ids=\"term-term-B\">\n                    term B\n                    <index entries=\"('single',\\ 'term\\ B',\\ 'term-term-B',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        def B\n"
        );
    }

    /// `line[indent_len:]` is a RAW slice: the entry's indentation is fixed
    /// by its first definition line, and a later line indented less loses
    /// characters rather than being re-dedented to the minimum. Probe
    /// `under_indented_continuation`: `      shallow` under a nine-column
    /// first line renders `llow`.
    #[test]
    fn a_glossary_definition_dedents_by_its_first_line_not_the_minimum() {
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n         deep def\n      shallow\n"),
            "<document source=\"<snippet>\">\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        deep def\n                        llow\n"
        );
    }

    /// `line[indent_len:]` is a Python CHARACTER slice, so the offset may
    /// not be applied as a byte count: a non-ASCII continuation line either
    /// panics on a split multi-byte char or silently keeps one character
    /// too many. Every expectation pasted from `probe_gloss_utf8.py`
    /// (harness3, Sphinx 9.1.0).
    #[test]
    fn a_glossary_definition_dedent_counts_characters_not_bytes() {
        // indent_len 3 lands INSIDE the two-byte 'é' of `  éx` — a byte
        // slice panics here ("byte index 3 is not a char boundary").
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n      deep\n     éx\n"),
            "<document source=\"<snippet>\">\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        deep\n                        x\n"
        );
        // No panic, but the wrong text: `  ébcdef` under indent_len 4 is
        // `cdef` by characters and `bcdef` by bytes.
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n       deep\n     ébcdef\n"),
            "<document source=\"<snippet>\">\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        deep\n                        cdef\n"
        );
        // The clamp is on the CHARACTER length: `  é` is three characters
        // under an indent_len of 7, so the whole line goes.
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n          deep\n     é\n"),
            "<document source=\"<snippet>\">\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        deep\n"
        );
        // The multi-byte char sits exactly ON the boundary and survives.
        assert_eq!(
            pf_sphinx(".. glossary::\n\n   term A\n      deep\n      éx\n"),
            "<document source=\"<snippet>\">\n    <glossary sorted=\"0\">\n        <definition_list classes=\"glossary\">\n            <definition_list_item>\n                <term ids=\"term-term-A\">\n                    term A\n                    <index entries=\"('single',\\ 'term\\ A',\\ 'term-term-A',\\ 'main',\\ None)\">\n                <definition>\n                    <paragraph>\n                        deep\n                        éx\n"
        );
    }

    /// The well-formed shapes stay warning-free — the guard against a state
    /// machine that warns on everything.
    #[test]
    fn well_formed_glossaries_do_not_warn() {
        for src in [
            ".. glossary::\n\n   term A\n      def A\n\n   term B\n      def B\n",
            ".. glossary::\n\n   term A\n   term B\n      shared def\n",
            ".. glossary::\n\n   .. lead comment\n   term A\n      def A\n",
            ".. glossary::\n\n   term A\n      def A\n   \n      more A\n",
            ".. glossary::\n\n   ..\n      def\n",
        ] {
            assert!(
                !pf_sphinx(src).contains("system_message"),
                "well-formed glossary warned: {src:?}"
            );
        }
    }
}

/// The `py:*` directive family (M2 wave 4.5 task 6). Every expected
/// pformat below is pasted verbatim from the Sphinx 9.1.0 oracle probes —
/// the research spec [PY §1.6/1.7] and this task's probe_t6 run (harness3
/// conventions, pinned wheels) — never written from memory.
#[cfg(test)]
mod py_desc_tests {
    use super::*;
    use crate::py::PySigConfig;
    use crate::rst::{parse_rst_full, ParseOptions, ParseOutput};

    fn py_opts(py: PySigConfig) -> ParseOptions {
        ParseOptions {
            source_path: "<snippet>".into(),
            sphinx: true,
            docname: "index".into(),
            exclude_patterns: Vec::new(),
            py,
            found_docs: None,
            srcdir: None,
            ..Default::default()
        }
    }

    fn parse_py(src: &str) -> ParseOutput {
        parse_rst_full(src, &py_opts(PySigConfig::default()))
    }

    fn pf_py(src: &str) -> String {
        parse_py(src).doctree.root.pformat()
    }

    fn pf_py_cfg(src: &str, py: PySigConfig) -> String {
        parse_rst_full(src, &py_opts(py)).doctree.root.pformat()
    }

    /// `(fullname, objtype, node_id, aliased)` of every py object record.
    fn objects(out: &ParseOutput) -> Vec<(String, String, String, bool)> {
        out.registry
            .py_objects
            .iter()
            .map(|r| {
                (
                    r.fullname.clone(),
                    r.objtype.clone(),
                    r.node_id.clone(),
                    r.aliased,
                )
            })
            .collect()
    }

    fn owned(v: &[(&str, &str, &str, bool)]) -> Vec<(String, String, String, bool)> {
        v.iter()
            .map(|(a, b, c, d)| (a.to_string(), b.to_string(), c.to_string(), *d))
            .collect()
    }

    // ---- py_sig_re (checklist row 1) ----------------------------------

    #[test]
    fn py_sig_match_groups_and_spans() {
        let m = py_sig_match("mymod.func(a, b) -> str").unwrap();
        assert_eq!(m.prefix.as_deref(), Some("mymod."));
        assert_eq!(m.name, "func");
        assert_eq!(m.tp_list, None);
        assert_eq!(m.tp_span, (0, 0));
        assert_eq!(m.arglist.as_deref(), Some("a, b"));
        assert_eq!(m.arg_span, (11, 15));
        assert_eq!(m.retann.as_deref(), Some("str"));

        let m = py_sig_match("f[T](x)").unwrap();
        assert_eq!(m.tp_list.as_deref(), Some("T"));
        assert_eq!(m.tp_span, (2, 3));
        assert_eq!(m.arglist.as_deref(), Some("x"));

        // Empty written parens: group 4 participates with '' — falsy, so
        // handle_signature routes it to the needs_arglist branch.
        let m = py_sig_match("f()").unwrap();
        assert_eq!(m.arglist.as_deref(), Some(""));

        // The greedy-arglist edge: the LAST ')' closes the group, so a
        // parenthesized return annotation is swallowed INTO the arglist
        // and group 5 never participates (probe retann_tuple_greedy).
        let m = py_sig_match("f(x) -> (int, str)").unwrap();
        assert_eq!(m.arglist.as_deref(), Some("x) -> (int, str"));
        assert_eq!(m.retann, None);

        assert!(py_sig_match("not a signature!").is_none());
        assert!(py_sig_match("f(x").is_none());
    }

    // ---- baseline shapes (rows 1, 3, 4) --------------------------------

    #[test]
    fn function_plain_args_matches_the_sphinx_probe() {
        let out = parse_py(".. py:function:: func(a, b)\n\n   Body.\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ func()',\\ 'func',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"func()\" _toc_parts=\"('func',)\" class=\"\" classes=\"sig sig-object\" fullname=\"func\" ids=\"func\" module=\"True\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                func\n",
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        a\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        b\n",
                "        <desc_content>\n",
                "            <paragraph>\n",
                "                Body.\n",
            )
        );
        assert_eq!(objects(&out), owned(&[("func", "function", "func", false)]));
        assert!(out.registry.log_warnings().is_empty());
    }

    #[test]
    fn function_full_markers_matches_the_sphinx_probe() {
        assert_eq!(
            pf_py(".. py:function:: mymod.func(a, b=1, *args, c: int = 2, **kwargs) -> str\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ mymod.func()',\\ 'mymod.func',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                // A dotted prefix at top level is a CLASS prefix (trap 12):
                // class="mymod", desc_addname, index still "built-in".
                "        <desc_signature _toc_name=\"mymod.func()\" _toc_parts=\"('mymod', 'func')\" class=\"mymod\" classes=\"sig sig-object\" fullname=\"mymod.func\" ids=\"mymod.func\" module=\"True\">\n",
                "            <desc_addname classes=\"sig-prename descclassname\" xml:space=\"preserve\">\n",
                "                mymod.\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                func\n",
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        a\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        b\n",
                "                    <desc_sig_operator classes=\"o\">\n",
                "                        =\n",
                "                    <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                "                        1\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_operator classes=\"o\">\n",
                "                        *\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        args\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        c\n",
                "                    <desc_sig_punctuation classes=\"p\">\n",
                "                        :\n",
                "                    <desc_sig_space classes=\"w\">\n",
                "                         \n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
                "                            int\n",
                "                    <desc_sig_space classes=\"w\">\n",
                "                         \n",
                "                    <desc_sig_operator classes=\"o\">\n",
                "                        =\n",
                "                    <desc_sig_space classes=\"w\">\n",
                "                         \n",
                "                    <inline classes=\"default_value\" support_smartquotes=\"0\">\n",
                "                        2\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_operator classes=\"o\">\n",
                "                        **\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        kwargs\n",
                "            <desc_returns xml:space=\"preserve\">\n",
                "                <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
                "                    str\n",
                "        <desc_content>\n",
            )
        );
    }

    /// Row 4/trap 1: no written arglist AND empty written `()` both take
    /// the bare attr-less paramlist for needs_arglist kinds; a class
    /// (needs_arglist false) with `()` gets NO paramlist at all.
    #[test]
    fn no_arglist_and_empty_parens_take_the_bare_paramlist() {
        let out = pf_py(".. py:function:: func\n");
        assert!(
            out.contains("            <desc_parameterlist xml:space=\"preserve\">\n"),
            "bare attr-less paramlist: {out}"
        );
        // Empty parens with a return annotation (probe empty_parens_retann):
        // still the bare list, followed by desc_returns.
        let out = pf_py(".. py:function:: f() -> int\n");
        assert!(out.contains(concat!(
            "            <desc_parameterlist xml:space=\"preserve\">\n",
            "            <desc_returns xml:space=\"preserve\">\n",
            "                <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
            "                    int\n",
        )));
        // Probe class_no_parens: PyClasslike never needs an arglist.
        let out = pf_py(".. py:class:: C()\n");
        assert!(!out.contains("desc_parameterlist"), "{out}");
    }

    /// Row 1/trap 7: a failed py_sig_re match is SILENT — raw sig in one
    /// desc_name, empty toc attrs, no ids, no registration — and drops
    /// option tails with the cleared signode (probe annotation_bad_sig).
    #[test]
    fn a_bad_signature_is_silent_with_empty_toc_and_no_registration() {
        let out = parse_py(".. py:function:: not a signature!\n   :annotation: tail\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"\" _toc_parts=\"()\" classes=\"sig sig-object\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                not a signature!\n",
                "        <desc_content>\n",
            )
        );
        assert!(objects(&out).is_empty());
        assert!(
            out.registry.log_warnings().is_empty(),
            "no warning (trap 7)"
        );
    }

    /// Row 4: `:async:` prefix annotation and the `:annotation:` tail
    /// (probes function_async / function_annotation_option).
    #[test]
    fn async_prefix_and_annotation_option_tail() {
        let out = pf_py(".. py:function:: coro(x)\n   :async:\n");
        assert!(out.contains(concat!(
            "            <desc_annotation xml:space=\"preserve\">\n",
            "                <desc_sig_keyword classes=\"k\">\n",
            "                    async\n",
            "                <desc_sig_space classes=\"w\">\n",
            "                     \n",
            "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
            "                coro\n",
        )));
        let out = pf_py(".. py:function:: f(x)\n   :annotation: something extra\n");
        assert!(out.ends_with(concat!(
            "            <desc_annotation xml:space=\"preserve\">\n",
            "                <desc_sig_space classes=\"w\">\n",
            "                     \n",
            "                something extra\n",
            "        <desc_content>\n",
        )));
    }

    // ---- module resolution (rows 2, 9, 10) -----------------------------

    /// Row 3/10: the `:module:` option qualifies ids/index/registration and
    /// pushes/pops the module scope around the content — the NEXT directive
    /// is back under the surrounding module (probe method_module_pop).
    #[test]
    fn the_module_option_qualifies_and_pops() {
        let out = parse_py(
            ".. py:module:: outer\n\n.. py:function:: g(x)\n   :module: inner\n\n.. py:function:: h(x)\n",
        );
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(
            "    <index entries=\"('single',\\ 'g()\\ (in\\ module\\ inner)',\\ 'inner.g',\\ '',\\ None)\">\n"
        ));
        assert!(pf.contains(
            "        <desc_signature _toc_name=\"g()\" _toc_parts=\"('inner', 'g')\" class=\"\" classes=\"sig sig-object\" fullname=\"g\" ids=\"inner.g\" module=\"inner\">\n"
        ));
        assert!(pf.contains(
            "    <index entries=\"('single',\\ 'h()\\ (in\\ module\\ outer)',\\ 'outer.h',\\ '',\\ None)\">\n"
        ));
        assert!(pf.contains(
            "        <desc_signature _toc_name=\"h()\" _toc_parts=\"('outer', 'h')\" class=\"\" classes=\"sig sig-object\" fullname=\"h\" ids=\"outer.h\" module=\"outer\">\n"
        ));
        assert_eq!(
            objects(&out),
            owned(&[
                ("outer", "module", "module-outer", false),
                ("inner.g", "function", "inner.g", false),
                ("outer.h", "function", "outer.h", false),
            ])
        );
    }

    /// Row 2: prefix resolution inside a class — the class's own prefix is
    /// stripped from display; a DIFFERENT prefix nests (fullname
    /// `C.D.meth`, desc_addname `D.`, index `meth() (C.D method)`) —
    /// probes method_class_prefix_given / method_other_prefix.
    #[test]
    fn class_prefixes_strip_or_nest() {
        let out = parse_py(
            ".. py:class:: C\n\n   .. py:method:: C.meth(x)\n\n   .. py:method:: D.meth(x)\n",
        );
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(
            "                <desc_signature _toc_name=\"C.meth()\" _toc_parts=\"('C', 'meth')\" class=\"C\" classes=\"sig sig-object\" fullname=\"C.meth\" ids=\"C.meth\" module=\"True\">\n"
        ));
        // The stripped prefix leaves no desc_addname on C.meth.
        let c_meth_sig = pf
            .split("fullname=\"C.meth\"")
            .nth(1)
            .unwrap()
            .split("desc_signature")
            .next()
            .unwrap();
        assert!(!c_meth_sig.contains("desc_addname"), "{c_meth_sig}");
        assert!(pf.contains(
            "                <desc_signature _toc_name=\"C.D.meth()\" _toc_parts=\"('C', 'D', 'meth')\" class=\"C\" classes=\"sig sig-object\" fullname=\"C.D.meth\" ids=\"C.D.meth\" module=\"True\">\n"
        ));
        assert!(pf.contains(concat!(
            "                    <desc_addname classes=\"sig-prename descclassname\" xml:space=\"preserve\">\n",
            "                        D.\n",
        )));
        assert!(pf.contains(
            "            <index entries=\"('single',\\ 'meth()\\ (C.D\\ method)',\\ 'C.D.meth',\\ '',\\ None)\">\n"
        ));
    }

    // ---- multi-signature (row 11) --------------------------------------

    #[test]
    fn multiple_signatures_share_one_desc_and_register_each_unique_name() {
        let out = parse_py(".. py:function:: f(x)\n                  g(y)\n\n   Shared body.\n");
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(
            "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None) ('pair',\\ 'built-in\\ function;\\ g()',\\ 'g',\\ '',\\ None)\">\n"
        ));
        assert!(pf.contains(
            "        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n"
        ));
        assert!(pf.contains(
            "        <desc_signature _toc_name=\"g()\" _toc_parts=\"('g',)\" class=\"\" classes=\"sig sig-object\" fullname=\"g\" ids=\"g\" module=\"True\">\n"
        ));
        assert_eq!(pf.matches("<desc_content>").count(), 1, "one shared body");
        assert_eq!(
            objects(&out),
            owned(&[("f", "function", "f", false), ("g", "function", "g", false),]),
            "every UNIQUE name registers ([PY §1.6 function_multi_sig])"
        );
        // Identical signatures dedupe via `if name not in self.names`.
        let out = parse_py(".. py:function:: f(x)\n                  f(x)\n");
        assert_eq!(objects(&out), owned(&[("f", "function", "f", false)]));
    }

    // ---- class + nesting (rows 2, 10) ----------------------------------

    #[test]
    fn class_with_bases_nests_the_method_scope() {
        let out = parse_py(
            ".. py:class:: MyClass(Base1, Base2)\n\n   .. py:method:: meth(self, arg)\n\n      Body.\n",
        );
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('single',\\ 'MyClass\\ (built-in\\ class)',\\ 'MyClass',\\ '',\\ None)\">\n",
                "    <desc classes=\"py class\" desctype=\"class\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"class\">\n",
                "        <desc_signature _toc_name=\"MyClass\" _toc_parts=\"('MyClass',)\" class=\"\" classes=\"sig sig-object\" fullname=\"MyClass\" ids=\"MyClass\" module=\"True\">\n",
                "            <desc_annotation xml:space=\"preserve\">\n",
                "                <desc_sig_keyword classes=\"k\">\n",
                "                    class\n",
                "                <desc_sig_space classes=\"w\">\n",
                "                     \n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                MyClass\n",
                // Class bases parse as an ordinary arglist — names only.
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        Base1\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        Base2\n",
                "        <desc_content>\n",
                "            <index entries=\"('single',\\ 'meth()\\ (MyClass\\ method)',\\ 'MyClass.meth',\\ '',\\ None)\">\n",
                "            <desc classes=\"py method\" desctype=\"method\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"method\">\n",
                "                <desc_signature _toc_name=\"MyClass.meth()\" _toc_parts=\"('MyClass', 'meth')\" class=\"MyClass\" classes=\"sig sig-object\" fullname=\"MyClass.meth\" ids=\"MyClass.meth\" module=\"True\">\n",
                "                    <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                        meth\n",
                "                    <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                        <desc_parameter xml:space=\"preserve\">\n",
                "                            <desc_sig_name classes=\"n\">\n",
                "                                self\n",
                "                        <desc_parameter xml:space=\"preserve\">\n",
                "                            <desc_sig_name classes=\"n\">\n",
                "                                arg\n",
                "                <desc_content>\n",
                "                    <paragraph>\n",
                "                        Body.\n",
            )
        );
        assert_eq!(
            objects(&out),
            owned(&[
                ("MyClass", "class", "MyClass", false),
                ("MyClass.meth", "method", "MyClass.meth", false),
            ])
        );
    }

    /// Row 10: nested classes stack and unwind — after the inner class's
    /// content, the OUTER class scope is restored (probe nested_classes:
    /// inner signode class="Outer.Inner", _toc_parts ('Outer','Inner','m')).
    #[test]
    fn nested_classes_stack_and_unwind() {
        let out = parse_py(concat!(
            ".. py:class:: Outer\n",
            "\n",
            "   .. py:class:: Inner\n",
            "\n",
            "      .. py:method:: m(x)\n",
            "\n",
            "   .. py:method:: back(x)\n",
        ));
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(
            "class=\"Outer.Inner\" classes=\"sig sig-object\" fullname=\"Outer.Inner.m\" ids=\"Outer.Inner.m\""
        ));
        assert!(pf.contains("_toc_parts=\"('Outer', 'Inner', 'm')\""));
        assert!(
            pf.contains("class=\"Outer\" classes=\"sig sig-object\" fullname=\"Outer.back\""),
            "the inner class popped back to Outer: {pf}"
        );
    }

    // ---- method options / aliasing directives (rows 4, 7; trap 13) -----

    #[test]
    fn method_option_trio_prefixes_and_index_texts() {
        let out = parse_py(concat!(
            ".. py:class:: C\n",
            "\n",
            "   .. py:method:: m1(x)\n",
            "      :classmethod:\n",
            "\n",
            "   .. py:method:: m2(x)\n",
            "      :staticmethod:\n",
            "\n",
            "   .. py:method:: m3(x)\n",
            "      :abstractmethod:\n",
            "      :async:\n",
            "      :final:\n",
        ));
        let pf = out.doctree.root.pformat();
        let kw = |words: &[&str]| {
            let mut s =
                String::from("                    <desc_annotation xml:space=\"preserve\">\n");
            for w in words {
                s.push_str(&format!(
                    "                        <desc_sig_keyword classes=\"k\">\n                            {w}\n                        <desc_sig_space classes=\"w\">\n                             \n"
                ));
            }
            s
        };
        assert!(pf.contains(&kw(&["classmethod"])), "{pf}");
        // `:staticmethod:` prints keyword `static`.
        assert!(pf.contains(&kw(&["static"])), "{pf}");
        assert!(
            pf.contains(&kw(&["final", "abstractmethod", "async"])),
            "{pf}"
        );
        assert!(pf.contains("('single',\\ 'm1()\\ (C\\ class\\ method)',\\ 'C.m1',\\ '',\\ None)"));
        assert!(pf.contains("('single',\\ 'm2()\\ (C\\ static\\ method)',\\ 'C.m2',\\ '',\\ None)"));
        assert!(pf.contains("('single',\\ 'm3()\\ (C\\ method)',\\ 'C.m3',\\ '',\\ None)"));
    }

    /// Trap 13: `py:classmethod`/`py:staticmethod`/`py:decoratormethod`
    /// rewrite `self.name`, so their descs carry objtype `method` with the
    /// injected flag driving prefix and index text.
    #[test]
    fn aliasing_directives_register_the_aliased_objtype() {
        let out = parse_py(concat!(
            ".. py:class:: C\n",
            "\n",
            "   .. py:classmethod:: cm(x)\n",
            "\n",
            "   .. py:staticmethod:: sm(x)\n",
            "\n",
            "   .. py:decoratormethod:: dm\n",
        ));
        let pf = out.doctree.root.pformat();
        assert_eq!(
            pf.matches("desctype=\"method\" domain=\"py\"").count(),
            3,
            "{pf}"
        );
        assert!(pf.contains("('single',\\ 'cm()\\ (C\\ class\\ method)',\\ 'C.cm',\\ '',\\ None)"));
        assert!(pf.contains("('single',\\ 'sm()\\ (C\\ static\\ method)',\\ 'C.sm',\\ '',\\ None)"));
        assert!(pf.contains("('single',\\ 'dm()\\ (C\\ method)',\\ 'C.dm',\\ '',\\ None)"));
        // The decorator method: @ addname first, no forced parens.
        assert!(pf.contains(concat!(
            "                    <desc_addname classes=\"sig-prename descclassname\" xml:space=\"preserve\">\n",
            "                        @\n",
            "                    <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
            "                        dm\n",
        )));
        assert_eq!(
            objects(&out),
            owned(&[
                ("C", "class", "C", false),
                ("C.cm", "method", "C.cm", false),
                ("C.sm", "method", "C.sm", false),
                ("C.dm", "method", "C.dm", false),
            ])
        );
    }

    // ---- attribute / property / data (rows 5, 7; trap 3) ---------------

    #[test]
    fn attribute_typed_matches_the_sphinx_probe() {
        let out = parse_py(concat!(
            ".. py:class:: C\n",
            "\n",
            "   .. py:attribute:: attr\n",
            "      :type: int\n",
            "      :value: 42\n",
        ));
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(
            "            <index entries=\"('single',\\ 'attr\\ (C\\ attribute)',\\ 'C.attr',\\ '',\\ None)\">\n"
        ));
        assert!(pf.contains(concat!(
            "                <desc_signature _toc_name=\"C.attr\" _toc_parts=\"('C', 'attr')\" class=\"C\" classes=\"sig sig-object\" fullname=\"C.attr\" ids=\"C.attr\" module=\"True\">\n",
            "                    <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
            "                        attr\n",
            // `:` is desc_sig_punctuation here (trap 3), and the xref
            // carries the enclosing class in py:class.
            "                    <desc_annotation xml:space=\"preserve\">\n",
            "                        <desc_sig_punctuation classes=\"p\">\n",
            "                            :\n",
            "                        <desc_sig_space classes=\"w\">\n",
            "                             \n",
            "                        <pending_xref py:class=\"C\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
            "                            int\n",
            // `=` in `:value:` is desc_sig_punctuation too, unlike
            // parameter defaults (desc_sig_operator).
            "                    <desc_annotation xml:space=\"preserve\">\n",
            "                        <desc_sig_space classes=\"w\">\n",
            "                             \n",
            "                        <desc_sig_punctuation classes=\"p\">\n",
            "                            =\n",
            "                        <desc_sig_space classes=\"w\">\n",
            "                             \n",
            "                        42\n",
        )));
        // Bare-name attribute outside any scope: index text is the name
        // itself (probe value_only_attr).
        let pf = pf_py(".. py:attribute:: a\n   :value: 42\n");
        assert!(pf.contains("    <index entries=\"('single',\\ 'a',\\ 'a',\\ '',\\ None)\">\n"));
    }

    #[test]
    fn property_typed_prefix_and_type_only() {
        let out = parse_py(concat!(
            ".. py:class:: C\n",
            "\n",
            "   .. py:property:: prop\n",
            "      :type: str\n",
            "      :abstractmethod:\n",
            "      :classmethod:\n",
        ));
        let pf = out.doctree.root.pformat();
        // Prefix: abstract ␣ class ␣ property ␣ (three keyword+space pairs).
        assert!(pf.contains(concat!(
            "                    <desc_annotation xml:space=\"preserve\">\n",
            "                        <desc_sig_keyword classes=\"k\">\n",
            "                            abstract\n",
            "                        <desc_sig_space classes=\"w\">\n",
            "                             \n",
            "                        <desc_sig_keyword classes=\"k\">\n",
            "                            class\n",
            "                        <desc_sig_space classes=\"w\">\n",
            "                             \n",
            "                        <desc_sig_keyword classes=\"k\">\n",
            "                            property\n",
            "                        <desc_sig_space classes=\"w\">\n",
            "                             \n",
        )));
        assert!(pf.contains(concat!(
            "                    <desc_annotation xml:space=\"preserve\">\n",
            "                        <desc_sig_punctuation classes=\"p\">\n",
            "                            :\n",
            "                        <desc_sig_space classes=\"w\">\n",
            "                             \n",
            "                        <pending_xref py:class=\"C\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"str\" reftype=\"class\">\n",
            "                            str\n",
        )));
        assert!(pf.contains("('single',\\ 'prop\\ (C\\ property)',\\ 'C.prop',\\ '',\\ None)"));
        assert_eq!(
            objects(&out),
            owned(&[
                ("C", "class", "C", false),
                ("C.prop", "property", "C.prop", false),
            ])
        );
    }

    #[test]
    fn data_typed_renders_type_and_value_tails() {
        let out = parse_py(".. py:data:: CONST\n   :type: dict[str, int]\n   :value: {}\n");
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(
            "    <index entries=\"('single',\\ 'CONST\\ (built-in\\ variable)',\\ 'CONST',\\ '',\\ None)\">\n"
        ));
        // dict [ str , int ] — three xrefs with punctuation between.
        for target in ["dict", "str", "int"] {
            assert!(pf.contains(&format!(
                "<pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"{target}\" reftype=\"class\">"
            )));
        }
        assert!(pf.contains(concat!(
            "            <desc_annotation xml:space=\"preserve\">\n",
            "                <desc_sig_space classes=\"w\">\n",
            "                     \n",
            "                <desc_sig_punctuation classes=\"p\">\n",
            "                    =\n",
            "                <desc_sig_space classes=\"w\">\n",
            "                     \n",
            "                {}\n",
        )));
        assert_eq!(objects(&out), owned(&[("CONST", "data", "CONST", false)]));
    }

    // ---- decorator / type alias / exception (rows 5, 6, 7) -------------

    #[test]
    fn decorator_basic_matches_the_sphinx_probe() {
        let out = parse_py(".. py:decorator:: mydeco\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ mydeco()',\\ 'mydeco',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"mydeco()\" _toc_parts=\"('mydeco',)\" class=\"\" classes=\"sig sig-object\" fullname=\"mydeco\" ids=\"mydeco\" module=\"True\">\n",
                "            <desc_addname classes=\"sig-prename descclassname\" xml:space=\"preserve\">\n",
                "                @\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                mydeco\n",
                "        <desc_content>\n",
            )
        );
        assert_eq!(
            objects(&out),
            owned(&[("mydeco", "function", "mydeco", false)])
        );
    }

    #[test]
    fn type_alias_canonical_is_display_only() {
        let out = parse_py(".. py:type:: MyAlias\n   :canonical: list[int]\n");
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(
            "    <index entries=\"('single',\\ 'MyAlias',\\ 'MyAlias',\\ '',\\ None)\">\n"
        ));
        assert!(pf.contains(concat!(
            "            <desc_annotation xml:space=\"preserve\">\n",
            "                <desc_sig_keyword classes=\"k\">\n",
            "                    type\n",
            "                <desc_sig_space classes=\"w\">\n",
            "                     \n",
            "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
            "                MyAlias\n",
            "            <desc_annotation xml:space=\"preserve\">\n",
            "                <desc_sig_space classes=\"w\">\n",
            "                     \n",
            "                <desc_sig_punctuation classes=\"p\">\n",
            "                    =\n",
            "                <desc_sig_space classes=\"w\">\n",
            "                     \n",
            "                <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"list\" reftype=\"class\">\n",
            "                    list\n",
            "                <desc_sig_punctuation classes=\"p\">\n",
            "                    [\n",
            "                <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refspecific=\"0\" reftarget=\"int\" reftype=\"class\">\n",
            "                    int\n",
            "                <desc_sig_punctuation classes=\"p\">\n",
            "                    ]\n",
        )));
        // NO alias registration on py:type (§6).
        assert_eq!(
            objects(&out),
            owned(&[("MyAlias", "type", "MyAlias", false)])
        );
    }

    #[test]
    fn exception_basic_matches_the_sphinx_probe() {
        assert_eq!(
            pf_py(".. py:exception:: MyError\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                // Exception index entries are the bare name (trap 10).
                "    <index entries=\"('single',\\ 'MyError',\\ 'MyError',\\ '',\\ None)\">\n",
                "    <desc classes=\"py exception\" desctype=\"exception\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"exception\">\n",
                "        <desc_signature _toc_name=\"MyError\" _toc_parts=\"('MyError',)\" class=\"\" classes=\"sig sig-object\" fullname=\"MyError\" ids=\"MyError\" module=\"True\">\n",
                "            <desc_annotation xml:space=\"preserve\">\n",
                "                <desc_sig_keyword classes=\"k\">\n",
                "                    exception\n",
                "                <desc_sig_space classes=\"w\">\n",
                "                     \n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                MyError\n",
                "        <desc_content>\n",
            )
        );
    }

    // ---- py:module / py:currentmodule (row 9; traps 5, 6) --------------

    /// Row 9 — the PARSE-LAYER shape: sphinx's recorded doctree has the
    /// read transforms reorder the index nodes ahead of the target and
    /// PropagateTargets move the module target's id onto the next body node
    /// (`<target ismod="1" refid="module-mymod">` + desc
    /// `ids="module-mymod"`); this parse layer runs no transforms, so the
    /// target KEEPS its ids and the desc gains none. Everything else is the
    /// probe's bytes. The post-transform tree is the sphinx fixture's
    /// `py.module_basic` (tools/gen_sphinx_fixture.py).
    #[test]
    fn module_basic_pre_propagation_shape() {
        let out = parse_py(concat!(
            ".. py:module:: mymod\n",
            "   :synopsis: A module.\n",
            "   :platform: Unix\n",
            "\n",
            ".. py:function:: f(x)\n",
            "\n",
            "   Body.\n",
        ));
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'module;\\ mymod',\\ 'module-mymod',\\ '',\\ None)\">\n",
                "    <target ids=\"module-mymod\" ismod=\"1\">\n",
                "    <index entries=\"('single',\\ 'f()\\ (in\\ module\\ mymod)',\\ 'mymod.f',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"f()\" _toc_parts=\"('mymod', 'f')\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"mymod.f\" module=\"mymod\">\n",
                "            <desc_addname classes=\"sig-prename descclassname\" xml:space=\"preserve\">\n",
                "                mymod.\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                f\n",
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        x\n",
                "        <desc_content>\n",
                "            <paragraph>\n",
                "                Body.\n",
            )
        );
        assert_eq!(
            objects(&out),
            owned(&[
                ("mymod", "module", "module-mymod", false),
                ("mymod.f", "function", "mymod.f", false),
            ])
        );
        assert_eq!(out.registry.py_modules.len(), 1);
        let m = &out.registry.py_modules[0];
        assert_eq!(
            (
                m.name.as_str(),
                m.node_id.as_str(),
                m.synopsis.as_str(),
                m.platform.as_str(),
                m.deprecated,
                m.lineno
            ),
            ("mymod", "module-mymod", "A module.", "Unix", false, 1)
        );
    }

    /// Module content stays in place (the id-propagation onto it is the
    /// read-transform pass's, not the parse layer's); `:deprecated:`
    /// reaches the record.
    #[test]
    fn module_content_and_deprecated() {
        let out = parse_py(".. py:module:: secmod\n\n   Module body content.\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'module;\\ secmod',\\ 'module-secmod',\\ '',\\ None)\">\n",
                "    <target ids=\"module-secmod\" ismod=\"1\">\n",
                "    <paragraph>\n",
                "        Module body content.\n",
            )
        );
        let out = parse_py(".. py:module:: oldmod\n   :deprecated:\n");
        assert!(out.registry.py_modules[0].deprecated);
        assert_eq!(out.registry.py_modules[0].synopsis, "");
    }

    /// Trap 6: `:no-index:` on py:module still sets the module scope —
    /// nothing is emitted or registered for the module itself, but the
    /// following function is module-qualified. `:no-index-entry:` keeps
    /// target + registration and drops only the index node.
    #[test]
    fn module_noindex_and_noindexentry() {
        let out = parse_py(".. py:module:: quietmod\n   :no-index:\n\n.. py:function:: f(x)\n");
        let pf = out.doctree.root.pformat();
        assert!(!pf.contains("module-quietmod"), "{pf}");
        assert!(pf.contains(
            "    <index entries=\"('single',\\ 'f()\\ (in\\ module\\ quietmod)',\\ 'quietmod.f',\\ '',\\ None)\">\n"
        ));
        assert!(out.registry.py_modules.is_empty());
        assert_eq!(
            objects(&out),
            owned(&[("quietmod.f", "function", "quietmod.f", false)])
        );

        let out = parse_py(".. py:module:: halfmod\n   :no-index-entry:\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <target ids=\"module-halfmod\" ismod=\"1\">\n",
            )
        );
        assert_eq!(
            objects(&out),
            owned(&[("halfmod", "module", "module-halfmod", false)])
        );
        assert_eq!(out.registry.py_modules.len(), 1);
    }

    /// Row 9 option-spec quirks: PyModule's spec lacks the old
    /// `noindexentry` spelling → docutils unknown-option ERROR (probe
    /// module_bad_option: nothing runs, no scope set); `no-typesetting`
    /// is accepted but unused by PyModule.run (probe module_no_typesetting).
    #[test]
    fn module_option_spec_quirks() {
        let out = parse_py(".. py:module:: m\n   :noindexentry:\n\n.. py:function:: f(x)\n");
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(concat!(
            "    <system_message level=\"3\" line=\"1\" source=\"<snippet>\" type=\"ERROR\">\n",
            "        <paragraph>\n",
            "            Error in \"py:module\" directive:\n",
            "            unknown option: \"noindexentry\".\n",
        )));
        // The directive never ran: no module scope for the function.
        assert!(pf.contains("('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)"));
        assert!(out.registry.py_modules.is_empty());

        let out = parse_py(".. py:module:: m2\n   :no-typesetting:\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'module;\\ m2',\\ 'module-m2',\\ '',\\ None)\">\n",
                "    <target ids=\"module-m2\" ismod=\"1\">\n",
            ),
            "accepted-and-inert"
        );
    }

    /// Row 9: `py:currentmodule` sets the scope with no nodes and no
    /// registration; the literal argument `None` pops it (probes
    /// currentmodule / currentmodule_pop).
    #[test]
    fn currentmodule_sets_and_pops() {
        let out = parse_py(".. py:currentmodule:: curmod\n\n.. py:function:: f(x)\n");
        let pf = out.doctree.root.pformat();
        assert!(pf.starts_with(concat!(
            "<document source=\"<snippet>\">\n",
            "    <index entries=\"('single',\\ 'f()\\ (in\\ module\\ curmod)',\\ 'curmod.f',\\ '',\\ None)\">\n",
        )));
        assert!(pf.contains(concat!(
            "            <desc_addname classes=\"sig-prename descclassname\" xml:space=\"preserve\">\n",
            "                curmod.\n",
        )));
        assert_eq!(
            objects(&out),
            owned(&[("curmod.f", "function", "curmod.f", false)])
        );
        assert!(out.registry.py_modules.is_empty());

        let out = parse_py(
            ".. py:currentmodule:: curmod\n\n.. py:currentmodule:: None\n\n.. py:function:: f(x)\n",
        );
        assert!(out
            .doctree
            .root
            .pformat()
            .contains("('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)"));
    }

    // ---- registration edges (row 8) ------------------------------------

    #[test]
    fn canonical_function_adds_an_aliased_record() {
        let out = parse_py(".. py:function:: new_name()\n   :canonical: old.name\n");
        assert_eq!(
            objects(&out),
            owned(&[
                ("new_name", "function", "new_name", false),
                ("old.name", "function", "new_name", true),
            ])
        );
    }

    /// Row 8/[PY §1.5]: the empty-prefix make_id path — the id IS the
    /// fullname; the second definition collides and takes the `id0`
    /// serial (probe duplicate_functions; the duplicate WARNING itself is
    /// the env layer's job, T9).
    #[test]
    fn duplicate_definitions_take_the_id0_serial() {
        let out = parse_py(".. py:function:: dup()\n\n.. py:function:: dup()\n");
        let pf = out.doctree.root.pformat();
        assert!(pf.contains("fullname=\"dup\" ids=\"dup\" module=\"True\""));
        assert!(pf.contains("fullname=\"dup\" ids=\"id0\" module=\"True\""));
        assert!(pf.contains("('pair',\\ 'built-in\\ function;\\ dup()',\\ 'id0',\\ '',\\ None)"));
        assert_eq!(
            objects(&out),
            owned(&[
                ("dup", "function", "dup", false),
                ("dup", "function", "id0", false),
            ])
        );
    }

    /// Two `.. py:module:: dupmod`: the second module target takes the
    /// `module-0` serial (`make_id` prefix fallback, `util/nodes.py:633-636`)
    /// and registers over the first — probe duplicate_modules [PY §5].
    ///
    /// This is the parse-layer tree; in sphinx's, the read transforms fold
    /// the first target's id onto the second (`<target ids="module-0
    /// module-dupmod">`), which the sphinx fixture's `py.duplicate_modules`
    /// pins. The module-0 registration is pinned here on the parse's records
    /// and ids. The duplicate WARNING is the env layer's job (T9).
    #[test]
    fn duplicate_modules_take_the_module_0_serial() {
        let out = parse_py(".. py:module:: dupmod\n\n.. py:module:: dupmod\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'module;\\ dupmod',\\ 'module-dupmod',\\ '',\\ None)\">\n",
                "    <target ids=\"module-dupmod\" ismod=\"1\">\n",
                "    <index entries=\"('pair',\\ 'module;\\ dupmod',\\ 'module-0',\\ '',\\ None)\">\n",
                "    <target ids=\"module-0\" ismod=\"1\">\n",
            )
        );
        assert_eq!(
            objects(&out),
            owned(&[
                ("dupmod", "module", "module-dupmod", false),
                ("dupmod", "module", "module-0", false),
            ])
        );
        assert_eq!(
            out.registry
                .py_modules
                .iter()
                .map(|r| (r.name.as_str(), r.node_id.as_str()))
                .collect::<Vec<_>>(),
            vec![("dupmod", "module-dupmod"), ("dupmod", "module-0")]
        );
    }

    // ---- no-* family (row 7/8, [PY §1.7]) ------------------------------

    #[test]
    fn no_star_option_quartet() {
        // :no-index:: empty index node, no ids, nothing registered.
        let out = parse_py(".. py:function:: hidden()\n   :no-index:\n");
        let pf = out.doctree.root.pformat();
        assert!(pf.contains("    <index entries=\"\">\n"));
        assert!(pf.contains("no-index=\"1\""));
        assert!(pf.contains("noindex=\"1\""));
        assert!(pf.contains(
            "        <desc_signature _toc_name=\"hidden()\" _toc_parts=\"('hidden',)\" class=\"\" classes=\"sig sig-object\" fullname=\"hidden\" module=\"True\">\n"
        ));
        assert!(objects(&out).is_empty());

        // :noindex: old spelling behaves identically (both attrs 1).
        let out = parse_py(".. py:function:: hidden()\n   :noindex:\n");
        let pf2 = out.doctree.root.pformat();
        assert!(pf2.contains("no-index=\"1\"") && pf2.contains("noindex=\"1\""));
        assert!(objects(&out).is_empty());

        // :no-index-entry:: registered with ids, no index entry.
        let out = parse_py(".. py:function:: quiet()\n   :no-index-entry:\n");
        let pf = out.doctree.root.pformat();
        assert!(pf.contains("    <index entries=\"\">\n"));
        assert!(pf.contains("fullname=\"quiet\" ids=\"quiet\" module=\"True\""));
        assert_eq!(
            objects(&out),
            owned(&[("quiet", "function", "quiet", false)])
        );

        // :no-typesetting:: desc collapses to a bare target carrying the
        // collected ids; index entry + registration survive.
        let out = parse_py(".. py:function:: invisible()\n   :no-typesetting:\n");
        assert_eq!(
            out.doctree.root.pformat(),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ invisible()',\\ 'invisible',\\ '',\\ None)\">\n",
                "    <target ids=\"invisible\">\n",
            )
        );
        assert_eq!(
            objects(&out),
            owned(&[("invisible", "function", "invisible", false)])
        );
    }

    // ---- strip_signature_backslash (row 12) ----------------------------

    #[test]
    fn strip_signature_backslash_strips_before_parsing() {
        let cfg = PySigConfig {
            strip_signature_backslash: true,
            ..PySigConfig::default()
        };
        let pf = pf_py_cfg(".. py:function:: f(a\\_b)\n", cfg);
        assert!(pf.contains(concat!(
            "                    <desc_sig_name classes=\"n\">\n",
            "                        a_b\n",
        )));
        // Default off: the backslash survives into the (pseudo-parsed)
        // parameter (probe strip_backslash_off).
        let pf = pf_py(".. py:function:: f(a\\_b)\n");
        assert!(pf.contains(concat!(
            "                    <desc_sig_name classes=\"n\">\n",
            "                        a\\_b\n",
        )));
    }

    // ---- error channels (row 13) ---------------------------------------

    /// Row 13: duplicate parameter names WARN (`could not parse arglist`)
    /// with the pseudo fallback; tp-list failures WARN (`could not parse
    /// tp_list`) with the exception text interpolated — bytes pinned by
    /// probes arglist_dup_warning / tp_list_warning / tp_list_tokerror.
    #[test]
    fn arglist_and_tp_list_error_paths_warn() {
        let out = parse_py(".. py:function:: f(a, a)\n");
        assert_eq!(
            out.registry
                .log_warnings()
                .iter()
                .map(|w| (w.text.as_str(), w.line))
                .collect::<Vec<_>>(),
            vec![(
                "could not parse arglist ('a, a'): duplicate parameter name: 'a'",
                Some(1)
            )]
        );
        // Pseudo fallback still renders both parameters.
        let pf = out.doctree.root.pformat();
        assert_eq!(
            pf.matches(concat!(
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        a\n",
            ))
            .count(),
            2
        );

        let out = parse_py(".. py:function:: f[*Ts: int](x)\n");
        assert_eq!(
            out.registry.log_warnings()[0].text,
            "could not parse tp_list ('*Ts: int'): type parameter bound or constraint is not allowed for variadic positional parameters"
        );
        // The failed tp list is simply absent; the signature continues.
        let pf = out.doctree.root.pformat();
        assert!(!pf.contains("desc_type_parameter_list"));
        assert!(pf.contains("fullname=\"f\" ids=\"f\""));

        let out = parse_py(".. py:function:: f[(T](x)\n");
        assert_eq!(
            out.registry.log_warnings()[0].text,
            "could not parse tp_list ('(T'): ('unexpected EOF in multi-line statement', (1, 0))"
        );

        // The SyntaxError channel stays SILENT (debug level): brackets
        // fall back to the pseudo parser with no warning.
        let out = parse_py(".. py:function:: func(a[, b])\n");
        assert!(out.registry.log_warnings().is_empty());
        assert!(out.doctree.root.pformat().contains("<desc_optional"));
    }

    /// The greedy-arglist edge stays TOTAL but diverges from sphinx:
    /// sphinx's `_parse_arglist` wraps the captured `x) -> (int, str` in
    /// `def func(...): pass`, where the stray `)` closes the def and the
    /// tuple parses as a (discarded) def-level return annotation — params
    /// [x]. Our arglist grammar rejects the stray `)` (SyntaxError channel,
    /// silent) and pseudo-parses instead. Known divergence, excluded from
    /// the T8 corpus; this pin is a totality guard, not an oracle match.
    #[test]
    fn greedy_arglist_edge_is_total_and_silent() {
        let out = parse_py(".. py:function:: f(x) -> (int, str)\n");
        assert!(out.registry.log_warnings().is_empty());
        assert_eq!(objects(&out), owned(&[("f", "function", "f", false)]));
        assert!(out.doctree.root.pformat().contains("x) -> (int"));
    }

    // ---- toc config variants (row 3, [SIG §2.2]) -----------------------

    #[test]
    fn toc_entry_config_variants() {
        // hide: last part only (probe toc_hide: _toc_name "m()").
        let cfg = PySigConfig {
            toc_object_entries_show_parents: "hide".to_string(),
            ..PySigConfig::default()
        };
        let pf = pf_py_cfg(".. py:method:: C.m(x)\n", cfg);
        assert!(
            pf.contains("_toc_name=\"m()\" _toc_parts=\"('C', 'm')\""),
            "{pf}"
        );

        // all: every hierarchy part joined — the module joins the parts.
        let cfg = PySigConfig {
            toc_object_entries_show_parents: "all".to_string(),
            ..PySigConfig::default()
        };
        let pf = pf_py_cfg(".. py:module:: pkg\n\n.. py:method:: C.m(x)\n", cfg);
        assert!(
            pf.contains("_toc_name=\"pkg.C.m()\" _toc_parts=\"('pkg', 'C', 'm')\""),
            "{pf}"
        );

        // add_function_parentheses=false drops the parens from _toc_name
        // (and only functions/methods ever get them).
        let cfg = PySigConfig {
            add_function_parentheses: false,
            ..PySigConfig::default()
        };
        let pf = pf_py_cfg(".. py:method:: C.m(x)\n", cfg);
        assert!(
            pf.contains("_toc_name=\"C.m\" _toc_parts=\"('C', 'm')\""),
            "{pf}"
        );

        // toc_object_entries=false: empty toc attrs, everything else kept
        // (probe toc_off).
        let cfg = PySigConfig {
            toc_object_entries: false,
            ..PySigConfig::default()
        };
        let pf = pf_py_cfg(".. py:function:: f(x)\n", cfg);
        assert!(pf.contains(
            "        <desc_signature _toc_name=\"\" _toc_parts=\"()\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n"
        ));
    }

    // ---- multi-line signature wrapping ([SIG §2.5] probes) -------------

    #[test]
    fn long_signatures_wrap_and_single_line_options_suppress() {
        let cfg = || PySigConfig {
            maximum_signature_line_length: Some(20),
            ..PySigConfig::default()
        };
        let pf = pf_py_cfg(
            ".. py:function:: really_long_function_name(argument_one, argument_two)\n",
            cfg(),
        );
        assert!(pf.contains(
            "<desc_parameterlist multi_line_parameter_list=\"1\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">"
        ));
        let pf = pf_py_cfg(
            ".. py:function:: really_long_function_name(argument_one, argument_two)\n   :single-line-parameter-list:\n",
            cfg(),
        );
        assert!(pf.contains(
            "<desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">"
        ));
        let pf = pf_py_cfg(
            ".. py:class:: LongName[TypeParamOne, TypeParamTwo]\n",
            cfg(),
        );
        assert!(pf.contains(
            "<desc_type_parameter_list multi_line_parameter_list=\"1\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">"
        ));
        let pf = pf_py_cfg(
            ".. py:class:: LongName[TypeParamOne, TypeParamTwo]\n   :single-line-type-parameter-list:\n",
            cfg(),
        );
        assert!(pf.contains(
            "<desc_type_parameter_list multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">"
        ));
    }

    // ---- misc: add_module_names off ------------------------------------

    #[test]
    fn add_module_names_off_drops_the_module_addname() {
        let cfg = PySigConfig {
            add_module_names: false,
            ..PySigConfig::default()
        };
        let pf = pf_py_cfg(".. py:module:: mymod\n\n.. py:function:: f(x)\n", cfg);
        assert!(!pf.contains("desc_addname"), "{pf}");
        // Registration and index stay module-qualified regardless.
        assert!(pf.contains("ids=\"mymod.f\" module=\"mymod\""));
        assert!(
            pf.contains("('single',\\ 'f()\\ (in\\ module\\ mymod)',\\ 'mymod.f',\\ '',\\ None)")
        );
    }

    /// Row 7: the method-family index clsname is module-qualified iff
    /// `add_module_names` (probes method_module_qualified /
    /// method_module_qualified_off: `meth() (mymod.C method)` vs
    /// `meth() (C method)` — the node id stays qualified either way).
    #[test]
    fn method_index_clsname_qualification_follows_add_module_names() {
        let src = ".. py:module:: mymod\n\n.. py:class:: C\n\n   .. py:method:: meth(x)\n";
        let pf = pf_py(src);
        assert!(pf.contains(
            "('single',\\ 'meth()\\ (mymod.C\\ method)',\\ 'mymod.C.meth',\\ '',\\ None)"
        ));
        let cfg = PySigConfig {
            add_module_names: false,
            ..PySigConfig::default()
        };
        let pf = pf_py_cfg(src, cfg);
        assert!(
            pf.contains("('single',\\ 'meth()\\ (C\\ method)',\\ 'mymod.C.meth',\\ '',\\ None)")
        );
    }

    /// Row 10: a NON-nesting kind with a written prefix scopes its own
    /// content to that prefix — `before_content`'s `name_prefix.strip('.')`
    /// branch (probe method_prefix_scope: the xref inside carries
    /// `py:class="D"`), and the scope pops after the content.
    #[test]
    fn a_prefixed_method_scopes_its_content_without_nesting() {
        let out = parse_py(
            ".. py:method:: D.meth(x)\n\n   :py:func:`target`\n\n.. py:function:: after(x)\n",
        );
        let pf = out.doctree.root.pformat();
        assert!(pf.contains(
            "                <pending_xref py:class=\"D\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"target\" reftype=\"func\" refwarn=\"0\">\n"
        ));
        assert!(pf.contains("('single',\\ 'meth()\\ (D\\ method)',\\ 'D.meth',\\ '',\\ None)"));
        // after_content restored the empty scope for the next directive.
        assert!(pf.contains("fullname=\"after\" ids=\"after\" module=\"True\""));
    }

    /// [PY §3.1] probe role_in_module_scope: a py role inside a class's
    /// content carries the enclosing ref_context on the pending_xref —
    /// `py:class="C" py:module="mymod"` instead of the None sentinels.
    #[test]
    fn a_py_role_inside_a_scope_stamps_the_ref_context() {
        let pf = pf_py(".. py:module:: mymod\n\n.. py:class:: C\n\n   :py:func:`target`\n");
        assert!(
            pf.contains(concat!(
                "                <pending_xref py:class=\"C\" py:module=\"mymod\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"target\" reftype=\"func\" refwarn=\"0\">\n",
                "                    <literal classes=\"xref py py-func\">\n",
                "                        target()\n",
            )),
            "{pf}"
        );
    }
}

/// Doc-field transformation (M2 wave 4.5 task 7). Every expected pformat
/// below is pasted verbatim from the Sphinx 9.1.0 oracle — this task's
/// probe_t7 run (harness3 conventions, pinned wheels) over the research
/// specs [PY §1.6 "Doc fields"] and [SIG §4.2 item 2 / A.5] — never
/// written from memory.
#[cfg(test)]
mod py_docfield_tests {
    use super::*;
    use crate::py::PySigConfig;
    use crate::rst::{parse_rst_full, ParseOptions};

    fn pf_cfg(src: &str, py: PySigConfig) -> String {
        let opts = ParseOptions {
            source_path: "<snippet>".into(),
            sphinx: true,
            docname: "index".into(),
            exclude_patterns: Vec::new(),
            py,
            found_docs: None,
            srcdir: None,
            ..Default::default()
        };
        parse_rst_full(src, &opts).doctree.root.pformat()
    }

    fn pf(src: &str) -> String {
        pf_cfg(src, PySigConfig::default())
    }

    fn unqual() -> PySigConfig {
        PySigConfig {
            python_use_unqualified_type_names: true,
            ..PySigConfig::default()
        }
    }

    /// `PyXrefMixin._delimiters_re` split parity, pinned against the
    /// Python `re.split` outputs (delimiters kept, empties dropped).
    #[test]
    fn the_delimiter_split_matches_python_re_split() {
        let split = |t: &str| split_type_delimiters(t);
        let owned = |v: &[(&str, bool)]| -> Vec<(String, bool)> {
            v.iter().map(|(s, d)| (s.to_string(), *d)).collect()
        };
        assert_eq!(
            split("int or str"),
            owned(&[("int", false), (" or ", true), ("str", false)])
        );
        assert_eq!(
            split("Literal[1, 2]"),
            owned(&[
                ("Literal", false),
                ("[", true),
                ("1", false),
                (", ", true),
                ("2", false),
                ("]", true),
            ])
        );
        assert_eq!(
            split("list[int]"),
            owned(&[("list", false), ("[", true), ("int", false), ("]", true)])
        );
        assert_eq!(
            split("a | b"),
            owned(&[("a", false), (" | ", true), ("b", false)])
        );
        assert_eq!(split("int..."), owned(&[("int", false), ("...", true)]));
        assert_eq!(
            split("dict of str"),
            owned(&[("dict", false), (" of ", true), ("str", false)])
        );
        // No trailing whitespace after `or` -> the whole thing is text.
        assert_eq!(split("x or"), owned(&[("x or", false)]));
        assert_eq!(split("of or"), owned(&[("of or", false)]));
        assert_eq!(split(" or "), owned(&[(" or ", true)]));
        // A bracket delimiter swallows a following `or `.
        assert_eq!(
            split("int, or str"),
            owned(&[("int", false), (", or ", true), ("str", false)])
        );
        assert_eq!(
            split("tuple(int)"),
            owned(&[("tuple", false), ("(", true), ("int", false), (")", true)])
        );
        assert_eq!(
            split("a|b"),
            owned(&[("a", false), ("|", true), ("b", false)])
        );
    }

    /// std kinds run the transformer with an EMPTY typemap
    /// (`directives/__init__.py:295`; no std kind declares
    /// `doc_field_types`): every field takes the unknown branch —
    /// `Param x`, body untouched, fresh field_list (probe envvar_param).
    #[test]
    fn a_std_field_is_capitalized_and_passed_through() {
        assert_eq!(
            pf(".. envvar:: HOME_X\n\n   :param x: not transformed\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('single',\\ 'environment\\ variable;\\ HOME_X',\\ 'envvar-HOME_X',\\ '',\\ None)\">\n",
                "    <desc classes=\"std envvar\" desctype=\"envvar\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"envvar\">\n",
                "        <desc_signature _toc_name=\"\" _toc_parts=\"()\" classes=\"sig sig-object\" ids=\"envvar-HOME_X\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                HOME_X\n",
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Param x\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            not transformed\n",
            )
        );
    }

    /// `filter_meta_fields` guards `domain == 'py'`
    /// (`domains/python/__init__.py:610-611`), so a std `:meta private:`
    /// SURVIVES and renders renamed `Meta private` with its empty body
    /// (probe envvar_meta).
    #[test]
    fn a_std_meta_field_is_not_removed() {
        assert_eq!(
            pf(".. envvar:: HOME_Y\n\n   :meta private:\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('single',\\ 'environment\\ variable;\\ HOME_Y',\\ 'envvar-HOME_Y',\\ '',\\ None)\">\n",
                "    <desc classes=\"std envvar\" desctype=\"envvar\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"envvar\">\n",
                "        <desc_signature _toc_name=\"\" _toc_parts=\"()\" classes=\"sig sig-object\" ids=\"envvar-HOME_Y\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                HOME_Y\n",
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Meta private\n",
                "                    <field_body>\n",
            )
        );
    }

    /// Confval's `transform_content` inserts its own field_list BEFORE the
    /// transformer runs; both it and the body's field_list are direct
    /// desc_content children and both transform — pass-through for the
    /// generated `Type` field (same name, body untouched) and the
    /// unknown rename for `:param y:` (probe confval_type_and_field).
    #[test]
    fn the_confval_generated_field_list_transforms_too() {
        assert_eq!(
            pf(".. confval:: s\n   :type: text with *emphasis*\n\n   :param y: field in body\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 's;\\ configuration\\ value',\\ 'confval-s',\\ '',\\ None)\">\n",
                "    <desc classes=\"std confval\" desctype=\"confval\" domain=\"std\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"confval\">\n",
                "        <desc_signature _toc_name=\"s\" _toc_parts=\"('s',)\" classes=\"sig sig-object\" fullname=\"s\" ids=\"confval-s\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                s\n",
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Type\n",
                "                    <field_body>\n",
                "                        text with \n",
                "                        <emphasis>\n",
                "                            emphasis\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Param y\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            field in body\n",
            )
        );
    }

    /// The `len(field) != 2` pass-through on the input that makes sphinx
    /// itself abort (`sphinx/util/docfields.py:381`, `assert len(field) ==
    /// 2`): the unterminated emphasis in the option puts a one-child
    /// `system_message` beside the generated `Type`/`Default` field, as a
    /// DIRECT `field_list` child, and the transformer must leave both
    /// there — the field untouched, the message untouched, no panic.
    /// Sanctioned divergence; a crash has no oracle pformat, so the pin is
    /// crate-side (panel round C; the corpus holds the case out as
    /// `EXCLUDED["sx_std.confval_bad_type_markup"]`).
    ///
    // oracle (sphinx 9.1.0, full SphinxTestApp dummy build, verification
    // records v9/v22/v27): `:type: *bad` and `:default: *bad` -> RAISED
    // AssertionError at docfields.py:381; `:type: *bad*` / `:type: int`
    // -> BUILD OK.
    #[test]
    fn a_one_child_field_list_member_passes_through_without_panicking() {
        for (option, name) in [("type", "Type"), ("default", "Default")] {
            let pf = pf(&format!(".. confval:: t\n   :{option}: *bad\n"));
            let expected = format!(
                concat!(
                    "        <desc_content>\n",
                    "            <field_list>\n",
                    "                <field>\n",
                    "                    <field_name>\n",
                    "                        {name}\n",
                    "                    <field_body>\n",
                    "                        <problematic ids=\"id2\" refid=\"id1\">\n",
                    "                            *\n",
                    "                        bad\n",
                    "                <system_message backrefs=\"id2\" ids=\"id1\" level=\"2\" ",
                    "line=\"1\" source=\"<snippet>\" type=\"WARNING\">\n",
                    "                    <paragraph>\n",
                    "                        Inline emphasis start-string without end-string.\n",
                ),
                name = name
            );
            assert!(pf.contains(&expected), "{option}: {pf}");
        }
        // The well-formed neighbours sphinx builds clean stay two-child
        // fields with no message.
        for option in [":type: *bad*", ":type: int"] {
            let pf = pf(&format!(".. confval:: t\n   {option}\n"));
            assert!(!pf.contains("system_message"), "{option}: {pf}");
        }
    }

    /// `describe` (bare docutils registration, domain='') transforms with
    /// the empty map too (probe describe_param).
    #[test]
    fn describe_fields_take_the_unknown_branch() {
        let pf = pf(".. describe:: foo\n\n   :param x: desc\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Param x\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            desc\n",
            )),
            "{pf}"
        );
    }

    /// `option` transforms with the empty map (probe option_param).
    #[test]
    fn option_fields_take_the_unknown_branch() {
        let pf = pf(".. option:: --x\n\n   :param x: desc\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Param x\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            desc\n",
            )),
            "{pf}"
        );
    }

    /// The py field names mean nothing to a std kind: `:returns:` /
    /// `:rtype:` are renamed `Returns`/`Rtype` and left as plain fields
    /// (probe envvar_multi_fields).
    #[test]
    fn py_field_names_are_unknown_on_std_kinds() {
        let pf = pf(".. envvar:: HOME_W\n\n   :param a: one\n   :returns: two\n   :rtype: bool\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Param a\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            one\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Returns\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            two\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Rtype\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            bool\n",
            )),
            "{pf}"
        );
    }

    /// The [PY §1.6] `function_fields` probe: grouped Parameters with
    /// `:param int a:` inline-type and `:type b:` merge, Returns, Return
    /// type body role, Raises `exc` xref.
    #[test]
    fn function_fields_probe_byte_for_byte() {
        assert_eq!(
            pf(".. py:function:: f(a, b)\n\n   :param int a: first\n   :param b: second\n   :type b: str\n   :returns: something\n   :rtype: bool\n   :raises ValueError: when bad\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                f\n",
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        a\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        b\n",
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <bullet_list>\n",
                "                            <list_item>\n",
                "                                <paragraph>\n",
                "                                    <literal_strong>\n",
                "                                        a\n",
                "                                     (\n",
                "                                    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"int\" reftype=\"class\">\n",
                "                                        <literal_emphasis>\n",
                "                                            int\n",
                "                                    )\n",
                "                                     -- \n",
                "                                    first\n",
                "                            <list_item>\n",
                "                                <paragraph>\n",
                "                                    <literal_strong>\n",
                "                                        b\n",
                "                                     (\n",
                "                                    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"str\" reftype=\"class\">\n",
                "                                        <literal_emphasis>\n",
                "                                            str\n",
                "                                    )\n",
                "                                     -- \n",
                "                                    second\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Returns\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            something\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Return type\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"bool\" reftype=\"class\">\n",
                "                                bool\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Raises\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"ValueError\" reftype=\"exc\">\n",
                "                                <literal_strong>\n",
                "                                    ValueError\n",
                "                             -- \n",
                "                            when bad\n",
            )
        );
    }

    /// TypedField `can_collapse`: one item is a bare paragraph in the
    /// field_body — no bullet_list (probe param_single; F-U1 shape).
    #[test]
    fn a_single_param_collapses_to_a_paragraph() {
        assert_eq!(
            pf(".. py:function:: f(x)\n\n   :param x: only one\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                f\n",
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        x\n",
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             -- \n",
                "                            only one\n",
            )
        );
    }

    /// `:type x:` content lands as ` ( <xref> )` inside the single
    /// collapsed param entry (probe type_merge).
    #[test]
    fn a_type_field_merges_into_the_param_entry() {
        assert_eq!(
            pf(".. py:function:: f(x)\n\n   :param x: thing\n   :type x: str\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                f\n",
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        x\n",
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"str\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    str\n",
                "                            )\n",
                "                             -- \n",
                "                            thing\n",
            )
        );
    }

    /// `filter_meta_fields` removes the `:meta:` field BEFORE the
    /// transformer runs; the emptied `<field_list>` remains [PY §1.6
    /// meta probe].
    #[test]
    fn meta_fields_are_removed_but_the_field_list_remains() {
        assert_eq!(
            pf(".. py:function:: f()\n\n   :meta private:\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                f\n",
                "            <desc_parameterlist xml:space=\"preserve\">\n",
                "        <desc_content>\n",
                "            <field_list>\n",
            )
        );
    }

    /// `PyXrefMixin.make_xrefs` splits `int or str` into two xrefs around
    /// a `literal_emphasis` ` or ` delimiter (probe multi_type_or).
    #[test]
    fn a_multi_type_field_splits_on_or() {
        assert_eq!(
            pf(".. py:function:: f(x)\n\n   :param x: thing\n   :type x: int or str\n"),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                f\n",
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        x\n",
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"int\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    int\n",
                "                            <literal_emphasis>\n",
                "                                 or \n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"str\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    str\n",
                "                            )\n",
                "                             -- \n",
                "                            thing\n",
            )
        );
    }

    /// [SIG §4.2 item 2, probe F-U2]: under
    /// `python_use_unqualified_type_names`, the typed-field xref gets the
    /// two `pending_xref_condition` children with the `literal_emphasis`
    /// innernode wrapped INSIDE each condition.
    #[test]
    fn unqualified_type_names_wrap_field_xrefs_in_conditions() {
        assert_eq!(
            pf_cfg(".. py:function:: f(x)\n\n   :param x: thing\n   :type x: pkg.Cls\n", unqual()),
            concat!(
                "<document source=\"<snippet>\">\n",
                "    <index entries=\"('pair',\\ 'built-in\\ function;\\ f()',\\ 'f',\\ '',\\ None)\">\n",
                "    <desc classes=\"py function\" desctype=\"function\" domain=\"py\" no-contents-entry=\"0\" no-index=\"0\" no-index-entry=\"0\" no-typesetting=\"0\" nocontentsentry=\"0\" noindex=\"0\" noindexentry=\"0\" objtype=\"function\">\n",
                "        <desc_signature _toc_name=\"f()\" _toc_parts=\"('f',)\" class=\"\" classes=\"sig sig-object\" fullname=\"f\" ids=\"f\" module=\"True\">\n",
                "            <desc_name classes=\"sig-name descname\" xml:space=\"preserve\">\n",
                "                f\n",
                "            <desc_parameterlist multi_line_parameter_list=\"0\" multi_line_trailing_comma=\"1\" xml:space=\"preserve\">\n",
                "                <desc_parameter xml:space=\"preserve\">\n",
                "                    <desc_sig_name classes=\"n\">\n",
                "                        x\n",
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"pkg.Cls\" reftype=\"class\">\n",
                "                                <pending_xref_condition condition=\"resolved\">\n",
                "                                    <literal_emphasis>\n",
                "                                        Cls\n",
                "                                <pending_xref_condition condition=\"*\">\n",
                "                                    <literal_emphasis>\n",
                "                                        pkg.Cls\n",
                "                            )\n",
                "                             -- \n",
                "                            thing\n",
            )
        );
    }

    /// GroupedField collapse applies only to a single item; two `:raises:`
    /// build a bullet_list (probe raises_two).
    #[test]
    fn multiple_raises_stay_a_bullet_list() {
        let pf =
            pf(".. py:function:: f()\n\n   :raises ValueError: bad\n   :raises TypeError: worse\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Raises\n",
                "                    <field_body>\n",
                "                        <bullet_list>\n",
                "                            <list_item>\n",
                "                                <paragraph>\n",
                "                                    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"ValueError\" reftype=\"exc\">\n",
                "                                        <literal_strong>\n",
                "                                            ValueError\n",
                "                                     -- \n",
                "                                    bad\n",
                "                            <list_item>\n",
                "                                <paragraph>\n",
                "                                    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"TypeError\" reftype=\"exc\">\n",
                "                                        <literal_strong>\n",
                "                                            TypeError\n",
                "                                     -- \n",
                "                                    worse\n",
            )),
            "{pf}"
        );
    }

    /// `:ivar:`/`:vartype:` render under Variables, and the xref reads the
    /// enclosing class scope: `py:class=\"C\"` (probe ivar_vartype).
    #[test]
    fn variables_fields_read_the_class_ref_context() {
        let pf = pf(".. py:class:: C\n\n   :ivar x: doc\n   :vartype x: int\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Variables\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"C\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"int\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    int\n",
                "                            )\n",
                "                             -- \n",
                "                            doc\n",
            )),
            "{pf}"
        );
    }

    /// An unknown field name is capitalized and the field passed through
    /// untouched (probe unknown_field).
    #[test]
    fn an_unknown_field_is_capitalized_and_passed_through() {
        let pf = pf(".. py:function:: f()\n\n   :custom foo: bar\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Custom foo\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            bar\n",
            )),
            "{pf}"
        );
    }

    /// A `:type x:` with no matching `:param x:` is consumed into the
    /// types map and never re-emitted — empty field_list (probe
    /// orphan_type).
    #[test]
    fn an_orphan_type_field_is_consumed_silently() {
        let pf = pf(".. py:function:: f(x)\n\n   :type x: int\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
            )),
            "{pf}"
        );
    }

    /// `:param:` and `:keyword:` are the same `parameter` group (probe
    /// param_keyword_group).
    #[test]
    fn param_and_keyword_share_one_parameters_group() {
        let pf = pf(".. py:function:: f(a, b)\n\n   :param a: pos\n   :keyword b: kw\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <bullet_list>\n",
                "                            <list_item>\n",
                "                                <paragraph>\n",
                "                                    <literal_strong>\n",
                "                                        a\n",
                "                                     -- \n",
                "                                    pos\n",
                "                            <list_item>\n",
                "                                <paragraph>\n",
                "                                    <literal_strong>\n",
                "                                        b\n",
                "                                     -- \n",
                "                                    kw\n",
            )),
            "{pf}"
        );
    }

    /// `~pkg.Cls` in a type field: full reftarget, short title in the
    /// `literal_emphasis` innernode (probe tilde_type).
    #[test]
    fn a_tilde_type_takes_the_short_title() {
        let pf = pf(".. py:function:: f(x)\n\n   :param x: thing\n   :type x: ~pkg.Cls\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"pkg.Cls\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    Cls\n",
                "                            )\n",
                "                             -- \n",
                "                            thing\n",
            )),
            "{pf}"
        );
    }

    /// `Literal[...]` suppression is sticky: everything after the
    /// `Literal` xref renders as plain `literal_emphasis` (probe
    /// literal_type).
    #[test]
    fn literal_bracket_types_suppress_inner_xrefs() {
        let pf = pf(".. py:function:: f(x)\n\n   :param x: thing\n   :type x: Literal[1, 2]\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"Literal\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    Literal\n",
                "                            <literal_emphasis>\n",
                "                                [\n",
                "                            <literal_emphasis>\n",
                "                                1\n",
                "                            <literal_emphasis>\n",
                "                                , \n",
                "                            <literal_emphasis>\n",
                "                                2\n",
                "                            <literal_emphasis>\n",
                "                                ]\n",
                "                            )\n",
                "                             -- \n",
                "                            thing\n",
            )),
            "{pf}"
        );
    }

    /// The rtype body role splits too, and the split contnode keeps BARE
    /// Text children — no literal_emphasis (probe rtype_or_split).
    #[test]
    fn rtype_bodies_split_and_keep_bare_text() {
        let pf = pf(".. py:function:: f()\n\n   :rtype: int or str\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Return type\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"int\" reftype=\"class\">\n",
                "                                int\n",
                "                             or \n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"str\" reftype=\"class\">\n",
                "                                str\n",
            )),
            "{pf}"
        );
    }

    /// `types.pop()` semantics: a doubled `:param x:` gets the type on the
    /// first entry only (probe dup_param_typed).
    #[test]
    fn a_doubled_param_consumes_its_type_once() {
        let pf = pf(".. py:function:: f(x)\n\n   :param x: a\n   :param x: b\n   :type x: int\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <bullet_list>\n",
                "                            <list_item>\n",
                "                                <paragraph>\n",
                "                                    <literal_strong>\n",
                "                                        x\n",
                "                                     (\n",
                "                                    <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"int\" reftype=\"class\">\n",
                "                                        <literal_emphasis>\n",
                "                                            int\n",
                "                                    )\n",
                "                                     -- \n",
                "                                    a\n",
                "                            <list_item>\n",
                "                                <paragraph>\n",
                "                                    <literal_strong>\n",
                "                                        x\n",
                "                                     -- \n",
                "                                    b\n",
            )),
            "{pf}"
        );
    }

    /// Field xrefs carry `py:module` from the ref_context (probe
    /// currentmodule_ctx).
    #[test]
    fn field_xrefs_read_the_module_ref_context() {
        let pf = pf(".. py:currentmodule:: curmod\n\n.. py:function:: f(x)\n\n   :param x: thing\n   :type x: str\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"curmod\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"str\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    str\n",
                "                            )\n",
                "                             -- \n",
                "                            thing\n",
            )),
            "{pf}"
        );
    }

    /// `returnvalue` is a plain `Field` (no PyXrefMixin): `int or str`
    /// stays one Text (probe returns_or_not_split).
    #[test]
    fn returns_bodies_are_never_split() {
        let pf = pf(".. py:function:: f()\n\n   :returns: int or str\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Returns\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            int or str\n",
            )),
            "{pf}"
        );
    }

    /// Removing a `:meta:` field keeps its siblings transforming (probe
    /// meta_then_param).
    #[test]
    fn meta_removal_keeps_sibling_fields() {
        let pf = pf(".. py:function:: f(x)\n\n   :param x: kept\n   :meta private:\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             -- \n",
                "                            kept\n",
            )),
            "{pf}"
        );
    }

    /// `:type:` with no argument mismatches `has_arg` and passes through
    /// renamed `Type` — but a lone-Text body is still type-linked
    /// (`docfields.py:409-432`; probe type_no_arg).
    #[test]
    fn a_bare_type_field_is_unknown_but_type_linked() {
        let pf = pf(".. py:function:: f(x)\n\n   :type: int\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Type\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"int\" reftype=\"class\">\n",
                "                                int\n",
            )),
            "{pf}"
        );
    }

    /// `:returns foo:` mismatches `has_arg=False` and passes through as
    /// `Returns foo` (probe returns_with_arg_mismatch).
    #[test]
    fn an_arg_on_returns_demotes_it_to_unknown() {
        let pf = pf(".. py:function:: f()\n\n   :returns foo: x\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Returns foo\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            x\n",
            )),
            "{pf}"
        );
    }

    /// `:raises:` with no argument is the unknown path — capitalized name,
    /// body untouched, no xref (probe raises_no_arg_mismatch).
    #[test]
    fn raises_without_arg_is_passed_through() {
        let pf = pf(".. py:function:: f()\n\n   :raises: something\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Raises\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            something\n",
            )),
            "{pf}"
        );
    }

    /// TypedField adds ` -- ` only when the description has content (probe
    /// param_no_desc).
    #[test]
    fn an_empty_description_omits_the_dashes() {
        let pf = pf(".. py:function:: f(x)\n\n   :param x:\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
            )),
            "{pf}"
        );
    }

    /// A multi-paragraph field body keeps its paragraphs, nested inside
    /// the item paragraph after ` -- ` (probe param_multipara).
    #[test]
    fn multi_paragraph_content_nests_in_the_item() {
        let pf = pf(".. py:function:: f(x)\n\n   :param x: first para\n\n      second para\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             -- \n",
                "                            <paragraph>\n",
                "                                first para\n",
                "                            <paragraph>\n",
                "                                second para\n",
            )),
            "{pf}"
        );
    }

    /// A `:type x:` body with markup is not a single Text: the parsed
    /// nodes (here a role-generated pending_xref) are spliced verbatim
    /// between the parens (probe type_markup_body).
    #[test]
    fn a_markup_type_body_is_spliced_verbatim() {
        let pf = pf(".. py:function:: f(x)\n\n   :param x: thing\n   :type x: :class:`Foo`\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"Foo\" reftype=\"class\" refwarn=\"0\">\n",
                "                                <literal classes=\"xref py py-class\">\n",
                "                                    Foo\n",
                "                            )\n",
                "                             -- \n",
                "                            thing\n",
            )),
            "{pf}"
        );
    }

    /// Description inline markup rides along into the transformed entry
    /// (probe param_markup_desc).
    #[test]
    fn markup_in_descriptions_is_spliced() {
        let pf = pf(".. py:function:: f(x)\n\n   :param x: has *emphasis* here\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             -- \n",
                "                            has \n",
                "                            <emphasis>\n",
                "                                emphasis\n",
                "                             here\n",
            )),
            "{pf}"
        );
    }

    /// Only immediate field_list children transform — and each one does,
    /// independently (probe two_field_lists).
    #[test]
    fn each_field_list_transforms_independently() {
        let pf = pf(
            ".. py:function:: f(x)\n\n   :param x: one\n\n   Body between.\n\n   :returns: two\n",
        );
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             -- \n",
                "                            one\n",
                "            <paragraph>\n",
                "                Body between.\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Returns\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            two\n",
            )),
            "{pf}"
        );
    }

    /// `:param pkg.Cls x:` rsplits into type + name (probe
    /// param_type_name_syntax_dotted).
    #[test]
    fn param_type_name_syntax_takes_the_last_token() {
        let pf = pf(".. py:function:: f(x)\n\n   :param pkg.Cls x: doc\n");
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"pkg.Cls\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    pkg.Cls\n",
                "                            )\n",
                "                             -- \n",
                "                            doc\n",
            )),
            "{pf}"
        );
    }

    /// `~pkg.Cls` under unqualified names: the title-rewrite branch wins
    /// and NO condition nodes appear (probe FU2_tilde_precedence).
    #[test]
    fn the_title_rewrite_beats_unqualified_conditions() {
        let pf = pf_cfg(
            ".. py:function:: f(x)\n\n   :param x: thing\n   :type x: ~pkg.Cls\n",
            unqual(),
        );
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Parameters\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <literal_strong>\n",
                "                                x\n",
                "                             (\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"pkg.Cls\" reftype=\"class\">\n",
                "                                <literal_emphasis>\n",
                "                                    Cls\n",
                "                            )\n",
                "                             -- \n",
                "                            thing\n",
            )),
            "{pf}"
        );
    }

    /// Raises xrefs wrap in conditions too, with their `literal_strong`
    /// innernode inside each (probe FU2_raises).
    #[test]
    fn unqualified_wraps_raises_xrefs_too() {
        let pf = pf_cfg(
            ".. py:function:: f()\n\n   :raises pkg.Err: bad\n",
            unqual(),
        );
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Raises\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"pkg.Err\" reftype=\"exc\">\n",
                "                                <pending_xref_condition condition=\"resolved\">\n",
                "                                    <literal_strong>\n",
                "                                        Err\n",
                "                                <pending_xref_condition condition=\"*\">\n",
                "                                    <literal_strong>\n",
                "                                        pkg.Err\n",
                "                             -- \n",
                "                            bad\n",
            )),
            "{pf}"
        );
    }

    /// The rtype body path: `resolved` holds the default `emphasis`
    /// innernode, `*` keeps the original bare-Text contnode (probe
    /// FU2_rtype).
    #[test]
    fn unqualified_rtype_conditions_use_emphasis_innernode() {
        let pf = pf_cfg(".. py:function:: f()\n\n   :rtype: pkg.Cls\n", unqual());
        assert!(
            pf.contains(concat!(
                "        <desc_content>\n",
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Return type\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            <pending_xref py:class=\"True\" py:module=\"True\" refdomain=\"py\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"pkg.Cls\" reftype=\"class\">\n",
                "                                <pending_xref_condition condition=\"resolved\">\n",
                "                                    <emphasis>\n",
                "                                        Cls\n",
                "                                <pending_xref_condition condition=\"*\">\n",
                "                                    pkg.Cls\n",
            )),
            "{pf}"
        );
    }

    /// `replace_self` runs `new_list.update_basic_atts(old_list)`
    /// (`DU/nodes.py`), so the rebuilt field list KEEPS `ids`, `names`,
    /// `classes` and `dupnames` and drops everything else. Task 7 deferred
    /// this as "over-drops"; task 16 probed 9.1.0
    /// (`rst_class_before_field_list_in_py_desc` /
    /// `..._in_confval` / `name_target_before_field_list_in_py_desc`) and
    /// confirmed the carry-over, so the reset is now selective.
    ///
    /// Driven at the function, not through a parse, because BOTH parse-time
    /// routes to a decorated `field_list` are blocked upstream by
    /// pre-existing gaps this wave did not touch (both re-probed here and
    /// ledgered for wave 5):
    ///
    /// * `.. class::` / `.. rst-class::` before a field list attaches the
    ///   class to the field BODY's paragraph here, where docutils attaches
    ///   it to the `field_list` (`para\n\n.. class:: c\n\n:param x: v`
    ///   → `<field_list classes="c">` in docutils 0.22.4);
    /// * `ids`/`names` reach a field list only through `PropagateTargets`,
    ///   which runs after this parse layer, in the read-transform pass.
    ///
    /// Pinning the mechanism directly means the fix stays correct when
    /// either gap closes, instead of waiting on it.
    #[test]
    fn the_transformed_field_list_keeps_its_basic_attributes() {
        let mut list = Node::elem(kinds::FIELD_LIST, Span::ZERO);
        list.attrs.ids.push("fl-target".to_string());
        list.attrs.names.push("fl-target".to_string());
        list.attrs.classes.push("myclass".to_string());
        list.attrs.dupnames.push("dup".to_string());
        list.attrs.backrefs.push("gone".to_string());
        list.set("dropped", AttrValue::Int(1));

        let mut field = Node::elem("field", Span::ZERO);
        let mut name = Node::elem("field_name", Span::ZERO);
        name.children
            .push(Node::text_node("param x".to_string(), Span::ZERO));
        let mut body = Node::elem("field_body", Span::ZERO);
        let mut para = Node::elem(kinds::PARAGRAPH, Span::ZERO);
        para.children
            .push(Node::text_node("v".to_string(), Span::ZERO));
        body.children.push(para);
        field.children.push(name);
        field.children.push(body);
        list.children.push(field);

        let ctx = crate::py::annotations::PyRefContext {
            module: None,
            class_: None,
            span: Span::ZERO,
        };
        transform_doc_field_list(&mut list, py_field_type_map, &ctx, &PySigConfig::default());

        assert_eq!(list.attrs.ids, vec!["fl-target".to_string()]);
        assert_eq!(list.attrs.names, vec!["fl-target".to_string()]);
        assert_eq!(list.attrs.classes, vec!["myclass".to_string()]);
        assert_eq!(list.attrs.dupnames, vec!["dup".to_string()]);
        // Not a basic attribute: dropped with the replaced node.
        assert!(list.attrs.backrefs.is_empty());
        assert_eq!(list.get("dropped"), None);
        // ... and the transform still ran (`param x` -> the Parameters
        // field), so this is not passing by doing nothing.
        assert!(list.pformat().contains("Parameters"), "{}", list.pformat());
    }

    /// The empty type map the std kinds hand in makes every field take the
    /// unknown branch, which never reads the collected content — so the
    /// body is no longer cloned for it. Behaviour-neutral by construction;
    /// this pins the behaviour half.
    #[test]
    fn std_fields_still_render_with_the_lazy_content_clone() {
        let pf = pf(".. confval:: t\n\n   :param x: v\n   :type: int\n");
        assert!(
            pf.contains(concat!(
                "            <field_list>\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Param x\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            v\n",
                "                <field>\n",
                "                    <field_name>\n",
                "                        Type\n",
                "                    <field_body>\n",
                "                        <paragraph>\n",
                "                            int\n",
            )),
            "{pf}"
        );
    }
}

// ----------------------------------------------------------------------
// include tests (T12; expectations pinned against docutils 0.22.4 /
// sphinx 9.1.0 probes — [INC §1-2, §5] and this task's probe_t12 run —
// with the §Scope-8 srcdir-relative path spellings where noted)
// ----------------------------------------------------------------------

#[cfg(test)]
mod include_tests {
    use super::*;
    use crate::doctree::Doctree;
    use crate::rst::{parse_rst, parse_rst_full, ParseOptions, ParseOutput};
    use std::path::Path;

    fn write(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Sphinx-mode parse of `main` as `<docname>.rst` inside `srcdir`.
    fn parse_sphinx(srcdir: &Path, docname: &str, main: &str) -> Doctree {
        parse_sphinx_full(srcdir, docname, main).doctree
    }

    /// [`parse_sphinx`] keeping the whole [`ParseOutput`] — the
    /// `dependencies`/`included` records live in its registry export.
    fn parse_sphinx_full(srcdir: &Path, docname: &str, main: &str) -> ParseOutput {
        parse_rst_full(
            main,
            &ParseOptions {
                source_path: srcdir.join(format!("{docname}.rst")).display().to_string(),
                sphinx: true,
                docname: docname.to_string(),
                found_docs: None,
                exclude_patterns: Vec::new(),
                py: Default::default(),
                srcdir: Some(srcdir.to_path_buf()),
                ..Default::default()
            },
        )
    }

    /// Docutils-mode parse with an absolute source path (the containing-
    /// file-relative branch resolves against its directory).
    fn parse_docutils(source_path: &Path, main: &str) -> Doctree {
        parse_rst(
            main,
            &ParseOptions {
                source_path: source_path.display().to_string(),
                sphinx: false,
                docname: "index".to_string(),
                found_docs: None,
                exclude_patterns: Vec::new(),
                py: Default::default(),
                srcdir: None,
                ..Default::default()
            },
        )
    }

    fn messages_of(tree: &Doctree) -> Vec<(i64, i64, String, String)> {
        fn walk(node: &Node, out: &mut Vec<(i64, i64, String, String)>) {
            if node.kind == kinds::SYSTEM_MESSAGE {
                let level = match node.get("level") {
                    Some(AttrValue::Int(n)) => *n,
                    _ => 0,
                };
                let line = match node.get("line") {
                    Some(AttrValue::Int(n)) => *n,
                    _ => 0,
                };
                let source = match node.get("source") {
                    Some(AttrValue::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let text = node
                    .children
                    .first()
                    .map(|p| p.astext())
                    .unwrap_or_default();
                out.push((level, line, source, text));
            }
            for child in &node.children {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        walk(&tree.root, &mut out);
        out
    }

    /// Top-level paragraphs only (message paragraphs live inside
    /// system_message nodes and are not collected).
    fn paragraphs_of(tree: &Doctree) -> Vec<String> {
        tree.root
            .children
            .iter()
            .filter(|n| n.kind == kinds::PARAGRAPH)
            .map(|n| n.astext())
            .collect()
    }

    // ---- row 1: option spec + converters -----------------------------

    #[test]
    fn number_lines_rejects_a_non_integer_value_with_the_int_error() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "L1\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :literal:\n   :number-lines: x7\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1, "{}", tree.root.pformat());
        assert_eq!(msgs[0].0, 3);
        assert_eq!(
            msgs[0].3,
            "Error in \"include\" directive:\ninvalid option value: (option: \"number-lines\"; \
             value: 'x7')\ninvalid literal for int() with base 10: 'x7'."
        );
    }

    #[test]
    fn an_unknown_encoding_fails_option_conversion_with_the_docutils_text() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "L1\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :encoding: bogus-enc\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1, "{}", tree.root.pformat());
        assert_eq!(
            msgs[0].3,
            "Error in \"include\" directive:\ninvalid option value: (option: \"encoding\"; \
             value: 'bogus-enc')\nunknown encoding: \"bogus-enc\"."
        );
    }

    #[test]
    fn negative_tab_width_disables_expansion_in_literal_mode() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "a\tb\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :literal:\n   :tab-width: -1\n",
        );
        let pf = tree.root.pformat();
        assert!(pf.contains("a\tb"), "tabs must survive: {pf}");
    }

    /// CPython converts `expandtabs`'s tabsize to a C `int` first, so a
    /// value past `i32::MAX` is an `OverflowError` rather than a
    /// multi-gigabyte pad. Every include mode reaches `expandtabs`, and
    /// the check must return PROMPTLY — an unbounded loop pushing one
    /// space at a time made the build effectively non-terminating.
    ///
    // oracle (scratchpad A/tw.py, pinned toolchain):
    //   'ab'.expandtabs(2**31)   -> OverflowError: Python int too large
    //                               to convert to C int  (no tab needed)
    //   'a\tb'.expandtabs(2**31-1) and .expandtabs(-2**31) succeed
    //   `.. literalinclude:: t.txt` + `:tab-width: 2147483648` ->
    //     "index.rst:1: WARNING: Python int too large to convert to C int"
    //   `.. include:: t.txt` + the same option -> sphinx ABORTS with the
    //     uncaught OverflowError, so our SEVERE is a better-than-sphinx,
    //     unpinnable divergence.
    #[test]
    fn a_huge_tab_width_is_an_overflow_error_not_a_hang() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "a\tb\n");
        for options in [
            "   :tab-width: 2147483648\n",
            "   :literal:\n   :tab-width: 2147483648\n",
            "   :code:\n   :tab-width: 9223372036854775807\n",
            "   :tab-width: -2147483649\n",
        ] {
            let tree = parse_sphinx(
                tmp.path(),
                "main",
                &format!(".. include:: inc.rst\n{options}"),
            );
            let msgs = messages_of(&tree);
            assert_eq!(msgs.len(), 1, "{options}: {msgs:?}");
            assert_eq!(msgs[0].0, 4, "SEVERE");
            assert_eq!(
                msgs[0].3,
                "Problem with \"include\" directive:\nPython int too large to convert to C int"
            );
        }
        // The largest accepted magnitude still parses: the range check
        // happens before any padding, and a negative width removes tabs.
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :tab-width: -2147483648\n",
        );
        assert!(messages_of(&tree).is_empty());
    }

    /// The range check waits for the read, exactly where docutils calls
    /// `expandtabs`: an earlier failure reports ITSELF alone, an empty
    /// text in insert mode never expands at all, and literal/code mode's
    /// `tab_width >= 0` gate skips a negative out-of-range width entirely
    /// (panel round C — the round-B guard sat at the top of `run_include`
    /// and reported the overflow ahead of a missing file).
    ///
    // oracle (docutils 0.22.4 publish_doctree, scratchpad rc/probe_tw.py;
    // sphinx-build 9.1.0 prints the same lone CRITICAL for the missing
    // file and finishes, per the verification record):
    //   missing.rst + 2147483648 (insert/literal) -> only the path SEVERE
    //   empty.rst + 2147483648 (insert)   -> no message, nothing inserted
    //   empty.rst + 2147483648 (literal/code) -> OverflowError: the C-int
    //     check precedes the string, even an empty one
    //   a\tb + 2147483648 (insert/literal/code) -> OverflowError
    //   a\tb + -2147483649 (literal/code) -> no message, tab preserved
    //   a\tb + -2147483649 (insert)       -> OverflowError (string2lines)
    //   :start-after: zzz + 2147483648    -> only the start-after SEVERE
    //   :start-line: 5 (clips to '') + 2147483648 (insert) -> nothing
    #[test]
    fn the_tab_width_check_waits_for_the_read_like_expandtabs() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "a\tb\n");
        write(tmp.path(), "empty.rst", "");
        let overflow =
            "Problem with \"include\" directive:\nPython int too large to convert to C int";
        let one_severe = |src: &str| -> String {
            let tree = parse_sphinx(tmp.path(), "main", src);
            let msgs = messages_of(&tree);
            assert_eq!(msgs.len(), 1, "{src:?}: {msgs:?}");
            assert_eq!(msgs[0].0, 4, "SEVERE: {src:?}");
            msgs[0].3.clone()
        };
        // A missing file reports only the read error, in every mode.
        for mode in ["", "   :literal:\n", "   :code:\n"] {
            assert_eq!(
                one_severe(&format!(
                    ".. include:: missing.rst\n{mode}   :tab-width: 2147483648\n"
                )),
                "Problems with \"include\" directive path:\nInputError: [Errno 2] \
                 No such file or directory: 'missing.rst'."
            );
        }
        // An empty file: insert mode has no line to expand; literal and
        // code mode call `expandtabs` on the empty string, which still
        // rejects the width.
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: empty.rst\n   :tab-width: 2147483648\n",
        );
        assert!(messages_of(&tree).is_empty(), "{:?}", messages_of(&tree));
        assert!(paragraphs_of(&tree).is_empty());
        for mode in ["   :literal:\n", "   :code:\n"] {
            assert_eq!(
                one_severe(&format!(
                    ".. include:: empty.rst\n{mode}   :tab-width: 2147483648\n"
                )),
                overflow
            );
        }
        // A file with content: the overflow, in every mode.
        for mode in ["", "   :literal:\n", "   :code:\n"] {
            assert_eq!(
                one_severe(&format!(
                    ".. include:: inc.rst\n{mode}   :tab-width: 2147483648\n"
                )),
                overflow
            );
        }
        // A negative out-of-range width: literal and code mode never reach
        // `expandtabs` (the tab survives); insert mode always does.
        for mode in ["   :literal:\n", "   :code:\n"] {
            let tree = parse_sphinx(
                tmp.path(),
                "main",
                &format!(".. include:: inc.rst\n{mode}   :tab-width: -2147483649\n"),
            );
            assert!(messages_of(&tree).is_empty(), "{:?}", messages_of(&tree));
            assert!(
                tree.root.pformat().contains("a\tb"),
                "{}",
                tree.root.pformat()
            );
        }
        assert_eq!(
            one_severe(".. include:: inc.rst\n   :tab-width: -2147483649\n"),
            overflow
        );
        // A failing clip wins over the width ...
        assert_eq!(
            one_severe(".. include:: inc.rst\n   :start-after: zzz\n   :tab-width: 2147483648\n"),
            "Problem with \"start-after\" option of \"include\" directive:\nText not found."
        );
        // ... and a clip that leaves nothing behind leaves nothing to expand.
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :start-line: 5\n   :tab-width: 2147483648\n",
        );
        assert!(messages_of(&tree).is_empty(), "{:?}", messages_of(&tree));
    }

    /// End to end: the file OPENED is the one sphinx's `.resolve()`
    /// names. `link` points outside the srcdir, so `link/../shared.txt`
    /// is the link target's neighbour, not the srcdir's.
    ///
    // oracle: sphinx probe (scratchpad probe4 in the panel record) —
    // BASE/secret.txt OUTSIDE, BASE/src/secret.txt INSIDE, BASE/src/link
    // -> BASE/ext; `.. literalinclude:: link/../secret.txt` renders the
    // OUTSIDE content.
    #[cfg(unix)]
    #[test]
    fn an_include_through_a_symlink_reads_the_targets_neighbour() {
        let base = tempfile::tempdir().unwrap();
        let base = crate::utils::canonicalize_simplified(base.path()).unwrap();
        let srcdir = base.join("src");
        std::fs::create_dir_all(base.join("ext/inner")).unwrap();
        std::fs::create_dir_all(&srcdir).unwrap();
        write(&srcdir, "shared.txt", "INSIDE\n");
        std::fs::write(base.join("ext/shared.txt"), "OUTSIDE\n").unwrap();
        std::os::unix::fs::symlink(base.join("ext/inner"), srcdir.join("link")).unwrap();

        let tree = parse_sphinx(&srcdir, "main", ".. include:: link/../shared.txt\n");
        assert_eq!(paragraphs_of(&tree), vec!["OUTSIDE".to_string()]);
        // literalinclude resolves through the same rule.
        let tree = parse_sphinx(&srcdir, "main", ".. literalinclude:: link/../shared.txt\n");
        assert!(
            tree.root.pformat().contains("OUTSIDE"),
            "{}",
            tree.root.pformat()
        );

        // The bookkeeping records follow the RESOLVED path as well:
        // `note_included` and docutils' `record_dependencies` both see the
        // `.resolve()`d absolute, so they can name a different docname
        // than the lexical spelling — or none (panel round C, sweep [8]).
        //
        // oracle (sphinx 9.1.0 `-b dummy`, scratchpad rc/probe_symlink.py):
        //   s1 `link -> BASE/ext/inner`, `.. include:: link/../part.rst`
        //      with BASE/src/part.rst a real sibling: env.included maps
        //      index to '<B>/ext/part' — an absolute "docname" nothing
        //      matches, so `part.rst` still gets `document isn't included
        //      in any toctree`; env.dependencies = {'<B>/src/../ext/part.rst'}
        //   s2 `link -> src/a/b`, `link/../c.rst` (src/c.rst AND src/a/c.rst
        //      exist): paragraphs 'Deep para.'; included 'a/c' (not 'c');
        //      dependency '<B>/src/a/c.rst'; `c.rst` is the orphan
        //   s3 `alias.rst -> real.rst`: included 'real'; dependency
        //      '<B>/src/real.rst'; `alias.rst` is the orphan
        //   s5 literalinclude through s1's link: dependency
        //      '<B>/src/../ext/part.txt'
        // (`path2doc` returns None for the outside landing — same orphan
        // outcome, and no absolute path in `env.included`.)
        std::fs::write(base.join("ext/part.rst"), "OUTSIDE PART\n").unwrap();
        write(&srcdir, "part.rst", "Part\n====\n\nINSIDE PART\n");
        let output = parse_sphinx_full(&srcdir, "main", ".. include:: link/../part.rst\n");
        assert_eq!(
            paragraphs_of(&output.doctree),
            vec!["OUTSIDE PART".to_string()]
        );
        assert!(
            output.registry.included.is_empty(),
            "an outside landing marks no document included: {:?}",
            output.registry.included
        );
        assert_eq!(
            output.registry.dependencies,
            vec!["../ext/part.rst".to_string()],
            "the dependency is the file actually read, srcdir-relative"
        );
        let output = parse_sphinx_full(&srcdir, "main", ".. literalinclude:: link/../shared.txt\n");
        assert_eq!(
            output.registry.dependencies,
            vec!["../ext/shared.txt".to_string()]
        );
        // s2: a link INSIDE the tree lands on another docname.
        std::fs::create_dir_all(srcdir.join("a/b")).unwrap();
        write(&srcdir, "a/c.rst", "DEEP\n");
        write(&srcdir, "c.rst", "SHALLOW\n");
        std::os::unix::fs::symlink(srcdir.join("a/b"), srcdir.join("inlink")).unwrap();
        let output = parse_sphinx_full(&srcdir, "main", ".. include:: inlink/../c.rst\n");
        assert_eq!(paragraphs_of(&output.doctree), vec!["DEEP".to_string()]);
        assert_eq!(output.registry.included, vec!["a/c".to_string()]);
        assert_eq!(output.registry.dependencies, vec!["a/c.rst".to_string()]);
        // s3: a symlinked FILE records its target's docname; the display
        // surfaces keep the lexical spelling (§Scope-8).
        std::os::unix::fs::symlink(srcdir.join("c.rst"), srcdir.join("alias.rst")).unwrap();
        let output = parse_sphinx_full(&srcdir, "main", ".. include:: alias.rst\n");
        assert_eq!(paragraphs_of(&output.doctree), vec!["SHALLOW".to_string()]);
        assert_eq!(output.registry.included, vec!["c".to_string()]);
        assert_eq!(output.registry.dependencies, vec!["c.rst".to_string()]);
        assert!(
            output.doctree.sources.contains(&"alias.rst".to_string()),
            "{:?}",
            output.doctree.sources
        );
    }

    // ---- row 2: path resolution (§Scope-2a) --------------------------

    #[test]
    fn sphinx_mode_resolves_relative_to_the_document_directory() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "part.rst", "from part\n");
        write(tmp.path(), "sub/doc.rst", "unused\n");
        let tree = parse_sphinx(tmp.path(), "sub/doc", ".. include:: ../part.rst\n");
        assert_eq!(paragraphs_of(&tree), vec!["from part".to_string()]);
        assert!(
            tree.sources.contains(&"part.rst".to_string()),
            "provenance spells the srcdir-relative path: {:?}",
            tree.sources
        );
    }

    #[test]
    fn a_leading_slash_resolves_against_srcdir() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "sub/abs_part.rst", "abs part para\n");
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: /sub/abs_part.rst\n");
        assert_eq!(paragraphs_of(&tree), vec!["abs part para".to_string()]);
        assert!(tree.sources.contains(&"sub/abs_part.rst".to_string()));
    }

    /// Sphinx rewrites EVERY include argument through relfn2path before
    /// docutils resolves anything, so a nested relative include resolves
    /// against the current *document*'s directory — NOT the directory of
    /// the included file containing the directive ([INC §2 item 3]).
    #[test]
    fn nested_includes_resolve_against_the_document_not_the_containing_file() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "sub/inner.rst", ".. include:: x.rst\n");
        write(tmp.path(), "x.rst", "x at srcdir root\n");
        write(tmp.path(), "sub/x.rst", "x beside inner\n");
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: /sub/inner.rst\n");
        assert_eq!(
            paragraphs_of(&tree),
            vec!["x at srcdir root".to_string()],
            "{}",
            tree.root.pformat()
        );
    }

    /// Docutils mode keeps the containing-file-relative branch
    /// (`adapt_path`, `misc.py:28-39`).
    #[test]
    fn docutils_mode_resolves_against_the_containing_file() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "sub/inner.rst", ".. include:: deep.rst\n");
        write(tmp.path(), "sub/deep.rst", "deep beside inner\n");
        let tree = parse_docutils(&tmp.path().join("main.rst"), ".. include:: sub/inner.rst\n");
        assert_eq!(
            paragraphs_of(&tree),
            vec!["deep beside inner".to_string()],
            "{}",
            tree.root.pformat()
        );
    }

    // ---- row 3: standard includes ------------------------------------

    #[test]
    fn a_standard_include_splices_the_vendored_file() {
        let tree = parse_rst(
            "x |rarr| y\n\n.. include:: <isonum.txt>\n",
            &ParseOptions::default(),
        );
        let pf = tree.root.pformat();
        assert!(
            pf.contains("<substitution_definition names=\"rarr\">"),
            "{pf}"
        );
        assert!(
            tree.sources.contains(&"<isonum.txt>".to_string()),
            "provenance spells the bracketed form: {:?}",
            &tree.sources[..3.min(tree.sources.len())]
        );
    }

    #[test]
    fn a_missing_standard_include_is_a_severe_with_the_bracketed_spelling() {
        let tree = parse_rst(".. include:: <bogus.txt>\n", &ParseOptions::default());
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, 4);
        // Documented divergence: docutils spells its installation
        // directory here (environment-dependent); ours is the argument.
        assert_eq!(
            msgs[0].3,
            "Problems with \"include\" directive path:\nInputError: [Errno 2] No such file or \
             directory: '<bogus.txt>'."
        );
    }

    // ---- row 4: read_file error texts --------------------------------

    /// [`py_input_error_text`] renders PYTHON's `(errno, strerror)` pair,
    /// which is the same pair on every platform: `os.strerror(2)` is
    /// `'No such file or directory'` in Windows Python exactly as in
    /// Linux Python. Rust's Windows `io::Error` carries neither — its
    /// `raw_os_error()` is the WIN32 code (2 `ERROR_FILE_NOT_FOUND`, 3
    /// `ERROR_PATH_NOT_FOUND`) and its text is "The system cannot find
    /// the file specified." — so the mapping goes through `ErrorKind`
    /// there. Built from `io::Error` values rather than from a real
    /// failed open, so the pin holds wherever the suite runs.
    #[test]
    fn the_input_error_text_speaks_pythons_errno_table() {
        // A raw OS error 2: ENOENT on Unix, ERROR_FILE_NOT_FOUND on
        // Windows — `ErrorKind::NotFound` and `[Errno 2]` either way.
        assert_eq!(
            py_input_error_text(&std::io::Error::from_raw_os_error(2), "nothere.rst"),
            "InputError: [Errno 2] No such file or directory: 'nothere.rst'"
        );
        // The kinds with no OS error behind them take the portable table.
        for (kind, pair) in [
            (
                std::io::ErrorKind::NotFound,
                "[Errno 2] No such file or directory",
            ),
            (
                std::io::ErrorKind::PermissionDenied,
                "[Errno 13] Permission denied",
            ),
            (
                std::io::ErrorKind::NotADirectory,
                "[Errno 20] Not a directory",
            ),
            (
                std::io::ErrorKind::IsADirectory,
                "[Errno 21] Is a directory",
            ),
        ] {
            assert_eq!(
                py_input_error_text(&std::io::Error::from(kind), "x.rst"),
                format!("InputError: {pair}: 'x.rst'")
            );
        }
    }

    #[test]
    fn a_missing_file_is_a_severe_with_the_input_error_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: nothere.rst\n");
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1, "{}", tree.root.pformat());
        assert_eq!(msgs[0].0, 4);
        assert_eq!(msgs[0].1, 1);
        assert_eq!(
            msgs[0].3,
            "Problems with \"include\" directive path:\nInputError: [Errno 2] No such file or \
             directory: 'nothere.rst'."
        );
        // The rawsource literal rides the message (states.py:2285-2291).
        let pf = tree.root.pformat();
        assert!(
            pf.contains(
                "        <literal_block xml:space=\"preserve\">\n            .. include:: nothere.rst\n"
            ),
            "{pf}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_a_severe_with_errno_13() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "sekrit.rst", "hi\n");
        std::fs::set_permissions(
            tmp.path().join("sekrit.rst"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        // A privileged user (root, CAP_DAC_OVERRIDE — the usual container
        // setup) reads a mode-000 file anyway, so there is no EACCES to
        // observe; the premise, not the parser, is what fails there.
        if std::fs::read(tmp.path().join("sekrit.rst")).is_ok() {
            eprintln!("skipping: mode 000 does not deny reads to this user");
            return;
        }
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: sekrit.rst\n");
        std::fs::set_permissions(
            tmp.path().join("sekrit.rst"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].3,
            "Problems with \"include\" directive path:\nInputError: [Errno 13] Permission \
             denied: 'sekrit.rst'."
        );
    }

    #[test]
    fn a_decode_failure_is_a_severe_with_pythons_unicode_error_no_period() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("inc.rst"), b"caf\xe9 latin-1 bytes\n").unwrap();
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :encoding: utf-8\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, 4);
        assert_eq!(
            msgs[0].3,
            "Problem with \"include\" directive:\nUnicodeDecodeError: 'utf-8' codec can't \
             decode byte 0xe9 in position 3: invalid continuation byte"
        );
    }

    /// No `:encoding:` option means `settings.input_encoding`, and the two
    /// venues set it differently: sphinx overwrites it with
    /// `config.source_encoding` (default `'utf-8-sig'`, which STRIPS a
    /// BOM), while bare docutils leaves it at docutils' own `'utf-8'`
    /// default and the BOM survives as U+FEFF. A surviving BOM also
    /// disables the first construct — `\u{feff}.. note::` is not explicit
    /// markup.
    ///
    // oracle (scratchpad A/encprobe.py, pinned toolchain):
    //   get_default_settings(Parser).input_encoding == 'utf-8'
    //   docutils publish_string of `.. include:: bom.txt` (bom.txt =
    //     EF BB BF + '.. note::\n\n   Hi from include\n') ->
    //     '<paragraph>\n        \ufeff.. note:\n    <literal_block ...'
    //   sphinx dummy build of the same input -> '<note>\n <paragraph>\n
    //     Hi from include\n' (no BOM anywhere)
    #[test]
    fn the_default_include_encoding_strips_a_bom_in_sphinx_mode_only() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("bom.txt"),
            b"\xef\xbb\xbf.. note::\n\n   Hi from include\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("bom2.txt"), b"\xef\xbb\xbfhello bom\n").unwrap();

        // Sphinx mode: the BOM is gone, so the directive fires.
        let pf = parse_sphinx(tmp.path(), "main", ".. include:: bom.txt\n")
            .root
            .pformat();
        assert!(pf.contains("<note>"), "must strip the BOM: {pf}");
        assert!(!pf.contains('\u{feff}'), "{pf}");
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: bom2.txt\n");
        assert_eq!(paragraphs_of(&tree), vec!["hello bom".to_string()]);

        // Docutils mode: the BOM survives, exactly as bare docutils does.
        let tree = parse_docutils(&tmp.path().join("main.rst"), ".. include:: bom2.txt\n");
        assert_eq!(paragraphs_of(&tree), vec!["\u{feff}hello bom".to_string()]);

        // An explicit `:encoding:` still wins over either default.
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: bom2.txt\n   :encoding: utf-8\n",
        );
        assert_eq!(paragraphs_of(&tree), vec!["\u{feff}hello bom".to_string()]);
    }

    #[test]
    fn latin_1_decodes_the_bytes_utf_8_rejects() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("inc.rst"), b"caf\xe9\n").unwrap();
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :encoding: latin-1\n",
        );
        assert_eq!(paragraphs_of(&tree), vec!["caf\u{e9}".to_string()]);
    }

    /// The `source_encoding` config key (panel fix round B, [19]) is the
    /// `:encoding:` default of BOTH file-inserting directives in sphinx
    /// mode — `include` via `settings.input_encoding`
    /// (`environment/__init__.py:375`), `literalinclude` via
    /// `config.source_encoding` (`code.py:210`). Probed on the pinned
    /// toolchain with `source_encoding = 'latin-1'` over `caf\xe9 here\n`:
    /// both nodes render `café here`. An explicit `:encoding:` still
    /// wins, and docutils mode ignores the key (its own default is
    /// `'utf-8'`, so the bytes fail to decode there).
    #[test]
    fn source_encoding_is_the_default_for_include_and_literalinclude() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("inc.txt"), b"caf\xe9 here\n").unwrap();
        let parse = |main: &str, encoding: &str, sphinx: bool| {
            crate::rst::parse_rst(
                main,
                &ParseOptions {
                    source_path: tmp.path().join("main.rst").display().to_string(),
                    sphinx,
                    docname: "main".to_string(),
                    srcdir: Some(tmp.path().to_path_buf()),
                    source_encoding: encoding.to_string(),
                    ..Default::default()
                },
            )
        };

        let tree = parse(
            ".. include:: inc.txt\n\n.. literalinclude:: inc.txt\n",
            "latin-1",
            true,
        );
        assert_eq!(paragraphs_of(&tree), vec!["caf\u{e9} here".to_string()]);
        let literal = tree
            .root
            .children
            .iter()
            .find(|node| node.kind == kinds::LITERAL_BLOCK)
            .expect("the literalinclude block");
        assert_eq!(literal.astext(), "caf\u{e9} here\n");

        // The default (`utf-8-sig`) cannot decode the byte: include SEVEREs,
        // literalinclude funnels the decode error into its reporter warning.
        let pf = parse(
            ".. include:: inc.txt\n\n.. literalinclude:: inc.txt\n",
            "utf-8-sig",
            true,
        )
        .root
        .pformat();
        assert!(pf.contains("UnicodeDecodeError"), "{pf}");
        assert!(!pf.contains("caf\u{e9}"), "{pf}");

        // `:encoding:` wins over the configured default.
        let tree = parse(
            ".. include:: inc.txt\n   :encoding: latin-1\n",
            "utf-8-sig",
            true,
        );
        assert_eq!(paragraphs_of(&tree), vec!["caf\u{e9} here".to_string()]);

        // Docutils mode: the key is sphinx's, not docutils'.
        let pf = parse(".. include:: inc.txt\n", "latin-1", false)
            .root
            .pformat();
        assert!(pf.contains("UnicodeDecodeError"), "{pf}");
    }

    // ---- row 5: clipping ---------------------------------------------

    fn clipped(tmp: &Path, options: &str) -> Vec<String> {
        write(tmp, "inc.rst", "L1\nL2\nL3\nL4\nL5\n");
        let tree = parse_sphinx(tmp, "main", &format!(".. include:: inc.rst\n{options}"));
        paragraphs_of(&tree)
    }

    #[test]
    fn start_and_end_line_slice_zero_based_end_exclusive() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            clipped(tmp.path(), "   :start-line: 1\n   :end-line: 3\n"),
            vec!["L2\nL3".to_string()]
        );
    }

    #[test]
    fn negative_start_line_counts_from_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            clipped(tmp.path(), "   :start-line: -2\n"),
            vec!["L4\nL5".to_string()]
        );
    }

    #[test]
    fn out_of_range_slices_clamp_silently() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            clipped(tmp.path(), "   :start-line: 99\n"),
            Vec::<String>::new()
        );
        let tmp2 = tempfile::tempdir().unwrap();
        assert_eq!(
            clipped(tmp2.path(), "   :end-line: 99\n"),
            vec!["L1\nL2\nL3\nL4\nL5".to_string()]
        );
    }

    #[test]
    fn start_line_zero_alone_is_a_no_op_trigger() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            clipped(tmp.path(), "   :start-line: 0\n"),
            vec!["L1\nL2\nL3\nL4\nL5".to_string()]
        );
    }

    #[test]
    fn start_after_matches_on_the_character_stream_inside_lines() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "aaa MARK\nbbb\nccc\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :start-after: MARK\n",
        );
        assert_eq!(
            paragraphs_of(&tree),
            vec!["bbb\nccc".to_string()],
            "the match text is a mid-line substring, not a whole line"
        );
    }

    #[test]
    fn clip_order_is_line_slice_then_start_after_then_end_before() {
        let tmp = tempfile::tempdir().unwrap();
        // "L1" only exists OUTSIDE the line slice, so start-after misses.
        write(tmp.path(), "inc.rst", "L1\nL2\nL3\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :start-line: 1\n   :start-after: L1\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, 4);
        assert_eq!(
            msgs[0].3,
            "Problem with \"start-after\" option of \"include\" directive:\nText not found."
        );
    }

    #[test]
    fn end_before_not_found_is_a_severe() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "aaa\nbbb\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :end-before: ZZZ\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].3,
            "Problem with \"end-before\" option of \"include\" directive:\nText not found."
        );
    }

    // ---- row 6: splice mechanics -------------------------------------

    /// The [INC PROBE 1] item layout: padding blank (synthetic source,
    /// lineno 0 for docutils' offset −1), the included lines, the
    /// appended `''` + marker pair carrying the INCLUDED source with
    /// continuing linenos, and the padding blank after (offset len).
    #[test]
    fn the_splice_layout_matches_probe_1() {
        let mut p = BlockParser::new(
            "before para\n\n.. include:: inc.rst\n\nafter para\n",
            "main.rst",
        );
        let request = SpliceRequest {
            segments: vec![
                SpliceSegment {
                    lines: vec![String::new()],
                    source_path: "internal padding before inc.rst".to_string(),
                    first_lineno: 0,
                },
                SpliceSegment {
                    lines: vec![
                        "included para line1".to_string(),
                        "included para line2".to_string(),
                        String::new(),
                        "Bad Title".to_string(),
                        "===".to_string(),
                        String::new(),
                        ".. end of inclusion from \"inc.rst\"".to_string(),
                    ],
                    source_path: "inc.rst".to_string(),
                    first_lineno: 1,
                },
                SpliceSegment {
                    lines: vec![String::new()],
                    source_path: "internal padding after inc.rst".to_string(),
                    first_lineno: 8,
                },
            ],
        };
        let mut stream = std::mem::take(&mut p.top);
        p.apply_splice(&mut stream, 4, request);
        let items: Vec<(String, u32)> = stream
            .iter()
            .map(|l| (p.sources.path(l.source).to_string(), l.lineno))
            .collect();
        assert_eq!(
            items,
            vec![
                ("main.rst".to_string(), 1),
                ("main.rst".to_string(), 2),
                ("main.rst".to_string(), 3),
                ("main.rst".to_string(), 4),
                ("internal padding before inc.rst".to_string(), 0),
                ("inc.rst".to_string(), 1),
                ("inc.rst".to_string(), 2),
                ("inc.rst".to_string(), 3),
                ("inc.rst".to_string(), 4),
                ("inc.rst".to_string(), 5),
                ("inc.rst".to_string(), 6),
                ("inc.rst".to_string(), 7),
                ("internal padding after inc.rst".to_string(), 8),
                ("main.rst".to_string(), 5),
            ]
        );
    }

    /// A document ending in an `include`: docutils appends `''` and the
    /// end-of-inclusion marker to the included lines and pads them with a
    /// blank `internal padding after <source>` line (`misc.py:264-266`,
    /// `statemachine.py:385-393`). The marker reads nothing (`states.py:
    /// 2427-2433`) and the explicit list stops at the padding blank
    /// (`Explicit.blank`, `:2803`), which the top level eats: a message
    /// with no node is located one past the padding line, whatever the
    /// included file ends with. Probed against Sphinx 9.1 (a 1-line
    /// paragraph, bullet list or hyperlink target included last:
    /// `internal padding after inc_….rst:5`).
    #[test]
    fn a_document_ending_in_an_include_ends_past_the_padding() {
        let tmp = tempfile::tempdir().unwrap();
        for (name, content) in [
            ("inc_para.rst", "Inc para.\n"),
            ("inc_list.rst", "- inc item\n"),
            ("inc_target.rst", ".. _t: https://x/\n"),
        ] {
            write(tmp.path(), name, content);
            let out =
                parse_sphinx_full(tmp.path(), "main", &format!("A.\n\n.. include:: {name}\n"));
            let (source, line) = out.end_of_input.expect("a line");
            assert_eq!(
                (out.doctree.sources[usize::from(source)].as_str(), line),
                (format!("internal padding after {name}").as_str(), 5),
                "{name}"
            );
        }
    }

    /// End-to-end over the real directive: the trailing blank prevents an
    /// included file ending in paragraph text from absorbing the marker,
    /// and the marker itself pops silently (no comment node).
    #[test]
    fn the_marker_never_reaches_the_tree_and_never_joins_a_paragraph() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "ends in a paragraph");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            "before\n\n.. include:: inc.rst\n\nafter\n",
        );
        assert_eq!(
            paragraphs_of(&tree),
            vec![
                "before".to_string(),
                "ends in a paragraph".to_string(),
                "after".to_string()
            ]
        );
        assert!(
            !tree.root.pformat().contains("comment"),
            "{}",
            tree.root.pformat()
        );
        assert_eq!(
            tree.sources,
            vec![
                tmp.path().join("main.rst").display().to_string(),
                "internal padding before inc.rst".to_string(),
                "inc.rst".to_string(),
                "internal padding after inc.rst".to_string(),
            ]
        );
    }

    #[test]
    fn included_content_carries_its_own_source_and_line_numbers() {
        let tmp = tempfile::tempdir().unwrap();
        // Lines 3-4 carry a "Title underline too short." WARNING.
        write(
            tmp.path(),
            "part.rst",
            "part first para\n\nBad Title\n======\n",
        );
        let tree = parse_sphinx(tmp.path(), "main", "intro\n\n.. include:: part.rst\n");
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1, "{}", tree.root.pformat());
        assert_eq!(
            (msgs[0].0, msgs[0].1, msgs[0].2.as_str()),
            (2, 4, "part.rst"),
            "attributed to the line WITHIN the included file"
        );
        assert_eq!(msgs[0].3, "Title underline too short.");
    }

    /// The `misc.py:267` TODO reproduced deliberately: a `start-line`
    /// clip restarts line numbers at the clip, so a problem on original
    /// line 4 reports the clipped line 2 (row 10's faithful bug).
    #[test]
    fn a_start_line_clip_keeps_docutils_restart_at_zero_numbering() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "part.rst", "L1\nL2\nBad Title\n======\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: part.rst\n   :start-line: 2\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            (msgs[0].1, msgs[0].2.as_str()),
            (2, "part.rst"),
            "clip-relative, not the original line 4"
        );
    }

    #[test]
    fn the_same_file_twice_sequentially_is_legal() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "same para\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n\n.. include:: inc.rst\n",
        );
        assert_eq!(
            paragraphs_of(&tree),
            vec!["same para".to_string(), "same para".to_string()]
        );
        assert!(messages_of(&tree).is_empty());
    }

    #[test]
    fn circular_inclusion_warns_with_the_chain_and_in_file_attribution() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.rst", "in a\n\n.. include:: b.rst\n");
        write(tmp.path(), "b.rst", "in b\n\n.. include:: a.rst\n");
        let tree = parse_sphinx(tmp.path(), "main", "top\n\n.. include:: a.rst\n");
        assert_eq!(
            paragraphs_of(&tree),
            vec!["top".to_string(), "in a".to_string(), "in b".to_string()]
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1, "{}", tree.root.pformat());
        assert_eq!(
            (msgs[0].0, msgs[0].1, msgs[0].2.as_str()),
            (2, 3, "b.rst"),
            "attributed to the including line INSIDE the included file"
        );
        assert_eq!(
            msgs[0].3,
            "circular inclusion in \"include\" directive:\na.rst\n> b.rst\n> a.rst\n> main.rst"
        );
    }

    #[test]
    fn self_inclusion_warns_with_the_two_entry_chain() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.rst", "in a\n\n.. include:: a.rst\n");
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: a.rst\n");
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].3,
            "circular inclusion in \"include\" directive:\na.rst\n> a.rst\n> main.rst"
        );
    }

    #[test]
    fn different_clip_options_are_not_circular() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "a.rst",
            "L1\nL2\nL3\n\n.. include:: a.rst\n   :start-line: 0\n   :end-line: 3\n",
        );
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: a.rst\n");
        assert!(messages_of(&tree).is_empty(), "{}", tree.root.pformat());
        assert_eq!(
            paragraphs_of(&tree),
            vec!["L1\nL2\nL3".to_string(), "L1\nL2\nL3".to_string()]
        );
    }

    #[test]
    fn an_over_long_line_aborts_with_the_length_limit_warning() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "inc.rst",
            &format!("ok\n{}\n", "x".repeat(10_001)),
        );
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: inc.rst\n");
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, 2);
        assert_eq!(
            msgs[0].3,
            "\"inc.rst\": line 2 exceeds the line-length-limit."
        );
        assert!(paragraphs_of(&tree).is_empty(), "the include aborts");
    }

    #[test]
    fn the_length_limit_line_number_carries_the_start_line_bias() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "inc.rst",
            &format!("ok\n{}\n", "x".repeat(10_001)),
        );
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :start-line: 1\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].3, "\"inc.rst\": line 2 exceeds the line-length-limit.",
            "clipped line 1, biased by start-line 1"
        );
    }

    /// `string2lines` rstrips each line with Python's `str.rstrip()`
    /// (`DU/statemachine.py:1516`) and the line-length-limit check runs on
    /// that OUTPUT (`misc.py:245-250`): a member line of 10 000 characters
    /// plus a trailing `\x1f` is 10 001 characters raw and exactly the limit
    /// once rstripped, so docutils parses it as a paragraph (probe-pinned,
    /// panel fix round F; Rust's `trim_end()` had kept the `\x1f`).
    #[test]
    fn an_included_line_at_the_limit_plus_a_trailing_us_is_not_over_the_limit() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "long_us.rst",
            &format!("{}\x1f\n", "a".repeat(10_000)),
        );
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: long_us.rst\n");
        assert!(messages_of(&tree).is_empty(), "{}", tree.root.pformat());
        assert_eq!(paragraphs_of(&tree), vec!["a".repeat(10_000)]);
    }

    #[test]
    fn insert_mode_expands_tabs_at_the_given_tab_width() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "a\tb\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :tab-width: 4\n",
        );
        assert_eq!(paragraphs_of(&tree), vec!["a   b".to_string()]);
    }

    #[test]
    fn insert_mode_negative_tab_width_removes_tabs_like_python() {
        // Probe-pinned: expandtabs with a non-positive width removes the
        // tab outright ("a\tb" -> "ab").
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "a\tb\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :tab-width: -3\n",
        );
        assert_eq!(paragraphs_of(&tree), vec!["ab".to_string()]);
    }

    #[test]
    fn an_empty_included_file_inserts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            "before\n\n.. include:: inc.rst\n\nafter\n",
        );
        assert_eq!(
            paragraphs_of(&tree),
            vec!["before".to_string(), "after".to_string()]
        );
        assert!(messages_of(&tree).is_empty());
    }

    /// Included lines join the enclosing document's section hierarchy —
    /// the reason include is a splice, not a detached parse (§Scope-2).
    #[test]
    fn included_sections_join_the_enclosing_hierarchy() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "part.rst",
            "Sub Section\n-----------\n\nsub body\n",
        );
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            "Top\n===\n\ntop body\n\n.. include:: part.rst\n",
        );
        let pf = tree.root.pformat();
        let top = tree
            .root
            .children
            .iter()
            .find(|n| n.kind == kinds::SECTION)
            .expect("top section");
        assert!(
            top.children.iter().any(|n| n.kind == kinds::SECTION),
            "the included section nests under the enclosing one: {pf}"
        );
    }

    // ---- row 7: literal / code / parser modes ------------------------

    #[test]
    fn literal_mode_matches_the_probe_shape() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "some *raw* text\n\tafter tab\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :literal:\n   :class: foo\n   :name: lit1\n",
        );
        assert_eq!(
            tree.root.pformat(),
            "<document source=\"{}\">\n    <literal_block classes=\"foo\" ids=\"lit1\" \
             names=\"lit1\" source=\"inc.rst\" xml:space=\"preserve\">\n        some *raw* \
             text\n                after tab\n"
                .replace("{}", &tmp.path().join("main.rst").display().to_string())
        );
    }

    #[test]
    fn literal_number_lines_pads_to_the_last_line_width() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "L1\nL2\nL3\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :literal:\n   :number-lines: 8\n",
        );
        let pf = tree.root.pformat();
        for expected in [" 8 ", " 9 ", "10 "] {
            assert!(
                pf.contains(&format!(
                    "<inline classes=\"ln\">\n            {expected}\n"
                )),
                "{pf}"
            );
        }
    }

    #[test]
    fn literal_number_lines_flag_numbers_the_clip_from_one() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "L1\nL2\nL3\nL4\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :literal:\n   :start-line: 2\n   :number-lines:\n",
        );
        let pf = tree.root.pformat();
        assert!(
            pf.contains("<literal_block source=\"inc.rst\" xml:space=\"preserve\">"),
            "{pf}"
        );
        assert!(pf.contains("1 \n        L3"), "{pf}");
        assert!(pf.contains("2 \n        L4"), "{pf}");
    }

    /// The `:number-lines:` column is sized from docutils'
    /// `len(text.splitlines())` on the suffix-stripped expanded text
    /// (`misc.py:174-176`), not from the rendered `split('\n')` lines. On
    /// an EMPTY file the two disagree — `''.splitlines()` is zero lines,
    /// `''.split('\n')` is one — and a split-based count padded the single
    /// rendered number to width 2. docutils 0.22.4 renders exactly
    /// `<inline classes="ln">` / `1 ` (probe: empty.txt + `:literal:` +
    /// `:number-lines:`, and the same for `:code:` / `:code: text`).
    #[test]
    fn literal_number_lines_on_an_empty_file_is_width_one() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "empty.txt", "");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: empty.txt\n   :literal:\n   :number-lines:\n",
        );
        let pf = tree.root.pformat();
        assert!(
            pf.contains("<inline classes=\"ln\">\n            1 \n"),
            "{pf}"
        );
        assert!(!pf.contains("             1 "), "padded to width 2: {pf}");
        // A one-line file also renders width 1 (`endline = 1 + 1 = 2`).
        write(tmp.path(), "one.txt", "alpha\n");
        let pf = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: one.txt\n   :literal:\n   :number-lines:\n",
        )
        .root
        .pformat();
        assert!(
            pf.contains("<inline classes=\"ln\">\n            1 \n        alpha\n"),
            "{pf}"
        );
    }

    /// The number column is sized from `len(text.splitlines())` AFTER the
    /// single trailing newline is removed (`misc.py:175-176`), not from
    /// the raw line count. A file whose last line is blank therefore
    /// counts one fewer — and at a digit boundary that is a whole space of
    /// padding.
    ///
    // oracle (scratchpad A/nlprobe.py, sphinx 9.1.0 dummy build):
    //   nine.txt = '1\n2\n3\n4\n5\n6\n7\n8\n\n' -> `1 ` .. `9 ` (width 1)
    //   ten.txt  = '1\n..\n9\n\n'                  -> ` 1 ` .. `10 ` (width 2)
    #[test]
    fn literal_number_lines_width_drops_the_trailing_blank_line() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "nine.txt", "1\n2\n3\n4\n5\n6\n7\n8\n\n");
        let pf = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: nine.txt\n   :literal:\n   :number-lines:\n",
        )
        .root
        .pformat();
        assert!(
            pf.contains("<inline classes=\"ln\">\n            1 \n        1\n"),
            "width must be 1: {pf}"
        );
        assert!(
            pf.ends_with("<inline classes=\"ln\">\n            9 \n"),
            "{pf}"
        );
        assert!(!pf.contains("             1 "), "padded to width 2: {pf}");

        // One more content line crosses the boundary: lastline is 10.
        write(tmp.path(), "ten.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n\n");
        let pf = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: ten.txt\n   :literal:\n   :number-lines:\n",
        )
        .root
        .pformat();
        assert!(
            pf.contains("<inline classes=\"ln\">\n             1 \n        1\n"),
            "width must be 2: {pf}"
        );
        assert!(
            pf.ends_with("<inline classes=\"ln\">\n            10 \n"),
            "{pf}"
        );
    }

    /// Pins OUR `:code:` shape: this crate's `code` machinery is the
    /// Pygments-less docutils (wave 3), so a language argument fails with
    /// the pygments WARNING where the sphinx oracle (pygments installed)
    /// would tokenize — a documented divergence for T14 to exclude.
    #[test]
    fn code_mode_with_a_language_pins_our_pygments_less_shape() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "x = 1\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :code: python\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, 2);
        assert_eq!(
            msgs[0].3,
            "Cannot analyze code. Pygments package not found."
        );
    }

    #[test]
    fn code_mode_without_a_language_is_a_code_classed_literal_with_source() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "x = 1\ny = 2\n");
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: inc.rst\n   :code:\n");
        let pf = tree.root.pformat();
        assert!(
            pf.contains(
                "<literal_block classes=\"code\" source=\"inc.rst\" xml:space=\"preserve\">\n        x = 1\n        y = 2\n"
            ),
            "{pf}"
        );
    }

    #[test]
    fn code_mode_number_lines_uses_the_flag_or_int_value() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "x = 1\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :code:\n   :number-lines: 5\n",
        );
        let pf = tree.root.pformat();
        assert!(
            pf.contains("<inline classes=\"ln\">\n            5 \n"),
            "{pf}"
        );
    }

    /// The include-called CodeBlock receives its whole file as ONE
    /// content element (`[text.removesuffix('\n')]`, misc.py:187-205), so
    /// body.py:194's `endline = startline + len(self.content)` is
    /// `startline + 1` and the number column is sized for THAT — probed
    /// verbatim against docutils 0.22.4 this session: a 12-line file with
    /// a bare `:number-lines:` renders `1 `..`9 `, then `10 `..`12 ` with
    /// no padding (ragged width-1 column), where the plain `code`
    /// directive over the same 12 lines pads to width 2 (` 1 `).
    #[test]
    fn code_mode_number_column_width_comes_from_the_single_content_element() {
        let tmp = tempfile::tempdir().unwrap();
        let twelve: String = (1..=12).map(|i| format!("line{i}\n")).collect();
        write(tmp.path(), "inc.rst", &twelve);
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :code:\n   :number-lines:\n",
        );
        let pf = tree.root.pformat();
        // endline = 1 + 1 = 2 -> width 1: no padding anywhere, even for
        // the two-digit numbers.
        for n in [1, 9, 10, 12] {
            assert!(
                pf.contains(&format!("<inline classes=\"ln\">\n            {n} \n")),
                "{pf}"
            );
        }
        assert!(
            !pf.contains("\n             1 \n"),
            "a width-2 padded column would be the real-count shape: {pf}"
        );

        // Contrast case crossing the digit boundary the other way:
        // 3 lines from 8 -> endline = 8 + 1 = 9 -> width 1 ("8 ", "9 ",
        // "10 "), where :literal: (real count: lastline 11) pads to
        // width 2 (" 8 ") — probed verbatim.
        let tmp2 = tempfile::tempdir().unwrap();
        write(tmp2.path(), "inc.rst", "L1\nL2\nL3\n");
        let code = parse_sphinx(
            tmp2.path(),
            "main",
            ".. include:: inc.rst\n   :code:\n   :number-lines: 8\n",
        )
        .root
        .pformat();
        for n in [8, 9, 10] {
            assert!(
                code.contains(&format!("<inline classes=\"ln\">\n            {n} \n")),
                "{code}"
            );
        }
        assert!(!code.contains("\n             8 \n"), "{code}");
    }

    /// The include log is document-level state: an include running inside
    /// a DETACHED sub-parse (a csv-table cell) must still see the outer
    /// open inclusion, so circularity is detected across the boundary.
    #[test]
    fn circularity_is_detected_across_a_detached_parse_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "a.rst",
            "in a\n\n.. csv-table::\n\n   \".. include:: a.rst\"\n",
        );
        let tree = parse_sphinx(tmp.path(), "main", ".. include:: a.rst\n");
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1, "{}", tree.root.pformat());
        assert_eq!(msgs[0].0, 2);
        assert_eq!(
            msgs[0].3, "circular inclusion in \"include\" directive:\na.rst\n> a.rst\n> main.rst",
            "the cell's sub-parser saw the outer log entry"
        );
        // ...and the log transferred back out: the outer marker still
        // pops cleanly, so a sequential re-include stays legal. The
        // paragraph check alone does not discriminate (it holds whether or
        // not the log leaked), so pin the pop directly: a leaked marker
        // leaves the "end of inclusion" comment line unconsumed.
        let pformat = tree.root.pformat();
        assert!(!pformat.contains("end of inclusion"), "{pformat}");
        assert_eq!(paragraphs_of(&tree), vec!["in a".to_string()]);
        // A second, SEQUENTIAL include of the same file is legal — which
        // it would not be if a.rst were still on the log.
        let again = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: a.rst\n\n.. include:: a.rst\n",
        );
        assert_eq!(
            messages_of(&again).len(),
            2,
            "one circularity message per csv cell, none from the resequence: {}",
            again.root.pformat()
        );
    }

    #[test]
    fn parser_mode_is_the_documented_unsupported_severe() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "content\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :parser: myst\n",
        );
        let msgs = messages_of(&tree);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, 4);
        assert_eq!(
            msgs[0].3,
            "Problem with \"include\" directive:\nparser mode is not supported by sphinx-ultra \
             (planned with MyST, M2 wave 6)"
        );
    }

    // ---- rows 8-9: the sphinx record layer ---------------------------

    /// Row 8: dependency for every successfully opened project file
    /// (non-doc files included), `included` only for docname-mapping
    /// paths, standard includes recording neither (§Scope-2b), and a
    /// missing file recording `included` but no dependency (sphinx's
    /// `note_included` runs before the open, `other.py:415`).
    #[test]
    fn the_registry_records_dependencies_and_included_docnames() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "part.rst", "part para\n");
        write(tmp.path(), "sub/abs_part.rst", "abs part\n");
        write(tmp.path(), "data.txt", "plain text\n");
        let out = parse_sphinx_full(
            tmp.path(),
            "a",
            "A\n=\n\n.. include:: part.rst\n\n.. include:: /sub/abs_part.rst\n\n\
             .. include:: data.txt\n\n.. include:: <isonum.txt>\n\n\
             .. include:: missing.rst\n",
        );
        assert_eq!(
            out.registry.dependencies,
            vec![
                "part.rst".to_string(),
                "sub/abs_part.rst".to_string(),
                "data.txt".to_string(),
            ],
            "opened files only; standard include excluded"
        );
        assert_eq!(
            out.registry.included,
            vec![
                "part".to_string(),
                "sub/abs_part".to_string(),
                "missing".to_string(),
            ],
            "docname-mapping paths only (.txt maps to no docname), missing file included"
        );
    }

    /// The dependency is recorded at OPEN time (`misc.py:130`), so a file
    /// that fails to DECODE still records.
    #[test]
    fn a_decode_failure_still_records_the_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("inc.rst"), b"caf\xe9\n").unwrap();
        let out = parse_sphinx_full(tmp.path(), "a", ".. include:: inc.rst\n");
        assert_eq!(out.registry.dependencies, vec!["inc.rst".to_string()]);
    }

    /// A standalone (docutils-mode) parse has no environment to replay
    /// into: no records.
    #[test]
    fn docutils_mode_records_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "para\n");
        let out = crate::rst::parse_rst_full(
            ".. include:: inc.rst\n",
            &ParseOptions {
                source_path: tmp.path().join("main.rst").display().to_string(),
                sphinx: false,
                docname: "index".to_string(),
                found_docs: None,
                exclude_patterns: Vec::new(),
                py: Default::default(),
                srcdir: None,
                ..Default::default()
            },
        );
        assert!(out.registry.dependencies.is_empty());
        assert!(out.registry.included.is_empty());
    }

    #[test]
    fn literal_wins_over_code_wins_over_parser() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "inc.rst", "text\n");
        let tree = parse_sphinx(
            tmp.path(),
            "main",
            ".. include:: inc.rst\n   :literal:\n   :code: python\n   :parser: myst\n",
        );
        let pf = tree.root.pformat();
        assert!(messages_of(&tree).is_empty(), "{pf}");
        assert!(pf.contains("<literal_block source=\"inc.rst\""), "{pf}");
    }
}

#[cfg(test)]
mod literalinclude_reader_tests {
    //! The pure `LiteralIncludeReader` battery ([INC §3.2], T13 rows
    //! 1-3 and 6) — no rst parsing anywhere; the reader runs against the
    //! committed probe-mirror fixture module.

    use super::*;
    use std::path::PathBuf;

    /// A committed fixture path, joined ONE SEGMENT AT A TIME: a single
    /// `join("tests/fixtures/literalinclude")` embeds POSIX separators
    /// inside a Windows path, and while that still opens, it renders as
    /// the mixed `D:\repo\tests/fixtures/literalinclude\example.py`.
    fn fixture(name: &str) -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for segment in ["tests", "fixtures", "literalinclude", name] {
            path.push(segment);
        }
        path
    }

    fn example() -> PathBuf {
        fixture("example.py")
    }

    /// Set one INVALID_OPTIONS_PAIR-relevant option by its sphinx name.
    fn set_opt(options: &mut LiteralIncludeOptions, name: &str) {
        match name {
            "lineno-match" => options.lineno_match = true,
            "lineno-start" => options.lineno_start = Some(1),
            "append" => options.append = Some("x".to_string()),
            "prepend" => options.prepend = Some("x".to_string()),
            "start-after" => options.start_after = Some("x".to_string()),
            "start-at" => options.start_at = Some("x".to_string()),
            "end-before" => options.end_before = Some("x".to_string()),
            "end-at" => options.end_at = Some("x".to_string()),
            "diff" => options.diff = Some(PathBuf::from("x")),
            "pyobject" => options.pyobject = Some("x".to_string()),
            "lines" => options.lines = Some("1".to_string()),
            other => panic!("unknown option {other}"),
        }
    }

    fn read(options: LiteralIncludeOptions) -> Result<(String, usize), String> {
        LiteralIncludeReader::new(example(), options, "utf-8-sig")?.read()
    }

    fn read_with_warnings(
        options: LiteralIncludeOptions,
    ) -> (Result<(String, usize), String>, Vec<String>) {
        let mut reader =
            LiteralIncludeReader::new(example(), options, "utf-8-sig").expect("no option conflict");
        let result = reader.read();
        (result, reader.take_warnings())
    }

    // ---- row 1: INVALID_OPTIONS_PAIR --------------------------------

    #[test]
    fn every_invalid_options_pair_errs_with_the_exact_text() {
        for (option1, option2) in LITERALINCLUDE_INVALID_PAIRS {
            let mut options = LiteralIncludeOptions::default();
            set_opt(&mut options, option1);
            set_opt(&mut options, option2);
            let err = LiteralIncludeReader::new(example(), options, "utf-8-sig")
                .err()
                .unwrap_or_else(|| panic!("{option1}+{option2} must conflict"));
            assert_eq!(
                err,
                format!("Cannot use both \"{option1}\" and \"{option2}\" options")
            );
        }
    }

    /// The matrix is checked in list order: with lineno-match,
    /// lineno-start AND diff all present, the first listed pair names
    /// the error (probed).
    #[test]
    fn the_first_matching_pair_in_list_order_names_the_error() {
        let mut options = LiteralIncludeOptions::default();
        set_opt(&mut options, "lineno-match");
        set_opt(&mut options, "lineno-start");
        set_opt(&mut options, "diff");
        assert_eq!(
            LiteralIncludeReader::new(example(), options, "utf-8-sig")
                .err()
                .unwrap(),
            "Cannot use both \"lineno-match\" and \"lineno-start\" options"
        );
    }

    // ---- row 2: chain order -----------------------------------------

    /// `start-at` runs BEFORE `lines`: the line spec addresses the
    /// clipped region, not the file.
    #[test]
    fn lines_apply_after_the_start_clip() {
        let options = LiteralIncludeOptions {
            start_at: Some("class Foo".to_string()),
            lines: Some("1-2".to_string()),
            ..Default::default()
        };
        let (text, count) = read(options).unwrap();
        assert_eq!(text, "class Foo:\n    \"\"\"A class.\"\"\"\n");
        assert_eq!(count, 2);
    }

    /// `dedent` runs BEFORE `prepend`/`append`, and the returned count
    /// is post-filter (the probe-pinned prepend-append-dedent shape).
    #[test]
    fn dedent_precedes_prepend_and_append_and_count_is_post_filter() {
        let options = LiteralIncludeOptions {
            lines: Some("15-17".to_string()),
            dedent: Some(Some(4)),
            prepend: Some("# begin".to_string()),
            append: Some("# end".to_string()),
            ..Default::default()
        };
        let (text, count) = read(options).unwrap();
        assert_eq!(
            text,
            "# begin\n\ndef method(self):\n    return self.attr\n# end\n"
        );
        assert_eq!(count, 5);
    }

    /// `pyobject` is the FIRST chain slot: `start-at` searches the tag's
    /// slice, not the file, so a pattern that exists OUTSIDE the object
    /// (`def top`, line 6) is not found (probe
    /// `pyobject_startat_outside`).
    #[test]
    fn pyobject_slot_runs_before_the_start_filter() {
        let options = LiteralIncludeOptions {
            pyobject: Some("Foo".to_string()),
            start_at: Some("def top".to_string()),
            ..Default::default()
        };
        assert_eq!(
            read(options).err().unwrap(),
            "start-at pattern not found: def top"
        );
    }

    /// `lines[start - 1:end]` over the 1-based inclusive tag, and
    /// `lineno-match` ASSIGNS `lineno_start = start` (probe
    /// `pyobject_method_lineno_match`: `linenostart: 16`). The class tag
    /// carries its interior blank lines and stops at 17 — its trailing
    /// blanks were trimmed by the analyzer (probe `fixture`).
    #[test]
    fn pyobject_slices_the_tag_and_lineno_match_assigns_its_start() {
        let mut reader = LiteralIncludeReader::new(
            example(),
            LiteralIncludeOptions {
                pyobject: Some("Foo.method".to_string()),
                lineno_match: true,
                ..Default::default()
            },
            "utf-8-sig",
        )
        .expect("no option conflict");
        let (text, count) = reader.read().expect("Foo.method is a tag");
        assert_eq!(text, "    def method(self):\n        return self.attr\n");
        assert_eq!(count, 2);
        assert_eq!(reader.lineno_start, 16);

        let options = LiteralIncludeOptions {
            pyobject: Some("Foo".to_string()),
            ..Default::default()
        };
        let (text, count) = read(options).expect("Foo is a tag");
        assert_eq!(
            text,
            "class Foo:\n    \"\"\"A class.\"\"\"\n\n    attr = 2\n\n\
             \x20   def method(self):\n        return self.attr\n"
        );
        assert_eq!(count, 7);
    }

    /// The pyobject start ASSIGNS, the later filters ADD: probe
    /// `pyobject_startat_linenomatch` pins `linenostart: 14` for
    /// `Foo` (11) plus the `attr` offset (3).
    #[test]
    fn lineno_match_composes_the_pyobject_start_with_the_start_filter() {
        let mut reader = LiteralIncludeReader::new(
            example(),
            LiteralIncludeOptions {
                pyobject: Some("Foo".to_string()),
                start_at: Some("attr".to_string()),
                lineno_match: true,
                ..Default::default()
            },
            "utf-8-sig",
        )
        .expect("no option conflict");
        let (text, _) = reader.read().expect("Foo is a tag");
        assert_eq!(
            text,
            "    attr = 2\n\n    def method(self):\n        return self.attr\n"
        );
        assert_eq!(reader.lineno_start, 14);
    }

    /// An unknown object name errs with the `_StrPath(...)` text (probe
    /// `pyobject_missing`) — reached only because the analyzer succeeded
    /// and simply has no such tag.
    #[test]
    fn an_unknown_pyobject_names_the_include_file() {
        let options = LiteralIncludeOptions {
            pyobject: Some("Nope".to_string()),
            ..Default::default()
        };
        assert_eq!(
            read(options).err().unwrap(),
            format!(
                "Object named 'Nope' not found in include file _StrPath({})",
                py_repr(Some(&example().display().to_string()))
            )
        );
    }

    /// An analyzer failure keeps sphinx's `parsing %r failed: ` prefix with
    /// our own detail: sphinx interpolates a CPython `SyntaxError` repr
    /// there, which this port cannot reproduce (probe
    /// `pyobject_broken_triple`: `parsing '<abs>/broken2.py' failed:
    /// SyntaxError('unterminated triple-quoted string literal (detected at
    /// line 3)', ('<unknown>', 1, 5, 'x = """abc', 1, 5))`). See
    /// `crate::py::pycode` for the divergence.
    #[test]
    fn an_analyzer_failure_errs_with_the_parsing_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let broken = tmp.path().join("broken2.py");
        std::fs::write(&broken, "x = \"\"\"abc\ndef f():\n    pass\n").unwrap();
        let options = LiteralIncludeOptions {
            pyobject: Some("f".to_string()),
            ..Default::default()
        };
        let err = LiteralIncludeReader::new(broken.clone(), options, "utf-8-sig")
            .expect("no option conflict")
            .read()
            .err()
            .unwrap();
        assert_eq!(
            err,
            format!(
                "parsing {} failed: unterminated triple-quoted string literal \
                 (detected at line 3)",
                py_repr(Some(&broken.display().to_string()))
            )
        );
    }

    // ---- row 3: filter semantics + exact texts ----------------------

    #[test]
    fn start_at_matches_a_mid_line_substring_and_keeps_the_line() {
        let options = LiteralIncludeOptions {
            start_at: Some("s Foo".to_string()),
            end_at: Some("\"\"\"A class".to_string()),
            ..Default::default()
        };
        let (text, _) = read(options).unwrap();
        assert_eq!(text, "class Foo:\n    \"\"\"A class.\"\"\"\n");
    }

    #[test]
    fn start_after_drops_through_the_matched_line() {
        let options = LiteralIncludeOptions {
            start_after: Some("def tail".to_string()),
            ..Default::default()
        };
        let (text, count) = read(options).unwrap();
        assert_eq!(text, "    pass\n");
        assert_eq!(count, 1);
    }

    #[test]
    fn start_and_end_not_found_texts_are_exact() {
        for (set, expected) in [
            (
                "start-after",
                "start-after pattern not found: NOPE".to_string(),
            ),
            ("start-at", "start-at pattern not found: NOPE".to_string()),
            (
                "end-before",
                "end-before pattern not found: NOPE".to_string(),
            ),
            ("end-at", "end-at pattern not found: NOPE".to_string()),
        ] {
            let mut options = LiteralIncludeOptions::default();
            set_opt(&mut options, set);
            match set {
                "start-after" => options.start_after = Some("NOPE".to_string()),
                "start-at" => options.start_at = Some("NOPE".to_string()),
                "end-before" => options.end_before = Some("NOPE".to_string()),
                "end-at" => options.end_at = Some("NOPE".to_string()),
                _ => unreachable!(),
            }
            assert_eq!(read(options).err().unwrap(), expected);
        }
    }

    /// `end-before` ignores a first-line match and keeps scanning: the
    /// next match wins.
    #[test]
    fn end_before_skips_a_first_line_match_and_takes_the_next() {
        let options = LiteralIncludeOptions {
            start_at: Some("def top".to_string()),
            end_before: Some("def".to_string()),
            ..Default::default()
        };
        // Region starts at file line 6 ("def top(x):"); the first-line
        // match is skipped, the next "def" is "    def method" at
        // region index 10 -> lines[..10] = file lines 6-15.
        let (text, count) = read(options).unwrap();
        assert_eq!(count, 10);
        assert!(text.starts_with("def top(x):\n"));
        assert!(text.ends_with("    attr = 2\n\n"));
    }

    /// A first-line-ONLY match raises not-found (probed: the loop's
    /// `pass` keeps scanning and falls off the end).
    #[test]
    fn end_before_with_only_a_first_line_match_errs_not_found() {
        let options = LiteralIncludeOptions {
            start_at: Some("def tail".to_string()),
            end_before: Some("def tail".to_string()),
            ..Default::default()
        };
        assert_eq!(
            read(options).err().unwrap(),
            "end-before pattern not found: def tail"
        );
    }

    #[test]
    fn end_at_keeps_the_matched_line() {
        let options = LiteralIncludeOptions {
            end_at: Some("CONST".to_string()),
            ..Default::default()
        };
        let (text, count) = read(options).unwrap();
        assert_eq!(text, "\"\"\"Example module.\"\"\"\n\nCONST = 1\n");
        assert_eq!(count, 3);
    }

    #[test]
    fn parse_line_num_spec_semantics_and_error_texts() {
        // 0-based single.
        let spec = parse_line_num_spec("3", 21).unwrap();
        assert_eq!(spec.in_range_values(21), vec![2]);
        // `0` parses to -1 (Python int('0') - 1).
        let spec = parse_line_num_spec("0", 21).unwrap();
        assert_eq!(spec.in_range_values(21), vec![-1]);
        // Open left: `-10` is range(0, 10).
        let spec = parse_line_num_spec("-10", 21).unwrap();
        assert_eq!(spec.in_range_values(21), (0..10).collect::<Vec<i64>>());
        // Open right: `10-` is range(9, max(10, total)).
        let spec = parse_line_num_spec("10-", 21).unwrap();
        assert_eq!(spec.in_range_values(21), (9..21).collect::<Vec<i64>>());
        // Open right past the end: `10-` with 5 lines is range(9, 10) —
        // wholly out of range, nothing selected.
        let spec = parse_line_num_spec("10-", 5).unwrap();
        assert!(spec.any_out_of_range(5));
        assert!(spec.in_range_values(5).is_empty());
        // Duplicates and written order are preserved.
        let spec = parse_line_num_spec("6-8,1,1", 21).unwrap();
        assert_eq!(spec.in_range_values(21), vec![5, 6, 7, 0, 0]);
        // Reversed, bare dash, three-part and non-numeric specs all
        // raise the exact `invalid line number spec: {spec!r}` text.
        for bad in ["5-3", "-", "1-2-3", "x", "", "1,"] {
            assert_eq!(
                parse_line_num_spec(bad, 21).err().unwrap(),
                format!("invalid line number spec: {}", py_repr(Some(bad))),
                "spec {bad:?}"
            );
        }
    }

    #[test]
    fn out_of_range_lines_warn_and_drop() {
        let options = LiteralIncludeOptions {
            lines: Some("1,99".to_string()),
            ..Default::default()
        };
        let (result, warnings) = read_with_warnings(options);
        let (text, count) = result.unwrap();
        assert_eq!(text, "\"\"\"Example module.\"\"\"\n");
        assert_eq!(count, 1);
        assert_eq!(
            warnings,
            vec!["line number spec is out of range(1-21): '1,99'".to_string()]
        );
    }

    /// `:lines: 99`: the out-of-range warning fires AND the empty
    /// selection raises — with the `_StrPath(...)` repr in the bytes.
    #[test]
    fn no_lines_pulled_errs_with_the_strpath_repr_after_warning() {
        let options = LiteralIncludeOptions {
            lines: Some("99".to_string()),
            ..Default::default()
        };
        let (result, warnings) = read_with_warnings(options);
        assert_eq!(
            warnings,
            vec!["line number spec is out of range(1-21): '99'".to_string()]
        );
        assert_eq!(
            result.err().unwrap(),
            format!(
                "Line spec '99': no lines pulled from include file _StrPath({})",
                py_repr(Some(&example().display().to_string()))
            )
        );
    }

    #[test]
    fn lineno_match_with_disjoint_lines_errs() {
        let options = LiteralIncludeOptions {
            lines: Some("1,6".to_string()),
            lineno_match: true,
            ..Default::default()
        };
        assert_eq!(
            read(options).err().unwrap(),
            "Cannot use \"lineno-match\" with a disjoint set of \"lines\""
        );
    }

    /// Python's negative-index wrap: `:lines: 0` selects the LAST line.
    #[test]
    fn lines_zero_wraps_to_the_last_line() {
        let options = LiteralIncludeOptions {
            lines: Some("0".to_string()),
            ..Default::default()
        };
        let (text, count) = read(options).unwrap();
        assert_eq!(text, "    pass\n");
        assert_eq!(count, 1);
    }

    // ---- lineno-match arithmetic per filter -------------------------

    #[test]
    fn lineno_match_arithmetic_start_at() {
        let options = LiteralIncludeOptions {
            start_at: Some("class Foo".to_string()),
            lineno_match: true,
            ..Default::default()
        };
        let mut reader = LiteralIncludeReader::new(example(), options, "utf-8-sig").unwrap();
        reader.read().unwrap();
        // 1 + lineno(10) — start-at keeps the matched line.
        assert_eq!(reader.lineno_start, 11);
    }

    #[test]
    fn lineno_match_arithmetic_start_after() {
        let options = LiteralIncludeOptions {
            start_after: Some("\"\"\"Example module.\"\"\"".to_string()),
            lineno_match: true,
            ..Default::default()
        };
        let mut reader = LiteralIncludeReader::new(example(), options, "utf-8-sig").unwrap();
        reader.read().unwrap();
        // 1 + lineno(0) + 1 — start-after drops through the match.
        assert_eq!(reader.lineno_start, 2);
    }

    #[test]
    fn lineno_match_arithmetic_lines() {
        let options = LiteralIncludeOptions {
            lines: Some("6-8".to_string()),
            lineno_match: true,
            ..Default::default()
        };
        let mut reader = LiteralIncludeReader::new(example(), options, "utf-8-sig").unwrap();
        let (text, _) = reader.read().unwrap();
        assert_eq!(
            text,
            "def top(x):\n    \"\"\"Top function.\"\"\"\n    return x + 1\n"
        );
        // 1 + linelist[0] (5) — the probe-pinned linenostart 6.
        assert_eq!(reader.lineno_start, 6);
    }

    /// `:lines: 0` + lineno-match: [-1] is trivially contiguous and
    /// biases the start DOWN — linenostart 0 (probed).
    #[test]
    fn lineno_match_arithmetic_lines_zero() {
        let options = LiteralIncludeOptions {
            lines: Some("0".to_string()),
            lineno_match: true,
            ..Default::default()
        };
        let mut reader = LiteralIncludeReader::new(example(), options, "utf-8-sig").unwrap();
        reader.read().unwrap();
        assert_eq!(reader.lineno_start, 0);
    }

    // ---- dedent -----------------------------------------------------

    #[test]
    fn int_dedent_preserves_a_bare_newline_and_warns_on_stripped_text() {
        let options = LiteralIncludeOptions {
            lines: Some("15-17".to_string()),
            dedent: Some(Some(4)),
            ..Default::default()
        };
        let (result, warnings) = read_with_warnings(options);
        // Line 15 is bare "\n": the cut empties it and the '\n' is put
        // back; the indented lines lose exactly 4 columns.
        assert_eq!(
            result.unwrap().0,
            "\ndef method(self):\n    return self.attr\n"
        );
        assert!(warnings.is_empty());

        let options = LiteralIncludeOptions {
            lines: Some("1".to_string()),
            dedent: Some(Some(2)),
            ..Default::default()
        };
        let (result, warnings) = read_with_warnings(options);
        assert_eq!(result.unwrap().0, "\"Example module.\"\"\"\n");
        assert_eq!(
            warnings,
            vec!["non-whitespace stripped by dedent".to_string()]
        );
    }

    /// Dedent past every line's end: lines become bare newlines, one
    /// warning (probed).
    #[test]
    fn int_dedent_past_line_ends_leaves_bare_newlines() {
        let options = LiteralIncludeOptions {
            lines: Some("11-13".to_string()),
            dedent: Some(Some(400)),
            ..Default::default()
        };
        let (result, warnings) = read_with_warnings(options);
        assert_eq!(result.unwrap().0, "\n\n\n");
        assert_eq!(
            warnings,
            vec!["non-whitespace stripped by dedent".to_string()]
        );
    }

    /// Bare `:dedent:` is a full `textwrap.dedent` (probed shape).
    #[test]
    fn bare_dedent_runs_textwrap_dedent() {
        let options = LiteralIncludeOptions {
            lines: Some("12-17".to_string()),
            dedent: Some(None),
            ..Default::default()
        };
        let (text, count) = read(options).unwrap();
        assert_eq!(
            text,
            "\"\"\"A class.\"\"\"\n\nattr = 2\n\ndef method(self):\n    return self.attr\n"
        );
        assert_eq!(count, 6);
    }

    /// The 3.12 `textwrap.dedent` port, pinned against this session's
    /// interpreter probes (incl. the mixed-tab/space no-op, whitespace-
    /// only-line blanking, and the unterminated trailing segment).
    #[test]
    fn textwrap_dedent_port_matches_python() {
        for (input, expected) in [
            ("  a\n    b\n", "a\n  b\n"),
            ("  a\n\t b\n", "  a\n\t b\n"),
            ("\ta\n\tb\n", "a\nb\n"),
            ("  a\n   \n  b\n", "a\n\nb\n"),
            ("  a\n  b", "a\nb"),
            ("  a\nb\n", "  a\nb\n"),
            ("    only\n", "only\n"),
            ("  a\n  \n", "a\n\n"),
            ("  a\n  b\n   ", "a\nb\n"),
        ] {
            assert_eq!(py_textwrap_dedent(input), expected, "input {input:?}");
        }
    }

    // ---- read_file: encoding, tabs, universal newlines --------------

    #[test]
    fn missing_file_errs_with_the_exact_text() {
        let path = fixture("nothere.py");
        let options = LiteralIncludeOptions::default();
        let err = LiteralIncludeReader::new(path.clone(), options, "utf-8-sig")
            .unwrap()
            .read()
            .err()
            .unwrap();
        assert_eq!(
            err,
            format!(
                "Include file '{}' not found or reading it failed",
                path.display()
            )
        );
    }

    /// The default encoding is sphinx's `source_encoding` default
    /// `'utf-8-sig'` (probed in the error bytes), and `:encoding:`
    /// replaces the `%r` in the text.
    #[test]
    fn encoding_error_texts_spell_the_encoding_repr() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.bin");
        std::fs::write(&path, b"caf\xe9 line\n").unwrap();
        let err =
            LiteralIncludeReader::new(path.clone(), LiteralIncludeOptions::default(), "utf-8-sig")
                .unwrap()
                .read()
                .err()
                .unwrap();
        assert_eq!(
            err,
            format!(
                "Encoding 'utf-8-sig' used for reading included file '{}' seems to be \
                 wrong, try giving an :encoding: option",
                path.display()
            )
        );
        let options = LiteralIncludeOptions {
            encoding: Some("ascii".to_string()),
            ..Default::default()
        };
        let err = LiteralIncludeReader::new(path.clone(), options, "utf-8-sig")
            .unwrap()
            .read()
            .err()
            .unwrap();
        assert_eq!(
            err,
            format!(
                "Encoding 'ascii' used for reading included file '{}' seems to be \
                 wrong, try giving an :encoding: option",
                path.display()
            )
        );
        // latin-1 decodes the same bytes fine.
        let options = LiteralIncludeOptions {
            encoding: Some("latin-1".to_string()),
            ..Default::default()
        };
        let (text, _) = LiteralIncludeReader::new(path, options, "utf-8-sig")
            .unwrap()
            .read()
            .unwrap();
        assert_eq!(text, "caf\u{e9} line\n");
    }

    #[test]
    fn tab_width_expands_before_splitting() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tabs.py");
        std::fs::write(&path, "def f():\n\treturn 1\n").unwrap();
        let options = LiteralIncludeOptions {
            tab_width: Some(4),
            ..Default::default()
        };
        let (text, count) = LiteralIncludeReader::new(path, options, "utf-8-sig")
            .unwrap()
            .read()
            .unwrap();
        assert_eq!(text, "def f():\n    return 1\n");
        assert_eq!(count, 2);
    }

    /// A file without a trailing newline: the last line is a real line;
    /// `append` adds its own element, so the two concatenate (probed
    /// `y = 2# after`).
    #[test]
    fn no_trailing_newline_concatenates_with_append() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonl.py");
        std::fs::write(&path, "x = 1\ny = 2").unwrap();
        let options = LiteralIncludeOptions {
            append: Some("# after".to_string()),
            ..Default::default()
        };
        let (text, count) = LiteralIncludeReader::new(path, options, "utf-8-sig")
            .unwrap()
            .read()
            .unwrap();
        assert_eq!(text, "x = 1\ny = 2# after\n");
        assert_eq!(count, 3);
    }

    #[test]
    fn crlf_input_reads_as_universal_newlines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("crlf.py");
        std::fs::write(&path, "a\r\nb\rc\n").unwrap();
        let (text, count) =
            LiteralIncludeReader::new(path, LiteralIncludeOptions::default(), "utf-8-sig")
                .unwrap()
                .read()
                .unwrap();
        assert_eq!(text, "a\nb\nc\n");
        assert_eq!(count, 3);
    }

    // ---- row 6: diff mode -------------------------------------------

    /// The probe-pinned whole-file diff of the fixture pair: headers
    /// `--- old` / `+++ new` with no timestamps, one hunk.
    #[test]
    fn diff_mode_produces_the_probed_unified_diff() {
        let old = fixture("example_old.py");
        let options = LiteralIncludeOptions {
            diff: Some(old.clone()),
            ..Default::default()
        };
        let mut reader = LiteralIncludeReader::new(example(), options, "utf-8-sig").unwrap();
        let (text, count) = reader.read().unwrap();
        let expected = format!(
            "--- {}\n+++ {}\n@@ -1,7 +1,21 @@\n \"\"\"Example module.\"\"\"\n \n\
             -CONST = 0\n+CONST = 1\n \n \n def top(x):\n-    return x\n\
             +    \"\"\"Top function.\"\"\"\n+    return x + 1\n+\n+\n\
             +class Foo:\n+    \"\"\"A class.\"\"\"\n+\n+    attr = 2\n+\n\
             +    def method(self):\n+        return self.attr\n+\n+\n\
             +def tail():\n+    pass\n",
            old.display(),
            example().display()
        );
        assert_eq!(text, expected);
        // 2 headers + 1 hunk line + 23 body lines.
        assert_eq!(count, 26);
        // linenostart stays at its default in diff mode.
        assert_eq!(reader.lineno_start, 1);
    }

    /// Identical files: no groups, no headers — empty text, count 0
    /// (probed empty literal_block).
    #[test]
    fn diff_of_identical_files_is_empty() {
        let options = LiteralIncludeOptions {
            diff: Some(example()),
            ..Default::default()
        };
        let (text, count) = read(options).unwrap();
        assert_eq!(text, "");
        assert_eq!(count, 0);
    }

    /// `prepend`/`append`/`dedent` are LEGAL beside `:diff:` (not in the
    /// invalid matrix) but the diff short-circuit skips the whole filter
    /// chain — no prepend/append lines, no dedent, no dedent warning
    /// (probed this session).
    #[test]
    fn diff_mode_skips_the_legal_but_bypassed_filters() {
        let old = fixture("example_old.py");
        let plain = {
            let options = LiteralIncludeOptions {
                diff: Some(old.clone()),
                ..Default::default()
            };
            read(options).unwrap()
        };
        let options = LiteralIncludeOptions {
            diff: Some(old),
            prepend: Some("# P".to_string()),
            append: Some("# A".to_string()),
            dedent: Some(Some(1)),
            ..Default::default()
        };
        let (result, warnings) = read_with_warnings(options);
        assert_eq!(result.unwrap(), plain);
        assert!(warnings.is_empty());
    }

    /// Missing diff file: the CURRENT file reads first, then the old
    /// one fails with the read_file text (probed).
    #[test]
    fn diff_with_a_missing_old_file_errs_through_read_file() {
        let gone = fixture("gone.py");
        let options = LiteralIncludeOptions {
            diff: Some(gone.clone()),
            ..Default::default()
        };
        assert_eq!(
            read(options).err().unwrap(),
            format!(
                "Include file '{}' not found or reading it failed",
                gone.display()
            )
        );
    }

    /// The difflib port, pinned against CPython 3.12 outputs for a
    /// replace+delete+insert mix (with an unterminated last line kept
    /// verbatim), two far-apart hunks, and an empty old side.
    #[test]
    fn unified_diff_port_matches_python_difflib() {
        let to_lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let a = to_lines(&["a\n", "b\n", "c\n", "d\n", "e"]);
        let b = to_lines(&["a\n", "x\n", "c\n", "e", "f\n"]);
        assert_eq!(
            py_unified_diff(&a, &b, "o", "n"),
            to_lines(&[
                "--- o\n",
                "+++ n\n",
                "@@ -1,5 +1,5 @@\n",
                " a\n",
                "-b\n",
                "+x\n",
                " c\n",
                "-d\n",
                " e",
                "+f\n",
            ])
        );

        let a: Vec<String> = (0..30).map(|i| format!("l{i}\n")).collect();
        let mut b = a.clone();
        b[2] = "l2X\n".to_string();
        b[27] = "l27X\n".to_string();
        assert_eq!(
            py_unified_diff(&a, &b, "o", "n"),
            to_lines(&[
                "--- o\n",
                "+++ n\n",
                "@@ -1,6 +1,6 @@\n",
                " l0\n",
                " l1\n",
                "-l2\n",
                "+l2X\n",
                " l3\n",
                " l4\n",
                " l5\n",
                "@@ -25,6 +25,6 @@\n",
                " l24\n",
                " l25\n",
                " l26\n",
                "-l27\n",
                "+l27X\n",
                " l28\n",
                " l29\n",
            ])
        );

        assert_eq!(
            py_unified_diff(&[], &to_lines(&["a\n"]), "o", "n"),
            to_lines(&["--- o\n", "+++ n\n", "@@ -0,0 +1 @@\n", "+a\n"])
        );
    }

    /// Autojunk: 260 lines where every second one is blank — blank lines
    /// are popular (> n/100 + 1 occurrences), cannot seed matches, and
    /// the grouping still comes out exactly as CPython's (pinned bytes
    /// from a 3.12 probe).
    #[test]
    fn unified_diff_port_reproduces_autojunk_grouping() {
        let mut a: Vec<String> = Vec::new();
        for i in 0..130 {
            a.push(format!("line {i}\n"));
            a.push("\n".to_string());
        }
        let mut b = a.clone();
        b[40] = "line 20 CHANGED\n".to_string();
        b[200] = "line 100 CHANGED\n".to_string();
        b.insert(100, "inserted\n".to_string());
        let expected: Vec<String> = [
            "--- old\n",
            "+++ new\n",
            "@@ -38,7 +38,7 @@\n",
            " \n",
            " line 19\n",
            " \n",
            "-line 20\n",
            "+line 20 CHANGED\n",
            " \n",
            " line 21\n",
            " \n",
            "@@ -98,6 +98,7 @@\n",
            " \n",
            " line 49\n",
            " \n",
            "+inserted\n",
            " line 50\n",
            " \n",
            " line 51\n",
            "@@ -198,7 +199,7 @@\n",
            " \n",
            " line 99\n",
            " \n",
            "-line 100\n",
            "+line 100 CHANGED\n",
            " \n",
            " line 101\n",
            " \n",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(py_unified_diff(&a, &b, "old", "new"), expected);
    }

    #[test]
    fn splitlines_keepends_matches_python_boundaries() {
        assert_eq!(
            py_splitlines_keepends("a\r\nb\rc\x0bd\ne"),
            vec!["a\r\n", "b\r", "c\x0b", "d\n", "e"]
        );
        assert_eq!(py_splitlines_keepends(""), Vec::<String>::new());
        assert_eq!(py_splitlines_keepends("\n\n"), vec!["\n", "\n"]);
    }
}

#[cfg(test)]
mod literalinclude_tests {
    //! rst-level literalinclude battery: node anatomy (row 4), captions
    //! (row 5), diff (row 6), the error funnel (row 7), the dependency
    //! record (row 8), and the warning-channel split (row 3, [INC §3.4])
    //! — pformats and warning bytes pinned against PROBE 4/5 plus this
    //! session's probes.

    use super::*;
    use crate::rst::{parse_rst_full, ParseOptions, ParseOutput};
    use std::path::Path;

    const EXAMPLE_PY: &str = include_str!("../../tests/fixtures/literalinclude/example.py");
    const EXAMPLE_OLD_PY: &str = include_str!("../../tests/fixtures/literalinclude/example_old.py");

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    /// The srcdir as the READER spells it. Sphinx opens (and reports) the
    /// `.resolve()`d path (`environment/__init__.py:475`), so every path
    /// inside a reader message — and the `:diff:` header, which the
    /// reader builds — carries symlinks already followed. On macOS the
    /// system temp dir is itself a symlink, which is what makes the
    /// difference visible here.
    fn resolved(dir: &Path) -> String {
        crate::utils::resolve_path(dir).display().to_string()
    }

    /// The OS-native spelling of `rel` under `dir`, one segment at a
    /// time — which is how the product builds every path it renders
    /// (`relfn2path` pushes segment by segment). An expectation written
    /// `{dir}/{rel}` is a POSIX-only expectation: on Windows the product
    /// renders `\` there, exactly as `sphinx-build` does.
    fn at(dir: &Path, rel: &str) -> String {
        let mut path = dir.to_path_buf();
        for segment in rel.split('/') {
            path.push(segment);
        }
        path.display().to_string()
    }

    /// [`at`] under the RESOLVED directory — the spelling every reader
    /// message carries (see [`resolved`]).
    fn resolved_at(dir: &Path, rel: &str) -> String {
        at(Path::new(&resolved(dir)), rel)
    }

    /// Sphinx-mode parse of `main` as `main.rst` inside a srcdir holding
    /// the fixture module.
    fn parse(srcdir: &Path, main: &str) -> ParseOutput {
        write(srcdir, "example.py", EXAMPLE_PY);
        parse_rst_full(main, &main_options(srcdir))
    }

    /// [`parse`], then Sphinx's read transforms.
    fn parse_and_transform(srcdir: &Path, main: &str) -> ParseOutput {
        write(srcdir, "example.py", EXAMPLE_PY);
        crate::transforms::parse_full_and_transform(main, &main_options(srcdir))
    }

    fn main_options(srcdir: &Path) -> ParseOptions {
        ParseOptions {
            source_path: srcdir.join("main.rst").display().to_string(),
            sphinx: true,
            docname: "main".to_string(),
            srcdir: Some(srcdir.to_path_buf()),
            ..Default::default()
        }
    }

    /// `parse_line_num_spec` (`SP/util/_lines.py`) `strip()`s each part with
    /// Python's set before `int()` sees it, so a spec opening with `\x1f`
    /// selects the same lines as one without: sphinx 9.1.0 renders
    /// `:lines: \x1f1` and `:lines: 1` identically (probe-pinned, panel fix
    /// round F; Rust's `trim()` had left the `\x1f` for `int()` to reject).
    #[test]
    fn a_line_spec_opening_with_a_c0_separator_is_stripped_like_python() {
        let tmp = tempfile::tempdir().unwrap();
        let control = parse(tmp.path(), ".. literalinclude:: example.py\n   :lines: 1\n");
        let with_us = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n   :lines: \x1f1\n",
        );
        assert!(
            messages_of(&with_us).is_empty(),
            "{}",
            with_us.doctree.root.pformat()
        );
        assert_eq!(
            with_us.doctree.root.pformat(),
            control.doctree.root.pformat()
        );
    }

    fn messages_of(output: &ParseOutput) -> Vec<(i64, i64, String, String)> {
        fn walk(node: &Node, out: &mut Vec<(i64, i64, String, String)>) {
            if node.kind == kinds::SYSTEM_MESSAGE {
                let level = match node.get("level") {
                    Some(AttrValue::Int(n)) => *n,
                    _ => 0,
                };
                let line = match node.get("line") {
                    Some(AttrValue::Int(n)) => *n,
                    _ => 0,
                };
                let source = match node.get("source") {
                    Some(AttrValue::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let text = node
                    .children
                    .first()
                    .map(|p| p.astext())
                    .unwrap_or_default();
                out.push((level, line, source, text));
            }
            for child in &node.children {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        walk(&output.doctree.root, &mut out);
        out
    }

    // ---- row 4: node anatomy ----------------------------------------

    /// The PROBE 4 lines-emph-caption-name shape, byte for byte:
    /// container takes ids/names, caption parses inline markup,
    /// `hl_lines` renders 1-based before the unconditional linenostart,
    /// `language` only because it was given, NO linenos attr.
    #[test]
    fn caption_name_emphasize_anatomy_matches_the_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :language: python\n\
             \x20  :lines: 1,6-8\n\
             \x20  :emphasize-lines: 2,4\n\
             \x20  :caption: The *example* file\n\
             \x20  :name: lit-example\n",
        );
        let p = at(tmp.path(), "example.py");
        assert_eq!(
            output.doctree.root.children[0].pformat(),
            format!(
                "<container classes=\"literal-block-wrapper\" ids=\"lit-example\" \
                 literal_block=\"1\" names=\"lit-example\">\n\
                 \x20   <caption>\n\
                 \x20       The \n\
                 \x20       <emphasis>\n\
                 \x20           example\n\
                 \x20        file\n\
                 \x20   <literal_block force=\"0\" highlight_args=\"{{'hl_lines': [2, 4], \
                 'linenostart': 1}}\" language=\"python\" source=\"{p}\" \
                 xml:space=\"preserve\">\n\
                 \x20       \"\"\"Example module.\"\"\"\n\
                 \x20       def top(x):\n\
                 \x20           \"\"\"Top function.\"\"\"\n\
                 \x20           return x + 1\n"
            )
        );
        assert!(messages_of(&output).is_empty());
        assert!(output.registry.log_warnings().is_empty());
    }

    /// `linenos="1"` appears iff one of linenos/lineno-start/
    /// lineno-match is given (absent otherwise — probed); lineno-match
    /// with `:lines:` biases linenostart.
    #[test]
    fn linenos_appears_only_when_asked_and_lineno_match_biases_start() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :lines: 6-8\n\
             \x20  :lineno-match:\n",
        );
        let p = at(tmp.path(), "example.py");
        assert_eq!(
            output.doctree.root.children[0].pformat(),
            format!(
                "<literal_block force=\"0\" highlight_args=\"{{'linenostart': 6}}\" \
                 linenos=\"1\" source=\"{p}\" xml:space=\"preserve\">\n\
                 \x20   def top(x):\n\
                 \x20       \"\"\"Top function.\"\"\"\n\
                 \x20       return x + 1\n"
            )
        );
    }

    /// `:pyobject:` end to end (probe `pyobject_method_lineno_match`): the
    /// `Foo.method` tag slices lines 16-17 out of the fixture module and
    /// `lineno-match` renders `linenostart: 16`.
    #[test]
    fn pyobject_extracts_a_method_and_lineno_match_numbers_it() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :pyobject: Foo.method\n\
             \x20  :lineno-match:\n",
        );
        let p = at(tmp.path(), "example.py");
        assert_eq!(
            output.doctree.root.children[0].pformat(),
            format!(
                "<literal_block force=\"0\" highlight_args=\"{{'linenostart': 16}}\" \
                 linenos=\"1\" source=\"{p}\" xml:space=\"preserve\">\n\
                 \x20       def method(self):\n\
                 \x20           return self.attr\n"
            )
        );
        assert!(messages_of(&output).is_empty());
        assert!(output.registry.log_warnings().is_empty());
    }

    #[test]
    fn lineno_start_sets_linenos_and_linenostart() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :lines: 1-2\n\
             \x20  :lineno-start: 5\n",
        );
        let p = at(tmp.path(), "example.py");
        assert_eq!(
            output.doctree.root.children[0].pformat(),
            format!(
                "<literal_block force=\"0\" highlight_args=\"{{'linenostart': 5}}\" \
                 linenos=\"1\" source=\"{p}\" xml:space=\"preserve\">\n\
                 \x20   \"\"\"Example module.\"\"\"\n\
                 \x20   \n"
            )
        );
    }

    /// force/class land as attrs; absent `:language:` leaves the
    /// attribute unset entirely (NO highlight_language fallback), and
    /// the trailing blank line of the clip survives into the text.
    #[test]
    fn start_end_clip_anatomy_with_force_and_class() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :start-at: class Foo\n\
             \x20  :end-before: def method\n\
             \x20  :force:\n\
             \x20  :class: snippet\n",
        );
        let p = at(tmp.path(), "example.py");
        assert_eq!(
            output.doctree.root.children[0].pformat(),
            format!(
                "<literal_block classes=\"snippet\" force=\"1\" \
                 highlight_args=\"{{'linenostart': 1}}\" source=\"{p}\" \
                 xml:space=\"preserve\">\n\
                 \x20   class Foo:\n\
                 \x20       \"\"\"A class.\"\"\"\n\
                 \x20   \n\
                 \x20       attr = 2\n\
                 \x20   \n"
            )
        );
    }

    #[test]
    fn prepend_append_dedent_anatomy_matches_the_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :lines: 15-17\n\
             \x20  :dedent: 4\n\
             \x20  :prepend: # begin\n\
             \x20  :append: # end\n",
        );
        let p = at(tmp.path(), "example.py");
        assert_eq!(
            output.doctree.root.children[0].pformat(),
            format!(
                "<literal_block force=\"0\" highlight_args=\"{{'linenostart': 1}}\" \
                 source=\"{p}\" xml:space=\"preserve\">\n\
                 \x20   # begin\n\
                 \x20   \n\
                 \x20   def method(self):\n\
                 \x20       return self.attr\n\
                 \x20   # end\n"
            )
        );
        assert!(messages_of(&output).is_empty());
    }

    /// `:name:` without a caption lands on the literal_block itself.
    #[test]
    fn name_without_caption_lands_on_the_literal_block() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :lines: 3\n\
             \x20  :name: const-line\n",
        );
        let node = &output.doctree.root.children[0];
        assert_eq!(node.kind, kinds::LITERAL_BLOCK);
        assert_eq!(node.attrs.ids, vec!["const-line".to_string()]);
        assert_eq!(node.attrs.names, vec!["const-line".to_string()]);
    }

    // ---- row 5: captions --------------------------------------------

    /// The EMPTY `:caption:` falls back to the path as written, and the
    /// unnamed captioned container gets the AutoNumbering implicit
    /// `ids="id1"` with no name (probed) — from the read transform
    /// (`crate::transforms`, 210), not the parse, which leaves it id-less.
    #[test]
    fn empty_caption_falls_back_to_the_path_and_gets_id1() {
        let tmp = tempfile::tempdir().unwrap();
        let src = ".. literalinclude:: example.py\n\
                   \x20  :caption:\n\
                   \x20  :lines: 3\n";
        let p = at(tmp.path(), "example.py");
        let expected = |ids: &str| {
            format!(
                "<container classes=\"literal-block-wrapper\"{ids} \
                 literal_block=\"1\">\n\
                 \x20   <caption>\n\
                 \x20       example.py\n\
                 \x20   <literal_block force=\"0\" highlight_args=\"{{'linenostart': 1}}\" \
                 source=\"{p}\" xml:space=\"preserve\">\n\
                 \x20       CONST = 1\n"
            )
        };
        let parsed = parse(tmp.path(), src);
        assert_eq!(parsed.doctree.root.children[0].pformat(), expected(""));
        let transformed = parse_and_transform(tmp.path(), src);
        assert_eq!(
            transformed.doctree.root.children[0].pformat(),
            expected(" ids=\"id1\"")
        );
    }

    /// A caption whose parse leads with a system_message raises the
    /// `Invalid caption` ValueError into the reporter funnel — the
    /// message body renders through system_message.astext()'s
    /// `source:line: (TYPE/level)` prefix (probed bytes).
    #[test]
    fn an_invalid_caption_becomes_one_reporter_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :lines: 3\n\
             \x20  :caption: .. bogus::\n",
        );
        let p = at(tmp.path(), "main.rst");
        let msgs = messages_of(&output);
        assert_eq!(msgs.len(), 1, "{}", output.doctree.root.pformat());
        assert_eq!(msgs[0].0, 2);
        assert_eq!(msgs[0].1, 1);
        assert_eq!(
            msgs[0].3,
            format!(
                "Invalid caption: {p}:1: (INFO/1) No directive entry for \
                 \"bogus\" in module \"docutils.parsers.rst.languages.en\".\n\
                 Trying \"bogus\" as canonical directive name."
            )
        );
    }

    /// The caption is a throwaway nested parse at the directive's content
    /// offset (`container_wrapper`, `code.py:78-96`): the unknown-directive
    /// ERROR it creates is printed — at the offset past the option block,
    /// line 5 here — although only the `Invalid caption` warning reaches the
    /// tree (probed against sphinx 9.1.0: `main.rst:5: ERROR: Unknown
    /// directive type "bogus".` then `main.rst:1: WARNING: Invalid caption:
    /// ...`).
    #[test]
    fn a_caption_throwaway_parse_prints_at_the_content_offset() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :lines: 3\n\
             \x20  :caption: .. bogus::\n",
        );
        let records: Vec<(u8, Option<u32>, &str)> = output
            .registry
            .diagnostics
            .iter()
            .map(|d| (d.level, d.line, d.text.as_str()))
            .collect();
        assert_eq!(records.len(), 2, "{records:#?}");
        assert_eq!(
            records[0],
            (
                3,
                Some(5),
                "Unknown directive type \"bogus\".\n\n.. bogus::"
            )
        );
        assert_eq!((records[1].0, records[1].1), (2, Some(1)));
        assert!(records[1].2.starts_with("Invalid caption: "));
    }

    // ---- row 6: diff mode -------------------------------------------

    /// Diff mode: `--- old` / `+++ new` headers with no timestamps,
    /// `language="udiff"`, every other filter skipped; the diff file is
    /// NOT a dependency, the main file IS.
    #[test]
    fn diff_mode_anatomy_and_dependency_scope() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "example_old.py", EXAMPLE_OLD_PY);
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :diff: example_old.py\n",
        );
        let old = resolved_at(tmp.path(), "example_old.py");
        let new = resolved_at(tmp.path(), "example.py");
        let node = &output.doctree.root.children[0];
        assert_eq!(node.kind, kinds::LITERAL_BLOCK);
        assert_eq!(
            node.get("language"),
            Some(&AttrValue::Str("udiff".to_string()))
        );
        assert_eq!(
            node.get("source"),
            Some(&AttrValue::Str(at(tmp.path(), "example.py")))
        );
        let text = node.children[0].astext();
        assert!(
            text.starts_with(&format!("--- {old}\n+++ {new}\n@@ -1,7 +1,21 @@\n")),
            "{text}"
        );
        assert_eq!(output.registry.dependencies, vec!["example.py".to_string()]);
    }

    // ---- row 3: the warning-channel split ---------------------------

    /// The three logger-channel warnings are logger records with the
    /// doc2path-doubled RENDERED location and never enter the tree
    /// ([INC §3.4]).
    #[test]
    fn the_three_logger_warnings_render_the_doubled_suffix_location() {
        let tmp = tempfile::tempdir().unwrap();
        let p = at(tmp.path(), "main.rst");
        for (main, line, message) in [
            (
                ".. literalinclude:: example.py\n\x20  :lines: 1,99\n".to_string(),
                1i64,
                "line number spec is out of range(1-21): '1,99'".to_string(),
            ),
            (
                "para\n\n.. literalinclude:: example.py\n\
                 \x20  :lines: 1-3\n\x20  :emphasize-lines: 2,9\n"
                    .to_string(),
                3,
                "line number spec is out of range(1-3): '2,9'".to_string(),
            ),
            (
                ".. literalinclude:: example.py\n\
                 \x20  :lines: 1\n\x20  :dedent: 2\n"
                    .to_string(),
                1,
                "non-whitespace stripped by dedent".to_string(),
            ),
        ] {
            let output = parse(tmp.path(), &main);
            assert!(
                messages_of(&output).is_empty(),
                "logger-channel warnings never enter the tree: {}",
                output.doctree.root.pformat()
            );
            let warnings = output.registry.log_warnings();
            assert_eq!(warnings.len(), 1, "{main}");
            assert_eq!(warnings[0].text, message);
            assert_eq!(warnings[0].line.map(i64::from), Some(line));
            assert!(warnings[0].doc2path_location);
            let table_path = &output.doctree.sources[warnings[0].source as usize];
            // The RENDERED location string — the doc2path append doubles
            // the suffix exactly as sphinx renders it.
            assert_eq!(warnings[0].rendered_path(table_path), format!("{p}.rst"));
        }
    }

    /// A literalinclude inside an INCLUDED file attributes the logger
    /// warning to the included file's own provenance (its srcdir-relative
    /// display spelling, §Scope-8), still with the doubled suffix.
    #[test]
    fn logger_location_inside_an_included_file_is_the_included_source() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "part.rst",
            "part para\n\n.. literalinclude:: example.py\n\x20  :lines: 1,99\n",
        );
        let output = parse(tmp.path(), ".. include:: part.rst\n");
        let warnings = output.registry.log_warnings();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].line, Some(3));
        let table_path = &output.doctree.sources[warnings[0].source as usize];
        assert_eq!(warnings[0].rendered_path(table_path), "part.rst.rst");
    }

    /// `:lines: 99` produces BOTH channels: the logger out-of-range
    /// warning AND the reporter no-lines-pulled warning with the
    /// `_StrPath` repr bytes (probed pair).
    #[test]
    fn lines_99_warns_on_the_logger_channel_and_errs_on_the_reporter() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            "para\n\n.. literalinclude:: example.py\n\x20  :lines: 99\n",
        );
        let main = at(tmp.path(), "main.rst");
        let example = py_repr(Some(&resolved_at(tmp.path(), "example.py")));
        assert_eq!(output.registry.log_warnings().len(), 1);
        assert_eq!(
            output.registry.log_warnings()[0].text,
            "line number spec is out of range(1-21): '99'"
        );
        let msgs = messages_of(&output);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0],
            (
                2,
                3,
                main,
                format!(
                    "Line spec '99': no lines pulled from include file \
                     _StrPath({example})"
                )
            )
        );
    }

    /// The three `:emphasize-lines:` / negative-index edges task 13 probed
    /// but left unpinned, re-probed against sphinx 9.1.0 alongside the
    /// env-fixture `inc_*` projects (which pin the middle one end to end,
    /// as `inc_warn`'s second `literal_block`).
    ///
    /// `parselinenos` maps a spec entry `n` to `n - 1`, so `0` becomes the
    /// Python index `-1`. For `:emphasize-lines:` that survives the
    /// `+1` round trip as a literal `0` with no out-of-range warning
    /// (`hl_lines` is validated against `>= lines`, and `0` is not); for
    /// `:lines:` it selects the LAST line, which on an EMPTY file is
    /// Python's own `IndexError` funnelled into the reader's single
    /// reporter warning.
    #[test]
    fn the_negative_index_edges_of_linenos_specs_match_the_probe() {
        let tmp = tempfile::tempdir().unwrap();
        let p = at(tmp.path(), "example.py");
        let main = at(tmp.path(), "main.rst");

        // `:emphasize-lines: 0` -> hl_lines [0], silently.
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\x20  :lines: 1-3\n\x20  :emphasize-lines: 0\n",
        );
        assert_eq!(
            output.doctree.root.children[0].pformat(),
            format!(
                "<literal_block force=\"0\" highlight_args=\"{{'hl_lines': [0], \
                 'linenostart': 1}}\" source=\"{p}\" xml:space=\"preserve\">\n\
                 \x20   \"\"\"Example module.\"\"\"\n\
                 \x20   \n\
                 \x20   CONST = 1\n"
            )
        );
        assert!(output.registry.log_warnings().is_empty());
        assert!(messages_of(&output).is_empty());

        // Every emphasized line out of range -> the out-of-range warning
        // and an EMPTY hl_lines list, still rendered.
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\x20  :lines: 1-3\n\x20  :emphasize-lines: 9\n",
        );
        assert!(output.doctree.root.children[0]
            .pformat()
            .starts_with(&format!(
                "<literal_block force=\"0\" highlight_args=\"{{'hl_lines': [], \
                 'linenostart': 1}}\" source=\"{p}\""
            )));
        assert_eq!(output.registry.log_warnings().len(), 1);
        assert_eq!(
            output.registry.log_warnings()[0].text,
            "line number spec is out of range(1-3): '9'"
        );

        // `:lines: 0` against an empty file indexes `[-1]` of nothing.
        write(tmp.path(), "empty.py", "");
        let output = parse(
            tmp.path(),
            "para\n\n.. literalinclude:: empty.py\n\x20  :lines: 0\n",
        );
        assert!(output.registry.log_warnings().is_empty());
        assert_eq!(
            messages_of(&output),
            vec![(2, 3, main, "list index out of range".to_string())]
        );
    }

    // ---- row 7: the error funnel ------------------------------------

    /// Every reader error is ONE reporter warning at the directive line
    /// with the error text as the message — no literal rawsource child
    /// (`reporter.warning(exc, line=...)`, not a directive error).
    /// `LiteralInclude.run`'s `except Exception` (`code.py:505`) turns
    /// the same OverflowError into one directive warning — here sphinx and
    /// the port agree exactly.
    ///
    // oracle (scratchpad A/tw.py): `.. literalinclude:: t.txt` +
    // `:tab-width: 2147483648` -> "index.rst:1: WARNING: Python int too
    // large to convert to C int [docutils]".
    #[test]
    fn a_huge_tab_width_becomes_the_readers_overflow_warning() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("t.txt"), "a\tb\n").unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: t.txt\n   :tab-width: 2147483648\n",
        );
        let msgs = messages_of(&output);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert_eq!(msgs[0].3, "Python int too large to convert to C int");
    }

    #[test]
    fn reader_errors_funnel_into_one_reporter_warning() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("bad.bin"), b"caf\xe9\n").unwrap();
        std::fs::write(
            tmp.path().join("broken2.py"),
            "x = \"\"\"abc\ndef f():\n    pass\n",
        )
        .unwrap();
        let file = |rel: &str| resolved_at(tmp.path(), rel);
        for (main, line, message) in [
            (
                "c\n\n.. literalinclude:: nothere.py\n".to_string(),
                3i64,
                format!(
                    "Include file '{}' not found or reading it failed",
                    file("nothere.py")
                ),
            ),
            (
                ".. literalinclude:: example.py\n\
                 \x20  :start-after: x\n\x20  :start-at: y\n"
                    .to_string(),
                1,
                "Cannot use both \"start-after\" and \"start-at\" options".to_string(),
            ),
            (
                ".. literalinclude:: bad.bin\n".to_string(),
                1,
                format!(
                    "Encoding 'utf-8-sig' used for reading included file '{}' \
                     seems to be wrong, try giving an :encoding: option",
                    file("bad.bin")
                ),
            ),
            (
                ".. literalinclude:: example.py\n\
                 \x20  :lines: 1,6\n\x20  :lineno-match:\n"
                    .to_string(),
                1,
                "Cannot use \"lineno-match\" with a disjoint set of \"lines\"".to_string(),
            ),
            (
                ".. literalinclude:: example.py\n\x20  :start-after: NOPE\n".to_string(),
                1,
                "start-after pattern not found: NOPE".to_string(),
            ),
            (
                ".. literalinclude:: example.py\n\x20  :lines: 5-3\n".to_string(),
                1,
                "invalid line number spec: '5-3'".to_string(),
            ),
            (
                // An invalid emphasize spec replaces the whole node.
                ".. literalinclude:: example.py\n\
                 \x20  :lines: 1-3\n\x20  :emphasize-lines: 5-3\n"
                    .to_string(),
                1,
                "invalid line number spec: '5-3'".to_string(),
            ),
            (
                // probe `pyobject_missing`.
                ".. literalinclude:: example.py\n\x20  :pyobject: Nope\n".to_string(),
                1,
                format!(
                    "Object named 'Nope' not found in include file _StrPath({})",
                    py_repr(Some(&file("example.py")))
                ),
            ),
            (
                // The analyzer-failure funnel: sphinx's prefix, our detail
                // (probe `pyobject_broken_triple`; see `py::pycode`).
                ".. literalinclude:: broken2.py\n\x20  :pyobject: f\n".to_string(),
                1,
                format!(
                    "parsing {} failed: unterminated triple-quoted \
                     string literal (detected at line 3)",
                    py_repr(Some(&file("broken2.py")))
                ),
            ),
        ] {
            let output = parse(tmp.path(), &main);
            let msgs = messages_of(&output);
            assert_eq!(msgs.len(), 1, "{main}:\n{}", output.doctree.root.pformat());
            assert_eq!(msgs[0].0, 2, "{main}");
            assert_eq!(msgs[0].1, line, "{main}");
            assert_eq!(msgs[0].3, message, "{main}");
            // No literal_block node replaced the failed directive.
            assert!(
                output
                    .doctree
                    .root
                    .children
                    .iter()
                    .all(|n| n.kind != kinds::LITERAL_BLOCK),
                "{main}"
            );
            // The reporter message carries no rawsource literal child.
            let sm = output
                .doctree
                .root
                .children
                .iter()
                .find(|n| n.kind == kinds::SYSTEM_MESSAGE)
                .unwrap();
            assert_eq!(sm.children.len(), 1, "{main}");
        }
    }

    /// The docutils option-conversion layer still owns converter errors:
    /// a negative `:dedent:` is a directive ERROR with the optional_int
    /// text (probed), not a reader warning.
    #[test]
    fn negative_dedent_is_a_directive_option_error() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\x20  :dedent: -2\n",
        );
        let msgs = messages_of(&output);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, 3);
        assert_eq!(
            msgs[0].3,
            "Error in \"literalinclude\" directive:\ninvalid option value: \
             (option: \"dedent\"; value: '-2')\nnegative value; must be positive or zero."
        );
        // The directive never ran: no dependency was recorded.
        assert!(output.registry.dependencies.is_empty());
    }

    // ---- row 8: the dependency record -------------------------------

    /// `env.note_dependency(rel_filename)` is recorded BEFORE reading:
    /// a missing file and an option-conflict error both still record.
    #[test]
    fn dependency_records_precede_the_read_even_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let output = parse(tmp.path(), ".. literalinclude:: nothere.py\n");
        assert_eq!(output.registry.dependencies, vec!["nothere.py".to_string()]);

        let output = parse(
            tmp.path(),
            ".. literalinclude:: example.py\n\
             \x20  :start-after: x\n\x20  :start-at: y\n",
        );
        assert_eq!(output.registry.dependencies, vec!["example.py".to_string()]);
    }

    /// Docname-relative resolution: a document in a subdirectory
    /// resolves the argument against its own directory (env.relfn2path).
    #[test]
    fn the_argument_resolves_docname_relative() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        write(tmp.path(), "sub/data.py", "x = 1\n");
        write(tmp.path(), "example.py", EXAMPLE_PY);
        let output = parse_rst_full(
            ".. literalinclude:: data.py\n",
            &ParseOptions {
                source_path: at(tmp.path(), "sub/page.rst"),
                sphinx: true,
                docname: "sub/page".to_string(),
                srcdir: Some(tmp.path().to_path_buf()),
                ..Default::default()
            },
        );
        assert_eq!(
            output.registry.dependencies,
            vec!["sub/data.py".to_string()]
        );
        let node = &output.doctree.root.children[0];
        assert_eq!(
            node.get("source"),
            Some(&AttrValue::Str(at(tmp.path(), "sub/data.py")))
        );
        assert_eq!(node.children[0].astext(), "x = 1\n");
    }
}
