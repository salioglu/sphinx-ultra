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
        }
    }

    /// Attach Sphinx's `type.subtype` category (see [`BuildWarning::category`]).
    #[must_use]
    pub fn with_category(mut self, category: Option<String>) -> Self {
        self.category = category;
        self
    }

    /// The warning as `sphinx-build` prints it:
    /// `path[:line]: WARNING: message[ [type.subtype]]`.
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
        if self.file.as_os_str().is_empty() {
            return format!("WARNING: {}{category}", self.message);
        }
        let line = match self.line {
            Some(line) => format!(":{line}"),
            None => String::new(),
        };
        format!(
            "{}{line}: WARNING: {}{category}",
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
