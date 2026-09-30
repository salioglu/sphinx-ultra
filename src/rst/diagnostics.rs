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

use serde::{Deserialize, Serialize};

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
    /// (`ParseLogWarning::rendered_path`).
    ///
    /// This flag only records that quirk. `BuildWarning::from_diagnostic`
    /// ignores it: the caller that resolves `source` to a path (the merge
    /// phase) applies the suffix to that path and passes the result as
    /// `source_path`.
    pub doc2path_location: bool,
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

    #[test]
    fn a_record_missing_a_field_does_not_decode() {
        // The cache-shape rule: no `#[serde(default)]`, so a stale entry
        // misses rather than decoding with `doc2path_location` off.
        let mut value = serde_json::to_value(sample()).unwrap();
        value.as_object_mut().unwrap().remove("doc2path_location");
        assert!(serde_json::from_value::<Diagnostic>(value).is_err());
    }
}
