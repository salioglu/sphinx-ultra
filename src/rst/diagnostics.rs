//! The one record every read-phase diagnostic travels as.
//!
//! Sphinx prints two kinds of message while it reads a document, and both
//! end up as a line on the warning stream:
//!
//! * the **reporter channel** — docutils `Reporter.system_message` writes
//!   `msg.astext() + '\n'` to its stream *at creation*, for every message at
//!   or above `report_level` (`docutils/utils/__init__.py:213-215`), and
//!   Sphinx's `WarningStream` (`sphinx/util/docutils.py:385-393`) re-logs it
//!   as `logger.log(LEVEL, message, location='source:line', type='docutils')`;
//! * the **logger channel** — a directive or domain calling
//!   `logger.warning(..., location=..., type=..., subtype=...)` directly
//!   (toctree, duplicate object descriptions, the `literalinclude` reader).
//!
//! The two interleave in creation order, so they share one record and one
//! `seq` counter; [`crate::error::BuildWarning::from_diagnostic`] turns a
//! record into the line `sphinx-build` prints.
//!
//! The record is persisted inside `RegistryExport` (the document cache), so
//! no field carries `#[serde(default)]`: a stale cache entry written before a
//! field existed must miss and re-parse, not decode with the field silently
//! zeroed (see `RegistryExport::program_options`).

use std::cell::{Cell, RefCell};

use serde::{Deserialize, Serialize};

use crate::doctree::{AttrValue, Node};

/// Which Sphinx pipeline a record came from; it decides the `[category]`
/// suffix (see [`Diagnostic::category`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticChannel {
    /// A docutils `system_message` printed at creation. Always logged with
    /// `type='docutils'` (`sphinx/util/docutils.py:393`).
    Reporter,
    /// A direct `logger.warning(...)` call; carries its own `type.subtype`
    /// category, or none.
    Logger,
}

/// One message the read phase prints, in the shape the printer needs and
/// nothing more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Creation order within the document, shared by both channels so the
    /// merge phase can interleave them exactly as Sphinx's buffered
    /// `pending_warnings` flush does (`sphinx/builders/__init__.py:403-409`).
    pub seq: u32,
    pub channel: DiagnosticChannel,
    /// The docutils level (`2` WARNING, `3` ERROR, `4` SEVERE). Levels `0`
    /// and `1` never reach the warning stream (below `report_level` 2), so
    /// they are never recorded; `5` only arises as a `halt_level`.
    pub level: u8,
    /// The logger's `type.subtype` (`toc.not_readable`, ...), shown as
    /// ` [category]`. A [`DiagnosticChannel::Reporter`] record's category is
    /// always `docutils`, which the renderer supplies, so producers leave this
    /// `None` for that channel.
    pub category: Option<String>,
    /// The message exactly as Sphinx prints it: `Element.astext()` of the
    /// `system_message` *at creation* (children joined by a blank line), with
    /// trailing whitespace stripped (`sphinx/util/docutils.py:391`). A
    /// `DirectiveError` message is created before its literal block is
    /// appended (`docutils/parsers/rst/states.py:2286-2290`), so its printed
    /// text lacks the literal although the tree node has it.
    pub text: String,
    /// Index into the document's source table: the file whose line `line`
    /// counts (the document itself, or an `include`d file).
    pub source: u16,
    /// 1-based line in that source, or `None` when docutils had none
    /// (`Anonymous hyperlink mismatch`); the reporter then prints
    /// `source:: LEVEL: ...` with a doubled colon.
    pub line: Option<u32>,
    /// Sphinx passes some `location=` tuples whose first item is a path, not
    /// a docname, and `doc2path` then appends the project's first source
    /// suffix (`sphinx/util/logging.py:507-512`), doubling it — the
    /// `literalinclude` reader warnings print `<srcdir>/a.rst.rst`
    /// ([`Self::rendered_path`]).
    ///
    /// This flag only records that quirk. `BuildWarning::from_diagnostic`
    /// ignores it: the caller that resolves `source` to a path (the merge
    /// phase) applies the suffix to that path and passes the result as
    /// `source_path`.
    pub doc2path_location: bool,
}

impl Diagnostic {
    /// The path the printed location spells for this record, given its
    /// source-table path.
    ///
    /// WHY the doubled suffix: a Sphinx `logger.warning(...,
    /// location=(source, line))` tuple is treated by the log translator as
    /// `(docname, lineno)` and rendered `f'{env.doc2path(docname)}:{lineno}'`
    /// (`SP/util/logging.py:507-512`); `doc2path` on a string that is not a
    /// known docname appends the project's first source suffix
    /// (`SP/project.py:114-128`). The literalinclude reader warnings and
    /// `CodeBlock`'s out-of-range `:emphasize-lines:` warning pass a full
    /// path like `<srcdir>/a.rst` as the tuple's `source`, so Sphinx renders
    /// the doubled `<srcdir>/a.rst.rst` — byte-exact oracle behavior
    /// (probed, [INC §3.4]). The appended suffix is the crate's first
    /// source suffix (`.rst`, matching sphinx's default `source_suffix[0]`
    /// and this crate's discovery order).
    pub fn rendered_path(&self, source_path: &str) -> String {
        if self.doc2path_location {
            format!("{source_path}.rst")
        } else {
            source_path.to_string()
        }
    }
}

/// docutils' default `report_level` (`docutils/frontend.py:769-773`), which
/// Sphinx leaves in place: a message below it is never written to the
/// stream, so it never prints.
const REPORT_LEVEL: u8 = 2;

/// One parse's diagnostics stream: the recorder every message-creation site
/// reports to, the moment it creates the message.
///
/// docutils' `Reporter.system_message` writes a message to the warning
/// stream when it creates it (`docutils/utils/__init__.py:213-215`), before
/// the caller does anything with the node: a `DirectiveError` message is
/// written and only then given its literal block
/// (`docutils/parsers/rst/states.py:2286-2290`), and a message created in a
/// nested parse whose nodes are thrown away is written all the same. So the
/// parser reports each message as it builds it, with the text it has then,
/// and the stream is exactly docutils' — nothing is reconstructed from the
/// finished tree.
///
/// The logger channel shares the same counter: [`Self::log`] records a
/// directive's `logger.warning`, and [`Self::next_seq`] hands a number to a
/// registration whose duplicate warning is only known once the environment
/// replays it, so the merge phase can put that warning where Sphinx logged
/// it.
///
/// Every method takes `&self` (the state is in cells): message creation
/// happens in `&self` helpers throughout the block parser.
#[derive(Debug, Default)]
pub struct Reporter {
    next: Cell<u32>,
    records: RefCell<Vec<Diagnostic>>,
}

impl Reporter {
    /// A recorder that carries on a stream another one started: its first
    /// record (or registration) takes `next`.
    ///
    /// The read-transform pass records into one of these, seeded with the
    /// parse's [`Self::peek_seq`]: Sphinx runs its read transforms after
    /// the parse, so every record a transform makes follows every record
    /// the parse made — and every registration whose duplicate warning the
    /// parse logged — in the document's one stream.
    pub fn continuing_from(next: u32) -> Reporter {
        Reporter {
            next: Cell::new(next),
            records: RefCell::default(),
        }
    }

    /// The number the next record or registration will take, unspent.
    pub fn peek_seq(&self) -> u32 {
        self.next.get()
    }

    /// The next number in creation order, spent.
    pub fn next_seq(&self) -> u32 {
        let seq = self.next.get();
        self.next.set(seq + 1);
        seq
    }

    /// Record the `system_message` `msg` as docutils writes it at creation:
    /// its `level`, `line` and source ([`Node::span`]'s `source` — the
    /// source-table index of the path its `source` attribute names), and
    /// its `Element.astext()` *now* — children joined by a blank line
    /// (`docutils/nodes.py`, `Element.child_text_separator`), trailing
    /// whitespace stripped the way `WarningStream` `rstrip()`s it
    /// (`sphinx/util/docutils.py:391`). Below level 2 nothing is recorded.
    pub fn report(&self, msg: &Node) {
        let level = match msg.get("level") {
            Some(AttrValue::Int(level)) => u8::try_from(*level).unwrap_or(u8::MAX),
            _ => return,
        };
        if level < REPORT_LEVEL {
            return;
        }
        let line = match msg.get("line") {
            Some(AttrValue::Int(line)) => u32::try_from(*line).ok(),
            _ => None,
        };
        let text = msg
            .children
            .iter()
            .map(Node::astext)
            .collect::<Vec<_>>()
            .join("\n\n");
        let text = text.trim_end_matches(crate::utils::py_isspace).to_string();
        self.push(
            DiagnosticChannel::Reporter,
            level,
            None,
            text,
            msg.span.source,
            line,
            false,
        );
    }

    /// Record a directive's or domain's `logger.log(level, text, type=...,
    /// location=...)` at the point Sphinx makes the call.
    pub fn log(
        &self,
        level: u8,
        category: Option<String>,
        text: String,
        source: u16,
        line: Option<u32>,
        doc2path_location: bool,
    ) {
        self.push(
            DiagnosticChannel::Logger,
            level,
            category,
            text,
            source,
            line,
            doc2path_location,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &self,
        channel: DiagnosticChannel,
        level: u8,
        category: Option<String>,
        text: String,
        source: u16,
        line: Option<u32>,
        doc2path_location: bool,
    ) {
        let seq = self.next_seq();
        self.records.borrow_mut().push(Diagnostic {
            seq,
            channel,
            level,
            category,
            text,
            source,
            line,
            doc2path_location,
        });
    }

    /// The recorded stream, in `seq` order (records are appended as their
    /// numbers are handed out).
    pub fn take(self) -> Vec<Diagnostic> {
        self.records.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Diagnostic {
        Diagnostic {
            seq: 7,
            channel: DiagnosticChannel::Reporter,
            level: 3,
            category: None,
            text: "Unknown directive type \"x\".\n\n.. x::".to_string(),
            source: 2,
            line: Some(12),
            doc2path_location: false,
        }
    }

    #[test]
    fn a_diagnostic_survives_the_document_cache_round_trip() {
        for d in [
            sample(),
            Diagnostic {
                channel: DiagnosticChannel::Logger,
                level: 2,
                category: Some("toc.not_readable".to_string()),
                line: None,
                doc2path_location: true,
                ..sample()
            },
        ] {
            let json = serde_json::to_string(&d).unwrap();
            assert_eq!(serde_json::from_str::<Diagnostic>(&json).unwrap(), d);
        }
    }

    fn parse(source: &str) -> crate::rst::ParseOutput {
        crate::rst::parse_rst_full(
            source,
            &crate::rst::ParseOptions {
                source_path: "<snippet>".to_string(),
                ..Default::default()
            },
        )
    }

    /// docutils' `assert_has_content` raises a `DirectiveError`; the
    /// directive machinery creates the message from it — which writes it —
    /// and only then appends the literal block
    /// (`docutils/parsers/rst/states.py:2286-2290`). The printed record
    /// lacks the literal the tree message carries (probed:
    /// `<snippet>:1: ERROR: Content block expected for the "note" directive;
    /// none found. [docutils]`).
    #[test]
    fn a_directive_error_prints_without_its_literal() {
        let out = parse(".. note::\n");
        let records = &out.registry.diagnostics;
        assert_eq!(records.len(), 1, "{records:#?}");
        let record = &records[0];
        assert_eq!(
            record.text,
            "Content block expected for the \"note\" directive; none found."
        );
        assert_eq!(
            (record.channel, record.level, record.line, record.source),
            (DiagnosticChannel::Reporter, 3, Some(1), 0)
        );
        let tree = out.doctree.root.pformat();
        assert!(
            tree.contains("<literal_block xml:space=\"preserve\">\n            .. note::"),
            "the tree message keeps the literal: {tree}"
        );
    }

    /// The option-parse failure is a `MarkupError` the directive machinery
    /// turns into a message WITH the literal passed at creation
    /// (`states.py:2276-2281`), so the printed record carries it (probed).
    #[test]
    fn an_option_error_prints_its_literal() {
        let out = parse(".. note::\n   :bogus: x\n\n   Body.\n");
        let texts: Vec<&str> = out
            .registry
            .diagnostics
            .iter()
            .map(|d| d.text.as_str())
            .collect();
        assert_eq!(
            texts,
            [
                "Error in \"note\" directive:\nunknown option: \"bogus\".\n\n\
              .. note::\n   :bogus: x\n\n   Body."
            ]
        );
    }

    /// Reporter and logger records share one counter, handed out as each
    /// is created, and so do the registrations whose environment replay
    /// warns (`PythonDomain.note_object` runs in `add_target_and_index`,
    /// after `handle_signature` logged the arglist warning).
    #[test]
    fn records_are_numbered_in_creation_order() {
        let out = crate::rst::parse_rst_full(
            "Title\n====\n\n.. toctree::\n\n   missing\n\n\
             .. py:function:: f(a, a)\n\n*emph\n",
            &crate::rst::ParseOptions {
                source_path: "<snippet>".to_string(),
                sphinx: true,
                found_docs: Some(std::sync::Arc::new(
                    ["index".to_string()].into_iter().collect(),
                )),
                ..Default::default()
            },
        );
        let records: Vec<(u32, DiagnosticChannel, Option<u32>, &str)> = out
            .registry
            .diagnostics
            .iter()
            .map(|d| (d.seq, d.channel, d.line, d.text.as_str()))
            .collect();
        assert_eq!(
            records,
            [
                (
                    0,
                    DiagnosticChannel::Reporter,
                    Some(2),
                    "Title underline too short.\n\nTitle\n===="
                ),
                (
                    1,
                    DiagnosticChannel::Logger,
                    Some(4),
                    "toctree contains reference to nonexisting document 'missing'"
                ),
                (
                    2,
                    DiagnosticChannel::Logger,
                    Some(8),
                    "could not parse arglist ('a, a'): duplicate parameter name: 'a'"
                ),
                (
                    4,
                    DiagnosticChannel::Reporter,
                    Some(10),
                    "Inline emphasis start-string without end-string."
                ),
            ]
        );
        let registrations: Vec<(&str, u32)> = out
            .registry
            .py_objects
            .iter()
            .map(|r| (r.fullname.as_str(), r.seq))
            .collect();
        assert_eq!(registrations, [("f", 3)]);
    }

    /// Only what reaches Sphinx's stream is recorded: docutils'
    /// `report_level` stays at its default 2, so INFO never prints.
    #[test]
    fn info_messages_are_never_recorded() {
        let reporter = Reporter::default();
        reporter.report(&crate::doctree::messages::system_message(
            crate::doctree::messages::INFO,
            "Duplicate implicit target name: \"a\".",
            0,
            3,
            "<snippet>",
        ));
        assert_eq!(reporter.next_seq(), 0, "no seq was spent either");
        assert!(reporter.take().is_empty());
    }

    /// A record keeps the text its message had when it was reported; a
    /// child appended afterwards (the `DirectiveError` literal) is not in
    /// it. Trailing whitespace goes, the way `WarningStream` `rstrip()`s.
    #[test]
    fn a_record_keeps_the_text_it_had_when_reported() {
        let reporter = Reporter::default();
        let msg = crate::doctree::messages::system_message(
            crate::doctree::messages::SEVERE,
            "Problems with \"include\" directive path:\nInputError: gone.  \n",
            2,
            7,
            "inc.rst",
        );
        reporter.report(&msg);
        let _tree_message = crate::doctree::messages::with_literal(msg, ".. include:: x");
        reporter.log(
            2,
            Some("toc.not_readable".to_string()),
            "toctree contains reference to nonexisting document 'x'".to_string(),
            0,
            Some(9),
            false,
        );
        assert_eq!(
            reporter.take(),
            [
                Diagnostic {
                    seq: 0,
                    channel: DiagnosticChannel::Reporter,
                    level: 4,
                    category: None,
                    text: "Problems with \"include\" directive path:\nInputError: gone."
                        .to_string(),
                    source: 2,
                    line: Some(7),
                    doc2path_location: false,
                },
                Diagnostic {
                    seq: 1,
                    channel: DiagnosticChannel::Logger,
                    level: 2,
                    category: Some("toc.not_readable".to_string()),
                    text: "toctree contains reference to nonexisting document 'x'".to_string(),
                    source: 0,
                    line: Some(9),
                    doc2path_location: false,
                },
            ]
        );
    }

    #[test]
    fn a_record_missing_a_field_does_not_decode() {
        // The cache-shape rule: no `#[serde(default)]`, so a stale entry
        // misses rather than decoding with `doc2path_location` off.
        let mut value = serde_json::to_value(sample()).unwrap();
        value.as_object_mut().unwrap().remove("doc2path_location");
        assert!(serde_json::from_value::<Diagnostic>(value).is_err());
    }
}
