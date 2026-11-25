use anyhow::Result;
use log::{debug, info};
use rayon::prelude::*;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cache::BuildCache;
use crate::config::BuildConfig;
use crate::diagnostics::{Diagnostic, DiagnosticSink, WarningFilter};
use crate::doctree::Doctree;
use crate::document::Document;
use crate::env;
use crate::env::citation as env_citation;
use crate::env::genindex as env_genindex;
use crate::env::numbers as env_numbers;
use crate::env::py_domain as env_py_domain;
use crate::env::resolve as env_resolve;
use crate::env::toctree as env_toctree;
use crate::env::toctree::ConsistencyLevel;
use crate::env::BuildEnvironment;
use crate::error::{BuildErrorReport, BuildWarning, ErrorType, WarningType};
use crate::extensions::{ExtensionLoader, SphinxApp};
use crate::intersphinx::{self, HttpConfig, Intersphinx, LoadRequest, UreqFetcher};
use crate::inventory::InventoryFile;
use crate::matching;
use crate::parser::Parser;
use crate::read::{merge_document, MergeInputs, ReadOutcome, ReadSettings};
use crate::rst::SphinxCrash;
use crate::transforms::post;
use crate::transforms::tags::Tags;
use crate::utils;
use crate::utils::py_repr_str;

/// Subdirectory of the cache dir holding one bincode doctree per document.
/// It lives inside the `.config-fingerprint`-governed cache directory, so a
/// configuration change wipes these along with everything else.
const DOCTREE_SUBDIR: &str = "doctrees";

/// Magic prefix identifying a persisted doctree file ("sphinx-ultra
/// doctree"). Together with [`DOCTREE_FORMAT_VERSION`] it forms the 8-byte
/// header [`SphinxBuilder::store_doctree`] writes ahead of the bincode blob.
const DOCTREE_MAGIC: &[u8; 4] = b"SUDT";

/// Format version of the per-document doctree files.
///
/// The blob itself is bincode, which has no field-presence framing and no
/// self-description: bytes written by an older build decode *successfully*
/// into a plausible-looking doctree and are then silently mis-read. This
/// word is what turns that into an honest cache miss
/// ([`SphinxBuilder::load_doctree`] returns `None` for a missing or
/// mismatched version, and the document is re-read).
///
/// Bump it whenever previously written blobs would be misread, namely:
/// - the serialized shape changes — a field added to or removed from
///   `Doctree`/`Node`/`Attrs`, a different `AttrValue` variant set, or a
///   different bincode configuration;
/// - the *meaning* of what the parser stores changes while the shape does
///   not. Wave 4's index-entry attribute moving from `AttrValue::Str` to
///   `AttrValue::List` is the worked example: both variants decode, and an
///   old blob then harvests the wrong index entries.
///
/// Version 2: wave 4.5's provenance change — `Span` gained a `line` field
/// (and its byte range now indexes the parser's processed source text),
/// so version-1 blobs no longer decode as written.
///
/// Version 3: wave 5a's node side-car — `Node` gained `ext`
/// (`NodeExt`, persisting a node's `rawsource` and a `pending` node's
/// transform and details), so version-2 blobs no longer decode as written.
///
/// Version 4: wave 5a's read transforms — the persisted doctree is the
/// post-read tree, which `FilterSystemMessages` (999) has rid of the
/// reporter's `system_message`s (every one under the default
/// `keep_warnings = False`), and its consumers no longer skip them: a
/// version-3 blob still holds them, and the search index of a document
/// recovered from it would index their words.
///
/// Version 5: wave 5a's `DocInfo` (340) and the metadata collector's pop
/// — the persisted doctree no longer holds a document's leading field
/// list, and the search index no longer skips one: a version-4 blob still
/// holds it, and its words would be indexed.
const DOCTREE_FORMAT_VERSION: u32 = 5;

/// Bytes of the [`DOCTREE_MAGIC`] + [`DOCTREE_FORMAT_VERSION`] header.
const DOCTREE_HEADER_LEN: usize = DOCTREE_MAGIC.len() + std::mem::size_of::<u32>();

/// Sphinx's `root_doc` default (`config.py`), used when the configuration
/// leaves it unset.
const DEFAULT_ROOT_DOC: &str = "index";

/// The Sphinx builder a build's resolve pass stands for. In wave 5a it
/// only picks the tags `.. only::` is evaluated against ([`Self::tags`],
/// decision 12): every resolve pass gives the dummy builder's `''` URIs,
/// until 5b adds `DirHtml` and the URI policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BuilderKind {
    /// `-b html`, `StandaloneHTMLBuilder`: the CLI's builder.
    #[default]
    Html,
    /// `-b dummy`, `DummyBuilder`: the builder the environment oracle and
    /// the doctree oracles build with.
    Dummy,
}

impl BuilderKind {
    /// The builder's `name` (`Builder.name`): what
    /// `smartquotes_excludes['builders']` names it by
    /// (`SP/sphinx/transforms/__init__.py:392`).
    pub fn name(self) -> &'static str {
        match self {
            BuilderKind::Html => "html",
            BuilderKind::Dummy => "dummy",
        }
    }

    /// The tags the builder adds to the build's when it is made
    /// (`SP/sphinx/builders/__init__.py:120-124`): its `format`, its
    /// `name`, `format_<format>` and `builder_<name>`. The HTML builder's
    /// format and name are both `html`. The dummy builder's format is `''`
    /// (`SP/sphinx/builders/dummy.py:17-18`), so it adds the empty tag,
    /// which no condition can name, and `format_`.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            BuilderKind::Html => &["html", "format_html", "builder_html"],
            BuilderKind::Dummy => &["", "dummy", "format_", "builder_dummy"],
        }
    }
}

/// Decision 14's read schedule: whether the parallel parses of a build
/// stand, given the role registries each document started from and the
/// ones it left ([`crate::rst::ParseOutput::roles_after`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadSchedule {
    /// Every document started from the registries Sphinx's read would
    /// have given it: each parse saw what Sphinx's read would have.
    Parallel,
    /// The document at `from`, in sorted docname order, is the first that
    /// started from other registries: it and every document after it are
    /// parsed again, in order, each from the registries the one before it
    /// left (the first document of the build from the fresh ones).
    Reparse { from: usize },
}

impl ReadSchedule {
    /// The schedule for the documents a build parsed, in sorted docname
    /// order, given the registries each one started from and the ones it
    /// left. Sphinx reads them in that order in one process, so the first
    /// starts from the registries no document has changed (`fresh`), and
    /// every other from the ones the document before it left.
    fn plan(
        fresh: &crate::rst::roles::RoleTable,
        starts: &[&crate::rst::roles::RoleTable],
        roles_after: &[&crate::rst::roles::RoleTable],
    ) -> ReadSchedule {
        let expected = std::iter::once(fresh).chain(roles_after.iter().copied());
        match starts
            .iter()
            .zip(expected)
            .position(|(start, expected)| *start != expected)
        {
            Some(from) => ReadSchedule::Reparse { from },
            None => ReadSchedule::Parallel,
        }
    }
}

/// One document's read-phase output: the pipeline's [`Document`] and the
/// read it was derived from.
///
/// The parse's toctree, directive and role records and its registry
/// export ride the `Document`, not the read: a cache hit skips parsing, so
/// the only honest source for them is what was persisted with the cached
/// `Document` (see [`Document::registry`]). The read holds empty stand-ins
/// for them, and the merge lends the registry export and the toctree
/// records back to it while stage M runs ([`SphinxBuilder::merge_phase`]).
struct ReadResult {
    /// Root-relative docname (`docname_of_path`).
    docname: String,
    document: Document,
    /// The read ([`ReadOutcome`]): the document's doctree and, for a
    /// document this build parsed, the docutils document state beside it,
    /// which the merge's stage M reads. A document recovered from the cache
    /// has its persisted doctree and an empty state.
    outcome: ReadOutcome,
    /// Read completion time in microseconds since the epoch — what Sphinx
    /// stores in `env.all_docs[docname]` (`builders/__init__.py:665`).
    ///
    /// `None` for a document this build did *not* read: its rendered output
    /// and its doctree came back from the cache, and everything it
    /// contributed to the environment — its read time included — is
    /// whatever the build that did read it left behind.
    read_time_us: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct BuildStats {
    pub files_processed: usize,
    pub files_skipped: usize,
    pub build_time: Duration,
    pub output_size_mb: f64,
    pub cache_hits: usize,
    /// Genuine per-file failures (an unreadable or undecodable source, a
    /// page or doctree that could not be written) — never a record of the
    /// warning stream.
    pub errors: usize,
    /// Every record the build printed, which is what Sphinx's `_warncount`
    /// counts: a record `suppress_warnings` drops is neither printed nor
    /// counted (`WarningSuppressor`, `sphinx/util/logging.py:429-450`).
    pub warnings: usize,
    /// The records themselves, in the order Sphinx emits them (see
    /// [`SphinxBuilder::build`]).
    pub warning_details: Vec<Diagnostic>,
    pub error_details: Vec<BuildErrorReport>,
}

pub struct SphinxBuilder {
    config: BuildConfig,
    source_dir: PathBuf,
    output_dir: PathBuf,
    cache: BuildCache,
    parser: Parser,
    parallel_jobs: usize,
    incremental: bool,
    /// The builder the resolve pass stands for ([`BuilderKind`]): the CLI
    /// builds as `html`, the environment oracle's harness as `dummy`.
    builder_kind: BuilderKind,
    /// The warning stream so far: every flushed sink's records, minus the
    /// ones [`Self::filter`] suppresses.
    records: Mutex<Vec<Diagnostic>>,
    /// `suppress_warnings` and `show_warning_types`, applied where a sink
    /// is flushed ([`Self::flush`]).
    filter: WarningFilter,
    errors: Arc<Mutex<Vec<BuildErrorReport>>>,
    #[allow(dead_code)]
    sphinx_app: Option<SphinxApp>,
    #[allow(dead_code)]
    extension_loader: ExtensionLoader,
    /// Persisted build state (toctree graph, section/figure numbering, std
    /// domain data, ...). Loaded from the cache dir's `env.bin` if present
    /// and current; otherwise a fresh, empty environment.
    ///
    /// It steers the build: [`BuildEnvironment::get_outdated_files`] decides
    /// which documents this build reads, the merge phase folds those (and
    /// only those) back in, and the resolve phase saves it again. A document
    /// that is not read keeps every contribution the build that read it
    /// made.
    env: BuildEnvironment,
    /// docname -> the pseudo-XML of that document's *resolved* doctree, as
    /// the resolve phase left it (Sphinx's `get_and_resolve_doctree`
    /// output). Kept for [`Self::snapshot_env`], which is what the
    /// environment-oracle differential diffs; the write phase does not
    /// consume doctrees yet.
    resolved: Mutex<BTreeMap<String, String>>,
    /// Sphinx's `IndexBuilder`, fed one resolved document at a time by the
    /// resolve phase (where Sphinx's `write_doc_serialized` feeds it) and
    /// frozen into `searchindex.js` by [`Self::generate_search_index`].
    search_index: Mutex<crate::search::SphinxIndexBuilder>,
    /// The general index this build assembled (`IndexEntries.create_index`),
    /// kept beside [`Self::resolved`] for the same reason: it is build
    /// output derived from the environment plus the builder's own uri
    /// scheme, not environment state.
    genindex: Mutex<Vec<env_genindex::IndexGroup>>,
    /// The Python module index (`PythonModuleIndex.generate`), build output
    /// exactly like [`Self::genindex`] — sphinx assembles it while the HTML
    /// builder writes the `py-modindex` page.
    py_modindex: Mutex<env_py_domain::PyModindex>,
    /// The cross-project inventories `intersphinx_mapping` names, loaded
    /// once per build. Empty (and inert) unless a mapping is configured.
    intersphinx: Intersphinx,
    /// How many documents the last read phase parsed a second time, in
    /// sorted docname order, to follow decision 14's schedule: what the
    /// unit tests read to pin that the parallel parses stand.
    reparsed: std::sync::atomic::AtomicUsize,
}

/// [`BuildConfig`] fields deliberately kept *out* of the cache fingerprint.
///
/// Both are operational switches that steer how this run reports
/// diagnostics; neither changes the content the cache holds, and Sphinx
/// treats neither as an environment-invalidating change:
///
/// * `nitpicky` (`-n`) is a `Config` value whose rebuild class is `''`
///   (`sphinx/config.py:272`), and `_config_status` only reports
///   `CONFIG_CHANGED` for values whose rebuild class is `'env'`
///   (`sphinx/environment/__init__.py:366-369`) — so `-n` never invalidates
///   Sphinx's environment.
/// * `fail_on_warning` (`-W`, Sphinx's `warningiserror`) is not a `Config`
///   value at all: it is a `sphinx-build` argument and can never take part
///   in a config comparison.
///
/// Including them here would be worse than a spurious full rebuild. A
/// fingerprint mismatch wipes the whole cache directory (documents,
/// `doctrees/`, `env.bin`, `__intersphinx_cache__`), and read-phase
/// diagnostics are only emitted for documents a build actually reads — so
/// merely adding `-W` would force a cold read that re-emits every
/// read-phase warning and fails, while the *next* identical `-W` run would
/// be warm, emit nothing, and pass. Everything else stays in, `tags`
/// included: tags select `only::` branches and therefore change the
/// output.
const EXCLUDED_FROM_FINGERPRINT: [&str; 2] = ["fail_on_warning", "nitpicky"];

/// blake3 over the configuration minus [`EXCLUDED_FROM_FINGERPRINT`].
///
/// The value is serialized through `serde_json::Value` before hashing, which
/// also makes the digest order-independent: `Value::Object` is a `BTreeMap`,
/// so every map in the configuration is emitted in sorted-key order no
/// matter what order the source type iterates in.
fn config_fingerprint(config: &BuildConfig) -> Result<String> {
    let mut value = serde_json::to_value(config)?;
    if let Some(map) = value.as_object_mut() {
        for key in EXCLUDED_FROM_FINGERPRINT {
            map.remove(key);
        }
        // A prolog or an epilog Sphinx's reader crashes on changes every
        // read, though the typed fields read it as `None`
        // ([`BuildConfig::decoration_crashes`]).
        if !config.decoration_crashes.is_empty() {
            map.insert(
                "decoration_crashes".to_string(),
                serde_json::to_value(&config.decoration_crashes)?,
            );
        }
    }
    Ok(blake3::hash(serde_json::to_string(&value)?.as_bytes())
        .to_hex()
        .to_string())
}

/// Render the Sphinx 9.1 `.buildinfo` file.
pub(crate) fn sphinx_build_info_contents(config: &BuildConfig, tags: &[String]) -> Result<String> {
    let config_value = sphinx_html_config(config);
    let config_hash = stable_hash(&config_value);
    // `BuildInfo(app.config, app.tags, {'html'})`: the build's tags, the
    // HTML builder's own among them.
    let mut sphinx_tags = BuilderKind::Html
        .tags()
        .iter()
        .map(|tag| tag.to_string())
        .collect::<Vec<_>>();
    sphinx_tags.extend(tags.iter().cloned());
    sphinx_tags.sort();
    sphinx_tags.dedup();
    let tags_hash = stable_hash(&StableValue::List(
        sphinx_tags.into_iter().map(StableValue::String).collect(),
    ));
    let newline = if cfg!(windows) { "\r\n" } else { "\n" };

    Ok(format!(
        "# Sphinx build info version 1{newline}\
         # This file records the configuration used when building these files. When it is not found, a full rebuild will be done.{newline}\
         config: {config_hash}{newline}\
         tags: {tags_hash}{newline}"
    ))
}

/// A small value model for Sphinx's `stable_hash`. Its collection branches
/// deliberately hash child values first, sort those hashes, then hash the
/// Python-style list representation — this is the algorithm in
/// `sphinx.util._serialise`, not a JSON or Rust-debug serialization.
#[derive(Clone)]
enum StableValue {
    None,
    Bool(bool),
    String(String),
    List(Vec<StableValue>),
    Dict(BTreeMap<String, StableValue>),
    Tuple(Vec<StableValue>),
    Raw(String),
}

fn stable_hash(value: &StableValue) -> String {
    match value {
        // Sphinx first replaces a dict with the sorted hashes of its items,
        // then runs the list branch over that result once more. The second
        // pass is easy to miss because lists and tuples only have one pass.
        StableValue::Dict(values) => stable_collection(values.iter().map(|(key, value)| {
            StableValue::String(stable_hash(&StableValue::Tuple(vec![
                StableValue::String(key.clone()),
                value.clone(),
            ])))
        })),
        StableValue::List(values) | StableValue::Tuple(values) => {
            stable_collection(values.iter().cloned())
        }
        StableValue::None => md5_hex("None"),
        StableValue::Bool(value) => md5_hex(if *value { "True" } else { "False" }),
        StableValue::String(value) | StableValue::Raw(value) => md5_hex(value),
    }
}

fn stable_collection(values: impl Iterator<Item = StableValue>) -> String {
    let mut hashes: Vec<String> = values.map(|value| stable_hash(&value)).collect();
    hashes.sort();
    let repr = hashes
        .iter()
        .map(|hash| format!("'{hash}'"))
        .collect::<Vec<_>>()
        .join(", ");
    md5_hex(&format!("[{repr}]"))
}

/// A `conf.py` literal as `stable_hash` sees it, which hashes `str()` of a
/// number ([`crate::pyformat::number_str`]).
fn stable_hash_json(value: &serde_json::Value) -> StableValue {
    match value {
        serde_json::Value::Null => StableValue::None,
        serde_json::Value::Bool(value) => StableValue::Bool(*value),
        serde_json::Value::Number(value) => StableValue::Raw(crate::pyformat::number_str(value)),
        serde_json::Value::String(value) => StableValue::String(value.clone()),
        serde_json::Value::Array(values) => {
            StableValue::List(values.iter().map(stable_hash_json).collect())
        }
        serde_json::Value::Object(values) => StableValue::Dict(
            values
                .iter()
                .map(|(key, value)| (key.clone(), stable_hash_json(value)))
                .collect(),
        ),
    }
}

fn stable_strings(values: impl IntoIterator<Item = String>) -> StableValue {
    StableValue::List(values.into_iter().map(StableValue::String).collect())
}

fn stable_dict(entries: impl IntoIterator<Item = (String, StableValue)>) -> StableValue {
    StableValue::Dict(entries.into_iter().collect())
}

/// `BuildInfo.__init__`: `{c.name: c.value for c in config.filter({'html'})}`
/// — every configuration value of the `html` rebuild category, as Sphinx
/// holds it after `config-inited`.
///
/// A key `conf.py` set is hashed as the literal it was set to
/// ([`BuildConfig::conf_values`]); every other key is hashed at Sphinx's
/// default, including the defaults Sphinx computes from other values
/// (`html_title`, `html_short_title`, `singlehtml_sidebars`,
/// `qthelp_basename`). `html_context` is the one key read from the typed
/// field, because `-A` adds to it the way `-D html_context.name=value` does.
///
/// `html_css_files`/`html_js_files` are hashed as the `(filename, attrs)`
/// tuples `convert_html_css_files` rewrites them into at `config-inited`.
/// The builder's `init_css_files` later adds a `priority` to those `attrs`,
/// but `BuildInfo` has been computed by then, so the hash never sees it.
fn sphinx_html_config(config: &BuildConfig) -> StableValue {
    let conf = |key: &str| config.conf_values.get(key).map(stable_hash_json);
    let none = StableValue::None;
    let string = |value: &str| StableValue::String(value.to_string());
    let empty_list = || StableValue::List(Vec::new());
    let empty_dict = || stable_dict([]);

    // `copyright` and `project_copyright` are two names for one value
    // (`Config.__setattr__`); `copyright`'s default is `project_copyright`.
    let copyright = conf("copyright")
        .or_else(|| conf("project_copyright"))
        .unwrap_or_else(|| string(""));
    let html_title = conf("html_title").unwrap_or_else(|| {
        StableValue::String(format!(
            "{} {} documentation",
            config.project,
            config.release.as_deref().unwrap_or("")
        ))
    });
    let html_short_title = conf("html_short_title").unwrap_or_else(|| html_title.clone());
    let html_sidebars = conf("html_sidebars").unwrap_or_else(empty_dict);
    let singlehtml_sidebars = conf("singlehtml_sidebars").unwrap_or_else(|| html_sidebars.clone());
    let qthelp_basename = conf("qthelp_basename")
        .unwrap_or_else(|| StableValue::String(make_filename(&config.project)));
    let html_context = StableValue::Dict(
        config
            .html_context
            .iter()
            .map(|(key, value)| (key.clone(), stable_hash_json(value)))
            .collect(),
    );

    let mut values = BTreeMap::new();
    let defaults = [
        ("pygments_style", none.clone()),
        ("templates_path", empty_list()),
        ("template_bridge", none.clone()),
        ("modindex_common_prefix", empty_list()),
        ("html_theme", string("alabaster")),
        ("html_theme_path", empty_list()),
        ("html_theme_options", empty_dict()),
        ("html_style", none.clone()),
        ("html_logo", none.clone()),
        ("html_favicon", none.clone()),
        ("html_static_path", empty_list()),
        ("html_extra_path", empty_list()),
        ("html_last_updated_fmt", none.clone()),
        ("html_last_updated_use_utc", StableValue::Bool(false)),
        ("html_additional_pages", empty_dict()),
        ("html_domain_indices", StableValue::Bool(true)),
        ("html_permalinks", StableValue::Bool(true)),
        ("html_permalinks_icon", string("¶")),
        ("html_use_index", StableValue::Bool(true)),
        ("html_split_index", StableValue::Bool(false)),
        ("html_copy_source", StableValue::Bool(true)),
        ("html_show_sourcelink", StableValue::Bool(true)),
        ("html_sourcelink_suffix", string(".txt")),
        ("html_use_opensearch", string("")),
        ("html_file_suffix", none.clone()),
        ("html_link_suffix", none.clone()),
        ("html_show_copyright", StableValue::Bool(true)),
        ("html_show_search_summary", StableValue::Bool(true)),
        ("html_show_sphinx", StableValue::Bool(true)),
        ("html_output_encoding", string("utf-8")),
        ("html_compact_lists", StableValue::Bool(true)),
        ("html_secnumber_suffix", string(". ")),
        ("html_search_language", none.clone()),
        ("html_search_options", empty_dict()),
        ("html_scaled_image_link", StableValue::Bool(true)),
        ("html_baseurl", string("")),
        ("html_codeblock_linenos_style", string("inline")),
        ("html4_writer", StableValue::Bool(false)),
        (
            "mathjax_path",
            string("https://cdn.jsdelivr.net/npm/mathjax@4/tex-mml-chtml.js"),
        ),
        ("mathjax_options", empty_dict()),
        (
            "mathjax_inline",
            stable_strings(["\\(".to_string(), "\\)".to_string()]),
        ),
        (
            "mathjax_display",
            stable_strings(["\\[".to_string(), "\\]".to_string()]),
        ),
        ("mathjax_config", none.clone()),
        ("mathjax2_config", none.clone()),
        ("mathjax3_config", none.clone()),
        ("mathjax4_config", none.clone()),
        ("mathjax_config_path", string("")),
        ("htmlhelp_file_suffix", none.clone()),
        ("htmlhelp_link_suffix", none.clone()),
        ("qthelp_namespace", none.clone()),
        ("qthelp_theme", string("nonav")),
        ("qthelp_theme_options", empty_dict()),
    ];
    for (key, default) in defaults {
        values.insert(key.to_string(), conf(key).unwrap_or(default));
    }
    let computed = [
        ("project_copyright", copyright.clone()),
        ("copyright", copyright),
        ("html_title", html_title),
        ("html_short_title", html_short_title),
        ("html_sidebars", html_sidebars),
        ("singlehtml_sidebars", singlehtml_sidebars),
        ("qthelp_basename", qthelp_basename),
        ("html_context", html_context),
        ("html_css_files", html_asset_files(config, "html_css_files")),
        ("html_js_files", html_asset_files(config, "html_js_files")),
    ];
    for (key, value) in computed {
        values.insert(key.to_string(), value);
    }
    StableValue::Dict(values)
}

/// `convert_html_css_files`/`convert_html_js_files` (`builders/html`,
/// `config-inited`): a bare filename becomes `(filename, {})`, a
/// `(filename, attrs)` pair stays, and anything else is dropped with a
/// warning.
fn html_asset_files(config: &BuildConfig, key: &str) -> StableValue {
    let Some(serde_json::Value::Array(entries)) = config.conf_values.get(key) else {
        return StableValue::List(Vec::new());
    };
    StableValue::List(
        entries
            .iter()
            .filter_map(|entry| match entry {
                serde_json::Value::String(filename) => Some(StableValue::Tuple(vec![
                    StableValue::String(filename.clone()),
                    stable_dict([]),
                ])),
                serde_json::Value::Array(pair) if pair.len() == 2 => {
                    Some(StableValue::Tuple(vec![
                        stable_hash_json(&pair[0]),
                        stable_hash_json(&pair[1]),
                    ]))
                }
                _ => None,
            })
            .collect(),
    )
}

/// `sphinx.util.osutil.make_filename`: everything outside `[a-zA-Z0-9_-]`
/// stripped, `sphinx` when nothing is left. `qthelp_basename`'s default.
fn make_filename(name: &str) -> String {
    let filename: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    if filename.is_empty() {
        "sphinx".to_string()
    } else {
        filename
    }
}

fn md5_hex(input: &str) -> String {
    use md5::{Digest, Md5};
    format!("{:x}", Md5::digest(input.as_bytes()))
}

impl SphinxBuilder {
    pub fn new(mut config: BuildConfig, source_dir: PathBuf, output_dir: PathBuf) -> Result<Self> {
        // The intersphinx extension's `config-inited` step, for a caller
        // that has not run it; the CLI has, after printing the records
        // Sphinx logs for a rejected mapping, and running it again changes
        // nothing.
        config.init_intersphinx_mapping()?;
        // A `conf.py` prolog or epilog this crate could not evaluate, which
        // no override replaced, for a caller that has not checked: the CLI
        // has, once its `-D` overrides were in. The same for a
        // `source_suffix` this crate cannot read.
        config.check_evaluated()?;
        config.check_source_suffix()?;
        // -d/doctree_dir relocates the cache (sphinx-build's doctree dir).
        let cache_dir = config
            .doctree_dir
            .clone()
            .unwrap_or_else(|| output_dir.join(".sphinx-ultra-cache"));
        // A change to any *content-bearing* configuration value invalidates
        // cached documents (they were rendered under the old configuration);
        // the two purely operational flags in `BuildConfig` are excluded —
        // see [`config_fingerprint`].
        let config_fingerprint = config_fingerprint(&config)?;
        let cache = BuildCache::new(
            cache_dir,
            config.max_cache_size_mb,
            config.cache_expiration_hours,
            &config_fingerprint,
        )?;

        // Reuse whatever environment survived the fingerprint-wipe check
        // above (BuildCache::new already discarded it if the config
        // changed); a first build or an incompatible/corrupt env.bin both
        // fall back to a fresh, empty environment.
        let env = BuildEnvironment::load(cache.cache_dir()).unwrap_or_default();

        // Canonicalize source_dir so it matches the canonicalized absolute paths
        // returned by matching::get_matching_files; without this, relative
        // --source paths (including the default ".") fail strip_prefix later.
        let source_dir = crate::utils::canonicalize_simplified(&source_dir).unwrap_or(source_dir);

        let parser = Parser::new(&config)?.with_srcdir(source_dir.clone());

        let parallel_jobs = config.parallel_jobs.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });

        let filter = WarningFilter::from_config(&config);

        // Initialize Sphinx app with extensions
        let mut sphinx_app = SphinxApp::new(config.clone())?;
        let mut extension_loader = ExtensionLoader::new()?;

        // Load configured extensions
        for extension_name in &config.extensions {
            match extension_loader.load_extension(extension_name) {
                Ok(extension) => {
                    if let Err(e) = sphinx_app.add_extension(extension) {
                        log::warn!("Failed to add extension '{}': {}", extension_name, e);
                    }
                }
                Err(e) => {
                    log::warn!("Failed to load extension '{}': {}", extension_name, e);
                }
            }
        }

        Ok(Self {
            config,
            source_dir,
            output_dir,
            cache,
            parser,
            parallel_jobs,
            incremental: false,
            builder_kind: BuilderKind::default(),
            records: Mutex::new(Vec::new()),
            filter,
            errors: Arc::new(Mutex::new(Vec::new())),
            sphinx_app: Some(sphinx_app),
            extension_loader,
            env,
            resolved: Mutex::new(BTreeMap::new()),
            search_index: Mutex::new(crate::search::SphinxIndexBuilder::new()),
            genindex: Mutex::new(Vec::new()),
            py_modindex: Mutex::new(env_py_domain::PyModindex::default()),
            intersphinx: Intersphinx::default(),
            reparsed: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// How many documents the last build's read phase parsed a second
    /// time, in sorted docname order, because one read before them left
    /// role registries other than the ones they started from (decision
    /// 14): 0 while the parallel parses stand. The crate's own tests read
    /// it; it is no API.
    #[cfg(test)]
    pub(crate) fn reparsed_documents(&self) -> usize {
        self.reparsed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Read every inventory `intersphinx_mapping` names — Sphinx's
    /// `load_mappings`, which it runs at `builder-inited`, before the read
    /// phase (`ext/intersphinx/__init__.py:80`).
    ///
    /// Local inventory locations are resolved against the source directory
    /// and re-read on every build; remote ones go through [`UreqFetcher`]
    /// and are cached under the (fingerprint-wiped) cache directory, so a
    /// configuration change discards them along with everything else.
    /// Fails where Sphinx raises `ConfigError` from `load_mappings` — an
    /// entry that survived normalisation but violates
    /// `_IntersphinxProject`'s invariants — with a
    /// [`ConfigError`](crate::error::ConfigError), which aborts the build
    /// with the report and exit code an invalid mapping gets at config time.
    fn load_intersphinx_inventories(&mut self) -> Result<()> {
        // Without `sphinx.ext.intersphinx` Sphinx loads no inventory,
        // whatever `intersphinx_mapping` says ([TW F12]).
        if !self.config.intersphinx_enabled() || self.config.intersphinx_mapping.is_empty() {
            return Ok(());
        }
        let http = HttpConfig {
            tls_verify: self.config.tls_verify,
            tls_cacerts: self.config.tls_cacerts.clone(),
            user_agent: self.config.user_agent.clone(),
            timeout: self.config.intersphinx_timeout,
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs() as i64)
            .unwrap_or(0);
        let outcome = intersphinx::load_mappings(
            &LoadRequest {
                mapping: &self.config.intersphinx_mapping,
                srcdir: &self.source_dir,
                cache_dir: Some(self.cache.cache_dir().join(intersphinx::CACHE_DIR_NAME)),
                cache_limit: self.config.intersphinx_cache_limit,
                now,
                http: &http,
            },
            &UreqFetcher,
        )?;
        for message in outcome.infos {
            info!("{message}");
        }
        // The stream's first records: Sphinx loads the inventories at
        // `builder-inited`, before `build()` discovers a single document.
        let mut sink = DiagnosticSink::default();
        for message in outcome.warnings {
            // Sphinx logs these without a location and without a type, so
            // they render as a bare `WARNING: ...` — but they still count
            // toward the build's warning total and toward `-W`.
            sink.push_next(BuildWarning::new(
                PathBuf::new(),
                None,
                message,
                WarningType::Other,
            ));
        }
        self.flush(sink);
        self.intersphinx = Intersphinx {
            data: outcome.data,
            disabled_reftypes: self
                .config
                .intersphinx_disabled_reftypes
                .iter()
                .cloned()
                .collect(),
            resolve_self: self.config.intersphinx_resolve_self.clone(),
        };
        Ok(())
    }

    pub fn set_parallel_jobs(&mut self, jobs: usize) {
        self.parallel_jobs = jobs;
    }

    pub fn enable_incremental(&mut self) {
        self.incremental = true;
    }

    /// Read and resolve as `kind` builds ([`BuilderKind`]); `html` unless
    /// set. The read takes the builder from here too: it decides whether
    /// `smartquotes_excludes` turns smart quotes off.
    pub fn set_builder_kind(&mut self, kind: BuilderKind) {
        self.builder_kind = kind;
        self.parser.set_builder_kind(&self.config, kind);
    }

    /// Discard the saved environment before building (sphinx-build `-E`).
    ///
    /// Sphinx's `-E` is `freshenv=True`: the pickled environment is not
    /// loaded at all, and the fresh one it builds instead reports every
    /// document as new. Emptying the cache directory is the same statement
    /// about the *other* half of the persisted state (documents and
    /// doctrees), and dropping the already-loaded environment here is what
    /// keeps the two halves saying the same thing.
    pub fn fresh_env(&mut self) -> Result<()> {
        self.cache.clear()?;
        self.env = BuildEnvironment::default();
        Ok(())
    }

    /// Append one warning to the stream, after every record flushed so
    /// far — unless `suppress_warnings` drops it.
    #[allow(dead_code)]
    pub fn add_warning(&self, warning: BuildWarning) {
        let mut sink = DiagnosticSink::default();
        sink.push_next(warning);
        self.flush(sink);
    }

    /// Append a sink's records to the stream, in their
    /// [`OrderKey`](crate::diagnostics::OrderKey) order, dropping the ones
    /// `suppress_warnings` names: a suppressed record is neither printed nor
    /// counted.
    fn flush(&self, sink: DiagnosticSink) {
        let mut records = self.records.lock().unwrap();
        records.extend(
            sink.into_ordered()
                .into_iter()
                .filter(|record| !self.filter.is_suppressed(&record.category)),
        );
    }

    /// The records the build has put in the stream so far, in the order
    /// Sphinx emits them: after a build that crashed Sphinx (decision 13),
    /// the ones Sphinx prints before the crash's report.
    pub fn recorded_warnings(&self) -> Vec<Diagnostic> {
        self.records.lock().unwrap().clone()
    }

    /// Add an error to the collection
    #[allow(dead_code)]
    pub fn add_error(&self, error: BuildErrorReport) {
        self.errors.lock().unwrap().push(error);
    }

    /// Check if warnings should be treated as errors
    #[allow(dead_code)]
    pub fn should_fail_on_warning(&self) -> bool {
        self.config.fail_on_warning
    }

    pub async fn clean(&mut self) -> Result<()> {
        if self.output_dir.exists() {
            tokio::fs::remove_dir_all(&self.output_dir).await?;
        }
        // A clean build must not reuse documents cached before the clean
        // (the on-disk cache lived inside the output dir we just removed),
        // nor the environment that was loaded from it.
        self.cache.clear()?;
        self.env = BuildEnvironment::default();
        Ok(())
    }

    /// Run the build: read → merge → resolve → write, then validation.
    ///
    /// The four phases mirror Sphinx's own split (`builders/__init__.py`):
    ///
    /// - **read** ([`Self::read_phase`], parallel): parse every *outdated*
    ///   source file into a `Document` + doctree, and recover the rest from
    ///   the cache.
    /// - **merge** ([`Self::merge_phase`], sequential, docname-ordered):
    ///   fold each re-read document's output into the [`BuildEnvironment`] —
    ///   Sphinx's `merge_info_from` plus the collectors it dispatches — and
    ///   persist that document's doctree once those have mutated it.
    /// - **resolve** ([`Self::resolve_phase`]): whole-project state that
    ///   needs every document read first, then persist the environment.
    /// - **write** ([`Self::write_phase`]): emit the output files.
    ///
    /// `&mut self` only so the environment can be moved out and back; the
    /// phase methods themselves take `&self` and the environment by
    /// reference.
    ///
    /// Each phase collects its warnings in a [`DiagnosticSink`] — one per
    /// document for the read and resolve phases — and the build flushes
    /// the sinks in the order Sphinx emits their records (`Builder.build`,
    /// `builders/__init__.py:389-466`; `read`, `:469-590`; `write`,
    /// `:705-778`): intersphinx's inventory loading at builder init,
    /// discovery, the read sinks in sorted docname order, numbering
    /// (`env-get-updated`, which `check_dependents` runs before
    /// `check_consistency`), consistency, the resolve sinks in sorted
    /// docname order, then the finishing steps (the general index). The
    /// native subcommand's validation pass comes last.
    pub async fn build(&mut self) -> Result<BuildStats> {
        let start_time = Instant::now();
        info!("Starting build process...");

        // Ensure output directory exists
        tokio::fs::create_dir_all(&self.output_dir).await?;

        self.load_intersphinx_inventories()?;

        // Discover all source files
        let mut discovery = DiagnosticSink::default();
        let source_files = self.discover_source_files(&mut discovery).await?;
        self.flush(discovery);
        info!("Discovered {} source files", source_files.len());

        let mut env = std::mem::take(&mut self.env);
        let to_read = self.plan_read(&env, &source_files);
        let mut read_results = match self.read_phase(&source_files, &to_read) {
            Ok(read_results) => read_results,
            Err(error) => {
                self.env = env;
                return Err(error);
            }
        };

        // An input that crashes Sphinx (decision 13), in a document's read
        // or as it is resolved, ends the build with that crash
        // ([`SphinxCrash`]), once the records Sphinx prints before it are
        // in the stream ([`Self::recorded_warnings`]). The environment
        // keeps what the documents merged before it left, and is not saved.
        let crashed = self
            .merge_phase(&mut env, &mut read_results)
            .and_then(|()| self.resolve_phase(&mut env, &read_results));
        self.env = env;
        crashed?;

        let files_skipped = read_results
            .iter()
            .filter(|result| result.read_time_us.is_none())
            .count();

        // Keep documents in discovery order (the merge phase iterates a
        // docname-sorted view of its own): the write and validation phases
        // below produce warnings in this order, which is user-visible. The
        // doctrees ride along for their source tables, which the
        // validation pass needs to spell an included file's path.
        let (processed_docs, doctrees): (Vec<Document>, Vec<Doctree>) = read_results
            .into_iter()
            .map(|result| (result.document, result.outcome.output.doctree))
            .unzip();

        self.write_phase(&processed_docs);

        // M1's directive/role validation, which Sphinx has no counterpart
        // for: sphinx-build mode turns it off (the CLI forces
        // `validate_directives` false there), so its records only ever
        // reach the native subcommand's stream.
        if self.config.validate_directives {
            self.validate_directives_and_roles(&processed_docs, &doctrees);
        }

        // Generate cross-references and indices
        self.generate_indices(&processed_docs).await?;

        // Copy static assets
        self.copy_static_assets().await?;

        // Generate sitemap and search index
        self.generate_search_index().await?;
        tokio::fs::write(
            self.output_dir.join(".buildinfo"),
            sphinx_build_info_contents(&self.config, &self.config.tags)?,
        )
        .await?;

        let build_time = start_time.elapsed();
        let output_size = utils::calculate_directory_size(&self.output_dir).await?;

        let records = self.records.lock().unwrap();
        let errors = self.errors.lock().unwrap();

        let stats = BuildStats {
            files_processed: processed_docs.len(),
            files_skipped,
            build_time,
            output_size_mb: output_size as f64 / 1024.0 / 1024.0,
            cache_hits: self.cache.hit_count(),
            errors: errors.len(),
            warnings: records.len(),
            warning_details: records.clone(),
            error_details: errors.clone(),
        };

        info!("Build completed in {:?}", build_time);
        Ok(stats)
    }

    async fn discover_source_files(&self, sink: &mut DiagnosticSink) -> Result<Vec<PathBuf>> {
        // Use pattern-based file discovery like Sphinx
        let include_patterns = &self.config.include_patterns;
        let exclude_patterns = &self.config.exclude_patterns;

        // Add built-in exclude patterns for common build artifacts and hidden files
        let mut all_exclude_patterns = exclude_patterns.clone();
        all_exclude_patterns.extend_from_slice(&[
            "_build/**".to_string(),
            "__pycache__/**".to_string(),
            ".git/**".to_string(),
            ".svn/**".to_string(),
            ".hg/**".to_string(),
            ".*/**".to_string(), // Skip all hidden directories
            "Thumbs.db".to_string(),
            ".DS_Store".to_string(),
        ]);

        match matching::get_matching_files(
            &self.source_dir,
            include_patterns,
            &all_exclude_patterns,
        ) {
            // Sphinx's Project.discover keeps only files with a configured
            // source suffix, regardless of include_patterns
            Ok(files) => Ok(self.dedup_by_docname(
                files
                    .into_iter()
                    .filter(|path| self.is_source_file(path))
                    .collect(),
                sink,
            )),
            Err(e) => {
                log::warn!(
                    "Pattern matching failed, falling back to simple discovery: {}",
                    e
                );
                // Fallback to old method if pattern matching fails
                let mut files = Vec::new();
                self.discover_files_sync(&self.source_dir, &mut files)?;
                Ok(self.dedup_by_docname(files, sink))
            }
        }
    }

    /// Sphinx's `Project.discover` collision handling
    /// (`SP/sphinx/project.py:64-79`): two files that map to one docname
    /// are not two documents. Sphinx keeps the first its directory walk
    /// yields, which takes a directory's files in sorted order
    /// (`get_matching_files`, `SP/sphinx/util/matching.py:115-160`), so of
    /// `page.rst` and `page.txt` it keeps `page.rst`, whatever order
    /// `source_suffix` names their suffixes in; and it warns `multiple files
    /// found for the document "%s"` once and names the file it kept. Two
    /// files can only collide when `source_suffix` names more than one
    /// suffix: the docname is the file's path less its suffix.
    ///
    /// Everything downstream of discovery is keyed by docname alone — the
    /// persisted doctree, `env.all_docs`, `plan_read`'s source map, the
    /// output path — so admitting both files meant the rendered page
    /// alternated between them across builds, the loser's edits never
    /// invalidated anything, and two threads wrote one output path at once.
    ///
    /// Two deliberate deviations from Sphinx's message, both because
    /// Sphinx's own rendering is an artifact rather than a contract: the
    /// listed files are the colliding *documents* in sorted order (Sphinx
    /// globs `docname.*`, so its list picks up non-source neighbours and
    /// comes out in filesystem order), and the kept path is rendered as a
    /// plain Python string repr (Sphinx's `%r` hits `_StrPath.__repr__` and
    /// prints `_StrPath('/path')`).
    fn dedup_by_docname(&self, files: Vec<PathBuf>, sink: &mut DiagnosticSink) -> Vec<PathBuf> {
        let mut by_docname: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for path in &files {
            by_docname
                .entry(self.docname_of_path(path))
                .or_default()
                .push(path.clone());
        }

        let mut dropped: BTreeSet<PathBuf> = BTreeSet::new();
        for (docname, mut candidates) in by_docname {
            if candidates.len() < 2 {
                continue;
            }
            candidates.sort();
            let listed: Vec<String> = candidates
                .iter()
                .map(|path| {
                    path.strip_prefix(&self.source_dir)
                        .unwrap_or(path)
                        .display()
                        .to_string()
                })
                .collect();
            sink.push_next(BuildWarning::new(
                // Logged with no `location`, so it prints bare.
                PathBuf::new(),
                None,
                format!(
                    "multiple files found for the document \"{docname}\": {}\nUse {} for the build.",
                    listed.join(", "),
                    py_repr_str(&candidates[0].display().to_string()),
                ),
                WarningType::Other,
            ));
            dropped.extend(candidates.into_iter().skip(1));
        }

        if dropped.is_empty() {
            return files;
        }
        // Discovery order is otherwise preserved: only the losers go.
        files
            .into_iter()
            .filter(|path| !dropped.contains(path))
            .collect()
    }

    /// Fallback file discovery for when pattern matching fails
    fn discover_files_sync(&self, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Skip hidden directories and build artifacts
                if let Some(name) = path.file_name() {
                    if name.to_string_lossy().starts_with('.')
                        || name == "_build"
                        || name == "__pycache__"
                    {
                        continue;
                    }
                }

                self.discover_files_sync(&path, files)?;
            } else if self.is_source_file(&path) {
                files.push(path);
            }
        }
        Ok(())
    }

    /// Whether discovery treats `path` as a document: whether its name ends
    /// with a `source_suffix` ([`Self::source_docname`]).
    fn is_source_file(&self, path: &Path) -> bool {
        self.source_docname(path).is_some()
    }

    /// `Project.path2doc` (`SP/sphinx/project.py:94-112`): the docname of
    /// the file at `path`, under the source directory, when its name ends
    /// with a `source_suffix` ([`crate::config::SourceSuffix::docname`]).
    fn source_docname(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(&self.source_dir).unwrap_or(path);
        let relative = relative.to_string_lossy().replace('\\', "/");
        self.config.source_suffix.docname(&relative)
    }

    /// Which documents this build has to read
    /// ([`BuildEnvironment::get_outdated_files`]), and the `updating
    /// environment:` line Sphinx prints about it.
    ///
    /// A non-incremental build reads everything, `sphinx-build -a` included
    /// — that flag turns the document cache off here (see the mapping in
    /// `main.rs`), where sphinx would still have read incrementally. Reading
    /// more than sphinx does is slower, never wrong.
    fn plan_read(&self, env: &BuildEnvironment, files: &[PathBuf]) -> BTreeSet<String> {
        // `env.doc2path` for the documents this build discovered, and
        // `env.found_docs` as its key set.
        let sources: BTreeMap<String, PathBuf> = files
            .iter()
            .map(|path| (self.docname_of_path(path), path.clone()))
            .collect();
        let found: BTreeSet<String> = sources.keys().cloned().collect();

        if !self.incremental {
            debug!(
                "Not an incremental build: reading all {} files",
                found.len()
            );
            return found;
        }

        let outdated = env.get_outdated_files(
            &found,
            // A content-bearing configuration change wipes the cache
            // directory whole (`.config-fingerprint`), so it reaches this
            // point as an empty environment as well; saying it out loud
            // keeps the two statements from drifting apart. Operational
            // flags (`-W`, `-n`) are excluded from the fingerprint
            // (`EXCLUDED_FROM_FINGERPRINT`) and therefore never land here.
            self.cache.config_changed(),
            &env::FileTimes {
                source_modified_us: &|docname| {
                    sources.get(docname).and_then(|path| modified_us(path))
                },
                doctree_exists: &|docname| self.doctree_path(docname).is_file(),
                dependency_modified_us: &modified_us,
            },
        );

        // Sphinx's `updating environment: %s added, %s changed, %s removed`
        // (`builders/__init__.py:493-497`), minus the `[reason]` prefix: the
        // whole-configuration fingerprint this crate uses cannot tell "new
        // config" from "config changed".
        info!(
            "updating environment: {} added, {} changed, {} removed",
            outdated.added.len(),
            outdated.changed.len(),
            outdated.removed.len()
        );

        let mut to_read = outdated.to_read();

        // Deliberate divergence: the toctrees that pointed at a deleted
        // document are read again.
        //
        // Sphinx does not do this — it resolves toctree entries a second
        // time while *writing* each page (`adapters/toctree.py`), which is
        // where its "toctree contains reference to nonexisting document"
        // warning comes from on a rebuild. This crate resolves entries once,
        // in the parser, so leaving the container unread would make the
        // deletion silent until the next cold build. Re-reading it is how a
        // read-time resolver keeps an incremental build's diagnostics equal
        // to a cold one's; it costs one re-parse per container, and only
        // when a document actually disappears.
        for removed in &outdated.removed {
            for container in env.files_to_rebuild.get(removed).into_iter().flatten() {
                if found.contains(container) {
                    to_read.insert(container.clone());
                }
            }
        }

        to_read
    }

    /// Read phase: recover every up-to-date document from the cache and
    /// parse every outdated one, in parallel, then follow decision 14's
    /// schedule ([`ReadSchedule`]): the role registries are build-scoped,
    /// as docutils' `_roles` is for Sphinx, whose read is in sorted
    /// docname order. The first document to read starts from the fresh
    /// registries ([`Parser::fresh_roles`]), every other from the build's
    /// baseline ([`Parser::baseline_roles`]), which adds `rst_prolog`'s
    /// own definitions, the ones every document made before its own text;
    /// once a document leaves others (a `.. role::`, a `.. default-role::`
    /// without `default_role` set, a definition in `rst_epilog`), every
    /// document parsed after it in sorted docname order is parsed again,
    /// in that order, each from the registries the one before it left.
    /// Like Sphinx's, only the documents read in this build contribute:
    /// one recovered from the cache adds nothing. An incremental build then
    /// stores each parsed document in the cache once, as its last parse
    /// left it.
    ///
    /// One file failing must not abort the build: failures become
    /// `BuildErrorReport`s (and a non-zero exit) while the rest continue.
    /// A read that crashes Sphinx keeps its crash
    /// ([`crate::transforms::state::DocumentState::crash`]), which
    /// [`Self::merge_phase`] fails the build with, in Sphinx's read order.
    /// Results keep the discovery order of `files`.
    fn read_phase(&self, files: &[PathBuf], to_read: &BTreeSet<String>) -> Result<Vec<ReadResult>> {
        info!(
            "Processing {} files with {} parallel jobs",
            files.len(),
            self.parallel_jobs
        );

        // Sphinx's `env.found_docs`: known before any file is parsed, and
        // needed *during* the parse so `toctree` entries resolve.
        let found_docs = Arc::new(
            files
                .iter()
                .map(|path| self.docname_of_path(path))
                .collect::<BTreeSet<String>>(),
        );

        // Configure rayon thread pool
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(self.parallel_jobs)
            .build()?;

        let fresh = Arc::clone(self.parser.fresh_roles());
        let baseline = Arc::clone(self.parser.baseline_roles());
        // Sphinx reads the first document of its read before any other has
        // registered a role, so only that one starts from the fresh
        // registries: its leading field-marker lines, which come before
        // `rst_prolog`, see none of the prolog's definitions. The schedule
        // below parses it again if another document turns out to be first.
        let first_read = to_read.first().cloned();
        let start_of = |docname: &str| {
            if first_read.as_deref() == Some(docname) {
                &fresh
            } else {
                &baseline
            }
        };
        let mut results: Vec<(PathBuf, Result<ReadResult>)> = pool.install(|| {
            files
                .par_iter()
                .map(|file_path| {
                    let docname = self.docname_of_path(file_path);
                    let outdated = to_read.contains(&docname);
                    let start = start_of(&docname);
                    (
                        file_path.clone(),
                        self.read_one_file(file_path, docname, &found_docs, outdated, start),
                    )
                })
                .collect()
        });

        // The documents this build parsed, in sorted docname order.
        let mut parsed: Vec<(String, usize)> = results
            .iter()
            .enumerate()
            .filter_map(|(index, (_, result))| match result {
                Ok(read) if read.read_time_us.is_some() => Some((read.docname.clone(), index)),
                _ => None,
            })
            .collect();
        parsed.sort();
        let starts: Vec<&crate::rst::roles::RoleTable> = parsed
            .iter()
            .map(|(docname, _)| start_of(docname).as_ref())
            .collect();
        let roles_after: Vec<&crate::rst::roles::RoleTable> = parsed
            .iter()
            .map(|(_, index)| match &results[*index].1 {
                Ok(read) => &read.outcome.output.roles_after,
                Err(_) => unreachable!("only successful reads are listed"),
            })
            .collect();
        let mut reparsed = 0;
        if let ReadSchedule::Reparse { from } = ReadSchedule::plan(&fresh, &starts, &roles_after) {
            let mut roles = match from.checked_sub(1) {
                Some(before) => Arc::new(roles_after[before].clone()),
                None => Arc::clone(&fresh),
            };
            for (docname, index) in &parsed[from..] {
                let file_path = results[*index].0.clone();
                let result = self.parse_one_file(&file_path, docname.clone(), &found_docs, &roles);
                if let Ok(read) = &result {
                    roles = Arc::new(read.outcome.output.roles_after.clone());
                }
                results[*index].1 = result;
                reparsed += 1;
            }
        }
        self.reparsed
            .store(reparsed, std::sync::atomic::Ordering::Relaxed);

        // The cache stores each parsed document once, as the schedule left
        // it, in parallel: a parse the schedule redid never reaches it. A
        // document that cannot be stored fails as one that cannot be
        // parsed does.
        if self.incremental {
            pool.install(|| {
                results.par_iter_mut().for_each(|(file_path, result)| {
                    let stored = match result {
                        Ok(read) if read.read_time_us.is_some() => {
                            self.cache.store_document(file_path, &read.document)
                        }
                        _ => return,
                    };
                    if let Err(error) = stored {
                        *result = Err(error);
                    }
                });
            });
        }

        let mut read_results = Vec::with_capacity(results.len());
        for (file_path, result) in results {
            match result {
                Ok(read) => read_results.push(read),
                Err(e) => {
                    self.errors.lock().unwrap().push(BuildErrorReport::new(
                        file_path,
                        None,
                        format!("{e:#}"),
                        ErrorType::ParseError,
                    ));
                }
            }
        }

        Ok(read_results)
    }

    /// One document's read-phase result.
    ///
    /// `outdated` decides *how*: an outdated document is parsed (its cache
    /// entry, however valid, describes a document the environment has
    /// already been told to forget), an up-to-date one is recovered whole
    /// from the cache — rendered page and persisted doctree — and is not
    /// merged into the environment again. A recovery that fails is not
    /// fatal: the document is parsed instead, which is honest work rather
    /// than a hit, and the cache counts it as the miss it is.
    fn read_one_file(
        &self,
        file_path: &Path,
        docname: String,
        found_docs: &Arc<BTreeSet<String>>,
        outdated: bool,
        roles: &Arc<crate::rst::roles::RoleTable>,
    ) -> Result<ReadResult> {
        let relative_path = file_path.strip_prefix(&self.source_dir)?;
        debug!("Processing file: {}", relative_path.display());

        // The write phase still writes an unread document's page — skipping
        // the write is how cached pages went missing from the output tree.
        if !outdated && self.incremental {
            if let Ok(file_mtime) = utils::get_file_mtime(file_path) {
                let hit = self.cache.get_document_with(file_path, |cached| {
                    if cached.source_mtime < file_mtime || cached.html.is_empty() {
                        return None;
                    }
                    // A cached document whose doctree file is gone, or was
                    // written in a format this build no longer reads, is not
                    // usable: the resolve phase needs that doctree, and
                    // inventing an empty one would quietly drop the
                    // document's toc, titles and toctrees. Re-parse instead
                    // — this counts as a cache miss.
                    self.load_doctree(&docname)
                });
                if let Some((document, doctree)) = hit {
                    debug!("Using cached version of {}", relative_path.display());
                    return Ok(ReadResult {
                        docname,
                        document,
                        outcome: ReadOutcome::without_parse(doctree),
                        read_time_us: None,
                    });
                }
            }
        }

        self.parse_one_file(file_path, docname, found_docs, roles)
    }

    /// One document's parse, from the role registries `roles`: its read
    /// and the document it gives. [`Self::read_phase`] stores the document
    /// in the cache once the read schedule has settled, when the build is
    /// incremental.
    fn parse_one_file(
        &self,
        file_path: &Path,
        docname: String,
        found_docs: &Arc<BTreeSet<String>>,
        roles: &Arc<crate::rst::roles::RoleTable>,
    ) -> Result<ReadResult> {
        let content = std::fs::read_to_string(file_path)?;
        let parsed = self.parser.parse_full_from(
            file_path,
            &content,
            &docname,
            Some(Arc::clone(found_docs)),
            Arc::clone(roles),
        )?;
        let mut document = parsed.document;

        // Simple document rendering (placeholder). Done here, in the read
        // phase, because the rendered HTML is part of what the incremental
        // cache stores; the write phase only puts it on disk.
        let rendered_html = format!(
            "<html><body>{}</body></html>",
            html_escape::encode_text(&document.content.to_string())
        );
        document.html = rendered_html;

        // The doctree is *not* persisted here: the merge phase's stage M
        // still changes it (the index domain removes a node whose entries
        // do not validate, `FilterSystemMessages` the reporter's messages),
        // and what a later build loads has to be the tree this build
        // resolved. See [`Self::merge_phase`].

        Ok(ReadResult {
            docname,
            document,
            outcome: parsed.outcome,
            read_time_us: Some(now_micros()),
        })
    }

    /// Merge phase: fold the read phase's per-document output into the
    /// environment, in docname order.
    ///
    /// Sequential and deterministic, mirroring Sphinx's `_read_serial`
    /// (`builders/__init__.py:578-589`): each document's stage M
    /// ([`merge_document`]) runs the read transforms that touch the
    /// environment — the domains' `process_doc`, the `doctree-read`
    /// collectors — and the rest of the read-transform table after them,
    /// `FilterSystemMessages` (999) included.
    ///
    /// Only documents this build actually **read** are merged. Sphinx's
    /// `Builder._read_serial` clears a document immediately before reading
    /// it and touches nothing else; a document that was not outdated keeps
    /// every contribution the build that read it made, which is the whole
    /// point of the environment being persistent.
    ///
    /// This phase also **persists** each re-read document's doctree, at the
    /// end of its merge: stage M changes the tree (the index domain removes
    /// an `index` node whose entries do not validate, 999 the reporter's
    /// messages), and a later build must load what it left, not what the
    /// parser produced.
    ///
    /// And it prints each re-read document's read-phase records, in the
    /// order stage M returns them — stage R's (the parse's reporter and
    /// logger records, in creation order), then stage M's — one document at
    /// a time in sorted docname order, whatever order the parallel reads
    /// finished in. A document recovered from the cache prints none
    /// (decision 6).
    ///
    /// A document whose read crashed Sphinx (decision 13) ends the phase
    /// with its crash, located: the first such document in this sorted
    /// order is the one Sphinx's read crashes on, whatever stage of its
    /// read raised, and whatever `-j` read it. Its records before the
    /// crash print, as the earlier documents' did; it is not merged and
    /// its doctree is not persisted, and the documents after it are not
    /// merged.
    fn merge_phase(
        &self,
        env: &mut BuildEnvironment,
        results: &mut [ReadResult],
    ) -> Result<(), SphinxCrash> {
        env.root_doc = self
            .config
            .root_doc
            .clone()
            .unwrap_or_else(|| DEFAULT_ROOT_DOC.to_string());

        // Documents that vanished since the saved environment was written
        // (Sphinx's `removed` set) must not leave stale state behind. The
        // set is taken from what the read phase came back with rather than
        // from `get_outdated_files`: it is the same set plus any document
        // that failed to read, whose recorded state is equally worthless.
        let present: HashSet<&str> = results.iter().map(|r| r.docname.as_str()).collect();
        let stale: Vec<String> = env
            .all_docs
            .keys()
            .filter(|docname| !present.contains(docname.as_str()))
            .cloned()
            .collect();
        for docname in stale {
            env.clear_doc(&docname);
        }

        let mut ordered: Vec<usize> = (0..results.len()).collect();
        ordered.sort_by(|a, b| results[*a].docname.cmp(&results[*b].docname));

        // Sphinx's `env.doc2path`: the source file a docname was discovered
        // in, and for a docname the project has no file for,
        // `<srcdir>/<docname><source_suffix[0]>`, the path `doc2path`
        // synthesizes (`SP/sphinx/project.py:114-128`).
        //
        // Owned rather than borrowed from `results`, which this loop holds
        // mutably: stage M changes each document's read.
        let paths: HashMap<String, PathBuf> = results
            .iter()
            .map(|result| (result.docname.clone(), result.document.source_path.clone()))
            .collect();
        let doc2path = |docname: &str| -> PathBuf {
            paths
                .get(docname)
                .cloned()
                .unwrap_or_else(|| self.undiscovered_path(docname))
        };
        let settings = ReadSettings::from_config(&self.config, self.builder_kind);

        for index in ordered {
            let result = &mut results[index];
            // A document this build did not read contributes nothing: what
            // it contributed last time is still in the environment, and
            // still correct.
            let Some(read_time_us) = result.read_time_us else {
                continue;
            };
            let docname = result.docname.clone();
            let docname = docname.as_str();

            // Stage M reads the parse's registry export and toctree records
            // from the read, where a standalone read keeps them; the read
            // phase moved them on to the `Document`, which the incremental
            // cache persists, so they are lent back for the merge. The
            // parse's directive and role records, which it moved there too
            // for the validators, are not: stage M sees them empty in a
            // build, while `read_standalone` keeps them. No stage-M step
            // reads them; one that does must be lent them here as well.
            let output = &mut result.outcome.output;
            output.registry = std::mem::take(&mut result.document.registry);
            output.toctrees = std::mem::take(&mut result.document.toctrees);
            let inputs = MergeInputs {
                source_path: &result.document.source_path,
                source_dir: &self.source_dir,
                doc2path: &doc2path,
                read_time_us,
            };
            let records = merge_document(env, docname, &mut result.outcome, &inputs, &settings);
            let output = &mut result.outcome.output;
            result.document.registry = std::mem::take(&mut output.registry);
            result.document.toctrees = std::mem::take(&mut output.toctrees);

            // The document's read-phase records, already in the order
            // Sphinx prints them: keyed in that order, the sink keeps it
            // and drops what `suppress_warnings` names. Sphinx reads
            // documents in this same sorted order, so the read sinks flush
            // in it too.
            let mut sink = DiagnosticSink::default();
            for record in records {
                sink.push_next(record);
            }
            self.flush(sink);
            if let Some(crash) = &result.outcome.state().crash {
                return Err(crash.clone().located(
                    &result.outcome.doctree().sources,
                    &self.source_dir,
                    docname,
                    self.config.source_suffix.first(),
                ));
            }

            // Persist the doctree *after* stage M has had it, the way Sphinx
            // writes its pickle after the read's transforms and
            // `doctree-read` handlers run (`builders/__init__.py:632-671`).
            // Persisting the parser's tree instead would hand the next build
            // a document this build already rejected part of — and the two
            // builds would resolve different trees.
            //
            // Failing to write it is reported like any other per-document
            // failure, but the file is removed first: a stale doctree left
            // beside a fresh `all_docs` entry would look current to the next
            // build, whereas a missing one simply makes the document
            // outdated and re-read.
            if let Err(e) = self.store_doctree(docname, result.outcome.doctree()) {
                let _ = std::fs::remove_file(self.doctree_path(docname));
                self.errors.lock().unwrap().push(BuildErrorReport::new(
                    result.document.source_path.clone(),
                    None,
                    format!("{e:#}"),
                    ErrorType::Other,
                ));
            }
        }
        Ok(())
    }

    /// Resolve phase: whole-project state that only exists once every
    /// document has been read, then persist the environment.
    ///
    /// Runs the numbering passes (`TocTreeCollector.get_updated_docs`, which
    /// Sphinx dispatches through `env-get-updated` right after the read
    /// phase) and Sphinx's post-read consistency checks
    /// (`env.check_consistency()`) — over the finished toctree graph, then
    /// the citation domain's — then saves the environment — Sphinx's own
    /// end-of-read-phase step (`builders/__init__.py:420`).
    ///
    /// Every document is resolved, not only the ones this build read: the
    /// write phase emits every page (see [`Self::write_phase`]), and a page
    /// is written from a doctree resolved against the environment as it
    /// stands *now* — a document that was not re-read can still have gained
    /// a section number, or lost the target of one of its references.
    ///
    /// Failing to save the environment is **not** a build failure: the
    /// cache directory is optional infrastructure, the output this build
    /// produced is valid without it, and the only consequence is that the
    /// next build starts cold. It is reported and the build goes on.
    ///
    /// A reference that crashes Sphinx as it is resolved (decision 13)
    /// ends the phase with that crash ([`Self::xref_phase`]): the general
    /// index is not assembled and the environment is not saved.
    fn resolve_phase(
        &self,
        env: &mut BuildEnvironment,
        results: &[ReadResult],
    ) -> Result<(), SphinxCrash> {
        info!("Resolving build environment");

        let sources: HashMap<&str, &Path> = results
            .iter()
            .map(|result| {
                (
                    result.docname.as_str(),
                    result.document.source_path.as_path(),
                )
            })
            .collect();

        self.number_phase(env, results);
        let mut consistency = DiagnosticSink::default();
        for message in env_toctree::check_consistency(env) {
            // Sphinx logs these with `location=docname`, which renders as
            // the document's source path with no line number.
            let source = sources
                .get(message.docname.as_str())
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&message.docname));
            match message.level {
                ConsistencyLevel::Warning => consistency.push_next(
                    BuildWarning::new(source, None, message.message, WarningType::OrphanedDocument)
                        .with_category(message.category),
                ),
                // Sphinx uses `logger.info` for the multiple-parents note,
                // so it must stay out of the warning count (and out of -W).
                ConsistencyLevel::Info => info!("{}: {}", source.display(), message.message),
            }
        }
        // Then the domains' own checks (`self.domains._check_consistency()`,
        // `environment/__init__.py:820-821`), of which only the citation
        // domain's says anything: its citations no document refers to,
        // located `(docname, lineno)`, whose `doc2path` is the document's
        // source.
        let doc2path = |docname: &str| -> PathBuf {
            sources
                .get(docname)
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| self.undiscovered_path(docname))
        };
        for warning in env_citation::check_consistency(env, &doc2path) {
            consistency.push_next(warning);
        }
        self.flush(consistency);

        self.xref_phase(env, results, &doc2path)?;
        self.genindex_phase(env, &sources);
        self.py_modindex_phase(env);

        if let Err(e) = env.save(self.cache.cache_dir()) {
            log::warn!(
                "Could not save the build environment to {}: {e:#} — this build's \
                 output is complete, but the next one will start from scratch",
                self.cache.cache_dir().display()
            );
        }
        Ok(())
    }

    /// Assemble the general index (`IndexEntries.create_index`).
    ///
    /// Sphinx runs this from the HTML builder's `write_genindex`, so a
    /// `dummy` build never reaches it — but the environment oracle calls it
    /// explicitly, right after the build and before snapshotting warnings
    /// (see `tools/gen_env_fixture.py`), which is why this runs last and
    /// unconditionally rather than from the writer.
    fn genindex_phase(&self, env: &BuildEnvironment, sources: &HashMap<&str, &Path>) {
        // The oracle's dummy builder answers `get_relative_uri('genindex',
        // docname)` with `''` for every document, making each target a bare
        // `#<target_id>` — the same honest answer [`Self::xref_phase`] gives
        // until the HTML writer supplies its own uri scheme.
        let rel_uri = |_docname: &str| Some(String::new());
        let mut messages = Vec::new();
        let groups = env_genindex::create_index(env, &rel_uri, &mut messages);
        // Sphinx assembles the index while finishing the build
        // (`write_genindex`), after every page is written.
        let mut finish = DiagnosticSink::default();
        for message in messages {
            // `location=docname`: the document's source path, no line.
            let source = sources
                .get(message.docname.as_str())
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&message.docname));
            finish.push_next(message.into_warning(&source));
        }
        self.flush(finish);
        *self.genindex.lock().unwrap() = groups;
    }

    /// Assemble the Python module index (`PythonModuleIndex.generate`).
    ///
    /// Like [`Self::genindex_phase`], Sphinx only runs this from an HTML
    /// build (`write_domain_indices`) — the environment oracle calls
    /// `generate()` explicitly after its dummy build, so this too runs
    /// unconditionally at the end of the resolve phase. It raises no
    /// diagnostics of its own.
    fn py_modindex_phase(&self, env: &BuildEnvironment) {
        *self.py_modindex.lock().unwrap() =
            env_py_domain::generate_modindex(&env.py, &self.config.modindex_common_prefix);
    }

    /// Sphinx's `get_and_resolve_doctree`, per document as Sphinx writes it
    /// — after numbering, which `:numref:` reads: the post-transforms,
    /// `ReferencesResolver` (10), then `OnlyNodeTransform` (50),
    /// `PropagateDescDomain` (200), `HighlightLanguageTransform` (400) and
    /// `TrimDoctestFlagsTransform` (401) ([`post::apply_post_transforms`]),
    /// then the document's own toctrees, each replaced by its resolution
    /// ([`env_toctree::resolve_body_toctrees`]); the `only` nodes, the tocs'
    /// included, are evaluated against the `-t` tags and the builder's own
    /// ([`BuilderKind::tags`]). `doc2path` is `env.doc2path`, where a
    /// toctree warning about a document points.
    ///
    /// Each document is resolved over a *copy* of its doctree, exactly like
    /// Sphinx's `get_and_resolve_doctree`, and the result is kept as
    /// pseudo-XML for [`Self::snapshot_env`]. Documents are visited in
    /// docname order, which is the order Sphinx's write loop uses and
    /// therefore the order its warnings come out in.
    fn xref_phase(
        &self,
        env: &BuildEnvironment,
        results: &[ReadResult],
        doc2path: &dyn Fn(&str) -> PathBuf,
    ) -> Result<(), SphinxCrash> {
        let in_memory: HashMap<&str, &Doctree> = results
            .iter()
            .map(|result| (result.docname.as_str(), result.outcome.doctree()))
            .collect();
        let load_doctree = |docname: &str| -> Option<Cow<'_, Doctree>> {
            match in_memory.get(docname) {
                Some(doctree) => Some(Cow::Borrowed(*doctree)),
                None => self.load_doctree(docname).map(Cow::Owned),
            }
        };
        // The oracle builds with sphinx's dummy builder, whose
        // `get_target_uri` is `''`; nothing consumes a resolved doctree's
        // URIs yet (the write phase renders from `Document.html`), so this
        // is the one honest answer until the HTML writer supplies its own.
        let relative_uri = |_from: &str, _to: &str| String::new();
        let resolver = env_resolve::Resolver {
            env,
            numfig: self.config.numfig,
            numfig_format: &self.config.numfig_format,
            math: crate::env::math::MathConfig::from(&self.config),
            doctree: &load_doctree,
            relative_uri: &relative_uri,
            intersphinx: &self.intersphinx,
        };
        let nitpick = env_resolve::NitpickConfig {
            nitpicky: self.config.nitpicky,
            ignore: &self.config.nitpick_ignore,
            ignore_regex: &self.config.nitpick_ignore_regex,
        };
        let tags = Tags::new(
            self.config
                .tags
                .iter()
                .cloned()
                .chain(self.builder_kind.tags().iter().map(|tag| tag.to_string())),
        );
        let config = post::PostTransformConfig {
            tags: &tags,
            highlight_language: &self.config.highlight_language,
            trim_doctest_flags: self.config.trim_doctest_flags,
            source_dir: &self.source_dir,
        };
        let sources = |docname: &str| {
            in_memory
                .get(docname)
                .map(|doctree| doctree.sources.as_slice())
        };
        let locations = env_toctree::ToctreeLocations {
            source_dir: &self.source_dir,
            doc2path,
            sources: &sources,
            include_patterns: &self.config.include_patterns,
            exclude_patterns: &self.config.exclude_patterns,
        };

        let mut ordered: Vec<&ReadResult> = results.iter().collect();
        ordered.sort_by(|a, b| a.docname.cmp(&b.docname));

        // A second `build()` on the same builder must not keep the previous
        // one's documents (one of them may since have been deleted).
        self.resolved.lock().unwrap().clear();
        *self.search_index.lock().unwrap() = crate::search::SphinxIndexBuilder::new();
        let mut unresolvable_domain_refs = 0usize;
        for result in ordered {
            let mut doctree = result.outcome.doctree().clone();
            let post = post::apply_post_transforms(
                &resolver,
                &nitpick,
                &config,
                &result.docname,
                &result.document.source_path,
                &mut doctree,
            );
            unresolvable_domain_refs += post.unresolvable_domain_refs;
            // One resolve sink per document, flushed in this sorted order,
            // its records in the order the post-transforms, then the
            // toctree resolution, logged them.
            let mut sink = DiagnosticSink::default();
            for record in post.records {
                sink.push_next(record);
            }
            // A reference that crashes Sphinx (decision 13) ends the
            // phase, after the records Sphinx logged before it: those of
            // the documents written before this one, and this one's.
            if let Some(crash) = post.crash {
                self.flush(sink);
                return Err(crash);
            }
            env_toctree::resolve_body_toctrees(
                env,
                &result.docname,
                &mut doctree,
                &relative_uri,
                &tags,
                &locations,
                &mut sink,
            );
            self.flush(sink);

            // Sphinx feeds the search index from exactly this resolved
            // doctree (`write_doc_serialized` → `index_page`), with the
            // longtitle as plain text and the source-relative filename.
            let title = env
                .longtitles
                .get(&result.docname)
                .map(crate::doctree::Node::astext)
                .unwrap_or_default();
            let filename = result
                .document
                .source_path
                .strip_prefix(&self.source_dir)
                .unwrap_or(&result.document.source_path)
                .to_string_lossy()
                .into_owned();
            self.search_index.lock().unwrap().feed(
                &result.docname,
                &filename,
                &title,
                &doctree,
                env,
            );

            self.resolved
                .lock()
                .unwrap()
                .insert(result.docname.clone(), doctree.root.pformat());
        }

        if unresolvable_domain_refs > 0 {
            // References into domains this build has no resolver for —
            // `refdomain` outside `{"", "std", "py"}` (`c:`, `cpp:`, `js:`,
            // ...) — counted by the resolver rather than warned about.
            info!(
                "{unresolvable_domain_refs} cross-domain reference(s) not validated \
                 (domain not implemented until M5)"
            );
        }
        Ok(())
    }

    /// Section and figure numbering (`TocTreeCollector.get_updated_docs`,
    /// `collectors/toctree.py:194`), run in that order: figure numbers are
    /// scoped by the section numbers the first pass assigns.
    ///
    /// The doctree loader hands the walks whatever this build already has in
    /// memory — every read result carries its doctree, including the ones a
    /// warm cache hit loaded from disk — and falls back to the persisted
    /// doctree for anything else.
    ///
    /// The returned docnames (Sphinx's `rewrite_needed`) are the documents
    /// whose numbering moved, which Sphinx adds to its write set. They are
    /// logged rather than consumed here because this builder's write set is
    /// already every found document (see [`Self::write_phase`]) — a
    /// superset — so there is nothing left for them to widen.
    fn number_phase(&self, env: &mut BuildEnvironment, results: &[ReadResult]) {
        let in_memory: HashMap<&str, &Doctree> = results
            .iter()
            .map(|result| (result.docname.as_str(), result.outcome.doctree()))
            .collect();
        let load_doctree = |docname: &str| -> Option<std::borrow::Cow<'_, Doctree>> {
            match in_memory.get(docname) {
                Some(doctree) => Some(std::borrow::Cow::Borrowed(*doctree)),
                None => self.load_doctree(docname).map(std::borrow::Cow::Owned),
            }
        };

        let sections = env_numbers::assign_section_numbers(env, &load_doctree);
        let mut sink = DiagnosticSink::default();
        for warning in sections.warnings {
            self.report_numbering_warning(&warning, results, &mut sink);
        }
        self.flush(sink);
        let figures = env_numbers::assign_figure_numbers(
            env,
            self.config.numfig,
            self.config.numfig_secnum_depth,
            &load_doctree,
        );
        debug!(
            "Numbering: {} document(s) with changed section numbers, {} with changed figure numbers",
            sections.changed.len(),
            figures.len()
        );
    }

    /// Surface one numbering diagnostic at the location Sphinx logs it —
    /// the `(source, line)` of the `toctree` node it names
    /// (`location=toctreenode`), which the parse record for that
    /// document's Nth toctree carries; a toctree spliced in from an
    /// included file names that file through the doctree's source table,
    /// made absolute as `get_node_location` makes it
    /// ([`crate::rst::node_source_path`]).
    fn report_numbering_warning(
        &self,
        warning: &env_numbers::NumberingWarning,
        results: &[ReadResult],
        sink: &mut DiagnosticSink,
    ) {
        let result = results
            .iter()
            .find(|result| result.docname == warning.docname);
        let document = result.map(|result| &result.document);
        let toctree = document.and_then(|document| document.toctrees.get(warning.toctree_index));
        let source = result
            .and_then(|result| {
                let toctree = toctree?;
                result
                    .outcome
                    .doctree()
                    .sources
                    .get(toctree.source as usize)
                    .map(|source| {
                        PathBuf::from(crate::rst::node_source_path(source, &self.source_dir))
                    })
            })
            .or_else(|| document.map(|document| document.source_path.clone()))
            .unwrap_or_else(|| PathBuf::from(&warning.docname));
        let line = toctree.map(|toctree| toctree.line as usize);
        sink.push_next(
            BuildWarning::new(
                source,
                line,
                warning.message.clone(),
                WarningType::MissingToctreeRef,
            )
            .with_category(warning.category.clone()),
        );
    }

    /// Write phase: emit every document's rendered output, in parallel and
    /// after resolution, so that a page can be written with whole-project
    /// knowledge (numbering, relations) once those exist.
    ///
    /// **Every found document is written, not just the ones this build
    /// read.** Sphinx writes the read set plus the toctree containers of
    /// what changed plus the documents whose numbering moved
    /// (`builders/__init__.py:717-736`), because its HTML builder also
    /// compares each output file against its sources and can tell that the
    /// rest are already on disk and current. This builder cannot do that
    /// yet, so it writes the superset — which is also what makes a cache
    /// hit still produce a page, rather than leaving a hole in the output
    /// tree where an unchanged document should be.
    ///
    /// One page failing to write must not abort the build (the same rule the
    /// read phase follows): it becomes a `BuildErrorReport`, and a non-zero
    /// exit, while the remaining pages are still written.
    fn write_phase(&self, documents: &[Document]) {
        documents.par_iter().for_each(|document| {
            if let Err(e) = self.write_one(document) {
                self.errors.lock().unwrap().push(BuildErrorReport::new(
                    document.source_path.clone(),
                    None,
                    format!("{e:#}"),
                    ErrorType::Other,
                ));
            }
        });
    }

    fn write_one(&self, document: &Document) -> Result<()> {
        let output_path = self.get_output_path(&document.source_path)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&output_path, &document.html)?;
        Ok(())
    }

    /// `<cache_dir>/doctrees/<blake3(docname)>.doctree`.
    fn doctree_path(&self, docname: &str) -> PathBuf {
        let hash = blake3::hash(docname.as_bytes());
        self.cache
            .cache_dir()
            .join(DOCTREE_SUBDIR)
            .join(format!("{}.doctree", hash.to_hex()))
    }

    /// Persist one document's doctree, behind the
    /// [`DOCTREE_MAGIC`]/[`DOCTREE_FORMAT_VERSION`] header that lets a later
    /// build tell whether the bytes are still readable *by meaning*, not
    /// just by bincode.
    fn store_doctree(&self, docname: &str, doctree: &Doctree) -> Result<()> {
        let path = self.doctree_path(docname);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let blob = crate::doctree::to_bincode(doctree);
        let mut bytes = Vec::with_capacity(DOCTREE_HEADER_LEN + blob.len());
        bytes.extend_from_slice(DOCTREE_MAGIC);
        bytes.extend_from_slice(&DOCTREE_FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&blob);
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// The persisted doctree for `docname`, or `None` if it is missing,
    /// carries another format version (including none at all — a file
    /// written before the header existed), or cannot be decoded (a
    /// truncated write). Every `None` means the same thing to callers: this
    /// document has to be read again.
    fn load_doctree(&self, docname: &str) -> Option<Doctree> {
        let path = self.doctree_path(docname);
        let bytes = std::fs::read(&path).ok()?;
        let Some(blob) = current_format_doctree(&bytes) else {
            debug!(
                "Ignoring doctree {} written in another format (re-reading {docname})",
                path.display()
            );
            return None;
        };
        match crate::doctree::from_bincode(blob) {
            Ok(doctree) => Some(doctree),
            Err(e) => {
                debug!(
                    "Ignoring unreadable doctree {}: {e:#} (re-reading {docname})",
                    path.display()
                );
                None
            }
        }
    }

    /// The page of the document read from `source_path`:
    /// `<outdir>/<docname>.html`. Swapping the file's extension for `.html`
    /// would misname it for a `source_suffix` with a second dot or a comma
    /// in it (`page.rst.txt`, `index.rst,.txt`).
    fn get_output_path(&self, source_path: &Path) -> Result<PathBuf> {
        // A file outside the source directory has no page in the output
        // directory: its docname would be its absolute path.
        source_path.strip_prefix(&self.source_dir)?;
        let docname = self.docname_of_path(source_path);
        let mut output_path = self.output_dir.clone();
        output_path.extend(docname.split('/'));
        output_path.as_mut_os_string().push(".html");
        Ok(output_path)
    }

    async fn generate_indices(&self, _documents: &[Document]) -> Result<()> {
        info!("Generating indices and cross-references");

        // The same `get_objects()` rows the search index lists, through the
        // same writer the direct `HTMLBuilder` uses.
        let domains = env::objects::domain_objects(&self.env);
        InventoryFile::dump(
            self.output_dir.join("objects.inv"),
            &self.config.project,
            self.config.version.as_deref().unwrap_or(""),
            &domains,
            // `StandaloneHTMLBuilder.get_target_uri`: `quote(docname) + link_suffix`.
            |docname| format!("{}.html", utils::quote_url_path(docname)),
        )
        .await
    }

    async fn copy_static_assets(&self) -> Result<()> {
        info!("Copying static assets");

        // Create _static directory
        let static_output_dir = self.output_dir.join("_static");
        tokio::fs::create_dir_all(&static_output_dir).await?;

        // Copy built-in static assets - use relative path from binary location
        let exe_dir = std::env::current_exe()?
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Could not determine executable directory"))?
            .to_path_buf();

        // Try multiple possible locations for static assets
        let possible_static_dirs = [
            exe_dir.join("../static"),                      // Release build
            exe_dir.join("../../static"),                   // Debug build
            exe_dir.join("../../../static"),                // Deep build
            Path::new("rust-builder/static").to_path_buf(), // Local development
        ];

        let mut static_assets_copied = false;
        for builtin_static_dir in &possible_static_dirs {
            if builtin_static_dir.exists() {
                debug!("Found static assets at: {:?}", builtin_static_dir);
                for entry in std::fs::read_dir(builtin_static_dir)? {
                    let entry = entry?;
                    let file_path = entry.path();
                    if file_path.is_file() {
                        let file_name = file_path.file_name().unwrap();
                        let dest_path = static_output_dir.join(file_name);
                        tokio::fs::copy(&file_path, &dest_path).await?;
                        debug!("Copied static asset: {:?}", file_name);
                    }
                }
                static_assets_copied = true;
                break;
            }
        }

        if !static_assets_copied {
            debug!("No built-in static assets found, creating basic ones");
            // Create minimal CSS files if not found
            self.create_default_static_assets(&static_output_dir)
                .await?;
        }

        // Copy project-specific static assets
        let static_dirs = [
            self.source_dir.join("_static"),
            self.source_dir.join("_templates"),
        ];

        for static_dir in &static_dirs {
            if static_dir.exists() {
                let dest = self.output_dir.join(static_dir.file_name().unwrap());
                utils::copy_dir_recursive(static_dir, &dest).await?;
                debug!("Copied static directory: {:?}", static_dir);
            }
        }

        Ok(())
    }

    async fn create_default_static_assets(&self, static_dir: &Path) -> Result<()> {
        // Create basic pygments.css
        let pygments_css = include_str!("../static/pygments.css");
        tokio::fs::write(static_dir.join("pygments.css"), pygments_css).await?;

        // Create basic theme.css
        let theme_css = include_str!("../static/theme.css");
        tokio::fs::write(static_dir.join("theme.css"), theme_css).await?;

        // Create basic JavaScript files
        let jquery_js = include_str!("../static/jquery.js");
        tokio::fs::write(static_dir.join("jquery.js"), jquery_js).await?;

        let doctools_js = include_str!("../static/doctools.js");
        tokio::fs::write(static_dir.join("doctools.js"), doctools_js).await?;

        let sphinx_highlight_js = include_str!("../static/sphinx_highlight.js");
        tokio::fs::write(static_dir.join("sphinx_highlight.js"), sphinx_highlight_js).await?;

        debug!("Created default static assets");
        Ok(())
    }

    /// `doc2path(docname)` for a docname the project has no file for:
    /// `<srcdir>/<docname><source_suffix[0]>` (`Project.doc2path`,
    /// `SP/sphinx/project.py:114-128`).
    fn undiscovered_path(&self, docname: &str) -> PathBuf {
        self.source_dir
            .join(format!("{docname}{}", self.config.source_suffix.first()))
    }

    /// Root-relative docname (forward slashes) for a source path: the path
    /// less its `source_suffix` ([`Self::source_docname`]), or, for a path
    /// discovery would not have admitted, less its extension.
    fn docname_of_path(&self, path: &Path) -> String {
        self.source_docname(path).unwrap_or_else(|| {
            let relative = path.strip_prefix(&self.source_dir).unwrap_or(path);
            relative
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/")
        })
    }

    /// Run the directive/role validation system over every RST document.
    ///
    /// Findings surface as build *warnings*, flushed after every other
    /// record of the build (so `-W`/`-w` govern promotion); `Unknown`
    /// results stay silent — the built-in validators cover a
    /// fraction of real Sphinx, and reporting the rest would drown every
    /// real project in noise.
    fn validate_directives_and_roles(&self, processed_docs: &[Document], doctrees: &[Doctree]) {
        use crate::directives::validation::{
            DirectiveValidationResult, DirectiveValidationSystem, ParsedDirective, ParsedRole,
            RoleValidationResult, SourceLocation,
        };
        use crate::document::DocumentContent;

        debug_assert_eq!(processed_docs.len(), doctrees.len());
        let results: Vec<(Vec<BuildWarning>, usize)> = processed_docs
            .par_iter()
            .zip(doctrees.par_iter())
            .filter_map(|(doc, doctree)| {
                if !matches!(&doc.content, DocumentContent::RestructuredText(_)) {
                    return None;
                }

                let mut warnings = Vec::new();
                let mut unknown = 0usize;
                // Statistics make validate_* take &mut self, so each document
                // gets its own (cheap) system instance for the parallel pass.
                let mut system = DirectiveValidationSystem::new();
                // Since wave 3 the feed comes from the parse-time records
                // (M1-scanner-compatible tuples), not a raw re-scan. Each
                // record's `line` is numbered within its own source, so the
                // path comes from the doctree's source table: a directive
                // inside an included file is reported against that file.
                let file_of = |source: u16| -> String {
                    doctree
                        .sources
                        .get(source as usize)
                        .cloned()
                        .unwrap_or_else(|| doc.source_path.display().to_string())
                };
                let directives: Vec<ParsedDirective> = doc
                    .directive_records
                    .iter()
                    .map(|r| ParsedDirective {
                        name: r.name.clone(),
                        arguments: r.arguments.clone(),
                        options: r.options.iter().cloned().collect(),
                        content: r.content.clone(),
                        location: SourceLocation {
                            file: file_of(r.source),
                            line: r.line as usize,
                            column: 0,
                        },
                    })
                    .collect();
                let roles: Vec<ParsedRole> = doc
                    .role_records
                    .iter()
                    .map(|r| ParsedRole {
                        name: r.name.clone(),
                        target: r.target.clone(),
                        display_text: r.display.clone(),
                        location: SourceLocation {
                            file: file_of(r.source),
                            line: r.line as usize,
                            column: 0,
                        },
                    })
                    .collect();

                for directive in &directives {
                    match system.validate_directive(directive) {
                        DirectiveValidationResult::Valid => {}
                        DirectiveValidationResult::Unknown => unknown += 1,
                        DirectiveValidationResult::Warning(msg)
                        | DirectiveValidationResult::Error(msg) => {
                            warnings.push(BuildWarning::new(
                                PathBuf::from(&directive.location.file),
                                Some(directive.location.line),
                                msg,
                                crate::error::WarningType::Other,
                            ));
                        }
                    }
                }

                for role in &roles {
                    match system.validate_role(role) {
                        RoleValidationResult::Valid => {}
                        RoleValidationResult::Unknown => unknown += 1,
                        RoleValidationResult::Warning(msg) | RoleValidationResult::Error(msg) => {
                            warnings.push(BuildWarning::new(
                                PathBuf::from(&role.location.file),
                                Some(role.location.line),
                                msg,
                                crate::error::WarningType::Other,
                            ));
                        }
                    }
                }

                Some((warnings, unknown))
            })
            .collect();

        let mut unknown_total = 0usize;
        let mut sink = DiagnosticSink::default();
        for (warnings, unknown) in results {
            unknown_total += unknown;
            for warning in warnings {
                sink.push_next(warning);
            }
        }
        self.flush(sink);
        if unknown_total > 0 {
            debug!(
                "{} directive/role occurrence(s) had no validator and were not checked",
                unknown_total
            );
        }
    }

    async fn generate_search_index(&self) -> Result<()> {
        info!("Generating search index");
        let value = self.search_index.lock().unwrap().freeze(&self.env);
        let contents = crate::search::dumps_sphinx_index(&value)?;
        tokio::fs::write(self.output_dir.join("searchindex.js"), contents).await?;
        Ok(())
    }

    /// The environment this build produced (empty before [`Self::build`]).
    pub fn env(&self) -> &BuildEnvironment {
        &self.env
    }

    /// [`BuildEnvironment::snapshot`] of this build's environment — the
    /// shape the `env_differential` oracle compares against — plus the
    /// `resolved_pformat` of every document this build resolved, which is
    /// build output rather than environment state and so has no home inside
    /// [`BuildEnvironment`].
    pub fn snapshot_env(&self) -> serde_json::Value {
        let mut snapshot = self.env.snapshot();
        let resolved: serde_json::Map<String, serde_json::Value> = self
            .resolved
            .lock()
            .unwrap()
            .iter()
            .map(|(docname, pformat)| (docname.clone(), serde_json::Value::String(pformat.clone())))
            .collect();
        if let Some(object) = snapshot.as_object_mut() {
            object.insert(
                "resolved_pformat".to_string(),
                serde_json::Value::Object(resolved),
            );
            object.insert(
                "genindex".to_string(),
                env_genindex::snapshot(&self.genindex.lock().unwrap()),
            );
            object.insert(
                "py_modindex".to_string(),
                env_py_domain::modindex_snapshot(&self.py_modindex.lock().unwrap()),
            );
        }
        snapshot
    }
}

/// The bincode blob inside a persisted doctree file, or `None` if the file
/// does not start with this build's [`DOCTREE_MAGIC`] +
/// [`DOCTREE_FORMAT_VERSION`] header.
fn current_format_doctree(bytes: &[u8]) -> Option<&[u8]> {
    let (header, blob) = bytes.split_at_checked(DOCTREE_HEADER_LEN)?;
    if &header[..DOCTREE_MAGIC.len()] != DOCTREE_MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(header[DOCTREE_MAGIC.len()..].try_into().ok()?);
    (version == DOCTREE_FORMAT_VERSION).then_some(blob)
}

/// A file's modification time in microseconds since the Unix epoch — the
/// unit `env.all_docs` read times are in, so the two are directly
/// comparable (Sphinx's `_StrPath._last_modified_time`).
///
/// `None` when the file cannot be stat-ed, which is Sphinx's `OSError`
/// path: the caller treats it as "this document is outdated".
fn modified_us(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_micros() as u64)
            .unwrap_or(0),
    )
}

/// Wall-clock microseconds since the Unix epoch, the unit Sphinx stores in
/// `env.all_docs` (`time.time_ns() // 1_000`). A pre-epoch clock yields 0
/// rather than wrapping.
fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_project(source_dir: &Path) {
        std::fs::create_dir_all(source_dir).unwrap();
        std::fs::write(
            source_dir.join("index.rst"),
            "Index\n=====\n\n.. toctree::\n\n   a\n",
        )
        .unwrap();
        std::fs::write(source_dir.join("a.rst"), "A\n=\n\nBody.\n").unwrap();
    }

    /// A library caller that builds from a loaded `conf.py` without the
    /// CLI's `config-inited` step still gets the extension's
    /// `intersphinx_mapping` ([`BuildConfig::init_intersphinx_mapping`]),
    /// and a malformed one still stops the build.
    #[test]
    fn a_builder_initialises_the_intersphinx_mapping_itself() {
        let tmp = TempDir::new().unwrap();
        let conf = tmp.path().join("conf.py");
        std::fs::write(
            &conf,
            "extensions = ['sphinx.ext.intersphinx']\n\
             intersphinx_mapping = {'other': ('https://example.org/', None)}\n",
        )
        .unwrap();
        let config = BuildConfig::from_file(&conf).unwrap();
        assert!(config.intersphinx_mapping.is_empty(), "not yet initialised");
        let builder =
            SphinxBuilder::new(config, tmp.path().to_path_buf(), tmp.path().join("out")).unwrap();
        assert_eq!(
            builder.config.intersphinx_mapping["other"],
            ("https://example.org/".to_string(), vec![None])
        );

        std::fs::write(
            &conf,
            "extensions = ['sphinx.ext.intersphinx']\nintersphinx_mapping = {'p': 'x'}\n",
        )
        .unwrap();
        let config = BuildConfig::from_file(&conf).unwrap();
        let error = SphinxBuilder::new(config, tmp.path().to_path_buf(), tmp.path().join("out2"))
            .err()
            .expect("a malformed mapping stops the build");
        assert_eq!(
            error.to_string(),
            "Invalid `intersphinx_mapping` configuration (1 error)."
        );
    }

    fn build_incrementally(source_dir: &Path, output_dir: &Path) -> (BuildStats, SphinxBuilder) {
        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.to_path_buf(),
            output_dir.to_path_buf(),
        )
        .unwrap();
        builder.enable_incremental();
        let stats = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(builder.build())
            .unwrap();
        (stats, builder)
    }

    /// A cold build of the project `files` (relative path, text) with
    /// `jobs` parallel jobs, as the CLI's sphinx-build mode configures it:
    /// each document's resolved doctree, by docname, and the warning
    /// stream.
    fn build_roles_project(
        files: &[(&str, &str)],
        jobs: usize,
    ) -> (BTreeMap<String, String>, Vec<String>) {
        let (resolved, warnings, _) = build_roles_project_with(BuildConfig::default(), files, jobs);
        (resolved, warnings)
    }

    /// [`build_roles_project`] under `config`, M1's validation off, with
    /// how many documents the read schedule parsed a second time
    /// ([`SphinxBuilder::reparsed_documents`]).
    fn build_roles_project_with(
        config: BuildConfig,
        files: &[(&str, &str)],
        jobs: usize,
    ) -> (BTreeMap<String, String>, Vec<String>, usize) {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("src");
        for (path, text) in files {
            let path = source_dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let config = BuildConfig {
            validate_directives: false,
            ..config
        };
        let mut builder = SphinxBuilder::new(config, source_dir, tmp.path().join("out")).unwrap();
        builder.set_parallel_jobs(jobs);
        let stats = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(builder.build())
            .unwrap();
        let resolved = builder.resolved.lock().unwrap().clone();
        let warnings = stats
            .warning_details
            .iter()
            .map(|record| record.message.clone())
            .collect();
        (resolved, warnings, builder.reparsed_documents())
    }

    /// The lines of `resolved`'s document `docname` after its first
    /// `<paragraph>` line.
    fn paragraph<'r>(resolved: &'r BTreeMap<String, String>, docname: &str) -> Vec<&'r str> {
        let pformat = &resolved[docname];
        let start = pformat
            .lines()
            .position(|line| line.trim() == "<paragraph>")
            .unwrap_or_else(|| panic!("{docname} has a paragraph:\n{pformat}"));
        pformat.lines().skip(start + 1).collect()
    }

    /// Decision 14 ([RC §3.15]): docutils keeps the roles `.. role::`
    /// registers for the whole process, and Sphinx reads a build's
    /// documents in sorted docname order, so a custom role exists in every
    /// document after the one that defines it and in none before; so does
    /// a `.. default-role::` while `default_role` is unset ([RC §4.2]).
    /// The parallel parse starts the first document from the fresh registry
    /// and every other from the baseline, and reads each one after the
    /// first to change it again, in order.
    #[test]
    fn custom_role_defined_in_earlier_doc_applies_to_later_doc() {
        let files = [
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   c\n   d\n",
            ),
            ("a.rst", "A\n=\n\nEarly :red:`x`.\n"),
            (
                "b.rst",
                "B\n=\n\n.. role:: red\n\n.. default-role:: math\n\nHere :red:`y`.\n",
            ),
            ("c.rst", "C\n=\n\nLater :red:`z` and `a^2`.\n"),
            ("d.rst", "D\n=\n\nLast `b^2`.\n"),
        ];
        for jobs in [4, 1] {
            let (resolved, warnings) = build_roles_project(&files, jobs);
            assert_eq!(
                warnings,
                ["Unknown interpreted text role \"red\"."],
                "-j {jobs}"
            );
            assert_eq!(
                paragraph(&resolved, "c")[..6],
                [
                    "            Later ",
                    "            <inline classes=\"red\">",
                    "                z",
                    "             and ",
                    "            <math>",
                    "                a^2",
                ],
                "-j {jobs}"
            );
            assert_eq!(
                paragraph(&resolved, "d")[..3],
                [
                    "            Last ",
                    "            <math>",
                    "                b^2"
                ],
                "-j {jobs}"
            );
        }
    }

    /// Decision 14: a role defined in an included file is registered by
    /// the document that includes it, in that document's place in the
    /// read order.
    #[test]
    fn role_defined_in_included_file_applies_to_later_doc() {
        let files = [
            ("index.rst", "Index\n=====\n\n.. toctree::\n\n   a\n   b\n"),
            ("a.rst", "A\n=\n\n.. include:: roles.inc\n"),
            ("roles.inc", ".. role:: red\n"),
            ("b.rst", "B\n=\n\nLater :red:`z`.\n"),
        ];
        let (resolved, warnings) = build_roles_project(&files, 4);
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(
            paragraph(&resolved, "b")[..3],
            [
                "            Later ",
                "            <inline classes=\"red\">",
                "                z"
            ]
        );
    }

    /// Decision 14: a role `rst_epilog` defines is registered after each
    /// document's own text, so the first document read cannot use it and
    /// every later one can (Sphinx 9.1.0 `-j 1`, scratch
    /// `t17b/specs/epilog_role.json`: only `a.rst:4` errors).
    #[test]
    fn rst_epilog_role_applies_to_later_docs_only() {
        let config = BuildConfig {
            rst_epilog: Some(".. role:: red\n".to_string()),
            ..BuildConfig::default()
        };
        let files = [
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   a\n   b\n\nIndex :red:`i`.\n",
            ),
            ("a.rst", "A\n=\n\nEarly :red:`x`.\n"),
            ("b.rst", "B\n=\n\nLater :red:`y`.\n"),
        ];
        for jobs in [4, 1] {
            let (resolved, warnings, reparsed) =
                build_roles_project_with(config.clone(), &files, jobs);
            assert_eq!(
                warnings,
                ["Unknown interpreted text role \"red\"."],
                "-j {jobs}"
            );
            // `a` leaves the registries with `red`: `b` and `index` are
            // read again from them.
            assert_eq!(reparsed, 2, "-j {jobs}");
            assert_eq!(
                paragraph(&resolved, "b")[..3],
                [
                    "            Later ",
                    "            <inline classes=\"red\">",
                    "                y"
                ],
                "-j {jobs}"
            );
            let index = &resolved["index"];
            assert!(
                index.contains("<inline classes=\"red\">\n                i"),
                "-j {jobs}: {index}"
            );
        }
    }

    /// Decision 14's baseline: every document makes `rst_prolog`'s role
    /// and default-role definitions again before its own text, so each
    /// one leaves the registries the build's baseline holds, every
    /// document after the first starts from them, and the parallel parses
    /// stand. (Godot's prolog defines four roles this way.)
    #[test]
    fn prolog_only_roles_keep_reads_parallel() {
        use crate::rst::{parse_rst_full, ParseOptions, ReadConfig};
        let config = BuildConfig {
            rst_prolog: Some(
                ".. role:: button\n   :class: role-button role-ui\n\n.. default-role:: math\n"
                    .to_string(),
            ),
            ..BuildConfig::default()
        };
        let docs = [
            ("a", "A\n=\n\nPress :button:`a` for `a^2`.\n"),
            ("b", "B\n=\n\nPress :button:`b` for `b^2`.\n"),
            ("c", "C\n=\n\nPress :button:`c` for `c^2`.\n"),
        ];

        let parser = Parser::new(&config).unwrap();
        let (fresh, baseline) = (parser.fresh_roles(), parser.baseline_roles());
        assert_ne!(fresh, baseline, "the prolog defines roles");
        let starts = [fresh.as_ref(), baseline.as_ref(), baseline.as_ref()];
        let after: Vec<_> = docs
            .iter()
            .zip(starts)
            .map(|((docname, text), start)| {
                let opts = ParseOptions {
                    sphinx: true,
                    docname: docname.to_string(),
                    read: ReadConfig::from(&config),
                    roles: Some(Arc::new(start.clone())),
                    ..ParseOptions::default()
                };
                parse_rst_full(text, &opts).roles_after
            })
            .collect();
        let after: Vec<_> = after.iter().collect();
        assert_eq!(
            ReadSchedule::plan(fresh, &starts, &after),
            ReadSchedule::Parallel
        );

        let mut files = vec![(
            "index.rst",
            "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   c\n",
        )];
        let names = docs.map(|(docname, _)| format!("{docname}.rst"));
        files.extend(
            names
                .iter()
                .map(String::as_str)
                .zip(docs.map(|(_, text)| text)),
        );
        for jobs in [4, 1] {
            let (resolved, warnings, reparsed) =
                build_roles_project_with(config.clone(), &files, jobs);
            assert_eq!(warnings, Vec::<String>::new(), "-j {jobs}");
            assert_eq!(reparsed, 0, "-j {jobs}: the parallel parses stand");
            for (docname, _) in docs {
                assert_eq!(
                    paragraph(&resolved, docname)[..6].join("\n"),
                    format!(
                        "            Press \n\
                         \x20           <inline classes=\"role-button role-ui\">\n\
                         \x20               {docname}\n\
                         \x20            for \n\
                         \x20           <math>\n\
                         \x20               {docname}^2"
                    ),
                    "-j {jobs}"
                );
            }
        }
    }

    /// A prolog's `include` reads its file where a document at the root of
    /// the source directory would (Sphinx's `Include` resolves the path
    /// against the document's directory), so the baseline, computed once
    /// the source directory is known, holds the role the included file
    /// defines, and the parallel parses stand. (Sphinx 9.1.0, scratch
    /// `t17b-fix1/specs/prolog_include_role.json`: no record.)
    #[test]
    fn a_role_a_prolog_includes_keeps_reads_parallel() {
        let config = BuildConfig {
            rst_prolog: Some(".. include:: roles.inc\n".to_string()),
            ..BuildConfig::default()
        };
        let files = [
            ("roles.inc", ".. role:: red\n"),
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   a\n   b\n\nIndex :red:`i`.\n",
            ),
            ("a.rst", "A\n=\n\nEarly :red:`x`.\n"),
            ("b.rst", "B\n=\n\nLater :red:`y`.\n"),
        ];
        for jobs in [4, 1] {
            let (resolved, warnings, reparsed) =
                build_roles_project_with(config.clone(), &files, jobs);
            assert_eq!(warnings, Vec::<String>::new(), "-j {jobs}");
            assert_eq!(reparsed, 0, "-j {jobs}: the parallel parses stand");
            for docname in ["a", "b", "index"] {
                let paragraph = paragraph(&resolved, docname).join("\n");
                assert!(
                    paragraph.contains("<inline classes=\"red\">"),
                    "-j {jobs}, {docname}: {paragraph}"
                );
            }
        }
    }

    /// The first document Sphinx reads starts from the registries no
    /// document has changed, so the leading field-marker lines of that
    /// one alone, which come before `rst_prolog`, see none of its
    /// definitions; every later document's do (Sphinx 9.1.0 `-j 1`,
    /// scratch `t17b/specs/prolog_role_fieldline.json`: only `a.rst:1`
    /// errors).
    #[test]
    fn the_first_read_starts_from_the_fresh_registries() {
        let config = BuildConfig {
            rst_prolog: Some(".. role:: red\n\n.. default-role:: math\n".to_string()),
            ..BuildConfig::default()
        };
        let files = [
            (
                "index.rst",
                ":author: :red:`me` `a^2`\n\nIndex\n=====\n\n.. toctree::\n\n   a\n   b\n\n\
                 Index :red:`i` `x`.\n",
            ),
            (
                "a.rst",
                ":author: :red:`me` `a^2`\n\nA\n=\n\nEarly :red:`x`.\n",
            ),
            (
                "b.rst",
                ":author: :red:`me` `b^2`\n\nB\n=\n\nLater :red:`y`.\n",
            ),
        ];
        for jobs in [4, 1] {
            let (_, warnings, reparsed) = build_roles_project_with(config.clone(), &files, jobs);
            assert_eq!(
                warnings,
                [
                    "Unknown interpreted text role \"red\".",
                    "Bibliographic field \"author\"\nmust contain a single <paragraph>, not a \
                     <paragraph>.",
                ],
                "-j {jobs}"
            );
            // `a` read first, from the fresh registries, and the others
            // from the baseline: the parallel parses stand.
            assert_eq!(reparsed, 0, "-j {jobs}");
        }
    }

    /// Decision 14's schedule as a unit ([`ReadSchedule::plan`]): the
    /// parallel parses stand while each document started from the
    /// registry Sphinx's read in sorted docname order gives it, the fresh
    /// one for the first and the one the document before left for every
    /// other; otherwise the first that did not, and every document after
    /// it, are read again. And the build's re-read threads the registry
    /// from document to document: `c` uses `blue`, which `b` defines over
    /// `a`'s `red`, so `b` must be read from `a`'s registry and `c` from
    /// `b`'s.
    #[test]
    fn read_schedule_follows_role_table_changes() {
        use crate::rst::roles::{RoleFn, RoleTable};
        let fresh = RoleTable::sphinx();
        let mut red = RoleTable::sphinx();
        red.register_local("red", RoleFn::GenericCustom);
        let mut math = RoleTable::sphinx();
        math.set_default(Some(RoleFn::Math));
        let plan =
            |starts: &[&RoleTable], after: &[&RoleTable]| ReadSchedule::plan(&fresh, starts, after);
        assert_eq!(plan(&[], &[]), ReadSchedule::Parallel);
        assert_eq!(
            plan(&[&fresh; 3], &[&fresh, &fresh, &fresh]),
            ReadSchedule::Parallel
        );
        assert_eq!(
            plan(&[&fresh; 4], &[&fresh, &red, &red, &math]),
            ReadSchedule::Reparse { from: 2 }
        );
        assert_eq!(
            plan(&[&fresh; 2], &[&math, &math]),
            ReadSchedule::Reparse { from: 1 }
        );
        // What the last document leaves reaches no document of this read.
        assert_eq!(plan(&[&fresh; 2], &[&fresh, &red]), ReadSchedule::Parallel);
        // The first document read starts from the fresh registry: one that
        // started from another is read again, and every document after it.
        assert_eq!(
            plan(&[&red, &red], &[&red, &red]),
            ReadSchedule::Reparse { from: 0 }
        );
        // `rst_prolog`'s own definitions: every document makes them again,
        // so each one after the first starts from the registry they leave
        // (the build's baseline), and the parallel parses stand.
        let mut prolog = RoleTable::sphinx();
        prolog.register_local("button", RoleFn::GenericCustom);
        assert_eq!(
            plan(&[&fresh, &prolog, &prolog], &[&prolog, &prolog, &prolog]),
            ReadSchedule::Parallel
        );

        let files = [
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   c\n",
            ),
            ("a.rst", "A\n=\n\n.. role:: red\n"),
            ("b.rst", "B\n=\n\n.. role:: blue(red)\n"),
            ("c.rst", "C\n=\n\nUse :blue:`z`.\n"),
        ];
        for jobs in [4, 1] {
            let (resolved, warnings) = build_roles_project(&files, jobs);
            assert_eq!(warnings, Vec::<String>::new(), "-j {jobs}");
            assert_eq!(
                paragraph(&resolved, "c")[..3],
                [
                    "            Use ",
                    "            <inline classes=\"blue\">",
                    "                z"
                ],
                "-j {jobs}"
            );
        }
    }

    /// Decision 14 parses a document again when one read before it changed
    /// the role registries, and the cache stores each parsed document once,
    /// as the schedule left it: here `a` changes them, so `b`, `c` and
    /// `index` are parsed twice and stored once each, four stores in all.
    /// A warm build then recovers all four.
    #[test]
    fn a_reparsed_document_is_stored_once() {
        let files = [
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   c\n",
            ),
            ("a.rst", "A\n=\n\n.. role:: red\n"),
            ("b.rst", "B\n=\n\nUse :red:`b`.\n"),
            ("c.rst", "C\n=\n\nUse :red:`c`.\n"),
        ];
        for jobs in [4, 1] {
            let tmp = TempDir::new().unwrap();
            let source_dir = tmp.path().join("src");
            std::fs::create_dir_all(&source_dir).unwrap();
            for (path, text) in files {
                std::fs::write(source_dir.join(path), text).unwrap();
            }
            let build = || {
                let config = BuildConfig {
                    validate_directives: false,
                    ..BuildConfig::default()
                };
                let mut builder =
                    SphinxBuilder::new(config, source_dir.clone(), tmp.path().join("out")).unwrap();
                builder.set_parallel_jobs(jobs);
                builder.enable_incremental();
                let stats = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(builder.build())
                    .unwrap();
                (stats.cache_hits, builder.cache.store_count())
            };
            assert_eq!(build(), (0, 4), "cold, -j {jobs}");
            assert_eq!(build(), (4, 0), "warm, -j {jobs}");
        }
    }

    /// `-W` and `-n` steer diagnostics, not content: toggling either must
    /// leave the fingerprint — and therefore the whole cache directory —
    /// alone. Sphinx cannot invalidate on either (`config.py:272` gives
    /// `nitpicky` rebuild class `''`; `warningiserror` is not a `Config`
    /// value), so neither may invalidate here.
    #[test]
    fn operational_flags_are_excluded_from_the_fingerprint() {
        let base = BuildConfig::default();
        let baseline = config_fingerprint(&base).unwrap();

        let mut warned = base.clone();
        warned.fail_on_warning = true;
        assert_eq!(config_fingerprint(&warned).unwrap(), baseline, "-W");

        let mut nitpicky = base.clone();
        nitpicky.nitpicky = true;
        assert_eq!(config_fingerprint(&nitpicky).unwrap(), baseline, "-n");

        let mut both = base.clone();
        both.fail_on_warning = true;
        both.nitpicky = true;
        assert_eq!(config_fingerprint(&both).unwrap(), baseline, "-W -n");
    }

    /// The filter is exactly two keys wide. Everything else — `tags`
    /// included, because tags select `only::` branches and so change the
    /// output — still invalidates.
    #[test]
    fn content_bearing_config_still_changes_the_fingerprint() {
        let base = BuildConfig::default();
        let baseline = config_fingerprint(&base).unwrap();

        let mut tagged = base.clone();
        tagged.tags = vec!["draft".to_string()];
        assert_ne!(config_fingerprint(&tagged).unwrap(), baseline, "tags");

        // `source_encoding` is rebuild class `'env'` (`config.py:244`): a
        // change must re-read every document.
        let mut encoded = base.clone();
        encoded.source_encoding = "latin-1".to_string();
        assert_ne!(
            config_fingerprint(&encoded).unwrap(),
            baseline,
            "source_encoding"
        );

        // So is `source_suffix` (`config.py:243`), whose order is data: it
        // decides which suffix `doc2path` appends.
        let suffixed = |suffixes: serde_json::Value| BuildConfig {
            source_suffix: crate::config::SourceSuffix::from_conf_value(&suffixes).unwrap(),
            ..base.clone()
        };
        let rst_txt = config_fingerprint(&suffixed(serde_json::json!([".rst", ".txt"]))).unwrap();
        let txt_rst = config_fingerprint(&suffixed(serde_json::json!([".txt", ".rst"]))).unwrap();
        assert_ne!(rst_txt, baseline, "source_suffix");
        assert_ne!(rst_txt, txt_rst, "source_suffix order");

        // ...while the config-inited diagnostic record is not configuration
        // at all, and must not invalidate anything.
        let mut mismatched = base.clone();
        mismatched.note_confval_type_mismatch("maximum_signature_line_length", "str");
        assert_eq!(
            config_fingerprint(&mismatched).unwrap(),
            baseline,
            "confval_type_mismatches"
        );
        // A prolog Sphinx's reader crashes on, which the typed field reads
        // as `None`, changes every read: Sphinx re-reads every document
        // when it changes, so the cache must miss.
        let mut crashing = base.clone();
        crashing.decoration_crashes = vec![("rst_prolog".to_string(), "int".to_string())];
        assert_ne!(
            config_fingerprint(&crashing).unwrap(),
            baseline,
            "decoration_crashes"
        );

        let mut nitpick_ignore = base.clone();
        nitpick_ignore.nitpick_ignore = vec![("ref".to_string(), "x".to_string())];
        assert_ne!(
            config_fingerprint(&nitpick_ignore).unwrap(),
            baseline,
            "nitpick_ignore is data, not an operational flag"
        );

        let mut numfig = base.clone();
        numfig.numfig = true;
        assert_ne!(config_fingerprint(&numfig).unwrap(), baseline, "numfig");

        // The read-side keys of wave 5a T17a are rebuild class `'env'`
        // (`config.py:247`, `:271`, `:282-285`).
        for (key, changed) in [
            (
                "default_role",
                BuildConfig {
                    default_role: Some("any".to_string()),
                    ..base.clone()
                },
            ),
            (
                "manpages_url",
                BuildConfig {
                    manpages_url: Some("https://man/{page}".to_string()),
                    ..base.clone()
                },
            ),
            (
                "math_number_all",
                BuildConfig {
                    math_number_all: true,
                    ..base.clone()
                },
            ),
            (
                "math_eqref_format",
                BuildConfig {
                    math_eqref_format: Some("Eq. {number}".to_string()),
                    ..base.clone()
                },
            ),
            (
                "math_numfig",
                BuildConfig {
                    math_numfig: false,
                    ..base.clone()
                },
            ),
            (
                "math_numsep",
                BuildConfig {
                    math_numsep: "-".to_string(),
                    ..base.clone()
                },
            ),
            // T17b's keys (`config.py:265-266`).
            (
                "rst_prolog",
                BuildConfig {
                    rst_prolog: Some(".. |a| replace:: A\n".to_string()),
                    ..base.clone()
                },
            ),
            (
                "rst_epilog",
                BuildConfig {
                    rst_epilog: Some(String::new()),
                    ..base.clone()
                },
            ),
        ] {
            assert_ne!(config_fingerprint(&changed).unwrap(), baseline, "{key}");
        }

        // Both warning-stream keys are rebuild class `'env'`
        // (`config.py:262-263`): a warm build replays no read-phase record,
        // so a change must re-read for the stream to reflect it.
        let mut suppressed = base.clone();
        suppressed.suppress_warnings = vec!["docutils".to_string()];
        assert_ne!(
            config_fingerprint(&suppressed).unwrap(),
            baseline,
            "suppress_warnings"
        );
        let mut untyped = base.clone();
        untyped.show_warning_types = false;
        assert_ne!(
            config_fingerprint(&untyped).unwrap(),
            baseline,
            "show_warning_types"
        );

        // `keep_warnings` is rebuild class `'env'` (`config.py:261`): it
        // decides which messages the persisted doctree keeps.
        let mut kept = base.clone();
        kept.keep_warnings = true;
        assert_ne!(
            config_fingerprint(&kept).unwrap(),
            baseline,
            "keep_warnings"
        );

        // `trim_doctest_flags` is rebuild class `'env'` (`config.py:267`),
        // though the post-transform that reads it runs in the resolve
        // pass.
        let mut untrimmed = base.clone();
        untrimmed.trim_doctest_flags = false;
        assert_ne!(
            config_fingerprint(&untrimmed).unwrap(),
            baseline,
            "trim_doctest_flags"
        );

        // The smart-quotes keys are rebuild class `'env'`
        // (`config.py:289-295`): SmartQuotes (750) runs in the read.
        let mut straight = base.clone();
        straight.smartquotes = false;
        assert_ne!(
            config_fingerprint(&straight).unwrap(),
            baseline,
            "smartquotes"
        );
        let mut action = base.clone();
        action.smartquotes_action = "q".to_string();
        assert_ne!(
            config_fingerprint(&action).unwrap(),
            baseline,
            "smartquotes_action"
        );
        let mut excludes = base.clone();
        excludes.smartquotes_excludes.builders =
            crate::config::ExcludedNames::Text("html".to_string());
        assert_ne!(
            config_fingerprint(&excludes).unwrap(),
            baseline,
            "smartquotes_excludes"
        );

        // `today` and `today_fmt` are rebuild class `'env'`
        // (`config.py:227-229`): `|today|` is substituted in the read.
        let mut today = base.clone();
        today.today = "April 1".to_string();
        assert_ne!(config_fingerprint(&today).unwrap(), baseline, "today");
        let mut today_fmt = base.clone();
        today_fmt.today_fmt = Some("%Y".to_string());
        assert_ne!(
            config_fingerprint(&today_fmt).unwrap(),
            baseline,
            "today_fmt"
        );
    }

    /// Serializing through `serde_json::Value` sorts every map, so a
    /// multi-key `html_context` hashes the same on every call — the
    /// property that keeps the cache from being wiped on every build.
    #[test]
    fn multi_key_html_context_fingerprints_stably() {
        let mut config = BuildConfig::default();
        for key in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            config.html_context.insert(
                key.to_string(),
                serde_json::Value::String(key.to_uppercase()),
            );
        }

        let first = config_fingerprint(&config).unwrap();
        for _ in 0..8 {
            assert_eq!(config_fingerprint(&config).unwrap(), first);
        }

        // ...and it is still sensitive to the contents.
        let mut changed = config.clone();
        changed
            .html_context
            .insert("a".to_string(), serde_json::Value::String("Z".to_string()));
        assert_ne!(config_fingerprint(&changed).unwrap(), first);
    }

    /// Row 9 of the include checklist: the parse-time records replay
    /// into `env.included`/`env.dependencies` on merge, and the orphan
    /// warning consults `env.included` (src/env/toctree.rs check) — a doc
    /// reachable only through an `include` stays silent while a genuinely
    /// unlinked one still warns.
    #[test]
    fn include_records_replay_into_the_environment_and_suppress_the_orphan() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("index.rst"),
            "Index\n=====\n\n.. toctree::\n\n   a\n",
        )
        .unwrap();
        std::fs::write(
            source_dir.join("a.rst"),
            "A\n=\n\n.. include:: part.rst\n\n.. include:: snippet.txt\n",
        )
        .unwrap();
        std::fs::write(source_dir.join("part.rst"), "part para\n").unwrap();
        std::fs::write(source_dir.join("snippet.txt"), "plain snippet\n").unwrap();
        std::fs::write(
            source_dir.join("not_linked.rst"),
            "Not Linked\n==========\n\nOrphan candidate.\n",
        )
        .unwrap();

        let (stats, builder) = build_incrementally(&source_dir, &output_dir);

        // Canonicalized like the builder's own source_dir.
        let src = crate::utils::canonicalize_simplified(&source_dir).unwrap();
        assert_eq!(
            builder.env.included.get("a"),
            Some(&std::collections::BTreeSet::from(["part".to_string()])),
            "only the docname-mapping include registers (snippet.txt maps to no docname)"
        );
        assert_eq!(
            builder
                .env
                .dependencies
                .get("a")
                .map(|set| set.iter().cloned().collect::<Vec<_>>()),
            Some(vec![src.join("part.rst"), src.join("snippet.txt")]),
            "every opened include target is a dependency, the non-doc file too"
        );

        let orphan_warnings: Vec<String> = stats
            .warning_details
            .iter()
            .filter(|w| w.message.contains("isn't included in any toctree"))
            .map(|w| match &w.location {
                crate::diagnostics::Location::Path { path, line: None } => path.clone(),
                other => panic!("an orphan is located at its document: {other:?}"),
            })
            .collect();
        assert_eq!(
            orphan_warnings.len(),
            1,
            "exactly the genuinely unlinked doc warns: {orphan_warnings:?}"
        );
        assert!(
            orphan_warnings[0].ends_with("not_linked.rst"),
            "{orphan_warnings:?}"
        );
    }

    #[test]
    fn every_read_document_persists_its_doctree() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_stats, builder) = build_incrementally(&source_dir, &output_dir);

        for docname in ["index", "a"] {
            let path = builder.doctree_path(docname);
            assert!(
                path.is_file(),
                "{docname}: no doctree at {}",
                path.display()
            );
            let doctree = builder.load_doctree(docname).expect("doctree decodes");
            assert_eq!(doctree.root.kind, crate::doctree::kinds::DOCUMENT);
        }
        assert!(
            builder.cache.cache_dir().join("env.bin").is_file(),
            "the resolve phase must persist the environment"
        );
    }

    #[test]
    fn cache_hit_whose_doctree_is_missing_is_treated_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (first, builder) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(first.cache_hits, 0, "cold build cannot hit the cache");

        // Warm: both documents come from the cache.
        let (warm, _) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(warm.cache_hits, 2);

        // Delete one document's doctree. Its cache entry is still valid, but
        // unusable — the build must re-read the file rather than pretend.
        std::fs::remove_file(builder.doctree_path("a")).unwrap();
        let (degraded, rebuilt) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(
            degraded.cache_hits, 1,
            "a document whose doctree is gone is a cache miss, not a hit"
        );
        assert!(
            rebuilt.doctree_path("a").is_file(),
            "the re-read must persist the doctree it just produced"
        );
        assert_eq!(degraded.errors, 0);

        // And the environment is complete either way.
        let env = rebuilt.env();
        assert_eq!(env.all_docs.len(), 2);
        assert!(env.tocs.contains_key("a"));
    }

    #[test]
    fn persisted_doctrees_carry_the_format_version_header() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_stats, builder) = build_incrementally(&source_dir, &output_dir);

        let bytes = std::fs::read(builder.doctree_path("index")).unwrap();
        assert_eq!(
            &bytes[..DOCTREE_MAGIC.len()],
            DOCTREE_MAGIC,
            "a persisted doctree must be self-identifying"
        );
        let version = u32::from_le_bytes(
            bytes[DOCTREE_MAGIC.len()..DOCTREE_MAGIC.len() + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(version, DOCTREE_FORMAT_VERSION);
        assert_eq!(
            builder.load_doctree("index").unwrap().root.kind,
            crate::doctree::kinds::DOCUMENT,
            "the header must not disturb the round trip"
        );
    }

    #[test]
    fn a_doctree_written_in_the_unversioned_format_is_treated_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, builder) = build_incrementally(&source_dir, &output_dir);

        // Exactly what the pre-versioning builder wrote: a bare bincode
        // blob. It still *decodes* — which is the trap: an old blob whose
        // attribute shapes have since changed decodes into a plausible
        // doctree and then mis-harvests. The version word is what makes it
        // a miss instead.
        let doctree = builder.load_doctree("index").expect("doctree decodes");
        std::fs::write(
            builder.doctree_path("index"),
            crate::doctree::to_bincode(&doctree),
        )
        .unwrap();
        assert!(
            builder.load_doctree("index").is_none(),
            "an unversioned blob must not be trusted"
        );

        let (stats, rebuilt) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(
            stats.cache_hits, 1,
            "the document whose doctree is stale must be re-read"
        );
        assert!(rebuilt.load_doctree("index").is_some());
    }

    #[test]
    fn a_doctree_from_a_future_format_version_is_treated_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, builder) = build_incrementally(&source_dir, &output_dir);
        let doctree = builder.load_doctree("index").expect("doctree decodes");

        let mut bytes = Vec::from(DOCTREE_MAGIC);
        bytes.extend_from_slice(&(DOCTREE_FORMAT_VERSION + 1).to_le_bytes());
        bytes.extend_from_slice(&crate::doctree::to_bincode(&doctree));
        std::fs::write(builder.doctree_path("index"), bytes).unwrap();

        assert!(builder.load_doctree("index").is_none());
    }

    #[test]
    fn corrupt_doctree_file_is_treated_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, builder) = build_incrementally(&source_dir, &output_dir);
        std::fs::write(builder.doctree_path("index"), b"not a doctree").unwrap();

        let (stats, rebuilt) = build_incrementally(&source_dir, &output_dir);
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.errors, 0);
        assert!(rebuilt.load_doctree("index").is_some());
    }

    /// The cache directory is optional infrastructure: a build whose
    /// environment cannot be written still produced valid output, and
    /// saying "build failed" over it would be a lie. (A directory where
    /// `env.bin` belongs is the portable way to make exactly that one write
    /// fail while every other cache write succeeds.)
    #[test]
    fn a_build_whose_environment_cannot_be_saved_still_writes_its_output() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, builder) = build_incrementally(&source_dir, &output_dir);
        let env_file = builder.cache.cache_dir().join("env.bin");
        std::fs::remove_file(&env_file).unwrap();
        std::fs::create_dir(&env_file).unwrap();

        let (stats, rebuilt) = build_incrementally(&source_dir, &output_dir);

        assert_eq!(stats.errors, 0, "an unsaveable environment is not an error");
        assert!(
            output_dir.join("index.html").is_file() && output_dir.join("a.html").is_file(),
            "the pages this build produced are still written"
        );
        assert_eq!(
            rebuilt.env().all_docs.len(),
            2,
            "the in-memory environment is complete; only its persistence failed"
        );
    }

    /// `-E`: the persisted environment is not to be trusted, and neither is
    /// the half of it already sitting in memory.
    #[test]
    fn fresh_env_discards_the_loaded_environment_and_re_reads_everything() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        build_incrementally(&source_dir, &output_dir);

        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.clone(),
            output_dir.clone(),
        )
        .unwrap();
        builder.enable_incremental();
        assert_eq!(
            builder.env().all_docs.len(),
            2,
            "the builder loads the saved environment"
        );

        builder.fresh_env().unwrap();
        assert!(
            builder.env().all_docs.is_empty(),
            "-E starts from an empty environment, not the loaded one"
        );

        let stats = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(builder.build())
            .unwrap();
        assert_eq!(stats.cache_hits, 0, "every document is new again");
        assert_eq!(stats.files_skipped, 0);
        assert_eq!(builder.env().all_docs.len(), 2);
    }

    /// A build with the document cache off reads every document — which is
    /// what `sphinx-build -a` maps to here.
    #[test]
    fn a_non_incremental_build_reads_every_document() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        build_incrementally(&source_dir, &output_dir);

        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.clone(),
            output_dir.clone(),
        )
        .unwrap();
        let stats = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(builder.build())
            .unwrap();

        assert_eq!(stats.cache_hits, 0);
        assert_eq!(stats.files_skipped, 0, "nothing was skipped: all was read");
        assert!(output_dir.join("index.html").is_file() && output_dir.join("a.html").is_file());
    }

    #[test]
    fn cache_hit_still_writes_output_and_fills_the_environment() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let output_dir = tmp.path().join("build");
        write_project(&source_dir);

        let (_cold, _) = build_incrementally(&source_dir, &output_dir);
        std::fs::remove_file(output_dir.join("index.html")).unwrap();
        std::fs::remove_file(output_dir.join("a.html")).unwrap();

        let (warm, builder) = build_incrementally(&source_dir, &output_dir);

        assert_eq!(warm.cache_hits, 2);
        assert!(output_dir.join("index.html").is_file());
        assert!(output_dir.join("a.html").is_file());
        assert_eq!(
            builder.env().toctree_includes.get("index"),
            Some(&vec!["a".to_string()]),
            "a fully cached build still rebuilds the environment from the \
             persisted doctrees"
        );
    }

    /// The `config:` hash real `sphinx-build` 9.1.0 writes to `.buildinfo`
    /// for one-setting `conf.py` files, pinned here so the html-category
    /// table cannot drift from Sphinx's defaults or from the literal-vs-
    /// default rule. The `sphinx_rtd_theme` value is from
    /// `BuildInfo(app.config, app.tags, {'html'})` over the same config,
    /// because Sphinx cannot load that theme in the oracle environment.
    /// `stable_hash` hashes `str()` of a float, its `repr()`: `1e+16`,
    /// `1e-05`, `1.5e+17`, `1e-07`, `1.0`, whether the literal spells its
    /// exponent with a sign or not.
    #[test]
    fn buildinfo_config_hash_matches_sphinx_for_configured_values() {
        let cases = [
            ("project = 'Fixture'\n", "bc9d5af5c9b6d45a25e31a5ff598283a"),
            (
                "project = 'Fixture'\nhtml_theme = 'alabaster'\n",
                "bc9d5af5c9b6d45a25e31a5ff598283a",
            ),
            (
                "project = 'Fixture'\nhtml_theme = 'classic'\n",
                "58dd99f3a17211a74d9b5a57b3e09abd",
            ),
            (
                "project = 'Fixture'\nhtml_theme = 'sphinx_rtd_theme'\n",
                "dbf973fabe50a41cd3aba49a63addfb0",
            ),
            (
                "project = 'Fixture'\nhtml_css_files = ['a.css']\n",
                "05a38004493ea6ed2731ff1f46a9dbea",
            ),
            (
                "project = 'Fixture'\nhtml_css_files = [('a.css', {'media': 'print'})]\n",
                "2baebae26e35eb22fb7aa2e2d1aa9597",
            ),
            (
                "project = 'Fixture'\nhtml_js_files = ['b.js']\n",
                "0e188cc551c4d963314c65091a73a387",
            ),
            (
                "project = 'Fixture'\nhtml_static_path = ['_static']\n",
                "199d55036ccc652911c8d7544403296b",
            ),
            (
                "project = 'Fixture'\ntemplates_path = ['_templates']\n",
                "dd8472423e919d99edb63ef88589b4d1",
            ),
            (
                "project = 'Fixture'\nhtml_theme_options = {'nosidebar': True}\n",
                "a2a03cb33b0b36892e5a3836146f8e60",
            ),
            (
                "project = 'Fixture'\nhtml_context = {'a': 1e16, 'b': 0.00001}\n",
                "331b237fdfa68d15a92ce0941673f23f",
            ),
            (
                "project = 'Fixture'\nhtml_theme_options = {'w': 1.5e17, 'x': 0.0000001, 'y': 1.0}\n",
                "720f1792bff65e847089cadb967c5a1d",
            ),
            (
                "project = 'Fixture'\nhtml_context = {'a': 1e-05, 'b': -1e-07, 'c': 1e+16}\n",
                "f61e44fa098222f240ecbdf89dc96a4d",
            ),
            (
                "project = 'Fixture'\nhtml_theme_options = {'w': 1.5e17, 'x': -1e-07, 'y': 1.0}\n",
                "44dc13739b758fbdb1b29f3ea837b267",
            ),
            (
                "project = 'Fixture'\nhtml_last_updated_fmt = '%Y'\n",
                "7106224b6b475e01e49bd6989dc2fa29",
            ),
            (
                "project = 'Fixture'\nhtml_use_opensearch = 'https://example.org'\n",
                "0acadda62c6d2f4f946d7a0fa145cd53",
            ),
            (
                "project = 'My.Project v2'\n",
                "ed09fc06b53dbc75b8ac7d282178aaab",
            ),
            (
                "project = 'Fixture'\nmodindex_common_prefix = ['probe.']\n",
                "0652c60ff2bdad1e6cafdc981c25594b",
            ),
            (
                "project = 'Fixture'\nhtml_logo = 'logo.png'\nhtml_favicon = 'fav.ico'\n\
                 html_title = 'Custom Title'\nhtml_short_title = 'Short'\n\
                 html_show_sphinx = False\nhtml_copy_source = False\n\
                 copyright = '2026, Me'\nversion = '1.2'\nrelease = '1.2.3'\n",
                "5b84a1185cee8c88c692fc47d88b91dc",
            ),
        ];
        let dir = tempfile::tempdir().unwrap();
        for (conf, expected) in cases {
            std::fs::write(dir.path().join("conf.py"), conf).unwrap();
            let parsed = crate::python_config::PythonConfigParser::new()
                .unwrap()
                .parse_conf_py(dir.path().join("conf.py"))
                .unwrap();
            let mut config = parsed.to_build_config().unwrap();
            // The driver's `config-inited` step: the probe directory holds
            // no logo or favicon file, so both settings fall back to `None`
            // exactly as they did in the oracle run.
            config.validate_html_logo_and_favicon(dir.path());
            let contents = sphinx_build_info_contents(&config, &config.tags).unwrap();
            let line = contents
                .lines()
                .find(|line| line.starts_with("config: "))
                .unwrap();
            assert_eq!(&line["config: ".len()..], expected, "{conf}");
        }
    }

    #[test]
    fn make_filename_strips_like_sphinx() {
        assert_eq!(make_filename("My.Project v2"), "MyProjectv2");
        assert_eq!(make_filename("snake_case-ok"), "snake_case-ok");
        assert_eq!(make_filename("¡!"), "sphinx");
    }
}
