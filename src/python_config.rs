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
    /// cannot evaluate, the first such deferred one ([`Pending`]) a `-D`
    /// override replaces, and the first one none does
    /// ([`Unevaluable::overridable`]): at most three per key.
    unevaluable: Vec<Pending>,
    /// For a key of [`MUST_EVALUATE`] the namespace holds, the Python type
    /// of the literal it holds (`type(value).__name__`), which its JSON
    /// form cannot always tell: a tuple's is an array, as a list's is, and
    /// an empty `bytes`, which reads as no value, has none.
    literal_types: HashMap<&'static str, &'static str>,
}

/// An [`Unevaluable`] binding while the parse goes on.
#[derive(Debug, Clone)]
struct Pending {
    binding: Unevaluable,
    /// Whether the module's name is bound once the statements so far have
    /// run: a top-level statement binds it whenever `conf.py` runs, one in
    /// a block may not.
    definite: bool,
    /// Whether the binding runs when something calls the function it is
    /// in (`setup(app)`, an event handler), or changes an attribute of
    /// another object (`app.config.rst_prolog`): after every top-level
    /// statement, so no later statement binds the key again.
    deferred: bool,
}

/// The keys whose `conf.py` value this reader must evaluate or fail the
/// configuration with, unless a `-D` override sets them and replaces what
/// `conf.py` binds them to (wave 5a T17b; [`Unevaluable::overridable`]):
/// Sphinx reads `rst_prolog` and `rst_epilog` around every document, so a
/// value dropped would read every document differently.
const MUST_EVALUATE: [&str; 2] = ["rst_prolog", "rst_epilog"];

/// A conf.py construct that could not be parsed and was dropped.
#[derive(Debug, Clone)]
pub struct ConfigWarning {
    /// 1-based line in conf.py where the construct starts.
    pub line: usize,
    pub message: String,
}

/// A statement binding `rst_prolog` or `rst_epilog` that this reader
/// cannot evaluate: a configuration error, unless it is
/// [`Self::overridable`] and a `-D` override sets the key
/// ([`crate::config::BuildConfig::check_evaluated`]), as Sphinx then
/// never reads what `conf.py` bound it to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unevaluable {
    /// The key the statement binds.
    pub key: &'static str,
    /// 1-based line in conf.py where the statement starts.
    pub line: usize,
    /// Why the reader cannot evaluate it.
    pub reason: String,
    /// The configuration file's name, as the message names it
    /// (`conf.py`); empty for a parse of a text with no file.
    pub file: String,
    /// Whether a `-D` override of the key replaces the binding, as it
    /// replaces any binding of the module's name: Sphinx 9.1.0's
    /// `Config.__getattr__` takes the override before `conf.py`'s
    /// namespace (`config.py:446-465`). An assignment to an attribute
    /// (`app.config.rst_prolog = ...` in `setup(app)`, `config.rst_epilog
    /// += ...` in a `config-inited` handler) is not replaced:
    /// `Config.__setattr__` (`:434-444`) stores it in the instance's
    /// `__dict__`, which Python reads before it ever calls `__getattr__`,
    /// so Sphinx reads it over any override (scratch
    /// `t17b-fix2/specs/attr_*`).
    pub overridable: bool,
}

impl std::fmt::Display for Unevaluable {
    /// The configuration error: Sphinx names its configuration file the
    /// same way when it cannot compile it, `(conf.py, line N)`. It offers
    /// the `-D` override that builds without an edit to `conf.py` only for
    /// a binding the override replaces.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let file = if self.file.is_empty() {
            "conf.py"
        } else {
            &self.file
        };
        let key = self.key;
        write!(
            f,
            "cannot evaluate {key} ({file}, line {line}: {reason}): sphinx-ultra reads conf.py \
             without running it, so rst_prolog and rst_epilog must be string literals; ",
            line = self.line,
            reason = self.reason,
        )?;
        if self.overridable {
            write!(
                f,
                "set it with -D {key}=... to build without editing conf.py"
            )
        } else {
            write!(
                f,
                "Sphinx reads this binding over any -D {key}=... override, so edit conf.py"
            )
        }
    }
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
    /// reader cannot evaluate is in [`Self::unevaluable`].
    pub rst_prolog: Option<String>,
    /// `rst_epilog` (`config.py:265`), read as `rst_prolog` is.
    pub rst_epilog: Option<String>,
    /// The bindings of `rst_prolog` and `rst_epilog` this reader could not
    /// evaluate ([`PythonConfigParser::unevaluable_bindings`]): a
    /// configuration error unless a `-D` override of the key replaces them
    /// ([`crate::config::BuildConfig::check_evaluated`]).
    #[serde(skip)]
    pub unevaluable: Vec<Unevaluable>,
    /// `(key, python type name)` for a true `rst_prolog` or `rst_epilog`
    /// that is no `str`, on which Sphinx's reader crashes
    /// ([`crate::config::BuildConfig::decoration_crashes`]).
    pub decoration_crashes: Vec<(String, String)>,

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
            literal_types: HashMap::new(),
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
        self.unevaluable_bindings().into_iter().next()
    }

    /// Every such binding, the first by line first (at one line,
    /// `rst_prolog` first), at most two per key: a key's first that a `-D`
    /// override replaces, and its first that none does
    /// ([`Unevaluable::overridable`]), which an earlier binding of the key
    /// must not hide.
    pub fn unevaluable_bindings(&self) -> Vec<&Unevaluable> {
        let mut bindings: Vec<&Unevaluable> = self
            .unevaluable
            .iter()
            .map(|pending| &pending.binding)
            .collect();
        bindings.sort_by_key(|binding| (binding.line, binding.key != "rst_prolog"));
        let mut kinds = Vec::new();
        bindings.retain(|binding| {
            let kind = (binding.key, binding.overridable);
            let first = !kinds.contains(&kind);
            kinds.push(kind);
            first
        });
        bindings
    }

    /// Parse a conf.py file and extract configuration. A value of
    /// `rst_prolog` or `rst_epilog` it cannot evaluate is carried, naming
    /// the key ([`ConfPyConfig::unevaluable`]), into the build
    /// configuration, which fails once the `-D` overrides are in unless
    /// one of the key replaces it
    /// ([`crate::config::BuildConfig::check_evaluated`]);
    /// [`Self::warnings`] holds what the parse dropped.
    pub fn parse_conf_py<P: AsRef<Path>>(&mut self, conf_py_path: P) -> Result<ConfPyConfig> {
        let conf_py_path = conf_py_path.as_ref();
        let _conf_dir = conf_py_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid conf.py path"))?;

        // Read the conf.py file. Python's tokenizer reads `\r\n` and `\r`
        // as `\n`, inside string literals too.
        let conf_py_content = std::fs::read_to_string(conf_py_path)?
            .replace("\r\n", "\n")
            .replace('\r', "\n");

        self.parse_statements(&conf_py_content)?;

        // Extract configuration values
        let mut config = self.extract_configuration()?;
        let file = conf_py_path
            .file_name()
            .map_or_else(|| "conf.py".into(), |name| name.to_string_lossy());
        for binding in &mut config.unevaluable {
            binding.file = file.to_string();
        }
        Ok(config)
    }

    /// Parse the declarative subset of a conf.py: literal assignments, with a
    /// warning recorded for every construct that had to be dropped, except
    /// the statements that bind a key of [`MUST_EVALUATE`] ([`bindings`]),
    /// which are evaluated or recorded as [`Self::unevaluable`].
    fn parse_statements(&mut self, content: &str) -> Result<()> {
        // The `def` and `class` statements whose bodies the statements are
        // in, innermost last.
        let mut scopes: Vec<Scope> = Vec::new();
        for (line, raw) in logical_statements(content) {
            let stmt = raw.trim();
            if stmt.is_empty() {
                continue;
            }
            let indent = indentation(&raw);
            while scopes.last().is_some_and(|scope| scope.indent >= indent) {
                scopes.pop();
            }

            let found = bindings(stmt, indent, &mut scopes);
            if !found.is_empty() {
                for (key, binding) in found {
                    self.bind(line, key, binding);
                }
                continue;
            }

            // Imports set no configuration values; ignoring them loses
            // nothing.
            if imported_names(stmt).is_some() {
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

    /// Applies `binding`, the statement on `line`'s binding of `key` (a
    /// key of [`MUST_EVALUATE`]): evaluates it into the namespace, or
    /// records it as [`Self::unevaluable`]. Only three statements are
    /// evaluated, all at the top level: an assignment of a literal to the
    /// key alone; `key += <string literal>` while the key holds a string
    /// (Python's concatenation; implicit concatenation of literals
    /// included); and `del key` while the key is bound, which leaves it to
    /// Sphinx's default, `None`.
    fn bind(&mut self, line: usize, key: &'static str, binding: Binding<'_>) {
        match binding {
            Binding::Local => {}
            Binding::Assign(value_src) => match parse_python_literal_typed(value_src) {
                // A true `bytes` crashes Sphinx's reader where docutils first
                // matches a pattern against one of its lines, after whatever
                // records the parse makes before it, which this reader cannot
                // place among them.
                Ok((value, "bytes")) if is_true(&value) => {
                    self.opaque(key, line, "a bytes literal".to_string(), true, false, true);
                }
                Ok((value, type_name)) => {
                    let value = if type_name == "bytes" {
                        serde_json::Value::Null
                    } else {
                        value
                    };
                    self.evaluated(key, value, type_name);
                }
                Err(reason) => self.opaque(key, line, reason, true, false, true),
            },
            Binding::Augment { plus, value_src } => {
                // What the key holds, unless a binding since the last value
                // this reader evaluated left it unknown.
                let held = if self.pending(key).is_some() {
                    None
                } else {
                    self.conf_namespace
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .filter(|_| self.literal_types.get(key) == Some(&"str"))
                };
                match (plus, held, parse_python_literal_typed(value_src)) {
                    (true, Some(held), Ok((serde_json::Value::String(added), "str"))) => {
                        let joined = serde_json::Value::String(format!("{held}{added}"));
                        self.evaluated(key, joined, "str");
                    }
                    _ => self.opaque(
                        key,
                        line,
                        "an augmented assignment".to_string(),
                        true,
                        false,
                        true,
                    ),
                }
            }
            Binding::Delete => {
                // `del` of a name that is not bound raises `NameError`,
                // which fails Sphinx's configuration.
                let bound = match self.pending(key) {
                    Some(pending) => pending.definite,
                    None => self.conf_namespace.contains_key(key),
                };
                if bound {
                    self.conf_namespace.remove(key);
                    self.literal_types.remove(key);
                    self.unevaluable
                        .retain(|pending| pending.binding.key != key || pending.deferred);
                } else {
                    let reason = "a del of a name that may not be bound".to_string();
                    self.opaque(key, line, reason, false, false, true);
                }
            }
            Binding::Opaque {
                reason,
                definite,
                deferred,
                overridable,
            } => self.opaque(
                key,
                line,
                reason.to_string(),
                definite,
                deferred,
                overridable,
            ),
        }
    }

    /// The binding of `key` since the last value this reader evaluated
    /// for it that it could not evaluate, if any: a deferred one stands
    /// whatever the top-level statements do.
    fn pending(&self, key: &str) -> Option<&Pending> {
        self.unevaluable
            .iter()
            .find(|pending| pending.binding.key == key && !pending.deferred)
    }

    /// `key` now holds `value`, a literal of Python type `type_name`: the
    /// binding Python's last one wins over, unless it is deferred.
    fn evaluated(&mut self, key: &'static str, value: serde_json::Value, type_name: &'static str) {
        self.conf_namespace.insert(key.to_string(), value);
        self.literal_types.insert(key, type_name);
        self.unevaluable
            .retain(|pending| pending.binding.key != key || pending.deferred);
    }

    /// Records a binding of `key` on `line` this reader cannot evaluate,
    /// unless an earlier one of its kind since the last value it evaluated
    /// stands: the first is the one reported. Its kind is whether it is
    /// `deferred` ([`Pending`]) and whether a `-D` override replaces it
    /// (`overridable`, [`Unevaluable::overridable`]; a top-level binding
    /// always is), so a binding Sphinx reads over an override never hides
    /// behind one it does not, nor the reverse.
    fn opaque(
        &mut self,
        key: &'static str,
        line: usize,
        reason: String,
        definite: bool,
        deferred: bool,
        overridable: bool,
    ) {
        if let Some(pending) = self.unevaluable.iter_mut().find(|pending| {
            pending.binding.key == key
                && pending.deferred == deferred
                && pending.binding.overridable == overridable
        }) {
            pending.definite |= definite;
            return;
        }
        self.unevaluable.push(Pending {
            binding: Unevaluable {
                key,
                line,
                reason,
                file: String::new(),
                overridable,
            },
            definite,
            deferred,
        });
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
        // reports, under its Python type ([`Self::literal_types`]: a
        // tuple, an empty `bytes`), and Sphinx's reader adds nothing for a
        // false one (`if not prologue: return`,
        // `SP/sphinx/util/rst.py:82-83`). On a true one it raises
        // `AttributeError` (`splitlines`), where it reads the first
        // document (decision 13; a true `bytes` is unevaluable,
        // [`PythonConfigParser::bind`]).
        for (key, slot) in [
            ("rst_prolog", &mut config.rst_prolog),
            ("rst_epilog", &mut config.rst_epilog),
        ] {
            let type_name = self.literal_types.get(key).copied();
            *slot = match (self.conf_namespace.get(key), type_name) {
                (_, Some("bytes")) => {
                    mismatches.push((key.to_string(), "bytes".to_string()));
                    None
                }
                (None | Some(serde_json::Value::Null), _) => None,
                (Some(serde_json::Value::String(text)), _) => Some(text.clone()),
                (Some(value), type_name) => {
                    let type_name = type_name.unwrap_or_else(|| python_type_name(value));
                    mismatches.push((key.to_string(), type_name.to_string()));
                    if is_true(value) {
                        config
                            .decoration_crashes
                            .push((key.to_string(), type_name.to_string()));
                    }
                    None
                }
            };
        }
        config.unevaluable = self.unevaluable_bindings().into_iter().cloned().collect();

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

/// A `def` or `class` statement whose body the statements after it, more
/// indented, are in: a scope of its own, where a statement binds a local
/// variable or a class attribute, not the module's name, unless a `global`
/// statement in it names that name.
struct Scope {
    /// The header's indentation ([`indentation`]).
    indent: usize,
    /// The keys of [`MUST_EVALUATE`] the scope's `global` statements name.
    globals: Vec<&'static str>,
}

/// The indentation of `raw`, a logical statement as [`logical_statements`]
/// yields it: its leading columns, as Python's tokenizer counts them (a tab
/// to the next multiple of 8; a form feed starts the count again).
fn indentation(raw: &str) -> usize {
    let mut column = 0;
    for c in raw.chars() {
        match c {
            ' ' => column += 1,
            '\t' => column = (column / 8 + 1) * 8,
            '\x0c' => column = 0,
            _ => break,
        }
    }
    column
}

/// What a statement's binding of a key of [`MUST_EVALUATE`] binds, in the
/// scope it runs in ([`PythonConfigParser::bind`]).
enum Binding<'s> {
    /// A local variable or a class attribute: nothing this reader reads.
    Local,
    /// `key = <value_src>`, the key the one target, at the top level.
    Assign(&'s str),
    /// `key op= <value_src>` at the top level; `plus` for `+=`.
    Augment { plus: bool, value_src: &'s str },
    /// `del key` at the top level.
    Delete,
    /// Any other binding: why this reader cannot evaluate it, whether it
    /// binds the module's name whenever `conf.py` runs, whether it runs
    /// after every top-level statement ([`Pending`]), and whether a `-D`
    /// override replaces it ([`Unevaluable::overridable`]).
    Opaque {
        reason: &'static str,
        definite: bool,
        deferred: bool,
        overridable: bool,
    },
}

/// A statement's binding of a key of [`MUST_EVALUATE`] as the statement
/// alone tells it, before the scope it runs in decides what it binds
/// ([`resolve`]).
enum Bound<'s> {
    Assign(&'s str),
    Augment {
        plus: bool,
        value_src: &'s str,
    },
    Delete,
    /// Another binding of the name: why this reader cannot evaluate it, and
    /// whether it binds the name whenever the statement runs.
    Other {
        reason: &'static str,
        definite: bool,
    },
    /// A target that is an attribute named like the key: Sphinx 9.1.0 reads
    /// `app.config.rst_prolog = ...` in `setup(app)`, and `config.rst_epilog
    /// = ...` in a `config-inited` handler (scratch `t17b-fix1/specs/attr_*`),
    /// over any `-D` override (`t17b-fix2/specs/attr_*`).
    Attribute,
}

/// The keys of [`MUST_EVALUATE`] that `stmt`, a logical statement at
/// `indent`, binds, and what each binding binds in the scope it runs in:
/// `scopes`, which a `def` or `class` header opens one in and a `global`
/// statement declares names in. A statement that binds neither key
/// returns nothing.
///
/// The bindings it sees are an assignment's targets (`=`, `op=`, a tuple,
/// starred or chained target, but not a subscript's key or a lambda's
/// parameter), `del`'s, a `for`'s, a `with`'s or an `except`'s target, an
/// assignment expression, an import's names, and a `def`'s or a `class`'s
/// name. A star import and `exec(...)` bind names it cannot see.
fn bindings<'s>(
    stmt: &'s str,
    indent: usize,
    scopes: &mut Vec<Scope>,
) -> Vec<(&'static str, Binding<'s>)> {
    let in_block = indent > 0;
    let Some(header) = compound_header(stmt) else {
        return simple_bindings(stmt, scopes, in_block);
    };
    // A header binds in the scope around it.
    let mut found: Vec<(&'static str, Binding<'s>)> = header
        .bindings()
        .into_iter()
        .map(|(key, bound)| (key, resolve(bound, key, scopes, in_block)))
        .collect();
    let opens_scope = matches!(header.keyword, "def" | "class");
    if opens_scope {
        scopes.push(Scope {
            indent,
            globals: Vec::new(),
        });
    }
    if !header.body.is_empty() {
        // A one-line compound statement: its body is in the scope the
        // header opens, or in a block.
        for (key, binding) in simple_bindings(header.body, scopes, true) {
            if !found.iter().any(|(bound, _)| *bound == key) {
                found.push((key, binding));
            }
        }
        if opens_scope {
            scopes.pop();
        }
    }
    found
}

/// [`bindings`] of a simple statement, in `scopes`, `in_block` when a
/// compound statement at the top level holds it: a `global` statement
/// declares its names in the innermost scope, and binds nothing.
fn simple_bindings<'s>(
    stmt: &'s str,
    scopes: &mut [Scope],
    in_block: bool,
) -> Vec<(&'static str, Binding<'s>)> {
    if let Some(names) = keyword_rest(stmt, "global") {
        if let Some(scope) = scopes.last_mut() {
            for name in names.split(',').map(str::trim) {
                if let Some(key) = MUST_EVALUATE.into_iter().find(|key| *key == name) {
                    if !scope.globals.contains(&key) {
                        scope.globals.push(key);
                    }
                }
            }
        }
        return Vec::new();
    }
    statement_bindings(stmt)
        .into_iter()
        .map(|(key, bound)| (key, resolve(bound, key, scopes, in_block)))
        .collect()
}

/// What `bound`, a statement's binding of `key`, binds in the scope it
/// runs in. An attribute's runs after every top-level statement (in a
/// function Sphinx calls), and Sphinx reads it over any `-D` override; so
/// does a function's own binding of the module's name, which a `global`
/// statement makes, but an override replaces it; in a function or a class
/// body otherwise, a name is the scope's own. At the top level, in a
/// block, a statement may run once, many times or not at all.
fn resolve<'s>(
    bound: Bound<'s>,
    key: &'static str,
    scopes: &[Scope],
    in_block: bool,
) -> Binding<'s> {
    if let Bound::Attribute = bound {
        return Binding::Opaque {
            reason: "an assignment to an attribute of that name",
            definite: false,
            deferred: true,
            overridable: false,
        };
    }
    match scopes.last() {
        Some(scope) if !scope.globals.contains(&key) => Binding::Local,
        Some(_) => Binding::Opaque {
            reason: "a binding in a function or class body that declares it global",
            definite: false,
            deferred: true,
            overridable: true,
        },
        None if in_block => Binding::Opaque {
            reason: match bound {
                Bound::Delete => "a del inside a block",
                _ => "an assignment inside a block",
            },
            definite: false,
            deferred: false,
            overridable: true,
        },
        None => match bound {
            Bound::Assign(value_src) => Binding::Assign(value_src),
            Bound::Augment { plus, value_src } => Binding::Augment { plus, value_src },
            Bound::Delete => Binding::Delete,
            Bound::Other { reason, definite } => Binding::Opaque {
                reason,
                definite,
                deferred: false,
                overridable: true,
            },
            Bound::Attribute => unreachable!("handled above"),
        },
    }
}

/// The bindings of the keys of [`MUST_EVALUATE`] that the simple statement
/// `stmt` makes, as the statement alone tells them: an import's names,
/// `del`'s targets, an assignment's targets, an assignment expression's.
fn statement_bindings(stmt: &str) -> Vec<(&'static str, Bound<'_>)> {
    let mut found: Vec<(&'static str, Bound<'_>)> = Vec::new();
    if let Some(names) = imported_names(stmt) {
        for key in MUST_EVALUATE {
            if names.contains(&key) {
                let (reason, definite) = ("an import", true);
                found.push((key, Bound::Other { reason, definite }));
            }
        }
        return found;
    }
    if let Some(targets) = keyword_rest(stmt, "del") {
        let targets = target_bindings(targets);
        for key in MUST_EVALUATE {
            if targets.names.contains(&key) {
                found.push((key, Bound::Delete));
            } else if targets.attributes.contains(&key) {
                found.push((key, Bound::Attribute));
            }
        }
        return found;
    }
    let assignment = assignment(stmt);
    let targets: Vec<TargetBindings<'_>> = assignment
        .targets
        .iter()
        .map(|target| target_bindings(target))
        .collect();
    for key in MUST_EVALUATE {
        if targets.iter().any(|target| target.names.contains(&key)) {
            let alone = matches!(assignment.targets[..], [target] if target.trim() == key);
            let value_src = assignment.value;
            let bound = match (alone, assignment.augmented) {
                (true, None) => Bound::Assign(value_src),
                (true, Some(operator)) => Bound::Augment {
                    plus: operator == "+",
                    value_src,
                },
                (false, None) => Bound::Other {
                    reason: "an assignment it cannot evaluate",
                    definite: true,
                },
                (false, Some(_)) => Bound::Other {
                    reason: "an augmented assignment",
                    definite: true,
                },
            };
            found.push((key, bound));
        } else if targets
            .iter()
            .any(|target| target.attributes.contains(&key))
        {
            found.push((key, Bound::Attribute));
        } else if walrus_targets(stmt).contains(&key) {
            let (reason, definite) = ("an assignment expression", false);
            found.push((key, Bound::Other { reason, definite }));
        }
    }
    found
}

/// A compound statement's header: its keyword (`async` dropped), the text
/// between the keyword and the colon, and the body after the colon on the
/// same line (empty when the body is a block).
struct Header<'s> {
    keyword: &'static str,
    text: &'s str,
    body: &'s str,
}

/// The compound statements whose header can bind a name or end in a
/// one-line body. (`match` and `case`, soft keywords, are not told apart
/// from names here: a `case` pattern's capture binds a name this reader
/// does not see.)
const COMPOUND_KEYWORDS: [&str; 11] = [
    "if", "elif", "else", "while", "for", "try", "except", "finally", "with", "def", "class",
];

/// `stmt`'s header, when it is a compound statement: its keyword, then a
/// colon outside strings and brackets that no `=` follows (`:=`).
fn compound_header(stmt: &str) -> Option<Header<'_>> {
    let (word, rest) = leading_word(stmt);
    let (word, rest) = match word {
        "async" => leading_word(rest.trim_start()),
        _ => (word, rest),
    };
    let keyword = COMPOUND_KEYWORDS
        .into_iter()
        .find(|keyword| *keyword == word)?;
    let bytes = rest.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i = skip_string(bytes, i, quote);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 && bytes.get(i + 1) != Some(&b'=') => {
                return Some(Header {
                    keyword,
                    text: rest[..i].trim(),
                    body: rest[i + 1..].trim(),
                });
            }
            _ => {}
        }
        i += 1;
    }
    None
}

impl<'s> Header<'s> {
    /// The keys of [`MUST_EVALUATE`] the header binds: a `for`'s target, a
    /// `with`'s or an `except`'s `as` target, a `def`'s or a `class`'s
    /// name, an assignment expression in its expressions.
    fn bindings(&self) -> Vec<(&'static str, Bound<'s>)> {
        let mut found: Vec<(&'static str, Bound<'s>)> = Vec::new();
        let mut targets = |targets: TargetBindings<'_>, reason: &'static str, definite: bool| {
            for key in MUST_EVALUATE {
                if found.iter().any(|(bound, _)| *bound == key) {
                    continue;
                }
                if targets.names.contains(&key) {
                    found.push((key, Bound::Other { reason, definite }));
                } else if targets.attributes.contains(&key) {
                    found.push((key, Bound::Attribute));
                }
            }
        };
        match self.keyword {
            "for" => targets(
                target_bindings(for_target(self.text)),
                "a for target",
                false,
            ),
            "with" | "except" => {
                for target in as_targets(self.text) {
                    match self.keyword {
                        "with" => targets(target_bindings(target), "a with target", true),
                        _ => targets(target_bindings(target), "an except target", false),
                    }
                }
            }
            "def" | "class" => {
                let (name, _) = leading_word(self.text);
                let reason = match self.keyword {
                    "def" => "a def statement",
                    _ => "a class statement",
                };
                if let Some(key) = MUST_EVALUATE.into_iter().find(|key| *key == name) {
                    found.push((
                        key,
                        Bound::Other {
                            reason,
                            definite: true,
                        },
                    ));
                }
            }
            _ => {}
        }
        for key in MUST_EVALUATE {
            if !found.iter().any(|(bound, _)| *bound == key)
                && walrus_targets(self.text).contains(&key)
            {
                let (reason, definite) = ("an assignment expression", false);
                found.push((key, Bound::Other { reason, definite }));
            }
        }
        found
    }
}

/// The identifier `text` starts with (empty when it starts with something
/// else), and what follows it.
fn leading_word(text: &str) -> (&str, &str) {
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    text.split_at(end)
}

/// `text` after `keyword`, when `text` starts with it as a whole word.
fn keyword_rest<'s>(text: &'s str, keyword: &str) -> Option<&'s str> {
    let rest = text.strip_prefix(keyword)?;
    match rest.chars().next() {
        Some(c) if c.is_alphanumeric() || c == '_' => None,
        _ => Some(rest),
    }
}

/// A `for` header's target: the text before the first `in` outside
/// strings and brackets.
fn for_target(text: &str) -> &str {
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i = skip_string(bytes, i, quote);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            byte if is_identifier_byte(byte) => {
                let start = i;
                while i < bytes.len() && is_identifier_byte(bytes[i]) {
                    i += 1;
                }
                if depth == 0 && &text[start..i] == "in" {
                    return &text[..start];
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    text
}

/// The targets of the `as` clauses in `text`, a `with` or an `except`
/// header's: after each `as` outside strings, a name, an attribute, or a
/// bracketed target, up to the `,` or the end of the clause.
fn as_targets(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut targets = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => i = skip_string(bytes, i, quote),
            byte if is_identifier_byte(byte) => {
                let start = i;
                while i < bytes.len() && is_identifier_byte(bytes[i]) {
                    i += 1;
                }
                if &text[start..i] != "as" {
                    continue;
                }
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let (target, mut depth) = (i, 0i32);
                while i < bytes.len() {
                    match bytes[i] {
                        quote @ (b'\'' | b'"') => {
                            i = skip_string(bytes, i, quote);
                            continue;
                        }
                        b'(' | b'[' | b'{' => depth += 1,
                        b')' | b']' | b'}' if depth > 0 => depth -= 1,
                        b')' | b']' | b'}' | b',' | b':' if depth == 0 => break,
                        byte if byte.is_ascii_whitespace() && depth == 0 => break,
                        _ => {}
                    }
                    i += 1;
                }
                targets.push(&text[target..i]);
            }
            _ => i += 1,
        }
    }
    targets
}

/// The names `stmt` binds with assignment expressions (`name := value`),
/// in brackets or not.
fn walrus_targets(stmt: &str) -> Vec<&str> {
    let bytes = stmt.as_bytes();
    let mut names = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i = skip_string(bytes, i, quote);
                continue;
            }
            b':' if bytes.get(i + 1) == Some(&b'=') => {
                let before = stmt[..i].trim_end();
                let start = before
                    .char_indices()
                    .rev()
                    .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
                    .map_or(0, |(at, c)| at + c.len_utf8());
                names.push(&before[start..]);
                i += 2;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    names
}

/// Whether `byte` can be part of an identifier (any non-ASCII byte is taken
/// for one).
fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// What an assignment statement assigns to, as far as a key of
/// [`MUST_EVALUATE`] needs telling.
struct Assignment<'s> {
    /// The text before each top-level `=` (a chained assignment has
    /// several), or before the augmented operator; empty for a statement
    /// with neither.
    targets: Vec<&'s str>,
    /// An augmented assignment's operator (`+`, `//`, ...).
    augmented: Option<&'s str>,
    /// What is assigned: the text after the last target's `=`.
    value: &'s str,
}

/// The assignment `stmt` makes, from its top-level `=` signs: outside
/// strings and brackets, and none of `==`, `!=`, `<=`, `>=` or `:=`. An
/// `=` that a binary operator precedes (`+=`, `//=`, `>>=`, ...) makes it
/// an augmented assignment, whose one target stops before the operator.
/// The `=` of a lambda's default parameter is part of the value
/// (`f = lambda x='': x`). A `for` or `with` target, a `def`, an import
/// and a `del` are not assignments here.
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
                let target = &stmt[start..i - operator];
                if has_lambda(target) {
                    break;
                }
                targets.push(target);
                if operator > 0 {
                    return Assignment {
                        targets,
                        augmented: Some(&stmt[i - operator..i]),
                        value: stmt[i + 1..].trim(),
                    };
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    Assignment {
        value: if targets.is_empty() {
            ""
        } else {
            stmt[start..].trim()
        },
        targets,
        augmented: None,
    }
}

/// Whether `text` holds the keyword `lambda` outside strings and brackets.
fn has_lambda(text: &str) -> bool {
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i = skip_string(bytes, i, quote);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            byte if is_identifier_byte(byte) => {
                let start = i;
                while i < bytes.len() && is_identifier_byte(bytes[i]) {
                    i += 1;
                }
                if depth == 0 && &text[start..i] == "lambda" {
                    return true;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// The names the import statement `stmt` binds, or `None` when it is no
/// import: each name (or its `as` alias) of `from m import a, b as c`,
/// parenthesized or not (`import(a)` too), and the first component (or
/// the alias) of each module of `import a.b, d as e`. A star import binds
/// nothing this reader can name.
fn imported_names(stmt: &str) -> Option<Vec<&str>> {
    let (items, from) = match keyword_rest(stmt, "from") {
        Some(rest) => {
            // `from <module> import ...`: the module is a dotted name or dots.
            let rest = rest.trim_start();
            let module = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(rest.len());
            (keyword_rest(rest[module..].trim_start(), "import")?, true)
        }
        None => (keyword_rest(stmt, "import")?, false),
    };
    let items = items.trim();
    let items = items
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .unwrap_or(items);
    Some(
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
            .collect(),
    )
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

/// The names and attributes a target binds.
struct TargetBindings<'s> {
    /// The names it binds: the target itself, or a tuple's or list's items.
    names: Vec<&'s str>,
    /// The attributes it binds: `name` of `x.name`.
    attributes: Vec<&'s str>,
}

/// What `target` binds: each name or attribute that ends a target, outside
/// strings and the brackets of a call or a subscript, whose content is
/// read, not bound (`x[rst_prolog]` binds nothing by name). A bracket that
/// follows a name, a closing bracket or a string opens a call or a
/// subscript; any other opens a tuple or a list, whose items are targets.
fn target_bindings(target: &str) -> TargetBindings<'_> {
    #[derive(Clone, Copy, PartialEq)]
    enum Prev {
        Start,
        Name,
        Close,
        Dot,
        Other,
    }
    let bytes = target.as_bytes();
    let mut bound = TargetBindings {
        names: Vec::new(),
        attributes: Vec::new(),
    };
    // For each open bracket, whether it is a tuple's or a list's.
    let mut displays: Vec<bool> = Vec::new();
    let mut prev = Prev::Start;
    // A name, and whether it is an attribute, until what follows it tells
    // whether it ends a target: what `.`, `(` or `[` follows is read.
    let mut waiting: Option<(&str, bool)> = None;
    fn flush<'s>(
        waiting: &mut Option<(&'s str, bool)>,
        next: Option<u8>,
        bound: &mut TargetBindings<'s>,
    ) {
        if let Some((name, attribute)) = waiting.take() {
            if !matches!(next, Some(b'.' | b'(' | b'[')) {
                match attribute {
                    true => bound.attributes.push(name),
                    false => bound.names.push(name),
                }
            }
        }
    }
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        flush(&mut waiting, Some(byte), &mut bound);
        if byte == b'\'' || byte == b'"' {
            i = skip_string(bytes, i, byte);
            prev = Prev::Close;
            continue;
        }
        if is_identifier_byte(byte) {
            let start = i;
            while i < bytes.len() && is_identifier_byte(bytes[i]) {
                i += 1;
            }
            if !byte.is_ascii_digit() && !displays.contains(&false) {
                waiting = Some((&target[start..i], prev == Prev::Dot));
            }
            prev = Prev::Name;
            continue;
        }
        match byte {
            b'(' | b'[' | b'{' => {
                displays.push(!matches!(prev, Prev::Name | Prev::Close));
                prev = Prev::Other;
            }
            b')' | b']' | b'}' => {
                displays.pop();
                prev = Prev::Close;
            }
            b'.' => prev = Prev::Dot,
            _ => prev = Prev::Other,
        }
        i += 1;
    }
    flush(&mut waiting, None, &mut bound);
    bound
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
/// Supports: strings (CPython 3.12's escapes, implicit adjacent
/// concatenation, triple quotes; a `bytes` literal reads as the `str` its
/// bytes are the Latin-1 code points of), ints/floats, True/False/None,
/// lists, tuples (as arrays), dicts with string keys, arbitrary nesting,
/// trailing commas.
pub(crate) fn parse_python_literal(src: &str) -> std::result::Result<serde_json::Value, String> {
    parse_python_literal_typed(src).map(|(value, _)| value)
}

/// [`parse_python_literal`], with the value's Python type
/// (`type(value).__name__`): `str` or `bytes` for a string literal,
/// `tuple` or `list` for an array, and so on.
pub(crate) fn parse_python_literal_typed(
    src: &str,
) -> std::result::Result<(serde_json::Value, &'static str), String> {
    let chars: Vec<char> = src.chars().collect();
    let mut p = PyLiteralParser {
        chars,
        pos: 0,
        saw_comma: false,
        type_name: "NoneType",
    };
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos < p.chars.len() {
        return Err("trailing expression".to_string());
    }
    Ok((value, p.type_name))
}

/// Python's truth value of a literal: false for `None`, `False`, a zero,
/// and an empty string, sequence or dict.
fn is_true(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(number) => number.as_f64() != Some(0.0),
        serde_json::Value::String(text) => !text.is_empty(),
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(map) => !map.is_empty(),
    }
}

struct PyLiteralParser {
    chars: Vec<char>,
    pos: usize,
    /// Whether the most recently closed sequence contained a comma — used to
    /// tell a parenthesized grouping `(x)` from a one-element tuple `(x,)`.
    saw_comma: bool,
    /// The Python type of the value parsed last: once the parse is done,
    /// the whole value's.
    type_name: &'static str,
}

/// What a string literal's prefix makes it.
#[derive(Clone, Copy)]
struct StringKind {
    /// `r`: a backslash escapes nothing.
    raw: bool,
    /// `b`: a `bytes` literal.
    bytes: bool,
}

/// CPython's message for a `bytes` literal holding a character that is not
/// ASCII.
const BYTES_ASCII: &str = "bytes can only contain ASCII literal characters";

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
        if let Some(kind) = self.string_prefix() {
            let mut s = self.parse_string(kind)?;
            // Implicit adjacent string concatenation: 'a' 'b' == 'ab'
            while let Some(next) = {
                self.skip_ws();
                self.string_prefix()
            } {
                if next.bytes != kind.bytes {
                    return Err("cannot mix bytes and nonbytes literals".to_string());
                }
                s.push_str(&self.parse_string(next)?);
            }
            self.type_name = if kind.bytes { "bytes" } else { "str" };
            return Ok(serde_json::Value::String(s));
        }
        let value = match self.peek() {
            Some('[') => {
                let value = self.parse_sequence('[', ']');
                self.type_name = "list";
                value
            }
            Some('(') => {
                // Python: `(x)` is grouping, `(x,)` / `(x, y)` is a tuple.
                // Either way an array (or the inner value) serves config
                // needs; the type is the inner value's, or `tuple`.
                let value = self.parse_sequence('(', ')')?;
                match value {
                    serde_json::Value::Array(items) if items.len() == 1 && !self.saw_comma => {
                        return Ok(items.into_iter().next().unwrap());
                    }
                    other => {
                        self.type_name = "tuple";
                        Ok(other)
                    }
                }
            }
            Some('{') => {
                let value = self.parse_dict();
                self.type_name = "dict";
                value
            }
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
        }?;
        if !matches!(
            value,
            serde_json::Value::Array(_) | serde_json::Value::Object(_)
        ) {
            self.type_name = python_type_name(&value);
        }
        Ok(value)
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
    /// is consumed: what kind of literal it is. A raw literal's backslashes
    /// survive (`nitpick_ignore_regex` entries are conventionally written
    /// `r'...'`); a `b` prefix makes it a `bytes` literal. `f` prefixes are
    /// deliberately not accepted: an f-string's braces are an expression
    /// this parser cannot evaluate, so it stays an unsupported expression
    /// (skipped) rather than being taken literally.
    fn string_prefix(&mut self) -> Option<StringKind> {
        if matches!(self.peek(), Some('\'') | Some('"')) {
            return Some(StringKind {
                raw: false,
                bytes: false,
            });
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
                return Some(StringKind {
                    raw: prefix.contains('r'),
                    bytes: prefix.contains('b'),
                });
            }
        }
        None
    }

    /// The string literal here, its prefix consumed ([`Self::string_prefix`]),
    /// as CPython 3.12 reads it, or why CPython rejects it or this reader
    /// cannot hold what it gives: a raw literal keeps every backslash; any
    /// other reads its escape sequences ([`Self::escape`]). A newline ends
    /// a single-quoted literal unless a backslash escapes it, and a `bytes`
    /// literal holds ASCII characters only.
    fn parse_string(&mut self, kind: StringKind) -> std::result::Result<String, String> {
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
            if kind.bytes && !c.is_ascii() {
                return Err(BYTES_ASCII.to_string());
            }
            if c == '\\' {
                let next = *self
                    .chars
                    .get(self.pos + 1)
                    .ok_or("unterminated string literal")?;
                if kind.bytes && !next.is_ascii() {
                    return Err(BYTES_ASCII.to_string());
                }
                self.pos += 2;
                if kind.raw {
                    // A raw literal keeps both characters — but the escaped
                    // quote still does not end the string.
                    out.push('\\');
                    out.push(next);
                } else {
                    self.escape(next, kind.bytes, &mut out)?;
                }
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
            if !triple && matches!(c, '\n' | '\r') {
                return Err("unterminated string literal".to_string());
            }
            out.push(c);
            self.pos += 1;
        }
    }

    /// The escape sequence a backslash and `next` start, the parser past
    /// `next`, as CPython 3.12 reads it in a `str` literal (`bytes`
    /// false) or a `bytes` one, onto `out` — a `bytes` value's bytes as
    /// the Latin-1 code points they are:
    /// - a backslash before a line break removes both;
    /// - `\\`, `\'`, `\"`, `\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`;
    /// - one to three octal digits: the code point they give, up to 0o777
    ///   in a `str` (CPython 3.12 warns above 0o377), that modulo 256 in a
    ///   `bytes`;
    /// - `\x` and exactly two hex digits;
    /// - in a `str`, `\u` and four, `\U` and eight hex digits; a lone
    ///   surrogate, which a Rust `String` cannot hold, and `\N{...}`, whose
    ///   names this crate has no table of, fail the literal;
    /// - any other keeps the backslash (CPython 3.12 warns).
    ///
    /// A short `\x`, `\u` or `\U`, and a `\U` past U+10FFFF, are CPython's
    /// `SyntaxError`s: they fail the literal with its message.
    fn escape(
        &mut self,
        next: char,
        bytes: bool,
        out: &mut String,
    ) -> std::result::Result<(), String> {
        let simple = match next {
            '\n' => return Ok(()),
            '\r' => {
                // CPython reads a source's `\r\n` as one line break.
                if self.peek() == Some('\n') {
                    self.pos += 1;
                }
                return Ok(());
            }
            '\\' | '\'' | '"' => Some(next),
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'f' => Some('\x0c'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            'v' => Some('\x0b'),
            _ => None,
        };
        if let Some(c) = simple {
            out.push(c);
            return Ok(());
        }
        let code = match next {
            '0'..='7' => {
                let mut value = next.to_digit(8).expect("an octal digit");
                for _ in 0..2 {
                    match self.peek().and_then(|c| c.to_digit(8)) {
                        Some(digit) => {
                            value = value * 8 + digit;
                            self.pos += 1;
                        }
                        None => break,
                    }
                }
                if bytes {
                    value % 256
                } else {
                    value
                }
            }
            'x' => self.hex_digits(2, "truncated \\xXX escape")?,
            'u' if !bytes => self.hex_digits(4, "truncated \\uXXXX escape")?,
            'U' if !bytes => {
                let value = self.hex_digits(8, "truncated \\UXXXXXXXX escape")?;
                if value > 0x10FFFF {
                    return Err("illegal Unicode character".to_string());
                }
                value
            }
            'N' if !bytes => return Err("a \\N{...} escape".to_string()),
            _ => {
                out.push('\\');
                out.push(next);
                return Ok(());
            }
        };
        out.push(char::from_u32(code).ok_or("a lone surrogate")?);
        Ok(())
    }

    /// The value of the `count` hex digits here, the parser past them, or
    /// `error` when fewer follow.
    fn hex_digits(&mut self, count: usize, error: &str) -> std::result::Result<u32, String> {
        let mut value = 0;
        for _ in 0..count {
            let digit = self
                .peek()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| error.to_string())?;
            value = value * 16 + digit;
            self.pos += 1;
        }
        Ok(value)
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
            unevaluable: Vec::new(),
            decoration_crashes: Vec::new(),
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
        config.unevaluable = self.unevaluable.clone();
        config.decoration_crashes = self.decoration_crashes.clone();
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
    /// a block, one it cannot split, an import of the name, a `for`'s, a
    /// `with`'s or an `except`'s target, an assignment expression, a `def`
    /// or a `class` of the name, a binding in a function that declares it
    /// global, and an assignment to an attribute of that name, which
    /// Sphinx 9.1.0 reads in `setup(app)` (scratch
    /// `t17b-fix1/specs/attr_*`, `def_*`, `del_prolog`). A later top-level
    /// assignment it can evaluate is the value Python's last binding
    /// leaves, unless the earlier binding runs later, in a function; `del`
    /// of a bound name leaves Sphinx's `None`. Reading the keys binds
    /// nothing, nor does a subscript's key, a lambda's parameter, a call's
    /// keyword argument, or an assignment in a function or a class body,
    /// which binds a local variable or a class attribute.
    #[test]
    fn unevaluable_prolog_or_epilog_fails_the_parse() {
        /// The pending binding, or what `rst_prolog` holds (`None`: unset
        /// or `None`).
        type Expected = Result<Option<&'static str>, (&'static str, usize, &'static str)>;
        let rows: [(&str, &str, Expected); 68] = [
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
                "+= of a bytes",
                "rst_prolog = 'a'\nrst_prolog += b'b'\n",
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
                "in a one-line if",
                "if x: rst_prolog = 'a'\n",
                Err(("rst_prolog", 1, "an assignment inside a block")),
            ),
            (
                "in a try block",
                "try:\n    rst_prolog = 'a'\nexcept ImportError:\n    pass\n",
                Err(("rst_prolog", 2, "an assignment inside a block")),
            ),
            (
                "in a for block",
                "for x in y:\n    rst_prolog = 'a'\n",
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
                "a parenthesized tuple target",
                "(x, [rst_epilog]) = make()\n",
                Err(("rst_epilog", 1, "an assignment it cannot evaluate")),
            ),
            (
                "a starred target",
                "*rst_epilog, x = make()\n",
                Err(("rst_epilog", 1, "an assignment it cannot evaluate")),
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
                Ok(Some("plain")),
            ),
            (
                "+= of string literals",
                "rst_prolog = 'a'\nrst_prolog += 'b' 'c'\nrst_prolog += \"\"\"\nd\"\"\"\n",
                Ok(Some("abc\nd")),
            ),
            (
                "reads bind nothing",
                "rst_prolog = 'a'\nx = rst_prolog + 'b'\nhtml_context = {'p': rst_prolog}\n",
                Ok(Some("a")),
            ),
            (
                "an unrelated block",
                "rst_prolog = 'a'\nif x:\n    y = rst_prolog\n",
                Ok(Some("a")),
            ),
            (
                "a subscript's key",
                "rst_prolog = 'a'\nx = {}\nx[rst_prolog] = 1\nx.y[rst_prolog] += 1\n",
                Ok(Some("a")),
            ),
            (
                "a lambda's parameter",
                "rst_prolog = 'a'\nf = lambda rst_prolog='': rst_prolog\n",
                Ok(Some("a")),
            ),
            (
                "a call's keyword argument",
                "rst_prolog = 'a'\nconfigure(rst_prolog='x')\nhtml_context = dict(\n    \
                 rst_prolog='x',\n)\n",
                Ok(Some("a")),
            ),
            (
                "a def body",
                "rst_prolog = 'a'\ndef setup(app):\n    rst_prolog = 'x'\n    if app:\n        \
                 rst_prolog += 'y'\n",
                Ok(Some("a")),
            ),
            (
                "a one-line def",
                "rst_prolog = 'a'\ndef f(): rst_prolog = 'x'\n",
                Ok(Some("a")),
            ),
            (
                "a class body",
                "rst_prolog = 'a'\nclass C:\n    rst_prolog = 'y'\n    def m(self):\n        \
                 rst_prolog = 'z'\n",
                Ok(Some("a")),
            ),
            (
                "a global in another function",
                "def f():\n    global rst_prolog\ndef g():\n    rst_prolog = 'x'\nrst_prolog = 'a'\n",
                Ok(Some("a")),
            ),
            (
                "a function nested in one with the global",
                "def f():\n    global rst_prolog\n    def g():\n        rst_prolog = 'x'\n\
                 rst_prolog = 'a'\n",
                Ok(Some("a")),
            ),
            (
                "a global binding in a function",
                "rst_prolog = 'a'\ndef setup(app):\n    global rst_prolog\n    rst_prolog = 'b'\n",
                Err((
                    "rst_prolog",
                    4,
                    "a binding in a function or class body that declares it global",
                )),
            ),
            (
                "a function's global binding runs after a later literal",
                "def setup(app):\n    global rst_epilog\n    if app:\n        rst_epilog += 'b'\n\
                 rst_epilog = 'a'\n",
                Err((
                    "rst_epilog",
                    4,
                    "a binding in a function or class body that declares it global",
                )),
            ),
            (
                "an attribute in setup",
                "rst_prolog = 'a'\ndef setup(app):\n    app.config.rst_prolog = 'b'\n",
                Err(("rst_prolog", 3, "an assignment to an attribute of that name")),
            ),
            (
                "an attribute in an event handler",
                "def inited(app, config):\n    config.rst_epilog += 'b'\n",
                Err(("rst_epilog", 2, "an assignment to an attribute of that name")),
            ),
            (
                "del",
                "rst_prolog = 'a'\ndel rst_prolog\n",
                Ok(None),
            ),
            (
                "del after a binding it cannot evaluate",
                "rst_prolog = f'x'\ndel x, (rst_prolog)\n",
                Ok(None),
            ),
            (
                "a literal after del",
                "rst_prolog = 'a'\ndel rst_prolog\nrst_prolog = 'b'\n",
                Ok(Some("b")),
            ),
            (
                "del of an unbound name",
                "del rst_prolog\n",
                Err(("rst_prolog", 1, "a del of a name that may not be bound")),
            ),
            (
                "del of a name a block may have bound",
                "if x:\n    rst_prolog = 'a'\ndel rst_prolog\n",
                Err(("rst_prolog", 2, "an assignment inside a block")),
            ),
            (
                "del in a block",
                "rst_prolog = 'a'\nif x:\n    del rst_prolog\n",
                Err(("rst_prolog", 3, "a del inside a block")),
            ),
            (
                "del of a subscript",
                "rst_prolog = 'a'\nx = {}\ndel x[rst_prolog]\n",
                Ok(Some("a")),
            ),
            (
                "a for target",
                "for rst_prolog in ['a']:\n    pass\n",
                Err(("rst_prolog", 1, "a for target")),
            ),
            (
                "a for target in a tuple",
                "for x, rst_epilog in pairs: pass\n",
                Err(("rst_epilog", 1, "a for target")),
            ),
            (
                "a with target",
                "with open('p') as rst_prolog:\n    pass\n",
                Err(("rst_prolog", 1, "a with target")),
            ),
            (
                "a parenthesized with target",
                "with (open('p') as f, open('e') as rst_epilog):\n    pass\n",
                Err(("rst_epilog", 1, "a with target")),
            ),
            (
                "an except target",
                "try:\n    pass\nexcept OSError as rst_prolog:\n    pass\n",
                Err(("rst_prolog", 3, "an except target")),
            ),
            (
                "an assignment expression",
                "print(rst_epilog := 'x')\n",
                Err(("rst_epilog", 1, "an assignment expression")),
            ),
            (
                "an assignment expression after a character that is no ASCII",
                "x = (\u{ab}rst_epilog := 'x')\n",
                Err(("rst_epilog", 1, "an assignment expression")),
            ),
            (
                "a parenthesized assignment expression in a header",
                "if (rst_prolog := 'a'):\n    pass\n",
                Err(("rst_prolog", 1, "an assignment expression")),
            ),
            (
                "a def of the name",
                "def rst_prolog():\n    pass\n",
                Err(("rst_prolog", 1, "a def statement")),
            ),
            (
                "a class of the name",
                "class rst_epilog:\n    pass\n",
                Err(("rst_epilog", 1, "a class statement")),
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
                "a parenthesized import with no space",
                "from shared import(rst_prolog)\n",
                Err(("rst_prolog", 1, "an import")),
            ),
            (
                "an import in a function",
                "rst_prolog = 'a'\ndef setup(app):\n    from shared import rst_prolog\n",
                Ok(Some("a")),
            ),
            (
                "an import before a literal",
                "from shared import rst_prolog\nrst_prolog = 'plain'\n",
                Ok(Some("plain")),
            ),
            (
                "importing other names",
                "import rst_prolog_tools as tools\nfrom shared import rst_prologue\n\
                 rst_prolog = 'a'\n",
                Ok(Some("a")),
            ),
            (
                "an escaped line break",
                "rst_prolog = \"\"\"\\\n.. |a| replace:: A\n\"\"\"\n",
                Ok(Some(".. |a| replace:: A\n")),
            ),
            (
                "a \\N escape",
                "rst_prolog = '\\N{BULLET}'\n",
                Err(("rst_prolog", 1, "a \\N{...} escape")),
            ),
            (
                "a short \\x escape",
                "rst_epilog = '\\x4'\n",
                Err(("rst_epilog", 1, "truncated \\xXX escape")),
            ),
            (
                "a lone surrogate",
                "rst_prolog = '\\ud800'\n",
                Err(("rst_prolog", 1, "a lone surrogate")),
            ),
            (
                "a true bytes",
                "rst_prolog = b'.. |a| replace:: A\\n'\n",
                Err(("rst_prolog", 1, "a bytes literal")),
            ),
            (
                "bytes mixed with str",
                "rst_prolog = b'a' 'b'\n",
                Err(("rst_prolog", 1, "cannot mix bytes and nonbytes literals")),
            ),
            (
                "an empty bytes",
                "rst_prolog = b''\n",
                Ok(None),
            ),
            (
                "None",
                "rst_prolog = 'a'\nrst_prolog = None\n",
                Ok(None),
            ),
            (
                "a mistyped value",
                "rst_prolog = ('x',)\n",
                Ok(None),
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
                    let held = p.conf_namespace.get("rst_prolog");
                    match value {
                        Some(value) => assert_eq!(
                            held,
                            Some(&serde_json::Value::String(value.to_string())),
                            "{name}"
                        ),
                        None if name == "a mistyped value" => {
                            assert!(held.is_some_and(serde_json::Value::is_array), "{name}");
                        }
                        None => assert!(
                            held.is_none_or(serde_json::Value::is_null),
                            "{name}: {held:?}"
                        ),
                    }
                }
            }
        }
    }

    /// A true `rst_prolog` or `rst_epilog` that is no `str` crashes
    /// Sphinx 9.1.0's reader on its `splitlines` (decision 13): its Python
    /// type is recorded for the type warning and the crash, a tuple's as
    /// `tuple`. A false one, an empty `bytes` included, warns only
    /// (scratch `t17b-fix1/specs/crash_*`, `bytes_empty`).
    #[test]
    fn a_true_mistyped_prolog_or_epilog_is_a_crash() {
        for (content, mismatches, crashes) in [
            (
                "rst_prolog = 1\n",
                vec![("rst_prolog", "int")],
                vec![("rst_prolog", "int")],
            ),
            (
                "rst_epilog = ('x',)\n",
                vec![("rst_epilog", "tuple")],
                vec![("rst_epilog", "tuple")],
            ),
            (
                "rst_prolog = ['p']\nrst_epilog = {'a': 1}\n",
                vec![("rst_prolog", "list"), ("rst_epilog", "dict")],
                vec![("rst_prolog", "list"), ("rst_epilog", "dict")],
            ),
            (
                "rst_prolog = (['p'])\n",
                vec![("rst_prolog", "list")],
                vec![("rst_prolog", "list")],
            ),
            (
                "rst_prolog = 0.0\nrst_epilog = ()\n",
                vec![("rst_prolog", "float"), ("rst_epilog", "tuple")],
                vec![],
            ),
            ("rst_prolog = b''\n", vec![("rst_prolog", "bytes")], vec![]),
            (
                "rst_prolog = True\n",
                vec![("rst_prolog", "bool")],
                vec![("rst_prolog", "bool")],
            ),
        ] {
            let config = parse(content).extract_configuration().unwrap();
            let pairs = |pairs: &[(String, String)]| {
                pairs
                    .iter()
                    .map(|(key, type_name)| (key.clone(), type_name.clone()))
                    .collect::<Vec<_>>()
            };
            let owned = |pairs: Vec<(&str, &str)>| {
                pairs
                    .into_iter()
                    .map(|(key, type_name)| (key.to_string(), type_name.to_string()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                pairs(&config.confval_type_mismatches),
                owned(mismatches),
                "{content}"
            );
            assert_eq!(
                pairs(&config.decoration_crashes),
                owned(crashes),
                "{content}"
            );
            assert_eq!(
                (&config.rst_prolog, &config.rst_epilog),
                (&None, &None),
                "{content}"
            );
        }
    }

    /// The bindings it cannot evaluate reach the build configuration, the
    /// first by line first, named with the configuration file
    /// ([`PythonConfigParser::parse_conf_py`]), and fail it only there,
    /// once the `-D` overrides are in
    /// ([`crate::config::BuildConfig::check_evaluated`]).
    #[test]
    fn unevaluable_bindings_reach_the_build_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("conf.py");
        std::fs::write(
            &path,
            "rst_epilog = f'{x}'\r\nrst_prolog = PROLOG\r\nrst_epilog += 'b'\r\n",
        )
        .unwrap();
        let config = PythonConfigParser::new()
            .unwrap()
            .parse_conf_py(&path)
            .unwrap()
            .to_build_config()
            .unwrap();
        let bindings: Vec<(&str, usize, &str, &str)> = config
            .unevaluable
            .iter()
            .map(|binding| {
                let (key, line) = (binding.key, binding.line);
                (key, line, binding.reason.as_str(), binding.file.as_str())
            })
            .collect();
        assert_eq!(
            bindings,
            [
                ("rst_epilog", 1, "unsupported expression", "conf.py"),
                ("rst_prolog", 2, "unsupported expression", "conf.py"),
            ]
        );
        assert_eq!(
            config.check_evaluated().unwrap_err().to_string(),
            "cannot evaluate rst_epilog (conf.py, line 1: unsupported expression): sphinx-ultra \
             reads conf.py without running it, so rst_prolog and rst_epilog must be string \
             literals; set it with -D rst_epilog=... to build without editing conf.py"
        );
    }

    /// An assignment to an attribute of that name is a binding no `-D`
    /// override replaces ([`Unevaluable::overridable`]): Sphinx 9.1.0
    /// reads it over the override, whether the key's other binding comes
    /// before or after it, at the top level or in a function that declares
    /// the name global (scratch `t17b-fix2/specs/mixed_top_attr_D.json`,
    /// `global_then_attr_D.json`, `attr_then_global_D.json`). So it stands
    /// beside the key's first binding an override replaces, never merged
    /// into it, and a key keeps its first binding of each kind. Its message
    /// offers no `-D`.
    #[test]
    fn an_attribute_binding_stands_beside_the_keys_other_bindings() {
        for (content, expected) in [
            (
                "rst_prolog = f'{x}'\ndef setup(app):\n    app.config.rst_prolog = 'b'\n",
                [("rst_prolog", 1, true), ("rst_prolog", 3, false)],
            ),
            (
                "def setup(app):\n    global rst_prolog\n    rst_prolog = 'c'\n    \
                 app.config.rst_prolog = 'b'\n",
                [("rst_prolog", 3, true), ("rst_prolog", 4, false)],
            ),
            (
                "def setup(app):\n    app.config.rst_prolog = 'b'\n    global rst_prolog\n    \
                 rst_prolog = 'c'\n",
                [("rst_prolog", 2, false), ("rst_prolog", 4, true)],
            ),
            (
                "def inited(app, config):\n    config.rst_epilog = 'a'\n    \
                 config.rst_epilog += 'b'\nrst_epilog = f'{x}'\nrst_epilog += 'c'\n",
                [("rst_epilog", 2, false), ("rst_epilog", 4, true)],
            ),
        ] {
            let p = parse(content);
            let bindings: Vec<(&str, usize, bool)> = p
                .unevaluable_bindings()
                .into_iter()
                .map(|binding| (binding.key, binding.line, binding.overridable))
                .collect();
            assert_eq!(bindings, expected, "{content}");
        }
        let p = parse("def setup(app):\n    app.config.rst_prolog = 'b'\n");
        assert_eq!(
            p.unevaluable().unwrap().to_string(),
            "cannot evaluate rst_prolog (conf.py, line 2: an assignment to an attribute of that \
             name): sphinx-ultra reads conf.py without running it, so rst_prolog and rst_epilog \
             must be string literals; Sphinx reads this binding over any -D rst_prolog=... \
             override, so edit conf.py"
        );
    }

    /// The conf.py reader's string literals, as CPython 3.12.2 compiles
    /// them (scratch `t17b-fix1/esc/cpython.out`, through the locked core
    /// profile): every escape of a `str` and a `bytes` literal, a raw
    /// literal's kept backslashes, implicit concatenation of a raw and a
    /// plain literal, and the literals CPython rejects. A `bytes` value
    /// reads as the `str` of its bytes' Latin-1 code points. Two values
    /// CPython gives fail here instead: a `\N{...}` escape (this crate has
    /// no Unicode name table) and a lone surrogate (a `String` holds none).
    #[test]
    fn string_literals_read_as_cpython_compiles_them() {
        let rows: [(&str, Result<&str, &str>, &str); 46] = [
            ("'a\\\nb'", Ok("ab"), "str"),
            (
                "\"\"\"\\\n.. |a| replace:: A\n\"\"\"",
                Ok(".. |a| replace:: A\n"),
                "str",
            ),
            ("'\\a\\b\\f\\v'", Ok("\u{7}\u{8}\u{c}\u{b}"), "str"),
            ("'\\n\\r\\t\\\\\\'\\\"'", Ok("\n\r\t\\'\""), "str"),
            ("'\\0'", Ok("\0"), "str"),
            ("'\\12'", Ok("\n"), "str"),
            ("'\\123'", Ok("S"), "str"),
            ("'\\1234'", Ok("S4"), "str"),
            ("'\\400'", Ok("\u{100}"), "str"),
            ("'\\777'", Ok("\u{1ff}"), "str"),
            ("'\\8'", Ok("\\8"), "str"),
            ("'\\9'", Ok("\\9"), "str"),
            ("'\\d'", Ok("\\d"), "str"),
            ("'\\q'", Ok("\\q"), "str"),
            ("'\\x41'", Ok("A"), "str"),
            ("'\\x4'", Err("truncated \\xXX escape"), ""),
            ("'\\xg1'", Err("truncated \\xXX escape"), ""),
            ("'\\xE9'", Ok("\u{e9}"), "str"),
            ("'\\u0041'", Ok("A"), "str"),
            ("'\\u004'", Err("truncated \\uXXXX escape"), ""),
            ("'\\u00e9x'", Ok("\u{e9}x"), "str"),
            ("'\\U00000041'", Ok("A"), "str"),
            ("'\\U0001F600'", Ok("\u{1f600}"), "str"),
            ("'\\U00110000'", Err("illegal Unicode character"), ""),
            ("'\\U0000004'", Err("truncated \\UXXXXXXXX escape"), ""),
            ("'\\N{BULLET}'", Err("a \\N{...} escape"), ""),
            ("'\\ud800'", Err("a lone surrogate"), ""),
            ("r'\\n'", Ok("\\n"), "str"),
            ("r'\\''", Ok("\\'"), "str"),
            ("R'\\x41'", Ok("\\x41"), "str"),
            ("r'a\\\nb'", Ok("a\\\nb"), "str"),
            ("'a' r'\\b'", Ok("a\\b"), "str"),
            ("r'\\b' '\\b'", Ok("\\b\u{8}"), "str"),
            ("b'\\x41'", Ok("A"), "bytes"),
            ("b'\\u0041'", Ok("\\u0041"), "bytes"),
            ("b'\\777'", Ok("\u{ff}"), "bytes"),
            ("b'\\N{BULLET}'", Ok("\\N{BULLET}"), "bytes"),
            ("b'\\a'", Ok("\u{7}"), "bytes"),
            ("rb'\\x41'", Ok("\\x41"), "bytes"),
            ("b'\\xff'", Ok("\u{ff}"), "bytes"),
            (
                "b'a' 'b'",
                Err("cannot mix bytes and nonbytes literals"),
                "",
            ),
            (
                "b'\u{e9}'",
                Err("bytes can only contain ASCII literal characters"),
                "",
            ),
            ("'\\\r\nb'", Ok("b"), "str"),
            ("'a\nb'", Err("unterminated string literal"), ""),
            ("u'\\x41'", Ok("A"), "str"),
            ("'\\x41\\\nx'", Ok("Ax"), "str"),
        ];
        for (src, expected, type_name) in rows {
            let got = parse_python_literal_typed(src);
            match expected {
                Ok(value) => assert_eq!(
                    got,
                    Ok((serde_json::Value::String(value.to_string()), type_name)),
                    "{src:?}"
                ),
                Err(reason) => assert_eq!(got, Err(reason.to_string()), "{src:?}"),
            }
        }

        // What CPython rejects or this reader cannot hold is any other
        // key's drop warning, as before.
        let p = parse("project = '\\x4'\n");
        assert_eq!(p.conf_namespace.get("project"), None);
        assert_eq!(
            p.warnings()[0].message,
            "unsupported value for 'project' dropped (truncated \\xXX escape): '\\x4'"
        );
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
