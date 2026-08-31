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
//! covered. Live so far (tasks 5-6): `tocs_pformat`, `toc_num_entries`,
//! `toctree_includes`, `files_to_rebuild`, `relations`, and `warnings` for
//! every project whose expected diagnostics are toctree diagnostics (see
//! [`KNOWN_WARNING_GAPS`]).
//!
//! Each live test materializes every fixture project into a tempdir, runs
//! the real library build (read + merge + resolve), and diffs
//! `SphinxBuilder::snapshot_env()` (plus the build's warnings) against the
//! oracle. The fixture's `conf` overrides are not applied: the only ones
//! present are inert for every live key ([`KNOWN_INERT_CONF`] pins that
//! claim, so a future fixture project carrying a behavior-relevant key
//! fails here instead of silently comparing against a differently
//! configured oracle).

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

#[derive(serde::Deserialize)]
pub struct StdObjectEntry {
    pub objtype: String,
    pub name: String,
    pub docname: String,
    pub labelid: String,
}

#[derive(serde::Deserialize)]
pub struct StdProgOptionEntry {
    pub program: Option<String>,
    pub name: String,
    pub docname: String,
    pub labelid: String,
}

#[derive(serde::Deserialize)]
pub struct GenIndexGroup {
    pub group: String,
    pub entries: Vec<GenIndexEntry>,
}

#[derive(serde::Deserialize)]
pub struct GenIndexEntry {
    pub name: String,
    /// (main_flag, uri) pairs; main_flag is the literal sphinx string
    /// `"main"` or `""`, not a bool.
    pub targets: Vec<(String, String)>,
    pub subitems: Vec<GenIndexSubItem>,
    pub category_key: Option<String>,
}

#[derive(serde::Deserialize)]
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

    let mut builder = SphinxBuilder::new(BuildConfig::default(), source_dir.clone(), output_dir)
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

    Built {
        env: builder.snapshot_env(),
        warnings,
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
const KNOWN_TOC_GAPS: &[(&str, &str, &str)] = &[(
    "std_objects",
    "a",
    "the `.. confval::` object signature contributes an `addnodes.desc` toc \
     entry; nothing in this crate produces desc nodes yet",
)];

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
        "`image file not readable` plus `numfig is disabled. :numref: is \
         ignored.` — both land with numbering/std-domain resolution",
    ),
    (
        "labels_dups",
        "`duplicate label` is a std-domain diagnostic (std domain task)",
    ),
    (
        "glossary_terms",
        "`term not in glossary` is a std-domain resolution diagnostic",
    ),
    (
        "std_objects",
        "`unknown option` is a std-domain resolution diagnostic",
    ),
    (
        "doc_refs",
        "`unknown document` is a std-domain cross-reference diagnostic",
    ),
];

fn known_warning_gap(project: &str) -> Option<&'static str> {
    KNOWN_WARNING_GAPS
        .iter()
        .find(|(p, _)| *p == project)
        .map(|(_, why)| *why)
}

/// Fixture `conf` keys that provably steer no behavior this crate has
/// implemented, which is what makes it sound to build every project with a
/// default [`BuildConfig`] instead of applying the overrides.
///
/// `smartquotes`: no smart-quote transform exists. `numfig`,
/// `numfig_secnum_depth`, `numfig_format`: they only steer
/// `toc_secnumbers`/`toc_fignumbers` and the `numref` role, none of which
/// is live. A new fixture project introducing any other key must either
/// have its key added here (with the same kind of justification) or make
/// the harness apply the overrides.
const KNOWN_INERT_CONF: &[&str] = &[
    "smartquotes",
    "numfig",
    "numfig_secnum_depth",
    "numfig_format",
];

/// Drop `secnumber="..."` attributes from a toc pformat.
///
/// `assign_section_numbers` stamps those onto the very `reference` nodes
/// `build_toc` creates, from the same walk that fills `toc_secnumbers` —
/// which is a later task's `expect` key. Comparing the toc *structure*
/// therefore means comparing it without that one attribute; the guard in
/// [`local_tocs_match_oracle`] keeps the normalization from silently
/// widening beyond the documents `toc_secnumbers` actually covers.
fn strip_secnumbers(pformat: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r#" secnumber="[^"]*""#).unwrap())
        .replace_all(pformat, "")
        .into_owned()
}

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

    // The harness builds with a default `BuildConfig` rather than the
    // project's confoverrides; that is only sound while every override is
    // inert for the live keys.
    for project in &fixture.projects {
        let conf = project
            .conf
            .as_object()
            .unwrap_or_else(|| panic!("project {:?}: conf is not an object", project.name));
        for key in conf.keys() {
            assert!(
                KNOWN_INERT_CONF.contains(&key.as_str()),
                "project {:?} sets conf key {key:?}, which is not in \
                 KNOWN_INERT_CONF — either prove it inert and list it, or \
                 teach build_project() to apply the overrides",
                project.name
            );
        }
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
            // Guard the secnumber normalization: it may only apply to
            // documents the oracle actually assigned section numbers to.
            if expected.contains("secnumber=") {
                assert!(
                    project.expect.toc_secnumbers.contains_key(docname),
                    "[{}] {docname}: oracle toc carries secnumber attributes but \
                     no toc_secnumbers entry — the normalization below would be \
                     hiding a real divergence",
                    project.name
                );
            }
            let actual = &tocs[docname];
            assert!(
                !actual.contains("secnumber="),
                "[{}] {docname}: this crate does not assign section numbers yet, \
                 so its toc must not carry secnumber attributes",
                project.name
            );

            let expected_toc = strip_secnumbers(expected);
            let expected_entries = project.expect.toc_num_entries[docname];
            let actual_entries = num_entries[docname];
            let matches = *actual == expected_toc && actual_entries == expected_entries;

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
                     --- oracle ---\n{expected_toc}--- ours ---\n{actual}",
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
// Pending: later wave-4 tasks
// ---------------------------------------------------------------------------

#[test]
#[ignore = "task 6+: needs assign_section_numbers/assign_figure_numbers"]
fn section_and_figure_numbering_matches_oracle() {
    let fixture = load_fixture();
    for project in &fixture.projects {
        let _ = &project.expect.toc_secnumbers;
        let _ = &project.expect.toc_fignumbers;
    }
    todo!(
        "diff toc_secnumbers/toc_fignumbers, and drop strip_secnumbers() from \
         local_tocs_match_oracle once the toc references carry secnumber"
    );
}

#[test]
#[ignore = "task 7+: needs std domain (labels/anonlabels/objects/progoptions/terms)"]
fn std_domain_matches_oracle() {
    let fixture = load_fixture();
    for project in &fixture.projects {
        let _ = &project.expect.std;
    }
    todo!("diff std.labels/anonlabels/objects/progoptions/terms");
}

#[test]
#[ignore = "task 8+: needs index domain entries + genindex adapter"]
fn index_and_genindex_match_oracle() {
    let fixture = load_fixture();
    for project in &fixture.projects {
        let _ = &project.expect.index_entries;
        let _ = &project.expect.genindex;
    }
    todo!("diff index_entries and IndexEntries(env).create_index() equivalent");
}

#[test]
#[ignore = "task 9+: needs get_and_resolve_doctree equivalent (post-transforms + toctree resolution)"]
fn resolved_doctrees_match_oracle() {
    let fixture = load_fixture();
    for project in &fixture.projects {
        let _ = &project.expect.resolved_pformat;
    }
    todo!("diff resolved_pformat per docname");
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

/// Toctree diagnostics are produced during the parse, which a warm cache
/// hit skips — so they have to ride the cached parse records, not be
/// recomputed. A rebuild that reports fewer warnings than a cold build is
/// the failure this guards.
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
