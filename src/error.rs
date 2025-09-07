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
    Template(#[from] handlebars::RenderError),

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
}

#[derive(Debug, Clone)]
pub struct BuildWarning {
    pub file: PathBuf,
    pub line: Option<usize>,
    pub message: String,
    #[allow(dead_code)]
    pub warning_type: WarningType,
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
        }
    }

    pub fn missing_toctree_ref(file: PathBuf, line: Option<usize>, reference: &str) -> Self {
        Self::new(
            file,
            line,
            format!(
                "toctree contains reference to nonexisting document '{}'",
                reference
            ),
            WarningType::MissingToctreeRef,
        )
    }

    pub fn orphaned_document(file: PathBuf) -> Self {
        Self::new(
            file,
            None,
            "document isn't included in any toctree".to_string(),
            WarningType::OrphanedDocument,
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
    #[allow(dead_code)]
    pub fn new(file: PathBuf, line: Option<usize>, message: String, error_type: ErrorType) -> Self {
        Self {
            file,
            line,
            message,
            error_type,
        }
    }
}
