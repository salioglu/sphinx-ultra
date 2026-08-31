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
//! covered. Live so far (task 5): `tocs_pformat`, `toc_num_entries`,
//! `toctree_includes`, `files_to_rebuild`.
//!
//! Each live test materializes every fixture project into a tempdir, runs
//! the real library build (read + merge + resolve), and diffs
//! `SphinxBuilder::snapshot_env()` against the oracle. The fixture's `conf`
//! overrides are not applied: the only ones present (`smartquotes`,
//! `numfig*`) affect no key that is live yet — `smartquotes` because no
//! smart-quote transform exists, `numfig*` because they only steer
//! `toc_fignumbers`.

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

/// Materialize one project into a tempdir, build it, and return
/// `SphinxBuilder::snapshot_env()`.
fn build_project(project: &Project) -> serde_json::Value {
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

    let mut builder = SphinxBuilder::new(BuildConfig::default(), source_dir, output_dir)
        .unwrap_or_else(|e| panic!("project {}: builder setup failed: {e:#}", project.name));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(builder.build())
        .unwrap_or_else(|e| panic!("project {}: build failed: {e:#}", project.name));
    builder.snapshot_env()
}

/// Every project's environment snapshot, built once for the whole test
/// binary (each build materializes a tempdir and runs the full pipeline).
fn built_envs() -> &'static BTreeMap<String, serde_json::Value> {
    static ENVS: OnceLock<BTreeMap<String, serde_json::Value>> = OnceLock::new();
    ENVS.get_or_init(|| {
        load_fixture()
            .projects
            .iter()
            .map(|project| (project.name.clone(), build_project(project)))
            .collect()
    })
}

fn env_of(project: &Project) -> &'static serde_json::Value {
    built_envs()
        .get(&project.name)
        .unwrap_or_else(|| panic!("no build for project {}", project.name))
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
                Some(why) => assert!(
                    !matches,
                    "[{}] {docname}: listed in KNOWN_TOC_GAPS ({why}) but the toc \
                     now matches the oracle — delete the exemption",
                    project.name
                ),
                None if !matches => divergences.push(format!(
                    "[{}] {docname}: toc_num_entries {expected_entries} vs {actual_entries}\n\
                     --- oracle ---\n{expected_toc}--- ours ---\n{actual}",
                    project.name
                )),
                None => {}
            }
        }
    }

    report(&divergences, "tocs_pformat, toc_num_entries");
}

// ---------------------------------------------------------------------------
// Pending: later wave-4 tasks
// ---------------------------------------------------------------------------

#[test]
#[ignore = "task 6+: needs env.collect_relations() (toctree traversal + prev/next)"]
fn relations_match_oracle() {
    let fixture = load_fixture();
    for project in &fixture.projects {
        let _ = &project.expect.relations;
    }
    todo!("diff collect_relations() against expect.relations (None = uncomputable in sphinx)");
}

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

#[test]
#[ignore = "task 10+: needs warning text parity (byte-identical messages)"]
fn warnings_match_oracle() {
    let fixture = load_fixture();
    for project in &fixture.projects {
        let _ = &project.expect.warnings;
    }
    todo!("diff warnings, in order, byte-identical");
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
