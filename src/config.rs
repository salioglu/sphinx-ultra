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

    /// `(reftype, target)` pairs whose missing-reference warnings `nitpicky`
    /// must not raise (`nitpick_ignore`, `config.py`). Matched exactly, with
    /// the reftype spelled either `domain:type` or — for the std domain —
    /// bare `type` (`post_transforms/__init__.py:266-273`).
    pub nitpick_ignore: Vec<(String, String)>,

    /// The same, with both halves matched as regular expressions that must
    /// match in full (`nitpick_ignore_regex`, `:274-282`).
    pub nitpick_ignore_regex: Vec<(String, String)>,

    /// Tags set via `-t` (consumed by `only`/`ifconfig` once M2 lands)
    pub tags: Vec<String>,

    /// Cache/doctree directory override (Sphinx `-d`); defaults to
    /// `<output>/.sphinx-ultra-cache` when unset
    pub doctree_dir: Option<std::path::PathBuf>,

    /// Extra HTML template variables (conf.py `html_context`, CLI `-A`).
    ///
    /// Ordered, not hashed: this struct's serialization is the cache/
    /// environment fingerprint (`builder::config_fingerprint`), and a
    /// `HashMap` would emit its entries in `RandomState` order — a digest
    /// that differs on every process, wiping the cache directory on every
    /// build for any project that sets two or more `html_context` keys.
    pub html_context: std::collections::BTreeMap<String, serde_json::Value>,

    /// Run directive/role validation during the build
    pub validate_directives: bool,

    /// Number figures, tables and code blocks (`numfig`, `config.py:275`).
    /// Off by default, exactly like Sphinx; when off,
    /// `assign_figure_numbers` assigns nothing and `:numref:` degrades.
    pub numfig: bool,

    /// Per-figtype number format (`numfig_format`, `config.py:682-693`).
    ///
    /// Sphinx seeds this with `{section: 'Section %s', figure: 'Fig. %s',
    /// table: 'Table %s', code-block: 'Listing %s'}` and **merges** the
    /// user's dict over those defaults rather than replacing them, so a
    /// `conf.py` that only overrides `figure` keeps the other three. That
    /// merge lives in [`crate::python_config::PythonConfig::to_build_config`];
    /// this field always holds the merged result, which is why
    /// [`Default`] populates it with the four defaults.
    pub numfig_format: std::collections::BTreeMap<String, String>,

    /// How many leading section numbers a figure number is scoped by
    /// (`numfig_secnum_depth`, `config.py:276`). 0 numbers figures
    /// project-globally (1, 2, 3...); 1 (the default) numbers them per
    /// top-level section (1.1, 1.2, 2.1...).
    pub numfig_secnum_depth: u32,

    /// `source_encoding` (`config.py:244`, default `'utf-8-sig'`, rebuild
    /// class `'env'` — so it enters the cache fingerprint like every other
    /// read-phase key). The encoding the file-inserting directives decode
    /// their targets with when no `:encoding:` option is given: `include`
    /// through `settings.input_encoding`, which the environment sets from
    /// this key (`environment/__init__.py:375`), and `literalinclude`
    /// through `config.source_encoding` directly (`code.py:210`). A value
    /// other than UTF-8 earns sphinx's own deprecation warning at config
    /// time ([`BuildConfig::validate`]). Documented limitation: this
    /// crate still reads its OWN source documents as UTF-8.
    pub source_encoding: String,

    /// Type mismatches `check_confval_types` (`config.py:775-847`) will
    /// report — `(key, python type name)` for the two `int | None` keys
    /// whose value arrived as some other type: a `-D` override (always
    /// `str`, because `convert_overrides` has no int branch for a key whose
    /// default is `None` and returns the raw string, `config.py:397`) or a
    /// mistyped `conf.py` assignment. Diagnostic state rather than
    /// configuration: skipped by serde, so it neither enters the cache
    /// fingerprint nor survives a save/load.
    #[serde(skip)]
    pub confval_type_mismatches: Vec<(String, String)>,

    // --- Object-signature / py-domain family (research spec §1-5, §7) ---
    //
    // The first ten keys below are rebuild category `'env'` in sphinx, i.e.
    // read-phase inputs: a change to any of them invalidates every parsed
    // document. `modindex_common_prefix` alone is `'html'` (`config.py:264`),
    // a write-phase key. All eleven still enter the build-cache fingerprint,
    // which hashes this whole struct minus
    // `builder::EXCLUDED_FROM_FINGERPRINT`: over-invalidating on the one
    // write-only key costs a rebuild, while under-invalidating on any of the
    // other ten would serve stale doctrees.
    /// `maximum_signature_line_length`, default `None` (`config.py:279-281`):
    /// the wrap threshold shared by the py/js/c/cpp object domains, behind
    /// each domain's own override. See [`crate::py::PySigConfig::max_len`]
    /// for how the two py keys combine.
    pub maximum_signature_line_length: Option<i64>,

    /// `python_maximum_signature_line_length`, default `None`
    /// (`domains/python/__init__.py:1108-1113`). An explicit `0` is *not*
    /// the same as unset: see [`crate::py::PySigConfig::max_len`].
    pub python_maximum_signature_line_length: Option<i64>,

    /// `python_trailing_comma_in_multi_line_signatures`, default `True`
    /// (`domains/python/__init__.py:1114-1119`).
    pub python_trailing_comma_in_multi_line_signatures: bool,

    /// `python_display_short_literal_types`, default `False`
    /// (`domains/python/__init__.py:1120-1122`).
    pub python_display_short_literal_types: bool,

    /// `python_use_unqualified_type_names`, default `False`
    /// (`domains/python/__init__.py:1105-1107`).
    pub python_use_unqualified_type_names: bool,

    /// `toc_object_entries`, default `True` (`config.py:250`).
    pub toc_object_entries: bool,

    /// `toc_object_entries_show_parents`, default `'domain'`, an
    /// `ENUM('domain', 'all', 'hide')` (`config.py:251-253`). Stored as the
    /// raw string because sphinx only *warns* about a value outside the
    /// enum and keeps it — see [`BuildConfig::validate`].
    pub toc_object_entries_show_parents: String,

    /// `add_function_parentheses`, default `True` (`config.py:248`) — the
    /// `fix_parens` roles (`:py:func:`, `:py:meth:`) append `()` to an
    /// implicit title, and object descriptions do the same for `_toc_name`.
    pub add_function_parentheses: bool,

    /// `add_module_names`, default `True` (`config.py:249`): whether a
    /// signature renders its module prefix.
    pub add_module_names: bool,

    /// `strip_signature_backslash`, default `False`
    /// (`directives/__init__.py:370-372`): strip backslashes out of a
    /// signature before it is measured and parsed.
    pub strip_signature_backslash: bool,

    /// `modindex_common_prefix`, default `[]` (`config.py:264`): module-name
    /// prefixes the python module index ignores when sorting. The one
    /// `'html'`-rebuild key in this family.
    pub modindex_common_prefix: Vec<String>,

    /// `intersphinx_mapping`, already normalised and validated
    /// (`ext/intersphinx/_load.py:38-136`): project name -> (target URI,
    /// inventory locations). Loading a `conf.py` whose mapping fails
    /// validation is an error, exactly as Sphinx's `ConfigError` aborts the
    /// build — see [`crate::intersphinx::validate_mapping`].
    pub intersphinx_mapping: crate::intersphinx::IntersphinxMapping,

    /// `intersphinx_disabled_reftypes`, default `['std:doc']`
    /// (`ext/intersphinx/__init__.py:79`). Entries are `domain:objtype`,
    /// `domain:*` or `*`, and they only ever block a *bare* reference: the
    /// `inv:target` and `:external:` forms bypass them.
    pub intersphinx_disabled_reftypes: Vec<String>,

    /// `intersphinx_resolve_self`, default `''` (`__init__.py:69`): the
    /// inventory name that means "this project", so `name:target` resolves
    /// locally instead of through an inventory.
    pub intersphinx_resolve_self: String,

    /// `intersphinx_cache_limit` in days, default 5 (`__init__.py:70`).
    /// Negative means a cached inventory never expires.
    pub intersphinx_cache_limit: i64,

    /// `intersphinx_timeout` in seconds, default `None` — which Sphinx
    /// passes to `requests` as no timeout at all (`__init__.py:71`).
    pub intersphinx_timeout: Option<f64>,

    /// `tls_verify`, default `True` (`config.py:286`).
    pub tls_verify: bool,

    /// `tls_cacerts`, default `None` (`config.py:287`): one CA bundle path,
    /// or a per-host mapping of them.
    pub tls_cacerts: Option<crate::intersphinx::TlsCacerts>,

    /// `user_agent`, default `None` (`config.py:288`) — unset means
    /// [`crate::intersphinx::DEFAULT_USER_AGENT`].
    pub user_agent: Option<String>,
}

/// Sphinx's default `source_encoding` (`config.py:244`).
pub const DEFAULT_SOURCE_ENCODING: &str = crate::rst::DEFAULT_SOURCE_ENCODING;

/// The config keys registered with a `None` default and `int | NoneType`
/// as their valid types (`config.py:279-281`,
/// `domains/python/__init__.py:1108-1113`), in sphinx's registration
/// order — the order `check_confval_types` reports them in.
const NONE_DEFAULT_INT_KEYS: [&str; 2] = [
    "maximum_signature_line_length",
    "python_maximum_signature_line_length",
];

/// `deprecate_source_encoding` (`config.py:886-896`, a `config-inited`
/// handler at priority 790): the encodings it does NOT warn about.
const UTF8_SPELLINGS: [&str; 3] = ["utf-8", "utf-8-sig", "utf8"];

/// The three values `toc_object_entries_show_parents` accepts —
/// `ENUM('domain', 'all', 'hide')` (`config.py:251-253`), in sphinx's own
/// registration order.
pub const TOC_OBJECT_ENTRIES_SHOW_PARENTS: [&str; 3] = ["domain", "all", "hide"];

/// Sphinx's `numfig_format` defaults (`config.py:682-693`), which user
/// entries merge over.
pub fn default_numfig_format() -> std::collections::BTreeMap<String, String> {
    [
        ("section", "Section %s"),
        ("figure", "Fig. %s"),
        ("table", "Table %s"),
        ("code-block", "Listing %s"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
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
            // Sphinx defaults both values to the empty string when conf.py
            // does not define them. Keep None here so artifact headers use
            // that same default while explicit conf.py values remain intact.
            version: None,
            release: None,
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
            nitpick_ignore: vec![],
            nitpick_ignore_regex: vec![],
            tags: vec![],
            doctree_dir: None,
            html_context: std::collections::BTreeMap::new(),
            validate_directives: true,

            numfig: false,
            numfig_format: default_numfig_format(),
            numfig_secnum_depth: 1,
            source_encoding: DEFAULT_SOURCE_ENCODING.to_string(),
            confval_type_mismatches: Vec::new(),

            // Object-signature / py-domain family, probe-verified against
            // sphinx 9.1.0 (task-2 brief, "Probe outcomes").
            maximum_signature_line_length: None,
            python_maximum_signature_line_length: None,
            python_trailing_comma_in_multi_line_signatures: true,
            python_display_short_literal_types: false,
            python_use_unqualified_type_names: false,
            toc_object_entries: true,
            toc_object_entries_show_parents: "domain".to_string(),
            add_function_parentheses: true,
            add_module_names: true,
            strip_signature_backslash: false,
            modindex_common_prefix: Vec::new(),

            intersphinx_mapping: Default::default(),
            intersphinx_disabled_reftypes: vec!["std:doc".to_string()],
            intersphinx_resolve_self: String::new(),
            intersphinx_cache_limit: 5,
            intersphinx_timeout: None,
            tls_verify: true,
            tls_cacerts: None,
            user_agent: None,
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
        conf_py_config.to_build_config()
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

    /// Sphinx's `check_confval_types` pass, which runs once at
    /// `config-inited` — after `conf.py` *and* after every `-D` override —
    /// and reports values outside a setting's declared type or enum.
    ///
    /// It **warns**; it does not fail. A rejected value is left in place and
    /// the build carries on with it (probe E of the task-2 brief:
    /// `-D toc_object_entries_show_parents=bogus` builds successfully with
    /// `config.toc_object_entries_show_parents == 'bogus'`). Returns the
    /// warning texts so the caller can log them, write them to `-w`, and
    /// count them toward `-W`, like every other config-time warning.
    ///
    /// Sphinx renders the candidate set as a python `frozenset` repr, whose
    /// element order is hash-order and therefore varies between processes
    /// (verified: three runs, three orders). The registration order is used
    /// here instead, which is the only deterministic choice.
    ///
    /// The `config-inited` handlers run in priority order, which fixes the
    /// order of the warnings: `deprecate_source_encoding` (790) before
    /// `check_confval_types` (800), and inside the latter the options in
    /// registration order — `toc_object_entries_show_parents`
    /// (`config.py:251`) before `maximum_signature_line_length`
    /// (`config.py:279`) before the py domain's
    /// `python_maximum_signature_line_length`. Each message is logged
    /// `once=True`, so a key is reported at most once.
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        // config-inited @790: `deprecate_source_encoding`. Byte-exact.
        if !UTF8_SPELLINGS.contains(&self.source_encoding.to_lowercase().as_str()) {
            warnings.push(
                "Support for source encodings other than UTF-8 is deprecated and will be \
                 removed in Sphinx 10. Please comment at \
                 https://github.com/sphinx-doc/sphinx/issues/13665 if this causes a problem."
                    .to_string(),
            );
        }
        // This crate's own check (sphinx has none — it raises `LookupError`
        // at the first file it opens): a codec outside the include
        // directives' table cannot be decoded here, and the parser falls
        // back to the default rather than mis-decoding silently.
        if !crate::rst::block::is_supported_encoding(&self.source_encoding) {
            warnings.push(format!(
                "source_encoding '{}' is not an encoding sphinx-ultra can decode \
                 (utf-8, utf-8-sig, ascii, latin-1); included files will be read as \
                 '{DEFAULT_SOURCE_ENCODING}'",
                self.source_encoding
            ));
        }

        // config-inited @800: `check_confval_types`, in registration order.
        if !TOC_OBJECT_ENTRIES_SHOW_PARENTS.contains(&self.toc_object_entries_show_parents.as_str())
        {
            let candidates = TOC_OBJECT_ENTRIES_SHOW_PARENTS
                .iter()
                .map(|value| format!("'{value}'"))
                .collect::<Vec<_>>()
                .join(", ");
            warnings.push(format!(
                "The config value `toc_object_entries_show_parents` has to be a one of \
                 frozenset({{{candidates}}}), but `{}` is given.",
                self.toc_object_entries_show_parents
            ));
        }
        // The type-mismatch branch (`config.py:822-838`): `type_value` is not
        // in `{int, NoneType}` and shares no non-trivial base with NoneType,
        // so the warning names the permitted set, `sorted` by the
        // backticked spelling — `NoneType' before `int' (N < i).
        for key in NONE_DEFAULT_INT_KEYS {
            if let Some((_, type_name)) = self
                .confval_type_mismatches
                .iter()
                .find(|(mismatched, _)| mismatched == key)
            {
                warnings.push(format!(
                    "The config value `{key}' has type `{type_name}'; expected `NoneType' or \
                     `int'."
                ));
            }
        }
        warnings
    }

    /// Record that `key` (one of [`NONE_DEFAULT_INT_KEYS`]) received a value
    /// of python type `type_name`, for [`Self::validate`] to report. One
    /// entry per key, like sphinx's `once=True`.
    pub fn note_confval_type_mismatch(&mut self, key: &str, type_name: &str) {
        if !self
            .confval_type_mismatches
            .iter()
            .any(|(mismatched, _)| mismatched == key)
        {
            self.confval_type_mismatches
                .push((key.to_string(), type_name.to_string()));
        }
    }

    /// Apply a `-D key=value` override (sphinx-build semantics): the value is
    /// coerced to the type the field already has, dotted keys reach the nested
    /// sections (`output.*`, `theme.*`) and map-typed settings
    /// (`html_context.name`), and an unknown key warns and is ignored rather
    /// than failing the build.
    ///
    /// Returns the sphinx-style warning message when the override was ignored
    /// — the caller decides how to report it (it must count toward `-W`).
    pub fn apply_override(&mut self, key: &str, value: &str) -> Result<Option<String>> {
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

        // `convert_overrides` (`config.py:354-399`) has no `int` branch for
        // a key whose default is `None`: control reaches `isinstance(default,
        // str) or default is None: return value`, so the value stays the
        // raw STRING, and `check_confval_types` then warns that it has type
        // `str'. Sphinx keeps that string — and the first py signature
        // raises `TypeError: '>' not supported between instances of 'int'
        // and 'str'` at `_object.py:304` (probe-pinned, panel fix round B)
        // — so "unset" is the only value this crate can sanely carry. The
        // warning itself is reported by [`Self::validate`], at
        // `config-inited` like sphinx's.
        if NONE_DEFAULT_INT_KEYS.contains(&key) {
            match key {
                "maximum_signature_line_length" => self.maximum_signature_line_length = None,
                _ => self.python_maximum_signature_line_length = None,
            }
            self.note_confval_type_mismatch(key, "str");
            return Ok(None);
        }

        let mut tree = serde_json::to_value(&*self)?;

        // Resolve the dotted path. A key missing from its parent object is
        // inserted as Null (map-typed settings like html_context accept new
        // keys); whether it truly landed is checked after the round-trip —
        // structs silently drop unknown fields, which we report as unknown.
        let mut slot = &mut tree;
        for part in key.split('.') {
            slot = match slot {
                serde_json::Value::Object(map) => map
                    .entry(part.to_string())
                    .or_insert(serde_json::Value::Null),
                _ => {
                    return Ok(Some(format!(
                        "unknown config value '{}' in override, ignoring",
                        key
                    )))
                }
            };
        }

        // Whole-dict overrides are not expressible on the command line
        // (sphinx-build warns and continues too).
        if slot.is_object() {
            return Ok(Some(format!(
                "cannot override dictionary config setting '{}', ignoring (use -D {}.key=value)",
                key, key
            )));
        }

        let coerced = Self::coerce_override_value(slot, key, value)?;
        let retry_as_string = matches!(coerced, serde_json::Value::Number(_))
            && matches!(slot, serde_json::Value::Null);
        *slot = coerced;

        let mut applied: Self = match serde_json::from_value(tree.clone()) {
            Ok(config) => config,
            // A Null slot gave no type information and the numeric guess was
            // wrong (e.g. -D html_title=2024 targets an Option<String>):
            // retry with the raw string before giving up.
            Err(first_err) => {
                if retry_as_string {
                    let mut retry_tree = tree;
                    let mut retry_slot = &mut retry_tree;
                    for part in key.split('.') {
                        retry_slot = retry_slot.get_mut(part).expect("path resolved above");
                    }
                    *retry_slot = serde_json::Value::String(value.to_string());
                    serde_json::from_value(retry_tree).map_err(|e| {
                        anyhow::anyhow!("invalid value for -D {}={}: {}", key, value, e)
                    })?
                } else {
                    return Err(anyhow::anyhow!(
                        "invalid value for -D {}={}: {}",
                        key,
                        value,
                        first_err
                    ));
                }
            }
        };

        // Did the key survive the round-trip? Structs drop unknown fields
        // silently; a vanished key means the setting doesn't exist.
        let check = serde_json::to_value(&applied)?;
        let mut probe = Some(&check);
        for part in key.split('.') {
            probe = probe.and_then(|v| v.get(part));
        }
        if probe.is_none() {
            return Ok(Some(format!(
                "unknown config value '{}' in override, ignoring",
                key
            )));
        }

        // Serde-skipped state does not survive the round trip; carry it.
        applied.confval_type_mismatches = std::mem::take(&mut self.confval_type_mismatches);
        *self = applied;
        Ok(None)
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
            // parses as one (parallel_jobs, intersphinx_timeout), otherwise
            // store the string. A wrong numeric guess is retried as a string
            // by the caller, so trying the fractional form costs nothing and
            // is the only way to reach an `Option<f64>` setting.
            Value::Null => value
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| value.parse::<f64>().map(Value::from))
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
        let warning = config.apply_override("totally_unknown_key", "1").unwrap();
        assert_eq!(config, before);
        assert!(warning.unwrap().contains("unknown config value"));

        // Unknown nested keys are dropped by the struct round-trip and
        // reported the same way.
        let warning = config.apply_override("output.bogus_knob", "1").unwrap();
        assert_eq!(config, before);
        assert!(warning.unwrap().contains("unknown config value"));
    }

    #[test]
    fn override_option_number_field() {
        let mut config = BuildConfig::default();
        assert!(config
            .apply_override("parallel_jobs", "3")
            .unwrap()
            .is_none());
        assert_eq!(config.parallel_jobs, Some(3));
    }

    #[test]
    fn override_bad_bool_is_an_error() {
        let mut config = BuildConfig::default();
        assert!(config.apply_override("nitpicky", "maybe").is_err());
    }

    #[test]
    fn override_numeric_value_for_unset_string_option_stays_a_string() {
        // sphinx-build sets html_title="2024"; the numeric guess for the
        // Null slot must fall back to a string instead of failing the build.
        let mut config = BuildConfig::default();
        assert!(config
            .apply_override("html_title", "2024")
            .unwrap()
            .is_none());
        assert_eq!(config.html_title, Some("2024".to_string()));
    }

    #[test]
    fn override_dict_member_and_whole_dict() {
        let mut config = BuildConfig::default();

        // -D html_context.banner=on inserts into the map (sphinx-build syntax)
        assert!(config
            .apply_override("html_context.banner", "on")
            .unwrap()
            .is_none());
        assert_eq!(
            config.html_context.get("banner"),
            Some(&serde_json::Value::String("on".to_string()))
        );

        // A whole-dict override warns and is ignored, like sphinx-build
        let before = config.clone();
        let warning = config.apply_override("html_context", "x").unwrap();
        assert_eq!(config, before);
        assert!(warning
            .unwrap()
            .contains("cannot override dictionary config setting"));
    }

    #[test]
    fn intersphinx_and_http_defaults_match_sphinx() {
        let config = BuildConfig::default();
        assert!(config.intersphinx_mapping.is_empty());
        assert_eq!(
            config.intersphinx_disabled_reftypes,
            vec!["std:doc".to_string()],
            "the one default entry is what stops a bare `:doc:` resolving externally"
        );
        assert_eq!(config.intersphinx_resolve_self, "");
        assert_eq!(config.intersphinx_cache_limit, 5);
        assert_eq!(config.intersphinx_timeout, None);
        assert!(config.tls_verify);
        assert_eq!(config.tls_cacerts, None);
        assert_eq!(config.user_agent, None);
    }

    #[test]
    fn an_invalid_intersphinx_mapping_fails_configuration_loading() {
        // Sphinx raises ConfigError here, which aborts the build before it
        // starts; the CLI turns a config-loading error into exit code 2.
        let temp_dir = TempDir::new().unwrap();
        let p = temp_dir.path().join("conf.py");
        fs::write(
            &p,
            "intersphinx_mapping = {'a': ('https://x/', None), 'b': ('https://x/', None)}\n",
        )
        .unwrap();

        let err = BuildConfig::from_file(&p).expect_err("a duplicate target URI must abort");
        assert_eq!(
            err.to_string(),
            "Invalid `intersphinx_mapping` configuration (1 error)."
        );
    }

    #[test]
    fn intersphinx_scalars_are_overridable_from_the_command_line() {
        let mut config = BuildConfig::default();
        assert!(config
            .apply_override("intersphinx_cache_limit", "-1")
            .unwrap()
            .is_none());
        assert_eq!(config.intersphinx_cache_limit, -1);

        assert!(config
            .apply_override("intersphinx_disabled_reftypes", "std:doc,std:label")
            .unwrap()
            .is_none());
        assert_eq!(
            config.intersphinx_disabled_reftypes,
            vec!["std:doc".to_string(), "std:label".to_string()]
        );

        assert!(config.apply_override("tls_verify", "0").unwrap().is_none());
        assert!(!config.tls_verify);

        // An unset `Option<f64>`: the slot carries no type information, so
        // the fractional form has to be guessed at.
        assert!(config
            .apply_override("intersphinx_timeout", "2.5")
            .unwrap()
            .is_none());
        assert_eq!(config.intersphinx_timeout, Some(2.5));
        assert!(config
            .apply_override("intersphinx_timeout", "5")
            .unwrap()
            .is_none());
        assert_eq!(config.intersphinx_timeout, Some(5.0));
    }

    #[test]
    fn numfig_defaults_match_sphinx() {
        let config = BuildConfig::default();
        assert!(!config.numfig);
        assert_eq!(config.numfig_secnum_depth, 1);
        assert_eq!(config.numfig_format["section"], "Section %s");
        assert_eq!(config.numfig_format["figure"], "Fig. %s");
        assert_eq!(config.numfig_format["table"], "Table %s");
        assert_eq!(config.numfig_format["code-block"], "Listing %s");
    }

    /// Probe D of the task-2 brief dumped `app.config` for all eleven keys
    /// under sphinx 9.1.0; these are those values.
    #[test]
    fn object_signature_and_py_domain_defaults_match_sphinx() {
        let config = BuildConfig::default();
        assert_eq!(config.maximum_signature_line_length, None);
        assert_eq!(config.python_maximum_signature_line_length, None);
        assert!(config.python_trailing_comma_in_multi_line_signatures);
        assert!(!config.python_display_short_literal_types);
        assert!(!config.python_use_unqualified_type_names);
        assert!(config.toc_object_entries);
        assert_eq!(config.toc_object_entries_show_parents, "domain");
        assert!(config.add_function_parentheses);
        assert!(config.add_module_names);
        assert!(!config.strip_signature_backslash);
        assert!(config.modindex_common_prefix.is_empty());
    }

    /// Sphinx's `convert_overrides` has no `int` branch for a key whose
    /// default is `None` (`config.py:354-399` ends in `isinstance(default,
    /// str) or default is None: return value`), so `-D` hands
    /// `check_confval_types` the raw STRING and it warns — for `20`, `0`,
    /// `abc` and `None` alike. Probed on the pinned toolchain (panel fix
    /// round B, [18]): the warning fires with no py directive in the
    /// project, `-W` exits 1, and with a `.. py:function::` present sphinx
    /// then crashes (`TypeError: '>' not supported between instances of
    /// 'int' and 'str'`, `_object.py:304`). This crate warns byte-exactly
    /// and leaves the key UNSET, the only value it can sanely carry.
    #[test]
    fn a_none_default_int_key_overridden_from_the_command_line_warns_like_sphinx() {
        for value in ["20", "0", "abc", "None"] {
            let mut config = BuildConfig {
                maximum_signature_line_length: Some(60),
                ..Default::default()
            };
            assert!(config
                .apply_override("maximum_signature_line_length", value)
                .unwrap()
                .is_none());
            assert_eq!(
                config.maximum_signature_line_length, None,
                "{value}: never coerced, never kept as the old number"
            );
            assert!(config
                .apply_override("python_maximum_signature_line_length", value)
                .unwrap()
                .is_none());
            assert_eq!(config.python_maximum_signature_line_length, None);
            assert_eq!(
                config.validate(),
                vec![
                    "The config value `maximum_signature_line_length' has type `str'; \
                     expected `NoneType' or `int'."
                        .to_string(),
                    "The config value `python_maximum_signature_line_length' has type `str'; \
                     expected `NoneType' or `int'."
                        .to_string(),
                ],
                "{value}"
            );
        }

        // `once=True`: a key overridden twice is reported once, and a
        // later ordinary override keeps the record through the round trip.
        let mut config = BuildConfig::default();
        config
            .apply_override("maximum_signature_line_length", "1")
            .unwrap();
        config
            .apply_override("maximum_signature_line_length", "2")
            .unwrap();
        config.apply_override("nitpicky", "1").unwrap();
        assert_eq!(config.validate().len(), 1);

        // Registration order: the ENUM key (`config.py:251`) is reported
        // before `maximum_signature_line_length` (`config.py:279`).
        let mut config = BuildConfig::default();
        config
            .apply_override("maximum_signature_line_length", "20")
            .unwrap();
        config
            .apply_override("toc_object_entries_show_parents", "bogus")
            .unwrap();
        let warnings = config.validate();
        assert!(warnings[0].contains("toc_object_entries_show_parents"));
        assert!(warnings[1].contains("maximum_signature_line_length"));
    }

    /// `source_encoding` (`config.py:244`): default `'utf-8-sig'`,
    /// overridable, a non-UTF-8 value earns sphinx's deprecation text
    /// (byte-exact, probed: `deprecate_source_encoding` at config-inited
    /// priority 790, i.e. BEFORE the type checks), and a codec this crate
    /// cannot decode earns this crate's own fallback notice on top.
    #[test]
    fn source_encoding_is_a_real_key_with_sphinxs_deprecation_warning() {
        let config = BuildConfig::default();
        assert_eq!(config.source_encoding, "utf-8-sig");
        assert!(config.validate().is_empty());

        let deprecation = "Support for source encodings other than UTF-8 is deprecated and \
                           will be removed in Sphinx 10. Please comment at \
                           https://github.com/sphinx-doc/sphinx/issues/13665 if this causes \
                           a problem.";
        for quiet in ["utf-8", "UTF-8", "utf8", "utf-8-sig", "UTF-8-SIG"] {
            let mut config = BuildConfig::default();
            assert!(config
                .apply_override("source_encoding", quiet)
                .unwrap()
                .is_none());
            assert_eq!(config.source_encoding, quiet);
            assert!(config.validate().is_empty(), "{quiet}");
        }

        let mut config = BuildConfig::default();
        config.apply_override("source_encoding", "latin-1").unwrap();
        assert_eq!(config.validate(), vec![deprecation.to_string()]);

        let mut config = BuildConfig::default();
        config.apply_override("source_encoding", "cp1252").unwrap();
        config
            .apply_override("maximum_signature_line_length", "20")
            .unwrap();
        let warnings = config.validate();
        assert_eq!(warnings.len(), 3, "{warnings:#?}");
        assert_eq!(warnings[0], deprecation);
        assert_eq!(
            warnings[1],
            "source_encoding 'cp1252' is not an encoding sphinx-ultra can decode (utf-8, \
             utf-8-sig, ascii, latin-1); included files will be read as 'utf-8-sig'"
        );
        assert!(warnings[2].starts_with("The config value `maximum_signature_line_length'"));
    }

    #[test]
    fn object_signature_family_is_overridable_from_the_command_line() {
        let mut config = BuildConfig::default();

        for key in [
            "python_trailing_comma_in_multi_line_signatures",
            "python_display_short_literal_types",
            "python_use_unqualified_type_names",
            "toc_object_entries",
            "add_function_parentheses",
            "add_module_names",
            "strip_signature_backslash",
        ] {
            assert!(config.apply_override(key, "0").unwrap().is_none(), "{key}");
            assert!(config.apply_override(key, "1").unwrap().is_none(), "{key}");
        }
        assert!(config.python_trailing_comma_in_multi_line_signatures);
        assert!(config.add_function_parentheses);
        assert!(config.strip_signature_backslash);

        assert!(config
            .apply_override("toc_object_entries_show_parents", "hide")
            .unwrap()
            .is_none());
        assert_eq!(config.toc_object_entries_show_parents, "hide");

        assert!(config
            .apply_override("modindex_common_prefix", "mypkg.,other.")
            .unwrap()
            .is_none());
        assert_eq!(
            config.modindex_common_prefix,
            vec!["mypkg.".to_string(), "other.".to_string()]
        );
    }

    /// `toc_object_entries_show_parents` is `ENUM('domain', 'all', 'hide')`
    /// (`config.py:251-253`), and sphinx's `check_confval_types` only
    /// **warns** about a value outside it — the build continues with the
    /// offending value untouched (probe E, recorded in the task-2 brief).
    /// So `validate` returns warnings and never fails.
    #[test]
    fn an_out_of_enum_toc_show_parents_warns_and_is_kept() {
        for accepted in ["domain", "all", "hide"] {
            let mut config = BuildConfig::default();
            config
                .apply_override("toc_object_entries_show_parents", accepted)
                .unwrap();
            assert!(
                config.validate().is_empty(),
                "{accepted} is one of the three ENUM values"
            );
        }

        let mut config = BuildConfig::default();
        config
            .apply_override("toc_object_entries_show_parents", "bogus")
            .unwrap();
        let warnings = config.validate();
        assert_eq!(
            warnings,
            vec![
                "The config value `toc_object_entries_show_parents` has to be a one of \
                 frozenset({'domain', 'all', 'hide'}), but `bogus` is given."
                    .to_string()
            ]
        );
        assert_eq!(
            config.toc_object_entries_show_parents, "bogus",
            "sphinx keeps the rejected value rather than resetting it"
        );

        // The comparison is case-sensitive, exactly like a python set test.
        let mut config = BuildConfig::default();
        config
            .apply_override("toc_object_entries_show_parents", "Domain")
            .unwrap();
        assert_eq!(config.validate().len(), 1);
    }

    #[test]
    fn numfig_family_is_overridable_from_the_command_line() {
        let mut config = BuildConfig::default();

        // sphinx-build spells booleans as 0/1; `true`/`True` work too.
        assert!(config.apply_override("numfig", "1").unwrap().is_none());
        assert!(config.numfig);
        assert!(config.apply_override("numfig", "0").unwrap().is_none());
        assert!(!config.numfig);
        assert!(config.apply_override("numfig", "true").unwrap().is_none());
        assert!(config.numfig);
        assert!(config.apply_override("numfig", "yes").is_err());

        assert!(config
            .apply_override("numfig_secnum_depth", "2")
            .unwrap()
            .is_none());
        assert_eq!(config.numfig_secnum_depth, 2);

        // A dict setting is overridden key by key, which leaves the other
        // defaults in place.
        assert!(config
            .apply_override("numfig_format.figure", "Figure %s")
            .unwrap()
            .is_none());
        assert_eq!(config.numfig_format["figure"], "Figure %s");
        assert_eq!(config.numfig_format["table"], "Table %s");
    }
}
