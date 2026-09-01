//! End-to-end tests that run the actual `sphinx-ultra` binary against fixture
//! projects and assert on exit codes, warnings, and the output tree.
//!
//! These tests lock in *current* behavior. Where current behavior is a known
//! ROADMAP M1 defect, the assertion documents it with a comment so the fix is
//! a deliberate test change, not an accident.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

    let run2 = sphinx_build(&[&s, &o]);
    assert!(
        stderr_of(&run2).contains("Cache hits: 3"),
        "an unchanged project reads nothing, stderr: {}",
        stderr_of(&run2)
    );

    // Adding -W must not wipe the cache, and must not fail the build over
    // warnings a warm build never re-emits.
    let run3 = sphinx_build(&[&s, &o, "-W"]);
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
    let run5 = sphinx_build(&[&s, &o, "-n"]);
    assert!(
        stderr_of(&run5).contains("Cache hits: 3"),
        "-n must not invalidate the cache, stderr: {}",
        stderr_of(&run5)
    );
    let run6 = sphinx_build(&[&s, &o]);
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
    assert!(
        stderr_of(&result).contains("with warnings treated as errors"),
        "sphinx 9.1 -W message, stderr: {}",
        stderr_of(&result)
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

    let run2 = sphinx_build(&[s, out.to_str().unwrap()]);
    assert!(
        stderr_of(&run2).contains("Cache hits: 2"),
        "compat mode is incremental by default (sphinx-build parity), stderr: {}",
        stderr_of(&run2)
    );

    let run3 = sphinx_build(&[s, out.to_str().unwrap(), "-E"]);
    assert!(
        stderr_of(&run3).contains("Cache hits: 0"),
        "-E discards the saved environment, stderr: {}",
        stderr_of(&run3)
    );

    let run4 = sphinx_build(&[s, out.to_str().unwrap(), "-a"]);
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

#[test]
fn directive_validation_off_switch() {
    let src = temp_source("dv-off", &[("index.rst", "Title\n=====\n\n.. note::\n")]);
    let out = out_dir("dv-off");
    let result = sphinx_build(&[
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "-D",
        "validate_directives=0",
    ]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    assert!(
        !stderr_of(&result).contains("Note directive requires content"),
        "-D validate_directives=0 must disable the pass, stderr: {}",
        stderr_of(&result)
    );
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
fn nitpicky_skips_python_and_external_refs() {
    let src = temp_source(
        "nitpicky-skips",
        &[(
            "index.rst",
            "Title\n=====\n\nCall :py:func:`missing.fn` and :func:`also.missing`.\nSee `docs <https://example.com>`_ and :doc:`https://example.com/page`.\n",
        )],
    );
    let out = out_dir("nitpicky-skips");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap(), "-n"]);

    assert!(result.status.success(), "stderr: {}", stderr_of(&result));
    let stderr = stderr_of(&result);
    assert!(
        !stderr.contains("unknown document") && !stderr.contains("undefined label"),
        "python-domain and external refs must not be reported broken, stderr: {stderr}"
    );
    let aggregate_count = stderr
        .matches("python-domain reference(s) not validated")
        .count();
    assert_eq!(
        aggregate_count, 1,
        "the unvalidatable-python-refs notice appears exactly once, stderr: {stderr}"
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
// exercised end to end without ever touching the network.
// ---------------------------------------------------------------------------

#[test]
fn intersphinx_resolves_a_cross_project_ref_and_reports_a_missing_external() {
    let out = out_dir("intersphinx");
    let src = fixture("intersphinx");
    let result = sphinx_build(&[src.to_str().unwrap(), out.to_str().unwrap()]);

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
    assert!(
        stderr.contains("build succeeded, 1 warning."),
        "the external miss is the only warning, stderr: {stderr}"
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
        "-D",
        "intersphinx_disabled_reftypes=std:label",
    ]);

    let stderr = stderr_of(&result);
    assert!(
        stderr.contains("index.rst:4: WARNING: undefined label: 'example' [ref.ref]"),
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
                "project = 'Bad'\nintersphinx_mapping = {'p': 'https://x/'}\n",
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
                "project = 'Empty'\nintersphinx_mapping = {'p': ('https://x/', ())}\n",
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
