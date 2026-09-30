use crate::rst::diagnostics::{Diagnostic, DiagnosticChannel};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum BuildError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML serialization error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Template rendering error: {0}")]
    Template(String),

    #[error("File parsing error: {file}: {message}")]
    Parse { file: String, message: String },

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Thread pool error: {0}")]
    ThreadPool(#[from] rayon::ThreadPoolBuildError),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Invalid document format: {0}")]
    InvalidFormat(String),

    #[error("Cross-reference error: {reference} not found")]
    CrossReference { reference: String },

    #[error("Template not found: {0}")]
    TemplateNotFound(String),

    #[error("Syntax highlighting error: {0}")]
    SyntaxHighlight(String),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

#[derive(Debug, Clone)]
pub struct BuildWarning {
    pub file: PathBuf,
    pub line: Option<usize>,
    pub message: String,
    #[allow(dead_code)]
    pub warning_type: WarningType,
    /// Sphinx's `type.subtype` warning category (`toc.not_readable`,
    /// `toc.not_included`, ...), which `show_warning_types` — on by default
    /// since Sphinx 8.3 — appends to the rendered message as ` [category]`.
    ///
    /// `None` for warnings Sphinx logs without a `type` (its
    /// `SphinxLoggerAdapter` only appends the suffix when `type` is set, so
    /// a `subtype`-only warning such as the toctree `empty_glob` one prints
    /// bare). See `util/logging.py:545-549`.
    pub category: Option<String>,
    /// The severity Sphinx's log prefix spells (`WARNING: `, `ERROR: `,
    /// `CRITICAL: `). Every constructor defaults to [`WarningLevel::Warning`];
    /// only read-phase [`Diagnostic`]s raise it ([`Self::from_diagnostic`]).
    pub level: WarningLevel,
    /// The location is a docutils reporter `source:line` string rather than a
    /// logger location. Sphinx's `WarningStream` hands the log handler
    /// `location='{source}:{line}'` with the line empty when docutils had
    /// none (`sphinx/util/docutils.py:388-393`), and the handler then prints
    /// `{location}: {prefix}{message}` (`sphinx/util/logging.py:99-107`), so
    /// a reporter record without a line prints `source:: ERROR: ...` where a
    /// logger warning without a line prints `source: WARNING: ...`.
    pub docutils_location: bool,
}

/// Sphinx's warning-stream severity. A docutils system message reaches the
/// stream as `WARNING`, `ERROR` or `SEVERE` and `SEVERE` prints as
/// `CRITICAL` (`LEVEL_NAMES`, `sphinx/util/logging.py:30-40`; the prefix,
/// `:116-126`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLevel {
    Warning,
    Error,
    Critical,
}

impl WarningLevel {
    /// The level for a docutils system-message level: `2` is WARNING, `3`
    /// ERROR, `4` (SEVERE) and above CRITICAL. Levels below 2 never print
    /// (docutils' `report_level` is 2), so no diagnostic carries one; they
    /// fall to WARNING, the default of Sphinx's `LEVEL_NAMES`.
    pub fn from_docutils(level: u8) -> Self {
        match level {
            0..=2 => Self::Warning,
            3 => Self::Error,
            _ => Self::Critical,
        }
    }

    /// The prefix `SphinxWarningLogRecord.prefix` puts before the message.
    fn prefix(self) -> &'static str {
        match self {
            Self::Warning => "WARNING: ",
            Self::Error => "ERROR: ",
            Self::Critical => "CRITICAL: ",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildErrorReport {
    pub file: PathBuf,
    pub line: Option<usize>,
    pub message: String,
    #[allow(dead_code)]
    pub error_type: ErrorType,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum WarningType {
    MissingToctreeRef,
    OrphanedDocument,
    BrokenCrossReference,
    MissingFile,
    UnusedLabel,
    DuplicateLabel,
    EmptyToctree,
    Other,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ErrorType {
    ParseError,
    FileNotFound,
    TemplateError,
    SyntaxError,
    Other,
}

impl BuildWarning {
    pub fn new(
        file: PathBuf,
        line: Option<usize>,
        message: String,
        warning_type: WarningType,
    ) -> Self {
        Self {
            file,
            line,
            message,
            warning_type,
            category: None,
            level: WarningLevel::Warning,
            docutils_location: false,
        }
    }

    /// The warning for a read-phase [`Diagnostic`], located at `source_path`.
    ///
    /// The caller resolves the record's `source` index to a path; any
    /// `doc2path` suffix quirk ([`Diagnostic::doc2path_location`]) is folded
    /// in there, so it is not read here. A reporter record prints with the
    /// level Sphinx prefixes, the `docutils` type and a docutils location
    /// (`source:line`, or `source:` with no line); a logger record keeps its
    /// own category, or none, and a plain location.
    pub fn from_diagnostic(d: &Diagnostic, source_path: PathBuf) -> Self {
        let reporter = d.channel == DiagnosticChannel::Reporter;
        Self {
            file: source_path,
            line: d.line.map(|line| line as usize),
            message: d.text.clone(),
            warning_type: WarningType::Other,
            category: if reporter {
                Some("docutils".to_string())
            } else {
                d.category.clone()
            },
            level: WarningLevel::from_docutils(d.level),
            docutils_location: reporter,
        }
    }

    /// Attach Sphinx's `type.subtype` category (see [`BuildWarning::category`]).
    #[must_use]
    pub fn with_category(mut self, category: Option<String>) -> Self {
        self.category = category;
        self
    }

    /// The warning as `sphinx-build` prints it:
    /// `path[:line]: LEVEL: message[ [type.subtype]]`, where `LEVEL` is
    /// `WARNING`, `ERROR` or `CRITICAL` ([`WarningLevel`]).
    ///
    /// One renderer for every sink (stderr, `-w` warning file, the
    /// environment-oracle differential) so a message can only ever be
    /// formatted one way.
    ///
    /// An empty `file` is a warning Sphinx logs with no `location` at all
    /// (the intersphinx "failed to reach any of the inventories" report, for
    /// one): those print as a bare `WARNING: ...`, with no location prefix
    /// and no stray colon.
    pub fn render(&self) -> String {
        let category = match &self.category {
            Some(category) => format!(" [{category}]"),
            None => String::new(),
        };
        let prefix = self.level.prefix();
        if self.file.as_os_str().is_empty() {
            return format!("{prefix}{}{category}", self.message);
        }
        let line = match (self.line, self.docutils_location) {
            (Some(line), _) => format!(":{line}"),
            // `source:` + an empty line, then the handler's own `: `.
            (None, true) => ":".to_string(),
            (None, false) => String::new(),
        };
        format!(
            "{}{line}: {prefix}{}{category}",
            self.file.display(),
            self.message
        )
    }

    #[allow(dead_code)]
    pub fn broken_cross_reference(file: PathBuf, line: Option<usize>, reference: &str) -> Self {
        Self::new(
            file,
            line,
            format!("cross-reference target not found: '{}'", reference),
            WarningType::BrokenCrossReference,
        )
    }
}

impl BuildErrorReport {
    pub fn new(file: PathBuf, line: Option<usize>, message: String, error_type: ErrorType) -> Self {
        Self {
            file,
            line,
            message,
            error_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        channel: DiagnosticChannel,
        level: u8,
        line: Option<u32>,
        category: Option<&str>,
        text: &str,
    ) -> Diagnostic {
        Diagnostic {
            seq: 0,
            channel,
            level,
            category: category.map(str::to_string),
            text: text.to_string(),
            source: 0,
            line,
            doc2path_location: false,
        }
    }

    fn rendered(d: &Diagnostic) -> String {
        BuildWarning::from_diagnostic(d, PathBuf::from("index.rst")).render()
    }

    #[test]
    fn reporter_records_render_with_level_and_docutils_type() {
        let d = record(
            DiagnosticChannel::Reporter,
            4,
            Some(14),
            None,
            "Problems with \"include\" directive path:\nInputError: [Errno 2] No such file or directory: 'nothere.rst'.",
        );
        assert_eq!(
            rendered(&d),
            "index.rst:14: CRITICAL: Problems with \"include\" directive path:\nInputError: [Errno 2] No such file or directory: 'nothere.rst'. [docutils]"
        );
    }

    #[test]
    fn level_two_renders_warning() {
        let d = record(
            DiagnosticChannel::Reporter,
            2,
            Some(6),
            None,
            "Inline emphasis start-string without end-string.",
        );
        assert_eq!(
            rendered(&d),
            "index.rst:6: WARNING: Inline emphasis start-string without end-string. [docutils]"
        );
    }

    #[test]
    fn level_three_renders_error() {
        let d = record(
            DiagnosticChannel::Reporter,
            3,
            Some(2),
            None,
            "Unknown directive type \"nope\".",
        );
        assert_eq!(
            rendered(&d),
            "index.rst:2: ERROR: Unknown directive type \"nope\". [docutils]"
        );
    }

    #[test]
    fn a_record_without_a_line_renders_a_double_colon() {
        let d = record(
            DiagnosticChannel::Reporter,
            3,
            None,
            None,
            "Anonymous hyperlink mismatch: 1 references but 0 targets.\nSee \"backrefs\" attribute for IDs.",
        );
        assert_eq!(
            rendered(&d),
            "index.rst:: ERROR: Anonymous hyperlink mismatch: 1 references but 0 targets.\nSee \"backrefs\" attribute for IDs. [docutils]"
        );
    }

    #[test]
    fn logger_records_keep_their_category_or_none() {
        let bare = record(
            DiagnosticChannel::Logger,
            2,
            Some(8),
            None,
            "toctree glob pattern 'x*' didn't match any documents",
        );
        assert_eq!(
            rendered(&bare),
            "index.rst:8: WARNING: toctree glob pattern 'x*' didn't match any documents"
        );
        let typed = record(
            DiagnosticChannel::Logger,
            2,
            Some(8),
            Some("toc.not_readable"),
            "toctree contains reference to nonexisting document 'gone'",
        );
        assert_eq!(
            rendered(&typed),
            "index.rst:8: WARNING: toctree contains reference to nonexisting document 'gone' [toc.not_readable]"
        );
    }

    #[test]
    fn a_logger_record_without_a_line_keeps_the_single_colon_form() {
        // Only the docutils reporter spells the location `source:line`, so a
        // missing line leaves `source:` and the printer adds its own `: `.
        // A logger location is a bare path that gets no second colon.
        let d = record(DiagnosticChannel::Logger, 2, None, None, "text");
        assert_eq!(rendered(&d), "index.rst: WARNING: text");
    }

    #[test]
    fn from_diagnostic_carries_the_source_the_line_and_the_level() {
        let d = record(DiagnosticChannel::Reporter, 3, Some(9), None, "t");
        let w = BuildWarning::from_diagnostic(&d, PathBuf::from("/abs/inc.rst"));
        assert_eq!(w.file, PathBuf::from("/abs/inc.rst"));
        assert_eq!(w.line, Some(9));
        assert_eq!(w.level, WarningLevel::Error);
        assert_eq!(w.category.as_deref(), Some("docutils"));
        assert_eq!(w.message, "t");
        // `doc2path_location` is the caller's business (it is folded into
        // `source_path` before this call), so the flag changes nothing here.
        let mut d2 = d.clone();
        d2.doc2path_location = true;
        assert_eq!(
            BuildWarning::from_diagnostic(&d2, PathBuf::from("/abs/inc.rst")).render(),
            w.render()
        );
    }

    #[test]
    fn docutils_levels_map_to_warning_error_critical() {
        assert_eq!(WarningLevel::from_docutils(2), WarningLevel::Warning);
        assert_eq!(WarningLevel::from_docutils(3), WarningLevel::Error);
        assert_eq!(WarningLevel::from_docutils(4), WarningLevel::Critical);
        // docutils' `halt_level` 5 (and anything above SEVERE) is still the
        // top of Sphinx's logging scale.
        assert_eq!(WarningLevel::from_docutils(5), WarningLevel::Critical);
    }

    #[test]
    fn existing_warnings_still_render_as_warning() {
        let with_line = BuildWarning::new(
            PathBuf::from("a.rst"),
            Some(3),
            "duplicate label x".to_string(),
            WarningType::Other,
        );
        assert_eq!(with_line.level, WarningLevel::Warning);
        assert_eq!(with_line.render(), "a.rst:3: WARNING: duplicate label x");

        let no_line = BuildWarning::new(
            PathBuf::from("a.rst"),
            None,
            "document isn't included in any toctree".to_string(),
            WarningType::OrphanedDocument,
        )
        .with_category(Some("toc.not_included".to_string()));
        assert_eq!(
            no_line.render(),
            "a.rst: WARNING: document isn't included in any toctree [toc.not_included]"
        );

        let no_location = BuildWarning::new(
            PathBuf::new(),
            None,
            "failed to reach any of the inventories".to_string(),
            WarningType::Other,
        );
        assert_eq!(
            no_location.render(),
            "WARNING: failed to reach any of the inventories"
        );
    }
}
