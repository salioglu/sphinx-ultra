//! End-to-end tests that run the actual `sphinx-ultra` binary against fixture
//! projects and assert on exit codes, warnings, and the output tree.
//!
//! These tests lock in *current* behavior. Where current behavior is a known
//! ROADMAP M1 defect, the assertion documents it with a comment so the fix is
//! a deliberate test change, not an accident.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flate2::read::ZlibDecoder;
use sphinx_ultra::{BuildConfig, HTMLBuilder, InventoryFile, SearchIndex};

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sphinx-ultra"));
    // Stderr assertions must not depend on the developer/CI environment;
    // the dedicated RUST_LOG test re-sets it explicitly via .env().
    cmd.env_remove("RUST_LOG");
    // Nor may a build: a `SOURCE_DATE_EPOCH` Sphinx cannot read fails it
    // (decision 13). The tests that date a build set it via .env().
    cmd.env_remove("SOURCE_DATE_EPOCH");
    cmd
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Fresh per-test output directory under the system temp dir.
fn out_dir(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sphinx-ultra-e2e-{test_name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn build(source: &Path, out: &Path, extra: &[&str]) -> Output {
    bin()
        .arg("build")
        .arg("--source")
        .arg(source)
        .arg("--output")
        .arg(out)
        .args(extra)
        .output()
        .expect("binary should run")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn native_newline() -> &'static str {
    if cfg!(windows) {
        "\r\n"
    } else {
        "\n"
    }
}

fn search_value(out: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(out.join("searchindex.js")).unwrap();
    assert!(raw.starts_with("Search.setIndex("));
    assert!(raw.ends_with(')'));
    serde_json::from_str(&raw["Search.setIndex(".len()..raw.len() - 1]).unwrap()
}

fn assert_search_fixture(
    name: &str,
    docnames: &[&str],
    filenames: &[String],
    titles: &[&str],
    alltitles: serde_json::Value,
    titleterms: serde_json::Value,
) -> serde_json::Value {
    let out = out_dir(&format!("search-{name}"));
    let result = build(&fixture(name), &out, &[]);
    assert!(result.status.success(), "{name}: {}", stderr_of(&result));
    let value = search_value(&out);
    assert_eq!(value["docnames"], serde_json::json!(docnames), "{name}");
    assert_eq!(value["filenames"], serde_json::json!(filenames), "{name}");
    assert_eq!(value["titles"], serde_json::json!(titles), "{name}");
    assert_eq!(value["alltitles"], alltitles, "{name}");
    assert_eq!(value["titleterms"], titleterms, "{name}");
    assert_eq!(value["objects"], serde_json::json!({}), "{name}");
    assert_eq!(value["objtypes"], serde_json::json!({}), "{name}");
    assert_eq!(value["objnames"], serde_json::json!({}), "{name}");
    value
}

#[test]
fn build_succeeds_and_writes_html_tree() {
    let out = out_dir("basic-build");
    let result = build(&fixture("basic"), &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let index = out.join("index.html");
    let installation = out.join("installation.html");
    assert!(index.is_file(), "missing {}", index.display());
    assert!(installation.is_file(), "missing {}", installation.display());
    let html = std::fs::read_to_string(&index).unwrap();
    assert!(
        html.contains("Welcome"),
        "index.html should carry the title text"
    );
}

#[test]
fn build_emits_sphinx_search_index() {
    let out = out_dir("basic-search-index");
    let result = build(&fixture("basic"), &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let raw = std::fs::read_to_string(out.join("searchindex.js")).unwrap();
    assert!(raw.starts_with("Search.setIndex("));
    assert!(raw.ends_with(')'));

    let value: serde_json::Value =
        serde_json::from_str(&raw["Search.setIndex(".len()..raw.len() - ")".len()]).unwrap();
    let object = value.as_object().unwrap();
    let keys: Vec<&str> = object.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec![
            "alltitles",
            "docnames",
            "envversion",
            "filenames",
            "indexentries",
            "objects",
            "objnames",
            "objtypes",
            "terms",
            "titles",
            "titleterms",
        ]
    );
    assert_eq!(
        object["docnames"],
        serde_json::json!(["index", "installation"])
    );
    assert_eq!(
        object["filenames"],
        serde_json::json!(["index.rst", "installation.rst"])
    );
    assert_eq!(
        object["titles"],
        serde_json::json!(["Welcome", "Installation"])
    );
    assert_eq!(
        object["alltitles"],
        serde_json::json!({
            "Installation": [[1, null]],
            "Welcome": [[0, null]],
        })
    );
    // `Some` stems to the stopword `some`, so Sphinx indexes it unstemmed.
    assert_eq!(
        object["terms"],
        serde_json::json!({"Some": 1, "instal": 0, "text": 1})
    );
    assert_eq!(
        object["titleterms"],
        serde_json::json!({"instal": 1, "welcom": 0})
    );
    assert_eq!(object["objects"], serde_json::json!({}));
    assert_eq!(object["objnames"], serde_json::json!({}));
    assert_eq!(object["objtypes"], serde_json::json!({}));
    assert_eq!(object["indexentries"], serde_json::json!({}));
}

#[test]
fn build_emits_sphinx_object_inventory() {
    let out = out_dir("basic-object-inventory");
    let result = build(&fixture("basic"), &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let raw = std::fs::read(out.join("objects.inv")).unwrap();
    let mut lines = raw.splitn(5, |byte| *byte == b'\n');
    assert_eq!(
        lines.next(),
        Some(b"# Sphinx inventory version 2".as_slice())
    );
    assert_eq!(lines.next(), Some(b"# Project: Fixture".as_slice()));
    assert_eq!(lines.next(), Some(b"# Version: ".as_slice()));
    assert_eq!(
        lines.next(),
        Some(b"# The remainder of this file is compressed using zlib.".as_slice())
    );

    let mut body = String::new();
    ZlibDecoder::new(lines.next().unwrap())
        .read_to_string(&mut body)
        .unwrap();
    assert_eq!(
        body,
        concat!(
            "genindex std:label -1 genindex.html Index\n",
            "index std:doc -1 index.html Welcome\n",
            "installation std:doc -1 installation.html Installation\n",
            "modindex std:label -1 py-modindex.html Module Index\n",
            "py-modindex std:label -1 py-modindex.html Python Module Index\n",
            "search std:label -1 search.html Search Page\n",
        )
    );
}

#[test]
fn build_emits_sphinx_build_info() {
    let out = out_dir("basic-build-info");
    let result = build(&fixture("basic"), &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let raw = std::fs::read_to_string(out.join(".buildinfo")).unwrap();
    let mut lines = raw.split(native_newline());
    assert_eq!(lines.next(), Some("# Sphinx build info version 1"));
    assert_eq!(
        lines.next(),
        Some(
            "# This file records the configuration used when building these files. When it is not found, a full rebuild will be done."
        )
    );
    let config = lines.next().unwrap();
    let tags = lines.next().unwrap();
    assert!(config.starts_with("config: "));
    assert!(tags.starts_with("tags: "));
    assert_eq!(
        &config["config: ".len()..],
        "bc9d5af5c9b6d45a25e31a5ff598283a"
    );
    assert_eq!(&tags["tags: ".len()..], "645f666f9bcd5a90fca523b33c5a78b7");
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), None);
}

#[test]
fn search_index_matches_sphinx_fixture_shapes() {
    let basic = assert_search_fixture(
        "basic",
        &["index", "installation"],
        &["index.rst".to_string(), "installation.rst".to_string()],
        &["Welcome", "Installation"],
        serde_json::json!({"Installation": [[1, null]], "Welcome": [[0, null]]}),
        serde_json::json!({"instal": 1, "welcom": 0}),
    );
    assert_eq!(
        basic["terms"],
        serde_json::json!({"Some": 1, "instal": 0, "text": 1})
    );

    let missing = assert_search_fixture(
        "basic_missing_ref",
        &["index"],
        &["index.rst".to_string()],
        &["Welcome"],
        serde_json::json!({"Welcome": [[0, null]]}),
        serde_json::json!({"welcom": 0}),
    );
    assert_eq!(missing["terms"], serde_json::json!({}));

    let deps = assert_search_fixture(
        "deps_image",
        &["index", "page"],
        &["index.rst".to_string(), "page.rst".to_string()],
        &["Welcome", "Page"],
        serde_json::json!({"Page": [[1, null]], "Welcome": [[0, null]]}),
        serde_json::json!({"page": 1, "welcom": 0}),
    );
    assert_eq!(deps["terms"]["page"], serde_json::json!(0));

    let intersphinx = assert_search_fixture(
        "intersphinx",
        &["index"],
        &["index.rst".to_string()],
        &["Intersphinx Fixture"],
        serde_json::json!({"Intersphinx Fixture": [[0, null]]}),
        serde_json::json!({"fixtur": 0, "intersphinx": 0}),
    );
    // Snowball spelling is covered on the stacked stemmer branch. These are
    // the non-stemming terms that must remain in the Sphinx 9.1 index.
    for term in ["defin", "exampl", "exist", "label", "project", "target"] {
        assert!(intersphinx["terms"].get(term).is_some(), "missing {term}");
    }

    let forms = assert_search_fixture(
        "toctree_forms",
        &["index", "installation", "sub/page", "sub/sibling"],
        &[
            "index.rst".to_string(),
            "installation.rst".to_string(),
            format!("sub{}page.rst", std::path::MAIN_SEPARATOR),
            format!("sub{}sibling.rst", std::path::MAIN_SEPARATOR),
        ],
        &["Welcome", "Installation", "Page", "Sibling"],
        serde_json::json!({
            "Contents:": [[0, null]],
            "Installation": [[1, null]],
            "Page": [[2, null]],
            "Sibling": [[3, null]],
            "Welcome": [[0, null]],
        }),
        serde_json::json!({
            "content": 0,
            "instal": 1,
            "page": 2,
            "sibl": 3,
            "welcom": 0,
        }),
    );
    assert!(forms["terms"].get("sub").is_none());

    let glob = assert_search_fixture(
        "toctree_glob",
        &["index", "pages/a", "pages/b"],
        &[
            "index.rst".to_string(),
            format!("pages{}a.rst", std::path::MAIN_SEPARATOR),
            format!("pages{}b.rst", std::path::MAIN_SEPARATOR),
        ],
        &["Welcome", "Alpha", "Beta"],
        serde_json::json!({
            "Alpha": [[1, null]],
            "Beta": [[2, null]],
            "Welcome": [[0, null]],
        }),
        serde_json::json!({"alpha": 1, "beta": 2, "welcom": 0}),
    );
    assert_eq!(
        glob["terms"],
        serde_json::json!({
            "alpha": 0,
            "beta": 0,
            "first": 1,
            "page": [1, 2],
            "second": 2,
        })
    );
    assert!(glob["terms"].get("miss").is_none());
}

#[test]
fn buildinfo_hashes_match_sphinx_for_all_html_fixtures() {
    let expected = [
        ("basic", "bc9d5af5c9b6d45a25e31a5ff598283a"),
        ("basic_missing_ref", "bc9d5af5c9b6d45a25e31a5ff598283a"),
        ("deps_image", "2a19b9281246b838bad487cca62ce7dd"),
        ("intersphinx", "9df0ca2b2a4bc173c812878b2b25105c"),
        ("toctree_forms", "bc9d5af5c9b6d45a25e31a5ff598283a"),
        ("toctree_glob", "bc9d5af5c9b6d45a25e31a5ff598283a"),
    ];
    let newline = native_newline();
    for (name, config_hash) in expected {
        let out = out_dir(&format!("buildinfo-{name}"));
        let result = build(&fixture(name), &out, &[]);
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        let raw = std::fs::read_to_string(out.join(".buildinfo")).unwrap();
        assert_eq!(
            raw,
            format!(
                "# Sphinx build info version 1{newline}# This file records the configuration used when building these files. When it is not found, a full rebuild will be done.{newline}config: {config_hash}{newline}tags: 645f666f9bcd5a90fca523b33c5a78b7{newline}"
            ),
            "{name}"
        );
    }
}

#[test]
fn custom_tag_build_emits_sphinx_tag_hash() {
    let out = out_dir("buildinfo-custom-tag");
    let result = sphinx_build(&[
        fixture("basic").to_str().unwrap(),
        out.to_str().unwrap(),
        "-t",
        "custom",
    ]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let raw = std::fs::read_to_string(out.join(".buildinfo")).unwrap();
    let newline = native_newline();
    assert!(raw.contains(&format!(
        "config: bc9d5af5c9b6d45a25e31a5ff598283a{newline}"
    )));
    assert!(raw.contains(&format!("tags: c191a6fc30aa78c716b20a6a34088b42{newline}")));
}

#[test]
fn written_inventories_load_for_all_html_fixtures() {
    for name in [
        "basic",
        "basic_missing_ref",
        "deps_image",
        "intersphinx",
        "toctree_forms",
        "toctree_glob",
    ] {
        let out = out_dir(&format!("inventory-load-{name}"));
        let result = build(&fixture(name), &out, &[]);
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        let bytes = std::fs::read(out.join("objects.inv")).unwrap();
        let inventory = InventoryFile::loads(&bytes, "").unwrap();
        assert!(inventory.contains("std:doc", "index"), "{name}");
    }
}

#[test]
fn direct_html_builder_writes_search_and_inventory_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let mut builder = HTMLBuilder::new(
        BuildConfig::default(),
        root.path().join("src"),
        root.path().join("out"),
    )
    .unwrap();
    let mut index = SearchIndex::new("en".to_string());
    index
        .add_document(
            "index".to_string(),
            "index.rst".to_string(),
            "Welcome".to_string(),
            "Body",
        )
        .unwrap();
    index
        .add_object(
            "Thing".to_string(),
            "index",
            Some("thing".to_string()),
            "py:function",
            Some("Thing".to_string()),
        )
        .unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(builder.init()).unwrap();
    runtime.block_on(builder.dump_search_index(&index)).unwrap();
    runtime
        .block_on(builder.dump_object_inventory(&index))
        .unwrap();
    let value = search_value(&root.path().join("out"));
    assert_eq!(value["objtypes"]["0"], "py:function");
    let bytes = std::fs::read(root.path().join("out").join("objects.inv")).unwrap();
    let inventory = InventoryFile::loads(&bytes, "").unwrap();
    assert!(inventory.contains("py:function", "Thing"));
}

#[test]
fn first_level_section_titles_keep_their_anchor() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("index.rst"),
        "Document\n========\n\nSection\n-------\n\nBody.\n",
    )
    .unwrap();
    let out = root.path().join("out");
    let result = build(&source, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let value = search_value(&out);
    assert_eq!(
        value["alltitles"]["Section"],
        serde_json::json!([[0, "section"]])
    );
}

#[test]
fn build_with_relative_source_path_works() {
    // Regression test for the 2026-08 relative-`--source` crash.
    let out = out_dir("relative-source");
    let result = bin()
        .arg("build")
        .args(["--source", "basic"])
        .arg("--output")
        .arg(&out)
        .current_dir(fixture("basic").parent().unwrap())
        .output()
        .expect("binary should run");

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("index.html").is_file());
}

#[test]
fn missing_toctree_ref_warns_and_exits_zero() {
    let out = out_dir("missing-ref");
    let result = build(&fixture("basic_missing_ref"), &out, &[]);

    // Without -W, warnings do not fail the build.
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    // Sphinx logs this with `location=toctree` — the *directive* node, whose
    // source info is the `.. toctree::` marker line (4 here), not the entry's
    // own line. Verified against the environment oracle
    // (`toctree_self_ref`: "index.rst:4: ... nonexisting document 'index'"),
    // and the `[toc.not_readable]` suffix is what `show_warning_types`
    // appends.
    assert!(
        stderr_of(&result)
            .contains("index.rst:4: WARNING: toctree contains reference to nonexisting document 'nonexistent_page' [toc.not_readable]"),
        "expected the Sphinx-exact toctree warning, stderr: {}",
        stderr_of(&result)
    );
}

#[test]
fn toctree_forms_build_without_false_positives() {
    // Captions, `Title <doc>` entries, and document-relative targets are all
    // valid Sphinx toctree forms and must not warn.
    let out = out_dir("toctree-forms");
    let result = build(&fixture("toctree_forms"), &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        !stderr.contains("WARNING"),
        "no warnings expected for valid toctree forms, stderr: {stderr}"
    );
}

#[test]
fn toctree_glob_matches_and_warns_on_dead_pattern() {
    let out = out_dir("toctree-glob");
    let result = build(&fixture("toctree_glob"), &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        !stderr.contains("nonexisting document"),
        "glob patterns must not be treated as literal references, stderr: {stderr}"
    );
    // As above: sphinx locates this at the toctree directive (line 4). It
    // passes `subtype='empty_glob'` but no `type`, so — unlike the
    // missing-document warning — no `[...]` suffix is appended
    // (`util/logging.py:545-549`).
    assert!(
        stderr.contains(
            "index.rst:4: WARNING: toctree glob pattern 'missing*' didn't match any documents"
        ),
        "dead glob pattern must warn at the directive, stderr: {stderr}"
    );
    assert!(
        !stderr.contains("isn't included in any toctree"),
        "glob-matched documents are referenced, not orphans, stderr: {stderr}"
    );
}

#[test]
fn per_file_error_reports_and_exits_one() {
    // The failing file is created here rather than checked in: a fixture with
    // invalid UTF-8 bytes is hostile to git tooling and editors.
    let src = out_dir("broken-file-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Welcome\n=======\n\n.. toctree::\n\n   good\n",
    )
    .unwrap();
    std::fs::write(src.join("good.rst"), "Good\n----\n\nFine.\n").unwrap();
    std::fs::write(
        src.join("bad.rst"),
        b"Title\n=====\n\xFF\xFE broken\n" as &[u8],
    )
    .unwrap();

    let out = out_dir("broken-file-out");
    let result = build(&src, &out, &[]);

    assert_eq!(
        result.status.code(),
        Some(1),
        "builds with errors must exit 1 (sphinx-build parity), stderr: {}",
        stderr_of(&result)
    );
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("bad.rst: ERROR:"),
        "per-file failure must be reported, stderr: {stderr}"
    );
    assert!(
        out.join("index.html").is_file() && out.join("good.html").is_file(),
        "build must continue past the failing file"
    );
}

#[test]
fn fail_on_warning_exits_one() {
    let out = out_dir("fail-on-warning");
    let result = build(&fixture("basic_missing_ref"), &out, &["-W"]);

    assert_eq!(
        result.status.code(),
        Some(1),
        "-W must turn warnings into a failing exit"
    );
}

/// A document reachable from two toctrees is an *information* notice in
/// Sphinx (`logger.info`, `environment/__init__.py:950-959`), not a
/// warning — so `-W` must not turn it into a failing build.
#[test]
fn multiple_toctree_parents_do_not_fail_under_fail_on_warning() {
    let src = out_dir("multi-parent-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   a\n   b\n",
    )
    .unwrap();
    std::fs::write(src.join("a.rst"), "A\n=\n\n.. toctree::\n\n   c\n").unwrap();
    std::fs::write(src.join("b.rst"), "B\n=\n\n.. toctree::\n\n   c\n").unwrap();
    std::fs::write(src.join("c.rst"), "C\n=\n\nShared leaf.\n").unwrap();

    let out = out_dir("multi-parent-out");
    let result = build(&src, &out, &["-W"]);

    let stderr = stderr_of(&result);
    assert!(
        result.status.success(),
        "the multiple-parents notice must not count as a warning, stderr: {stderr}"
    );
    assert!(
        !stderr.contains("WARNING"),
        "no warnings expected for a shared toctree leaf, stderr: {stderr}"
    );
}

#[test]
fn warning_file_is_written() {
    let out = out_dir("warning-file");
    let warnings_path = out_dir("warning-file-log").join("warnings.txt");
    let result = build(
        &fixture("basic_missing_ref"),
        &out,
        &["-w", warnings_path.to_str().unwrap()],
    );

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let contents = std::fs::read_to_string(&warnings_path).expect("warning file should exist");
    assert!(
        contents.contains("WARNING: toctree contains reference to nonexisting document"),
        "warning file contents: {contents}"
    );
}

#[test]
fn clean_removes_output_dir() {
    let out = out_dir("clean");
    let result = build(&fixture("basic"), &out, &[]);
    assert!(result.status.success());
    assert!(out.exists());

    let clean = bin()
        .arg("clean")
        .arg("--output")
        .arg(&out)
        .output()
        .expect("binary should run");
    assert!(clean.status.success());
    assert!(!out.exists(), "clean must remove the output directory");
}

#[test]
fn config_flag_accepts_conf_py() {
    let out = out_dir("config-conf-py");
    let conf_dir = out_dir("config-conf-py-src");
    std::fs::create_dir_all(&conf_dir).unwrap();
    let conf = conf_dir.join("conf.py");
    std::fs::write(&conf, "project = 'E2E'\n").unwrap();

    let result = bin()
        .arg("--config")
        .arg(&conf)
        .arg("build")
        .arg("--source")
        .arg(fixture("basic"))
        .arg("--output")
        .arg(&out)
        .output()
        .expect("binary should run");

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("index.html").is_file());
}

#[test]
fn conf_py_multiline_and_dynamic_warning() {
    let conf_dir = out_dir("conf-py-multiline-src");
    std::fs::create_dir_all(&conf_dir).unwrap();
    let conf = conf_dir.join("conf.py");
    std::fs::write(
        &conf,
        "import os\n\
         project = 'Multi'\n\
         extensions = [\n    'sphinx.ext.autodoc',\n    'sphinx.ext.viewcode',\n]\n\
         html_theme_options = {\n    'collapse_navigation': False,\n}\n\
         release = os.environ['RELEASE']\n",
    )
    .unwrap();

    let out = out_dir("conf-py-multiline-out");
    let result = bin()
        .arg("--config")
        .arg(&conf)
        .arg("build")
        .arg("--source")
        .arg(fixture("basic"))
        .arg("--output")
        .arg(&out)
        .output()
        .expect("binary should run");

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("conf.py:10: unsupported value for 'release'"),
        "dynamic values must warn with their line, stderr: {stderr}"
    );
}

#[test]
fn config_flag_accepts_partial_yaml() {
    let out = out_dir("config-partial-yaml");
    let conf_dir = out_dir("config-partial-yaml-src");
    std::fs::create_dir_all(&conf_dir).unwrap();
    let conf = conf_dir.join("custom.yaml");
    std::fs::write(&conf, "project: 'Partial'\n").unwrap();

    let result = bin()
        .arg("--config")
        .arg(&conf)
        .arg("build")
        .arg("--source")
        .arg(fixture("basic"))
        .arg("--output")
        .arg(&out)
        .output()
        .expect("binary should run");

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("index.html").is_file());
}

#[test]
fn incremental_cache_hit_still_writes_output() {
    let out = out_dir("cache-write-on-hit");
    let run1 = build(&fixture("basic"), &out, &["--incremental"]);
    assert!(run1.status.success(), "stderr: {}", stderr_of(&run1));

    // Simulate lost output while the cache stays warm: a cache hit must
    // still emit the page, never skip writing.
    std::fs::remove_file(out.join("index.html")).unwrap();
    std::fs::remove_file(out.join("installation.html")).unwrap();

    let run2 = build(&fixture("basic"), &out, &["--incremental"]);
    assert!(run2.status.success(), "stderr: {}", stderr_of(&run2));
    let stderr = stderr_of(&run2);
    assert!(
        stderr.contains("Cache hits: 2"),
        "second run should hit the cache for both docs, stderr: {stderr}"
    );
    assert!(
        out.join("index.html").is_file() && out.join("installation.html").is_file(),
        "cache hits must still write output files"
    );
}

/// A document is outdated when a file it pulls in is newer than the build
/// that read it — not only when its own source changes. The `deps_image`
/// fixture's `page.rst` embeds `pic.png`; touching the picture must re-read
/// that page and nothing else.
#[test]
fn touching_an_embedded_image_re_reads_only_the_page_that_embeds_it() {
    let src = out_dir("deps-image-src");
    std::fs::create_dir_all(&src).unwrap();
    for name in ["conf.py", "index.rst", "page.rst", "pic.png"] {
        std::fs::copy(fixture("deps_image").join(name), src.join(name)).unwrap();
    }
    let out = out_dir("deps-image-out");

    let run1 = build(&src, &out, &["--incremental"]);
    assert!(run1.status.success(), "stderr: {}", stderr_of(&run1));

    let run2 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run2).contains("Cache hits: 2"),
        "an unchanged project reads nothing, stderr: {}",
        stderr_of(&run2)
    );

    // Touch the picture: its mtime moves past the time `page` was read.
    let picture = src.join("pic.png");
    let bytes = std::fs::read(&picture).unwrap();
    std::fs::write(&picture, &bytes).unwrap();

    let run3 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run3).contains("Cache hits: 1"),
        "the page embedding the touched image must be re-read, stderr: {}",
        stderr_of(&run3)
    );
    assert!(
        out.join("page.html").is_file() && out.join("index.html").is_file(),
        "both pages are still written"
    );

    let run4 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run4).contains("Cache hits: 2"),
        "the re-read settled the dependency, stderr: {}",
        stderr_of(&run4)
    );
}

/// docutils records the files `csv-table :file:` and `raw :file:` read
/// (`settings.record_dependencies.add(source)`, `tables.py`, `misc.py`),
/// which Sphinx's `DependenciesCollector` notes: touching either re-reads
/// the page that reads it, and only that page. Probed on Sphinx 9.1.0: a
/// warm build after the edit re-reads the page (`1 changed`) and indexes
/// the new cell's word.
#[test]
fn touching_a_csv_or_raw_file_re_reads_the_page_that_reads_it() {
    let src = temp_source(
        "deps-csv-raw",
        &[
            ("index.rst", "Index\n=====\n\n.. toctree::\n\n   page\n"),
            (
                "page.rst",
                "Page\n====\n\n.. csv-table:: T\n   :file: data.csv\n\n\
                 .. raw:: html\n   :file: snippet.html\n",
            ),
            ("data.csv", "a, b\n"),
            ("snippet.html", "<p>raw</p>\n"),
        ],
    );
    let out = out_dir("deps-csv-raw-out");
    let hits = |run: &Output, expected: &str, why: &str| {
        assert!(
            stderr_of(run).contains(expected),
            "{why}, stderr: {}",
            stderr_of(run)
        );
    };
    let run1 = build(&src, &out, &["--incremental"]);
    assert!(run1.status.success(), "stderr: {}", stderr_of(&run1));
    hits(
        &build(&src, &out, &["--incremental"]),
        "Cache hits: 2",
        "an unchanged project reads nothing",
    );

    std::fs::write(src.join("data.csv"), "a, uniquecsvword\n").unwrap();
    hits(
        &build(&src, &out, &["--incremental"]),
        "Cache hits: 1",
        "the csv file's page is re-read",
    );
    let terms = search_value(&out)["terms"].clone();
    assert!(
        terms.get("uniquecsvword").is_some(),
        "the new cell is indexed: {terms}"
    );
    hits(
        &build(&src, &out, &["--incremental"]),
        "Cache hits: 2",
        "the re-read settled it",
    );

    std::fs::write(src.join("snippet.html"), "<p>uniquerawword</p>\n").unwrap();
    hits(
        &build(&src, &out, &["--incremental"]),
        "Cache hits: 1",
        "the raw file's page is re-read",
    );
}

/// The include dependency + orphan wiring, end to end (wave 4.5 T12,
/// [INC PROBE 6] matrix): a cold build records the include targets as
/// dependencies, a warm build reads nothing, touching an included `.rst`
/// re-reads the includer AND the included doc (it is discovered and read
/// standalone too), touching an included `.txt`, which the default
/// `source_suffix` makes no document, re-reads the includer alone, and an
/// included-only doc never earns the orphan warning. Sphinx 9.1.0's
/// `reading sources` lines on the same project read `a`, `index`, `part`
/// cold, nothing warm, `a` and `part` after `part.rst` is touched, `a`
/// alone after `snippet.txt` is, and nothing once settled (probe
/// `scratchpad/t17c/p3_misc.py`, `touch`).
#[test]
fn touching_an_included_file_re_reads_the_documents_that_pull_it_in() {
    let src = out_dir("deps-include-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   a\n",
    )
    .unwrap();
    std::fs::write(
        src.join("a.rst"),
        "A\n=\n\n.. include:: part.rst\n\n.. include:: snippet.txt\n",
    )
    .unwrap();
    std::fs::write(src.join("part.rst"), "part para\n").unwrap();
    std::fs::write(src.join("snippet.txt"), "plain snippet\n").unwrap();
    let out = out_dir("deps-include-out");

    let run1 = build(&src, &out, &["--incremental"]);
    assert!(run1.status.success(), "stderr: {}", stderr_of(&run1));
    assert!(
        !stderr_of(&run1).contains("isn't included in any toctree"),
        "part.rst is reachable through the include; nothing is orphaned: {}",
        stderr_of(&run1)
    );

    // Warm no-op: the three documents hit the cache, and the orphan check
    // replayed from the persisted env stays quiet.
    let run2 = build(&src, &out, &["--incremental"]);
    let stderr2 = stderr_of(&run2);
    assert!(
        stderr2.contains("Cache hits: 3"),
        "an unchanged project reads nothing, stderr: {stderr2}"
    );
    assert!(
        !stderr2.contains("isn't included in any toctree"),
        "{stderr2}"
    );

    // Touch the included .rst: outdates the includer (via
    // env.dependencies) and itself (its own document) — PROBE 6's
    // changed={a, part}.
    let part = src.join("part.rst");
    let bytes = std::fs::read(&part).unwrap();
    std::fs::write(&part, &bytes).unwrap();
    let run3 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run3).contains("Cache hits: 1"),
        "touch part.rst re-reads a + part, stderr: {}",
        stderr_of(&run3)
    );

    // Touch the included .txt: outdates the includer, through
    // env.dependencies, and nothing else — it is no document.
    let snippet = src.join("snippet.txt");
    let bytes = std::fs::read(&snippet).unwrap();
    std::fs::write(&snippet, &bytes).unwrap();
    let run4 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run4).contains("Cache hits: 2"),
        "touch snippet.txt re-reads a alone, stderr: {}",
        stderr_of(&run4)
    );

    // The re-reads settled every dependency.
    let run5 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run5).contains("Cache hits: 3"),
        "stderr: {}",
        stderr_of(&run5)
    );
}

#[test]
fn clean_incremental_build_produces_full_output() {
    let out = out_dir("clean-incremental");
    let run1 = build(&fixture("basic"), &out, &["--incremental"]);
    assert!(run1.status.success(), "stderr: {}", stderr_of(&run1));

    let run2 = build(&fixture("basic"), &out, &["--clean", "--incremental"]);
    assert!(run2.status.success(), "stderr: {}", stderr_of(&run2));
    assert!(
        out.join("index.html").is_file() && out.join("installation.html").is_file(),
        "--clean --incremental must produce a complete output tree"
    );
}

#[test]
fn config_change_invalidates_cache() {
    let src = out_dir("cache-config-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Welcome\n=======\n\n.. toctree::\n\n   other\n",
    )
    .unwrap();
    std::fs::write(src.join("other.rst"), "Other\n-----\n\nText.\n").unwrap();
    std::fs::write(src.join("sphinx-ultra.yaml"), "project: 'A'\n").unwrap();

    let out = out_dir("cache-config-out");
    let run1 = build(&src, &out, &["--incremental"]);
    assert!(run1.status.success(), "stderr: {}", stderr_of(&run1));

    let run2 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run2).contains("Cache hits: 2"),
        "unchanged config should reuse the cache, stderr: {}",
        stderr_of(&run2)
    );

    std::fs::write(src.join("sphinx-ultra.yaml"), "project: 'B'\n").unwrap();
    let run3 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run3).contains("Cache hits: 0"),
        "config change must invalidate the cache, stderr: {}",
        stderr_of(&run3)
    );
}

/// Sphinx's default `source_suffix` is `.rst` alone, so a `README.md` or
/// a `.txt` in the source tree is no document, and no orphan: it earns no
/// `toc.not_included` warning. An orphaned `.rst` still does.
#[test]
fn a_non_rst_file_is_not_reported_as_an_orphan() {
    let src = out_dir("orphan-suffixes-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("conf.py"), "project = 'p'\n").unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   page\n",
    )
    .unwrap();
    std::fs::write(src.join("page.rst"), "Page\n====\n\nBody.\n").unwrap();
    std::fs::write(src.join("README.md"), "# Readme\n").unwrap();
    std::fs::write(src.join("notes.txt"), "Notes\n=====\n\nBody.\n").unwrap();
    std::fs::write(src.join("stray.rst"), "Stray\n=====\n\nBody.\n").unwrap();

    let out = out_dir("orphan-suffixes-out");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap()]);

    let stderr = stderr_of(&result);
    assert!(
        !stderr.contains("README.md"),
        "a .md file is not a document Sphinx reads, stderr: {stderr}"
    );
    assert!(
        !stderr.contains("notes.txt"),
        "nor is a .txt file, stderr: {stderr}"
    );
    assert!(
        stderr.contains("stray.rst") && stderr.contains("isn't included in any toctree"),
        "an orphaned .rst still warns, stderr: {stderr}"
    );
}

/// Builds `src` in sphinx-build mode (`-q -w <file>`, then `extra`) into
/// `name`'s output directory: the process's output, the text of the `-w`
/// file, which these tests compare whole, and the output directory.
fn sphinx_build_warnings(name: &str, src: &Path, extra: &[&str]) -> (Output, String, PathBuf) {
    let out = out_dir(name);
    let warnings = out_dir(&format!("{name}-log")).join("warnings.txt");
    let mut args = vec![
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-w",
        warnings.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let result = sphinx_build(&args);
    let written = std::fs::read_to_string(&warnings).unwrap_or_default();
    (result, written, out)
}

/// Discovery keeps a file only when its name ends with a `source_suffix`
/// (`Project.discover`, `SP/sphinx/project.py:49-112`), `.rst` alone by
/// default (`config.py:243`): neither the `.txt` an `include` reads nor a
/// `README.md` is a document, so a toctree entry or a `:doc:` naming one
/// names a document the project does not have. Sphinx 9.1.0 builds the
/// same project with exactly these five records and writes no page for
/// those files (P08, [RC §4.5]; probe `scratchpad/t17c/p3_misc.py`,
/// `gate`). The native subcommand discovers the same way
/// (`touching_an_included_file_re_reads_the_documents_that_pull_it_in`).
#[test]
fn source_suffix_gates_discovery() {
    let src = temp_source(
        "source-suffix-gate",
        &[
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   notes\n   readme\n   notes.txt\n\n\
                 :doc:`notes` :doc:`readme`\n\n.. include:: snippet.txt\n",
            ),
            ("notes.txt", "Notes\n=====\n\nPlain *text* file.\n"),
            ("readme.md", "# Readme\n\nSome *markdown*.\n"),
            ("snippet.txt", "Included *snippet*.\n"),
            ("README.md", "# Project\n"),
        ],
    );
    let (result, written, out) = sphinx_build_warnings("source-suffix-gate", &src, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let index = sphinx_ultra::utils::canonicalize_simplified(&src)
        .unwrap()
        .join("index.rst");
    let index = index.display();
    assert_eq!(
        written,
        format!(
            "{index}:4: WARNING: toctree contains reference to nonexisting document 'notes' \
             [toc.not_readable]\n\
             {index}:4: WARNING: toctree contains reference to nonexisting document 'readme' \
             [toc.not_readable]\n\
             {index}:4: WARNING: toctree contains reference to nonexisting document \
             'notes.txt' [toc.not_readable]\n\
             {index}:10: WARNING: unknown document: 'notes' [ref.doc]\n\
             {index}:10: WARNING: unknown document: 'readme' [ref.doc]\n"
        )
    );
    assert!(out.join("index.html").is_file());
    for page in ["notes", "readme", "snippet", "README"] {
        assert!(
            !out.join(format!("{page}.html")).exists(),
            "{page} is no document, so it has no page"
        );
    }
}

/// Discovery leaves out exactly what Sphinx's `env.find_files` does
/// (`SP/sphinx/environment/__init__.py:485-495`): `exclude_patterns`,
/// `templates_path`, `html_extra_path` and `html_static_path`, each entry
/// an exclude pattern as written, matched against the srcdir-relative path
/// (a directory it matches is not walked), then `Project.discover`'s
/// `EXCLUDE_PATHS` (`**/_sources`, `.#*`, `**/.#*`, `*.lproj/**`,
/// `SP/sphinx/project.py:20`). Nothing else: a `_build/`, a hidden
/// directory, `__pycache__` and `.git` are walked like any other. So an
/// autosummary template (Jinja text) under `templates_path`, an `.rst` under
/// an asset path and a markdown file under `html_static_path` are no
/// documents, while a top-level `_sources/` or a nested `en.lproj/`, which
/// no pattern matches, holds documents. `-D` sets the three settings as
/// lists. Sphinx 9.1.0 builds each project with an empty warning stream and
/// these pages (`scratchpad/t17c-fix3/p1b_pins.py`). The native subcommand
/// leaves out the same paths and its own: `_build/`, hidden directories,
/// `__pycache__`.
#[test]
fn discovery_excludes_what_sphinx_excludes() {
    let index = |entries: &[&str]| {
        let mut text = "Index\n=====\n\n.. toctree::\n\n".to_string();
        for entry in entries {
            text.push_str(&format!("   {entry}\n"));
        }
        text
    };
    let page = |title: &str| format!("{title}\n{}\n\nText.\n", "=".repeat(title.len()));
    let only_a = index(&["a"]);
    let a = page("A");
    // `<out>/<docname>.html`, built a component at a time.
    let page_path = |out: &Path, docname: &str| {
        let mut path = out.join(docname.split('/').collect::<PathBuf>());
        let name = format!("{}.html", path.file_name().unwrap().to_string_lossy());
        path.set_file_name(name);
        path
    };
    let documents = |out: &Path, pages: &[&str], not: &[&str]| {
        for page in pages {
            let path = page_path(out, page);
            assert!(path.is_file(), "{} is written", path.display());
        }
        for page in not {
            assert!(!page_path(out, page).exists(), "{page} is no document");
        }
    };

    let assets = [
        (
            "conf.py",
            "project = 'p'\ntemplates_path = ['_templates']\nhtml_static_path = ['_static']\n\
             html_extra_path = ['_extra']\n"
                .to_string(),
        ),
        ("index.rst", only_a.clone()),
        ("a.rst", a.clone()),
        (
            "_templates/autosummary/class.rst",
            "{{ fullname | escape | underline}}\n\n.. currentmodule:: {{ module }}\n\n\
             .. autoclass:: {{ objname }}\n   :members:\n"
                .to_string(),
        ),
        ("_static/s.rst", page("S")),
        ("_extra/e.rst", page("E")),
    ];
    let assets: Vec<(&str, &str)> = assets.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let src = temp_source("t17c-discovery-assets", &assets);
    let out = out_dir("t17c-discovery-assets");
    let (result, written) = warning_stream(&src, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, "");
    assert_eq!(stderr_of(&result), "");
    documents(
        &out,
        &["index", "a"],
        &["_templates/autosummary/class", "_static/s", "_extra/e"],
    );
    // The native subcommand leaves them out too.
    let native = out_dir("t17c-discovery-assets-native");
    let result = build(&src, &native, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    documents(
        &native,
        &["index", "a"],
        &["_templates/autosummary/class", "_static/s", "_extra/e"],
    );

    let overridden = [
        ("index.rst", only_a.clone()),
        ("a.rst", a.clone()),
        ("_t/t.rst", page("T")),
        ("_s/s.rst", page("S")),
        ("_s2/s2.rst", page("S2")),
        ("_e/e.rst", page("E")),
    ];
    let overridden: Vec<(&str, &str)> = overridden.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let src = temp_source("t17c-discovery-assets-d", &overridden);
    let out = out_dir("t17c-discovery-assets-d");
    let (result, written) = warning_stream(
        &src,
        &out,
        &[
            "-D",
            "templates_path=_t",
            "-D",
            "html_static_path=_s,_s2",
            "-D",
            "html_extra_path=_e",
        ],
    );
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, "");
    assert_eq!(stderr_of(&result), "");
    documents(&out, &["index", "a"], &["_t/t", "_s/s", "_s2/s2", "_e/e"]);

    // A markdown file under `html_static_path` is no document, so its file
    // type stops nothing.
    let markdown = [
        (
            "conf.py",
            "project = 'p'\nhtml_static_path = ['_static']\n\
             source_suffix = {'.rst': 'restructuredtext', '.md': 'markdown'}\n"
                .to_string(),
        ),
        ("index.rst", only_a.clone()),
        ("a.rst", a.clone()),
        ("_static/readme.md", "# R\n".to_string()),
    ];
    let markdown: Vec<(&str, &str)> = markdown.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let src = temp_source("t17c-discovery-static-markdown", &markdown);
    let out = out_dir("t17c-discovery-static-markdown");
    let (result, written) = warning_stream(&src, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, "");
    assert_eq!(stderr_of(&result), "");
    documents(&out, &["index", "a"], &[]);

    // Sphinx walks what this crate's native subcommand leaves out, and
    // `EXCLUDE_PATHS` reaches only what its patterns match.
    let walked = [
        "_build/b",
        ".hidden/h",
        "__pycache__/p",
        ".git/g",
        "_sources/s",
        "sub/en.lproj/m",
    ];
    let excluded = ["sub/_sources/t", "en.lproj/l", ".#b", "sub/.#c"];
    let mut entries = vec!["a"];
    entries.extend_from_slice(&walked);
    let built_ins_index = index(&entries);
    let mut built_ins = vec![
        ("conf.py".to_string(), "project = 'p'\n".to_string()),
        ("index.rst".to_string(), built_ins_index),
        ("a.rst".to_string(), a.clone()),
    ];
    for docname in walked.iter().chain(&excluded) {
        built_ins.push((format!("{docname}.rst"), page("Page")));
    }
    let built_ins: Vec<(&str, &str)> = built_ins
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let src = temp_source("t17c-discovery-built-ins", &built_ins);
    let out = out_dir("t17c-discovery-built-ins");
    let (result, written) = warning_stream(&src, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, "");
    assert_eq!(stderr_of(&result), "");
    documents(&out, &walked, &excluded);
    documents(&out, &["index", "a"], &[]);
    // The native subcommand keeps its own exclusions on top of Sphinx's.
    let native = out_dir("t17c-discovery-built-ins-native");
    let result = build(&src, &native, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    documents(
        &native,
        &["index", "a", "_sources/s", "sub/en.lproj/m"],
        &["_build/b", ".hidden/h", "__pycache__/p", ".git/g"],
    );
    documents(&native, &[], &excluded);
}

/// A logger warning's location goes through Sphinx's `doc2path`
/// (`SphinxLogRecordTranslator`, `SP/sphinx/util/logging.py:500-525`;
/// `Project.doc2path`, `SP/sphinx/project.py:114-128`): `location=docname`
/// and `(docname, lineno)` name the file the document was discovered in,
/// `.rst` or `.txt`, while a `(source, line)` tuple, which passes a source
/// path where a docname goes, finds no document of that name and takes
/// `source_suffix[0]` after it. Sphinx 9.1.0 prints exactly these records
/// for both orders of the two suffixes (probe `scratchpad/t17c/p3_misc.py`,
/// `loc_rst_first` and `loc_txt_first`).
#[test]
fn logger_locations_use_the_first_source_suffix() {
    for (order, first) in [("'.rst', '.txt'", ".rst"), ("'.txt', '.rst'", ".txt")] {
        let name = format!("first-source-suffix{first}");
        let src = temp_source(
            &name,
            &[
                (
                    "conf.py",
                    &format!(
                        "project = 'p'\nsource_suffix = [{order}]\n\
                         default_role = 'nonexistent'\n"
                    ),
                ),
                (
                    "index.rst",
                    "Index\n=====\n\n.. toctree::\n\n   notes\n\n\
                     .. code-block:: python\n   :emphasize-lines: 5\n\n   x = 1\n\n\
                     .. hlist::\n\n   text\n",
                ),
                (
                    "notes.txt",
                    "Notes\n=====\n\n.. code-block:: python\n   :emphasize-lines: 7\n\n   \
                     y = 2\n\n.. hlist::\n\n   more\n",
                ),
            ],
        );
        let (result, written, out) = sphinx_build_warnings(&name, &src, &[]);

        assert!(result.status.success(), "stderr: {}", stderr_of(&result));
        let source = sphinx_ultra::utils::canonicalize_simplified(&src).unwrap();
        let path = |name: String| source.join(name).display().to_string();
        assert_eq!(
            written,
            format!(
                "{index}: WARNING: default role nonexistent not found\n\
                 {index_tuple}:8: WARNING: line number spec is out of range(1-1): '5'\n\
                 {index}:13: WARNING: .. hlist content is not a list\n\
                 {notes}: WARNING: default role nonexistent not found\n\
                 {notes_tuple}:4: WARNING: line number spec is out of range(1-1): '7'\n\
                 {notes}:9: WARNING: .. hlist content is not a list\n",
                index = path("index.rst".to_string()),
                index_tuple = path(format!("index.rst{first}")),
                notes = path("notes.txt".to_string()),
                notes_tuple = path(format!("notes.txt{first}")),
            ),
            "source_suffix = [{order}]"
        );
        assert!(out.join("notes.html").is_file(), "notes.txt is a document");
    }
}

/// `-D source_suffix=<text>` sets one suffix: the option is typed `Any`
/// (`SP/sphinx/config.py:243`), so `convert_overrides` passes the text as
/// given and `convert_source_suffix` makes it `{text: 'restructuredtext'}`
/// (`:354-358`, `:636-667`). With `.txt` the `.txt` files are the
/// documents, the `.rst` beside them is none, and a toctree entry is
/// stripped of `.txt`. Sphinx 9.1.0 prints exactly this record and writes
/// these pages (probe `scratchpad/t17c/p3_misc.py`, `D_txt_design`).
#[test]
fn a_source_suffix_override_names_the_suffix_documents_end_with() {
    let src = temp_source(
        "source-suffix-override-txt",
        &[
            (
                "index.txt",
                "Index\n=====\n\n.. toctree::\n\n   page.txt\n   other\n",
            ),
            ("page.txt", "Page\n====\n\ntxt page.\n"),
            ("other.rst", "Other\n=====\n\nrst page.\n"),
        ],
    );
    let (result, written, out) = sphinx_build_warnings(
        "source-suffix-override-txt",
        &src,
        &["-D", "source_suffix=.txt"],
    );

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let index = sphinx_ultra::utils::canonicalize_simplified(&src)
        .unwrap()
        .join("index.txt");
    assert_eq!(
        written,
        format!(
            "{}:4: WARNING: toctree contains reference to nonexisting document 'other' \
             [toc.not_readable]\n",
            index.display()
        )
    );
    assert!(out.join("index.html").is_file() && out.join("page.html").is_file());
    assert!(!out.join("other.html").exists(), "other.rst is no document");
}

/// Sphinx's read stops when the documents it read do not include the root
/// document (`SP/sphinx/builders/__init__.py:527-568`): a `SphinxError`,
/// after the read's records, exit 2. Its path is `doc2path(root_doc)`,
/// `<srcdir>/<root_doc><source_suffix[0]>` for a document the project has
/// no file for, and its message names the first reason that holds, each
/// pattern matched against that path, absolute: a built-in exclude pattern,
/// a conf.py exclude pattern (`**index.rst`; `index.rst`, which matches
/// only the relative path, leads to the generic message), custom
/// include patterns none of which matches (`[]`, which matches no file,
/// included), otherwise the generic one, as for `-D source_suffix=.txt` on
/// a project of `.rst` files. A `.#index.rst` root is a file discovery
/// leaves out, by the built-in pattern `.#*`; a root under
/// `html_static_path` is one too, which the message does not name. Sphinx
/// raises the error on purpose, so the report ends with it
/// ([`root_document_error`]). Probed on Sphinx 9.1.0
/// (`scratchpad/t17c-fix1/p4_root.py`: `D_txt_records`, `exclude_starstar`,
/// `exclude_relative`, `include_custom`, `builtin_dot_hash`,
/// `builtin_dot_hash_file`; `scratchpad/t17c-fix3/p3_include_empty.py`:
/// `include_empty`; `scratchpad/t17c-fix3/p1c_root_variants.py`:
/// `root_under_static`). The native subcommand has no such rule; with
/// `include_patterns = []` it finds no document either.
#[test]
fn a_read_that_did_not_find_the_root_document_exits_two() {
    let index = ("index.rst", "Index\n=====\n\n.. toctree::\n\n   a\n");
    let a = ("a.rst", "A\n=\n\nSee :nope:`x` here.\n");
    let a_record = "<SRCDIR>/a.rst:4: ERROR: Unknown interpreted text role \"nope\". [docutils]\n";
    let unable = |message: &str| root_document_error(message);
    let generic = |path: &str| {
        unable(&format!(
            "({path}). The master document must be within the source directory or a \
             subdirectory of it."
        ))
    };

    let stream = crash_stream(
        "t17c-root-d-txt",
        &[
            index,
            a,
            ("other.txt", "Other\n=====\n\nSee :nope:`y` here.\n"),
        ],
        &[],
        &["-D", "source_suffix=.txt"],
    );
    assert_eq!(
        stream,
        format!(
            "<SRCDIR>/other.txt:4: ERROR: Unknown interpreted text role \"nope\". [docutils]\n{}",
            generic("<SRCDIR>/index.txt")
        )
    );

    let stream = crash_stream(
        "t17c-root-excluded",
        &[
            (
                "conf.py",
                "project = 'p'\nexclude_patterns = ['_build', '**index.rst']\n",
            ),
            index,
            a,
        ],
        &[],
        &[],
    );
    assert_eq!(
        stream,
        format!(
            "{a_record}{}",
            unable(
                "(<SRCDIR>/index.rst) because it matches an exclude pattern specified in \
                 conf.py, '**index.rst'. Please remove this pattern from conf.py."
            )
        )
    );

    let stream = crash_stream(
        "t17c-root-excluded-relative",
        &[
            (
                "conf.py",
                "project = 'p'\nexclude_patterns = ['index.rst']\n",
            ),
            index,
            a,
        ],
        &[],
        &[],
    );
    assert_eq!(
        stream,
        format!("{a_record}{}", generic("<SRCDIR>/index.rst"))
    );

    let stream = crash_stream(
        "t17c-root-not-included",
        &[
            ("conf.py", "project = 'p'\ninclude_patterns = ['a.rst']\n"),
            index,
            a,
        ],
        &[],
        &[],
    );
    assert_eq!(
        stream,
        format!(
            "{a_record}{}",
            unable(
                "(<SRCDIR>/index.rst) because it is not included in the custom \
                 include_patterns = ['a.rst']. Ensure that a pattern in include_patterns \
                 matches the master document."
            )
        )
    );

    let built_in = unable(
        "(<SRCDIR>/.#index.rst) because it matches a built-in exclude pattern '**/.#*'. \
         Please move your master document to a different location.",
    );
    let stream = crash_stream(
        "t17c-root-built-in",
        &[
            ("conf.py", "project = 'p'\nroot_doc = '.#index'\n"),
            index,
            a,
        ],
        &[],
        &[],
    );
    assert_eq!(stream, format!("{a_record}{built_in}"));
    let stream = crash_stream(
        "t17c-root-built-in-file",
        &[
            ("conf.py", "project = 'p'\nroot_doc = '.#index'\n"),
            index,
            a,
            (".#index.rst", "X\n=\n"),
        ],
        &[],
        &[],
    );
    assert_eq!(stream, format!("{a_record}{built_in}"));
    // A root discovery leaves out because it lies under an asset path is
    // named by neither variant: the check names only the built-in patterns
    // and conf.py's `exclude_patterns`.
    let stream = crash_stream(
        "t17c-root-under-static",
        &[
            (
                "conf.py",
                "project = 'p'\nroot_doc = '_static/index'\nhtml_static_path = ['_static']\n",
            ),
            index,
            a,
            ("_static/index.rst", index.1),
        ],
        &[],
        &[],
    );
    assert_eq!(
        stream,
        format!("{a_record}{}", generic("<SRCDIR>/_static/index.rst"))
    );

    let include_empty = [
        ("conf.py", "project = 'p'\ninclude_patterns = []\n"),
        index,
        a,
    ];
    let stream = crash_stream("t17c-root-include-empty", &include_empty, &[], &[]);
    assert_eq!(
        stream,
        unable(
            "(<SRCDIR>/index.rst) because it is not included in the custom include_patterns \
             = []. Ensure that a pattern in include_patterns matches the master document."
        )
    );
    let src = temp_source("t17c-root-include-empty-native", &include_empty);
    let out = out_dir("t17c-root-include-empty-native");
    let result = build(&src, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(!out.join("index.html").exists() && !out.join("a.html").exists());

    // The native subcommand builds the documents it has, root or none.
    let src = temp_source(
        "t17c-root-excluded-native",
        &[
            (
                "conf.py",
                "project = 'p'\nexclude_patterns = ['_build', '**index.rst']\n",
            ),
            index,
            a,
        ],
    );
    let out = out_dir("t17c-root-excluded-native");
    let result = build(&src, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("a.html").is_file() && !out.join("index.html").exists());
}

/// Sphinx's `check_master_doc` (`SP/sphinx/config.py:861-883`), the
/// `env-get-outdated` handler its read runs once discovery is done: when
/// the root document is `index` (the default, or `index` set in conf.py or
/// with `-D root_doc=index`), `index` is no document and `contents` is, it
/// warns and makes `contents` the root. The root-document check then
/// passes, the orphan check spares `contents`, and every page is written.
/// The record prints after `builder-inited`'s (intersphinx's) and before
/// the read's, on every build: Sphinx pickles the configuration it
/// changed, so its next build finds `master_doc` changed and reads every
/// document again (`[config changed ('master_doc')]`), which is why the
/// warm builds print `contents`'s read record too. Nothing falls back when
/// `index` is a document, when `contents` is none (excluded, or only a
/// `sub/contents`), or when the root is another document. Probed on Sphinx
/// 9.1.0 (`scratchpad/t17c-fix2/p1_contents.py`, `p2_records.py`).
#[test]
fn a_missing_index_falls_back_to_a_contents_root_document() {
    const FALLBACK: &str = "WARNING: Sphinx now uses \"index\" as the master document by \
                            default. To keep pre-2.0 behaviour, set \"master_doc = \
                            'contents'\".\n";
    let contents = (
        "contents.rst",
        "Contents\n========\n\n.. toctree::\n\n   a\n",
    );
    let a = ("a.rst", "A\n=\n\nText.\n");

    let src = temp_source(
        "t17c-contents-records",
        &[
            (
                "conf.py",
                "project = 'p'\nhtml_logo = 'missing.png'\n\
                 extensions = ['sphinx.ext.intersphinx']\n\
                 intersphinx_mapping = {'x': ('https://example.invalid/', 'bad.inv')}\n",
            ),
            (
                "bad.inv",
                "Not a Sphinx inventory header\nsome other content\n",
            ),
            (
                "contents.rst",
                "Contents\n========\n\n.. toctree::\n\n   a\n\nSee :nope:`x` here.\n",
            ),
            ("a.rst", "A\n=\n\nSee :ref:`nowhere`.\n"),
            ("orphan.rst", "Orphan\n======\n\nNot in a toctree.\n"),
        ],
    );
    let out = out_dir("t17c-contents-records");
    let stream = format!(
        "WARNING: logo file 'missing.png' does not exist\n\
         WARNING: failed to reach any of the inventories with the following issues:\n\
         unknown or unsupported inventory version: ValueError('invalid inventory header: Not \
         a Sphinx inventory header')\n\
         {FALLBACK}\
         <SRCDIR>/contents.rst:8: ERROR: Unknown interpreted text role \"nope\". [docutils]\n\
         <SRCDIR>/orphan.rst: WARNING: document isn't included in any toctree \
         [toc.not_included]\n\
         <SRCDIR>/a.rst:4: WARNING: undefined label: 'nowhere' [ref.ref]\n"
    );
    for build in ["cold", "warm", "warm again"] {
        let (result, written) = warning_stream(&src, &out, &[]);
        assert!(
            result.status.success(),
            "{build}: stderr: {}",
            stderr_of(&result)
        );
        assert_eq!(written, stream, "{build}");
    }
    for page in ["a", "contents", "orphan"] {
        assert!(out.join(format!("{page}.html")).is_file(), "{page}.html");
    }
    assert!(!out.join("index.html").exists());

    // The root is `index` however it is set, and `index` is no document
    // discovery found, an excluded `index.rst` included.
    let index = ("index.rst", "Index\n=====\n");
    for (test, conf, files, extra) in [
        (
            "t17c-contents-default",
            "project = 'p'\n",
            &[contents, a][..],
            &[][..],
        ),
        (
            "t17c-contents-root-doc",
            "project = 'p'\nroot_doc = 'index'\n",
            &[contents, a][..],
            &[][..],
        ),
        (
            "t17c-contents-master-doc",
            "project = 'p'\nmaster_doc = 'index'\n",
            &[contents, a][..],
            &[][..],
        ),
        (
            "t17c-contents-d-root-doc",
            "project = 'p'\n",
            &[contents, a][..],
            &["-D", "root_doc=index"][..],
        ),
        (
            "t17c-contents-index-excluded",
            "project = 'p'\nexclude_patterns = ['index.rst']\n",
            &[contents, a, index][..],
            &[][..],
        ),
    ] {
        let mut project = vec![("conf.py", conf)];
        project.extend_from_slice(files);
        let src = temp_source(test, &project);
        let out = out_dir(test);
        let (result, written) = warning_stream(&src, &out, extra);
        assert!(
            result.status.success(),
            "{test}: stderr: {}",
            stderr_of(&result)
        );
        assert_eq!(written, FALLBACK, "{test}");
        assert!(
            out.join("a.html").is_file() && out.join("contents.html").is_file(),
            "{test}"
        );
    }

    // The record counts toward `-W`.
    let src = temp_source("t17c-contents-fail-on-warning", &[contents, a]);
    let (result, written) =
        warning_stream(&src, &out_dir("t17c-contents-fail-on-warning"), &["-W"]);
    assert_eq!(
        result.status.code(),
        Some(1),
        "stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(written, FALLBACK);

    // With `index` a document nothing falls back: `contents` is an orphan.
    let src = temp_source(
        "t17c-contents-and-index",
        &[
            ("index.rst", "Index\n=====\n\n.. toctree::\n\n   a\n"),
            contents,
            a,
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir("t17c-contents-and-index"), &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "<SRCDIR>/contents.rst: WARNING: document isn't included in any toctree \
         [toc.not_included]\n"
    );

    // With `contents` no document, or another root, the root check stops
    // the build.
    let unable = |path: &str| {
        root_document_error(&format!(
            "({path}). The master document must be within the source directory or a \
             subdirectory of it."
        ))
    };
    for (test, conf, files, path) in [
        (
            "t17c-contents-other-root",
            "project = 'p'\nroot_doc = 'other'\n",
            &[contents, a][..],
            "<SRCDIR>/other.rst",
        ),
        (
            "t17c-contents-excluded",
            "project = 'p'\nexclude_patterns = ['contents.rst']\n",
            &[contents, a][..],
            "<SRCDIR>/index.rst",
        ),
        (
            "t17c-contents-in-sub",
            "project = 'p'\n",
            &[("sub/contents.rst", "Contents\n========\n"), a][..],
            "<SRCDIR>/index.rst",
        ),
    ] {
        let mut project = vec![("conf.py", conf)];
        project.extend_from_slice(files);
        assert_eq!(
            crash_stream(test, &project, &[], &[]),
            unable(path),
            "{test}"
        );
    }

    // A document this crate cannot read stops the build after the record:
    // the configuration error of
    // `a_source_suffix_this_crate_cannot_build_is_a_configuration_error`,
    // where Sphinx reads `a` and `contents` first and stops at `readme`.
    let src = temp_source(
        "t17c-contents-markdown",
        &[
            (
                "conf.py",
                "project = 'p'\n\
                 source_suffix = {'.rst': 'restructuredtext', '.md': 'markdown'}\n",
            ),
            contents,
            a,
            ("readme.md", "# Readme\n"),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir("t17c-contents-markdown"), &[]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(
        written,
        format!(
            "{FALLBACK}\nConfiguration error!\n\n    sphinx.errors.ConfigError: source_suffix \
             maps '.md' to the file type markdown, and the document 'readme' \
             (<SRCDIR>/readme.md) has that suffix: sphinx-ultra reads only reStructuredText \
             documents; set -D source_suffix=<suffix> to build the documents of one suffix \
             without editing conf.py\n"
        )
    );
}

/// `master_doc` and `root_doc` are one setting (`Config.__setattr__` copies
/// either into the other, `SP/sphinx/config.py:434-439`), which
/// `check_confval_types` reads as `master_doc` first (`:239-240`): the root
/// is the first of a `-D master_doc`, conf.py's `master_doc`, a
/// `-D root_doc` and conf.py's `root_doc`, whatever order conf.py assigns
/// them in or the overrides come in, and `index` without any. The project's
/// documents are all orphans but the root, so the orphan records name the
/// root by its absence. On a `contents` project, `-D master_doc=contents`
/// leaves nothing to fall back from, `-D master_doc=index` falls back over
/// conf.py's `root_doc = 'contents'`, and conf.py's `master_doc =
/// 'contents'` keeps `-D root_doc=index` from it. Sphinx 9.1.0 prints
/// exactly these records (`scratchpad/t17c-fix3/p2_root_alias.py`).
#[test]
fn master_doc_and_root_doc_are_one_setting() {
    const MASTER: &str = "master_doc = 'a'\n";
    const ROOT: &str = "root_doc = 'b'\n";
    const D_MASTER: [&str; 2] = ["-D", "master_doc=c"];
    const D_ROOT: [&str; 2] = ["-D", "root_doc=d"];
    let docs = ["index", "a", "b", "c", "d"];
    let pages: Vec<(String, String)> = docs
        .iter()
        .map(|doc| {
            let title = doc.to_uppercase();
            (
                format!("{doc}.rst"),
                format!("{title}\n{}\n\nText.\n", "=".repeat(title.len())),
            )
        })
        .collect();
    let both = [D_MASTER, D_ROOT].concat();
    let both_reversed = [D_ROOT, D_MASTER].concat();
    for (test, conf, extra, root) in [
        ("t17c-alias-none", String::new(), &[][..], "index"),
        ("t17c-alias-master", MASTER.to_string(), &[][..], "a"),
        ("t17c-alias-root", ROOT.to_string(), &[][..], "b"),
        ("t17c-alias-d-master", String::new(), &D_MASTER[..], "c"),
        ("t17c-alias-d-root", String::new(), &D_ROOT[..], "d"),
        (
            "t17c-alias-master-root",
            format!("{MASTER}{ROOT}"),
            &[][..],
            "a",
        ),
        (
            "t17c-alias-root-master",
            format!("{ROOT}{MASTER}"),
            &[][..],
            "a",
        ),
        (
            "t17c-alias-d-master-master",
            MASTER.to_string(),
            &D_MASTER[..],
            "c",
        ),
        (
            "t17c-alias-d-master-root",
            ROOT.to_string(),
            &D_MASTER[..],
            "c",
        ),
        (
            "t17c-alias-d-root-master",
            MASTER.to_string(),
            &D_ROOT[..],
            "a",
        ),
        ("t17c-alias-d-root-root", ROOT.to_string(), &D_ROOT[..], "d"),
        (
            "t17c-alias-d-root-both",
            format!("{MASTER}{ROOT}"),
            &D_ROOT[..],
            "a",
        ),
        (
            "t17c-alias-d-master-both",
            format!("{MASTER}{ROOT}"),
            &D_MASTER[..],
            "c",
        ),
        ("t17c-alias-d-both", String::new(), &both[..], "c"),
        (
            "t17c-alias-d-both-reversed",
            String::new(),
            &both_reversed[..],
            "c",
        ),
        (
            "t17c-alias-d-both-over-both",
            format!("{ROOT}{MASTER}"),
            &both_reversed[..],
            "c",
        ),
    ] {
        let conf = format!("project = 'p'\n{conf}");
        let mut files = vec![("conf.py", conf.as_str())];
        files.extend(
            pages
                .iter()
                .map(|(path, text)| (path.as_str(), text.as_str())),
        );
        let src = temp_source(test, &files);
        let out = out_dir(test);
        let (result, written) = warning_stream(&src, &out, extra);
        assert!(
            result.status.success(),
            "{test}: stderr: {}",
            stderr_of(&result)
        );
        let mut orphans: Vec<&str> = docs.iter().copied().filter(|doc| *doc != root).collect();
        orphans.sort();
        let expected: String = orphans
            .iter()
            .map(|doc| {
                format!(
                    "<SRCDIR>/{doc}.rst: WARNING: document isn't included in any toctree \
                     [toc.not_included]\n"
                )
            })
            .collect();
        assert_eq!(written, expected, "{test}");
        assert_eq!(srcdir_spelled(&result.stderr, &src), written, "{test}");
    }

    const FALLBACK: &str = "WARNING: Sphinx now uses \"index\" as the master document by \
                            default. To keep pre-2.0 behaviour, set \"master_doc = \
                            'contents'\".\n";
    let contents = (
        "contents.rst",
        "Contents\n========\n\n.. toctree::\n\n   a\n",
    );
    let a = ("a.rst", "A\n=\n\nText.\n");
    for (test, conf, extra, expected) in [
        (
            "t17c-alias-contents-d-master",
            "",
            &["-D", "master_doc=contents"][..],
            "",
        ),
        (
            "t17c-alias-contents-d-master-index",
            "",
            &["-D", "master_doc=index"][..],
            FALLBACK,
        ),
        (
            "t17c-alias-contents-root-d-master-index",
            "root_doc = 'contents'\n",
            &["-D", "master_doc=index"][..],
            FALLBACK,
        ),
        (
            "t17c-alias-contents-master-d-root-index",
            "master_doc = 'contents'\n",
            &["-D", "root_doc=index"][..],
            "",
        ),
    ] {
        let conf = format!("project = 'p'\n{conf}");
        let src = temp_source(test, &[("conf.py", conf.as_str()), contents, a]);
        let out = out_dir(test);
        let (result, written) = warning_stream(&src, &out, extra);
        assert!(
            result.status.success(),
            "{test}: stderr: {}",
            stderr_of(&result)
        );
        assert_eq!(written, expected, "{test}");
        assert_eq!(srcdir_spelled(&result.stderr, &src), written, "{test}");
        assert!(out.join("contents.html").is_file(), "{test}");
    }
}

/// An HTTP server on 127.0.0.1 that answers every request with `body` and
/// keeps the path of each `GET`: a remote intersphinx inventory, without
/// the network. It serves until the test process ends.
fn serve_inventory(body: Vec<u8>) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let gets = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = gets.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => request.push(byte[0]),
                    _ => break,
                }
            }
            let request = String::from_utf8_lossy(&request);
            if let Some(path) = request.strip_prefix("GET ") {
                let path = path.split(' ').next().unwrap_or_default();
                seen.lock().unwrap().push(path.to_string());
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    (port, gets)
}

/// Sphinx keeps the inventories intersphinx fetched in its doctree
/// directory (`__intersphinx_cache__`), which neither a configuration
/// change nor `-E` deletes, and reads a project's file there whenever its
/// memory holds no inventory for the target URI and the file is younger
/// than `intersphinx_cache_limit` (`SP/sphinx/ext/intersphinx/_load.py:
/// 264-280`). So a project fetches its inventory once: cold, then never on
/// a warm build, a configuration change, `-E`, or a new target URI, whose
/// links take the bytes already fetched. That holds for the `contents`
/// fallback, whose every build is a configuration change too. Sphinx 9.1.0
/// fetches exactly this often, and resolves the reference on every build
/// (`scratchpad/t17c-fix3/p5_intersphinx_cache.py`): no record.
#[test]
fn the_inventory_cache_outlives_a_configuration_change() {
    let (port, gets) =
        serve_inventory(std::fs::read(fixture("intersphinx").join("local.inv")).unwrap());
    let conf = |uri: &str, extra: &str| {
        format!(
            "project = 'p'\nextensions = ['sphinx.ext.intersphinx']\n\
             intersphinx_mapping = {{'x': ('{uri}', None)}}\n{extra}"
        )
    };
    let base = format!("http://127.0.0.1:{port}/");
    // The build of `src` into `out`, `-q -w`, with no proxy between it and
    // the server: the paths it fetched, and its warning stream.
    let fetched = |src: &Path, out: &Path, extra: &[&str]| {
        let log = out.with_extension("w");
        let before = gets.lock().unwrap().len();
        let mut command = bin();
        command
            .arg(src)
            .arg(out)
            .args(["-q", "-w"])
            .arg(&log)
            .args(extra);
        for key in [
            "http_proxy",
            "HTTP_PROXY",
            "https_proxy",
            "HTTPS_PROXY",
            "all_proxy",
            "ALL_PROXY",
        ] {
            command.env_remove(key);
        }
        let result = command.output().expect("binary should run");
        assert!(result.status.success(), "stderr: {}", stderr_of(&result));
        let written = srcdir_spelled(&std::fs::read(&log).unwrap(), src);
        (gets.lock().unwrap()[before..].to_vec(), written)
    };
    let root = |name: &str| {
        (
            format!("{name}.rst"),
            "Root\n====\n\n.. toctree::\n\n   a\n\nSee :ref:`x:example`.\n".to_string(),
        )
    };
    let a = ("a.rst", "A\n=\n\nText.\n");

    let (index, index_text) = root("index");
    let control_conf = conf(&base, "");
    let src = temp_source(
        "t17c-inventory-cache",
        &[
            ("conf.py", control_conf.as_str()),
            (index.as_str(), index_text.as_str()),
            a,
        ],
    );
    let out = out_dir("t17c-inventory-cache");
    let inventory = vec!["/objects.inv".to_string()];
    assert_eq!(
        fetched(&src, &out, &[]),
        (inventory.clone(), String::new()),
        "cold"
    );
    assert_eq!(fetched(&src, &out, &[]), (vec![], String::new()), "warm");
    std::fs::write(src.join("conf.py"), conf(&base, "html_title = 'T'\n")).unwrap();
    assert_eq!(fetched(&src, &out, &[]), (vec![], String::new()), "changed");
    assert_eq!(fetched(&src, &out, &["-E"]), (vec![], String::new()), "-E");
    let moved = format!("{base}v2/");
    std::fs::write(src.join("conf.py"), conf(&moved, "html_title = 'T'\n")).unwrap();
    assert_eq!(fetched(&src, &out, &[]), (vec![], String::new()), "moved");

    const FALLBACK: &str = "WARNING: Sphinx now uses \"index\" as the master document by \
                            default. To keep pre-2.0 behaviour, set \"master_doc = \
                            'contents'\".\n";
    let (contents, contents_text) = root("contents");
    let src = temp_source(
        "t17c-inventory-cache-fallback",
        &[
            ("conf.py", control_conf.as_str()),
            (contents.as_str(), contents_text.as_str()),
            a,
        ],
    );
    let out = out_dir("t17c-inventory-cache-fallback");
    let fallback = FALLBACK.to_string();
    assert_eq!(
        fetched(&src, &out, &[]),
        (inventory, fallback.clone()),
        "cold"
    );
    for build in ["warm", "warm again"] {
        assert_eq!(
            fetched(&src, &out, &[]),
            (vec![], fallback.clone()),
            "{build}"
        );
    }
}

/// `-D source_suffix=.rst,.txt` is the one suffix `.rst,.txt`: no comma
/// splits it, as none splits Sphinx's (see the test above). The project's
/// documents are the files whose name ends with it, `index.rst,.txt`
/// alone, whose page is `index.html`; `page.rst` and `page.txt` are none,
/// and the toctree entry `page.rst,.txt` is stripped to `page`. Sphinx
/// 9.1.0 prints exactly these records and writes this page (probe
/// `scratchpad/t17c/p3_misc.py`, `D_rst_txt_design`).
#[test]
fn a_source_suffix_override_is_not_split_on_commas() {
    let src = temp_source(
        "source-suffix-override-comma",
        &[
            (
                "index.rst,.txt",
                "Index\n=====\n\n.. toctree::\n\n   page\n   page.rst,.txt\n",
            ),
            ("page.rst", "Page\n====\n\nrst page.\n"),
            ("page.txt", "Page\n====\n\ntxt page.\n"),
        ],
    );
    let (result, written, out) = sphinx_build_warnings(
        "source-suffix-override-comma",
        &src,
        &["-D", "source_suffix=.rst,.txt"],
    );

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let index = sphinx_ultra::utils::canonicalize_simplified(&src)
        .unwrap()
        .join("index.rst,.txt");
    let record = format!(
        "{}:4: WARNING: toctree contains reference to nonexisting document 'page' \
         [toc.not_readable]\n",
        index.display()
    );
    assert_eq!(written, format!("{record}{record}"));
    assert!(out.join("index.html").is_file());
    assert!(
        !out.join("page.html").exists(),
        "neither page file is a document"
    );
}

/// A `source_suffix` value that is no string, list or dict is Sphinx's own
/// `ConfigError`, raised where `convert_source_suffix` runs (`config-inited`
/// @800, `SP/sphinx/config.py:636-667`), after the records logged before it
/// (`deprecate_source_encoding` @790; probe `scratchpad/t17c/
/// p7_config_error.py`).
///
/// A file type other than `restructuredtext` is no error while no document
/// has it, in this crate as in Sphinx, which builds such a project as if the
/// suffix were not there (probe `scratchpad/t17c-fix1/p1_filetype.py`:
/// `md_type_no_md_file`, `md_type_md_excluded`, Sphinx 9.1.0's whole `-w`).
/// A document of that type that discovery finds, after `exclude_patterns`
/// apply, stops the build before any document is read, with a
/// configuration error naming it: this crate has no parser for it. Sphinx
/// reads such a document through the parser an extension registers for it,
/// and stops at it without one (`Source parser for markdown not
/// registered`, after the records of the documents read before it:
/// `md_type_md_file`), a deliberate divergence. `-D source_suffix`
/// replaces what `conf.py` says, as Sphinx's does: the project then builds
/// with the records Sphinx prints for it (probe `scratchpad/t17c/
/// p1_suffix.py`, `dict_md_D`).
#[test]
fn a_source_suffix_this_crate_cannot_build_is_a_configuration_error() {
    let src = temp_source(
        "source-suffix-int",
        &[
            (
                "conf.py",
                "project = 'p'\nsource_encoding = 'latin-1'\nsource_suffix = 5\n",
            ),
            ("index.rst", "Index\n=====\n"),
        ],
    );
    let (result, written, _) = sphinx_build_warnings("source-suffix-int", &src, &[]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(
        written,
        "WARNING: Support for source encodings other than UTF-8 is deprecated and will be \
         removed in Sphinx 10. Please comment at \
         https://github.com/sphinx-doc/sphinx/issues/13665 if this causes a problem.\n\
         \nConfiguration error!\n\n    sphinx.errors.ConfigError: The config value \
         `source_suffix' expects a dictionary, a string, or a list of strings. Got `5' instead \
         (type <class 'int'>).\n"
    );

    let markdown = "project = 'p'\n\
                    source_suffix = {'.rst': 'restructuredtext', '.md': 'markdown'}\n";
    let index = (
        "index.rst",
        "Index\n=====\n\n.. toctree::\n\n   notes\n   readme\n   notes.txt\n\n\
         :doc:`notes` :doc:`readme`\n",
    );
    let notes = ("notes.txt", "Notes\n=====\n\nPlain *text* file.\n");
    let readme = ("readme.md", "# Readme\n\nSome *markdown*.\n");
    // What Sphinx 9.1.0 prints for the project once `readme.md` is no
    // document, whichever way it is not.
    let records = |src: &Path| {
        let index = sphinx_ultra::utils::canonicalize_simplified(src)
            .unwrap()
            .join("index.rst");
        let index = index.display();
        format!(
            "{index}:4: WARNING: toctree contains reference to nonexisting document 'notes' \
             [toc.not_readable]\n\
             {index}:4: WARNING: toctree contains reference to nonexisting document 'readme' \
             [toc.not_readable]\n\
             {index}:4: WARNING: toctree contains reference to nonexisting document \
             'notes.txt' [toc.not_readable]\n\
             {index}:10: WARNING: unknown document: 'notes' [ref.doc]\n\
             {index}:10: WARNING: unknown document: 'readme' [ref.doc]\n"
        )
    };

    let src = temp_source(
        "source-suffix-markdown-unused",
        &[("conf.py", markdown), index, notes],
    );
    let (result, written, _) = sphinx_build_warnings("source-suffix-markdown-unused", &src, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, records(&src));

    let src = temp_source(
        "source-suffix-markdown-excluded",
        &[
            (
                "conf.py",
                &format!("{markdown}exclude_patterns = ['readme.md']\n"),
            ),
            index,
            notes,
            readme,
        ],
    );
    let (result, written, _) = sphinx_build_warnings("source-suffix-markdown-excluded", &src, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, records(&src));

    // The report of the project at `src` holding `readme.md`.
    let report = |src: &Path| {
        format!(
            "\nConfiguration error!\n\n    sphinx.errors.ConfigError: source_suffix maps '.md' \
             to the file type markdown, and the document 'readme' ({}) has that suffix: \
             sphinx-ultra reads only reStructuredText documents; set -D source_suffix=<suffix> \
             to build the documents of one suffix without editing conf.py\n",
            sphinx_ultra::utils::canonicalize_simplified(src)
                .unwrap()
                .join("readme.md")
                .display()
        )
    };
    let src = temp_source(
        "source-suffix-markdown",
        &[("conf.py", markdown), index, notes, readme],
    );
    let (result, written, _) = sphinx_build_warnings("source-suffix-markdown", &src, &[]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    let report_here = report(&src);
    assert_eq!(written, report_here);
    assert!(
        stderr_of(&result).ends_with(&report_here),
        "stderr: {}",
        stderr_of(&result)
    );

    let (result, written, out) = sphinx_build_warnings(
        "source-suffix-markdown-override",
        &src,
        &["-D", "source_suffix=.rst"],
    );
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, records(&src));
    assert!(!out.join("readme.html").exists());

    // The records the build prints before discovery print first.
    let src = temp_source(
        "source-suffix-markdown-records",
        &[
            (
                "conf.py",
                &format!("{markdown}source_encoding = 'latin-1'\n"),
            ),
            index,
            notes,
            readme,
        ],
    );
    let (result, written, _) = sphinx_build_warnings("source-suffix-markdown-records", &src, &[]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(
        written,
        format!(
            "WARNING: Support for source encodings other than UTF-8 is deprecated and will be \
             removed in Sphinx 10. Please comment at \
             https://github.com/sphinx-doc/sphinx/issues/13665 if this causes a problem.\n\
             {}",
            report(&src)
        )
    );
}

/// Decision 13: a `source_suffix` list holding an item that is no string
/// is one Sphinx 9.1.0 keeps (`dict.fromkeys`) and crashes on when its
/// discovery first tests a file name against it (`TypeError`,
/// `Project.path2doc`), which with a fresh environment it does while it
/// sets its application up: after every `config-inited` record (the type
/// check and `check_primary_domain` among them), before `builder-inited`
/// loads the inventories, whose record it never prints. An item Python
/// cannot hash crashes `convert_source_suffix` itself, a `config-inited`
/// handler, before the records of the handlers after it. `-D
/// source_suffix` replaces either. Probes `scratchpad/t17c/p1_suffix.py`
/// (`list_nonstr`), `scratchpad/t17c-fix1/p5c_nonstr_records.py`.
#[test]
fn a_source_suffix_item_sphinx_crashes_on_exits_two() {
    const ENCODING: &str = "WARNING: Support for source encodings other than UTF-8 is \
                            deprecated and will be removed in Sphinx 10. Please comment at \
                            https://github.com/sphinx-doc/sphinx/issues/13665 if this causes a \
                            problem.\n";
    const CHECKS: &str = "WARNING: The config value `maximum_signature_line_length' has type \
                          `str'; expected `NoneType' or `int'.\n\
                          WARNING: primary_domain 'nope' not found, ignored.\n";
    let conf = |source_suffix: &str| {
        format!(
            "project = 'p'\nsource_encoding = 'latin-1'\nmaximum_signature_line_length = 'x'\n\
             primary_domain = 'nope'\nextensions = ['sphinx.ext.intersphinx']\n\
             intersphinx_mapping = {{'x': ('https://example.invalid/', 'missing.inv')}}\n\
             source_suffix = {source_suffix}\n"
        )
    };

    let conf_int = conf("['.rst', 1]");
    let stream = crash_stream(
        "t17c-suffix-int-item",
        &[("conf.py", &conf_int), ("index.rst", "Index\n=====\n")],
        &[],
        &[],
    );
    assert_eq!(
        stream,
        format!(
            "{ENCODING}{CHECKS}{}",
            crash_report(
                None,
                "TypeError: endswith first arg must be str or a tuple of str, not int",
                None
            )
        )
    );

    let conf_list = conf("['.rst', ['.txt']]");
    let stream = crash_stream(
        "t17c-suffix-list-item",
        &[("conf.py", &conf_list), ("index.rst", "Index\n=====\n")],
        &[],
        &[],
    );
    assert_eq!(
        stream,
        format!(
            "{ENCODING}{}",
            crash_report(
                Some("Extension error (sphinx.config)"),
                "sphinx.errors.ExtensionError: Handler <function convert_source_suffix at \
                 0x...> for event 'config-inited' threw an exception (exception: unhashable \
                 type: 'list')",
                None
            )
        )
    );

    let src = temp_source(
        "t17c-suffix-int-item-override",
        &[
            (
                "conf.py",
                "project = 'p'\nsource_encoding = 'latin-1'\nmaximum_signature_line_length = \
                 'x'\nprimary_domain = 'nope'\nsource_suffix = ['.rst', 1]\n",
            ),
            ("index.rst", "Index\n=====\n"),
        ],
    );
    let (result, written, out) = sphinx_build_warnings(
        "t17c-suffix-int-item-override",
        &src,
        &["-D", "source_suffix=.rst"],
    );
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, format!("{ENCODING}{CHECKS}"));
    assert!(out.join("index.html").is_file());
}

/// `TocTree.parse_content` tells an excluded entry from a nonexisting one
/// by matching `exclude_patterns` against `doc2path(entry, False)`, which
/// for a document the project does not have is the entry plus
/// `source_suffix[0]` (`SP/sphinx/directives/other.py:150-166`): under
/// `exclude_patterns = ['missing.txt']`, `missing` is excluded when `.txt`
/// comes first and nonexisting when `.rst` does. Probed on Sphinx 9.1.0
/// (`scratchpad/t17c/p3_misc.py`, `exc_rst_first` and `exc_txt_first`).
#[test]
fn a_toctree_entry_is_matched_against_exclude_patterns_with_the_first_source_suffix() {
    for (order, first, verdict) in [
        (
            "'.rst', '.txt'",
            "rst",
            "nonexisting document 'missing' [toc.not_readable]",
        ),
        (
            "'.txt', '.rst'",
            "txt",
            "excluded document 'missing' [toc.excluded]",
        ),
    ] {
        let name = format!("toctree-excluded-first-suffix-{first}");
        let src = temp_source(
            &name,
            &[
                (
                    "conf.py",
                    &format!(
                        "project = 'p'\nsource_suffix = [{order}]\n\
                         exclude_patterns = ['missing.txt']\n"
                    ),
                ),
                (
                    "index.rst",
                    "Index\n=====\n\n.. toctree::\n\n   missing\n   gone\n",
                ),
            ],
        );
        let (result, written, _) = sphinx_build_warnings(&name, &src, &[]);

        assert!(result.status.success(), "stderr: {}", stderr_of(&result));
        let index = sphinx_ultra::utils::canonicalize_simplified(&src)
            .unwrap()
            .join("index.rst");
        let index = index.display();
        assert_eq!(
            written,
            format!(
                "{index}:4: WARNING: toctree contains reference to {verdict}\n\
                 {index}:4: WARNING: toctree contains reference to nonexisting document 'gone' \
                 [toc.not_readable]\n"
            ),
            "source_suffix = [{order}]"
        );
    }
}

/// Two files mapping to one docname are not two documents: every piece of
/// per-document state downstream of discovery is keyed by docname alone,
/// so one of them has to go (`Project.discover`,
/// `SP/sphinx/project.py:64-79`). Under `source_suffix = ['.rst', '.txt']`
/// `page.rst` and `page.txt` collide (`page.md` beside them is no
/// document), and Sphinx keeps the first its walk yields, which takes a
/// directory's files in sorted order: `page.rst`, in either order of the
/// suffixes. It warns `multiple files found for the document "page"` once
/// on every build, cold or warm, since discovery runs on each, which `-W`
/// turns into a failure. Probed on Sphinx 9.1.0 (`scratchpad/t17c/
/// p3_misc.py`, `collision`, `collision_txt_first`).
#[test]
fn two_files_for_one_docname_warn_and_keep_the_first_in_sorted_order() {
    for order in ["'.rst', '.txt'", "'.txt', '.rst'"] {
        let name = format!("duplicate-docname-{}", &order[2..5]);
        let src = out_dir(&format!("{name}-src"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("conf.py"),
            format!("project = 'p'\nsource_suffix = [{order}]\n"),
        )
        .unwrap();
        std::fs::write(
            src.join("index.rst"),
            "Index\n=====\n\n.. toctree::\n\n   page\n",
        )
        .unwrap();
        std::fs::write(src.join("page.rst"), "Page RST\n========\n\nrst body.\n").unwrap();
        std::fs::write(src.join("page.md"), "Page MD\n=======\n\nmd body.\n").unwrap();
        std::fs::write(src.join("page.txt"), "Page TXT\n========\n\ntxt body.\n").unwrap();

        // The output directory persists across the two builds: the second
        // is warm.
        let out = out_dir(&format!("{name}-out"));
        let page = out.join("page.html");
        let mut cold = None;
        for build in ["cold", "warm"] {
            let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap()]);
            let stderr = stderr_of(&result);
            assert!(
                result.status.success(),
                "{order}, {build}: stderr: {stderr}"
            );
            assert_eq!(
                stderr
                    .matches("multiple files found for the document \"page\"")
                    .count(),
                1,
                "{order}, {build}: one record, stderr: {stderr}"
            );
            let rendered = std::fs::read_to_string(&page).unwrap();
            assert!(
                rendered.contains("rst body"),
                "{order}, {build}: page.rst wins: {rendered}"
            );
            // The defect this pins was an output page that alternated
            // between the two sources.
            assert_eq!(
                cold.get_or_insert_with(|| rendered.clone()),
                &rendered,
                "{order}: the page must not alternate between the sources"
            );
        }
        let strict = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-W"]);
        assert_eq!(
            strict.status.code(),
            Some(1),
            "{order}: -W fails on the record, stderr: {}",
            stderr_of(&strict)
        );
    }
}

/// `:numbered:` takes an optional depth (`int_or_nothing` in Sphinx's
/// `TocTree.option_spec`), so `:numbered: 2` is the documented spelling of
/// the feature, not an error. The retained M1 directive validator had it
/// filed as a flag option and warned on every use, failing `-W` on a
/// project sphinx 9.1.0 builds clean.
#[test]
fn a_numbered_toctree_with_a_depth_builds_clean() {
    let src = out_dir("toctree-numbered-depth-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("conf.py"), "project = 'p'\n").unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n   :numbered: 2\n   :maxdepth: 2\n\n   a\n",
    )
    .unwrap();
    std::fs::write(src.join("a.rst"), "A\n=\n\nBody.\n").unwrap();

    let out = out_dir("toctree-numbered-depth-out");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-W"]);

    let stderr = stderr_of(&result);
    assert!(
        !stderr.contains("numbered option should not have a value"),
        "`:numbered: 2` is valid input, stderr: {stderr}"
    );
    assert!(
        result.status.success(),
        "sphinx builds this clean, so -W must pass, stderr: {stderr}"
    );
}

/// A `.. _label:` written above a figure/table/code-block labels it exactly
/// as the `:name:` option does — docutils' `PropagateTargets` moves the ids
/// onto the node before Sphinx numbers it. Numbering and `:numref:` must
/// agree on the propagated id, or every reference to such a node fails with
/// "Any number is not assigned" and takes `-W` down with it. sphinx 9.1.0
/// builds this project clean and renders `Fig. 1` / `Fig. 2`.
#[test]
fn a_label_above_a_figure_numbers_it_and_numref_resolves() {
    let src = out_dir("numfig-propagated-label-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("conf.py"), "project = 'p'\nnumfig = True\n").unwrap();
    std::fs::write(src.join("pic.png"), b"x").unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Index\n=====\n\n\
         .. figure:: pic.png\n   :name: fig1\n\n   A caption.\n\n\
         .. _fig2:\n\n.. figure:: pic.png\n\n   Another caption.\n\n\
         See :numref:`fig1` and :numref:`fig2`.\n",
    )
    .unwrap();

    let out = out_dir("numfig-propagated-label-out");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-W"]);

    let stderr = stderr_of(&result);
    assert!(
        !stderr.contains("Any number is not assigned"),
        "the labelled figure must be numbered, stderr: {stderr}"
    );
    assert!(
        result.status.success(),
        "sphinx builds this clean, so -W must pass, stderr: {stderr}"
    );
}

/// `-W` and `-n` are operational flags, not configuration: adding either
/// must not invalidate the cache. Sphinx cannot invalidate on them
/// (`nitpicky`'s rebuild class is `''`, `warningiserror` is not a `Config`
/// value at all), and if this crate did, the forced cold read would re-emit
/// every read-phase warning — so the first `-W` run would fail and the next
/// identical one would pass.
#[test]
fn operational_flags_do_not_invalidate_the_cache() {
    let src = out_dir("cache-operational-flags-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   a\n   b\n",
    )
    .unwrap();
    // b.rst redefines a.rst's label: one read-phase warning, emitted only
    // by a build that actually reads b.
    std::fs::write(src.join("a.rst"), ".. _dup:\n\nA\n=\n\nBody.\n").unwrap();
    std::fs::write(src.join("b.rst"), ".. _dup:\n\nB\n=\n\nBody.\n").unwrap();
    std::fs::write(src.join("conf.py"), "project = 'flags'\n").unwrap();

    // sphinx-build compat mode: incremental by default, and the only mode
    // that accepts both -W and -n.
    let out = out_dir("cache-operational-flags-out");
    let s = src.to_str().unwrap().to_string();
    let o = out.to_str().unwrap().to_string();

    let run1 = sphinx_build(&[&s, &o]);
    assert!(run1.status.success(), "stderr: {}", stderr_of(&run1));
    assert!(
        stderr_of(&run1).contains("duplicate label dup"),
        "the cold build reports the duplicate label, stderr: {}",
        stderr_of(&run1)
    );

    // The build's progress log, `Cache hits` among it, prints with `-v`:
    // without it sphinx-build mode's stderr is the warning stream alone.
    let run2 = sphinx_build(&[&s, &o, "-v"]);
    assert!(
        stderr_of(&run2).contains("Cache hits: 3"),
        "an unchanged project reads nothing, stderr: {}",
        stderr_of(&run2)
    );

    // Adding -W must not wipe the cache, and must not fail the build over
    // warnings a warm build never re-emits.
    let run3 = sphinx_build(&[&s, &o, "-W", "-v"]);
    assert!(
        stderr_of(&run3).contains("Cache hits: 3"),
        "-W must not invalidate the cache, stderr: {}",
        stderr_of(&run3)
    );
    let run4 = sphinx_build(&[&s, &o, "-W"]);
    assert_eq!(
        run3.status.success(),
        run4.status.success(),
        "two identical -W runs must agree on the exit code; \
         run3: {}\nrun4: {}",
        stderr_of(&run3),
        stderr_of(&run4)
    );
    assert!(
        run3.status.success() && run4.status.success(),
        "a warm -W build over an unchanged project has no warnings to fail on, \
         stderr: {}",
        stderr_of(&run3)
    );

    // Same for -n, and dropping the flags again is likewise free.
    let run5 = sphinx_build(&[&s, &o, "-n", "-v"]);
    assert!(
        stderr_of(&run5).contains("Cache hits: 3"),
        "-n must not invalidate the cache, stderr: {}",
        stderr_of(&run5)
    );
    let run6 = sphinx_build(&[&s, &o, "-v"]);
    assert!(
        stderr_of(&run6).contains("Cache hits: 3"),
        "dropping the flags must not invalidate the cache either, stderr: {}",
        stderr_of(&run6)
    );
}

#[test]
fn stats_prints_source_file_count() {
    let result = bin()
        .arg("stats")
        .arg("--source")
        .arg(fixture("basic"))
        .output()
        .expect("binary should run");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Source files: 2"), "stats stdout: {stdout}");
}

// ---------------------------------------------------------------------------
// sphinx-build-compatible argument mode (ROADMAP M1 "CLI foundation").
//
// Parity targets below were measured against real sphinx-build 9.1.0
// (2026-08-07): -W collects all warnings and exits 1 with "(with warnings
// treated as errors)" — keep-going has been the default since Sphinx 8.1 and
// --keep-going is an accepted no-op; an unknown builder exits 2; -M html
// writes into OUTPUTDIR/html and prints "The HTML pages are in <dir>.";
// -M clean prints "Removing everything under '<dir>'..."; -q suppresses
// progress output but keeps WARNING lines; an unknown -D key warns
// "unknown config value 'x' in override, ignoring" and the build goes on.
// ---------------------------------------------------------------------------

/// sphinx-build style: `sphinx-ultra SOURCEDIR OUTPUTDIR [opts]`.
fn sphinx_build(args: &[&str]) -> Output {
    bin().args(args).output().expect("binary should run")
}

#[test]
fn sphinx_build_mode_positional() {
    let out = out_dir("sb-positional");
    let src = fixture("basic");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-b", "html"]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("index.html").is_file());
    assert!(out.join("installation.html").is_file());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("The HTML pages are in"),
        "sphinx-build prints the final location line, stdout: {stdout}"
    );
}

#[test]
fn sphinx_build_mode_default_builder_is_html() {
    let out = out_dir("sb-default-builder");
    let src = fixture("basic");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap()]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("index.html").is_file());
}

#[test]
fn sphinx_build_mode_unsupported_builder_exits_two() {
    let out = out_dir("sb-bad-builder");
    let src = fixture("basic");
    let result = sphinx_build(&["-b", "latex", src.to_str().unwrap(), out.to_str().unwrap()]);

    assert_eq!(
        result.status.code(),
        Some(2),
        "unknown builder exits 2 (sphinx-build parity), stderr: {}",
        stderr_of(&result)
    );
    assert!(
        stderr_of(&result).contains("latex"),
        "error must name the builder, stderr: {}",
        stderr_of(&result)
    );
}

#[test]
fn sphinx_build_make_mode_html_and_clean() {
    let out = out_dir("sb-make-mode");
    let src = fixture("basic");

    let result = sphinx_build(&["-M", "html", src.to_str().unwrap(), out.to_str().unwrap()]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        out.join("html/index.html").is_file(),
        "-M html writes into OUTPUTDIR/html (sphinx-build parity)"
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("The HTML pages are in"), "stdout: {stdout}");

    let clean = sphinx_build(&["-M", "clean", src.to_str().unwrap(), out.to_str().unwrap()]);
    assert!(clean.status.success(), "stderr: {}", stderr_of(&clean));
    assert!(
        !out.join("html").exists(),
        "-M clean removes everything under OUTPUTDIR"
    );
    assert!(out.exists(), "-M clean keeps OUTPUTDIR itself");
    assert!(
        String::from_utf8_lossy(&clean.stdout).contains("Removing everything under"),
        "-M clean prints sphinx-build's removal message"
    );

    // Nothing to clean: sphinx-build returns 0 silently.
    let gone = out_dir("sb-make-mode-gone");
    let noop = sphinx_build(&["-M", "clean", src.to_str().unwrap(), gone.to_str().unwrap()]);
    assert!(noop.status.success());
    assert!(
        !String::from_utf8_lossy(&noop.stdout).contains("Removing"),
        "-M clean on a missing dir is silent"
    );

    let bad = sphinx_build(&[
        "-M",
        "latexpdf",
        src.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        bad.status.code(),
        Some(2),
        "unsupported make-mode target exits 2, stderr: {}",
        stderr_of(&bad)
    );
}

#[test]
fn sphinx_build_d_override_excludes_file() {
    let out = out_dir("sb-D-exclude");
    let src = fixture("basic");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-D",
        "exclude_patterns=installation.rst",
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("index.html").is_file());
    assert!(
        !out.join("installation.html").exists(),
        "-D exclude_patterns must reach file discovery"
    );
}

#[test]
fn sphinx_build_d_unknown_key_warns_and_continues() {
    let out = out_dir("sb-D-unknown");
    let src = fixture("basic");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-D",
        "totally_unknown_key=1",
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        stderr_of(&result)
            .contains("unknown config value 'totally_unknown_key' in override, ignoring"),
        "sphinx-build warning text, stderr: {}",
        stderr_of(&result)
    );

    // Config-time warnings count toward -W, like every other warning.
    let out_w = out_dir("sb-D-unknown-W");
    let result_w = sphinx_build(&[
        src.to_str().unwrap(),
        out_w.to_str().unwrap(),
        "-D",
        "totally_unknown_key=1",
        "-W",
    ]);
    assert_eq!(
        result_w.status.code(),
        Some(1),
        "-W must see override warnings, stderr: {}",
        stderr_of(&result_w)
    );
}

/// The object-signature / py-domain config family is reachable from `-D`:
/// the booleans land in the bool branch, the ENUM key keeps whatever
/// string it is given while warning — exactly what sphinx's
/// `check_confval_types` does (probe E, task-2 brief) — and the two
/// `int | None` keys warn TOO: sphinx's `convert_overrides` keeps the raw
/// string for a None-default key and `check_confval_types` rejects its
/// type (probed, panel fix round B [18]; sphinx then crashes on the first
/// py signature, which this crate replaces with "unset").
#[test]
fn sphinx_build_d_reaches_the_object_signature_config_family() {
    let out = out_dir("sb-D-py-config");
    let src = fixture("basic");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-D",
        "add_function_parentheses=0",
        "-D",
        "toc_object_entries_show_parents=hide",
        "-D",
        "modindex_common_prefix=mypkg.",
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        !stderr_of(&result).contains("unknown config value")
            && !stderr_of(&result).contains("The config value"),
        "every key in the family must be a known, well-typed setting, stderr: {}",
        stderr_of(&result)
    );

    // The `int | None` keys: sphinx's type warning, byte-exact, and the
    // build carries on.
    let typed = out_dir("sb-D-py-config-int-none");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        typed.to_str().unwrap(),
        "-D",
        "maximum_signature_line_length=88",
        "-D",
        "python_maximum_signature_line_length=0",
    ]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains(
            "The config value `maximum_signature_line_length' has type `str'; expected \
             `NoneType' or `int'."
        ),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(
            "The config value `python_maximum_signature_line_length' has type `str'; \
             expected `NoneType' or `int'."
        ),
        "stderr: {stderr}"
    );
    let typed_w = out_dir("sb-D-py-config-int-none-W");
    let result_w = sphinx_build(&[
        src.to_str().unwrap(),
        typed_w.to_str().unwrap(),
        "-D",
        "maximum_signature_line_length=88",
        "-W",
    ]);
    assert_eq!(
        result_w.status.code(),
        Some(1),
        "-W must see the type warning, stderr: {}",
        stderr_of(&result_w)
    );

    // An out-of-ENUM value warns and the build carries on with it.
    let bad = out_dir("sb-D-py-config-enum");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        bad.to_str().unwrap(),
        "-D",
        "toc_object_entries_show_parents=bogus",
    ]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        stderr_of(&result).contains(
            "The config value `toc_object_entries_show_parents` has to be a one of \
             frozenset({'domain', 'all', 'hide'}), but `bogus` is given."
        ),
        "sphinx's ENUM rejection text, stderr: {}",
        stderr_of(&result)
    );

    // ...and, being a config-time warning, it counts toward -W.
    let bad_w = out_dir("sb-D-py-config-enum-W");
    let result_w = sphinx_build(&[
        src.to_str().unwrap(),
        bad_w.to_str().unwrap(),
        "-D",
        "toc_object_entries_show_parents=bogus",
        "-W",
    ]);
    assert_eq!(
        result_w.status.code(),
        Some(1),
        "-W must see the ENUM warning, stderr: {}",
        stderr_of(&result_w)
    );
}

#[test]
fn sphinx_build_w_exits_one_with_sphinx_message() {
    let out = out_dir("sb-W");
    let src = fixture("basic_missing_ref");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-W"]);

    assert_eq!(
        result.status.code(),
        Some(1),
        "stderr: {}",
        stderr_of(&result)
    );
    // Sphinx logs the summary on its status stream, stdout.
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("with warnings treated as errors"),
        "sphinx 9.1 -W message, stdout: {}",
        String::from_utf8_lossy(&result.stdout)
    );

    // keep-going is the default since Sphinx 8.1; the flag is an accepted no-op.
    let out2 = out_dir("sb-W-keep-going");
    let result2 = sphinx_build(&[
        src.to_str().unwrap(),
        out2.to_str().unwrap(),
        "-W",
        "--keep-going",
    ]);
    assert_eq!(result2.status.code(), Some(1));
}

#[test]
fn sphinx_build_confdir_flag() {
    let sandbox = out_dir("sb-confdir");
    let confdir = sandbox.join("conf");
    std::fs::create_dir_all(&confdir).unwrap();
    std::fs::write(
        confdir.join("conf.py"),
        "project = 'Confdir Probe'\nexclude_patterns = ['installation.rst']\n",
    )
    .unwrap();
    let out = sandbox.join("out");
    let src = fixture("basic");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-c",
        confdir.to_str().unwrap(),
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        !out.join("installation.html").exists(),
        "-c confdir conf.py must be honored"
    );
}

#[test]
fn sphinx_build_doctreedir_flag() {
    let sandbox = out_dir("sb-doctreedir");
    let doctrees = sandbox.join("trees");
    let out = sandbox.join("out");
    let src = fixture("basic");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-d",
        doctrees.to_str().unwrap(),
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        doctrees.join(".config-fingerprint").is_file(),
        "-d must relocate the cache dir"
    );
    assert!(
        !out.join(".sphinx-ultra-cache").exists(),
        "default cache location must not be used when -d is given"
    );
}

#[test]
fn sphinx_build_incremental_by_default_and_fresh_env() {
    let sandbox = out_dir("sb-incremental");
    let out = sandbox.join("out");
    let src = fixture("basic");
    let s = src.to_str().unwrap();

    let run1 = sphinx_build(&[s, out.to_str().unwrap()]);
    assert!(run1.status.success(), "stderr: {}", stderr_of(&run1));

    // The build's progress log, `Cache hits` among it, prints with `-v`:
    // without it sphinx-build mode's stderr is the warning stream alone.
    let run2 = sphinx_build(&[s, out.to_str().unwrap(), "-v"]);
    assert!(
        stderr_of(&run2).contains("Cache hits: 2"),
        "compat mode is incremental by default (sphinx-build parity), stderr: {}",
        stderr_of(&run2)
    );

    let run3 = sphinx_build(&[s, out.to_str().unwrap(), "-E", "-v"]);
    assert!(
        stderr_of(&run3).contains("Cache hits: 0"),
        "-E discards the saved environment, stderr: {}",
        stderr_of(&run3)
    );

    let run4 = sphinx_build(&[s, out.to_str().unwrap(), "-a", "-v"]);
    assert!(
        stderr_of(&run4).contains("Cache hits: 0"),
        "-a rewrites all files, stderr: {}",
        stderr_of(&run4)
    );
}

#[test]
fn sphinx_build_quiet_keeps_warnings() {
    let out = out_dir("sb-quiet");
    let src = fixture("basic_missing_ref");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-q"]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("WARNING: toctree contains reference"),
        "-q keeps warnings, stderr: {stderr}"
    );
    assert!(
        !stderr.contains("Build completed successfully"),
        "-q suppresses progress output, stderr: {stderr}"
    );
}

#[test]
fn sphinx_build_j_auto_and_a_flag_accepted() {
    let out = out_dir("sb-j-auto");
    let src = fixture("basic");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-j",
        "auto",
        "-A",
        "release_banner=1",
        "-t",
        "mytag",
        "-T",
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("index.html").is_file());
}

#[test]
fn sphinx_build_filenames_accepted_with_warning() {
    let out = out_dir("sb-filenames");
    let src = fixture("basic");
    let file = src.join("index.rst");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        file.to_str().unwrap(),
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        stderr_of(&result).contains("not supported yet"),
        "specific-file builds are honestly reported as unsupported, stderr: {}",
        stderr_of(&result)
    );
}

#[test]
fn source_dir_named_build_works_via_dot_slash() {
    let sandbox = out_dir("sb-dir-named-build");
    let src = sandbox.join("build");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("index.rst"), "Title\n=====\n\nBody.\n").unwrap();
    std::fs::write(src.join("conf.py"), "project = 'Named Build'\n").unwrap();
    let out = sandbox.join("out");

    let result = bin()
        .current_dir(&sandbox)
        .arg("./build")
        .arg(out.to_str().unwrap())
        .output()
        .expect("binary should run");

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(out.join("index.html").is_file());
}

#[test]
fn preset_rust_log_is_respected() {
    let out = out_dir("rust-log-respected");
    let result = bin()
        .env("RUST_LOG", "error")
        .arg("build")
        .arg("--source")
        .arg(fixture("basic_missing_ref"))
        .arg("--output")
        .arg(&out)
        .output()
        .expect("binary should run");

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        !stderr.contains("WARNING") && !stderr.contains("INFO"),
        "RUST_LOG=error must silence warn/info logging (pre-set RUST_LOG wins), stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Validation systems wired into the build (ROADMAP M1: directive/role
// validation on by default with false-positive heuristics fixed or demoted;
// cross-reference validation behind -n/nitpicky).
// ---------------------------------------------------------------------------

/// Write an inline source tree and return its dir. A minimal conf.py is
/// added unless the caller provides one (sphinx-build mode requires it).
fn temp_source(test_name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = out_dir(&format!("{test_name}-src"));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    if !files.iter().any(|(name, _)| *name == "conf.py") {
        std::fs::write(dir.join("conf.py"), "project = 'Temp'\n").unwrap();
    }
    dir
}

#[test]
fn directive_validation_reports_real_problems() {
    let src = temp_source(
        "dv-problems",
        &[(
            "index.rst",
            "Title\n=====\n\n.. note::\n\n.. toctree::\n   :bogus:\n\n   self\n",
        )],
    );
    let out = out_dir("dv-problems");
    let result = build(&src, &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("index.rst:4: WARNING: Note directive requires content"),
        "empty note must be flagged with file:line, stderr: {stderr}"
    );
    assert!(
        stderr.contains("Unknown option 'bogus' for toctree directive"),
        "bogus toctree option must be flagged, stderr: {stderr}"
    );

    // -W promotes validation warnings to a failing exit
    let out_w = out_dir("dv-problems-W");
    let result_w = build(&src, &out_w, &["-W"]);
    assert_eq!(
        result_w.status.code(),
        Some(1),
        "-W must see validation warnings, stderr: {}",
        stderr_of(&result_w)
    );
}

#[test]
fn directive_validation_silent_on_valid_sphinx() {
    let src = temp_source(
        "dv-valid",
        &[(
            "index.rst",
            "Title\n=====\n\n.. note:: One-line inline note.\n\n.. code-block::\n\n   no language, still fine\n\nSee :ref:`A Label With Spaces` in prose.\n\n.. _a label with spaces:\n\nSection\n-------\n\nBody.\n",
        )],
    );
    let out = out_dir("dv-valid");
    let result = build(&src, &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    for needle in [
        "Note directive",
        "No language",
        "cannot contain spaces",
        "lowercase",
    ] {
        assert!(
            !stderr.contains(needle),
            "valid Sphinx must not trigger '{needle}', stderr: {stderr}"
        );
    }
}

/// Sphinx has no counterpart to M1's directive and role validators, so
/// sphinx-build mode never runs them: an empty `note` gets no validator
/// record, on stderr or in the `-w` file, without any `-D`.
#[test]
fn sphinx_build_mode_runs_no_validation() {
    let src = temp_source(
        "sb-no-validation",
        &[("index.rst", "Title\n=====\n\n.. note::\n")],
    );
    let out = out_dir("sb-no-validation");
    let warnings = out_dir("sb-no-validation-log").join("warnings.txt");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-w",
        warnings.to_str().unwrap(),
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let written = std::fs::read_to_string(&warnings).unwrap();
    for (sink, text) in [("stderr", stderr_of(&result)), ("-w file", written)] {
        assert!(
            !text.contains("Note directive requires content"),
            "{sink} holds a validator record: {text}"
        );
    }
}

/// `|today|` formats the build date, which Sphinx's `format_date` takes
/// from `SOURCE_DATE_EPOCH` when it is set, in UTC
/// (`SP/sphinx/util/i18n.py:268-282`): the binary reads it from its own
/// environment, once per build, and formats it with conf.py's
/// `today_fmt`, which `-D` overrides; `-D today=` replaces the date with
/// its text. The pages are 5b's, so the substituted text is read from the
/// search index, which is fed from the read doctree: one term per run.
#[test]
fn today_follows_source_date_epoch() {
    let src = temp_source(
        "sb-today",
        &[
            ("conf.py", "project = 'Temp'\ntoday_fmt = 'on%Y%m%d'\n"),
            ("index.rst", "Title\n=====\n\nBuilt |today|.\n"),
        ],
    );
    // 1700000000 is 2023-11-14T22:13:20Z, day 318 of the year. The search
    // index drops all-digit words, so each format leads with letters.
    for (name, extra, term) in [
        ("conf", &[][..], "on20231114"),
        ("fmt", &["-D", "today_fmt=day%j%y"][..], "day31823"),
        ("text", &["-D", "today=x19990101"][..], "x19990101"),
    ] {
        let out = out_dir(&format!("sb-today-{name}"));
        let mut args = vec![src.to_str().unwrap(), out.to_str().unwrap(), "-q"];
        args.extend_from_slice(extra);
        let result = bin()
            .args(&args)
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .output()
            .expect("binary should run");
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        let terms = search_value(&out)["terms"].clone();
        assert!(
            terms.get(term).is_some(),
            "{name}: no term {term:?} in {terms}"
        );
    }
}

/// The native subcommand keeps the validators, and `validate_directives`
/// in its configuration file still switches them off.
#[test]
fn native_validation_off_switch() {
    let src = temp_source(
        "native-dv-off",
        &[("index.rst", "Title\n=====\n\n.. note::\n")],
    );
    let conf = out_dir("native-dv-off-conf").join("off.yaml");
    std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
    std::fs::write(&conf, "validate_directives: false\n").unwrap();

    let out = out_dir("native-dv-off");
    let off = bin()
        .arg("--config")
        .arg(&conf)
        .arg("build")
        .arg("--source")
        .arg(&src)
        .arg("--output")
        .arg(&out)
        .output()
        .expect("binary should run");
    assert!(off.status.success(), "stderr: {}", stderr_of(&off));
    assert!(
        !stderr_of(&off).contains("Note directive requires content"),
        "validate_directives: false must disable the pass, stderr: {}",
        stderr_of(&off)
    );

    let out_on = out_dir("native-dv-on");
    let on = build(&src, &out_on, &[]);
    assert!(on.status.success(), "stderr: {}", stderr_of(&on));
    assert!(
        stderr_of(&on).contains("index.rst:4: WARNING: Note directive requires content"),
        "the same project without the switch is validated, stderr: {}",
        stderr_of(&on)
    );
}

/// sphinx-build mode prints each record bare, the same bytes the `-w`
/// file gets (Sphinx's `TeeStripANSI`); under `-q` nothing else reaches
/// stderr — the closing summary is on the status stream, stdout.
#[test]
fn sphinx_build_mode_stderr_equals_warning_file() {
    let src = temp_source(
        "sb-stderr-is-w",
        &[("index.rst", "Title\n=====\n\n.. toctree::\n\n   missing\n")],
    );
    let out = out_dir("sb-stderr-is-w");
    let warnings = out_dir("sb-stderr-is-w-log").join("warnings.txt");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-w",
        warnings.to_str().unwrap(),
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    // Strict UTF-8 on both sides, so the string comparison is byte-exact.
    let written = String::from_utf8(std::fs::read(&warnings).unwrap()).unwrap();
    let source = sphinx_ultra::utils::canonicalize_simplified(&src).unwrap();
    assert_eq!(
        written,
        format!(
            "{}:4: WARNING: toctree contains reference to nonexisting document 'missing' \
             [toc.not_readable]\n",
            source.join("index.rst").display()
        )
    );
    assert_eq!(String::from_utf8(result.stderr).unwrap(), written);
}

/// Decision 8 holds without `-q` too: Sphinx writes its status to stdout,
/// so its stderr is the warning stream alone, byte for byte the `-w` file,
/// and empty for a project with no warnings. `-q` empties stdout even with
/// extensions configured (Sphinx sets them up silently). The build's own
/// progress log needs `-v`.
#[test]
fn sphinx_build_mode_stderr_is_the_warning_stream_without_q() {
    let src = temp_source(
        "sb-stderr-noq",
        &[("index.rst", "Title\n=====\n\n.. toctree::\n\n   missing\n")],
    );
    let out = out_dir("sb-stderr-noq");
    let warnings = out_dir("sb-stderr-noq-log").join("warnings.txt");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-w",
        warnings.to_str().unwrap(),
    ]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let written = String::from_utf8(std::fs::read(&warnings).unwrap()).unwrap();
    assert!(written.contains("[toc.not_readable]"), "{written}");
    assert_eq!(String::from_utf8(result.stderr).unwrap(), written);

    let clean = temp_source(
        "sb-stderr-clean",
        &[
            (
                "conf.py",
                "extensions = ['sphinx.ext.autodoc', 'sphinx.ext.intersphinx']\n",
            ),
            ("index.rst", "Title\n=====\n\nBody.\n"),
        ],
    );
    let out = out_dir("sb-stderr-clean");
    let result = sphinx_build(&[clean.to_str().unwrap(), out.to_str().unwrap()]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(stderr_of(&result), "");
    let out = out_dir("sb-stderr-clean-q");
    let result = sphinx_build(&[clean.to_str().unwrap(), out.to_str().unwrap(), "-q"]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "");
    assert_eq!(stderr_of(&result), "");
}

/// A logger warning raised inside an included file prints the path Sphinx's
/// log translator makes of its location: a node location is the included
/// file made absolute, a tuple location goes through `doc2path`, which
/// appends `.rst` again. Bytes probed on Sphinx 9.1.0 (`-q -w`, cwd = the
/// source directory) for this very project.
#[test]
fn included_file_logger_warnings_spell_sphinx_paths() {
    let src = temp_source(
        "sb-included-logger",
        &[
            ("conf.py", "project = 'p'\n"),
            ("index.rst", "Index\n=====\n\n.. include:: part.inc\n"),
            (
                "part.inc",
                "Part para.\n\n.. literalinclude:: example.py\n   :lines: 1,99\n\n\
                 .. option:: =bad\n",
            ),
            ("example.py", "a = 1\nb = 2\n"),
        ],
    );
    let out = out_dir("sb-included-logger");
    let warnings = out_dir("sb-included-logger-log").join("warnings.txt");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-w",
        warnings.to_str().unwrap(),
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let written = String::from_utf8(std::fs::read(&warnings).unwrap()).unwrap();
    let source = sphinx_ultra::utils::canonicalize_simplified(&src).unwrap();
    assert_eq!(
        written,
        format!(
            "{}:3: WARNING: line number spec is out of range(1-2): '1,99'\n\
             {}:6: WARNING: Malformed option description '=bad', should look like \"opt\", \
             \"-opt args\", \"--opt args\", \"/opt args\" or \"+opt args\"\n",
            source.join("part.inc.rst").display(),
            source.join("part.inc").display()
        )
    );
}

/// `suppress_warnings` drops a record before it is printed or counted,
/// and `show_warning_types = False` drops the ` [type.subtype]` suffix
/// (`sphinx/util/logging.py:415-450`, `:531-551`).
#[test]
fn suppressed_warnings_neither_print_nor_count() {
    let project = |name: &str, conf: &str| {
        temp_source(
            name,
            &[
                ("conf.py", conf),
                ("index.rst", "Index\n=====\n\nBody.\n"),
                ("orphan.rst", "Orphan\n======\n\nBody.\n"),
            ],
        )
    };
    let run = |src: &Path, name: &str| {
        let out = out_dir(name);
        let warnings = out_dir(&format!("{name}-log")).join("warnings.txt");
        let result = sphinx_build(&[
            src.to_str().unwrap(),
            out.to_str().unwrap(),
            "-q",
            "-W",
            "-w",
            warnings.to_str().unwrap(),
        ]);
        let written = std::fs::read_to_string(&warnings).unwrap();
        (result, written)
    };

    // The control: the orphan is the project's one record, and fails -W.
    let plain = project("suppress-control", "project = 'p'\n");
    let (result, written) = run(&plain, "suppress-control");
    let source = sphinx_ultra::utils::canonicalize_simplified(&plain).unwrap();
    let orphan = source.join("orphan.rst");
    assert_eq!(
        written,
        format!(
            "{}: WARNING: document isn't included in any toctree [toc.not_included]\n",
            orphan.display()
        )
    );
    assert_eq!(
        result.status.code(),
        Some(1),
        "stderr: {}",
        stderr_of(&result)
    );

    let suppressed = project(
        "suppress-on",
        "project = 'p'\nsuppress_warnings = ['toc.not_included']\n",
    );
    let (result, written) = run(&suppressed, "suppress-on");
    assert_eq!(
        result.status.code(),
        Some(0),
        "a suppressed record does not count toward -W, stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(written, "", "a suppressed record is not written");
    assert_eq!(stderr_of(&result), "", "a suppressed record is not printed");

    let untyped = project(
        "suppress-untyped",
        "project = 'p'\nshow_warning_types = False\n",
    );
    let (result, written) = run(&untyped, "suppress-untyped");
    let source = sphinx_ultra::utils::canonicalize_simplified(&untyped).unwrap();
    let line = format!(
        "{}: WARNING: document isn't included in any toctree\n",
        source.join("orphan.rst").display()
    );
    assert_eq!(written, line);
    assert_eq!(stderr_of(&result), line);
    assert_eq!(result.status.code(), Some(1), "still a record under -W");
}

// ---------------------------------------------------------------------------
// docutils reporter records (wave 5a T8): every message the reporter creates
// at level WARNING or above prints as a `[docutils]` record, with the text it
// had when docutils created it, ordered by its creation among the document's
// parse records. Every pinned stream below is Sphinx 9.1.0's own `-q -w`
// output for the same project (cwd = the source directory), with the source
// directory spelled `<SRCDIR>`.
// ---------------------------------------------------------------------------

/// A `-q -w` sphinx-build run of `src` into `out`, with `extra` flags: the
/// run, and its `-w` file with the source directory spelled `<SRCDIR>` (and
/// the separator after it `/`, so one literal holds on every platform).
fn warning_stream(src: &Path, out: &Path, extra: &[&str]) -> (Output, String) {
    let log = out.with_extension("w");
    let _ = std::fs::remove_file(&log);
    let mut args = vec![
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-w",
        log.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let result = sphinx_build(&args);
    (result, srcdir_spelled(&std::fs::read(&log).unwrap(), src))
}

/// `bytes` as strict UTF-8 with the canonical `src` (and the separator
/// after it) spelled `<SRCDIR>/`.
fn srcdir_spelled(bytes: &[u8], src: &Path) -> String {
    let root = sphinx_ultra::utils::canonicalize_simplified(src).unwrap();
    String::from_utf8(bytes.to_vec()).unwrap().replace(
        &format!("{}{}", root.display(), std::path::MAIN_SEPARATOR),
        "<SRCDIR>/",
    )
}

/// Rewrites `path` with its own bytes, so its mtime moves past the build
/// that read it.
fn touch(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// The read records of `b.rst` in [`touched_document_reemits_its_records`].
const TOUCHED_B_RECORDS: &str = "\
<SRCDIR>/b.rst:6: ERROR: Unexpected indentation. [docutils]
<SRCDIR>/b.rst:8: WARNING: Inline emphasis start-string without end-string. [docutils]
";

/// The resolve records of `a.rst` and `d.rst`, in that docname order, in
/// [`touched_document_reemits_its_records`].
const UNDEFINED_LABEL_RECORDS: &str = "\
<SRCDIR>/a.rst:4: WARNING: undefined label: 'elsewhere' [ref.ref]
<SRCDIR>/d.rst:4: WARNING: undefined label: 'nowhere' [ref.ref]
";

/// Review Focus 1: a warm build re-reads the touched document and prints
/// its read records — all of them, in the cold build's order — and none of
/// the untouched documents' read records. Resolve records are recomputed
/// for every document on every build, so `a`'s and `d`'s `undefined
/// label`s follow, in sorted docname order: decision 6's documented
/// divergence from `-b html`, which writes, and so resolves, only the
/// outdated `b` (probed: Sphinx 9.1.0's warm `-b html` prints `b`'s two
/// records alone, its `-b dummy` prints them and then `a`'s and `d`'s, as
/// here).
#[test]
fn touched_document_reemits_its_records() {
    let src = temp_source(
        "t8-touch",
        &[
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   d\n\nSee *this.\n",
            ),
            ("a.rst", "A title\n=====\n\nSee :ref:`elsewhere`.\n"),
            ("b.rst", "B\n=\n\nPara\nmore\n   indented.\n\nAnd *more.\n"),
            ("d.rst", "D\n=\n\nSee :ref:`nowhere`.\n"),
        ],
    );
    let out = out_dir("t8-touch");

    let (cold, written) = warning_stream(&src, &out, &[]);
    assert!(cold.status.success(), "stderr: {}", stderr_of(&cold));
    assert_eq!(
        written,
        format!(
            "<SRCDIR>/a.rst:2: WARNING: Title underline too short.\n\nA title\n===== \
             [docutils]\n{TOUCHED_B_RECORDS}<SRCDIR>/index.rst:10: WARNING: Inline emphasis \
             start-string without end-string. [docutils]\n{UNDEFINED_LABEL_RECORDS}"
        )
    );

    touch(&src.join("b.rst"));
    let (warm, written) = warning_stream(&src, &out, &[]);
    assert!(warm.status.success(), "stderr: {}", stderr_of(&warm));
    assert_eq!(
        written,
        format!("{TOUCHED_B_RECORDS}{UNDEFINED_LABEL_RECORDS}")
    );
}

/// Review Focus 2: `-j 4` reads the documents in parallel — `e` is long,
/// so its read finishes after the others' — and still prints the stream of
/// `-j 1`: each document's records, in sorted docname order, each
/// document's parse records first, then its transforms' and collectors'
/// in transform order: `a`'s parse ERROR, `b`'s AnonymousHyperlinks (440)
/// mismatch, which has no line, `c`'s unreferenced footnote (622) and `d`'s
/// unreadable image (880), each after its document's later parse record. (Sphinx's own `-j 4` prints its
/// read chunks in completion order; the pinned stream is its `-j 1` one,
/// probed on Sphinx 9.1.0.)
#[test]
fn parallel_read_keeps_warning_order() {
    let long = (0..4000)
        .map(|n| format!("Paragraph {n} of filler text.\n\n"))
        .collect::<String>();
    let index = format!(
        "Index\n=====\n\n.. toctree::\n\n{}\nSee *this.\n",
        ('a'..='h').map(|d| format!("   {d}\n")).collect::<String>()
    );
    let e = format!("E\n=\n\n{long}Tail *e.\n");
    let src = temp_source(
        "t8-parallel",
        &[
            ("index.rst", &index),
            ("a.rst", "A\n=\n\nText\nmore\n   indented.\n\nPara *a.\n"),
            (
                "b.rst",
                "B\n=\n\nPara *b.\n\nSee `one`__ and `two`__.\n\n__ https://example.com\n",
            ),
            (
                "c.rst",
                "C\n=\n\n.. [1] An unreferenced footnote.\n\nPara *c.\n",
            ),
            ("d.rst", "D\n=\n\n.. image:: missing.png\n\nPara *d.\n"),
            ("e.rst", &e),
            ("f.rst", "F\n=\n\nClean.\n"),
            ("g.rst", "G\n=\n\nPara *g.\n"),
            ("h.rst", "H\n=\n\nText\nmore\n   indented.\n"),
        ],
    );
    let expected = "\
<SRCDIR>/a.rst:6: ERROR: Unexpected indentation. [docutils]
<SRCDIR>/a.rst:8: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/b.rst:4: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/b.rst:: ERROR: Anonymous hyperlink mismatch: 2 references but 1 targets.
See \"backrefs\" attribute for IDs. [docutils]
<SRCDIR>/c.rst:6: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/c.rst:4: WARNING: Footnote [1] is not referenced. [ref.footnote]
<SRCDIR>/d.rst:6: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/d.rst:4: WARNING: image file not readable: missing.png [image.not_readable]
<SRCDIR>/e.rst:8004: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/g.rst:4: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/h.rst:6: ERROR: Unexpected indentation. [docutils]
<SRCDIR>/index.rst:15: WARNING: Inline emphasis start-string without end-string. [docutils]
";

    for jobs in ["4", "1"] {
        let (result, written) = warning_stream(
            &src,
            &out_dir(&format!("t8-parallel-j{jobs}")),
            &["-j", jobs],
        );
        assert!(result.status.success(), "stderr: {}", stderr_of(&result));
        assert_eq!(written, expected, "-j {jobs}");
    }
}

/// Review Focus 4: a CRLF project prints the reporter records of its LF
/// twin byte for byte — line numbers, and a record's literal block, which
/// holds no carriage return.
#[test]
fn crlf_reporter_lines_match_lf() {
    let index = "Title\n====\n\n.. toctree::\n\n   part\n\nPara\nmore\n   indented.\n\n\
                 | line one\n| line *two\n\n.. note:: x\n   :name: t\n\n.. _t:\n\nEnd *star.\n";
    let part = "Part title\n=====\n\nText *x.\n";
    let expected = "\
<SRCDIR>/index.rst:2: WARNING: Title underline too short.

Title
==== [docutils]
<SRCDIR>/index.rst:10: ERROR: Unexpected indentation. [docutils]
<SRCDIR>/index.rst:13: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/index.rst:18: WARNING: Duplicate explicit target name: \"t\". [docutils]
<SRCDIR>/index.rst:20: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/part.rst:2: WARNING: Title underline too short.

Part title
===== [docutils]
<SRCDIR>/part.rst:4: WARNING: Inline emphasis start-string without end-string. [docutils]
";
    let crlf = |text: &str| text.replace('\n', "\r\n");
    let (crlf_index, crlf_part) = (crlf(index), crlf(part));
    for (name, index, part) in [
        ("t8-lf", index, part),
        ("t8-crlf", crlf_index.as_str(), crlf_part.as_str()),
    ] {
        let src = temp_source(name, &[("index.rst", index), ("part.rst", part)]);
        let (result, written) = warning_stream(&src, &out_dir(name), &[]);
        assert!(result.status.success(), "stderr: {}", stderr_of(&result));
        assert_eq!(written, expected, "{name}");
    }
}

/// Review Focus 3: Sphinx counts docutils' records in its warning total, so
/// `-W` fails a build whose only records are a reporter's, after printing
/// them; `suppress_warnings = ['docutils']` drops the records before they
/// are printed or counted, and the build passes.
#[test]
fn fail_on_warning_counts_reporter_records() {
    let records = "<SRCDIR>/b.rst:2: WARNING: Title underline too short.\n\nB title\n===== \
                   [docutils]\n<SRCDIR>/index.rst:2: WARNING: Title underline too short.\n\n\
                   Title\n==== [docutils]\n";
    let index = "Title\n====\n\n.. toctree::\n\n   b\n";
    let b = "B title\n=====\n\nBody.\n";
    let src = temp_source("t8-fail-on-warning", &[("index.rst", index), ("b.rst", b)]);
    let (result, written) = warning_stream(&src, &out_dir("t8-fail-on-warning"), &["-W"]);
    assert_eq!(written, records);
    assert_eq!(srcdir_spelled(&result.stderr, &src), records);
    assert_eq!(
        result.status.code(),
        Some(1),
        "stderr: {}",
        stderr_of(&result)
    );

    let suppressed = temp_source(
        "t8-fail-on-warning-suppressed",
        &[
            (
                "conf.py",
                "project = 'p'\nsuppress_warnings = ['docutils']\n",
            ),
            ("index.rst", index),
            ("b.rst", b),
        ],
    );
    let (result, written) = warning_stream(
        &suppressed,
        &out_dir("t8-fail-on-warning-suppressed"),
        &["-W"],
    );
    assert_eq!(written, "");
    assert_eq!(stderr_of(&result), "");
    assert_eq!(
        result.status.code(),
        Some(0),
        "stderr: {}",
        stderr_of(&result)
    );
}

/// The citation domain checks its citations once every document has been
/// read (`CitationDomain.check_consistency`,
/// `SP/sphinx/domains/citation.py:88-97`): a citation no document refers
/// to warns after every read-phase record, and after the toctree's own
/// consistency check, in the order the citations were noted. The location
/// is `(docname, lineno)`: the document's path and the citation's line,
/// which for a citation an included file holds is its line in that file.
/// Probed on Sphinx 9.1.0 (this project's warning stream).
#[test]
fn unreferenced_citation_warns_in_consistency() {
    let src = temp_source(
        "t13b-unreferenced",
        &[
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   d\n",
            ),
            (
                "a.rst",
                "A\n=\n\n.. [Z] Unreferenced.\n\n.. [Y] Referenced from b.\n",
            ),
            ("b.rst", "B\n=\n\nSee [Y]_ and *this.\n"),
            ("c.rst", "C\n=\n\nOrphan.\n"),
            ("d.rst", "D\n=\n\nIntro.\n\n.. include:: part.inc\n"),
            ("part.inc", "Part.\n\n.. [INC] Included, unreferenced.\n"),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir("t13b-unreferenced"), &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "\
<SRCDIR>/b.rst:4: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/c.rst: WARNING: document isn't included in any toctree [toc.not_included]
<SRCDIR>/a.rst:4: WARNING: Citation [Z] is not referenced. [ref.citation]
<SRCDIR>/d.rst:3: WARNING: Citation [INC] is not referenced. [ref.citation]
"
    );
}

/// A citation reference becomes a `pending_xref` into the citation domain
/// (619-017), which the write phase resolves against every document's
/// citations (`CitationDomain.resolve_xref`); one that names no citation
/// warns `citation not found` (`dangling_warnings`,
/// `SP/sphinx/domains/citation.py:37-39`), `[ref.ref]`, after the
/// consistency records. It is located where `copy_source_info` put it, the
/// source and the line of the reference's nearest ancestors that have one:
/// the paragraph's first line for both of `b`'s, the rubric's for the one
/// a rubric holds, the end of the rubric's directive block, and, for a
/// rubric in a list item, which has line 0, the item's line. Probed on
/// Sphinx 9.1.0 (this project's warning stream).
#[test]
fn citation_not_found_warns_at_resolve() {
    let src = temp_source(
        "t13b-not-found",
        &[
            ("index.rst", "Index\n=====\n\n.. toctree::\n\n   a\n   b\n"),
            ("a.rst", "A\n=\n\n.. [Z] Unreferenced.\n"),
            (
                "b.rst",
                "B\n=\n\nSee [W]_ here,\nand [Q]_ there.\n\n.. rubric:: Rubric [R]_\n\nText *b.\n\n\
                 - item\n\n  .. rubric:: Listed [L]_\n",
            ),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir("t13b-not-found"), &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "\
<SRCDIR>/b.rst:9: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/a.rst:4: WARNING: Citation [Z] is not referenced. [ref.citation]
<SRCDIR>/b.rst:4: WARNING: citation not found: W [ref.ref]
<SRCDIR>/b.rst:4: WARNING: citation not found: Q [ref.ref]
<SRCDIR>/b.rst:8: WARNING: citation not found: R [ref.ref]
<SRCDIR>/b.rst:11: WARNING: citation not found: L [ref.ref]
"
    );
}

/// A `:doc:` whose target is a URL names no document, and Sphinx says so
/// like for any other missing document (`dangling_warnings`,
/// `SP/sphinx/domains/std/__init__.py:790-796`): `unknown document`, with
/// the target as written (decision 17 ends the M1 carve-out that kept such
/// targets silent). Probed on Sphinx 9.1.0 (this project's warning
/// stream).
#[test]
fn doc_role_url_target_warns_unknown_document() {
    let src = temp_source(
        "t13b-doc-url",
        &[(
            "index.rst",
            "Title\n=====\n\nCall x.\nSee :doc:`https://example.com/page` and :doc:`missing`.\n",
        )],
    );
    let (result, written) = warning_stream(&src, &out_dir("t13b-doc-url"), &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "\
<SRCDIR>/index.rst:4: WARNING: unknown document: 'https://example.com/page' [ref.doc]
<SRCDIR>/index.rst:4: WARNING: unknown document: 'missing' [ref.doc]
"
    );
}

/// Review Focus 3, for a transform's record: Sphinx counts docutils'
/// records in its warning total whichever stage made them, so `-W` fails a
/// build whose one record is AnonymousHyperlinks' (440) mismatch, after
/// printing it, to stderr and to `-w` alike. Probed on Sphinx 9.1.0 (the
/// record).
#[test]
fn fail_on_warning_counts_transform_records() {
    let record = "<SRCDIR>/index.rst:: ERROR: Anonymous hyperlink mismatch: 2 references but 1 \
                  targets.\nSee \"backrefs\" attribute for IDs. [docutils]\n";
    let src = temp_source(
        "t13b-fail-on-transform-record",
        &[(
            "index.rst",
            "Title\n=====\n\nSee `one`__ and `two`__.\n\n__ https://example.com\n",
        )],
    );
    let (result, written) =
        warning_stream(&src, &out_dir("t13b-fail-on-transform-record"), &["-W"]);
    assert_eq!(written, record);
    assert_eq!(srcdir_spelled(&result.stderr, &src), record);
    assert_eq!(
        result.status.code(),
        Some(1),
        "stderr: {}",
        stderr_of(&result)
    );
}

/// A reporter message raised inside an included file prints the included
/// file's source as `render_records` spells it, srcdir-relative, which is
/// docutils' own cwd-relative spelling when the cwd is the source directory
/// ([TW §5.2]: `circ_b.rst:3`), as here. The same file read as a document
/// of its own prints its absolute path.
#[test]
fn included_file_reporter_source_is_cwd_relative() {
    let src = temp_source(
        "t8-include",
        &[
            (
                "index.rst",
                "Index\n=====\n\nIntro *star.\n\n.. include:: part.rst\n\nAfter *star.\n",
            ),
            (
                "part.rst",
                "Part para.\n\nPart *star here.\n\nMore\ntext\n   indented.\n",
            ),
        ],
    );
    let out = out_dir("t8-include");
    let log = out.with_extension("w");
    let result = bin()
        .current_dir(&src)
        .args([
            ".",
            out.to_str().unwrap(),
            "-q",
            "-w",
            log.to_str().unwrap(),
        ])
        .output()
        .expect("binary should run");
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let emphasis = "WARNING: Inline emphasis start-string without end-string. [docutils]";
    let indentation = "ERROR: Unexpected indentation. [docutils]";
    assert_eq!(
        srcdir_spelled(&std::fs::read(&log).unwrap(), &src),
        format!(
            "<SRCDIR>/index.rst:4: {emphasis}\npart.rst:3: {emphasis}\npart.rst:7: {indentation}\n\
             <SRCDIR>/index.rst:8: {emphasis}\n<SRCDIR>/part.rst:3: {emphasis}\n\
             <SRCDIR>/part.rst:7: {indentation}\n"
        )
    );
}

/// A document's parse records print in the order the parse created them,
/// whichever channel logged them: a logger warning (the malformed option),
/// toctree warnings and reporter records interleave by their creation
/// sequence, not grouped by channel.
#[test]
fn parse_records_print_in_creation_order() {
    let src = temp_source(
        "t8-interleave",
        &[(
            "index.rst",
            "Index\n=====\n\n.. option:: =bad\n\n.. toctree::\n\n   missing\n\nSee *this.\n\n\
             .. toctree::\n\n   gone\n\n:unknownrole:`x`\n",
        )],
    );
    let (result, written) = warning_stream(&src, &out_dir("t8-interleave"), &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "<SRCDIR>/index.rst:4: WARNING: Malformed option description '=bad', should look like \
         \"opt\", \"-opt args\", \"--opt args\", \"/opt args\" or \"+opt args\"\n\
         <SRCDIR>/index.rst:6: WARNING: toctree contains reference to nonexisting document \
         'missing' [toc.not_readable]\n\
         <SRCDIR>/index.rst:10: WARNING: Inline emphasis start-string without end-string. \
         [docutils]\n\
         <SRCDIR>/index.rst:12: WARNING: toctree contains reference to nonexisting document \
         'gone' [toc.not_readable]\n\
         <SRCDIR>/index.rst:16: ERROR: Unknown interpreted text role \"unknownrole\". \
         [docutils]\n"
    );
}

/// sphinx-build logs its closing summary on the status stream, stdout,
/// which `-q` silences (`sphinx/application.py:452-481`), never on the
/// warning stream: the summary is not a record. Bytes probed on Sphinx
/// 9.1.0: `build succeeded.` or `build succeeded, N warning(s).`, then a
/// blank line and the final location; under `-W` with a record,
/// `build finished with problems, …` and no final location. The final
/// location names the output directory as it was given, where Sphinx
/// names it relative to the working directory (`relpath(self.outdir)`,
/// `:483-490`): the absolute path the test passes prints as given here and
/// as `../…` there, a stdout-only divergence.
#[test]
fn closing_summary_goes_to_the_status_stream() {
    let run = |name: &str, body: &str, extra: &[&str]| {
        let src = temp_source(name, &[("index.rst", body)]);
        let out = out_dir(name);
        let mut args = vec![src.to_str().unwrap(), out.to_str().unwrap()];
        args.extend_from_slice(extra);
        (sphinx_build(&args), out)
    };
    let stdout = |result: &Output| String::from_utf8(result.stdout.clone()).unwrap();

    let (result, out) = run("t8-summary-clean", "Title\n=====\n\nBody.\n", &[]);
    assert_eq!(
        stdout(&result),
        format!(
            "build succeeded.\n\nThe HTML pages are in {}.\n",
            out.display()
        )
    );

    let (result, out) = run("t8-summary-one", "Title\n====\n\nBody.\n", &[]);
    assert_eq!(
        stdout(&result),
        format!(
            "build succeeded, 1 warning.\n\nThe HTML pages are in {}.\n",
            out.display()
        )
    );
    assert!(
        !stderr_of(&result).contains("build succeeded"),
        "stderr: {}",
        stderr_of(&result)
    );

    let (result, _) = run("t8-summary-quiet", "Title\n====\n\nBody.\n", &["-q"]);
    assert_eq!(stdout(&result), "");

    let (result, _) = run("t8-summary-w", "Title\n====\n\nBody.\n", &["-W"]);
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(
        stdout(&result),
        "build finished with problems, 1 warning (with warnings treated as errors).\n"
    );
}

/// Sphinx reads `<confdir>/docutils.conf`; this crate does not, and says so
/// once on stderr, outside the warning stream: the record-free build stays
/// record-free, the `-w` file empty and `-W` passing.
#[test]
fn docutils_conf_prints_a_notice_outside_the_stream() {
    let files = [("index.rst", "Title\n=====\n\nSection\n-------\n\nBody.\n")];
    let plain = temp_source("docutils-conf-control", &files);
    let configured = temp_source("docutils-conf", &files);
    std::fs::write(
        configured.join("docutils.conf"),
        "[general]\ninitial_header_level: 3\n",
    )
    .unwrap();

    let run = |src: &Path, name: &str| {
        let out = out_dir(name);
        let warnings = out_dir(&format!("{name}-log")).join("warnings.txt");
        let result = sphinx_build(&[
            src.to_str().unwrap(),
            out.to_str().unwrap(),
            "-q",
            "-W",
            "-w",
            warnings.to_str().unwrap(),
        ]);
        let written = std::fs::read_to_string(&warnings).unwrap();
        let page = std::fs::read(out.join("index.html")).unwrap();
        (result, written, page)
    };
    let (control, _, control_page) = run(&plain, "docutils-conf-control");
    assert!(control.status.success(), "stderr: {}", stderr_of(&control));

    let (result, written, page) = run(&configured, "docutils-conf");
    assert!(
        result.status.success(),
        "the notice is not counted by -W, stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(written, "", "the notice is not a record of the -w file");
    assert_eq!(
        stderr_of(&result),
        format!(
            "NOTICE: docutils.conf is not read by sphinx-ultra: {}\n",
            configured.join("docutils.conf").display()
        ),
        "the notice prints once"
    );
    assert_eq!(page, control_page, "the build is otherwise unchanged");
}

/// The notice is written as the warning stream writes stderr: a stderr
/// whose reader is gone (a broken pipe) loses it, and the build goes on.
#[test]
fn docutils_conf_notice_survives_a_closed_stderr() {
    let src = temp_source(
        "docutils-conf-closed-stderr",
        &[("index.rst", "Title\n=====\n\nBody.\n")],
    );
    std::fs::write(src.join("docutils.conf"), "[general]\n").unwrap();
    let out = out_dir("docutils-conf-closed-stderr");
    let mut child = bin()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "-q"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("binary should run");
    // Close the pipe's read end before the build writes the notice.
    drop(child.stderr.take());
    let status = child.wait().expect("the build ends");
    assert_eq!(
        status.code(),
        Some(0),
        "a closed stderr does not abort the build"
    );
    assert!(out.join("index.html").is_file());
}

#[test]
fn nitpicky_flags_broken_refs() {
    let src = temp_source(
        "nitpicky-broken",
        &[(
            "index.rst",
            "Title\n=====\n\nSee :doc:`missing_doc` and :ref:`missing-label`.\n",
        )],
    );

    // Without -n: `:doc:` and `:ref:` are `warn_dangling` roles, so Sphinx
    // reports them broken in a plain build too — `nitpicky` only widens the
    // warning to the roles that are *not* warn_dangling (and to the other
    // domains). Until the std domain landed, this crate reported neither
    // without -n; the environment-differential oracle (`doc_refs`,
    // `glossary_terms`, built with nitpicky off) is what settles it.
    let out_quiet = out_dir("nitpicky-broken-off");
    let result = build(&src, &out_quiet, &[]);
    assert!(result.status.success());
    let quiet_stderr = stderr_of(&result);
    assert!(
        quiet_stderr.contains("index.rst:4: WARNING: unknown document: 'missing_doc'")
            && quiet_stderr.contains("index.rst:4: WARNING: undefined label: 'missing-label'"),
        "warn_dangling roles report broken references without -n, stderr: {quiet_stderr}"
    );

    // With -n (compat mode): both broken refs warn, with line numbers
    let out = out_dir("nitpicky-broken-on");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-n"]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("index.rst:4: WARNING: unknown document: 'missing_doc'"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("index.rst:4: WARNING: undefined label: 'missing-label'"),
        "stderr: {stderr}"
    );
}

#[test]
fn nitpicky_resolves_python_refs_and_counts_cross_domain_ones() {
    let src = temp_source(
        "nitpicky-py-domain",
        &[(
            "index.rst",
            "Title\n=====\n\n.. py:function:: real.fn()\n\nCall :py:func:`real.fn` and :py:func:`missing.fn` and :func:`also.missing` and :c:func:`cfn`.\nSee `docs <https://example.com>`_ and :doc:`https://example.com/page`.\n",
        )],
    );
    let out = out_dir("nitpicky-py-domain");
    // `-v` for the cross-domain notice, which is this crate's progress log,
    // not a record.
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-n", "-v"]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    // (a) A defined py ref RESOLVES: under -n an unresolved one would have
    // to warn, so its absence from the warning stream is the proof.
    assert!(
        !stderr.contains("real.fn"),
        "the defined py ref must resolve silently, stderr: {stderr}"
    );
    // (b) A missing py ref warns in sphinx's exact non-std shape — through
    // the domain-prefixed `:py:func:` role and the bare `:func:` role alike
    // (primary_domain defaults to `py`).
    assert!(
        stderr.contains(
            "index.rst:6: WARNING: py:func reference target not found: missing.fn [ref.func]"
        ),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(
            "index.rst:6: WARNING: py:func reference target not found: also.missing [ref.func]"
        ),
        "stderr: {stderr}"
    );
    // (c) A `:c:func:` ref trips the cross-domain counter exactly once, and
    // the URL-shaped `:doc:` names no document, which Sphinx 9.1.0 says
    // as it says for any missing document (decision 17 ended the M1
    // carve-out that kept it silent; probed, the same line).
    assert!(
        stderr.contains(
            "index.rst:6: WARNING: unknown document: 'https://example.com/page' [ref.doc]"
        ),
        "stderr: {stderr}"
    );
    assert_eq!(
        stderr
            .matches("1 cross-domain reference(s) not validated (domain not implemented until M5)")
            .count(),
        1,
        "the cross-domain notice appears exactly once, stderr: {stderr}"
    );
}

#[test]
fn sphinx_build_refuses_output_overlapping_source() {
    let src = temp_source("sb-overlap", &[("index.rst", "Title\n=====\n\nBody.\n")]);

    // OUTPUTDIR == SOURCEDIR: sphinx-build refuses; sources must survive.
    let same = sphinx_build(&["-M", "clean", src.to_str().unwrap(), src.to_str().unwrap()]);
    assert_eq!(
        same.status.code(),
        Some(1),
        "-M clean into the source dir must refuse, stderr: {}",
        stderr_of(&same)
    );
    assert!(
        stderr_of(&same).contains("is same as source directory"),
        "stderr: {}",
        stderr_of(&same)
    );
    assert!(
        src.join("index.rst").is_file() && src.join("conf.py").is_file(),
        "sources must be untouched after the refused clean"
    );

    // Plain build into the source dir is refused the same way.
    let build_same = sphinx_build(&[src.to_str().unwrap(), src.to_str().unwrap()]);
    assert_eq!(build_same.status.code(), Some(1));

    // OUTPUTDIR being an ancestor of SOURCEDIR is refused too.
    let parent = src.parent().unwrap();
    let contains = sphinx_build(&[
        "-M",
        "clean",
        src.to_str().unwrap(),
        parent.to_str().unwrap(),
    ]);
    assert_eq!(
        contains.status.code(),
        Some(1),
        "stderr: {}",
        stderr_of(&contains)
    );
    assert!(
        stderr_of(&contains).contains("directory contains source directory"),
        "stderr: {}",
        stderr_of(&contains)
    );
}

#[test]
fn sphinx_build_requires_a_config() {
    let sandbox = out_dir("sb-no-conf");
    let src = sandbox.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("index.rst"), "Title\n=====\n\nBody.\n").unwrap();
    let out = sandbox.join("out");

    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "sphinx-build errors when the source has no conf.py, stderr: {}",
        stderr_of(&result)
    );
    assert!(
        stderr_of(&result).contains("doesn't contain a conf.py file"),
        "stderr: {}",
        stderr_of(&result)
    );

    // The native subcommand keeps its lenient auto-detect behavior.
    let native = build(&src, &out, &[]);
    assert!(
        native.status.success(),
        "native build still auto-detects/defaults, stderr: {}",
        stderr_of(&native)
    );
}

#[test]
fn nitpicky_resolves_real_refs() {
    let src = temp_source(
        "nitpicky-good",
        &[
            (
                "index.rst",
                "Title\n=====\n\n.. toctree::\n\n   installation\n\nSee :doc:`installation` and :ref:`setup-label` and :doc:`/installation`.\n",
            ),
            (
                "installation.rst",
                "Install\n=======\n\n.. _setup-label:\n\nSetup\n-----\n\nSteps.\n",
            ),
        ],
    );
    let out = out_dir("nitpicky-good");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-n"]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        !stderr.contains("unknown document") && !stderr.contains("undefined label"),
        "resolvable refs must not warn under -n, stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// intersphinx (tests/fixtures/intersphinx): a project whose conf.py maps one
// other project to a *local* inventory file, so the whole feature is
// exercised end to end without ever touching the network. Its conf.py does
// not load `sphinx.ext.intersphinx` (the HTML oracle's `intersphinx` case
// records what Sphinx does then); `-D extensions=sphinx.ext.intersphinx`
// loads it, as it does in Sphinx (`config.py:316-321`).
// ---------------------------------------------------------------------------

/// The `-D` override that loads intersphinx into the fixture project.
const LOAD_INTERSPHINX: [&str; 2] = ["-D", "extensions=sphinx.ext.intersphinx"];

#[test]
fn intersphinx_resolves_a_cross_project_ref_and_reports_a_missing_external() {
    let out = out_dir("intersphinx");
    let src = fixture("intersphinx");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        LOAD_INTERSPHINX[0],
        LOAD_INTERSPHINX[1],
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);

    // `:ref:`example`` names a label that exists only in the other
    // project's inventory: resolving it through intersphinx is what stops
    // the dangling-reference warning.
    assert!(
        !stderr.contains("undefined label"),
        "the cross-project label must resolve, stderr: {stderr}"
    );
    // `:external:std:ref:`whatever`` matches nothing anywhere, and reports
    // it in Sphinx's exact words — `type='ref', subtype=reftype` is what
    // makes the category `ref.ref`.
    assert!(
        stderr.contains(
            "index.rst:6: WARNING: external std:ref reference target not found: whatever [ref.ref]"
        ),
        "stderr: {stderr}"
    );
    // The closing summary is on the status stream, stdout.
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("build succeeded, 1 warning."),
        "the external miss is the only warning, stdout: {stdout}"
    );
}

/// `IntersphinxRole` reads its inventory name as written and calls the
/// domain's role with the text unescaped (`_resolve.py:378-473`): an
/// `intersphinx_mapping` key with a capital is found under that spelling
/// only, and an escaped `\<` opens an explicit target there. Sphinx 9.1.0,
/// probed on this project: exit 0 and the lowercase spelling's warning
/// alone, the two other references linking into the inventory. The stream
/// pins both halves: a lowercased inventory name warns on lines 4 and 6
/// too, and a target read through the escape warns `external std:ref
/// reference target not found: x <example>` on line 6.
#[test]
fn an_external_role_keeps_its_inventory_case_and_unescapes_its_text() {
    let src = temp_source(
        "external-role-case",
        &[
            (
                "conf.py",
                "extensions = ['sphinx.ext.intersphinx']\n\
                 intersphinx_mapping = {'Other': ('https://example.org/', 'local.inv')}\n",
            ),
            (
                "index.rst",
                "Title\n=====\n\nUpper :external+Other:ref:`example`.\n\n\
                 Escaped :external+Other:ref:`x \\<example>` here.\n\n\
                 Lower :external+other:ref:`example`.\n",
            ),
        ],
    );
    std::fs::copy(
        fixture("intersphinx").join("local.inv"),
        src.join("local.inv"),
    )
    .unwrap();
    let out = out_dir("external-role-case");
    let (result, written) = warning_stream(&src, &out, &[]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "<SRCDIR>/index.rst:8: WARNING: inventory for external cross-reference not found: \
         'other' [intersphinx.external]\n"
    );
}

#[test]
fn disabling_the_labels_objtype_takes_the_cross_project_ref_away_again() {
    // The control for the test above: with `std:label` disabled, the very
    // same bare `:ref:` stops resolving and the dangling warning comes back
    // — which is what proves intersphinx (and not something else) resolved
    // it.
    let out = out_dir("intersphinx-disabled");
    let src = fixture("intersphinx");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        LOAD_INTERSPHINX[0],
        LOAD_INTERSPHINX[1],
        "-D",
        "intersphinx_disabled_reftypes=std:label",
    ]);

    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("index.rst:4: WARNING: undefined label: 'example' [ref.ref]"),
        "stderr: {stderr}"
    );
}

/// Without `sphinx.ext.intersphinx` in `extensions`, Sphinx loads no
/// inventory and has no `:external:` role: the fixture's
/// `:external:std:ref:` is an unknown domain role, logged with no
/// location, then docutils' `[docutils]` ERROR for the role, and the label
/// only the inventory holds stays undefined (the HTML oracle's
/// `html_projects/intersphinx`, [TW F12]).
#[test]
fn without_the_extension_intersphinx_is_inert() {
    let out = out_dir("intersphinx-not-loaded");
    let src = fixture("intersphinx");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap()]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains(&format!(
            "WARNING: unknown role name: external:std:ref\n{}:6: ERROR: Unknown interpreted \
             text role \"external:std:ref\". [docutils]\n",
            sphinx_ultra::utils::canonicalize_simplified(&src)
                .unwrap()
                .join("index.rst")
                .display()
        )),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("index.rst:4: WARNING: undefined label: 'example' [ref.ref]"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("reference target not found"),
        "stderr: {stderr}"
    );
}

#[test]
fn an_invalid_intersphinx_mapping_exits_two_with_sphinxs_config_error() {
    // Sphinx raises ConfigError from `validate_intersphinx_mapping`, which
    // aborts the build; sphinx-build reports an exception with exit code 2.
    let src = temp_source(
        "intersphinx-bad-mapping",
        &[
            ("index.rst", "Title\n=====\n\nText.\n"),
            (
                "conf.py",
                "project = 'Bad'\nextensions = ['sphinx.ext.intersphinx']\n\
                 intersphinx_mapping = {'p': 'https://x/'}\n",
            ),
        ],
    );
    let out = out_dir("intersphinx-bad-mapping");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap()]);

    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("Invalid `intersphinx_mapping` configuration (1 error)."),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(
            "Invalid value `'https://x/'` in intersphinx_mapping['p']. \
             Expected a two-element tuple or list."
        ),
        "the per-entry error is logged before the abort, stderr: {stderr}"
    );
}

#[test]
fn an_empty_inventory_location_tuple_exits_two_with_sphinxs_invariant_error() {
    // `('https://x/', ())` passes `intersphinx_mapping` validation and then
    // fails `_IntersphinxProject`'s invariants when the inventories load,
    // which is a ConfigError in Sphinx — the same abort, one phase later.
    let src = temp_source(
        "intersphinx-empty-locations",
        &[
            ("index.rst", "Title\n=====\n\nText.\n"),
            (
                "conf.py",
                "project = 'Empty'\nextensions = ['sphinx.ext.intersphinx']\n\
                 intersphinx_mapping = {'p': ('https://x/', ())}\n",
            ),
        ],
    );
    let out = out_dir("intersphinx-empty-locations");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap()]);

    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("An invalid intersphinx_mapping entry was added after normalisation."),
        "stderr: {stderr}"
    );
}

/// `intersphinx_mapping` is validated, and its inventories loaded, only
/// with `sphinx.ext.intersphinx` loaded: without it the key is an ordinary
/// `conf.py` variable, so neither a malformed entry nor one that would
/// break an inventory invariant stops the build (Sphinx 9.1.0, probed:
/// exit 0, an empty `-w` file).
#[test]
fn without_the_extension_intersphinx_mapping_is_not_validated() {
    for (name, mapping) in [
        ("intersphinx-bad-no-extension", "{'p': 'https://x/'}"),
        (
            "intersphinx-empty-no-extension",
            "{'p': ('https://x/', ())}",
        ),
    ] {
        let conf = format!("project = 'P'\nintersphinx_mapping = {mapping}\n");
        let src = temp_source(
            name,
            &[
                ("index.rst", "Title\n=====\n\nText.\n"),
                ("conf.py", conf.as_str()),
            ],
        );
        let out = out_dir(name);
        let warnings = out_dir(&format!("{name}-log")).join("warnings.txt");
        let result = sphinx_build(&[
            src.to_str().unwrap(),
            out.to_str().unwrap(),
            "-q",
            "-w",
            warnings.to_str().unwrap(),
        ]);
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        assert_eq!(std::fs::read_to_string(&warnings).unwrap(), "", "{name}");
    }
}

/// `validate_intersphinx_mapping` logs one bare `ERROR` record per rejected
/// entry before it raises (`SP/sphinx/ext/intersphinx/_load.py:64-125`):
/// the records reach the warning stream, stderr and the `-w` file alike,
/// ahead of the `Configuration error!` report (Sphinx 9.1.0, probed; the
/// report as the HTML oracle reduces it).
#[test]
fn intersphinx_mapping_errors_reach_both_halves_before_the_report() {
    let src = temp_source(
        "intersphinx-mapping-errors",
        &[
            ("index.rst", "Title\n=====\n\nText.\n"),
            (
                "conf.py",
                "project = 'Bad'\nextensions = ['sphinx.ext.intersphinx']\n\
                 intersphinx_mapping = {'p': 'https://x/'}\n",
            ),
        ],
    );
    let out = out_dir("intersphinx-mapping-errors");
    let warnings = out_dir("intersphinx-mapping-errors-log").join("warnings.txt");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-w",
        warnings.to_str().unwrap(),
    ]);

    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    let written = String::from_utf8(std::fs::read(&warnings).unwrap()).unwrap();
    assert_eq!(
        written,
        "ERROR: Invalid value `'https://x/'` in intersphinx_mapping['p']. \
         Expected a two-element tuple or list.\n\
         \nConfiguration error!\n\n    sphinx.errors.ConfigError: Invalid \
         `intersphinx_mapping` configuration (1 error).\n"
    );
    assert_eq!(String::from_utf8(result.stderr).unwrap(), written);
}

/// The `ConfigError` raised while the inventories load gets the report a
/// configuration that fails to load gets: Sphinx's `handle_exception` writes
/// it to `args.error`, the tee of stderr and the `-w` file
/// (`sphinx/cmd/build.py:333-361`, `:395-452`), so both halves carry the
/// same bytes. Sphinx 9.1.0, probed with this project, exits 2 and writes
/// the identical report to both; the expected bytes are that report as the
/// HTML oracle's `reduce_sphinx_error_report` reduces it.
#[test]
fn a_load_time_config_error_writes_sphinxs_report_to_both_halves() {
    let src = temp_source(
        "intersphinx-load-config-error",
        &[
            ("index.rst", "Title\n=====\n\nText.\n"),
            (
                "conf.py",
                "extensions = ['sphinx.ext.intersphinx']\n\
                 intersphinx_mapping = {'x': ('https://x.invalid/', ())}\n",
            ),
        ],
    );
    let out = out_dir("intersphinx-load-config-error");
    let warnings = out_dir("intersphinx-load-config-error-log").join("warnings.txt");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-w",
        warnings.to_str().unwrap(),
    ]);

    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    // Strict UTF-8 on both sides, so the string comparison is byte-exact.
    let written = String::from_utf8(std::fs::read(&warnings).unwrap()).unwrap();
    assert_eq!(
        written,
        "\nConfiguration error!\n\n    sphinx.errors.ConfigError: An invalid \
         intersphinx_mapping entry was added after normalisation.\n"
    );
    assert_eq!(String::from_utf8(result.stderr).unwrap(), written);
}

/// A build educates quotes, dashes and apostrophes, as Sphinx's default
/// `smartquotes = True` (`config.py:289`) has it, so a title reaches the
/// search index curled; `-D smartquotes=0` turns that off; and any other
/// spelling, `False` included, is the `ConfigError` Sphinx raises for a
/// bool option (`convert_overrides`, `config.py:366-374`): exit 2 and
/// Sphinx's report.
//
// oracle (Sphinx 9.1.0, smart-quotes report S2 `probe_conftypes.out`):
//   `-D smartquotes=0` leaves `"Hi" -- it's`; `-D smartquotes=False`
//   raises `ConfigError: 'smartquotes' must be '0' or '1', got 'False'`.
#[test]
fn smartquotes_follow_the_default_and_the_d_override() {
    let src = temp_source(
        "smartquotes",
        &[("index.rst", "\"Hi\" -- it's\n============\n\nText.\n")],
    );
    let titles = |name: &str, overrides: &[&str]| {
        let out = out_dir(name);
        let mut args = vec![src.to_str().unwrap(), out.to_str().unwrap(), "-q"];
        args.extend_from_slice(overrides);
        let result = sphinx_build(&args);
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        search_value(&out)["titles"].clone()
    };
    assert_eq!(
        titles("smartquotes-default", &[]),
        serde_json::json!(["\u{201c}Hi\u{201d} \u{2013} it\u{2019}s"])
    );
    assert_eq!(
        titles("smartquotes-off", &["-D", "smartquotes=0"]),
        serde_json::json!(["\"Hi\" -- it's"])
    );

    let out = out_dir("smartquotes-false");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-D",
        "smartquotes=False",
    ]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(
        stderr_of(&result),
        "\nConfiguration error!\n\n    sphinx.errors.ConfigError: 'smartquotes' must be \
         '0' or '1', got 'False'\n"
    );
}

/// A `-D smartquotes_excludes.<key>` override is the string as given,
/// which Sphinx's `in` searches: `dirhtml` excludes the `html` builder,
/// `en_GB` the `en` language, and `man,text` is no list and excludes no
/// `html`. With no `conf.py` dict the override starts an empty one, so a
/// `ja` build, which the default dict excludes, educates with `ja`'s
/// quotes.
//
// oracle (Sphinx 9.1.0 `-b html` builds, wave 5a T16b fix round 1,
//   scratch `t16bf/probe_excludes.out`): the doctree's text, `"Hi" --
//   it's` or educated as asserted.
#[test]
fn smartquotes_excludes_overrides_are_searched_strings() {
    let titles = |name: &str, conf: &str, overrides: &[&str]| {
        let src = temp_source(
            name,
            &[
                ("conf.py", conf),
                ("index.rst", "\"Hi\" -- it's\n============\n\nText.\n"),
            ],
        );
        let out = out_dir(name);
        let mut args = vec![src.to_str().unwrap(), out.to_str().unwrap(), "-q"];
        args.extend_from_slice(overrides);
        let result = sphinx_build(&args);
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        assert_eq!(stderr_of(&result), "", "{name}");
        search_value(&out)["titles"].clone()
    };
    let straight = serde_json::json!(["\"Hi\" -- it's"]);
    let curled = serde_json::json!(["\u{201c}Hi\u{201d} \u{2013} it\u{2019}s"]);
    let project = "project = 'Temp'\n";
    for (name, value, expected) in [
        (
            "sq-excl-dirhtml",
            "smartquotes_excludes.builders=dirhtml",
            &straight,
        ),
        (
            "sq-excl-man-text",
            "smartquotes_excludes.builders=man,text",
            &curled,
        ),
        (
            "sq-excl-en-gb",
            "smartquotes_excludes.languages=en_GB",
            &straight,
        ),
    ] {
        assert_eq!(&titles(name, project, &["-D", value]), expected, "{value}");
    }
    let japanese = "project = 'Temp'\nlanguage = 'ja'\n";
    assert_eq!(titles("sq-excl-ja", japanese, &[]), straight);
    assert_eq!(
        titles(
            "sq-excl-ja-man",
            japanese,
            &["-D", "smartquotes_excludes.builders=man"]
        ),
        serde_json::json!(["\u{300c}Hi\u{300d} \u{2013} it\u{2019}s"])
    );
}

/// A `smartquotes` in `conf.py` that is no bool keeps smart quotes on, as
/// Sphinx tests it with `is False`, and earns `check_confval_types`'
/// warning, with no location and no type, which `-W` counts; an `int`,
/// `bool`'s base, earns none.
//
// oracle (Sphinx 9.1.0 `-b html` builds, wave 5a T16b fix round 1,
//   scratch `t16bf/probe_conftypes.out`): stderr and the `-w` file hold
//   the one line asserted; `-W` exits 1.
#[test]
fn a_mistyped_smartquotes_warns_and_keeps_smart_quotes_on() {
    let src = temp_source(
        "smartquotes-none",
        &[
            ("conf.py", "project = 'Temp'\nsmartquotes = None\n"),
            ("index.rst", "\"Hi\" -- it's\n============\n\nText.\n"),
        ],
    );
    let out = out_dir("smartquotes-none");
    let warnings = out_dir("smartquotes-none-w").join("warnings.txt");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-w",
        warnings.to_str().unwrap(),
    ]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let line = "WARNING: The config value `smartquotes' has type `NoneType'; expected `bool'.\n";
    assert_eq!(stderr_of(&result), line);
    assert_eq!(std::fs::read_to_string(&warnings).unwrap(), line);
    assert_eq!(
        search_value(&out)["titles"],
        serde_json::json!(["\u{201c}Hi\u{201d} \u{2013} it\u{2019}s"])
    );

    let out = out_dir("smartquotes-none-W");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-q", "-W"]);
    assert_eq!(
        result.status.code(),
        Some(1),
        "stderr: {}",
        stderr_of(&result)
    );

    let src = temp_source(
        "smartquotes-zero",
        &[
            ("conf.py", "project = 'Temp'\nsmartquotes = 0\n"),
            ("index.rst", "\"Hi\" -- it's\n============\n\nText.\n"),
        ],
    );
    let out = out_dir("smartquotes-zero");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-q"]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(stderr_of(&result), "");
}

/// `source_encoding` is a real key (panel fix round B, [19]): `-D` reaches
/// it, a non-UTF-8 value prints sphinx's deprecation warning byte-exactly
/// (probed), and the warning counts toward `-W`.
#[test]
fn sphinx_build_d_source_encoding_warns_about_deprecation_like_sphinx() {
    let src = fixture("basic");
    let out = out_dir("sb-D-source-encoding");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-D",
        "source_encoding=latin-1",
    ]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        stderr_of(&result).contains(
            "Support for source encodings other than UTF-8 is deprecated and will be removed \
             in Sphinx 10. Please comment at https://github.com/sphinx-doc/sphinx/issues/13665 \
             if this causes a problem."
        ),
        "stderr: {}",
        stderr_of(&result)
    );
    assert!(
        !stderr_of(&result).contains("unknown config value"),
        "stderr: {}",
        stderr_of(&result)
    );

    let out_w = out_dir("sb-D-source-encoding-W");
    let result_w = sphinx_build(&[
        src.to_str().unwrap(),
        out_w.to_str().unwrap(),
        "-D",
        "source_encoding=latin-1",
        "-W",
    ]);
    assert_eq!(result_w.status.code(), Some(1));

    let quiet = out_dir("sb-D-source-encoding-utf8");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        quiet.to_str().unwrap(),
        "-D",
        "source_encoding=utf-8",
    ]);
    assert!(result.status.success());
    assert!(!stderr_of(&result).contains("deprecated"));
}

/// F8b: every logger warning located at a node (`location=node`) prints
/// the node's source made absolute (`get_node_location`'s `abspath`), so a
/// node from an included file prints `<SRCDIR>/part.inc:N`, while the
/// reporter's records keep the include's relative spelling. Probed on
/// Sphinx 9.1.0 for each phase that warns about a node: the parse-time
/// duplicates (std and py), the index and std domains in stage M, the
/// numbering (`location=toctreenode`), and the resolve pass's
/// `ReferencesResolver` and `OnlyNodeTransform`; the math and citation
/// domains already located theirs so. `abspath` does not depend on the
/// cwd: the bytes are the same from the source directory and from its
/// parent.
#[test]
fn included_file_node_locations_are_absolute() {
    let src = temp_source(
        "t14-node-locations",
        &[
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n   :numbered:\n\n   a\n\n.. include:: num.inc\n\n\
                 .. toctree::\n\n   other\n   zdoc\n",
            ),
            ("a.rst", "A\n=\n\nSec\n---\n"),
            ("b.rst", "B\n=\n\nBsec\n----\n"),
            (
                "num.inc",
                "Part\n\n.. toctree::\n   :numbered:\n\n   a\n   b\n",
            ),
            (
                "other.rst",
                "Other\n=====\n\n.. _lbl:\n\nSec\n---\n\n.. envvar:: XVAR\n\n\
                 .. py:function:: f()\n\n.. [cit] other citation\n\n.. math::\n   :label: eq1\n\n   \
                 x\n\nSee [cit]_ and :eq:`eq1`.\n",
            ),
            ("zdoc.rst", "Zdoc\n====\n\n.. include:: part.inc\n"),
            (
                "part.inc",
                "Part para.\n\n.. _lbl:\n\nPsec\n----\n\n.. envvar:: XVAR\n\n\
                 .. py:function:: f()\n\n.. index:: pair: lonely\n\n.. [cit] part citation\n\n\
                 .. math::\n   :label: eq1\n\n   y\n\nSee :ref:`missing-target` and [cit]_ and \
                 :eq:`nosuch` and [nocit]_.\n\n.. only:: html and\n\n   Kept.\n",
            ),
        ],
    );
    let expected = "\
<SRCDIR>/part.inc:8: WARNING: duplicate envvar description of XVAR, other instance in other
<SRCDIR>/part.inc:10: WARNING: duplicate object description of f, other instance in other, use :no-index: for one of them
<SRCDIR>/part.inc:16: WARNING: duplicate label of equation eq1, other instance in other [ref.equation]
<SRCDIR>/part.inc:14: WARNING: duplicate citation cit, other instance in <SRCDIR>/other.rst [ref.citation]
<SRCDIR>/part.inc:12: WARNING: invalid pair index entry 'lonely' [index]
<SRCDIR>/part.inc:6: WARNING: duplicate label lbl, other instance in <SRCDIR>/other.rst
<SRCDIR>/num.inc:3: WARNING: a is already assigned section numbers (nested numbered toctree?) [toc.secnum]
<SRCDIR>/part.inc:21: WARNING: undefined label: 'missing-target' [ref.ref]
<SRCDIR>/part.inc:21: WARNING: equation not found: nosuch [ref.eq]
<SRCDIR>/part.inc:21: WARNING: citation not found: nocit [ref.ref]
<SRCDIR>/part.inc:23: WARNING: exception while evaluating only directive expression: unexpected token 'end of template'
  line 1
";
    let parent = src.parent().expect("the source directory has a parent");
    for (name, cwd, source) in [("srcdir", src.as_path(), "."), ("parent", parent, "")] {
        let out = out_dir(&format!("t14-node-locations-{name}"));
        let log = out.with_extension("w");
        let source = if source.is_empty() {
            src.to_str().unwrap()
        } else {
            source
        };
        let result = bin()
            .current_dir(cwd)
            .args([
                source,
                out.to_str().unwrap(),
                "-q",
                "-w",
                log.to_str().unwrap(),
            ])
            .output()
            .expect("binary should run");
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        assert_eq!(
            srcdir_spelled(&std::fs::read(&log).unwrap(), &src),
            expected,
            "{name}"
        );
    }
}

/// `-t` adds a tag to the build's, which `.. only::` reads in the resolve
/// pass (`OnlyNodeTransform`): with `-t mytag` the content of `.. only::
/// mytag` stays and that of `.. only:: not mytag` goes, and without it the
/// other way round. Seen in what the search index holds, which Sphinx
/// feeds from the resolved doctree; probed on Sphinx 9.1.0 (`-b html`):
/// `keptalpha` and `word` with the tag, `droppedbeta` and `word` without.
#[test]
fn only_directive_follows_cli_tags() {
    let src = temp_source(
        "t14-only-tags",
        &[(
            "index.rst",
            "Tags\n====\n\n.. only:: mytag\n\n   Keptalpha words.\n\n.. only:: not mytag\n\n   \
             Droppedbeta words.\n",
        )],
    );
    for (name, extra, terms) in [
        ("tagged", &["-t", "mytag"][..], ["keptalpha", "word"]),
        ("untagged", &[][..], ["droppedbeta", "word"]),
    ] {
        let out = out_dir(&format!("t14-only-tags-{name}"));
        let mut args = vec![src.to_str().unwrap(), out.to_str().unwrap(), "-q"];
        args.extend_from_slice(extra);
        let result = sphinx_build(&args);
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        let indexed: Vec<String> = search_value(&out)["terms"]
            .as_object()
            .expect("terms is an object")
            .keys()
            .cloned()
            .collect();
        assert_eq!(indexed, terms, "{name}");
    }
}

/// `trim_doctest_flags` reaches the resolve pass: the search index, which
/// Sphinx feeds from the resolved doctree, holds no `# doctest:` comment of
/// a doctest block by default and holds it under `-D
/// trim_doctest_flags=0`. Probed on Sphinx 9.1.0 (`-b html`): `comput` and
/// `result`, then `comput`, `doctest`, `result` and `skip`.
#[test]
fn trim_doctest_flags_reaches_the_search_index() {
    let src = temp_source(
        "t14-trim-flags",
        &[(
            "index.rst",
            "Trim\n====\n\n>>> compute()  # doctest: +SKIP\nresult\n",
        )],
    );
    for (name, extra, terms) in [
        ("default", &[][..], &["comput", "result"][..]),
        (
            "off",
            &["-D", "trim_doctest_flags=0"][..],
            &["comput", "doctest", "result", "skip"][..],
        ),
    ] {
        let out = out_dir(&format!("t14-trim-flags-{name}"));
        let mut args = vec![src.to_str().unwrap(), out.to_str().unwrap(), "-q"];
        args.extend_from_slice(extra);
        let result = sphinx_build(&args);
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        let indexed: Vec<String> = search_value(&out)["terms"]
            .as_object()
            .expect("terms is an object")
            .keys()
            .cloned()
            .collect();
        assert_eq!(indexed, terms, "{name}");
    }
}

/// Decision 14: custom roles and the default role follow Sphinx's
/// build-scoped registry, documents read in sorted docname order, under
/// `-j 4` as under `-j 1`. `d01` uses `:red:` before `d02` defines it;
/// `d03` defines `blue` over `red`, so its parse needs `d02`'s registry;
/// `d04` uses both; `d05`'s `.. default-role::` (no `default_role` set)
/// leaks into `d06` and `d07`, which `d07` then resets for `d08`. Each
/// document's parse in parallel starts from the baseline registry, and
/// every one after `d02`, the first to leave another, is read again in
/// order. The pinned streams are Sphinx 9.1.0's `-j 1` ones (its own `-j 4`
/// reads chunks in separate processes, where the registry does not
/// carry). A warm build in which `d04` alone is outdated reads it from
/// the baseline registry: the cached `d02` and `d03` contribute nothing,
/// as in Sphinx's incremental build.
#[test]
fn custom_roles_under_parallel_reads() {
    let index = format!(
        "Index\n=====\n\n.. toctree::\n\n{}",
        (1..=8).map(|n| format!("   d0{n}\n")).collect::<String>()
    );
    let files = [
        ("index.rst", index.as_str()),
        ("d01.rst", "D01\n===\n\n:red:`a` comes before its role.\n"),
        ("d02.rst", "D02\n===\n\n.. role:: red\n\n:red:`b` is red.\n"),
        (
            "d03.rst",
            "D03\n===\n\n.. role:: blue(red)\n\n:blue:`c` is blue.\n",
        ),
        ("d04.rst", "D04\n===\n\n:red:`d` and :blue:`e`.\n"),
        ("d05.rst", "D05\n===\n\n.. default-role:: pep-reference\n"),
        ("d06.rst", "D06\n===\n\n`abc` is no PEP.\n"),
        ("d07.rst", "D07\n===\n\n`8` is one.\n\n.. default-role::\n"),
        ("d08.rst", "D08\n===\n\n`abc` is a title.\n"),
    ];
    let src = temp_source("t17a-parallel-roles", &files);
    let cold = "\
<SRCDIR>/d01.rst:4: ERROR: Unknown interpreted text role \"red\". [docutils]
<SRCDIR>/d06.rst:4: ERROR: PEP number must be a number from 0 to 9999; \"abc\" is invalid. [docutils]
";
    for jobs in ["4", "1"] {
        let (result, written) = warning_stream(
            &src,
            &out_dir(&format!("t17a-parallel-roles-j{jobs}")),
            &["-j", jobs],
        );
        assert!(result.status.success(), "stderr: {}", stderr_of(&result));
        assert_eq!(written, cold, "-j {jobs}");
    }

    let out = out_dir("t17a-parallel-roles-warm");
    let (result, written) = warning_stream(&src, &out, &["-j", "4"]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, cold);
    touch(&src.join("d04.rst"));
    let (result, written) = warning_stream(&src, &out, &["-j", "4"]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "\
<SRCDIR>/d04.rst:4: ERROR: Unknown interpreted text role \"red\". [docutils]
<SRCDIR>/d04.rst:4: ERROR: Unknown interpreted text role \"blue\". [docutils]
"
    );
}

/// A build of `files` that Sphinx 9.1.0 crashes on (decision 13): exit
/// status 2, and the report — on stderr and in the `-w` file
/// ([`crash_stream`]) — naming the exception Sphinx raises, and where the
/// input is.
fn assert_crash_exits_two(test: &str, files: &[(&str, &str)], exception: &str, location: &str) {
    let stderr = crash_stream(test, files, &[], &[]);
    assert!(
        stderr.contains(&format!("\n    {exception}\n")),
        "the report names the crash: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{location}: Sphinx 9.1.0 crashes on this input; sphinx-ultra stops the build as \
             it does."
        )),
        "the report says where: {stderr}"
    );
}

/// Decision 13: a trailing `&` leaves `GUILabel` an empty span, whose first
/// character Sphinx 9.1.0 takes: `IndexError`.
#[test]
fn guilabel_trailing_ampersand_exits_two() {
    assert_crash_exits_two(
        "t17a-crash-guilabel",
        &[("index.rst", "Title\n=====\n\nPress :guilabel:`a&`.\n")],
        "IndexError: string index out of range",
        "<SRCDIR>/index.rst:4",
    );
}

/// Decision 13: `MenuSelection` is `GUILabel` after its `-->`s: the same
/// `IndexError`.
#[test]
fn menuselection_trailing_ampersand_exits_two() {
    assert_crash_exits_two(
        "t17a-crash-menuselection",
        &[(
            "index.rst",
            "Title\n=====\n\nOpen :menuselection:`File --> Save &`.\n",
        )],
        "IndexError: string index out of range",
        "<SRCDIR>/index.rst:4",
    );
}

/// Decision 13: an entry type `process_index_entry` no longer supports, in
/// an `:index:` role's explicit target, raises `ValueError` out of the
/// role.
#[test]
fn index_role_removed_entry_type_exits_two() {
    assert_crash_exits_two(
        "t17a-crash-index",
        &[(
            "index.rst",
            "Title\n=====\n\nSee :index:`os <module: os>`.\n",
        )],
        "ValueError: 'module' is no longer supported for index entries (from entry 'module: \
         os'). Use 'pair: module; os' instead.",
        "<SRCDIR>/index.rst:4",
    );
}

/// Decision 13: `Manpage` builds its link with
/// `manpages_url.format_map(info)` over the page's `path`, `page` and
/// `section`, so a field naming none of them raises `KeyError` out of the
/// role, and a positional field `ValueError` (Sphinx 9.1.0, probed: exit 2
/// with each).
#[test]
fn manpages_url_unknown_field_exits_two() {
    for (test, url, exception) in [
        (
            "t17a-crash-manpages-url-key",
            "https://man.example/{foo}",
            "KeyError: 'foo'",
        ),
        (
            "t17a-crash-manpages-url-positional",
            "https://man.example/{}",
            "ValueError: Format string contains positional fields",
        ),
    ] {
        let conf = format!("manpages_url = '{url}'\n");
        assert_crash_exits_two(
            test,
            &[
                ("conf.py", conf.as_str()),
                ("index.rst", "Title\n=====\n\nSee :manpage:`ls(1)`.\n"),
            ],
            exception,
            "<SRCDIR>/index.rst:4",
        );
    }
}

/// Decision 13: with no `default_role`, a `.. default-role::` of a Sphinx
/// role leaks into the documents read after its own, and the first of
/// them that uses bare interpreted text calls that role with no default
/// role to name it: `SphinxError('cannot determine default role!')`,
/// reported under Sphinx's `Sphinx error!` heading. Under `-j 4` too: the
/// leak reaches `b` through the sorted re-read.
#[test]
fn leaked_sphinx_default_role_exits_two() {
    let files = [
        ("index.rst", "Index\n=====\n\n.. toctree::\n\n   a\n   b\n"),
        (
            "a.rst",
            "A\n=\n\n.. default-role:: kbd\n\nPress `Ctrl+C`.\n",
        ),
        ("b.rst", "B\n=\n\nThen `y` here.\n"),
    ];
    assert_crash_exits_two(
        "t17a-crash-default-role",
        &files,
        "sphinx.errors.SphinxError: cannot determine default role!",
        "<SRCDIR>/b.rst:4",
    );
    let src = temp_source("t17a-crash-default-role-j4", &files);
    let out = out_dir("t17a-crash-default-role-j4");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-q",
        "-j",
        "4",
    ]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    let stderr = stderr_of(&result);
    assert!(
        stderr.starts_with("\nSphinx error!\n\n    sphinx.errors.SphinxError:"),
        "{stderr}"
    );
}

/// Decision 13: `IntersphinxRole.run` opens with `assert self.name ==
/// self.orig_name.lower()` (`_resolve.py:379`), so Sphinx 9.1.0 crashes
/// with a message-less `AssertionError` when an `:external:` role is called
/// by another name: through a custom role over it, or as a default role
/// whose name has a capital (`SphinxRole.__call__` lowercases only a
/// written name). Probed on both projects: exit 2, the traceback ending in
/// the bare `AssertionError`.
#[test]
fn external_role_called_by_another_name_exits_two() {
    for (test, index) in [
        (
            "t17a-crash-external-custom-role",
            "Title\n=====\n\n.. role:: ext(external+myinv:py:class)\n\nCustom :ext:`int`.\n",
        ),
        (
            "t17a-crash-external-default-role",
            "Title\n=====\n\n.. default-role:: external+MyInv:py:class\n\nDefault `int`.\n",
        ),
    ] {
        assert_crash_exits_two(
            test,
            &[
                ("conf.py", "extensions = ['sphinx.ext.intersphinx']\n"),
                ("index.rst", index),
            ],
            "AssertionError",
            "<SRCDIR>/index.rst:6",
        );
    }
}

/// Sphinx looks `default_role` up for each document inside
/// `sphinx_domains` but before `source-read`, where intersphinx installs
/// the dispatcher that claims the `external:` names
/// (`SP/sphinx/util/docutils.py:886-898`,
/// `SP/sphinx/ext/intersphinx/__init__.py:81`). A configured `:external:`
/// default role is therefore an unknown domain's role, not found, in every
/// document, and bare interpreted text stays a `title_reference`. Sphinx
/// 9.1.0's stream for this project, probed: both warnings once per
/// document, in read order, the first with no location and the second at
/// the document.
#[test]
fn a_configured_external_default_role_is_not_found() {
    let src = temp_source(
        "t17a-external-default-role-conf",
        &[
            (
                "conf.py",
                "extensions = ['sphinx.ext.intersphinx']\n\
                 intersphinx_mapping = {'myinv': ('https://other.example/', 'local.inv')}\n\
                 default_role = 'external+myinv:py:class'\n",
            ),
            (
                "index.rst",
                "Title\n=====\n\n.. toctree::\n\n   other\n\nDefault `int`.\n",
            ),
            ("other.rst", "Other\n=====\n\nAlso `str`.\n"),
        ],
    );
    std::fs::copy(
        fixture("intersphinx").join("local.inv"),
        src.join("local.inv"),
    )
    .unwrap();
    let out = out_dir("t17a-external-default-role-conf");
    let (result, written) = warning_stream(&src, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "\
WARNING: unknown role name: external+myinv:py:class
<SRCDIR>/index.rst: WARNING: default role external+myinv:py:class not found
WARNING: unknown role name: external+myinv:py:class
<SRCDIR>/other.rst: WARNING: default role external+myinv:py:class not found
"
    );
}

/// A resolver that logs a warning and still resolves the reference
/// resolves it on every path that calls it. `MathDomain.resolve_any_xref`
/// is the domain's `resolve_xref` (`SP/sphinx/domains/math.py:136-150`).
/// The self-referential retry of `intersphinx_resolve_self`
/// (`SP/sphinx/transforms/post_transforms/__init__.py:140-155`) runs the
/// domain again and keeps what it returns, which for `:numref:` can be the
/// content node. So a `math_eqref_format` the number cannot fill warns
/// `Invalid math_eqref_format` and the reference still links, under
/// `:any:` and after the retry as under `:eq:`. A `:numref:` that numfig
/// leaves alone warns `numfig is disabled` after the retry, with no
/// dangling warning. Sphinx 9.1.0's streams, probed on each project (the
/// two retry projects load the fixture inventory, which holds neither
/// target).
#[test]
fn a_reference_resolved_with_a_warning_resolves_on_every_path() {
    let equation = "Title\n=====\n\n.. math:: x^2\n   :label: why\n\n";
    let any = format!("{equation}See :eq:`why` and :any:`why`.\n");
    let retried = format!("{equation}Self :eq:`self:why` and :any:`self:why`.\n");
    let invalid = "<SRCDIR>/index.rst:7: WARNING: Invalid math_eqref_format: KeyError('foo')\n";
    let resolve_self = "extensions = ['sphinx.ext.intersphinx']\n\
                        intersphinx_mapping = {'other': ('https://other.example/', 'local.inv')}\n\
                        intersphinx_resolve_self = 'self'\n";
    let eqref_format = "math_eqref_format = '{foo} {number}'\n";
    for (test, conf, index, expected) in [
        (
            "t17a-any-eqref-format",
            eqref_format.to_string(),
            any.as_str(),
            invalid.repeat(2),
        ),
        (
            "t17a-retry-eqref-format",
            format!("{resolve_self}{eqref_format}"),
            retried.as_str(),
            invalid.repeat(2),
        ),
        (
            "t17a-retry-numref-kept",
            resolve_self.to_string(),
            "Title\n=====\n\n.. table:: The caption.\n   :name: tbl\n\n   =  =\n   a  b\n   =  =\n\n\
             See :numref:`self:tbl`.\n",
            "<SRCDIR>/index.rst:11: WARNING: numfig is disabled. :numref: is ignored.\n".to_string(),
        ),
    ] {
        let src = temp_source(test, &[("conf.py", conf.as_str()), ("index.rst", index)]);
        if conf.contains("local.inv") {
            std::fs::copy(
                fixture("intersphinx").join("local.inv"),
                src.join("local.inv"),
            )
            .unwrap();
        }
        let (result, written) = warning_stream(&src, &out_dir(test), &[]);
        assert!(result.status.success(), "{test}: {}", stderr_of(&result));
        assert_eq!(written, expected, "{test}");
    }
}

/// The report a build that Sphinx 9.1.0 crashes on ends its warning stream
/// with (decision 13, `SphinxCrash::report`): the category line of one of
/// Sphinx's own errors, the exception, and where the input is, when the
/// crash has a place in a document.
fn crash_report(category: Option<&str>, exception: &str, location: Option<&str>) -> String {
    let category = category
        .map(|category| format!("{category}!\n"))
        .unwrap_or_default();
    let location = location
        .map(|location| format!("{location}: "))
        .unwrap_or_default();
    format!(
        "\n{category}\n    {exception}\n\n{location}Sphinx 9.1.0 crashes on this input; \
         sphinx-ultra stops the build as it does.\n"
    )
}

/// The report of the `SphinxError` Sphinx 9.1.0 raises on purpose when its
/// read did not find the root document, `Sphinx is unable to load the
/// master document <message>`: the category line and the exception, the
/// form of the `Configuration error!` report and all of Sphinx's report the
/// HTML oracle keeps (`reduce_sphinx_error_report`). It does not say
/// Sphinx crashes, as [`crash_report`] does: Sphinx does not.
fn root_document_error(message: &str) -> String {
    format!(
        "\nSphinx error!\n\n    sphinx.errors.SphinxError: Sphinx is unable to load the master \
         document {message}\n"
    )
}

/// The warning stream of a sphinx-build of `files` that Sphinx 9.1.0
/// crashes on (decision 13), or stops on purpose ([`root_document_error`]),
/// run with `env` set and `extra` arguments: the build exits 2, and stderr
/// holds what the `-w` file does, which is returned with the source
/// directory spelled `<SRCDIR>/`.
fn crash_stream(
    test: &str,
    files: &[(&str, &str)],
    env: &[(&str, &str)],
    extra: &[&str],
) -> String {
    let src = temp_source(test, files);
    let out = out_dir(test);
    let log = out.with_extension("w");
    let _ = std::fs::remove_file(&log);
    let mut command = bin();
    command
        .arg(&src)
        .arg(&out)
        .args(["-q", "-w"])
        .arg(&log)
        .args(extra);
    for (key, value) in env {
        command.env(key, value);
    }
    let result = command.output().expect("binary should run");
    assert_eq!(
        result.status.code(),
        Some(2),
        "{test}: {}",
        stderr_of(&result)
    );
    let written = srcdir_spelled(&std::fs::read(&log).unwrap(), &src);
    assert_eq!(
        srcdir_spelled(&result.stderr, &src),
        written,
        "{test}: the -w file holds what stderr does"
    );
    written
}

/// The project of the stream-before-the-crash pins: a configuration
/// warning, one in the document read before the crashing one, and the
/// crashing document's own before and after the input Sphinx crashes on.
const STREAM_BEFORE_THE_CRASH: [(&str, &str); 5] = [
    ("conf.py", "project = 'Temp'\nhtml_logo = 'missing.png'\n"),
    (
        "index.rst",
        "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   c\n",
    ),
    ("a.rst", "A\n=\n\nSee :nope:`x` here.\n"),
    (
        "b.rst",
        "B\n=\n\nFirst :nope:`y`.\n\nPress :guilabel:`a&`.\n\nLater :nope:`z` and `dangling`_.\n",
    ),
    ("c.rst", "C\n=\n\nAlso :nope:`w`.\n"),
];

/// What Sphinx 9.1.0 prints for [`STREAM_BEFORE_THE_CRASH`], probed under
/// its default `-j 1`: the configuration's record, the record of `a`, read
/// before `b`, and the one `b` raised before its role crashed.
fn stream_before_the_crash() -> String {
    format!(
        "\
WARNING: logo file 'missing.png' does not exist
<SRCDIR>/a.rst:4: ERROR: Unknown interpreted text role \"nope\". [docutils]
<SRCDIR>/b.rst:4: ERROR: Unknown interpreted text role \"nope\". [docutils]
{}",
        crash_report(
            None,
            "IndexError: string index out of range",
            Some("<SRCDIR>/b.rst:6")
        )
    )
}

/// Decision 13, the stream before the crash: Sphinx 9.1.0 reads the
/// documents one at a time in sorted docname order and holds their
/// warnings back until the read ends (`logging.pending_warnings()`), which
/// flushes them when the read raises. So a crash prints, before its
/// report, the configuration's records, every record of the documents read
/// before the crashing one, and the crashing document's records up to the
/// crash: here a role's, at parse time, so not the document's later ones.
/// The build prints the same stream whatever its `-j`.
#[test]
fn a_crash_prints_the_records_before_it() {
    for jobs in ["1", "4"] {
        assert_eq!(
            crash_stream(
                &format!("t17a2-stream-parse-j{jobs}"),
                &STREAM_BEFORE_THE_CRASH,
                &[],
                &["-j", jobs],
            ),
            stream_before_the_crash(),
            "-j {jobs}"
        );
    }
}

/// The native subcommand writes the same stream to its `-w` file when the
/// build crashes: the configuration's records, which it otherwise writes
/// once the build is done, then the records before the crash, then the
/// report; and it exits 2.
#[test]
fn a_native_crash_writes_the_configuration_records_first() {
    let src = temp_source("t17a2-stream-native", &STREAM_BEFORE_THE_CRASH);
    let src = sphinx_ultra::utils::canonicalize_simplified(&src).unwrap();
    let out = out_dir("t17a2-stream-native");
    let log = out.with_extension("w");
    let _ = std::fs::remove_file(&log);
    let result = build(&src, &out, &["-w", log.to_str().unwrap()]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(
        srcdir_spelled(&std::fs::read(&log).unwrap(), &src),
        stream_before_the_crash()
    );
}

/// Decision 13: when several documents would crash, the first in Sphinx's
/// read order decides, whatever stage it crashes in. Sphinx reads `a`
/// whole — parse and transforms — before it parses `b`, so `a`'s crash in
/// `HandleCodeBlocks` (210) wins over `b`'s role crash at parse time, and
/// `a`'s parse records print before it. Probed on Sphinx 9.1.0 (`-j 1`);
/// the build's result does not depend on its `-j`.
#[test]
fn the_first_crash_in_read_order_decides() {
    let files = [
        ("conf.py", "project = 'Temp'\nhtml_logo = 'missing.png'\n"),
        (
            "index.rst",
            "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   c\n",
        ),
        (
            "a.rst",
            "A\n=\n\nSee :nope:`x`.\n\n.. epigraph::\n\n   .. default-domain:: py\n\nAfter :nope:`z`.\n",
        ),
        ("b.rst", "B\n=\n\nPress :guilabel:`a&`.\n"),
        ("c.rst", "C\n=\n\nAlso :nope:`w`.\n"),
    ];
    let expected = format!(
        "\
WARNING: logo file 'missing.png' does not exist
<SRCDIR>/a.rst:4: ERROR: Unknown interpreted text role \"nope\". [docutils]
<SRCDIR>/a.rst:10: ERROR: Unknown interpreted text role \"nope\". [docutils]
{}",
        crash_report(
            None,
            "AssertionError: Losing \"classes\" attribute: ['epigraph']",
            Some("<SRCDIR>/a.rst:8")
        )
    );
    for jobs in ["1", "4"] {
        assert_eq!(
            crash_stream(
                &format!("t17a2-two-crashes-j{jobs}"),
                &files,
                &[],
                &["-j", jobs],
            ),
            expected,
            "-j {jobs}"
        );
    }
}

/// Decision 13, a crash at resolve time: Sphinx 9.1.0 resolves each
/// document as it writes it, in sorted docname order, after the whole
/// read and the consistency checks. So the stream holds every read
/// record, the consistency check's, the resolve records of the documents
/// written before the crashing one, and the crashing document's up to the
/// reference that crashes; here an explicit `:numref:` title with a
/// positional field, which `str.format` cannot fill (`IndexError`, which
/// `_resolve_numref_xref` does not catch). Probed on Sphinx 9.1.0.
#[test]
fn a_crash_at_resolve_time_prints_the_read_records_first() {
    let table = ".. _tbl:\n\n.. table:: Cap\n\n   ===  ===\n   a    b\n   ===  ===\n\n";
    let b = format!(
        "B\n=\n\nFirst :nope:`y` and :ref:`nolabel-b1`.\n\n{table}\
         See :numref:`Tab {{0}} {{number}} <tbl>` and :ref:`nolabel-b2`.\n"
    );
    let files = [
        (
            "conf.py",
            "project = 'Temp'\nhtml_logo = 'missing.png'\nnumfig = True\n",
        ),
        (
            "index.rst",
            "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   c\n",
        ),
        ("a.rst", "A\n=\n\nSee :nope:`x` and :ref:`nolabel-a`.\n"),
        ("b.rst", b.as_str()),
        ("c.rst", "C\n=\n\nAlso :nope:`w` and :ref:`nolabel-c`.\n"),
        ("orphan.rst", "Orphan\n======\n\nText.\n"),
    ];
    assert_eq!(
        crash_stream("t17a2-stream-resolve", &files, &[], &[]),
        format!(
            "\
WARNING: logo file 'missing.png' does not exist
<SRCDIR>/a.rst:4: ERROR: Unknown interpreted text role \"nope\". [docutils]
<SRCDIR>/b.rst:4: ERROR: Unknown interpreted text role \"nope\". [docutils]
<SRCDIR>/c.rst:4: ERROR: Unknown interpreted text role \"nope\". [docutils]
<SRCDIR>/orphan.rst: WARNING: document isn't included in any toctree [toc.not_included]
<SRCDIR>/a.rst:4: WARNING: undefined label: 'nolabel-a' [ref.ref]
<SRCDIR>/b.rst:4: WARNING: undefined label: 'nolabel-b1' [ref.ref]
{}",
            crash_report(
                None,
                "IndexError: Replacement index 0 out of range for positional args tuple",
                Some("<SRCDIR>/b.rst:14")
            )
        )
    );
}

/// Decision 13, crash input 1: `HandleCodeBlocks` (210) replaces an empty
/// block quote by its no children, and docutils' `replace_self([])`
/// asserts that no basic attribute is lost, so a quote with a class whose
/// content makes no node crashes Sphinx 9.1.0 there, where the quote is.
fn assert_empty_quote_exits_two(test: &str, index: &str, classes: &str) {
    assert_eq!(
        crash_stream(test, &[("index.rst", index)], &[], &[]),
        crash_report(
            None,
            &format!("AssertionError: Losing \"classes\" attribute: {classes}"),
            Some("<SRCDIR>/index.rst:5")
        )
    );
}

/// Crash input 1: an `epigraph` whose content parses to nothing.
#[test]
fn empty_epigraph_exits_two() {
    assert_empty_quote_exits_two(
        "t17a2-crash-epigraph",
        "Para.\n\n.. epigraph::\n\n   .. default-domain:: py\n",
        "['epigraph']",
    );
}

/// Crash input 1: a `highlights` whose content parses to nothing.
#[test]
fn empty_highlights_exits_two() {
    assert_empty_quote_exits_two(
        "t17a2-crash-highlights",
        "Para.\n\n.. highlights::\n\n   .. default-domain:: py\n",
        "['highlights']",
    );
}

/// Crash input 1: a `pull-quote` whose content parses to nothing.
#[test]
fn empty_pull_quote_exits_two() {
    assert_empty_quote_exits_two(
        "t17a2-crash-pull-quote",
        "Para.\n\n.. pull-quote::\n\n   .. default-domain:: py\n",
        "['pull-quote']",
    );
}

/// Crash input 1: `.. rst-class::` gives its classes to each node its
/// content makes, an indented block that parses to nothing among them.
#[test]
fn empty_quote_under_rst_class_exits_two() {
    assert_empty_quote_exits_two(
        "t17a2-crash-rst-class",
        ".. rst-class:: foo bar\n\n   Para.\n\n      .. default-domain:: py\n",
        "['foo', 'bar']",
    );
}

/// Decision 13, crash input 2: `Substitutions` (220) looks a nested
/// reference's name up with `normed[name.lower()]`, which raises
/// `KeyError` when no definition has it, while the copy of the reference
/// that holds it is made.
#[test]
fn undefined_nested_substitution_exits_two() {
    assert_eq!(
        crash_stream(
            "t17a2-crash-subst-undefined",
            &[(
                "index.rst",
                "Use |a| here.\n\n.. |a| replace:: x |Nope| y\n"
            )],
            &[],
            &[],
        ),
        crash_report(None, "KeyError: 'nope'", Some("<SRCDIR>/index.rst:1"))
    );
}

/// Crash input 2: a definition found circular a second time, after the
/// first message took its place, is replaced again: `replace_self` looks
/// it up among its former parent's children (`ValueError`, naming it by
/// docutils' `repr`), after the second message is logged.
#[test]
fn circular_substitution_reported_twice_exits_two() {
    let detected = "<SRCDIR>/index.rst:3: ERROR: Circular substitution definition detected:\n\n\
                    .. |c| replace:: |c| |c| [docutils]\n";
    assert_eq!(
        crash_stream(
            "t17a2-crash-subst-twice",
            &[("index.rst", "P0 |c| q.\n\n.. |c| replace:: |c| |c|\n")],
            &[],
            &[],
        ),
        format!(
            "<SRCDIR>/index.rst:1: ERROR: Circular substitution definition referenced: \"c\". \
             [docutils]\n{detected}{detected}{}",
            crash_report(
                None,
                "ValueError: <substitution_definition \"c\": <substitution_reference...><#text: \
                 ' '><substitution_ref ...> is not in list",
                Some("<SRCDIR>/index.rst:3")
            )
        )
    );
}

/// Crash input 2: a reference left in a copy that a circular reference
/// stopped before it was placed is reached by the loop all the same, and
/// when its own copy is circular, the copy it stands in is replaced by the
/// message: that copy has no parent (`AttributeError`).
#[test]
fn circular_substitution_in_a_dropped_copy_exits_two() {
    let detected = |line: u32, definition: &str| {
        format!(
            "<SRCDIR>/index.rst:{line}: ERROR: Circular substitution definition detected:\n\n\
             {definition} [docutils]\n"
        )
    };
    let referenced = |name: &str, line: u32| {
        format!(
            "<SRCDIR>/index.rst:{line}: ERROR: Circular substitution definition referenced: \
             \"{name}\". [docutils]\n"
        )
    };
    let a = ".. |a| replace:: |b| |a|";
    assert_eq!(
        crash_stream(
            "t17a2-crash-subst-dropped",
            &[(
                "index.rst",
                "P0 |a| q.\n\nP1 |a| q.\n\n.. |a| replace:: |b| |a|\n.. |b| replace:: |b|\n",
            )],
            &[],
            &[],
        ),
        format!(
            "{}{}{}{}{}{}{}",
            referenced("a", 3),
            detected(5, a),
            detected(6, ".. |b| replace:: |b|"),
            referenced("b", 1),
            referenced("a", 1),
            detected(5, a),
            crash_report(
                None,
                "AttributeError: 'NoneType' object has no attribute 'replace'",
                Some("<SRCDIR>/index.rst:5")
            )
        )
    );
}

/// Decision 13, crash input 3: `today_fmt`'s `%U` and `%W` map to babel's
/// `WW`, which babel rejects. Sphinx's `babel_format_date` takes the
/// `ValueError` for an unknown locale, warns `Invalid Babel locale`, and
/// tries `en`, which raises it again, where `|today|` is substituted
/// (`DefaultSubstitutions`, 210).
#[test]
fn today_fmt_week_number_exits_two() {
    for (test, today_fmt) in [
        ("t17a2-crash-today-fmt-u", "%U"),
        ("t17a2-crash-today-fmt-w", "%Y week %W"),
    ] {
        let conf = format!("project = 'Temp'\ntoday_fmt = '{today_fmt}'\n");
        assert_eq!(
            crash_stream(
                test,
                &[
                    ("conf.py", conf.as_str()),
                    ("index.rst", "Index\n=====\n\nToday |today|.\n"),
                ],
                &[],
                &[],
            ),
            format!(
                "WARNING: Invalid Babel locale: 'en'. [i18n.babel]\n{}",
                crash_report(
                    None,
                    "ValueError: Invalid length for field: 'WW'",
                    Some("<SRCDIR>/index.rst:4")
                )
            ),
            "{today_fmt}"
        );
    }
}

/// Decision 13, crash input 4: Sphinx 9.1.0 reads `SOURCE_DATE_EPOCH`
/// three times, and each can crash it. `sphinx.builders.gettext`, which
/// Sphinx loads before it logs anything about the configuration, takes
/// `time.gmtime(float(value))` when it is imported; `correct_copyright_year`,
/// a `config-inited` handler, `int(value)`, which Sphinx reports as an
/// `ExtensionError` naming the handler (its address in memory, which no
/// build can repeat, is elided), after the records of the handlers before
/// it; and `format_date`, where `|today|` is substituted,
/// `datetime.fromtimestamp(float(value))`, which holds only years 1 to
/// 9999. Probed on Sphinx 9.1.0.
#[test]
fn unreadable_source_date_epoch_exits_two() {
    let conf = ("conf.py", "project = 'Temp'\nhtml_logo = 'missing.png'\n");
    let index = ("index.rst", "Index\n=====\n\nToday :nope:`x` |today|.\n");
    let logo = "WARNING: logo file 'missing.png' does not exist\n";
    let role = "<SRCDIR>/index.rst:4: ERROR: Unknown interpreted text role \"nope\". [docutils]\n";
    // 4,301 digits, which `float()` reads as 1.
    let over_the_int_limit = format!("{}1", "0".repeat(4300));
    for (test, epoch, expected) in [
        (
            "t17a2-crash-epoch-float",
            "soon",
            crash_report(
                None,
                "ValueError: could not convert string to float: 'soon'",
                None,
            ),
        ),
        (
            "t17a2-crash-epoch-int",
            "1.5",
            format!(
                "{logo}{}",
                crash_report(
                    Some("Extension error (sphinx.config)"),
                    "sphinx.errors.ExtensionError: Handler <function correct_copyright_year at \
                     0x...> for event 'config-inited' threw an exception (exception: invalid \
                     literal for int() with base 10: '1.5')",
                    None,
                )
            ),
        ),
        (
            "t17a2-crash-epoch-year",
            "300000000000",
            format!(
                "{logo}{role}{}",
                crash_report(
                    None,
                    "ValueError: year 11476 is out of range",
                    Some("<SRCDIR>/index.rst:4"),
                )
            ),
        ),
        // `float()` and `int()` read a decimal digit of any script
        // (ARABIC-INDIC here) and strip ASCII whitespace alone, not the C0
        // separators `str.isspace` also admits.
        (
            "t17a2-crash-epoch-int-digits",
            "\u{661}.\u{665}",
            format!(
                "{logo}{}",
                crash_report(
                    Some("Extension error (sphinx.config)"),
                    "sphinx.errors.ExtensionError: Handler <function correct_copyright_year at \
                     0x...> for event 'config-inited' threw an exception (exception: invalid \
                     literal for int() with base 10: '\u{661}.\u{665}')",
                    None,
                )
            ),
        ),
        (
            "t17a2-crash-epoch-year-digits",
            "\u{663}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}",
            format!(
                "{logo}{role}{}",
                crash_report(
                    None,
                    "ValueError: year 11476 is out of range",
                    Some("<SRCDIR>/index.rst:4"),
                )
            ),
        ),
        (
            "t17a2-crash-epoch-separator",
            "\x1c1700000000",
            crash_report(
                None,
                "ValueError: could not convert string to float: '\\x1c1700000000'",
                None,
            ),
        ),
        // `int()` reads no more than 4,300 digits, leading zeros among
        // them, where `float()` reads any number.
        (
            "t17a2-crash-epoch-int-limit",
            over_the_int_limit.as_str(),
            format!(
                "{logo}{}",
                crash_report(
                    Some("Extension error (sphinx.config)"),
                    "sphinx.errors.ExtensionError: Handler <function correct_copyright_year at \
                     0x...> for event 'config-inited' threw an exception (exception: Exceeds the \
                     limit (4300 digits) for integer string conversion: value has 4301 digits; use \
                     sys.set_int_max_str_digits() to increase the limit)",
                    None,
                )
            ),
        ),
    ] {
        assert_eq!(
            crash_stream(test, &[conf, index], &[("SOURCE_DATE_EPOCH", epoch)], &[]),
            expected,
            "{test}"
        );
    }
}

/// Python's `float()` and `int()` read a decimal digit of any script as
/// its ASCII digit, and whitespace past ASCII as a space (CPython's
/// `_PyUnicode_TransformDecimalAndSpaceToASCII`), so Sphinx 9.1.0 reads a
/// `SOURCE_DATE_EPOCH` of ARABIC-INDIC or FULLWIDTH digits, grouped or
/// not, or one between a NO-BREAK SPACE and an IDEOGRAPHIC SPACE, as it
/// reads `1700000000`: the build succeeds with no record, and `|today|`
/// is that day. Probed on Sphinx 9.1.0 (`Built on20231114`); the pages
/// are 5b's, so the date is read from the search index.
#[test]
fn a_source_date_epoch_of_any_decimal_digits_dates_the_build() {
    let src = temp_source(
        "t17a2-epoch-digits",
        &[
            ("conf.py", "project = 'Temp'\ntoday_fmt = 'on%Y%m%d'\n"),
            ("index.rst", "Index\n=====\n\nBuilt |today|.\n"),
        ],
    );
    for (name, epoch) in [
        (
            "arabic",
            "\u{661}\u{667}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}\u{660}",
        ),
        (
            "fullwidth",
            "\u{ff11}\u{ff17}\u{ff10}\u{ff10}\u{ff10}\u{ff10}\u{ff10}\u{ff10}\u{ff10}\u{ff10}",
        ),
        (
            "grouped",
            "\u{661}_\u{667}\u{660}\u{660}_\u{660}\u{660}\u{660}_\u{660}\u{660}\u{660}",
        ),
        ("spaced", "\u{a0}1700000000\u{3000}"),
    ] {
        let out = out_dir(&format!("t17a2-epoch-digits-{name}"));
        let log = out.with_extension("w");
        let _ = std::fs::remove_file(&log);
        let result = bin()
            .arg(&src)
            .arg(&out)
            .args(["-q", "-w"])
            .arg(&log)
            .env("SOURCE_DATE_EPOCH", epoch)
            .output()
            .expect("binary should run");
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "", "{name}");
        let terms = search_value(&out)["terms"].clone();
        assert!(
            terms.get("on20231114").is_some(),
            "{name}: no term on20231114 in {terms}"
        );
    }
}

/// Decision 13, crash input 4, the configuration's records: those Sphinx
/// logs before `correct_copyright_year` raises — the ignored `-D`
/// override, `deprecate_source_encoding` (790) and the HTML builder's logo
/// check (800, registered before `sphinx.config`'s handlers) — print
/// before the report, and the type and `primary_domain` checks that would
/// follow it do not. Probed on Sphinx 9.1.0.
#[test]
fn a_configuration_crash_prints_the_records_before_it() {
    assert_eq!(
        crash_stream(
            "t17a2-crash-config-records",
            &[
                (
                    "conf.py",
                    "project = 'Temp'\nhtml_logo = 'missing.png'\nsmartquotes = 'yes'\n\
                     primary_domain = 'nope'\nsource_encoding = 'latin-1'\n",
                ),
                ("index.rst", "Index\n=====\n\nText.\n"),
            ],
            &[("SOURCE_DATE_EPOCH", "1.5")],
            &["-D", "nonexistent_key=1"],
        ),
        format!(
            "\
WARNING: unknown config value 'nonexistent_key' in override, ignoring
WARNING: Support for source encodings other than UTF-8 is deprecated and will be removed in \
             Sphinx 10. Please comment at https://github.com/sphinx-doc/sphinx/issues/13665 if \
             this causes a problem.
WARNING: logo file 'missing.png' does not exist
{}",
            crash_report(
                Some("Extension error (sphinx.config)"),
                "sphinx.errors.ExtensionError: Handler <function correct_copyright_year at 0x...> \
                 for event 'config-inited' threw an exception (exception: invalid literal for \
                 int() with base 10: '1.5')",
                None,
            )
        )
    );
}

/// The configuration of [`configuration_records_follow_the_handler_order`]:
/// one value for each `config-inited` check that warns.
const CONFIG_RECORDS_CONF: &str = "project = 'Temp'\nhtml_logo = 'missing.png'\n\
                                   smartquotes = 'yes'\nprimary_domain = 'nope'\n\
                                   source_encoding = 'latin-1'\n";

/// The records Sphinx 9.1.0 logs for [`CONFIG_RECORDS_CONF`] up to
/// `correct_copyright_year`: `deprecate_source_encoding` (790), then the
/// HTML builder's logo check (800, `sphinx.builders.html` registers it
/// before `sphinx.config` registers its handlers).
const CONFIG_RECORDS_BEFORE_THE_COPYRIGHT_YEAR: &str = "\
WARNING: Support for source encodings other than UTF-8 is deprecated and will be removed in \
Sphinx 10. Please comment at https://github.com/sphinx-doc/sphinx/issues/13665 if this causes a \
problem.
WARNING: logo file 'missing.png' does not exist
";

/// The `config-inited` records in Sphinx's handler order (priority, then
/// registration), whatever the order of conf.py: `deprecate_source_encoding`
/// (790), the HTML builder's logo check (800), then, after
/// `correct_copyright_year`, `sphinx.config`'s `check_confval_types` and
/// `check_primary_domain` (800). Probed on Sphinx 9.1.0, which exits 0.
#[test]
fn configuration_records_follow_the_handler_order() {
    let src = temp_source(
        "t17a2-config-order",
        &[
            ("conf.py", CONFIG_RECORDS_CONF),
            ("index.rst", "Index\n=====\n\nText.\n"),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir("t17a2-config-order"), &[]);
    assert!(result.status.success(), "{}", stderr_of(&result));
    assert_eq!(
        written,
        format!(
            "{CONFIG_RECORDS_BEFORE_THE_COPYRIGHT_YEAR}\
             WARNING: The config value `smartquotes' has type `str'; expected `bool'.\n\
             WARNING: primary_domain 'nope' not found, ignored.\n"
        )
    );
}

/// A native build of `files` with `env` set that Sphinx 9.1.0 would crash
/// on (decision 13): it exits 2, and its `-w` file, returned with the
/// source directory spelled `<SRCDIR>/`, holds the stream sphinx-build
/// mode prints.
fn native_crash_stream(test: &str, files: &[(&str, &str)], env: &[(&str, &str)]) -> String {
    let src = temp_source(test, files);
    let src = sphinx_ultra::utils::canonicalize_simplified(&src).unwrap();
    let out = out_dir(test);
    let log = out.with_extension("w");
    let _ = std::fs::remove_file(&log);
    let mut command = bin();
    command
        .arg("build")
        .arg("--source")
        .arg(&src)
        .arg("--output")
        .arg(&out)
        .arg("-w")
        .arg(&log);
    for (key, value) in env {
        command.env(key, value);
    }
    let result = command.output().expect("binary should run");
    assert_eq!(
        result.status.code(),
        Some(2),
        "{test}: {}",
        stderr_of(&result)
    );
    srcdir_spelled(&std::fs::read(&log).unwrap(), &src)
}

/// The native subcommand writes a `config-inited` crash's stream to its
/// `-w` file as sphinx-build mode prints it: the configuration's records
/// logged before `correct_copyright_year` raises, which it otherwise
/// writes once the build is done, then the report; and it exits 2.
#[test]
fn a_native_configuration_crash_writes_the_records_before_it() {
    assert_eq!(
        native_crash_stream(
            "t17a2-native-config-crash",
            &[
                ("conf.py", CONFIG_RECORDS_CONF),
                ("index.rst", "Index\n=====\n\nText.\n"),
            ],
            &[("SOURCE_DATE_EPOCH", "1.5")],
        ),
        format!(
            "{CONFIG_RECORDS_BEFORE_THE_COPYRIGHT_YEAR}{}",
            crash_report(
                Some("Extension error (sphinx.config)"),
                "sphinx.errors.ExtensionError: Handler <function correct_copyright_year at 0x...> \
                 for event 'config-inited' threw an exception (exception: invalid literal for \
                 int() with base 10: '1.5')",
                None,
            )
        )
    );
}

/// The native subcommand crashes on a `SOURCE_DATE_EPOCH` `float()`
/// rejects where Sphinx 9.1.0 does, while it sets its application up,
/// before it logs anything about the configuration: the `-w` file holds
/// the report alone, and the build exits 2.
#[test]
fn a_native_build_crashes_on_an_unreadable_source_date_epoch_first() {
    assert_eq!(
        native_crash_stream(
            "t17a2-native-epoch-crash",
            &[
                ("conf.py", CONFIG_RECORDS_CONF),
                ("index.rst", "Index\n=====\n\nText.\n"),
            ],
            &[("SOURCE_DATE_EPOCH", "soon")],
        ),
        crash_report(
            None,
            "ValueError: could not convert string to float: 'soon'",
            None,
        )
    );
}

/// Decision 13, crash input 5: `DocInfo` (340) words its warning about a
/// bibliographic field that is not one paragraph from the body's children
/// other than system messages, and `content[0]` of a body holding only
/// messages raises `IndexError`.
#[test]
fn bibliographic_field_of_messages_exits_two() {
    assert_eq!(
        crash_stream(
            "t17a2-crash-docinfo",
            &[("index.rst", ":Author: .. bogus::\n\nText.\n")],
            &[],
            &[],
        ),
        format!(
            "<SRCDIR>/index.rst:1: ERROR: Unknown directive type \"bogus\".\n\n.. bogus:: \
             [docutils]\n{}",
            crash_report(
                None,
                "IndexError: list index out of range",
                Some("<SRCDIR>/index.rst:1")
            )
        )
    );
}

/// Decision 13, crash input 6: with `numfig` on, `MathDomain.resolve_xref`
/// reads an equation's number from `env.toc_fignumbers[docname]
/// ['displaymath']`, which a numbered document whose figures the toctree
/// numbered but which holds no numbered equation lacks (`KeyError`): here
/// the equation's block went with a list-table its parse rejected, while
/// its label stayed registered. Probed on Sphinx 9.1.0.
#[test]
fn numfig_without_displaymath_numbers_exits_two() {
    let list_table = ".. list-table::\n\n   * - .. math:: x\n          :label: e1\n   \
                      * - text\n     - more\n\n";
    let index = format!(
        "Index\n=====\n\n.. figure:: pic.png\n\n   Caption.\n\n{list_table}See :eq:`e1`.\n"
    );
    assert_eq!(
        crash_stream(
            "t17a2-crash-displaymath",
            &[
                ("conf.py", "project = 'Temp'\nnumfig = True\n"),
                ("index.rst", index.as_str()),
            ],
            &[],
            &[],
        ),
        format!(
            "<SRCDIR>/index.rst:8: ERROR: Error parsing content block for the \"list-table\" \
             directive: uniform two-level bullet list expected, but row 2 does not contain the \
             same number of items as row 1 (2 vs 1).\n\n{}[docutils]\n\
             <SRCDIR>/index.rst:4: WARNING: image file not readable: pic.png \
             [image.not_readable]\n{}",
            list_table.trim_end().to_string() + " ",
            crash_report(
                None,
                "KeyError: 'displaymath'",
                Some("<SRCDIR>/index.rst:15")
            )
        )
    );
}

/// Decision 13, crash input 7: the `index` directive hands each entry to
/// `process_index_entry`, which raises `ValueError` for an entry type it
/// no longer supports, as for the role (`index_role_removed_entry_type_exits_two`),
/// at parse time: nothing the document holds after it prints.
#[test]
fn index_directive_removed_entry_type_exits_two() {
    assert_eq!(
        crash_stream(
            "t17a2-crash-index-directive",
            &[(
                "index.rst",
                "Title\n=====\n\n.. index::\n   single: ok\n   module: os\n\nText :nope:`x`.\n",
            )],
            &[],
            &[],
        ),
        crash_report(
            None,
            "ValueError: 'module' is no longer supported for index entries (from entry 'module: \
             os'). Use 'pair: module; os' instead.",
            Some("<SRCDIR>/index.rst:4")
        )
    );
}

/// Decision 13, crash input 8: a new-style format that Python's
/// `str.format` cannot parse (`ValueError`) or fill (`IndexError` for a
/// positional field) in `math_eqref_format`, in `numfig_format` or in an
/// explicit `:numref:` title (`a_crash_at_resolve_time_prints_the_read_records_first`):
/// `MathDomain` catches only `KeyError` and `StandardDomain` `KeyError`
/// and `TypeError`, so Sphinx 9.1.0 crashes where the reference is
/// resolved. A doubled brace is a literal one, so `{{x}` ends on a single
/// `}`. The rest of a field is looked up on the string its keyword gives
/// (`a_format_field_indexes_a_string_as_python_does`): an empty attribute
/// or anything but `.` or `[` after a `]` is a `ValueError`, an index past
/// the string's end an `IndexError`, an attribute `str` lacks an
/// `AttributeError` (with the suggestion Python's traceback adds), and a
/// name in brackets a `TypeError`, which `MathDomain` does not catch.
/// Python reads a decimal digit of any script as a digit, in a positional
/// field and in an index alike. Probed on Sphinx 9.1.0.
#[test]
fn invalid_new_style_format_exits_two() {
    let equation = "Index\n=====\n\n.. math:: x\n   :label: e1\n\nSee :eq:`e1`.\n";
    let any_equation = equation.replace(":eq:", ":any:");
    let math_numref = equation.replace(":eq:", ":math:numref:");
    let table = "Index\n=====\n\n.. _tbl:\n\n.. table:: Cap\n\n   ===  ===\n   a    b\n   \
                 ===  ===\n\nSee :numref:`tbl`.\n";
    let titled = table.replace(":numref:`tbl`", ":numref:`Tab {name[3]} {number} <tbl>`");
    // A decimal digit of any script is a digit (ARABIC-INDIC DIGIT THREE,
    // twenty ARABIC-INDIC DIGIT NINEs).
    let nines = "\u{669}".repeat(20);
    let eqref_many_digits = format!("math_eqref_format = 'Eq {{number[{nines}]}}'\n");
    let titled_many_digits = table.replace(
        ":numref:`tbl`",
        &format!(":numref:`Tab {{name[{nines}]}} {{number}} <tbl>`"),
    );
    let titled_positional =
        table.replace(":numref:`tbl`", ":numref:`Tab {\u{663}} {number} <tbl>`");
    for (test, conf, index, exception, line) in [
        (
            "t17a2-crash-eqref-positional",
            "math_eqref_format = 'Eq {0}'\n",
            equation,
            "IndexError: Replacement index 0 out of range for positional args tuple",
            7,
        ),
        (
            "t17a2-crash-eqref-single-close",
            "math_eqref_format = 'Eq }'\n",
            equation,
            "ValueError: Single '}' encountered in format string",
            7,
        ),
        (
            "t17a2-crash-eqref-doubled-open",
            "math_eqref_format = '{{x} {number}'\n",
            equation,
            "ValueError: Single '}' encountered in format string",
            7,
        ),
        (
            "t17a2-crash-numfig-format-unbalanced",
            "numfig = True\nnumfig_format = {'table': 'Table {number'}\n",
            table,
            "ValueError: expected '}' before end of string",
            12,
        ),
        // `MathDomain.resolve_any_xref` is the domain's `resolve_xref`, and
        // `:math:numref:` is the domain's other role.
        (
            "t17a2-crash-eqref-any",
            "math_eqref_format = 'Eq {0}'\n",
            any_equation.as_str(),
            "IndexError: Replacement index 0 out of range for positional args tuple",
            7,
        ),
        (
            "t17a2-crash-eqref-math-numref",
            "math_eqref_format = 'Eq }'\n",
            math_numref.as_str(),
            "ValueError: Single '}' encountered in format string",
            7,
        ),
        (
            "t17a2-crash-eqref-empty-attribute",
            "math_eqref_format = 'Eq {number.}'\n",
            equation,
            "ValueError: Empty attribute in format string",
            7,
        ),
        (
            "t17a2-crash-eqref-after-index",
            "math_eqref_format = 'Eq {number[0]x}'\n",
            equation,
            "ValueError: Only '.' or '[' may follow ']' in format field specifier",
            7,
        ),
        (
            "t17a2-crash-eqref-index-range",
            "math_eqref_format = 'Eq {number[5]}'\n",
            equation,
            "IndexError: string index out of range",
            7,
        ),
        (
            "t17a2-crash-eqref-attribute",
            "math_eqref_format = 'Eq {number.uper}'\n",
            equation,
            "AttributeError: 'str' object has no attribute 'uper'. Did you mean: 'upper'?",
            7,
        ),
        (
            "t17a2-crash-eqref-string-index",
            "math_eqref_format = 'Eq {number[x]}'\n",
            any_equation.as_str(),
            "TypeError: string indices must be integers, not 'str'",
            7,
        ),
        (
            "t17a2-crash-numfig-format-empty-attribute",
            "numfig = True\nnumfig_format = {'table': 'Table {number.}'}\n",
            table,
            "ValueError: Empty attribute in format string",
            12,
        ),
        (
            "t17a2-crash-numref-title-index-range",
            "numfig = True\n",
            titled.as_str(),
            "IndexError: string index out of range",
            12,
        ),
        (
            "t17a2-crash-eqref-arabic-positional",
            "math_eqref_format = 'Eq {\u{663}}'\n",
            equation,
            "IndexError: Replacement index 3 out of range for positional args tuple",
            7,
        ),
        (
            "t17a2-crash-eqref-many-digits",
            eqref_many_digits.as_str(),
            equation,
            "ValueError: Too many decimal digits in format string",
            7,
        ),
        (
            "t17a2-crash-numref-title-many-digits",
            "numfig = True\n",
            titled_many_digits.as_str(),
            "ValueError: Too many decimal digits in format string",
            12,
        ),
        (
            "t17a2-crash-numref-title-arabic-positional",
            "numfig = True\n",
            titled_positional.as_str(),
            "IndexError: Replacement index 3 out of range for positional args tuple",
            12,
        ),
    ] {
        let conf = format!("project = 'Temp'\n{conf}");
        assert_eq!(
            crash_stream(
                test,
                &[("conf.py", conf.as_str()), ("index.rst", index)],
                &[],
                &[],
            ),
            crash_report(None, exception, Some(&format!("<SRCDIR>/index.rst:{line}"))),
            "{test}"
        );
    }
}

/// `str.format` looks up the rest of a field on the string its keyword
/// gives (CPython's `FieldNameIterator`): an index of digits is that
/// string's character, so `Eq {number[0]}` and `Tab {name[0]}{number[0]}`
/// fill without a record, as do their digits of another script (here
/// ARABIC-INDIC DIGIT ZERO and ONE), which Python reads as decimal digits;
/// a name in brackets is a `TypeError`, which `_resolve_numref_xref`
/// catches and warns about without the exception (`invalid numfig_format:
/// <title>`), where `MathDomain` crashes
/// (`invalid_new_style_format_exits_two`). Sphinx 9.1.0's streams, probed
/// on each project.
#[test]
fn a_format_field_indexes_a_string_as_python_does() {
    let equation = "Index\n=====\n\n.. math:: x\n   :label: e1\n\nSee :eq:`e1`.\n";
    let titled = |title: &str| {
        format!(
            "Index\n=====\n\n.. _tbl:\n\n.. table:: Cap\n\n   ===  ===\n   a    b\n   ===  \
             ===\n\nSee :numref:`{title} <tbl>`.\n"
        )
    };
    for (test, conf, index, expected) in [
        (
            "t17a2-eqref-index",
            "math_eqref_format = 'Eq {number[0]}'\n",
            equation.to_string(),
            "",
        ),
        (
            "t17a2-numref-title-index",
            "numfig = True\n",
            titled("Tab {name[0]}{number[0]}"),
            "",
        ),
        (
            "t17a2-numref-title-string-index",
            "numfig = True\n",
            titled("Tab {name[x]} {number}"),
            "<SRCDIR>/index.rst:12: WARNING: invalid numfig_format: Tab {name[x]} {number}\n",
        ),
        (
            "t17a2-eqref-arabic-index",
            "math_eqref_format = 'Eq {number[\u{660}]}'\n",
            equation.to_string(),
            "",
        ),
        (
            "t17a2-numref-title-arabic-index",
            "numfig = True\n",
            titled("Tab {name[\u{661}]}{number[\u{660}]}"),
            "",
        ),
    ] {
        let conf = format!("project = 'Temp'\n{conf}");
        let src = temp_source(test, &[("conf.py", conf.as_str()), ("index.rst", &index)]);
        let (result, written) = warning_stream(&src, &out_dir(test), &[]);
        assert!(result.status.success(), "{test}: {}", stderr_of(&result));
        assert_eq!(written, expected, "{test}");
    }
}

/// Decision 13, crash input 9, the HTML oracle's reference crash
/// `docutils-0500`: a figure whose `:figname:` registered the name before
/// Sphinx's `Figure` gives it `:name:` and registers its names again, so
/// `set_duplicate_name_id` finds the figure's own name taken by the figure
/// and `dupname` removes it from the figure twice (`ValueError`), before
/// any message is made. The build's stream opens with Sphinx's own, which
/// the oracle recorded with its exit status.
#[test]
fn figure_name_after_figname_exits_two() {
    let input = fixture("html_oracle/core/inputs/docutils_snippets/docutils-0500");
    let files: Vec<(String, String)> = ["conf.py", "index.rst"]
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                std::fs::read_to_string(input.join(name)).unwrap(),
            )
        })
        .collect();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let index: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("html_oracle/core/index.json")).unwrap(),
    )
    .unwrap();
    let case = index["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["case_id"] == "docutils-0500")
        .expect("the oracle records docutils-0500");
    assert_eq!(case["status"], "reference-crash");
    assert_eq!(case["exit_code"], 2);
    let written = crash_stream("t17a2-crash-figname", &files, &[], &[]);
    assert_eq!(
        written,
        crash_report(
            None,
            "ValueError: list.remove(x): x not in list",
            Some("<SRCDIR>/index.rst:1")
        )
    );
    assert!(
        written.starts_with(case["warnings"].as_str().unwrap()),
        "{written:?} opens with the oracle's stream"
    );
}

/// Decision 13, crash input 9's variant: a figure whose `:name:` is its
/// `:figname:` carries the name twice when Sphinx's `Figure` registers
/// its names again. docutils' `dupname` removes one occurrence per call,
/// so the first duplicate's `dupname`s of the figure as the new node and
/// as the old one both succeed and its `Duplicate explicit target name`
/// warning is logged; the second duplicate's `dupname` finds the name
/// gone and raises `ValueError`. So the build prints that warning, after
/// the document's records before it and before the report; the earlier
/// implicit target a section title made does not change it. Probed on
/// Sphinx 9.1.0.
#[test]
fn figure_named_as_its_figname_exits_two() {
    let figure = ".. figure:: pic.png\n   :figname: x\n   :name: x\n\n   Caption.\n";
    let duplicate = |line: u32| {
        format!(
            "<SRCDIR>/index.rst:{line}: WARNING: Duplicate explicit target name: \"x\". \
             [docutils]\n"
        )
    };
    let crash = |line: u32| {
        crash_report(
            None,
            "ValueError: list.remove(x): x not in list",
            Some(&format!("<SRCDIR>/index.rst:{line}")),
        )
    };
    for (test, index, expected) in [
        (
            "t17a2-crash-figname-twice",
            figure.to_string(),
            format!("{}{}", duplicate(1), crash(1)),
        ),
        (
            "t17a2-crash-figname-twice-later",
            format!("Title\n=====\n\nText :nope:`a`.\n\n{figure}\nLater :nope:`b`.\n"),
            format!(
                "<SRCDIR>/index.rst:4: ERROR: Unknown interpreted text role \"nope\". \
                 [docutils]\n{}{}",
                duplicate(6),
                crash(6)
            ),
        ),
        (
            "t17a2-crash-figname-twice-section",
            format!("x\n=\n\nText.\n\n{figure}"),
            format!("{}{}", duplicate(6), crash(6)),
        ),
    ] {
        assert_eq!(
            crash_stream(test, &[("index.rst", index.as_str())], &[], &[]),
            expected,
            "{test}"
        );
    }
}

/// The project of the Task 17a3 math pins: one numbered equation and one
/// reference to it, at line 7.
const EQUATION: &str = "Index\n=====\n\n.. math:: x^2\n   :label: e1\n\nSee :eq:`e1`.\n";

/// The project of the Task 17a3 numref pins: one numbered table, labelled
/// `tbl`, and one reference to it, at line 12, written as `reference`.
fn numbered_table(reference: &str) -> String {
    format!(
        "Index\n=====\n\n.. _tbl:\n\n.. table:: Cap\n\n   ===  ===\n   a    b\n   ===  ===\n\n\
         See {reference}.\n"
    )
}

/// `MathDomain.resolve_xref` formats `math_eqref_format` with Python's
/// `str.format`, which renders a format spec (`str.__format__`) and a
/// conversion (`repr()`) before it: `Eq{number:x>4}` gives `Eqxxx1` and
/// `Eq{number!r}` gives `Eq'1'`, with no record. Sphinx 9.1.0's pages,
/// probed; here the text is read from the search index (the pages are
/// 5b's), whose terms hold `eqxxx1` and `eq` only when the format renders,
/// not `(1)`.
#[test]
fn math_eqref_format_renders_a_format_spec_and_a_conversion() {
    for (test, eqref_format, term) in [
        ("t17a3-eqref-spec", "Eq{number:x>4}", "eqxxx1"),
        ("t17a3-eqref-conversion", "Eq{number!r}", "eq"),
    ] {
        let conf = format!("project = 'Temp'\nmath_eqref_format = '{eqref_format}'\n");
        let src = temp_source(test, &[("conf.py", conf.as_str()), ("index.rst", EQUATION)]);
        let out = out_dir(test);
        let (result, written) = warning_stream(&src, &out, &[]);
        assert!(result.status.success(), "{test}: {}", stderr_of(&result));
        assert_eq!(written, "", "{test}");
        let terms = search_value(&out)["terms"].clone();
        assert!(
            terms.get(term).is_some(),
            "{test}: no term {term} in {terms}"
        );
    }
}

/// Decision 13: `str.format` raises `ValueError` for a format spec a
/// `str` does not take (`Unknown format code 'd'`) and for a conversion
/// other than `s`, `r` and `a`, which `MathDomain.resolve_xref` does not
/// catch: Sphinx 9.1.0 crashes where the reference is resolved. Probed.
#[test]
fn math_eqref_format_invalid_spec_or_conversion_exits_two() {
    for (test, eqref_format, exception) in [
        (
            "t17a3-crash-eqref-spec",
            "Eq {number:d}",
            "ValueError: Unknown format code 'd' for object of type 'str'",
        ),
        (
            "t17a3-crash-eqref-conversion",
            "Eq {number!z}",
            "ValueError: Unknown conversion specifier z",
        ),
    ] {
        let conf = format!("project = 'Temp'\nmath_eqref_format = '{eqref_format}'\n");
        assert_eq!(
            crash_stream(
                test,
                &[("conf.py", conf.as_str()), ("index.rst", EQUATION)],
                &[],
                &[],
            ),
            crash_report(None, exception, Some("<SRCDIR>/index.rst:7")),
            "{test}"
        );
    }
}

/// Decision 13: a `numfig_format` entry, or an explicit `:numref:` title,
/// that holds neither `{name}` nor `number` is applied with `%`
/// (`title % fignum`), and `_resolve_numref_xref` catches `KeyError` and
/// `TypeError` only. A format that ends in its `%` is `ValueError:
/// incomplete format`, and a conversion character `%` does not know is
/// `ValueError: unsupported format character`, after the argument is
/// taken: Sphinx 9.1.0 crashes where the reference is resolved. Probed.
#[test]
fn old_style_numfig_format_value_error_exits_two() {
    let plain = numbered_table(":numref:`tbl`");
    let titled = numbered_table(":numref:`Tab %z <tbl>`");
    for (test, conf, index, exception) in [
        (
            "t17a3-crash-numfig-format-incomplete",
            "numfig = True\nnumfig_format = {'table': 'Table %'}\n",
            plain.as_str(),
            "ValueError: incomplete format",
        ),
        (
            "t17a3-crash-numref-title-unsupported",
            "numfig = True\n",
            titled.as_str(),
            "ValueError: unsupported format character 'z' (0x7a) at index 5",
        ),
    ] {
        let conf = format!("project = 'Temp'\n{conf}");
        assert_eq!(
            crash_stream(
                test,
                &[("conf.py", conf.as_str()), ("index.rst", index)],
                &[],
                &[],
            ),
            crash_report(None, exception, Some("<SRCDIR>/index.rst:12")),
            "{test}"
        );
    }
}

/// `title % fignum` renders `%r` as `repr()` of the number, so an explicit
/// `:numref:` title `Tab%r` gives `Tab'1'` with no record, where a title
/// with no conversion is `TypeError: not all arguments converted`, which
/// `_resolve_numref_xref` catches: `invalid numfig_format: <title>`, and the
/// content node stays (its text `Tab%r` would index the term `r`). Sphinx
/// 9.1.0's streams and pages, probed.
#[test]
fn old_style_numref_titles_render_or_warn_as_python_formats_them() {
    for (test, title, expected) in [
        ("t17a3-numref-title-repr", "Tab%r", ""),
        (
            "t17a3-numref-title-no-conversion",
            "Figure 1",
            "<SRCDIR>/index.rst:12: WARNING: invalid numfig_format: Figure 1\n",
        ),
    ] {
        let index = numbered_table(&format!(":numref:`{title} <tbl>`"));
        let src = temp_source(
            test,
            &[
                ("conf.py", "project = 'Temp'\nnumfig = True\n"),
                ("index.rst", index.as_str()),
            ],
        );
        let out = out_dir(test);
        let (result, written) = warning_stream(&src, &out, &[]);
        assert!(result.status.success(), "{test}: {}", stderr_of(&result));
        assert_eq!(written, expected, "{test}");
        if expected.is_empty() {
            let terms = search_value(&out)["terms"].clone();
            assert!(terms.get("r").is_none(), "{test}: the title rendered");
        }
    }
}

/// Decision 13: `Manpage.run` builds its link with
/// `manpages_url.format_map(info)`, whose `section` is `None` for a page the
/// target names no section of (`ls.`). `None` takes no format spec
/// (`TypeError: unsupported format string passed to NoneType.__format__`),
/// which nothing catches: Sphinx 9.1.0 crashes at parse time. Probed.
#[test]
fn manpages_url_spec_on_a_missing_section_exits_two() {
    assert_crash_exits_two(
        "t17a3-crash-manpages-url-none-spec",
        &[
            (
                "conf.py",
                "manpages_url = 'https://man.example/{page}.{section:>3}'\n",
            ),
            ("index.rst", "Title\n=====\n\nSee :manpage:`ls.`.\n"),
        ],
        "TypeError: unsupported format string passed to NoneType.__format__",
        "<SRCDIR>/index.rst:4",
    );
}

/// `manpages_url.format_map(info)` renders a format spec on a `str` and a
/// conversion of the `None` section as Python does
/// (`https://man.example/ls____.'1'` and `https://man.example/ls____.None`,
/// pinned in the role's tree by `manpage_matches_sphinx`), with no record.
/// Probed on Sphinx 9.1.0.
#[test]
fn manpages_url_with_a_valid_spec_renders() {
    let test = "t17a3-manpages-url-spec";
    let src = temp_source(
        test,
        &[
            (
                "conf.py",
                "manpages_url = 'https://man.example/{page:_<6}.{section!r}'\n",
            ),
            (
                "index.rst",
                "Title\n=====\n\nSee :manpage:`ls(1)` and :manpage:`ls.`.\n",
            ),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir(test), &[]);
    assert!(result.status.success(), "{}", stderr_of(&result));
    assert_eq!(written, "");
}

/// Decision 13: Python formats the value of an attribute a string has
/// (`{page.upper}`, a bound method whose text names its address, which
/// sphinx-ultra does not model) and reads on, so a later field raises what
/// it raises: in `manpages_url`, a `KeyError` for a key `info` lacks and
/// the `ValueError` of a positional field, at parse time; in
/// `math_eqref_format`, a format spec a `str` does not take, at resolve
/// time. Sphinx 9.1.0 crashes on each. Probed.
#[test]
fn a_field_after_an_attribute_value_exits_two() {
    for (test, url, exception) in [
        (
            "t17a3-crash-manpages-url-after-attribute-key",
            "https://m/{page.upper}/{nope}",
            "KeyError: 'nope'",
        ),
        (
            "t17a3-crash-manpages-url-after-attribute-positional",
            "https://m/{page.upper}/{0}",
            "ValueError: Format string contains positional fields",
        ),
    ] {
        let conf = format!("manpages_url = '{url}'\n");
        assert_crash_exits_two(
            test,
            &[
                ("conf.py", conf.as_str()),
                ("index.rst", "Title\n=====\n\nSee :manpage:`ls(1)`.\n"),
            ],
            exception,
            "<SRCDIR>/index.rst:4",
        );
    }
    let test = "t17a3-crash-eqref-after-attribute";
    assert_eq!(
        crash_stream(
            test,
            &[
                (
                    "conf.py",
                    "project = 'Temp'\nmath_eqref_format = 'Eq {number.upper} {number:d}'\n",
                ),
                ("index.rst", EQUATION),
            ],
            &[],
            &[],
        ),
        crash_report(
            None,
            "ValueError: Unknown format code 'd' for object of type 'str'",
            Some("<SRCDIR>/index.rst:7")
        ),
        "{test}"
    );
}

/// Decision 13: after `!r` the value of an attribute a string has
/// (`{number.upper!r}`, whose text sphinx-ultra does not model) is a
/// `str`, so Python checks its format spec as `str.__format__` does,
/// whatever the text, and reads on. In `math_eqref_format`, a spec a `str`
/// does not take crashes Sphinx 9.1.0 where the reference is resolved, and
/// a later field that names no argument is the `KeyError` it warns about.
/// Probed.
#[test]
fn math_eqref_format_checks_a_spec_after_a_converted_attribute_value() {
    let test = "t17a3-crash-eqref-converted-attribute-spec";
    assert_eq!(
        crash_stream(
            test,
            &[
                (
                    "conf.py",
                    "project = 'Temp'\nmath_eqref_format = '{number.upper!r:d}'\n",
                ),
                ("index.rst", EQUATION),
            ],
            &[],
            &[],
        ),
        crash_report(
            None,
            "ValueError: Unknown format code 'd' for object of type 'str'",
            Some("<SRCDIR>/index.rst:7")
        ),
        "{test}"
    );
    let test = "t17a3-eqref-converted-attribute-then-key";
    let src = temp_source(
        test,
        &[
            (
                "conf.py",
                "project = 'Temp'\nmath_eqref_format = '{number.upper!r:>3} {nope}'\n",
            ),
            ("index.rst", EQUATION),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir(test), &[]);
    assert!(result.status.success(), "{test}: {}", stderr_of(&result));
    assert_eq!(
        written, "<SRCDIR>/index.rst:7: WARNING: Invalid math_eqref_format: KeyError('nope')\n",
        "{test}"
    );
}

/// docutils reads a `unicode` directive's character code with
/// `chr(int(code))`, or `chr(int(value, 16))` for a hexadecimal one, and
/// CPython 3.12's `chr` takes a C `int`: up to `2**31 - 1` a code past
/// 0x10FFFF is `ValueError: chr() arg not in range(0x110000)`, and past it
/// `OverflowError: Python int too large to convert to C int`, which
/// `unicode_code` words as its own `ValueError`, `code too large (...)`.
/// `csv-table`'s `:delim:` reads a code the same way. Sphinx 9.1.0's
/// stream, probed.
#[test]
fn character_codes_past_chr_range_read_as_python_words_them() {
    let test = "t17a3-unicode-code-chr-bounds";
    let index = "Title\n=====\n\n\
                 .. |c| unicode:: 2147483647\n.. |d| unicode:: 2147483648\n\
                 .. |e| unicode:: U+7FFFFFFF\n.. |f| unicode:: U+80000000\n\n\
                 .. csv-table::\n   :delim: 2147483648\n\n   a, b\n";
    let not_in_range = "ValueError: chr() arg not in range(0x110000)";
    let too_large = "code too large (Python int too large to convert to C int)";
    let src = temp_source(test, &[("index.rst", index)]);
    let (result, written) = warning_stream(&src, &out_dir(test), &[]);
    assert!(result.status.success(), "{}", stderr_of(&result));
    assert_eq!(
        written,
        format!(
            "<SRCDIR>/index.rst:4: ERROR: Invalid character code: 2147483647\n\
             {not_in_range} [docutils]\n\
             <SRCDIR>/index.rst:4: WARNING: Substitution definition \"c\" empty or invalid.\n\n\
             .. |c| unicode:: 2147483647 [docutils]\n\
             <SRCDIR>/index.rst:5: ERROR: Invalid character code: 2147483648\n\
             ValueError: {too_large} [docutils]\n\
             <SRCDIR>/index.rst:5: WARNING: Substitution definition \"d\" empty or invalid.\n\n\
             .. |d| unicode:: 2147483648 [docutils]\n\
             <SRCDIR>/index.rst:6: ERROR: Invalid character code: U+7FFFFFFF\n\
             {not_in_range} [docutils]\n\
             <SRCDIR>/index.rst:6: WARNING: Substitution definition \"e\" empty or invalid.\n\n\
             .. |e| unicode:: U+7FFFFFFF [docutils]\n\
             <SRCDIR>/index.rst:7: ERROR: Invalid character code: U+80000000\n\
             ValueError: {too_large} [docutils]\n\
             <SRCDIR>/index.rst:7: WARNING: Substitution definition \"f\" empty or invalid.\n\n\
             .. |f| unicode:: U+80000000 [docutils]\n\
             <SRCDIR>/index.rst:9: ERROR: Error in \"csv-table\" directive:\n\
             invalid option value: (option: \"delim\"; value: '2147483648')\n\
             {too_large}.\n\n\
             .. csv-table::\n   :delim: 2147483648\n\n   a, b [docutils]\n"
        )
    );
}

/// docutils reads `:header-rows:` with `int()`, which reads no more than
/// 4,300 digits, leading zeros among them: 4,300 zeros and a 1 is the
/// option error Python's message words, not a table of one header row.
/// Sphinx 9.1.0's stream, probed.
#[test]
fn a_directive_option_past_int_digit_limit_is_invalid() {
    let digits = format!("{}1", "0".repeat(4300));
    let index = format!("Title\n=====\n\n.. csv-table::\n   :header-rows: {digits}\n\n   a, b\n");
    let test = "t17a3-option-int-limit";
    let src = temp_source(test, &[("index.rst", index.as_str())]);
    let (result, written) = warning_stream(&src, &out_dir(test), &[]);
    assert!(result.status.success(), "{}", stderr_of(&result));
    assert_eq!(
        written,
        format!(
            "<SRCDIR>/index.rst:4: ERROR: Error in \"csv-table\" directive:\n\
             invalid option value: (option: \"header-rows\"; value: '{digits}')\n\
             Exceeds the limit (4300 digits) for integer string conversion: value has 4301 \
             digits; use sys.set_int_max_str_digits() to increase the limit.\n\n\
             .. csv-table::\n   :header-rows: {digits}\n\n   a, b [docutils]\n"
        )
    );
}

/// Every caller of `int()` reads Python's one: its message names a literal
/// it cannot read by its `repr()` cut to 200 characters (`%.200R`), and its
/// 4,300-digit limit holds for a `unicode` directive's character code,
/// which `code.isdigit()` lets through to `int()`, as for an option.
/// Sphinx 9.1.0's streams, probed.
#[test]
fn int_errors_read_as_python_words_them() {
    let xs = "x".repeat(250);
    let ones = "1".repeat(4301);
    for (test, index, expected) in [
        (
            "t17a3-option-int-literal",
            format!("Title\n=====\n\n.. csv-table::\n   :header-rows: {xs}\n\n   a, b\n"),
            format!(
                "<SRCDIR>/index.rst:4: ERROR: Error in \"csv-table\" directive:\n\
                 invalid option value: (option: \"header-rows\"; value: '{xs}')\n\
                 invalid literal for int() with base 10: '{}.\n\n\
                 .. csv-table::\n   :header-rows: {xs}\n\n   a, b [docutils]\n",
                "x".repeat(199)
            ),
        ),
        (
            "t17a3-unicode-code-int-limit",
            format!("Title\n=====\n\n.. |x| unicode:: {ones}\n\nText |x|.\n"),
            format!(
                "<SRCDIR>/index.rst:4: ERROR: Invalid character code: {ones}\n\
                 ValueError: Exceeds the limit (4300 digits) for integer string conversion: value \
                 has 4301 digits; use sys.set_int_max_str_digits() to increase the limit \
                 [docutils]\n\
                 <SRCDIR>/index.rst:4: WARNING: Substitution definition \"x\" empty or invalid.\n\n\
                 .. |x| unicode:: {ones} [docutils]\n\
                 <SRCDIR>/index.rst:6: ERROR: Undefined substitution referenced: \"x\". \
                 [docutils]\n"
            ),
        ),
    ] {
        let src = temp_source(test, &[("index.rst", index.as_str())]);
        let (result, written) = warning_stream(&src, &out_dir(test), &[]);
        assert!(result.status.success(), "{test}: {}", stderr_of(&result));
        assert_eq!(written, expected, "{test}");
    }
}

/// The `config:` hash of the `.buildinfo` the build into `out` wrote.
fn buildinfo_config_hash(out: &Path) -> String {
    std::fs::read_to_string(out.join(".buildinfo"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("config: "))
        .unwrap()
        .to_string()
}

/// A one-page project of `conf`, built in sphinx-build mode with `-q` and
/// `extra`: the output directory, after the build exited 0 and printed
/// nothing.
fn quiet_build(test: &str, conf: &str, extra: &[&str]) -> PathBuf {
    let src = temp_source(test, &[("conf.py", conf), ("index.rst", "Title\n=====\n")]);
    let out = out_dir(test);
    let mut args = vec![
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-b",
        "html",
        "-q",
    ];
    args.extend_from_slice(extra);
    let result = sphinx_build(&args);
    assert!(result.status.success(), "{test}: {}", stderr_of(&result));
    assert_eq!(stderr_of(&result), "", "{test}");
    out
}

/// Sphinx's `Config.__init__` keeps a dotted `-D html_context.<key>=<text>`
/// as the `str` given (`config.py:309-312`), so `.buildinfo` hashes the text
/// as typed, whatever number it spells, and the `str` replaces an entry
/// `conf.py` gave another type, with no warning. The hashes are the ones
/// Sphinx 9.1.0 writes for the same `conf.py` and command line. Probed.
#[test]
fn a_dotted_html_context_override_hashes_the_text_given() {
    let typed =
        "project = 'Fixture'\nhtml_context = {'x': 1, 'y': True, 'z': [1, 2], 'w': {'v': 1}}\n";
    for (test, conf, defines, hash) in [
        (
            "t17a3-d-html-context-fixed",
            "project = 'Fixture'\n",
            &["html_context.x=0.00001"][..],
            "69e66bb85cad83a51b8604826fe329af",
        ),
        (
            "t17a3-d-html-context-short-exponent",
            "project = 'Fixture'\n",
            &["html_context.x=1e-7"][..],
            "8632daeb5d418bb25b3332372ab8d54f",
        ),
        (
            "t17a3-d-html-context-unsigned-exponent",
            "project = 'Fixture'\n",
            &["html_context.x=1e16"][..],
            "aba83ce55c1a8736a05ebc77b0c899e6",
        ),
        (
            "t17a3-d-html-context-typed",
            typed,
            &[
                "html_context.x=2",
                "html_context.y=yes",
                "html_context.z=a,b",
                "html_context.w=u",
                "html_context.a.b=c",
            ][..],
            "da25bc31eb8d89cf120ce1efdb743ef2",
        ),
    ] {
        let extra: Vec<&str> = defines.iter().flat_map(|define| ["-D", define]).collect();
        let out = quiet_build(test, conf, &extra);
        assert_eq!(buildinfo_config_hash(&out), hash, "{test}");
    }
}

/// A `conf.py` float literal whose exponent has a sign is Python's float:
/// Sphinx 9.1.0 prints nothing, and `.buildinfo` hashes the floats'
/// `repr()`. Probed.
#[test]
fn conf_py_floats_with_a_signed_exponent_are_read() {
    let out = quiet_build(
        "t17a3-conf-py-signed-exponent",
        "project = 'Fixture'\nhtml_context = {'a': 1e-05, 'b': -1e-07, 'c': 1e+16}\n",
        &[],
    );
    assert_eq!(
        buildinfo_config_hash(&out),
        "f61e44fa098222f240ecbdf89dc96a4d"
    );
}

// ---------------------------------------------------------------------------
// rst_prolog and rst_epilog (wave 5a T17b)
// ---------------------------------------------------------------------------

/// An `rst_prolog` this crate's `conf.py` reader cannot evaluate — here an
/// f-string, which Python formats and this reader, which runs no Python,
/// cannot — is a fatal configuration error naming the key, never dropped:
/// the prolog is read before every document, so a build without it would
/// read every document differently from Sphinx. The report is the one a
/// configuration that fails to load gets, on stderr and in the `-w`
/// file, exit 2; it names the `-D` override that builds all the same
/// ([`a_d_override_rescues_an_unevaluable_rst_epilog`]).
#[test]
fn unevaluable_rst_prolog_is_a_configuration_error() {
    let src = temp_source(
        "t17b-unevaluable-prolog",
        &[
            (
                "conf.py",
                "project = 'P'\nversion = '1.0'\n\
                 rst_prolog = f\"\"\"\n.. |version| replace:: {version}\n\"\"\"\n",
            ),
            ("index.rst", "Title\n=====\n\nVersion |version|.\n"),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir("t17b-unevaluable-prolog"), &[]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    assert_eq!(
        written,
        "\nConfiguration error!\n\n    sphinx.errors.ConfigError: cannot evaluate rst_prolog \
         (conf.py, line 3: unsupported expression): sphinx-ultra reads conf.py without \
         running it, so rst_prolog and rst_epilog must be string literals; set it with \
         -D rst_prolog=... to build without editing conf.py\n"
    );
    assert_eq!(String::from_utf8(result.stderr).unwrap(), written);
}

/// Sphinx never reads a value a `-D` override replaces, so a `-D` for an
/// `rst_epilog` this crate could not evaluate builds as Sphinx 9.1.0 does
/// with the same override: the epilog's record and the document's, exit
/// 0 (scratch `t17b-fix1/specs/d_rescue.json`). Without it, or with an
/// override of `rst_prolog` only, the configuration error names
/// `rst_epilog`.
#[test]
fn a_d_override_rescues_an_unevaluable_rst_epilog() {
    let src = temp_source(
        "t17b-d-rescue",
        &[
            (
                "conf.py",
                "project = 'P'\nversion = '1.0'\n\
                 rst_epilog = f\"\"\"\n.. |a| replace:: {version}\n\"\"\"\n",
            ),
            ("index.rst", "Index\n=====\n\nUses |a| and |b|.\n"),
        ],
    );
    let out = out_dir("t17b-d-rescue");
    let epilog = "rst_epilog=.. |a| replace:: A\n\n*unclosed epilog emphasis\n";
    let (result, written) = warning_stream(&src, &out, &["-D", epilog]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "\
<rst_epilogue>:3: WARNING: Inline emphasis start-string without end-string. [docutils]
<SRCDIR>/index.rst:4: ERROR: Undefined substitution referenced: \"b\". [docutils]
"
    );

    let error = "\nConfiguration error!\n\n    sphinx.errors.ConfigError: cannot evaluate \
                 rst_epilog (conf.py, line 3: unsupported expression): sphinx-ultra reads \
                 conf.py without running it, so rst_prolog and rst_epilog must be string \
                 literals; set it with -D rst_epilog=... to build without editing conf.py\n";
    for extra in [&[][..], &["-D", "rst_prolog=.. |b| replace:: B"][..]] {
        let (result, written) = warning_stream(&src, &out, extra);
        assert_eq!(result.status.code(), Some(2), "{extra:?}");
        assert_eq!(written, error, "{extra:?}");
    }
}

/// An assignment to an attribute of Sphinx's configuration goes through
/// `Config.__setattr__` into the instance's `__dict__`, which Python reads
/// before `Config.__getattr__` ever takes a `-D` override: Sphinx 9.1.0
/// reads `app.config.rst_prolog = ...` in `setup(app)` over `-D
/// rst_prolog=...`, and `config.rst_epilog += ...` in a `config-inited`
/// handler extends the override as it extends `conf.py`'s value, exit 0
/// with and without the override (scratch `t17b-fix2/specs/attr_setup*`,
/// `attr_inited_lit*`). So it does when the statement binds the name as
/// well, in a chained assignment or a one-line `for` body
/// (`t17b-fix3/specs/chain_local_attr*`, `chain_global_attr*`,
/// `for_body_attr*`), and for an item of the configuration, which
/// `Config.__setitem__` sets as an attribute, and a `setattr` call
/// (`item_setup*`, `setattr_setup*`); a `del` of the attribute crashes
/// Sphinx, exit 2 (`del_attr_setup*`). This crate evaluates none of them,
/// so each is the configuration error with the override as without it,
/// naming the line of the binding no override replaces, and the error
/// offers no `-D`. A function's `global` binding of the module's name, or
/// an item of `globals()`, is not such a binding: `Config.__getattr__`
/// takes the override before the namespace that binding changes, and the
/// override builds as Sphinx does (`t17b-fix2/specs/global_setup_D.json`,
/// `t17b-fix3/specs/globals_setup_D.json`). An item of `vars()` in a
/// function binds nothing, and the build reads no prolog, as Sphinx's
/// (`vars_setup.json`).
#[test]
fn an_attribute_binding_is_fatal_even_with_a_d_override() {
    let index = ("index.rst", "T\n=\n\nText |a| |b|.\n");
    let attribute = "an assignment to an attribute of that name";
    let cases = [
        (
            "t17b-attr-setup",
            "extensions = []\ndef setup(app):\n    \
             app.config.rst_prolog = '.. |a| replace:: A\\n'\n",
            "rst_prolog",
            3,
            attribute,
        ),
        (
            "t17b-attr-inited",
            "extensions = []\nrst_epilog = '.. |c| replace:: C\\n'\n\
             def f(app, config):\n    config.rst_epilog += '.. |a| replace:: A\\n'\n\
             def setup(app):\n    app.connect('config-inited', f)\n",
            "rst_epilog",
            4,
            attribute,
        ),
        // After a binding of the key an override replaces, which neither
        // hides the attribute's nor is reported before it
        // (`mixed_top_attr*.json`: Sphinx reads the attribute's, exit 0).
        (
            "t17b-attr-after-top",
            "extensions = []\nx = 'C'\nrst_prolog = f'.. |c| replace:: {x}\\n'\n\
             def setup(app):\n    app.config.rst_prolog = '.. |a| replace:: A\\n'\n",
            "rst_prolog",
            5,
            attribute,
        ),
        (
            "t17b-chain-local-attr",
            "extensions = []\ndef setup(app):\n    \
             rst_prolog = app.config.rst_prolog = '.. |a| replace:: A\\n'\n",
            "rst_prolog",
            3,
            attribute,
        ),
        // The `global` binding on the same line is one an override
        // replaces; the attribute's is reported.
        (
            "t17b-chain-global-attr",
            "extensions = []\ndef setup(app):\n    global rst_prolog\n    \
             rst_prolog = app.config.rst_prolog = '.. |a| replace:: A\\n'\n",
            "rst_prolog",
            4,
            attribute,
        ),
        (
            "t17b-for-body-attr",
            "extensions = []\ndef setup(app):\n    \
             for rst_prolog in ['.. |a| replace:: A\\n']: app.config.rst_prolog = rst_prolog\n",
            "rst_prolog",
            3,
            attribute,
        ),
        (
            "t17b-item-setup",
            "extensions = []\ndef setup(app):\n    \
             app.config['rst_prolog'] = '.. |a| replace:: A\\n'\n",
            "rst_prolog",
            3,
            "an assignment to an item of that name",
        ),
        (
            "t17b-setattr-setup",
            "extensions = []\ndef setup(app):\n    \
             setattr(app.config, 'rst_prolog', '.. |a| replace:: A\\n')\n",
            "rst_prolog",
            3,
            "a setattr call with that name",
        ),
        (
            "t17b-del-attr-setup",
            "extensions = []\ndef setup(app):\n    del app.config.rst_prolog\n",
            "rst_prolog",
            3,
            "a del of an attribute of that name",
        ),
    ];
    for (name, conf, key, line, reason) in cases {
        let src = temp_source(name, &[("conf.py", conf), index]);
        let out = out_dir(name);
        let error = format!(
            "\nConfiguration error!\n\n    sphinx.errors.ConfigError: cannot evaluate {key} \
             (conf.py, line {line}: {reason}): sphinx-ultra reads conf.py without running it, \
             so rst_prolog and rst_epilog must be string literals; conf.py changes an attribute \
             or item of that name, which a -D {key}=... override does not replace, so edit \
             conf.py\n"
        );
        let override_ = format!("{key}=.. |b| replace:: B\n");
        for extra in [&["-D", override_.as_str()][..], &[][..]] {
            let (result, written) = warning_stream(&src, &out, extra);
            assert_eq!(
                result.status.code(),
                Some(2),
                "{name} {extra:?}: {}",
                stderr_of(&result)
            );
            assert_eq!(written, error, "{name} {extra:?}");
            // After the reader's notices of the statements it dropped.
            assert!(
                srcdir_spelled(&result.stderr, &src).ends_with(&error),
                "{name} {extra:?}: {}",
                stderr_of(&result)
            );
        }
    }

    let undefined = |name: &str| {
        format!("<SRCDIR>/index.rst:4: ERROR: Undefined substitution referenced: \"{name}\". [docutils]\n")
    };
    let override_ = ["-D", "rst_prolog=.. |b| replace:: B\n"];
    let boundaries = [
        (
            "t17b-global-setup",
            "extensions = []\ndef setup(app):\n    global rst_prolog\n    \
             rst_prolog = '.. |a| replace:: A\\n'\n",
            &override_[..],
            undefined("a"),
        ),
        (
            "t17b-globals-setup",
            "extensions = []\ndef setup(app):\n    \
             globals()['rst_prolog'] = '.. |a| replace:: A\\n'\n",
            &override_[..],
            undefined("a"),
        ),
        (
            "t17b-vars-setup",
            "extensions = []\ndef setup(app):\n    \
             vars()['rst_prolog'] = '.. |a| replace:: A\\n'\n",
            &[][..],
            undefined("a") + &undefined("b"),
        ),
    ];
    for (name, conf, extra, expected) in boundaries {
        let src = temp_source(name, &[("conf.py", conf), index]);
        let (result, written) = warning_stream(&src, &out_dir(name), extra);
        assert!(result.status.success(), "{name}: {}", stderr_of(&result));
        assert_eq!(written, expected, "{name}");
    }
}

/// A statement that binds `rst_epilog` and another name is the
/// configuration error for the key, and the reader drops what else it
/// binds, as it drops any statement it cannot read: with a `-D` override
/// that lifts the error, the build reads the override as Sphinx 9.1.0
/// does, and the dropped `html_title` still gets its notice, as the next
/// line's `html_short_title` does (scratch `t17b-fix3/specs/chain_title*`).
/// Without the override, the notice comes before the error.
#[test]
fn a_statement_binding_rst_epilog_and_another_name_notices_the_other() {
    let src = temp_source(
        "t17b-chain-title",
        &[
            (
                "conf.py",
                "project = 'P'\nhtml_title = rst_epilog = f'{project} docs'\n\
                 html_short_title = f'{project}'\n",
            ),
            ("index.rst", "T\n=\n\nText |a| |b|.\n"),
        ],
    );
    let out = out_dir("t17b-chain-title");
    // The reader's notices name `conf.py` by the path the build was given.
    let notice = "conf.py:2: unsupported statement dropped: \
                  html_title = rst_epilog = f'{project} docs'\n";
    let short_title = "conf.py:3: unsupported value for 'html_short_title' dropped \
                       (unsupported expression): f'{project}'\n";

    let (result, written) = warning_stream(&src, &out, &["-D", "rst_epilog=.. |b| replace:: B\n"]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "<SRCDIR>/index.rst:4: ERROR: Undefined substitution referenced: \"a\". [docutils]\n"
    );
    let stderr = stderr_of(&result);
    let at = |text: &str| {
        stderr
            .find(text)
            .unwrap_or_else(|| panic!("{text}: {stderr}"))
    };
    assert!(at(notice) < at(short_title), "{stderr}");

    let (result, written) = warning_stream(&src, &out, &[]);
    assert_eq!(
        result.status.code(),
        Some(2),
        "stderr: {}",
        stderr_of(&result)
    );
    let error = "\nConfiguration error!\n\n    sphinx.errors.ConfigError: cannot evaluate \
                 rst_epilog (conf.py, line 2: an assignment it cannot evaluate): sphinx-ultra \
                 reads conf.py without running it, so rst_prolog and rst_epilog must be string \
                 literals; set it with -D rst_epilog=... to build without editing conf.py\n";
    assert_eq!(written, error);
    let stderr = stderr_of(&result);
    assert!(
        stderr.contains(notice) && stderr.ends_with(error),
        "{stderr}"
    );
}

/// A backslash that ends a line of a string literal removes the line
/// break, as Python reads it: the epilog opens with its substitution
/// definition, and Sphinx 9.1.0 prints nothing (scratch `t17b/esc`).
#[test]
fn an_escaped_line_break_in_rst_epilog_is_removed() {
    let src = temp_source(
        "t17b-esc",
        &[
            (
                "conf.py",
                "project = 'P'\nrst_epilog = \"\"\"\\\n.. |a| replace:: A\n\"\"\"\n",
            ),
            ("index.rst", "T\n=\n\nText |a|.\n"),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir("t17b-esc"), &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, "");
}

/// A true `rst_prolog` or `rst_epilog` that is no `str` crashes Sphinx
/// 9.1.0 (decision 13): `check_confval_types` warns about its type, and
/// the reader raises `AttributeError` on its `splitlines` as it decorates
/// the first document it reads (`a`, of `a`, `b` and `index`), before any
/// of its records: exit 2, nothing on stdout under `-q`, and the same
/// stream on stderr and in the `-w` file (scratch
/// `t17b-fix1/specs/crash_*.json`, `stdout/so.out`; Sphinx `-j 1`).
#[test]
fn a_true_mistyped_prolog_or_epilog_crashes_the_first_read() {
    let documents = [
        (
            "index.rst",
            "Index\n=====\n\n.. toctree::\n\n   a\n   b\n\nText :nope:`x`.\n",
        ),
        ("a.rst", "A\n=\n\nText :nope:`y`.\n"),
        ("b.rst", "B\n=\n\nText.\n"),
    ];
    for (test, conf, key, type_name) in [
        ("t17b-crash-int", "rst_prolog = 1\n", "rst_prolog", "int"),
        (
            "t17b-crash-bool",
            "rst_prolog = True\n",
            "rst_prolog",
            "bool",
        ),
        (
            "t17b-crash-list",
            "rst_epilog = ['x']\n",
            "rst_epilog",
            "list",
        ),
        (
            "t17b-crash-dict",
            "rst_epilog = {'a': 1}\n",
            "rst_epilog",
            "dict",
        ),
    ] {
        let conf = format!("project = 'P'\n{conf}");
        let mut files = vec![("conf.py", conf.as_str())];
        files.extend(documents);
        let src = temp_source(test, &files);
        let out = out_dir(test);
        let log = out.with_extension("w");
        let result = bin()
            .arg(&src)
            .arg(&out)
            .args(["-q", "-w"])
            .arg(&log)
            .output()
            .expect("binary should run");
        assert_eq!(
            result.status.code(),
            Some(2),
            "{test}: {}",
            stderr_of(&result)
        );
        assert_eq!(result.stdout, b"", "{test}");
        let written = srcdir_spelled(&std::fs::read(&log).unwrap(), &src);
        assert_eq!(srcdir_spelled(&result.stderr, &src), written, "{test}");
        assert_eq!(
            written,
            format!(
                "WARNING: The config value `{key}' has type `{type_name}'; expected `str'.\n{}",
                crash_report(
                    None,
                    &format!("AttributeError: '{type_name}' object has no attribute 'splitlines'"),
                    Some("<SRCDIR>/a.rst")
                )
            ),
            "{test}"
        );
    }

    // The records Sphinx logs before it, the configuration's and the
    // first document's default role's, print first (scratch
    // `t17b-fix1/specs/crash_default_role3.json`).
    let conf = "project = 'P'\nhtml_logo = 'missing.png'\ndefault_role = 'nope'\nrst_prolog = 1\n";
    let mut files = vec![("conf.py", conf)];
    files.extend(documents);
    assert_eq!(
        crash_stream("t17b-crash-after-records", &files, &[], &[]),
        format!(
            "WARNING: logo file 'missing.png' does not exist\n\
             WARNING: The config value `rst_prolog' has type `int'; expected `str'.\n\
             <SRCDIR>/a.rst: WARNING: default role nope not found\n{}",
            crash_report(
                None,
                "AttributeError: 'int' object has no attribute 'splitlines'",
                Some("<SRCDIR>/a.rst")
            )
        )
    );
}

/// A hyperlink target's location is docutils' `abs_line_number()`, its
/// line in the decorated input, so `rst_prolog`'s lines and the blank line
/// after them move it down: Sphinx 9.1.0 (`-q -w`, `-j 1`, scratch
/// `t17b/specs/a1_e2e.json`) prints the target on `index.rst` line 4 at
/// line 6, and the duplicate label on `z.rst` line 6 at line 8 (wave 5a
/// final review A1).
#[test]
fn explicit_target_lines_count_the_prolog() {
    let src = temp_source(
        "t17b-target-lines",
        &[
            (
                "conf.py",
                "project = 'P'\nrst_prolog = '.. |p| replace:: P\\n'\n",
            ),
            (
                "index.rst",
                "Title\n=====\n\n.. _x: y_\n\n.. toctree::\n\n   a\n   z\n",
            ),
            ("a.rst", ".. _dup:\n\nA\n=\n\nText.\n"),
            ("z.rst", "Z\n=\n\nText.\n\n.. _dup:\n"),
        ],
    );
    let (result, written) = warning_stream(&src, &out_dir("t17b-target-lines"), &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(
        written,
        "\
<SRCDIR>/index.rst:6: ERROR: Indirect hyperlink target \"x\" (id=\"x\") refers to target \"y\", which does not exist. [docutils]
<SRCDIR>/z.rst:8: WARNING: duplicate label dup, other instance in <SRCDIR>/a.rst
"
    );
}

/// Sphinx reads the first document of each read before any other has
/// registered a role, so only that document's leading field-marker lines,
/// which come before `rst_prolog`, miss the prolog's role: `a` on a cold
/// build, and `b`, the one document a warm build re-reads, once `b` is
/// touched (Sphinx 9.1.0, `-q`, cold then warm, scratch `t17b/incr`).
#[test]
fn the_first_document_read_misses_the_prolog_roles_in_its_field_lines() {
    let src = temp_source(
        "t17b-first-read",
        &[
            (
                "conf.py",
                "project = 'P'\nrst_prolog = '.. role:: red\\n'\n",
            ),
            ("index.rst", "Index\n=====\n\n.. toctree::\n\n   a\n   b\n"),
            ("a.rst", ":author: :red:`me`\n\nA\n=\n\nText.\n"),
            ("b.rst", ":author: :red:`me`\n\nB\n=\n\nText.\n"),
        ],
    );
    let records = |doc: &str| {
        format!(
            "<SRCDIR>/{doc}.rst:1: ERROR: Unknown interpreted text role \"red\". [docutils]\n\
             <SRCDIR>/{doc}.rst:1: WARNING: Bibliographic field \"author\"\n\
             must contain a single <paragraph>, not a <paragraph>. [docutils]\n"
        )
    };
    let out = out_dir("t17b-first-read");
    let (result, written) = warning_stream(&src, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, records("a"), "cold");
    touch(&src.join("b.rst"));
    let (result, written) = warning_stream(&src, &out, &[]);
    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert_eq!(written, records("b"), "warm, b touched");
}
