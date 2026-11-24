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

/// The include dependency + orphan wiring, end to end (wave 4.5 T12,
/// [INC PROBE 6] matrix): a cold build records the include targets as
/// dependencies, a warm build reads nothing, touching an included `.rst`
/// re-reads the includer AND the included doc (it is discovered and read
/// standalone too), and an included-only doc never earns the orphan
/// warning. Divergence note: touching an included `.txt` re-reads the
/// includer and the `.txt`'s own document — sphinx's default
/// `source_suffix` does not read `.txt` standalone, this crate's wider
/// discovery does.
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

    // Warm no-op: all four discovered documents (snippet.txt included —
    // this crate's discovery is wider than sphinx's) hit the cache, and
    // the orphan check replayed from the persisted env stays quiet.
    let run2 = build(&src, &out, &["--incremental"]);
    let stderr2 = stderr_of(&run2);
    assert!(
        stderr2.contains("Cache hits: 4"),
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
        stderr_of(&run3).contains("Cache hits: 2"),
        "touch part.rst re-reads a + part, stderr: {}",
        stderr_of(&run3)
    );

    // Touch the included .txt: outdates the includer; the .txt's own
    // document is the divergence noted above.
    let snippet = src.join("snippet.txt");
    let bytes = std::fs::read(&snippet).unwrap();
    std::fs::write(&snippet, &bytes).unwrap();
    let run4 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run4).contains("Cache hits: 2"),
        "touch snippet.txt re-reads a + snippet, stderr: {}",
        stderr_of(&run4)
    );

    // The re-reads settled every dependency.
    let run5 = build(&src, &out, &["--incremental"]);
    assert!(
        stderr_of(&run5).contains("Cache hits: 4"),
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

/// This crate's discovery is wider than Sphinx's — it admits `.md` and
/// `.txt` where Sphinx's default `source_suffix` is `.rst` alone — and the
/// orphan check is where that difference reaches the user: a `README.md`
/// must not earn a `toc.not_included` warning Sphinx never emits. An
/// orphaned `.rst` still does.
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

/// Two files mapping to one docname are not two documents: every piece of
/// per-document state downstream of discovery is keyed by docname alone, so
/// one of them has to go (`Project.discover`, `project.py:64-79`), and which
/// one must not depend on directory walk order.
///
/// It must go **silently** here, though. Sphinx's default `source_suffix` is
/// `.rst` alone, so it never discovers the `.md`/`.txt` sibling and builds
/// this tree clean; warning about a collision Sphinx cannot see would fail
/// `-W` on a build Sphinx passes. The report is reserved for a collision
/// between two files Sphinx would both have read.
#[test]
fn two_files_for_one_docname_resolve_silently_and_deterministically() {
    let src = out_dir("duplicate-docname-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("conf.py"), "project = 'p'\n").unwrap();
    std::fs::write(
        src.join("index.rst"),
        "Index\n=====\n\n.. toctree::\n\n   page\n",
    )
    .unwrap();
    std::fs::write(src.join("page.rst"), "Page RST\n========\n\nrst body.\n").unwrap();
    std::fs::write(src.join("page.md"), "Page MD\n=======\n\nmd body.\n").unwrap();
    std::fs::write(src.join("page.txt"), "Page TXT\n========\n\ntxt body.\n").unwrap();

    let out = out_dir("duplicate-docname-out");
    let first = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-W"]);
    let stderr = stderr_of(&first);

    assert!(
        !stderr.contains("multiple files found for the document"),
        "sphinx sees no collision here, so neither may we, stderr: {stderr}"
    );
    assert!(
        first.status.success(),
        "sphinx builds this clean, so -W must pass, stderr: {stderr}"
    );

    // The winner is `.rst` — first in the discovery suffix order — and is
    // stable across builds, warm or cold. The defect this pins was an
    // output page that alternated between the two sources.
    let page = out.join("page.html");
    let cold = std::fs::read_to_string(&page).unwrap();
    assert!(cold.contains("rst body"), "first suffix wins: {cold}");
    for _ in 0..2 {
        let again = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-W"]);
        assert!(again.status.success(), "stderr: {}", stderr_of(&again));
        assert_eq!(
            std::fs::read_to_string(&page).unwrap(),
            cold,
            "the rendered page must not alternate between the two sources"
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

/// The resolve record of `d.rst` in [`touched_document_reemits_its_records`].
const UNDEFINED_LABEL_RECORD: &str =
    "<SRCDIR>/d.rst:4: WARNING: undefined label: 'nowhere' [ref.ref]\n";

/// Review Focus 1: a warm build re-reads the touched document and prints
/// its read records — all of them, in the cold build's order — and none of
/// the untouched documents' read records. Resolve records are recomputed
/// for every document on every build, so `d`'s `undefined label` follows,
/// in sorted docname order: decision 6's documented divergence from
/// `-b html`, which writes, and so resolves, only the outdated `b`
/// (probed: Sphinx 9.1.0's warm `-b html` prints `b`'s two records alone,
/// its `-b dummy` prints them and then `d`'s, as here).
#[test]
fn touched_document_reemits_its_records() {
    let src = temp_source(
        "t8-touch",
        &[
            (
                "index.rst",
                "Index\n=====\n\n.. toctree::\n\n   a\n   b\n   d\n\nSee *this.\n",
            ),
            ("a.rst", "A title\n=====\n\nBody.\n"),
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
             start-string without end-string. [docutils]\n{UNDEFINED_LABEL_RECORD}"
        )
    );

    touch(&src.join("b.rst"));
    let (warm, written) = warning_stream(&src, &out, &[]);
    assert!(warm.status.success(), "stderr: {}", stderr_of(&warm));
    assert_eq!(
        written,
        format!("{TOUCHED_B_RECORDS}{UNDEFINED_LABEL_RECORD}")
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
/// `build finished with problems, …` and no final location.
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
