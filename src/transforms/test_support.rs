//! What the transforms' unit tests share: a snippet read the way the
//! read-state fixture reads one (`tools/gen_read_state_fixture.py`), and the
//! spellings Sphinx's reads are compared in.

use crate::config::BuildConfig;
use crate::read::{read_standalone, ReadSettings};
use crate::rst::ParseOptions;
use crate::transforms::reporter::LogRecord;

/// The document line every post-read pformat of `<SRCDIR>/index.rst` opens
/// with.
pub(crate) const DOC: &str = "<document source=\"<SRCDIR>/index.rst\" \
                              translation_progress=\"{'total': 0, 'translated': 0}\">\n";

/// One id-trace event, `[phase, kind, id, attached]`, the phase's Python
/// name shortened to its class ([`read_snippet`]).
pub(crate) type Event = (String, &'static str, String, bool);

/// One read of a snippet as the read-state harness reads it.
pub(crate) struct Read {
    /// The post-read pformat, less [`DOC`].
    pub(crate) body: String,
    /// The warning stream, one entry per line.
    pub(crate) warnings: Vec<String>,
    /// The id trace.
    pub(crate) trace: Vec<Event>,
    /// The reporter's records of every level, `(level, text)`.
    pub(crate) reported: Vec<(u8, String)>,
}

/// How Sphinx parses `<SRCDIR>/index.rst`.
pub(crate) fn index_rst() -> ParseOptions {
    ParseOptions {
        source_path: "<SRCDIR>/index.rst".to_string(),
        sphinx: true,
        docname: "index".to_string(),
        ..ParseOptions::default()
    }
}

/// The read-state fixture's read settings (M1's validation off, smart
/// quotes off, the id trace on), with `keep_warnings` as given.
pub(crate) fn harness_settings(keep_warnings: bool) -> ReadSettings {
    ReadSettings {
        trace_ids: true,
        ..ReadSettings::from_config(&BuildConfig {
            validate_directives: false,
            keep_warnings,
            ..BuildConfig::default()
        })
    }
}

/// Our read of `rst` as `<SRCDIR>/index.rst`, the way the read-state
/// fixture reads a snippet ([`harness_settings`]), with `keep_warnings` as
/// given.
pub(crate) fn read_snippet(rst: &str, keep_warnings: bool) -> Read {
    read_snippet_with(rst, &harness_settings(keep_warnings))
}

/// [`read_snippet`] under `settings`.
pub(crate) fn read_snippet_with(rst: &str, settings: &ReadSettings) -> Read {
    let read = read_standalone(rst, &index_rst(), settings);
    let pformat = read.outcome.doctree().root.pformat();
    let body = pformat
        .strip_prefix(DOC)
        .unwrap_or_else(|| panic!("{rst:?}: unexpected document line:\n{pformat}"))
        .to_string();
    let warnings = read
        .diagnostics
        .iter()
        .flat_map(|diagnostic| {
            diagnostic
                .render(true)
                .split('\n')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    let state = read.outcome.state();
    let trace = state
        .ids
        .trace()
        .iter()
        .map(|event| {
            let phase = event.phase.rsplit('.').next().unwrap_or_default();
            (
                phase.to_string(),
                event.kind,
                event.id.clone(),
                event.attached,
            )
        })
        .collect();
    let reported = state
        .log
        .iter()
        .filter_map(|record| match record {
            LogRecord::Reporter { level, text, .. } => Some((*level, text.clone())),
            LogRecord::Logger { .. } => None,
        })
        .collect();
    Read {
        body,
        warnings,
        trace,
        reported,
    }
}

/// An id-trace event of [`Read::trace`].
pub(crate) fn event(phase: &str, kind: &'static str, id: &str, attached: bool) -> Event {
    (phase.to_string(), kind, id.to_string(), attached)
}

/// `<SRCDIR>/index.rst:<line>: ERROR: <text> [docutils]`, as the stream's
/// lines.
pub(crate) fn error(line: &str, text: &str) -> Vec<String> {
    format!("<SRCDIR>/index.rst:{line}: ERROR: {text} [docutils]")
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// What Sphinx 9.1.0's read of one snippet gives through the read-state
/// harness (`keep_warnings = True`): the post-read pformat less [`DOC`],
/// the warning stream's lines and the id trace, each event's phase
/// shortened to its class.
pub(crate) struct SphinxRead {
    pub(crate) rst: &'static str,
    pub(crate) body: &'static str,
    pub(crate) warnings: &'static [&'static str],
    pub(crate) trace: &'static [(&'static str, &'static str, &'static str, bool)],
}

/// Our read of each row ([`read_snippet`], `keep_warnings = True`) against
/// Sphinx's: one entry per row whose body, warning stream or id trace
/// differs, with both sides.
pub(crate) fn differences(rows: &[SphinxRead]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            let read = read_snippet(row.rst, true);
            let trace: Vec<Event> = row
                .trace
                .iter()
                .map(|&(phase, kind, id, attached)| event(phase, kind, id, attached))
                .collect();
            let same =
                read.body == row.body && read.warnings == row.warnings && read.trace == trace;
            (!same).then(|| {
                format!(
                    "{:?}\n--- sphinx 9.1.0 ---\n{}{:#?}\n{:#?}\n--- ours ---\n{}{:#?}\n{:#?}",
                    row.rst, row.body, row.warnings, trace, read.body, read.warnings, read.trace
                )
            })
        })
        .collect()
}
