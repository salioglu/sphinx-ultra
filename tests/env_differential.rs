//! Differential test SKELETON: our environment layer vs the ENVIRONMENT
//! ORACLE -- `BuildEnvironment` state (toctree graph, relations, section and
//! figure numbering, std-domain registries, index entries, genindex, and
//! fully cross-reference-resolved doctrees) a real `sphinx-build` 9.1.0 read
//! + resolve phase produces for the committed multi-document project corpus.
//!
//! Regenerate the fixture (manual, never in CI):
//!     uv run --python 3.12 --with 'sphinx==9.1.0' --with 'docutils==0.22.4' \
//!         python tools/gen_env_fixture.py
//!
//! THIS FILE IS A SKELETON (M2 wave 4 task 3): it only loads the fixture,
//! asserts the version pins and the project-count floor, and defines the
//! serde types that mirror the fixture schema so later tasks can deserialize
//! it without re-deriving the shape. It does NOT yet materialize any project
//! into a tempdir or run the library's read+resolve phases -- there is no
//! `SphinxBuilder` / `snapshot_env()` hook to diff against yet (that lands in
//! task 5+). Each `#[ignore]`d stub below corresponds to one group of
//! `expect` keys; a later task un-ignores its stub, materializes the
//! project's `files` into a tempdir, runs the real build, and replaces the
//! stub body with an actual diff against a mirrored snapshot struct. By the
//! time all stubs are un-ignored (task 10), every `expect` key is covered.

use std::collections::BTreeMap;

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
// Per-expect-key stubs. Each is `#[ignore]`d until the corresponding library
// surface exists; un-ignoring one is the signal that its `expect` keys are
// now covered. Bodies below only touch the fixture (no library calls yet),
// establishing where the real materialize-and-diff logic will go.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "task 5+: needs SphinxBuilder::snapshot_env() toctree graph fields"]
fn toctree_graph_matches_oracle() {
    let fixture = load_fixture();
    for project in &fixture.projects {
        let _ = &project.expect.toctree_includes;
        let _ = &project.expect.files_to_rebuild;
        let _ = &project.expect.relations;
    }
    todo!("materialize project.files into a tempdir, build, diff toctree_includes/files_to_rebuild/relations");
}

#[test]
#[ignore = "task 6+: needs TocTreeCollector-equivalent numbering + toc pformat"]
fn toc_structure_and_numbering_matches_oracle() {
    let fixture = load_fixture();
    for project in &fixture.projects {
        let _ = &project.expect.tocs_pformat;
        let _ = &project.expect.toc_num_entries;
        let _ = &project.expect.toc_secnumbers;
        let _ = &project.expect.toc_fignumbers;
    }
    todo!("diff tocs_pformat/toc_num_entries/toc_secnumbers/toc_fignumbers");
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
