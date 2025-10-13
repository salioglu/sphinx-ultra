//! Integration tests for include_patterns and exclude_patterns functionality.

use std::fs;
use tempfile::TempDir;

use sphinx_ultra::config::BuildConfig;
use sphinx_ultra::matching::{get_matching_files, pattern_match, translate_pattern};
use sphinx_ultra::python_config::PythonConfigParser;

#[test]
fn test_sphinx_compatibility_default_patterns() {
    let config = BuildConfig::default();

    // Check Sphinx-compatible defaults
    assert_eq!(config.include_patterns, vec!["**"]);
    assert_eq!(config.exclude_patterns, Vec::<String>::new());
}

#[test]
fn test_pattern_translation_compatibility() {
    // Test patterns that should work the same as Sphinx
    assert_eq!(translate_pattern("*.rst"), "^[^/]*\\.rst$");
    assert_eq!(translate_pattern("**"), "^.*$");
    assert_eq!(
        translate_pattern("**/index.rst"),
        "^(?:[^/]+/)*index\\.rst$"
    );
    assert_eq!(
        translate_pattern("docs/**/*.rst"),
        "^docs/(?:[^/]+/)*[^/]*\\.rst$"
    );

    // Test character classes (fnmatch style)
    assert_eq!(translate_pattern("[abc].rst"), "^[abc]\\.rst$");
    assert_eq!(translate_pattern("[!_]*.rst"), "^[^_][^/]*\\.rst$");
}

#[test]
fn test_pattern_matching_sphinx_examples() {
    // Test cases from Sphinx documentation

    // Basic wildcards
    assert!(pattern_match("index.rst", "*.rst").unwrap());
    assert!(pattern_match("chapter1.rst", "chapter?.rst").unwrap());
    assert!(!pattern_match("chapter10.rst", "chapter?.rst").unwrap());

    // Double star patterns
    assert!(pattern_match("docs/api/module.rst", "**/api/*.rst").unwrap());
    assert!(pattern_match("api/module.rst", "**/api/*.rst").unwrap());
    assert!(pattern_match("deep/nested/api/module.rst", "**/api/*.rst").unwrap());

    // Exclude patterns
    assert!(pattern_match("_build/index.html", "_build/**").unwrap());
    assert!(pattern_match("_build/html/index.html", "_build/**").unwrap());
    assert!(pattern_match("Thumbs.db", "Thumbs.db").unwrap());

    // Directory patterns
    assert!(pattern_match("docs/index.rst", "docs/**").unwrap());
    assert!(!pattern_match("src/code.py", "docs/**").unwrap());
}

#[test]
fn test_file_discovery_with_patterns() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create test file structure
    fs::create_dir_all(base_path.join("docs/api")).unwrap();
    fs::create_dir_all(base_path.join("_build/html")).unwrap();
    fs::create_dir_all(base_path.join("src")).unwrap();

    fs::write(base_path.join("index.rst"), "Main index").unwrap();
    fs::write(base_path.join("docs/guide.rst"), "User guide").unwrap();
    fs::write(base_path.join("docs/api/reference.rst"), "API reference").unwrap();
    fs::write(base_path.join("_build/html/index.html"), "Built HTML").unwrap();
    fs::write(base_path.join("src/code.py"), "Python code").unwrap();
    fs::write(base_path.join("README.md"), "Readme").unwrap();
    fs::write(base_path.join("Thumbs.db"), "Windows thumbnail").unwrap();

    // Test 1: Include all RST files
    let files = get_matching_files(base_path, &["**/*.rst".to_string()], &[]).unwrap();

    assert_eq!(files.len(), 3);
    let file_names: Vec<_> = files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy())
        .collect();
    assert!(file_names.contains(&"index.rst".into()));
    assert!(file_names.contains(&"guide.rst".into()));
    assert!(file_names.contains(&"reference.rst".into()));

    // Test 2: Include all files but exclude _build
    let files =
        get_matching_files(base_path, &["**".to_string()], &["_build/**".to_string()]).unwrap();

    // Should not include any files in _build directory
    assert!(!files.iter().any(|p| p.to_string_lossy().contains("_build")));

    // Test 3: Multiple exclude patterns
    let files = get_matching_files(
        base_path,
        &["**".to_string()],
        &[
            "_build/**".to_string(),
            "**/*.py".to_string(),
            "Thumbs.db".to_string(),
        ],
    )
    .unwrap();

    // Should exclude Python files, build artifacts, and Thumbs.db
    assert!(!files
        .iter()
        .any(|p| p.extension().is_some_and(|ext| ext == "py")));
    assert!(!files.iter().any(|p| p.file_name().unwrap() == "Thumbs.db"));
    assert!(!files.iter().any(|p| p.to_string_lossy().contains("_build")));

    // Test 4: Include only docs directory
    let files = get_matching_files(base_path, &["docs/**".to_string()], &[]).unwrap();

    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|p| p.to_string_lossy().contains("docs")));
}

#[test]
fn test_sphinx_built_in_excludes() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create files that should be excluded by default
    fs::create_dir_all(base_path.join(".git")).unwrap();
    fs::create_dir_all(base_path.join("__pycache__")).unwrap();
    fs::write(base_path.join(".git/config"), "git config").unwrap();
    fs::write(base_path.join("__pycache__/module.pyc"), "compiled python").unwrap();
    fs::write(base_path.join(".DS_Store"), "macOS metadata").unwrap();
    fs::write(base_path.join("index.rst"), "documentation").unwrap();

    // Test that built-in excludes work
    let default_excludes = vec![
        "_build/**".to_string(),
        "__pycache__/**".to_string(),
        ".git/**".to_string(),
        ".*/**".to_string(),
        "Thumbs.db".to_string(),
        ".DS_Store".to_string(),
    ];

    let files = get_matching_files(base_path, &["**".to_string()], &default_excludes).unwrap();

    // Should only include the RST file
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap(), "index.rst");
}

#[test]
fn test_conf_py_pattern_parsing() {
    let temp_dir = TempDir::new().unwrap();
    let conf_path = temp_dir.path().join("conf.py");

    // Create a conf.py with pattern configuration
    let conf_content = r#"
# Sphinx configuration

project = 'Test Project'
version = '1.0'

# File patterns
include_patterns = ['docs/**', '*.rst']
exclude_patterns = ['_build/**', '*.tmp', 'drafts/**']

# Extensions
extensions = ['sphinx.ext.autodoc']

html_theme = 'sphinx_rtd_theme'
"#;

    fs::write(&conf_path, conf_content).unwrap();

    // Parse the configuration
    let mut parser = PythonConfigParser::new().unwrap();
    let conf_config = parser.parse_conf_py(&conf_path).unwrap();
    let build_config = conf_config.to_build_config();

    // Verify patterns were parsed correctly
    assert_eq!(build_config.include_patterns, vec!["docs/**", "*.rst"]);
    assert_eq!(
        build_config.exclude_patterns,
        vec!["_build/**", "*.tmp", "drafts/**"]
    );
}

#[test]
fn test_conf_py_default_patterns() {
    let temp_dir = TempDir::new().unwrap();
    let conf_path = temp_dir.path().join("conf.py");

    // Create a minimal conf.py without pattern configuration
    let conf_content = r#"
project = 'Test Project'
version = '1.0'
html_theme = 'alabaster'
"#;

    fs::write(&conf_path, conf_content).unwrap();

    // Parse the configuration
    let mut parser = PythonConfigParser::new().unwrap();
    let conf_config = parser.parse_conf_py(&conf_path).unwrap();
    let build_config = conf_config.to_build_config();

    // Should have Sphinx-compatible defaults
    assert_eq!(build_config.include_patterns, vec!["**"]);
    assert_eq!(build_config.exclude_patterns, Vec::<String>::new());
}

#[test]
fn test_pattern_priority_exclude_over_include() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create test files
    fs::create_dir_all(base_path.join("drafts")).unwrap();
    fs::write(base_path.join("index.rst"), "Main file").unwrap();
    fs::write(base_path.join("drafts/unfinished.rst"), "Draft file").unwrap();

    // Include all RST files but exclude drafts directory
    let files = get_matching_files(
        base_path,
        &["**/*.rst".to_string()],  // Include all RST files
        &["drafts/**".to_string()], // Exclude drafts directory
    )
    .unwrap();

    // Should only include index.rst, not the draft
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap(), "index.rst");
}

#[test]
fn test_character_class_patterns() {
    // Test character classes like Sphinx supports
    assert!(pattern_match("a.rst", "[abc].rst").unwrap());
    assert!(pattern_match("b.rst", "[abc].rst").unwrap());
    assert!(pattern_match("c.rst", "[abc].rst").unwrap());
    assert!(!pattern_match("d.rst", "[abc].rst").unwrap());

    // Test negated character classes
    assert!(!pattern_match("_hidden.rst", "[!_]*.rst").unwrap());
    assert!(pattern_match("visible.rst", "[!_]*.rst").unwrap());
}

#[test]
fn test_cross_platform_path_handling() {
    // Test that paths work consistently across platforms
    // Note: Our normalize_path function converts backslashes to forward slashes
    let normalized_path = "docs/api/module.rst";

    // Both patterns should match the normalized path
    assert!(pattern_match(normalized_path, "**/api/*.rst").unwrap());
    assert!(pattern_match(normalized_path, "docs/**/*.rst").unwrap());

    // Test that Windows-style paths also work after normalization
    use sphinx_ultra::matching::normalize_path;
    use std::path::Path;

    let windows_path = Path::new("docs\\api\\module.rst");
    let normalized = normalize_path(windows_path);
    assert_eq!(normalized, "docs/api/module.rst");
    assert!(pattern_match(&normalized, "**/api/*.rst").unwrap());
}
