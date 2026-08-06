use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::python_config::PythonConfigParser;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
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

    /// Glob-style patterns for file inclusion (Sphinx compatibility)
    /// Default: ["**"] (include all files)
    pub include_patterns: Vec<String>,

    /// Glob-style patterns for file exclusion (Sphinx compatibility)
    /// Default: [] (exclude nothing)
    /// Exclusions have priority over inclusions
    pub exclude_patterns: Vec<String>,

    /// Warn about all missing cross-references (Sphinx `nitpicky` / `-n`)
    pub nitpicky: bool,

    /// Tags set via `-t` (consumed by `only`/`ifconfig` once M2 lands)
    pub tags: Vec<String>,

    /// Cache/doctree directory override (Sphinx `-d`); defaults to
    /// `<output>/.sphinx-ultra-cache` when unset
    pub doctree_dir: Option<std::path::PathBuf>,

    /// Extra HTML template variables (conf.py `html_context`, CLI `-A`)
    pub html_context: std::collections::HashMap<String, serde_json::Value>,

    /// Run directive/role validation during the build
    pub validate_directives: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
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

            // File pattern matching (Sphinx compatibility)
            include_patterns: vec!["**".to_string()],
            exclude_patterns: vec![],

            nitpicky: false,
            tags: vec![],
            doctree_dir: None,
            html_context: std::collections::HashMap::new(),
            validate_directives: true,
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

        // Sphinx projects configure via conf.py; route it to the Python
        // config parser so `--config conf.py` behaves like auto-detection.
        let is_python = path.file_name().and_then(|s| s.to_str()) == Some("conf.py")
            || path.extension().and_then(|s| s.to_str()) == Some("py");
        if is_python {
            return Self::from_conf_py(path);
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read config file {}: {e}", path.display()))?;
        let config = if path.extension().and_then(|s| s.to_str()) == Some("yaml")
            || path.extension().and_then(|s| s.to_str()) == Some("yml")
        {
            serde_yaml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("invalid config file {}: {e}", path.display()))?
        } else {
            serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("invalid config file {}: {e}", path.display()))?
        };
        Ok(config)
    }

    /// Load configuration from a Sphinx conf.py file
    pub fn from_conf_py<P: AsRef<std::path::Path>>(conf_py_path: P) -> Result<Self> {
        let conf_py_path = conf_py_path.as_ref();
        let mut parser = PythonConfigParser::new()?;
        let conf_py_config = parser.parse_conf_py(conf_py_path)?;
        // Silent dropping is banned: surface every construct the parser
        // could not handle.
        for warning in parser.warnings() {
            log::warn!(
                "{}:{}: {}",
                conf_py_path.display(),
                warning.line,
                warning.message
            );
        }
        Ok(conf_py_config.to_build_config())
    }

    /// Try to auto-detect and load configuration from various sources
    pub fn auto_detect<P: AsRef<std::path::Path>>(source_dir: P) -> Result<Self> {
        let source_dir = source_dir.as_ref();

        // Try conf.py first (Sphinx standard)
        let conf_py_path = source_dir.join("conf.py");
        if conf_py_path.exists() {
            return Self::from_conf_py(conf_py_path);
        }

        // Try sphinx-ultra.yaml
        let yaml_path = source_dir.join("sphinx-ultra.yaml");
        if yaml_path.exists() {
            return Self::from_file(yaml_path);
        }

        // Try sphinx-ultra.yml
        let yml_path = source_dir.join("sphinx-ultra.yml");
        if yml_path.exists() {
            return Self::from_file(yml_path);
        }

        // Try sphinx-ultra.json
        let json_path = source_dir.join("sphinx-ultra.json");
        if json_path.exists() {
            return Self::from_file(json_path);
        }

        // Return default configuration
        Ok(Self::default())
    }

    /// Apply a `-D key=value` override (sphinx-build semantics): the value is
    /// coerced to the type the field already has, dotted keys reach the nested
    /// sections (`output.*`, `theme.*`, `optimization.*`), and an unknown key
    /// warns and is ignored rather than failing the build.
    pub fn apply_override(&mut self, key: &str, value: &str) -> Result<()> {
        // `html_theme` is the Sphinx name; it lives in two places here.
        // Fan aliases out first so both copies stay in sync.
        match key {
            "html_theme" => {
                self.apply_override("output.html_theme", value)?;
                return self.apply_override("theme.name", value);
            }
            "templates_path" => {
                self.apply_override("template_dirs", value)?;
                // fall through to set templates_path itself below
            }
            "html_static_path" => {
                self.apply_override("static_dirs", value)?;
                // fall through to set html_static_path itself below
            }
            _ => {}
        }

        let mut tree = serde_json::to_value(&*self)?;

        // Resolve the dotted path to the existing slot; unknown keys warn
        // (sphinx-build behavior) instead of erroring.
        let mut slot = &mut tree;
        for part in key.split('.') {
            match slot.get_mut(part) {
                Some(next) => slot = next,
                None => {
                    log::warn!("unknown config value '{}' in override, ignoring", key);
                    return Ok(());
                }
            }
        }

        *slot = Self::coerce_override_value(slot, key, value)?;
        *self = serde_json::from_value(tree)
            .map_err(|e| anyhow::anyhow!("invalid value for -D {}={}: {}", key, value, e))?;
        Ok(())
    }

    /// Coerce a CLI string to the JSON type currently occupying the slot.
    fn coerce_override_value(
        current: &serde_json::Value,
        key: &str,
        value: &str,
    ) -> Result<serde_json::Value> {
        use serde_json::Value;
        Ok(match current {
            Value::Bool(_) => match value {
                "1" | "true" | "True" => Value::Bool(true),
                "0" | "false" | "False" => Value::Bool(false),
                other => anyhow::bail!("invalid boolean for -D {}={}", key, other),
            },
            Value::Number(_) => value
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| value.parse::<f64>().map(Value::from))
                .map_err(|_| anyhow::anyhow!("invalid number for -D {}={}", key, value))?,
            Value::Array(_) => Value::Array(
                value
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| Value::String(s.trim().to_string()))
                    .collect(),
            ),
            // Null slots are Option<...> fields: prefer a number if the value
            // parses as one (parallel_jobs), otherwise store the string.
            Value::Null => value
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::String(value.to_string())),
            _ => Value::String(value.to_string()),
        })
    }

    #[allow(dead_code)]
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
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn minimal_yaml_loads_with_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let p = temp_dir.path().join("sphinx-ultra.yaml");
        fs::write(&p, "project: 'Tiny'\n").unwrap();

        let config = BuildConfig::from_file(&p).unwrap();
        assert_eq!(config.project, "Tiny");
        assert_eq!(config.max_cache_size_mb, 500); // default filled in
        assert_eq!(config.include_patterns, vec!["**".to_string()]);
    }

    #[test]
    fn from_file_routes_conf_py() {
        let temp_dir = TempDir::new().unwrap();
        let p = temp_dir.path().join("conf.py");
        fs::write(&p, "project = 'PyProject'\n").unwrap();

        let config = BuildConfig::from_file(&p).unwrap();
        assert_eq!(config.project, "PyProject");
    }

    #[test]
    fn shipped_yaml_examples_load() {
        for rel in ["sphinx-ultra.yaml", "examples/basic/sphinx-ultra.yaml"] {
            let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
            BuildConfig::from_file(&p).unwrap_or_else(|e| panic!("{rel} failed to load: {e}"));
        }
    }

    #[test]
    fn test_auto_detect_conf_py() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("conf.py"), "project = 'Test Project'\n").unwrap();

        let config = BuildConfig::auto_detect(root).unwrap();
        assert_eq!(config.project, "Test Project");
    }

    #[test]
    fn test_auto_detect_yaml() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let yaml_content = r#"
project: 'YAML Project'
output:
  html_theme: 'alabaster'
"#;
        fs::write(root.join("sphinx-ultra.yaml"), yaml_content).unwrap();

        let config = BuildConfig::auto_detect(root).unwrap();
        assert_eq!(config.project, "YAML Project");
    }

    #[test]
    fn test_auto_detect_default() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // No config files
        let config = BuildConfig::auto_detect(root).unwrap();
        assert_eq!(config, BuildConfig::default());
    }

    #[test]
    fn override_string_bool_number_and_list() {
        let mut config = BuildConfig::default();
        config.apply_override("project", "Custom").unwrap();
        assert_eq!(config.project, "Custom");

        config.apply_override("fail_on_warning", "1").unwrap();
        assert!(config.fail_on_warning);
        config.apply_override("fail_on_warning", "False").unwrap();
        assert!(!config.fail_on_warning);

        config.apply_override("max_cache_size_mb", "64").unwrap();
        assert_eq!(config.max_cache_size_mb, 64);

        config
            .apply_override("exclude_patterns", "drafts/**,_scratch")
            .unwrap();
        assert_eq!(
            config.exclude_patterns,
            vec!["drafts/**".to_string(), "_scratch".to_string()]
        );
    }

    #[test]
    fn override_dotted_path_reaches_nested_sections() {
        let mut config = BuildConfig::default();
        config.apply_override("output.minify_html", "true").unwrap();
        assert!(config.output.minify_html);
    }

    #[test]
    fn override_html_theme_alias_syncs_both_copies() {
        let mut config = BuildConfig::default();
        config.apply_override("html_theme", "furo").unwrap();
        assert_eq!(config.output.html_theme, "furo");
        assert_eq!(config.theme.name, "furo");
    }

    #[test]
    fn override_templates_path_syncs_template_dirs() {
        let mut config = BuildConfig::default();
        config
            .apply_override("templates_path", "_mytemplates")
            .unwrap();
        assert_eq!(config.templates_path, vec![PathBuf::from("_mytemplates")]);
        assert_eq!(config.template_dirs, vec![PathBuf::from("_mytemplates")]);
    }

    #[test]
    fn override_unknown_key_is_ignored_not_error() {
        let mut config = BuildConfig::default();
        let before = config.clone();
        config.apply_override("totally_unknown_key", "1").unwrap();
        assert_eq!(config, before);
    }

    #[test]
    fn override_option_number_field() {
        let mut config = BuildConfig::default();
        config.apply_override("parallel_jobs", "3").unwrap();
        assert_eq!(config.parallel_jobs, Some(3));
    }

    #[test]
    fn override_bad_bool_is_an_error() {
        let mut config = BuildConfig::default();
        assert!(config.apply_override("nitpicky", "maybe").is_err());
    }
}
