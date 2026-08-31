//! Differential test: our environment layer vs the ENVIRONMENT ORACLE --
//! `BuildEnvironment` state (toctree graph, relations, section and figure
//! numbering, std-domain registries, index entries, genindex, and fully
//! cross-reference-resolved doctrees) a real `sphinx-build` 9.1.0 read +
//! resolve phase produces for the committed multi-document project corpus.
//!
//! Regenerate the fixture (manual, never in CI):
//!     uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
//!         python tools/gen_env_fixture.py
//!
//! Each `expect` key group has exactly one test. A group whose library
//! surface doesn't exist yet stays `#[ignore]`d with the wave-4 task that
//! will build it; un-ignoring one is the signal that its keys are now
//! covered. **Every** `expect` key is now live (tasks 5-10):
//! `tocs_pformat`, `toc_num_entries`, `toctree_includes`,
//! `files_to_rebuild`, `relations`, `toc_secnumbers`, `toc_fignumbers`,
//! `std`, `index_entries`, `genindex`, `resolved_pformat` and `warnings` —
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

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

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
    pub resolved_pformat: BTreeMap<String, String>,
    pub warnings: Vec<String>,
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
    // Warning locations come from walking the source tree, which resolves
    // symlinks (`/var/...` -> `/private/var/...` on macOS): normalize the
    // root the same way so the `<project>` substitution below lands.
    let source_dir = std::fs::canonicalize(&source_dir).unwrap();

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
    let warnings = stats
        .warning_details
        .iter()
        .map(|warning| warning.render().replace(&root, "<project>"))
        .collect();

    // Doctrees carry their absolute source path on the `document` node, and
    // `dependencies` holds absolute paths: the oracle records both with the
    // srcdir replaced by `<project>` (see tools/gen_env_fixture.py), so the
    // whole snapshot goes through the same substitution.
    let env = builder
        .snapshot_env()
        .to_string()
        .replace(&root, "<project>");
    let env = serde_json::from_str(&env).expect("the snapshot survives path normalization");

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
/// A `-D` value is a single scalar, so a list-valued (or nested, or null)
/// conf entry has no faithful spelling here: stringifying it would hand
/// `apply_override` something like `["a","b"]`, which its `Value::Array`
/// arm cheerfully splits on commas into `["[\"a\"", "\"b\"]"]` and stores
/// — a silently *differently* configured build compared against the
/// oracle. Rejected outright instead, naming the key, so a future fixture
/// project carrying such a value fails here rather than diverging quietly.
fn scalar_override(project: &str, key: &str, value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => value.to_string(),
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
];

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

/// Fixture `conf` keys that provably steer no behavior this crate has
/// implemented, which is what makes it sound to leave them off the
/// [`config_of`] override pass.
///
/// `smartquotes`: no smart-quote transform exists. A new fixture project
/// introducing another such key must either have it added here (with the
/// same kind of justification) or be expressible as a `-D` override.
const KNOWN_INERT_CONF: &[&str] = &["smartquotes"];

fn report(divergences: &[String], keys: &str) {
    assert!(
        divergences.is_empty(),
        "{} divergence(s) vs the sphinx 9.1.0 environment oracle ({keys}):\n\n{}",
        divergences.len(),
        divergences.join("\n\n")
    );
}

#[test]
fn fixture_loads_and_meets_floor() {
    let fixture = load_fixture();
    assert_eq!(fixture.sphinx_version, "9.1.0");
    assert_eq!(fixture.docutils_version, "0.22.4");
    assert!(
        fixture.projects.len() >= 12,
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

/// Every diagnostic the build emits, byte-identical to `sphinx-build`'s —
/// message text, `file:line` location, and the `[type.subtype]` suffix
/// `show_warning_types` appends.
///
/// Live for the projects whose oracle warnings are toctree diagnostics;
/// the rest are pinned in [`KNOWN_WARNING_GAPS`] to the task that will
/// produce them.
#[test]
fn warnings_match_oracle() {
    let fixture = load_fixture();
    let mut divergences = Vec::new();
    let mut visited_gaps: Vec<&str> = Vec::new();

    for project in &fixture.projects {
        let actual = &built_of(project).warnings;
        let matches = *actual == project.expect.warnings;

        match known_warning_gap(&project.name) {
            Some(why) => {
                visited_gaps.push(project.name.as_str());
                assert!(
                    !matches,
                    "[{}] listed in KNOWN_WARNING_GAPS ({why}) but the warnings \
                     now match the oracle — delete the exemption",
                    project.name
                );
            }
            None if !matches => divergences.push(format!(
                "[{}] warnings\n  expected: {:#?}\n  actual:   {actual:#?}",
                project.name, project.expect.warnings
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

    report(&divergences, "warnings");
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
            let expected = expected.replace(TRANSLATION_PROGRESS_ATTR, "");
            let actual = &resolved[docname];
            let matches = *actual == expected;

            match known_resolved_gap(&project.name, docname) {
                Some(why) => {
                    visited_gaps.push((project.name.as_str(), docname.as_str()));
                    assert!(
                        !matches,
                        "[{}] {docname}: listed in KNOWN_RESOLVED_GAPS ({why}) but \
                         the resolved doctree now matches the oracle — delete the \
                         exemption",
                        project.name
                    );
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

    report(&divergences, "resolved_pformat");
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

/// Label collection and reference resolution both read parse output that a
/// warm cache hit does not recompute — the id/name registry and the source
/// text (for warning line numbers) ride the cached `Document`, the doctree
/// comes off disk. A rebuild that loses either reports different warnings,
/// or none.
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
    let source_dir = std::fs::canonicalize(&source_dir).unwrap();

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
            stats
                .warning_details
                .iter()
                .map(|warning| warning.render().replace(&root, "<project>"))
                .collect(),
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
            // Sphinx purges a document's domain data immediately before
            // re-reading *that* document (`Builder._read_serial`:
            // `env.clear_doc(docname)` then `env.read_doc(docname)`), so
            // while `a` is being re-read, `dup` is still the *previous*
            // build's entry — owned by `b`. Reading `b` then finds the entry
            // `a` just wrote. Sphinx reports the same pair whenever both
            // documents are re-read against a warm environment; what keeps
            // it out of a real rebuild is that an unchanged document is not
            // re-read at all, which is the outdated computation this crate
            // has yet to port.
            "<project>/a.rst:7: WARNING: duplicate label dup, other instance in <project>/b.rst"
                .to_string(),
            "<project>/b.rst:7: WARNING: duplicate label dup, other instance in <project>/a.rst"
                .to_string(),
            "<project>/a.rst:9: WARNING: unknown document: 'nope' [ref.doc]".to_string(),
            "<project>/b.rst:9: WARNING: term not in glossary: 'nothing' [ref.term]".to_string(),
        ],
        "the resolution warnings must survive a warm rebuild unchanged: they \
         are recomputed from the cached document's registry, source text and \
         persisted doctree"
    );
}

/// Toctree diagnostics are produced during the parse, which a warm cache
/// hit skips — so they have to ride the cached parse records, not be
/// recomputed. A rebuild that reports fewer warnings than a cold build is
/// the failure this guards.
/// `Cmdoption.handle_signature`'s malformed-option diagnostic goes to the
/// logger, not the tree, so it has to ride the parse records to reach the
/// build's warning list — and survive a cache hit that skipped the parse.
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
    let source_dir = std::fs::canonicalize(&source_dir).unwrap();

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
            stats
                .warning_details
                .iter()
                .map(|warning| warning.render().replace(&root, "<project>"))
                .collect(),
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
    assert_eq!(
        warm, expected,
        "a cache hit skips the parse, so the diagnostic has to come back off \
         the cached document's records"
    );
    assert_eq!(warm_env["std"], cold_env["std"]);
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
    let source_dir = std::fs::canonicalize(&source_dir).unwrap();

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
            stats
                .warning_details
                .iter()
                .map(|warning| warning.render().replace(&root, "<project>"))
                .collect(),
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
    assert_eq!(
        warm, expected_warnings,
        "a cache hit skips the parse, so the diagnostic has to come back off \
         the cached doctree's entries"
    );
    assert_eq!(warm_env["index_entries"], expected_entries);
    assert_eq!(warm_env["genindex"], expected_genindex);
}

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
    assert_eq!(build_once(), cold, "a warm rebuild must warn identically");
}
