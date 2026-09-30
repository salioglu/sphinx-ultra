//! Differential test: our environment layer vs the ENVIRONMENT ORACLE --
//! `BuildEnvironment` state (toctree graph, relations, section and figure
//! numbering, std-domain registries, index entries, genindex, and fully
//! cross-reference-resolved doctrees) a real `sphinx-build` 9.1.0 read +
//! resolve phase produces for the committed multi-document project corpus.
//!
//! Regenerate the fixture (manual, never in CI):
//!     PYTHONNOUSERSITE=1 uv run --python 3.12 --with 'sphinx==9.1.0' \
//!         --with 'docutils==0.22.4' \
//!         python tools/gen_env_fixture.py
//!
//! Each `expect` key group has exactly one test. A group whose library
//! surface doesn't exist yet stays `#[ignore]`d with the wave-4 task that
//! will build it; un-ignoring one is the signal that its keys are now
//! covered. **Every** `expect` key is now live (tasks 5-10):
//! `tocs_pformat`, `toc_num_entries`, `toctree_includes`,
//! `files_to_rebuild`, `relations`, `toc_secnumbers`, `toc_fignumbers`,
//! `std`, `index_entries`, `genindex`, `resolved_pformat` and
//! `warning_records` (which supersedes the line-split `warnings`) —
//! the last three with strict, self-cleaning exemption tables
//! ([`KNOWN_STD_GAPS`], [`KNOWN_RESOLVED_GAPS`], [`KNOWN_WARNING_GAPS`])
//! naming what each remaining divergence waits on.
//!
//! Each live test materializes every fixture project into a tempdir, runs
//! the real library build (read + merge + resolve), and diffs
//! `SphinxBuilder::snapshot_env()` (plus the build's warnings) against the
//! oracle. The fixture's `conf` overrides are applied through
//! `BuildConfig::apply_override` (sphinx-build's `-D`), except the ones
//! [`KNOWN_INERT_CONF`] proves steer nothing this crate implements — so a
//! future fixture project carrying a behavior-relevant key that the harness
//! cannot express fails here instead of silently comparing against a
//! differently configured oracle.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use sphinx_ultra::error::BuildWarning;
use sphinx_ultra::{BuildConfig, SphinxBuilder};

/// `[parent, prev, next]` as recorded by `env.collect_relations()`.
pub type RelationsEntry = (Option<String>, Option<String>, Option<String>);

/// `(entry_type, value, target_id, main, category_key)` as recorded in
/// `env.domaindata['index']['entries']`.
pub type IndexEntryTuple = (String, String, String, String, Option<String>);

#[derive(serde::Deserialize)]
pub struct Fixture {
    pub sphinx_version: String,
    pub docutils_version: String,
    #[allow(dead_code)]
    pub generator: String,
    pub projects: Vec<Project>,
}

#[derive(serde::Deserialize)]
pub struct Project {
    pub name: String,
    /// Full confoverrides (BASE_CONFOVERRIDES merged with the project's own
    /// extras, e.g. numfig/numfig_secnum_depth/numfig_format) as passed to
    /// `SphinxTestApp`. Left as an untyped JSON value: the value shapes vary
    /// per key (bool/int/dict) and no task yet needs to deserialize it
    /// structurally.
    #[allow(dead_code)]
    pub conf: serde_json::Value,
    /// docname -> rst source. Nested docnames (e.g. "sub/b") map to
    /// "sub/b.rst" when materialized.
    pub files: BTreeMap<String, String>,
    /// relative path -> literal text of a non-document member (the `.py` /
    /// `.inc` / `.png` files the include and literalinclude projects read),
    /// written verbatim beside the sources. Never `.rst`/`.md`/`.txt`: this
    /// crate's discovery is wider than Sphinx's default `source_suffix`, so
    /// a `.txt` member would become a document on our side only.
    #[serde(default)]
    pub data_files: BTreeMap<String, String>,
    pub expect: Expect,
}

#[derive(serde::Deserialize)]
pub struct Expect {
    pub toctree_includes: BTreeMap<String, Vec<String>>,
    pub files_to_rebuild: BTreeMap<String, Vec<String>>,
    /// `env.collect_relations()` -> `{docname: [parent, prev, next]}`.
    /// `None` for the one project (`toctree_circular`) where a genuine
    /// multi-doc toctree cycle makes the real sphinx 9.1.0 attribute
    /// uncomputable (`_traverse_toctree` recurses without bound for a
    /// mutual A<->B cycle; see tools/gen_env_fixture.py for the verified
    /// RecursionError and rationale).
    pub relations: Option<BTreeMap<String, RelationsEntry>>,
    pub tocs_pformat: BTreeMap<String, String>,
    pub toc_num_entries: BTreeMap<String, u32>,
    pub toc_secnumbers: BTreeMap<String, BTreeMap<String, Vec<u32>>>,
    pub toc_fignumbers: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<u32>>>>,
    pub std: StdData,
    /// docname -> list of (entry_type, value, target_id, main, category_key).
    pub index_entries: BTreeMap<String, Vec<IndexEntryTuple>>,
    pub genindex: Vec<GenIndexGroup>,
    /// `domaindata['py']['objects']` as records in REGISTRATION order —
    /// the order is oracle data (Sphinx's fuzzy resolution iterates it),
    /// so unlike the std lists it is never sorted on either side.
    pub py_objects: Vec<PyObjectExpect>,
    /// `domaindata['py']['modules']`, registration-ordered like
    /// [`Self::py_objects`].
    pub py_modules: Vec<PyModuleExpect>,
    /// `PythonModuleIndex(py_domain).generate()` — the exact
    /// `(content, collapse)` tuples the py-modindex page renders.
    pub py_modindex: PyModindexExpect,
    /// `env.dependencies`: docname -> the files the document is built from
    /// besides its own source, `<project>`-normalized and sorted. Only
    /// documents with dependencies appear.
    pub dependencies: BTreeMap<String, Vec<String>>,
    /// `env.included`: docname -> the docnames it textually includes
    /// (`note_included`), values sorted.
    pub included: BTreeMap<String, Vec<String>>,
    pub resolved_pformat: BTreeMap<String, String>,
    /// The build's warning stream, one entry per printed RECORD (a
    /// multi-line docutils message is one entry, blank lines kept), in
    /// print order. The fixture's older `warnings` key is this stream split
    /// into non-blank lines — the generator asserts exactly that — so it
    /// is not read: it cannot tell two records from one.
    pub warning_records: Vec<String>,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct PyObjectExpect {
    pub name: String,
    pub docname: String,
    pub node_id: String,
    pub objtype: String,
    pub aliased: bool,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct PyModuleExpect {
    pub name: String,
    pub docname: String,
    pub node_id: String,
    pub synopsis: String,
    pub platform: String,
    pub deprecated: bool,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct PyModindexExpect {
    pub collapse: bool,
    pub groups: Vec<PyModindexGroup>,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct PyModindexGroup {
    pub letter: String,
    pub entries: Vec<PyModindexEntry>,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct PyModindexEntry {
    pub name: String,
    pub subtype: u8,
    pub docname: String,
    pub anchor: String,
    pub extra: String,
    pub qualifier: String,
    pub descr: String,
}

#[derive(serde::Deserialize)]
pub struct StdData {
    /// labelname -> (docname, labelid, sectionname). Includes the
    /// preseeded virtual labels (genindex/modindex/py-modindex/search) --
    /// see tools/gen_env_fixture.py docstring: these are part of the real
    /// oracle contract, not filtered out.
    pub labels: BTreeMap<String, (String, String, String)>,
    /// labelname -> (docname, labelid).
    pub anonlabels: BTreeMap<String, (String, String)>,
    pub objects: Vec<StdObjectEntry>,
    pub progoptions: Vec<StdProgOptionEntry>,
    /// lowercased term -> (docname, labelid).
    pub terms: BTreeMap<String, (String, String)>,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct StdObjectEntry {
    pub objtype: String,
    pub name: String,
    pub docname: String,
    pub labelid: String,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct StdProgOptionEntry {
    pub program: Option<String>,
    pub name: String,
    pub docname: String,
    pub labelid: String,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct GenIndexGroup {
    pub group: String,
    pub entries: Vec<GenIndexEntry>,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct GenIndexEntry {
    pub name: String,
    /// (main_flag, uri) pairs; main_flag is the literal sphinx string
    /// `"main"` or `""`, not a bool.
    pub targets: Vec<(String, String)>,
    pub subitems: Vec<GenIndexSubItem>,
    pub category_key: Option<String>,
}

#[derive(serde::Deserialize, Debug, PartialEq, Eq)]
pub struct GenIndexSubItem {
    pub name: String,
    pub targets: Vec<(String, String)>,
}

fn load_fixture() -> Fixture {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/env_differential.json"
    ));
    serde_json::from_str(raw).expect("env_differential.json fixture parses")
}

// ---------------------------------------------------------------------------
// Running the library build over a fixture project
// ---------------------------------------------------------------------------

/// One project's build output: the environment snapshot plus every warning
/// the build emitted, rendered the way `sphinx-build` renders them (source
/// paths replaced by the fixture's `<project>` placeholder).
struct Built {
    env: serde_json::Value,
    warnings: Vec<String>,
}

/// The fixture's stand-in for a project's source root.
const PROJECT: &str = "<project>";

/// Rewrite `root`-rooted absolute paths in `rendered` to [`PROJECT`], and
/// spell what follows the placeholder with forward slashes.
///
/// The oracle is generated on POSIX, so its expected strings read
/// `<project>/sub/page.rst`. On Windows the *product* renders the same
/// warning with native separators — `<project>\sub\page.rst` — and that is
/// correct: `sphinx-build` on Windows prints native separators too. The
/// difference is a property of the comparison, not of the build, so it is
/// normalized here.
///
/// Only placeholder-rooted segments are rewritten, and each segment stops
/// at the first character that cannot continue a path here: the `:` before
/// a line number, whitespace, a quote, or a comma. A blanket backslash
/// replacement would corrupt warning *text*, which legitimately carries
/// backslashes (RST escapes, a repr'd index entry, …).
///
/// Operates on decoded strings only — a rendered warning, or one string
/// *inside* a parsed snapshot. Never run it over JSON text: there a `\`
/// may open an escape sequence rather than separate two path components.
fn normalize_source_paths(rendered: &str, root: &str) -> String {
    // A path that reached the string through Python `repr()` carries every
    // separator DOUBLED — `_StrPath('C:\\proj\\example.py')` — so the
    // escaped spelling of the root is a second thing to rewrite, and the
    // tail below has to undo the doubling before it flips separators. On
    // POSIX both spellings of the root are the same string and both passes
    // are no-ops.
    let escaped_root = root.replace('\\', r"\\");
    let replaced = rendered
        .replace(&escaped_root, PROJECT)
        .replace(root, PROJECT);

    let mut out = String::with_capacity(replaced.len());
    let mut rest = replaced.as_str();
    while let Some(start) = rest.find(PROJECT) {
        let (before, at_placeholder) = rest.split_at(start);
        out.push_str(before);
        out.push_str(PROJECT);

        let tail = &at_placeholder[PROJECT.len()..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ':' | '\'' | '"' | ','))
            .unwrap_or(tail.len());
        out.push_str(&tail[..end].replace(r"\\", "/").replace('\\', "/"));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// §Scope-8 canonicalization, applied to BOTH sides of the `warnings` and
/// `resolved_pformat` comparisons: collapse the `<project>/` prefix so the
/// canonical spelling of every in-srcdir path is srcdir-relative.
///
/// docutils' `adapt_path` keeps the cwd-relative spelling of an *included*
/// file only in the node `source` attribute; what sphinx PRINTS is absolute
/// (an in-include warning location is cwd-absolutized by
/// `get_node_location`, or srcdir-joined by `doc2path` for a tuple
/// location). This crate deliberately spells included-content provenance
/// srcdir-relative (node `source` attrs, circular-inclusion chains, SEVERE
/// error texts, in-include warning locations — see `resolve_include_target`
/// in src/rst/block.rs) and everything else absolute, so a CLI user sees a
/// bare relative path where sphinx prints an absolute one. The generator already rewrites both
/// sphinx spellings to `<project>/...`; collapsing the token here maps our
/// absolute spellings AND sphinx's onto the same srcdir-relative form the
/// srcdir-relative spellings already use. Because the same transformation
/// runs on both sides, the only divergence class it can mask is
/// prefix-presence itself — exactly the class §Scope-8 sanctions; any
/// difference in the path *below* the srcdir still diverges.
/// Applied corpus-wide, not only to the include projects: every project's
/// `warnings` and `resolved_pformat` go through it, because the spelling
/// difference is a property of THIS crate's path handling and not of the
/// include family. Widening it costs nothing — a project with no
/// `<project>/` prefix in its strings is unchanged.
///
/// The `concat!` keeps the token spelled once as a literal that a
/// `<project>`-substituting tool cannot rewrite by accident; it is
/// `PROJECT` + a separator, and must stay in step with [`PROJECT`].
fn canon_scope8(text: &str) -> String {
    debug_assert_eq!(PROJECT, "<project>", "canon_scope8 literal is out of step");
    text.replace(concat!("<project>", "/"), "")
}

/// [`normalize_source_paths`] over a build's rendered warnings.
fn normalize_warnings(warnings: &[BuildWarning], root: &str) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| normalize_source_paths(&warning.render(), root))
        .collect()
}

/// [`normalize_source_paths`] over every string in an environment snapshot,
/// keys included.
///
/// Doctrees carry their absolute source path on the `document` node and
/// `dependencies` holds absolute paths; the oracle records both with the
/// srcdir replaced by [`PROJECT`] (see `tools/gen_env_fixture.py`), so the
/// whole snapshot goes through the same substitution.
///
/// Deliberately a walk over the parsed value rather than a substitution on
/// its JSON text: a serialized doctree escapes the quotes around
/// `source="…"`, so text-level rewriting cannot tell a path separator from
/// the backslash opening an escape sequence.
fn normalize_snapshot(snapshot: &serde_json::Value, root: &str) -> serde_json::Value {
    match snapshot {
        serde_json::Value::String(text) => {
            serde_json::Value::String(normalize_source_paths(text, root))
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| normalize_snapshot(item, root))
                .collect(),
        ),
        serde_json::Value::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    (
                        normalize_source_paths(key, root),
                        normalize_snapshot(value, root),
                    )
                })
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

/// Materialize one project into a tempdir, build it, and capture both
/// halves of the oracle comparison.
fn build_project(project: &Project) -> Built {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();

    for (docname, body) in &project.files {
        let path = source_dir.join(format!("{docname}.rst"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
    }
    for (relpath, body) in &project.data_files {
        let path = source_dir.join(relpath);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
    }
    // Warning locations come from walking the source tree, which resolves
    // symlinks (`/var/...` -> `/private/var/...` on macOS): normalize the
    // root the same way so the `<project>` substitution below lands.
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let config = config_of(project);
    let mut builder = SphinxBuilder::new(config, source_dir.clone(), output_dir)
        .unwrap_or_else(|e| panic!("project {}: builder setup failed: {e:#}", project.name));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let stats = runtime
        .block_on(builder.build())
        .unwrap_or_else(|e| panic!("project {}: build failed: {e:#}", project.name));

    let root = source_dir.to_string_lossy().into_owned();
    let warnings = normalize_warnings(&stats.warning_details, &root);
    let env = normalize_snapshot(&builder.snapshot_env(), &root);

    Built { env, warnings }
}

/// The project's `conf` overrides, applied to a default [`BuildConfig`] the
/// way `sphinx-build -D key=value` applies them.
///
/// Keys listed in [`KNOWN_INERT_CONF`] are skipped (nothing in this crate
/// reads them); every other key must be expressible as an override, so a
/// fixture project that starts setting something the harness cannot apply
/// fails loudly here.
fn config_of(project: &Project) -> BuildConfig {
    let mut config = BuildConfig::default();
    let conf = project.conf.as_object().expect("conf is an object");
    for (key, value) in conf {
        if KNOWN_INERT_CONF.contains(&key.as_str()) {
            assert_inert_conf_is_sound(&project.name, key, value);
            continue;
        }
        // A dict-valued setting is applied key by key (`-D numfig_format.figure=...`),
        // which is also what makes user entries merge over the defaults.
        let overrides: Vec<(String, String)> = match value {
            serde_json::Value::Object(map) => map
                .iter()
                .map(|(sub, v)| {
                    let key = format!("{key}.{sub}");
                    let value = scalar_override(&project.name, &key, v);
                    (key, value)
                })
                .collect(),
            other => vec![(key.clone(), scalar_override(&project.name, key, other))],
        };
        for (key, value) in overrides {
            let ignored = config
                .apply_override(&key, &value)
                .unwrap_or_else(|e| panic!("project {}: -D {key}={value}: {e:#}", project.name));
            assert!(
                ignored.is_none(),
                "project {}: -D {key}={value} was ignored: {}",
                project.name,
                ignored.unwrap()
            );
        }
    }
    config
}

/// One conf value rendered as the string a `-D` override carries.
///
/// A `-D` value is a single scalar, so most non-scalar (nested, or null)
/// conf entries have no faithful spelling here: stringifying one would hand
/// `apply_override` something like `["a","b"]`, which its `Value::Array`
/// arm cheerfully splits on commas into `["[\"a\"", "\"b\"]"]` and stores
/// — a silently *differently* configured build compared against the
/// oracle. The one expressible non-scalar is an array of comma-free
/// strings (`modindex_common_prefix = ['pkg.']`): `apply_override` splits
/// the `-D` value on commas, so joining the elements with commas round-trips
/// exactly — asserted per element, because an element *containing* a comma
/// would silently split into two (the divergence the paragraph above
/// guards against). Everything else is rejected outright, naming the key,
/// so a future fixture project carrying such a value fails here rather
/// than diverging quietly.
fn scalar_override(project: &str, key: &str, value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => value.to_string(),
        serde_json::Value::Array(items) => {
            let elements: Vec<&str> = items
                .iter()
                .map(|item| match item {
                    serde_json::Value::String(text) if !text.contains(',') => text.as_str(),
                    other => panic!(
                        "project {project:?}: conf key {key:?} has array element {other}, \
                         which the comma-joined -D form cannot carry faithfully (only \
                         comma-free strings round-trip through apply_override's \
                         comma-split)"
                    ),
                })
                .collect();
            elements.join(",")
        }
        other => panic!(
            "project {project:?}: conf key {key:?} has the non-scalar value {other}, which \
             -D cannot express — teach the harness to apply it (or prove it inert and add \
             it to KNOWN_INERT_CONF) rather than letting it stringify into garbage"
        ),
    }
}

/// Every project's build, run once for the whole test binary (each build
/// materializes a tempdir and runs the full pipeline).
fn built_projects() -> &'static BTreeMap<String, Built> {
    static BUILDS: OnceLock<BTreeMap<String, Built>> = OnceLock::new();
    BUILDS.get_or_init(|| {
        load_fixture()
            .projects
            .iter()
            .map(|project| (project.name.clone(), build_project(project)))
            .collect()
    })
}

fn built_of(project: &Project) -> &'static Built {
    built_projects()
        .get(&project.name)
        .unwrap_or_else(|| panic!("no build for project {}", project.name))
}

fn env_of(project: &Project) -> &'static serde_json::Value {
    &built_of(project).env
}

fn snapshot_field<T: serde::de::DeserializeOwned>(env: &serde_json::Value, key: &str) -> T {
    serde_json::from_value(env[key].clone())
        .unwrap_or_else(|e| panic!("snapshot key {key:?} has an unexpected shape: {e}"))
}

/// Divergences this task deliberately does not close, each pinned to the
/// wave-4 task that will. Checked **strictly**: a listed (project, docname)
/// that stops diverging fails the test, so the exemption is deleted rather
/// than left to rot.
const KNOWN_TOC_GAPS: &[(&str, &str, &str)] = &[];

fn known_toc_gap(project: &str, docname: &str) -> Option<&'static str> {
    KNOWN_TOC_GAPS
        .iter()
        .find(|(p, d, _)| *p == project && *d == docname)
        .map(|(_, _, why)| *why)
}

/// Projects whose oracle `warnings` this task deliberately does not
/// reproduce, each pinned to the wave-4 task that will. Checked
/// **strictly**, exactly like [`KNOWN_TOC_GAPS`]: a listed project whose
/// warnings start matching fails the test, so the exemption is deleted
/// rather than left to rot.
const KNOWN_WARNING_GAPS: &[(&str, &str)] = &[
    (
        "toctree_circular",
        "the three `circular toctree references detected` warnings come from \
         the write phase's toctree *resolution* (`_resolve_toctree` / \
         `_toctree_entry`, adapters/toctree.py), which lands with the \
         resolved-doctree task",
    ),
    (
        "toctree_numbered_depth2",
        "`image file not readable` — image collection is not ported yet",
    ),
    (
        "numfig_on",
        "`image file not readable` — image collection is not ported yet",
    ),
    (
        "numfig_off_numref",
        "`image file not readable` — image collection is not ported yet (the \
         project's other warning, `numfig is disabled. :numref: is ignored.`, \
         this task does produce)",
    ),
];

/// Projects whose oracle `std` data this task deliberately does not
/// reproduce. Checked **strictly**, exactly like [`KNOWN_TOC_GAPS`].
const KNOWN_STD_GAPS: &[(&str, &str)] = &[];

/// The one normalization this harness applies to the oracle's
/// `resolved_pformat`: docutils' i18n totaliser stamps every `document` node
/// with a `translation_progress` attribute, which `tools/gen_sphinx_fixture.py`
/// already strips from *its* oracle as a harness artifact (see that file's
/// `TP_ATTR`) and which nothing in this crate models. Stripping it here keeps
/// the two fixtures' document lines comparable to the same parse output.
const TRANSLATION_PROGRESS_ATTR: &str = " translation_progress=\"{'total': 0, 'translated': 0}\"";

/// Reasons shared by several [`KNOWN_RESOLVED_GAPS`] entries.
const TOCTREE_RESOLUTION: &str = "the write-phase toctree resolution that turns a \
     `toctree` node into a `compact_paragraph` entry tree is not ported yet";
const IMAGE_CANDIDATES: &str = "`ImageCollector.process_doc` stamps `candidates` onto \
     every `image`; no image collection exists yet";
const PROPAGATE_TARGETS: &str = "docutils' `PropagateTargets` transform (which moves a \
     block-level target's ids and names onto the node after it) is replayed for label \
     collection but not applied to the tree itself";
const PROPAGATE_MODULE_TARGETS: &str = "plan §Scope-3: a `py:module` target's ids \
     migrate onto the following node in Sphinx's tree (docutils `PropagateTargets`, \
     plus the index/target reorder around it) — the in-tree application is deferred \
     to wave 5; the registration records (`py_objects`/`py_modules`) pin the \
     pre-propagation node ids for these same documents, and a module registered as \
     a document's LAST node (py_dup's `a`) compares clean because a trailing \
     target has nothing to propagate onto";

/// Per-document `resolved_pformat` divergences this task deliberately does
/// not close, each pinned to what would close it. Checked **strictly**,
/// exactly like [`KNOWN_TOC_GAPS`].
const KNOWN_RESOLVED_GAPS: &[(&str, &str, &str)] = &[
    // Write-phase toctree resolution (`adapters/toctree.py`'s
    // `_resolve_toctree`), which replaces a `toctree` node with the
    // `compact_paragraph`/`bullet_list` tree of entry references. Not part
    // of this task; every document below carries a `toctree` and nothing
    // else it needs.
    ("toctree_nested", "a", TOCTREE_RESOLUTION),
    ("toctree_nested", "index", TOCTREE_RESOLUTION),
    ("toctree_glob", "index", TOCTREE_RESOLUTION),
    ("toctree_numbered", "index", TOCTREE_RESOLUTION),
    ("toctree_numbered_depth2", "index", TOCTREE_RESOLUTION),
    ("toctree_self_ref", "index", TOCTREE_RESOLUTION),
    ("toctree_circular", "a", TOCTREE_RESOLUTION),
    ("toctree_circular", "b", TOCTREE_RESOLUTION),
    ("toctree_circular", "index", TOCTREE_RESOLUTION),
    ("toctree_multi_parent", "a", TOCTREE_RESOLUTION),
    ("toctree_multi_parent", "b", TOCTREE_RESOLUTION),
    ("toctree_multi_parent", "index", TOCTREE_RESOLUTION),
    ("orphan_doc", "index", TOCTREE_RESOLUTION),
    ("numfig_on", "index", TOCTREE_RESOLUTION),
    ("numfig_off_numref", "index", TOCTREE_RESOLUTION),
    ("labels_dups", "index", TOCTREE_RESOLUTION),
    ("glossary_terms", "index", TOCTREE_RESOLUTION),
    ("index_entries", "index", TOCTREE_RESOLUTION),
    ("doc_refs", "index", TOCTREE_RESOLUTION),
    ("std_objects", "index", TOCTREE_RESOLUTION),
    // Read-phase gaps of other subsystems, each already tracked by the
    // task that owns it.
    ("toctree_numbered_depth2", "a", IMAGE_CANDIDATES),
    ("numfig_off_numref", "a", IMAGE_CANDIDATES),
    (
        "numfig_on",
        "a",
        "`image[candidates]` (see IMAGE_CANDIDATES) plus the `linenos` flag \
         a captioned `code-block` stamps onto its `literal_block`",
    ),
    (
        "orphan_doc",
        "orphan",
        "`MetadataCollector.process_doc` *removes* the docinfo field list \
         from the doctree after reading it (`collectors/metadata.py:40`); \
         ours reads it and leaves the node in place",
    ),
    ("labels_dups", "a", PROPAGATE_TARGETS),
    ("labels_dups", "b", PROPAGATE_TARGETS),
    ("index_entries", "a", PROPAGATE_TARGETS),
    // Wave 4.5 py projects: toctree-bearing index documents share the
    // write-phase gap above; module-bearing documents are §Scope-3
    // propagation-visible (verified: each diff is exactly the module
    // target ids moving onto the section/following target, nothing else).
    ("py_basic", "index", TOCTREE_RESOLUTION),
    ("py_dup", "index", TOCTREE_RESOLUTION),
    ("py_toc", "index", TOCTREE_RESOLUTION),
    ("py_toc_parents", "index", TOCTREE_RESOLUTION),
    ("py_basic", "a", PROPAGATE_MODULE_TARGETS),
    ("py_dup", "b", PROPAGATE_MODULE_TARGETS),
    ("py_toc", "mod", PROPAGATE_MODULE_TARGETS),
    ("py_toc_parents", "mod", PROPAGATE_MODULE_TARGETS),
    ("py_modindex", "index", PROPAGATE_MODULE_TARGETS),
    ("py_modindex_prefix", "index", PROPAGATE_MODULE_TARGETS),
    // Wave 4.5 include/literalinclude projects. Only the toctree-bearing
    // index documents and the one image-bearing document are skipped
    // outright; every other document of these three projects is compared,
    // six of them through the narrow [`KNOWN_HIGHLIGHT_STAMP_GAPS`]
    // exemption rather than being dropped.
    ("inc_basic", "index", TOCTREE_RESOLUTION),
    ("inc_warn", "index", TOCTREE_RESOLUTION),
    ("inc_deps", "index", TOCTREE_RESOLUTION),
    // Panel fix round B: `py_any` keeps every `:any:` reference in `a`
    // (compared at full strength) and its definitions in `b`, whose two
    // labels sit before section titles — the PropagateTargets shape. The
    // module is the document's LAST node, so its target propagates onto
    // nothing (the py_dup shape) and is not part of the diff.
    ("py_any", "index", TOCTREE_RESOLUTION),
    ("py_any", "b", PROPAGATE_TARGETS),
    (
        "inc_deps",
        "a",
        "`image[candidates]` (see IMAGE_CANDIDATES) — and, on the same \
         document's `literal_block`, the highlight stamp of \
         KNOWN_HIGHLIGHT_STAMP_GAPS, which cannot be applied to a document \
         that already diverges structurally",
    ),
    // Wave 5 diagnostics-stream projects: both documents of
    // `reporter_interleave` that carry a toctree (the second one's names
    // only a missing document, which Sphinx's resolution replaces with
    // nothing). The project exists for its warning stream; `first` is
    // compared at full strength.
    ("reporter_interleave", "index", TOCTREE_RESOLUTION),
    ("reporter_interleave", "second", TOCTREE_RESOLUTION),
];

/// The attributes Sphinx's `HighlightLanguageTransform`
/// (`transforms/post_transforms/code.py`, priority 400) stamps onto a
/// `literal_block` that does not already carry them: `language` (+ `force`)
/// from the `highlight_language` setting in scope, and `linenos` from the
/// `.. highlight::` line-number threshold.
const HIGHLIGHT_STAMP_ATTRS: [&str; 3] = ["language", "force", "linenos"];

/// Documents whose ONLY `resolved_pformat` divergence is that transform.
///
/// Unlike [`KNOWN_RESOLVED_GAPS`], which skips a document wholesale, an
/// entry here still compares the entire tree — every node, every other
/// attribute, every character of text — and exempts nothing but the
/// *presence* of a [`HIGHLIGHT_STAMP_ATTRS`] attribute the oracle's
/// `literal_block` carries and ours does not have at all. A value both
/// sides do carry still has to agree. That keeps these projects doing the
/// job they exist for (they are the oracle venue for the file-inserting
/// directives' node shapes — see tools/gen_env_fixture.py) instead of
/// dropping out of the comparison over three stamped attributes.
///
/// The transform belongs to wave 5 (docs/superpowers/plans/
/// 2026-08-07-m2-wave-map.md lists it among the exemptions that wave is
/// expected to delete, as "the `linenos` flag a captioned `code-block`
/// stamps onto its `literal_block`" — the same pass, met there through
/// `numfig_on`/`a`). Checked **strictly** in both directions: a listed
/// document that starts matching outright fails, and one that diverges by
/// anything beyond the stamp is reported as a real divergence.
///
/// BOUNDARY, stated so the next reader does not have to derive it: the
/// forgiveness is keyed on OUR side lacking the attribute entirely
/// ([`drop_unstamped_highlight_attrs`] skips any attribute we do carry).
/// On these documents that means a `language`/`force` value set
/// explicitly BY A DIRECTIVE would be forgiven if we failed to emit it at
/// all — e.g. a `:language:` added to `inc_basic/a`, where the whole
/// attribute going missing on our side would read as an unstamped
/// attribute rather than as the bug it is. (Panel fix round B, [21]: the
/// earlier claim that none of the six wave-4.5 entries carries a
/// directive-set value was false — `inc_basic/b`'s `:lineno-match:`
/// literalinclude carries `linenos="1"`, set by the directive,
/// `code.py:476-481`.) `linenos` is
/// therefore held to a tighter rule: the transform only ever stamps
/// `linenos` from the `linenothreshold` comparison, which this corpus
/// never lowers below its `sys.maxsize` default, so a stamped value is
/// always `"0"` and an oracle `linenos="1"` is by construction
/// directive-set — [`drop_unstamped_highlight_attrs`] refuses to drop it.
/// A `language`/`force` value set by a directive on one of these
/// documents still belongs in a document outside this table, or the
/// table shrinks. The full-strength venue for all three attributes is
/// `inc_highlight/index` (two literalincludes with `:language:` plus
/// `:linenos:`/`:lineno-start:`, one with `:force:`, and a `:linenos:`
/// code-block), which is in neither table; every other `literal_block`
/// in the corpus lives in a document one of the two tables covers.
const KNOWN_HIGHLIGHT_STAMP_GAPS: &[(&str, &str)] = &[
    ("inc_basic", "a"),
    ("inc_basic", "b"),
    ("inc_basic", "circ_a"),
    ("inc_basic", "circ_b"),
    ("inc_basic", "shared/frag"),
    ("inc_warn", "a"),
    // The literal blocks of in-tree `system_message`s (kept by
    // `keep_warnings`): an include failure's, and an error's raised inside
    // an included file.
    ("inc_missing", "index"),
    ("inc_nested_error", "index"),
];

fn known_highlight_stamp_gap(project: &str, docname: &str) -> bool {
    KNOWN_HIGHLIGHT_STAMP_GAPS
        .iter()
        .any(|(p, d)| *p == project && *d == docname)
}

/// `expected` with every [`HIGHLIGHT_STAMP_ATTRS`] attribute dropped from a
/// `literal_block` open tag whose counterpart in `actual` does not carry
/// that attribute at all — i.e. exactly the stamping
/// `HighlightLanguageTransform` performs, undone.
///
/// The two texts are walked line by line, which is sound because a
/// `pformat` renders one node per line: if the trees differ structurally
/// the paired lines stop being the same node and the comparison fails,
/// which is the outcome we want. A line is only touched when BOTH sides
/// open a `literal_block` there, so a stray `<literal_block ` inside
/// literal *text* would have to appear at the same position on both sides
/// AND carry a stamped attribute on the oracle side alone to be misread.
fn drop_unstamped_highlight_attrs(expected: &str, actual: &str) -> String {
    let mut ours = actual.lines();
    let mut out = String::with_capacity(expected.len());
    for oracle_line in expected.lines() {
        let our_line = ours.next().unwrap_or_default();
        let mut oracle_line = oracle_line.to_string();
        if oracle_line.trim_start().starts_with("<literal_block ")
            && our_line.trim_start().starts_with("<literal_block ")
        {
            for attr in HIGHLIGHT_STAMP_ATTRS {
                let needle = format!(" {attr}=\"");
                if our_line.contains(&needle) {
                    continue;
                }
                // A `linenos="1"` cannot be the transform's doing in this
                // corpus (see the table's BOUNDARY note): never forgive it.
                if attr == "linenos" && oracle_line.contains(" linenos=\"1\"") {
                    continue;
                }
                let Some(start) = oracle_line.find(&needle) else {
                    continue;
                };
                let value = start + needle.len();
                if let Some(offset) = oracle_line[value..].find('"') {
                    oracle_line.replace_range(start..value + offset + 1, "");
                }
            }
        }
        out.push_str(&oracle_line);
        out.push('\n');
    }
    out
}

fn known_resolved_gap(project: &str, docname: &str) -> Option<&'static str> {
    KNOWN_RESOLVED_GAPS
        .iter()
        .find(|(p, d, _)| *p == project && *d == docname)
        .map(|(_, _, why)| *why)
}

fn known_std_gap(project: &str) -> Option<&'static str> {
    KNOWN_STD_GAPS
        .iter()
        .find(|(p, _)| *p == project)
        .map(|(_, why)| *why)
}

fn known_warning_gap(project: &str) -> Option<&'static str> {
    KNOWN_WARNING_GAPS
        .iter()
        .find(|(p, _)| *p == project)
        .map(|(_, why)| *why)
}

/// The side-condition that makes a [`KNOWN_WARNING_GAPS`] entry sound.
///
/// The table's own check is direction-blind — it asserts only that the two
/// sides still DIFFER — so a project that started emitting a warning of its
/// own would go on passing under an exemption written for a warning it
/// FAILS to emit. Every entry's stated reason is of that second kind ("not
/// ported yet"), so the sound invariant is a one-directional one: our
/// warnings must be a subset of the oracle's. A warning on our side that
/// the oracle lacks is a fabricated diagnostic, which is precisely what
/// breaks `-W` on a project Sphinx builds clean.
fn assert_warning_gap_is_sound(project: &str, actual: &[String], expected: &[String]) {
    // Multiset containment, not set membership: the corpus treats
    // multiplicity as load-bearing (inc_warn expects one line exactly
    // twice), and a warning the oracle emits once that we emit twice is an
    // invented duplicate, which set membership would wave through (panel
    // fix round B, minor). Each oracle line is a budget one actual line
    // may spend.
    let mut budget: BTreeMap<&String, usize> = BTreeMap::new();
    for line in expected {
        *budget.entry(line).or_default() += 1;
    }
    let extra: Vec<&String> = actual
        .iter()
        .filter(|line| match budget.get_mut(line) {
            Some(remaining) if *remaining > 0 => {
                *remaining -= 1;
                false
            }
            _ => true,
        })
        .collect();
    assert!(
        extra.is_empty(),
        "[{project}] is in KNOWN_WARNING_GAPS, but emits warnings the oracle \
         does not (or emits one more often than it does) — the exemption \
         covers MISSING warnings, never invented ones: {extra:#?}"
    );
}

/// Fixture `conf` keys that provably steer no behavior this crate has
/// implemented, which is what makes it sound to leave them off the
/// [`config_of`] override pass.
///
/// `smartquotes`: no smart-quote transform exists.
///
/// `keep_warnings`: no `FilterSystemMessages` transform exists — this crate
/// never removes a reporter `system_message` from a doctree, which is
/// precisely what `keep_warnings=True` asks Sphinx to do (its transform
/// keeps every message of level >= 2). The *stripping* half — the `False`
/// default, which drops every system_message below SEVERE — is the
/// unimplemented one, so the entry is sound only while every project that
/// sets the key sets it `True`; [`assert_inert_conf_is_sound`] pins that.
/// Projects that leave it unset are self-defending: Sphinx strips their
/// messages, so a system_message appearing on our side alone would fail the
/// `resolved_pformat` comparison rather than pass silently.
///
/// A new fixture project introducing another such key must either have it
/// added here (with the same kind of justification) or be expressible as a
/// `-D` override.
const KNOWN_INERT_CONF: &[&str] = &["smartquotes", "keep_warnings"];

/// The side-condition that makes one [`KNOWN_INERT_CONF`] entry sound.
///
/// `keep_warnings` is inert only in its `True` sense (see the constant's
/// docs), so a project setting it any other way must fail here rather than
/// be compared against a differently filtered oracle.
///
/// Exhaustive on purpose. A future key added to [`KNOWN_INERT_CONF`] with
/// no arm here fails the first project that sets it, with instructions —
/// the mechanical forcing the table otherwise lacks, so "inert" cannot be
/// claimed for a new key without either a side-condition or an explicit
/// statement that every value is inert.
fn assert_inert_conf_is_sound(project: &str, key: &str, value: &serde_json::Value) {
    match key {
        // Inert in EVERY sense: no smart-quote transform exists at all, so
        // neither `True` nor `False` steers anything.
        "smartquotes" => {}
        "keep_warnings" => assert_eq!(
            value,
            &serde_json::Value::Bool(true),
            "project {project:?}: keep_warnings={value}, but the harness treats the key \
             as inert only in its `True` sense — this crate never strips reporter \
             system_messages, and the stripping half is unimplemented"
        ),
        other => panic!(
            "project {project:?} sets {other:?}, which is listed in KNOWN_INERT_CONF \
             but has no soundness arm in assert_inert_conf_is_sound. Add one: either \
             an empty arm with a comment saying why EVERY value is inert, or an \
             assertion narrowing it to the values that are."
        ),
    }
}

fn report(divergences: &[String], keys: &str) {
    assert!(
        divergences.is_empty(),
        "{} divergence(s) vs the sphinx 9.1.0 environment oracle ({keys}):\n\n{}",
        divergences.len(),
        divergences.join("\n\n")
    );
}

/// The oracle's expected strings are generated on POSIX; Windows renders
/// the same warnings with native separators, which is what `sphinx-build`
/// does there too. [`normalize_source_paths`] is what makes the comparison
/// separator-insensitive, so it is pinned here directly — this machine
/// cannot produce a backslash-shaped rendering to exercise it end to end.
#[test]
fn source_paths_normalize_across_separators() {
    // A Windows rendering, with the root spelled the way the product would.
    let win_root = r"C:\proj\source";
    assert_eq!(
        normalize_source_paths(
            r"C:\proj\source\sub\b.rst:7: WARNING: duplicate label dup, other instance in C:\proj\source\a.rst",
            win_root,
        ),
        "<project>/sub/b.rst:7: WARNING: duplicate label dup, \
         other instance in <project>/a.rst",
        "both the location prefix and an in-message path are rewritten"
    );

    // The POSIX rendering this machine produces is already canonical, and
    // must come out byte-identical to the Windows one.
    assert_eq!(
        normalize_source_paths(
            "/proj/source/sub/b.rst:7: WARNING: duplicate label dup, \
             other instance in /proj/source/a.rst",
            "/proj/source",
        ),
        "<project>/sub/b.rst:7: WARNING: duplicate label dup, \
         other instance in <project>/a.rst"
    );

    // Backslashes outside a placeholder-rooted path are warning *text* and
    // must survive untouched.
    assert_eq!(
        normalize_source_paths(
            r"C:\proj\source\a.rst:4: WARNING: invalid pair index entry 'a\nb' [index]",
            win_root,
        ),
        r"<project>/a.rst:4: WARNING: invalid pair index entry 'a\nb' [index]"
    );

    // A path Python `repr()`'d into the message — the `_StrPath(...)` the
    // literalinclude reader raises — has its separators doubled, and comes
    // out spelled like every other one.
    assert_eq!(
        normalize_source_paths(
            r"C:\proj\source\a.rst:15: WARNING: Object named 'nope' not found in include file _StrPath('C:\\proj\\source\\example.py') [docutils]",
            win_root,
        ),
        "<project>/a.rst:15: WARNING: Object named 'nope' not found in include file \
         _StrPath('<project>/example.py') [docutils]"
    );

    // A warning Sphinx logs with no location has no path to rewrite.
    assert_eq!(
        normalize_source_paths("WARNING: failed to reach any of the inventories", win_root),
        "WARNING: failed to reach any of the inventories"
    );

    // The snapshot carries paths inside a serialized doctree's `source`
    // attribute, in `dependencies`, and in object keys. Walking the parsed
    // value normalizes all three without touching the JSON escaping that a
    // text-level substitution would have to fight.
    let windows_snapshot = serde_json::json!({
        "dependencies": { "a": [r"C:\proj\source\pic.png"] },
        "resolved": { "a": r#"<document source="C:\proj\source\a.rst">"# },
        "by_path": { r"C:\proj\source\a.rst": 1 },
    });
    let posix_snapshot = serde_json::json!({
        "dependencies": { "a": ["/proj/source/pic.png"] },
        "resolved": { "a": r#"<document source="/proj/source/a.rst">"# },
        "by_path": { "/proj/source/a.rst": 1 },
    });
    assert_eq!(
        normalize_snapshot(&windows_snapshot, win_root),
        normalize_snapshot(&posix_snapshot, "/proj/source"),
        "the two platforms' snapshots must normalize to the same value"
    );
    assert_eq!(
        normalize_snapshot(&windows_snapshot, win_root),
        serde_json::json!({
            "dependencies": { "a": ["<project>/pic.png"] },
            "resolved": { "a": r#"<document source="<project>/a.rst">"# },
            "by_path": { "<project>/a.rst": 1 },
        })
    );
}

/// The one non-scalar `-D` form the harness can express: an array of
/// comma-free strings renders comma-joined, which `apply_override`'s
/// comma-split turns back into exactly the same elements. Scalars render as
/// before; an element carrying a comma would silently split into two, so it
/// panics instead (pinned via the round-trip's behavior being asserted, not
/// via `should_panic` — the panic message names the key).
#[test]
fn conf_arrays_of_comma_free_strings_render_as_the_comma_joined_override() {
    assert_eq!(
        scalar_override("p", "modindex_common_prefix", &serde_json::json!(["pkg."])),
        "pkg."
    );
    assert_eq!(
        scalar_override(
            "p",
            "modindex_common_prefix",
            &serde_json::json!(["a.", "b."])
        ),
        "a.,b."
    );
    // And the round trip through the real override machinery lands the
    // exact elements, which is the property the comma-free assert protects.
    let mut config = BuildConfig::default();
    config
        .apply_override("modindex_common_prefix", "a.,b.")
        .unwrap();
    assert_eq!(config.modindex_common_prefix, vec!["a.", "b."]);

    assert_eq!(
        scalar_override("p", "nitpicky", &serde_json::json!(true)),
        "true"
    );
    assert_eq!(
        scalar_override("p", "numfig_secnum_depth", &serde_json::json!(2)),
        "2"
    );
}

/// [`drop_unstamped_highlight_attrs`] removes an oracle-only highlight
/// stamp and nothing else: a value both sides carry stays (and keeps
/// diverging if it differs), a non-`literal_block` line is untouched, and
/// the surrounding tree text is preserved byte for byte.
#[test]
fn only_oracle_only_highlight_stamps_are_dropped() {
    let oracle = concat!(
        "<document source=\"a.rst\">\n",
        "    <literal_block force=\"0\" language=\"default\" linenos=\"0\" ",
        "source=\"x.inc\" xml:space=\"preserve\">\n",
        "        text\n",
        "    <paragraph language=\"default\">\n",
        "        not a literal block\n",
    );
    // Ours carries `force`; `language`/`linenos` it has never heard of.
    let ours = concat!(
        "<document source=\"a.rst\">\n",
        "    <literal_block force=\"0\" source=\"x.inc\" xml:space=\"preserve\">\n",
        "        text\n",
        "    <paragraph language=\"default\">\n",
        "        not a literal block\n",
    );
    assert_eq!(drop_unstamped_highlight_attrs(oracle, ours), ours);

    // A stamped attribute BOTH sides carry is left in place, so a wrong
    // value still diverges.
    let ours_wrong_force = ours.replace("force=\"0\"", "force=\"1\"");
    assert_eq!(
        drop_unstamped_highlight_attrs(oracle, &ours_wrong_force),
        ours,
        "only `language` and `linenos` come off when ours carries `force`"
    );
    assert_ne!(
        drop_unstamped_highlight_attrs(oracle, &ours_wrong_force),
        ours_wrong_force,
        "a `force` value we disagree on still diverges"
    );

    // Any other divergence survives: the text of the block still has to
    // match.
    let ours_wrong_text = ours.replace("        text\n", "        other\n");
    assert_ne!(
        drop_unstamped_highlight_attrs(oracle, &ours_wrong_text),
        ours_wrong_text
    );

    // A `linenos="1"` is never forgiven, even when ours carries no
    // `linenos` at all: the transform only ever stamps `"0"` in this
    // corpus, so an oracle `"1"` we fail to emit is a directive-set value
    // going missing — a real divergence, which the BOUNDARY refusal keeps
    // (panel round C pin for [21]). `language` still comes off beside it.
    let oracle_directive_linenos = oracle.replace("linenos=\"0\"", "linenos=\"1\"");
    let kept = drop_unstamped_highlight_attrs(&oracle_directive_linenos, ours);
    let block = kept
        .lines()
        .find(|line| line.trim_start().starts_with("<literal_block "))
        .expect("the literal_block line survives");
    assert!(block.contains(" linenos=\"1\""), "{kept}");
    assert!(!block.contains(" language="), "{kept}");
    assert_ne!(
        kept, ours,
        "a directive-set `linenos` we do not emit still diverges"
    );
}

#[test]
fn fixture_loads_and_meets_floor() {
    let fixture = load_fixture();
    assert_eq!(fixture.sphinx_version, "9.1.0");
    assert_eq!(fixture.docutils_version, "0.22.4");
    assert!(
        fixture.projects.len() >= 20,
        "fixture truncated? only {} projects",
        fixture.projects.len()
    );

    let mut names: Vec<&str> = fixture.projects.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    let mut deduped = names.clone();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        names.len(),
        "project names must be unique: {names:?}"
    );

    // Every project must have a root "index" doc to be buildable at all.
    for project in &fixture.projects {
        assert!(
            project.files.contains_key("index"),
            "project {:?} has no root index.rst",
            project.name
        );
    }

    // Every conf key is either applied as a `-D` override or listed as
    // provably inert: `config_of` panics on a key that is neither (an
    // override `apply_override` rejects or silently ignores).
    for project in &fixture.projects {
        assert!(
            project.conf.is_object(),
            "project {:?}: conf is not an object",
            project.name
        );
        let _ = config_of(project);
    }
}

// ---------------------------------------------------------------------------
// Live: task 5 keys
// ---------------------------------------------------------------------------

/// `env.toctree_includes` and `env.files_to_rebuild` — the toctree graph
/// `note_toctree` builds from every resolved `toctree` node.
#[test]
fn toctree_graph_matches_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);

        let includes: BTreeMap<String, Vec<String>> = snapshot_field(env, "toctree_includes");
        if includes != project.expect.toctree_includes {
            divergences.push(format!(
                "[{}] toctree_includes\n  expected: {:?}\n  actual:   {:?}",
                project.name, project.expect.toctree_includes, includes
            ));
        }

        let rebuild: BTreeMap<String, Vec<String>> = snapshot_field(env, "files_to_rebuild");
        if rebuild != project.expect.files_to_rebuild {
            divergences.push(format!(
                "[{}] files_to_rebuild\n  expected: {:?}\n  actual:   {:?}",
                project.name, project.expect.files_to_rebuild, rebuild
            ));
        }
    }

    report(&divergences, "toctree_includes, files_to_rebuild");
}

/// `env.tocs` (as pseudo-XML) and `env.toc_num_entries` — one document's
/// local table of contents, per `TocTreeCollector.process_doc`.
#[test]
fn local_tocs_match_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();
    let mut visited_gaps: Vec<(&str, &str)> = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);
        let tocs: BTreeMap<String, String> = snapshot_field(env, "tocs_pformat");
        let num_entries: BTreeMap<String, u32> = snapshot_field(env, "toc_num_entries");

        assert_eq!(
            tocs.keys().collect::<Vec<_>>(),
            project.expect.tocs_pformat.keys().collect::<Vec<_>>(),
            "[{}] env.tocs covers a different document set than the oracle",
            project.name
        );

        for (docname, expected) in &project.expect.tocs_pformat {
            // Compared verbatim, `secnumber` attributes included: those are
            // stamped onto these very `reference` nodes by
            // `assign_section_numbers`, from the same walk that fills
            // `toc_secnumbers`.
            let actual = &tocs[docname];
            let expected_entries = project.expect.toc_num_entries[docname];
            let actual_entries = num_entries[docname];
            let matches = actual == expected && actual_entries == expected_entries;

            match known_toc_gap(&project.name, docname) {
                Some(why) => {
                    visited_gaps.push((project.name.as_str(), docname.as_str()));
                    assert!(
                        !matches,
                        "[{}] {docname}: listed in KNOWN_TOC_GAPS ({why}) but the toc \
                         now matches the oracle — delete the exemption",
                        project.name
                    );
                }
                None if !matches => divergences.push(format!(
                    "[{}] {docname}: toc_num_entries {expected_entries} vs {actual_entries}\n\
                     --- oracle ---\n{expected}--- ours ---\n{actual}",
                    project.name
                )),
                None => {}
            }
        }
    }

    // Self-cleaning exemptions: an entry naming a (project, docname) the
    // loop above never reached is stale — the fixture no longer contains
    // it, so the exemption is silently exempting nothing.
    for (project, docname, why) in KNOWN_TOC_GAPS {
        assert!(
            visited_gaps.contains(&(*project, *docname)),
            "KNOWN_TOC_GAPS entry ({project}, {docname}) — {why} — was never \
             visited: no such project/document in the fixture. Delete it."
        );
    }

    report(&divergences, "tocs_pformat, toc_num_entries");
}

// ---------------------------------------------------------------------------
// Live: task 6 keys
// ---------------------------------------------------------------------------

/// `env.collect_relations()` — the pre-order toctree walk that gives every
/// document its `[parent, prev, next]` rellinks.
///
/// One project (`toctree_circular`) records `relations: null`: real sphinx
/// 9.1.0 raises `RecursionError` computing it, so there is nothing to
/// compare against. Our port is cycle-guarded and must still answer, which
/// the null branch asserts rather than skipping outright.
#[test]
fn relations_match_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);
        let relations: BTreeMap<String, RelationsEntry> = snapshot_field(env, "relations");

        match &project.expect.relations {
            Some(expected) => {
                if relations != *expected {
                    divergences.push(format!(
                        "[{}] relations\n  expected: {expected:?}\n  actual:   {relations:?}",
                        project.name
                    ));
                }
            }
            None => assert!(
                relations.contains_key("index"),
                "[{}] the oracle cannot compute relations for this project \
                 (sphinx recurses without bound), but our cycle-guarded port \
                 must still produce a best-effort answer rooted at the root \
                 document; got {relations:?}",
                project.name
            ),
        }
    }

    report(&divergences, "relations");
}

/// Every diagnostic the build emits, byte-identical to `sphinx-build`'s and
/// compared record by record — message text (a multi-line docutils message
/// with its blank lines), `file:line` location, the `WARNING`/`ERROR`/
/// `CRITICAL` prefix, the `[type.subtype]` suffix `show_warning_types`
/// appends — in print order: each re-read document's read-phase stream
/// (docutils reporter records and the directives' and domains' logger
/// records, interleaved as they were created), then its index and std
/// `process_doc` warnings, documents in sorted order; resolution-phase
/// warnings after all of them.
///
/// The projects whose oracle stream holds a warning of a subsystem not
/// ported yet are pinned in [`KNOWN_WARNING_GAPS`].
#[test]
fn warning_records_match_the_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();
    let mut visited_gaps: Vec<&str> = Vec::new();

    for project in &fixture.projects {
        // Both sides pass through the §Scope-8 canonicalization; see
        // [`canon_scope8`] for why this is comparison-preserving.
        let actual: Vec<String> = built_of(project)
            .warnings
            .iter()
            .map(|warning| canon_scope8(warning))
            .collect();
        let expected: Vec<String> = project
            .expect
            .warning_records
            .iter()
            .map(|warning| canon_scope8(warning))
            .collect();
        let matches = actual == expected;

        match known_warning_gap(&project.name) {
            Some(why) => {
                visited_gaps.push(project.name.as_str());
                assert!(
                    !matches,
                    "[{}] listed in KNOWN_WARNING_GAPS ({why}) but the warnings \
                     now match the oracle — delete the exemption",
                    project.name
                );
                assert_warning_gap_is_sound(&project.name, &actual, &expected);
            }
            None if !matches => divergences.push(format!(
                "[{}] warning_records\n  expected: {expected:#?}\n  actual:   {actual:#?}",
                project.name
            )),
            None => {}
        }
    }

    for (project, why) in KNOWN_WARNING_GAPS {
        assert!(
            visited_gaps.contains(project),
            "KNOWN_WARNING_GAPS entry ({project}) — {why} — was never visited: \
             no such project in the fixture. Delete it."
        );
    }

    report(&divergences, "warning_records");
}

// ---------------------------------------------------------------------------
// Live: task 7 keys
// ---------------------------------------------------------------------------

/// `env.toc_secnumbers` and `env.toc_fignumbers` — `assign_section_numbers`
/// and `assign_figure_numbers` (`collectors/toctree.py:197-378`), the
/// numbering pass the resolve phase runs over the finished toc set.
#[test]
fn section_and_figure_numbering_matches_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);

        let secnumbers: BTreeMap<String, BTreeMap<String, Vec<u32>>> =
            snapshot_field(env, "toc_secnumbers");
        if secnumbers != project.expect.toc_secnumbers {
            divergences.push(format!(
                "[{}] toc_secnumbers\n  expected: {:?}\n  actual:   {secnumbers:?}",
                project.name, project.expect.toc_secnumbers
            ));
        }

        let fignumbers: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<u32>>>> =
            snapshot_field(env, "toc_fignumbers");
        if fignumbers != project.expect.toc_fignumbers {
            divergences.push(format!(
                "[{}] toc_fignumbers\n  expected: {:?}\n  actual:   {fignumbers:?}",
                project.name, project.expect.toc_fignumbers
            ));
        }
    }

    report(&divergences, "toc_secnumbers, toc_fignumbers");
}

// ---------------------------------------------------------------------------
// Pending: later wave-4 tasks
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Live: task 8 keys
// ---------------------------------------------------------------------------

/// `env.domaindata['std']` — the labels, anonymous labels, objects, program
/// options and glossary terms `StandardDomain.process_doc` (and the
/// directives that register objects) collect.
#[test]
fn std_domain_matches_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();
    let mut visited_gaps: Vec<&str> = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);
        let expected = &project.expect.std;

        let std = &env["std"];
        let labels: BTreeMap<String, (String, String, String)> = snapshot_field(std, "labels");
        let anonlabels: BTreeMap<String, (String, String)> = snapshot_field(std, "anonlabels");
        let objects: Vec<StdObjectEntry> = snapshot_field(std, "objects");
        let progoptions: Vec<StdProgOptionEntry> = snapshot_field(std, "progoptions");
        let terms: BTreeMap<String, (String, String)> = snapshot_field(std, "terms");

        let mut mismatches = Vec::new();
        if labels != expected.labels {
            mismatches.push(format!(
                "  labels\n    expected: {:?}\n    actual:   {labels:?}",
                expected.labels
            ));
        }
        if anonlabels != expected.anonlabels {
            mismatches.push(format!(
                "  anonlabels\n    expected: {:?}\n    actual:   {anonlabels:?}",
                expected.anonlabels
            ));
        }
        if objects != expected.objects {
            mismatches.push(format!(
                "  objects\n    expected: {:?}\n    actual:   {objects:?}",
                expected.objects
            ));
        }
        if progoptions != expected.progoptions {
            mismatches.push(format!(
                "  progoptions\n    expected: {:?}\n    actual:   {progoptions:?}",
                expected.progoptions
            ));
        }
        if terms != expected.terms {
            mismatches.push(format!(
                "  terms\n    expected: {:?}\n    actual:   {terms:?}",
                expected.terms
            ));
        }

        match known_std_gap(&project.name) {
            Some(why) => {
                visited_gaps.push(project.name.as_str());
                assert!(
                    !mismatches.is_empty(),
                    "[{}] listed in KNOWN_STD_GAPS ({why}) but the std domain \
                     data now matches the oracle — delete the exemption",
                    project.name
                );
            }
            None if !mismatches.is_empty() => {
                divergences.push(format!("[{}] std\n{}", project.name, mismatches.join("\n")))
            }
            None => {}
        }
    }

    for (project, why) in KNOWN_STD_GAPS {
        assert!(
            visited_gaps.contains(project),
            "KNOWN_STD_GAPS entry ({project}) — {why} — was never visited: \
             no such project in the fixture. Delete it."
        );
    }

    report(
        &divergences,
        "std.labels/anonlabels/objects/progoptions/terms",
    );
}

// ---------------------------------------------------------------------------
// Live: task 10 keys
// ---------------------------------------------------------------------------

/// `env.domaindata['index']['entries']` — every `index` node's 5-tuples,
/// harvested per document by `IndexDomain.process_doc` — and the finished
/// general index `IndexEntries.create_index()` assembles from them.
///
/// The oracle builds with sphinx's dummy builder, whose
/// `get_relative_uri('genindex', docname)` is `''`, so every genindex target
/// uri is a bare `#<target_id>`.
#[test]
fn index_and_genindex_match_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);

        let entries: BTreeMap<String, Vec<IndexEntryTuple>> = snapshot_field(env, "index_entries");
        if entries != project.expect.index_entries {
            divergences.push(format!(
                "[{}] index_entries\n  expected: {:#?}\n  actual:   {entries:#?}",
                project.name, project.expect.index_entries
            ));
        }

        let genindex: Vec<GenIndexGroup> = snapshot_field(env, "genindex");
        if genindex != project.expect.genindex {
            divergences.push(format!(
                "[{}] genindex\n  expected: {:#?}\n  actual:   {genindex:#?}",
                project.name, project.expect.genindex
            ));
        }
    }

    report(&divergences, "index_entries, genindex");
}

// ---------------------------------------------------------------------------
// Live: wave-4.5 task 14 keys
// ---------------------------------------------------------------------------

/// `env.domaindata['py']` — objects and modules as registration-ordered
/// record lists — and the py-modindex `PythonModuleIndex.generate()`
/// derives from them. The record ORDER is part of the comparison: Sphinx's
/// fuzzy resolution pass iterates the registry in insertion order and takes
/// the first match, so a re-sorted registry would resolve differently.
#[test]
fn py_domain_registries_and_modindex_match_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);

        let objects: Vec<PyObjectExpect> = snapshot_field(env, "py_objects");
        if objects != project.expect.py_objects {
            divergences.push(format!(
                "[{}] py_objects\n  expected: {:#?}\n  actual:   {objects:#?}",
                project.name, project.expect.py_objects
            ));
        }

        let modules: Vec<PyModuleExpect> = snapshot_field(env, "py_modules");
        if modules != project.expect.py_modules {
            divergences.push(format!(
                "[{}] py_modules\n  expected: {:#?}\n  actual:   {modules:#?}",
                project.name, project.expect.py_modules
            ));
        }

        let modindex: PyModindexExpect = snapshot_field(env, "py_modindex");
        if modindex != project.expect.py_modindex {
            divergences.push(format!(
                "[{}] py_modindex\n  expected: {:#?}\n  actual:   {modindex:#?}",
                project.name, project.expect.py_modindex
            ));
        }
    }

    report(&divergences, "py_objects, py_modules, py_modindex");
}

/// `env.dependencies` and `env.included` — which files each document is
/// built from besides its own source, and which documents each document
/// textually includes.
///
/// Dependency paths are compared `<project>`-normalized. The value lists
/// are order-insensitive on both sides (the oracle sorts normalized
/// strings, this side's `BTreeSet<PathBuf>` sorts by component), so ours
/// are re-sorted as strings before the comparison — set membership, not
/// ordering, is the oracle data here.
#[test]
fn dependencies_and_included_match_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);

        let mut dependencies: BTreeMap<String, Vec<String>> = snapshot_field(env, "dependencies");
        for paths in dependencies.values_mut() {
            paths.sort_unstable();
        }
        if dependencies != project.expect.dependencies {
            divergences.push(format!(
                "[{}] dependencies\n  expected: {:?}\n  actual:   {dependencies:?}",
                project.name, project.expect.dependencies
            ));
        }

        let included: BTreeMap<String, Vec<String>> = snapshot_field(env, "included");
        if included != project.expect.included {
            divergences.push(format!(
                "[{}] included\n  expected: {:?}\n  actual:   {included:?}",
                project.name, project.expect.included
            ));
        }
    }

    report(&divergences, "dependencies, included");
}

/// `env.get_and_resolve_doctree(docname, builder)` as pseudo-XML — every
/// document's doctree after cross-reference resolution.
///
/// Compared per document, because only part of that pass exists: the
/// `pending_xref` resolution this task ports is live, while the write-phase
/// *toctree* resolution (`adapters/toctree.py`) that turns a `toctree` node
/// into a `compact_paragraph` tree is not — so every document carrying a
/// toctree, plus the handful with read-phase gaps of their own, is pinned in
/// [`KNOWN_RESOLVED_GAPS`] rather than compared.
#[test]
fn resolved_doctrees_match_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();
    let mut visited_gaps: Vec<(&str, &str)> = Vec::new();
    let mut visited_stamps: Vec<(&str, &str)> = Vec::new();

    for project in &fixture.projects {
        let env = env_of(project);
        let resolved: BTreeMap<String, String> = snapshot_field(env, "resolved_pformat");

        assert_eq!(
            resolved.keys().collect::<Vec<_>>(),
            project.expect.resolved_pformat.keys().collect::<Vec<_>>(),
            "[{}] the build resolved a different document set than the oracle",
            project.name
        );

        for (docname, expected) in &project.expect.resolved_pformat {
            // Both sides pass through the §Scope-8 canonicalization; see
            // [`canon_scope8`] for why this is comparison-preserving.
            let expected = canon_scope8(&expected.replace(TRANSLATION_PROGRESS_ATTR, ""));
            let actual = canon_scope8(&resolved[docname]);
            let matches = actual == expected;
            let stamp_exempt = known_highlight_stamp_gap(&project.name, docname);

            match known_resolved_gap(&project.name, docname) {
                Some(why) => {
                    assert!(
                        !stamp_exempt,
                        "[{}] {docname} is in both KNOWN_RESOLVED_GAPS ({why}) and \
                         KNOWN_HIGHLIGHT_STAMP_GAPS — a skipped document cannot also \
                         be compared modulo the highlight stamp; keep one",
                        project.name
                    );
                    visited_gaps.push((project.name.as_str(), docname.as_str()));
                    assert!(
                        !matches,
                        "[{}] {docname}: listed in KNOWN_RESOLVED_GAPS ({why}) but \
                         the resolved doctree now matches the oracle — delete the \
                         exemption",
                        project.name
                    );
                }
                None if stamp_exempt => {
                    visited_stamps.push((project.name.as_str(), docname.as_str()));
                    assert!(
                        !matches,
                        "[{}] {docname}: listed in KNOWN_HIGHLIGHT_STAMP_GAPS but the \
                         resolved doctree now matches the oracle outright — delete the \
                         exemption",
                        project.name
                    );
                    let unstamped = drop_unstamped_highlight_attrs(&expected, &actual);
                    if actual != unstamped {
                        divergences.push(format!(
                            "[{}] {docname} (beyond the HighlightLanguageTransform \
                             stamp)\n--- oracle, unstamped ---\n{unstamped}\
                             --- ours ---\n{actual}",
                            project.name
                        ));
                    }
                }
                None if !matches => divergences.push(format!(
                    "[{}] {docname}\n--- oracle ---\n{expected}--- ours ---\n{actual}",
                    project.name
                )),
                None => {}
            }
        }
    }

    for (project, docname, why) in KNOWN_RESOLVED_GAPS {
        assert!(
            visited_gaps.contains(&(*project, *docname)),
            "KNOWN_RESOLVED_GAPS entry ({project}, {docname}) — {why} — was \
             never visited: no such project/document in the fixture. Delete it."
        );
    }
    for (project, docname) in KNOWN_HIGHLIGHT_STAMP_GAPS {
        assert!(
            visited_stamps.contains(&(*project, *docname)),
            "KNOWN_HIGHLIGHT_STAMP_GAPS entry ({project}, {docname}) was never \
             visited: no such project/document in the fixture. Delete it."
        );
    }

    report(&divergences, "resolved_pformat");
}

/// The exemption arithmetic the docs quote (docs/IMPLEMENTATION_STATUS.md,
/// ROADMAP.md, CHANGELOG.md, README.md), computed from the tables and the
/// fixture rather than by hand (panel fix round B, [29]): how many
/// documents' resolved doctrees and how many projects' warning streams are
/// compared byte-for-byte, how many documents are skipped wholesale, and
/// how many are compared modulo the highlight stamp. The subtraction is
/// sound on its own terms — the test first proves that each table names a
/// fixture document (or project) exactly once and that the two document
/// tables are disjoint — so the table lengths ARE the exemption counts.
/// Update the seven constants and the doc sites together.
const DOCUMENTED_PROJECTS: usize = 32;
const DOCUMENTED_DOCUMENTS: usize = 89;
const DOCUMENTED_WHOLESALE_EXEMPT_DOCUMENTS: usize = 45;
const DOCUMENTED_STAMP_EXEMPT_DOCUMENTS: usize = 8;
const DOCUMENTED_BYTE_EXACT_DOCUMENTS: usize = 36;
const DOCUMENTED_WARNING_EXEMPT_PROJECTS: usize = 4;
const DOCUMENTED_BYTE_EXACT_WARNING_PROJECTS: usize = 28;

#[test]
fn exemption_arithmetic_matches_the_documented_numbers() {
    let fixture = load_fixture();
    let projects: BTreeSet<&str> = fixture.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        projects.len(),
        fixture.projects.len(),
        "a project name repeats"
    );
    let documents: BTreeSet<(&str, &str)> = fixture
        .projects
        .iter()
        .flat_map(|project| {
            project
                .expect
                .resolved_pformat
                .keys()
                .map(move |docname| (project.name.as_str(), docname.as_str()))
        })
        .collect();

    let wholesale: BTreeSet<(&str, &str)> = KNOWN_RESOLVED_GAPS
        .iter()
        .map(|(project, docname, _)| (*project, *docname))
        .collect();
    let stamp: BTreeSet<(&str, &str)> = KNOWN_HIGHLIGHT_STAMP_GAPS.iter().copied().collect();
    let warning_exempt: BTreeSet<&str> = KNOWN_WARNING_GAPS.iter().map(|(p, _)| *p).collect();
    assert_eq!(
        wholesale.len(),
        KNOWN_RESOLVED_GAPS.len(),
        "KNOWN_RESOLVED_GAPS lists a document twice"
    );
    assert_eq!(
        stamp.len(),
        KNOWN_HIGHLIGHT_STAMP_GAPS.len(),
        "KNOWN_HIGHLIGHT_STAMP_GAPS lists a document twice"
    );
    assert_eq!(
        warning_exempt.len(),
        KNOWN_WARNING_GAPS.len(),
        "KNOWN_WARNING_GAPS lists a project twice"
    );
    assert!(
        wholesale.is_disjoint(&stamp),
        "a document is in both document tables: {:?}",
        wholesale.intersection(&stamp).collect::<Vec<_>>()
    );
    assert!(
        wholesale.is_subset(&documents) && stamp.is_subset(&documents),
        "a document table names a document the fixture does not have"
    );
    assert!(
        warning_exempt.is_subset(&projects),
        "KNOWN_WARNING_GAPS names a project the fixture does not have"
    );

    let byte_exact_documents = documents.len() - wholesale.len() - stamp.len();
    let byte_exact_warning_projects = projects.len() - warning_exempt.len();
    assert_eq!(
        (
            projects.len(),
            documents.len(),
            wholesale.len(),
            stamp.len(),
            byte_exact_documents,
            warning_exempt.len(),
            byte_exact_warning_projects,
        ),
        (
            DOCUMENTED_PROJECTS,
            DOCUMENTED_DOCUMENTS,
            DOCUMENTED_WHOLESALE_EXEMPT_DOCUMENTS,
            DOCUMENTED_STAMP_EXEMPT_DOCUMENTS,
            DOCUMENTED_BYTE_EXACT_DOCUMENTS,
            DOCUMENTED_WARNING_EXEMPT_PROJECTS,
            DOCUMENTED_BYTE_EXACT_WARNING_PROJECTS,
        ),
        "the corpus or a table changed: {} projects, {} documents, {} wholesale-exempt, \
         {} stamp-exempt ({byte_exact_documents} documents byte-for-byte), {} \
         warning-exempt projects ({byte_exact_warning_projects} warning streams \
         byte-for-byte) — update the constants AND the doc sites \
         (docs/IMPLEMENTATION_STATUS.md, ROADMAP.md, CHANGELOG.md, README.md)",
        projects.len(),
        documents.len(),
        wholesale.len(),
        stamp.len(),
        warning_exempt.len(),
    );
}

/// The build must not leave the environment's per-document state doubled up
/// across incremental rebuilds: `toctree_includes` is `extend`ed per toctree
/// node, so a re-read that forgets to clear the document first silently
/// duplicates every entry.
#[test]
fn rebuilding_a_project_does_not_accumulate_environment_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   a\n",
    )
    .unwrap();
    std::fs::write(source_dir.join("a.rst"), "A\n=\n\nBody.\n").unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let build_once = |source: &Path, output: &Path| -> serde_json::Value {
        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source.to_path_buf(),
            output.to_path_buf(),
        )
        .unwrap();
        builder.enable_incremental();
        runtime.block_on(builder.build()).unwrap();
        builder.snapshot_env()
    };

    let first = build_once(&source_dir, &output_dir);
    let second = build_once(&source_dir, &output_dir);

    assert_eq!(
        second["toctree_includes"], first["toctree_includes"],
        "a warm rebuild must reproduce the toctree graph, not append to it"
    );
    assert_eq!(second["files_to_rebuild"], first["files_to_rebuild"]);
    assert_eq!(second["tocs_pformat"], first["tocs_pformat"]);

    // A document that disappears must not leave state behind.
    std::fs::remove_file(source_dir.join("a.rst")).unwrap();
    let third = build_once(&source_dir, &output_dir);
    assert!(
        third["tocs_pformat"].get("a").is_none(),
        "removed documents must be cleared from the environment: {}",
        third["tocs_pformat"]
    );
}

// ---------------------------------------------------------------------------
// Incremental rebuilds: what the build re-reads, and what that leaves the
// environment holding
// ---------------------------------------------------------------------------

/// One incremental build of `source` into `output` — what `--incremental`
/// (and compat-mode `sphinx-build`, which is incremental by default) runs.
///
/// Returns the number of documents the build did *not* have to read (its
/// cache hits), the environment snapshot, and the warnings, rendered with
/// the source root replaced by `<project>`.
fn incremental_build(source: &Path, output: &Path) -> (usize, serde_json::Value, Vec<String>) {
    let mut builder = SphinxBuilder::new(
        BuildConfig::default(),
        source.to_path_buf(),
        output.to_path_buf(),
    )
    .unwrap();
    builder.enable_incremental();
    let stats = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(builder.build())
        .unwrap();
    let root = source.to_string_lossy().into_owned();
    let warnings = normalize_warnings(&stats.warning_details, &root);
    // The snapshot stays as the build produced it: its callers compare a
    // warm build against a cold one from this same helper, so both sides
    // carry the platform's own paths and agree by construction. Only the
    // warnings are matched against POSIX-shaped expected strings.
    (stats.cache_hits, builder.snapshot_env(), warnings)
}

/// The snapshot with `all_docs`' read timestamps blanked out.
///
/// An incremental rebuild deliberately keeps the timestamp written by the
/// build that last read each document, so those values cannot match a cold
/// build's — while *which* documents are in the map, and every other field,
/// must.
fn without_read_times(env: &serde_json::Value) -> serde_json::Value {
    let mut env = env.clone();
    let all_docs = env["all_docs"].as_object_mut().expect("all_docs is a map");
    for value in all_docs.values_mut() {
        *value = serde_json::Value::Null;
    }
    env
}

fn write(source_dir: &Path, docname: &str, body: &str) {
    let path = source_dir.join(format!("{docname}.rst"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// The heart of the incremental contract: a rebuild that re-reads only the
/// document that changed has to leave the environment in exactly the state
/// a build that read everything would have left it in. Anything less and
/// "incremental" means "sometimes wrong".
///
/// Two documents, one touched: the untouched one is not read (it comes back
/// as a cache hit), and the resulting environment — toctree graph,
/// numbering, std domain, index, resolved doctrees — equals a cold build's
/// down to the read timestamps that cannot be equal by construction.
#[test]
fn touching_one_document_re_reads_only_it_and_the_environment_still_matches_a_cold_build() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    write(
        &source_dir,
        "index",
        "Index\n=====\n\n.. toctree::\n\n   a\n   b\n",
    );
    write(
        &source_dir,
        "a",
        "A\n=\n\n.. _label-a:\n\nSection A\n---------\n",
    );
    write(&source_dir, "b", "B\n=\n\nSee :ref:`label-a`.\n");
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let warm_out = tmp.path().join("warm");
    let (cold_hits, first_env, _) = incremental_build(&source_dir, &warm_out);
    assert_eq!(cold_hits, 0, "a cold build cannot hit the cache");

    let (steady_hits, steady_env, _) = incremental_build(&source_dir, &warm_out);
    assert_eq!(
        steady_hits, 3,
        "an unchanged project re-reads nothing at all"
    );
    assert_eq!(
        steady_env["all_docs"], first_env["all_docs"],
        "`all_docs` records when each document was *read*: a rebuild that \
         read nothing must not restamp it"
    );

    // Touch one document: rewriting the file moves its mtime past the read
    // time recorded for it.
    write(&source_dir, "b", "B\n=\n\nSee :ref:`label-a` twice.\n");
    let (hits, incremental_env, incremental_warnings) = incremental_build(&source_dir, &warm_out);
    assert_eq!(
        hits, 2,
        "only the touched document is read; the other two are hits"
    );
    assert_eq!(
        incremental_env["all_docs"]["index"], first_env["all_docs"]["index"],
        "an untouched document keeps the read time it was read at"
    );
    assert_eq!(incremental_env["all_docs"]["a"], first_env["all_docs"]["a"]);
    assert_ne!(
        incremental_env["all_docs"]["b"], first_env["all_docs"]["b"],
        "the touched document was read again, so its read time advanced"
    );

    // The reference build: same sources, an output directory that has never
    // been built into, so nothing is cached and everything is read.
    let (cold_hits, cold_env, cold_warnings) =
        incremental_build(&source_dir, &tmp.path().join("cold"));
    assert_eq!(cold_hits, 0);

    assert_eq!(
        without_read_times(&incremental_env),
        without_read_times(&cold_env),
        "an incremental rebuild's environment must equal a cold build's"
    );
    assert_eq!(incremental_warnings, cold_warnings);

    // Now touch the document that *owns* the label. Its own entry has to be
    // cleared before it is read again, or re-registering the label finds
    // the previous build's copy of itself and warns about a duplicate that
    // does not exist.
    write(
        &source_dir,
        "a",
        "A\n=\n\n.. _label-a:\n\nSection A\n---------\n\nMore.\n",
    );
    let (hits, env, warnings) = incremental_build(&source_dir, &warm_out);
    assert_eq!(hits, 2);
    assert!(
        warnings.is_empty(),
        "re-reading a label's own document must not make it a duplicate of \
         itself: {warnings:?}"
    );
    assert_eq!(env["std"]["labels"], cold_env["std"]["labels"]);
}

/// `(name, docname, node_id)` of every `py_objects` snapshot record, in
/// the registration order the snapshot preserves.
fn py_object_rows(env: &serde_json::Value) -> Vec<(String, String, String)> {
    env["py_objects"]
        .as_array()
        .expect("py_objects is a list")
        .iter()
        .map(|o| {
            (
                o["name"].as_str().unwrap().to_string(),
                o["docname"].as_str().unwrap().to_string(),
                o["node_id"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn rows(v: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    v.iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect()
}

/// The py domain across incremental rebuilds, pinned cell by cell to what
/// a real sphinx 9.1.0 build does with this exact project (dummy builder,
/// touching one file at a time):
///
/// - cold: `a` registers `dup` first (docname order), `b`'s re-definition
///   warns naming `a` and wins **in `a`'s insertion slot**;
/// - steady warm: nothing is read, so nothing warns and nothing moves;
/// - touch `a` (whose registration lost): only `a` is re-read; clearing it
///   leaves `b`'s surviving `dup`, so the replay finds it and the warning
///   RE-FIRES from `a` naming `b` — and `a`'s other entries move to the
///   end of the registration order, exactly like Sphinx's dict after
///   clear + re-insert. Warm therefore does NOT equal cold here, because
///   it doesn't in Sphinx either;
/// - touch `b` (whose registration won): clearing `b` removes `dup`
///   entirely, the replay re-registers it without a prior entry, and no
///   warning fires at all.
#[test]
fn py_registrations_across_incremental_rebuilds_match_sphinx_s_clear_and_replay() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    write(
        &source_dir,
        "index",
        "Idx\n===\n\n.. toctree::\n\n   a\n   b\n",
    );
    let doc_a = "A\n=\n\n.. py:function:: dup()\n\n.. py:module:: alpha\n";
    let doc_b = "B\n=\n\n.. py:function:: dup()\n\n.. py:module:: beta\n";
    write(&source_dir, "a", doc_a);
    write(&source_dir, "b", doc_b);
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let duplicate_warning = |docname: &str, other: &str| {
        format!(
            "<project>/{docname}.rst:4: WARNING: duplicate object description of dup, \
             other instance in {other}, use :no-index: for one of them"
        )
    };

    let out = tmp.path().join("out");
    let (_, cold_env, cold_warnings) = incremental_build(&source_dir, &out);
    assert_eq!(cold_warnings, vec![duplicate_warning("b", "a")]);
    assert_eq!(
        py_object_rows(&cold_env),
        rows(&[
            ("dup", "b", "dup"),
            ("alpha", "a", "module-alpha"),
            ("beta", "b", "module-beta"),
        ])
    );
    assert_eq!(
        cold_env["py_modules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["name"].as_str().unwrap(), m["docname"].as_str().unwrap()))
            .collect::<Vec<_>>(),
        vec![("alpha", "a"), ("beta", "b")]
    );

    // Steady state: three cache hits, no replay, warm == cold everywhere.
    let (hits, steady_env, steady_warnings) = incremental_build(&source_dir, &out);
    assert_eq!(hits, 3);
    assert!(
        steady_warnings.is_empty(),
        "an unread document re-fires no duplicate warnings: {steady_warnings:?}"
    );
    assert_eq!(steady_env["py_objects"], cold_env["py_objects"]);
    assert_eq!(steady_env["py_modules"], cold_env["py_modules"]);

    // Touch the document whose registration LOST the duplicate.
    write(&source_dir, "a", doc_a);
    let (hits, env, warnings) = incremental_build(&source_dir, &out);
    assert_eq!(hits, 2);
    assert_eq!(
        warnings,
        vec![duplicate_warning("a", "b")],
        "the re-read finds b's surviving entry, so the warning re-fires \
         naming b — sphinx does exactly this"
    );
    assert_eq!(
        py_object_rows(&env),
        rows(&[
            // `dup` keeps slot 0 (b's entry was overwritten in place) but
            // now belongs to `a`; `alpha` was cleared and re-appended.
            ("dup", "a", "dup"),
            ("beta", "b", "module-beta"),
            ("alpha", "a", "module-alpha"),
        ])
    );

    // Touch the document whose registration WON, against a fresh build.
    let out2 = tmp.path().join("out2");
    let (_, _, cold2_warnings) = incremental_build(&source_dir, &out2);
    assert_eq!(cold2_warnings.len(), 1);
    write(&source_dir, "b", doc_b);
    let (hits, env, warnings) = incremental_build(&source_dir, &out2);
    assert_eq!(hits, 2);
    assert!(
        warnings.is_empty(),
        "clearing b removed the only `dup` entry, so its replay registers \
         silently: {warnings:?}"
    );
    assert_eq!(
        py_object_rows(&env),
        rows(&[
            // Clearing b removed `dup` from slot 0; the replay re-appends
            // it after the surviving `alpha`.
            ("alpha", "a", "module-alpha"),
            ("dup", "b", "dup"),
            ("beta", "b", "module-beta"),
        ])
    );
}

/// A document that disappears is cleared from the environment, and both
/// diagnostics a cold build would now report show up: the reference into
/// the deleted document dangles (resolution, recomputed for every document
/// anyway) and the toctree that listed it does too (read-time, which is why
/// its container is read again).
#[test]
fn deleting_a_document_clears_it_and_updates_the_warnings() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    write(
        &source_dir,
        "index",
        "Index\n=====\n\n.. toctree::\n\n   a\n   b\n",
    );
    write(&source_dir, "a", "A\n=\n\n.. _shared:\n\nAnchor\n------\n");
    write(&source_dir, "b", "B\n=\n\nSee :ref:`shared`.\n");
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let (_, first_env, first_warnings) = incremental_build(&source_dir, &output_dir);
    assert!(first_warnings.is_empty(), "{first_warnings:?}");
    assert!(first_env["std"]["labels"].get("shared").is_some());

    std::fs::remove_file(source_dir.join("a.rst")).unwrap();
    let (hits, env, warnings) = incremental_build(&source_dir, &output_dir);

    assert_eq!(
        hits, 1,
        "`b` is a hit; `index` listed the deleted document in its toctree, \
         so it is read again"
    );
    assert_eq!(
        env["all_docs"]["b"], first_env["all_docs"]["b"],
        "a deletion elsewhere does not restamp the documents that stayed"
    );
    assert!(
        env["all_docs"].get("a").is_none() && env["tocs_pformat"].get("a").is_none(),
        "a deleted document must leave no trace: {env}"
    );
    assert!(
        env["std"]["labels"].get("shared").is_none(),
        "its labels go with it: {}",
        env["std"]["labels"]
    );
    assert_eq!(
        env["toctree_includes"]["index"],
        serde_json::json!(["b"]),
        "the re-read container no longer claims the deleted document"
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:4: WARNING: toctree contains reference to nonexisting \
             document 'a' [toc.not_readable]"
                .to_string(),
            "<project>/b.rst:4: WARNING: undefined label: 'shared' [ref.ref]".to_string(),
        ],
        "both the toctree entry and the reference into the deleted document \
         now dangle — exactly what a cold build of this tree reports"
    );

    // And that claim is checked, not assumed.
    let (_, cold_env, cold_warnings) = incremental_build(&source_dir, &tmp.path().join("cold"));
    assert_eq!(warnings, cold_warnings);
    assert_eq!(without_read_times(&env), without_read_times(&cold_env));
}

/// A toctree entry naming a document that does not exist puts its container
/// in `reread_always` (sphinx's `env.note_reread()`): the container is read
/// on every build until the missing document turns up, which is what lets
/// the warning stop the moment it does.
#[test]
fn a_dangling_toctree_entry_re_reads_its_container_until_the_document_appears() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    write(
        &source_dir,
        "index",
        "Index\n=====\n\n.. toctree::\n\n   a\n   later\n",
    );
    write(&source_dir, "a", "A\n=\n\nBody.\n");
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let dangling = vec![
        "<project>/index.rst:4: WARNING: toctree contains reference to \
                         nonexisting document 'later' [toc.not_readable]"
            .to_string(),
    ];

    let (_, env, warnings) = incremental_build(&source_dir, &output_dir);
    assert_eq!(warnings, dangling);
    assert_eq!(env["reread_always"], serde_json::json!(["index"]));

    let (hits, _, warnings) = incremental_build(&source_dir, &output_dir);
    assert_eq!(
        hits, 1,
        "`index` is read again however unchanged it is; `a` is a hit"
    );
    assert_eq!(warnings, dangling, "and it says the same thing");

    // The missing document turns up: the container reads it, stops warning,
    // and stops asking to be re-read.
    write(&source_dir, "later", "Later\n=====\n\nBody.\n");
    let (_, env, warnings) = incremental_build(&source_dir, &output_dir);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        env["toctree_includes"]["index"],
        serde_json::json!(["a", "later"])
    );
    assert_eq!(env["reread_always"], serde_json::json!([]));

    let (hits, _, _) = incremental_build(&source_dir, &output_dir);
    assert_eq!(hits, 3, "nothing is outdated any more");
}

/// A globbed toctree's entries depend on which files exist, not on its own
/// text — so a build that adds or removes any file has to re-read every
/// document that has one, even though none of them changed.
#[test]
fn adding_a_file_re_reads_the_globbed_toctree_that_would_have_matched_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    write(
        &source_dir,
        "index",
        "Index\n=====\n\n.. toctree::\n   :glob:\n\n   pages/*\n",
    );
    write(&source_dir, "pages/a", "A\n=\n\nBody.\n");
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let (_, first_env, _) = incremental_build(&source_dir, &output_dir);
    assert_eq!(
        first_env["toctree_includes"]["index"],
        serde_json::json!(["pages/a"])
    );

    write(&source_dir, "pages/b", "B\n=\n\nBody.\n");
    let (hits, env, _) = incremental_build(&source_dir, &output_dir);

    assert_eq!(
        hits, 1,
        "the new document is read and so is the glob container; only \
         `pages/a` is a hit"
    );
    assert_eq!(
        env["toctree_includes"]["index"],
        serde_json::json!(["pages/a", "pages/b"]),
        "the re-read container picked the new document up"
    );
}

/// A document whose *source* never changed is still outdated when a file it
/// pulls in has: this is what `env.dependencies` is for.
#[test]
fn touching_an_image_re_reads_only_the_document_that_embeds_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    write(
        &source_dir,
        "index",
        "Index\n=====\n\n.. toctree::\n\n   a\n   b\n",
    );
    write(&source_dir, "a", "A\n=\n\n.. image:: pic.png\n");
    write(&source_dir, "b", "B\n=\n\nBody.\n");
    let picture = source_dir.join("pic.png");
    std::fs::write(&picture, b"first").unwrap();
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let (_, env, _) = incremental_build(&source_dir, &output_dir);
    assert_eq!(
        env["dependencies"]["a"],
        serde_json::json!([source_dir.join("pic.png")]),
        "the image is recorded as a dependency of the document that embeds it"
    );

    let (steady, _, _) = incremental_build(&source_dir, &output_dir);
    assert_eq!(steady, 3, "nothing changed: nothing is read");

    std::fs::write(&picture, b"second").unwrap();
    let (hits, _, _) = incremental_build(&source_dir, &output_dir);
    assert_eq!(
        hits, 2,
        "the document embedding the touched image is re-read; the others hit"
    );

    let (settled, _, _) = incremental_build(&source_dir, &output_dir);
    assert_eq!(
        settled, 3,
        "the re-read recorded a newer read time, so the image is no longer \
         newer than it"
    );
}

/// `:orphan:` exempts a document from the orphan warning even when a
/// `PreBibliographic` node precedes the field list — `raw` being the one
/// such node a document can produce before any transform runs. Getting the
/// skip set wrong turns every `.. raw::`-led orphan into a false
/// `toc.not_included` warning, and into a failing build under `-W`.
#[test]
fn an_orphan_marked_after_a_raw_block_is_still_exempt() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("index.rst"), "Index\n=====\n\nRoot.\n").unwrap();
    std::fs::write(
        source_dir.join("aside.rst"),
        ".. raw:: html\n\n   <hr>\n\n:orphan:\n\nAside\n=====\n\nBody.\n",
    )
    .unwrap();

    let mut builder =
        SphinxBuilder::new(BuildConfig::default(), source_dir, tmp.path().join("build")).unwrap();
    let stats = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(builder.build())
        .unwrap();

    let warnings: Vec<String> = stats
        .warning_details
        .iter()
        .map(|warning| warning.render())
        .collect();
    assert!(
        warnings.is_empty(),
        "an `:orphan:` document must not warn, whatever precedes its field \
         list: {warnings:?}"
    );
}

/// Build one inline project and return its warnings, rendered.
fn warnings_of(files: &[(&str, &str)], config: BuildConfig) -> Vec<String> {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    for (docname, body) in files {
        std::fs::write(source_dir.join(format!("{docname}.rst")), body).unwrap();
    }
    let mut builder = SphinxBuilder::new(config, source_dir, tmp.path().join("build")).unwrap();
    let stats = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(builder.build())
        .unwrap();
    stats
        .warning_details
        .iter()
        .map(|warning| warning.render())
        .collect()
}

/// Sphinx's non-xref roles (`roles.py:28-36` generic_docroles and
/// `:608-626` specific_docroles) produce plain inline nodes and resolve
/// nothing. This crate still parses them as `pending_xref` — a recorded
/// wave-3 gap — so what keeps them quiet is that they are not
/// `warn_dangling`. Treating every unknown std role as one made a document
/// that merely mentioned a keystroke warn `'kbd' reference target not
/// found`.
#[test]
fn sphinxs_non_xref_roles_do_not_report_missing_references() {
    let warnings = warnings_of(
        &[(
            "index",
            "Index\n=====\n\nPress :kbd:`Ctrl-C`, open :file:`~/.bashrc`, click \
             :guilabel:`Save`,\nrun :command:`ls`, mind the :abbr:`LIFO (last in, \
             first out)` order,\nand see :program:`rm`, :samp:`print {x}`, \
             :menuselection:`File --> Open`,\n:dfn:`a defined term`, \
             :mimetype:`text/plain`, :regexp:`^a.*z$`, :manpage:`ls(1)`.\n",
        )],
        BuildConfig::default(),
    );

    assert!(
        warnings.is_empty(),
        "no non-xref role may report a missing reference: {warnings:?}"
    );
}

/// `:numref:` is registered with `lowercase=True`, so a `:name:` that was
/// written in mixed case still resolves (docutils lowercases the label it
/// registers, and the role has to match).
#[test]
fn a_numref_target_is_lowercased_like_the_label_it_names() {
    let config = BuildConfig {
        numfig: true,
        ..BuildConfig::default()
    };
    let warnings = warnings_of(
        &[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n   b\n"),
            (
                "a",
                "A\n=\n\n.. figure:: pic.png\n   :name: Fig-A\n\n   The Caption\n",
            ),
            ("b", "B\n=\n\nSee :numref:`FIG-A` and :ref:`Fig-A`.\n"),
        ],
        config,
    );

    assert!(
        warnings.is_empty(),
        "a mixed-case numref/ref target must resolve: {warnings:?}"
    );
}

/// What a warm rebuild says about a project it does not have to read.
///
/// The two halves come apart: **collection** warnings (duplicate labels)
/// are raised while a document is read, so a rebuild that reads nothing
/// does not raise them again — sphinx behaves the same way, and only ever
/// reported them twice here because this crate used to re-read everything.
/// **Resolution** warnings are recomputed for every document on every
/// build, because every document is written on every build: they come back
/// unchanged, off the persisted doctree and the cached source text. The
/// environment itself — labels, terms, the program-scoped option, which
/// live only in the parse export — has to survive the round trip through
/// `env.bin` intact.
#[test]
fn a_warm_rebuild_reports_the_same_std_domain_warnings() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   a\n   b\n",
    )
    .unwrap();
    // The program/option pair at the end of `a` and the `:option:` in `b`
    // are here for the *cache* round trip: a program option's scope lives
    // only in the parse export (`RegistryExport::program_options`), so a
    // warm rebuild that skipped the parse has to recover it from the cached
    // document or `progoptions` comes back empty. Both are appended after
    // the existing content so the warning line numbers below are unmoved.
    std::fs::write(
        source_dir.join("a.rst"),
        "A\n=\n\n.. _dup:\n\nOne\n---\n\nSee :doc:`nope`.\n\n         .. program:: myprog\n\n.. option:: --verbose\n\n   Verbose output.\n",
    )
    .unwrap();
    std::fs::write(
        source_dir.join("b.rst"),
        "B\n=\n\n.. _dup:\n\nTwo\n---\n\nSee :ref:`dup` and :term:`nothing`.\n\n         Also :option:`myprog --verbose`.\n",
    )
    .unwrap();
    // Warning locations come from the canonicalized source tree (macOS
    // resolves `/var/...` to `/private/var/...`).
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let build_once = || -> (Vec<String>, serde_json::Value) {
        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.clone(),
            output_dir.clone(),
        )
        .unwrap();
        builder.enable_incremental();
        let stats = runtime.block_on(builder.build()).unwrap();
        let root = source_dir.to_string_lossy().into_owned();
        (
            normalize_warnings(&stats.warning_details, &root),
            // Raw, for the same reason as in `incremental_build`: this
            // snapshot is only ever compared with another from this closure.
            builder.snapshot_env(),
        )
    };

    let (cold, cold_env) = build_once();
    assert_eq!(
        cold_env["std"]["progoptions"],
        serde_json::json!([{
            "program": "myprog",
            "name": "--verbose",
            "docname": "a",
            "labelid": "cmdoption-myprog-verbose",
        }]),
        "the `.. program::` scope must reach the std domain"
    );
    assert_eq!(
        cold,
        vec![
            "<project>/b.rst:7: WARNING: duplicate label dup, other instance in <project>/a.rst"
                .to_string(),
            "<project>/a.rst:9: WARNING: unknown document: 'nope' [ref.doc]".to_string(),
            "<project>/b.rst:9: WARNING: term not in glossary: 'nothing' [ref.term]".to_string(),
        ],
        "collection warnings come out with the document that raised them, \
         resolution warnings after every document has been read"
    );

    let (warm, warm_env) = build_once();
    assert_eq!(
        warm_env["std"], cold_env["std"],
        "a warm rebuild must reproduce the std-domain registries, not \
         accumulate them"
    );
    assert_eq!(
        warm,
        vec![
            // The duplicate-label warning is missing on purpose: it is
            // raised *while reading* `b`, and this build read nothing. The
            // duplicate is still recorded — `warm_env["std"]` above is the
            // cold build's, `dup` pointing at `a` either way — it just is
            // not news any more. Sphinx says the same thing about the same
            // rebuild.
            "<project>/a.rst:9: WARNING: unknown document: 'nope' [ref.doc]".to_string(),
            "<project>/b.rst:9: WARNING: term not in glossary: 'nothing' [ref.term]".to_string(),
        ],
        "the resolution warnings must survive a warm rebuild unchanged: every \
         document is resolved on every build, from the persisted doctree and \
         the cached source text"
    );
}

/// `Cmdoption.handle_signature`'s malformed-option diagnostic goes to the
/// logger, not the tree, so it has to ride the parse records to reach the
/// build's warning list. A rebuild that reads nothing does not repeat it —
/// it is a *read*-phase diagnostic — but the registration it made survives
/// in the environment, and re-reading the document brings the diagnostic
/// back with it.
/// Text and location are byte-checked against a sphinx 9.1.0 build of the
/// same source, which reports:
///
/// ```text
/// a.rst:4: WARNING: Malformed option description '=bad', should look like "opt", "-opt args", "--opt args", "/opt args" or "+opt args"
/// a.rst:8: WARNING: Malformed option description '', should look like "opt", "-opt args", "--opt args", "/opt args" or "+opt args"
/// ```
#[test]
fn a_malformed_option_description_warns_like_sphinx_across_a_rebuild() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   a\n",
    )
    .unwrap();
    // The second directive is malformed in its FIRST spelling only: sphinx
    // warns and still registers `--ok`.
    std::fs::write(
        source_dir.join("a.rst"),
        "A\n=\n\n.. option:: =bad\n\n   Body.\n\n.. option:: , --ok\n\n   Body.\n",
    )
    .unwrap();
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let build_once = || -> (Vec<String>, serde_json::Value) {
        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.clone(),
            output_dir.clone(),
        )
        .unwrap();
        builder.enable_incremental();
        let stats = runtime.block_on(builder.build()).unwrap();
        let root = source_dir.to_string_lossy().into_owned();
        (
            normalize_warnings(&stats.warning_details, &root),
            // Raw, for the same reason as in `incremental_build`: this
            // snapshot is only ever compared with another from this closure.
            builder.snapshot_env(),
        )
    };

    let expected = vec![
        "<project>/a.rst:4: WARNING: Malformed option description '=bad', should look like \
         \"opt\", \"-opt args\", \"--opt args\", \"/opt args\" or \"+opt args\""
            .to_string(),
        "<project>/a.rst:8: WARNING: Malformed option description '', should look like \
         \"opt\", \"-opt args\", \"--opt args\", \"/opt args\" or \"+opt args\""
            .to_string(),
    ];
    let (cold, cold_env) = build_once();
    assert_eq!(cold, expected);
    assert_eq!(
        cold_env["std"]["progoptions"],
        serde_json::json!([{
            "program": null,
            "name": "--ok",
            "docname": "a",
            "labelid": "cmdoption-ok",
        }]),
        "the surviving spelling still registers"
    );

    let (warm, warm_env) = build_once();
    assert!(
        warm.is_empty(),
        "a rebuild that reads nothing raises no read-phase diagnostic: {warm:?}"
    );
    assert_eq!(
        warm_env["std"], cold_env["std"],
        "what the diagnostic's document registered is still in the environment"
    );

    // Touch the document: it is read again, and the diagnostic comes back
    // with the read that produces it.
    std::fs::write(
        source_dir.join("a.rst"),
        "A\n=\n\n.. option:: =bad\n\n   Body.\n\n.. option:: , --ok\n\n   Body.\n",
    )
    .unwrap();
    let (reread, reread_env) = build_once();
    assert_eq!(reread, expected);
    assert_eq!(reread_env["std"], cold_env["std"]);
}

/// `IndexDomain.process_doc` rejects a whole `index` node when *any* of its
/// entries fails `split_index_msg`: the good entries beside the bad one are
/// lost with it, the node leaves the doctree (its `target` stays), and the
/// diagnostic rides the cached parse across a rebuild.
///
/// The oracle corpus contains no invalid index entry, so the text, location
/// and `[index]` category are pinned here against a sphinx 9.1.0 build of
/// the same source, which reports:
///
/// ```text
/// a.rst:4: WARNING: invalid pair index entry 'lonely' [index]
/// ```
///
/// and leaves `env.domaindata['index']['entries']['a']` holding only the
/// `('single', 'Fine', 'index-1', '', None)` entry of the *second* node.
#[test]
fn an_invalid_index_entry_drops_its_whole_node_like_sphinx() {
    let tmp = tempfile::TempDir::new().unwrap();
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
        "A\n=\n\n.. index::\n   single: Good\n   pair: lonely\n\nBody.\n\n\
         .. index::\n   single: Fine\n\nMore.\n",
    )
    .unwrap();
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let build_once = || -> (Vec<String>, serde_json::Value) {
        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.clone(),
            output_dir.clone(),
        )
        .unwrap();
        builder.enable_incremental();
        let stats = runtime.block_on(builder.build()).unwrap();
        let root = source_dir.to_string_lossy().into_owned();
        (
            normalize_warnings(&stats.warning_details, &root),
            // Raw, for the same reason as in `incremental_build`: this
            // snapshot is only ever compared with another from this closure.
            builder.snapshot_env(),
        )
    };

    let expected_warnings =
        vec!["<project>/a.rst:4: WARNING: invalid pair index entry 'lonely' [index]".to_string()];
    let expected_entries = serde_json::json!({
        "a": [["single", "Fine", "index-1", "", null]],
        "index": [],
    });
    let expected_genindex = serde_json::json!([{
        "group": "F",
        "entries": [{
            "name": "Fine",
            "targets": [["", "#index-1"]],
            "subitems": [],
            "category_key": null,
        }],
    }]);

    let (cold, cold_env) = build_once();
    assert_eq!(cold, expected_warnings);
    assert_eq!(cold_env["index_entries"], expected_entries);
    assert_eq!(cold_env["genindex"], expected_genindex);
    // The rejected node is gone from the resolved doctree; the `target` the
    // directive emitted beside it is not.
    let resolved = cold_env["resolved_pformat"]["a"].as_str().unwrap();
    assert_eq!(
        resolved.matches("<index ").count(),
        1,
        "only the surviving index node may remain:\n{resolved}"
    );
    assert!(resolved.contains("'Fine'"), "{resolved}");
    assert!(!resolved.contains("'Good'"), "{resolved}");

    let (warm, warm_env) = build_once();
    assert!(
        warm.is_empty(),
        "the rejection happened while reading, and this build read nothing: {warm:?}"
    );
    assert_eq!(
        warm_env["resolved_pformat"], cold_env["resolved_pformat"],
        "the rejected node stays rejected: a rebuild resolves the doctree the \
         *read* left behind — index domain's removal included — not the one \
         the parser first produced"
    );
    assert_eq!(
        warm_env["index_entries"], expected_entries,
        "the entries that survived the rejection are still recorded"
    );
    assert_eq!(
        warm_env["genindex"], expected_genindex,
        "and the index assembled from them is unchanged"
    );

    // Re-reading the document re-runs the rejection, diagnostic included.
    std::fs::write(
        source_dir.join("a.rst"),
        "A\n=\n\n.. index::\n   single: Good\n   pair: lonely\n\nBody.\n\n\
         .. index::\n   single: Fine\n\nMore.\n",
    )
    .unwrap();
    let (reread, reread_env) = build_once();
    assert_eq!(reread, expected_warnings);
    assert_eq!(reread_env["index_entries"], expected_entries);
    assert_eq!(reread_env["genindex"], expected_genindex);
}

/// A toctree diagnostic is produced while the document is read, so a
/// rebuild that reads the document again has to report it again — off the
/// same parse records. (This document *is* read again on every build: a
/// dangling toctree entry puts its container in `reread_always`.)
#[test]
fn a_warm_rebuild_reports_the_same_toctree_warnings() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    let output_dir = tmp.path().join("build");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   gone\n",
    )
    .unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let build_once = || -> Vec<String> {
        let mut builder = SphinxBuilder::new(
            BuildConfig::default(),
            source_dir.clone(),
            output_dir.clone(),
        )
        .unwrap();
        builder.enable_incremental();
        let stats = runtime.block_on(builder.build()).unwrap();
        stats
            .warning_details
            .iter()
            .map(|warning| warning.render())
            .collect()
    };

    let cold = build_once();
    assert_eq!(
        cold.len(),
        1,
        "one missing-document warning expected: {cold:?}"
    );
    assert!(
        cold[0].ends_with(
            "index.rst:4: WARNING: toctree contains reference to nonexisting \
             document 'gone' [toc.not_readable]"
        ),
        "{cold:?}"
    );
    // The dangling entry put `index` in `reread_always`, so the rebuild
    // reads it again however unchanged it is — and the parse records the
    // warning rides on have to come back the same.
    assert_eq!(build_once(), cold, "a warm rebuild must warn identically");
}

/// Sphinx logs the missing entry with `%r` — CPython `repr`, which escapes
/// every character `str.isprintable()` rejects, an NBSP included. The
/// expected line is byte-pinned to a sphinx 9.1.0 dummy build of this
/// project through `tools/gen_env_fixture.py`'s own `build_project`
/// (panel fix round F; `py_repr_str` had escaped only `< 0x20` and `0x7f`).
#[test]
fn a_toctree_warning_reprs_its_target_like_cpython() {
    let (_, warnings) = resolved_build(
        &[
            (
                "index",
                "Index\n=====\n\n.. toctree::\n\n   foo\u{a0}bar\n   real\n",
            ),
            ("real", "Real\n====\n\nbody\n"),
        ],
        &|_, _| {},
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:4: WARNING: toctree contains reference to nonexisting \
             document 'foo\\xa0bar' [toc.not_readable]"
        ]
    );
}

/// `TocTree.parse_content` reads `self.content` verbatim — no strip — so
/// an entry indented deeper than the block keeps its extra spaces and names
/// a document that does not exist. Byte-pinned to a sphinx 9.1.0 dummy build
/// of this project through `tools/gen_env_fixture.py`'s `build_project`
/// (panel fix round F; the crate had trimmed the entry and resolved `b`).
#[test]
fn a_deeper_indented_toctree_entry_keeps_its_spaces_like_sphinx() {
    let (_, warnings) = resolved_build(
        &[
            ("index", "Index\n=====\n\n.. toctree::\n\n   a\n     b\n"),
            ("a", "A\n=\n\nbody\n"),
            ("b", "B\n=\n\nbody\n"),
        ],
        &|_, _| {},
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:4: WARNING: toctree contains reference to nonexisting \
             document '  b' [toc.not_readable]",
            "<project>/b.rst: WARNING: document isn't included in any toctree \
             [toc.not_included]",
        ]
    );
}

// ---------------------------------------------------------------------------
// py cross-reference resolution: the [PY §3.3]/[PY §3.5] resolve probes as
// library-level build tests, each expectation byte-pinned to a sphinx 9.1.0
// dummy build of the same project (probe scripts, session of 2026-09-02).
// ---------------------------------------------------------------------------

/// Build one project (no incremental) with a caller-shaped [`BuildConfig`]
/// and return the resolved pformat per docname plus the rendered warnings,
/// both with the srcdir replaced by `<project>`.
fn resolved_build(
    files: &[(&str, &str)],
    configure: &dyn Fn(&Path, &mut BuildConfig),
) -> (BTreeMap<String, String>, Vec<String>) {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    for (docname, body) in files {
        write(&source_dir, docname, body);
    }
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();
    let mut config = BuildConfig::default();
    configure(&source_dir, &mut config);
    let mut builder =
        SphinxBuilder::new(config, source_dir.clone(), tmp.path().join("out")).unwrap();
    let stats = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(builder.build())
        .unwrap();
    let root = source_dir.to_string_lossy().into_owned();
    let warnings = normalize_warnings(&stats.warning_details, &root);
    let env = normalize_snapshot(&builder.snapshot_env(), &root);
    let resolved = env["resolved_pformat"]
        .as_object()
        .expect("resolved_pformat is a map")
        .iter()
        .map(|(docname, pformat)| (docname.clone(), pformat.as_str().unwrap().to_string()))
        .collect();
    (resolved, warnings)
}

fn py_build(files: &[(&str, &str)]) -> (BTreeMap<String, String>, Vec<String>) {
    resolved_build(files, &|_, _| {})
}

/// [PY §3.3] `resolve_basic`: all seven reference outcomes of the basic
/// project — bare/qualified/tilde targets, a module ref with the synopsis
/// reftitle, a class ref, an `:obj:` ref, and the silent dangling ref.
#[test]
fn py_refs_resolve_across_all_seven_basic_shapes() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "T\n=\n\n.. py:module:: mymod\n   :synopsis: My synopsis.\n   :platform: Unix\n\n\
         .. py:function:: f()\n\n.. py:class:: C\n\n   .. py:method:: meth()\n\n\
         Refs: :py:func:`f` and :py:func:`mymod.f` and :py:meth:`~mymod.C.meth`\n\
         and :py:mod:`mymod` and :py:class:`C` and :py:func:`nope_dangling`\n\
         and :py:obj:`f`.\n",
    )]);
    let index = &resolved["index"];

    // `f`, `mymod.f` and the `:obj:` ref all land on the same entry.
    assert_eq!(
        index
            .matches("<reference internal=\"1\" refid=\"mymod.f\" reftitle=\"mymod.f\">")
            .count(),
        3,
        "{index}"
    );
    // Their inner literals keep the role-shaped titles.
    for inner in [
        "<literal classes=\"xref py py-func\">\n                    f()",
        "<literal classes=\"xref py py-func\">\n                    mymod.f()",
        "<literal classes=\"xref py py-obj\">\n                    f",
    ] {
        assert!(index.contains(inner), "missing {inner:?} in {index}");
    }
    // `~mymod.C.meth`: tilde title, exact bare-name target.
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"mymod.C.meth\" reftitle=\"mymod.C.meth\">"
        ),
        "{index}"
    );
    // The module ref carries the `_make_module_refnode` reftitle.
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"module-mymod\" \
             reftitle=\"mymod: My synopsis. (Unix)\">"
        ),
        "{index}"
    );
    assert!(
        index.contains("<reference internal=\"1\" refid=\"mymod.C\" reftitle=\"mymod.C\">"),
        "{index}"
    );
    // The dangling ref keeps its literal with NO reference wrapper and — py
    // roles are not warn_dangling — no warning outside nitpicky mode.
    assert!(index.contains("nope_dangling()") && !index.contains("refid=\"nope_dangling\""));
    assert_eq!(warnings, Vec::<String>::new());
}

/// [PY §3.3] `resolve_class_context`: the `py:class` node attr enables the
/// `classname.name` exact lookup, and `.meth` finds the same target through
/// refspecific search.
#[test]
fn py_class_context_resolves_bare_and_dotted_method_refs() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "T\n=\n\n.. py:module:: m\n\n.. py:class:: C\n\n   .. py:method:: meth()\n\n   \
         In-class ref: :py:meth:`meth` and :py:meth:`.meth`.\n",
    )]);
    let index = &resolved["index"];
    assert_eq!(
        index
            .matches("<reference internal=\"1\" refid=\"m.C.meth\" reftitle=\"m.C.meth\">")
            .count(),
        2,
        "{index}"
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// [PY §3.3] `resolve_ambiguous_fuzzy`, order-distinguishing: zeta.same is
/// registered BEFORE alpha.same, so the fuzzy pass resolves zeta.same and
/// the warning lists the candidates in registration order — byte-pinned to
/// the sphinx 9.1.0 probe.
#[test]
fn py_fuzzy_ambiguity_takes_the_first_registration_and_warns_in_order() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "T\n=\n\n.. py:function:: zeta.same()\n\n.. py:function:: alpha.same()\n\n\
         Ref :py:func:`.same`.\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains("<reference internal=\"1\" refid=\"zeta.same\" reftitle=\"zeta.same\">"),
        "{index}"
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:8: WARNING: more than one target found for \
             cross-reference 'same': zeta.same, alpha.same [ref.python]"
        ]
    );
}

/// [PY §3.3] `resolve_parens_off`: `add_function_parentheses = False`
/// strips the implicit titles; both spellings still resolve (find_obj's
/// `()` strip is independent of the config).
#[test]
fn py_parens_off_strips_titles_and_still_resolves() {
    let (resolved, warnings) = resolved_build(
        &[(
            "index",
            "T\n=\n\n.. py:function:: f()\n\nRef :py:func:`f` and :py:func:`f()`.\n",
        )],
        &|_, config| config.add_function_parentheses = false,
    );
    let index = &resolved["index"];
    assert_eq!(
        index
            .matches("<reference internal=\"1\" refid=\"f\" reftitle=\"f\">")
            .count(),
        2,
        "{index}"
    );
    assert_eq!(
        index
            .matches("<literal classes=\"xref py py-func\">\n                    f\n")
            .count(),
        2,
        "both titles lose the parens (the index entry alone keeps `f()`): {index}"
    );
    assert!(
        index.contains("_toc_name=\"f\""),
        "the parens-off config reaches `_toc_name` too: {index}"
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// [PY §3.3] `resolve_type_alias_class_fallback`: a `:py:class:` ref falls
/// back to the data entry (type aliases documented as data).
#[test]
fn py_class_refs_fall_back_to_data_entries() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "T\n=\n\n.. py:data:: Alias\n\nRef :py:class:`Alias`.\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"Alias\" reftitle=\"Alias\">\n                \
             <literal classes=\"xref py py-class\">"
        ),
        "{index}"
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// [PY §3.5] `resolve_nitpicky`: the missing refs warn in the generic
/// non-std shape with `[ref.{typ}]`, while `builtin_resolver` silences
/// `int` — the pending_xref is replaced by its literal child with no
/// reference wrapper and no warning. Byte-pinned to the probe.
#[test]
fn py_nitpicky_warns_the_exact_bytes_and_builtins_stay_silent() {
    let (resolved, warnings) = resolved_build(
        &[(
            "index",
            "T\n=\n\nRef :py:func:`missing_fn` and :py:class:`int` and :py:class:`Missing`.\n",
        )],
        &|_, config| config.nitpicky = true,
    );
    let index = &resolved["index"];
    assert!(
        !index.contains("<reference"),
        "nothing resolves to a reference here: {index}"
    );
    assert!(index.contains("<literal classes=\"xref py py-class\">\n                int"));
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:4: WARNING: py:func reference target not found: \
             missing_fn [ref.func]",
            "<project>/index.rst:4: WARNING: py:class reference target not found: \
             Missing [ref.class]",
        ]
    );
}

/// [PY §3.5] `resolve_module_deprecated` + the both-flags probe: the module
/// reftitle appends `: synopsis`, ` (deprecated)`, ` (platform)` — in that
/// order, deprecated BEFORE platform (probe `module_both`).
#[test]
fn py_module_reftitles_append_deprecated_before_platform() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "T\n=\n\n.. py:module:: dep\n   :synopsis: Old stuff.\n   :deprecated:\n\n\
         .. py:module:: both\n   :synopsis: Some synopsis.\n   :platform: Unix, Windows\n\
         \x20  :deprecated:\n\nRef :py:mod:`dep` and :py:mod:`both`.\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"module-dep\" \
             reftitle=\"dep: Old stuff. (deprecated)\">"
        ),
        "{index}"
    );
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"module-both\" \
             reftitle=\"both: Some synopsis. (deprecated) (Unix, Windows)\">"
        ),
        "{index}"
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// `nitpick_ignore` matching for py refs, probe-verified: `('py:func',
/// target)` silences the warning; the bare `('func', target)` form does
/// NOT — sphinx tries the domainless form only for std types.
#[test]
fn py_nitpick_ignore_matches_domain_qualified_entries_only() {
    let source: &[(&str, &str)] = &[("index", "T\n=\n\nRef :py:func:`missing_fn`.\n")];
    let (_, silenced) = resolved_build(source, &|_, config| {
        config.nitpicky = true;
        config.nitpick_ignore = vec![("py:func".to_string(), "missing_fn".to_string())];
    });
    assert_eq!(silenced, Vec::<String>::new());

    let (_, bare) = resolved_build(source, &|_, config| {
        config.nitpicky = true;
        config.nitpick_ignore = vec![("func".to_string(), "missing_fn".to_string())];
    });
    assert_eq!(
        bare,
        vec![
            "<project>/index.rst:4: WARNING: py:func reference target not found: \
             missing_fn [ref.func]"
        ],
        "a bare-typ entry must not silence a py ref"
    );
}

/// `searchmode = 1 if node.hasattr('refspecific')`: annotation xrefs carry
/// `refspecific="0"` and STILL search refspecific — the bare `Cls`
/// annotation resolves `pkg.Cls` through the fuzzy pass (probe
/// `annotation_refspecific_zero`).
#[test]
fn py_annotation_xrefs_search_refspecific_by_attribute_presence() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "T\n=\n\n.. py:class:: pkg.Cls\n\n.. py:function:: f(x: Cls)\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"pkg.Cls\" reftitle=\"pkg.Cls\">\n                                Cls"
        ),
        "{index}"
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// The missing-reference event order, probe-pinned: intersphinx listens at
/// its default priority (500), `builtin_resolver` at 900 — so a builtin
/// name a loaded inventory carries resolves EXTERNALLY, one it doesn't
/// carry is silenced, and a plain miss still warns under nitpicky.
#[test]
fn py_builtins_in_a_loaded_inventory_resolve_externally_before_the_silencer() {
    let (resolved, warnings) = resolved_build(
        &[(
            "index",
            "T\n=\n\nRef :py:class:`int` and :py:class:`bool` and :py:class:`Missing`.\n",
        )],
        &|source_dir, config| {
            let header = b"# Sphinx inventory version 2\n\
                           # Project: other\n\
                           # Version: 1.0\n\
                           # The remainder of this file is compressed using zlib.\n";
            let payload = b"int py:class 1 library/functions.html#int -\n";
            let mut compressed = Vec::from(&header[..]);
            use std::io::Write as _;
            let mut encoder =
                flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::default());
            encoder.write_all(payload).unwrap();
            encoder.finish().unwrap();
            std::fs::write(source_dir.join("local.inv"), compressed).unwrap();
            config.nitpicky = true;
            config.intersphinx_mapping.insert(
                "other".to_string(),
                (
                    "https://other.example/".to_string(),
                    vec![Some("local.inv".to_string())],
                ),
            );
        },
    );
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"0\" reftitle=\"(in other v1.0)\" \
             refuri=\"https://other.example/library/functions.html#int\">"
        ),
        "`int` is in the inventory, so intersphinx (500) beats \
         builtin_resolver (900): {index}"
    );
    assert!(
        !index.contains("refid=\"bool\"") && !index.contains("#bool"),
        "`bool` is not in the inventory and is silenced by builtin_resolver: {index}"
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:4: WARNING: py:class reference target not found: \
             Missing [ref.class]"
        ]
    );
}

// ---------------------------------------------------------------------------
// `:any:` resolution: `ReferencesResolver._resolve_pending_any_xref`
// (`post_transforms/__init__.py:180-250`), each expectation byte-pinned to a
// sphinx 9.1.0 dummy build (probe_any.py, session of 2026-09-02). The
// COMMITTED oracle for the same shapes is the `py_any` project of
// tests/fixtures/env_differential.json (panel fix round B, [23]): its `a`
// document — a py-func hit bare and as `f()`, py-mod and py-data hits, a std
// label and a doc hit, each with the winner's extended literal classes, the
// std/py ambiguity with its ` or `-joined `[ref.any]` warning, and a dangling
// target warning without nitpicky — compares at full strength in
// `resolved_doctrees_match_oracle` and `warning_records_match_the_oracle`.
// The tests below stay as the fast, in-process feedback layer.
// ---------------------------------------------------------------------------

/// [PY §3.4] `resolve_any_role`: `f` → py-func with refid, `m` → py-mod —
/// the winner's inner literal classes are extended with
/// `[domain, role.replace(':', '-')]`, and find_obj's `()` strip makes
/// `:any:`f()`` resolve too.
#[test]
fn any_refs_resolve_py_targets_and_extend_the_literal_classes() {
    let (resolved, warnings) = py_build(&[(
        "index",
        ".. py:module:: m\n\n.. py:function:: f()\n\n\
         Ref: :any:`f` and :any:`m` and :any:`f()`.\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"m.f\" reftitle=\"m.f\">\n            \
             <literal classes=\"xref any py py-func\">\n                f\n"
        ),
        "{index}"
    );
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"module-m\" reftitle=\"m\">\n            \
             <literal classes=\"xref any py py-mod\">\n                m\n"
        ),
        "{index}"
    );
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"m.f\" reftitle=\"m.f\">\n            \
             <literal classes=\"xref any py py-func\">\n                f()\n"
        ),
        "{index}"
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// An explicit title rides through, and a miss warns `'any' reference
/// target not found` with `[ref.any]` — `:any:` is `warn_dangling=True`,
/// so this fires OUTSIDE nitpicky mode.
#[test]
fn any_misses_warn_outside_nitpicky_and_explicit_titles_survive() {
    let (resolved, warnings) = py_build(&[(
        "index",
        ".. py:function:: g()\n\nRef: :any:`click <g>` and :any:`missing_thing`.\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"g\" reftitle=\"g\">\n            \
             <literal classes=\"xref any py py-func\">\n                click\n"
        ),
        "{index}"
    );
    assert!(
        index.contains("<literal classes=\"xref any\">\n            missing_thing\n"),
        "the miss keeps the bare contnode: {index}"
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:3: WARNING: 'any' reference target not found: \
             missing_thing [ref.any]"
        ]
    );
}

/// A std-vs-py ambiguity: std wins (it resolves before the other domains),
/// the winner's fresh `std std-ref` inline is extended AGAIN (probe:
/// `classes="std std-ref std std-ref"`), and the warning joins the
/// candidates with ` or ` in resolution order — byte-pinned.
#[test]
fn any_ambiguity_prefers_std_and_warns_with_the_or_joined_candidates() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "Head\n====\n\n.. _same:\n\nSect\n----\n\n.. py:function:: same()\n\n\
         Ref: :any:`same`.\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"same\">\n                    \
             <inline classes=\"std std-ref std std-ref\">\n                        Sect\n"
        ),
        "{index}"
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:11: WARNING: more than one target found for 'any' \
             cross-reference 'same': could be :std:ref:`Sect` or :py:func:`same` [ref.any]"
        ]
    );
}

/// A lone std label hit through `:any:` — no warning, and the doubled
/// class extension again.
#[test]
fn any_resolves_a_std_label_silently() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "Head\n====\n\n.. _mylabel:\n\nSect\n----\n\nRef: :any:`mylabel`.\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"mylabel\">\n                    \
             <inline classes=\"std std-ref std std-ref\">\n                        Sect\n"
        ),
        "{index}"
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// `:doc:` resolution runs FIRST and its role string is the unprefixed
/// `doc`, so the winner's inline gains `doc doc` (probe:
/// `classes="doc doc doc"`) and the candidate list spells `:doc:`.
#[test]
fn any_tries_doc_first_and_the_doc_winner_triples_its_class() {
    let (resolved, warnings) = py_build(&[
        (
            "index",
            "Head\n====\n\n.. toctree::\n\n   other\n\nRef: :any:`other`.\n",
        ),
        (
            "other",
            "Other Title\n===========\n\n.. py:function:: other()\n",
        ),
    ]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refuri=\"\">\n                \
             <inline classes=\"doc doc doc\">\n                    Other Title\n"
        ),
        "{index}"
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:8: WARNING: more than one target found for 'any' \
             cross-reference 'other': could be :doc:`Other Title` or :py:func:`other` \
             [ref.any]"
        ]
    );
}

/// The std objects walk: envvar/option/confval hits keep the contnode
/// literal (extended `xref any std std-*`), while a glossary term is only
/// looked up LOWERCASED against its as-written objects key — so `Aterm`
/// dangles under both spellings (probe-pinned quirk).
#[test]
fn any_hits_std_objects_but_mixed_case_terms_dangle() {
    let (resolved, warnings) = py_build(&[(
        "index",
        "Head\n====\n\n.. envvar:: MYVAR\n\n.. program:: prog\n\n.. option:: --flag\n\n\
         .. glossary::\n\n   Aterm\n      Def.\n\n.. confval:: setting\n\n\
         Ref: :any:`MYVAR` and :any:`--flag` and :any:`Aterm` \
         and :any:`aterm` and :any:`setting`.\n",
    )]);
    let index = &resolved["index"];
    for hit in [
        "<reference internal=\"1\" refid=\"envvar-MYVAR\">\n                \
         <literal classes=\"xref any std std-envvar\">\n                    MYVAR\n",
        "<reference internal=\"1\" refid=\"cmdoption-prog-flag\">\n                \
         <literal classes=\"xref any std std-option\">\n                    --flag\n",
        "<reference internal=\"1\" refid=\"confval-setting\">\n                \
         <literal classes=\"xref any std std-confval\">\n                    setting\n",
    ] {
        assert!(index.contains(hit), "missing {hit:?} in {index}");
    }
    assert!(
        index.contains("<literal classes=\"xref any\">\n                Aterm\n"),
        "{index}"
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:17: WARNING: 'any' reference target not found: \
             Aterm [ref.any]",
            "<project>/index.rst:17: WARNING: 'any' reference target not found: \
             aterm [ref.any]",
        ]
    );
}

/// A py-only fuzzy ambiguity through `:any:`: candidates in REGISTRATION
/// order, first match wins — byte-pinned to the probe.
#[test]
fn any_py_fuzzy_ambiguity_lists_candidates_in_registration_order() {
    let (resolved, warnings) = py_build(&[(
        "index",
        ".. py:function:: zeta.same()\n   :no-index-entry:\n\n\
         .. py:function:: alpha.same()\n   :no-index-entry:\n\n\
         Ref: :any:`same`.\n",
    )]);
    let index = &resolved["index"];
    assert!(
        index.contains(
            "<reference internal=\"1\" refid=\"zeta.same\" reftitle=\"zeta.same\">\n            \
             <literal classes=\"xref any py py-func\">\n                same\n"
        ),
        "{index}"
    );
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:7: WARNING: more than one target found for 'any' \
             cross-reference 'same': could be :py:func:`zeta.same` or \
             :py:func:`alpha.same` [ref.any]"
        ]
    );
}

/// A module candidate renders its FULL `_make_module_refnode` reftitle in
/// the candidate list — `:py:mod:`syn: The syn module.`` (probe-pinned).
#[test]
fn any_ambiguity_candidates_render_module_reftitles_verbatim() {
    let (_, warnings) = py_build(&[(
        "index",
        "Head\n====\n\n.. _syn:\n\nSect\n----\n\n.. py:module:: syn\n   \
         :synopsis: The syn module.\n\nRef: :any:`syn`.\n",
    )]);
    assert_eq!(
        warnings,
        vec![
            "<project>/index.rst:12: WARNING: more than one target found for 'any' \
             cross-reference 'syn': could be :std:ref:`Sect` or \
             :py:mod:`syn: The syn module.` [ref.any]"
        ]
    );
}

// ---------------------------------------------------------------------------
// TOC object entries: `TocTreeCollector.build_toc`'s object branch
// (`collectors/toctree.py:125-160`) over py descriptions, byte-pinned to
// the [SIG §A.2] probe_toc.py outputs (sphinx 9.1.0).
// ---------------------------------------------------------------------------

/// Build one project and return the environment snapshot plus warnings,
/// both srcdir-normalized — the toc tests read `tocs_pformat`,
/// `toc_num_entries` and `toctree_includes` off it.
fn env_build(
    files: &[(&str, &str)],
    configure: &dyn Fn(&Path, &mut BuildConfig),
) -> (serde_json::Value, Vec<String>) {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    for (docname, body) in files {
        write(&source_dir, docname, body);
    }
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();
    let mut config = BuildConfig::default();
    configure(&source_dir, &mut config);
    let mut builder =
        SphinxBuilder::new(config, source_dir.clone(), tmp.path().join("out")).unwrap();
    let stats = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(builder.build())
        .unwrap();
    let root = source_dir.to_string_lossy().into_owned();
    let warnings = normalize_warnings(&stats.warning_details, &root);
    (normalize_snapshot(&builder.snapshot_env(), &root), warnings)
}

/// The [SIG §A.2] two-document probe project.
const TOC_PROBE_FILES: &[(&str, &str)] = &[
    ("index", "Index\n=====\n\n.. toctree::\n\n   mod\n"),
    (
        "mod",
        "Mod\n===\n\n.. py:module:: pkg.mymod\n\n.. py:class:: MyClass\n\n   Class body.\n\n   \
         .. py:method:: my_method(arg)\n\n      Method body.\n\n.. py:function:: my_func(x)\n\n   \
         Function body.\n",
    ),
];

/// `env.tocs['mod']` for the class/method/function texts a variant renders
/// — the nesting (method under class via `memo_parents`), the shared
/// anchorname counter and `skip_section_number` stamps are common to all.
fn toc_probe_expected(class: &str, method: &str, function: &str) -> String {
    format!(
        concat!(
            "<bullet_list>\n",
            "    <list_item>\n",
            "        <compact_paragraph>\n",
            "            <reference anchorname=\"\" internal=\"1\" refuri=\"mod\">\n",
            "                Mod\n",
            "        <bullet_list>\n",
            "            <list_item>\n",
            "                <compact_paragraph skip_section_number=\"1\">\n",
            "                    <reference anchorname=\"#pkg.mymod.MyClass\" internal=\"1\" \
             refuri=\"mod\">\n",
            "                        <literal>\n",
            "                            {class}\n",
            "                <bullet_list>\n",
            "                    <list_item>\n",
            "                        <compact_paragraph skip_section_number=\"1\">\n",
            "                            <reference anchorname=\"#pkg.mymod.MyClass.my_method\" \
             internal=\"1\" refuri=\"mod\">\n",
            "                                <literal>\n",
            "                                    {method}\n",
            "            <list_item>\n",
            "                <compact_paragraph skip_section_number=\"1\">\n",
            "                    <reference anchorname=\"#pkg.mymod.my_func\" internal=\"1\" \
             refuri=\"mod\">\n",
            "                        <literal>\n",
            "                            {function}\n",
        ),
        class = class,
        method = method,
        function = function,
    )
}

/// All five [SIG §A.2] config variants, `env.tocs['mod']` byte-verbatim,
/// plus the shared-counter `toc_num_entries` and the untouched
/// `toctree_includes`.
#[test]
fn toc_object_entries_match_the_probe_for_all_five_config_variants() {
    type Variant<'a> = (&'a str, &'a dyn Fn(&mut BuildConfig), String, u64);
    let variants: &[Variant<'_>] = &[
        (
            "defaults",
            &|_| {},
            toc_probe_expected("MyClass", "MyClass.my_method()", "my_func()"),
            4,
        ),
        (
            "show_parents='hide'",
            &|config| config.toc_object_entries_show_parents = "hide".to_string(),
            toc_probe_expected("MyClass", "my_method()", "my_func()"),
            4,
        ),
        (
            "show_parents='all'",
            &|config| config.toc_object_entries_show_parents = "all".to_string(),
            toc_probe_expected(
                "pkg.mymod.MyClass",
                "pkg.mymod.MyClass.my_method()",
                "pkg.mymod.my_func()",
            ),
            4,
        ),
        (
            "toc_object_entries=False",
            &|config| config.toc_object_entries = false,
            concat!(
                "<bullet_list>\n",
                "    <list_item>\n",
                "        <compact_paragraph>\n",
                "            <reference anchorname=\"\" internal=\"1\" refuri=\"mod\">\n",
                "                Mod\n",
            )
            .to_string(),
            1,
        ),
        (
            "add_function_parentheses=False",
            &|config| config.add_function_parentheses = false,
            toc_probe_expected("MyClass", "MyClass.my_method", "my_func"),
            4,
        ),
    ];
    for (label, configure, expected_toc, expected_num) in variants {
        let (env, warnings) = env_build(TOC_PROBE_FILES, &|_, config| configure(config));
        assert_eq!(
            env["tocs_pformat"]["mod"].as_str().unwrap(),
            expected_toc,
            "{label}"
        );
        assert_eq!(
            env["toc_num_entries"]["mod"].as_u64().unwrap(),
            *expected_num,
            "{label}"
        );
        assert_eq!(
            env["toc_num_entries"]["index"].as_u64().unwrap(),
            1,
            "{label}"
        );
        assert_eq!(
            env["toctree_includes"],
            serde_json::json!({ "index": ["mod"] }),
            "{label}: toctree_includes is unaffected in every mode"
        );
        assert_eq!(warnings, Vec::<String>::new(), "{label}");
    }
}

/// A document whose FIRST collected entry is a top-level desc: sections
/// and objects share the `numentries` counter, so the object gets
/// `anchorname=""` and the later section starts at `#` — byte-pinned to
/// the probe (toc_first_entry_desc, 2026-09-02).
#[test]
fn a_leading_object_entry_takes_the_empty_anchorname() {
    let (env, warnings) = env_build(
        &[
            ("index", "Head\n====\n\n.. toctree::\n\n   mod\n"),
            (
                "mod",
                ".. py:function:: lead()\n\nBody.\n\nLater\n=====\n\n.. py:function:: trail()\n",
            ),
        ],
        &|_, _| {},
    );
    assert_eq!(
        env["tocs_pformat"]["mod"].as_str().unwrap(),
        concat!(
            "<bullet_list>\n",
            "    <list_item>\n",
            "        <compact_paragraph skip_section_number=\"1\">\n",
            "            <reference anchorname=\"\" internal=\"1\" refuri=\"mod\">\n",
            "                <literal>\n",
            "                    lead()\n",
            "    <list_item>\n",
            "        <compact_paragraph>\n",
            "            <reference anchorname=\"#later\" internal=\"1\" refuri=\"mod\">\n",
            "                Later\n",
            "        <bullet_list>\n",
            "            <list_item>\n",
            "                <compact_paragraph skip_section_number=\"1\">\n",
            "                    <reference anchorname=\"#trail\" internal=\"1\" refuri=\"mod\">\n",
            "                        <literal>\n",
            "                            trail()\n",
        )
    );
    assert_eq!(env["toc_num_entries"]["mod"].as_u64().unwrap(), 3);
    assert_eq!(env["toc_num_entries"]["index"].as_u64().unwrap(), 1);
    assert_eq!(warnings, Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// py-modindex: `PythonModuleIndex.generate` (`__init__.py:620-717`) through
// the whole build, pinned to the probe_modindex.py two_module dump
// (sphinx 9.1.0, 2026-09-02).
// ---------------------------------------------------------------------------

/// A submodule promotes its parent to a group head (subtype 1), carries
/// the module options into the entry fields, and two modules with one
/// top-level do not collapse (2 − 1 = 1 < 1 is false).
#[test]
fn py_modindex_snapshot_matches_the_two_module_probe() {
    let (env, warnings) = env_build(
        &[(
            "index",
            "Head\n====\n\n.. py:module:: pkg\n   :synopsis: Top package.\n\n\
             .. py:module:: pkg.sub\n   :platform: Unix\n",
        )],
        &|_, _| {},
    );
    assert_eq!(
        env["py_modindex"],
        serde_json::json!({
            "collapse": false,
            "groups": [{
                "letter": "p",
                "entries": [
                    {
                        "name": "pkg",
                        "subtype": 1,
                        "docname": "index",
                        "anchor": "module-pkg",
                        "extra": "",
                        "qualifier": "",
                        "descr": "Top package.",
                    },
                    {
                        "name": "pkg.sub",
                        "subtype": 2,
                        "docname": "index",
                        "anchor": "module-pkg.sub",
                        "extra": "Unix",
                        "qualifier": "",
                        "descr": "",
                    },
                ],
            }],
        })
    );
    assert_eq!(warnings, Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// Warning provenance for content that lives in an included file, and for
// the xrefs the py directives synthesize (panel fix round B, cluster P).
// ---------------------------------------------------------------------------

/// A build over files written at their RELATIVE PATHS (so `.inc` members
/// that are not documents can sit beside the `.rst` sources), returning
/// the rendered warnings with the srcdir prefix collapsed the way the
/// oracle spells them (`canon_scope8`): `index.rst:4: WARNING: ...`.
fn tree_warnings(files: &[(&str, &str)], configure: &dyn Fn(&mut BuildConfig)) -> Vec<String> {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    for (relative, body) in files {
        let path = source_dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();
    let mut config = BuildConfig::default();
    configure(&mut config);
    let mut builder =
        SphinxBuilder::new(config, source_dir.clone(), tmp.path().join("out")).unwrap();
    let stats = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(builder.build())
        .unwrap();
    let root = source_dir.to_string_lossy().into_owned();
    normalize_warnings(&stats.warning_details, &root)
        .iter()
        .map(|warning| canon_scope8(warning))
        .collect()
}

fn nitpicky(config: &mut BuildConfig) {
    config.nitpicky = true;
}

/// Sphinx locates a resolution warning with `location=node`, which goes
/// through docutils' `get_source_line`: the node's own `(source, line)`
/// when it has one. A signature annotation's `pending_xref` is built by
/// the directive and inherits the `desc_signature`'s stamp, so the warning
/// names the signature's OWN file and line — the included file's for a
/// signature written inside an `.. include::`. Bytes probed on the pinned
/// toolchain (scratchpad/B/probe_loc2.py, `annotations`).
#[test]
fn annotation_xref_warnings_locate_at_the_signature_in_its_own_file() {
    let warnings = tree_warnings(
        &[
            (
                "index.rst",
                "Top\n===\n\n.. include:: inc.inc\n\n\
                 .. py:function:: g(y: alsomissing) -> retmissing\n\n\
                 .. py:data:: d\n   :type: typemissing\n",
            ),
            ("inc.inc", ".. py:function:: f(x: missingtype)\n"),
        ],
        &nitpicky,
    );
    assert_eq!(
        warnings,
        vec![
            "inc.inc:1: WARNING: py:class reference target not found: missingtype [ref.class]",
            "index.rst:6: WARNING: py:class reference target not found: alsomissing [ref.class]",
            "index.rst:6: WARNING: py:class reference target not found: retmissing [ref.class]",
            "index.rst:8: WARNING: py:class reference target not found: typemissing [ref.class]",
        ]
    );
}

/// The doc-field xrefs (`:param T x:`, `:rtype:`) are synthesized by
/// `DocFieldTransformer` into nodes that carry NO provenance in docutils —
/// and neither do the `desc`/`desc_content` above them — so
/// `get_source_line` walks up to the enclosing section (its underline
/// line), the `.. note::` directive line, or the list item, whichever is
/// nearest; through an include it lands in the INCLUDER's section. An
/// inline role written in a field body keeps its own line. Bytes probed on
/// the pinned toolchain (scratchpad/B/probe_loc2.py, `docfields`).
#[test]
fn doc_field_xref_warnings_locate_at_the_nearest_stamped_ancestor() {
    let warnings = tree_warnings(
        &[
            (
                "index.rst",
                "Top\n===\n\n.. include:: inc.inc\n\nSec2\n----\n\n\
                 .. py:function:: g(y)\n\n\
                 \x20  :param nosuch_doc y: d\n\
                 \x20  :param k: see :py:class:`nosuch_inline`\n\
                 \x20  :rtype: nosuch_rt\n\n\
                 .. note::\n\n\
                 \x20  .. py:function:: h(z)\n\n\
                 \x20     :param nosuch_note z: q\n\n\
                 - item\n\n\
                 \x20 .. py:class:: C\n\n\
                 \x20    .. py:method:: m(w)\n\n\
                 \x20       :param nosuch_nested w: q\n",
            ),
            (
                "inc.inc",
                ".. py:function:: h0(z)\n\n   :param nosuch_inc z: q\n",
            ),
        ],
        &nitpicky,
    );
    assert_eq!(
        warnings,
        vec![
            "index.rst:2: WARNING: py:class reference target not found: nosuch_inc [ref.class]",
            "index.rst:7: WARNING: py:class reference target not found: nosuch_doc [ref.class]",
            "index.rst:12: WARNING: py:class reference target not found: nosuch_inline [ref.class]",
            "index.rst:7: WARNING: py:class reference target not found: nosuch_rt [ref.class]",
            "index.rst:15: WARNING: py:class reference target not found: nosuch_note [ref.class]",
            "index.rst:21: WARNING: py:class reference target not found: nosuch_nested [ref.class]",
        ]
    );
}

/// With no stamped ancestor at all (a description directly under the
/// document, whose `document` node has no `source`/`line` in docutils'
/// sense), `get_node_location` returns None and sphinx prints the warning
/// with NO location prefix. Probed (`nosection`).
#[test]
fn a_doc_field_xref_with_no_stamped_ancestor_warns_without_a_location() {
    let warnings = tree_warnings(
        &[(
            "index.rst",
            ".. py:function:: g(y)\n\n   :param nosuch_doc y: d\n",
        )],
        &nitpicky,
    );
    assert_eq!(
        warnings,
        vec!["WARNING: py:class reference target not found: nosuch_doc [ref.class]"]
    );
}

/// The `[ref.python]` ambiguity warning is not nitpick-gated, so a default
/// build exercises both locations too: a `.Dup` annotation (refspecific)
/// at the signature line, a `:param Dup y:` field at the section
/// underline. Probed (`ambig`).
#[test]
fn ambiguity_warnings_follow_the_same_locations_in_a_default_build() {
    let warnings = tree_warnings(
        &[(
            "index.rst",
            "Top\n===\n\n.. py:module:: a\n\n.. py:class:: Dup\n\n.. py:module:: b\n\n\
             .. py:class:: Dup\n\n.. py:currentmodule:: None\n\nSec\n---\n\n\
             .. py:function:: f(x: .Dup)\n\n   :param Dup y: thing\n",
        )],
        &|_| {},
    );
    assert_eq!(
        warnings,
        vec![
            "index.rst:17: WARNING: more than one target found for cross-reference 'Dup': \
             a.Dup, b.Dup [ref.python]",
            "index.rst:15: WARNING: more than one target found for cross-reference 'Dup': \
             a.Dup, b.Dup [ref.python]",
        ]
    );
}

/// `TocTree.parse_content` logs with `location=toctree` — the node, whose
/// `source` is the included file's. Probed (`tocinc`).
#[test]
fn toctree_warnings_inside_an_included_file_name_that_file() {
    let warnings = tree_warnings(
        &[
            ("index.rst", "Index\n=====\n\n.. include:: toc_part.inc\n"),
            (
                "toc_part.inc",
                "Shared fragment.\n\n.. toctree::\n\n   nosuchdoc\n",
            ),
        ],
        &|_| {},
    );
    assert_eq!(
        warnings,
        vec![
            "toc_part.inc:3: WARNING: toctree contains reference to nonexisting document \
             'nosuchdoc' [toc.not_readable]"
        ]
    );
}

/// The section-numbering warning is logged at the `toctree` node too
/// (`location=toctreenode`, `collectors/toctree.py`). The includer sorts
/// AFTER `index` here so that `index`'s numbered toctree claims `b` first
/// and the spliced toctree inside `z` is the one that finds it already
/// assigned — sphinx iterates `env.numbered_toctrees` in set order, so the
/// message SET is not oracle-stable, but the location of this warning is
/// fixed by the node's provenance.
#[test]
fn numbering_warnings_inside_an_included_file_name_that_file() {
    let warnings = tree_warnings(
        &[
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n   :numbered:\n\n   z\n   b\n",
            ),
            ("z.rst", "Z\n=\n\n.. include:: part.inc\n"),
            ("b.rst", "B\n=\n\nbody\n"),
            ("part.inc", "Frag.\n\n.. toctree::\n   :numbered:\n\n   b\n"),
        ],
        &|_| {},
    );
    let nested = "b is already assigned section numbers (nested numbered toctree?) [toc.secnum]";
    assert_eq!(
        warnings,
        vec![
            format!("index.rst:4: WARNING: {nested}"),
            format!("part.inc:3: WARNING: {nested}"),
        ]
    );
}

/// D1 (wave 5, sub-project 1): markup docutils itself diagnoses earns no
/// validator finding, in an included file as much as in the document. This
/// test used to pin the validator's `Invalid alignment: bogus` naming
/// `frag.inc:3` rather than the includer's `index.rst:3` (a blank line
/// there). That check is gone: docutils' own `Error in "image" directive:
/// invalid option value` for the same `:align:` is the one message, and the
/// file and line it carries for a directive inside an included file are
/// pinned with the printed diagnostics this sub-project adds.
#[test]
fn a_docutils_diagnosed_directive_inside_an_included_file_earns_no_validator_warning() {
    let warnings = tree_warnings(
        &[
            (
                "index.rst",
                "Title\n=====\n\nParagraph one.\n\nParagraph two.\n\n.. include:: frag.inc\n",
            ),
            (
                "frag.inc",
                "Fragment.\n\n.. image:: x.png\n   :align: bogus\n\n.. note::\n",
            ),
        ],
        &|_| {},
    );
    for validator_text in ["Invalid alignment", "Note directive requires content"] {
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains(validator_text)),
            "{validator_text:?} is a validator echo of a docutils error: {warnings:#?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Validation must not fabricate (panel fix round B, cluster V)
// ---------------------------------------------------------------------------

/// A document sphinx-build 9.1.0 builds with NO warnings (probed:
/// scratchpad/B/pv): a standard include, a `.py` shown with `:literal:`, a
/// literalinclude with a negative `:tab-width:`/`:lineno-start:` (typed
/// plain `int` in sphinx), two empty code-blocks, and a toctree with the
/// documented `:maxdepth: -1`. The validators used to invent five
/// warnings for it — "Unusual file extension", "must be a positive
/// integer" (twice), "has no content", "maxdepth must be a positive
/// integer" — every one a `-W` failure on a clean project.
#[test]
fn a_clean_sphinx_project_earns_no_validation_warnings() {
    let warnings = tree_warnings(
        &[
            (
                "index.rst",
                "Title\n=====\n\n.. include:: <isonum.txt>\n\n\
                 .. include:: snippet.py\n   :literal:\n\n\
                 .. literalinclude:: snippet.py\n   :tab-width: -1\n   :lineno-start: -3\n\n\
                 .. code-block::\n\n.. code-block:: python\n\n\
                 .. toctree::\n   :maxdepth: -1\n\n   other\n",
            ),
            ("other.rst", "Other\n=====\n\nbody\n"),
            ("snippet.py", "def f():\n\treturn 1\n"),
        ],
        &|_| {},
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// D1 (wave 5, sub-project 1): every check the M1 validators made that has
/// no Sphinx counterpart fired on markup `sphinx-build` accepts. This project
/// holds one instance of each -- a local `.ico` image, a remote one without
/// an extension, an empty toctree, an empty and a `\left\{` math directive,
/// `:doc:` targets that are real docnames ending in `.rst`/`.md`, a `.whl`,
/// an extension-less and a URL download, braces that `\{` makes "unbalanced",
/// a lowercase abbreviation, a shell pipeline, path characters, `<>` as text
/// in every non-cross-reference role, and the `&` access key -- and
/// sphinx 9.1.0 / docutils 0.22.4 builds it with no warning at all (probed:
/// scratchpad probe-task5, `accepted_all_text_bodies`). The validators used
/// to add 26.
#[test]
fn markup_sphinx_accepts_earns_no_validation_warnings() {
    let warnings = tree_warnings(
        &[
            (
                "index.rst",
                "Title\n=====\n\n\
                 .. image:: favicon.ico\n\n\
                 .. image:: https://example.com/badge/logo\n\n\
                 .. figure:: https://example.com/badge/logo\n\n   A caption.\n\n\
                 .. toctree::\n\n\
                 .. math::\n\n\
                 .. math::\n\n   \\left\\{ x \\right.\n\n\
                 :doc:`notes.rst` and :doc:`notes.md`\n\n\
                 :download:`pkg <pkg.whl>`, :download:`license <LICENSE>` and \
                 :download:`site <https://example.com/file.pdf>`\n\n\
                 :math:`\\left\\{ x \\right.` and :math:`a <>`\n\n\
                 :abbr:`rst (reStructuredText)` and :abbr:`x <>`\n\n\
                 :command:`ls | grep foo` and :command:`x <>`\n\n\
                 :file:`C:\\Windows`, :file:`a?b*c` and :file:`x <>`\n\n\
                 :kbd:`<>` and :kbd:`x <>`\n\n\
                 :menuselection:`<>` and :menuselection:`x <>`\n\n\
                 :guilabel:`&File`, :guilabel:`a && b` and :guilabel:`<>`\n",
            ),
            ("favicon.ico", "icon\n"),
            ("pkg.whl", "wheel\n"),
            ("LICENSE", "MIT\n"),
            ("notes.rst.rst", ":orphan:\n\nNotes\n=====\n"),
            ("notes.md.rst", ":orphan:\n\nNotes\n=====\n"),
        ],
        &|_| {},
    );
    assert_eq!(warnings, Vec::<String>::new());
}

/// D1, the cross-reference half: a `:doc:`/`:ref:` whose target does not
/// resolve is reported by Sphinx itself (`warn_dangling`, no `-n` needed),
/// and the build already prints those lines byte-for-byte -- the
/// validators' "should not include file extension" and "requires a ...
/// target" findings were the same problems said a second time, in words
/// Sphinx does not use. Probed: scratchpad probe-task5,
/// `doc_ref_unresolvable`.
#[test]
fn unresolvable_doc_and_ref_targets_earn_only_sphinxs_own_warnings() {
    let warnings = tree_warnings(
        &[
            (
                "index.rst",
                "Title\n=====\n\n:doc:`other.rst`\n\n:doc:`Title <>`\n\n:ref:`Title <>`\n",
            ),
            ("other.rst", ":orphan:\n\nOther\n=====\n"),
        ],
        &|_| {},
    );
    assert_eq!(
        warnings,
        vec![
            "index.rst:4: WARNING: unknown document: 'other.rst' [ref.doc]",
            "index.rst:6: WARNING: unknown document: '' [ref.doc]",
            "index.rst:8: WARNING: undefined label: '' [ref.ref]",
        ]
    );
}

// ---------------------------------------------------------------------------
// `source_encoding` reaches the read phase (panel fix round B, [19])
// ---------------------------------------------------------------------------

/// The configured `source_encoding` is what the build's parser decodes
/// include and literalinclude targets with (probed: sphinx 9.1.0 with
/// `source_encoding = 'latin-1'` renders `café here` for both directives
/// over `caf\xe9 here\n`, plus its deprecation warning).
#[test]
fn source_encoding_is_threaded_from_the_build_configuration_into_the_parser() {
    let tmp = tempfile::TempDir::new().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("index.rst"),
        "Top\n===\n\n.. include:: inc.inc\n\n.. literalinclude:: inc.inc\n",
    )
    .unwrap();
    std::fs::write(source_dir.join("inc.inc"), b"caf\xe9 here\n").unwrap();
    let source_dir = sphinx_ultra::utils::canonicalize_simplified(&source_dir).unwrap();
    let config = BuildConfig {
        source_encoding: "latin-1".to_string(),
        ..Default::default()
    };
    let mut builder =
        SphinxBuilder::new(config, source_dir.clone(), tmp.path().join("out")).unwrap();
    let stats = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(builder.build())
        .unwrap();
    let env = builder.snapshot_env();
    let index = env["resolved_pformat"]["index"].as_str().unwrap();
    assert_eq!(
        index.matches("caf\u{e9} here").count(),
        2,
        "both directives decode through source_encoding: {index}"
    );
    assert!(
        stats.warning_details.is_empty(),
        "{:?}",
        stats.warning_details
    );
}
