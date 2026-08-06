//! End-to-end tests that run the actual `sphinx-ultra` binary against fixture
//! projects and assert on exit codes, warnings, and the output tree.
//!
//! These tests lock in *current* behavior. Where current behavior is a known
//! ROADMAP M1 defect, the assertion documents it with a comment so the fix is
//! a deliberate test change, not an accident.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sphinx-ultra"))
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
    assert!(
        stderr_of(&result)
            .contains("index.rst:6: WARNING: toctree contains reference to nonexisting document 'nonexistent_page'"),
        "expected toctree warning with the entry's real line number, stderr: {}",
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
    assert!(
        stderr.contains(
            "index.rst:8: WARNING: toctree glob pattern 'missing*' didn't match any documents"
        ),
        "dead glob pattern must warn with its line, stderr: {stderr}"
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
