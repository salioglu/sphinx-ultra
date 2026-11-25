use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

    /// `today`, default `''` (`config.py:227`, rebuild class `'env'`): the
    /// text of the `|today|` default substitution when it is set
    /// ([`crate::read::ReadSettings::today`]).
    pub today: String,

    /// `today_fmt`, default `None` (`config.py:229`, rebuild class
    /// `'env'`): the strftime format `|today|` gives the build date when
    /// `today` is empty; unset, the language's own, `'%b %d, %Y'` in `en`
    /// ([`crate::read::ReadSettings::today_fmt`]).
    pub today_fmt: Option<String>,

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

    /// Tags set via `-t`, which `.. only::` reads beside the builder's own
    /// ([`crate::transforms::tags::Tags`])
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

    /// Every value `conf.py` assigned (plus `-D` overrides), as the literal
    /// it was assigned: Sphinx's `Config` namespace before any defaults.
    ///
    /// The typed fields above can't tell "unset" from "set to the default"
    /// (`html_static_path = ['_static']` is both sphinx-quickstart's
    /// default and this crate's), and Sphinx's `.buildinfo` hash covers
    /// every html-category key *as configured*; this map is how
    /// [`crate::builder::sphinx_build_info_contents`] tells the two apart,
    /// and hashes a key the typed fields don't model at all.
    pub conf_values: std::collections::BTreeMap<String, serde_json::Value>,

    /// Run directive/role validation during the build
    pub validate_directives: bool,

    /// `suppress_warnings`, default `[]` (`config.py:262`, a list or tuple):
    /// `type`, `type.*` or `type.subtype` patterns whose records are dropped
    /// before they are printed or counted
    /// ([`crate::diagnostics::WarningFilter`]).
    pub suppress_warnings: Vec<String>,

    /// `show_warning_types`, default `True` (`config.py:263`): whether a
    /// record carries its ` [type.subtype]` suffix
    /// ([`crate::diagnostics::Diagnostic::render`]).
    pub show_warning_types: bool,

    /// `keep_warnings`, default `False` (`config.py:261`, rebuild class
    /// `'env'`): whether `FilterSystemMessages` keeps the reporter's
    /// `system_message`s of level WARNING and up in the post-read doctree
    /// ([`crate::read::ReadSettings::keep_warnings`]). A `-D` override takes
    /// Sphinx's bool grammar, `0` or `1` only ([`STRICT_BOOL_KEYS`]).
    pub keep_warnings: bool,

    /// `smartquotes`, default `True` (`config.py:289`, rebuild class
    /// `'env'`): whether `SphinxSmartQuotes` (750) educates the quotes,
    /// dashes and ellipses of the read's text
    /// ([`crate::transforms::smartquotes::settings_for`]). The transform
    /// tests the value with `is False`
    /// (`SP/sphinx/transforms/__init__.py:389`), so only a `conf.py` that
    /// sets it to `False` turns it off; `0`, `None` and `''` leave it on.
    /// A `-D` override takes Sphinx's bool grammar, `0` or `1` only
    /// ([`STRICT_BOOL_KEYS`]).
    pub smartquotes: bool,

    /// `smartquotes_action`, default `'qDe'` (`config.py:290`, rebuild
    /// class `'env'`): what the transform educates, docutils'
    /// `educate_tokens` attribute string
    /// ([`crate::transforms::smartquotes::educate::Actions::parse`]).
    pub smartquotes_action: String,

    /// `smartquotes_excludes` (`config.py:291-295`, rebuild class `'env'`):
    /// the languages and builders the transform does not run for
    /// ([`SmartquotesExcludes`]). A dict, so a `-D` override sets one key
    /// of it to the string as given (`-D smartquotes_excludes.builders=man`),
    /// never the whole, and when `conf.py` did not set the dict, the
    /// override starts a new one, so the other key excludes nothing
    /// ([`Self::apply_override`]).
    pub smartquotes_excludes: SmartquotesExcludes,

    /// `trim_doctest_flags`, default `True` (`config.py:267`, rebuild class
    /// `'env'`): whether `TrimDoctestFlagsTransform`, the post-transform at
    /// priority 401, strips `# doctest: +FLAG` comments and `<BLANKLINE>`
    /// markers from doctest blocks and Python console literal blocks
    /// ([`crate::transforms::post`]). A `-D` override takes Sphinx's bool
    /// grammar, `0` or `1` only ([`STRICT_BOOL_KEYS`]).
    pub trim_doctest_flags: bool,

    /// `primary_domain`, default `'py'` (`config.py:268`, rebuild class
    /// `'env'`, `None` allowed): every document's default domain
    /// ([`crate::rst::ReadConfig::primary_domain`]). A name no domain is
    /// registered under warns at `config-inited` ([`Self::validate`]) and,
    /// like `None`, leaves documents without a default domain.
    pub primary_domain: Option<String>,

    /// `highlight_language`, default `'default'` (`config.py:257`, rebuild
    /// class `'env'`): the language of a `code-block` or `code` directive
    /// with no argument and no `.. highlight::` before it
    /// ([`crate::rst::ReadConfig::highlight_language`]).
    pub highlight_language: String,

    /// `default_role`, default `None` (`config.py:247`, rebuild class
    /// `'env'`): the role bare interpreted text has in every document
    /// ([`crate::rst::ReadConfig::default_role`]).
    pub default_role: Option<String>,

    /// `manpages_url`, default `None` (`config.py:271`, rebuild class
    /// `'env'`): where the `manpage` role links a page
    /// ([`crate::rst::ReadConfig::manpages_url`]).
    pub manpages_url: Option<String>,

    /// `rst_prolog`, default `None` (`config.py:266`, rebuild class
    /// `'env'`): text read before every document's own lines
    /// ([`crate::rst::ReadConfig::rst_prolog`]). A `conf.py` value this
    /// crate cannot evaluate is a configuration error, never dropped
    /// ([`crate::python_config`]).
    pub rst_prolog: Option<String>,

    /// `rst_epilog`, default `None` (`config.py:265`, rebuild class
    /// `'env'`): text read after every document's last line
    /// ([`crate::rst::ReadConfig::rst_epilog`]), evaluated as `rst_prolog`
    /// is.
    pub rst_epilog: Option<String>,

    /// `math_number_all`, default `False` (`config.py:282`, rebuild class
    /// `'env'`): the `math` directive numbers an equation with no label
    /// too ([`crate::rst::ReadConfig::math_number_all`]). A `-D` override
    /// takes Sphinx's bool grammar ([`STRICT_BOOL_KEYS`]).
    pub math_number_all: bool,

    /// `math_eqref_format`, default `None` (`config.py:283`, rebuild class
    /// `'env'`): the text of a reference to an equation, its `{number}`
    /// formatted in; `None` is `({number})`
    /// ([`crate::env::math::resolve_xref`]).
    pub math_eqref_format: Option<String>,

    /// `math_numfig`, default `True` (`config.py:284`, rebuild class
    /// `'env'`): with `numfig`, an equation's number is the one the
    /// toctree's figure numbering gave it. A `-D` override takes Sphinx's
    /// bool grammar ([`STRICT_BOOL_KEYS`]).
    pub math_numfig: bool,

    /// `math_numsep`, default `'.'` (`config.py:285`, rebuild class
    /// `'env'`): what separates an equation's own number from its
    /// section's in such a number.
    pub math_numsep: String,

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
    /// report — `(key, python type name)` for a key of
    /// [`CONFVAL_TYPE_CHECKED_KEYS`] whose value arrived as some other type.
    /// For the two `int | None` keys that is a `-D` override (always
    /// `str`, because `convert_overrides` has no int branch for a key whose
    /// default is `None` and returns the raw string, `config.py:397`) or a
    /// mistyped `conf.py` assignment; for `smartquotes`, a `conf.py`
    /// assignment of a type that shares no base with `bool` (an `int`
    /// does), which a `-D smartquotes=0|1` override replaces; for
    /// `rst_prolog` and `rst_epilog`, a `conf.py` value that is neither a
    /// `str` nor `None` (a `-D` override is a `str`). Diagnostic
    /// state rather than configuration: skipped by serde, so it neither
    /// enters the cache fingerprint nor survives a save/load.
    #[serde(skip)]
    pub confval_type_mismatches: Vec<(String, String)>,

    /// The `conf.py` bindings of `rst_prolog` and `rst_epilog` this crate
    /// could not evaluate, the first by line first
    /// ([`crate::python_config::Unevaluable`]): a configuration error once
    /// the `-D` overrides are in, for a key no override set, and for a
    /// binding no override replaces, of an attribute or an item of that
    /// name ([`crate::python_config::Unevaluable::overridable`],
    /// [`Self::check_evaluated`]). Sphinx reads an override, never what
    /// `conf.py` bound the module's name to. Diagnostic state like
    /// [`Self::confval_type_mismatches`]: skipped by serde.
    #[serde(skip)]
    pub unevaluable: Vec<crate::python_config::Unevaluable>,

    /// `(key, python type name)` for a `conf.py` `rst_prolog` or
    /// `rst_epilog` that is true and no `str`, which the typed fields
    /// cannot hold (they read `None`): Sphinx's reader raises
    /// `AttributeError` on its `splitlines` as it reads the first document
    /// (decision 13, [`crate::rst::ReadConfig::decoration_crash`]). A `-D`
    /// override, a `str`, clears its key's. Skipped by serde, it enters
    /// the cache fingerprint all the same (`builder::config_fingerprint`):
    /// it changes what every read does.
    #[serde(skip)]
    pub decoration_crashes: Vec<(String, String)>,

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

    /// `intersphinx_mapping`, normalised and validated
    /// (`ext/intersphinx/_load.py:38-136`): project name -> (target URI,
    /// inventory locations). [`Self::init_intersphinx_mapping`] fills it
    /// from the `conf.py` value at `config-inited`, after the `-D`
    /// overrides, and only with `sphinx.ext.intersphinx` loaded; a mapping
    /// that fails validation is then an error, exactly as Sphinx's
    /// `ConfigError` aborts the build — see
    /// [`crate::intersphinx::validate_mapping`].
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

/// The keys whose value `check_confval_types` (`config.py:775-847`) can
/// find mistyped here ([`BuildConfig::confval_type_mismatches`]), in
/// Sphinx's registration order, the order it reports them in, each with the
/// types its warning names, `sorted` by their backticked spelling:
/// `rst_epilog` and `rst_prolog` (`config.py:265-266`), the two
/// `int | None` keys ([`NONE_DEFAULT_INT_KEYS`]) and, between those,
/// `smartquotes` (`config.py:289`), a core key, where the python domain
/// registers the second when it is set up.
const CONFVAL_TYPE_CHECKED_KEYS: [(&str, &str); 5] = [
    ("rst_epilog", "`str'"),
    ("rst_prolog", "`str'"),
    ("maximum_signature_line_length", "`NoneType' or `int'"),
    ("smartquotes", "`bool'"),
    (
        "python_maximum_signature_line_length",
        "`NoneType' or `int'",
    ),
];

/// The config keys whose only valid type is `bool` and that this crate
/// types, whose `-D` value takes Sphinx's bool grammar: `convert_overrides`
/// turns `'0'` and `'1'` into `False` and `True` and raises `ConfigError`
/// for anything else (`config.py:366-374`). The crate's other bool keys
/// still also take `true`/`false`.
const STRICT_BOOL_KEYS: [&str; 5] = [
    "keep_warnings",
    "trim_doctest_flags",
    "smartquotes",
    "math_number_all",
    "math_numfig",
];

/// `smartquotes_excludes` as `SphinxSmartQuotes.is_available` reads it
/// (`SP/sphinx/transforms/__init__.py:383-397`): the two keys of a dict,
/// each read with `.get(key, [])` and tested with Python's `in`
/// ([`ExcludedNames`]): `name in builders` and `config.language in
/// languages`. Unset, the dict is Sphinx's default ([`Default`]:
/// `{'languages': ['ja', 'zh_CN', 'zh_TW'], 'builders': ['man', 'text']}`,
/// `config.py:291-295`); a dict that leaves a key out excludes nothing by
/// that key, which is what an empty list reads as.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartquotesExcludes {
    /// The values of `language` the transform does not run for.
    #[serde(default)]
    pub languages: ExcludedNames,
    /// The names of the builders the transform does not run for
    /// ([`crate::builder::BuilderKind::name`]).
    #[serde(default)]
    pub builders: ExcludedNames,
}

impl Default for SmartquotesExcludes {
    fn default() -> Self {
        let names = |values: &[&str]| {
            ExcludedNames::Names(values.iter().map(|value| value.to_string()).collect())
        };
        Self {
            languages: names(&["ja", "zh_CN", "zh_TW"]),
            builders: names(&["man", "text"]),
        }
    }
}

impl SmartquotesExcludes {
    /// An empty dict, `{}`, whose keys both exclude nothing: what a dotted
    /// `-D smartquotes_excludes.<key>` override starts from when `conf.py`
    /// set no dict ([`BuildConfig::apply_override`]).
    pub fn empty() -> Self {
        Self {
            languages: ExcludedNames::default(),
            builders: ExcludedNames::default(),
        }
    }
}

/// One value of [`SmartquotesExcludes`], as Python's `name in value` tests
/// a name against it ([`Self::contains`]). Another value (`None`, a number)
/// makes Sphinx raise `TypeError` when the transform runs, which is not
/// modelled (decision 13): it is read as no names.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ExcludedNames {
    /// A list or a tuple, or a dict, whose keys `in` tests: a name is in it
    /// when it is one of them, whole (`zh_CN` does not exclude `zh-CN`).
    Names(Vec<String>),
    /// A string, which `in` searches: a name is in it when it occurs in it
    /// (`'html' in 'dirhtml'`, `'en' in 'en_GB'`). A `-D
    /// smartquotes_excludes.<key>` override is one, the string as given
    /// ([`BuildConfig::apply_override`]).
    Text(String),
}

impl Default for ExcludedNames {
    /// `[]`, what `.get(key, [])` gives for a key the dict leaves out.
    fn default() -> Self {
        ExcludedNames::Names(Vec::new())
    }
}

impl ExcludedNames {
    /// Python's `name in value`.
    pub fn contains(&self, name: &str) -> bool {
        match self {
            ExcludedNames::Names(names) => names.iter().any(|known| known == name),
            ExcludedNames::Text(text) => text.contains(name),
        }
    }
}

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
            today: String::new(),
            today_fmt: None,
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
            conf_values: std::collections::BTreeMap::new(),
            validate_directives: true,
            suppress_warnings: Vec::new(),
            show_warning_types: true,
            keep_warnings: false,
            smartquotes: true,
            smartquotes_action: "qDe".to_string(),
            smartquotes_excludes: SmartquotesExcludes::default(),
            trim_doctest_flags: true,
            primary_domain: Some("py".to_string()),
            highlight_language: "default".to_string(),
            default_role: None,
            manpages_url: None,
            rst_prolog: None,
            rst_epilog: None,
            math_number_all: false,
            math_eqref_format: None,
            math_numfig: true,
            math_numsep: ".".to_string(),

            numfig: false,
            numfig_format: default_numfig_format(),
            numfig_secnum_depth: 1,
            source_encoding: DEFAULT_SOURCE_ENCODING.to_string(),
            confval_type_mismatches: Vec::new(),
            unevaluable: Vec::new(),
            decoration_crashes: Vec::new(),

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
        let parsed = parser.parse_conf_py(conf_py_path);
        // Silent dropping is banned: surface every construct the parser
        // could not handle, before the configuration error a value it must
        // evaluate and cannot makes.
        for warning in parser.warnings() {
            log::warn!(
                "{}:{}: {}",
                conf_py_path.display(),
                warning.line,
                warning.message
            );
        }
        parsed?.to_build_config()
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
    ///
    /// `check_primary_domain` (800, registered after
    /// `check_confval_types`, `config.py:854-858`) closes the list: a
    /// `primary_domain` that names no registered domain warns. Sphinx then
    /// sets it to `None`; [`crate::rst::ReadConfig`] reads such a name as
    /// no domain anyway.
    ///
    /// The two halves, [`Self::validate_source_encoding`] (790) and
    /// [`Self::validate_confval_types`] (800, `sphinx.config`'s), are
    /// apart in a build: the HTML builder's 800 handlers run between them,
    /// and `correct_copyright_year` before the second.
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = self.validate_source_encoding();
        warnings.extend(self.validate_confval_types());
        warnings
    }

    /// [`Self::validate`]'s first half: `deprecate_source_encoding`
    /// (`config-inited` @790), and this crate's own note on a codec it
    /// cannot decode.
    pub fn validate_source_encoding(&self) -> Vec<String> {
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
        warnings
    }

    /// [`Self::validate`]'s second half: `check_confval_types`, then
    /// `check_primary_domain` (`config-inited` @800, registered by
    /// `sphinx.config` in that order, after `correct_copyright_year`).
    pub fn validate_confval_types(&self) -> Vec<String> {
        let mut warnings = Vec::new();

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
        // a permitted type and shares no non-trivial base with the default's
        // type (an `int` does with `bool`), so the warning names the
        // permitted set, `sorted` by the backticked spelling — `NoneType'
        // before `int' (N < i).
        for (key, permitted) in CONFVAL_TYPE_CHECKED_KEYS {
            if let Some((_, type_name)) = self
                .confval_type_mismatches
                .iter()
                .find(|(mismatched, _)| mismatched == key)
            {
                warnings.push(format!(
                    "The config value `{key}' has type `{type_name}'; expected {permitted}."
                ));
            }
        }
        // config-inited @800: `check_primary_domain`, registered after
        // `check_confval_types`.
        if let Some(primary_domain) = self
            .primary_domain
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            if crate::rst::roles::DomainId::from_name(primary_domain).is_none() {
                warnings.push(format!(
                    "primary_domain {} not found, ignored.",
                    crate::utils::py_repr_str(primary_domain)
                ));
            }
        }
        warnings
    }

    /// Fails with the configuration error of the first `conf.py` binding
    /// of `rst_prolog` or `rst_epilog` this crate could not evaluate and no
    /// `-D` override replaced ([`Self::unevaluable`]): run once the
    /// overrides are in. A binding no override replaces comes first,
    /// whatever its line
    /// ([`crate::python_config::Unevaluable::overridable`]): no override
    /// builds while one stands, so no error that offers one is shown
    /// before it. The error names the key and, for a binding an override
    /// replaces, the override that builds without an edit to `conf.py`.
    pub fn check_evaluated(&self) -> Result<()> {
        let first = self
            .unevaluable
            .iter()
            .find(|binding| !binding.overridable)
            .or(self.unevaluable.first());
        match first {
            Some(binding) => Err(anyhow::anyhow!("{binding}")),
            None => Ok(()),
        }
    }

    /// Record that `key` (one of [`CONFVAL_TYPE_CHECKED_KEYS`]) received a
    /// value of python type `type_name`, for [`Self::validate`] to report.
    /// One entry per key, like sphinx's `once=True`.
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

    /// Whether `sphinx.ext.intersphinx` is in `extensions`. Without it,
    /// Sphinx registers none of intersphinx's configuration values, loads
    /// no inventory and has no `:external:` role: `intersphinx_mapping` is
    /// an ordinary `conf.py` variable ([TW F12]).
    pub fn intersphinx_enabled(&self) -> bool {
        self.extensions
            .iter()
            .any(|extension| extension == "sphinx.ext.intersphinx")
    }

    /// `validate_intersphinx_mapping` (`ext/intersphinx/_load.py:38-136`),
    /// the `config-inited` handler `sphinx.ext.intersphinx` registers
    /// (priority 800, after Sphinx's own): with the extension, normalise
    /// `intersphinx_mapping` as `conf.py` wrote it ([`Self::conf_values`]),
    /// or fail with the `ERROR` records it logs for each rejected entry and
    /// the `ConfigError` it raises; a mapping no `conf.py` wrote (a YAML or
    /// JSON configuration's typed value) stands. Without the extension
    /// there is no such configuration value, and no inventory loads.
    pub fn init_intersphinx_mapping(&mut self) -> Result<(), crate::error::ConfigInitError> {
        if !self.intersphinx_enabled() {
            self.intersphinx_mapping.clear();
            return Ok(());
        }
        let Some(raw) = self.conf_values.get("intersphinx_mapping") else {
            return Ok(());
        };
        let (mapping, errors) = crate::intersphinx::validate_mapping(raw);
        if !errors.is_empty() {
            return Err(crate::error::ConfigInitError {
                message: crate::intersphinx::mapping_config_error(errors.len()),
                errors,
            });
        }
        self.intersphinx_mapping = mapping;
        Ok(())
    }

    /// `validate_html_logo`/`validate_html_favicon`
    /// (`builders/html/__init__.py:1404-1423`, run at `config-inited`): a
    /// logo or favicon that is neither a file under the confdir nor a URL
    /// is warned about and unset, so the build — and the `.buildinfo`
    /// hash — see `None`. Returns the warnings, worded as Sphinx words them.
    pub fn validate_html_logo_and_favicon(&mut self, confdir: &Path) -> Vec<String> {
        let mut messages = Vec::new();
        let mut check = |key: &str, label: &str, field: &mut Option<String>| {
            let Some(value) = field.as_deref() else {
                return;
            };
            // `is_url`: `'://' in url`; an empty value is falsy and skipped.
            if value.is_empty() || value.contains("://") || confdir.join(value).is_file() {
                return;
            }
            messages.push(format!(
                "{label} file {} does not exist",
                crate::utils::py_repr_str(value)
            ));
            *field = None;
            self.conf_values
                .insert(key.to_string(), serde_json::Value::Null);
        };
        let mut html_logo = self.html_logo.take();
        check("html_logo", "logo", &mut html_logo);
        let mut html_favicon = self.html_favicon.take();
        check("html_favicon", "favicon", &mut html_favicon);
        self.html_logo = html_logo;
        self.html_favicon = html_favicon;
        messages
    }

    /// Apply a `-D key=value` override (sphinx-build semantics): the value is
    /// coerced to the type the field already has, dotted keys reach the nested
    /// sections (`output.*`, `theme.*`) and map-typed settings
    /// (`numfig_format.figure`), and an unknown key warns and is ignored
    /// rather than failing the build. A dotted `html_context.<entry>` or
    /// `smartquotes_excludes.<entry>` sets that entry to the string as
    /// given, as Sphinx's `Config.__init__` does.
    ///
    /// Returns the sphinx-style warning message when the override was ignored
    /// — the caller decides how to report it (it must count toward `-W`).
    pub fn apply_override(&mut self, key: &str, value: &str) -> Result<Option<String>> {
        // `html_theme` is the Sphinx name; it lives in two places here.
        // Fan aliases out first so both copies stay in sync.
        match key {
            "html_theme" => {
                self.apply_override("output.html_theme", value)?;
                let result = self.apply_override("theme.name", value)?;
                self.conf_values.insert(
                    key.to_string(),
                    serde_json::Value::String(value.to_string()),
                );
                return Ok(result);
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

        // Sphinx's bool grammar: anything but `0` and `1` is a `ConfigError`,
        // which aborts configuration (`config.py:366-374`).
        if STRICT_BOOL_KEYS.contains(&key) && !matches!(value, "0" | "1") {
            anyhow::bail!("'{key}' must be '0' or '1', got '{value}'");
        }

        if let Some(entry) = key.strip_prefix("smartquotes_excludes.") {
            self.override_smartquotes_excludes(entry, value);
            return Ok(None);
        }
        // `Config.__init__` (`config.py:309-312`):
        // `raw_config.setdefault('html_context', {})[entry] = value`, the
        // entry being everything after the first dot and the value the
        // string as given, which `convert_overrides` never sees: no number
        // is read from it, and it replaces whatever `conf.py` put under that
        // entry, a dict included, without a warning.
        if let Some(entry) = key.strip_prefix("html_context.") {
            self.html_context.insert(
                entry.to_string(),
                serde_json::Value::String(value.to_string()),
            );
            return Ok(None);
        }

        let mut tree = serde_json::to_value(&*self)?;

        // Resolve the dotted path. A key missing from its parent object is
        // inserted as Null (map-typed settings like numfig_format accept new
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
        applied.unevaluable = std::mem::take(&mut self.unevaluable);
        applied.decoration_crashes = std::mem::take(&mut self.decoration_crashes);
        // `check_confval_types` reads the override, which `Config.__getattr__`
        // takes before `conf.py`'s value, and a bool override has the one
        // type the key allows, as a `str` key's does: nothing to report
        // about conf.py's value.
        if STRICT_BOOL_KEYS.contains(&key) || matches!(key, "rst_prolog" | "rst_epilog") {
            applied
                .confval_type_mismatches
                .retain(|(mismatched, _)| mismatched != key);
        }
        // Nor does Sphinx read the prolog or epilog `conf.py` bound the
        // module's name to, which this crate may not have evaluated, or on
        // which Sphinx's reader would crash: an override of the key
        // replaces it. It does not replace a binding of an attribute or an
        // item of that name (`Unevaluable::overridable`): that binding
        // stays.
        if matches!(key, "rst_prolog" | "rst_epilog") {
            applied
                .unevaluable
                .retain(|binding| binding.key != key || !binding.overridable);
            applied
                .decoration_crashes
                .retain(|(crashing, _)| crashing != key);
        }
        // A top-level key is a Sphinx setting name: the `.buildinfo` hash
        // sees the override as conf.py would have. (A dotted key names
        // this crate's own nesting, not a conf.py setting.)
        if !key.contains('.') && key != "conf_values" {
            if let Some(value) = check.get(key) {
                applied.conf_values.insert(key.to_string(), value.clone());
            }
        }
        *self = applied;
        Ok(None)
    }

    /// A `-D smartquotes_excludes.<entry>=<value>` override, as Sphinx's
    /// `Config.__init__` applies a dotted override (`config.py:309-312`):
    /// `raw_config.setdefault('smartquotes_excludes', {})[entry] = value`.
    /// The value is the string as given, which `convert_overrides` never
    /// sees, so it is not split, and `in` searches it
    /// ([`ExcludedNames::Text`]). It goes into the dict `conf.py` assigned
    /// or an earlier such override made, which [`Self::conf_values`] holds
    /// as Sphinx's `raw_config` does; with neither, into a new empty dict,
    /// so the default's other key is gone and excludes nothing. An entry
    /// the transform never reads makes the dict all the same, and Sphinx
    /// reports no dotted override as unknown.
    fn override_smartquotes_excludes(&mut self, entry: &str, value: &str) {
        use serde_json::{Map, Value};
        if !self.conf_values.contains_key("smartquotes_excludes") {
            self.smartquotes_excludes = SmartquotesExcludes::empty();
            self.conf_values.insert(
                "smartquotes_excludes".to_string(),
                Value::Object(Map::new()),
            );
        }
        // A `conf.py` value that is no dict makes Sphinx raise `TypeError`
        // here, which is not modelled (decision 13).
        if let Some(Value::Object(raw)) = self.conf_values.get_mut("smartquotes_excludes") {
            raw.insert(entry.to_string(), Value::String(value.to_string()));
        }
        let names = ExcludedNames::Text(value.to_string());
        match entry {
            "languages" => self.smartquotes_excludes.languages = names,
            "builders" => self.smartquotes_excludes.builders = names,
            _ => {}
        }
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

    /// Sphinx's `Config.__init__` puts a dotted `-D name.key=value` into
    /// the raw configuration as given (`config.py:309-312`): the key is
    /// everything after the first dot, and the value is the `str` typed,
    /// never a number, over whatever `conf.py` put under that key, with no
    /// warning. The values are `app.config.html_context` as Sphinx 9.1.0
    /// holds it after these overrides (probed).
    #[test]
    fn a_dotted_html_context_override_keeps_the_text_given() {
        for text in ["0.00001", "1e-7", "1e16", "inf", "007"] {
            let mut config = BuildConfig::default();
            assert_eq!(
                config.apply_override("html_context.x", text).unwrap(),
                None,
                "{text}"
            );
            assert_eq!(
                serde_json::to_value(&config.html_context).unwrap(),
                serde_json::json!({ "x": text }),
                "{text}"
            );
        }

        // `conf.py`'s `html_context = {'x': 1, 'y': True, 'z': [1, 2],
        // 'w': {'v': 1}}`.
        let mut config = BuildConfig {
            html_context: serde_json::from_value(serde_json::json!({
                "x": 1, "y": true, "z": [1, 2], "w": { "v": 1 }
            }))
            .unwrap(),
            ..BuildConfig::default()
        };
        for (key, value) in [
            ("html_context.x", "2"),
            ("html_context.y", "yes"),
            ("html_context.z", "a,b"),
            ("html_context.w", "u"),
            ("html_context.a.b", "c"),
        ] {
            assert_eq!(config.apply_override(key, value).unwrap(), None, "{key}");
        }
        assert_eq!(
            serde_json::to_value(&config.html_context).unwrap(),
            serde_json::json!({ "x": "2", "y": "yes", "z": "a,b", "w": "u", "a.b": "c" })
        );
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

    /// Sphinx validates `intersphinx_mapping` at `config-inited` only with
    /// `sphinx.ext.intersphinx` loaded — after the `-D` overrides, so
    /// `-D extensions=sphinx.ext.intersphinx` loads it (`config.py:316-321`)
    /// — and aborts with `ConfigError` after logging each rejected entry.
    /// Without the extension the mapping is an ordinary variable: nothing
    /// is validated and no inventory loads. A mapping that no `conf.py`
    /// wrote (a YAML or JSON configuration's typed value) stands.
    #[test]
    fn intersphinx_mapping_needs_the_extension() {
        let temp_dir = TempDir::new().unwrap();
        let p = temp_dir.path().join("conf.py");
        let duplicated =
            "intersphinx_mapping = {'a': ('https://x/', None), 'b': ('https://x/', None)}\n";
        fs::write(&p, duplicated).unwrap();
        let mut config = BuildConfig::from_file(&p).expect("loading does not validate it");
        assert!(!config.intersphinx_enabled());
        config.init_intersphinx_mapping().expect("not validated");
        assert!(config.intersphinx_mapping.is_empty());

        let mut config = BuildConfig::from_file(&p).unwrap();
        assert_eq!(
            config
                .apply_override("extensions", "sphinx.ext.intersphinx")
                .unwrap(),
            None
        );
        assert!(config.intersphinx_enabled());
        let err = config
            .init_intersphinx_mapping()
            .expect_err("a duplicate target URI must abort");
        assert_eq!(
            err.to_string(),
            "Invalid `intersphinx_mapping` configuration (1 error)."
        );
        assert_eq!(
            err.errors,
            [
                "Invalid target URI value `'https://x/'` in intersphinx_mapping['b'][0]. \
              Target URIs must be unique (other instance in intersphinx_mapping['a'])."
            ]
        );

        fs::write(
            &p,
            "extensions = ['sphinx.ext.intersphinx']\n\
             intersphinx_mapping = {'a': ('https://x/', None)}\n",
        )
        .unwrap();
        let mut config = BuildConfig::from_file(&p).unwrap();
        config.init_intersphinx_mapping().unwrap();
        assert_eq!(
            config.intersphinx_mapping["a"],
            ("https://x/".to_string(), vec![None])
        );
        // Without the extension, even a valid mapping loads nothing.
        config.extensions.clear();
        config.init_intersphinx_mapping().unwrap();
        assert!(config.intersphinx_mapping.is_empty());

        let mut typed = BuildConfig {
            extensions: vec!["sphinx.ext.intersphinx".to_string()],
            ..BuildConfig::default()
        };
        typed
            .intersphinx_mapping
            .insert("t".to_string(), ("https://t/".to_string(), vec![None]));
        typed.init_intersphinx_mapping().unwrap();
        assert_eq!(typed.intersphinx_mapping.len(), 1);
    }

    /// `primary_domain` and `highlight_language` are read-side keys with
    /// Sphinx's defaults (`config.py:257`, `:268`). Their `-D` value is the
    /// string `convert_overrides` returns for a `str` default, `'None'`
    /// included; the parse reads them, and the extension list, through
    /// [`crate::rst::ReadConfig`].
    #[test]
    fn read_config_keys_take_sphinxs_defaults_and_overrides() {
        // Sphinx's own `extensions` default is empty; this crate's default
        // configuration lists three extensions, intersphinx among them.
        assert!(crate::rst::ReadConfig::from(&BuildConfig::default()).intersphinx);
        let mut config = BuildConfig {
            extensions: Vec::new(),
            ..BuildConfig::default()
        };
        assert_eq!(config.primary_domain.as_deref(), Some("py"));
        assert_eq!(config.highlight_language, "default");
        assert_eq!(
            crate::rst::ReadConfig::from(&config),
            crate::rst::ReadConfig::default()
        );

        assert_eq!(config.apply_override("primary_domain", "js").unwrap(), None);
        assert_eq!(
            config
                .apply_override("highlight_language", "python3")
                .unwrap(),
            None
        );
        assert_eq!(
            config
                .apply_override("extensions", "sphinx.ext.intersphinx")
                .unwrap(),
            None
        );
        assert_eq!(
            crate::rst::ReadConfig::from(&config),
            crate::rst::ReadConfig {
                primary_domain: Some("js".to_string()),
                highlight_language: "python3".to_string(),
                intersphinx: true,
                ..crate::rst::ReadConfig::default()
            }
        );
        assert_eq!(
            config.conf_values.get("highlight_language"),
            Some(&serde_json::Value::String("python3".to_string()))
        );

        assert_eq!(
            config.apply_override("primary_domain", "None").unwrap(),
            None
        );
        assert_eq!(config.primary_domain.as_deref(), Some("None"));
        let mut unset = BuildConfig {
            primary_domain: None,
            ..BuildConfig::default()
        };
        assert_eq!(unset.apply_override("primary_domain", "c").unwrap(), None);
        assert_eq!(unset.primary_domain.as_deref(), Some("c"));
    }

    /// `-D rst_prolog=...` and `-D rst_epilog=...` are the string as given
    /// (`convert_overrides`: a `str` key, `config.py:395-396`), even one
    /// that spells a number, and reach the parse through
    /// [`crate::rst::ReadConfig`]. `check_confval_types` then reads the
    /// override, a `str`: a mistyped `conf.py` value is not reported.
    #[test]
    fn prolog_and_epilog_take_overrides() {
        let mut config = BuildConfig::default();
        config.note_confval_type_mismatch("rst_epilog", "int");
        config.decoration_crashes = vec![("rst_epilog".to_string(), "int".to_string())];
        config.unevaluable = vec![crate::python_config::Unevaluable {
            key: "rst_prolog",
            line: 3,
            reason: "unsupported expression".to_string(),
            file: "conf.py".to_string(),
            overridable: true,
        }];
        for (key, value) in [("rst_prolog", ".. |a| replace:: A"), ("rst_epilog", "1")] {
            assert_eq!(config.apply_override(key, value).unwrap(), None, "{key}");
        }
        assert_eq!(config.validate_confval_types(), Vec::<String>::new());
        // Sphinx reads the override, never what conf.py bound the key to:
        // neither the binding this crate could not evaluate nor the value
        // its reader crashes on.
        assert!(config.check_evaluated().is_ok());
        assert_eq!(config.decoration_crashes, Vec::new());
        assert_eq!(
            crate::rst::ReadConfig::from(&config),
            crate::rst::ReadConfig {
                rst_prolog: Some(".. |a| replace:: A".to_string()),
                rst_epilog: Some("1".to_string()),
                ..crate::rst::ReadConfig::from(&BuildConfig::default())
            }
        );
        assert_eq!(
            config.conf_values.get("rst_epilog"),
            Some(&serde_json::Value::String("1".to_string()))
        );
    }

    /// An override of one key leaves the other's unevaluable binding and
    /// crash in place, and an unrelated override keeps both
    /// ([`BuildConfig::check_evaluated`],
    /// [`crate::rst::ReadConfig::decoration_crash`], the prolog's first).
    #[test]
    fn an_override_replaces_only_its_own_key() {
        let binding = |key| crate::python_config::Unevaluable {
            key,
            line: 2,
            reason: "an import".to_string(),
            file: "conf.py".to_string(),
            overridable: true,
        };
        let mut config = BuildConfig {
            unevaluable: vec![binding("rst_epilog")],
            decoration_crashes: vec![
                ("rst_prolog".to_string(), "list".to_string()),
                ("rst_epilog".to_string(), "dict".to_string()),
            ],
            ..BuildConfig::default()
        };
        assert_eq!(
            crate::rst::ReadConfig::from(&config)
                .decoration_crash
                .as_deref(),
            Some("list")
        );
        for (key, value) in [("project", "P"), ("rst_prolog", "x")] {
            assert_eq!(config.apply_override(key, value).unwrap(), None, "{key}");
        }
        assert_eq!(
            config.check_evaluated().unwrap_err().to_string(),
            "cannot evaluate rst_epilog (conf.py, line 2: an import): sphinx-ultra reads \
             conf.py without running it, so rst_prolog and rst_epilog must be string literals; \
             set it with -D rst_epilog=... to build without editing conf.py"
        );
        assert_eq!(
            crate::rst::ReadConfig::from(&config)
                .decoration_crash
                .as_deref(),
            Some("dict")
        );
    }

    /// An override of a key keeps the key's bindings no override
    /// replaces, of an attribute or an item of that name
    /// ([`crate::python_config::Unevaluable::overridable`]), and the
    /// configuration error reports such a binding before any other,
    /// whatever its line: no override builds while one stands, so its
    /// message offers none, and no message that offers one is shown
    /// while one stands.
    #[test]
    fn an_override_keeps_a_binding_sphinx_reads_over_it() {
        let binding = |key, line, overridable| crate::python_config::Unevaluable {
            key,
            line,
            reason: if overridable {
                "unsupported expression".to_string()
            } else {
                "an assignment to an attribute of that name".to_string()
            },
            file: "conf.py".to_string(),
            overridable,
        };
        let mut config = BuildConfig {
            unevaluable: vec![
                binding("rst_prolog", 1, true),
                binding("rst_epilog", 2, true),
                binding("rst_prolog", 5, false),
            ],
            ..BuildConfig::default()
        };
        let attribute = "cannot evaluate rst_prolog (conf.py, line 5: an assignment to an \
                         attribute of that name): sphinx-ultra reads conf.py without running \
                         it, so rst_prolog and rst_epilog must be string literals; conf.py \
                         changes an attribute or item of that name, which a -D rst_prolog=... \
                         override does not replace, so edit conf.py";
        assert_eq!(config.check_evaluated().unwrap_err().to_string(), attribute);
        assert_eq!(config.apply_override("rst_prolog", "x").unwrap(), None);
        let left: Vec<(&str, usize)> = config
            .unevaluable
            .iter()
            .map(|binding| (binding.key, binding.line))
            .collect();
        assert_eq!(left, [("rst_epilog", 2), ("rst_prolog", 5)]);
        assert_eq!(config.check_evaluated().unwrap_err().to_string(), attribute);
        assert_eq!(config.apply_override("rst_epilog", "y").unwrap(), None);
        assert_eq!(config.check_evaluated().unwrap_err().to_string(), attribute);
    }

    /// The read-side keys of wave 5a T17a take a `-D` override as Sphinx's
    /// `convert_overrides` gives it: the string for a `str`- or
    /// `None`-defaulted key, the bool grammar `0`/`1` for the math domain's
    /// two bools (any other value a `ConfigError`). They reach the parse
    /// through [`crate::rst::ReadConfig`] and the resolution through
    /// [`crate::env::math::MathConfig`].
    #[test]
    fn role_and_math_keys_take_overrides() {
        let mut config = BuildConfig::default();
        for (key, value) in [
            ("default_role", "obj"),
            ("manpages_url", "https://man/{path}"),
            ("math_number_all", "1"),
            ("math_eqref_format", "({number})"),
            ("math_numfig", "0"),
            ("math_numsep", "-"),
        ] {
            assert_eq!(config.apply_override(key, value).unwrap(), None, "{key}");
        }
        assert_eq!(
            crate::rst::ReadConfig::from(&config),
            crate::rst::ReadConfig {
                default_role: Some("obj".to_string()),
                manpages_url: Some("https://man/{path}".to_string()),
                math_number_all: true,
                ..crate::rst::ReadConfig::from(&BuildConfig::default())
            }
        );
        assert_eq!(
            crate::env::math::MathConfig::from(&config),
            crate::env::math::MathConfig {
                numfig: false,
                numsep: "-".to_string(),
                eqref_format: Some("({number})".to_string()),
            }
        );
        for key in ["math_number_all", "math_numfig"] {
            let error = BuildConfig::default()
                .apply_override(key, "true")
                .unwrap_err()
                .to_string();
            assert_eq!(error, format!("'{key}' must be '0' or '1', got 'true'"));
        }
    }

    /// `check_primary_domain` (`config-inited`, 800, `config.py:854-858`):
    /// a `primary_domain` no domain is registered under warns, after the
    /// type checks; an unset or empty one does not (probed on Sphinx
    /// 9.1.0: `primary_domain = 'nope'` prints
    /// `WARNING: primary_domain 'nope' not found, ignored.`).
    #[test]
    fn an_unknown_primary_domain_warns_at_config_inited() {
        let warnings = |primary_domain: Option<&str>| {
            BuildConfig {
                primary_domain: primary_domain.map(str::to_string),
                ..BuildConfig::default()
            }
            .validate()
        };
        assert_eq!(
            warnings(Some("nope")),
            ["primary_domain 'nope' not found, ignored."]
        );
        assert_eq!(
            warnings(Some("None")),
            ["primary_domain 'None' not found, ignored."]
        );
        for domain in [
            "py",
            "std",
            "c",
            "cpp",
            "js",
            "rst",
            "math",
            "index",
            "citation",
            "changeset",
        ] {
            assert!(warnings(Some(domain)).is_empty(), "{domain}");
        }
        assert!(warnings(None).is_empty());
        assert!(warnings(Some("")).is_empty());

        let mut config = BuildConfig {
            primary_domain: Some("nope".to_string()),
            toc_object_entries_show_parents: "bogus".to_string(),
            ..BuildConfig::default()
        };
        config.note_confval_type_mismatch("maximum_signature_line_length", "str");
        let warnings = config.validate();
        assert_eq!(warnings.len(), 3);
        assert_eq!(warnings[2], "primary_domain 'nope' not found, ignored.");
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

    /// `suppress_warnings` (default `[]`) and `show_warning_types` (default
    /// `True`), `config.py:262-263`. `-D` splits a list on commas, as
    /// Sphinx's `convert_overrides` does, and takes `0`/`1` for a bool.
    #[test]
    fn warning_stream_keys_default_like_sphinx_and_take_overrides() {
        let mut config = BuildConfig::default();
        assert!(config.suppress_warnings.is_empty());
        assert!(config.show_warning_types);

        assert!(config
            .apply_override("suppress_warnings", "toc.not_included,ref.*")
            .unwrap()
            .is_none());
        assert_eq!(config.suppress_warnings, ["toc.not_included", "ref.*"]);
        assert!(config
            .apply_override("show_warning_types", "0")
            .unwrap()
            .is_none());
        assert!(!config.show_warning_types);
    }

    /// `keep_warnings` (`config.py:261`: default `False`, type `bool` only).
    /// Its `-D` value is Sphinx's bool grammar: `convert_overrides` takes
    /// `'0'` and `'1'` and raises `ConfigError` for anything else
    /// (`config.py:366-374`), `true` included, with the message Sphinx
    /// prints. The override reaches `conf_values` like any other.
    #[test]
    fn keep_warnings_takes_sphinxs_bool_grammar() {
        let mut config = BuildConfig::default();
        assert!(!config.keep_warnings);

        assert!(config
            .apply_override("keep_warnings", "1")
            .unwrap()
            .is_none());
        assert!(config.keep_warnings);
        assert_eq!(
            config.conf_values.get("keep_warnings"),
            Some(&serde_json::Value::Bool(true))
        );
        assert!(config
            .apply_override("keep_warnings", "0")
            .unwrap()
            .is_none());
        assert!(!config.keep_warnings);

        for value in ["true", "True", "false", "yes", ""] {
            let error = config
                .apply_override("keep_warnings", value)
                .expect_err(value);
            assert_eq!(
                error.to_string(),
                format!("'keep_warnings' must be '0' or '1', got '{value}'")
            );
            assert!(!config.keep_warnings, "{value}: the setting is unchanged");
        }
    }

    /// The smart-quotes keys (`config.py:289-295`) default like Sphinx's,
    /// and take `-D` as Sphinx does: `smartquotes` only as `0` or `1`, any
    /// other value (`False` included) a fatal `ConfigError` with Sphinx's
    /// message; `smartquotes_action` as a string; `smartquotes_excludes`
    /// one key at a time, never as a whole dict, each value the string as
    /// given, never split on commas. With no `conf.py` dict, the first
    /// dotted override starts an empty one, so the default's other key is
    /// gone; a later override adds to that dict, which `conf_values` holds,
    /// as Sphinx's `raw_config` does.
    //
    // oracle (Sphinx 9.1.0, `-b html` builds, wave 5a T16b fix round 1,
    //   scratch `t16bf/probe_excludes.out`): with `language = 'ja'` and no
    //   conf.py dict, `-D smartquotes_excludes.builders=man` educates
    //   (`「Hi」 – it’s`), `-D smartquotes_excludes.foo=x` too, with no
    //   warning, and `.languages=ja` with `.builders=man`, in either order,
    //   does not; `-D smartquotes_excludes=x` warns and is ignored.
    #[test]
    fn smartquotes_keys_take_sphinxs_override_grammar() {
        let mut config = BuildConfig::default();
        assert!(config.smartquotes);
        assert_eq!(config.smartquotes_action, "qDe");
        let names = |values: &[&str]| {
            ExcludedNames::Names(values.iter().map(|value| value.to_string()).collect())
        };
        let text = |value: &str| ExcludedNames::Text(value.to_string());
        assert_eq!(
            config.smartquotes_excludes,
            SmartquotesExcludes {
                languages: names(&["ja", "zh_CN", "zh_TW"]),
                builders: names(&["man", "text"]),
            }
        );

        assert!(config.apply_override("smartquotes", "0").unwrap().is_none());
        assert!(!config.smartquotes);
        assert_eq!(
            config.conf_values.get("smartquotes"),
            Some(&serde_json::Value::Bool(false))
        );
        assert!(config.apply_override("smartquotes", "1").unwrap().is_none());
        assert!(config.smartquotes);
        for value in ["False", "false", "true", "no", ""] {
            let error = config
                .apply_override("smartquotes", value)
                .expect_err(value);
            assert_eq!(
                error.to_string(),
                format!("'smartquotes' must be '0' or '1', got '{value}'")
            );
            assert!(config.smartquotes, "{value}: the setting is unchanged");
        }

        assert!(config
            .apply_override("smartquotes_action", "qBe")
            .unwrap()
            .is_none());
        assert_eq!(config.smartquotes_action, "qBe");

        // No conf.py dict: the override starts `{}`.
        assert!(config
            .apply_override("smartquotes_excludes.builders", "html,dummy")
            .unwrap()
            .is_none());
        assert_eq!(
            config.smartquotes_excludes,
            SmartquotesExcludes {
                languages: ExcludedNames::default(),
                builders: text("html,dummy"),
            }
        );
        assert_eq!(
            config.conf_values.get("smartquotes_excludes"),
            Some(&serde_json::json!({"builders": "html,dummy"}))
        );
        // The dict it made takes the next one.
        assert!(config
            .apply_override("smartquotes_excludes.languages", "en_GB")
            .unwrap()
            .is_none());
        assert_eq!(
            config.smartquotes_excludes,
            SmartquotesExcludes {
                languages: text("en_GB"),
                builders: text("html,dummy"),
            }
        );
        assert_eq!(
            config.conf_values.get("smartquotes_excludes"),
            Some(&serde_json::json!({"builders": "html,dummy", "languages": "en_GB"}))
        );
        let ignored = config
            .apply_override("smartquotes_excludes", "html")
            .unwrap();
        assert!(
            ignored.is_some_and(|message| message
                .starts_with("cannot override dictionary config setting 'smartquotes_excludes'")),
            "a whole dict is not overridden"
        );
        assert_eq!(config.smartquotes_excludes.builders, text("html,dummy"));

        // A key the transform never reads makes the dict all the same, and
        // is no unknown key.
        let mut config = BuildConfig::default();
        assert!(config
            .apply_override("smartquotes_excludes.foo", "x")
            .unwrap()
            .is_none());
        assert_eq!(config.smartquotes_excludes, SmartquotesExcludes::empty());
        assert_eq!(
            config.conf_values.get("smartquotes_excludes"),
            Some(&serde_json::json!({"foo": "x"}))
        );
    }

    /// `trim_doctest_flags` (`config.py:267`: default `True`, type `bool`
    /// only) takes the same bool grammar on `-D` as `keep_warnings`, and
    /// its override reaches `conf_values`.
    #[test]
    fn trim_doctest_flags_takes_sphinxs_bool_grammar() {
        let mut config = BuildConfig::default();
        assert!(config.trim_doctest_flags);

        assert!(config
            .apply_override("trim_doctest_flags", "0")
            .unwrap()
            .is_none());
        assert!(!config.trim_doctest_flags);
        assert_eq!(
            config.conf_values.get("trim_doctest_flags"),
            Some(&serde_json::Value::Bool(false))
        );
        assert!(config
            .apply_override("trim_doctest_flags", "1")
            .unwrap()
            .is_none());
        assert!(config.trim_doctest_flags);

        for value in ["false", "False", "true", "no", ""] {
            let error = config
                .apply_override("trim_doctest_flags", value)
                .expect_err(value);
            assert_eq!(
                error.to_string(),
                format!("'trim_doctest_flags' must be '0' or '1', got '{value}'")
            );
            assert!(
                config.trim_doctest_flags,
                "{value}: the setting is unchanged"
            );
        }
    }

    /// `today` (`config.py:227`, default `''`) and `today_fmt` (`:229`,
    /// default `None`) are strings, which a `-D` override sets verbatim
    /// (`convert_overrides`: a `str` or `None` default returns the value,
    /// `config.py:393-394`), a number-looking one included. The override
    /// reaches `conf_values` like any other.
    #[test]
    fn today_and_today_fmt_take_string_overrides() {
        let mut config = BuildConfig::default();
        assert_eq!(
            (config.today.as_str(), config.today_fmt.as_deref()),
            ("", None)
        );
        for (key, value) in [
            ("today", "April 1, 2026"),
            ("today", "2026"),
            ("today_fmt", "%B %-d, %Y"),
            ("today_fmt", "2026"),
        ] {
            assert!(
                config.apply_override(key, value).unwrap().is_none(),
                "{key}={value}"
            );
            let set = match key {
                "today" => Some(config.today.as_str()),
                _ => config.today_fmt.as_deref(),
            };
            assert_eq!(set, Some(value), "{key}={value}");
            assert_eq!(
                config.conf_values.get(key),
                Some(&serde_json::Value::String(value.to_string())),
                "{key}={value}"
            );
        }
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
