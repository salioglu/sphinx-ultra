//! The build's warning stream, in Sphinx's record grammar.
//!
//! Every record the build prints is a [`Diagnostic`], rendered by
//! [`Diagnostic::render`] exactly as `sphinx-build` writes a logged record to
//! its warning stream (`sphinx/util/logging.py:99-125`, `:531-551`):
//!
//! ```text
//! [<location>: ]<PREFIX><message>[ [<type>[.<subtype>]]]
//! ```
//!
//! Records are collected per phase in a [`DiagnosticSink`] and leave it in
//! [`OrderKey`] order; the builder flushes the sinks in Sphinx's emission
//! order and drops what [`WarningFilter`] suppresses before anything is
//! printed or counted. The `src/env` producers keep returning
//! [`BuildWarning`]s, which convert into records at the sink boundary
//! (`From<BuildWarning>`).

use std::borrow::Cow;
use std::path::Path;

use crate::config::BuildConfig;
use crate::error::{BuildErrorReport, BuildWarning};

/// A record's level, as Sphinx names it in the record's prefix
/// (`SphinxWarningLogRecord.prefix`, `sphinx/util/logging.py:116-125`).
///
/// Docutils' reporter levels map 2 → `Warning`, 3 → `Error` and
/// 4 (`SEVERE`) → `Critical` (`LEVEL_NAMES`, `:30-41`); INFO and DEBUG never
/// reach the warning stream, so they have no variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Warning,
    Error,
    Critical,
}

impl Severity {
    /// The prefix the record prints after its location.
    fn prefix(self) -> &'static str {
        match self {
            Severity::Warning => "WARNING: ",
            Severity::Error => "ERROR: ",
            Severity::Critical => "CRITICAL: ",
        }
    }
}

/// Where a record points, spelled the way it prints.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    /// No location: the record prints bare, `WARNING: …`.
    None,
    /// A location string printed as it is: a docutils reporter's
    /// `source:line`, or `source:` for a message with no line, which then
    /// prints `source:: ERROR: …`.
    Verbatim(String),
    /// A logger location, already resolved to the path Sphinx prints
    /// (`get_node_location`'s `abspath`, `doc2path`), with `:line` when there
    /// is one.
    Path { path: String, line: Option<u32> },
}

impl Location {
    /// The text before the record's `: ` separator.
    fn spelling(&self) -> Option<Cow<'_, str>> {
        match self {
            Location::None => None,
            Location::Verbatim(location) => Some(Cow::Borrowed(location)),
            Location::Path { path, line: None } => Some(Cow::Borrowed(path)),
            Location::Path {
                path,
                line: Some(line),
            } => Some(Cow::Owned(format!("{path}:{line}"))),
        }
    }
}

/// Sphinx's warning `type` and `subtype` (`SphinxLoggerAdapter`), which
/// `suppress_warnings` matches and `show_warning_types` prints as the
/// record's suffix.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    /// No `type`: no suffix, and never suppressed.
    None,
    /// A `type` alone, ` [type]`.
    Type(String),
    /// A `type` and a `subtype`, ` [type.subtype]`.
    TypeSub(String, String),
}

impl Category {
    /// The category of a record logged with `type=ty, subtype=subtype`.
    ///
    /// Empty strings count as absent, Python's truthiness, and a `subtype`
    /// without a `type` is no category at all: `WarningLogRecordTranslator`
    /// prints no suffix for it (`sphinx/util/logging.py:545-549`), as for
    /// the toctree `empty_glob` warning.
    pub fn from_parts(ty: Option<&str>, subtype: Option<&str>) -> Category {
        fn present(part: Option<&str>) -> Option<&str> {
            part.filter(|part| !part.is_empty())
        }
        match (present(ty), present(subtype)) {
            (None, _) => Category::None,
            (Some(ty), None) => Category::Type(ty.to_string()),
            (Some(ty), Some(subtype)) => Category::TypeSub(ty.to_string(), subtype.to_string()),
        }
    }

    /// The `type.subtype` spelling a [`BuildWarning::category`] (or a
    /// toctree warning's category) carries: split on the first `.`.
    pub(crate) fn parse(spelled: &str) -> Category {
        match spelled.split_once('.') {
            Some((ty, subtype)) => Category::from_parts(Some(ty), Some(subtype)),
            None => Category::from_parts(Some(spelled), None),
        }
    }
}

/// One record of the warning stream.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub location: Location,
    pub severity: Severity,
    pub message: String,
    pub category: Category,
}

impl Diagnostic {
    /// The record as `sphinx-build` prints it: one record, possibly several
    /// lines, with no trailing newline.
    ///
    /// The ` [type.subtype]` suffix follows the message's last line and only
    /// when `show_warning_types` is set (`WarningLogRecordTranslator`,
    /// `sphinx/util/logging.py:531-551`). The location and prefix then follow
    /// `SphinxLogRecord.getMessage` (`:99-108`), including its quirk: a
    /// record without a location only gets its prefix when the message does
    /// not already contain it.
    pub fn render(&self, show_warning_types: bool) -> String {
        let mut message = self.message.clone();
        if show_warning_types {
            match &self.category {
                Category::None => {}
                Category::Type(ty) => message.push_str(&format!(" [{ty}]")),
                Category::TypeSub(ty, subtype) => {
                    message.push_str(&format!(" [{ty}.{subtype}]"));
                }
            }
        }
        let prefix = self.severity.prefix();
        match self.location.spelling() {
            Some(location) if !location.is_empty() => format!("{location}: {prefix}{message}"),
            _ if message.contains(prefix) => message,
            _ => format!("{prefix}{message}"),
        }
    }
}

/// Builds a record's location from a producer's `(file, line)`: an empty
/// file is a record logged with no location at all, which prints bare.
fn location_of(file: &Path, line: Option<usize>) -> Location {
    if file.as_os_str().is_empty() {
        return Location::None;
    }
    Location::Path {
        path: file.display().to_string(),
        line: line.map(|line| u32::try_from(line).unwrap_or(u32::MAX)),
    }
}

/// The record an `src/env` producer's warning becomes when the builder
/// pushes it into a phase's sink.
impl From<BuildWarning> for Diagnostic {
    fn from(warning: BuildWarning) -> Self {
        Diagnostic {
            location: location_of(&warning.file, warning.line),
            severity: Severity::Warning,
            message: warning.message,
            category: warning
                .category
                .as_deref()
                .map_or(Category::None, Category::parse),
        }
    }
}

/// A genuine per-file failure, printed as an `ERROR:` record. It is not a
/// warning: it never enters a sink, and it counts in `BuildStats.errors`.
impl From<&BuildErrorReport> for Diagnostic {
    fn from(error: &BuildErrorReport) -> Self {
        Diagnostic {
            location: location_of(&error.file, error.line),
            severity: Severity::Error,
            message: error.message.clone(),
            category: Category::None,
        }
    }
}

/// Per-document order: parse records (0, 0, seq); transform/collector records (priority, serial, seq).
///
/// `(priority, serial)` is the docutils transformer's sort key for the
/// transform that logged the record; `seq` keeps records of one emitter in
/// emission order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderKey {
    pub priority: u16,
    pub serial: u16,
    pub seq: u32,
}

/// One phase's records (one document's, for the per-document phases), held
/// until the builder flushes them.
#[derive(Debug, Default)]
pub struct DiagnosticSink {
    records: Vec<(OrderKey, Diagnostic)>,
    next_seq: u32,
}

impl DiagnosticSink {
    pub fn push(&mut self, key: OrderKey, d: Diagnostic) {
        self.records.push((key, d));
    }

    /// A fresh `seq`, counting up from 0 per sink.
    pub fn next_seq(&mut self) -> u32 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// The records in key order; records with equal keys keep the order
    /// they were pushed in (a stable sort).
    pub fn into_ordered(mut self) -> Vec<Diagnostic> {
        self.records.sort_by_key(|(key, _)| *key);
        self.records.into_iter().map(|(_, d)| d).collect()
    }
}

/// The configuration's say over the stream: which records are dropped
/// (`suppress_warnings`) and whether records print their category
/// (`show_warning_types`).
#[derive(Clone, Debug)]
pub struct WarningFilter {
    suppress_warnings: Vec<String>,
    show_warning_types: bool,
}

impl WarningFilter {
    pub fn from_config(config: &BuildConfig) -> Self {
        Self {
            suppress_warnings: config.suppress_warnings.clone(),
            show_warning_types: config.show_warning_types,
        }
    }

    /// Port of `is_suppressed_warning` (`sphinx/util/logging.py:415-426`):
    /// a pattern names the record's `type`, `type.*` or `type.subtype`. A
    /// record without a `type` is never suppressed, whatever its subtype,
    /// as Sphinx's `logger.warning` records are: it passes `type=None` when
    /// it is given none (`SphinxLoggerAdapter.warning`, `:157-200`), and
    /// `is_suppressed_warning` returns early for `None` (`:419`).
    ///
    /// One divergence: a record Sphinx logs with no `type` attribute at
    /// all, as `validate_intersphinx_mapping`'s `LOGGER.error`s are
    /// (`SP/sphinx/ext/intersphinx/_load.py:64-125`), is read as type and
    /// subtype `''` (`WarningSuppressor.filter`, `:436-446`), so `''`, `.`
    /// and `.*` suppress it there. This crate prints those `ERROR`s all,
    /// before the configuration error that stops the build either way.
    /// Probed on Sphinx 9.1.0.
    pub fn is_suppressed(&self, category: &Category) -> bool {
        let (ty, subtype) = match category {
            Category::None => return false,
            // Sphinx reads a missing subtype as `''`, so `type.` matches it.
            Category::Type(ty) => (ty.as_str(), ""),
            Category::TypeSub(ty, subtype) => (ty.as_str(), subtype.as_str()),
        };
        let patterns = [
            Cow::Borrowed(ty),
            Cow::Owned(format!("{ty}.*")),
            Cow::Owned(format!("{ty}.{subtype}")),
        ];
        patterns
            .iter()
            .any(|pattern| self.suppress_warnings.iter().any(|known| known == pattern))
    }

    /// Whether records print their ` [type.subtype]` suffix.
    pub fn show_warning_types(&self) -> bool {
        self.show_warning_types
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorType, WarningType};
    use std::path::PathBuf;

    fn path(path: &str, line: Option<u32>) -> Location {
        Location::Path {
            path: path.into(),
            line,
        }
    }

    fn docutils() -> Category {
        Category::Type("docutils".into())
    }

    fn typesub(ty: &str, subtype: &str) -> Category {
        Category::TypeSub(ty.into(), subtype.into())
    }

    fn record(
        location: Location,
        severity: Severity,
        message: &str,
        category: Category,
    ) -> Diagnostic {
        Diagnostic {
            location,
            severity,
            message: message.into(),
            category,
        }
    }

    /// The location forms of the oracle's warning stream [TW §5.2], each
    /// rendered with Sphinx's default `show_warning_types = True`.
    #[test]
    fn render_matches_sphinx_record_grammar() {
        let rows = [
            (
                path("/s/index.rst", Some(3)),
                Severity::Warning,
                "Title underline too short.",
                docutils(),
                "/s/index.rst:3: WARNING: Title underline too short. [docutils]",
            ),
            (
                // A reporter message with no line: docutils' `source:` plus
                // the `: ` separator.
                Location::Verbatim("/s/index.rst:".into()),
                Severity::Error,
                "Anonymous hyperlink mismatch: 1 references but 0 targets.\nSee \"backrefs\" attribute for IDs.",
                docutils(),
                "/s/index.rst:: ERROR: Anonymous hyperlink mismatch: 1 references but 0 targets.\nSee \"backrefs\" attribute for IDs. [docutils]",
            ),
            (
                path("/s/a.rst", None),
                Severity::Warning,
                "document isn't included in any toctree",
                typesub("toc", "not_included"),
                "/s/a.rst: WARNING: document isn't included in any toctree [toc.not_included]",
            ),
            (
                Location::None,
                Severity::Warning,
                "unknown role name: a:b:c",
                Category::None,
                "WARNING: unknown role name: a:b:c",
            ),
            (
                // Docutils' SEVERE prints as CRITICAL; the suffix follows the
                // last line of a multi-line message.
                Location::Verbatim("/s/a.rst:8".into()),
                Severity::Critical,
                "Problems with \"include\" directive path:\nInputError: x.",
                docutils(),
                "/s/a.rst:8: CRITICAL: Problems with \"include\" directive path:\nInputError: x. [docutils]",
            ),
        ];
        for (location, severity, message, category, expected) in rows {
            assert_eq!(
                record(location, severity, message, category).render(true),
                expected
            );
        }

        // A `subtype` without a `type` has no category: no suffix, and
        // nothing for `suppress_warnings` to match.
        assert_eq!(
            Category::from_parts(None, Some("empty_glob")),
            Category::None
        );
        assert_eq!(
            Category::from_parts(Some("ref"), Some("ref")),
            typesub("ref", "ref")
        );
        assert_eq!(Category::from_parts(Some("docutils"), None), docutils());
    }

    /// `getMessage` adds the level prefix to a record without a location
    /// only when the message does not already contain it
    /// (`sphinx/util/logging.py:99-108`).
    #[test]
    fn a_location_less_record_already_holding_its_prefix_prints_as_is() {
        assert_eq!(
            record(
                Location::None,
                Severity::Warning,
                "tool said WARNING: x",
                Category::None
            )
            .render(true),
            "tool said WARNING: x"
        );
        // The check is per level, and a located record always gets it.
        assert_eq!(
            record(
                Location::None,
                Severity::Error,
                "tool said WARNING: x",
                Category::None
            )
            .render(true),
            "ERROR: tool said WARNING: x"
        );
        assert_eq!(
            record(
                path("/s/a.rst", Some(1)),
                Severity::Warning,
                "tool said WARNING: x",
                Category::None
            )
            .render(true),
            "/s/a.rst:1: WARNING: tool said WARNING: x"
        );
    }

    #[test]
    fn show_warning_types_false_drops_the_suffix() {
        assert_eq!(
            record(
                path("/s/a.rst", None),
                Severity::Warning,
                "document isn't included in any toctree",
                typesub("toc", "not_included"),
            )
            .render(false),
            "/s/a.rst: WARNING: document isn't included in any toctree"
        );
        assert_eq!(
            record(
                Location::Verbatim("/s/index.rst:3".into()),
                Severity::Error,
                "Unexpected indentation.",
                docutils(),
            )
            .render(false),
            "/s/index.rst:3: ERROR: Unexpected indentation."
        );
    }

    /// A document's parse record sorts before its transform records, and
    /// those sort by priority, then serial, whatever order they were
    /// pushed in; equal keys keep push order.
    #[test]
    fn ordered_sink_interleaves_parse_and_transform_records() {
        let message = |text: &str| record(Location::None, Severity::Warning, text, Category::None);
        let key = |priority, serial, seq| OrderKey {
            priority,
            serial,
            seq,
        };
        let mut sink = DiagnosticSink::default();
        sink.push(key(850, 39, 0), message("dangling reference"));
        sink.push(key(220, 4, 0), message("undefined substitution"));
        sink.push(key(0, 0, 2), message("parse record"));
        sink.push(key(220, 4, 0), message("second undefined substitution"));
        let ordered: Vec<String> = sink
            .into_ordered()
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(
            ordered,
            [
                "parse record",
                "undefined substitution",
                "second undefined substitution",
                "dangling reference",
            ]
        );

        let mut sink = DiagnosticSink::default();
        assert_eq!(
            [sink.next_seq(), sink.next_seq(), sink.next_seq()],
            [0, 1, 2]
        );
    }

    /// `is_suppressed_warning` (`sphinx/util/logging.py:415-426`): a pattern
    /// names a `type`, `type.*` or `type.subtype`.
    #[test]
    fn suppress_warnings_matches_sphinx() {
        let filter = |patterns: &[&str]| {
            WarningFilter::from_config(&BuildConfig {
                suppress_warnings: patterns.iter().map(|pattern| pattern.to_string()).collect(),
                ..BuildConfig::default()
            })
        };
        let rows: [(&[&str], Category, bool); 12] = [
            (&["docutils"], docutils(), true),
            (&["ref.*"], typesub("ref", "footnote"), true),
            (&["ref.*"], Category::Type("ref".into()), true),
            (&["ref.ref"], typesub("ref", "ref"), true),
            (&["ref.ref"], typesub("ref", "footnote"), false),
            (&["ref.ref"], Category::Type("ref".into()), false),
            (&["image"], typesub("image", "not_readable"), true),
            (
                &["image.not_readable"],
                typesub("image", "nonlocal_uri"),
                false,
            ),
            (&["toc"], typesub("ref", "doc"), false),
            (&[], docutils(), false),
            (
                &["docutils", "ref.*", "image", "toc.not_included"],
                Category::None,
                false,
            ),
            // `logger.warning`'s `type=None`: not even the patterns a `''`
            // type would match (probed with the malformed option
            // description, which has no type, and a subtype-only record).
            (&["", ".", ".*", ".empty_glob"], Category::None, false),
        ];
        for (patterns, category, suppressed) in rows {
            assert_eq!(
                filter(patterns).is_suppressed(&category),
                suppressed,
                "{patterns:?} vs {category:?}"
            );
        }
    }

    /// The builder converts an `src/env` producer's warning at the sink
    /// boundary: an empty file is a record with no location, a category
    /// splits on its first `.`.
    #[test]
    fn build_warnings_convert_at_the_sink_boundary() {
        let warning = |file: &str, line: Option<usize>, category: Option<&str>| {
            BuildWarning::new(
                PathBuf::from(file),
                line,
                "message".into(),
                WarningType::Other,
            )
            .with_category(category.map(str::to_string))
        };
        let rows = [
            (
                warning("", None, None),
                Location::None,
                Category::None,
                "WARNING: message",
            ),
            (
                warning("/s/a.rst", Some(4), Some("toc.not_readable")),
                path("/s/a.rst", Some(4)),
                typesub("toc", "not_readable"),
                "/s/a.rst:4: WARNING: message [toc.not_readable]",
            ),
            (
                warning("/s/a.rst", None, Some("ref")),
                path("/s/a.rst", None),
                Category::Type("ref".into()),
                "/s/a.rst: WARNING: message [ref]",
            ),
            (
                // Only the first `.` separates the type.
                warning("/s/a.rst", Some(1), Some("a.b.c")),
                path("/s/a.rst", Some(1)),
                typesub("a", "b.c"),
                "/s/a.rst:1: WARNING: message [a.b.c]",
            ),
        ];
        for (warning, location, category, rendered) in rows {
            assert_eq!(warning.render(), rendered);
            assert_eq!(
                Diagnostic::from(warning),
                record(location, Severity::Warning, "message", category)
            );
        }

        let error = BuildErrorReport::new(
            PathBuf::from("/s/bad.rst"),
            None,
            "stream did not contain valid UTF-8".into(),
            ErrorType::ParseError,
        );
        assert_eq!(
            Diagnostic::from(&error).render(true),
            "/s/bad.rst: ERROR: stream did not contain valid UTF-8"
        );
    }
}
