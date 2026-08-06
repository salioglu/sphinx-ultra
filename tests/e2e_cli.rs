//! End-to-end tests that run the actual `sphinx-ultra` binary against fixture
//! projects and assert on exit codes, warnings, and the output tree.
//!
//! These tests lock in *current* behavior. Where current behavior is a known
//! ROADMAP M1 defect (e.g. builds with errors exit 0), the assertion documents
//! it with a comment so the fix is a deliberate test change, not an accident.

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
    assert!(html.contains("Welcome"), "index.html should carry the title text");
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
        stderr_of(&result).contains("nonexisting document 'nonexistent_page'"),
        "expected toctree warning, stderr: {}",
        stderr_of(&result)
    );
}

#[test]
fn fail_on_warning_exits_one() {
    let out = out_dir("fail-on-warning");
    let result = build(&fixture("basic_missing_ref"), &out, &["-W"]);

    assert_eq!(result.status.code(), Some(1), "-W must turn warnings into a failing exit");
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
fn stats_prints_source_file_count() {
    let result = bin()
        .arg("stats")
        .arg("--source")
        .arg(fixture("basic"))
        .output()
        .expect("binary should run");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("Source files: 2"),
        "stats stdout: {stdout}"
    );
}
