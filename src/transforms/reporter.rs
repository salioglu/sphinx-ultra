//! The reporter log: every message docutils' reporter and Sphinx's logger
//! raise for a document, recorded when it is raised, and rendered into
//! warning-stream records ([TW §5.3, §5.4; DS §6.4]).
//!
//! Docutils writes a message to Sphinx's warning stream inside
//! `Reporter.system_message`, the moment it creates it, so what prints is
//! the message's text at creation, wherever the node later ends up — in the
//! tree, detached, or nowhere. The log keeps exactly that, with the
//! message's level and docutils location; whether it prints is decided
//! when the log is rendered.

use crate::diagnostics::{Category, Diagnostic, Location, OrderKey, Severity};
use crate::doctree::messages::{ERROR, WARNING};
use crate::transforms::state::{DocLoc, SourceRef};

/// One logged record of a document, with the key it is ordered by among
/// the document's records ([`OrderKey`]: parse records `(0, 0, seq)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogRecord {
    /// A docutils reporter message: its level, its location as docutils
    /// stamped it on the node, and the node's `Element.astext()` at
    /// creation.
    Reporter {
        key: OrderKey,
        level: u8,
        loc: DocLoc,
        text: String,
    },
    /// A Sphinx `logger.warning`, its location rendered when it was logged
    /// ([`crate::rst::LogLocation::render`]).
    Logger {
        key: OrderKey,
        diagnostic: Diagnostic,
    },
}

/// The warning-stream records of `records`, each with its own key, in
/// input order. A reporter message prints iff its level reaches Sphinx's
/// `report_level` of 2 [TW §5.3.3] (INFO never prints and is dropped) as a
/// `[docutils]` record: WARNING, ERROR, and SEVERE as `CRITICAL`
/// [decision 4]. Its location is docutils' own `source:line`, verbatim
/// ([TW §5.3.1]): `sources[id]` for a table source, `doc_path` (docutils'
/// `reporter.source`) when it names none, and `source:` when it has no
/// line. The table holds the document absolute and included files, and
/// the include padding, in the srcdir-relative spelling, which is
/// docutils' cwd-relative path when the cwd is the source directory. The
/// message is the text Sphinx's `WarningStream` logs:
/// `report_re.sub('', text).rstrip()` (`SP/sphinx/util/docutils.py:385-393`).
/// Logger records pass through as they were rendered.
pub fn render_records(
    records: Vec<LogRecord>,
    sources: &[String],
    doc_path: &str,
) -> Vec<(OrderKey, Diagnostic)> {
    records
        .into_iter()
        .filter_map(|record| match record {
            LogRecord::Reporter {
                key,
                level,
                loc,
                text,
            } => {
                let severity = match level {
                    WARNING => Severity::Warning,
                    ERROR => Severity::Error,
                    level if level > ERROR => Severity::Critical,
                    _ => return None,
                };
                let source = match loc.source {
                    Some(SourceRef::Table(source)) => sources
                        .get(usize::from(source))
                        .map_or(doc_path, String::as_str),
                    None => doc_path,
                };
                let location = match loc.line {
                    Some(line) => format!("{source}:{line}"),
                    None => format!("{source}:"),
                };
                Some((
                    key,
                    Diagnostic {
                        location: Location::Verbatim(location),
                        severity,
                        message: text.trim_end_matches(crate::utils::py_isspace).to_string(),
                        category: Category::Type("docutils".to_string()),
                    },
                ))
            }
            LogRecord::Logger { key, diagnostic } => Some((key, diagnostic)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctree::messages::{INFO, SEVERE};

    fn key(seq: u32) -> OrderKey {
        OrderKey {
            priority: 0,
            serial: 0,
            seq,
        }
    }

    fn reporter(seq: u32, level: u8, loc: DocLoc, text: &str) -> LogRecord {
        LogRecord::Reporter {
            key: key(seq),
            level,
            loc,
            text: text.to_string(),
        }
    }

    /// A reporter record prints its source string as the table spells it —
    /// the document absolute, an included file and the include padding
    /// srcdir-relative, as docutils' cwd-relative path reads with the cwd
    /// in the source directory ([TW §5.2]: `circ_b.rst:3`) — and the
    /// document's path when it names no source. A record without a line
    /// prints `source:`, which renders `source:: ERROR` ([TW §5.3.4]).
    /// INFO never prints; WARNING, ERROR and SEVERE map to Sphinx's
    /// prefixes, SEVERE to `CRITICAL`. Keys and input order survive, the
    /// message loses its trailing whitespace, and logger records pass
    /// through untouched.
    #[test]
    fn render_records_prints_sources_verbatim() {
        let sources = [
            "/src/index.rst".to_string(),
            "part.rst".to_string(),
            "internal padding after part.rst".to_string(),
        ];
        let logged = Diagnostic {
            location: Location::None,
            severity: Severity::Warning,
            message: "unknown role name: a:b:c".to_string(),
            category: Category::None,
        };
        let records = vec![
            reporter(
                0,
                WARNING,
                DocLoc::at(0, 3),
                "Title underline too short.\n\nTitle\n===",
            ),
            reporter(1, INFO, DocLoc::at(0, 4), "Duplicate implicit target name."),
            reporter(2, ERROR, DocLoc::at(1, 6), "Unknown target name: \"x\".\n"),
            LogRecord::Logger {
                key: key(3),
                diagnostic: logged.clone(),
            },
            reporter(4, SEVERE, DocLoc::at(1, 1), "Problems with \"include\"."),
            reporter(
                5,
                ERROR,
                DocLoc {
                    source: Some(SourceRef::Table(2)),
                    line: Some(5),
                },
                "Anonymous hyperlink mismatch.",
            ),
            reporter(6, ERROR, DocLoc::default(), "Anonymous hyperlink mismatch."),
            reporter(
                7,
                WARNING,
                DocLoc {
                    source: Some(SourceRef::Table(0)),
                    line: None,
                },
                "Unknown target name.",
            ),
        ];
        let rendered: Vec<(u32, String)> = render_records(records, &sources, "/src/index.rst")
            .into_iter()
            .map(|(key, diagnostic)| (key.seq, diagnostic.render(true)))
            .collect();
        assert_eq!(
            rendered,
            [
                (
                    0,
                    "/src/index.rst:3: WARNING: Title underline too short.\n\nTitle\n=== [docutils]"
                ),
                (
                    2,
                    "part.rst:6: ERROR: Unknown target name: \"x\". [docutils]"
                ),
                (3, "WARNING: unknown role name: a:b:c"),
                (
                    4,
                    "part.rst:1: CRITICAL: Problems with \"include\". [docutils]"
                ),
                (
                    5,
                    "internal padding after part.rst:5: ERROR: Anonymous hyperlink mismatch. [docutils]"
                ),
                (
                    6,
                    "/src/index.rst:: ERROR: Anonymous hyperlink mismatch. [docutils]"
                ),
                (
                    7,
                    "/src/index.rst:: WARNING: Unknown target name. [docutils]"
                ),
            ]
            .map(|(seq, line)| (seq, line.to_string()))
        );
    }
}
