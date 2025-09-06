use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// Number of parallel jobs to use (defaults to number of CPU cores)
    pub parallel_jobs: Option<usize>,

    /// Maximum cache size in MB
    pub max_cache_size_mb: usize,

    /// Cache expiration time in hours
    pub cache_expiration_hours: u64,

    /// Output format configuration
    pub output: OutputConfig,

    /// Theme configuration
    pub theme: ThemeConfig,

    /// Extension configuration
    pub extensions: Vec<String>,

    /// Custom template directories
    pub template_dirs: Vec<PathBuf>,

    /// Static file directories
    pub static_dirs: Vec<PathBuf>,

    /// Build optimization settings
    pub optimization: OptimizationConfig,

    // Sphinx-compatible fields
    /// Project name
    pub project: String,

    /// Project version
    pub version: Option<String>,

    /// Project release
    pub release: Option<String>,

    /// Copyright notice
    pub copyright: Option<String>,

    /// Language code
    pub language: Option<String>,

    /// Root document
    pub root_doc: Option<String>,

    /// HTML theme style files
    pub html_style: Vec<String>,

    /// HTML CSS files
    pub html_css_files: Vec<String>,

    /// HTML JavaScript files
    pub html_js_files: Vec<String>,

    /// HTML static paths
    pub html_static_path: Vec<PathBuf>,

    /// HTML logo file
    pub html_logo: Option<String>,

    /// HTML favicon file
    pub html_favicon: Option<String>,

    /// HTML title
    pub html_title: Option<String>,

    /// HTML short title
    pub html_short_title: Option<String>,

    /// Show copyright in HTML
    pub html_show_copyright: Option<bool>,

    /// Show Sphinx attribution
    pub html_show_sphinx: Option<bool>,

    /// Copy source files
    pub html_copy_source: Option<bool>,

    /// Show source links
    pub html_show_sourcelink: Option<bool>,

    /// Source link suffix
    pub html_sourcelink_suffix: Option<String>,

    /// Use index
    pub html_use_index: Option<bool>,

    /// Use OpenSearch
    pub html_use_opensearch: Option<bool>,

    /// Last updated format
    pub html_last_updated_fmt: Option<String>,

    /// Templates path
    pub templates_path: Vec<PathBuf>,

    /// Turn warnings into errors
    pub fail_on_warning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Output HTML format
    pub html_theme: String,

    /// Enable syntax highlighting
    pub syntax_highlighting: bool,

    /// Syntax highlighting theme
    pub highlight_theme: String,

    /// Generate search index
    pub search_index: bool,

    /// Minify output HTML
    pub minify_html: bool,

    /// Compress output files
    pub compress_output: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Theme name
    pub name: String,

    /// Theme-specific configuration
    pub options: serde_json::Value,

    /// Custom CSS files
    pub custom_css: Vec<PathBuf>,

    /// Custom JavaScript files
    pub custom_js: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationConfig {
    /// Enable parallel processing
    pub parallel_processing: bool,

    /// Enable incremental builds
    pub incremental_builds: bool,

    /// Cache parsed documents
    pub document_caching: bool,

    /// Optimize images
    pub image_optimization: bool,

    /// Bundle assets
    pub asset_bundling: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            parallel_jobs: None,
            max_cache_size_mb: 500,
            cache_expiration_hours: 24,
            output: OutputConfig::default(),
            theme: ThemeConfig::default(),
            extensions: vec![
                "sphinx.ext.autodoc".to_string(),
                "sphinx.ext.viewcode".to_string(),
                "sphinx.ext.intersphinx".to_string(),
            ],
            template_dirs: vec![],
            static_dirs: vec![],
            optimization: OptimizationConfig::default(),

            // Sphinx-compatible defaults
            project: "Sphinx Ultra Project".to_string(),
            version: Some("1.0.0".to_string()),
            release: Some("1.0.0".to_string()),
            copyright: Some("2024, Sphinx Ultra".to_string()),
            language: Some("en".to_string()),
            root_doc: Some("index".to_string()),
            html_style: vec!["sphinx_rtd_theme.css".to_string()],
            html_css_files: vec![],
            html_js_files: vec![],
            html_static_path: vec![PathBuf::from("_static")],
            html_logo: None,
            html_favicon: None,
            html_title: None,
            html_short_title: None,
            html_show_copyright: Some(true),
            html_show_sphinx: Some(true),
            html_copy_source: Some(true),
            html_show_sourcelink: Some(true),
            html_sourcelink_suffix: Some(".txt".to_string()),
            html_use_index: Some(true),
            html_use_opensearch: Some(false),
            html_last_updated_fmt: Some("%b %d, %Y".to_string()),
            templates_path: vec![PathBuf::from("_templates")],

            // Warning handling
            fail_on_warning: false,
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            html_theme: "sphinx_rtd_theme".to_string(),
            syntax_highlighting: true,
            highlight_theme: "github".to_string(),
            search_index: true,
            minify_html: false,
            compress_output: false,
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: "sphinx_rtd_theme".to_string(),
            options: serde_json::json!({}),
            custom_css: vec![],
            custom_js: vec![],
        }
    }
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            parallel_processing: true,
            incremental_builds: true,
            document_caching: true,
            image_optimization: false,
            asset_bundling: false,
        }
    }
}

impl BuildConfig {
    pub fn from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        let config = if path.extension().and_then(|s| s.to_str()) == Some("yaml")
            || path.extension().and_then(|s| s.to_str()) == Some("yml")
        {
            serde_yaml::from_str(&content)?
        } else {
            serde_json::from_str(&content)?
        };
        Ok(config)
    }

    pub fn save_to_file<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        let content = if path.as_ref().extension().and_then(|s| s.to_str()) == Some("yaml")
            || path.as_ref().extension().and_then(|s| s.to_str()) == Some("yml")
        {
            serde_yaml::to_string(self)?
        } else {
            serde_json::to_string_pretty(self)?
        };
        std::fs::write(path, content)?;
        Ok(())
    }
}
