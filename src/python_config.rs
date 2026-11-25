use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::config::BuildConfig;

/// Python configuration parser for conf.py files.
///
/// This is a *parser*, not an executor: it handles the declarative subset of
/// Python used by typical conf.py files (assignments of literals, including
/// multi-line lists/dicts/tuples, string concatenation, and triple-quoted
/// strings). Every construct it cannot handle produces a [`ConfigWarning`] —
/// silent dropping is banned. Full execution arrives with the Python sidecar
/// (ROADMAP M5).
pub struct PythonConfigParser {
    conf_namespace: HashMap<String, serde_json::Value>,
    warnings: Vec<ConfigWarning>,
    /// For a key of [`MUST_EVALUATE`], the first statement since the last
    /// one this reader evaluated for it that binds it to something it
    /// cannot evaluate: at most one per key.
    unevaluable: Vec<Unevaluable>,
}

/// The keys whose `conf.py` value this reader must evaluate or fail the
/// configuration with (wave 5a T17b): Sphinx reads `rst_prolog` and
/// `rst_epilog` around every document, so a value dropped would read every
/// document differently.
const MUST_EVALUATE: [&str; 2] = ["rst_prolog", "rst_epilog"];

/// A conf.py construct that could not be parsed and was dropped.
#[derive(Debug, Clone)]
pub struct ConfigWarning {
    /// 1-based line in conf.py where the construct starts.
    pub line: usize,
    pub message: String,
}

/// A statement binding `rst_prolog` or `rst_epilog` that this reader
/// cannot evaluate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unevaluable {
    /// The key the statement binds.
    pub key: &'static str,
    /// 1-based line in conf.py where the statement starts.
    pub line: usize,
    /// Why the reader cannot evaluate it.
    pub reason: String,
}

/// Represents a parsed conf.py configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfPyConfig {
    // Project information
    pub project: Option<String>,
    pub version: Option<String>,
    pub release: Option<String>,
    /// `today` (`config.py:227`), `None` when conf.py said nothing.
    pub today: Option<String>,
    /// `today_fmt` (`config.py:229`), `None` when conf.py said nothing or
    /// set it to `None`, both of which leave Sphinx's default.
    pub today_fmt: Option<String>,
    pub copyright: Option<String>,
    pub author: Option<String>,

    // General configuration
    pub extensions: Vec<String>,
    pub templates_path: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub include_patterns: Vec<String>,
    pub source_suffix: HashMap<String, String>,
    pub root_doc: Option<String>,
    pub language: Option<String>,
    pub locale_dirs: Vec<String>,
    pub gettext_compact: Option<bool>,

    // HTML output options
    pub html_theme: Option<String>,
    pub html_theme_options: HashMap<String, serde_json::Value>,
    pub html_title: Option<String>,
    pub html_short_title: Option<String>,
    pub html_logo: Option<String>,
    pub html_favicon: Option<String>,
    pub html_css_files: Vec<String>,
    pub html_js_files: Vec<String>,
    pub html_static_path: Vec<String>,
    pub html_extra_path: Vec<String>,
    pub html_use_index: Option<bool>,
    pub html_split_index: Option<bool>,
    pub html_copy_source: Option<bool>,
    pub html_show_sourcelink: Option<bool>,
    pub html_sourcelink_suffix: Option<String>,
    pub html_use_opensearch: Option<String>,
    pub html_file_suffix: Option<String>,
    pub html_link_suffix: Option<String>,
    pub html_show_copyright: Option<bool>,
    pub html_show_sphinx: Option<bool>,
    pub html_context: HashMap<String, serde_json::Value>,
    pub html_output_encoding: Option<String>,
    pub html_compact_lists: Option<bool>,
    pub html_secnumber_suffix: Option<String>,
    pub html_search_language: Option<String>,
    pub html_search_options: HashMap<String, serde_json::Value>,
    pub html_search_scorer: Option<String>,
    pub html_scaled_image_link: Option<bool>,
    pub html_baseurl: Option<String>,
    pub html_codeblock_linenos_style: Option<String>,
    pub html_math_renderer: Option<String>,
    pub html_math_renderer_options: HashMap<String, serde_json::Value>,

    // LaTeX output options
    pub latex_engine: Option<String>,
    pub latex_documents: Vec<(String, String, String, String, String)>,
    pub latex_logo: Option<String>,
    pub latex_appendices: Vec<String>,
    pub latex_domain_indices: Option<bool>,
    pub latex_show_pagerefs: Option<bool>,
    pub latex_show_urls: Option<String>,
    pub latex_use_latex_multicolumn: Option<bool>,
    pub latex_use_xindy: Option<bool>,
    pub latex_toplevel_sectioning: Option<String>,
    pub latex_docclass: HashMap<String, String>,
    pub latex_additional_files: Vec<String>,
    pub latex_elements: HashMap<String, String>,

    // ePub output options
    pub epub_title: Option<String>,
    pub epub_author: Option<String>,
    pub epub_language: Option<String>,
    pub epub_publisher: Option<String>,
    pub epub_copyright: Option<String>,
    pub epub_identifier: Option<String>,
    pub epub_scheme: Option<String>,
    pub epub_uid: Option<String>,
    pub epub_cover: Option<(String, String)>,
    pub epub_css_files: Vec<String>,
    pub epub_pre_files: Vec<(String, String)>,
    pub epub_post_files: Vec<(String, String)>,
    pub epub_exclude_files: Vec<String>,
    pub epub_tocdepth: Option<i32>,
    pub epub_tocdup: Option<bool>,
    pub epub_tocscope: Option<String>,
    pub epub_fix_images: Option<bool>,
    pub epub_max_image_width: Option<i32>,
    pub epub_show_urls: Option<String>,
    pub epub_use_index: Option<bool>,
    pub epub_description: Option<String>,
    pub epub_contributor: Option<String>,
    pub epub_writing_mode: Option<String>,

    // Extension-specific configurations
    pub extension_configs: HashMap<String, HashMap<String, serde_json::Value>>,

    // Build options
    pub needs_sphinx: Option<String>,
    pub needs_extensions: HashMap<String, String>,
    pub manpages_url: Option<String>,
    pub nitpicky: Option<bool>,
    pub nitpick_ignore: Vec<(String, String)>,
    pub nitpick_ignore_regex: Vec<(String, String)>,
    pub numfig: Option<bool>,
    pub numfig_format: HashMap<String, String>,
    pub numfig_secnum_depth: Option<i32>,
    pub math_number_all: Option<bool>,
    pub math_eqref_format: Option<String>,
    pub math_numfig: Option<bool>,
    pub tls_verify: Option<bool>,
    pub tls_cacerts: Option<crate::intersphinx::TlsCacerts>,
    pub user_agent: Option<String>,

    // The warning stream (`config.py:262-263`).
    /// `suppress_warnings` as written (a list or a tuple); empty when
    /// conf.py said nothing, which is also Sphinx's default.
    pub suppress_warnings: Vec<String>,
    /// `show_warning_types`, `None` when conf.py said nothing.
    pub show_warning_types: Option<bool>,
    /// `keep_warnings` (`config.py:261`), `None` when conf.py said nothing.
    pub keep_warnings: Option<bool>,
    /// `trim_doctest_flags` (`config.py:267`), `None` when conf.py said
    /// nothing.
    pub trim_doctest_flags: Option<bool>,

    // Smart quotes (`config.py:289-295`).
    /// `smartquotes`: `Some` for a conf.py bool only. `SphinxSmartQuotes`
    /// tests the value with `is False`
    /// (`SP/sphinx/transforms/__init__.py:389`), so any other value
    /// (`0`, `None`, `''`) leaves Sphinx's `True` in place.
    pub smartquotes: Option<bool>,
    /// `smartquotes_action`, `None` when conf.py said nothing.
    pub smartquotes_action: Option<String>,
    /// `smartquotes_excludes`, `None` when conf.py said nothing; a dict is
    /// read key by key, as `.get(key, [])` reads it.
    pub smartquotes_excludes: Option<crate::config::SmartquotesExcludes>,

    // The read-side keys (`config.py:257`, `:268`).
    /// `primary_domain`: `None` when conf.py said nothing, `Some(None)` for
    /// an explicit `primary_domain = None`.
    pub primary_domain: Option<Option<String>>,
    /// `highlight_language`, `None` when conf.py said nothing.
    pub highlight_language: Option<String>,
    /// `default_role`: `None` when conf.py said nothing, `Some(None)` for
    /// an explicit `default_role = None`.
    pub default_role: Option<Option<String>>,
    /// `math_numsep`, `None` when conf.py said nothing.
    pub math_numsep: Option<String>,
    /// `rst_prolog` (`config.py:266`): `None` when conf.py said nothing or
    /// set it to `None`, which Sphinx's reader reads alike. A value this
    /// reader cannot evaluate fails the parse
    /// ([`PythonConfigParser::parse_conf_py`]).
    pub rst_prolog: Option<String>,
    /// `rst_epilog` (`config.py:265`), read as `rst_prolog` is.
    pub rst_epilog: Option<String>,

    // Object-signature / py-domain family. `None` means "conf.py did not
    // mention it", which is what keeps sphinx's own default in place —
    // notably distinct from `Some(0)` for the two line-length keys, where
    // the difference decides `PySigConfig::max_len`.
    pub maximum_signature_line_length: Option<i64>,
    pub python_maximum_signature_line_length: Option<i64>,
    pub python_trailing_comma_in_multi_line_signatures: Option<bool>,
    pub python_display_short_literal_types: Option<bool>,
    pub python_use_unqualified_type_names: Option<bool>,
    pub toc_object_entries: Option<bool>,
    pub toc_object_entries_show_parents: Option<String>,
    /// `source_encoding` as written, `None` when conf.py said nothing.
    pub source_encoding: Option<String>,
    /// `(key, python type name)` for the `int | None` keys whose conf.py
    /// value is neither an int nor `None`, and for a `smartquotes` that is
    /// neither a bool nor an int — what sphinx's `check_confval_types`
    /// warns about (see
    /// [`crate::config::BuildConfig::confval_type_mismatches`]).
    pub confval_type_mismatches: Vec<(String, String)>,
    pub add_function_parentheses: Option<bool>,
    pub add_module_names: Option<bool>,
    pub strip_signature_backslash: Option<bool>,
    pub modindex_common_prefix: Vec<String>,

    // intersphinx
    /// The raw `intersphinx_mapping` value, exactly as `conf.py` wrote it.
    /// Normalisation and validation happen in [`ConfPyConfig::to_build_config`],
    /// where a failure can abort configuration the way Sphinx's `ConfigError`
    /// aborts the build.
    pub intersphinx_mapping: serde_json::Value,
    pub intersphinx_disabled_reftypes: Option<Vec<String>>,
    pub intersphinx_resolve_self: Option<String>,
    pub intersphinx_cache_limit: Option<i64>,
    pub intersphinx_timeout: Option<f64>,

    // Internationalization
    pub gettext_uuid: Option<bool>,
    pub gettext_location: Option<bool>,
    pub gettext_auto_build: Option<bool>,
    pub gettext_additional_targets: Vec<String>,

    // Custom configurations (catch-all for extension-specific or custom settings)
    pub custom_configs: HashMap<String, serde_json::Value>,

    /// Every assignment the conf.py made, as parsed: the raw namespace the
    /// typed fields above are extracted from. Carried into
    /// [`BuildConfig::conf_values`].
    pub raw_values: BTreeMap<String, serde_json::Value>,
}

impl PythonConfigParser {
    /// Create a new Python configuration parser
    pub fn new() -> Result<Self> {
        Ok(Self {
            conf_namespace: HashMap::new(),
            warnings: Vec::new(),
            unevaluable: Vec::new(),
        })
    }

    /// Constructs dropped during the last parse (never silently discarded).
    pub fn warnings(&self) -> &[ConfigWarning] {
        &self.warnings
    }

    /// The first statement binding `rst_prolog` or `rst_epilog` that the
    /// last parse could not evaluate, and that no later statement it could
    /// evaluate bound again (by line; at one line, `rst_prolog` first).
    pub fn unevaluable(&self) -> Option<&Unevaluable> {
        self.unevaluable.iter().min_by_key(|binding| binding.line)
    }

    /// Parse a conf.py file and extract configuration. A value of
    /// `rst_prolog` or `rst_epilog` it cannot evaluate fails it, naming
    /// the key ([`Self::unevaluable`]); [`Self::warnings`] still holds what
    /// the parse dropped.
    pub fn parse_conf_py<P: AsRef<Path>>(&mut self, conf_py_path: P) -> Result<ConfPyConfig> {
        let conf_py_path = conf_py_path.as_ref();
        let _conf_dir = conf_py_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid conf.py path"))?;

        // Read the conf.py file
        let conf_py_content = std::fs::read_to_string(conf_py_path)?;

        self.parse_statements(&conf_py_content)?;
        if let Some(binding) = self.unevaluable() {
            // Sphinx names its configuration file the same way when it
            // cannot compile it: `(conf.py, line N)`.
            let file = conf_py_path
                .file_name()
                .map_or_else(|| "conf.py".into(), |name| name.to_string_lossy());
            anyhow::bail!(
                "cannot evaluate {} ({file}, line {}: {}): sphinx-ultra reads conf.py without \
                 running it, so rst_prolog and rst_epilog must be string literals",
                binding.key,
                binding.line,
                binding.reason
            );
        }

        // Extract configuration values
        self.extract_configuration()
    }

    /// Parse the declarative subset of a conf.py: literal assignments, with a
    /// warning recorded for every construct that had to be dropped, except
    /// the statements that bind a key of [`MUST_EVALUATE`], which are
    /// evaluated or recorded as [`Self::unevaluable`].
    fn parse_statements(&mut self, content: &str) -> Result<()> {
        for (line, raw) in logical_statements(content) {
            let stmt = raw.trim();
            if stmt.is_empty() {
                continue;
            }

            // Imports set no configuration values; ignoring them loses
            // nothing, except an import that binds a key this reader must
            // evaluate (a star import names nothing it can tell).
            if stmt.starts_with("import ") || stmt.starts_with("from ") {
                let imported = imported_names(stmt);
                for key in MUST_EVALUATE {
                    if imported.contains(&key)
                        && !self.unevaluable.iter().any(|binding| binding.key == key)
                    {
                        let reason = "an import".to_string();
                        self.unevaluable.push(Unevaluable { key, line, reason });
                    }
                }
                continue;
            }

            // A statement indented under a compound statement's header,
            // which this reader drops: it may run once, many times or not
            // at all.
            let indented = raw.starts_with([' ', '\t']);
            let assignment = assignment(stmt);
            let bound: Vec<&'static str> = MUST_EVALUATE
                .into_iter()
                .filter(|key| assignment.binds(key))
                .collect();
            if !bound.is_empty() {
                for key in bound {
                    match self.evaluate_binding(stmt, &assignment, key, indented) {
                        Ok(()) => self.unevaluable.retain(|binding| binding.key != key),
                        Err(reason) => {
                            if !self.unevaluable.iter().any(|binding| binding.key == key) {
                                self.unevaluable.push(Unevaluable { key, line, reason });
                            }
                        }
                    }
                }
                continue;
            }

            match split_assignment(stmt) {
                Some((name, value_src)) => match parse_python_literal(value_src) {
                    Ok(value) => {
                        self.conf_namespace.insert(name.to_string(), value);
                    }
                    Err(reason) => self.warnings.push(ConfigWarning {
                        line,
                        message: format!(
                            "unsupported value for '{}' dropped ({}): {}",
                            name,
                            reason,
                            snippet(value_src)
                        ),
                    }),
                },
                None => self.warnings.push(ConfigWarning {
                    line,
                    message: format!("unsupported statement dropped: {}", snippet(stmt)),
                }),
            }
        }

        Ok(())
    }

    /// Evaluates `stmt`, whose `assignment` binds `key` (a key of
    /// [`MUST_EVALUATE`]), into the namespace, or says why it cannot. Only
    /// two statements are evaluated, both at the top level: an assignment
    /// of a literal to the key alone, and `key += <string literal>` while
    /// the key holds a string (Python's concatenation; implicit
    /// concatenation of literals included).
    fn evaluate_binding(
        &mut self,
        stmt: &str,
        assignment: &Assignment<'_>,
        key: &'static str,
        indented: bool,
    ) -> std::result::Result<(), String> {
        if indented {
            return Err("an assignment inside a block".to_string());
        }
        if assignment.augmented {
            // What the key holds, unless a binding since the last value
            // this reader evaluated left it unknown.
            let held = if self.unevaluable.iter().any(|binding| binding.key == key) {
                None
            } else {
                self.conf_namespace
                    .get(key)
                    .and_then(serde_json::Value::as_str)
            };
            let added = stmt
                .strip_prefix(key)
                .and_then(|rest| rest.trim_start().strip_prefix("+="))
                .map(|value_src| parse_python_literal(value_src.trim()));
            return match (held, added) {
                (Some(held), Some(Ok(serde_json::Value::String(added)))) => {
                    let joined = serde_json::Value::String(format!("{held}{added}"));
                    self.conf_namespace.insert(key.to_string(), joined);
                    Ok(())
                }
                _ => Err("an augmented assignment".to_string()),
            };
        }
        match split_assignment(stmt) {
            Some((name, value_src)) if name == key => {
                let value = parse_python_literal(value_src)?;
                self.conf_namespace.insert(key.to_string(), value);
                Ok(())
            }
            _ => Err("an assignment it cannot evaluate".to_string()),
        }
    }

    /// Extract configuration values from the parsed Python namespace
    fn extract_configuration(&self) -> Result<ConfPyConfig> {
        let mut config = ConfPyConfig::default();

        // Helper function to extract optional string values
        let extract_string = |key: &str| -> Option<String> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_str().map(|s| s.to_string()))
        };

        // Helper function to extract optional bool values
        let extract_bool = |key: &str| -> Option<bool> {
            self.conf_namespace.get(key).and_then(|val| val.as_bool())
        };

        // Helper function to extract optional int values
        let extract_int = |key: &str| -> Option<i32> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_i64().map(|i| i as i32))
        };

        // Helper function to extract list of strings
        let extract_string_list = |key: &str| -> Vec<String> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };

        // Helper function to extract a list of 2-string tuples
        // (`nitpick_ignore`'s `[('py:func', 'foo'), ...]` shape).
        let extract_pair_list = |key: &str| -> Vec<(String, String)> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|pair| {
                            let pair = pair.as_array()?;
                            let first = pair.first()?.as_str()?;
                            let second = pair.get(1)?.as_str()?;
                            Some((first.to_string(), second.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        // Helper function to extract dictionary
        let extract_dict = |key: &str| -> HashMap<String, serde_json::Value> {
            self.conf_namespace
                .get(key)
                .and_then(|val| val.as_object())
                .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default()
        };

        // Extract project information
        config.project = extract_string("project");
        config.version = extract_string("version");
        config.release = extract_string("release");
        config.today = extract_string("today");
        config.today_fmt = extract_string("today_fmt");
        config.copyright = extract_string("copyright");
        config.author = extract_string("author");

        // Extract general configuration
        config.extensions = extract_string_list("extensions");
        config.templates_path = extract_string_list("templates_path");
        config.exclude_patterns = extract_string_list("exclude_patterns");
        config.include_patterns = extract_string_list("include_patterns");
        config.root_doc = extract_string("root_doc").or_else(|| extract_string("master_doc"));
        config.language = extract_string("language");
        config.locale_dirs = extract_string_list("locale_dirs");
        config.gettext_compact = extract_bool("gettext_compact");

        // Extract HTML output options
        config.html_theme = extract_string("html_theme");
        config.html_theme_options = extract_dict("html_theme_options");
        config.html_title = extract_string("html_title");
        config.html_short_title = extract_string("html_short_title");
        config.html_logo = extract_string("html_logo");
        config.html_favicon = extract_string("html_favicon");
        config.html_css_files = extract_string_list("html_css_files");
        config.html_js_files = extract_string_list("html_js_files");
        config.html_static_path = extract_string_list("html_static_path");
        config.html_extra_path = extract_string_list("html_extra_path");
        config.html_use_index = extract_bool("html_use_index");
        config.html_split_index = extract_bool("html_split_index");
        config.html_copy_source = extract_bool("html_copy_source");
        config.html_show_sourcelink = extract_bool("html_show_sourcelink");
        config.html_sourcelink_suffix = extract_string("html_sourcelink_suffix");
        config.html_use_opensearch = extract_string("html_use_opensearch");
        config.html_file_suffix = extract_string("html_file_suffix");
        config.html_link_suffix = extract_string("html_link_suffix");
        config.html_show_copyright = extract_bool("html_show_copyright");
        config.html_show_sphinx = extract_bool("html_show_sphinx");
        config.html_context = extract_dict("html_context");
        config.html_output_encoding = extract_string("html_output_encoding");
        config.html_compact_lists = extract_bool("html_compact_lists");
        config.html_secnumber_suffix = extract_string("html_secnumber_suffix");
        config.html_search_language = extract_string("html_search_language");
        config.html_search_options = extract_dict("html_search_options");
        config.html_search_scorer = extract_string("html_search_scorer");
        config.html_scaled_image_link = extract_bool("html_scaled_image_link");
        config.html_baseurl = extract_string("html_baseurl");
        config.html_codeblock_linenos_style = extract_string("html_codeblock_linenos_style");
        config.html_math_renderer = extract_string("html_math_renderer");
        config.html_math_renderer_options = extract_dict("html_math_renderer_options");

        // Extract build options
        config.needs_sphinx = extract_string("needs_sphinx");
        config.manpages_url = extract_string("manpages_url");
        config.nitpicky = extract_bool("nitpicky");
        config.nitpick_ignore = extract_pair_list("nitpick_ignore");
        config.nitpick_ignore_regex = extract_pair_list("nitpick_ignore_regex");
        config.numfig = extract_bool("numfig");
        // `numfig_format` is a str -> str dict; a non-string value is not a
        // format string sphinx could interpolate, so it is dropped here
        // rather than carried as JSON.
        config.numfig_format = extract_dict("numfig_format")
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
            .collect();
        config.numfig_secnum_depth = extract_int("numfig_secnum_depth");
        config.math_number_all = extract_bool("math_number_all");
        config.math_eqref_format = extract_string("math_eqref_format");
        config.math_numfig = extract_bool("math_numfig");
        config.tls_verify = extract_bool("tls_verify");
        // `tls_cacerts` is `str | dict[str, str] | None` (`config.py:287`):
        // one CA bundle for everything, or one per host.
        config.tls_cacerts = self
            .conf_namespace
            .get("tls_cacerts")
            .and_then(|value| match value {
                serde_json::Value::String(path) => {
                    Some(crate::intersphinx::TlsCacerts::Bundle(path.clone()))
                }
                serde_json::Value::Object(map) => Some(crate::intersphinx::TlsCacerts::PerHost(
                    map.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                        .collect(),
                )),
                _ => None,
            });
        config.user_agent = extract_string("user_agent");
        config.suppress_warnings = extract_string_list("suppress_warnings");
        config.show_warning_types = extract_bool("show_warning_types");
        config.keep_warnings = extract_bool("keep_warnings");
        config.trim_doctest_flags = extract_bool("trim_doctest_flags");
        config.smartquotes = extract_bool("smartquotes");
        config.smartquotes_action = extract_string("smartquotes_action");
        config.smartquotes_excludes = self
            .conf_namespace
            .get("smartquotes_excludes")
            .and_then(serde_json::Value::as_object)
            .map(|excludes| {
                // `excludes.get(key, [])`, which `name in` tests
                // ([`crate::config::ExcludedNames`]): a list or a tuple by
                // the names in it, a string by searching it, a dict by its
                // keys. Any other value makes Sphinx raise `TypeError`
                // (not modelled) and is read as no names.
                let names = |key: &str| -> crate::config::ExcludedNames {
                    use crate::config::ExcludedNames;
                    match excludes.get(key) {
                        Some(serde_json::Value::Array(items)) => ExcludedNames::Names(
                            items
                                .iter()
                                .filter_map(|item| item.as_str().map(str::to_string))
                                .collect(),
                        ),
                        Some(serde_json::Value::String(text)) => ExcludedNames::Text(text.clone()),
                        Some(serde_json::Value::Object(map)) => {
                            ExcludedNames::Names(map.keys().cloned().collect())
                        }
                        _ => ExcludedNames::default(),
                    }
                };
                crate::config::SmartquotesExcludes {
                    languages: names("languages"),
                    builders: names("builders"),
                }
            });
        config.primary_domain = match self.conf_namespace.get("primary_domain") {
            Some(serde_json::Value::Null) => Some(None),
            Some(value) => value.as_str().map(|name| Some(name.to_string())),
            None => None,
        };
        config.highlight_language = extract_string("highlight_language");
        config.default_role = match self.conf_namespace.get("default_role") {
            Some(serde_json::Value::Null) => Some(None),
            Some(value) => value.as_str().map(|name| Some(name.to_string())),
            None => None,
        };
        config.math_numsep = extract_string("math_numsep");

        // The values `check_confval_types` reports
        // (`crate::config::BuildConfig::confval_type_mismatches`).
        // `smartquotes` is a `bool` (`config.py:289`): Sphinx keeps any
        // other value, which leaves smart quotes on
        // (`ConfPyConfig::smartquotes`), and reports one whose type shares
        // no base with `bool`, which is every type but `int`, `bool`'s base
        // (`The config value `smartquotes' has type `NoneType'; expected
        // `bool'.`).
        let mut mismatches: Vec<(String, String)> = Vec::new();
        if let Some(value) = self.conf_namespace.get("smartquotes") {
            let type_name = python_type_name(value);
            if !matches!(type_name, "bool" | "int") {
                mismatches.push(("smartquotes".to_string(), type_name.to_string()));
            }
        }
        // `rst_prolog` and `rst_epilog` are `str` (`config.py:265-266`):
        // any other value but `None` is what `check_confval_types`
        // reports, and Sphinx's reader adds nothing for a false one (`if
        // not prologue: return`, `SP/sphinx/util/rst.py:82-83`). A true one
        // makes Sphinx's reader raise `AttributeError` (`splitlines`),
        // which is not modelled: it reads as `None` here.
        for (key, slot) in [
            ("rst_prolog", &mut config.rst_prolog),
            ("rst_epilog", &mut config.rst_epilog),
        ] {
            *slot = match self.conf_namespace.get(key) {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(text)) => Some(text.clone()),
                Some(value) => {
                    mismatches.push((key.to_string(), python_type_name(value).to_string()));
                    None
                }
            };
        }

        // Object-signature / py-domain family. The two line-length keys are
        // `int | None`, so they read through `as_i64` rather than
        // `extract_int`: `x = None` in conf.py is a JSON null, which lands
        // as `None` exactly like an absent key, and `x = 0` stays `Some(0)`.
        // Any other type is what sphinx's `check_confval_types` warns about
        // (`The config value ... has type `str'; expected `NoneType' or
        // `int'.`) — recorded with the python type name and left unset.
        let mut extract_none_default_int = |key: &str| -> Option<i64> {
            match self.conf_namespace.get(key) {
                None | Some(serde_json::Value::Null) => None,
                Some(value) => match value.as_i64() {
                    Some(int) => Some(int),
                    None => {
                        mismatches.push((key.to_string(), python_type_name(value).to_string()));
                        None
                    }
                },
            }
        };
        config.maximum_signature_line_length =
            extract_none_default_int("maximum_signature_line_length");
        config.python_maximum_signature_line_length =
            extract_none_default_int("python_maximum_signature_line_length");
        config.confval_type_mismatches = mismatches;
        config.source_encoding = extract_string("source_encoding");
        config.python_trailing_comma_in_multi_line_signatures =
            extract_bool("python_trailing_comma_in_multi_line_signatures");
        config.python_display_short_literal_types =
            extract_bool("python_display_short_literal_types");
        config.python_use_unqualified_type_names =
            extract_bool("python_use_unqualified_type_names");
        config.toc_object_entries = extract_bool("toc_object_entries");
        config.toc_object_entries_show_parents = extract_string("toc_object_entries_show_parents");
        config.add_function_parentheses = extract_bool("add_function_parentheses");
        config.add_module_names = extract_bool("add_module_names");
        config.strip_signature_backslash = extract_bool("strip_signature_backslash");
        config.modindex_common_prefix = extract_string_list("modindex_common_prefix");

        // Extract intersphinx configuration. The mapping is carried raw:
        // validating it is `to_build_config`'s job, because that is where a
        // failure can be reported as a configuration error.
        config.intersphinx_mapping = self
            .conf_namespace
            .get("intersphinx_mapping")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        config.intersphinx_disabled_reftypes = self
            .conf_namespace
            .get("intersphinx_disabled_reftypes")
            .map(|_| extract_string_list("intersphinx_disabled_reftypes"));
        config.intersphinx_resolve_self = extract_string("intersphinx_resolve_self");
        config.intersphinx_cache_limit = self
            .conf_namespace
            .get("intersphinx_cache_limit")
            .and_then(serde_json::Value::as_i64);
        config.intersphinx_timeout = self
            .conf_namespace
            .get("intersphinx_timeout")
            .and_then(serde_json::Value::as_f64);

        // Extract internationalization
        config.gettext_uuid = extract_bool("gettext_uuid");
        config.gettext_location = extract_bool("gettext_location");
        config.gettext_auto_build = extract_bool("gettext_auto_build");
        config.gettext_additional_targets = extract_string_list("gettext_additional_targets");

        // Extract custom configurations
        for (key, value) in &self.conf_namespace {
            if !Self::is_standard_config_key(key) {
                config.custom_configs.insert(key.clone(), value.clone());
            }
        }
        config.raw_values = self
            .conf_namespace
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        Ok(config)
    }

    /// Check if a configuration key is a standard Sphinx configuration
    fn is_standard_config_key(key: &str) -> bool {
        matches!(
            key,
            "project"
                | "version"
                | "release"
                | "today"
                | "today_fmt"
                | "copyright"
                | "author"
                | "extensions"
                | "templates_path"
                | "exclude_patterns"
                | "include_patterns"
                | "source_suffix"
                | "source_encoding"
                | "root_doc"
                | "master_doc"
                | "language"
                | "locale_dirs"
                | "gettext_compact"
                | "html_theme"
                | "html_theme_options"
                | "html_title"
                | "html_short_title"
                | "html_logo"
                | "html_favicon"
                | "html_css_files"
                | "html_js_files"
                | "html_static_path"
                | "html_extra_path"
                | "html_use_index"
                | "html_split_index"
                | "html_copy_source"
                | "html_show_sourcelink"
                | "html_sourcelink_suffix"
                | "html_use_opensearch"
                | "html_file_suffix"
                | "html_link_suffix"
                | "html_show_copyright"
                | "html_show_sphinx"
                | "html_context"
                | "html_output_encoding"
                | "html_compact_lists"
                | "html_secnumber_suffix"
                | "html_search_language"
                | "html_search_options"
                | "html_search_scorer"
                | "html_scaled_image_link"
                | "html_baseurl"
                | "html_codeblock_linenos_style"
                | "html_math_renderer"
                | "html_math_renderer_options"
                | "needs_sphinx"
                | "nitpicky"
                | "nitpick_ignore"
                | "nitpick_ignore_regex"
                | "maximum_signature_line_length"
                | "python_maximum_signature_line_length"
                | "python_trailing_comma_in_multi_line_signatures"
                | "python_display_short_literal_types"
                | "python_use_unqualified_type_names"
                | "toc_object_entries"
                | "toc_object_entries_show_parents"
                | "add_function_parentheses"
                | "add_module_names"
                | "strip_signature_backslash"
                | "modindex_common_prefix"
                | "numfig"
                | "numfig_format"
                | "numfig_secnum_depth"
                | "manpages_url"
                | "math_number_all"
                | "math_eqref_format"
                | "math_numfig"
                | "tls_verify"
                | "tls_cacerts"
                | "user_agent"
                | "suppress_warnings"
                | "show_warning_types"
                | "keep_warnings"
                | "trim_doctest_flags"
                | "smartquotes"
                | "smartquotes_action"
                | "smartquotes_excludes"
                | "primary_domain"
                | "highlight_language"
                | "default_role"
                | "math_numsep"
                | "rst_prolog"
                | "rst_epilog"
                | "intersphinx_mapping"
                | "intersphinx_disabled_reftypes"
                | "intersphinx_resolve_self"
                | "intersphinx_cache_limit"
                | "intersphinx_timeout"
                | "gettext_uuid"
                | "gettext_location"
                | "gettext_auto_build"
                | "gettext_additional_targets"
        )
    }
}

/// The `type(value).__name__` sphinx's `check_confval_types` prints for a
/// conf.py literal, by way of its JSON shape. A python tuple arrives as a
/// list here, so it would be named `list` — a spelling-only difference in
/// a warning about an already-rejected value.
fn python_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "NoneType",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(number) if number.is_i64() || number.is_u64() => "int",
        serde_json::Value::Number(_) => "float",
        serde_json::Value::String(_) => "str",
        serde_json::Value::Array(_) => "list",
        serde_json::Value::Object(_) => "dict",
    }
}

/// First ~60 chars of a construct, for warning messages.
fn snippet(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(60) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_string(),
    }
}

/// Split Python source into logical statements: physical lines joined while
/// brackets are open, a string (incl. triple-quoted) is unterminated, or a
/// trailing backslash continues the line. Comments outside strings are
/// stripped. Yields `(1-based start line, statement text)`.
fn logical_statements(content: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = content.chars().collect();
    let mut statements = Vec::new();

    let mut buf = String::new();
    let mut start_line = 1usize;
    let mut line = 1usize;
    let mut depth = 0i32;
    // (quote char, is_triple)
    let mut string_state: Option<(char, bool)> = None;
    let mut escaped = false;

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        if let Some((quote, triple)) = string_state {
            buf.push(c);
            if c == '\n' {
                line += 1;
            }
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                if triple {
                    if i + 2 < chars.len() && chars[i + 1] == quote && chars[i + 2] == quote {
                        buf.push(quote);
                        buf.push(quote);
                        i += 2;
                        string_state = None;
                    }
                } else {
                    string_state = None;
                }
            }
            i += 1;
            continue;
        }

        match c {
            '\'' | '"' => {
                let triple = i + 2 < chars.len() && chars[i + 1] == c && chars[i + 2] == c;
                buf.push(c);
                if triple {
                    buf.push(c);
                    buf.push(c);
                    i += 2;
                }
                string_state = Some((c, triple));
            }
            '#' => {
                // Comment: skip to (but not past) end of line.
                while i + 1 < chars.len() && chars[i + 1] != '\n' {
                    i += 1;
                }
            }
            '(' | '[' | '{' => {
                depth += 1;
                buf.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                buf.push(c);
            }
            '\\' if i + 1 < chars.len() && chars[i + 1] == '\n' => {
                // Explicit line continuation: join without the backslash.
                buf.push(' ');
                line += 1;
                i += 1;
            }
            '\n' => {
                line += 1;
                if depth > 0 {
                    buf.push('\n');
                } else {
                    if !buf.trim().is_empty() {
                        statements.push((start_line, std::mem::take(&mut buf)));
                    } else {
                        buf.clear();
                    }
                    start_line = line;
                }
            }
            _ => {
                if buf.trim().is_empty() && !c.is_whitespace() && buf.is_empty() {
                    start_line = line;
                }
                buf.push(c);
            }
        }
        i += 1;
    }

    if !buf.trim().is_empty() {
        statements.push((start_line, buf));
    }

    statements
}

/// Split `identifier = <value>` at the first top-level `=` that is a plain
/// assignment (not `==`, `!=`, `<=`, `>=`, or an augmented assignment).
/// Returns `None` for anything that is not a simple assignment to a bare name.
fn split_assignment(stmt: &str) -> Option<(&str, &str)> {
    let bytes = stmt.as_bytes();
    let mut depth = 0i32;
    let mut string_quote: Option<u8> = None;

    for i in 0..bytes.len() {
        let b = bytes[i];
        if let Some(q) = string_quote {
            if b == q && (i == 0 || bytes[i - 1] != b'\\') {
                string_quote = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => string_quote = Some(b),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'=' if depth == 0 => {
                let next_eq = bytes.get(i + 1) == Some(&b'=');
                let prev = if i > 0 { bytes[i - 1] } else { 0 };
                if next_eq || matches!(prev, b'=' | b'!' | b'<' | b'>') {
                    return None; // comparison
                }
                if matches!(
                    prev,
                    b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^' | b'@'
                ) {
                    return None; // augmented assignment
                }
                let name = stmt[..i].trim();
                let is_identifier = !name.is_empty()
                    && name
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_alphabetic() || c == '_')
                        .unwrap_or(false)
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !is_identifier {
                    return None;
                }
                return Some((name, stmt[i + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

/// What an assignment statement assigns to, as far as a key of
/// [`MUST_EVALUATE`] needs telling: the text of each of its targets and
/// whether it is an augmented assignment (`+=`, `-=`, ...).
struct Assignment<'s> {
    /// The text before each top-level `=` (a chained assignment has
    /// several), or before the augmented operator; empty for a statement
    /// with neither.
    targets: Vec<&'s str>,
    augmented: bool,
}

impl Assignment<'_> {
    /// Whether one of the targets names `key`, outside strings and
    /// attribute access (`x.key`): a plain or augmented assignment to it,
    /// a tuple or a chained target holding it, or one written after a
    /// compound statement's header on the same line (`if x: key = ...`).
    fn binds(&self, key: &str) -> bool {
        self.targets
            .iter()
            .any(|target| target_names(target).any(|name| name == key))
    }
}

/// The assignment `stmt` makes, from its top-level `=` signs: outside
/// strings and brackets, and none of `==`, `!=`, `<=`, `>=` or `:=`. An
/// `=` that a binary operator precedes (`+=`, `//=`, `>>=`, ...) makes it
/// an augmented assignment, whose one target stops before the operator.
/// A `for` or `with` target, a `def`, an import and a `del` are not
/// assignments here.
fn assignment(stmt: &str) -> Assignment<'_> {
    let bytes = stmt.as_bytes();
    let mut targets = Vec::new();
    let (mut depth, mut start, mut i) = (0i32, 0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i = skip_string(bytes, i, quote);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'=' if depth == 0 => {
                if bytes.get(i + 1) == Some(&b'=') {
                    i += 2;
                    continue;
                }
                let prev = i.checked_sub(1).map(|j| bytes[j]);
                let doubled = prev.is_some() && i.checked_sub(2).map(|j| bytes[j]) == prev;
                let operator = match prev {
                    Some(b'<' | b'>') if doubled => 2,
                    Some(b'=' | b'!' | b'<' | b'>' | b':') => {
                        i += 1;
                        continue;
                    }
                    Some(b'*' | b'/') if doubled => 2,
                    Some(b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^' | b'@') => 1,
                    _ => 0,
                };
                targets.push(&stmt[start..i - operator]);
                if operator > 0 {
                    return Assignment {
                        targets,
                        augmented: true,
                    };
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    Assignment {
        targets,
        augmented: false,
    }
}

/// The names the import statement `stmt` binds: each name (or its `as`
/// alias) of `from m import a, b as c`, parenthesized or not, and the
/// first component (or the alias) of each module of `import a.b, d as e`.
/// A star import binds nothing this reader can name.
fn imported_names(stmt: &str) -> Vec<&str> {
    let (items, from) = match stmt.strip_prefix("from ") {
        Some(rest) => match rest.split_once(" import ") {
            Some((_, names)) => (names, true),
            None => return Vec::new(),
        },
        None => match stmt.strip_prefix("import ") {
            Some(modules) => (modules, false),
            None => return Vec::new(),
        },
    };
    let items = items.trim().trim_start_matches('(').trim_end_matches(')');
    items
        .split(',')
        .filter_map(|item| {
            let mut words = item.split_whitespace();
            let name = words.next()?;
            match (words.next(), words.next()) {
                (Some("as"), Some(alias)) => Some(alias),
                _ if from => Some(name),
                _ => name.split('.').next(),
            }
        })
        .collect()
}

/// The index just past the Python string literal whose opening quote,
/// `quote`, is at `start` (triple-quoted when three open it); a backslash
/// escapes the next byte, as it does even in a raw string's termination.
fn skip_string(bytes: &[u8], start: usize, quote: u8) -> usize {
    let triple = bytes.get(start + 1) == Some(&quote) && bytes.get(start + 2) == Some(&quote);
    let width = if triple { 3 } else { 1 };
    let mut i = start + width;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            byte if byte == quote
                && (!triple
                    || (bytes.get(i + 1) == Some(&quote) && bytes.get(i + 2) == Some(&quote))) =>
            {
                return i + width;
            }
            _ => i += 1,
        }
    }
    bytes.len()
}

/// The names `target` holds, outside strings and attribute names (the
/// `name` of `x.name`): every identifier, whether the target binds it or
/// only reads it (a subscript's key).
fn target_names(target: &str) -> impl Iterator<Item = &str> {
    let bytes = target.as_bytes();
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80;
    let mut names = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\'' || byte == b'"' {
            i = skip_string(bytes, i, byte);
        } else if identifier(byte) {
            let start = i;
            while i < bytes.len() && identifier(bytes[i]) {
                i += 1;
            }
            let attribute = target[..start].trim_end().ends_with('.');
            if !byte.is_ascii_digit() && !attribute {
                names.push(&target[start..i]);
            }
        } else {
            i += 1;
        }
    }
    names.into_iter()
}

/// Python 3.12's decimal number literals, unsigned: `Decnumber` and
/// `Floatnumber` of `Lib/tokenize.py:74-80` (the language reference's
/// `decinteger`, `pointfloat` and `exponentfloat`). Digits that single
/// underscores may separate; an integer that starts with `0` is all zeros;
/// a float has a `.` with digits on at least one side, an exponent (`e` or
/// `E`, a sign or none, digits), or both. Any other run of these
/// characters is no number to Python: a `SyntaxError`, or an attribute of
/// a float (`5..e5`), which this reader does not model either.
static DECIMAL_LITERAL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    let digits = "[0-9](?:_?[0-9])*";
    let exponent = format!("[eE][-+]?{digits}");
    regex::Regex::new(&format!(
        "^(?:0(?:_?0)*|[1-9](?:_?[0-9])*\
         |(?:{digits}\\.(?:{digits})?|\\.{digits})(?:{exponent})?\
         |{digits}{exponent})$"
    ))
    .expect("the decimal literal pattern compiles")
});

/// Recursive-descent parser for Python literals → JSON values.
/// Supports: strings (escapes, implicit adjacent concatenation, triple
/// quotes), ints/floats, True/False/None, lists, tuples (as arrays), dicts
/// with string keys, arbitrary nesting, trailing commas.
pub(crate) fn parse_python_literal(src: &str) -> std::result::Result<serde_json::Value, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut p = PyLiteralParser {
        chars,
        pos: 0,
        saw_comma: false,
    };
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos < p.chars.len() {
        return Err("trailing expression".to_string());
    }
    Ok(value)
}

struct PyLiteralParser {
    chars: Vec<char>,
    pos: usize,
    /// Whether the most recently closed sequence contained a comma — used to
    /// tell a parenthesized grouping `(x)` from a one-element tuple `(x,)`.
    saw_comma: bool,
}

impl PyLiteralParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> std::result::Result<serde_json::Value, String> {
        self.skip_ws();
        if let Some(raw) = self.string_prefix() {
            let mut s = self.parse_string(raw)?;
            // Implicit adjacent string concatenation: 'a' 'b' == 'ab'
            while let Some(raw) = {
                self.skip_ws();
                self.string_prefix()
            } {
                s.push_str(&self.parse_string(raw)?);
            }
            return Ok(serde_json::Value::String(s));
        }
        match self.peek() {
            Some('[') => self.parse_sequence('[', ']'),
            Some('(') => {
                // Python: `(x)` is grouping, `(x,)` / `(x, y)` is a tuple.
                // Either way an array (or the inner value) serves config needs.
                let value = self.parse_sequence('(', ')')?;
                match value {
                    serde_json::Value::Array(items) if items.len() == 1 && !self.saw_comma => {
                        Ok(items.into_iter().next().unwrap())
                    }
                    other => Ok(other),
                }
            }
            Some('{') => self.parse_dict(),
            Some(c) if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' => {
                self.parse_number()
            }
            Some(_) => {
                if self.eat_keyword("True") {
                    Ok(serde_json::Value::Bool(true))
                } else if self.eat_keyword("False") {
                    Ok(serde_json::Value::Bool(false))
                } else if self.eat_keyword("None") {
                    Ok(serde_json::Value::Null)
                } else {
                    Err("unsupported expression".to_string())
                }
            }
            None => Err("empty value".to_string()),
        }
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        let end = self.pos + kw.len();
        if end <= self.chars.len() && self.chars[self.pos..end].iter().collect::<String>() == kw {
            let boundary = self
                .chars
                .get(end)
                .map(|c| !c.is_ascii_alphanumeric() && *c != '_')
                .unwrap_or(true);
            if boundary {
                self.pos = end;
                return true;
            }
        }
        false
    }

    /// A string literal starts here — with an optional Python prefix, which
    /// is consumed. `Some(true)` means the literal is *raw*: `nitpick_ignore_regex`
    /// entries are conventionally written `r'...'`, and a raw literal's
    /// backslashes must survive into the pattern. `f` prefixes are
    /// deliberately not accepted: an f-string's braces are an expression
    /// this parser cannot evaluate, so it stays an unsupported expression
    /// (skipped) rather than being taken literally.
    fn string_prefix(&mut self) -> Option<bool> {
        if matches!(self.peek(), Some('\'') | Some('"')) {
            return Some(false);
        }
        for len in [2usize, 1] {
            let quote_at = self.pos + len;
            if !matches!(self.chars.get(quote_at), Some('\'') | Some('"')) {
                continue;
            }
            let prefix: String = self.chars[self.pos..quote_at]
                .iter()
                .collect::<String>()
                .to_lowercase();
            if matches!(prefix.as_str(), "r" | "u" | "b" | "rb" | "br") {
                self.pos = quote_at;
                return Some(prefix.contains('r'));
            }
        }
        None
    }

    fn parse_string(&mut self, raw: bool) -> std::result::Result<String, String> {
        let quote = self.peek().ok_or("expected string")?;
        self.pos += 1;
        let triple = self.chars.get(self.pos) == Some(&quote)
            && self.chars.get(self.pos + 1) == Some(&quote);
        if triple {
            self.pos += 2;
        }

        let mut out = String::new();
        loop {
            let c = *self
                .chars
                .get(self.pos)
                .ok_or("unterminated string literal")?;
            if c == '\\' {
                let next = *self
                    .chars
                    .get(self.pos + 1)
                    .ok_or("unterminated escape sequence")?;
                if raw {
                    // A raw literal keeps both characters — but the escaped
                    // quote still does not end the string.
                    out.push('\\');
                    out.push(next);
                    self.pos += 2;
                    continue;
                }
                let translated = match next {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '\\' => '\\',
                    '\'' => '\'',
                    '"' => '"',
                    other => {
                        // Unknown escape: Python keeps the backslash.
                        out.push('\\');
                        other
                    }
                };
                out.push(translated);
                self.pos += 2;
                continue;
            }
            if c == quote {
                if triple {
                    if self.chars.get(self.pos + 1) == Some(&quote)
                        && self.chars.get(self.pos + 2) == Some(&quote)
                    {
                        self.pos += 3;
                        return Ok(out);
                    }
                } else {
                    self.pos += 1;
                    return Ok(out);
                }
            }
            out.push(c);
            self.pos += 1;
        }
    }

    /// A number: an optional sign, then the run of characters a decimal
    /// literal is made of (a sign among them only right after the
    /// exponent's `e` or `E`), which must be one Python 3.12 compiles
    /// ([`DECIMAL_LITERAL`]). The other bases and the imaginary literals end
    /// the run at their letter, and a sign spelled apart from the literal or
    /// twice ends it before the digits, so each of those stays an
    /// unsupported value.
    fn parse_number(&mut self) -> std::result::Result<serde_json::Value, String> {
        let start = self.pos;
        if matches!(self.peek(), Some('-') | Some('+')) {
            self.pos += 1;
        }
        let literal = self.pos;
        while let Some(c) = self.peek() {
            let exponent_sign = matches!(c, '-' | '+')
                && self.pos > literal
                && matches!(self.chars[self.pos - 1], 'e' | 'E');
            if !(c.is_ascii_digit() || matches!(c, '.' | '_' | 'e' | 'E') || exponent_sign) {
                break;
            }
            self.pos += 1;
        }
        let run: String = self.chars[literal..self.pos].iter().collect();
        if !DECIMAL_LITERAL.is_match(&run) {
            let written: String = self.chars[start..self.pos].iter().collect();
            return Err(format!("invalid number '{written}'"));
        }
        let text: String = self.chars[start..self.pos]
            .iter()
            .filter(|c| **c != '_')
            .collect();
        if let Ok(i) = text.parse::<i64>() {
            return Ok(serde_json::Value::Number(i.into()));
        }
        if let Ok(f) = text.parse::<f64>() {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return Ok(serde_json::Value::Number(n));
            }
        }
        Err(format!("invalid number '{text}'"))
    }

    fn parse_sequence(
        &mut self,
        open: char,
        close: char,
    ) -> std::result::Result<serde_json::Value, String> {
        debug_assert_eq!(self.peek(), Some(open));
        self.pos += 1;
        self.saw_comma = false;
        let mut items = Vec::new();
        let mut saw_comma = false;
        loop {
            self.skip_ws();
            if self.peek() == Some(close) {
                self.pos += 1;
                self.saw_comma = saw_comma;
                return Ok(serde_json::Value::Array(items));
            }
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    saw_comma = true;
                    self.pos += 1;
                }
                Some(c) if c == close => {}
                _ => return Err(format!("expected ',' or '{close}'")),
            }
        }
    }

    fn parse_dict(&mut self) -> std::result::Result<serde_json::Value, String> {
        debug_assert_eq!(self.peek(), Some('{'));
        self.pos += 1;
        let mut map = serde_json::Map::new();
        loop {
            self.skip_ws();
            if self.peek() == Some('}') {
                self.pos += 1;
                return Ok(serde_json::Value::Object(map));
            }
            let key = match self.parse_value()? {
                serde_json::Value::String(s) => s,
                other => return Err(format!("non-string dict key {other}")),
            };
            self.skip_ws();
            if self.peek() != Some(':') {
                return Err("expected ':' in dict".to_string());
            }
            self.pos += 1;
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some('}') => {}
                _ => return Err("expected ',' or '}'".to_string()),
            }
        }
    }
}

impl Default for ConfPyConfig {
    fn default() -> Self {
        Self {
            project: None,
            version: None,
            release: None,
            today: None,
            today_fmt: None,
            copyright: None,
            author: None,
            extensions: Vec::new(),
            templates_path: vec!["_templates".to_string()],
            exclude_patterns: Vec::new(),
            include_patterns: vec!["**".to_string()], // Sphinx default
            source_suffix: HashMap::new(),
            root_doc: Some("index".to_string()),
            language: None,
            locale_dirs: vec!["locales".to_string()],
            gettext_compact: Some(true),
            html_theme: Some("alabaster".to_string()),
            html_theme_options: HashMap::new(),
            html_title: None,
            html_short_title: None,
            html_logo: None,
            html_favicon: None,
            html_css_files: Vec::new(),
            html_js_files: Vec::new(),
            html_static_path: vec!["_static".to_string()],
            html_extra_path: Vec::new(),
            html_use_index: Some(true),
            html_split_index: Some(false),
            html_copy_source: Some(true),
            html_show_sourcelink: Some(true),
            html_sourcelink_suffix: Some(".txt".to_string()),
            html_use_opensearch: None,
            html_file_suffix: Some(".html".to_string()),
            html_link_suffix: Some(".html".to_string()),
            html_show_copyright: Some(true),
            html_show_sphinx: Some(true),
            html_context: HashMap::new(),
            html_output_encoding: Some("utf-8".to_string()),
            html_compact_lists: Some(true),
            html_secnumber_suffix: Some(". ".to_string()),
            html_search_language: None,
            html_search_options: HashMap::new(),
            html_search_scorer: None,
            html_scaled_image_link: Some(true),
            html_baseurl: None,
            html_codeblock_linenos_style: Some("table".to_string()),
            html_math_renderer: Some("mathjax".to_string()),
            html_math_renderer_options: HashMap::new(),
            latex_engine: Some("pdflatex".to_string()),
            latex_documents: Vec::new(),
            latex_logo: None,
            latex_appendices: Vec::new(),
            latex_domain_indices: Some(true),
            latex_show_pagerefs: Some(false),
            latex_show_urls: Some("no".to_string()),
            latex_use_latex_multicolumn: Some(false),
            latex_use_xindy: Some(false),
            latex_toplevel_sectioning: None,
            latex_docclass: HashMap::new(),
            latex_additional_files: Vec::new(),
            latex_elements: HashMap::new(),
            epub_title: None,
            epub_author: None,
            epub_language: None,
            epub_publisher: None,
            epub_copyright: None,
            epub_identifier: None,
            epub_scheme: None,
            epub_uid: None,
            epub_cover: None,
            epub_css_files: Vec::new(),
            epub_pre_files: Vec::new(),
            epub_post_files: Vec::new(),
            epub_exclude_files: Vec::new(),
            epub_tocdepth: Some(3),
            epub_tocdup: Some(true),
            epub_tocscope: Some("default".to_string()),
            epub_fix_images: Some(false),
            epub_max_image_width: Some(0),
            epub_show_urls: Some("inline".to_string()),
            epub_use_index: Some(true),
            epub_description: None,
            epub_contributor: None,
            epub_writing_mode: Some("horizontal".to_string()),
            extension_configs: HashMap::new(),
            needs_sphinx: None,
            needs_extensions: HashMap::new(),
            manpages_url: None,
            nitpicky: Some(false),
            nitpick_ignore: Vec::new(),
            nitpick_ignore_regex: Vec::new(),
            numfig: Some(false),
            numfig_format: HashMap::new(),
            numfig_secnum_depth: Some(1),
            math_number_all: Some(false),
            math_eqref_format: None,
            math_numfig: Some(true),
            tls_verify: Some(true),
            tls_cacerts: None,
            user_agent: None,
            suppress_warnings: Vec::new(),
            show_warning_types: None,
            keep_warnings: None,
            trim_doctest_flags: None,
            smartquotes: None,
            smartquotes_action: None,
            smartquotes_excludes: None,
            primary_domain: None,
            highlight_language: None,
            default_role: None,
            math_numsep: None,
            rst_prolog: None,
            rst_epilog: None,
            // `None` = "conf.py said nothing", which leaves
            // `BuildConfig::default()`'s sphinx defaults untouched.
            maximum_signature_line_length: None,
            python_maximum_signature_line_length: None,
            python_trailing_comma_in_multi_line_signatures: None,
            python_display_short_literal_types: None,
            python_use_unqualified_type_names: None,
            toc_object_entries: None,
            toc_object_entries_show_parents: None,
            source_encoding: None,
            confval_type_mismatches: Vec::new(),
            add_function_parentheses: None,
            add_module_names: None,
            strip_signature_backslash: None,
            modindex_common_prefix: Vec::new(),
            intersphinx_mapping: serde_json::Value::Null,
            intersphinx_disabled_reftypes: None,
            intersphinx_resolve_self: None,
            intersphinx_cache_limit: None,
            intersphinx_timeout: None,
            gettext_uuid: Some(false),
            gettext_location: Some(true),
            gettext_auto_build: Some(true),
            gettext_additional_targets: Vec::new(),
            custom_configs: HashMap::new(),
            raw_values: BTreeMap::new(),
        }
    }
}

impl ConfPyConfig {
    /// Convert conf.py configuration to BuildConfig.
    ///
    /// Nothing fails here today: the one `ConfigError` Sphinx raises at
    /// `config-inited`, `intersphinx_mapping` validation
    /// (`ext/intersphinx/_load.py:131-136`), runs once the `-D` overrides
    /// are in ([`BuildConfig::init_intersphinx_mapping`]), and only with
    /// the extension loaded.
    pub fn to_build_config(&self) -> Result<BuildConfig> {
        let mut config = BuildConfig::default();

        // Map basic project information
        if let Some(project) = &self.project {
            config.project = project.clone();
        }
        if let Some(version) = &self.version {
            config.version = Some(version.clone());
        }
        if let Some(release) = &self.release {
            config.release = Some(release.clone());
        }
        if let Some(today) = &self.today {
            config.today = today.clone();
        }
        config.today_fmt = self.today_fmt.clone();
        if let Some(copyright) = &self.copyright {
            config.copyright = Some(copyright.clone());
        }
        if let Some(language) = &self.language {
            config.language = Some(language.clone());
        }
        if let Some(root_doc) = &self.root_doc {
            config.root_doc = Some(root_doc.clone());
        }

        // Map extensions
        config.extensions = self.extensions.clone();

        // Map template paths
        config.template_dirs = self.templates_path.iter().map(PathBuf::from).collect();

        // Map static paths
        config.static_dirs = self.html_static_path.iter().map(PathBuf::from).collect();
        config.html_static_path = self.html_static_path.iter().map(PathBuf::from).collect();

        // Map HTML configuration
        if let Some(html_theme) = &self.html_theme {
            config.output.html_theme = html_theme.clone();
            config.theme.name = html_theme.clone();
        }
        if let Some(html_title) = &self.html_title {
            config.html_title = Some(html_title.clone());
        }
        if let Some(html_short_title) = &self.html_short_title {
            config.html_short_title = Some(html_short_title.clone());
        }
        if let Some(html_logo) = &self.html_logo {
            config.html_logo = Some(html_logo.clone());
        }
        if let Some(html_favicon) = &self.html_favicon {
            config.html_favicon = Some(html_favicon.clone());
        }
        config.html_css_files = self.html_css_files.clone();
        config.html_js_files = self.html_js_files.clone();
        if let Some(html_show_copyright) = self.html_show_copyright {
            config.html_show_copyright = Some(html_show_copyright);
        }
        if let Some(html_show_sphinx) = self.html_show_sphinx {
            config.html_show_sphinx = Some(html_show_sphinx);
        }
        if let Some(html_copy_source) = self.html_copy_source {
            config.html_copy_source = Some(html_copy_source);
        }
        if let Some(html_show_sourcelink) = self.html_show_sourcelink {
            config.html_show_sourcelink = Some(html_show_sourcelink);
        }
        if let Some(html_sourcelink_suffix) = &self.html_sourcelink_suffix {
            config.html_sourcelink_suffix = Some(html_sourcelink_suffix.clone());
        }
        if let Some(html_use_index) = self.html_use_index {
            config.html_use_index = Some(html_use_index);
        }
        if let Some(html_use_opensearch) = &self.html_use_opensearch {
            config.html_use_opensearch = Some(!html_use_opensearch.is_empty());
        }
        if let Some(html_last_updated_fmt) = &self.html_context.get("last_updated") {
            if let Some(fmt_str) = html_last_updated_fmt.as_str() {
                config.html_last_updated_fmt = Some(fmt_str.to_string());
            }
        }

        // Map templates path
        config.templates_path = self.templates_path.iter().map(PathBuf::from).collect();

        // Map file patterns (Sphinx compatibility)
        config.include_patterns = if self.include_patterns.is_empty() {
            vec!["**".to_string()] // Sphinx default
        } else {
            self.include_patterns.clone()
        };
        config.exclude_patterns = self.exclude_patterns.clone();

        config.nitpicky = self.nitpicky.unwrap_or(false);
        config.nitpick_ignore = self.nitpick_ignore.clone();
        config.nitpick_ignore_regex = self.nitpick_ignore_regex.clone();
        config.html_context = self
            .html_context
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        // Numbering (`numfig` family). `numfig_format` MERGES over the
        // defaults `BuildConfig::default()` seeded — sphinx applies the
        // user dict on top of its own at `config-inited` prio 800
        // (`config.py:682-693`), so a conf.py naming only `figure` keeps
        // `section`/`table`/`code-block`.
        config.numfig = self.numfig.unwrap_or(false);
        for (figtype, format) in &self.numfig_format {
            config.numfig_format.insert(figtype.clone(), format.clone());
        }
        if let Some(depth) = self.numfig_secnum_depth {
            config.numfig_secnum_depth = depth.max(0) as u32;
        }

        // Object-signature / py-domain family: every key that `conf.py`
        // actually named overrides the sphinx default, and nothing else does.
        // The two `Option<i64>` keys assign straight through, because for
        // them "unset" and "set to None" are the same thing in sphinx too.
        config.maximum_signature_line_length = self.maximum_signature_line_length;
        config.python_maximum_signature_line_length = self.python_maximum_signature_line_length;
        config.confval_type_mismatches = self.confval_type_mismatches.clone();
        if let Some(source_encoding) = &self.source_encoding {
            config.source_encoding = source_encoding.clone();
        }
        if let Some(trailing_comma) = self.python_trailing_comma_in_multi_line_signatures {
            config.python_trailing_comma_in_multi_line_signatures = trailing_comma;
        }
        if let Some(short_literals) = self.python_display_short_literal_types {
            config.python_display_short_literal_types = short_literals;
        }
        if let Some(unqualified) = self.python_use_unqualified_type_names {
            config.python_use_unqualified_type_names = unqualified;
        }
        if let Some(toc_object_entries) = self.toc_object_entries {
            config.toc_object_entries = toc_object_entries;
        }
        if let Some(show_parents) = &self.toc_object_entries_show_parents {
            // Carried through even when it is outside the ENUM: sphinx only
            // warns (`BuildConfig::validate`).
            config.toc_object_entries_show_parents = show_parents.clone();
        }
        if let Some(add_parens) = self.add_function_parentheses {
            config.add_function_parentheses = add_parens;
        }
        if let Some(add_module_names) = self.add_module_names {
            config.add_module_names = add_module_names;
        }
        if let Some(strip_backslash) = self.strip_signature_backslash {
            config.strip_signature_backslash = strip_backslash;
        }
        config.modindex_common_prefix = self.modindex_common_prefix.clone();

        // The intersphinx family + the shared HTTP configuration group.
        // `intersphinx_mapping` itself is the extension's to validate, at
        // `config-inited` ([`BuildConfig::init_intersphinx_mapping`]), from
        // the value `conf_values` keeps.
        if let Some(disabled) = &self.intersphinx_disabled_reftypes {
            config.intersphinx_disabled_reftypes = disabled.clone();
        }
        if let Some(resolve_self) = &self.intersphinx_resolve_self {
            config.intersphinx_resolve_self = resolve_self.clone();
        }
        if let Some(limit) = self.intersphinx_cache_limit {
            config.intersphinx_cache_limit = limit;
        }
        config.intersphinx_timeout = self.intersphinx_timeout;
        if let Some(tls_verify) = self.tls_verify {
            config.tls_verify = tls_verify;
        }
        config.tls_cacerts = self.tls_cacerts.clone();
        config.user_agent = self.user_agent.clone();

        config.suppress_warnings = self.suppress_warnings.clone();
        if let Some(show_warning_types) = self.show_warning_types {
            config.show_warning_types = show_warning_types;
        }
        if let Some(primary_domain) = &self.primary_domain {
            config.primary_domain = primary_domain.clone();
        }
        if let Some(highlight_language) = &self.highlight_language {
            config.highlight_language = highlight_language.clone();
        }
        if let Some(default_role) = &self.default_role {
            config.default_role = default_role.clone();
        }
        if self.manpages_url.is_some() {
            config.manpages_url = self.manpages_url.clone();
        }
        config.rst_prolog = self.rst_prolog.clone();
        config.rst_epilog = self.rst_epilog.clone();
        if let Some(math_number_all) = self.math_number_all {
            config.math_number_all = math_number_all;
        }
        if self.math_eqref_format.is_some() {
            config.math_eqref_format = self.math_eqref_format.clone();
        }
        if let Some(math_numfig) = self.math_numfig {
            config.math_numfig = math_numfig;
        }
        if let Some(math_numsep) = &self.math_numsep {
            config.math_numsep = math_numsep.clone();
        }
        if let Some(keep_warnings) = self.keep_warnings {
            config.keep_warnings = keep_warnings;
        }
        if let Some(trim_doctest_flags) = self.trim_doctest_flags {
            config.trim_doctest_flags = trim_doctest_flags;
        }
        if let Some(smartquotes) = self.smartquotes {
            config.smartquotes = smartquotes;
        }
        if let Some(action) = &self.smartquotes_action {
            config.smartquotes_action = action.clone();
        }
        if let Some(excludes) = &self.smartquotes_excludes {
            config.smartquotes_excludes = excludes.clone();
        }

        config.conf_values = self.raw_values.clone();

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> PythonConfigParser {
        let mut parser = PythonConfigParser::new().unwrap();
        parser.parse_statements(content).unwrap();
        parser
    }

    #[test]
    fn multiline_list_parses() {
        let p = parse("extensions = [\n    'sphinx.ext.autodoc',\n    'sphinx.ext.viewcode',\n]\n");
        let v = p.conf_namespace.get("extensions").expect("extensions set");
        let items: Vec<&str> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_str().unwrap())
            .collect();
        assert_eq!(items, vec!["sphinx.ext.autodoc", "sphinx.ext.viewcode"]);
        assert!(p.warnings().is_empty(), "warnings: {:?}", p.warnings());
    }

    #[test]
    fn multiline_dict_parses() {
        let p = parse(
            "html_theme_options = {\n    'collapse_navigation': False,\n    'navigation_depth': 4,\n}\n",
        );
        let v = p
            .conf_namespace
            .get("html_theme_options")
            .expect("dict set");
        let obj = v.as_object().unwrap();
        assert_eq!(
            obj.get("collapse_navigation"),
            Some(&serde_json::Value::Bool(false))
        );
        assert_eq!(
            obj.get("navigation_depth").and_then(|n| n.as_i64()),
            Some(4)
        );
    }

    #[test]
    fn adjacent_string_concat_parses() {
        let p = parse("copyright = ('2024, ' 'Team')\n");
        assert_eq!(
            p.conf_namespace.get("copyright").and_then(|v| v.as_str()),
            Some("2024, Team")
        );
    }

    #[test]
    fn triple_quoted_string_parses() {
        let p = parse("project = \"\"\"Multi\nLine\"\"\"\n");
        assert_eq!(
            p.conf_namespace.get("project").and_then(|v| v.as_str()),
            Some("Multi\nLine")
        );
    }

    #[test]
    fn trailing_comment_stripped() {
        let p = parse("version = '1.0'  # the version\n");
        assert_eq!(
            p.conf_namespace.get("version").and_then(|v| v.as_str()),
            Some("1.0")
        );
    }

    #[test]
    fn unsupported_value_warns_and_drops() {
        let p = parse("project = os.environ['P']\n");
        assert!(!p.conf_namespace.contains_key("project"));
        assert_eq!(p.warnings().len(), 1);
        assert_eq!(p.warnings()[0].line, 1);
        assert!(
            p.warnings()[0].message.contains("project"),
            "warning names the variable: {}",
            p.warnings()[0].message
        );
    }

    #[test]
    fn unsupported_statement_warns_but_imports_do_not() {
        let p = parse("import os\nfrom pathlib import Path\nsys.path.insert(0, 'x')\n");
        assert_eq!(p.warnings().len(), 1, "warnings: {:?}", p.warnings());
        assert_eq!(p.warnings()[0].line, 3);
    }

    #[test]
    fn nested_structures_parse() {
        let p = parse(
            "intersphinx_mapping = {\n    'python': ('https://docs.python.org/3', None),\n}\n",
        );
        let v = p.conf_namespace.get("intersphinx_mapping").unwrap();
        let python = v
            .as_object()
            .unwrap()
            .get("python")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(python[0].as_str(), Some("https://docs.python.org/3"));
        assert!(python[1].is_null());
    }

    /// A number literal read as Python 3.12.2 compiles `x = <text>`: the
    /// type and `str()` of the value, with no warning, or `None` where
    /// Python raises `SyntaxError` (`invalid decimal literal`, `invalid
    /// syntax`, or `leading zeros in decimal integer literals are not
    /// permitted`) and this reader drops the value with a warning. Probed.
    #[test]
    fn number_literals_read_as_python_compiles_them() {
        let read = |text: &str| {
            let p = parse(&format!("x = {text}\n"));
            let value = p.conf_namespace.get("x").map(|value| match value {
                serde_json::Value::Number(number) => (
                    if number.is_f64() { "float" } else { "int" },
                    crate::pyformat::number_str(number),
                ),
                other => panic!("{text}: {other}"),
            });
            (value, p.warnings().len())
        };
        let python: &[(&str, Option<(&str, &str)>)] = &[
            // An exponent: `e` or `E`, a sign or none, then digits that
            // single underscores may separate.
            ("1e-05", Some(("float", "1e-05"))),
            ("-1e-07", Some(("float", "-1e-07"))),
            ("1e+16", Some(("float", "1e+16"))),
            ("+1e+16", Some(("float", "1e+16"))),
            ("1E-5", Some(("float", "1e-05"))),
            ("1e5", Some(("float", "100000.0"))),
            ("1E+5", Some(("float", "100000.0"))),
            ("1e-5_0", Some(("float", "1e-50"))),
            ("1e1_0", Some(("float", "10000000000.0"))),
            ("1e", None),
            ("1e+", None),
            ("1e-", None),
            ("1e_5", None),
            ("1e+_5", None),
            ("1e5_", None),
            ("1e5__0", None),
            ("1_e5", None),
            ("1e5e5", None),
            ("1e--5", None),
            ("1.5e", None),
            ("1e5x", None),
            // A leading or a trailing `.`.
            (".5", Some(("float", "0.5"))),
            ("-.5", Some(("float", "-0.5"))),
            ("+.5", Some(("float", "0.5"))),
            ("5.", Some(("float", "5.0"))),
            ("-5.", Some(("float", "-5.0"))),
            ("5.e3", Some(("float", "5000.0"))),
            (".5e-3", Some(("float", "0.0005"))),
            ("1.e-5", Some(("float", "1e-05"))),
            (".e5", None),
            (".", None),
            ("-.", None),
            ("-", None),
            ("5.5.5", None),
            ("1.2.3", None),
            ("1..5", None),
            // Underscores only between digits.
            ("1_000", Some(("int", "1000"))),
            ("1_000.5", Some(("float", "1000.5"))),
            ("1_000.000_1", Some(("float", "1000.0001"))),
            ("0_0", Some(("int", "0"))),
            ("1_0_0", Some(("int", "100"))),
            ("1__000", None),
            ("1_", None),
            ("1_.5", None),
            ("1._5", None),
            ("1.5_", None),
            ("5._", None),
            ("0__0", None),
            ("0_", None),
            ("1_e", None),
            // Leading zeros: only an integer of zeros, or a float.
            ("0", Some(("int", "0"))),
            ("00", Some(("int", "0"))),
            ("000", Some(("int", "0"))),
            ("007.5", Some(("float", "7.5"))),
            ("007e1", Some(("float", "70.0"))),
            ("00.5", Some(("float", "0.5"))),
            ("0e0", Some(("float", "0.0"))),
            ("09.5", Some(("float", "9.5"))),
            ("-0", Some(("int", "0"))),
            ("-0.0", Some(("float", "-0.0"))),
            ("+0", Some(("int", "0"))),
            ("007", None),
            ("0_7", None),
            ("-007", None),
            // The ends of an `i64` and of the float range.
            ("9223372036854775807", Some(("int", "9223372036854775807"))),
            (
                "-9223372036854775808",
                Some(("int", "-9223372036854775808")),
            ),
            ("1e308", Some(("float", "1e+308"))),
            ("2.5e-324", Some(("float", "5e-324"))),
            ("1e-400", Some(("float", "0.0"))),
        ];
        let mismatches: Vec<String> = python
            .iter()
            .filter_map(|&(text, expected)| {
                let (value, warnings) = read(text);
                let got = value
                    .as_ref()
                    .map(|(kind, number)| (*kind, number.as_str()));
                (got != expected || warnings != usize::from(expected.is_none())).then(|| {
                    format!("{text}: read {got:?} with {warnings} warnings, Python {expected:?}")
                })
            })
            .collect();
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));

        // Numbers Python reads that this reader does not model: the other
        // bases, imaginary literals, and a sign as a unary operator, spelled
        // apart from the literal or twice. Each is dropped with a warning,
        // never misread.
        for text in [
            "0x1F", "0X1f", "0o17", "0O17", "0b101", "0B101", "0x_1F", "1j", "1.5J", "1e5j", "- 5",
            "--5", "-+5", "+-5", "- 1e-5",
        ] {
            assert_eq!(read(text), (None, 1), "{text}");
        }
    }

    #[test]
    fn numfig_family_reaches_the_build_config() {
        let p = parse(
            "numfig = True\nnumfig_secnum_depth = 2\n\
             numfig_format = {'figure': 'Figure %s', 'table': 'Table {number}'}\n",
        );
        let config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();

        assert!(config.numfig);
        assert_eq!(config.numfig_secnum_depth, 2);
        // The user's two entries merge OVER the four defaults rather than
        // replacing them (`config.py:682-693`).
        assert_eq!(config.numfig_format["figure"], "Figure %s");
        assert_eq!(config.numfig_format["table"], "Table {number}");
        assert_eq!(config.numfig_format["section"], "Section %s");
        assert_eq!(config.numfig_format["code-block"], "Listing %s");
    }

    /// `suppress_warnings` is a list or a tuple of patterns and
    /// `show_warning_types` a bool (`config.py:262-263`); a conf.py that
    /// names neither keeps Sphinx's defaults, `[]` and `True`.
    #[test]
    fn warning_stream_keys_reach_the_build_config() {
        let config_of = |source: &str| {
            parse(source)
                .extract_configuration()
                .unwrap()
                .to_build_config()
                .unwrap()
        };

        let listed = config_of(
            "suppress_warnings = ['toc.not_included', 'ref.*']\nshow_warning_types = False\n",
        );
        assert_eq!(listed.suppress_warnings, ["toc.not_included", "ref.*"]);
        assert!(!listed.show_warning_types);

        let tupled = config_of("suppress_warnings = ('docutils',)\n");
        assert_eq!(tupled.suppress_warnings, ["docutils"]);
        assert!(tupled.show_warning_types);

        let unset = config_of("project = 'p'\n");
        assert!(unset.suppress_warnings.is_empty());
        assert!(unset.show_warning_types);
    }

    #[test]
    fn nitpick_ignore_lists_reach_the_build_config() {
        let p = parse(
            "nitpicky = True\n\
             nitpick_ignore = [('py:func', 'nope'), ('doc', 'missing')]\n\
             nitpick_ignore_regex = [(r'std:.*', r'legacy-.*')]\n",
        );
        let config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();

        assert!(config.nitpicky);
        assert_eq!(
            config.nitpick_ignore,
            vec![
                ("py:func".to_string(), "nope".to_string()),
                ("doc".to_string(), "missing".to_string()),
            ]
        );
        assert_eq!(
            config.nitpick_ignore_regex,
            vec![("std:.*".to_string(), "legacy-.*".to_string())]
        );
    }

    /// The eleven object-signature / py-domain keys must reach `BuildConfig`
    /// from `conf.py`, with the defaults surviving for anything the file
    /// does not mention.
    #[test]
    fn the_object_signature_family_reaches_the_build_config() {
        let p = parse(
            "maximum_signature_line_length = 88\n\
             python_maximum_signature_line_length = 0\n\
             python_trailing_comma_in_multi_line_signatures = False\n\
             python_display_short_literal_types = True\n\
             python_use_unqualified_type_names = True\n\
             toc_object_entries = False\n\
             toc_object_entries_show_parents = 'all'\n\
             add_function_parentheses = False\n\
             add_module_names = False\n\
             strip_signature_backslash = True\n\
             modindex_common_prefix = ['mypkg.', 'other.']\n",
        );
        let config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();

        assert_eq!(config.maximum_signature_line_length, Some(88));
        assert_eq!(
            config.python_maximum_signature_line_length,
            Some(0),
            "an explicit 0 is NOT the same as unset — max_len()'s truthiness \
             fall-through depends on carrying it through unchanged"
        );
        assert!(!config.python_trailing_comma_in_multi_line_signatures);
        assert!(config.python_display_short_literal_types);
        assert!(config.python_use_unqualified_type_names);
        assert!(!config.toc_object_entries);
        assert_eq!(config.toc_object_entries_show_parents, "all");
        assert!(!config.add_function_parentheses);
        assert!(!config.add_module_names);
        assert!(config.strip_signature_backslash);
        assert_eq!(
            config.modindex_common_prefix,
            vec!["mypkg.".to_string(), "other.".to_string()]
        );

        // A conf.py that mentions none of them keeps sphinx's defaults.
        let untouched = parse("project = 'x'\n")
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        assert_eq!(untouched.maximum_signature_line_length, None);
        assert!(untouched.add_function_parentheses);
        assert!(untouched.toc_object_entries);
        assert_eq!(untouched.toc_object_entries_show_parents, "domain");
    }

    /// A key this crate maps is a *standard* key: it must not also be
    /// dumped into `custom_configs`, which is the catch-all for settings
    /// only an extension understands.
    #[test]
    fn the_object_signature_family_is_not_treated_as_custom_config() {
        let p = parse(
            "maximum_signature_line_length = 88\n\
             python_maximum_signature_line_length = 40\n\
             python_trailing_comma_in_multi_line_signatures = False\n\
             python_display_short_literal_types = True\n\
             python_use_unqualified_type_names = True\n\
             toc_object_entries = False\n\
             toc_object_entries_show_parents = 'all'\n\
             add_function_parentheses = False\n\
             add_module_names = False\n\
             strip_signature_backslash = True\n\
             modindex_common_prefix = ['mypkg.']\n\
             nitpick_ignore = [('py:func', 'nope')]\n\
             nitpick_ignore_regex = [('py:.*', 'nope.*')]\n\
             my_extension_knob = 3\n",
        );
        let config = p.extract_configuration().unwrap();

        assert_eq!(
            config.custom_configs.keys().collect::<Vec<_>>(),
            vec!["my_extension_knob"],
            "only the genuinely unknown key is custom: {:?}",
            config.custom_configs
        );
    }

    #[test]
    fn raw_and_prefixed_string_literals_parse() {
        // A raw Rust literal: what follows is byte-for-byte what conf.py holds.
        let p = parse(
            r"a = r'back\slash'
b = R'\d+'
c = u'plain'
d = rb'bytes'
e = 'esc\n'
",
        );
        let ns = |key: &str| p.conf_namespace[key].as_str().unwrap().to_string();
        assert_eq!(
            ns("a"),
            r"back\slash",
            "a raw literal keeps its backslashes"
        );
        assert_eq!(ns("b"), r"\d+");
        assert_eq!(ns("c"), "plain");
        assert_eq!(ns("d"), "bytes");
        assert_eq!(ns("e"), "esc\n", "a plain literal still translates escapes");
    }

    #[test]
    fn the_intersphinx_family_reaches_the_build_config() {
        let p = parse(
            "extensions = ['sphinx.ext.intersphinx']\n\
             intersphinx_mapping = {\n\
             'python': ('https://docs.python.org/3', None),\n\
             'other': ('https://example.org/', 'local.inv'),\n\
             }\n\
             intersphinx_disabled_reftypes = ['std:doc', 'std:label']\n\
             intersphinx_resolve_self = 'mine'\n\
             intersphinx_cache_limit = 0\n\
             intersphinx_timeout = 2.5\n\
             tls_verify = False\n\
             tls_cacerts = '/etc/ca.pem'\n\
             user_agent = 'mine/1'\n",
        );
        let mut config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        config
            .init_intersphinx_mapping()
            .expect("a valid mapping must not fail configuration");

        assert_eq!(
            config.intersphinx_mapping["python"],
            ("https://docs.python.org/3".to_string(), vec![None])
        );
        assert_eq!(
            config.intersphinx_mapping["other"],
            (
                "https://example.org/".to_string(),
                vec![Some("local.inv".to_string())]
            )
        );
        assert_eq!(
            config.intersphinx_disabled_reftypes,
            vec!["std:doc".to_string(), "std:label".to_string()]
        );
        assert_eq!(config.intersphinx_resolve_self, "mine");
        assert_eq!(config.intersphinx_cache_limit, 0);
        assert_eq!(config.intersphinx_timeout, Some(2.5));
        assert!(!config.tls_verify);
        assert_eq!(
            config.tls_cacerts,
            Some(crate::intersphinx::TlsCacerts::Bundle(
                "/etc/ca.pem".to_string()
            ))
        );
        assert_eq!(config.user_agent, Some("mine/1".to_string()));
    }

    #[test]
    fn tls_cacerts_accepts_the_per_host_mapping_form() {
        let p = parse("tls_cacerts = {'docs.example.org': '/etc/example.pem'}\n");
        let config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        assert_eq!(
            config.tls_cacerts,
            Some(crate::intersphinx::TlsCacerts::PerHost(
                std::collections::BTreeMap::from([(
                    "docs.example.org".to_string(),
                    "/etc/example.pem".to_string()
                )])
            ))
        );
    }

    /// `validate_intersphinx_mapping` is a `config-inited` handler of the
    /// extension (`ext/intersphinx/_load.py:38-136`): a malformed entry
    /// aborts the configuration only with `sphinx.ext.intersphinx` loaded,
    /// after the per-entry errors Sphinx logs; without it the mapping is an
    /// ordinary variable and nothing loads. Loading the `conf.py` never
    /// fails on it, so the `-D` overrides apply first.
    #[test]
    fn an_invalid_intersphinx_mapping_stops_the_configuration_with_the_extension() {
        let p = parse(
            "extensions = ['sphinx.ext.intersphinx']\nintersphinx_mapping = {'p': 'https://x/'}\n",
        );
        let mut config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .expect("loading does not validate the mapping");
        let err = config
            .init_intersphinx_mapping()
            .expect_err("a malformed entry must abort");
        assert_eq!(
            err.to_string(),
            "Invalid `intersphinx_mapping` configuration (1 error)."
        );
        assert_eq!(
            err.errors,
            ["Invalid value `'https://x/'` in intersphinx_mapping['p']. \
              Expected a two-element tuple or list."]
        );

        let p = parse("intersphinx_mapping = {'p': 'https://x/'}\n");
        let mut config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        config
            .init_intersphinx_mapping()
            .expect("without the extension the mapping is not validated");
        assert!(config.intersphinx_mapping.is_empty());
    }

    /// `primary_domain` (`config.py:268`, `None` allowed) and
    /// `highlight_language` (`config.py:257`) are standard keys read into
    /// the build configuration: an explicit `None` unsets the primary
    /// domain, and unset, each keeps Sphinx's default.
    #[test]
    fn read_side_keys_reach_the_build_config() {
        let p = parse("primary_domain = None\nhighlight_language = 'python3'\n");
        let config = p.extract_configuration().unwrap();
        assert!(!config.custom_configs.contains_key("primary_domain"));
        assert!(!config.custom_configs.contains_key("highlight_language"));
        let config = config.to_build_config().unwrap();
        assert_eq!(config.primary_domain, None);
        assert_eq!(config.highlight_language, "python3");

        let p = parse("primary_domain = 'js'\n");
        let config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        assert_eq!(config.primary_domain.as_deref(), Some("js"));

        let p = parse("project = 'x'\n");
        let config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        assert_eq!(config.primary_domain.as_deref(), Some("py"));
        assert_eq!(config.highlight_language, "default");
    }

    /// The read-side keys of wave 5a T17a — `default_role` (`config.py:247`,
    /// `None` allowed), `manpages_url` (`:271`) and the math domain's
    /// `math_number_all`, `math_eqref_format`, `math_numfig` and
    /// `math_numsep` (`:282-285`) — are standard keys read into the build
    /// configuration; unset, each keeps Sphinx's default.
    #[test]
    fn role_and_math_keys_reach_the_build_config() {
        let p = parse(
            "default_role = 'any'\nmanpages_url = 'https://man/{page}'\n\
             math_number_all = True\nmath_eqref_format = 'Eq. {number}'\n\
             math_numfig = False\nmath_numsep = '-'\n",
        );
        let config = p.extract_configuration().unwrap();
        for key in [
            "default_role",
            "manpages_url",
            "math_number_all",
            "math_eqref_format",
            "math_numfig",
            "math_numsep",
        ] {
            assert!(!config.custom_configs.contains_key(key), "{key}");
        }
        let config = config.to_build_config().unwrap();
        assert_eq!(config.default_role.as_deref(), Some("any"));
        assert_eq!(config.manpages_url.as_deref(), Some("https://man/{page}"));
        assert!(config.math_number_all);
        assert_eq!(config.math_eqref_format.as_deref(), Some("Eq. {number}"));
        assert!(!config.math_numfig);
        assert_eq!(config.math_numsep, "-");

        let config = parse("default_role = None\n")
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        assert_eq!(config.default_role, None);

        let config = parse("project = 'x'\n")
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        assert_eq!(
            (
                config.default_role,
                config.manpages_url,
                config.math_number_all,
                config.math_eqref_format,
                config.math_numfig,
                config.math_numsep.as_str()
            ),
            (None, None, false, None, true, ".")
        );
    }

    /// `rst_prolog` and `rst_epilog` (`config.py:265-266`) are standard keys
    /// read into the build configuration, `None` unset or set to `None`.
    #[test]
    fn prolog_and_epilog_reach_the_build_config() {
        let p = parse(
            "rst_prolog = '.. |a| replace:: A\\n'\n\
             rst_epilog = \"\"\"\n.. |b| replace:: B\n\"\"\"\n",
        );
        let config = p.extract_configuration().unwrap();
        for key in ["rst_prolog", "rst_epilog"] {
            assert!(!config.custom_configs.contains_key(key), "{key}");
        }
        let config = config.to_build_config().unwrap();
        assert_eq!(config.rst_prolog.as_deref(), Some(".. |a| replace:: A\n"));
        assert_eq!(config.rst_epilog.as_deref(), Some("\n.. |b| replace:: B\n"));

        for content in ["rst_prolog = None\nrst_epilog = None\n", "project = 'x'\n"] {
            let config = parse(content)
                .extract_configuration()
                .unwrap()
                .to_build_config()
                .unwrap();
            assert_eq!((config.rst_prolog, config.rst_epilog), (None, None));
        }
    }

    /// `check_confval_types` warns about a prolog or epilog that is no
    /// `str`, `rst_epilog` first (`config.py:265-266`); Sphinx's reader
    /// then adds nothing for a false value (Sphinx 9.1.0, scratch
    /// `t17b/specs/prolog_false.json`).
    #[test]
    fn a_mistyped_prolog_or_epilog_is_recorded_not_read() {
        let config = parse("rst_prolog = False\nrst_epilog = 0\n")
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        assert_eq!((&config.rst_prolog, &config.rst_epilog), (&None, &None));
        assert_eq!(
            config.validate(),
            [
                "The config value `rst_epilog' has type `int'; expected `str'.",
                "The config value `rst_prolog' has type `bool'; expected `str'.",
            ]
        );
    }

    /// `rst_prolog` and `rst_epilog` are read before every document, so a
    /// value this reader cannot evaluate fails the configuration, naming
    /// the key, instead of being dropped (wave 5a T17b): an expression it
    /// cannot evaluate, an augmented assignment other than a top-level
    /// `+=` of a string literal to a string it holds, an assignment inside
    /// a block, one it cannot split, or an import of the name. A later
    /// top-level assignment it can evaluate is the value Python's last
    /// binding leaves, and reading the keys binds nothing.
    #[test]
    fn unevaluable_prolog_or_epilog_fails_the_parse() {
        type Expected = Result<&'static str, (&'static str, usize, &'static str)>;
        let rows: [(&str, &str, Expected); 23] = [
            (
                "f-string",
                "rst_prolog = f\"x {version}\"\n",
                Err(("rst_prolog", 1, "unsupported expression")),
            ),
            (
                "str.format",
                "version = '1'\nrst_epilog = \"\"\"a{v}\"\"\".format(v=version)\n",
                Err(("rst_epilog", 2, "trailing expression")),
            ),
            (
                "a name",
                "PROLOG = 'a'\nrst_prolog = PROLOG\n",
                Err(("rst_prolog", 2, "unsupported expression")),
            ),
            (
                "+= of an f-string",
                "rst_epilog = 'a'\nrst_epilog += f'b'\n",
                Err(("rst_epilog", 2, "an augmented assignment")),
            ),
            (
                "+= to an unset name",
                "rst_epilog += 'b'\n",
                Err(("rst_epilog", 1, "an augmented assignment")),
            ),
            (
                "+= to None",
                "rst_prolog = None\nrst_prolog += 'b'\n",
                Err(("rst_prolog", 2, "an augmented assignment")),
            ),
            (
                "+= of a number",
                "rst_prolog = 'a'\nrst_prolog += 1\n",
                Err(("rst_prolog", 2, "an augmented assignment")),
            ),
            (
                "*=",
                "rst_prolog = 'a'\nrst_prolog *= 2\n",
                Err(("rst_prolog", 2, "an augmented assignment")),
            ),
            (
                "in an if block",
                "if x:\n    rst_prolog = 'a'\n",
                Err(("rst_prolog", 2, "an assignment inside a block")),
            ),
            (
                "in a try block",
                "try:\n    rst_prolog = 'a'\nexcept ImportError:\n    pass\n",
                Err(("rst_prolog", 2, "an assignment inside a block")),
            ),
            (
                "+= in a with block",
                "with open('defs.rst.inc') as f:\n    rst_epilog += f.read()\n",
                Err(("rst_epilog", 2, "an assignment inside a block")),
            ),
            (
                "CPython's conf.py",
                "rst_epilog = f\"\"\"\n|{v}|\n\"\"\"\nif lc:\n    rst_epilog += f\"\"\"\\\n.. _T: x\n\"\"\"\n\
                 else:\n    rst_epilog += \"\"\"\\\n.. _T: y\n\"\"\"\n",
                Err(("rst_epilog", 1, "unsupported expression")),
            ),
            (
                "a tuple target",
                "rst_prolog, rst_epilog = make()\n",
                Err(("rst_prolog", 1, "an assignment it cannot evaluate")),
            ),
            (
                "a chained assignment",
                "a = rst_epilog = 'x'\n",
                Err(("rst_epilog", 1, "an assignment it cannot evaluate")),
            ),
            (
                "an unevaluable binding after the last evaluated one",
                "rst_prolog = f'x'\nrst_prolog = 'a'\nif y:\n    rst_prolog += 'b'\n",
                Err(("rst_prolog", 4, "an assignment inside a block")),
            ),
            (
                "a later literal",
                "rst_prolog = f'x'\nrst_prolog = 'plain'\n",
                Ok("plain"),
            ),
            (
                "+= of string literals",
                "rst_prolog = 'a'\nrst_prolog += 'b' 'c'\nrst_prolog += \"\"\"\nd\"\"\"\n",
                Ok("abc\nd"),
            ),
            (
                "reads bind nothing",
                "rst_prolog = 'a'\nx = rst_prolog + 'b'\nhtml_context = {'p': rst_prolog}\n",
                Ok("a"),
            ),
            (
                "an unrelated block",
                "rst_prolog = 'a'\nif x:\n    y = rst_prolog\n",
                Ok("a"),
            ),
            (
                "an import",
                "rst_prolog = 'a'\nfrom shared import rst_prolog\n",
                Err(("rst_prolog", 2, "an import")),
            ),
            (
                "an import's alias, parenthesized",
                "from shared import (\n    x as rst_epilog,\n    y,\n)\n",
                Err(("rst_epilog", 1, "an import")),
            ),
            (
                "an import before a literal",
                "from shared import rst_prolog\nrst_prolog = 'plain'\n",
                Ok("plain"),
            ),
            (
                "importing other names",
                "import rst_prolog_tools as tools\nfrom shared import rst_prologue\n\
                 rst_prolog = 'a'\n",
                Ok("a"),
            ),
        ];
        for (name, content, expected) in rows {
            let p = parse(content);
            let unevaluable = p
                .unevaluable()
                .map(|binding| (binding.key, binding.line, binding.reason.as_str()));
            match expected {
                Err(binding) => {
                    assert_eq!(unevaluable, Some(binding), "{name}");
                    // The configuration error replaces the statement's
                    // drop warning.
                    assert!(
                        p.warnings().iter().all(|warning| warning.line != binding.1),
                        "{name}: {:?}",
                        p.warnings()
                    );
                }
                Ok(value) => {
                    assert_eq!(unevaluable, None, "{name}");
                    assert_eq!(
                        p.conf_namespace.get("rst_prolog"),
                        Some(&serde_json::Value::String(value.to_string())),
                        "{name}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_conf_py_without_numfig_keeps_the_defaults() {
        let p = parse("project = 'Docs'\n");
        let config = p
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();

        assert!(!config.numfig);
        assert_eq!(config.numfig_secnum_depth, 1);
        assert_eq!(config.numfig_format["figure"], "Fig. %s");
    }

    /// conf.py side of `check_confval_types` for the two `int | None` keys
    /// (panel fix round B, [18]): an int or `None` is taken as is; any
    /// other literal is recorded with its python type name for
    /// `BuildConfig::validate` to report, and the key stays unset.
    #[test]
    fn a_mistyped_none_default_int_key_in_conf_py_is_recorded_not_coerced() {
        let p = parse(
            "maximum_signature_line_length = '88'\n\
             python_maximum_signature_line_length = 42\n",
        );
        let config = p.extract_configuration().unwrap();
        assert_eq!(config.maximum_signature_line_length, None);
        assert_eq!(config.python_maximum_signature_line_length, Some(42));
        assert_eq!(
            config.confval_type_mismatches,
            vec![(
                "maximum_signature_line_length".to_string(),
                "str".to_string()
            )]
        );
        let build = config.to_build_config().unwrap();
        assert_eq!(build.maximum_signature_line_length, None);
        assert_eq!(
            build.validate(),
            vec![
                "The config value `maximum_signature_line_length' has type `str'; expected \
                 `NoneType' or `int'."
                    .to_string()
            ]
        );

        for (literal, type_name) in [
            ("88.0", "float"),
            ("True", "bool"),
            ("[88]", "list"),
            ("{'a': 1}", "dict"),
        ] {
            let p = parse(&format!(
                "python_maximum_signature_line_length = {literal}\n"
            ));
            let config = p.extract_configuration().unwrap();
            assert_eq!(
                config.python_maximum_signature_line_length, None,
                "{literal}"
            );
            assert_eq!(
                config.confval_type_mismatches,
                vec![(
                    "python_maximum_signature_line_length".to_string(),
                    type_name.to_string()
                )],
                "{literal}"
            );
        }

        // `None` and an absent key are the same thing, and neither is a
        // mismatch.
        let p = parse("maximum_signature_line_length = None\n");
        let config = p.extract_configuration().unwrap();
        assert_eq!(config.maximum_signature_line_length, None);
        assert!(config.confval_type_mismatches.is_empty());
    }

    /// `source_encoding` is a standard key: read from conf.py, handed to
    /// the build configuration, and never dropped into `custom_configs`.
    #[test]
    fn source_encoding_is_read_from_conf_py() {
        let p = parse("source_encoding = 'latin-1'\n");
        let config = p.extract_configuration().unwrap();
        assert_eq!(config.source_encoding.as_deref(), Some("latin-1"));
        assert!(!config.custom_configs.contains_key("source_encoding"));
        assert_eq!(config.to_build_config().unwrap().source_encoding, "latin-1");

        let p = parse("project = 'x'\n");
        let config = p.extract_configuration().unwrap();
        assert_eq!(config.source_encoding, None);
        assert_eq!(
            config.to_build_config().unwrap().source_encoding,
            "utf-8-sig"
        );
    }

    /// `keep_warnings` is a standard key (`config.py:261`): read from
    /// conf.py into the build configuration, never into `custom_configs`;
    /// unset, it keeps Sphinx's `False`.
    #[test]
    fn keep_warnings_is_read_from_conf_py() {
        let p = parse("keep_warnings = True\n");
        let config = p.extract_configuration().unwrap();
        assert!(!config.custom_configs.contains_key("keep_warnings"));
        assert!(config.to_build_config().unwrap().keep_warnings);

        let p = parse("keep_warnings = False\n");
        let config = p.extract_configuration().unwrap();
        assert!(!config.to_build_config().unwrap().keep_warnings);

        let p = parse("project = 'x'\n");
        let config = p.extract_configuration().unwrap();
        assert!(!config.to_build_config().unwrap().keep_warnings);
    }

    /// `trim_doctest_flags` is a standard key (`config.py:267`): read from
    /// conf.py into the build configuration, never into `custom_configs`;
    /// unset, it keeps Sphinx's `True`.
    #[test]
    fn trim_doctest_flags_is_read_from_conf_py() {
        let p = parse("trim_doctest_flags = False\n");
        let config = p.extract_configuration().unwrap();
        assert!(!config.custom_configs.contains_key("trim_doctest_flags"));
        assert!(!config.to_build_config().unwrap().trim_doctest_flags);

        let p = parse("trim_doctest_flags = True\n");
        let config = p.extract_configuration().unwrap();
        assert!(config.to_build_config().unwrap().trim_doctest_flags);

        let p = parse("project = 'x'\n");
        let config = p.extract_configuration().unwrap();
        assert!(config.to_build_config().unwrap().trim_doctest_flags);
    }

    /// The smart-quotes keys are standard keys (`config.py:289-295`): read
    /// from conf.py into the build configuration, never into
    /// `custom_configs`. Only `smartquotes = False` turns smart quotes off:
    /// Sphinx tests the value with `is False`
    /// (`SP/sphinx/transforms/__init__.py:389`), so `0`, `None` and `''`
    /// keep them on, as an unset key does. A `smartquotes_excludes` dict is
    /// read key by key with `.get(key, [])`, so a key it leaves out
    /// excludes nothing, and each value as `in` tests it: a list or a
    /// tuple by its names, a string as a string, a dict by its keys.
    //
    // oracle (Sphinx 9.1.0, [SQ] S2 `probe_conftypes.out`): with
    //   `smartquotes = 0`, `None` or `''` in conf.py, `"Hi" -- it's` is
    //   educated, as it is unset. The values' `in`: `settings_for`'s test.
    #[test]
    fn smartquotes_keys_are_read_from_conf_py() {
        use crate::config::{ExcludedNames, SmartquotesExcludes};

        let build = |source: &str| {
            parse(source)
                .extract_configuration()
                .unwrap()
                .to_build_config()
                .unwrap()
        };
        assert!(!build("smartquotes = False\n").smartquotes);
        for on in [
            "smartquotes = True\n",
            "smartquotes = 0\n",
            "smartquotes = None\n",
            "smartquotes = ''\n",
            "project = 'x'\n",
        ] {
            assert!(build(on).smartquotes, "{on:?}");
        }

        let config = parse(
            "smartquotes_action = 'qe'\n\
             smartquotes_excludes = {'builders': ['dummy', 'man'], 'other': ['x']}\n",
        )
        .extract_configuration()
        .unwrap();
        for key in ["smartquotes_action", "smartquotes_excludes"] {
            assert!(!config.custom_configs.contains_key(key), "{key}");
        }
        let built = config.to_build_config().unwrap();
        assert_eq!(built.smartquotes_action, "qe");
        let names = |values: &[&str]| {
            ExcludedNames::Names(values.iter().map(|value| value.to_string()).collect())
        };
        assert_eq!(
            built.smartquotes_excludes,
            SmartquotesExcludes {
                languages: ExcludedNames::default(),
                builders: names(&["dummy", "man"]),
            }
        );
        assert!(!parse("smartquotes = 1\n")
            .extract_configuration()
            .unwrap()
            .custom_configs
            .contains_key("smartquotes"));

        let unset = build("project = 'x'\n");
        assert_eq!(unset.smartquotes_action, "qDe");
        assert_eq!(unset.smartquotes_excludes, SmartquotesExcludes::default());
        assert_eq!(
            build("smartquotes_excludes = {'languages': 'ja', 'builders': ('html',)}\n")
                .smartquotes_excludes,
            SmartquotesExcludes {
                languages: ExcludedNames::Text("ja".to_string()),
                builders: names(&["html"]),
            }
        );
        assert_eq!(
            build("smartquotes_excludes = {'builders': {'html': 1}, 'languages': None}\n")
                .smartquotes_excludes,
            SmartquotesExcludes {
                languages: ExcludedNames::default(),
                builders: names(&["html"]),
            }
        );
    }

    /// A `-D smartquotes_excludes.<key>` override goes into the dict
    /// conf.py assigned, whose other key stays (with none, it starts an
    /// empty one: `config::tests::smartquotes_keys_take_sphinxs_override_grammar`).
    //
    // oracle (Sphinx 9.1.0, `-b html`, wave 5a T16b fix round 1, scratch
    //   `t16bf/probe_excludes.out`): with `language = 'ja'` and
    //   `smartquotes_excludes = {'languages': ['ja'], 'builders': []}`,
    //   `-D smartquotes_excludes.builders=man` leaves `"Hi" -- it's`.
    #[test]
    fn a_dotted_excludes_override_keeps_the_conf_py_dict() {
        use crate::config::{ExcludedNames, SmartquotesExcludes};

        let mut config = parse(
            "language = 'ja'\n\
             smartquotes_excludes = {'languages': ['ja'], 'builders': []}\n",
        )
        .extract_configuration()
        .unwrap()
        .to_build_config()
        .unwrap();
        assert!(config
            .apply_override("smartquotes_excludes.builders", "man")
            .unwrap()
            .is_none());
        assert_eq!(
            config.smartquotes_excludes,
            SmartquotesExcludes {
                languages: ExcludedNames::Names(vec!["ja".to_string()]),
                builders: ExcludedNames::Text("man".to_string()),
            }
        );
        assert_eq!(
            config.conf_values.get("smartquotes_excludes"),
            Some(&serde_json::json!({"languages": ["ja"], "builders": "man"}))
        );
        assert_eq!(
            crate::transforms::smartquotes::settings_for(&config, "ja", "html"),
            None
        );

        // Without the conf.py dict, the override's dict has no `languages`.
        let mut config = parse("language = 'ja'\n")
            .extract_configuration()
            .unwrap()
            .to_build_config()
            .unwrap();
        assert!(config
            .apply_override("smartquotes_excludes.builders", "man")
            .unwrap()
            .is_none());
        assert!(crate::transforms::smartquotes::settings_for(&config, "ja", "html").is_some());
    }

    /// `check_confval_types` (`config.py:775-847`) on `smartquotes`, a
    /// `bool`: a value whose type shares no base with `bool` warns, after
    /// the `int | None` key Sphinx registers before it and before the one
    /// the python domain registers, and Sphinx keeps it, so smart quotes
    /// stay on; an `int`, `bool`'s base, does not warn, and neither does a
    /// value a `-D smartquotes=0|1` override replaces. A tuple is read as
    /// a list here, so its warning names `list` where Sphinx's names
    /// `tuple` (`python_type_name`).
    //
    // oracle (Sphinx 9.1.0, `-b html` builds, wave 5a T16b fix round 1,
    //   scratch `t16bf/probe_conftypes.out`): each warning line as
    //   asserted, `WARNING: ` and the text with no location and no type, on
    //   stderr and in the `-w` file; none for `0`, `1`, `False`, or `None`
    //   with `-D smartquotes=0` or `=1`; the four in the order asserted
    //   (the frozenset's members print in Python's hash order, which the
    //   ENUM warning here spells in registration order, as before).
    #[test]
    fn a_mistyped_smartquotes_warns_like_check_confval_types() {
        let build = |source: &str| {
            parse(source)
                .extract_configuration()
                .unwrap()
                .to_build_config()
                .unwrap()
        };
        let warning = |type_name: &str| {
            format!("The config value `smartquotes' has type `{type_name}'; expected `bool'.")
        };
        for (literal, type_name) in [
            ("None", "NoneType"),
            ("''", "str"),
            ("'yes'", "str"),
            ("1.5", "float"),
            ("[]", "list"),
            ("{}", "dict"),
        ] {
            let config = build(&format!("smartquotes = {literal}\n"));
            assert!(config.smartquotes, "{literal}");
            assert_eq!(config.validate(), [warning(type_name)], "{literal}");
        }
        for literal in ["0", "1", "True", "False"] {
            let config = build(&format!("smartquotes = {literal}\n"));
            assert!(config.validate().is_empty(), "{literal}");
        }
        for (value, on) in [("0", false), ("1", true)] {
            let mut config = build("smartquotes = None\n");
            assert!(config
                .apply_override("smartquotes", value)
                .unwrap()
                .is_none());
            assert_eq!(config.smartquotes, on);
            assert!(config.validate().is_empty(), "-D smartquotes={value}");
        }

        let config = build(
            "toc_object_entries_show_parents = 'bogus'\n\
             maximum_signature_line_length = 'a'\n\
             smartquotes = None\n\
             python_maximum_signature_line_length = 'b'\n",
        );
        assert_eq!(
            config.validate(),
            [
                "The config value `toc_object_entries_show_parents` has to be a one of \
                 frozenset({'domain', 'all', 'hide'}), but `bogus` is given."
                    .to_string(),
                "The config value `maximum_signature_line_length' has type `str'; expected \
                 `NoneType' or `int'."
                    .to_string(),
                warning("NoneType"),
                "The config value `python_maximum_signature_line_length' has type `str'; \
                 expected `NoneType' or `int'."
                    .to_string(),
            ]
        );
    }

    /// `today` and `today_fmt` are standard keys (`config.py:227-229`):
    /// read from conf.py into the build configuration, never into
    /// `custom_configs`; unset, or `today_fmt = None`, they keep Sphinx's
    /// `''` and `None`.
    #[test]
    fn today_and_today_fmt_are_read_from_conf_py() {
        let p = parse("today = 'April 1'\ntoday_fmt = '%Y-%m-%d'\n");
        let config = p.extract_configuration().unwrap();
        assert!(!config.custom_configs.contains_key("today"));
        assert!(!config.custom_configs.contains_key("today_fmt"));
        let build = config.to_build_config().unwrap();
        assert_eq!(build.today, "April 1");
        assert_eq!(build.today_fmt.as_deref(), Some("%Y-%m-%d"));

        for source in ["project = 'x'\n", "today_fmt = None\n"] {
            let build = parse(source)
                .extract_configuration()
                .unwrap()
                .to_build_config()
                .unwrap();
            assert_eq!(build.today, "", "{source:?}");
            assert_eq!(build.today_fmt, None, "{source:?}");
        }
    }
}
